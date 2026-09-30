//! Org-change lifecycle vocabulary (STORY-ORG-001, HANDOFF §15/§16).
//!
//! Pure state machine + typed proposal ops + wire DTOs for the org-change
//! engine: draft → preflight → ordered SoD approval → effective-dated apply.
//! No IO here; the adapter owns persistence, the rest crate owns HTTP.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

use console_kernel_core::KernelError;
use serde::{Deserialize, Serialize};
use time::{Date, OffsetDateTime};
use uuid::Uuid;

// The workspace `time` build has no `serde-human-readable`, so a bare `Date`
// would hit the wire as `[year, ordinal]`; this pins the ISO calendar date.
time::serde::format_description!(iso_date, Date, "[year]-[month]-[day]");

// ---------------------------------------------------------------------------
// Vocabulary
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum OrgChangeKind {
    New,
    Reorg,
    Dissolve,
}

impl OrgChangeKind {
    #[must_use]
    pub const fn as_db(self) -> &'static str {
        match self {
            Self::New => "NEW",
            Self::Reorg => "REORG",
            Self::Dissolve => "DISSOLVE",
        }
    }

    pub fn from_db(value: &str) -> Result<Self, KernelError> {
        match value {
            "NEW" => Ok(Self::New),
            "REORG" => Ok(Self::Reorg),
            "DISSOLVE" => Ok(Self::Dissolve),
            _ => Err(KernelError::validation("unknown org-change kind")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum OrgChangeStatus {
    Draft,
    Prechecked,
    InApproval,
    Approved,
    Applied,
    Settling,
    Archived,
    Rejected,
    Cancelled,
}

impl OrgChangeStatus {
    #[must_use]
    pub const fn as_db(self) -> &'static str {
        match self {
            Self::Draft => "DRAFT",
            Self::Prechecked => "PRECHECKED",
            Self::InApproval => "IN_APPROVAL",
            Self::Approved => "APPROVED",
            Self::Applied => "APPLIED",
            Self::Settling => "SETTLING",
            Self::Archived => "ARCHIVED",
            Self::Rejected => "REJECTED",
            Self::Cancelled => "CANCELLED",
        }
    }

    pub fn from_db(value: &str) -> Result<Self, KernelError> {
        match value {
            "DRAFT" => Ok(Self::Draft),
            "PRECHECKED" => Ok(Self::Prechecked),
            "IN_APPROVAL" => Ok(Self::InApproval),
            "APPROVED" => Ok(Self::Approved),
            "APPLIED" => Ok(Self::Applied),
            "SETTLING" => Ok(Self::Settling),
            "ARCHIVED" => Ok(Self::Archived),
            "REJECTED" => Ok(Self::Rejected),
            "CANCELLED" => Ok(Self::Cancelled),
            _ => Err(KernelError::conflict("unknown org-change status")),
        }
    }

    /// Terminal statuses accept no further transition (append-only history;
    /// a rejected request is revised as a NEW row via `supersedes_id`).
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Applied | Self::Archived | Self::Rejected | Self::Cancelled
        )
    }

    /// The draft-editable window: draft fields (kind, effective date, reason,
    /// proposal) may only change here, and cancel is only legal here.
    #[must_use]
    pub const fn is_draft_editable(self) -> bool {
        matches!(self, Self::Draft | Self::Prechecked)
    }

    pub fn can_transition_to(self, next: Self) -> Result<(), KernelError> {
        let allowed = matches!(
            (self, next),
            // Preflight persists nothing, so submit drives DRAFT → IN_APPROVAL
            // directly off a receipt recomputed in the submit transaction.
            (Self::Draft, Self::InApproval)
                // DRAFT → PRECHECKED is deliberately absent: nothing may
                // promote a draft on the strength of a preflight run. PRECHECKED
                // is now an inbound-only legacy state — rows already stored in
                // it keep every exit they had.
                | (Self::Prechecked, Self::Draft)
                | (Self::Prechecked, Self::InApproval)
                | (Self::InApproval, Self::Approved | Self::Rejected)
                // NEW/REORG apply is terminal; DISSOLVE opens settlement.
                | (Self::Approved, Self::Applied | Self::Settling)
                | (Self::Settling, Self::Archived)
                | (Self::Draft | Self::Prechecked, Self::Cancelled)
        );
        if allowed {
            Ok(())
        } else {
            Err(KernelError::conflict(
                "illegal org-change status transition",
            ))
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum TargetKind {
    Entity,
    Region,
    Branch,
    Site,
    OrgUnit,
}

impl TargetKind {
    #[must_use]
    pub const fn as_db(self) -> &'static str {
        match self {
            Self::Entity => "ENTITY",
            Self::Region => "REGION",
            Self::Branch => "BRANCH",
            Self::Site => "SITE",
            Self::OrgUnit => "ORG_UNIT",
        }
    }

    pub fn from_db(value: &str) -> Result<Self, KernelError> {
        match value {
            "ENTITY" => Ok(Self::Entity),
            "REGION" => Ok(Self::Region),
            "BRANCH" => Ok(Self::Branch),
            "SITE" => Ok(Self::Site),
            "ORG_UNIT" => Ok(Self::OrgUnit),
            _ => Err(KernelError::validation("unknown org-change target kind")),
        }
    }
}

/// Ordered SoD chain roles (HR → 재무 → 법무 → 임원), fixed in slice 1; the
/// step table's `role_key TEXT + step_order` shape already accommodates a
/// configurable approval matrix later without a migration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalRoleKey {
    Hr,
    Finance,
    Legal,
    Executive,
}

impl ApprovalRoleKey {
    pub const ORDER: [Self; 4] = [Self::Hr, Self::Finance, Self::Legal, Self::Executive];

    #[must_use]
    pub const fn as_db(self) -> &'static str {
        match self {
            Self::Hr => "hr",
            Self::Finance => "finance",
            Self::Legal => "legal",
            Self::Executive => "executive",
        }
    }

    pub fn from_db(value: &str) -> Result<Self, KernelError> {
        match value {
            "hr" => Ok(Self::Hr),
            "finance" => Ok(Self::Finance),
            "legal" => Ok(Self::Legal),
            "executive" => Ok(Self::Executive),
            _ => Err(KernelError::conflict("unknown approval role key")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum StepDecision {
    Pending,
    Approved,
    Rejected,
}

impl StepDecision {
    #[must_use]
    pub const fn as_db(self) -> &'static str {
        match self {
            Self::Pending => "PENDING",
            Self::Approved => "APPROVED",
            Self::Rejected => "REJECTED",
        }
    }

    pub fn from_db(value: &str) -> Result<Self, KernelError> {
        match value {
            "PENDING" => Ok(Self::Pending),
            "APPROVED" => Ok(Self::Approved),
            "REJECTED" => Ok(Self::Rejected),
            _ => Err(KernelError::conflict("unknown step decision")),
        }
    }
}

/// The six dissolve settlement items (§3.9.3), seeded at effectuate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum SettlementKey {
    TransferEmployees,
    Positions,
    CostCenters,
    CloseOpenDocs,
    Assets,
    PayrollSocialFinal,
}

impl SettlementKey {
    pub const ALL: [Self; 6] = [
        Self::TransferEmployees,
        Self::Positions,
        Self::CostCenters,
        Self::CloseOpenDocs,
        Self::Assets,
        Self::PayrollSocialFinal,
    ];

    #[must_use]
    pub const fn as_db(self) -> &'static str {
        match self {
            Self::TransferEmployees => "TRANSFER_EMPLOYEES",
            Self::Positions => "POSITIONS",
            Self::CostCenters => "COST_CENTERS",
            Self::CloseOpenDocs => "CLOSE_OPEN_DOCS",
            Self::Assets => "ASSETS",
            Self::PayrollSocialFinal => "PAYROLL_SOCIAL_FINAL",
        }
    }

    /// Server-issued display label (design §3.9.3 settlement list).
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::TransferEmployees => "소속 직원 전보·전적(동의·통지 기간)",
            Self::Positions => "포지션 이관·폐지",
            Self::CostCenters => "코스트센터·예산 재배정",
            Self::CloseOpenDocs => "진행 중 공고·결재 종결",
            Self::Assets => "자산 이관·반납",
            Self::PayrollSocialFinal => "급여·4대보험·퇴직 정산",
        }
    }
}

// ---------------------------------------------------------------------------
// Proposal ops — the typed sandbox diff, replayed in order at apply time.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReassignScope {
    pub company: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "SCREAMING_SNAKE_CASE", deny_unknown_fields)]
pub enum OrgProposalOp {
    #[serde(rename_all = "camelCase")]
    CreateRegion { name: String },
    #[serde(rename_all = "camelCase")]
    RenameRegion { region_id: Uuid, name: String },
    #[serde(rename_all = "camelCase")]
    DeactivateRegion { region_id: Uuid },
    #[serde(rename_all = "camelCase")]
    CreateBranch { region_id: Uuid, name: String },
    /// Rename and/or move a branch across regions.
    #[serde(rename_all = "camelCase")]
    RenameBranch {
        branch_id: Uuid,
        name: Option<String>,
        region_id: Option<Uuid>,
    },
    #[serde(rename_all = "camelCase")]
    DeactivateBranch { branch_id: Uuid },
    /// Slice 1 covers the org-tree edit semantics (사업장 추가 / rename);
    /// extended site fields keep routing through the registry REST surface.
    #[serde(rename_all = "camelCase")]
    CreateSite { customer_id: Uuid, name: String },
    #[serde(rename_all = "camelCase")]
    UpdateSite { site_id: Uuid, name: String },
    /// OrgUnit move: emits canonical `hr.transfer` per matched employee.
    /// `from_org_unit` / `to_org_unit` are OrgUnit UUID strings (not free-text
    /// team labels). Legacy `employees.org_unit` stores the same UUID text.
    #[serde(rename_all = "camelCase")]
    ReassignOrgUnit {
        from_org_unit: String,
        to_org_unit: String,
        scope: ReassignScope,
    },
}

const MAX_PROPOSAL_OPS: usize = 100;

fn bounded(value: &str, name: &str, max: usize) -> Result<(), KernelError> {
    if value.trim().is_empty() || value.chars().count() > max {
        return Err(KernelError::validation(format!(
            "{name} is required and must be at most {max} characters"
        )));
    }
    Ok(())
}

impl OrgProposalOp {
    pub fn validate(&self) -> Result<(), KernelError> {
        match self {
            Self::CreateRegion { name }
            | Self::RenameRegion { name, .. }
            | Self::CreateBranch { name, .. }
            | Self::CreateSite { name, .. }
            | Self::UpdateSite { name, .. } => bounded(name, "name", 120),
            Self::RenameBranch {
                name, region_id, ..
            } => {
                if name.is_none() && region_id.is_none() {
                    return Err(KernelError::validation(
                        "RENAME_BRANCH requires a name and/or a target region",
                    ));
                }
                name.as_deref()
                    .map(|n| bounded(n, "name", 120))
                    .transpose()?;
                Ok(())
            }
            Self::DeactivateRegion { .. } | Self::DeactivateBranch { .. } => Ok(()),
            Self::ReassignOrgUnit {
                from_org_unit,
                to_org_unit,
                scope,
            } => {
                bounded(from_org_unit, "fromOrgUnit", 120)?;
                bounded(to_org_unit, "toOrgUnit", 120)?;
                bounded(&scope.company, "scope.company", 120)?;
                if Uuid::parse_str(from_org_unit.trim()).is_err()
                    || Uuid::parse_str(to_org_unit.trim()).is_err()
                {
                    return Err(KernelError::validation(
                        "REASSIGN_ORG_UNIT fromOrgUnit/toOrgUnit must be OrgUnit UUIDs",
                    ));
                }
                if from_org_unit == to_org_unit {
                    return Err(KernelError::validation(
                        "REASSIGN_ORG_UNIT source and target must differ",
                    ));
                }
                Ok(())
            }
        }
    }
}

pub fn validate_proposal(ops: &[OrgProposalOp]) -> Result<(), KernelError> {
    if ops.len() > MAX_PROPOSAL_OPS {
        return Err(KernelError::validation(format!(
            "proposal may contain at most {MAX_PROPOSAL_OPS} ops"
        )));
    }
    for op in ops {
        op.validate()?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Wire DTOs (camelCase JSON, shared by adapter and rest)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OrgChangeTarget {
    pub kind: TargetKind,
    #[serde(rename = "ref")]
    pub target_ref: String,
    pub label: String,
}

impl OrgChangeTarget {
    pub fn validate(&self) -> Result<(), KernelError> {
        bounded(&self.target_ref, "target.ref", 200)?;
        bounded(&self.label, "target.label", 200)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreflightBlocker {
    pub code: String,
    pub label: String,
    pub dependent_kind: String,
    pub count: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreflightWarning {
    pub code: String,
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dependent_kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub count: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreflightReport {
    #[serde(with = "time::serde::rfc3339")]
    pub computed_at: OffsetDateTime,
    /// True when the draft was edited after this receipt was computed.
    pub stale: bool,
    pub blockers: Vec<PreflightBlocker>,
    pub warnings: Vec<PreflightWarning>,
    pub headcount: i64,
    pub dependents_total: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OrgChangeSummary {
    pub id: Uuid,
    /// Server-issued display code `OC-YYYY-NNNN`.
    pub code: String,
    pub kind: OrgChangeKind,
    pub status: OrgChangeStatus,
    pub target: OrgChangeTarget,
    #[serde(with = "iso_date")]
    pub effective_date: Date,
    pub reason: String,
    pub headcount: i64,
    pub site_count: i64,
    pub team_count: i64,
    pub drafted_by: Uuid,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub supersedes_id: Option<Uuid>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalStepView {
    pub id: Uuid,
    pub step_order: i16,
    pub role_key: ApprovalRoleKey,
    pub decision: StepDecision,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub decided_by: Option<Uuid>,
    #[serde(
        with = "time::serde::rfc3339::option",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub decided_at: Option<OffsetDateTime>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memo: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SettlementItemView {
    pub id: Uuid,
    pub item_key: SettlementKey,
    pub label: String,
    pub done: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub done_by: Option<Uuid>,
    #[serde(
        with = "time::serde::rfc3339::option",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub done_at: Option<OffsetDateTime>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memo: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OrgChangeEventView {
    #[serde(with = "time::serde::rfc3339")]
    pub at: OffsetDateTime,
    pub actor: Uuid,
    pub action: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from_status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to_status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OrgChangeDetail {
    #[serde(flatten)]
    pub summary: OrgChangeSummary,
    pub proposal: Vec<OrgProposalOp>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preflight: Option<PreflightReport>,
    pub approval_steps: Vec<ApprovalStepView>,
    pub settlement_items: Vec<SettlementItemView>,
    pub events: Vec<OrgChangeEventView>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OrgChangePage {
    pub items: Vec<OrgChangeSummary>,
    pub total: i64,
}

/// How a legacy org-structure row resolves onto a canonical Company/OrgUnit id.
///
/// `Ambiguous` is reserved for name/text resolution attempts: it MUST NOT be
/// treated as authority to mint a binding (L5-ORG / handoff P5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum CanonicalResolutionStatus {
    Resolved,
    Unbound,
    Ambiguous,
}

/// One legacy region/branch (or already-bound source) projected onto OrgUnit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OrgUnitReference {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub org_unit_id: Option<Uuid>,
    pub source_kind: String,
    pub source_id: String,
    pub resolution_status: CanonicalResolutionStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OrgEntitySummary {
    pub org_id: Uuid,
    pub slug: String,
    pub name: String,
    pub status: String,
    /// Tenant identity IS the Company (`organizations.id`); never a parallel
    /// `/companies` id space.
    pub company_id: Uuid,
    pub company_resolution_status: CanonicalResolutionStatus,
    /// Region/branch → OrgUnit projections for this tenant (unbound rows stay
    /// visible with `orgUnitId: null` so clients can see gaps deny-by-field).
    pub org_units: Vec<OrgUnitReference>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allows_each_documented_transition() {
        use OrgChangeStatus as S;
        for (from, to) in [
            // The shipped submit path: preflight persists nothing.
            (S::Draft, S::InApproval),
            (S::Prechecked, S::Draft),
            (S::Prechecked, S::InApproval),
            (S::InApproval, S::Approved),
            (S::InApproval, S::Rejected),
            (S::Approved, S::Applied),
            (S::Approved, S::Settling),
            (S::Settling, S::Archived),
            (S::Draft, S::Cancelled),
            (S::Prechecked, S::Cancelled),
        ] {
            from.can_transition_to(to).expect("documented transition");
        }
    }

    #[test]
    fn rejects_illegal_terminal_and_skip_transitions() {
        use OrgChangeStatus as S;
        for (from, to) in [
            // Preflight persists nothing: no caller may promote a DRAFT to
            // PRECHECKED. Re-introducing the write fails here, with no database.
            (S::Draft, S::Prechecked),
            (S::Draft, S::Approved),
            (S::InApproval, S::Cancelled),
            (S::Approved, S::Archived),
            (S::Applied, S::Draft),
            (S::Archived, S::Settling),
            (S::Rejected, S::InApproval),
            (S::Cancelled, S::Draft),
            (S::Settling, S::Applied),
        ] {
            assert!(from.can_transition_to(to).is_err(), "{from:?} -> {to:?}");
        }
    }

    #[test]
    fn status_round_trips_through_db_vocabulary() {
        use OrgChangeStatus as S;
        for status in [
            S::Draft,
            S::Prechecked,
            S::InApproval,
            S::Approved,
            S::Applied,
            S::Settling,
            S::Archived,
            S::Rejected,
            S::Cancelled,
        ] {
            assert_eq!(S::from_db(status.as_db()).unwrap(), status);
        }
        assert!(S::from_db("BOGUS").is_err());
    }

    #[test]
    fn terminal_statuses_are_exactly_the_four_documented_ones() {
        use OrgChangeStatus as S;
        let terminal: Vec<S> = [
            S::Draft,
            S::Prechecked,
            S::InApproval,
            S::Approved,
            S::Applied,
            S::Settling,
            S::Archived,
            S::Rejected,
            S::Cancelled,
        ]
        .into_iter()
        .filter(|s| s.is_terminal())
        .collect();
        assert_eq!(
            terminal,
            vec![S::Applied, S::Archived, S::Rejected, S::Cancelled]
        );
    }

    #[test]
    fn proposal_ops_round_trip_their_wire_tags() {
        let ops = vec![
            OrgProposalOp::CreateRegion {
                name: "수도권".into(),
            },
            OrgProposalOp::ReassignOrgUnit {
                from_org_unit: "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee".into(),
                to_org_unit: "ffffffff-0000-1111-2222-333333333333".into(),
                scope: ReassignScope {
                    company: "KNL".into(),
                },
            },
        ];
        let json = serde_json::to_value(&ops).unwrap();
        assert_eq!(json[0]["op"], "CREATE_REGION");
        assert_eq!(json[1]["op"], "REASSIGN_ORG_UNIT");
        assert_eq!(
            json[1]["fromOrgUnit"],
            "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"
        );
        let back: Vec<OrgProposalOp> = serde_json::from_value(json).unwrap();
        assert_eq!(back, ops);
    }

    #[test]
    fn proposal_validation_fails_closed_on_unbounded_or_no_op_input() {
        assert!(
            OrgProposalOp::CreateRegion { name: "  ".into() }
                .validate()
                .is_err()
        );
        assert!(
            OrgProposalOp::RenameBranch {
                branch_id: Uuid::new_v4(),
                name: None,
                region_id: None,
            }
            .validate()
            .is_err()
        );
        assert!(
            OrgProposalOp::ReassignOrgUnit {
                from_org_unit: "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee".into(),
                to_org_unit: "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee".into(),
                scope: ReassignScope {
                    company: "KNL".into()
                },
            }
            .validate()
            .is_err()
        );
        assert!(
            OrgProposalOp::ReassignOrgUnit {
                from_org_unit: "정비1팀".into(),
                to_org_unit: "정비2팀".into(),
                scope: ReassignScope {
                    company: "KNL".into()
                },
            }
            .validate()
            .is_err(),
            "free-text team labels must fail closed now that OrgUnit UUIDs are required"
        );
        let too_many = vec![
            OrgProposalOp::CreateRegion {
                name: "지역".into()
            };
            101
        ];
        assert!(validate_proposal(&too_many).is_err());
    }

    #[test]
    fn summary_serializes_effective_date_as_iso_calendar_date() {
        let summary = OrgChangeSummary {
            id: Uuid::nil(),
            code: "OC-2026-0001".into(),
            kind: OrgChangeKind::Reorg,
            status: OrgChangeStatus::Draft,
            target: OrgChangeTarget {
                kind: TargetKind::Region,
                target_ref: Uuid::nil().to_string(),
                label: "수도권".into(),
            },
            effective_date: Date::from_calendar_date(2026, time::Month::July, 24).unwrap(),
            reason: "개편".into(),
            headcount: 0,
            site_count: 0,
            team_count: 0,
            drafted_by: Uuid::nil(),
            created_at: OffsetDateTime::UNIX_EPOCH,
            updated_at: OffsetDateTime::UNIX_EPOCH,
            supersedes_id: None,
        };
        let json = serde_json::to_value(&summary).unwrap();
        // Without this pin the workspace `time` build (no serde-human-readable)
        // would emit `[2026, 205]` and break every generated client.
        assert_eq!(json["effectiveDate"], "2026-07-24");
    }

    #[test]
    fn approval_chain_order_and_settlement_catalog_are_fixed() {
        assert_eq!(
            ApprovalRoleKey::ORDER.map(ApprovalRoleKey::as_db),
            ["hr", "finance", "legal", "executive"]
        );
        assert_eq!(SettlementKey::ALL.len(), 6);
        for key in SettlementKey::ALL {
            assert!(!key.label().is_empty());
            assert_eq!(
                serde_json::to_value(key).unwrap(),
                serde_json::Value::String(key.as_db().to_owned())
            );
        }
    }
}
