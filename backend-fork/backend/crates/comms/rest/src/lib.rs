//! Webmail REST API.
//!
//! Endpoints (all under `/api/v1/mail`, tenant-scoped, audited):
//!   * `GET  /mail/account`        — the configured mailbox (write-only password)
//!   * `PUT  /mail/account`        — configure/replace the mailbox
//!   * `POST /mail/account/test`   — SMTP test-connection (structured result)
//!   * `POST /mail/send`           — prepare an immutable outbound operation
//!   * `POST /mail/reply`          — prepare reply (In-Reply-To/References)
//!   * `POST /mail/forward`        — prepare forward (In-Reply-To/References)
//!
//! AuthZ: `MailAccountManage` gates account config + test; `MailUse` gates send/
//! reply/forward. Every config change and preparation is audited in the store.
//!
//! # Graceful missing key
//!
//! The master KEK ([`console_comms_credential_cipher`]) is OPTIONAL at boot. When it
//! is absent the router still mounts (so the paths exist for the OpenAPI gate):
//! read-only mailbox endpoints degrade to a clean no-account/empty state, while
//! credential-using endpoints fail closed with `503 email_not_configured`.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

mod openapi;
pub use openapi::OPENAPI_FRAGMENT;

use std::str::FromStr;
use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::header::AUTHORIZATION;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, patch, post};
use axum::{Json, Router};
use console_comms_adapter_imap::AsyncImapClient;
use console_comms_adapter_mox::{Incoming, MoxWebapiSender};
use console_comms_adapter_postgres::{PgMailNotifier, PgMailStore};
use console_comms_adapter_smtp::LettreMailSender;
use console_comms_application::{
    AccountService, AccountView, AddressLookup, ConfigureAccountCommand, EmailMessageId,
    FolderView, ImapFolder, InboundUpsert, MailAttachmentStore, MailFuture, MailNotifier,
    MailReadStore, MailServiceError, MessageView, PreparedOutbound, SendKind, SendMessageCommand,
    SendService, SmtpSender, SmtpTransportConfig, StoredAccount, SyncService, TestConnectionResult,
    ThreadDetail, ThreadQuery, ThreadView,
};
use console_comms_credential_cipher::EnvelopeCredentialCipher;
use console_comms_domain::{FolderRole, MailSecurity, MessageAddress, normalize_subject};
use console_kernel_core::{ErrorKind, KernelError, OrgId, TraceContext};
use console_platform_auth::JwtVerifier;
use console_platform_authz::{Action, Feature, Principal, authorize_capability};
use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};
use subtle::ConstantTimeEq;
use time::OffsetDateTime;

pub const MAIL_ACCOUNT_PATH: &str = "/api/v1/mail/account";
pub const MAIL_ACCOUNT_TEST_PATH: &str = "/api/v1/mail/account/test";
pub const MAIL_SEND_PATH: &str = "/api/v1/mail/send";
pub const MAIL_REPLY_PATH: &str = "/api/v1/mail/reply";
pub const MAIL_FORWARD_PATH: &str = "/api/v1/mail/forward";
pub const MAIL_FOLDERS_PATH: &str = "/api/v1/mail/folders";
pub const MAIL_THREADS_PATH: &str = "/api/v1/mail/threads";
pub const MAIL_THREAD_PATH: &str = "/api/v1/mail/threads/{id}";
pub const MAIL_THREAD_READ_STATE_PATH: &str = "/api/v1/mail/threads/{id}/read-state";
pub const MAIL_MESSAGE_PATH: &str = "/api/v1/mail/messages/{id}";
pub const MAIL_ATTACHMENT_DOWNLOAD_PATH: &str = "/api/v1/mail/attachments/{id}/download";
pub const COMMS_ROUTE_PATHS: &[&str] = &[
    MAIL_ACCOUNT_PATH,
    MAIL_ACCOUNT_TEST_PATH,
    MAIL_SEND_PATH,
    MAIL_REPLY_PATH,
    MAIL_FORWARD_PATH,
    MAIL_FOLDERS_PATH,
    MAIL_THREADS_PATH,
    MAIL_THREAD_PATH,
    MAIL_THREAD_READ_STATE_PATH,
    MAIL_MESSAGE_PATH,
    MAIL_ATTACHMENT_DOWNLOAD_PATH,
];

/// The mox delivery webhook receiver. NOT a client/console API — mox (our own
/// mail server) POSTs incoming-delivery events here, authenticated by a shared
/// secret, so it is deliberately absent from the customer OpenAPI + SDK clients.
pub const MAIL_MOX_WEBHOOK_PATH: &str = "/api/v1/mail/mox/webhook";

/// The outbound transport behind the [`SmtpSender`] port. `Smtp` is the original
/// lettre client to an external account; `Mox` rides our own mox server's webapi.
/// Selection is by config/env at router construction (see
/// [`CommsRestState::with_mox_transport`]).
#[derive(Clone)]
enum MailSender {
    Smtp(LettreMailSender),
    Mox(MoxWebapiSender),
}

impl SmtpSender for MailSender {
    fn prepare(
        &self,
        config: &SmtpTransportConfig,
        message: &SendMessageCommand,
        from_address: &str,
    ) -> Result<PreparedOutbound, MailServiceError> {
        match self {
            Self::Smtp(s) => s.prepare(config, message, from_address),
            Self::Mox(m) => m.prepare(config, message, from_address),
        }
    }

    fn test_connection<'a>(
        &'a self,
        config: &'a SmtpTransportConfig,
    ) -> MailFuture<'a, Result<TestConnectionResult, MailServiceError>> {
        match self {
            Self::Smtp(s) => s.test_connection(config),
            Self::Mox(m) => m.test_connection(config),
        }
    }
}

/// The shared attachment-store handle (presigned GET for inbound attachments).
pub type SharedAttachmentStore = Arc<dyn MailAttachmentStore>;

/// A no-op realtime notifier: outbound persistence never depends on it, and the
/// realtime channel for inbound mail lands in B-mail-5.
#[derive(Debug, Default, Clone)]
struct NoopMailNotifier;

impl console_comms_application::MailNotifier for NoopMailNotifier {
    fn notify_posted(
        &self,
        _account_id: console_comms_application::EmailAccountId,
    ) -> console_comms_application::MailFuture<'_, ()> {
        Box::pin(async {})
    }
}

#[derive(Clone)]
pub struct CommsRestState {
    store: PgMailStore,
    sender: MailSender,
    imap: AsyncImapClient,
    /// The master-key cipher. `None` when `CONSOLE_MAIL_MASTER_KEY` is absent —
    /// credential-using endpoints are then unavailable (503) but the app still boots.
    cipher: Option<Arc<EnvelopeCredentialCipher>>,
    /// The object store for inbound attachment presigned GETs. `None` when
    /// storage is unconfigured — the attachment-download endpoint then 503s.
    attachments: Option<SharedAttachmentStore>,
    /// The shared secret mox must present (as `Authorization: Bearer <secret>`)
    /// on the delivery webhook. `None` disables the webhook (503) — it is never
    /// hardcoded and never defaulted.
    mox_webhook_secret: Option<SecretString>,
    jwt_verifier: Option<JwtVerifier>,
}

impl CommsRestState {
    #[must_use]
    pub fn new(
        store: PgMailStore,
        cipher: Option<Arc<EnvelopeCredentialCipher>>,
        jwt_verifier: Option<JwtVerifier>,
    ) -> Self {
        Self {
            store,
            sender: MailSender::Smtp(LettreMailSender::new()),
            imap: AsyncImapClient::new(),
            cipher,
            attachments: None,
            mox_webhook_secret: None,
            jwt_verifier,
        }
    }

    /// Attach the object store backing inbound attachment presigned GETs.
    #[must_use]
    pub fn with_attachments(mut self, attachments: Option<SharedAttachmentStore>) -> Self {
        self.attachments = attachments;
        self
    }

    /// Select the mox webapi as the outbound transport when `base_url` is set
    /// (e.g. `http://mox:1080`). Absent → keep the default lettre SMTP client.
    #[must_use]
    pub fn with_mox_transport(mut self, base_url: Option<String>) -> Self {
        if let Some(base) = base_url.filter(|b| !b.trim().is_empty()) {
            self.sender = MailSender::Mox(MoxWebapiSender::new(base));
        }
        self
    }

    /// Set the shared secret that authenticates the mox delivery webhook. A
    /// blank/whitespace value is treated as unset (the webhook then 503s).
    #[must_use]
    pub fn with_mox_webhook_secret(mut self, secret: Option<String>) -> Self {
        self.mox_webhook_secret = secret
            .filter(|s| !s.trim().is_empty())
            .map(SecretString::from);
        self
    }

    fn pool(&self) -> &sqlx::PgPool {
        self.store.pool()
    }
}

pub fn router(state: CommsRestState) -> Router {
    let verifier = state.jwt_verifier.clone();
    let pool = state.pool().clone();
    // JWT-authed console API. Wrapped in the request-context layer, which REQUIRES
    // a valid tenant bearer on every route it covers.
    let authed = Router::new()
        .route(MAIL_ACCOUNT_PATH, get(get_account).put(put_account))
        .route(MAIL_ACCOUNT_TEST_PATH, post(test_account))
        .route(MAIL_SEND_PATH, post(send_new))
        .route(MAIL_REPLY_PATH, post(send_reply))
        .route(MAIL_FORWARD_PATH, post(send_forward))
        .route(MAIL_FOLDERS_PATH, get(list_folders))
        .route(MAIL_THREADS_PATH, get(list_threads))
        .route(MAIL_THREAD_PATH, get(get_thread))
        .route(MAIL_THREAD_READ_STATE_PATH, patch(set_thread_read_state))
        .route(MAIL_MESSAGE_PATH, get(get_message))
        .route(MAIL_ATTACHMENT_DOWNLOAD_PATH, get(download_attachment))
        .with_state(state.clone());
    let authed = console_platform_request_context::with_request_context(authed, verifier, pool);
    // The mox delivery webhook is machine-to-machine (mox → us), authenticated by
    // its own shared secret — NOT a tenant JWT. It is merged OUTSIDE the
    // request-context layer so the layer's mandatory-bearer check never rejects it.
    let webhook = Router::new()
        .route(MAIL_MOX_WEBHOOK_PATH, post(mox_webhook))
        .with_state(state);
    authed.merge(webhook)
}

// ---------------------------------------------------------------------------
// Wire DTOs
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct ConfigureAccountRequest {
    display_name: String,
    email_address: String,
    #[serde(default)]
    from_name: Option<String>,
    imap_host: String,
    imap_port: u16,
    imap_security: MailSecurity,
    imap_username: String,
    /// Write-only: present = (re)seal; absent/null = keep the stored secret.
    #[serde(default)]
    imap_password: Option<String>,
    smtp_host: String,
    smtp_port: u16,
    smtp_security: MailSecurity,
    smtp_username: String,
    #[serde(default)]
    smtp_password: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AddressDto {
    address: String,
    #[serde(default)]
    name: Option<String>,
}

impl AddressDto {
    fn into_domain(self) -> Result<MessageAddress, RestError> {
        MessageAddress::new(self.address)
            .map(|a| a.with_name(self.name))
            .map_err(RestError::from_kernel)
    }
}

#[derive(Debug, Deserialize)]
struct SendRequest {
    operation_id: Option<uuid::Uuid>,
    to: Vec<AddressDto>,
    #[serde(default)]
    cc: Vec<AddressDto>,
    #[serde(default)]
    bcc: Vec<AddressDto>,
    subject: String,
    body_text: String,
    #[serde(default)]
    attachments: Vec<AttachmentDto>,
    /// Required for reply/forward: the Message-ID being responded to.
    #[serde(default)]
    in_reply_to: Option<String>,
    #[serde(default)]
    references: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct ThreadReadStateRequest {
    seen: bool,
}

#[derive(Debug, Deserialize)]
struct AttachmentDto {
    filename: String,
    content_type: String,
    /// Standard-base64 content.
    content_base64: String,
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

async fn get_account(
    State(state): State<CommsRestState>,
    headers: HeaderMap,
) -> Result<Response, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    authorize_feature(&principal, Feature::MailAccountManage)?;
    let view: Option<AccountView> = read_account(&state, principal.org_id)
        .await?
        .map(|account| account.to_view());
    match view {
        Some(view) => Ok(Json(view).into_response()),
        None => Ok(StatusCode::NO_CONTENT.into_response()),
    }
}

async fn put_account(
    State(state): State<CommsRestState>,
    headers: HeaderMap,
    Json(body): Json<ConfigureAccountRequest>,
) -> Result<Response, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    authorize_feature(&principal, Feature::MailAccountManage)?;
    let cipher = state.cipher_ref()?;
    let service = AccountService::new(state.store.clone(), state.sender.clone(), cipher);

    let command = ConfigureAccountCommand {
        actor: principal.user_id,
        org_id: principal.org_id,
        display_name: body.display_name,
        email_address: body.email_address,
        from_name: body.from_name,
        imap_host: body.imap_host,
        imap_port: body.imap_port,
        imap_security: body.imap_security,
        imap_username: body.imap_username,
        // A present-but-empty/whitespace password means "keep the existing
        // secret", NOT "seal an empty password": `{"smtp_password":""}` must not
        // re-seal a blank credential. Only an explicit non-empty value re-seals.
        imap_password: keep_existing_if_blank(body.imap_password),
        smtp_host: body.smtp_host,
        smtp_port: body.smtp_port,
        smtp_security: body.smtp_security,
        smtp_username: body.smtp_username,
        smtp_password: keep_existing_if_blank(body.smtp_password),
        trace: TraceContext::generate(),
        occurred_at: OffsetDateTime::now_utc(),
    };
    let view = service
        .configure(command)
        .await
        .map_err(RestError::from_service)?;
    Ok(Json(view).into_response())
}

async fn test_account(
    State(state): State<CommsRestState>,
    headers: HeaderMap,
) -> Result<Response, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    authorize_feature(&principal, Feature::MailAccountManage)?;
    let cipher = state.cipher_ref()?;
    // Probe SMTP first (rate-limited + decrypted inside the service). A failure
    // short-circuits with the SMTP error code.
    let smtp_service = AccountService::new(state.store.clone(), state.sender.clone(), cipher);
    let smtp: TestConnectionResult = smtp_service
        .test_connection(principal.user_id, OffsetDateTime::now_utc())
        .await
        .map_err(RestError::from_service)?;
    if !smtp.ok {
        return Ok(Json(smtp).into_response());
    }

    // SMTP is reachable — now actually probe IMAP (the part deferred from B-mail-2).
    // The stored IMAP password is decrypted in-memory inside the SyncService and
    // dropped after the probe; the SSRF guard + TLS enforcement apply.
    let Some(account) = read_account(&state, principal.org_id).await? else {
        return Err(RestError::validation(
            "no mailbox is configured for this tenant",
        ));
    };
    let sync = SyncService::new(
        state.store.clone(),
        state.imap.clone(),
        NoopAttachmentStore,
        cipher,
    );
    let imap: TestConnectionResult = sync
        .test_connection(&account)
        .await
        .map_err(RestError::from_service)?;
    Ok(Json(imap).into_response())
}

async fn send_new(
    State(state): State<CommsRestState>,
    headers: HeaderMap,
    Json(body): Json<SendRequest>,
) -> Result<Response, RestError> {
    send_impl(state, headers, body, SendKind::New).await
}

async fn send_reply(
    State(state): State<CommsRestState>,
    headers: HeaderMap,
    Json(body): Json<SendRequest>,
) -> Result<Response, RestError> {
    send_impl(state, headers, body, SendKind::Reply).await
}

async fn send_forward(
    State(state): State<CommsRestState>,
    headers: HeaderMap,
    Json(body): Json<SendRequest>,
) -> Result<Response, RestError> {
    send_impl(state, headers, body, SendKind::Forward).await
}

async fn send_impl(
    state: CommsRestState,
    headers: HeaderMap,
    body: SendRequest,
    kind: SendKind,
) -> Result<Response, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    authorize_feature(&principal, Feature::MailUse)?;
    let operation_id = body
        .operation_id
        .ok_or_else(|| RestError::validation("operation_id is required for mail reconciliation"))?;
    let cipher = state.cipher_ref()?;
    let service = SendService::new(
        state.store.clone(),
        state.sender.clone(),
        cipher,
        NoopMailNotifier,
    );

    let to = body
        .to
        .into_iter()
        .map(AddressDto::into_domain)
        .collect::<Result<Vec<_>, _>>()?;
    let cc = body
        .cc
        .into_iter()
        .map(AddressDto::into_domain)
        .collect::<Result<Vec<_>, _>>()?;
    let bcc = body
        .bcc
        .into_iter()
        .map(AddressDto::into_domain)
        .collect::<Result<Vec<_>, _>>()?;

    let attachments = body
        .attachments
        .into_iter()
        .map(decode_attachment)
        .collect::<Result<Vec<_>, _>>()?;

    let command = SendMessageCommand {
        operation_id,
        actor: principal.user_id,
        kind,
        to,
        cc,
        bcc,
        subject: body.subject,
        body_text: body.body_text,
        attachments,
        in_reply_to: body.in_reply_to,
        references: body.references,
        trace: TraceContext::generate(),
        occurred_at: OffsetDateTime::now_utc(),
    };
    service
        .send(command)
        .await
        .map_err(RestError::from_service)?;
    // Keep the public boundary fail-closed if a later service change starts
    // returning Ok before the fenced two-site publication path is wired here.
    Err(RestError::unavailable("outbound mail egress is not armed"))
}

// ---------------------------------------------------------------------------
// READ API (B-mail-3): folders, threads, messages, attachment download. All
// MailUse-gated and RLS-armed (the store re-arms `app.current_org` to the
// principal's org for every query).
// ---------------------------------------------------------------------------

/// Query string for the thread list: `unread`, `q` (search), `folder`, `before`
/// (keyset cursor on `last_message_at`), `limit`.
#[derive(Debug, Deserialize, Default)]
struct ThreadListQuery {
    #[serde(default)]
    unread: bool,
    #[serde(default)]
    q: Option<String>,
    #[serde(default)]
    folder: Option<String>,
    #[serde(default)]
    before: Option<i64>,
    #[serde(default)]
    limit: Option<i64>,
}

async fn list_folders(
    State(state): State<CommsRestState>,
    headers: HeaderMap,
) -> Result<Response, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    authorize_feature(&principal, Feature::MailUse)?;
    let Some(account) = read_account(&state, principal.org_id).await? else {
        return Ok(Json(Vec::<FolderView>::new()).into_response());
    };
    let folders = state
        .store
        .list_folders(principal.org_id, account.id)
        .await
        .map_err(RestError::from_service)?;
    Ok(Json(folders).into_response())
}

async fn list_threads(
    State(state): State<CommsRestState>,
    headers: HeaderMap,
    Query(params): Query<ThreadListQuery>,
) -> Result<Response, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    authorize_feature(&principal, Feature::MailUse)?;
    let Some(account) = read_account(&state, principal.org_id).await? else {
        return Ok(Json(Vec::<ThreadView>::new()).into_response());
    };
    let before = params
        .before
        .and_then(|ts| OffsetDateTime::from_unix_timestamp(ts).ok());
    let folder_id = params
        .folder
        .as_deref()
        .and_then(|s| uuid::Uuid::from_str(s).ok());
    let query = ThreadQuery {
        folder_id,
        unread_only: params.unread,
        search: params.q,
        before,
        limit: params.limit.unwrap_or(50),
    };
    let threads = state
        .store
        .list_threads(principal.org_id, account.id, &query)
        .await
        .map_err(RestError::from_service)?;
    Ok(Json(threads).into_response())
}

async fn get_thread(
    State(state): State<CommsRestState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    authorize_feature(&principal, Feature::MailUse)?;
    let thread_id =
        uuid::Uuid::from_str(&id).map_err(|_| RestError::bad_request("invalid thread id"))?;
    let detail: Option<ThreadDetail> = state
        .store
        .get_thread(principal.org_id, thread_id)
        .await
        .map_err(RestError::from_service)?;
    match detail {
        Some(detail) => Ok(Json(detail).into_response()),
        None => Err(RestError::new(
            StatusCode::NOT_FOUND,
            "not_found",
            "thread not found",
        )),
    }
}

async fn set_thread_read_state(
    State(state): State<CommsRestState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<ThreadReadStateRequest>,
) -> Result<Response, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    authorize_feature(&principal, Feature::MailUse)?;
    let thread_id =
        uuid::Uuid::from_str(&id).map_err(|_| RestError::bad_request("invalid thread id"))?;
    let detail = state
        .store
        .get_thread(principal.org_id, thread_id)
        .await
        .map_err(RestError::from_service)?
        .ok_or_else(|| RestError::new(StatusCode::NOT_FOUND, "not_found", "thread not found"))?;
    let inbound_count = detail
        .messages
        .iter()
        .filter(|message| message.direction == "IN")
        .count() as i64;
    let before_unread_count = detail
        .messages
        .iter()
        .filter(|message| message.direction == "IN" && !message.seen)
        .count() as i64;
    let after_unread_count = if body.seen { 0 } else { inbound_count };
    let audit = console_comms_application::thread_read_state_audit_event(
        principal.user_id,
        thread_id,
        before_unread_count,
        after_unread_count,
        body.seen,
        TraceContext::generate(),
        OffsetDateTime::now_utc(),
    )
    .map_err(RestError::from_kernel)?
    .with_org(principal.org_id);
    let updated = state
        .store
        .set_thread_seen(principal.org_id, thread_id, body.seen, audit)
        .await
        .map_err(RestError::from_service)?;
    if !updated {
        return Err(RestError::new(
            StatusCode::NOT_FOUND,
            "not_found",
            "thread not found",
        ));
    }
    Ok(StatusCode::NO_CONTENT.into_response())
}

async fn get_message(
    State(state): State<CommsRestState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    authorize_feature(&principal, Feature::MailUse)?;
    let message_id =
        EmailMessageId::from_str(&id).map_err(|_| RestError::bad_request("invalid message id"))?;
    let message: Option<MessageView> = state
        .store
        .get_message(principal.org_id, message_id)
        .await
        .map_err(RestError::from_service)?;
    match message {
        Some(message) => Ok(Json(message).into_response()),
        None => Err(RestError::new(
            StatusCode::NOT_FOUND,
            "not_found",
            "message not found",
        )),
    }
}

async fn download_attachment(
    State(state): State<CommsRestState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    authorize_feature(&principal, Feature::MailUse)?;
    let attachment_id =
        uuid::Uuid::from_str(&id).map_err(|_| RestError::bad_request("invalid attachment id"))?;
    let attachments = state
        .attachments
        .as_ref()
        .ok_or_else(RestError::email_not_configured)?;
    // Resolve the attachment's storage key under the armed org (cross-tenant
    // invisible: a key for another org's attachment is simply not found).
    let reference = state
        .store
        .get_attachment_key(principal.org_id, attachment_id)
        .await
        .map_err(RestError::from_service)?
        .ok_or_else(|| {
            RestError::new(StatusCode::NOT_FOUND, "not_found", "attachment not found")
        })?;
    let url = attachments
        .presign_get(&reference.s3_key)
        .await
        .map_err(RestError::from_service)?;
    // 302 to the short-lived presigned URL (the UI follows it); the raw key is
    // never exposed.
    Ok(Json(AttachmentDownload { url }).into_response())
}

/// The presigned-GET response for an attachment download.
#[derive(Debug, Serialize)]
struct AttachmentDownload {
    url: String,
}

// ---------------------------------------------------------------------------
// MOX DELIVERY WEBHOOK (inbound ingest)
//
// mox (our own mail server) POSTs an `Incoming` event here when a message is
// delivered to a local account. Authenticated by a shared secret (never a tenant
// JWT). The recipient address selects the tenant/account (owner-conn id-only
// lookup), then the message is UPSERTed into the read model under that org's RLS
// and a realtime notification is emitted. Idempotent on redelivery: the store
// dedupes on the mox MsgID and, authoritatively, on the RFC Message-ID.
// ---------------------------------------------------------------------------

/// The webhook ack. `ingested` is false when the delivery was a redelivery of an
/// already-stored message (idempotent no-op) or for an unknown local recipient.
#[derive(Debug, Serialize)]
struct WebhookAck {
    ingested: bool,
}

async fn mox_webhook(
    State(state): State<CommsRestState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, RestError> {
    // 1. Authenticate BEFORE touching the (untrusted) body. The secret must be
    //    configured; mox presents it verbatim as `Authorization: Bearer <secret>`.
    let secret = state
        .mox_webhook_secret
        .as_ref()
        .ok_or_else(|| RestError::unavailable("mox delivery webhook is not configured"))?;
    let expected = format!("Bearer {}", secret.expose_secret());
    let presented = headers
        .get(AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    if !bool::from(presented.as_bytes().ct_eq(expected.as_bytes())) {
        return Err(RestError::unauthorized("invalid mox webhook credential"));
    }

    // 2. Parse the mox Incoming payload.
    let incoming: Incoming = serde_json::from_slice(&body)
        .map_err(|_| RestError::bad_request("malformed mox webhook payload"))?;
    let Some(recipient) = incoming.recipient_address() else {
        return Err(RestError::bad_request(
            "mox webhook has no recipient address",
        ));
    };

    // 3. Resolve the tenant/account by recipient address (owner-conn, id-only).
    //    An unknown local recipient, and an AMBIGUOUS one (the address exists
    //    under more than one org — the store already wrote the anomaly audit +
    //    error log), are both ACKed (200, ingested=false) so mox does not retry
    //    forever; an ambiguous address is delivered to NONE of the matching orgs.
    let account = match state
        .store
        .find_account_by_address(&recipient)
        .await
        .map_err(RestError::from_service)?
    {
        AddressLookup::Found(account) => account,
        AddressLookup::NotFound | AddressLookup::Ambiguous => {
            return Ok(Json(WebhookAck { ingested: false }).into_response());
        }
    };
    let org = account.org_id;

    // 4. Ensure the destination folder exists (org-armed by the adapter).
    let mailbox = incoming.mailbox_name();
    let role = if mailbox.eq_ignore_ascii_case("inbox") {
        FolderRole::Inbox
    } else {
        FolderRole::Custom
    };
    let folder = ImapFolder {
        imap_path: mailbox.to_owned(),
        role,
        name: mailbox.to_owned(),
    };
    let cursor = state
        .store
        .upsert_folders(org, account.account_id, std::slice::from_ref(&folder))
        .await
        .map_err(RestError::from_service)?
        .into_iter()
        .next()
        .ok_or_else(|| RestError::internal("mox webhook: folder upsert returned no cursor"))?;

    // 5. UPSERT the message into the read model (idempotent) and, when it is a
    //    genuinely new message, fire the realtime notification.
    let upsert = InboundUpsert {
        id: EmailMessageId::new(),
        account_id: account.account_id,
        folder_id: cursor.folder_id,
        // Webhook-ingested messages have no IMAP UIDVALIDITY; 0 is the sentinel.
        // Dedupe identity is (account, folder, 0, mox MsgID) + Message-ID.
        uid_validity: 0,
        message: incoming.to_fetched_message(),
        normalized_subject: normalize_subject(&incoming.subject),
        stored_attachments: Vec::new(),
    };
    let ingested = state
        .store
        .upsert_inbound(org, upsert)
        .await
        .map_err(RestError::from_service)?;
    if ingested {
        PgMailNotifier::new(state.pool().clone())
            .notify_posted(account.account_id)
            .await;
    }
    Ok(Json(WebhookAck { ingested }).into_response())
}

/// Read the tenant's single configured mailbox as a [`StoredAccount`] (sealed
/// credentials included), org-armed. `None` when no mailbox is configured.
async fn read_account(
    state: &CommsRestState,
    org: OrgId,
) -> Result<Option<StoredAccount>, RestError> {
    use console_comms_application::MailStore;
    use console_platform_request_context::CURRENT_ORG;
    CURRENT_ORG
        .scope(org, state.store.get_account())
        .await
        .map_err(RestError::from_service)
}

/// A no-op attachment store for paths that never upload (the IMAP test-connection
/// probe builds a `SyncService` but only calls `test_connection`, which never
/// touches storage).
#[derive(Clone, Copy)]
struct NoopAttachmentStore;

impl MailAttachmentStore for NoopAttachmentStore {
    fn put<'a>(
        &'a self,
        _key: String,
        _content_type: String,
        _bytes: Vec<u8>,
    ) -> console_comms_application::MailFuture<'a, Result<(), MailServiceError>> {
        Box::pin(async { Ok(()) })
    }

    fn presign_get<'a>(
        &'a self,
        _key: &'a str,
    ) -> console_comms_application::MailFuture<'a, Result<String, MailServiceError>> {
        Box::pin(async {
            Err(MailServiceError::Transport {
                code: "presign_unavailable",
            })
        })
    }
}

/// Collapse a present-but-blank password to `None` so the write-only contract
/// treats `""`/whitespace as "keep the existing secret" rather than re-sealing an
/// empty credential. A non-empty value is passed through verbatim (NOT trimmed —
/// a password may legitimately contain leading/trailing spaces).
fn keep_existing_if_blank(value: Option<String>) -> Option<SecretString> {
    value
        .filter(|password| !password.trim().is_empty())
        .map(SecretString::from)
}

fn decode_attachment(
    dto: AttachmentDto,
) -> Result<console_comms_application::OutboundAttachment, RestError> {
    use base64::Engine as _;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(dto.content_base64.trim())
        .map_err(|_| RestError::bad_request("attachment content is not valid base64"))?;
    Ok(console_comms_application::OutboundAttachment {
        filename: dto.filename,
        content_type: dto.content_type,
        bytes,
    })
}

// ---------------------------------------------------------------------------
// AuthZ + auth extraction
// ---------------------------------------------------------------------------

impl CommsRestState {
    /// Borrow the cipher or fail with a 503 when the master key is unconfigured.
    fn cipher_ref(&self) -> Result<&EnvelopeCredentialCipher, RestError> {
        self.cipher
            .as_deref()
            .ok_or_else(RestError::email_not_configured)
    }
}

/// Mail is org-global: `email_folders`, `email_threads`, `email_messages` and
/// `email_attachments` have no `branch_id`, and every handler keys off
/// `principal.org_id` and never loads a branch. Capability decision.
///
/// TODO(branch-scope): `email_accounts.branch_id` IS present but NULLABLE
/// (migration 0053) and unused by these handlers. If account-level branch
/// confinement is ever wanted, `MailAccountManage` should thread that real
/// column into `authorize` instead of widening this check.
fn authorize_feature(principal: &Principal, feature: Feature) -> Result<(), RestError> {
    authorize_capability(principal, Action::new(feature)).map_err(RestError::from_kernel)
}

async fn principal_from_headers(
    state: &CommsRestState,
    headers: &HeaderMap,
) -> Result<Principal, RestError> {
    let verifier = state.jwt_verifier.as_ref().ok_or_else(|| {
        RestError::unavailable("JWT verification is not configured for the mail API")
    })?;
    console_platform_request_context::resolve_principal(verifier, state.pool(), headers)
        .await
        .map_err(rest_error_from_request_context)
}

fn rest_error_from_request_context(
    err: console_platform_request_context::RequestContextError,
) -> RestError {
    match err {
        console_platform_request_context::RequestContextError::VerifierUnavailable => {
            RestError::unavailable("JWT verification is not configured for the mail API")
        }
        console_platform_request_context::RequestContextError::WrongTokenTier => {
            RestError::forbidden("token tier is not valid for this route")
        }
        console_platform_request_context::RequestContextError::AccessScope(error) => {
            RestError::from_kernel(error)
        }
        console_platform_request_context::RequestContextError::BranchScope(message)
        | console_platform_request_context::RequestContextError::EffectivePolicy(message) => {
            RestError::internal(message)
        }
        console_platform_request_context::RequestContextError::MissingOrg => {
            RestError::internal("no tenant context is bound to the current request")
        }
        console_platform_request_context::RequestContextError::MissingBearer => {
            RestError::unauthorized("missing or malformed bearer token")
        }
        console_platform_request_context::RequestContextError::InvalidToken => {
            RestError::unauthorized("invalid bearer token")
        }
        console_platform_request_context::RequestContextError::InvalidClaim(message) => {
            RestError::unauthorized(format!("token claim is invalid: {message}"))
        }
    }
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
struct ErrorBody {
    error: ErrorPayload,
}

#[derive(Debug, Serialize)]
struct ErrorPayload {
    code: &'static str,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    operation_id: Option<uuid::Uuid>,
}

#[derive(Debug)]
struct RestError {
    status: StatusCode,
    code: &'static str,
    message: String,
    operation_id: Option<uuid::Uuid>,
}

impl RestError {
    fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
            operation_id: None,
        }
    }

    fn bad_request(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, "bad_request", message)
    }

    fn unauthorized(message: impl Into<String>) -> Self {
        Self::new(StatusCode::UNAUTHORIZED, "unauthorized", message)
    }

    fn forbidden(message: impl Into<String>) -> Self {
        Self::new(StatusCode::FORBIDDEN, "forbidden", message)
    }

    fn validation(message: impl Into<String>) -> Self {
        Self::new(StatusCode::UNPROCESSABLE_ENTITY, "validation", message)
    }

    fn unavailable(message: impl Into<String>) -> Self {
        Self::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "service_unavailable",
            message,
        )
    }

    /// The outbound rate limit was hit (per-org + per-user). A 429 with a fixed,
    /// non-secret message; the bucket code rides in the message.
    fn too_many_requests(code: &'static str) -> Self {
        Self::new(
            StatusCode::TOO_MANY_REQUESTS,
            "too_many_requests",
            format!("outbound mail rate limit exceeded ({code}); retry later"),
        )
    }

    fn internal(message: impl Into<String>) -> Self {
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, "internal", message)
    }

    /// The graceful missing-key surface: the master key is absent, so the email
    /// feature is unavailable. A clear, non-secret 503.
    fn email_not_configured() -> Self {
        Self::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "email_not_configured",
            "email is not configured on this server",
        )
    }

    fn from_kernel(error: KernelError) -> Self {
        match error.kind {
            ErrorKind::Validation => Self::validation(error.message),
            ErrorKind::Forbidden => Self::forbidden(error.message),
            ErrorKind::NotFound => Self::new(StatusCode::NOT_FOUND, "not_found", error.message),
            ErrorKind::Conflict | ErrorKind::InvalidTransition => {
                Self::new(StatusCode::CONFLICT, "conflict", error.message)
            }
            ErrorKind::Internal => Self::internal("internal server error"),
        }
    }

    fn from_service(error: MailServiceError) -> Self {
        match error {
            MailServiceError::Domain(kernel) => Self::from_kernel(kernel),
            MailServiceError::NotConfigured => {
                Self::validation("no mailbox is configured for this tenant")
            }
            // Transport failures carry a fixed, non-secret code; surface it as a
            // 422 so the caller can correct host/credentials without us leaking
            // the raw transport string.
            MailServiceError::Transport { code } => {
                Self::new(StatusCode::UNPROCESSABLE_ENTITY, "mail_transport", code)
            }
            // The per-org + per-user outbound rate limit tripped before any SMTP
            // call. A 429 so clients back off.
            MailServiceError::RateLimited { code } => Self::too_many_requests(code),
            MailServiceError::OperationConflict => Self::new(
                StatusCode::CONFLICT,
                "mail_operation_conflict",
                "operation_id was already used for different mail content",
            ),
            MailServiceError::AdmissionUnknown { operation_id } => Self {
                operation_id: Some(operation_id),
                ..Self::new(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "mail_admission_unknown",
                    "mail was not sent; durable admission is unconfirmed",
                )
            },
            // Store/cipher errors are server-internal: log nothing secret, return
            // a generic 500.
            other => {
                tracing::error!(kind = ?other.kind(), "mail service error");
                Self::internal("internal server error")
            }
        }
    }
}

impl IntoResponse for RestError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(ErrorBody {
                error: ErrorPayload {
                    code: self.code,
                    message: self.message,
                    operation_id: self.operation_id,
                },
            }),
        )
            .into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::ConstantTimeEq;
    use super::keep_existing_if_blank;
    use secrecy::ExposeSecret;

    // -------------------------------------------------------------------
    // Webhook credential comparison: the length-XOR-truncated-to-u8
    // hand-roll this replaced would silently mask a length delta that is a
    // multiple of 256 (`(a.len() ^ b.len()) as u8 == 0` for such deltas).
    // The 256-length-delta vector proves the `subtle`-backed check catches
    // exactly that case: a 256-byte-longer presented value must still be
    // rejected.
    // -------------------------------------------------------------------
    #[test]
    fn credential_eq_rejects_256_length_delta() {
        let expected = b"Bearer test-mox-webhook-secret".to_vec();
        let mut presented = expected.clone();
        presented.extend(std::iter::repeat_n(b'x', 256));
        assert!(!bool::from(presented.as_slice().ct_eq(expected.as_slice())));
    }

    #[test]
    fn credential_eq_accepts_exact_match() {
        let secret = b"Bearer test-mox-webhook-secret".to_vec();
        assert!(bool::from(secret.as_slice().ct_eq(secret.as_slice())));
    }

    #[test]
    fn credential_eq_rejects_wrong_value_same_length() {
        let expected = b"Bearer test-mox-webhook-secret".to_vec();
        let mut presented = expected.clone();
        *presented.last_mut().unwrap() ^= 0x01;
        assert!(!bool::from(presented.as_slice().ct_eq(expected.as_slice())));
    }

    #[test]
    fn blank_password_keeps_existing_secret() {
        // L1: a present-but-empty/whitespace password must collapse to None so the
        // write-only contract treats `{"smtp_password":""}` as "keep the existing
        // secret", NOT "re-seal an empty credential".
        assert!(keep_existing_if_blank(None).is_none());
        assert!(keep_existing_if_blank(Some(String::new())).is_none());
        assert!(keep_existing_if_blank(Some("   ".to_owned())).is_none());
        assert!(keep_existing_if_blank(Some("\t\n ".to_owned())).is_none());
    }

    #[test]
    fn non_empty_password_is_passed_through_verbatim() {
        // A real password re-seals — and is NOT trimmed (a password may carry
        // significant leading/trailing whitespace).
        let sealed = keep_existing_if_blank(Some(" s3cret ".to_owned()))
            .expect("a non-blank password must seal");
        assert_eq!(sealed.expose_secret(), " s3cret ");
        let plain = keep_existing_if_blank(Some("hunter2".to_owned())).unwrap();
        assert_eq!(plain.expose_secret(), "hunter2");
    }
}
