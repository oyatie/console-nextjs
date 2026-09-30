//! Webmail domain.
//!
//! Pure value objects and enum wire contracts only. Persistence (sqlx), IMAP
//! (`async-imap`), SMTP (`lettre`), credential encryption, audit, and REST all
//! live in outer layers — this crate has NO async and NO I/O dependencies.
//!
//! Resolved design rulings (see `.omc/research/webmail-build-plan.md`):
//!   * `FolderRole` includes `Junk` (DB CHECK + enum); UI may collapse it.
//!   * `MailSecurity` has NO plaintext/`None` variant — plaintext is
//!     unrepresentable. `SslTls` maps to the DB token `'TLS'`, `StartTls` to
//!     `'STARTTLS'`.
//!   * Threading groups by a normalized subject after stripping localized
//!     reply/forward prefixes (English + Korean), per account.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

use console_kernel_core::KernelError;
use serde::{Deserialize, Serialize};

/// Direction of a stored message relative to the tenant's mailbox.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum MailDirection {
    /// Received from a remote sender (mirrored in from IMAP).
    In,
    /// Sent by the tenant (composed/replied/forwarded out via SMTP).
    Out,
}

impl MailDirection {
    #[must_use]
    pub const fn as_db_str(self) -> &'static str {
        match self {
            Self::In => "IN",
            Self::Out => "OUT",
        }
    }

    pub fn parse(value: &str) -> Result<Self, KernelError> {
        match value {
            "IN" => Ok(Self::In),
            "OUT" => Ok(Self::Out),
            other => Err(KernelError::validation(format!(
                "unknown mail direction {other:?}"
            ))),
        }
    }
}

/// The mutable per-message IMAP flags this subsystem tracks. Mapped to the
/// boolean columns on `email_messages`; modelled as an enum so the sync/store
/// layers can talk about a single flag transition without a wide signature.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum MailFlag {
    /// `\Seen` — the message has been read.
    Seen,
    /// `\Flagged` — starred / important.
    Flagged,
    /// `\Answered` — a reply has been sent.
    Answered,
    /// `\Draft` — an unsent draft.
    Draft,
}

impl MailFlag {
    /// The canonical RFC 3501 system-flag spelling (including the leading `\`).
    #[must_use]
    pub const fn imap_flag(self) -> &'static str {
        match self {
            Self::Seen => "\\Seen",
            Self::Flagged => "\\Flagged",
            Self::Answered => "\\Answered",
            Self::Draft => "\\Draft",
        }
    }
}

/// The role a mailbox folder plays. `Custom` covers any server folder that is
/// not one of the well-known roles. `Junk` is retained (the DB CHECK allows it);
/// the UI may present it under a generic row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum FolderRole {
    Inbox,
    Sent,
    Drafts,
    Archive,
    Trash,
    Junk,
    Custom,
}

impl FolderRole {
    #[must_use]
    pub const fn as_db_str(self) -> &'static str {
        match self {
            Self::Inbox => "INBOX",
            Self::Sent => "SENT",
            Self::Drafts => "DRAFTS",
            Self::Archive => "ARCHIVE",
            Self::Trash => "TRASH",
            Self::Junk => "JUNK",
            Self::Custom => "CUSTOM",
        }
    }

    pub fn parse(value: &str) -> Result<Self, KernelError> {
        match value {
            "INBOX" => Ok(Self::Inbox),
            "SENT" => Ok(Self::Sent),
            "DRAFTS" => Ok(Self::Drafts),
            "ARCHIVE" => Ok(Self::Archive),
            "TRASH" => Ok(Self::Trash),
            "JUNK" => Ok(Self::Junk),
            "CUSTOM" => Ok(Self::Custom),
            other => Err(KernelError::validation(format!(
                "unknown mail folder role {other:?}"
            ))),
        }
    }
}

/// Transport security for an SMTP/IMAP connection. There is deliberately NO
/// plaintext / opportunistic variant: plaintext must be unrepresentable so a
/// misconfiguration cannot downgrade a tenant's mail to cleartext.
///
/// `SslTls` is implicit TLS (IMAPS/SMTPS); the wire token is the DB's `'TLS'`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum MailSecurity {
    /// Implicit TLS on connect (IMAP 993 / SMTP 465). DB token `'TLS'`.
    SslTls,
    /// Upgrade a plaintext connection via STARTTLS (IMAP 143 / SMTP 587).
    StartTls,
}

impl MailSecurity {
    /// The DB CHECK token. `SslTls` maps to `'TLS'` (the migration's allowed
    /// value), `StartTls` to `'STARTTLS'`.
    #[must_use]
    pub const fn as_db_str(self) -> &'static str {
        match self {
            Self::SslTls => "TLS",
            Self::StartTls => "STARTTLS",
        }
    }

    pub fn parse(value: &str) -> Result<Self, KernelError> {
        match value {
            "TLS" => Ok(Self::SslTls),
            "STARTTLS" => Ok(Self::StartTls),
            other => Err(KernelError::validation(format!(
                "unknown mail security mode {other:?} (plaintext is not permitted)"
            ))),
        }
    }
}

/// A single addressee on a message header (`From`/`To`/`Cc`/`Bcc`).
///
/// `address` is the bare mailbox (`user@example.com`); `name` is the optional
/// display name. Stored as JSONB on `email_messages`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageAddress {
    pub address: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

impl MessageAddress {
    pub fn new(address: impl Into<String>) -> Result<Self, KernelError> {
        let address = address.into();
        let trimmed = address.trim();
        if trimmed.is_empty() {
            return Err(KernelError::validation("message address is required"));
        }
        Ok(Self {
            address: trimmed.to_owned(),
            name: None,
        })
    }

    #[must_use]
    pub fn with_name(mut self, name: Option<String>) -> Self {
        self.name = name.map(|n| n.trim().to_owned()).filter(|n| !n.is_empty());
        self
    }
}

/// Reply/forward subject prefixes to strip when normalizing for threading.
/// Lowercased, compared case-insensitively. Includes the common Korean mail
/// client prefixes (`회신`, `답장`, `전달`) alongside the RFC `re`/`fwd`/`fw`.
const SUBJECT_PREFIXES: &[&str] = &["re", "fwd", "fw", "회신", "답장", "전달"];

/// Normalize a subject for threading: trim, strip any leading run of
/// reply/forward prefixes (e.g. `Re:`, `RE[2]:`, `Fwd:`, `회신:`, `답장:`),
/// and collapse internal whitespace. The result is what `email_threads.
/// normalized_subject` stores and what the threading fallback groups on.
///
/// An all-prefix (or empty) subject normalizes to the empty string; callers
/// treat an empty normalized subject as "no subject-based grouping".
#[must_use]
pub fn normalize_subject(subject: &str) -> String {
    let mut current = subject.trim();

    // Repeatedly peel a single leading prefix token followed by `:` (optionally
    // with a bracketed/parenthesized count like `Re[2]:` or `Re(2):`).
    'outer: loop {
        for prefix in SUBJECT_PREFIXES {
            if let Some(rest) = strip_one_prefix(current, prefix) {
                current = rest.trim_start();
                continue 'outer;
            }
        }
        break;
    }

    collapse_whitespace(current)
}

/// If `subject` (case-insensitively) begins with `prefix`, an optional
/// `[n]`/`(n)` counter, and a `:`, return the remainder after the colon.
fn strip_one_prefix<'a>(subject: &'a str, prefix: &str) -> Option<&'a str> {
    let lower = subject.to_lowercase();
    let prefix_lower = prefix.to_lowercase();
    if !lower.starts_with(&prefix_lower) {
        return None;
    }
    // Byte length of the prefix is identical in the original and lowercased
    // string because lowercasing here never changes the byte length for ASCII
    // and the Korean prefixes are unchanged by `to_lowercase`.
    let mut rest = &subject[prefix.len()..];
    rest = rest.trim_start();

    // Optional bracketed/parenthesized counter, e.g. `[2]` or `(3)`.
    if let Some(after) = strip_counter(rest) {
        rest = after.trim_start();
    }

    let rest = rest.strip_prefix(':')?;
    Some(rest)
}

/// Strip a leading `[..]` or `(..)` group (a reply counter) if present.
fn strip_counter(s: &str) -> Option<&str> {
    let (open, close) = if s.starts_with('[') {
        ('[', ']')
    } else if s.starts_with('(') {
        ('(', ')')
    } else {
        return None;
    };
    debug_assert_eq!(s.chars().next(), Some(open));
    let close_idx = s.find(close)?;
    Some(&s[close_idx + close.len_utf8()..])
}

/// Collapse all runs of whitespace to a single space and trim the ends.
fn collapse_whitespace(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Derive a stable per-account thread grouping key from the normalized subject.
///
/// Threading proper (References/In-Reply-To walk) happens in the sync layer;
/// this key is the subject-based FALLBACK and the value persisted in
/// `email_threads.normalized_subject`. The account scope is included so two
/// accounts with the same subject never collide; the org scope is enforced by
/// RLS at the storage layer (every row carries `org_id`), so it is intentionally
/// NOT part of this key.
#[must_use]
pub fn thread_key(account_id: &str, subject: &str) -> String {
    format!("{account_id}|{}", normalize_subject(subject))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn db_str_roundtrips() {
        for d in [MailDirection::In, MailDirection::Out] {
            assert_eq!(MailDirection::parse(d.as_db_str()).unwrap(), d);
        }
        for r in [
            FolderRole::Inbox,
            FolderRole::Sent,
            FolderRole::Drafts,
            FolderRole::Archive,
            FolderRole::Trash,
            FolderRole::Junk,
            FolderRole::Custom,
        ] {
            assert_eq!(FolderRole::parse(r.as_db_str()).unwrap(), r);
        }
        for s in [MailSecurity::SslTls, MailSecurity::StartTls] {
            assert_eq!(MailSecurity::parse(s.as_db_str()).unwrap(), s);
        }
    }

    #[test]
    fn ssl_tls_maps_to_tls_token() {
        // The decisive ruling: SslTls -> the DB's 'TLS'.
        assert_eq!(MailSecurity::SslTls.as_db_str(), "TLS");
        assert_eq!(MailSecurity::StartTls.as_db_str(), "STARTTLS");
        assert_eq!(MailSecurity::parse("TLS").unwrap(), MailSecurity::SslTls);
    }

    #[test]
    fn mail_security_rejects_plaintext() {
        assert!(MailSecurity::parse("NONE").is_err());
        assert!(MailSecurity::parse("PLAIN").is_err());
        assert!(MailSecurity::parse("").is_err());
    }

    #[test]
    fn imap_flag_spellings() {
        assert_eq!(MailFlag::Seen.imap_flag(), "\\Seen");
        assert_eq!(MailFlag::Flagged.imap_flag(), "\\Flagged");
        assert_eq!(MailFlag::Answered.imap_flag(), "\\Answered");
        assert_eq!(MailFlag::Draft.imap_flag(), "\\Draft");
    }

    #[test]
    fn parse_rejects_unknown() {
        assert!(MailDirection::parse("SIDEWAYS").is_err());
        assert!(FolderRole::parse("SPAM").is_err());
    }

    #[test]
    fn message_address_trims_and_validates() {
        let a = MessageAddress::new("  user@example.com  ").unwrap();
        assert_eq!(a.address, "user@example.com");
        assert!(a.name.is_none());
        assert!(MessageAddress::new("   ").is_err());

        let named = MessageAddress::new("u@e.com")
            .unwrap()
            .with_name(Some("  Field Tech  ".to_owned()));
        assert_eq!(named.name.as_deref(), Some("Field Tech"));
        // An empty display name is dropped to None.
        let empty = MessageAddress::new("u@e.com")
            .unwrap()
            .with_name(Some("   ".to_owned()));
        assert!(empty.name.is_none());
    }

    #[test]
    fn normalize_subject_strips_plain_text() {
        assert_eq!(normalize_subject("Quarterly report"), "Quarterly report");
        assert_eq!(normalize_subject("   spaced   out   "), "spaced out");
        assert_eq!(normalize_subject(""), "");
    }

    #[test]
    fn normalize_subject_strips_reply_and_forward_prefixes() {
        assert_eq!(normalize_subject("Re: Hello"), "Hello");
        assert_eq!(normalize_subject("RE: Hello"), "Hello");
        assert_eq!(normalize_subject("re:Hello"), "Hello");
        assert_eq!(normalize_subject("Fwd: Hello"), "Hello");
        assert_eq!(normalize_subject("FW: Hello"), "Hello");
        // Stacked prefixes peel fully.
        assert_eq!(normalize_subject("Re: Fwd: Re: Hello"), "Hello");
    }

    #[test]
    fn normalize_subject_strips_counters() {
        assert_eq!(normalize_subject("Re[2]: Hello"), "Hello");
        assert_eq!(normalize_subject("RE(3): Hello"), "Hello");
        assert_eq!(normalize_subject("Re [2] : Hello"), "Hello");
    }

    #[test]
    fn normalize_subject_strips_korean_prefixes() {
        assert_eq!(normalize_subject("회신: 안녕하세요"), "안녕하세요");
        assert_eq!(normalize_subject("답장: 견적 문의"), "견적 문의");
        assert_eq!(normalize_subject("전달: 작업 지시서"), "작업 지시서");
        // Mixed English + Korean stack.
        assert_eq!(normalize_subject("Re: 전달: 견적"), "견적");
    }

    #[test]
    fn normalize_subject_all_prefix_becomes_empty() {
        assert_eq!(normalize_subject("Re:"), "");
        assert_eq!(normalize_subject("Re: Fwd:"), "");
    }

    #[test]
    fn normalize_subject_does_not_strip_word_starting_with_prefix() {
        // "Reminder" starts with "re" but is not a "re:" prefix — must survive.
        assert_eq!(
            normalize_subject("Reminder: pay invoice"),
            "Reminder: pay invoice"
        );
        assert_eq!(
            normalize_subject("Fworder of operations"),
            "Fworder of operations"
        );
    }

    #[test]
    fn thread_key_scopes_by_account_and_normalizes() {
        let k1 = thread_key("acct-1", "Re: Budget");
        let k2 = thread_key("acct-1", "Budget");
        assert_eq!(k1, k2, "reply normalizes to the same thread key");
        assert_eq!(k1, "acct-1|Budget");

        let other = thread_key("acct-2", "Budget");
        assert_ne!(k1, other, "different accounts never share a thread key");
    }

    #[test]
    fn thread_key_handles_empty_subject() {
        assert_eq!(thread_key("acct-1", "Re:"), "acct-1|");
    }
}
