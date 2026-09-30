//! In-console office document sessions (ONLYOFFICE / Euro-Office, slice 0).
//!
//! HANDOFF §12: the console HOST owns storage, versions, PBAC, audit, approval;
//! the editor is only the canvas. This module implements the host side of the
//! ONLYOFFICE DocumentServer integration:
//!
//!   * `POST /api/v1/office/sessions` — issue a signed DocumentServer editor
//!     config for the latest version of a document (permissions mapped from the
//!     caller's real authz; `document.key` = a per-version hash so the editor
//!     never serves a stale cache).
//!   * `POST /api/v1/office/callback` — the machine force-save callback. JWT
//!     verified (the shared HS256 secret), status 2/6 fetches the produced
//!     document and stores it as an IMMUTABLE new version. Idempotent per
//!     editing-session key. No user principal — a signed callback token binds
//!     the request to (org, document) so it can never write cross-tenant.
//!   * `GET  /api/v1/office/documents/{documentRef}/versions` — version list.
//!   * `POST /api/v1/office/documents/{documentRef}/versions/{versionNo}/restore`
//!     — non-destructive rollback: re-publish an old version as a NEW version.
//!
//! The FORK modifications (per-edit audit hooks, covert section render-blocking,
//! DLP, real-time co-edit presence, AP- approval integration) are a LATER epic
//! and are deliberately NOT implemented here — see the PR body.
//!
//! # Graceful missing config
//!
//! Like the mail router, the office routes always mount (so the OpenAPI paths
//! exist) but degrade to `503 office_not_configured` when the shared JWT secret
//! or the object store is absent at boot.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

use std::collections::HashSet;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration as StdDuration;

use axum::extract::{Extension, Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use console_kernel_core::{
    AuditAction, AuditEvent, ErrorKind, KernelError, OrgId, TraceContext, UserId,
};
use console_platform_authz::{Action, Feature, Principal, authorize_org_wide};
use console_platform_db::{DbError, with_audit, with_audits, with_org_conn};
use console_platform_storage::{PresignGetRequest, S3ObjectStore, SeaweedS3Storage};
use jsonwebtoken::{Algorithm, DecodingKey, EncodingKey, Header, Validation, decode, encode};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Postgres, Row, Transaction};
use time::OffsetDateTime;
use uuid::Uuid;

pub const OFFICE_SESSIONS_PATH: &str = "/api/v1/office/sessions";
pub const OFFICE_CALLBACK_PATH: &str = "/api/v1/office/callback";
pub const OFFICE_VERSIONS_PATH: &str = "/api/v1/office/documents/{documentRef}/versions";
pub const OFFICE_RESTORE_PATH: &str =
    "/api/v1/office/documents/{documentRef}/versions/{versionNo}/restore";

/// Route paths asserted present in `openapi.yaml` by the drift test.
pub const OFFICE_ROUTE_PATHS: &[&str] = &[
    OFFICE_SESSIONS_PATH,
    OFFICE_CALLBACK_PATH,
    OFFICE_VERSIONS_PATH,
    OFFICE_RESTORE_PATH,
];

/// Presigned-GET / editor-config lifetime.
const PRESIGN_TTL: StdDuration = StdDuration::from_secs(60 * 60);
/// Callback-token lifetime: the editing session may stay open a while, and
/// DocumentServer force-saves ~10s after the last editor leaves.
const CALLBACK_TOKEN_TTL_SECS: i64 = 24 * 60 * 60;

/// Clock-skew tolerance applied ONLY to the ONLYOFFICE-signed callback
/// payload's `exp` (when present) — DocumentServer and the host may not have
/// perfectly synced clocks. The host-issued callback token below deliberately
/// keeps zero leeway (it is our own clock on both ends).
const CALLBACK_CLAIMS_LEEWAY_SECS: u64 = 30;

/// Maximum logical document-reference length accepted by API and DB.
const DOCUMENT_REF_MAX_CHARS: usize = 200;
/// Maximum produced document bytes fetched from DocumentServer on callback.
const PRODUCED_DOCUMENT_MAX_BYTES: usize = 50 * 1024 * 1024;
/// Connection timeout for fetching force-saved documents from DocumentServer.
const DOCSERVER_CONNECT_TIMEOUT: StdDuration = StdDuration::from_secs(5);
/// End-to-end timeout for fetching force-saved documents from DocumentServer.
const DOCSERVER_REQUEST_TIMEOUT: StdDuration = StdDuration::from_secs(30);

/// Document formats slice 0 accepts (ONLYOFFICE word/cell/slide editors).
const ALLOWED_FILE_TYPES: &[&str] = &["docx", "xlsx", "pptx"];

// ===========================================================================
// Config + state
// ===========================================================================

/// Office integration config. Present only when all of `CONSOLE_OFFICE_JWT_SECRET`,
/// `CONSOLE_OFFICE_CALLBACK_BASE_URL`, and `CONSOLE_OFFICE_DOCSERVER_URL` are set.
#[derive(Debug, Clone)]
pub struct OfficeConfig {
    /// Shared HS256 secret — identical to the DocumentServer JWT secret. Signs
    /// the editor config + callback token, and verifies the inbound callback.
    pub jwt_secret: String,
    /// Public base URL DocumentServer uses to reach the host callback (in dev,
    /// `http://host.docker.internal:8090`). Becomes `editorConfig.callbackUrl`.
    pub callback_base_url: String,
    /// DocumentServer origin the browser loads `api.js` from — and the host
    /// allowlist for the callback-produced document URL (SSRF guard).
    pub docserver_url: String,
}

/// Parse [`OfficeConfig`] from env. Returns `None` (office disabled) unless all
/// three vars are present, and errors only if a present value is malformed.
pub fn office_config_from_vars(
    get: impl Fn(&str) -> Option<String>,
) -> Result<Option<OfficeConfig>, String> {
    let secret = get("CONSOLE_OFFICE_JWT_SECRET").filter(|s| !s.trim().is_empty());
    let callback_base_url =
        get("CONSOLE_OFFICE_CALLBACK_BASE_URL").filter(|s| !s.trim().is_empty());
    let docserver_url = get("CONSOLE_OFFICE_DOCSERVER_URL").filter(|s| !s.trim().is_empty());
    match (secret, callback_base_url, docserver_url) {
        (Some(jwt_secret), Some(callback_base_url), Some(docserver_url)) => {
            // Validate the docserver URL up front so the SSRF host allowlist has
            // a real host to compare against.
            url::Url::parse(&docserver_url)
                .ok()
                .and_then(|u| u.host_str().map(str::to_owned))
                .ok_or_else(|| "CONSOLE_OFFICE_DOCSERVER_URL must be an absolute URL".to_owned())?;
            Ok(Some(OfficeConfig {
                jwt_secret,
                callback_base_url: callback_base_url.trim_end_matches('/').to_owned(),
                docserver_url,
            }))
        }
        (None, None, None) => Ok(None),
        _ => Err(
            "office editor needs all of CONSOLE_OFFICE_JWT_SECRET, CONSOLE_OFFICE_CALLBACK_BASE_URL, \
             CONSOLE_OFFICE_DOCSERVER_URL, or none"
                .to_owned(),
        ),
    }
}

#[derive(Clone)]
pub struct OfficeState {
    pool: PgPool,
    jwt_verifier: Option<console_platform_auth::JwtVerifier>,
    config: Option<OfficeConfig>,
    blobs: Option<Arc<dyn OfficeBlobStore>>,
}

impl OfficeState {
    #[must_use]
    pub fn new(
        pool: PgPool,
        jwt_verifier: Option<console_platform_auth::JwtVerifier>,
        config: Option<OfficeConfig>,
        blobs: Option<Arc<dyn OfficeBlobStore>>,
    ) -> Self {
        Self {
            pool,
            jwt_verifier,
            config,
            blobs,
        }
    }

    /// Build the real object-store-backed blob port from the already-configured
    /// SeaweedFS handle used for evidence/sales media.
    #[must_use]
    pub fn seaweed_blobs(
        config: &OfficeConfig,
        store: SeaweedS3Storage,
        bucket: String,
    ) -> Option<Arc<dyn OfficeBlobStore>> {
        let docserver_origin = url::Url::parse(&config.docserver_url).ok()?;
        // Never follow redirects on the callback-produced-document fetch: a
        // same-origin URL that 30x's to an internal host would otherwise let a
        // forged (but same-origin) redirect target bypass the SSRF guard below.
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(DOCSERVER_CONNECT_TIMEOUT)
            .timeout(DOCSERVER_REQUEST_TIMEOUT)
            .build()
            .ok()?;
        Some(Arc::new(SeaweedOfficeBlobStore {
            store,
            bucket,
            http,
            docserver_origin,
        }))
    }
}

pub fn router(state: OfficeState) -> Router {
    let verifier = state.jwt_verifier.clone();
    let pool = state.pool.clone();
    // Authenticated (user principal) surface — session issuance, version list,
    // restore. Wrapped in the per-request org middleware exactly like every
    // other domain router.
    let authed = Router::new()
        .route(OFFICE_SESSIONS_PATH, post(create_session))
        .route(OFFICE_VERSIONS_PATH, get(list_versions_handler))
        .route(OFFICE_RESTORE_PATH, post(restore_handler))
        .with_state(state.clone());
    let authed = console_platform_request_context::with_request_context(authed, verifier, pool);

    // The DocumentServer force-save callback is a MACHINE request with no user
    // token, so it must NOT go through the principal middleware (which is
    // fail-closed 401). It is authenticated by the ONLYOFFICE JWT plus the
    // host-issued callback token instead.
    let callback = Router::new()
        .route(OFFICE_CALLBACK_PATH, post(callback_handler))
        .with_state(state);

    authed.merge(callback)
}

// ===========================================================================
// Blob port (the only external-IO boundary; stubbable in tests)
// ===========================================================================

pub type OfficeFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, OfficeError>> + Send + 'a>>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredBlob {
    pub content_hash: String,
    pub byte_size: i64,
}

/// The object-storage boundary the office domain leans on. Real impl fetches the
/// DocumentServer-produced document over HTTP and persists it to SeaweedFS; the
/// full editor round-trip therefore needs the running container + object store
/// (CI cannot exercise it — see `tests/office_versions.rs`). The pure version
/// domain + JWT logic are testable without it.
pub trait OfficeBlobStore: Send + Sync {
    /// Fetch the produced document from DocumentServer and persist it immutably
    /// at `dest_key`, returning its content hash + size.
    fn store_from_url(&self, source_url: String, dest_key: String) -> OfficeFuture<'_, StoredBlob>;
    /// Short-lived presigned GET so the browser editor can load a version's
    /// bytes without ever seeing the raw object key.
    fn presign_get(&self, key: String) -> OfficeFuture<'_, String>;
}

struct SeaweedOfficeBlobStore {
    store: SeaweedS3Storage,
    bucket: String,
    http: reqwest::Client,
    /// The DocumentServer origin (scheme + host + port) the callback URL MUST
    /// match — SSRF guard so a forged (but somehow valid-token) callback
    /// cannot make us fetch an arbitrary internal URL. `host_str()` alone is
    /// not enough: a same-host-different-port URL can reach an unrelated
    /// internal service DocumentServer never serves from.
    docserver_origin: url::Url,
}

/// Same-origin check (scheme + host + port, default-port-aware) used as the
/// SSRF guard on the callback-produced-document URL.
fn same_origin(a: &url::Url, b: &url::Url) -> bool {
    a.scheme() == b.scheme()
        && a.host_str() == b.host_str()
        && a.port_or_known_default() == b.port_or_known_default()
}

impl OfficeBlobStore for SeaweedOfficeBlobStore {
    fn store_from_url(&self, source_url: String, dest_key: String) -> OfficeFuture<'_, StoredBlob> {
        Box::pin(async move {
            let parsed = url::Url::parse(&source_url)
                .map_err(|_| OfficeError::validation("callback document url is not a valid URL"))?;
            if !same_origin(&parsed, &self.docserver_origin) {
                return Err(OfficeError::validation(
                    "callback document url origin does not match the configured DocumentServer",
                ));
            }
            // Belt-and-suspenders: the client is built with redirects
            // disabled (see `seaweed_blobs`), so a 3xx here is treated as a
            // failure rather than silently followed off-origin.
            let response = self
                .http
                .get(parsed)
                .send()
                .await
                .map_err(|e| OfficeError::storage(format!("fetch produced document: {e}")))?;
            if !response.status().is_success() {
                return Err(OfficeError::storage(format!(
                    "DocumentServer returned {} for the produced document",
                    response.status()
                )));
            }
            if response
                .content_length()
                .is_some_and(|len| len > PRODUCED_DOCUMENT_MAX_BYTES as u64)
            {
                return Err(OfficeError::validation(
                    "produced document exceeds the maximum allowed size",
                ));
            }
            let mut response = response;
            let mut bytes = Vec::new();
            while let Some(chunk) = response
                .chunk()
                .await
                .map_err(|e| OfficeError::storage(format!("read produced document: {e}")))?
            {
                let next_len = bytes.len().checked_add(chunk.len()).ok_or_else(|| {
                    OfficeError::validation("produced document exceeds the maximum allowed size")
                })?;
                if next_len > PRODUCED_DOCUMENT_MAX_BYTES {
                    return Err(OfficeError::validation(
                        "produced document exceeds the maximum allowed size",
                    ));
                }
                bytes.extend_from_slice(&chunk);
            }
            let content_hash = sha256_hex(&bytes);
            let byte_size = i64::try_from(bytes.len()).unwrap_or(i64::MAX);
            self.store
                .put_bytes(
                    &self.bucket,
                    &dest_key,
                    "application/octet-stream",
                    bytes.to_vec(),
                )
                .await
                .map_err(|e| OfficeError::storage(format!("persist produced document: {e}")))?;
            Ok(StoredBlob {
                content_hash,
                byte_size,
            })
        })
    }

    fn presign_get(&self, key: String) -> OfficeFuture<'_, String> {
        Box::pin(async move {
            self.store
                .presign_get(PresignGetRequest {
                    bucket: self.bucket.clone(),
                    key,
                    expires_in: PRESIGN_TTL,
                })
                .await
                .map_err(|e| OfficeError::storage(format!("presign document url: {e}")))
        })
    }
}

// ===========================================================================
// Version domain (pure DB, RLS-armed, audited)
// ===========================================================================

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DocumentVersion {
    pub id: Uuid,
    pub document_ref: String,
    pub version_no: i32,
    pub content_hash: String,
    pub file_type: String,
    pub byte_size: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub restored_from: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_by: Option<Uuid>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    // storage_key + source_key are internal; never serialized to clients.
    #[serde(skip)]
    pub storage_key: String,
}

/// Inputs for appending an immutable version.
#[derive(Debug, Clone)]
pub struct NewVersion {
    pub org: OrgId,
    /// `None` = system-initiated (the machine force-save callback).
    pub actor: Option<UserId>,
    pub document_ref: String,
    pub file_type: String,
    pub storage_key: String,
    pub content_hash: String,
    pub byte_size: i64,
    /// The ONLYOFFICE editing-session key that produced this version (callback
    /// path only). Drives idempotency.
    pub source_key: Option<String>,
    pub restored_from: Option<i32>,
}

fn version_from_row(row: &sqlx::postgres::PgRow) -> Result<DocumentVersion, OfficeError> {
    Ok(DocumentVersion {
        id: row.try_get("id")?,
        document_ref: row.try_get("document_ref")?,
        version_no: row.try_get("version_no")?,
        content_hash: row.try_get("content_hash")?,
        file_type: row.try_get("file_type")?,
        byte_size: row.try_get("byte_size")?,
        restored_from: row.try_get("restored_from")?,
        created_by: row.try_get("created_by")?,
        created_at: row.try_get("created_at")?,
        storage_key: row.try_get("storage_key")?,
    })
}

/// The latest version of a document within an already-open transaction, or
/// `None` if it has none. Shared by [`latest_version`] (plain read) and
/// [`issue_session_version`] (audited read, inside `with_audits`).
async fn latest_version_tx(
    tx: &mut Transaction<'_, Postgres>,
    document_ref: &str,
) -> Result<Option<DocumentVersion>, OfficeError> {
    let row = sqlx::query(concat!(
        "SELECT id, document_ref, version_no, content_hash, file_type, byte_size, ",
        "restored_from, created_by, created_at, storage_key FROM document_versions ",
        "WHERE document_ref = $1 ORDER BY version_no DESC LIMIT 1"
    ))
    .bind(document_ref)
    .fetch_optional(tx.as_mut())
    .await?;
    row.as_ref().map(version_from_row).transpose()
}

/// The latest version of a document, or `None` if it has none.
pub async fn latest_version(
    pool: &PgPool,
    org: OrgId,
    document_ref: &str,
) -> Result<Option<DocumentVersion>, OfficeError> {
    let document_ref = normalize_document_ref(document_ref)?;
    with_org_conn::<_, _, OfficeError>(pool, org, move |tx| {
        Box::pin(async move { latest_version_tx(tx, &document_ref).await })
    })
    .await
}

/// Fetch the latest version AND record the sensitive "editor session issued"
/// grant as an audit event, atomically (same pattern the person-view read
/// audit uses — the read and its audit trail commit or roll back together).
/// This is what backs `POST /office/sessions`: issuing an edit-capable,
/// presigned session is a grant worth an audit trail even though it mutates
/// no document row.
pub async fn issue_session_version(
    pool: &PgPool,
    org: OrgId,
    actor: UserId,
    document_ref: &str,
) -> Result<DocumentVersion, OfficeError> {
    let document_ref_owned = normalize_document_ref(document_ref)?;
    with_audits::<_, DocumentVersion, OfficeError>(pool, org, move |tx| {
        Box::pin(async move {
            let latest = latest_version_tx(tx, &document_ref_owned)
                .await?
                .ok_or_else(|| {
                    OfficeError::not_found(
                        "document has no versions; create it via the records module first",
                    )
                })?;
            let event = AuditEvent::new(
                Some(actor),
                AuditAction::new("office.session.issue")?,
                "document_version",
                latest.id.to_string(),
                TraceContext::generate(),
                OffsetDateTime::now_utc(),
            )
            .with_org(org)
            .with_snapshots(
                None,
                Some(json!({
                    "documentRef": latest.document_ref,
                    "versionNo": latest.version_no,
                    "capability": "edit",
                })),
            );
            Ok((latest, vec![event]))
        })
    })
    .await
}

/// All versions of a document, newest first.
pub async fn list_versions(
    pool: &PgPool,
    org: OrgId,
    document_ref: &str,
) -> Result<Vec<DocumentVersion>, OfficeError> {
    let document_ref = normalize_document_ref(document_ref)?;
    with_org_conn::<_, _, OfficeError>(pool, org, move |tx| {
        Box::pin(async move {
            let rows = sqlx::query(concat!(
                "SELECT id, document_ref, version_no, content_hash, file_type, byte_size, ",
                "restored_from, created_by, created_at, storage_key FROM document_versions ",
                "WHERE document_ref = $1 ORDER BY version_no DESC"
            ))
            .bind(&document_ref)
            .fetch_all(tx.as_mut())
            .await?;
            rows.iter().map(version_from_row).collect()
        })
    })
    .await
}

async fn version_by_no(
    pool: &PgPool,
    org: OrgId,
    document_ref: &str,
    version_no: i32,
) -> Result<Option<DocumentVersion>, OfficeError> {
    let document_ref = normalize_document_ref(document_ref)?;
    with_org_conn::<_, _, OfficeError>(pool, org, move |tx| {
        Box::pin(async move {
            let row = sqlx::query(concat!(
                "SELECT id, document_ref, version_no, content_hash, file_type, byte_size, ",
                "restored_from, created_by, created_at, storage_key FROM document_versions ",
                "WHERE document_ref = $1 AND version_no = $2"
            ))
            .bind(&document_ref)
            .bind(version_no)
            .fetch_optional(tx.as_mut())
            .await?;
            row.as_ref().map(version_from_row).transpose()
        })
    })
    .await
}

async fn version_by_source_key(
    pool: &PgPool,
    org: OrgId,
    source_key: &str,
) -> Result<Option<DocumentVersion>, OfficeError> {
    let source_key = source_key.to_owned();
    with_org_conn::<_, _, OfficeError>(pool, org, move |tx| {
        Box::pin(async move {
            let row = sqlx::query(concat!(
                "SELECT id, document_ref, version_no, content_hash, file_type, byte_size, ",
                "restored_from, created_by, created_at, storage_key FROM document_versions ",
                "WHERE source_key = $1"
            ))
            .bind(&source_key)
            .fetch_optional(tx.as_mut())
            .await?;
            row.as_ref().map(version_from_row).transpose()
        })
    })
    .await
}

/// Append an immutable version. Computes the next `version_no` inside the audited
/// transaction, so the row + its audit event commit atomically. Idempotent per
/// `(org, source_key)`: a retried callback returns the already-stored version
/// without appending a duplicate.
pub async fn record_version(
    pool: &PgPool,
    mut new: NewVersion,
) -> Result<DocumentVersion, OfficeError> {
    new.document_ref = normalize_document_ref(&new.document_ref)?;
    validate_file_type(&new.file_type)?;

    // Idempotency short-circuit (no audit noise on a plain replay). The partial
    // unique index is still the authoritative guard against a concurrent racer.
    if let Some(source_key) = new.source_key.as_deref()
        && let Some(existing) = version_by_source_key(pool, new.org, source_key).await?
    {
        return Ok(existing);
    }

    let action = if new.restored_from.is_some() {
        "office.document_version.restore"
    } else {
        "office.document_version.record"
    };
    let version_id = Uuid::new_v4();
    let event = AuditEvent::new(
        new.actor,
        AuditAction::new(action)?,
        "document_version",
        version_id.to_string(),
        TraceContext::generate(),
        OffsetDateTime::now_utc(),
    )
    .with_org(new.org)
    .with_snapshots(
        None,
        Some(json!({
            "documentRef": new.document_ref,
            "restoredFrom": new.restored_from,
        })),
    );

    let org_uuid = *new.org.as_uuid();
    let insert = new.clone();
    let result = with_audit::<_, DocumentVersion, OfficeError>(pool, event, move |tx| {
        Box::pin(async move {
            let next_no: i32 = sqlx::query_scalar(
                "SELECT COALESCE(MAX(version_no), 0) + 1 FROM document_versions \
                 WHERE org_id = $1 AND document_ref = $2",
            )
            .bind(org_uuid)
            .bind(&insert.document_ref)
            .fetch_one(tx.as_mut())
            .await?;
            let row = sqlx::query(concat!(
                "INSERT INTO document_versions ",
                "(id, org_id, document_ref, version_no, content_hash, storage_key, file_type, ",
                "byte_size, source_key, restored_from, created_by) ",
                "VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11) ",
                "RETURNING id, document_ref, version_no, content_hash, file_type, byte_size, ",
                "restored_from, created_by, created_at, storage_key"
            ))
            .bind(version_id)
            .bind(org_uuid)
            .bind(&insert.document_ref)
            .bind(next_no)
            .bind(&insert.content_hash)
            .bind(&insert.storage_key)
            .bind(&insert.file_type)
            .bind(insert.byte_size)
            .bind(insert.source_key.as_deref())
            .bind(insert.restored_from)
            .bind(insert.actor.map(|a| *a.as_uuid()))
            .fetch_one(tx.as_mut())
            .await?;
            version_from_row(&row)
        })
    })
    .await;

    // A racing callback replay lost the insert race on the source-key unique
    // index — resolve to the winner rather than surfacing a 500.
    match result {
        Err(OfficeError {
            retryable_conflict: true,
            ..
        }) => {
            if let Some(source_key) = new.source_key.as_deref()
                && let Some(existing) = version_by_source_key(pool, new.org, source_key).await?
            {
                return Ok(existing);
            }
            Err(OfficeError::internal(
                "version insert conflicted with no resolvable winner",
            ))
        }
        other => other,
    }
}

/// Non-destructive rollback: re-publish `version_no` as a NEW version. The blob
/// is immutable and shared, so the new version simply points at the same
/// `storage_key` + hash with `restored_from` set. Audited.
pub async fn restore_version(
    pool: &PgPool,
    org: OrgId,
    actor: UserId,
    document_ref: &str,
    version_no: i32,
) -> Result<DocumentVersion, OfficeError> {
    let document_ref = normalize_document_ref(document_ref)?;
    let target = version_by_no(pool, org, &document_ref, version_no)
        .await?
        .ok_or_else(|| OfficeError::not_found("no such document version to restore"))?;
    record_version(
        pool,
        NewVersion {
            org,
            actor: Some(actor),
            document_ref,
            file_type: target.file_type,
            storage_key: target.storage_key,
            content_hash: target.content_hash,
            byte_size: target.byte_size,
            source_key: None,
            restored_from: Some(version_no),
        },
    )
    .await
}

fn normalize_document_ref(document_ref: &str) -> Result<String, OfficeError> {
    let normalized = document_ref.trim();
    if normalized.is_empty() {
        return Err(OfficeError::validation("documentRef is required"));
    }
    if normalized.chars().count() > DOCUMENT_REF_MAX_CHARS {
        return Err(OfficeError::validation(format!(
            "documentRef must be at most {DOCUMENT_REF_MAX_CHARS} characters"
        )));
    }
    if normalized.chars().any(char::is_control) {
        return Err(OfficeError::validation(
            "documentRef must not contain control characters",
        ));
    }
    Ok(normalized.to_owned())
}

fn validate_file_type(file_type: &str) -> Result<(), OfficeError> {
    if ALLOWED_FILE_TYPES.contains(&file_type) {
        Ok(())
    } else {
        Err(OfficeError::validation(format!(
            "unsupported fileType {file_type:?} (expected one of {ALLOWED_FILE_TYPES:?})"
        )))
    }
}

// ===========================================================================
// JWT (HS256, the shared DocumentServer secret)
// ===========================================================================

/// Host-issued token embedded in the callbackUrl. Binds a callback to exactly
/// one (org, document): because the office secret is per-DEPLOYMENT (shared
/// across tenants), the org/document must come from a token WE signed, never a
/// mutable query param — otherwise a valid callback for tenant A could be
/// replayed against tenant B.
#[derive(Debug, Serialize, Deserialize)]
struct CallbackToken {
    org: Uuid,
    doc: String,
    exp: i64,
}

/// The ONLYOFFICE callback body claims (the editor signs the whole body; v7.1+
/// puts the token in `token`, falling back to the Authorization header).
#[derive(Debug, Deserialize)]
struct CallbackClaims {
    status: i64,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    key: Option<String>,
    #[serde(default)]
    filetype: Option<String>,
}

fn sign_hs256<T: Serialize>(secret: &str, claims: &T) -> Result<String, OfficeError> {
    encode(
        &Header::new(Algorithm::HS256),
        claims,
        &EncodingKey::from_secret(secret.as_bytes()),
    )
    .map_err(|e| OfficeError::internal(format!("sign token: {e}")))
}

fn verify_hs256<T: DeserializeOwned>(
    secret: &str,
    token: &str,
    validate_exp: bool,
    leeway_secs: u64,
) -> Result<T, OfficeError> {
    let mut validation = Validation::new(Algorithm::HS256);
    validation.validate_exp = validate_exp;
    validation.validate_aud = false;
    validation.leeway = leeway_secs;
    // `exp` is validated when present (jsonwebtoken checks the raw claim
    // independent of T) but never REQUIRED — ONLYOFFICE's payload otherwise
    // carries no standard registered claims.
    validation.required_spec_claims = HashSet::new();
    decode::<T>(
        token,
        &DecodingKey::from_secret(secret.as_bytes()),
        &validation,
    )
    .map(|data| data.claims)
    .map_err(|_| OfficeError {
        status: StatusCode::UNAUTHORIZED,
        code: "invalid_token",
        message: "token signature verification failed".to_owned(),
        retryable_conflict: false,
    })
}

/// Derive a stable, editor-safe `document.key`. A NEW version ⇒ a new key ⇒ the
/// editor never serves a stale cache. ≤128 chars, `[A-Za-z0-9._-]`.
fn document_key(org: OrgId, document_ref: &str, version_no: i32) -> String {
    let mut hasher = Sha256::new();
    hasher.update(org.as_uuid().as_bytes());
    hasher.update(document_ref.as_bytes());
    let digest = hex_short(&hasher.finalize(), 16);
    format!("{digest}-v{version_no}")
}

fn office_storage_key(org: OrgId, document_ref: &str, file_type: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(document_ref.as_bytes());
    let doc_hash = hex_short(&hasher.finalize(), 12);
    format!(
        "office/{}/{}/{}.{}",
        org.as_uuid(),
        doc_hash,
        Uuid::new_v4(),
        file_type
    )
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    hex_short(&digest, digest.len())
}

fn hex_short(bytes: &[u8], take: usize) -> String {
    bytes
        .iter()
        .take(take)
        .map(|b| format!("{b:02x}"))
        .collect()
}

// ===========================================================================
// Handlers
// ===========================================================================

fn authorize_office_edit(principal: &Principal) -> Result<(), OfficeError> {
    // Slice 0 gates the whole office surface on the records-management tier
    // (LifecycleManage — ADMIN + SUPER_ADMIN), the closest existing capability
    // for host-governed document editing. Per-permission Cedar mapping
    // (edit/review/comment/download/print separately) is DEFERRED (HANDOFF §12).
    authorize_org_wide(principal, Action::new(Feature::LifecycleManage)).map_err(OfficeError::from)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateSessionRequest {
    document_ref: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SessionResponse {
    /// Where the browser loads DocumentServer `api.js` from.
    document_server_url: String,
    /// The ONLYOFFICE editor config (already carrying its signed `token`).
    config: serde_json::Value,
}

async fn create_session(
    State(state): State<OfficeState>,
    Extension(principal): Extension<Principal>,
    Json(request): Json<CreateSessionRequest>,
) -> Result<Json<SessionResponse>, OfficeError> {
    authorize_office_edit(&principal)?;
    let config = state.config()?;
    let blobs = state.blobs()?;

    let document_ref = normalize_document_ref(&request.document_ref)?;

    // Slice 0 opens an EXISTING document (initial-version creation is a
    // records-module concern, DEFERRED — see the PR body). Issuing an
    // edit-capable, presigned session is a sensitive grant, so the fetch is
    // audited atomically via `issue_session_version` (`office.session.issue`).
    let latest = issue_session_version(
        &state.pool,
        principal.org_id,
        principal.user_id,
        &document_ref,
    )
    .await?;

    let key = document_key(principal.org_id, &document_ref, latest.version_no);
    let url = blobs.presign_get(latest.storage_key.clone()).await?;

    let callback_token = sign_hs256(
        &config.jwt_secret,
        &CallbackToken {
            org: *principal.org_id.as_uuid(),
            doc: document_ref.clone(),
            exp: OffsetDateTime::now_utc().unix_timestamp() + CALLBACK_TOKEN_TTL_SECS,
        },
    )?;
    let callback_url = format!(
        "{}{}?ct={}",
        config.callback_base_url, OFFICE_CALLBACK_PATH, callback_token
    );

    // The config the editor consumes. `permissions` are all-true for a
    // LifecycleManage holder in slice 0 (the deny path is: no capability ⇒ 403
    // before we get here); granular Cedar-mapped permissions are DEFERRED.
    let mut config_payload = json!({
        "document": {
            "key": key,
            "title": format!("{document_ref}.{}", latest.file_type),
            "url": url,
            "fileType": latest.file_type,
            "permissions": {
                "edit": true,
                "review": true,
                "comment": true,
                "download": true,
                "print": true,
            },
        },
        "editorConfig": {
            "mode": "edit",
            "callbackUrl": callback_url,
            "user": {
                "id": principal.user_id.as_uuid().to_string(),
                "name": principal.user_id.as_uuid().to_string(),
            },
        },
    });
    let token = sign_hs256(&config.jwt_secret, &config_payload)?;
    if let Some(object) = config_payload.as_object_mut() {
        object.insert("token".to_owned(), json!(token));
    }

    Ok(Json(SessionResponse {
        document_server_url: config.docserver_url.clone(),
        config: config_payload,
    }))
}

#[derive(Debug, Deserialize)]
struct CallbackQuery {
    /// Host-issued callback token binding this request to (org, document).
    ct: String,
}

/// The raw callback body. `token` carries the ONLYOFFICE-signed payload (v7.1+
/// in-body default). We ALWAYS verify from that signed token, never the raw
/// body fields.
#[derive(Debug, Deserialize)]
struct CallbackBody {
    #[serde(default)]
    token: Option<String>,
}

async fn callback_handler(
    State(state): State<OfficeState>,
    Query(query): Query<CallbackQuery>,
    headers: HeaderMap,
    Json(body): Json<CallbackBody>,
) -> Response {
    match handle_callback(&state, &query, &headers, &body).await {
        Ok(()) => Json(json!({ "error": 0 })).into_response(),
        Err(err) => {
            // The ONLYOFFICE contract wants a JSON `error` body, not an HTTP
            // status, so DocumentServer surfaces the failure to the user and
            // retries. We keep the diagnostic in the log, not the wire.
            tracing::warn!(error = %err.message, code = err.code, "office callback rejected");
            Json(json!({ "error": 1 })).into_response()
        }
    }
}

async fn handle_callback(
    state: &OfficeState,
    query: &CallbackQuery,
    headers: &HeaderMap,
    body: &CallbackBody,
) -> Result<(), OfficeError> {
    let config = state.config()?;

    // 1. Tenant binding: org + document come from OUR signed token, exp-checked
    // with NO clock-skew grace (both ends are our own clock).
    let callback_token: CallbackToken = verify_hs256(&config.jwt_secret, &query.ct, true, 0)?;
    let org = OrgId::from_uuid(callback_token.org);
    let document_ref = normalize_document_ref(&callback_token.doc)?;

    // 2. Authenticity: the ONLYOFFICE-signed payload (body `token`, else the
    // Authorization: Bearer header). We use the VERIFIED claims for status/url.
    // `exp`, if DocumentServer sets one, is enforced (small clock-skew leeway)
    // so a captured callback token cannot be replayed indefinitely.
    let signed = body
        .token
        .clone()
        .or_else(|| bearer_token(headers))
        .ok_or_else(|| OfficeError::unauthorized("callback carries no ONLYOFFICE token"))?;
    let claims: CallbackClaims = verify_hs256(
        &config.jwt_secret,
        &signed,
        true,
        CALLBACK_CLAIMS_LEEWAY_SECS,
    )?;

    // status 2 = ready for saving, 6 = force-saved while editing. Anything else
    // (1 editing, 4 closed no-change, 3/7 errors) is a benign no-op for us.
    if claims.status != 2 && claims.status != 6 {
        return Ok(());
    }

    let source_key = claims
        .key
        .ok_or_else(|| OfficeError::validation("save callback is missing the document key"))?;
    let source_url = claims
        .url
        .ok_or_else(|| OfficeError::validation("save callback is missing the document url"))?;

    // File type: the callback's own filetype, else the current latest version's.
    let file_type = match claims.filetype {
        Some(ft) if ALLOWED_FILE_TYPES.contains(&ft.as_str()) => ft,
        _ => latest_version(&state.pool, org, &document_ref)
            .await?
            .map(|v| v.file_type)
            .unwrap_or_else(|| "docx".to_owned()),
    };

    let blobs = state.blobs()?;
    let dest_key = office_storage_key(org, &document_ref, &file_type);
    let stored = blobs.store_from_url(source_url, dest_key.clone()).await?;

    record_version(
        &state.pool,
        NewVersion {
            org,
            actor: None,
            document_ref,
            file_type,
            storage_key: dest_key,
            content_hash: stored.content_hash,
            byte_size: stored.byte_size,
            source_key: Some(source_key),
            restored_from: None,
        },
    )
    .await?;
    Ok(())
}

fn bearer_token(headers: &HeaderMap) -> Option<String> {
    headers
        .get(axum::http::header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
        .map(str::to_owned)
}

#[derive(Debug, Serialize)]
struct VersionListResponse {
    items: Vec<DocumentVersion>,
}

async fn list_versions_handler(
    State(state): State<OfficeState>,
    Extension(principal): Extension<Principal>,
    Path(document_ref): Path<String>,
) -> Result<Json<VersionListResponse>, OfficeError> {
    authorize_office_edit(&principal)?;
    let items = list_versions(&state.pool, principal.org_id, &document_ref).await?;
    Ok(Json(VersionListResponse { items }))
}

async fn restore_handler(
    State(state): State<OfficeState>,
    Extension(principal): Extension<Principal>,
    Path((document_ref, version_no)): Path<(String, i32)>,
) -> Result<Json<DocumentVersion>, OfficeError> {
    authorize_office_edit(&principal)?;
    let restored = restore_version(
        &state.pool,
        principal.org_id,
        principal.user_id,
        &document_ref,
        version_no,
    )
    .await?;
    Ok(Json(restored))
}

impl OfficeState {
    fn config(&self) -> Result<&OfficeConfig, OfficeError> {
        self.config
            .as_ref()
            .ok_or_else(|| OfficeError::unavailable("office editor is not configured"))
    }

    fn blobs(&self) -> Result<Arc<dyn OfficeBlobStore>, OfficeError> {
        self.blobs
            .clone()
            .ok_or_else(|| OfficeError::unavailable("office document storage is not configured"))
    }
}

// ===========================================================================
// Error
// ===========================================================================

#[derive(Debug)]
pub struct OfficeError {
    status: StatusCode,
    code: &'static str,
    message: String,
    /// Set when the failure was a unique-constraint conflict on the callback
    /// source-key index, so `record_version` can resolve to the winning row.
    retryable_conflict: bool,
}

impl OfficeError {
    fn from_kernel(error: KernelError) -> Self {
        let (status, code) = match error.kind {
            ErrorKind::Validation => (StatusCode::UNPROCESSABLE_ENTITY, "validation"),
            ErrorKind::NotFound => (StatusCode::NOT_FOUND, "not_found"),
            ErrorKind::Forbidden => (StatusCode::FORBIDDEN, "forbidden"),
            ErrorKind::Conflict => (StatusCode::CONFLICT, "conflict"),
            ErrorKind::InvalidTransition => (StatusCode::CONFLICT, "invalid_transition"),
            ErrorKind::Internal => (StatusCode::INTERNAL_SERVER_ERROR, "internal"),
        };
        Self {
            status,
            code,
            message: error.message,
            retryable_conflict: false,
        }
    }

    fn validation(message: impl Into<String>) -> Self {
        Self::from_kernel(KernelError::validation(message.into()))
    }

    fn not_found(message: impl Into<String>) -> Self {
        Self::from_kernel(KernelError::not_found(message.into()))
    }

    fn unavailable(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::SERVICE_UNAVAILABLE,
            code: "office_not_configured",
            message: message.into(),
            retryable_conflict: false,
        }
    }

    fn unauthorized(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::UNAUTHORIZED,
            code: "invalid_token",
            message: message.into(),
            retryable_conflict: false,
        }
    }

    fn storage(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_GATEWAY,
            code: "storage",
            message: message.into(),
            retryable_conflict: false,
        }
    }

    fn internal(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            code: "internal",
            message: message.into(),
            retryable_conflict: false,
        }
    }
}

impl From<KernelError> for OfficeError {
    fn from(error: KernelError) -> Self {
        Self::from_kernel(error)
    }
}

impl From<DbError> for OfficeError {
    fn from(value: DbError) -> Self {
        // A unique violation on the callback source-key index is an expected,
        // recoverable race — flag it so record_version resolves the winner.
        if let DbError::Sqlx(sqlx::Error::Database(db)) = &value
            && db.code().as_deref() == Some("23505")
        {
            return Self {
                status: StatusCode::CONFLICT,
                code: "conflict",
                message: "version conflict".to_owned(),
                retryable_conflict: true,
            };
        }
        tracing::error!(error = %value, "office database operation failed");
        Self::internal("office request failed")
    }
}

impl From<sqlx::Error> for OfficeError {
    fn from(value: sqlx::Error) -> Self {
        Self::from(DbError::Sqlx(value))
    }
}

impl IntoResponse for OfficeError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(json!({ "error": { "code": self.code, "message": self.message } })),
        )
            .into_response()
    }
}

// ===========================================================================
// Unit tests — JWT + key derivation (no DB / no object store)
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    const SECRET: &str = "test-office-shared-secret";

    #[cfg(not(feature = "test-postgres"))]
    #[test]
    fn callback_token_round_trips_and_rejects_tamper() {
        let token = sign_hs256(
            SECRET,
            &CallbackToken {
                org: Uuid::from_u128(1),
                doc: "DOC-1".to_owned(),
                exp: OffsetDateTime::now_utc().unix_timestamp() + 60,
            },
        )
        .unwrap();

        let claims: CallbackToken = verify_hs256(SECRET, &token, true, 0).unwrap();
        assert_eq!(claims.doc, "DOC-1");
        assert_eq!(claims.org, Uuid::from_u128(1));

        // A different secret must not verify (cross-deploy / forged token).
        assert!(verify_hs256::<CallbackToken>("other-secret", &token, true, 0).is_err());
    }

    #[cfg(not(feature = "test-postgres"))]
    #[test]
    fn expired_callback_token_is_rejected() {
        let token = sign_hs256(
            SECRET,
            &CallbackToken {
                org: Uuid::from_u128(1),
                doc: "DOC-1".to_owned(),
                exp: OffsetDateTime::now_utc().unix_timestamp() - 10,
            },
        )
        .unwrap();
        // The host-issued token gets NO clock-skew grace.
        assert!(verify_hs256::<CallbackToken>(SECRET, &token, true, 0).is_err());
    }

    #[cfg(not(feature = "test-postgres"))]
    #[test]
    fn onlyoffice_callback_claims_verify_from_signed_payload() {
        // Simulate what DocumentServer sends: a JWT signing the callback body.
        let signed = sign_hs256(
            SECRET,
            &json!({ "status": 2, "url": "http://ds/doc.docx", "key": "abc", "filetype": "docx" }),
        )
        .unwrap();
        let claims: CallbackClaims =
            verify_hs256(SECRET, &signed, true, CALLBACK_CLAIMS_LEEWAY_SECS).unwrap();
        assert_eq!(claims.status, 2);
        assert_eq!(claims.key.as_deref(), Some("abc"));
        assert_eq!(claims.url.as_deref(), Some("http://ds/doc.docx"));
    }

    #[cfg(not(feature = "test-postgres"))]
    #[test]
    fn onlyoffice_callback_claims_reject_expired_payload_beyond_leeway() {
        // A captured/replayed callback whose OWN exp claim has lapsed well
        // past the clock-skew window must not verify (replay hardening).
        let signed = sign_hs256(
            SECRET,
            &json!({
                "status": 2,
                "url": "http://ds/doc.docx",
                "key": "abc",
                "exp": OffsetDateTime::now_utc().unix_timestamp() - 3600,
            }),
        )
        .unwrap();
        assert!(
            verify_hs256::<CallbackClaims>(SECRET, &signed, true, CALLBACK_CLAIMS_LEEWAY_SECS)
                .is_err()
        );
    }

    #[cfg(not(feature = "test-postgres"))]
    #[test]
    fn onlyoffice_callback_claims_accept_expiry_within_clock_skew_leeway() {
        // Just past `exp` but inside the tolerance — normal clock drift, not
        // a replay attempt.
        let signed = sign_hs256(
            SECRET,
            &json!({
                "status": 2,
                "url": "http://ds/doc.docx",
                "key": "abc",
                "exp": OffsetDateTime::now_utc().unix_timestamp() - 5,
            }),
        )
        .unwrap();
        assert!(
            verify_hs256::<CallbackClaims>(SECRET, &signed, true, CALLBACK_CLAIMS_LEEWAY_SECS)
                .is_ok()
        );
    }

    #[cfg(not(feature = "test-postgres"))]
    #[test]
    fn document_key_changes_with_version_and_is_editor_safe() {
        let org = OrgId::from_uuid(Uuid::from_u128(7));
        let v1 = document_key(org, "DOC-1", 1);
        let v2 = document_key(org, "DOC-1", 2);
        assert_ne!(
            v1, v2,
            "a new version must yield a new key (no stale cache)"
        );
        assert!(v1.len() <= 128);
        assert!(
            v1.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.' || c == '_')
        );
    }

    #[cfg(not(feature = "test-postgres"))]
    #[test]
    fn config_token_signs_the_whole_config() {
        let payload = json!({ "document": { "key": "k-v1" }, "editorConfig": { "mode": "edit" } });
        let token = sign_hs256(SECRET, &payload).unwrap();
        let claims: serde_json::Value = verify_hs256(SECRET, &token, false, 0).unwrap();
        assert_eq!(claims["document"]["key"], "k-v1");
    }

    #[cfg(not(feature = "test-postgres"))]
    #[test]
    fn file_type_allowlist_rejects_unknown() {
        assert!(validate_file_type("docx").is_ok());
        assert!(validate_file_type("exe").is_err());
    }

    #[cfg(not(feature = "test-postgres"))]
    #[test]
    fn document_ref_is_trimmed_bounded_and_control_free() {
        assert_eq!(normalize_document_ref("  DOC-1  ").unwrap(), "DOC-1");
        assert!(normalize_document_ref("   ").is_err());
        assert!(normalize_document_ref("DOC\n1").is_err());
        assert!(normalize_document_ref(&"x".repeat(DOCUMENT_REF_MAX_CHARS + 1)).is_err());
    }

    // -----------------------------------------------------------------------
    // SSRF guard: the callback-produced-document URL must match the
    // configured DocumentServer's full origin (scheme + host + port), not
    // just its host.
    // -----------------------------------------------------------------------
    #[cfg(not(feature = "test-postgres"))]
    #[test]
    fn same_origin_rejects_same_host_different_port_attack_url() {
        let configured = url::Url::parse("http://docserver.internal:8080").unwrap();
        // A same-host-different-port URL could reach an unrelated internal
        // service (e.g. a metadata endpoint or an admin port) that
        // DocumentServer never serves from.
        let attack = url::Url::parse("http://docserver.internal:9999/produced.docx").unwrap();
        assert!(!same_origin(&attack, &configured));

        let legit = url::Url::parse("http://docserver.internal:8080/produced.docx").unwrap();
        assert!(same_origin(&legit, &configured));
    }

    #[cfg(not(feature = "test-postgres"))]
    #[test]
    fn same_origin_rejects_scheme_and_host_mismatch() {
        let configured = url::Url::parse("https://docserver.internal").unwrap();
        assert!(!same_origin(
            &url::Url::parse("http://docserver.internal/produced.docx").unwrap(),
            &configured
        ));
        assert!(!same_origin(
            &url::Url::parse("https://evil.example/produced.docx").unwrap(),
            &configured
        ));
    }

    #[cfg(not(feature = "test-postgres"))]
    #[test]
    fn same_origin_is_default_port_aware() {
        let configured = url::Url::parse("http://docserver.internal").unwrap();
        let explicit_default_port =
            url::Url::parse("http://docserver.internal:80/produced.docx").unwrap();
        assert!(same_origin(&explicit_default_port, &configured));
    }
}
