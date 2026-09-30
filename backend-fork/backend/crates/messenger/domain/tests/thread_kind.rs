#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use console_messenger_domain::{MessageBody, ThreadKind};

#[test]
fn thread_kind_db_wire_uses_plan_values() {
    assert_eq!(ThreadKind::WorkOrder.as_db_str(), "work_order");
    assert_eq!(ThreadKind::Team.as_db_str(), "team");
    assert_eq!(ThreadKind::Dm.as_db_str(), "dm");
    assert_eq!(ThreadKind::Group.as_db_str(), "group");
    assert_eq!(
        ThreadKind::from_db_str("work_order").unwrap(),
        ThreadKind::WorkOrder
    );
    assert!(ThreadKind::from_db_str("WORK_ORDER").is_err());
}

#[test]
fn message_body_trims_and_rejects_blank() {
    assert_eq!(
        MessageBody::new("  누유 확인  ").unwrap().as_str(),
        "누유 확인"
    );
    assert!(MessageBody::new(" \t\n ").is_err());
}
