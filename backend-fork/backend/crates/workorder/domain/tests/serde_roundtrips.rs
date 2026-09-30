#![allow(clippy::unwrap_used)]

use console_workorder_domain::{
    ApprovalRole, ApprovalStatus, AssignmentRole, AttachmentStage, DelayReason, PriorityLevel,
    TransitionActor, WorkOrderStatus, WorkResultType,
};
use serde::Serialize;
use serde::de::DeserializeOwned;

fn assert_screaming_roundtrip<T>(value: T, expected_json: &str)
where
    T: Serialize + DeserializeOwned + PartialEq + std::fmt::Debug,
{
    let json = serde_json::to_string(&value).unwrap();
    assert_eq!(json, expected_json);
    let back: T = serde_json::from_str(&json).unwrap();
    assert_eq!(back, value);
}

#[test]
fn all_wire_enums_roundtrip_as_screaming_snake_case() {
    assert_screaming_roundtrip(WorkOrderStatus::ReportSubmitted, "\"REPORT_SUBMITTED\"");
    assert_screaming_roundtrip(PriorityLevel::Outsource, "\"OUTSOURCE\"");
    assert_screaming_roundtrip(DelayReason::MechanicOverloaded, "\"MECHANIC_OVERLOADED\"");
    assert_screaming_roundtrip(WorkResultType::TemporaryAction, "\"TEMPORARY_ACTION\"");
    assert_screaming_roundtrip(AttachmentStage::OutsourceResult, "\"OUTSOURCE_RESULT\"");
    assert_screaming_roundtrip(ApprovalRole::Executive, "\"EXECUTIVE\"");
    assert_screaming_roundtrip(ApprovalStatus::NotStarted, "\"NOT_STARTED\"");
    assert_screaming_roundtrip(AssignmentRole::Primary, "\"PRIMARY\"");
    assert_screaming_roundtrip(TransitionActor::Admin, "\"ADMIN\"");
}

#[test]
fn db_string_parsers_accept_prior_project_values_and_reject_unknowns() {
    assert_eq!(
        WorkOrderStatus::from_db_str("EQUIPMENT_IN_USE").unwrap(),
        WorkOrderStatus::EquipmentInUse
    );
    assert_eq!(
        WorkOrderStatus::EquipmentInUse.as_db_str(),
        "EQUIPMENT_IN_USE"
    );
    assert!(WorkOrderStatus::from_db_str("BROADCASTING").is_err());
}
