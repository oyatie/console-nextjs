//! Docs/evidence REST surface — the read + integrity actions the console's
//! EvidenceCard needs over the EV objects the `console-docs-adapter-postgres` store
//! already owns (RLS + audit + WORM append-only).
//!
//! Endpoints:
//!  * `GET  /api/v1/evidence/objects`            — RLS-scoped list with filters;
//!  * `GET  /api/v1/evidence/objects/{id}`       — full detail: original-vs-derivative
//!    copies (each with its stored SHA-256), the custody-event chain, TSA proof
//!    state (nullable — RFC-3161 is a FUTURE lane, never faked), legal holds and
//!    exports;
//!  * `POST /api/v1/evidence/objects/{id}/verify`— REAL fixity check: for each copy
//!    it HEADs the WORM object and compares the store's recorded SHA-256 against
//!    the digest persisted at registration. A mismatch is reported (and audited),
//!    never silently ignored;
//!  * `POST /api/v1/evidence/objects/{id}/hold`  — apply OR release a legal hold.
//!    Applying is protective (no four-eyes). RELEASE is fail-closed behind a
//!    distinct-approver four-eyes decision read from the governance store.
//!
//! All mutations flow through the store's `with_audits`; reads arm `app.current_org`
//! via `with_org_conn` inside the store, so FORCE RLS scopes every row to the
//! caller's tenant. `router(state)` self-applies `with_request_context`; the wire
//! lane merges it into `build_router` (this crate does not touch that merge).
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

mod openapi;
pub use openapi::OPENAPI_FRAGMENT;

use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use console_docs_adapter_postgres::PgDocsStore;
use console_docs_application::{
    ApplyLegalHoldCommand, EvidenceObjectCursor, EvidenceObjectDetail, EvidenceObjectPage,
    LegalHoldRecordView, ListEvidenceObjectsQuery, ReleaseLegalHoldCommand,
};
use console_docs_domain::{
    AdmissibilityStatus, CustodyStage, EvidenceClassification, EvidenceCopyKind,
    EvidenceSourceType, LegalHoldState,
};
use console_governance_adapter_postgres::PgGovernanceStore;
use console_kernel_core::{
    AuditAction, AuditEvent, ErrorKind, EvidenceLegalHoldId, EvidenceObjectId, KernelError,
    TraceContext, UserId,
};
use console_platform_auth::JwtVerifier;
use console_platform_authz::{Action, Feature, Principal, authorize_org_wide};
use console_platform_db::{DbError, with_audits};
use console_platform_request_context::current_org;
use console_platform_storage::S3ObjectStore;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// State + router
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct DocsRestState {
    docs: PgDocsStore,
    governance: PgGovernanceStore,
    /// The WORM (compliance-locked) object store the fixity check HEADs. `None`
    /// when object storage is unconfigured — `verify` then 503s rather than
    /// green-lighting an unverifiable object.
    storage: Option<Arc<dyn S3ObjectStore>>,
    /// The bucket WORM copies live in (the storage config's `replica_bucket`).
    worm_bucket: String,
    jwt_verifier: Option<JwtVerifier>,
}

impl DocsRestState {
    #[must_use]
    pub fn new(
        docs: PgDocsStore,
        governance: PgGovernanceStore,
        storage: Option<Arc<dyn S3ObjectStore>>,
        worm_bucket: String,
        jwt_verifier: Option<JwtVerifier>,
    ) -> Self {
        Self {
            docs,
            governance,
            storage,
            worm_bucket,
            jwt_verifier,
        }
    }

    /// The underlying evidence store (RLS + audit + WORM append-only). Exposed so
    /// tests and automation can drive the same store the handlers use.
    #[must_use]
    pub fn docs_store(&self) -> &PgDocsStore {
        &self.docs
    }

    /// The governance store the release gate reads four-eyes approvals from.
    #[must_use]
    pub fn governance_store(&self) -> &PgGovernanceStore {
        &self.governance
    }
}

pub const OBJECTS_PATH: &str = "/api/v1/evidence/objects";
pub const OBJECT_ID_PATH: &str = "/api/v1/evidence/objects/{id}";
pub const OBJECT_VERIFY_PATH: &str = "/api/v1/evidence/objects/{id}/verify";
pub const OBJECT_HOLD_PATH: &str = "/api/v1/evidence/objects/{id}/hold";

pub const EVIDENCE_ROUTE_PATHS: &[&str] = &[
    OBJECTS_PATH,
    OBJECT_ID_PATH,
    OBJECT_VERIFY_PATH,
    OBJECT_HOLD_PATH,
];

pub fn router(state: DocsRestState) -> Router {
    let verifier = state.jwt_verifier.clone();
    let pool = state.docs.pool().clone();
    let router = Router::new()
        .route(OBJECTS_PATH, get(list_objects))
        .route(OBJECT_ID_PATH, get(get_object))
        .route(OBJECT_VERIFY_PATH, post(verify_object))
        .route(OBJECT_HOLD_PATH, post(hold_object))
        .with_state(state);
    console_platform_request_context::with_request_context(router, verifier, pool)
}

// ---------------------------------------------------------------------------
// Read surface
// ---------------------------------------------------------------------------

/// Query string for the list endpoint. Bare scalars so a browser can drive it;
/// converted into the store's typed [`ListEvidenceObjectsQuery`].
#[derive(Debug, Default, Deserialize)]
struct ListQuery {
    #[serde(default)]
    q: Option<String>,
    #[serde(default)]
    source_type: Option<EvidenceSourceType>,
    #[serde(default)]
    source_id: Option<String>,
    #[serde(default)]
    admissibility_status: Option<AdmissibilityStatus>,
    #[serde(default)]
    legal_hold_state: Option<LegalHoldState>,
    #[serde(default)]
    custody_stage: Option<CustodyStage>,
    #[serde(default)]
    classification: Option<EvidenceClassification>,
    #[serde(default)]
    limit: Option<i64>,
    #[serde(default)]
    offset: Option<i64>,
    #[serde(default)]
    as_of: Option<String>,
    #[serde(default)]
    cursor: Option<String>,
}

impl TryFrom<ListQuery> for ListEvidenceObjectsQuery {
    type Error = KernelError;

    fn try_from(value: ListQuery) -> Result<Self, Self::Error> {
        let as_of = value
            .as_of
            .as_deref()
            .map(|raw| {
                raw.parse::<i64>()
                    .map_err(|_| KernelError::validation("as_of must be a register sequence"))
            })
            .transpose()?;
        let cursor = value
            .cursor
            .as_deref()
            .map(decode_register_cursor)
            .transpose()?;
        Ok(Self {
            q: value.q,
            source_type: value.source_type,
            source_id: value.source_id,
            admissibility_status: value.admissibility_status,
            legal_hold_state: value.legal_hold_state,
            custody_stage: value.custody_stage,
            classification: value.classification,
            limit: value.limit,
            offset: value.offset,
            as_of,
            cursor,
        })
    }
}

fn decode_register_cursor(raw: &str) -> Result<EvidenceObjectCursor, KernelError> {
    use base64::Engine as _;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(raw)
        .map_err(|_| KernelError::validation("cursor is not valid base64url"))?;
    serde_json::from_slice(&bytes).map_err(|_| KernelError::validation("cursor payload is invalid"))
}

fn encode_register_cursor(cursor: &EvidenceObjectCursor) -> Result<String, KernelError> {
    use base64::Engine as _;
    let bytes = serde_json::to_vec(cursor)
        .map_err(|_| KernelError::internal("EV cursor serialization failed"))?;
    Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes))
}

#[derive(Debug, Serialize)]
struct EvidenceObjectPageResponse {
    items: Vec<console_docs_application::EvidenceObjectView>,
    limit: i64,
    offset: i64,
    total: i64,
    as_of: i64,
    next_cursor: Option<String>,
}

impl TryFrom<EvidenceObjectPage> for EvidenceObjectPageResponse {
    type Error = KernelError;

    fn try_from(page: EvidenceObjectPage) -> Result<Self, Self::Error> {
        Ok(Self {
            items: page.items,
            limit: page.limit,
            offset: page.offset,
            total: page.total,
            as_of: page.as_of,
            next_cursor: page
                .next_cursor
                .as_ref()
                .map(encode_register_cursor)
                .transpose()?,
        })
    }
}

async fn list_objects(
    State(state): State<DocsRestState>,
    headers: HeaderMap,
    Query(query): Query<ListQuery>,
) -> Result<Json<EvidenceObjectPageResponse>, RestError> {
    authorize(&state, &headers, Feature::EvidenceAttach).await?;
    let page = state
        .docs
        .list_objects(query.try_into().map_err(RestError::from_kernel)?)
        .await
        .map_err(RestError::from_docs)?;
    Ok(Json(page.try_into().map_err(RestError::from_kernel)?))
}

async fn get_object(
    State(state): State<DocsRestState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Json<EvidenceObjectDetail>, RestError> {
    authorize(&state, &headers, Feature::EvidenceAttach).await?;
    let detail = state
        .docs
        .get_object(EvidenceObjectId::from_uuid(id))
        .await
        .map_err(RestError::from_docs)?
        .ok_or_else(|| RestError::from_kernel(KernelError::not_found("EV object was not found")))?;
    Ok(Json(detail))
}

// ---------------------------------------------------------------------------
// Verify (real WORM fixity check)
// ---------------------------------------------------------------------------

/// Per-copy fixity verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum FixityStatus {
    /// The store's recorded SHA-256 equals the digest persisted at registration.
    Match,
    /// The store's recorded SHA-256 differs — the object or its record changed.
    Mismatch,
    /// The store returned no SHA-256 checksum, so fixity cannot be confirmed.
    ChecksumUnavailable,
    /// The object could not be read from the store (missing / transport error).
    StorageError,
}

/// Overall verdict for the object: any mismatch dominates; otherwise any
/// unconfirmable copy makes the whole check indeterminate; else verified.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum VerifyOutcome {
    Verified,
    Mismatch,
    Indeterminate,
}

#[derive(Debug, Clone, Serialize)]
pub struct CopyVerification {
    pub copy_id: Uuid,
    pub copy_kind: EvidenceCopyKind,
    pub recorded_digest_sha256: String,
    /// The store's recorded digest, normalised to lowercase hex (`None` when the
    /// store exposes none).
    pub storage_checksum_sha256: Option<String>,
    pub recorded_size_bytes: i64,
    pub storage_size_bytes: Option<i64>,
    pub status: FixityStatus,
}

#[derive(Debug, Clone, Serialize)]
pub struct EvidenceVerifyReport {
    pub evidence_object_id: Uuid,
    #[serde(with = "time::serde::rfc3339")]
    pub verified_at: OffsetDateTime,
    pub outcome: VerifyOutcome,
    pub copies: Vec<CopyVerification>,
}

impl DocsRestState {
    /// Recompute fixity for every copy of an object against the WORM store and
    /// write ONE audit row recording the verdict. HTTP-independent so tests /
    /// automation can drive it directly. Reads happen under `current_org`; the
    /// caller must already have armed it (the store's read path does so).
    pub async fn verify_object_fixity(
        &self,
        actor: UserId,
        object_id: EvidenceObjectId,
        trace: TraceContext,
        now: OffsetDateTime,
    ) -> Result<EvidenceVerifyReport, VerifyError> {
        let store = self
            .storage
            .as_ref()
            .ok_or(VerifyError::StorageUnconfigured)?;
        let detail = self
            .docs
            .get_object(object_id)
            .await
            .map_err(VerifyError::Docs)?
            .ok_or(VerifyError::NotFound)?;

        // HEAD each copy OUTSIDE any DB transaction, then persist the verdict.
        let mut copies = Vec::with_capacity(detail.copies.len());
        for copy in &detail.copies {
            let recorded = copy.digest_sha256.as_str().to_owned();
            let head = store
                .head_object(self.worm_bucket.clone(), copy.storage.object_id.clone())
                .await;
            let (storage_checksum, storage_size, status) = match head {
                Ok(head) => {
                    let checksum = head.checksum_sha256.as_deref().and_then(normalize_sha256);
                    let status = match checksum.as_deref() {
                        Some(actual) if actual == recorded => FixityStatus::Match,
                        Some(_) => FixityStatus::Mismatch,
                        None => FixityStatus::ChecksumUnavailable,
                    };
                    (checksum, Some(head.size_bytes), status)
                }
                Err(err) => {
                    tracing::warn!(error = %err, copy_id = %copy.id.as_uuid(), "EV fixity HEAD failed");
                    (None, None, FixityStatus::StorageError)
                }
            };
            copies.push(CopyVerification {
                copy_id: *copy.id.as_uuid(),
                copy_kind: copy.copy_kind,
                recorded_digest_sha256: recorded,
                storage_checksum_sha256: storage_checksum,
                recorded_size_bytes: copy.size_bytes,
                storage_size_bytes: storage_size,
                status,
            });
        }

        let outcome = verdict(&copies);
        let report = EvidenceVerifyReport {
            evidence_object_id: *object_id.as_uuid(),
            verified_at: now,
            outcome,
            copies,
        };

        // Audit-only writeback: one row recording who verified what and the
        // verdict. No domain-table mutation (verify is a read-side integrity act).
        let org = current_org()
            .map_err(KernelError::from)
            .map_err(VerifyError::from)?;
        let audit_after = serde_json::json!({
            "outcome": report.outcome,
            "copies": report.copies,
        });
        with_audits::<_, (), VerifyError>(self.docs.pool(), org, move |_tx| {
            let event = AuditEvent::new(
                Some(actor),
                match AuditAction::new("evidence_object.verify") {
                    Ok(action) => action,
                    Err(err) => return Box::pin(async move { Err(VerifyError::from(err)) }),
                },
                "evidence_object",
                object_id.to_string(),
                trace,
                now,
            )
            .with_org(org)
            .with_snapshots(None, Some(audit_after));
            Box::pin(async move { Ok(((), vec![event])) })
        })
        .await?;

        Ok(report)
    }
}

/// Fold the per-copy verdicts into the object verdict. No copies ⇒ nothing to
/// stand on ⇒ indeterminate (never a false "verified").
fn verdict(copies: &[CopyVerification]) -> VerifyOutcome {
    if copies.is_empty() {
        return VerifyOutcome::Indeterminate;
    }
    if copies.iter().any(|c| c.status == FixityStatus::Mismatch) {
        return VerifyOutcome::Mismatch;
    }
    if copies.iter().any(|c| c.status != FixityStatus::Match) {
        return VerifyOutcome::Indeterminate;
    }
    VerifyOutcome::Verified
}

/// Normalise a store-reported SHA-256 to lowercase hex. S3 exposes checksums as
/// base64 (`x-amz-checksum-sha256`); some stores echo hex. Accept either, reject
/// anything that is not a 32-byte digest.
fn normalize_sha256(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    // Already lowercase/upper hex?
    if trimmed.len() == 64 && trimmed.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Some(trimmed.to_ascii_lowercase());
    }
    // Base64 (standard, with padding) of the 32 raw bytes.
    use base64::Engine as _;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(trimmed)
        .ok()?;
    if bytes.len() == 32 {
        Some(hex::encode(bytes))
    } else {
        None
    }
}

async fn verify_object(
    State(state): State<DocsRestState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Json<EvidenceVerifyReport>, RestError> {
    let principal = authorize(&state, &headers, Feature::EvidenceAttach).await?;
    let report = state
        .verify_object_fixity(
            principal.user_id,
            EvidenceObjectId::from_uuid(id),
            TraceContext::generate(),
            OffsetDateTime::now_utc(),
        )
        .await
        .map_err(RestError::from_verify)?;
    Ok(Json(report))
}

// ---------------------------------------------------------------------------
// Legal hold (apply / release)
// ---------------------------------------------------------------------------

/// The hold request body. `op` selects apply vs release; release additionally
/// requires a four-eyes `fourEyesRequestRef` whose approval is read from the DB.
#[derive(Debug, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
enum HoldRequest {
    Apply {
        case_ref: String,
        basis: String,
        reason: String,
    },
    Release {
        hold_id: Uuid,
        reason: String,
        four_eyes_request_ref: Uuid,
    },
}

impl DocsRestState {
    /// Apply a legal hold — protective, no four-eyes. Flips the object to
    /// `legal_hold_state = ACTIVE`, which the DB trigger uses to block disposal.
    pub async fn apply_hold(
        &self,
        command: ApplyLegalHoldCommand,
    ) -> Result<LegalHoldRecordView, HoldError> {
        self.docs
            .apply_legal_hold(command)
            .await
            .map_err(HoldError::Docs)
    }

    /// Release a legal hold — fail-closed behind a DISTINCT-approver four-eyes
    /// decision that is BOUND to this hold and SINGLE-USE. The approval must be
    /// decided under `kind = evidence.hold.release` with its `target_ref` equal to
    /// the hold being released (both the gate and the release act on the same
    /// `command.hold_id`), and it is consumed here so it can never release a second
    /// hold (replay denied). Consumed in its own committed step ahead of the release
    /// tx: a release failure spends the approval (fail-closed; re-request).
    pub async fn release_hold(
        &self,
        four_eyes_request_ref: Uuid,
        command: ReleaseLegalHoldCommand,
    ) -> Result<LegalHoldRecordView, HoldError> {
        match self
            .governance
            .four_eyes_consume(
                four_eyes_request_ref,
                EVIDENCE_HOLD_RELEASE_FOUR_EYES_KIND,
                Some(*command.hold_id.as_uuid()),
                command.actor,
            )
            .await
            .map_err(HoldError::governance)?
        {
            Some(true) => {}
            _ => return Err(HoldError::FourEyesRequired),
        }
        self.docs
            .release_legal_hold(command)
            .await
            .map_err(HoldError::Docs)
    }
}

/// The four-eyes `kind` a hold-release approval is decided under — must match the
/// `kind` the console opens the approval with (`web/.../evidenceApi.ts`).
const EVIDENCE_HOLD_RELEASE_FOUR_EYES_KIND: &str = "evidence.hold.release";

async fn hold_object(
    State(state): State<DocsRestState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(body): Json<HoldRequest>,
) -> Result<Response, RestError> {
    let principal = authorize(&state, &headers, Feature::RoleManage).await?;
    let object_id = EvidenceObjectId::from_uuid(id);
    let now = OffsetDateTime::now_utc();
    let trace = TraceContext::generate();

    match body {
        HoldRequest::Apply {
            case_ref,
            basis,
            reason,
        } => {
            let hold = state
                .apply_hold(ApplyLegalHoldCommand {
                    actor: principal.user_id,
                    evidence_object_id: object_id,
                    case_ref,
                    basis,
                    reason,
                    trace,
                    occurred_at: now,
                })
                .await
                .map_err(RestError::from_hold)?;
            Ok((StatusCode::CREATED, Json(hold)).into_response())
        }
        HoldRequest::Release {
            hold_id,
            reason,
            four_eyes_request_ref,
        } => {
            let hold = state
                .release_hold(
                    four_eyes_request_ref,
                    ReleaseLegalHoldCommand {
                        actor: principal.user_id,
                        evidence_object_id: object_id,
                        hold_id: EvidenceLegalHoldId::from_uuid(hold_id),
                        release_reason: reason,
                        trace,
                        occurred_at: now,
                    },
                )
                .await
                .map_err(RestError::from_hold)?;
            Ok(Json(hold).into_response())
        }
    }
}

// ---------------------------------------------------------------------------
// Typed action errors (HTTP-independent, so tests can assert the reason)
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub enum VerifyError {
    /// Object storage is not configured — fixity cannot be checked.
    StorageUnconfigured,
    NotFound,
    Docs(console_docs_adapter_postgres::PgDocsError),
    Kernel(KernelError),
    Db(DbError),
}

impl From<KernelError> for VerifyError {
    fn from(value: KernelError) -> Self {
        Self::Kernel(value)
    }
}

impl From<DbError> for VerifyError {
    fn from(value: DbError) -> Self {
        Self::Db(value)
    }
}

#[derive(Debug)]
pub enum HoldError {
    /// Release was requested without a distinct-approver four-eyes approval.
    FourEyesRequired,
    Docs(console_docs_adapter_postgres::PgDocsError),
}

impl HoldError {
    fn governance(error: console_governance_adapter_postgres::PgGovernanceError) -> Self {
        use console_governance_adapter_postgres::PgGovernanceError as E;
        match error {
            E::Db(db) => Self::Docs(console_docs_adapter_postgres::PgDocsError::from(db)),
            E::Domain(kernel) => {
                Self::Docs(console_docs_adapter_postgres::PgDocsError::from(kernel))
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Auth
// ---------------------------------------------------------------------------

/// The console EvidenceCard is an org-scoped compliance surface. Reads + verify
/// gate on `EvidenceAttach`; the legal-hold mutation gates on the org-admin
/// `RoleManage` capability (a hold is governance-grade).
// ponytail: legacy role matrix is the sole enforcer today; the wire lane may
// swap these for the granular `evidence_*` feature keys already in the catalog.
async fn authorize(
    state: &DocsRestState,
    headers: &HeaderMap,
    feature: Feature,
) -> Result<Principal, RestError> {
    let verifier = state.jwt_verifier.as_ref().ok_or_else(|| {
        RestError::unavailable("JWT verification is not configured for evidence API")
    })?;
    let principal =
        console_platform_request_context::resolve_principal(verifier, state.docs.pool(), headers)
            .await
            .map_err(rest_error_from_request_context)?;
    authorize_org_wide(&principal, Action::new(feature)).map_err(RestError::from_kernel)?;
    Ok(principal)
}

// ---------------------------------------------------------------------------
// Errors (mirrors the ontology/governance rest error surface)
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct RestError {
    status: StatusCode,
    code: &'static str,
    message: String,
}

impl RestError {
    fn unauthorized(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::UNAUTHORIZED,
            code: "unauthorized",
            message: message.into(),
        }
    }

    fn unavailable(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::SERVICE_UNAVAILABLE,
            code: "unavailable",
            message: message.into(),
        }
    }

    /// Distinguishes a fixity-storage outage from generic 503s (for example,
    /// JWT verifier infrastructure), which the console must keep retryable.
    fn evidence_store_unavailable(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::SERVICE_UNAVAILABLE,
            code: "evidence_store_unavailable",
            message: message.into(),
        }
    }

    fn internal(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            code: "internal",
            message: message.into(),
        }
    }

    fn from_kernel(error: KernelError) -> Self {
        Self {
            status: status_for_error_kind(error.kind),
            code: code_for_error_kind(error.kind),
            message: error.message,
        }
    }

    fn from_docs(error: console_docs_adapter_postgres::PgDocsError) -> Self {
        use console_docs_adapter_postgres::PgDocsError as E;
        match error {
            E::Domain(kernel) => Self::from_kernel(kernel),
            E::Db(db) => Self::from_db(db),
        }
    }

    fn from_verify(error: VerifyError) -> Self {
        match error {
            VerifyError::StorageUnconfigured => Self::evidence_store_unavailable(
                "evidence storage is not configured for fixity verification",
            ),
            VerifyError::NotFound => {
                Self::from_kernel(KernelError::not_found("EV object was not found"))
            }
            VerifyError::Docs(error) => Self::from_docs(error),
            VerifyError::Kernel(kernel) => Self::from_kernel(kernel),
            VerifyError::Db(db) => Self::from_db(db),
        }
    }

    fn from_hold(error: HoldError) -> Self {
        match error {
            HoldError::FourEyesRequired => Self {
                status: StatusCode::FORBIDDEN,
                code: "four_eyes_required",
                message: "releasing a legal hold requires a distinct-approver four-eyes approval"
                    .to_owned(),
            },
            HoldError::Docs(error) => Self::from_docs(error),
        }
    }

    fn from_db(error: DbError) -> Self {
        match error {
            DbError::Sqlx(sqlx::Error::RowNotFound) => {
                Self::from_kernel(KernelError::not_found("row was not found"))
            }
            DbError::Sqlx(sqlx::Error::Database(err))
                if err.code().is_some_and(|code| code == "23505") =>
            {
                tracing::error!(error = %err, "evidence unique-constraint violation");
                Self::from_kernel(KernelError::conflict("resource already exists"))
            }
            DbError::Sqlx(err) => {
                tracing::error!(error = %err, "database error");
                Self::internal("internal server error")
            }
            DbError::Serialize(err) => {
                tracing::error!(error = %err, "serialization error");
                Self::internal("internal server error")
            }
            DbError::CodeIssuance(err) => {
                tracing::error!(error = %err, "object-code issuance error");
                Self::internal("internal server error")
            }
        }
    }
}

fn rest_error_from_request_context(
    err: console_platform_request_context::RequestContextError,
) -> RestError {
    use console_platform_request_context::RequestContextError as E;
    match err {
        E::VerifierUnavailable => {
            RestError::unavailable("JWT verification is not configured for evidence API")
        }
        E::WrongTokenTier => RestError::from_kernel(KernelError::forbidden(
            "token tier is not valid for this route",
        )),
        E::AccessScope(error) => RestError::from_kernel(error),
        E::BranchScope(message) | E::EffectivePolicy(message) => RestError::internal(message),
        E::MissingOrg => RestError::internal("no tenant context is bound to the current request"),
        E::MissingBearer => RestError::unauthorized("missing or malformed bearer token"),
        E::InvalidToken => RestError::unauthorized("invalid bearer token"),
        E::InvalidClaim(message) => {
            RestError::unauthorized(format!("token claim is invalid: {message}"))
        }
    }
}

#[derive(Debug, Serialize)]
struct ErrorBody {
    error: ErrorPayload,
}

#[derive(Debug, Serialize)]
struct ErrorPayload {
    code: &'static str,
    message: String,
}

impl IntoResponse for RestError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(ErrorBody {
                error: ErrorPayload {
                    code: self.code,
                    message: self.message,
                },
            }),
        )
            .into_response()
    }
}

const fn status_for_error_kind(kind: ErrorKind) -> StatusCode {
    match kind {
        ErrorKind::Validation => StatusCode::UNPROCESSABLE_ENTITY,
        ErrorKind::NotFound => StatusCode::NOT_FOUND,
        ErrorKind::Forbidden => StatusCode::FORBIDDEN,
        ErrorKind::Conflict | ErrorKind::InvalidTransition => StatusCode::CONFLICT,
        ErrorKind::Internal => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

const fn code_for_error_kind(kind: ErrorKind) -> &'static str {
    match kind {
        ErrorKind::Validation => "validation",
        ErrorKind::NotFound => "not_found",
        ErrorKind::Forbidden => "forbidden",
        ErrorKind::Conflict => "conflict",
        ErrorKind::InvalidTransition => "invalid_transition",
        ErrorKind::Internal => "internal",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evidence_store_unavailability_has_a_stable_distinct_error_code() {
        let error = RestError::from_verify(VerifyError::StorageUnconfigured);
        assert_eq!(error.status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(error.code, "evidence_store_unavailable");
    }

    #[test]
    fn generic_service_unavailability_keeps_the_generic_error_code() {
        let error = RestError::unavailable("JWT verification is not configured for evidence API");
        assert_eq!(error.status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(error.code, "unavailable");
    }

    #[test]
    fn normalize_accepts_hex_and_base64_and_rejects_junk() {
        let digest = "a".repeat(64);
        assert_eq!(
            normalize_sha256(&digest.to_uppercase()),
            Some(digest.clone())
        );

        use base64::Engine as _;
        let raw = [0xABu8; 32];
        let b64 = base64::engine::general_purpose::STANDARD.encode(raw);
        assert_eq!(normalize_sha256(&b64), Some("ab".repeat(32)));

        assert_eq!(normalize_sha256("not-a-digest"), None);
        assert_eq!(normalize_sha256(""), None);
    }

    #[test]
    fn register_cursor_round_trips_as_an_opaque_base64url_token() {
        let cursor = EvidenceObjectCursor {
            snapshot_sequence: 42,
            register_sequence: 41,
            id: EvidenceObjectId::new(),
        };
        let token = encode_register_cursor(&cursor).expect("cursor encodes");
        assert!(!token.contains('{'));
        assert_eq!(decode_register_cursor(&token).unwrap(), cursor);
        assert!(decode_register_cursor("not_base64!").is_err());
    }

    #[test]
    fn verdict_folds_copy_statuses() {
        let mk = |status| CopyVerification {
            copy_id: Uuid::nil(),
            copy_kind: EvidenceCopyKind::Original,
            recorded_digest_sha256: String::new(),
            storage_checksum_sha256: None,
            recorded_size_bytes: 0,
            storage_size_bytes: None,
            status,
        };
        assert_eq!(verdict(&[]), VerifyOutcome::Indeterminate);
        assert_eq!(verdict(&[mk(FixityStatus::Match)]), VerifyOutcome::Verified);
        assert_eq!(
            verdict(&[mk(FixityStatus::Match), mk(FixityStatus::Mismatch)]),
            VerifyOutcome::Mismatch
        );
        assert_eq!(
            verdict(&[
                mk(FixityStatus::Match),
                mk(FixityStatus::ChecksumUnavailable)
            ]),
            VerifyOutcome::Indeterminate
        );
    }
}
