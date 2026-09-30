//! Branch-scoped authorization policy engine.
//!
//! The policy has two independent gates:
//! 1. feature permission from the inherited role matrix;
//! 2. resource `branch_id` membership from the kernel [`BranchScope`].
//!
//! Both gates default-deny. Repository adapters should use [`repository_filter`]
//! when listing branch-scoped rows so missing scope checks are difficult to
//! express accidentally.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

use std::collections::{BTreeMap, BTreeSet};
use std::str::FromStr;

use console_kernel_core::{
    AccessScope, AccessScopeLevel, BranchId, BranchProjection, BranchScope, KernelError, OrgId,
    ServicePrincipalId, Timestamp, UserId,
};
use sqlx::{PgPool, Postgres, Row, Transaction};

pub mod cedar_pbac;
mod resource_branch;
mod temporal;

pub use resource_branch::{BranchScopedResource, ResourceBranch};
pub use temporal::{GrantValidity, enforce_assignment_non_overlap};

pub use cedar_pbac::{
    AuthorizationAuditEvent, AuthorizationContext, AuthorizationDecision,
    AuthorizationMetricLabels, AuthorizationRequest, AuthorizationResource, AuthorizationSubject,
    CedarEvaluation, CoexistenceMapEntry, CompiledBundleCacheKey, DecisionEffect, DecisionEngine,
    DecisionReason, DualEngineMode, RlsScopeProof, RlsScopeProofSource, SubjectFreshness,
    SubjectFreshnessRequirement, evaluate_cedar_pbac_boundary, evaluate_legacy_contract,
    observe_cedar_pbac_decision,
};

/// Canonical role codes stored in `users.roles`.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
pub enum Role {
    #[serde(rename = "SUPER_ADMIN")]
    SuperAdmin,
    #[serde(rename = "ADMIN")]
    Admin,
    #[serde(rename = "MECHANIC")]
    Mechanic,
    #[serde(rename = "RECEPTIONIST")]
    Receptionist,
    #[serde(rename = "EXECUTIVE")]
    Executive,
    /// Lowest-privilege tier. The default role for an open self-service signup
    /// (#38): a freshly self-registered account can sign in but sees almost
    /// nothing until an admin elevates it. Deliberately the bottom of the matrix
    /// (`matrix_index` 0) with `Login` as its only `Allow` cell.
    #[serde(rename = "MEMBER")]
    Member,
}

impl Role {
    pub const ALL: [Self; 6] = [
        Self::Member,
        Self::Receptionist,
        Self::Mechanic,
        Self::Admin,
        Self::Executive,
        Self::SuperAdmin,
    ];

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SuperAdmin => "SUPER_ADMIN",
            Self::Admin => "ADMIN",
            Self::Mechanic => "MECHANIC",
            Self::Receptionist => "RECEPTIONIST",
            Self::Executive => "EXECUTIVE",
            Self::Member => "MEMBER",
        }
    }

    const fn matrix_index(self) -> usize {
        match self {
            Self::Member => 0,
            Self::Receptionist => 1,
            Self::Mechanic => 2,
            Self::Admin => 3,
            Self::Executive => 4,
            Self::SuperAdmin => 5,
        }
    }
}

impl FromStr for Role {
    type Err = KernelError;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        match raw {
            "SUPER_ADMIN" => Ok(Self::SuperAdmin),
            "ADMIN" => Ok(Self::Admin),
            "MECHANIC" => Ok(Self::Mechanic),
            "RECEPTIONIST" => Ok(Self::Receptionist),
            "EXECUTIVE" => Ok(Self::Executive),
            "MEMBER" => Ok(Self::Member),
            _ => Err(KernelError::validation(format!("unknown role code: {raw}"))),
        }
    }
}

/// Feature/action rows from the inherited permission matrix.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum Feature {
    Login,
    WorkOrderCreate,
    WorkOrderEditIntake,
    WorkOrderReadAll,
    WorkOrderStart,
    WorkReportSubmit,
    EvidenceAttach,
    PriorityManage,
    AssigneeManage,
    TargetManage,
    CompletionReview,
    /// Delegate finalization for generalized approval documents. Author
    /// finalization is owner-checked by the workflow runtime; delegate mode uses
    /// this policy gate and records the inert Cedar shadow.
    ApprovalFinalize,
    DailyPlanRequest,
    DailyPlanReview,
    /// Org-wide read of the work-order + daily-plan queues regardless of branch
    /// membership. EXECUTIVE + SUPER_ADMIN only — it widens triage visibility to
    /// every branch in the tenant (RLS still confines it to the caller's org),
    /// matching [`resolve_branch_scope_in_org`]'s org-wide tier. A branch-scoped
    /// ADMIN stays confined to its branches. The future "org-admin" custom role
    /// will hold this capability (see docs/specs/rbac-configurable.md).
    OrgWideQueueTriage,
    KpiRead,
    KpiExclusionManage,
    UserManage,
    SubordinateUserCreate,
    ElevatedRoleGrant,
    /// Define tenant-owned custom role policies. Initially SUPER_ADMIN-only;
    /// custom role assignment/effective-policy publication is held behind the
    /// policy-studio safety gates in docs/specs/rbac-configurable.md.
    RoleManage,
    /// Create/rename regions (지역) during org setup.
    RegionManage,
    /// Create/rename branches (지점) during org setup.
    BranchManage,
    /// Create/update/soft-delete equipment master rows (지게차) outside the
    /// bulk master-list import path.
    EquipmentManage,
    MasterListImport,
    RentalQuoteManage,
    EquipmentCostLedgerRead,
    EquipmentCostLedgerWrite,
    PurchaseRequestCreate,
    PurchaseRequestRead,
    PurchaseRequestApprove,
    PurchaseFinalApprove,
    PurchaseExecute,
    InspectionScheduleManage,
    InspectionRoundComplete,
    AuditLogRead,
    ExcelDownload,
    /// Read the per-tenant operational dashboard (work-order funnel, SLA risk,
    /// utilization, equipment/substitution rollups). The current route is an
    /// org-wide picture with no branch filter and therefore also passes through
    /// `authorize_org_wide`; that makes SUPER_ADMIN the only built-in role that
    /// satisfies both gates. The ADMIN matrix cell is retained for a future
    /// branch-filtered projection, not authority over today's tenant-wide route.
    OpsDashboardRead,
    /// Manage the public sales catalog (#6 지게차 매매): create/update/withdraw
    /// used-forklift listings and triage inbound customer inquiries. ADMIN tier.
    SalesManage,
    /// Receive an advance shipping notice and record warehouse receipt
    /// exceptions. Custom-grant only: no built-in role fallback.
    LogisticsReceive,
    /// Complete governed putaway into a warehouse stock location. Custom-grant
    /// only: no built-in role fallback.
    LogisticsPutaway,
    /// Release a fulfillment order and reserve available stock. Custom-grant
    /// only: no built-in role fallback.
    LogisticsRelease,
    /// Pick and pack released inventory. Custom-grant only: no built-in role
    /// fallback.
    LogisticsPickPack,
    /// Dispatch a packed shipment through its assigned carrier/vehicle leg.
    /// Custom-grant only: no built-in role fallback.
    LogisticsDispatch,
    /// Record recipient-confirmed proof of delivery with immutable evidence.
    /// Custom-grant only: no built-in role fallback.
    LogisticsPod,
    /// Derive the SLA assessment and operational-cost settlement after delivery.
    /// Custom-grant only: no built-in role fallback.
    LogisticsSettle,
    /// Read IV inventory stock objects/events inside the caller's branch scope.
    InventoryRead,
    /// Create/update/archive IV inventory items, locations, and thresholds.
    InventoryManage,
    /// Consume IV inventory stock against visible work orders or P1 dispatches.
    InventoryConsume,
    /// Prepare future reorder/purchase requests from low-stock IV items.
    InventoryReorder,
    /// Read tenant benefit-catalog policy rows and console-ready legal/extra benefit details.
    BenefitCatalogRead,
    /// Create and maintain tenant benefit-catalog rows, tiers, and conditions.
    BenefitCatalogManage,
    /// Dedicated consulting pilot read gate. Kept distinct from benefit policy
    /// management so rollout may remain dark without borrowing authority.
    ConsultingRead,
    /// Dedicated consulting pilot mutation gate.
    ConsultingManage,
    /// Read the tenant compliance domain (regulations, obligations, frameworks,
    /// and controls). Org-wide catalog rows require an org-wide principal;
    /// branch-scoped obligations are separately authorized at their branch.
    ComplianceDomainRead,
    /// Create tenant compliance-domain rows and append compliance relations.
    /// Org-wide resources require org-wide authorization.
    ComplianceDomainManage,
    /// Link evidence to a compliance control or obligation. This is separate
    /// from generic work-report evidence attachment because it changes the
    /// audited compliance evidence register.
    ComplianceEvidenceLink,
    /// Permission metadata for the future AI assistant seam. T0.6 requires the
    /// 22-feature matrix; this does not implement an AI adapter or demo mode.
    AiAssist,
    /// Read governance findings from the integrity engine.
    /// EXECUTIVE + SUPER_ADMIN only — labor-law sensitivity; an ADMIN must NOT
    /// read findings about themselves or their subordinates.
    IntegrityFindingsRead,
    /// Triage (OPEN → REVIEWED / DISMISSED / ESCALATED) a governance finding.
    /// Gated identically to read; triage is itself audited via `with_audit`.
    IntegrityFindingTriage,
    /// Configure the tenant's corporate webmail account (SMTP/IMAP host, port,
    /// credentials). Stores credentials write-only (envelope AEAD); every change
    /// is audited. ADMIN + SUPER_ADMIN only — it holds the mailbox secrets.
    MailAccountManage,
    /// Use the configured webmail: send / reply / forward, and (in later
    /// batches) read inbound threads. RECEPTIONIST + ADMIN + EXECUTIVE +
    /// SUPER_ADMIN; MECHANIC is excluded (work lives in the messenger surface).
    MailUse,
    /// Read the tenant HR employee directory. EXECUTIVE + ADMIN + SUPER_ADMIN only.
    EmployeeDirectoryRead,
    /// Import/manage tenant HR employee rows. ADMIN + SUPER_ADMIN only; employees
    /// are deliberately not auth users.
    EmployeeDirectoryManage,
    /// Report a staff exit case (무단결근 → 퇴사 보고). The branch/site manager
    /// tier — ADMIN + SUPER_ADMIN. Distinct from HR/HQ confirmation so a site
    /// manager can open a case without holding the confirmation authority.
    ExitCaseReport,
    /// First-tier (HR) confirmation of a reported exit case. ADMIN + SUPER_ADMIN.
    /// Separation of duties: the code additionally requires the HQ confirmer to
    /// differ from the recorded HR confirmer, so holding this capability is
    /// necessary but not sufficient to also HQ-confirm the same case.
    ExitCaseHrConfirm,
    /// Second-tier (HQ) confirmation of an already HR-confirmed exit case.
    /// EXECUTIVE + SUPER_ADMIN — the headquarters/leadership tier, mirroring the
    /// org-wide tier of [`OrgWideQueueTriage`]. The client `hq_confirmation`
    /// boolean is NOT the authority: this capability plus a prior HR_CONFIRMED by
    /// a DIFFERENT actor is.
    ExitCaseHqConfirm,
    /// Generate/draft the exit settlement package + submit it for approval
    /// (퇴직 정산 초안/승인 상신). ADMIN + SUPER_ADMIN — the HR settlement tier.
    ExitSettlementManage,
    /// Manage attendance exceptions. ADMIN + SUPER_ADMIN only; the Attendance
    /// REST boundary continues to enforce branch/resource scope independently.
    AttendanceExceptionManage,
    /// Manage attendance substitutions. ADMIN + SUPER_ADMIN only; the
    /// Attendance REST boundary continues to enforce branch/resource scope independently.
    AttendanceSubstitutionManage,
    /// Read the CEO/top-clearance covert audit stream. This is deliberately
    /// denied by the legacy role matrix and enrolled only through Cedar/PBAC
    /// clearance facts.
    AuditStreamRead,
    /// Read the audit-of-access stream for CEO/top-clearance audit reads.
    /// Deliberately Cedar-only like [`Self::AuditStreamRead`].
    AuditStreamAccessLogRead,
    /// Lock/unlock payroll & accounting freeze windows (월마감/마감). Locking a
    /// period makes every date-stamping write inside it fail closed, so this is
    /// the close authority: ADMIN + EXECUTIVE + SUPER_ADMIN.
    PeriodLockManage,
    /// Drive the generic object-lifecycle engine (state transitions, legal
    /// hold, retention). ADMIN + SUPER_ADMIN — the records-management tier,
    /// mirroring how payroll admin endpoints gate.
    LifecycleManage,
    /// Read payroll draft-run/line staging rows (`payroll_draft_runs`/
    /// `payroll_draft_lines`, mig 0074). Financial/HR-sensitive; gated via
    /// `authorize_org_wide` because the table has no `branch_id` column to
    /// narrow a branch-scoped ADMIN's visibility. Built-in access is
    /// EXECUTIVE + SUPER_ADMIN only — `authorize_org_wide`'s built-in path
    /// never grants ADMIN (all-branch-scoped or not), matching
    /// `EmployeeDirectoryRead`/`OrgWideQueueTriage` today; an ADMIN's only
    /// path in is a custom org-wide PBAC grant. See the payroll REST crate's
    /// module docs for the deny-by-omission rationale.
    PayrollRunRead,
    /// Drive the payroll run lifecycle (attendance close, calculation,
    /// exception resolution, submit/decide/withdraw, disbursement
    /// scheduling/attestation, payslip issuance). Same matrix row and
    /// org-wide gating as [`Self::PayrollRunRead`]: gated via
    /// `authorize_org_wide` (the tables have `org_id` only), built-in access
    /// EXECUTIVE + SUPER_ADMIN, ADMIN only via a custom org-wide PBAC grant,
    /// branch-scoped callers denied.
    PayrollRunManage,
    /// Create a draft board notice, publish it (issues the NT- code, snapshots
    /// every active org member as a recipient, and fans out a notification to
    /// each), and read 수령확인 progress. The HQ/announcement tier: ADMIN +
    /// EXECUTIVE + SUPER_ADMIN. Reading a PUBLISHED notice is open to any
    /// authenticated org member — this feature gates only draft visibility and
    /// the publish/progress mutations.
    NoticeManage,
    /// Operate the scheduled HVAC preventive-maintenance queue.
    FacilitiesManage,
    /// Triage and assign scheduled facilities cases.
    FacilitiesDispatch,
    /// Start and submit assigned facilities work.
    FacilitiesExecute,
    /// Accept or reject submitted facilities work.
    FacilitiesAccept,
    /// Read facilities cases and record metering observations.
    FacilitiesObserve,
    /// Dedicated workload-only production source ingress. Built-in human roles
    /// are all denied; a registered service principal receives this only via an
    /// explicit tenant-scoped effective grant.
    ProductionSourceIngest,
    /// Register serialized 3R rental units. Custom-grant only: no built-in
    /// role fallback.
    Equipment3rRegistry,
    /// Create idempotent 3R rental-case quotes. Custom-grant only: no built-in
    /// role fallback.
    Equipment3rQuote,
    /// Approve or decline a quoted 3R rental case (four-eyes enforced).
    /// Custom-grant only: no built-in role fallback.
    Equipment3rApprove,
    /// Dispatch an approved 3R case and record customer handover. Custom-grant
    /// only: no built-in role fallback.
    Equipment3rDispatch,
    /// Record on-rent 3R inspections and maintenance. Custom-grant only: no
    /// built-in role fallback.
    Equipment3rInspect,
    /// Record 3R returns and return assessments. Custom-grant only: no
    /// built-in role fallback.
    Equipment3rAssess,
    /// Complete repair/refurbish/resale dispositions. Custom-grant only: no
    /// built-in role fallback.
    Equipment3rDisposition,
    /// Read 3R units, rental cases, and history. Custom-grant only: no
    /// built-in role fallback.
    Equipment3rObserve,
    /// Read the recruiting pipeline (postings, applicants, offers, talent
    /// pool). HR-owned data: ADMIN + EXECUTIVE + SUPER_ADMIN, mirroring
    /// `EmployeeDirectoryRead`. Gated org-wide (`authorize_org_wide`) — the
    /// recruiting tables carry no branch column to narrow by.
    RecruitingRead,
    /// Manage the recruiting pipeline: postings (draft/publish/close),
    /// applicant transitions, offers, and the hire handshake. ADMIN +
    /// SUPER_ADMIN, mirroring `EmployeeDirectoryManage`; hire additionally
    /// requires `EmployeeDirectoryManage` (the owning HR domain's gate).
    RecruitingManage,
    /// Read evaluation cycles, subjects, preflight reports, and the finalized
    /// person ledger. HR-sensitive — mirrors the `EmployeeDirectoryRead` tier
    /// (ADMIN + EXECUTIVE + SUPER_ADMIN); the person-ledger read is itself
    /// audited (`evaluation.history.viewed`).
    EvaluationRead,
    /// Drive the evaluation cycle lifecycle (create/open/start-calibration/
    /// finalize/archive), enroll subjects, replace goals, and calibrate.
    /// Mirrors the `EmployeeDirectoryManage` tier (ADMIN + SUPER_ADMIN); the
    /// calibration handler additionally enforces four-eyes SoD in code.
    EvaluationManage,
    /// Record and submit self/manager review drafts and read one's own
    /// evaluation task list. The per-subject check (caller is the subject's
    /// assigned manager, or holds `EvaluationManage`) is enforced in code with
    /// deny-by-omission 404s.
    EvaluationSubmit,
}

impl Feature {
    pub const ALL: [Self; 96] = [
        Self::Login,
        Self::WorkOrderCreate,
        Self::WorkOrderEditIntake,
        Self::WorkOrderReadAll,
        Self::WorkOrderStart,
        Self::WorkReportSubmit,
        Self::EvidenceAttach,
        Self::PriorityManage,
        Self::AssigneeManage,
        Self::TargetManage,
        Self::CompletionReview,
        Self::ApprovalFinalize,
        Self::DailyPlanRequest,
        Self::DailyPlanReview,
        Self::OrgWideQueueTriage,
        Self::KpiRead,
        Self::KpiExclusionManage,
        Self::UserManage,
        Self::SubordinateUserCreate,
        Self::ElevatedRoleGrant,
        Self::RoleManage,
        Self::RegionManage,
        Self::BranchManage,
        Self::EquipmentManage,
        Self::MasterListImport,
        Self::RentalQuoteManage,
        Self::EquipmentCostLedgerRead,
        Self::EquipmentCostLedgerWrite,
        Self::PurchaseRequestCreate,
        Self::PurchaseRequestRead,
        Self::PurchaseRequestApprove,
        Self::PurchaseFinalApprove,
        Self::PurchaseExecute,
        Self::InspectionScheduleManage,
        Self::InspectionRoundComplete,
        Self::AuditLogRead,
        Self::ExcelDownload,
        Self::OpsDashboardRead,
        Self::SalesManage,
        Self::LogisticsReceive,
        Self::LogisticsPutaway,
        Self::LogisticsRelease,
        Self::LogisticsPickPack,
        Self::LogisticsDispatch,
        Self::LogisticsPod,
        Self::LogisticsSettle,
        Self::InventoryRead,
        Self::InventoryManage,
        Self::InventoryConsume,
        Self::InventoryReorder,
        Self::BenefitCatalogRead,
        Self::BenefitCatalogManage,
        Self::ConsultingRead,
        Self::ConsultingManage,
        Self::ComplianceDomainRead,
        Self::ComplianceDomainManage,
        Self::ComplianceEvidenceLink,
        Self::AiAssist,
        Self::IntegrityFindingsRead,
        Self::IntegrityFindingTriage,
        Self::MailAccountManage,
        Self::MailUse,
        Self::EmployeeDirectoryRead,
        Self::EmployeeDirectoryManage,
        Self::ExitCaseReport,
        Self::ExitCaseHrConfirm,
        Self::ExitCaseHqConfirm,
        Self::ExitSettlementManage,
        Self::AttendanceExceptionManage,
        Self::AttendanceSubstitutionManage,
        Self::AuditStreamRead,
        Self::AuditStreamAccessLogRead,
        Self::PeriodLockManage,
        Self::LifecycleManage,
        Self::PayrollRunRead,
        Self::PayrollRunManage,
        Self::NoticeManage,
        Self::FacilitiesManage,
        Self::FacilitiesDispatch,
        Self::FacilitiesExecute,
        Self::FacilitiesAccept,
        Self::FacilitiesObserve,
        Self::ProductionSourceIngest,
        Self::Equipment3rRegistry,
        Self::Equipment3rQuote,
        Self::Equipment3rApprove,
        Self::Equipment3rDispatch,
        Self::Equipment3rInspect,
        Self::Equipment3rAssess,
        Self::Equipment3rDisposition,
        Self::Equipment3rObserve,
        Self::RecruitingRead,
        Self::RecruitingManage,
        Self::EvaluationRead,
        Self::EvaluationManage,
        Self::EvaluationSubmit,
    ];

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Login => "login",
            Self::WorkOrderCreate => "work_order_create",
            Self::WorkOrderEditIntake => "work_order_edit_intake",
            Self::WorkOrderReadAll => "work_order_read_all",
            Self::WorkOrderStart => "work_order_start",
            Self::WorkReportSubmit => "work_report_submit",
            Self::EvidenceAttach => "evidence_attach",
            Self::PriorityManage => "priority_manage",
            Self::AssigneeManage => "assignee_manage",
            Self::TargetManage => "target_manage",
            Self::CompletionReview => "completion_review",
            Self::ApprovalFinalize => "approval_finalize",
            Self::DailyPlanRequest => "daily_plan_request",
            Self::DailyPlanReview => "daily_plan_review",
            Self::OrgWideQueueTriage => "org_wide_queue_triage",
            Self::KpiRead => "kpi_read",
            Self::KpiExclusionManage => "kpi_exclusion_manage",
            Self::UserManage => "user_manage",
            Self::SubordinateUserCreate => "subordinate_user_create",
            Self::ElevatedRoleGrant => "elevated_role_grant",
            Self::RoleManage => "role_manage",
            Self::RegionManage => "region_manage",
            Self::BranchManage => "branch_manage",
            Self::EquipmentManage => "equipment_manage",
            Self::MasterListImport => "master_list_import",
            Self::RentalQuoteManage => "rental_quote_manage",
            Self::EquipmentCostLedgerRead => "equipment_cost_ledger_read",
            Self::EquipmentCostLedgerWrite => "equipment_cost_ledger_write",
            Self::PurchaseRequestCreate => "purchase_request_create",
            Self::PurchaseRequestRead => "purchase_request_read",
            Self::PurchaseRequestApprove => "purchase_request_approve",
            Self::PurchaseFinalApprove => "purchase_final_approve",
            Self::PurchaseExecute => "purchase_execute",
            Self::InspectionScheduleManage => "inspection_schedule_manage",
            Self::InspectionRoundComplete => "inspection_round_complete",
            Self::AuditLogRead => "audit_log_read",
            Self::ExcelDownload => "excel_download",
            Self::OpsDashboardRead => "ops_dashboard_read",
            Self::SalesManage => "sales_manage",
            Self::LogisticsReceive => "logistics_receive",
            Self::LogisticsPutaway => "logistics_putaway",
            Self::LogisticsRelease => "logistics_release",
            Self::LogisticsPickPack => "logistics_pick_pack",
            Self::LogisticsDispatch => "logistics_dispatch",
            Self::LogisticsPod => "logistics_pod",
            Self::LogisticsSettle => "logistics_settle",
            Self::InventoryRead => "inventory_read",
            Self::InventoryManage => "inventory_manage",
            Self::InventoryConsume => "inventory_consume",
            Self::InventoryReorder => "inventory_reorder",
            Self::BenefitCatalogRead => "benefit_catalog_read",
            Self::BenefitCatalogManage => "benefit_catalog_manage",
            Self::ConsultingRead => "consulting_read",
            Self::ConsultingManage => "consulting_manage",
            Self::ComplianceDomainRead => "compliance_domain_read",
            Self::ComplianceDomainManage => "compliance_domain_manage",
            Self::ComplianceEvidenceLink => "compliance_evidence_link",
            Self::AiAssist => "ai_assist",
            Self::IntegrityFindingsRead => "integrity_findings_read",
            Self::IntegrityFindingTriage => "integrity_finding_triage",
            Self::MailAccountManage => "mail_account_manage",
            Self::MailUse => "mail_use",
            Self::EmployeeDirectoryRead => "employee_directory_read",
            Self::EmployeeDirectoryManage => "employee_directory_manage",
            Self::ExitCaseReport => "exit_case_report",
            Self::ExitCaseHrConfirm => "exit_case_hr_confirm",
            Self::ExitCaseHqConfirm => "exit_case_hq_confirm",
            Self::ExitSettlementManage => "exit_settlement_manage",
            Self::AttendanceExceptionManage => "attendance_exception_manage",
            Self::AttendanceSubstitutionManage => "attendance_substitution_manage",
            Self::AuditStreamRead => "audit_stream_read",
            Self::AuditStreamAccessLogRead => "audit_stream_access_log_read",
            Self::PeriodLockManage => "period_lock_manage",
            Self::LifecycleManage => "lifecycle_manage",
            Self::PayrollRunRead => "payroll_run_read",
            Self::PayrollRunManage => "payroll_run_manage",
            Self::NoticeManage => "notice_manage",
            Self::FacilitiesManage => "facilities_manage",
            Self::FacilitiesDispatch => "facilities_dispatch",
            Self::FacilitiesExecute => "facilities_execute",
            Self::FacilitiesAccept => "facilities_accept",
            Self::FacilitiesObserve => "facilities_observe",
            Self::ProductionSourceIngest => "production_source_ingest",
            Self::Equipment3rRegistry => "equipment_3r_registry",
            Self::Equipment3rQuote => "equipment_3r_quote",
            Self::Equipment3rApprove => "equipment_3r_approve",
            Self::Equipment3rDispatch => "equipment_3r_dispatch",
            Self::Equipment3rInspect => "equipment_3r_inspect",
            Self::Equipment3rAssess => "equipment_3r_assess",
            Self::Equipment3rDisposition => "equipment_3r_disposition",
            Self::Equipment3rObserve => "equipment_3r_observe",
            Self::RecruitingRead => "recruiting_read",
            Self::RecruitingManage => "recruiting_manage",
            Self::EvaluationRead => "evaluation_read",
            Self::EvaluationManage => "evaluation_manage",
            Self::EvaluationSubmit => "evaluation_submit",
        }
    }

    const fn matrix_row(self) -> [PermissionLevel; 6] {
        use PermissionLevel::{Allow as A, Deny as D, Limited as L, RequestOnly as R};

        // Column order matches `Role::matrix_index`:
        // [MEMBER, RECEPTIONIST, MECHANIC, ADMIN, EXECUTIVE, SUPER_ADMIN].
        // MEMBER (the open-signup default) is default-DENY everywhere except
        // `Login`: a self-registered account can authenticate but sees nothing
        // actionable until an admin grants it a real role.
        match self {
            Self::Login => [A, A, A, A, A, A],
            Self::WorkOrderCreate => [D, A, L, A, L, A],
            Self::WorkOrderEditIntake => [D, A, L, A, L, A],
            Self::WorkOrderReadAll => [D, A, A, A, A, A],
            Self::WorkOrderStart => [D, L, A, A, L, A],
            Self::WorkReportSubmit => [D, L, A, A, L, A],
            Self::EvidenceAttach => [D, A, A, A, L, A],
            Self::PriorityManage => [D, D, D, A, D, A],
            Self::AssigneeManage => [D, D, D, A, D, A],
            Self::TargetManage => [D, D, R, A, D, A],
            Self::CompletionReview => [D, D, D, A, D, A],
            Self::ApprovalFinalize => [D, D, D, A, A, A],
            Self::DailyPlanRequest => [D, D, A, A, D, A],
            Self::DailyPlanReview => [D, D, D, A, D, A],
            // Org-wide queue read: EXECUTIVE + SUPER_ADMIN only, matching the
            // org-wide tier of `resolve_branch_scope_in_org`. A branch ADMIN is
            // deliberately NOT here — it stays confined to its branch scope.
            Self::OrgWideQueueTriage => [D, D, D, D, A, A],
            Self::KpiRead => [D, D, D, A, A, A],
            Self::KpiExclusionManage => [D, D, D, A, A, A],
            Self::UserManage => [D, D, D, A, D, A],
            Self::SubordinateUserCreate => [D, D, D, L, D, A],
            Self::ElevatedRoleGrant => [D, D, D, D, D, A],
            Self::RoleManage => [D, D, D, D, D, A],
            Self::RegionManage => [D, D, D, A, A, A],
            Self::BranchManage => [D, D, D, A, A, A],
            Self::EquipmentManage => [D, D, D, A, A, A],
            Self::MasterListImport => [D, D, D, A, D, A],
            Self::RentalQuoteManage => [D, A, D, A, A, A],
            Self::EquipmentCostLedgerRead => [D, D, D, A, A, A],
            Self::EquipmentCostLedgerWrite => [D, D, D, A, D, A],
            Self::PurchaseRequestCreate => [D, A, R, A, D, A],
            Self::PurchaseRequestRead => [D, A, L, A, A, A],
            Self::PurchaseRequestApprove => [D, D, D, A, D, A],
            Self::PurchaseFinalApprove => [D, D, D, D, A, A],
            Self::PurchaseExecute => [D, A, D, A, D, A],
            Self::InspectionScheduleManage => [D, D, D, A, D, A],
            Self::InspectionRoundComplete => [D, D, A, A, D, A],
            Self::AuditLogRead => [D, D, D, A, D, A],
            Self::ExcelDownload => [D, A, A, A, A, A],
            Self::OpsDashboardRead => [D, D, D, A, D, A],
            Self::SalesManage => [D, D, D, A, A, A],
            // Logistics is capability-driven from its first slice. Built-in
            // roles do not silently widen access; explicit tenant grants are
            // the only allow path.
            Self::LogisticsReceive
            | Self::LogisticsPutaway
            | Self::LogisticsRelease
            | Self::LogisticsPickPack
            | Self::LogisticsDispatch
            | Self::LogisticsPod
            | Self::LogisticsSettle => [D, D, D, D, D, D],
            // IV inventory is branch-operational: front-office/mechanics may
            // read stock; mechanics/admins may consume; management/reorder
            // remains the admin tier until purchase integration lands.
            Self::InventoryRead => [D, A, A, A, A, A],
            Self::InventoryManage => [D, D, D, A, D, A],
            Self::InventoryConsume => [D, D, A, A, D, A],
            Self::InventoryReorder => [D, D, D, A, D, A],
            // Benefits are HR/legal policy data. Admin/super-admin can manage;
            // executives can read org-wide catalog state for policy oversight.
            Self::BenefitCatalogRead => [D, D, D, A, A, A],
            Self::BenefitCatalogManage => [D, D, D, A, D, A],
            // Deliberately deny every built-in role while the console is dark.
            Self::ConsultingRead => [D, D, D, D, D, D],
            Self::ConsultingManage => [D, D, D, D, D, D],
            // Compliance's org-owned catalog objects are additionally guarded
            // by the REST boundary's org-wide scope check. The ADMIN cells keep
            // branch-scoped obligation work available without granting an
            // arbitrary branch an org-wide regulation/framework/evidence view.
            Self::ComplianceDomainRead => [D, D, D, A, A, A],
            Self::ComplianceDomainManage => [D, D, D, A, D, A],
            Self::ComplianceEvidenceLink => [D, D, D, A, D, A],
            // Retain the historical wire key without granting an AI capability in v1.
            Self::AiAssist => [D, D, D, D, D, D],
            // Integrity findings are labor-law sensitive: ADMIN must not read
            // findings about themselves. EXECUTIVE + SUPER_ADMIN only.
            Self::IntegrityFindingsRead => [D, D, D, D, A, A],
            Self::IntegrityFindingTriage => [D, D, D, D, A, A],
            // Configuring the mailbox holds the tenant's mail secrets: ADMIN +
            // SUPER_ADMIN only.
            Self::MailAccountManage => [D, D, D, A, D, A],
            // Sending/replying/forwarding mail: front-office + leadership.
            // MECHANIC is excluded (their workflow is the messenger surface).
            Self::MailUse => [D, A, D, A, A, A],
            Self::EmployeeDirectoryRead => [D, D, D, A, A, A],
            Self::EmployeeDirectoryManage => [D, D, D, A, D, A],
            // Separation of duties across the absence → exit → settlement chain.
            // Report + HR confirm + settlement are the branch HR/manager tier
            // (ADMIN + SUPER_ADMIN); HQ confirm is the org-wide leadership tier
            // (EXECUTIVE + SUPER_ADMIN), matching OrgWideQueueTriage. A single
            // SUPER_ADMIN could hold both confirm cells, but the confirm handler's
            // distinct-actor check still forbids one person from doing both tiers
            // on the same case.
            Self::ExitCaseReport => [D, D, D, A, D, A],
            Self::ExitCaseHrConfirm => [D, D, D, A, D, A],
            Self::ExitCaseHqConfirm => [D, D, D, D, A, A],
            Self::ExitSettlementManage => [D, D, D, A, D, A],
            Self::AttendanceExceptionManage | Self::AttendanceSubstitutionManage => {
                [D, D, D, A, D, A]
            }
            // B26b covert audit stream: no legacy/static-role fallback. Cedar
            // clearance facts are the only allow path; omission denies.
            Self::AuditStreamRead | Self::AuditStreamAccessLogRead => [D, D, D, D, D, D],
            // Freezing a payroll/accounting period is close authority: the
            // branch close tier (ADMIN) plus org-wide leadership (EXECUTIVE),
            // and SUPER_ADMIN.
            Self::PeriodLockManage => [D, D, D, A, A, A],
            Self::LifecycleManage => [D, D, D, A, D, A],
            // Payroll draft-run/line staging read: financial/HR-sensitive,
            // same tier as EmployeeDirectoryRead. Gated via
            // `authorize_org_wide` (requires BranchScope::All) since the
            // table has no branch_id to narrow a branch-scoped ADMIN.
            Self::PayrollRunRead => [D, D, D, A, A, A],
            // Payroll run lifecycle writes: identical row + org-wide gating to
            // PayrollRunRead (ADMIN cell only ever reachable through a custom
            // org-wide PBAC grant — `authorize_org_wide`'s built-in path never
            // grants ADMIN).
            Self::PayrollRunManage => [D, D, D, A, A, A],
            // The HQ/announcement tier: ADMIN + EXECUTIVE + SUPER_ADMIN.
            Self::NoticeManage => [D, D, D, A, A, A],
            Self::FacilitiesManage => [D, D, D, A, D, A],
            Self::FacilitiesDispatch => [D, D, D, A, D, A],
            Self::FacilitiesExecute => [D, D, A, A, D, A],
            Self::FacilitiesAccept => [D, D, D, A, D, A],
            Self::FacilitiesObserve => [D, A, A, A, A, A],
            Self::ProductionSourceIngest => [D, D, D, D, D, D],
            // Equipment 3R is capability-driven from its first slice. Built-in
            // roles do not silently widen access; explicit tenant grants are
            // the only allow path.
            Self::Equipment3rRegistry
            | Self::Equipment3rQuote
            | Self::Equipment3rApprove
            | Self::Equipment3rDispatch
            | Self::Equipment3rInspect
            | Self::Equipment3rAssess
            | Self::Equipment3rDisposition
            | Self::Equipment3rObserve => [D, D, D, D, D, D],
            // Recruiting mirrors the HR directory pair: recruiting rows are
            // HR-owned data with EXECUTIVE read-only visibility.
            Self::RecruitingRead => [D, D, D, A, A, A],
            Self::RecruitingManage => [D, D, D, A, D, A],
            // Evaluation is HR-sensitive: read/submit mirror the
            // EmployeeDirectoryRead tier, manage mirrors EmployeeDirectoryManage.
            // The per-subject submit check and calibration four-eyes SoD are
            // additionally enforced in the evaluation REST/adapter code.
            Self::EvaluationRead => [D, D, D, A, A, A],
            Self::EvaluationManage => [D, D, D, A, D, A],
            Self::EvaluationSubmit => [D, D, D, A, A, A],
        }
    }
}

impl FromStr for Feature {
    type Err = KernelError;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        match raw {
            "login" => Ok(Self::Login),
            "work_order_create" => Ok(Self::WorkOrderCreate),
            "work_order_edit_intake" => Ok(Self::WorkOrderEditIntake),
            "work_order_read_all" => Ok(Self::WorkOrderReadAll),
            "work_order_start" => Ok(Self::WorkOrderStart),
            "work_report_submit" => Ok(Self::WorkReportSubmit),
            "evidence_attach" => Ok(Self::EvidenceAttach),
            "priority_manage" => Ok(Self::PriorityManage),
            "assignee_manage" => Ok(Self::AssigneeManage),
            "target_manage" => Ok(Self::TargetManage),
            "completion_review" => Ok(Self::CompletionReview),
            "approval_finalize" => Ok(Self::ApprovalFinalize),
            "daily_plan_request" => Ok(Self::DailyPlanRequest),
            "daily_plan_review" => Ok(Self::DailyPlanReview),
            "org_wide_queue_triage" => Ok(Self::OrgWideQueueTriage),
            "kpi_read" => Ok(Self::KpiRead),
            "kpi_exclusion_manage" => Ok(Self::KpiExclusionManage),
            "user_manage" => Ok(Self::UserManage),
            "subordinate_user_create" => Ok(Self::SubordinateUserCreate),
            "elevated_role_grant" => Ok(Self::ElevatedRoleGrant),
            "role_manage" => Ok(Self::RoleManage),
            "region_manage" => Ok(Self::RegionManage),
            "branch_manage" => Ok(Self::BranchManage),
            "equipment_manage" => Ok(Self::EquipmentManage),
            "master_list_import" => Ok(Self::MasterListImport),
            "rental_quote_manage" => Ok(Self::RentalQuoteManage),
            "equipment_cost_ledger_read" => Ok(Self::EquipmentCostLedgerRead),
            "equipment_cost_ledger_write" => Ok(Self::EquipmentCostLedgerWrite),
            "purchase_request_create" => Ok(Self::PurchaseRequestCreate),
            "purchase_request_read" => Ok(Self::PurchaseRequestRead),
            "purchase_request_approve" => Ok(Self::PurchaseRequestApprove),
            "purchase_final_approve" => Ok(Self::PurchaseFinalApprove),
            "purchase_execute" => Ok(Self::PurchaseExecute),
            "inspection_schedule_manage" => Ok(Self::InspectionScheduleManage),
            "inspection_round_complete" => Ok(Self::InspectionRoundComplete),
            "audit_log_read" => Ok(Self::AuditLogRead),
            "excel_download" => Ok(Self::ExcelDownload),
            "ops_dashboard_read" => Ok(Self::OpsDashboardRead),
            "sales_manage" => Ok(Self::SalesManage),
            "logistics_receive" => Ok(Self::LogisticsReceive),
            "logistics_putaway" => Ok(Self::LogisticsPutaway),
            "logistics_release" => Ok(Self::LogisticsRelease),
            "logistics_pick_pack" => Ok(Self::LogisticsPickPack),
            "logistics_dispatch" => Ok(Self::LogisticsDispatch),
            "logistics_pod" => Ok(Self::LogisticsPod),
            "logistics_settle" => Ok(Self::LogisticsSettle),
            "inventory_read" => Ok(Self::InventoryRead),
            "inventory_manage" => Ok(Self::InventoryManage),
            "inventory_consume" => Ok(Self::InventoryConsume),
            "inventory_reorder" => Ok(Self::InventoryReorder),
            "benefit_catalog_read" => Ok(Self::BenefitCatalogRead),
            "benefit_catalog_manage" => Ok(Self::BenefitCatalogManage),
            "consulting_read" => Ok(Self::ConsultingRead),
            "consulting_manage" => Ok(Self::ConsultingManage),
            "compliance_domain_read" => Ok(Self::ComplianceDomainRead),
            "compliance_domain_manage" => Ok(Self::ComplianceDomainManage),
            "compliance_evidence_link" => Ok(Self::ComplianceEvidenceLink),
            "ai_assist" => Ok(Self::AiAssist),
            "integrity_findings_read" => Ok(Self::IntegrityFindingsRead),
            "integrity_finding_triage" => Ok(Self::IntegrityFindingTriage),
            "mail_account_manage" => Ok(Self::MailAccountManage),
            "mail_use" => Ok(Self::MailUse),
            "employee_directory_read" => Ok(Self::EmployeeDirectoryRead),
            "employee_directory_manage" => Ok(Self::EmployeeDirectoryManage),
            "exit_case_report" => Ok(Self::ExitCaseReport),
            "exit_case_hr_confirm" => Ok(Self::ExitCaseHrConfirm),
            "exit_case_hq_confirm" => Ok(Self::ExitCaseHqConfirm),
            "exit_settlement_manage" => Ok(Self::ExitSettlementManage),
            "attendance_exception_manage" => Ok(Self::AttendanceExceptionManage),
            "attendance_substitution_manage" => Ok(Self::AttendanceSubstitutionManage),
            "audit_stream_read" => Ok(Self::AuditStreamRead),
            "audit_stream_access_log_read" => Ok(Self::AuditStreamAccessLogRead),
            "period_lock_manage" => Ok(Self::PeriodLockManage),
            "lifecycle_manage" => Ok(Self::LifecycleManage),
            "payroll_run_read" => Ok(Self::PayrollRunRead),
            "payroll_run_manage" => Ok(Self::PayrollRunManage),
            "notice_manage" => Ok(Self::NoticeManage),
            "facilities_manage" => Ok(Self::FacilitiesManage),
            "facilities_dispatch" => Ok(Self::FacilitiesDispatch),
            "facilities_execute" => Ok(Self::FacilitiesExecute),
            "facilities_accept" => Ok(Self::FacilitiesAccept),
            "facilities_observe" => Ok(Self::FacilitiesObserve),
            "production_source_ingest" => Ok(Self::ProductionSourceIngest),
            "equipment_3r_registry" => Ok(Self::Equipment3rRegistry),
            "equipment_3r_quote" => Ok(Self::Equipment3rQuote),
            "equipment_3r_approve" => Ok(Self::Equipment3rApprove),
            "equipment_3r_dispatch" => Ok(Self::Equipment3rDispatch),
            "equipment_3r_inspect" => Ok(Self::Equipment3rInspect),
            "equipment_3r_assess" => Ok(Self::Equipment3rAssess),
            "equipment_3r_disposition" => Ok(Self::Equipment3rDisposition),
            "equipment_3r_observe" => Ok(Self::Equipment3rObserve),
            "recruiting_read" => Ok(Self::RecruitingRead),
            "recruiting_manage" => Ok(Self::RecruitingManage),
            "evaluation_read" => Ok(Self::EvaluationRead),
            "evaluation_manage" => Ok(Self::EvaluationManage),
            "evaluation_submit" => Ok(Self::EvaluationSubmit),
            _ => Err(KernelError::validation(format!(
                "unknown feature key: {raw}"
            ))),
        }
    }
}

/// Permission-cell semantics from the inherited Korean matrix.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionLevel {
    Deny,
    RequestOnly,
    Limited,
    Allow,
}

impl PermissionLevel {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Deny => "deny",
            Self::RequestOnly => "request_only",
            Self::Limited => "limited",
            Self::Allow => "allow",
        }
    }

    const fn satisfies(self, required: Self) -> bool {
        match required {
            Self::Deny => true,
            Self::Allow => matches!(self, Self::Allow),
            Self::Limited => matches!(self, Self::Allow | Self::Limited),
            Self::RequestOnly => matches!(self, Self::Allow | Self::RequestOnly),
        }
    }
}

impl FromStr for PermissionLevel {
    type Err = KernelError;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        match raw {
            "deny" => Ok(Self::Deny),
            "request_only" => Ok(Self::RequestOnly),
            "limited" => Ok(Self::Limited),
            "allow" => Ok(Self::Allow),
            _ => Err(KernelError::validation(format!(
                "unknown permission level: {raw}"
            ))),
        }
    }
}

/// A requested authorization operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Action {
    feature: Feature,
    required: PermissionLevel,
}

impl Action {
    /// Full/direct use of a feature (`가능` cells only).
    #[must_use]
    pub const fn new(feature: Feature) -> Self {
        Self {
            feature,
            required: PermissionLevel::Allow,
        }
    }

    /// Limited feature use (`제한` or `가능` cells).
    #[must_use]
    pub const fn limited(feature: Feature) -> Self {
        Self {
            feature,
            required: PermissionLevel::Limited,
        }
    }

    /// Request-only feature use (`요청 가능` or `가능` cells).
    #[must_use]
    pub const fn request(feature: Feature) -> Self {
        Self {
            feature,
            required: PermissionLevel::RequestOnly,
        }
    }

    #[must_use]
    pub const fn feature(self) -> Feature {
        self.feature
    }

    #[must_use]
    pub const fn required_permission(self) -> PermissionLevel {
        self.required
    }
}

/// Authenticated principal plus its already-resolved branch scope.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Principal {
    pub user_id: UserId,
    /// The tenant this principal belongs to, taken from the verified token's
    /// `org` claim. Used to arm `app.current_org` for RLS on this request.
    pub org_id: OrgId,
    pub access_scope: AccessScope,
    pub roles: BTreeSet<Role>,
    pub branch_scope: BranchScope,
    /// Runtime-effective grants resolved from active tenant-owned custom roles.
    ///
    /// These are additive to the built-in role matrix and never widen
    /// [`Self::branch_scope`]. Resolver failures fail closed before a request
    /// principal is built; unsupported ABAC/PBAC conditions are omitted rather
    /// than guessed.
    pub effective_feature_grants: Vec<EffectiveFeatureGrant>,
    /// Grants that carry an effective-dated interval (ADR-0032 §1).
    ///
    /// They are a SEPARATE field from [`Self::effective_feature_grants`] on
    /// purpose. Every reader of that field — including the REST list gates in
    /// crates this one does not own — is time-blind: it is given no decision
    /// instant and cannot acquire one. Keeping dated grants out of it is what
    /// makes "an expired grant authorizes nothing" a property of the types
    /// rather than of every reader remembering a filter. The only way to get an
    /// [`EffectiveFeatureGrant`] out of one is
    /// [`DatedFeatureGrant::resolve`], which demands the instant.
    #[serde(default)]
    pub dated_feature_grants: Vec<DatedFeatureGrant>,
    /// Subject authorization freshness carried by the verified access token
    /// (Cedar/PBAC activation, ADR-0021). Set from the token's mint-time snapshot
    /// claims by `resolve_principal_from_bearer_token`.
    ///
    /// SLICE-2 only SOURCES this; no live authorization decision consults it and
    /// the Cedar path stays unreachable. It defaults to the no-material baseline
    /// (all zero), so a token minted before the freshness claims existed yields
    /// [`SubjectFreshness::default`] and every current live path is unchanged.
    #[serde(default)]
    pub authz_freshness: SubjectFreshness,
}

/// A machine identity resolved from the production-service-principal resolver.
/// It intentionally has no roles or effective human grants.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServicePrincipal {
    pub id: ServicePrincipalId,
    pub org_id: OrgId,
    pub branch_id: BranchId,
    pub feature: Feature,
}

impl ServicePrincipal {
    #[must_use]
    pub const fn new(
        id: ServicePrincipalId,
        org_id: OrgId,
        branch_id: BranchId,
        feature: Feature,
    ) -> Self {
        Self {
            id,
            org_id,
            branch_id,
            feature,
        }
    }
}

impl Principal {
    #[must_use]
    pub const fn new(
        user_id: UserId,
        org_id: OrgId,
        roles: BTreeSet<Role>,
        branch_scope: BranchScope,
    ) -> Self {
        Self {
            user_id,
            org_id,
            access_scope: AccessScope::legacy_org(org_id),
            roles,
            branch_scope,
            effective_feature_grants: Vec::new(),
            dated_feature_grants: Vec::new(),
            authz_freshness: SubjectFreshness {
                policy_version: 0,
                subject_version: 0,
                session_generation: 0,
                step_up_generation: None,
            },
        }
    }

    #[must_use]
    pub const fn with_access_scope(mut self, access_scope: AccessScope) -> Self {
        self.access_scope = access_scope;
        self
    }

    /// Attach the subject authorization freshness snapshot carried by the
    /// verified token (Cedar/PBAC activation). SLICE-2 only sources it; no live
    /// decision consults it yet.
    #[must_use]
    pub const fn with_authz_freshness(mut self, freshness: SubjectFreshness) -> Self {
        self.authz_freshness = freshness;
        self
    }

    #[must_use]
    pub fn with_effective_feature_grants(mut self, grants: Vec<EffectiveFeatureGrant>) -> Self {
        self.effective_feature_grants = grants;
        self
    }

    /// Attach grants that carry an effective-dated interval. Only
    /// [`authorize_scoped`], which is given the decision instant, can see them.
    #[must_use]
    pub fn with_dated_feature_grants(mut self, grants: Vec<DatedFeatureGrant>) -> Self {
        self.dated_feature_grants = grants;
        self
    }
}

/// One runtime-effective custom-role grant after tenant/RLS, status, feature,
/// permission, and supported condition checks have all succeeded.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct EffectiveFeatureGrant {
    pub feature: Feature,
    pub permission: PermissionLevel,
    pub branch_scope: BranchScope,
}

impl EffectiveFeatureGrant {
    #[must_use]
    pub const fn new(
        feature: Feature,
        permission: PermissionLevel,
        branch_scope: BranchScope,
    ) -> Self {
        Self {
            feature,
            permission,
            branch_scope,
        }
    }
}

/// An [`EffectiveFeatureGrant`] that carries an effective-dated interval
/// (ADR-0032 §1) — a 발령 with a start date, and possibly an end date.
///
/// The grant inside is **unreachable without a decision instant**. The field is
/// private and the only way through is [`Self::resolve`], which takes a
/// `Timestamp` and yields nothing outside the interval. That is deliberate and
/// it is the whole design: an interval carried as one more public field beside
/// the others would be an OPT-IN filter, correct only in the readers that
/// remember it, and this crate has readers it does not own — `inventory/rest`,
/// `dispatch/rest` and `identity/rest` all fold over
/// [`Principal::effective_feature_grants`] with no instant in hand and no way to
/// get one. So a dated grant is not in that list at all
/// ([`Principal::dated_feature_grants`] is its own field), and the only thing
/// that can put one there is [`Self::resolve`] having already said yes.
///
/// Forgetting the filter is therefore not possible: there is no filter, and the
/// next reader added inherits the guarantee instead of the default.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DatedFeatureGrant {
    grant: EffectiveFeatureGrant,
    validity: GrantValidity,
}

impl DatedFeatureGrant {
    #[must_use]
    pub const fn new(grant: EffectiveFeatureGrant, validity: GrantValidity) -> Self {
        Self { grant, validity }
    }

    #[must_use]
    pub const fn validity(&self) -> GrantValidity {
        self.validity
    }

    /// The grant — but only at an instant it is effective at, and only for as
    /// long as the borrow lasts. Re-resolved on every decision, never memoised
    /// (ADR-0032 §3): crossing `valid_from` or `valid_to` is not a write, so no
    /// freshness counter can record it and nothing but re-evaluating this
    /// predicate can notice it (§6).
    #[must_use]
    pub fn resolve(&self, at: Timestamp) -> Option<&EffectiveFeatureGrant> {
        self.validity.contains(at).then_some(&self.grant)
    }
}

// ---------------------------------------------------------------------------
// Platform tier — the SaaS-vendor identity ABOVE all tenants.
// ---------------------------------------------------------------------------
//
// The platform tier is a DISTINCT concept from the per-tenant [`Role`]s. It is
// deliberately NOT just another `Role`: a platform actor must never be treated
// as a tenant member, regardless of how many tenant roles exist. Instead a
// platform principal is its own type with its own
// small capability set, and it can NEVER hold a tenant `Role` or be authorized
// for a tenant [`Feature`] (there is no bridge from [`PlatformFeature`] to
// [`Feature`], and [`PlatformPrincipal`] carries no `BranchScope`).

/// Cross-tenant capabilities held only by the platform (SaaS-vendor) tier.
///
/// Every platform action is cross-tenant and must be explicit + audited; a
/// tenant admin can never reach these (the platform extractor rejects a tenant
/// token, and tenant middleware rejects a platform token).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlatformFeature {
    /// Create (onboard) a new tenant organization + seed its first admin.
    TenantCreate,
    /// List all tenants (cross-tenant read).
    TenantList,
    /// Suspend / reactivate a tenant (status change).
    TenantSuspend,
    /// Hard-remove (delete) an empty/test tenant org + its onboarding shell.
    /// Strictly more destructive than [`Self::TenantSuspend`]; a tenant's own
    /// admin can never reach it (the platform extractor rejects a tenant token).
    TenantRemove,
    /// Read a tenant's health/status.
    TenantHealthRead,
    /// Mint an audited tenant-admin context so a platform operator can manage a
    /// specific tenant through the ordinary tenant-scoped UI/API.
    TenantManage,
    /// Manage platform group identities and subsidiary membership.
    GroupManage,
    /// Read the platform-tier audit trail.
    PlatformAuditRead,
}

impl PlatformFeature {
    pub const ALL: [Self; 8] = [
        Self::TenantCreate,
        Self::TenantList,
        Self::TenantSuspend,
        Self::TenantRemove,
        Self::TenantHealthRead,
        Self::TenantManage,
        Self::GroupManage,
        Self::PlatformAuditRead,
    ];
}

/// An authenticated PLATFORM principal — the SaaS-vendor tier above all tenants.
///
/// It holds NO tenant [`Role`] and NO [`BranchScope`]: a platform principal can
/// never create a work order or touch tenant-scoped data through the tenant
/// matrix. Its authority is the full [`PlatformFeature`] set (the platform token
/// is a single trust level today; finer-grained platform RBAC can subset this
/// later without touching the tenant matrix).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PlatformPrincipal {
    pub user_id: UserId,
}

impl PlatformPrincipal {
    #[must_use]
    pub const fn new(user_id: UserId) -> Self {
        Self { user_id }
    }

    /// Default-deny authorization for one platform capability. Today every
    /// platform principal holds the full set, so this returns `Ok` for any
    /// [`PlatformFeature`]; it exists so call sites are explicit about the
    /// capability they require and so subsetting later is a one-line change.
    pub fn authorize(&self, _feature: PlatformFeature) -> Result<(), KernelError> {
        Ok(())
    }
}

/// Return one role's matrix cell for one feature.
#[must_use]
pub fn permission_for(role: Role, feature: Feature) -> PermissionLevel {
    feature.matrix_row()[role.matrix_index()]
}

pub(crate) const fn feature_available_in_v1(feature: Feature) -> bool {
    !matches!(feature, Feature::AiAssist)
}

/// Authorize a principal for an org-wide feature read/action with no concrete
/// resource branch in the request. This is the single source of truth for
/// branch-omitted org-wide routes: callers must already have `BranchScope::All`,
/// built-in org-wide authority is limited to `SUPER_ADMIN`/`EXECUTIVE`, and
/// custom-role grants must themselves be org-wide (`BranchScope::All`) to pass
/// this gate. Branch-narrow custom grants may still authorize concrete branch
/// requests through [`authorize`], but never widen into an all-branch read.
pub fn authorize_org_wide(principal: &Principal, action: Action) -> Result<(), KernelError> {
    if !feature_available_in_v1(action.feature()) {
        return Err(KernelError::forbidden("feature is unavailable in v1"));
    }
    if principal.branch_scope != BranchScope::All {
        return Err(KernelError::forbidden(
            "org-wide access requires all-branch scope",
        ));
    }

    let has_builtin_org_wide_permission = principal.roles.iter().any(|role| {
        matches!(role, Role::SuperAdmin | Role::Executive)
            && permission_for(*role, action.feature()).satisfies(action.required_permission())
    });
    let has_custom_org_wide_permission = principal.effective_feature_grants.iter().any(|grant| {
        grant.feature == action.feature()
            && grant.permission.satisfies(action.required_permission())
            && grant.branch_scope == BranchScope::All
    });

    if !(has_builtin_org_wide_permission || has_custom_org_wide_permission) {
        return Err(KernelError::forbidden("role is not allowed to use feature"));
    }

    Ok(())
}

/// Authorize a principal for a feature whose resource carries NO BRANCH AT ALL —
/// a workflow run, a workflow definition, a sales listing, an evaluation cycle:
/// rows whose table has no `branch_id` column, and list gates whose confinement
/// is the caller's whole `branch_scope` rather than one branch.
///
/// Complete the set:
/// * [`authorize`] — the resource HAS a branch; pass THAT branch. Never pass a
///   branch derived from `principal.branch_scope`. `authorize` checks
///   `principal.branch_scope.allows(resource_branch)` first, so a
///   principal-derived branch makes that check a TAUTOLOGY on both arms: `All`
///   allows any fabricated id, and `Branches(set).iter().next()` is by
///   definition a member of `set`. The branch dimension then vanishes silently
///   instead of visibly. If you were about to write
///   `match principal.branch_scope { All => BranchId::new(), Branches(b) =>
///   b.iter().next() }` — or the `.iter().any(|b| authorize(.., *b).is_ok())`
///   variant, which is the same tautology, only wordier — you want THIS
///   function. `console-gate-fabricated-branch` fails CI if you write it anyway.
/// * [`authorize_org_wide`] — the ACTION spans every branch. Requires the caller
///   to already hold `BranchScope::All` and limits built-in authority to
///   `SUPER_ADMIN`/`EXECUTIVE`. Do NOT reach for it merely because a branch is
///   unavailable: it denies every branch-scoped principal and every ADMIN.
///
/// The built-in-role arm is byte-identical to [`authorize`]'s, which never
/// consults the branch — so for every principal whose access comes from a
/// built-in role the verdict is unchanged by construction.
///
/// # The one behavioural delta in this migration
///
/// The custom-grant arm requires the grant to COVER the principal's whole scope
/// (`grant ⊇ principal`) rather than merely to allow one branch of it. This is
/// the only place any verdict moves, and it moves in one direction: `grant ⊇ p`
/// implies `grant.allows(b)` for every `b ∈ p`, so nothing this admits was denied
/// before. For an `All`-scoped principal it collapses to exactly
/// [`authorize_org_wide`]'s `grant.branch_scope == BranchScope::All`; for a
/// single-branch principal `grant ⊇ {b}` IS `grant.allows(b)`. Both are unchanged.
///
/// **Who loses access.** Exactly one class: a principal holding TWO OR MORE
/// branches, with no built-in role permission for the feature, whose custom
/// (PBAC) grant covers some but not all of those branches. A `{seoul, busan}`
/// principal with a Seoul-only grant is denied here and may have been allowed
/// before.
///
/// **Why that is correct.** "May have been" is the point: what this replaced was
/// not a policy, it was a sort order. The fabricated check authorized against
/// `branches.iter().next()` — the LOWEST-SORTING branch UUID of a `BTreeSet` — so
/// the identical partial grant admitted or denied depending on which random id
/// happened to sort first. Nothing in the product means that, and no operator
/// could predict it. Deterministic deny replaces a coin flip, and it is the
/// conservative side of the flip.
///
/// Deny is also the right answer on the merits: these are the sites where nothing
/// downstream narrows to a branch. A branch-less row has no branch to confine by,
/// and a list gate's confinement IS the caller's whole `branch_scope` — so a grant
/// covering part of that scope cannot authorize an action that reaches all of it.
/// The remedy for an affected tenant is to widen the grant to the principal's
/// scope, which is what the grant already meant to say.
///
/// Pinned by `capability_replaces_an_order_dependent_grant_verdict_with_a_deny`;
/// recorded in `docs/decisions/notes/DN-0004-adr-0028-branchless-capability-authorization.md`.
pub fn authorize_capability(principal: &Principal, action: Action) -> Result<(), KernelError> {
    authorize_inner(principal, action, None, None)
}

/// [`authorize_capability`] at a decision instant — the branch-LESS half of the
/// spine, and the ONLY door that takes no resource.
///
/// It is a separate function rather than a `None` on [`authorize_scoped`] on
/// purpose. An `Option<ResourceBranch>` parameter is a type boundary its own
/// caller can step around: every fabricating call site that
/// [`ResourceBranch`] makes uncompilable stays compilable by passing `None`,
/// and the branch dimension disappears exactly as silently as it did before —
/// a convention wearing a type's clothes. With two doors there is no argument
/// to omit, so CONVERTING a branch-scoped call site to the branch-less one is a
/// changed function name and visible in a diff.
///
/// That is the whole of the claim. A NEW handler for a branch-scoped resource
/// can still call this one and drop the branch dimension exactly as quietly as
/// `authorize_scoped(.., None, ..)` did — nothing here can tell that its caller
/// had a resource. Choosing the right door is still a judgement; the split only
/// removes the way to get it wrong by omission.
///
/// The temporal contract is [`authorize_scoped`]'s: `at` is the decision
/// instant, [`Principal::dated_feature_grants`] are resolved against it on
/// every call, and nothing is carried across the boundary (ADR-0032 §3, §6).
///
/// # Errors
///
/// [`console_kernel_core::ErrorKind::Forbidden`] when the principal has no
/// branch scope at all, or when neither a built-in role nor a grant effective
/// AT `at` and covering that whole scope permits the action.
pub fn authorize_capability_at(
    principal: &Principal,
    action: Action,
    at: Timestamp,
) -> Result<(), KernelError> {
    authorize_inner(principal, action, None, Some(at))
}

/// The single body behind [`authorize`], [`authorize_capability`] and
/// [`authorize_scoped`], so the branch dimension and the grant predicate cannot
/// drift apart across three copies.
///
/// `branch` is the whole difference between the two doors:
/// * `Some(b)` — the resource HAS a branch. Membership is
///   `branch_scope.allows(b)`, and a custom grant qualifies when it allows `b`.
/// * `None` — the resource has NO branch, so confinement IS the caller's whole
///   scope. Membership is "the scope is non-empty" (without it,
///   `intersect({}, {}) == {}` makes the coverage test below vacuously true and
///   a membership-less principal would be authorized by any grant it holds; every
///   fabricating helper this replaced denied that principal, and
///   `BranchScope::none` is documented as "allows nothing"), and a custom grant
///   qualifies only when it COVERS that whole scope.
///
/// `at` is the decision instant, and it is the only temporal input.
/// * `Some(at)` — [`Principal::dated_feature_grants`] are resolved against it,
///   per call, never memoised and never carried across the decision boundary
///   (ADR-0032 §3).
/// * `None` — this door was given no instant, so it cannot resolve a dated
///   grant and does not guess. It sees only the interval-less grants, which
///   fails CLOSED (§6): an effective-dated grant can never widen a decision
///   that could not have checked its dates.
fn authorize_inner(
    principal: &Principal,
    action: Action,
    branch: Option<BranchId>,
    at: Option<Timestamp>,
) -> Result<(), KernelError> {
    if !feature_available_in_v1(action.feature()) {
        return Err(KernelError::forbidden("feature is unavailable in v1"));
    }
    match branch {
        Some(resource_branch) => {
            if !principal.branch_scope.allows(resource_branch) {
                return Err(KernelError::forbidden(
                    "resource branch is outside principal scope",
                ));
            }
        }
        None => {
            if principal.branch_scope.is_empty() {
                return Err(KernelError::forbidden("principal has no branch scope"));
            }
        }
    }

    // Resolved HERE, at the decision, and dropped when it returns.
    let effective_now = principal
        .effective_feature_grants
        .iter()
        .chain(at.into_iter().flat_map(|at| {
            principal
                .dated_feature_grants
                .iter()
                .filter_map(move |dated| dated.resolve(at))
        }));

    let has_feature_permission = principal.roles.iter().any(|role| {
        permission_for(*role, action.feature()).satisfies(action.required_permission())
    }) || effective_now.into_iter().any(|grant| {
        grant.feature == action.feature()
            && grant.permission.satisfies(action.required_permission())
            && match branch {
                Some(resource_branch) => grant.branch_scope.allows(resource_branch),
                None => {
                    grant.branch_scope.intersect(&principal.branch_scope) == principal.branch_scope
                }
            }
    });

    if !has_feature_permission {
        return Err(KernelError::forbidden("role is not allowed to use feature"));
    }

    Ok(())
}

/// The BRANCH-SCOPED authorization door: the decision instant is an argument and
/// the resource's branch is a value that only Postgres can have produced.
///
/// `resource` is branch authorization, identical to [`authorize`], against a
/// branch that came off the RESOURCE. It cannot have come from anywhere else:
/// [`ResourceBranch`] has no constructor that takes a `BranchId`, so the
/// fabricating shapes do not compile rather than needing to be recognised.
///
/// When the resource has NO branch — no `branch_id` column, or a list gate whose
/// confinement is the caller's whole scope — the door is
/// [`authorize_capability_at`], which is a different function and not an omitted
/// argument. This one took `Option<ResourceBranch>` until the option was removed:
/// a boundary a caller can step around by passing `None` is a convention, not a
/// type, and `None` is the one spelling of a fabricated branch that a compiler
/// cannot object to.
///
/// `at` is the instant the decision is made. The grant set is resolved against
/// it on every call and the result is never cached across the boundary
/// (ADR-0032 §3). Crossing `valid_from` or `valid_to` is not a write, so no
/// freshness counter can record it and nothing but re-evaluating the predicate
/// can notice it (§6).
///
/// # The parameter IS a `ResourceBranch`
///
/// THE CONTROL, and the positive form of the claim: the signature is asserted
/// whole, so re-widening the parameter is a compile error here rather than a
/// convention nobody re-reads.
///
/// ```
/// use console_kernel_core::{KernelError, Timestamp};
/// use console_platform_authz::{Action, Principal, ResourceBranch, authorize_scoped};
///
/// let branch_scoped_door: fn(&Principal, Action, ResourceBranch, Timestamp)
///     -> Result<(), KernelError> = authorize_scoped;
/// ```
///
/// The same coercion with the parameter back in an `Option` — one token's
/// difference — no longer describes this function:
///
/// ```compile_fail,E0308
/// use console_kernel_core::{KernelError, Timestamp};
/// use console_platform_authz::{Action, Principal, ResourceBranch, authorize_scoped};
///
/// let branch_scoped_door: fn(&Principal, Action, Option<ResourceBranch>, Timestamp)
///     -> Result<(), KernelError> = authorize_scoped;
/// ```
///
/// # A fabricated branch does not compile
///
/// The branch-LESS call is ordinary, and it is its own function:
///
/// ```
/// use std::collections::BTreeSet;
///
/// use console_kernel_core::{BranchId, BranchScope, OrgId, Timestamp, UserId};
/// use console_platform_authz::{Action, Feature, Principal, Role, authorize_capability_at};
///
/// let principal = Principal::new(
///     UserId::new(),
///     OrgId::knl(),
///     BTreeSet::from([Role::Admin]),
///     BranchScope::single(BranchId::new()),
/// );
/// let decision = authorize_capability_at(
///     &principal,
///     Action::new(Feature::CompletionReview),
///     Timestamp::now_utc(),
/// );
/// assert!(decision.is_ok());
/// ```
///
/// Aiming that same call at the branch-scoped door with a minted branch is
/// rejected by the compiler, not by a scan:
///
/// ```compile_fail,E0308
/// use std::collections::BTreeSet;
///
/// use console_kernel_core::{BranchId, BranchScope, OrgId, Timestamp, UserId};
/// use console_platform_authz::{Action, Feature, Principal, Role, authorize_scoped};
///
/// let principal = Principal::new(
///     UserId::new(),
///     OrgId::knl(),
///     BTreeSet::from([Role::Admin]),
///     BranchScope::single(BranchId::new()),
/// );
/// let decision = authorize_scoped(
///     &principal,
///     Action::new(Feature::CompletionReview),
///     BranchId::new(),
///     Timestamp::now_utc(),
/// );
/// assert!(decision.is_ok());
/// ```
///
/// # Nor does `None`, because that is not a resource
///
/// The escape hatch this door used to have. It is the identical example with the
/// branch replaced by `None` and nothing else changed:
///
/// ```compile_fail,E0308
/// use std::collections::BTreeSet;
///
/// use console_kernel_core::{BranchId, BranchScope, OrgId, Timestamp, UserId};
/// use console_platform_authz::{Action, Feature, Principal, Role, authorize_scoped};
///
/// let principal = Principal::new(
///     UserId::new(),
///     OrgId::knl(),
///     BTreeSet::from([Role::Admin]),
///     BranchScope::single(BranchId::new()),
/// );
/// let decision = authorize_scoped(
///     &principal,
///     Action::new(Feature::CompletionReview),
///     None,
///     Timestamp::now_utc(),
/// );
/// ```
///
/// # The TENANT is checked here, not trusted from the lookup
///
/// [`ResourceBranch::lookup`] binds `org_id` in its statement, but the org it
/// binds is an argument: a handler that takes it from the same request path as
/// the row id (`path.org_id`) names a tenant the principal need not belong to,
/// and the lookup then succeeds against THAT tenant's row. For a
/// `BranchScope::All` principal `allows(branch)` is unconditionally true, so
/// the decision would be an ALLOW against another tenant's resource.
///
/// So the branch carries the org it was read under and this door compares it to
/// the principal's own. The tenant cannot be supplied independently of the
/// principal being authorized — it can only be supplied wrongly, which denies.
///
/// # Errors
///
/// [`console_kernel_core::ErrorKind::Forbidden`] when the resource was read
/// under a different tenant than the principal's, when the resource branch is
/// outside the principal's scope, or when neither a built-in role nor a grant
/// effective AT `at` permits the action.
pub fn authorize_scoped(
    principal: &Principal,
    action: Action,
    resource: ResourceBranch,
    at: Timestamp,
) -> Result<(), KernelError> {
    if resource.org() != principal.org_id {
        return Err(KernelError::forbidden(
            "resource was read under a different tenant than the principal",
        ));
    }

    authorize_inner(principal, action, Some(resource.get()), Some(at))
}

/// Authorize a principal for a feature against a concrete resource branch.
///
/// This intentionally checks both role permission and branch membership for
/// every call. Listing APIs should also use [`repository_filter`] so the data
/// access path is constrained before rows are materialized.
///
/// # The branch must come from the RESOURCE
///
/// `resource_branch` must be read off the row being authorized. A branch derived
/// from `principal.branch_scope` makes the first check a tautology on BOTH arms
/// — `All` allows a fresh `BranchId::new()`, and `branches.iter().next()` is a
/// member of `branches` by definition — which silently deletes the branch
/// dimension instead of enforcing it. When the resource genuinely has no branch,
/// call [`authorize_capability`]; when the action genuinely spans every branch,
/// call [`authorize_org_wide`]. `console-gate-fabricated-branch` fails CI on the
/// fabricating shapes.
///
/// # A [`ResourceBranch`] cannot be unwrapped into this door
///
/// This door takes a bare [`BranchId`] and so has no tenant to compare: a caller
/// that unwrapped a [`ResourceBranch`] here would reach the same decision while
/// skipping the org check only [`authorize_scoped`] performs, which is an ALLOW
/// against another tenant's resource for any `BranchScope::All` principal. That
/// spelling does not exist — the branch accessor is crate-private, so the
/// unwrap does not compile outside this crate:
///
/// ```compile_fail,E0624
/// fn unwrap_the_resource(
///     resource: console_platform_authz::ResourceBranch,
/// ) -> console_kernel_core::BranchId {
///     resource.get()
/// }
/// ```
///
/// THE CONTROL, and a one-token delta: the same argument, the same call syntax,
/// the accessor that IS public — so the failure above is the unwrap and not the
/// imports, the type name or the method-call shape.
///
/// ```
/// fn read_the_tenant(
///     resource: console_platform_authz::ResourceBranch,
/// ) -> console_kernel_core::OrgId {
///     resource.org()
/// }
/// ```
pub fn authorize(
    principal: &Principal,
    action: Action,
    resource_branch: BranchId,
) -> Result<(), KernelError> {
    authorize_inner(principal, action, Some(resource_branch), None)
}

/// Authorize a machine principal. Unlike [`authorize`], this cannot inherit a
/// human role or tenant custom grant: both the exact feature and exact branch
/// must match the credential registration.
pub fn authorize_service(
    principal: &ServicePrincipal,
    action: Action,
    resource_branch: BranchId,
) -> Result<(), KernelError> {
    if !feature_available_in_v1(action.feature())
        || principal.feature != action.feature()
        || principal.branch_id != resource_branch
    {
        return Err(KernelError::forbidden(
            "service principal is not authorized for resource",
        ));
    }
    Ok(())
}

#[derive(Debug)]
struct RuntimePolicyPermissionRow {
    /// The `user_role_assignments` row this permission was reached through. A
    /// role with three permissions joins to three rows and is still ONE
    /// assignment; ADR-0032 §1's non-overlap constraint is about assignments.
    assignment_id: uuid::Uuid,
    role_id: uuid::Uuid,
    feature_key: String,
    permission_level: String,
}

#[derive(Debug)]
struct RuntimePolicyConditionRow {
    role_id: uuid::Uuid,
    attribute: String,
    operator: String,
    condition_values: Vec<String>,
}

/// Resolve runtime-effective tenant custom-role grants under an explicitly
/// armed org.
///
/// Safety boundary:
/// * only assignments to `ACTIVE`, non-system roles are effective;
/// * feature/permission strings are parse-or-deny;
/// * elevated/scope-widening features stay system-role-only for this slice;
/// * branch conditions may only narrow the already-live branch scope;
/// * team conditions must match the target user's live team attribute;
/// * unsupported ABAC/PBAC conditions fail closed for runtime authorization
///   while remaining persisted/visible in Policy Studio previews.
pub async fn resolve_effective_feature_grants_in_org(
    pool: &PgPool,
    org: OrgId,
    user_id: UserId,
    live_branch_scope: &BranchScope,
) -> Result<Vec<EffectiveFeatureGrant>, KernelError> {
    let mut tx = pool.begin().await.map_err(map_effective_policy_error)?;
    sqlx::query("SELECT set_config('app.current_org', $1, true)")
        .bind(org.as_uuid().to_string())
        .execute(tx.as_mut())
        .await
        .map_err(map_effective_policy_error)?;
    let grants =
        resolve_effective_feature_grants_in_tx(&mut tx, org, user_id, live_branch_scope).await?;
    tx.commit().await.map_err(map_effective_policy_error)?;
    Ok(grants)
}

/// Same resolver for an already Company-armed owner transaction. It must not
/// acquire another pooled connection while the caller holds authentication locks.
pub async fn resolve_effective_feature_grants_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    org: OrgId,
    user_id: UserId,
    live_branch_scope: &BranchScope,
) -> Result<Vec<EffectiveFeatureGrant>, KernelError> {
    let user_team: Option<String> =
        sqlx::query_scalar("SELECT team FROM users WHERE org_id = $1 AND id = $2")
            .bind(*org.as_uuid())
            .bind(*user_id.as_uuid())
            .fetch_optional(tx.as_mut())
            .await
            .map_err(map_effective_policy_error)?
            .flatten();

    let permission_rows = sqlx::query(
        r#"
        SELECT ura.id AS assignment_id, pr.id AS role_id, prp.feature_key, prp.permission_level
        FROM user_role_assignments AS ura
        JOIN policy_roles AS pr
          ON pr.org_id = ura.org_id
         AND pr.id = ura.role_id
        JOIN policy_role_permissions AS prp
          ON prp.org_id = pr.org_id
         AND prp.role_id = pr.id
        WHERE ura.org_id = $1
          AND ura.user_id = $2
          AND pr.status = 'ACTIVE'
          AND pr.is_system = false
        ORDER BY pr.role_key, prp.feature_key
        "#,
    )
    .bind(*org.as_uuid())
    .bind(*user_id.as_uuid())
    .fetch_all(tx.as_mut())
    .await
    .map_err(map_effective_policy_error)?
    .into_iter()
    .map(|row| {
        Ok(RuntimePolicyPermissionRow {
            assignment_id: row.try_get("assignment_id")?,
            role_id: row.try_get("role_id")?,
            feature_key: row.try_get("feature_key")?,
            permission_level: row.try_get("permission_level")?,
        })
    })
    .collect::<Result<Vec<_>, sqlx::Error>>()
    .map_err(map_effective_policy_error)?;

    let role_ids = permission_rows
        .iter()
        .map(|row| row.role_id)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();

    let condition_rows = if role_ids.is_empty() {
        Vec::new()
    } else {
        sqlx::query(
            r#"
            SELECT role_id, attribute, operator, condition_values
            FROM policy_role_conditions
            WHERE org_id = $1
              AND role_id = ANY($2)
            ORDER BY role_id, condition_key
            "#,
        )
        .bind(*org.as_uuid())
        .bind(&role_ids)
        .fetch_all(tx.as_mut())
        .await
        .map_err(map_effective_policy_error)?
        .into_iter()
        .map(|row| {
            Ok(RuntimePolicyConditionRow {
                role_id: row.try_get("role_id")?,
                attribute: row.try_get("attribute")?,
                operator: row.try_get("operator")?,
                condition_values: row.try_get("condition_values")?,
            })
        })
        .collect::<Result<Vec<_>, sqlx::Error>>()
        .map_err(map_effective_policy_error)?
    };

    let mut conditions_by_role: BTreeMap<uuid::Uuid, Vec<RuntimePolicyConditionRow>> =
        BTreeMap::new();
    for condition in condition_rows {
        conditions_by_role
            .entry(condition.role_id)
            .or_default()
            .push(condition);
    }

    let mut effective_scopes_by_role = BTreeMap::new();
    for role_id in role_ids {
        let Some(scope) = effective_scope_for_custom_role_conditions(
            live_branch_scope,
            user_team.as_deref(),
            conditions_by_role
                .get(&role_id)
                .map_or(&[][..], Vec::as_slice),
        ) else {
            continue;
        };
        if !scope.is_empty() {
            effective_scopes_by_role.insert(role_id, scope);
        }
    }

    effective_grants_from_checked_assignments(permission_rows, &effective_scopes_by_role)
}

/// One entry per 발령, for [`enforce_assignment_non_overlap`] to group by role.
///
/// The permission join repeats one assignment once per permission, so the rows
/// are folded by ASSIGNMENT id. Keying this by `role_id` instead reads as a
/// tidy-up and is not one: two records for a role would collapse into one entry
/// and the guard downstream could never fire again.
///
/// Every interval is [`GrantValidity::always()`] because
/// `user_role_assignments` has no `valid_from`/`valid_to` columns yet — the
/// migration is ADR-0032's other half. That is not a placeholder that weakens
/// the check today: `always()` overlaps `always()`, so two rows for one role
/// are still refused.
///
/// It is NOT, however, the shape the columns arrive in. When they land, THIS
/// function must read them; leaving `always()` here would fold two legitimately
/// consecutive 발령 — `[100, 200)` then `[200, 300)`, the case
/// `touching_half_open_intervals_for_one_role_do_not_overlap` exists to permit —
/// into two `always()` intervals that overlap, and the resolver would return
/// `Conflict` instead of that user's whole custom-role authority.
///
/// That obligation is executable, not a note: the integration test
/// `assignment_validity_is_stamped_only_while_the_schema_has_no_intervals`
/// asserts against the live schema that the columns are absent, so the
/// migration that adds them fails here first and names this function.
fn assignment_validities(
    permission_rows: &[RuntimePolicyPermissionRow],
) -> Vec<(uuid::Uuid, GrantValidity)> {
    permission_rows
        .iter()
        .map(|row| (row.assignment_id, row.role_id))
        .collect::<BTreeMap<_, _>>()
        .into_values()
        .map(|role_id| (role_id, GrantValidity::always()))
        .collect()
}

/// The fold from assignment rows to runtime authority — behind ADR-0032 §1.
///
/// The check is FIRST and the fold is only reachable through it, so an
/// ambiguous assignment set cannot become a grant by a caller forgetting to
/// ask. `enforce_assignment_non_overlap` is otherwise a predicate with no
/// production caller, which is indistinguishable from not having one.
///
/// The checked set is derived HERE, from the same rows that are about to be
/// folded. It was a separate argument, and that made the wiring provable only
/// by inspection: the resolver could pass an empty slice — accidentally, or
/// through an edit to what it collected — and the guard would still be called,
/// still return `Ok`, and no test could tell, because the assignments a test
/// passes are not the rows it folds. One argument means the two cannot be
/// different sets.
///
/// The rows checked are the ones that are about to become authority:
/// assignments to ACTIVE, non-system roles that carry a permission. A duplicate
/// on a DRAFT role grants nothing either way, and the database constraint (still
/// unwritten) covers the rest.
///
/// # Errors
///
/// [`console_kernel_core::ErrorKind::Conflict`] when two assignments of one role
/// cover a shared instant.
fn effective_grants_from_checked_assignments(
    permission_rows: Vec<RuntimePolicyPermissionRow>,
    effective_scopes_by_role: &BTreeMap<uuid::Uuid, BranchScope>,
) -> Result<Vec<EffectiveFeatureGrant>, KernelError> {
    enforce_assignment_non_overlap(&assignment_validities(&permission_rows))?;

    Ok(permission_rows
        .into_iter()
        .filter_map(|row| {
            let scope = effective_scopes_by_role.get(&row.role_id)?;
            let feature = Feature::from_str(&row.feature_key).ok()?;
            let permission = PermissionLevel::from_str(&row.permission_level).ok()?;
            if permission == PermissionLevel::Deny || !custom_role_runtime_feature_allowed(feature)
            {
                return None;
            }
            Some(EffectiveFeatureGrant::new(
                feature,
                permission,
                scope.clone(),
            ))
        })
        .collect())
}

/// Single safety predicate for features a custom role may grant at runtime.
///
/// Used by [`effective_grants_from_checked_assignments`] and by identity REST
/// write/preview/assignment ceilings. Callers must not re-encode this block
/// list. Policy Studio catalog visibility is a separate UI concern in
/// `console-identity-rest` (`policy_studio_feature_visible`).
#[must_use]
pub fn custom_role_runtime_feature_allowed(feature: Feature) -> bool {
    !matches!(
        feature,
        Feature::RoleManage
            | Feature::ElevatedRoleGrant
            | Feature::OrgWideQueueTriage
            | Feature::AuditStreamRead
            | Feature::AuditStreamAccessLogRead
            | Feature::AiAssist
    )
}

fn effective_scope_for_custom_role_conditions(
    live_branch_scope: &BranchScope,
    user_team: Option<&str>,
    conditions: &[RuntimePolicyConditionRow],
) -> Option<BranchScope> {
    let mut scope = live_branch_scope.clone();
    for condition in conditions {
        if !matches!(condition.operator.as_str(), "equals" | "in") {
            return None;
        }

        match condition.attribute.as_str() {
            "branch" => {
                let mut branches = BTreeSet::new();
                for value in &condition.condition_values {
                    let Ok(branch) = BranchId::from_str(value) else {
                        return None;
                    };
                    branches.insert(branch);
                }
                scope = scope.intersect(&BranchScope::Branches(branches));
            }
            "team" => {
                if !team_condition_matches(user_team, &condition.condition_values) {
                    return None;
                }
            }
            _ => return None,
        }
    }
    Some(scope)
}

fn team_condition_matches(user_team: Option<&str>, values: &[String]) -> bool {
    let Some(user_team) = user_team.map(str::trim).filter(|team| !team.is_empty()) else {
        return false;
    };
    let Some(accepted) = team_policy_values(user_team) else {
        return false;
    };
    values.iter().any(|value| {
        let value = value.trim();
        accepted
            .iter()
            .any(|accepted| value == *accepted || value.eq_ignore_ascii_case(accepted))
    })
}

fn team_policy_values(user_team: &str) -> Option<[&'static str; 2]> {
    match user_team {
        "정비" => Some(["MAINTENANCE", "정비"]),
        "예방" => Some(["PREVENTION", "예방"]),
        "관리" => Some(["MANAGEMENT", "관리"]),
        "접수" => Some(["RECEPTION", "접수"]),
        _ => None,
    }
}

fn map_effective_policy_error(err: sqlx::Error) -> KernelError {
    KernelError::internal(format!("failed to resolve effective policy: {err}"))
}

/// Resolve branch scope from `user_branches` under an explicitly-armed tenant.
///
/// `SUPER_ADMIN` and `EXECUTIVE` resolve to [`BranchScope::All`] for global
/// read/rollup surfaces; write authority is still constrained by the matrix.
///
/// `user_branches` is FORCE RLS, so a bare-pool read returns ZERO branches when
/// `app.current_org` is unset — silently narrowing a non-super admin's scope to
/// nothing. This opens a transaction, arms the GUC to `org` (the caller's
/// verified-token tenant), then runs the query, so RLS narrows to exactly that
/// org's memberships. Callers that run BEFORE the per-request tenant middleware
/// (the principal-resolution paths) pass the org from the verified token.
pub async fn resolve_branch_scope_in_org(
    pool: &PgPool,
    org: OrgId,
    user_id: UserId,
    roles: &[Role],
) -> Result<BranchScope, KernelError> {
    if roles
        .iter()
        .any(|role| matches!(role, Role::SuperAdmin | Role::Executive))
    {
        return Ok(BranchScope::All);
    }

    let mut tx = pool
        .begin()
        .await
        .map_err(|err| KernelError::internal(format!("failed to resolve branch scope: {err}")))?;
    sqlx::query("SELECT set_config('app.current_org', $1, true)")
        .bind(org.as_uuid().to_string())
        .execute(tx.as_mut())
        .await
        .map_err(|err| KernelError::internal(format!("failed to resolve branch scope: {err}")))?;
    let scope = resolve_branch_scope_in_tx(&mut tx, user_id, roles).await?;
    tx.commit()
        .await
        .map_err(|err| KernelError::internal(format!("failed to resolve branch scope: {err}")))?;
    Ok(scope)
}

/// Resolve branch scope inside the caller's already Company-armed transaction.
pub async fn resolve_branch_scope_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    user_id: UserId,
    roles: &[Role],
) -> Result<BranchScope, KernelError> {
    if roles
        .iter()
        .any(|role| matches!(role, Role::SuperAdmin | Role::Executive))
    {
        return Ok(BranchScope::All);
    }
    let rows: Vec<uuid::Uuid> = sqlx::query_scalar(
        "SELECT branch_id FROM user_branches WHERE user_id = $1 ORDER BY branch_id",
    )
    .bind(*user_id.as_uuid())
    .fetch_all(tx.as_mut())
    .await
    .map_err(|err| KernelError::internal(format!("failed to resolve branch scope: {err}")))?;

    Ok(BranchScope::Branches(
        rows.into_iter().map(BranchId::from_uuid).collect(),
    ))
}

/// Apply a claim-level hierarchy scope to a live DB membership scope for one
/// ordinary tenant route.
///
/// The live scope is authoritative for membership revocation. The access scope
/// may only narrow that set; it never widens. A `Group` access scope is rejected
/// here because group-wide reads must use the consolidated group helper, which
/// first resolves authorized member orgs and then performs N separately armed
/// per-org reads. Region/worksite projections are not wired yet, so they fail
/// closed until a DB-backed hierarchy resolver supplies a matching projection.
pub fn effective_branch_scope_for_tenant(
    live_scope: BranchScope,
    access_scope: AccessScope,
    org_id: OrgId,
) -> Result<BranchScope, KernelError> {
    let projected_scope = match access_scope.level {
        AccessScopeLevel::Group => {
            return Err(KernelError::forbidden(
                "group access scope must use a group fan-out resolver",
            ));
        }
        AccessScopeLevel::Org => access_scope.branch_scope_for_org(org_id, None),
        AccessScopeLevel::Branch => {
            let branch_id = BranchId::from_uuid(*access_scope.node_id.as_uuid());
            let projection = BranchProjection::single(access_scope.node_id, org_id, branch_id);
            access_scope.branch_scope_for_org(org_id, Some(&projection))
        }
        AccessScopeLevel::Region | AccessScopeLevel::Worksite => {
            access_scope.branch_scope_for_org(org_id, None)
        }
    };

    Ok(live_scope.intersect(&projected_scope))
}

/// A validated SQL identifier for a branch column, e.g. `work_orders.branch_id`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BranchColumn(&'static str);

impl BranchColumn {
    pub fn new(raw: &'static str) -> Result<Self, KernelError> {
        if is_safe_column(raw) {
            Ok(Self(raw))
        } else {
            Err(KernelError::validation(format!(
                "unsafe branch column identifier: {raw}"
            )))
        }
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        self.0
    }
}

/// A SQL predicate plus branch IDs to bind as `$1`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SqlPredicate {
    sql: String,
    branch_ids: Vec<uuid::Uuid>,
}

impl SqlPredicate {
    #[must_use]
    pub fn sql(&self) -> &str {
        &self.sql
    }

    #[must_use]
    pub fn branch_ids(&self) -> &[uuid::Uuid] {
        &self.branch_ids
    }
}

/// Produce the default branch predicate for repository list/detail queries.
///
/// For explicit empty scope, this returns `FALSE`, not an empty `ANY` clause,
/// making the default deny behavior visible in generated SQL.
pub fn repository_filter(
    scope: &BranchScope,
    column: BranchColumn,
) -> Result<SqlPredicate, KernelError> {
    match scope {
        BranchScope::All => Ok(SqlPredicate {
            sql: "TRUE".to_owned(),
            branch_ids: Vec::new(),
        }),
        BranchScope::Branches(branches) if branches.is_empty() => Ok(SqlPredicate {
            sql: "FALSE".to_owned(),
            branch_ids: Vec::new(),
        }),
        BranchScope::Branches(branches) => Ok(SqlPredicate {
            sql: format!("{} = ANY($1)", column.as_str()),
            branch_ids: branches.iter().map(|branch| *branch.as_uuid()).collect(),
        }),
    }
}

fn is_safe_column(raw: &str) -> bool {
    let mut segments = raw.split('.');
    let first = segments.next();
    let second = segments.next();
    let too_many = segments.next().is_some();

    match (first, second, too_many) {
        (Some(column), None, false) => is_safe_ident(column),
        (Some(table), Some(column), false) => is_safe_ident(table) && is_safe_ident(column),
        _ => false,
    }
}

fn is_safe_ident(raw: &str) -> bool {
    let mut chars = raw.chars();
    let Some(first) = chars.next() else {
        return false;
    };

    matches!(first, 'a'..='z' | '_') && chars.all(|ch| matches!(ch, 'a'..='z' | '0'..='9' | '_'))
}

#[cfg(test)]
mod tests {
    use super::*;
    use console_kernel_core::{ErrorKind, ScopeNodeId};

    #[test]
    fn attendance_management_features_are_admin_and_super_admin_only() {
        let cases = [
            (
                Feature::AttendanceExceptionManage,
                "attendance_exception_manage",
            ),
            (
                Feature::AttendanceSubstitutionManage,
                "attendance_substitution_manage",
            ),
        ];

        for (feature, code) in cases {
            assert!(Feature::ALL.contains(&feature));
            assert_eq!(feature.as_str(), code);
            assert_eq!(code.parse::<Feature>().unwrap(), feature);
            assert_eq!(
                serde_json::to_string(&feature).unwrap(),
                format!("\"{code}\"")
            );
            assert_eq!(
                serde_json::from_str::<Feature>(&format!("\"{code}\"")).unwrap(),
                feature
            );

            for role in Role::ALL {
                let expected = match role {
                    Role::Admin | Role::SuperAdmin => PermissionLevel::Allow,
                    Role::Member | Role::Receptionist | Role::Mechanic | Role::Executive => {
                        PermissionLevel::Deny
                    }
                };
                assert_eq!(
                    permission_for(role, feature),
                    expected,
                    "{role:?} on {feature:?}"
                );
            }
        }
    }

    #[test]
    fn service_authorization_requires_exact_feature_and_branch() {
        let branch = BranchId::new();
        let service = ServicePrincipal::new(
            ServicePrincipalId::new(),
            OrgId::new(),
            branch,
            Feature::ProductionSourceIngest,
        );
        assert!(
            authorize_service(
                &service,
                Action::limited(Feature::ProductionSourceIngest),
                branch,
            )
            .is_ok()
        );
        assert!(
            authorize_service(
                &service,
                Action::limited(Feature::ProductionSourceIngest),
                BranchId::new(),
            )
            .is_err()
        );
        assert!(
            authorize_service(&service, Action::limited(Feature::DailyPlanRequest), branch,)
                .is_err()
        );
    }

    #[test]
    fn effective_scope_preserves_legacy_org_live_scope() -> Result<(), KernelError> {
        let org = OrgId::new();
        let branch = BranchId::new();
        let live_scope = BranchScope::single(branch);

        let effective =
            effective_branch_scope_for_tenant(live_scope, AccessScope::legacy_org(org), org)?;

        assert_eq!(effective, BranchScope::single(branch));
        Ok(())
    }

    #[test]
    fn effective_scope_fails_closed_on_org_mismatch() -> Result<(), KernelError> {
        let effective = effective_branch_scope_for_tenant(
            BranchScope::All,
            AccessScope::legacy_org(OrgId::new()),
            OrgId::new(),
        )?;

        assert_eq!(effective, BranchScope::none());
        Ok(())
    }

    #[test]
    fn effective_scope_branch_claim_narrows_live_all() -> Result<(), KernelError> {
        let org = OrgId::new();
        let branch = BranchId::new();
        let scope = AccessScope::new(
            AccessScopeLevel::Branch,
            ScopeNodeId::from_uuid(*branch.as_uuid()),
        );

        let effective = effective_branch_scope_for_tenant(BranchScope::All, scope, org)?;

        assert_eq!(effective, BranchScope::single(branch));
        Ok(())
    }

    #[test]
    fn effective_scope_branch_claim_intersects_live_memberships() -> Result<(), KernelError> {
        let org = OrgId::new();
        let branch = BranchId::new();
        let other = BranchId::new();
        let scope = AccessScope::new(
            AccessScopeLevel::Branch,
            ScopeNodeId::from_uuid(*branch.as_uuid()),
        );

        let allowed = effective_branch_scope_for_tenant(BranchScope::single(branch), scope, org)?;
        let denied = effective_branch_scope_for_tenant(BranchScope::single(other), scope, org)?;

        assert_eq!(allowed, BranchScope::single(branch));
        assert_eq!(denied, BranchScope::none());
        Ok(())
    }

    #[test]
    fn effective_scope_rejects_group_on_ordinary_tenant_routes() {
        let err_kind = effective_branch_scope_for_tenant(
            BranchScope::All,
            AccessScope::new(
                AccessScopeLevel::Group,
                ScopeNodeId::from_uuid(uuid::Uuid::new_v4()),
            ),
            OrgId::new(),
        )
        .err()
        .map(|err| err.kind);

        assert_eq!(err_kind, Some(ErrorKind::Forbidden));
    }

    #[test]
    fn effective_scope_sub_org_levels_without_projection_fail_closed() -> Result<(), KernelError> {
        for level in [AccessScopeLevel::Region, AccessScopeLevel::Worksite] {
            let effective = effective_branch_scope_for_tenant(
                BranchScope::All,
                AccessScope::new(level, ScopeNodeId::from_uuid(uuid::Uuid::new_v4())),
                OrgId::new(),
            )?;

            assert_eq!(effective, BranchScope::none());
        }
        Ok(())
    }

    // -----------------------------------------------------------------------
    // authorize_capability — the branch-less primitive
    // -----------------------------------------------------------------------

    fn principal_with(roles: [Role; 1], scope: BranchScope) -> Principal {
        Principal::new(
            UserId::new(),
            OrgId::knl(),
            std::collections::BTreeSet::from(roles),
            scope,
        )
    }

    /// The approval inbox must not empty out. `CompletionReview` is
    /// `[D,D,D,A,D,A]`; routing these sites through `authorize_org_wide` instead
    /// would have collapsed them to SUPER_ADMIN only.
    #[test]
    fn capability_preserves_builtin_admin_on_completion_review() {
        let branch = BranchId::new();
        let admin = principal_with([Role::Admin], BranchScope::single(branch));
        let action = Action::new(Feature::CompletionReview);

        assert!(authorize_capability(&admin, action).is_ok());
        // Same verdict the fabricated-branch call produced.
        assert!(authorize(&admin, action, branch).is_ok());
        // ...and what org-wide routing would have done instead.
        assert!(authorize_org_wide(&admin, action).is_err());
    }

    /// `Feature::TargetManage` is `[D,D,R,A,D,A]` — MECHANIC holds `RequestOnly`.
    /// A hardcoded `== PermissionLevel::Allow` capability check would deny every
    /// mechanic on the request-only target surface.
    #[test]
    fn capability_honors_request_only_permission_level() {
        let mechanic = principal_with([Role::Mechanic], BranchScope::single(BranchId::new()));

        assert!(authorize_capability(&mechanic, Action::request(Feature::TargetManage)).is_ok());
        assert!(authorize_capability(&mechanic, Action::new(Feature::TargetManage)).is_err());
    }

    /// The grant must COVER the principal's scope. A Seoul-only grant does not
    /// authorize a Seoul+Busan principal for an action that reaches both.
    #[test]
    fn capability_denies_grant_narrower_than_principal_scope() {
        let seoul = BranchId::new();
        let busan = BranchId::new();
        let member = principal_with(
            [Role::Member],
            BranchScope::Branches(std::collections::BTreeSet::from([seoul, busan])),
        )
        .with_effective_feature_grants(vec![EffectiveFeatureGrant::new(
            Feature::CompletionReview,
            PermissionLevel::Allow,
            BranchScope::single(seoul),
        )]);

        assert!(authorize_capability(&member, Action::new(Feature::CompletionReview)).is_err());
    }

    /// THE ONE BEHAVIOURAL DELTA, as a differential against what shipped.
    ///
    /// The losing class: ≥2 branches, no built-in role permission, a custom grant
    /// covering only part of the scope. The first two assertions are the
    /// pre-migration verdict for that class — the SAME partial grant admits or
    /// denies depending only on which branch UUID sorts first in the `BTreeSet`,
    /// because the fabricated branch was `branches.iter().next()`. The last two
    /// are this function: denied either way.
    #[test]
    fn capability_replaces_an_order_dependent_grant_verdict_with_a_deny() {
        let mut ids = [BranchId::new(), BranchId::new()];
        ids.sort();
        let [lowest, other] = ids;
        let scope = BranchScope::Branches(std::collections::BTreeSet::from(ids));
        let action = Action::new(Feature::CompletionReview);
        let granted_on = |branch| {
            principal_with([Role::Member], scope.clone()).with_effective_feature_grants(vec![
                EffectiveFeatureGrant::new(
                    Feature::CompletionReview,
                    PermissionLevel::Allow,
                    BranchScope::single(branch),
                ),
            ])
        };

        assert!(
            authorize(&granted_on(lowest), action, lowest).is_ok(),
            "pre-migration: a partial grant on the lowest-sorting branch was ALLOWED"
        );
        assert!(
            authorize(&granted_on(other), action, lowest).is_err(),
            "pre-migration: the same partial grant on the other branch was DENIED"
        );

        assert!(authorize_capability(&granted_on(lowest), action).is_err());
        assert!(authorize_capability(&granted_on(other), action).is_err());
    }

    /// ...and the other direction: a grant that covers more than the principal's
    /// scope still authorizes.
    #[test]
    fn capability_allows_grant_covering_principal_scope() {
        let seoul = BranchId::new();
        let busan = BranchId::new();
        let member = principal_with([Role::Member], BranchScope::single(seoul))
            .with_effective_feature_grants(vec![EffectiveFeatureGrant::new(
                Feature::CompletionReview,
                PermissionLevel::Allow,
                BranchScope::Branches(std::collections::BTreeSet::from([seoul, busan])),
            )]);

        assert!(authorize_capability(&member, Action::new(Feature::CompletionReview)).is_ok());
    }

    /// For an `All`-scoped principal the coverage rule collapses to exactly
    /// `authorize_org_wide`'s grant rule: only an `All` grant passes.
    #[test]
    fn capability_denies_partial_grant_for_all_scoped_principal() {
        let partial = principal_with([Role::Member], BranchScope::All)
            .with_effective_feature_grants(vec![EffectiveFeatureGrant::new(
                Feature::CompletionReview,
                PermissionLevel::Allow,
                BranchScope::single(BranchId::new()),
            )]);
        let org_wide = principal_with([Role::Member], BranchScope::All)
            .with_effective_feature_grants(vec![EffectiveFeatureGrant::new(
                Feature::CompletionReview,
                PermissionLevel::Allow,
                BranchScope::All,
            )]);

        assert!(authorize_capability(&partial, Action::new(Feature::CompletionReview)).is_err());
        assert!(authorize_capability(&org_wide, Action::new(Feature::CompletionReview)).is_ok());
    }

    /// A principal with zero memberships is denied whatever it holds — matching
    /// every helper this primitive replaced.
    #[test]
    fn capability_denies_empty_branch_scope_regardless_of_role() {
        for role in Role::ALL {
            let principal = principal_with([role], BranchScope::none())
                .with_effective_feature_grants(vec![EffectiveFeatureGrant::new(
                    Feature::Login,
                    PermissionLevel::Allow,
                    BranchScope::All,
                )]);
            assert!(
                authorize_capability(&principal, Action::new(Feature::Login)).is_err(),
                "{role:?} with empty scope must be denied"
            );
        }
    }

    /// The no-outage property, stated as a differential: with no custom grants,
    /// `authorize_capability` agrees with `authorize` for EVERY branch in the
    /// principal's scope — including the one the fabrication happened to pick.
    #[test]
    fn capability_agrees_with_authorize_on_every_in_scope_branch() {
        let branches = [BranchId::new(), BranchId::new(), BranchId::new()];
        let scope = BranchScope::Branches(branches.into_iter().collect());

        for role in Role::ALL {
            let principal = principal_with([role], scope.clone());
            for feature in Feature::ALL {
                for action in [
                    Action::new(feature),
                    Action::limited(feature),
                    Action::request(feature),
                ] {
                    let capability = authorize_capability(&principal, action).is_ok();
                    for branch in branches {
                        assert_eq!(
                            capability,
                            authorize(&principal, action, branch).is_ok(),
                            "{role:?}/{feature:?} disagreed on an in-scope branch"
                        );
                    }
                }
            }
        }
    }

    /// The branch dimension is still real where a branch exists: an out-of-scope
    /// resource branch denies through `authorize`. `authorize_capability` is for
    /// resources that have no branch, not a way to skip this check.
    #[test]
    fn authorize_still_denies_a_resource_branch_outside_scope() {
        let mine = BranchId::new();
        let theirs = BranchId::new();
        let admin = principal_with([Role::Admin], BranchScope::single(mine));
        let action = Action::new(Feature::CompletionReview);

        assert!(authorize(&admin, action, mine).is_ok());
        assert!(authorize(&admin, action, theirs).is_err());
    }

    /// STRICTER OR EQUAL, for every principal shape a migrated site can meet.
    ///
    /// `before` reproduces the fabricated-branch call verbatim — `BranchId::new()`
    /// for `All`, `branches.iter().next()` otherwise — and `after` is the
    /// primitive that replaced it, over all 96 features × all three permission
    /// levels. The assertion runs one way only: nothing `after` admits may have
    /// been denied `before`. Widening is what F1 proved can happen by accident, so
    /// it is asserted rather than argued.
    ///
    /// Run with `--nocapture` to print the per-shape before/after counts.
    #[test]
    fn capability_is_stricter_or_equal_for_every_principal_shape() {
        let branch = BranchId::new();
        let second = BranchId::new();
        let grant = |feature, scope| {
            vec![EffectiveFeatureGrant::new(
                feature,
                PermissionLevel::Allow,
                scope,
            )]
        };

        let shapes: Vec<(&str, Principal)> = vec![
            (
                "group-admin ADMIN ({Admin}+All)",
                principal_with([Role::Admin], BranchScope::All),
            ),
            (
                "branch-scoped ADMIN",
                principal_with([Role::Admin], BranchScope::single(branch)),
            ),
            (
                "MECHANIC",
                principal_with([Role::Mechanic], BranchScope::single(branch)),
            ),
            (
                "RECEPTIONIST",
                principal_with([Role::Receptionist], BranchScope::single(branch)),
            ),
            (
                "Executive",
                principal_with([Role::Executive], BranchScope::All),
            ),
            (
                "SuperAdmin",
                principal_with([Role::SuperAdmin], BranchScope::All),
            ),
            (
                "empty-scope principal",
                principal_with([Role::Admin], BranchScope::none()),
            ),
            (
                "grant-only (covering)",
                principal_with([Role::Member], BranchScope::single(branch))
                    .with_effective_feature_grants(grant(
                        Feature::CompletionReview,
                        BranchScope::single(branch),
                    )),
            ),
            (
                "grant-only (partial, 2 branches)",
                principal_with(
                    [Role::Member],
                    BranchScope::Branches(std::collections::BTreeSet::from([branch, second])),
                )
                .with_effective_feature_grants(grant(
                    Feature::CompletionReview,
                    BranchScope::single(*[branch, second].iter().min().unwrap()),
                )),
            ),
        ];

        for (label, principal) in &shapes {
            let (mut before_allowed, mut after_allowed) = (0usize, 0usize);
            for feature in Feature::ALL {
                for action in [
                    Action::new(feature),
                    Action::limited(feature),
                    Action::request(feature),
                ] {
                    let fabricated = match &principal.branch_scope {
                        // fabricated-branch: ok this IS the fabrication, reproduced as a test fixture
                        BranchScope::All => Some(BranchId::new()),
                        // fabricated-branch: ok same, the `Branches` half of the same fixture
                        BranchScope::Branches(branches) => branches.iter().next().copied(),
                    };
                    let before =
                        fabricated.is_some_and(|b| authorize(principal, action, b).is_ok());
                    let after = authorize_capability(principal, action).is_ok();
                    before_allowed += usize::from(before);
                    after_allowed += usize::from(after);
                    assert!(
                        !(after && !before),
                        "{label}: {feature:?}/{:?} is WIDER after the migration",
                        action.required_permission()
                    );
                }
            }
            println!("{label}: before={before_allowed} after={after_allowed} allows");
        }
    }

    /// NOTHING BROKE, site by site. Every feature that a migrated
    /// fabricated-branch helper gates, with the verdict a branch-scoped built-in
    /// ADMIN gets today. `false` entries are ADMIN-DENY in the matrix and must
    /// stay denied; every `true` entry is a surface an ADMIN uses today and must
    /// keep — the `CompletionReview` row is the approval inbox.
    ///
    /// Each row also asserts the fabricated-branch call this replaced agrees, so
    /// the table cannot drift from the behavior it is protecting.
    #[test]
    fn migrated_sites_keep_todays_admin_verdict() {
        let branch = BranchId::new();
        let admin = principal_with([Role::Admin], BranchScope::single(branch));

        let sites: &[(&str, Action, bool)] = &[
            // workorder/rest authorize_feature_in_scope — the approval inbox.
            (
                "workorder: CompletionReview",
                Action::new(Feature::CompletionReview),
                true,
            ),
            (
                "workorder: WorkOrderReadAll",
                Action::new(Feature::WorkOrderReadAll),
                true,
            ),
            (
                "workorder: DailyPlanRequest",
                Action::new(Feature::DailyPlanRequest),
                true,
            ),
            (
                "workorder: DailyPlanReview",
                Action::new(Feature::DailyPlanReview),
                true,
            ),
            (
                "workorder: OrgWideQueueTriage (ADMIN deny today)",
                Action::new(Feature::OrgWideQueueTriage),
                false,
            ),
            (
                "workorder: TargetManage",
                Action::new(Feature::TargetManage),
                true,
            ),
            // identity/rest authorize_org_manage.
            (
                "identity: UserManage",
                Action::new(Feature::UserManage),
                true,
            ),
            ("identity: Login", Action::new(Feature::Login), true),
            (
                "identity: RoleManage (ADMIN deny today)",
                Action::new(Feature::RoleManage),
                false,
            ),
            (
                "identity: RegionManage",
                Action::new(Feature::RegionManage),
                true,
            ),
            (
                "identity: BranchManage",
                Action::new(Feature::BranchManage),
                true,
            ),
            (
                "identity: ElevatedRoleGrant (ADMIN deny today)",
                Action::new(Feature::ElevatedRoleGrant),
                false,
            ),
            // sales/rest, analytics-quant/rest, comms/rest.
            (
                "sales: SalesManage",
                Action::new(Feature::SalesManage),
                true,
            ),
            ("analytics: KpiRead", Action::new(Feature::KpiRead), true),
            (
                "comms: MailAccountManage",
                Action::new(Feature::MailAccountManage),
                true,
            ),
            ("comms: MailUse", Action::new(Feature::MailUse), true),
            // compliance/integrity — ADMIN is DENY today and stays DENY.
            (
                "integrity: IntegrityFindingsRead (ADMIN deny today)",
                Action::new(Feature::IntegrityFindingsRead),
                false,
            ),
            (
                "integrity: IntegrityFindingTriage (ADMIN deny today)",
                Action::new(Feature::IntegrityFindingTriage),
                false,
            ),
            // inspection/rest list gates.
            (
                "inspection: InspectionScheduleManage",
                Action::new(Feature::InspectionScheduleManage),
                true,
            ),
            (
                "inspection: InspectionRoundComplete",
                Action::new(Feature::InspectionRoundComplete),
                true,
            ),
            // evaluation/rest, leave/rest + benefit/rest Branches arm.
            (
                "evaluation: EvaluationRead",
                Action::new(Feature::EvaluationRead),
                true,
            ),
            (
                "evaluation: EvaluationManage",
                Action::new(Feature::EvaluationManage),
                true,
            ),
            (
                "evaluation: EvaluationSubmit",
                Action::new(Feature::EvaluationSubmit),
                true,
            ),
            (
                "leave: EmployeeDirectoryRead",
                Action::new(Feature::EmployeeDirectoryRead),
                true,
            ),
            (
                "leave: EmployeeDirectoryManage",
                Action::new(Feature::EmployeeDirectoryManage),
                true,
            ),
            // app/src/lib.rs authorize_audit_read.
            (
                "app: AuditLogRead",
                Action::new(Feature::AuditLogRead),
                true,
            ),
            // support/rest list gates share Login + WorkOrderReadAll, above.
        ];

        for (site, action, admin_allowed) in sites {
            assert_eq!(
                authorize_capability(&admin, *action).is_ok(),
                *admin_allowed,
                "{site}: migrated verdict changed for a branch-scoped ADMIN"
            );
            assert_eq!(
                authorize(&admin, *action, branch).is_ok(),
                *admin_allowed,
                "{site}: the pre-migration fabricated-branch call disagrees, so \
                 this table no longer describes what shipped"
            );
        }
    }

    // -----------------------------------------------------------------------
    // `authorize_scoped` — the branchless/branch spine with decision-time grants
    // -----------------------------------------------------------------------

    fn ts(secs: i64) -> Timestamp {
        Timestamp::from_unix_timestamp(secs).unwrap()
    }

    /// `None` is branch-less capability authorization, byte-for-byte.
    #[test]
    fn none_is_branchless_capability_authorization_for_every_principal_shape() {
        let branches = [BranchId::new(), BranchId::new()];
        let scopes = [
            BranchScope::All,
            BranchScope::single(branches[0]),
            BranchScope::Branches(branches.into_iter().collect()),
            BranchScope::none(),
        ];

        for role in Role::ALL {
            for scope in &scopes {
                let principal = principal_with([role], scope.clone());
                for feature in Feature::ALL {
                    for action in [
                        Action::new(feature),
                        Action::limited(feature),
                        Action::request(feature),
                    ] {
                        assert_eq!(
                            authorize_capability_at(&principal, action, ts(1_000)).is_ok(),
                            authorize_capability(&principal, action).is_ok(),
                            "{role:?}/{feature:?}: the branch-less door must BE authorize_capability"
                        );
                    }
                }
            }
        }
    }

    /// `Some(b)` is branch authorization, byte-for-byte — including the deny for
    /// a branch outside the principal's scope.
    #[test]
    fn some_is_branch_authorization_for_every_principal_shape() {
        let mine = BranchId::new();
        let theirs = BranchId::new();

        for role in Role::ALL {
            for scope in [BranchScope::All, BranchScope::single(mine)] {
                let principal = principal_with([role], scope);
                for feature in Feature::ALL {
                    let action = Action::new(feature);
                    for branch in [mine, theirs] {
                        assert_eq!(
                            authorize_scoped(
                                &principal,
                                action,
                                ResourceBranch::for_test(principal.org_id, branch),
                                ts(1_000),
                            )
                            .is_ok(),
                            authorize(&principal, action, branch).is_ok(),
                            "{role:?}/{feature:?}: a resource branch must BE authorize"
                        );
                    }
                }
            }
        }
    }

    /// CROSS-TENANT. The tenant a branch was read under is part of what makes a
    /// `ResourceBranch` a proof: a row id is a bare uuid, so a call site that
    /// takes the org from the same request path as the id (`path.org_id`) can
    /// name a tenant the principal does not belong to. The lookup then succeeds
    /// against THAT tenant's row, and for a `BranchScope::All` principal
    /// `allows(branch)` is unconditionally true — an ALLOW against another
    /// tenant's resource.
    ///
    /// So the decision checks the org too, and the principal is the only side
    /// that can supply it.
    #[test]
    fn a_resource_branch_read_under_another_tenant_is_denied_however_wide_the_scope() {
        let branch = BranchId::new();
        let principal = principal_with([Role::Admin], BranchScope::All);
        let action = Action::new(Feature::CompletionReview);
        let other_tenant = OrgId::from_uuid(uuid::Uuid::from_u128(0xb0b));
        assert_ne!(other_tenant, principal.org_id);

        // THE CONTROL: the same branch, read under the principal's own org, is
        // an ordinary allow — so the refusal below is the tenant and nothing
        // else about the fixture.
        assert!(
            authorize_scoped(
                &principal,
                action,
                ResourceBranch::for_test(principal.org_id, branch),
                ts(1),
            )
            .is_ok()
        );

        let err = authorize_scoped(
            &principal,
            action,
            ResourceBranch::for_test(other_tenant, branch),
            ts(1),
        )
        .unwrap_err();
        assert_eq!(err.kind, ErrorKind::Forbidden);
    }

    /// The fold every REST list gate performs over
    /// `principal.effective_feature_grants` verbatim: match on feature +
    /// permission, take the grant's `branch_scope`. `inventory/rest`'s and
    /// `dispatch/rest`'s `custom_feature_scope`, and
    /// `identity/rest`'s `principal_holds_policy_permission`, are all this
    /// shape. None of them takes a decision instant, so none of them can
    /// consult an interval — which is exactly why an effective-dated grant
    /// must be UNREACHABLE from this field rather than merely filtered out of
    /// it by every reader remembering to.
    fn scope_a_time_blind_caller_reads(
        principal: &Principal,
        feature: Feature,
    ) -> Vec<BranchScope> {
        principal
            .effective_feature_grants
            .iter()
            .filter(|grant| {
                grant.feature == feature && grant.permission.satisfies(PermissionLevel::Allow)
            })
            .map(|grant| grant.branch_scope.clone())
            .collect()
    }

    /// An expired grant must be invisible to a reader that never asked what
    /// time it is — including a reader outside this crate that this crate
    /// cannot patch. The guarantee has to be that the field CANNOT hold one.
    #[test]
    fn an_expired_grant_is_unreachable_from_the_time_blind_grant_list() {
        let branch = BranchId::new();
        let expired = principal_with([Role::Member], BranchScope::single(branch))
            .with_dated_feature_grants(vec![DatedFeatureGrant::new(
                EffectiveFeatureGrant::new(
                    Feature::WorkOrderCreate,
                    PermissionLevel::Allow,
                    BranchScope::single(branch),
                ),
                GrantValidity::half_open(ts(100), Some(ts(200))).unwrap(),
            )]);

        assert!(
            scope_a_time_blind_caller_reads(&expired, Feature::WorkOrderCreate).is_empty(),
            "a dated grant must never appear in the field time-blind callers fold over"
        );
        assert!(
            authorize_capability_at(&expired, Action::new(Feature::WorkOrderCreate), ts(250))
                .is_err(),
            "and the decision-time door must still deny it after valid_to"
        );
    }

    fn dated_grant(validity: GrantValidity) -> Principal {
        let branch = BranchId::new();
        principal_with([Role::Member], BranchScope::single(branch)).with_dated_feature_grants(vec![
            DatedFeatureGrant::new(
                EffectiveFeatureGrant::new(
                    Feature::WorkOrderCreate,
                    PermissionLevel::Allow,
                    BranchScope::single(branch),
                ),
                validity,
            ),
        ])
    }

    /// Both ends of the grant interval, probed explicitly through a real
    /// authorization decision — not just through `GrantValidity::contains`.
    #[test]
    fn a_dated_grant_is_effective_from_valid_from_inclusive_to_valid_to_exclusive() {
        let principal = dated_grant(GrantValidity::half_open(ts(100), Some(ts(200))).unwrap());
        let action = Action::new(Feature::WorkOrderCreate);

        assert!(authorize_capability_at(&principal, action, ts(99)).is_err());
        assert!(
            authorize_capability_at(&principal, action, ts(100)).is_ok(),
            "valid_from is INSIDE the interval"
        );
        assert!(authorize_capability_at(&principal, action, ts(199)).is_ok());
        assert!(
            authorize_capability_at(&principal, action, ts(200)).is_err(),
            "valid_to is OUTSIDE the interval"
        );
        assert!(authorize_capability_at(&principal, action, ts(201)).is_err());
    }

    /// The same two ends on the branch-scoped door, so it cannot quietly skip the
    /// interval predicate the branch-less door enforces.
    #[test]
    fn the_branch_arm_honors_both_interval_ends_too() {
        let branch = BranchId::new();
        let principal = principal_with([Role::Member], BranchScope::single(branch))
            .with_dated_feature_grants(vec![DatedFeatureGrant::new(
                EffectiveFeatureGrant::new(
                    Feature::WorkOrderCreate,
                    PermissionLevel::Allow,
                    BranchScope::single(branch),
                ),
                GrantValidity::half_open(ts(100), Some(ts(200))).unwrap(),
            )]);
        let action = Action::new(Feature::WorkOrderCreate);
        let resource = ResourceBranch::for_test(principal.org_id, branch);

        assert!(authorize_scoped(&principal, action, resource, ts(99)).is_err());
        assert!(authorize_scoped(&principal, action, resource, ts(100)).is_ok());
        assert!(authorize_scoped(&principal, action, resource, ts(199)).is_ok());
        assert!(authorize_scoped(&principal, action, resource, ts(200)).is_err());
    }

    /// ADR-0032 §3/§6: the grant set is resolved WHEN THE DECISION IS MADE.
    /// Crossing an interval boundary is not a write, so no freshness counter can
    /// record it; the same principal value must yield different verdicts at
    /// different instants from the same call site.
    #[test]
    fn the_grant_set_resolves_at_decision_time_not_once_per_principal() {
        let principal = dated_grant(GrantValidity::half_open(ts(100), Some(ts(200))).unwrap());
        let action = Action::new(Feature::WorkOrderCreate);

        let verdicts: Vec<bool> = [ts(50), ts(150), ts(250), ts(150)]
            .into_iter()
            .map(|at| authorize_capability_at(&principal, action, at).is_ok())
            .collect();

        assert_eq!(verdicts, vec![false, true, false, true]);
    }

    /// FAIL CLOSED. `authorize`, `authorize_capability` and `authorize_org_wide`
    /// take no instant, so they cannot evaluate an interval. An effective-dated
    /// grant is therefore INVISIBLE to them — it can never widen a time-blind
    /// decision, not even inside its own interval.
    #[test]
    fn an_effective_dated_grant_is_invisible_to_the_time_blind_entry_points() {
        let branch = BranchId::new();
        let scope = BranchScope::single(branch);
        let grant = EffectiveFeatureGrant::new(
            Feature::WorkOrderCreate,
            PermissionLevel::Allow,
            scope.clone(),
        );
        let action = Action::new(Feature::WorkOrderCreate);

        let undated = principal_with([Role::Member], scope.clone())
            .with_effective_feature_grants(vec![grant.clone()]);
        let dated = principal_with([Role::Member], scope).with_dated_feature_grants(vec![
            DatedFeatureGrant::new(
                grant,
                GrantValidity::half_open(ts(100), Some(ts(200))).unwrap(),
            ),
        ]);

        // The undated grant is what ships today, and it still authorizes.
        assert!(authorize_capability(&undated, action).is_ok());
        assert!(authorize(&undated, action, branch).is_ok());

        // The dated one does not, at any instant, through a time-blind door.
        assert!(authorize_capability(&dated, action).is_err());
        assert!(authorize(&dated, action, branch).is_err());
        // ...but it does through the decision-time door, inside its interval.
        assert!(authorize_capability_at(&dated, action, ts(150)).is_ok());
    }

    /// An all-branch principal whose org-wide authority comes only from a dated
    /// custom grant must not pass the time-blind org-wide gate either.
    #[test]
    fn an_effective_dated_grant_is_invisible_to_authorize_org_wide() {
        let action = Action::new(Feature::WorkOrderCreate);
        let grant = EffectiveFeatureGrant::new(
            Feature::WorkOrderCreate,
            PermissionLevel::Allow,
            BranchScope::All,
        );

        let undated = principal_with([Role::Member], BranchScope::All)
            .with_effective_feature_grants(vec![grant.clone()]);
        let dated =
            principal_with([Role::Member], BranchScope::All).with_dated_feature_grants(vec![
                DatedFeatureGrant::new(
                    grant,
                    GrantValidity::half_open(ts(100), Some(ts(200))).unwrap(),
                ),
            ]);

        assert!(authorize_org_wide(&undated, action).is_ok());
        assert!(authorize_org_wide(&dated, action).is_err());
    }

    /// ADR-0032 §1 is enforced HERE or nowhere. `enforce_assignment_non_overlap`
    /// was defined, unit tested and never called: every reference to it in the
    /// tree was its own definition, its re-export or its own tests, and the
    /// resolver folded straight over the assignment rows. A predicate that is
    /// never applied is not a guard.
    ///
    /// The fold lives behind the check, so grants cannot be produced without it
    /// having run — and this test says what happens when it does. Delete the
    /// `enforce_assignment_non_overlap(..)?` line and the second half returns
    /// one grant instead of a `Conflict`.
    ///
    /// The input is the ROWS, not a hand-built assignment set: the checked set
    /// used to be a separate argument, and an argument the resolver supplies is
    /// an argument a resolver edit can supply empty — with the guard still
    /// nominally called, still returning `Ok`, and no test able to tell. There
    /// is no such argument now, so this drives the same derivation production
    /// drives.
    #[test]
    fn an_ambiguous_assignment_set_refuses_to_produce_grants() {
        let role = uuid::Uuid::from_u128(0xa);
        let scopes = BTreeMap::from([(role, BranchScope::All)]);
        let row = |assignment: u128| RuntimePolicyPermissionRow {
            assignment_id: uuid::Uuid::from_u128(assignment),
            role_id: role,
            feature_key: Feature::WorkOrderCreate.as_str().to_owned(),
            permission_level: PermissionLevel::Allow.as_str().to_owned(),
        };

        // THE CONTROL: one 발령 for the role folds into one grant, so the
        // refusal below is the overlap and not the fixture.
        let unambiguous = effective_grants_from_checked_assignments(vec![row(1)], &scopes).unwrap();
        assert_eq!(unambiguous.len(), 1);

        // Two records claiming when this one authority began. The resolver must
        // refuse the set, not pick one and fold.
        let ambiguous =
            effective_grants_from_checked_assignments(vec![row(1), row(2)], &scopes).unwrap_err();
        assert_eq!(ambiguous.kind, ErrorKind::Conflict);
    }

    /// The guard above is only a guard if the resolver hands it one entry per
    /// 발령. The permission join repeats an assignment once per permission, so
    /// the rows are folded to the assignment first — and that fold is where the
    /// guard can be switched off without any other test noticing: key the map
    /// by `role_id` instead of the assignment id and two records for one role
    /// collapse into one entry, after which `enforce_assignment_non_overlap`
    /// can never fire again.
    #[test]
    fn one_assignment_is_one_entry_however_many_permissions_it_carries() {
        let role = uuid::Uuid::from_u128(0xa);
        let row = |assignment: u128, feature: Feature| RuntimePolicyPermissionRow {
            assignment_id: uuid::Uuid::from_u128(assignment),
            role_id: role,
            feature_key: feature.as_str().to_owned(),
            permission_level: PermissionLevel::Allow.as_str().to_owned(),
        };

        // One 발령 carrying two permissions is ONE assignment. Two entries here
        // would refuse every user who holds a role with more than one feature.
        let one_appointment = assignment_validities(&[
            row(1, Feature::WorkOrderCreate),
            row(1, Feature::CompletionReview),
        ]);
        assert_eq!(one_appointment.len(), 1);

        // Two 발령 for the same role are TWO assignments — the ambiguity the
        // guard exists to refuse. One entry here is the guard going silent.
        let two_appointments = assignment_validities(&[
            row(1, Feature::WorkOrderCreate),
            row(2, Feature::WorkOrderCreate),
        ]);
        assert_eq!(two_appointments.len(), 2);
        assert!(enforce_assignment_non_overlap(&two_appointments).is_err());
    }

    /// A grant carrying no interval keeps today's behaviour exactly, on every
    /// door. This is the whole no-regression claim of the temporal change.
    #[test]
    fn an_interval_less_grant_decides_identically_on_every_entry_point() {
        let branch = BranchId::new();
        let scope = BranchScope::single(branch);

        for feature in Feature::ALL {
            for permission in [PermissionLevel::Allow, PermissionLevel::Limited] {
                let principal = principal_with([Role::Member], scope.clone())
                    .with_effective_feature_grants(vec![EffectiveFeatureGrant::new(
                        feature,
                        permission,
                        scope.clone(),
                    )]);
                for action in [
                    Action::new(feature),
                    Action::limited(feature),
                    Action::request(feature),
                ] {
                    assert_eq!(
                        authorize_capability_at(&principal, action, ts(1)).is_ok(),
                        authorize_capability(&principal, action).is_ok(),
                        "{feature:?}/{permission:?}: branchless verdict moved"
                    );
                    assert_eq!(
                        authorize_scoped(
                            &principal,
                            action,
                            ResourceBranch::for_test(principal.org_id, branch),
                            ts(1),
                        )
                        .is_ok(),
                        authorize(&principal, action, branch).is_ok(),
                        "{feature:?}/{permission:?}: branch verdict moved"
                    );
                }
            }
        }
    }

    /// Totality pin for custom-role runtime restrictions, including the retired
    /// v1 AI permission. Historical rows must never resolve to live grants.
    #[test]
    fn custom_role_runtime_feature_allowed_blocks_only_the_safety_set() {
        let blocked = [
            Feature::RoleManage,
            Feature::ElevatedRoleGrant,
            Feature::OrgWideQueueTriage,
            Feature::AuditStreamRead,
            Feature::AuditStreamAccessLogRead,
            Feature::AiAssist,
        ];
        for feature in Feature::ALL {
            let expected = !blocked.contains(&feature);
            assert_eq!(
                custom_role_runtime_feature_allowed(feature),
                expected,
                "{feature:?}"
            );
        }
        assert!(!custom_role_runtime_feature_allowed(Feature::AiAssist));
    }

    #[test]
    fn retired_ai_grant_cannot_authorize_a_scoped_resource() {
        let branch = BranchId::new();
        let principal = principal_with([Role::SuperAdmin], BranchScope::single(branch))
            .with_effective_feature_grants(vec![EffectiveFeatureGrant::new(
                Feature::AiAssist,
                PermissionLevel::Allow,
                BranchScope::single(branch),
            )]);
        for action in [
            Action::new(Feature::AiAssist),
            Action::limited(Feature::AiAssist),
            Action::request(Feature::AiAssist),
        ] {
            assert!(
                authorize_scoped(
                    &principal,
                    action,
                    ResourceBranch::for_test(principal.org_id, branch),
                    ts(1),
                )
                .is_err()
            );
        }
    }
}
