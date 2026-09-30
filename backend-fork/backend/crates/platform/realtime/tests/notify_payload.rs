#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use console_kernel_core::{BranchId, MessageId, OrgId, ThreadId};
use console_messenger_application::MessagePostedNotification;
use console_platform_realtime::{
    MessageNotifyPayload, NOTIFY_PAYLOAD_LIMIT_BYTES, NotifyPayloadError, PostgresMessageNotifier,
};
use serde_json::Value;

#[test]
fn message_notify_payload_serializes_ids_only_under_postgres_ceiling() {
    let notification = MessagePostedNotification {
        message_id: MessageId::new(),
        thread_id: ThreadId::new(),
        branch_id: BranchId::new(),
    };
    let org_id = OrgId::knl();

    let bytes = MessageNotifyPayload::from_notification(notification, org_id)
        .to_json_bytes()
        .unwrap();
    assert!(
        bytes.len() < NOTIFY_PAYLOAD_LIMIT_BYTES,
        "ID-only NOTIFY payload must stay under PostgreSQL's 8000-byte ceiling; got {} bytes",
        bytes.len()
    );

    let json: Value = serde_json::from_slice(&bytes).unwrap();
    let object = json.as_object().unwrap();
    assert_eq!(
        object.len(),
        3,
        "NOTIFY payload carries IDs only (message_id, thread_id, org_id)"
    );
    assert_eq!(
        object.get("message_id").and_then(Value::as_str),
        Some(notification.message_id.to_string().as_str())
    );
    assert_eq!(
        object.get("thread_id").and_then(Value::as_str),
        Some(notification.thread_id.to_string().as_str())
    );
    assert_eq!(
        object.get("org_id").and_then(Value::as_str),
        Some(org_id.to_string().as_str()),
        "org_id arms app.current_org in the background listener, which has no request context"
    );
    assert!(
        object.get("branch_id").is_none(),
        "branch_id is re-read from Postgres, not trusted from NOTIFY"
    );
    assert!(
        object.get("body").is_none(),
        "message bodies must never ride transient NOTIFY payloads"
    );

    let decoded = MessageNotifyPayload::from_json_bytes(&bytes).unwrap();
    assert_eq!(decoded.message_id, notification.message_id);
    assert_eq!(decoded.thread_id, notification.thread_id);
    assert_eq!(decoded.org_id, org_id);
}

#[test]
fn notifier_rejects_payloads_at_send_time_when_they_reach_the_postgres_ceiling() {
    let max_valid_payload = vec![b'x'; NOTIFY_PAYLOAD_LIMIT_BYTES - 1];
    PostgresMessageNotifier::validate_payload_size_for_test(&max_valid_payload).unwrap();

    let too_large_payload = vec![b'x'; NOTIFY_PAYLOAD_LIMIT_BYTES];
    let err = PostgresMessageNotifier::validate_payload_size_for_test(&too_large_payload)
        .expect_err("payloads at the 8000-byte boundary must be rejected before pg_notify");

    assert!(matches!(
        err,
        NotifyPayloadError::PayloadTooLarge { size, limit }
            if size == NOTIFY_PAYLOAD_LIMIT_BYTES && limit == NOTIFY_PAYLOAD_LIMIT_BYTES
    ));
}
