use std::collections::{BTreeMap, BTreeSet};
use std::io::Cursor;

use axum::extract::{DefaultBodyLimit, Multipart, Path, Query, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Extension, Json, Router};
use calamine::{Data, DataType, Reader, Xlsx};
use console_governance_domain::{GateChainConfig, GateEvidence, evaluate_gate_chain};
use console_kernel_core::{
    AuditAction, AuditEvent, BranchId, BranchScope, ErrorKind, KernelError, OrgId, TraceContext,
    UserId,
};
use console_leave_adapter_postgres::{PgLeaveError, PgLeaveStore};
use console_leave_domain::LeaveBalanceAmount;
// `ObjectKey::Employment`'s owner. Every `employees` statement this module used
// to hold lives there now; console-app is no longer a second writer of it.
use console_ontology_canonical_adapter_postgres::employment;
use console_ontology_canonical_adapter_postgres::person::{
    PersonCommand, PersonError, write_in_tx as write_person_in_tx,
};
use console_payroll_domain::{
    ProfessionalReviewerKind, ProfessionalValidation, SeverancePayInput, build_severance_pay_draft,
    moel_retirement_pay_source, nhis_qualification_loss_form_source,
};
use console_platform_auth::JwtVerifier;
use console_platform_authz::{
    Action, Feature, Principal, Role, authorize, authorize_capability, authorize_org_wide,
};
use console_platform_db::{DbError, with_audit, with_audits, with_org_conn};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Postgres, QueryBuilder, Row};
use time::{Date, Month, OffsetDateTime};
use uuid::Uuid;

pub const EMPLOYEES_PATH: &str = "/api/v1/employees";
pub const EMPLOYEE_DETAIL_PATH_TEMPLATE: &str = "/api/v1/employees/{id}";
pub const EMPLOYEES_IMPORT_PATH: &str = "/api/v1/employees/import";
pub const EMPLOYEES_IMPORT_PREVIEW_PATH: &str = "/api/v1/employees/import/preview";
pub const EMPLOYEES_IMPORT_DRY_RUN_PATH_TEMPLATE: &str =
    "/api/v1/employees/import/{run_id}/dry-run";
pub const EMPLOYEES_IMPORT_APPLY_PATH_TEMPLATE: &str = "/api/v1/employees/import/{run_id}/apply";
pub const EMPLOYEES_EXPORT_CSV_PATH: &str = "/api/v1/employees/export.csv";
pub const EMPLOYEE_LIFECYCLE_EVENTS_PATH_TEMPLATE: &str = "/api/v1/employees/{id}/lifecycle-events";
pub const EMPLOYEE_HOME_BRANCH_PATH_TEMPLATE: &str = "/api/v1/employees/{id}/home-branch";
pub const HR_ORG_CHART_PATH: &str = "/api/v1/hr/org-chart";
pub const HR_LEAVE_BALANCES_PATH: &str = "/api/v1/hr/leave-balances";
pub const HR_ATTENDANCE_SUMMARY_PATH: &str = "/api/v1/hr/attendance-summary";
pub const HR_READINESS_SUMMARY_PATH: &str = "/api/v1/hr/readiness-summary";
pub const HR_ATTENDANCE_IMPORT_PREVIEW_PATH: &str = "/api/v1/hr/attendance-import/preview";
pub const HR_ATTENDANCE_IMPORT_DRY_RUN_PATH_TEMPLATE: &str =
    "/api/v1/hr/attendance-import/{run_id}/dry-run";
pub const HR_ATTENDANCE_IMPORT_APPLY_PATH_TEMPLATE: &str =
    "/api/v1/hr/attendance-import/{run_id}/apply";
pub const HR_ATTENDANCE_IMPORT_SUMMARY_PATH: &str = "/api/v1/hr/attendance-import/summary";
pub const HR_MY_ATTENDANCE_RECORDS_PATH: &str = "/api/v1/hr/attendance-records/me";
pub const HR_ATTENDANCE_RECORDS_PATH: &str = "/api/v1/hr/attendance-records";
pub const HR_ABSENCE_EXIT_DASHBOARD_PATH: &str = "/api/v1/hr/absence-exit-dashboard";
pub const HR_EXIT_CASES_PATH: &str = "/api/v1/hr/exit-cases";
pub const HR_EXIT_CASE_CONFIRM_PATH_TEMPLATE: &str = "/api/v1/hr/exit-cases/{id}/confirm";
pub const HR_EXIT_CASE_APPROVAL_DRAFT_PATH_TEMPLATE: &str =
    "/api/v1/hr/exit-cases/{id}/approval-draft";
pub const HR_ROUTE_PATHS: &[&str] = &[
    EMPLOYEES_PATH,
    EMPLOYEE_DETAIL_PATH_TEMPLATE,
    EMPLOYEES_IMPORT_PATH,
    EMPLOYEES_IMPORT_PREVIEW_PATH,
    EMPLOYEES_IMPORT_DRY_RUN_PATH_TEMPLATE,
    EMPLOYEES_IMPORT_APPLY_PATH_TEMPLATE,
    EMPLOYEES_EXPORT_CSV_PATH,
    EMPLOYEE_LIFECYCLE_EVENTS_PATH_TEMPLATE,
    EMPLOYEE_HOME_BRANCH_PATH_TEMPLATE,
    HR_ORG_CHART_PATH,
    HR_LEAVE_BALANCES_PATH,
    HR_ATTENDANCE_SUMMARY_PATH,
    HR_READINESS_SUMMARY_PATH,
    HR_ATTENDANCE_IMPORT_PREVIEW_PATH,
    HR_ATTENDANCE_IMPORT_DRY_RUN_PATH_TEMPLATE,
    HR_ATTENDANCE_IMPORT_APPLY_PATH_TEMPLATE,
    HR_ATTENDANCE_IMPORT_SUMMARY_PATH,
    HR_MY_ATTENDANCE_RECORDS_PATH,
    HR_ATTENDANCE_RECORDS_PATH,
    HR_ABSENCE_EXIT_DASHBOARD_PATH,
    HR_EXIT_CASES_PATH,
    HR_EXIT_CASE_CONFIRM_PATH_TEMPLATE,
    HR_EXIT_CASE_APPROVAL_DRAFT_PATH_TEMPLATE,
];
const MAX_IMPORT_BYTES: usize = 16 * 1024 * 1024;
const MAX_IMPORT_HEADER_SCAN_ROWS: usize = 25;
const DEFAULT_LIMIT: i64 = 500;
const MAX_LIMIT: i64 = 1000;

#[derive(Debug, Clone)]
pub struct HrState {
    pool: PgPool,
    leave_command_store: Option<PgLeaveStore>,
    jwt_verifier: Option<JwtVerifier>,
}

impl HrState {
    #[must_use]
    pub fn new(pool: PgPool, jwt_verifier: Option<JwtVerifier>) -> Self {
        Self {
            pool,
            leave_command_store: None,
            jwt_verifier,
        }
    }

    #[must_use]
    pub fn with_leave_command_store(mut self, store: PgLeaveStore) -> Self {
        self.leave_command_store = Some(store);
        self
    }
}

pub fn router(state: HrState) -> Router {
    let verifier = state.jwt_verifier.clone();
    let pool = state.pool.clone();
    let router = Router::new()
        .route(EMPLOYEES_PATH, get(list_employees).post(create_employee))
        .route(EMPLOYEE_DETAIL_PATH_TEMPLATE, get(get_employee_detail))
        .route(HR_ORG_CHART_PATH, get(get_hr_org_chart))
        .route(HR_LEAVE_BALANCES_PATH, get(list_leave_balances))
        .route(HR_ATTENDANCE_SUMMARY_PATH, get(list_attendance_summary))
        .route(HR_READINESS_SUMMARY_PATH, get(get_hr_readiness_summary))
        .route(
            HR_ABSENCE_EXIT_DASHBOARD_PATH,
            get(get_absence_exit_dashboard),
        )
        .route(HR_EXIT_CASES_PATH, post(report_employee_exit_case))
        .route(
            HR_EXIT_CASE_CONFIRM_PATH_TEMPLATE,
            post(confirm_employee_exit_case),
        )
        .route(
            HR_EXIT_CASE_APPROVAL_DRAFT_PATH_TEMPLATE,
            post(draft_employee_exit_approval),
        )
        .route(
            HR_ATTENDANCE_IMPORT_PREVIEW_PATH,
            post(preview_attendance_import).layer(DefaultBodyLimit::max(MAX_IMPORT_BYTES)),
        )
        .route(
            HR_ATTENDANCE_IMPORT_DRY_RUN_PATH_TEMPLATE,
            post(dry_run_attendance_import),
        )
        .route(
            HR_ATTENDANCE_IMPORT_APPLY_PATH_TEMPLATE,
            post(apply_attendance_import),
        )
        .route(
            HR_ATTENDANCE_IMPORT_SUMMARY_PATH,
            get(list_attendance_import_summary),
        )
        .route(
            HR_MY_ATTENDANCE_RECORDS_PATH,
            get(list_my_attendance_records).post(create_my_attendance_record),
        )
        .route(HR_ATTENDANCE_RECORDS_PATH, get(list_attendance_records))
        .route(
            EMPLOYEES_IMPORT_PATH,
            post(import_employees).layer(DefaultBodyLimit::max(MAX_IMPORT_BYTES)),
        )
        .route(
            EMPLOYEES_IMPORT_PREVIEW_PATH,
            post(preview_employee_import).layer(DefaultBodyLimit::max(MAX_IMPORT_BYTES)),
        )
        .route(
            EMPLOYEES_IMPORT_DRY_RUN_PATH_TEMPLATE,
            post(dry_run_employee_import),
        )
        .route(
            EMPLOYEES_IMPORT_APPLY_PATH_TEMPLATE,
            post(apply_employee_import),
        )
        .route(EMPLOYEES_EXPORT_CSV_PATH, get(export_employees_csv))
        .route(
            EMPLOYEE_LIFECYCLE_EVENTS_PATH_TEMPLATE,
            get(list_employee_lifecycle_events).post(create_employee_lifecycle_event),
        )
        .route(
            EMPLOYEE_HOME_BRANCH_PATH_TEMPLATE,
            put(set_employee_home_branch),
        )
        .with_state(state);
    console_platform_request_context::with_request_context(router, verifier, pool)
}

#[derive(Debug, Deserialize)]
struct EmployeeListQuery {
    company: Option<String>,
    search: Option<String>,
    home_branch_review_required: Option<bool>,
    limit: Option<i64>,
    offset: Option<i64>,
}

#[derive(Debug, Serialize)]
struct EmployeePage {
    items: Vec<EmployeeResponse>,
    total: i64,
    limit: i64,
    offset: i64,
}

#[derive(Debug, Serialize)]
struct EmployeeResponse {
    id: Uuid,
    company: String,
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    employee_number: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    org_unit: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    worksite_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    worksite: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    job: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    position: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    hire_date: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    exit_date: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    leave_accrued: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    leave_used: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    leave_remaining: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    home_branch_id: Option<Uuid>,
    #[serde(skip_serializing_if = "Option::is_none")]
    home_branch_name: Option<String>,
    home_branch_review_required: bool,
    identity_resolution_strategy: String,
    identity_resolution_confidence: String,
    identity_review_required: bool,
    identity_name_only_merge: bool,
    created_at: time::OffsetDateTime,
    updated_at: time::OffsetDateTime,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CreateEmployeeRequest {
    pub(crate) employee_number: String,
    pub(crate) name: String,
    pub(crate) company: String,
    pub(crate) employment_type: String,
    pub(crate) phone: String,
    pub(crate) org_unit: String,
    pub(crate) position: String,
    pub(crate) site: String,
    pub(crate) home_branch_id: Uuid,
    pub(crate) base_pay: String,
    pub(crate) idempotency_key: String,
}

#[derive(Debug, Serialize)]
struct EmployeeEmploymentDetail {
    employment_type: String,
    phone_e164: String,
    base_pay: String,
    currency: String,
}

#[derive(Debug, Serialize)]
pub(crate) struct EmployeeDetailResponse {
    employee: EmployeeResponse,
    employment: EmployeeEmploymentDetail,
}

impl EmployeeDetailResponse {
    /// The created (or replayed) employee id, for callers that link the
    /// employee object without re-reading the row.
    pub(crate) fn employee_id(&self) -> Uuid {
        self.employee.id
    }

    pub(crate) fn employee_home_branch_id(&self) -> Option<Uuid> {
        self.employee.home_branch_id
    }

    /// CAS token for the command-side home-branch assignment.
    pub(crate) fn employee_updated_at(&self) -> time::OffsetDateTime {
        self.employee.updated_at
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SetEmployeeHomeBranchRequest {
    branch_id: Uuid,
    /// Optimistic concurrency precondition from the employee list/read model.
    expected_updated_at: String,
}

#[derive(Debug, Serialize)]
struct EmployeeHomeBranchResponse {
    employee_id: Uuid,
    branch_id: Uuid,
    branch_name: String,
    updated_at: OffsetDateTime,
}

#[derive(Debug, Serialize)]
struct HrOrgChartResponse {
    companies: Vec<HrOrgChartCompany>,
}

#[derive(Debug, Serialize)]
struct HrOrgChartCompany {
    company: String,
    total: i64,
    active: i64,
    units: Vec<HrOrgChartUnit>,
}

#[derive(Debug, Serialize)]
struct HrOrgChartUnit {
    name: String,
    total: i64,
    positions: Vec<HrOrgChartPosition>,
}

#[derive(Debug, Serialize)]
struct HrOrgChartPosition {
    title: String,
    total: i64,
    employees: Vec<HrOrgChartEmployee>,
}

#[derive(Debug, Serialize)]
struct HrOrgChartEmployee {
    id: Uuid,
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    employee_number: Option<String>,
    status: String,
}

#[derive(Debug, Serialize)]
struct LeaveBalancePage {
    items: Vec<LeaveBalanceItem>,
    total: i64,
    limit: i64,
    offset: i64,
    summary: LeaveBalanceSummary,
}

#[derive(Debug, Serialize)]
struct LeaveBalanceItem {
    id: Uuid,
    company: String,
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    employee_number: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    org_unit: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    position: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    leave_accrued: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    leave_used: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    leave_remaining: Option<String>,
}

#[derive(Debug, Serialize)]
struct LeaveBalanceSummary {
    accrued: String,
    used: String,
    remaining: String,
}

#[derive(Debug, Serialize)]
struct AttendanceSummaryPage {
    items: Vec<AttendanceSummaryItem>,
    total: i64,
    limit: i64,
    offset: i64,
}

#[derive(Debug, Serialize)]
struct AttendanceSummaryItem {
    user_id: Uuid,
    display_name: String,
    arrivals: i64,
    departures: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_event_at: Option<time::OffsetDateTime>,
}
#[derive(Debug, Deserialize)]
struct AttendanceSummaryQuery {
    limit: Option<i64>,
    offset: Option<i64>,
    branch_id: Option<Uuid>,
}

#[derive(Debug, Deserialize)]
struct AttendanceManagerRecordsQuery {
    limit: Option<i64>,
    offset: Option<i64>,
    employee_id: Option<Uuid>,
    branch_id: Option<Uuid>,
}

#[derive(Debug, Deserialize)]
struct AttendanceSelfRecordsQuery {
    limit: Option<i64>,
    offset: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct CreateEmployeeAttendanceRecordRequest {
    kind: String,
    idempotency_key: String,
    #[serde(default)]
    note: Option<String>,
}

#[derive(Debug, Serialize)]
struct EmployeeAttendanceRecordPage {
    items: Vec<EmployeeAttendanceRecordResponse>,
    total: i64,
    limit: i64,
    offset: i64,
}

#[derive(Debug, Clone, Serialize)]
struct EmployeeAttendanceRecordResponse {
    id: Uuid,
    employee_id: Uuid,
    employee_display_name: String,
    kind: String,
    occurred_at: time::OffsetDateTime,
    work_date: String,
    state_after: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    note: Option<String>,
    payroll_material_ref_id: Uuid,
    payroll_link_status: String,
    duplicate: bool,
}

#[derive(Debug, Clone)]
struct LinkedEmployee {
    employee_id: Uuid,
    display_name: String,
}

#[derive(Debug, Serialize)]
struct HrReadinessSummary {
    imports: HrImportReadinessSummary,
    payroll: HrPayrollReadinessSummary,
    annual_leave: HrAnnualLeaveReadinessSummary,
    attendance: HrAttendanceReadinessSummary,
}

#[derive(Debug, Serialize)]
struct HrImportReadinessSummary {
    runs: i64,
    applied_runs: i64,
    input_rows: i64,
    candidate_rows: i64,
    preserved_rows: i64,
    ledger_rows: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    latest_import_at: Option<time::OffsetDateTime>,
}

#[derive(Debug, Serialize)]
struct HrPayrollReadinessSummary {
    draft_runs: i64,
    blocked_runs: i64,
    calculation_enabled_runs: i64,
    active_close_runs: i64,
    draft_lines: i64,
    payroll_source_rows: i64,
    attendance_source_rows: i64,
    attendance_event_links: i64,
    attendance_material_refs: i64,
    gross_pay_source_lines: i64,
    net_pay_source_lines: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    latest_status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    latest_source_label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    latest_period_start: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    latest_period_end: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    latest_updated_at: Option<time::OffsetDateTime>,
}

#[derive(Debug, Serialize)]
struct HrAnnualLeaveReadinessSummary {
    obligations: i64,
    usage_promotion_required: i64,
    payout_review_required: i64,
    needs_review: i64,
    remaining_days: String,
}

#[derive(Debug, Serialize)]
struct HrAttendanceReadinessSummary {
    durable_events: i64,
    self_service_records: i64,
    payroll_material_refs: i64,
}

#[derive(Debug, Deserialize)]
struct CreateEmployeeLifecycleEventRequest {
    event_type: String,
    #[serde(default)]
    to_status: Option<String>,
    #[serde(default)]
    to_company: Option<String>,
    #[serde(default)]
    to_org_unit: Option<String>,
    #[serde(default)]
    to_position: Option<String>,
    effective_date: String,
    comment: String,
    signoffs: EmployeeLifecycleSignoffs,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
struct EmployeeLifecycleSignoffs {
    #[serde(default)]
    privacy_notice_ack: bool,
    #[serde(default)]
    korean_labor_law_ack: bool,
    #[serde(default)]
    payroll_cutoff_ack: bool,
    #[serde(default)]
    retirement_settlement_ack: bool,
}

#[derive(Debug, Serialize)]
struct EmployeeLifecycleEventPage {
    items: Vec<EmployeeLifecycleEventResponse>,
}

#[derive(Debug, Serialize)]
struct EmployeeLifecycleEventResponse {
    id: Uuid,
    employee_id: Uuid,
    event_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    from_status: Option<String>,
    to_status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    from_company: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    to_company: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    from_org_unit: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    to_org_unit: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    from_position: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    to_position: Option<String>,
    effective_date: String,
    comment: String,
    signoffs: EmployeeLifecycleSignoffs,
    created_by: Uuid,
    created_at: time::OffsetDateTime,
}

#[derive(Debug)]
struct EmployeeForLifecycle {
    company: String,
    org_unit: Option<String>,
    position: Option<String>,
    employment_status: String,
}

#[derive(Debug, Deserialize)]
struct HrListQuery {
    limit: Option<i64>,
    offset: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct AbsenceExitDashboardQuery {
    limit: Option<i64>,
    offset: Option<i64>,
    #[serde(default)]
    employee_id: Option<Uuid>,
}

#[derive(Debug, Serialize)]
struct AbsenceExitDashboardResponse {
    summary: AbsenceExitSummary,
    alerts: Vec<EmployeeAbsenceAlertResponse>,
    exit_cases: Vec<EmployeeExitCaseResponse>,
}

#[derive(Debug, Serialize)]
struct AbsenceExitSummary {
    open_absence_alerts: i64,
    exit_cases_pending_hr: i64,
    settlement_needs_source: i64,
    settlement_ready: i64,
    approval_drafts: i64,
    submitted: i64,
}

#[derive(Debug, Serialize)]
struct EmployeeAbsenceAlertResponse {
    id: Uuid,
    employee_id: Uuid,
    employee_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    employee_number: Option<String>,
    company: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    org_unit: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    worksite_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    branch_id: Option<Uuid>,
    #[serde(skip_serializing_if = "Option::is_none")]
    branch_name: Option<String>,
    work_date: String,
    source: String,
    status: String,
    severity: String,
    audience_roles: Vec<String>,
    signal_payload: Value,
    notification_title: String,
    notification_message: String,
    link_href: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    exit_case_id: Option<Uuid>,
    detected_at: OffsetDateTime,
}

#[derive(Debug, Deserialize)]
struct ReportEmployeeExitCaseRequest {
    employee_id: Uuid,
    #[serde(default)]
    branch_id: Option<Uuid>,
    #[serde(default)]
    absence_alert_id: Option<Uuid>,
    effective_exit_date: String,
    site_manager_note: String,
}

#[derive(Debug, Deserialize)]
struct ConfirmEmployeeExitCaseRequest {
    #[serde(default)]
    decision: Option<String>,
    #[serde(default)]
    hq_confirmation: bool,
    #[serde(default)]
    note: Option<String>,
    #[serde(default)]
    settlement_input: Option<ExitSettlementInput>,
}

#[derive(Debug, Deserialize)]
struct DraftEmployeeExitApprovalRequest {
    #[serde(default)]
    submit: bool,
    #[serde(default)]
    note: Option<String>,
    #[serde(default)]
    settlement_input: Option<ExitSettlementInput>,
}

#[derive(Debug, Clone, Deserialize)]
struct ExitSettlementInput {
    average_wage_period_start: String,
    average_wage_period_end: String,
    average_wage_calendar_days: i64,
    average_wage_total_won: i64,
    /// Monthly 통상임금 (ordinary wage) in won, gathered alongside the average-wage
    /// source fields. Mandatory (no `#[serde(default)]`): the request fails to
    /// deserialize when absent, so the statutory 통상임금 floor can never be
    /// silently skipped for the depressed-window absence→exit population.
    monthly_ordinary_wage_won: i64,
}

#[derive(Debug, Serialize)]
struct EmployeeExitCaseResponse {
    id: Uuid,
    employee_id: Uuid,
    employee_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    employee_number: Option<String>,
    company: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    org_unit: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    worksite_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    branch_id: Option<Uuid>,
    #[serde(skip_serializing_if = "Option::is_none")]
    branch_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    absence_alert_id: Option<Uuid>,
    status: String,
    effective_exit_date: String,
    site_manager_note: String,
    reported_by: Uuid,
    reported_at: OffsetDateTime,
    #[serde(skip_serializing_if = "Option::is_none")]
    hr_confirmed_by: Option<Uuid>,
    #[serde(skip_serializing_if = "Option::is_none")]
    hr_confirmed_at: Option<OffsetDateTime>,
    #[serde(skip_serializing_if = "Option::is_none")]
    hq_confirmed_by: Option<Uuid>,
    #[serde(skip_serializing_if = "Option::is_none")]
    hq_confirmed_at: Option<OffsetDateTime>,
    #[serde(skip_serializing_if = "Option::is_none")]
    approval_submitted_by: Option<Uuid>,
    #[serde(skip_serializing_if = "Option::is_none")]
    approval_submitted_at: Option<OffsetDateTime>,
    #[serde(skip_serializing_if = "Option::is_none")]
    settlement_package: Option<EmployeeExitSettlementPackageResponse>,
    next_actions: Vec<ExitCaseNextAction>,
}

#[derive(Debug, Serialize)]
struct EmployeeExitSettlementPackageResponse {
    id: Uuid,
    status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    service_days: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    average_wage_period_start: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    average_wage_period_end: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    average_wage_calendar_days: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    average_wage_total_won: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    average_daily_wage_milliwon: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    severance_pay_won: Option<i64>,
    /// 통상임금 (ordinary-wage) statutory basis (0093 review HIGH #2): the monthly
    /// ordinary wage a reviewer signs, the 통상일급 derived from it via the 209h/8h
    /// rule, and the daily wage that actually governed severance = max(average,
    /// ordinary). Persisted, digest-covered, and exposed so the money trail is
    /// auditable.
    #[serde(skip_serializing_if = "Option::is_none")]
    monthly_ordinary_wage_won: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    ordinary_daily_wage_won: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    statutory_daily_wage_milliwon: Option<i64>,
    missing_source_fields: Vec<String>,
    statutory_basis: Value,
    /// The generated MOEL/NHIS insurance-loss document. Also carries a
    /// `certification_status` key (stamped by `exit_case_from_row`, mirroring
    /// the field below) so the document itself never omits the uncertified
    /// marker even if forwarded independently of this DTO.
    insurance_loss_payload: Value,
    /// The generated approval-submission document. Same
    /// `certification_status` stamping as `insurance_loss_payload`.
    approval_payload: Value,
    /// EFFECTIVE certification state (single source for the "산정 초안 — 노무사
    /// 검증 전" label): reported `CERTIFIED` only when the stored
    /// `certification_status` is CERTIFIED AND the stored digest still binds the
    /// current numbers; a stale/absent digest is reported `UNCERTIFIED_DRAFT`.
    certification_status: String,
    generated_at: OffsetDateTime,
    #[serde(skip_serializing_if = "Option::is_none")]
    submitted_by: Option<Uuid>,
    #[serde(skip_serializing_if = "Option::is_none")]
    submitted_at: Option<OffsetDateTime>,
}

#[derive(Debug, Serialize)]
struct ExitCaseNextAction {
    key: String,
    label: String,
    href: String,
}

async fn list_employees(
    State(state): State<HrState>,
    Extension(principal): Extension<Principal>,
    Query(query): Query<EmployeeListQuery>,
) -> Result<Json<EmployeePage>, HrError> {
    authorize_hr_org_wide(&principal, Feature::EmployeeDirectoryRead)?;
    record_hr_read("employees");
    let org = principal.org_id;
    let limit = query.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let offset = query.offset.unwrap_or(0).max(0);
    let company = query
        .company
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty());
    let search = query
        .search
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty());
    let home_branch_review_required = query.home_branch_review_required;

    let (items, total) = with_org_conn::<_, _, HrError>(&state.pool, org, move |tx| {
        Box::pin(async move {
            let mut count = QueryBuilder::<Postgres>::new(
                "SELECT count(*) FROM employees e \
                 LEFT JOIN branches b ON b.id = e.home_branch_id \
                   AND b.org_id = e.org_id AND b.deactivated_at IS NULL \
                 WHERE TRUE",
            );
            if let Some(company) = company.as_deref() {
                count.push(" AND e.company = ");
                count.push_bind(company);
            }
            if let Some(search) = search.as_deref() {
                count.push(" AND (e.name ILIKE ");
                count.push_bind(format!("%{search}%"));
                count.push(" OR e.employee_number ILIKE ");
                count.push_bind(format!("%{search}%"));
                count.push(")");
            }
            if let Some(required) = home_branch_review_required {
                count.push(" AND (b.id IS NULL) = ");
                count.push_bind(required);
            }
            let total: i64 = count.build_query_scalar().fetch_one(tx.as_mut()).await?;

            let mut rows = QueryBuilder::<Postgres>::new(
                "SELECT e.id, e.company, e.name, e.employee_number, e.org_unit, e.job, e.position, e.worksite_name, e.worksite_address, e.hire_date, e.exit_date, e.employment_status, e.leave_accrued::TEXT AS leave_accrued, e.leave_used::TEXT AS leave_used, e.leave_remaining::TEXT AS leave_remaining, b.id AS home_branch_id, b.name AS home_branch_name, e.identity_resolution_strategy, e.identity_resolution_confidence, e.identity_review_required, e.identity_name_only_merge, e.created_at, e.updated_at FROM employees e LEFT JOIN branches b ON b.id = e.home_branch_id AND b.org_id = e.org_id AND b.deactivated_at IS NULL WHERE TRUE",
            );
            if let Some(company) = company.as_deref() {
                rows.push(" AND e.company = ");
                rows.push_bind(company);
            }
            if let Some(search) = search.as_deref() {
                rows.push(" AND (e.name ILIKE ");
                rows.push_bind(format!("%{search}%"));
                rows.push(" OR e.employee_number ILIKE ");
                rows.push_bind(format!("%{search}%"));
                rows.push(")");
            }
            if let Some(required) = home_branch_review_required {
                rows.push(" AND (b.id IS NULL) = ");
                rows.push_bind(required);
            }
            rows.push(" ORDER BY e.company ASC, e.name ASC, e.source_sheet ASC, e.source_row ASC LIMIT ");
            rows.push_bind(limit);
            rows.push(" OFFSET ");
            rows.push_bind(offset);

            let items = rows
                .build()
                .fetch_all(tx.as_mut())
                .await?
                .into_iter()
                .map(employee_from_row)
                .collect::<Result<Vec<_>, _>>()?;
            Ok((items, total))
        })
    })
    .await?;

    Ok(Json(EmployeePage {
        items,
        total,
        limit,
        offset,
    }))
}

async fn create_employee(
    State(state): State<HrState>,
    Extension(principal): Extension<Principal>,
    Json(body): Json<CreateEmployeeRequest>,
) -> Result<(StatusCode, Json<EmployeeDetailResponse>), HrError> {
    authorize_hr_org_wide(&principal, Feature::EmployeeDirectoryManage)?;
    // Fail closed BEFORE any write: the home-branch routing authority is
    // command-only (0166) and must be assignable for the created employee.
    let command_store = require_home_branch_command_store(&state)?;
    let request = normalize_create_employee_request(body)?;
    let org = principal.org_id;
    let org_uuid = *org.as_uuid();
    let actor = principal.user_id;
    let employee_id = Uuid::new_v4();
    let home_branch_id = request.home_branch_id;

    // The `employees` row is written through `ObjectKey::Employment`'s owner (the
    // canonical adapter), NOT through the leave command capability, so
    // `console_leave_definer` no longer INSERTs into `employees` for People &
    // Workforce creation. The first home-branch routing authority is established
    // post-commit by [`assign_home_branch_if_unset`], exactly like the recruiting
    // hire handshake.
    let (created, replayed) = with_audits::<_, _, HrError>(&state.pool, org, |tx| {
        Box::pin(async move {
            let request_hash = sha256_hex(
                serde_json::to_string(&request)
                    .map_err(|_| HrError::validation("employee request could not be serialized"))?
                    .as_bytes(),
            );
            let (detail, replayed) =
                create_employee_core(tx, org_uuid, actor, &request, &request_hash, employee_id)
                    .await?;
            let audits = if replayed {
                Vec::new()
            } else {
                vec![employee_create_audit(org, actor, &request, employee_id)?]
            };
            Ok(((detail, replayed), audits))
        })
    })
    .await?;

    let employee_id = created.employee_id();
    let detail = assign_home_branch_if_unset(
        &state.pool,
        &command_store,
        org,
        employee_id,
        home_branch_id,
        actor,
        replayed,
    )
    .await?;
    Ok((
        if replayed {
            StatusCode::OK
        } else {
            StatusCode::CREATED
        },
        Json(detail),
    ))
}

pub(crate) fn employee_create_db_error(error: sqlx::Error) -> HrError {
    if error
        .as_database_error()
        .and_then(|database| database.code())
        .is_some_and(|code| code == "23505")
    {
        return HrError::from_kernel(KernelError::conflict(
            "employee number or idempotency key is already in use",
        ));
    }
    HrError::from(error)
}

/// The home-branch command capability both employee-creating surfaces
/// (People & Workforce create, recruiting hire) must hold BEFORE writing.
pub(crate) fn require_home_branch_command_store(state: &HrState) -> Result<PgLeaveStore, HrError> {
    state
        .leave_command_store
        .clone()
        .filter(PgLeaveStore::has_leave_command_pool)
        .ok_or_else(|| {
            HrError::unavailable(
                "leave command database is not configured; employee creation requires the home-branch command capability",
            )
        })
}

/// Establish the employee's first home-branch routing authority through the
/// isolated leave command capability (`leave_api.set_employee_home_branch`,
/// which CASes on `updated_at`, re-validates the branch, requires the
/// org-wide admin capability for a first assignment, and writes its own
/// `employee.home_branch_set` audit). Idempotent and race-convergent: an
/// already-assigned employee (replay) is returned untouched, and losing a
/// concurrent first-assignment race to the SAME branch is success. A FRESH
/// creation that finds a DIFFERENT branch already assigned lost the routing
/// race to another org-wide directory manager and is refused (conflict) rather
/// than reported as 201 for the wrong branch.
pub(crate) async fn assign_home_branch_if_unset(
    pool: &PgPool,
    command_store: &PgLeaveStore,
    org: OrgId,
    employee_id: Uuid,
    home_branch_id: Uuid,
    actor: UserId,
    replayed: bool,
) -> Result<EmployeeDetailResponse, HrError> {
    let load = |pool: &PgPool| {
        let pool = pool.clone();
        async move {
            with_org_conn::<_, _, HrError>(&pool, org, move |tx| {
                Box::pin(async move { load_employee_detail(tx, *org.as_uuid(), employee_id).await })
            })
            .await
        }
    };
    let detail = load(pool).await?;
    if let Some(existing_branch) = detail.employee_home_branch_id() {
        // A replayed create already established routing authority and a replay
        // must never clobber a later reassignment. A FRESH creation that finds a
        // DIFFERENT branch means another org-wide directory manager won the
        // first-assignment race — refuse rather than return success for an
        // employee routed to the wrong branch.
        if replayed || existing_branch == home_branch_id {
            return Ok(detail);
        }
        return Err(HrError::from_kernel(KernelError::conflict(
            "employee's home branch was assigned to a different branch concurrently",
        )));
    }
    match command_store
        .set_employee_home_branch_create(
            employee_id,
            home_branch_id,
            detail.employee_updated_at(),
            actor,
            TraceContext::generate(),
        )
        .await
    {
        Ok(_) => load(pool).await,
        Err(PgLeaveError::ConcurrentModification) => {
            let current = load(pool).await?;
            if current.employee_home_branch_id() == Some(home_branch_id) {
                // A same-key race partner won the identical first assignment.
                Ok(current)
            } else {
                Err(HrError::from_leave_store(
                    PgLeaveError::ConcurrentModification,
                ))
            }
        }
        Err(error) => {
            let mapped = HrError::from_leave_store(error);
            // Telemetry canary (LOW-4, shared across BOTH employee-creating
            // surfaces): after HIGH-1 the routing authority is the SAME
            // `assert_employee_directory_manager` predicate that passed
            // in-transaction BEFORE any write, so a FORBIDDEN (403) here means
            // either (a) predicate drift, or (b) a genuine mid-window
            // revocation/deactivation that committed after `with_audits`
            // released its `FOR UPDATE` locks. BOTH orphan the just-committed
            // row, so a non-zero count is an INVESTIGATE signal, never a false
            // alarm. Concurrent-modification / branch-deactivated /
            // command-unavailable failures are NOT forbidden and are NOT counted.
            if mapped.status == StatusCode::FORBIDDEN {
                metrics::counter!("hr_employee_create_write_then_deny_total").increment(1);
            }
            Err(mapped)
        }
    }
}

/// The transactional core of employee creation — the ONLY write path into
/// `employees` + `employee_employment_profiles`. Shared by the People &
/// Workforce endpoint above and the recruiting hire handshake
/// (`recruiting_hire`), which calls it inside its own `with_audits`
/// transaction so the employee row and the recruiting linkage commit
/// atomically. Same idempotency reservation, same tables, same lifecycle
/// event. Callers own authorization and append the `employee.create` audit
/// (from [`employee_create_audit`]) only when `replayed` is false.
pub(crate) async fn create_employee_core(
    tx: &mut sqlx::Transaction<'_, Postgres>,
    org_uuid: Uuid,
    actor: UserId,
    request: &NormalizedCreateEmployeeRequest,
    request_hash: &str,
    employee_id: Uuid,
) -> Result<(EmployeeDetailResponse, bool), HrError> {
    // Live actor revalidation at the write boundary (defense-in-depth): the
    // request-context principal is built from token roles and a request-time
    // grant resolution, so a deactivated, role-revoked, or grant-revoked actor
    // with an unexpired token would otherwise pass the pre-flight
    // `authorize_hr_org_wide`. Restore the migration-0183
    // `leave_api.assert_employee_directory_manager` predicate — SUPER_ADMIN, or
    // (SUPER_ADMIN/EXECUTIVE AND a live, active, non-branch-narrowed
    // `employee_directory_manage` allow grant) — reading `users` and
    // `user_role_assignments`/`policy_roles`/`policy_role_permissions`/
    // `policy_role_conditions` under the SAME `FOR UPDATE` locks this
    // transaction holds, so a concurrent deactivation, role revocation, or
    // grant revocation cannot slip between this check and the insert. This
    // predicate is byte-for-byte the routing authority of
    // `leave_api.assert_employee_directory_manager` (see the 0221 migration),
    // so no actor who passes this recheck can be denied at the later
    // first-home-branch assignment on authority grounds.
    let actor_row = sqlx::query(
        "SELECT is_active, roles, team FROM users WHERE id = $1 AND org_id = $2 FOR UPDATE",
    )
    .bind(*actor.as_uuid())
    .bind(org_uuid)
    .fetch_optional(tx.as_mut())
    .await?
    .ok_or_else(|| {
        HrError::from_kernel(KernelError::forbidden(
            "employee create: actor not found in this organization",
        ))
    })?;
    let actor_is_active: bool = actor_row.try_get("is_active")?;
    if !actor_is_active {
        return Err(HrError::from_kernel(KernelError::forbidden(
            "employee create: actor account is deactivated",
        )));
    }
    let actor_roles: Vec<String> = actor_row.try_get("roles")?;
    let actor_team: Option<String> = actor_row.try_get("team")?;
    let is_super_admin = actor_roles
        .iter()
        .any(|role| role == Role::SuperAdmin.as_str());
    if !is_super_admin {
        let has_directory_tier = actor_roles
            .iter()
            .any(|role| role == Role::SuperAdmin.as_str() || role == Role::Executive.as_str());
        if !has_directory_tier {
            return Err(HrError::from_kernel(KernelError::forbidden(
                "employee create: actor lacks an org-wide directory-manage role",
            )));
        }
        // Lock the matching grant rows (roles/permissions) FIRST, matching the
        // policy writer's role→condition lock order (`update_policy_role` locks
        // `policy_roles` before deleting that role's conditions), so the two
        // cannot form a lock-order deadlock.
        let role_ids: Vec<Uuid> = sqlx::query_scalar(
            "SELECT ura.role_id \
             FROM user_role_assignments ura \
             JOIN policy_roles pr ON pr.org_id = ura.org_id AND pr.id = ura.role_id \
             JOIN policy_role_permissions prp ON prp.org_id = pr.org_id AND prp.role_id = pr.id \
             WHERE ura.org_id = $1 AND ura.user_id = $2 \
               AND pr.status = 'ACTIVE' AND NOT pr.is_system \
               AND prp.feature_key = 'employee_directory_manage' \
               AND prp.permission_level = 'allow' \
             FOR UPDATE OF ura, pr, prp",
        )
        .bind(org_uuid)
        .bind(*actor.as_uuid())
        .fetch_all(tx.as_mut())
        .await?;
        // Lock the condition rows for those roles (same order as the policy
        // writer: roles first, then conditions).
        if !role_ids.is_empty() {
            sqlx::query(
                "SELECT 1 FROM policy_role_conditions prc \
                 WHERE prc.org_id = $1 AND prc.role_id = ANY($2) \
                 FOR UPDATE",
            )
            .bind(org_uuid)
            .bind(&role_ids)
            .execute(tx.as_mut())
            .await?;
        }
        // Live custom-grant revalidation (HIGH-2): a custom grant only widens an
        // EXECUTIVE/SUPER_ADMIN. The team condition mirrors 0183 exactly,
        // including the Korean-team → English-condition normalization. Evaluated
        // AFTER both the role and condition rows are locked.
        let live_org_wide_grant: Option<i32> = sqlx::query_scalar(
            r#"
            SELECT 1
            FROM user_role_assignments ura
            JOIN policy_roles pr ON pr.org_id = ura.org_id AND pr.id = ura.role_id
            JOIN policy_role_permissions prp
              ON prp.org_id = pr.org_id AND prp.role_id = pr.id
            WHERE ura.org_id = $1
              AND ura.user_id = $2
              AND pr.status = 'ACTIVE'
              AND NOT pr.is_system
              AND prp.feature_key = 'employee_directory_manage'
              AND prp.permission_level = 'allow'
              AND NOT EXISTS (
                  SELECT 1
                  FROM policy_role_conditions prc
                  WHERE prc.org_id = pr.org_id
                    AND prc.role_id = pr.id
                    AND (
                        prc.operator NOT IN ('equals', 'in')
                        OR prc.attribute = 'branch'
                        OR prc.attribute <> 'team'
                        OR $3::text IS NULL
                        OR NOT EXISTS (
                            SELECT 1
                            FROM unnest(prc.condition_values) AS cv
                            WHERE btrim(cv) = $3::text
                               OR upper(btrim(cv)) = CASE $3::text
                                    WHEN '정비' THEN 'MAINTENANCE'
                                    WHEN '예방' THEN 'PREVENTION'
                                    WHEN '관리' THEN 'MANAGEMENT'
                                    WHEN '접수' THEN 'RECEPTION'
                                    ELSE NULL
                                  END
                        )
                    )
              )
            LIMIT 1
            FOR UPDATE
            "#,
        )
        .bind(org_uuid)
        .bind(*actor.as_uuid())
        .bind(actor_team.as_deref())
        .fetch_optional(tx.as_mut())
        .await?;
        if live_org_wide_grant.is_none() {
            return Err(HrError::from_kernel(KernelError::forbidden(
                "employee create: actor's org-wide directory-manage grant was revoked",
            )));
        }
    }
    sqlx::query(
        "INSERT INTO employee_create_idempotency (org_id, idempotency_key, request_hash) VALUES ($1, $2, $3) ON CONFLICT (org_id, idempotency_key) DO NOTHING",
    )
    .bind(org_uuid)
    .bind(&request.idempotency_key)
    .bind(request_hash)
    .execute(tx.as_mut())
    .await
    .map_err(employee_create_db_error)?;
    let reservation = sqlx::query(
        "SELECT request_hash, employee_id FROM employee_create_idempotency WHERE org_id = $1 AND idempotency_key = $2 FOR UPDATE",
    )
    .bind(org_uuid)
    .bind(&request.idempotency_key)
    .fetch_one(tx.as_mut())
    .await?;
    let stored_hash: String = reservation.try_get("request_hash")?;
    if stored_hash != request_hash {
        return Err(HrError::from_kernel(KernelError::conflict(
            "idempotency key already used with a different employee payload",
        )));
    }
    if let Some(existing_id) = reservation.try_get::<Option<Uuid>, _>("employee_id")? {
        return Ok((load_employee_detail(tx, org_uuid, existing_id).await?, true));
    }
    let branch_exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM branches WHERE org_id = $1 AND id = $2 AND deactivated_at IS NULL)",
    )
    .bind(org_uuid)
    .bind(request.home_branch_id)
    .fetch_one(tx.as_mut())
    .await?;
    if !branch_exists {
        return Err(HrError::from_kernel(KernelError::not_found(
            "active home branch was not found in this organization",
        )));
    }
    if let Ok(org_unit_id) = Uuid::parse_str(&request.org_unit) {
        employment::ensure_org_unit_exists(tx, org_uuid, org_unit_id).await?;
    }
    if let Ok(job_position_id) = Uuid::parse_str(&request.position) {
        employment::ensure_job_position_exists(tx, org_uuid, job_position_id).await?;
    }

    // The statement itself lives in `console-ontology-canonical-adapter-postgres`,
    // which the canonical contract names as `ObjectKey::Employment`'s owner — the
    // one crate permitted to hold DML against `employees`. It takes THIS
    // transaction, so the employee row, its employment profile, its lifecycle
    // event and the idempotency reservation still commit or roll back together.
    //
    // home_branch_id is intentionally NOT inserted: since 0166 the branch
    // routing authority is command-only (the console_rt guard rejects it), so the
    // caller establishes it post-commit via [`assign_home_branch_if_unset`].
    employment::insert_employee_record(
        tx,
        org_uuid,
        employment::NewEmployeeRecord {
            employee_id,
            company: &request.company,
            name: &request.name,
            employee_number: &request.employee_number,
            org_unit: &request.org_unit,
            position: &request.position,
            worksite_name: &request.site,
        },
    )
    .await
    .map_err(employee_create_db_error)?;
    // Directory create mints the Person in the same transaction (person_id =
    // employee_id). hr.appoint stays a separate four-eyes assignment.
    write_person_in_tx(
        tx,
        &PersonCommand::create_bound(
            OrgId::from_uuid(org_uuid),
            actor,
            employee_id,
            &request.name,
        ),
    )
    .await?;
    sqlx::query(
        r#"INSERT INTO employee_employment_profiles (
            employee_id, org_id, employment_type, phone_e164, base_pay,
            idempotency_key, request_hash, created_by
        ) VALUES ($1, $2, $3, $4, $5::numeric, $6, $7, $8)"#,
    )
    .bind(employee_id)
    .bind(org_uuid)
    .bind(&request.employment_type)
    .bind(&request.phone_e164)
    .bind(&request.base_pay)
    .bind(&request.idempotency_key)
    .bind(request_hash)
    .bind(*actor.as_uuid())
    .execute(tx.as_mut())
    .await
    .map_err(employee_create_db_error)?;
    sqlx::query(
        r#"INSERT INTO employee_lifecycle_events (
            id, org_id, employee_id, event_type, to_status, to_company,
            to_org_unit, to_position, effective_date, comment, signoffs, created_by
        ) VALUES ($1, $2, $3, 'ONBOARD', 'ACTIVE', $4, $5, $6, $7,
            'Created through People & Workforce',
            '{}'::jsonb,
            $8)"#,
    )
    .bind(Uuid::new_v4())
    .bind(org_uuid)
    .bind(employee_id)
    .bind(&request.company)
    .bind(&request.org_unit)
    .bind(&request.position)
    .bind(OffsetDateTime::now_utc().date().to_string())
    .bind(*actor.as_uuid())
    .execute(tx.as_mut())
    .await
    .map_err(employee_create_db_error)?;
    sqlx::query(
        "UPDATE employee_create_idempotency SET employee_id = $3 WHERE org_id = $1 AND idempotency_key = $2",
    )
    .bind(org_uuid).bind(&request.idempotency_key).bind(employee_id)
    .execute(tx.as_mut()).await?;
    Ok((
        load_employee_detail(tx, org_uuid, employee_id).await?,
        false,
    ))
}

/// The `employee.create` audit event that accompanies a non-replayed
/// [`create_employee_core`] call, identical for both write paths.
pub(crate) fn employee_create_audit(
    org: OrgId,
    actor: UserId,
    request: &NormalizedCreateEmployeeRequest,
    employee_id: Uuid,
) -> Result<AuditEvent, HrError> {
    Ok(AuditEvent::new(
        Some(actor),
        AuditAction::new("employee.create").map_err(HrError::from_kernel)?,
        "employee",
        employee_id.to_string(),
        TraceContext::generate(),
        OffsetDateTime::now_utc(),
    )
    .with_org(org)
    .with_branch(BranchId::from_uuid(request.home_branch_id))
    .with_snapshots(
        None,
        Some(json!({
            "employee_number": &request.employee_number,
            "employment_type": &request.employment_type,
            "requested_home_branch_id": request.home_branch_id,
            "compensation_recorded": true,
            "phone_recorded": true
        })),
    ))
}

async fn get_employee_detail(
    State(state): State<HrState>,
    Extension(principal): Extension<Principal>,
    Path(employee_id): Path<Uuid>,
) -> Result<Json<EmployeeDetailResponse>, HrError> {
    authorize_hr_org_wide(&principal, Feature::EmployeeDirectoryManage)?;
    let org = principal.org_id;
    let org_uuid = *org.as_uuid();
    let detail = with_org_conn::<_, _, HrError>(&state.pool, org, move |tx| {
        Box::pin(async move { load_employee_detail(tx, org_uuid, employee_id).await })
    })
    .await?;
    Ok(Json(detail))
}

/// Assign the employee's authoritative approval-routing branch. No inference is
/// accepted: the caller names one active branch and must have manage authority
/// over both that branch and the employee's prior home branch (when present).
/// The row mutation and its before/after audit snapshots commit atomically.
async fn set_employee_home_branch(
    State(state): State<HrState>,
    Extension(principal): Extension<Principal>,
    Path(employee_id): Path<Uuid>,
    Json(body): Json<SetEmployeeHomeBranchRequest>,
) -> Result<Json<EmployeeHomeBranchResponse>, HrError> {
    authorize(
        &principal,
        Action::new(Feature::EmployeeDirectoryManage),
        BranchId::from_uuid(body.branch_id),
    )
    .map_err(HrError::from_kernel)?;

    let command_store = state.leave_command_store.clone().ok_or_else(|| {
        HrError::unavailable(
            "leave command database is not configured; home-branch assignment is unavailable",
        )
    })?;

    let org = principal.org_id;
    let org_uuid = *org.as_uuid();
    let actor = principal.user_id;
    let target_branch_id = body.branch_id;
    let expected_updated_at = OffsetDateTime::parse(
        body.expected_updated_at.trim(),
        &time::format_description::well_known::Rfc3339,
    )
    .map_err(|_| HrError::validation("expected_updated_at must be an RFC 3339 timestamp"))?;
    let trace = TraceContext::generate();

    let (branch_name, prior_branch_id, prior_updated_at) =
        with_org_conn::<_, _, HrError>(&state.pool, org, move |tx| {
            Box::pin(async move {
                let branch = sqlx::query(
                    "SELECT id, name FROM branches \
                 WHERE org_id = $1 AND id = $2 AND deactivated_at IS NULL",
                )
                .bind(org_uuid)
                .bind(target_branch_id)
                .fetch_optional(tx.as_mut())
                .await?
                .ok_or_else(|| {
                    HrError::from_kernel(KernelError::not_found(
                        "active home branch was not found in this organization",
                    ))
                })?;
                let branch_name: String = branch.try_get("name")?;

                let employee = sqlx::query(
                "SELECT home_branch_id, updated_at FROM employees WHERE org_id = $1 AND id = $2",
            )
            .bind(org_uuid)
            .bind(employee_id)
            .fetch_optional(tx.as_mut())
            .await?
            .ok_or_else(|| {
                HrError::from_kernel(KernelError::not_found(
                    "employee was not found in this organization",
                ))
            })?;
                let prior_branch_id: Option<Uuid> = employee.try_get("home_branch_id")?;
                let prior_updated_at: OffsetDateTime = employee.try_get("updated_at")?;

                Ok((branch_name, prior_branch_id, prior_updated_at))
            })
        })
        .await?;

    authorize_home_branch_transition(&principal, prior_branch_id, target_branch_id)?;
    if prior_updated_at != expected_updated_at {
        return Err(HrError::from_kernel(KernelError::conflict(
            "employee changed since it was read; reload before assigning a home branch",
        )));
    }

    // Sensitive employee routing columns are mechanically protected from
    // direct `console_rt` UPDATE. The adapter routes this mutation through the
    // separately credentialed command pool; the database re-checks tenant,
    // actor, branch authority, active-branch state, and optimistic version and
    // owns the one atomic before/after audit.
    let updated = console_platform_request_context::scope_org(org, async move {
        command_store
            .set_employee_home_branch(
                employee_id,
                target_branch_id,
                expected_updated_at,
                actor,
                trace,
            )
            .await
    })
    .await
    .map_err(HrError::from_leave_store)?;

    let response = EmployeeHomeBranchResponse {
        employee_id: updated.employee_id,
        branch_id: updated.home_branch_id,
        branch_name,
        updated_at: updated.updated_at,
    };

    Ok(Json(response))
}

async fn get_hr_org_chart(
    State(state): State<HrState>,
    Extension(principal): Extension<Principal>,
) -> Result<Json<HrOrgChartResponse>, HrError> {
    authorize_hr_org_wide(&principal, Feature::EmployeeDirectoryRead)?;
    record_hr_read("org_chart");
    let org = principal.org_id;

    let companies = with_org_conn::<_, _, HrError>(&state.pool, org, move |tx| {
        Box::pin(async move {
            let rows = sqlx::query(
                r#"
                SELECT
                    id,
                    company,
                    name,
                    employee_number,
                    COALESCE(NULLIF(org_unit, ''), '소속 미지정') AS org_unit,
                    COALESCE(NULLIF(position, ''), '직책 미지정') AS position,
                    employment_status
                FROM employees
                ORDER BY company ASC, org_unit ASC, position ASC, name ASC, source_sheet ASC, source_row ASC
                LIMIT 5000
                "#,
            )
            .fetch_all(tx.as_mut())
            .await?;

            let mut companies = Vec::<HrOrgChartCompany>::new();
            for row in rows {
                let company: String = row.try_get("company")?;
                let unit: String = row.try_get("org_unit")?;
                let position: String = row.try_get("position")?;
                let status: String = row.try_get("employment_status")?;

                let company_index = find_or_insert_company(&mut companies, company);
                let company = &mut companies[company_index];
                company.total += 1;
                if status == "ACTIVE" {
                    company.active += 1;
                }

                let unit_index = find_or_insert_unit(&mut company.units, unit);
                let unit = &mut company.units[unit_index];
                unit.total += 1;

                let position_index = find_or_insert_position(&mut unit.positions, position);
                let position = &mut unit.positions[position_index];
                position.total += 1;
                position.employees.push(HrOrgChartEmployee {
                    id: row.try_get("id")?,
                    name: row.try_get("name")?,
                    employee_number: row.try_get("employee_number")?,
                    status,
                });
            }
            Ok(companies)
        })
    })
    .await?;

    Ok(Json(HrOrgChartResponse { companies }))
}

async fn list_leave_balances(
    State(state): State<HrState>,
    Extension(principal): Extension<Principal>,
    Query(query): Query<HrListQuery>,
) -> Result<Json<LeaveBalancePage>, HrError> {
    authorize_hr_org_wide(&principal, Feature::EmployeeDirectoryRead)?;
    record_hr_read("leave_balances");
    let org = principal.org_id;
    let limit = query.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let offset = query.offset.unwrap_or(0).max(0);

    let (items, total, summary) = with_org_conn::<_, _, HrError>(&state.pool, org, move |tx| {
        Box::pin(async move {
            let total: i64 = sqlx::query_scalar(
                r#"
                SELECT count(*)
                FROM employees
                WHERE leave_accrued IS NOT NULL
                   OR leave_used IS NOT NULL
                   OR leave_remaining IS NOT NULL
                "#,
            )
            .fetch_one(tx.as_mut())
            .await?;

            let summary_row = sqlx::query(
                r#"
                SELECT
                    COALESCE(SUM(leave_accrued), 0)::TEXT AS accrued,
                    COALESCE(SUM(leave_used), 0)::TEXT AS used,
                    COALESCE(SUM(leave_remaining), 0)::TEXT AS remaining
                FROM employees
                WHERE leave_accrued IS NOT NULL
                   OR leave_used IS NOT NULL
                   OR leave_remaining IS NOT NULL
                "#,
            )
            .fetch_one(tx.as_mut())
            .await?;
            let summary = LeaveBalanceSummary {
                accrued: summary_row.try_get("accrued")?,
                used: summary_row.try_get("used")?,
                remaining: summary_row.try_get("remaining")?,
            };

            let items = sqlx::query(
                r#"
                SELECT
                    id,
                    company,
                    name,
                    employee_number,
                    org_unit,
                    position,
                    leave_accrued::TEXT AS leave_accrued,
                    leave_used::TEXT AS leave_used,
                    leave_remaining::TEXT AS leave_remaining
                FROM employees
                WHERE leave_accrued IS NOT NULL
                   OR leave_used IS NOT NULL
                   OR leave_remaining IS NOT NULL
                ORDER BY company ASC, name ASC, source_sheet ASC, source_row ASC
                LIMIT $1 OFFSET $2
                "#,
            )
            .bind(limit)
            .bind(offset)
            .fetch_all(tx.as_mut())
            .await?
            .into_iter()
            .map(|row| {
                Ok(LeaveBalanceItem {
                    id: row.try_get("id")?,
                    company: row.try_get("company")?,
                    name: row.try_get("name")?,
                    employee_number: row.try_get("employee_number")?,
                    org_unit: row.try_get("org_unit")?,
                    position: row.try_get("position")?,
                    leave_accrued: row.try_get("leave_accrued")?,
                    leave_used: row.try_get("leave_used")?,
                    leave_remaining: row.try_get("leave_remaining")?,
                })
            })
            .collect::<Result<Vec<_>, HrError>>()?;

            Ok((items, total, summary))
        })
    })
    .await?;

    Ok(Json(LeaveBalancePage {
        items,
        total,
        limit,
        offset,
        summary,
    }))
}

async fn list_attendance_summary(
    State(state): State<HrState>,
    Extension(principal): Extension<Principal>,
    Query(query): Query<AttendanceSummaryQuery>,
) -> Result<Json<AttendanceSummaryPage>, HrError> {
    let scope = authorize_hr_attendance_branch_query(
        &principal,
        Feature::EmployeeDirectoryRead,
        query.branch_id,
    )?;
    record_hr_read("attendance_summary");
    let org = principal.org_id;
    let limit = query.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let offset = query.offset.unwrap_or(0).max(0);

    let (items, total) = with_org_conn::<_, _, HrError>(&state.pool, org, move |tx| {
        Box::pin(async move {
            let mut total_query = QueryBuilder::<Postgres>::new(
                "SELECT COUNT(*) FROM (SELECT l.user_id FROM site_attendance_events l WHERE ",
            );
            push_attendance_branch_filter(&mut total_query, scope);
            total_query.push(" GROUP BY l.user_id) counted");
            let total: i64 = total_query
                .build_query_scalar()
                .fetch_one(tx.as_mut())
                .await?;

            let mut rows_query = QueryBuilder::<Postgres>::new(
                r#"
                SELECT
                    l.user_id,
                    COALESCE(u.display_name, '사용자 미확인') AS display_name,
                    COUNT(*) FILTER (WHERE l.kind = 'ARRIVAL') AS arrivals,
                    COUNT(*) FILTER (WHERE l.kind = 'DEPARTURE') AS departures,
                    (ARRAY_AGG(l.kind ORDER BY l.occurred_at DESC))[1] AS last_kind,
                    MAX(l.occurred_at) AS last_event_at
                FROM site_attendance_events l
                LEFT JOIN users u ON u.id = l.user_id
                WHERE
                "#,
            );
            push_attendance_branch_filter(&mut rows_query, scope);
            rows_query.push(
                " GROUP BY l.user_id, u.display_name ORDER BY last_event_at DESC, l.user_id DESC LIMIT ",
            );
            rows_query.push_bind(limit);
            rows_query.push(" OFFSET ");
            rows_query.push_bind(offset);

            let items = rows_query
                .build()
                .fetch_all(tx.as_mut())
                .await?
                .into_iter()
                .map(|row| {
                    Ok(AttendanceSummaryItem {
                        user_id: row.try_get("user_id")?,
                        display_name: row.try_get("display_name")?,
                        arrivals: row.try_get("arrivals")?,
                        departures: row.try_get("departures")?,
                        last_kind: row.try_get("last_kind")?,
                        last_event_at: row.try_get("last_event_at")?,
                    })
                })
                .collect::<Result<Vec<_>, HrError>>()?;

            Ok((items, total))
        })
    })
    .await?;

    Ok(Json(AttendanceSummaryPage {
        items,
        total,
        limit,
        offset,
    }))
}
async fn list_my_attendance_records(
    State(state): State<HrState>,
    Extension(principal): Extension<Principal>,
    Query(query): Query<AttendanceSelfRecordsQuery>,
) -> Result<Json<EmployeeAttendanceRecordPage>, HrError> {
    record_hr_read("employee_attendance_self");
    let org = principal.org_id;
    let user_id = principal.user_id;
    let limit = query.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let offset = query.offset.unwrap_or(0).max(0);

    let page = with_org_conn::<_, _, HrError>(&state.pool, org, move |tx| {
        Box::pin(async move {
            // Self-scoped read, no role gate: an authenticated user with no
            // linked employee — an ADMIN/system account — has zero personal
            // records, so return an empty page rather than a 403. (Writes still
            // require a link via `load_linked_employee_for_user`.)
            match load_optional_linked_employee_id(tx, org, user_id).await? {
                Some(employee_id) => {
                    list_attendance_records_for_employee(tx, employee_id, None, limit, offset).await
                }
                None => Ok(EmployeeAttendanceRecordPage {
                    items: Vec::new(),
                    total: 0,
                    limit,
                    offset,
                }),
            }
        })
    })
    .await?;

    Ok(Json(page))
}

async fn list_attendance_records(
    State(state): State<HrState>,
    Extension(principal): Extension<Principal>,
    Query(query): Query<AttendanceManagerRecordsQuery>,
) -> Result<Json<EmployeeAttendanceRecordPage>, HrError> {
    let branch_id = authorize_hr_attendance_branch_query(
        &principal,
        Feature::EmployeeDirectoryRead,
        query.branch_id,
    )?;
    record_hr_read("employee_attendance_management");
    let org = principal.org_id;
    let limit = query.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let offset = query.offset.unwrap_or(0).max(0);
    let employee_id = query.employee_id;

    let page = with_org_conn::<_, _, HrError>(&state.pool, org, move |tx| {
        Box::pin(async move {
            if let Some(employee_id) = employee_id {
                list_attendance_records_for_employee(tx, employee_id, branch_id, limit, offset)
                    .await
            } else {
                list_attendance_records_for_org(tx, branch_id, limit, offset).await
            }
        })
    })
    .await?;

    Ok(Json(page))
}

async fn create_my_attendance_record(
    State(state): State<HrState>,
    Extension(principal): Extension<Principal>,
    Json(body): Json<CreateEmployeeAttendanceRecordRequest>,
) -> Result<Json<EmployeeAttendanceRecordResponse>, HrError> {
    let org = principal.org_id;
    let actor = principal.user_id;
    let kind = normalize_attendance_kind(&body.kind)?;
    let idempotency_key = normalize_idempotency_key(body.idempotency_key)?;
    let note = normalize_attendance_note(body.note)?;

    let response = with_audits::<_, _, HrError>(&state.pool, org, move |tx| {
        Box::pin(async move {
            let linked = load_linked_employee_for_user(tx, org, actor, true).await?;

            if let Some(existing) =
                load_attendance_record_by_idempotency_key(tx, linked.employee_id, &idempotency_key)
                    .await?
            {
                if existing.kind.as_str() != kind || existing.note != note {
                    return Err(HrError::from_kernel(KernelError::conflict(
                        "idempotency key already used with different attendance payload",
                    )));
                }
                return Ok((existing, Vec::new()));
            }

            let previous_state: Option<String> = sqlx::query_scalar(
                r#"
                SELECT state_after
                FROM employee_attendance_records
                WHERE employee_id = $1
                ORDER BY occurred_at DESC, created_at DESC, id DESC
                LIMIT 1
                "#,
            )
            .bind(linked.employee_id)
            .fetch_optional(tx.as_mut())
            .await?;
            let state_after = next_employee_attendance_state(previous_state.as_deref(), kind)?;

            let record_row = sqlx::query(
                r#"
                INSERT INTO employee_attendance_records (
                    org_id,
                    employee_id,
                    actor_user_id,
                    kind,
                    state_after,
                    note,
                    idempotency_key
                )
                VALUES ($1, $2, $3, $4, $5, $6, $7)
                RETURNING
                    id,
                    employee_id,
                    kind,
                    occurred_at,
                    work_date::TEXT AS work_date,
                    state_after,
                    note
                "#,
            )
            .bind(*org.as_uuid())
            .bind(linked.employee_id)
            .bind(*actor.as_uuid())
            .bind(kind)
            .bind(state_after)
            .bind(note)
            .bind(&idempotency_key)
            .fetch_one(tx.as_mut())
            .await?;

            let record_id: Uuid = record_row.try_get("id")?;
            let work_date: String = record_row.try_get("work_date")?;
            let digest = sha256_hex(
                format!(
                    "employee_self_service|{}|{}|{}|{}",
                    org.as_uuid(),
                    linked.employee_id,
                    record_id,
                    kind
                )
                .as_bytes(),
            );

            // Freeze-window gate: an attendance write that stamps a work_date
            // inside a locked payroll period must fail closed (rolls back the
            // whole attendance record + material-ref transaction, 409).
            let work_date_parsed = Date::parse(
                &work_date,
                &time::format_description::well_known::Iso8601::DATE,
            )
            .map_err(|e| {
                HrError::from_kernel(KernelError::internal(format!(
                    "attendance work_date unparseable: {e}"
                )))
            })?;
            console_platform_db::assert_period_open(
                tx,
                console_platform_db::PeriodLockDomain::Payroll,
                work_date_parsed,
            )
            .await
            .map_err(HrError::from_kernel)?;

            let ref_id: Uuid = sqlx::query_scalar(
                r#"
                INSERT INTO payroll_attendance_material_refs (
                    org_id,
                    attendance_record_id,
                    employee_id,
                    work_date,
                    source_digest
                )
                VALUES ($1, $2, $3, $4::DATE, $5)
                RETURNING id
                "#,
            )
            .bind(*org.as_uuid())
            .bind(record_id)
            .bind(linked.employee_id)
            .bind(&work_date)
            .bind(digest)
            .fetch_one(tx.as_mut())
            .await?;

            let response = employee_attendance_record_from_parts(
                record_row,
                linked.display_name,
                ref_id,
                false,
            )?;

            let attendance_audit = AuditEvent::new(
                Some(actor),
                AuditAction::new("employee_attendance.record").map_err(HrError::from_kernel)?,
                "employee_attendance_record",
                record_id.to_string(),
                TraceContext::generate(),
                OffsetDateTime::now_utc(),
            )
            .with_org(org)
            .with_snapshots(
                None,
                Some(json!({
                    "employee_id": linked.employee_id,
                    "kind": response.kind,
                    "state_after": response.state_after,
                    "work_date": response.work_date,
                    "payroll_material_ref_id": ref_id,
                })),
            );
            let payroll_audit = AuditEvent::new(
                Some(actor),
                AuditAction::new("payroll_attendance.link").map_err(HrError::from_kernel)?,
                "payroll_attendance_material_ref",
                ref_id.to_string(),
                TraceContext::generate(),
                OffsetDateTime::now_utc(),
            )
            .with_org(org)
            .with_snapshots(
                None,
                Some(json!({
                    "attendance_record_id": record_id,
                    "employee_id": linked.employee_id,
                    "work_date": response.work_date,
                    "source_type": "employee_self_service",
                })),
            );

            Ok((response, vec![attendance_audit, payroll_audit]))
        })
    })
    .await?;

    Ok(Json(response))
}

async fn get_hr_readiness_summary(
    State(state): State<HrState>,
    Extension(principal): Extension<Principal>,
) -> Result<Json<HrReadinessSummary>, HrError> {
    authorize_hr_org_wide(&principal, Feature::EmployeeDirectoryRead)?;
    record_hr_read("readiness_summary");
    let org = principal.org_id;
    let scope = principal.branch_scope.clone();

    let summary = with_org_conn::<_, _, HrError>(&state.pool, org, move |tx| {
        Box::pin(async move {
            let import_row = sqlx::query(
                r#"
                SELECT
                    COUNT(*)::BIGINT AS runs,
                    COUNT(*) FILTER (WHERE status = 'APPLIED')::BIGINT AS applied_runs,
                    COALESCE(SUM(input_rows), 0)::BIGINT AS input_rows,
                    COALESCE(SUM(candidate_rows), 0)::BIGINT AS candidate_rows,
                    COALESCE(SUM(preserved_rows), 0)::BIGINT AS preserved_rows,
                    MAX(COALESCE(applied_at, updated_at, created_at)) AS latest_import_at
                FROM data_import_runs
                WHERE entity_type = 'employee_hr'
                "#,
            )
            .fetch_one(tx.as_mut())
            .await?;

            let ledger_rows: i64 = sqlx::query_scalar(
                r#"
                SELECT COUNT(*)::BIGINT
                FROM data_import_rows r
                JOIN data_import_runs run
                  ON run.id = r.run_id
                 AND run.org_id = r.org_id
                WHERE run.entity_type = 'employee_hr'
                "#,
            )
            .fetch_one(tx.as_mut())
            .await?;

            let payroll_run_row = sqlx::query(
                r#"
                SELECT
                    COUNT(*)::BIGINT AS draft_runs,
                    COUNT(*) FILTER (WHERE status = 'BLOCKED_LEGAL_GATE')::BIGINT AS blocked_runs,
                    COUNT(*) FILTER (WHERE calculation_enabled)::BIGINT AS calculation_enabled_runs,
                    -- Inspectable close states are STAGED, BLOCKED_LEGAL_GATE,
                    -- READY_FOR_REVIEW, and APPROVED. ISSUED and VOID are terminal history.
                    COUNT(*) FILTER (
                        WHERE status IN ('STAGED', 'BLOCKED_LEGAL_GATE', 'READY_FOR_REVIEW', 'APPROVED')
                    )::BIGINT AS active_close_runs,
                    (ARRAY_AGG(status ORDER BY updated_at DESC, id DESC))[1] AS latest_status,
                    (ARRAY_AGG(source_label ORDER BY updated_at DESC, id DESC))[1] AS latest_source_label,
                    (ARRAY_AGG(period_start::TEXT ORDER BY updated_at DESC, id DESC))[1] AS latest_period_start,
                    (ARRAY_AGG(period_end::TEXT ORDER BY updated_at DESC, id DESC))[1] AS latest_period_end,
                    MAX(updated_at) AS latest_updated_at
                FROM payroll_draft_runs
                "#,
            )
            .fetch_one(tx.as_mut())
            .await?;

            let payroll_line_row = sqlx::query(
                r#"
                SELECT
                    COUNT(*)::BIGINT AS draft_lines,
                    COALESCE(SUM(payroll_source_row_count), 0)::BIGINT AS payroll_source_rows,
                    COALESCE(SUM(attendance_source_row_count), 0)::BIGINT AS attendance_source_rows,
                    COALESCE(SUM(attendance_event_count), 0)::BIGINT AS attendance_event_links,
                    COUNT(*) FILTER (WHERE gross_pay_source_present)::BIGINT AS gross_pay_source_lines,
                    COUNT(*) FILTER (WHERE net_pay_source_present)::BIGINT AS net_pay_source_lines
                FROM payroll_draft_lines
                "#,
            )
            .fetch_one(tx.as_mut())
            .await?;

            let leave_row = sqlx::query(
                r#"
                SELECT
                    COUNT(*)::BIGINT AS obligations,
                    COUNT(*) FILTER (WHERE status = 'USAGE_PROMOTION_DRAFT_REQUIRED')::BIGINT AS usage_promotion_required,
                    COUNT(*) FILTER (WHERE status = 'PAYOUT_REVIEW_REQUIRED')::BIGINT AS payout_review_required,
                    COUNT(*) FILTER (WHERE status = 'NEEDS_HR_REVIEW')::BIGINT AS needs_review,
                    COALESCE(SUM(leave_remaining), 0)::TEXT AS remaining_days
                FROM annual_leave_obligations
                "#,
            )
            .fetch_one(tx.as_mut())
            .await?;

            let mut attendance_query =
                QueryBuilder::<Postgres>::new("SELECT COUNT(*)::BIGINT FROM site_attendance_events l WHERE ");
            push_attendance_branch_scope(&mut attendance_query, &scope);
            let durable_events: i64 = attendance_query
                .build_query_scalar()
                .fetch_one(tx.as_mut())
                .await?;
            let self_service_records: i64 = sqlx::query_scalar(
                "SELECT COUNT(*)::BIGINT FROM employee_attendance_records",
            )
            .fetch_one(tx.as_mut())
            .await?;
            let attendance_material_refs: i64 = sqlx::query_scalar(
                "SELECT COUNT(*)::BIGINT FROM payroll_attendance_material_refs",
            )
            .fetch_one(tx.as_mut())
            .await?;


            Ok(HrReadinessSummary {
                imports: HrImportReadinessSummary {
                    runs: import_row.try_get("runs")?,
                    applied_runs: import_row.try_get("applied_runs")?,
                    input_rows: import_row.try_get("input_rows")?,
                    candidate_rows: import_row.try_get("candidate_rows")?,
                    preserved_rows: import_row.try_get("preserved_rows")?,
                    ledger_rows,
                    latest_import_at: import_row.try_get("latest_import_at")?,
                },
                payroll: HrPayrollReadinessSummary {
                    draft_runs: payroll_run_row.try_get("draft_runs")?,
                    blocked_runs: payroll_run_row.try_get("blocked_runs")?,
                    calculation_enabled_runs: payroll_run_row
                        .try_get("calculation_enabled_runs")?,
                    active_close_runs: payroll_run_row.try_get("active_close_runs")?,
                    draft_lines: payroll_line_row.try_get("draft_lines")?,
                    payroll_source_rows: payroll_line_row.try_get("payroll_source_rows")?,
                    attendance_source_rows: payroll_line_row
                        .try_get("attendance_source_rows")?,
                    attendance_event_links: payroll_line_row.try_get("attendance_event_links")?,
                    attendance_material_refs,
                    gross_pay_source_lines: payroll_line_row
                        .try_get("gross_pay_source_lines")?,
                    net_pay_source_lines: payroll_line_row.try_get("net_pay_source_lines")?,
                    latest_status: payroll_run_row.try_get("latest_status")?,
                    latest_source_label: payroll_run_row.try_get("latest_source_label")?,
                    latest_period_start: payroll_run_row.try_get("latest_period_start")?,
                    latest_period_end: payroll_run_row.try_get("latest_period_end")?,
                    latest_updated_at: payroll_run_row.try_get("latest_updated_at")?,
                },
                annual_leave: HrAnnualLeaveReadinessSummary {
                    obligations: leave_row.try_get("obligations")?,
                    usage_promotion_required: leave_row
                        .try_get("usage_promotion_required")?,
                    payout_review_required: leave_row.try_get("payout_review_required")?,
                    needs_review: leave_row.try_get("needs_review")?,
                    remaining_days: leave_row.try_get("remaining_days")?,
                },
                attendance: HrAttendanceReadinessSummary {
                    durable_events,
                    self_service_records,
                    payroll_material_refs: attendance_material_refs,
                },
            })
        })
    })
    .await?;

    Ok(Json(summary))
}

async fn get_absence_exit_dashboard(
    State(state): State<HrState>,
    Extension(principal): Extension<Principal>,
    Query(query): Query<AbsenceExitDashboardQuery>,
) -> Result<Json<AbsenceExitDashboardResponse>, HrError> {
    authorize_hr_scoped(&principal, Feature::EmployeeDirectoryRead)?;
    record_hr_read("absence_exit_dashboard");
    let org = principal.org_id;
    let org_uuid = *org.as_uuid();
    let scope = principal.branch_scope.clone();
    let actor = principal.user_id;
    let limit = query.limit.unwrap_or(50).clamp(1, 200);
    let offset = query.offset.unwrap_or(0).max(0);
    let employee_id = query.employee_id;

    // `with_audits` (not `with_org_conn`): the read-path materializer mutates the
    // audited `employee_absence_alerts` table, so a changed pass must land an
    // audit row in the SAME transaction (0093 review MEDIUM #5). It emits ZERO
    // events on an unchanged read, so the materializer's idempotency/write-storm
    // guard is preserved — no audit spam on repeated dashboard GETs.
    let dashboard = with_audits::<_, _, HrError>(&state.pool, org, move |tx| {
        Box::pin(async move {
            let changed_alert_ids =
                materialize_absence_alerts_from_imports(tx, org_uuid, &scope).await?;
            let summary = load_absence_exit_summary(tx, &scope).await?;
            let alerts = load_absence_alerts(tx, &scope, employee_id, limit, offset).await?;
            let exit_cases = load_exit_cases(tx, &scope, employee_id, limit, offset).await?;

            let mut events = Vec::new();
            if !changed_alert_ids.is_empty() {
                events.push(
                    AuditEvent::new(
                        Some(actor),
                        AuditAction::new("employee.absence_alert.materialize")
                            .map_err(HrError::from_kernel)?,
                        "employee_absence_alerts",
                        org_uuid.to_string(),
                        TraceContext::generate(),
                        OffsetDateTime::now_utc(),
                    )
                    .with_org(org)
                    .with_snapshots(
                        None,
                        Some(json!({
                            "changed_count": changed_alert_ids.len(),
                            "changed_alert_ids": changed_alert_ids,
                            "source": "attendance_direct_import",
                            "trigger": "absence_exit_dashboard_read",
                        })),
                    ),
                );
            }

            Ok((
                AbsenceExitDashboardResponse {
                    summary,
                    alerts,
                    exit_cases,
                },
                events,
            ))
        })
    })
    .await?;

    Ok(Json(dashboard))
}

async fn report_employee_exit_case(
    State(state): State<HrState>,
    Extension(principal): Extension<Principal>,
    Json(body): Json<ReportEmployeeExitCaseRequest>,
) -> Result<(StatusCode, Json<EmployeeExitCaseResponse>), HrError> {
    let effective_exit_date = normalize_date_text(&body.effective_exit_date)?;
    let site_manager_note =
        normalize_limited_text(body.site_manager_note, 1000, "site_manager_note")?;
    let actor = principal.user_id;
    let org = principal.org_id;
    let org_uuid = *org.as_uuid();

    let exit_case = with_audits::<_, _, HrError>(&state.pool, org, |tx| {
        Box::pin(async move {
            let branch_id = resolve_exit_case_branch(
                tx,
                org_uuid,
                body.employee_id,
                body.branch_id,
                body.absence_alert_id,
            )
            .await?;
            authorize_hr_scoped_write(&principal, Feature::ExitCaseReport, branch_id)?;

            let case_id: Uuid = sqlx::query_scalar(
                r#"
                INSERT INTO employee_exit_cases (
                    org_id, employee_id, branch_id, absence_alert_id,
                    effective_exit_date, site_manager_note, reported_by
                )
                VALUES ($1, $2, $3, $4, $5::DATE, $6, $7)
                RETURNING id
                "#,
            )
            .bind(org_uuid)
            .bind(body.employee_id)
            .bind(branch_id)
            .bind(body.absence_alert_id)
            .bind(&effective_exit_date)
            .bind(&site_manager_note)
            .bind(*actor.as_uuid())
            .fetch_one(tx.as_mut())
            .await?;

            if let Some(alert_id) = body.absence_alert_id {
                let updated = sqlx::query(
                    r#"
                    UPDATE employee_absence_alerts
                    SET status = 'LINKED_EXIT',
                        linked_exit_case_id = $3,
                        updated_at = now()
                    WHERE org_id = $1 AND id = $2
                    "#,
                )
                .bind(org_uuid)
                .bind(alert_id)
                .bind(case_id)
                .execute(tx.as_mut())
                .await?;
                if updated.rows_affected() != 1 {
                    return Err(HrError::from_kernel(KernelError::internal(
                        "locked absence alert disappeared before exit-case linkage",
                    )));
                }
            }

            let exit_case = load_exit_case_by_id(tx, org_uuid, case_id).await?;
            let mut event = AuditEvent::new(
                Some(actor),
                AuditAction::new("employee.exit.report").map_err(HrError::from_kernel)?,
                "employee",
                body.employee_id.to_string(),
                TraceContext::generate(),
                OffsetDateTime::now_utc(),
            )
            .with_org(org)
            .with_snapshots(
                None,
                Some(json!({
                    "employee_id": body.employee_id,
                    "absence_alert_id": body.absence_alert_id,
                    "effective_exit_date": effective_exit_date,
                    "branch_id": branch_id,
                    "requested_branch_id": body.branch_id
                })),
            );
            if let Some(branch_id) = branch_id {
                event = event.with_branch(BranchId::from_uuid(branch_id));
            }
            Ok((exit_case, vec![event]))
        })
    })
    .await?;

    Ok((StatusCode::CREATED, Json(exit_case)))
}

// M2-strangler-debt: this absence->exit->settlement flow is a hardcoded cross-module
// decision spine (report -> HR confirm -> HQ confirm -> settlement -> approval). It is the
// SECOND such flow (after completion->approval->payroll) slated to migrate onto the ADR-0018
// workflow runtime spine (workflow_runs / workflow_node_runs / workflow_waiting_tasks /
// workflow_outbox_events). State machine -> spine-IR mapping: exit_case.status transitions map
// to workflow_node_runs; the two-tier confirm maps to workflow_waiting_tasks with required_policy
// guards; the settlement/certification side effects map to idempotency-keyed workflow_outbox_events.
// See .omc/plans/ralplan-pr166-completion.md S8.2 (M2 charter follow-up). Migration is mechanical,
// not a rewrite, once the M2 executor lands.
async fn confirm_employee_exit_case(
    State(state): State<HrState>,
    Extension(principal): Extension<Principal>,
    Path(case_id): Path<Uuid>,
    Json(body): Json<ConfirmEmployeeExitCaseRequest>,
) -> Result<Json<EmployeeExitCaseResponse>, HrError> {
    let actor = principal.user_id;
    let org = principal.org_id;
    let org_uuid = *org.as_uuid();
    let decision = body
        .decision
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("CONFIRM")
        .to_ascii_uppercase();
    let note = normalize_optional_limited_text(body.note, 1000, "note")?;

    let event = AuditEvent::new(
        Some(actor),
        AuditAction::new("employee.exit.confirm").map_err(HrError::from_kernel)?,
        "employee_exit_case",
        case_id.to_string(),
        TraceContext::generate(),
        OffsetDateTime::now_utc(),
    )
    .with_org(org)
    .with_snapshots(
        None,
        Some(json!({
            "exit_case_id": case_id,
            "decision": decision,
            "hq_confirmation": body.hq_confirmation
        })),
    );

    let exit_case = with_audit::<_, _, HrError>(&state.pool, event, |tx| {
        Box::pin(async move {
            let context = load_exit_case_context(tx, org_uuid, case_id, true).await?;

            // Separation-of-duties capability gate (replaces the coarse
            // EmployeeDirectoryManage gate). A REJECT may be performed by anyone
            // holding either confirmation capability; a CONFIRM is gated per tier
            // below. Both are checked as capabilities against the case's branch,
            // consistent with the move toward capability (not role-string) authz.
            let holds_hr_confirm =
                authorize_hr_scoped_write(&principal, Feature::ExitCaseHrConfirm, context.branch_id)
                    .is_ok();
            let holds_hq_confirm =
                authorize_hr_scoped_write(&principal, Feature::ExitCaseHqConfirm, context.branch_id)
                    .is_ok();

            if decision == "REJECT" {
                if !(holds_hr_confirm || holds_hq_confirm) {
                    return Err(HrError::from_kernel(KernelError::forbidden(
                        "rejecting an exit case requires an exit-case confirmation capability",
                    )));
                }
                sqlx::query(
                    r#"
                    UPDATE employee_exit_cases
                    SET status = 'REJECTED',
                        confirmation_note = $3,
                        updated_at = now()
                    WHERE org_id = $1 AND id = $2
                    "#,
                )
                .bind(org_uuid)
                .bind(case_id)
                .bind(note.as_deref())
                .execute(tx.as_mut())
                .await?;
                return load_exit_case_by_id(tx, org_uuid, case_id).await;
            }
            if decision != "CONFIRM" {
                return Err(HrError::validation("decision must be CONFIRM or REJECT"));
            }

            // Two-tier separation of duties enforced in CODE — the client
            // `hq_confirmation` boolean only selects which tier is attempted; the
            // capability + stored-state + distinct-actor checks are the authority.
            if body.hq_confirmation {
                if !holds_hq_confirm {
                    return Err(HrError::from_kernel(KernelError::forbidden(
                        "HQ confirmation requires the exit-case HQ confirmation capability",
                    )));
                }
                authorize_exit_confirmation_hq_tier(
                    &context.status,
                    context.hr_confirmed_by,
                    *actor.as_uuid(),
                )?;
            } else if !holds_hr_confirm {
                return Err(HrError::from_kernel(KernelError::forbidden(
                    "HR confirmation requires the exit-case HR confirmation capability",
                )));
            }

            insert_confirmed_exit_lifecycle_event(tx, org_uuid, actor, &context, note.as_deref())
                .await?;

            let next_status = if body.hq_confirmation {
                "HQ_CONFIRMED"
            } else {
                "HR_CONFIRMED"
            };
            sqlx::query(
                r#"
                UPDATE employee_exit_cases
                SET status = $3,
                    hr_confirmed_by = COALESCE(hr_confirmed_by, $4),
                    hr_confirmed_at = COALESCE(hr_confirmed_at, now()),
                    hq_confirmed_by = CASE WHEN $5 THEN $4 ELSE hq_confirmed_by END,
                    hq_confirmed_at = CASE WHEN $5 THEN COALESCE(hq_confirmed_at, now()) ELSE hq_confirmed_at END,
                    confirmation_note = $6,
                    updated_at = now()
                WHERE org_id = $1 AND id = $2
                "#,
            )
            .bind(org_uuid)
            .bind(case_id)
            .bind(next_status)
            .bind(*actor.as_uuid())
            .bind(body.hq_confirmation)
            .bind(note.as_deref())
            .execute(tx.as_mut())
            .await?;

            upsert_exit_settlement_package(tx, org_uuid, case_id, body.settlement_input.clone())
                .await?;
            load_exit_case_by_id(tx, org_uuid, case_id).await
        })
    })
    .await?;

    Ok(Json(exit_case))
}

async fn draft_employee_exit_approval(
    State(state): State<HrState>,
    Extension(principal): Extension<Principal>,
    Path(case_id): Path<Uuid>,
    Json(body): Json<DraftEmployeeExitApprovalRequest>,
) -> Result<Json<EmployeeExitCaseResponse>, HrError> {
    let actor = principal.user_id;
    let org = principal.org_id;
    let org_uuid = *org.as_uuid();
    let note = normalize_optional_limited_text(body.note, 1000, "note")?;

    let event = AuditEvent::new(
        Some(actor),
        AuditAction::new("employee.exit.approval_draft").map_err(HrError::from_kernel)?,
        "employee_exit_case",
        case_id.to_string(),
        TraceContext::generate(),
        OffsetDateTime::now_utc(),
    )
    .with_org(org)
    .with_snapshots(
        None,
        Some(json!({
            "exit_case_id": case_id,
            "submit": body.submit,
            "has_settlement_input": body.settlement_input.is_some(),
            // Certification state is captured in the audit trail: writing the
            // approval_payload (a certification-covered field) atomically resets
            // the package to UNCERTIFIED_DRAFT below, so the settlement figure
            // being drafted/submitted is, by construction, an uncertified draft.
            "certification_status": "UNCERTIFIED_DRAFT"
        })),
    );

    let exit_case = with_audit::<_, _, HrError>(&state.pool, event, |tx| {
        Box::pin(async move {
            let context = load_exit_case_context(tx, org_uuid, case_id, true).await?;
            authorize_hr_scoped_write(
                &principal,
                Feature::ExitSettlementManage,
                context.branch_id,
            )?;
            if !matches!(
                context.status.as_str(),
                "HR_CONFIRMED" | "HQ_CONFIRMED" | "SETTLEMENT_READY" | "APPROVAL_DRAFTED"
            ) {
                return Err(HrError::from_kernel(KernelError::invalid_transition(
                    "exit approval draft requires HR or HQ confirmation first",
                )));
            }

            let existing_ready_package_id = if body.settlement_input.is_none() {
                sqlx::query_scalar::<_, Uuid>(
                    r#"
                    SELECT id
                    FROM employee_exit_settlement_packages
                    WHERE org_id = $1
                      AND exit_case_id = $2
                      AND severance_pay_won IS NOT NULL
                      AND CARDINALITY(missing_source_fields) = 0
                    ORDER BY updated_at DESC, generated_at DESC
                    LIMIT 1
                    "#,
                )
                .bind(org_uuid)
                .bind(case_id)
                .fetch_optional(tx.as_mut())
                .await?
            } else {
                None
            };
            let package_id = if let Some(package_id) = existing_ready_package_id {
                package_id
            } else {
                upsert_exit_settlement_package(tx, org_uuid, case_id, body.settlement_input.clone())
                    .await?
            };
            let package_ready = sqlx::query(
                r#"
                SELECT severance_pay_won, missing_source_fields,
                       monthly_ordinary_wage_won, ordinary_daily_wage_won,
                       statutory_daily_wage_milliwon
                FROM employee_exit_settlement_packages
                WHERE org_id = $1 AND id = $2
                "#,
            )
            .bind(org_uuid)
            .bind(package_id)
            .fetch_one(tx.as_mut())
            .await?;
            let severance_pay_won: Option<i64> = package_ready.try_get("severance_pay_won")?;
            let missing_source_fields: Vec<String> =
                package_ready.try_get("missing_source_fields")?;
            if severance_pay_won.is_none() || !missing_source_fields.is_empty() {
                return Err(HrError::validation(
                    "exit settlement requires complete wage source fields before approval draft",
                ));
            }

            // Embed the 통상임금 basis into the filed approval document too (0093
            // review HIGH #2), from the same persisted columns the digest covers,
            // so a read-path digest recompute stays consistent.
            let approval_payload = with_ordinary_wage_basis(
                build_exit_approval_payload(&context, note.as_deref()),
                package_ready.try_get("monthly_ordinary_wage_won")?,
                package_ready.try_get("ordinary_daily_wage_won")?,
                package_ready.try_get("statutory_daily_wage_milliwon")?,
            );
            let package_status = if body.submit {
                "SUBMITTED"
            } else {
                "APPROVAL_DRAFTED"
            };
            sqlx::query(
                r#"
                UPDATE employee_exit_settlement_packages
                SET status = $3,
                    approval_payload = $4,
                    submitted_by = CASE WHEN $5 THEN $6 ELSE submitted_by END,
                    submitted_at = CASE WHEN $5 THEN now() ELSE submitted_at END,
                    -- Atomic re-uncertification (0093 HIGH): approval_payload is a
                    -- certification-covered field, so writing it invalidates any
                    -- prior certification in the same statement.
                    certification_status = 'UNCERTIFIED_DRAFT',
                    certification_artifact = NULL,
                    certified_package_digest = NULL,
                    updated_at = now()
                WHERE org_id = $1 AND id = $2
                "#,
            )
            .bind(org_uuid)
            .bind(package_id)
            .bind(package_status)
            .bind(approval_payload)
            .bind(body.submit)
            .bind(*actor.as_uuid())
            .execute(tx.as_mut())
            .await?;

            sqlx::query(
                r#"
                UPDATE employee_exit_cases
                SET status = $3,
                    approval_submitted_by = CASE WHEN $4 THEN $5 ELSE approval_submitted_by END,
                    approval_submitted_at = CASE WHEN $4 THEN now() ELSE approval_submitted_at END,
                    updated_at = now()
                WHERE org_id = $1 AND id = $2
                "#,
            )
            .bind(org_uuid)
            .bind(case_id)
            .bind(if body.submit {
                "SUBMITTED"
            } else {
                "APPROVAL_DRAFTED"
            })
            .bind(body.submit)
            .bind(*actor.as_uuid())
            .execute(tx.as_mut())
            .await?;

            load_exit_case_by_id(tx, org_uuid, case_id).await
        })
    })
    .await?;

    Ok(Json(exit_case))
}

async fn preview_attendance_import(
    State(state): State<HrState>,
    Extension(principal): Extension<Principal>,
    multipart: Multipart,
) -> Result<Json<AttendanceImportPreviewResponse>, HrError> {
    authorize_hr_org_wide(&principal, Feature::EmployeeDirectoryManage)?;
    let upload = read_attendance_upload(multipart).await?;
    let source_sha256 = sha256_hex(&upload.bytes);
    let parsed = parse_attendance_import_upload(&upload.filename, &upload.bytes)?;
    let org = principal.org_id;
    let org_uuid = *org.as_uuid();
    let actor = principal.user_id;
    let run_id = Uuid::new_v4();
    let input_rows = i32::try_from(parsed.rows.len())
        .map_err(|_| HrError::validation("attendance import row count exceeds i32"))?;
    let candidate_rows = i32::try_from(
        parsed
            .rows
            .iter()
            .filter(|row| row.row_status == ImportRowStatus::Candidate)
            .count(),
    )
    .map_err(|_| HrError::validation("attendance candidate row count exceeds i32"))?;
    let preserved_rows = i32::try_from(
        parsed
            .rows
            .iter()
            .filter(|row| row.row_status != ImportRowStatus::Candidate)
            .count(),
    )
    .map_err(|_| HrError::validation("attendance error row count exceeds i32"))?;
    let filename = upload.filename.clone();
    let source_format = attendance_source_format(&filename)?;
    let columns = attendance_import_columns_from_rows(&parsed.rows);
    let mapping_profile = attendance_import_mapping_profile(&columns);
    let audit_after = json!({
        "run_id": run_id,
        "entity_type": "attendance_direct",
        "source_filename": &filename,
        "source_sha256": &source_sha256,
        "input_rows": input_rows,
        "candidate_rows": candidate_rows,
        "preserved_rows": preserved_rows,
        "sensitive_values_returned": false,
        "payroll_effect": "lineage_only_not_payable"
    });
    let event = AuditEvent::new(
        Some(actor),
        AuditAction::new("attendance_import.preview").map_err(HrError::from_kernel)?,
        "data_import_run",
        run_id.to_string(),
        TraceContext::generate(),
        OffsetDateTime::now_utc(),
    )
    .with_org(org)
    .with_snapshots(None, Some(audit_after));

    let preview = with_audit::<_, _, HrError>(&state.pool, event, |tx| {
        let filename = filename.clone();
        let pay_period_start = upload.pay_period_start;
        let pay_period_end = upload.pay_period_end;
        let source_sha256 = source_sha256.clone();
        let parsed = parsed.clone();
        let mapping_profile = mapping_profile.clone();
        Box::pin(async move {
            sqlx::query(
                r#"
                INSERT INTO data_import_runs (
                    id, org_id, entity_type, status, source_filename, source_format,
                    source_sha256, mapping_profile, input_rows, candidate_rows,
                    preserved_rows, created_by, pay_period_start, pay_period_end
                )
                VALUES ($1, $2, 'attendance_direct', 'PREVIEWED', $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)
                "#,
            )
            .bind(run_id)
            .bind(org_uuid)
            .bind(&filename)
            .bind(source_format)
            .bind(&source_sha256)
            .bind(&mapping_profile)
            .bind(input_rows)
            .bind(candidate_rows)
            .bind(preserved_rows)
            .bind(*actor.as_uuid())
            .bind(pay_period_start)
            .bind(pay_period_end)
            .execute(tx.as_mut())
            .await?;

            for row in &parsed.rows {
                sqlx::query(
                    r#"
                    INSERT INTO data_import_rows (
                        org_id, run_id, source_sheet, source_row, source_key,
                        row_status, raw_row, canonical_row, validation
                    )
                    VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
                    "#,
                )
                .bind(org_uuid)
                .bind(run_id)
                .bind(&row.source_sheet)
                .bind(row.source_row)
                .bind(&row.source_key)
                .bind(row.row_status.as_str())
                .bind(&row.raw_row)
                .bind(attendance_canonical_row_json(row, &source_sha256))
                .bind(attendance_validation_json(row))
                .execute(tx.as_mut())
                .await?;
            }

            Ok(AttendanceImportPreviewResponse::from_rows(
                run_id,
                filename,
                source_sha256,
                parsed.rows,
            ))
        })
    })
    .await?;

    metrics::counter!("hr_attendance_import_runs_total", "stage" => "preview").increment(1);
    Ok(Json(preview))
}

async fn dry_run_attendance_import(
    State(state): State<HrState>,
    Extension(principal): Extension<Principal>,
    Path(run_id): Path<Uuid>,
) -> Result<Json<AttendanceImportDryRunSummary>, HrError> {
    authorize_hr_org_wide(&principal, Feature::EmployeeDirectoryManage)?;
    let org = principal.org_id;
    let org_uuid = *org.as_uuid();
    let actor = principal.user_id;

    let event = AuditEvent::new(
        Some(actor),
        AuditAction::new("attendance_import.dry_run").map_err(HrError::from_kernel)?,
        "data_import_run",
        run_id.to_string(),
        TraceContext::generate(),
        OffsetDateTime::now_utc(),
    )
    .with_org(org)
    .with_snapshots(
        None,
        Some(json!({ "run_id": run_id, "entity_type": "attendance_direct" })),
    );

    let summary = with_audit::<_, _, HrError>(&state.pool, event, |tx| {
        Box::pin(async move {
            let run = import_run_for_update(tx, org_uuid, run_id).await?;
            ensure_attendance_import_run(&run, &["PREVIEWED", "DRY_RUN"])?;
            let rows = load_attendance_import_rows(tx, org_uuid, run_id).await?;
            let summary = resolve_attendance_import_rows(tx, org_uuid, run_id, &run, &rows).await?;
            sqlx::query(
                r#"
                UPDATE data_import_runs
                SET status = 'DRY_RUN', dry_run_summary = $3, updated_at = now()
                WHERE org_id = $1 AND id = $2
                "#,
            )
            .bind(org_uuid)
            .bind(run_id)
            .bind(json!(&summary))
            .execute(tx.as_mut())
            .await?;
            Ok(summary)
        })
    })
    .await?;

    metrics::counter!("hr_attendance_import_runs_total", "stage" => "dry_run").increment(1);
    Ok(Json(summary))
}

async fn apply_attendance_import(
    State(state): State<HrState>,
    Extension(principal): Extension<Principal>,
    Path(run_id): Path<Uuid>,
    body: Option<Json<ImportApplyRequest>>,
) -> Result<Json<AttendanceImportApplyReport>, HrError> {
    authorize_hr_org_wide(&principal, Feature::EmployeeDirectoryManage)?;
    let checklist_all_acknowledged = body.and_then(|Json(b)| b.checklist_all_acknowledged);
    let gate_outcome = evaluate_ingest_checklist_gate(checklist_all_acknowledged)?;
    let org = principal.org_id;
    let org_uuid = *org.as_uuid();
    let actor = principal.user_id;

    let event = AuditEvent::new(
        Some(actor),
        AuditAction::new("attendance_import.apply").map_err(HrError::from_kernel)?,
        "data_import_run",
        run_id.to_string(),
        TraceContext::generate(),
        OffsetDateTime::now_utc(),
    )
    .with_org(org)
    .with_snapshots(
        None,
        Some(json!({
            "run_id": run_id,
            "entity_type": "attendance_direct",
            "gate_outcome": gate_outcome
        })),
    );

    let report = with_audit::<_, _, HrError>(&state.pool, event, |tx| {
            Box::pin(async move {
                let run = import_run_for_update(tx, org_uuid, run_id).await?;
                ensure_attendance_import_run(&run, &["DRY_RUN"])?;
                let rows = load_attendance_import_rows(tx, org_uuid, run_id).await?;
                let summary = resolve_attendance_import_rows(tx, org_uuid, run_id, &run, &rows).await?;
                if !summary.row_errors.is_empty() {
                    return Err(HrError::validation(
                        "cannot apply attendance import with unresolved or invalid rows",
                    ));
                }

                let mut inserted = 0usize;
                for row in summary.ready_rows_for_apply {
                    let insert_result = sqlx::query(
                        r#"
                        INSERT INTO attendance_direct_import_events (
                            org_id, run_id, import_row_id, employee_id, branch_id,
                            source_sheet, source_row, source_key, source_sha256,
                            employee_number, employee_name, branch_name, work_date,
                            check_in_at, check_out_at, minutes_worked, fact_key
                        )
                        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17)
                        ON CONFLICT (org_id, fact_key) DO NOTHING
                        "#,
                    )
                    .bind(org_uuid)
                    .bind(run_id)
                    .bind(row.import_row_id)
                    .bind(row.employee_id)
                    .bind(row.branch_id)
                    .bind(&row.source_sheet)
                    .bind(row.source_row)
                    .bind(&row.source_key)
                    .bind(&run.source_sha256)
                    .bind(&row.employee_number)
                    .bind(&row.employee_name)
                    .bind(&row.branch_name)
                    .bind(&row.work_date)
                    .bind(&row.check_in_at)
                    .bind(&row.check_out_at)
                    .bind(row.minutes_worked)
                    .bind(&row.fact_key)
                    .execute(tx.as_mut())
                    .await?;
                    inserted += usize::try_from(insert_result.rows_affected()).unwrap_or_default();
                }

                let report = AttendanceImportApplyReport {
                    run_id,
                    inserted,
                    skipped: summary.ready_rows.saturating_sub(inserted),
                    error_rows: 0,
                };
                sqlx::query(
                    r#"
                    UPDATE data_import_runs
                    SET status = 'APPLIED', apply_summary = $3, applied_by = $4,
                        applied_at = now(), updated_at = now()
                    WHERE org_id = $1 AND id = $2
                    "#,
                )
                .bind(org_uuid)
                .bind(run_id)
                .bind(json!(&report))
                .bind(*actor.as_uuid())
                .execute(tx.as_mut())
                .await?;
                Ok(report)
            })
        })
    .await?;

    metrics::counter!("hr_attendance_import_runs_total", "stage" => "apply").increment(1);
    Ok(Json(report))
}

async fn list_attendance_import_summary(
    State(state): State<HrState>,
    Extension(principal): Extension<Principal>,
    Query(query): Query<HrListQuery>,
) -> Result<Json<AttendanceImportSummaryPage>, HrError> {
    authorize_hr_org_wide(&principal, Feature::EmployeeDirectoryRead)?;
    record_hr_read("attendance_import_summary");
    let org = principal.org_id;
    let org_uuid = *org.as_uuid();
    let limit = query.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let offset = query.offset.unwrap_or(0).max(0);

    let (items, total) = with_org_conn::<_, _, HrError>(&state.pool, org, move |tx| {
        Box::pin(async move {
            let total: i64 = sqlx::query_scalar(
                r#"
                SELECT COUNT(*)
                FROM data_import_runs
                WHERE org_id = $1 AND entity_type = 'attendance_direct'
                "#,
            )
            .bind(org_uuid)
            .fetch_one(tx.as_mut())
            .await?;

            let items = sqlx::query(
                r#"
                SELECT
                    id, status, source_filename, source_format, source_sha256,
                    input_rows, candidate_rows, preserved_rows,
                    dry_run_summary, apply_summary, created_at, applied_at
                FROM data_import_runs
                WHERE org_id = $1 AND entity_type = 'attendance_direct'
                ORDER BY created_at DESC, id DESC
                LIMIT $2 OFFSET $3
                "#,
            )
            .bind(org_uuid)
            .bind(limit)
            .bind(offset)
            .fetch_all(tx.as_mut())
            .await?
            .into_iter()
            .map(|row| {
                Ok(AttendanceImportSummaryItem {
                    run_id: row.try_get("id")?,
                    status: row.try_get("status")?,
                    source_filename: row.try_get("source_filename")?,
                    source_format: row.try_get("source_format")?,
                    source_sha256: row.try_get("source_sha256")?,
                    input_rows: row.try_get("input_rows")?,
                    candidate_rows: row.try_get("candidate_rows")?,
                    preserved_rows: row.try_get("preserved_rows")?,
                    dry_run_summary: row.try_get("dry_run_summary")?,
                    apply_summary: row.try_get("apply_summary")?,
                    created_at: row.try_get("created_at")?,
                    applied_at: row.try_get("applied_at")?,
                })
            })
            .collect::<Result<Vec<_>, HrError>>()?;

            Ok((items, total))
        })
    })
    .await?;

    Ok(Json(AttendanceImportSummaryPage {
        items,
        total,
        limit,
        offset,
    }))
}
async fn import_employees(
    State(state): State<HrState>,
    Extension(principal): Extension<Principal>,
    multipart: Multipart,
) -> Result<Json<EmployeeImportReport>, HrError> {
    authorize_hr_org_wide(&principal, Feature::EmployeeDirectoryManage)?;
    require_leave_import_command_store(&state)?;
    let upload = read_xlsx_upload(multipart).await?;
    let source_ref = sha256_hex(&upload.bytes);
    let parsed = parse_employee_workbook(&upload.filename, &upload.bytes)?;
    let org = principal.org_id;
    let actor = principal.user_id;

    let report =
        apply_employee_import_batch(&state, org, actor, None, source_ref, parsed.rows, json!({}))
            .await?;

    record_hr_import(report.inserted, report.updated);
    Ok(Json(report))
}

async fn preview_employee_import(
    State(state): State<HrState>,
    Extension(principal): Extension<Principal>,
    multipart: Multipart,
) -> Result<Json<EmployeeImportPreviewResponse>, HrError> {
    authorize_hr_org_wide(&principal, Feature::EmployeeDirectoryManage)?;
    let upload = read_xlsx_upload(multipart).await?;
    let source_sha256 = sha256_hex(&upload.bytes);
    let parsed = parse_employee_import_workbook(&upload.filename, &upload.bytes)?;
    let org = principal.org_id;
    let org_uuid = *org.as_uuid();
    let actor = principal.user_id;
    let run_id = Uuid::new_v4();
    let mapping_profile = employee_import_mapping_profile(&parsed.columns);
    let input_rows = i32::try_from(parsed.input_rows())
        .map_err(|_| HrError::validation("import row count exceeds i32"))?;
    let candidate_rows = i32::try_from(parsed.candidate_rows())
        .map_err(|_| HrError::validation("candidate row count exceeds i32"))?;
    let preserved_rows = i32::try_from(parsed.preserved_rows())
        .map_err(|_| HrError::validation("preserved row count exceeds i32"))?;
    let audit_after = json!({
        "run_id": run_id,
        "entity_type": "employee_hr",
        "source_filename": &upload.filename,
        "source_sha256": &source_sha256,
        "input_rows": input_rows,
        "candidate_rows": candidate_rows,
        "preserved_rows": preserved_rows,
        "sensitive_values_returned": false
    });
    let event = AuditEvent::new(
        Some(actor),
        AuditAction::new("data_import.preview").map_err(HrError::from_kernel)?,
        "data_import_run",
        run_id.to_string(),
        TraceContext::generate(),
        OffsetDateTime::now_utc(),
    )
    .with_org(org)
    .with_snapshots(None, Some(audit_after));

    let preview = with_audit::<_, _, HrError>(&state.pool, event, |tx| {
        let rows = parsed.rows.clone();
        let columns = parsed.columns.clone();
        let filename = upload.filename.clone();
        let pay_period_start = upload.pay_period_start;
        let pay_period_end = upload.pay_period_end;
        let source_sha256 = source_sha256.clone();
        let mapping_profile = mapping_profile.clone();
        Box::pin(async move {
            sqlx::query(
                r#"
                INSERT INTO data_import_runs (
                    id, org_id, entity_type, status, source_filename, source_format,
                    source_sha256, mapping_profile, input_rows, candidate_rows,
                    preserved_rows, created_by, pay_period_start, pay_period_end
                )
                VALUES ($1, $2, 'employee_hr', 'PREVIEWED', $3, 'xlsx', $4, $5, $6, $7, $8, $9, $10, $11)
                "#,
            )
            .bind(run_id)
            .bind(org_uuid)
            .bind(&filename)
            .bind(&source_sha256)
            .bind(&mapping_profile)
            .bind(input_rows)
            .bind(candidate_rows)
            .bind(preserved_rows)
            .bind(*actor.as_uuid())
            .bind(pay_period_start)
            .bind(pay_period_end)
            .execute(tx.as_mut())
            .await?;

            for row in &rows {
                sqlx::query(
                    r#"
                    INSERT INTO data_import_rows (
                        org_id, run_id, source_sheet, source_row, source_key,
                        row_status, raw_row, canonical_row, validation
                    )
                    VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
                    "#,
                )
                .bind(org_uuid)
                .bind(run_id)
                .bind(&row.source_sheet)
                .bind(row.source_row)
                .bind(&row.source_key)
                .bind(row.row_status.as_str())
                .bind(&row.raw_row)
                .bind(import_canonical_row_json(row))
                .bind(import_validation_json(row))
                .execute(tx.as_mut())
                .await?;
            }

            Ok(EmployeeImportPreviewResponse::from_rows(
                run_id,
                filename,
                source_sha256,
                columns,
                rows,
            ))
        })
    })
    .await?;

    metrics::counter!("hr_data_import_runs_total", "stage" => "preview").increment(1);
    Ok(Json(preview))
}

async fn dry_run_employee_import(
    State(state): State<HrState>,
    Extension(principal): Extension<Principal>,
    Path(run_id): Path<Uuid>,
) -> Result<Json<EmployeeImportDryRunSummary>, HrError> {
    authorize_hr_org_wide(&principal, Feature::EmployeeDirectoryManage)?;
    let org = principal.org_id;
    let org_uuid = *org.as_uuid();
    let actor = principal.user_id;

    let event = AuditEvent::new(
        Some(actor),
        AuditAction::new("data_import.dry_run").map_err(HrError::from_kernel)?,
        "data_import_run",
        run_id.to_string(),
        TraceContext::generate(),
        OffsetDateTime::now_utc(),
    )
    .with_org(org)
    .with_snapshots(
        None,
        Some(json!({ "run_id": run_id, "entity_type": "employee_hr" })),
    );

    let summary = with_audit::<_, _, HrError>(&state.pool, event, |tx| {
        Box::pin(async move {
            let run = import_run_for_update(tx, org_uuid, run_id).await?;
            if run.entity_type != "employee_hr" {
                return Err(HrError::validation(
                    "import run entity_type is not employee_hr",
                ));
            }
            if run.status != "PREVIEWED" && run.status != "DRY_RUN" {
                return Err(HrError::from_kernel(KernelError::invalid_transition(
                    format!("cannot dry-run import run in {} status", run.status),
                )));
            }
            let rows = load_candidate_import_rows(tx, org_uuid, run_id).await?;
            let summary =
                compute_employee_import_dry_run(tx, org_uuid, run_id, &run, &rows).await?;
            sqlx::query(
                r#"
                UPDATE data_import_runs
                SET status = 'DRY_RUN', dry_run_summary = $3, updated_at = now()
                WHERE org_id = $1 AND id = $2
                "#,
            )
            .bind(org_uuid)
            .bind(run_id)
            .bind(json!(&summary))
            .execute(tx.as_mut())
            .await?;
            Ok(summary)
        })
    })
    .await?;

    metrics::counter!("hr_data_import_runs_total", "stage" => "dry_run").increment(1);
    Ok(Json(summary))
}

async fn apply_employee_import(
    State(state): State<HrState>,
    Extension(principal): Extension<Principal>,
    Path(run_id): Path<Uuid>,
    body: Option<Json<ImportApplyRequest>>,
) -> Result<Json<EmployeeImportReport>, HrError> {
    authorize_hr_org_wide(&principal, Feature::EmployeeDirectoryManage)?;
    require_leave_import_command_store(&state)?;
    let checklist_all_acknowledged = body.and_then(|Json(b)| b.checklist_all_acknowledged);
    let gate_outcome = evaluate_ingest_checklist_gate(checklist_all_acknowledged)?;
    let org = principal.org_id;
    let org_uuid = *org.as_uuid();
    let actor = principal.user_id;

    // The runtime connection only reads immutable candidate rows. One command
    // statement then owns every write and the staged-run transition.
    let rows = with_org_conn::<_, _, HrError>(&state.pool, org, |tx| {
        Box::pin(async move {
            let run = import_run_for_update(tx, org_uuid, run_id).await?;
            if run.entity_type != "employee_hr" {
                return Err(HrError::validation(
                    "import run entity_type is not employee_hr",
                ));
            }
            if run.status == "APPLIED" {
                return Ok(Vec::new());
            }
            if run.status != "DRY_RUN" {
                return Err(HrError::from_kernel(KernelError::invalid_transition(
                    format!("apply requires DRY_RUN status, got {}", run.status),
                )));
            }
            let rows = load_candidate_import_rows(tx, org_uuid, run_id).await?;
            let parsed_rows = rows
                .into_iter()
                .map(StoredEmployeeImportRow::into_parsed)
                .collect::<Result<Vec<_>, _>>()?;
            Ok(parsed_rows)
        })
    })
    .await?;
    let report = apply_employee_import_batch(
        &state,
        org,
        actor,
        Some(run_id),
        format!("run:{run_id}"),
        rows,
        json!({
            "run_id": run_id,
            "entity_type": "employee_hr",
            "gate_outcome": gate_outcome
        }),
    )
    .await?;

    record_hr_import(report.inserted, report.updated);
    metrics::counter!("hr_data_import_runs_total", "stage" => "apply").increment(1);
    Ok(Json(report))
}

async fn export_employees_csv(
    State(state): State<HrState>,
    Extension(principal): Extension<Principal>,
) -> Result<Response, HrError> {
    authorize_hr_org_wide(&principal, Feature::EmployeeDirectoryRead)?;
    authorize_hr_org_wide(&principal, Feature::ExcelDownload)?;
    let org = principal.org_id;
    let rows = with_org_conn::<_, _, HrError>(&state.pool, org, move |tx| {
        Box::pin(async move {
            sqlx::query(
                r#"
                SELECT company, name, employee_number, org_unit, worksite_name, job,
                       position, hire_date, exit_date, employment_status,
                       leave_remaining::TEXT AS leave_remaining
                FROM employees
                ORDER BY company ASC, name ASC, source_sheet ASC, source_row ASC
                LIMIT 10000
                "#,
            )
            .fetch_all(tx.as_mut())
            .await
            .map_err(HrError::from)
        })
    })
    .await?;

    let csv = standardized_employees_csv(&rows)?;
    let mut response = csv.into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/csv; charset=utf-8"),
    );
    response.headers_mut().insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_static("attachment; filename=\"employees-standard.csv\""),
    );
    Ok(response)
}

async fn list_employee_lifecycle_events(
    State(state): State<HrState>,
    Extension(principal): Extension<Principal>,
    Path(employee_id): Path<Uuid>,
) -> Result<Json<EmployeeLifecycleEventPage>, HrError> {
    authorize_hr_org_wide(&principal, Feature::EmployeeDirectoryRead)?;
    record_hr_read("lifecycle_events");
    let org = principal.org_id;
    let org_uuid = *org.as_uuid();

    let items = with_org_conn::<_, _, HrError>(&state.pool, org, move |tx| {
        Box::pin(async move {
            // Keep missing employees distinguishable from an employee with no
            // lifecycle events, without returning raw employee/import data.
            let exists: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM employees WHERE org_id = $1 AND id = $2)",
            )
            .bind(org_uuid)
            .bind(employee_id)
            .fetch_one(tx.as_mut())
            .await?;
            if !exists {
                return Err(HrError::from_kernel(KernelError::not_found(
                    "employee not found",
                )));
            }

            sqlx::query(
                r#"
                SELECT
                    id, employee_id, event_type, from_status, to_status,
                    from_company, to_company, from_org_unit, to_org_unit,
                    from_position, to_position, effective_date, comment,
                    signoffs, created_by, created_at
                FROM employee_lifecycle_events
                WHERE org_id = $1 AND employee_id = $2
                ORDER BY created_at DESC, id DESC
                LIMIT 200
                "#,
            )
            .bind(org_uuid)
            .bind(employee_id)
            .fetch_all(tx.as_mut())
            .await?
            .into_iter()
            .map(employee_lifecycle_event_from_row)
            .collect::<Result<Vec<_>, HrError>>()
        })
    })
    .await?;

    Ok(Json(EmployeeLifecycleEventPage { items }))
}

async fn create_employee_lifecycle_event(
    State(state): State<HrState>,
    Extension(principal): Extension<Principal>,
    Path(employee_id): Path<Uuid>,
    Json(body): Json<CreateEmployeeLifecycleEventRequest>,
) -> Result<Json<EmployeeLifecycleEventResponse>, HrError> {
    authorize_hr_org_wide(&principal, Feature::EmployeeDirectoryManage)?;

    let org = principal.org_id;
    let org_uuid = *org.as_uuid();
    let actor = principal.user_id;
    let transition = normalize_lifecycle_transition(body)?;

    let event = AuditEvent::new(
        Some(actor),
        AuditAction::new("employee.lifecycle.record").map_err(HrError::from_kernel)?,
        "employee",
        employee_id.to_string(),
        TraceContext::generate(),
        OffsetDateTime::now_utc(),
    )
    .with_org(org)
    .with_snapshots(
        None,
        Some(json!({
            "employee_id": employee_id,
            "event_type": &transition.event_type,
            "to_status": &transition.to_status,
            "effective_date": &transition.effective_date,
            "has_privacy_notice_ack": transition.signoffs.privacy_notice_ack,
            "has_korean_labor_law_ack": transition.signoffs.korean_labor_law_ack,
            "has_payroll_cutoff_ack": transition.signoffs.payroll_cutoff_ack,
            "has_retirement_settlement_ack": transition.signoffs.retirement_settlement_ack
        })),
    );

    let item = with_audit::<_, _, HrError>(&state.pool, event, |tx| {
        Box::pin(async move {
            let current = load_employee_for_lifecycle(tx, org_uuid, employee_id).await?;
            validate_lifecycle_transition(&current, &transition)?;
            let next_company = transition
                .to_company
                .clone()
                .unwrap_or_else(|| current.company.clone());
            let next_org_unit = transition
                .to_org_unit
                .clone()
                .or_else(|| current.org_unit.clone());
            let next_position = transition
                .to_position
                .clone()
                .or_else(|| current.position.clone());
            let lifecycle_id = Uuid::new_v4();

            sqlx::query(
                r#"
                INSERT INTO employee_lifecycle_events (
                    id, org_id, employee_id, event_type, from_status, to_status,
                    from_company, to_company, from_org_unit, to_org_unit,
                    from_position, to_position, effective_date, comment,
                    signoffs, created_by
                )
                VALUES (
                    $1, $2, $3, $4, $5, $6,
                    $7, $8, $9, $10,
                    $11, $12, $13, $14,
                    $15, $16
                )
                "#,
            )
            .bind(lifecycle_id)
            .bind(org_uuid)
            .bind(employee_id)
            .bind(&transition.event_type)
            .bind(&current.employment_status)
            .bind(&transition.to_status)
            .bind(&current.company)
            .bind(&next_company)
            .bind(current.org_unit.as_deref())
            .bind(next_org_unit.as_deref())
            .bind(current.position.as_deref())
            .bind(next_position.as_deref())
            .bind(&transition.effective_date)
            .bind(&transition.comment)
            .bind(json!(&transition.signoffs))
            .bind(*actor.as_uuid())
            .execute(tx.as_mut())
            .await?;

            // The Employment owner's statement, inside this transaction. See
            // [`create_employee_core`] for why the DML is not held here.
            employment::apply_employment_change(
                tx,
                org_uuid,
                employee_id,
                employment::EmploymentChange {
                    company: &next_company,
                    org_unit: next_org_unit.as_deref(),
                    position: next_position.as_deref(),
                    employment_status: &transition.to_status,
                    effective_date: &transition.effective_date,
                },
            )
            .await?;

            let row = sqlx::query(
                r#"
                SELECT
                    id, employee_id, event_type, from_status, to_status,
                    from_company, to_company, from_org_unit, to_org_unit,
                    from_position, to_position, effective_date, comment,
                    signoffs, created_by, created_at
                FROM employee_lifecycle_events
                WHERE org_id = $1 AND id = $2
                "#,
            )
            .bind(org_uuid)
            .bind(lifecycle_id)
            .fetch_one(tx.as_mut())
            .await?;

            employee_lifecycle_event_from_row(row)
        })
    })
    .await?;

    metrics::counter!("hr_employee_lifecycle_events_total", "event_type" => item.event_type.clone())
        .increment(1);
    Ok(Json(item))
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct EmployeeImportReport {
    input_rows: usize,
    inserted: usize,
    updated: usize,
    skipped: usize,
    companies: Vec<CompanyImportSummary>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct CompanyImportSummary {
    company: String,
    input_rows: usize,
    inserted: usize,
    updated: usize,
    skipped: usize,
}

#[derive(Debug, Clone, Serialize)]
struct EmployeeImportColumn {
    source_header: String,
    normalized_header: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    target: Option<String>,
    classification: String,
    preview_allowed: bool,
}

#[derive(Debug, Serialize)]
struct EmployeeImportPreviewRow {
    source_sheet: String,
    source_row: i32,
    row_status: String,
    values: BTreeMap<String, Value>,
}

#[derive(Debug, Serialize)]
struct EmployeeImportPreviewResponse {
    run_id: Uuid,
    entity_type: String,
    source_filename: String,
    source_sha256: String,
    input_rows: usize,
    candidate_rows: usize,
    preserved_rows: usize,
    columns: Vec<EmployeeImportColumn>,
    sample_rows: Vec<EmployeeImportPreviewRow>,
    mapping_profile: Value,
}

impl EmployeeImportPreviewResponse {
    fn from_rows(
        run_id: Uuid,
        source_filename: String,
        source_sha256: String,
        columns: Vec<EmployeeImportColumn>,
        rows: Vec<ParsedEmployeeImportRow>,
    ) -> Self {
        let sample_rows = rows
            .iter()
            .take(12)
            .map(|row| EmployeeImportPreviewRow {
                source_sheet: row.source_sheet.clone(),
                source_row: row.source_row,
                row_status: row.row_status.as_str().to_owned(),
                values: masked_preview_values(&row.raw_row, &columns),
            })
            .collect::<Vec<_>>();
        let input_rows = rows.len();
        let candidate_rows = rows
            .iter()
            .filter(|row| row.row_status == ImportRowStatus::Candidate)
            .count();
        let preserved_rows = rows
            .iter()
            .filter(|row| row.row_status == ImportRowStatus::Preserved)
            .count();
        let mapping_profile = employee_import_mapping_profile(&columns);

        Self {
            run_id,
            entity_type: "employee_hr".to_owned(),
            source_filename,
            source_sha256,
            input_rows,
            candidate_rows,
            preserved_rows,
            columns,
            sample_rows,
            mapping_profile,
        }
    }
}

#[derive(Debug, Serialize)]
struct EmployeeImportDryRunSummary {
    run_id: Uuid,
    input_rows: usize,
    candidate_rows: usize,
    preserved_rows: usize,
    insert_candidates: usize,
    update_candidates: usize,
    companies: Vec<CompanyImportSummary>,
}

#[derive(Debug, Clone, Serialize)]
struct AttendanceImportColumn {
    source_header: String,
    normalized_header: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    target: Option<String>,
    classification: String,
    preview_allowed: bool,
}

#[derive(Debug, Serialize)]
struct AttendanceImportPreviewRow {
    source_sheet: String,
    source_row: i32,
    row_status: String,
    values: BTreeMap<String, Value>,
    validation: Value,
}

#[derive(Debug, Serialize)]
struct AttendanceImportPreviewResponse {
    run_id: Uuid,
    entity_type: String,
    source_filename: String,
    source_sha256: String,
    input_rows: usize,
    candidate_rows: usize,
    preserved_rows: usize,
    columns: Vec<AttendanceImportColumn>,
    sample_rows: Vec<AttendanceImportPreviewRow>,
    mapping_profile: Value,
}

impl AttendanceImportPreviewResponse {
    fn from_rows(
        run_id: Uuid,
        source_filename: String,
        source_sha256: String,
        rows: Vec<ParsedAttendanceImportRow>,
    ) -> Self {
        let columns = attendance_import_columns_from_rows(&rows);
        let sample_rows = rows
            .iter()
            .take(12)
            .map(|row| AttendanceImportPreviewRow {
                source_sheet: row.source_sheet.clone(),
                source_row: row.source_row,
                row_status: row.row_status.as_str().to_owned(),
                values: attendance_preview_values(&row.raw_row, &columns),
                validation: attendance_validation_json(row),
            })
            .collect::<Vec<_>>();
        let input_rows = rows.len();
        let candidate_rows = rows
            .iter()
            .filter(|row| row.row_status == ImportRowStatus::Candidate)
            .count();
        let preserved_rows = rows
            .iter()
            .filter(|row| row.row_status != ImportRowStatus::Candidate)
            .count();

        Self {
            run_id,
            entity_type: "attendance_direct".to_owned(),
            source_filename,
            source_sha256,
            input_rows,
            candidate_rows,
            preserved_rows,
            columns: columns.clone(),
            sample_rows,
            mapping_profile: attendance_import_mapping_profile(&columns),
        }
    }
}

#[derive(Debug, Default, Serialize)]
struct AttendanceImportDryRunSummary {
    run_id: Uuid,
    input_rows: usize,
    candidate_rows: usize,
    preserved_rows: usize,
    ready_rows: usize,
    error_rows: usize,
    duplicate_rows: usize,
    missing_employee_rows: usize,
    ambiguous_employee_rows: usize,
    row_errors: Vec<AttendanceImportRowError>,
    #[serde(skip_serializing)]
    ready_rows_for_apply: Vec<ResolvedAttendanceImportRow>,
}

#[derive(Debug, Clone, Serialize)]
struct AttendanceImportRowError {
    source_sheet: String,
    source_row: i32,
    source_key: String,
    code: String,
    message: String,
}

#[derive(Debug, Serialize)]
struct AttendanceImportApplyReport {
    run_id: Uuid,
    inserted: usize,
    skipped: usize,
    error_rows: usize,
}

#[derive(Debug, Serialize)]
struct AttendanceImportSummaryItem {
    run_id: Uuid,
    status: String,
    source_filename: String,
    source_format: String,
    source_sha256: String,
    input_rows: i32,
    candidate_rows: i32,
    preserved_rows: i32,
    dry_run_summary: Value,
    apply_summary: Value,
    created_at: OffsetDateTime,
    #[serde(skip_serializing_if = "Option::is_none")]
    applied_at: Option<OffsetDateTime>,
}

#[derive(Debug, Serialize)]
struct AttendanceImportSummaryPage {
    items: Vec<AttendanceImportSummaryItem>,
    total: i64,
    limit: i64,
    offset: i64,
}

#[derive(Debug)]
struct DataImportRunRecord {
    entity_type: String,
    status: String,
    source_sha256: String,
    input_rows: i32,
    candidate_rows: i32,
    preserved_rows: i32,
}

#[derive(Debug)]
struct StoredEmployeeImportRow {
    company: String,
    name: String,
    source_filename: String,
    source_sheet: String,
    source_row: i32,
    source_key: String,
    raw_row: Value,
    source_metadata: Value,
    canonical: EmployeeCanonicalFields,
}

impl StoredEmployeeImportRow {
    fn into_parsed(self) -> Result<ParsedEmployeeRow, HrError> {
        if self.name.trim().is_empty() {
            return Err(HrError::validation(
                "candidate import row is missing required employee name",
            ));
        }
        Ok(ParsedEmployeeRow {
            company: self.company,
            name: self.name,
            source_filename: self.source_filename,
            source_sheet: self.source_sheet,
            source_row: self.source_row,
            source_key: self.source_key,
            raw_row: self.raw_row,
            source_metadata: self.source_metadata,
            canonical: self.canonical,
        })
    }
}

struct XlsxUpload {
    filename: String,
    bytes: Vec<u8>,
    /// The pay period this upload covers, declared by the uploader.
    ///
    /// Not optional and never defaulted. `data_import_runs.pay_period_*` is NOT
    /// NULL precisely so an upload cannot acquire a period nobody chose; a
    /// default here would put that fabricated provenance back one layer up.
    pay_period_start: Date,
    pay_period_end: Date,
}

/// Read one multipart text part as a `YYYY-MM-DD` date.
fn parse_period_part(name: &str, raw: &str) -> Result<Date, HrError> {
    Date::parse(
        raw.trim(),
        &time::format_description::well_known::Iso8601::DATE,
    )
    .map_err(|_| HrError::validation(format!("{name} must be an ISO date (YYYY-MM-DD)")))
}

/// Collect the file part and the declared pay period from a multipart upload.
///
/// EVERY part is drained before anything is decided. The previous readers
/// `return Ok(...)` from INSIDE the `next_field()` loop the moment they had the
/// file, so any part sent AFTER `file` was silently discarded -- a period field
/// would have vanished with no error at all, and the upload would have been
/// rejected later for a reason that named the wrong thing.
async fn read_periodised_upload(
    mut multipart: Multipart,
    default_filename: &str,
    accept: fn(&str) -> bool,
    accept_message: &'static str,
) -> Result<XlsxUpload, HrError> {
    let mut filename: Option<String> = None;
    let mut bytes: Option<Vec<u8>> = None;
    let mut start_raw: Option<String> = None;
    let mut end_raw: Option<String> = None;

    while let Some(mut field) = multipart
        .next_field()
        .await
        .map_err(|err| HrError::validation(err.to_string()))?
    {
        match field.name() {
            Some("file") => {
                let name = field
                    .file_name()
                    .map(ToOwned::to_owned)
                    .unwrap_or_else(|| default_filename.to_owned());
                if !accept(&name.to_ascii_lowercase()) {
                    return Err(HrError::validation(accept_message));
                }
                let mut buffer = Vec::new();
                while let Some(chunk) = field
                    .chunk()
                    .await
                    .map_err(|err| HrError::validation(err.to_string()))?
                {
                    if buffer.len() + chunk.len() > MAX_IMPORT_BYTES {
                        return Err(HrError::validation(
                            "uploaded file exceeds the maximum import size",
                        ));
                    }
                    buffer.extend_from_slice(&chunk);
                }
                filename = Some(name);
                bytes = Some(buffer);
            }
            Some("pay_period_start") => {
                start_raw = Some(
                    field
                        .text()
                        .await
                        .map_err(|err| HrError::validation(err.to_string()))?,
                );
            }
            Some("pay_period_end") => {
                end_raw = Some(
                    field
                        .text()
                        .await
                        .map_err(|err| HrError::validation(err.to_string()))?,
                );
            }
            _ => continue,
        }
    }

    let (Some(filename), Some(bytes)) = (filename, bytes) else {
        return Err(HrError::validation(
            "multipart upload is missing the 'file' field",
        ));
    };
    if bytes.is_empty() {
        return Err(HrError::validation("uploaded file is empty"));
    }
    // Required, not defaulted. An import with no declared period cannot be
    // scoped to a payroll run, and guessing one is how a roster ends up built
    // from the wrong month.
    let (Some(start_raw), Some(end_raw)) = (start_raw, end_raw) else {
        return Err(HrError::validation(
            "multipart upload must declare 'pay_period_start' and 'pay_period_end'",
        ));
    };
    let pay_period_start = parse_period_part("pay_period_start", &start_raw)?;
    let pay_period_end = parse_period_part("pay_period_end", &end_raw)?;
    if pay_period_end < pay_period_start {
        return Err(HrError::validation(
            "pay_period_end must not precede pay_period_start",
        ));
    }
    Ok(XlsxUpload {
        filename,
        bytes,
        pay_period_start,
        pay_period_end,
    })
}

async fn read_xlsx_upload(multipart: Multipart) -> Result<XlsxUpload, HrError> {
    read_periodised_upload(
        multipart,
        "employees.xlsx",
        |lower| lower.ends_with(".xlsx"),
        "employee import currently accepts .xlsx workbooks only",
    )
    .await
}

async fn read_attendance_upload(multipart: Multipart) -> Result<XlsxUpload, HrError> {
    read_periodised_upload(
        multipart,
        "attendance.csv",
        |lower| lower.ends_with(".xlsx") || lower.ends_with(".csv"),
        "attendance import accepts .xlsx workbooks or .csv files only",
    )
    .await
}

#[derive(Debug)]
struct ParsedEmployeeWorkbook {
    rows: Vec<ParsedEmployeeRow>,
}

#[derive(Debug, Clone)]
struct ParsedEmployeeRow {
    company: String,
    name: String,
    source_filename: String,
    source_sheet: String,
    source_row: i32,
    source_key: String,
    raw_row: Value,
    source_metadata: Value,
    canonical: EmployeeCanonicalFields,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct EmployeeCanonicalFields {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    employee_number: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    org_unit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    job: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    position: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    worksite_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    worksite_address: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    hire_date: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    exit_date: Option<String>,
    #[serde(default = "default_active_status")]
    employment_status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    leave_accrued: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    leave_used: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    leave_remaining: Option<String>,
}

fn default_active_status() -> String {
    "ACTIVE".to_owned()
}

#[derive(Debug, Clone)]
struct ParsedEmployeeImportWorkbook {
    rows: Vec<ParsedEmployeeImportRow>,
    columns: Vec<EmployeeImportColumn>,
}

impl ParsedEmployeeImportWorkbook {
    fn input_rows(&self) -> usize {
        self.rows.len()
    }

    fn candidate_rows(&self) -> usize {
        self.rows
            .iter()
            .filter(|row| row.row_status == ImportRowStatus::Candidate)
            .count()
    }

    fn preserved_rows(&self) -> usize {
        self.rows
            .iter()
            .filter(|row| row.row_status == ImportRowStatus::Preserved)
            .count()
    }
}

#[derive(Debug, Clone)]
struct ParsedEmployeeImportRow {
    company: String,
    name: Option<String>,
    source_filename: String,
    source_sheet: String,
    source_row: i32,
    source_key: String,
    raw_row: Value,
    source_metadata: Value,
    canonical: Option<EmployeeCanonicalFields>,
    row_status: ImportRowStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ImportRowStatus {
    Candidate,
    Preserved,
    Error,
}

impl ImportRowStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Candidate => "CANDIDATE",
            Self::Preserved => "PRESERVED",
            Self::Error => "ERROR",
        }
    }

    fn from_db(value: &str) -> Result<Self, HrError> {
        match value {
            "CANDIDATE" => Ok(Self::Candidate),
            "PRESERVED" => Ok(Self::Preserved),
            "ERROR" => Ok(Self::Error),
            _ => Err(HrError::validation("stored import row has invalid status")),
        }
    }
}

#[derive(Debug, Clone)]
struct ParsedAttendanceImportUpload {
    rows: Vec<ParsedAttendanceImportRow>,
}

#[derive(Debug, Clone)]
struct ParsedAttendanceImportRow {
    source_sheet: String,
    source_row: i32,
    source_key: String,
    raw_row: Value,
    employee_number: Option<String>,
    employee_name: Option<String>,
    branch_name: Option<String>,
    work_date: Option<String>,
    check_in_at: Option<String>,
    check_out_at: Option<String>,
    minutes_worked: Option<i32>,
    row_status: ImportRowStatus,
    validation_errors: Vec<String>,
}

impl ParsedAttendanceImportRow {
    fn duplicate_fingerprint(&self) -> Option<String> {
        Some(format!(
            "employee:{}|name:{}|branch:{}|date:{}|in:{}|out:{}|minutes:{}",
            self.employee_number
                .as_deref()
                .or(self.employee_name.as_deref())?,
            self.employee_name.as_deref().unwrap_or_default(),
            self.branch_name.as_deref()?,
            self.work_date.as_deref()?,
            self.check_in_at.as_deref().unwrap_or_default(),
            self.check_out_at.as_deref().unwrap_or_default(),
            self.minutes_worked
                .map(|value| value.to_string())
                .unwrap_or_default()
        ))
    }
}

#[derive(Debug)]
struct AttendanceImportHeader {
    zero_based_row: usize,
    normalized_headers: Vec<String>,
    columns: Vec<AttendanceImportColumn>,
}

#[derive(Debug)]
struct AttendanceImportFieldDefinition {
    target: &'static str,
    aliases: &'static [&'static str],
    required: bool,
}

const ATTENDANCE_IMPORT_FIELD_DEFINITIONS: &[AttendanceImportFieldDefinition] = &[
    AttendanceImportFieldDefinition {
        target: "employee_number",
        aliases: &[
            "사번",
            "직원번호",
            "임직원번호",
            "employee_number",
            "employeenumber",
        ],
        required: false,
    },
    AttendanceImportFieldDefinition {
        target: "employee_name",
        aliases: &[
            "성명",
            "이름",
            "직원명",
            "사원명",
            "근로자명",
            "employee_name",
            "employeename",
        ],
        required: false,
    },
    AttendanceImportFieldDefinition {
        target: "branch_name",
        aliases: &[
            "지점",
            "지점명",
            "근무지",
            "사업장",
            "branch",
            "branch_name",
            "branchname",
        ],
        required: true,
    },
    AttendanceImportFieldDefinition {
        target: "work_date",
        aliases: &[
            "근무일",
            "일자",
            "날짜",
            "출근일",
            "work_date",
            "workdate",
            "date",
        ],
        required: true,
    },
    AttendanceImportFieldDefinition {
        target: "check_in_at",
        aliases: &[
            "출근",
            "출근시간",
            "시작시간",
            "clock_in",
            "check_in",
            "checkin",
        ],
        required: false,
    },
    AttendanceImportFieldDefinition {
        target: "check_out_at",
        aliases: &[
            "퇴근",
            "퇴근시간",
            "종료시간",
            "clock_out",
            "check_out",
            "checkout",
        ],
        required: false,
    },
    AttendanceImportFieldDefinition {
        target: "minutes_worked",
        aliases: &[
            "근무분",
            "근무시간분",
            "근무시간",
            "minutes_worked",
            "minutesworked",
            "work_minutes",
        ],
        required: false,
    },
];

fn parse_attendance_import_upload(
    filename: &str,
    bytes: &[u8],
) -> Result<ParsedAttendanceImportUpload, HrError> {
    let lower = filename.to_ascii_lowercase();
    let mut rows = if lower.ends_with(".csv") {
        let text = std::str::from_utf8(bytes)
            .map_err(|_| HrError::workbook("attendance CSV must be valid UTF-8"))?;
        parse_attendance_csv(filename, text)?
    } else if lower.ends_with(".xlsx") {
        parse_attendance_xlsx(filename, bytes)?
    } else {
        return Err(HrError::validation(
            "attendance import accepts .xlsx workbooks or .csv files only",
        ));
    };

    mark_duplicate_attendance_rows(&mut rows);
    if rows.is_empty() {
        return Err(HrError::workbook(
            "attendance import did not contain any non-empty data rows",
        ));
    }
    Ok(ParsedAttendanceImportUpload { rows })
}

fn parse_attendance_xlsx(
    filename: &str,
    bytes: &[u8],
) -> Result<Vec<ParsedAttendanceImportRow>, HrError> {
    let mut workbook =
        Xlsx::new(Cursor::new(bytes)).map_err(|err| HrError::workbook(err.to_string()))?;
    let mut rows = Vec::new();
    for sheet in workbook.sheet_names().to_owned() {
        let range = workbook
            .worksheet_range(&sheet)
            .map_err(|err| HrError::workbook(err.to_string()))?;
        let values = range
            .rows()
            .map(|row| row.iter().map(cell_json).collect::<Vec<_>>())
            .collect::<Vec<_>>();
        rows.extend(parse_attendance_tabular_sheet(
            filename, &sheet, &values, false,
        )?);
    }
    Ok(rows)
}

fn parse_attendance_csv(
    filename: &str,
    text: &str,
) -> Result<Vec<ParsedAttendanceImportRow>, HrError> {
    let rows = parse_csv_rows(text)?
        .into_iter()
        .map(|row| row.into_iter().map(Value::String).collect::<Vec<_>>())
        .collect::<Vec<_>>();
    parse_attendance_tabular_sheet(filename, "CSV", &rows, true)
}

fn parse_attendance_tabular_sheet(
    _filename: &str,
    sheet: &str,
    rows: &[Vec<Value>],
    require_header: bool,
) -> Result<Vec<ParsedAttendanceImportRow>, HrError> {
    let Some(header) = detect_attendance_import_header(rows) else {
        return if require_header {
            Err(HrError::workbook(
                "attendance import is missing required headers",
            ))
        } else {
            Ok(Vec::new())
        };
    };

    let mut parsed = Vec::new();
    for (zero_based_idx, row) in rows
        .iter()
        .enumerate()
        .skip(header.zero_based_row.saturating_add(1))
    {
        if !row.iter().any(|cell| json_value_text(cell).is_some()) {
            continue;
        }
        let source_row = i32::try_from(zero_based_idx + 1)
            .map_err(|_| HrError::workbook("source row does not fit i32"))?;
        let raw_row = attendance_raw_row(row, &header.normalized_headers);
        let employee_number =
            raw_text_for_attendance_target(&raw_row, &header.columns, "employee_number");
        let employee_name =
            raw_text_for_attendance_target(&raw_row, &header.columns, "employee_name");
        let branch_name = raw_text_for_attendance_target(&raw_row, &header.columns, "branch_name");
        let work_date_raw = raw_text_for_attendance_target(&raw_row, &header.columns, "work_date");
        let (work_date, invalid_work_date) = match work_date_raw.as_deref() {
            Some(value) => match normalized_work_date(value) {
                Ok(date) => (Some(date), false),
                Err(_) => (None, true),
            },
            None => (None, false),
        };
        let check_in_raw = raw_text_for_attendance_target(&raw_row, &header.columns, "check_in_at");
        let (check_in_at, invalid_check_in_at) = match check_in_raw.as_deref() {
            Some(value) => match normalized_attendance_time(value) {
                Ok(time) => (Some(time), false),
                Err(_) => (None, true),
            },
            None => (None, false),
        };
        let check_out_raw =
            raw_text_for_attendance_target(&raw_row, &header.columns, "check_out_at");
        let (check_out_at, invalid_check_out_at) = match check_out_raw.as_deref() {
            Some(value) => match normalized_attendance_time(value) {
                Ok(time) => (Some(time), false),
                Err(_) => (None, true),
            },
            None => (None, false),
        };
        let minutes_raw =
            raw_text_for_attendance_target(&raw_row, &header.columns, "minutes_worked");
        let (minutes_worked, invalid_minutes) = match minutes_raw.as_deref() {
            Some(value) => match normalized_minutes_worked(value) {
                Ok(minutes) => (Some(minutes), false),
                Err(_) => (None, true),
            },
            None => (None, false),
        };

        let mut validation_errors = Vec::new();
        if employee_number.is_none() && employee_name.is_none() {
            validation_errors.push("missing_employee_identifier".to_owned());
        }
        if branch_name.is_none() {
            validation_errors.push("missing_branch_name".to_owned());
        }
        if work_date.is_none() {
            validation_errors.push("missing_work_date".to_owned());
        }
        if invalid_work_date {
            validation_errors.push("invalid_work_date".to_owned());
        }
        if invalid_minutes {
            validation_errors.push("invalid_minutes_worked".to_owned());
        }
        if invalid_check_in_at {
            validation_errors.push("invalid_check_in_at".to_owned());
        }
        if invalid_check_out_at {
            validation_errors.push("invalid_check_out_at".to_owned());
        }
        if check_in_at.is_none() && check_out_at.is_none() && minutes_worked.is_none() {
            validation_errors.push("missing_attendance_time".to_owned());
        }

        let row_status = if validation_errors.is_empty() {
            ImportRowStatus::Candidate
        } else {
            ImportRowStatus::Error
        };
        parsed.push(ParsedAttendanceImportRow {
            source_sheet: sheet.to_owned(),
            source_row,
            source_key: format!("sheet:{sheet}|row:{source_row}"),
            raw_row,
            employee_number,
            employee_name,
            branch_name,
            work_date,
            check_in_at,
            check_out_at,
            minutes_worked,
            row_status,
            validation_errors,
        });
    }
    Ok(parsed)
}

fn detect_attendance_import_header(rows: &[Vec<Value>]) -> Option<AttendanceImportHeader> {
    for (zero_based_row, row) in rows.iter().enumerate().take(MAX_IMPORT_HEADER_SCAN_ROWS) {
        let normalized_headers = row
            .iter()
            .map(|cell| {
                json_value_text(cell)
                    .map_or_else(String::new, |value| normalize_header_label(&value))
            })
            .collect::<Vec<_>>();
        let targets = normalized_headers
            .iter()
            .filter_map(|header| attendance_import_target_for_header(header))
            .collect::<BTreeSet<_>>();
        let has_employee = targets.contains("employee_number") || targets.contains("employee_name");
        let has_required = ATTENDANCE_IMPORT_FIELD_DEFINITIONS
            .iter()
            .filter(|field| field.required)
            .all(|field| targets.contains(field.target));
        if has_employee && has_required {
            let columns = normalized_headers
                .iter()
                .filter(|header| !header.is_empty())
                .map(|header| attendance_import_column(header, header))
                .collect::<Vec<_>>();
            return Some(AttendanceImportHeader {
                zero_based_row,
                normalized_headers,
                columns,
            });
        }
    }
    None
}

fn attendance_raw_row(row: &[Value], normalized_headers: &[String]) -> Value {
    let mut raw = Map::new();
    for (idx, header_label) in normalized_headers.iter().enumerate() {
        if header_label.is_empty() {
            continue;
        }
        raw.insert(
            header_label.clone(),
            row.get(idx).cloned().unwrap_or(Value::Null),
        );
    }
    Value::Object(raw)
}

fn mark_duplicate_attendance_rows(rows: &mut [ParsedAttendanceImportRow]) {
    let mut counts = BTreeMap::<String, usize>::new();
    for row in rows.iter() {
        if row.row_status == ImportRowStatus::Candidate
            && let Some(key) = row.duplicate_fingerprint()
        {
            *counts.entry(key).or_default() += 1;
        }
    }
    for row in rows.iter_mut() {
        let Some(key) = row.duplicate_fingerprint() else {
            continue;
        };
        if counts.get(&key).copied().unwrap_or_default() > 1 {
            row.row_status = ImportRowStatus::Error;
            row.validation_errors
                .push("duplicate_row_in_file".to_owned());
        }
    }
}

fn parse_csv_rows(text: &str) -> Result<Vec<Vec<String>>, HrError> {
    let mut rows = Vec::new();
    let mut row = Vec::new();
    let mut field = String::new();
    let mut chars = text.chars().peekable();
    let mut in_quotes = false;

    while let Some(ch) = chars.next() {
        match ch {
            '"' if in_quotes && chars.peek() == Some(&'"') => {
                field.push('"');
                chars.next();
            }
            '"' => {
                in_quotes = !in_quotes;
            }
            ',' if !in_quotes => {
                row.push(field.trim().to_owned());
                field.clear();
            }
            '\n' if !in_quotes => {
                row.push(field.trim().to_owned());
                field.clear();
                if row.iter().any(|value| !value.is_empty()) {
                    rows.push(std::mem::take(&mut row));
                } else {
                    row.clear();
                }
            }
            '\r' if !in_quotes => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                row.push(field.trim().to_owned());
                field.clear();
                if row.iter().any(|value| !value.is_empty()) {
                    rows.push(std::mem::take(&mut row));
                } else {
                    row.clear();
                }
            }
            _ => field.push(ch),
        }
    }

    if in_quotes {
        return Err(HrError::workbook(
            "attendance CSV has an unclosed quoted field",
        ));
    }
    row.push(field.trim().to_owned());
    if row.iter().any(|value| !value.is_empty()) {
        rows.push(row);
    }
    Ok(rows)
}

fn normalized_minutes_worked(value: &str) -> Result<i32, HrError> {
    let cleaned = value.replace(',', "").trim().to_owned();
    let minutes = cleaned
        .parse::<i32>()
        .map_err(|_| HrError::validation("minutes_worked must be a whole number"))?;
    if minutes < 0 {
        return Err(HrError::validation("minutes_worked must not be negative"));
    }
    Ok(minutes)
}

fn normalized_work_date(value: &str) -> Result<String, HrError> {
    let cleaned = value.trim().replace(['.', '/'], "-");
    let parts = cleaned.split('-').collect::<Vec<_>>();
    if parts.len() != 3 {
        return Err(HrError::validation("work_date must use YYYY-MM-DD"));
    }
    let year = parts[0]
        .parse::<u16>()
        .map_err(|_| HrError::validation("work_date year must be numeric"))?;
    let month = parts[1]
        .parse::<u8>()
        .map_err(|_| HrError::validation("work_date month must be numeric"))?;
    let day = parts[2]
        .parse::<u8>()
        .map_err(|_| HrError::validation("work_date day must be numeric"))?;
    if !(1900..=2100).contains(&year) || month == 0 || month > 12 {
        return Err(HrError::validation(
            "work_date is outside the supported date range",
        ));
    }
    let max_day = days_in_month(year, month);
    if day == 0 || day > max_day {
        return Err(HrError::validation("work_date day is invalid"));
    }
    Ok(format!("{year:04}-{month:02}-{day:02}"))
}

fn normalized_attendance_time(value: &str) -> Result<String, HrError> {
    let cleaned = value.trim();
    let parts = cleaned.split(':').collect::<Vec<_>>();
    if parts.len() != 2 {
        return Err(HrError::validation("attendance time must use HH:MM"));
    }
    let hour = parts[0]
        .parse::<u8>()
        .map_err(|_| HrError::validation("attendance time hour must be numeric"))?;
    let minute = parts[1]
        .parse::<u8>()
        .map_err(|_| HrError::validation("attendance time minute must be numeric"))?;
    if hour > 23 || minute > 59 {
        return Err(HrError::validation(
            "attendance time is outside the supported range",
        ));
    }
    Ok(format!("{hour:02}:{minute:02}"))
}

fn days_in_month(year: u16, month: u8) -> u8 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap_year(year) => 29,
        2 => 28,
        _ => 0,
    }
}

fn is_leap_year(year: u16) -> bool {
    (year.is_multiple_of(4) && !year.is_multiple_of(100)) || year.is_multiple_of(400)
}

fn attendance_source_format(filename: &str) -> Result<&'static str, HrError> {
    let lower = filename.to_ascii_lowercase();
    if lower.ends_with(".xlsx") {
        Ok("xlsx")
    } else if lower.ends_with(".csv") {
        Ok("csv")
    } else {
        Err(HrError::validation(
            "attendance import accepts .xlsx workbooks or .csv files only",
        ))
    }
}

fn parse_employee_workbook(
    filename: &str,
    bytes: &[u8],
) -> Result<ParsedEmployeeWorkbook, HrError> {
    let mut workbook =
        Xlsx::new(Cursor::new(bytes)).map_err(|err| HrError::workbook(err.to_string()))?;
    let mut rows = Vec::new();
    for sheet in workbook.sheet_names().to_owned() {
        let range = workbook
            .worksheet_range(&sheet)
            .map_err(|err| HrError::workbook(err.to_string()))?;
        rows.extend(parse_employee_sheet(filename, &sheet, &range)?);
    }
    Ok(ParsedEmployeeWorkbook { rows })
}

fn parse_employee_sheet(
    filename: &str,
    sheet: &str,
    range: &calamine::Range<Data>,
) -> Result<Vec<ParsedEmployeeRow>, HrError> {
    let Some(header) = detect_employee_import_header(range, sheet)? else {
        return Ok(Vec::new());
    };

    let mut parsed = Vec::new();
    for (zero_based_idx, row) in range
        .rows()
        .enumerate()
        .skip(header.zero_based_row.saturating_add(1))
    {
        let Some(name) = row
            .get(header.name_index)
            .map(cell_text)
            .filter(|value| !value.is_empty())
        else {
            continue;
        };
        let source_row = i32::try_from(zero_based_idx + 1)
            .map_err(|_| HrError::workbook("source row does not fit i32"))?;
        let mut raw = Map::new();
        for (idx, header_label) in header.normalized_headers.iter().enumerate() {
            if header_label.is_empty() {
                continue;
            }
            let value = row.get(idx).map(cell_json).unwrap_or(Value::Null);
            raw.insert(header_label.clone(), value);
        }
        let source_key = format!("filename:{filename}|sheet:{sheet}|row:{source_row}");
        let raw_row = Value::Object(raw);
        let canonical = canonical_employee_fields_for_import(&raw_row, &header.columns);
        let mapped_company = raw_text_for_import_target(&raw_row, &header.columns, "company");
        let company_source = if mapped_company.is_some() {
            "mapped_column"
        } else {
            "sheet_name_default"
        };
        parsed.push(ParsedEmployeeRow {
            company: mapped_company.unwrap_or_else(|| sheet.to_owned()),
            name,
            source_filename: filename.to_owned(),
            source_sheet: sheet.to_owned(),
            source_row,
            source_key,
            raw_row,
            source_metadata: json!({
                "filename": filename,
                "sheet": sheet,
                "row": source_row,
                "source_key_kind": "filename_sheet_row",
                "header_row": header.zero_based_row + 1,
                "company_source": company_source
            }),
            canonical,
        });
    }
    Ok(parsed)
}

fn parse_employee_import_workbook(
    filename: &str,
    bytes: &[u8],
) -> Result<ParsedEmployeeImportWorkbook, HrError> {
    let mut workbook =
        Xlsx::new(Cursor::new(bytes)).map_err(|err| HrError::workbook(err.to_string()))?;
    let mut rows = Vec::new();
    let mut columns = BTreeMap::<String, EmployeeImportColumn>::new();
    for sheet in workbook.sheet_names().to_owned() {
        let range = workbook
            .worksheet_range(&sheet)
            .map_err(|err| HrError::workbook(err.to_string()))?;
        let parsed = parse_employee_import_sheet(filename, &sheet, &range)?;
        for column in parsed.columns {
            columns
                .entry(column.normalized_header.clone())
                .or_insert(column);
        }
        rows.extend(parsed.rows);
    }
    Ok(ParsedEmployeeImportWorkbook {
        rows,
        columns: columns.into_values().collect(),
    })
}

#[derive(Debug)]
struct ParsedEmployeeImportSheet {
    rows: Vec<ParsedEmployeeImportRow>,
    columns: Vec<EmployeeImportColumn>,
}

fn parse_employee_import_sheet(
    filename: &str,
    sheet: &str,
    range: &calamine::Range<Data>,
) -> Result<ParsedEmployeeImportSheet, HrError> {
    let Some(header) = detect_employee_import_header(range, sheet)? else {
        return Ok(ParsedEmployeeImportSheet {
            rows: Vec::new(),
            columns: Vec::new(),
        });
    };

    let mut parsed = Vec::new();
    for (zero_based_idx, row) in range
        .rows()
        .enumerate()
        .skip(header.zero_based_row.saturating_add(1))
    {
        if !row.iter().any(|cell| !cell_text(cell).is_empty()) {
            continue;
        }
        let source_row = i32::try_from(zero_based_idx + 1)
            .map_err(|_| HrError::workbook("source row does not fit i32"))?;
        let mut raw = Map::new();
        for (idx, header_label) in header.normalized_headers.iter().enumerate() {
            if header_label.is_empty() {
                continue;
            }
            let value = row.get(idx).map(cell_json).unwrap_or(Value::Null);
            raw.insert(header_label.clone(), value);
        }
        let raw_row = Value::Object(raw);
        let name = row
            .get(header.name_index)
            .map(cell_text)
            .filter(|value| !value.is_empty());
        let canonical = name
            .as_ref()
            .map(|_| canonical_employee_fields_for_import(&raw_row, &header.columns));
        let mapped_company = raw_text_for_import_target(&raw_row, &header.columns, "company");
        let company_source = if mapped_company.is_some() {
            "mapped_column"
        } else {
            "sheet_name_default"
        };
        let company = mapped_company.unwrap_or_else(|| sheet.to_owned());
        let source_key = format!("filename:{filename}|sheet:{sheet}|row:{source_row}");
        parsed.push(ParsedEmployeeImportRow {
            company,
            name,
            source_filename: filename.to_owned(),
            source_sheet: sheet.to_owned(),
            source_row,
            source_key,
            raw_row,
            source_metadata: json!({
                "filename": filename,
                "sheet": sheet,
                "row": source_row,
                "source_key_kind": "filename_sheet_row",
                "header_row": header.zero_based_row + 1,
                "header_normalization": "trim_remove_whitespace_for_storage_schema_catalog_for_matching",
                "company_source": company_source
            }),
            canonical,
            row_status: if row
                .get(header.name_index)
                .map(cell_text)
                .filter(|value| !value.is_empty())
                .is_some()
            {
                ImportRowStatus::Candidate
            } else {
                ImportRowStatus::Preserved
            },
        });
    }

    Ok(ParsedEmployeeImportSheet {
        rows: parsed,
        columns: header.columns,
    })
}

fn cell_text(cell: &Data) -> String {
    match cell {
        Data::String(value) | Data::DateTimeIso(value) | Data::DurationIso(value) => {
            value.trim().to_owned()
        }
        Data::Int(value) => value.to_string(),
        Data::Float(value) if value.fract() == 0.0 => format!("{value:.0}"),
        Data::Float(value) => value.to_string(),
        Data::Bool(value) => value.to_string(),
        Data::DateTime(value) => value.to_string(),
        Data::Error(value) => value.to_string(),
        Data::Empty => String::new(),
    }
}

fn cell_json(cell: &Data) -> Value {
    if cell.is_empty() {
        return Value::Null;
    }
    match cell {
        Data::Int(value) => json!(value),
        Data::Float(value) => json!(value),
        Data::Bool(value) => json!(value),
        Data::String(_)
        | Data::DateTimeIso(_)
        | Data::DurationIso(_)
        | Data::DateTime(_)
        | Data::Error(_) => {
            json!(cell_text(cell))
        }
        Data::Empty => Value::Null,
    }
}

fn normalize_header_label(raw: &str) -> String {
    raw.chars()
        .filter(|ch| !ch.is_whitespace())
        .collect::<String>()
}

#[derive(Debug)]
struct EmployeeImportHeader {
    zero_based_row: usize,
    normalized_headers: Vec<String>,
    columns: Vec<EmployeeImportColumn>,
    name_index: usize,
}

#[derive(Debug)]
struct EmployeeImportFieldDefinition {
    target: &'static str,
    aliases: &'static [&'static str],
    classification: &'static str,
    required: bool,
}

const EMPLOYEE_IMPORT_FIELD_DEFINITIONS: &[EmployeeImportFieldDefinition] = &[
    EmployeeImportFieldDefinition {
        target: "name",
        aliases: &[
            "성명",
            "이름",
            "직원명",
            "사원명",
            "근로자명",
            "임직원명",
            "name",
            "employee_name",
            "employeename",
            "worker_name",
            "workername",
        ],
        classification: "canonical",
        required: true,
    },
    EmployeeImportFieldDefinition {
        target: "employee_number",
        aliases: &[
            "사번",
            "사원번호",
            "직원번호",
            "사용자ID",
            "user_id",
            "userid",
            "employee_id",
            "employeeid",
            "employee_number",
            "employeenumber",
            "staff_id",
            "staffid",
        ],
        classification: "canonical",
        required: false,
    },
    EmployeeImportFieldDefinition {
        target: "org_unit",
        aliases: &[
            "부서명",
            "부서",
            "소속",
            "소속부서",
            "조직",
            "팀",
            "org_unit",
            "orgunit",
            "department",
            "dept",
            "team",
        ],
        classification: "canonical",
        required: false,
    },
    EmployeeImportFieldDefinition {
        target: "job",
        aliases: &[
            "업무",
            "담당업무",
            "직무",
            "직종",
            "job",
            "work_type",
            "worktype",
        ],
        classification: "canonical",
        required: false,
    },
    EmployeeImportFieldDefinition {
        target: "position",
        aliases: &["직책", "직위", "직급", "직함", "position", "title", "rank"],
        classification: "canonical",
        required: false,
    },
    EmployeeImportFieldDefinition {
        target: "worksite_name",
        aliases: &[
            "근무지",
            "현장",
            "현장명",
            "사업장",
            "근무사업장",
            "worksite",
            "workplace_name",
            "workplacename",
            "site",
        ],
        classification: "canonical",
        required: false,
    },
    EmployeeImportFieldDefinition {
        target: "worksite_address",
        aliases: &[
            "근무지(주소)",
            "근무지주소",
            "근무지주소지",
            "현장주소",
            "사업장주소",
            "주소",
            "worksite_address",
            "worksiteaddress",
            "workplace_address",
            "workplaceaddress",
            "site_address",
            "siteaddress",
        ],
        classification: "location",
        required: false,
    },
    EmployeeImportFieldDefinition {
        target: "hire_date",
        aliases: &[
            "입사일",
            "입사일자",
            "채용일",
            "보험가입일",
            "hire_date",
            "hiredate",
            "start_date",
            "startdate",
        ],
        classification: "canonical",
        required: false,
    },
    EmployeeImportFieldDefinition {
        target: "exit_date",
        aliases: &[
            "퇴사일",
            "퇴사일자",
            "퇴직일",
            "보험상실일",
            "exit_date",
            "exitdate",
            "end_date",
            "enddate",
            "termination_date",
            "terminationdate",
        ],
        classification: "canonical",
        required: false,
    },
    EmployeeImportFieldDefinition {
        target: "leave_accrued",
        aliases: &[
            "발생연차",
            "연차발생",
            "부여연차",
            "발생휴가",
            "leave_accrued",
            "leaveaccrued",
        ],
        classification: "canonical",
        required: false,
    },
    EmployeeImportFieldDefinition {
        target: "leave_used",
        aliases: &[
            "사용연차",
            "연차사용",
            "사용휴가",
            "leave_used",
            "leaveused",
        ],
        classification: "canonical",
        required: false,
    },
    EmployeeImportFieldDefinition {
        target: "leave_remaining",
        aliases: &[
            "잔여연차",
            "남은연차",
            "연차잔여",
            "잔여휴가",
            "leave_balance",
            "leavebalance",
            "leave_remaining",
            "leaveremaining",
        ],
        classification: "canonical",
        required: false,
    },
    EmployeeImportFieldDefinition {
        target: "company",
        aliases: &[
            "회사",
            "법인",
            "소속회사",
            "계열사",
            "조직회사",
            "company",
            "corporation",
            "legal_entity",
            "legalentity",
        ],
        classification: "canonical",
        required: false,
    },
];

fn detect_employee_import_header(
    range: &calamine::Range<Data>,
    sheet: &str,
) -> Result<Option<EmployeeImportHeader>, HrError> {
    if !range
        .rows()
        .any(|row| row.iter().any(|cell| !cell_text(cell).is_empty()))
    {
        return Ok(None);
    }

    for (zero_based_row, header_cells) in range.rows().enumerate().take(MAX_IMPORT_HEADER_SCAN_ROWS)
    {
        let source_headers = header_cells.iter().map(cell_text).collect::<Vec<_>>();
        let normalized_headers = source_headers
            .iter()
            .map(|header| normalize_header_label(header))
            .collect::<Vec<_>>();
        let column_targets = normalized_headers
            .iter()
            .map(|header| employee_import_target_for_header(header))
            .collect::<Vec<_>>();
        let Some(name_index) = column_targets
            .iter()
            .position(|target| *target == Some("name"))
        else {
            continue;
        };
        let columns = source_headers
            .iter()
            .zip(normalized_headers.iter())
            .filter(|(_, normalized)| !normalized.is_empty())
            .map(|(source, normalized)| employee_import_column(source, normalized))
            .collect::<Vec<_>>();
        return Ok(Some(EmployeeImportHeader {
            zero_based_row,
            normalized_headers,
            columns,
            name_index,
        }));
    }

    Err(HrError::workbook(format!(
        "sheet {sheet} is missing an employee name header"
    )))
}

fn employee_import_column(source_header: &str, normalized_header: &str) -> EmployeeImportColumn {
    let target = employee_import_target_for_header(normalized_header).map(ToOwned::to_owned);
    let classification =
        employee_import_column_classification(normalized_header, target.as_deref());
    EmployeeImportColumn {
        source_header: source_header.trim().to_owned(),
        normalized_header: normalized_header.to_owned(),
        target,
        preview_allowed: classification == "canonical" || classification == "retained",
        classification: classification.to_owned(),
    }
}

fn employee_import_target_for_header(header: &str) -> Option<&'static str> {
    employee_import_field_for_header(header).map(|field| field.target)
}

fn import_header_match_key(header: &str) -> String {
    normalize_header_label(header)
        .chars()
        .filter(|ch| {
            !matches!(
                ch,
                '(' | ')' | '[' | ']' | '{' | '}' | '_' | '-' | '/' | '\\' | '.' | '·' | ':'
            )
        })
        .collect::<String>()
        .to_ascii_lowercase()
}

fn employee_import_column_classification(header: &str, target: Option<&str>) -> &'static str {
    if let Some(field) = target.and_then(employee_import_field_for_target) {
        return field.classification;
    }
    if is_location_header(header) {
        return "location";
    }
    if is_restricted_employee_import_header(header) {
        return "restricted";
    }
    if target.is_some() {
        return "canonical";
    }
    "retained"
}

fn employee_import_field_for_header(
    header: &str,
) -> Option<&'static EmployeeImportFieldDefinition> {
    let key = import_header_match_key(header);
    EMPLOYEE_IMPORT_FIELD_DEFINITIONS.iter().find(|field| {
        field
            .aliases
            .iter()
            .any(|alias| import_header_match_key(alias) == key)
    })
}

fn employee_import_field_for_target(
    target: &str,
) -> Option<&'static EmployeeImportFieldDefinition> {
    EMPLOYEE_IMPORT_FIELD_DEFINITIONS
        .iter()
        .find(|field| field.target == target)
}

fn is_location_header(header: &str) -> bool {
    header.contains("위치") || header.contains("주소")
}

fn is_restricted_employee_import_header(header: &str) -> bool {
    let restricted_fragments = [
        "주민",
        "급여",
        "시급",
        "통상",
        "수당",
        "국민연금",
        "건강보험",
        "고용보험",
        "산재",
        "소득세",
        "은행",
        "계좌",
        "장애",
        "퇴직금",
        "지급일",
        "급여산정",
        "휴대폰",
        "전화",
        "연락처",
        "개인주소",
        "거주주소",
    ];
    restricted_fragments
        .iter()
        .any(|fragment| header.contains(fragment))
}

fn employee_import_mapping_profile(columns: &[EmployeeImportColumn]) -> Value {
    let target_allowlist = EMPLOYEE_IMPORT_FIELD_DEFINITIONS
        .iter()
        .map(|field| field.target)
        .collect::<Vec<_>>();
    let target_catalog = EMPLOYEE_IMPORT_FIELD_DEFINITIONS
        .iter()
        .map(|field| {
            json!({
                "target": field.target,
                "aliases": field.aliases,
                "classification": field.classification,
                "required": field.required
            })
        })
        .collect::<Vec<_>>();
    json!({
        "entity_type": "employee_hr",
        "target_allowlist": target_allowlist,
        "target_catalog": target_catalog,
        "columns": columns,
        "policy": {
            "unknown_columns": "retain_raw_only",
            "restricted_columns": "retain_raw_mask_preview",
            "blank_name_rows": "preserve_raw_only",
            "header_detection": {
                "strategy": "schema_catalog_first_mappable_row",
                "scan_rows": MAX_IMPORT_HEADER_SCAN_ROWS
            },
            "company_resolution": ["mapped_company_column", "sheet_name_default"],
            "server_side_entity_allowlist": ["employee_hr"]
        }
    })
}

fn masked_preview_values(
    raw_row: &Value,
    columns: &[EmployeeImportColumn],
) -> BTreeMap<String, Value> {
    let Some(object) = raw_row.as_object() else {
        return BTreeMap::new();
    };
    columns
        .iter()
        .filter_map(|column| {
            let value = object.get(&column.normalized_header)?;
            let masked = if column.preview_allowed {
                safe_preview_value(value)
            } else if value.is_null() {
                Value::Null
            } else {
                json!("••••")
            };
            Some((column.normalized_header.clone(), masked))
        })
        .collect()
}

fn safe_preview_value(value: &Value) -> Value {
    match value {
        Value::String(value) if value.len() > 80 => {
            json!(format!(
                "{}…",
                neutralize_spreadsheet_formula(&value.chars().take(80).collect::<String>())
            ))
        }
        Value::String(value) => json!(neutralize_spreadsheet_formula(value)),
        Value::Number(_) | Value::Bool(_) | Value::Null => value.clone(),
        Value::Array(_) | Value::Object(_) => json!("복합 값"),
    }
}

fn attendance_import_target_for_header(header: &str) -> Option<&'static str> {
    let key = import_header_match_key(header);
    ATTENDANCE_IMPORT_FIELD_DEFINITIONS
        .iter()
        .find(|field| {
            field
                .aliases
                .iter()
                .any(|alias| import_header_match_key(alias) == key)
        })
        .map(|field| field.target)
}

fn attendance_import_column(
    source_header: &str,
    normalized_header: &str,
) -> AttendanceImportColumn {
    let target = attendance_import_target_for_header(normalized_header).map(ToOwned::to_owned);
    let classification = if target.is_some() {
        "canonical"
    } else if is_restricted_attendance_import_header(normalized_header) {
        "restricted"
    } else {
        "retained"
    };
    AttendanceImportColumn {
        source_header: source_header.trim().to_owned(),
        normalized_header: normalized_header.to_owned(),
        target,
        classification: classification.to_owned(),
        preview_allowed: classification == "canonical",
    }
}

fn is_restricted_attendance_import_header(header: &str) -> bool {
    let restricted_fragments = [
        "주민",
        "급여",
        "시급",
        "수당",
        "보험",
        "소득세",
        "은행",
        "계좌",
        "전화",
        "연락처",
        "휴대폰",
        "주소",
        "개인",
    ];
    restricted_fragments
        .iter()
        .any(|fragment| header.contains(fragment))
}

fn attendance_import_columns_from_rows(
    rows: &[ParsedAttendanceImportRow],
) -> Vec<AttendanceImportColumn> {
    let mut columns = BTreeMap::<String, AttendanceImportColumn>::new();
    for row in rows {
        let Some(object) = row.raw_row.as_object() else {
            continue;
        };
        for key in object.keys() {
            columns
                .entry(key.clone())
                .or_insert_with(|| attendance_import_column(key, key));
        }
    }
    columns.into_values().collect()
}

fn attendance_import_mapping_profile(columns: &[AttendanceImportColumn]) -> Value {
    let target_allowlist = ATTENDANCE_IMPORT_FIELD_DEFINITIONS
        .iter()
        .map(|field| field.target)
        .collect::<Vec<_>>();
    let target_catalog = ATTENDANCE_IMPORT_FIELD_DEFINITIONS
        .iter()
        .map(|field| {
            json!({
                "target": field.target,
                "aliases": field.aliases,
                "required": field.required
            })
        })
        .collect::<Vec<_>>();
    json!({
        "entity_type": "attendance_direct",
        "target_allowlist": target_allowlist,
        "target_catalog": target_catalog,
        "columns": columns,
        "policy": {
            "unknown_columns": "retain_raw_only",
            "restricted_columns": "retain_raw_mask_preview",
            "header_detection": {
                "strategy": "attendance_schema_catalog_first_required_row",
                "scan_rows": MAX_IMPORT_HEADER_SCAN_ROWS
            },
            "employee_resolution": ["employee_number_unique", "employee_name_unique_or_ambiguous_row_error"],
            "server_side_entity_allowlist": ["attendance_direct"],
            "payroll_effect": "lineage_only_not_payable"
        }
    })
}

fn attendance_preview_values(
    raw_row: &Value,
    columns: &[AttendanceImportColumn],
) -> BTreeMap<String, Value> {
    let Some(object) = raw_row.as_object() else {
        return BTreeMap::new();
    };
    columns
        .iter()
        .filter_map(|column| {
            let value = object.get(&column.normalized_header)?;
            let masked = if column.preview_allowed {
                safe_preview_value(value)
            } else if value.is_null() {
                Value::Null
            } else {
                json!("••••")
            };
            Some((column.normalized_header.clone(), masked))
        })
        .collect()
}

fn raw_text_for_attendance_target(
    raw_row: &Value,
    columns: &[AttendanceImportColumn],
    target: &str,
) -> Option<String> {
    let object = raw_row.as_object()?;
    columns
        .iter()
        .filter(|column| column.target.as_deref() == Some(target))
        .find_map(|column| {
            let value = object.get(&column.normalized_header)?;
            json_value_text(value)
        })
}

fn attendance_canonical_row_json(row: &ParsedAttendanceImportRow, source_sha256: &str) -> Value {
    json!({
        "source_sheet": &row.source_sheet,
        "source_row": row.source_row,
        "source_key": &row.source_key,
        "source_sha256": source_sha256,
        "canonical": {
            "employee_number": &row.employee_number,
            "employee_name": &row.employee_name,
            "branch_name": &row.branch_name,
            "work_date": &row.work_date,
            "check_in_at": &row.check_in_at,
            "check_out_at": &row.check_out_at,
            "minutes_worked": row.minutes_worked
        }
    })
}

fn attendance_validation_json(row: &ParsedAttendanceImportRow) -> Value {
    match row.row_status {
        ImportRowStatus::Candidate => json!({ "status": "ok", "errors": [], "warnings": [] }),
        ImportRowStatus::Preserved => json!({
            "status": "preserved",
            "errors": [],
            "warnings": ["row_preserved_raw_only"]
        }),
        ImportRowStatus::Error => json!({
            "status": "error",
            "errors": &row.validation_errors,
            "warnings": []
        }),
    }
}

fn import_canonical_row_json(row: &ParsedEmployeeImportRow) -> Value {
    let Some(name) = row.name.as_ref() else {
        return json!({
            "source_filename": &row.source_filename,
            "source_sheet": &row.source_sheet,
            "source_row": row.source_row,
            "source_key": &row.source_key,
            "raw_only_reason": "missing_name"
        });
    };
    json!({
        "company": &row.company,
        "name": name,
        "source_filename": &row.source_filename,
        "source_sheet": &row.source_sheet,
        "source_row": row.source_row,
        "source_key": &row.source_key,
        "source_metadata": &row.source_metadata,
        "canonical": &row.canonical
    })
}

fn import_validation_json(row: &ParsedEmployeeImportRow) -> Value {
    match row.row_status {
        ImportRowStatus::Candidate => json!({ "status": "ok", "errors": [], "warnings": [] }),
        ImportRowStatus::Preserved => json!({
            "status": "preserved",
            "errors": [],
            "warnings": ["missing_name_preserved_raw_only"]
        }),
        ImportRowStatus::Error => json!({
            "status": "error",
            "errors": ["invalid_employee_import_row"],
            "warnings": []
        }),
    }
}

fn employee_import_batch_rows(rows: &[ParsedEmployeeRow]) -> Value {
    Value::Array(
        rows.iter()
            .map(|row| {
                let identity = employee_identity_resolution_from_metadata(&row.source_metadata);
                json!({
                    "company": &row.company,
                    "name": &row.name,
                    "source_filename": &row.source_filename,
                    "source_sheet": &row.source_sheet,
                    "source_row": row.source_row,
                    "source_key": &row.source_key,
                    "raw_row": &row.raw_row,
                    "source_metadata": &row.source_metadata,
                    "canonical": &row.canonical,
                    "identity": {
                        "strategy": identity.strategy,
                        "confidence": identity.confidence,
                        "review_required": identity.review_required
                    }
                })
            })
            .collect(),
    )
}

async fn apply_employee_import_batch(
    state: &HrState,
    org: OrgId,
    actor: UserId,
    run_id: Option<Uuid>,
    source_ref: String,
    rows: Vec<ParsedEmployeeRow>,
    apply_audit: Value,
) -> Result<EmployeeImportReport, HrError> {
    let command_store = state.leave_command_store.clone().ok_or_else(|| {
        HrError::unavailable(
            "leave command database is not configured; employee balance import is unavailable",
        )
    })?;
    let batch_rows = employee_import_batch_rows(&rows);
    let trace = TraceContext::generate();
    let result = console_platform_request_context::scope_org(org, async {
        command_store
            .apply_employee_import_batch(
                run_id,
                &source_ref,
                &batch_rows,
                actor,
                &apply_audit,
                trace,
            )
            .await
    })
    .await
    .map_err(HrError::from_leave_store)?;
    serde_json::from_value(result)
        .map_err(|error| HrError::internal(format!("invalid employee import report: {error}")))
}

fn require_leave_import_command_store(state: &HrState) -> Result<(), HrError> {
    state
        .leave_command_store
        .as_ref()
        .map(|_| ())
        .ok_or_else(|| {
            HrError::unavailable(
                "leave command database is not configured; employee balance import is unavailable",
            )
        })
}

/// Optional body for an ingest-commit ("적재") apply call. `checklist_all_acknowledged`
/// is the §16 self-checklist evidence the gate chain below requires (85 판정).
#[derive(Debug, Default, Deserialize)]
struct ImportApplyRequest {
    #[serde(default)]
    checklist_all_acknowledged: Option<bool>,
}

/// §16 ingest-commit gate (85 판정): apply ("적재") requires self-checklist
/// evidence — fail-closed deny, nothing written, if it's missing or false.
/// Authority is already enforced by the caller's `authorize_hr_org_wide` check;
/// four-eyes/egress are not part of this gate (ingest commit is single-actor).
/// ponytail: reuse the legacy authz check already in front of every apply
/// handler instead of re-deriving Authority through Cedar for this lane.
fn evaluate_ingest_checklist_gate(
    checklist_all_acknowledged: Option<bool>,
) -> Result<Value, HrError> {
    let outcome = evaluate_gate_chain(
        GateChainConfig {
            self_checklist: true,
            ..GateChainConfig::default()
        },
        &GateEvidence {
            checklist_all_acknowledged,
            ..GateEvidence::default()
        },
    );
    if !outcome.allow {
        return Err(HrError::from_kernel(KernelError::forbidden(
            "ingest commit requires acknowledging the self-checklist before applying",
        )));
    }
    Ok(json!(outcome))
}

async fn import_run_for_update(
    tx: &mut sqlx::Transaction<'_, Postgres>,
    org_uuid: Uuid,
    run_id: Uuid,
) -> Result<DataImportRunRecord, HrError> {
    let row = sqlx::query(
        r#"
        SELECT entity_type, status, source_sha256, input_rows, candidate_rows, preserved_rows
        FROM data_import_runs
        WHERE org_id = $1 AND id = $2
        FOR UPDATE
        "#,
    )
    .bind(org_uuid)
    .bind(run_id)
    .fetch_optional(tx.as_mut())
    .await?
    .ok_or_else(|| HrError::from_kernel(KernelError::not_found("import run not found")))?;

    Ok(DataImportRunRecord {
        entity_type: row.try_get("entity_type")?,
        status: row.try_get("status")?,
        source_sha256: row.try_get("source_sha256")?,
        input_rows: row.try_get("input_rows")?,
        candidate_rows: row.try_get("candidate_rows")?,
        preserved_rows: row.try_get("preserved_rows")?,
    })
}

async fn load_candidate_import_rows(
    tx: &mut sqlx::Transaction<'_, Postgres>,
    org_uuid: Uuid,
    run_id: Uuid,
) -> Result<Vec<StoredEmployeeImportRow>, HrError> {
    sqlx::query(
        r#"
        SELECT raw_row, canonical_row
        FROM data_import_rows
        WHERE org_id = $1 AND run_id = $2 AND row_status = 'CANDIDATE'
        ORDER BY source_sheet ASC, source_row ASC
        "#,
    )
    .bind(org_uuid)
    .bind(run_id)
    .fetch_all(tx.as_mut())
    .await?
    .into_iter()
    .map(|row| stored_employee_import_row(row.try_get("raw_row")?, row.try_get("canonical_row")?))
    .collect()
}

fn stored_employee_import_row(
    raw_row: Value,
    canonical_row: Value,
) -> Result<StoredEmployeeImportRow, HrError> {
    let source_metadata = canonical_row
        .get("source_metadata")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let canonical = serde_json::from_value::<EmployeeCanonicalFields>(
        canonical_row
            .get("canonical")
            .cloned()
            .unwrap_or_else(|| json!({ "employment_status": "ACTIVE" })),
    )
    .map_err(|err| HrError::validation(format!("stored import canonical row is invalid: {err}")))?;
    Ok(StoredEmployeeImportRow {
        company: required_json_string(&canonical_row, "company")?,
        name: required_json_string(&canonical_row, "name")?,
        source_filename: required_json_string(&canonical_row, "source_filename")?,
        source_sheet: required_json_string(&canonical_row, "source_sheet")?,
        source_row: required_json_i32(&canonical_row, "source_row")?,
        source_key: required_json_string(&canonical_row, "source_key")?,
        raw_row,
        source_metadata,
        canonical,
    })
}

fn required_json_string(value: &Value, key: &'static str) -> Result<String, HrError> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| HrError::validation(format!("stored import row is missing {key}")))
}

fn required_json_i32(value: &Value, key: &'static str) -> Result<i32, HrError> {
    let number = value
        .get(key)
        .and_then(Value::as_i64)
        .ok_or_else(|| HrError::validation(format!("stored import row is missing {key}")))?;
    i32::try_from(number)
        .map_err(|_| HrError::validation(format!("stored import row {key} does not fit i32")))
}

#[derive(Debug, Clone, Deserialize)]
struct AttendanceCanonicalFields {
    employee_number: Option<String>,
    employee_name: Option<String>,
    branch_name: Option<String>,
    work_date: Option<String>,
    check_in_at: Option<String>,
    check_out_at: Option<String>,
    minutes_worked: Option<i32>,
}

#[derive(Debug)]
struct StoredAttendanceImportRow {
    import_row_id: Uuid,
    source_sheet: String,
    source_row: i32,
    source_key: String,
    row_status: ImportRowStatus,
    canonical: AttendanceCanonicalFields,
    validation_errors: Vec<String>,
}

#[derive(Debug, Clone)]
struct ResolvedAttendanceImportRow {
    import_row_id: Uuid,
    employee_id: Uuid,
    branch_id: Uuid,
    source_sheet: String,
    source_row: i32,
    source_key: String,
    employee_number: Option<String>,
    employee_name: String,
    branch_name: String,
    work_date: String,
    check_in_at: Option<String>,
    check_out_at: Option<String>,
    minutes_worked: Option<i32>,
    fact_key: String,
}

struct AttendanceEmployeeResolution {
    id: Uuid,
    name: String,
    employee_number: Option<String>,
}

enum AttendanceEmployeeLookup {
    Matched(AttendanceEmployeeResolution),
    Missing,
    Ambiguous,
}

struct AttendanceBranchResolution {
    id: Uuid,
    name: String,
}

enum AttendanceBranchLookup {
    Matched(AttendanceBranchResolution),
    Missing,
    Ambiguous,
}

async fn load_attendance_import_rows(
    tx: &mut sqlx::Transaction<'_, Postgres>,
    org_uuid: Uuid,
    run_id: Uuid,
) -> Result<Vec<StoredAttendanceImportRow>, HrError> {
    sqlx::query(
        r#"
        SELECT id, source_sheet, source_row, source_key, row_status, canonical_row, validation
        FROM data_import_rows
        WHERE org_id = $1 AND run_id = $2
        ORDER BY source_sheet ASC, source_row ASC
        "#,
    )
    .bind(org_uuid)
    .bind(run_id)
    .fetch_all(tx.as_mut())
    .await?
    .into_iter()
    .map(stored_attendance_import_row)
    .collect()
}

fn stored_attendance_import_row(
    row: sqlx::postgres::PgRow,
) -> Result<StoredAttendanceImportRow, HrError> {
    let canonical_row: Value = row.try_get("canonical_row")?;
    let validation: Value = row.try_get("validation")?;
    let canonical = serde_json::from_value::<AttendanceCanonicalFields>(
        canonical_row
            .get("canonical")
            .cloned()
            .unwrap_or_else(|| json!({})),
    )
    .map_err(|err| {
        HrError::validation(format!(
            "stored attendance import canonical row is invalid: {err}"
        ))
    })?;
    let row_status_raw: String = row.try_get("row_status")?;
    let validation_errors = validation
        .get("errors")
        .and_then(Value::as_array)
        .map(|errors| {
            errors
                .iter()
                .filter_map(Value::as_str)
                .map(ToOwned::to_owned)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    Ok(StoredAttendanceImportRow {
        import_row_id: row.try_get("id")?,
        source_sheet: row.try_get("source_sheet")?,
        source_row: row.try_get("source_row")?,
        source_key: row.try_get("source_key")?,
        row_status: ImportRowStatus::from_db(&row_status_raw)?,
        canonical,
        validation_errors,
    })
}

fn ensure_attendance_import_run(
    run: &DataImportRunRecord,
    allowed_statuses: &[&str],
) -> Result<(), HrError> {
    if run.entity_type != "attendance_direct" {
        return Err(HrError::from_kernel(KernelError::conflict(
            "import run is not an attendance_direct run",
        )));
    }
    if !allowed_statuses.iter().any(|status| *status == run.status) {
        return Err(HrError::from_kernel(KernelError::conflict(format!(
            "attendance import run status {} is not allowed for this transition",
            run.status
        ))));
    }
    Ok(())
}

async fn resolve_attendance_import_rows(
    tx: &mut sqlx::Transaction<'_, Postgres>,
    org_uuid: Uuid,
    run_id: Uuid,
    run: &DataImportRunRecord,
    rows: &[StoredAttendanceImportRow],
) -> Result<AttendanceImportDryRunSummary, HrError> {
    let source_keys = rows
        .iter()
        .map(|row| row.source_key.clone())
        .collect::<Vec<_>>();
    let existing_keys = if source_keys.is_empty() {
        BTreeSet::new()
    } else {
        sqlx::query_scalar::<_, String>(
            r#"
            SELECT source_key
            FROM attendance_direct_import_events
            WHERE org_id = $1 AND source_sha256 = $2 AND source_key = ANY($3)
            "#,
        )
        .bind(org_uuid)
        .bind(&run.source_sha256)
        .bind(&source_keys)
        .fetch_all(tx.as_mut())
        .await?
        .into_iter()
        .collect::<BTreeSet<_>>()
    };

    let mut summary = AttendanceImportDryRunSummary {
        run_id,
        input_rows: usize::try_from(run.input_rows).unwrap_or_default(),
        candidate_rows: usize::try_from(run.candidate_rows).unwrap_or_default(),
        preserved_rows: usize::try_from(run.preserved_rows).unwrap_or_default(),
        ..AttendanceImportDryRunSummary::default()
    };

    let mut resolved_fact_keys = BTreeSet::<String>::new();

    for row in rows {
        if row.row_status != ImportRowStatus::Candidate {
            summary.error_rows += 1;
            if row.validation_errors.is_empty() {
                summary.row_errors.push(attendance_row_error(
                    row,
                    "invalid_row",
                    "attendance import row is not a candidate",
                ));
            } else {
                for code in &row.validation_errors {
                    summary
                        .row_errors
                        .push(attendance_row_error(row, code, code));
                }
            }
            continue;
        }

        if existing_keys.contains(&row.source_key) {
            summary.error_rows += 1;
            summary.duplicate_rows += 1;
            summary.row_errors.push(attendance_row_error(
                row,
                "duplicate_import_row",
                "attendance import row source key was already applied",
            ));
            continue;
        }

        let employee = match resolve_attendance_employee(tx, org_uuid, row).await? {
            AttendanceEmployeeLookup::Matched(employee) => employee,
            AttendanceEmployeeLookup::Missing => {
                summary.error_rows += 1;
                summary.missing_employee_rows += 1;
                summary.row_errors.push(attendance_row_error(
                    row,
                    "missing_employee",
                    "no employee matched the attendance row identifier",
                ));
                continue;
            }
            AttendanceEmployeeLookup::Ambiguous => {
                summary.error_rows += 1;
                summary.ambiguous_employee_rows += 1;
                summary.row_errors.push(attendance_row_error(
                    row,
                    "ambiguous_employee",
                    "attendance row identifier matched multiple employees",
                ));
                continue;
            }
        };
        let branch = match resolve_attendance_branch(tx, org_uuid, row).await? {
            AttendanceBranchLookup::Matched(branch) => branch,
            AttendanceBranchLookup::Missing => {
                summary.error_rows += 1;
                summary.row_errors.push(attendance_row_error(
                    row,
                    "missing_branch",
                    "no branch matched the attendance row branch name",
                ));
                continue;
            }
            AttendanceBranchLookup::Ambiguous => {
                summary.error_rows += 1;
                summary.row_errors.push(attendance_row_error(
                    row,
                    "ambiguous_branch",
                    "attendance row branch name matched multiple branches",
                ));
                continue;
            }
        };

        let employee_number = row
            .canonical
            .employee_number
            .clone()
            .or(employee.employee_number.clone());
        let Some(work_date) = row.canonical.work_date.clone() else {
            summary.error_rows += 1;
            summary.row_errors.push(attendance_row_error(
                row,
                "missing_work_date",
                "work_date is required",
            ));
            continue;
        };

        let fact_key = attendance_fact_key(
            employee.id,
            branch.id,
            &work_date,
            row.canonical.check_in_at.as_deref(),
            row.canonical.check_out_at.as_deref(),
            row.canonical.minutes_worked,
        );
        if !resolved_fact_keys.insert(fact_key.clone()) {
            summary.error_rows += 1;
            summary.duplicate_rows += 1;
            summary.row_errors.push(attendance_row_error(
                row,
                "duplicate_attendance_fact_in_file",
                "attendance fact is duplicated within this import run",
            ));
            continue;
        }
        if attendance_fact_exists(tx, org_uuid, &fact_key).await? {
            summary.error_rows += 1;
            summary.duplicate_rows += 1;
            summary.row_errors.push(attendance_row_error(
                row,
                "duplicate_attendance_fact",
                "attendance fact was already imported",
            ));
            continue;
        }

        summary
            .ready_rows_for_apply
            .push(ResolvedAttendanceImportRow {
                import_row_id: row.import_row_id,
                employee_id: employee.id,
                branch_id: branch.id,
                source_sheet: row.source_sheet.clone(),
                source_row: row.source_row,
                source_key: row.source_key.clone(),
                employee_number,
                employee_name: employee.name,
                branch_name: branch.name,
                work_date,
                check_in_at: row.canonical.check_in_at.clone(),
                check_out_at: row.canonical.check_out_at.clone(),
                minutes_worked: row.canonical.minutes_worked,
                fact_key,
            });
    }

    summary.ready_rows = summary.ready_rows_for_apply.len();
    Ok(summary)
}

async fn resolve_attendance_employee(
    tx: &mut sqlx::Transaction<'_, Postgres>,
    org_uuid: Uuid,
    row: &StoredAttendanceImportRow,
) -> Result<AttendanceEmployeeLookup, HrError> {
    let records = if let Some(employee_number) = row.canonical.employee_number.as_deref() {
        sqlx::query(
            r#"
            SELECT id, name, employee_number
            FROM employees
            WHERE org_id = $1 AND employee_number = $2
            ORDER BY id
            LIMIT 2
            "#,
        )
        .bind(org_uuid)
        .bind(employee_number)
        .fetch_all(tx.as_mut())
        .await?
    } else if let Some(employee_name) = row.canonical.employee_name.as_deref() {
        sqlx::query(
            r#"
            SELECT id, name, employee_number
            FROM employees
            WHERE org_id = $1 AND name = $2
            ORDER BY id
            LIMIT 2
            "#,
        )
        .bind(org_uuid)
        .bind(employee_name)
        .fetch_all(tx.as_mut())
        .await?
    } else {
        return Ok(AttendanceEmployeeLookup::Missing);
    };

    if records.is_empty() {
        return Ok(AttendanceEmployeeLookup::Missing);
    }
    if records.len() > 1 {
        return Ok(AttendanceEmployeeLookup::Ambiguous);
    }
    let Some(row) = records.into_iter().next() else {
        return Ok(AttendanceEmployeeLookup::Missing);
    };
    Ok(AttendanceEmployeeLookup::Matched(
        AttendanceEmployeeResolution {
            id: row.try_get("id")?,
            name: row.try_get("name")?,
            employee_number: row.try_get("employee_number")?,
        },
    ))
}

async fn resolve_attendance_branch(
    tx: &mut sqlx::Transaction<'_, Postgres>,
    org_uuid: Uuid,
    row: &StoredAttendanceImportRow,
) -> Result<AttendanceBranchLookup, HrError> {
    let Some(branch_name) = row.canonical.branch_name.as_deref() else {
        return Ok(AttendanceBranchLookup::Missing);
    };
    let records = sqlx::query(
        r#"
        SELECT id, name
        FROM branches
        WHERE org_id = $1 AND name = $2
        ORDER BY id
        LIMIT 2
        "#,
    )
    .bind(org_uuid)
    .bind(branch_name)
    .fetch_all(tx.as_mut())
    .await?;

    if records.is_empty() {
        return Ok(AttendanceBranchLookup::Missing);
    }
    if records.len() > 1 {
        return Ok(AttendanceBranchLookup::Ambiguous);
    }
    let Some(row) = records.into_iter().next() else {
        return Ok(AttendanceBranchLookup::Missing);
    };
    Ok(AttendanceBranchLookup::Matched(
        AttendanceBranchResolution {
            id: row.try_get("id")?,
            name: row.try_get("name")?,
        },
    ))
}

async fn attendance_fact_exists(
    tx: &mut sqlx::Transaction<'_, Postgres>,
    org_uuid: Uuid,
    fact_key: &str,
) -> Result<bool, HrError> {
    let exists: bool = sqlx::query_scalar(
        r#"
        SELECT EXISTS (
            SELECT 1
            FROM attendance_direct_import_events
            WHERE org_id = $1 AND fact_key = $2
        )
        "#,
    )
    .bind(org_uuid)
    .bind(fact_key)
    .fetch_one(tx.as_mut())
    .await?;
    Ok(exists)
}

fn attendance_fact_key(
    employee_id: Uuid,
    branch_id: Uuid,
    work_date: &str,
    check_in_at: Option<&str>,
    check_out_at: Option<&str>,
    minutes_worked: Option<i32>,
) -> String {
    format!(
        "employee:{employee_id}|branch:{branch_id}|date:{work_date}|in:{}|out:{}|minutes:{}",
        check_in_at.unwrap_or_default(),
        check_out_at.unwrap_or_default(),
        minutes_worked
            .map(|value| value.to_string())
            .unwrap_or_default()
    )
}

fn attendance_row_error(
    row: &StoredAttendanceImportRow,
    code: impl Into<String>,
    message: impl Into<String>,
) -> AttendanceImportRowError {
    AttendanceImportRowError {
        source_sheet: row.source_sheet.clone(),
        source_row: row.source_row,
        source_key: row.source_key.clone(),
        code: code.into(),
        message: message.into(),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct EmployeeIdentityResolution {
    strategy: String,
    confidence: String,
    review_required: bool,
    name_only_merge: bool,
}

fn employee_identity_resolution_from_metadata(metadata: &Value) -> EmployeeIdentityResolution {
    let identity = metadata
        .get("identity_resolution")
        .and_then(Value::as_object);
    let strategy = identity
        .and_then(|value| value.get("strategy"))
        .and_then(Value::as_str)
        .filter(|value| {
            matches!(
                *value,
                "employee_number"
                    | "legal_identifier_hash"
                    | "birth_hire_fingerprint"
                    | "source_row_fingerprint"
            )
        })
        .unwrap_or("source_row_fingerprint")
        .to_owned();
    let confidence = match strategy.as_str() {
        "employee_number" | "legal_identifier_hash" => "high",
        "birth_hire_fingerprint" => "medium",
        _ => "low",
    }
    .to_owned();
    let explicit_review_clearance = identity
        .and_then(|value| value.get("manual_review_required"))
        .and_then(Value::as_bool)
        == Some(false);
    let review_required = !(explicit_review_clearance
        && matches!(
            strategy.as_str(),
            "employee_number" | "legal_identifier_hash"
        ));

    EmployeeIdentityResolution {
        strategy,
        confidence,
        review_required,
        // Name-only merging is not a supported identity strategy. Even if stale
        // or malicious import metadata claims otherwise, keep the public record
        // non-mergeable until HR performs an explicit reviewed resolution.
        name_only_merge: false,
    }
}

async fn compute_employee_import_dry_run(
    tx: &mut sqlx::Transaction<'_, Postgres>,
    org_uuid: Uuid,
    run_id: Uuid,
    run: &DataImportRunRecord,
    rows: &[StoredEmployeeImportRow],
) -> Result<EmployeeImportDryRunSummary, HrError> {
    let source_keys = rows
        .iter()
        .map(|row| row.source_key.clone())
        .collect::<Vec<_>>();
    let existing = if source_keys.is_empty() {
        Vec::new()
    } else {
        sqlx::query_scalar::<_, String>(
            "SELECT source_key FROM employees WHERE org_id = $1 AND source_key = ANY($2)",
        )
        .bind(org_uuid)
        .bind(&source_keys)
        .fetch_all(tx.as_mut())
        .await?
    };
    let existing = existing
        .into_iter()
        .collect::<std::collections::BTreeSet<_>>();
    let mut by_company = BTreeMap::<String, CompanyImportSummary>::new();
    let mut insert_candidates = 0usize;
    let mut update_candidates = 0usize;
    for row in rows {
        let entry = by_company
            .entry(row.company.clone())
            .or_insert_with(|| CompanyImportSummary {
                company: row.company.clone(),
                ..CompanyImportSummary::default()
            });
        entry.input_rows += 1;
        if existing.contains(&row.source_key) {
            entry.updated += 1;
            update_candidates += 1;
        } else {
            entry.inserted += 1;
            insert_candidates += 1;
        }
    }
    Ok(EmployeeImportDryRunSummary {
        run_id,
        input_rows: usize::try_from(run.input_rows).unwrap_or_default(),
        candidate_rows: usize::try_from(run.candidate_rows).unwrap_or_default(),
        preserved_rows: usize::try_from(run.preserved_rows).unwrap_or_default(),
        insert_candidates,
        update_candidates,
        companies: by_company.into_values().collect(),
    })
}

pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

#[derive(Debug)]
struct NormalizedEmployeeLifecycleTransition {
    event_type: String,
    to_status: String,
    to_company: Option<String>,
    to_org_unit: Option<String>,
    to_position: Option<String>,
    effective_date: String,
    comment: String,
    signoffs: EmployeeLifecycleSignoffs,
}

fn normalize_lifecycle_transition(
    body: CreateEmployeeLifecycleEventRequest,
) -> Result<NormalizedEmployeeLifecycleTransition, HrError> {
    let event_type = normalize_enum_text(body.event_type);
    if !matches!(
        event_type.as_str(),
        "ONBOARD" | "OFFBOARD" | "TERMINATE" | "TRANSFER"
    ) {
        return Err(HrError::validation(
            "event_type must be ONBOARD, OFFBOARD, TERMINATE, or TRANSFER",
        ));
    }

    let to_status = body
        .to_status
        .map(normalize_enum_text)
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| default_lifecycle_status(&event_type).to_owned());
    if !matches!(to_status.as_str(), "ACTIVE" | "EXITED" | "UNKNOWN") {
        return Err(HrError::validation(
            "to_status must be ACTIVE, EXITED, or UNKNOWN",
        ));
    }

    let effective_date = body.effective_date.trim().to_owned();
    if effective_date.is_empty() {
        return Err(HrError::validation("effective_date is required"));
    }
    let comment = body.comment.trim().to_owned();
    if comment.is_empty() {
        return Err(HrError::validation("comment is required"));
    }

    Ok(NormalizedEmployeeLifecycleTransition {
        event_type,
        to_status,
        to_company: normalize_optional_text(body.to_company),
        to_org_unit: normalize_optional_text(body.to_org_unit),
        to_position: normalize_optional_text(body.to_position),
        effective_date,
        comment,
        signoffs: body.signoffs,
    })
}

fn default_lifecycle_status(event_type: &str) -> &'static str {
    match event_type {
        "OFFBOARD" | "TERMINATE" => "EXITED",
        "ONBOARD" | "TRANSFER" => "ACTIVE",
        _ => "UNKNOWN",
    }
}

#[derive(Debug, Clone)]
struct ExitCaseContext {
    id: Uuid,
    employee_id: Uuid,
    employee_name: String,
    employee_number: Option<String>,
    company: String,
    org_unit: Option<String>,
    position: Option<String>,
    worksite_name: Option<String>,
    hire_date: Option<String>,
    branch_id: Option<Uuid>,
    branch_name: Option<String>,
    status: String,
    /// Who recorded the first-tier (HR) confirmation, if any. Authoritative
    /// input to the two-tier separation-of-duties check in the confirm handler:
    /// the HQ confirmer must differ from this actor.
    hr_confirmed_by: Option<Uuid>,
    effective_exit_date: String,
    site_manager_note: String,
}

/// Materialize absence alerts from imported attendance facts, returning the ids
/// of the rows this pass actually INSERTed or UPDATEd. On unchanged imports the
/// `IS DISTINCT FROM` guard on the ON CONFLICT UPDATE (plus DO NOTHING on true
/// duplicates) means `RETURNING` yields nothing — so a repeated dashboard read
/// reports zero changed rows and emits no audit event (0093 review MEDIUM #5:
/// the caller audits only real mutations, preserving the write-storm guard).
async fn materialize_absence_alerts_from_imports(
    tx: &mut sqlx::Transaction<'_, Postgres>,
    org_uuid: Uuid,
    scope: &BranchScope,
) -> Result<Vec<Uuid>, HrError> {
    let mut builder = QueryBuilder::<Postgres>::new(
        r#"
        INSERT INTO employee_absence_alerts (
            org_id, employee_id, branch_id, work_date, source, severity, signal_payload
        )
        SELECT DISTINCT ON (a.employee_id, a.work_date::DATE)
            a.org_id,
            a.employee_id,
            a.branch_id,
            a.work_date::DATE,
            'attendance_direct_import',
            'WARNING',
            jsonb_build_object(
                'signal', 'NO_CLOCK_OR_ZERO_MINUTES',
                'source_sheet', a.source_sheet,
                'source_row', a.source_row,
                'source_key', a.source_key,
                'employee_name', a.employee_name,
                'branch_name', a.branch_name,
                'work_date', a.work_date,
                'message', 'Imported attendance row has no clock-in/out and zero worked minutes.'
            )
        FROM attendance_direct_import_events a
        JOIN employees e
          ON e.id = a.employee_id
         AND e.org_id = a.org_id
        WHERE a.org_id =
        "#,
    );
    builder.push_bind(org_uuid);
    builder.push(
        r#"
          AND e.employment_status = 'ACTIVE'
          AND a.check_in_at IS NULL
          AND a.check_out_at IS NULL
          AND COALESCE(a.minutes_worked, 0) = 0
          AND
        "#,
    );
    push_branch_scope_column(&mut builder, scope, "a.branch_id");
    builder.push(
        r#"
        ORDER BY a.employee_id, a.work_date::DATE, a.created_at DESC, a.id DESC
        ON CONFLICT (org_id, employee_id, work_date, source)
        DO UPDATE SET
            branch_id = EXCLUDED.branch_id,
            severity = EXCLUDED.severity,
            signal_payload = EXCLUDED.signal_payload,
            updated_at = now()
        WHERE employee_absence_alerts.status = 'OPEN'
          -- Write-storm bound (S6.3): this materializer runs on the dashboard GET
          -- path, so an unconditional DO UPDATE would rewrite every OPEN alert on
          -- every read (dead tuples + WAL per request). The IS DISTINCT FROM guard
          -- makes a repeated read over unchanged imports touch ZERO rows, so the
          -- per-request write set is bounded to alerts whose import facts actually
          -- changed, while still refreshing an alert when a re-import corrects it.
          AND (
            employee_absence_alerts.branch_id IS DISTINCT FROM EXCLUDED.branch_id
            OR employee_absence_alerts.severity IS DISTINCT FROM EXCLUDED.severity
            OR employee_absence_alerts.signal_payload IS DISTINCT FROM EXCLUDED.signal_payload
          )
        RETURNING id
        "#,
    );
    let changed_ids: Vec<Uuid> = builder.build_query_scalar().fetch_all(tx.as_mut()).await?;
    Ok(changed_ids)
}

async fn load_absence_exit_summary(
    tx: &mut sqlx::Transaction<'_, Postgres>,
    scope: &BranchScope,
) -> Result<AbsenceExitSummary, HrError> {
    Ok(AbsenceExitSummary {
        open_absence_alerts: count_absence_alerts(tx, scope, "a.status = 'OPEN'").await?,
        exit_cases_pending_hr: count_exit_cases(tx, scope, "c.status = 'REPORTED'").await?,
        settlement_needs_source: count_exit_packages(tx, scope, "p.status = 'NEEDS_SOURCE'")
            .await?,
        settlement_ready: count_exit_packages(tx, scope, "p.status = 'READY_FOR_APPROVAL'").await?,
        approval_drafts: count_exit_packages(tx, scope, "p.status = 'APPROVAL_DRAFTED'").await?,
        submitted: count_exit_cases(tx, scope, "c.status = 'SUBMITTED'").await?,
    })
}

async fn count_absence_alerts(
    tx: &mut sqlx::Transaction<'_, Postgres>,
    scope: &BranchScope,
    predicate: &'static str,
) -> Result<i64, HrError> {
    let mut builder = QueryBuilder::<Postgres>::new(
        "SELECT COUNT(*)::BIGINT FROM employee_absence_alerts a WHERE ",
    );
    builder.push(predicate);
    builder.push(" AND ");
    push_branch_scope_column(&mut builder, scope, "a.branch_id");
    Ok(builder.build_query_scalar().fetch_one(tx.as_mut()).await?)
}

async fn count_exit_cases(
    tx: &mut sqlx::Transaction<'_, Postgres>,
    scope: &BranchScope,
    predicate: &'static str,
) -> Result<i64, HrError> {
    let mut builder =
        QueryBuilder::<Postgres>::new("SELECT COUNT(*)::BIGINT FROM employee_exit_cases c WHERE ");
    builder.push(predicate);
    builder.push(" AND ");
    push_branch_scope_column(&mut builder, scope, "c.branch_id");
    Ok(builder.build_query_scalar().fetch_one(tx.as_mut()).await?)
}

async fn count_exit_packages(
    tx: &mut sqlx::Transaction<'_, Postgres>,
    scope: &BranchScope,
    predicate: &'static str,
) -> Result<i64, HrError> {
    let mut builder = QueryBuilder::<Postgres>::new(
        r#"
        SELECT COUNT(*)::BIGINT
        FROM employee_exit_settlement_packages p
        JOIN employee_exit_cases c
          ON c.id = p.exit_case_id
         AND c.org_id = p.org_id
        WHERE
        "#,
    );
    builder.push(predicate);
    builder.push(" AND ");
    push_branch_scope_column(&mut builder, scope, "c.branch_id");
    Ok(builder.build_query_scalar().fetch_one(tx.as_mut()).await?)
}

async fn load_absence_alerts(
    tx: &mut sqlx::Transaction<'_, Postgres>,
    scope: &BranchScope,
    employee_id: Option<Uuid>,
    limit: i64,
    offset: i64,
) -> Result<Vec<EmployeeAbsenceAlertResponse>, HrError> {
    let mut builder = QueryBuilder::<Postgres>::new(
        r#"
        SELECT
            a.id,
            a.employee_id,
            e.name AS employee_name,
            e.employee_number,
            e.company,
            e.org_unit,
            e.worksite_name,
            a.branch_id,
            b.name AS branch_name,
            a.work_date::TEXT AS work_date,
            a.source,
            a.status,
            a.severity,
            a.audience_roles,
            a.signal_payload,
            a.linked_exit_case_id AS exit_case_id,
            a.detected_at
        FROM employee_absence_alerts a
        JOIN employees e
          ON e.id = a.employee_id
         AND e.org_id = a.org_id
        LEFT JOIN branches b
          ON b.id = a.branch_id
         AND b.org_id = a.org_id
        WHERE
        "#,
    );
    push_branch_scope_column(&mut builder, scope, "a.branch_id");
    if let Some(employee_id) = employee_id {
        builder.push(" AND a.employee_id = ");
        builder.push_bind(employee_id);
    }
    builder.push(" ORDER BY a.work_date DESC, a.detected_at DESC, a.id DESC LIMIT ");
    builder.push_bind(limit);
    builder.push(" OFFSET ");
    builder.push_bind(offset);

    builder
        .build()
        .fetch_all(tx.as_mut())
        .await?
        .into_iter()
        .map(absence_alert_from_row)
        .collect::<Result<Vec<_>, HrError>>()
}

async fn load_exit_cases(
    tx: &mut sqlx::Transaction<'_, Postgres>,
    scope: &BranchScope,
    employee_id: Option<Uuid>,
    limit: i64,
    offset: i64,
) -> Result<Vec<EmployeeExitCaseResponse>, HrError> {
    let mut builder = QueryBuilder::<Postgres>::new(exit_case_select_sql());
    builder.push(" WHERE ");
    push_branch_scope_column(&mut builder, scope, "c.branch_id");
    if let Some(employee_id) = employee_id {
        builder.push(" AND c.employee_id = ");
        builder.push_bind(employee_id);
    }
    builder.push(" ORDER BY c.updated_at DESC, c.reported_at DESC, c.id DESC LIMIT ");
    builder.push_bind(limit);
    builder.push(" OFFSET ");
    builder.push_bind(offset);

    builder
        .build()
        .fetch_all(tx.as_mut())
        .await?
        .into_iter()
        .map(exit_case_from_row)
        .collect::<Result<Vec<_>, HrError>>()
}

async fn load_exit_case_by_id(
    tx: &mut sqlx::Transaction<'_, Postgres>,
    org_uuid: Uuid,
    case_id: Uuid,
) -> Result<EmployeeExitCaseResponse, HrError> {
    let mut builder = QueryBuilder::<Postgres>::new(exit_case_select_sql());
    builder.push(" WHERE c.org_id = ");
    builder.push_bind(org_uuid);
    builder.push(" AND c.id = ");
    builder.push_bind(case_id);

    let row = builder
        .build()
        .fetch_optional(tx.as_mut())
        .await?
        .ok_or_else(|| HrError::from_kernel(KernelError::not_found("exit case not found")))?;
    exit_case_from_row(row)
}

fn exit_case_select_sql() -> &'static str {
    r#"
    SELECT
        c.id,
        c.employee_id,
        e.name AS employee_name,
        e.employee_number,
        e.company,
        e.org_unit,
        e.worksite_name,
        c.branch_id,
        b.name AS branch_name,
        c.absence_alert_id,
        c.status,
        c.effective_exit_date::TEXT AS effective_exit_date,
        c.site_manager_note,
        c.reported_by,
        c.reported_at,
        c.hr_confirmed_by,
        c.hr_confirmed_at,
        c.hq_confirmed_by,
        c.hq_confirmed_at,
        c.approval_submitted_by,
        c.approval_submitted_at,
        p.id AS package_id,
        p.status AS package_status,
        p.service_days,
        p.average_wage_period_start::TEXT AS average_wage_period_start,
        p.average_wage_period_end::TEXT AS average_wage_period_end,
        p.average_wage_calendar_days,
        p.average_wage_total_won,
        p.average_daily_wage_milliwon,
        p.severance_pay_won,
        p.monthly_ordinary_wage_won,
        p.ordinary_daily_wage_won,
        p.statutory_daily_wage_milliwon,
        p.missing_source_fields,
        p.statutory_basis,
        p.insurance_loss_payload,
        p.approval_payload,
        p.certification_status,
        p.certified_package_digest,
        p.generated_at,
        p.submitted_by,
        p.submitted_at
    FROM employee_exit_cases c
    JOIN employees e
      ON e.id = c.employee_id
     AND e.org_id = c.org_id
    LEFT JOIN branches b
      ON b.id = c.branch_id
     AND b.org_id = c.org_id
    LEFT JOIN employee_exit_settlement_packages p
      ON p.exit_case_id = c.id
     AND p.org_id = c.org_id
    "#
}

async fn load_exit_case_context(
    tx: &mut sqlx::Transaction<'_, Postgres>,
    org_uuid: Uuid,
    case_id: Uuid,
    lock_case: bool,
) -> Result<ExitCaseContext, HrError> {
    let mut builder = QueryBuilder::<Postgres>::new(
        r#"
        SELECT
            c.id,
            c.employee_id,
            e.name AS employee_name,
            e.employee_number,
            e.company,
            e.org_unit,
            e.position,
            e.worksite_name,
            e.hire_date,
            c.branch_id,
            b.name AS branch_name,
            c.status,
            c.hr_confirmed_by,
            c.effective_exit_date::TEXT AS effective_exit_date,
            c.site_manager_note
        FROM employee_exit_cases c
        JOIN employees e
          ON e.id = c.employee_id
         AND e.org_id = c.org_id
        LEFT JOIN branches b
          ON b.id = c.branch_id
         AND b.org_id = c.org_id
        WHERE c.org_id =
        "#,
    );
    builder.push_bind(org_uuid);
    builder.push(" AND c.id = ");
    builder.push_bind(case_id);
    if lock_case {
        builder.push(" FOR UPDATE OF c");
    }

    let row = builder
        .build()
        .fetch_optional(tx.as_mut())
        .await?
        .ok_or_else(|| HrError::from_kernel(KernelError::not_found("exit case not found")))?;

    Ok(ExitCaseContext {
        id: row.try_get("id")?,
        employee_id: row.try_get("employee_id")?,
        employee_name: row.try_get("employee_name")?,
        employee_number: row.try_get("employee_number")?,
        company: row.try_get("company")?,
        org_unit: row.try_get("org_unit")?,
        position: row.try_get("position")?,
        worksite_name: row.try_get("worksite_name")?,
        hire_date: row.try_get("hire_date")?,
        branch_id: row.try_get("branch_id")?,
        branch_name: row.try_get("branch_name")?,
        status: row.try_get("status")?,
        hr_confirmed_by: row.try_get("hr_confirmed_by")?,
        effective_exit_date: row.try_get("effective_exit_date")?,
        site_manager_note: row.try_get("site_manager_note")?,
    })
}

async fn resolve_exit_case_branch(
    tx: &mut sqlx::Transaction<'_, Postgres>,
    org_uuid: Uuid,
    employee_id: Uuid,
    requested_branch_id: Option<Uuid>,
    absence_alert_id: Option<Uuid>,
) -> Result<Option<Uuid>, HrError> {
    // The employee row is durable authority; a client-supplied branch never is.
    // FOR SHARE holds the home-branch decision stable until the exit case lands.
    let employee_branch: Option<Option<Uuid>> = sqlx::query_scalar(
        "SELECT home_branch_id FROM employees WHERE org_id = $1 AND id = $2 FOR SHARE",
    )
    .bind(org_uuid)
    .bind(employee_id)
    .fetch_optional(tx.as_mut())
    .await?;
    let Some(employee_branch) = employee_branch else {
        return Err(HrError::from_kernel(KernelError::not_found(
            "employee not found",
        )));
    };

    // When an alert is named it must exist for this exact employee. Distinguish
    // a missing row from a real alert whose branch is NULL; silently falling
    // through would let a fabricated or stale alert id acquire authority.
    let alert_branch = if let Some(alert_id) = absence_alert_id {
        let row: Option<Option<Uuid>> = sqlx::query_scalar(
            r#"
            SELECT branch_id
            FROM employee_absence_alerts
            WHERE org_id = $1 AND id = $2 AND employee_id = $3
            FOR UPDATE
            "#,
        )
        .bind(org_uuid)
        .bind(alert_id)
        .bind(employee_id)
        .fetch_optional(tx.as_mut())
        .await?;
        let Some(branch_id) = row else {
            return Err(HrError::from_kernel(KernelError::not_found(
                "absence alert not found for employee",
            )));
        };
        branch_id
    } else {
        None
    };

    // An alert is the strongest evidence for an absence-driven report; otherwise
    // use the employee's durable home branch, then the latest imported attendance
    // fact. A missing branch stays missing and requires org-wide authorization.
    let derived_branch = if let Some(branch_id) = alert_branch.or(employee_branch) {
        Some(branch_id)
    } else {
        sqlx::query_scalar(
            r#"
            SELECT branch_id
            FROM attendance_direct_import_events
            WHERE org_id = $1 AND employee_id = $2
            ORDER BY work_date DESC, created_at DESC, id DESC
            LIMIT 1
            "#,
        )
        .bind(org_uuid)
        .bind(employee_id)
        .fetch_optional(tx.as_mut())
        .await?
    };

    if let Some(requested_branch_id) = requested_branch_id
        && derived_branch != Some(requested_branch_id)
    {
        return Err(HrError::from_kernel(KernelError::not_found(
            "employee or absence alert was not found in the requested branch",
        )));
    }
    Ok(derived_branch)
}

async fn insert_confirmed_exit_lifecycle_event(
    tx: &mut sqlx::Transaction<'_, Postgres>,
    org_uuid: Uuid,
    actor: UserId,
    context: &ExitCaseContext,
    note: Option<&str>,
) -> Result<(), HrError> {
    let current = load_employee_for_lifecycle(tx, org_uuid, context.employee_id).await?;
    if normalize_enum_text(current.employment_status.clone()) == "EXITED" {
        return Ok(());
    }
    let transition = NormalizedEmployeeLifecycleTransition {
        event_type: "TERMINATE".to_owned(),
        to_status: "EXITED".to_owned(),
        to_company: None,
        to_org_unit: None,
        to_position: None,
        effective_date: context.effective_exit_date.clone(),
        comment: note
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| format!("Exit confirmed from case {}", context.id)),
        signoffs: EmployeeLifecycleSignoffs {
            privacy_notice_ack: true,
            korean_labor_law_ack: true,
            payroll_cutoff_ack: true,
            retirement_settlement_ack: true,
        },
    };
    validate_lifecycle_transition(&current, &transition)?;

    let lifecycle_id = Uuid::new_v4();
    sqlx::query(
        r#"
        INSERT INTO employee_lifecycle_events (
            id, org_id, employee_id, event_type, from_status, to_status,
            from_company, to_company, from_org_unit, to_org_unit,
            from_position, to_position, effective_date, comment,
            signoffs, created_by
        )
        VALUES (
            $1, $2, $3, $4, $5, $6,
            $7, $8, $9, $10,
            $11, $12, $13, $14,
            $15, $16
        )
        "#,
    )
    .bind(lifecycle_id)
    .bind(org_uuid)
    .bind(context.employee_id)
    .bind(&transition.event_type)
    .bind(&current.employment_status)
    .bind(&transition.to_status)
    .bind(&current.company)
    .bind(&current.company)
    .bind(current.org_unit.as_deref())
    .bind(current.org_unit.as_deref())
    .bind(current.position.as_deref())
    .bind(current.position.as_deref())
    .bind(&transition.effective_date)
    .bind(&transition.comment)
    .bind(json!(&transition.signoffs))
    .bind(*actor.as_uuid())
    .execute(tx.as_mut())
    .await?;

    // The same Employment-owner statement the lifecycle handler calls: its
    // `exit_date = CASE WHEN $6 = 'EXITED' THEN $7 ELSE NULL` arm fires here
    // (EXITED stamps; ACTIVE|UNKNOWN clears), and carrying the employee's
    // CURRENT company/org-unit/position through leaves those three columns
    // exactly as they stood — which is what the dedicated
    // `SET employment_status = 'EXITED', exit_date = $3` statement did.
    employment::apply_employment_change(
        tx,
        org_uuid,
        context.employee_id,
        employment::EmploymentChange {
            company: &current.company,
            org_unit: current.org_unit.as_deref(),
            position: current.position.as_deref(),
            employment_status: &transition.to_status,
            effective_date: &context.effective_exit_date,
        },
    )
    .await?;
    Ok(())
}

async fn upsert_exit_settlement_package(
    tx: &mut sqlx::Transaction<'_, Postgres>,
    org_uuid: Uuid,
    case_id: Uuid,
    settlement_input: Option<ExitSettlementInput>,
) -> Result<Uuid, HrError> {
    let context = load_exit_case_context(tx, org_uuid, case_id, false).await?;
    let (
        package_status,
        service_days,
        period_start,
        period_end,
        calendar_days,
        total_won,
        daily_milliwon,
        severance_pay_won,
        missing_source_fields,
        monthly_ordinary_wage_won,
        ordinary_daily_wage_won,
        statutory_daily_wage_milliwon,
    ) = build_settlement_calculation(&context, settlement_input.as_ref())?;
    let statutory_basis = exit_statutory_basis();
    let insurance_loss_payload = with_ordinary_wage_basis(
        build_insurance_loss_payload(&context),
        monthly_ordinary_wage_won,
        ordinary_daily_wage_won,
        statutory_daily_wage_milliwon,
    );

    let package_id: Uuid = sqlx::query_scalar(
        r#"
        INSERT INTO employee_exit_settlement_packages (
            org_id, exit_case_id, employee_id, status, service_days,
            average_wage_period_start, average_wage_period_end,
            average_wage_calendar_days, average_wage_total_won,
            average_daily_wage_milliwon, severance_pay_won,
            missing_source_fields, statutory_basis, insurance_loss_payload,
            monthly_ordinary_wage_won, ordinary_daily_wage_won,
            statutory_daily_wage_milliwon,
            generated_at
        )
        VALUES (
            $1, $2, $3, $4, $5,
            $6::DATE, $7::DATE,
            $8, $9,
            $10, $11,
            $12, $13, $14,
            $15, $16,
            $17,
            now()
        )
        ON CONFLICT (org_id, exit_case_id)
        DO UPDATE SET
            status = EXCLUDED.status,
            service_days = EXCLUDED.service_days,
            average_wage_period_start = EXCLUDED.average_wage_period_start,
            average_wage_period_end = EXCLUDED.average_wage_period_end,
            average_wage_calendar_days = EXCLUDED.average_wage_calendar_days,
            average_wage_total_won = EXCLUDED.average_wage_total_won,
            average_daily_wage_milliwon = EXCLUDED.average_daily_wage_milliwon,
            severance_pay_won = EXCLUDED.severance_pay_won,
            missing_source_fields = EXCLUDED.missing_source_fields,
            statutory_basis = EXCLUDED.statutory_basis,
            insurance_loss_payload = EXCLUDED.insurance_loss_payload,
            monthly_ordinary_wage_won = EXCLUDED.monthly_ordinary_wage_won,
            ordinary_daily_wage_won = EXCLUDED.ordinary_daily_wage_won,
            statutory_daily_wage_milliwon = EXCLUDED.statutory_daily_wage_milliwon,
            -- Atomic re-uncertification (0093 HIGH #1): recomputing settlement fields
            -- invalidates any prior 노무사/세무사 certification. Reset here explicitly
            -- AND enforced for every write path by migration 0094's
            -- enforce_settlement_certification_reset() BEFORE UPDATE trigger. This
            -- path is never itself a certification action.
            certification_status = 'UNCERTIFIED_DRAFT',
            certification_artifact = NULL,
            certified_package_digest = NULL,
            generated_at = now(),
            updated_at = now()
        RETURNING id
        "#,
    )
    .bind(org_uuid)
    .bind(case_id)
    .bind(context.employee_id)
    .bind(package_status)
    .bind(service_days)
    .bind(period_start.as_deref())
    .bind(period_end.as_deref())
    .bind(calendar_days)
    .bind(total_won)
    .bind(daily_milliwon)
    .bind(severance_pay_won)
    .bind(missing_source_fields)
    .bind(statutory_basis)
    .bind(insurance_loss_payload)
    .bind(monthly_ordinary_wage_won)
    .bind(ordinary_daily_wage_won)
    .bind(statutory_daily_wage_milliwon)
    .fetch_one(tx.as_mut())
    .await?;

    if package_status == "READY_FOR_APPROVAL" {
        sqlx::query(
            r#"
            UPDATE employee_exit_cases
            SET status = CASE
                    WHEN status IN ('HR_CONFIRMED','HQ_CONFIRMED') THEN 'SETTLEMENT_READY'
                    ELSE status
                END,
                updated_at = now()
            WHERE org_id = $1 AND id = $2
            "#,
        )
        .bind(org_uuid)
        .bind(case_id)
        .execute(tx.as_mut())
        .await?;
    }

    Ok(package_id)
}

type SettlementCalculation = (
    &'static str,
    Option<i32>,
    Option<String>,
    Option<String>,
    Option<i32>,
    Option<i64>,
    Option<i64>,
    Option<i64>,
    Vec<String>,
    // 통상임금 (ordinary-wage) statutory basis (0093 review HIGH #2), all `None`
    // until the wage source arrives: monthly ordinary wage, derived ordinary
    // daily wage, and the daily wage that actually governed severance.
    Option<i64>,
    Option<i64>,
    Option<i64>,
);

/// 월 소정근로시간 (statutory monthly standard hours) for a 주40시간 worker.
const MONTHLY_STANDARD_HOURS: i64 = 209;
/// 1일 소정근로시간 (contractual daily hours).
const DAILY_CONTRACTUAL_HOURS: i64 = 8;

/// 통상일급 (1-day ordinary wage) from the monthly 통상임금 via the statutory
/// 기준시간 rule: 통상시급 = 월 통상임금 / 209h, 통상일급 = 통상시급 × 8h.
///
/// STANDARD-WORKER ASSUMPTION (explicit, v1 scope): 209h/month and 8h/day are the
/// 주40시간·1일 8시간 standard-worker convention (the official Minimum Wage
/// Commission tables publish hourly, 8-hour daily, and "monthly (209h basis)"
/// figures on exactly this basis). This derivation is valid ONLY for standard
/// 40h-week / 8h-day workers; non-standard schedules (shorter contractual hours,
/// shift patterns) need schedule-specific monthly standard hours and are OUT OF
/// v1 SCOPE.
///
/// 0093 review HIGH #3 (money bug): multiply BEFORE dividing —
/// `(monthly * 8) / 209`, not `(monthly / 209) * 8` — in checked i128 wide
/// arithmetic. Flooring `monthly / 209` first understated the daily figure by up
/// to 7 won/day, which under-paid severance whenever the ordinary floor governed.
fn ordinary_daily_wage_won_from_monthly(monthly_ordinary_wage_won: i64) -> Result<i64, HrError> {
    let daily = i128::from(monthly_ordinary_wage_won) * i128::from(DAILY_CONTRACTUAL_HOURS)
        / i128::from(MONTHLY_STANDARD_HOURS);
    i64::try_from(daily).map_err(|_| HrError::validation("ordinary daily wage overflow"))
}

/// Deterministic SHA-256 that binds a settlement package's CERTIFIED state to
/// the exact revision a 노무사/세무사 signed off (0093 HIGH: a non-null artifact
/// only proves an artifact exists, not that it certified *these* numbers).
///
/// Covers every certification-relevant field persisted on the row: the
/// severance figure, the statutory basis, the insurance-loss payload, the
/// approval payload, the wage-source-derived inputs (period bounds, calendar
/// days, wage total, average daily wage, service days), AND the 통상임금
/// (ordinary-wage) basis (monthly ordinary wage, derived ordinary daily wage,
/// and the statutory daily wage that governed). The ordinary-wage basis is bound
/// DIRECTLY (0093 review HIGH #2): binding it only transitively via
/// `severance_pay_won` was false, because the ordinary wage can change while the
/// average wage still governs, or two ordinary inputs can floor to the same
/// severance.
///
/// This covered-field set MUST stay byte-identical to the certification-covered
/// column set enforced by migration 0094's
/// `enforce_settlement_certification_reset()` BEFORE UPDATE trigger — the two are
/// cross-referenced so any covered write invalidates a stale certification.
///
/// Order-stable: this workspace builds `serde_json` without `preserve_order`, so
/// its object map is a `BTreeMap` that serializes with sorted keys, and embedded
/// JSONB values round-trip through the same `BTreeMap`. Identical inputs
/// therefore always hash identically regardless of column or JSON key order.
/// Recursively re-key every JSON object in sorted key order so a digest over the
/// result is canonical regardless of serde_json's `preserve_order` feature. A
/// transitive dependency (e.g. cedar-policy -> schemars) can enable
/// `serde_json/preserve_order` workspace-wide, flipping `Value` objects from a
/// sorted `BTreeMap` to an insertion-order `IndexMap`; a statutory certification
/// digest must never depend on such a global cargo feature. Sorting at every
/// level yields the same bytes serde_json produced by default (sorted) before any
/// such feature was present, so this does NOT change any already-stored digest.
fn canonical_json(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let sorted: BTreeMap<&String, &Value> = map.iter().collect();
            Value::Object(
                sorted
                    .into_iter()
                    .map(|(k, v)| (k.clone(), canonical_json(v)))
                    .collect(),
            )
        }
        Value::Array(items) => Value::Array(items.iter().map(canonical_json).collect()),
        scalar => scalar.clone(),
    }
}

#[allow(clippy::too_many_arguments)]
fn compute_certified_package_digest(
    severance_pay_won: Option<i64>,
    statutory_basis: &Value,
    insurance_loss_payload: &Value,
    approval_payload: &Value,
    average_wage_period_start: Option<&str>,
    average_wage_period_end: Option<&str>,
    average_wage_calendar_days: Option<i32>,
    average_wage_total_won: Option<i64>,
    average_daily_wage_milliwon: Option<i64>,
    service_days: Option<i32>,
    monthly_ordinary_wage_won: Option<i64>,
    ordinary_daily_wage_won: Option<i64>,
    statutory_daily_wage_milliwon: Option<i64>,
) -> String {
    let canonical = json!({
        "severance_pay_won": severance_pay_won,
        "statutory_basis": statutory_basis,
        "insurance_loss_payload": insurance_loss_payload,
        "approval_payload": approval_payload,
        "average_wage_period_start": average_wage_period_start,
        "average_wage_period_end": average_wage_period_end,
        "average_wage_calendar_days": average_wage_calendar_days,
        "average_wage_total_won": average_wage_total_won,
        "average_daily_wage_milliwon": average_daily_wage_milliwon,
        "service_days": service_days,
        "monthly_ordinary_wage_won": monthly_ordinary_wage_won,
        "ordinary_daily_wage_won": ordinary_daily_wage_won,
        "statutory_daily_wage_milliwon": statutory_daily_wage_milliwon,
    });
    // `canonical_json` sorts every object key recursively so the digest is
    // independent of serde_json's `preserve_order` feature (a transitive dep can
    // enable it workspace-wide). `Value::to_string()` is serde_json's infallible
    // `Display` serializer (same compact bytes as `serde_json::to_vec`), so the
    // money path carries no panic branch (also avoids the `clippy::expect_used`
    // deny without an allow).
    sha256_hex(canonical_json(&canonical).to_string().as_bytes())
}

/// Stamps the EFFECTIVE `certification_status` onto a generated payload
/// (insurance-loss or approval) so any surface that renders or exports the
/// payload directly can still derive the "산정 초안 — 노무사 검증 전" marker
/// (pre-mortem #4: the label must never be hand-placed per surface, only
/// derived from this single computed value). No-op if the payload isn't a
/// JSON object.
fn with_certification_status_marker(mut payload: Value, certification_status: &str) -> Value {
    if let Value::Object(map) = &mut payload {
        map.insert(
            "certification_status".to_owned(),
            Value::String(certification_status.to_owned()),
        );
    }
    payload
}

/// Embeds the 통상임금 (ordinary-wage) statutory basis into a generated payload
/// (insurance-loss or approval) so the filed MOEL/NHIS document itself records
/// which ordinary-wage input was compared against the average wage (0093 review
/// HIGH #2 — the money trail must be auditable in the document, not just on the
/// row). No-op unless the payload is a JSON object AND all three basis figures
/// are present (i.e. the package reached READY_FOR_APPROVAL).
fn with_ordinary_wage_basis(
    mut payload: Value,
    monthly_ordinary_wage_won: Option<i64>,
    ordinary_daily_wage_won: Option<i64>,
    statutory_daily_wage_milliwon: Option<i64>,
) -> Value {
    if let (Value::Object(map), Some(monthly), Some(daily), Some(statutory_daily)) = (
        &mut payload,
        monthly_ordinary_wage_won,
        ordinary_daily_wage_won,
        statutory_daily_wage_milliwon,
    ) {
        map.insert(
            "ordinary_wage_basis".to_owned(),
            json!({
                "monthly_ordinary_wage_won": monthly,
                "ordinary_daily_wage_won": daily,
                "statutory_daily_wage_milliwon": statutory_daily,
            }),
        );
    }
    payload
}

/// Serialize a `ProfessionalValidation` into the exact 4-key JSON artifact that
/// migration 0093's `..._cert_artifact_shape_chk` CHECK requires, emitting
/// `reviewer_kind` as exactly `LABOR_ATTORNEY` / `TAX_ACCOUNTANT`.
///
/// This is the write-side shape a future 노무사 certification-recording endpoint
/// persists. v1 ships no such endpoint, so `CERTIFIED` is unreachable-by-design
/// in production (safe — nothing gates on it yet, and the atomic reset in every
/// settlement UPDATE plus the digest-match honoring below hold the invariant the
/// moment a recording path is added). It is exercised by the certification tests.
// The certification tests that exercise it are `#[cfg(feature =
// "test-postgres")]`, so it is unused in a plain `cfg(test)` build too.
#[allow(dead_code)]
fn certification_artifact_json(validation: &ProfessionalValidation) -> Value {
    let reviewer_kind = match validation.reviewer_kind {
        ProfessionalReviewerKind::LaborAttorney => "LABOR_ATTORNEY",
        ProfessionalReviewerKind::TaxAccountant => "TAX_ACCOUNTANT",
    };
    json!({
        "reviewer_kind": reviewer_kind,
        "reviewed_on": validation.reviewed_on.to_string(),
        "artifact_sha256": validation.artifact_sha256,
        "reviewer_reference": validation.reviewer_reference,
    })
}

fn build_settlement_calculation(
    context: &ExitCaseContext,
    input: Option<&ExitSettlementInput>,
) -> Result<SettlementCalculation, HrError> {
    let mut missing = Vec::new();
    let Some(hire_date_text) = context.hire_date.as_deref() else {
        missing.push("hire_date".to_owned());
        missing.extend(missing_wage_source_fields());
        return Ok((
            "NEEDS_SOURCE",
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            missing,
            None,
            None,
            None,
        ));
    };
    let hire_date = parse_yyyy_mm_dd(hire_date_text)?;
    let exit_date = parse_yyyy_mm_dd(&context.effective_exit_date)?;
    let service_days = exit_date
        .to_julian_day()
        .saturating_sub(hire_date.to_julian_day())
        + 1;

    let Some(input) = input else {
        missing.extend(missing_wage_source_fields());
        return Ok((
            "NEEDS_SOURCE",
            Some(service_days),
            None,
            None,
            None,
            None,
            None,
            None,
            missing,
            None,
            None,
            None,
        ));
    };

    let period_start = parse_yyyy_mm_dd(&input.average_wage_period_start)?;
    let period_end = parse_yyyy_mm_dd(&input.average_wage_period_end)?;
    // 통상일급 (1-day ordinary wage) derived from the monthly 통상임금 via the
    // 기준시간 209h/8h rule (see `ordinary_daily_wage_won_from_monthly`). This
    // derivation policy lives in the app layer, not the payroll kernel (which
    // only enforces the max(average, ordinary) floor). Fail loud rather than
    // default so the depressed-window population is never under-calculated by
    // silently skipping the floor.
    if input.monthly_ordinary_wage_won <= 0 {
        return Err(HrError::validation(
            "monthly ordinary wage (월 통상임금) is required and must be positive for the 통상임금 floor",
        ));
    }
    let ordinary_daily_wage_won =
        ordinary_daily_wage_won_from_monthly(input.monthly_ordinary_wage_won)?;
    let draft = build_severance_pay_draft(SeverancePayInput {
        hire_date,
        exit_date,
        average_wage_period_start: period_start,
        average_wage_period_end: period_end,
        average_wage_calendar_days: input.average_wage_calendar_days,
        average_wage_total_won: input.average_wage_total_won,
        ordinary_daily_wage_won,
    })
    .map_err(HrError::from_kernel)?;

    let service_days = i32::try_from(draft.service_days)
        .map_err(|_| HrError::validation("service days overflow"))?;
    let calendar_days = i32::try_from(draft.average_wage_calendar_days)
        .map_err(|_| HrError::validation("average wage calendar days overflow"))?;
    Ok((
        "READY_FOR_APPROVAL",
        Some(service_days),
        Some(draft.average_wage_period_start.to_string()),
        Some(draft.average_wage_period_end.to_string()),
        Some(calendar_days),
        Some(draft.average_wage_total_won),
        Some(draft.average_daily_wage_milliwon),
        Some(draft.severance_pay_won),
        missing,
        // 통상임금 basis (0093 review HIGH #2): monthly input, derived daily, and
        // the daily wage that actually governed = max(average, ordinary).
        Some(input.monthly_ordinary_wage_won),
        Some(draft.ordinary_daily_wage_won),
        Some(draft.statutory_daily_wage_milliwon),
    ))
}

fn missing_wage_source_fields() -> Vec<String> {
    vec![
        "average_wage_period_start".to_owned(),
        "average_wage_period_end".to_owned(),
        "average_wage_calendar_days".to_owned(),
        "average_wage_total_won".to_owned(),
        "pre_exit_three_month_wage_sources".to_owned(),
    ]
}

fn exit_statutory_basis() -> Value {
    json!({
        "retirement_pay": {
            "authority": moel_retirement_pay_source().authority,
            "title": moel_retirement_pay_source().title,
            "url": moel_retirement_pay_source().url,
            "retrieved_on": moel_retirement_pay_source().retrieved_on.to_string(),
            "formula": "average_daily_wage * 30 * service_days / 365"
        },
        "insurance_loss": {
            "authority": nhis_qualification_loss_form_source().authority,
            "title": nhis_qualification_loss_form_source().title,
            "url": nhis_qualification_loss_form_source().url,
            "retrieved_on": nhis_qualification_loss_form_source().retrieved_on.to_string()
        }
    })
}

fn build_insurance_loss_payload(context: &ExitCaseContext) -> Value {
    json!({
        "employee": {
            "id": context.employee_id,
            "name": context.employee_name,
            "employee_number": context.employee_number,
            "company": context.company,
            "org_unit": context.org_unit,
            "position": context.position,
            "worksite_name": context.worksite_name
        },
        "exit": {
            "case_id": context.id,
            "effective_exit_date": context.effective_exit_date,
            "reported_reason": context.site_manager_note
        },
        "forms": [
            "national_pension_workplace_subscriber_loss",
            "health_insurance_workplace_subscriber_loss",
            "employment_insurance_insured_loss",
            "workers_compensation_insured_loss"
        ],
        "source_url": nhis_qualification_loss_form_source().url
    })
}

fn build_exit_approval_payload(context: &ExitCaseContext, note: Option<&str>) -> Value {
    json!({
        "document_type": "employee_exit_settlement",
        "title": format!("{} 퇴사 정산 및 4대보험 상실신고", context.employee_name),
        "target_date": context.effective_exit_date,
        "employee_id": context.employee_id,
        "company": context.company,
        "org_unit": context.org_unit,
        "branch_id": context.branch_id,
        "branch_name": context.branch_name,
        "requested_note": note,
        "approval_line": [
            "site_manager",
            "employee_hr_manager",
            "hq_hr_manager",
            "payroll_manager",
            "insurance_loss_reporter"
        ],
        "tracking": {
            "payroll_cutoff": true,
            "insurance_loss_report": true,
            "retirement_settlement": true
        },
        "href": format!("/approvals?source=employee-exit&focus={}", context.id)
    })
}

fn absence_alert_from_row(
    row: sqlx::postgres::PgRow,
) -> Result<EmployeeAbsenceAlertResponse, HrError> {
    let id: Uuid = row.try_get("id")?;
    let employee_id: Uuid = row.try_get("employee_id")?;
    let employee_name: String = row.try_get("employee_name")?;
    let work_date: String = row.try_get("work_date")?;
    let branch_name: Option<String> = row.try_get("branch_name")?;
    let audience_roles: Vec<String> = row.try_get("audience_roles")?;
    let notification_title = format!("결근 이상징후: {employee_name}");
    let notification_message = match branch_name.as_deref() {
        Some(branch) => {
            format!("{work_date} {branch} 근태자료에서 출퇴근 기록이 확인되지 않았습니다.")
        }
        None => format!("{work_date} 근태자료에서 출퇴근 기록이 확인되지 않았습니다."),
    };
    Ok(EmployeeAbsenceAlertResponse {
        id,
        employee_id,
        employee_name,
        employee_number: row.try_get("employee_number")?,
        company: row.try_get("company")?,
        org_unit: row.try_get("org_unit")?,
        worksite_name: row.try_get("worksite_name")?,
        branch_id: row.try_get("branch_id")?,
        branch_name,
        work_date,
        source: row.try_get("source")?,
        status: row.try_get("status")?,
        severity: row.try_get("severity")?,
        audience_roles,
        signal_payload: row.try_get("signal_payload")?,
        notification_title,
        notification_message,
        link_href: format!("/insurance-assist?employee={employee_id}&alert={id}"),
        exit_case_id: row.try_get("exit_case_id")?,
        detected_at: row.try_get("detected_at")?,
    })
}

fn exit_case_from_row(row: sqlx::postgres::PgRow) -> Result<EmployeeExitCaseResponse, HrError> {
    let package_id: Option<Uuid> = row.try_get("package_id")?;
    let settlement_package = if let Some(id) = package_id {
        let service_days: Option<i32> = row.try_get("service_days")?;
        let average_wage_period_start: Option<String> = row.try_get("average_wage_period_start")?;
        let average_wage_period_end: Option<String> = row.try_get("average_wage_period_end")?;
        let average_wage_calendar_days: Option<i32> = row.try_get("average_wage_calendar_days")?;
        let average_wage_total_won: Option<i64> = row.try_get("average_wage_total_won")?;
        let average_daily_wage_milliwon: Option<i64> =
            row.try_get("average_daily_wage_milliwon")?;
        let severance_pay_won: Option<i64> = row.try_get("severance_pay_won")?;
        let monthly_ordinary_wage_won: Option<i64> = row.try_get("monthly_ordinary_wage_won")?;
        let ordinary_daily_wage_won: Option<i64> = row.try_get("ordinary_daily_wage_won")?;
        let statutory_daily_wage_milliwon: Option<i64> =
            row.try_get("statutory_daily_wage_milliwon")?;
        let statutory_basis: Value = row.try_get("statutory_basis")?;
        let insurance_loss_payload: Value = row.try_get("insurance_loss_payload")?;
        let approval_payload: Value = row.try_get("approval_payload")?;
        let stored_certification_status: String = row.try_get("certification_status")?;
        let stored_digest: Option<String> = row.try_get("certified_package_digest")?;

        // Honor CERTIFIED only when the stored digest still binds the CURRENT
        // numbers. A stale digest (numbers recomputed after certification) or a
        // missing digest is reported as UNCERTIFIED_DRAFT — the code, not the DB
        // CHECK, is what proves the artifact certified *these* figures.
        let recomputed_digest = compute_certified_package_digest(
            severance_pay_won,
            &statutory_basis,
            &insurance_loss_payload,
            &approval_payload,
            average_wage_period_start.as_deref(),
            average_wage_period_end.as_deref(),
            average_wage_calendar_days,
            average_wage_total_won,
            average_daily_wage_milliwon,
            service_days,
            monthly_ordinary_wage_won,
            ordinary_daily_wage_won,
            statutory_daily_wage_milliwon,
        );
        let certification_status = if stored_certification_status == "CERTIFIED"
            && stored_digest.as_deref() == Some(recomputed_digest.as_str())
        {
            "CERTIFIED".to_owned()
        } else {
            "UNCERTIFIED_DRAFT".to_owned()
        };

        // 0093 MEDIUM (label plumbing is a backend deliverable): the generated
        // insurance-loss and approval payloads are the documents a human can
        // actually file with MOEL/NHIS, so the EFFECTIVE certification status
        // must ride along inside them too — not just the top-level DTO field —
        // or a surface that only forwards the raw payload could drop the
        // uncertified marker. Mutated AFTER the digest above is computed from
        // the untouched stored values, so this annotation never feeds back
        // into what a certification digest binds.
        let insurance_loss_payload =
            with_certification_status_marker(insurance_loss_payload, &certification_status);
        let approval_payload =
            with_certification_status_marker(approval_payload, &certification_status);

        Some(EmployeeExitSettlementPackageResponse {
            id,
            status: row.try_get("package_status")?,
            service_days,
            average_wage_period_start,
            average_wage_period_end,
            average_wage_calendar_days,
            average_wage_total_won,
            average_daily_wage_milliwon,
            severance_pay_won,
            monthly_ordinary_wage_won,
            ordinary_daily_wage_won,
            statutory_daily_wage_milliwon,
            missing_source_fields: row.try_get("missing_source_fields")?,
            statutory_basis,
            insurance_loss_payload,
            approval_payload,
            certification_status,
            generated_at: row.try_get("generated_at")?,
            submitted_by: row.try_get("submitted_by")?,
            submitted_at: row.try_get("submitted_at")?,
        })
    } else {
        None
    };
    let case_id: Uuid = row.try_get("id")?;
    let status: String = row.try_get("status")?;
    Ok(EmployeeExitCaseResponse {
        id: case_id,
        employee_id: row.try_get("employee_id")?,
        employee_name: row.try_get("employee_name")?,
        employee_number: row.try_get("employee_number")?,
        company: row.try_get("company")?,
        org_unit: row.try_get("org_unit")?,
        worksite_name: row.try_get("worksite_name")?,
        branch_id: row.try_get("branch_id")?,
        branch_name: row.try_get("branch_name")?,
        absence_alert_id: row.try_get("absence_alert_id")?,
        status: status.clone(),
        effective_exit_date: row.try_get("effective_exit_date")?,
        site_manager_note: row.try_get("site_manager_note")?,
        reported_by: row.try_get("reported_by")?,
        reported_at: row.try_get("reported_at")?,
        hr_confirmed_by: row.try_get("hr_confirmed_by")?,
        hr_confirmed_at: row.try_get("hr_confirmed_at")?,
        hq_confirmed_by: row.try_get("hq_confirmed_by")?,
        hq_confirmed_at: row.try_get("hq_confirmed_at")?,
        approval_submitted_by: row.try_get("approval_submitted_by")?,
        approval_submitted_at: row.try_get("approval_submitted_at")?,
        settlement_package,
        next_actions: exit_case_next_actions(case_id, &status),
    })
}

fn exit_case_next_actions(case_id: Uuid, status: &str) -> Vec<ExitCaseNextAction> {
    let mut actions = Vec::new();
    if status == "REPORTED" {
        actions.push(ExitCaseNextAction {
            key: "confirm_exit".to_owned(),
            label: "퇴사 확인/승인".to_owned(),
            href: format!("/insurance-assist?exitCase={case_id}"),
        });
    }
    if matches!(
        status,
        "HR_CONFIRMED" | "HQ_CONFIRMED" | "SETTLEMENT_READY" | "APPROVAL_DRAFTED"
    ) {
        actions.push(ExitCaseNextAction {
            key: "prepare_settlement".to_owned(),
            label: "4대보험/퇴직금 결제상신".to_owned(),
            href: format!("/payroll?exitCase={case_id}"),
        });
    }
    actions
}

fn validate_lifecycle_transition(
    current: &EmployeeForLifecycle,
    transition: &NormalizedEmployeeLifecycleTransition,
) -> Result<(), HrError> {
    if !transition.signoffs.privacy_notice_ack {
        return Err(HrError::validation(
            "privacy_notice_ack is required for employee lifecycle events",
        ));
    }
    if !transition.signoffs.korean_labor_law_ack {
        return Err(HrError::validation(
            "korean_labor_law_ack is required for employee lifecycle events",
        ));
    }

    let current_status = normalize_enum_text(current.employment_status.clone());
    match transition.event_type.as_str() {
        "ONBOARD" => {
            if current_status == "EXITED" {
                return Err(HrError::from_kernel(KernelError::invalid_transition(
                    "employee is already EXITED and cannot be onboarded",
                )));
            }
            if transition.to_status != "ACTIVE" {
                return Err(HrError::from_kernel(KernelError::invalid_transition(
                    "ONBOARD must result in ACTIVE status",
                )));
            }
        }
        "OFFBOARD" | "TERMINATE" => {
            if current_status == "EXITED" {
                return Err(HrError::from_kernel(KernelError::invalid_transition(
                    "employee is already EXITED",
                )));
            }
            if transition.to_status != "EXITED" {
                return Err(HrError::from_kernel(KernelError::invalid_transition(
                    "OFFBOARD and TERMINATE must result in EXITED status",
                )));
            }
            if !transition.signoffs.payroll_cutoff_ack {
                return Err(HrError::validation(
                    "payroll_cutoff_ack is required before offboarding or termination",
                ));
            }
            if !transition.signoffs.retirement_settlement_ack {
                return Err(HrError::validation(
                    "retirement_settlement_ack is required before offboarding or termination",
                ));
            }
        }
        "TRANSFER" => {
            if current_status == "EXITED" {
                return Err(HrError::from_kernel(KernelError::invalid_transition(
                    "employee is already EXITED and cannot be transferred",
                )));
            }
            if transition.to_status != "ACTIVE" {
                return Err(HrError::from_kernel(KernelError::invalid_transition(
                    "TRANSFER must keep the employee ACTIVE",
                )));
            }
            let cross_company = transition
                .to_company
                .as_ref()
                .is_some_and(|next| next != &current.company);
            if cross_company
                && (!transition.signoffs.payroll_cutoff_ack
                    || !transition.signoffs.retirement_settlement_ack)
            {
                return Err(HrError::validation(
                    "cross-company transfer requires payroll cutoff and retirement-settlement signoffs",
                ));
            }
        }
        _ => {}
    }
    Ok(())
}

async fn load_employee_for_lifecycle(
    tx: &mut sqlx::Transaction<'_, Postgres>,
    org_uuid: Uuid,
    employee_id: Uuid,
) -> Result<EmployeeForLifecycle, HrError> {
    let row = sqlx::query(
        r#"
        SELECT company, org_unit, position, employment_status
        FROM employees
        WHERE org_id = $1 AND id = $2
        FOR UPDATE
        "#,
    )
    .bind(org_uuid)
    .bind(employee_id)
    .fetch_optional(tx.as_mut())
    .await?
    .ok_or_else(|| HrError::from_kernel(KernelError::not_found("employee not found")))?;

    Ok(EmployeeForLifecycle {
        company: row.try_get("company")?,
        org_unit: row.try_get("org_unit")?,
        position: row.try_get("position")?,
        employment_status: row.try_get("employment_status")?,
    })
}
async fn load_linked_employee_for_user(
    tx: &mut sqlx::Transaction<'_, Postgres>,
    org: OrgId,
    user_id: UserId,
    lock_user: bool,
) -> Result<LinkedEmployee, HrError> {
    let sql = if lock_user {
        r#"
        SELECT u.employee_id, e.name AS employee_name
        FROM users u
        LEFT JOIN employees e
          ON e.id = u.employee_id
         AND e.org_id = u.org_id
        WHERE u.id = $1
          AND u.org_id = $2
        FOR UPDATE OF u
        "#
    } else {
        r#"
        SELECT u.employee_id, e.name AS employee_name
        FROM users u
        LEFT JOIN employees e
          ON e.id = u.employee_id
         AND e.org_id = u.org_id
        WHERE u.id = $1
          AND u.org_id = $2
        "#
    };

    let row = sqlx::query(sql)
        .bind(*user_id.as_uuid())
        .bind(*org.as_uuid())
        .fetch_optional(tx.as_mut())
        .await?
        .ok_or_else(|| {
            HrError::from_kernel(KernelError::forbidden("linked employee account required"))
        })?;

    let employee_id = row
        .try_get::<Option<Uuid>, _>("employee_id")?
        .ok_or_else(|| {
            HrError::from_kernel(KernelError::forbidden("linked employee account required"))
        })?;
    let display_name = row
        .try_get::<Option<String>, _>("employee_name")?
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            HrError::from_kernel(KernelError::forbidden("linked employee record required"))
        })?;

    Ok(LinkedEmployee {
        employee_id,
        display_name,
    })
}

/// Employee id linked to `user_id`, or `None` when the account has no employee
/// link. Self-service *reads* use this so an unlinked user (an ADMIN/system
/// account) sees an empty page instead of the 403 `load_linked_employee_for_user`
/// raises for the write path.
async fn load_optional_linked_employee_id(
    tx: &mut sqlx::Transaction<'_, Postgres>,
    org: OrgId,
    user_id: UserId,
) -> Result<Option<Uuid>, HrError> {
    let employee_id: Option<Uuid> =
        sqlx::query_scalar("SELECT employee_id FROM users WHERE id = $1 AND org_id = $2")
            .bind(*user_id.as_uuid())
            .bind(*org.as_uuid())
            .fetch_optional(tx.as_mut())
            .await?
            .flatten();
    Ok(employee_id)
}

async fn list_attendance_records_for_employee(
    tx: &mut sqlx::Transaction<'_, Postgres>,
    employee_id: Uuid,
    branch_id: Option<Uuid>,
    limit: i64,
    offset: i64,
) -> Result<EmployeeAttendanceRecordPage, HrError> {
    let mut total_query = QueryBuilder::<Postgres>::new(
        "SELECT COUNT(*)::BIGINT FROM employee_attendance_records r JOIN employees e ON e.id = r.employee_id AND e.org_id = r.org_id WHERE r.employee_id = ",
    );
    total_query.push_bind(employee_id);
    if let Some(branch_id) = branch_id {
        total_query.push(" AND e.home_branch_id = ");
        total_query.push_bind(branch_id);
    }
    let total: i64 = total_query
        .build_query_scalar()
        .fetch_one(tx.as_mut())
        .await?;

    let mut rows_query = QueryBuilder::<Postgres>::new(
        r#"
        SELECT
            r.id,
            r.employee_id,
            e.name AS employee_display_name,
            r.kind,
            r.occurred_at,
            r.work_date::TEXT AS work_date,
            r.state_after,
            r.note,
            pmr.id AS payroll_material_ref_id
        FROM employee_attendance_records r
        JOIN employees e
          ON e.id = r.employee_id
         AND e.org_id = r.org_id
        JOIN payroll_attendance_material_refs pmr
          ON pmr.attendance_record_id = r.id
         AND pmr.org_id = r.org_id
        WHERE r.employee_id =
        "#,
    );
    rows_query.push_bind(employee_id);
    if let Some(branch_id) = branch_id {
        rows_query.push(" AND e.home_branch_id = ");
        rows_query.push_bind(branch_id);
    }
    rows_query.push(" ORDER BY r.occurred_at DESC, r.created_at DESC, r.id DESC LIMIT ");
    rows_query.push_bind(limit);
    rows_query.push(" OFFSET ");
    rows_query.push_bind(offset);
    let rows = rows_query.build().fetch_all(tx.as_mut()).await?;

    let items = rows
        .into_iter()
        .map(|row| employee_attendance_record_from_joined_row(row, false))
        .collect::<Result<Vec<_>, _>>()?;

    Ok(EmployeeAttendanceRecordPage {
        items,
        total,
        limit,
        offset,
    })
}

async fn list_attendance_records_for_org(
    tx: &mut sqlx::Transaction<'_, Postgres>,
    branch_id: Option<Uuid>,
    limit: i64,
    offset: i64,
) -> Result<EmployeeAttendanceRecordPage, HrError> {
    let mut total_query = QueryBuilder::<Postgres>::new(
        "SELECT COUNT(*)::BIGINT FROM employee_attendance_records r JOIN employees e ON e.id = r.employee_id AND e.org_id = r.org_id WHERE TRUE",
    );
    if let Some(branch_id) = branch_id {
        total_query.push(" AND e.home_branch_id = ");
        total_query.push_bind(branch_id);
    }
    let total: i64 = total_query
        .build_query_scalar()
        .fetch_one(tx.as_mut())
        .await?;

    let mut rows_query = QueryBuilder::<Postgres>::new(
        r#"
        SELECT
            r.id,
            r.employee_id,
            e.name AS employee_display_name,
            r.kind,
            r.occurred_at,
            r.work_date::TEXT AS work_date,
            r.state_after,
            r.note,
            pmr.id AS payroll_material_ref_id
        FROM employee_attendance_records r
        JOIN employees e
          ON e.id = r.employee_id
         AND e.org_id = r.org_id
        JOIN payroll_attendance_material_refs pmr
          ON pmr.attendance_record_id = r.id
         AND pmr.org_id = r.org_id
        WHERE TRUE
        "#,
    );
    if let Some(branch_id) = branch_id {
        rows_query.push(" AND e.home_branch_id = ");
        rows_query.push_bind(branch_id);
    }
    rows_query.push(" ORDER BY r.occurred_at DESC, r.created_at DESC, r.id DESC LIMIT ");
    rows_query.push_bind(limit);
    rows_query.push(" OFFSET ");
    rows_query.push_bind(offset);
    let rows = rows_query.build().fetch_all(tx.as_mut()).await?;

    let items = rows
        .into_iter()
        .map(|row| employee_attendance_record_from_joined_row(row, false))
        .collect::<Result<Vec<_>, _>>()?;

    Ok(EmployeeAttendanceRecordPage {
        items,
        total,
        limit,
        offset,
    })
}

async fn load_attendance_record_by_idempotency_key(
    tx: &mut sqlx::Transaction<'_, Postgres>,
    employee_id: Uuid,
    idempotency_key: &str,
) -> Result<Option<EmployeeAttendanceRecordResponse>, HrError> {
    let row = sqlx::query(
        r#"
        SELECT
            r.id,
            r.employee_id,
            e.name AS employee_display_name,
            r.kind,
            r.occurred_at,
            r.work_date::TEXT AS work_date,
            r.state_after,
            r.note,
            pmr.id AS payroll_material_ref_id
        FROM employee_attendance_records r
        JOIN employees e
          ON e.id = r.employee_id
         AND e.org_id = r.org_id
        JOIN payroll_attendance_material_refs pmr
          ON pmr.attendance_record_id = r.id
         AND pmr.org_id = r.org_id
        WHERE r.employee_id = $1
          AND r.idempotency_key = $2
        "#,
    )
    .bind(employee_id)
    .bind(idempotency_key)
    .fetch_optional(tx.as_mut())
    .await?;

    row.map(|row| employee_attendance_record_from_joined_row(row, true))
        .transpose()
}

fn employee_attendance_record_from_joined_row(
    row: sqlx::postgres::PgRow,
    duplicate: bool,
) -> Result<EmployeeAttendanceRecordResponse, HrError> {
    Ok(EmployeeAttendanceRecordResponse {
        id: row.try_get("id")?,
        employee_id: row.try_get("employee_id")?,
        employee_display_name: row.try_get("employee_display_name")?,
        kind: row.try_get("kind")?,
        occurred_at: row.try_get("occurred_at")?,
        work_date: row.try_get("work_date")?,
        state_after: row.try_get("state_after")?,
        note: row.try_get("note")?,
        payroll_material_ref_id: row.try_get("payroll_material_ref_id")?,
        payroll_link_status: "LINKED".to_owned(),
        duplicate,
    })
}

fn employee_attendance_record_from_parts(
    row: sqlx::postgres::PgRow,
    employee_display_name: String,
    payroll_material_ref_id: Uuid,
    duplicate: bool,
) -> Result<EmployeeAttendanceRecordResponse, HrError> {
    Ok(EmployeeAttendanceRecordResponse {
        id: row.try_get("id")?,
        employee_id: row.try_get("employee_id")?,
        employee_display_name,
        kind: row.try_get("kind")?,
        occurred_at: row.try_get("occurred_at")?,
        work_date: row.try_get("work_date")?,
        state_after: row.try_get("state_after")?,
        note: row.try_get("note")?,
        payroll_material_ref_id,
        payroll_link_status: "LINKED".to_owned(),
        duplicate,
    })
}

fn normalize_attendance_kind(raw: &str) -> Result<&'static str, HrError> {
    match raw.trim().to_ascii_uppercase().as_str() {
        "CLOCK_IN" => Ok("CLOCK_IN"),
        "OUT_FOR_WORK" => Ok("OUT_FOR_WORK"),
        "BUSINESS_TRIP" => Ok("BUSINESS_TRIP"),
        "RETURNED" => Ok("RETURNED"),
        "CLOCK_OUT" => Ok("CLOCK_OUT"),
        _ => Err(HrError::validation("unsupported attendance record kind")),
    }
}

fn normalize_idempotency_key(value: String) -> Result<String, HrError> {
    let value = normalize_optional_text(Some(value))
        .ok_or_else(|| HrError::validation("idempotency key is required"))?;
    if value.chars().count() > 128 {
        return Err(HrError::validation(
            "idempotency key must be 128 characters or fewer",
        ));
    }
    Ok(value)
}

fn normalize_attendance_note(value: Option<String>) -> Result<Option<String>, HrError> {
    match normalize_optional_text(value) {
        Some(value) if value.chars().count() > 500 => Err(HrError::validation(
            "attendance note must be 500 characters or fewer",
        )),
        value => Ok(value),
    }
}

fn next_employee_attendance_state(
    previous_state: Option<&str>,
    kind: &str,
) -> Result<&'static str, HrError> {
    match (previous_state.unwrap_or("OFF_DUTY"), kind) {
        ("OFF_DUTY", "CLOCK_IN") => Ok("CLOCKED_IN"),
        ("CLOCKED_IN", "OUT_FOR_WORK") => Ok("OUT_FOR_WORK"),
        ("CLOCKED_IN", "BUSINESS_TRIP") => Ok("BUSINESS_TRIP"),
        ("CLOCKED_IN", "CLOCK_OUT") => Ok("OFF_DUTY"),
        ("OUT_FOR_WORK", "RETURNED") | ("BUSINESS_TRIP", "RETURNED") => Ok("CLOCKED_IN"),
        ("OUT_FOR_WORK", "CLOCK_OUT") | ("BUSINESS_TRIP", "CLOCK_OUT") => Ok("OFF_DUTY"),
        (_, _) => Err(HrError::from_kernel(KernelError::invalid_transition(
            "invalid employee attendance transition",
        ))),
    }
}

fn employee_lifecycle_event_from_row(
    row: sqlx::postgres::PgRow,
) -> Result<EmployeeLifecycleEventResponse, HrError> {
    let signoffs: Value = row.try_get("signoffs")?;
    let signoffs = serde_json::from_value::<EmployeeLifecycleSignoffs>(signoffs)
        .map_err(|err| HrError::validation(format!("invalid lifecycle signoffs: {err}")))?;
    Ok(EmployeeLifecycleEventResponse {
        id: row.try_get("id")?,
        employee_id: row.try_get("employee_id")?,
        event_type: row.try_get("event_type")?,
        from_status: row.try_get("from_status")?,
        to_status: row.try_get("to_status")?,
        from_company: row.try_get("from_company")?,
        to_company: row.try_get("to_company")?,
        from_org_unit: row.try_get("from_org_unit")?,
        to_org_unit: row.try_get("to_org_unit")?,
        from_position: row.try_get("from_position")?,
        to_position: row.try_get("to_position")?,
        effective_date: row.try_get("effective_date")?,
        comment: row.try_get("comment")?,
        signoffs,
        created_by: row.try_get("created_by")?,
        created_at: row.try_get("created_at")?,
    })
}

fn normalize_enum_text(value: String) -> String {
    value.trim().to_ascii_uppercase()
}

fn normalize_optional_text(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

#[derive(Debug, Serialize)]
pub(crate) struct NormalizedCreateEmployeeRequest {
    employee_number: String,
    name: String,
    company: String,
    employment_type: String,
    phone_e164: String,
    org_unit: String,
    position: String,
    site: String,
    home_branch_id: Uuid,
    base_pay: String,
    idempotency_key: String,
}

fn required_employee_text(value: String, field: &str) -> Result<String, HrError> {
    let normalized = value.trim().to_owned();
    if normalized.is_empty() || normalized.len() > 200 {
        return Err(HrError::validation(format!(
            "{field} is required and must be 200 characters or fewer"
        )));
    }
    Ok(normalized)
}

fn normalize_phone_e164(value: String) -> Result<String, HrError> {
    let compact: String = value
        .chars()
        .filter(|c| c.is_ascii_digit() || *c == '+')
        .collect();
    let phone = if let Some(local) = compact.strip_prefix("+82") {
        format!("+82{}", local.strip_prefix('0').unwrap_or(local))
    } else if let Some(international) = compact.strip_prefix("82") {
        format!(
            "+82{}",
            international.strip_prefix('0').unwrap_or(international)
        )
    } else if let Some(local) = compact.strip_prefix('0') {
        format!("+82{local}")
    } else if compact.starts_with('+') {
        compact
    } else {
        format!("+{compact}")
    };
    let valid = phone.len() >= 9
        && phone.len() <= 16
        && phone.starts_with('+')
        && phone[1..].chars().next().is_some_and(|c| c != '0')
        && phone[1..].chars().all(|c| c.is_ascii_digit());
    if !valid {
        return Err(HrError::validation("phone must be an E.164 number"));
    }
    Ok(phone)
}

pub(crate) fn normalize_create_employee_request(
    body: CreateEmployeeRequest,
) -> Result<NormalizedCreateEmployeeRequest, HrError> {
    let employment_type = normalize_enum_text(body.employment_type);
    if !matches!(
        employment_type.as_str(),
        "REGULAR" | "CONTRACT" | "PART_TIME" | "INTERN"
    ) {
        return Err(HrError::validation(
            "employment_type must be REGULAR, CONTRACT, PART_TIME, or INTERN",
        ));
    }
    let base_pay = normalize_employee_base_pay(body.base_pay)?;
    Ok(NormalizedCreateEmployeeRequest {
        employee_number: required_employee_text(body.employee_number, "employee_number")?,
        name: required_employee_text(body.name, "name")?,
        company: required_employee_text(body.company, "company")?,
        employment_type,
        phone_e164: normalize_phone_e164(body.phone)?,
        org_unit: required_employee_text(body.org_unit, "org_unit")?,
        position: required_employee_text(body.position, "position")?,
        site: required_employee_text(body.site, "site")?,
        home_branch_id: body.home_branch_id,
        base_pay,
        idempotency_key: normalize_idempotency_key(body.idempotency_key)?,
    })
}

/// Produces a fixed-scale decimal accepted by PostgreSQL `NUMERIC(14,2)` without
/// float rounding, exponent notation, leading zeros, or silent truncation.
fn normalize_employee_base_pay(value: String) -> Result<String, HrError> {
    let value = value.trim();
    let (whole, fraction) = value
        .split_once('.')
        .map_or((value, None), |(whole, fraction)| (whole, Some(fraction)));
    let valid_whole = whole == "0"
        || (whole.len() <= 12
            && !whole.starts_with('0')
            && whole.chars().all(|character| character.is_ascii_digit()));
    let valid_fraction = fraction.is_none_or(|fraction| {
        !fraction.is_empty()
            && fraction.len() <= 2
            && fraction.chars().all(|character| character.is_ascii_digit())
    });
    if !valid_whole || !valid_fraction {
        return Err(HrError::validation(
            "base_pay must be a canonical NUMERIC(14,2) decimal",
        ));
    }
    let fraction = fraction.unwrap_or("0");
    Ok(format!("{whole}.{fraction:0<2}"))
}

async fn load_employee_detail(
    tx: &mut sqlx::Transaction<'_, Postgres>,
    org_id: Uuid,
    employee_id: Uuid,
) -> Result<EmployeeDetailResponse, HrError> {
    let row = sqlx::query(
        r#"SELECT e.id, e.company, e.name, e.employee_number, e.org_unit, e.job,
            e.position, e.worksite_name, e.worksite_address, e.hire_date, e.exit_date,
            e.employment_status, e.leave_accrued::TEXT AS leave_accrued,
            e.leave_used::TEXT AS leave_used, e.leave_remaining::TEXT AS leave_remaining,
            b.id AS home_branch_id, b.name AS home_branch_name,
            e.identity_resolution_strategy, e.identity_resolution_confidence,
            e.identity_review_required, e.identity_name_only_merge, e.created_at, e.updated_at,
            p.employment_type, p.phone_e164, p.base_pay::TEXT AS base_pay, p.currency
        FROM employees e
        JOIN employee_employment_profiles p ON p.employee_id = e.id AND p.org_id = e.org_id
        LEFT JOIN branches b ON b.id = e.home_branch_id AND b.org_id = e.org_id AND b.deactivated_at IS NULL
        WHERE e.org_id = $1 AND e.id = $2"#,
    ).bind(org_id).bind(employee_id).fetch_optional(tx.as_mut()).await?
        .ok_or_else(|| HrError::from_kernel(KernelError::not_found("employee was not found in this organization")))?;
    let employment = EmployeeEmploymentDetail {
        employment_type: row.try_get("employment_type")?,
        phone_e164: row.try_get("phone_e164")?,
        base_pay: row.try_get("base_pay")?,
        currency: row.try_get("currency")?,
    };
    Ok(EmployeeDetailResponse {
        employee: employee_from_row(row)?,
        employment,
    })
}

fn employee_from_row(row: sqlx::postgres::PgRow) -> Result<EmployeeResponse, HrError> {
    Ok(EmployeeResponse {
        id: row.try_get("id")?,
        company: row.try_get("company")?,
        name: row.try_get("name")?,
        employee_number: row.try_get("employee_number")?,
        org_unit: row.try_get("org_unit")?,
        worksite_name: row.try_get("worksite_name")?,
        worksite: row.try_get("worksite_address")?,
        job: row.try_get("job")?,
        position: row.try_get("position")?,
        hire_date: row.try_get("hire_date")?,
        exit_date: row.try_get("exit_date")?,
        status: row.try_get("employment_status")?,
        leave_accrued: row.try_get("leave_accrued")?,
        leave_used: row.try_get("leave_used")?,
        leave_remaining: row.try_get("leave_remaining")?,
        home_branch_id: row.try_get("home_branch_id")?,
        home_branch_name: row.try_get("home_branch_name")?,
        home_branch_review_required: row.try_get::<Option<Uuid>, _>("home_branch_id")?.is_none(),
        identity_resolution_strategy: row.try_get("identity_resolution_strategy")?,
        identity_resolution_confidence: row.try_get("identity_resolution_confidence")?,
        identity_review_required: row.try_get("identity_review_required")?,
        identity_name_only_merge: row.try_get("identity_name_only_merge")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

fn standardized_employees_csv(rows: &[sqlx::postgres::PgRow]) -> Result<String, HrError> {
    let mut csv =
        "company,name,employee_number,org_unit,worksite_name,job,position,hire_date,exit_date,status,leave_remaining\r\n"
            .to_owned();
    for row in rows {
        let cells = [
            optional_row_text(row, "company")?,
            optional_row_text(row, "name")?,
            optional_row_text(row, "employee_number")?,
            optional_row_text(row, "org_unit")?,
            optional_row_text(row, "worksite_name")?,
            optional_row_text(row, "job")?,
            optional_row_text(row, "position")?,
            optional_row_text(row, "hire_date")?,
            optional_row_text(row, "exit_date")?,
            optional_row_text(row, "employment_status")?,
            optional_row_text(row, "leave_remaining")?,
        ];
        csv.push_str(
            &cells
                .iter()
                .map(|cell| csv_field(cell))
                .collect::<Vec<_>>()
                .join(","),
        );
        csv.push_str("\r\n");
    }
    Ok(csv)
}

fn optional_row_text(row: &sqlx::postgres::PgRow, key: &str) -> Result<String, HrError> {
    Ok(row.try_get::<Option<String>, _>(key)?.unwrap_or_default())
}

fn csv_field(value: &str) -> String {
    let safe = neutralize_spreadsheet_formula(value);
    if safe.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", safe.replace('"', "\"\""))
    } else {
        safe
    }
}

fn neutralize_spreadsheet_formula(value: &str) -> String {
    if matches!(value.chars().next(), Some('=' | '+' | '-' | '@')) {
        format!("'{value}")
    } else {
        value.to_owned()
    }
}

#[cfg(test)]
fn canonical_employee_fields(raw_row: &Value) -> EmployeeCanonicalFields {
    let exit_date = raw_text(raw_row, target_header_aliases("exit_date"));
    EmployeeCanonicalFields {
        employee_number: raw_text(raw_row, target_header_aliases("employee_number")),
        org_unit: raw_text(raw_row, target_header_aliases("org_unit")),
        job: raw_text(raw_row, target_header_aliases("job")),
        position: raw_text(raw_row, target_header_aliases("position")),
        worksite_name: raw_text(raw_row, target_header_aliases("worksite_name")),
        worksite_address: raw_text(raw_row, target_header_aliases("worksite_address")),
        hire_date: raw_text(raw_row, target_header_aliases("hire_date")),
        exit_date: exit_date.clone(),
        employment_status: if exit_date.is_some() {
            "EXITED"
        } else {
            "ACTIVE"
        }
        .to_owned(),
        leave_accrued: raw_decimal_text(raw_row, target_header_aliases("leave_accrued")),
        leave_used: raw_decimal_text(raw_row, target_header_aliases("leave_used")),
        leave_remaining: raw_decimal_text(raw_row, target_header_aliases("leave_remaining")),
    }
}

fn canonical_employee_fields_for_import(
    raw_row: &Value,
    columns: &[EmployeeImportColumn],
) -> EmployeeCanonicalFields {
    let exit_date = raw_text_for_import_target(raw_row, columns, "exit_date");
    EmployeeCanonicalFields {
        employee_number: raw_text_for_import_target(raw_row, columns, "employee_number"),
        org_unit: raw_text_for_import_target(raw_row, columns, "org_unit"),
        job: raw_text_for_import_target(raw_row, columns, "job"),
        position: raw_text_for_import_target(raw_row, columns, "position"),
        worksite_name: raw_text_for_import_target(raw_row, columns, "worksite_name"),
        worksite_address: raw_text_for_import_target(raw_row, columns, "worksite_address"),
        hire_date: raw_text_for_import_target(raw_row, columns, "hire_date"),
        exit_date: exit_date.clone(),
        employment_status: if exit_date.is_some() {
            "EXITED"
        } else {
            "ACTIVE"
        }
        .to_owned(),
        leave_accrued: raw_decimal_text_for_import_target(raw_row, columns, "leave_accrued"),
        leave_used: raw_decimal_text_for_import_target(raw_row, columns, "leave_used"),
        leave_remaining: raw_decimal_text_for_import_target(raw_row, columns, "leave_remaining"),
    }
}

#[cfg(test)]
fn raw_decimal_text(raw_row: &Value, headers: &[&str]) -> Option<String> {
    let raw = raw_text(raw_row, headers)?;
    normalized_decimal_text(&raw)
}

fn raw_decimal_text_for_import_target(
    raw_row: &Value,
    columns: &[EmployeeImportColumn],
    target: &str,
) -> Option<String> {
    let raw = raw_text_for_import_target(raw_row, columns, target)?;
    normalized_decimal_text(&raw)
}

fn normalized_decimal_text(raw: &str) -> Option<String> {
    let cleaned = raw.replace(',', "");
    LeaveBalanceAmount::parse_decimal(cleaned.trim())
        .ok()
        .map(LeaveBalanceAmount::canonical_decimal)
}

fn raw_text_for_import_target(
    raw_row: &Value,
    columns: &[EmployeeImportColumn],
    target: &str,
) -> Option<String> {
    let object = raw_row.as_object()?;
    columns
        .iter()
        .filter(|column| column.target.as_deref() == Some(target))
        .find_map(|column| {
            let value = object.get(&column.normalized_header)?;
            json_value_text(value)
        })
        .or_else(|| raw_text(raw_row, target_header_aliases(target)))
}

fn raw_text(raw_row: &Value, headers: &[&str]) -> Option<String> {
    let object = raw_row.as_object()?;
    headers.iter().find_map(|header| {
        let value = object.get(*header)?;
        json_value_text(value)
    })
}

fn json_value_text(value: &Value) -> Option<String> {
    let text = match value {
        Value::String(value) => value.trim().to_owned(),
        Value::Number(value) => value.to_string(),
        Value::Bool(value) => value.to_string(),
        Value::Null | Value::Array(_) | Value::Object(_) => String::new(),
    };
    (!text.is_empty()).then_some(text)
}

fn target_header_aliases(target: &str) -> &'static [&'static str] {
    employee_import_field_for_target(target)
        .map(|field| field.aliases)
        .unwrap_or(&[])
}

fn find_or_insert_company(companies: &mut Vec<HrOrgChartCompany>, name: String) -> usize {
    if let Some(index) = companies.iter().position(|company| company.company == name) {
        return index;
    }
    companies.push(HrOrgChartCompany {
        company: name,
        total: 0,
        active: 0,
        units: vec![],
    });
    companies.len() - 1
}

fn find_or_insert_unit(units: &mut Vec<HrOrgChartUnit>, name: String) -> usize {
    if let Some(index) = units.iter().position(|unit| unit.name == name) {
        return index;
    }
    units.push(HrOrgChartUnit {
        name,
        total: 0,
        positions: vec![],
    });
    units.len() - 1
}

fn find_or_insert_position(positions: &mut Vec<HrOrgChartPosition>, title: String) -> usize {
    if let Some(index) = positions
        .iter()
        .position(|position| position.title == title)
    {
        return index;
    }
    positions.push(HrOrgChartPosition {
        title,
        total: 0,
        employees: vec![],
    });
    positions.len() - 1
}

fn push_attendance_branch_scope(builder: &mut QueryBuilder<Postgres>, scope: &BranchScope) {
    match scope {
        BranchScope::All => {
            builder.push(" TRUE ");
        }
        BranchScope::Branches(branches) if branches.is_empty() => {
            builder.push(" FALSE ");
        }
        BranchScope::Branches(branches) => {
            let ids = branches
                .iter()
                .map(|branch| *branch.as_uuid())
                .collect::<Vec<_>>();
            builder.push(" l.branch_id = ANY(");
            builder.push_bind(ids);
            builder.push(") ");
        }
    };
}

/// Manager attendance endpoints either receive a concrete authorized branch or
/// passed the explicit organization-wide gate. Do not derive a filter from the
/// caller's ambient scope here: branch omission is already a distinct contract.
fn push_attendance_branch_filter(builder: &mut QueryBuilder<Postgres>, branch_id: Option<Uuid>) {
    match branch_id {
        Some(branch_id) => {
            builder.push(" l.branch_id = ");
            builder.push_bind(branch_id);
            builder.push(" ");
        }
        None => {
            builder.push(" TRUE ");
        }
    }
}

fn push_branch_scope_column(
    builder: &mut QueryBuilder<Postgres>,
    scope: &BranchScope,
    column: &'static str,
) {
    match scope {
        BranchScope::All => {
            builder.push(" TRUE ");
        }
        BranchScope::Branches(branches) if branches.is_empty() => {
            builder.push(" FALSE ");
        }
        BranchScope::Branches(branches) => {
            let ids = branches
                .iter()
                .map(|branch| *branch.as_uuid())
                .collect::<Vec<_>>();
            builder.push(column);
            builder.push(" = ANY(");
            builder.push_bind(ids);
            builder.push(") ");
        }
    };
}

fn record_hr_read(surface: &'static str) {
    metrics::counter!("hr_core_requests_total", "surface" => surface).increment(1);
}

fn record_hr_import(inserted: usize, updated: usize) {
    metrics::counter!("hr_employee_import_rows_total", "outcome" => "inserted")
        .increment(inserted as u64);
    metrics::counter!("hr_employee_import_rows_total", "outcome" => "updated")
        .increment(updated as u64);
}

fn authorize_hr_org_wide(principal: &Principal, feature: Feature) -> Result<(), HrError> {
    authorize_org_wide(principal, Action::new(feature)).map_err(HrError::from_kernel)
}

/// Attendance manager routes are explicit about whether a concrete branch was
/// requested. A missing query parameter is an org-wide request; it is never
/// inferred from the caller's branch membership.
fn authorize_hr_attendance_branch_query(
    principal: &Principal,
    feature: Feature,
    branch_id: Option<Uuid>,
) -> Result<Option<Uuid>, HrError> {
    match branch_id {
        Some(branch_id) => {
            authorize(
                principal,
                Action::new(feature),
                BranchId::from_uuid(branch_id),
            )
            .map_err(HrError::from_kernel)?;
            Ok(Some(branch_id))
        }
        None => {
            authorize_hr_org_wide(principal, feature)?;
            Ok(None)
        }
    }
}

fn authorize_hr_scoped(principal: &Principal, feature: Feature) -> Result<(), HrError> {
    let action = Action::new(feature);
    match &principal.branch_scope {
        // Preserve the existing org-wide tier: collapsing this arm into the
        // capability check would admit an all-scoped ADMIN to a surface that
        // authorize_org_wide deliberately reserves for its org-wide roles.
        BranchScope::All => authorize_hr_org_wide(principal, feature),
        // The dashboard/store confines rows to the caller's complete scope. No
        // concrete resource branch exists at this preflight point, so selecting
        // one of the principal's own branches would make authorization vacuous.
        BranchScope::Branches(_) => {
            authorize_capability(principal, action).map_err(HrError::from_kernel)
        }
    }
}

/// Enforce the second-tier (HQ) separation of duties for an exit-case
/// confirmation. Called only after the `ExitCaseHqConfirm` capability check has
/// passed. The stored case state plus the distinct-actor rule — never the
/// client `hq_confirmation` boolean — is the authority: HQ confirmation is
/// allowed only when the case is already `HR_CONFIRMED` AND the recorded HR
/// confirmer is a DIFFERENT actor than the one now attempting HQ confirmation.
fn authorize_exit_confirmation_hq_tier(
    current_status: &str,
    hr_confirmed_by: Option<Uuid>,
    actor: Uuid,
) -> Result<(), HrError> {
    if current_status != "HR_CONFIRMED" {
        return Err(HrError::from_kernel(KernelError::invalid_transition(
            "HQ confirmation requires a prior HR confirmation",
        )));
    }
    match hr_confirmed_by {
        Some(hr_actor) if hr_actor == actor => Err(HrError::from_kernel(KernelError::forbidden(
            "HQ confirmation must be performed by a different actor than the HR confirmer",
        ))),
        Some(_) => Ok(()),
        None => Err(HrError::from_kernel(KernelError::invalid_transition(
            "HQ confirmation requires a recorded HR confirmer",
        ))),
    }
}

fn authorize_hr_scoped_write(
    principal: &Principal,
    feature: Feature,
    branch_id: Option<Uuid>,
) -> Result<(), HrError> {
    if let Some(branch_id) = branch_id {
        return authorize(
            principal,
            Action::new(feature),
            BranchId::from_uuid(branch_id),
        )
        .map_err(HrError::from_kernel);
    }
    // A branchless employee/case cannot be confined to one of the caller's
    // branches. Require true org-wide authority instead of treating the write as
    // a branchless capability over a branch-scoped principal.
    authorize_hr_org_wide(principal, feature)
}

fn authorize_home_branch_transition(
    principal: &Principal,
    prior_branch_id: Option<Uuid>,
    target_branch_id: Uuid,
) -> Result<(), HrError> {
    authorize(
        principal,
        Action::new(Feature::EmployeeDirectoryManage),
        BranchId::from_uuid(target_branch_id),
    )
    .map_err(HrError::from_kernel)?;
    match prior_branch_id {
        Some(prior_branch_id) => authorize(
            principal,
            Action::new(Feature::EmployeeDirectoryManage),
            BranchId::from_uuid(prior_branch_id),
        )
        .map_err(HrError::from_kernel),
        None => authorize_hr_org_wide(principal, Feature::EmployeeDirectoryManage),
    }
}

fn normalize_date_text(value: &str) -> Result<String, HrError> {
    Ok(parse_yyyy_mm_dd(value)?.to_string())
}

fn parse_yyyy_mm_dd(value: &str) -> Result<Date, HrError> {
    let value = value.trim();
    let parts = value.split('-').collect::<Vec<_>>();
    if parts.len() != 3 || parts[0].len() != 4 || parts[1].len() != 2 || parts[2].len() != 2 {
        return Err(HrError::validation("date must use YYYY-MM-DD"));
    }
    let year = parts[0]
        .parse::<i32>()
        .map_err(|_| HrError::validation("date year must be numeric"))?;
    let month = parts[1]
        .parse::<u8>()
        .map_err(|_| HrError::validation("date month must be numeric"))?;
    let day = parts[2]
        .parse::<u8>()
        .map_err(|_| HrError::validation("date day must be numeric"))?;
    let month = Month::try_from(month).map_err(|_| HrError::validation("date month is invalid"))?;
    Date::from_calendar_date(year, month, day)
        .map_err(|_| HrError::validation("date day is invalid"))
}

fn normalize_limited_text(
    value: String,
    max_chars: usize,
    field: &'static str,
) -> Result<String, HrError> {
    let value = normalize_optional_text(Some(value))
        .ok_or_else(|| HrError::validation(format!("{field} is required")))?;
    if value.chars().count() > max_chars {
        return Err(HrError::validation(format!(
            "{field} must be {max_chars} characters or fewer"
        )));
    }
    Ok(value)
}

fn normalize_optional_limited_text(
    value: Option<String>,
    max_chars: usize,
    field: &'static str,
) -> Result<Option<String>, HrError> {
    match normalize_optional_text(value) {
        Some(value) if value.chars().count() > max_chars => Err(HrError::validation(format!(
            "{field} must be {max_chars} characters or fewer"
        ))),
        value => Ok(value),
    }
}

#[derive(Debug)]
pub(crate) struct HrError {
    status: StatusCode,
    code: &'static str,
    message: String,
}

impl HrError {
    pub(crate) fn from_kernel(error: KernelError) -> Self {
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

    pub(crate) fn validation(message: impl Into<String>) -> Self {
        Self::from_kernel(KernelError::validation(message.into()))
    }

    fn workbook(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::UNPROCESSABLE_ENTITY,
            code: "workbook",
            message: message.into(),
        }
    }

    fn internal(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            code: "internal",
            message: message.into(),
        }
    }

    fn unavailable(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::SERVICE_UNAVAILABLE,
            code: "service_unavailable",
            message: message.into(),
        }
    }

    fn from_leave_store(error: PgLeaveError) -> Self {
        match error {
            PgLeaveError::CommandUnavailable => Self::unavailable(
                "leave command database is not configured; protected employee leave changes are unavailable",
            ),
            PgLeaveError::Domain(error) => Self::from_kernel(error),
            PgLeaveError::ConcurrentModification => Self::from_kernel(KernelError::conflict(
                "employee changed since it was read; reload before retrying the protected leave change",
            )),
            PgLeaveError::Db(error) => Self::from(error),
            PgLeaveError::MissingHomeBranch | PgLeaveError::ChargeReviewRequired(_) => {
                tracing::error!(error = ?error, "unexpected protected employee leave command result");
                Self::internal("protected employee leave change failed")
            }
        }
    }
}

impl From<KernelError> for HrError {
    fn from(error: KernelError) -> Self {
        Self::from_kernel(error)
    }
}

impl From<DbError> for HrError {
    fn from(value: DbError) -> Self {
        tracing::error!(error = ?value, "employee directory database operation failed");
        Self::internal("employee directory request failed")
    }
}

impl From<sqlx::Error> for HrError {
    fn from(value: sqlx::Error) -> Self {
        HrError::from(DbError::Sqlx(value))
    }
}

impl From<PersonError> for HrError {
    fn from(error: PersonError) -> Self {
        match error {
            PersonError::Blocked(blockers) => Self::from_kernel(KernelError::validation(format!(
                "preflight blocked the command: {blockers:?}"
            ))),
            PersonError::DigestConflict(id) => Self::from_kernel(KernelError::conflict(format!(
                "command {id} was already applied with a different payload"
            ))),
            PersonError::Database(error) => Self::from(error),
            PersonError::UnreadableReceipt(id, message) => {
                tracing::error!(command = %id, receipt = %message, "person command receipt unreadable");
                Self::internal("employee directory request failed")
            }
        }
    }
}

impl From<employment::EmploymentError> for HrError {
    fn from(error: employment::EmploymentError) -> Self {
        match error {
            // §3.9.1 freeze refusal (Conflict) and unparseable date (Validation)
            // carry client-safe, actionable messages and pass through unchanged.
            // A gate failure (Internal) carries raw driver diagnostics that must
            // be logged, not returned, per the kernel error contract.
            employment::EmploymentError::Frozen(error) => match error.kind {
                ErrorKind::Internal => {
                    tracing::error!(error = %error.message, "employment freeze gate failed");
                    Self::internal("employment change could not be applied")
                }
                _ => Self::from_kernel(error),
            },
            // Preserve the exact driver-error mapping (and its trace log).
            employment::EmploymentError::Database(error) => Self::from(error),
            employment::EmploymentError::UnknownOrgUnit(id) => {
                Self::from_kernel(KernelError::validation(format!(
                    "org_unit_id {id} is not a known OrgUnit in this tenant"
                )))
            }
            employment::EmploymentError::UnknownJobPosition(id) => {
                Self::from_kernel(KernelError::validation(format!(
                    "job_position_id {id} is not a known JobPosition in this tenant"
                )))
            }
            employment::EmploymentError::OrgUnitRefNotUuid => {
                Self::from_kernel(KernelError::validation(
                    "fromOrgUnit/toOrgUnit must be OrgUnit UUIDs, not free-text team labels"
                        .to_owned(),
                ))
            }
            // Port-level refusals other than the directory UUID refs surface
            // through the ontology dispatcher. Mapped as internal so a future
            // variant fails loud rather than as a wrong HTTP status.
            other => Self::from_kernel(KernelError::internal(other.to_string())),
        }
    }
}

impl IntoResponse for HrError {
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
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use calamine::Range;

    /// Exercises the LIVE custom-grant recheck at the transactional boundary
    /// directly (bypassing the request-time pre-flight): an EXECUTIVE whose
    /// org-wide `employee_directory_manage` grant is revoked after the
    /// principal was minted must be refused 403 by `create_employee_core`
    /// itself, not merely by `authorize_hr_org_wide`.
    #[cfg(feature = "test-postgres")]
    #[sqlx::test(migrations = "../crates/platform/db/migrations")]
    async fn create_employee_core_refuses_a_revoked_custom_grant(
        pool: sqlx::PgPool,
    ) -> Result<(), String> {
        use console_kernel_core::UserId;
        let org_id = Uuid::new_v4();
        let region_id = Uuid::new_v4();
        let branch_id = Uuid::new_v4();
        let actor = UserId::new();
        sqlx::query("INSERT INTO organizations (id,slug,name) VALUES ($1,$2,'Grant recheck test')")
            .bind(org_id)
            .bind(format!("grant-recheck-{}", &org_id.to_string()[..8]))
            .execute(&pool)
            .await
            .map_err(|error| error.to_string())?;
        sqlx::query("INSERT INTO regions (id,name,org_id) VALUES ($1,$2,$3)")
            .bind(region_id)
            .bind("Grant recheck region")
            .bind(org_id)
            .execute(&pool)
            .await
            .map_err(|error| error.to_string())?;
        sqlx::query("INSERT INTO branches (id,region_id,name,org_id) VALUES ($1,$2,'본사',$3)")
            .bind(branch_id)
            .bind(region_id)
            .bind(org_id)
            .execute(&pool)
            .await
            .map_err(|error| error.to_string())?;
        sqlx::query(
            "INSERT INTO users (id,display_name,roles,is_active,org_id) \
             VALUES ($1,'Executive',ARRAY['EXECUTIVE']::text[],true,$2)",
        )
        .bind(*actor.as_uuid())
        .bind(org_id)
        .execute(&pool)
        .await
        .map_err(|error| error.to_string())?;
        let role_id: Uuid = sqlx::query_scalar(
            "INSERT INTO policy_roles (org_id,role_key,display_name,status) \
             VALUES ($1,$2,$3,'ACTIVE') RETURNING id",
        )
        .bind(org_id)
        .bind(format!("grant_recheck_{}", Uuid::new_v4().simple()))
        .bind("Grant recheck role")
        .fetch_one(&pool)
        .await
        .map_err(|error| error.to_string())?;
        sqlx::query(
            "INSERT INTO policy_role_permissions (org_id,role_id,feature_key,permission_level) \
             VALUES ($1,$2,'employee_directory_manage','allow')",
        )
        .bind(org_id)
        .bind(role_id)
        .execute(&pool)
        .await
        .map_err(|error| error.to_string())?;
        sqlx::query("INSERT INTO user_role_assignments (org_id,user_id,role_id) VALUES ($1,$2,$3)")
            .bind(org_id)
            .bind(*actor.as_uuid())
            .bind(role_id)
            .execute(&pool)
            .await
            .map_err(|error| error.to_string())?;

        // Revoke the grant, then invoke the transactional boundary DIRECTLY with
        // the formerly-authorized principal.
        sqlx::query("DELETE FROM user_role_assignments WHERE org_id=$1 AND user_id=$2")
            .bind(org_id)
            .bind(*actor.as_uuid())
            .execute(&pool)
            .await
            .map_err(|error| error.to_string())?;

        let request = NormalizedCreateEmployeeRequest {
            employee_number: "GRANT-REVOKED".to_owned(),
            name: "Grant revoked".to_owned(),
            company: "테스트".to_owned(),
            employment_type: "REGULAR".to_owned(),
            phone_e164: "010-1234-5678".to_owned(),
            org_unit: "인사".to_owned(),
            position: "사원".to_owned(),
            site: "서울".to_owned(),
            home_branch_id: branch_id,
            base_pay: "50000000".to_owned(),
            idempotency_key: "grant-revoked-key".to_owned(),
        };
        let mut tx = pool.begin().await.map_err(|error| error.to_string())?;
        sqlx::query("SELECT set_config('app.current_org', $1, true)")
            .bind(org_id.to_string())
            .execute(tx.as_mut())
            .await
            .map_err(|error| error.to_string())?;
        let result = create_employee_core(
            &mut tx,
            org_id,
            actor,
            &request,
            "grant-revoked-hash",
            Uuid::new_v4(),
        )
        .await;
        let err = match result {
            Ok(_) => return Err("expected a grant-revoked actor to be refused".to_owned()),
            Err(err) => err,
        };
        assert_eq!(err.status, StatusCode::FORBIDDEN);
        assert!(
            err.message.contains("grant was revoked"),
            "unexpected message: {}",
            err.message
        );
        tx.rollback().await.map_err(|error| error.to_string())?;
        Ok(())
    }

    #[cfg(not(feature = "test-postgres"))]
    #[test]
    fn frozen_internal_gate_failure_is_redacted_in_the_rest_response() {
        // A gate failure (Internal) carries raw driver diagnostics; the REST
        // mapping must log, not return, them.
        let err = HrError::from(employment::EmploymentError::Frozen(KernelError::internal(
            "period lock check failed: sqlx database connection refused (secret host)",
        )));
        assert_eq!(err.status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(err.code, "internal");
        assert!(!err.message.contains("sqlx"));
        assert!(!err.message.contains("secret host"));

        // A closed window (Conflict) stays actionable, not redacted.
        let refused = HrError::from(employment::EmploymentError::Frozen(KernelError::conflict(
            "payroll period 2026-08-01..2026-08-31 is locked; write refused",
        )));
        assert_eq!(refused.status, StatusCode::CONFLICT);
        assert!(refused.message.contains("locked"));
    }

    #[cfg(not(feature = "test-postgres"))]
    #[test]
    fn parses_each_sheet_as_company_and_preserves_extra_columns() -> Result<(), String> {
        let mut range = Range::new((0, 0), (2, 2));
        range.set_value((0, 0), Data::String("성명".to_owned()));
        range.set_value((0, 1), Data::String("급여".to_owned()));
        range.set_value((0, 2), Data::String("비고".to_owned()));
        range.set_value((1, 0), Data::String("홍길동".to_owned()));
        range.set_value((1, 1), Data::Int(123));
        range.set_value((1, 2), Data::String("민감".to_owned()));
        range.set_value((2, 1), Data::String("성명 없음".to_owned()));

        let rows = parse_employee_sheet("payroll.xlsx", "A회사", &range)
            .map_err(|err| format!("expected employee sheet to parse, got {err:?}"))?;

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].company, "A회사");
        assert_eq!(rows[0].name, "홍길동");
        assert_eq!(
            rows[0].source_key,
            "filename:payroll.xlsx|sheet:A회사|row:2"
        );
        assert_eq!(rows[0].raw_row["급여"], json!(123));
        assert_eq!(rows[0].raw_row["비고"], json!("민감"));
        Ok(())
    }

    #[cfg(not(feature = "test-postgres"))]
    #[test]
    fn missing_name_header_is_a_workbook_error() -> Result<(), String> {
        let mut range = Range::new((0, 0), (0, 0));
        range.set_value((0, 0), Data::String("급여".to_owned()));

        let err = match parse_employee_sheet("payroll.xlsx", "A회사", &range) {
            Ok(rows) => return Err(format!("expected missing-name-header error, got {rows:?}")),
            Err(err) => err,
        };

        assert_eq!(err.status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(err.code, "workbook");
        Ok(())
    }

    #[cfg(not(feature = "test-postgres"))]
    #[test]
    fn attendance_import_csv_parses_valid_rows_and_masks_preview_values() -> Result<(), String> {
        let csv = "\
사번,성명,지점,근무일,출근시간,퇴근시간,근무분,급여메모
E-001,=홍길동,본사,2026-07-01,09:00,18:00,540,=cmd|' /C calc'!A0
";
        let parsed = parse_attendance_import_upload("attendance.csv", csv.as_bytes())
            .map_err(|err| format!("expected attendance CSV to parse, got {err:?}"))?;

        assert_eq!(parsed.rows.len(), 1);
        assert_eq!(parsed.rows[0].row_status, ImportRowStatus::Candidate);
        assert_eq!(parsed.rows[0].employee_number.as_deref(), Some("E-001"));
        assert_eq!(parsed.rows[0].employee_name.as_deref(), Some("=홍길동"));
        assert_eq!(parsed.rows[0].branch_name.as_deref(), Some("본사"));
        assert_eq!(parsed.rows[0].minutes_worked, Some(540));
        assert_eq!(parsed.rows[0].work_date.as_deref(), Some("2026-07-01"));
        assert_eq!(parsed.rows[0].source_key, "sheet:CSV|row:2");

        let response = AttendanceImportPreviewResponse::from_rows(
            Uuid::nil(),
            "attendance.csv".to_owned(),
            "0".repeat(64),
            parsed.rows,
        );
        let values = &response.sample_rows[0].values;
        assert_eq!(values.get("성명"), Some(&json!("'=홍길동")));
        assert_eq!(values.get("급여메모"), Some(&json!("••••")));
        assert_eq!(
            response.mapping_profile["policy"]["payroll_effect"],
            json!("lineage_only_not_payable")
        );
        Ok(())
    }

    #[cfg(not(feature = "test-postgres"))]
    #[test]
    fn attendance_import_marks_missing_employee_and_duplicate_rows() -> Result<(), String> {
        let missing_employee_csv = "\
사번,성명,지점,근무일,출근시간
,,본사,2026-07-01,09:00
";
        let parsed =
            parse_attendance_import_upload("attendance.csv", missing_employee_csv.as_bytes())
                .map_err(|err| format!("expected row-level validation, got {err:?}"))?;

        assert_eq!(parsed.rows.len(), 1);
        assert_eq!(parsed.rows[0].row_status, ImportRowStatus::Error);
        assert!(
            parsed.rows[0]
                .validation_errors
                .contains(&"missing_employee_identifier".to_owned())
        );

        let duplicate_csv = "\
사번,성명,지점,근무일,출근시간,퇴근시간
E-001,홍길동,본사,2026-07-01,09:00,18:00
E-001,홍길동,본사,2026-07-01,09:00,18:00
";
        let duplicate = parse_attendance_import_upload("attendance.csv", duplicate_csv.as_bytes())
            .map_err(|err| format!("expected duplicate row validation, got {err:?}"))?;

        assert_eq!(duplicate.rows.len(), 2);
        assert!(
            duplicate
                .rows
                .iter()
                .all(|row| row.row_status == ImportRowStatus::Error)
        );
        assert!(duplicate.rows.iter().all(|row| {
            row.validation_errors
                .contains(&"duplicate_row_in_file".to_owned())
        }));
        Ok(())
    }

    #[cfg(not(feature = "test-postgres"))]
    #[test]
    fn attendance_import_rejects_invalid_work_date() -> Result<(), String> {
        let csv = "\
사번,성명,지점,근무일,출근시간
E-001,홍길동,본사,45500,09:00
";
        let parsed = parse_attendance_import_upload("attendance.csv", csv.as_bytes())
            .map_err(|err| format!("expected row-level work-date validation, got {err:?}"))?;

        assert_eq!(parsed.rows.len(), 1);
        assert_eq!(parsed.rows[0].row_status, ImportRowStatus::Error);
        assert!(
            parsed.rows[0]
                .validation_errors
                .contains(&"invalid_work_date".to_owned())
        );
        Ok(())
    }

    #[cfg(not(feature = "test-postgres"))]
    #[test]
    fn attendance_import_rejects_invalid_attendance_time() -> Result<(), String> {
        let csv = "\
사번,성명,지점,근무일,출근시간
E-001,홍길동,본사,2026-07-01,25:99
";
        let parsed = parse_attendance_import_upload("attendance.csv", csv.as_bytes())
            .map_err(|err| format!("expected row-level time validation, got {err:?}"))?;

        assert_eq!(parsed.rows.len(), 1);
        assert_eq!(parsed.rows[0].row_status, ImportRowStatus::Error);
        assert!(
            parsed.rows[0]
                .validation_errors
                .contains(&"invalid_check_in_at".to_owned())
        );
        Ok(())
    }
    #[cfg(feature = "test-postgres")]
    #[sqlx::test(migrations = "../crates/platform/db/migrations")]
    async fn attendance_import_resolves_dedups_and_enforces_runtime_guards(
        pool: sqlx::PgPool,
    ) -> Result<(), String> {
        let org_id = Uuid::new_v4();
        let branch_id = Uuid::new_v4();
        let region_id = Uuid::new_v4();
        let employee_id = Uuid::new_v4();
        let run_id = Uuid::new_v4();
        let ready_row_id = Uuid::new_v4();
        let duplicate_row_id = Uuid::new_v4();
        let user_id = Uuid::new_v4();
        let mailbox_domain_id = Uuid::new_v4();
        let mailbox_id = Uuid::new_v4();
        let mailbox_alias_id = Uuid::new_v4();
        let mailbox_message_id = Uuid::new_v4();
        let mailbox_delivery_id = Uuid::new_v4();
        let source_sha256 = "a".repeat(64);

        sqlx::query("INSERT INTO organizations (id, slug, name) VALUES ($1, $2, $3)")
            .bind(org_id)
            .bind(format!("attendance-{}", &org_id.to_string()[..8]))
            .bind("Attendance Import Test")
            .execute(&pool)
            .await
            .map_err(|err| format!("seed organization failed: {err}"))?;
        sqlx::query("INSERT INTO regions (id, name, org_id) VALUES ($1, $2, $3)")
            .bind(region_id)
            .bind(format!("Attendance Region {}", &region_id.to_string()[..8]))
            .bind(org_id)
            .execute(&pool)
            .await
            .map_err(|err| format!("seed region failed: {err}"))?;
        sqlx::query("INSERT INTO branches (id, region_id, name, org_id) VALUES ($1, $2, $3, $4)")
            .bind(branch_id)
            .bind(region_id)
            .bind("본사")
            .bind(org_id)
            .execute(&pool)
            .await
            .map_err(|err| format!("seed branch failed: {err}"))?;
        sqlx::query(
            "INSERT INTO users (id, display_name, roles, is_active, org_id) VALUES ($1, $2, ARRAY['ADMIN']::TEXT[], true, $3)",
        )
        .bind(user_id)
        .bind("Mailbox Force Remove Owner")
        .bind(org_id)
        .execute(&pool)
        .await
        .map_err(|err| format!("seed mailbox owner user failed: {err}"))?;
        let mailbox_domain = format!("attendance-{}.example.test", &org_id.to_string()[..8]);
        sqlx::query(
            r#"
            INSERT INTO mailbox_domains (
                id, org_id, domain, created_by
            )
            VALUES ($1, $2, $3, $4)
            "#,
        )
        .bind(mailbox_domain_id)
        .bind(org_id)
        .bind(&mailbox_domain)
        .bind(user_id)
        .execute(&pool)
        .await
        .map_err(|err| format!("seed mailbox domain failed: {err}"))?;
        sqlx::query(
            r#"
            INSERT INTO mailboxes (
                id, org_id, domain_id, local_part, display_name, mailbox_kind, created_by
            )
            VALUES ($1, $2, $3, 'ops', 'Ops Mailbox', 'SHARED', $4)
            "#,
        )
        .bind(mailbox_id)
        .bind(org_id)
        .bind(mailbox_domain_id)
        .bind(user_id)
        .execute(&pool)
        .await
        .map_err(|err| format!("seed mailbox failed: {err}"))?;
        sqlx::query(
            r#"
            INSERT INTO mailbox_aliases (
                id, org_id, domain_id, target_mailbox_id, local_part, created_by
            )
            VALUES ($1, $2, $3, $4, 'alias', $5)
            "#,
        )
        .bind(mailbox_alias_id)
        .bind(org_id)
        .bind(mailbox_domain_id)
        .bind(mailbox_id)
        .bind(user_id)
        .execute(&pool)
        .await
        .map_err(|err| format!("seed mailbox alias failed: {err}"))?;
        sqlx::query(
            r#"
            INSERT INTO mailbox_messages (
                id, org_id, mailbox_id, domain_id, direction,
                raw_object_key, raw_size_bytes, received_at
            )
            VALUES ($1, $2, $3, $4, 'IN', 'raw-object-12345678', 0, now())
            "#,
        )
        .bind(mailbox_message_id)
        .bind(org_id)
        .bind(mailbox_id)
        .bind(mailbox_domain_id)
        .execute(&pool)
        .await
        .map_err(|err| format!("seed mailbox message failed: {err}"))?;
        sqlx::query(
            r#"
            INSERT INTO mailbox_deliveries (
                id, org_id, mailbox_message_id, direction, status,
                recipient_domain, recipient_local_part, queue_key,
                accepted_at, completed_at
            )
            VALUES ($1, $2, $3, 'IN', 'STORED', $4, 'ops', $5, now(), now())
            "#,
        )
        .bind(mailbox_delivery_id)
        .bind(org_id)
        .bind(mailbox_message_id)
        .bind(&mailbox_domain)
        .bind(format!("queue-{}", mailbox_delivery_id))
        .execute(&pool)
        .await
        .map_err(|err| format!("seed mailbox delivery failed: {err}"))?;
        sqlx::query(
            r#"
            INSERT INTO employees (
                id, org_id, company, name, employee_number, source_filename,
                source_sheet, source_row, source_key, raw_row, source_metadata
            )
            VALUES ($1, $2, '테스트', '홍길동', NULL, 'employees.xlsx', '직원', 2, 'employee-row-2', '{}', '{}')
            "#,
        )
        .bind(employee_id)
        .bind(org_id)
        .execute(&pool)
        .await
        .map_err(|err| format!("seed employee failed: {err}"))?;
        sqlx::query(
            r#"
            INSERT INTO employee_lifecycle_events (
                org_id, employee_id, event_type, to_status, effective_date, comment, created_by
            )
            VALUES ($1, $2, 'ONBOARD', 'ACTIVE', '2026-07-01', 'force-remove test lifecycle event', $3)
            "#,
        )
        .bind(org_id)
        .bind(employee_id)
        .bind(user_id)
        .execute(&pool)
        .await
        .map_err(|err| format!("seed lifecycle event failed: {err}"))?;
        sqlx::query(
            r#"
            INSERT INTO data_import_runs (
                id, org_id, entity_type, status, source_filename, source_format,
                source_sha256, mapping_profile, input_rows, candidate_rows, preserved_rows,
                pay_period_start, pay_period_end
            )
            VALUES ($1, $2, 'attendance_direct', 'DRY_RUN', 'attendance.csv', 'csv', $3, '{}', 2, 2, 0, DATE '2026-06-01', DATE '2026-06-30')
            "#,
        )
        .bind(run_id)
        .bind(org_id)
        .bind(&source_sha256)
        .execute(&pool)
        .await
        .map_err(|err| format!("seed import run failed: {err}"))?;

        let ready_canonical = json!({
            "source_sheet": "CSV",
            "source_row": 2,
            "source_key": "sheet:CSV|row:2",
            "source_sha256": source_sha256,
            "canonical": {
                "employee_number": null,
                "employee_name": "홍길동",
                "branch_name": "본사",
                "work_date": "2026-07-01",
                "check_in_at": "09:00",
                "check_out_at": "18:00",
                "minutes_worked": 540
            }
        });
        let duplicate_canonical = json!({
            "source_sheet": "CSV",
            "source_row": 3,
            "source_key": "sheet:CSV|row:3",
            "source_sha256": source_sha256,
            "canonical": {
                "employee_number": null,
                "employee_name": "홍길동",
                "branch_name": "본사",
                "work_date": "2026-07-02",
                "check_in_at": "09:00",
                "check_out_at": "18:00",
                "minutes_worked": 540
            }
        });
        for (row_id, source_row, source_key, canonical) in [
            (ready_row_id, 2, "sheet:CSV|row:2", ready_canonical),
            (duplicate_row_id, 3, "sheet:CSV|row:3", duplicate_canonical),
        ] {
            sqlx::query(
                r#"
                INSERT INTO data_import_rows (
                    id, org_id, run_id, source_sheet, source_row, source_key,
                    row_status, raw_row, canonical_row, validation
                )
                VALUES ($1, $2, $3, 'CSV', $4, $5, 'CANDIDATE', '{}', $6, '{"status":"ok","errors":[],"warnings":[]}')
                "#,
            )
            .bind(row_id)
            .bind(org_id)
            .bind(run_id)
            .bind(source_row)
            .bind(source_key)
            .bind(canonical)
            .execute(&pool)
            .await
            .map_err(|err| format!("seed import row {source_key} failed: {err}"))?;
        }
        let existing_fact_key = attendance_fact_key(
            employee_id,
            branch_id,
            "2026-07-02",
            Some("09:00"),
            Some("18:00"),
            Some(540),
        );
        sqlx::query(
            r#"
            INSERT INTO attendance_direct_import_events (
                org_id, run_id, import_row_id, employee_id, branch_id,
                source_sheet, source_row, source_key, source_sha256, fact_key,
                employee_number, employee_name, branch_name, work_date,
                check_in_at, check_out_at, minutes_worked
            )
            VALUES ($1, $2, $3, $4, $5, 'CSV', 99, 'sheet:CSV|row:99', $6, $7,
                    NULL, '홍길동', '본사', '2026-07-02', '09:00', '18:00', 540)
            "#,
        )
        .bind(org_id)
        .bind(run_id)
        .bind(duplicate_row_id)
        .bind(employee_id)
        .bind(branch_id)
        .bind(&source_sha256)
        .bind(&existing_fact_key)
        .execute(&pool)
        .await
        .map_err(|err| format!("seed existing attendance event failed: {err}"))?;

        let mut tx = pool
            .begin()
            .await
            .map_err(|err| format!("begin dry-run transaction failed: {err}"))?;
        let run = import_run_for_update(&mut tx, org_id, run_id)
            .await
            .map_err(|err| format!("load import run failed: {err:?}"))?;
        ensure_attendance_import_run(&run, &["DRY_RUN"])
            .map_err(|err| format!("status gate failed: {err:?}"))?;
        let rows = load_attendance_import_rows(&mut tx, org_id, run_id)
            .await
            .map_err(|err| format!("load import rows failed: {err:?}"))?;
        let summary = resolve_attendance_import_rows(&mut tx, org_id, run_id, &run, &rows)
            .await
            .map_err(|err| format!("resolve import rows failed: {err:?}"))?;
        tx.rollback()
            .await
            .map_err(|err| format!("rollback dry-run transaction failed: {err}"))?;

        assert_eq!(summary.ready_rows, 1);
        assert_eq!(summary.duplicate_rows, 1);
        assert_eq!(summary.error_rows, 1);
        assert_eq!(summary.ready_rows_for_apply[0].employee_id, employee_id);
        assert_eq!(summary.ready_rows_for_apply[0].employee_number, None);
        assert!(
            summary
                .row_errors
                .iter()
                .any(|error| error.code == "duplicate_attendance_fact")
        );

        let update_result = sqlx::query(
            "UPDATE attendance_direct_import_events SET employee_name = '위조' WHERE org_id = $1 AND run_id = $2",
        )
        .bind(org_id)
        .bind(run_id)
        .execute(&pool)
        .await;
        assert!(
            update_result.is_err(),
            "append-only attendance import events must reject UPDATE"
        );

        let mut rls_tx = pool
            .begin()
            .await
            .map_err(|err| format!("begin rls transaction failed: {err}"))?;
        sqlx::query("SET LOCAL ROLE console_rt")
            .execute(rls_tx.as_mut())
            .await
            .map_err(|err| format!("set runtime role failed: {err}"))?;
        sqlx::query("SELECT set_config('app.current_org', $1, true)")
            .bind(org_id.to_string())
            .execute(rls_tx.as_mut())
            .await
            .map_err(|err| format!("set current org failed: {err}"))?;
        let visible_rows: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM attendance_direct_import_events")
                .fetch_one(rls_tx.as_mut())
                .await
                .map_err(|err| format!("runtime select failed: {err}"))?;
        sqlx::query("SELECT set_config('app.current_org', $1, true)")
            .bind(Uuid::new_v4().to_string())
            .execute(rls_tx.as_mut())
            .await
            .map_err(|err| format!("set other org failed: {err}"))?;
        let hidden_rows: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM attendance_direct_import_events")
                .fetch_one(rls_tx.as_mut())
                .await
                .map_err(|err| format!("runtime isolation select failed: {err}"))?;
        rls_tx
            .rollback()
            .await
            .map_err(|err| format!("rollback rls transaction failed: {err}"))?;

        assert_eq!(visible_rows, 1);
        assert_eq!(hidden_rows, 0);
        sqlx::query("UPDATE organizations SET status = 'ARCHIVED' WHERE id = $1")
            .bind(org_id)
            .execute(&pool)
            .await
            .map_err(|err| format!("archive organization failed: {err}"))?;
        let force_remove_result: String =
            sqlx::query_scalar("SELECT platform_force_remove_organization($1)")
                .bind(org_id)
                .fetch_one(&pool)
                .await
                .map_err(|err| format!("force-remove organization failed: {err}"))?;
        assert_eq!(force_remove_result, "removed");
        let remaining_import_rows: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM data_import_rows WHERE org_id = $1")
                .bind(org_id)
                .fetch_one(&pool)
                .await
                .map_err(|err| format!("count remaining import rows failed: {err}"))?;
        let remaining_events: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM attendance_direct_import_events WHERE org_id = $1",
        )
        .bind(org_id)
        .fetch_one(&pool)
        .await
        .map_err(|err| format!("count remaining attendance events failed: {err}"))?;
        let remaining_mailbox_rows: i64 = sqlx::query_scalar(
            r#"
            SELECT
                (SELECT COUNT(*) FROM mailbox_deliveries WHERE org_id = $1)
              + (SELECT COUNT(*) FROM mailbox_messages WHERE org_id = $1)
              + (SELECT COUNT(*) FROM mailbox_aliases WHERE org_id = $1)
              + (SELECT COUNT(*) FROM mailboxes WHERE org_id = $1)
              + (SELECT COUNT(*) FROM mailbox_domains WHERE org_id = $1)
            "#,
        )
        .bind(org_id)
        .fetch_one(&pool)
        .await
        .map_err(|err| format!("count remaining mailbox rows failed: {err}"))?;
        let remaining_lifecycle_events: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM employee_lifecycle_events WHERE org_id = $1")
                .bind(org_id)
                .fetch_one(&pool)
                .await
                .map_err(|err| format!("count remaining lifecycle events failed: {err}"))?;
        assert_eq!(remaining_import_rows, 0);
        assert_eq!(remaining_events, 0);
        assert_eq!(remaining_mailbox_rows, 0);
        assert_eq!(remaining_lifecycle_events, 0);
        Ok(())
    }

    #[cfg(not(feature = "test-postgres"))]
    #[test]
    fn attendance_import_keeps_invalid_minutes_as_row_error() -> Result<(), String> {
        let csv = "\
사번,성명,지점,근무일,근무분
E-001,홍길동,본사,2026-07-01,abc
";
        let parsed = parse_attendance_import_upload("attendance.csv", csv.as_bytes())
            .map_err(|err| format!("expected row-level minutes validation, got {err:?}"))?;

        assert_eq!(parsed.rows.len(), 1);
        assert_eq!(parsed.rows[0].row_status, ImportRowStatus::Error);
        assert!(
            parsed.rows[0]
                .validation_errors
                .contains(&"invalid_minutes_worked".to_owned())
        );
        Ok(())
    }

    #[cfg(not(feature = "test-postgres"))]
    #[test]
    fn attendance_import_rejects_unclosed_csv_quote() -> Result<(), String> {
        let err = match parse_csv_rows("\"unterminated") {
            Ok(rows) => return Err(format!("unclosed quote unexpectedly parsed as {rows:?}")),
            Err(err) => err,
        };
        assert_eq!(err.code, "workbook");
        Ok(())
    }

    #[cfg(not(feature = "test-postgres"))]
    #[test]
    fn governed_import_detects_schema_header_below_title_rows() -> Result<(), String> {
        let mut range = Range::new((0, 0), (2, 2));
        range.set_value((0, 0), Data::String("2026년 임직원 명부".to_owned()));
        range.set_value((1, 0), Data::String("직원번호".to_owned()));
        range.set_value((1, 1), Data::String("이름".to_owned()));
        range.set_value((1, 2), Data::String("법인".to_owned()));
        range.set_value((2, 0), Data::String("ALT-010".to_owned()));
        range.set_value((2, 1), Data::String("김표준".to_owned()));
        range.set_value((2, 2), Data::String("운영법인".to_owned()));

        let parsed = parse_employee_import_sheet("employees.xlsx", "원천", &range)
            .map_err(|err| format!("expected governed import sheet to parse, got {err:?}"))?;

        assert_eq!(parsed.rows.len(), 1);
        assert_eq!(parsed.rows[0].source_row, 3);
        assert_eq!(parsed.rows[0].company, "운영법인");
        assert_eq!(parsed.rows[0].source_metadata["header_row"], json!(2));
        assert_eq!(
            parsed.rows[0].source_metadata["company_source"],
            json!("mapped_column")
        );
        assert!(
            parsed
                .columns
                .iter()
                .any(|column| column.source_header == "직원번호"
                    && column.target.as_deref() == Some("employee_number")),
            "schema catalog must map moved/shuffled employee number columns",
        );
        Ok(())
    }

    #[cfg(not(feature = "test-postgres"))]
    #[test]
    fn canonical_employee_fields_extract_hr_safe_columns() {
        let raw = json!({
            "사번": "A-001",
            "부서명": "물류팀",
            "업무": "정비",
            "직책": "대리",
            "근무지": "인천센터",
            "근무지(주소)": "인천광역시",
            "입사일": "2024-01-02",
            "발생연차": "15.00",
            "사용연차": "7.50",
            "잔여연차": "7.50"
        });

        let canonical = canonical_employee_fields(&raw);

        assert_eq!(canonical.employee_number.as_deref(), Some("A-001"));
        assert_eq!(canonical.org_unit.as_deref(), Some("물류팀"));
        assert_eq!(canonical.position.as_deref(), Some("대리"));
        assert_eq!(canonical.worksite_name.as_deref(), Some("인천센터"));
        assert_eq!(canonical.employment_status, "ACTIVE");
        assert_eq!(canonical.leave_accrued.as_deref(), Some("15.000000"));
        assert_eq!(canonical.leave_used.as_deref(), Some("7.500000"));
        assert_eq!(canonical.leave_remaining.as_deref(), Some("7.500000"));
    }

    #[cfg(not(feature = "test-postgres"))]
    #[test]
    fn canonical_employee_fields_marks_exited_people_without_deleting_raw_data() {
        let raw = json!({
            "성명": "이퇴사",
            "퇴사일": "2026-01-31",
            "퇴직금 중간정산일": "2025-12-31"
        });

        let canonical = canonical_employee_fields(&raw);

        assert_eq!(canonical.exit_date.as_deref(), Some("2026-01-31"));
        assert_eq!(canonical.employment_status, "EXITED");
        assert_eq!(raw["퇴직금 중간정산일"], json!("2025-12-31"));
    }

    #[cfg(not(feature = "test-postgres"))]
    #[test]
    fn governed_import_preview_preserves_blank_name_rows_and_masks_sensitive_columns()
    -> Result<(), String> {
        let mut range = Range::new((0, 0), (2, 4));
        range.set_value((0, 0), Data::String("성명".to_owned()));
        range.set_value((0, 1), Data::String("근무지\n(주소)".to_owned()));
        range.set_value((0, 2), Data::String("계좌번호".to_owned()));
        range.set_value((0, 3), Data::String("퇴직금 중간정산일".to_owned()));
        range.set_value((0, 4), Data::String("메모".to_owned()));
        range.set_value((1, 0), Data::String("홍길동".to_owned()));
        range.set_value((1, 1), Data::String("서울".to_owned()));
        range.set_value((1, 2), Data::String("123-456".to_owned()));
        range.set_value((1, 3), Data::String("2025-12-31".to_owned()));
        range.set_value((1, 4), Data::String("현장 배치".to_owned()));
        range.set_value((2, 2), Data::String("빈 이름 원천 행".to_owned()));

        let parsed = parse_employee_import_sheet("employees.xlsx", "코스", &range)
            .map_err(|err| format!("expected governed import sheet to parse, got {err:?}"))?;

        assert_eq!(parsed.rows.len(), 2);
        assert_eq!(parsed.rows[0].row_status, ImportRowStatus::Candidate);
        assert_eq!(parsed.rows[1].row_status, ImportRowStatus::Preserved);
        assert_eq!(parsed.rows[0].raw_row["근무지(주소)"], json!("서울"));
        assert_eq!(parsed.rows[1].raw_row["계좌번호"], json!("빈 이름 원천 행"));

        let preview = masked_preview_values(&parsed.rows[0].raw_row, &parsed.columns);
        assert_eq!(preview["근무지(주소)"], json!("••••"));
        assert_eq!(preview["계좌번호"], json!("••••"));
        assert_eq!(preview["퇴직금중간정산일"], json!("••••"));
        assert_eq!(preview["메모"], json!("현장 배치"));
        Ok(())
    }

    #[cfg(not(feature = "test-postgres"))]
    #[test]
    fn governed_import_maps_shuffled_alias_headers_without_column_position_assumptions()
    -> Result<(), String> {
        let mut range = Range::new((0, 0), (2, 5));
        range.set_value((0, 0), Data::String("메모".to_owned()));
        range.set_value((0, 1), Data::String("직원번호".to_owned()));
        range.set_value((0, 2), Data::String("남은 연차".to_owned()));
        range.set_value((0, 3), Data::String("근무지 주소".to_owned()));
        range.set_value((0, 4), Data::String("이름".to_owned()));
        range.set_value((0, 5), Data::String("계열사".to_owned()));
        range.set_value((1, 0), Data::String("현장 배치".to_owned()));
        range.set_value((1, 1), Data::String("ALT-001".to_owned()));
        range.set_value((1, 2), Data::Float(7.5));
        range.set_value((1, 3), Data::String("서울".to_owned()));
        range.set_value((1, 4), Data::String("홍길동".to_owned()));
        range.set_value((1, 5), Data::String("코스".to_owned()));
        range.set_value((2, 0), Data::String("raw-only keep".to_owned()));

        let parsed = parse_employee_import_sheet("employees.xlsx", "원천시트", &range)
            .map_err(|err| format!("expected alias import sheet to parse, got {err:?}"))?;

        assert_eq!(parsed.rows.len(), 2);
        assert_eq!(parsed.rows[0].row_status, ImportRowStatus::Candidate);
        assert_eq!(parsed.rows[1].row_status, ImportRowStatus::Preserved);
        assert_eq!(parsed.rows[0].company, "코스");
        assert_eq!(parsed.rows[0].name.as_deref(), Some("홍길동"));
        let canonical = parsed.rows[0]
            .canonical
            .as_ref()
            .ok_or_else(|| "candidate row missing canonical fields".to_owned())?;
        assert_eq!(canonical.employee_number.as_deref(), Some("ALT-001"));
        assert_eq!(canonical.leave_remaining.as_deref(), Some("7.500000"));
        assert_eq!(canonical.worksite_address.as_deref(), Some("서울"));

        let preview = masked_preview_values(&parsed.rows[0].raw_row, &parsed.columns);
        assert_eq!(preview["근무지주소"], json!("••••"));
        assert_eq!(preview["남은연차"], json!(7.5));
        assert_eq!(preview["메모"], json!("현장 배치"));
        assert!(
            parsed
                .columns
                .iter()
                .any(|column| column.source_header == "직원번호"
                    && column.target.as_deref() == Some("employee_number")),
            "직원번호 must map to the canonical employee_number target",
        );
        assert!(
            parsed
                .columns
                .iter()
                .any(|column| column.source_header == "이름"
                    && column.target.as_deref() == Some("name")),
            "이름 must map to the canonical name target",
        );
        Ok(())
    }

    #[cfg(not(feature = "test-postgres"))]
    #[test]
    fn standardized_csv_neutralizes_spreadsheet_formulas() {
        assert_eq!(csv_field("=cmd|' /C calc'!A0"), "'=cmd|' /C calc'!A0");
        assert_eq!(csv_field("hello, world"), "\"hello, world\"");
    }

    #[cfg(not(feature = "test-postgres"))]
    #[test]
    fn employee_response_serializes_canonical_fields_without_import_provenance()
    -> Result<(), String> {
        let body = serde_json::to_value(EmployeeResponse {
            id: Uuid::nil(),
            company: "코스".to_owned(),
            name: "홍길동".to_owned(),
            employee_number: Some("A-001".to_owned()),
            org_unit: Some("물류팀".to_owned()),
            worksite_name: None,
            worksite: None,
            job: None,
            position: Some("대리".to_owned()),
            hire_date: Some("2024-01-02".to_owned()),
            exit_date: None,
            status: Some("ACTIVE".to_owned()),
            leave_accrued: None,
            leave_used: None,
            leave_remaining: None,
            home_branch_id: None,
            home_branch_name: None,
            home_branch_review_required: true,
            identity_resolution_strategy: "employee_number".to_owned(),
            identity_resolution_confidence: "high".to_owned(),
            identity_review_required: false,
            identity_name_only_merge: false,
            created_at: time::OffsetDateTime::UNIX_EPOCH,
            updated_at: time::OffsetDateTime::UNIX_EPOCH,
        })
        .map_err(|err| format!("employee response serialization failed: {err}"))?;

        for forbidden in [
            "raw_row",
            "source_metadata",
            "source_filename",
            "source_sheet",
            "source_row",
        ] {
            assert!(
                body.get(forbidden).is_none(),
                "public employee response must not expose {forbidden}",
            );
        }
        assert_eq!(
            body["identity_resolution_strategy"],
            json!("employee_number")
        );
        assert_eq!(body["identity_resolution_confidence"], json!("high"));
        assert_eq!(body["identity_review_required"], json!(false));
        assert_eq!(body["identity_name_only_merge"], json!(false));
        assert_eq!(body["home_branch_review_required"], json!(true));
        Ok(())
    }

    #[cfg(not(feature = "test-postgres"))]
    #[test]
    fn employee_identity_resolution_rejects_name_only_and_untrusted_confidence() {
        let metadata = json!({
            "identity_resolution": {
                "strategy": "source_row_fingerprint",
                "confidence": "high",
                "manual_review_required": true,
                "name_only_merge": true
            }
        });

        let identity = employee_identity_resolution_from_metadata(&metadata);

        assert_eq!(identity.strategy, "source_row_fingerprint");
        assert_eq!(identity.confidence, "low");
        assert!(identity.review_required);
        assert!(!identity.name_only_merge);
    }

    #[cfg(not(feature = "test-postgres"))]
    #[test]
    fn employee_identity_resolution_accepts_high_confidence_trusted_strategies() {
        let metadata = json!({
            "identity_resolution": {
                "strategy": "legal_identifier_hash",
                "manual_review_required": false
            }
        });

        let identity = employee_identity_resolution_from_metadata(&metadata);

        assert_eq!(identity.strategy, "legal_identifier_hash");
        assert_eq!(identity.confidence, "high");
        assert!(!identity.review_required);
        assert!(!identity.name_only_merge);
    }

    #[cfg(not(feature = "test-postgres"))]
    #[test]
    fn employee_identity_resolution_keeps_weak_strategies_review_required() {
        let metadata = json!({
            "identity_resolution": {
                "strategy": "birth_hire_fingerprint",
                "manual_review_required": false
            }
        });

        let identity = employee_identity_resolution_from_metadata(&metadata);

        assert_eq!(identity.strategy, "birth_hire_fingerprint");
        assert_eq!(identity.confidence, "medium");
        assert!(identity.review_required);
        assert!(!identity.name_only_merge);
    }

    #[cfg(not(feature = "test-postgres"))]
    #[test]
    fn org_wide_hr_authorization_rejects_branch_scoped_principals() -> Result<(), String> {
        use console_kernel_core::{BranchId, OrgId, UserId};
        use console_platform_authz::Role;
        use std::collections::BTreeSet;

        let principal = Principal::new(
            UserId::new(),
            OrgId::new(),
            BTreeSet::from([Role::Admin]),
            BranchScope::single(BranchId::new()),
        );

        let err = match authorize_hr_org_wide(&principal, Feature::EmployeeDirectoryRead) {
            Ok(()) => {
                return Err(
                    "branch-scoped HR read authorized an org-wide employee surface".to_owned(),
                );
            }
            Err(err) => err,
        };
        assert_eq!(err.status, StatusCode::FORBIDDEN);
        Ok(())
    }

    #[cfg(not(feature = "test-postgres"))]
    #[test]
    fn org_wide_hr_authorization_uses_core_org_wide_gate() -> Result<(), String> {
        use console_kernel_core::{OrgId, UserId};
        use console_platform_authz::Role;
        use std::collections::BTreeSet;

        let admin = Principal::new(
            UserId::new(),
            OrgId::new(),
            BTreeSet::from([Role::Admin]),
            BranchScope::All,
        );

        let admin_err = match authorize_hr_org_wide(&admin, Feature::EmployeeDirectoryRead) {
            Ok(()) => {
                return Err(
                    "synthetic all-branch ADMIN authorized an org-wide employee surface".to_owned(),
                );
            }
            Err(err) => err,
        };
        assert_eq!(admin_err.status, StatusCode::FORBIDDEN);

        let executive = Principal::new(
            UserId::new(),
            OrgId::new(),
            BTreeSet::from([Role::Executive]),
            BranchScope::All,
        );

        authorize_hr_org_wide(&executive, Feature::EmployeeDirectoryRead)
            .map_err(|err| format!("org-wide executive HR read was rejected: {}", err.message))?;
        Ok(())
    }

    #[cfg(not(feature = "test-postgres"))]
    #[test]
    fn scoped_hr_authorization_checks_the_complete_scope_without_picking_a_branch()
    -> Result<(), String> {
        use console_kernel_core::{BranchId, OrgId, UserId};
        use console_platform_authz::{EffectiveFeatureGrant, PermissionLevel, Role};
        use std::collections::BTreeSet;

        let branch_a = BranchId::new();
        let branch_b = BranchId::new();
        let scope = BranchScope::Branches(BTreeSet::from([branch_a, branch_b]));

        let admin = Principal::new(
            UserId::new(),
            OrgId::new(),
            BTreeSet::from([Role::Admin]),
            scope.clone(),
        );
        authorize_hr_scoped(&admin, Feature::EmployeeDirectoryRead)
            .map_err(|err| format!("branch-scoped ADMIN was rejected: {}", err.message))?;
        authorize_hr_scoped_write(&admin, Feature::ExitCaseReport, Some(*branch_a.as_uuid()))
            .map_err(|err| format!("in-branch exit write was rejected: {}", err.message))?;
        assert!(
            authorize_hr_scoped_write(
                &admin,
                Feature::ExitCaseReport,
                Some(*BranchId::new().as_uuid()),
            )
            .is_err(),
            "a concrete foreign-branch write must be denied"
        );
        assert!(
            authorize_hr_scoped_write(&admin, Feature::ExitCaseReport, None).is_err(),
            "a branchless write must require org-wide authority"
        );

        let empty = Principal::new(
            UserId::new(),
            OrgId::new(),
            BTreeSet::from([Role::Admin]),
            BranchScope::none(),
        );
        assert!(
            authorize_hr_scoped(&empty, Feature::EmployeeDirectoryRead).is_err(),
            "an empty branch scope must fail closed"
        );

        let partial_grant =
            Principal::new(UserId::new(), OrgId::new(), BTreeSet::new(), scope.clone())
                .with_effective_feature_grants(vec![EffectiveFeatureGrant::new(
                    Feature::EmployeeDirectoryRead,
                    PermissionLevel::Allow,
                    BranchScope::single(branch_a),
                )]);
        assert!(
            authorize_hr_scoped(&partial_grant, Feature::EmployeeDirectoryRead).is_err(),
            "a grant over only one of two visible branches must not authorize the whole read"
        );

        let covering_grant =
            Principal::new(UserId::new(), OrgId::new(), BTreeSet::new(), scope.clone())
                .with_effective_feature_grants(vec![EffectiveFeatureGrant::new(
                    Feature::EmployeeDirectoryRead,
                    PermissionLevel::Allow,
                    scope,
                )]);
        authorize_hr_scoped(&covering_grant, Feature::EmployeeDirectoryRead)
            .map_err(|err| format!("covering HR grant was rejected: {}", err.message))?;

        let all_admin = Principal::new(
            UserId::new(),
            OrgId::new(),
            BTreeSet::from([Role::Admin]),
            BranchScope::All,
        );
        assert!(
            authorize_hr_scoped(&all_admin, Feature::EmployeeDirectoryRead).is_err(),
            "the existing all-scope ADMIN denial must be preserved"
        );

        let super_admin = Principal::new(
            UserId::new(),
            OrgId::new(),
            BTreeSet::from([Role::SuperAdmin]),
            BranchScope::All,
        );
        authorize_hr_scoped_write(&super_admin, Feature::ExitCaseReport, None).map_err(|err| {
            format!(
                "org-wide branchless exit write was rejected: {}",
                err.message
            )
        })?;
        Ok(())
    }

    #[cfg(not(feature = "test-postgres"))]
    #[test]
    fn home_branch_assignment_requires_org_wide_authority_when_unassigned() -> Result<(), String> {
        use console_kernel_core::{OrgId, UserId};
        use console_platform_authz::Role;
        use std::collections::BTreeSet;

        let branch = BranchId::new();
        let branch_admin = Principal::new(
            UserId::new(),
            OrgId::new(),
            BTreeSet::from([Role::Admin]),
            BranchScope::single(branch),
        );
        let err = authorize_home_branch_transition(&branch_admin, None, *branch.as_uuid())
            .expect_err("branch admin must not claim an unassigned employee");
        assert_eq!(err.status, StatusCode::FORBIDDEN);

        let super_admin = Principal::new(
            UserId::new(),
            OrgId::new(),
            BTreeSet::from([Role::SuperAdmin]),
            BranchScope::All,
        );
        authorize_home_branch_transition(&super_admin, None, *branch.as_uuid())
            .map_err(|err| err.message)?;
        Ok(())
    }

    #[cfg(not(feature = "test-postgres"))]
    #[tokio::test]
    async fn home_branch_assignment_fails_closed_without_command_pool() -> Result<(), String> {
        use console_kernel_core::{OrgId, UserId};
        use console_platform_authz::Role;
        use sqlx::postgres::PgPoolOptions;
        use std::collections::BTreeSet;

        let branch = BranchId::new();
        let principal = Principal::new(
            UserId::new(),
            OrgId::new(),
            BTreeSet::from([Role::SuperAdmin]),
            BranchScope::All,
        );
        let pool = PgPoolOptions::new()
            .connect_lazy("postgresql://unused:unused@127.0.0.1/unused")
            .map_err(|err| err.to_string())?;

        let err = set_employee_home_branch(
            State(HrState::new(pool, None)),
            Extension(principal),
            Path(Uuid::new_v4()),
            Json(SetEmployeeHomeBranchRequest {
                branch_id: *branch.as_uuid(),
                expected_updated_at: "2026-07-19T12:00:00Z".to_owned(),
            }),
        )
        .await
        .expect_err("missing leave command database must fail closed");

        assert_eq!(err.status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(err.code, "service_unavailable");
        Ok(())
    }

    #[cfg(not(feature = "test-postgres"))]
    #[test]
    fn home_branch_reassignment_requires_authority_over_old_and_new_branch() {
        use console_kernel_core::{OrgId, UserId};
        use console_platform_authz::Role;
        use std::collections::BTreeSet;

        let old_branch = BranchId::new();
        let new_branch = BranchId::new();
        let new_branch_only = Principal::new(
            UserId::new(),
            OrgId::new(),
            BTreeSet::from([Role::Admin]),
            BranchScope::single(new_branch),
        );
        assert_eq!(
            authorize_home_branch_transition(
                &new_branch_only,
                Some(*old_branch.as_uuid()),
                *new_branch.as_uuid(),
            )
            .expect_err("new-branch authority alone must not move an employee")
            .status,
            StatusCode::FORBIDDEN,
        );

        let both = Principal::new(
            UserId::new(),
            OrgId::new(),
            BTreeSet::from([Role::Admin]),
            BranchScope::Branches(BTreeSet::from([old_branch, new_branch])),
        );
        assert!(
            authorize_home_branch_transition(
                &both,
                Some(*old_branch.as_uuid()),
                *new_branch.as_uuid(),
            )
            .is_ok()
        );
    }

    #[cfg(feature = "test-postgres")]
    #[sqlx::test(migrations = "../crates/platform/db/migrations")]
    async fn employee_import_batch_rolls_back_mid_batch_then_retries_atomically(
        pool: sqlx::PgPool,
    ) -> Result<(), String> {
        use console_kernel_core::{OrgId, UserId};

        let org_id = Uuid::new_v4();
        let actor_id = Uuid::new_v4();
        sqlx::query("INSERT INTO organizations (id,slug,name) VALUES ($1,$2,'Import audit test')")
            .bind(org_id)
            .bind(format!("import-audit-{}", &org_id.to_string()[..8]))
            .execute(&pool)
            .await
            .map_err(|error| error.to_string())?;
        sqlx::query(
            "INSERT INTO users (id,display_name,roles,is_active,org_id) \
             VALUES ($1,'HR Owner',ARRAY['SUPER_ADMIN']::text[],true,$2)",
        )
        .bind(actor_id)
        .bind(org_id)
        .execute(&pool)
        .await
        .map_err(|error| error.to_string())?;
        let run_id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO data_import_runs (id,org_id,entity_type,status,source_filename,\
             source_format,source_sha256,mapping_profile,input_rows,candidate_rows,preserved_rows,\
             pay_period_start,pay_period_end) \
             VALUES ($1,$2,'employee_hr','DRY_RUN','employees.xlsx','xlsx',$3,'{}',2,2,0,\
                     DATE '2026-06-01', DATE '2026-06-30')",
        )
        .bind(run_id)
        .bind(org_id)
        .bind("a".repeat(64))
        .execute(&pool)
        .await
        .map_err(|error| error.to_string())?;
        let source_keys = [
            format!("employee-{}", Uuid::new_v4()),
            format!("employee-{}", Uuid::new_v4()),
        ];
        let imported_row = |index: usize| ParsedEmployeeRow {
            company: "KNL".to_owned(),
            name: format!("Imported Employee {index}"),
            source_filename: "employees.xlsx".to_owned(),
            source_sheet: "Sheet1".to_owned(),
            source_row: index as i32 + 2,
            source_key: source_keys[index].clone(),
            raw_row: json!({"name":format!("Imported Employee {index}")}),
            source_metadata: json!({}),
            canonical: EmployeeCanonicalFields {
                employment_status: "ACTIVE".to_owned(),
                leave_accrued: Some("12.000001".to_owned()),
                leave_used: Some("1.125000".to_owned()),
                leave_remaining: Some("10.875001".to_owned()),
                ..EmployeeCanonicalFields::default()
            },
        };
        let valid_rows = vec![imported_row(0), imported_row(1)];
        for row in &valid_rows {
            let canonical_row = json!({
                "company": &row.company,
                "name": &row.name,
                "source_filename": &row.source_filename,
                "source_sheet": &row.source_sheet,
                "source_row": row.source_row,
                "source_key": &row.source_key,
                "source_metadata": &row.source_metadata,
                "canonical": &row.canonical
            });
            sqlx::query(
                "INSERT INTO data_import_rows (org_id,run_id,source_sheet,source_row,\
                 source_key,row_status,raw_row,canonical_row,validation) \
                 VALUES ($1,$2,$3,$4,$5,'CANDIDATE',$6,$7,'{}')",
            )
            .bind(org_id)
            .bind(run_id)
            .bind(&row.source_sheet)
            .bind(row.source_row)
            .bind(&row.source_key)
            .bind(&row.raw_row)
            .bind(canonical_row)
            .execute(&pool)
            .await
            .map_err(|error| error.to_string())?;
        }
        sqlx::raw_sql(
            r#"
            CREATE FUNCTION reject_second_employee_import_for_atomicity_test()
            RETURNS TRIGGER LANGUAGE plpgsql AS $$
            BEGIN
                IF NEW.name = 'Imported Employee 1' THEN
                    RAISE EXCEPTION USING ERRCODE='23514',
                        MESSAGE='employee_import_batch.test_second_row_failure';
                END IF;
                RETURN NEW;
            END
            $$;
            CREATE TRIGGER trg_reject_second_employee_import_for_atomicity_test
                BEFORE INSERT OR UPDATE ON employees
                FOR EACH ROW EXECUTE FUNCTION reject_second_employee_import_for_atomicity_test();
            "#,
        )
        .execute(&pool)
        .await
        .map_err(|error| error.to_string())?;
        let command_store = PgLeaveStore::new(
            pool.clone(),
            std::sync::Arc::new(console_inbox_adapter_postgres::PgInboxStore::new(
                pool.clone(),
            )),
        )
        .with_leave_command_pool(pool.clone());
        let state = HrState::new(pool.clone(), None).with_leave_command_store(command_store);
        let org = OrgId::from_uuid(org_id);
        let actor = UserId::from_uuid(actor_id);
        let source_ref = format!("run:{run_id}");

        apply_employee_import_batch(
            &state,
            org,
            actor,
            Some(run_id),
            source_ref.clone(),
            valid_rows.clone(),
            json!({"gate_outcome":"test"}),
        )
        .await
        .expect_err("the deterministic second-row constraint failure must abort the batch");

        let after_failure: (i64, i64, i64, String) = sqlx::query_as(
            "SELECT (SELECT count(*) FROM employees WHERE org_id=$1),\
             (SELECT count(*) FROM leave_balance_import_receipts WHERE org_id=$1),\
             (SELECT count(*) FROM audit_events WHERE org_id=$1),\
             (SELECT status FROM data_import_runs WHERE org_id=$1 AND id=$2)",
        )
        .bind(org_id)
        .bind(run_id)
        .fetch_one(&pool)
        .await
        .map_err(|error| error.to_string())?;
        assert_eq!(after_failure, (0, 0, 0, "DRY_RUN".to_owned()));

        sqlx::raw_sql(
            r#"
            DROP TRIGGER trg_reject_second_employee_import_for_atomicity_test ON employees;
            DROP FUNCTION reject_second_employee_import_for_atomicity_test();
            "#,
        )
        .execute(&pool)
        .await
        .map_err(|error| error.to_string())?;
        let first_apply = apply_employee_import_batch(
            &state,
            org,
            actor,
            Some(run_id),
            source_ref.clone(),
            valid_rows.clone(),
            json!({"gate_outcome":"test"}),
        );
        let concurrent_apply = apply_employee_import_batch(
            &state,
            org,
            actor,
            Some(run_id),
            source_ref.clone(),
            valid_rows,
            json!({"gate_outcome":"test"}),
        );
        let (first_result, concurrent_result) = tokio::join!(first_apply, concurrent_apply);
        let first_report = first_result.map_err(|error| error.message)?;
        let concurrent_report = concurrent_result.map_err(|error| error.message)?;
        let mut outcomes = [
            (
                first_report.inserted,
                first_report.updated,
                first_report.skipped,
            ),
            (
                concurrent_report.inserted,
                concurrent_report.updated,
                concurrent_report.skipped,
            ),
        ];
        outcomes.sort_unstable();
        assert_eq!(outcomes, [(0, 0, 2), (2, 0, 0)]);
        let timestamps_before_replay: Vec<OffsetDateTime> = sqlx::query_scalar(
            "SELECT updated_at FROM employees WHERE org_id=$1 ORDER BY source_key",
        )
        .bind(org_id)
        .fetch_all(&pool)
        .await
        .map_err(|error| error.to_string())?;
        let replay = apply_employee_import_batch(
            &state,
            org,
            actor,
            Some(run_id),
            source_ref,
            Vec::new(),
            json!({"gate_outcome":"test"}),
        )
        .await
        .map_err(|error| error.message)?;
        assert_eq!((replay.inserted, replay.updated, replay.skipped), (0, 0, 2));

        let stored: Vec<(String, String, String, OffsetDateTime)> = sqlx::query_as(
            "SELECT leave_accrued::text,leave_used::text,leave_remaining::text,updated_at \
             FROM employees WHERE org_id=$1 ORDER BY source_key",
        )
        .bind(org_id)
        .fetch_all(&pool)
        .await
        .map_err(|error| error.to_string())?;
        assert_eq!(stored.len(), 2);
        for (index, row) in stored.iter().enumerate() {
            assert_eq!(
                row,
                &(
                    "12.000001".to_owned(),
                    "1.125000".to_owned(),
                    "10.875001".to_owned(),
                    timestamps_before_replay[index],
                )
            );
        }
        let evidence: (i64, i64, i64, String) = sqlx::query_as(
            "SELECT (SELECT count(*) FROM leave_balance_import_receipts WHERE org_id=$1),\
             (SELECT count(*) FROM audit_events WHERE org_id=$1 \
                AND action='employee.leave_balance_import'),\
             (SELECT count(*) FROM audit_events WHERE org_id=$1 \
                AND action='data_import.apply' AND target_id=$2::text),\
             (SELECT status FROM data_import_runs WHERE org_id=$1 AND id=$2)",
        )
        .bind(org_id)
        .bind(run_id)
        .fetch_one(&pool)
        .await
        .map_err(|error| error.to_string())?;
        assert_eq!(evidence, (2, 2, 1, "APPLIED".to_owned()));
        Ok(())
    }

    #[cfg(feature = "test-postgres")]
    #[sqlx::test(migrations = "../crates/platform/db/migrations")]
    async fn legacy_employee_import_audits_roster_only_apply_and_skips_exact_replay(
        pool: sqlx::PgPool,
    ) -> Result<(), String> {
        use console_kernel_core::{OrgId, UserId};

        let org_id = Uuid::new_v4();
        let actor_id = Uuid::new_v4();
        let source_key = format!("legacy-{}", Uuid::new_v4());
        let source_ref = "b".repeat(64);
        sqlx::query("INSERT INTO organizations (id,slug,name) VALUES ($1,$2,'Legacy import test')")
            .bind(org_id)
            .bind(format!("legacy-import-{}", &org_id.to_string()[..8]))
            .execute(&pool)
            .await
            .map_err(|error| error.to_string())?;
        sqlx::query(
            "INSERT INTO users (id,display_name,roles,is_active,org_id) \
             VALUES ($1,'Legacy importer',ARRAY['SUPER_ADMIN']::text[],true,$2)",
        )
        .bind(actor_id)
        .bind(org_id)
        .execute(&pool)
        .await
        .map_err(|error| error.to_string())?;
        let row = ParsedEmployeeRow {
            company: "KNL".to_owned(),
            name: "Roster Only Employee".to_owned(),
            source_filename: "legacy.xlsx".to_owned(),
            source_sheet: "Sheet1".to_owned(),
            source_row: 2,
            source_key: source_key.clone(),
            raw_row: json!({"name":"Roster Only Employee"}),
            source_metadata: json!({}),
            canonical: EmployeeCanonicalFields {
                employment_status: "ACTIVE".to_owned(),
                ..EmployeeCanonicalFields::default()
            },
        };
        let command_store = PgLeaveStore::new(
            pool.clone(),
            std::sync::Arc::new(console_inbox_adapter_postgres::PgInboxStore::new(
                pool.clone(),
            )),
        )
        .with_leave_command_pool(pool.clone());
        let state = HrState::new(pool.clone(), None).with_leave_command_store(command_store);
        let org = OrgId::from_uuid(org_id);
        let actor = UserId::from_uuid(actor_id);

        let first = apply_employee_import_batch(
            &state,
            org,
            actor,
            None,
            source_ref.clone(),
            vec![row.clone()],
            json!({}),
        )
        .await
        .map_err(|error| error.message)?;
        assert_eq!((first.inserted, first.updated, first.skipped), (1, 0, 0));
        let first_updated_at: OffsetDateTime = sqlx::query_scalar(
            "SELECT updated_at FROM employees WHERE org_id=$1 AND source_key=$2",
        )
        .bind(org_id)
        .bind(&source_key)
        .fetch_one(&pool)
        .await
        .map_err(|error| error.to_string())?;

        let replay = apply_employee_import_batch(
            &state,
            org,
            actor,
            None,
            source_ref.clone(),
            vec![row],
            json!({}),
        )
        .await
        .map_err(|error| error.message)?;
        assert_eq!((replay.inserted, replay.updated, replay.skipped), (0, 0, 1));
        let evidence: (OffsetDateTime, i64, i64) = sqlx::query_as(
            "SELECT updated_at,\
             (SELECT count(*) FROM leave_balance_import_receipts \
                WHERE org_id=$1 AND source_ref=$3),\
             (SELECT count(*) FROM audit_events WHERE org_id=$1 \
                AND action='data_import.apply' AND target_type='employee_import_batch' \
                AND target_id=$3) \
             FROM employees WHERE org_id=$1 AND source_key=$2",
        )
        .bind(org_id)
        .bind(&source_key)
        .bind(&source_ref)
        .fetch_one(&pool)
        .await
        .map_err(|error| error.to_string())?;
        assert_eq!(evidence, (first_updated_at, 1, 1));
        Ok(())
    }

    #[cfg(feature = "test-postgres")]
    #[sqlx::test(migrations = "../crates/platform/db/migrations")]
    async fn home_branch_assignment_is_explicit_versioned_and_audited(
        pool: sqlx::PgPool,
    ) -> Result<(), String> {
        use console_kernel_core::{OrgId, UserId};
        use console_platform_authz::Role;
        use std::collections::BTreeSet;

        let org_id = Uuid::new_v4();
        let region_id = Uuid::new_v4();
        let branch_id = Uuid::new_v4();
        let inactive_branch_id = Uuid::new_v4();
        let employee_id = Uuid::new_v4();
        let actor_id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO organizations (id, slug, name) VALUES ($1, $2, 'Home Branch Test')",
        )
        .bind(org_id)
        .bind(format!("home-branch-{}", &org_id.to_string()[..8]))
        .execute(&pool)
        .await
        .map_err(|err| err.to_string())?;
        sqlx::query("INSERT INTO regions (id, name, org_id) VALUES ($1, $2, $3)")
            .bind(region_id)
            .bind(format!("Home Branch Region {region_id}"))
            .bind(org_id)
            .execute(&pool)
            .await
            .map_err(|err| err.to_string())?;
        sqlx::query(
            "INSERT INTO branches (id, region_id, name, org_id, deactivated_at) VALUES ($1, $2, 'Active', $3, NULL), ($4, $2, 'Inactive', $3, now())",
        )
        .bind(branch_id)
        .bind(region_id)
        .bind(org_id)
        .bind(inactive_branch_id)
        .execute(&pool)
        .await
        .map_err(|err| err.to_string())?;
        sqlx::query(
            "INSERT INTO users (id, display_name, roles, is_active, org_id) VALUES ($1, 'HR Owner', ARRAY['SUPER_ADMIN']::TEXT[], true, $2)",
        )
        .bind(actor_id)
        .bind(org_id)
        .execute(&pool)
        .await
        .map_err(|err| err.to_string())?;
        sqlx::query(
            "INSERT INTO employees (id, org_id, company, name, source_filename, source_sheet, source_row, source_key, raw_row, source_metadata) VALUES ($1, $2, 'Test', 'Employee', 'test.xlsx', 'Sheet', 1, 'home-branch-employee', '{}', '{}')",
        )
        .bind(employee_id)
        .bind(org_id)
        .execute(&pool)
        .await
        .map_err(|err| err.to_string())?;
        let expected_updated_at: OffsetDateTime =
            sqlx::query_scalar("SELECT updated_at FROM employees WHERE id = $1")
                .bind(employee_id)
                .fetch_one(&pool)
                .await
                .map_err(|err| err.to_string())?;
        let principal = Principal::new(
            UserId::from_uuid(actor_id),
            OrgId::from_uuid(org_id),
            BTreeSet::from([Role::SuperAdmin]),
            BranchScope::All,
        );
        let command_store = PgLeaveStore::new(
            pool.clone(),
            std::sync::Arc::new(console_inbox_adapter_postgres::PgInboxStore::new(
                pool.clone(),
            )),
        )
        .with_leave_command_pool(pool.clone());
        let response = set_employee_home_branch(
            State(HrState::new(pool.clone(), None).with_leave_command_store(command_store.clone())),
            Extension(principal.clone()),
            Path(employee_id),
            Json(SetEmployeeHomeBranchRequest {
                branch_id,
                expected_updated_at: expected_updated_at
                    .format(&time::format_description::well_known::Rfc3339)
                    .map_err(|err| err.to_string())?,
            }),
        )
        .await
        .map_err(|err| err.message)?;
        assert_eq!(response.0.branch_id, branch_id);
        let stored: Option<Uuid> =
            sqlx::query_scalar("SELECT home_branch_id FROM employees WHERE id = $1")
                .bind(employee_id)
                .fetch_one(&pool)
                .await
                .map_err(|err| err.to_string())?;
        assert_eq!(stored, Some(branch_id));
        let audits: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM audit_events WHERE org_id = $1 AND target_id = $2 AND action = 'employee.home_branch_set' AND actor = $3 AND before_snap->>'home_branch_id' IS NULL AND after_snap->>'home_branch_id' = $4",
        )
        .bind(org_id)
        .bind(employee_id.to_string())
        .bind(actor_id)
        .bind(branch_id.to_string())
        .fetch_one(&pool)
        .await
        .map_err(|err| err.to_string())?;
        assert_eq!(audits, 1);

        let inactive_err = set_employee_home_branch(
            State(HrState::new(pool.clone(), None).with_leave_command_store(command_store)),
            Extension(principal),
            Path(employee_id),
            Json(SetEmployeeHomeBranchRequest {
                branch_id: inactive_branch_id,
                expected_updated_at: response
                    .0
                    .updated_at
                    .format(&time::format_description::well_known::Rfc3339)
                    .map_err(|err| err.to_string())?,
            }),
        )
        .await
        .expect_err("deactivated branch must not become authoritative");
        assert_eq!(inactive_err.status, StatusCode::NOT_FOUND);
        Ok(())
    }
    #[cfg(not(feature = "test-postgres"))]
    #[test]
    fn employee_attendance_state_machine_accepts_mobile_pc_workday_flow() -> Result<(), String> {
        assert_eq!(
            next_employee_attendance_state(None, "CLOCK_IN").map_err(|err| err.message.clone())?,
            "CLOCKED_IN"
        );
        assert_eq!(
            next_employee_attendance_state(Some("CLOCKED_IN"), "OUT_FOR_WORK")
                .map_err(|err| err.message.clone())?,
            "OUT_FOR_WORK"
        );
        assert_eq!(
            next_employee_attendance_state(Some("OUT_FOR_WORK"), "RETURNED")
                .map_err(|err| err.message.clone())?,
            "CLOCKED_IN"
        );
        assert_eq!(
            next_employee_attendance_state(Some("CLOCKED_IN"), "BUSINESS_TRIP")
                .map_err(|err| err.message.clone())?,
            "BUSINESS_TRIP"
        );
        assert_eq!(
            next_employee_attendance_state(Some("BUSINESS_TRIP"), "CLOCK_OUT")
                .map_err(|err| err.message.clone())?,
            "OFF_DUTY"
        );
        Ok(())
    }

    #[cfg(not(feature = "test-postgres"))]
    #[test]
    fn employee_attendance_state_machine_rejects_invalid_duplicate_punches() -> Result<(), String> {
        let err = match next_employee_attendance_state(None, "CLOCK_OUT") {
            Ok(state) => return Err(format!("clock-out before clock-in returned {state}")),
            Err(err) => err,
        };
        assert_eq!(err.status, StatusCode::CONFLICT);
        assert_eq!(err.code, "invalid_transition");

        let err = match next_employee_attendance_state(Some("CLOCKED_IN"), "CLOCK_IN") {
            Ok(state) => return Err(format!("duplicate clock-in returned {state}")),
            Err(err) => err,
        };
        assert_eq!(err.status, StatusCode::CONFLICT);
        assert_eq!(err.code, "invalid_transition");
        Ok(())
    }

    #[cfg(not(feature = "test-postgres"))]
    #[test]
    fn attendance_input_normalization_bounds_mobile_retry_fields() {
        let kind = normalize_attendance_kind(" business_trip ")
            .map_err(|err| err.message)
            .unwrap_or("invalid");
        assert_eq!(kind, "BUSINESS_TRIP");

        let idempotency_key = normalize_idempotency_key(" retry-1 ".to_owned())
            .map_err(|err| err.message)
            .unwrap_or_default();
        assert_eq!(idempotency_key, "retry-1");
        assert!(
            normalize_idempotency_key("   ".to_owned()).is_err(),
            "blank idempotency keys must be rejected"
        );
        assert!(
            normalize_attendance_note(Some("x".repeat(501))).is_err(),
            "long attendance notes must be rejected before persistence"
        );
    }

    #[allow(dead_code)]
    fn sample_settlement_input() -> ExitSettlementInput {
        ExitSettlementInput {
            average_wage_period_start: "2026-04-01".to_owned(),
            average_wage_period_end: "2026-06-30".to_owned(),
            average_wage_calendar_days: 91,
            average_wage_total_won: 9_000_000,
            monthly_ordinary_wage_won: 3_000_000,
        }
    }

    #[allow(dead_code)]
    async fn arm_console_rt(
        tx: &mut sqlx::Transaction<'_, Postgres>,
        org_id: Uuid,
    ) -> Result<(), String> {
        sqlx::query("SET LOCAL ROLE console_rt")
            .execute(tx.as_mut())
            .await
            .map_err(|err| format!("set runtime role failed: {err}"))?;
        sqlx::query("SELECT set_config('app.current_org', $1, true)")
            .bind(org_id.to_string())
            .execute(tx.as_mut())
            .await
            .map_err(|err| format!("arm current org failed: {err}"))?;
        Ok(())
    }

    #[allow(dead_code)]
    async fn seed_exit_case(pool: &sqlx::PgPool) -> Result<(Uuid, Uuid), String> {
        let org_id = Uuid::new_v4();
        let region_id = Uuid::new_v4();
        let branch_id = Uuid::new_v4();
        let user_id = Uuid::new_v4();
        let employee_id = Uuid::new_v4();
        let case_id = Uuid::new_v4();
        sqlx::query("INSERT INTO organizations (id, slug, name) VALUES ($1, $2, $3)")
            .bind(org_id)
            .bind(format!("exit-{}", &org_id.to_string()[..8]))
            .bind("Exit Settlement Test")
            .execute(pool)
            .await
            .map_err(|err| format!("seed organization failed: {err}"))?;
        sqlx::query("INSERT INTO regions (id, name, org_id) VALUES ($1, $2, $3)")
            .bind(region_id)
            .bind("정산지역")
            .bind(org_id)
            .execute(pool)
            .await
            .map_err(|err| format!("seed region failed: {err}"))?;
        sqlx::query("INSERT INTO branches (id, region_id, name, org_id) VALUES ($1, $2, $3, $4)")
            .bind(branch_id)
            .bind(region_id)
            .bind("본사")
            .bind(org_id)
            .execute(pool)
            .await
            .map_err(|err| format!("seed branch failed: {err}"))?;
        sqlx::query(
            "INSERT INTO users (id, display_name, roles, is_active, org_id) VALUES ($1, $2, ARRAY['ADMIN']::TEXT[], true, $3)",
        )
        .bind(user_id)
        .bind("Exit HR Manager")
        .bind(org_id)
        .execute(pool)
        .await
        .map_err(|err| format!("seed user failed: {err}"))?;
        sqlx::query(
            r#"
            INSERT INTO employees (
                id, org_id, company, name, employee_number, hire_date,
                source_filename, source_sheet, source_row, source_key, raw_row, source_metadata
            )
            VALUES ($1, $2, '테스트', '홍길동', 'E-001', '2020-01-01',
                    'employees.xlsx', '직원', 2, 'employee-row-2', '{}', '{}')
            "#,
        )
        .bind(employee_id)
        .bind(org_id)
        .execute(pool)
        .await
        .map_err(|err| format!("seed employee failed: {err}"))?;
        sqlx::query(
            r#"
            INSERT INTO employee_exit_cases (
                id, org_id, employee_id, branch_id, status,
                effective_exit_date, site_manager_note, reported_by
            )
            VALUES ($1, $2, $3, $4, 'HR_CONFIRMED', '2026-06-30', '무단결근 확인', $5)
            "#,
        )
        .bind(case_id)
        .bind(org_id)
        .bind(employee_id)
        .bind(branch_id)
        .bind(user_id)
        .execute(pool)
        .await
        .map_err(|err| format!("seed exit case failed: {err}"))?;
        Ok((org_id, case_id))
    }

    #[cfg(not(feature = "test-postgres"))]
    #[test]
    fn certified_package_digest_is_deterministic_and_canonical() {
        let statutory = json!({"formula": "avg*30*days/365", "authority": "MOEL"});
        let insurance = json!({"forms": ["np", "hi"]});
        let approval = json!({"document_type": "employee_exit_settlement"});
        let digest = |severance: Option<i64>, basis: &Value| {
            compute_certified_package_digest(
                severance,
                basis,
                &insurance,
                &approval,
                Some("2026-04-01"),
                Some("2026-06-30"),
                Some(91),
                Some(9_000_000),
                Some(98_901),
                Some(2373),
                Some(3_000_000),
                Some(114_832),
                Some(98_901),
            )
        };
        let d1 = digest(Some(30_000_000), &statutory);
        let d2 = digest(Some(30_000_000), &statutory);
        assert_eq!(d1, d2, "identical inputs must hash identically");
        assert_eq!(d1.len(), 64, "digest is a 64-hex SHA-256");
        assert!(
            d1.chars().all(|c| c.is_ascii_hexdigit()),
            "digest must be lowercase hex"
        );

        let changed = digest(Some(30_000_001), &statutory);
        assert_ne!(
            d1, changed,
            "a changed severance figure must change the digest"
        );

        // Embedded-JSON key order must not affect the digest (canonical form).
        let mut reordered = serde_json::Map::new();
        reordered.insert("authority".to_owned(), json!("MOEL"));
        reordered.insert("formula".to_owned(), json!("avg*30*days/365"));
        let reordered = Value::Object(reordered);
        assert_eq!(
            d1,
            digest(Some(30_000_000), &reordered),
            "key order in embedded JSON must not change the digest"
        );
    }

    /// 0093 review HIGH #2: the certified digest must bind the 통상임금 basis
    /// DIRECTLY, not merely transitively through `severance_pay_won`. Changing
    /// ONLY the monthly ordinary wage (holding severance and every other covered
    /// field byte-identical — the case where the average wage governs the floor)
    /// must still change the digest.
    #[cfg(not(feature = "test-postgres"))]
    #[test]
    fn certified_digest_binds_ordinary_wage_even_when_severance_unchanged() {
        let statutory = json!({"formula": "avg*30*days/365"});
        let insurance = json!({"forms": ["np", "hi"]});
        let approval = json!({"document_type": "employee_exit_settlement"});
        let digest = |monthly: Option<i64>| {
            compute_certified_package_digest(
                // Severance + every non-ordinary covered field held CONSTANT.
                Some(30_000_000),
                &statutory,
                &insurance,
                &approval,
                Some("2026-04-01"),
                Some("2026-06-30"),
                Some(91),
                Some(9_000_000),
                Some(98_901),
                Some(2373),
                monthly,
                Some(114_832),
                Some(98_901),
            )
        };
        assert_ne!(
            digest(Some(3_000_000)),
            digest(Some(3_100_000)),
            "changing the ordinary wage must change the digest even when severance is unchanged"
        );
    }

    /// 0093 review HIGH #3 (money bug): the 통상일급 derivation must multiply
    /// BEFORE dividing (`(monthly * 8) / 209`), never floor `monthly / 209` first.
    /// Flooring first understated the daily ordinary wage by up to 7 won/day and
    /// under-paid severance whenever the ordinary floor governed.
    #[cfg(not(feature = "test-postgres"))]
    #[test]
    fn ordinary_daily_wage_multiplies_before_dividing() -> Result<(), String> {
        // monthly % 209 != 0 and the remainder × 8 crosses 209, so the buggy
        // floor-first formula and the corrected formula actually diverge.
        let monthly = 3_000_100_i64;
        assert_ne!(
            monthly % MONTHLY_STANDARD_HOURS,
            0,
            "must exercise a remainder"
        );
        // Buggy floor-first: floor(3_000_100 / 209) * 8 = 14_354 * 8 = 114_832.
        let buggy_floor_first = monthly / MONTHLY_STANDARD_HOURS * DAILY_CONTRACTUAL_HOURS;
        assert_eq!(
            buggy_floor_first, 114_832,
            "buggy floor-first value for reference"
        );
        // Corrected multiply-before-divide: (3_000_100 * 8) / 209 = 114_836.
        let corrected = ordinary_daily_wage_won_from_monthly(monthly)
            .map_err(|err| format!("unexpected overflow: {}", err.message))?;
        assert_eq!(corrected, 114_836, "must be (monthly * 8) / 209");
        assert!(
            corrected > buggy_floor_first,
            "multiply-before-divide must not understate the daily wage (corrected {corrected} vs buggy {buggy_floor_first})"
        );
        Ok(())
    }

    #[cfg(feature = "test-postgres")]
    #[sqlx::test(migrations = "../crates/platform/db/migrations")]
    async fn settlement_recalculation_reverts_certification(
        pool: sqlx::PgPool,
    ) -> Result<(), String> {
        let (org_id, case_id) = seed_exit_case(&pool).await?;
        let mut tx = pool
            .begin()
            .await
            .map_err(|err| format!("begin failed: {err}"))?;
        arm_console_rt(&mut tx, org_id).await?;

        // Build the settlement package via the real code path, as console_rt.
        let package_id = upsert_exit_settlement_package(
            &mut tx,
            org_id,
            case_id,
            Some(sample_settlement_input()),
        )
        .await
        .map_err(|err| format!("initial upsert failed: {err:?}"))?;

        // Simulate the (deferred) 노무사 recording action: mark CERTIFIED with a
        // valid artifact + digest as console_rt.
        let validation = ProfessionalValidation {
            reviewer_kind: ProfessionalReviewerKind::LaborAttorney,
            reviewed_on: time::macros::date!(2026 - 07 - 03),
            artifact_sha256: "a".repeat(64),
            reviewer_reference: "노무법인 검증 2026-1".to_owned(),
        };
        sqlx::query(
            r#"
            UPDATE employee_exit_settlement_packages
            SET certification_status = 'CERTIFIED',
                certification_artifact = $3,
                certified_package_digest = $4
            WHERE org_id = $1 AND id = $2
            "#,
        )
        .bind(org_id)
        .bind(package_id)
        .bind(certification_artifact_json(&validation))
        .bind("b".repeat(64))
        .execute(tx.as_mut())
        .await
        .map_err(|err| format!("simulate certification failed: {err}"))?;

        // Recompute settlement fields via the normal code path.
        upsert_exit_settlement_package(&mut tx, org_id, case_id, Some(sample_settlement_input()))
            .await
            .map_err(|err| format!("recompute upsert failed: {err:?}"))?;

        let row = sqlx::query(
            r#"
            SELECT certification_status,
                   certification_artifact IS NULL AS artifact_null,
                   certified_package_digest IS NULL AS digest_null
            FROM employee_exit_settlement_packages
            WHERE org_id = $1 AND id = $2
            "#,
        )
        .bind(org_id)
        .bind(package_id)
        .fetch_one(tx.as_mut())
        .await
        .map_err(|err| format!("reload package failed: {err}"))?;
        let status: String = row
            .try_get("certification_status")
            .map_err(|err| format!("read status failed: {err}"))?;
        let artifact_null: bool = row
            .try_get("artifact_null")
            .map_err(|err| format!("read artifact_null failed: {err}"))?;
        let digest_null: bool = row
            .try_get("digest_null")
            .map_err(|err| format!("read digest_null failed: {err}"))?;

        tx.rollback()
            .await
            .map_err(|err| format!("rollback failed: {err}"))?;

        assert_eq!(
            status, "UNCERTIFIED_DRAFT",
            "a settlement recompute must revert certification_status"
        );
        assert!(artifact_null, "recompute must clear certification_artifact");
        assert!(digest_null, "recompute must clear certified_package_digest");
        Ok(())
    }

    #[cfg(feature = "test-postgres")]
    #[sqlx::test(migrations = "../crates/platform/db/migrations")]
    async fn certification_honored_only_when_digest_binds_current_numbers(
        pool: sqlx::PgPool,
    ) -> Result<(), String> {
        let (org_id, case_id) = seed_exit_case(&pool).await?;
        let mut tx = pool
            .begin()
            .await
            .map_err(|err| format!("begin failed: {err}"))?;
        arm_console_rt(&mut tx, org_id).await?;

        let package_id = upsert_exit_settlement_package(
            &mut tx,
            org_id,
            case_id,
            Some(sample_settlement_input()),
        )
        .await
        .map_err(|err| format!("upsert failed: {err:?}"))?;

        // Compute the digest that binds the CURRENT row and certify with it.
        let covered = sqlx::query(
            r#"
            SELECT severance_pay_won, statutory_basis, insurance_loss_payload, approval_payload,
                   average_wage_period_start::TEXT AS average_wage_period_start,
                   average_wage_period_end::TEXT AS average_wage_period_end,
                   average_wage_calendar_days, average_wage_total_won,
                   average_daily_wage_milliwon, service_days,
                   monthly_ordinary_wage_won, ordinary_daily_wage_won,
                   statutory_daily_wage_milliwon
            FROM employee_exit_settlement_packages
            WHERE org_id = $1 AND id = $2
            "#,
        )
        .bind(org_id)
        .bind(package_id)
        .fetch_one(tx.as_mut())
        .await
        .map_err(|err| format!("load covered fields failed: {err}"))?;
        let severance_pay_won: Option<i64> = covered
            .try_get("severance_pay_won")
            .map_err(|err| format!("{err}"))?;
        let statutory_basis: Value = covered
            .try_get("statutory_basis")
            .map_err(|err| format!("{err}"))?;
        let insurance_loss_payload: Value = covered
            .try_get("insurance_loss_payload")
            .map_err(|err| format!("{err}"))?;
        let approval_payload: Value = covered
            .try_get("approval_payload")
            .map_err(|err| format!("{err}"))?;
        let period_start: Option<String> = covered
            .try_get("average_wage_period_start")
            .map_err(|err| format!("{err}"))?;
        let period_end: Option<String> = covered
            .try_get("average_wage_period_end")
            .map_err(|err| format!("{err}"))?;
        let calendar_days: Option<i32> = covered
            .try_get("average_wage_calendar_days")
            .map_err(|err| format!("{err}"))?;
        let total_won: Option<i64> = covered
            .try_get("average_wage_total_won")
            .map_err(|err| format!("{err}"))?;
        let daily_milliwon: Option<i64> = covered
            .try_get("average_daily_wage_milliwon")
            .map_err(|err| format!("{err}"))?;
        let service_days: Option<i32> = covered
            .try_get("service_days")
            .map_err(|err| format!("{err}"))?;
        let monthly_ordinary_wage_won: Option<i64> = covered
            .try_get("monthly_ordinary_wage_won")
            .map_err(|err| format!("{err}"))?;
        let ordinary_daily_wage_won: Option<i64> = covered
            .try_get("ordinary_daily_wage_won")
            .map_err(|err| format!("{err}"))?;
        let statutory_daily_wage_milliwon: Option<i64> = covered
            .try_get("statutory_daily_wage_milliwon")
            .map_err(|err| format!("{err}"))?;
        let matching_digest = compute_certified_package_digest(
            severance_pay_won,
            &statutory_basis,
            &insurance_loss_payload,
            &approval_payload,
            period_start.as_deref(),
            period_end.as_deref(),
            calendar_days,
            total_won,
            daily_milliwon,
            service_days,
            monthly_ordinary_wage_won,
            ordinary_daily_wage_won,
            statutory_daily_wage_milliwon,
        );
        let validation = ProfessionalValidation {
            reviewer_kind: ProfessionalReviewerKind::TaxAccountant,
            reviewed_on: time::macros::date!(2026 - 07 - 03),
            artifact_sha256: "c".repeat(64),
            reviewer_reference: "세무법인 검증 2026-2".to_owned(),
        };
        sqlx::query(
            r#"
            UPDATE employee_exit_settlement_packages
            SET certification_status = 'CERTIFIED',
                certification_artifact = $3,
                certified_package_digest = $4
            WHERE org_id = $1 AND id = $2
            "#,
        )
        .bind(org_id)
        .bind(package_id)
        .bind(certification_artifact_json(&validation))
        .bind(&matching_digest)
        .execute(tx.as_mut())
        .await
        .map_err(|err| format!("certify with matching digest failed: {err}"))?;

        // Matching digest → the read path honors CERTIFIED.
        let honored = load_exit_case_by_id(&mut tx, org_id, case_id)
            .await
            .map_err(|err| format!("load case (matching) failed: {err:?}"))?;
        assert_eq!(
            honored
                .settlement_package
                .as_ref()
                .map(|p| p.certification_status.as_str()),
            Some("CERTIFIED"),
            "a digest that binds the current numbers must be honored"
        );

        // Interleave a recompute the DB CHECK cannot catch: change a covered field
        // WITHOUT resetting certification, leaving a stale-certified row.
        sqlx::query(
            r#"
            UPDATE employee_exit_settlement_packages
            SET severance_pay_won = severance_pay_won + 1
            WHERE org_id = $1 AND id = $2
            "#,
        )
        .bind(org_id)
        .bind(package_id)
        .execute(tx.as_mut())
        .await
        .map_err(|err| format!("mutate covered field failed: {err}"))?;

        let stale = load_exit_case_by_id(&mut tx, org_id, case_id)
            .await
            .map_err(|err| format!("load case (stale) failed: {err:?}"))?;
        assert_eq!(
            stale
                .settlement_package
                .as_ref()
                .map(|p| p.certification_status.as_str()),
            Some("UNCERTIFIED_DRAFT"),
            "a stale digest must NOT be honored as certified even if the row says CERTIFIED"
        );

        tx.rollback()
            .await
            .map_err(|err| format!("rollback failed: {err}"))?;
        Ok(())
    }

    /// 0093 review HIGH #1: certification invalidation is a TABLE invariant, not
    /// an app convention. A DIRECT covered-field SQL UPDATE (as the real `console_rt`
    /// runtime role) on a CERTIFIED row must leave the STORED
    /// certification_status = UNCERTIFIED_DRAFT with artifact/digest cleared —
    /// proving migration 0094's `enforce_settlement_certification_reset()` BEFORE
    /// UPDATE trigger fires for ANY write path, not just the two app statements.
    /// This asserts the STORED row (not merely the read-path demotion).
    #[cfg(feature = "test-postgres")]
    #[sqlx::test(migrations = "../crates/platform/db/migrations")]
    async fn direct_covered_field_update_resets_stored_certification(
        pool: sqlx::PgPool,
    ) -> Result<(), String> {
        let (org_id, case_id) = seed_exit_case(&pool).await?;
        let mut tx = pool
            .begin()
            .await
            .map_err(|err| format!("begin failed: {err}"))?;
        arm_console_rt(&mut tx, org_id).await?;

        let package_id = upsert_exit_settlement_package(
            &mut tx,
            org_id,
            case_id,
            Some(sample_settlement_input()),
        )
        .await
        .map_err(|err| format!("upsert failed: {err:?}"))?;

        // Certify as console_rt: this UPDATE touches ONLY the three certification
        // columns (none certification-covered), so the trigger leaves it intact.
        let validation = ProfessionalValidation {
            reviewer_kind: ProfessionalReviewerKind::LaborAttorney,
            reviewed_on: time::macros::date!(2026 - 07 - 03),
            artifact_sha256: "a".repeat(64),
            reviewer_reference: "노무법인 검증 2026-9".to_owned(),
        };
        sqlx::query(
            r#"
            UPDATE employee_exit_settlement_packages
            SET certification_status = 'CERTIFIED',
                certification_artifact = $3,
                certified_package_digest = $4
            WHERE org_id = $1 AND id = $2
            "#,
        )
        .bind(org_id)
        .bind(package_id)
        .bind(certification_artifact_json(&validation))
        .bind("b".repeat(64))
        .execute(tx.as_mut())
        .await
        .map_err(|err| format!("certify failed: {err}"))?;

        let before: String = sqlx::query_scalar(
            "SELECT certification_status FROM employee_exit_settlement_packages WHERE org_id = $1 AND id = $2",
        )
        .bind(org_id)
        .bind(package_id)
        .fetch_one(tx.as_mut())
        .await
        .map_err(|err| format!("read pre-update status failed: {err}"))?;
        assert_eq!(
            before, "CERTIFIED",
            "certification must persist before the covered write"
        );

        // DIRECT covered-field UPDATE that does NOT touch any certification column.
        sqlx::query(
            r#"
            UPDATE employee_exit_settlement_packages
            SET severance_pay_won = severance_pay_won + 1
            WHERE org_id = $1 AND id = $2
            "#,
        )
        .bind(org_id)
        .bind(package_id)
        .execute(tx.as_mut())
        .await
        .map_err(|err| format!("direct covered-field update failed: {err}"))?;

        let row = sqlx::query(
            r#"
            SELECT certification_status,
                   certification_artifact IS NULL AS artifact_null,
                   certified_package_digest IS NULL AS digest_null
            FROM employee_exit_settlement_packages
            WHERE org_id = $1 AND id = $2
            "#,
        )
        .bind(org_id)
        .bind(package_id)
        .fetch_one(tx.as_mut())
        .await
        .map_err(|err| format!("reload row failed: {err}"))?;
        let status: String = row
            .try_get("certification_status")
            .map_err(|err| format!("read status failed: {err}"))?;
        let artifact_null: bool = row
            .try_get("artifact_null")
            .map_err(|err| format!("read artifact_null failed: {err}"))?;
        let digest_null: bool = row
            .try_get("digest_null")
            .map_err(|err| format!("read digest_null failed: {err}"))?;

        tx.rollback()
            .await
            .map_err(|err| format!("rollback failed: {err}"))?;

        assert_eq!(
            status, "UNCERTIFIED_DRAFT",
            "the DB trigger must reset STORED certification on ANY covered-field UPDATE"
        );
        assert!(
            artifact_null,
            "trigger must clear the STORED certification_artifact"
        );
        assert!(
            digest_null,
            "trigger must clear the STORED certified_package_digest"
        );
        Ok(())
    }

    /// GUARD (0093 pre-mortem #4): a human must never be able to file an
    /// uncertified severance figure with MOEL/NHIS because a label was
    /// missing from one of the generated documents. This FAILS if either
    /// generated payload (insurance-loss or approval) omits the effective
    /// certification status while the package is UNCERTIFIED_DRAFT, and
    /// FAILS if either payload fails to flip to CERTIFIED once a matching
    /// digest is recorded — proving the marker derives from the single
    /// effective-status computation rather than being hand-placed per payload.
    #[cfg(feature = "test-postgres")]
    #[sqlx::test(migrations = "../crates/platform/db/migrations")]
    async fn generated_payloads_carry_the_uncertified_draft_marker(
        pool: sqlx::PgPool,
    ) -> Result<(), String> {
        let (org_id, case_id) = seed_exit_case(&pool).await?;
        let mut tx = pool
            .begin()
            .await
            .map_err(|err| format!("begin failed: {err}"))?;
        arm_console_rt(&mut tx, org_id).await?;

        let package_id = upsert_exit_settlement_package(
            &mut tx,
            org_id,
            case_id,
            Some(sample_settlement_input()),
        )
        .await
        .map_err(|err| format!("upsert failed: {err:?}"))?;

        // Persist an approval_payload the way the approval-draft handler does,
        // so both generated payloads (not just insurance-loss) are covered.
        let context = load_exit_case_context(&mut tx, org_id, case_id, false)
            .await
            .map_err(|err| format!("load context failed: {err:?}"))?;
        let approval_payload = build_exit_approval_payload(&context, None);
        sqlx::query(
            r#"
            UPDATE employee_exit_settlement_packages
            SET approval_payload = $3
            WHERE org_id = $1 AND id = $2
            "#,
        )
        .bind(org_id)
        .bind(package_id)
        .bind(&approval_payload)
        .execute(tx.as_mut())
        .await
        .map_err(|err| format!("persist approval payload failed: {err}"))?;

        let uncertified = load_exit_case_by_id(&mut tx, org_id, case_id)
            .await
            .map_err(|err| format!("load case (uncertified) failed: {err:?}"))?;
        let package = uncertified
            .settlement_package
            .as_ref()
            .ok_or("settlement package must exist after upsert")?;
        assert_eq!(package.certification_status, "UNCERTIFIED_DRAFT");
        assert_eq!(
            package.insurance_loss_payload.get("certification_status"),
            Some(&Value::String("UNCERTIFIED_DRAFT".to_owned())),
            "insurance-loss payload must carry the draft marker when uncertified"
        );
        assert_eq!(
            package.approval_payload.get("certification_status"),
            Some(&Value::String("UNCERTIFIED_DRAFT".to_owned())),
            "approval payload must carry the draft marker when uncertified"
        );

        // Certify with a digest that binds the CURRENT numbers and verify both
        // payloads flip to CERTIFIED too.
        let covered = sqlx::query(
            r#"
            SELECT severance_pay_won, statutory_basis, insurance_loss_payload, approval_payload,
                   average_wage_period_start::TEXT AS average_wage_period_start,
                   average_wage_period_end::TEXT AS average_wage_period_end,
                   average_wage_calendar_days, average_wage_total_won,
                   average_daily_wage_milliwon, service_days,
                   monthly_ordinary_wage_won, ordinary_daily_wage_won,
                   statutory_daily_wage_milliwon
            FROM employee_exit_settlement_packages
            WHERE org_id = $1 AND id = $2
            "#,
        )
        .bind(org_id)
        .bind(package_id)
        .fetch_one(tx.as_mut())
        .await
        .map_err(|err| format!("load covered fields failed: {err}"))?;
        let severance_pay_won: Option<i64> = covered
            .try_get("severance_pay_won")
            .map_err(|err| format!("{err}"))?;
        let statutory_basis: Value = covered
            .try_get("statutory_basis")
            .map_err(|err| format!("{err}"))?;
        let insurance_loss_payload: Value = covered
            .try_get("insurance_loss_payload")
            .map_err(|err| format!("{err}"))?;
        let approval_payload: Value = covered
            .try_get("approval_payload")
            .map_err(|err| format!("{err}"))?;
        let period_start: Option<String> = covered
            .try_get("average_wage_period_start")
            .map_err(|err| format!("{err}"))?;
        let period_end: Option<String> = covered
            .try_get("average_wage_period_end")
            .map_err(|err| format!("{err}"))?;
        let calendar_days: Option<i32> = covered
            .try_get("average_wage_calendar_days")
            .map_err(|err| format!("{err}"))?;
        let total_won: Option<i64> = covered
            .try_get("average_wage_total_won")
            .map_err(|err| format!("{err}"))?;
        let daily_milliwon: Option<i64> = covered
            .try_get("average_daily_wage_milliwon")
            .map_err(|err| format!("{err}"))?;
        let service_days: Option<i32> = covered
            .try_get("service_days")
            .map_err(|err| format!("{err}"))?;
        let monthly_ordinary_wage_won: Option<i64> = covered
            .try_get("monthly_ordinary_wage_won")
            .map_err(|err| format!("{err}"))?;
        let ordinary_daily_wage_won: Option<i64> = covered
            .try_get("ordinary_daily_wage_won")
            .map_err(|err| format!("{err}"))?;
        let statutory_daily_wage_milliwon: Option<i64> = covered
            .try_get("statutory_daily_wage_milliwon")
            .map_err(|err| format!("{err}"))?;
        let matching_digest = compute_certified_package_digest(
            severance_pay_won,
            &statutory_basis,
            &insurance_loss_payload,
            &approval_payload,
            period_start.as_deref(),
            period_end.as_deref(),
            calendar_days,
            total_won,
            daily_milliwon,
            service_days,
            monthly_ordinary_wage_won,
            ordinary_daily_wage_won,
            statutory_daily_wage_milliwon,
        );
        let validation = ProfessionalValidation {
            reviewer_kind: ProfessionalReviewerKind::LaborAttorney,
            reviewed_on: time::macros::date!(2026 - 07 - 03),
            artifact_sha256: "d".repeat(64),
            reviewer_reference: "노무법인 검증 2026-3".to_owned(),
        };
        sqlx::query(
            r#"
            UPDATE employee_exit_settlement_packages
            SET certification_status = 'CERTIFIED',
                certification_artifact = $3,
                certified_package_digest = $4
            WHERE org_id = $1 AND id = $2
            "#,
        )
        .bind(org_id)
        .bind(package_id)
        .bind(certification_artifact_json(&validation))
        .bind(&matching_digest)
        .execute(tx.as_mut())
        .await
        .map_err(|err| format!("certify with matching digest failed: {err}"))?;

        let certified = load_exit_case_by_id(&mut tx, org_id, case_id)
            .await
            .map_err(|err| format!("load case (certified) failed: {err:?}"))?;
        let package = certified
            .settlement_package
            .as_ref()
            .ok_or("settlement package must exist after certification")?;
        assert_eq!(package.certification_status, "CERTIFIED");
        assert_eq!(
            package.insurance_loss_payload.get("certification_status"),
            Some(&Value::String("CERTIFIED".to_owned())),
            "insurance-loss payload must reflect CERTIFIED once the digest matches"
        );
        assert_eq!(
            package.approval_payload.get("certification_status"),
            Some(&Value::String("CERTIFIED".to_owned())),
            "approval payload must reflect CERTIFIED once the digest matches"
        );

        tx.rollback()
            .await
            .map_err(|err| format!("rollback failed: {err}"))?;
        Ok(())
    }

    #[allow(dead_code)]
    async fn seed_exit_confirmer(
        pool: &sqlx::PgPool,
        org_id: Uuid,
        role: &str,
    ) -> Result<Uuid, String> {
        let user_id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO users (id, display_name, roles, is_active, org_id) VALUES ($1, $2, ARRAY[$3]::TEXT[], true, $4)",
        )
        .bind(user_id)
        .bind(format!("Exit {role}"))
        .bind(role)
        .bind(org_id)
        .execute(pool)
        .await
        .map_err(|err| format!("seed confirmer failed: {err}"))?;
        Ok(user_id)
    }

    /// US-005 two-tier separation of duties, pure decision function: HQ
    /// confirmation is gated on stored state + a distinct actor, never the
    /// client `hq_confirmation` boolean alone.
    #[cfg(not(feature = "test-postgres"))]
    #[test]
    fn exit_confirmation_hq_tier_enforces_state_and_distinct_actor() -> Result<(), String> {
        let hr_actor = Uuid::new_v4();
        let hq_actor = Uuid::new_v4();

        let reject = |status: &str, hr_by: Option<Uuid>, actor: Uuid, label: &str| {
            match authorize_exit_confirmation_hq_tier(status, hr_by, actor) {
                Ok(()) => Err(format!("{label} should have been rejected")),
                Err(err) => Ok(err),
            }
        };

        // (a) the actor who recorded the HR confirmation cannot also HQ-confirm.
        let same_actor = reject("HR_CONFIRMED", Some(hr_actor), hr_actor, "same-actor HQ")?;
        assert_eq!(same_actor.status, StatusCode::FORBIDDEN);

        // (b) HQ confirmation attempted while the case is still REPORTED (no HR
        // confirmation yet) is rejected out of order.
        let out_of_order = reject("REPORTED", None, hq_actor, "HQ-before-HR")?;
        assert_eq!(out_of_order.code, "invalid_transition");

        // An HR_CONFIRMED status with no recorded confirmer is still refused.
        let missing_hr = reject(
            "HR_CONFIRMED",
            None,
            hq_actor,
            "HQ with no recorded HR confirmer",
        )?;
        assert_eq!(missing_hr.code, "invalid_transition");

        // (c) happy path: a DISTINCT HQ actor on an HR_CONFIRMED case is allowed.
        authorize_exit_confirmation_hq_tier("HR_CONFIRMED", Some(hr_actor), hq_actor)
            .map_err(|err| format!("distinct HQ actor was rejected: {}", err.message))?;
        Ok(())
    }

    /// US-005 per-endpoint capability matrix, checked as capabilities against a
    /// real case's branch as `console_rt`: a role lacking each new capability is
    /// rejected on the corresponding endpoint's gate.
    #[cfg(feature = "test-postgres")]
    #[sqlx::test(migrations = "../crates/platform/db/migrations")]
    async fn exit_endpoints_reject_roles_lacking_the_new_capabilities(
        pool: sqlx::PgPool,
    ) -> Result<(), String> {
        use console_platform_authz::Role;

        let (org_id, case_id) = seed_exit_case(&pool).await?;
        let mut tx = pool
            .begin()
            .await
            .map_err(|err| format!("begin failed: {err}"))?;
        arm_console_rt(&mut tx, org_id).await?;

        let context = load_exit_case_context(&mut tx, org_id, case_id, false)
            .await
            .map_err(|err| format!("load context failed: {err:?}"))?;
        let branch = context.branch_id;
        let branch_scope = branch.map_or_else(BranchScope::none, |b| {
            BranchScope::single(BranchId::from_uuid(b))
        });
        let scoped = |role: Role, scope: BranchScope| {
            Principal::new(UserId::new(), OrgId::knl(), BTreeSet::from([role]), scope)
        };

        // MECHANIC holds none of the exit-workflow capabilities.
        let mechanic = scoped(Role::Mechanic, branch_scope.clone());
        for feature in [
            Feature::ExitCaseReport,
            Feature::ExitCaseHrConfirm,
            Feature::ExitCaseHqConfirm,
            Feature::ExitSettlementManage,
        ] {
            assert!(
                authorize_hr_scoped_write(&mechanic, feature, branch).is_err(),
                "MECHANIC must be rejected for {feature:?}"
            );
        }

        // Branch ADMIN holds report / HR-confirm / settlement, but NOT HQ-confirm.
        let admin = scoped(Role::Admin, branch_scope);
        for feature in [
            Feature::ExitCaseReport,
            Feature::ExitCaseHrConfirm,
            Feature::ExitSettlementManage,
        ] {
            authorize_hr_scoped_write(&admin, feature, branch)
                .map_err(|err| format!("ADMIN unexpectedly rejected for {feature:?}: {err:?}"))?;
        }
        assert!(
            authorize_hr_scoped_write(&admin, Feature::ExitCaseHqConfirm, branch).is_err(),
            "a branch ADMIN must NOT hold the HQ confirmation capability"
        );

        // Org-wide EXECUTIVE holds HQ-confirm, but NOT the HR-manager write tier.
        let executive = scoped(Role::Executive, BranchScope::All);
        authorize_hr_scoped_write(&executive, Feature::ExitCaseHqConfirm, branch)
            .map_err(|err| format!("EXECUTIVE unexpectedly rejected for HQ confirm: {err:?}"))?;
        for feature in [
            Feature::ExitCaseReport,
            Feature::ExitCaseHrConfirm,
            Feature::ExitSettlementManage,
        ] {
            assert!(
                authorize_hr_scoped_write(&executive, feature, branch).is_err(),
                "EXECUTIVE (read/oversight tier) must NOT hold {feature:?}"
            );
        }

        tx.rollback()
            .await
            .map_err(|err| format!("rollback failed: {err}"))?;
        Ok(())
    }

    /// US-005 two-tier enforcement against REAL stored state read as `console_rt`:
    /// the decision derives from the persisted status + `hr_confirmed_by`, not
    /// the client flag. Covers (a) same-actor HQ rejected, (b) out-of-order HQ
    /// (still REPORTED) rejected, (c) a distinct HQ actor allowed.
    #[cfg(feature = "test-postgres")]
    #[sqlx::test(migrations = "../crates/platform/db/migrations")]
    async fn exit_confirmation_two_tier_uses_stored_state_not_client_flag(
        pool: sqlx::PgPool,
    ) -> Result<(), String> {
        let (org_id, case_id) = seed_exit_case(&pool).await?;
        let hr_actor = seed_exit_confirmer(&pool, org_id, "ADMIN").await?;
        let hq_actor = seed_exit_confirmer(&pool, org_id, "EXECUTIVE").await?;

        let mut tx = pool
            .begin()
            .await
            .map_err(|err| format!("begin failed: {err}"))?;
        arm_console_rt(&mut tx, org_id).await?;

        // Record the first-tier (HR) confirmation on the seeded HR_CONFIRMED case
        // as console_rt (proves the runtime role can write the case under RLS).
        sqlx::query(
            r#"
            UPDATE employee_exit_cases
            SET hr_confirmed_by = $3, hr_confirmed_at = now()
            WHERE org_id = $1 AND id = $2
            "#,
        )
        .bind(org_id)
        .bind(case_id)
        .bind(hr_actor)
        .execute(tx.as_mut())
        .await
        .map_err(|err| format!("record HR confirmer failed: {err}"))?;

        // Read the authoritative state back as console_rt (proves RLS + the new
        // hr_confirmed_by column read on the load path).
        let confirmed = load_exit_case_context(&mut tx, org_id, case_id, true)
            .await
            .map_err(|err| format!("load HR_CONFIRMED context failed: {err:?}"))?;
        assert_eq!(confirmed.status, "HR_CONFIRMED");
        assert_eq!(confirmed.hr_confirmed_by, Some(hr_actor));

        // (a) the HR confirmer cannot also HQ-confirm the same case.
        let same_actor = match authorize_exit_confirmation_hq_tier(
            &confirmed.status,
            confirmed.hr_confirmed_by,
            hr_actor,
        ) {
            Ok(()) => {
                return Err(
                    "the HR confirmer must not be able to HQ-confirm the same case".to_owned(),
                );
            }
            Err(err) => err,
        };
        assert_eq!(same_actor.status, StatusCode::FORBIDDEN);

        // (c) a distinct HQ actor may HQ-confirm the HR-confirmed case.
        authorize_exit_confirmation_hq_tier(&confirmed.status, confirmed.hr_confirmed_by, hq_actor)
            .map_err(|err| format!("distinct HQ actor was rejected: {}", err.message))?;

        // (b) reset the case to REPORTED with no HR confirmer and prove an HQ
        // attempt is rejected out of order.
        sqlx::query(
            r#"
            UPDATE employee_exit_cases
            SET status = 'REPORTED', hr_confirmed_by = NULL, hr_confirmed_at = NULL
            WHERE org_id = $1 AND id = $2
            "#,
        )
        .bind(org_id)
        .bind(case_id)
        .execute(tx.as_mut())
        .await
        .map_err(|err| format!("reset to REPORTED failed: {err}"))?;

        let reported = load_exit_case_context(&mut tx, org_id, case_id, true)
            .await
            .map_err(|err| format!("load REPORTED context failed: {err:?}"))?;
        assert_eq!(reported.status, "REPORTED");
        let out_of_order = match authorize_exit_confirmation_hq_tier(
            &reported.status,
            reported.hr_confirmed_by,
            hq_actor,
        ) {
            Ok(()) => {
                return Err(
                    "HQ confirmation before any HR confirmation must be rejected".to_owned(),
                );
            }
            Err(err) => err,
        };
        assert_eq!(out_of_order.code, "invalid_transition");

        tx.rollback()
            .await
            .map_err(|err| format!("rollback failed: {err}"))?;
        Ok(())
    }

    // ---------------------------------------------------------------------
    // US-006: tenant isolation, audit coverage, and materializer discipline.
    // ---------------------------------------------------------------------

    /// Begin a transaction already dropped to the `console_rt` runtime role with the
    /// org GUC armed — the ONLY correct way to exercise RLS here, since the test
    /// pool logs in as the superuser/BYPASSRLS migration role and `console_rt` is
    /// NOLOGIN (cannot be a login pool).
    #[allow(dead_code)]
    async fn armed_tx(
        pool: &sqlx::PgPool,
        org_id: Uuid,
    ) -> Result<sqlx::Transaction<'_, Postgres>, String> {
        let mut tx = pool
            .begin()
            .await
            .map_err(|err| format!("begin failed: {err}"))?;
        arm_console_rt(&mut tx, org_id).await?;
        Ok(tx)
    }

    /// Seed one fully-formed org (region + branch + ACTIVE employee + a user) as
    /// the superuser pool role. Returns `(org_id, branch_id, employee_id,
    /// user_id)`. Seeding deliberately bypasses RLS; the isolation checks run in
    /// a separate `console_rt` transaction.
    #[allow(dead_code)]
    async fn seed_g009_base(pool: &sqlx::PgPool) -> Result<(Uuid, Uuid, Uuid, Uuid), String> {
        let org_id = Uuid::new_v4();
        let region_id = Uuid::new_v4();
        let branch_id = Uuid::new_v4();
        let employee_id = Uuid::new_v4();
        let user_id = Uuid::new_v4();
        sqlx::query("INSERT INTO organizations (id, slug, name) VALUES ($1, $2, $3)")
            .bind(org_id)
            .bind(format!("g009-{}", &org_id.to_string()[..8]))
            .bind("G009 Isolation Test")
            .execute(pool)
            .await
            .map_err(|err| format!("seed organization failed: {err}"))?;
        sqlx::query("INSERT INTO regions (id, name, org_id) VALUES ($1, $2, $3)")
            .bind(region_id)
            .bind("이탈지역")
            .bind(org_id)
            .execute(pool)
            .await
            .map_err(|err| format!("seed region failed: {err}"))?;
        sqlx::query("INSERT INTO branches (id, region_id, name, org_id) VALUES ($1, $2, $3, $4)")
            .bind(branch_id)
            .bind(region_id)
            .bind("본사")
            .bind(org_id)
            .execute(pool)
            .await
            .map_err(|err| format!("seed branch failed: {err}"))?;
        sqlx::query(
            "INSERT INTO users (id, display_name, roles, is_active, org_id) VALUES ($1, $2, ARRAY['ADMIN']::TEXT[], true, $3)",
        )
        .bind(user_id)
        .bind("G009 Reporter")
        .bind(org_id)
        .execute(pool)
        .await
        .map_err(|err| format!("seed user failed: {err}"))?;
        sqlx::query(
            r#"
            INSERT INTO employees (
                id, org_id, company, name, employee_number, hire_date,
                source_filename, source_sheet, source_row, source_key, raw_row, source_metadata
            )
            VALUES ($1, $2, '테스트', '홍길동', 'E-001', '2020-01-01',
                    'employees.xlsx', '직원', 2, 'employee-row-2', '{}', '{}')
            "#,
        )
        .bind(employee_id)
        .bind(org_id)
        .execute(pool)
        .await
        .map_err(|err| format!("seed employee failed: {err}"))?;
        let mut command = pool
            .begin()
            .await
            .map_err(|err| format!("begin home-branch fixture command failed: {err}"))?;
        sqlx::query("SET LOCAL ROLE console_leave_definer")
            .execute(command.as_mut())
            .await
            .map_err(|err| format!("set leave definer role failed: {err}"))?;
        sqlx::query("SELECT set_config('app.current_org', $1, true)")
            .bind(org_id.to_string())
            .execute(command.as_mut())
            .await
            .map_err(|err| format!("arm fixture org failed: {err}"))?;
        let updated =
            sqlx::query("UPDATE employees SET home_branch_id = $1 WHERE id = $2 AND org_id = $3")
                .bind(branch_id)
                .bind(employee_id)
                .bind(org_id)
                .execute(command.as_mut())
                .await
                .map_err(|err| format!("set fixture employee home branch failed: {err}"))?;
        if updated.rows_affected() != 1 {
            return Err(
                "fixture home-branch command did not update exactly one employee".to_owned(),
            );
        }
        command
            .commit()
            .await
            .map_err(|err| format!("commit home-branch fixture command failed: {err}"))?;
        Ok((org_id, branch_id, employee_id, user_id))
    }

    /// Seed one row into each of the three G009 tenant tables for `org_id`.
    /// Returns `(alert_id, case_id, package_id)`.
    #[allow(dead_code)]
    async fn seed_g009_rows(
        pool: &sqlx::PgPool,
        org_id: Uuid,
        branch_id: Uuid,
        employee_id: Uuid,
        user_id: Uuid,
    ) -> Result<(Uuid, Uuid, Uuid), String> {
        let alert_id = Uuid::new_v4();
        let case_id = Uuid::new_v4();
        let package_id = Uuid::new_v4();
        sqlx::query(
            r#"
            INSERT INTO employee_absence_alerts (id, org_id, employee_id, branch_id, work_date)
            VALUES ($1, $2, $3, $4, '2026-07-01')
            "#,
        )
        .bind(alert_id)
        .bind(org_id)
        .bind(employee_id)
        .bind(branch_id)
        .execute(pool)
        .await
        .map_err(|err| format!("seed absence alert failed: {err}"))?;
        sqlx::query(
            r#"
            INSERT INTO employee_exit_cases (
                id, org_id, employee_id, branch_id, status,
                effective_exit_date, site_manager_note, reported_by
            )
            VALUES ($1, $2, $3, $4, 'REPORTED', '2026-06-30', '무단결근 확인', $5)
            "#,
        )
        .bind(case_id)
        .bind(org_id)
        .bind(employee_id)
        .bind(branch_id)
        .bind(user_id)
        .execute(pool)
        .await
        .map_err(|err| format!("seed exit case failed: {err}"))?;
        sqlx::query(
            r#"
            INSERT INTO employee_exit_settlement_packages (id, org_id, exit_case_id, employee_id)
            VALUES ($1, $2, $3, $4)
            "#,
        )
        .bind(package_id)
        .bind(org_id)
        .bind(case_id)
        .bind(employee_id)
        .execute(pool)
        .await
        .map_err(|err| format!("seed settlement package failed: {err}"))?;
        Ok((alert_id, case_id, package_id))
    }

    /// Every G009 tenant table is invisible AND un-writable across orgs when
    /// queried as the real `console_rt` runtime role armed to a different tenant.
    /// Seeding runs as the superuser pool role; the assertions run strictly as
    /// `console_rt` (via `arm_console_rt`), so a broken `org_isolation` policy cannot be
    /// masked by BYPASSRLS.
    #[cfg(feature = "test-postgres")]
    #[sqlx::test(migrations = "../crates/platform/db/migrations")]
    async fn g009_tables_isolate_tenants_as_console_rt(pool: sqlx::PgPool) -> Result<(), String> {
        let (org_a, branch_a, emp_a, user_a) = seed_g009_base(&pool).await?;
        seed_g009_rows(&pool, org_a, branch_a, emp_a, user_a).await?;
        let (org_b, branch_b, emp_b, user_b) = seed_g009_base(&pool).await?;
        let (alert_b, case_b, pkg_b) =
            seed_g009_rows(&pool, org_b, branch_b, emp_b, user_b).await?;

        // Reads + filtered updates, as console_rt armed to org A.
        let mut tx = armed_tx(&pool, org_a).await?;
        for (table, b_id) in [
            ("employee_absence_alerts", alert_b),
            ("employee_exit_cases", case_b),
            ("employee_exit_settlement_packages", pkg_b),
        ] {
            // sqlx 0.9 only accepts `&'static str` in `query`; dynamic table
            // names go through QueryBuilder (the file's own idiom).
            let visible: i64 =
                QueryBuilder::<Postgres>::new(format!("SELECT COUNT(*) FROM {table}"))
                    .build_query_scalar()
                    .fetch_one(tx.as_mut())
                    .await
                    .map_err(|err| format!("{table}: count as console_rt failed: {err}"))?;
            assert_eq!(
                visible, 1,
                "{table}: org A must see only its own row under RLS"
            );

            let mut b_lookup =
                QueryBuilder::<Postgres>::new(format!("SELECT COUNT(*) FROM {table} WHERE id = "));
            b_lookup.push_bind(b_id);
            let b_visible: i64 = b_lookup
                .build_query_scalar()
                .fetch_one(tx.as_mut())
                .await
                .map_err(|err| format!("{table}: org B lookup failed: {err}"))?;
            assert_eq!(
                b_visible, 0,
                "{table}: an org B row must be invisible to org A"
            );

            let mut cross_org_update = QueryBuilder::<Postgres>::new(format!(
                "UPDATE {table} SET updated_at = now() WHERE id = "
            ));
            cross_org_update.push_bind(b_id);
            let updated = cross_org_update
                .build()
                .execute(tx.as_mut())
                .await
                .map_err(|err| format!("{table}: cross-org update failed: {err}"))?;
            assert_eq!(
                updated.rows_affected(),
                0,
                "{table}: org A must not update an org B row (RLS USING filters it out)"
            );
        }
        tx.rollback()
            .await
            .map_err(|err| format!("rollback failed: {err}"))?;

        // Cross-org INSERTs must be rejected by the RLS WITH CHECK. A failed
        // statement aborts its transaction, so each runs in its own armed tx.
        {
            let mut tx = armed_tx(&pool, org_a).await?;
            let res = sqlx::query(
                "INSERT INTO employee_absence_alerts (org_id, employee_id, branch_id, work_date) VALUES ($1, $2, $3, '2026-07-09')",
            )
            .bind(org_b)
            .bind(emp_b)
            .bind(branch_b)
            .execute(tx.as_mut())
            .await;
            assert!(
                res.is_err(),
                "org A (console_rt) must not INSERT an org B absence alert"
            );
            let _ = tx.rollback().await;
        }
        {
            let mut tx = armed_tx(&pool, org_a).await?;
            let res = sqlx::query(
                "INSERT INTO employee_exit_cases (org_id, employee_id, branch_id, status, effective_exit_date, site_manager_note, reported_by) VALUES ($1, $2, $3, 'REPORTED', '2026-06-30', 'x', $4)",
            )
            .bind(org_b)
            .bind(emp_b)
            .bind(branch_b)
            .bind(user_b)
            .execute(tx.as_mut())
            .await;
            assert!(
                res.is_err(),
                "org A (console_rt) must not INSERT an org B exit case"
            );
            let _ = tx.rollback().await;
        }
        {
            let mut tx = armed_tx(&pool, org_a).await?;
            let res = sqlx::query(
                "INSERT INTO employee_exit_settlement_packages (org_id, exit_case_id, employee_id) VALUES ($1, $2, $3)",
            )
            .bind(org_b)
            .bind(case_b)
            .bind(emp_b)
            .execute(tx.as_mut())
            .await;
            assert!(
                res.is_err(),
                "org A (console_rt) must not INSERT an org B settlement package"
            );
            let _ = tx.rollback().await;
        }
        Ok(())
    }

    #[cfg(feature = "test-postgres")]
    #[sqlx::test(migrations = "../crates/platform/db/migrations")]
    async fn exit_report_uses_durable_branch_evidence_and_fails_closed_without_it(
        pool: sqlx::PgPool,
    ) -> Result<(), String> {
        use console_platform_authz::Role;

        let (org_id, home_branch_id, employee_id, actor_id) = seed_g009_base(&pool).await?;
        let region_id: Uuid = sqlx::query_scalar("SELECT region_id FROM branches WHERE id = $1")
            .bind(home_branch_id)
            .fetch_one(&pool)
            .await
            .map_err(|err| format!("load region failed: {err}"))?;
        let foreign_branch_id = Uuid::new_v4();
        sqlx::query("INSERT INTO branches (id, region_id, name, org_id) VALUES ($1, $2, $3, $4)")
            .bind(foreign_branch_id)
            .bind(region_id)
            .bind("타지점")
            .bind(org_id)
            .execute(&pool)
            .await
            .map_err(|err| format!("seed foreign branch failed: {err}"))?;

        let org = OrgId::from_uuid(org_id);
        let state = HrState::new(pool.clone(), None);
        let principal = |scope| {
            Principal::new(
                UserId::from_uuid(actor_id),
                org,
                BTreeSet::from([Role::Admin]),
                scope,
            )
        };
        let request =
            |target_employee_id, branch_id, absence_alert_id| ReportEmployeeExitCaseRequest {
                employee_id: target_employee_id,
                branch_id,
                absence_alert_id,
                effective_exit_date: "2026-06-30".to_owned(),
                site_manager_note: "branch provenance test".to_owned(),
            };

        let forged = report_employee_exit_case(
            State(state.clone()),
            Extension(principal(BranchScope::single(BranchId::from_uuid(
                foreign_branch_id,
            )))),
            Json(request(employee_id, Some(foreign_branch_id), None)),
        )
        .await
        .expect_err("a client branch must not override the employee's home branch");
        assert_eq!(forged.status, StatusCode::NOT_FOUND);

        let home_scope = BranchScope::single(BranchId::from_uuid(home_branch_id));
        let other_employee_id = Uuid::new_v4();
        sqlx::query(
            r#"
            INSERT INTO employees (
                id, org_id, company, name, hire_date,
                source_filename, source_sheet, source_row, source_key, raw_row, source_metadata
            )
            VALUES ($1, $2, '테스트', '다른직원', '2020-01-01',
                    'other.xlsx', '직원', 2, $3, '{}', '{}')
            "#,
        )
        .bind(other_employee_id)
        .bind(org_id)
        .bind(format!("other-{other_employee_id}"))
        .execute(&pool)
        .await
        .map_err(|err| format!("seed other employee failed: {err}"))?;
        let other_alert_id: Uuid = sqlx::query_scalar(
            r#"
            INSERT INTO employee_absence_alerts (org_id, employee_id, branch_id, work_date)
            VALUES ($1, $2, $3, '2026-06-29')
            RETURNING id
            "#,
        )
        .bind(org_id)
        .bind(other_employee_id)
        .bind(home_branch_id)
        .fetch_one(&pool)
        .await
        .map_err(|err| format!("seed other alert failed: {err}"))?;
        let wrong_employee_alert = report_employee_exit_case(
            State(state.clone()),
            Extension(principal(home_scope.clone())),
            Json(request(
                employee_id,
                Some(home_branch_id),
                Some(other_alert_id),
            )),
        )
        .await
        .expect_err("another employee's alert must not authorize an exit report");
        assert_eq!(wrong_employee_alert.status, StatusCode::NOT_FOUND);
        let other_alert_state: (String, Option<Uuid>) = sqlx::query_as(
            "SELECT status, linked_exit_case_id FROM employee_absence_alerts WHERE id = $1",
        )
        .bind(other_alert_id)
        .fetch_one(&pool)
        .await
        .map_err(|err| format!("read other alert failed: {err}"))?;
        assert_eq!(other_alert_state, ("OPEN".to_owned(), None));

        let missing_alert = report_employee_exit_case(
            State(state.clone()),
            Extension(principal(home_scope.clone())),
            Json(request(
                employee_id,
                Some(home_branch_id),
                Some(Uuid::new_v4()),
            )),
        )
        .await
        .expect_err("a nonexistent alert must not silently fall through");
        assert_eq!(missing_alert.status, StatusCode::NOT_FOUND);

        let branchless = report_employee_exit_case(
            State(state.clone()),
            Extension(principal(home_scope)),
            Json(request(other_employee_id, None, None)),
        )
        .await
        .expect_err("a branch-scoped ADMIN must not mutate a branchless employee");
        assert_eq!(branchless.status, StatusCode::FORBIDDEN);

        for failed_employee_id in [employee_id, other_employee_id] {
            let leaked_cases: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM employee_exit_cases WHERE employee_id = $1",
            )
            .bind(failed_employee_id)
            .fetch_one(&pool)
            .await
            .map_err(|err| format!("count leaked cases failed: {err}"))?;
            let leaked_audits: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM audit_events WHERE action = 'employee.exit.report' AND target_id = $1",
            )
            .bind(failed_employee_id.to_string())
            .fetch_one(&pool)
            .await
            .map_err(|err| format!("count leaked audits failed: {err}"))?;
            assert_eq!(leaked_cases, 0);
            assert_eq!(leaked_audits, 0);
        }

        let super_admin = Principal::new(
            UserId::from_uuid(actor_id),
            org,
            BTreeSet::from([Role::SuperAdmin]),
            BranchScope::All,
        );
        let created = report_employee_exit_case(
            State(state),
            Extension(super_admin),
            Json(request(other_employee_id, None, None)),
        )
        .await
        .map_err(|err| format!("org-wide branchless report was rejected: {err:?}"))?;
        assert_eq!(created.0, StatusCode::CREATED);
        assert_eq!(created.1.0.branch_id, None);
        Ok(())
    }

    /// Every G009 state-mutation handler routes its write through `with_audit`,
    /// so an `audit_events` row lands for the exit report, HR confirmation, HQ
    /// confirmation, and approval draft + submission. The settlement upsert is a
    /// side effect INSIDE the confirm/approval-draft audited transaction (there
    /// is no standalone settlement endpoint), so those events cover it, and the
    /// certification-bearing approval-draft event captures the resulting
    /// certification state. This drives the real handlers through the test pool
    /// role (audit emission is role-independent); the RLS proof is the dedicated
    /// `console_rt` test above.
    #[cfg(feature = "test-postgres")]
    #[sqlx::test(migrations = "../crates/platform/db/migrations")]
    async fn exit_workflow_handlers_emit_audit_events(pool: sqlx::PgPool) -> Result<(), String> {
        use console_platform_authz::Role;

        let (org_id, branch_id, employee_id, hr_user) = seed_g009_base(&pool).await?;
        let hq_user = seed_exit_confirmer(&pool, org_id, "EXECUTIVE").await?;
        let org = OrgId::from_uuid(org_id);
        let state = HrState::new(pool.clone(), None);

        let hr_principal = Principal::new(
            UserId::from_uuid(hr_user),
            org,
            BTreeSet::from([Role::Admin]),
            BranchScope::single(BranchId::from_uuid(branch_id)),
        );
        let hq_principal = Principal::new(
            UserId::from_uuid(hq_user),
            org,
            BTreeSet::from([Role::Executive]),
            BranchScope::All,
        );

        // (1) site-manager exit report.
        let reported = report_employee_exit_case(
            State(state.clone()),
            Extension(hr_principal.clone()),
            Json(ReportEmployeeExitCaseRequest {
                employee_id,
                branch_id: Some(branch_id),
                absence_alert_id: None,
                effective_exit_date: "2026-06-30".to_owned(),
                site_manager_note: "무단결근 3일 — 이탈 보고".to_owned(),
            }),
        )
        .await
        .map_err(|err| format!("report failed: {err:?}"))?;
        assert_eq!(reported.0, StatusCode::CREATED);
        let case_id = reported.1.0.id;

        // (2) HR confirmation (no wage source yet, so the case stays HR_CONFIRMED
        // for the HQ tier rather than being bumped to SETTLEMENT_READY).
        let _ = confirm_employee_exit_case(
            State(state.clone()),
            Extension(hr_principal.clone()),
            Path(case_id),
            Json(ConfirmEmployeeExitCaseRequest {
                decision: None,
                hq_confirmation: false,
                note: None,
                settlement_input: None,
            }),
        )
        .await
        .map_err(|err| format!("HR confirm failed: {err:?}"))?;

        // (3) HQ confirmation by a DISTINCT actor.
        let _ = confirm_employee_exit_case(
            State(state.clone()),
            Extension(hq_principal.clone()),
            Path(case_id),
            Json(ConfirmEmployeeExitCaseRequest {
                decision: None,
                hq_confirmation: true,
                note: None,
                settlement_input: None,
            }),
        )
        .await
        .map_err(|err| format!("HQ confirm failed: {err:?}"))?;

        // (4) approval draft — the wage source arrives here and the severance is
        // computed, then (5) submission of the same ready package.
        let _ = draft_employee_exit_approval(
            State(state.clone()),
            Extension(hr_principal.clone()),
            Path(case_id),
            Json(DraftEmployeeExitApprovalRequest {
                submit: false,
                note: None,
                settlement_input: Some(sample_settlement_input()),
            }),
        )
        .await
        .map_err(|err| format!("approval draft failed: {err:?}"))?;
        let _ = draft_employee_exit_approval(
            State(state.clone()),
            Extension(hr_principal.clone()),
            Path(case_id),
            Json(DraftEmployeeExitApprovalRequest {
                submit: true,
                note: None,
                settlement_input: None,
            }),
        )
        .await
        .map_err(|err| format!("approval submission failed: {err:?}"))?;

        let report_events: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM audit_events WHERE org_id = $1 AND action = 'employee.exit.report'",
        )
        .bind(org_id)
        .fetch_one(&pool)
        .await
        .map_err(|err| format!("count report audit failed: {err}"))?;
        assert_eq!(report_events, 1, "exit report must write one audit event");

        let hr_confirm_events: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM audit_events WHERE org_id = $1 AND action = 'employee.exit.confirm' AND actor = $2",
        )
        .bind(org_id)
        .bind(hr_user)
        .fetch_one(&pool)
        .await
        .map_err(|err| format!("count HR confirm audit failed: {err}"))?;
        assert_eq!(
            hr_confirm_events, 1,
            "HR confirmation must write one audit event for the HR actor"
        );

        let hq_confirm_events: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM audit_events WHERE org_id = $1 AND action = 'employee.exit.confirm' AND actor = $2",
        )
        .bind(org_id)
        .bind(hq_user)
        .fetch_one(&pool)
        .await
        .map_err(|err| format!("count HQ confirm audit failed: {err}"))?;
        assert_eq!(
            hq_confirm_events, 1,
            "HQ confirmation must write one audit event for the distinct HQ actor"
        );

        let draft_events: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM audit_events WHERE org_id = $1 AND action = 'employee.exit.approval_draft'",
        )
        .bind(org_id)
        .fetch_one(&pool)
        .await
        .map_err(|err| format!("count approval draft audit failed: {err}"))?;
        assert_eq!(
            draft_events, 2,
            "approval draft + submission must each write an audit event"
        );

        // Certification-bearing path: the approval-draft audit captures the
        // certification state of the figure being drafted/submitted.
        let cert_state: Option<String> = sqlx::query_scalar(
            "SELECT after_snap->>'certification_status' FROM audit_events WHERE org_id = $1 AND action = 'employee.exit.approval_draft' LIMIT 1",
        )
        .bind(org_id)
        .fetch_one(&pool)
        .await
        .map_err(|err| format!("read approval-draft snapshot failed: {err}"))?;
        assert_eq!(
            cert_state.as_deref(),
            Some("UNCERTIFIED_DRAFT"),
            "the approval-draft audit must record the certification state of the drafted figure"
        );

        // The settlement upsert (side effect of the audited confirm/draft
        // transactions) persisted an uncertified package for this case.
        let pkg_cert: String = sqlx::query_scalar(
            "SELECT certification_status FROM employee_exit_settlement_packages WHERE org_id = $1 AND exit_case_id = $2",
        )
        .bind(org_id)
        .bind(case_id)
        .fetch_one(&pool)
        .await
        .map_err(|err| format!("read settlement certification failed: {err}"))?;
        assert_eq!(pkg_cert, "UNCERTIFIED_DRAFT");
        Ok(())
    }

    /// The dashboard-path absence-alert materializer is idempotent AND
    /// write-bounded. Run twice over the SAME imported attendance facts (as
    /// `console_rt`): the second pass creates no duplicate alert (UNIQUE(org_id,
    /// employee_id, work_date, source)) AND rewrites no existing row (the
    /// IS DISTINCT FROM guard on the ON CONFLICT UPDATE), proving a repeated
    /// dashboard GET cannot write-storm.
    #[cfg(feature = "test-postgres")]
    #[sqlx::test(migrations = "../crates/platform/db/migrations")]
    async fn absence_alert_materializer_is_idempotent_and_write_bounded(
        pool: sqlx::PgPool,
    ) -> Result<(), String> {
        let (org_id, branch_id, employee_id, _user_id) = seed_g009_base(&pool).await?;
        let run_id = Uuid::new_v4();
        let source_sha256 = "a".repeat(64);
        sqlx::query(
            r#"
            INSERT INTO data_import_runs (
                id, org_id, entity_type, status, source_filename, source_format,
                source_sha256, mapping_profile, input_rows, candidate_rows, preserved_rows,
                pay_period_start, pay_period_end
            )
            VALUES ($1, $2, 'attendance_direct', 'DRY_RUN', 'attendance.csv', 'csv', $3, '{}', 2, 2, 0, DATE '2026-06-01', DATE '2026-06-30')
            "#,
        )
        .bind(run_id)
        .bind(org_id)
        .bind(&source_sha256)
        .execute(&pool)
        .await
        .map_err(|err| format!("seed import run failed: {err}"))?;
        for (idx, work_date) in ["2026-07-01", "2026-07-02"].into_iter().enumerate() {
            let import_row_id = Uuid::new_v4();
            let source_key = format!("sheet:CSV|row:{}", idx + 2);
            sqlx::query(
                r#"
                INSERT INTO data_import_rows (
                    id, org_id, run_id, source_sheet, source_row, source_key,
                    row_status, raw_row, canonical_row, validation
                )
                VALUES ($1, $2, $3, 'CSV', $4, $5, 'CANDIDATE', '{}', '{}',
                        '{"status":"ok","errors":[],"warnings":[]}')
                "#,
            )
            .bind(import_row_id)
            .bind(org_id)
            .bind(run_id)
            .bind(idx as i32 + 2)
            .bind(&source_key)
            .execute(&pool)
            .await
            .map_err(|err| format!("seed import row failed: {err}"))?;
            sqlx::query(
                r#"
                INSERT INTO attendance_direct_import_events (
                    org_id, run_id, import_row_id, employee_id, branch_id,
                    source_sheet, source_row, source_key, source_sha256, fact_key,
                    employee_name, branch_name, work_date,
                    check_in_at, check_out_at, minutes_worked
                )
                VALUES ($1, $2, $3, $4, $5, 'CSV', $6, $7, $8, $9,
                        '홍길동', '본사', $10, NULL, NULL, 0)
                "#,
            )
            .bind(org_id)
            .bind(run_id)
            .bind(import_row_id)
            .bind(employee_id)
            .bind(branch_id)
            .bind(idx as i32 + 2)
            .bind(&source_key)
            .bind(&source_sha256)
            .bind(format!("fact-{idx}-{work_date}"))
            .bind(work_date)
            .execute(&pool)
            .await
            .map_err(|err| format!("seed attendance event failed: {err}"))?;
        }

        let fingerprint = |pool: sqlx::PgPool, org: Uuid| async move {
            sqlx::query_scalar::<_, String>(
                "SELECT COALESCE(string_agg(xmin::text, ',' ORDER BY id), '') FROM employee_absence_alerts WHERE org_id = $1",
            )
            .bind(org)
            .fetch_one(&pool)
            .await
            .map_err(|err| format!("fingerprint failed: {err}"))
        };

        // First materialization pass, as console_rt.
        {
            let mut tx = armed_tx(&pool, org_id).await?;
            materialize_absence_alerts_from_imports(&mut tx, org_id, &BranchScope::All)
                .await
                .map_err(|err| format!("first materialize failed: {err:?}"))?;
            tx.commit()
                .await
                .map_err(|err| format!("commit first pass failed: {err}"))?;
        }
        let count_first: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM employee_absence_alerts WHERE org_id = $1")
                .bind(org_id)
                .fetch_one(&pool)
                .await
                .map_err(|err| format!("count after first pass failed: {err}"))?;
        assert_eq!(count_first, 2, "one alert per absent work-date");
        let fingerprint_first = fingerprint(pool.clone(), org_id).await?;

        // Second pass over the identical facts, as console_rt.
        {
            let mut tx = armed_tx(&pool, org_id).await?;
            materialize_absence_alerts_from_imports(&mut tx, org_id, &BranchScope::All)
                .await
                .map_err(|err| format!("second materialize failed: {err:?}"))?;
            tx.commit()
                .await
                .map_err(|err| format!("commit second pass failed: {err}"))?;
        }
        let count_second: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM employee_absence_alerts WHERE org_id = $1")
                .bind(org_id)
                .fetch_one(&pool)
                .await
                .map_err(|err| format!("count after second pass failed: {err}"))?;
        assert_eq!(
            count_second, 2,
            "re-materializing the same facts must not create duplicate alerts"
        );
        let fingerprint_second = fingerprint(pool.clone(), org_id).await?;
        assert_eq!(
            fingerprint_first, fingerprint_second,
            "second materialization must rewrite no row (bounded write set — no write-storm)"
        );
        Ok(())
    }

    /// 0093 review MEDIUM #5: the dashboard-path materializer mutates the audited
    /// `employee_absence_alerts` table, so the FIRST (changed) dashboard read must
    /// emit an audit event that records the changed row count/ids + source facts.
    /// A SECOND read over unchanged imports must emit NO further event, so the
    /// idempotency / write-storm guard is preserved (audit real mutations only).
    #[cfg(feature = "test-postgres")]
    #[sqlx::test(migrations = "../crates/platform/db/migrations")]
    async fn dashboard_materializer_emits_audit_on_changed_facts(
        pool: sqlx::PgPool,
    ) -> Result<(), String> {
        use console_platform_authz::Role;

        let (org_id, branch_id, employee_id, user_id) = seed_g009_base(&pool).await?;

        // Seed one absent imported attendance fact so the first read materializes.
        let run_id = Uuid::new_v4();
        let source_sha256 = "a".repeat(64);
        sqlx::query(
            r#"
            INSERT INTO data_import_runs (
                id, org_id, entity_type, status, source_filename, source_format,
                source_sha256, mapping_profile, input_rows, candidate_rows, preserved_rows,
                pay_period_start, pay_period_end
            )
            VALUES ($1, $2, 'attendance_direct', 'DRY_RUN', 'attendance.csv', 'csv', $3, '{}', 1, 1, 0, DATE '2026-06-01', DATE '2026-06-30')
            "#,
        )
        .bind(run_id)
        .bind(org_id)
        .bind(&source_sha256)
        .execute(&pool)
        .await
        .map_err(|err| format!("seed import run failed: {err}"))?;
        let import_row_id = Uuid::new_v4();
        let source_key = "sheet:CSV|row:2".to_owned();
        sqlx::query(
            r#"
            INSERT INTO data_import_rows (
                id, org_id, run_id, source_sheet, source_row, source_key,
                row_status, raw_row, canonical_row, validation
            )
            VALUES ($1, $2, $3, 'CSV', 2, $4, 'CANDIDATE', '{}', '{}',
                    '{"status":"ok","errors":[],"warnings":[]}')
            "#,
        )
        .bind(import_row_id)
        .bind(org_id)
        .bind(run_id)
        .bind(&source_key)
        .execute(&pool)
        .await
        .map_err(|err| format!("seed import row failed: {err}"))?;
        sqlx::query(
            r#"
            INSERT INTO attendance_direct_import_events (
                org_id, run_id, import_row_id, employee_id, branch_id,
                source_sheet, source_row, source_key, source_sha256, fact_key,
                employee_name, branch_name, work_date,
                check_in_at, check_out_at, minutes_worked
            )
            VALUES ($1, $2, $3, $4, $5, 'CSV', 2, $6, $7, 'fact-1',
                    '홍길동', '본사', '2026-07-01', NULL, NULL, 0)
            "#,
        )
        .bind(org_id)
        .bind(run_id)
        .bind(import_row_id)
        .bind(employee_id)
        .bind(branch_id)
        .bind(&source_key)
        .bind(&source_sha256)
        .execute(&pool)
        .await
        .map_err(|err| format!("seed attendance event failed: {err}"))?;

        let org = OrgId::from_uuid(org_id);
        let state = HrState::new(pool.clone(), None);
        // Executive + org-wide scope: `authorize_org_wide` limits built-in
        // org-wide reads to SUPER_ADMIN/EXECUTIVE, so a branch-scoped ADMIN
        // cannot drive the org-wide dashboard read.
        let principal = Principal::new(
            UserId::from_uuid(user_id),
            org,
            BTreeSet::from([Role::Executive]),
            BranchScope::All,
        );
        let query = || AbsenceExitDashboardQuery {
            limit: None,
            offset: None,
            employee_id: None,
        };

        // First read: materializes one alert → one audit event.
        let _ = get_absence_exit_dashboard(
            State(state.clone()),
            Extension(principal.clone()),
            Query(query()),
        )
        .await
        .map_err(|err| format!("first dashboard read failed: {err:?}"))?;

        let (event_count, changed_count): (i64, Option<i64>) = {
            let count: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM audit_events WHERE org_id = $1 AND action = 'employee.absence_alert.materialize'",
            )
            .bind(org_id)
            .fetch_one(&pool)
            .await
            .map_err(|err| format!("count materialize audit failed: {err}"))?;
            let changed: Option<i64> = sqlx::query_scalar(
                "SELECT (after_snap->>'changed_count')::BIGINT FROM audit_events WHERE org_id = $1 AND action = 'employee.absence_alert.materialize' LIMIT 1",
            )
            .bind(org_id)
            .fetch_one(&pool)
            .await
            .map_err(|err| format!("read changed_count failed: {err}"))?;
            (count, changed)
        };
        assert_eq!(
            event_count, 1,
            "first materialization must emit exactly one audit event"
        );
        assert_eq!(
            changed_count,
            Some(1),
            "the audit event must record the changed row count"
        );

        // Second read over the identical facts: no change → no new audit event.
        let _ = get_absence_exit_dashboard(
            State(state.clone()),
            Extension(principal.clone()),
            Query(query()),
        )
        .await
        .map_err(|err| format!("second dashboard read failed: {err:?}"))?;

        let event_count_after_second: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM audit_events WHERE org_id = $1 AND action = 'employee.absence_alert.materialize'",
        )
        .bind(org_id)
        .fetch_one(&pool)
        .await
        .map_err(|err| format!("recount materialize audit failed: {err}"))?;
        assert_eq!(
            event_count_after_second, 1,
            "an unchanged dashboard read must not emit a second audit event (write-storm guard preserved)"
        );
        Ok(())
    }

    #[cfg(not(feature = "test-postgres"))]
    #[test]
    fn lifecycle_transition_refuses_onboard_and_transfer_when_already_exited() {
        let exited = EmployeeForLifecycle {
            company: "ACME".to_owned(),
            org_unit: None,
            position: None,
            employment_status: "EXITED".to_owned(),
        };
        let base_signoffs = EmployeeLifecycleSignoffs {
            privacy_notice_ack: true,
            korean_labor_law_ack: true,
            ..EmployeeLifecycleSignoffs::default()
        };

        let onboard = NormalizedEmployeeLifecycleTransition {
            event_type: "ONBOARD".to_owned(),
            to_status: "ACTIVE".to_owned(),
            to_company: None,
            to_org_unit: None,
            to_position: None,
            effective_date: "2026-08-14".to_owned(),
            comment: "reinstate".to_owned(),
            signoffs: base_signoffs.clone(),
        };
        let onboard_err = validate_lifecycle_transition(&exited, &onboard).unwrap_err();
        assert_eq!(onboard_err.status, StatusCode::CONFLICT);
        assert_eq!(onboard_err.code, "invalid_transition");
        assert!(
            onboard_err.message.contains("EXITED"),
            "refusal must name the EXITED state, got: {}",
            onboard_err.message
        );

        let transfer = NormalizedEmployeeLifecycleTransition {
            event_type: "TRANSFER".to_owned(),
            to_status: "ACTIVE".to_owned(),
            to_company: Some("ACME-SUBSIDIARY".to_owned()),
            to_org_unit: None,
            to_position: None,
            effective_date: "2026-08-14".to_owned(),
            comment: "transfer".to_owned(),
            signoffs: base_signoffs,
        };
        let transfer_err = validate_lifecycle_transition(&exited, &transfer).unwrap_err();
        assert_eq!(transfer_err.status, StatusCode::CONFLICT);
        assert_eq!(transfer_err.code, "invalid_transition");
        assert!(
            transfer_err.message.contains("EXITED"),
            "refusal must name the EXITED state, got: {}",
            transfer_err.message
        );
    }
}
