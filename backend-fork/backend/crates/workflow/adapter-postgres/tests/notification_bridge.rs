#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Compensation-bridge gate: a `NOTIFICATION` outbox event (the approval-line
//! notify the post-finalization-rejection flow enqueues) is drained into real
//! notification-center rows, once per recipient, idempotently — proven as the
//! genuine non-owner `console_rt` role under FORCE RLS.

use console_kernel_core::OrgId;
use console_notifications_adapter_postgres::PgNotificationStore;
use console_platform_request_context::scope_org;
use console_workflow_runtime_adapter_postgres::PgWorkflowRuntimeStore;
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use uuid::Uuid;

async fn runtime_role_pool(owner_pool: &PgPool) -> PgPool {
    for grant in [
        "GRANT SELECT, INSERT, UPDATE ON notifications TO console_rt",
        "GRANT SELECT, UPDATE ON workflow_outbox_events TO console_rt",
        "GRANT SELECT, INSERT ON audit_events TO console_rt",
        "GRANT SELECT ON users TO console_rt",
        "GRANT SELECT ON organizations TO console_rt",
    ] {
        sqlx::query(grant).execute(owner_pool).await.unwrap();
    }
    let options = owner_pool.connect_options().as_ref().clone();
    PgPoolOptions::new()
        .max_connections(4)
        .after_connect(|conn, _meta| {
            Box::pin(async move {
                sqlx::query("SET ROLE console_rt").execute(conn).await?;
                Ok(())
            })
        })
        .connect_with(options)
        .await
        .unwrap()
}

/// Seed org + user + a minimal published definition + a RUNNING run + one PENDING
/// NOTIFICATION outbox event addressed to `recipient`. Seeded via the owner pool
/// (console_rt is SELECT-only on organizations); the drain under test runs as console_rt.
async fn seed(owner_pool: &PgPool, org: Uuid, recipient: Uuid) -> Uuid {
    sqlx::query("INSERT INTO organizations (id, slug, name) VALUES ($1, 'bridge', 'Bridge Org') ON CONFLICT (id) DO NOTHING")
        .bind(org)
        .execute(owner_pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO users (id, display_name, roles, org_id) VALUES ($1, 'Approver', $2, $3)",
    )
    .bind(recipient)
    .bind(Vec::from(["ADMIN"]))
    .bind(org)
    .execute(owner_pool)
    .await
    .unwrap();
    let definition_id: Uuid = sqlx::query_scalar(
        "INSERT INTO workflow_definitions \
             (org_id, workflow_key, display_name, object_type, status, latest_version, active_version) \
         VALUES ($1, 'compensation.bridge', 'Bridge', 'payroll_period', 'ACTIVE', 1, 1) RETURNING id",
    )
    .bind(org)
    .fetch_one(owner_pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO workflow_definition_versions \
             (org_id, definition_id, version, status, definition, required_approval_line, required_payment_line) \
         VALUES ($1, $2, 1, 'PUBLISHED', '{}'::jsonb, TRUE, TRUE)",
    )
    .bind(org)
    .bind(definition_id)
    .execute(owner_pool)
    .await
    .unwrap();
    let run_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO workflow_runs \
             (id, org_id, definition_id, definition_version, status, trigger_type, \
              idempotency_key, correlation_id, input_payload) \
         VALUES ($1, $2, $3, 1, 'RUNNING', 'OBJECT_EVENT', $4, $5, '{}'::jsonb)",
    )
    .bind(run_id)
    .bind(org)
    .bind(definition_id)
    .bind(format!("bridge-trigger-{run_id}"))
    .bind(format!("bridge-corr-{run_id}"))
    .execute(owner_pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO workflow_outbox_events \
             (org_id, run_id, channel, destination_ref, idempotency_key, status, payload) \
         VALUES ($1, $2, 'NOTIFICATION', 'approval_line', $3, 'PENDING', $4)",
    )
    .bind(org)
    .bind(run_id)
    .bind(format!("bridge-notify-{run_id}"))
    .bind(serde_json::json!({
        "event": "post_finalization_rejection",
        "reason": "예산 초과",
        // A bogus non-UUID entry alongside the real recipient exercises L-2:
        // it is skipped, counted, and does not block delivery of the valid one.
        "recipients": [recipient.to_string(), "not-a-uuid"],
        "recipient_count": 1,
    }))
    .execute(owner_pool)
    .await
    .unwrap();
    run_id
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn notification_outbox_bridges_to_rows_idempotently(owner_pool: PgPool) {
    let rt_pool = runtime_role_pool(&owner_pool).await;
    let org = OrgId::from_uuid(Uuid::from_u128(0x8303_8303_8303_8303_8303_8303_8303_8303));
    let recipient = Uuid::new_v4();
    let run_id = seed(&owner_pool, *org.as_uuid(), recipient).await;

    let store = PgWorkflowRuntimeStore::new(rt_pool.clone());
    let sink = PgNotificationStore::new(rt_pool.clone());

    // First drain: one recipient -> one notification; event flips to DELIVERED.
    let emitted = scope_org(org, store.drain_notification_outbox(org, 100, &sink))
        .await
        .expect("bridge drain");
    assert_eq!(
        emitted, 1,
        "one approval-line recipient yields one notification"
    );

    let (count, category, link_kind, run_ref): (i64, String, String, String) = sqlx::query_as(
        "SELECT COUNT(*)::bigint, MIN(category), MIN(link->>'kind'), MIN(link->>'id') \
         FROM notifications WHERE recipient_user_id = $1",
    )
    .bind(recipient)
    .fetch_one(&owner_pool)
    .await
    .unwrap();
    assert_eq!(count, 1);
    assert_eq!(category, "결재");
    assert_eq!(link_kind, "workflow_run");
    assert_eq!(run_ref, run_id.to_string());

    let status: String =
        sqlx::query_scalar("SELECT status FROM workflow_outbox_events WHERE run_id = $1")
            .bind(run_id)
            .fetch_one(&owner_pool)
            .await
            .unwrap();
    assert_eq!(status, "DELIVERED");

    // Second drain: the event is DELIVERED (nothing to claim) AND the dedup_key
    // would no-op anyway. Exactly one notification survives.
    let emitted_again = scope_org(org, store.drain_notification_outbox(org, 100, &sink))
        .await
        .expect("second bridge drain");
    assert_eq!(
        emitted_again, 0,
        "a re-drain claims nothing and doubles nothing"
    );

    let final_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*)::bigint FROM notifications WHERE recipient_user_id = $1",
    )
    .bind(recipient)
    .fetch_one(&owner_pool)
    .await
    .unwrap();
    assert_eq!(final_count, 1, "idempotent: still exactly one notification");
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn poison_notification_event_dead_letters_at_ceiling(owner_pool: PgPool) {
    let rt_pool = runtime_role_pool(&owner_pool).await;
    let org = OrgId::from_uuid(Uuid::from_u128(0x8404_8404_8404_8404_8404_8404_8404_8404));
    let good = Uuid::new_v4();
    let run_id = seed(&owner_pool, *org.as_uuid(), good).await;

    // Poison event: a syntactically valid recipient UUID with NO users row, so
    // emit FK-fails every drain. Pre-aged to attempt 9 with a due next_attempt_at
    // so ONE drain crosses the dead-letter ceiling (10).
    let poison_key = format!("bridge-poison-{}", Uuid::new_v4());
    let bad = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO workflow_outbox_events \
             (org_id, run_id, channel, destination_ref, idempotency_key, status, payload, \
              attempt_count, next_attempt_at) \
         VALUES ($1, $2, 'NOTIFICATION', 'approval_line', $3, 'FAILED', $4, 9, \
                 now() - interval '1 minute')",
    )
    .bind(org.as_uuid())
    .bind(run_id)
    .bind(&poison_key)
    .bind(serde_json::json!({ "reason": "x", "recipients": [bad.to_string()] }))
    .execute(&owner_pool)
    .await
    .unwrap();

    let store = PgWorkflowRuntimeStore::new(rt_pool.clone());
    let sink = PgNotificationStore::new(rt_pool.clone());

    scope_org(org, store.drain_notification_outbox(org, 100, &sink))
        .await
        .expect("drain");
    let (status, attempts): (String, i32) = sqlx::query_as(
        "SELECT status, attempt_count FROM workflow_outbox_events WHERE idempotency_key = $1",
    )
    .bind(&poison_key)
    .fetch_one(&owner_pool)
    .await
    .unwrap();
    assert_eq!(
        status, "DEAD_LETTERED",
        "poison event dead-letters at the ceiling"
    );
    assert_eq!(attempts, 10);

    // A dead-lettered event is excluded from the claim predicate forever.
    scope_org(org, store.drain_notification_outbox(org, 100, &sink))
        .await
        .expect("second drain");
    let after: String =
        sqlx::query_scalar("SELECT status FROM workflow_outbox_events WHERE idempotency_key = $1")
            .bind(&poison_key)
            .fetch_one(&owner_pool)
            .await
            .unwrap();
    assert_eq!(after, "DEAD_LETTERED", "never re-claimed after dead-letter");
}
