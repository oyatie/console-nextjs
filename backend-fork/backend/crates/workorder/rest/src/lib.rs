//! Work-order REST API.
//!
//! This layer handles JWT authentication, branch-scoped authorization, and
//! HTTP error mapping. State changes are delegated to the Postgres adapter,
//! which writes through `with_audit`.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

mod openapi;
pub use openapi::OPENAPI_FRAGMENT;

use std::collections::{BTreeMap, BTreeSet};
use std::str::FromStr;

use axum::extract::{Path, Query, RawQuery, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, patch, post, put};
use axum::{Json, Router};
use console_kernel_core::{
    AuditAction, AuditEvent, BranchId, BranchScope, DailyPlanId, DeviceId, ErrorKind, EvidenceId,
    KernelError, OrgId, TraceContext, UserId, WorkOrderId,
};
use console_platform_auth::{
    JwtVerifier, MobilePasskeyStepUpBinding, MobilePasskeyStepUpEnvelope,
    MobilePasskeyStepUpVerificationError, PasskeyService,
};
use console_platform_authz::{
    Action, BranchColumn, Feature, PermissionLevel, Principal, Role, authorize,
    authorize_capability, permission_for,
};
use console_platform_db::{DbError, with_audit, with_org_conn};
use console_platform_jobs::{JobQueue, JobRequest};
use console_platform_request_context::current_org;
use console_platform_storage::{
    EvidenceMedia, EvidenceService, EvidenceUploadCommand, EvidenceUploadTicket, MediaKind,
    PresignedUpload, ProcessingStatus, S3ObjectStore, StagingUploadCommand, StorageError,
    WormReplicaStatus,
};
use console_workflow_runtime_adapter_postgres::PgWorkflowRuntimeStore;
use console_workorder_adapter_postgres::{PgWorkOrderError, PgWorkOrderStore};
use console_workorder_application::{
    AssignmentInput, CreateDailyPlanCommand, CreateOutsourceWorkCommand, CreateSettlementCommand,
    CreateWorkOrderCommand, DailyPlanItemInput, DailyPlanListQuery, DailyPlanStatus,
    DailyPlanSummary, RejectWorkOrderCommand, ReviewDailyPlanCommand, ReviewSettlementCommand,
    ReviewTargetChangeCommand, SendDailyPlanForReviewCommand, SettlementLineInput,
    SettlementReviewDecision, SettlementSummary, SubmitReportCommand, SubmitSettlementCommand,
    TargetChangeDecision, TargetChangeRequestCommand, TargetChangeRequestSummary,
    TargetChangeStatus, UpdatePriorityCommand, UpdateWorkOrderIntakeCommand, VoidSettlementCommand,
    WorkOrderApprovalCommand, WorkOrderAssignmentCommand, WorkOrderStartCommand,
};
use console_workorder_domain::{
    AssignmentRole, AttachmentStage, MaintenanceCause, MaintenanceType, PriorityLevel,
    SettlementLineKind, WorkOrderStatus, WorkResultType,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Postgres, QueryBuilder, Row};
use std::sync::Arc;
use time::format_description::well_known::Rfc3339;

// ISO calendar-date (`YYYY-MM-DD`) serde for `time::Date` request fields. The
// default `time::Date` serde shape is a structured array, which mismatches the
// OpenAPI `Date` contract (`type: string, format: date`) and rejects the ISO
// strings the web/mobile clients send. This module aligns the wire format.
time::serde::format_description!(iso_date, Date, "[year]-[month]-[day]");

/// M2 workflow-runtime strangler seam. `pub` so the app's outbox drain worker can
/// call the crash-recovery reconciler ([`m2_strangler::reconcile_completion_tails`])
/// and so the completion tail can be driven directly in integration tests; the REST
/// entry (`drive_completion_if_enabled`) stays crate-private.
pub mod m2_strangler;
pub mod workflow_triggers;

pub const APPROVAL_ITEMS_PATH: &str = "/api/approval-items";
pub const WORKORDERS_V1_PATH: &str = "/api/v1/work-orders";
pub const WORKORDER_DETAIL_V1_PATH_TEMPLATE: &str = "/api/v1/work-orders/{work_order_id}";
pub const WORKORDER_REJECT_V1_PATH_TEMPLATE: &str = "/api/v1/work-orders/{work_order_id}/reject";
pub const EQUIPMENT_LOOKUP_PATH: &str = "/api/v1/equipment/lookup";
pub const EQUIPMENT_AUTOCOMPLETE_PATH: &str = "/api/v1/equipment";
pub const WORKORDERS_PATH: &str = "/api/work-orders";
pub const WORKORDER_PATH_TEMPLATE: &str = "/api/work-orders/{work_order_id}";
pub const WORKORDER_PRIORITY_PATH_TEMPLATE: &str = "/api/work-orders/{work_order_id}/priority";
pub const WORKORDER_ASSIGNMENTS_PATH_TEMPLATE: &str =
    "/api/work-orders/{work_order_id}/assignments";
pub const WORKORDER_START_PATH_TEMPLATE: &str = "/api/work-orders/{work_order_id}/start";
pub const WORKORDER_REPORT_PATH_TEMPLATE: &str = "/api/work-orders/{work_order_id}/report";
pub const WORKORDER_APPROVE_PATH_TEMPLATE: &str = "/api/work-orders/{work_order_id}/approve";
pub const WORKORDER_TARGET_CHANGE_REQUESTS_PATH_TEMPLATE: &str =
    "/api/work-orders/{work_order_id}/target-change-requests";
pub const TARGET_CHANGE_REVIEW_PATH_TEMPLATE: &str =
    "/api/target-change-requests/{request_id}/review";
pub const DAILY_WORK_PLANS_PATH: &str = "/api/daily-work-plans";
pub const DAILY_WORK_PLAN_PATH_TEMPLATE: &str = "/api/daily-work-plans/{plan_id}";
pub const DAILY_WORK_PLAN_REQUEST_REVIEW_PATH_TEMPLATE: &str =
    "/api/daily-work-plans/{plan_id}/request-review";
pub const DAILY_WORK_PLAN_REVIEW_PATH_TEMPLATE: &str = "/api/daily-work-plans/{plan_id}/review";
pub const DAILY_WORK_PLAN_CONFIRM_PATH_TEMPLATE: &str = "/api/daily-work-plans/{plan_id}/confirm";
pub const WORKORDER_OUTSOURCE_WORKS_PATH_TEMPLATE: &str =
    "/api/work-orders/{work_order_id}/outsource-works";
pub const WORKORDER_SETTLEMENT_V1_PATH_TEMPLATE: &str =
    "/api/v1/work-orders/{work_order_id}/settlement";
pub const SETTLEMENT_SUBMIT_V1_PATH_TEMPLATE: &str = "/api/v1/settlements/{settlement_id}/submit";
pub const SETTLEMENT_REVIEW_V1_PATH_TEMPLATE: &str = "/api/v1/settlements/{settlement_id}/review";
pub const SETTLEMENT_VOID_V1_PATH_TEMPLATE: &str = "/api/v1/settlements/{settlement_id}/void";
pub const SYNC_PATH: &str = "/api/v1/sync";
pub const EVIDENCE_PRESIGN_PATH: &str = "/api/v1/evidence/presign";
pub const EVIDENCE_CONFIRM_PATH_TEMPLATE: &str = "/api/v1/evidence/{evidenceId}/confirm";
/// Media-processing staging-upload presign: the mechanic PUTs the ORIGINAL to a
/// tenant-scoped staging key and a PROCESSING row + transcode job are created.
pub const EVIDENCE_STAGING_PRESIGN_PATH: &str = "/api/v1/evidence/staging-presign";
/// Per-row processing-status poll (처리 중 → 완료 / 실패) for the web UI.
pub const EVIDENCE_STATUS_PATH_TEMPLATE: &str = "/api/v1/evidence/{evidenceId}/status";
pub const DEVICES_PATH: &str = "/api/v1/devices";
pub const MOBILE_APPROVE_WORK_ORDER_PATH_TEMPLATE: &str =
    "/api/v1/mobile/work-orders/{work_order_id}/approve";
pub const WORKORDER_ROUTE_PATHS: &[&str] = &[
    APPROVAL_ITEMS_PATH,
    WORKORDERS_V1_PATH,
    WORKORDER_DETAIL_V1_PATH_TEMPLATE,
    WORKORDER_REJECT_V1_PATH_TEMPLATE,
    EQUIPMENT_LOOKUP_PATH,
    EQUIPMENT_AUTOCOMPLETE_PATH,
    WORKORDERS_PATH,
    WORKORDER_PATH_TEMPLATE,
    WORKORDER_PRIORITY_PATH_TEMPLATE,
    WORKORDER_ASSIGNMENTS_PATH_TEMPLATE,
    WORKORDER_START_PATH_TEMPLATE,
    WORKORDER_REPORT_PATH_TEMPLATE,
    WORKORDER_APPROVE_PATH_TEMPLATE,
    WORKORDER_TARGET_CHANGE_REQUESTS_PATH_TEMPLATE,
    TARGET_CHANGE_REVIEW_PATH_TEMPLATE,
    DAILY_WORK_PLANS_PATH,
    DAILY_WORK_PLAN_PATH_TEMPLATE,
    DAILY_WORK_PLAN_REQUEST_REVIEW_PATH_TEMPLATE,
    DAILY_WORK_PLAN_REVIEW_PATH_TEMPLATE,
    DAILY_WORK_PLAN_CONFIRM_PATH_TEMPLATE,
    WORKORDER_OUTSOURCE_WORKS_PATH_TEMPLATE,
    WORKORDER_SETTLEMENT_V1_PATH_TEMPLATE,
    SETTLEMENT_SUBMIT_V1_PATH_TEMPLATE,
    SETTLEMENT_REVIEW_V1_PATH_TEMPLATE,
    SETTLEMENT_VOID_V1_PATH_TEMPLATE,
];
pub const MOBILE_ROUTE_PATHS: &[&str] = &[
    SYNC_PATH,
    EVIDENCE_PRESIGN_PATH,
    EVIDENCE_CONFIRM_PATH_TEMPLATE,
    EVIDENCE_STAGING_PRESIGN_PATH,
    EVIDENCE_STATUS_PATH_TEMPLATE,
    DEVICES_PATH,
    MOBILE_APPROVE_WORK_ORDER_PATH_TEMPLATE,
];

/// Maximum number of operations accepted in a single `/sync` batch. A larger
/// batch is rejected (422) before any allocation or replay so a single
/// principal cannot monopolize a pooled DB connection with a long sequential
/// replay.
pub const MAX_SYNC_BATCH_OPERATIONS: usize = 200;

#[derive(Debug, Clone)]
pub struct WorkOrderRestState {
    store: PgWorkOrderStore,
    jwt_verifier: Option<JwtVerifier>,
    /// M2 workflow-runtime store for the completion strangler (design §Strangler).
    /// `None` disables the runtime path entirely (pure legacy). Even when `Some`,
    /// the per-tenant `workflow_runtime_m2_strangler` flag gates it, and it ships
    /// dark, so production stays on the legacy path byte-for-byte.
    workflow_runtime: Option<PgWorkflowRuntimeStore>,
}

impl WorkOrderRestState {
    #[must_use]
    pub fn new(store: PgWorkOrderStore, jwt_verifier: Option<JwtVerifier>) -> Self {
        Self {
            store,
            jwt_verifier,
            workflow_runtime: None,
        }
    }

    /// Attach the M2 workflow-runtime store that backs the completion strangler.
    #[must_use]
    pub fn with_workflow_runtime(
        mut self,
        workflow_runtime: Option<PgWorkflowRuntimeStore>,
    ) -> Self {
        self.workflow_runtime = workflow_runtime;
        self
    }
}

#[derive(Clone)]
pub struct MobileRestState<S> {
    pool: PgPool,
    store: PgWorkOrderStore,
    jwt_verifier: Option<JwtVerifier>,
    passkey_step_up: Option<PasskeyService>,
    workflow_runtime: Option<PgWorkflowRuntimeStore>,
    evidence_service: Option<EvidenceService<S>>,
    /// Job queue used to enqueue the async evidence transcode after a staging
    /// upload presign. `None` disables the media-processing path (the staging
    /// presign endpoint then returns 503).
    job_queue: Option<Arc<dyn JobQueue>>,
}

impl<S> MobileRestState<S> {
    #[must_use]
    pub fn new(
        pool: PgPool,
        store: PgWorkOrderStore,
        jwt_verifier: Option<JwtVerifier>,
        evidence_service: Option<EvidenceService<S>>,
    ) -> Self {
        Self {
            pool,
            store,
            jwt_verifier,
            passkey_step_up: None,
            workflow_runtime: None,
            evidence_service,
            job_queue: None,
        }
    }

    #[must_use]
    pub fn with_passkey_step_up(mut self, passkey_step_up: Option<PasskeyService>) -> Self {
        self.passkey_step_up = passkey_step_up;
        self
    }

    #[must_use]
    pub fn with_workflow_runtime(
        mut self,
        workflow_runtime: Option<PgWorkflowRuntimeStore>,
    ) -> Self {
        self.workflow_runtime = workflow_runtime;
        self
    }

    /// Attach the job queue that backs the async evidence-transcode pipeline.
    #[must_use]
    pub fn with_job_queue(mut self, job_queue: Option<Arc<dyn JobQueue>>) -> Self {
        self.job_queue = job_queue;
        self
    }
}

pub fn router(state: WorkOrderRestState) -> Router {
    let verifier = state.jwt_verifier.clone();
    let pool = state.store.pool().clone();
    let router = Router::new()
        .route(APPROVAL_ITEMS_PATH, get(list_approval_items))
        .route(WORKORDERS_V1_PATH, get(list_work_orders))
        .route(
            WORKORDER_DETAIL_V1_PATH_TEMPLATE,
            get(get_work_order_detail),
        )
        .route(WORKORDER_REJECT_V1_PATH_TEMPLATE, post(reject_work_order))
        .route(EQUIPMENT_LOOKUP_PATH, get(lookup_equipment))
        .route(EQUIPMENT_AUTOCOMPLETE_PATH, get(autocomplete_equipment))
        .route(WORKORDERS_PATH, post(create_work_order))
        .route(
            WORKORDER_PATH_TEMPLATE,
            get(get_work_order).patch(update_work_order_intake),
        )
        .route(WORKORDER_PRIORITY_PATH_TEMPLATE, patch(update_priority))
        .route(WORKORDER_ASSIGNMENTS_PATH_TEMPLATE, put(assign_work_order))
        .route(WORKORDER_START_PATH_TEMPLATE, post(start_work))
        .route(WORKORDER_REPORT_PATH_TEMPLATE, post(submit_report))
        .route(WORKORDER_APPROVE_PATH_TEMPLATE, post(approve_work_order))
        .route(
            WORKORDER_TARGET_CHANGE_REQUESTS_PATH_TEMPLATE,
            post(request_target_change),
        )
        .route(
            TARGET_CHANGE_REVIEW_PATH_TEMPLATE,
            post(review_target_change),
        )
        .route(
            DAILY_WORK_PLANS_PATH,
            get(list_daily_plans).post(create_daily_plan),
        )
        .route(DAILY_WORK_PLAN_PATH_TEMPLATE, get(get_daily_plan))
        .route(
            DAILY_WORK_PLAN_REQUEST_REVIEW_PATH_TEMPLATE,
            post(request_daily_plan_review),
        )
        .route(
            DAILY_WORK_PLAN_REVIEW_PATH_TEMPLATE,
            post(review_daily_plan),
        )
        .route(
            DAILY_WORK_PLAN_CONFIRM_PATH_TEMPLATE,
            post(confirm_daily_plan),
        )
        .route(
            WORKORDER_OUTSOURCE_WORKS_PATH_TEMPLATE,
            post(create_outsource_work),
        )
        .route(
            WORKORDER_SETTLEMENT_V1_PATH_TEMPLATE,
            get(get_settlement).post(create_settlement),
        )
        .route(SETTLEMENT_SUBMIT_V1_PATH_TEMPLATE, post(submit_settlement))
        .route(SETTLEMENT_REVIEW_V1_PATH_TEMPLATE, post(review_settlement))
        .route(SETTLEMENT_VOID_V1_PATH_TEMPLATE, post(void_settlement))
        .with_state(state);
    console_platform_request_context::with_request_context(router, verifier, pool)
}

pub fn mobile_router<S>(state: MobileRestState<S>) -> Router
where
    S: S3ObjectStore + Clone + Send + Sync + 'static,
{
    let verifier = state.jwt_verifier.clone();
    let pool = state.pool.clone();
    let router = Router::new()
        .route(SYNC_PATH, post(sync_batch::<S>))
        .route(DEVICES_PATH, post(register_device::<S>))
        .route(EVIDENCE_PRESIGN_PATH, post(presign_evidence::<S>))
        .route(EVIDENCE_CONFIRM_PATH_TEMPLATE, post(confirm_evidence::<S>))
        .route(
            EVIDENCE_STAGING_PRESIGN_PATH,
            post(presign_evidence_staging::<S>),
        )
        .route(
            MOBILE_APPROVE_WORK_ORDER_PATH_TEMPLATE,
            post(approve_mobile_work_order::<S>),
        )
        .route(EVIDENCE_STATUS_PATH_TEMPLATE, get(evidence_status::<S>))
        .with_state(state);
    console_platform_request_context::with_request_context(router, verifier, pool)
}

#[derive(Debug, Deserialize)]
struct CreateWorkOrderRequest {
    branch_id: BranchId,
    management_no: String,
    symptom: String,
    customer_request: Option<String>,
    #[serde(default, with = "time::serde::rfc3339::option")]
    target_due_at: Option<time::OffsetDateTime>,
    /// Typed maintenance classification (유형). Optional on the wire so legacy
    /// mobile callers stay valid; the console intake requires it client-side.
    #[serde(default)]
    maintenance_type: Option<MaintenanceType>,
    /// Typed maintenance cause (원인). Optional on the wire.
    #[serde(default)]
    maintenance_cause: Option<MaintenanceCause>,
}

#[derive(Debug, Deserialize)]
struct UpdateWorkOrderIntakeRequest {
    symptom: Option<String>,
    customer_request: Option<String>,
    #[serde(default)]
    maintenance_type: Option<MaintenanceType>,
    #[serde(default)]
    maintenance_cause: Option<MaintenanceCause>,
}

#[derive(Debug, Serialize, Deserialize)]
struct SettlementLineRequest {
    kind: SettlementLineKind,
    label: String,
    amount_krw: i64,
    #[serde(default)]
    source_ref: Option<String>,
    #[serde(default)]
    sort_order: Option<i32>,
}

/// `Serialize` is derived so the accepted body can be re-serialized into the
/// canonical form that is hashed for `Idempotency-Key` replay comparison.
#[derive(Debug, Serialize, Deserialize)]
struct CreateSettlementRequest {
    lines: Vec<SettlementLineRequest>,
    #[serde(default)]
    note: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ReviewSettlementRequest {
    decision: SettlementReviewDecision,
    #[serde(default)]
    comment: Option<String>,
}

#[derive(Debug, Deserialize)]
struct VoidSettlementRequest {
    reason: String,
}

#[derive(Debug, Deserialize)]
struct UpdatePriorityRequest {
    priority: PriorityLevel,
}

#[derive(Debug, Deserialize)]
struct AssignmentRequest {
    mechanic_id: UserId,
    role: AssignmentRole,
}

#[derive(Debug, Deserialize)]
struct AssignWorkOrderRequest {
    assignments: Vec<AssignmentRequest>,
    admin_approver_id: Option<UserId>,
    executive_approver_id: Option<UserId>,
}

#[derive(Debug, Deserialize)]
struct SubmitReportRequest {
    result_type: WorkResultType,
    diagnosis: String,
    action_taken: String,
}

#[derive(Debug, Deserialize)]
struct ApproveWorkOrderRequest {
    comment: String,
}

#[derive(Debug, Deserialize)]
struct MobileApproveWorkOrderRequest {
    comment: String,
    #[serde(default)]
    step_up: Option<MobilePasskeyStepUpEnvelope>,
}

#[derive(Debug, Deserialize)]
struct TargetChangeRequestBody {
    // The OpenAPI contract types this as an rfc3339 `string`; a bare OffsetDateTime
    // serde impl expects the array form and rejects the ISO instant the web client
    // sends (422). Deserialize it from the rfc3339 string instead.
    #[serde(with = "time::serde::rfc3339")]
    requested_target_due_at: time::OffsetDateTime,
    reason: String,
}

#[derive(Debug, Deserialize)]
struct ReviewTargetChangeRequestBody {
    decision: TargetChangeDecision,
    memo: Option<String>,
}

#[derive(Debug, Deserialize)]
struct DailyPlanItemRequest {
    work_order_id: WorkOrderId,
    description: String,
}

#[derive(Debug, Deserialize)]
struct CreateDailyPlanRequest {
    branch_id: BranchId,
    mechanic_id: UserId,
    #[serde(with = "iso_date")]
    plan_date: time::Date,
    items: Vec<DailyPlanItemRequest>,
}

#[derive(Debug, Deserialize)]
struct ReviewDailyPlanRequestBody {
    decision: DailyPlanStatus,
    memo: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct ListDailyPlansQuery {
    /// Optional `YYYY-MM-DD` filter for a single plan day; absent = all days.
    plan_date: Option<String>,
}

#[derive(Debug, Serialize)]
struct DailyPlanListResponse {
    items: Vec<DailyPlanSummary>,
}

#[derive(Debug, Default, Deserialize)]
struct ApprovalItemsQuery {
    limit: Option<i64>,
    offset: Option<i64>,
}

#[derive(Debug)]
struct NormalizedApprovalItemsQuery {
    limit: i64,
    offset: i64,
}

#[derive(Debug, Clone, Copy)]
struct ApprovalSourceVisibility {
    work_orders: bool,
    daily_plans: bool,
    target_changes: bool,
}

impl ApprovalSourceVisibility {
    fn any(self) -> bool {
        self.work_orders || self.daily_plans || self.target_changes
    }
}

#[derive(Debug, Serialize)]
struct ApprovalItemsPage {
    items: Vec<ApprovalItem>,
    sources: Vec<ApprovalItemSource>,
    limit: i64,
    offset: i64,
    total: i64,
}

#[derive(Debug, Serialize)]
struct ApprovalItemSource {
    key: &'static str,
    label: &'static str,
    status: &'static str,
    count: i64,
}

#[derive(Debug, Serialize)]
struct ApprovalItem {
    /// Stable federated id (`{source}:{source_id}`) for UI selection, audit
    /// correlation, and future object-activity references.
    id: String,
    source: String,
    source_id: uuid::Uuid,
    branch_id: BranchId,
    status: String,
    title: String,
    summary: String,
    #[serde(with = "time::serde::rfc3339::option")]
    requested_at: Option<time::OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339::option")]
    due_at: Option<time::OffsetDateTime>,
    href: String,
    action_href: String,
    ontology: ApprovalOntologyContext,
    workflow: ApprovalWorkflowContext,
    policy: ApprovalPolicyContext,
    #[serde(skip_serializing_if = "Option::is_none")]
    work_order: Option<WorkOrderListItem>,
    #[serde(skip_serializing_if = "Option::is_none")]
    daily_plan: Option<DailyPlanSummary>,
    #[serde(skip_serializing_if = "Option::is_none")]
    target_change: Option<TargetChangeRequestSummary>,
}

#[derive(Debug, Serialize)]
struct ApprovalOntologyContext {
    object_type: String,
    object_id: uuid::Uuid,
    tenant_id: OrgId,
    branch_id: BranchId,
}

#[derive(Debug, Serialize)]
struct ApprovalWorkflowContext {
    workflow_key: String,
    action_key: String,
}

#[derive(Debug, Serialize)]
struct ApprovalPolicyContext {
    decision: &'static str,
    enforcement: &'static str,
    required_features: Vec<&'static str>,
    scope_kind: &'static str,
    scope_id: uuid::Uuid,
}

#[derive(Debug, Deserialize)]
struct CreateOutsourceWorkRequest {
    vendor_name: String,
    vendor_contact: Option<String>,
    reason: String,
}

#[derive(Debug, Default)]
struct WorkOrderListQuery {
    status: Vec<String>,
    priority: Vec<String>,
    maintenance_type: Vec<String>,
    maintenance_cause: Vec<String>,
    assigned_to: Option<String>,
    customer_id: Option<uuid::Uuid>,
    site_id: Option<uuid::Uuid>,
    branch_id: Option<uuid::Uuid>,
    equipment_id: Option<uuid::Uuid>,
    around_work_order_id: Option<uuid::Uuid>,
    target_due_from: Option<time::OffsetDateTime>,
    target_due_to: Option<time::OffsetDateTime>,
    limit: Option<i64>,
    offset: Option<i64>,
}

#[derive(Debug, Clone)]
struct NormalizedWorkOrderListQuery {
    statuses: Vec<String>,
    priorities: Vec<String>,
    maintenance_types: Vec<String>,
    maintenance_causes: Vec<String>,
    assigned_to: Option<UserId>,
    customer_id: Option<uuid::Uuid>,
    site_id: Option<uuid::Uuid>,
    branch_id: Option<uuid::Uuid>,
    equipment_id: Option<uuid::Uuid>,
    around_work_order_id: Option<uuid::Uuid>,
    target_due_from: Option<time::OffsetDateTime>,
    target_due_to: Option<time::OffsetDateTime>,
    limit: i64,
    offset: i64,
}

#[derive(Debug, Deserialize)]
struct EquipmentLookupQuery {
    management_no: String,
}

#[derive(Debug, Deserialize)]
struct EquipmentAutocompleteQuery {
    q: String,
    limit: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct RejectWorkOrderRequest {
    memo: String,
}

#[derive(Debug, Serialize)]
struct WorkOrderListPage {
    items: Vec<WorkOrderListItem>,
    limit: i64,
    offset: i64,
    total: i64,
    lens: WorkOrderObjectSetLens,
}

#[derive(Debug, Serialize)]
struct WorkOrderListItem {
    id: WorkOrderId,
    request_no: String,
    branch_id: BranchId,
    status: String,
    priority: String,
    result_type: String,
    maintenance_type: Option<String>,
    maintenance_cause: Option<String>,
    #[serde(with = "time::serde::rfc3339::option")]
    target_due_at: Option<time::OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339")]
    created_at: time::OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    updated_at: time::OffsetDateTime,
    equipment: EquipmentSummary,
    customer: NamedEntity,
    site: NamedEntity,
    // Site's registered representative contact (#13), so the dispatch board can
    // show who to call on site. None when the site has no contact registered.
    site_contact: Option<SiteContact>,
    assignments: Vec<AssignmentSummary>,
}

#[derive(Debug, Serialize)]
struct WorkOrderDetail {
    id: WorkOrderId,
    request_no: String,
    branch_id: BranchId,
    status: String,
    priority: String,
    result_type: String,
    maintenance_type: Option<String>,
    maintenance_cause: Option<String>,
    symptom: String,
    customer_request: Option<String>,
    #[serde(with = "time::serde::rfc3339::option")]
    target_due_at: Option<time::OffsetDateTime>,
    delay_reason: Option<String>,
    delay_note: Option<String>,
    diagnosis: Option<String>,
    action_taken: Option<String>,
    report_submitted_by: Option<UserId>,
    #[serde(with = "time::serde::rfc3339::option")]
    report_submitted_at: Option<time::OffsetDateTime>,
    kpi_excluded: bool,
    evidence_verified: bool,
    #[serde(with = "time::serde::rfc3339")]
    created_at: time::OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    updated_at: time::OffsetDateTime,
    equipment: EquipmentSummary,
    customer: NamedEntity,
    site: NamedEntity,
    // The site's registered representative contact (대표 담당자 연락처, #13), so the
    // dispatched mechanic/admin can see who to call on site. None when the site
    // has no contact registered.
    site_contact: Option<SiteContact>,
    assignments: Vec<AssignmentSummary>,
    approval_line: Vec<ApprovalStepSummary>,
    status_history: Vec<StatusHistorySummary>,
    evidence: Vec<EvidenceSummary>,
    /// The live (non-VOID) cost settlement of this order, if one exists.
    settlement: Option<SettlementSummary>,
}

#[derive(Debug, Clone, Serialize)]
struct EquipmentSummary {
    id: uuid::Uuid,
    equipment_no: String,
    management_no: Option<String>,
    model: Option<String>,
    status: String,
    specification: String,
    ton_text: String,
}

#[derive(Debug, Clone, Serialize)]
struct NamedEntity {
    id: uuid::Uuid,
    name: String,
}

#[derive(Debug, Clone, Serialize)]
struct SiteContact {
    name: Option<String>,
    phone: Option<String>,
    email: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct AssignmentSummary {
    id: uuid::Uuid,
    mechanic_id: UserId,
    mechanic_name: String,
    role: String,
    #[serde(with = "time::serde::rfc3339")]
    assigned_at: time::OffsetDateTime,
}

#[derive(Debug, Serialize)]
struct WorkOrderObjectSetLens {
    object_type: &'static str,
    aggregates: WorkOrderLensAggregates,
    facets: WorkOrderLensFacets,
    histograms: WorkOrderLensHistograms,
    listograms: WorkOrderLensListograms,
}

#[derive(Debug, Serialize)]
struct WorkOrderLensAggregates {
    total_count: i64,
    p1_count: i64,
    overdue_open_count: i64,
    unassigned_count: i64,
    /// Share (0..=1) of closed preventive orders that reached FINAL_COMPLETED
    /// no later than their target due date. `null` when the filtered set has no
    /// closed preventive order with a target — never a fabricated 0 or 100.
    preventive_on_time_rate: Option<f64>,
    /// Mean minutes from the first IN_PROGRESS transition to the first
    /// REPORT_SUBMITTED transition (OT-13 MTTR). `null` when no order in the
    /// filtered set has completed that span.
    mttr_minutes: Option<f64>,
}

#[derive(Debug, Serialize)]
struct WorkOrderLensFacets {
    status: Vec<WorkOrderFacetBucket>,
    priority: Vec<WorkOrderFacetBucket>,
    maintenance_type: Vec<WorkOrderFacetBucket>,
    maintenance_cause: Vec<WorkOrderFacetBucket>,
}

#[derive(Debug, Serialize)]
struct WorkOrderFacetBucket {
    value: String,
    count: i64,
    filters: BTreeMap<String, String>,
}

#[derive(Debug, Serialize)]
struct WorkOrderLensHistograms {
    target_due_date: Vec<WorkOrderHistogramBucket>,
}

#[derive(Debug, Serialize)]
struct WorkOrderHistogramBucket {
    bucket: String,
    count: i64,
    filters: BTreeMap<String, String>,
}

#[derive(Debug, Serialize)]
struct WorkOrderLensListograms {
    customers: Vec<WorkOrderNamedBucket>,
    sites: Vec<WorkOrderNamedBucket>,
}

#[derive(Debug, Serialize)]
struct WorkOrderNamedBucket {
    id: uuid::Uuid,
    name: String,
    count: i64,
    filters: BTreeMap<String, String>,
}

struct WorkOrderNamedListogramSpec {
    table: &'static str,
    alias: &'static str,
    join_column: &'static str,
    filter_key: &'static str,
}

#[derive(Debug, Serialize)]
struct ApprovalStepSummary {
    id: uuid::Uuid,
    step_order: i16,
    role: String,
    approver_id: Option<UserId>,
    approver_name: Option<String>,
    status: String,
    #[serde(with = "time::serde::rfc3339::option")]
    requested_at: Option<time::OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339::option")]
    approved_at: Option<time::OffsetDateTime>,
    approved_by_id: Option<UserId>,
    approved_by_name: Option<String>,
    decision_comment: Option<String>,
}

#[derive(Debug, Serialize)]
struct StatusHistorySummary {
    id: uuid::Uuid,
    actor: Option<UserId>,
    action: String,
    from_status: Option<String>,
    to_status: String,
    #[serde(with = "time::serde::rfc3339")]
    occurred_at: time::OffsetDateTime,
}

#[derive(Debug, Serialize)]
struct EvidenceSummary {
    id: EvidenceId,
    stage: String,
    content_type: String,
    size_bytes: i64,
    uploaded_by: UserId,
    worm_replica_status: String,
    retry_count: i32,
    #[serde(with = "time::serde::rfc3339::option")]
    verified_at: Option<time::OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339")]
    created_at: time::OffsetDateTime,
}

#[derive(Debug, Serialize)]
struct EquipmentAutocompletePage {
    items: Vec<EquipmentLookupResponse>,
    limit: i64,
}

#[derive(Debug, Serialize)]
struct EquipmentLookupResponse {
    id: uuid::Uuid,
    branch_id: BranchId,
    equipment_no: String,
    management_no: Option<String>,
    model: Option<String>,
    status: String,
    specification: String,
    ton_text: String,
    maker: Option<String>,
    vin: Option<String>,
    vehicle_registration_no: Option<String>,
    customer: NamedEntity,
    site: NamedEntity,
}

#[derive(Debug, Deserialize)]
struct SyncBatchRequest {
    sync_id: String,
    operations: Vec<SyncOperationRequest>,
}

#[derive(Debug, Clone, Deserialize)]
struct SyncOperationRequest {
    request_id: String,
    operation: SyncOperationKind,
    #[serde(with = "time::serde::rfc3339")]
    created_at: time::OffsetDateTime,
    payload: serde_json::Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
enum SyncOperationKind {
    WorkOrderStart,
    WorkOrderReport,
}

impl SyncOperationKind {
    const fn as_db_str(self) -> &'static str {
        match self {
            Self::WorkOrderStart => "WORK_ORDER_START",
            Self::WorkOrderReport => "WORK_ORDER_REPORT",
        }
    }
}

#[derive(Debug, Deserialize)]
struct SyncWorkOrderStartPayload {
    work_order_id: WorkOrderId,
}

#[derive(Debug, Deserialize)]
struct SyncWorkOrderReportPayload {
    work_order_id: WorkOrderId,
    result_type: WorkResultType,
    diagnosis: String,
    action_taken: String,
}

#[derive(Debug, Serialize)]
struct SyncBatchResponse {
    sync_id: String,
    results: Vec<SyncOperationResult>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SyncOperationResult {
    request_id: String,
    operation: SyncOperationKind,
    status: SyncOperationStatus,
    http_status: u16,
    result: Option<serde_json::Value>,
    error: Option<SyncErrorPayload>,
    replayed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
enum SyncOperationStatus {
    Applied,
    Failed,
}

impl SyncOperationStatus {
    const fn as_db_str(self) -> &'static str {
        match self {
            Self::Applied => "APPLIED",
            Self::Failed => "FAILED",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SyncErrorPayload {
    code: String,
    message: String,
}

#[derive(Debug, Deserialize)]
struct DeviceRegistrationRequest {
    platform: DevicePlatform,
    push_token: Option<String>,
    app_version: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum DevicePlatform {
    Ios,
    Android,
}

impl DevicePlatform {
    const fn as_db_str(self) -> &'static str {
        match self {
            Self::Ios => "IOS",
            Self::Android => "ANDROID",
        }
    }

    fn from_db_str(value: &str) -> Result<Self, KernelError> {
        match value {
            "IOS" => Ok(Self::Ios),
            "ANDROID" => Ok(Self::Android),
            other => Err(KernelError::validation(format!(
                "unknown device platform {other:?}"
            ))),
        }
    }
}

#[derive(Debug, Serialize)]
struct DeviceRegistrationResponse {
    id: DeviceId,
    user_id: UserId,
    device_hash: String,
    platform: DevicePlatform,
    push_token: Option<String>,
    app_version: String,
    #[serde(with = "time::serde::rfc3339")]
    last_registered_at: time::OffsetDateTime,
}

#[derive(Debug, Deserialize)]
struct EvidencePresignRequest {
    work_order_id: WorkOrderId,
    stage: AttachmentStage,
    content_type: String,
    size_bytes: i64,
    checksum_sha256: Option<String>,
}

#[derive(Debug, Serialize)]
struct EvidencePresignResponse {
    id: EvidenceId,
    work_order_id: WorkOrderId,
    stage: AttachmentStage,
    upload: PresignedUpload,
}

#[derive(Debug, Serialize)]
struct EvidenceConfirmResponse {
    id: EvidenceId,
    work_order_id: WorkOrderId,
    stage: AttachmentStage,
    worm_replica_status: WormReplicaStatus,
    retry_count: i32,
    // The OpenAPI contract types this as a nullable rfc3339 `string`; the bare
    // OffsetDateTime serde impl emits the numeric component array instead, which
    // clients (and the drift-checked spec) reject. Serialize it as rfc3339.
    #[serde(with = "time::serde::rfc3339::option")]
    verified_at: Option<time::OffsetDateTime>,
}

#[derive(Debug, Deserialize)]
struct EvidenceStagingPresignRequest {
    work_order_id: WorkOrderId,
    stage: AttachmentStage,
    /// The ORIGINAL upload's content type (image/* or video/*); validated and
    /// classified server-side.
    content_type: String,
    size_bytes: i64,
    checksum_sha256: Option<String>,
}

#[derive(Debug, Serialize)]
struct EvidenceStagingPresignResponse {
    id: EvidenceId,
    work_order_id: WorkOrderId,
    stage: AttachmentStage,
    media_kind: MediaKind,
    processing_status: ProcessingStatus,
    upload: PresignedUpload,
}

#[derive(Debug, Serialize)]
struct EvidenceStatusResponse {
    id: EvidenceId,
    work_order_id: WorkOrderId,
    stage: AttachmentStage,
    processing_status: ProcessingStatus,
    content_type: String,
    /// Short-lived presigned GET URL for the generated thumbnail (null until the
    /// row is READY). Replaces the raw `thumbnail_s3_key`, which leaked the
    /// internal object key; the client renders this URL directly.
    thumbnail_url: Option<String>,
    processing_error: Option<String>,
    #[serde(with = "time::serde::rfc3339::option")]
    processed_at: Option<time::OffsetDateTime>,
}

#[derive(Debug, Serialize)]
struct ErrorBody {
    error: ErrorPayload,
}

#[derive(Debug, Serialize)]
struct ErrorPayload {
    code: &'static str,
    message: String,
}

#[derive(Debug)]
struct RestError {
    status: StatusCode,
    kind: ErrorKind,
    code_override: Option<&'static str>,
    message: String,
}

impl RestError {
    fn bad_request(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            kind: ErrorKind::Validation,
            code_override: None,
            message: message.into(),
        }
    }

    fn unauthorized(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::UNAUTHORIZED,
            kind: ErrorKind::Forbidden,
            code_override: None,
            message: message.into(),
        }
    }

    fn unavailable(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::SERVICE_UNAVAILABLE,
            kind: ErrorKind::Internal,
            code_override: None,
            message: message.into(),
        }
    }

    fn internal(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            kind: ErrorKind::Internal,
            code_override: None,
            message: message.into(),
        }
    }

    fn precondition_required(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::PRECONDITION_REQUIRED,
            kind: ErrorKind::Validation,
            code_override: Some(code),
            message: message.into(),
        }
    }

    fn passkey_step_up_failed(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::UNAUTHORIZED,
            kind: ErrorKind::Forbidden,
            code_override: Some(code),
            message: message.into(),
        }
    }

    fn from_kernel(error: KernelError) -> Self {
        Self {
            status: status_for_error_kind(error.kind),
            kind: error.kind,
            code_override: None,
            message: error.message,
        }
    }

    fn from_store(error: PgWorkOrderError) -> Self {
        match error {
            PgWorkOrderError::Domain(error) => Self::from_kernel(error),
            PgWorkOrderError::Db(error) => Self::from_db(error),
        }
    }

    fn from_storage(error: StorageError) -> Self {
        match error {
            StorageError::Domain(error) => Self::from_kernel(error),
            StorageError::Db(error) => Self::from_db(error),
            StorageError::S3(message)
            | StorageError::Presign(message)
            | StorageError::Verification(message)
            | StorageError::Processing(message) => Self::internal(message),
        }
    }

    fn from_db(error: DbError) -> Self {
        match error {
            DbError::Sqlx(sqlx::Error::RowNotFound) => {
                Self::from_kernel(KernelError::not_found("row was not found"))
            }
            DbError::Sqlx(sqlx::Error::Database(err))
                if err.code().is_some_and(|code| code == "23505") =>
            {
                // Log the constraint name server-side; never leak it (schema
                // disclosure, OWASP A05). Clients get a stable generic message.
                tracing::error!(error = %err, "database unique-constraint violation");
                Self::from_kernel(KernelError::conflict("resource already exists"))
            }
            DbError::Sqlx(err) => {
                tracing::error!(error = %err, "database error");
                Self::internal("internal server error")
            }
            DbError::Serialize(err) => {
                tracing::error!(error = %err, "serialization error");
                Self::internal("internal server error")
            }
            DbError::CodeIssuance(err) => {
                tracing::error!(error = %err, "object-code issuance error");
                Self::internal("internal server error")
            }
        }
    }

    fn code(&self) -> &'static str {
        if let Some(code) = self.code_override {
            return code;
        }
        match self.kind {
            ErrorKind::Validation => "validation",
            ErrorKind::NotFound => "not_found",
            ErrorKind::Forbidden => "forbidden",
            ErrorKind::Conflict => "conflict",
            ErrorKind::InvalidTransition => "invalid_transition",
            ErrorKind::Internal => "internal",
        }
    }
}

impl IntoResponse for RestError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(ErrorBody {
                error: ErrorPayload {
                    code: self.code(),
                    message: self.message,
                },
            }),
        )
            .into_response()
    }
}

impl From<DbError> for RestError {
    fn from(value: DbError) -> Self {
        Self::from_db(value)
    }
}

impl From<sqlx::Error> for RestError {
    fn from(value: sqlx::Error) -> Self {
        Self::from_db(DbError::Sqlx(value))
    }
}

impl From<serde_json::Error> for RestError {
    fn from(value: serde_json::Error) -> Self {
        Self::internal(value.to_string())
    }
}

async fn sync_batch<S>(
    State(state): State<MobileRestState<S>>,
    headers: HeaderMap,
    Json(body): Json<SyncBatchRequest>,
) -> Result<Json<SyncBatchResponse>, RestError>
where
    S: S3ObjectStore + Clone + Send + Sync + 'static,
{
    let principal = mobile_principal_from_headers(&state, &headers).await?;
    let device_hash = device_hash_from_headers(&headers)?;
    let sync_id = normalize_non_empty(body.sync_id, "sync_id")?;
    if body.operations.is_empty() {
        return Err(RestError::from_kernel(KernelError::validation(
            "sync operations must not be empty",
        )));
    }
    // Cap the batch before allocating / replaying: one authenticated principal
    // must not be able to monopolize a pooled DB connection with an oversized
    // sequential replay.
    if body.operations.len() > MAX_SYNC_BATCH_OPERATIONS {
        return Err(RestError::from_kernel(KernelError::validation(format!(
            "sync batch exceeds the maximum of {MAX_SYNC_BATCH_OPERATIONS} operations"
        ))));
    }

    // FIX 1: a request_id must be unique within a single batch — a client that
    // reuses one for two distinct operations would otherwise have one silently
    // dropped by the idempotency cache.
    let mut seen_request_ids = BTreeSet::new();
    let mut results = Vec::with_capacity(body.operations.len());
    for operation in body.operations {
        let request_id = normalize_non_empty(operation.request_id.clone(), "request_id")?;
        if !seen_request_ids.insert(request_id.clone()) {
            results.push(SyncOperationResult {
                request_id,
                operation: operation.operation,
                status: SyncOperationStatus::Failed,
                http_status: StatusCode::CONFLICT.as_u16(),
                result: None,
                error: Some(SyncErrorPayload {
                    code: "conflict".to_owned(),
                    message: "duplicate request_id within sync batch".to_owned(),
                }),
                replayed: false,
            });
            continue;
        }
        results.push(
            replay_sync_operation(
                &state,
                &principal,
                &device_hash,
                &sync_id,
                request_id,
                operation,
            )
            .await?,
        );
    }

    Ok(Json(SyncBatchResponse { sync_id, results }))
}

/// FIX 1: build the canonical, order-stable sha256 hash that binds an
/// idempotency record to its operation content. `serde_json` sorts object keys
/// (no `preserve_order` feature), so re-serializing the envelope is canonical.
fn sync_payload_hash(
    user_id: UserId,
    sync_id: &str,
    operation: SyncOperationKind,
    client_created_at: time::OffsetDateTime,
    payload: &serde_json::Value,
) -> Result<(String, serde_json::Value), RestError> {
    let created_at = client_created_at
        .format(&Rfc3339)
        .map_err(|err| RestError::internal(format!("invalid client_created_at: {err}")))?;
    let envelope = serde_json::json!({
        "user_id": user_id.to_string(),
        "sync_id": sync_id,
        "operation_type": operation.as_db_str(),
        "client_created_at": created_at,
        "payload": payload,
    });
    let canonical = serde_json::to_vec(&envelope)?;
    let hash = hex::encode(Sha256::digest(&canonical));
    Ok((hash, envelope))
}

async fn replay_sync_operation<S>(
    state: &MobileRestState<S>,
    principal: &Principal,
    device_hash: &str,
    sync_id: &str,
    request_id: String,
    operation: SyncOperationRequest,
) -> Result<SyncOperationResult, RestError>
where
    S: S3ObjectStore + Clone + Send + Sync + 'static,
{
    let (payload_hash, payload_envelope) = sync_payload_hash(
        principal.user_id,
        sync_id,
        operation.operation,
        operation.created_at,
        &operation.payload,
    )?;
    match claim_sync_request(
        &state.pool,
        principal.user_id,
        device_hash,
        sync_id,
        &request_id,
        operation.operation,
        operation.created_at,
        &payload_hash,
        &payload_envelope,
    )
    .await?
    {
        SyncClaim::Cached(mut cached) => {
            cached.replayed = true;
            Ok(cached)
        }
        SyncClaim::PayloadMismatch => Ok(SyncOperationResult {
            request_id,
            operation: operation.operation,
            status: SyncOperationStatus::Failed,
            http_status: StatusCode::CONFLICT.as_u16(),
            result: None,
            error: Some(SyncErrorPayload {
                code: "conflict".to_owned(),
                message: "request_id reused with different operation content".to_owned(),
            }),
            replayed: true,
        }),
        SyncClaim::Claimed(sync_request_id) => {
            let outcome = execute_sync_operation(state, principal, operation).await;
            complete_sync_request(&state.pool, sync_request_id, principal.user_id, outcome).await
        }
        // FIX 2: a previously-claimed-but-never-completed row (worker crash
        // between the mutation commit and the completion mark). Re-run the
        // operation; `execute_sync_operation` is idempotent — if the mutation
        // already applied it re-derives the response from the current
        // work-order state instead of double-mutating — then finalize the row.
        SyncClaim::Stale(sync_request_id) => {
            let outcome = execute_sync_operation(state, principal, operation).await;
            let mut result =
                complete_sync_request(&state.pool, sync_request_id, principal.user_id, outcome)
                    .await?;
            result.replayed = true;
            Ok(result)
        }
    }
}

async fn execute_sync_operation<S>(
    state: &MobileRestState<S>,
    principal: &Principal,
    operation: SyncOperationRequest,
) -> PendingSyncOutcome
where
    S: S3ObjectStore + Clone + Send + Sync + 'static,
{
    let request_id = operation.request_id;
    match operation.operation {
        SyncOperationKind::WorkOrderStart => {
            let parsed = serde_json::from_value::<SyncWorkOrderStartPayload>(operation.payload)
                .map_err(|err| RestError::bad_request(format!("invalid start payload: {err}")));
            let payload = match parsed {
                Ok(payload) => payload,
                Err(error) => {
                    return PendingSyncOutcome::failure(
                        request_id,
                        operation.operation,
                        None,
                        error,
                    );
                }
            };
            let result = async {
                let work_order = state
                    .store
                    .work_order(payload.work_order_id)
                    .await
                    .map_err(RestError::from_store)?;
                authorize(
                    principal,
                    Action::new(Feature::WorkOrderStart),
                    work_order.branch_id,
                )
                .map_err(RestError::from_kernel)?;
                let summary = match state
                    .store
                    .start_work(WorkOrderStartCommand {
                        actor: principal.user_id,
                        work_order_id: payload.work_order_id,
                        trace: TraceContext::generate(),
                        occurred_at: time::OffsetDateTime::now_utc(),
                    })
                    .await
                {
                    Ok(summary) => summary,
                    // FIX 2 reconciliation: if the start already applied (a crash
                    // re-runs this), the transition guard rejects the re-run; the
                    // current state IS the result, so re-derive it idempotently.
                    Err(err) => {
                        let current = state
                            .store
                            .work_order(payload.work_order_id)
                            .await
                            .map_err(RestError::from_store)?;
                        if current.status == WorkOrderStatus::InProgress {
                            current
                        } else {
                            return Err(RestError::from_store(err));
                        }
                    }
                };
                Ok(summary)
            }
            .await;
            match result {
                Ok(summary) => PendingSyncOutcome::success(
                    request_id,
                    operation.operation,
                    Some(summary.id),
                    Some(summary.branch_id),
                    serde_json::to_value(summary).map_err(RestError::from),
                ),
                Err(error) => PendingSyncOutcome::failure(
                    request_id,
                    operation.operation,
                    Some(payload.work_order_id),
                    error,
                ),
            }
        }
        SyncOperationKind::WorkOrderReport => {
            let parsed = serde_json::from_value::<SyncWorkOrderReportPayload>(operation.payload)
                .map_err(|err| RestError::bad_request(format!("invalid report payload: {err}")));
            let payload = match parsed {
                Ok(payload) => payload,
                Err(error) => {
                    return PendingSyncOutcome::failure(
                        request_id,
                        operation.operation,
                        None,
                        error,
                    );
                }
            };
            let work_order_id = payload.work_order_id;
            let result_type = payload.result_type;
            let diagnosis = payload.diagnosis;
            let action_taken = payload.action_taken;
            let result = async {
                let work_order = state
                    .store
                    .work_order(work_order_id)
                    .await
                    .map_err(RestError::from_store)?;
                authorize(
                    principal,
                    Action::new(Feature::WorkReportSubmit),
                    work_order.branch_id,
                )
                .map_err(RestError::from_kernel)?;
                let summary = match state
                    .store
                    .submit_report(SubmitReportCommand {
                        actor: principal.user_id,
                        work_order_id,
                        result_type,
                        diagnosis,
                        action_taken,
                        trace: TraceContext::generate(),
                        occurred_at: time::OffsetDateTime::now_utc(),
                    })
                    .await
                {
                    Ok(summary) => summary,
                    // FIX 2 reconciliation: if the report already applied (a
                    // crash re-runs this), re-derive the current state instead of
                    // double-mutating.
                    Err(err) => {
                        let current = state
                            .store
                            .work_order(work_order_id)
                            .await
                            .map_err(RestError::from_store)?;
                        if current.status == WorkOrderStatus::ReportSubmitted {
                            current
                        } else {
                            return Err(RestError::from_store(err));
                        }
                    }
                };
                Ok(summary)
            }
            .await;
            match result {
                Ok(summary) => PendingSyncOutcome::success(
                    request_id,
                    operation.operation,
                    Some(summary.id),
                    Some(summary.branch_id),
                    serde_json::to_value(summary).map_err(RestError::from),
                ),
                Err(error) => PendingSyncOutcome::failure(
                    request_id,
                    operation.operation,
                    Some(work_order_id),
                    error,
                ),
            }
        }
    }
}

enum SyncClaim {
    /// Fresh claim — run the operation, then complete the row.
    Claimed(uuid::Uuid),
    /// Already completed with a matching payload — return the cached response.
    Cached(SyncOperationResult),
    /// Claimed but never completed (worker crash). Reconcile against the target
    /// work-order state and finalize the row (FIX 2).
    Stale(uuid::Uuid),
    /// Same (device_hash, request_id) replayed with a DIFFERENT payload (FIX 1).
    PayloadMismatch,
}

struct PendingSyncOutcome {
    result: SyncOperationResult,
    work_order_id: Option<WorkOrderId>,
    branch_id: Option<BranchId>,
}

impl PendingSyncOutcome {
    fn success(
        request_id: String,
        operation: SyncOperationKind,
        work_order_id: Option<WorkOrderId>,
        branch_id: Option<BranchId>,
        value: Result<serde_json::Value, RestError>,
    ) -> Self {
        match value {
            Ok(result) => Self {
                result: SyncOperationResult {
                    request_id,
                    operation,
                    status: SyncOperationStatus::Applied,
                    http_status: StatusCode::OK.as_u16(),
                    result: Some(result),
                    error: None,
                    replayed: false,
                },
                work_order_id,
                branch_id,
            },
            Err(error) => Self::failure(request_id, operation, work_order_id, error),
        }
    }

    fn failure(
        request_id: String,
        operation: SyncOperationKind,
        work_order_id: Option<WorkOrderId>,
        error: RestError,
    ) -> Self {
        Self {
            branch_id: None,
            work_order_id,
            result: SyncOperationResult {
                request_id,
                operation,
                status: SyncOperationStatus::Failed,
                http_status: error.status.as_u16(),
                result: None,
                error: Some(SyncErrorPayload {
                    code: error.code().to_owned(),
                    message: error.message,
                }),
                replayed: false,
            },
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn claim_sync_request(
    pool: &PgPool,
    actor: UserId,
    device_hash: &str,
    sync_id: &str,
    request_id: &str,
    operation: SyncOperationKind,
    client_created_at: time::OffsetDateTime,
    payload_hash: &str,
    payload_envelope: &serde_json::Value,
) -> Result<SyncClaim, RestError> {
    let org = current_org().map_err(|err| RestError::from_kernel(err.into()))?;
    let org_uuid = *org.as_uuid();
    let event: AuditEvent = AuditEvent::new(
        Some(actor),
        AuditAction::new("offline_sync.receive").map_err(RestError::from_kernel)?,
        "offline_sync_request",
        request_id,
        TraceContext::generate(),
        time::OffsetDateTime::now_utc(),
    )
    .with_org(org)
    .with_snapshots(
        None,
        Some(serde_json::json!({
            "request_id": request_id,
            "sync_id": sync_id,
            "operation": operation.as_db_str(),
            "payload_hash": payload_hash,
        })),
    );

    with_audit::<_, SyncClaim, RestError>(pool, event, |tx| {
        let device_hash = device_hash.to_owned();
        let sync_id = sync_id.to_owned();
        let request_id = request_id.to_owned();
        let payload_hash = payload_hash.to_owned();
        let payload_envelope = payload_envelope.clone();
        Box::pin(async move {
            // Lock the existing row (if any) so a concurrent claim of the same
            // (device_hash, request_id) is serialized.
            let existing = sqlx::query(
                r#"
                SELECT id, status, payload_hash, response_body
                FROM offline_sync_requests
                WHERE device_hash = $1 AND request_id = $2
                FOR UPDATE
                "#,
            )
            .bind(&device_hash)
            .bind(&request_id)
            .fetch_optional(tx.as_mut())
            .await?;

            if let Some(row) = existing {
                let existing_hash: Option<String> = row.try_get("payload_hash")?;
                // FIX 1: same idempotency key, different content → reject.
                if existing_hash.as_deref() != Some(payload_hash.as_str()) {
                    return Ok(SyncClaim::PayloadMismatch);
                }
                let status: String = row.try_get("status")?;
                let response: Option<serde_json::Value> = row.try_get("response_body")?;
                let id: uuid::Uuid = row.try_get("id")?;
                return match (status.as_str(), response) {
                    // Completed with a matching payload → cached idempotent reply.
                    (_, Some(value)) => Ok(SyncClaim::Cached(serde_json::from_value(value)?)),
                    // Claimed but never completed → reconcile (FIX 2).
                    ("IN_PROGRESS", None) => Ok(SyncClaim::Stale(id)),
                    // APPLIED/FAILED with no body should not happen; treat as
                    // reconcilable rather than returning an empty response.
                    (_, None) => Ok(SyncClaim::Stale(id)),
                };
            }

            let id: uuid::Uuid = sqlx::query_scalar(
                r#"
                INSERT INTO offline_sync_requests (
                    user_id, device_hash, request_id, sync_id, operation_type,
                    client_created_at, status, payload_hash, request_payload, org_id
                )
                VALUES ($1, $2, $3, $4, $5, $6, 'IN_PROGRESS', $7, $8, $9)
                RETURNING id
                "#,
            )
            .bind(*actor.as_uuid())
            .bind(&device_hash)
            .bind(&request_id)
            .bind(&sync_id)
            .bind(operation.as_db_str())
            .bind(client_created_at)
            .bind(&payload_hash)
            .bind(&payload_envelope)
            .bind(org_uuid)
            .fetch_one(tx.as_mut())
            .await?;
            Ok(SyncClaim::Claimed(id))
        })
    })
    .await
}

async fn complete_sync_request(
    pool: &PgPool,
    sync_request_id: uuid::Uuid,
    actor: UserId,
    outcome: PendingSyncOutcome,
) -> Result<SyncOperationResult, RestError> {
    let org = current_org().map_err(|err| RestError::from_kernel(err.into()))?;
    let response_body = serde_json::to_value(&outcome.result)?;
    let event: AuditEvent = AuditEvent::new(
        Some(actor),
        AuditAction::new("offline_sync.complete").map_err(RestError::from_kernel)?,
        "offline_sync_request",
        sync_request_id.to_string(),
        TraceContext::generate(),
        time::OffsetDateTime::now_utc(),
    )
    .with_org(org)
    .with_snapshots(None, Some(response_body.clone()));
    let result = outcome.result.clone();

    with_audit::<_, (), RestError>(pool, event, |tx| {
        Box::pin(async move {
            sqlx::query(
                r#"
                UPDATE offline_sync_requests
                SET status = $2,
                    http_status = $3,
                    response_body = $4,
                    branch_id = $5,
                    work_order_id = $6,
                    completed_at = now()
                WHERE id = $1
                "#,
            )
            .bind(sync_request_id)
            .bind(outcome.result.status.as_db_str())
            .bind(i32::from(outcome.result.http_status))
            .bind(response_body)
            .bind(outcome.branch_id.map(|id| *id.as_uuid()))
            .bind(
                outcome
                    .branch_id
                    .and_then(|_| outcome.work_order_id.map(|id| *id.as_uuid())),
            )
            .execute(tx.as_mut())
            .await?;
            Ok(())
        })
    })
    .await?;
    Ok(result)
}

async fn register_device<S>(
    State(state): State<MobileRestState<S>>,
    headers: HeaderMap,
    Json(body): Json<DeviceRegistrationRequest>,
) -> Result<Json<DeviceRegistrationResponse>, RestError>
where
    S: S3ObjectStore + Clone + Send + Sync + 'static,
{
    let principal = mobile_principal_from_headers(&state, &headers).await?;
    let device_hash = device_hash_from_headers(&headers)?;
    let app_version = normalize_non_empty(body.app_version, "app_version")?;
    let now = time::OffsetDateTime::now_utc();
    let org_uuid = *principal.org_id.as_uuid();
    let event: AuditEvent = AuditEvent::new(
        Some(principal.user_id),
        AuditAction::new("device.register").map_err(RestError::from_kernel)?,
        "registered_device",
        device_hash.clone(),
        TraceContext::generate(),
        now,
    )
    .with_org(principal.org_id);
    let event = match audit_branch_for_principal(&principal)? {
        Some(branch_id) => event.with_branch(branch_id),
        None => event,
    };
    let event = event.with_snapshots(
        None,
        Some(serde_json::json!({
            "platform": body.platform,
            "app_version": app_version,
            "push_token_present": body.push_token.as_ref().is_some_and(|token| !token.trim().is_empty()),
        })),
    );

    let response =
        with_audit::<_, DeviceRegistrationResponse, RestError>(&state.pool, event, |tx| {
            Box::pin(async move {
                let row = sqlx::query(
                    r#"
                    INSERT INTO registered_devices (
                        user_id, device_hash, platform, push_token, app_version,
                        last_registered_at, created_at, updated_at, org_id
                    )
                    VALUES ($1, $2, $3, $4, $5, $6, $6, $6, $7)
                    ON CONFLICT (user_id, device_hash) DO UPDATE
                    SET platform = EXCLUDED.platform,
                        push_token = EXCLUDED.push_token,
                        app_version = EXCLUDED.app_version,
                        last_registered_at = EXCLUDED.last_registered_at,
                        updated_at = EXCLUDED.updated_at
                    RETURNING id, user_id, device_hash, platform, push_token,
                              app_version, last_registered_at
                    "#,
                )
                .bind(*principal.user_id.as_uuid())
                .bind(&device_hash)
                .bind(body.platform.as_db_str())
                .bind(body.push_token.as_deref().map(str::trim))
                .bind(&app_version)
                .bind(now)
                .bind(org_uuid)
                .fetch_one(tx.as_mut())
                .await?;
                device_registration_from_row(&row)
            })
        })
        .await?;

    Ok(Json(response))
}

async fn presign_evidence<S>(
    State(state): State<MobileRestState<S>>,
    headers: HeaderMap,
    Json(body): Json<EvidencePresignRequest>,
) -> Result<Json<EvidencePresignResponse>, RestError>
where
    S: S3ObjectStore + Clone + Send + Sync + 'static,
{
    let principal = mobile_principal_from_headers(&state, &headers).await?;
    let work_order = state
        .store
        .work_order(body.work_order_id)
        .await
        .map_err(RestError::from_store)?;
    authorize_evidence_access(&state, &principal, body.work_order_id, work_order.branch_id).await?;
    // FIX 3: reject AFTER/REPORT completion evidence for terminal work orders at
    // the REST boundary, before issuing a presigned upload URL. The storage
    // adapter re-checks under a row lock and the DB trigger is the final barrier.
    if matches!(body.stage, AttachmentStage::After | AttachmentStage::Report)
        && is_terminal_work_order_status(work_order.status)
    {
        return Err(RestError::from_kernel(KernelError::conflict(format!(
            "cannot attach {} evidence to a work order in terminal status {}",
            body.stage.as_db_str(),
            work_order.status.as_db_str()
        ))));
    }
    let service = state.evidence_service.as_ref().ok_or_else(|| {
        RestError::unavailable("evidence storage is not configured for mobile API")
    })?;
    let ticket = service
        .issue_presigned_upload(EvidenceUploadCommand {
            actor: principal.user_id,
            work_order_id: body.work_order_id,
            stage: body.stage,
            content_type: body.content_type,
            size_bytes: body.size_bytes,
            checksum_sha256: body.checksum_sha256,
            trace: TraceContext::generate(),
            occurred_at: time::OffsetDateTime::now_utc(),
        })
        .await
        .map_err(RestError::from_storage)?;
    record_evidence_presign_audit(&state.pool, &principal, work_order.branch_id, &ticket).await?;

    Ok(Json(EvidencePresignResponse {
        id: ticket.media.id,
        work_order_id: ticket.media.work_order_id,
        stage: ticket.media.stage,
        upload: ticket.upload,
    }))
}

async fn confirm_evidence<S>(
    State(state): State<MobileRestState<S>>,
    headers: HeaderMap,
    Path(evidence_id): Path<uuid::Uuid>,
) -> Result<Json<EvidenceConfirmResponse>, RestError>
where
    S: S3ObjectStore + Clone + Send + Sync + 'static,
{
    let principal = mobile_principal_from_headers(&state, &headers).await?;
    let media_id = EvidenceId::from_uuid(evidence_id);
    let service = state.evidence_service.as_ref().ok_or_else(|| {
        RestError::unavailable("evidence storage is not configured for mobile API")
    })?;
    let media = service
        .evidence_media(media_id)
        .await
        .map_err(RestError::from_storage)?;
    let work_order = state
        .store
        .work_order(media.work_order_id)
        .await
        .map_err(RestError::from_store)?;
    authorize_evidence_access(
        &state,
        &principal,
        media.work_order_id,
        work_order.branch_id,
    )
    .await?;
    service
        .confirm_upload(
            media_id,
            principal.user_id,
            TraceContext::generate(),
            time::OffsetDateTime::now_utc(),
        )
        .await
        .map_err(RestError::from_storage)?;
    let outcome = service
        .replicate_once(
            media_id,
            TraceContext::generate(),
            time::OffsetDateTime::now_utc(),
        )
        .await
        .map_err(RestError::from_storage)?;
    // Re-read the row to return the post-replication verified_at. If the media
    // has vanished (e.g. a concurrent purge between replication and this read),
    // surface the storage error — never serve the stale pre-replication row,
    // which would report success with a null verified_at.
    let media = service
        .evidence_media(media_id)
        .await
        .map_err(RestError::from_storage)?;

    Ok(Json(EvidenceConfirmResponse {
        id: media.id,
        work_order_id: media.work_order_id,
        stage: media.stage,
        worm_replica_status: outcome.status,
        retry_count: outcome.retry_count,
        verified_at: media.verified_at,
    }))
}

/// Media-processing staging-upload presign. The mechanic PUTs the ORIGINAL to a
/// tenant-scoped staging key; a PROCESSING evidence row is created and an async
/// transcode job is enqueued. The optimized 1080p/recompressed artifact replaces
/// the original at the FINAL key before the row becomes READY. Same authz as the
/// direct presign: only an ASSIGNED mechanic (or admin) on the work order.
async fn presign_evidence_staging<S>(
    State(state): State<MobileRestState<S>>,
    headers: HeaderMap,
    Json(body): Json<EvidenceStagingPresignRequest>,
) -> Result<Json<EvidenceStagingPresignResponse>, RestError>
where
    S: S3ObjectStore + Clone + Send + Sync + 'static,
{
    let principal = mobile_principal_from_headers(&state, &headers).await?;
    let work_order = state
        .store
        .work_order(body.work_order_id)
        .await
        .map_err(RestError::from_store)?;
    authorize_evidence_access(&state, &principal, body.work_order_id, work_order.branch_id).await?;
    if matches!(body.stage, AttachmentStage::After | AttachmentStage::Report)
        && is_terminal_work_order_status(work_order.status)
    {
        return Err(RestError::from_kernel(KernelError::conflict(format!(
            "cannot attach {} evidence to a work order in terminal status {}",
            body.stage.as_db_str(),
            work_order.status.as_db_str()
        ))));
    }
    let service = state.evidence_service.as_ref().ok_or_else(|| {
        RestError::unavailable("evidence storage is not configured for mobile API")
    })?;
    let job_queue = state.job_queue.as_ref().ok_or_else(|| {
        RestError::unavailable("evidence processing is not configured for mobile API")
    })?;
    let org = current_org()
        .map_err(KernelError::from)
        .map_err(RestError::from_kernel)?;
    let ticket = service
        .issue_staging_upload(StagingUploadCommand {
            actor: principal.user_id,
            work_order_id: body.work_order_id,
            stage: body.stage,
            content_type: body.content_type,
            size_bytes: body.size_bytes,
            checksum_sha256: body.checksum_sha256,
            trace: TraceContext::generate(),
            occurred_at: time::OffsetDateTime::now_utc(),
        })
        .await
        .map_err(RestError::from_storage)?;
    // Enqueue the async transcode carrying the owning tenant so the worker arms
    // app.current_org correctly. Enqueue AFTER the PROCESSING row commits so the
    // worker can always find the row to claim.
    let request = JobRequest::evidence_transcode(org, ticket.media.id)
        .map_err(|err| RestError::internal(err.to_string()))?;
    job_queue
        .enqueue(request)
        .await
        .map_err(|err| RestError::internal(err.to_string()))?;
    record_evidence_presign_audit_named(
        &state.pool,
        &principal,
        work_order.branch_id,
        ticket.media.id,
        ticket.media.stage,
        "evidence.staging.presign",
    )
    .await?;

    Ok(Json(EvidenceStagingPresignResponse {
        id: ticket.media.id,
        work_order_id: ticket.media.work_order_id,
        stage: ticket.media.stage,
        media_kind: ticket.media_kind,
        processing_status: ticket.media.processing_status,
        upload: ticket.upload,
    }))
}

/// Poll the server-side processing status of an evidence row (처리 중 → 완료 /
/// 실패). Read access follows branch-scoped work-order visibility.
async fn evidence_status<S>(
    State(state): State<MobileRestState<S>>,
    headers: HeaderMap,
    Path(evidence_id): Path<uuid::Uuid>,
) -> Result<Json<EvidenceStatusResponse>, RestError>
where
    S: S3ObjectStore + Clone + Send + Sync + 'static,
{
    let principal = mobile_principal_from_headers(&state, &headers).await?;
    let media_id = EvidenceId::from_uuid(evidence_id);
    let media = if let Some(service) = state.evidence_service.as_ref() {
        service
            .evidence_media(media_id)
            .await
            .map_err(RestError::from_storage)?
    } else {
        evidence_media_status_from_db(&state.pool, media_id).await?
    };
    let work_order = state
        .store
        .work_order(media.work_order_id)
        .await
        .map_err(RestError::from_store)?;
    authorize_evidence_status_access(&principal, work_order.branch_id)?;

    // Hand the client a short-lived presigned GET URL for the thumbnail instead
    // of the raw object key (which leaked internal storage layout). Null until
    // the row is READY and a thumbnail exists.
    let thumbnail_url = if let Some(service) = state.evidence_service.as_ref() {
        service
            .presigned_thumbnail_url(&media)
            .await
            .map_err(RestError::from_storage)?
    } else {
        None
    };

    Ok(Json(EvidenceStatusResponse {
        id: media.id,
        work_order_id: media.work_order_id,
        stage: media.stage,
        processing_status: media.processing_status,
        content_type: media.content_type,
        thumbnail_url,
        processing_error: media.processing_error,
        processed_at: media.processed_at,
    }))
}

async fn evidence_media_status_from_db(
    pool: &PgPool,
    media_id: EvidenceId,
) -> Result<EvidenceMedia, RestError> {
    let org = current_org()
        .map_err(KernelError::from)
        .map_err(RestError::from_kernel)?;
    let row = with_org_conn::<_, _, RestError>(pool, org, move |tx| {
        Box::pin(async move {
            Ok(sqlx::query(
                r#"
                SELECT id, work_order_id, stage, s3_key, content_type, size_bytes,
                       checksum_sha256, uploaded_by, worm_replica_status, retry_count,
                       next_retry_at, last_error, verified_at, upload_confirmed_at,
                       confirmed_by, created_at, updated_at,
                       processing_status, staging_s3_key, thumbnail_s3_key,
                       original_content_type, processing_error, processed_at
                FROM evidence_media
                WHERE id = $1
                "#,
            )
            .bind(*media_id.as_uuid())
            .fetch_optional(tx.as_mut())
            .await?)
        })
    })
    .await?
    .ok_or_else(|| {
        RestError::from_kernel(KernelError::not_found("evidence media was not found"))
    })?;

    evidence_media_from_status_row(&row)
}

fn evidence_media_from_status_row(row: &sqlx::postgres::PgRow) -> Result<EvidenceMedia, RestError> {
    let stage: String = row.try_get("stage")?;
    let worm_status: String = row.try_get("worm_replica_status")?;
    let processing_status: String = row.try_get("processing_status")?;
    Ok(EvidenceMedia {
        id: EvidenceId::from_uuid(row.try_get("id")?),
        work_order_id: WorkOrderId::from_uuid(row.try_get("work_order_id")?),
        stage: AttachmentStage::from_db_str(&stage).map_err(RestError::from_kernel)?,
        s3_key: row.try_get("s3_key")?,
        content_type: row.try_get("content_type")?,
        size_bytes: row.try_get("size_bytes")?,
        checksum_sha256: row.try_get("checksum_sha256")?,
        uploaded_by: UserId::from_uuid(row.try_get("uploaded_by")?),
        worm_replica_status: WormReplicaStatus::from_db_str(&worm_status)
            .map_err(RestError::from_kernel)?,
        retry_count: row.try_get("retry_count")?,
        next_retry_at: row.try_get("next_retry_at")?,
        last_error: row.try_get("last_error")?,
        verified_at: row.try_get("verified_at")?,
        upload_confirmed_at: row.try_get("upload_confirmed_at")?,
        confirmed_by: row
            .try_get::<Option<uuid::Uuid>, _>("confirmed_by")?
            .map(UserId::from_uuid),
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
        processing_status: ProcessingStatus::from_db_str(&processing_status)
            .map_err(RestError::from_kernel)?,
        staging_s3_key: row.try_get("staging_s3_key")?,
        thumbnail_s3_key: row.try_get("thumbnail_s3_key")?,
        original_content_type: row.try_get("original_content_type")?,
        processing_error: row.try_get("processing_error")?,
        processed_at: row.try_get("processed_at")?,
    })
}

fn authorize_evidence_status_access(
    principal: &Principal,
    branch_id: BranchId,
) -> Result<(), RestError> {
    authorize(principal, Action::new(Feature::WorkOrderReadAll), branch_id)
        .map_err(RestError::from_kernel)
}
async fn authorize_evidence_access<S>(
    state: &MobileRestState<S>,
    principal: &Principal,
    work_order_id: WorkOrderId,
    branch_id: BranchId,
) -> Result<(), RestError>
where
    S: S3ObjectStore + Clone + Send + Sync + 'static,
{
    authorize(principal, Action::new(Feature::EvidenceAttach), branch_id)
        .map_err(RestError::from_kernel)?;
    if is_admin_like(principal)
        || is_assigned_to_work_order(&state.pool, work_order_id, principal.user_id).await?
    {
        Ok(())
    } else {
        Err(RestError::from_kernel(KernelError::forbidden(
            "evidence upload requires an assigned mechanic or admin role",
        )))
    }
}

async fn is_assigned_to_work_order(
    pool: &PgPool,
    work_order_id: WorkOrderId,
    user_id: UserId,
) -> Result<bool, RestError> {
    let org = current_org()
        .map_err(KernelError::from)
        .map_err(RestError::from_kernel)?;
    let work_order_uuid = *work_order_id.as_uuid();
    let user_uuid = *user_id.as_uuid();
    let count: i64 = with_org_conn::<_, _, RestError>(pool, org, move |tx| {
        Box::pin(async move {
            Ok(sqlx::query_scalar(
                r#"
        SELECT COUNT(*)
        FROM work_order_assignments
        WHERE work_order_id = $1 AND mechanic_id = $2
        "#,
            )
            .bind(work_order_uuid)
            .bind(user_uuid)
            .fetch_one(tx.as_mut())
            .await?)
        })
    })
    .await?;
    Ok(count > 0)
}

fn is_admin_like(principal: &Principal) -> bool {
    principal
        .roles
        .iter()
        .any(|role| matches!(role, Role::Admin | Role::SuperAdmin))
}

/// Whether the principal may read the work-order + daily-plan queues org-wide
/// (every branch in the tenant) regardless of branch membership.
///
/// Gated on the [`Feature::OrgWideQueueTriage`] capability — EXECUTIVE +
/// SUPER_ADMIN today — NOT a role string. A branch-scoped ADMIN does NOT hold
/// it and stays confined to its branch set, matching `resolve_branch_scope_in_org`.
fn has_org_wide_queue_triage(principal: &Principal) -> bool {
    principal
        .roles
        .iter()
        .any(|role| permission_for(*role, Feature::OrgWideQueueTriage) == PermissionLevel::Allow)
}

/// The branch scope to apply when triaging the work-order queue.
///
/// A receptionist files a work order against a branch they belong to; the
/// org-wide triager who must act on it may not be a member of that branch, so a
/// strict branch-scope filter would hide the just-created order from them
/// (#19.13b). Principals holding `OrgWideQueueTriage` (EXECUTIVE + SUPER_ADMIN)
/// see the whole org — RLS still confines the read to the caller's tenant —
/// while every other role (including a branch-scoped ADMIN) keeps its explicit
/// branch set.
fn work_order_list_scope(principal: &Principal) -> BranchScope {
    if has_org_wide_queue_triage(principal) {
        BranchScope::All
    } else {
        principal.branch_scope.clone()
    }
}

/// Work orders in these statuses are closed: their completion evidence set is
/// frozen and must not accept further AFTER/REPORT attachments (FIX 3).
fn is_terminal_work_order_status(status: WorkOrderStatus) -> bool {
    matches!(
        status,
        WorkOrderStatus::FinalCompleted | WorkOrderStatus::Archived | WorkOrderStatus::Cancelled
    )
}

async fn record_evidence_presign_audit(
    pool: &PgPool,
    principal: &Principal,
    branch_id: BranchId,
    ticket: &EvidenceUploadTicket,
) -> Result<(), RestError> {
    let org = current_org()
        .map_err(KernelError::from)
        .map_err(RestError::from_kernel)?;
    let event: AuditEvent = AuditEvent::new(
        Some(principal.user_id),
        AuditAction::new("evidence.presign").map_err(RestError::from_kernel)?,
        "evidence_media",
        ticket.media.id.to_string(),
        TraceContext::generate(),
        time::OffsetDateTime::now_utc(),
    )
    .with_branch(branch_id)
    .with_org(org)
    .with_snapshots(
        None,
        Some(serde_json::json!({
            "work_order_id": ticket.media.work_order_id,
            "stage": ticket.media.stage,
            "s3_key": ticket.media.s3_key,
        })),
    );
    with_audit::<_, (), RestError>(pool, event, |_tx| Box::pin(async move { Ok(()) })).await
}

/// Audit a media-processing staging presign under an explicit action name.
/// Mirrors [`record_evidence_presign_audit`]; the org is armed by `with_audit`
/// via the event's org stamp so the audit insert is RLS-correct as `console_rt`.
async fn record_evidence_presign_audit_named(
    pool: &PgPool,
    principal: &Principal,
    branch_id: BranchId,
    media_id: EvidenceId,
    stage: AttachmentStage,
    action: &str,
) -> Result<(), RestError> {
    let org = current_org()
        .map_err(KernelError::from)
        .map_err(RestError::from_kernel)?;
    let event: AuditEvent = AuditEvent::new(
        Some(principal.user_id),
        AuditAction::new(action).map_err(RestError::from_kernel)?,
        "evidence_media",
        media_id.to_string(),
        TraceContext::generate(),
        time::OffsetDateTime::now_utc(),
    )
    .with_branch(branch_id)
    .with_org(org)
    .with_snapshots(
        None,
        Some(serde_json::json!({
            "stage": stage,
            "processing_status": "PROCESSING",
        })),
    );
    with_audit::<_, (), RestError>(pool, event, |_tx| Box::pin(async move { Ok(()) })).await
}

async fn list_work_orders(
    State(state): State<WorkOrderRestState>,
    headers: HeaderMap,
    RawQuery(raw_query): RawQuery,
) -> Result<Json<WorkOrderListPage>, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    authorize_read_access(&principal)?;
    let query = parse_work_order_list_query(raw_query.as_deref(), principal.user_id)?;
    let pool = state.store.pool();
    // Admins triage org-wide so a just-filed order is never hidden from them by a
    // branch-scope filter (#19.13b); other roles keep their explicit branch set.
    let list_scope = work_order_list_scope(&principal);

    let org = current_org()
        .map_err(KernelError::from)
        .map_err(RestError::from_kernel)?;

    let total = {
        let mut builder =
            QueryBuilder::<Postgres>::new("SELECT COUNT(*) FROM work_orders w WHERE ");
        push_work_order_filters(&mut builder, &list_scope, &query)?;
        with_org_conn::<_, _, RestError>(pool, org, move |tx| {
            Box::pin(async move {
                Ok(builder
                    .build_query_scalar::<i64>()
                    .fetch_one(tx.as_mut())
                    .await?)
            })
        })
        .await?
    };

    let mut list_builder = QueryBuilder::<Postgres>::new(
        r#"
        SELECT
            w.id, w.request_no, w.branch_id, w.status, w.priority, w.result_type,
            w.maintenance_type, w.maintenance_cause,
            w.target_due_at, w.created_at, w.updated_at,
            e.id AS equipment_id, e.equipment_no, e.management_no, e.model,
            e.status AS equipment_status, e.specification, e.ton_text,
            c.id AS customer_id, c.name AS customer_name,
            s.id AS site_id, s.name AS site_name,
            s.contact_name AS site_contact_name,
            s.contact_phone AS site_contact_phone,
            s.contact_email AS site_contact_email
        FROM work_orders w
        JOIN registry_equipment e ON e.id = w.equipment_id
        JOIN registry_customers c ON c.id = w.customer_id
        JOIN registry_sites s ON s.id = w.site_id
        WHERE
        "#,
    );
    push_work_order_filters(&mut list_builder, &list_scope, &query)?;
    list_builder.push(
        r#"
        ORDER BY
            CASE w.priority
                WHEN 'P1' THEN 1
                WHEN 'P2' THEN 2
                WHEN 'P3' THEN 3
                WHEN 'OUTSOURCE' THEN 4
                ELSE 5
            END,
            w.target_due_at ASC NULLS LAST,
            w.created_at ASC,
            w.id ASC
        LIMIT
        "#,
    );
    list_builder.push_bind(query.limit);
    list_builder.push(" OFFSET ");
    list_builder.push_bind(query.offset);
    let rows = with_org_conn::<_, _, RestError>(pool, org, move |tx| {
        Box::pin(async move { Ok(list_builder.build().fetch_all(tx.as_mut()).await?) })
    })
    .await?;
    let work_order_ids = rows
        .iter()
        .map(|row| row.try_get("id"))
        .collect::<Result<Vec<uuid::Uuid>, _>>()?;
    let assignments = fetch_assignment_map(pool, &work_order_ids).await?;
    let items = rows
        .iter()
        .map(|row| work_order_list_item_from_row(row, &assignments))
        .collect::<Result<Vec<_>, _>>()?;
    let lens = fetch_work_order_object_set_lens(pool, org, &list_scope, &query).await?;

    Ok(Json(WorkOrderListPage {
        items,
        limit: query.limit,
        offset: query.offset,
        total,
        lens,
    }))
}

async fn list_approval_items(
    State(state): State<WorkOrderRestState>,
    headers: HeaderMap,
    Query(query): Query<ApprovalItemsQuery>,
) -> Result<Json<ApprovalItemsPage>, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    let visibility = approval_source_visibility(&principal)?;
    let query = NormalizedApprovalItemsQuery {
        limit: normalize_limit(query.limit, 100, 200)?,
        offset: normalize_offset(query.offset)?,
    };
    // Approval queues follow the same queue-visibility rule as work-order and
    // daily-plan triage: org-wide queue triagers see every branch in the tenant;
    // everyone else remains confined to their explicit branch membership.
    let branch_scope = work_order_list_scope(&principal);
    let pool = state.store.pool();

    let counts =
        fetch_approval_source_counts(pool, &branch_scope, visibility, principal.user_id).await?;
    let total: i64 = counts.values().sum();
    let rows =
        fetch_approval_rows(pool, &branch_scope, visibility, principal.user_id, &query).await?;
    let work_order_ids = rows
        .iter()
        .filter_map(|row| {
            let source: String = row.try_get("source").ok()?;
            (source == "WORK_ORDER")
                .then(|| row.try_get("source_id").ok())
                .flatten()
        })
        .collect::<Vec<uuid::Uuid>>();
    let assignments = fetch_assignment_map(pool, &work_order_ids).await?;
    let items = rows
        .iter()
        .map(|row| approval_item_from_row(row, &assignments))
        .collect::<Result<Vec<_>, _>>()?;

    Ok(Json(ApprovalItemsPage {
        items,
        sources: approval_sources_from_counts(&counts, visibility),
        limit: query.limit,
        offset: query.offset,
        total,
    }))
}

async fn fetch_work_order_object_set_lens(
    pool: &PgPool,
    org: OrgId,
    branch_scope: &BranchScope,
    query: &NormalizedWorkOrderListQuery,
) -> Result<WorkOrderObjectSetLens, RestError> {
    let aggregates = fetch_work_order_lens_aggregates(pool, org, branch_scope, query).await?;
    let status =
        fetch_work_order_string_facet(pool, org, branch_scope, query, "w.status", "status").await?;
    let priority =
        fetch_work_order_string_facet(pool, org, branch_scope, query, "w.priority", "priority")
            .await?;
    // Classification facets skip legacy rows with no classification: an absent
    // value is rendered as an absent chip, never a facet bucket.
    let maintenance_type = fetch_work_order_string_facet(
        pool,
        org,
        branch_scope,
        query,
        "w.maintenance_type",
        "maintenance_type",
    )
    .await?;
    let maintenance_cause = fetch_work_order_string_facet(
        pool,
        org,
        branch_scope,
        query,
        "w.maintenance_cause",
        "maintenance_cause",
    )
    .await?;
    let target_due_date = fetch_work_order_due_histogram(pool, org, branch_scope, query).await?;
    let customers = fetch_work_order_named_listogram(
        pool,
        org,
        branch_scope,
        query,
        WorkOrderNamedListogramSpec {
            table: "registry_customers",
            alias: "c",
            join_column: "w.customer_id",
            filter_key: "customer_id",
        },
    )
    .await?;
    let sites = fetch_work_order_named_listogram(
        pool,
        org,
        branch_scope,
        query,
        WorkOrderNamedListogramSpec {
            table: "registry_sites",
            alias: "s",
            join_column: "w.site_id",
            filter_key: "site_id",
        },
    )
    .await?;

    Ok(WorkOrderObjectSetLens {
        object_type: "work_order",
        aggregates,
        facets: WorkOrderLensFacets {
            status,
            priority,
            maintenance_type,
            maintenance_cause,
        },
        histograms: WorkOrderLensHistograms { target_due_date },
        listograms: WorkOrderLensListograms { customers, sites },
    })
}

async fn fetch_work_order_lens_aggregates(
    pool: &PgPool,
    org: OrgId,
    branch_scope: &BranchScope,
    query: &NormalizedWorkOrderListQuery,
) -> Result<WorkOrderLensAggregates, RestError> {
    let mut builder = QueryBuilder::<Postgres>::new(
        r#"
        SELECT
            COUNT(*) AS total_count,
            COUNT(*) FILTER (WHERE w.priority = 'P1') AS p1_count,
            COUNT(*) FILTER (
                WHERE w.target_due_at < now()
                  AND w.status NOT IN ('FINAL_COMPLETED', 'REJECTED', 'ARCHIVED', 'CANCELLED')
            ) AS overdue_open_count,
            COUNT(*) FILTER (
                WHERE NOT EXISTS (
                    SELECT 1
                    FROM work_order_assignments a
                    WHERE a.work_order_id = w.id
                )
            ) AS unassigned_count,
            (AVG(
                CASE WHEN completed.occurred_at <= w.target_due_at THEN 1.0 ELSE 0.0 END
            ) FILTER (
                WHERE w.maintenance_type = 'PREVENTIVE'
                  AND w.status = 'FINAL_COMPLETED'
                  AND w.target_due_at IS NOT NULL
                  AND completed.occurred_at IS NOT NULL
            ))::float8 AS preventive_on_time_rate,
            (AVG(
                EXTRACT(EPOCH FROM (reported.occurred_at - started.occurred_at)) / 60.0
            ) FILTER (
                WHERE started.occurred_at IS NOT NULL
                  AND reported.occurred_at IS NOT NULL
                  AND reported.occurred_at >= started.occurred_at
            ))::float8 AS mttr_minutes
        FROM work_orders w
        LEFT JOIN LATERAL (
            SELECT MIN(h.occurred_at) AS occurred_at
            FROM work_order_status_history h
            WHERE h.work_order_id = w.id AND h.to_status = 'FINAL_COMPLETED'
        ) completed ON TRUE
        LEFT JOIN LATERAL (
            SELECT MIN(h.occurred_at) AS occurred_at
            FROM work_order_status_history h
            WHERE h.work_order_id = w.id AND h.to_status = 'IN_PROGRESS'
        ) started ON TRUE
        LEFT JOIN LATERAL (
            SELECT MIN(h.occurred_at) AS occurred_at
            FROM work_order_status_history h
            WHERE h.work_order_id = w.id AND h.to_status = 'REPORT_SUBMITTED'
        ) reported ON TRUE
        WHERE
        "#,
    );
    push_work_order_filters(&mut builder, branch_scope, query)?;
    let row = with_org_conn::<_, _, RestError>(pool, org, move |tx| {
        Box::pin(async move { Ok(builder.build().fetch_one(tx.as_mut()).await?) })
    })
    .await?;

    Ok(WorkOrderLensAggregates {
        total_count: row.try_get("total_count")?,
        p1_count: row.try_get("p1_count")?,
        overdue_open_count: row.try_get("overdue_open_count")?,
        unassigned_count: row.try_get("unassigned_count")?,
        preventive_on_time_rate: row.try_get("preventive_on_time_rate")?,
        mttr_minutes: row.try_get("mttr_minutes")?,
    })
}

async fn fetch_work_order_string_facet(
    pool: &PgPool,
    org: OrgId,
    branch_scope: &BranchScope,
    query: &NormalizedWorkOrderListQuery,
    column: &'static str,
    filter_key: &'static str,
) -> Result<Vec<WorkOrderFacetBucket>, RestError> {
    let mut builder = QueryBuilder::<Postgres>::new("SELECT ");
    builder.push(column);
    builder.push(
        r#" AS value, COUNT(*) AS count
        FROM work_orders w
        WHERE
        "#,
    );
    push_work_order_filters(&mut builder, branch_scope, query)?;
    // NULL cells (legacy rows with no classification) are absent chips, not a
    // bucket; non-nullable columns are unaffected by this predicate.
    builder.push(" AND ");
    builder.push(column);
    builder.push(" IS NOT NULL GROUP BY ");
    builder.push(column);
    builder.push(" ORDER BY count DESC, value ASC LIMIT 16");
    let rows = with_org_conn::<_, _, RestError>(pool, org, move |tx| {
        Box::pin(async move { Ok(builder.build().fetch_all(tx.as_mut()).await?) })
    })
    .await?;

    rows.into_iter()
        .map(|row| {
            let value: String = row.try_get("value")?;
            let count: i64 = row.try_get("count")?;
            Ok(WorkOrderFacetBucket {
                filters: lens_filter(filter_key, value.clone()),
                value,
                count,
            })
        })
        .collect()
}

async fn fetch_work_order_due_histogram(
    pool: &PgPool,
    org: OrgId,
    branch_scope: &BranchScope,
    query: &NormalizedWorkOrderListQuery,
) -> Result<Vec<WorkOrderHistogramBucket>, RestError> {
    let mut builder = QueryBuilder::<Postgres>::new(
        r#"
        SELECT
            to_char((w.target_due_at AT TIME ZONE 'Asia/Seoul')::date, 'YYYY-MM-DD') AS bucket,
            COUNT(*) AS count
        FROM work_orders w
        WHERE
        "#,
    );
    push_work_order_filters(&mut builder, branch_scope, query)?;
    builder.push(
        r#"
          AND w.target_due_at IS NOT NULL
        GROUP BY bucket
        ORDER BY bucket ASC
        LIMIT 14
        "#,
    );
    let rows = with_org_conn::<_, _, RestError>(pool, org, move |tx| {
        Box::pin(async move { Ok(builder.build().fetch_all(tx.as_mut()).await?) })
    })
    .await?;

    rows.into_iter()
        .map(|row| {
            let bucket: String = row.try_get("bucket")?;
            let count: i64 = row.try_get("count")?;
            Ok(WorkOrderHistogramBucket {
                filters: due_day_filter(&bucket),
                bucket,
                count,
            })
        })
        .collect()
}

async fn fetch_work_order_named_listogram(
    pool: &PgPool,
    org: OrgId,
    branch_scope: &BranchScope,
    query: &NormalizedWorkOrderListQuery,
    spec: WorkOrderNamedListogramSpec,
) -> Result<Vec<WorkOrderNamedBucket>, RestError> {
    let mut builder = QueryBuilder::<Postgres>::new("SELECT ");
    builder.push(spec.alias);
    builder.push(".id AS id, ");
    builder.push(spec.alias);
    builder.push(
        r#".name AS name, COUNT(*) AS count
        FROM work_orders w
        JOIN "#,
    );
    builder.push(spec.table);
    builder.push(" ");
    builder.push(spec.alias);
    builder.push(" ON ");
    builder.push(spec.alias);
    builder.push(".id = ");
    builder.push(spec.join_column);
    builder.push(" WHERE ");
    push_work_order_filters(&mut builder, branch_scope, query)?;
    builder.push(" GROUP BY ");
    builder.push(spec.alias);
    builder.push(".id, ");
    builder.push(spec.alias);
    builder.push(".name ORDER BY count DESC, name ASC LIMIT 10");
    let rows = with_org_conn::<_, _, RestError>(pool, org, move |tx| {
        Box::pin(async move { Ok(builder.build().fetch_all(tx.as_mut()).await?) })
    })
    .await?;

    rows.into_iter()
        .map(|row| {
            let id: uuid::Uuid = row.try_get("id")?;
            let name: String = row.try_get("name")?;
            let count: i64 = row.try_get("count")?;
            Ok(WorkOrderNamedBucket {
                filters: lens_filter(spec.filter_key, id.to_string()),
                id,
                name,
                count,
            })
        })
        .collect()
}

fn lens_filter(key: &str, value: String) -> BTreeMap<String, String> {
    BTreeMap::from([(key.to_owned(), value)])
}

fn due_day_filter(bucket: &str) -> BTreeMap<String, String> {
    BTreeMap::from([
        (
            "target_due_from".to_owned(),
            format!("{bucket}T00:00:00+09:00"),
        ),
        (
            "target_due_to".to_owned(),
            format!("{bucket}T23:59:59+09:00"),
        ),
    ])
}

async fn get_work_order_detail(
    State(state): State<WorkOrderRestState>,
    headers: HeaderMap,
    Path(work_order_id): Path<uuid::Uuid>,
) -> Result<Json<WorkOrderDetail>, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    authorize_read_access(&principal)?;
    let pool = state.store.pool();
    let org = current_org()
        .map_err(KernelError::from)
        .map_err(RestError::from_kernel)?;
    let mut builder = QueryBuilder::<Postgres>::new(
        r#"
        SELECT
            w.id, w.request_no, w.branch_id, w.status, w.priority, w.result_type,
            w.maintenance_type, w.maintenance_cause,
            w.symptom, w.customer_request, w.target_due_at, w.delay_reason, w.delay_note,
            w.diagnosis, w.action_taken, w.report_submitted_by, w.report_submitted_at,
            w.kpi_excluded, w.created_at, w.updated_at,
            EXISTS (
                SELECT 1
                FROM evidence_media verified
                WHERE verified.work_order_id = w.id
                  AND verified.stage IN ('AFTER', 'REPORT')
                  AND verified.worm_replica_status = 'VERIFIED'
            ) AND NOT EXISTS (
                SELECT 1
                FROM evidence_media unverified
                WHERE unverified.work_order_id = w.id
                  AND unverified.stage IN ('AFTER', 'REPORT')
                  AND unverified.worm_replica_status <> 'VERIFIED'
            ) AS evidence_verified,
            e.id AS equipment_id, e.equipment_no, e.management_no, e.model,
            e.status AS equipment_status, e.specification, e.ton_text,
            c.id AS customer_id, c.name AS customer_name,
            s.id AS site_id, s.name AS site_name,
            s.contact_name AS site_contact_name,
            s.contact_phone AS site_contact_phone,
            s.contact_email AS site_contact_email
        FROM work_orders w
        JOIN registry_equipment e ON e.id = w.equipment_id
        JOIN registry_customers c ON c.id = w.customer_id
        JOIN registry_sites s ON s.id = w.site_id
        WHERE w.id =
        "#,
    );
    builder.push_bind(work_order_id);
    builder.push(" AND ");
    push_branch_scope_filter(
        &mut builder,
        &principal.branch_scope,
        BranchColumn::new("w.branch_id").map_err(RestError::from_kernel)?,
    );
    let row = with_org_conn::<_, _, RestError>(pool, org, move |tx| {
        Box::pin(async move { Ok(builder.build().fetch_optional(tx.as_mut()).await?) })
    })
    .await?
    .ok_or_else(|| RestError::from_kernel(KernelError::not_found("work order was not found")))?;
    let work_order_id = WorkOrderId::from_uuid(work_order_id);
    let assignments = fetch_assignment_map(pool, &[*work_order_id.as_uuid()])
        .await?
        .remove(work_order_id.as_uuid())
        .unwrap_or_default();
    let approval_line = fetch_approval_line(pool, work_order_id).await?;
    let status_history = fetch_status_history(pool, work_order_id).await?;
    let evidence = fetch_evidence_summaries(pool, work_order_id).await?;
    let settlement = state
        .store
        .live_settlement_for_work_order(work_order_id)
        .await
        .map_err(RestError::from_store)?;

    Ok(Json(work_order_detail_from_row(
        &row,
        assignments,
        approval_line,
        status_history,
        evidence,
        settlement,
    )?))
}

/// Fetch up to 2 candidate equipment rows matching `management_no` within the
/// caller's branch scope. `exact = true` uses `e.management_no = $typed`;
/// `exact = false` uses the leading-zero-insensitive normalized form
/// `ltrim(e.management_no,'0') = ltrim($typed,'0')`. LIMIT 2 is enough to
/// distinguish "exactly one" from "ambiguous" without a full table scan.
async fn fetch_equipment_lookup_candidates(
    state: &WorkOrderRestState,
    org: uuid::Uuid,
    branch_scope: &BranchScope,
    management_no: String,
    exact: bool,
) -> Result<Vec<sqlx::postgres::PgRow>, RestError> {
    let mut builder = QueryBuilder::<Postgres>::new(
        r#"
        SELECT
            e.id, e.branch_id, e.equipment_no, e.management_no, e.model, e.status,
            e.specification, e.ton_text, e.maker, e.vin, e.vehicle_registration_no,
            c.id AS customer_id, c.name AS customer_name,
            s.id AS site_id, s.name AS site_name
        FROM registry_equipment e
        JOIN registry_customers c ON c.id = e.customer_id
        JOIN registry_sites s ON s.id = e.site_id
        WHERE
"#,
    );
    push_branch_scope_filter(
        &mut builder,
        branch_scope,
        BranchColumn::new("e.branch_id").map_err(RestError::from_kernel)?,
    );
    if exact {
        builder.push(" AND e.management_no = ");
        builder.push_bind(management_no);
    } else {
        builder.push(" AND ltrim(e.management_no, '0') = ltrim(");
        builder.push_bind(management_no);
        builder.push(", '0')");
    }
    builder.push(" LIMIT 2");
    let org_id = console_kernel_core::OrgId::from_uuid(org);
    with_org_conn::<_, _, RestError>(state.store.pool(), org_id, move |tx| {
        Box::pin(async move { Ok(builder.build().fetch_all(tx.as_mut()).await?) })
    })
    .await
}

async fn lookup_equipment(
    State(state): State<WorkOrderRestState>,
    headers: HeaderMap,
    Query(query): Query<EquipmentLookupQuery>,
) -> Result<Json<EquipmentLookupResponse>, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    authorize_read_access(&principal)?;
    let management_no = normalize_management_no(&query.management_no)?;
    let org = current_org()
        .map_err(KernelError::from)
        .map_err(RestError::from_kernel)?;

    // 1. EXACT match first: a stored `10` and a stored `0010` are DIFFERENT
    //    equipment; an exact hit is always the unambiguous answer.
    let exact_rows = fetch_equipment_lookup_candidates(
        &state,
        *org.as_uuid(),
        &principal.branch_scope,
        management_no.clone(),
        true,
    )
    .await?;
    match exact_rows.as_slice() {
        [row] => return Ok(Json(equipment_lookup_from_row(row)?)),
        [] => {} // fall through to normalized fallback
        _ => {
            return Err(RestError::from_kernel(KernelError::conflict(
                "여러 장비의 관리번호가 같은 번호로 정규화됩니다. 앞자리 0을 포함한 정확한 관리번호를 입력하세요 \
                 (multiple equipment share this normalized management number — enter the exact management_no including any leading zeros)",
            )));
        }
    }

    // 2. Leading-zero-insensitive fallback: `42` typed resolves stored `0042`,
    //    but ONLY when exactly one row matches — never a silent "newest wins" guess.
    let norm_rows = fetch_equipment_lookup_candidates(
        &state,
        *org.as_uuid(),
        &principal.branch_scope,
        management_no,
        false,
    )
    .await?;
    match norm_rows.as_slice() {
        [row] => Ok(Json(equipment_lookup_from_row(row)?)),
        [] => Err(RestError::from_kernel(KernelError::not_found(
            "equipment was not found",
        ))),
        _ => Err(RestError::from_kernel(KernelError::conflict(
            "여러 장비의 관리번호가 같은 번호로 정규화됩니다. 앞자리 0을 포함한 정확한 관리번호를 입력하세요 \
             (multiple equipment share this normalized management number — enter the exact management_no including any leading zeros)",
        ))),
    }
}

async fn autocomplete_equipment(
    State(state): State<WorkOrderRestState>,
    headers: HeaderMap,
    Query(query): Query<EquipmentAutocompleteQuery>,
) -> Result<Json<EquipmentAutocompletePage>, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    authorize_read_access(&principal)?;
    let raw_query = normalize_management_no(&query.q)?;
    let limit = normalize_limit(query.limit, 10, 20)?;
    let org = current_org()
        .map_err(KernelError::from)
        .map_err(RestError::from_kernel)?;
    let prefix = format!("{raw_query}%");
    let mut builder = QueryBuilder::<Postgres>::new(
        r#"
        SELECT
            e.id, e.branch_id, e.equipment_no, e.management_no, e.model, e.status,
            e.specification, e.ton_text, e.maker, e.vin, e.vehicle_registration_no,
            c.id AS customer_id, c.name AS customer_name,
            s.id AS site_id, s.name AS site_name
        FROM registry_equipment e
        JOIN registry_customers c ON c.id = e.customer_id
        JOIN registry_sites s ON s.id = e.site_id
        WHERE
"#,
    );
    push_branch_scope_filter(
        &mut builder,
        &principal.branch_scope,
        BranchColumn::new("e.branch_id").map_err(RestError::from_kernel)?,
    );
    // management_no is leading-zero-insensitive (stored '010' matches typed
    // '10' / '10호기'); equipment_no/model keep the plain normalized prefix so
    // model / equipment-number search still works.
    builder.push(" AND (ltrim(e.management_no, '0') ILIKE ltrim(");
    builder.push_bind(raw_query.clone());
    builder.push(", '0') || '%'");
    builder.push(" OR e.equipment_no ILIKE ");
    builder.push_bind(prefix.clone());
    builder.push(" OR e.model ILIKE ");
    builder.push_bind(prefix);
    builder.push(") ORDER BY e.management_no ASC NULLS LAST, e.updated_at DESC LIMIT ");
    builder.push_bind(limit);
    let rows = with_org_conn::<_, _, RestError>(state.store.pool(), org, move |tx| {
        Box::pin(async move { Ok(builder.build().fetch_all(tx.as_mut()).await?) })
    })
    .await?;
    let items = rows
        .iter()
        .map(equipment_lookup_from_row)
        .collect::<Result<Vec<_>, _>>()?;

    Ok(Json(EquipmentAutocompletePage { items, limit }))
}

async fn reject_work_order(
    State(state): State<WorkOrderRestState>,
    headers: HeaderMap,
    Path(work_order_id): Path<uuid::Uuid>,
    Json(body): Json<RejectWorkOrderRequest>,
) -> Result<impl IntoResponse, RestError> {
    let work_order_id = WorkOrderId::from_uuid(work_order_id);
    let principal = authorize_for_work_order(
        &state,
        &headers,
        work_order_id,
        Action::new(Feature::CompletionReview),
    )
    .await?;
    let summary = state
        .store
        .reject_work_order(RejectWorkOrderCommand {
            actor: principal.user_id,
            work_order_id,
            memo: body.memo,
            trace: TraceContext::generate(),
            occurred_at: time::OffsetDateTime::now_utc(),
        })
        .await
        .map_err(RestError::from_store)?;
    Ok(Json(summary))
}

/// Minimum accepted `Idempotency-Key` length, mirrored by the DB CHECK on
/// `work_order_settlements.idempotency_key` so a short key fails as 422 here
/// rather than a 500 at the constraint.
const MIN_IDEMPOTENCY_KEY_LEN: usize = 16;

fn idempotency_key_from_headers(headers: &HeaderMap) -> Result<String, RestError> {
    let key = headers
        .get("Idempotency-Key")
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .unwrap_or_default();
    if key.len() < MIN_IDEMPOTENCY_KEY_LEN {
        return Err(RestError::from_kernel(KernelError::validation(
            "an Idempotency-Key header of at least 16 characters is required",
        )));
    }
    Ok(key.to_owned())
}

async fn create_settlement(
    State(state): State<WorkOrderRestState>,
    headers: HeaderMap,
    Path(work_order_id): Path<uuid::Uuid>,
    Json(body): Json<CreateSettlementRequest>,
) -> Result<impl IntoResponse, RestError> {
    let work_order_id = WorkOrderId::from_uuid(work_order_id);
    let principal = principal_from_headers(&state, &headers).await?;
    let summary = state
        .store
        .work_order(work_order_id)
        .await
        .map_err(RestError::from_store)?;
    // Reviewer path: CompletionReview on the order's branch. Mechanic path:
    // WorkReportSubmit AND an assignment on this exact order (the assignment is
    // re-checked inside the audited transaction). Deny-by-default otherwise.
    let require_actor_assignment = if authorize(
        &principal,
        Action::new(Feature::CompletionReview),
        summary.branch_id,
    )
    .is_ok()
    {
        false
    } else {
        authorize(
            &principal,
            Action::new(Feature::WorkReportSubmit),
            summary.branch_id,
        )
        .map_err(RestError::from_kernel)?;
        true
    };
    let idempotency_key = idempotency_key_from_headers(&headers)?;
    let request_hash = hex::encode(Sha256::digest(serde_json::to_vec(&body)?));
    let lines = body
        .lines
        .into_iter()
        .map(|line| SettlementLineInput {
            kind: line.kind,
            label: line.label,
            amount_krw: line.amount_krw,
            source_ref: line.source_ref,
            sort_order: line.sort_order,
        })
        .collect();
    let settlement = state
        .store
        .create_settlement(CreateSettlementCommand {
            actor: principal.user_id,
            work_order_id,
            lines,
            note: body.note,
            idempotency_key,
            request_hash,
            require_actor_assignment,
            trace: TraceContext::generate(),
            occurred_at: time::OffsetDateTime::now_utc(),
        })
        .await
        .map_err(RestError::from_store)?;
    Ok((StatusCode::CREATED, Json(settlement)))
}

async fn get_settlement(
    State(state): State<WorkOrderRestState>,
    headers: HeaderMap,
    Path(work_order_id): Path<uuid::Uuid>,
) -> Result<Json<SettlementSummary>, RestError> {
    let work_order_id = WorkOrderId::from_uuid(work_order_id);
    let principal = principal_from_headers(&state, &headers).await?;
    let summary = state
        .store
        .work_order(work_order_id)
        .await
        .map_err(RestError::from_store)?;
    authorize(
        &principal,
        Action::new(Feature::WorkOrderReadAll),
        summary.branch_id,
    )
    .map_err(RestError::from_kernel)?;
    let settlement = state
        .store
        .live_settlement_for_work_order(work_order_id)
        .await
        .map_err(RestError::from_store)?
        .ok_or_else(|| {
            RestError::from_kernel(KernelError::not_found("settlement was not found"))
        })?;
    Ok(Json(settlement))
}

/// A settlement may be submitted by a completion reviewer, or by its creator
/// (who drafted it under `WorkReportSubmit`). Anyone else is denied without
/// revealing whether they merely lack the branch or the feature.
fn authorize_settlement_submitter(
    principal: &Principal,
    settlement: &SettlementSummary,
) -> Result<(), RestError> {
    if authorize(
        principal,
        Action::new(Feature::CompletionReview),
        settlement.branch_id,
    )
    .is_ok()
    {
        return Ok(());
    }
    if settlement.created_by == principal.user_id {
        return authorize(
            principal,
            Action::new(Feature::WorkReportSubmit),
            settlement.branch_id,
        )
        .map_err(RestError::from_kernel);
    }
    Err(RestError::from_kernel(KernelError::forbidden(
        "only the settlement creator or a completion reviewer may submit",
    )))
}

async fn submit_settlement(
    State(state): State<WorkOrderRestState>,
    headers: HeaderMap,
    Path(settlement_id): Path<uuid::Uuid>,
) -> Result<Json<SettlementSummary>, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    let current = state
        .store
        .settlement(settlement_id)
        .await
        .map_err(RestError::from_store)?;
    authorize_settlement_submitter(&principal, &current)?;
    let settlement = state
        .store
        .submit_settlement(SubmitSettlementCommand {
            actor: principal.user_id,
            settlement_id,
            trace: TraceContext::generate(),
            occurred_at: time::OffsetDateTime::now_utc(),
        })
        .await
        .map_err(RestError::from_store)?;
    Ok(Json(settlement))
}

async fn review_settlement(
    State(state): State<WorkOrderRestState>,
    headers: HeaderMap,
    Path(settlement_id): Path<uuid::Uuid>,
    Json(body): Json<ReviewSettlementRequest>,
) -> Result<Json<SettlementSummary>, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    let current = state
        .store
        .settlement(settlement_id)
        .await
        .map_err(RestError::from_store)?;
    authorize(
        &principal,
        Action::new(Feature::CompletionReview),
        current.branch_id,
    )
    .map_err(RestError::from_kernel)?;
    // Four-eyes (approver ≠ submitter) and the RETURNED-requires-comment rule
    // are enforced inside the audited transaction.
    let settlement = state
        .store
        .review_settlement(ReviewSettlementCommand {
            actor: principal.user_id,
            settlement_id,
            decision: body.decision,
            comment: body.comment,
            trace: TraceContext::generate(),
            occurred_at: time::OffsetDateTime::now_utc(),
        })
        .await
        .map_err(RestError::from_store)?;
    Ok(Json(settlement))
}

async fn void_settlement(
    State(state): State<WorkOrderRestState>,
    headers: HeaderMap,
    Path(settlement_id): Path<uuid::Uuid>,
    Json(body): Json<VoidSettlementRequest>,
) -> Result<Json<SettlementSummary>, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    let current = state
        .store
        .settlement(settlement_id)
        .await
        .map_err(RestError::from_store)?;
    authorize(
        &principal,
        Action::new(Feature::CompletionReview),
        current.branch_id,
    )
    .map_err(RestError::from_kernel)?;
    // Void is a correction of financial record state: on top of the feature it
    // requires a built-in admin-tier role, so a custom-role grant of
    // completion_review alone can never erase a settlement.
    if !principal
        .roles
        .iter()
        .any(|role| matches!(role, Role::Admin | Role::SuperAdmin | Role::Executive))
    {
        return Err(RestError::from_kernel(KernelError::forbidden(
            "voiding a settlement requires an admin role",
        )));
    }
    let settlement = state
        .store
        .void_settlement(VoidSettlementCommand {
            actor: principal.user_id,
            settlement_id,
            reason: body.reason,
            trace: TraceContext::generate(),
            occurred_at: time::OffsetDateTime::now_utc(),
        })
        .await
        .map_err(RestError::from_store)?;
    Ok(Json(settlement))
}

async fn create_work_order(
    State(state): State<WorkOrderRestState>,
    headers: HeaderMap,
    Json(body): Json<CreateWorkOrderRequest>,
) -> Result<impl IntoResponse, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    authorize(
        &principal,
        Action::limited(Feature::WorkOrderCreate),
        body.branch_id,
    )
    .map_err(RestError::from_kernel)?;
    let actor = principal.user_id;
    let summary = state
        .store
        .create_work_order(CreateWorkOrderCommand {
            actor,
            branch_id: body.branch_id,
            management_no: body.management_no,
            symptom: body.symptom,
            customer_request: body.customer_request,
            target_due_at: body.target_due_at,
            maintenance_type: body.maintenance_type,
            maintenance_cause: body.maintenance_cause,
            trace: TraceContext::generate(),
            occurred_at: time::OffsetDateTime::now_utc(),
        })
        .await
        .map_err(RestError::from_store)?;
    Ok((StatusCode::CREATED, Json(summary)))
}

async fn get_work_order(
    State(state): State<WorkOrderRestState>,
    headers: HeaderMap,
    Path(work_order_id): Path<uuid::Uuid>,
) -> Result<impl IntoResponse, RestError> {
    let work_order_id = WorkOrderId::from_uuid(work_order_id);
    let principal = principal_from_headers(&state, &headers).await?;
    let summary = state
        .store
        .work_order(work_order_id)
        .await
        .map_err(RestError::from_store)?;
    authorize(
        &principal,
        Action::new(Feature::WorkOrderReadAll),
        summary.branch_id,
    )
    .map_err(RestError::from_kernel)?;
    Ok(Json(summary))
}

async fn update_work_order_intake(
    State(state): State<WorkOrderRestState>,
    headers: HeaderMap,
    Path(work_order_id): Path<uuid::Uuid>,
    Json(body): Json<UpdateWorkOrderIntakeRequest>,
) -> Result<impl IntoResponse, RestError> {
    let work_order_id = WorkOrderId::from_uuid(work_order_id);
    let principal = authorize_for_work_order(
        &state,
        &headers,
        work_order_id,
        Action::new(Feature::WorkOrderEditIntake),
    )
    .await?;
    let summary = state
        .store
        .update_work_order_intake(UpdateWorkOrderIntakeCommand {
            actor: principal.user_id,
            work_order_id,
            symptom: body.symptom,
            customer_request: body.customer_request,
            maintenance_type: body.maintenance_type,
            maintenance_cause: body.maintenance_cause,
            trace: TraceContext::generate(),
            occurred_at: time::OffsetDateTime::now_utc(),
        })
        .await
        .map_err(RestError::from_store)?;
    Ok(Json(summary))
}

async fn update_priority(
    State(state): State<WorkOrderRestState>,
    headers: HeaderMap,
    Path(work_order_id): Path<uuid::Uuid>,
    Json(body): Json<UpdatePriorityRequest>,
) -> Result<impl IntoResponse, RestError> {
    let work_order_id = WorkOrderId::from_uuid(work_order_id);
    let principal = authorize_for_work_order(
        &state,
        &headers,
        work_order_id,
        Action::new(Feature::PriorityManage),
    )
    .await?;
    let summary = state
        .store
        .update_priority(UpdatePriorityCommand {
            actor: principal.user_id,
            work_order_id,
            priority: body.priority,
            trace: TraceContext::generate(),
            occurred_at: time::OffsetDateTime::now_utc(),
        })
        .await
        .map_err(RestError::from_store)?;
    Ok(Json(summary))
}

async fn assign_work_order(
    State(state): State<WorkOrderRestState>,
    headers: HeaderMap,
    Path(work_order_id): Path<uuid::Uuid>,
    Json(body): Json<AssignWorkOrderRequest>,
) -> Result<impl IntoResponse, RestError> {
    let work_order_id = WorkOrderId::from_uuid(work_order_id);
    let principal = authorize_for_work_order(
        &state,
        &headers,
        work_order_id,
        Action::new(Feature::AssigneeManage),
    )
    .await?;
    let assignments = body
        .assignments
        .into_iter()
        .map(|assignment| AssignmentInput {
            mechanic_id: assignment.mechanic_id,
            role: assignment.role,
        })
        .collect();
    let summary = state
        .store
        .assign_work_order(WorkOrderAssignmentCommand {
            actor: principal.user_id,
            work_order_id,
            assignments,
            admin_approver_id: body.admin_approver_id,
            executive_approver_id: body.executive_approver_id,
            trace: TraceContext::generate(),
            occurred_at: time::OffsetDateTime::now_utc(),
        })
        .await
        .map_err(RestError::from_store)?;
    Ok(Json(summary))
}

async fn start_work(
    State(state): State<WorkOrderRestState>,
    headers: HeaderMap,
    Path(work_order_id): Path<uuid::Uuid>,
) -> Result<impl IntoResponse, RestError> {
    let work_order_id = WorkOrderId::from_uuid(work_order_id);
    let principal = authorize_for_work_order(
        &state,
        &headers,
        work_order_id,
        Action::new(Feature::WorkOrderStart),
    )
    .await?;
    let summary = state
        .store
        .start_work(WorkOrderStartCommand {
            actor: principal.user_id,
            work_order_id,
            trace: TraceContext::generate(),
            occurred_at: time::OffsetDateTime::now_utc(),
        })
        .await
        .map_err(RestError::from_store)?;
    Ok(Json(summary))
}

async fn submit_report(
    State(state): State<WorkOrderRestState>,
    headers: HeaderMap,
    Path(work_order_id): Path<uuid::Uuid>,
    Json(body): Json<SubmitReportRequest>,
) -> Result<impl IntoResponse, RestError> {
    let work_order_id = WorkOrderId::from_uuid(work_order_id);
    let principal = authorize_for_work_order(
        &state,
        &headers,
        work_order_id,
        Action::new(Feature::WorkReportSubmit),
    )
    .await?;
    let summary = state
        .store
        .submit_report(SubmitReportCommand {
            actor: principal.user_id,
            work_order_id,
            result_type: body.result_type,
            diagnosis: body.diagnosis,
            action_taken: body.action_taken,
            trace: TraceContext::generate(),
            occurred_at: time::OffsetDateTime::now_utc(),
        })
        .await
        .map_err(RestError::from_store)?;
    Ok(Json(summary))
}

async fn approve_work_order(
    State(state): State<WorkOrderRestState>,
    headers: HeaderMap,
    Path(work_order_id): Path<uuid::Uuid>,
    Json(body): Json<ApproveWorkOrderRequest>,
) -> Result<impl IntoResponse, RestError> {
    let work_order_id = WorkOrderId::from_uuid(work_order_id);
    let principal = authorize_for_work_order(
        &state,
        &headers,
        work_order_id,
        Action::new(Feature::CompletionReview),
    )
    .await?;
    let summary = state
        .store
        .approve_work_order(WorkOrderApprovalCommand {
            actor: principal.user_id,
            work_order_id,
            comment: body.comment,
            trace: TraceContext::generate(),
            occurred_at: time::OffsetDateTime::now_utc(),
        })
        .await
        .map_err(RestError::from_store)?;

    // Domain-event publish (BE-AUTO slice 1). Only the executive approval that
    // reaches FINAL_COMPLETED is a candidate — admin → AdminReview never touches
    // the runtime. The legacy completion above already committed (work_orders
    // status + audit) in its own transaction; this publish is purely additive.
    // The dispatcher evaluates an ordered binding list: (1) the built-in M2
    // completion-tail strangler — flag-gated, byte-identical to the previous
    // inline start (dark default = one read-only SELECT, nothing written) —
    // then (2) every enabled workflow_trigger_bindings rule for
    // 'work_order.completed'. Legacy-first ordering means a trigger failure
    // never fails a request the tenant already saw succeed — it is logged and
    // the payroll draft is eventually restaged by the outbox drainer.
    if summary.status == WorkOrderStatus::FinalCompleted
        && let Some(runtime) = state.workflow_runtime.as_ref()
        && let Err(err) = workflow_triggers::publish_work_order_completed(
            runtime,
            &principal,
            summary.branch_id,
            work_order_id,
        )
        .await
    {
        tracing::warn!(
            error = %err.message,
            work_order_id = %work_order_id,
            "workflow triggers: work_order.completed publish failed (completion already persisted; drainer will restage payroll)"
        );
    }

    Ok(Json(summary))
}

async fn approve_mobile_work_order<S>(
    State(state): State<MobileRestState<S>>,
    headers: HeaderMap,
    Path(work_order_id): Path<uuid::Uuid>,
    Json(body): Json<MobileApproveWorkOrderRequest>,
) -> Result<impl IntoResponse, RestError>
where
    S: S3ObjectStore + Clone + Send + Sync + 'static,
{
    let work_order_id = WorkOrderId::from_uuid(work_order_id);
    let principal = mobile_principal_from_headers(&state, &headers).await?;
    let current = state
        .store
        .work_order(work_order_id)
        .await
        .map_err(RestError::from_store)?;
    authorize(
        &principal,
        Action::new(Feature::CompletionReview),
        current.branch_id,
    )
    .map_err(RestError::from_kernel)?;

    verify_mobile_approval_step_up(
        &state,
        &principal,
        work_order_id,
        body.step_up.ok_or_else(|| {
            RestError::precondition_required(
                "passkey_step_up_required",
                "mobile work-order approval requires a fresh passkey step-up",
            )
        })?,
    )
    .await?;

    let summary = state
        .store
        .approve_work_order(WorkOrderApprovalCommand {
            actor: principal.user_id,
            work_order_id,
            comment: body.comment,
            trace: TraceContext::generate(),
            occurred_at: time::OffsetDateTime::now_utc(),
        })
        .await
        .map_err(RestError::from_store)?;

    if summary.status == WorkOrderStatus::FinalCompleted
        && let Some(runtime) = state.workflow_runtime.as_ref()
        && let Err(err) = m2_strangler::drive_completion_if_enabled(
            runtime,
            &principal,
            summary.branch_id,
            work_order_id,
        )
        .await
    {
        tracing::warn!(
            error = %err.message,
            work_order_id = %work_order_id,
            "m2 strangler: runtime completion record failed after mobile approval (completion already persisted; drainer will restage payroll)"
        );
    }

    Ok(Json(summary))
}

async fn verify_mobile_approval_step_up<S>(
    state: &MobileRestState<S>,
    principal: &Principal,
    work_order_id: WorkOrderId,
    step_up: MobilePasskeyStepUpEnvelope,
) -> Result<(), RestError> {
    step_up
        .binding
        .validate()
        .map_err(|err| RestError::from_kernel(KernelError::validation(err.to_string())))?;
    let expected_binding = MobilePasskeyStepUpBinding::approval_decision(
        *work_order_id.as_uuid(),
        step_up.binding.replay_attempt,
    );
    let verifier = state.passkey_step_up.as_ref().ok_or_else(|| {
        RestError::unavailable("passkey step-up is not configured for mobile work-order API")
    })?;
    verifier
        .verify_mobile_step_up_for_user(
            &state.pool,
            step_up,
            *principal.user_id.as_uuid(),
            &expected_binding,
        )
        .await
        .map_err(rest_error_from_mobile_step_up)
}

fn rest_error_from_mobile_step_up(error: MobilePasskeyStepUpVerificationError) -> RestError {
    match error {
        MobilePasskeyStepUpVerificationError::BindingMismatch => RestError::passkey_step_up_failed(
            "passkey_step_up_binding_mismatch",
            "passkey step-up binding does not match the requested action",
        ),
        MobilePasskeyStepUpVerificationError::Auth(err) => {
            RestError::passkey_step_up_failed("passkey_step_up_failed", err.to_string())
        }
    }
}

async fn request_target_change(
    State(state): State<WorkOrderRestState>,
    headers: HeaderMap,
    Path(work_order_id): Path<uuid::Uuid>,
    Json(body): Json<TargetChangeRequestBody>,
) -> Result<impl IntoResponse, RestError> {
    let work_order_id = WorkOrderId::from_uuid(work_order_id);
    let principal = authorize_for_work_order(
        &state,
        &headers,
        work_order_id,
        Action::request(Feature::TargetManage),
    )
    .await?;
    let summary = state
        .store
        .request_target_change(TargetChangeRequestCommand {
            actor: principal.user_id,
            work_order_id,
            requested_target_due_at: body.requested_target_due_at,
            reason: body.reason,
            trace: TraceContext::generate(),
            occurred_at: time::OffsetDateTime::now_utc(),
        })
        .await
        .map_err(RestError::from_store)?;
    Ok((StatusCode::CREATED, Json(summary)))
}

async fn review_target_change(
    State(state): State<WorkOrderRestState>,
    headers: HeaderMap,
    Path(request_id): Path<uuid::Uuid>,
    Json(body): Json<ReviewTargetChangeRequestBody>,
) -> Result<impl IntoResponse, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    let target = state
        .store
        .target_change_request(request_id)
        .await
        .map_err(RestError::from_store)?;
    authorize(
        &principal,
        Action::new(Feature::TargetManage),
        target.branch_id,
    )
    .map_err(RestError::from_kernel)?;
    let summary = state
        .store
        .review_target_change(ReviewTargetChangeCommand {
            actor: principal.user_id,
            request_id,
            decision: body.decision,
            memo: body.memo,
            trace: TraceContext::generate(),
            occurred_at: time::OffsetDateTime::now_utc(),
        })
        .await
        .map_err(RestError::from_store)?;
    Ok(Json(summary))
}

async fn list_daily_plans(
    State(state): State<WorkOrderRestState>,
    headers: HeaderMap,
    Query(query): Query<ListDailyPlansQuery>,
) -> Result<Json<DailyPlanListResponse>, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    authorize_daily_plan_list(&principal)?;
    let plan_date = query
        .plan_date
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| {
            time::Date::parse(
                value,
                time::macros::format_description!("[year]-[month]-[day]"),
            )
            .map_err(|_| RestError::bad_request("plan_date must use YYYY-MM-DD"))
        })
        .transpose()?;
    // Admins triage the queue org-wide so DRAFT/REQUESTED plans in any branch are
    // reachable (#19.17); other roles keep their explicit branch set.
    let branch_scope = work_order_list_scope(&principal);
    let page = state
        .store
        .list_daily_plans(DailyPlanListQuery {
            branch_scope,
            plan_date,
        })
        .await
        .map_err(RestError::from_store)?;
    Ok(Json(DailyPlanListResponse { items: page.items }))
}

async fn create_daily_plan(
    State(state): State<WorkOrderRestState>,
    headers: HeaderMap,
    Json(body): Json<CreateDailyPlanRequest>,
) -> Result<impl IntoResponse, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    authorize(
        &principal,
        Action::new(Feature::DailyPlanRequest),
        body.branch_id,
    )
    .map_err(RestError::from_kernel)?;
    let items = body
        .items
        .into_iter()
        .map(|item| DailyPlanItemInput {
            work_order_id: item.work_order_id,
            description: item.description,
        })
        .collect();
    let summary = state
        .store
        .create_daily_plan(CreateDailyPlanCommand {
            actor: principal.user_id,
            branch_id: body.branch_id,
            mechanic_id: body.mechanic_id,
            plan_date: body.plan_date,
            items,
            trace: TraceContext::generate(),
            occurred_at: time::OffsetDateTime::now_utc(),
        })
        .await
        .map_err(RestError::from_store)?;
    Ok((StatusCode::CREATED, Json(summary)))
}

async fn request_daily_plan_review(
    State(state): State<WorkOrderRestState>,
    headers: HeaderMap,
    Path(plan_id): Path<uuid::Uuid>,
) -> Result<impl IntoResponse, RestError> {
    transition_daily_plan_endpoint(
        state,
        headers,
        DailyPlanId::from_uuid(plan_id),
        Action::new(Feature::DailyPlanRequest),
        DailyPlanEndpointAction::RequestReview,
    )
    .await
}

async fn get_daily_plan(
    State(state): State<WorkOrderRestState>,
    headers: HeaderMap,
    Path(plan_id): Path<uuid::Uuid>,
) -> Result<impl IntoResponse, RestError> {
    let plan_id = DailyPlanId::from_uuid(plan_id);
    let principal = principal_from_headers(&state, &headers).await?;
    let plan = state
        .store
        .daily_plan(plan_id)
        .await
        .map_err(RestError::from_store)?;
    // Both the creating mechanic and the reviewing admin hold DailyPlanRequest
    // (Mechanic/Admin = Allow), so it scopes the read to the plan's branch
    // without granting it to roles that have no business in the plan flow.
    authorize(
        &principal,
        Action::new(Feature::DailyPlanRequest),
        plan.branch_id,
    )
    .map_err(RestError::from_kernel)?;
    Ok(Json(plan))
}

async fn review_daily_plan(
    State(state): State<WorkOrderRestState>,
    headers: HeaderMap,
    Path(plan_id): Path<uuid::Uuid>,
    Json(body): Json<ReviewDailyPlanRequestBody>,
) -> Result<impl IntoResponse, RestError> {
    let plan_id = DailyPlanId::from_uuid(plan_id);
    let principal = principal_from_headers(&state, &headers).await?;
    let plan = state
        .store
        .daily_plan(plan_id)
        .await
        .map_err(RestError::from_store)?;
    authorize(
        &principal,
        Action::new(Feature::DailyPlanReview),
        plan.branch_id,
    )
    .map_err(RestError::from_kernel)?;
    let summary = state
        .store
        .review_daily_plan(ReviewDailyPlanCommand {
            actor: principal.user_id,
            plan_id,
            decision: body.decision,
            memo: body.memo,
            trace: TraceContext::generate(),
            occurred_at: time::OffsetDateTime::now_utc(),
        })
        .await
        .map_err(RestError::from_store)?;
    Ok(Json(summary))
}

async fn confirm_daily_plan(
    State(state): State<WorkOrderRestState>,
    headers: HeaderMap,
    Path(plan_id): Path<uuid::Uuid>,
) -> Result<impl IntoResponse, RestError> {
    transition_daily_plan_endpoint(
        state,
        headers,
        DailyPlanId::from_uuid(plan_id),
        Action::new(Feature::DailyPlanRequest),
        DailyPlanEndpointAction::Confirm,
    )
    .await
}

async fn create_outsource_work(
    State(state): State<WorkOrderRestState>,
    headers: HeaderMap,
    Path(work_order_id): Path<uuid::Uuid>,
    Json(body): Json<CreateOutsourceWorkRequest>,
) -> Result<impl IntoResponse, RestError> {
    let work_order_id = WorkOrderId::from_uuid(work_order_id);
    let principal = authorize_for_work_order(
        &state,
        &headers,
        work_order_id,
        Action::new(Feature::PriorityManage),
    )
    .await?;
    let summary = state
        .store
        .create_outsource_work(CreateOutsourceWorkCommand {
            actor: principal.user_id,
            work_order_id,
            vendor_name: body.vendor_name,
            vendor_contact: body.vendor_contact,
            reason: body.reason,
            trace: TraceContext::generate(),
            occurred_at: time::OffsetDateTime::now_utc(),
        })
        .await
        .map_err(RestError::from_store)?;
    Ok((StatusCode::CREATED, Json(summary)))
}

enum DailyPlanEndpointAction {
    RequestReview,
    Confirm,
}

async fn transition_daily_plan_endpoint(
    state: WorkOrderRestState,
    headers: HeaderMap,
    plan_id: DailyPlanId,
    action: Action,
    endpoint_action: DailyPlanEndpointAction,
) -> Result<Response, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    let plan = state
        .store
        .daily_plan(plan_id)
        .await
        .map_err(RestError::from_store)?;
    authorize(&principal, action, plan.branch_id).map_err(RestError::from_kernel)?;
    let command = SendDailyPlanForReviewCommand {
        actor: principal.user_id,
        plan_id,
        trace: TraceContext::generate(),
        occurred_at: time::OffsetDateTime::now_utc(),
    };
    let summary = match endpoint_action {
        DailyPlanEndpointAction::RequestReview => {
            state.store.request_daily_plan_review(command).await
        }
        DailyPlanEndpointAction::Confirm => state.store.confirm_daily_plan(command).await,
    }
    .map_err(RestError::from_store)?;
    Ok(Json(summary).into_response())
}

fn authorize_read_access(principal: &Principal) -> Result<(), RestError> {
    authorize_feature_in_scope(principal, Feature::WorkOrderReadAll)
}

/// Authorize the daily-plan LIST: the queue is a MECHANIC-requests / ADMIN-reviews
/// flow, so visibility is gated on holding `DailyPlanRequest` OR `DailyPlanReview`
/// — NOT the broad `WorkOrderReadAll` (which RECEPTIONIST + EXECUTIVE also pass).
/// A RECEPTIONIST or EXECUTIVE with no daily-plan permission gets a 403. Branch
/// filtering still happens via `work_order_list_scope`.
fn authorize_daily_plan_list(principal: &Principal) -> Result<(), RestError> {
    // The daily-plan request/review participants OR an org-wide queue triager
    // (EXECUTIVE / SUPER_ADMIN, via OrgWideQueueTriage) may read the queue — the
    // latter for org-wide oversight, mirroring their work-order-queue visibility.
    // RECEPTIONIST (none of these capabilities) stays excluded.
    authorize_feature_in_scope(principal, Feature::DailyPlanRequest)
        .or_else(|_| authorize_feature_in_scope(principal, Feature::DailyPlanReview))
        .or_else(|_| authorize_feature_in_scope(principal, Feature::OrgWideQueueTriage))
}

/// Resolve the exact approval sources this principal may see. This is
/// intentionally stricter than generic work-order read: mechanics may read their
/// own work but must not see the manager/admin approval inbox. Source visibility
/// follows source review capabilities; `OrgWideQueueTriage` widens both branch
/// scope and source visibility for executive triage. Source-specific actions
/// still re-authorize on their own endpoints before mutating state.
fn approval_source_visibility(
    principal: &Principal,
) -> Result<ApprovalSourceVisibility, RestError> {
    let org_wide = feature_allowed_in_scope(principal, Feature::OrgWideQueueTriage);
    let visibility = ApprovalSourceVisibility {
        work_orders: org_wide || feature_allowed_in_scope(principal, Feature::CompletionReview),
        daily_plans: org_wide || feature_allowed_in_scope(principal, Feature::DailyPlanReview),
        target_changes: org_wide || feature_allowed_in_scope(principal, Feature::TargetManage),
    };
    if visibility.any() {
        Ok(visibility)
    } else {
        Err(RestError::from_kernel(KernelError::forbidden(
            "role is not allowed to use approval items",
        )))
    }
}

fn feature_allowed_in_scope(principal: &Principal, feature: Feature) -> bool {
    authorize_feature_in_scope(principal, feature).is_ok()
}

/// Authorize a LIST/visibility feature. These gates decide what the caller may
/// see across its whole `branch_scope` — `work_order_list_scope` and
/// `fetch_approval_source_counts` do the row confinement — so there is no single
/// resource branch to authorize against. Every row-level site in this file passes
/// the real `work_orders.branch_id` / `daily_work_plans.branch_id` through
/// [`authorize`] and must keep doing so.
///
/// NOT `authorize_org_wide`: `CompletionReview` is `[D,D,D,A,D,A]`, so org-wide
/// routing would collapse the approval inbox to SUPER_ADMIN and lock out every
/// ADMIN who uses it today.
fn authorize_feature_in_scope(principal: &Principal, feature: Feature) -> Result<(), RestError> {
    authorize_capability(principal, Action::new(feature)).map_err(RestError::from_kernel)
}

async fn fetch_approval_source_counts(
    pool: &PgPool,
    branch_scope: &BranchScope,
    visibility: ApprovalSourceVisibility,
    actor: UserId,
) -> Result<BTreeMap<String, i64>, RestError> {
    let org = current_org()
        .map_err(KernelError::from)
        .map_err(RestError::from_kernel)?;
    let mut builder =
        QueryBuilder::<Postgres>::new("SELECT source, COUNT(*)::BIGINT AS count FROM (");
    push_approval_federation_union(&mut builder, branch_scope, visibility, actor)?;
    builder.push(") approval GROUP BY source");
    let rows = with_org_conn::<_, _, RestError>(pool, org, move |tx| {
        Box::pin(async move { Ok(builder.build().fetch_all(tx.as_mut()).await?) })
    })
    .await?;

    rows.iter()
        .map(|row| {
            Ok((
                row.try_get::<String, _>("source")?,
                row.try_get::<i64, _>("count")?,
            ))
        })
        .collect()
}

async fn fetch_approval_rows(
    pool: &PgPool,
    branch_scope: &BranchScope,
    visibility: ApprovalSourceVisibility,
    actor: UserId,
    query: &NormalizedApprovalItemsQuery,
) -> Result<Vec<sqlx::postgres::PgRow>, RestError> {
    let org = current_org()
        .map_err(KernelError::from)
        .map_err(RestError::from_kernel)?;
    let mut builder = QueryBuilder::<Postgres>::new(
        r#"
        SELECT
            approval.source,
            approval.source_id,
            approval.org_id AS approval_org_id,
            approval.branch_id AS approval_branch_id,
            approval.status AS approval_status,
            approval.requested_at AS approval_requested_at,
            approval.due_at AS approval_due_at,
            w.id AS id,
            w.request_no,
            w.branch_id,
            w.status,
            w.priority,
            w.result_type,
            w.maintenance_type,
            w.maintenance_cause,
            w.target_due_at,
            w.created_at,
            w.updated_at,
            e.id AS equipment_id,
            e.equipment_no,
            e.management_no,
            e.model,
            e.status AS equipment_status,
            e.specification,
            e.ton_text,
            c.id AS customer_id,
            c.name AS customer_name,
            s.id AS site_id,
            s.name AS site_name,
            s.contact_name AS site_contact_name,
            s.contact_phone AS site_contact_phone,
            s.contact_email AS site_contact_email,
            d.id AS daily_plan_id,
            d.branch_id AS daily_branch_id,
            d.mechanic_id AS daily_mechanic_id,
            d.plan_date AS daily_plan_date,
            d.status AS daily_status,
            t.id AS target_change_id,
            t.work_order_id AS target_work_order_id,
            t.requested_target_due_at AS target_requested_target_due_at,
            t.status AS target_status
        FROM (
        "#,
    );
    push_approval_federation_union(&mut builder, branch_scope, visibility, actor)?;
    builder.push(
        r#"
        ) approval
        LEFT JOIN work_orders w
          ON approval.source = 'WORK_ORDER'
         AND w.id = approval.source_id
        LEFT JOIN registry_equipment e
          ON approval.source = 'WORK_ORDER'
         AND e.id = w.equipment_id
        LEFT JOIN registry_customers c
          ON approval.source = 'WORK_ORDER'
         AND c.id = w.customer_id
        LEFT JOIN registry_sites s
          ON approval.source = 'WORK_ORDER'
         AND s.id = w.site_id
        LEFT JOIN daily_work_plans d
          ON approval.source = 'DAILY_PLAN'
         AND d.id = approval.source_id
        LEFT JOIN target_change_requests t
          ON approval.source = 'TARGET_CHANGE'
         AND t.id = approval.source_id
        ORDER BY
            approval.due_at ASC NULLS LAST,
            approval.requested_at ASC NULLS LAST,
            approval.sort_rank ASC,
            approval.source_id ASC
        LIMIT
        "#,
    );
    builder.push_bind(query.limit);
    builder.push(" OFFSET ");
    builder.push_bind(query.offset);

    with_org_conn::<_, _, RestError>(pool, org, move |tx| {
        Box::pin(async move { Ok(builder.build().fetch_all(tx.as_mut()).await?) })
    })
    .await
}

fn push_approval_federation_union(
    builder: &mut QueryBuilder<Postgres>,
    branch_scope: &BranchScope,
    visibility: ApprovalSourceVisibility,
    actor: UserId,
) -> Result<(), RestError> {
    let mut pushed = false;
    if visibility.work_orders {
        push_approval_union_separator(builder, &mut pushed);
        builder.push(
            r#"
        SELECT
            'WORK_ORDER'::TEXT AS source,
            w.id AS source_id,
            w.org_id,
            w.branch_id,
            w.status,
            COALESCE(pending_step.requested_at, w.report_submitted_at, w.updated_at, w.created_at)
                AS requested_at,
            w.target_due_at AS due_at,
            1 AS sort_rank
        FROM work_orders w
        LEFT JOIN LATERAL (
            SELECT step.requested_at, step.approver_id
            FROM work_order_approval_steps step
            WHERE step.work_order_id = w.id
              AND step.role IN ('ADMIN', 'EXECUTIVE')
              AND step.status = 'PENDING'
            ORDER BY step.step_order ASC
            LIMIT 1
        ) pending_step ON TRUE
        WHERE w.status IN ('REPORT_SUBMITTED', 'ADMIN_REVIEW')
          AND pending_step.approver_id =
        "#,
        );
        builder.push_bind(*actor.as_uuid());
        builder.push(
            r#"
          AND
        "#,
        );
        push_branch_scope_filter(
            builder,
            branch_scope,
            BranchColumn::new("w.branch_id").map_err(RestError::from_kernel)?,
        );
    }
    if visibility.daily_plans {
        push_approval_union_separator(builder, &mut pushed);
        builder.push(
            r#"
        SELECT
            'DAILY_PLAN'::TEXT AS source,
            d.id AS source_id,
            d.org_id,
            d.branch_id,
            d.status,
            COALESCE(d.requested_at, d.updated_at, d.created_at) AS requested_at,
            (d.plan_date::TIMESTAMP AT TIME ZONE 'Asia/Seoul') AS due_at,
            2 AS sort_rank
        FROM daily_work_plans d
        WHERE d.status = 'REQUESTED'
          AND
        "#,
        );
        push_branch_scope_filter(
            builder,
            branch_scope,
            BranchColumn::new("d.branch_id").map_err(RestError::from_kernel)?,
        );
    }
    if visibility.target_changes {
        push_approval_union_separator(builder, &mut pushed);
        builder.push(
            r#"
        SELECT
            'TARGET_CHANGE'::TEXT AS source,
            t.id AS source_id,
            w.org_id,
            w.branch_id,
            t.status,
            t.created_at AS requested_at,
            t.requested_target_due_at AS due_at,
            3 AS sort_rank
        FROM target_change_requests t
        JOIN work_orders w ON w.id = t.work_order_id
        WHERE t.status = 'REQUESTED'
          AND
        "#,
        );
        push_branch_scope_filter(
            builder,
            branch_scope,
            BranchColumn::new("w.branch_id").map_err(RestError::from_kernel)?,
        );
    }
    debug_assert!(
        pushed,
        "approval visibility must include at least one source"
    );
    Ok(())
}

fn push_approval_union_separator(builder: &mut QueryBuilder<Postgres>, pushed: &mut bool) {
    if *pushed {
        builder.push(" UNION ALL ");
    }
    *pushed = true;
}

fn approval_sources_from_counts(
    counts: &BTreeMap<String, i64>,
    visibility: ApprovalSourceVisibility,
) -> Vec<ApprovalItemSource> {
    fn count(counts: &BTreeMap<String, i64>, source: &str) -> i64 {
        counts.get(source).copied().unwrap_or(0)
    }

    let mut sources = Vec::new();
    if visibility.work_orders {
        sources.push(ApprovalItemSource {
            key: "workOrders",
            label: "작업 보고",
            status: "ok",
            count: count(counts, "WORK_ORDER"),
        });
    }
    if visibility.daily_plans {
        sources.push(ApprovalItemSource {
            key: "dailyPlans",
            label: "계획업무",
            status: "ok",
            count: count(counts, "DAILY_PLAN"),
        });
    }
    if visibility.target_changes {
        sources.push(ApprovalItemSource {
            key: "targetChanges",
            label: "일정 변경",
            status: "ok",
            count: count(counts, "TARGET_CHANGE"),
        });
    }
    sources
}

fn parse_work_order_list_query(
    raw_query: Option<&str>,
    actor: UserId,
) -> Result<NormalizedWorkOrderListQuery, RestError> {
    let mut query = WorkOrderListQuery::default();
    if let Some(raw_query) = raw_query {
        for pair in raw_query.split('&').filter(|pair| !pair.is_empty()) {
            let (raw_key, raw_value) = pair.split_once('=').unwrap_or((pair, ""));
            let key = decode_query_component(raw_key)?;
            let value = decode_query_component(raw_value)?;
            match key.as_str() {
                "status" | "status[]" => query.status.push(value),
                "priority" | "priority[]" => query.priority.push(value),
                "maintenance_type" | "maintenance_type[]" => query.maintenance_type.push(value),
                "maintenance_cause" | "maintenance_cause[]" => query.maintenance_cause.push(value),
                "assigned_to" => query.assigned_to = non_empty_query_value(value),
                "customer_id" => query.customer_id = parse_uuid_query_value("customer_id", value)?,
                "site_id" => query.site_id = parse_uuid_query_value("site_id", value)?,
                "branch_id" => query.branch_id = parse_uuid_query_value("branch_id", value)?,
                "equipment_id" => {
                    query.equipment_id = parse_uuid_query_value("equipment_id", value)?;
                }
                "around_work_order_id" | "search_around_work_order_id" => {
                    query.around_work_order_id =
                        parse_uuid_query_value("around_work_order_id", value)?;
                }
                "target_due_from" => {
                    query.target_due_from = parse_datetime_query_value("target_due_from", value)?;
                }
                "target_due_to" => {
                    query.target_due_to = parse_datetime_query_value("target_due_to", value)?;
                }
                "limit" => query.limit = parse_i64_query_value("limit", value)?,
                "offset" => query.offset = parse_i64_query_value("offset", value)?,
                _ => {}
            }
        }
    }
    Ok(NormalizedWorkOrderListQuery {
        statuses: normalize_status_filters(query.status)?,
        priorities: normalize_priority_filters(query.priority)?,
        maintenance_types: normalize_maintenance_type_filters(query.maintenance_type)?,
        maintenance_causes: normalize_maintenance_cause_filters(query.maintenance_cause)?,
        assigned_to: normalize_assigned_to(query.assigned_to, actor)?,
        customer_id: query.customer_id,
        site_id: query.site_id,
        branch_id: query.branch_id,
        equipment_id: query.equipment_id,
        around_work_order_id: query.around_work_order_id,
        target_due_from: query.target_due_from,
        target_due_to: query.target_due_to,
        limit: normalize_limit(query.limit, 50, 100)?,
        offset: normalize_offset(query.offset)?,
    })
}

fn normalize_status_filters(raw: Vec<String>) -> Result<Vec<String>, RestError> {
    let mut statuses = Vec::new();
    for value in expand_query_values(raw) {
        let normalized = value.to_ascii_uppercase();
        let status = WorkOrderStatus::from_db_str(&normalized).map_err(RestError::from_kernel)?;
        statuses.push(status.as_db_str().to_owned());
    }
    Ok(statuses)
}

fn normalize_priority_filters(raw: Vec<String>) -> Result<Vec<String>, RestError> {
    let mut priorities = Vec::new();
    for value in expand_query_values(raw) {
        let normalized = value.to_ascii_uppercase();
        let priority = PriorityLevel::from_db_str(&normalized).map_err(RestError::from_kernel)?;
        priorities.push(priority.as_db_str().to_owned());
    }
    Ok(priorities)
}

fn normalize_maintenance_type_filters(raw: Vec<String>) -> Result<Vec<String>, RestError> {
    let mut types = Vec::new();
    for value in expand_query_values(raw) {
        let normalized = value.to_ascii_uppercase();
        let maintenance_type =
            MaintenanceType::from_db_str(&normalized).map_err(RestError::from_kernel)?;
        types.push(maintenance_type.as_db_str().to_owned());
    }
    Ok(types)
}

fn normalize_maintenance_cause_filters(raw: Vec<String>) -> Result<Vec<String>, RestError> {
    let mut causes = Vec::new();
    for value in expand_query_values(raw) {
        let normalized = value.to_ascii_uppercase();
        let maintenance_cause =
            MaintenanceCause::from_db_str(&normalized).map_err(RestError::from_kernel)?;
        causes.push(maintenance_cause.as_db_str().to_owned());
    }
    Ok(causes)
}

fn expand_query_values(raw: Vec<String>) -> Vec<String> {
    raw.into_iter()
        .flat_map(|value| {
            value
                .split(',')
                .map(str::trim)
                .filter(|part| !part.is_empty())
                .map(str::to_owned)
                .collect::<Vec<_>>()
        })
        .collect()
}

fn non_empty_query_value(value: String) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_owned())
    }
}

fn parse_uuid_query_value(name: &str, value: String) -> Result<Option<uuid::Uuid>, RestError> {
    let Some(value) = non_empty_query_value(value) else {
        return Ok(None);
    };
    uuid::Uuid::parse_str(&value).map(Some).map_err(|_| {
        RestError::from_kernel(KernelError::validation(format!("{name} must be a UUID")))
    })
}

fn parse_datetime_query_value(
    name: &str,
    value: String,
) -> Result<Option<time::OffsetDateTime>, RestError> {
    let Some(value) = non_empty_query_value(value) else {
        return Ok(None);
    };
    time::OffsetDateTime::parse(&value, &Rfc3339)
        .map(Some)
        .map_err(|_| {
            RestError::from_kernel(KernelError::validation(format!(
                "{name} must be an RFC 3339 timestamp"
            )))
        })
}

fn parse_i64_query_value(name: &str, value: String) -> Result<Option<i64>, RestError> {
    let Some(value) = non_empty_query_value(value) else {
        return Ok(None);
    };
    value.parse::<i64>().map(Some).map_err(|_| {
        RestError::from_kernel(KernelError::validation(format!(
            "{name} must be an integer"
        )))
    })
}

fn decode_query_component(input: &str) -> Result<String, RestError> {
    let bytes = input.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'+' => {
                decoded.push(b' ');
                index += 1;
            }
            b'%' => {
                if index + 2 >= bytes.len() {
                    return Err(RestError::from_kernel(KernelError::validation(
                        "query contains an incomplete percent-escape",
                    )));
                }
                let high = hex_value(bytes[index + 1]).ok_or_else(|| {
                    RestError::from_kernel(KernelError::validation(
                        "query contains an invalid percent-escape",
                    ))
                })?;
                let low = hex_value(bytes[index + 2]).ok_or_else(|| {
                    RestError::from_kernel(KernelError::validation(
                        "query contains an invalid percent-escape",
                    ))
                })?;
                decoded.push((high << 4) | low);
                index += 3;
            }
            byte => {
                decoded.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8(decoded).map_err(|_| {
        RestError::from_kernel(KernelError::validation("query contains invalid UTF-8 data"))
    })
}

const fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn normalize_assigned_to(raw: Option<String>, actor: UserId) -> Result<Option<UserId>, RestError> {
    let Some(raw) = raw else {
        return Ok(None);
    };
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    if trimmed.eq_ignore_ascii_case("me") {
        return Ok(Some(actor));
    }
    UserId::from_str(trimmed).map(Some).map_err(|_| {
        RestError::from_kernel(KernelError::validation(
            "assigned_to must be me or a user id",
        ))
    })
}

fn normalize_management_no(raw: &str) -> Result<String, RestError> {
    let normalized = raw
        .trim()
        .trim_start_matches('#')
        .trim()
        .trim_end_matches("호기")
        .trim();
    if normalized.is_empty() {
        return Err(RestError::from_kernel(KernelError::validation(
            "management_no must not be empty",
        )));
    }
    Ok(normalized.to_owned())
}

fn normalize_limit(raw: Option<i64>, default: i64, max: i64) -> Result<i64, RestError> {
    let limit = raw.unwrap_or(default);
    if !(1..=max).contains(&limit) {
        return Err(RestError::from_kernel(KernelError::validation(format!(
            "limit must be between 1 and {max}"
        ))));
    }
    Ok(limit)
}

fn normalize_offset(raw: Option<i64>) -> Result<i64, RestError> {
    let offset = raw.unwrap_or(0);
    if offset < 0 {
        return Err(RestError::from_kernel(KernelError::validation(
            "offset must be non-negative",
        )));
    }
    Ok(offset)
}

fn push_work_order_filters(
    builder: &mut QueryBuilder<Postgres>,
    branch_scope: &BranchScope,
    query: &NormalizedWorkOrderListQuery,
) -> Result<(), RestError> {
    push_branch_scope_filter(
        builder,
        branch_scope,
        BranchColumn::new("w.branch_id").map_err(RestError::from_kernel)?,
    );
    if !query.statuses.is_empty() {
        builder.push(" AND w.status = ANY(");
        builder.push_bind(query.statuses.clone());
        builder.push(")");
    }
    if !query.priorities.is_empty() {
        builder.push(" AND w.priority = ANY(");
        builder.push_bind(query.priorities.clone());
        builder.push(")");
    }
    if !query.maintenance_types.is_empty() {
        builder.push(" AND w.maintenance_type = ANY(");
        builder.push_bind(query.maintenance_types.clone());
        builder.push(")");
    }
    if !query.maintenance_causes.is_empty() {
        builder.push(" AND w.maintenance_cause = ANY(");
        builder.push_bind(query.maintenance_causes.clone());
        builder.push(")");
    }
    if let Some(assigned_to) = query.assigned_to {
        builder.push(
            r#"
            AND EXISTS (
                SELECT 1
                FROM work_order_assignments filtered_assignments
                WHERE filtered_assignments.work_order_id = w.id
                  AND filtered_assignments.mechanic_id =
            "#,
        );
        builder.push_bind(*assigned_to.as_uuid());
        builder.push(")");
    }
    if let Some(customer_id) = query.customer_id {
        builder.push(" AND w.customer_id = ");
        builder.push_bind(customer_id);
    }
    if let Some(site_id) = query.site_id {
        builder.push(" AND w.site_id = ");
        builder.push_bind(site_id);
    }
    if let Some(branch_id) = query.branch_id {
        builder.push(" AND w.branch_id = ");
        builder.push_bind(branch_id);
    }
    if let Some(equipment_id) = query.equipment_id {
        builder.push(" AND w.equipment_id = ");
        builder.push_bind(equipment_id);
    }
    if let Some(around_work_order_id) = query.around_work_order_id {
        builder.push(
            r#"
            AND EXISTS (
                SELECT 1
                FROM work_orders seed
                WHERE seed.id =
            "#,
        );
        builder.push_bind(around_work_order_id);
        builder.push(" AND ");
        push_branch_scope_filter(
            builder,
            branch_scope,
            BranchColumn::new("seed.branch_id").map_err(RestError::from_kernel)?,
        );
        builder.push(
            r#"
                  AND seed.org_id = w.org_id
                  AND (
                    seed.id = w.id
                    OR seed.customer_id = w.customer_id
                    OR seed.site_id = w.site_id
                    OR seed.equipment_id = w.equipment_id
                  )
            )
            "#,
        );
    }
    if let Some(from) = query.target_due_from {
        builder.push(" AND w.target_due_at >= ");
        builder.push_bind(from);
    }
    if let Some(to) = query.target_due_to {
        builder.push(" AND w.target_due_at <= ");
        builder.push_bind(to);
    }
    Ok(())
}

fn push_branch_scope_filter(
    builder: &mut QueryBuilder<Postgres>,
    scope: &BranchScope,
    column: BranchColumn,
) {
    match scope {
        BranchScope::All => {
            builder.push("TRUE");
        }
        BranchScope::Branches(branches) if branches.is_empty() => {
            builder.push("FALSE");
        }
        BranchScope::Branches(branches) => {
            let branch_ids = branches
                .iter()
                .map(|branch| *branch.as_uuid())
                .collect::<Vec<_>>();
            builder.push(column.as_str());
            builder.push(" = ANY(");
            builder.push_bind(branch_ids);
            builder.push(")");
        }
    }
}

async fn fetch_assignment_map(
    pool: &PgPool,
    work_order_ids: &[uuid::Uuid],
) -> Result<BTreeMap<uuid::Uuid, Vec<AssignmentSummary>>, RestError> {
    if work_order_ids.is_empty() {
        return Ok(BTreeMap::new());
    }
    let org = current_org()
        .map_err(KernelError::from)
        .map_err(RestError::from_kernel)?;
    let ids = work_order_ids.to_vec();
    let rows = with_org_conn::<_, _, RestError>(pool, org, move |tx| {
        Box::pin(async move {
            Ok(sqlx::query(
                r#"
        SELECT
            a.work_order_id, a.id, a.mechanic_id, u.display_name AS mechanic_name,
            a.role, a.assigned_at
        FROM work_order_assignments a
        JOIN users u ON u.id = a.mechanic_id
        WHERE a.work_order_id = ANY($1)
        ORDER BY a.work_order_id,
                 CASE a.role WHEN 'PRIMARY' THEN 1 ELSE 2 END,
                 a.assigned_at,
                 a.id
        "#,
            )
            .bind(ids)
            .fetch_all(tx.as_mut())
            .await?)
        })
    })
    .await?;
    let mut assignments = BTreeMap::<uuid::Uuid, Vec<AssignmentSummary>>::new();
    for row in rows {
        let work_order_id = row.try_get("work_order_id")?;
        assignments
            .entry(work_order_id)
            .or_default()
            .push(assignment_from_row(&row)?);
    }
    Ok(assignments)
}

async fn fetch_approval_line(
    pool: &PgPool,
    work_order_id: WorkOrderId,
) -> Result<Vec<ApprovalStepSummary>, RestError> {
    let org = current_org()
        .map_err(KernelError::from)
        .map_err(RestError::from_kernel)?;
    let wo_uuid = *work_order_id.as_uuid();
    let rows = with_org_conn::<_, _, RestError>(pool, org, move |tx| {
        Box::pin(async move {
            Ok(sqlx::query(
                r#"
        SELECT
            step.id,
            step.step_order,
            step.role,
            step.approver_id,
            approver.display_name AS approver_name,
            step.status,
            step.requested_at,
            step.approved_at,
            step.approved_by_id,
            approved_by.display_name AS approved_by_name,
            step.decision_comment
        FROM work_order_approval_steps step
        LEFT JOIN users approver ON approver.id = step.approver_id
        LEFT JOIN users approved_by ON approved_by.id = step.approved_by_id
        WHERE step.work_order_id = $1
        ORDER BY step.step_order
        "#,
            )
            .bind(wo_uuid)
            .fetch_all(tx.as_mut())
            .await?)
        })
    })
    .await?;
    rows.iter().map(approval_step_from_row).collect()
}

async fn fetch_status_history(
    pool: &PgPool,
    work_order_id: WorkOrderId,
) -> Result<Vec<StatusHistorySummary>, RestError> {
    let org = current_org()
        .map_err(KernelError::from)
        .map_err(RestError::from_kernel)?;
    let wo_uuid = *work_order_id.as_uuid();
    let rows = with_org_conn::<_, _, RestError>(pool, org, move |tx| {
        Box::pin(async move {
            Ok(sqlx::query(
                r#"
        SELECT id, actor, action, from_status, to_status, occurred_at
        FROM work_order_status_history
        WHERE work_order_id = $1
        ORDER BY occurred_at, id
        "#,
            )
            .bind(wo_uuid)
            .fetch_all(tx.as_mut())
            .await?)
        })
    })
    .await?;
    rows.iter().map(status_history_from_row).collect()
}

async fn fetch_evidence_summaries(
    pool: &PgPool,
    work_order_id: WorkOrderId,
) -> Result<Vec<EvidenceSummary>, RestError> {
    let org = current_org()
        .map_err(KernelError::from)
        .map_err(RestError::from_kernel)?;
    let wo_uuid = *work_order_id.as_uuid();
    let rows = with_org_conn::<_, _, RestError>(pool, org, move |tx| {
        Box::pin(async move {
            Ok(sqlx::query(
                r#"
        SELECT
            id, stage, content_type, size_bytes, uploaded_by,
            worm_replica_status, retry_count, verified_at, created_at
        FROM evidence_media
        WHERE work_order_id = $1
        ORDER BY created_at, id
        "#,
            )
            .bind(wo_uuid)
            .fetch_all(tx.as_mut())
            .await?)
        })
    })
    .await?;
    rows.iter().map(evidence_from_row).collect()
}

fn work_order_list_item_from_row(
    row: &sqlx::postgres::PgRow,
    assignments: &BTreeMap<uuid::Uuid, Vec<AssignmentSummary>>,
) -> Result<WorkOrderListItem, RestError> {
    let id: uuid::Uuid = row.try_get("id")?;
    Ok(WorkOrderListItem {
        id: WorkOrderId::from_uuid(id),
        request_no: row.try_get("request_no")?,
        branch_id: BranchId::from_uuid(row.try_get("branch_id")?),
        status: row.try_get("status")?,
        priority: row.try_get("priority")?,
        result_type: row.try_get("result_type")?,
        maintenance_type: row.try_get("maintenance_type")?,
        maintenance_cause: row.try_get("maintenance_cause")?,
        target_due_at: row.try_get("target_due_at")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
        equipment: equipment_summary_from_row(row)?,
        customer: NamedEntity {
            id: row.try_get("customer_id")?,
            name: row.try_get("customer_name")?,
        },
        site: NamedEntity {
            id: row.try_get("site_id")?,
            name: row.try_get("site_name")?,
        },
        site_contact: site_contact_from_row(row)?,
        assignments: assignments.get(&id).cloned().unwrap_or_default(),
    })
}

fn approval_item_from_row(
    row: &sqlx::postgres::PgRow,
    assignments: &BTreeMap<uuid::Uuid, Vec<AssignmentSummary>>,
) -> Result<ApprovalItem, RestError> {
    let source: String = row.try_get("source")?;
    let source_id: uuid::Uuid = row.try_get("source_id")?;
    let tenant_id = OrgId::from_uuid(row.try_get("approval_org_id")?);
    let branch_id = BranchId::from_uuid(row.try_get("approval_branch_id")?);
    let status: String = row.try_get("approval_status")?;
    let requested_at = row.try_get("approval_requested_at")?;
    let due_at = row.try_get("approval_due_at")?;
    let base = ApprovalItemBase {
        source: source.clone(),
        source_id,
        tenant_id,
        branch_id,
        status: status.clone(),
        requested_at,
        due_at,
    };

    match source.as_str() {
        "WORK_ORDER" => {
            let work_order = work_order_list_item_from_row(row, assignments)?;
            let title = format!("{} 작업 보고 승인", work_order.request_no);
            let summary = work_order
                .equipment
                .model
                .as_ref()
                .filter(|model| !model.trim().is_empty())
                .cloned()
                .unwrap_or_else(|| work_order.equipment.equipment_no.clone());
            Ok(base.into_item(
                title,
                summary,
                format!("/approvals?source=work-order&focus={source_id}"),
                format!("/api/work-orders/{source_id}/approve"),
                Some(work_order),
                None,
                None,
            ))
        }
        "DAILY_PLAN" => {
            let daily_status: String = row.try_get("daily_status")?;
            let plan = DailyPlanSummary {
                id: DailyPlanId::from_uuid(row.try_get("daily_plan_id")?),
                branch_id: BranchId::from_uuid(row.try_get("daily_branch_id")?),
                mechanic_id: UserId::from_uuid(row.try_get("daily_mechanic_id")?),
                plan_date: row.try_get("daily_plan_date")?,
                status: DailyPlanStatus::from_db_str(&daily_status)
                    .map_err(RestError::from_kernel)?,
                items: Vec::new(),
            };
            let title = format!("{} 계획업무 검토", plan.plan_date);
            Ok(base.into_item(
                title,
                "계획업무 검토 요청".to_owned(),
                format!("/daily-plan?planId={source_id}"),
                format!("/api/daily-work-plans/{source_id}/review"),
                None,
                Some(plan),
                None,
            ))
        }
        "TARGET_CHANGE" => {
            let target_status: String = row.try_get("target_status")?;
            let request = TargetChangeRequestSummary {
                id: row.try_get("target_change_id")?,
                work_order_id: WorkOrderId::from_uuid(row.try_get("target_work_order_id")?),
                branch_id,
                requested_target_due_at: row.try_get("target_requested_target_due_at")?,
                status: TargetChangeStatus::from_db_str(&target_status)
                    .map_err(RestError::from_kernel)?,
            };
            Ok(base.into_item(
                "일정 변경 요청".to_owned(),
                "목표 완료 변경 검토".to_owned(),
                format!("#target-change-{source_id}"),
                format!("/api/target-change-requests/{source_id}/review"),
                None,
                None,
                Some(request),
            ))
        }
        other => Err(RestError::from_kernel(KernelError::validation(format!(
            "unknown approval source {other:?}"
        )))),
    }
}

struct ApprovalItemBase {
    source: String,
    source_id: uuid::Uuid,
    tenant_id: OrgId,
    branch_id: BranchId,
    status: String,
    requested_at: Option<time::OffsetDateTime>,
    due_at: Option<time::OffsetDateTime>,
}

impl ApprovalItemBase {
    #[allow(clippy::too_many_arguments)]
    fn into_item(
        self,
        title: String,
        summary: String,
        href: String,
        action_href: String,
        work_order: Option<WorkOrderListItem>,
        daily_plan: Option<DailyPlanSummary>,
        target_change: Option<TargetChangeRequestSummary>,
    ) -> ApprovalItem {
        let ontology = ApprovalOntologyContext {
            object_type: self.source.clone(),
            object_id: self.source_id,
            tenant_id: self.tenant_id,
            branch_id: self.branch_id,
        };
        let workflow = approval_workflow_context(&self.source);
        let policy = approval_policy_context(&self.source, self.branch_id);
        ApprovalItem {
            id: format!("{}:{}", self.source, self.source_id),
            source: self.source,
            source_id: self.source_id,
            branch_id: self.branch_id,
            status: self.status,
            title,
            summary,
            requested_at: self.requested_at,
            due_at: self.due_at,
            href,
            action_href,
            ontology,
            workflow,
            policy,
            work_order,
            daily_plan,
            target_change,
        }
    }
}

fn approval_workflow_context(source: &str) -> ApprovalWorkflowContext {
    let (workflow_key, action_key) = match source {
        "WORK_ORDER" => ("work_order.report_completion_review", "approve_work_order"),
        "DAILY_PLAN" => ("daily_plan.review", "review_daily_plan"),
        "TARGET_CHANGE" => ("work_order.target_change_review", "review_target_change"),
        _ => ("approval.unknown", "unknown"),
    };
    ApprovalWorkflowContext {
        workflow_key: workflow_key.to_owned(),
        action_key: action_key.to_owned(),
    }
}

fn approval_policy_context(source: &str, branch_id: BranchId) -> ApprovalPolicyContext {
    let required_features = match source {
        "WORK_ORDER" => vec!["completion_review"],
        "DAILY_PLAN" => vec!["daily_plan_review"],
        "TARGET_CHANGE" => vec!["target_manage"],
        _ => Vec::new(),
    };
    ApprovalPolicyContext {
        decision: "ALLOWED",
        enforcement: "server",
        required_features,
        scope_kind: "BRANCH",
        scope_id: *branch_id.as_uuid(),
    }
}

fn work_order_detail_from_row(
    row: &sqlx::postgres::PgRow,
    assignments: Vec<AssignmentSummary>,
    approval_line: Vec<ApprovalStepSummary>,
    status_history: Vec<StatusHistorySummary>,
    evidence: Vec<EvidenceSummary>,
    settlement: Option<SettlementSummary>,
) -> Result<WorkOrderDetail, RestError> {
    Ok(WorkOrderDetail {
        id: WorkOrderId::from_uuid(row.try_get("id")?),
        request_no: row.try_get("request_no")?,
        branch_id: BranchId::from_uuid(row.try_get("branch_id")?),
        status: row.try_get("status")?,
        priority: row.try_get("priority")?,
        result_type: row.try_get("result_type")?,
        maintenance_type: row.try_get("maintenance_type")?,
        maintenance_cause: row.try_get("maintenance_cause")?,
        symptom: row.try_get("symptom")?,
        customer_request: row.try_get("customer_request")?,
        target_due_at: row.try_get("target_due_at")?,
        delay_reason: row.try_get("delay_reason")?,
        delay_note: row.try_get("delay_note")?,
        diagnosis: row.try_get("diagnosis")?,
        action_taken: row.try_get("action_taken")?,
        report_submitted_by: row
            .try_get::<Option<uuid::Uuid>, _>("report_submitted_by")?
            .map(UserId::from_uuid),
        report_submitted_at: row.try_get("report_submitted_at")?,
        kpi_excluded: row.try_get("kpi_excluded")?,
        evidence_verified: row.try_get("evidence_verified")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
        equipment: equipment_summary_from_row(row)?,
        customer: NamedEntity {
            id: row.try_get("customer_id")?,
            name: row.try_get("customer_name")?,
        },
        site: NamedEntity {
            id: row.try_get("site_id")?,
            name: row.try_get("site_name")?,
        },
        site_contact: site_contact_from_row(row)?,
        assignments,
        approval_line,
        status_history,
        evidence,
        settlement,
    })
}

/// Build the site's representative contact from the detail row. Returns None when
/// the site has no contact registered (all three columns NULL) so the response
/// omits an empty object.
fn site_contact_from_row(row: &sqlx::postgres::PgRow) -> Result<Option<SiteContact>, RestError> {
    let name: Option<String> = row.try_get("site_contact_name")?;
    let phone: Option<String> = row.try_get("site_contact_phone")?;
    let email: Option<String> = row.try_get("site_contact_email")?;
    if name.is_none() && phone.is_none() && email.is_none() {
        Ok(None)
    } else {
        Ok(Some(SiteContact { name, phone, email }))
    }
}

fn equipment_summary_from_row(row: &sqlx::postgres::PgRow) -> Result<EquipmentSummary, RestError> {
    Ok(EquipmentSummary {
        id: row.try_get("equipment_id")?,
        equipment_no: row.try_get("equipment_no")?,
        management_no: row.try_get("management_no")?,
        model: row.try_get("model")?,
        status: row.try_get("equipment_status")?,
        specification: row.try_get("specification")?,
        ton_text: row.try_get("ton_text")?,
    })
}

fn equipment_lookup_from_row(
    row: &sqlx::postgres::PgRow,
) -> Result<EquipmentLookupResponse, RestError> {
    Ok(EquipmentLookupResponse {
        id: row.try_get("id")?,
        branch_id: BranchId::from_uuid(row.try_get("branch_id")?),
        equipment_no: row.try_get("equipment_no")?,
        management_no: row.try_get("management_no")?,
        model: row.try_get("model")?,
        status: row.try_get("status")?,
        specification: row.try_get("specification")?,
        ton_text: row.try_get("ton_text")?,
        maker: row.try_get("maker")?,
        vin: row.try_get("vin")?,
        vehicle_registration_no: row.try_get("vehicle_registration_no")?,
        customer: NamedEntity {
            id: row.try_get("customer_id")?,
            name: row.try_get("customer_name")?,
        },
        site: NamedEntity {
            id: row.try_get("site_id")?,
            name: row.try_get("site_name")?,
        },
    })
}

fn assignment_from_row(row: &sqlx::postgres::PgRow) -> Result<AssignmentSummary, RestError> {
    Ok(AssignmentSummary {
        id: row.try_get("id")?,
        mechanic_id: UserId::from_uuid(row.try_get("mechanic_id")?),
        mechanic_name: row.try_get("mechanic_name")?,
        role: row.try_get("role")?,
        assigned_at: row.try_get("assigned_at")?,
    })
}

fn approval_step_from_row(row: &sqlx::postgres::PgRow) -> Result<ApprovalStepSummary, RestError> {
    Ok(ApprovalStepSummary {
        id: row.try_get("id")?,
        step_order: row.try_get("step_order")?,
        role: row.try_get("role")?,
        approver_id: row
            .try_get::<Option<uuid::Uuid>, _>("approver_id")?
            .map(UserId::from_uuid),
        approver_name: row.try_get("approver_name")?,
        status: row.try_get("status")?,
        requested_at: row.try_get("requested_at")?,
        approved_at: row.try_get("approved_at")?,
        approved_by_id: row
            .try_get::<Option<uuid::Uuid>, _>("approved_by_id")?
            .map(UserId::from_uuid),
        approved_by_name: row.try_get("approved_by_name")?,
        decision_comment: row.try_get("decision_comment")?,
    })
}

fn status_history_from_row(row: &sqlx::postgres::PgRow) -> Result<StatusHistorySummary, RestError> {
    Ok(StatusHistorySummary {
        id: row.try_get("id")?,
        actor: row
            .try_get::<Option<uuid::Uuid>, _>("actor")?
            .map(UserId::from_uuid),
        action: row.try_get("action")?,
        from_status: row.try_get("from_status")?,
        to_status: row.try_get("to_status")?,
        occurred_at: row.try_get("occurred_at")?,
    })
}

fn evidence_from_row(row: &sqlx::postgres::PgRow) -> Result<EvidenceSummary, RestError> {
    Ok(EvidenceSummary {
        id: EvidenceId::from_uuid(row.try_get("id")?),
        stage: row.try_get("stage")?,
        content_type: row.try_get("content_type")?,
        size_bytes: row.try_get("size_bytes")?,
        uploaded_by: UserId::from_uuid(row.try_get("uploaded_by")?),
        worm_replica_status: row.try_get("worm_replica_status")?,
        retry_count: row.try_get("retry_count")?,
        verified_at: row.try_get("verified_at")?,
        created_at: row.try_get("created_at")?,
    })
}

async fn authorize_for_work_order(
    state: &WorkOrderRestState,
    headers: &HeaderMap,
    work_order_id: WorkOrderId,
    action: Action,
) -> Result<Principal, RestError> {
    let principal = principal_from_headers(state, headers).await?;
    let summary = state
        .store
        .work_order(work_order_id)
        .await
        .map_err(RestError::from_store)?;
    authorize(&principal, action, summary.branch_id).map_err(RestError::from_kernel)?;
    Ok(principal)
}

async fn mobile_principal_from_headers<S>(
    state: &MobileRestState<S>,
    headers: &HeaderMap,
) -> Result<Principal, RestError> {
    let verifier = state.jwt_verifier.as_ref().ok_or_else(|| {
        RestError::unavailable("JWT verification is not configured for mobile API")
    })?;
    console_platform_request_context::resolve_principal(verifier, &state.pool, headers)
        .await
        .map_err(rest_error_from_request_context)
}

async fn principal_from_headers(
    state: &WorkOrderRestState,
    headers: &HeaderMap,
) -> Result<Principal, RestError> {
    let verifier = state.jwt_verifier.as_ref().ok_or_else(|| {
        RestError::unavailable("JWT verification is not configured for work-order API")
    })?;
    console_platform_request_context::resolve_principal(verifier, state.store.pool(), headers)
        .await
        .map_err(rest_error_from_request_context)
}

fn rest_error_from_request_context(
    err: console_platform_request_context::RequestContextError,
) -> RestError {
    match err {
        console_platform_request_context::RequestContextError::VerifierUnavailable => {
            RestError::unavailable("JWT verification is not configured for this API")
        }
        console_platform_request_context::RequestContextError::WrongTokenTier => {
            RestError::from_kernel(KernelError::forbidden(
                "token tier is not valid for this route",
            ))
        }
        console_platform_request_context::RequestContextError::AccessScope(error) => {
            RestError::from_kernel(error)
        }
        console_platform_request_context::RequestContextError::BranchScope(message)
        | console_platform_request_context::RequestContextError::EffectivePolicy(message) => {
            RestError::internal(message)
        }
        console_platform_request_context::RequestContextError::MissingOrg => {
            RestError::internal("no tenant context is bound to the current request")
        }
        console_platform_request_context::RequestContextError::MissingBearer => {
            RestError::unauthorized("missing or malformed bearer token")
        }
        console_platform_request_context::RequestContextError::InvalidToken => {
            RestError::unauthorized("invalid bearer token")
        }
        console_platform_request_context::RequestContextError::InvalidClaim(message) => {
            RestError::unauthorized(format!("token claim is invalid: {message}"))
        }
    }
}

fn device_hash_from_headers(headers: &HeaderMap) -> Result<String, RestError> {
    let raw = headers
        .get("x-device-id")
        .ok_or_else(|| RestError::bad_request("missing X-Device-Id header"))?
        .to_str()
        .map_err(|_| RestError::bad_request("invalid X-Device-Id header"))?;
    let raw = raw.trim();
    if raw.is_empty() {
        return Err(RestError::bad_request(
            "X-Device-Id header must not be empty",
        ));
    }
    Ok(hex::encode(Sha256::digest(raw.as_bytes())))
}

fn normalize_non_empty(value: String, field: &'static str) -> Result<String, RestError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        Err(RestError::from_kernel(KernelError::validation(format!(
            "{field} must not be empty"
        ))))
    } else {
        Ok(trimmed.to_owned())
    }
}

/// The branch to stamp on a device-registration audit row. `registered_devices`
/// (migration 0010) has no `branch_id`, so this is the ACTOR's branch, not the
/// resource's.
///
/// The two branch-less outcomes are NOT the same and must not be collapsed into
/// one `None`:
///
/// * `All` — no single branch to name. Leave the audit column NULL rather than
///   stamp a random UUID that belongs to no branch. Mirrors
///   `console_app::audit_event_branch`. Still authorized.
/// * `Branches({})` — no branch membership at all, so the caller belongs nowhere
///   in this org. FORBIDDEN. This is the ONLY authorization check
///   [`register_device`] performs beyond authenticating the token, and a
///   membership-less principal is denied everywhere else in this file
///   (`authorize_capability` rejects `branch_scope.is_empty()` up front), so it
///   must be denied here too. Returning a bare `Option` would silently admit it.
fn audit_branch_for_principal(principal: &Principal) -> Result<Option<BranchId>, RestError> {
    match &principal.branch_scope {
        BranchScope::All => Ok(None),
        // fabricated-branch: ok stamps the ACTOR's branch on an audit row; never reaches authorize
        BranchScope::Branches(branches) => {
            branches.iter().next().copied().map(Some).ok_or_else(|| {
                RestError::from_kernel(KernelError::forbidden("principal has no branch scope"))
            })
        }
    }
}

fn device_registration_from_row(
    row: &sqlx::postgres::PgRow,
) -> Result<DeviceRegistrationResponse, RestError> {
    let platform: String = row.try_get("platform")?;
    Ok(DeviceRegistrationResponse {
        id: DeviceId::from_uuid(row.try_get("id")?),
        user_id: UserId::from_uuid(row.try_get("user_id")?),
        device_hash: row.try_get("device_hash")?,
        platform: DevicePlatform::from_db_str(&platform).map_err(RestError::from_kernel)?,
        push_token: row.try_get("push_token")?,
        app_version: row.try_get("app_version")?,
        last_registered_at: row.try_get("last_registered_at")?,
    })
}

fn status_for_error_kind(kind: ErrorKind) -> StatusCode {
    match kind {
        ErrorKind::Validation => StatusCode::UNPROCESSABLE_ENTITY,
        ErrorKind::NotFound => StatusCode::NOT_FOUND,
        ErrorKind::Forbidden => StatusCode::FORBIDDEN,
        ErrorKind::Conflict | ErrorKind::InvalidTransition => StatusCode::CONFLICT,
        ErrorKind::Internal => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ApprovalItem, ApprovalOntologyContext, ApprovalPolicyContext, ApprovalStepSummary,
        ApprovalWorkflowContext, AssignmentSummary, CreateWorkOrderRequest, DevicePlatform,
        DeviceRegistrationResponse, EquipmentSummary, EvidenceStatusResponse, EvidenceSummary,
        NamedEntity, Principal, StatusCode, StatusHistorySummary, SyncBatchRequest,
        WorkOrderDetail, WorkOrderListItem, audit_branch_for_principal, normalize_management_no,
    };
    use console_kernel_core::{
        BranchId, BranchScope, DeviceId, EvidenceId, OrgId, UserId, WorkOrderId,
    };
    use console_platform_storage::ProcessingStatus;
    use console_workorder_domain::AttachmentStage;
    use std::collections::BTreeSet;

    fn device_principal(branch_scope: BranchScope) -> Principal {
        Principal::new(
            UserId::from_uuid(uuid::Uuid::nil()),
            OrgId::from_uuid(uuid::Uuid::nil()),
            BTreeSet::new(),
            branch_scope,
        )
    }

    /// `POST /devices` has no feature gate: the audit-branch resolution IS its
    /// authorization check. All three branch-scope shapes are pinned because the
    /// migration to `Option` once collapsed two of them and admitted a principal
    /// that was 403 before.
    ///
    /// `All`: authorized, and the audit row's branch stays NULL rather than
    /// carrying a minted UUID that belongs to no branch.
    #[test]
    fn device_audit_branch_is_none_for_an_all_scoped_actor() {
        let principal = device_principal(BranchScope::All);

        assert_eq!(audit_branch_for_principal(&principal).unwrap(), None);
    }

    /// A member: authorized, and the audit row carries the ACTOR's branch.
    #[test]
    fn device_audit_branch_is_the_actors_branch_for_a_member() {
        let branch = BranchId::new();
        let principal = device_principal(BranchScope::single(branch));

        assert_eq!(
            audit_branch_for_principal(&principal).unwrap(),
            Some(branch)
        );
    }

    /// No memberships at all: 403. This is the regression the review caught —
    /// `BranchScope::Branches({})` is not `All`, and turning both into a bare
    /// `None` would register the device for a principal that belongs to no branch
    /// in this org.
    #[test]
    fn device_registration_is_forbidden_for_an_empty_branch_scope() {
        let principal = device_principal(BranchScope::none());

        let error = audit_branch_for_principal(&principal)
            .expect_err("an empty branch scope must not reach the INSERT");
        assert_eq!(error.status, StatusCode::FORBIDDEN);
    }

    fn test_equipment_summary() -> EquipmentSummary {
        EquipmentSummary {
            id: uuid::Uuid::nil(),
            equipment_no: "EQ-001".to_owned(),
            management_no: Some("001".to_owned()),
            model: Some("TEST".to_owned()),
            status: "ACTIVE".to_owned(),
            specification: "TEST".to_owned(),
            ton_text: "1T".to_owned(),
        }
    }

    fn test_customer() -> NamedEntity {
        NamedEntity {
            id: uuid::Uuid::nil(),
            name: "Test customer".to_owned(),
        }
    }

    fn test_site() -> NamedEntity {
        NamedEntity {
            id: uuid::Uuid::nil(),
            name: "Test site".to_owned(),
        }
    }

    fn test_assignments(timestamp: time::OffsetDateTime) -> Vec<AssignmentSummary> {
        vec![AssignmentSummary {
            id: uuid::Uuid::nil(),
            mechanic_id: UserId::from_uuid(uuid::Uuid::nil()),
            mechanic_name: "Test mechanic".to_owned(),
            role: "PRIMARY".to_owned(),
            assigned_at: timestamp,
        }]
    }

    #[test]
    fn work_order_list_item_serializes_timestamps_as_rfc3339_strings() {
        let timestamp = time::OffsetDateTime::from_unix_timestamp(0).unwrap();
        let mut item = WorkOrderListItem {
            id: WorkOrderId::from_uuid(uuid::Uuid::nil()),
            request_no: "20260722-001".to_owned(),
            branch_id: BranchId::from_uuid(uuid::Uuid::nil()),
            status: "ASSIGNED".to_owned(),
            priority: "P2".to_owned(),
            result_type: "REPAIR".to_owned(),
            maintenance_type: Some("PREVENTIVE".to_owned()),
            maintenance_cause: Some("SCHEDULED".to_owned()),
            target_due_at: Some(timestamp),
            created_at: timestamp,
            updated_at: timestamp,
            equipment: test_equipment_summary(),
            customer: test_customer(),
            site: test_site(),
            site_contact: None,
            assignments: test_assignments(timestamp),
        };

        let json = serde_json::to_value(&item).unwrap();
        for pointer in [
            "/target_due_at",
            "/created_at",
            "/updated_at",
            "/assignments/0/assigned_at",
        ] {
            assert_eq!(
                json.pointer(pointer).and_then(serde_json::Value::as_str),
                Some("1970-01-01T00:00:00Z"),
                "{pointer} must honor the OpenAPI string/date-time contract"
            );
        }

        item.target_due_at = None;
        let json = serde_json::to_value(item).unwrap();
        assert_eq!(
            json.pointer("/target_due_at"),
            Some(&serde_json::Value::Null),
            "an absent target due date must remain JSON null"
        );
    }

    #[test]
    fn work_order_requests_deserialize_rfc3339_timestamps() {
        let create: CreateWorkOrderRequest = serde_json::from_str(
            r#"{
                "branch_id": "00000000-0000-0000-0000-000000000001",
                "management_no": "EQ-001",
                "symptom": "Test symptom",
                "target_due_at": "2026-07-22T12:34:56.123456789Z"
            }"#,
        )
        .unwrap();
        assert_eq!(
            create
                .target_due_at
                .expect("target_due_at must decode")
                .nanosecond(),
            123_456_789
        );

        let create_without_due_at: CreateWorkOrderRequest = serde_json::from_str(
            r#"{
                "branch_id": "00000000-0000-0000-0000-000000000001",
                "management_no": "EQ-001",
                "symptom": "Test symptom"
            }"#,
        )
        .unwrap();
        assert_eq!(create_without_due_at.target_due_at, None);

        let sync: SyncBatchRequest = serde_json::from_str(
            r#"{
                "sync_id": "sync-1",
                "operations": [{
                    "request_id": "request-1",
                    "operation": "WORK_ORDER_START",
                    "created_at": "2026-07-22T12:34:56.987654321Z",
                    "payload": {}
                }]
            }"#,
        )
        .unwrap();
        assert_eq!(sync.operations.len(), 1);
        assert_eq!(sync.operations[0].created_at.nanosecond(), 987_654_321);
    }

    #[test]
    fn approval_item_serializes_optional_timestamps_as_rfc3339_strings_or_null() {
        let timestamp = time::OffsetDateTime::from_unix_timestamp(0)
            .unwrap()
            .replace_nanosecond(123_456_789)
            .unwrap();
        let mut item = ApprovalItem {
            id: "work_order:00000000-0000-0000-0000-000000000001".to_owned(),
            source: "work_order".to_owned(),
            source_id: uuid::Uuid::nil(),
            branch_id: BranchId::from_uuid(uuid::Uuid::nil()),
            status: "PENDING".to_owned(),
            title: "Test approval".to_owned(),
            summary: "Test summary".to_owned(),
            requested_at: Some(timestamp),
            due_at: Some(timestamp),
            href: "/approvals/1".to_owned(),
            action_href: "/approvals/1/approve".to_owned(),
            ontology: ApprovalOntologyContext {
                object_type: "WORK_ORDER".to_owned(),
                object_id: uuid::Uuid::nil(),
                tenant_id: OrgId::from_uuid(uuid::Uuid::nil()),
                branch_id: BranchId::from_uuid(uuid::Uuid::nil()),
            },
            workflow: ApprovalWorkflowContext {
                workflow_key: "work_order".to_owned(),
                action_key: "approve".to_owned(),
            },
            policy: ApprovalPolicyContext {
                decision: "ALLOW",
                enforcement: "ENFORCED",
                required_features: Vec::new(),
                scope_kind: "BRANCH",
                scope_id: uuid::Uuid::nil(),
            },
            work_order: None,
            daily_plan: None,
            target_change: None,
        };

        let json = serde_json::to_value(&item).unwrap();
        for pointer in ["/requested_at", "/due_at"] {
            assert_eq!(
                json.pointer(pointer).and_then(serde_json::Value::as_str),
                Some("1970-01-01T00:00:00.123456789Z"),
                "{pointer} must honor the OpenAPI string/date-time contract"
            );
        }

        item.requested_at = None;
        item.due_at = None;
        let json = serde_json::to_value(item).unwrap();
        for pointer in ["/requested_at", "/due_at"] {
            assert_eq!(json.pointer(pointer), Some(&serde_json::Value::Null));
        }
    }

    #[test]
    fn work_order_detail_serializes_all_timestamps_as_rfc3339_strings() {
        let timestamp = time::OffsetDateTime::from_unix_timestamp(0)
            .unwrap()
            .replace_nanosecond(123_456_789)
            .unwrap();
        let expected = "1970-01-01T00:00:00.123456789Z";
        let detail = WorkOrderDetail {
            id: WorkOrderId::from_uuid(uuid::Uuid::nil()),
            request_no: "20260722-001".to_owned(),
            branch_id: BranchId::from_uuid(uuid::Uuid::nil()),
            status: "ASSIGNED".to_owned(),
            priority: "P2".to_owned(),
            result_type: "REPAIR".to_owned(),
            maintenance_type: Some("EMERGENCY".to_owned()),
            maintenance_cause: Some("BREAKDOWN".to_owned()),
            symptom: "Test symptom".to_owned(),
            customer_request: None,
            target_due_at: Some(timestamp),
            delay_reason: None,
            delay_note: None,
            diagnosis: None,
            action_taken: None,
            report_submitted_by: Some(UserId::from_uuid(uuid::Uuid::nil())),
            report_submitted_at: Some(timestamp),
            kpi_excluded: false,
            evidence_verified: true,
            created_at: timestamp,
            updated_at: timestamp,
            equipment: test_equipment_summary(),
            customer: test_customer(),
            site: test_site(),
            site_contact: None,
            assignments: test_assignments(timestamp),
            approval_line: vec![ApprovalStepSummary {
                id: uuid::Uuid::nil(),
                step_order: 1,
                role: "ADMIN".to_owned(),
                approver_id: Some(UserId::from_uuid(uuid::Uuid::nil())),
                approver_name: Some("Test approver".to_owned()),
                status: "APPROVED".to_owned(),
                requested_at: Some(timestamp),
                approved_at: Some(timestamp),
                approved_by_id: Some(UserId::from_uuid(uuid::Uuid::nil())),
                approved_by_name: Some("Test approver".to_owned()),
                decision_comment: None,
            }],
            status_history: vec![StatusHistorySummary {
                id: uuid::Uuid::nil(),
                actor: Some(UserId::from_uuid(uuid::Uuid::nil())),
                action: "ASSIGN".to_owned(),
                from_status: Some("OPEN".to_owned()),
                to_status: "ASSIGNED".to_owned(),
                occurred_at: timestamp,
            }],
            evidence: vec![EvidenceSummary {
                id: EvidenceId::from_uuid(uuid::Uuid::nil()),
                stage: "READY".to_owned(),
                content_type: "image/jpeg".to_owned(),
                size_bytes: 1,
                uploaded_by: UserId::from_uuid(uuid::Uuid::nil()),
                worm_replica_status: "VERIFIED".to_owned(),
                retry_count: 0,
                verified_at: Some(timestamp),
                created_at: timestamp,
            }],
            settlement: None,
        };

        let json = serde_json::to_value(detail).unwrap();
        for pointer in [
            "/target_due_at",
            "/report_submitted_at",
            "/created_at",
            "/updated_at",
            "/assignments/0/assigned_at",
            "/approval_line/0/requested_at",
            "/approval_line/0/approved_at",
            "/status_history/0/occurred_at",
            "/evidence/0/verified_at",
            "/evidence/0/created_at",
        ] {
            assert_eq!(
                json.pointer(pointer).and_then(serde_json::Value::as_str),
                Some(expected),
                "{pointer} must honor the OpenAPI string/date-time contract"
            );
        }
    }

    #[test]
    fn mobile_timestamp_responses_serialize_fractional_rfc3339_strings() {
        let timestamp = time::OffsetDateTime::from_unix_timestamp(0)
            .unwrap()
            .replace_nanosecond(123_456_789)
            .unwrap();
        let device = DeviceRegistrationResponse {
            id: DeviceId::from_uuid(uuid::Uuid::nil()),
            user_id: UserId::from_uuid(uuid::Uuid::nil()),
            device_hash: "a".repeat(64),
            platform: DevicePlatform::Ios,
            push_token: None,
            app_version: "1.0.0".to_owned(),
            last_registered_at: timestamp,
        };
        let evidence = EvidenceStatusResponse {
            id: EvidenceId::from_uuid(uuid::Uuid::nil()),
            work_order_id: WorkOrderId::from_uuid(uuid::Uuid::nil()),
            stage: AttachmentStage::After,
            processing_status: ProcessingStatus::Ready,
            content_type: "image/jpeg".to_owned(),
            thumbnail_url: None,
            processing_error: None,
            processed_at: Some(timestamp),
        };

        assert_eq!(
            serde_json::to_value(device)
                .unwrap()
                .pointer("/last_registered_at")
                .and_then(serde_json::Value::as_str),
            Some("1970-01-01T00:00:00.123456789Z")
        );
        assert_eq!(
            serde_json::to_value(evidence)
                .unwrap()
                .pointer("/processed_at")
                .and_then(serde_json::Value::as_str),
            Some("1970-01-01T00:00:00.123456789Z")
        );
    }

    /// The console equipment search lets a user type the 호기 the way it appears
    /// on the floor: a leading '#', a trailing '호기', or both, with stray
    /// whitespace. They must all normalize to the same bare token, which the
    /// leading-zero-insensitive SQL then matches against the stored zero-padded
    /// `management_no` (e.g. '010').
    #[test]
    fn normalize_management_no_strips_hash_and_hogi_suffix() {
        for raw in ["#10호기", "10호기", " 10호기 ", "#10", "010", "10"] {
            assert_eq!(
                normalize_management_no(raw).unwrap(),
                if raw.trim() == "010" { "010" } else { "10" },
                "input {raw:?} must normalize to its bare 호기 core"
            );
        }
        // '010' keeps its stored zero-padding; ltrim(...,'0') in SQL handles the
        // leading-zero match, NOT this Rust normalization.
        assert_eq!(normalize_management_no("010").unwrap(), "010");
        assert_eq!(normalize_management_no("#010호기").unwrap(), "010");
    }

    #[test]
    fn normalize_management_no_rejects_empty_input() {
        for raw in ["", "   ", "#", "호기", " #호기 "] {
            assert!(
                normalize_management_no(raw).is_err(),
                "blank-after-normalization input {raw:?} must error, never match every row"
            );
        }
    }
}
