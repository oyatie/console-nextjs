//! HTTP-independent contracts for the evaluation console.
//!
//! Field names are the wire contract (snake_case, mirroring the design
//! contract §4 and the openapi fragment); `org_id` is deliberately absent —
//! the adapter derives the tenant from the authenticated request context.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

use console_evaluation_domain::{
    CycleKind, CycleStage, CycleTransition, EvidenceKind, Grade, MetricKind, ReviewKind,
    ReviewStatus, SubjectState,
};
use serde::{Deserialize, Serialize};
use time::{Date, OffsetDateTime};
use uuid::Uuid;

/// `time::Date` wire format (`YYYY-MM-DD`), shared by requests and views.
pub mod date_fmt {
    use serde::{Deserialize, Deserializer, Serializer};
    use time::Date;
    use time::format_description::well_known::Iso8601;

    pub fn serialize<S: Serializer>(date: &Date, ser: S) -> Result<S::Ok, S::Error> {
        let text = date
            .format(&Iso8601::DATE)
            .map_err(serde::ser::Error::custom)?;
        ser.serialize_str(&text)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(de: D) -> Result<Date, D::Error> {
        let text = String::deserialize(de)?;
        Date::parse(&text, &Iso8601::DATE).map_err(serde::de::Error::custom)
    }
}

// ---------------------------------------------------------------------------
// Inputs (validated at the REST boundary before they reach the adapter)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateCycleInput {
    pub name: String,
    pub kind: CycleKind,
    pub period_label: String,
    #[serde(with = "date_fmt")]
    pub due_date: Date,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoalInput {
    pub title: String,
    pub metric_kind: MetricKind,
    pub target_label: String,
    pub weight_pct: i16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvidenceInput {
    pub object_kind: EvidenceKind,
    pub object_ref: String,
    pub label: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewDraftInput {
    pub grade: Option<Grade>,
    pub note: Option<String>,
    pub evidence_links: Vec<EvidenceInput>,
}

/// Normalized cycle-list query (limit already clamped by the REST boundary).
#[derive(Debug, Clone, Copy)]
pub struct CycleQuery {
    pub stage: Option<CycleStage>,
    pub limit: i64,
    pub offset: i64,
}

// ---------------------------------------------------------------------------
// Views
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CycleSummary {
    pub id: Uuid,
    pub name: String,
    pub kind: CycleKind,
    pub period_label: String,
    #[serde(with = "date_fmt")]
    pub due_date: Date,
    pub stage: CycleStage,
    pub subjects_total: i64,
    pub manager_submitted: i64,
    pub self_submitted: i64,
    pub calibrated: i64,
    pub finalized: i64,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CyclePage {
    pub items: Vec<CycleSummary>,
    pub total: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnitProgress {
    pub org_unit: Option<String>,
    pub total: i64,
    pub manager_submitted: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CycleDetail {
    pub id: Uuid,
    pub name: String,
    pub kind: CycleKind,
    pub period_label: String,
    #[serde(with = "date_fmt")]
    pub due_date: Date,
    pub stage: CycleStage,
    pub subjects_total: i64,
    pub manager_submitted: i64,
    pub self_submitted: i64,
    pub calibrated: i64,
    pub finalized: i64,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339::option")]
    pub opened_at: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub calibration_started_at: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub finalized_at: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub archived_at: Option<OffsetDateTime>,
    pub created_by: Uuid,
    pub progress_by_unit: Vec<UnitProgress>,
    pub subjects: Vec<SubjectSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubjectSummary {
    pub id: Uuid,
    pub cycle_id: Uuid,
    pub employee_id: Uuid,
    pub employee_name: String,
    pub org_unit: Option<String>,
    pub manager_user_id: Uuid,
    pub state: SubjectState,
    pub final_grade: Option<Grade>,
    pub rv_code: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubjectDetail {
    pub id: Uuid,
    pub cycle_id: Uuid,
    pub employee_id: Uuid,
    pub employee_name: String,
    pub org_unit: Option<String>,
    pub manager_user_id: Uuid,
    pub state: SubjectState,
    pub final_grade: Option<Grade>,
    pub rv_code: Option<String>,
    pub goals: Vec<GoalView>,
    pub reviews: Vec<ReviewView>,
    pub calibrated_grade: Option<Grade>,
    pub calibration_reason: Option<String>,
    pub calibrated_by: Option<Uuid>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub calibrated_at: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub finalized_at: Option<OffsetDateTime>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoalView {
    pub id: Uuid,
    pub title: String,
    pub metric_kind: MetricKind,
    pub target_label: String,
    pub weight_pct: i16,
    pub sort_order: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewView {
    pub id: Uuid,
    pub subject_id: Uuid,
    pub kind: ReviewKind,
    pub status: ReviewStatus,
    pub evaluator_user_id: Uuid,
    pub grade: Option<Grade>,
    pub note: Option<String>,
    pub evidence_links: Vec<EvidenceLinkView>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub submitted_at: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvidenceLinkView {
    pub id: Uuid,
    pub object_kind: EvidenceKind,
    pub object_ref: String,
    pub label: String,
    pub sort_order: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskItem {
    pub subject_id: Uuid,
    pub cycle_id: Uuid,
    pub cycle_name: String,
    #[serde(with = "date_fmt")]
    pub due_date: Date,
    pub employee_id: Uuid,
    pub employee_name: String,
    pub kind: ReviewKind,
    pub review_status: Option<ReviewStatus>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskPage {
    pub items: Vec<TaskItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LedgerEntry {
    pub rv_code: String,
    pub cycle_id: Uuid,
    pub cycle_name: String,
    pub period_label: String,
    pub final_grade: Grade,
    #[serde(with = "time::serde::rfc3339")]
    pub finalized_at: OffsetDateTime,
    pub subject_id: Uuid,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LedgerPage {
    pub items: Vec<LedgerEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PreflightItem {
    pub code: String,
    pub message: String,
    pub subject_id: Option<Uuid>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PreflightReport {
    pub next_transition: Option<CycleTransition>,
    pub blockers: Vec<PreflightItem>,
    pub advisories: Vec<PreflightItem>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use console_evaluation_domain::{CycleKind, Grade, ReviewKind};
    use time::Month;

    #[test]
    fn wire_shapes_match_the_design_contract() {
        let input: CreateCycleInput = serde_json::from_str(
            r#"{"name":"2026 H2","kind":"REGULAR","period_label":"2026-H2","due_date":"2026-08-31"}"#,
        )
        .expect("contract-shaped request parses");
        assert_eq!(input.kind, CycleKind::Regular);
        assert_eq!(input.due_date.year(), 2026);
        assert_eq!(input.due_date.month(), Month::August);

        let task = TaskItem {
            subject_id: Uuid::nil(),
            cycle_id: Uuid::nil(),
            cycle_name: "cycle".into(),
            due_date: Date::from_calendar_date(2026, Month::August, 31).expect("valid date"),
            employee_id: Uuid::nil(),
            employee_name: "employee".into(),
            kind: ReviewKind::SelfReview,
            review_status: None,
        };
        let json = serde_json::to_value(&task).expect("task serializes");
        assert_eq!(json["due_date"], "2026-08-31");
        assert_eq!(json["kind"], "SELF");

        let grade = serde_json::to_value(Grade::S).expect("grade serializes");
        assert_eq!(grade, "S");
    }
}
