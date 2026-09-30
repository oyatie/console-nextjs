//! Messenger domain.
//!
//! Pure value objects and enum wire contracts only. Persistence, audit, REST,
//! and realtime delivery live in outer layers.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

use console_kernel_core::{KernelError, UserId};
use serde::{Deserialize, Serialize};

/// Extract the `UserId`s mentioned in a message body, in first-seen order,
/// deduplicated. Mirrors the web token grammar (DESIGN §4.7-7): a mention is an
/// `@` that is boundary-preceded (start-of-string / whitespace / `([{`)
/// followed by the confirmed candidate's code, which for a person is the raw
/// user UUID (`web/src/lib/objectCandidates.ts` returns `code = member.id`, and
/// `confirmToken` inserts `@<code>`). Only `@` is a mention — `#object-link`
/// and `!code-link` carry no notification (DESIGN §4.7-7: `#` = 알림 없음).
///
/// This is intentionally the *parse* step only: it does not verify the user
/// exists or is reachable. The caller filters the result down to real,
/// permitted recipients (thread members) so an `@<uuid>` for someone outside
/// the thread resolves to nothing — deny-by-omission, never a link/notify.
#[must_use]
pub fn extract_mention_user_ids(body: &str) -> Vec<UserId> {
    let mut out: Vec<UserId> = Vec::new();
    let mut prev: Option<char> = None;
    for (idx, ch) in body.char_indices() {
        if ch == '@' && prev.is_none_or(is_mention_boundary) {
            let rest = &body[idx + 1..];
            let end = rest
                .find(|c: char| !(c.is_ascii_hexdigit() || c == '-'))
                .unwrap_or(rest.len());
            if let Ok(id) = rest[..end].parse::<UserId>()
                && !out.contains(&id)
            {
                out.push(id);
            }
        }
        prev = Some(ch);
    }
    out
}

fn is_mention_boundary(c: char) -> bool {
    c.is_whitespace() || matches!(c, '(' | '[' | '{')
}

/// Extract the `#`-object-code tokens in a message body, in first-seen order,
/// deduplicated. Mirrors [`extract_mention_user_ids`]'s boundary rule and the
/// web token grammar (DESIGN §4.7-7): a `#` that is boundary-preceded followed
/// by a candidate code — an uppercase prefix (`WO`, `AP`, …), a `-`, then the
/// code body (`[A-Za-z0-9-]`, e.g. a `YYYYMMDD-NNN` request no or a bare
/// sequence). Only the shape is checked here; the caller validates the prefix
/// against the seeded `object_types.code_prefix` set (so `#hashtag` noise is
/// dropped) and resolves the target under policy at read time.
///
/// Unlike `@`-mentions, `#`-refs carry no notification (DESIGN §4.7-7: `#` =
/// 알림 없음) — this is purely the persisted-reference parse.
///
/// Capped at [`MAX_OBJECT_CODE_REFS`] distinct codes: with `MessageBody`
/// already bounded to [`MAX_MESSAGE_BODY_CHARS`], a body still fits many more
/// than that many `#code` tokens (e.g. `"#A-A "` repeated), and each ref
/// costs a lookup + row write downstream — the cap is a message-ref
/// amplification guard, not an expected ceiling.
#[must_use]
pub fn extract_object_code_refs(body: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut prev: Option<char> = None;
    for (idx, ch) in body.char_indices() {
        if out.len() >= MAX_OBJECT_CODE_REFS {
            break;
        }
        if ch == '#' && prev.is_none_or(is_mention_boundary) {
            let rest = &body[idx + 1..];
            let end = rest
                .find(|c: char| !(c.is_ascii_alphanumeric() || c == '-'))
                .unwrap_or(rest.len());
            let candidate = &rest[..end];
            if is_code_shaped(candidate) && !out.iter().any(|c| c == candidate) {
                out.push(candidate.to_owned());
            }
        }
        prev = Some(ch);
    }
    out
}

/// Cap on distinct `#`-object-code refs parsed from one message body (message-
/// ref amplification guard — see [`extract_object_code_refs`]).
pub const MAX_OBJECT_CODE_REFS: usize = 50;

/// A code is `<UPPER prefix>-<body>`: 1+ leading uppercase ASCII letters, a
/// single `-`, then at least one more char. Rejects `hashtag`, `-x`, `WO-`.
fn is_code_shaped(candidate: &str) -> bool {
    let Some(dash) = candidate.find('-') else {
        return false;
    };
    let (prefix, rest) = (&candidate[..dash], &candidate[dash + 1..]);
    !prefix.is_empty() && prefix.chars().all(|c| c.is_ascii_uppercase()) && !rest.is_empty()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThreadKind {
    WorkOrder,
    Team,
    Dm,
    Group,
}

/// How a thread is offered in the sidebar (Slack/Teams taxonomy):
/// `Channel` = a named, branch-scoped room any active branch member may join;
/// `Direct` = a fixed member set (DMs, work-order auto-threads, ad-hoc groups).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThreadVisibility {
    Channel,
    Direct,
}

impl ThreadVisibility {
    #[must_use]
    pub const fn as_db_str(self) -> &'static str {
        match self {
            Self::Channel => "channel",
            Self::Direct => "direct",
        }
    }

    pub fn from_db_str(value: &str) -> Result<Self, KernelError> {
        match value {
            "channel" => Ok(Self::Channel),
            "direct" => Ok(Self::Direct),
            other => Err(KernelError::validation(format!(
                "unknown messenger thread visibility {other:?}"
            ))),
        }
    }

    /// The visibility a thread of `kind` defaults to when the caller does not
    /// specify one: a named team thread is a channel; everything else is direct.
    #[must_use]
    pub const fn default_for(kind: ThreadKind, has_title: bool) -> Self {
        match kind {
            ThreadKind::Team if has_title => Self::Channel,
            _ => Self::Direct,
        }
    }
}

/// Activity-derived presence, honest about staleness: it reflects the age of a
/// user's last real action (message/read/ack), never a live socket.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PresenceStatus {
    Online,
    Away,
    Offline,
}

/// A user counts as online for 5 minutes after their last action, away for the
/// next 25, and offline (or never-seen) after that. Tuned as a coarse presence
/// dot, not an SLA — the thresholds live here so the boundary is one testable
/// place rather than scattered SQL.
pub const PRESENCE_ONLINE_SECONDS: i64 = 5 * 60;
pub const PRESENCE_AWAY_SECONDS: i64 = 30 * 60;

/// Derive presence from the seconds elapsed since a user's last activity.
/// `None` (never active) is [`PresenceStatus::Offline`]. A negative age (clock
/// skew, activity stamped slightly in the future) is treated as online.
#[must_use]
pub fn presence_status_for_age(age_seconds: Option<i64>) -> PresenceStatus {
    match age_seconds {
        None => PresenceStatus::Offline,
        Some(age) if age < PRESENCE_ONLINE_SECONDS => PresenceStatus::Online,
        Some(age) if age < PRESENCE_AWAY_SECONDS => PresenceStatus::Away,
        Some(_) => PresenceStatus::Offline,
    }
}

impl ThreadKind {
    #[must_use]
    pub const fn as_db_str(self) -> &'static str {
        match self {
            Self::WorkOrder => "work_order",
            Self::Team => "team",
            Self::Dm => "dm",
            Self::Group => "group",
        }
    }

    pub fn from_db_str(value: &str) -> Result<Self, KernelError> {
        match value {
            "work_order" => Ok(Self::WorkOrder),
            "team" => Ok(Self::Team),
            "dm" => Ok(Self::Dm),
            "group" => Ok(Self::Group),
            other => Err(KernelError::validation(format!(
                "unknown messenger thread kind {other:?}"
            ))),
        }
    }
}

/// Max message body length, matching the workflow-studio decision/return
/// comment cap (`app/src/workflow_studio.rs`) already established elsewhere in
/// this codebase for free-text fields. Also bounds parse cost downstream
/// (`extract_mention_user_ids` / `extract_object_code_refs` scan the whole
/// body) — a message-amplification guard, not just a UI courtesy.
pub const MAX_MESSAGE_BODY_CHARS: usize = 4000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct MessageBody(String);

impl MessageBody {
    pub fn new(value: impl Into<String>) -> Result<Self, KernelError> {
        let value = value.into();
        let trimmed = value.trim();
        if trimmed.is_empty() {
            return Err(KernelError::validation("message body is required"));
        }
        if trimmed.chars().count() > MAX_MESSAGE_BODY_CHARS {
            return Err(KernelError::validation(format!(
                "message body must be at most {MAX_MESSAGE_BODY_CHARS} characters"
            )));
        }
        Ok(Self(trimmed.to_owned()))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    /// Fixed RFC-4122 UUID that parses as [`UserId`] (version nibble 4, variant 8).
    const ALICE: &str = "11111111-1111-4111-8111-111111111111";
    const BOB: &str = "22222222-2222-4222-8222-222222222222";

    fn alice() -> UserId {
        UserId::from_str(ALICE).expect("ALICE is a valid UserId")
    }

    fn bob() -> UserId {
        UserId::from_str(BOB).expect("BOB is a valid UserId")
    }

    #[test]
    fn thread_kind_db_str_roundtrip_and_rejects_unknown() {
        for kind in [
            ThreadKind::WorkOrder,
            ThreadKind::Team,
            ThreadKind::Dm,
            ThreadKind::Group,
        ] {
            assert_eq!(ThreadKind::from_db_str(kind.as_db_str()).unwrap(), kind);
        }
        assert_eq!(ThreadKind::WorkOrder.as_db_str(), "work_order");
        assert_eq!(ThreadKind::Team.as_db_str(), "team");
        assert_eq!(ThreadKind::Dm.as_db_str(), "dm");
        assert_eq!(ThreadKind::Group.as_db_str(), "group");
        assert!(ThreadKind::from_db_str("WORK_ORDER").is_err());
        assert!(ThreadKind::from_db_str("unknown").is_err());
        assert!(ThreadKind::from_db_str("").is_err());
    }

    #[test]
    fn thread_visibility_db_str_roundtrip_and_rejects_unknown() {
        for v in [ThreadVisibility::Channel, ThreadVisibility::Direct] {
            assert_eq!(ThreadVisibility::from_db_str(v.as_db_str()).unwrap(), v);
        }
        assert_eq!(ThreadVisibility::Channel.as_db_str(), "channel");
        assert_eq!(ThreadVisibility::Direct.as_db_str(), "direct");
        assert!(ThreadVisibility::from_db_str("CHANNEL").is_err());
        assert!(ThreadVisibility::from_db_str("nonsense").is_err());
        assert!(ThreadVisibility::from_db_str("").is_err());
    }

    #[test]
    fn thread_visibility_default_for_named_team_is_channel_else_direct() {
        assert_eq!(
            ThreadVisibility::default_for(ThreadKind::Team, true),
            ThreadVisibility::Channel
        );
        assert_eq!(
            ThreadVisibility::default_for(ThreadKind::Team, false),
            ThreadVisibility::Direct
        );
        for kind in [ThreadKind::WorkOrder, ThreadKind::Dm, ThreadKind::Group] {
            assert_eq!(
                ThreadVisibility::default_for(kind, true),
                ThreadVisibility::Direct
            );
            assert_eq!(
                ThreadVisibility::default_for(kind, false),
                ThreadVisibility::Direct
            );
        }
    }

    #[test]
    fn presence_status_for_age_thresholds() {
        assert_eq!(presence_status_for_age(None), PresenceStatus::Offline);
        // Negative age (clock skew / future stamp) is online.
        assert_eq!(presence_status_for_age(Some(-1)), PresenceStatus::Online);
        assert_eq!(presence_status_for_age(Some(0)), PresenceStatus::Online);
        assert_eq!(
            presence_status_for_age(Some(PRESENCE_ONLINE_SECONDS - 1)),
            PresenceStatus::Online
        );
        // <300 online; <1800 away; else offline (constants: 5*60 / 30*60).
        assert_eq!(
            presence_status_for_age(Some(PRESENCE_ONLINE_SECONDS)),
            PresenceStatus::Away
        );
        assert_eq!(
            presence_status_for_age(Some(PRESENCE_AWAY_SECONDS - 1)),
            PresenceStatus::Away
        );
        assert_eq!(
            presence_status_for_age(Some(PRESENCE_AWAY_SECONDS)),
            PresenceStatus::Offline
        );
        assert_eq!(
            presence_status_for_age(Some(PRESENCE_AWAY_SECONDS + 1)),
            PresenceStatus::Offline
        );
    }

    #[test]
    fn message_body_rejects_empty_whitespace_and_over_max_accepts_valid() {
        assert!(MessageBody::new("").is_err());
        assert!(MessageBody::new(" \t\n ").is_err());
        let too_long = "a".repeat(MAX_MESSAGE_BODY_CHARS + 1);
        assert!(MessageBody::new(too_long).is_err());

        let ok = MessageBody::new("  누유 확인  ").unwrap();
        assert_eq!(ok.as_str(), "누유 확인");
        let at_cap = "a".repeat(MAX_MESSAGE_BODY_CHARS);
        assert!(MessageBody::new(at_cap).is_ok());
    }

    #[test]
    fn extract_mention_user_ids_boundary_dedupe_and_mid_word_ignore() {
        let body = format!("확인 부탁 @{ALICE} 그리고 (@{BOB}) 다시 @{ALICE}");
        assert_eq!(
            extract_mention_user_ids(&body),
            vec![alice(), bob()],
            "valid UUID after @ with boundary; first-seen order; dedupe"
        );
        // Mid-word @ is not a mention (email local-part / attached letter).
        assert!(extract_mention_user_ids(&format!("메일a@{ALICE}")).is_empty());
        assert!(extract_mention_user_ids(&format!("user@{ALICE}")).is_empty());
        // Non-UUID after @ is ignored.
        assert!(extract_mention_user_ids("@홍길동 안녕").is_empty());
        assert!(extract_mention_user_ids("@@ @! @123").is_empty());
    }

    #[test]
    fn extract_object_code_refs_wo_shape_rejects_noise_and_caps() {
        assert_eq!(
            extract_object_code_refs("확인 #WO-20260612-001 그리고 (#AP-3121)"),
            vec!["WO-20260612-001".to_owned(), "AP-3121".to_owned()],
        );
        // Hashtag noise / malformed: no dash, lowercase prefix, empty body after dash.
        assert!(
            extract_object_code_refs(
                "#hashtag #wo-1 #WO- plain @11111111-1111-4111-8111-111111111111"
            )
            .is_empty()
        );
        let body = (0..MAX_OBJECT_CODE_REFS + 25)
            .map(|i| format!("#CODE-{i}"))
            .collect::<Vec<_>>()
            .join(" ");
        assert_eq!(
            extract_object_code_refs(&body).len(),
            MAX_OBJECT_CODE_REFS,
            "refs capped at MAX_OBJECT_CODE_REFS"
        );
    }
}
