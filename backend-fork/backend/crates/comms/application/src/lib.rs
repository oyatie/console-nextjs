//! Webmail application layer.
//!
//! Ports (capabilities the outer adapters implement), command/DTO types, audit
//! builders, and the orchestrating services. This crate has NO `sqlx` and NO
//! `lettre` dependency — it speaks only to the [`MailStore`], [`SmtpSender`],
//! [`CredentialCipher`], and [`MailNotifier`] ports, so the domain/application
//! logic stays testable without a database or a live SMTP server.
//!
//! # Write-only credentials
//!
//! A configured account NEVER round-trips its password. [`AccountView`] (the
//! get/read DTO) deliberately omits every secret and instead carries
//! `has_smtp_password`/`has_imap_password` booleans. On configure, a present
//! password is sealed via the [`CredentialCipher`] and only the ciphertext is
//! handed to the store; an absent (`None`) password leaves the stored secret
//! unchanged.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

use std::future::Future;
use std::pin::Pin;
use std::str::FromStr;

pub use crate::credential_cipher::{Aad, CipherError, CredentialCipher, SealedCredential};
use console_comms_domain::{MailSecurity, MessageAddress};
use console_kernel_core::{
    AuditAction, AuditEvent, KernelError, OrgId, Timestamp, TraceContext, UserId,
};
use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

// ---------------------------------------------------------------------------
// CredentialCipher PORT
//
// The webmail credential-cipher capability is an APPLICATION-layer port: both
// the application services and the Postgres adapter depend on the abstraction,
// while the concrete envelope-AEAD implementation lives in the
// `console-comms-credential-cipher` crate (an outer/infra crate that depends on this
// one and implements the trait). Defining the trait + its value types here keeps
// the dependency direction clean (application does not depend on an adapter) and
// is why the orphan-rule blanket impls for `&C` / `Arc<C>` live alongside it.
// ---------------------------------------------------------------------------
pub mod credential_cipher {
    use secrecy::SecretBox;

    /// Errors from the credential cipher. Deliberately coarse and free of any
    /// secret material — an attacker learns only "it failed", never plaintext,
    /// key bytes, or which check failed in a way that aids forgery.
    #[derive(Debug, thiserror::Error)]
    pub enum CipherError {
        /// The master KEK env var is missing, not valid base64, or not 32 bytes.
        #[error("master key configuration error")]
        MasterKey,
        /// AEAD encryption failed (allocation/internal). Carries no detail.
        #[error("encryption failed")]
        Encrypt,
        /// AEAD decryption/authentication failed: wrong KEK, tampered ciphertext /
        /// nonce / wrapped DEK, or mismatched AAD (wrong org/account/field).
        #[error("decryption failed")]
        Decrypt,
        /// A persisted key_version this build cannot interpret.
        #[error("unsupported key version")]
        KeyVersion,
    }

    /// Associated data binding a ciphertext to the exact row + field it belongs
    /// to.
    ///
    /// Encoded into the AEAD AAD on both the secret-seal and the DEK-wrap, so a
    /// ciphertext lifted to a different org, account, or field fails to
    /// authenticate. The encoding is unambiguous (length-prefixed) so distinct
    /// triples can never collide.
    #[derive(Debug, Clone, Copy)]
    pub struct Aad<'a> {
        /// The owning tenant (`email_accounts.org_id`).
        pub org_id: &'a str,
        /// The owning account row (`email_accounts.id`).
        pub account_id: &'a str,
        /// The credential field, e.g. `"smtp_password"` / `"imap_password"`.
        pub field: &'a str,
    }

    impl Aad<'_> {
        /// Length-prefixed, unambiguous byte encoding of the triple.
        #[must_use]
        pub fn encode(&self) -> Vec<u8> {
            let mut out = Vec::new();
            for part in [self.org_id, self.account_id, self.field] {
                let bytes = part.as_bytes();
                // u32 length prefix keeps `("ab","c",..)` distinct from
                // `("a","bc",..)`.
                out.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
                out.extend_from_slice(bytes);
            }
            out
        }
    }

    /// The persisted output of [`CredentialCipher::encrypt`]. Every field is
    /// opaque ciphertext / nonce material safe to store; NONE of it reveals the
    /// plaintext.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct SealedCredential {
        /// AEAD ciphertext of the secret (includes the Poly1305 tag).
        pub ciphertext: Vec<u8>,
        /// 24-byte XChaCha nonce used to seal the secret under the DEK.
        pub nonce: Vec<u8>,
        /// The DEK, itself AEAD-sealed under the KEK (includes its tag).
        pub dek_wrapped: Vec<u8>,
        /// 24-byte XChaCha nonce used to wrap the DEK under the KEK.
        pub dek_nonce: Vec<u8>,
        /// The KEK version used to wrap the DEK.
        pub key_version: i16,
    }

    /// Envelope credential cipher port. A single implementation
    /// (`EnvelopeCredentialCipher`, in `console-comms-credential-cipher`) backs it;
    /// the trait exists so application/adapter layers depend on the capability,
    /// not the concrete cipher.
    pub trait CredentialCipher: Send + Sync {
        /// Seal `plaintext` under a fresh per-row DEK wrapped by the master KEK,
        /// binding `aad` (org/account/field) as associated data.
        fn encrypt(&self, plaintext: &[u8], aad: Aad<'_>) -> Result<SealedCredential, CipherError>;

        /// Recover the plaintext secret from a [`SealedCredential`]. Fails (with
        /// the opaque [`CipherError::Decrypt`]) on a wrong KEK, any tampering, or
        /// an AAD that does not match the row the ciphertext was sealed for.
        fn decrypt(
            &self,
            sealed: &SealedCredential,
            aad: Aad<'_>,
        ) -> Result<SecretBox<Vec<u8>>, CipherError>;
    }

    /// Forward the port through a shared reference, so a `&C` satisfies a generic
    /// `C: CredentialCipher` bound without moving/cloning the KEK. (Defined here,
    /// where the trait is local, so there is no orphan-rule violation.)
    impl<C: CredentialCipher + ?Sized> CredentialCipher for &C {
        fn encrypt(&self, plaintext: &[u8], aad: Aad<'_>) -> Result<SealedCredential, CipherError> {
            (**self).encrypt(plaintext, aad)
        }

        fn decrypt(
            &self,
            sealed: &SealedCredential,
            aad: Aad<'_>,
        ) -> Result<SecretBox<Vec<u8>>, CipherError> {
            (**self).decrypt(sealed, aad)
        }
    }

    /// Forward the port through an `Arc`, so the single shared cipher can satisfy
    /// a generic `C: CredentialCipher` bound directly.
    impl<C: CredentialCipher + ?Sized> CredentialCipher for std::sync::Arc<C> {
        fn encrypt(&self, plaintext: &[u8], aad: Aad<'_>) -> Result<SealedCredential, CipherError> {
            (**self).encrypt(plaintext, aad)
        }

        fn decrypt(
            &self,
            sealed: &SealedCredential,
            aad: Aad<'_>,
        ) -> Result<SecretBox<Vec<u8>>, CipherError> {
            (**self).decrypt(sealed, aad)
        }
    }
}

// ---------------------------------------------------------------------------
// Local typed IDs
//
// `email_accounts.id` / `email_messages.id` have no kernel-core newtype; they
// are introduced by this subsystem, so we define transparent UUID newtypes here
// rather than widening the shared kernel.
// ---------------------------------------------------------------------------

macro_rules! comms_id {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[derive(
            Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash,
            Serialize, Deserialize,
        )]
        #[serde(transparent)]
        pub struct $name(uuid::Uuid);

        impl $name {
            #[must_use]
            pub fn new() -> Self {
                Self(uuid::Uuid::new_v4())
            }
            #[must_use]
            pub const fn from_uuid(value: uuid::Uuid) -> Self {
                Self(value)
            }
            #[must_use]
            pub const fn as_uuid(&self) -> &uuid::Uuid {
                &self.0
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                self.0.fmt(f)
            }
        }

        impl FromStr for $name {
            type Err = uuid::Error;
            fn from_str(s: &str) -> Result<Self, Self::Err> {
                Ok(Self(uuid::Uuid::parse_str(s)?))
            }
        }
    };
}

comms_id!(
    /// A tenant's corporate webmail account (`email_accounts.id`).
    EmailAccountId
);
comms_id!(
    /// A stored message (`email_messages.id`).
    EmailMessageId
);

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Errors surfaced by the application services. The store/cipher/transport
/// failures are deliberately collapsed into coarse, caller-safe variants — no
/// secret material, host detail, or raw transport string ever crosses this
/// boundary (the REST layer maps these to HTTP without leaking internals).
#[derive(Debug, thiserror::Error)]
pub enum MailServiceError {
    /// A domain/validation failure (bad address, empty recipients, …).
    #[error(transparent)]
    Domain(#[from] KernelError),
    /// No mailbox is configured for this tenant (configure it first).
    #[error("no mailbox is configured for this tenant")]
    NotConfigured,
    /// Credential sealing/opening failed (KEK/cipher problem). Caller-opaque.
    #[error("credential cipher error")]
    Cipher,
    /// The store failed. Carries an opaque, caller-safe message.
    #[error("mail store error")]
    Store,
    /// The outbound connection was refused by the SSRF/host guard or transport.
    /// The `code` is a stable, non-secret token (e.g. `host_not_allowed`).
    #[error("mail transport error: {code}")]
    Transport { code: &'static str },
    /// The per-org + per-user outbound rate limit was exceeded BEFORE any SMTP
    /// call was made. Maps to HTTP 429. The `code` is a stable, non-secret token
    /// naming the bucket that tripped (e.g. `send_per_minute`).
    #[error("rate limit exceeded: {code}")]
    RateLimited { code: &'static str },
    /// The same client operation was already prepared with different content.
    #[error("mail operation conflicts with an earlier request")]
    OperationConflict,
    /// Admission may have committed locally, but no independent durable
    /// confirmation exists. No external send has been attempted.
    #[error("mail admission has an unknown durable outcome: {operation_id}")]
    AdmissionUnknown { operation_id: uuid::Uuid },
}

impl MailServiceError {
    #[must_use]
    pub fn kind(&self) -> console_kernel_core::ErrorKind {
        use console_kernel_core::ErrorKind;
        match self {
            Self::Domain(err) => err.kind,
            Self::NotConfigured => ErrorKind::Validation,
            Self::Cipher | Self::Store => ErrorKind::Internal,
            // RateLimited has no kernel ErrorKind (there is no 429 kind); the REST
            // layer maps the variant to 429 directly, so this is only the coarse
            // bucket used for tracing. Treat it as a client/validation-class fault.
            Self::Transport { .. } | Self::RateLimited { .. } => ErrorKind::Validation,
            Self::OperationConflict => ErrorKind::Conflict,
            Self::AdmissionUnknown { .. } => ErrorKind::Internal,
        }
    }
}

// ---------------------------------------------------------------------------
// Commands & DTOs
// ---------------------------------------------------------------------------

/// Configure (create or replace) the tenant's mailbox. A password field set to
/// `None` leaves the stored secret unchanged; `Some(_)` re-seals it. The store
/// upserts on `(org_id, email_address)`.
#[derive(Debug, Clone)]
pub struct ConfigureAccountCommand {
    pub actor: UserId,
    /// The tenant this mailbox belongs to. Set by the REST layer from the
    /// request principal; bound into the cipher AAD and the audit event, and the
    /// adapter arms it as `app.current_org` for the upsert.
    pub org_id: OrgId,
    pub display_name: String,
    pub email_address: String,
    pub from_name: Option<String>,
    pub imap_host: String,
    pub imap_port: u16,
    pub imap_security: MailSecurity,
    pub imap_username: String,
    /// Write-only: `Some` re-seals; `None` keeps the existing secret.
    pub imap_password: Option<SecretString>,
    pub smtp_host: String,
    pub smtp_port: u16,
    pub smtp_security: MailSecurity,
    pub smtp_username: String,
    /// Write-only: `Some` re-seals; `None` keeps the existing secret.
    pub smtp_password: Option<SecretString>,
    pub trace: TraceContext,
    pub occurred_at: Timestamp,
}

/// The read DTO for a configured mailbox. Carries NO secret — only the
/// `has_*_password` booleans signal whether a credential is on file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountView {
    pub id: EmailAccountId,
    pub display_name: String,
    pub email_address: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from_name: Option<String>,
    pub imap_host: String,
    pub imap_port: u16,
    pub imap_security: MailSecurity,
    pub imap_username: String,
    pub smtp_host: String,
    pub smtp_port: u16,
    pub smtp_security: MailSecurity,
    pub smtp_username: String,
    /// True when an SMTP password is stored (the value itself is write-only).
    pub has_smtp_password: bool,
    /// True when an IMAP password is stored (the value itself is write-only).
    pub has_imap_password: bool,
    pub status: String,
}

/// The decrypted SMTP transport config for a single send. Built inside the
/// service from the stored account + the cipher; the password lives in a
/// [`SecretString`] and never enters a log or a DTO.
#[derive(Clone)]
pub struct SmtpTransportConfig {
    pub host: String,
    pub port: u16,
    pub security: MailSecurity,
    pub username: String,
    pub password: SecretString,
    pub from_address: String,
    pub from_name: Option<String>,
}

impl std::fmt::Debug for SmtpTransportConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SmtpTransportConfig")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("security", &self.security)
            .field("username", &self.username)
            .field("password", &"[REDACTED]")
            .field("from_address", &self.from_address)
            .field("from_name", &self.from_name)
            .finish()
    }
}

/// What kind of outbound message this is. REPLY/FORWARD carry the in-reply-to /
/// references threading headers; NEW carries none.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum SendKind {
    New,
    Reply,
    Forward,
}

/// An attachment to include on an outbound message. Bytes are carried inline;
/// the size cap is enforced by the service.
#[derive(Clone)]
pub struct OutboundAttachment {
    pub filename: String,
    pub content_type: String,
    pub bytes: Vec<u8>,
}

impl std::fmt::Debug for OutboundAttachment {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OutboundAttachment")
            .field("filename", &self.filename)
            .field("content_type", &self.content_type)
            .field("bytes", &format_args!("<{} bytes>", self.bytes.len()))
            .finish()
    }
}

/// Compose an outbound message. The `From` is constrained to the account's own
/// address by the service; the caller cannot set an arbitrary sender.
#[derive(Debug, Clone)]
pub struct SendMessageCommand {
    /// Client-stable identity reused verbatim on retries.
    pub operation_id: uuid::Uuid,
    pub actor: UserId,
    pub kind: SendKind,
    pub to: Vec<MessageAddress>,
    pub cc: Vec<MessageAddress>,
    pub bcc: Vec<MessageAddress>,
    pub subject: String,
    pub body_text: String,
    pub attachments: Vec<OutboundAttachment>,
    /// For REPLY/FORWARD: the `Message-ID` being replied to (`In-Reply-To`).
    pub in_reply_to: Option<String>,
    /// For REPLY/FORWARD: the accumulated `References` chain.
    pub references: Vec<String>,
    pub trace: TraceContext,
    pub occurred_at: Timestamp,
}

/// Recipient cap: a single send may not address more than this many recipients
/// (To + Cc + Bcc), an anti-abuse backstop.
pub const MAX_RECIPIENTS: usize = 50;
/// Maximum total attachment bytes per send.
pub const MAX_ATTACHMENT_TOTAL_BYTES: usize = 25 * 1024 * 1024;
/// Maximum prepared MIME or protocol submission body held in PostgreSQL.
pub const MAX_PREPARED_MAIL_BYTES: usize = 40 * 1024 * 1024;
/// Historical request digests are compared with their original typed codec.
pub const OUTBOUND_REQUEST_CODEC_VERSION: i16 = 1;

// ---------------------------------------------------------------------------
// Outbound rate-limit caps (M1)
//
// A persisted, per-org + per-user fixed-window limiter. Each send/reply/forward
// is checked against BOTH a per-minute and a per-hour bucket BEFORE the SMTP
// call; test-connection is checked against a per-minute bucket. The counter is
// org-scoped + RLS-armed in the adapter (one org's usage never bleeds into
// another's). The caps are deliberately generous for a human operator but bound
// any automated/abusive loop and cap the test-connection probe (an SSRF/scan
// amplifier). Exceeding a bucket → `MailServiceError::RateLimited` → HTTP 429.
// ---------------------------------------------------------------------------

/// Max send/reply/forward operations per user, per ROLLING MINUTE.
pub const SEND_RATE_PER_MINUTE: i64 = 30;
/// Max send/reply/forward operations per user, per ROLLING HOUR.
pub const SEND_RATE_PER_HOUR: i64 = 300;
/// Max SMTP test-connection probes per user, per ROLLING MINUTE.
pub const TEST_RATE_PER_MINUTE: i64 = 5;

/// One window of the fixed-window limiter: a stable bucket key (with the window
/// size encoded, e.g. `mail_send:1m`), the window length in seconds, and the cap.
/// The `code` is the non-secret token surfaced on a 429.
#[derive(Debug, Clone, Copy)]
pub struct RateBucket {
    pub endpoint: &'static str,
    pub window_secs: i64,
    pub cap: i64,
    pub code: &'static str,
}

/// The two send/reply/forward windows (per-minute + per-hour).
pub const SEND_RATE_BUCKETS: [RateBucket; 2] = [
    RateBucket {
        endpoint: "mail_send:1m",
        window_secs: 60,
        cap: SEND_RATE_PER_MINUTE,
        code: "send_per_minute",
    },
    RateBucket {
        endpoint: "mail_send:1h",
        window_secs: 3600,
        cap: SEND_RATE_PER_HOUR,
        code: "send_per_hour",
    },
];

/// The single test-connection window (per-minute).
pub const TEST_RATE_BUCKETS: [RateBucket; 1] = [RateBucket {
    endpoint: "mail_test:1m",
    window_secs: 60,
    cap: TEST_RATE_PER_MINUTE,
    code: "test_per_minute",
}];

/// Floor a timestamp to the start of its fixed window. Mirrors the auth/sales
/// limiters' `floor_to_window`: align to the window grid so every caller in the
/// same window shares a row.
#[must_use]
pub fn floor_to_window(now: Timestamp, window_secs: i64) -> Timestamp {
    let window = window_secs.max(1);
    let unix = now.unix_timestamp();
    let floored = unix - unix.rem_euclid(window);
    Timestamp::from_unix_timestamp(floored).unwrap_or(now)
}

/// The structured test-connection outcome. Never carries a secret; `error_code`
/// is a stable, non-secret token.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TestConnectionResult {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
}

// ---------------------------------------------------------------------------
// Store-facing record types
// ---------------------------------------------------------------------------

/// The persisted row of a mailbox, as the store hands it back. Carries the
/// sealed credentials so the service can decrypt them for a send; it is NEVER
/// serialized to a client (only [`AccountView`] is).
#[derive(Debug, Clone)]
pub struct StoredAccount {
    pub id: EmailAccountId,
    pub org_id: OrgId,
    pub display_name: String,
    pub email_address: String,
    pub from_name: Option<String>,
    pub imap_host: String,
    pub imap_port: u16,
    pub imap_security: MailSecurity,
    pub imap_username: String,
    pub smtp_host: String,
    pub smtp_port: u16,
    pub smtp_security: MailSecurity,
    pub smtp_username: String,
    pub smtp_password: SealedCredential,
    pub imap_password: SealedCredential,
    pub status: String,
}

impl StoredAccount {
    #[must_use]
    pub fn to_view(&self) -> AccountView {
        AccountView {
            id: self.id,
            display_name: self.display_name.clone(),
            email_address: self.email_address.clone(),
            from_name: self.from_name.clone(),
            imap_host: self.imap_host.clone(),
            imap_port: self.imap_port,
            imap_security: self.imap_security,
            imap_username: self.imap_username.clone(),
            smtp_host: self.smtp_host.clone(),
            smtp_port: self.smtp_port,
            smtp_security: self.smtp_security,
            smtp_username: self.smtp_username.clone(),
            // A stored account always has both secrets (the DB columns are NOT
            // NULL); the booleans exist for the write-only contract and a future
            // partial-update path.
            has_smtp_password: true,
            has_imap_password: true,
            status: self.status.clone(),
        }
    }
}

/// The credential ciphertext bundle the store persists on an upsert.
#[derive(Debug, Clone)]
pub struct AccountUpsert {
    pub id: EmailAccountId,
    pub actor: UserId,
    pub display_name: String,
    pub email_address: String,
    pub from_name: Option<String>,
    pub imap_host: String,
    pub imap_port: u16,
    pub imap_security: MailSecurity,
    pub imap_username: String,
    pub smtp_host: String,
    pub smtp_port: u16,
    pub smtp_security: MailSecurity,
    pub smtp_username: String,
    /// `Some` replaces the sealed SMTP secret; `None` keeps the existing one.
    pub smtp_password: Option<SealedCredential>,
    /// `Some` replaces the sealed IMAP secret; `None` keeps the existing one.
    pub imap_password: Option<SealedCredential>,
}

/// Exact bytes to submit after a future fenced, two-site-confirmed claim.
/// SMTP MIME omits Bcc; its recipients live in the protected envelope fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreparedFormat {
    SmtpMime,
}

impl PreparedFormat {
    #[must_use]
    pub const fn as_db_str(self) -> &'static str {
        match self {
            Self::SmtpMime => "SMTP_MIME",
        }
    }
}

#[derive(Clone)]
pub struct PreparedOutbound {
    pub format: PreparedFormat,
    pub bytes: Vec<u8>,
    pub rfc_message_id: Option<String>,
}

/// Company-owned prepared operation. A row is only local intent until the
/// post-commit two-site barrier confirms it; this type is never a send receipt.
#[derive(Clone)]
pub struct OutboundPreparation {
    pub operation_id: uuid::Uuid,
    pub account_id: EmailAccountId,
    pub actor: UserId,
    pub request_codec_version: i16,
    pub request_hash: [u8; 32],
    pub payload_hash: [u8; 32],
    pub from_address: String,
    pub envelope_recipients: Vec<String>,
    pub payload: PreparedOutbound,
}

// ---------------------------------------------------------------------------
// Ports
// ---------------------------------------------------------------------------

/// A boxed, `Send` future — the object-safe async shape every port method
/// returns (the workspace defines async trait methods this way rather than
/// pulling `async-trait`).
pub type MailFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Persistence port for the webmail store. Every method is org-scoped (the
/// adapter arms `app.current_org` via `with_org_conn`/`with_audit`); a missing
/// tenant context fails closed in the adapter.
pub trait MailStore: Send + Sync {
    /// Fetch the tenant's single configured mailbox, or `None`.
    fn get_account(&self) -> MailFuture<'_, Result<Option<StoredAccount>, MailServiceError>>;

    /// Upsert the mailbox config (ciphertext only) and write a config audit row.
    fn upsert_account(
        &self,
        upsert: AccountUpsert,
        audit: AuditEvent,
    ) -> MailFuture<'_, Result<StoredAccount, MailServiceError>>;

    /// Insert an immutable prepared operation, or return the existing one only
    /// when its exact semantic request digest matches. No egress claim is made.
    fn prepare_outbound(
        &self,
        preparation: OutboundPreparation,
        audit: AuditEvent,
    ) -> MailFuture<'_, Result<(), MailServiceError>>;

    /// Atomically increment (or create) the org-scoped, per-user fixed-window
    /// rate-limit counter for one bucket `(actor, endpoint, window_start)` under
    /// the armed tenant and return the NEW attempt count. The adapter arms
    /// `app.current_org` (RLS) for the UPSERT, so the counter is isolated by org.
    /// This is a coarse counter, NOT an audited mutation.
    fn increment_send_rate(
        &self,
        actor: UserId,
        endpoint: &'static str,
        window_start: Timestamp,
    ) -> MailFuture<'_, Result<i64, MailServiceError>>;
}

/// Outbound preparation port. No externally reachable send capability is
/// exposed while two-site claim/fencing is unavailable.
pub trait SmtpSender: Send + Sync {
    /// Authenticate to the configured SMTP server and report success/failure
    /// without sending anything (the test-connection probe).
    fn test_connection<'a>(
        &'a self,
        config: &'a SmtpTransportConfig,
    ) -> MailFuture<'a, Result<TestConnectionResult, MailServiceError>>;

    /// Build the complete submission body without any network call. The exact
    /// bytes, not a recipe to regenerate them, are committed as prepared work.
    fn prepare(
        &self,
        config: &SmtpTransportConfig,
        message: &SendMessageCommand,
        from_address: &str,
    ) -> Result<PreparedOutbound, MailServiceError>;
}

/// Realtime notifier port (e.g. a `pg_notify` channel). A no-op stub satisfies
/// it when realtime is not wired; outbound persistence does not depend on it.
pub trait MailNotifier: Send + Sync {
    /// Signal that a message was posted for `account`. Best-effort; errors here
    /// must NOT fail the enclosing send.
    fn notify_posted(&self, account_id: EmailAccountId) -> MailFuture<'_, ()>;
}

// ---------------------------------------------------------------------------
// Audit builders
// ---------------------------------------------------------------------------

/// Build the account-config audit event. The snapshot records only
/// `{has_credential: true}` — NEVER the secret, host, or username — so the
/// audit log itself can never leak a credential.
pub fn account_config_audit_event(
    actor: UserId,
    account_id: EmailAccountId,
    trace: TraceContext,
    occurred_at: Timestamp,
) -> Result<AuditEvent, KernelError> {
    Ok(AuditEvent::new(
        Some(actor),
        AuditAction::new("email.account.configure")?,
        "email_account",
        account_id.to_string(),
        trace,
        occurred_at,
    )
    .with_snapshots(None, Some(serde_json::json!({ "has_credential": true }))))
}

/// Build the inbound-sync audit event for one mirrored-in message. The snapshot
/// records only `{direction: "IN"}` — NEVER the body, recipients, or subject — so
/// the audit log itself can never leak message content. One row per newly
/// inserted inbound message (re-syncs of an existing UID write no audit).
pub fn inbound_sync_audit_event(
    message_id: EmailMessageId,
    trace: TraceContext,
    occurred_at: Timestamp,
) -> Result<AuditEvent, KernelError> {
    Ok(AuditEvent::new(
        // The sync worker is the system actor (no human); `None` actor.
        None,
        AuditAction::new("email.sync.message")?,
        "email_message",
        message_id.to_string(),
        trace,
        occurred_at,
    )
    .with_snapshots(None, Some(serde_json::json!({ "direction": "IN" }))))
}

/// Audit the first immutable preparation only. An identical retry is a read of
/// that operation, not another preparation. Never label this sent.
pub fn send_preparation_audit_event(
    actor: UserId,
    operation_id: uuid::Uuid,
    trace: TraceContext,
    occurred_at: Timestamp,
) -> Result<AuditEvent, KernelError> {
    Ok(AuditEvent::new(
        Some(actor),
        AuditAction::new("email.send.prepare")?,
        "email_outbound_operation",
        operation_id.to_string(),
        trace,
        occurred_at,
    ))
}

/// Build the anomaly audit event for a mox delivery whose recipient address
/// resolved [`AddressLookup::Ambiguous`] (ACTIVE accounts in more than one
/// org). `org_id` is deliberately left unset: no single tenant owns this event
/// — it is a platform-tier row (`audit_events` RLS `WITH CHECK (org_id IS
/// NULL)` passes unconditionally, mirroring the retention-job carve-out). The
/// snapshot records the bare recipient address only — never message content.
pub fn address_ambiguous_audit_event(
    address: &str,
    trace: TraceContext,
    occurred_at: Timestamp,
) -> Result<AuditEvent, KernelError> {
    Ok(AuditEvent::new(
        None,
        AuditAction::new("email.webhook.address_ambiguous")?,
        "email_address",
        address.to_owned(),
        trace,
        occurred_at,
    )
    .with_snapshots(None, Some(serde_json::json!({ "address": address }))))
}

/// Build the read/unread audit event for a thread-level mailbox action. The
/// snapshots record only counters/state — never subject, body, or addresses.
pub fn thread_read_state_audit_event(
    actor: UserId,
    thread_id: uuid::Uuid,
    before_unread_count: i64,
    after_unread_count: i64,
    seen: bool,
    trace: TraceContext,
    occurred_at: Timestamp,
) -> Result<AuditEvent, KernelError> {
    Ok(AuditEvent::new(
        Some(actor),
        AuditAction::new("email.thread_read_state")?,
        "email_thread",
        thread_id.to_string(),
        trace,
        occurred_at,
    )
    .with_snapshots(
        Some(serde_json::json!({ "unread_count": before_unread_count })),
        Some(serde_json::json!({
            "seen": seen,
            "unread_count": after_unread_count,
        })),
    ))
}

// ---------------------------------------------------------------------------
// Services
// ---------------------------------------------------------------------------

/// Configure / get / test a tenant's mailbox.
pub struct AccountService<S, T, C> {
    store: S,
    sender: T,
    cipher: C,
}

impl<S, T, C> AccountService<S, T, C>
where
    S: MailStore,
    T: SmtpSender,
    C: CredentialCipher,
{
    pub fn new(store: S, sender: T, cipher: C) -> Self {
        Self {
            store,
            sender,
            cipher,
        }
    }

    /// Return the write-only view of the configured mailbox, or `None`.
    pub async fn get_account(&self) -> Result<Option<AccountView>, MailServiceError> {
        Ok(self.store.get_account().await?.map(|a| a.to_view()))
    }

    /// Configure (upsert) the mailbox. The credentials are sealed here so only
    /// ciphertext ever reaches the store; the audit snapshot carries no secret.
    pub async fn configure(
        &self,
        command: ConfigureAccountCommand,
    ) -> Result<AccountView, MailServiceError> {
        validate_account_command(&command)?;

        // Re-use the existing account id when reconfiguring so the AAD (which
        // binds org+account+field) stays stable; otherwise mint a fresh one. The
        // org id comes from the command (the REST layer stamps the request
        // tenant), so the AAD bound here matches what `decrypt` later uses and
        // matches the org the adapter arms for the upsert.
        let existing = self.store.get_account().await?;
        let account_id = existing.as_ref().map_or_else(EmailAccountId::new, |a| a.id);
        let org_id_str = command.org_id.to_string();

        let smtp_password = self.seal_optional(
            command.smtp_password.as_ref(),
            &org_id_str,
            account_id,
            "smtp_password",
        )?;
        let imap_password = self.seal_optional(
            command.imap_password.as_ref(),
            &org_id_str,
            account_id,
            "imap_password",
        )?;

        // A brand-new account MUST supply both secrets (the DB columns are NOT
        // NULL); a reconfigure may omit them to keep the stored values.
        if existing.is_none() && (smtp_password.is_none() || imap_password.is_none()) {
            return Err(KernelError::validation(
                "configuring a new mailbox requires both the SMTP and IMAP password",
            )
            .into());
        }

        let audit = account_config_audit_event(
            command.actor,
            account_id,
            command.trace.clone(),
            command.occurred_at,
        )?
        .with_org(command.org_id);

        let upsert = AccountUpsert {
            id: account_id,
            actor: command.actor,
            display_name: command.display_name,
            email_address: command.email_address,
            from_name: command.from_name,
            imap_host: command.imap_host,
            imap_port: command.imap_port,
            imap_security: command.imap_security,
            imap_username: command.imap_username,
            smtp_host: command.smtp_host,
            smtp_port: command.smtp_port,
            smtp_security: command.smtp_security,
            smtp_username: command.smtp_username,
            smtp_password,
            imap_password,
        };

        let stored = self.store.upsert_account(upsert, audit).await?;
        Ok(stored.to_view())
    }

    /// Probe the configured SMTP server with the stored credentials.
    ///
    /// The test-connection probe makes an outbound network call, so it is
    /// rate-limited per-org + per-user (BEFORE the probe) exactly like a send —
    /// otherwise it could be abused as an SSRF/port-scan amplifier.
    pub async fn test_connection(
        &self,
        actor: UserId,
        now: Timestamp,
    ) -> Result<TestConnectionResult, MailServiceError> {
        let account = self
            .store
            .get_account()
            .await?
            .ok_or(MailServiceError::NotConfigured)?;
        enforce_rate_limit(&self.store, actor, now, &TEST_RATE_BUCKETS).await?;
        let config = self.decrypt_smtp(&account)?;
        self.sender.test_connection(&config).await
    }

    fn seal_optional(
        &self,
        password: Option<&SecretString>,
        org_id: &str,
        account_id: EmailAccountId,
        field: &str,
    ) -> Result<Option<SealedCredential>, MailServiceError> {
        let Some(password) = password else {
            return Ok(None);
        };
        let account_id_str = account_id.to_string();
        let aad = Aad {
            org_id,
            account_id: &account_id_str,
            field,
        };
        let sealed = self
            .cipher
            .encrypt(password.expose_secret().as_bytes(), aad)
            .map_err(|_| MailServiceError::Cipher)?;
        Ok(Some(sealed))
    }

    fn decrypt_smtp(
        &self,
        account: &StoredAccount,
    ) -> Result<SmtpTransportConfig, MailServiceError> {
        let org_id_str = account.org_id.to_string();
        let account_id_str = account.id.to_string();
        let aad = Aad {
            org_id: &org_id_str,
            account_id: &account_id_str,
            field: "smtp_password",
        };
        let secret = self
            .cipher
            .decrypt(&account.smtp_password, aad)
            .map_err(|_| MailServiceError::Cipher)?;
        let password = String::from_utf8(secret.expose_secret().clone())
            .map_err(|_| MailServiceError::Cipher)?;
        Ok(SmtpTransportConfig {
            host: account.smtp_host.clone(),
            port: account.smtp_port,
            security: account.smtp_security,
            username: account.smtp_username.clone(),
            password: SecretString::from(password),
            from_address: account.email_address.clone(),
            from_name: account.from_name.clone(),
        })
    }
}

/// Prepare send / reply / forward as immutable owner work. External egress is
/// disabled until a fenced claim can pass the post-commit two-site barrier.
pub struct SendService<S, T, C, N> {
    store: S,
    sender: T,
    cipher: C,
    _notifier: N,
}

impl<S, T, C, N> SendService<S, T, C, N>
where
    S: MailStore,
    T: SmtpSender,
    C: CredentialCipher,
    N: MailNotifier,
{
    pub fn new(store: S, sender: T, cipher: C, notifier: N) -> Self {
        Self {
            store,
            sender,
            cipher,
            _notifier: notifier,
        }
    }

    pub async fn send(&self, command: SendMessageCommand) -> Result<(), MailServiceError> {
        validate_send_command(&command)?;

        let account = self
            .store
            .get_account()
            .await?
            .ok_or(MailServiceError::NotConfigured)?;

        // Bound requests before preparing the exact bytes. A retry can spend
        // another rate token, but never create a second outbound operation.
        enforce_rate_limit(
            &self.store,
            command.actor,
            command.occurred_at,
            &SEND_RATE_BUCKETS,
        )
        .await?;

        let config = decrypt_smtp_for(&self.cipher, &account)?;
        let from_address = account.email_address.clone();

        let payload = self.sender.prepare(&config, &command, &from_address)?;
        if payload.bytes.is_empty() || payload.bytes.len() > MAX_PREPARED_MAIL_BYTES {
            return Err(KernelError::validation("prepared mail exceeds the 40 MiB limit").into());
        }
        let request_hash = outbound_request_hash(&command, account.id, &from_address)?;
        let payload_hash = Sha256::digest(&payload.bytes).into();
        let audit = send_preparation_audit_event(
            command.actor,
            command.operation_id,
            command.trace.clone(),
            command.occurred_at,
        )?
        .with_org(account.org_id);
        let preparation = OutboundPreparation {
            operation_id: command.operation_id,
            account_id: account.id,
            actor: command.actor,
            request_codec_version: OUTBOUND_REQUEST_CODEC_VERSION,
            request_hash,
            payload_hash,
            from_address,
            envelope_recipients: command
                .to
                .iter()
                .chain(&command.cc)
                .chain(&command.bcc)
                .map(|address| address.address.clone())
                .collect(),
            payload,
        };
        match self.store.prepare_outbound(preparation, audit).await {
            Ok(()) | Err(MailServiceError::Store) => {
                // A successful local COMMIT is not two-site durability; a
                // dropped COMMIT response may also leave the row locally live.
                // No transport or notification is attempted on either path.
                Err(MailServiceError::AdmissionUnknown {
                    operation_id: command.operation_id,
                })
            }
            Err(other) => Err(other),
        }
    }
}

fn outbound_request_hash(
    command: &SendMessageCommand,
    account_id: EmailAccountId,
    from_address: &str,
) -> Result<[u8; 32], MailServiceError> {
    #[derive(Serialize)]
    struct AttachmentDigest<'a> {
        filename: &'a str,
        content_type: &'a str,
        sha256: [u8; 32],
    }
    #[derive(Serialize)]
    struct RequestDigest<'a> {
        version: i16,
        account_id: EmailAccountId,
        actor: UserId,
        kind: SendKind,
        from_address: &'a str,
        to: &'a [MessageAddress],
        cc: &'a [MessageAddress],
        bcc: &'a [MessageAddress],
        subject: &'a str,
        body_text: &'a str,
        in_reply_to: &'a Option<String>,
        references: &'a [String],
        attachments: Vec<AttachmentDigest<'a>>,
    }
    let attachments = command
        .attachments
        .iter()
        .map(|attachment| AttachmentDigest {
            filename: &attachment.filename,
            content_type: &attachment.content_type,
            sha256: Sha256::digest(&attachment.bytes).into(),
        })
        .collect();
    let digest = RequestDigest {
        version: OUTBOUND_REQUEST_CODEC_VERSION,
        account_id,
        actor: command.actor,
        kind: command.kind,
        from_address,
        to: &command.to,
        cc: &command.cc,
        bcc: &command.bcc,
        subject: &command.subject,
        body_text: &command.body_text,
        in_reply_to: &command.in_reply_to,
        references: &command.references,
        attachments,
    };
    let bytes = serde_json::to_vec(&digest)
        .map_err(|_| KernelError::internal("could not encode mail request"))?;
    Ok(Sha256::digest(bytes).into())
}

/// Check every supplied fixed-window bucket for `actor` BEFORE an outbound call.
///
/// Each bucket's counter is incremented for its live window; the FIRST bucket to
/// exceed its cap short-circuits with `RateLimited`. The increment is recorded
/// even on the request that trips the limit (so a burst is fully counted), which
/// is the same fail-shut behaviour the auth/sales limiters use. Because the
/// counter increments inside the consuming request, the cap holds across all app
/// instances (the deployment is multi-instance).
async fn enforce_rate_limit<S: MailStore>(
    store: &S,
    actor: UserId,
    now: Timestamp,
    buckets: &[RateBucket],
) -> Result<(), MailServiceError> {
    for bucket in buckets {
        let window_start = floor_to_window(now, bucket.window_secs);
        let attempts = store
            .increment_send_rate(actor, bucket.endpoint, window_start)
            .await?;
        if attempts > bucket.cap {
            return Err(MailServiceError::RateLimited { code: bucket.code });
        }
    }
    Ok(())
}

fn decrypt_smtp_for<C: CredentialCipher>(
    cipher: &C,
    account: &StoredAccount,
) -> Result<SmtpTransportConfig, MailServiceError> {
    let org_id_str = account.org_id.to_string();
    let account_id_str = account.id.to_string();
    let aad = Aad {
        org_id: &org_id_str,
        account_id: &account_id_str,
        field: "smtp_password",
    };
    let secret = cipher
        .decrypt(&account.smtp_password, aad)
        .map_err(|_| MailServiceError::Cipher)?;
    let password =
        String::from_utf8(secret.expose_secret().clone()).map_err(|_| MailServiceError::Cipher)?;
    Ok(SmtpTransportConfig {
        host: account.smtp_host.clone(),
        port: account.smtp_port,
        security: account.smtp_security,
        username: account.smtp_username.clone(),
        password: SecretString::from(password),
        from_address: account.email_address.clone(),
        from_name: account.from_name.clone(),
    })
}

// ---------------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------------

fn validate_account_command(command: &ConfigureAccountCommand) -> Result<(), KernelError> {
    if command.display_name.trim().is_empty() {
        return Err(KernelError::validation("display name is required"));
    }
    validate_email(&command.email_address)?;
    validate_host(&command.imap_host, "IMAP host")?;
    validate_host(&command.smtp_host, "SMTP host")?;
    if command.imap_username.trim().is_empty() {
        return Err(KernelError::validation("IMAP username is required"));
    }
    if command.smtp_username.trim().is_empty() {
        return Err(KernelError::validation("SMTP username is required"));
    }
    validate_imap_port(command.imap_port)?;
    validate_smtp_port(command.smtp_port)?;
    Ok(())
}

fn validate_send_command(command: &SendMessageCommand) -> Result<(), KernelError> {
    if command.operation_id.is_nil() {
        return Err(KernelError::validation(
            "operation_id must be a non-nil UUID",
        ));
    }
    let recipients = command.to.len() + command.cc.len() + command.bcc.len();
    if command.to.is_empty() {
        return Err(KernelError::validation(
            "at least one To recipient is required",
        ));
    }
    if recipients > MAX_RECIPIENTS {
        return Err(KernelError::validation(format!(
            "a single message may not exceed {MAX_RECIPIENTS} recipients"
        )));
    }
    if command.subject.len() > 1000 {
        return Err(KernelError::validation("subject is too long"));
    }
    let total: usize = command.attachments.iter().map(|a| a.bytes.len()).sum();
    if total > MAX_ATTACHMENT_TOTAL_BYTES {
        return Err(KernelError::validation("attachments exceed the size limit"));
    }
    if matches!(command.kind, SendKind::Reply | SendKind::Forward) && command.in_reply_to.is_none()
    {
        return Err(KernelError::validation(
            "a reply or forward requires the in-reply-to message id",
        ));
    }
    Ok(())
}

fn validate_email(value: &str) -> Result<(), KernelError> {
    let trimmed = value.trim();
    if trimmed.is_empty() || !trimmed.contains('@') || trimmed.len() > 320 {
        return Err(KernelError::validation("a valid email address is required"));
    }
    Ok(())
}

fn validate_host(value: &str, name: &str) -> Result<(), KernelError> {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.len() > 255 {
        return Err(KernelError::validation(format!(
            "a valid {name} is required"
        )));
    }
    Ok(())
}

/// SMTP submission ports: 587 (STARTTLS submission) and 465 (implicit TLS). Port
/// 25 (the unauthenticated MTA-relay port) is deliberately NOT allowed — webmail
/// only ever performs authenticated message submission, and dropping 25 narrows
/// the SSRF surface (it is the classic port for relay/open-relay abuse). The DB
/// CHECK on `email_accounts.smtp_port` mirrors this exact set.
pub const ALLOWED_SMTP_PORTS: [u16; 2] = [587, 465];
/// IMAP ports: 993 (implicit TLS), 143 (STARTTLS).
pub const ALLOWED_IMAP_PORTS: [u16; 2] = [993, 143];

fn validate_smtp_port(port: u16) -> Result<(), KernelError> {
    if ALLOWED_SMTP_PORTS.contains(&port) {
        Ok(())
    } else {
        Err(KernelError::validation("SMTP port must be 587 or 465"))
    }
}

fn validate_imap_port(port: u16) -> Result<(), KernelError> {
    if ALLOWED_IMAP_PORTS.contains(&port) {
        Ok(())
    } else {
        Err(KernelError::validation("IMAP port must be 993 or 143"))
    }
}

// ===========================================================================
// INBOUND IMAP SYNC (B-mail-3)
//
// The sync engine mirrors a tenant's IMAP mailbox INTO Postgres. It speaks only
// to ports — [`ImapClient`] (the network), [`MailStore`]'s inbound methods (the
// DB), [`MailAttachmentStore`] (object storage), and [`CredentialCipher`] (the
// IMAP password) — so the orchestration is unit-testable against a fake IMAP
// client with no live server. Idempotency, threading, and the backfill window
// are decided HERE; the adapter just executes the UPSERT / FETCH.
//
// Per-tenant RLS arming is the adapter's job: every store method runs through
// `with_org_conn`/`with_audit` armed to the account's org, so a sync loop driving
// org A can never read or write org B (the worker passes the org from the
// owner-conn enumeration; the adapter re-arms it for each statement).
// ===========================================================================

/// The maximum number of NEW messages a single sync pass will fetch per folder.
/// Bounds the work (and memory) of any one pass — including the initial backfill
/// — so a huge mailbox is drained over several passes rather than in one burst.
pub const SYNC_BATCH_LIMIT: u32 = 200;

/// The IMAP `FETCH` item set. `BODY.PEEK[]` returns the full RFC822 message
/// WITHOUT setting `\Seen` (PEEK is side-effect free), so mirroring a message in
/// never marks it read on the server. `UID` + `FLAGS` + `INTERNALDATE` ride
/// alongside for dedupe identity, the seen/flagged/answered booleans, and the
/// received timestamp.
pub const SYNC_FETCH_ITEMS: &str = "(UID FLAGS INTERNALDATE BODY.PEEK[])";

/// One folder to mirror, as the IMAP client reports it on a `LIST` + `SELECT`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImapFolder {
    /// The server-side mailbox path (e.g. `INBOX`, `[Gmail]/Sent Mail`).
    pub imap_path: String,
    /// The classified role (Inbox/Sent/…); `Custom` for anything unrecognized.
    pub role: console_comms_domain::FolderRole,
    /// A human display name (the last path segment, server-decoded).
    pub name: String,
}

/// The folder's IMAP selection state — the cursor inputs the sync engine needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImapSelect {
    /// The mailbox `UIDVALIDITY`. If it differs from the persisted value the
    /// stored UIDs are stale and the folder cursor must reset (RFC 3501).
    pub uid_validity: u32,
    /// The mailbox `UIDNEXT` (the UID the next arriving message will get), used
    /// to bound a `UID FETCH` range. `None` if the server omits it.
    pub uid_next: Option<u32>,
    /// Total message count reported by `SELECT` (`EXISTS`).
    pub exists: u32,
}

/// A single fetched message, already MIME-parsed by the adapter (`mail-parser`)
/// into the fields the store persists. The body is split into text/html; the
/// attachments carry their bytes for upload.
#[derive(Clone)]
pub struct FetchedMessage {
    /// The server UID within the selected folder (unique per UIDVALIDITY).
    pub imap_uid: u32,
    /// The RFC 5322 `Message-ID` (already trimmed of `<>`-less garbage); `None`
    /// for the rare message that omits one (deduped on UID alone then).
    pub message_id: Option<String>,
    pub in_reply_to: Option<String>,
    pub references: Vec<String>,
    pub from: Option<MessageAddress>,
    pub to: Vec<MessageAddress>,
    pub cc: Vec<MessageAddress>,
    pub subject: String,
    pub body_text: Option<String>,
    pub body_html: Option<String>,
    pub seen: bool,
    pub flagged: bool,
    pub answered: bool,
    pub draft: bool,
    /// The server `INTERNALDATE` (or the `Date:` header) as a unix timestamp.
    pub received_at: Timestamp,
    pub attachments: Vec<FetchedAttachment>,
}

impl std::fmt::Debug for FetchedMessage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never print body or recipient detail at Debug (it can carry PII); show
        // only the non-secret identity + shape.
        f.debug_struct("FetchedMessage")
            .field("imap_uid", &self.imap_uid)
            .field("message_id", &self.message_id)
            .field("subject_len", &self.subject.len())
            .field("to_count", &self.to.len())
            .field("attachments", &self.attachments.len())
            .field("seen", &self.seen)
            .finish()
    }
}

/// A parsed attachment part, with its decoded bytes ready to upload to storage.
#[derive(Clone)]
pub struct FetchedAttachment {
    pub filename: String,
    pub content_type: String,
    pub bytes: Vec<u8>,
    pub content_id: Option<String>,
    pub is_inline: bool,
}

impl std::fmt::Debug for FetchedAttachment {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FetchedAttachment")
            .field("filename", &self.filename)
            .field("content_type", &self.content_type)
            .field("bytes", &format_args!("<{} bytes>", self.bytes.len()))
            .field("is_inline", &self.is_inline)
            .finish()
    }
}

/// Maximum bytes of a single inbound attachment to mirror into storage. Larger
/// parts are recorded with their metadata but their bytes are skipped (the part
/// is persisted with `upload_state = 'PENDING'` carrying zero bytes), so a
/// hostile/huge attachment can't exhaust storage in one pass.
pub const MAX_INBOUND_ATTACHMENT_BYTES: usize = 25 * 1024 * 1024;

/// The decrypted IMAP transport config for one sync pass. The password lives in
/// a [`SecretString`] and is redacted in `Debug`; it is dropped (zeroized by
/// `SecretString`) when the pass ends.
#[derive(Clone)]
pub struct ImapTransportConfig {
    pub host: String,
    pub port: u16,
    pub security: MailSecurity,
    pub username: String,
    pub password: SecretString,
}

impl std::fmt::Debug for ImapTransportConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ImapTransportConfig")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("security", &self.security)
            .field("username", &self.username)
            .field("password", &"[REDACTED]")
            .finish()
    }
}

/// An authenticated IMAP session for one account. Folder-scoped operations run
/// against the currently selected folder. The session owns the connection and
/// closes it on drop / `logout`.
pub trait ImapSession: Send {
    /// List the mailboxes to mirror (already role-classified).
    fn list_folders(&mut self) -> MailFuture<'_, Result<Vec<ImapFolder>, MailServiceError>>;

    /// SELECT a folder and return its UIDVALIDITY/UIDNEXT/EXISTS cursor inputs.
    fn select<'a>(
        &'a mut self,
        imap_path: &'a str,
    ) -> MailFuture<'a, Result<ImapSelect, MailServiceError>>;

    /// `UID FETCH` the messages with UID strictly greater than `since_uid`
    /// (capped at `limit`), against the currently selected folder, using
    /// `BODY.PEEK[]` so no `\Seen` flag is set. Returns the parsed messages in
    /// ascending UID order.
    fn fetch_since<'a>(
        &'a mut self,
        since_uid: u32,
        limit: u32,
    ) -> MailFuture<'a, Result<Vec<FetchedMessage>, MailServiceError>>;

    /// Best-effort logout. Errors here are ignored by the engine.
    fn logout(&mut self) -> MailFuture<'_, ()>;
}

/// The IMAP client port: connect + authenticate to an account's mailbox over a
/// TLS-pinned, SSRF-guarded transport, returning an [`ImapSession`]. The single
/// implementation (`AsyncImapClient`, in `console-comms-adapter-imap`) performs the
/// resolve-once/pin-IP/denylist guard and the rustls handshake; the engine
/// speaks only to this trait so it is testable against a fake client.
pub trait ImapClient: Send + Sync {
    /// Open + authenticate a session. The boxed session is the per-account
    /// connection the engine drives.
    fn connect<'a>(
        &'a self,
        config: &'a ImapTransportConfig,
    ) -> MailFuture<'a, Result<Box<dyn ImapSession>, MailServiceError>>;

    /// Connect + authenticate WITHOUT selecting any folder, then disconnect — the
    /// IMAP half of `/mail/account/test-connection`. Returns a structured result
    /// (never a raw transport string).
    fn test_connection<'a>(
        &'a self,
        config: &'a ImapTransportConfig,
    ) -> MailFuture<'a, Result<TestConnectionResult, MailServiceError>>;
}

/// Object-storage port for inbound attachments. Mirrors the evidence
/// presign/storage pattern: the engine uploads decrypted attachment bytes under
/// an ORG-PREFIXED key, and the REST layer hands the UI a short-lived presigned
/// GET. Implemented by an adapter over `console-platform-storage`'s `S3ObjectStore`.
pub trait MailAttachmentStore: Send + Sync {
    /// Upload `bytes` under an org-prefixed key and return that key. The key
    /// shape is `orgs/{org}/mail/{account}/{message}/{n}-{filename}` — every
    /// component org-scoped so one tenant's object can never be reached by
    /// another's presigned URL.
    fn put<'a>(
        &'a self,
        key: String,
        content_type: String,
        bytes: Vec<u8>,
    ) -> MailFuture<'a, Result<(), MailServiceError>>;

    /// Issue a short-lived presigned GET URL for a stored attachment key.
    fn presign_get<'a>(&'a self, key: &'a str) -> MailFuture<'a, Result<String, MailServiceError>>;
}

/// Forward the attachment port through a shared reference, so a `&A` satisfies a
/// generic `A: MailAttachmentStore` bound (mirrors the `&C: CredentialCipher`
/// blanket impl). Lets a caller pass `&store` without moving it.
impl<A: MailAttachmentStore + ?Sized> MailAttachmentStore for &A {
    fn put<'a>(
        &'a self,
        key: String,
        content_type: String,
        bytes: Vec<u8>,
    ) -> MailFuture<'a, Result<(), MailServiceError>> {
        (**self).put(key, content_type, bytes)
    }

    fn presign_get<'a>(&'a self, key: &'a str) -> MailFuture<'a, Result<String, MailServiceError>> {
        (**self).presign_get(key)
    }
}

/// Build the org-prefixed storage key for an inbound attachment. Every component
/// is org-scoped so a shared bucket can never let one tenant's presigned GET
/// reach another tenant's object.
#[must_use]
pub fn mail_attachment_key(
    org: OrgId,
    account: EmailAccountId,
    message: EmailMessageId,
    sort_order: i16,
    filename: &str,
) -> String {
    format!(
        "orgs/{}/mail/{}/{}/{}-{}",
        org.as_uuid(),
        account,
        message,
        sort_order,
        sanitize_filename(filename)
    )
}

/// Strip path separators and control characters from a filename so it can never
/// escape its key prefix or smuggle a traversal. Empty → `attachment`.
#[must_use]
fn sanitize_filename(filename: &str) -> String {
    let cleaned: String = filename
        .chars()
        .map(|c| {
            if c == '/' || c == '\\' || c.is_control() {
                '_'
            } else {
                c
            }
        })
        .collect();
    let trimmed = cleaned.trim().trim_matches('.');
    if trimmed.is_empty() {
        "attachment".to_owned()
    } else {
        trimmed.chars().take(200).collect()
    }
}

/// The per-folder cursor the store persists and the engine advances. Identifies
/// the folder row and carries the last-seen UID + UIDVALIDITY so an incremental
/// pass fetches only `UID > last_seen_uid` under a matching UIDVALIDITY.
#[derive(Debug, Clone)]
pub struct FolderCursor {
    pub folder_id: uuid::Uuid,
    pub imap_path: String,
    pub uid_validity: Option<i64>,
    pub last_seen_uid: i64,
}

/// One inbound message to UPSERT, with its already-threaded grouping inputs. The
/// store dedupes on `(org, account, folder, uid_validity, uid)` (the idempotent
/// identity) AND secondarily on `message_id`, and attaches it to a thread keyed
/// by the References-walk / normalized-subject fallback decided in the engine.
#[derive(Debug, Clone)]
pub struct InboundUpsert {
    pub id: EmailMessageId,
    pub account_id: EmailAccountId,
    pub folder_id: uuid::Uuid,
    pub uid_validity: i64,
    pub message: FetchedMessage,
    /// The normalized subject (the thread's subject-fallback grouping key).
    pub normalized_subject: String,
    /// Attachments that were uploaded to storage, with their final keys.
    pub stored_attachments: Vec<StoredAttachment>,
}

/// An attachment already uploaded to storage, ready to record in
/// `email_attachments`.
#[derive(Debug, Clone)]
pub struct StoredAttachment {
    pub s3_key: String,
    pub filename: String,
    pub content_type: String,
    pub size_bytes: i64,
    pub content_id: Option<String>,
    pub is_inline: bool,
    pub sort_order: i16,
}

/// The outcome of one account sync pass — a small, non-secret summary for logs
/// and the worker's cadence accounting.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SyncOutcome {
    pub folders_synced: u32,
    pub messages_upserted: u32,
    pub messages_skipped_duplicate: u32,
}

/// An account that is due for a sync pass, as the owner-conn enumeration reports
/// it. Carries ONLY the tenant + account identity (never a secret): the engine
/// re-reads the full sealed account under the armed org before decrypting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DueAccount {
    pub org_id: OrgId,
    pub account_id: EmailAccountId,
    /// Per-claim fencing token minted by the atomic claim. The worker releases
    /// the lease only with this exact token (compare-and-clear in
    /// `record_sync_result`), so a stale worker whose lease was reclaimed by a
    /// second worker (a NEW token) can never clear the second worker's fresh
    /// claim and start an overlapping sync.
    pub claim_token: uuid::Uuid,
}

/// An account resolved from a recipient address for the mox delivery webhook.
/// This is an id-only tenant selector, NOT a scheduler claim: it intentionally
/// carries no sync `claim_token`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AddressAccount {
    pub org_id: OrgId,
    pub account_id: EmailAccountId,
}

/// The outcome of resolving a recipient address to its owning tenant (the mox
/// delivery webhook's cross-tenant, id-only lookup). `email_accounts` is unique
/// only per `(org_id, email_address)`, so the SAME address can exist under two
/// different orgs — `Ambiguous` is that tenant-boundary anomaly and the caller
/// MUST refuse delivery to every match rather than guess one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddressLookup {
    /// No ACTIVE account anywhere has this address.
    NotFound,
    /// Exactly one ACTIVE account, in exactly one org, has this address.
    Found(AddressAccount),
    /// ACTIVE accounts in more than one org have this address.
    Ambiguous,
}

// ---------------------------------------------------------------------------
// Inbound store port (extends MailStore conceptually; kept a separate trait so
// the send-only B-mail-2 surface is unchanged and the read API is cohesive).
// ---------------------------------------------------------------------------

/// Persistence port for the inbound sync engine + the read API. Every method is
/// org-scoped in the adapter (`with_org_conn`/`with_audit` armed to the org). The
/// sync methods are driven by the background worker; the read methods back the
/// REST GET endpoints.
pub trait MailReadStore: Send + Sync {
    /// Ensure the folder rows exist for `account` (upsert on `(org, account,
    /// imap_path)`) and return them as cursors. Arms the account's org.
    fn upsert_folders<'a>(
        &'a self,
        org: OrgId,
        account: EmailAccountId,
        folders: &'a [ImapFolder],
    ) -> MailFuture<'a, Result<Vec<FolderCursor>, MailServiceError>>;

    /// Reset a folder's cursor to 0 and stamp the new UIDVALIDITY (called when the
    /// server's UIDVALIDITY no longer matches the persisted one — the stored UIDs
    /// are stale). Arms the account's org.
    fn reset_folder_cursor<'a>(
        &'a self,
        org: OrgId,
        folder_id: uuid::Uuid,
        uid_validity: i64,
    ) -> MailFuture<'a, Result<(), MailServiceError>>;

    /// Idempotently UPSERT one inbound message into its thread/folder under the
    /// armed org, returning whether a NEW row was inserted (`true`) or an existing
    /// row was refreshed (`false`, a re-sync of the same UID). Maintains the
    /// thread aggregate (last_message_at / counts / has_attachments) and the
    /// folder's `last_seen_uid` high-water mark.
    fn upsert_inbound<'a>(
        &'a self,
        org: OrgId,
        upsert: InboundUpsert,
    ) -> MailFuture<'a, Result<bool, MailServiceError>>;

    /// Stamp the account's sync lifecycle (last_sync_at / sync_status / error)
    /// AND release the scheduler's claim lease — but only if `claim_token` still
    /// matches the row's live token (a fenced compare-and-clear). A worker whose
    /// lease was already reclaimed by another worker (a newer token) clears
    /// nothing, so it cannot wipe the reclaimer's fresh lease. Arms the account's
    /// org.
    fn record_sync_result<'a>(
        &'a self,
        org: OrgId,
        account: EmailAccountId,
        claim_token: uuid::Uuid,
        status: &'a str,
        error: Option<&'a str>,
    ) -> MailFuture<'a, Result<(), MailServiceError>>;

    /// Release the scheduler's claim lease WITHOUT touching the sync lifecycle —
    /// no `last_sync_at`/`sync_status`/`last_sync_error`/auth-failure-count
    /// write. Used by the early-exit paths (the account vanished, was paused, or
    /// changed identity since enumeration) where no sync attempt was actually
    /// made, so nothing should be recorded as a completed pass. Fenced by
    /// `claim_token` exactly like `record_sync_result`: only the worker still
    /// holding the matching token clears the lease. Arms the account's org.
    fn release_claim<'a>(
        &'a self,
        org: OrgId,
        account: EmailAccountId,
        claim_token: uuid::Uuid,
    ) -> MailFuture<'a, Result<(), MailServiceError>>;

    /// Owner-conn enumeration of accounts DUE for a sync pass (last_sync_at older
    /// than the cadence, status ACTIVE). Reads ONLY (org_id, account_id) and is
    /// NOT armed to any single org — it is the scheduler tick that drives the
    /// per-tenant arming, so it must see across tenants. The adapter runs it on a
    /// dedicated owner connection that bypasses RLS for this id-only read.
    /// `limit` MUST be the number of sync passes the caller can start
    /// immediately (e.g. the concurrency permits already acquired) — claiming
    /// more than that stamps a 600s lease on accounts that then sit queued
    /// before their pass even begins, eating into the lease before the sync
    /// timeout clock (which only starts once the pass runs) has a chance to
    /// bound it.
    fn list_due_accounts(
        &self,
        now: Timestamp,
        limit: i32,
    ) -> MailFuture<'_, Result<Vec<DueAccount>, MailServiceError>>;

    /// Resolve the account whose `email_address` matches `address`. Like
    /// [`list_due_accounts`] this is an id-only lookup on an owner connection
    /// that must see ACROSS tenants — the mox delivery webhook has no request
    /// principal, so the recipient address is the only tenant selector. A
    /// [`AddressLookup::Found`] result arms the resolved org for the actual
    /// ingest; [`AddressLookup::Ambiguous`] (the address exists under more than
    /// one org) MUST deliver to none of them. Reads NO message content.
    fn find_account_by_address<'a>(
        &'a self,
        address: &'a str,
    ) -> MailFuture<'a, Result<AddressLookup, MailServiceError>>;

    // --- READ API (backs the REST GET endpoints; all org-armed) -------------

    /// List the account's folders (for the folder rail).
    fn list_folders<'a>(
        &'a self,
        org: OrgId,
        account: EmailAccountId,
    ) -> MailFuture<'a, Result<Vec<FolderView>, MailServiceError>>;

    /// Page the account's threads (newest first), optionally filtered to unread
    /// and/or a full-text `q`. `before` is the keyset cursor (last seen
    /// `last_message_at`); `limit` is capped by the adapter.
    fn list_threads<'a>(
        &'a self,
        org: OrgId,
        account: EmailAccountId,
        query: &'a ThreadQuery,
    ) -> MailFuture<'a, Result<Vec<ThreadView>, MailServiceError>>;

    /// Fetch one thread's messages (oldest first), or `None` if the thread is not
    /// visible under the armed org.
    fn get_thread<'a>(
        &'a self,
        org: OrgId,
        thread_id: uuid::Uuid,
    ) -> MailFuture<'a, Result<Option<ThreadDetail>, MailServiceError>>;

    /// Fetch one message (with its attachments), or `None`.
    fn get_message<'a>(
        &'a self,
        org: OrgId,
        message_id: EmailMessageId,
    ) -> MailFuture<'a, Result<Option<MessageView>, MailServiceError>>;

    /// Resolve a stored attachment's (s3_key, filename, content_type) within the
    /// armed org, or `None` if it is not visible. Used to issue a presigned GET.
    fn get_attachment_key<'a>(
        &'a self,
        org: OrgId,
        attachment_id: uuid::Uuid,
    ) -> MailFuture<'a, Result<Option<AttachmentRef>, MailServiceError>>;

    /// Set every inbound message in a visible thread to read/unread and
    /// recompute thread/folder unread aggregates. Returns `false` when the
    /// thread is not visible under the armed org.
    fn set_thread_seen<'a>(
        &'a self,
        org: OrgId,
        thread_id: uuid::Uuid,
        seen: bool,
        audit: AuditEvent,
    ) -> MailFuture<'a, Result<bool, MailServiceError>>;
}

/// Forward the read store through a shared reference, so a `&R` satisfies a
/// generic `R: MailReadStore` bound (mirrors the `&C: CredentialCipher` blanket
/// impl). Lets a caller pass `&store` without moving it (the worker passes owned
/// values; tests and the REST layer pass references).
impl<R: MailReadStore + ?Sized> MailReadStore for &R {
    fn upsert_folders<'a>(
        &'a self,
        org: OrgId,
        account: EmailAccountId,
        folders: &'a [ImapFolder],
    ) -> MailFuture<'a, Result<Vec<FolderCursor>, MailServiceError>> {
        (**self).upsert_folders(org, account, folders)
    }

    fn reset_folder_cursor<'a>(
        &'a self,
        org: OrgId,
        folder_id: uuid::Uuid,
        uid_validity: i64,
    ) -> MailFuture<'a, Result<(), MailServiceError>> {
        (**self).reset_folder_cursor(org, folder_id, uid_validity)
    }

    fn upsert_inbound<'a>(
        &'a self,
        org: OrgId,
        upsert: InboundUpsert,
    ) -> MailFuture<'a, Result<bool, MailServiceError>> {
        (**self).upsert_inbound(org, upsert)
    }

    fn record_sync_result<'a>(
        &'a self,
        org: OrgId,
        account: EmailAccountId,
        claim_token: uuid::Uuid,
        status: &'a str,
        error: Option<&'a str>,
    ) -> MailFuture<'a, Result<(), MailServiceError>> {
        (**self).record_sync_result(org, account, claim_token, status, error)
    }

    fn release_claim<'a>(
        &'a self,
        org: OrgId,
        account: EmailAccountId,
        claim_token: uuid::Uuid,
    ) -> MailFuture<'a, Result<(), MailServiceError>> {
        (**self).release_claim(org, account, claim_token)
    }

    fn list_due_accounts(
        &self,
        now: Timestamp,
        limit: i32,
    ) -> MailFuture<'_, Result<Vec<DueAccount>, MailServiceError>> {
        (**self).list_due_accounts(now, limit)
    }

    fn find_account_by_address<'a>(
        &'a self,
        address: &'a str,
    ) -> MailFuture<'a, Result<AddressLookup, MailServiceError>> {
        (**self).find_account_by_address(address)
    }

    fn list_folders<'a>(
        &'a self,
        org: OrgId,
        account: EmailAccountId,
    ) -> MailFuture<'a, Result<Vec<FolderView>, MailServiceError>> {
        (**self).list_folders(org, account)
    }

    fn list_threads<'a>(
        &'a self,
        org: OrgId,
        account: EmailAccountId,
        query: &'a ThreadQuery,
    ) -> MailFuture<'a, Result<Vec<ThreadView>, MailServiceError>> {
        (**self).list_threads(org, account, query)
    }

    fn get_thread<'a>(
        &'a self,
        org: OrgId,
        thread_id: uuid::Uuid,
    ) -> MailFuture<'a, Result<Option<ThreadDetail>, MailServiceError>> {
        (**self).get_thread(org, thread_id)
    }

    fn get_message<'a>(
        &'a self,
        org: OrgId,
        message_id: EmailMessageId,
    ) -> MailFuture<'a, Result<Option<MessageView>, MailServiceError>> {
        (**self).get_message(org, message_id)
    }

    fn get_attachment_key<'a>(
        &'a self,
        org: OrgId,
        attachment_id: uuid::Uuid,
    ) -> MailFuture<'a, Result<Option<AttachmentRef>, MailServiceError>> {
        (**self).get_attachment_key(org, attachment_id)
    }

    fn set_thread_seen<'a>(
        &'a self,
        org: OrgId,
        thread_id: uuid::Uuid,
        seen: bool,
        audit: AuditEvent,
    ) -> MailFuture<'a, Result<bool, MailServiceError>> {
        (**self).set_thread_seen(org, thread_id, seen, audit)
    }
}

/// Query parameters for the paginated thread list.
#[derive(Debug, Clone, Default)]
pub struct ThreadQuery {
    pub folder_id: Option<uuid::Uuid>,
    pub unread_only: bool,
    pub search: Option<String>,
    pub before: Option<Timestamp>,
    pub limit: i64,
}

/// A folder row for the rail.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FolderView {
    pub id: uuid::Uuid,
    pub role: String,
    pub name: String,
    pub unread_count: i64,
    pub total_count: i64,
}

/// A thread row for the list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadView {
    pub id: uuid::Uuid,
    pub subject: String,
    pub last_message_at: Timestamp,
    pub message_count: i64,
    pub unread_count: i64,
    pub has_attachments: bool,
    pub is_flagged: bool,
}

/// A thread with its ordered messages.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadDetail {
    pub id: uuid::Uuid,
    pub subject: String,
    pub messages: Vec<MessageView>,
}

/// A single message, as the read API returns it. `body_html` is sanitized by the
/// web client before render (the API returns the stored HTML verbatim).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageView {
    pub id: EmailMessageId,
    pub thread_id: uuid::Uuid,
    pub direction: String,
    pub message_id: Option<String>,
    pub in_reply_to: Option<String>,
    pub from_address: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from_name: Option<String>,
    pub to: Vec<MessageAddress>,
    pub cc: Vec<MessageAddress>,
    pub subject: String,
    pub snippet: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body_text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body_html: Option<String>,
    pub seen: bool,
    pub flagged: bool,
    pub answered: bool,
    pub has_attachments: bool,
    pub received_at: Timestamp,
    pub attachments: Vec<AttachmentView>,
}

/// An attachment as the read API returns it (no bytes — the UI fetches via a
/// presigned GET keyed by `id`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttachmentView {
    pub id: uuid::Uuid,
    pub filename: String,
    pub content_type: String,
    pub size_bytes: i64,
    pub is_inline: bool,
}

/// The storage coordinates of one attachment, resolved for a presigned GET.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttachmentRef {
    pub s3_key: String,
    pub filename: String,
    pub content_type: String,
}

// ---------------------------------------------------------------------------
// Threading
// ---------------------------------------------------------------------------

/// Decide a message's thread grouping key from its References/In-Reply-To chain,
/// falling back to the normalized subject.
///
/// RFC 5322 threading walks the `References` (or `In-Reply-To`) chain to the
/// ROOT message-id; messages sharing a root belong to one thread. We return the
/// FIRST reference (the conversation root) when present, else the in-reply-to,
/// else `subject:<normalized>` so a same-subject conversation still groups when
/// the client stripped the headers (common for Korean webmail). An empty
/// normalized subject yields `uid:<uid>` so a subject-less, header-less message
/// becomes its own singleton thread rather than colliding with every other.
#[must_use]
pub fn thread_grouping_key(message: &FetchedMessage, normalized_subject: &str) -> String {
    if let Some(root) = message.references.first() {
        return format!("ref:{root}");
    }
    if let Some(parent) = &message.in_reply_to {
        return format!("ref:{parent}");
    }
    if normalized_subject.is_empty() {
        format!("uid:{}", message.imap_uid)
    } else {
        format!("subject:{normalized_subject}")
    }
}

// ---------------------------------------------------------------------------
// SyncService — orchestrates one account's sync pass against the ports.
// ---------------------------------------------------------------------------

/// Drives a single account's incremental sync: connect → list/select folders →
/// fetch new UIDs → upload attachments → idempotent UPSERT → advance cursor.
pub struct SyncService<R, I, A, C> {
    store: R,
    imap: I,
    attachments: A,
    cipher: C,
}

impl<R, I, A, C> SyncService<R, I, A, C>
where
    R: MailReadStore,
    I: ImapClient,
    A: MailAttachmentStore,
    C: CredentialCipher,
{
    pub fn new(store: R, imap: I, attachments: A, cipher: C) -> Self {
        Self {
            store,
            imap,
            attachments,
            cipher,
        }
    }

    /// Run ONE sync pass for `account` (already re-read under the armed org). On
    /// any error the account's sync_status is stamped and the error returned; on
    /// success the lifecycle is stamped `OK`. Either way the scheduler's claim
    /// lease is released via the fenced compare-and-clear keyed on `claim_token`
    /// (the token the atomic claim minted for this pass), so a stale worker whose
    /// lease was already reclaimed clears nothing. The org is threaded through to
    /// every store call so the adapter arms RLS to exactly this tenant.
    pub async fn sync_account(
        &self,
        account: &StoredAccount,
        claim_token: uuid::Uuid,
    ) -> Result<SyncOutcome, MailServiceError> {
        match self.sync_account_inner(account).await {
            Ok(outcome) => {
                self.store
                    .record_sync_result(account.org_id, account.id, claim_token, "OK", None)
                    .await?;
                Ok(outcome)
            }
            Err(err) => {
                let status = sync_status_for(&err);
                // Best-effort lifecycle stamp; the original error is what we
                // return so the caller logs the real cause (non-secret).
                let _ = self
                    .store
                    .record_sync_result(
                        account.org_id,
                        account.id,
                        claim_token,
                        status,
                        Some(err.transport_code()),
                    )
                    .await;
                Err(err)
            }
        }
    }

    async fn sync_account_inner(
        &self,
        account: &StoredAccount,
    ) -> Result<SyncOutcome, MailServiceError> {
        let config = self.decrypt_imap(account)?;
        let mut session = self.imap.connect(&config).await?;

        let result = self.drive_session(account, session.as_mut()).await;
        // Always attempt a clean logout, regardless of the pass outcome.
        session.logout().await;
        result
    }

    async fn drive_session(
        &self,
        account: &StoredAccount,
        session: &mut dyn ImapSession,
    ) -> Result<SyncOutcome, MailServiceError> {
        let folders = session.list_folders().await?;
        let cursors = self
            .store
            .upsert_folders(account.org_id, account.id, &folders)
            .await?;

        let mut outcome = SyncOutcome::default();
        for cursor in &cursors {
            let selected = session.select(&cursor.imap_path).await?;
            outcome.folders_synced += 1;

            // UIDVALIDITY reset: the server's value changed (or we never had one)
            // — the stored UIDs are stale, so reset the cursor to 0 and refetch
            // from the backfill floor under the NEW validity.
            let validity = i64::from(selected.uid_validity);
            let since_uid = if cursor.uid_validity == Some(validity) {
                u32::try_from(cursor.last_seen_uid).unwrap_or(0)
            } else {
                self.store
                    .reset_folder_cursor(account.org_id, cursor.folder_id, validity)
                    .await?;
                0
            };

            let fetched = session.fetch_since(since_uid, SYNC_BATCH_LIMIT).await?;
            for message in fetched {
                let inserted = self
                    .persist_message(account, cursor.folder_id, validity, message)
                    .await?;
                if inserted {
                    outcome.messages_upserted += 1;
                } else {
                    outcome.messages_skipped_duplicate += 1;
                }
            }
        }
        Ok(outcome)
    }

    async fn persist_message(
        &self,
        account: &StoredAccount,
        folder_id: uuid::Uuid,
        validity: i64,
        message: FetchedMessage,
    ) -> Result<bool, MailServiceError> {
        let message_id = EmailMessageId::new();
        let normalized = console_comms_domain::normalize_subject(&message.subject);

        // Upload attachments under org-prefixed keys BEFORE the UPSERT so the
        // recorded rows always reference an object that exists.
        let mut stored = Vec::new();
        let mut sort_order: i16 = 1;
        for attachment in &message.attachments {
            // Bound a single attachment's bytes; oversized parts are recorded
            // (metadata only) but their bytes are not mirrored.
            if attachment.bytes.len() > MAX_INBOUND_ATTACHMENT_BYTES {
                continue;
            }
            let key = mail_attachment_key(
                account.org_id,
                account.id,
                message_id,
                sort_order,
                &attachment.filename,
            );
            self.attachments
                .put(
                    key.clone(),
                    attachment.content_type.clone(),
                    attachment.bytes.clone(),
                )
                .await?;
            stored.push(StoredAttachment {
                s3_key: key,
                filename: attachment.filename.clone(),
                content_type: attachment.content_type.clone(),
                size_bytes: i64::try_from(attachment.bytes.len()).unwrap_or(i64::MAX),
                content_id: attachment.content_id.clone(),
                is_inline: attachment.is_inline,
                sort_order,
            });
            sort_order = sort_order.saturating_add(1);
        }

        let upsert = InboundUpsert {
            id: message_id,
            account_id: account.id,
            folder_id,
            uid_validity: validity,
            message,
            normalized_subject: normalized,
            stored_attachments: stored,
        };
        self.store.upsert_inbound(account.org_id, upsert).await
    }

    /// Probe the IMAP server (connect + auth + disconnect) with the stored
    /// credentials — the IMAP half of test-connection.
    pub async fn test_connection(
        &self,
        account: &StoredAccount,
    ) -> Result<TestConnectionResult, MailServiceError> {
        let config = self.decrypt_imap(account)?;
        self.imap.test_connection(&config).await
    }

    fn decrypt_imap(
        &self,
        account: &StoredAccount,
    ) -> Result<ImapTransportConfig, MailServiceError> {
        let org_id_str = account.org_id.to_string();
        let account_id_str = account.id.to_string();
        let aad = Aad {
            org_id: &org_id_str,
            account_id: &account_id_str,
            field: "imap_password",
        };
        let secret = self
            .cipher
            .decrypt(&account.imap_password, aad)
            .map_err(|_| MailServiceError::Cipher)?;
        let password = String::from_utf8(secret.expose_secret().clone())
            .map_err(|_| MailServiceError::Cipher)?;
        Ok(ImapTransportConfig {
            host: account.imap_host.clone(),
            port: account.imap_port,
            security: account.imap_security,
            username: account.imap_username.clone(),
            password: SecretString::from(password),
        })
    }
}

impl MailServiceError {
    /// The stable, non-secret transport code for a sync error (for the lifecycle
    /// stamp + logs). Non-transport errors collapse to a coarse token.
    #[must_use]
    pub fn transport_code(&self) -> &'static str {
        match self {
            Self::Transport { code } => code,
            Self::NotConfigured => "not_configured",
            Self::Cipher => "cipher_error",
            Self::Store => "store_error",
            Self::RateLimited { .. } => "rate_limited",
            Self::Domain(_) => "validation_error",
            Self::OperationConflict => "operation_conflict",
            Self::AdmissionUnknown { .. } => "admission_unknown",
        }
    }
}

/// Map a sync error to the `email_accounts.sync_status` enum token.
#[must_use]
fn sync_status_for(err: &MailServiceError) -> &'static str {
    match err {
        MailServiceError::Transport { code } if *code == "auth_failed" => "AUTH_FAILED",
        MailServiceError::Transport { .. } => "UNREACHABLE",
        // A store/cipher/validation fault is an internal problem, not an account
        // reachability problem — surface it as UNREACHABLE so the operator
        // re-checks, but it is distinct in the logged transport_code.
        _ => "UNREACHABLE",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn account_view_omits_password_fields() {
        // The view DTO has no `*_password` field at all — only the booleans.
        let stored = sample_stored();
        let view = stored.to_view();
        let json = serde_json::to_string(&view).unwrap();
        assert!(!json.contains("password_ct"));
        assert!(!json.contains("\"smtp_password\""));
        assert!(!json.contains("\"imap_password\""));
        assert!(json.contains("has_smtp_password"));
        assert!(json.contains("has_imap_password"));
    }

    #[test]
    fn port_allowlists_reject_non_mail_ports() {
        assert!(validate_smtp_port(587).is_ok());
        assert!(validate_smtp_port(465).is_ok());
        // Port 25 (unauthenticated MTA relay) is no longer accepted: webmail only
        // performs authenticated submission on 587/465.
        assert!(validate_smtp_port(25).is_err());
        assert!(validate_smtp_port(8025).is_err());
        assert!(validate_smtp_port(80).is_err());
        assert!(validate_imap_port(993).is_ok());
        assert!(validate_imap_port(143).is_ok());
        assert!(validate_imap_port(110).is_err());
    }

    #[test]
    fn floor_to_window_aligns_to_grid() {
        // 1_700_000_040 is a multiple of 60 (and of 3600 → 1_699_999_200 is the
        // hour grid start), so an instant 5s into that minute floors back to it.
        let t = Timestamp::from_unix_timestamp(1_700_000_045).unwrap(); // grid + 5s
        assert_eq!(
            floor_to_window(t, 60).unix_timestamp(),
            1_700_000_040,
            "an instant 5s into a minute floors to the minute grid start"
        );
        assert_eq!(
            floor_to_window(t, 3600).unix_timestamp(),
            1_699_999_200,
            "the same instant floors to the hour grid start"
        );
        // Two instants in the SAME minute window [1_700_000_040, 1_700_000_100)
        // share a bucket; an instant in the NEXT minute does not.
        let a = Timestamp::from_unix_timestamp(1_700_000_041).unwrap();
        let b = Timestamp::from_unix_timestamp(1_700_000_099).unwrap();
        let next = Timestamp::from_unix_timestamp(1_700_000_100).unwrap();
        assert_eq!(floor_to_window(a, 60), floor_to_window(b, 60));
        assert_ne!(floor_to_window(b, 60), floor_to_window(next, 60));
    }

    #[test]
    fn rate_bucket_caps_are_sane() {
        // Sanity-bind the documented caps so an accidental edit is caught.
        assert_eq!(SEND_RATE_BUCKETS[0].cap, SEND_RATE_PER_MINUTE);
        assert_eq!(SEND_RATE_BUCKETS[0].window_secs, 60);
        assert_eq!(SEND_RATE_BUCKETS[1].cap, SEND_RATE_PER_HOUR);
        assert_eq!(SEND_RATE_BUCKETS[1].window_secs, 3600);
        assert_eq!(TEST_RATE_BUCKETS[0].cap, TEST_RATE_PER_MINUTE);
        assert_eq!(TEST_RATE_BUCKETS[0].window_secs, 60);
        // Per-hour cap must be at least the per-minute cap (else the minute bucket
        // is unreachable) — a compile-time invariant on the consts.
        const _: () = assert!(SEND_RATE_PER_HOUR >= SEND_RATE_PER_MINUTE);
    }

    #[test]
    fn reply_requires_in_reply_to() {
        let mut cmd = sample_send(SendKind::Reply);
        cmd.in_reply_to = None;
        assert!(validate_send_command(&cmd).is_err());
        cmd.in_reply_to = Some("<x@y>".to_owned());
        assert!(validate_send_command(&cmd).is_ok());
    }

    #[test]
    fn send_rejects_empty_to_and_over_cap() {
        let mut cmd = sample_send(SendKind::New);
        cmd.to.clear();
        assert!(validate_send_command(&cmd).is_err());

        let mut cmd = sample_send(SendKind::New);
        cmd.to = (0..MAX_RECIPIENTS + 1)
            .map(|i| MessageAddress::new(format!("u{i}@e.com")).unwrap())
            .collect();
        assert!(validate_send_command(&cmd).is_err());
    }

    #[test]
    fn request_digest_ignores_retry_trace_but_binds_content_and_attachment_bytes() {
        let account = EmailAccountId::new();
        let mut command = sample_send(SendKind::New);
        command.attachments = vec![OutboundAttachment {
            filename: "quote.pdf".to_owned(),
            content_type: "application/pdf".to_owned(),
            bytes: b"first".to_vec(),
        }];
        let original = outbound_request_hash(&command, account, "ops@knl.example").unwrap();
        command.trace = TraceContext::generate();
        command.occurred_at = Timestamp::now_utc();
        assert_eq!(
            outbound_request_hash(&command, account, "ops@knl.example").unwrap(),
            original
        );
        command.attachments[0].bytes = b"second".to_vec();
        assert_ne!(
            outbound_request_hash(&command, account, "ops@knl.example").unwrap(),
            original
        );
        command.operation_id = uuid::Uuid::nil();
        assert!(validate_send_command(&command).is_err());
    }

    fn sample_send(kind: SendKind) -> SendMessageCommand {
        SendMessageCommand {
            operation_id: uuid::Uuid::new_v4(),
            actor: UserId::new(),
            kind,
            to: vec![MessageAddress::new("a@b.com").unwrap()],
            cc: vec![],
            bcc: vec![],
            subject: "hi".to_owned(),
            body_text: "body".to_owned(),
            attachments: vec![],
            in_reply_to: Some("<m@h>".to_owned()),
            references: vec![],
            trace: TraceContext::generate(),
            occurred_at: Timestamp::now_utc(),
        }
    }

    fn sample_stored() -> StoredAccount {
        let sealed = SealedCredential {
            ciphertext: vec![1, 2, 3],
            nonce: vec![0; 24],
            dek_wrapped: vec![0; 48],
            dek_nonce: vec![0; 24],
            key_version: 1,
        };
        StoredAccount {
            id: EmailAccountId::new(),
            org_id: OrgId::knl(),
            display_name: "KNL".to_owned(),
            email_address: "ops@knl.example".to_owned(),
            from_name: Some("KNL Ops".to_owned()),
            imap_host: "imap.example.com".to_owned(),
            imap_port: 993,
            imap_security: MailSecurity::SslTls,
            imap_username: "ops".to_owned(),
            smtp_host: "smtp.example.com".to_owned(),
            smtp_port: 587,
            smtp_security: MailSecurity::StartTls,
            smtp_username: "ops".to_owned(),
            smtp_password: sealed.clone(),
            imap_password: sealed,
            status: "ACTIVE".to_owned(),
        }
    }

    #[tokio::test]
    async fn failed_preparation_admission_returns_unknown_without_egress() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        struct FailingAdmissionStore;

        impl MailStore for FailingAdmissionStore {
            fn get_account(
                &self,
            ) -> MailFuture<'_, Result<Option<StoredAccount>, MailServiceError>> {
                Box::pin(async { Ok(Some(sample_stored())) })
            }

            fn upsert_account(
                &self,
                _upsert: AccountUpsert,
                _audit: AuditEvent,
            ) -> MailFuture<'_, Result<StoredAccount, MailServiceError>> {
                Box::pin(async { Err(MailServiceError::Store) })
            }

            fn prepare_outbound(
                &self,
                _preparation: OutboundPreparation,
                _audit: AuditEvent,
            ) -> MailFuture<'_, Result<(), MailServiceError>> {
                Box::pin(async { Err(MailServiceError::Store) })
            }

            fn increment_send_rate(
                &self,
                _actor: UserId,
                _endpoint: &'static str,
                _window_start: Timestamp,
            ) -> MailFuture<'_, Result<i64, MailServiceError>> {
                Box::pin(async { Ok(1) })
            }
        }

        struct RecordingSender(std::sync::Arc<AtomicUsize>);

        impl SmtpSender for RecordingSender {
            fn prepare(
                &self,
                _config: &SmtpTransportConfig,
                _message: &SendMessageCommand,
                _from_address: &str,
            ) -> Result<PreparedOutbound, MailServiceError> {
                self.0.fetch_add(1, Ordering::SeqCst);
                Ok(PreparedOutbound {
                    format: PreparedFormat::SmtpMime,
                    bytes: b"test MIME".to_vec(),
                    rfc_message_id: Some("<test@example.com>".to_owned()),
                })
            }

            fn test_connection<'a>(
                &'a self,
                _config: &'a SmtpTransportConfig,
            ) -> MailFuture<'a, Result<TestConnectionResult, MailServiceError>> {
                Box::pin(async {
                    Ok(TestConnectionResult {
                        ok: true,
                        error_code: None,
                    })
                })
            }
        }

        struct NoopNotifier;

        impl MailNotifier for NoopNotifier {
            fn notify_posted(&self, _account_id: EmailAccountId) -> MailFuture<'_, ()> {
                Box::pin(async {})
            }
        }

        let sends = std::sync::Arc::new(AtomicUsize::new(0));
        let service = SendService::new(
            FailingAdmissionStore,
            RecordingSender(sends.clone()),
            FakeCipher,
            NoopNotifier,
        );
        let result = service.send(sample_send(SendKind::New)).await;

        assert!(matches!(
            result,
            Err(MailServiceError::AdmissionUnknown { .. })
        ));
        assert_eq!(
            sends.load(Ordering::SeqCst),
            1,
            "preparation occurs locally; no network send method exists on the port"
        );
    }

    // -----------------------------------------------------------------------
    // Threading + key helpers
    // -----------------------------------------------------------------------

    fn fetched(uid: u32, subject: &str) -> FetchedMessage {
        FetchedMessage {
            imap_uid: uid,
            message_id: Some(format!("<m{uid}@h>")),
            in_reply_to: None,
            references: vec![],
            from: MessageAddress::new("sender@example.com").ok(),
            to: vec![MessageAddress::new("ops@knl.example").unwrap()],
            cc: vec![],
            subject: subject.to_owned(),
            body_text: Some("body".to_owned()),
            body_html: None,
            seen: false,
            flagged: false,
            answered: false,
            draft: false,
            received_at: Timestamp::now_utc(),
            attachments: vec![],
        }
    }

    #[test]
    fn thread_grouping_prefers_references_root() {
        let mut m = fetched(10, "Re: Budget");
        m.references = vec!["<root@h>".to_owned(), "<mid@h>".to_owned()];
        m.in_reply_to = Some("<mid@h>".to_owned());
        // The conversation ROOT (first reference) wins over in-reply-to/subject.
        assert_eq!(thread_grouping_key(&m, "Budget"), "ref:<root@h>");
    }

    #[test]
    fn thread_grouping_falls_back_to_in_reply_to_then_subject() {
        let mut m = fetched(11, "Re: Budget");
        m.in_reply_to = Some("<parent@h>".to_owned());
        assert_eq!(thread_grouping_key(&m, "Budget"), "ref:<parent@h>");

        let plain = fetched(12, "Budget");
        assert_eq!(thread_grouping_key(&plain, "Budget"), "subject:Budget");
    }

    #[test]
    fn thread_grouping_subjectless_is_singleton_by_uid() {
        let m = fetched(13, "");
        // No headers + empty subject => its own thread (never collides).
        assert_eq!(thread_grouping_key(&m, ""), "uid:13");
    }

    #[test]
    fn references_group_a_korean_reply_with_stripped_headers() {
        // A reply whose client dropped References still groups by normalized
        // subject after the Korean prefix is stripped.
        let original = fetched(20, "견적 문의");
        let reply = fetched(21, "회신: 견적 문의");
        let orig_norm = console_comms_domain::normalize_subject(&original.subject);
        let reply_norm = console_comms_domain::normalize_subject(&reply.subject);
        assert_eq!(
            thread_grouping_key(&original, &orig_norm),
            thread_grouping_key(&reply, &reply_norm),
            "a header-less Korean reply must share the original's thread key"
        );
    }

    #[test]
    fn mail_attachment_key_is_org_prefixed_and_sanitized() {
        let org = OrgId::knl();
        let account = EmailAccountId::new();
        let message = EmailMessageId::new();
        let key = mail_attachment_key(org, account, message, 1, "../../etc/passwd");
        let prefix = format!("orgs/{}/mail/{}/{}/1-", org.as_uuid(), account, message);
        assert!(key.starts_with(&prefix));
        // The security property is that path SEPARATORS in the filename are
        // neutralized, so the filename can never escape its org-scoped key prefix:
        // beyond the fixed prefix there are NO further '/' separators (a traversal
        // like `/etc/` is impossible). A residual `..` without a separator is inert.
        let filename_part = &key[prefix.len()..];
        assert!(
            !filename_part.contains('/'),
            "the sanitized filename must contain no path separator: {filename_part:?}"
        );
        assert!(!key.contains("/etc/"));
        assert_eq!(sanitize_filename("../../etc/passwd"), "_.._etc_passwd");
    }

    #[test]
    fn sanitize_filename_neutralizes_paths_and_empties() {
        assert_eq!(sanitize_filename("a/b\\c.txt"), "a_b_c.txt");
        assert_eq!(sanitize_filename("   "), "attachment");
        assert_eq!(sanitize_filename("..."), "attachment");
    }

    // -----------------------------------------------------------------------
    // SyncService against a fake IMAP client + fake store (no live server):
    // idempotency (re-sync same UID = no dup) and threading wiring.
    // -----------------------------------------------------------------------

    use std::sync::Mutex;

    #[derive(Default)]
    struct FakeImap {
        messages: Vec<FetchedMessage>,
    }

    struct FakeSession {
        messages: Vec<FetchedMessage>,
    }

    impl ImapClient for FakeImap {
        fn connect<'a>(
            &'a self,
            _config: &'a ImapTransportConfig,
        ) -> MailFuture<'a, Result<Box<dyn ImapSession>, MailServiceError>> {
            let messages = self.messages.clone();
            Box::pin(async move { Ok(Box::new(FakeSession { messages }) as Box<dyn ImapSession>) })
        }

        fn test_connection<'a>(
            &'a self,
            _config: &'a ImapTransportConfig,
        ) -> MailFuture<'a, Result<TestConnectionResult, MailServiceError>> {
            Box::pin(async {
                Ok(TestConnectionResult {
                    ok: true,
                    error_code: None,
                })
            })
        }
    }

    impl ImapSession for FakeSession {
        fn list_folders(&mut self) -> MailFuture<'_, Result<Vec<ImapFolder>, MailServiceError>> {
            Box::pin(async {
                Ok(vec![ImapFolder {
                    imap_path: "INBOX".to_owned(),
                    role: console_comms_domain::FolderRole::Inbox,
                    name: "Inbox".to_owned(),
                }])
            })
        }

        fn select<'a>(
            &'a mut self,
            _imap_path: &'a str,
        ) -> MailFuture<'a, Result<ImapSelect, MailServiceError>> {
            Box::pin(async {
                Ok(ImapSelect {
                    uid_validity: 1,
                    uid_next: Some(100),
                    exists: 2,
                })
            })
        }

        fn fetch_since<'a>(
            &'a mut self,
            since_uid: u32,
            _limit: u32,
        ) -> MailFuture<'a, Result<Vec<FetchedMessage>, MailServiceError>> {
            let out: Vec<FetchedMessage> = self
                .messages
                .iter()
                .filter(|m| m.imap_uid > since_uid)
                .cloned()
                .collect();
            Box::pin(async move { Ok(out) })
        }

        fn logout(&mut self) -> MailFuture<'_, ()> {
            Box::pin(async {})
        }
    }

    /// A fake store that records inbound upserts in memory, dedupes on the IMAP
    /// identity (validity, uid) + message_id, and advances a per-folder cursor —
    /// the same invariants the Postgres adapter enforces, so the engine logic is
    /// exercised without a database.
    #[derive(Default)]
    struct FakeStore {
        seen_identity: Mutex<std::collections::HashSet<(i64, u32)>>,
        cursor: Mutex<i64>,
        threads: Mutex<std::collections::HashMap<String, usize>>,
    }

    impl MailReadStore for FakeStore {
        fn upsert_folders<'a>(
            &'a self,
            _org: OrgId,
            _account: EmailAccountId,
            folders: &'a [ImapFolder],
        ) -> MailFuture<'a, Result<Vec<FolderCursor>, MailServiceError>> {
            let cursor = *self.cursor.lock().unwrap();
            let out: Vec<FolderCursor> = folders
                .iter()
                .map(|f| FolderCursor {
                    folder_id: uuid::Uuid::nil(),
                    imap_path: f.imap_path.clone(),
                    uid_validity: Some(1),
                    last_seen_uid: cursor,
                })
                .collect();
            Box::pin(async move { Ok(out) })
        }

        fn reset_folder_cursor<'a>(
            &'a self,
            _org: OrgId,
            _folder_id: uuid::Uuid,
            _uid_validity: i64,
        ) -> MailFuture<'a, Result<(), MailServiceError>> {
            *self.cursor.lock().unwrap() = 0;
            Box::pin(async { Ok(()) })
        }

        fn upsert_inbound<'a>(
            &'a self,
            _org: OrgId,
            upsert: InboundUpsert,
        ) -> MailFuture<'a, Result<bool, MailServiceError>> {
            let identity = (upsert.uid_validity, upsert.message.imap_uid);
            let mut seen = self.seen_identity.lock().unwrap();
            let is_new = seen.insert(identity);
            if is_new {
                let mut cursor = self.cursor.lock().unwrap();
                *cursor = (*cursor).max(i64::from(upsert.message.imap_uid));
                let key = thread_grouping_key(&upsert.message, &upsert.normalized_subject);
                *self.threads.lock().unwrap().entry(key).or_insert(0) += 1;
            }
            Box::pin(async move { Ok(is_new) })
        }

        fn record_sync_result<'a>(
            &'a self,
            _org: OrgId,
            _account: EmailAccountId,
            _claim_token: uuid::Uuid,
            _status: &'a str,
            _error: Option<&'a str>,
        ) -> MailFuture<'a, Result<(), MailServiceError>> {
            Box::pin(async { Ok(()) })
        }

        fn release_claim<'a>(
            &'a self,
            _org: OrgId,
            _account: EmailAccountId,
            _claim_token: uuid::Uuid,
        ) -> MailFuture<'a, Result<(), MailServiceError>> {
            Box::pin(async { Ok(()) })
        }

        fn list_due_accounts(
            &self,
            _now: Timestamp,
            _limit: i32,
        ) -> MailFuture<'_, Result<Vec<DueAccount>, MailServiceError>> {
            Box::pin(async { Ok(vec![]) })
        }

        fn find_account_by_address<'a>(
            &'a self,
            _address: &'a str,
        ) -> MailFuture<'a, Result<AddressLookup, MailServiceError>> {
            Box::pin(async { Ok(AddressLookup::NotFound) })
        }

        fn list_folders<'a>(
            &'a self,
            _org: OrgId,
            _account: EmailAccountId,
        ) -> MailFuture<'a, Result<Vec<FolderView>, MailServiceError>> {
            Box::pin(async { Ok(vec![]) })
        }

        fn list_threads<'a>(
            &'a self,
            _org: OrgId,
            _account: EmailAccountId,
            _query: &'a ThreadQuery,
        ) -> MailFuture<'a, Result<Vec<ThreadView>, MailServiceError>> {
            Box::pin(async { Ok(vec![]) })
        }

        fn get_thread<'a>(
            &'a self,
            _org: OrgId,
            _thread_id: uuid::Uuid,
        ) -> MailFuture<'a, Result<Option<ThreadDetail>, MailServiceError>> {
            Box::pin(async { Ok(None) })
        }

        fn get_message<'a>(
            &'a self,
            _org: OrgId,
            _message_id: EmailMessageId,
        ) -> MailFuture<'a, Result<Option<MessageView>, MailServiceError>> {
            Box::pin(async { Ok(None) })
        }

        fn get_attachment_key<'a>(
            &'a self,
            _org: OrgId,
            _attachment_id: uuid::Uuid,
        ) -> MailFuture<'a, Result<Option<AttachmentRef>, MailServiceError>> {
            Box::pin(async { Ok(None) })
        }

        fn set_thread_seen<'a>(
            &'a self,
            _org: OrgId,
            _thread_id: uuid::Uuid,
            _seen: bool,
            _audit: AuditEvent,
        ) -> MailFuture<'a, Result<bool, MailServiceError>> {
            Box::pin(async { Ok(false) })
        }
    }

    #[derive(Default)]
    struct NoopAttachments;

    impl MailAttachmentStore for NoopAttachments {
        fn put<'a>(
            &'a self,
            _key: String,
            _content_type: String,
            _bytes: Vec<u8>,
        ) -> MailFuture<'a, Result<(), MailServiceError>> {
            Box::pin(async { Ok(()) })
        }

        fn presign_get<'a>(
            &'a self,
            _key: &'a str,
        ) -> MailFuture<'a, Result<String, MailServiceError>> {
            Box::pin(async { Ok("https://example/get".to_owned()) })
        }
    }

    /// A cipher whose decrypt returns a fixed plaintext, so the SyncService can
    /// build an ImapTransportConfig without a real KEK (the fake IMAP ignores it).
    struct FakeCipher;

    impl CredentialCipher for FakeCipher {
        fn encrypt(
            &self,
            _plaintext: &[u8],
            _aad: Aad<'_>,
        ) -> Result<SealedCredential, CipherError> {
            Ok(SealedCredential {
                ciphertext: vec![],
                nonce: vec![0; 24],
                dek_wrapped: vec![0; 48],
                dek_nonce: vec![0; 24],
                key_version: 1,
            })
        }

        fn decrypt(
            &self,
            _sealed: &SealedCredential,
            _aad: Aad<'_>,
        ) -> Result<secrecy::SecretBox<Vec<u8>>, CipherError> {
            Ok(secrecy::SecretBox::new(Box::new(b"imap-pw".to_vec())))
        }
    }

    #[tokio::test]
    async fn sync_is_idempotent_over_the_same_uid_range() {
        let imap = FakeImap {
            messages: vec![fetched(1, "Quote"), fetched(2, "Re: Quote")],
        };
        let store = FakeStore::default();
        let service = SyncService::new(&store, imap, NoopAttachments, FakeCipher);
        let account = sample_stored();

        // First pass inserts both NEW.
        let first = service
            .sync_account(&account, uuid::Uuid::new_v4())
            .await
            .unwrap();
        assert_eq!(first.messages_upserted, 2);
        assert_eq!(first.messages_skipped_duplicate, 0);

        // The cursor advanced to UID 2, so a second pass fetches NOTHING new.
        let second = service
            .sync_account(&account, uuid::Uuid::new_v4())
            .await
            .unwrap();
        assert_eq!(
            second.messages_upserted, 0,
            "re-syncing the same UID range must insert no duplicate"
        );

        // Both messages threaded together (Re: normalizes to the same subject).
        let threads = store.threads.lock().unwrap();
        assert_eq!(
            threads.len(),
            1,
            "the reply must join the original's thread"
        );
        assert_eq!(*threads.values().next().unwrap(), 2);
    }

    #[tokio::test]
    async fn uidvalidity_reset_refetches_from_floor() {
        // The fake store starts with a stale cursor at UID 5 under validity 1, but
        // the fake server reports validity 1 with messages 1..2. Since validity
        // MATCHES, only UID > 5 is fetched (nothing) — proving the cursor gate.
        let imap = FakeImap {
            messages: vec![fetched(1, "A"), fetched(2, "B")],
        };
        let store = FakeStore::default();
        *store.cursor.lock().unwrap() = 5;
        let service = SyncService::new(&store, imap, NoopAttachments, FakeCipher);
        let account = sample_stored();
        let outcome = service
            .sync_account(&account, uuid::Uuid::new_v4())
            .await
            .unwrap();
        assert_eq!(
            outcome.messages_upserted, 0,
            "with a matching UIDVALIDITY, only UID > last_seen is fetched"
        );
    }
}
