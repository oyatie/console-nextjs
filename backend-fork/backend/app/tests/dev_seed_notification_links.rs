//! Locks the shape of every notification `link` literal in scripts/dev-seed.sql.
//! Each must deserialize as the real [`NotificationLink`] (serde `tag = "type"`).
//! A malformed link (e.g. missing the `type` discriminator) loads fine as jsonb
//! via psql but returns 500 "missing field `type`" from GET /me/notifications at
//! query time — this test fails in the fast backend CI job instead, before the
//! seed can reach the dev-auth e2e that renders those notifications.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use console_notifications_domain::NotificationLink;

const DEV_SEED: &str = include_str!("../../../scripts/dev-seed.sql");

/// Pull each `{"type":...}` object literal out of the seed. NotificationLink
/// values are flat (no nested braces), so `{"type"` .. the next `}` is the whole
/// object. `"type"` is unique to notification links in this seed — workflow
/// graphs/definitions use `{"nodes"`/`{"step"`/`{"channel"`/`{"role"`.
fn notification_link_literals() -> Vec<&'static str> {
    let mut out = Vec::new();
    let mut rest = DEV_SEED;
    while let Some(start) = rest.find("{\"type\"") {
        let tail = &rest[start..];
        let end = tail
            .find('}')
            .expect("unterminated JSON object after `{\"type\"` in dev-seed.sql");
        out.push(&tail[..=end]);
        rest = &tail[end + 1..];
    }
    out
}

#[test]
fn every_seeded_notification_link_deserializes() {
    let links = notification_link_literals();
    assert!(
        links.len() >= 3,
        "expected the seeded notification link literals; found {} — did the \
         notifications block move or drop its `type` discriminator?",
        links.len()
    );
    for literal in links {
        let parsed: NotificationLink = serde_json::from_str(literal).unwrap_or_else(|e| {
            panic!("dev-seed.sql link `{literal}` must deserialize as NotificationLink: {e}")
        });
        parsed.validated().unwrap_or_else(|e| {
            panic!("dev-seed.sql link `{literal}` failed NotificationLink validation: {e}")
        });
    }
}
