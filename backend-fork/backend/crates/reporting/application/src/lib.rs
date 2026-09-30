//! Reporting application layer.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

use std::future::Future;

use console_kernel_core::{BranchScope, KernelError, Timestamp, TraceContext, UserId};
pub use console_reporting_domain::{
    AnalyticsDefinitionVersion, AnalyticsEvidence, AnalyticsFactQueryIdentity, AnalyticsMetric,
    AnalyticsPeriod, AnalyticsSourceDomain, DailyStatusReport, DailyStatusRow, DashboardAnalytics,
    DashboardAnalyticsScope, DurationEvidence, ExportSourceNote, KpiMetric, KpiReport, KpiRollup,
    KpiRollupScope, KpiScope, LaborCostAnalytics, LaborCostAnalyticsScope, MetricAvailability,
    MetricUnavailable, OpsEquipmentStatus, OpsFunnel, OpsMechanicLoad, OpsSummary, Period,
    PeriodicInspectionRow, RatioEvidence, SumEvidence, TrendSlot, UnavailableMetric,
    WorkDiaryActionEntry, WorkDiaryBody, WorkDiaryDraft, WorkDiaryStatus,
};
use time::Date;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KpiQuery {
    pub period: Period,
    pub scope: KpiScope,
    pub branch_scope: BranchScope,
}

/// Audited KPI workbook download request. Carries the same aggregation inputs as
/// `KpiQuery` plus the actor/trace/timestamp needed to record the download in
/// `excel_export_logs` + `audit_events`, exactly like the sibling
/// daily-status / work-diary exports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KpiExportQuery {
    pub actor: UserId,
    pub period: Period,
    pub scope: KpiScope,
    pub branch_scope: BranchScope,
    pub trace: TraceContext,
    pub occurred_at: Timestamp,
}

#[derive(Debug, thiserror::Error)]
pub enum KpiQueryError {
    #[error(transparent)]
    Kernel(#[from] KernelError),

    #[error("database error: {0}")]
    Database(String),
}

pub trait KpiQueryPort {
    fn query_kpis(
        &self,
        query: KpiQuery,
    ) -> impl Future<Output = Result<KpiReport, KpiQueryError>> + Send + '_;
}

/// Authenticated request context supplied by the application boundary.
///
/// This type deliberately has no wire deserialization: identity, trace, and
/// time are trusted only after authentication middleware has established them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnalyticsReadContext {
    actor: UserId,
    trace: TraceContext,
    occurred_at: Timestamp,
}

impl AnalyticsReadContext {
    pub fn new(
        actor: UserId,
        trace: TraceContext,
        occurred_at: Timestamp,
    ) -> Result<Self, KernelError> {
        if occurred_at.offset() != time::UtcOffset::UTC {
            return Err(KernelError::validation(
                "analytics read context occurred_at must be UTC",
            ));
        }
        Ok(Self {
            actor,
            trace,
            occurred_at,
        })
    }

    #[must_use]
    pub fn actor(&self) -> UserId {
        self.actor
    }

    #[must_use]
    pub fn trace(&self) -> &TraceContext {
        &self.trace
    }

    #[must_use]
    pub fn occurred_at(&self) -> Timestamp {
        self.occurred_at
    }
}

/// The analytics vertical a fact capability was issued for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnalyticsVertical {
    Dashboard,
    LaborCost,
}

/// The exact resolved business scope captured by an issued summary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnalyticsResolvedScope {
    Dashboard(DashboardAnalyticsScope),
    LaborCost(LaborCostAnalyticsScope),
}

/// The complete immutable binding an adapter must persist with a fact
/// capability. It includes the authorization scope because reporting RLS is
/// tenant-level, not branch-level.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnalyticsFactBinding {
    actor: UserId,
    vertical: AnalyticsVertical,
    resolved_scope: AnalyticsResolvedScope,
    authorized_branch_scope: BranchScope,
    fact_query_identity: AnalyticsFactQueryIdentity,
}

impl AnalyticsFactBinding {
    pub fn new(
        actor: UserId,
        vertical: AnalyticsVertical,
        resolved_scope: AnalyticsResolvedScope,
        authorized_branch_scope: BranchScope,
        fact_query_identity: AnalyticsFactQueryIdentity,
    ) -> Result<Self, KernelError> {
        let scope_matches_vertical = matches!(
            (vertical, resolved_scope),
            (
                AnalyticsVertical::Dashboard,
                AnalyticsResolvedScope::Dashboard(_)
            ) | (
                AnalyticsVertical::LaborCost,
                AnalyticsResolvedScope::LaborCost(_)
            )
        );
        if !scope_matches_vertical {
            return Err(KernelError::validation(
                "analytics fact binding scope does not match its vertical",
            ));
        }
        if let AnalyticsResolvedScope::Dashboard(DashboardAnalyticsScope::Branch(branch_id)) =
            resolved_scope
            && !authorized_branch_scope.allows(branch_id)
        {
            return Err(KernelError::forbidden(
                "analytics fact binding branch is outside the authorized scope",
            ));
        }
        // Region membership is not represented by `BranchScope`; adapters must
        // resolve a region to its branches and verify that membership before
        // issuing a region capability. Integration tests must cover that check.
        Ok(Self {
            actor,
            vertical,
            resolved_scope,
            authorized_branch_scope,
            fact_query_identity,
        })
    }

    #[must_use]
    pub fn actor(&self) -> UserId {
        self.actor
    }

    #[must_use]
    pub fn vertical(&self) -> AnalyticsVertical {
        self.vertical
    }
    #[must_use]
    pub fn resolved_scope(&self) -> AnalyticsResolvedScope {
        self.resolved_scope
    }
    #[must_use]
    pub fn authorized_branch_scope(&self) -> &BranchScope {
        &self.authorized_branch_scope
    }
    #[must_use]
    pub fn fact_query_identity(&self) -> &AnalyticsFactQueryIdentity {
        &self.fact_query_identity
    }
}

/// Opaque, adapter-minted handle for one persisted summary binding.
///
/// Adapters must issue unguessable values and fail closed unless the stored
/// binding exactly equals the [`AnalyticsFactBinding`] in a page request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnalyticsFactCapability(String);

impl AnalyticsFactCapability {
    /// Accepts a value returned by an adapter's summary issuance path; it is
    /// not a client-selected fact predicate.
    pub fn from_adapter_issued(value: impl Into<String>) -> Result<Self, KernelError> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(KernelError::validation(
                "analytics fact capability cannot be empty",
            ));
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A fact page can only be requested with the capability and complete binding
/// issued alongside its summary; offset pagination and bare predicates are not
/// accepted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnalyticsFactPageQuery {
    context: AnalyticsReadContext,
    capability: AnalyticsFactCapability,
    binding: AnalyticsFactBinding,
    after: Option<AnalyticsFactCursor>,
    limit: u16,
}

impl AnalyticsFactPageQuery {
    pub fn new(
        context: AnalyticsReadContext,
        capability: AnalyticsFactCapability,
        binding: AnalyticsFactBinding,
        after: Option<AnalyticsFactCursor>,
        limit: u16,
    ) -> Result<Self, KernelError> {
        if context.actor() != binding.actor() {
            return Err(KernelError::forbidden(
                "analytics fact page actor does not match the issued binding",
            ));
        }
        if limit == 0 || limit > 100 {
            return Err(KernelError::validation(
                "analytics fact page limit must be between 1 and 100",
            ));
        }
        Ok(Self {
            context,
            capability,
            binding,
            after,
            limit,
        })
    }

    #[must_use]
    pub fn context(&self) -> &AnalyticsReadContext {
        &self.context
    }

    #[must_use]
    pub fn capability(&self) -> &AnalyticsFactCapability {
        &self.capability
    }
    #[must_use]
    pub fn binding(&self) -> &AnalyticsFactBinding {
        &self.binding
    }
    #[must_use]
    pub fn after(&self) -> Option<&AnalyticsFactCursor> {
        self.after.as_ref()
    }
    #[must_use]
    pub fn limit(&self) -> u16 {
        self.limit
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DashboardAnalyticsFactPageQuery(AnalyticsFactPageQuery);

impl DashboardAnalyticsFactPageQuery {
    pub fn new(query: AnalyticsFactPageQuery) -> Result<Self, KernelError> {
        if query.binding().vertical() != AnalyticsVertical::Dashboard {
            return Err(KernelError::validation(
                "dashboard fact page requires a dashboard capability",
            ));
        }
        Ok(Self(query))
    }

    #[must_use]
    pub fn as_inner(&self) -> &AnalyticsFactPageQuery {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaborCostAnalyticsFactPageQuery(AnalyticsFactPageQuery);

impl LaborCostAnalyticsFactPageQuery {
    pub fn new(query: AnalyticsFactPageQuery) -> Result<Self, KernelError> {
        if query.binding().vertical() != AnalyticsVertical::LaborCost {
            return Err(KernelError::validation(
                "labor cost fact page requires a labor cost capability",
            ));
        }
        Ok(Self(query))
    }

    #[must_use]
    pub fn as_inner(&self) -> &AnalyticsFactPageQuery {
        &self.0
    }
}

/// Opaque, adapter-issued cursor. The application never accepts an offset.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnalyticsFactCursor(String);

impl AnalyticsFactCursor {
    pub fn new(value: impl Into<String>) -> Result<Self, KernelError> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(KernelError::validation(
                "analytics fact cursor cannot be empty",
            ));
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnalyticsFact {
    id: AnalyticsFactId,
    occurred_at: Timestamp,
    source_domain: AnalyticsSourceDomain,
    kind: AnalyticsFactKind,
    contribution: AnalyticsFactContribution,
    evidence_href: String,
}

/// The metric contribution represented by a fact. Payroll has no fact kind
/// because gross payroll remains structurally unavailable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnalyticsFactKind {
    CompletedWorkOrder,
    WorkedDuration,
    Readiness,
}

/// A fact's explicit, typed contribution to its metric.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnalyticsFactContribution {
    Count(u64),
    Duration(DurationEvidence),
    Ratio(RatioEvidence),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AnalyticsFactId(String);

impl AnalyticsFactId {
    pub fn new(value: impl Into<String>) -> Result<Self, KernelError> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(KernelError::validation("analytics fact id cannot be empty"));
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl AnalyticsFact {
    pub fn new(
        id: AnalyticsFactId,
        occurred_at: Timestamp,
        source_domain: AnalyticsSourceDomain,
        kind: AnalyticsFactKind,
        contribution: AnalyticsFactContribution,
        evidence_href: impl Into<String>,
    ) -> Result<Self, KernelError> {
        let source_matches_kind = matches!(
            (source_domain, kind),
            (
                AnalyticsSourceDomain::WorkOrders,
                AnalyticsFactKind::CompletedWorkOrder
            ) | (
                AnalyticsSourceDomain::Attendance,
                AnalyticsFactKind::WorkedDuration
            ) | (
                AnalyticsSourceDomain::Readiness,
                AnalyticsFactKind::Readiness
            )
        );
        if !source_matches_kind {
            return Err(KernelError::validation(
                "analytics fact source domain does not match its kind",
            ));
        }
        let contribution_matches_kind = matches!(
            (kind, contribution),
            (
                AnalyticsFactKind::CompletedWorkOrder,
                AnalyticsFactContribution::Count(_)
            ) | (
                AnalyticsFactKind::WorkedDuration,
                AnalyticsFactContribution::Duration(_)
            ) | (
                AnalyticsFactKind::Readiness,
                AnalyticsFactContribution::Ratio(_)
            )
        );
        if !contribution_matches_kind {
            return Err(KernelError::validation(
                "analytics fact contribution does not match its kind",
            ));
        }
        let evidence_href = evidence_href.into();
        if evidence_href.trim().is_empty() {
            return Err(KernelError::validation(
                "analytics fact evidence href cannot be empty",
            ));
        }
        Ok(Self {
            id,
            occurred_at,
            source_domain,
            kind,
            contribution,
            evidence_href,
        })
    }

    #[must_use]
    pub fn id(&self) -> &AnalyticsFactId {
        &self.id
    }
    #[must_use]
    pub fn occurred_at(&self) -> Timestamp {
        self.occurred_at
    }
    #[must_use]
    pub fn source_domain(&self) -> AnalyticsSourceDomain {
        self.source_domain
    }
    #[must_use]
    pub fn kind(&self) -> AnalyticsFactKind {
        self.kind
    }
    #[must_use]
    pub fn contribution(&self) -> AnalyticsFactContribution {
        self.contribution
    }
    #[must_use]
    pub fn evidence_href(&self) -> &str {
        &self.evidence_href
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnalyticsFactPage {
    binding: AnalyticsFactBinding,
    facts: Vec<AnalyticsFact>,
    next_cursor: Option<AnalyticsFactCursor>,
}

/// A dashboard summary together with the only capability/binding that may be
/// used to drill into its facts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssuedDashboardAnalytics {
    analytics: DashboardAnalytics,
    capability: AnalyticsFactCapability,
    binding: AnalyticsFactBinding,
}

impl IssuedDashboardAnalytics {
    pub fn new(
        analytics: DashboardAnalytics,
        capability: AnalyticsFactCapability,
        binding: AnalyticsFactBinding,
    ) -> Result<Self, KernelError> {
        if binding.vertical() != AnalyticsVertical::Dashboard
            || binding.resolved_scope()
                != AnalyticsResolvedScope::Dashboard(analytics.resolved_scope())
        {
            return Err(KernelError::validation(
                "issued dashboard capability does not match the resolved summary scope",
            ));
        }
        Ok(Self {
            analytics,
            capability,
            binding,
        })
    }

    #[must_use]
    pub fn analytics(&self) -> &DashboardAnalytics {
        &self.analytics
    }

    pub fn fact_page(
        &self,
        context: AnalyticsReadContext,
        after: Option<AnalyticsFactCursor>,
        limit: u16,
    ) -> Result<DashboardAnalyticsFactPageQuery, KernelError> {
        DashboardAnalyticsFactPageQuery::new(AnalyticsFactPageQuery::new(
            context,
            self.capability.clone(),
            self.binding.clone(),
            after,
            limit,
        )?)
    }
}

/// A company-only labor-cost summary plus its separately issued drill-down
/// capability. The constructor refuses any non-company binding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssuedLaborCostAnalytics {
    analytics: LaborCostAnalytics,
    capability: AnalyticsFactCapability,
    binding: AnalyticsFactBinding,
}

impl IssuedLaborCostAnalytics {
    pub fn new(
        analytics: LaborCostAnalytics,
        capability: AnalyticsFactCapability,
        binding: AnalyticsFactBinding,
    ) -> Result<Self, KernelError> {
        if binding.vertical() != AnalyticsVertical::LaborCost
            || binding.resolved_scope()
                != AnalyticsResolvedScope::LaborCost(analytics.resolved_scope())
        {
            return Err(KernelError::validation(
                "issued labor cost capability does not match the company summary scope",
            ));
        }
        Ok(Self {
            analytics,
            capability,
            binding,
        })
    }

    #[must_use]
    pub fn analytics(&self) -> &LaborCostAnalytics {
        &self.analytics
    }

    pub fn fact_page(
        &self,
        context: AnalyticsReadContext,
        after: Option<AnalyticsFactCursor>,
        limit: u16,
    ) -> Result<LaborCostAnalyticsFactPageQuery, KernelError> {
        LaborCostAnalyticsFactPageQuery::new(AnalyticsFactPageQuery::new(
            context,
            self.capability.clone(),
            self.binding.clone(),
            after,
            limit,
        )?)
    }
}

impl AnalyticsFactPage {
    #[must_use]
    pub fn new(
        binding: AnalyticsFactBinding,
        facts: Vec<AnalyticsFact>,
        next_cursor: Option<AnalyticsFactCursor>,
    ) -> Self {
        Self {
            binding,
            facts,
            next_cursor,
        }
    }
    #[must_use]
    pub fn binding(&self) -> &AnalyticsFactBinding {
        &self.binding
    }
    #[must_use]
    pub fn facts(&self) -> &[AnalyticsFact] {
        &self.facts
    }
    #[must_use]
    pub fn next_cursor(&self) -> Option<&AnalyticsFactCursor> {
        self.next_cursor.as_ref()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum AnalyticsQueryError {
    #[error(transparent)]
    Kernel(#[from] KernelError),
    #[error("analytics read error: {0}")]
    Read(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DashboardAnalyticsQuery {
    context: AnalyticsReadContext,
    period: AnalyticsPeriod,
    requested_scope: DashboardAnalyticsScope,
    branch_scope: BranchScope,
}

impl DashboardAnalyticsQuery {
    pub fn new(
        context: AnalyticsReadContext,
        period: AnalyticsPeriod,
        requested_scope: DashboardAnalyticsScope,
        branch_scope: BranchScope,
    ) -> Result<Self, KernelError> {
        if let DashboardAnalyticsScope::Branch(branch_id) = requested_scope
            && !branch_scope.allows(branch_id)
        {
            return Err(KernelError::forbidden(
                "dashboard branch is outside the authorized scope",
            ));
        }
        Ok(Self {
            context,
            period,
            requested_scope,
            branch_scope,
        })
    }

    #[must_use]
    pub fn context(&self) -> &AnalyticsReadContext {
        &self.context
    }
    #[must_use]
    pub fn period(&self) -> AnalyticsPeriod {
        self.period
    }
    #[must_use]
    pub fn requested_scope(&self) -> DashboardAnalyticsScope {
        self.requested_scope
    }
    #[must_use]
    pub fn branch_scope(&self) -> &BranchScope {
        &self.branch_scope
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaborCostAnalyticsQuery {
    context: AnalyticsReadContext,
    period: AnalyticsPeriod,
    branch_scope: BranchScope,
}

impl LaborCostAnalyticsQuery {
    pub fn new(
        context: AnalyticsReadContext,
        period: AnalyticsPeriod,
        branch_scope: BranchScope,
    ) -> Result<Self, KernelError> {
        AnalyticsPeriod::monthly(period.start(), period.end())?;
        Ok(Self {
            context,
            period,
            branch_scope,
        })
    }

    #[must_use]
    pub fn context(&self) -> &AnalyticsReadContext {
        &self.context
    }
    #[must_use]
    pub fn period(&self) -> AnalyticsPeriod {
        self.period
    }
    #[must_use]
    pub fn branch_scope(&self) -> &BranchScope {
        &self.branch_scope
    }
}

pub trait DashboardAnalyticsPort {
    fn dashboard_analytics(
        &self,
        query: DashboardAnalyticsQuery,
    ) -> impl Future<Output = Result<IssuedDashboardAnalytics, AnalyticsQueryError>> + Send + '_;

    fn dashboard_facts(
        &self,
        query: DashboardAnalyticsFactPageQuery,
    ) -> impl Future<Output = Result<AnalyticsFactPage, AnalyticsQueryError>> + Send + '_;
}

/// Labor cost remains company-only: branch scoping authorizes the read but
/// cannot alter the returned business scope or invent a payroll amount.
pub trait LaborCostAnalyticsPort {
    fn labor_cost_analytics(
        &self,
        query: LaborCostAnalyticsQuery,
    ) -> impl Future<Output = Result<IssuedLaborCostAnalytics, AnalyticsQueryError>> + Send + '_;

    fn labor_cost_facts(
        &self,
        query: LaborCostAnalyticsFactPageQuery,
    ) -> impl Future<Output = Result<AnalyticsFactPage, AnalyticsQueryError>> + Send + '_;
}

/// Per-tenant operational dashboard query.
///
/// `aging_hours` is the threshold past which an unresolved work order is counted
/// as aging; `at_risk_minutes` is the lead time before a P1 accept-window
/// deadline at which a dispatch is flagged at-risk; `top_mechanics` caps the
/// utilization list. All reads run org-scoped under RLS.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OpsSummaryQuery {
    pub aging_hours: u32,
    pub at_risk_minutes: u32,
    pub top_mechanics: u32,
}

pub trait OpsSummaryPort {
    fn ops_summary(
        &self,
        query: OpsSummaryQuery,
    ) -> impl Future<Output = Result<OpsSummary, KpiQueryError>> + Send + '_;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReportingExportQuery {
    pub actor: UserId,
    pub date: Date,
    pub branch_scope: BranchScope,
    pub trace: TraceContext,
    pub occurred_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkDiaryQuery {
    pub actor: UserId,
    pub date: Date,
    pub branch_scope: BranchScope,
    pub trace: TraceContext,
    pub occurred_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkDiaryUpdateCommand {
    pub actor: UserId,
    pub date: Date,
    pub branch_scope: BranchScope,
    pub body: WorkDiaryBody,
    pub trace: TraceContext,
    pub occurred_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkDiaryConfirmCommand {
    pub actor: UserId,
    pub date: Date,
    pub branch_scope: BranchScope,
    pub trace: TraceContext,
    pub occurred_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportedWorkbook {
    pub file_name: String,
    pub content_type: &'static str,
    pub bytes: Vec<u8>,
}

#[derive(Debug, thiserror::Error)]
pub enum ReportingExportError {
    #[error(transparent)]
    Kernel(#[from] KernelError),

    #[error("database error: {0}")]
    Database(String),

    #[error("workbook error: {0}")]
    Workbook(String),
}

impl From<KpiQueryError> for ReportingExportError {
    fn from(error: KpiQueryError) -> Self {
        match error {
            KpiQueryError::Kernel(error) => Self::Kernel(error),
            KpiQueryError::Database(message) => Self::Database(message),
        }
    }
}

pub trait ReportingExportPort {
    fn export_daily_status(
        &self,
        query: ReportingExportQuery,
    ) -> impl Future<Output = Result<ExportedWorkbook, ReportingExportError>> + Send + '_;

    fn export_work_diary(
        &self,
        query: ReportingExportQuery,
    ) -> impl Future<Output = Result<ExportedWorkbook, ReportingExportError>> + Send + '_;

    /// Build the KPI report (identical aggregation to the JSON `query_kpis`
    /// path) as a downloadable Excel workbook. The download is audited exactly
    /// like the sibling daily-status / work-diary exports: one
    /// `excel_export_logs` row plus one `audit_events` row, recorded under RLS.
    fn export_kpi(
        &self,
        query: KpiExportQuery,
    ) -> impl Future<Output = Result<ExportedWorkbook, ReportingExportError>> + Send + '_;
}

pub trait WorkDiaryDraftPort {
    fn get_or_generate_work_diary(
        &self,
        query: WorkDiaryQuery,
    ) -> impl Future<Output = Result<WorkDiaryDraft, ReportingExportError>> + Send + '_;

    fn update_work_diary(
        &self,
        command: WorkDiaryUpdateCommand,
    ) -> impl Future<Output = Result<WorkDiaryDraft, ReportingExportError>> + Send + '_;

    fn confirm_work_diary(
        &self,
        command: WorkDiaryConfirmCommand,
    ) -> impl Future<Output = Result<WorkDiaryDraft, ReportingExportError>> + Send + '_;
}

#[cfg(test)]
mod analytics_contract_tests {
    use super::*;
    use time::macros::datetime;

    fn context(actor: UserId) -> AnalyticsReadContext {
        AnalyticsReadContext::new(
            actor,
            TraceContext::generate(),
            datetime!(2026-07-01 00:00 UTC),
        )
        .unwrap()
    }

    #[test]
    fn fact_page_requires_a_bounded_issued_capability_contract() {
        let actor = UserId::new();
        let identity = AnalyticsFactQueryIdentity::new("dashboard-july-company").unwrap();
        let binding = AnalyticsFactBinding::new(
            actor,
            AnalyticsVertical::Dashboard,
            AnalyticsResolvedScope::Dashboard(DashboardAnalyticsScope::Company),
            BranchScope::All,
            identity,
        )
        .unwrap();
        assert!(AnalyticsFactCapability::from_adapter_issued(" ").is_err());
        let capability =
            AnalyticsFactCapability::from_adapter_issued("adapter-issued-capability").unwrap();
        assert!(
            AnalyticsFactPageQuery::new(
                context(actor),
                capability.clone(),
                binding.clone(),
                None,
                0
            )
            .is_err()
        );
        assert!(
            AnalyticsFactPageQuery::new(
                context(actor),
                capability.clone(),
                binding.clone(),
                None,
                101
            )
            .is_err()
        );
        let query = AnalyticsFactPageQuery::new(
            context(actor),
            capability,
            binding,
            Some(AnalyticsFactCursor::new("opaque-next-page").unwrap()),
            100,
        )
        .unwrap();
        assert_eq!(query.after().unwrap().as_str(), "opaque-next-page");
    }

    #[test]
    fn fact_binding_rejects_cross_vertical_scope_forgery() {
        let identity = AnalyticsFactQueryIdentity::new("labor-cost-july").unwrap();
        assert!(
            AnalyticsFactBinding::new(
                UserId::new(),
                AnalyticsVertical::LaborCost,
                AnalyticsResolvedScope::Dashboard(DashboardAnalyticsScope::Company),
                BranchScope::All,
                identity,
            )
            .is_err()
        );
    }

    #[test]
    fn dashboard_fact_port_rejects_labor_cost_capability() {
        let actor = UserId::new();
        let binding = AnalyticsFactBinding::new(
            actor,
            AnalyticsVertical::LaborCost,
            AnalyticsResolvedScope::LaborCost(LaborCostAnalyticsScope::Company),
            BranchScope::All,
            AnalyticsFactQueryIdentity::new("labor-cost-july").unwrap(),
        )
        .unwrap();
        let query = AnalyticsFactPageQuery::new(
            context(actor),
            AnalyticsFactCapability::from_adapter_issued("labor-capability").unwrap(),
            binding,
            None,
            10,
        )
        .unwrap();
        assert!(DashboardAnalyticsFactPageQuery::new(query).is_err());
    }

    #[test]
    fn fact_binding_keeps_cross_scope_authorization_distinct() {
        let actor = UserId::new();
        let branch_a = console_kernel_core::BranchId::new();
        let branch_b = console_kernel_core::BranchId::new();
        let identity = AnalyticsFactQueryIdentity::new("dashboard-july-branch").unwrap();
        let issued_for_a = AnalyticsFactBinding::new(
            actor,
            AnalyticsVertical::Dashboard,
            AnalyticsResolvedScope::Dashboard(DashboardAnalyticsScope::Branch(branch_a)),
            BranchScope::single(branch_a),
            identity.clone(),
        )
        .unwrap();
        let attempted_for_b = AnalyticsFactBinding::new(
            actor,
            AnalyticsVertical::Dashboard,
            AnalyticsResolvedScope::Dashboard(DashboardAnalyticsScope::Branch(branch_b)),
            BranchScope::single(branch_b),
            identity,
        )
        .unwrap();
        assert_ne!(issued_for_a, attempted_for_b);
        assert!(!issued_for_a.authorized_branch_scope().allows(branch_b));
    }

    #[test]
    fn fact_binding_rejects_branch_outside_authorized_scope() {
        let branch_a = console_kernel_core::BranchId::new();
        let branch_b = console_kernel_core::BranchId::new();
        assert!(
            AnalyticsFactBinding::new(
                UserId::new(),
                AnalyticsVertical::Dashboard,
                AnalyticsResolvedScope::Dashboard(DashboardAnalyticsScope::Branch(branch_a)),
                BranchScope::single(branch_b),
                AnalyticsFactQueryIdentity::new("dashboard-july-branch-a").unwrap(),
            )
            .is_err()
        );
    }

    #[test]
    fn labor_cost_query_accepts_only_whole_utc_months() {
        let actor = UserId::new();
        let month = AnalyticsPeriod::monthly(
            datetime!(2026-07-01 00:00 UTC),
            datetime!(2026-08-01 00:00 UTC),
        )
        .unwrap();
        assert!(LaborCostAnalyticsQuery::new(context(actor), month, BranchScope::All).is_ok());
        let partial = AnalyticsPeriod::new(
            datetime!(2026-07-02 00:00 UTC),
            datetime!(2026-08-01 00:00 UTC),
        )
        .unwrap();
        assert!(LaborCostAnalyticsQuery::new(context(actor), partial, BranchScope::All).is_err());
    }

    #[test]
    fn fact_page_rejects_a_different_authenticated_actor() {
        let actor = UserId::new();
        let binding = AnalyticsFactBinding::new(
            actor,
            AnalyticsVertical::Dashboard,
            AnalyticsResolvedScope::Dashboard(DashboardAnalyticsScope::Company),
            BranchScope::All,
            AnalyticsFactQueryIdentity::new("dashboard-july-company").unwrap(),
        )
        .unwrap();
        assert!(
            AnalyticsFactPageQuery::new(
                context(UserId::new()),
                AnalyticsFactCapability::from_adapter_issued("dashboard-capability").unwrap(),
                binding,
                None,
                10,
            )
            .is_err()
        );
    }

    #[test]
    fn facts_require_an_explicit_kind_matched_contribution() {
        let fact = AnalyticsFact::new(
            AnalyticsFactId::new("attendance-1").unwrap(),
            datetime!(2026-07-01 09:00 UTC),
            AnalyticsSourceDomain::Attendance,
            AnalyticsFactKind::WorkedDuration,
            AnalyticsFactContribution::Duration(DurationEvidence::new(3_601, 1).unwrap()),
            "/attendance/1",
        )
        .unwrap();
        assert_eq!(fact.kind(), AnalyticsFactKind::WorkedDuration);
        assert_eq!(
            fact.contribution(),
            AnalyticsFactContribution::Duration(DurationEvidence::new(3_601, 1).unwrap())
        );
        assert!(
            AnalyticsFact::new(
                AnalyticsFactId::new("attendance-2").unwrap(),
                datetime!(2026-07-01 10:00 UTC),
                AnalyticsSourceDomain::Attendance,
                AnalyticsFactKind::WorkedDuration,
                AnalyticsFactContribution::Count(1),
                "/attendance/2",
            )
            .is_err()
        );
    }

    #[test]
    fn facts_reject_payroll_and_source_kind_mismatches() {
        let duration = AnalyticsFactContribution::Duration(DurationEvidence::new(1, 1).unwrap());
        assert!(
            AnalyticsFact::new(
                AnalyticsFactId::new("work-order-valid").unwrap(),
                datetime!(2026-07-01 09:00 UTC),
                AnalyticsSourceDomain::WorkOrders,
                AnalyticsFactKind::CompletedWorkOrder,
                AnalyticsFactContribution::Count(1),
                "/work-orders/valid",
            )
            .is_ok()
        );
        assert!(
            AnalyticsFact::new(
                AnalyticsFactId::new("readiness-valid").unwrap(),
                datetime!(2026-07-01 09:00 UTC),
                AnalyticsSourceDomain::Readiness,
                AnalyticsFactKind::Readiness,
                AnalyticsFactContribution::Ratio(RatioEvidence::new(1, 1).unwrap()),
                "/readiness/valid",
            )
            .is_ok()
        );
        assert!(
            AnalyticsFact::new(
                AnalyticsFactId::new("payroll-1").unwrap(),
                datetime!(2026-07-01 09:00 UTC),
                AnalyticsSourceDomain::Payroll,
                AnalyticsFactKind::WorkedDuration,
                duration,
                "/payroll/1",
            )
            .is_err()
        );
        assert!(
            AnalyticsFact::new(
                AnalyticsFactId::new("work-order-1").unwrap(),
                datetime!(2026-07-01 09:00 UTC),
                AnalyticsSourceDomain::WorkOrders,
                AnalyticsFactKind::WorkedDuration,
                duration,
                "/work-orders/1",
            )
            .is_err()
        );
        assert!(
            AnalyticsFact::new(
                AnalyticsFactId::new("readiness-1").unwrap(),
                datetime!(2026-07-01 09:00 UTC),
                AnalyticsSourceDomain::Readiness,
                AnalyticsFactKind::CompletedWorkOrder,
                AnalyticsFactContribution::Count(1),
                "/readiness/1",
            )
            .is_err()
        );
    }

    #[test]
    fn dashboard_query_carries_context_and_rejects_unauthorized_branches() {
        let actor = UserId::new();
        let allowed = console_kernel_core::BranchId::new();
        let denied = console_kernel_core::BranchId::new();
        let period = AnalyticsPeriod::new(
            datetime!(2026-07-01 00:00 UTC),
            datetime!(2026-08-01 00:00 UTC),
        )
        .unwrap();
        let query = DashboardAnalyticsQuery::new(
            context(actor),
            period,
            DashboardAnalyticsScope::Branch(allowed),
            BranchScope::single(allowed),
        )
        .unwrap();
        assert_eq!(query.context().actor(), actor);
        assert!(
            DashboardAnalyticsQuery::new(
                context(actor),
                period,
                DashboardAnalyticsScope::Branch(denied),
                BranchScope::single(allowed),
            )
            .is_err()
        );
    }
}
