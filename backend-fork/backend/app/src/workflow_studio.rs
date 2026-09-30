use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, patch, post};
use axum::{Extension, Json, Router};
use console_governance_adapter_postgres::{PgGovernanceError, four_eyes_consume_conn};
use console_governance_domain::{GateChainConfig, GateEvidence, evaluate_gate_chain};
use console_kernel_core::{
    AuditAction, AuditEvent, BranchId, ErrorKind, KernelError, TraceContext, UserId,
};
use console_platform_auth::{JwtVerifier, PasskeyAuthenticationCredential, PasskeyService};
use console_platform_authz::{
    Action, AuthorizationAuditEvent, AuthorizationRequest, AuthorizationResource, Feature,
    PermissionLevel, Principal, RlsScopeProof, authorize_org_wide, permission_for,
};
use console_platform_db::{DbError, with_audit, with_org_conn};
use console_workflow_domain::{
    FinalizeWaitingTaskCommand, FinalizeWaitingTaskContext, PostFinalizationRejectionCommand,
    RunStatus, TriggerType, WaitingTaskStatus, WorkflowRuntimePort,
};
use console_workflow_runtime::{
    AuditContext, ExecGraph, FinalizeMode, FinalizePolicyOutcome, NodeKind, StartRunRequest,
    WAITING_COMPLETION_DOMAIN, build_guard_request, drive_from, guard, start_run,
    workflow_coexistence_entry,
};
use console_workflow_runtime_adapter_postgres::{
    AdminRunListFilter, ClaimWaitingTaskCommand, DecideWaitingTaskCommand, PgWorkflowRuntimeStore,
    RunListFilter, RunListItem, TaskDecision, WaitingTaskListFilter, WaitingTaskListItem,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::{PgPool, Postgres, Row, Transaction};
use std::collections::{HashMap, HashSet, VecDeque};
use std::str::FromStr;
use time::OffsetDateTime;
use uuid::Uuid;

pub const WORKFLOW_STUDIO_CATALOG_PATH: &str = "/api/v1/workflow-studio/catalog";
pub const WORKFLOW_STUDIO_DEFINITIONS_PATH: &str = "/api/v1/workflow-studio/definitions";
pub const WORKFLOW_STUDIO_SUBMITTABLE_DEFINITIONS_PATH: &str =
    "/api/v1/workflow-studio/submittable-definitions";
pub const WORKFLOW_STUDIO_DEFINITION_PATH_TEMPLATE: &str =
    "/api/v1/workflow-studio/definitions/{id}";
pub const WORKFLOW_STUDIO_DEFINITION_HISTORY_PATH_TEMPLATE: &str =
    "/api/v1/workflow-studio/definitions/{id}/history";
pub const WORKFLOW_STUDIO_DEFINITION_RUN_LOG_PATH_TEMPLATE: &str =
    "/api/v1/workflow-studio/definitions/{id}/run-log";
pub const WORKFLOW_STUDIO_DEFINITION_RUN_PATH_TEMPLATE: &str =
    "/api/v1/workflow-studio/definitions/{id}/run";
pub const WORKFLOW_STUDIO_DEFINITION_SIMULATE_PATH_TEMPLATE: &str =
    "/api/v1/workflow-studio/definitions/{id}/simulate";
pub const WORKFLOW_STUDIO_DEFINITION_PUBLISH_PATH_TEMPLATE: &str =
    "/api/v1/workflow-studio/definitions/{id}/publish";
pub const WORKFLOW_STUDIO_DEFINITION_APPROVE_REVISION_PATH_TEMPLATE: &str =
    "/api/v1/workflow-studio/definitions/{id}/revisions/{rev}/approve";
pub const WORKFLOW_STUDIO_DEFINITION_WITHDRAW_REVISION_PATH_TEMPLATE: &str =
    "/api/v1/workflow-studio/definitions/{id}/revisions/{rev}/withdraw";
pub const WORKFLOW_STUDIO_DEFINITION_PAUSE_PATH_TEMPLATE: &str =
    "/api/v1/workflow-studio/definitions/{id}/pause";
pub const WORKFLOW_STUDIO_DEFINITION_RESUME_PATH_TEMPLATE: &str =
    "/api/v1/workflow-studio/definitions/{id}/resume";
pub const WORKFLOW_STUDIO_DEFINITION_ROLLBACK_PATH_TEMPLATE: &str =
    "/api/v1/workflow-studio/definitions/{id}/rollback";
pub const WORKFLOW_STUDIO_DEFINITION_CLONE_PATH_TEMPLATE: &str =
    "/api/v1/workflow-studio/definitions/{id}/clone";
pub const WORKFLOW_STUDIO_DEFINITIONS_BY_OBJECT_KIND_PATH_TEMPLATE: &str =
    "/api/v1/workflow-studio/definitions/by-object-kind/{kind}";
pub const WORKFLOW_STUDIO_TRIGGER_BINDINGS_PATH: &str = "/api/v1/workflow-studio/trigger-bindings";
pub const WORKFLOW_STUDIO_TRIGGER_BINDING_ENABLE_PATH_TEMPLATE: &str =
    "/api/v1/workflow-studio/trigger-bindings/{id}/enable";
pub const WORKFLOW_STUDIO_TRIGGER_BINDING_DISABLE_PATH_TEMPLATE: &str =
    "/api/v1/workflow-studio/trigger-bindings/{id}/disable";
pub const WORKFLOW_STUDIO_SCHEDULES_PATH: &str = "/api/v1/workflow-studio/schedules";
pub const WORKFLOW_STUDIO_SCHEDULE_PATH_TEMPLATE: &str = "/api/v1/workflow-studio/schedules/{id}";
pub const WORKFLOW_STUDIO_SCHEDULE_PREVIEW_PATH: &str =
    "/api/v1/workflow-studio/schedules/preview-next-runs";
pub const WORKFLOW_STUDIO_SCHEDULE_RUNS_PATH_TEMPLATE: &str =
    "/api/v1/workflow-studio/schedules/{id}/runs";
pub const WORKFLOW_TASK_FINALIZE_PATH_TEMPLATE: &str = "/api/v1/workflow-tasks/{task_id}/finalize";
pub const WORKFLOW_RUN_POST_FINALIZATION_REJECTION_PATH_TEMPLATE: &str =
    "/api/v1/workflow-runs/{run_id}/post-finalization-rejection";
pub const WORKFLOW_RUNS_PATH: &str = "/api/v1/workflow-runs";
pub const WORKFLOW_RUNS_MINE_PATH: &str = "/api/v1/workflow-runs/mine";
pub const WORKFLOW_RUN_PATH_TEMPLATE: &str = "/api/v1/workflow-runs/{run_id}";
pub const WORKFLOW_TASKS_PATH: &str = "/api/v1/workflow-tasks";
pub const BULK_APPROVAL_INBOX_PATH: &str = "/api/v1/approval-inbox/bulk-tasks";
pub const WORKFLOW_TASK_CLAIM_PATH_TEMPLATE: &str = "/api/v1/workflow-tasks/{task_id}/claim";
pub const WORKFLOW_TASK_DECIDE_PATH_TEMPLATE: &str = "/api/v1/workflow-tasks/{task_id}/decide";
pub const WORKFLOW_STUDIO_ROUTE_PATHS: &[&str] = &[
    WORKFLOW_STUDIO_CATALOG_PATH,
    WORKFLOW_STUDIO_DEFINITIONS_PATH,
    WORKFLOW_STUDIO_DEFINITION_PATH_TEMPLATE,
    WORKFLOW_STUDIO_DEFINITION_HISTORY_PATH_TEMPLATE,
    WORKFLOW_STUDIO_DEFINITION_RUN_LOG_PATH_TEMPLATE,
    WORKFLOW_STUDIO_DEFINITION_RUN_PATH_TEMPLATE,
    WORKFLOW_STUDIO_DEFINITION_SIMULATE_PATH_TEMPLATE,
    WORKFLOW_STUDIO_DEFINITION_PUBLISH_PATH_TEMPLATE,
    WORKFLOW_STUDIO_DEFINITION_APPROVE_REVISION_PATH_TEMPLATE,
    WORKFLOW_STUDIO_DEFINITION_WITHDRAW_REVISION_PATH_TEMPLATE,
    WORKFLOW_STUDIO_DEFINITION_PAUSE_PATH_TEMPLATE,
    WORKFLOW_STUDIO_DEFINITION_RESUME_PATH_TEMPLATE,
    WORKFLOW_STUDIO_DEFINITION_ROLLBACK_PATH_TEMPLATE,
    WORKFLOW_STUDIO_DEFINITION_CLONE_PATH_TEMPLATE,
    WORKFLOW_STUDIO_SUBMITTABLE_DEFINITIONS_PATH,
    WORKFLOW_STUDIO_DEFINITIONS_BY_OBJECT_KIND_PATH_TEMPLATE,
    WORKFLOW_STUDIO_TRIGGER_BINDINGS_PATH,
    WORKFLOW_STUDIO_TRIGGER_BINDING_ENABLE_PATH_TEMPLATE,
    WORKFLOW_STUDIO_TRIGGER_BINDING_DISABLE_PATH_TEMPLATE,
    WORKFLOW_STUDIO_SCHEDULES_PATH,
    WORKFLOW_STUDIO_SCHEDULE_PATH_TEMPLATE,
    WORKFLOW_STUDIO_SCHEDULE_PREVIEW_PATH,
    WORKFLOW_STUDIO_SCHEDULE_RUNS_PATH_TEMPLATE,
    WORKFLOW_RUNS_PATH,
    WORKFLOW_RUNS_MINE_PATH,
    WORKFLOW_RUN_PATH_TEMPLATE,
    WORKFLOW_RUN_POST_FINALIZATION_REJECTION_PATH_TEMPLATE,
    WORKFLOW_TASKS_PATH,
    BULK_APPROVAL_INBOX_PATH,
    WORKFLOW_TASK_CLAIM_PATH_TEMPLATE,
    WORKFLOW_TASK_DECIDE_PATH_TEMPLATE,
    WORKFLOW_TASK_FINALIZE_PATH_TEMPLATE,
];

const WORKFLOW_STUDIO_REQUESTS_TOTAL: &str = "workflow_studio_requests_total";
const WORKFLOW_DEFINITION_SCHEMA_VERSION: &str = "workflow.definition.v1";
/// Bumped schema version for an *executable* definition: a run/node graph the M2
/// workflow runtime interprets (design §3 closes "definition JSONB has no node
/// graph"). `workflow.definition.v1` stays the authoring/policy schema; `wf.exec.v1`
/// additionally carries a `nodes` graph the runtime walks.
const WORKFLOW_EXEC_SCHEMA_VERSION: &str = "wf.exec.v1";
const POLICY_TEMPLATE_EQUIPMENT_LOCATION_ACCESS: &str = "equipment_location_access";
const POLICY_ACTION_START_WORK_ORDER: &str = "maintenance:StartWorkOrder";

const ALLOWED_CONNECTORS: &[ConnectorDescriptor] = &[
    ConnectorDescriptor {
        connector_key: "internal.approvals",
        display_name: "전자결재시스템",
        action_keys: &["request_approval", "notify_assignee"],
    },
    ConnectorDescriptor {
        connector_key: "internal.notifications",
        display_name: "알림",
        action_keys: &["send_badge", "send_push", "send_email_digest"],
    },
    ConnectorDescriptor {
        connector_key: "internal.mail",
        display_name: "업무 메일",
        action_keys: &["send_work_mail"],
    },
    ConnectorDescriptor {
        connector_key: "internal.audit",
        display_name: "감사 로그",
        action_keys: &["append_timeline_event"],
    },
    // JOB channel (design §E / M2 runtime): the transactional-outbox connector the
    // completion→approval→payroll template fans out through. `emit_payroll` enqueues
    // one `JOB` outbox event via this connector; publish-validation rejects the
    // template's action_allowlist entry `internal.jobs.draft_payroll_run` unless this
    // descriptor is allowlisted (see `maintenance_completion_execution_definition`).
    ConnectorDescriptor {
        connector_key: "internal.jobs",
        display_name: "백그라운드 잡",
        action_keys: &["draft_payroll_run"],
    },
];

const WORKFLOW_TEMPLATES: &[WorkflowTemplate] = &[
    WorkflowTemplate {
        template_key: "equipment_location_access_policy",
        display_name: "장비·위치 접근 정책",
        object_type: "equipment",
        required_approval_line: true,
        required_payment_line: false,
    },
    WorkflowTemplate {
        template_key: "maintenance_completion_approval",
        display_name: "정비 완료 승인",
        object_type: "work_order",
        required_approval_line: true,
        required_payment_line: false,
    },
    WorkflowTemplate {
        template_key: "purchase_payment_approval",
        display_name: "구매·정산 승인",
        object_type: "purchase_request",
        required_approval_line: true,
        required_payment_line: true,
    },
    WorkflowTemplate {
        template_key: "asset_transfer_signoff",
        display_name: "자산 이전 승인",
        object_type: "asset_transfer",
        required_approval_line: true,
        required_payment_line: false,
    },
];

#[allow(dead_code)]
const APPROVAL_TEMPLATES: &[ApprovalTemplate] = &[
    ApprovalTemplate {
        key: "ot",
        workflow_key: "approval.ot",
        reason_enum: &["업무 마감", "긴급 대응", "정기 점검", "기타"],
        linked_objects: &[ApprovalLinkedObject {
            kind: "work_order",
            required: false,
        }],
        default_line: &["team_lead_reviewer", "hr_approver"],
        receipt_required: false,
    },
    ApprovalTemplate {
        key: "leave",
        workflow_key: "approval.leave",
        reason_enum: &["개인 사유", "병가", "경조", "가족 돌봄", "기타"],
        linked_objects: &[ApprovalLinkedObject {
            kind: "attendance_schedule",
            required: true,
        }],
        default_line: &["manager_approver"],
        receipt_required: true,
    },
    ApprovalTemplate {
        key: "expense",
        workflow_key: "approval.expense",
        reason_enum: &["교통", "식대", "숙박", "소모품", "접대", "기타"],
        linked_objects: &[ApprovalLinkedObject {
            kind: "contract",
            required: false,
        }],
        default_line: &["team_lead_reviewer", "finance_approver"],
        receipt_required: false,
    },
    ApprovalTemplate {
        key: "sub",
        workflow_key: "approval.sub",
        reason_enum: &["결원 대체", "휴가 대체", "긴급 투입", "기타"],
        linked_objects: &[ApprovalLinkedObject {
            kind: "site",
            required: false,
        }],
        default_line: &["team_lead_reviewer", "hr_approver"],
        receipt_required: false,
    },
    ApprovalTemplate {
        key: "purchase",
        workflow_key: "approval.purchase",
        reason_enum: &["자재", "비품", "장비", "수리", "기타"],
        linked_objects: &[ApprovalLinkedObject {
            kind: "asset_or_inventory",
            required: false,
        }],
        default_line: &["team_lead_reviewer", "finance_approver"],
        receipt_required: false,
    },
    ApprovalTemplate {
        key: "benefit",
        workflow_key: "approval.benefit",
        reason_enum: &["경조", "자기계발", "건강검진", "포상", "기타"],
        linked_objects: &[ApprovalLinkedObject {
            kind: "payee",
            required: true,
        }],
        default_line: &["team_lead_reviewer", "hr_approver"],
        receipt_required: false,
    },
    ApprovalTemplate {
        key: "reimburse",
        workflow_key: "approval.reimburse",
        reason_enum: &["교통", "식대", "숙박", "소모품", "접대", "기타"],
        linked_objects: &[ApprovalLinkedObject {
            kind: "project_or_work",
            required: false,
        }],
        default_line: &["team_lead_reviewer", "finance_approver"],
        receipt_required: false,
    },
    ApprovalTemplate {
        key: "general",
        workflow_key: "approval.general",
        reason_enum: &["보고", "요청", "건의", "기타"],
        linked_objects: &[],
        default_line: &["team_lead_reviewer", "division_approver"],
        receipt_required: false,
    },
];

#[derive(Clone)]
pub struct WorkflowStudioState {
    pool: PgPool,
    jwt_verifier: Option<JwtVerifier>,
    passkey_step_up: Option<PasskeyService>,
}

impl WorkflowStudioState {
    #[must_use]
    pub fn new(pool: PgPool, jwt_verifier: Option<JwtVerifier>) -> Self {
        Self {
            pool,
            jwt_verifier,
            passkey_step_up: None,
        }
    }

    #[must_use]
    pub fn with_passkey_step_up(mut self, passkey_step_up: Option<PasskeyService>) -> Self {
        self.passkey_step_up = passkey_step_up;
        self
    }
}

pub fn router(state: WorkflowStudioState) -> Router {
    let verifier = state.jwt_verifier.clone();
    let pool = state.pool.clone();
    let router = Router::new()
        .route(WORKFLOW_STUDIO_CATALOG_PATH, get(get_catalog))
        .route(
            WORKFLOW_STUDIO_DEFINITIONS_PATH,
            get(list_definitions).post(create_definition),
        )
        .route(
            WORKFLOW_STUDIO_SUBMITTABLE_DEFINITIONS_PATH,
            get(list_submittable_definitions),
        )
        .route(
            WORKFLOW_STUDIO_DEFINITION_PATH_TEMPLATE,
            patch(update_definition).delete(archive_definition),
        )
        .route(
            WORKFLOW_STUDIO_DEFINITION_HISTORY_PATH_TEMPLATE,
            get(list_definition_history),
        )
        .route(
            WORKFLOW_STUDIO_DEFINITION_RUN_LOG_PATH_TEMPLATE,
            get(list_definition_run_log),
        )
        .route(
            WORKFLOW_STUDIO_DEFINITION_RUN_PATH_TEMPLATE,
            post(trigger_definition_run),
        )
        .route(
            WORKFLOW_STUDIO_DEFINITION_SIMULATE_PATH_TEMPLATE,
            post(simulate_definition),
        )
        .route(
            WORKFLOW_STUDIO_DEFINITION_PUBLISH_PATH_TEMPLATE,
            post(publish_definition),
        )
        .route(
            WORKFLOW_STUDIO_DEFINITION_APPROVE_REVISION_PATH_TEMPLATE,
            post(approve_revision),
        )
        .route(
            WORKFLOW_STUDIO_DEFINITION_WITHDRAW_REVISION_PATH_TEMPLATE,
            post(withdraw_revision),
        )
        .route(
            WORKFLOW_STUDIO_DEFINITION_PAUSE_PATH_TEMPLATE,
            post(pause_definition),
        )
        .route(
            WORKFLOW_STUDIO_DEFINITION_RESUME_PATH_TEMPLATE,
            post(resume_definition),
        )
        .route(
            WORKFLOW_STUDIO_DEFINITION_ROLLBACK_PATH_TEMPLATE,
            post(rollback_definition),
        )
        .route(
            WORKFLOW_STUDIO_DEFINITION_CLONE_PATH_TEMPLATE,
            post(clone_definition),
        )
        .route(
            WORKFLOW_STUDIO_DEFINITIONS_BY_OBJECT_KIND_PATH_TEMPLATE,
            get(list_definitions_by_object_kind),
        )
        .route(
            WORKFLOW_STUDIO_TRIGGER_BINDINGS_PATH,
            get(list_trigger_bindings).post(create_trigger_binding),
        )
        .route(
            WORKFLOW_STUDIO_TRIGGER_BINDING_ENABLE_PATH_TEMPLATE,
            post(enable_trigger_binding),
        )
        .route(
            WORKFLOW_STUDIO_TRIGGER_BINDING_DISABLE_PATH_TEMPLATE,
            post(disable_trigger_binding),
        )
        .route(
            WORKFLOW_STUDIO_SCHEDULES_PATH,
            get(list_schedules).post(create_schedule),
        )
        .route(
            WORKFLOW_STUDIO_SCHEDULE_PATH_TEMPLATE,
            patch(update_schedule),
        )
        .route(
            WORKFLOW_STUDIO_SCHEDULE_PREVIEW_PATH,
            post(preview_schedule_next_runs),
        )
        .route(
            WORKFLOW_STUDIO_SCHEDULE_RUNS_PATH_TEMPLATE,
            get(list_schedule_runs),
        )
        .route(WORKFLOW_TASK_FINALIZE_PATH_TEMPLATE, post(finalize_task))
        .route(
            WORKFLOW_RUN_POST_FINALIZATION_REJECTION_PATH_TEMPLATE,
            post(create_post_finalization_rejection),
        )
        .route(
            WORKFLOW_RUNS_PATH,
            post(start_workflow_run).get(list_workflow_runs_admin),
        )
        .route(WORKFLOW_RUNS_MINE_PATH, get(list_my_workflow_runs))
        .route(WORKFLOW_RUN_PATH_TEMPLATE, get(get_workflow_run))
        .route(WORKFLOW_TASKS_PATH, get(list_workflow_tasks))
        .route(BULK_APPROVAL_INBOX_PATH, get(list_bulk_approval_inbox))
        .route(WORKFLOW_TASK_CLAIM_PATH_TEMPLATE, post(claim_workflow_task))
        .route(
            WORKFLOW_TASK_DECIDE_PATH_TEMPLATE,
            post(decide_workflow_task),
        )
        .with_state(state);
    console_platform_request_context::with_request_context(router, verifier, pool)
}

#[derive(Debug, Serialize)]
struct WorkflowStudioCatalogResponse {
    connectors: Vec<ConnectorResponse>,
    templates: Vec<WorkflowTemplateResponse>,
}

#[derive(Debug, Clone, Copy)]
struct ConnectorDescriptor {
    connector_key: &'static str,
    display_name: &'static str,
    action_keys: &'static [&'static str],
}

#[derive(Debug, Serialize)]
struct ConnectorResponse {
    connector_key: String,
    display_name: String,
    action_keys: Vec<String>,
}

#[derive(Debug, Clone, Copy)]
struct WorkflowTemplate {
    template_key: &'static str,
    display_name: &'static str,
    object_type: &'static str,
    required_approval_line: bool,
    required_payment_line: bool,
}

#[derive(Debug, Clone, Copy)]
#[allow(dead_code)]
struct ApprovalTemplate {
    key: &'static str,
    workflow_key: &'static str,
    reason_enum: &'static [&'static str],
    linked_objects: &'static [ApprovalLinkedObject],
    default_line: &'static [&'static str],
    receipt_required: bool,
}

#[derive(Debug, Clone, Copy)]
#[allow(dead_code)]
struct ApprovalLinkedObject {
    kind: &'static str,
    required: bool,
}

#[derive(Debug, Serialize)]
struct WorkflowTemplateResponse {
    template_key: String,
    display_name: String,
    object_type: String,
    required_approval_line: bool,
    required_payment_line: bool,
}

#[derive(Debug, Serialize)]
struct WorkflowDefinitionListResponse {
    items: Vec<WorkflowDefinitionResponse>,
}

#[derive(Debug, Serialize)]
struct WorkflowDefinitionHistoryResponse {
    items: Vec<WorkflowDefinitionEventResponse>,
}

#[derive(Debug, Serialize)]
struct WorkflowRunLogResponse {
    items: Vec<WorkflowRunResponse>,
}

#[derive(Debug, Serialize)]
struct WorkflowRunResponse {
    id: Uuid,
    code: String,
    definition_id: Uuid,
    definition_version: i32,
    trigger_type: String,
    status: String,
    actor_display_name: Option<String>,
    summary: String,
    error_message: Option<String>,
    generated_objects: Vec<String>,
    #[serde(with = "time::serde::rfc3339")]
    started_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    updated_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339::option")]
    completed_at: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339::option")]
    failed_at: Option<OffsetDateTime>,
}

#[derive(Debug, Serialize, Clone)]
struct WorkflowDefinitionResponse {
    id: Uuid,
    workflow_key: String,
    display_name: String,
    object_type: String,
    status: String,
    latest_version: i32,
    active_version: Option<i32>,
    definition: Value,
    approval_line: Vec<Value>,
    payment_line: Vec<Value>,
    notification_rules: Vec<Value>,
    action_allowlist: Vec<Value>,
    required_approval_line: bool,
    required_payment_line: bool,
    /// The ontology object kinds this definition's nodes touch (dynamics↔ontology).
    object_kinds: Vec<String>,
    /// A staged revision (version number) awaiting four-eyes approval; the active
    /// version keeps serving until then. `None` when no revision is pending.
    pending_version: Option<i32>,
    /// Who staged the pending revision (the actor barred from self-approving it).
    pending_staged_by: Option<Uuid>,
    #[serde(with = "time::serde::rfc3339")]
    created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    updated_at: OffsetDateTime,
}

#[derive(Debug, Serialize)]
struct WorkflowDefinitionEventResponse {
    id: Uuid,
    definition_id: Uuid,
    version: Option<i32>,
    status: String,
    action: String,
    actor_display_name: Option<String>,
    summary: String,
    #[serde(with = "time::serde::rfc3339")]
    created_at: OffsetDateTime,
}

#[derive(Debug, Deserialize)]
struct TriggerWorkflowRunRequest {
    #[serde(default = "default_workflow_run_trigger_type")]
    trigger_type: String,
    #[serde(default)]
    idempotency_key: Option<String>,
    /// §16 org-scope automation gate (85 판정): the caller's `gov_approvals`
    /// request ref for org-scope automations (§3.9.0-① personal-scope ignores
    /// this and runs direct).
    #[serde(default)]
    four_eyes_request_ref: Option<Uuid>,
}

#[derive(Debug, Deserialize)]
struct CreateWorkflowDefinitionRequest {
    workflow_key: String,
    display_name: String,
    object_type: String,
    #[serde(default = "empty_object")]
    definition: Value,
    #[serde(default)]
    approval_line: Vec<Value>,
    #[serde(default)]
    payment_line: Vec<Value>,
    #[serde(default)]
    notification_rules: Vec<Value>,
    #[serde(default)]
    action_allowlist: Vec<Value>,
    #[serde(default)]
    required_approval_line: bool,
    #[serde(default)]
    required_payment_line: bool,
}

#[derive(Debug, Deserialize)]
struct UpdateWorkflowDefinitionRequest {
    #[serde(default)]
    display_name: Option<String>,
    #[serde(default)]
    definition: Option<Value>,
    #[serde(default)]
    approval_line: Option<Vec<Value>>,
    #[serde(default)]
    payment_line: Option<Vec<Value>>,
    #[serde(default)]
    notification_rules: Option<Vec<Value>>,
    #[serde(default)]
    action_allowlist: Option<Vec<Value>>,
    #[serde(default)]
    required_approval_line: Option<bool>,
    #[serde(default)]
    required_payment_line: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct WorkflowStepUpRequest {
    #[serde(default)]
    step_up: Option<WorkflowStepUpAssertionRequest>,
    /// §16 org-scope automation gate (85 판정): required on `publish` for a
    /// never-activated org-scope definition; ignored by every other handler
    /// that shares this request shape.
    #[serde(default)]
    four_eyes_request_ref: Option<Uuid>,
}

#[derive(Debug, Deserialize)]
struct RollbackWorkflowDefinitionRequest {
    target_version: i32,
    #[serde(default)]
    step_up: Option<WorkflowStepUpAssertionRequest>,
}

#[derive(Debug, Deserialize)]
struct CloneWorkflowDefinitionRequest {
    #[serde(default)]
    workflow_key: Option<String>,
    #[serde(default)]
    display_name: Option<String>,
    #[serde(default)]
    step_up: Option<WorkflowStepUpAssertionRequest>,
}

#[derive(Debug, Deserialize)]
struct SimulateWorkflowDefinitionRequest {
    #[serde(default)]
    definition: Option<Value>,
    #[serde(default)]
    approval_line: Option<Vec<Value>>,
    #[serde(default)]
    payment_line: Option<Vec<Value>>,
    #[serde(default)]
    notification_rules: Option<Vec<Value>>,
    #[serde(default)]
    action_allowlist: Option<Vec<Value>>,
    /// Sample run context to exercise condition/branch nodes against (defaults to
    /// `{}`). The response's `simulated_path` reports the node keys that would
    /// execute for this context — the branch actually taken.
    #[serde(default)]
    sample_context: Option<Value>,
}

#[derive(Debug, Deserialize)]
struct WorkflowStepUpAssertionRequest {
    ceremony_id: Uuid,
    credential: PasskeyAuthenticationCredential,
}

#[derive(Debug, Deserialize)]
struct FinalizeWorkflowTaskRequest {
    mode: FinalizeWorkflowTaskMode,
    #[serde(default)]
    reason: Option<String>,
    idempotency_key: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum FinalizeWorkflowTaskMode {
    Author,
    Delegate,
}

impl FinalizeWorkflowTaskMode {
    const fn as_str(&self) -> &'static str {
        match self {
            Self::Author => "author",
            Self::Delegate => "delegate",
        }
    }

    const fn policy_mode(&self) -> FinalizeMode {
        match self {
            Self::Author => FinalizeMode::Author,
            Self::Delegate => FinalizeMode::Delegate,
        }
    }
}

#[derive(Debug, Serialize)]
struct FinalizeWorkflowTaskResponse {
    task: FinalizedTaskResponse,
    run: FinalizedRunResponse,
    #[serde(skip_serializing_if = "Option::is_none")]
    archive_ref: Option<Value>,
}

#[derive(Debug, Serialize)]
struct FinalizedTaskResponse {
    id: Uuid,
    run_id: Uuid,
    status: String,
    completed_by: Option<UserId>,
    decision_payload: Value,
}

#[derive(Debug, Serialize)]
struct FinalizedRunResponse {
    id: Uuid,
    status: String,
}

#[derive(Debug, Deserialize)]
struct PostFinalizationRejectionRequest {
    reason: String,
    idempotency_key: String,
}

#[derive(Debug, Serialize)]
struct PostFinalizationRejectionResponse {
    compensation: PostFinalizationRejectionDocumentResponse,
    run: FinalizedRunResponse,
}

#[derive(Debug, Serialize)]
struct PostFinalizationRejectionDocumentResponse {
    id: Uuid,
    original_run_id: Uuid,
    reason: String,
    created_by: UserId,
}

#[derive(Debug, Serialize)]
struct WorkflowSimulationResponse {
    decision: String,
    findings: Vec<WorkflowSimulationFinding>,
    /// For a `wf.exec.v1` definition: the ordered node keys that WOULD execute
    /// for the sample context — the branch actually taken through any condition
    /// nodes, stopping at the first human task or a terminal node. `None` for a
    /// non-executable (authoring/policy-only) definition.
    #[serde(skip_serializing_if = "Option::is_none")]
    simulated_path: Option<Vec<String>>,
}

#[derive(Debug, Serialize)]
struct WorkflowSimulationFinding {
    severity: String,
    code: String,
    message: String,
}

#[derive(Debug, Clone)]
struct WorkflowVersionRow {
    definition_id: Uuid,
    workflow_key: String,
    display_name: String,
    object_type: String,
    status: String,
    latest_version: i32,
    active_version: Option<i32>,
    definition: Value,
    approval_line: Vec<Value>,
    payment_line: Vec<Value>,
    notification_rules: Vec<Value>,
    action_allowlist: Vec<Value>,
    required_approval_line: bool,
    required_payment_line: bool,
    pending_version: Option<i32>,
    pending_staged_by: Option<Uuid>,
    created_at: OffsetDateTime,
    updated_at: OffsetDateTime,
}

async fn get_catalog(
    Extension(principal): Extension<Principal>,
) -> Result<Json<WorkflowStudioCatalogResponse>, WorkflowStudioError> {
    authorize_workflow_manage(&principal)?;
    record_workflow_studio_request("catalog", "success");
    Ok(Json(WorkflowStudioCatalogResponse {
        connectors: ALLOWED_CONNECTORS
            .iter()
            .map(|connector| ConnectorResponse {
                connector_key: connector.connector_key.to_owned(),
                display_name: connector.display_name.to_owned(),
                action_keys: connector
                    .action_keys
                    .iter()
                    .map(|action| (*action).to_owned())
                    .collect(),
            })
            .collect(),
        templates: WORKFLOW_TEMPLATES
            .iter()
            .map(|template| WorkflowTemplateResponse {
                template_key: template.template_key.to_owned(),
                display_name: template.display_name.to_owned(),
                object_type: template.object_type.to_owned(),
                required_approval_line: template.required_approval_line,
                required_payment_line: template.required_payment_line,
            })
            .collect(),
    }))
}

async fn finalize_task(
    State(state): State<WorkflowStudioState>,
    Extension(principal): Extension<Principal>,
    Path(task_id): Path<Uuid>,
    Json(request): Json<FinalizeWorkflowTaskRequest>,
) -> Result<Json<FinalizeWorkflowTaskResponse>, WorkflowStudioError> {
    let idempotency_key = request.idempotency_key.trim();
    if idempotency_key.len() < 16 {
        return Err(WorkflowStudioError::validation(
            "idempotency_key must be at least 16 characters",
        ));
    }

    let store = PgWorkflowRuntimeStore::new(state.pool.clone());
    let context = store
        .load_finalize_waiting_task(principal.org_id, task_id)
        .await?
        .ok_or_else(|| KernelError::not_found("workflow task not found"))?;
    if context.waiting_key != "finalize.author"
        && context.required_policy.as_deref() != Some("approval_finalize")
    {
        return Err(WorkflowStudioError::validation(
            "workflow task is not a finalization task",
        ));
    }

    let branch = WORKFLOW_SPINE_HAS_NO_BRANCH;
    let resource_type = context.object_type.as_deref().unwrap_or("workflow_run");
    let resource_id = context
        .object_id
        .map(|id| id.to_string())
        .unwrap_or_else(|| context.run_id.to_string());
    let shadow_resource_id = resource_id.clone();
    let policy = enforce_spine_finalize_policy(
        &principal,
        &context,
        request.mode.policy_mode(),
        request.reason.as_deref(),
        resource_type,
        &resource_id,
    )?;

    let mut audits = Vec::new();
    if let Some(guard_audit) = policy.guard_audit {
        audits.push(shadow_audit_event(
            &guard_audit,
            principal.user_id,
            principal.org_id,
            task_id,
        )?);
    }

    // Enrollment wave 2: audit-only Cedar parity observation. Legacy
    // (`enforce_finalize_policy`) already enforced above (this line is only
    // reached on its ALLOW); the shadow records how Cedar-alone compares and can
    // never affect the finalize.
    let shadow_resource = shadow_resource(
        principal.org_id,
        branch,
        context
            .object_type
            .as_deref()
            .unwrap_or("workflow_run")
            .to_owned(),
    )
    .with_resource_id(shadow_resource_id);
    crate::cedar_parity::observe_parity(
        &state.pool,
        &principal,
        principal.org_id,
        Feature::ApprovalFinalize,
        shadow_resource,
        crate::cedar_parity::WORKFLOW_DECIDE_DOMAIN,
        crate::cedar_parity::CEDAR_PBAC_SHADOW_WORKFLOW_DECIDE_FLAG,
        true,
    )
    .await;

    let finalized = store
        .finalize_waiting_task(
            principal.org_id,
            FinalizeWaitingTaskCommand {
                task_id,
                actor: principal.user_id,
                idempotency_key: idempotency_key.to_owned(),
                mode: request.mode.as_str().to_owned(),
                delegated_reason: policy.delegated_reason,
                transition_audits: audits,
            },
        )
        .await?;

    record_workflow_studio_request("task_finalize", "success");
    Ok(Json(FinalizeWorkflowTaskResponse {
        task: FinalizedTaskResponse {
            id: finalized.task_id,
            run_id: finalized.run_id,
            status: finalized.status.as_db_str().to_owned(),
            completed_by: finalized.completed_by,
            decision_payload: finalized.decision_payload,
        },
        run: FinalizedRunResponse {
            id: finalized.run_id,
            status: finalized.run_status.as_db_str().to_owned(),
        },
        archive_ref: None,
    }))
}

/// Post-finalization rejection: reverse an already-finalized run with a
/// compensating document.
///
/// AUTHORITY IS ORG-WIDE BY DESIGN (security M4, DESIGN §2): the guard is
/// [`Feature::ApprovalFinalize`] with no branch/object narrowing, so an
/// 감사·컴플라이언스·CEO principal holding that feature may compensate ANY finalized
/// run across the tenant. This is the charter's reversal authority — a documented
/// decision, not a missing scope check. Narrowing it would break the
/// audit/compliance reversal path.
async fn create_post_finalization_rejection(
    State(state): State<WorkflowStudioState>,
    Extension(principal): Extension<Principal>,
    Path(run_id): Path<Uuid>,
    Json(request): Json<PostFinalizationRejectionRequest>,
) -> Result<Json<PostFinalizationRejectionResponse>, WorkflowStudioError> {
    let idempotency_key = request.idempotency_key.trim();
    if idempotency_key.len() < 16 {
        return Err(WorkflowStudioError::validation(
            "idempotency_key must be at least 16 characters",
        ));
    }
    let reason = request.reason.trim();
    if reason.is_empty() {
        return Err(WorkflowStudioError::validation(
            "post-finalization rejection requires a non-empty reason",
        ));
    }

    let branch = WORKFLOW_SPINE_HAS_NO_BRANCH;
    let authz_request = spine_guard_request(
        &principal,
        Feature::ApprovalFinalize.as_str(),
        principal.org_id,
        branch,
        "workflow_run",
        &run_id.to_string(),
        WAITING_COMPLETION_DOMAIN,
    )
    .map_err(|_| {
        WorkflowStudioError::from(KernelError::forbidden(
            "post-finalization rejection policy denied",
        ))
    })?;
    let entry = workflow_coexistence_entry(
        "workflow.waiting_task.post_finalization_rejection",
        WAITING_COMPLETION_DOMAIN,
        Feature::ApprovalFinalize,
        "workflow_run",
    );
    let guard_outcome = guard(&authz_request, &entry);
    if !guard_outcome.is_allowed() {
        return Err(WorkflowStudioError::from(KernelError::forbidden(
            "post-finalization rejection policy denied",
        )));
    }

    // Enrollment wave 2: audit-only Cedar parity observation (legacy already
    // enforced above). Scope is branch + run-specific so the parity row mirrors
    // the workflow run the already-enforced legacy decision acted on.
    let shadow_resource = shadow_resource(principal.org_id, branch, "workflow_run")
        .with_resource_id(run_id.to_string());
    crate::cedar_parity::observe_parity(
        &state.pool,
        &principal,
        principal.org_id,
        Feature::ApprovalFinalize,
        shadow_resource,
        crate::cedar_parity::WORKFLOW_DECIDE_DOMAIN,
        crate::cedar_parity::CEDAR_PBAC_SHADOW_WORKFLOW_DECIDE_FLAG,
        true,
    )
    .await;

    let store = PgWorkflowRuntimeStore::new(state.pool.clone());
    let compensation = store
        .create_post_finalization_rejection(
            principal.org_id,
            PostFinalizationRejectionCommand {
                original_run_id: run_id,
                actor: principal.user_id,
                reason: reason.to_owned(),
                idempotency_key: idempotency_key.to_owned(),
                transition_audits: vec![shadow_audit_event_for(
                    &guard_outcome.audit,
                    principal.user_id,
                    principal.org_id,
                    "workflow_run",
                    run_id,
                )?],
            },
        )
        .await?;

    record_workflow_studio_request("post_finalization_rejection", "success");
    Ok(Json(PostFinalizationRejectionResponse {
        compensation: PostFinalizationRejectionDocumentResponse {
            id: compensation.id,
            original_run_id: compensation.original_run_id,
            reason: compensation.reason,
            created_by: compensation.created_by,
        },
        run: FinalizedRunResponse {
            id: compensation.original_run_id,
            status: compensation.run_status.as_db_str().to_owned(),
        },
    }))
}

// ===========================================================================
// Instance / task REST surface (engine-gen spike §"Instance/Task REST Surface").
// ===========================================================================

#[derive(Debug, Deserialize)]
struct StartWorkflowRunRequest {
    definition_id: Uuid,
    #[serde(default)]
    definition_version: Option<i32>,
    #[serde(default)]
    object_type: Option<String>,
    #[serde(default)]
    object_id: Option<Uuid>,
    trigger_type: TriggerType,
    idempotency_key: String,
    #[serde(default)]
    correlation_id: Option<String>,
    #[serde(default = "empty_object")]
    input_payload: Value,
    #[serde(default = "empty_object")]
    context_payload: Value,
}

#[derive(Debug, Serialize)]
struct StartWorkflowRunResponse {
    run: RunSummaryResponse,
    #[serde(skip_serializing_if = "Option::is_none")]
    next_task: Option<TaskSummaryResponse>,
}

#[derive(Debug, Serialize)]
struct RunSummaryResponse {
    id: Uuid,
    status: String,
    definition_id: Uuid,
    definition_version: i32,
    object_type: Option<String>,
    object_id: Option<Uuid>,
    initiated_by: Option<Uuid>,
    #[serde(with = "time::serde::rfc3339")]
    started_at: OffsetDateTime,
}

#[derive(Debug, Serialize)]
struct TaskSummaryResponse {
    task_id: Uuid,
    run_id: Uuid,
    waiting_key: String,
    title: String,
    assignee_role_key: Option<String>,
    required_policy: Option<String>,
    object_type: Option<String>,
    object_id: Option<Uuid>,
    status: String,
    claimed_by: Option<Uuid>,
    #[serde(with = "time::serde::rfc3339::option")]
    due_at: Option<OffsetDateTime>,
    form_payload: Value,
}

impl From<WaitingTaskListItem> for TaskSummaryResponse {
    fn from(item: WaitingTaskListItem) -> Self {
        Self {
            task_id: item.task_id,
            run_id: item.run_id,
            waiting_key: item.waiting_key,
            title: item.title,
            assignee_role_key: item.assignee_role_key,
            required_policy: item.required_policy,
            object_type: item.object_type,
            object_id: item.object_id,
            status: item.status.as_db_str().to_owned(),
            claimed_by: item.claimed_by,
            due_at: item.due_at,
            form_payload: item.form_payload,
        }
    }
}

#[derive(Debug, Serialize)]
struct WorkflowTaskListResponse {
    items: Vec<TaskSummaryResponse>,
}

#[derive(Debug, Deserialize)]
struct TaskListQuery {
    #[serde(default)]
    role_key: Option<String>,
    #[serde(default)]
    assignee: Option<String>,
    #[serde(default)]
    status: Option<String>,
}

#[derive(Debug, Serialize)]
struct RunListResponse {
    items: Vec<RunListItemResponse>,
}

#[derive(Debug, Serialize)]
struct RunListItemResponse {
    run_id: Uuid,
    status: String,
    definition_id: Uuid,
    definition_version: i32,
    object_type: Option<String>,
    object_id: Option<Uuid>,
    initiated_by: Option<Uuid>,
    #[serde(with = "time::serde::rfc3339")]
    started_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    updated_at: OffsetDateTime,
}

impl From<RunListItem> for RunListItemResponse {
    fn from(item: RunListItem) -> Self {
        Self {
            run_id: item.run_id,
            status: item.status.as_db_str().to_owned(),
            definition_id: item.definition_id,
            definition_version: item.definition_version,
            object_type: item.object_type,
            object_id: item.object_id,
            initiated_by: item.initiated_by,
            started_at: item.started_at,
            updated_at: item.updated_at,
        }
    }
}

#[derive(Debug, Deserialize)]
struct RunListQuery {
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    object_type: Option<String>,
    // `q` (free-text search): case-insensitive substring match over the submission
    // row's human-readable content (object_type + input_payload). Applied inside the
    // org-scoped, initiator-scoped query — it only ever narrows, never widens.
    #[serde(default)]
    q: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ClaimWorkflowTaskRequest {
    idempotency_key: String,
}

#[derive(Debug, Serialize)]
struct ClaimTaskResponse {
    task: ClaimedTaskResponse,
}

#[derive(Debug, Serialize)]
struct ClaimedTaskResponse {
    task_id: Uuid,
    run_id: Uuid,
    status: String,
    claimed_by: Option<Uuid>,
    #[serde(with = "time::serde::rfc3339::option")]
    claimed_at: Option<OffsetDateTime>,
}

#[derive(Debug, Deserialize)]
struct DecideWorkflowTaskRequest {
    decision: DecisionRequest,
    #[serde(default)]
    comment: Option<String>,
    idempotency_key: String,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
enum DecisionRequest {
    Approve,
    Reject,
    Return,
}

impl From<DecisionRequest> for TaskDecision {
    fn from(value: DecisionRequest) -> Self {
        match value {
            DecisionRequest::Approve => Self::Approve,
            DecisionRequest::Reject => Self::Reject,
            DecisionRequest::Return => Self::Return,
        }
    }
}

#[derive(Debug, Serialize)]
struct DecideTaskResponse {
    task: DecidedTaskResponse,
    run: FinalizedRunResponse,
    #[serde(skip_serializing_if = "Option::is_none")]
    next_task: Option<TaskSummaryResponse>,
}

#[derive(Debug, Serialize)]
struct DecidedTaskResponse {
    task_id: Uuid,
    run_id: Uuid,
    status: String,
    decision_payload: Value,
}

/// Map an approval `required_policy` string to a canonical legacy `Feature` key so
/// the guard is well-defined without extending the frozen legacy permission matrix.
/// `approval_review`/`approval_decide` reuse `completion_review`; the closeout
/// policies reuse `approval_finalize`. Any other value must already be a real
/// `Feature` key, else the caller treats it as a deny (fail closed).
fn guard_policy(required_policy: &str) -> Option<String> {
    match required_policy {
        "approval_review" | "approval_decide" => Some("completion_review".to_owned()),
        "approval_finalize" | "approval_receipt" => Some("approval_finalize".to_owned()),
        other => Feature::from_str(other).ok().map(|_| other.to_owned()),
    }
}

/// Serialized-size ceiling for a caller-supplied run payload (security M2). 64 KiB
/// is far above any legitimate approval input; over it is a 422.
const MAX_RUN_PAYLOAD_BYTES: usize = 64 * 1024;

/// Reject an oversize `input_payload`/`context_payload` (security M2) with a 422.
fn check_payload_size(field: &str, payload: &Value) -> Result<(), WorkflowStudioError> {
    // Cheap upper bound: serialize once and measure. Bounded by the axum body
    // limit already, but an explicit per-field cap keeps a single 64 KiB blob from
    // riding in as run state.
    let len = serde_json::to_vec(payload)
        .map(|bytes| bytes.len())
        .unwrap_or(usize::MAX);
    if len > MAX_RUN_PAYLOAD_BYTES {
        return Err(WorkflowStudioError::validation(format!(
            "{field} must be at most {MAX_RUN_PAYLOAD_BYTES} bytes serialized"
        )));
    }
    Ok(())
}

/// Fail-closed guard for the claim/decide action path (security H1, defense in
/// depth): a waiting task with no `required_policy` carries no authorization
/// boundary. Authoring now REQUIRES one (`validate_execution_graph`), so a
/// policy-less row can only be a legacy record — refuse to act on it with a 403
/// rather than fall through `guard_task_policy`'s ungated `Ok(None)` path (that
/// path is for self-service *run starts*, never for acting on an existing task).
fn require_task_authorization_boundary(
    required_policy: Option<&str>,
) -> Result<(), WorkflowStudioError> {
    if required_policy.is_none() {
        return Err(WorkflowStudioError::from(KernelError::forbidden(
            "task has no authorization boundary",
        )));
    }
    Ok(())
}

/// Legacy-enforce + Cedar-shadow guard for a task/run policy. Returns the shadow
/// audit event to fold into the mutation (`None` when the task carries no policy —
/// an ungated self-service step), or a 403 when the legacy contract denies.
#[allow(clippy::too_many_arguments)]
fn guard_task_policy(
    principal: &Principal,
    org: console_kernel_core::OrgId,
    branch: Option<BranchId>,
    required_policy: Option<&str>,
    resource_type: &str,
    resource_id: &str,
    action_id: &'static str,
    shadow_target_id: Uuid,
) -> Result<Option<AuditEvent>, WorkflowStudioError> {
    let Some(policy) = required_policy else {
        return Ok(None);
    };
    let feature_key = guard_policy(policy).ok_or_else(|| {
        WorkflowStudioError::from(KernelError::forbidden("workflow task policy is unknown"))
    })?;
    let feature = Feature::from_str(&feature_key).map_err(|_| {
        WorkflowStudioError::from(KernelError::forbidden("workflow task policy is unknown"))
    })?;
    let request = spine_guard_request(
        principal,
        &feature_key,
        org,
        branch,
        resource_type,
        resource_id,
        WAITING_COMPLETION_DOMAIN,
    )
    .map_err(|_| {
        WorkflowStudioError::from(KernelError::forbidden("workflow task policy denied"))
    })?;
    let entry = workflow_coexistence_entry(
        action_id,
        WAITING_COMPLETION_DOMAIN,
        feature,
        resource_type.to_owned(),
    );
    let outcome = guard(&request, &entry);
    if !outcome.is_allowed() {
        return Err(WorkflowStudioError::from(KernelError::forbidden(
            "workflow task policy denied",
        )));
    }
    Ok(Some(shadow_audit_event(
        &outcome.audit,
        principal.user_id,
        org,
        shadow_target_id,
    )?))
}

/// Enrollment wave 2: fire the audit-only Cedar parity observation for a
/// decide/claim task guard that legacy just ALLOWED (this is only called after
/// `guard_task_policy` returned `Ok`). Best-effort and side-effect-only — it can
/// never affect the mutation. Records nothing for a policy-less task (there is no
/// capability decision to compare).
async fn observe_task_decide_parity(
    pool: &PgPool,
    principal: &Principal,
    org: console_kernel_core::OrgId,
    branch: Option<BranchId>,
    required_policy: Option<&str>,
    resource_type: &str,
    resource_id: &str,
) {
    let Some(feature) = required_policy
        .and_then(guard_policy)
        .and_then(|key| Feature::from_str(&key).ok())
    else {
        return;
    };
    let resource = shadow_resource(org, branch, resource_type.to_owned())
        .with_resource_id(resource_id.to_owned());
    crate::cedar_parity::observe_parity(
        pool,
        principal,
        org,
        feature,
        resource,
        crate::cedar_parity::WORKFLOW_DECIDE_DOMAIN,
        crate::cedar_parity::CEDAR_PBAC_SHADOW_WORKFLOW_DECIDE_FLAG,
        true,
    )
    .await;
}

/// The workflow authority role keys resolved through the legacy matrix (security
/// M3). All map to `completion_review` — the review/decide/approve tiers of the
/// approval and completion lines — so "holds this role key" reuses the SAME guard
/// `task_visible` runs, never a parallel role system.
const WORKFLOW_AUTHORITY_ROLE_KEYS: [&str; 4] =
    ["hr_reviewer", "manager_approver", "executive", "admin"];

/// How a principal can "hold" a human-task `assignee_role_key` (security M3).
enum RoleKeyKind {
    /// Held when the principal passes the guard for this legacy feature key —
    /// the same matrix guard `guard_policy`/`task_visible` already use.
    Authority(&'static str),
    /// Held only by the run's initiator (`workflow_runs.initiated_by == me`), a
    /// fact the feature matrix cannot express. The task still carries a broad
    /// policy (e.g. `approval_finalize`), so ownership — not policy — is the
    /// addressee boundary.
    Ownership,
}

/// Classify a human-task `assignee_role_key` (security M3). Authority keys reuse
/// the matrix guard; ownership keys bind to the run initiator. An unknown key is
/// held by no one (fail closed) — it surfaces only when claimed.
///
/// ponytail: `receipt_subject` has no per-user binding column yet
/// (`workflow_waiting_tasks.assignee_user_id` is never written on insert), so it
/// cannot be scoped to its subject — it is left unclassified (deny) until a
/// subject column exists. In-scope templates use only
/// hr_reviewer/manager_approver/initiator.
fn classify_role_key(role_key: &str) -> Option<RoleKeyKind> {
    if WORKFLOW_AUTHORITY_ROLE_KEYS.contains(&role_key) {
        return Some(RoleKeyKind::Authority("completion_review"));
    }
    match role_key {
        "initiator" => Some(RoleKeyKind::Ownership),
        _ => None,
    }
}

/// Whether the principal passes the legacy guard for a bare feature capability
/// (security M3), reusing the exact `build_guard_request`/`guard` path
/// `task_visible` uses — no concrete resource, since role membership is a
/// capability question, not a per-object one.
fn principal_holds_feature(
    principal: &Principal,
    org: console_kernel_core::OrgId,
    branch: Option<BranchId>,
    feature_key: &str,
) -> bool {
    let Ok(feature) = Feature::from_str(feature_key) else {
        return false;
    };
    let Ok(request) = spine_guard_request(
        principal,
        feature_key,
        org,
        branch,
        "workflow_run",
        "role_membership",
        WAITING_COMPLETION_DOMAIN,
    ) else {
        return false;
    };
    let entry = workflow_coexistence_entry(
        "workflow.waiting_task.role_membership",
        WAITING_COMPLETION_DOMAIN,
        feature,
        "workflow_run".to_owned(),
    );
    guard(&request, &entry).is_allowed()
}

/// The authority role keys this principal holds (security M3), for the personal
/// inbox's OPEN-task filter (`assignee_role_key = ANY(...)`).
pub(crate) fn held_authority_role_keys(
    principal: &Principal,
    org: console_kernel_core::OrgId,
    branch: Option<BranchId>,
) -> Vec<String> {
    WORKFLOW_AUTHORITY_ROLE_KEYS
        .iter()
        .filter(|role_key| match classify_role_key(role_key) {
            Some(RoleKeyKind::Authority(feature)) => {
                principal_holds_feature(principal, org, branch, feature)
            }
            _ => false,
        })
        .map(|role_key| (*role_key).to_owned())
        .collect()
}

/// Whether the principal may see the group (`role_key=`) inbox for `role_key`
/// (security M3): an authority key requires the matrix guard; an ownership key has
/// no org-wide queue (the owner sees it via `assignee=me`); an unknown key is
/// denied. A false result is a deny-by-omission (200 empty), never a 403.
fn holds_group_inbox_role(
    principal: &Principal,
    org: console_kernel_core::OrgId,
    branch: Option<BranchId>,
    role_key: &str,
) -> bool {
    match classify_role_key(role_key) {
        Some(RoleKeyKind::Authority(feature)) => {
            principal_holds_feature(principal, org, branch, feature)
        }
        Some(RoleKeyKind::Ownership) | None => false,
    }
}

/// Read-only visibility check for the inbox listings: a policy-bearing row is
/// visible only when the legacy contract allows the principal (deny-by-omission —
/// forbidden rows are absent, never returned as 403). Rows with no policy are
/// visible (their `role_key`/`assignee` filter is the access boundary).
fn task_visible(
    principal: &Principal,
    org: console_kernel_core::OrgId,
    branch: Option<BranchId>,
    item: &WaitingTaskListItem,
) -> bool {
    // Ownership-held rows (e.g. the initiator's own finalize task) reach this
    // filter ONLY when the adapter SQL already bound them to the caller
    // (initiated_by/claimed_by). Author finalization is owner-checked, not
    // policy-gated (authz: ApprovalFinalize), so a low-privilege owner must not
    // be stripped here by the policy layer (security M3).
    if item
        .assignee_role_key
        .as_deref()
        .and_then(classify_role_key)
        .is_some_and(|kind| matches!(kind, RoleKeyKind::Ownership))
    {
        return true;
    }
    let Some(policy) = item.required_policy.as_deref() else {
        return true;
    };
    let Some(feature_key) = guard_policy(policy) else {
        return false;
    };
    let Ok(feature) = Feature::from_str(&feature_key) else {
        return false;
    };
    let resource_type = item.object_type.as_deref().unwrap_or("workflow_run");
    let resource_id = item
        .object_id
        .map(|id| id.to_string())
        .unwrap_or_else(|| item.run_id.to_string());
    let Ok(request) = spine_guard_request(
        principal,
        &feature_key,
        org,
        branch,
        resource_type,
        &resource_id,
        WAITING_COMPLETION_DOMAIN,
    ) else {
        return false;
    };
    let entry = workflow_coexistence_entry(
        "workflow.waiting_task.list",
        WAITING_COMPLETION_DOMAIN,
        feature,
        resource_type.to_owned(),
    );
    guard(&request, &entry).is_allowed()
}

fn parse_task_statuses(raw: Option<&str>) -> Result<Vec<WaitingTaskStatus>, WorkflowStudioError> {
    let Some(raw) = raw else {
        return Ok(vec![WaitingTaskStatus::Open]);
    };
    raw.split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| WaitingTaskStatus::from_db_str(value).map_err(WorkflowStudioError::from))
        .collect()
}

fn parse_run_statuses(raw: Option<&str>) -> Result<Vec<RunStatus>, WorkflowStudioError> {
    let Some(raw) = raw else {
        return Ok(Vec::new());
    };
    raw.split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| RunStatus::from_db_str(value).map_err(WorkflowStudioError::from))
        .collect()
}

/// Load a definition's chosen version JSON, gating on ACTIVE status. Returns the
/// resolved `(version, definition)` for the run to bind to.
///
/// Security (four-eyes): a run may only start an APPROVED version. `active_version`
/// is lifecycle-trusted — it is set ONLY by publish/approve/rollback, all of which
/// require RoleManage + step-up, so the default (unpinned) path is always safe,
/// whatever the resolved version's own status is (e.g. `ROLLED_BACK`). A
/// caller-*pinned* `definition_version` that is NOT the current active version is
/// the untrusted path: it must be an already-approved historical version
/// (`status = 'PUBLISHED'`), never a staged/pending `DRAFT` — that pin is
/// rejected (422), so an initiator cannot execute a revision that never passed
/// the second-actor approval.
async fn resolve_start_definition(
    pool: &PgPool,
    org: console_kernel_core::OrgId,
    definition_id: Uuid,
    requested_version: Option<i32>,
) -> Result<(i32, Value), WorkflowStudioError> {
    let row = with_org_conn::<
        _,
        Option<(
            String,
            Option<Value>,
            Option<i32>,
            Option<String>,
            Option<i32>,
        )>,
        DbError,
    >(pool, org, move |tx| {
        Box::pin(async move {
            let row = sqlx::query(
                "SELECT d.status, v.definition, v.version, v.status AS version_status, \
                        d.active_version \
                     FROM workflow_definitions d \
                     LEFT JOIN workflow_definition_versions v \
                       ON v.definition_id = d.id AND v.org_id = d.org_id \
                      AND v.version = COALESCE($2, d.active_version) \
                     WHERE d.id = $1",
            )
            .bind(definition_id)
            .bind(requested_version)
            .fetch_optional(tx.as_mut())
            .await?;
            let Some(row) = row else {
                return Ok(None);
            };
            Ok(Some((
                row.try_get("status")?,
                row.try_get("definition")?,
                row.try_get("version")?,
                row.try_get("version_status")?,
                row.try_get("active_version")?,
            )))
        })
    })
    .await
    .map_err(WorkflowStudioError::from)?;

    let Some((status, definition, version, version_status, active_version)) = row else {
        return Err(WorkflowStudioError::from(KernelError::not_found(
            "workflow definition not found",
        )));
    };
    if status != "ACTIVE" {
        return Err(WorkflowStudioError::from(KernelError::conflict(
            "workflow definition is not active",
        )));
    }
    let (Some(definition), Some(version), Some(version_status)) =
        (definition, version, version_status)
    else {
        return Err(WorkflowStudioError::from(KernelError::conflict(
            "workflow definition has no published version to start",
        )));
    };
    // Four-eyes gate: a PINNED version that is NOT the current active version
    // must be an already-approved (PUBLISHED) historical version — never a
    // staged/pending DRAFT. The active version itself is always trusted (only
    // publish/approve/rollback set it), so this does not touch the default
    // (unpinned) or rollback-produced-active path.
    if Some(version) != active_version && version_status != "PUBLISHED" {
        return Err(WorkflowStudioError::validation(
            "workflow definition version is not an approved (published) version",
        ));
    }
    Ok((version, definition))
}

/// Load a run summary + its current OPEN/CLAIMED waiting task (the run's `next_task`).
async fn load_run_view(
    pool: &PgPool,
    org: console_kernel_core::OrgId,
    run_id: Uuid,
) -> Result<(RunSummaryResponse, Option<TaskSummaryResponse>), WorkflowStudioError> {
    with_org_conn::<_, (RunSummaryResponse, Option<TaskSummaryResponse>), DbError>(
        pool,
        org,
        move |tx| {
            Box::pin(async move {
                let run = sqlx::query(
                    "SELECT id, status, definition_id, definition_version, object_type, \
                            object_id, initiated_by, started_at \
                     FROM workflow_runs WHERE id = $1",
                )
                .bind(run_id)
                .fetch_one(tx.as_mut())
                .await?;
                let summary = RunSummaryResponse {
                    id: run.try_get("id")?,
                    status: run.try_get("status")?,
                    definition_id: run.try_get("definition_id")?,
                    definition_version: run.try_get("definition_version")?,
                    object_type: run.try_get("object_type")?,
                    object_id: run.try_get("object_id")?,
                    initiated_by: run.try_get("initiated_by")?,
                    started_at: run.try_get("started_at")?,
                };

                let task = sqlx::query(
                    "SELECT t.id AS task_id, t.run_id, t.waiting_key, t.title, \
                            t.assignee_role_key, t.required_policy, t.status, t.claimed_by, \
                            t.due_at, t.form_payload, r.object_type, r.object_id \
                     FROM workflow_waiting_tasks t \
                     JOIN workflow_runs r ON r.id = t.run_id AND r.org_id = t.org_id \
                     WHERE t.run_id = $1 AND t.status IN ('OPEN', 'CLAIMED') \
                     ORDER BY t.created_at DESC LIMIT 1",
                )
                .bind(run_id)
                .fetch_optional(tx.as_mut())
                .await?;
                let next_task = match task {
                    None => None,
                    Some(task) => Some(TaskSummaryResponse {
                        task_id: task.try_get("task_id")?,
                        run_id: task.try_get("run_id")?,
                        waiting_key: task.try_get("waiting_key")?,
                        title: task.try_get("title")?,
                        assignee_role_key: task.try_get("assignee_role_key")?,
                        required_policy: task.try_get("required_policy")?,
                        object_type: task.try_get("object_type")?,
                        object_id: task.try_get("object_id")?,
                        status: task.try_get("status")?,
                        claimed_by: task.try_get("claimed_by")?,
                        due_at: task.try_get("due_at")?,
                        form_payload: task.try_get("form_payload")?,
                    }),
                };
                Ok((summary, next_task))
            })
        },
    )
    .await
    .map_err(WorkflowStudioError::from)
}

async fn start_workflow_run(
    State(state): State<WorkflowStudioState>,
    Extension(principal): Extension<Principal>,
    Json(request): Json<StartWorkflowRunRequest>,
) -> Result<Json<StartWorkflowRunResponse>, WorkflowStudioError> {
    let idempotency_key = request.idempotency_key.trim();
    if idempotency_key.len() < 16 {
        return Err(WorkflowStudioError::validation(
            "idempotency_key must be at least 16 characters",
        ));
    }
    if request.object_type.is_some() != request.object_id.is_some() {
        return Err(WorkflowStudioError::validation(
            "object_type and object_id must be provided together",
        ));
    }
    // Bound the caller-supplied payloads (security M2): an unbounded input/context
    // blob is a memory/storage abuse vector. 64 KiB serialized is far above any
    // legitimate approval payload; over that is a 422.
    check_payload_size("input_payload", &request.input_payload)?;
    check_payload_size("context_payload", &request.context_payload)?;

    let org = principal.org_id;
    let store = PgWorkflowRuntimeStore::new(state.pool.clone());
    let (version, definition) = resolve_start_definition(
        &state.pool,
        org,
        request.definition_id,
        request.definition_version,
    )
    .await?;
    let graph = ExecGraph::parse(&definition).map_err(WorkflowStudioError::from)?;
    let entry = graph
        .entry_node_key()
        .map_err(WorkflowStudioError::from)?
        .to_owned();

    // Start authz: legacy-enforce + Cedar-shadow, gated on the definition's
    // per-start authority (security M2). A top-level `start_policy` (additive,
    // wf.exec.v1-compatible) constrains WHO may initiate this definition; when
    // absent it falls back to the entry node's policy. Approval templates
    // deliberately carry NEITHER (their entry gate is self-service) so 전자결재
    // 기안/상신 stays all-employee per DESIGN §4.8; operational pipelines (e.g. the
    // completion→approval→payroll template) set `start_policy` so a start is a
    // policy-gated 403 + shadow for non-privileged personas.
    let branch = WORKFLOW_SPINE_HAS_NO_BRANCH;
    let entry_policy = match graph.node_spec(&entry).map(|spec| &spec.kind) {
        Some(NodeKind::HumanTask {
            required_policy, ..
        }) => required_policy.clone(),
        _ => None,
    };
    let start_policy = definition
        .get("start_policy")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .or(entry_policy);
    let resource_type = request
        .object_type
        .clone()
        .unwrap_or_else(|| "workflow_run".to_owned());
    let resource_id = request
        .object_id
        .map(|id| id.to_string())
        .unwrap_or_else(|| request.definition_id.to_string());
    let start_shadow = guard_task_policy(
        &principal,
        org,
        branch,
        start_policy.as_deref(),
        &resource_type,
        &resource_id,
        "workflow.run.start",
        request.definition_id,
    )?;

    let run_id = Uuid::new_v4();
    let correlation_id = request
        .correlation_id
        .clone()
        .unwrap_or_else(|| format!("workflow-run:{run_id}"));
    if correlation_id.trim().len() < 8 {
        return Err(WorkflowStudioError::validation(
            "correlation_id must be at least 8 characters",
        ));
    }
    let audit = AuditContext {
        actor: Some(principal.user_id),
        trace: TraceContext::generate(),
        occurred_at: OffsetDateTime::now_utc(),
    };

    let started = start_run(
        &store,
        StartRunRequest {
            run_id,
            org_id: org,
            definition_id: request.definition_id,
            definition_version: version,
            trigger_type: request.trigger_type,
            object_type: request.object_type.clone(),
            object_id: request.object_id,
            idempotency_key: idempotency_key.to_owned(),
            correlation_id,
            trace_id: None,
            input_payload: request.input_payload.clone(),
            context_payload: request.context_payload.clone(),
            initiated_by: Some(principal.user_id),
            schedule_id: None,
        },
        &audit,
    )
    .await;

    let resolved_run_id = match started {
        Ok(id) => {
            // Fresh run: drive synchronously until the first WAITING task or terminal.
            let guard_audits: Vec<AuditEvent> = start_shadow.into_iter().collect();
            drive_from(
                &store,
                org,
                id,
                RunStatus::Running,
                &graph,
                &entry,
                guard_audits,
                &request.context_payload,
                &audit,
            )
            .await?;
            id
        }
        Err(err) if err.kind == ErrorKind::Conflict => {
            // Replay: same idempotency_key. Return the existing run if it matches;
            // a mismatch on the same key is a 409.
            match store
                .load_run_by_idempotency_key(org, idempotency_key.to_owned())
                .await?
            {
                Some(existing) => {
                    if existing.definition_id != request.definition_id
                        || existing.object_type != request.object_type
                        || existing.object_id != request.object_id
                    {
                        return Err(WorkflowStudioError::from(KernelError::conflict(
                            "idempotency_key already used for a different run",
                        )));
                    }
                    existing.id
                }
                None => return Err(WorkflowStudioError::from(err)),
            }
        }
        Err(err) => return Err(WorkflowStudioError::from(err)),
    };

    let (run, next_task) = load_run_view(&state.pool, org, resolved_run_id).await?;
    record_workflow_studio_request("run_start", "success");
    Ok(Json(StartWorkflowRunResponse { run, next_task }))
}

#[derive(Debug, Deserialize)]
struct BulkApprovalInboxQuery {
    #[serde(default)]
    cursor: Option<String>,
    #[serde(default = "default_bulk_approval_page_limit")]
    limit: i64,
}

const fn default_bulk_approval_page_limit() -> i64 {
    50
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
enum BulkApprovalIneligibilityReason {
    ClaimedByAnotherUser,
}

#[derive(Debug, Serialize)]
struct BulkApprovalCapability {
    decidable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<BulkApprovalIneligibilityReason>,
}

#[derive(Debug, Serialize)]
struct BulkApprovalTaskResponse {
    task_id: Uuid,
    run_id: Uuid,
    waiting_key: String,
    title: String,
    assignee_role_key: Option<String>,
    status: String,
    claimed_by: Option<Uuid>,
    #[serde(with = "time::serde::rfc3339::option")]
    due_at: Option<OffsetDateTime>,
    bulk_decision: BulkApprovalCapability,
}

#[derive(Debug, Serialize)]
struct BulkApprovalInboxResponse {
    items: Vec<BulkApprovalTaskResponse>,
    has_more: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    next_cursor: Option<String>,
}

fn parse_bulk_approval_cursor(
    raw: Option<&str>,
) -> Result<Option<(OffsetDateTime, String)>, WorkflowStudioError> {
    let Some(raw) = raw else {
        return Ok(None);
    };
    let Some((created_at, task_id)) = raw.split_once('|') else {
        return Err(WorkflowStudioError::validation(
            "invalid bulk approval cursor",
        ));
    };
    let created_at =
        OffsetDateTime::parse(created_at, &time::format_description::well_known::Rfc3339)
            .map_err(|_| WorkflowStudioError::validation("invalid bulk approval cursor"))?;
    let task_id = Uuid::parse_str(task_id)
        .map_err(|_| WorkflowStudioError::validation("invalid bulk approval cursor"))?;
    Ok(Some((created_at, format!("approval:{task_id}"))))
}

fn bulk_approval_cursor(item: &WaitingTaskListItem) -> Option<String> {
    item.created_at
        .format(&time::format_description::well_known::Rfc3339)
        .ok()
        .map(|created_at| format!("{created_at}|{}", item.task_id))
}

async fn list_bulk_approval_inbox(
    State(state): State<WorkflowStudioState>,
    Extension(principal): Extension<Principal>,
    axum::extract::Query(query): axum::extract::Query<BulkApprovalInboxQuery>,
) -> Result<Json<BulkApprovalInboxResponse>, WorkflowStudioError> {
    let limit = query.limit.clamp(1, 200) as usize;
    let org = principal.org_id;
    let branch = WORKFLOW_SPINE_HAS_NO_BRANCH;
    let store = PgWorkflowRuntimeStore::new(state.pool.clone());
    let filter = WaitingTaskListFilter {
        role_key: None,
        assignee_me: true,
        authority_role_keys: held_authority_role_keys(&principal, org, branch),
        statuses: vec![WaitingTaskStatus::Open, WaitingTaskStatus::Claimed],
    };
    let mut after = parse_bulk_approval_cursor(query.cursor.as_deref())?;
    let as_of = OffsetDateTime::now_utc();
    let mut items = Vec::with_capacity(limit);
    let mut next_cursor = None;
    // Keyset scan keeps a denied row out of both the page and its cardinality.
    // It advances until a full authorized page is found or the authoritative
    // source is exhausted; unlike offset paging, denied rows cannot create holes.
    loop {
        let (candidates, source_has_more) = store
            .list_waiting_tasks_action_page(
                org,
                principal.user_id,
                filter.clone(),
                as_of,
                after.clone(),
                200,
            )
            .await?;
        let last = candidates.last().and_then(bulk_approval_cursor);
        for item in &candidates {
            if item.required_policy.as_deref() != Some("approval_decide")
                || !task_visible(&principal, org, branch, item)
            {
                continue;
            }
            // Once the requested page is full, keep scanning until finding one
            // more authorized row. This makes `has_more` truthful without
            // exposing a page made solely of denied rows.
            if items.len() == limit {
                return Ok(Json(BulkApprovalInboxResponse {
                    items,
                    has_more: true,
                    next_cursor,
                }));
            }
            let capability = if item
                .claimed_by
                .is_some_and(|claimed_by| claimed_by != *principal.user_id.as_uuid())
            {
                BulkApprovalCapability {
                    decidable: false,
                    reason: Some(BulkApprovalIneligibilityReason::ClaimedByAnotherUser),
                }
            } else {
                BulkApprovalCapability {
                    decidable: true,
                    reason: None,
                }
            };
            let cursor = bulk_approval_cursor(item);
            items.push(BulkApprovalTaskResponse {
                task_id: item.task_id,
                run_id: item.run_id,
                waiting_key: item.waiting_key.clone(),
                title: item.title.clone(),
                assignee_role_key: item.assignee_role_key.clone(),
                status: item.status.as_db_str().to_owned(),
                claimed_by: item.claimed_by,
                due_at: item.due_at,
                bulk_decision: capability,
            });
            next_cursor = cursor;
        }
        if !source_has_more {
            return Ok(Json(BulkApprovalInboxResponse {
                items,
                has_more: false,
                next_cursor: None,
            }));
        }
        let Some(next) = last else {
            return Err(WorkflowStudioError::from(KernelError::internal(
                "workflow task cursor did not advance",
            )));
        };
        after = parse_bulk_approval_cursor(Some(&next))?;
    }
}

async fn list_workflow_tasks(
    State(state): State<WorkflowStudioState>,
    Extension(principal): Extension<Principal>,
    axum::extract::Query(query): axum::extract::Query<TaskListQuery>,
) -> Result<Json<WorkflowTaskListResponse>, WorkflowStudioError> {
    let assignee_me = query.assignee.as_deref() == Some("me");
    if query.role_key.is_none() && !assignee_me {
        return Err(WorkflowStudioError::validation(
            "workflow-tasks requires role_key or assignee=me",
        ));
    }
    let statuses = parse_task_statuses(query.status.as_deref())?;
    let org = principal.org_id;
    let branch = WORKFLOW_SPINE_HAS_NO_BRANCH;

    // Group inbox (security M3): a `role_key=` query returns rows only when the
    // caller holds that role. Deny-by-omission — a caller who does not is handed
    // an empty list (200), never a 403 (never leaks the queue's existence).
    if let Some(role_key) = query.role_key.as_deref()
        && !holds_group_inbox_role(&principal, org, branch, role_key)
    {
        record_workflow_studio_request("task_list", "success");
        return Ok(Json(WorkflowTaskListResponse { items: vec![] }));
    }

    let store = PgWorkflowRuntimeStore::new(state.pool.clone());
    let items = store
        .list_waiting_tasks(
            org,
            principal.user_id,
            WaitingTaskListFilter {
                role_key: query.role_key.clone(),
                assignee_me,
                // Personal inbox OPEN-task gate (security M3): the authority role
                // keys this caller holds. Ownership-keyed OPEN tasks bind to the
                // run initiator in SQL instead.
                authority_role_keys: held_authority_role_keys(&principal, org, branch),
                statuses,
            },
        )
        .await?;
    let items = items
        .into_iter()
        .filter(|item| task_visible(&principal, org, branch, item))
        .map(TaskSummaryResponse::from)
        .collect();
    record_workflow_studio_request("task_list", "success");
    Ok(Json(WorkflowTaskListResponse { items }))
}

/// The caller's actionable approval/workflow tasks for the unified action inbox
/// (`GET /api/v1/me/action-inbox`): their personal-inbox OPEN + CLAIMED waiting
/// tasks, scoped and visibility-filtered EXACTLY as `GET /api/v1/workflow-tasks?
/// assignee=me` — same `list_waiting_tasks` predicate + `task_visible` gate, so
/// the aggregate can never widen visibility beyond the source list endpoint.
pub(crate) async fn my_action_inbox_tasks_page(
    pool: &PgPool,
    principal: &Principal,
    as_of: OffsetDateTime,
    after: Option<(OffsetDateTime, String)>,
    limit: usize,
) -> Result<(Vec<WaitingTaskListItem>, bool), KernelError> {
    let org = principal.org_id;
    let branch = WORKFLOW_SPINE_HAS_NO_BRANCH;
    let store = PgWorkflowRuntimeStore::new(pool.clone());
    let filter = WaitingTaskListFilter {
        role_key: None,
        assignee_me: true,
        authority_role_keys: held_authority_role_keys(principal, org, branch),
        statuses: vec![WaitingTaskStatus::Open, WaitingTaskStatus::Claimed],
    };
    let mut visible = Vec::new();
    let mut cursor = after;
    let mut scanned = 0usize;
    loop {
        let (page, source_has_more) = store
            .list_waiting_tasks_action_page(
                org,
                principal.user_id,
                filter.clone(),
                as_of,
                cursor.clone(),
                200,
            )
            .await?;
        scanned += page.len();
        let next_cursor = page
            .last()
            .map(|item| (item.created_at, format!("approval:{}", item.task_id)));
        visible.extend(
            page.into_iter()
                .filter(|item| task_visible(principal, org, branch, item)),
        );
        if visible.len() > limit {
            visible.truncate(limit + 1);
            return Ok((visible, true));
        }
        if !source_has_more {
            return Ok((visible, false));
        }
        if scanned >= 1_000 {
            return Err(KernelError::internal(
                "workflow action-inbox scan budget exhausted",
            ));
        }
        match next_cursor {
            Some(next) if Some(&next) != cursor.as_ref() => cursor = Some(next),
            _ => {
                return Err(KernelError::internal(
                    "workflow task cursor did not advance",
                ));
            }
        }
    }
}

/// Bounded exact-count attempt for action-inbox total semantics. `exact=false`
/// is explicit when the defensive scan budget is reached; callers must never
/// present the returned lower bound as an exact queue size.
pub(crate) async fn my_action_inbox_task_count(
    pool: &PgPool,
    principal: &Principal,
    as_of: OffsetDateTime,
) -> Result<(usize, bool), KernelError> {
    let org = principal.org_id;
    let branch = WORKFLOW_SPINE_HAS_NO_BRANCH;
    let store = PgWorkflowRuntimeStore::new(pool.clone());
    let filter = WaitingTaskListFilter {
        role_key: None,
        assignee_me: true,
        authority_role_keys: held_authority_role_keys(principal, org, branch),
        statuses: vec![WaitingTaskStatus::Open, WaitingTaskStatus::Claimed],
    };
    let mut count = 0usize;
    let mut scanned = 0usize;
    let mut cursor = None;
    loop {
        let (page, has_more) = store
            .list_waiting_tasks_action_page(
                org,
                principal.user_id,
                filter.clone(),
                as_of,
                cursor.clone(),
                200,
            )
            .await?;
        scanned += page.len();
        cursor = page
            .last()
            .map(|item| (item.created_at, format!("approval:{}", item.task_id)));
        count += page
            .iter()
            .filter(|item| task_visible(principal, org, branch, item))
            .count();
        if !has_more {
            return Ok((count, true));
        }
        if scanned >= 1_000 {
            return Ok((count, false));
        }
        if cursor.is_none() {
            return Err(KernelError::internal(
                "workflow task cursor did not advance",
            ));
        }
    }
}

async fn list_my_workflow_runs(
    State(state): State<WorkflowStudioState>,
    Extension(principal): Extension<Principal>,
    axum::extract::Query(query): axum::extract::Query<RunListQuery>,
) -> Result<Json<RunListResponse>, WorkflowStudioError> {
    let statuses = parse_run_statuses(query.status.as_deref())?;
    let org = principal.org_id;
    let store = PgWorkflowRuntimeStore::new(state.pool.clone());
    let items = store
        .list_runs_for_initiator(
            org,
            principal.user_id,
            RunListFilter {
                statuses,
                object_type: query.object_type.clone(),
                // Empty/whitespace-only q is treated as absent (returns all rows).
                q: query
                    .q
                    .as_deref()
                    .map(str::trim)
                    .filter(|q| !q.is_empty())
                    .map(ToOwned::to_owned),
            },
        )
        .await?;
    record_workflow_studio_request("run_mine", "success");
    Ok(Json(RunListResponse {
        items: items.into_iter().map(RunListItemResponse::from).collect(),
    }))
}

#[derive(Debug, Deserialize)]
struct AdminRunListQuery {
    #[serde(default)]
    status: Option<String>,
    /// Keyset cursor: the last `run_id` of the previous page.
    #[serde(default)]
    before: Option<Uuid>,
    #[serde(default)]
    limit: Option<i64>,
}

#[derive(Debug, Serialize)]
struct AdminRunListResponse {
    items: Vec<RunListItemResponse>,
    /// Cursor to pass as `?before=` for the next page; absent on the last page.
    #[serde(skip_serializing_if = "Option::is_none")]
    next_cursor: Option<Uuid>,
}

/// `GET /api/v1/workflow-runs?status=...&before=...&limit=...` — org-wide admin
/// run list (workflow-manage). Filterable by status (incl. `FAILED`/`DEAD_LETTERED`
/// for dead-letter visibility) and keyset-paginated over `(updated_at, id)`.
async fn list_workflow_runs_admin(
    State(state): State<WorkflowStudioState>,
    Extension(principal): Extension<Principal>,
    axum::extract::Query(query): axum::extract::Query<AdminRunListQuery>,
) -> Result<Json<AdminRunListResponse>, WorkflowStudioError> {
    authorize_workflow_manage(&principal)?;
    let statuses = parse_run_statuses(query.status.as_deref())?;
    let limit = query.limit.unwrap_or(50).clamp(1, 200);
    let org = principal.org_id;
    let store = PgWorkflowRuntimeStore::new(state.pool.clone());
    let items = store
        .list_runs_admin(
            org,
            AdminRunListFilter {
                statuses,
                before: query.before,
                limit,
            },
        )
        .await?;
    // A full page implies more rows may follow: hand back the last row's id as the
    // next cursor. A short page is the end of the list.
    let next_cursor = (items.len() as i64 == limit)
        .then(|| items.last().map(|item| item.run_id))
        .flatten();
    record_workflow_studio_request("run_admin_list", "success");
    Ok(Json(AdminRunListResponse {
        items: items.into_iter().map(RunListItemResponse::from).collect(),
        next_cursor,
    }))
}

#[derive(Debug, Serialize)]
struct RunDetailRun {
    id: Uuid,
    status: String,
    definition_id: Uuid,
    definition_version: i32,
    trigger_type: String,
    object_type: Option<String>,
    object_id: Option<Uuid>,
    initiated_by: Option<Uuid>,
    /// Failure reason for FAILED / DEAD_LETTERED runs (dead-letter visibility).
    #[serde(skip_serializing_if = "Option::is_none")]
    error_payload: Option<Value>,
    #[serde(with = "time::serde::rfc3339")]
    started_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    updated_at: OffsetDateTime,
    #[serde(
        with = "time::serde::rfc3339::option",
        skip_serializing_if = "Option::is_none"
    )]
    completed_at: Option<OffsetDateTime>,
    #[serde(
        with = "time::serde::rfc3339::option",
        skip_serializing_if = "Option::is_none"
    )]
    failed_at: Option<OffsetDateTime>,
}

/// One executed node in a run's timeline (append-only `workflow_node_runs`),
/// enriched with the deciding actor + outcome from its linked waiting task.
#[derive(Debug, Serialize)]
struct RunTimelineStep {
    node_key: String,
    /// The node kind (`object_gate` / `human_task` / `job` / ...).
    node_type: String,
    status: String,
    attempt: i32,
    #[serde(
        with = "time::serde::rfc3339::option",
        skip_serializing_if = "Option::is_none"
    )]
    started_at: Option<OffsetDateTime>,
    #[serde(
        with = "time::serde::rfc3339::option",
        skip_serializing_if = "Option::is_none"
    )]
    finished_at: Option<OffsetDateTime>,
    /// The user who decided this node (decision nodes only).
    #[serde(skip_serializing_if = "Option::is_none")]
    actor: Option<Uuid>,
    /// The decision payload recorded on the node's waiting task, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    outcome: Option<Value>,
    /// The node's error payload for a FAILED node step.
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<Value>,
}

#[derive(Debug, Serialize)]
struct RunDetailResponse {
    run: RunDetailRun,
    /// Current OPEN/CLAIMED waiting task(s).
    waiting_tasks: Vec<TaskSummaryResponse>,
    /// Node-step timeline, oldest first.
    timeline: Vec<RunTimelineStep>,
}

/// `GET /api/v1/workflow-runs/{run_id}` — read-only run detail: head, current
/// waiting task(s), and the node-step timeline. Visibility mirrors the approval
/// inbox exactly (`resolve_approval_run`): the initiator, a claimer, or a holder
/// of a routed authority role — plus workflow-manage admins org-wide. Everyone
/// else gets 404 (deny-by-omission), never a leak of another branch's run.
async fn get_workflow_run(
    State(state): State<WorkflowStudioState>,
    Extension(principal): Extension<Principal>,
    Path(run_id): Path<Uuid>,
) -> Result<Json<RunDetailResponse>, WorkflowStudioError> {
    let org = principal.org_id;
    let is_admin = authorize_workflow_manage(&principal).is_ok();
    let caller = *principal.user_id.as_uuid();
    let held_role_keys = held_authority_role_keys(&principal, org, WORKFLOW_SPINE_HAS_NO_BRANCH);

    let detail = with_org_conn::<_, Option<RunDetailResponse>, WorkflowStudioError>(
        &state.pool,
        org,
        move |tx| {
            Box::pin(async move {
                let run = sqlx::query(
                    "SELECT r.id, r.status, r.definition_id, r.definition_version, \
                            r.trigger_type, r.object_type, r.object_id, r.initiated_by, \
                            r.error_payload, r.started_at, r.updated_at, \
                            r.completed_at, r.failed_at \
                     FROM workflow_runs r \
                     WHERE r.id = $1 \
                       AND ($2 \
                            OR r.initiated_by = $3 \
                            OR EXISTS ( \
                                SELECT 1 FROM workflow_waiting_tasks t \
                                WHERE t.run_id = r.id AND t.org_id = r.org_id \
                                  AND t.status IN ('OPEN', 'CLAIMED') \
                                  AND (t.claimed_by = $3 OR t.assignee_role_key = ANY($4))))",
                )
                .bind(run_id)
                .bind(is_admin)
                .bind(caller)
                .bind(&held_role_keys)
                .fetch_optional(tx.as_mut())
                .await?;
                let Some(run) = run else {
                    return Ok(None);
                };
                let run = RunDetailRun {
                    id: run.try_get("id")?,
                    status: run.try_get("status")?,
                    definition_id: run.try_get("definition_id")?,
                    definition_version: run.try_get("definition_version")?,
                    trigger_type: run.try_get("trigger_type")?,
                    object_type: run.try_get("object_type")?,
                    object_id: run.try_get("object_id")?,
                    initiated_by: run.try_get("initiated_by")?,
                    error_payload: run.try_get("error_payload")?,
                    started_at: run.try_get("started_at")?,
                    updated_at: run.try_get("updated_at")?,
                    completed_at: run.try_get("completed_at")?,
                    failed_at: run.try_get("failed_at")?,
                };

                let task_rows = sqlx::query(
                    "SELECT t.id AS task_id, t.run_id, t.waiting_key, t.title, \
                            t.assignee_role_key, t.required_policy, t.status, t.claimed_by, \
                            t.due_at, t.form_payload, r.object_type, r.object_id \
                     FROM workflow_waiting_tasks t \
                     JOIN workflow_runs r ON r.id = t.run_id AND r.org_id = t.org_id \
                     WHERE t.run_id = $1 AND t.status IN ('OPEN', 'CLAIMED') \
                     ORDER BY t.created_at ASC",
                )
                .bind(run_id)
                .fetch_all(tx.as_mut())
                .await?;
                let waiting_tasks = task_rows
                    .iter()
                    .map(|task| {
                        Ok(TaskSummaryResponse {
                            task_id: task.try_get("task_id")?,
                            run_id: task.try_get("run_id")?,
                            waiting_key: task.try_get("waiting_key")?,
                            title: task.try_get("title")?,
                            assignee_role_key: task.try_get("assignee_role_key")?,
                            required_policy: task.try_get("required_policy")?,
                            object_type: task.try_get("object_type")?,
                            object_id: task.try_get("object_id")?,
                            status: task.try_get("status")?,
                            claimed_by: task.try_get("claimed_by")?,
                            due_at: task.try_get("due_at")?,
                            form_payload: task.try_get("form_payload")?,
                        })
                    })
                    .collect::<Result<Vec<_>, sqlx::Error>>()?;

                let step_rows = sqlx::query(
                    "SELECT nr.node_key, nr.node_type, nr.status, nr.attempt, \
                            nr.started_at, nr.finished_at, nr.error_payload, \
                            t.completed_by, t.decision_payload \
                     FROM workflow_node_runs nr \
                     LEFT JOIN workflow_waiting_tasks t \
                            ON t.node_run_id = nr.id AND t.org_id = nr.org_id \
                     WHERE nr.run_id = $1 \
                     ORDER BY COALESCE(nr.started_at, nr.updated_at) ASC, nr.node_key ASC, nr.attempt ASC",
                )
                .bind(run_id)
                .fetch_all(tx.as_mut())
                .await?;
                let timeline = step_rows
                    .iter()
                    .map(|step| {
                        Ok(RunTimelineStep {
                            node_key: step.try_get("node_key")?,
                            node_type: step.try_get("node_type")?,
                            status: step.try_get("status")?,
                            attempt: step.try_get("attempt")?,
                            started_at: step.try_get("started_at")?,
                            finished_at: step.try_get("finished_at")?,
                            actor: step.try_get("completed_by")?,
                            outcome: step.try_get("decision_payload")?,
                            error: step.try_get("error_payload")?,
                        })
                    })
                    .collect::<Result<Vec<_>, sqlx::Error>>()?;

                Ok(Some(RunDetailResponse {
                    run,
                    waiting_tasks,
                    timeline,
                }))
            })
        },
    )
    .await?;

    match detail {
        Some(detail) => {
            record_workflow_studio_request("run_detail", "success");
            Ok(Json(detail))
        }
        None => Err(WorkflowStudioError::from(KernelError::not_found(
            "workflow run not found",
        ))),
    }
}

async fn claim_workflow_task(
    State(state): State<WorkflowStudioState>,
    Extension(principal): Extension<Principal>,
    Path(task_id): Path<Uuid>,
    Json(request): Json<ClaimWorkflowTaskRequest>,
) -> Result<Json<ClaimTaskResponse>, WorkflowStudioError> {
    if request.idempotency_key.trim().len() < 16 {
        return Err(WorkflowStudioError::validation(
            "idempotency_key must be at least 16 characters",
        ));
    }
    let org = principal.org_id;
    let store = PgWorkflowRuntimeStore::new(state.pool.clone());
    let context = store
        .load_finalize_waiting_task(org, task_id)
        .await?
        .ok_or_else(|| KernelError::not_found("workflow task not found"))?;

    // Defense in depth (security H1): a legacy task row that predates the
    // authoring-time `required_policy` requirement carries no authorization
    // boundary. Refuse the mutation rather than let any org member claim it.
    require_task_authorization_boundary(context.required_policy.as_deref())?;

    let branch = WORKFLOW_SPINE_HAS_NO_BRANCH;
    let resource_type = context.object_type.as_deref().unwrap_or("workflow_run");
    let resource_id = context
        .object_id
        .map(|id| id.to_string())
        .unwrap_or_else(|| context.run_id.to_string());
    let shadow = guard_task_policy(
        &principal,
        org,
        branch,
        context.required_policy.as_deref(),
        resource_type,
        &resource_id,
        "workflow.waiting_task.claim",
        task_id,
    )?;
    observe_task_decide_parity(
        &state.pool,
        &principal,
        org,
        branch,
        context.required_policy.as_deref(),
        resource_type,
        &resource_id,
    )
    .await;

    let claimed = store
        .claim_waiting_task(
            org,
            ClaimWaitingTaskCommand {
                task_id,
                actor: principal.user_id,
                transition_audits: shadow.into_iter().collect(),
            },
        )
        .await?;
    record_workflow_studio_request("task_claim", "success");
    Ok(Json(ClaimTaskResponse {
        task: ClaimedTaskResponse {
            task_id: claimed.task_id,
            run_id: claimed.run_id,
            status: claimed.status.as_db_str().to_owned(),
            claimed_by: claimed.claimed_by,
            claimed_at: claimed.claimed_at,
        },
    }))
}

async fn decide_workflow_task(
    State(state): State<WorkflowStudioState>,
    Extension(principal): Extension<Principal>,
    Path(task_id): Path<Uuid>,
    Json(request): Json<DecideWorkflowTaskRequest>,
) -> Result<Json<DecideTaskResponse>, WorkflowStudioError> {
    let idempotency_key = request.idempotency_key.trim();
    if idempotency_key.len() < 16 {
        return Err(WorkflowStudioError::validation(
            "idempotency_key must be at least 16 characters",
        ));
    }
    // Bound the free-text comment (security L5), mirroring the DB-bounded reason
    // pattern of migration 0096: an over-long comment is a 422, not a silent DB
    // truncation or an unbounded write.
    if request
        .comment
        .as_deref()
        .is_some_and(|comment| comment.chars().count() > 4000)
    {
        return Err(WorkflowStudioError::validation(
            "comment must be at most 4000 characters",
        ));
    }
    let decision = TaskDecision::from(request.decision);
    let comment = request
        .comment
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    if matches!(decision, TaskDecision::Reject | TaskDecision::Return) && comment.is_none() {
        return Err(WorkflowStudioError::validation(
            "reject and return require a non-empty comment",
        ));
    }

    let org = principal.org_id;
    let store = PgWorkflowRuntimeStore::new(state.pool.clone());
    let context = store
        .load_finalize_waiting_task(org, task_id)
        .await?
        .ok_or_else(|| KernelError::not_found("workflow task not found"))?;

    // Defense in depth (security H1): a policy-less legacy task row has no
    // authorization boundary — refuse to decide it (403) rather than let any
    // org member push the run forward.
    require_task_authorization_boundary(context.required_policy.as_deref())?;

    let branch = WORKFLOW_SPINE_HAS_NO_BRANCH;
    let resource_type = context.object_type.as_deref().unwrap_or("workflow_run");
    let resource_id = context
        .object_id
        .map(|id| id.to_string())
        .unwrap_or_else(|| context.run_id.to_string());
    let shadow = guard_task_policy(
        &principal,
        org,
        branch,
        context.required_policy.as_deref(),
        resource_type,
        &resource_id,
        "workflow.waiting_task.decide",
        task_id,
    )?;
    observe_task_decide_parity(
        &state.pool,
        &principal,
        org,
        branch,
        context.required_policy.as_deref(),
        resource_type,
        &resource_id,
    )
    .await;

    let decided = store
        .decide_waiting_task(
            org,
            DecideWaitingTaskCommand {
                task_id,
                actor: principal.user_id,
                decision,
                comment: comment.map(ToOwned::to_owned),
                idempotency_key: idempotency_key.to_owned(),
                transition_audits: shadow.into_iter().collect(),
            },
        )
        .await?;
    record_workflow_studio_request("task_decide", "success");
    Ok(Json(DecideTaskResponse {
        task: DecidedTaskResponse {
            task_id: decided.task_id,
            run_id: decided.run_id,
            status: decided.status.as_db_str().to_owned(),
            decision_payload: decided.decision_payload,
        },
        run: FinalizedRunResponse {
            id: decided.run_id,
            status: decided.run_status.as_db_str().to_owned(),
        },
        next_task: decided.next_task.map(TaskSummaryResponse::from),
    }))
}

async fn list_definitions(
    State(state): State<WorkflowStudioState>,
    Extension(principal): Extension<Principal>,
) -> Result<Json<WorkflowDefinitionListResponse>, WorkflowStudioError> {
    authorize_workflow_manage(&principal)?;
    let org = principal.org_id;
    let items = with_org_conn::<_, _, WorkflowStudioError>(&state.pool, org, |tx| {
        Box::pin(async move {
            let rows = sqlx::query(
                r#"
                SELECT
                    d.id,
                    d.workflow_key,
                    d.display_name,
                    d.object_type,
                    d.status,
                    d.latest_version,
                    d.active_version,
                    d.pending_version,
                    d.pending_staged_by,
                    d.created_at,
                    d.updated_at,
                    COALESCE(v.definition, '{}'::jsonb) AS definition,
                    COALESCE(v.approval_line, '[]'::jsonb) AS approval_line,
                    COALESCE(v.payment_line, '[]'::jsonb) AS payment_line,
                    COALESCE(v.notification_rules, '[]'::jsonb) AS notification_rules,
                    COALESCE(v.action_allowlist, '[]'::jsonb) AS action_allowlist,
                    COALESCE(v.required_approval_line, false) AS required_approval_line,
                    COALESCE(v.required_payment_line, false) AS required_payment_line
                FROM workflow_definitions d
                LEFT JOIN workflow_definition_versions v
                    ON v.definition_id = d.id
                   AND v.org_id = d.org_id
                   AND v.version = d.latest_version
                WHERE d.status <> 'RETIRED'
                ORDER BY d.updated_at DESC, d.display_name ASC
                "#,
            )
            .fetch_all(tx.as_mut())
            .await?;
            rows.into_iter().map(response_from_row).collect()
        })
    })
    .await?;
    record_workflow_studio_request("definitions", "success");
    Ok(Json(WorkflowDefinitionListResponse { items }))
}

#[derive(Debug, Serialize)]
struct DefinitionsByObjectKindResponse {
    kind: String,
    /// Definitions whose primary object_type is this kind OR whose declared
    /// object_kinds chain touches it.
    definitions: Vec<WorkflowDefinitionResponse>,
    /// Enabled/disabled trigger bindings scoped to this kind.
    bindings: Vec<TriggerBindingResponse>,
}

/// The explore screen's "작용 자동화" panel source: every automation rule that
/// touches a given object kind — the definitions whose nodes act on it (by
/// primary object_type or declared object_kinds chain) plus the trigger
/// bindings scoped to it.
async fn list_definitions_by_object_kind(
    State(state): State<WorkflowStudioState>,
    Extension(principal): Extension<Principal>,
    Path(kind): Path<String>,
) -> Result<Json<DefinitionsByObjectKindResponse>, WorkflowStudioError> {
    authorize_workflow_manage(&principal)?;
    if !is_object_kind_slug(&kind) {
        return Err(WorkflowStudioError::validation(
            "object kind must be a valid kind slug",
        ));
    }
    let org = principal.org_id;
    let lookup_kind = kind.clone();
    let (definitions, bindings) =
        with_org_conn::<_, _, WorkflowStudioError>(&state.pool, org, move |tx| {
            Box::pin(async move {
                let def_rows = sqlx::query(
                    r#"
                    SELECT
                        d.id, d.workflow_key, d.display_name, d.object_type, d.status,
                        d.latest_version, d.active_version, d.pending_version,
                        d.pending_staged_by, d.created_at, d.updated_at,
                        COALESCE(v.definition, '{}'::jsonb) AS definition,
                        COALESCE(v.approval_line, '[]'::jsonb) AS approval_line,
                        COALESCE(v.payment_line, '[]'::jsonb) AS payment_line,
                        COALESCE(v.notification_rules, '[]'::jsonb) AS notification_rules,
                        COALESCE(v.action_allowlist, '[]'::jsonb) AS action_allowlist,
                        COALESCE(v.required_approval_line, false) AS required_approval_line,
                        COALESCE(v.required_payment_line, false) AS required_payment_line
                    FROM workflow_definitions d
                    LEFT JOIN workflow_definition_versions v
                        ON v.definition_id = d.id
                       AND v.org_id = d.org_id
                       AND v.version = d.latest_version
                    WHERE d.status <> 'RETIRED'
                      AND (
                          d.object_type = $1
                          OR jsonb_exists(v.definition -> 'object_kinds', $1)
                      )
                    ORDER BY d.updated_at DESC, d.display_name ASC
                    "#,
                )
                .bind(&lookup_kind)
                .fetch_all(tx.as_mut())
                .await?;
                let definitions: Vec<WorkflowDefinitionResponse> = def_rows
                    .into_iter()
                    .map(response_from_row)
                    .collect::<Result<_, _>>()?;

                let binding_rows = sqlx::query(
                    "SELECT id, definition_id, trigger_type, event_key, subject_kind, \
                            enabled, created_at, updated_at \
                     FROM workflow_trigger_bindings \
                     WHERE subject_kind = $1 \
                     ORDER BY created_at DESC",
                )
                .bind(&lookup_kind)
                .fetch_all(tx.as_mut())
                .await?;
                let bindings: Vec<TriggerBindingResponse> = binding_rows
                    .iter()
                    .map(|row| trigger_binding_from_row(row).map_err(WorkflowStudioError::from))
                    .collect::<Result<_, _>>()?;
                Ok((definitions, bindings))
            })
        })
        .await?;
    record_workflow_studio_request("definitions_by_object_kind", "success");
    Ok(Json(DefinitionsByObjectKindResponse {
        kind,
        definitions,
        bindings,
    }))
}

#[derive(Debug, Serialize)]
struct SubmittableDefinitionListResponse {
    items: Vec<SubmittableDefinitionResponse>,
}

/// A workflow definition the caller may START from the 기안 template gallery.
/// Carries only the metadata definitions actually hold — no invented
/// icon/desc/tone (those are frontend presentation keyed off `workflow_key` /
/// `object_type`). `active_version` is the version a `POST /workflow-runs` start
/// binds to.
#[derive(Debug, Serialize)]
struct SubmittableDefinitionResponse {
    id: Uuid,
    workflow_key: String,
    display_name: String,
    object_type: String,
    active_version: i32,
    required_approval_line: bool,
    required_payment_line: bool,
}

/// Raw ACTIVE-definition row + its active-version graph, before the start-authority
/// filter is applied in Rust.
struct SubmittableCandidate {
    id: Uuid,
    workflow_key: String,
    display_name: String,
    object_type: String,
    active_version: i32,
    definition: Value,
    required_approval_line: bool,
    required_payment_line: bool,
}

impl From<SubmittableCandidate> for SubmittableDefinitionResponse {
    fn from(c: SubmittableCandidate) -> Self {
        Self {
            id: c.id,
            workflow_key: c.workflow_key,
            display_name: c.display_name,
            object_type: c.object_type,
            active_version: c.active_version,
            required_approval_line: c.required_approval_line,
            required_payment_line: c.required_payment_line,
        }
    }
}

/// `GET /api/v1/workflow-studio/submittable-definitions` — the all-employee 기안
/// template gallery source. Unlike every other workflow-studio catalog endpoint
/// (which is `authorize_workflow_manage` admin-only), this is member-gated
/// (Feature::Login), because starting an approval is self-service per DESIGN §4.8.
///
/// Deny-by-omission: a definition is listed ONLY when it is ACTIVE AND the caller
/// could actually START it — the identical start authority `start_workflow_run`
/// enforces (top-level `start_policy`, else the entry node's `required_policy`;
/// absent = self-service). The catalog must never advertise a definition the
/// caller would get a 403 starting (no affordance-then-403).
async fn list_submittable_definitions(
    State(state): State<WorkflowStudioState>,
    Extension(principal): Extension<Principal>,
) -> Result<Json<SubmittableDefinitionListResponse>, WorkflowStudioError> {
    authorize_workflow_member(&principal)?;
    let org = principal.org_id;
    let branch = WORKFLOW_SPINE_HAS_NO_BRANCH;
    let candidates = with_org_conn::<_, Vec<SubmittableCandidate>, WorkflowStudioError>(
        &state.pool,
        org,
        |tx| {
            Box::pin(async move {
                let rows = sqlx::query(
                    r#"
                    SELECT
                        d.id,
                        d.workflow_key,
                        d.display_name,
                        d.object_type,
                        d.active_version,
                        v.definition,
                        COALESCE(v.required_approval_line, false) AS required_approval_line,
                        COALESCE(v.required_payment_line, false) AS required_payment_line
                    FROM workflow_definitions d
                    JOIN workflow_definition_versions v
                        ON v.definition_id = d.id
                       AND v.org_id = d.org_id
                       AND v.version = d.active_version
                    WHERE d.status = 'ACTIVE' AND d.active_version IS NOT NULL
                    ORDER BY d.display_name ASC, d.id ASC
                    "#,
                )
                .fetch_all(tx.as_mut())
                .await?;
                rows.into_iter()
                    .map(|row| {
                        Ok(SubmittableCandidate {
                            id: row.try_get("id")?,
                            workflow_key: row.try_get("workflow_key")?,
                            display_name: row.try_get("display_name")?,
                            object_type: row.try_get("object_type")?,
                            active_version: row.try_get("active_version")?,
                            definition: row.try_get("definition")?,
                            required_approval_line: row.try_get("required_approval_line")?,
                            required_payment_line: row.try_get("required_payment_line")?,
                        })
                    })
                    .collect::<Result<Vec<_>, WorkflowStudioError>>()
            })
        },
    )
    .await?;

    let items = candidates
        .into_iter()
        .filter(|c| caller_can_start(&principal, org, branch, c.id, &c.definition))
        .map(SubmittableDefinitionResponse::from)
        .collect();
    record_workflow_studio_request("submittable_definitions", "success");
    Ok(Json(SubmittableDefinitionListResponse { items }))
}

/// Read-only mirror of `start_workflow_run`'s start authority (no shadow-audit
/// side effect — this is a catalog read, not a start). Returns whether the
/// principal could initiate this definition:
/// - a definition whose graph fails to parse / has no entry is NOT startable
///   (a start would 422) → omit;
/// - `start_policy` = top-level `start_policy`, else the entry node's
///   `required_policy`; absent = self-service (any member) → include;
/// - otherwise the SAME legacy guard the start path enforces
///   (`guard_policy` → `build_guard_request` → `workflow_coexistence_entry` →
///   `guard`); denied → omit.
///
/// The guard resource mirrors the start path's no-object fallback
/// (`workflow_run` / definition id): start policies gate a capability, not a
/// per-object grant, so this is the same decision a target-less start makes.
fn caller_can_start(
    principal: &Principal,
    org: console_kernel_core::OrgId,
    branch: Option<BranchId>,
    definition_id: Uuid,
    definition: &Value,
) -> bool {
    let Ok(graph) = ExecGraph::parse(definition) else {
        return false;
    };
    let Ok(entry) = graph.entry_node_key() else {
        return false;
    };
    let entry_policy = match graph.node_spec(entry).map(|spec| &spec.kind) {
        Some(NodeKind::HumanTask {
            required_policy, ..
        }) => required_policy.clone(),
        _ => None,
    };
    let start_policy = definition
        .get("start_policy")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .or(entry_policy);
    let Some(policy) = start_policy else {
        return true; // self-service: all-employee 기안/상신 (DESIGN §4.8)
    };
    let Some(feature_key) = guard_policy(&policy) else {
        return false;
    };
    let Ok(feature) = Feature::from_str(&feature_key) else {
        return false;
    };
    let Ok(request) = spine_guard_request(
        principal,
        &feature_key,
        org,
        branch,
        "workflow_run",
        &definition_id.to_string(),
        WAITING_COMPLETION_DOMAIN,
    ) else {
        return false;
    };
    let entry = workflow_coexistence_entry(
        "workflow.run.start",
        WAITING_COMPLETION_DOMAIN,
        feature,
        "workflow_run".to_owned(),
    );
    guard(&request, &entry).is_allowed()
}

/// All-employee gate for the submittable-templates catalog (Feature::Login),
/// mirroring `objects::authorize_object_member` — every authenticated tenant
/// member may browse the 기안 gallery (the per-row start-authority filter is what
/// scopes what they actually see).
fn authorize_workflow_member(principal: &Principal) -> Result<(), WorkflowStudioError> {
    let allowed_by_role = principal
        .roles
        .iter()
        .any(|role| permission_for(*role, Feature::Login) == PermissionLevel::Allow);
    let allowed_by_grant = principal
        .effective_feature_grants
        .iter()
        .any(|grant| grant.feature == Feature::Login && grant.permission == PermissionLevel::Allow);
    if allowed_by_role || allowed_by_grant {
        Ok(())
    } else {
        Err(WorkflowStudioError::from(KernelError::forbidden(
            "submittable definitions require an authenticated tenant member",
        )))
    }
}

async fn create_definition(
    State(state): State<WorkflowStudioState>,
    Extension(principal): Extension<Principal>,
    Json(body): Json<CreateWorkflowDefinitionRequest>,
) -> Result<Json<WorkflowDefinitionResponse>, WorkflowStudioError> {
    authorize_workflow_manage(&principal)?;
    let draft = normalize_create_request(body)?;
    let definition_id = Uuid::new_v4();
    let actor = principal.user_id;
    let org = principal.org_id;
    let trace = TraceContext::generate();
    let now = OffsetDateTime::now_utc();
    let audit_after = json!({
        "id": definition_id,
        "workflow_key": draft.workflow_key,
        "status": "DRAFT",
        "latest_version": 1,
        "required_approval_line": draft.required_approval_line,
        "required_payment_line": draft.required_payment_line
    });
    let event = AuditEvent::new(
        Some(actor),
        AuditAction::new("workflow_definition.create_draft")?,
        "workflow_definition",
        definition_id.to_string(),
        trace,
        now,
    )
    .with_org(org)
    .with_snapshots(None, Some(audit_after));
    let response = with_audit::<_, _, WorkflowStudioError>(&state.pool, event, |tx| {
        Box::pin(async move {
            validate_object_kinds_exist(tx, &draft.definition).await?;
            let row = sqlx::query(
                r#"
                INSERT INTO workflow_definitions (
                    id, org_id, workflow_key, display_name, object_type,
                    status, latest_version, active_version, created_by, updated_by
                ) VALUES ($1, $2, $3, $4, $5, 'DRAFT', 1, NULL, $6, $6)
                RETURNING id, workflow_key, display_name, object_type, status,
                    latest_version, active_version, created_at, updated_at
                "#,
            )
            .bind(definition_id)
            .bind(*org.as_uuid())
            .bind(&draft.workflow_key)
            .bind(&draft.display_name)
            .bind(&draft.object_type)
            .bind(*actor.as_uuid())
            .fetch_one(tx.as_mut())
            .await?;

            sqlx::query(
                r#"
                INSERT INTO workflow_definition_versions (
                    org_id, definition_id, version, status, definition,
                    approval_line, payment_line, notification_rules, action_allowlist,
                    required_approval_line, required_payment_line, created_by
                ) VALUES ($1, $2, 1, 'DRAFT', $3, $4, $5, $6, $7, $8, $9, $10)
                "#,
            )
            .bind(*org.as_uuid())
            .bind(definition_id)
            .bind(&draft.definition)
            .bind(Value::Array(draft.approval_line.clone()))
            .bind(Value::Array(draft.payment_line.clone()))
            .bind(Value::Array(draft.notification_rules.clone()))
            .bind(Value::Array(draft.action_allowlist.clone()))
            .bind(draft.required_approval_line)
            .bind(draft.required_payment_line)
            .bind(*actor.as_uuid())
            .execute(tx.as_mut())
            .await?;

            insert_workflow_event(
                tx,
                WorkflowAuditEvent {
                    org,
                    definition_id,
                    version: Some(1),
                    action: "workflow_definition.create_draft",
                    actor: Some(actor),
                    summary: "초안 생성",
                    before_snap: None,
                    after_snap: Some(json!({
                        "workflow_key": draft.workflow_key,
                        "version": 1,
                        "status": "DRAFT"
                    })),
                },
            )
            .await?;

            Ok(WorkflowDefinitionResponse {
                id: row.try_get("id")?,
                workflow_key: row.try_get("workflow_key")?,
                display_name: row.try_get("display_name")?,
                object_type: row.try_get("object_type")?,
                status: row.try_get("status")?,
                latest_version: row.try_get("latest_version")?,
                active_version: row.try_get("active_version")?,
                object_kinds: definition_object_kinds(&draft.definition),
                definition: draft.definition,
                approval_line: draft.approval_line,
                payment_line: draft.payment_line,
                notification_rules: draft.notification_rules,
                action_allowlist: draft.action_allowlist,
                required_approval_line: draft.required_approval_line,
                required_payment_line: draft.required_payment_line,
                pending_version: None,
                pending_staged_by: None,
                created_at: row.try_get("created_at")?,
                updated_at: row.try_get("updated_at")?,
            })
        })
    })
    .await?;
    record_workflow_studio_request("create_draft", "success");
    Ok(Json(response))
}

async fn update_definition(
    State(state): State<WorkflowStudioState>,
    Extension(principal): Extension<Principal>,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateWorkflowDefinitionRequest>,
) -> Result<Json<WorkflowDefinitionResponse>, WorkflowStudioError> {
    authorize_workflow_manage(&principal)?;
    let update = normalize_update_request(body)?;
    let actor = principal.user_id;
    let org = principal.org_id;
    let trace = TraceContext::generate();
    let now = OffsetDateTime::now_utc();
    let event = AuditEvent::new(
        Some(actor),
        AuditAction::new("workflow_definition.update_draft")?,
        "workflow_definition",
        id.to_string(),
        trace,
        now,
    )
    .with_org(org);

    let response = with_audit::<_, _, WorkflowStudioError>(&state.pool, event, move |tx| {
        Box::pin(async move {
            let current = load_latest_version(tx, id, true).await?;
            let before = snapshot_from_row(&current);
            let next = apply_draft_update(&current, update)?;
            validate_object_kinds_exist(tx, &next.definition).await?;
            let new_version = current.latest_version + 1;
            // pendingRev decoupling: editing a LIVE definition (one that has an
            // active_version) must NOT take it out of service — the active
            // version keeps serving while the new DRAFT version is staged as the
            // proposed revision ("개정 대기 v+1 · 현행 유지"). A never-published
            // definition (active_version IS NULL) stays DRAFT as before.
            let definition_status = keep_live_status(&current);
            let updated = insert_version_and_update_definition(
                tx,
                WorkflowVersionMutation {
                    org,
                    actor,
                    source: &next,
                    new_version,
                    version_status: "DRAFT",
                    definition_status,
                    active_version: current.active_version,
                },
            )
            .await?;

            insert_workflow_event(
                tx,
                WorkflowAuditEvent {
                    org,
                    definition_id: id,
                    version: Some(new_version),
                    action: "workflow_definition.update_draft",
                    actor: Some(actor),
                    summary: "초안 편집",
                    before_snap: Some(before),
                    after_snap: Some(snapshot_from_response(&updated)),
                },
            )
            .await?;

            Ok(updated)
        })
    })
    .await?;
    record_workflow_studio_request("update_draft", "success");
    Ok(Json(response))
}

async fn archive_definition(
    State(state): State<WorkflowStudioState>,
    Extension(principal): Extension<Principal>,
    Path(id): Path<Uuid>,
    Json(body): Json<WorkflowStepUpRequest>,
) -> Result<Json<WorkflowDefinitionResponse>, WorkflowStudioError> {
    authorize_workflow_manage(&principal)?;
    verify_workflow_step_up(&state, &principal, body.step_up).await?;
    let actor = principal.user_id;
    let org = principal.org_id;
    let trace = TraceContext::generate();
    let now = OffsetDateTime::now_utc();
    let event = AuditEvent::new(
        Some(actor),
        AuditAction::new("workflow_definition.archive_draft")?,
        "workflow_definition",
        id.to_string(),
        trace,
        now,
    )
    .with_org(org);

    let response = with_audit::<_, _, WorkflowStudioError>(&state.pool, event, move |tx| {
        Box::pin(async move {
            let current = load_latest_version(tx, id, true).await?;
            ensure_draft_definition(&current, "archived")?;
            let before = snapshot_from_row(&current);
            let row = sqlx::query(
                r#"
                UPDATE workflow_definitions
                   SET status = 'RETIRED',
                       updated_by = $2,
                       updated_at = now()
                 WHERE id = $1
                RETURNING id, workflow_key, display_name, object_type, status,
                    latest_version, active_version, pending_version,
                    pending_staged_by, created_at, updated_at
                "#,
            )
            .bind(id)
            .bind(*actor.as_uuid())
            .fetch_one(tx.as_mut())
            .await?;

            let updated = WorkflowDefinitionResponse {
                id: row.try_get("id")?,
                workflow_key: row.try_get("workflow_key")?,
                display_name: row.try_get("display_name")?,
                object_type: row.try_get("object_type")?,
                status: row.try_get("status")?,
                latest_version: row.try_get("latest_version")?,
                active_version: row.try_get("active_version")?,
                object_kinds: definition_object_kinds(&current.definition),
                definition: current.definition.clone(),
                approval_line: current.approval_line.clone(),
                payment_line: current.payment_line.clone(),
                notification_rules: current.notification_rules.clone(),
                action_allowlist: current.action_allowlist.clone(),
                required_approval_line: current.required_approval_line,
                required_payment_line: current.required_payment_line,
                pending_version: row.try_get("pending_version")?,
                pending_staged_by: row.try_get("pending_staged_by")?,
                created_at: row.try_get("created_at")?,
                updated_at: row.try_get("updated_at")?,
            };

            insert_workflow_event(
                tx,
                WorkflowAuditEvent {
                    org,
                    definition_id: id,
                    version: Some(current.latest_version),
                    action: "workflow_definition.archive_draft",
                    actor: Some(actor),
                    summary: "초안 삭제",
                    before_snap: Some(before),
                    after_snap: Some(snapshot_from_response(&updated)),
                },
            )
            .await?;

            Ok(updated)
        })
    })
    .await?;
    record_workflow_studio_request("archive_draft", "success");
    Ok(Json(response))
}

async fn list_definition_history(
    State(state): State<WorkflowStudioState>,
    Extension(principal): Extension<Principal>,
    Path(id): Path<Uuid>,
) -> Result<Json<WorkflowDefinitionHistoryResponse>, WorkflowStudioError> {
    authorize_workflow_manage(&principal)?;
    let org = principal.org_id;
    let items = with_org_conn::<_, _, WorkflowStudioError>(&state.pool, org, move |tx| {
        Box::pin(async move {
            ensure_definition_exists(tx, id).await?;
            let rows = sqlx::query(
                r#"
                SELECT
                    e.id,
                    e.definition_id,
                    e.version,
                    COALESCE(v.status, 'EVENT') AS status,
                    e.action,
                    u.display_name AS actor_display_name,
                    e.summary,
                    e.created_at
                FROM workflow_definition_events e
                LEFT JOIN users u ON u.id = e.actor_id
                LEFT JOIN workflow_definition_versions v
                    ON v.definition_id = e.definition_id
                   AND v.org_id = e.org_id
                   AND v.version = e.version
                WHERE e.definition_id = $1
                ORDER BY e.created_at DESC
                "#,
            )
            .bind(id)
            .fetch_all(tx.as_mut())
            .await?;
            rows.into_iter()
                .map(|row| {
                    Ok(WorkflowDefinitionEventResponse {
                        id: row.try_get("id")?,
                        definition_id: row.try_get("definition_id")?,
                        version: row.try_get("version")?,
                        status: row.try_get("status")?,
                        action: row.try_get("action")?,
                        actor_display_name: row.try_get("actor_display_name")?,
                        summary: row.try_get("summary")?,
                        created_at: row.try_get("created_at")?,
                    })
                })
                .collect()
        })
    })
    .await?;
    Ok(Json(WorkflowDefinitionHistoryResponse { items }))
}

async fn list_definition_run_log(
    State(state): State<WorkflowStudioState>,
    Extension(principal): Extension<Principal>,
    Path(id): Path<Uuid>,
) -> Result<Json<WorkflowRunLogResponse>, WorkflowStudioError> {
    authorize_workflow_manage(&principal)?;
    let org = principal.org_id;
    let items = with_org_conn::<_, _, WorkflowStudioError>(&state.pool, org, move |tx| {
        Box::pin(async move {
            ensure_definition_exists(tx, id).await?;
            let rows = sqlx::query(workflow_run_log_sql())
                .bind(id)
                .fetch_all(tx.as_mut())
                .await?;
            rows.into_iter().map(run_response_from_row).collect()
        })
    })
    .await?;
    record_workflow_studio_request("run_log", "success");
    Ok(Json(WorkflowRunLogResponse { items }))
}

async fn trigger_definition_run(
    State(state): State<WorkflowStudioState>,
    Extension(principal): Extension<Principal>,
    Path(id): Path<Uuid>,
    Json(body): Json<TriggerWorkflowRunRequest>,
) -> Result<Json<WorkflowRunResponse>, WorkflowStudioError> {
    authorize_workflow_manage(&principal)?;
    let trigger_type = normalize_workflow_run_trigger_type(&body.trigger_type)?;
    let four_eyes_request_ref = body.four_eyes_request_ref;
    let run_id = Uuid::new_v4();
    let actor = principal.user_id;
    let org = principal.org_id;
    let idempotency_key =
        normalize_workflow_run_idempotency_key(body.idempotency_key, id, &trigger_type, run_id)?;
    let trace = TraceContext::generate();
    let trace_id = trace.trace_id().to_owned();
    let event = AuditEvent::new(
        Some(actor),
        AuditAction::new("workflow_run.trigger")?,
        "workflow_run",
        run_id.to_string(),
        trace,
        OffsetDateTime::now_utc(),
    )
    .with_org(org)
    .with_snapshots(
        None,
        Some(json!({
            "definition_id": id,
            "trigger_type": &trigger_type,
            "status": "RUNNING"
        })),
    );

    let response = with_audit::<_, _, WorkflowStudioError>(&state.pool, event, move |tx| {
        Box::pin(async move {
            let current = load_latest_version(tx, id, false).await?;
            let Some(active_version) = current.active_version else {
                return Err(WorkflowStudioError::from(KernelError::conflict(
                    "workflow definition must be active before it can run",
                )));
            };
            if current.status != "ACTIVE" {
                return Err(WorkflowStudioError::from(KernelError::conflict(
                    "workflow definition must be active before it can run",
                )));
            }
            if let Some(existing) = load_run_by_idempotency_key(tx, &idempotency_key).await? {
                return Ok(existing);
            }

            // §16 org-scope automation gate (85 판정): re-checked in this tx
            // (TOCTOU-safe), only for a genuinely new run (an idempotent replay
            // above already passed the gate when the run was first created).
            // Personal-scope (§3.9.0-①) skips the gate.
            let owner_scope_is_org = workflow_owner_scope_is_org(&current.definition);
            // Bind-match AND consume inside this run tx (single-use, TOCTOU-safe):
            // the approval must be decided for THIS definition (`id`).
            let four_eyes_approved = match four_eyes_request_ref {
                Some(request_ref) => four_eyes_consume_conn(
                    tx.as_mut(),
                    request_ref,
                    WORKFLOW_RUN_FOUR_EYES_KIND,
                    Some(id),
                    actor,
                )
                .await
                .map_err(governance_to_workflow_studio)?,
                None => None,
            };
            let gate_outcome = evaluate_automation_four_eyes_gate(owner_scope_is_org, four_eyes_approved);
            if !gate_outcome.allow {
                return Err(WorkflowStudioError::from(KernelError::forbidden(
                    "org-scope automation run requires a distinct four-eyes approval",
                )));
            }
            insert_workflow_event(
                tx,
                WorkflowAuditEvent {
                    org,
                    definition_id: id,
                    version: Some(active_version),
                    action: "workflow_definition.run_gate",
                    actor: Some(actor),
                    summary: "실행 게이트",
                    before_snap: None,
                    after_snap: Some(json!({ "run_id": run_id, "gate_outcome": gate_outcome })),
                },
            )
            .await?;

            let row = sqlx::query(
                r#"
                WITH inserted AS (
                    INSERT INTO workflow_runs (
                        id, org_id, definition_id, definition_version, status,
                        trigger_type, idempotency_key, correlation_id, trace_id,
                        input_payload, context_payload, initiated_by
                    ) VALUES (
                        $1, $2, $3, $4, 'RUNNING', $5, $6, $7, $8, $9, $10, $11
                    )
                    RETURNING id, definition_id, definition_version, trigger_type, status,
                        initiated_by, output_payload, error_payload, started_at, updated_at,
                        completed_at, failed_at
                )
                SELECT
                    i.id,
                    concat('RUN-', upper(substr(replace(i.id::text, '-', ''), 1, 6))) AS code,
                    i.definition_id,
                    i.definition_version,
                    i.trigger_type,
                    i.status,
                    u.display_name AS actor_display_name,
                    CASE i.trigger_type
                        WHEN 'SCHEDULE' THEN '예약 실행 시작'
                        ELSE '수동 실행 시작'
                    END AS summary,
                    COALESCE(i.error_payload->>'message', i.error_payload->>'error') AS error_message,
                    COALESCE(i.output_payload->'generated_objects', '[]'::jsonb) AS generated_objects,
                    i.started_at,
                    i.updated_at,
                    i.completed_at,
                    i.failed_at
                FROM inserted i
                LEFT JOIN users u ON u.id = i.initiated_by
                "#,
            )
            .bind(run_id)
            .bind(*org.as_uuid())
            .bind(id)
            .bind(active_version)
            .bind(&trigger_type)
            .bind(&idempotency_key)
            .bind(format!("workflow-studio:{run_id}"))
            .bind(trace_id)
            .bind(json!({ "trigger_type": trigger_type, "source": "workflow_studio" }))
            .bind(json!({
                "workflow_key": current.workflow_key,
                "display_name": current.display_name,
                "object_type": current.object_type
            }))
            .bind(*actor.as_uuid())
            .fetch_one(tx.as_mut())
            .await?;
            run_response_from_row(row)
        })
    })
    .await?;
    record_workflow_studio_request("trigger_run", "success");
    Ok(Json(response))
}

async fn simulate_definition(
    State(state): State<WorkflowStudioState>,
    Extension(principal): Extension<Principal>,
    Path(id): Path<Uuid>,
    Json(body): Json<SimulateWorkflowDefinitionRequest>,
) -> Result<Json<WorkflowSimulationResponse>, WorkflowStudioError> {
    authorize_workflow_manage(&principal)?;
    let org = principal.org_id;
    let sample_context = body.sample_context.unwrap_or_else(|| json!({}));
    let result = with_org_conn::<_, _, WorkflowStudioError>(&state.pool, org, move |tx| {
        Box::pin(async move {
            let mut row = load_latest_version(tx, id, false).await?;
            if let Some(definition) = body.definition {
                row.definition = validate_definition_for_object_type(definition, &row.object_type)?;
            }
            if let Some(approval_line) = body.approval_line {
                row.approval_line = approval_line;
            }
            if let Some(payment_line) = body.payment_line {
                row.payment_line = payment_line;
            }
            if let Some(notification_rules) = body.notification_rules {
                row.notification_rules = notification_rules;
            }
            if let Some(action_allowlist) = body.action_allowlist {
                validate_action_allowlist(&action_allowlist)?;
                row.action_allowlist = action_allowlist;
            }
            let mut result = simulation_for(&row);
            attach_simulated_path(&mut result, &row.definition, &sample_context);
            Ok(result)
        })
    })
    .await?;
    record_workflow_studio_request("simulate", "success");
    Ok(Json(result))
}

/// Publish a definition revision.
///
/// * A definition that has never been activated (`active_version IS NULL`) is
///   published **directly**: a new PUBLISHED version is appended and activated.
/// * A definition that is already live (`active_version IS NOT NULL`) is NOT
///   applied directly — publishing **stages** the editing-produced DRAFT as a
///   pending revision (the active version keeps serving) that a SECOND, distinct
///   actor must approve (`approve_revision`). The publisher cannot self-approve
///   (mirrors the #205 workflow-decide SoD, enforced at approve time).
async fn publish_definition(
    State(state): State<WorkflowStudioState>,
    Extension(principal): Extension<Principal>,
    Path(id): Path<Uuid>,
    Json(body): Json<WorkflowStepUpRequest>,
) -> Result<Json<WorkflowDefinitionResponse>, WorkflowStudioError> {
    authorize_workflow_manage(&principal)?;
    verify_workflow_step_up(&state, &principal, body.step_up).await?;
    let four_eyes_request_ref = body.four_eyes_request_ref;
    let actor = principal.user_id;
    let org = principal.org_id;
    let event = AuditEvent::new(
        Some(actor),
        AuditAction::new("workflow_definition.publish")?,
        "workflow_definition",
        id.to_string(),
        TraceContext::generate(),
        OffsetDateTime::now_utc(),
    )
    .with_org(org);

    let (response, staged) =
        with_audit::<_, _, WorkflowStudioError>(&state.pool, event, move |tx| {
            Box::pin(async move {
                let current = load_latest_version(tx, id, true).await?;
                ensure_not_retired(&current)?;
                let findings = validate_publishable(&current);
                if !findings.is_empty() {
                    return Err(WorkflowStudioError::validation(
                        findings
                            .into_iter()
                            .map(|finding| finding.message)
                            .collect::<Vec<_>>()
                            .join("; "),
                    ));
                }
                let before = snapshot_from_row(&current);

                if current.active_version.is_none() {
                    // Direct activate: never-published definition. §16 org-scope
                    // gate (85 판정): an org-owned automation's FIRST activation
                    // requires a distinct four-eyes approval — re-checked here,
                    // inside this writeback tx, so the gate is TOCTOU-safe.
                    // Personal-scope (§3.9.0-①) skips the gate and stays direct.
                    let owner_scope_is_org = workflow_owner_scope_is_org(&current.definition);
                    // Bind-match AND consume inside this publish tx (single-use,
                    // TOCTOU-safe): the approval must be decided for THIS definition.
                    let four_eyes_approved = match four_eyes_request_ref {
                        Some(request_ref) => four_eyes_consume_conn(
                            tx.as_mut(),
                            request_ref,
                            WORKFLOW_PUBLISH_FOUR_EYES_KIND,
                            Some(id),
                            actor,
                        )
                        .await
                        .map_err(governance_to_workflow_studio)?,
                        None => None,
                    };
                    let gate_outcome =
                        evaluate_automation_four_eyes_gate(owner_scope_is_org, four_eyes_approved);
                    if !gate_outcome.allow {
                        return Err(WorkflowStudioError::from(KernelError::forbidden(
                            "org-scope automation publish requires a distinct four-eyes approval",
                        )));
                    }

                    let new_version = current.latest_version + 1;
                    let updated = insert_version_and_update_definition(
                        tx,
                        WorkflowVersionMutation {
                            org,
                            actor,
                            source: &current,
                            new_version,
                            version_status: "PUBLISHED",
                            definition_status: "ACTIVE",
                            active_version: Some(new_version),
                        },
                    )
                    .await?;
                    insert_workflow_event(
                        tx,
                        WorkflowAuditEvent {
                            org,
                            definition_id: id,
                            version: Some(new_version),
                            action: "workflow_definition.publish",
                            actor: Some(actor),
                            summary: "게시",
                            before_snap: Some(before),
                            after_snap: Some(json!({
                                "response": snapshot_from_response(&updated),
                                "gate_outcome": gate_outcome
                            })),
                        },
                    )
                    .await?;
                    return Ok((updated, false));
                }

                // Four-eyes staging on a live definition.
                if current.pending_version.is_some() {
                    return Err(WorkflowStudioError::from(KernelError::conflict(
                        "a revision is already pending approval; approve or withdraw it first",
                    )));
                }
                if current.latest_version == current.active_version.unwrap_or_default() {
                    return Err(WorkflowStudioError::from(KernelError::conflict(
                        "no draft revision to publish; edit the definition first",
                    )));
                }
                let pending_version = current.latest_version;
                let updated = stage_pending_revision(tx, id, pending_version, actor).await?;
                insert_workflow_event(
                    tx,
                    WorkflowAuditEvent {
                        org,
                        definition_id: id,
                        version: Some(pending_version),
                        action: "workflow_definition.stage_revision",
                        actor: Some(actor),
                        summary: "개정 상신(적용 대기)",
                        before_snap: Some(before),
                        after_snap: Some(snapshot_from_response(&updated)),
                    },
                )
                .await?;
                Ok((updated, true))
            })
        })
        .await?;
    record_workflow_studio_request(if staged { "stage_revision" } else { "publish" }, "success");
    Ok(Json(response))
}

/// Set the pending-revision pointer on a live definition (staging). The active
/// version and definition status are untouched — it keeps serving.
async fn stage_pending_revision(
    tx: &mut Transaction<'_, Postgres>,
    definition_id: Uuid,
    pending_version: i32,
    actor: UserId,
) -> Result<WorkflowDefinitionResponse, WorkflowStudioError> {
    let row = sqlx::query(
        r#"
        UPDATE workflow_definitions
           SET pending_version = $2,
               pending_staged_by = $3,
               updated_by = $3,
               updated_at = now()
         WHERE id = $1
        RETURNING id, workflow_key, display_name, object_type, status,
            latest_version, active_version, pending_version, pending_staged_by,
            created_at, updated_at
        "#,
    )
    .bind(definition_id)
    .bind(pending_version)
    .bind(*actor.as_uuid())
    .fetch_one(tx.as_mut())
    .await?;
    // Carry the staged (latest DRAFT) version's content on the response.
    let staged = load_specific_version(tx, definition_id, pending_version).await?;
    definition_response(&row, &staged)
}

/// Build a definition response from a definitions row + the version row whose
/// content/lines should be surfaced.
fn definition_response(
    row: &sqlx::postgres::PgRow,
    version: &WorkflowVersionRow,
) -> Result<WorkflowDefinitionResponse, WorkflowStudioError> {
    Ok(WorkflowDefinitionResponse {
        id: row.try_get("id")?,
        workflow_key: row.try_get("workflow_key")?,
        display_name: row.try_get("display_name")?,
        object_type: row.try_get("object_type")?,
        status: row.try_get("status")?,
        latest_version: row.try_get("latest_version")?,
        active_version: row.try_get("active_version")?,
        object_kinds: definition_object_kinds(&version.definition),
        definition: version.definition.clone(),
        approval_line: version.approval_line.clone(),
        payment_line: version.payment_line.clone(),
        notification_rules: version.notification_rules.clone(),
        action_allowlist: version.action_allowlist.clone(),
        required_approval_line: version.required_approval_line,
        required_payment_line: version.required_payment_line,
        pending_version: row.try_get("pending_version")?,
        pending_staged_by: row.try_get("pending_staged_by")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

/// Approve a staged pending revision — the four-eyes application. A SECOND,
/// distinct actor (not the publisher who staged it) appends the PUBLISHED
/// version from the pending DRAFT and flips `active_version` to it. The staging
/// actor cannot self-approve, regardless of title.
async fn approve_revision(
    State(state): State<WorkflowStudioState>,
    Extension(principal): Extension<Principal>,
    Path((id, rev)): Path<(Uuid, i32)>,
    Json(body): Json<WorkflowStepUpRequest>,
) -> Result<Json<WorkflowDefinitionResponse>, WorkflowStudioError> {
    authorize_workflow_manage(&principal)?;
    verify_workflow_step_up(&state, &principal, body.step_up).await?;
    let actor = principal.user_id;
    let org = principal.org_id;
    let event = AuditEvent::new(
        Some(actor),
        AuditAction::new("workflow_definition.approve_revision")?,
        "workflow_definition",
        id.to_string(),
        TraceContext::generate(),
        OffsetDateTime::now_utc(),
    )
    .with_org(org);

    let response = with_audit::<_, _, WorkflowStudioError>(&state.pool, event, move |tx| {
        Box::pin(async move {
            let current = load_latest_version(tx, id, true).await?;
            let Some(pending) = current.pending_version else {
                return Err(WorkflowStudioError::from(KernelError::conflict(
                    "no revision is pending approval",
                )));
            };
            if pending != rev {
                return Err(WorkflowStudioError::from(KernelError::conflict(
                    "the pending revision does not match the requested version",
                )));
            }
            let staged_by = current.pending_staged_by.ok_or_else(|| {
                KernelError::forbidden("개정 기안자를 확인할 수 없어 승인할 수 없습니다")
            })?;
            if staged_by == *actor.as_uuid() {
                return Err(
                    KernelError::forbidden("본인이 상신한 개정은 승인할 수 없습니다").into(),
                );
            }

            let before = snapshot_from_row(&current);
            let source = load_specific_version(tx, id, pending).await?;
            let new_version = current.latest_version + 1;
            let source_for_insert = WorkflowVersionRow {
                latest_version: current.latest_version,
                status: current.status.clone(),
                active_version: current.active_version,
                pending_version: None,
                pending_staged_by: None,
                created_at: current.created_at,
                updated_at: current.updated_at,
                ..source
            };
            let updated = insert_version_and_clear_pending(
                tx,
                WorkflowVersionMutation {
                    org,
                    actor,
                    source: &source_for_insert,
                    new_version,
                    version_status: "PUBLISHED",
                    definition_status: "ACTIVE",
                    active_version: Some(new_version),
                },
            )
            .await?;
            insert_workflow_event(
                tx,
                WorkflowAuditEvent {
                    org,
                    definition_id: id,
                    version: Some(new_version),
                    action: "workflow_definition.approve_revision",
                    actor: Some(actor),
                    summary: "개정 적용 승인(four-eyes)",
                    before_snap: Some(before),
                    after_snap: Some(snapshot_from_response(&updated)),
                },
            )
            .await?;
            Ok(updated)
        })
    })
    .await?;
    record_workflow_studio_request("approve_revision", "success");
    Ok(Json(response))
}

/// Withdraw (discard) a staged pending revision: clears the pointer, the active
/// version keeps serving, the DRAFT stays in history. Any workflow-manager may
/// withdraw (it does not apply anything, so it is not an SoD-gated action).
async fn withdraw_revision(
    State(state): State<WorkflowStudioState>,
    Extension(principal): Extension<Principal>,
    Path((id, rev)): Path<(Uuid, i32)>,
) -> Result<Json<WorkflowDefinitionResponse>, WorkflowStudioError> {
    authorize_workflow_manage(&principal)?;
    let actor = principal.user_id;
    let org = principal.org_id;
    let event = AuditEvent::new(
        Some(actor),
        AuditAction::new("workflow_definition.withdraw_revision")?,
        "workflow_definition",
        id.to_string(),
        TraceContext::generate(),
        OffsetDateTime::now_utc(),
    )
    .with_org(org);

    let response = with_audit::<_, _, WorkflowStudioError>(&state.pool, event, move |tx| {
        Box::pin(async move {
            let current = load_latest_version(tx, id, true).await?;
            let Some(pending) = current.pending_version else {
                return Err(WorkflowStudioError::from(KernelError::conflict(
                    "no revision is pending approval",
                )));
            };
            if pending != rev {
                return Err(WorkflowStudioError::from(KernelError::conflict(
                    "the pending revision does not match the requested version",
                )));
            }
            let before = snapshot_from_row(&current);
            let row = sqlx::query(
                r#"
                UPDATE workflow_definitions
                   SET pending_version = NULL,
                       pending_staged_by = NULL,
                       updated_by = $2,
                       updated_at = now()
                 WHERE id = $1
                RETURNING id, workflow_key, display_name, object_type, status,
                    latest_version, active_version, pending_version,
                    pending_staged_by, created_at, updated_at
                "#,
            )
            .bind(id)
            .bind(*actor.as_uuid())
            .fetch_one(tx.as_mut())
            .await?;
            let updated = definition_response(&row, &current)?;
            insert_workflow_event(
                tx,
                WorkflowAuditEvent {
                    org,
                    definition_id: id,
                    version: Some(pending),
                    action: "workflow_definition.withdraw_revision",
                    actor: Some(actor),
                    summary: "개정 철회",
                    before_snap: Some(before),
                    after_snap: Some(snapshot_from_response(&updated)),
                },
            )
            .await?;
            Ok(updated)
        })
    })
    .await?;
    record_workflow_studio_request("withdraw_revision", "success");
    Ok(Json(response))
}

/// Append the approved version AND clear the pending pointer in one UPDATE.
async fn insert_version_and_clear_pending(
    tx: &mut Transaction<'_, Postgres>,
    mutation: WorkflowVersionMutation<'_>,
) -> Result<WorkflowDefinitionResponse, WorkflowStudioError> {
    sqlx::query(
        r#"
        INSERT INTO workflow_definition_versions (
            org_id, definition_id, version, status, definition,
            approval_line, payment_line, notification_rules, action_allowlist,
            required_approval_line, required_payment_line, created_by
        ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)
        "#,
    )
    .bind(*mutation.org.as_uuid())
    .bind(mutation.source.definition_id)
    .bind(mutation.new_version)
    .bind(mutation.version_status)
    .bind(&mutation.source.definition)
    .bind(Value::Array(mutation.source.approval_line.clone()))
    .bind(Value::Array(mutation.source.payment_line.clone()))
    .bind(Value::Array(mutation.source.notification_rules.clone()))
    .bind(Value::Array(mutation.source.action_allowlist.clone()))
    .bind(mutation.source.required_approval_line)
    .bind(mutation.source.required_payment_line)
    .bind(*mutation.actor.as_uuid())
    .execute(tx.as_mut())
    .await?;

    let row = sqlx::query(
        r#"
        UPDATE workflow_definitions
           SET status = $2,
               latest_version = $3,
               active_version = $4,
               pending_version = NULL,
               pending_staged_by = NULL,
               updated_by = $5,
               updated_at = now()
         WHERE id = $1
        RETURNING id, workflow_key, display_name, object_type, status,
            latest_version, active_version, pending_version, pending_staged_by,
            created_at, updated_at
        "#,
    )
    .bind(mutation.source.definition_id)
    .bind(mutation.definition_status)
    .bind(mutation.new_version)
    .bind(mutation.active_version)
    .bind(*mutation.actor.as_uuid())
    .fetch_one(tx.as_mut())
    .await?;
    definition_response(&row, mutation.source)
}

async fn pause_definition(
    State(state): State<WorkflowStudioState>,
    Extension(principal): Extension<Principal>,
    Path(id): Path<Uuid>,
    Json(body): Json<WorkflowStepUpRequest>,
) -> Result<Json<WorkflowDefinitionResponse>, WorkflowStudioError> {
    authorize_workflow_manage(&principal)?;
    verify_workflow_step_up(&state, &principal, body.step_up).await?;
    mutate_definition(
        &state,
        principal,
        id,
        "workflow_definition.pause",
        "일시정지",
        |row| {
            if row.status != "ACTIVE" {
                return Err(WorkflowStudioError::validation(
                    "only ACTIVE workflow definitions can be paused",
                ));
            }
            Ok((
                "PAUSED",
                "PAUSED",
                row.latest_version + 1,
                row.active_version,
            ))
        },
    )
    .await
    .map(Json)
}

async fn resume_definition(
    State(state): State<WorkflowStudioState>,
    Extension(principal): Extension<Principal>,
    Path(id): Path<Uuid>,
    Json(body): Json<WorkflowStepUpRequest>,
) -> Result<Json<WorkflowDefinitionResponse>, WorkflowStudioError> {
    authorize_workflow_manage(&principal)?;
    verify_workflow_step_up(&state, &principal, body.step_up).await?;
    mutate_definition(
        &state,
        principal,
        id,
        "workflow_definition.resume",
        "재개",
        |row| {
            if row.status != "PAUSED" {
                return Err(WorkflowStudioError::validation(
                    "only PAUSED workflow definitions can be resumed",
                ));
            }
            Ok((
                "ACTIVE",
                "RESUMED",
                row.latest_version + 1,
                row.active_version,
            ))
        },
    )
    .await
    .map(Json)
}

async fn rollback_definition(
    State(state): State<WorkflowStudioState>,
    Extension(principal): Extension<Principal>,
    Path(id): Path<Uuid>,
    Json(body): Json<RollbackWorkflowDefinitionRequest>,
) -> Result<Json<WorkflowDefinitionResponse>, WorkflowStudioError> {
    authorize_workflow_manage(&principal)?;
    verify_workflow_step_up(&state, &principal, body.step_up).await?;
    if body.target_version < 1 {
        return Err(WorkflowStudioError::validation(
            "target_version must be 1 or greater",
        ));
    }
    let target_version = body.target_version;
    mutate_definition_with_source_version(
        &state,
        principal,
        id,
        target_version,
        "workflow_definition.rollback",
        "롤백",
    )
    .await
    .map(Json)
}

async fn clone_definition(
    State(state): State<WorkflowStudioState>,
    Extension(principal): Extension<Principal>,
    Path(id): Path<Uuid>,
    Json(body): Json<CloneWorkflowDefinitionRequest>,
) -> Result<Json<WorkflowDefinitionResponse>, WorkflowStudioError> {
    authorize_workflow_manage(&principal)?;
    verify_workflow_step_up(&state, &principal, body.step_up).await?;
    let actor = principal.user_id;
    let org = principal.org_id;
    let new_id = Uuid::new_v4();
    let trace = TraceContext::generate();
    let now = OffsetDateTime::now_utc();
    let event = AuditEvent::new(
        Some(actor),
        AuditAction::new("workflow_definition.clone")?,
        "workflow_definition",
        new_id.to_string(),
        trace,
        now,
    )
    .with_org(org)
    .with_snapshots(Some(json!({ "source_definition_id": id })), None);
    let response = with_audit::<_, _, WorkflowStudioError>(&state.pool, event, move |tx| {
        Box::pin(async move {
            let source = load_latest_version(tx, id, true).await?;
            ensure_not_retired(&source)?;
            validate_definition_for_object_type(source.definition.clone(), &source.object_type)?;
            let workflow_key = match body.workflow_key {
                Some(value) => normalize_workflow_key(&value)?,
                None => format!(
                    "{}_copy_{}",
                    source.workflow_key,
                    &new_id.simple().to_string()[..8]
                ),
            };
            let display_name = body
                .display_name
                .map(|value| normalize_display_name(&value))
                .transpose()?
                .unwrap_or_else(|| format!("{} 복제본", source.display_name));

            let row = sqlx::query(
                r#"
                INSERT INTO workflow_definitions (
                    id, org_id, workflow_key, display_name, object_type,
                    status, latest_version, active_version, created_by, updated_by
                ) VALUES ($1, $2, $3, $4, $5, 'DRAFT', 1, NULL, $6, $6)
                RETURNING id, workflow_key, display_name, object_type, status,
                    latest_version, active_version, created_at, updated_at
                "#,
            )
            .bind(new_id)
            .bind(*org.as_uuid())
            .bind(&workflow_key)
            .bind(&display_name)
            .bind(&source.object_type)
            .bind(*actor.as_uuid())
            .fetch_one(tx.as_mut())
            .await?;

            sqlx::query(
                r#"
                INSERT INTO workflow_definition_versions (
                    org_id, definition_id, version, status, definition,
                    approval_line, payment_line, notification_rules, action_allowlist,
                    required_approval_line, required_payment_line, created_by
                ) VALUES ($1, $2, 1, 'CLONED', $3, $4, $5, $6, $7, $8, $9, $10)
                "#,
            )
            .bind(*org.as_uuid())
            .bind(new_id)
            .bind(&source.definition)
            .bind(Value::Array(source.approval_line.clone()))
            .bind(Value::Array(source.payment_line.clone()))
            .bind(Value::Array(source.notification_rules.clone()))
            .bind(Value::Array(source.action_allowlist.clone()))
            .bind(source.required_approval_line)
            .bind(source.required_payment_line)
            .bind(*actor.as_uuid())
            .execute(tx.as_mut())
            .await?;

            insert_workflow_event(
                tx,
                WorkflowAuditEvent {
                    org,
                    definition_id: new_id,
                    version: Some(1),
                    action: "workflow_definition.clone",
                    actor: Some(actor),
                    summary: "복제본 생성",
                    before_snap: Some(json!({
                        "source_definition_id": id,
                        "source_version": source.latest_version
                    })),
                    after_snap: Some(json!({
                        "workflow_key": workflow_key,
                        "version": 1,
                        "status": "DRAFT"
                    })),
                },
            )
            .await?;

            Ok(WorkflowDefinitionResponse {
                id: row.try_get("id")?,
                workflow_key,
                display_name,
                object_type: row.try_get("object_type")?,
                status: row.try_get("status")?,
                latest_version: row.try_get("latest_version")?,
                active_version: row.try_get("active_version")?,
                object_kinds: definition_object_kinds(&source.definition),
                definition: source.definition,
                approval_line: source.approval_line,
                payment_line: source.payment_line,
                notification_rules: source.notification_rules,
                action_allowlist: source.action_allowlist,
                required_approval_line: source.required_approval_line,
                required_payment_line: source.required_payment_line,
                pending_version: None,
                pending_staged_by: None,
                created_at: row.try_get("created_at")?,
                updated_at: row.try_get("updated_at")?,
            })
        })
    })
    .await?;
    record_workflow_studio_request("clone", "success");
    Ok(Json(response))
}

async fn mutate_definition<F>(
    state: &WorkflowStudioState,
    principal: Principal,
    definition_id: Uuid,
    audit_action: &'static str,
    summary: &'static str,
    transition: F,
) -> Result<WorkflowDefinitionResponse, WorkflowStudioError>
where
    F: FnOnce(
            &WorkflowVersionRow,
        )
            -> Result<(&'static str, &'static str, i32, Option<i32>), WorkflowStudioError>
        + Send
        + 'static,
{
    let actor = principal.user_id;
    let org = principal.org_id;
    let trace = TraceContext::generate();
    let now = OffsetDateTime::now_utc();
    let event = AuditEvent::new(
        Some(actor),
        AuditAction::new(audit_action)?,
        "workflow_definition",
        definition_id.to_string(),
        trace,
        now,
    )
    .with_org(org);

    let response = with_audit::<_, _, WorkflowStudioError>(&state.pool, event, move |tx| {
        Box::pin(async move {
            let current = load_latest_version(tx, definition_id, true).await?;
            ensure_not_retired(&current)?;
            let before = snapshot_from_row(&current);
            let (definition_status, version_status, new_version, active_version_override) =
                transition(&current)?;
            let active_version = if definition_status == "ACTIVE" {
                Some(new_version)
            } else {
                active_version_override
            };

            let updated = insert_version_and_update_definition(
                tx,
                WorkflowVersionMutation {
                    org,
                    actor,
                    source: &current,
                    new_version,
                    version_status,
                    definition_status,
                    active_version,
                },
            )
            .await?;

            insert_workflow_event(
                tx,
                WorkflowAuditEvent {
                    org,
                    definition_id,
                    version: Some(new_version),
                    action: audit_action,
                    actor: Some(actor),
                    summary,
                    before_snap: Some(before),
                    after_snap: Some(snapshot_from_response(&updated)),
                },
            )
            .await?;
            Ok(updated)
        })
    })
    .await?;
    record_workflow_studio_request(audit_action, "success");
    Ok(response)
}

async fn mutate_definition_with_source_version(
    state: &WorkflowStudioState,
    principal: Principal,
    definition_id: Uuid,
    target_version: i32,
    audit_action: &'static str,
    summary: &'static str,
) -> Result<WorkflowDefinitionResponse, WorkflowStudioError> {
    let actor = principal.user_id;
    let org = principal.org_id;
    let trace = TraceContext::generate();
    let now = OffsetDateTime::now_utc();
    let event = AuditEvent::new(
        Some(actor),
        AuditAction::new(audit_action)?,
        "workflow_definition",
        definition_id.to_string(),
        trace,
        now,
    )
    .with_org(org);

    let response = with_audit::<_, _, WorkflowStudioError>(&state.pool, event, move |tx| {
        Box::pin(async move {
            let current = load_latest_version(tx, definition_id, true).await?;
            ensure_not_retired(&current)?;
            let source = load_specific_version(tx, definition_id, target_version).await?;
            validate_definition_for_object_type(source.definition.clone(), &source.object_type)?;
            let before = snapshot_from_row(&current);
            let new_version = current.latest_version + 1;
            let source_for_insert = WorkflowVersionRow {
                latest_version: current.latest_version,
                status: current.status.clone(),
                active_version: current.active_version,
                created_at: current.created_at,
                updated_at: current.updated_at,
                ..source
            };
            let updated = insert_version_and_update_definition(
                tx,
                WorkflowVersionMutation {
                    org,
                    actor,
                    source: &source_for_insert,
                    new_version,
                    version_status: "ROLLED_BACK",
                    definition_status: "ACTIVE",
                    active_version: Some(new_version),
                },
            )
            .await?;

            insert_workflow_event(
                tx,
                WorkflowAuditEvent {
                    org,
                    definition_id,
                    version: Some(new_version),
                    action: audit_action,
                    actor: Some(actor),
                    summary,
                    before_snap: Some(before),
                    after_snap: Some(json!({
                        "rolled_back_to": target_version,
                        "new_version": new_version,
                        "status": "ACTIVE"
                    })),
                },
            )
            .await?;
            Ok(updated)
        })
    })
    .await?;
    record_workflow_studio_request(audit_action, "success");
    Ok(response)
}

struct WorkflowVersionMutation<'a> {
    org: console_kernel_core::OrgId,
    actor: UserId,
    source: &'a WorkflowVersionRow,
    new_version: i32,
    version_status: &'static str,
    definition_status: &'static str,
    active_version: Option<i32>,
}

async fn insert_version_and_update_definition(
    tx: &mut Transaction<'_, Postgres>,
    mutation: WorkflowVersionMutation<'_>,
) -> Result<WorkflowDefinitionResponse, WorkflowStudioError> {
    sqlx::query(
        r#"
        INSERT INTO workflow_definition_versions (
            org_id, definition_id, version, status, definition,
            approval_line, payment_line, notification_rules, action_allowlist,
            required_approval_line, required_payment_line, created_by
        ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)
        "#,
    )
    .bind(*mutation.org.as_uuid())
    .bind(mutation.source.definition_id)
    .bind(mutation.new_version)
    .bind(mutation.version_status)
    .bind(&mutation.source.definition)
    .bind(Value::Array(mutation.source.approval_line.clone()))
    .bind(Value::Array(mutation.source.payment_line.clone()))
    .bind(Value::Array(mutation.source.notification_rules.clone()))
    .bind(Value::Array(mutation.source.action_allowlist.clone()))
    .bind(mutation.source.required_approval_line)
    .bind(mutation.source.required_payment_line)
    .bind(*mutation.actor.as_uuid())
    .execute(tx.as_mut())
    .await?;

    let row = sqlx::query(
        r#"
        UPDATE workflow_definitions
           SET display_name = $2,
               status = $3,
               latest_version = $4,
               active_version = $5,
               updated_by = $6,
               updated_at = now()
         WHERE id = $1
        RETURNING id, workflow_key, display_name, object_type, status,
            latest_version, active_version, pending_version, pending_staged_by,
            created_at, updated_at
        "#,
    )
    .bind(mutation.source.definition_id)
    .bind(&mutation.source.display_name)
    .bind(mutation.definition_status)
    .bind(mutation.new_version)
    .bind(mutation.active_version)
    .bind(*mutation.actor.as_uuid())
    .fetch_one(tx.as_mut())
    .await?;

    Ok(WorkflowDefinitionResponse {
        id: row.try_get("id")?,
        workflow_key: row.try_get("workflow_key")?,
        display_name: row.try_get("display_name")?,
        object_type: row.try_get("object_type")?,
        status: row.try_get("status")?,
        latest_version: row.try_get("latest_version")?,
        active_version: row.try_get("active_version")?,
        definition: mutation.source.definition.clone(),
        object_kinds: definition_object_kinds(&mutation.source.definition),
        approval_line: mutation.source.approval_line.clone(),
        payment_line: mutation.source.payment_line.clone(),
        notification_rules: mutation.source.notification_rules.clone(),
        action_allowlist: mutation.source.action_allowlist.clone(),
        required_approval_line: mutation.source.required_approval_line,
        required_payment_line: mutation.source.required_payment_line,
        pending_version: row.try_get("pending_version")?,
        pending_staged_by: row.try_get("pending_staged_by")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

async fn load_latest_version(
    tx: &mut Transaction<'_, Postgres>,
    definition_id: Uuid,
    for_update: bool,
) -> Result<WorkflowVersionRow, WorkflowStudioError> {
    let row = if for_update {
        sqlx::query(
            r#"
            SELECT
                d.id,
                d.workflow_key,
                d.display_name,
                d.object_type,
                d.status,
                d.latest_version,
                d.active_version,
                d.pending_version,
                d.pending_staged_by,
                d.created_at,
                d.updated_at,
                v.definition,
                v.approval_line,
                v.payment_line,
                v.notification_rules,
                v.action_allowlist,
                v.required_approval_line,
                v.required_payment_line
            FROM workflow_definitions d
            JOIN workflow_definition_versions v
              ON v.definition_id = d.id
             AND v.org_id = d.org_id
             AND v.version = d.latest_version
            WHERE d.id = $1
            FOR UPDATE OF d
            "#,
        )
        .bind(definition_id)
        .fetch_optional(tx.as_mut())
        .await?
    } else {
        sqlx::query(
            r#"
            SELECT
                d.id,
                d.workflow_key,
                d.display_name,
                d.object_type,
                d.status,
                d.latest_version,
                d.active_version,
                d.pending_version,
                d.pending_staged_by,
                d.created_at,
                d.updated_at,
                v.definition,
                v.approval_line,
                v.payment_line,
                v.notification_rules,
                v.action_allowlist,
                v.required_approval_line,
                v.required_payment_line
            FROM workflow_definitions d
            JOIN workflow_definition_versions v
              ON v.definition_id = d.id
             AND v.org_id = d.org_id
             AND v.version = d.latest_version
            WHERE d.id = $1
            "#,
        )
        .bind(definition_id)
        .fetch_optional(tx.as_mut())
        .await?
    }
    .ok_or_else(|| KernelError::not_found("workflow definition not found"))?;
    row_to_version(row)
}

async fn load_specific_version(
    tx: &mut Transaction<'_, Postgres>,
    definition_id: Uuid,
    version: i32,
) -> Result<WorkflowVersionRow, WorkflowStudioError> {
    let row = sqlx::query(
        r#"
        SELECT
            d.id,
            d.workflow_key,
            d.display_name,
            d.object_type,
            d.status,
            d.latest_version,
            d.active_version,
            d.pending_version,
            d.pending_staged_by,
            d.created_at,
            d.updated_at,
            v.definition,
            v.approval_line,
            v.payment_line,
            v.notification_rules,
            v.action_allowlist,
            v.required_approval_line,
            v.required_payment_line
        FROM workflow_definitions d
        JOIN workflow_definition_versions v
          ON v.definition_id = d.id
         AND v.org_id = d.org_id
         AND v.version = $2
        WHERE d.id = $1
        "#,
    )
    .bind(definition_id)
    .bind(version)
    .fetch_optional(tx.as_mut())
    .await?
    .ok_or_else(|| KernelError::not_found("workflow definition version not found"))?;
    row_to_version(row)
}

async fn ensure_definition_exists(
    tx: &mut Transaction<'_, Postgres>,
    definition_id: Uuid,
) -> Result<(), WorkflowStudioError> {
    let exists = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM workflow_definitions WHERE id = $1)",
    )
    .bind(definition_id)
    .fetch_one(tx.as_mut())
    .await?;
    if exists {
        Ok(())
    } else {
        Err(KernelError::not_found("workflow definition not found").into())
    }
}

struct WorkflowAuditEvent {
    org: console_kernel_core::OrgId,
    definition_id: Uuid,
    version: Option<i32>,
    action: &'static str,
    actor: Option<UserId>,
    summary: &'static str,
    before_snap: Option<Value>,
    after_snap: Option<Value>,
}

async fn insert_workflow_event(
    tx: &mut Transaction<'_, Postgres>,
    event: WorkflowAuditEvent,
) -> Result<(), WorkflowStudioError> {
    sqlx::query(
        r#"
        INSERT INTO workflow_definition_events (
            org_id, definition_id, version, action, actor_id,
            summary, before_snap, after_snap
        ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
        "#,
    )
    .bind(*event.org.as_uuid())
    .bind(event.definition_id)
    .bind(event.version)
    .bind(event.action)
    .bind(event.actor.map(|user| *user.as_uuid()))
    .bind(event.summary)
    .bind(event.before_snap)
    .bind(event.after_snap)
    .execute(tx.as_mut())
    .await?;
    Ok(())
}

fn response_from_row(
    row: sqlx::postgres::PgRow,
) -> Result<WorkflowDefinitionResponse, WorkflowStudioError> {
    let definition: Value = row.try_get("definition")?;
    let object_kinds = definition_object_kinds(&definition);
    Ok(WorkflowDefinitionResponse {
        id: row.try_get("id")?,
        workflow_key: row.try_get("workflow_key")?,
        display_name: row.try_get("display_name")?,
        object_type: row.try_get("object_type")?,
        status: row.try_get("status")?,
        latest_version: row.try_get("latest_version")?,
        active_version: row.try_get("active_version")?,
        definition,
        object_kinds,
        approval_line: json_array(row.try_get("approval_line")?),
        payment_line: json_array(row.try_get("payment_line")?),
        notification_rules: json_array(row.try_get("notification_rules")?),
        action_allowlist: json_array(row.try_get("action_allowlist")?),
        required_approval_line: row.try_get("required_approval_line")?,
        required_payment_line: row.try_get("required_payment_line")?,
        pending_version: row.try_get("pending_version")?,
        pending_staged_by: row.try_get("pending_staged_by")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

fn row_to_version(row: sqlx::postgres::PgRow) -> Result<WorkflowVersionRow, WorkflowStudioError> {
    Ok(WorkflowVersionRow {
        definition_id: row.try_get("id")?,
        workflow_key: row.try_get("workflow_key")?,
        display_name: row.try_get("display_name")?,
        object_type: row.try_get("object_type")?,
        status: row.try_get("status")?,
        latest_version: row.try_get("latest_version")?,
        active_version: row.try_get("active_version")?,
        definition: row.try_get("definition")?,
        approval_line: json_array(row.try_get("approval_line")?),
        payment_line: json_array(row.try_get("payment_line")?),
        notification_rules: json_array(row.try_get("notification_rules")?),
        action_allowlist: json_array(row.try_get("action_allowlist")?),
        required_approval_line: row.try_get("required_approval_line")?,
        required_payment_line: row.try_get("required_payment_line")?,
        pending_version: row.try_get("pending_version")?,
        pending_staged_by: row.try_get("pending_staged_by")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

fn run_response_from_row(
    row: sqlx::postgres::PgRow,
) -> Result<WorkflowRunResponse, WorkflowStudioError> {
    Ok(WorkflowRunResponse {
        id: row.try_get("id")?,
        code: row.try_get("code")?,
        definition_id: row.try_get("definition_id")?,
        definition_version: row.try_get("definition_version")?,
        trigger_type: row.try_get("trigger_type")?,
        status: row.try_get("status")?,
        actor_display_name: row.try_get("actor_display_name")?,
        summary: row.try_get("summary")?,
        error_message: row.try_get("error_message")?,
        generated_objects: json_string_array(row.try_get("generated_objects")?),
        started_at: row.try_get("started_at")?,
        updated_at: row.try_get("updated_at")?,
        completed_at: row.try_get("completed_at")?,
        failed_at: row.try_get("failed_at")?,
    })
}

async fn load_run_by_idempotency_key(
    tx: &mut Transaction<'_, Postgres>,
    idempotency_key: &str,
) -> Result<Option<WorkflowRunResponse>, WorkflowStudioError> {
    sqlx::query(workflow_run_by_idempotency_key_sql())
        .bind(idempotency_key)
        .fetch_optional(tx.as_mut())
        .await?
        .map(run_response_from_row)
        .transpose()
}

fn workflow_run_log_sql() -> &'static str {
    r#"
    SELECT
        r.id,
        concat('RUN-', upper(substr(replace(r.id::text, '-', ''), 1, 6))) AS code,
        r.definition_id,
        r.definition_version,
        r.trigger_type,
        r.status,
        u.display_name AS actor_display_name,
        CASE r.status
            WHEN 'SUCCEEDED' THEN '실행 완료'
            WHEN 'FAILED' THEN '실행 실패'
            WHEN 'CANCELLED' THEN '실행 취소'
            WHEN 'WAITING' THEN '승인/작업 대기'
            ELSE CASE r.trigger_type
                WHEN 'SCHEDULE' THEN '예약 실행 시작'
                ELSE '수동 실행 시작'
            END
        END AS summary,
        COALESCE(r.error_payload->>'message', r.error_payload->>'error') AS error_message,
        COALESCE(r.output_payload->'generated_objects', '[]'::jsonb) AS generated_objects,
        r.started_at,
        r.updated_at,
        r.completed_at,
        r.failed_at
    FROM workflow_runs r
    LEFT JOIN users u ON u.id = r.initiated_by
    WHERE r.definition_id = $1
    ORDER BY COALESCE(r.completed_at, r.failed_at, r.updated_at, r.started_at) DESC, r.id DESC
    LIMIT 25
    "#
}

fn workflow_run_by_idempotency_key_sql() -> &'static str {
    r#"
    SELECT
        r.id,
        concat('RUN-', upper(substr(replace(r.id::text, '-', ''), 1, 6))) AS code,
        r.definition_id,
        r.definition_version,
        r.trigger_type,
        r.status,
        u.display_name AS actor_display_name,
        CASE r.status
            WHEN 'SUCCEEDED' THEN '실행 완료'
            WHEN 'FAILED' THEN '실행 실패'
            WHEN 'CANCELLED' THEN '실행 취소'
            WHEN 'WAITING' THEN '승인/작업 대기'
            ELSE CASE r.trigger_type
                WHEN 'SCHEDULE' THEN '예약 실행 시작'
                ELSE '수동 실행 시작'
            END
        END AS summary,
        COALESCE(r.error_payload->>'message', r.error_payload->>'error') AS error_message,
        COALESCE(r.output_payload->'generated_objects', '[]'::jsonb) AS generated_objects,
        r.started_at,
        r.updated_at,
        r.completed_at,
        r.failed_at
    FROM workflow_runs r
    LEFT JOIN users u ON u.id = r.initiated_by
    WHERE r.idempotency_key = $1
    LIMIT 1
    "#
}

fn default_workflow_run_trigger_type() -> String {
    "MANUAL".to_owned()
}

fn normalize_workflow_run_trigger_type(value: &str) -> Result<String, WorkflowStudioError> {
    let normalized = value.trim().to_ascii_uppercase();
    match normalized.as_str() {
        "MANUAL" | "SCHEDULE" | "WEBHOOK" | "SYSTEM" => Ok(normalized),
        _ => Err(WorkflowStudioError::validation(
            "workflow run trigger_type must be MANUAL, SCHEDULE, WEBHOOK, or SYSTEM",
        )),
    }
}

fn normalize_workflow_run_idempotency_key(
    value: Option<String>,
    definition_id: Uuid,
    trigger_type: &str,
    run_id: Uuid,
) -> Result<String, WorkflowStudioError> {
    match value {
        Some(raw) => {
            let trimmed = raw.trim();
            if trimmed.is_empty() || trimmed.len() > 160 {
                return Err(WorkflowStudioError::validation(
                    "workflow run idempotency_key must be 1..160 characters",
                ));
            }
            Ok(trimmed.to_owned())
        }
        None => Ok(format!(
            "workflow-studio:{definition_id}:{trigger_type}:{}",
            run_id.simple()
        )),
    }
}

fn normalize_create_request(
    body: CreateWorkflowDefinitionRequest,
) -> Result<CreateWorkflowDefinitionRequest, WorkflowStudioError> {
    let workflow_key = normalize_workflow_key(&body.workflow_key)?;
    let display_name = normalize_display_name(&body.display_name)?;
    let object_type = normalize_object_type(&body.object_type)?;
    let definition = validate_definition_for_object_type(body.definition, &object_type)?;
    validate_action_allowlist(&body.action_allowlist)?;
    validate_notification_rules(&body.notification_rules)?;
    Ok(CreateWorkflowDefinitionRequest {
        workflow_key,
        display_name,
        object_type,
        definition,
        approval_line: body.approval_line,
        payment_line: body.payment_line,
        notification_rules: body.notification_rules,
        action_allowlist: body.action_allowlist,
        required_approval_line: body.required_approval_line,
        required_payment_line: body.required_payment_line,
    })
}

struct NormalizedWorkflowDefinitionUpdate {
    display_name: Option<String>,
    definition: Option<Value>,
    approval_line: Option<Vec<Value>>,
    payment_line: Option<Vec<Value>>,
    notification_rules: Option<Vec<Value>>,
    action_allowlist: Option<Vec<Value>>,
    required_approval_line: Option<bool>,
    required_payment_line: Option<bool>,
}

fn normalize_update_request(
    body: UpdateWorkflowDefinitionRequest,
) -> Result<NormalizedWorkflowDefinitionUpdate, WorkflowStudioError> {
    if body.display_name.is_none()
        && body.definition.is_none()
        && body.approval_line.is_none()
        && body.payment_line.is_none()
        && body.notification_rules.is_none()
        && body.action_allowlist.is_none()
        && body.required_approval_line.is_none()
        && body.required_payment_line.is_none()
    {
        return Err(WorkflowStudioError::validation(
            "workflow draft update requires at least one field",
        ));
    }
    let display_name = body
        .display_name
        .map(|value| normalize_display_name(&value))
        .transpose()?;
    let definition = body
        .definition
        .map(validate_definition_for_optional_object_type)
        .transpose()?;
    if let Some(action_allowlist) = &body.action_allowlist {
        validate_action_allowlist(action_allowlist)?;
    }
    if let Some(notification_rules) = &body.notification_rules {
        validate_notification_rules(notification_rules)?;
    }
    Ok(NormalizedWorkflowDefinitionUpdate {
        display_name,
        definition,
        approval_line: body.approval_line,
        payment_line: body.payment_line,
        notification_rules: body.notification_rules,
        action_allowlist: body.action_allowlist,
        required_approval_line: body.required_approval_line,
        required_payment_line: body.required_payment_line,
    })
}

fn apply_draft_update(
    current: &WorkflowVersionRow,
    update: NormalizedWorkflowDefinitionUpdate,
) -> Result<WorkflowVersionRow, WorkflowStudioError> {
    ensure_editable(current)?;
    let mut next = current.clone();
    if let Some(display_name) = update.display_name {
        next.display_name = display_name;
    }
    if let Some(definition) = update.definition {
        next.definition = definition;
    }
    if let Some(approval_line) = update.approval_line {
        next.approval_line = approval_line;
    }
    if let Some(payment_line) = update.payment_line {
        next.payment_line = payment_line;
    }
    if let Some(notification_rules) = update.notification_rules {
        next.notification_rules = notification_rules;
    }
    if let Some(action_allowlist) = update.action_allowlist {
        next.action_allowlist = action_allowlist;
    }
    if let Some(required_approval_line) = update.required_approval_line {
        next.required_approval_line = required_approval_line;
    }
    if let Some(required_payment_line) = update.required_payment_line {
        next.required_payment_line = required_payment_line;
    }
    validate_definition_for_object_type(next.definition.clone(), &next.object_type)?;
    Ok(next)
}

fn ensure_draft_definition(
    row: &WorkflowVersionRow,
    operation: &'static str,
) -> Result<(), WorkflowStudioError> {
    if row.status == "DRAFT" {
        Ok(())
    } else {
        Err(KernelError::invalid_transition(format!(
            "only DRAFT workflow definitions can be {operation}"
        ))
        .into())
    }
}

/// Editing a definition (PATCH) is the pendingRev entry point: a DRAFT edits in
/// place, and a LIVE definition (ACTIVE/PAUSED) produces a v+1 DRAFT revision
/// while its active version keeps serving. Only a RETIRED definition, or one
/// that already has a revision awaiting approval, refuses the edit.
fn ensure_editable(row: &WorkflowVersionRow) -> Result<(), WorkflowStudioError> {
    if row.status == "RETIRED" {
        return Err(KernelError::invalid_transition(
            "retired workflow definitions cannot be edited",
        )
        .into());
    }
    if row.pending_version.is_some() {
        return Err(KernelError::conflict(
            "a revision is already pending approval; approve or withdraw it before editing",
        )
        .into());
    }
    Ok(())
}

fn ensure_not_retired(row: &WorkflowVersionRow) -> Result<(), WorkflowStudioError> {
    if row.status == "RETIRED" {
        Err(
            KernelError::invalid_transition("archived workflow definitions cannot be changed")
                .into(),
        )
    } else {
        Ok(())
    }
}

fn keep_live_status(current: &WorkflowVersionRow) -> &'static str {
    if current.active_version.is_none() {
        return "DRAFT";
    }
    match current.status.as_str() {
        "PAUSED" => "PAUSED",
        _ => "ACTIVE",
    }
}

fn normalize_workflow_key(raw: &str) -> Result<String, WorkflowStudioError> {
    let value = raw.trim().to_ascii_lowercase();
    let valid = value.split('.').count() >= 2
        && value.split('.').all(|segment| {
            let mut chars = segment.chars();
            matches!(chars.next(), Some(first) if first.is_ascii_lowercase())
                && chars.all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_')
        });
    if valid && value.len() <= 160 {
        Ok(value)
    } else {
        Err(WorkflowStudioError::validation(
            "workflow_key must be dot-separated lowercase segments",
        ))
    }
}

fn normalize_object_type(raw: &str) -> Result<String, WorkflowStudioError> {
    let value = raw.trim().to_ascii_lowercase();
    let valid = value.len() >= 2
        && value.len() <= 64
        && value
            .chars()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_')
        && value
            .chars()
            .next()
            .is_some_and(|first| first.is_ascii_lowercase());
    if valid {
        Ok(value)
    } else {
        Err(WorkflowStudioError::validation(
            "object_type must be lowercase snake_case",
        ))
    }
}

fn normalize_display_name(raw: &str) -> Result<String, WorkflowStudioError> {
    let value = raw.trim().to_owned();
    if (1..=120).contains(&value.chars().count()) {
        Ok(value)
    } else {
        Err(WorkflowStudioError::validation(
            "display_name must be 1 to 120 characters",
        ))
    }
}

#[derive(Debug, Clone)]
struct WorkflowGraphNodeInfo {
    id: String,
    node_type: String,
    input_ports: Vec<WorkflowPortInfo>,
    output_ports: Vec<WorkflowPortInfo>,
}

#[derive(Debug, Clone)]
struct WorkflowPortInfo {
    key: String,
    port_type: String,
    required: bool,
    cardinality_one: bool,
}

fn validate_definition_for_object_type(
    value: Value,
    object_type: &str,
) -> Result<Value, WorkflowStudioError> {
    validate_definition_with_expected_object_type(value, Some(object_type))
}

fn validate_definition_for_optional_object_type(
    value: Value,
) -> Result<Value, WorkflowStudioError> {
    validate_definition_with_expected_object_type(value, None)
}

#[allow(dead_code)]
fn build_approval_execution_definition(template_key: &str) -> Result<Value, WorkflowStudioError> {
    let template = APPROVAL_TEMPLATES
        .iter()
        .find(|template| template.key == template_key)
        .ok_or_else(|| WorkflowStudioError::validation("unknown approval template"))?;

    let mut nodes = Vec::with_capacity(template.default_line.len() + 3);
    nodes.push(json!({ "node_key": "submit", "node_type": "object_gate" }));
    for (index, role_key) in template.default_line.iter().enumerate() {
        nodes.push(json!({
            "node_key": format!("approve.{role_key}"),
            "node_type": "human_task",
            "title": format!("Approval step {}", index + 1),
            "required_policy": "approval_decide",
            "assignee_role_key": role_key
        }));
    }
    nodes.push(json!({
        "node_key": "finalize.author",
        "node_type": "human_task",
        "title": "Author finalize",
        "required_policy": "approval_finalize",
        "assignee_role_key": "initiator"
    }));
    if template.receipt_required {
        nodes.push(json!({
            "node_key": "receipt.target",
            "node_type": "human_task",
            "title": "Receipt confirmation",
            "required_policy": "approval_receipt",
            "assignee_role_key": "receipt_subject"
        }));
    }

    let mut edges = Vec::with_capacity(nodes.len().saturating_sub(1));
    for pair in nodes.windows(2) {
        edges.push(json!({
            "from": pair[0]["node_key"],
            "to": pair[1]["node_key"]
        }));
    }

    // No `start_policy`: 전자결재 기안/상신 is deliberately all-employee self-service
    // (DESIGN §4.8) — any employee may draft/submit an approval document. Only
    // operational pipelines (e.g. the completion→approval→payroll template) carry
    // a `start_policy` to constrain who may initiate them (security M2).
    Ok(json!({
        "schema_version": WORKFLOW_EXEC_SCHEMA_VERSION,
        "workflow_key": template.workflow_key,
        "object_type": "approval_document",
        "approval_template": template.key,
        "reason_enum": template.reason_enum,
        "linked_objects": template.linked_objects.iter().map(|link| {
            json!({ "kind": link.kind, "required": link.required })
        }).collect::<Vec<_>>(),
        "nodes": nodes,
        "edges": edges
    }))
}

fn validate_definition_object(value: Value) -> Result<Value, WorkflowStudioError> {
    validate_definition_for_optional_object_type(value)
}

fn validate_definition_with_expected_object_type(
    value: Value,
    expected_object_type: Option<&str>,
) -> Result<Value, WorkflowStudioError> {
    let Some(object) = value.as_object() else {
        return Err(WorkflowStudioError::validation(
            "definition must be a JSON object",
        ));
    };
    let schema_version = object.get("schema_version").and_then(Value::as_str);
    // Executable node-graph definition (M2 runtime). Validated separately; the
    // authoring/policy schema below is left byte-identical.
    if schema_version == Some(WORKFLOW_EXEC_SCHEMA_VERSION) {
        validate_execution_graph(object)?;
        return Ok(value);
    }
    if schema_version != Some(WORKFLOW_DEFINITION_SCHEMA_VERSION) {
        return Err(WorkflowStudioError::validation(format!(
            "definition schema_version must be {WORKFLOW_DEFINITION_SCHEMA_VERSION} or {WORKFLOW_EXEC_SCHEMA_VERSION}"
        )));
    }
    if object.contains_key("cedar_policy") || object.contains_key("cedar_policy_text") {
        return Err(WorkflowStudioError::validation(
            "Workflow Studio policy decisions must use policy_decision templates, not arbitrary Cedar text",
        ));
    }
    if let Some(policy_decision) = object.get("policy_decision") {
        validate_policy_decision(policy_decision)?;
    }

    let findings = validate_canonical_workflow_definition(&value, expected_object_type);
    if findings.is_empty() {
        Ok(value)
    } else {
        Err(WorkflowStudioError::validation(
            format_workflow_definition_findings(&findings),
        ))
    }
}

fn validate_canonical_workflow_definition(
    value: &Value,
    expected_object_type: Option<&str>,
) -> Vec<WorkflowSimulationFinding> {
    let mut findings = Vec::new();
    let Some(definition) = value.as_object() else {
        push_workflow_definition_finding(
            &mut findings,
            "definition_object",
            "definition must be a JSON object",
        );
        return findings;
    };

    if definition.get("schema_version").and_then(Value::as_str)
        != Some(WORKFLOW_DEFINITION_SCHEMA_VERSION)
    {
        push_workflow_definition_finding(
            &mut findings,
            "schema_version",
            "definition schema_version must be workflow.definition.v1",
        );
    }

    let metadata = definition.get("metadata").and_then(Value::as_object);
    let metadata_object_type = metadata
        .and_then(|metadata| metadata.get("object_type"))
        .and_then(Value::as_str);
    match (expected_object_type, metadata_object_type) {
        (Some(expected), Some(actual)) if actual == expected => {}
        (Some(_), Some(_)) => push_workflow_definition_finding(
            &mut findings,
            "object_type_mismatch",
            "definition metadata object_type must match the workflow object type",
        ),
        (Some(_), None) => push_workflow_definition_finding(
            &mut findings,
            "object_type_mismatch",
            "definition metadata must include object_type matching the workflow object type",
        ),
        (None, Some(_)) => {}
        (None, None) => push_workflow_definition_finding(
            &mut findings,
            "object_type_mismatch",
            "definition metadata must include object_type",
        ),
    }
    let graph_object_type = expected_object_type.or(metadata_object_type);

    let Some(graph) = definition.get("graph").and_then(Value::as_object) else {
        push_workflow_definition_finding(
            &mut findings,
            "graph_object",
            "definition graph must be a JSON object",
        );
        return findings;
    };
    let Some(nodes) = graph.get("nodes").and_then(Value::as_array) else {
        push_workflow_definition_finding(
            &mut findings,
            "nodes_array",
            "definition graph.nodes must be an array",
        );
        return findings;
    };
    let Some(edges) = graph.get("edges").and_then(Value::as_array) else {
        push_workflow_definition_finding(
            &mut findings,
            "edges_array",
            "definition graph.edges must be an array",
        );
        return findings;
    };

    let mut parsed_nodes = Vec::with_capacity(nodes.len());
    let mut ids = HashSet::new();
    let mut keys = HashSet::new();
    for node in nodes {
        let Some(node_object) = node.as_object() else {
            push_workflow_definition_finding(
                &mut findings,
                "node_object",
                "workflow nodes must be JSON objects",
            );
            continue;
        };
        let id = non_empty_string(node, "id");
        let key = non_empty_string(node, "key");
        let node_type = non_empty_string(node, "type");
        let (Some(id), Some(key), Some(node_type)) = (id, key, node_type) else {
            push_workflow_definition_finding(
                &mut findings,
                "node_identity",
                "workflow nodes require non-empty id, key, and type",
            );
            continue;
        };

        if !ids.insert(id.to_owned()) {
            push_workflow_definition_finding(
                &mut findings,
                "duplicate_node_id",
                "workflow node ids must be unique",
            );
        }
        if !keys.insert(key.to_owned()) {
            push_workflow_definition_finding(
                &mut findings,
                "duplicate_node_key",
                "workflow node keys must be unique",
            );
        }
        if !is_allowed_workflow_node_type(node_type) {
            push_workflow_definition_finding(
                &mut findings,
                "unknown_node_type",
                "workflow node type is not server-owned or allowlisted",
            );
        }

        let input_ports = parse_workflow_ports(node, "input_ports", "input", &mut findings);
        let output_ports = parse_workflow_ports(node, "output_ports", "output", &mut findings);
        validate_node_config(node_object, node_type, graph_object_type, &mut findings);
        parsed_nodes.push(WorkflowGraphNodeInfo {
            id: id.to_owned(),
            node_type: node_type.to_owned(),
            input_ports,
            output_ports,
        });
    }

    let trigger_count = parsed_nodes
        .iter()
        .filter(|node| node.node_type == "trigger.form_submission")
        .count();
    if trigger_count == 0 {
        push_workflow_definition_finding(
            &mut findings,
            "missing_trigger",
            "exactly one workflow trigger is required",
        );
    } else if trigger_count > 1 {
        push_workflow_definition_finding(
            &mut findings,
            "too_many_triggers",
            "only one workflow trigger is allowed",
        );
    }
    if !parsed_nodes
        .iter()
        .any(|node| node.node_type == "end.state")
    {
        push_workflow_definition_finding(
            &mut findings,
            "missing_terminal",
            "at least one terminal end.state node is required",
        );
    }

    validate_workflow_edges(&parsed_nodes, edges, &mut findings);
    findings
}

fn validate_workflow_edges(
    nodes: &[WorkflowGraphNodeInfo],
    edges: &[Value],
    findings: &mut Vec<WorkflowSimulationFinding>,
) {
    let nodes_by_id: HashMap<&str, &WorkflowGraphNodeInfo> =
        nodes.iter().map(|node| (node.id.as_str(), node)).collect();
    let mut inbound_counts: HashMap<(String, String), usize> = HashMap::new();
    let mut outbound_counts: HashMap<(String, String), usize> = HashMap::new();
    let mut adjacency: HashMap<String, Vec<String>> = HashMap::new();

    for edge in edges {
        if !edge.is_object() {
            push_workflow_definition_finding(
                findings,
                "edge_object",
                "workflow edges must be JSON objects",
            );
            continue;
        }
        let from_node_id = non_empty_string(edge, "from_node_id");
        let from_port = non_empty_string(edge, "from_port");
        let to_node_id = non_empty_string(edge, "to_node_id");
        let to_port = non_empty_string(edge, "to_port");
        let (Some(from_node_id), Some(from_port), Some(to_node_id), Some(to_port)) =
            (from_node_id, from_port, to_node_id, to_port)
        else {
            push_workflow_definition_finding(
                findings,
                "invalid_edge_endpoint",
                "edge endpoints must reference existing node ids and port keys",
            );
            continue;
        };

        let Some(from_node) = nodes_by_id.get(from_node_id) else {
            push_workflow_definition_finding(
                findings,
                "invalid_edge_endpoint",
                "edge source node is not present in the graph",
            );
            continue;
        };
        let Some(to_node) = nodes_by_id.get(to_node_id) else {
            push_workflow_definition_finding(
                findings,
                "invalid_edge_endpoint",
                "edge target node is not present in the graph",
            );
            continue;
        };
        let source_port = from_node
            .output_ports
            .iter()
            .find(|port| port.key == from_port);
        let target_port = to_node.input_ports.iter().find(|port| port.key == to_port);
        let (Some(source_port), Some(target_port)) = (source_port, target_port) else {
            push_workflow_definition_finding(
                findings,
                "invalid_edge_endpoint",
                "edge references a missing input or output port",
            );
            continue;
        };
        if !workflow_ports_compatible(&source_port.port_type, &target_port.port_type) {
            push_workflow_definition_finding(
                findings,
                "incompatible_ports",
                "edge connects incompatible workflow port types",
            );
        }

        *outbound_counts
            .entry((from_node_id.to_owned(), from_port.to_owned()))
            .or_insert(0) += 1;
        *inbound_counts
            .entry((to_node_id.to_owned(), to_port.to_owned()))
            .or_insert(0) += 1;
        adjacency
            .entry(from_node_id.to_owned())
            .or_default()
            .push(to_node_id.to_owned());
    }

    for node in nodes {
        for port in &node.input_ports {
            let count = inbound_counts
                .get(&(node.id.clone(), port.key.clone()))
                .copied()
                .unwrap_or(0);
            if port.required && count == 0 {
                push_workflow_definition_finding(
                    findings,
                    "unconnected_input",
                    "required input ports must have an incoming edge",
                );
            }
            if port.cardinality_one && count > 1 {
                push_workflow_definition_finding(
                    findings,
                    "too_many_input_edges",
                    "single-cardinality input ports can only be connected once",
                );
            }
        }
        for port in &node.output_ports {
            let count = outbound_counts
                .get(&(node.id.clone(), port.key.clone()))
                .copied()
                .unwrap_or(0);
            if port.required && count == 0 {
                push_workflow_definition_finding(
                    findings,
                    "unconnected_output",
                    "required output ports must have an outgoing edge",
                );
            }
            if port.cardinality_one && count > 1 {
                push_workflow_definition_finding(
                    findings,
                    "too_many_output_edges",
                    "single-cardinality output ports can only be connected once",
                );
            }
        }
    }

    let trigger_nodes: Vec<&WorkflowGraphNodeInfo> = nodes
        .iter()
        .filter(|node| node.node_type == "trigger.form_submission")
        .collect();
    if let [trigger] = trigger_nodes.as_slice() {
        let reachable = reachable_workflow_node_ids(&trigger.id, &adjacency);
        for node in nodes {
            if !reachable.contains(&node.id) {
                push_workflow_definition_finding(
                    findings,
                    "unreachable_node",
                    "all workflow nodes must be reachable from the trigger",
                );
            }
        }
    }
}

fn validate_node_config(
    node: &serde_json::Map<String, Value>,
    node_type: &str,
    object_type: Option<&str>,
    findings: &mut Vec<WorkflowSimulationFinding>,
) {
    let Some(config) = node.get("config").and_then(Value::as_object) else {
        push_workflow_definition_finding(
            findings,
            "node_config",
            "workflow nodes require a config object",
        );
        return;
    };
    if config.get("type").and_then(Value::as_str) != Some(node_type) {
        push_workflow_definition_finding(
            findings,
            "config_type_mismatch",
            "node config type must match node type",
        );
    }

    match node_type {
        "trigger.form_submission" => {
            validate_trigger_form_submission_config(config, object_type, findings);
        }
        "form.input" => {
            validate_form_input_config(config, object_type, findings);
        }
        "task.approval" => {
            let fallback_role = config
                .get("assignee_rule")
                .and_then(Value::as_object)
                .and_then(|rule| rule.get("fallback_role"))
                .and_then(Value::as_str)
                .is_some_and(|value| !value.trim().is_empty());
            if !fallback_role {
                push_workflow_definition_finding(
                    findings,
                    "missing_approval_fallback",
                    "approval task nodes require a fallback role",
                );
            }
        }
        "condition.branch" => {
            validate_condition_branch_config(node, config, findings);
        }
        "action.object_update" => {
            validate_object_update_config(config, object_type, findings);
        }
        "action.notification" => {
            let connector_key = config.get("connector_key").and_then(Value::as_str);
            let action_key = config.get("action_key").and_then(Value::as_str);
            match (connector_key, action_key) {
                (Some(connector_key), Some(action_key))
                    if connector_allows(connector_key, action_key) => {}
                _ => push_workflow_definition_finding(
                    findings,
                    "connector_action_not_allowlisted",
                    "notification connector action is not in the server-owned allowlist",
                ),
            }
        }
        "action.audit_append" => {
            let has_event_key = has_non_empty_config_string(config, "event_key");
            if !has_event_key {
                push_workflow_definition_finding(
                    findings,
                    "missing_audit_event",
                    "audit append nodes require an event_key",
                );
            }
        }
        "end.state" => {
            let has_status = has_non_empty_config_string(config, "status");
            if !has_status {
                push_workflow_definition_finding(
                    findings,
                    "missing_end_status",
                    "terminal nodes require a status",
                );
            }
        }
        _ => {}
    }
}

fn validate_trigger_form_submission_config(
    config: &serde_json::Map<String, Value>,
    object_type: Option<&str>,
    findings: &mut Vec<WorkflowSimulationFinding>,
) {
    let source = config.get("source").and_then(Value::as_object);
    let source_object_type = source
        .and_then(|source| source.get("object_type"))
        .and_then(Value::as_str);
    let source_event = source
        .and_then(|source| source.get("event"))
        .and_then(Value::as_str);
    let source_scope = source
        .and_then(|source| source.get("scope"))
        .and_then(Value::as_str);

    if source_object_type.is_none()
        || source_event != Some("submitted")
        || source_scope != Some("org")
    {
        push_workflow_definition_finding(
            findings,
            "trigger_source",
            "trigger nodes require server-owned submitted/org source metadata",
        );
    }
    if let (Some(expected), Some(actual)) = (object_type, source_object_type)
        && actual != expected
    {
        push_workflow_definition_finding(
            findings,
            "object_type_mismatch",
            "trigger source object_type must match the workflow object type",
        );
    }
}

fn validate_form_input_config(
    config: &serde_json::Map<String, Value>,
    object_type: Option<&str>,
    findings: &mut Vec<WorkflowSimulationFinding>,
) {
    let Some(fields) = config.get("fields").and_then(Value::as_array) else {
        push_workflow_definition_finding(
            findings,
            "missing_form_fields",
            "form input nodes require at least one field",
        );
        return;
    };
    if fields.is_empty() {
        push_workflow_definition_finding(
            findings,
            "missing_form_fields",
            "form input nodes require at least one field",
        );
        return;
    }

    for field in fields {
        let Some(field) = field.as_object() else {
            push_workflow_definition_finding(
                findings,
                "invalid_form_field",
                "form fields must be JSON objects",
            );
            continue;
        };
        if field.get("field_type").and_then(Value::as_str) == Some("object_ref") {
            let field_object_type = field.get("object_type").and_then(Value::as_str);
            if let (Some(expected), Some(actual)) = (object_type, field_object_type)
                && actual != expected
            {
                push_workflow_definition_finding(
                    findings,
                    "object_type_mismatch",
                    "form object_ref fields must match the workflow object type",
                );
            } else if field_object_type.is_none() {
                push_workflow_definition_finding(
                    findings,
                    "object_type_mismatch",
                    "form object_ref fields require object_type",
                );
            }
        }
    }
}

fn validate_condition_branch_config(
    node: &serde_json::Map<String, Value>,
    config: &serde_json::Map<String, Value>,
    findings: &mut Vec<WorkflowSimulationFinding>,
) {
    validate_condition_expression(config.get("expression"), findings);

    let declared_branch_ports = declared_branch_output_ports(node);
    let Some(branches) = config.get("branches").and_then(Value::as_array) else {
        push_workflow_definition_finding(
            findings,
            "missing_condition_branches",
            "condition nodes require at least two branches",
        );
        return;
    };
    if branches.len() < 2 {
        push_workflow_definition_finding(
            findings,
            "missing_condition_branches",
            "condition nodes require at least two branches",
        );
    }

    let mut branch_ports = HashSet::new();
    for branch in branches {
        let Some(branch) = branch.as_object() else {
            push_workflow_definition_finding(
                findings,
                "invalid_condition_branch",
                "condition branches must be JSON objects",
            );
            continue;
        };
        let port = branch.get("port").and_then(Value::as_str);
        let label = branch.get("label").and_then(Value::as_str);
        let when = branch.get("when").and_then(Value::as_str);
        let valid = port.is_some_and(|port| {
            !port.trim().is_empty()
                && branch_ports.insert(port.to_owned())
                && declared_branch_ports.contains(port)
        }) && label.is_some_and(|label| !label.trim().is_empty())
            && matches!(when, Some("true" | "false"));
        if !valid {
            push_workflow_definition_finding(
                findings,
                "invalid_condition_branch",
                "condition branch ports must be declared server-owned true/false output ports",
            );
        }
    }

    if declared_branch_ports.len() != branch_ports.len()
        || declared_branch_ports
            .iter()
            .any(|port| !branch_ports.contains(port))
    {
        push_workflow_definition_finding(
            findings,
            "invalid_condition_branch",
            "condition branch config must declare exactly the server-owned branch output ports",
        );
    }

    let default_port = config.get("default_port").and_then(Value::as_str);
    if default_port.is_none_or(|port| {
        port.trim().is_empty()
            || !branch_ports.contains(port)
            || !declared_branch_ports.contains(port)
    }) {
        push_workflow_definition_finding(
            findings,
            "invalid_condition_default",
            "condition default_port must reference a declared branch port",
        );
    }
}

fn validate_condition_expression(
    expression: Option<&Value>,
    findings: &mut Vec<WorkflowSimulationFinding>,
) {
    let Some(expression) = expression.and_then(Value::as_object) else {
        push_workflow_definition_finding(
            findings,
            "invalid_condition_expression",
            "condition expression must be a server-owned expression object",
        );
        return;
    };
    let op = expression.get("op").and_then(Value::as_str);
    let left_ref = expression
        .get("left")
        .and_then(Value::as_object)
        .and_then(|left| left.get("ref"))
        .and_then(Value::as_str);
    let right = expression.get("right");
    if !matches!(
        op,
        Some("equals" | "not_equals" | "in" | "not_in" | "exists")
    ) || left_ref != Some("approval.result")
        || right.is_none_or(|right| !(right.is_string() || right.is_boolean() || right.is_number()))
    {
        push_workflow_definition_finding(
            findings,
            "invalid_condition_expression",
            "condition expression must use the server-owned approval.result grammar",
        );
    }
}

fn validate_object_update_config(
    config: &serde_json::Map<String, Value>,
    object_type: Option<&str>,
    findings: &mut Vec<WorkflowSimulationFinding>,
) {
    let action_id = config.get("action_id").and_then(Value::as_str);
    let requires_policy = config.get("requires_policy").and_then(Value::as_str);
    if let Some(object_type) = object_type {
        let expected_action = format!("{object_type}.update_status");
        if action_id != Some(expected_action.as_str())
            || requires_policy != Some(expected_action.as_str())
        {
            push_workflow_definition_finding(
                findings,
                "object_action_not_allowlisted",
                "object update action and policy must match the server-owned workflow action allowlist",
            );
        }
    } else if action_id.is_none() || requires_policy.is_none() {
        push_workflow_definition_finding(
            findings,
            "object_action_not_allowlisted",
            "object update action and policy are required",
        );
    }

    let target_from = config
        .get("target")
        .and_then(Value::as_object)
        .and_then(|target| target.get("from"))
        .and_then(Value::as_str);
    if target_from != Some("trigger.object_ref") {
        push_workflow_definition_finding(
            findings,
            "invalid_object_update_target",
            "object update target must be the server-owned trigger.object_ref handle",
        );
    }

    validate_object_update_input(config.get("input"), findings);
}

fn validate_object_update_input(
    input: Option<&Value>,
    findings: &mut Vec<WorkflowSimulationFinding>,
) {
    let Some(input) = input.and_then(Value::as_object) else {
        push_workflow_definition_finding(
            findings,
            "invalid_object_update_input",
            "object update input must be a server-owned status update mapping",
        );
        return;
    };
    let status = input.get("status").and_then(Value::as_str);
    let updated_by = input_value_from_handle(input.get("updated_by"));
    let updated_at = input_value_from_handle(input.get("updated_at"));
    if input.len() != 3
        || !matches!(
            status,
            Some("approved" | "rejected" | "cancelled" | "completed" | "failed")
        )
        || updated_by != Some("approval.actor_id")
        || updated_at != Some("system.now")
    {
        push_workflow_definition_finding(
            findings,
            "invalid_object_update_input",
            "object update input may only map status plus approval.actor_id/system.now handles",
        );
    }
}

fn input_value_from_handle(value: Option<&Value>) -> Option<&str> {
    value
        .and_then(Value::as_object)
        .and_then(|object| object.get("from"))
        .and_then(Value::as_str)
}

fn declared_branch_output_ports(node: &serde_json::Map<String, Value>) -> HashSet<String> {
    node.get("output_ports")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|port| {
            let port = port.as_object()?;
            let key = port.get("key")?.as_str()?;
            let direction = port.get("direction").and_then(Value::as_str);
            let port_type = port.get("type").and_then(Value::as_str);
            (direction == Some("output") && port_type == Some("flow.branch"))
                .then(|| key.to_owned())
        })
        .collect()
}

fn parse_workflow_ports(
    node: &Value,
    field: &'static str,
    direction: &'static str,
    findings: &mut Vec<WorkflowSimulationFinding>,
) -> Vec<WorkflowPortInfo> {
    let Some(ports) = node.get(field).and_then(Value::as_array) else {
        push_workflow_definition_finding(
            findings,
            "ports_array",
            "workflow node ports must be arrays",
        );
        return Vec::new();
    };
    let mut parsed = Vec::with_capacity(ports.len());
    for port in ports {
        let Some(port_object) = port.as_object() else {
            push_workflow_definition_finding(
                findings,
                "invalid_port",
                "workflow ports must be JSON objects",
            );
            continue;
        };
        let key = non_empty_string(port, "key");
        let port_direction = port_object.get("direction").and_then(Value::as_str);
        let port_type = port_object.get("type").and_then(Value::as_str);
        let cardinality = port_object.get("cardinality").and_then(Value::as_str);
        let (Some(key), Some(port_type), Some(cardinality)) = (key, port_type, cardinality) else {
            push_workflow_definition_finding(
                findings,
                "invalid_port",
                "workflow ports require key, direction, allowed type, and cardinality",
            );
            continue;
        };
        if port_direction != Some(direction)
            || !is_allowed_workflow_port_type(port_type)
            || !matches!(cardinality, "one" | "many")
        {
            push_workflow_definition_finding(
                findings,
                "invalid_port",
                "workflow ports require key, direction, allowed type, and cardinality",
            );
            continue;
        }
        parsed.push(WorkflowPortInfo {
            key: key.to_owned(),
            port_type: port_type.to_owned(),
            required: port_object
                .get("required")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            cardinality_one: cardinality == "one",
        });
    }
    parsed
}

fn has_non_empty_config_string(config: &serde_json::Map<String, Value>, field: &str) -> bool {
    config
        .get(field)
        .and_then(Value::as_str)
        .is_some_and(|value| !value.trim().is_empty())
}

fn non_empty_string<'a>(value: &'a Value, field: &str) -> Option<&'a str> {
    value
        .get(field)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
}

fn is_allowed_workflow_node_type(node_type: &str) -> bool {
    matches!(
        node_type,
        "trigger.form_submission"
            | "form.input"
            | "task.approval"
            | "condition.branch"
            | "action.object_update"
            | "action.notification"
            | "action.audit_append"
            | "end.state"
    )
}

fn is_allowed_workflow_port_type(port_type: &str) -> bool {
    matches!(
        port_type,
        "flow.start"
            | "flow.next"
            | "flow.branch"
            | "flow.terminal"
            | "data.form_payload"
            | "data.approval_result"
            | "data.object_ref"
            | "data.audit_event"
    )
}

fn workflow_ports_compatible(output: &str, input: &str) -> bool {
    output == input
        || (input == "flow.terminal"
            && matches!(output, "flow.next" | "flow.branch" | "flow.terminal"))
        || (input == "flow.next" && output == "flow.branch")
}

fn reachable_workflow_node_ids(
    start_node_id: &str,
    adjacency: &HashMap<String, Vec<String>>,
) -> HashSet<String> {
    let mut reachable = HashSet::new();
    let mut queue = VecDeque::from([start_node_id.to_owned()]);
    while let Some(node_id) = queue.pop_front() {
        if !reachable.insert(node_id.clone()) {
            continue;
        }
        if let Some(next_nodes) = adjacency.get(&node_id) {
            queue.extend(next_nodes.iter().cloned());
        }
    }
    reachable
}

fn push_workflow_definition_finding(
    findings: &mut Vec<WorkflowSimulationFinding>,
    code: &'static str,
    message: &'static str,
) {
    findings.push(WorkflowSimulationFinding {
        severity: "blocker".to_owned(),
        code: code.to_owned(),
        message: message.to_owned(),
    });
}

fn format_workflow_definition_findings(findings: &[WorkflowSimulationFinding]) -> String {
    let mut parts: Vec<String> = findings
        .iter()
        .take(8)
        .map(|finding| format!("{}: {}", finding.code, finding.message))
        .collect();
    if findings.len() > parts.len() {
        parts.push(format!(
            "{} more validation blockers",
            findings.len() - parts.len()
        ));
    }
    format!(
        "workflow.definition.v1 graph validation failed: {}",
        parts.join("; ")
    )
}

/// Validate a `wf.exec.v1` executable definition's node graph (design §3/§template).
/// Every node needs a stable `node_key` + a known `node_type`; a `job` node must fan
/// out through an allowlisted connector, so the completion→approval→payroll template
/// cannot publish without the `internal.jobs` connector.
fn validate_execution_graph(
    object: &serde_json::Map<String, Value>,
) -> Result<(), WorkflowStudioError> {
    let nodes = object
        .get("nodes")
        .and_then(Value::as_array)
        .filter(|nodes| !nodes.is_empty())
        .ok_or_else(|| {
            WorkflowStudioError::validation("execution definition requires a non-empty nodes array")
        })?;
    let mut has_job = false;
    let mut condition_keys: Vec<String> = Vec::new();
    for node in nodes {
        let node = node.as_object().ok_or_else(|| {
            WorkflowStudioError::validation("execution nodes must be JSON objects")
        })?;
        let node_key = required_string(node, "node_key")?.to_owned();
        match required_string(node, "node_type")? {
            "object_gate" | "object_mutation" => {}
            "human_task" => {
                required_string(node, "assignee_role_key")?;
                // Fail-closed authorization boundary (security H1): a human task
                // MUST declare the policy that gates who may claim/decide it. An
                // omitted `required_policy` is an authoring-time 422, never a task
                // any org member could act on.
                required_string(node, "required_policy")?;
            }
            "job" => {
                has_job = true;
                let connector_key = required_string(node, "connector_key")?;
                let action_key = required_string(node, "action_key")?;
                if !connector_allows(connector_key, action_key) {
                    return Err(WorkflowStudioError::validation(
                        "execution node job action is not in the Workflow Studio connector allowlist",
                    ));
                }
            }
            "condition" => {
                // A condition node carries a small deterministic predicate; parse
                // it fail-closed so a malformed rule cannot publish (the runtime
                // walker parses the same shape).
                let predicate = node.get("predicate").ok_or_else(|| {
                    WorkflowStudioError::validation("condition node requires a predicate")
                })?;
                console_workflow_runtime::Predicate::parse(predicate)
                    .map_err(WorkflowStudioError::from)?;
                condition_keys.push(node_key);
            }
            "guard.checklist_attestation" => {
                validate_execution_checklist_guard(node)?;
            }
            "guard.four_eyes_peer_review" => {
                validate_execution_four_eyes_guard(node)?;
            }
            "guard.segregation_of_duties" => {
                validate_execution_sod_guard(node)?;
            }
            "guard.egress_policy" => {
                validate_execution_egress_guard(node)?;
            }
            _ => {
                return Err(WorkflowStudioError::validation(
                    "unsupported execution node_type",
                ));
            }
        }
    }

    // Every condition node needs BOTH a true and a false outgoing branch edge,
    // or a run could dead-end at it (fail-closed authoring, not a runtime error).
    let edges = object.get("edges").and_then(Value::as_array);
    for key in &condition_keys {
        let has_branch = |want: &str| {
            edges.is_some_and(|edges| {
                edges.iter().any(|edge| {
                    edge.get("from").and_then(Value::as_str) == Some(key.as_str())
                        && edge.get("when").and_then(Value::as_str) == Some(want)
                })
            })
        };
        if !has_branch("true") || !has_branch("false") {
            return Err(WorkflowStudioError::validation(format!(
                "condition node {key:?} requires both a \"true\" and a \"false\" branch edge"
            )));
        }
    }

    // Optional object-kind chain: the ontology kinds this definition's nodes
    // touch (dynamics↔ontology). Shape-validated here; existence in object_types
    // is checked against the DB at create/update time (validate_object_kinds).
    validate_object_kinds_shape(object)?;
    // Fail-closed start authority (Engine-Gen follow-up): a graph containing a `job`
    // node drives a system connector (e.g. the completion→approval→payroll pipeline's
    // payroll_draft), so it MUST declare a top-level `start_policy` constraining WHO
    // may initiate a run. Runtime start-authz already gates job pipelines on this
    // policy; authoring now refuses a job-bearing graph without it (422) so no
    // author-composed definition can slip a job into a self-service, all-employee
    // start. Job-free approval graphs stay deliberately self-service and unaffected.
    if has_job
        && object
            .get("start_policy")
            .and_then(Value::as_str)
            .filter(|policy| !policy.trim().is_empty())
            .is_none()
    {
        return Err(WorkflowStudioError::validation(
            "execution definition with a job node requires a non-empty start_policy",
        ));
    }
    Ok(())
}

fn validate_execution_checklist_guard(
    node: &serde_json::Map<String, Value>,
) -> Result<(), WorkflowStudioError> {
    let config = execution_guard_config(node)?;
    ensure_allowed_guard_fields(
        config,
        &[
            "label",
            "required_policy",
            "audit_event_key",
            "assignee_role_key",
            "items",
            "approve_requires_all_required",
            "reject_requires_memo",
            "step_up_required",
            "passkey_purpose",
            "due_after",
            "redaction",
            "on_missing_fact",
        ],
    )?;
    required_string(config, "label")?;
    validate_optional_feature_key(config.get("required_policy"))?;
    validate_optional_audit_event_key(config.get("audit_event_key"))?;
    let items = required_array(config, "items")?;
    if items.is_empty() || items.len() > 50 {
        return Err(WorkflowStudioError::validation(
            "checklist guardrail requires 1..=50 items",
        ));
    }
    for item in items {
        let item = item.as_object().ok_or_else(|| {
            WorkflowStudioError::validation("checklist guardrail items must be objects")
        })?;
        ensure_allowed_guard_fields(
            item,
            &[
                "key",
                "label",
                "kind",
                "required",
                "min_count",
                "source_ref",
            ],
        )?;
        required_string(item, "key")?;
        required_string(item, "label")?;
        match required_string(item, "kind")? {
            "checkbox" | "text" | "evidence_ref" | "object_ref" | "policy_ack" => {}
            _ => {
                return Err(WorkflowStudioError::validation(
                    "checklist guardrail item kind is not server-owned",
                ));
            }
        }
    }
    Ok(())
}

fn validate_execution_four_eyes_guard(
    node: &serde_json::Map<String, Value>,
) -> Result<(), WorkflowStudioError> {
    let config = execution_guard_config(node)?;
    ensure_allowed_guard_fields(
        config,
        &[
            "label",
            "required_policy",
            "audit_event_key",
            "assignee_role_key",
            "subject_actor_refs",
            "min_reviewers",
            "forbid_same_actor",
            "allow_org_lead_exemption",
            "allow_super_admin_exemption",
            "exemption_requires_memo",
            "step_up_required",
            "reject_requires_memo",
            "passkey_purpose",
            "due_after",
            "redaction",
            "on_missing_fact",
        ],
    )?;
    required_string(config, "label")?;
    validate_optional_feature_key(config.get("required_policy"))?;
    validate_optional_audit_event_key(config.get("audit_event_key"))?;
    let subject_actor_refs = required_array(config, "subject_actor_refs")?;
    if subject_actor_refs.is_empty()
        || subject_actor_refs
            .iter()
            .any(|value| value.as_str().is_none_or(str::is_empty))
    {
        return Err(WorkflowStudioError::validation(
            "four-eyes guardrail requires non-empty subject_actor_refs",
        ));
    }
    if let Some(min_reviewers) = config.get("min_reviewers").and_then(Value::as_u64)
        && !(1..=4).contains(&min_reviewers)
    {
        return Err(WorkflowStudioError::validation(
            "four-eyes guardrail min_reviewers must be 1..=4",
        ));
    }
    Ok(())
}

fn validate_execution_sod_guard(
    node: &serde_json::Map<String, Value>,
) -> Result<(), WorkflowStudioError> {
    let config = execution_guard_config(node)?;
    ensure_allowed_guard_fields(
        config,
        &[
            "label",
            "policy_key",
            "audit_event_key",
            "actor_under_test_ref",
            "blocked_actor_refs",
            "blocked_role_refs",
            "scope",
            "mode",
            "exemptions",
            "exemption_requires_memo",
            "on_missing_fact",
        ],
    )?;
    required_string(config, "label")?;
    required_string(config, "policy_key")?;
    validate_optional_audit_event_key(config.get("audit_event_key"))?;
    let blocked_actor_refs = required_array(config, "blocked_actor_refs")?;
    if blocked_actor_refs.is_empty()
        || blocked_actor_refs
            .iter()
            .any(|value| value.as_str().is_none_or(str::is_empty))
    {
        return Err(WorkflowStudioError::validation(
            "SoD guardrail requires non-empty blocked_actor_refs",
        ));
    }
    if let Some(mode) = config.get("mode").and_then(Value::as_str)
        && !matches!(mode, "hard_block" | "allow_with_governance_finding")
    {
        return Err(WorkflowStudioError::validation(
            "SoD guardrail mode is not server-owned",
        ));
    }
    Ok(())
}

fn validate_execution_egress_guard(
    node: &serde_json::Map<String, Value>,
) -> Result<(), WorkflowStudioError> {
    let config = execution_guard_config(node)?;
    ensure_allowed_guard_fields(
        config,
        &[
            "label",
            "egress_kind",
            "channel",
            "required_policy",
            "audit_event_key",
            "manual_review_role_key",
            "external_recipient_policy",
            "step_up_required",
            "passkey_purpose",
            "classification_ref",
            "data_classes",
            "lifecycle_requirements",
            "on_missing_fact",
        ],
    )?;
    required_string(config, "label")?;
    let egress_kind = required_string(config, "egress_kind")?;
    if !matches!(
        egress_kind,
        "mail" | "export" | "webhook" | "job" | "document_download"
    ) {
        return Err(WorkflowStudioError::validation(
            "egress guardrail kind is not server-owned",
        ));
    }
    required_string(config, "channel")?;
    validate_optional_feature_key(config.get("required_policy"))?;
    validate_optional_audit_event_key(config.get("audit_event_key"))?;
    if matches!(egress_kind, "mail" | "export" | "document_download") {
        required_string(config, "classification_ref")?;
    }
    if let Some(policy) = config
        .get("external_recipient_policy")
        .and_then(Value::as_str)
        && !matches!(policy, "block" | "allow_if_approved" | "manual_review")
    {
        return Err(WorkflowStudioError::validation(
            "egress guardrail external_recipient_policy is not server-owned",
        ));
    }
    Ok(())
}

fn execution_guard_config(
    node: &serde_json::Map<String, Value>,
) -> Result<&serde_json::Map<String, Value>, WorkflowStudioError> {
    if let Some(config) = node.get("config") {
        for key in node.keys() {
            if !matches!(key.as_str(), "node_key" | "node_type" | "config") {
                return Err(WorkflowStudioError::validation(
                    "guardrail execution node contains an unsupported top-level field",
                ));
            }
        }
        return config.as_object().ok_or_else(|| {
            WorkflowStudioError::validation("guardrail execution node config must be an object")
        });
    }
    Ok(node)
}

fn ensure_allowed_guard_fields(
    config: &serde_json::Map<String, Value>,
    allowed: &[&str],
) -> Result<(), WorkflowStudioError> {
    let allowed: HashSet<&str> = allowed.iter().copied().collect();
    for key in config.keys() {
        if matches!(key.as_str(), "node_key" | "node_type" | "config") {
            continue;
        }
        if !allowed.contains(key.as_str()) {
            return Err(WorkflowStudioError::validation(
                "guardrail execution config contains an unsupported field",
            ));
        }
    }
    Ok(())
}

fn required_array<'a>(
    object: &'a serde_json::Map<String, Value>,
    key: &'static str,
) -> Result<&'a Vec<Value>, WorkflowStudioError> {
    object.get(key).and_then(Value::as_array).ok_or_else(|| {
        WorkflowStudioError::validation(format!("guardrail config {key} array is required"))
    })
}

fn validate_optional_feature_key(value: Option<&Value>) -> Result<(), WorkflowStudioError> {
    if let Some(value) = value {
        let policy = value.as_str().ok_or_else(|| {
            WorkflowStudioError::validation("guardrail required_policy must be a Feature key")
        })?;
        Feature::from_str(policy).map_err(WorkflowStudioError::from)?;
    }
    Ok(())
}

fn validate_optional_audit_event_key(value: Option<&Value>) -> Result<(), WorkflowStudioError> {
    if let Some(value) = value {
        let action = value.as_str().ok_or_else(|| {
            WorkflowStudioError::validation("guardrail audit_event_key must be a string")
        })?;
        AuditAction::new(action).map_err(WorkflowStudioError::from)?;
        if !action.starts_with("workflow_guardrail.") {
            return Err(WorkflowStudioError::validation(
                "guardrail audit_event_key must use the workflow_guardrail namespace",
            ));
        }
    }
    Ok(())
}

/// The `object_kinds` chain declared on a definition (the ontology kinds its
/// nodes touch). Empty when unset.
fn definition_object_kinds(definition: &Value) -> Vec<String> {
    definition
        .get("object_kinds")
        .and_then(Value::as_array)
        .map(|kinds| {
            kinds
                .iter()
                .filter_map(|kind| kind.as_str().map(ToOwned::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

/// Shape-check the optional `object_kinds` field: an array of snake_case kind
/// slugs (same shape as an object_types.kind). Existence is enforced separately
/// against the DB.
fn validate_object_kinds_shape(
    object: &serde_json::Map<String, Value>,
) -> Result<(), WorkflowStudioError> {
    let Some(kinds) = object.get("object_kinds") else {
        return Ok(());
    };
    let kinds = kinds.as_array().ok_or_else(|| {
        WorkflowStudioError::validation("object_kinds must be an array of kind slugs")
    })?;
    for kind in kinds {
        let slug = kind.as_str().ok_or_else(|| {
            WorkflowStudioError::validation("object_kinds entries must be strings")
        })?;
        if !is_object_kind_slug(slug) {
            return Err(WorkflowStudioError::validation(format!(
                "object_kinds entry {slug:?} is not a valid kind slug"
            )));
        }
    }
    Ok(())
}

/// Mirror of the object_types.kind CHECK regex `^[a-z][a-z0-9_]{1,63}$`.
fn is_object_kind_slug(slug: &str) -> bool {
    let mut chars = slug.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    first.is_ascii_lowercase()
        && (2..=64).contains(&slug.len())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

/// Reject any `object_kinds` slug that is not a registered `object_types.kind`
/// (dynamics↔ontology: a rule cannot claim to touch a kind that does not exist).
/// Runs inside the caller's tenant transaction; object_types is a global table.
async fn validate_object_kinds_exist(
    tx: &mut Transaction<'_, Postgres>,
    definition: &Value,
) -> Result<(), WorkflowStudioError> {
    let kinds = definition_object_kinds(definition);
    for kind in kinds {
        let exists: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM object_types WHERE kind = $1)")
                .bind(&kind)
                .fetch_one(tx.as_mut())
                .await?;
        if !exists {
            return Err(WorkflowStudioError::validation(format!(
                "object_kinds entry {kind:?} is not a registered object type"
            )));
        }
    }
    Ok(())
}

fn validate_policy_decision(value: &Value) -> Result<(), WorkflowStudioError> {
    let object = value
        .as_object()
        .ok_or_else(|| WorkflowStudioError::validation("policy_decision must be a JSON object"))?;
    let template_key = required_string(object, "template_key")?;
    if template_key != POLICY_TEMPLATE_EQUIPMENT_LOCATION_ACCESS {
        Err(WorkflowStudioError::validation(
            "only equipment_location_access policy_decision is supported in this slice",
        ))
    } else if required_string(object, "effect")? != "allow" {
        Err(WorkflowStudioError::validation(
            "policy_decision effect must be allow",
        ))
    } else if required_string(object, "action")? != POLICY_ACTION_START_WORK_ORDER {
        Err(WorkflowStudioError::validation(format!(
            "policy_decision action must be {POLICY_ACTION_START_WORK_ORDER}"
        )))
    } else {
        validate_policy_resource(required_object(object, "resource")?)?;
        validate_policy_context(required_object(object, "context")?)?;
        validate_policy_scope(required_object(object, "scope")?)?;
        validate_policy_requirements(required_object(object, "requirements")?)?;
        Ok(())
    }
}

fn validate_policy_resource(
    resource: &serde_json::Map<String, Value>,
) -> Result<(), WorkflowStudioError> {
    if required_string(resource, "type")? != "equipment" {
        return Err(WorkflowStudioError::validation(
            "policy_decision resource.type must be equipment",
        ));
    }
    required_string(resource, "id")?;
    Ok(())
}

fn validate_policy_context(
    context: &serde_json::Map<String, Value>,
) -> Result<(), WorkflowStudioError> {
    for key in ["org_id", "location_id", "subject_role"] {
        required_string(context, key)?;
    }
    if context
        .get("passkey_step_up_satisfied")
        .and_then(Value::as_bool)
        != Some(true)
    {
        return Err(WorkflowStudioError::validation(
            "policy_decision context.passkey_step_up_satisfied must be true",
        ));
    }
    Ok(())
}

fn validate_policy_scope(
    scope: &serde_json::Map<String, Value>,
) -> Result<(), WorkflowStudioError> {
    for key in ["org_id", "location_id"] {
        required_string(scope, key)?;
    }
    Ok(())
}

fn validate_policy_requirements(
    requirements: &serde_json::Map<String, Value>,
) -> Result<(), WorkflowStudioError> {
    if requirements.get("passkey_step_up").and_then(Value::as_bool) != Some(true) {
        return Err(WorkflowStudioError::validation(
            "policy_decision requirements.passkey_step_up must be true",
        ));
    }
    required_string(requirements, "audit_event")?;
    Ok(())
}

fn required_object<'a>(
    object: &'a serde_json::Map<String, Value>,
    key: &'static str,
) -> Result<&'a serde_json::Map<String, Value>, WorkflowStudioError> {
    object.get(key).and_then(Value::as_object).ok_or_else(|| {
        WorkflowStudioError::validation(format!("policy_decision {key} is required"))
    })
}

fn required_string<'a>(
    object: &'a serde_json::Map<String, Value>,
    key: &'static str,
) -> Result<&'a str, WorkflowStudioError> {
    object
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            WorkflowStudioError::validation(format!("policy_decision {key} is required"))
        })
}

fn validate_action_allowlist(actions: &[Value]) -> Result<(), WorkflowStudioError> {
    for action in actions {
        let connector_key = action
            .get("connector_key")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                WorkflowStudioError::validation("action_allowlist entries require connector_key")
            })?;
        let action_key = action
            .get("action_key")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                WorkflowStudioError::validation("action_allowlist entries require action_key")
            })?;
        if !connector_allows(connector_key, action_key) {
            return Err(WorkflowStudioError::validation(
                "action_allowlist entry is not in the Workflow Studio connector allowlist",
            ));
        }
    }
    Ok(())
}

fn validate_notification_rules(rules: &[Value]) -> Result<(), WorkflowStudioError> {
    for rule in rules {
        if !rule.is_object() {
            return Err(WorkflowStudioError::validation(
                "notification_rules entries must be JSON objects",
            ));
        }
        if let Some(action_key) = rule.get("action_key").and_then(Value::as_str)
            && !connector_allows("internal.notifications", action_key)
            && !connector_allows("internal.mail", action_key)
        {
            return Err(WorkflowStudioError::validation(
                "notification action is not allowlisted",
            ));
        }
    }
    Ok(())
}

fn connector_allows(connector_key: &str, action_key: &str) -> bool {
    ALLOWED_CONNECTORS.iter().any(|connector| {
        connector.connector_key == connector_key && connector.action_keys.contains(&action_key)
    })
}

fn validation_findings(row: &WorkflowVersionRow) -> Vec<WorkflowSimulationFinding> {
    let mut findings = Vec::new();
    if row.required_approval_line && row.approval_line.is_empty() {
        findings.push(WorkflowSimulationFinding {
            severity: "blocker".to_owned(),
            code: "missing_approval_line".to_owned(),
            message: "required approval line is empty".to_owned(),
        });
    }
    if row.required_payment_line && row.payment_line.is_empty() {
        findings.push(WorkflowSimulationFinding {
            severity: "blocker".to_owned(),
            code: "missing_payment_line".to_owned(),
            message: "required payment line is empty".to_owned(),
        });
    }
    if row.action_allowlist.is_empty() {
        findings.push(WorkflowSimulationFinding {
            severity: "warning".to_owned(),
            code: "empty_action_allowlist".to_owned(),
            message: "no connector actions are enabled".to_owned(),
        });
    }
    if let Err(error) = validate_action_allowlist(&row.action_allowlist) {
        findings.push(WorkflowSimulationFinding {
            severity: "blocker".to_owned(),
            code: "invalid_action_allowlist".to_owned(),
            message: error.message,
        });
    }
    if let Err(error) = validate_notification_rules(&row.notification_rules) {
        findings.push(WorkflowSimulationFinding {
            severity: "blocker".to_owned(),
            code: "invalid_notification_rules".to_owned(),
            message: error.message,
        });
    }
    if row.definition.get("schema_version").and_then(Value::as_str)
        == Some(WORKFLOW_EXEC_SCHEMA_VERSION)
    {
        if let Err(error) = validate_definition_object(row.definition.clone()) {
            findings.push(WorkflowSimulationFinding {
                severity: "blocker".to_owned(),
                code: "invalid_execution_definition".to_owned(),
                message: error.message,
            });
        }
    } else {
        findings.extend(validate_canonical_workflow_definition(
            &row.definition,
            Some(&row.object_type),
        ));
        if row.definition.get("cedar_policy").is_some()
            || row.definition.get("cedar_policy_text").is_some()
        {
            findings.push(WorkflowSimulationFinding {
                severity: "blocker".to_owned(),
                code: "arbitrary_policy_text".to_owned(),
                message: "Workflow Studio policy decisions must use policy_decision templates, not arbitrary Cedar text".to_owned(),
            });
        }
        if let Some(policy_decision) = row.definition.get("policy_decision")
            && let Err(error) = validate_policy_decision(policy_decision)
        {
            findings.push(WorkflowSimulationFinding {
                severity: "blocker".to_owned(),
                code: "invalid_policy_decision".to_owned(),
                message: error.message,
            });
        }
    }
    findings
}

fn validate_publishable(row: &WorkflowVersionRow) -> Vec<WorkflowSimulationFinding> {
    validation_findings(row)
        .into_iter()
        .filter(|finding| finding.severity == "blocker")
        .collect()
}

#[cfg(test)]
fn format_publish_findings(findings: Vec<WorkflowSimulationFinding>) -> String {
    findings
        .into_iter()
        .map(|finding| format!("{}: {}", finding.code, finding.message))
        .collect::<Vec<_>>()
        .join("; ")
}

fn simulation_for(row: &WorkflowVersionRow) -> WorkflowSimulationResponse {
    let mut findings = validation_findings(row);
    if let Some(finding) = policy_decision_metadata_finding(&row.definition) {
        findings.push(finding);
    }
    let decision = if findings.iter().any(|finding| finding.severity == "blocker") {
        "blocked"
    } else {
        "ready"
    };
    WorkflowSimulationResponse {
        decision: decision.to_owned(),
        findings,
        simulated_path: None,
    }
}

/// Walk a `wf.exec.v1` definition's graph against `context` to report the branch
/// actually taken (the ordered node keys that would execute). `None` for a
/// non-executable definition; a walk error becomes a blocker finding on `result`.
fn attach_simulated_path(
    result: &mut WorkflowSimulationResponse,
    definition: &Value,
    context: &Value,
) {
    let is_exec = definition.get("schema_version").and_then(Value::as_str)
        == Some(WORKFLOW_EXEC_SCHEMA_VERSION);
    if !is_exec {
        return;
    }
    match ExecGraph::parse(definition)
        .and_then(|graph| console_workflow_runtime::simulate_path(&graph, context))
    {
        Ok(path) => result.simulated_path = Some(path),
        Err(error) => {
            result.decision = "blocked".to_owned();
            result.findings.push(WorkflowSimulationFinding {
                severity: "blocker".to_owned(),
                code: "unwalkable_graph".to_owned(),
                message: error.to_string(),
            });
        }
    }
}

fn policy_decision_metadata_finding(definition: &Value) -> Option<WorkflowSimulationFinding> {
    if validate_definition_object(definition.clone()).is_err() {
        return None;
    }
    let decision = definition.get("policy_decision")?;
    let object = decision.as_object()?;
    let resource = object.get("resource")?.as_object()?;
    let scope = object.get("scope")?.as_object()?;
    Some(WorkflowSimulationFinding {
        severity: "info".to_owned(),
        code: "policy_decision_metadata".to_owned(),
        message: format!(
            "Cedar/PBAC tuple schema={} template={} effect={} action={} resource={}:{} context=org_id,location_id,subject_role,passkey_step_up_satisfied scope={}/{}",
            WORKFLOW_DEFINITION_SCHEMA_VERSION,
            object.get("template_key")?.as_str()?,
            object.get("effect")?.as_str()?,
            object.get("action")?.as_str()?,
            resource.get("type")?.as_str()?,
            resource.get("id")?.as_str()?,
            scope.get("org_id")?.as_str()?,
            scope.get("location_id")?.as_str()?
        ),
    })
}

// ===========================================================================
// BE-AUTO slice 1 — trigger bindings + cron schedules authoring (gaps 8 & 9).
// ===========================================================================

#[derive(Debug, Serialize)]
struct TriggerBindingResponse {
    id: Uuid,
    definition_id: Uuid,
    trigger_type: String,
    event_key: String,
    /// The ontology object kind this rule acts on (dynamics↔ontology). `None`
    /// for a binding not scoped to a specific kind.
    subject_kind: Option<String>,
    enabled: bool,
    #[serde(with = "time::serde::rfc3339")]
    created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    updated_at: OffsetDateTime,
}

#[derive(Debug, Serialize)]
struct TriggerBindingListResponse {
    items: Vec<TriggerBindingResponse>,
    /// The registered domain-event vocabulary bindings may attach to (each key
    /// has a real dispatcher producer) — the authoring UI's event picker.
    registered_event_keys: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct CreateTriggerBindingRequest {
    definition_id: Uuid,
    trigger_type: String,
    event_key: String,
    /// Optional ontology object kind the rule acts on. When present it must be a
    /// registered object_types.kind (validated up front for a clean 422; the FK
    /// is the DB-level backstop).
    #[serde(default)]
    subject_kind: Option<String>,
    #[serde(default = "default_true")]
    enabled: bool,
}

const fn default_true() -> bool {
    true
}

fn trigger_binding_from_row(
    row: &sqlx::postgres::PgRow,
) -> Result<TriggerBindingResponse, DbError> {
    Ok(TriggerBindingResponse {
        id: row.try_get("id").map_err(DbError::Sqlx)?,
        definition_id: row.try_get("definition_id").map_err(DbError::Sqlx)?,
        trigger_type: row.try_get("trigger_type").map_err(DbError::Sqlx)?,
        event_key: row.try_get("event_key").map_err(DbError::Sqlx)?,
        subject_kind: row.try_get("subject_kind").map_err(DbError::Sqlx)?,
        enabled: row.try_get("enabled").map_err(DbError::Sqlx)?,
        created_at: row.try_get("created_at").map_err(DbError::Sqlx)?,
        updated_at: row.try_get("updated_at").map_err(DbError::Sqlx)?,
    })
}

async fn list_trigger_bindings(
    State(state): State<WorkflowStudioState>,
    Extension(principal): Extension<Principal>,
) -> Result<Json<TriggerBindingListResponse>, WorkflowStudioError> {
    authorize_workflow_manage(&principal)?;
    let org = principal.org_id;
    let items = with_org_conn::<_, Vec<TriggerBindingResponse>, WorkflowStudioError>(
        &state.pool,
        org,
        |tx| {
            Box::pin(async move {
                let rows = sqlx::query(
                    "SELECT id, definition_id, trigger_type, event_key, subject_kind, \
                            enabled, created_at, updated_at \
                     FROM workflow_trigger_bindings \
                     ORDER BY created_at DESC",
                )
                .fetch_all(tx.as_mut())
                .await?;
                rows.iter()
                    .map(|row| trigger_binding_from_row(row).map_err(WorkflowStudioError::from))
                    .collect()
            })
        },
    )
    .await?;
    record_workflow_studio_request("trigger_bindings", "success");
    Ok(Json(TriggerBindingListResponse {
        items,
        registered_event_keys: console_workflow_domain::REGISTERED_EVENT_KEYS
            .iter()
            .map(|key| (*key).to_owned())
            .collect(),
    }))
}

async fn create_trigger_binding(
    State(state): State<WorkflowStudioState>,
    Extension(principal): Extension<Principal>,
    Json(body): Json<CreateTriggerBindingRequest>,
) -> Result<Json<TriggerBindingResponse>, WorkflowStudioError> {
    authorize_workflow_manage(&principal)?;

    // Trigger type must be one of the reserved event-shaped TriggerType values
    // (MANUAL/SCHEDULE/API are not event bindings; 0105 CHECK backs this up).
    let trigger_type =
        TriggerType::from_db_str(body.trigger_type.trim()).map_err(WorkflowStudioError::from)?;
    if !trigger_type.is_event_binding() {
        return Err(WorkflowStudioError::validation(
            "trigger_type must be an event trigger (OBJECT_EVENT/IMPORT_EVENT/MAIL_EVENT/MESSENGER_EVENT/CALENDAR_EVENT/POLL_EVENT)",
        ));
    }
    // Only registered event keys have a real dispatcher producer; anything else
    // would be a rule that can never fire.
    let event_key = body.event_key.trim().to_owned();
    if !console_workflow_domain::REGISTERED_EVENT_KEYS.contains(&event_key.as_str()) {
        return Err(WorkflowStudioError::validation(format!(
            "event_key {event_key:?} is not a registered domain event"
        )));
    }
    // Optional object-kind scope (dynamics↔ontology): shape-check up front; the
    // FK to object_types + the DB existence check below give the clean 422.
    let subject_kind = body
        .subject_kind
        .as_deref()
        .map(str::trim)
        .filter(|kind| !kind.is_empty())
        .map(ToOwned::to_owned);
    if let Some(kind) = &subject_kind
        && !is_object_kind_slug(kind)
    {
        return Err(WorkflowStudioError::validation(format!(
            "subject_kind {kind:?} is not a valid kind slug"
        )));
    }

    let binding_id = Uuid::new_v4();
    let actor = principal.user_id;
    let org = principal.org_id;
    let definition_id = body.definition_id;
    let enabled = body.enabled;
    let trigger_type_db = trigger_type.as_db_str();
    let audit_event_key = event_key.clone();
    let audit_subject_kind = subject_kind.clone();
    let event = AuditEvent::new(
        Some(actor),
        AuditAction::new("workflow_trigger_binding.create")?,
        "workflow_trigger_binding",
        binding_id.to_string(),
        TraceContext::generate(),
        OffsetDateTime::now_utc(),
    )
    .with_org(org)
    .with_snapshots(
        None,
        Some(json!({
            "definition_id": definition_id,
            "trigger_type": trigger_type_db,
            "event_key": audit_event_key,
            "subject_kind": audit_subject_kind,
            "enabled": enabled,
        })),
    );

    let response = with_audit::<_, _, WorkflowStudioError>(&state.pool, event, move |tx| {
        Box::pin(async move {
            let definition_exists: bool =
                sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM workflow_definitions WHERE id = $1)")
                    .bind(definition_id)
                    .fetch_one(tx.as_mut())
                    .await?;
            if !definition_exists {
                return Err(WorkflowStudioError::from(KernelError::not_found(
                    "workflow definition not found",
                )));
            }
            if let Some(kind) = &subject_kind {
                let kind_exists: bool = sqlx::query_scalar(
                    "SELECT EXISTS(SELECT 1 FROM object_types WHERE kind = $1)",
                )
                .bind(kind)
                .fetch_one(tx.as_mut())
                .await?;
                if !kind_exists {
                    return Err(WorkflowStudioError::validation(format!(
                        "subject_kind {kind:?} is not a registered object type"
                    )));
                }
            }
            let duplicate: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM workflow_trigger_bindings \
                 WHERE definition_id = $1 AND event_key = $2)",
            )
            .bind(definition_id)
            .bind(&event_key)
            .fetch_one(tx.as_mut())
            .await?;
            if duplicate {
                return Err(WorkflowStudioError::from(KernelError::conflict(
                    "a binding for this definition and event already exists (enable/disable it instead)",
                )));
            }
            let row = sqlx::query(
                "INSERT INTO workflow_trigger_bindings \
                     (id, org_id, definition_id, trigger_type, event_key, subject_kind, \
                      enabled, created_by, updated_by) \
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $8) \
                 RETURNING id, definition_id, trigger_type, event_key, subject_kind, \
                           enabled, created_at, updated_at",
            )
            .bind(binding_id)
            .bind(*org.as_uuid())
            .bind(definition_id)
            .bind(trigger_type_db)
            .bind(&event_key)
            .bind(subject_kind.as_deref())
            .bind(enabled)
            .bind(*actor.as_uuid())
            .fetch_one(tx.as_mut())
            .await?;
            trigger_binding_from_row(&row).map_err(WorkflowStudioError::from)
        })
    })
    .await?;
    record_workflow_studio_request("trigger_binding_create", "success");
    Ok(Json(response))
}

async fn enable_trigger_binding(
    State(state): State<WorkflowStudioState>,
    Extension(principal): Extension<Principal>,
    Path(id): Path<Uuid>,
) -> Result<Json<TriggerBindingResponse>, WorkflowStudioError> {
    set_trigger_binding_enabled(state, principal, id, true).await
}

async fn disable_trigger_binding(
    State(state): State<WorkflowStudioState>,
    Extension(principal): Extension<Principal>,
    Path(id): Path<Uuid>,
) -> Result<Json<TriggerBindingResponse>, WorkflowStudioError> {
    set_trigger_binding_enabled(state, principal, id, false).await
}

async fn set_trigger_binding_enabled(
    state: WorkflowStudioState,
    principal: Principal,
    id: Uuid,
    enabled: bool,
) -> Result<Json<TriggerBindingResponse>, WorkflowStudioError> {
    authorize_workflow_manage(&principal)?;
    let actor = principal.user_id;
    let org = principal.org_id;
    let action = if enabled {
        "workflow_trigger_binding.enable"
    } else {
        "workflow_trigger_binding.disable"
    };
    // with_audits (not with_audit): the before-snapshot must record the REAL
    // prior enabled state read in the same transaction, not an assumption.
    let response = console_platform_db::with_audits::<_, _, WorkflowStudioError>(
        &state.pool,
        org,
        move |tx| {
            Box::pin(async move {
                let prior: Option<bool> = sqlx::query_scalar(
                    "SELECT enabled FROM workflow_trigger_bindings WHERE id = $1 FOR UPDATE",
                )
                .bind(id)
                .fetch_optional(tx.as_mut())
                .await?;
                let Some(prior) = prior else {
                    return Err(WorkflowStudioError::from(KernelError::not_found(
                        "trigger binding not found",
                    )));
                };
                let row = sqlx::query(
                    "UPDATE workflow_trigger_bindings \
                     SET enabled = $2, updated_by = $3, updated_at = now() \
                     WHERE id = $1 \
                     RETURNING id, definition_id, trigger_type, event_key, subject_kind, \
                               enabled, created_at, updated_at",
                )
                .bind(id)
                .bind(enabled)
                .bind(*actor.as_uuid())
                .fetch_one(tx.as_mut())
                .await?;
                let response = trigger_binding_from_row(&row).map_err(WorkflowStudioError::from)?;
                let event = AuditEvent::new(
                    Some(actor),
                    AuditAction::new(action)?,
                    "workflow_trigger_binding",
                    id.to_string(),
                    TraceContext::generate(),
                    OffsetDateTime::now_utc(),
                )
                .with_org(org)
                .with_snapshots(
                    Some(json!({ "enabled": prior })),
                    Some(json!({ "enabled": enabled })),
                );
                Ok((response, vec![event]))
            })
        },
    )
    .await?;
    record_workflow_studio_request(
        if enabled {
            "trigger_binding_enable"
        } else {
            "trigger_binding_disable"
        },
        "success",
    );
    Ok(Json(response))
}

#[derive(Debug, Serialize)]
struct WorkflowScheduleResponse {
    id: Uuid,
    label: String,
    cron_expr: String,
    timezone: String,
    definition_id: Uuid,
    enabled: bool,
    #[serde(with = "time::serde::rfc3339::option")]
    next_run_at: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339::option")]
    last_run_at: Option<OffsetDateTime>,
    last_status: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    updated_at: OffsetDateTime,
}

#[derive(Debug, Serialize)]
struct WorkflowScheduleListResponse {
    items: Vec<WorkflowScheduleResponse>,
}

#[derive(Debug, Deserialize)]
struct CreateWorkflowScheduleRequest {
    label: String,
    cron_expr: String,
    timezone: Option<String>,
    definition_id: Uuid,
    #[serde(default = "default_true")]
    enabled: bool,
}

#[derive(Debug, Deserialize)]
struct UpdateWorkflowScheduleRequest {
    label: Option<String>,
    cron_expr: Option<String>,
    timezone: Option<String>,
    enabled: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct PreviewScheduleRequest {
    cron_expr: String,
    timezone: Option<String>,
}

#[derive(Debug, Serialize)]
struct PreviewScheduleResponse {
    cron_expr: String,
    timezone: String,
    fire_times: Vec<String>,
}

#[derive(Debug, Serialize)]
struct ScheduleRunListResponse {
    items: Vec<ScheduleRunItem>,
}

#[derive(Debug, Serialize)]
struct ScheduleRunItem {
    run_id: Uuid,
    status: String,
    definition_id: Uuid,
    definition_version: i32,
    #[serde(with = "time::serde::rfc3339")]
    started_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339::option")]
    completed_at: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339::option")]
    failed_at: Option<OffsetDateTime>,
}

fn schedule_from_row(row: &sqlx::postgres::PgRow) -> Result<WorkflowScheduleResponse, DbError> {
    Ok(WorkflowScheduleResponse {
        id: row.try_get("id").map_err(DbError::Sqlx)?,
        label: row.try_get("label").map_err(DbError::Sqlx)?,
        cron_expr: row.try_get("cron_expr").map_err(DbError::Sqlx)?,
        timezone: row.try_get("timezone").map_err(DbError::Sqlx)?,
        definition_id: row.try_get("definition_id").map_err(DbError::Sqlx)?,
        enabled: row.try_get("enabled").map_err(DbError::Sqlx)?,
        next_run_at: row.try_get("next_run_at").map_err(DbError::Sqlx)?,
        last_run_at: row.try_get("last_run_at").map_err(DbError::Sqlx)?,
        last_status: row.try_get("last_status").map_err(DbError::Sqlx)?,
        created_at: row.try_get("created_at").map_err(DbError::Sqlx)?,
        updated_at: row.try_get("updated_at").map_err(DbError::Sqlx)?,
    })
}

async fn list_schedules(
    State(state): State<WorkflowStudioState>,
    Extension(principal): Extension<Principal>,
) -> Result<Json<WorkflowScheduleListResponse>, WorkflowStudioError> {
    authorize_workflow_manage(&principal)?;
    let org = principal.org_id;
    let items = with_org_conn::<_, Vec<WorkflowScheduleResponse>, WorkflowStudioError>(
        &state.pool,
        org,
        |tx| {
            Box::pin(async move {
                let rows = sqlx::query(
                    "SELECT id, label, cron_expr, timezone, definition_id, enabled, \
                            next_run_at, last_run_at, last_status, created_at, updated_at \
                     FROM workflow_schedules ORDER BY created_at DESC",
                )
                .fetch_all(tx.as_mut())
                .await?;
                rows.iter()
                    .map(|row| schedule_from_row(row).map_err(WorkflowStudioError::from))
                    .collect()
            })
        },
    )
    .await?;
    record_workflow_studio_request("schedules", "success");
    Ok(Json(WorkflowScheduleListResponse { items }))
}

async fn create_schedule(
    State(state): State<WorkflowStudioState>,
    Extension(principal): Extension<Principal>,
    Json(body): Json<CreateWorkflowScheduleRequest>,
) -> Result<Json<WorkflowScheduleResponse>, WorkflowStudioError> {
    authorize_workflow_manage(&principal)?;
    let label = body.label.trim().to_owned();
    if label.is_empty() || label.chars().count() > 120 {
        return Err(WorkflowStudioError::validation(
            "label must be 1-120 characters",
        ));
    }
    let cron_expr = body.cron_expr.trim().to_owned();
    let timezone = body
        .timezone
        .as_deref()
        .map(str::trim)
        .filter(|tz| !tz.is_empty())
        .unwrap_or(crate::workflow_schedules::DEFAULT_TIMEZONE)
        .to_owned();
    // Reject garbage before anything touches the DB; also computes the first
    // fire so the poller's due index sees the row immediately.
    let next_run_at = if body.enabled {
        Some(
            crate::workflow_schedules::next_occurrence(
                &cron_expr,
                &timezone,
                OffsetDateTime::now_utc(),
            )
            .map_err(WorkflowStudioError::from)?,
        )
    } else {
        // Still validate; a disabled schedule simply has no due fire.
        crate::workflow_schedules::next_occurrence(
            &cron_expr,
            &timezone,
            OffsetDateTime::now_utc(),
        )
        .map_err(WorkflowStudioError::from)?;
        None
    };

    let schedule_id = Uuid::new_v4();
    let actor = principal.user_id;
    let org = principal.org_id;
    let definition_id = body.definition_id;
    let enabled = body.enabled;
    let event = AuditEvent::new(
        Some(actor),
        AuditAction::new("workflow_schedule.create")?,
        "workflow_schedule",
        schedule_id.to_string(),
        TraceContext::generate(),
        OffsetDateTime::now_utc(),
    )
    .with_org(org)
    .with_snapshots(
        None,
        Some(json!({
            "label": label,
            "cron_expr": cron_expr,
            "timezone": timezone,
            "definition_id": definition_id,
            "enabled": enabled,
            "next_run_at": next_run_at.map(|at| at.to_string()),
        })),
    );
    let (label_ins, cron_ins, tz_ins) = (label.clone(), cron_expr.clone(), timezone.clone());
    let response = with_audit::<_, _, WorkflowStudioError>(&state.pool, event, move |tx| {
        Box::pin(async move {
            let definition_exists: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM workflow_definitions WHERE id = $1)",
            )
            .bind(definition_id)
            .fetch_one(tx.as_mut())
            .await?;
            if !definition_exists {
                return Err(WorkflowStudioError::from(KernelError::not_found(
                    "workflow definition not found",
                )));
            }
            let row = sqlx::query(
                "INSERT INTO workflow_schedules \
                     (id, org_id, label, cron_expr, timezone, definition_id, enabled, \
                      next_run_at, created_by, updated_by) \
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $9) \
                 RETURNING id, label, cron_expr, timezone, definition_id, enabled, \
                           next_run_at, last_run_at, last_status, created_at, updated_at",
            )
            .bind(schedule_id)
            .bind(*org.as_uuid())
            .bind(&label_ins)
            .bind(&cron_ins)
            .bind(&tz_ins)
            .bind(definition_id)
            .bind(enabled)
            .bind(next_run_at)
            .bind(*actor.as_uuid())
            .fetch_one(tx.as_mut())
            .await?;
            schedule_from_row(&row).map_err(WorkflowStudioError::from)
        })
    })
    .await?;
    record_workflow_studio_request("schedule_create", "success");
    Ok(Json(response))
}

async fn update_schedule(
    State(state): State<WorkflowStudioState>,
    Extension(principal): Extension<Principal>,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateWorkflowScheduleRequest>,
) -> Result<Json<WorkflowScheduleResponse>, WorkflowStudioError> {
    authorize_workflow_manage(&principal)?;
    if let Some(label) = &body.label {
        let label = label.trim();
        if label.is_empty() || label.chars().count() > 120 {
            return Err(WorkflowStudioError::validation(
                "label must be 1-120 characters",
            ));
        }
    }
    if let Some(cron_expr) = &body.cron_expr {
        crate::workflow_schedules::validate_cron(cron_expr).map_err(WorkflowStudioError::from)?;
    }
    if let Some(timezone) = &body.timezone {
        crate::workflow_schedules::validate_timezone(timezone)
            .map_err(WorkflowStudioError::from)?;
    }

    let actor = principal.user_id;
    let org = principal.org_id;
    let response = console_platform_db::with_audits::<_, _, WorkflowStudioError>(
        &state.pool,
        org,
        move |tx| {
            Box::pin(async move {
                let current = sqlx::query(
                    "SELECT id, label, cron_expr, timezone, definition_id, enabled, \
                        next_run_at, last_run_at, last_status, created_at, updated_at \
                 FROM workflow_schedules WHERE id = $1 FOR UPDATE",
                )
                .bind(id)
                .fetch_optional(tx.as_mut())
                .await?;
                let Some(current) = current else {
                    return Err(WorkflowStudioError::from(KernelError::not_found(
                        "workflow schedule not found",
                    )));
                };
                let current = schedule_from_row(&current).map_err(WorkflowStudioError::from)?;

                let label = body
                    .label
                    .as_deref()
                    .map(str::trim)
                    .map(ToOwned::to_owned)
                    .unwrap_or_else(|| current.label.clone());
                let cron_expr = body
                    .cron_expr
                    .as_deref()
                    .map(str::trim)
                    .map(ToOwned::to_owned)
                    .unwrap_or_else(|| current.cron_expr.clone());
                let timezone = body
                    .timezone
                    .as_deref()
                    .map(str::trim)
                    .map(ToOwned::to_owned)
                    .unwrap_or_else(|| current.timezone.clone());
                let enabled = body.enabled.unwrap_or(current.enabled);

                // Recompute the next fire whenever the effective firing inputs
                // change or the schedule (re-)enables — a stale past next_run_at
                // must never fire the OLD pattern once, and a re-enabled schedule
                // resumes in the future rather than firing for downtime.
                let firing_inputs_changed = cron_expr != current.cron_expr
                    || timezone != current.timezone
                    || (enabled && !current.enabled);
                let next_run_at = if !enabled {
                    None
                } else if firing_inputs_changed || current.next_run_at.is_none() {
                    Some(
                        crate::workflow_schedules::next_occurrence(
                            &cron_expr,
                            &timezone,
                            OffsetDateTime::now_utc(),
                        )
                        .map_err(WorkflowStudioError::from)?,
                    )
                } else {
                    current.next_run_at
                };

                let row = sqlx::query(
                    "UPDATE workflow_schedules \
                 SET label = $2, cron_expr = $3, timezone = $4, enabled = $5, \
                     next_run_at = $6, updated_by = $7, updated_at = now() \
                 WHERE id = $1 \
                 RETURNING id, label, cron_expr, timezone, definition_id, enabled, \
                           next_run_at, last_run_at, last_status, created_at, updated_at",
                )
                .bind(id)
                .bind(&label)
                .bind(&cron_expr)
                .bind(&timezone)
                .bind(enabled)
                .bind(next_run_at)
                .bind(*actor.as_uuid())
                .fetch_one(tx.as_mut())
                .await?;
                let response = schedule_from_row(&row).map_err(WorkflowStudioError::from)?;
                let event = AuditEvent::new(
                    Some(actor),
                    AuditAction::new("workflow_schedule.update")?,
                    "workflow_schedule",
                    id.to_string(),
                    TraceContext::generate(),
                    OffsetDateTime::now_utc(),
                )
                .with_org(org)
                .with_snapshots(
                    Some(json!({
                        "label": current.label,
                        "cron_expr": current.cron_expr,
                        "timezone": current.timezone,
                        "enabled": current.enabled,
                        "next_run_at": current.next_run_at,
                    })),
                    Some(json!({
                        "label": label,
                        "cron_expr": cron_expr,
                        "timezone": timezone,
                        "enabled": enabled,
                        "next_run_at": next_run_at,
                    })),
                );
                Ok((response, vec![event]))
            })
        },
    )
    .await?;
    record_workflow_studio_request("schedule_update", "success");
    Ok(Json(response))
}

async fn preview_schedule_next_runs(
    Extension(principal): Extension<Principal>,
    Json(body): Json<PreviewScheduleRequest>,
) -> Result<Json<PreviewScheduleResponse>, WorkflowStudioError> {
    authorize_workflow_manage(&principal)?;
    let timezone = body
        .timezone
        .as_deref()
        .map(str::trim)
        .filter(|tz| !tz.is_empty())
        .unwrap_or(crate::workflow_schedules::DEFAULT_TIMEZONE)
        .to_owned();
    let fires = crate::workflow_schedules::next_occurrences(
        &body.cron_expr,
        &timezone,
        OffsetDateTime::now_utc(),
        crate::workflow_schedules::PREVIEW_FIRE_COUNT,
    )
    .map_err(WorkflowStudioError::from)?;
    let fire_times = fires
        .into_iter()
        .map(|at| {
            at.format(&time::format_description::well_known::Rfc3339)
                .map_err(|err| WorkflowStudioError::internal(err.to_string()))
        })
        .collect::<Result<Vec<_>, _>>()?;
    record_workflow_studio_request("schedule_preview", "success");
    Ok(Json(PreviewScheduleResponse {
        cron_expr: body.cron_expr.trim().to_owned(),
        timezone,
        fire_times,
    }))
}

async fn list_schedule_runs(
    State(state): State<WorkflowStudioState>,
    Extension(principal): Extension<Principal>,
    Path(id): Path<Uuid>,
) -> Result<Json<ScheduleRunListResponse>, WorkflowStudioError> {
    authorize_workflow_manage(&principal)?;
    let org = principal.org_id;
    let items = with_org_conn::<_, Vec<ScheduleRunItem>, WorkflowStudioError>(
        &state.pool,
        org,
        move |tx| {
            Box::pin(async move {
                let exists: bool = sqlx::query_scalar(
                    "SELECT EXISTS(SELECT 1 FROM workflow_schedules WHERE id = $1)",
                )
                .bind(id)
                .fetch_one(tx.as_mut())
                .await?;
                if !exists {
                    return Err(WorkflowStudioError::from(KernelError::not_found(
                        "workflow schedule not found",
                    )));
                }
                let rows = sqlx::query(
                    "SELECT id, status, definition_id, definition_version, started_at, \
                            completed_at, failed_at \
                     FROM workflow_runs \
                     WHERE schedule_id = $1 \
                     ORDER BY started_at DESC \
                     LIMIT 100",
                )
                .bind(id)
                .fetch_all(tx.as_mut())
                .await?;
                rows.iter()
                    .map(|row| {
                        Ok(ScheduleRunItem {
                            run_id: row.try_get("id")?,
                            status: row.try_get("status")?,
                            definition_id: row.try_get("definition_id")?,
                            definition_version: row.try_get("definition_version")?,
                            started_at: row.try_get("started_at")?,
                            completed_at: row.try_get("completed_at")?,
                            failed_at: row.try_get("failed_at")?,
                        })
                    })
                    .collect::<Result<Vec<_>, sqlx::Error>>()
                    .map_err(WorkflowStudioError::from)
            })
        },
    )
    .await?;
    record_workflow_studio_request("schedule_runs", "success");
    Ok(Json(ScheduleRunListResponse { items }))
}

async fn verify_workflow_step_up(
    state: &WorkflowStudioState,
    principal: &Principal,
    step_up: Option<WorkflowStepUpAssertionRequest>,
) -> Result<(), WorkflowStudioError> {
    let step_up = step_up.ok_or_else(|| {
        WorkflowStudioError::new(
            StatusCode::PRECONDITION_REQUIRED,
            "passkey_step_up_required",
            "workflow publication changes require a fresh passkey step-up",
        )
    })?;
    let verifier = state.passkey_step_up.as_ref().ok_or_else(|| {
        WorkflowStudioError::unavailable("passkey step-up is not configured for Workflow Studio")
    })?;
    verifier
        .verify_step_up_for_user(
            &state.pool,
            step_up.ceremony_id,
            step_up.credential,
            *principal.user_id.as_uuid(),
        )
        .await
        .map_err(|_| WorkflowStudioError::unauthorized("passkey step-up failed"))?;
    Ok(())
}

fn authorize_workflow_manage(principal: &Principal) -> Result<(), WorkflowStudioError> {
    authorize_org_wide(principal, Action::new(Feature::RoleManage))
        .map_err(WorkflowStudioError::from)
}

/// §16 org-scope automation gate (85 판정): `metadata.owner_scope.type` on the
/// definition JSON distinguishes an org-owned automation (publish/run require
/// four-eyes) from a personal-scope one (§3.9.0-① stays direct). Missing or
/// unrecognized scope defaults to org — fail-closed toward the stricter gate.
fn workflow_owner_scope_is_org(definition: &Value) -> bool {
    definition
        .pointer("/metadata/owner_scope/type")
        .and_then(Value::as_str)
        != Some("personal")
}

/// Map the governance adapter's error onto this crate's error surface (mirrors
/// `governance_to_ontology` in `crates/ontology/rest`).
fn governance_to_workflow_studio(error: PgGovernanceError) -> WorkflowStudioError {
    match error {
        PgGovernanceError::Db(db) => WorkflowStudioError::from(db),
        PgGovernanceError::Domain(kernel) => WorkflowStudioError::from(kernel),
    }
}

/// The four-eyes `kind`s org-scope automation approvals are decided under. The
/// approval binds to the workflow definition id (`target_ref`), so an approval to
/// publish one definition can never authorize publishing/running another.
const WORKFLOW_PUBLISH_FOUR_EYES_KIND: &str = "workflow.publish";
const WORKFLOW_RUN_FOUR_EYES_KIND: &str = "workflow.run";

/// Evaluate the §16 four-eyes gate for an org-scope automation action.
/// Personal-scope automations (`owner_scope_is_org == false`) skip the gate
/// entirely (`GateChainConfig::default()` — every gate `NotRequired`), per
/// §3.9.0-①.
fn evaluate_automation_four_eyes_gate(
    owner_scope_is_org: bool,
    four_eyes_approved: Option<bool>,
) -> console_governance_domain::GateChainOutcome {
    evaluate_gate_chain(
        GateChainConfig {
            four_eyes: owner_scope_is_org,
            ..GateChainConfig::default()
        },
        &GateEvidence {
            four_eyes_approved,
            ..GateEvidence::default()
        },
    )
}

fn snapshot_from_row(row: &WorkflowVersionRow) -> Value {
    json!({
        "id": row.definition_id,
        "workflow_key": row.workflow_key,
        "status": row.status,
        "latest_version": row.latest_version,
        "active_version": row.active_version,
        "required_approval_line": row.required_approval_line,
        "required_payment_line": row.required_payment_line
    })
}

fn snapshot_from_response(row: &WorkflowDefinitionResponse) -> Value {
    json!({
        "id": row.id,
        "workflow_key": row.workflow_key,
        "status": row.status,
        "latest_version": row.latest_version,
        "active_version": row.active_version,
        "required_approval_line": row.required_approval_line,
        "required_payment_line": row.required_payment_line
    })
}

fn json_array(value: Value) -> Vec<Value> {
    match value {
        Value::Array(values) => values,
        _ => Vec::new(),
    }
}

fn json_string_array(value: Value) -> Vec<String> {
    match value {
        Value::Array(values) => values
            .into_iter()
            .filter_map(|value| value.as_str().map(ToOwned::to_owned))
            .collect(),
        _ => Vec::new(),
    }
}

fn empty_object() -> Value {
    json!({})
}

/// The workflow spine carries NO branch: `workflow_runs`, `workflow_waiting_tasks`,
/// `workflow_definitions`, `object_types` and `series` all lack a `branch_id`
/// column, and a definition-only run has no subject row to read one from. These
/// guards are therefore capability decisions
/// (`console_platform_authz::authorize_capability` via
/// `AuthorizationResource::branchless`), not per-branch ones.
///
/// This constant replaced a `guard_branch(principal)` helper that fabricated a
/// branch out of `principal.branch_scope` — `All => BranchId::new()`,
/// `Branches(b) => b.iter().next()`. Both arms made `authorize`'s
/// `branch_scope.allows(resource_branch)` check a tautology, silently deleting
/// the branch dimension instead of enforcing it. Where a workflow guard DOES have
/// a real subject branch (`workflow_object_context::resolve_subject_branch` reads
/// `work_orders.branch_id` / `support_tickets.branch_id`) it passes `Some(..)` and
/// the branch check is real.
pub(crate) const WORKFLOW_SPINE_HAS_NO_BRANCH: Option<BranchId> = None;

/// [`build_guard_request`] widened to the branch-less spine.
///
/// `Some(branch)` is the upstream helper verbatim. `None` builds the identical
/// request against [`AuthorizationResource::branchless`] instead — same feature,
/// same policy domain, same RLS scope proof — which routes the decision through
/// `authorize_capability`.
///
/// ## Why this lives here and not in `console-workflow-runtime`
///
/// `build_guard_request` takes `branch_id: BranchId`, and widening it to
/// `Option<BranchId>` is a public-signature change in a crate another lane owns
/// and is mid-flight in. This wrapper is the handoff's stand-in: DELETE it and
/// call `build_guard_request` directly once that lane lands the `Option`. See
/// `docs/decisions/notes/DN-0004-adr-0028-branchless-capability-authorization.md`
/// § "Handoff to the workflow lane".
fn spine_guard_request(
    principal: &Principal,
    required_policy: &str,
    org: console_kernel_core::OrgId,
    branch: Option<BranchId>,
    resource_type: &str,
    object_id: &str,
    domain: &str,
) -> Result<AuthorizationRequest, KernelError> {
    let Some(branch) = branch else {
        let feature = Feature::from_str(required_policy)?;
        let resource = AuthorizationResource::branchless(org, resource_type.to_owned())
            .with_resource_id(object_id.to_owned());
        return Ok(
            AuthorizationRequest::new(principal.clone(), Action::new(feature), resource)
                .with_policy_domain(domain.to_owned())
                .with_rls_scope_proof(RlsScopeProof::runtime_role_guc(org)),
        );
    };
    build_guard_request(
        principal,
        required_policy,
        org,
        branch,
        resource_type,
        object_id,
        domain,
    )
}

/// `console_workflow_runtime::enforce_finalize_policy` for the branch-less spine.
///
/// Identical rules — author finalize is owner-only, delegated finalize needs a
/// non-blank reason and a configured policy, and the policy is resolved with
/// `Feature::from_str` on the RAW `required_policy` (NOT through
/// [`guard_policy`], which would map `approval_review` onto `completion_review`
/// and admit a delegate the upstream rule denies). The single difference is the
/// guard resource: branch-less, because `workflow_waiting_tasks` has no
/// `branch_id` column.
///
/// Same handoff as [`spine_guard_request`]: `FinalizePolicyRequest::branch` is
/// `BranchId` in a crate another lane owns. DELETE this and call
/// `enforce_finalize_policy` again once that field is `Option<BranchId>`.
fn enforce_spine_finalize_policy(
    principal: &Principal,
    context: &FinalizeWaitingTaskContext,
    mode: FinalizeMode,
    reason: Option<&str>,
    resource_type: &str,
    resource_id: &str,
) -> Result<FinalizePolicyOutcome, WorkflowStudioError> {
    match mode {
        FinalizeMode::Author => {
            if principal.user_id != context.initiated_by {
                return Err(WorkflowStudioError::from(KernelError::forbidden(
                    "author finalize requires the initiating author",
                )));
            }
            Ok(FinalizePolicyOutcome {
                delegated_reason: None,
                guard_audit: None,
            })
        }
        FinalizeMode::Delegate => {
            let reason = reason
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| {
                    KernelError::validation("delegated finalize requires a non-empty reason")
                })?;
            let required_policy = context.required_policy.as_deref().ok_or_else(|| {
                KernelError::forbidden("delegated finalize requires a configured policy")
            })?;
            let feature = Feature::from_str(required_policy)
                .map_err(|_| KernelError::forbidden("delegated finalize policy is unknown"))?;
            let request = spine_guard_request(
                principal,
                required_policy,
                principal.org_id,
                WORKFLOW_SPINE_HAS_NO_BRANCH,
                resource_type,
                resource_id,
                WAITING_COMPLETION_DOMAIN,
            )
            .map_err(|_| KernelError::forbidden("delegated finalize policy denied"))?;
            let entry = workflow_coexistence_entry(
                "workflow.waiting_task.finalize.delegate",
                WAITING_COMPLETION_DOMAIN,
                feature,
                resource_type,
            );
            let outcome = guard(&request, &entry);
            if !outcome.is_allowed() {
                return Err(WorkflowStudioError::from(KernelError::forbidden(
                    "delegated finalize policy denied",
                )));
            }
            Ok(FinalizePolicyOutcome {
                delegated_reason: Some(reason.to_owned()),
                guard_audit: Some(outcome.audit),
            })
        }
    }
}

/// Mirror [`spine_guard_request`]'s resource shape for the audit-only Cedar parity
/// observations, so the parity row records the same branch-less/branch-bearing
/// scope the enforced decision used.
fn shadow_resource(
    org: console_kernel_core::OrgId,
    branch: Option<BranchId>,
    resource_type: impl Into<String>,
) -> AuthorizationResource {
    match branch {
        Some(branch) => AuthorizationResource::branch(org, branch, resource_type),
        None => AuthorizationResource::branchless(org, resource_type),
    }
}

fn record_workflow_studio_request(surface: &'static str, outcome: &'static str) {
    metrics::counter!(
        WORKFLOW_STUDIO_REQUESTS_TOTAL,
        "surface" => surface,
        "outcome" => outcome,
    )
    .increment(1);
}

fn shadow_audit_event(
    shadow: &AuthorizationAuditEvent,
    actor: UserId,
    org: console_kernel_core::OrgId,
    task_id: Uuid,
) -> Result<AuditEvent, KernelError> {
    shadow_audit_event_for(shadow, actor, org, "workflow_waiting_task", task_id)
}

fn shadow_audit_event_for(
    shadow: &AuthorizationAuditEvent,
    actor: UserId,
    org: console_kernel_core::OrgId,
    target_type: &'static str,
    target_id: Uuid,
) -> Result<AuditEvent, KernelError> {
    Ok(AuditEvent::new(
        Some(actor),
        AuditAction::new("workflow_runtime.cedar_shadow")?,
        target_type,
        target_id.to_string(),
        TraceContext::generate(),
        OffsetDateTime::now_utc(),
    )
    .with_org(org)
    .with_snapshots(
        None,
        Some(serde_json::to_value(shadow).map_err(|err| KernelError::internal(err.to_string()))?),
    ))
}

#[derive(Debug)]
struct WorkflowStudioError {
    status: StatusCode,
    code: &'static str,
    message: String,
}

impl WorkflowStudioError {
    fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
        }
    }

    fn validation(message: impl Into<String>) -> Self {
        Self::from(KernelError::validation(message.into()))
    }

    fn unauthorized(message: impl Into<String>) -> Self {
        Self::new(StatusCode::UNAUTHORIZED, "unauthorized", message)
    }

    fn unavailable(message: impl Into<String>) -> Self {
        Self::new(StatusCode::SERVICE_UNAVAILABLE, "unavailable", message)
    }

    fn internal(message: impl Into<String>) -> Self {
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, "internal", message)
    }
}

impl From<KernelError> for WorkflowStudioError {
    fn from(error: KernelError) -> Self {
        let status = match error.kind {
            ErrorKind::Validation => StatusCode::UNPROCESSABLE_ENTITY,
            ErrorKind::NotFound => StatusCode::NOT_FOUND,
            ErrorKind::Forbidden => StatusCode::FORBIDDEN,
            ErrorKind::Conflict | ErrorKind::InvalidTransition => StatusCode::CONFLICT,
            ErrorKind::Internal => StatusCode::INTERNAL_SERVER_ERROR,
        };
        Self {
            status,
            code: error_code(error.kind),
            message: error.message,
        }
    }
}

impl From<DbError> for WorkflowStudioError {
    fn from(value: DbError) -> Self {
        tracing::error!(error = %value, "workflow studio database operation failed");
        Self::internal("workflow studio request failed")
    }
}

impl From<sqlx::Error> for WorkflowStudioError {
    fn from(value: sqlx::Error) -> Self {
        Self::from(DbError::Sqlx(value))
    }
}

impl IntoResponse for WorkflowStudioError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(json!({ "error": { "code": self.code, "message": self.message } })),
        )
            .into_response()
    }
}

fn error_code(kind: ErrorKind) -> &'static str {
    match kind {
        ErrorKind::Validation => "validation",
        ErrorKind::NotFound => "not_found",
        ErrorKind::Forbidden => "forbidden",
        ErrorKind::Conflict => "conflict",
        ErrorKind::InvalidTransition => "invalid_transition",
        ErrorKind::Internal => "internal",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(not(feature = "test-postgres"))]
    #[test]
    fn allowlist_rejects_unknown_connector_actions() -> Result<(), String> {
        let err = match validate_action_allowlist(&[json!({
            "connector_key": "external.random",
            "action_key": "post_secret"
        })]) {
            Ok(()) => return Err("unknown connector must fail closed".to_owned()),
            Err(err) => err,
        };

        assert_eq!(err.status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(
            err.message
                .contains("not in the Workflow Studio connector allowlist")
        );
        assert!(!err.message.contains("external.random"));
        assert!(!err.message.contains("post_secret"));
        Ok(())
    }

    #[cfg(not(feature = "test-postgres"))]
    #[test]
    fn catalog_uses_electronic_approval_system_name_for_internal_approvals_connector() {
        let connector = ALLOWED_CONNECTORS
            .iter()
            .find(|connector| connector.connector_key == "internal.approvals");
        assert!(
            connector.is_some(),
            "internal.approvals connector must remain allowlisted"
        );
        if let Some(connector) = connector {
            assert_eq!(connector.display_name, "전자결재시스템");
            assert_eq!(
                connector.action_keys,
                &["request_approval", "notify_assignee"]
            );
        }
    }

    /// The canonical completion→approval→payroll executable node graph (design
    /// §template). Published via Studio as a `wf.exec.v1` definition; the M2 runtime
    /// walks it. `emit_payroll` fans out through the `internal.jobs` JOB connector.
    fn maintenance_completion_execution_definition() -> Value {
        json!({
            "schema_version": WORKFLOW_EXEC_SCHEMA_VERSION,
            "workflow_key": "work_order.maintenance_completion",
            "object_type": "work_order",
            // Operational pipeline, NOT self-service 기안: only completion_review
            // authority may initiate the completion→approval→payroll run (security
            // M2). Contrast the approval templates, which carry no `start_policy`.
            "start_policy": "completion_review",
            "nodes": [
                { "node_key": "mechanic_report", "node_type": "object_gate" },
                {
                    "node_key": "admin_approval",
                    "node_type": "human_task",
                    "title": "관리자 완료 승인",
                    "required_policy": "completion_review",
                    "assignee_role_key": "admin"
                },
                {
                    "node_key": "executive_approval",
                    "node_type": "human_task",
                    "title": "임원 완료 승인",
                    "required_policy": "completion_review",
                    "assignee_role_key": "executive"
                },
                {
                    "node_key": "apply_completion",
                    "node_type": "object_mutation",
                    "object_type": "work_order",
                    "target_status": "FINAL_COMPLETED"
                },
                {
                    "node_key": "emit_payroll",
                    "node_type": "job",
                    "connector_key": "internal.jobs",
                    "action_key": "draft_payroll_run",
                    "channel": "JOB",
                    "destination_ref": "payroll.draft_run",
                    "job": "payroll_draft"
                }
            ],
            "edges": [
                { "from": "mechanic_report", "to": "admin_approval" },
                { "from": "admin_approval", "to": "executive_approval" },
                { "from": "executive_approval", "to": "apply_completion" },
                { "from": "apply_completion", "to": "emit_payroll" }
            ]
        })
    }

    #[cfg(not(feature = "test-postgres"))]
    #[test]
    fn completion_approval_payroll_execution_graph_validates() -> Result<(), String> {
        // The canonical wf.exec.v1 completion→approval→payroll graph is a valid
        // executable definition.
        validate_definition_object(maintenance_completion_execution_definition())
            .map_err(|err| format!("canonical execution graph must validate: {}", err.message))?;
        // Its payroll JOB action publishes only because internal.jobs is allowlisted.
        assert!(connector_allows("internal.jobs", "draft_payroll_run"));
        // The publish-time action_allowlist check (validate_action_allowlist) accepts
        // the JOB connector entry the template carries.
        validate_action_allowlist(&[json!({
            "connector_key": "internal.jobs",
            "action_key": "draft_payroll_run"
        })])
        .map_err(|err| {
            format!(
                "internal.jobs.draft_payroll_run must allowlist: {}",
                err.message
            )
        })?;
        Ok(())
    }

    #[cfg(not(feature = "test-postgres"))]
    #[test]
    fn receipt_required_approval_definition_includes_receipt_waiting_node() -> Result<(), String> {
        let definition = build_approval_execution_definition("leave")
            .map_err(|err| format!("leave template should build: {}", err.message))?;

        validate_definition_object(definition.clone())
            .map_err(|err| format!("leave approval graph must validate: {}", err.message))?;

        let nodes = definition["nodes"]
            .as_array()
            .ok_or_else(|| "nodes must be an array".to_owned())?;
        assert!(
            nodes.iter().any(|node| {
                node["node_key"] == json!("receipt.target")
                    && node["node_type"] == json!("human_task")
                    && node["assignee_role_key"] == json!("receipt_subject")
                    && node["required_policy"] == json!("approval_receipt")
            }),
            "receipt-required template must emit receipt.target human task"
        );

        let edges = definition["edges"]
            .as_array()
            .ok_or_else(|| "edges must be an array".to_owned())?;
        assert!(
            edges
                .iter()
                .any(|edge| edge["from"] == json!("finalize.author")
                    && edge["to"] == json!("receipt.target")),
            "receipt-required template must route finalization to receipt confirmation"
        );
        Ok(())
    }

    #[cfg(not(feature = "test-postgres"))]
    #[test]
    fn all_approval_template_builder_outputs_validate() -> Result<(), String> {
        for template in APPROVAL_TEMPLATES {
            let definition = build_approval_execution_definition(template.key)
                .map_err(|err| format!("{} should build: {}", template.key, err.message))?;
            validate_definition_object(definition.clone()).map_err(|err| {
                format!(
                    "{} approval graph must validate: {}",
                    template.key, err.message
                )
            })?;

            let has_receipt = definition["nodes"]
                .as_array()
                .ok_or_else(|| format!("{} nodes must be an array", template.key))?
                .iter()
                .any(|node| node["node_key"] == json!("receipt.target"));
            assert_eq!(
                has_receipt, template.receipt_required,
                "{} receipt node must match catalog flag",
                template.key
            );
        }
        Ok(())
    }

    #[cfg(not(feature = "test-postgres"))]
    #[test]
    fn human_task_without_required_policy_fails_authoring() -> Result<(), String> {
        // Security H1(a): a human_task node MUST declare required_policy at authoring
        // time. Drop it from the executable graph → publish-validation is a 422.
        let mut definition = maintenance_completion_execution_definition();
        // node 1 is the admin_approval human_task.
        definition["nodes"][1]
            .as_object_mut()
            .ok_or_else(|| "node must be an object".to_owned())?
            .remove("required_policy");
        let err = match validate_definition_object(definition) {
            Ok(_) => {
                return Err("a human_task without required_policy must fail closed".to_owned());
            }
            Err(err) => err,
        };
        assert_eq!(err.status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(
            err.message.contains("required_policy"),
            "message should name the missing field: {}",
            err.message
        );
        Ok(())
    }

    #[cfg(not(feature = "test-postgres"))]
    #[test]
    fn job_node_without_start_policy_fails_authoring() -> Result<(), String> {
        // Engine-Gen follow-up: a graph containing a `job` node MUST declare a
        // top-level start_policy at authoring time (fail-closed start authority).
        // Drop start_policy from the canonical completion→approval→payroll graph
        // (which carries a payroll job) → publish-validation is a 422.
        let mut definition = maintenance_completion_execution_definition();
        definition
            .as_object_mut()
            .ok_or_else(|| "definition must be an object".to_owned())?
            .remove("start_policy");
        let err = match validate_definition_object(definition) {
            Ok(_) => {
                return Err("a job-bearing graph without start_policy must fail closed".to_owned());
            }
            Err(err) => err,
        };
        assert_eq!(err.status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(
            err.message.contains("start_policy"),
            "message should name the missing field: {}",
            err.message
        );

        // With start_policy present the same job-bearing graph validates.
        validate_definition_object(maintenance_completion_execution_definition()).map_err(
            |err| {
                format!(
                    "job-bearing graph with start_policy must validate: {}",
                    err.message
                )
            },
        )?;

        // A job-free approval graph is unaffected — no start_policy required.
        let approval = build_approval_execution_definition("leave")
            .map_err(|err| format!("leave template should build: {}", err.message))?;
        assert!(
            approval.get("start_policy").is_none(),
            "approval templates deliberately carry no start_policy"
        );
        validate_definition_object(approval).map_err(|err| {
            format!(
                "job-free approval graph must validate without start_policy: {}",
                err.message
            )
        })?;
        Ok(())
    }

    #[cfg(not(feature = "test-postgres"))]
    #[test]
    fn oversize_run_payload_is_rejected() -> Result<(), String> {
        // Security M2: a payload over the 64 KiB serialized ceiling is a 422.
        let big = json!({ "blob": "x".repeat(MAX_RUN_PAYLOAD_BYTES + 1) });
        let err = match check_payload_size("input_payload", &big) {
            Ok(()) => return Err("oversize payload must be rejected".to_owned()),
            Err(err) => err,
        };
        assert_eq!(err.status, StatusCode::UNPROCESSABLE_ENTITY);
        // A modest payload passes.
        check_payload_size("input_payload", &json!({ "reason": "annual" }))
            .map_err(|e| format!("a small payload must pass: {}", e.message))?;
        Ok(())
    }

    #[cfg(not(feature = "test-postgres"))]
    #[test]
    fn policy_less_task_has_no_authorization_boundary() -> Result<(), String> {
        // Security H1(b): the claim/decide path fails closed (403) on a legacy
        // policy-less row; a policy-bearing row passes the boundary check.
        let err = match require_task_authorization_boundary(None) {
            Ok(()) => return Err("a policy-less task must be refused".to_owned()),
            Err(err) => err,
        };
        assert_eq!(err.status, StatusCode::FORBIDDEN);
        assert!(err.message.contains("authorization boundary"));
        require_task_authorization_boundary(Some("approval_finalize"))
            .map_err(|e| format!("a policy-bearing task must pass: {}", e.message))?;
        Ok(())
    }

    #[cfg(not(feature = "test-postgres"))]
    #[test]
    fn execution_graph_job_node_requires_allowlisted_connector() -> Result<(), String> {
        // Swap the payroll node onto an unknown connector: publish-validation of the
        // graph must fail closed (internal.jobs is genuinely load-bearing).
        let mut definition = maintenance_completion_execution_definition();
        definition["nodes"][4]["connector_key"] = json!("external.random");
        let err = match validate_definition_object(definition) {
            Ok(_) => return Err("a job node on an unlisted connector must fail closed".to_owned()),
            Err(err) => err,
        };
        assert_eq!(err.status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(err.message.contains("connector allowlist"));
        assert!(!err.message.contains("external.random"));
        Ok(())
    }

    #[cfg(not(feature = "test-postgres"))]
    #[test]
    fn execution_graph_accepts_guardrail_control_point_nodes() -> Result<(), String> {
        let definition = json!({
            "schema_version": WORKFLOW_EXEC_SCHEMA_VERSION,
            "workflow_key": "work_order.guarded_completion",
            "object_type": "work_order",
            "nodes": [
                {
                    "node_key": "guard.checklist.ops_attestation",
                    "node_type": "guard.checklist_attestation",
                    "label": "Operations attestation",
                    "required_policy": "completion_review",
                    "assignee_role_key": "operations.manager",
                    "items": [{
                        "key": "evidence_uploaded",
                        "label": "Evidence uploaded",
                        "kind": "evidence_ref",
                        "required": true,
                        "min_count": 1
                    }]
                },
                {
                    "node_key": "guard.four_eyes.peer_review",
                    "node_type": "guard.four_eyes_peer_review",
                    "label": "Peer review",
                    "required_policy": "completion_review",
                    "assignee_role_key": "operations.manager",
                    "subject_actor_refs": ["run.initiated_by"],
                    "min_reviewers": 1
                },
                {
                    "node_key": "guard.sod.purchase_approval",
                    "node_type": "guard.segregation_of_duties",
                    "label": "Purchase approver must differ",
                    "policy_key": "purchase.self_approval",
                    "actor_under_test_ref": "run.current_actor",
                    "blocked_actor_refs": ["object.requested_by"],
                    "mode": "hard_block"
                },
                {
                    "node_key": "guard.egress.mail",
                    "node_type": "guard.egress_policy",
                    "label": "Mail egress gate",
                    "egress_kind": "mail",
                    "channel": "internal.mail",
                    "required_policy": "mail_use",
                    "classification_ref": "mail.thread.classification",
                    "external_recipient_policy": "block"
                }
            ],
            "edges": []
        });

        validate_definition_object(definition.clone()).map_err(|err| err.message)?;

        let mut invalid = definition;
        invalid["nodes"][0]["browser_supplied_org_id"] = json!("must-not-be-accepted");
        let err = match validate_definition_object(invalid) {
            Ok(_) => return Err("browser-supplied guardrail fields must fail closed".to_owned()),
            Err(err) => err,
        };
        assert_eq!(err.status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(err.message.contains("unsupported field"));
        Ok(())
    }

    #[cfg(not(feature = "test-postgres"))]
    #[test]
    fn required_lines_block_publish() {
        let row = WorkflowVersionRow {
            definition_id: Uuid::new_v4(),
            workflow_key: "work_order.completion".to_owned(),
            display_name: "Completion".to_owned(),
            object_type: "work_order".to_owned(),
            status: "DRAFT".to_owned(),
            latest_version: 1,
            active_version: None,
            definition: json!({}),
            approval_line: vec![],
            payment_line: vec![],
            notification_rules: vec![],
            action_allowlist: vec![json!({
                "connector_key": "internal.approvals",
                "action_key": "request_approval"
            })],
            required_approval_line: true,
            required_payment_line: true,
            pending_version: None,
            pending_staged_by: None,
            created_at: OffsetDateTime::UNIX_EPOCH,
            updated_at: OffsetDateTime::UNIX_EPOCH,
        };

        let findings = validate_publishable(&row);
        assert!(
            findings
                .iter()
                .any(|finding| finding.code == "missing_approval_line")
        );
        assert!(
            findings
                .iter()
                .any(|finding| finding.code == "missing_payment_line")
        );
    }

    #[cfg(not(feature = "test-postgres"))]
    #[test]
    fn create_rejects_invalid_canonical_workflow_graphs() -> Result<(), String> {
        let mut missing_schema = canonical_workflow_definition("leave_request");
        missing_schema
            .as_object_mut()
            .ok_or_else(|| "definition fixture must be an object".to_owned())?
            .remove("schema_version");

        let mut object_type_mismatch = canonical_workflow_definition("leave_request");
        object_type_mismatch["metadata"]["object_type"] = json!("work_order");

        let mut missing_trigger = canonical_workflow_definition("leave_request");
        missing_trigger["graph"]["nodes"]
            .as_array_mut()
            .ok_or_else(|| "nodes fixture must be an array".to_owned())?
            .retain(|node| node["type"] != "trigger.form_submission");

        let mut missing_terminal = canonical_workflow_definition("leave_request");
        missing_terminal["graph"]["nodes"]
            .as_array_mut()
            .ok_or_else(|| "nodes fixture must be an array".to_owned())?
            .retain(|node| node["type"] != "end.state");

        let mut duplicate_node_id = canonical_workflow_definition("leave_request");
        duplicate_node_id["graph"]["nodes"][1]["id"] = json!("node-trigger");

        let mut invalid_edge_endpoint = canonical_workflow_definition("leave_request");
        invalid_edge_endpoint["graph"]["edges"][0]["to_node_id"] = json!("node-missing");

        let mut unknown_node_type = canonical_workflow_definition("leave_request");
        unknown_node_type["graph"]["nodes"][1]["type"] = json!("action.raw_http");
        unknown_node_type["graph"]["nodes"][1]["config"]["type"] = json!("action.raw_http");

        let mut unknown_connector_action = canonical_workflow_definition("leave_request");
        unknown_connector_action["graph"]["nodes"][5]["config"]["action_key"] =
            json!("post_secret");

        let cases = [
            ("schema_version", missing_schema),
            ("object_type_mismatch", object_type_mismatch),
            ("missing_trigger", missing_trigger),
            ("missing_terminal", missing_terminal),
            ("duplicate_node_id", duplicate_node_id),
            ("invalid_edge_endpoint", invalid_edge_endpoint),
            ("unknown_node_type", unknown_node_type),
            ("connector_action_not_allowlisted", unknown_connector_action),
        ];

        for (expected_code, definition) in cases {
            let err = match normalize_create_request(create_request(definition, "leave_request")) {
                Ok(_) => return Err("invalid canonical workflow graph must fail closed".to_owned()),
                Err(err) => err,
            };
            assert_eq!(err.status, StatusCode::UNPROCESSABLE_ENTITY);
            assert!(
                err.message.contains(expected_code),
                "expected validation message to contain {expected_code}, got {}",
                err.message
            );
        }

        Ok(())
    }

    #[cfg(not(feature = "test-postgres"))]
    #[test]
    fn create_rejects_unsafe_condition_branch_config() -> Result<(), String> {
        let mut invalid_expression_op = canonical_workflow_definition("leave_request");
        invalid_expression_op["graph"]["nodes"][3]["config"]["expression"]["op"] =
            json!("browser_script");

        let mut invalid_expression_ref = canonical_workflow_definition("leave_request");
        invalid_expression_ref["graph"]["nodes"][3]["config"]["expression"]["left"]["ref"] =
            json!("trigger.raw_table");

        let mut invalid_branch_when = canonical_workflow_definition("leave_request");
        invalid_branch_when["graph"]["nodes"][3]["config"]["branches"][0]["when"] =
            json!("javascript:approved");

        let mut invalid_default_port = canonical_workflow_definition("leave_request");
        invalid_default_port["graph"]["nodes"][3]["config"]["default_port"] =
            json!("trigger.raw_table");

        let mut undeclared_branch_port = canonical_workflow_definition("leave_request");
        undeclared_branch_port["graph"]["nodes"][3]["output_ports"]
            .as_array_mut()
            .ok_or_else(|| "branch output ports fixture must be an array".to_owned())?
            .push(json!({
                "key": "browser_defined",
                "direction": "output",
                "type": "flow.branch",
                "required": false,
                "cardinality": "one",
                "label": "Browser-defined"
            }));

        let cases = [
            ("invalid_condition_expression", invalid_expression_op),
            ("invalid_condition_expression", invalid_expression_ref),
            ("invalid_condition_branch", invalid_branch_when),
            ("invalid_condition_default", invalid_default_port),
            ("invalid_condition_branch", undeclared_branch_port),
        ];

        for (expected_code, definition) in cases {
            let err = match normalize_create_request(create_request(definition, "leave_request")) {
                Ok(_) => return Err("unsafe condition.branch config must fail closed".to_owned()),
                Err(err) => err,
            };
            assert_eq!(err.status, StatusCode::UNPROCESSABLE_ENTITY);
            assert!(
                err.message.contains(expected_code),
                "expected validation message to contain {expected_code}, got {}",
                err.message
            );
        }

        Ok(())
    }

    #[cfg(not(feature = "test-postgres"))]
    #[test]
    fn create_rejects_browser_defined_object_scope_and_update_handles() -> Result<(), String> {
        let mut trigger_object_mismatch = canonical_workflow_definition("leave_request");
        trigger_object_mismatch["graph"]["nodes"][0]["config"]["source"]["object_type"] =
            json!("raw_table");

        let mut trigger_scope_escape = canonical_workflow_definition("leave_request");
        trigger_scope_escape["graph"]["nodes"][0]["config"]["source"]["scope"] = json!("browser");

        let mut form_object_mismatch = canonical_workflow_definition("leave_request");
        form_object_mismatch["graph"]["nodes"][1]["config"]["fields"][0]["object_type"] =
            json!("raw_table");

        let mut raw_action_id = canonical_workflow_definition("leave_request");
        raw_action_id["graph"]["nodes"][4]["config"]["action_id"] = json!("raw_table.update");

        let mut raw_policy = canonical_workflow_definition("leave_request");
        raw_policy["graph"]["nodes"][4]["config"]["requires_policy"] = json!("raw_table.update");

        let mut raw_target = canonical_workflow_definition("leave_request");
        raw_target["graph"]["nodes"][4]["config"]["target"]["from"] = json!("trigger.raw_table");

        let mut raw_input_handle = canonical_workflow_definition("leave_request");
        raw_input_handle["graph"]["nodes"][4]["config"]["input"]["updated_by"]["from"] =
            json!("form.browser_actor");

        let cases = [
            ("object_type_mismatch", trigger_object_mismatch),
            ("trigger_source", trigger_scope_escape),
            ("object_type_mismatch", form_object_mismatch),
            ("object_action_not_allowlisted", raw_action_id),
            ("object_action_not_allowlisted", raw_policy),
            ("invalid_object_update_target", raw_target),
            ("invalid_object_update_input", raw_input_handle),
        ];

        for (expected_code, definition) in cases {
            let err = match normalize_create_request(create_request(definition, "leave_request")) {
                Ok(_) => {
                    return Err("browser-defined object/action scope must fail closed".to_owned());
                }
                Err(err) => err,
            };
            assert_eq!(err.status, StatusCode::UNPROCESSABLE_ENTITY);
            assert!(
                err.message.contains(expected_code),
                "expected validation message to contain {expected_code}, got {}",
                err.message
            );
        }

        Ok(())
    }

    #[cfg(not(feature = "test-postgres"))]
    #[test]
    fn accepts_leave_and_catalog_canonical_workflow_templates() -> Result<(), String> {
        let mut object_types = vec!["leave_request"];
        object_types.extend(
            WORKFLOW_TEMPLATES
                .iter()
                .map(|template| template.object_type),
        );

        for object_type in object_types {
            normalize_create_request(create_request(
                canonical_workflow_definition(object_type),
                object_type,
            ))
            .map_err(|err| err.message)?;
        }

        Ok(())
    }

    #[cfg(not(feature = "test-postgres"))]
    #[test]
    fn publish_validation_blocks_invalid_canonical_workflow_graphs() {
        let mut invalid_definition = canonical_workflow_definition("leave_request");
        invalid_definition["graph"]["edges"][0]["to_port"] = json!("missing_port");
        let row = workflow_row(invalid_definition, "leave_request");

        let findings = validate_publishable(&row);

        assert!(
            findings
                .iter()
                .any(|finding| finding.code == "invalid_edge_endpoint"),
            "expected invalid_edge_endpoint blocker, got {findings:?}"
        );
    }

    #[cfg(not(feature = "test-postgres"))]
    #[test]
    fn publish_validation_messages_include_stable_codes() {
        let mut invalid_definition = canonical_workflow_definition("leave_request");
        invalid_definition["graph"]["edges"][0]["to_port"] = json!("missing_port");
        let row = workflow_row(invalid_definition, "leave_request");

        let message = format_publish_findings(validate_publishable(&row));

        assert!(message.contains("invalid_edge_endpoint:"));
        assert!(!message.contains("node-trigger"));
        assert!(!message.contains("missing_port"));
    }

    #[cfg(not(feature = "test-postgres"))]
    #[test]
    fn publish_validation_messages_hide_action_allowlist_payload_values() {
        let mut row = workflow_row(
            canonical_workflow_definition("leave_request"),
            "leave_request",
        );
        row.action_allowlist = vec![json!({
            "connector_key": "external.random",
            "action_key": "post_secret"
        })];

        let message = format_publish_findings(validate_publishable(&row));

        assert!(message.contains("invalid_action_allowlist:"));
        assert!(!message.contains("external.random"));
        assert!(!message.contains("post_secret"));
    }

    fn policy_decision_definition() -> Value {
        let mut definition = canonical_workflow_definition("equipment");
        definition["policy_decision"] = json!({
                "template_key": "equipment_location_access",
                "effect": "allow",
                "action": "maintenance:StartWorkOrder",
                "resource": { "type": "equipment", "id": "EQ-BOILER-17" },
                "context": {
                    "org_id": "org_demo_001",
                    "location_id": "loc_plant_2",
                    "subject_role": "MAINTENANCE_MANAGER",
                    "passkey_step_up_satisfied": true
                },
                "scope": {
                    "org_id": "org_demo_001",
                    "location_id": "loc_plant_2"
                },
                "requirements": {
                    "passkey_step_up": true,
                    "audit_event": "workflow_definition.publish"
                }
        });
        definition
    }

    fn create_request(definition: Value, object_type: &str) -> CreateWorkflowDefinitionRequest {
        CreateWorkflowDefinitionRequest {
            workflow_key: format!("{object_type}.approval"),
            display_name: format!("{object_type} approval"),
            object_type: object_type.to_owned(),
            definition,
            approval_line: vec![],
            payment_line: vec![],
            notification_rules: vec![],
            action_allowlist: vec![json!({
                "connector_key": "internal.notifications",
                "action_key": "send_push"
            })],
            required_approval_line: false,
            required_payment_line: false,
        }
    }

    fn workflow_row(definition: Value, object_type: &str) -> WorkflowVersionRow {
        WorkflowVersionRow {
            definition_id: Uuid::new_v4(),
            workflow_key: format!("{object_type}.approval"),
            display_name: format!("{object_type} approval"),
            object_type: object_type.to_owned(),
            status: "DRAFT".to_owned(),
            latest_version: 1,
            active_version: None,
            definition,
            approval_line: vec![],
            payment_line: vec![],
            notification_rules: vec![],
            action_allowlist: vec![json!({
                "connector_key": "internal.notifications",
                "action_key": "send_push"
            })],
            required_approval_line: false,
            required_payment_line: false,
            pending_version: None,
            pending_staged_by: None,
            created_at: OffsetDateTime::UNIX_EPOCH,
            updated_at: OffsetDateTime::UNIX_EPOCH,
        }
    }

    fn canonical_workflow_definition(object_type: &str) -> Value {
        json!({
            "schema_version": "workflow.definition.v1",
            "metadata": {
                "name": format!("{object_type} approval"),
                "description": "Server validation fixture for a canonical no-code workflow.",
                "owner_scope": { "type": "org" },
                "object_type": object_type,
                "sensitivity": "summary_only",
                "tags": [object_type, "approval"],
                "locale": "ko-KR"
            },
            "graph": {
                "nodes": [
                    node("node-trigger", &format!("trigger.{object_type}.submitted"), "trigger.form_submission", "Submitted", vec![], vec![output_port("submitted", "flow.next", "Submitted")], json!({
                        "type": "trigger.form_submission",
                        "source": { "object_type": object_type, "event": "submitted", "scope": "org" },
                        "actor": { "required_policy": format!("{object_type}.submit") },
                        "idempotency": { "key_template": format!("{object_type}:{{object_id}}:submitted:{{version}}") }
                    })),
                    node("node-form", &format!("form.{object_type}"), "form.input", "Review form", vec![input_port("in", "flow.next", "In")], vec![output_port("completed", "flow.next", "Completed")], json!({
                        "type": "form.input",
                        "fields": [{
                            "key": format!("{object_type}_id"),
                            "label": "Request",
                            "field_type": "object_ref",
                            "object_type": object_type,
                            "required": true,
                            "sensitivity": "summary_only"
                        }],
                        "submit_label": "Submit"
                    })),
                    node("node-approval", &format!("task.{object_type}.approval"), "task.approval", "Approval", vec![input_port("in", "flow.next", "In")], vec![output_port("decision", "flow.next", "Decision")], json!({
                        "type": "task.approval",
                        "assignee_rule": { "kind": "role", "subject_field": format!("{object_type}_id"), "fallback_role": "operations.manager" },
                        "decision_options": ["approve", "reject", "request_change"],
                        "requires_comment_on": ["reject"],
                        "requires_evidence": false,
                        "prevent_self_approval": true,
                        "sla": { "duration": "P2D", "escalate_to": "operations.director" },
                        "requires_passkey_step_up": false,
                        "policy": [format!("approval_request.approve.{object_type}")]
                    })),
                    node("node-condition", &format!("condition.{object_type}.approval_result"), "condition.branch", "Approval result", vec![input_port("in", "flow.next", "Decision")], vec![branch_port("approved", "Approved"), branch_port("rejected", "Rejected")], json!({
                        "type": "condition.branch",
                        "expression": { "op": "equals", "left": { "ref": "approval.result" }, "right": "approved" },
                        "branches": [
                            { "port": "approved", "label": "Approved", "when": "true" },
                            { "port": "rejected", "label": "Rejected", "when": "false" }
                        ],
                        "default_port": "rejected"
                    })),
                    action_update_node(object_type, "approved"),
                    notification_node(object_type, "approved"),
                    audit_node(object_type, "approved"),
                    end_node("approved"),
                    action_update_node(object_type, "rejected"),
                    notification_node(object_type, "rejected"),
                    audit_node(object_type, "rejected"),
                    end_node("rejected")
                ],
                "edges": [
                    edge("edge-trigger-form", "node-trigger", "submitted", "node-form", "in", "control"),
                    edge("edge-form-approval", "node-form", "completed", "node-approval", "in", "control"),
                    edge("edge-approval-condition", "node-approval", "decision", "node-condition", "in", "control"),
                    edge("edge-condition-approved-update", "node-condition", "approved", "node-approved-update", "in", "decision"),
                    edge("edge-approved-update-notify", "node-approved-update", "done", "node-approved-notify", "in", "control"),
                    edge("edge-approved-notify-audit", "node-approved-notify", "done", "node-approved-audit", "in", "control"),
                    edge("edge-approved-audit-end", "node-approved-audit", "done", "node-end-approved", "in", "control"),
                    edge("edge-condition-rejected-update", "node-condition", "rejected", "node-rejected-update", "in", "decision"),
                    edge("edge-rejected-update-notify", "node-rejected-update", "done", "node-rejected-notify", "in", "control"),
                    edge("edge-rejected-notify-audit", "node-rejected-notify", "done", "node-rejected-audit", "in", "control"),
                    edge("edge-rejected-audit-end", "node-rejected-audit", "done", "node-end-rejected", "in", "control")
                ],
                "variables": [],
                "simulation_cases": [{ "key": "happy_path", "label": "Happy path" }]
            },
            "canvas": { "layout_version": "workflow.canvas.v1", "nodes": {}, "viewport": { "x": 0, "y": 0, "zoom": 1 } },
            "validation": { "last_result": "valid", "last_validated_at": null, "compiler_version": null }
        })
    }

    fn action_update_node(object_type: &str, status: &str) -> Value {
        node(
            &format!("node-{status}-update"),
            &format!("action.{object_type}.{status}_status"),
            "action.object_update",
            &format!("Set status {status}"),
            vec![input_port("in", "flow.next", "In")],
            vec![output_port("done", "flow.next", "Done")],
            json!({
                "type": "action.object_update",
                "action_id": format!("{object_type}.update_status"),
                "target": { "from": "trigger.object_ref" },
                "input": { "status": status, "updated_by": { "from": "approval.actor_id" }, "updated_at": { "from": "system.now" } },
                "idempotency": { "key_template": format!("{{run_id}}:{{node_key}}:{object_type}.update_status.{status}") },
                "requires_policy": format!("{object_type}.update_status")
            }),
        )
    }

    fn notification_node(object_type: &str, status: &str) -> Value {
        node(
            &format!("node-{status}-notify"),
            &format!("action.{object_type}.notify_{status}"),
            "action.notification",
            &format!("Notify {status}"),
            vec![input_port("in", "flow.next", "In")],
            vec![output_port("done", "flow.next", "Done")],
            json!({
                "type": "action.notification",
                "connector_key": "internal.notifications",
                "action_key": "send_push",
                "recipient": { "kind": "requester" },
                "template_key": format!("{object_type}.{status}"),
                "redaction": "summary_only",
                "link": { "object_ref": "trigger.object_ref" }
            }),
        )
    }

    fn audit_node(object_type: &str, status: &str) -> Value {
        node(
            &format!("node-{status}-audit"),
            &format!("action.{object_type}.audit_{status}"),
            "action.audit_append",
            &format!("Audit {status}"),
            vec![input_port("in", "flow.next", "In")],
            vec![output_port("done", "flow.next", "Done")],
            json!({
                "type": "action.audit_append",
                "event_key": format!("{object_type}.workflow.{status}"),
                "summary_template": format!("{object_type} workflow completed with {status}."),
                "redaction": "summary_only"
            }),
        )
    }

    fn end_node(status: &str) -> Value {
        node(
            &format!("node-end-{status}"),
            &format!("end.{status}"),
            "end.state",
            &format!("{status} end"),
            vec![input_port("in", "flow.terminal", "In")],
            vec![],
            json!({ "type": "end.state", "status": status }),
        )
    }

    fn node(
        id: &str,
        key: &str,
        node_type: &str,
        label: &str,
        input_ports: Vec<Value>,
        output_ports: Vec<Value>,
        config: Value,
    ) -> Value {
        json!({
            "id": id,
            "key": key,
            "type": node_type,
            "label": label,
            "version": 1,
            "input_ports": input_ports,
            "output_ports": output_ports,
            "config": config,
            "policy": [],
            "data_sensitivity": "summary_only",
            "execution": {}
        })
    }

    fn input_port(key: &str, port_type: &str, label: &str) -> Value {
        port(key, "input", port_type, label)
    }

    fn output_port(key: &str, port_type: &str, label: &str) -> Value {
        port(key, "output", port_type, label)
    }

    fn branch_port(key: &str, label: &str) -> Value {
        port(key, "output", "flow.branch", label)
    }

    fn port(key: &str, direction: &str, port_type: &str, label: &str) -> Value {
        json!({ "key": key, "direction": direction, "type": port_type, "required": true, "cardinality": "one", "label": label })
    }

    fn edge(
        id: &str,
        from_node_id: &str,
        from_port: &str,
        to_node_id: &str,
        to_port: &str,
        kind: &str,
    ) -> Value {
        json!({ "id": id, "from_node_id": from_node_id, "from_port": from_port, "to_node_id": to_node_id, "to_port": to_port, "kind": kind })
    }

    fn policy_row(definition: Value) -> WorkflowVersionRow {
        WorkflowVersionRow {
            definition_id: Uuid::new_v4(),
            workflow_key: "equipment.equipment_location_access_policy".to_owned(),
            display_name: "Equipment Location Access".to_owned(),
            object_type: "equipment".to_owned(),
            status: "DRAFT".to_owned(),
            latest_version: 1,
            active_version: None,
            definition,
            approval_line: vec![json!({
                "step_key": "manager",
                "approver_role": "MAINTENANCE_MANAGER",
                "required": true
            })],
            payment_line: vec![],
            notification_rules: vec![],
            action_allowlist: vec![json!({
                "connector_key": "internal.audit",
                "action_key": "append_timeline_event"
            })],
            required_approval_line: true,
            required_payment_line: false,
            pending_version: None,
            pending_staged_by: None,
            created_at: OffsetDateTime::UNIX_EPOCH,
            updated_at: OffsetDateTime::UNIX_EPOCH,
        }
    }

    #[cfg(not(feature = "test-postgres"))]
    #[test]
    fn policy_decision_blocks_unsupported_templates() -> Result<(), String> {
        let mut definition = policy_decision_definition();
        definition["policy_decision"]["template_key"] = json!("approve_work_order");
        definition["policy_decision"]["action"] = json!("maintenance:ApproveWorkOrder");
        definition["policy_decision"]["resource"] = json!({ "type": "work_order", "id": "WO-17" });

        let err = match validate_definition_object(definition) {
            Ok(_) => return Err("unsupported policy template must fail closed".to_owned()),
            Err(err) => err,
        };

        assert_eq!(err.status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(err.message.contains("equipment_location_access"));
        Ok(())
    }

    #[cfg(not(feature = "test-postgres"))]
    #[test]
    fn definition_schema_version_is_required() -> Result<(), String> {
        let err = match validate_definition_object(json!({ "trigger": "work_order.completed" })) {
            Ok(_) => return Err("missing schema_version must fail closed".to_owned()),
            Err(err) => err,
        };

        assert_eq!(err.status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(err.message.contains("schema_version"));
        Ok(())
    }

    #[cfg(not(feature = "test-postgres"))]
    #[test]
    fn definition_schema_version_must_match_supported_version() -> Result<(), String> {
        let err = match validate_definition_object(json!({
            "schema_version": "workflow.definition.v0",
            "trigger": "work_order.completed"
        })) {
            Ok(_) => return Err("mismatched schema_version must fail closed".to_owned()),
            Err(err) => err,
        };

        assert_eq!(err.status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(err.message.contains(WORKFLOW_DEFINITION_SCHEMA_VERSION));
        Ok(())
    }

    #[cfg(not(feature = "test-postgres"))]
    #[test]
    fn policy_decision_simulation_surfaces_cedar_pbac_tuple() {
        let simulation = simulation_for(&policy_row(policy_decision_definition()));

        assert_eq!(simulation.decision, "ready");
        assert!(simulation.findings.iter().any(|finding| {
            finding.code == "policy_decision_metadata"
                && finding.message.contains("maintenance:StartWorkOrder")
                && finding.message.contains("equipment")
                && finding.message.contains("subject_role")
        }));
    }

    #[cfg(not(feature = "test-postgres"))]
    #[test]
    fn policy_decision_metadata_requires_valid_schema() {
        let mut definition = policy_decision_definition();
        definition["schema_version"] = json!("workflow.definition.v0");

        let simulation = simulation_for(&policy_row(definition));

        assert_eq!(simulation.decision, "blocked");
        assert!(simulation.findings.iter().any(|finding| {
            finding.code == "schema_version" && finding.message.contains("schema_version")
        }));
        assert!(
            simulation
                .findings
                .iter()
                .all(|finding| finding.code != "policy_decision_metadata")
        );
    }

    #[cfg(not(feature = "test-postgres"))]
    #[test]
    fn simulation_surfaces_non_blocking_findings() {
        let mut row = policy_row(policy_decision_definition());
        row.action_allowlist = vec![];

        let simulation = simulation_for(&row);

        assert_eq!(simulation.decision, "ready");
        assert!(
            simulation
                .findings
                .iter()
                .any(|finding| finding.code == "empty_action_allowlist")
        );
    }

    #[cfg(not(feature = "test-postgres"))]
    #[test]
    fn policy_decision_missing_scope_blocks_publish() -> Result<(), String> {
        let mut definition = policy_decision_definition();
        definition["policy_decision"]
            .as_object_mut()
            .ok_or_else(|| "policy_decision fixture must be an object".to_owned())?
            .remove("scope");

        let findings = validate_publishable(&policy_row(definition));

        assert!(findings.iter().any(|finding| {
            finding.code == "invalid_policy_decision" && finding.message.contains("scope")
        }));
        Ok(())
    }

    #[cfg(not(feature = "test-postgres"))]
    #[test]
    fn draft_update_merges_partial_payload_without_mutating_identity() -> Result<(), String> {
        let current = policy_row(policy_decision_definition());
        let next = apply_draft_update(
            &current,
            NormalizedWorkflowDefinitionUpdate {
                display_name: Some("Updated policy draft".to_owned()),
                definition: None,
                approval_line: Some(vec![json!({
                    "step_key": "owner",
                    "approver_role": "MAINTENANCE_MANAGER",
                    "required": true
                })]),
                payment_line: None,
                notification_rules: Some(vec![]),
                action_allowlist: None,
                required_approval_line: Some(false),
                required_payment_line: None,
            },
        )
        .map_err(|err| err.message)?;

        assert_eq!(next.definition_id, current.definition_id);
        assert_eq!(next.workflow_key, current.workflow_key);
        assert_eq!(next.object_type, current.object_type);
        assert_eq!(next.display_name, "Updated policy draft");
        assert_eq!(next.approval_line.len(), 1);
        assert!(next.notification_rules.is_empty());
        assert!(!next.required_approval_line);
        assert_eq!(next.definition, current.definition);
        Ok(())
    }

    fn edit(display: &str) -> NormalizedWorkflowDefinitionUpdate {
        NormalizedWorkflowDefinitionUpdate {
            display_name: Some(display.to_owned()),
            definition: None,
            approval_line: None,
            payment_line: None,
            notification_rules: None,
            action_allowlist: None,
            required_approval_line: None,
            required_payment_line: None,
        }
    }

    #[cfg(not(feature = "test-postgres"))]
    #[test]
    fn active_definition_is_editable_as_a_staged_revision() -> Result<(), String> {
        // pendingRev: editing a LIVE (ACTIVE) definition is allowed — it stages a
        // DRAFT revision while the active version keeps serving.
        let mut current = policy_row(policy_decision_definition());
        current.status = "ACTIVE".to_owned();
        current.active_version = Some(1);
        let next = apply_draft_update(&current, edit("Staged revision")).map_err(|e| e.message)?;
        assert_eq!(next.display_name, "Staged revision");
        Ok(())
    }

    #[cfg(not(feature = "test-postgres"))]
    #[test]
    fn retired_definition_is_not_editable() -> Result<(), String> {
        let mut current = policy_row(policy_decision_definition());
        current.status = "RETIRED".to_owned();
        let err = apply_draft_update(&current, edit("Cannot edit"))
            .err()
            .ok_or_else(|| "retired definitions must not be editable".to_owned())?;
        assert_eq!(err.code, "invalid_transition");
        Ok(())
    }

    #[cfg(not(feature = "test-postgres"))]
    #[test]
    fn definition_with_pending_revision_is_not_editable() -> Result<(), String> {
        let mut current = policy_row(policy_decision_definition());
        current.status = "ACTIVE".to_owned();
        current.active_version = Some(1);
        current.pending_version = Some(2);
        let err = apply_draft_update(&current, edit("Second edit"))
            .err()
            .ok_or_else(|| "a pending revision must block further edits".to_owned())?;
        assert_eq!(err.status, StatusCode::CONFLICT);
        Ok(())
    }

    #[cfg(not(feature = "test-postgres"))]
    #[test]
    fn draft_update_rejects_empty_payload() -> Result<(), String> {
        let err = match normalize_update_request(UpdateWorkflowDefinitionRequest {
            display_name: None,
            definition: None,
            approval_line: None,
            payment_line: None,
            notification_rules: None,
            action_allowlist: None,
            required_approval_line: None,
            required_payment_line: None,
        }) {
            Ok(_) => return Err("empty updates must not append no-op draft versions".to_owned()),
            Err(err) => err,
        };

        assert_eq!(err.status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(err.code, "validation");
        Ok(())
    }

    #[cfg(not(feature = "test-postgres"))]
    #[test]
    fn retired_definitions_cannot_take_sensitive_lifecycle_actions() -> Result<(), String> {
        let mut current = policy_row(policy_decision_definition());
        current.status = "RETIRED".to_owned();

        let err = match ensure_not_retired(&current) {
            Ok(()) => return Err("retired definitions must fail closed".to_owned()),
            Err(err) => err,
        };

        assert_eq!(err.status, StatusCode::CONFLICT);
        assert_eq!(err.code, "invalid_transition");
        Ok(())
    }
}
