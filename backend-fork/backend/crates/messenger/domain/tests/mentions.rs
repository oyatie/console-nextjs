#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Mention extraction (DESIGN §4.7-7): `@<uuid>` = mention, everything else
//! (`#object-link`, `!code`, email local-parts, bare text) carries no mention.

use console_messenger_domain::extract_mention_user_ids;

const ALICE: &str = "11111111-1111-4111-8111-111111111111";
const BOB: &str = "22222222-2222-4222-8222-222222222222";

#[test]
fn extracts_boundary_preceded_uuid_mentions_in_order_without_duplicates() {
    let body = format!("확인 부탁 @{ALICE} 그리고 (@{BOB}) 다시 @{ALICE}");
    let ids = extract_mention_user_ids(&body);
    assert_eq!(
        ids.iter().map(ToString::to_string).collect::<Vec<_>>(),
        vec![ALICE.to_owned(), BOB.to_owned()],
    );
}

#[test]
fn ignores_object_links_code_links_and_plain_text() {
    // `#`/`!` are not mentions; an email local-part `@` is not boundary-preceded.
    let body = format!("#WO-20260612-001 참고 !AP-3121 처리 user@example.com {ALICE}");
    assert!(extract_mention_user_ids(&body).is_empty());
}

#[test]
fn ignores_at_that_is_not_boundary_preceded_or_not_a_uuid() {
    assert!(extract_mention_user_ids(&format!("메일a@{ALICE}")).is_empty());
    assert!(extract_mention_user_ids("@홍길동 안녕").is_empty());
    assert!(extract_mention_user_ids("@@ @! @123").is_empty());
}
