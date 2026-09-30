#![cfg(feature = "test-physical-replication")]

use std::error::Error;
use std::net::{IpAddr, Ipv4Addr};
use std::process::Command;
use std::time::Duration;

use console_platform_db::durability::{
    AuditCustodyError, AuditRowRef, ConfirmationError, ExpectedWriter, PhysicalTopology,
    confirm_post_commit, observe_audited_row_post_commit, resume_audited_row_observation,
};
use sqlx::{Acquire, PgPool, Row};

#[tokio::test]
async fn confirms_only_a_fenced_physical_prefix() -> Result<(), Box<dyn Error>> {
    let primary = PgPool::connect(&std::env::var("V1_PRIMARY_DSN")?).await?;
    let standby = PgPool::connect(&std::env::var("V1_STANDBY_DSN")?).await?;
    let observer_primary = PgPool::connect(&std::env::var("V1_OBSERVER_PRIMARY_DSN")?).await?;
    let observer_standby = PgPool::connect(&std::env::var("V1_OBSERVER_STANDBY_DSN")?).await?;
    let address = IpAddr::V4(Ipv4Addr::LOCALHOST);
    let topology = PhysicalTopology {
        primary_address: address,
        primary_port: std::env::var("V1_PRIMARY_PORT")?.parse()?,
        standby_address: address,
        standby_port: std::env::var("V1_STANDBY_PORT")?.parse()?,
        standby_application_name: "v1_standby".to_owned(),
    };
    let row = sqlx::query(
        "SELECT (SELECT system_identifier::text FROM pg_control_system()) AS system_id, \
         pg_walfile_name(pg_current_wal_insert_lsn()) AS wal_file",
    )
    .fetch_one(&primary)
    .await?;
    let system_identifier: String = row.try_get("system_id")?;
    let wal_file: String = row.try_get("wal_file")?;
    let timeline_id = u32::from_str_radix(&wal_file[..8], 16)?;
    let writer = ExpectedWriter {
        writer_epoch: 7,
        system_identifier,
        timeline_id,
        primary_id: "disposable-primary".to_owned(),
    };

    // This test-only witness row is real replicated PostgreSQL state; no
    // production lease/fencing authority is installed by the test.
    sqlx::raw_sql(include_str!(
        "../migrations/0236_platform_durability_epoch.sql"
    ))
    .execute(&primary)
    .await?;
    let marker_count: i64 = sqlx::query_scalar("SELECT count(*) FROM platform_durability_epoch")
        .fetch_one(&primary)
        .await?;
    assert_eq!(
        marker_count, 0,
        "migration must not fabricate a writer lease"
    );
    let mut runtime_tx = primary.begin().await?;
    sqlx::query("SET LOCAL ROLE console_rt")
        .execute(&mut *runtime_tx)
        .await?;
    assert!(
        sqlx::query_scalar::<_, i64>("SELECT writer_epoch FROM platform_durability_epoch")
            .fetch_optional(&mut *runtime_tx)
            .await
            .is_err(),
        "ordinary runtime role must not read infrastructure epoch marker"
    );
    runtime_tx.rollback().await?;
    sqlx::query(
        "INSERT INTO platform_durability_epoch \
         (writer_epoch, system_identifier, timeline_id, primary_id) VALUES ($1, $2, $3, $4)",
    )
    .bind(writer.writer_epoch)
    .bind(&writer.system_identifier)
    .bind(i64::from(writer.timeline_id))
    .bind(&writer.primary_id)
    .execute(&primary)
    .await?;
    sqlx::query("CREATE TABLE durability_physical_probe (id bigint PRIMARY KEY)")
        .execute(&primary)
        .await?;
    // The fork's org_id is the Company RLS cell. The real observer login is
    // neither the table owner nor a BYPASSRLS role on either connection.
    sqlx::raw_sql(
        "CREATE TABLE audit_events (
            id uuid PRIMARY KEY, org_id uuid NOT NULL, action text NOT NULL,
            target_type text NOT NULL, target_id text NOT NULL
        );
        ALTER TABLE audit_events ENABLE ROW LEVEL SECURITY;
        ALTER TABLE audit_events FORCE ROW LEVEL SECURITY;
        ALTER TABLE audit_events SET (autovacuum_enabled = false);
        CREATE POLICY org_isolation ON audit_events
            USING (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid);
        GRANT SELECT ON platform_durability_epoch, audit_events TO durability_observer;",
    )
    .execute(&primary)
    .await?;
    let observer_role = sqlx::query(
        "SELECT r.rolsuper, r.rolbypassrls,
                r.oid = c.relowner AS owns_audit
         FROM pg_roles r CROSS JOIN pg_class c
         WHERE r.rolname = 'durability_observer' AND c.oid = 'audit_events'::regclass",
    )
    .fetch_one(&primary)
    .await?;
    assert!(!observer_role.try_get::<bool, _>("rolsuper")?);
    assert!(!observer_role.try_get::<bool, _>("rolbypassrls")?);
    assert!(!observer_role.try_get::<bool, _>("owns_audit")?);
    let observer_user: String = sqlx::query_scalar("SELECT current_user")
        .fetch_one(&observer_primary)
        .await?;
    assert_eq!(observer_user, "durability_observer");

    // A distinct direct standby can confirm an already committed WAL prefix.
    sqlx::query("INSERT INTO durability_physical_probe VALUES (1)")
        .execute(&primary)
        .await?;
    let confirmed = confirm_post_commit(
        &primary,
        &standby,
        &topology,
        &writer,
        Duration::from_secs(3),
    )
    .await?;
    assert_eq!(confirmed.writer_epoch(), 7);
    assert_eq!(confirmed.system_identifier(), writer.system_identifier);

    // Two DSNs pointed at the same PostgreSQL process cannot imitate sites.
    assert!(matches!(
        confirm_post_commit(
            &primary,
            &primary,
            &topology,
            &writer,
            Duration::from_millis(250)
        )
        .await,
        Err(ConfirmationError::Topology(_))
    ));
    let stale = ExpectedWriter {
        writer_epoch: 6,
        ..writer.clone()
    };
    assert!(matches!(
        confirm_post_commit(
            &primary,
            &standby,
            &topology,
            &stale,
            Duration::from_millis(250)
        )
        .await,
        Err(ConfirmationError::Epoch(_))
    ));
    let wrong_timeline = ExpectedWriter {
        timeline_id: writer.timeline_id + 1,
        ..writer.clone()
    };
    assert!(matches!(
        confirm_post_commit(
            &primary,
            &standby,
            &topology,
            &wrong_timeline,
            Duration::from_millis(250)
        )
        .await,
        Err(ConfirmationError::Epoch(_))
    ));
    let wrong_system = ExpectedWriter {
        system_identifier: format!("{}0", writer.system_identifier),
        ..writer.clone()
    };
    assert!(matches!(
        confirm_post_commit(
            &primary,
            &standby,
            &topology,
            &wrong_system,
            Duration::from_millis(250)
        )
        .await,
        Err(ConfirmationError::Topology(_))
    ));
    assert!(
        sqlx::query("UPDATE platform_durability_epoch SET writer_epoch = 6 WHERE singleton")
            .execute(&primary)
            .await
            .is_err(),
        "marker cannot move backward"
    );

    // Local COMMIT can return while replay is paused. Neither synchronous
    // settings nor primary visibility suffice for confirmation.
    sqlx::query("SELECT pg_wal_replay_pause()")
        .execute(&standby)
        .await?;
    wait_until_paused(&standby).await?;
    let mut tx = primary.begin().await?;
    sqlx::query("SET LOCAL synchronous_commit = 'local'")
        .execute(&mut *tx)
        .await?;
    sqlx::query("INSERT INTO durability_physical_probe VALUES (2)")
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    let lagged = confirm_post_commit(
        &primary,
        &standby,
        &topology,
        &writer,
        Duration::from_millis(250),
    )
    .await;
    assert!(
        matches!(lagged, Err(ConfirmationError::Deadline(_))),
        "{lagged:?}"
    );
    sqlx::query("SELECT pg_wal_replay_resume()")
        .execute(&standby)
        .await?;
    confirm_post_commit(
        &primary,
        &standby,
        &topology,
        &writer,
        Duration::from_secs(3),
    )
    .await?;

    // PostgreSQL can cancel synchronous waiting after local COMMIT. The row
    // still exists, so caller reconciliation can confirm the stable operation.
    sqlx::query("SELECT pg_wal_replay_pause()")
        .execute(&standby)
        .await?;
    wait_until_paused(&standby).await?;
    let company_a = uuid::Uuid::parse_str("11111111-1111-4111-8111-111111111111")?;
    let company_b = uuid::Uuid::parse_str("22222222-2222-4222-8222-222222222222")?;
    let audit = AuditRowRef {
        id: uuid::Uuid::new_v4(),
        org_id: company_a,
        action: "durability.write".to_owned(),
        target_type: "durability_probe".to_owned(),
        target_id: "3".to_owned(),
    };
    let mut conn = primary.acquire().await?;
    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *conn)
        .await?;
    let mut tx = conn.begin().await?;
    sqlx::query("INSERT INTO durability_physical_probe VALUES (3)")
        .execute(&mut *tx)
        .await?;
    sqlx::query(
        "INSERT INTO audit_events (id, org_id, action, target_type, target_id)
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(audit.id)
    .bind(audit.org_id)
    .bind(&audit.action)
    .bind(&audit.target_type)
    .bind(&audit.target_id)
    .execute(&mut *tx)
    .await?;
    let commit = tx.commit();
    let cancel = async {
        for _ in 0..100 {
            let waiting: bool = sqlx::query_scalar(
                "SELECT EXISTS (SELECT 1 FROM pg_stat_activity \
                 WHERE pid = $1 AND wait_event = 'SyncRep')",
            )
            .bind(pid)
            .fetch_one(&primary)
            .await?;
            if waiting {
                let canceled: bool = sqlx::query_scalar("SELECT pg_cancel_backend($1)")
                    .bind(pid)
                    .fetch_one(&primary)
                    .await?;
                assert!(canceled);
                return Ok::<(), Box<dyn Error>>(());
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        Err("COMMIT did not enter the synchronous replication wait".into())
    };
    let (commit_result, cancel_result) = tokio::join!(commit, cancel);
    cancel_result?;
    // PostgreSQL may report COMMIT success after cancellation despite the
    // standby still being paused; either response requires independent proof.
    eprintln!(
        "canceled SyncRep wait: COMMIT returned success={}",
        commit_result.is_ok()
    );
    let primary_persisted: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM durability_physical_probe WHERE id = 3)")
            .fetch_one(&primary)
            .await?;
    assert!(
        primary_persisted,
        "local COMMIT must survive SyncRep cancellation"
    );
    let post_commit_insert_lsn: String =
        sqlx::query_scalar("SELECT pg_current_wal_insert_lsn()::text")
            .fetch_one(&primary)
            .await?;
    let pending = match observe_audited_row_post_commit(
        &observer_primary,
        &observer_standby,
        &topology,
        &writer,
        &audit,
        Duration::from_millis(250),
    )
    .await
    {
        Err(AuditCustodyError::Pending(pending)) => pending,
        other => panic!("paused standby must leave exact row pending: {other:?}"),
    };
    let fixed_bound = pending.upper_bound_lsn().to_owned();
    // Only this opaque in-process token can be resumed. No identity-only
    // reconstruction is claimed after process loss; that outcome is UNKNOWN.
    assert!(
        lsn_at_least(&primary, &fixed_bound, &post_commit_insert_lsn).await?,
        "audit observation must capture a post-COMMIT primary INSERT bound"
    );
    // Every continuation must recheck the epoch marker before any progress
    // decision. A transient failure there must retain the same opaque token
    // and the exact SQL error, not sample a newer WAL bound on retry.
    let mut revoke = primary.begin().await?;
    sqlx::query("SET LOCAL synchronous_commit = 'local'")
        .execute(&mut *revoke)
        .await?;
    sqlx::query("REVOKE SELECT ON platform_durability_epoch FROM durability_observer")
        .execute(&mut *revoke)
        .await?;
    revoke.commit().await?;
    let pending = match resume_audited_row_observation(
        &observer_primary,
        &observer_standby,
        &topology,
        pending,
        Duration::from_secs(1),
    )
    .await
    {
        Err(AuditCustodyError::ObservationFailed { pending, source }) => {
            match *source {
                AuditCustodyError::Confirmation(ConfirmationError::Database(
                    sqlx::Error::Database(db),
                )) => assert_eq!(db.code().as_deref(), Some("42501")),
                other => panic!("wrong post-bound failure cause: {other:?}"),
            }
            pending
        }
        other => panic!("post-bound SQL failure lost its pending token: {other:?}"),
    };
    assert_eq!(pending.upper_bound_lsn(), fixed_bound);
    let mut restore = primary.begin().await?;
    sqlx::query("SET LOCAL synchronous_commit = 'local'")
        .execute(&mut *restore)
        .await?;
    sqlx::query("GRANT SELECT ON platform_durability_epoch TO durability_observer")
        .execute(&mut *restore)
        .await?;
    restore.commit().await?;
    // Advance unrelated WAL while replay stays paused. A continuation must
    // retain the same pending observation instead of taking a fresh bound.
    let mut later = primary.begin().await?;
    sqlx::query("SET LOCAL synchronous_commit = 'local'")
        .execute(&mut *later)
        .await?;
    sqlx::query("INSERT INTO durability_physical_probe VALUES (4)")
        .execute(&mut *later)
        .await?;
    later.commit().await?;
    let pending = match resume_audited_row_observation(
        &observer_primary,
        &observer_standby,
        &topology,
        pending,
        Duration::from_millis(250),
    )
    .await
    {
        Err(AuditCustodyError::Pending(pending)) => pending,
        other => panic!("paused retry must retain pending custody: {other:?}"),
    };
    assert_eq!(pending.upper_bound_lsn(), fixed_bound);
    assert!(matches!(
        confirm_post_commit(
            &primary,
            &standby,
            &topology,
            &writer,
            Duration::from_millis(250)
        )
        .await,
        Err(ConfirmationError::Deadline(_))
    ));
    sqlx::query("SELECT pg_wal_replay_resume()")
        .execute(&standby)
        .await?;
    let custody = resume_audited_row_observation(
        &observer_primary,
        &observer_standby,
        &topology,
        pending,
        Duration::from_secs(3),
    )
    .await?;
    assert_eq!(custody.upper_bound_lsn(), fixed_bound);
    assert_eq!(custody.audit_id(), audit.id);
    assert_eq!(custody.org_id(), company_a);
    confirm_post_commit(
        &primary,
        &standby,
        &topology,
        &writer,
        Duration::from_secs(3),
    )
    .await?;
    let persisted: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM durability_physical_probe WHERE id = 3)")
            .fetch_one(&standby)
            .await?;
    assert!(
        persisted,
        "canceled synchronous wait did not roll back local COMMIT"
    );

    let foreign = AuditRowRef {
        id: uuid::Uuid::new_v4(),
        org_id: company_b,
        action: "durability.write".to_owned(),
        target_type: "durability_probe".to_owned(),
        target_id: "foreign".to_owned(),
    };
    sqlx::query(
        "INSERT INTO audit_events (id, org_id, action, target_type, target_id)
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(foreign.id)
    .bind(foreign.org_id)
    .bind(&foreign.action)
    .bind(&foreign.target_type)
    .bind(&foreign.target_id)
    .execute(&primary)
    .await?;
    for observer in [&observer_primary, &observer_standby] {
        let unscoped: bool =
            sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM audit_events WHERE id = $1)")
                .bind(audit.id)
                .fetch_one(observer)
                .await?;
        assert!(
            !unscoped,
            "observer without Company context must see no row"
        );
        let mut scoped = observer.begin().await?;
        sqlx::query("SELECT set_config('app.current_org', $1, true)")
            .bind(company_b.to_string())
            .execute(&mut *scoped)
            .await?;
        let visible: (bool, bool) = sqlx::query_as(
            "SELECT EXISTS (SELECT 1 FROM audit_events WHERE id = $1),
                    EXISTS (SELECT 1 FROM audit_events WHERE id = $2)",
        )
        .bind(foreign.id)
        .bind(audit.id)
        .fetch_one(&mut *scoped)
        .await?;
        assert_eq!(
            visible,
            (true, false),
            "Company B must not read A's audit row"
        );
        scoped.rollback().await?;
    }
    for wrong in [
        AuditRowRef {
            org_id: company_b,
            ..audit.clone()
        },
        AuditRowRef {
            id: uuid::Uuid::new_v4(),
            ..audit.clone()
        },
        AuditRowRef {
            action: "durability.other".to_owned(),
            ..audit.clone()
        },
        AuditRowRef {
            target_type: "other_probe".to_owned(),
            ..audit.clone()
        },
        AuditRowRef {
            target_id: "other".to_owned(),
            ..audit.clone()
        },
    ] {
        assert!(matches!(
            observe_audited_row_post_commit(
                &observer_primary,
                &observer_standby,
                &topology,
                &writer,
                &wrong,
                Duration::from_secs(1),
            )
            .await,
            Err(AuditCustodyError::RowMismatch)
        ));
    }

    // A row that changes after the first bound must not be certified merely
    // because both sites eventually replay the old WAL prefix.
    let changed = AuditRowRef {
        id: uuid::Uuid::new_v4(),
        target_id: "changed".to_owned(),
        ..audit.clone()
    };
    sqlx::query(
        "INSERT INTO audit_events (id, org_id, action, target_type, target_id)
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(changed.id)
    .bind(changed.org_id)
    .bind(&changed.action)
    .bind(&changed.target_type)
    .bind(&changed.target_id)
    .execute(&primary)
    .await?;
    sqlx::query("SELECT pg_wal_replay_pause()")
        .execute(&standby)
        .await?;
    wait_until_paused(&standby).await?;
    let changed_pending = match observe_audited_row_post_commit(
        &observer_primary,
        &observer_standby,
        &topology,
        &writer,
        &changed,
        Duration::from_millis(250),
    )
    .await
    {
        Err(AuditCustodyError::Pending(pending)) => pending,
        other => panic!("paused replay must leave the original row pending: {other:?}"),
    };
    let mut changed_tx = primary.begin().await?;
    sqlx::query("SET LOCAL synchronous_commit = 'local'")
        .execute(&mut *changed_tx)
        .await?;
    sqlx::query("UPDATE audit_events SET action = 'durability.changed' WHERE id = $1")
        .bind(changed.id)
        .execute(&mut *changed_tx)
        .await?;
    changed_tx.commit().await?;
    sqlx::query("SELECT pg_wal_replay_resume()")
        .execute(&standby)
        .await?;
    confirm_post_commit(
        &primary,
        &standby,
        &topology,
        &writer,
        Duration::from_secs(3),
    )
    .await?;
    assert!(matches!(
        resume_audited_row_observation(
            &observer_primary,
            &observer_standby,
            &topology,
            changed_pending,
            Duration::from_secs(3),
        )
        .await,
        Err(AuditCustodyError::RowMismatch)
    ));

    // A primary row check blocked before INSERT-LSN capture is UNKNOWN, but
    // cannot produce a resumable post-bound token.
    let mut blocked = primary.begin().await?;
    sqlx::query("LOCK TABLE audit_events IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *blocked)
        .await?;
    assert!(matches!(
        observe_audited_row_post_commit(
            &observer_primary,
            &observer_standby,
            &topology,
            &writer,
            &audit,
            Duration::from_millis(150),
        )
        .await,
        Err(AuditCustodyError::DeadlineBeforeBound)
    ));
    blocked.rollback().await?;

    // The observer's first primary read after CHECKPOINT emits hint-bit WAL
    // for this exact table. No test write follows the checkpoint. Custody
    // must make progress on the INSERT bound captured after that read.
    let hints_enabled: bool =
        sqlx::query_scalar("SELECT current_setting('wal_log_hints')::boolean")
            .fetch_one(&primary)
            .await?;
    assert!(hints_enabled);
    let quiet = AuditRowRef {
        id: uuid::Uuid::new_v4(),
        org_id: company_a,
        action: "durability.quiet".to_owned(),
        target_type: "durability_probe".to_owned(),
        target_id: "quiet".to_owned(),
    };
    sqlx::query(
        "INSERT INTO audit_events (id, org_id, action, target_type, target_id)
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(quiet.id)
    .bind(quiet.org_id)
    .bind(&quiet.action)
    .bind(&quiet.target_type)
    .bind(&quiet.target_id)
    .execute(&primary)
    .await?;
    sqlx::query("CHECKPOINT").execute(&primary).await?;
    let before_hint: String = sqlx::query_scalar("SELECT pg_current_wal_insert_lsn()::text")
        .fetch_one(&primary)
        .await?;
    let relation_path: String =
        sqlx::query_scalar("SELECT pg_relation_filepath('audit_events'::regclass)")
            .fetch_one(&primary)
            .await?;
    let quiet_custody = observe_audited_row_post_commit(
        &observer_primary,
        &observer_standby,
        &topology,
        &writer,
        &quiet,
        Duration::from_secs(3),
    )
    .await?;
    assert!(
        lsn_greater(&primary, quiet_custody.upper_bound_lsn(), &before_hint).await?,
        "custody must preserve the post-read primary INSERT bound"
    );
    assert_hint_wal_for_relation(
        &relation_path,
        &before_hint,
        quiet_custody.upper_bound_lsn(),
    )?;
    let after_flush: String = sqlx::query_scalar("SELECT pg_current_wal_insert_lsn()::text")
        .fetch_one(&primary)
        .await?;
    let message_lsn = forced_flush_message_lsn(&before_hint, &after_flush)?;
    assert!(
        lsn_at_least(&primary, &message_lsn, quiet_custody.upper_bound_lsn()).await?
            && lsn_greater(&primary, &after_flush, quiet_custody.upper_bound_lsn()).await?,
        "flush message must start at/after and extend beyond the custody INSERT bound"
    );
    Ok(())
}

async fn lsn_at_least(pool: &PgPool, actual: &str, expected: &str) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar("SELECT $1::pg_lsn >= $2::pg_lsn")
        .bind(actual)
        .bind(expected)
        .fetch_one(pool)
        .await
}

async fn lsn_greater(pool: &PgPool, actual: &str, previous: &str) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar("SELECT $1::pg_lsn > $2::pg_lsn")
        .bind(actual)
        .bind(previous)
        .fetch_one(pool)
        .await
}

fn assert_hint_wal_for_relation(
    relation_path: &str,
    before: &str,
    bound: &str,
) -> Result<(), Box<dyn Error>> {
    let rel = relation_path
        .strip_prefix("base/")
        .ok_or("disposable audit table is not in the default tablespace")?;
    let pg_bin = std::env::var("PG18_BIN")?;
    let data_dir = std::env::var("V1_PRIMARY_DATA_DIR")?;
    let output = Command::new(format!("{pg_bin}/pg_waldump"))
        .args([
            "-p",
            &format!("{data_dir}/pg_wal"),
            "-R",
            &format!("1663/{rel}"),
            "-s",
            before,
            "-e",
            bound,
        ])
        .output()?;
    let records = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success() && records.contains("FPI_FOR_HINT"),
        "observer read emitted no relation-specific hint WAL: status={} stdout={} stderr={}",
        output.status,
        records,
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(())
}

fn forced_flush_message_lsn(before: &str, after: &str) -> Result<String, Box<dyn Error>> {
    let pg_bin = std::env::var("PG18_BIN")?;
    let data_dir = std::env::var("V1_PRIMARY_DATA_DIR")?;
    let output = Command::new(format!("{pg_bin}/pg_waldump"))
        .args([
            "-p",
            &format!("{data_dir}/pg_wal"),
            "-s",
            before,
            "-e",
            after,
        ])
        .output()?;
    let records = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "cannot inspect observer flush WAL: status={} stderr={}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    let record = records
        .lines()
        .find(|line| line.contains("LogicalMessage") && line.contains("console.audit-custody"))
        .ok_or_else(|| format!("observer never exercised its quiet-WAL flush path: {records}"))?;
    record
        .split("lsn:")
        .nth(1)
        .and_then(|part| part.split_whitespace().next())
        .map(|lsn| lsn.trim_end_matches(',').to_owned())
        .ok_or_else(|| format!("flush record has no LSN: {record}").into())
}

async fn wait_until_paused(standby: &PgPool) -> Result<(), Box<dyn Error>> {
    for _ in 0..100 {
        let state: String = sqlx::query_scalar("SELECT pg_get_wal_replay_pause_state()")
            .fetch_one(standby)
            .await?;
        if state == "paused" {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    Err("physical standby did not pause replay".into())
}
