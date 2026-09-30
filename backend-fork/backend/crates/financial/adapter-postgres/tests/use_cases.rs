#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use console_financial_adapter_postgres::PgFinancialStore;
use console_financial_application::{
    AppendCostLedgerEntryCommand, CostLedgerSource, CreatePurchaseRequestCommand,
    CreateRentalQuoteCommand, ExecutePurchaseCommand, FinancialConfigSnapshot,
    PrepareExpenditureCommand, PurchaseApprovalCommand, PurchaseRequestLineInput,
    PurchaseRestartCommand, PurchaseSubmitCommand, PurchaseType, RejectPurchaseCommand,
};
use console_financial_domain::{DepreciationMethod, PurchaseStatus};
use console_kernel_core::{
    BranchId, EquipmentId, EvidenceId, OrgId, TraceContext, UserId, WorkOrderId,
};
use sqlx::PgPool;
use time::macros::datetime;

static REQUEST_NO_SEQUENCE: AtomicUsize = AtomicUsize::new(901);

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn quote_ledger_and_purchase_chain_are_audited_and_feed_residuals(pool: PgPool) {
    console_platform_request_context::scope_org(console_kernel_core::OrgId::knl(), async move {
        let seeded = seed_financial_context(&pool).await;
        let store = PgFinancialStore::new(pool.clone());
        let occurred_at = datetime!(2026-06-12 12:00 UTC);
        let config = financial_config();

        let quote = store
            .create_rental_quote(CreateRentalQuoteCommand {
                actor: seeded.receptionist,
                branch_id: seeded.branch_id,
                equipment_id: seeded.negative_residual_equipment,
                config: config.clone(),
                trace: TraceContext::generate(),
                occurred_at,
            })
            .await
            .unwrap();
        assert!(quote.residual_was_floored);
        assert!(quote.monthly_total.amount() > 0);

        let manual_entry = store
            .append_cost_ledger_entry(AppendCostLedgerEntryCommand {
                actor: seeded.admin,
                branch_id: seeded.branch_id,
                equipment_id: seeded.normal_equipment,
                work_order_id: Some(seeded.work_order_id),
                source: CostLedgerSource::ManualAdmin,
                amount_won: 1_500_000,
                memo: "Hydraulic pump repair".to_owned(),
                config: config.clone(),
                trace: TraceContext::generate(),
                occurred_at,
            })
            .await
            .unwrap();
        assert_eq!(manual_entry.residual_after_won, 5_100_000);

        let purchase = store
            .create_purchase_request(CreatePurchaseRequestCommand {
                actor: seeded.mechanic,
                branch_id: seeded.branch_id,
                equipment_id: Some(seeded.normal_equipment),
                work_order_id: Some(seeded.work_order_id),
                statement_evidence_id: Some(seeded.statement_evidence_id),
                purchase_type: PurchaseType::LegacyManual,
                vendor_name: "Parts Supplier".to_owned(),
                amount_won: Some(3_000_000),
                lines: vec![manual_purchase_line("Pump assembly", 3_000_000)],
                quote_attachment_ids: Vec::new(),
                memo: "Pump assembly".to_owned(),
                config: config.clone(),
                trace: TraceContext::generate(),
                occurred_at,
            })
            .await
            .unwrap();
        assert_eq!(purchase.status, PurchaseStatus::StatementAttached);

        let submitted = store
            .submit_purchase_request(PurchaseSubmitCommand {
                actor: seeded.receptionist,
                purchase_request_id: purchase.id,
                trace: TraceContext::generate(),
                occurred_at,
            })
            .await
            .unwrap();
        assert_eq!(submitted.status, PurchaseStatus::RequestSubmitted);

        let rejected = store
            .reject_purchase_request(RejectPurchaseCommand {
                actor: seeded.admin,
                purchase_request_id: purchase.id,
                memo: "Attach corrected quotation".to_owned(),
                trace: TraceContext::generate(),
                occurred_at,
            })
            .await
            .unwrap();
        assert_eq!(rejected.status, PurchaseStatus::Rejected);

        let restarted = store
            .restart_purchase_request(PurchaseRestartCommand {
                actor: seeded.receptionist,
                purchase_request_id: purchase.id,
                statement_evidence_id: Some(seeded.statement_evidence_id),
                amount_won: Some(3_000_000),
                lines: vec![manual_purchase_line("Corrected quotation", 3_000_000)],
                quote_attachment_ids: Vec::new(),
                memo: "Corrected quotation attached".to_owned(),
                trace: TraceContext::generate(),
                occurred_at,
            })
            .await
            .unwrap();
        assert_eq!(restarted.status, PurchaseStatus::StatementAttached);

        store
            .submit_purchase_request(PurchaseSubmitCommand {
                actor: seeded.receptionist,
                purchase_request_id: purchase.id,
                trace: TraceContext::generate(),
                occurred_at,
            })
            .await
            .unwrap();
        store
            .approve_purchase_admin(PurchaseApprovalCommand {
                actor: seeded.admin,
                purchase_request_id: purchase.id,
                trace: TraceContext::generate(),
                occurred_at,
            })
            .await
            .unwrap();
        let executive_pending = store
            .prepare_expenditure(PrepareExpenditureCommand {
                actor: seeded.receptionist,
                purchase_request_id: purchase.id,
                expenditure_no: "EXP-20260612-001".to_owned(),
                trace: TraceContext::generate(),
                occurred_at,
            })
            .await
            .unwrap();
        assert_eq!(executive_pending.status, PurchaseStatus::ExecutivePending);

        store
            .approve_purchase_executive(PurchaseApprovalCommand {
                actor: seeded.executive,
                purchase_request_id: purchase.id,
                trace: TraceContext::generate(),
                occurred_at,
            })
            .await
            .unwrap();

        let executed = store
            .execute_purchase(ExecutePurchaseCommand {
                actor: seeded.receptionist,
                purchase_request_id: purchase.id,
                trace: TraceContext::generate(),
                occurred_at,
            })
            .await
            .unwrap();
        assert_eq!(executed.status, PurchaseStatus::Executed);

        let residual: i64 =
            sqlx::query_scalar("SELECT residual_value FROM registry_equipment WHERE id = $1")
                .bind(*seeded.normal_equipment.as_uuid())
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(residual, 2_100_000);

        let actions: Vec<String> = sqlx::query_scalar(
            "SELECT action FROM audit_events WHERE branch_id = $1 ORDER BY occurred_at, created_at",
        )
        .bind(*seeded.branch_id.as_uuid())
        .fetch_all(&pool)
        .await
        .unwrap();
        assert!(actions.contains(&"financial.quote.create".to_owned()));
        assert!(actions.contains(&"equipment.residual.recompute".to_owned()));
        assert!(actions.contains(&"purchase.execute".to_owned()));
    })
    .await;
}

// FIX 5 regression: a unit with a genuinely negative current residual must
// persist a quote even when the flooring flag is disabled. The persisted
// effective_residual_value_won is floored to 0 (DB CHECK >= 0) with
// residual_was_floored=true, while current_residual_value_won keeps the real
// negative value for audit.
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn negative_residual_quote_persists_with_flooring_disabled(pool: PgPool) {
    console_platform_request_context::scope_org(console_kernel_core::OrgId::knl(), async move {
        let seeded = seed_financial_context(&pool).await;
        let store = PgFinancialStore::new(pool.clone());
        let occurred_at = datetime!(2026-06-12 12:00 UTC);

        let quote = store
            .create_rental_quote(CreateRentalQuoteCommand {
                actor: seeded.receptionist,
                branch_id: seeded.branch_id,
                equipment_id: seeded.negative_residual_equipment,
                config: financial_config_no_floor(),
                trace: TraceContext::generate(),
                occurred_at,
            })
            .await
            .unwrap();

        // The persisted effective residual is floored to 0 and flagged, even
        // though the flooring config flag is false.
        assert!(quote.residual_was_floored);
        assert_eq!(quote.effective_residual_value.amount(), 0);
        assert!(quote.monthly_total.amount() > 0);

        let row: (i64, i64, bool, bool) = sqlx::query_as(
            r#"
            SELECT current_residual_value_won, effective_residual_value_won,
                   residual_was_floored, floor_negative_quote_residual
            FROM financial_rental_quotes
            WHERE id = $1
            "#,
        )
        .bind(*quote.id.as_uuid())
        .fetch_one(&pool)
        .await
        .unwrap();
        // Real negative residual preserved for audit (no >=0 check on this column).
        assert_eq!(row.0, -1_250_000);
        // Persisted effective residual floored to 0 (DB CHECK >= 0).
        assert_eq!(row.1, 0);
        assert!(row.2, "residual_was_floored must be true");
        assert!(!row.3, "flooring config flag was disabled");
    })
    .await;
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn financial_inputs_reject_cross_scope_evidence_and_work_orders(pool: PgPool) {
    console_platform_request_context::scope_org(console_kernel_core::OrgId::knl(), async move {
        let seeded = seed_financial_context(&pool).await;
        let other_branch = seed_branch(&pool).await;
        let other_user = seed_user(&pool, "Other Mechanic", "MECHANIC", other_branch).await;
        let other_equipment = seed_equipment(
            &pool,
            other_branch,
            "DEF12-1302",
            "1302",
            8_000_000,
            5_000_000,
        )
        .await;
        let other_work_order =
            seed_work_order(&pool, other_branch, other_user, other_equipment).await;
        let other_statement = seed_statement_evidence(&pool, other_work_order, other_user).await;
        let pending_statement = seed_evidence(
            &pool,
            seeded.work_order_id,
            seeded.mechanic,
            "REQUEST",
            "PENDING",
        )
        .await;
        let wrong_equipment_work_order = seed_work_order(
            &pool,
            seeded.branch_id,
            seeded.receptionist,
            seeded.negative_residual_equipment,
        )
        .await;
        let store = PgFinancialStore::new(pool.clone());
        let occurred_at = datetime!(2026-06-12 12:00 UTC);

        let cross_scope = store
            .create_purchase_request(CreatePurchaseRequestCommand {
                actor: seeded.mechanic,
                branch_id: seeded.branch_id,
                equipment_id: Some(seeded.normal_equipment),
                work_order_id: Some(seeded.work_order_id),
                statement_evidence_id: Some(other_statement),
                purchase_type: PurchaseType::LegacyManual,
                vendor_name: "Wrong Scope Vendor".to_owned(),
                amount_won: Some(500_000),
                lines: vec![manual_purchase_line("wrong evidence", 500_000)],
                quote_attachment_ids: Vec::new(),
                memo: "wrong evidence".to_owned(),
                config: financial_config(),
                trace: TraceContext::generate(),
                occurred_at,
            })
            .await
            .unwrap_err();
        assert!(cross_scope.to_string().contains("outside"));

        // The WORM-replica precondition is DEFERRED from create to submit: a
        // legitimate purchase request CREATES against still-replicating (PENDING)
        // REQUEST evidence — the async replica state must not bar create — and
        // only the SUBMIT into the approval pipeline waits on durable WORM
        // verification. (Before #19.18 the create itself 4xx'd with "verified
        // REQUEST", and the web catch{} swallowed the reason.)
        let pending_purchase = store
            .create_purchase_request(CreatePurchaseRequestCommand {
                actor: seeded.mechanic,
                branch_id: seeded.branch_id,
                equipment_id: Some(seeded.normal_equipment),
                work_order_id: Some(seeded.work_order_id),
                statement_evidence_id: Some(pending_statement),
                purchase_type: PurchaseType::LegacyManual,
                vendor_name: "Pending Vendor".to_owned(),
                amount_won: Some(500_000),
                lines: vec![manual_purchase_line("pending evidence", 500_000)],
                quote_attachment_ids: Vec::new(),
                memo: "pending evidence".to_owned(),
                config: financial_config(),
                trace: TraceContext::generate(),
                occurred_at,
            })
            .await
            .expect("create must succeed against PENDING (still-replicating) REQUEST evidence");
        assert_eq!(pending_purchase.status, PurchaseStatus::StatementAttached);

        // Submitting it into the approval pipeline is gated on durable WORM
        // verification, so it is refused with the deferred, surfaced reason while
        // the replica is still PENDING.
        let worm_pending = store
            .submit_purchase_request(PurchaseSubmitCommand {
                actor: seeded.receptionist,
                purchase_request_id: pending_purchase.id,
                trace: TraceContext::generate(),
                occurred_at,
            })
            .await
            .unwrap_err();
        assert!(worm_pending.to_string().contains("WORM-verified"));

        let wrong_work_order = store
            .append_cost_ledger_entry(AppendCostLedgerEntryCommand {
                actor: seeded.admin,
                branch_id: seeded.branch_id,
                equipment_id: seeded.normal_equipment,
                work_order_id: Some(wrong_equipment_work_order),
                source: CostLedgerSource::ManualAdmin,
                amount_won: 750_000,
                memo: "wrong work order".to_owned(),
                config: financial_config(),
                trace: TraceContext::generate(),
                occurred_at,
            })
            .await
            .unwrap_err();
        assert!(wrong_work_order.to_string().contains("work order"));
    })
    .await;
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn concurrent_cost_ledger_recomputes_from_serialized_equipment_lock(pool: PgPool) {
    console_platform_request_context::scope_org(console_kernel_core::OrgId::knl(), async move {
        let seeded = seed_financial_context(&pool).await;
        let store = PgFinancialStore::new(pool.clone());
        let occurred_at = datetime!(2026-06-12 12:00 UTC);
        let mut tx = pool.begin().await.unwrap();
        sqlx::query("SELECT id FROM registry_equipment WHERE id = $1 FOR UPDATE")
            .bind(*seeded.normal_equipment.as_uuid())
            .execute(tx.as_mut())
            .await
            .unwrap();
        let admin = seeded.admin;
        let branch_id = seeded.branch_id;
        let equipment_id = seeded.normal_equipment;
        let work_order_id = seeded.work_order_id;

        let first_store = store.clone();
        let first = tokio::spawn({
            let config = financial_config();
            async move {
                console_platform_request_context::scope_org(
                    console_kernel_core::OrgId::knl(),
                    async move {
                        first_store
                            .append_cost_ledger_entry(AppendCostLedgerEntryCommand {
                                actor: admin,
                                branch_id,
                                equipment_id,
                                work_order_id: Some(work_order_id),
                                source: CostLedgerSource::ManualAdmin,
                                amount_won: 1_000_000,
                                memo: "first concurrent cost".to_owned(),
                                config,
                                trace: TraceContext::generate(),
                                occurred_at,
                            })
                            .await
                    },
                )
                .await
            }
        });
        let second_store = store.clone();
        let second = tokio::spawn({
            let config = financial_config();
            async move {
                console_platform_request_context::scope_org(
                    console_kernel_core::OrgId::knl(),
                    async move {
                        second_store
                            .append_cost_ledger_entry(AppendCostLedgerEntryCommand {
                                actor: admin,
                                branch_id,
                                equipment_id,
                                work_order_id: Some(work_order_id),
                                source: CostLedgerSource::ManualAdmin,
                                amount_won: 2_000_000,
                                memo: "second concurrent cost".to_owned(),
                                config,
                                trace: TraceContext::generate(),
                                occurred_at,
                            })
                            .await
                    },
                )
                .await
            }
        });

        tokio::time::sleep(Duration::from_millis(100)).await;
        tx.rollback().await.unwrap();
        first.await.unwrap().unwrap();
        second.await.unwrap().unwrap();

        let residual: i64 =
            sqlx::query_scalar("SELECT residual_value FROM registry_equipment WHERE id = $1")
                .bind(*seeded.normal_equipment.as_uuid())
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(residual, 3_600_000);
        let entries = store
            .cost_ledger_for_equipment(seeded.normal_equipment)
            .await
            .unwrap();
        assert_eq!(entries.len(), 2);
        assert!(
            entries
                .iter()
                .any(|entry| entry.residual_after_won == 3_600_000)
        );
    })
    .await;
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn purchase_execute_rolls_back_if_ledger_update_fails(pool: PgPool) {
    console_platform_request_context::scope_org(console_kernel_core::OrgId::knl(), async move {
        let seeded = seed_financial_context(&pool).await;
        let store = PgFinancialStore::new(pool.clone());
        let occurred_at = datetime!(2026-06-12 12:00 UTC);
        let purchase = store
            .create_purchase_request(CreatePurchaseRequestCommand {
                actor: seeded.mechanic,
                branch_id: seeded.branch_id,
                equipment_id: Some(seeded.normal_equipment),
                work_order_id: Some(seeded.work_order_id),
                statement_evidence_id: Some(seeded.statement_evidence_id),
                purchase_type: PurchaseType::LegacyManual,
                vendor_name: "Rollback Parts".to_owned(),
                amount_won: Some(1_000_000),
                lines: vec![manual_purchase_line("rollback fixture", 1_000_000)],
                quote_attachment_ids: Vec::new(),
                memo: "rollback fixture".to_owned(),
                config: financial_config(),
                trace: TraceContext::generate(),
                occurred_at,
            })
            .await
            .unwrap();
        store
            .submit_purchase_request(PurchaseSubmitCommand {
                actor: seeded.receptionist,
                purchase_request_id: purchase.id,
                trace: TraceContext::generate(),
                occurred_at,
            })
            .await
            .unwrap();
        store
            .approve_purchase_admin(PurchaseApprovalCommand {
                actor: seeded.admin,
                purchase_request_id: purchase.id,
                trace: TraceContext::generate(),
                occurred_at,
            })
            .await
            .unwrap();
        // Below the executive threshold, preparation routes the independently
        // approved request straight to READY_TO_EXECUTE. The admin prepares it here.
        let ready = store
            .prepare_expenditure(PrepareExpenditureCommand {
                actor: seeded.admin,
                purchase_request_id: purchase.id,
                expenditure_no: "EXP-ROLLBACK-001".to_owned(),
                trace: TraceContext::generate(),
                occurred_at,
            })
            .await
            .unwrap();
        assert_eq!(ready.status, PurchaseStatus::ReadyToExecute);

        sqlx::query("UPDATE registry_equipment SET vehicle_value = NULL WHERE id = $1")
            .bind(*seeded.normal_equipment.as_uuid())
            .execute(&pool)
            .await
            .unwrap();
        let err = store
            .execute_purchase(ExecutePurchaseCommand {
                actor: seeded.receptionist,
                purchase_request_id: purchase.id,
                trace: TraceContext::generate(),
                occurred_at,
            })
            .await
            .unwrap_err();
        assert!(err.to_string().contains("vehicle value is required"));

        let status: String =
            sqlx::query_scalar("SELECT status FROM financial_purchase_requests WHERE id = $1")
                .bind(*purchase.id.as_uuid())
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(status, "READY_TO_EXECUTE");
        let ledger_rows: i64 = sqlx::query_scalar(
            "SELECT COUNT(*)::BIGINT FROM equipment_cost_ledger WHERE purchase_request_id = $1",
        )
        .bind(*purchase.id.as_uuid())
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(ledger_rows, 0);
        let execute_audits: i64 =
            sqlx::query_scalar("SELECT COUNT(*)::BIGINT FROM audit_events WHERE action = 'purchase.execute' AND target_id = $1")
                .bind(purchase.id.to_string())
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(execute_audits, 0);
    })
    .await;
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn non_knl_purchase_without_equipment_executes_to_expense_ledger(pool: PgPool) {
    let non_knl = OrgId::from_uuid(uuid::Uuid::from_u128(
        0x3333_3333_3333_3333_3333_3333_3333_3333,
    ));
    seed_org(&pool, non_knl, "other-co", "Other Company").await;
    let branch = seed_branch_for_org(&pool, non_knl).await;
    let requester = seed_user_for_org(&pool, non_knl, "Other Requester", "ADMIN", branch).await;
    let submitter =
        seed_user_for_org(&pool, non_knl, "Other Submitter", "RECEPTIONIST", branch).await;
    let approver = seed_user_for_org(&pool, non_knl, "Other Approver", "ADMIN", branch).await;
    let executor =
        seed_user_for_org(&pool, non_knl, "Other Executor", "RECEPTIONIST", branch).await;
    let occurred_at = datetime!(2026-06-30 09:00 UTC);
    let store = PgFinancialStore::new(pool.clone());

    let knl_seeded = seed_financial_context(&pool).await;
    let knl_rejection = console_platform_request_context::scope_org(OrgId::knl(), async {
        store
            .create_purchase_request(CreatePurchaseRequestCommand {
                actor: knl_seeded.admin,
                branch_id: knl_seeded.branch_id,
                equipment_id: None,
                work_order_id: None,
                statement_evidence_id: None,
                purchase_type: PurchaseType::OneOff,
                vendor_name: "장비없는 요청".to_owned(),
                amount_won: None,
                lines: vec![PurchaseRequestLineInput {
                    item: "사무용 소모품".to_owned(),
                    quantity: 1,
                    unit_supply_price_won: 50_000,
                    vat_won: None,
                }],
                quote_attachment_ids: Vec::new(),
                memo: "KNL 정비사업부는 호기 필요".to_owned(),
                config: financial_config(),
                trace: TraceContext::generate(),
                occurred_at,
            })
            .await
    })
    .await
    .unwrap_err();
    assert!(knl_rejection.to_string().contains("equipment is required"));

    console_platform_request_context::scope_org(non_knl, async {
        let created = store
            .create_purchase_request(CreatePurchaseRequestCommand {
                actor: requester,
                branch_id: branch,
                equipment_id: None,
                work_order_id: None,
                statement_evidence_id: None,
                purchase_type: PurchaseType::Other,
                vendor_name: "비장비 공급사".to_owned(),
                amount_won: None,
                lines: vec![
                    PurchaseRequestLineInput {
                        item: "사무실 소모품".to_owned(),
                        quantity: 2,
                        unit_supply_price_won: 100_000,
                        vat_won: None,
                    },
                    PurchaseRequestLineInput {
                        item: "배송비".to_owned(),
                        quantity: 1,
                        unit_supply_price_won: 20_000,
                        vat_won: Some(1_000),
                    },
                ],
                quote_attachment_ids: Vec::new(),
                memo: "장비와 무관한 운영 구매".to_owned(),
                config: financial_config(),
                trace: TraceContext::generate(),
                occurred_at,
            })
            .await
            .unwrap();

        assert_eq!(created.amount_won, 241_000);
        assert_eq!(created.equipment_id, None);
        assert_eq!(created.purchase_type, PurchaseType::Other);
        assert_eq!(created.requester.display_name, "Other Requester");
        assert_eq!(created.lines.len(), 2);
        assert_eq!(created.lines[0].vat_won, 20_000);
        assert!(!created.lines[0].vat_overridden);
        assert_eq!(created.lines[1].vat_won, 1_000);
        assert!(created.lines[1].vat_overridden);

        store
            .submit_purchase_request(PurchaseSubmitCommand {
                actor: submitter,
                purchase_request_id: created.id,
                trace: TraceContext::generate(),
                occurred_at,
            })
            .await
            .unwrap();
        store
            .approve_purchase_admin(PurchaseApprovalCommand {
                actor: approver,
                purchase_request_id: created.id,
                trace: TraceContext::generate(),
                occurred_at,
            })
            .await
            .unwrap();
        let ready = store
            .prepare_expenditure(PrepareExpenditureCommand {
                actor: approver,
                purchase_request_id: created.id,
                expenditure_no: "AP-20260630-001".to_owned(),
                trace: TraceContext::generate(),
                occurred_at,
            })
            .await
            .unwrap();
        assert_eq!(ready.status, PurchaseStatus::ReadyToExecute);

        let executed = store
            .execute_purchase(ExecutePurchaseCommand {
                actor: executor,
                purchase_request_id: created.id,
                trace: TraceContext::generate(),
                occurred_at,
            })
            .await
            .unwrap();
        assert_eq!(executed.status, PurchaseStatus::Executed);

        let expense_rows: i64 = sqlx::query_scalar(
            "SELECT COUNT(*)::BIGINT FROM financial_expense_ledger WHERE purchase_request_id = $1",
        )
        .bind(*created.id.as_uuid())
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(expense_rows, 1);

        let equipment_rows: i64 = sqlx::query_scalar(
            "SELECT COUNT(*)::BIGINT FROM equipment_cost_ledger WHERE purchase_request_id = $1",
        )
        .bind(*created.id.as_uuid())
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(equipment_rows, 0);
    })
    .await;
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn purchase_lines_drive_server_total_requester_and_quote_anomaly_gate(pool: PgPool) {
    console_platform_request_context::scope_org(OrgId::knl(), async move {
        let seeded = seed_financial_context(&pool).await;
        let store = PgFinancialStore::new(pool.clone());
        let occurred_at = datetime!(2026-06-30 10:00 UTC);

        seed_regular_purchase_price(&pool, seeded.branch_id, "정기부품사", "유압 필터", 100_000)
            .await;

        let created = store
            .create_purchase_request(CreatePurchaseRequestCommand {
                actor: seeded.receptionist,
                branch_id: seeded.branch_id,
                equipment_id: Some(seeded.normal_equipment),
                work_order_id: Some(seeded.work_order_id),
                statement_evidence_id: Some(seeded.statement_evidence_id),
                purchase_type: PurchaseType::Regular,
                vendor_name: "정기부품사".to_owned(),
                amount_won: None,
                lines: vec![PurchaseRequestLineInput {
                    item: "유압 필터".to_owned(),
                    quantity: 2,
                    unit_supply_price_won: 100_001,
                    vat_won: None,
                }],
                quote_attachment_ids: Vec::new(),
                memo: "단가 이상 감지 검증".to_owned(),
                config: financial_config(),
                trace: TraceContext::generate(),
                occurred_at,
            })
            .await
            .unwrap();

        assert_eq!(created.amount_won, 220_002);
        assert_eq!(created.requester.display_name, "Financial Receptionist");
        assert!(created.policy.price_anomaly);
        assert!(created.policy.quote_update_required);
        assert!(
            created
                .policy
                .messages
                .iter()
                .any(|message| message.contains("견적"))
        );

        let blocked = store
            .submit_purchase_request(PurchaseSubmitCommand {
                actor: seeded.admin,
                purchase_request_id: created.id,
                trace: TraceContext::generate(),
                occurred_at,
            })
            .await
            .unwrap_err();
        assert!(blocked.to_string().contains("quote update required"));
    })
    .await;
}

struct SeededFinancialContext {
    branch_id: BranchId,
    receptionist: UserId,
    mechanic: UserId,
    admin: UserId,
    executive: UserId,
    normal_equipment: EquipmentId,
    negative_residual_equipment: EquipmentId,
    work_order_id: WorkOrderId,
    statement_evidence_id: EvidenceId,
}

async fn seed_financial_context(pool: &PgPool) -> SeededFinancialContext {
    let branch_id = seed_branch(pool).await;
    let receptionist = seed_user(pool, "Financial Receptionist", "RECEPTIONIST", branch_id).await;
    let mechanic = seed_user(pool, "Financial Mechanic", "MECHANIC", branch_id).await;
    let admin = seed_user(pool, "Financial Admin", "ADMIN", branch_id).await;
    let executive = seed_user(pool, "Financial Executive", "EXECUTIVE", branch_id).await;
    let normal_equipment =
        seed_equipment(pool, branch_id, "ABC12-1300", "1300", 12_000_000, 9_000_000).await;
    let negative_residual_equipment = seed_equipment(
        pool,
        branch_id,
        "ABC12-1301",
        "1301",
        20_000_000,
        -1_250_000,
    )
    .await;
    let work_order_id = seed_work_order(pool, branch_id, receptionist, normal_equipment).await;
    let statement_evidence_id = seed_statement_evidence(pool, work_order_id, mechanic).await;

    SeededFinancialContext {
        branch_id,
        receptionist,
        mechanic,
        admin,
        executive,
        normal_equipment,
        negative_residual_equipment,
        work_order_id,
        statement_evidence_id,
    }
}

fn financial_config() -> FinancialConfigSnapshot {
    FinancialConfigSnapshot {
        depreciation_method: DepreciationMethod::StraightLine,
        useful_life_months: 60,
        residual_rate_bps: 1_000,
        declining_balance_rate_bps: 2_000,
        management_fee_rate_bps: 1_000,
        profit_rate_bps: 500,
        floor_negative_quote_residual: true,
        executive_approval_threshold_won: 2_000_000,
    }
}

fn financial_config_no_floor() -> FinancialConfigSnapshot {
    FinancialConfigSnapshot {
        floor_negative_quote_residual: false,
        ..financial_config()
    }
}

fn manual_purchase_line(item: &str, amount_won: i64) -> PurchaseRequestLineInput {
    PurchaseRequestLineInput {
        item: item.to_owned(),
        quantity: 1,
        unit_supply_price_won: amount_won,
        vat_won: Some(0),
    }
}

async fn seed_org(pool: &PgPool, org: OrgId, slug: &str, name: &str) {
    sqlx::query(
        "INSERT INTO organizations (id, slug, name) VALUES ($1, $2, $3) ON CONFLICT (id) DO NOTHING",
    )
    .bind(*org.as_uuid())
    .bind(slug)
    .bind(name)
    .execute(pool)
    .await
    .unwrap();
}

async fn seed_branch_for_org(pool: &PgPool, org: OrgId) -> BranchId {
    let region_id: uuid::Uuid =
        sqlx::query_scalar("INSERT INTO regions (name, org_id) VALUES ($1, $2) RETURNING id")
            .bind(format!("Financial Region {}", uuid::Uuid::new_v4()))
            .bind(*org.as_uuid())
            .fetch_one(pool)
            .await
            .unwrap();
    let branch_id: uuid::Uuid = sqlx::query_scalar(
        "INSERT INTO branches (region_id, name, org_id) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(region_id)
    .bind(format!("Financial Branch {}", uuid::Uuid::new_v4()))
    .bind(*org.as_uuid())
    .fetch_one(pool)
    .await
    .unwrap();
    BranchId::from_uuid(branch_id)
}

async fn seed_user_for_org(
    pool: &PgPool,
    org: OrgId,
    name: &str,
    role: &str,
    branch_id: BranchId,
) -> UserId {
    let user_id = UserId::new();
    sqlx::query("INSERT INTO users (id, display_name, roles, org_id) VALUES ($1, $2, $3, $4)")
        .bind(*user_id.as_uuid())
        .bind(name)
        .bind(Vec::from([role]))
        .bind(*org.as_uuid())
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO user_branches (user_id, branch_id, org_id) VALUES ($1, $2, $3)")
        .bind(*user_id.as_uuid())
        .bind(*branch_id.as_uuid())
        .bind(*org.as_uuid())
        .execute(pool)
        .await
        .unwrap();
    user_id
}

async fn seed_regular_purchase_price(
    pool: &PgPool,
    branch_id: BranchId,
    vendor_name: &str,
    item: &str,
    unit_supply_price_won: i64,
) {
    sqlx::query(
        r#"
        INSERT INTO financial_regular_purchase_prices (
            branch_id, vendor_name_norm, item_norm, last_unit_supply_price_won,
            updated_at, org_id
        )
        VALUES ($1, $2, $3, $4, now(), $5)
        "#,
    )
    .bind(*branch_id.as_uuid())
    .bind(
        vendor_name
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase(),
    )
    .bind(
        item.split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase(),
    )
    .bind(unit_supply_price_won)
    .bind(*OrgId::knl().as_uuid())
    .execute(pool)
    .await
    .unwrap();
}

async fn seed_branch(pool: &PgPool) -> BranchId {
    let region_id: uuid::Uuid =
        sqlx::query_scalar("INSERT INTO regions (name, org_id) VALUES ($1, $2) RETURNING id")
            .bind(format!("Financial Region {}", uuid::Uuid::new_v4()))
            .bind(*OrgId::knl().as_uuid())
            .fetch_one(pool)
            .await
            .unwrap();
    let branch_id: uuid::Uuid = sqlx::query_scalar(
        "INSERT INTO branches (region_id, name, org_id) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(region_id)
    .bind(format!("Financial Branch {}", uuid::Uuid::new_v4()))
    .bind(*OrgId::knl().as_uuid())
    .fetch_one(pool)
    .await
    .unwrap();
    BranchId::from_uuid(branch_id)
}

async fn seed_user(pool: &PgPool, name: &str, role: &str, branch_id: BranchId) -> UserId {
    let user_id = UserId::new();
    sqlx::query("INSERT INTO users (id, display_name, roles, org_id) VALUES ($1, $2, $3, $4)")
        .bind(*user_id.as_uuid())
        .bind(name)
        .bind(Vec::from([role]))
        .bind(*OrgId::knl().as_uuid())
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO user_branches (user_id, branch_id, org_id) VALUES ($1, $2, $3)")
        .bind(*user_id.as_uuid())
        .bind(*branch_id.as_uuid())
        .bind(*OrgId::knl().as_uuid())
        .execute(pool)
        .await
        .unwrap();
    user_id
}

async fn seed_equipment(
    pool: &PgPool,
    branch_id: BranchId,
    equipment_no: &str,
    management_no: &str,
    vehicle_value: i64,
    residual_value: i64,
) -> EquipmentId {
    let customer_id: uuid::Uuid = sqlx::query_scalar(
        "INSERT INTO registry_customers (branch_id, name, org_id) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(*branch_id.as_uuid())
    .bind(format!("Customer {management_no}"))
    .bind(*OrgId::knl().as_uuid())
    .fetch_one(pool)
    .await
    .unwrap();
    let site_id: uuid::Uuid = sqlx::query_scalar(
        "INSERT INTO registry_sites (branch_id, customer_id, name, org_id) VALUES ($1, $2, $3, $4) RETURNING id",
    )
    .bind(*branch_id.as_uuid())
    .bind(customer_id)
    .bind(format!("Site {management_no}"))
    .bind(*OrgId::knl().as_uuid())
    .fetch_one(pool)
    .await
    .unwrap();
    let equipment_id: uuid::Uuid = sqlx::query_scalar(
        r#"
        INSERT INTO registry_equipment (
            branch_id, customer_id, site_id, equipment_no, management_no,
            manufacturer_code, kind_code, power_code, status,
            specification, ton_text, vehicle_value, residual_value,
            asset_registered_on, source_sheet, source_row, org_id
        )
        VALUES ($1, $2, $3, $4, $5, 'A', 'B', 'C', '임대',
                '좌식', '2.5T', $6, $7, DATE '2023-12-12', 'financial-test', 1, $8)
        RETURNING id
        "#,
    )
    .bind(*branch_id.as_uuid())
    .bind(customer_id)
    .bind(site_id)
    .bind(equipment_no)
    .bind(management_no)
    .bind(vehicle_value)
    .bind(residual_value)
    .bind(*OrgId::knl().as_uuid())
    .fetch_one(pool)
    .await
    .unwrap();
    EquipmentId::from_uuid(equipment_id)
}

async fn seed_work_order(
    pool: &PgPool,
    branch_id: BranchId,
    requested_by: UserId,
    equipment_id: EquipmentId,
) -> WorkOrderId {
    let row: (uuid::Uuid, uuid::Uuid) =
        sqlx::query_as("SELECT customer_id, site_id FROM registry_equipment WHERE id = $1")
            .bind(*equipment_id.as_uuid())
            .fetch_one(pool)
            .await
            .unwrap();
    let work_order_id = WorkOrderId::new();
    sqlx::query(
        r#"
        INSERT INTO work_orders (
            id, request_no, branch_id, equipment_id, customer_id, site_id,
            requested_by, status, symptom, org_id
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, 'RECEIVED', 'financial fixture', $8)
        "#,
    )
    .bind(*work_order_id.as_uuid())
    .bind(format!(
        "20260612-{:03}",
        REQUEST_NO_SEQUENCE.fetch_add(1, Ordering::SeqCst)
    ))
    .bind(*branch_id.as_uuid())
    .bind(*equipment_id.as_uuid())
    .bind(row.0)
    .bind(row.1)
    .bind(*requested_by.as_uuid())
    .bind(*OrgId::knl().as_uuid())
    .execute(pool)
    .await
    .unwrap();
    work_order_id
}

async fn seed_statement_evidence(
    pool: &PgPool,
    work_order_id: WorkOrderId,
    uploaded_by: UserId,
) -> EvidenceId {
    seed_evidence(pool, work_order_id, uploaded_by, "REQUEST", "VERIFIED").await
}

async fn seed_evidence(
    pool: &PgPool,
    work_order_id: WorkOrderId,
    uploaded_by: UserId,
    stage: &str,
    worm_replica_status: &str,
) -> EvidenceId {
    let evidence_id = EvidenceId::new();
    sqlx::query(
        r#"
        INSERT INTO evidence_media (
            id, work_order_id, stage, s3_key, content_type, size_bytes,
            uploaded_by, worm_replica_status, retry_count, org_id
        )
        VALUES ($1, $2, $3, $4, 'application/pdf', 2048, $5, $6, 0, $7)
        "#,
    )
    .bind(*evidence_id.as_uuid())
    .bind(*work_order_id.as_uuid())
    .bind(stage)
    .bind(format!(
        "work-orders/{work_order_id}/REQUEST/{evidence_id}.pdf"
    ))
    .bind(*uploaded_by.as_uuid())
    .bind(worm_replica_status)
    .bind(*OrgId::knl().as_uuid())
    .execute(pool)
    .await
    .unwrap();
    evidence_id
}

/// Seed a user with the given role AND `is_org_lead = true` (대표/CEO).
async fn seed_org_lead_user(pool: &PgPool, role: &str, branch_id: BranchId) -> UserId {
    let user_id = UserId::new();
    sqlx::query(
        "INSERT INTO users (id, display_name, roles, org_id, is_org_lead)
         VALUES ($1, $2, $3, $4, true)",
    )
    .bind(*user_id.as_uuid())
    .bind("Org Lead")
    .bind(Vec::from([role]))
    .bind(*OrgId::knl().as_uuid())
    .execute(pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO user_branches (user_id, branch_id, org_id) VALUES ($1, $2, $3)")
        .bind(*user_id.as_uuid())
        .bind(*branch_id.as_uuid())
        .bind(*OrgId::knl().as_uuid())
        .execute(pool)
        .await
        .unwrap();
    user_id
}

// ===========================================================================
// Self-approval guard tests (Slice A, task #34)
// ===========================================================================

/// (a) A normal ADMIN who submitted/requested a 기안 must NOT be able to
///     approve it. The guard must return a 422-equivalent validation error:
///     "본인이 상신/요청한 건은 결재할 수 없습니다".
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn self_approval_blocked_for_normal_admin(pool: PgPool) {
    console_platform_request_context::scope_org(OrgId::knl(), async move {
        let seeded = seed_financial_context(&pool).await;
        let store = PgFinancialStore::new(pool.clone());
        let occurred_at = time::macros::datetime!(2026-06-23 09:00 UTC);
        let config = financial_config();

        // Create the purchase as the mechanic (they are requested_by).
        let purchase = store
            .create_purchase_request(CreatePurchaseRequestCommand {
                actor: seeded.admin, // admin creates → admin is requested_by
                branch_id: seeded.branch_id,
                equipment_id: Some(seeded.normal_equipment),
                work_order_id: None,
                statement_evidence_id: Some(seeded.statement_evidence_id),
                purchase_type: PurchaseType::LegacyManual,
                vendor_name: "Self Test Vendor".to_owned(),
                amount_won: Some(500_000),
                lines: vec![manual_purchase_line("Self-approval test", 500_000)],
                quote_attachment_ids: Vec::new(),
                memo: "Self-approval test".to_owned(),
                config: config.clone(),
                trace: TraceContext::generate(),
                occurred_at,
            })
            .await
            .unwrap();

        // Submit (moves to REQUEST_SUBMITTED).
        store
            .submit_purchase_request(PurchaseSubmitCommand {
                actor: seeded.receptionist,
                purchase_request_id: purchase.id,
                trace: TraceContext::generate(),
                occurred_at,
            })
            .await
            .unwrap();

        // The SAME admin who created the request tries to approve it.
        // Must be rejected with a validation error.
        let result = store
            .approve_purchase_admin(PurchaseApprovalCommand {
                actor: seeded.admin,
                purchase_request_id: purchase.id,
                trace: TraceContext::generate(),
                occurred_at,
            })
            .await;

        let err = result.expect_err("self-approval must be blocked");
        // KernelError::Validation maps to PgFinancialError::Domain.
        use console_financial_adapter_postgres::PgFinancialError;
        use console_kernel_core::ErrorKind;
        match err {
            PgFinancialError::Domain(e) => {
                assert_eq!(e.kind, ErrorKind::Validation);
                assert!(
                    e.message
                        .contains("본인이 상신/요청한 건은 결재할 수 없습니다"),
                    "expected Korean self-approval error, got: {}",
                    e.message
                );
            }
            other => panic!("expected Domain(Validation), got: {other:?}"),
        }
    })
    .await;
}

/// Both title exceptions are forbidden at admin and executive approval,
/// whether the maker requested or submitted the purchase.
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn title_self_approval_is_denied_at_both_steps(pool: PgPool) {
    console_platform_request_context::scope_org(OrgId::knl(), async move {
        let seeded = seed_financial_context(&pool).await;
        let store = PgFinancialStore::new(approval_runtime_pool(&pool).await);
        for lead in [true, false] {
            let actor = if lead {
                seed_org_lead_user(&pool, "ADMIN", seeded.branch_id).await
            } else {
                seed_user(&pool, "Root", "SUPER_ADMIN", seeded.branch_id).await
            };
            for executive in [false, true] {
                for maker_is_requester in [true, false] {
                    if lead {
                        sqlx::query("UPDATE users SET roles=ARRAY['ADMIN'] WHERE id=$1")
                            .bind(*actor.as_uuid())
                            .execute(&pool)
                            .await
                            .unwrap();
                    }
                    let requester = if maker_is_requester {
                        actor
                    } else {
                        seeded.mechanic
                    };
                    let submitter = if maker_is_requester {
                        seeded.receptionist
                    } else {
                        actor
                    };
                    let id =
                        submitted_purchase(&store, &seeded, requester, submitter, 3_000_000).await;
                    if executive {
                        store
                            .approve_purchase_admin(approval_command(id, seeded.admin))
                            .await
                            .unwrap();
                        store
                            .prepare_expenditure(preparation_command(id, seeded.receptionist))
                            .await
                            .unwrap();
                        if lead {
                            sqlx::query("UPDATE users SET roles=ARRAY['EXECUTIVE'] WHERE id=$1")
                                .bind(*actor.as_uuid())
                                .execute(&pool)
                                .await
                                .unwrap();
                        }
                    }
                    let before = purchase_evidence(&pool, id).await;
                    let result = if executive {
                        store
                            .approve_purchase_executive(approval_command(id, actor))
                            .await
                    } else {
                        store
                            .approve_purchase_admin(approval_command(id, actor))
                            .await
                    };
                    let err = result.expect_err("titles never permit self-approval");
                    match err {
                        console_financial_adapter_postgres::PgFinancialError::Domain(e) => {
                            assert_eq!(e.kind, console_kernel_core::ErrorKind::Validation);
                            assert!(
                                e.message
                                    .contains("본인이 상신/요청한 건은 결재할 수 없습니다")
                            );
                        }
                        other => panic!("unexpected {other:?}"),
                    }
                    assert_eq!(purchase_evidence(&pool, id).await, before);
                    let accepted = if executive {
                        store
                            .approve_purchase_executive(approval_command(id, seeded.executive))
                            .await
                            .unwrap()
                    } else {
                        store
                            .approve_purchase_admin(approval_command(id, seeded.admin))
                            .await
                            .unwrap()
                    };
                    assert_eq!(
                        accepted.status,
                        if executive {
                            PurchaseStatus::ReadyToExecute
                        } else {
                            PurchaseStatus::AdminApproved
                        }
                    );
                }
            }
        }
    })
    .await;
}

async fn approval_runtime_pool(pool: &PgPool) -> PgPool {
    sqlx::postgres::PgPoolOptions::new()
        .max_connections(4)
        .after_connect(|conn, _| {
            Box::pin(async move {
                sqlx::query("SET ROLE console_rt").execute(conn).await?;
                Ok(())
            })
        })
        .connect_with(pool.connect_options().as_ref().clone())
        .await
        .unwrap()
}

fn approval_command(
    id: console_kernel_core::PurchaseRequestId,
    actor: UserId,
) -> PurchaseApprovalCommand {
    PurchaseApprovalCommand {
        actor,
        purchase_request_id: id,
        trace: TraceContext::generate(),
        occurred_at: datetime!(2026-06-23 10:00 UTC),
    }
}
fn preparation_command(
    id: console_kernel_core::PurchaseRequestId,
    actor: UserId,
) -> PrepareExpenditureCommand {
    PrepareExpenditureCommand {
        actor,
        purchase_request_id: id,
        expenditure_no: format!("EXP-{id}"),
        trace: TraceContext::generate(),
        occurred_at: datetime!(2026-06-23 10:00 UTC),
    }
}
async fn submitted_purchase(
    store: &PgFinancialStore,
    seeded: &SeededFinancialContext,
    requester: UserId,
    submitter: UserId,
    amount: i64,
) -> console_kernel_core::PurchaseRequestId {
    let occurred_at = datetime!(2026-06-23 10:00 UTC);
    let p = store
        .create_purchase_request(CreatePurchaseRequestCommand {
            actor: requester,
            branch_id: seeded.branch_id,
            equipment_id: Some(seeded.normal_equipment),
            work_order_id: None,
            statement_evidence_id: Some(seeded.statement_evidence_id),
            purchase_type: PurchaseType::LegacyManual,
            vendor_name: "Approval test".to_owned(),
            amount_won: Some(amount),
            lines: vec![manual_purchase_line("Part", amount)],
            quote_attachment_ids: Vec::new(),
            memo: "Review purchase".to_owned(),
            config: financial_config(),
            trace: TraceContext::generate(),
            occurred_at,
        })
        .await
        .unwrap();
    store
        .submit_purchase_request(PurchaseSubmitCommand {
            actor: submitter,
            purchase_request_id: p.id,
            trace: TraceContext::generate(),
            occurred_at,
        })
        .await
        .unwrap();
    p.id
}
async fn purchase_evidence(
    pool: &PgPool,
    id: console_kernel_core::PurchaseRequestId,
) -> serde_json::Value {
    sqlx::query_scalar("SELECT jsonb_build_array(to_jsonb(p),
        (SELECT jsonb_agg(to_jsonb(h) ORDER BY id) FROM financial_purchase_history h WHERE purchase_request_id=p.id),
        (SELECT jsonb_agg(to_jsonb(f) ORDER BY id) FROM governance_findings f),
        (SELECT jsonb_agg(to_jsonb(a) ORDER BY id) FROM audit_events a WHERE target_id=p.id::text))
        FROM financial_purchase_requests p WHERE id=$1")
        .bind(*id.as_uuid()).fetch_one(pool).await.unwrap()
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn maker_can_prepare_only_after_independent_admin_approval(pool: PgPool) {
    console_platform_request_context::scope_org(OrgId::knl(), async move {
        let seeded = seed_financial_context(&pool).await;
        let store = PgFinancialStore::new(approval_runtime_pool(&pool).await);
        for amount in [500_000, 3_000_000] {
            for maker in [seeded.admin, seeded.receptionist] {
                let checker = seed_user(&pool, "Other admin", "ADMIN", seeded.branch_id).await;
                let id =
                    submitted_purchase(&store, &seeded, seeded.admin, seeded.receptionist, amount)
                        .await;
                store
                    .approve_purchase_admin(approval_command(id, checker))
                    .await
                    .unwrap();
                let prepared = store
                    .prepare_expenditure(preparation_command(id, maker))
                    .await
                    .unwrap();
                assert_eq!(
                    prepared.status,
                    if amount > 2_000_000 {
                        PurchaseStatus::ExecutivePending
                    } else {
                        PurchaseStatus::ReadyToExecute
                    }
                );
                if amount > 2_000_000 {
                    assert_eq!(
                        store
                            .approve_purchase_executive(approval_command(id, seeded.executive))
                            .await
                            .unwrap()
                            .status,
                        PurchaseStatus::ReadyToExecute
                    );
                }
            }
        }
    })
    .await;
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn preparation_and_executive_approval_cannot_substitute_for_each_other(pool: PgPool) {
    console_platform_request_context::scope_org(OrgId::knl(), async move {
        let seeded = seed_financial_context(&pool).await;
        let root = seed_user(&pool, "Root", "SUPER_ADMIN", seeded.branch_id).await;
        let store = PgFinancialStore::new(approval_runtime_pool(&pool).await);
        for amount in [500_000, 3_000_000] {
            let id = submitted_purchase(
                &store,
                &seeded,
                seeded.mechanic,
                seeded.receptionist,
                amount,
            )
            .await;
            store
                .approve_purchase_admin(approval_command(id, seeded.admin))
                .await
                .unwrap();
            let before = purchase_evidence(&pool, id).await;
            assert!(
                store
                    .approve_purchase_executive(approval_command(id, root))
                    .await
                    .is_err()
            );
            assert_eq!(purchase_evidence(&pool, id).await, before);
            store
                .prepare_expenditure(preparation_command(id, seeded.receptionist))
                .await
                .unwrap();
            if amount > 2_000_000 {
                let before = purchase_evidence(&pool, id).await;
                assert!(
                    store
                        .prepare_expenditure(preparation_command(id, root))
                        .await
                        .is_err()
                );
                assert_eq!(purchase_evidence(&pool, id).await, before);
                store
                    .approve_purchase_executive(approval_command(id, root))
                    .await
                    .unwrap();
            }
        }
    })
    .await;
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn missing_submission_and_invalid_approval_provenance_fail_closed(pool: PgPool) {
    console_platform_request_context::scope_org(OrgId::knl(), async move {
        let seeded = seed_financial_context(&pool).await;
        let root = seed_user(&pool, "Root", "SUPER_ADMIN", seeded.branch_id).await;
        let store = PgFinancialStore::new(approval_runtime_pool(&pool).await);
        // Historical incomplete or exempt records are preserved, never treated as new authority.
        for amount in [500_000, 3_000_000] {
            for invalid in ["missing_submitter", "missing_admin", "requester_admin", "submitter_admin"] {
                let id = submitted_purchase(&store, &seeded, seeded.mechanic, seeded.receptionist, amount).await;
                store.approve_purchase_admin(approval_command(id, seeded.admin)).await.unwrap();
                sqlx::query("UPDATE financial_purchase_requests SET
                    submitted_by=CASE WHEN $2='missing_submitter' THEN NULL ELSE submitted_by END,
                    admin_approved_by=CASE $2 WHEN 'missing_admin' THEN NULL WHEN 'requester_admin' THEN requested_by
                        WHEN 'submitter_admin' THEN submitted_by ELSE admin_approved_by END WHERE id=$1")
                    .bind(*id.as_uuid()).bind(invalid).execute(&pool).await.unwrap();
                let before = purchase_evidence(&pool, id).await;
                assert_financial_forbidden(store.prepare_expenditure(preparation_command(id, root)).await.unwrap_err());
                assert_eq!(purchase_evidence(&pool, id).await, before);
            }
        }
        for executive in [false, true] {
            let id = submitted_purchase(&store, &seeded, seeded.mechanic, seeded.receptionist, 3_000_000).await;
            if executive {
                store.approve_purchase_admin(approval_command(id, seeded.admin)).await.unwrap();
                store.prepare_expenditure(preparation_command(id, seeded.receptionist)).await.unwrap();
            }
            sqlx::query("UPDATE financial_purchase_requests SET submitted_by=NULL WHERE id=$1").bind(*id.as_uuid()).execute(&pool).await.unwrap();
            let before = purchase_evidence(&pool, id).await;
            let result = if executive {store.approve_purchase_executive(approval_command(id, root)).await}
                else {store.approve_purchase_admin(approval_command(id, root)).await};
            assert_financial_forbidden(result.unwrap_err());
            assert_eq!(purchase_evidence(&pool, id).await, before);
        }
    }).await;
}
fn assert_financial_forbidden(err: console_financial_adapter_postgres::PgFinancialError) {
    match err {
        console_financial_adapter_postgres::PgFinancialError::Domain(e) => {
            assert_eq!(e.kind, console_kernel_core::ErrorKind::Forbidden)
        }
        other => panic!("unexpected {other:?}"),
    }
}
