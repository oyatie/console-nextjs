//! Pure reporting/KPI domain.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

use std::collections::BTreeMap;

use console_kernel_core::{BranchId, RegionId, Timestamp, UserId};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Period {
    #[serde(with = "time::serde::rfc3339")]
    pub start: Timestamp,
    #[serde(with = "time::serde::rfc3339")]
    pub end: Timestamp,
}

/// A validated half-open UTC window used by the analytics verticals.
///
/// This deliberately does not replace the older KPI `Period`: adapters can
/// migrate independently without weakening its currently-public wire shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnalyticsPeriod {
    #[serde(with = "time::serde::rfc3339")]
    start: Timestamp,
    #[serde(with = "time::serde::rfc3339")]
    end: Timestamp,
}

impl AnalyticsPeriod {
    /// Creates a non-empty `[start, end)` period whose endpoints are UTC.
    pub fn new(start: Timestamp, end: Timestamp) -> Result<Self, console_kernel_core::KernelError> {
        if start.offset() != time::UtcOffset::UTC || end.offset() != time::UtcOffset::UTC {
            return Err(console_kernel_core::KernelError::validation(
                "analytics period endpoints must be UTC",
            ));
        }
        if start >= end {
            return Err(console_kernel_core::KernelError::validation(
                "analytics period must be non-empty and half-open",
            ));
        }
        Ok(Self { start, end })
    }

    #[must_use]
    pub fn start(self) -> Timestamp {
        self.start
    }

    #[must_use]
    pub fn end(self) -> Timestamp {
        self.end
    }

    #[must_use]
    pub fn contains(self, value: Timestamp) -> bool {
        value >= self.start && value < self.end
    }

    /// Validates a whole UTC calendar month, ending at the next month's start.
    pub fn monthly(
        start: Timestamp,
        end: Timestamp,
    ) -> Result<Self, console_kernel_core::KernelError> {
        let period = Self::new(start, end)?;
        if start.day() != 1 || start.time() != time::Time::MIDNIGHT {
            return Err(console_kernel_core::KernelError::validation(
                "labor cost period must start at UTC month boundary",
            ));
        }
        let next_month = if start.month() == time::Month::December {
            time::Date::from_calendar_date(start.year() + 1, time::Month::January, 1)
        } else {
            time::Date::from_calendar_date(start.year(), start.month().next(), 1)
        }
        .map_err(|_| {
            console_kernel_core::KernelError::validation("invalid monthly analytics period")
        })?
        .with_time(time::Time::MIDNIGHT)
        .assume_utc();
        if end != next_month {
            return Err(console_kernel_core::KernelError::validation(
                "labor cost period must end at the next UTC month boundary",
            ));
        }
        Ok(period)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AnalyticsDefinitionVersion(String);

impl AnalyticsDefinitionVersion {
    pub fn new(value: impl Into<String>) -> Result<Self, console_kernel_core::KernelError> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(console_kernel_core::KernelError::validation(
                "analytics definition version cannot be empty",
            ));
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnalyticsSourceDomain {
    WorkOrders,
    Attendance,
    Readiness,
    Payroll,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AnalyticsFactQueryIdentity(String);

impl AnalyticsFactQueryIdentity {
    pub fn new(value: impl Into<String>) -> Result<Self, console_kernel_core::KernelError> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(console_kernel_core::KernelError::validation(
                "analytics fact query identity cannot be empty",
            ));
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnalyticsEvidence {
    href: String,
    fact_query_identity: AnalyticsFactQueryIdentity,
    source_domain: AnalyticsSourceDomain,
}

impl AnalyticsEvidence {
    pub fn new(
        href: impl Into<String>,
        fact_query_identity: AnalyticsFactQueryIdentity,
        source_domain: AnalyticsSourceDomain,
    ) -> Result<Self, console_kernel_core::KernelError> {
        let href = href.into();
        if href.trim().is_empty() {
            return Err(console_kernel_core::KernelError::validation(
                "analytics evidence href cannot be empty",
            ));
        }
        Ok(Self {
            href,
            fact_query_identity,
            source_domain,
        })
    }

    #[must_use]
    pub fn href(&self) -> &str {
        &self.href
    }
    #[must_use]
    pub fn fact_query_identity(&self) -> &AnalyticsFactQueryIdentity {
        &self.fact_query_identity
    }
    #[must_use]
    pub fn source_domain(&self) -> AnalyticsSourceDomain {
        self.source_domain
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RatioEvidence {
    numerator: u64,
    denominator: u64,
}
impl RatioEvidence {
    pub fn new(numerator: u64, denominator: u64) -> Result<Self, console_kernel_core::KernelError> {
        if denominator == 0 {
            return Err(console_kernel_core::KernelError::validation(
                "analytics denominator must be non-zero",
            ));
        }
        Ok(Self {
            numerator,
            denominator,
        })
    }
    #[must_use]
    pub fn numerator(self) -> u64 {
        self.numerator
    }
    #[must_use]
    pub fn denominator(self) -> u64 {
        self.denominator
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SumEvidence {
    sum: i64,
    sample_count: u64,
}

/// Exact worked-duration evidence. Values are seconds, never rounded hours.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DurationEvidence {
    total_seconds: u64,
    pair_count: u64,
}

impl DurationEvidence {
    /// Creates duration evidence for one or more matched attendance pairs.
    pub fn new(
        total_seconds: u64,
        pair_count: u64,
    ) -> Result<Self, console_kernel_core::KernelError> {
        if pair_count == 0 {
            return Err(console_kernel_core::KernelError::validation(
                "duration evidence requires at least one attendance pair",
            ));
        }
        Ok(Self {
            total_seconds,
            pair_count,
        })
    }

    /// Combines disjoint duration evidence without losing seconds or pairs.
    pub fn checked_add(self, other: Self) -> Result<Self, console_kernel_core::KernelError> {
        let total_seconds = self
            .total_seconds
            .checked_add(other.total_seconds)
            .ok_or_else(|| {
                console_kernel_core::KernelError::validation("duration evidence seconds overflow")
            })?;
        let pair_count = self
            .pair_count
            .checked_add(other.pair_count)
            .ok_or_else(|| {
                console_kernel_core::KernelError::validation(
                    "duration evidence pair count overflow",
                )
            })?;
        Self::new(total_seconds, pair_count)
    }

    #[must_use]
    pub const fn total_seconds(self) -> u64 {
        self.total_seconds
    }

    #[must_use]
    pub const fn pair_count(self) -> u64 {
        self.pair_count
    }
}
impl SumEvidence {
    #[must_use]
    pub const fn new(sum: i64, sample_count: u64) -> Self {
        Self { sum, sample_count }
    }
    #[must_use]
    pub const fn sum(self) -> i64 {
        self.sum
    }
    #[must_use]
    pub const fn sample_count(self) -> u64 {
        self.sample_count
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MetricUnavailable {
    reason: String,
    source_domain: AnalyticsSourceDomain,
}
impl MetricUnavailable {
    pub fn new(
        reason: impl Into<String>,
        source_domain: AnalyticsSourceDomain,
    ) -> Result<Self, console_kernel_core::KernelError> {
        let reason = reason.into();
        if reason.trim().is_empty() {
            return Err(console_kernel_core::KernelError::validation(
                "analytics unavailable reason cannot be empty",
            ));
        }
        Ok(Self {
            reason,
            source_domain,
        })
    }
    #[must_use]
    pub fn reason(&self) -> &str {
        &self.reason
    }
    #[must_use]
    pub fn source_domain(&self) -> AnalyticsSourceDomain {
        self.source_domain
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "availability", content = "value")]
pub enum MetricAvailability<T> {
    Available(T),
    Unavailable(MetricUnavailable),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnalyticsMetric<T> {
    definition_version: AnalyticsDefinitionVersion,
    availability: MetricAvailability<T>,
    evidence: AnalyticsEvidence,
}
impl<T> AnalyticsMetric<T> {
    #[must_use]
    pub fn new(
        definition_version: AnalyticsDefinitionVersion,
        availability: MetricAvailability<T>,
        evidence: AnalyticsEvidence,
    ) -> Self {
        Self {
            definition_version,
            availability,
            evidence,
        }
    }
    #[must_use]
    pub fn definition_version(&self) -> &AnalyticsDefinitionVersion {
        &self.definition_version
    }
    #[must_use]
    pub fn availability(&self) -> &MetricAvailability<T> {
        &self.availability
    }
    #[must_use]
    pub fn evidence(&self) -> &AnalyticsEvidence {
        &self.evidence
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrendSlot<T> {
    period: AnalyticsPeriod,
    value: MetricAvailability<T>,
}
impl<T> TrendSlot<T> {
    #[must_use]
    pub fn new(period: AnalyticsPeriod, value: MetricAvailability<T>) -> Self {
        Self { period, value }
    }
    #[must_use]
    pub fn period(&self) -> AnalyticsPeriod {
        self.period
    }
    #[must_use]
    pub fn value(&self) -> &MetricAvailability<T> {
        &self.value
    }
}

/// Company/region/branch resolution exactly as evaluated by the analytics read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "id", rename_all = "snake_case")]
pub enum DashboardAnalyticsScope {
    Company,
    Region(RegionId),
    Branch(BranchId),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DashboardAnalytics {
    period: AnalyticsPeriod,
    resolved_scope: DashboardAnalyticsScope,
    coverage: RatioEvidence,
    completed_work_orders: AnalyticsMetric<SumEvidence>,
    completion_rate: AnalyticsMetric<RatioEvidence>,
    trend: Vec<TrendSlot<SumEvidence>>,
    observed_at: Timestamp,
}
impl DashboardAnalytics {
    pub fn new(
        period: AnalyticsPeriod,
        resolved_scope: DashboardAnalyticsScope,
        coverage: RatioEvidence,
        completed_work_orders: AnalyticsMetric<SumEvidence>,
        completion_rate: AnalyticsMetric<RatioEvidence>,
        trend: Vec<TrendSlot<SumEvidence>>,
        observed_at: Timestamp,
    ) -> Result<Self, console_kernel_core::KernelError> {
        if observed_at.offset() != time::UtcOffset::UTC {
            return Err(console_kernel_core::KernelError::validation(
                "analytics observed_at must be UTC",
            ));
        }
        Ok(Self {
            period,
            resolved_scope,
            coverage,
            completed_work_orders,
            completion_rate,
            trend,
            observed_at,
        })
    }
    #[must_use]
    pub fn period(&self) -> AnalyticsPeriod {
        self.period
    }
    #[must_use]
    pub fn resolved_scope(&self) -> DashboardAnalyticsScope {
        self.resolved_scope
    }
    #[must_use]
    pub fn coverage(&self) -> RatioEvidence {
        self.coverage
    }
    #[must_use]
    pub fn completed_work_orders(&self) -> &AnalyticsMetric<SumEvidence> {
        &self.completed_work_orders
    }
    #[must_use]
    pub fn completion_rate(&self) -> &AnalyticsMetric<RatioEvidence> {
        &self.completion_rate
    }
    #[must_use]
    pub fn trend(&self) -> &[TrendSlot<SumEvidence>] {
        &self.trend
    }
    #[must_use]
    pub fn observed_at(&self) -> Timestamp {
        self.observed_at
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaborCostAnalytics {
    resolved_scope: LaborCostAnalyticsScope,
    period: AnalyticsPeriod,
    coverage: RatioEvidence,
    worked_duration: AnalyticsMetric<DurationEvidence>,
    readiness_rate: AnalyticsMetric<RatioEvidence>,
    gross_payroll: MetricUnavailable,
    trend: Vec<TrendSlot<DurationEvidence>>,
    observed_at: Timestamp,
}

/// Labor cost is intentionally not branch- or region-addressable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LaborCostAnalyticsScope {
    Company,
}

impl LaborCostAnalytics {
    pub fn new(
        period: AnalyticsPeriod,
        coverage: RatioEvidence,
        worked_duration: AnalyticsMetric<DurationEvidence>,
        readiness_rate: AnalyticsMetric<RatioEvidence>,
        gross_payroll: MetricUnavailable,
        trend: Vec<TrendSlot<DurationEvidence>>,
        observed_at: Timestamp,
    ) -> Result<Self, console_kernel_core::KernelError> {
        AnalyticsPeriod::monthly(period.start(), period.end())?;
        if observed_at.offset() != time::UtcOffset::UTC {
            return Err(console_kernel_core::KernelError::validation(
                "analytics observed_at must be UTC",
            ));
        }
        Ok(Self {
            resolved_scope: LaborCostAnalyticsScope::Company,
            period,
            coverage,
            worked_duration,
            readiness_rate,
            gross_payroll,
            trend,
            observed_at,
        })
    }
    #[must_use]
    pub fn resolved_scope(&self) -> LaborCostAnalyticsScope {
        self.resolved_scope
    }
    #[must_use]
    pub fn period(&self) -> AnalyticsPeriod {
        self.period
    }
    #[must_use]
    pub fn coverage(&self) -> RatioEvidence {
        self.coverage
    }
    #[must_use]
    pub fn worked_duration(&self) -> &AnalyticsMetric<DurationEvidence> {
        &self.worked_duration
    }
    #[must_use]
    pub fn readiness_rate(&self) -> &AnalyticsMetric<RatioEvidence> {
        &self.readiness_rate
    }
    #[must_use]
    pub fn gross_payroll(&self) -> &MetricUnavailable {
        &self.gross_payroll
    }
    #[must_use]
    pub fn trend(&self) -> &[TrendSlot<DurationEvidence>] {
        &self.trend
    }
    #[must_use]
    pub fn observed_at(&self) -> Timestamp {
        self.observed_at
    }
}

impl Period {
    #[must_use]
    pub fn contains(self, value: Timestamp) -> bool {
        value >= self.start && value < self.end
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "id", rename_all = "snake_case")]
pub enum KpiScope {
    Company,
    Region(RegionId),
    Branch(BranchId),
    Technician(UserId),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", content = "id", rename_all = "snake_case")]
pub enum KpiRollupScope {
    Company,
    Region(RegionId),
    Branch(BranchId),
    Technician(UserId),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KpiMetric {
    CompletedCount,
    AverageResponseSpeed,
    CompletionDurationAndDueCompliance,
    RevisitRate,
    DelayRateAndReasonDistribution,
    InspectionPlanCompletionRate,
    P1AcceptanceRate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KpiWorkOrderStatus {
    FinalCompleted,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KpiPriorityLevel {
    P1,
    P2,
    P3,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KpiWorkResultType {
    RevisitRequired,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnavailableMetric {
    pub metric: KpiMetric,
    pub source_domain: String,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KpiReport {
    pub period: Period,
    pub requested_scope: KpiScope,
    pub rollups: Vec<KpiRollup>,
    pub unavailable_metrics: Vec<UnavailableMetric>,
}

impl KpiReport {
    #[must_use]
    pub fn rollup(&self, scope: &KpiRollupScope) -> Option<&KpiRollup> {
        self.rollups.iter().find(|rollup| &rollup.scope == scope)
    }

    #[must_use]
    pub fn unavailable_metric(&self, metric: KpiMetric) -> Option<&UnavailableMetric> {
        self.unavailable_metrics
            .iter()
            .find(|unavailable| unavailable.metric == metric)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KpiRollup {
    pub scope: KpiRollupScope,
    /// Human-readable name for the scope (region/branch/mechanic), resolved by
    /// the adapter via a same-org lookup after the pure aggregation runs. `None`
    /// for the company-wide scope (which has no id) or a since-deleted
    /// region/branch/user; the web renders it through `safeLabel` so a missing
    /// name never leaks the UUID. `#[serde(default)]` keeps the pure domain
    /// calculation (which leaves it `None`) and older payloads valid.
    #[serde(default)]
    pub scope_display_name: Option<String>,
    pub approved_report_count: u32,
    pub completed_count: u32,
    pub weighted_completed_points: u32,
    pub inspection_schedule_due_count: u32,
    pub inspection_schedule_completed_count: u32,
    pub inspection_plan_completion_bps: Option<u32>,
    pub p1_dispatch_count: u32,
    pub p1_accepted_count: u32,
    pub p1_acceptance_bps: Option<u32>,
    pub average_response_seconds: Option<i64>,
    pub average_completion_seconds: Option<i64>,
    pub target_due_compliance_bps: Option<u32>,
    pub revisit_rate_bps: u32,
    pub delay_rate_bps: u32,
    pub delay_reason_distribution: BTreeMap<String, u32>,
    #[serde(default, skip_serializing)]
    pub work_order_ids: Vec<uuid::Uuid>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KpiInputRecord {
    pub work_order_id: uuid::Uuid,
    pub branch_id: BranchId,
    pub region_id: RegionId,
    pub technician_id: Option<UserId>,
    pub status: KpiWorkOrderStatus,
    pub priority: KpiPriorityLevel,
    pub result_type: KpiWorkResultType,
    pub delay_reason: Option<String>,
    pub created_at: Timestamp,
    pub first_in_progress_at: Option<Timestamp>,
    pub approved_at: Timestamp,
    pub target_due_at: Option<Timestamp>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KpiInspectionRecord {
    pub schedule_id: uuid::Uuid,
    pub branch_id: BranchId,
    pub region_id: RegionId,
    pub technician_id: UserId,
    pub completed: bool,
}

/// One P1 emergency dispatch broadcast whose accept window opened within the
/// reporting period. `accepted` is true when a mechanic accepted the broadcast
/// (the dispatch reached `AUTO_ASSIGNED`, or at least one target responded
/// `ACCEPT`) — i.e. the broadcast was answered without a manager force-assign.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KpiP1Record {
    pub dispatch_id: uuid::Uuid,
    pub branch_id: BranchId,
    pub region_id: RegionId,
    pub technician_id: Option<UserId>,
    pub accepted: bool,
}

#[must_use]
pub fn calculate_kpi_report(
    period: Period,
    requested_scope: KpiScope,
    records: &[KpiInputRecord],
    inspection_records: &[KpiInspectionRecord],
    p1_records: &[KpiP1Record],
    unavailable_metrics: Vec<UnavailableMetric>,
) -> KpiReport {
    let mut builders = BTreeMap::<KpiRollupScope, KpiRollupBuilder>::new();
    for record in records {
        add_record(&mut builders, KpiRollupScope::Company, record);
        add_record(
            &mut builders,
            KpiRollupScope::Region(record.region_id),
            record,
        );
        add_record(
            &mut builders,
            KpiRollupScope::Branch(record.branch_id),
            record,
        );
        if let Some(technician_id) = record.technician_id {
            add_record(
                &mut builders,
                KpiRollupScope::Technician(technician_id),
                record,
            );
        }
    }
    for record in inspection_records {
        add_inspection_record(&mut builders, KpiRollupScope::Company, record);
        add_inspection_record(
            &mut builders,
            KpiRollupScope::Region(record.region_id),
            record,
        );
        add_inspection_record(
            &mut builders,
            KpiRollupScope::Branch(record.branch_id),
            record,
        );
        add_inspection_record(
            &mut builders,
            KpiRollupScope::Technician(record.technician_id),
            record,
        );
    }
    for record in p1_records {
        add_p1_record(&mut builders, KpiRollupScope::Company, record);
        add_p1_record(
            &mut builders,
            KpiRollupScope::Region(record.region_id),
            record,
        );
        add_p1_record(
            &mut builders,
            KpiRollupScope::Branch(record.branch_id),
            record,
        );
        if let Some(technician_id) = record.technician_id {
            add_p1_record(
                &mut builders,
                KpiRollupScope::Technician(technician_id),
                record,
            );
        }
    }

    KpiReport {
        period,
        requested_scope,
        rollups: builders
            .into_iter()
            .map(|(scope, builder)| builder.finish(scope))
            .collect(),
        unavailable_metrics,
    }
}

fn add_record(
    builders: &mut BTreeMap<KpiRollupScope, KpiRollupBuilder>,
    scope: KpiRollupScope,
    record: &KpiInputRecord,
) {
    builders.entry(scope).or_default().push(record);
}

fn add_inspection_record(
    builders: &mut BTreeMap<KpiRollupScope, KpiRollupBuilder>,
    scope: KpiRollupScope,
    record: &KpiInspectionRecord,
) {
    builders.entry(scope).or_default().push_inspection(record);
}

fn add_p1_record(
    builders: &mut BTreeMap<KpiRollupScope, KpiRollupBuilder>,
    scope: KpiRollupScope,
    record: &KpiP1Record,
) {
    builders.entry(scope).or_default().push_p1(record);
}

#[derive(Debug, Default)]
struct KpiRollupBuilder {
    approved_report_count: u32,
    completed_count: u32,
    weighted_completed_points: u32,
    response_seconds_total: i128,
    response_count: u32,
    completion_seconds_total: i128,
    completion_count: u32,
    target_due_count: u32,
    target_due_compliant_count: u32,
    inspection_schedule_due_count: u32,
    inspection_schedule_completed_count: u32,
    p1_dispatch_count: u32,
    p1_accepted_count: u32,
    revisit_count: u32,
    delay_count: u32,
    delay_reason_distribution: BTreeMap<String, u32>,
    work_order_ids: Vec<uuid::Uuid>,
}

impl KpiRollupBuilder {
    fn push(&mut self, record: &KpiInputRecord) {
        self.approved_report_count += 1;
        self.work_order_ids.push(record.work_order_id);

        if let Some(first_start) = record.first_in_progress_at
            && first_start >= record.created_at
        {
            self.response_seconds_total +=
                (first_start - record.created_at).whole_seconds() as i128;
            self.response_count += 1;
        }

        if record.result_type == KpiWorkResultType::RevisitRequired {
            self.revisit_count += 1;
        }

        if let Some(reason) = record
            .delay_reason
            .as_ref()
            .filter(|reason| !reason.is_empty())
        {
            self.delay_count += 1;
            *self
                .delay_reason_distribution
                .entry(reason.clone())
                .or_insert(0) += 1;
        }

        if !is_completed(record) {
            return;
        }

        self.completed_count += 1;
        self.weighted_completed_points += priority_weight(record.priority);
        if record.approved_at >= record.created_at {
            self.completion_seconds_total +=
                (record.approved_at - record.created_at).whole_seconds() as i128;
            self.completion_count += 1;
        }
        if let Some(target_due_at) = record.target_due_at {
            self.target_due_count += 1;
            if record.approved_at <= target_due_at {
                self.target_due_compliant_count += 1;
            }
        }
    }

    fn push_inspection(&mut self, record: &KpiInspectionRecord) {
        self.inspection_schedule_due_count += 1;
        if record.completed {
            self.inspection_schedule_completed_count += 1;
        }
    }

    fn push_p1(&mut self, record: &KpiP1Record) {
        self.p1_dispatch_count += 1;
        if record.accepted {
            self.p1_accepted_count += 1;
        }
    }

    fn finish(mut self, scope: KpiRollupScope) -> KpiRollup {
        self.work_order_ids.sort_unstable();
        KpiRollup {
            scope,
            // Names are resolved by the adapter post-pass (DB lookup); the pure
            // domain calculation has no access to region/branch/user names.
            scope_display_name: None,
            approved_report_count: self.approved_report_count,
            completed_count: self.completed_count,
            weighted_completed_points: self.weighted_completed_points,
            inspection_schedule_due_count: self.inspection_schedule_due_count,
            inspection_schedule_completed_count: self.inspection_schedule_completed_count,
            inspection_plan_completion_bps: rate_bps(
                self.inspection_schedule_completed_count,
                self.inspection_schedule_due_count,
            ),
            p1_dispatch_count: self.p1_dispatch_count,
            p1_accepted_count: self.p1_accepted_count,
            p1_acceptance_bps: rate_bps(self.p1_accepted_count, self.p1_dispatch_count),
            average_response_seconds: average_i64(self.response_seconds_total, self.response_count),
            average_completion_seconds: average_i64(
                self.completion_seconds_total,
                self.completion_count,
            ),
            target_due_compliance_bps: rate_bps(
                self.target_due_compliant_count,
                self.target_due_count,
            ),
            revisit_rate_bps: rate_bps(self.revisit_count, self.approved_report_count).unwrap_or(0),
            delay_rate_bps: rate_bps(self.delay_count, self.approved_report_count).unwrap_or(0),
            delay_reason_distribution: self.delay_reason_distribution,
            work_order_ids: self.work_order_ids,
        }
    }
}

fn is_completed(record: &KpiInputRecord) -> bool {
    record.status == KpiWorkOrderStatus::FinalCompleted
}

const fn priority_weight(priority: KpiPriorityLevel) -> u32 {
    match priority {
        KpiPriorityLevel::P1 => 3,
        KpiPriorityLevel::P2 => 2,
        KpiPriorityLevel::P3 => 1,
        KpiPriorityLevel::Other => 0,
    }
}

fn average_i64(total: i128, count: u32) -> Option<i64> {
    if count == 0 {
        return None;
    }
    let count = i128::from(count);
    i64::try_from(total / count).ok()
}

fn rate_bps(numerator: u32, denominator: u32) -> Option<u32> {
    numerator.saturating_mul(10_000).checked_div(denominator)
}

/// Column headers for the KPI Excel export, in render order. Kept next to
/// [`kpi_export_rows`] so the two never drift.
pub const KPI_EXPORT_HEADERS: &[&str] = &[
    "구분",
    "대상",
    "접수 보고 건수",
    "완료 건수",
    "가중 완료 점수",
    "평균 응답(초)",
    "평균 완료(초)",
    "납기 준수율",
    "재방문율",
    "지연율",
    "정기검사 예정",
    "정기검사 완료",
    "정기검사 완료율",
    "P1 출동",
    "P1 수락",
    "P1 수락률",
    "지연 사유",
];

/// Shape a [`KpiReport`] into workbook rows (one per rollup), each aligned 1:1
/// with [`KPI_EXPORT_HEADERS`]. Pure and deterministic: `report.rollups` is
/// already ordered by scope, and this only formats already-computed fields — no
/// I/O and no name resolution (`scope_display_name` is filled by the adapter
/// before this runs).
#[must_use]
pub fn kpi_export_rows(report: &KpiReport) -> Vec<Vec<String>> {
    report.rollups.iter().map(kpi_rollup_row).collect()
}

fn kpi_rollup_row(rollup: &KpiRollup) -> Vec<String> {
    vec![
        kpi_scope_kind_label(&rollup.scope).to_owned(),
        kpi_scope_name(rollup),
        rollup.approved_report_count.to_string(),
        rollup.completed_count.to_string(),
        rollup.weighted_completed_points.to_string(),
        opt_seconds(rollup.average_response_seconds),
        opt_seconds(rollup.average_completion_seconds),
        bps_percent(rollup.target_due_compliance_bps),
        bps_percent(Some(rollup.revisit_rate_bps)),
        bps_percent(Some(rollup.delay_rate_bps)),
        rollup.inspection_schedule_due_count.to_string(),
        rollup.inspection_schedule_completed_count.to_string(),
        bps_percent(rollup.inspection_plan_completion_bps),
        rollup.p1_dispatch_count.to_string(),
        rollup.p1_accepted_count.to_string(),
        bps_percent(rollup.p1_acceptance_bps),
        delay_reasons_text(&rollup.delay_reason_distribution),
    ]
}

fn kpi_scope_kind_label(scope: &KpiRollupScope) -> &'static str {
    match scope {
        KpiRollupScope::Company => "전사",
        KpiRollupScope::Region(_) => "지역",
        KpiRollupScope::Branch(_) => "지점",
        KpiRollupScope::Technician(_) => "담당자",
    }
}

fn kpi_scope_name(rollup: &KpiRollup) -> String {
    match rollup.scope {
        KpiRollupScope::Company => "전체".to_owned(),
        _ => rollup.scope_display_name.clone().unwrap_or_default(),
    }
}

/// Render basis points as a fixed two-decimal percentage (`5_000` -> `50.00%`);
/// `None` renders as an empty cell (metric not applicable / no denominator).
fn bps_percent(bps: Option<u32>) -> String {
    match bps {
        Some(value) => format!("{}.{:02}%", value / 100, value % 100),
        None => String::new(),
    }
}

fn opt_seconds(seconds: Option<i64>) -> String {
    seconds.map(|value| value.to_string()).unwrap_or_default()
}

fn delay_reasons_text(distribution: &BTreeMap<String, u32>) -> String {
    distribution
        .iter()
        .map(|(reason, count)| format!("{reason}: {count}"))
        .collect::<Vec<_>>()
        .join(", ")
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExportSourceNote {
    pub source_domain: String,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DailyStatusReport {
    pub date: time::Date,
    pub results: Vec<DailyStatusRow>,
    pub plans: Vec<DailyStatusRow>,
    pub pending_backlog: Vec<DailyStatusRow>,
    pub periodic_inspections: Vec<PeriodicInspectionRow>,
    pub source_notes: Vec<ExportSourceNote>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DailyStatusRow {
    pub request_date: Option<time::Date>,
    pub site_name: String,
    pub management_no: Option<String>,
    pub model: Option<String>,
    pub vin: Option<String>,
    pub symptom: String,
    pub mechanic_name: Option<String>,
    pub scheduled_date: Option<time::Date>,
    pub completed_date: Option<time::Date>,
    pub result_note: Option<String>,
    pub priority: String,
    pub status: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PeriodicInspectionRow {
    pub site_name: String,
    pub vehicle_no: Option<String>,
    pub management_no: Option<String>,
    pub model: Option<String>,
    pub serial_no: Option<String>,
    pub issue: String,
    pub inspection_period: Option<String>,
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkDiaryBody {
    pub previous_results: String,
    pub today_plans: String,
    pub urgent_actions: Vec<WorkDiaryActionEntry>,
    pub source_notes: Vec<ExportSourceNote>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkDiaryActionEntry {
    pub site_name: String,
    pub management_no: String,
    pub diagnosis: String,
    pub action_taken: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum WorkDiaryStatus {
    Draft,
    Confirmed,
}

impl WorkDiaryStatus {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Draft => "DRAFT",
            Self::Confirmed => "CONFIRMED",
        }
    }

    pub fn from_db_str(value: &str) -> Result<Self, console_kernel_core::KernelError> {
        match value {
            "DRAFT" => Ok(Self::Draft),
            "CONFIRMED" => Ok(Self::Confirmed),
            other => Err(console_kernel_core::KernelError::validation(format!(
                "unknown work diary status {other:?}"
            ))),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkDiaryDraft {
    pub id: uuid::Uuid,
    pub date: time::Date,
    pub status: WorkDiaryStatus,
    pub body: WorkDiaryBody,
    pub confirmed_by: Option<UserId>,
    pub confirmed_at: Option<Timestamp>,
}

// ---------------------------------------------------------------------------
// Operational dashboard (per-tenant ops console)
// ---------------------------------------------------------------------------

/// One stage of the work-order funnel with its current open count.
///
/// `접수`/RECEIVED → `배정`/ASSIGNED → `진행`/IN_PROGRESS → `완료`/COMPLETED.
/// Counts are point-in-time (current `work_orders.status`), org-scoped by RLS.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpsFunnel {
    /// RECEIVED + UNASSIGNED (intake, not yet assigned).
    pub received: u32,
    /// ASSIGNED (a mechanic is on it, work not started).
    pub assigned: u32,
    /// IN_PROGRESS + REPORT_SUBMITTED + ADMIN_REVIEW (active work).
    pub in_progress: u32,
    /// FINAL_COMPLETED (terminal success).
    pub completed: u32,
}

/// Distribution of equipment by lifecycle status (Korean enum values).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpsEquipmentStatus {
    /// 임대 — rented out.
    pub rented: u32,
    /// 예비 — spare / reserve.
    pub spare: u32,
    /// 폐기 — scrapped.
    pub scrapped: u32,
    /// 대체 — replacement unit.
    pub replacement: u32,
    /// 매각 — sold.
    pub sold: u32,
}

/// One mechanic's current active-assignment load (utilization top-N row).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpsMechanicLoad {
    pub mechanic_id: uuid::Uuid,
    pub display_name: String,
    /// Count of assignments on work orders that are not yet terminal.
    pub active_assignments: u32,
}

/// Point-in-time operational rollup for one tenant (org-scoped under RLS).
///
/// All counts are computed from the requesting tenant's data only — every read
/// runs inside `with_org_conn(current_org())`, so a second org's rows are never
/// visible. `aging_hours` is the threshold used for `aging_work_orders`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpsSummary {
    pub funnel: OpsFunnel,
    /// Threshold (hours) past which an unresolved work order counts as aging.
    pub aging_hours: u32,
    /// Open work orders older than `aging_hours` with no terminal status.
    pub aging_work_orders: u32,
    /// P1 dispatches still BROADCASTING whose accept window has expired.
    pub sla_breached: u32,
    /// P1 dispatches still BROADCASTING whose accept window expires soon.
    pub sla_at_risk: u32,
    /// Top-N mechanics by current active-assignment count.
    pub mechanic_load: Vec<OpsMechanicLoad>,
    pub equipment_status: OpsEquipmentStatus,
    /// Equipment substitutions (대차) currently active (not yet returned).
    pub active_substitutions: u32,
    /// Work-order approval steps awaiting a decision (PENDING).
    pub pending_approvals: u32,
    /// Support tickets not yet resolved/closed (OPEN + IN_PROGRESS + ON_HOLD).
    pub open_support_tickets: u32,
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::datetime;

    fn id(value: u128) -> uuid::Uuid {
        uuid::Uuid::from_u128(value)
    }

    fn record(
        work_order_id: uuid::Uuid,
        status: KpiWorkOrderStatus,
        priority: KpiPriorityLevel,
        result_type: KpiWorkResultType,
    ) -> KpiInputRecord {
        KpiInputRecord {
            work_order_id,
            branch_id: BranchId::from_uuid(id(0xb1)),
            region_id: RegionId::from_uuid(id(0xc1)),
            technician_id: Some(UserId::from_uuid(id(0xd1))),
            status,
            priority,
            result_type,
            delay_reason: None,
            created_at: datetime!(2026-07-01 09:00 UTC),
            first_in_progress_at: Some(datetime!(2026-07-01 10:00 UTC)),
            approved_at: datetime!(2026-07-01 12:00 UTC),
            target_due_at: Some(datetime!(2026-07-01 13:00 UTC)),
        }
    }

    #[test]
    fn kpi_rollups_use_typed_work_order_values_without_changing_known_aggregates() {
        let period = Period {
            start: datetime!(2026-07-01 00:00 UTC),
            end: datetime!(2026-07-02 00:00 UTC),
        };
        let records = vec![
            record(
                id(1),
                KpiWorkOrderStatus::FinalCompleted,
                KpiPriorityLevel::P1,
                KpiWorkResultType::Other,
            ),
            record(
                id(2),
                KpiWorkOrderStatus::FinalCompleted,
                KpiPriorityLevel::P2,
                KpiWorkResultType::Other,
            ),
            record(
                id(3),
                KpiWorkOrderStatus::Other,
                KpiPriorityLevel::P3,
                KpiWorkResultType::Other,
            ),
            record(
                id(4),
                KpiWorkOrderStatus::FinalCompleted,
                KpiPriorityLevel::P3,
                KpiWorkResultType::RevisitRequired,
            ),
        ];

        let report =
            calculate_kpi_report(period, KpiScope::Company, &records, &[], &[], Vec::new());
        let company = report.rollup(&KpiRollupScope::Company).unwrap();

        assert_eq!(company.approved_report_count, 4);
        assert_eq!(company.completed_count, 3);
        assert_eq!(company.weighted_completed_points, 6);
        assert_eq!(company.revisit_rate_bps, 2_500);
        assert_eq!(company.target_due_compliance_bps, Some(10_000));
    }

    fn analytics_evidence() -> AnalyticsEvidence {
        AnalyticsEvidence::new(
            "/reporting/facts?predicate=work-orders-july",
            AnalyticsFactQueryIdentity::new("work-orders-july").unwrap(),
            AnalyticsSourceDomain::WorkOrders,
        )
        .unwrap()
    }

    #[test]
    fn analytics_period_is_utc_non_empty_and_half_open() {
        let start = datetime!(2026-07-01 00:00 UTC);
        let end = datetime!(2026-08-01 00:00 UTC);
        let period = AnalyticsPeriod::new(start, end).unwrap();
        assert!(period.contains(start));
        assert!(!period.contains(end));
        assert!(AnalyticsPeriod::new(end, start).is_err());
        assert!(
            AnalyticsPeriod::new(
                datetime!(2026-07-01 00:00 +09:00),
                datetime!(2026-08-01 00:00 +09:00)
            )
            .is_err()
        );
    }

    #[test]
    fn labor_cost_requires_a_month_and_refuses_inferred_payroll() {
        let period = AnalyticsPeriod::monthly(
            datetime!(2026-07-01 00:00 UTC),
            datetime!(2026-08-01 00:00 UTC),
        )
        .unwrap();
        let definition = AnalyticsDefinitionVersion::new("labor-cost.v1").unwrap();
        let duration = AnalyticsMetric::new(
            definition.clone(),
            MetricAvailability::Available(DurationEvidence::new(576_000, 4).unwrap()),
            analytics_evidence(),
        );
        let readiness = AnalyticsMetric::new(
            definition.clone(),
            MetricAvailability::Available(RatioEvidence::new(3, 4).unwrap()),
            analytics_evidence(),
        );
        let payroll = MetricUnavailable::new(
            "payable calculation is not implemented",
            AnalyticsSourceDomain::Payroll,
        )
        .unwrap();
        assert!(
            LaborCostAnalytics::new(
                period,
                RatioEvidence::new(4, 4).unwrap(),
                duration.clone(),
                readiness.clone(),
                payroll.clone(),
                vec![],
                datetime!(2026-08-01 01:00 UTC),
            )
            .is_ok()
        );

        assert_eq!(
            LaborCostAnalytics::new(
                period,
                RatioEvidence::new(4, 4).unwrap(),
                duration,
                readiness,
                payroll,
                vec![],
                datetime!(2026-08-01 01:00 UTC),
            )
            .unwrap()
            .gross_payroll()
            .source_domain(),
            AnalyticsSourceDomain::Payroll
        );
    }

    #[test]
    fn duration_evidence_keeps_seconds_exact_and_rejects_invalid_shapes_or_overflow() {
        let first = DurationEvidence::new(3_599, 1).unwrap();
        let second = DurationEvidence::new(2, 1).unwrap();
        let total = first.checked_add(second).unwrap();
        assert_eq!(total.total_seconds(), 3_601);
        assert_eq!(total.pair_count(), 2);
        assert!(DurationEvidence::new(0, 0).is_err());
        assert!(
            DurationEvidence::new(u64::MAX, 1)
                .unwrap()
                .checked_add(DurationEvidence::new(1, 1).unwrap())
                .is_err()
        );
        assert!(
            DurationEvidence::new(1, u64::MAX)
                .unwrap()
                .checked_add(DurationEvidence::new(1, 1).unwrap())
                .is_err()
        );
    }
}

#[cfg(test)]
mod kpi_export_shaping_tests {
    use super::*;
    use time::OffsetDateTime;

    fn empty_period() -> Period {
        Period {
            start: OffsetDateTime::UNIX_EPOCH,
            end: OffsetDateTime::UNIX_EPOCH,
        }
    }

    fn sample_rollup(scope: KpiRollupScope) -> KpiRollup {
        KpiRollup {
            scope,
            scope_display_name: Some("서울지점".to_owned()),
            approved_report_count: 4,
            completed_count: 3,
            weighted_completed_points: 6,
            inspection_schedule_due_count: 3,
            inspection_schedule_completed_count: 2,
            inspection_plan_completion_bps: Some(6_666),
            p1_dispatch_count: 4,
            p1_accepted_count: 3,
            p1_acceptance_bps: Some(7_500),
            average_response_seconds: Some(7_200),
            average_completion_seconds: None,
            target_due_compliance_bps: Some(5_000),
            revisit_rate_bps: 2_500,
            delay_rate_bps: 0,
            delay_reason_distribution: BTreeMap::from([("MECHANIC_OVERLOADED".to_owned(), 1)]),
            work_order_ids: Vec::new(),
        }
    }

    #[test]
    fn every_row_matches_the_header_width() {
        let report = KpiReport {
            period: empty_period(),
            requested_scope: KpiScope::Company,
            rollups: vec![
                sample_rollup(KpiRollupScope::Company),
                sample_rollup(KpiRollupScope::Branch(BranchId::from_uuid(
                    uuid::Uuid::nil(),
                ))),
            ],
            unavailable_metrics: Vec::new(),
        };
        let rows = kpi_export_rows(&report);
        assert_eq!(rows.len(), 2);
        for row in &rows {
            assert_eq!(row.len(), KPI_EXPORT_HEADERS.len());
        }
    }

    #[test]
    fn formats_scope_percentages_seconds_and_delay_reasons() {
        let report = KpiReport {
            period: empty_period(),
            requested_scope: KpiScope::Company,
            rollups: vec![sample_rollup(KpiRollupScope::Branch(BranchId::from_uuid(
                uuid::Uuid::nil(),
            )))],
            unavailable_metrics: Vec::new(),
        };
        let row = &kpi_export_rows(&report)[0];

        assert_eq!(row[0], "지점");
        assert_eq!(row[1], "서울지점");
        assert_eq!(row[5], "7200"); // average response seconds
        assert_eq!(row[6], ""); // average completion seconds is None
        assert_eq!(row[7], "50.00%"); // target-due compliance (5_000 bps)
        assert_eq!(row[9], "0.00%"); // delay rate (0 bps)
        assert_eq!(row[12], "66.66%"); // inspection completion (6_666 bps)
        assert_eq!(row[15], "75.00%"); // P1 acceptance (7_500 bps)
        assert_eq!(row[16], "MECHANIC_OVERLOADED: 1");
    }

    #[test]
    fn company_scope_has_no_id_but_still_labels_the_row() {
        let mut rollup = sample_rollup(KpiRollupScope::Company);
        rollup.scope_display_name = None;
        let report = KpiReport {
            period: empty_period(),
            requested_scope: KpiScope::Company,
            rollups: vec![rollup],
            unavailable_metrics: Vec::new(),
        };
        let row = &kpi_export_rows(&report)[0];
        assert_eq!(row[0], "전사");
        assert_eq!(row[1], "전체");
    }
}
