#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use console_compliance_adapter_postgres::PgComplianceStore;
use console_compliance_application::{
    AcceptEvidenceBindingCommand, ArrivalEventQuery, ConsentTransitionCommand,
    ConsentTransitionKind, CreateEvidenceBindingCommand,
};
use console_compliance_domain::{
    EvidenceConfidence, EvidenceTargetType, LocationPing, PingVolumeBound,
};
use console_kernel_core::{BranchId, BranchScope, LocationPingId, OrgId, TraceContext, UserId};
use sqlx::{PgPool, Row};
use std::collections::BTreeSet;
use std::sync::Arc;
use time::{Duration, OffsetDateTime, macros::datetime};
use tokio::sync::Barrier;

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn withdrawal_destroys_pings_and_logs_while_auditing_only_consent(pool: PgPool) {
    console_platform_request_context::scope_org(console_kernel_core::OrgId::knl(), async move {
        let (user_id, branch_id) = seed_user_and_branch(&pool, "Consent User").await;
        let store = PgComplianceStore::new(pool.clone());

        store
            .transition_consent(command(
                ConsentTransitionKind::Grant,
                user_id,
                branch_id,
                datetime!(2026-06-12 08:00:00 UTC),
            ))
            .await
            .unwrap();

        for (lat, lon, recorded_at) in [
            (37.5665, 126.9780, datetime!(2026-06-12 09:00:00 UTC)),
            (37.5670, 126.9790, datetime!(2026-06-12 09:00:30 UTC)),
        ] {
            let ping = LocationPing::new(
                LocationPingId::new(),
                user_id,
                branch_id,
                lat,
                lon,
                Some(7.0),
                recorded_at,
                true,
            )
            .unwrap();
            store.record_location_ping(ping).await.unwrap();
        }

        assert_eq!(count_location_pings(&pool, user_id).await, 2);
        assert_eq!(count_collection_logs(&pool, user_id).await, 2);

        store
            .transition_consent(command(
                ConsentTransitionKind::Withdraw,
                user_id,
                branch_id,
                datetime!(2026-06-12 10:00:00 UTC),
            ))
            .await
            .unwrap();

        assert_eq!(count_location_pings(&pool, user_id).await, 0);
        assert_eq!(count_collection_logs(&pool, user_id).await, 0);

        let status: String =
            sqlx::query_scalar("SELECT status FROM location_consents WHERE user_id = $1")
                .bind(*user_id.as_uuid())
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(status, "WITHDRAWN");

        let withdraw_audits: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM audit_events WHERE target_type = 'location_consent' AND action = 'consent.withdraw'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(withdraw_audits, 1);

        let coordinate_audits: i64 = sqlx::query_scalar(
            r#"
            SELECT COUNT(*)
            FROM audit_events
            WHERE COALESCE(before_snap::text, '') || COALESCE(after_snap::text, '')
                  ~ '(latitude|longitude|coordinates|37\.5665|126\.9780|37\.5670|126\.9790)'
            "#,
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(coordinate_audits, 0);
    })
    .await;
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn retention_purge_drops_expired_day_partitions(pool: PgPool) {
    console_platform_request_context::scope_org(console_kernel_core::OrgId::knl(), async move {
        let (user_id, branch_id) = seed_user_and_branch(&pool, "Retention User").await;
        let store = PgComplianceStore::new(pool.clone());

        store
            .transition_consent(command(
                ConsentTransitionKind::Grant,
                user_id,
                branch_id,
                datetime!(2026-06-09 08:00:00 UTC),
            ))
            .await
            .unwrap();

        for recorded_at in [
            datetime!(2026-06-10 09:00:00 UTC),
            datetime!(2026-06-12 09:00:00 UTC),
        ] {
            let ping = LocationPing::new(
                LocationPingId::new(),
                user_id,
                branch_id,
                37.5665,
                126.9780,
                None,
                recorded_at,
                true,
            )
            .unwrap();
            store.record_location_ping(ping).await.unwrap();
        }

        assert!(partition_exists(&pool, "location_pings_20260610").await);
        assert!(partition_exists(&pool, "location_pings_20260612").await);

        let purge = store
            .purge_expired_location_data(datetime!(2026-06-11 00:00:00 UTC))
            .await
            .unwrap();

        assert_eq!(purge.dropped_ping_partitions, 1);
        assert_eq!(purge.deleted_collection_logs, 1);
        assert!(!partition_exists(&pool, "location_pings_20260610").await);
        assert!(partition_exists(&pool, "location_pings_20260612").await);
        assert_eq!(count_location_pings(&pool, user_id).await, 1);
        assert_eq!(count_collection_logs(&pool, user_id).await, 1);
    })
    .await;
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn ping_volume_stays_within_on_duty_window_rate_bound(pool: PgPool) {
    console_platform_request_context::scope_org(console_kernel_core::OrgId::knl(), async move {
        let (first_user, branch_id) = seed_user_and_branch(&pool, "Volume User 1").await;
        let (second_user, _) = seed_user_in_branch(&pool, branch_id, "Volume User 2").await;
        let store = PgComplianceStore::new(pool.clone());

        for user_id in [first_user, second_user] {
            store
                .transition_consent(command(
                    ConsentTransitionKind::Grant,
                    user_id,
                    branch_id,
                    datetime!(2026-06-12 08:00:00 UTC),
                ))
                .await
                .unwrap();
        }

        for (user_id, second_offset) in [(first_user, 0), (first_user, 1), (second_user, 0)] {
            let ping = LocationPing::new(
                LocationPingId::new(),
                user_id,
                branch_id,
                37.5665,
                126.9780,
                None,
                datetime!(2026-06-12 09:00:00 UTC) + Duration::seconds(second_offset),
                true,
            )
            .unwrap();
            store.record_location_ping(ping).await.unwrap();
        }

        let observed: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM location_pings")
            .fetch_one(&pool)
            .await
            .unwrap();
        let bound = PingVolumeBound::new(2, Duration::hours(1), Duration::minutes(30)).unwrap();
        assert!(bound.allows(u64::try_from(observed).unwrap()));
    })
    .await;
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn concurrent_first_pings_for_same_day_share_partition_creation(pool: PgPool) {
    console_platform_request_context::scope_org(console_kernel_core::OrgId::knl(), async move {
        let (user_id, branch_id) = seed_user_and_branch(&pool, "Concurrent User").await;
        let store = Arc::new(PgComplianceStore::new(pool.clone()));

        store
            .transition_consent(command(
                ConsentTransitionKind::Grant,
                user_id,
                branch_id,
                datetime!(2026-06-14 08:00:00 UTC),
            ))
            .await
            .unwrap();

        let mut handles = Vec::new();
        for offset in 0..32 {
            let store = Arc::clone(&store);
            handles.push(tokio::spawn(async move {
                console_platform_request_context::scope_org(
                    console_kernel_core::OrgId::knl(),
                    async move {
                        let ping = LocationPing::new(
                            LocationPingId::new(),
                            user_id,
                            branch_id,
                            37.5665,
                            126.9780,
                            None,
                            datetime!(2026-06-14 09:00:00 UTC) + Duration::seconds(offset),
                            true,
                        )
                        .unwrap();
                        store.record_location_ping(ping).await
                    },
                )
                .await
            }));
        }

        for handle in handles {
            handle.await.unwrap().unwrap();
        }

        assert!(partition_exists(&pool, "location_pings_20260614").await);
        assert_eq!(count_location_pings(&pool, user_id).await, 32);
        assert_eq!(count_collection_logs(&pool, user_id).await, 32);
    })
    .await;
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn consented_user_can_ping_a_different_branch(pool: PgPool) {
    console_platform_request_context::scope_org(console_kernel_core::OrgId::knl(), async move {
        // Consent is per-user (location_consents UNIQUE (user_id)). A multi-branch
        // mechanic who granted consent in branch A must still be able to ping while
        // on duty in branch B without a spurious 403.
        let (user_id, consent_branch) = seed_user_and_branch(&pool, "Multi Branch").await;
        let other_branch = seed_second_branch(&pool, user_id, "Multi Branch Other").await;
        let store = PgComplianceStore::new(pool.clone());

        store
            .transition_consent(command(
                ConsentTransitionKind::Grant,
                user_id,
                consent_branch,
                datetime!(2026-06-12 08:00:00 UTC),
            ))
            .await
            .unwrap();

        // current_consent for a different branch must succeed (no branch-mismatch 403).
        store.current_consent(user_id, other_branch).await.unwrap();

        let ping = LocationPing::new(
            LocationPingId::new(),
            user_id,
            other_branch,
            37.5665,
            126.9780,
            Some(5.0),
            datetime!(2026-06-12 09:00:00 UTC),
            true,
        )
        .unwrap();
        store.record_location_ping(ping).await.unwrap();

        assert_eq!(count_location_pings(&pool, user_id).await, 1);
        let stored_branch: uuid::Uuid =
            sqlx::query_scalar("SELECT branch_id FROM location_pings WHERE user_id = $1")
                .bind(*user_id.as_uuid())
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(stored_branch, *other_branch.as_uuid());
    })
    .await;
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn accepting_proposed_evidence_is_tenant_scoped_audited_and_not_replayable(pool: PgPool) {
    console_platform_request_context::scope_org(OrgId::knl(), async move {
        let (actor, _branch) = seed_user_and_branch(&pool, "Evidence Approver").await;
        let control_id = seed_compliance_control(&pool, actor).await;
        let store = PgComplianceStore::new(pool.clone());
        let created = store
            .create_evidence_binding(CreateEvidenceBindingCommand {
                actor,
                control_id,
                obligation_id: None,
                evidence_target_type: EvidenceTargetType::ExternalDocument,
                evidence_target_id: "K-ISMS-2026-001".to_owned(),
                source_audit_event_id: None,
                confidence: EvidenceConfidence::High,
                collected_at: None,
                collected_by: None,
                valid_from: None,
                valid_to: None,
                hash_sha256: None,
                metadata: serde_json::json!({"source": "audited-test"}),
                trace: TraceContext::generate(),
                occurred_at: datetime!(2026-07-24 09:00:00 UTC),
            })
            .await
            .unwrap();

        let accepted = store
            .accept_evidence_binding(AcceptEvidenceBindingCommand {
                actor,
                id: created.id,
                trace: TraceContext::generate(),
                occurred_at: datetime!(2026-07-24 10:00:00 UTC),
            })
            .await
            .unwrap();
        assert_eq!(accepted.status.as_db_str(), "ACCEPTED");
        assert_eq!(accepted.evidence_target_id, "K-ISMS-2026-001");
        assert_eq!(accepted.metadata, serde_json::json!({"source": "audited-test"}));

        let audit: (Option<serde_json::Value>, Option<serde_json::Value>) = sqlx::query_as(
            "SELECT before_snap, after_snap FROM audit_events WHERE action = 'compliance.evidence_binding.accept' AND target_id = $1",
        )
        .bind(created.id.to_string())
        .fetch_one(&pool)
        .await
        .unwrap();
        let before = audit.0.expect("accept audit records the prior binding snapshot");
        let after = audit.1.expect("accept audit records the accepted binding snapshot");
        assert_eq!(before["status"], "PROPOSED");
        assert_eq!(after["status"], "ACCEPTED");
        assert_eq!(after["evidence_target_id"], "K-ISMS-2026-001");

        let replay = store
            .accept_evidence_binding(AcceptEvidenceBindingCommand {
                actor,
                id: created.id,
                trace: TraceContext::generate(),
                occurred_at: datetime!(2026-07-24 10:01:00 UTC),
            })
            .await
            .unwrap_err();
        assert_eq!(replay.kind(), console_kernel_core::ErrorKind::Conflict);
        let accept_audits: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM audit_events WHERE action = 'compliance.evidence_binding.accept' AND target_id = $1",
        )
        .bind(created.id.to_string())
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(accept_audits, 1);
    })
    .await;
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn concurrent_evidence_acceptors_produce_one_accept_one_conflict_and_one_audit(pool: PgPool) {
    console_platform_request_context::scope_org(OrgId::knl(), async move {
        let (actor, _branch) = seed_user_and_branch(&pool, "Concurrent Evidence Approver").await;
        let control_id = seed_compliance_control(&pool, actor).await;
        let store = PgComplianceStore::new(pool.clone());
        let binding = store
            .create_evidence_binding(CreateEvidenceBindingCommand {
                actor,
                control_id,
                obligation_id: None,
                evidence_target_type: EvidenceTargetType::ExternalDocument,
                evidence_target_id: "K-ISMS-2026-RACE".to_owned(),
                source_audit_event_id: None,
                confidence: EvidenceConfidence::High,
                collected_at: None,
                collected_by: None,
                valid_from: None,
                valid_to: None,
                hash_sha256: None,
                metadata: serde_json::json!({"source": "race-test"}),
                trace: TraceContext::generate(),
                occurred_at: datetime!(2026-07-24 11:00:00 UTC),
            })
            .await
            .unwrap();

        let barrier = Arc::new(Barrier::new(2));
        let acceptor = |occurred_at| {
            let barrier = Arc::clone(&barrier);
            let store = store.clone();
            tokio::spawn(async move {
                barrier.wait().await;
                console_platform_request_context::scope_org(OrgId::knl(), async move {
                    store
                        .accept_evidence_binding(AcceptEvidenceBindingCommand {
                            actor,
                            id: binding.id,
                            trace: TraceContext::generate(),
                            occurred_at,
                        })
                        .await
                })
                .await
            })
        };
        let first = acceptor(datetime!(2026-07-24 11:01:00 UTC));
        let second = acceptor(datetime!(2026-07-24 11:02:00 UTC));
        let results = [first.await.unwrap(), second.await.unwrap()];
        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        assert_eq!(
            results
                .iter()
                .filter(|result| result.as_ref().is_err_and(|error| error.kind() == console_kernel_core::ErrorKind::Conflict))
                .count(),
            1,
            "the loser must observe the committed state transition as a conflict"
        );

        let accept_audits: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM audit_events WHERE action = 'compliance.evidence_binding.accept' AND target_id = $1",
        )
        .bind(binding.id.to_string())
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(accept_audits, 1, "only the winning transition is audited");
    })
    .await;
}

fn command(
    kind: ConsentTransitionKind,
    user_id: UserId,
    branch_id: BranchId,
    occurred_at: OffsetDateTime,
) -> ConsentTransitionCommand {
    ConsentTransitionCommand {
        kind,
        actor: Some(user_id),
        user_id,
        branch_id,
        trace: TraceContext::generate(),
        occurred_at,
    }
}

async fn seed_compliance_control(pool: &PgPool, actor: UserId) -> uuid::Uuid {
    let framework_id: uuid::Uuid = sqlx::query_scalar(
        "INSERT INTO compliance_frameworks (org_id, code, name, version_label, framework_kind, created_by, updated_by) VALUES ($1, $2, $3, $4, $5, $6, $6) RETURNING id",
    )
    .bind(*OrgId::knl().as_uuid())
    .bind("FW-9001")
    .bind("Evidence acceptance test framework")
    .bind("2026.07")
    .bind("INTERNAL_CONTROL")
    .bind(*actor.as_uuid())
    .fetch_one(pool)
    .await
    .unwrap();
    sqlx::query_scalar(
        "INSERT INTO compliance_controls (org_id, framework_id, control_key, title, objective, control_type, created_by, updated_by) VALUES ($1, $2, $3, $4, $5, $6, $7, $7) RETURNING id",
    )
    .bind(*OrgId::knl().as_uuid())
    .bind(framework_id)
    .bind("EVIDENCE.ACCEPT")
    .bind("Accept immutable evidence")
    .bind("Accept an evidence binding through the audited lifecycle action.")
    .bind("DETECTIVE")
    .bind(*actor.as_uuid())
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn seed_user_and_branch(pool: &PgPool, display_name: &str) -> (UserId, BranchId) {
    let region_id: uuid::Uuid =
        sqlx::query_scalar("INSERT INTO regions (name, org_id) VALUES ($1, $2) RETURNING id")
            .bind(format!("{display_name} Region"))
            .bind(*OrgId::knl().as_uuid())
            .fetch_one(pool)
            .await
            .unwrap();

    let branch_id: uuid::Uuid = sqlx::query_scalar(
        "INSERT INTO branches (region_id, name, org_id) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(region_id)
    .bind(format!("{display_name} Branch"))
    .bind(*OrgId::knl().as_uuid())
    .fetch_one(pool)
    .await
    .unwrap();

    seed_user_in_branch(pool, BranchId::from_uuid(branch_id), display_name).await
}

async fn seed_user_in_branch(
    pool: &PgPool,
    branch_id: BranchId,
    display_name: &str,
) -> (UserId, BranchId) {
    let user_id: uuid::Uuid = sqlx::query_scalar(
        "INSERT INTO users (display_name, roles, org_id) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(display_name)
    .bind(Vec::<String>::from(["MECHANIC".to_string()]))
    .bind(*OrgId::knl().as_uuid())
    .fetch_one(pool)
    .await
    .unwrap();

    sqlx::query("INSERT INTO user_branches (user_id, branch_id, org_id) VALUES ($1, $2, $3)")
        .bind(user_id)
        .bind(*branch_id.as_uuid())
        .bind(*OrgId::knl().as_uuid())
        .execute(pool)
        .await
        .unwrap();

    (UserId::from_uuid(user_id), branch_id)
}

async fn seed_second_branch(pool: &PgPool, user_id: UserId, display_name: &str) -> BranchId {
    let region_id: uuid::Uuid =
        sqlx::query_scalar("INSERT INTO regions (name, org_id) VALUES ($1, $2) RETURNING id")
            .bind(format!("{display_name} Region"))
            .bind(*OrgId::knl().as_uuid())
            .fetch_one(pool)
            .await
            .unwrap();

    let branch_id: uuid::Uuid = sqlx::query_scalar(
        "INSERT INTO branches (region_id, name, org_id) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(region_id)
    .bind(format!("{display_name} Branch"))
    .bind(*OrgId::knl().as_uuid())
    .fetch_one(pool)
    .await
    .unwrap();

    sqlx::query("INSERT INTO user_branches (user_id, branch_id, org_id) VALUES ($1, $2, $3)")
        .bind(*user_id.as_uuid())
        .bind(branch_id)
        .bind(*OrgId::knl().as_uuid())
        .execute(pool)
        .await
        .unwrap();

    BranchId::from_uuid(branch_id)
}

async fn count_location_pings(pool: &PgPool, user_id: UserId) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM location_pings WHERE user_id = $1")
        .bind(*user_id.as_uuid())
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn count_collection_logs(pool: &PgPool, user_id: UserId) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM location_collection_logs WHERE user_id = $1")
        .bind(*user_id.as_uuid())
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn partition_exists(pool: &PgPool, partition: &str) -> bool {
    let row = sqlx::query("SELECT to_regclass($1) IS NOT NULL AS exists")
        .bind(format!("public.{partition}"))
        .fetch_one(pool)
        .await
        .unwrap();
    row.get("exists")
}

// ── Geofence arrival/departure (issue #13) ──────────────────────────────────

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn geofence_arrival_departure_is_audited_and_survives_withdrawal(pool: PgPool) {
    console_platform_request_context::scope_org(OrgId::knl(), async move {
        let (user_id, branch_id) = seed_user_and_branch(&pool, "Geofence Mechanic").await;
        let customer_id = seed_customer(&pool, branch_id).await;
        // Site at Seoul City Hall; default 300 m geofence (no per-site override).
        let site_id = seed_site(&pool, branch_id, customer_id, 37.5665, 126.9780).await;
        let equipment_id = seed_equipment(&pool, branch_id, customer_id, site_id).await;
        let work_order_id = seed_assigned_work_order(
            &pool,
            branch_id,
            equipment_id,
            customer_id,
            site_id,
            user_id,
        )
        .await;

        let store = PgComplianceStore::new(pool.clone());
        store
            .transition_consent(command(
                ConsentTransitionKind::Grant,
                user_id,
                branch_id,
                datetime!(2026-06-12 08:00:00 UTC),
            ))
            .await
            .unwrap();

        // Ping INSIDE the geofence (the site coordinate) → exactly one ARRIVAL.
        record_on_duty_ping(
            &store,
            user_id,
            branch_id,
            37.5665,
            126.9780,
            datetime!(2026-06-12 09:00:00 UTC),
        )
        .await;
        assert_eq!(count_attendance(&pool, "ARRIVAL").await, 1);
        assert_eq!(count_attendance(&pool, "DEPARTURE").await, 0);

        // Re-ping a few metres away, still inside → no new event (edge-triggered).
        record_on_duty_ping(
            &store,
            user_id,
            branch_id,
            37.56655,
            126.97805,
            datetime!(2026-06-12 09:01:00 UTC),
        )
        .await;
        assert_eq!(count_attendance(&pool, "ARRIVAL").await, 1);

        // Ping far outside the geofence → one DEPARTURE.
        record_on_duty_ping(
            &store,
            user_id,
            branch_id,
            37.6500,
            127.1000,
            datetime!(2026-06-12 09:30:00 UTC),
        )
        .await;
        assert_eq!(count_attendance(&pool, "DEPARTURE").await, 1);

        // Both crossings are audited; one presence row tracks current state.
        assert_eq!(count_audit_action(&pool, "site.arrival").await, 1);
        assert_eq!(count_audit_action(&pool, "site.departure").await, 1);
        assert_eq!(count_presence(&pool, user_id).await, 1);

        let feed = store
            .list_arrival_events(
                &BranchScope::Branches(BTreeSet::from([branch_id])),
                ArrivalEventQuery {
                    user_id: None,
                    branch_id: None,
                    limit: 10,
                    offset: 0,
                },
            )
            .await
            .unwrap();
        assert_eq!(feed.total, 2);
        assert_eq!(feed.items[0].kind, "DEPARTURE");
        assert_eq!(feed.items[0].work_order_id, work_order_id.to_string());
        assert_eq!(feed.items[0].site_id, site_id.to_string());
        assert_eq!(feed.items[0].site_name, "Geofence Site");
        assert_eq!(feed.items[0].customer_name, "Geofence Customer");
        assert_eq!(feed.items[0].mechanic_name, "Geofence Mechanic");
        assert_eq!(feed.items[0].latitude, Some(37.5665));
        assert_eq!(feed.items[0].longitude, Some(126.9780));

        // Consent withdrawal erases the raw pings AND the transient geofence
        // presence state, but the durable coordinate-free attendance events
        // survive — the #13 carve-out (work fact, not location data).
        store
            .transition_consent(command(
                ConsentTransitionKind::Withdraw,
                user_id,
                branch_id,
                datetime!(2026-06-12 10:00:00 UTC),
            ))
            .await
            .unwrap();
        assert_eq!(count_location_pings(&pool, user_id).await, 0);
        assert_eq!(count_presence(&pool, user_id).await, 0);
        assert_eq!(count_attendance(&pool, "ARRIVAL").await, 1);
        assert_eq!(count_attendance(&pool, "DEPARTURE").await, 1);
    })
    .await;
}

async fn seed_customer(pool: &PgPool, branch_id: BranchId) -> uuid::Uuid {
    sqlx::query_scalar(
        "INSERT INTO registry_customers (branch_id, name, org_id) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(*branch_id.as_uuid())
    .bind("Geofence Customer")
    .bind(*OrgId::knl().as_uuid())
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn seed_site(
    pool: &PgPool,
    branch_id: BranchId,
    customer_id: uuid::Uuid,
    latitude: f64,
    longitude: f64,
) -> uuid::Uuid {
    sqlx::query_scalar(
        "INSERT INTO registry_sites (branch_id, customer_id, name, latitude, longitude, org_id) \
         VALUES ($1, $2, $3, $4, $5, $6) RETURNING id",
    )
    .bind(*branch_id.as_uuid())
    .bind(customer_id)
    .bind("Geofence Site")
    .bind(latitude)
    .bind(longitude)
    .bind(*OrgId::knl().as_uuid())
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn seed_equipment(
    pool: &PgPool,
    branch_id: BranchId,
    customer_id: uuid::Uuid,
    site_id: uuid::Uuid,
) -> uuid::Uuid {
    sqlx::query_scalar(
        "INSERT INTO registry_equipment (branch_id, customer_id, site_id, equipment_no, \
         management_no, model, manufacturer_code, kind_code, power_code, status, specification, \
         ton_text, ton_milli, source_sheet, source_row, org_id) \
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16) RETURNING id",
    )
    .bind(*branch_id.as_uuid())
    .bind(customer_id)
    .bind(site_id)
    .bind("GEOFN-0001")
    .bind("GEOFN-001")
    .bind("Geofence Model")
    .bind("GEO-MAKER")
    .bind("FORK")
    .bind("ELEC")
    .bind("임대")
    .bind("15t/6m")
    .bind("15t")
    .bind(15000_i32)
    .bind("geofence-seed")
    .bind(1_i32)
    .bind(*OrgId::knl().as_uuid())
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn seed_assigned_work_order(
    pool: &PgPool,
    branch_id: BranchId,
    equipment_id: uuid::Uuid,
    customer_id: uuid::Uuid,
    site_id: uuid::Uuid,
    user_id: UserId,
) -> uuid::Uuid {
    let work_order_id: uuid::Uuid = sqlx::query_scalar(
        "INSERT INTO work_orders (request_no, branch_id, equipment_id, customer_id, site_id, \
         requested_by, status, priority, symptom, org_id) \
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10) RETURNING id",
    )
    .bind("20260612-900")
    .bind(*branch_id.as_uuid())
    .bind(equipment_id)
    .bind(customer_id)
    .bind(site_id)
    .bind(*user_id.as_uuid())
    .bind("IN_PROGRESS")
    .bind("P2")
    .bind("geofence work order")
    .bind(*OrgId::knl().as_uuid())
    .fetch_one(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO work_order_assignments (work_order_id, mechanic_id, role, assigned_at, org_id) \
         VALUES ($1, $2, $3, now(), $4)",
    )
    .bind(work_order_id)
    .bind(*user_id.as_uuid())
    .bind("PRIMARY")
    .bind(*OrgId::knl().as_uuid())
    .execute(pool)
    .await
    .unwrap();
    work_order_id
}

async fn record_on_duty_ping(
    store: &PgComplianceStore,
    user_id: UserId,
    branch_id: BranchId,
    latitude: f64,
    longitude: f64,
    recorded_at: OffsetDateTime,
) {
    let ping = LocationPing::new(
        LocationPingId::new(),
        user_id,
        branch_id,
        latitude,
        longitude,
        Some(7.0),
        recorded_at,
        true,
    )
    .unwrap();
    store.record_location_ping(ping).await.unwrap();
}

async fn count_attendance(pool: &PgPool, kind: &str) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM site_attendance_events WHERE kind = $1")
        .bind(kind)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn count_audit_action(pool: &PgPool, action: &str) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM audit_events WHERE action = $1")
        .bind(action)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn count_presence(pool: &PgPool, user_id: UserId) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM site_geofence_presence WHERE user_id = $1")
        .bind(*user_id.as_uuid())
        .fetch_one(pool)
        .await
        .unwrap()
}

/// Regression: a multi-branch mechanic pings tagged to branch B while assigned to
/// a work order whose site belongs to branch A. The durable attendance fact must
/// be filed under the WORK ORDER's branch (A), not the ping's branch (B), or a
/// branch-A supervisor's branch-scoped read would never see the event.
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn attendance_is_filed_under_the_work_order_branch_not_the_ping_branch(pool: PgPool) {
    console_platform_request_context::scope_org(OrgId::knl(), async move {
        let (user_id, branch_a) = seed_user_and_branch(&pool, "Multi-Branch Mechanic").await;
        let branch_b = seed_second_branch(&pool, user_id, "Mechanic Branch B").await;
        let customer_id = seed_customer(&pool, branch_a).await;
        let site_id = seed_site(&pool, branch_a, customer_id, 37.5665, 126.9780).await;
        let equipment_id = seed_equipment(&pool, branch_a, customer_id, site_id).await;
        seed_assigned_work_order(&pool, branch_a, equipment_id, customer_id, site_id, user_id)
            .await;

        let store = PgComplianceStore::new(pool.clone());
        store
            .transition_consent(command(
                ConsentTransitionKind::Grant,
                user_id,
                branch_b,
                datetime!(2026-06-12 08:00:00 UTC),
            ))
            .await
            .unwrap();
        // On duty in branch B, but physically at the branch-A site.
        record_on_duty_ping(
            &store,
            user_id,
            branch_b,
            37.5665,
            126.9780,
            datetime!(2026-06-12 09:00:00 UTC),
        )
        .await;

        let event_branch: uuid::Uuid = sqlx::query_scalar(
            "SELECT branch_id FROM site_attendance_events WHERE kind = 'ARRIVAL'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(
            event_branch,
            *branch_a.as_uuid(),
            "filed under work-order branch A"
        );
        assert_ne!(event_branch, *branch_b.as_uuid(), "not the ping's branch B");

        // The audit row carries the same (work-order) branch.
        let audit_branch: Option<uuid::Uuid> =
            sqlx::query_scalar("SELECT branch_id FROM audit_events WHERE action = 'site.arrival'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(audit_branch, Some(*branch_a.as_uuid()));
    })
    .await;
}

/// Regression: recorded_at is a client capture time and offline-queued pings flush
/// out of order. A stale ping older than the last recorded transition must be
/// dropped, never flip the geofence state or emit a phantom crossing.
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn out_of_order_stale_ping_does_not_emit_a_phantom_crossing(pool: PgPool) {
    console_platform_request_context::scope_org(OrgId::knl(), async move {
        let (user_id, branch_id) = seed_user_and_branch(&pool, "Out Of Order Mechanic").await;
        let customer_id = seed_customer(&pool, branch_id).await;
        let site_id = seed_site(&pool, branch_id, customer_id, 37.5665, 126.9780).await;
        let equipment_id = seed_equipment(&pool, branch_id, customer_id, site_id).await;
        seed_assigned_work_order(
            &pool,
            branch_id,
            equipment_id,
            customer_id,
            site_id,
            user_id,
        )
        .await;

        let store = PgComplianceStore::new(pool.clone());
        store
            .transition_consent(command(
                ConsentTransitionKind::Grant,
                user_id,
                branch_id,
                datetime!(2026-06-12 08:00:00 UTC),
            ))
            .await
            .unwrap();

        // Arrival at 09:00.
        record_on_duty_ping(
            &store,
            user_id,
            branch_id,
            37.5665,
            126.9780,
            datetime!(2026-06-12 09:00:00 UTC),
        )
        .await;
        assert_eq!(count_attendance(&pool, "ARRIVAL").await, 1);

        // A STALE ping captured at 08:30 (before the arrival) flushed late, far
        // outside the geofence — must be DROPPED, not emit a phantom DEPARTURE.
        record_on_duty_ping(
            &store,
            user_id,
            branch_id,
            37.6500,
            127.1000,
            datetime!(2026-06-12 08:30:00 UTC),
        )
        .await;
        assert_eq!(count_attendance(&pool, "DEPARTURE").await, 0);
        assert_eq!(count_attendance(&pool, "ARRIVAL").await, 1);

        // A FRESH ping at 09:30 far outside → the real DEPARTURE still lands.
        record_on_duty_ping(
            &store,
            user_id,
            branch_id,
            37.6500,
            127.1000,
            datetime!(2026-06-12 09:30:00 UTC),
        )
        .await;
        assert_eq!(count_attendance(&pool, "DEPARTURE").await, 1);
    })
    .await;
}
