#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::{Arc, Mutex};

use console_kernel_core::{BranchId, ErrorKind, OrgId, TraceContext, UserId, WorkOrderId};
use console_workorder_adapter_postgres::PgWorkOrderStore;
use console_workorder_application::{
    AssignmentInput, CreateDailyPlanCommand, CreateOutsourceWorkCommand, CreateSettlementCommand,
    CreateWorkOrderCommand, DailyPlanItemInput, DailyPlanStatus, ReviewDailyPlanCommand,
    ReviewTargetChangeCommand, SendDailyPlanForReviewCommand, SettlementLineInput,
    SubmitReportCommand, SubmitSettlementCommand, TargetChangeDecision, TargetChangeRequestCommand,
    UpdatePriorityCommand, UpdateWorkOrderIntakeCommand, WorkOrderApprovalCommand,
    WorkOrderAssignmentCommand, WorkOrderCreatedEvent, WorkOrderCreatedFuture,
    WorkOrderCreatedListener, WorkOrderStartCommand,
};
use console_workorder_domain::{
    AssignmentRole, AttachmentStage, PriorityLevel, SettlementLineKind, WorkOrderStatus,
    WorkResultType,
};
use sqlx::{PgPool, Row};
use time::{Duration, OffsetDateTime, macros::date};

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn lifecycle_mutations_persist_state_and_audit_in_order(pool: PgPool) {
    console_platform_request_context::scope_org(console_kernel_core::OrgId::knl(), async move {
        let seeded = seed_operational_context(&pool).await;
        let store = PgWorkOrderStore::new(pool.clone());
        let created = store
            .create_work_order(CreateWorkOrderCommand {
                maintenance_type: None,
                maintenance_cause: None,
                actor: seeded.receptionist,
                branch_id: seeded.branch_id,
                management_no: "#290".to_owned(),
                symptom: "Hydraulic oil leak".to_owned(),
                customer_request: Some("Inspect before afternoon shift".to_owned()),
                target_due_at: Some(OffsetDateTime::now_utc() + Duration::days(1)),
                trace: TraceContext::generate(),
                occurred_at: OffsetDateTime::now_utc(),
            })
            .await
            .unwrap();

        assert_eq!(created.branch_id, seeded.branch_id);
        assert_eq!(created.status, WorkOrderStatus::Received);
        // The intake submitter cannot set 중요도: CreateWorkOrderCommand carries no
        // priority field, so a freshly created work order is server-assigned UNSET
        // and waits for an admin to classify it via update_priority (asserted below).
        assert_eq!(created.priority, PriorityLevel::Unset);
        assert_eq!(*created.equipment_id.as_uuid(), seeded.equipment_id);
        assert!(created.request_no.ends_with("-001"));

        let prioritized = store
            .update_priority(UpdatePriorityCommand {
                actor: seeded.admin,
                work_order_id: created.id,
                priority: PriorityLevel::P2,
                trace: TraceContext::generate(),
                occurred_at: OffsetDateTime::now_utc(),
            })
            .await
            .unwrap();
        assert_eq!(prioritized.priority, PriorityLevel::P2);

        let assigned = store
            .assign_work_order(WorkOrderAssignmentCommand {
                actor: seeded.admin,
                work_order_id: created.id,
                assignments: vec![
                    AssignmentInput {
                        mechanic_id: seeded.mechanic,
                        role: AssignmentRole::Primary,
                    },
                    AssignmentInput {
                        mechanic_id: seeded.helper,
                        role: AssignmentRole::Secondary,
                    },
                ],
                admin_approver_id: Some(seeded.admin),
                executive_approver_id: Some(seeded.executive),
                trace: TraceContext::generate(),
                occurred_at: OffsetDateTime::now_utc(),
            })
            .await
            .unwrap();
        assert_eq!(assigned.status, WorkOrderStatus::Assigned);

        let started = store
            .start_work(WorkOrderStartCommand {
                actor: seeded.mechanic,
                work_order_id: created.id,
                trace: TraceContext::generate(),
                occurred_at: OffsetDateTime::now_utc(),
            })
            .await
            .unwrap();
        assert_eq!(started.status, WorkOrderStatus::InProgress);

        let reported = store
            .submit_report(SubmitReportCommand {
                actor: seeded.mechanic,
                work_order_id: created.id,
                result_type: WorkResultType::Completed,
                diagnosis: "Hose fitting loosened under load".to_owned(),
                action_taken: "Retightened fitting and pressure-tested".to_owned(),
                trace: TraceContext::generate(),
                occurred_at: OffsetDateTime::now_utc(),
            })
            .await
            .unwrap();
        assert_eq!(reported.status, WorkOrderStatus::ReportSubmitted);

        let admin_review = store
            .approve_work_order(WorkOrderApprovalCommand {
                actor: seeded.admin,
                work_order_id: created.id,
                comment: "검토 의견".to_owned(),
                trace: TraceContext::generate(),
                occurred_at: OffsetDateTime::now_utc(),
            })
            .await
            .unwrap();
        assert_eq!(admin_review.status, WorkOrderStatus::AdminReview);

        let blocked = store
            .approve_work_order(WorkOrderApprovalCommand {
                actor: seeded.executive,
                work_order_id: created.id,
                comment: "검토 의견".to_owned(),
                trace: TraceContext::generate(),
                occurred_at: OffsetDateTime::now_utc(),
            })
            .await
            .unwrap_err();
        assert_eq!(blocked.kind(), ErrorKind::Conflict);
        assert_eq!(
            store.work_order(created.id).await.unwrap().status,
            WorkOrderStatus::AdminReview
        );

        insert_evidence_media(
            &pool,
            created.id,
            seeded.mechanic,
            AttachmentStage::Report,
            "VERIFIED",
        )
        .await;

        let completed = store
            .approve_work_order(WorkOrderApprovalCommand {
                actor: seeded.executive,
                work_order_id: created.id,
                comment: "검토 의견".to_owned(),
                trace: TraceContext::generate(),
                occurred_at: OffsetDateTime::now_utc(),
            })
            .await
            .unwrap();
        assert_eq!(completed.status, WorkOrderStatus::FinalCompleted);

        let actions = audit_actions_for(&pool, created.id).await;
        assert_eq!(
            actions,
            vec![
                "work_order.create",
                "work_order.priority",
                "work_order.assign",
                "work_order.start",
                "work_order.report",
                "work_order.approve",
                "work_order.approve",
            ]
        );
    })
    .await;
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn update_intake_edits_work_order_narrative_and_audits(pool: PgPool) {
    console_platform_request_context::scope_org(console_kernel_core::OrgId::knl(), async move {
        let seeded = seed_operational_context(&pool).await;
        let store = PgWorkOrderStore::new(pool.clone());
        let created = store
            .create_work_order(CreateWorkOrderCommand {
                maintenance_type: None,
                maintenance_cause: None,
                actor: seeded.receptionist,
                branch_id: seeded.branch_id,
                management_no: "#290".to_owned(),
                symptom: "Hydraulic oil leak".to_owned(),
                customer_request: Some("Inspect before afternoon shift".to_owned()),
                target_due_at: None,
                trace: TraceContext::generate(),
                occurred_at: OffsetDateTime::now_utc(),
            })
            .await
            .unwrap();

        let updated = store
            .update_work_order_intake(UpdateWorkOrderIntakeCommand {
                maintenance_type: None,
                maintenance_cause: None,
                actor: seeded.admin,
                work_order_id: created.id,
                symptom: Some("Hydraulic oil leak with pump noise".to_owned()),
                // Empty string is an intentional clear of the optional field.
                customer_request: Some("".to_owned()),
                trace: TraceContext::generate(),
                occurred_at: OffsetDateTime::now_utc(),
            })
            .await
            .unwrap();

        assert_eq!(updated.id, created.id);
        let row = sqlx::query("SELECT symptom, customer_request FROM work_orders WHERE id = $1")
            .bind(*created.id.as_uuid())
            .fetch_one(&pool)
            .await
            .unwrap();
        let symptom: String = row.try_get("symptom").unwrap();
        let customer_request: Option<String> = row.try_get("customer_request").unwrap();
        assert_eq!(symptom, "Hydraulic oil leak with pump noise");
        assert_eq!(customer_request, None);

        let actions = audit_actions_for(&pool, created.id).await;
        assert_eq!(
            actions,
            vec!["work_order.create", "work_order.update_intake"]
        );
    })
    .await;
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn final_completion_ignores_legacy_flag_and_blocks_unverified_completion_evidence(
    pool: PgPool,
) {
    console_platform_request_context::scope_org(console_kernel_core::OrgId::knl(), async move {
        let seeded = seed_operational_context(&pool).await;
        let store = PgWorkOrderStore::new(pool.clone());
        let created = create_reported_work_order(&store, &seeded).await;

        let admin_review = store
            .approve_work_order(WorkOrderApprovalCommand {
                actor: seeded.admin,
                work_order_id: created.id,
                comment: "검토 의견".to_owned(),
                trace: TraceContext::generate(),
                occurred_at: OffsetDateTime::now_utc(),
            })
            .await
            .unwrap();
        assert_eq!(admin_review.status, WorkOrderStatus::AdminReview);

        sqlx::query("UPDATE work_orders SET evidence_verified = true WHERE id = $1")
            .bind(*created.id.as_uuid())
            .execute(&pool)
            .await
            .unwrap();
        insert_evidence_media(
            &pool,
            created.id,
            seeded.mechanic,
            AttachmentStage::After,
            "PENDING",
        )
        .await;

        let blocked = store
            .approve_work_order(WorkOrderApprovalCommand {
                actor: seeded.executive,
                work_order_id: created.id,
                comment: "검토 의견".to_owned(),
                trace: TraceContext::generate(),
                occurred_at: OffsetDateTime::now_utc(),
            })
            .await
            .unwrap_err();
        assert_eq!(blocked.kind(), ErrorKind::Conflict);
        assert_eq!(
            store.work_order(created.id).await.unwrap().status,
            WorkOrderStatus::AdminReview
        );
        let history_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM equipment_maintenance_history WHERE work_order_id = $1",
        )
        .bind(*created.id.as_uuid())
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(
            history_count, 0,
            "a denied final completion must leave maintenance history unchanged"
        );
    })
    .await;
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn request_numbers_are_race_safe_and_sequential_per_day(pool: PgPool) {
    console_platform_request_context::scope_org(console_kernel_core::OrgId::knl(), async move {
        let seeded = seed_operational_context(&pool).await;
        let store = PgWorkOrderStore::new(pool.clone());
        let now = OffsetDateTime::now_utc();

        let first = store.create_work_order(CreateWorkOrderCommand {
            maintenance_type: None,
            maintenance_cause: None,
            actor: seeded.receptionist,
            branch_id: seeded.branch_id,
            management_no: "290".to_owned(),
            symptom: "First concurrent intake".to_owned(),
            customer_request: None,
            target_due_at: None,
            trace: TraceContext::generate(),
            occurred_at: now,
        });
        let second = store.create_work_order(CreateWorkOrderCommand {
            maintenance_type: None,
            maintenance_cause: None,
            actor: seeded.receptionist,
            branch_id: seeded.branch_id,
            management_no: "290".to_owned(),
            symptom: "Second concurrent intake".to_owned(),
            customer_request: None,
            target_due_at: None,
            trace: TraceContext::generate(),
            occurred_at: now,
        });

        let (first, second) = tokio::join!(first, second);
        let mut request_numbers = vec![first.unwrap().request_no, second.unwrap().request_no];
        request_numbers.sort();

        let ymd = now.date();
        let prefix = format!(
            "{:04}{:02}{:02}",
            ymd.year(),
            u8::from(ymd.month()),
            ymd.day()
        );
        assert_eq!(
            request_numbers,
            vec![format!("{prefix}-001"), format!("{prefix}-002")]
        );
    })
    .await;
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn create_work_order_emits_post_commit_created_event(pool: PgPool) {
    console_platform_request_context::scope_org(console_kernel_core::OrgId::knl(), async move {
        let seeded = seed_operational_context(&pool).await;
        let listener = Arc::new(RecordingCreatedListener::new(pool.clone()));
        let store = PgWorkOrderStore::new(pool.clone()).with_created_listener(listener.clone());

        let created = store
            .create_work_order(CreateWorkOrderCommand {
                maintenance_type: None,
                maintenance_cause: None,
                actor: seeded.receptionist,
                branch_id: seeded.branch_id,
                management_no: "290".to_owned(),
                symptom: "Create messenger thread".to_owned(),
                customer_request: None,
                target_due_at: None,
                trace: TraceContext::generate(),
                occurred_at: OffsetDateTime::now_utc(),
            })
            .await
            .unwrap();

        let events = listener.events.lock().unwrap().clone();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].work_order_id, created.id);
        assert_eq!(events[0].branch_id, seeded.branch_id);
        assert_eq!(events[0].actor, seeded.receptionist);
    })
    .await;
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn target_change_daily_plan_and_outsource_flows_are_audited(pool: PgPool) {
    console_platform_request_context::scope_org(console_kernel_core::OrgId::knl(), async move {
        let seeded = seed_operational_context(&pool).await;
        let store = PgWorkOrderStore::new(pool.clone());
        let work_order = create_assigned_work_order(&store, &seeded).await;

        let target_request = store
            .request_target_change(TargetChangeRequestCommand {
                actor: seeded.mechanic,
                work_order_id: work_order.id,
                requested_target_due_at: OffsetDateTime::now_utc() + Duration::days(3),
                reason: "Customer kept equipment in production".to_owned(),
                trace: TraceContext::generate(),
                occurred_at: OffsetDateTime::now_utc(),
            })
            .await
            .unwrap();
        let reviewed = store
            .review_target_change(ReviewTargetChangeCommand {
                actor: seeded.admin,
                request_id: target_request.id,
                decision: TargetChangeDecision::Approved,
                memo: Some("Customer schedule confirmed".to_owned()),
                trace: TraceContext::generate(),
                occurred_at: OffsetDateTime::now_utc(),
            })
            .await
            .unwrap();
        assert_eq!(reviewed.status, TargetChangeDecision::Approved.into());

        let daily_plan = store
            .create_daily_plan(CreateDailyPlanCommand {
                actor: seeded.mechanic,
                branch_id: seeded.branch_id,
                mechanic_id: seeded.mechanic,
                plan_date: date!(2026 - 06 - 12),
                items: vec![DailyPlanItemInput {
                    work_order_id: work_order.id,
                    description: "Repair hydraulic leak".to_owned(),
                }],
                trace: TraceContext::generate(),
                occurred_at: OffsetDateTime::now_utc(),
            })
            .await
            .unwrap();
        assert_eq!(daily_plan.status, DailyPlanStatus::Draft);
        assert_eq!(daily_plan.items.len(), 1);
        assert_eq!(daily_plan.items[0].work_order_id, Some(work_order.id));
        assert_eq!(
            daily_plan.items[0].request_no.as_deref(),
            Some(work_order.request_no.as_str())
        );

        let requested = store
            .request_daily_plan_review(SendDailyPlanForReviewCommand {
                actor: seeded.mechanic,
                plan_id: daily_plan.id,
                trace: TraceContext::generate(),
                occurred_at: OffsetDateTime::now_utc(),
            })
            .await
            .unwrap();
        assert_eq!(requested.status, DailyPlanStatus::Requested);
        assert_eq!(
            requested.items[0].request_no.as_deref(),
            Some(work_order.request_no.as_str())
        );

        let approved = store
            .review_daily_plan(ReviewDailyPlanCommand {
                actor: seeded.admin,
                plan_id: daily_plan.id,
                decision: DailyPlanStatus::Approved,
                memo: Some("Plan accepted".to_owned()),
                trace: TraceContext::generate(),
                occurred_at: OffsetDateTime::now_utc(),
            })
            .await
            .unwrap();
        assert_eq!(approved.status, DailyPlanStatus::Approved);

        let confirmed = store
            .confirm_daily_plan(SendDailyPlanForReviewCommand {
                actor: seeded.mechanic,
                plan_id: daily_plan.id,
                trace: TraceContext::generate(),
                occurred_at: OffsetDateTime::now_utc(),
            })
            .await
            .unwrap();
        assert_eq!(confirmed.status, DailyPlanStatus::FinalConfirmed);

        let outsource = store
            .create_outsource_work(CreateOutsourceWorkCommand {
                actor: seeded.admin,
                work_order_id: work_order.id,
                vendor_name: "Reliable Forklift Service".to_owned(),
                vendor_contact: Some("ops@example.invalid".to_owned()),
                reason: "Requires vendor diagnostic tool".to_owned(),
                trace: TraceContext::generate(),
                occurred_at: OffsetDateTime::now_utc(),
            })
            .await
            .unwrap();
        assert_eq!(outsource.vendor_name, "Reliable Forklift Service");

        let actions = audit_actions_for(&pool, work_order.id).await;
        assert!(actions.contains(&"target_change.request".to_owned()));
        assert!(actions.contains(&"target_change.review".to_owned()));
        assert!(actions.contains(&"work_order.outsource".to_owned()));

        let plan_audits: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM audit_events WHERE target_type = 'daily_work_plan'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(plan_audits, 4);
    })
    .await;
}

struct SeededContext {
    org_id: OrgId,
    branch_id: BranchId,
    receptionist: UserId,
    mechanic: UserId,
    helper: UserId,
    admin: UserId,
    executive: UserId,
    equipment_id: uuid::Uuid,
}

async fn create_assigned_work_order(
    store: &PgWorkOrderStore,
    seeded: &SeededContext,
) -> console_workorder_application::WorkOrderSummary {
    let created = store
        .create_work_order(CreateWorkOrderCommand {
            maintenance_type: None,
            maintenance_cause: None,
            actor: seeded.receptionist,
            branch_id: seeded.branch_id,
            management_no: "290".to_owned(),
            symptom: "Assigned WO fixture".to_owned(),
            customer_request: None,
            target_due_at: None,
            trace: TraceContext::generate(),
            occurred_at: OffsetDateTime::now_utc(),
        })
        .await
        .unwrap();

    store
        .assign_work_order(WorkOrderAssignmentCommand {
            actor: seeded.admin,
            work_order_id: created.id,
            assignments: vec![AssignmentInput {
                mechanic_id: seeded.mechanic,
                role: AssignmentRole::Primary,
            }],
            admin_approver_id: Some(seeded.admin),
            executive_approver_id: Some(seeded.executive),
            trace: TraceContext::generate(),
            occurred_at: OffsetDateTime::now_utc(),
        })
        .await
        .unwrap()
}

async fn create_reported_work_order(
    store: &PgWorkOrderStore,
    seeded: &SeededContext,
) -> console_workorder_application::WorkOrderSummary {
    let assigned = create_assigned_work_order(store, seeded).await;
    store
        .start_work(WorkOrderStartCommand {
            actor: seeded.mechanic,
            work_order_id: assigned.id,
            trace: TraceContext::generate(),
            occurred_at: OffsetDateTime::now_utc(),
        })
        .await
        .unwrap();
    store
        .submit_report(SubmitReportCommand {
            actor: seeded.mechanic,
            work_order_id: assigned.id,
            result_type: WorkResultType::Completed,
            diagnosis: "Pump cavitation under load".to_owned(),
            action_taken: "Replaced inlet seal and pressure-tested".to_owned(),
            trace: TraceContext::generate(),
            occurred_at: OffsetDateTime::now_utc(),
        })
        .await
        .unwrap()
}

async fn insert_evidence_media(
    pool: &PgPool,
    work_order_id: WorkOrderId,
    uploaded_by: UserId,
    stage: AttachmentStage,
    status: &str,
) {
    sqlx::query(
        r#"
        INSERT INTO evidence_media (
            work_order_id, stage, s3_key, content_type, size_bytes,
            uploaded_by, worm_replica_status, retry_count, org_id
        )
        VALUES ($1, $2, $3, 'image/jpeg', 1024, $4, $5, 0, $6)
        "#,
    )
    .bind(*work_order_id.as_uuid())
    .bind(stage.as_db_str())
    .bind(format!(
        "work-orders/{}/{}/{}.jpg",
        work_order_id,
        stage.as_db_str(),
        uuid::Uuid::new_v4()
    ))
    .bind(*uploaded_by.as_uuid())
    .bind(status)
    .bind(*OrgId::knl().as_uuid())
    .execute(pool)
    .await
    .unwrap();
}

async fn audit_actions_for(pool: &PgPool, work_order_id: WorkOrderId) -> Vec<String> {
    sqlx::query_scalar(
        r#"
        SELECT action
        FROM audit_events
        WHERE target_id = $1
        ORDER BY occurred_at, created_at
        "#,
    )
    .bind(work_order_id.to_string())
    .fetch_all(pool)
    .await
    .unwrap()
}

async fn seed_operational_context(pool: &PgPool) -> SeededContext {
    let org_id = OrgId::knl();
    let branch_id = seed_branch(pool).await;
    let receptionist = seed_user(pool, "Receptionist", "RECEPTIONIST", branch_id).await;
    let mechanic = seed_user(pool, "Mechanic", "MECHANIC", branch_id).await;
    let helper = seed_user(pool, "Helper", "MECHANIC", branch_id).await;
    let admin = seed_user(pool, "Admin", "ADMIN", branch_id).await;
    let executive = seed_user(pool, "Executive", "EXECUTIVE", branch_id).await;
    let equipment_id = seed_equipment(pool, branch_id).await;

    SeededContext {
        org_id,
        branch_id,
        receptionist,
        mechanic,
        helper,
        admin,
        executive,
        equipment_id,
    }
}

/// Seeds the existing operational fixture shape for a specified tenant. This
/// seam lets lifecycle tests prove tenant removal without weakening production
/// RLS.
async fn seed_operational_context_for_org(pool: &PgPool, org_id: OrgId) -> SeededContext {
    let branch_id = seed_branch_for_org(pool, org_id).await;
    let receptionist =
        seed_user_for_org(pool, org_id, "Receptionist", "RECEPTIONIST", branch_id).await;
    let mechanic = seed_user_for_org(pool, org_id, "Mechanic", "MECHANIC", branch_id).await;
    let helper = seed_user_for_org(pool, org_id, "Helper", "MECHANIC", branch_id).await;
    let admin = seed_user_for_org(pool, org_id, "Admin", "ADMIN", branch_id).await;
    let executive = seed_user_for_org(pool, org_id, "Executive", "EXECUTIVE", branch_id).await;
    let equipment_id = seed_equipment_for_org(pool, org_id, branch_id).await;

    SeededContext {
        org_id,
        branch_id,
        receptionist,
        mechanic,
        helper,
        admin,
        executive,
        equipment_id,
    }
}

async fn seed_branch(pool: &PgPool) -> BranchId {
    seed_branch_for_org(pool, OrgId::knl()).await
}

async fn seed_branch_for_org(pool: &PgPool, org_id: OrgId) -> BranchId {
    let org_uuid = *org_id.as_uuid();
    let region_id: uuid::Uuid =
        sqlx::query_scalar("INSERT INTO regions (name, org_id) VALUES ($1, $2) RETURNING id")
            .bind(format!("Region {}", uuid::Uuid::new_v4()))
            .bind(org_uuid)
            .fetch_one(pool)
            .await
            .unwrap();
    let branch_id: uuid::Uuid = sqlx::query_scalar(
        "INSERT INTO branches (region_id, name, org_id) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(region_id)
    .bind("HQ Test")
    .bind(org_uuid)
    .fetch_one(pool)
    .await
    .unwrap();
    BranchId::from_uuid(branch_id)
}

async fn seed_user(pool: &PgPool, name: &str, role: &str, branch_id: BranchId) -> UserId {
    seed_user_for_org(pool, OrgId::knl(), name, role, branch_id).await
}

async fn seed_user_for_org(
    pool: &PgPool,
    org_id: OrgId,
    name: &str,
    role: &str,
    branch_id: BranchId,
) -> UserId {
    let org_uuid = *org_id.as_uuid();
    let user_id = UserId::new();
    sqlx::query("INSERT INTO users (id, display_name, roles, org_id) VALUES ($1, $2, $3, $4)")
        .bind(*user_id.as_uuid())
        .bind(name)
        .bind(Vec::from([role]))
        .bind(org_uuid)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO user_branches (user_id, branch_id, org_id) VALUES ($1, $2, $3)")
        .bind(*user_id.as_uuid())
        .bind(*branch_id.as_uuid())
        .bind(org_uuid)
        .execute(pool)
        .await
        .unwrap();
    user_id
}

#[derive(Debug)]
struct RecordingCreatedListener {
    pool: PgPool,
    events: Arc<Mutex<Vec<WorkOrderCreatedEvent>>>,
}

impl RecordingCreatedListener {
    fn new(pool: PgPool) -> Self {
        Self {
            pool,
            events: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

impl WorkOrderCreatedListener for RecordingCreatedListener {
    fn work_order_created(&self, event: WorkOrderCreatedEvent) -> WorkOrderCreatedFuture<'_> {
        Box::pin(async move {
            let row = sqlx::query(
                r#"
                SELECT EXISTS(
                    SELECT 1 FROM work_orders WHERE id = $1
                ) AS work_order_exists,
                EXISTS(
                    SELECT 1 FROM audit_events
                    WHERE action = 'work_order.create'
                      AND target_id = $2
                ) AS audit_exists
                "#,
            )
            .bind(*event.work_order_id.as_uuid())
            .bind(event.work_order_id.to_string())
            .fetch_one(&self.pool)
            .await
            .unwrap();
            assert!(row.get::<Option<bool>, _>("work_order_exists").unwrap());
            assert!(row.get::<Option<bool>, _>("audit_exists").unwrap());
            self.events.lock().unwrap().push(event);
            Ok(())
        })
    }
}

async fn seed_equipment(pool: &PgPool, branch_id: BranchId) -> uuid::Uuid {
    seed_equipment_for_org(pool, OrgId::knl(), branch_id).await
}

async fn seed_equipment_for_org(pool: &PgPool, org_id: OrgId, branch_id: BranchId) -> uuid::Uuid {
    let org_uuid = *org_id.as_uuid();
    let customer_id: uuid::Uuid = sqlx::query_scalar(
        "INSERT INTO registry_customers (branch_id, name, org_id) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(*branch_id.as_uuid())
    .bind("Customer A")
    .bind(org_uuid)
    .fetch_one(pool)
    .await
    .unwrap();
    let site_id: uuid::Uuid = sqlx::query_scalar(
        "INSERT INTO registry_sites (branch_id, customer_id, name, org_id) VALUES ($1, $2, $3, $4) RETURNING id",
    )
    .bind(*branch_id.as_uuid())
    .bind(customer_id)
    .bind("Site A")
    .bind(org_uuid)
    .fetch_one(pool)
    .await
    .unwrap();
    sqlx::query_scalar(
        r#"
        INSERT INTO registry_equipment (
            branch_id, customer_id, site_id, equipment_no, management_no,
            manufacturer_code, kind_code, power_code, status,
            specification, ton_text, model, source_sheet, source_row, org_id
        )
        VALUES ($1, $2, $3, 'ABC12-0290', '290',
                'A', 'B', 'C', '임대', '좌식', '2.5', 'GTS25DE', 'test', 1, $4)
        RETURNING id
        "#,
    )
    .bind(*branch_id.as_uuid())
    .bind(customer_id)
    .bind(site_id)
    .bind(org_uuid)
    .fetch_one(pool)
    .await
    .unwrap()
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn final_completion_appends_one_immutable_equipment_history_snapshot(pool: PgPool) {
    console_platform_request_context::scope_org(console_kernel_core::OrgId::knl(), async move {
        let seeded = seed_operational_context(&pool).await;
        let store = PgWorkOrderStore::new(pool.clone());
        let created = create_reported_work_order(&store, &seeded).await;

        store
            .approve_work_order(WorkOrderApprovalCommand {
                actor: seeded.admin,
                work_order_id: created.id,
                comment: "Admin approved the completed report".to_owned(),
                trace: TraceContext::generate(),
                occurred_at: OffsetDateTime::now_utc(),
            })
            .await
            .unwrap();

        let evidence_id = insert_evidence_media_returning_id(
            &pool,
            created.id,
            seeded.mechanic,
            AttachmentStage::Report,
            "VERIFIED",
        )
        .await;
        let ledger_id = insert_cost_ledger_for_work_order(&pool, &seeded, created.id).await;

        let completed = store
            .approve_work_order(WorkOrderApprovalCommand {
                actor: seeded.executive,
                work_order_id: created.id,
                comment: "Executive approved the completed report".to_owned(),
                trace: TraceContext::generate(),
                occurred_at: OffsetDateTime::now_utc(),
            })
            .await
            .unwrap();
        assert_eq!(completed.status, WorkOrderStatus::FinalCompleted);

        let history = sqlx::query(
            "SELECT id, equipment_id, work_order_id FROM equipment_maintenance_history WHERE work_order_id = $1",
        )
        .bind(*created.id.as_uuid())
        .fetch_one(&pool)
        .await
        .unwrap();
        let history_id: uuid::Uuid = history.try_get("id").unwrap();
        assert_eq!(history.try_get::<uuid::Uuid, _>("equipment_id").unwrap(), seeded.equipment_id);
        assert_eq!(history.try_get::<uuid::Uuid, _>("work_order_id").unwrap(), *created.id.as_uuid());

        let history_evidence: Vec<uuid::Uuid> = sqlx::query_scalar(
            "SELECT evidence_media_id FROM equipment_maintenance_history_evidence WHERE history_id = $1",
        )
        .bind(history_id)
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(history_evidence, vec![evidence_id]);

        let history_costs: Vec<uuid::Uuid> = sqlx::query_scalar(
            "SELECT equipment_cost_ledger_id FROM equipment_maintenance_history_costs WHERE history_id = $1",
        )
        .bind(history_id)
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(history_costs, vec![ledger_id]);

        let repeat = store
            .approve_work_order(WorkOrderApprovalCommand {
                actor: seeded.executive,
                work_order_id: created.id,
                comment: "A duplicate terminal approval must not append history".to_owned(),
                trace: TraceContext::generate(),
                occurred_at: OffsetDateTime::now_utc(),
            })
            .await
            .unwrap_err();
        assert_eq!(repeat.kind(), ErrorKind::Conflict);
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM equipment_maintenance_history WHERE work_order_id = $1",
        )
        .bind(*created.id.as_uuid())
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(count, 1);

        let update = sqlx::query(
            "UPDATE equipment_maintenance_history SET completed_at = now() WHERE id = $1",
        )
        .bind(history_id)
        .execute(&pool)
        .await;
        assert!(update.is_err(), "maintenance history must be immutable");
        let delete = sqlx::query("DELETE FROM equipment_maintenance_history WHERE id = $1")
            .bind(history_id)
            .execute(&pool)
            .await;
        assert!(delete.is_err(), "maintenance history must be append-only");

        // console_rt may read its tenant history, but cannot construct a parent-only
        // or partial snapshot: the SECURITY DEFINER append operation owns all writes.
        let mut runtime_conn = pool.acquire().await.unwrap();
        sqlx::query("SET ROLE console_rt").execute(&mut *runtime_conn).await.unwrap();
        sqlx::query("SELECT set_config('app.current_org', $1, false)")
            .bind(OrgId::knl().to_string())
            .execute(&mut *runtime_conn).await.unwrap();
        let same_org_read: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM equipment_maintenance_history WHERE id = $1",
        )
        .bind(history_id)
        .fetch_one(&mut *runtime_conn)
        .await
        .unwrap();
        assert_eq!(same_org_read, 1);

        // A runtime caller can set the deletion GUC, but it remains harmless:
        // direct DML privileges are revoked and only the SECURITY DEFINER
        // archive-removal procedure can consume that flag.
        sqlx::query("SELECT set_config('app.maintenance_force_remove', 'on', false)")
            .execute(&mut *runtime_conn)
            .await
            .unwrap();
        let child_first_delete = sqlx::query(
            "DELETE FROM equipment_maintenance_history_evidence WHERE history_id = $1",
        )
        .bind(history_id)
        .execute(&mut *runtime_conn)
        .await
        .expect_err("console_rt must not delete immutable child history even with the bypass GUC armed");
        assert_eq!(
            child_first_delete
                .as_database_error()
                .and_then(|error| error.code().map(|code| code.to_string()))
                .as_deref(),
            Some("42501"),
            "the child-first deletion must be denied by privileges before the trigger guard"
        );
        let parent_only = sqlx::query(
            "INSERT INTO equipment_maintenance_history (org_id, equipment_id, work_order_id, completed_at) VALUES ($1,$2,$3,now())",
        ).bind(*OrgId::knl().as_uuid()).bind(seeded.equipment_id).bind(*created.id.as_uuid()).execute(&mut *runtime_conn).await;
        assert!(parent_only.is_err(), "runtime role cannot insert a parent-only snapshot");
        let partial_evidence = sqlx::query(
            "INSERT INTO equipment_maintenance_history_evidence (history_id, org_id, evidence_media_id) VALUES ($1,$2,$3)",
        ).bind(history_id).bind(*OrgId::knl().as_uuid()).bind(evidence_id).execute(&mut *runtime_conn).await;
        assert!(partial_evidence.is_err(), "runtime role cannot append partial evidence");
        let partial_cost = sqlx::query(
            "INSERT INTO equipment_maintenance_history_costs (history_id, org_id, equipment_cost_ledger_id) VALUES ($1,$2,$3)",
        ).bind(history_id).bind(*OrgId::knl().as_uuid()).bind(ledger_id).execute(&mut *runtime_conn).await;
        assert!(partial_cost.is_err(), "runtime role cannot append partial cost material");
        let foreign_org = uuid::Uuid::new_v4();
        sqlx::query("SELECT set_config('app.current_org', $1, false)")
            .bind(foreign_org.to_string()).execute(&mut *runtime_conn).await.unwrap();
        let cross_org_read: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM equipment_maintenance_history WHERE id = $1")
            .bind(history_id).fetch_one(&mut *runtime_conn).await.unwrap();
        assert_eq!(cross_org_read, 0, "RLS hides another tenant's parent history");
        let cross_parent = sqlx::query("INSERT INTO equipment_maintenance_history (org_id,equipment_id,work_order_id,completed_at) VALUES ($1,$2,$3,now())")
            .bind(*OrgId::knl().as_uuid()).bind(seeded.equipment_id).bind(*created.id.as_uuid()).execute(&mut *runtime_conn).await;
        assert!(cross_parent.is_err(), "RLS/direct grant denies cross-org parent write");
        let cross_evidence = sqlx::query("INSERT INTO equipment_maintenance_history_evidence (history_id,org_id,evidence_media_id) VALUES ($1,$2,$3)")
            .bind(history_id).bind(*OrgId::knl().as_uuid()).bind(evidence_id).execute(&mut *runtime_conn).await;
        assert!(cross_evidence.is_err(), "RLS/direct grant denies cross-org evidence write");
        let cross_cost = sqlx::query("INSERT INTO equipment_maintenance_history_costs (history_id,org_id,equipment_cost_ledger_id) VALUES ($1,$2,$3)")
            .bind(history_id).bind(*OrgId::knl().as_uuid()).bind(ledger_id).execute(&mut *runtime_conn).await;
        assert!(cross_cost.is_err(), "RLS/direct grant denies cross-org cost write");
        sqlx::query("RESET ROLE").execute(&mut *runtime_conn).await.unwrap();
        sqlx::query("RESET app.current_org")
            .execute(&mut *runtime_conn)
            .await
            .unwrap();
    })
    .await;
}

/// Complete a real work order through both approval stages and return the
/// immutable history row created by the terminal transition.  It deliberately
/// uses the adapter under the tenant request context rather than seeding any
/// history table directly.
async fn complete_work_order_with_history(
    pool: &PgPool,
    org_id: OrgId,
    seeded: &SeededContext,
) -> (WorkOrderId, uuid::Uuid) {
    let store = PgWorkOrderStore::new(pool.clone());
    let (work_order_id, history_id) = console_platform_request_context::scope_org(org_id, async {
        let created = create_reported_work_order(&store, seeded).await;
        store
            .approve_work_order(WorkOrderApprovalCommand {
                actor: seeded.admin,
                work_order_id: created.id,
                comment: "Admin approved the completed report".to_owned(),
                trace: TraceContext::generate(),
                occurred_at: OffsetDateTime::now_utc(),
            })
            .await
            .unwrap();

        insert_evidence_media_returning_id_for_org(
            pool,
            org_id,
            created.id,
            seeded.mechanic,
            AttachmentStage::Report,
            "VERIFIED",
        )
        .await;
        insert_cost_ledger_for_work_order(pool, seeded, created.id).await;

        let completed = store
            .approve_work_order(WorkOrderApprovalCommand {
                actor: seeded.executive,
                work_order_id: created.id,
                comment: "Executive approved the completed report".to_owned(),
                trace: TraceContext::generate(),
                occurred_at: OffsetDateTime::now_utc(),
            })
            .await
            .unwrap();
        assert_eq!(completed.status, WorkOrderStatus::FinalCompleted);

        let history_id: uuid::Uuid = sqlx::query_scalar(
            "SELECT id FROM equipment_maintenance_history WHERE work_order_id = $1",
        )
        .bind(*created.id.as_uuid())
        .fetch_one(pool)
        .await
        .unwrap();
        (created.id, history_id)
    })
    .await;
    (work_order_id, history_id)
}

/// The hard-removal lifecycle must delete maintenance history only through the
/// archived-tenant force-removal procedure.  This is intentionally a two-tenant
/// test: it proves the target's complete final-transition graph is removed while
/// a separately completed KNL graph survives unchanged.
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn force_remove_archived_tenant_removes_full_maintenance_history_only_for_target(
    pool: PgPool,
) {
    let control_org = OrgId::knl();
    let control = seed_operational_context_for_org(&pool, control_org).await;
    let (_control_work_order, control_history_id) =
        complete_work_order_with_history(&pool, control_org, &control).await;

    let removed_org = OrgId::new();
    sqlx::query("INSERT INTO organizations (id, slug, name, status) VALUES ($1, $2, $3, 'ACTIVE')")
        .bind(*removed_org.as_uuid())
        .bind(format!("fr-{}", uuid::Uuid::new_v4().simple()))
        .bind("Force removal maintenance fixture")
        .execute(&pool)
        .await
        .unwrap();
    let removed = seed_operational_context_for_org(&pool, removed_org).await;
    let (removed_work_order, removed_history_id) =
        complete_work_order_with_history(&pool, removed_org, &removed).await;

    let before_parent: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM equipment_maintenance_history WHERE org_id = $1")
            .bind(*removed_org.as_uuid())
            .fetch_one(&pool)
            .await
            .unwrap();
    let before_evidence: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM equipment_maintenance_history_evidence WHERE org_id = $1",
    )
    .bind(*removed_org.as_uuid())
    .fetch_one(&pool)
    .await
    .unwrap();
    let before_costs: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM equipment_maintenance_history_costs WHERE org_id = $1",
    )
    .bind(*removed_org.as_uuid())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!((before_parent, before_evidence, before_costs), (1, 1, 1));

    let blocked_while_active: String =
        sqlx::query_scalar("SELECT platform_force_remove_organization($1)")
            .bind(*removed_org.as_uuid())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        blocked_while_active, "blocked_active",
        "the force-removal function remains archive-gated"
    );

    sqlx::query("UPDATE organizations SET status = 'ARCHIVED' WHERE id = $1")
        .bind(*removed_org.as_uuid())
        .execute(&pool)
        .await
        .unwrap();
    let result: String = sqlx::query_scalar("SELECT platform_force_remove_organization($1)")
        .bind(*removed_org.as_uuid())
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(result, "removed");

    let remaining_history: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM equipment_maintenance_history WHERE org_id = $1")
            .bind(*removed_org.as_uuid())
            .fetch_one(&pool)
            .await
            .unwrap();
    let remaining_history_evidence: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM equipment_maintenance_history_evidence WHERE org_id = $1",
    )
    .bind(*removed_org.as_uuid())
    .fetch_one(&pool)
    .await
    .unwrap();
    let remaining_history_costs: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM equipment_maintenance_history_costs WHERE org_id = $1",
    )
    .bind(*removed_org.as_uuid())
    .fetch_one(&pool)
    .await
    .unwrap();
    let remaining_cost_ledger: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM equipment_cost_ledger WHERE org_id = $1")
            .bind(*removed_org.as_uuid())
            .fetch_one(&pool)
            .await
            .unwrap();
    let remaining_evidence_media: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM evidence_media WHERE org_id = $1")
            .bind(*removed_org.as_uuid())
            .fetch_one(&pool)
            .await
            .unwrap();
    let remaining_work_orders: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM work_orders WHERE org_id = $1")
            .bind(*removed_org.as_uuid())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        (
            remaining_history,
            remaining_history_evidence,
            remaining_history_costs,
            remaining_cost_ledger,
            remaining_evidence_media,
            remaining_work_orders,
        ),
        (0, 0, 0, 0, 0, 0),
        "force removal must remove the target tenant's complete maintenance graph"
    );
    let removed_org_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM organizations WHERE id = $1")
            .bind(*removed_org.as_uuid())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        removed_org_count, 0,
        "the archived tenant shell must be removed"
    );

    let control_org_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM organizations WHERE id = $1")
            .bind(*control_org.as_uuid())
            .fetch_one(&pool)
            .await
            .unwrap();
    let control_parent: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM equipment_maintenance_history WHERE id = $1")
            .bind(control_history_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    let control_evidence: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM equipment_maintenance_history_evidence WHERE history_id = $1",
    )
    .bind(control_history_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    let control_costs: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM equipment_maintenance_history_costs WHERE history_id = $1",
    )
    .bind(control_history_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        control_org_count, 1,
        "independent KNL control tenant remains"
    );
    assert_eq!((control_parent, control_evidence, control_costs), (1, 1, 1));

    // The returned ids must no longer resolve after the archival deletion.
    let removed_work_order_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM work_orders WHERE id = $1")
            .bind(*removed_work_order.as_uuid())
            .fetch_one(&pool)
            .await
            .unwrap();
    let removed_history_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM equipment_maintenance_history WHERE id = $1")
            .bind(removed_history_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!((removed_work_order_count, removed_history_count), (0, 0));
}

async fn insert_evidence_media_returning_id(
    pool: &PgPool,
    work_order_id: WorkOrderId,
    uploaded_by: UserId,
    stage: AttachmentStage,
    status: &str,
) -> uuid::Uuid {
    insert_evidence_media_returning_id_for_org(
        pool,
        OrgId::knl(),
        work_order_id,
        uploaded_by,
        stage,
        status,
    )
    .await
}

async fn insert_evidence_media_returning_id_for_org(
    pool: &PgPool,
    org_id: OrgId,
    work_order_id: WorkOrderId,
    uploaded_by: UserId,
    stage: AttachmentStage,
    status: &str,
) -> uuid::Uuid {
    sqlx::query_scalar(
        r#"
        INSERT INTO evidence_media (
            work_order_id, stage, s3_key, content_type, size_bytes,
            uploaded_by, worm_replica_status, retry_count, org_id
        )
        VALUES ($1, $2, $3, 'image/jpeg', 1024, $4, $5, 0, $6)
        RETURNING id
        "#,
    )
    .bind(*work_order_id.as_uuid())
    .bind(stage.as_db_str())
    .bind(format!(
        "work-orders/{}/{}/{}.jpg",
        work_order_id,
        stage.as_db_str(),
        uuid::Uuid::new_v4()
    ))
    .bind(*uploaded_by.as_uuid())
    .bind(status)
    .bind(*org_id.as_uuid())
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn insert_cost_ledger_for_work_order(
    pool: &PgPool,
    seeded: &SeededContext,
    work_order_id: WorkOrderId,
) -> uuid::Uuid {
    sqlx::query_scalar(
        r#"
        INSERT INTO equipment_cost_ledger (
            branch_id, equipment_id, work_order_id, source, amount_won, memo,
            residual_before_won, residual_after_won, entry_at, created_by, org_id
        )
        VALUES ($1, $2, $3, 'MANUAL_ADMIN', 1000, 'Verified maintenance expense',
                10000, 9000, now(), $4, $5)
        RETURNING id
        "#,
    )
    .bind(*seeded.branch_id.as_uuid())
    .bind(seeded.equipment_id)
    .bind(*work_order_id.as_uuid())
    .bind(*seeded.admin.as_uuid())
    .bind(*seeded.org_id.as_uuid())
    .fetch_one(pool)
    .await
    .unwrap()
}

/// gh#395 / bead console-ls3: every audit row written by the work-order,
/// daily-plan, and settlement mutations must carry the ACTING tenant's org.
/// The application-layer audit builders hardcoded `.with_org(OrgId::knl())`,
/// so any call site that did not remember to override stamped a non-KNL
/// tenant's audit rows as KNL — and, because `with_audit` arms the
/// transaction-local RLS GUC from `event.org_id`, RLS-scoped the audited
/// mutation itself to the wrong tenant.
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn audit_rows_carry_acting_tenant_org_not_hardcoded_knl(pool: PgPool) {
    let tenant = OrgId::new();
    sqlx::query("INSERT INTO organizations (id, slug, name, status) VALUES ($1, $2, $3, 'ACTIVE')")
        .bind(*tenant.as_uuid())
        .bind(format!("ls3-{}", uuid::Uuid::new_v4().simple()))
        .bind("LS3 audit-tenant fixture")
        .execute(&pool)
        .await
        .unwrap();
    let seeded = seed_operational_context_for_org(&pool, tenant).await;

    console_platform_request_context::scope_org(tenant, async move {
        let store = PgWorkOrderStore::new(pool.clone());

        let created = store
            .create_work_order(CreateWorkOrderCommand {
                maintenance_type: None,
                maintenance_cause: None,
                actor: seeded.receptionist,
                branch_id: seeded.branch_id,
                management_no: "#290".to_owned(),
                symptom: "Hydraulic oil leak".to_owned(),
                customer_request: None,
                target_due_at: Some(OffsetDateTime::now_utc() + Duration::days(1)),
                trace: TraceContext::generate(),
                occurred_at: OffsetDateTime::now_utc(),
            })
            .await
            .unwrap();

        store
            .update_priority(UpdatePriorityCommand {
                actor: seeded.admin,
                work_order_id: created.id,
                priority: PriorityLevel::P2,
                trace: TraceContext::generate(),
                occurred_at: OffsetDateTime::now_utc(),
            })
            .await
            .unwrap();

        store
            .update_work_order_intake(UpdateWorkOrderIntakeCommand {
                actor: seeded.admin,
                work_order_id: created.id,
                symptom: Some("Hydraulic oil leak worsens under load".to_owned()),
                customer_request: None,
                maintenance_type: None,
                maintenance_cause: None,
                trace: TraceContext::generate(),
                occurred_at: OffsetDateTime::now_utc(),
            })
            .await
            .unwrap();

        let target_change = store
            .request_target_change(TargetChangeRequestCommand {
                actor: seeded.admin,
                work_order_id: created.id,
                requested_target_due_at: OffsetDateTime::now_utc() + Duration::days(3),
                reason: "Parts on backorder".to_owned(),
                trace: TraceContext::generate(),
                occurred_at: OffsetDateTime::now_utc(),
            })
            .await
            .unwrap();

        store
            .review_target_change(ReviewTargetChangeCommand {
                actor: seeded.executive,
                request_id: target_change.id,
                decision: TargetChangeDecision::Approved,
                memo: Some("Approved for backordered parts".to_owned()),
                trace: TraceContext::generate(),
                occurred_at: OffsetDateTime::now_utc(),
            })
            .await
            .unwrap();

        store
            .assign_work_order(WorkOrderAssignmentCommand {
                actor: seeded.admin,
                work_order_id: created.id,
                assignments: vec![AssignmentInput {
                    mechanic_id: seeded.mechanic,
                    role: AssignmentRole::Primary,
                }],
                admin_approver_id: Some(seeded.admin),
                executive_approver_id: Some(seeded.executive),
                trace: TraceContext::generate(),
                occurred_at: OffsetDateTime::now_utc(),
            })
            .await
            .unwrap();

        store
            .start_work(WorkOrderStartCommand {
                actor: seeded.mechanic,
                work_order_id: created.id,
                trace: TraceContext::generate(),
                occurred_at: OffsetDateTime::now_utc(),
            })
            .await
            .unwrap();

        store
            .submit_report(SubmitReportCommand {
                actor: seeded.mechanic,
                work_order_id: created.id,
                result_type: WorkResultType::Completed,
                diagnosis: "Hose fitting loosened under load".to_owned(),
                action_taken: "Retightened fitting and pressure-tested".to_owned(),
                trace: TraceContext::generate(),
                occurred_at: OffsetDateTime::now_utc(),
            })
            .await
            .unwrap();

        let settlement = store
            .create_settlement(CreateSettlementCommand {
                actor: seeded.admin,
                work_order_id: created.id,
                lines: vec![SettlementLineInput {
                    kind: SettlementLineKind::Labor,
                    label: "Labor".to_owned(),
                    amount_krw: 150_000,
                    source_ref: None,
                    sort_order: None,
                }],
                note: None,
                idempotency_key: format!("ls3-{}", uuid::Uuid::new_v4()),
                request_hash: "a".repeat(64),
                require_actor_assignment: false,
                trace: TraceContext::generate(),
                occurred_at: OffsetDateTime::now_utc(),
            })
            .await
            .unwrap();

        store
            .submit_settlement(SubmitSettlementCommand {
                actor: seeded.admin,
                settlement_id: settlement.id,
                trace: TraceContext::generate(),
                occurred_at: OffsetDateTime::now_utc(),
            })
            .await
            .unwrap();

        let plan = store
            .create_daily_plan(CreateDailyPlanCommand {
                actor: seeded.mechanic,
                branch_id: seeded.branch_id,
                mechanic_id: seeded.mechanic,
                plan_date: date!(2026 - 08 - 12),
                items: vec![DailyPlanItemInput {
                    work_order_id: created.id,
                    description: "Repair hydraulic leak".to_owned(),
                }],
                trace: TraceContext::generate(),
                occurred_at: OffsetDateTime::now_utc(),
            })
            .await
            .unwrap();

        store
            .request_daily_plan_review(SendDailyPlanForReviewCommand {
                actor: seeded.mechanic,
                plan_id: plan.id,
                trace: TraceContext::generate(),
                occurred_at: OffsetDateTime::now_utc(),
            })
            .await
            .unwrap();

        store
            .review_daily_plan(ReviewDailyPlanCommand {
                actor: seeded.admin,
                plan_id: plan.id,
                decision: DailyPlanStatus::Approved,
                memo: Some("Plan accepted".to_owned()),
                trace: TraceContext::generate(),
                occurred_at: OffsetDateTime::now_utc(),
            })
            .await
            .unwrap();

        let targets = vec![
            created.id.to_string(),
            settlement.id.to_string(),
            plan.id.to_string(),
        ];
        let rows: Vec<(String, Option<uuid::Uuid>)> = sqlx::query_as(
            r#"
            SELECT action, org_id
            FROM audit_events
            WHERE target_id = ANY($1)
            ORDER BY occurred_at, created_at
            "#,
        )
        .bind(&targets)
        .fetch_all(&pool)
        .await
        .unwrap();

        let expected_actions = [
            "work_order.create",
            "work_order.priority",
            "work_order.update_intake",
            "target_change.request",
            "target_change.review",
            "work_order.assign",
            "work_order.start",
            "work_order.report",
            "work_order_settlement.create",
            "work_order_settlement.submit",
            "daily_plan.create",
            "daily_plan.request",
            "daily_plan.review",
        ];
        let seen: Vec<&str> = rows.iter().map(|(action, _)| action.as_str()).collect();
        for expected in expected_actions {
            assert!(
                seen.contains(&expected),
                "expected audit action {expected} to be recorded (got {seen:?})"
            );
        }
        for (action, org_id) in &rows {
            assert_eq!(
                *org_id,
                Some(*tenant.as_uuid()),
                "audit row {action} must carry the acting tenant's org, not a hardcoded default"
            );
        }
    })
    .await;
}
