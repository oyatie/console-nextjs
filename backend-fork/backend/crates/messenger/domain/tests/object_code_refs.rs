#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! `#`-object-code ref parsing (`extract_object_code_refs`) and the
//! message-ref amplification guards: `MessageBody`'s length cap and the
//! per-message ref count cap.

use console_messenger_domain::{
    MAX_MESSAGE_BODY_CHARS, MAX_OBJECT_CODE_REFS, MessageBody, extract_object_code_refs,
};

#[test]
fn extracts_boundary_preceded_codes_in_order_without_duplicates() {
    let body = "확인 부탁 #WO-20260612-001 그리고 (#AP-3121) 다시 #WO-20260612-001";
    assert_eq!(
        extract_object_code_refs(body),
        vec!["WO-20260612-001".to_owned(), "AP-3121".to_owned()],
    );
}

#[test]
fn drops_hashtag_noise_and_malformed_candidates() {
    // No dash, lowercase prefix, empty body after the dash -> not code-shaped.
    let body = "#hashtag #wo-1 #WO- plain text @11111111-1111-4111-8111-111111111111";
    assert!(extract_object_code_refs(body).is_empty());
}

#[test]
fn caps_refs_per_message_at_max_object_code_refs() {
    // Well over the cap of distinct, well-formed codes in one body.
    let body = (0..MAX_OBJECT_CODE_REFS + 25)
        .map(|i| format!("#CODE-{i}"))
        .collect::<Vec<_>>()
        .join(" ");
    let refs = extract_object_code_refs(&body);
    assert_eq!(
        refs.len(),
        MAX_OBJECT_CODE_REFS,
        "refs must be capped at MAX_OBJECT_CODE_REFS, got {}",
        refs.len()
    );
}

#[test]
fn message_body_rejects_over_max_length() {
    let ok = "a".repeat(MAX_MESSAGE_BODY_CHARS);
    assert!(MessageBody::new(ok).is_ok());

    let too_long = "a".repeat(MAX_MESSAGE_BODY_CHARS + 1);
    assert!(MessageBody::new(too_long).is_err());
}
