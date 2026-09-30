#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Concurrent consumption retries must replay the committed event, rather than
//! turn a uniqueness race into a generic conflict.

use console_inventory_adapter_postgres::PgInventoryStore;
use console_inventory_application::{
    ConsumeInventoryCommand, ConsumeInventorySource, CycleCountDecision, DecideCycleCountCommand,
    MovementSourceView, OpenCycleCountCommand, RecordReceiptCommand, SubmitCycleCountCommand,
    UpsertCountLineCommand,
};
use console_inventory_domain::VarianceReason;
use console_kernel_core::{
    BranchId, BranchScope, ErrorKind, InventoryItemId, InventoryStockLocationId, OrgId,
    P1DispatchId, TraceContext, WorkOrderId,
};
use console_platform_request_context::scope_org;
use console_platform_test_support::{grant_console_rt, seed_org_and_super_admin};
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use std::sync::Arc;
use time::OffsetDateTime;
use tokio::sync::Barrier;
use uuid::Uuid;

const ORG: Uuid = Uuid::from_u128(0x1A11_D3A0_0000_0000_0000_0000_0000_0001);
const OTHER_ORG: Uuid = Uuid::from_u128(0x2A11_D3A0_0000_0000_0000_0000_0000_0002);
const IDEMPOTENCY_KEY: &str = "inventory-consume-race-key";

#[test]
fn movement_sources_serialize_as_the_closed_discriminated_wire_contract() {
    let work_order_id = WorkOrderId::from_uuid(Uuid::from_u128(1));
    let dispatch_id = P1DispatchId::from_uuid(Uuid::from_u128(2));
    let count_id = Uuid::from_u128(3);
    let cases = [
        (
            MovementSourceView::WorkOrder { work_order_id },
            serde_json::json!({"kind": "work_order", "workOrderId": work_order_id}),
        ),
        (
            MovementSourceView::P1Dispatch {
                dispatch_id,
                work_order_id,
            },
            serde_json::json!({"kind": "p1_dispatch", "dispatchId": dispatch_id, "workOrderId": work_order_id}),
        ),
        (
            MovementSourceView::CycleCount {
                cycle_count_id: count_id,
                cc_code: "IC-0001".to_owned(),
            },
            serde_json::json!({"kind": "cycle_count", "cycleCountId": count_id, "ccCode": "IC-0001"}),
        ),
        (
            MovementSourceView::ExternalRef { source_ref: None },
            serde_json::json!({"kind": "external_ref", "sourceRef": null}),
        ),
    ];

    for (source, expected) in cases {
        assert_eq!(serde_json::to_value(source).unwrap(), expected);
    }
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn concurrent_identical_consumptions_replay_once_and_payload_mismatch_conflicts(
    owner_pool: PgPool,
) {
    grant_console_rt(
        &owner_pool,
        &[
            "GRANT SELECT ON work_orders TO console_rt",
            "GRANT SELECT, INSERT, UPDATE ON inventory_stock_locations TO console_rt",
            "GRANT SELECT, INSERT, UPDATE ON inventory_items TO console_rt",
            "GRANT SELECT, INSERT ON inventory_consumption_events TO console_rt",
            "GRANT SELECT, INSERT ON audit_events TO console_rt",
        ],
    )
    .await;

    let org = OrgId::from_uuid(ORG);
    let actor = seed_org_and_super_admin(&owner_pool, ORG, "inventory-idempotency").await;
    let (branch, item_id, work_order_id) = seed_consumption_fixture(&owner_pool, ORG, actor).await;
    let pool = two_connection_runtime_role_pool(&owner_pool).await;
    let store = PgInventoryStore::new(pool);
    let now = OffsetDateTime::now_utc();
    let command = consume_command(actor, branch, item_id, work_order_id, 300, Some(now), now);

    // Each task obtains a separate transaction/connection from the runtime pool.
    // The barrier makes both callers contend on the same durable idempotency key.
    let barrier = Arc::new(Barrier::new(2));
    let first = {
        let barrier = Arc::clone(&barrier);
        let store = store.clone();
        let command = command.clone();
        tokio::spawn(async move {
            barrier.wait().await;
            scope_org(org, store.consume_item(command)).await
        })
    };
    let second = {
        let barrier = Arc::clone(&barrier);
        let store = store.clone();
        let command = command.clone();
        tokio::spawn(async move {
            barrier.wait().await;
            scope_org(org, store.consume_item(command)).await
        })
    };

    let (first, second) = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        tokio::join!(first, second)
    })
    .await
    .expect("same-payload advisory-lock race must finish promptly");
    let first = first.unwrap().unwrap();
    let second = second.unwrap().unwrap();
    assert_eq!(
        first.event.id, second.event.id,
        "both callers replay one event"
    );
    assert_eq!(first.item.quantity_on_hand_milli, 700);
    assert_eq!(second.item.quantity_on_hand_milli, 700);

    let events: i64 =
        sqlx::query_scalar("SELECT count(*) FROM inventory_consumption_events WHERE item_id = $1")
            .bind(*item_id.as_uuid())
            .fetch_one(&owner_pool)
            .await
            .unwrap();
    let stock: i64 =
        sqlx::query_scalar("SELECT quantity_on_hand_milli FROM inventory_items WHERE id = $1")
            .bind(*item_id.as_uuid())
            .fetch_one(&owner_pool)
            .await
            .unwrap();
    let audits: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_events WHERE org_id = $1 AND action = 'inventory.consume'",
    )
    .bind(ORG)
    .fetch_one(&owner_pool)
    .await
    .unwrap();
    assert_eq!(events, 1, "one consumption event is recorded");
    assert_eq!(stock, 700, "stock is decremented once");
    assert_eq!(audits, 1, "one consumption audit event is recorded");

    let mismatch = scope_org(
        org,
        store.consume_item(ConsumeInventoryCommand {
            quantity_consumed_milli: 200,
            ..command.clone()
        }),
    )
    .await
    .expect_err("a reused key with a different payload must conflict");
    assert_eq!(mismatch.kind(), ErrorKind::Conflict);

    let timestamp_mismatch = scope_org(
        org,
        store.consume_item(ConsumeInventoryCommand {
            quantity_consumed_milli: 300,
            occurred_at: Some(now + time::Duration::seconds(1)),
            ..command
        }),
    )
    .await
    .expect_err("a reused key with a different explicit occurrence timestamp must conflict");
    assert_eq!(timestamp_mismatch.kind(), ErrorKind::Conflict);

    let events_after: i64 =
        sqlx::query_scalar("SELECT count(*) FROM inventory_consumption_events WHERE item_id = $1")
            .bind(*item_id.as_uuid())
            .fetch_one(&owner_pool)
            .await
            .unwrap();
    let stock_after: i64 =
        sqlx::query_scalar("SELECT quantity_on_hand_milli FROM inventory_items WHERE id = $1")
            .bind(*item_id.as_uuid())
            .fetch_one(&owner_pool)
            .await
            .unwrap();
    let audits_after: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_events WHERE org_id = $1 AND action = 'inventory.consume'",
    )
    .bind(ORG)
    .fetch_one(&owner_pool)
    .await
    .unwrap();
    assert_eq!(events_after, 1, "a mismatch records no second event");
    assert_eq!(stock_after, 700, "a mismatch leaves stock unchanged");
    assert_eq!(audits_after, 1, "a mismatch records no second audit");

    assert_tenant_and_key_locks_do_not_block(&owner_pool).await;

    let other_org = OrgId::from_uuid(OTHER_ORG);
    let other_actor =
        seed_org_and_super_admin(&owner_pool, OTHER_ORG, "inventory-idempotency-other").await;
    let (other_branch, other_item_id, other_work_order_id) =
        seed_consumption_fixture(&owner_pool, OTHER_ORG, other_actor).await;
    let other = scope_org(
        other_org,
        store.consume_item(consume_command(
            other_actor,
            other_branch,
            other_item_id,
            other_work_order_id,
            300,
            Some(now),
            now,
        )),
    )
    .await
    .expect("the same raw key in another tenant must not replay the first tenant event");
    assert_ne!(other.event.id, first.event.id);
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn omitted_occurrence_time_replays_despite_server_requested_at_and_rejects_explicit_presence(
    owner_pool: PgPool,
) {
    grant_console_rt(
        &owner_pool,
        &[
            "GRANT SELECT ON work_orders TO console_rt",
            "GRANT SELECT, INSERT, UPDATE ON inventory_stock_locations TO console_rt",
            "GRANT SELECT, INSERT, UPDATE ON inventory_items TO console_rt",
            "GRANT SELECT, INSERT ON inventory_consumption_events TO console_rt",
            "GRANT SELECT, INSERT ON audit_events TO console_rt",
        ],
    )
    .await;

    let org = OrgId::from_uuid(ORG);
    let actor = seed_org_and_super_admin(&owner_pool, ORG, "inventory-omitted-time").await;
    let (branch, item_id, work_order_id) = seed_consumption_fixture(&owner_pool, ORG, actor).await;
    let store = PgInventoryStore::new(two_connection_runtime_role_pool(&owner_pool).await);
    let requested_at = OffsetDateTime::now_utc();
    let first = consume_command(
        actor,
        branch,
        item_id,
        work_order_id,
        300,
        None,
        requested_at,
    );
    let second = ConsumeInventoryCommand {
        requested_at: requested_at + time::Duration::seconds(30),
        ..first.clone()
    };

    let first_result = scope_org(org, store.consume_item(first.clone()))
        .await
        .expect("the initial omitted-time request succeeds");
    let replay = scope_org(org, store.consume_item(second))
        .await
        .expect("omitted occurrence time must ignore server requested_at for replay");
    assert_eq!(first_result.event.id, replay.event.id);
    assert_eq!(replay.item.quantity_on_hand_milli, 700);

    let presence_mismatch = scope_org(
        org,
        store.consume_item(ConsumeInventoryCommand {
            occurred_at: Some(requested_at),
            ..first
        }),
    )
    .await
    .expect_err("explicit occurrence time must not replay an omitted occurrence time");
    assert_eq!(presence_mismatch.kind(), ErrorKind::Conflict);

    let events: i64 =
        sqlx::query_scalar("SELECT count(*) FROM inventory_consumption_events WHERE item_id = $1")
            .bind(*item_id.as_uuid())
            .fetch_one(&owner_pool)
            .await
            .unwrap();
    let stock: i64 =
        sqlx::query_scalar("SELECT quantity_on_hand_milli FROM inventory_items WHERE id = $1")
            .bind(*item_id.as_uuid())
            .fetch_one(&owner_pool)
            .await
            .unwrap();
    let audits: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_events WHERE org_id = $1 AND action = 'inventory.consume'",
    )
    .bind(ORG)
    .fetch_one(&owner_pool)
    .await
    .unwrap();
    assert_eq!(events, 1, "omitted replay records one event");
    assert_eq!(stock, 700, "omitted replay decrements stock once");
    assert_eq!(audits, 1, "omitted replay records one audit");
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn concurrent_explicit_timestamp_mismatch_conflicts_without_second_mutation(
    owner_pool: PgPool,
) {
    grant_console_rt(
        &owner_pool,
        &[
            "GRANT SELECT ON work_orders TO console_rt",
            "GRANT SELECT, INSERT, UPDATE ON inventory_stock_locations TO console_rt",
            "GRANT SELECT, INSERT, UPDATE ON inventory_items TO console_rt",
            "GRANT SELECT, INSERT ON inventory_consumption_events TO console_rt",
            "GRANT SELECT, INSERT ON audit_events TO console_rt",
        ],
    )
    .await;

    let org = OrgId::from_uuid(ORG);
    let actor = seed_org_and_super_admin(&owner_pool, ORG, "inventory-timestamp-race").await;
    let (branch, item_id, work_order_id) = seed_consumption_fixture(&owner_pool, ORG, actor).await;
    let store = PgInventoryStore::new(two_connection_runtime_role_pool(&owner_pool).await);
    let now = OffsetDateTime::now_utc();
    let first_command = consume_command(actor, branch, item_id, work_order_id, 300, Some(now), now);
    let second_command = ConsumeInventoryCommand {
        occurred_at: Some(now + time::Duration::seconds(1)),
        ..first_command.clone()
    };
    let barrier = Arc::new(Barrier::new(2));
    let first = {
        let barrier = Arc::clone(&barrier);
        let store = store.clone();
        tokio::spawn(async move {
            barrier.wait().await;
            scope_org(org, store.consume_item(first_command)).await
        })
    };
    let second = {
        let barrier = Arc::clone(&barrier);
        let store = store.clone();
        tokio::spawn(async move {
            barrier.wait().await;
            scope_org(org, store.consume_item(second_command)).await
        })
    };

    let (first, second) = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        tokio::join!(first, second)
    })
    .await
    .expect("timestamp-mismatch advisory-lock race must finish promptly");
    let first = first.unwrap();
    let second = second.unwrap();
    let winner = match (first, second) {
        (Ok(winner), Err(error)) | (Err(error), Ok(winner)) => {
            assert_eq!(error.kind(), ErrorKind::Conflict);
            winner
        }
        (left, right) => {
            panic!("expected one success and one timestamp conflict, got {left:?}, {right:?}")
        }
    };
    assert_eq!(winner.item.quantity_on_hand_milli, 700);

    let events: i64 =
        sqlx::query_scalar("SELECT count(*) FROM inventory_consumption_events WHERE item_id = $1")
            .bind(*item_id.as_uuid())
            .fetch_one(&owner_pool)
            .await
            .unwrap();
    let stock: i64 =
        sqlx::query_scalar("SELECT quantity_on_hand_milli FROM inventory_items WHERE id = $1")
            .bind(*item_id.as_uuid())
            .fetch_one(&owner_pool)
            .await
            .unwrap();
    let audits: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_events WHERE org_id = $1 AND action = 'inventory.consume'",
    )
    .bind(ORG)
    .fetch_one(&owner_pool)
    .await
    .unwrap();
    assert_eq!(events, 1, "timestamp mismatch records no second event");
    assert_eq!(stock, 700, "timestamp mismatch decrements stock once");
    assert_eq!(audits, 1, "timestamp mismatch records one audit");
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn cycle_approval_replays_once_and_applies_immutable_variance_to_current_stock(
    owner_pool: PgPool,
) {
    grant_console_rt(
        &owner_pool,
        &[
            "GRANT SELECT, INSERT, UPDATE ON inventory_stock_locations TO console_rt",
            "GRANT SELECT, INSERT, UPDATE ON inventory_items TO console_rt",
            "GRANT SELECT, INSERT ON inventory_consumption_events TO console_rt",
            "GRANT SELECT, INSERT, UPDATE ON inventory_cycle_counts TO console_rt",
            "GRANT SELECT, INSERT, UPDATE ON inventory_cycle_count_counters TO console_rt",
            "GRANT SELECT, INSERT, UPDATE ON inventory_cycle_count_lines TO console_rt",
            "GRANT SELECT, INSERT ON inventory_movements TO console_rt",
            "GRANT SELECT, INSERT ON audit_events TO console_rt",
        ],
    )
    .await;
    let org = OrgId::from_uuid(ORG);
    let maker = seed_org_and_super_admin(&owner_pool, ORG, "inventory-cycle-maker").await;
    let checker = seed_org_and_super_admin(&owner_pool, ORG, "inventory-cycle-checker").await;
    let other_checker =
        seed_org_and_super_admin(&owner_pool, ORG, "inventory-cycle-other-checker").await;
    let (branch, item_id, work_order_id) = seed_consumption_fixture(&owner_pool, ORG, maker).await;
    let location_id: Uuid =
        sqlx::query_scalar("SELECT stock_location_id FROM inventory_items WHERE id=$1")
            .bind(*item_id.as_uuid())
            .fetch_one(&owner_pool)
            .await
            .unwrap();
    let store = PgInventoryStore::new(two_connection_runtime_role_pool(&owner_pool).await);
    let now = OffsetDateTime::now_utc();
    let opened = scope_org(
        org,
        store.open_cycle_count(OpenCycleCountCommand {
            actor: maker,
            branch_scope: BranchScope::All,
            branch_id: branch,
            stock_location_id: InventoryStockLocationId::from_uuid(location_id),
            trace: TraceContext::generate(),
            occurred_at: now,
        }),
    )
    .await
    .unwrap();
    let line = scope_org(
        org,
        store.upsert_cycle_count_line(UpsertCountLineCommand {
            actor: maker,
            branch_scope: BranchScope::All,
            count_id: opened.count.id,
            expected_version: opened.count.version,
            item_id,
            counted_quantity_milli: 800,
            reason: Some(VarianceReason::Miscount),
            note: Some("physical count".to_owned()),
            trace: TraceContext::generate(),
            occurred_at: now,
        }),
    )
    .await
    .unwrap();
    let submitted = scope_org(
        org,
        store.submit_cycle_count(SubmitCycleCountCommand {
            actor: maker,
            branch_scope: BranchScope::All,
            count_id: opened.count.id,
            expected_version: line.count.version,
            trace: TraceContext::generate(),
            occurred_at: now,
        }),
    )
    .await
    .unwrap();
    // An intervening receipt changes current stock from the counted snapshot of 1000 to 1100.
    scope_org(
        org,
        store.record_receipt(RecordReceiptCommand {
            actor: maker,
            branch_scope: BranchScope::All,
            item_id,
            quantity_received_milli: 100,
            source_ref: Some("PO-119".to_owned()),
            memo: None,
            idempotency_key: "inventory-cycle-intervening-receipt".to_owned(),
            trace: TraceContext::generate(),
            requested_at: now,
        }),
    )
    .await
    .unwrap();
    let approval = DecideCycleCountCommand {
        actor: checker,
        branch_scope: BranchScope::All,
        count_id: opened.count.id,
        expected_version: submitted.count.version,
        decision: CycleCountDecision::Approve,
        memo: Some("verified".to_owned()),
        idempotency_key: Some("inventory-cycle-approval-key".to_owned()),
        trace: TraceContext::generate(),
        occurred_at: now,
    };
    // Two independently checked-out console_rt connections race the exact same
    // approval. The durable key plus row lock must produce one adjustment and
    // one audit, with the losing caller replaying the committed decision.
    let approval_barrier = Arc::new(Barrier::new(2));
    let first_approval = {
        let barrier = Arc::clone(&approval_barrier);
        let store = store.clone();
        let approval = approval.clone();
        tokio::spawn(async move {
            barrier.wait().await;
            scope_org(org, store.decide_cycle_count(approval)).await
        })
    };
    let second_approval = {
        let barrier = Arc::clone(&approval_barrier);
        let store = store.clone();
        let approval = approval.clone();
        tokio::spawn(async move {
            barrier.wait().await;
            scope_org(org, store.decide_cycle_count(approval)).await
        })
    };
    let (first_approval, second_approval) =
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            tokio::join!(first_approval, second_approval)
        })
        .await
        .expect("identical cycle approval race must finish promptly");
    let approved = first_approval.unwrap().unwrap();
    let replay = second_approval.unwrap().unwrap();
    assert_eq!(
        approved.count.version, replay.count.version,
        "same canonical approval race replays one stored outcome"
    );
    let stock: i64 =
        sqlx::query_scalar("SELECT quantity_on_hand_milli FROM inventory_items WHERE id=$1")
            .bind(*item_id.as_uuid())
            .fetch_one(&owner_pool)
            .await
            .unwrap();
    assert_eq!(
        stock, 900,
        "immutable -200 variance applies to locked current 1100 stock"
    );
    let adjustments: (i64, i64, i64, i64) = sqlx::query_as("SELECT count(*), min(quantity_before_milli), min(quantity_delta_milli), min(quantity_after_milli) FROM inventory_movements WHERE cycle_count_id=$1")
        .bind(opened.count.id).fetch_one(&owner_pool).await.unwrap();
    assert_eq!(adjustments, (1, 1100, -200, 900));
    let decision_audits: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_events WHERE org_id=$1 AND action='inventory.cycle_count.decide' AND target_id=$2",
    )
    .bind(ORG)
    .bind(opened.count.id.to_string())
    .fetch_one(&owner_pool)
    .await
    .unwrap();
    assert_eq!(
        decision_audits, 1,
        "approval race records exactly one decision audit"
    );
    let changed_memo = scope_org(
        org,
        store.decide_cycle_count(DecideCycleCountCommand {
            memo: Some("different".to_owned()),
            ..approval.clone()
        }),
    )
    .await
    .unwrap_err();
    assert_eq!(changed_memo.kind(), ErrorKind::Conflict);
    let changed_actor = scope_org(
        org,
        store.decide_cycle_count(DecideCycleCountCommand {
            actor: other_checker,
            ..approval.clone()
        }),
    )
    .await
    .unwrap_err();
    assert_eq!(changed_actor.kind(), ErrorKind::Conflict);

    // An approval that would drive current stock negative must roll its state
    // transition and audit back with the failed adjustment transaction.
    let unsafe_count = scope_org(
        org,
        store.open_cycle_count(OpenCycleCountCommand {
            actor: maker,
            branch_scope: BranchScope::All,
            branch_id: branch,
            stock_location_id: InventoryStockLocationId::from_uuid(location_id),
            trace: TraceContext::generate(),
            occurred_at: now,
        }),
    )
    .await
    .unwrap();
    let unsafe_line = scope_org(
        org,
        store.upsert_cycle_count_line(UpsertCountLineCommand {
            actor: maker,
            branch_scope: BranchScope::All,
            count_id: unsafe_count.count.id,
            expected_version: unsafe_count.count.version,
            item_id,
            counted_quantity_milli: 0,
            reason: Some(VarianceReason::Loss),
            note: Some("counted missing stock".to_owned()),
            trace: TraceContext::generate(),
            occurred_at: now,
        }),
    )
    .await
    .unwrap();
    let unsafe_submitted = scope_org(
        org,
        store.submit_cycle_count(SubmitCycleCountCommand {
            actor: maker,
            branch_scope: BranchScope::All,
            count_id: unsafe_count.count.id,
            expected_version: unsafe_line.count.version,
            trace: TraceContext::generate(),
            occurred_at: now,
        }),
    )
    .await
    .unwrap();
    scope_org(
        org,
        store.consume_item(ConsumeInventoryCommand {
            actor: maker,
            branch_scope: BranchScope::All,
            item_id,
            source: ConsumeInventorySource::WorkOrder { work_order_id },
            quantity_consumed_milli: 500,
            occurred_at: Some(now),
            memo: Some("intervening issue".to_owned()),
            idempotency_key: "inventory-cycle-unsafe-intervening-issue".to_owned(),
            trace: TraceContext::generate(),
            requested_at: now,
        }),
    )
    .await
    .unwrap();
    let unsafe_approval = scope_org(
        org,
        store.decide_cycle_count(DecideCycleCountCommand {
            actor: checker,
            branch_scope: BranchScope::All,
            count_id: unsafe_count.count.id,
            expected_version: unsafe_submitted.count.version,
            decision: CycleCountDecision::Approve,
            memo: Some("would underflow".to_owned()),
            idempotency_key: Some("inventory-cycle-unsafe-approval-key".to_owned()),
            trace: TraceContext::generate(),
            occurred_at: now,
        }),
    )
    .await
    .unwrap_err();
    assert_eq!(unsafe_approval.kind(), ErrorKind::Conflict);
    let state: String = sqlx::query_scalar("SELECT status FROM inventory_cycle_counts WHERE id=$1")
        .bind(unsafe_count.count.id)
        .fetch_one(&owner_pool)
        .await
        .unwrap();
    let unsafe_movements: i64 =
        sqlx::query_scalar("SELECT count(*) FROM inventory_movements WHERE cycle_count_id=$1")
            .bind(unsafe_count.count.id)
            .fetch_one(&owner_pool)
            .await
            .unwrap();
    assert_eq!(
        state, "SUBMITTED",
        "failed approval rolls back its status update"
    );
    assert_eq!(
        unsafe_movements, 0,
        "failed approval leaves no partial adjustment"
    );
    let unsafe_decision_audits: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_events WHERE org_id=$1 AND action='inventory.cycle_count.decide' AND target_id=$2",
    )
    .bind(ORG)
    .bind(unsafe_count.count.id.to_string())
    .fetch_one(&owner_pool)
    .await
    .unwrap();
    assert_eq!(
        unsafe_decision_audits, 0,
        "underflow rollback leaves no cycle decision audit"
    );

    let denied_branch = scope_org(
        org,
        store.get_cycle_count(opened.count.id, BranchScope::single(BranchId::new())),
    )
    .await
    .unwrap_err();
    assert_eq!(
        denied_branch.kind(),
        ErrorKind::Forbidden,
        "tenant-visible count rejects a nonempty unauthorized branch scope"
    );

    let foreign_org = OrgId::from_uuid(OTHER_ORG);
    seed_org_and_super_admin(&owner_pool, OTHER_ORG, "inventory-cycle-foreign-tenant").await;
    let foreign_tenant = scope_org(
        foreign_org,
        store.get_cycle_count(opened.count.id, BranchScope::All),
    )
    .await
    .unwrap();
    assert!(
        foreign_tenant.is_none(),
        "foreign tenant cannot discover this cycle count even with an all-branch scope"
    );
}

fn consume_command(
    actor: console_kernel_core::UserId,
    branch: BranchId,
    item_id: InventoryItemId,
    work_order_id: WorkOrderId,
    quantity_consumed_milli: i64,
    occurred_at: Option<OffsetDateTime>,
    now: OffsetDateTime,
) -> ConsumeInventoryCommand {
    ConsumeInventoryCommand {
        actor,
        branch_scope: BranchScope::single(branch),
        item_id,
        source: ConsumeInventorySource::WorkOrder { work_order_id },
        quantity_consumed_milli,
        occurred_at,
        memo: Some("concurrent idempotency regression".to_owned()),
        idempotency_key: IDEMPOTENCY_KEY.to_owned(),
        trace: TraceContext::generate(),
        requested_at: now,
    }
}

async fn assert_tenant_and_key_locks_do_not_block(pool: &PgPool) {
    let mut first = pool.begin().await.unwrap();
    sqlx::query(
        "SELECT pg_advisory_xact_lock(hashtextextended(\
            char_length($1::text)::text || ':' || $1::text || \
            char_length($2::text)::text || ':' || $2::text, \
            0\
        ))",
    )
    .bind(ORG.to_string())
    .bind(IDEMPOTENCY_KEY)
    .execute(&mut *first)
    .await
    .unwrap();

    let mut second = pool.begin().await.unwrap();
    // `pg_try_advisory_xact_lock` is bounded: a distinct raw tenant/key must
    // acquire immediately while the first transaction still holds its lock.
    let acquired: bool = sqlx::query_scalar(
        "SELECT pg_try_advisory_xact_lock(hashtextextended(\
            char_length($1::text)::text || ':' || $1::text || \
            char_length($2::text)::text || ':' || $2::text, \
            0\
        ))",
    )
    .bind(OTHER_ORG.to_string())
    .bind(IDEMPOTENCY_KEY)
    .fetch_one(&mut *second)
    .await
    .unwrap();
    assert!(
        acquired,
        "a distinct tenant/key must not block behind this lock"
    );
    let distinct_key_acquired: bool = sqlx::query_scalar(
        "SELECT pg_try_advisory_xact_lock(hashtextextended(\
            char_length($1::text)::text || ':' || $1::text || \
            char_length($2::text)::text || ':' || $2::text, \
            0\
        ))",
    )
    .bind(ORG.to_string())
    .bind(format!("{IDEMPOTENCY_KEY}-distinct"))
    .fetch_one(&mut *second)
    .await
    .unwrap();
    assert!(
        distinct_key_acquired,
        "a sufficiently distinct key must not block behind this lock"
    );
    second.commit().await.unwrap();
    first.commit().await.unwrap();
}

async fn two_connection_runtime_role_pool(owner_pool: &PgPool) -> PgPool {
    let options = owner_pool.connect_options().as_ref().clone();
    PgPoolOptions::new()
        .max_connections(2)
        .after_connect(|conn, _meta| {
            Box::pin(async move {
                sqlx::query("SET ROLE console_rt").execute(conn).await?;
                Ok(())
            })
        })
        .connect_with(options)
        .await
        .expect("connect two console_rt-role test connections")
}

async fn seed_consumption_fixture(
    pool: &PgPool,
    org: Uuid,
    actor: console_kernel_core::UserId,
) -> (BranchId, InventoryItemId, WorkOrderId) {
    let region_id = Uuid::new_v4();
    let branch_id = Uuid::new_v4();
    let customer_id = Uuid::new_v4();
    let site_id = Uuid::new_v4();
    let equipment_id = Uuid::new_v4();
    let work_order_id = Uuid::new_v4();
    let location_id = Uuid::new_v4();
    let item_id = Uuid::new_v4();
    let suffix = if org == ORG { "one" } else { "two" };
    let equipment_no = if org == ORG {
        "INVRA-0001"
    } else {
        "INVRA-0002"
    };
    let request_no = if org == ORG {
        "20260724-001"
    } else {
        "20260724-002"
    };
    let iv_code = if org == ORG {
        "IV-RACE-001"
    } else {
        "IV-RACE-002"
    };

    sqlx::query("INSERT INTO regions (id, name, org_id) VALUES ($1, $2, $3)")
        .bind(region_id)
        .bind(format!("inventory-idempotency-region-{suffix}"))
        .bind(org)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO branches (id, region_id, name, org_id) VALUES ($1, $2, $3, $4)")
        .bind(branch_id)
        .bind(region_id)
        .bind(format!("inventory-idempotency-branch-{suffix}"))
        .bind(org)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO registry_customers (id, branch_id, name, org_id) VALUES ($1, $2, 'Inventory customer', $3)")
        .bind(customer_id)
        .bind(branch_id)
        .bind(org)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO registry_sites (id, branch_id, customer_id, name, org_id) VALUES ($1, $2, $3, 'Inventory site', $4)")
        .bind(site_id)
        .bind(branch_id)
        .bind(customer_id)
        .bind(org)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO registry_equipment (id, branch_id, customer_id, site_id, equipment_no, manufacturer_code, kind_code, power_code, status, specification, ton_text, source_sheet, source_row, org_id) VALUES ($1, $2, $3, $4, $5, 'INV', 'TEST', 'DIESEL', '임대', 'test equipment', '1', 'test', 1, $6)",
    )
    .bind(equipment_id)
    .bind(branch_id)
    .bind(customer_id)
    .bind(site_id)
    .bind(equipment_no)
    .bind(org)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO work_orders (id, request_no, branch_id, equipment_id, customer_id, site_id, requested_by, status, symptom, org_id) VALUES ($1, $2, $3, $4, $5, $6, $7, 'RECEIVED', 'inventory test', $8)",
    )
    .bind(work_order_id)
    .bind(request_no)
    .bind(branch_id)
    .bind(equipment_id)
    .bind(customer_id)
    .bind(site_id)
    .bind(*actor.as_uuid())
    .bind(org)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO inventory_stock_locations (id, org_id, branch_id, label, status) VALUES ($1, $2, $3, 'Inventory test location', 'ACTIVE')",
    )
    .bind(location_id)
    .bind(org)
    .bind(branch_id)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO inventory_items (id, org_id, branch_id, stock_location_id, iv_code, display_name, unit_code, quantity_on_hand_milli, safety_stock_milli, unit_cost_won, status, created_by) VALUES ($1, $2, $3, $4, $5, 'Inventory race item', 'EA', 1000, 0, 100, 'ACTIVE', $6)",
    )
    .bind(item_id)
    .bind(org)
    .bind(branch_id)
    .bind(location_id)
    .bind(iv_code)
    .bind(*actor.as_uuid())
    .execute(pool)
    .await
    .unwrap();

    (
        BranchId::from_uuid(branch_id),
        InventoryItemId::from_uuid(item_id),
        WorkOrderId::from_uuid(work_order_id),
    )
}
