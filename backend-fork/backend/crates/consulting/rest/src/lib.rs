//! Tenant-scoped consulting engagement API. KPI values are intentionally never
//! copied here: observations retain KPI-definition and evidence references only.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

mod openapi;
pub use openapi::OPENAPI_FRAGMENT;

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use console_kernel_core::{
    AuditAction, AuditEvent, BranchScope, ErrorKind, KernelError, OrgId, UserId,
};
use console_platform_auth::JwtVerifier;
use console_platform_authz::{Action, Feature, Principal, authorize_org_wide};
use console_platform_db::{DbError, with_audits, with_org_conn};
use console_platform_request_context::{RequestContextError, current_audit_context, current_org};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Postgres, Row};
use time::OffsetDateTime;
use uuid::Uuid;

pub const CONSULTING_ENGAGEMENTS_PATH: &str = "/api/v1/consulting/engagements";
pub const CONSULTING_ENGAGEMENT_PATH: &str = "/api/v1/consulting/engagements/{engagement_id}";
pub const CONSULTING_DIAGNOSTICS_PATH: &str =
    "/api/v1/consulting/engagements/{engagement_id}/diagnostics";
pub const CONSULTING_FINDINGS_PATH: &str =
    "/api/v1/consulting/engagements/{engagement_id}/findings";
pub const CONSULTING_INITIATIVES_PATH: &str =
    "/api/v1/consulting/engagements/{engagement_id}/initiatives";
pub const CONSULTING_TRANSITION_PATH: &str =
    "/api/v1/consulting/engagements/{engagement_id}/transition";
pub const CONSULTING_OBSERVATIONS_PATH: &str =
    "/api/v1/consulting/engagements/{engagement_id}/observations";
pub const CONSULTING_HISTORY_PATH: &str = "/api/v1/consulting/engagements/{engagement_id}/history";
pub const CONSULTING_ROUTE_PATHS: &[&str] = &[
    CONSULTING_ENGAGEMENTS_PATH,
    CONSULTING_ENGAGEMENT_PATH,
    CONSULTING_DIAGNOSTICS_PATH,
    CONSULTING_FINDINGS_PATH,
    CONSULTING_INITIATIVES_PATH,
    CONSULTING_TRANSITION_PATH,
    CONSULTING_OBSERVATIONS_PATH,
    CONSULTING_HISTORY_PATH,
];

#[derive(Clone)]
pub struct ConsultingRestState {
    pool: PgPool,
    jwt_verifier: Option<JwtVerifier>,
}
impl ConsultingRestState {
    #[must_use]
    pub fn new(pool: PgPool, jwt_verifier: Option<JwtVerifier>) -> Self {
        Self { pool, jwt_verifier }
    }
}
pub fn router(state: ConsultingRestState) -> Router {
    let verifier = state.jwt_verifier.clone();
    let pool = state.pool.clone();
    let router = Router::new()
        .route(
            CONSULTING_ENGAGEMENTS_PATH,
            get(list_engagements).post(create_engagement),
        )
        .route(CONSULTING_ENGAGEMENT_PATH, get(get_engagement))
        .route(
            CONSULTING_DIAGNOSTICS_PATH,
            axum::routing::post(create_diagnostic),
        )
        .route(
            CONSULTING_FINDINGS_PATH,
            axum::routing::post(create_finding),
        )
        .route(
            CONSULTING_INITIATIVES_PATH,
            axum::routing::post(create_initiative),
        )
        .route(CONSULTING_TRANSITION_PATH, axum::routing::post(transition))
        .route(
            CONSULTING_OBSERVATIONS_PATH,
            axum::routing::post(record_observation),
        )
        .route(CONSULTING_HISTORY_PATH, get(list_history))
        .with_state(state);
    console_platform_request_context::with_request_context(router, verifier, pool)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ListParams {
    limit: Option<i64>,
    offset: Option<i64>,
    q: Option<String>,
}
#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
struct Page {
    items: Vec<Engagement>,
    limit: i64,
    offset: i64,
    total: i64,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
struct Engagement {
    id: Uuid,
    customer_id: Uuid,
    customer_document_id: Option<Uuid>,
    ontology_instance_id: Option<Uuid>,
    title: String,
    status: String,
    approval_id: Option<Uuid>,
    workflow_execution_id: Option<Uuid>,
    version: i64,
    #[serde(with = "engagement_timestamp")]
    created_at: OffsetDateTime,
    #[serde(with = "engagement_timestamp")]
    updated_at: OffsetDateTime,
}

/// Read tolerant, write strict, for the one `Engagement` timestamp pair that is
/// both a wire field and a persisted one.
///
/// `Engagement` is stored verbatim in `consulting_engagements.idempotency_response`
/// (JSONB) and read back by the replay branch of `create_engagement`. Rows written
/// before the RFC 3339 attributes landed hold `time`'s non-human-readable
/// component form, a nine-element integer array. Replay records are immutable, so
/// those rows are never rewritten and reading must accept both shapes. Writing is
/// always RFC 3339: the array form must never reach the wire, where
/// `openapi.yaml` declares these fields `{type: string, format: date-time}`.
///
/// DELETE WHEN no reachable environment still holds a pre-RFC-3339 replay record,
/// which is exactly:
/// `SELECT count(*) FROM consulting_engagements WHERE jsonb_typeof(idempotency_response->'created_at') = 'array'`
/// returning 0 in every deployed database. Until then this is live compatibility
/// code, not dead code, and `legacy_component_array_replay_survives_rfc3339_switch`
/// in `backend/app/tests/consulting_engagement_api.rs` proves it.
mod engagement_timestamp {
    use serde::de::Error as _;
    use serde::{Deserialize, Deserializer, Serializer};
    use time::format_description::well_known::Rfc3339;
    use time::{Date, OffsetDateTime, Time, UtcOffset};

    /// `time`'s two persisted shapes: RFC 3339 (current) and the component array
    /// its non-human-readable `Serialize` impl emits (legacy, read-only).
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Stored {
        Rfc3339(String),
        LegacyComponents(i32, u16, u8, u8, u8, u32, i8, i8, i8),
    }

    pub(super) fn serialize<S: Serializer>(
        value: &OffsetDateTime,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        time::serde::rfc3339::serialize(value, serializer)
    }

    pub(super) fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<OffsetDateTime, D::Error> {
        match Stored::deserialize(deserializer)? {
            Stored::Rfc3339(raw) => OffsetDateTime::parse(&raw, &Rfc3339).map_err(D::Error::custom),
            Stored::LegacyComponents(
                year,
                ordinal,
                hour,
                minute,
                second,
                nanosecond,
                offset_hours,
                offset_minutes,
                offset_seconds,
            ) => {
                let date = Date::from_ordinal_date(year, ordinal).map_err(D::Error::custom)?;
                let time = Time::from_hms_nano(hour, minute, second, nanosecond)
                    .map_err(D::Error::custom)?;
                let offset = UtcOffset::from_hms(offset_hours, offset_minutes, offset_seconds)
                    .map_err(D::Error::custom)?;
                Ok(date.with_time(time).assume_offset(offset))
            }
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
struct EngagementDetail {
    #[serde(flatten)]
    engagement: Engagement,
    diagnostics: Vec<Diagnostic>,
    findings: Vec<Finding>,
    initiatives: Vec<Initiative>,
    observations: Vec<Observation>,
}
#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
struct Diagnostic {
    id: Uuid,
    summary: String,
    document_id: Option<Uuid>,
    #[serde(with = "time::serde::rfc3339")]
    created_at: OffsetDateTime,
}
#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
struct Finding {
    id: Uuid,
    diagnostic_id: Uuid,
    statement: String,
    evidence_id: Uuid,
    document_id: Option<Uuid>,
    #[serde(with = "time::serde::rfc3339")]
    created_at: OffsetDateTime,
}
#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
struct Initiative {
    id: Uuid,
    finding_id: Uuid,
    title: String,
    hypothesis: String,
    kpi_definition_id: Uuid,
    target_direction: String,
    #[serde(with = "time::serde::rfc3339")]
    created_at: OffsetDateTime,
}
#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
struct Observation {
    id: Uuid,
    initiative_id: Uuid,
    kpi_definition_id: Uuid,
    evidence_id: Uuid,
    #[serde(with = "time::serde::rfc3339")]
    observed_at: OffsetDateTime,
    note: String,
    #[serde(with = "time::serde::rfc3339")]
    created_at: OffsetDateTime,
}
#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
struct History {
    id: Uuid,
    event_type: String,
    from_status: Option<String>,
    to_status: Option<String>,
    version: i64,
    payload: serde_json::Value,
    #[serde(with = "time::serde::rfc3339")]
    occurred_at: OffsetDateTime,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CreateEngagement {
    customer_id: Uuid,
    customer_document_id: Option<Uuid>,
    ontology_instance_id: Option<Uuid>,
    title: String,
    idempotency_key: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CreateDiagnostic {
    summary: String,
    document_id: Option<Uuid>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CreateFinding {
    diagnostic_id: Uuid,
    statement: String,
    evidence_id: Uuid,
    document_id: Option<Uuid>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CreateInitiative {
    finding_id: Uuid,
    title: String,
    hypothesis: String,
    kpi_definition_id: Uuid,
    target_direction: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Transition {
    to_status: String,
    expected_version: i64,
    approval_id: Option<Uuid>,
    reason: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CreateObservation {
    initiative_id: Uuid,
    kpi_definition_id: Uuid,
    evidence_id: Uuid,
    #[serde(with = "time::serde::rfc3339")]
    observed_at: OffsetDateTime,
    note: String,
}

async fn list_engagements(
    State(state): State<ConsultingRestState>,
    headers: HeaderMap,
    Query(params): Query<ListParams>,
) -> Result<Json<Page>, RestError> {
    let principal = principal(&state, &headers).await?;
    require(&principal, false)?;
    let limit = params.limit.unwrap_or(25).clamp(1, 100);
    let offset = params.offset.unwrap_or(0).max(0);
    let org = current_org().map_err(rest_error_from_request_context)?;
    let q = params.q.filter(|v| !v.trim().is_empty());
    let page = with_org_conn(&state.pool, org, |tx| Box::pin(async move {
        let total: i64 = sqlx::query_scalar("SELECT count(*) FROM consulting_engagements WHERE ($1::text IS NULL OR title ILIKE '%' || $1 || '%')").bind(q.as_deref()).fetch_one(tx.as_mut()).await?;
        let rows = sqlx::query("SELECT id, customer_id, customer_document_id, ontology_instance_id, title, status, approval_id, workflow_execution_id, version, created_at, updated_at FROM consulting_engagements WHERE ($1::text IS NULL OR title ILIKE '%' || $1 || '%') ORDER BY updated_at DESC, id LIMIT $2 OFFSET $3").bind(q.as_deref()).bind(limit).bind(offset).fetch_all(tx.as_mut()).await?;
        Ok::<_, DbError>(Page { items: rows.iter().map(engagement).collect::<Result<_,_>>()?, limit, offset, total })
    })).await.map_err(RestError::db)?;
    Ok(Json(page))
}

async fn create_engagement(
    State(state): State<ConsultingRestState>,
    headers: HeaderMap,
    Json(body): Json<CreateEngagement>,
) -> Result<(StatusCode, Json<Engagement>), RestError> {
    let actor = principal(&state, &headers).await?;
    require(&actor, true)?;
    required(&body.title, "title")?;
    required(&body.idempotency_key, "idempotencyKey")?;
    let org = current_org().map_err(rest_error_from_request_context)?;
    let actor_id = *actor.user_id.as_uuid();
    let id = Uuid::new_v4();
    let request_hash = request_hash(&body);
    let (response_status, value) = with_audits(&state.pool, org, |tx| Box::pin(async move {
        require_reference_kind(tx, body.customer_document_id, "DOCUMENT").await?;
        require_reference_kind(tx, body.ontology_instance_id, "ONTOLOGY_INSTANCE").await?;
        let row = sqlx::query("INSERT INTO consulting_engagements (id, org_id, customer_id, customer_document_id, ontology_instance_id, title, idempotency_key, idempotency_request_hash, created_by) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9) ON CONFLICT (org_id, idempotency_key) DO NOTHING RETURNING id, customer_id, customer_document_id, ontology_instance_id, title, status, approval_id, workflow_execution_id, version, idempotency_response_status, created_at, updated_at")
            .bind(id).bind(*org.as_uuid()).bind(body.customer_id).bind(body.customer_document_id).bind(body.ontology_instance_id).bind(body.title.trim()).bind(body.idempotency_key.trim()).bind(&request_hash).bind(actor_id).fetch_optional(tx.as_mut()).await?;
        if let Some(row) = row {
            let value = engagement(&row)?;
            let status: i16 = row.try_get("idempotency_response_status")?;
            let response = serde_json::to_value(&value).map_err(|error| sqlx::Error::Protocol(error.to_string()))?;
            sqlx::query("UPDATE consulting_engagements SET idempotency_response=$1 WHERE id=$2")
                .bind(response)
                .bind(id)
                .execute(tx.as_mut())
                .await?;
            let audit = insert_history(tx, *org.as_uuid(), id, actor_id, "engagement.created", None, Some("DRAFT"), 1, serde_json::json!({"customer_id": body.customer_id})).await?;
            return Ok(((status, value), vec![audit]));
        }
        let replay = sqlx::query("SELECT idempotency_request_hash, idempotency_response_status, idempotency_response FROM consulting_engagements WHERE org_id=$1 AND idempotency_key=$2")
            .bind(*org.as_uuid()).bind(body.idempotency_key.trim()).fetch_one(tx.as_mut()).await?;
        let stored_hash: String = replay.try_get("idempotency_request_hash")?;
        if stored_hash != request_hash {
            return Err(DbError::Sqlx(sqlx::Error::Protocol("idempotency key was already used with a different request payload".into())));
        }
        let status: i16 = replay.try_get("idempotency_response_status")?;
        let value = serde_json::from_value(replay.try_get("idempotency_response")?)
            .map_err(|error| sqlx::Error::Protocol(error.to_string()))?;
        Ok(((status, value), Vec::new()))
    })).await.map_err(RestError::conflict_or_db)?;
    let response_status = StatusCode::from_u16(response_status as u16).map_err(|_| {
        RestError::kernel(KernelError::validation(
            "invalid stored idempotency response status",
        ))
    })?;
    Ok((response_status, Json(value)))
}

async fn get_engagement(
    State(state): State<ConsultingRestState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Json<EngagementDetail>, RestError> {
    let actor = principal(&state, &headers).await?;
    require(&actor, false)?;
    let org = current_org().map_err(rest_error_from_request_context)?;
    let result = with_org_conn(&state.pool, org, |tx| {
        Box::pin(async move { detail(tx, id).await })
    })
    .await
    .map_err(RestError::db)?;
    result.map(Json).ok_or_else(|| {
        RestError::kernel(KernelError::not_found(
            "consulting engagement was not found",
        ))
    })
}

async fn create_diagnostic(
    State(state): State<ConsultingRestState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(body): Json<CreateDiagnostic>,
) -> Result<(StatusCode, Json<Diagnostic>), RestError> {
    let actor = principal(&state, &headers).await?;
    require(&actor, true)?;
    required(&body.summary, "summary")?;
    let org = current_org().map_err(rest_error_from_request_context)?;
    let actor_id = *actor.user_id.as_uuid();
    let item = with_audits(&state.pool, org, |tx| Box::pin(async move {
        ensure_writable_engagement(tx, id).await?;
        require_reference_kind(tx, body.document_id, "DOCUMENT").await?;
        let row = sqlx::query("INSERT INTO consulting_diagnostics (org_id, engagement_id, summary, document_id, created_by) VALUES ($1,$2,$3,$4,$5) RETURNING id, summary, document_id, created_at")
            .bind(*org.as_uuid()).bind(id).bind(body.summary.trim()).bind(body.document_id).bind(actor_id)
            .fetch_one(tx.as_mut()).await?;
        let engagement_version = version(tx, id).await?;
        let audit = insert_history(
            tx,
            *org.as_uuid(),
            id,
            actor_id,
            "diagnostic.recorded",
            None,
            None,
            engagement_version,
            serde_json::json!({"diagnostic_id": row.try_get::<Uuid, _>("id")?}),
        ).await?;
        let item = diagnostic(&row).map_err(DbError::from)?;
        Ok((item, vec![audit]))
    })).await.map_err(RestError::conflict_or_db)?;
    Ok((StatusCode::CREATED, Json(item)))
}

async fn create_finding(
    State(state): State<ConsultingRestState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(body): Json<CreateFinding>,
) -> Result<(StatusCode, Json<Finding>), RestError> {
    let actor = principal(&state, &headers).await?;
    require(&actor, true)?;
    required(&body.statement, "statement")?;
    let org = current_org().map_err(rest_error_from_request_context)?;
    let actor_id = *actor.user_id.as_uuid();
    let item = with_audits(&state.pool, org, |tx| Box::pin(async move {
        ensure_writable_engagement(tx, id).await?;
        require_reference_kind(tx, Some(body.evidence_id), "EVIDENCE").await?;
        require_reference_kind(tx, body.document_id, "DOCUMENT").await?;
        let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM consulting_diagnostics WHERE id=$1 AND engagement_id=$2)")
            .bind(body.diagnostic_id).bind(id).fetch_one(tx.as_mut()).await?;
        if !exists {
            return Err(DbError::Sqlx(sqlx::Error::RowNotFound));
        }
        let row = sqlx::query("INSERT INTO consulting_findings (org_id,engagement_id,diagnostic_id,statement,evidence_id,document_id,created_by) VALUES ($1,$2,$3,$4,$5,$6,$7) RETURNING id,diagnostic_id,statement,evidence_id,document_id,created_at")
            .bind(*org.as_uuid()).bind(id).bind(body.diagnostic_id).bind(body.statement.trim())
            .bind(body.evidence_id).bind(body.document_id).bind(actor_id)
            .fetch_one(tx.as_mut()).await?;
        let engagement_version = version(tx, id).await?;
        let audit = insert_history(
            tx,
            *org.as_uuid(),
            id,
            actor_id,
            "finding.recorded",
            None,
            None,
            engagement_version,
            serde_json::json!({
                "finding_id": row.try_get::<Uuid, _>("id")?,
                "evidence_id": body.evidence_id,
            }),
        ).await?;
        let item = finding(&row).map_err(DbError::from)?;
        Ok((item, vec![audit]))
    })).await.map_err(RestError::conflict_or_db)?;
    Ok((StatusCode::CREATED, Json(item)))
}

async fn create_initiative(
    State(state): State<ConsultingRestState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(body): Json<CreateInitiative>,
) -> Result<(StatusCode, Json<Initiative>), RestError> {
    let actor = principal(&state, &headers).await?;
    require(&actor, true)?;
    required(&body.title, "title")?;
    required(&body.hypothesis, "hypothesis")?;
    if !matches!(body.target_direction.as_str(), "INCREASE" | "DECREASE") {
        return Err(RestError::kernel(KernelError::validation(
            "targetDirection must be INCREASE or DECREASE",
        )));
    }
    let org = current_org().map_err(rest_error_from_request_context)?;
    let actor_id = *actor.user_id.as_uuid();
    let item = with_audits(&state.pool, org, |tx| Box::pin(async move {
        ensure_writable_engagement(tx, id).await?;
        require_reference_kind(tx, Some(body.kpi_definition_id), "KPI_DEFINITION").await?;
        let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM consulting_findings WHERE id=$1 AND engagement_id=$2)")
            .bind(body.finding_id).bind(id).fetch_one(tx.as_mut()).await?;
        if !exists {
            return Err(DbError::Sqlx(sqlx::Error::RowNotFound));
        }
        let row = sqlx::query("INSERT INTO consulting_initiatives (org_id,engagement_id,finding_id,title,hypothesis,kpi_definition_id,target_direction,created_by) VALUES ($1,$2,$3,$4,$5,$6,$7,$8) RETURNING id,finding_id,title,hypothesis,kpi_definition_id,target_direction,created_at")
            .bind(*org.as_uuid()).bind(id).bind(body.finding_id).bind(body.title.trim())
            .bind(body.hypothesis.trim()).bind(body.kpi_definition_id).bind(body.target_direction).bind(actor_id)
            .fetch_one(tx.as_mut()).await?;
        let engagement_version = version(tx, id).await?;
        let audit = insert_history(
            tx,
            *org.as_uuid(),
            id,
            actor_id,
            "initiative.proposed",
            None,
            None,
            engagement_version,
            serde_json::json!({
                "initiative_id": row.try_get::<Uuid, _>("id")?,
                "kpi_definition_id": body.kpi_definition_id,
            }),
        ).await?;
        let item = initiative(&row).map_err(DbError::from)?;
        Ok((item, vec![audit]))
    })).await.map_err(RestError::conflict_or_db)?;
    Ok((StatusCode::CREATED, Json(item)))
}

async fn transition(
    State(state): State<ConsultingRestState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(body): Json<Transition>,
) -> Result<Json<Engagement>, RestError> {
    let actor = principal(&state, &headers).await?;
    require(&actor, true)?;
    required(&body.reason, "reason")?;
    let org = current_org().map_err(rest_error_from_request_context)?;
    let actor_id = *actor.user_id.as_uuid();
    let value = with_audits(&state.pool, org, |tx| Box::pin(async move {
        let current = ensure_writable_engagement(tx, id).await?;
        if !allowed(&current.status, &body.to_status) {
            return Err(DbError::Sqlx(sqlx::Error::Protocol(
                "invalid consulting transition".into(),
            )));
        }
        if body.to_status == "APPROVED" {
            let approval_id = body.approval_id.ok_or_else(|| {
                sqlx::Error::Protocol("approvalId is required for APPROVED".into())
            })?;
            let consumed: Option<Uuid> = sqlx::query_scalar("INSERT INTO gov_approval_consumptions (org_id, approval_id, consumed_by) SELECT $1, a.id, $2 FROM gov_approvals a WHERE a.id=$3 AND a.org_id=$1 AND a.decision='approved' AND a.kind='consulting.engagement.approval' AND a.target_ref=$4 AND a.requested_by <> $2 AND NOT EXISTS (SELECT 1 FROM gov_approval_consumptions c WHERE c.org_id=$1 AND c.approval_id=a.id) ON CONFLICT (org_id, approval_id) DO NOTHING RETURNING approval_id")
                .bind(*org.as_uuid()).bind(actor_id).bind(approval_id).bind(id)
                .fetch_optional(tx.as_mut()).await?;
            if consumed.is_none() {
                return Err(DbError::Sqlx(sqlx::Error::Protocol(
                    "approval is not an unused four-eyes authorization for this engagement".into(),
                )));
            }
        }
        if body.to_status == "IMPLEMENTED" {
            let ready: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM consulting_initiatives WHERE engagement_id=$1)",
            )
            .bind(id)
            .fetch_one(tx.as_mut())
            .await?;
            if !ready {
                return Err(DbError::Sqlx(sqlx::Error::Protocol(
                    "an initiative is required before implementation closure".into(),
                )));
            }
        }
        if matches!(body.to_status.as_str(), "SUSTAINED" | "CORRECTIVE") {
            let ready: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM consulting_benefit_observations WHERE engagement_id=$1)",
            )
            .bind(id)
            .fetch_one(tx.as_mut())
            .await?;
            if !ready {
                return Err(DbError::Sqlx(sqlx::Error::Protocol(
                    "a benefit observation is required before outcome closure".into(),
                )));
            }
        }
        let row = sqlx::query("UPDATE consulting_engagements SET status=$1, approval_id=COALESCE($2,approval_id), version=version+1, updated_at=now() WHERE id=$3 AND version=$4 RETURNING id,customer_id,customer_document_id,ontology_instance_id,title,status,approval_id,workflow_execution_id,version,created_at,updated_at")
            .bind(&body.to_status).bind(body.approval_id).bind(id).bind(body.expected_version)
            .fetch_optional(tx.as_mut()).await?;
        let row = row.ok_or(sqlx::Error::RowNotFound)?;
        let value = engagement(&row)?;
        let audit = insert_history(
            tx,
            *org.as_uuid(),
            id,
            actor_id,
            "engagement.transitioned",
            Some(&current.status),
            Some(&value.status),
            value.version,
            serde_json::json!({"reason": body.reason, "approval_id": body.approval_id}),
        ).await?;
        Ok((value, vec![audit]))
    })).await.map_err(RestError::conflict_or_db)?;
    Ok(Json(value))
}

async fn record_observation(
    State(state): State<ConsultingRestState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(body): Json<CreateObservation>,
) -> Result<(StatusCode, Json<Observation>), RestError> {
    let actor = principal(&state, &headers).await?;
    require(&actor, true)?;
    required(&body.note, "note")?;
    let org = current_org().map_err(rest_error_from_request_context)?;
    let actor_id = *actor.user_id.as_uuid();
    let item = with_audits(&state.pool, org, |tx| Box::pin(async move {
        let engagement = ensure_writable_engagement(tx, id).await?;
        if engagement.status != "IMPLEMENTED" {
            return Err(DbError::Sqlx(sqlx::Error::Protocol(
                "implementation review must be completed before a benefit observation".into(),
            )));
        }
        require_reference_kind(tx, Some(body.kpi_definition_id), "KPI_DEFINITION").await?;
        require_reference_kind(tx, Some(body.evidence_id), "EVIDENCE").await?;
        let valid: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM consulting_initiatives WHERE id=$1 AND engagement_id=$2 AND kpi_definition_id=$3)")
            .bind(body.initiative_id).bind(id).bind(body.kpi_definition_id)
            .fetch_one(tx.as_mut()).await?;
        if !valid {
            return Err(DbError::Sqlx(sqlx::Error::RowNotFound));
        }
        let row = sqlx::query("INSERT INTO consulting_benefit_observations (org_id,engagement_id,initiative_id,kpi_definition_id,evidence_id,observed_at,note,created_by) VALUES ($1,$2,$3,$4,$5,$6,$7,$8) RETURNING id,initiative_id,kpi_definition_id,evidence_id,observed_at,note,created_at")
            .bind(*org.as_uuid()).bind(id).bind(body.initiative_id).bind(body.kpi_definition_id)
            .bind(body.evidence_id).bind(body.observed_at).bind(body.note.trim()).bind(actor_id)
            .fetch_one(tx.as_mut()).await?;
        let item = observation(&row)?;
        let engagement_version = version(tx, id).await?;
        let audit = insert_history(
            tx,
            *org.as_uuid(),
            id,
            actor_id,
            "benefit.observed",
            None,
            None,
            engagement_version,
            serde_json::json!({
                "observation_id": item.id,
                "kpi_definition_id": item.kpi_definition_id,
                "evidence_id": item.evidence_id,
            }),
        ).await?;
        Ok((item, vec![audit]))
    })).await.map_err(RestError::conflict_or_db)?;
    Ok((StatusCode::CREATED, Json(item)))
}

async fn list_history(
    State(state): State<ConsultingRestState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Json<Vec<History>>, RestError> {
    let actor = principal(&state, &headers).await?;
    require(&actor, false)?;
    let org = current_org().map_err(rest_error_from_request_context)?;
    let value=with_org_conn(&state.pool,org,|tx|Box::pin(async move{ensure_engagement(tx,id).await?;let rows=sqlx::query("SELECT id,event_type,from_status,to_status,version,payload,occurred_at FROM consulting_engagement_history WHERE engagement_id=$1 ORDER BY occurred_at,id").bind(id).fetch_all(tx.as_mut()).await?;rows.iter().map(history).collect::<Result<Vec<_>,_>>().map_err(DbError::from) })).await.map_err(RestError::db)?;
    Ok(Json(value))
}

async fn detail(
    tx: &mut sqlx::Transaction<'_, Postgres>,
    id: Uuid,
) -> Result<Option<EngagementDetail>, DbError> {
    let row=sqlx::query("SELECT id,customer_id,customer_document_id,ontology_instance_id,title,status,approval_id,workflow_execution_id,version,created_at,updated_at FROM consulting_engagements WHERE id=$1").bind(id).fetch_optional(tx.as_mut()).await?;
    let Some(row) = row else { return Ok(None) };
    let diagnostics=sqlx::query("SELECT id,summary,document_id,created_at FROM consulting_diagnostics WHERE engagement_id=$1 ORDER BY created_at").bind(id).fetch_all(tx.as_mut()).await?.iter().map(diagnostic).collect::<Result<_,_>>()?;
    let findings=sqlx::query("SELECT id,diagnostic_id,statement,evidence_id,document_id,created_at FROM consulting_findings WHERE engagement_id=$1 ORDER BY created_at").bind(id).fetch_all(tx.as_mut()).await?.iter().map(finding).collect::<Result<_,_>>()?;
    let initiatives=sqlx::query("SELECT id,finding_id,title,hypothesis,kpi_definition_id,target_direction,created_at FROM consulting_initiatives WHERE engagement_id=$1 ORDER BY created_at").bind(id).fetch_all(tx.as_mut()).await?.iter().map(initiative).collect::<Result<_,_>>()?;
    let observations=sqlx::query("SELECT id,initiative_id,kpi_definition_id,evidence_id,observed_at,note,created_at FROM consulting_benefit_observations WHERE engagement_id=$1 ORDER BY observed_at").bind(id).fetch_all(tx.as_mut()).await?.iter().map(observation).collect::<Result<_,_>>()?;
    Ok(Some(EngagementDetail {
        engagement: engagement(&row)?,
        diagnostics,
        findings,
        initiatives,
        observations,
    }))
}
async fn ensure_engagement(
    tx: &mut sqlx::Transaction<'_, Postgres>,
    id: Uuid,
) -> Result<Engagement, DbError> {
    let row=sqlx::query("SELECT id,customer_id,customer_document_id,ontology_instance_id,title,status,approval_id,workflow_execution_id,version,created_at,updated_at FROM consulting_engagements WHERE id=$1").bind(id).fetch_optional(tx.as_mut()).await?;
    row.map(|r| engagement(&r))
        .transpose()?
        .ok_or(DbError::Sqlx(sqlx::Error::RowNotFound))
}
async fn ensure_writable_engagement(
    tx: &mut sqlx::Transaction<'_, Postgres>,
    id: Uuid,
) -> Result<Engagement, DbError> {
    let engagement = ensure_engagement(tx, id).await?;
    if matches!(engagement.status.as_str(), "SUSTAINED" | "CORRECTIVE") {
        return Err(DbError::Sqlx(sqlx::Error::Protocol(
            "terminal consulting engagements are immutable".into(),
        )));
    }
    Ok(engagement)
}
async fn version(tx: &mut sqlx::Transaction<'_, Postgres>, id: Uuid) -> Result<i64, DbError> {
    sqlx::query_scalar("SELECT version FROM consulting_engagements WHERE id=$1")
        .bind(id)
        .fetch_one(tx.as_mut())
        .await
        .map_err(DbError::from)
}
fn request_hash(body: &CreateEngagement) -> String {
    // Keep keys in lexical order so the canonical bytes are identical whether
    // serde_json uses its default sorted map or the `preserve_order` feature.
    let canonical = serde_json::json!({
        "customer_document_id": body.customer_document_id,
        "customer_id": body.customer_id,
        "ontology_instance_id": body.ontology_instance_id,
        "title": body.title.trim(),
    });
    Sha256::digest(canonical.to_string().as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
async fn require_reference_kind(
    tx: &mut sqlx::Transaction<'_, Postgres>,
    id: Option<Uuid>,
    expected: &str,
) -> Result<(), DbError> {
    let Some(id) = id else { return Ok(()) };
    let actual: Option<String> =
        sqlx::query_scalar("SELECT source_kind FROM consulting_reference_bindings WHERE id=$1")
            .bind(id)
            .fetch_optional(tx.as_mut())
            .await?;
    if actual.as_deref() != Some(expected) {
        return Err(DbError::Sqlx(sqlx::Error::Protocol(format!(
            "consulting reference must be a {expected} binding"
        ))));
    }
    Ok(())
}
#[allow(clippy::too_many_arguments)]
async fn insert_history(
    tx: &mut sqlx::Transaction<'_, Postgres>,
    org: Uuid,
    id: Uuid,
    actor: Uuid,
    event: &str,
    from: Option<&str>,
    to: Option<&str>,
    version: i64,
    payload: serde_json::Value,
) -> Result<AuditEvent, DbError> {
    sqlx::query("INSERT INTO consulting_engagement_history (org_id,engagement_id,actor_id,event_type,from_status,to_status,version,payload) VALUES ($1,$2,$3,$4,$5,$6,$7,$8)").bind(org).bind(id).bind(actor).bind(event).bind(from).bind(to).bind(version).bind(&payload).execute(tx.as_mut()).await?;
    let action =
        AuditAction::new(event).map_err(|error| DbError::CodeIssuance(error.to_string()))?;
    let audit_context = current_audit_context().ok_or_else(|| {
        DbError::CodeIssuance(
            "authenticated consulting mutation is missing request audit context".to_owned(),
        )
    })?;
    Ok(AuditEvent::new(
        Some(UserId::from_uuid(actor)),
        action,
        "consulting_engagement",
        id.to_string(),
        audit_context.trace,
        OffsetDateTime::now_utc(),
    )
    .with_org(OrgId::from_uuid(org))
    .with_request_context(audit_context.request)
    .with_snapshots(
        from.map(|status| serde_json::json!({"status": status})),
        Some(serde_json::json!({
            "status": to,
            "version": version,
            "payload": payload,
        })),
    ))
}
fn engagement(row: &sqlx::postgres::PgRow) -> Result<Engagement, sqlx::Error> {
    Ok(Engagement {
        id: row.try_get("id")?,
        customer_id: row.try_get("customer_id")?,
        customer_document_id: row.try_get("customer_document_id")?,
        ontology_instance_id: row.try_get("ontology_instance_id")?,
        title: row.try_get("title")?,
        status: row.try_get("status")?,
        approval_id: row.try_get("approval_id")?,
        workflow_execution_id: row.try_get("workflow_execution_id")?,
        version: row.try_get("version")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}
fn diagnostic(row: &sqlx::postgres::PgRow) -> Result<Diagnostic, sqlx::Error> {
    Ok(Diagnostic {
        id: row.try_get("id")?,
        summary: row.try_get("summary")?,
        document_id: row.try_get("document_id")?,
        created_at: row.try_get("created_at")?,
    })
}
fn finding(row: &sqlx::postgres::PgRow) -> Result<Finding, sqlx::Error> {
    Ok(Finding {
        id: row.try_get("id")?,
        diagnostic_id: row.try_get("diagnostic_id")?,
        statement: row.try_get("statement")?,
        evidence_id: row.try_get("evidence_id")?,
        document_id: row.try_get("document_id")?,
        created_at: row.try_get("created_at")?,
    })
}
fn initiative(row: &sqlx::postgres::PgRow) -> Result<Initiative, sqlx::Error> {
    Ok(Initiative {
        id: row.try_get("id")?,
        finding_id: row.try_get("finding_id")?,
        title: row.try_get("title")?,
        hypothesis: row.try_get("hypothesis")?,
        kpi_definition_id: row.try_get("kpi_definition_id")?,
        target_direction: row.try_get("target_direction")?,
        created_at: row.try_get("created_at")?,
    })
}
fn observation(row: &sqlx::postgres::PgRow) -> Result<Observation, sqlx::Error> {
    Ok(Observation {
        id: row.try_get("id")?,
        initiative_id: row.try_get("initiative_id")?,
        kpi_definition_id: row.try_get("kpi_definition_id")?,
        evidence_id: row.try_get("evidence_id")?,
        observed_at: row.try_get("observed_at")?,
        note: row.try_get("note")?,
        created_at: row.try_get("created_at")?,
    })
}
fn history(row: &sqlx::postgres::PgRow) -> Result<History, sqlx::Error> {
    Ok(History {
        id: row.try_get("id")?,
        event_type: row.try_get("event_type")?,
        from_status: row.try_get("from_status")?,
        to_status: row.try_get("to_status")?,
        version: row.try_get("version")?,
        payload: row.try_get("payload")?,
        occurred_at: row.try_get("occurred_at")?,
    })
}
fn allowed(from: &str, to: &str) -> bool {
    matches!(
        (from, to),
        ("DRAFT", "PROPOSED")
            | ("PROPOSED", "APPROVED")
            | ("APPROVED", "IMPLEMENTED")
            | ("IMPLEMENTED", "MEASURED")
            | ("MEASURED", "SUSTAINED")
            | ("MEASURED", "CORRECTIVE")
    )
}
fn required(value: &str, field: &str) -> Result<(), RestError> {
    if value.trim().is_empty() {
        Err(RestError::kernel(KernelError::validation(format!(
            "{field} is required"
        ))))
    } else {
        Ok(())
    }
}
fn require(principal: &Principal, write: bool) -> Result<(), RestError> {
    let feature = if write {
        Feature::ConsultingManage
    } else {
        Feature::ConsultingRead
    };
    let action = Action::new(feature);
    let allowed = match &principal.branch_scope {
        BranchScope::All => authorize_org_wide(principal, action),
        // Engagements do not carry a branch/site key. Allowing any one branch to
        // authorize an org-wide row would widen that branch's authority.
        BranchScope::Branches(_) => Err(KernelError::forbidden(
            "consulting engagements require organization-wide authority",
        )),
    };
    allowed.map_err(RestError::kernel)
}
async fn principal(
    state: &ConsultingRestState,
    headers: &HeaderMap,
) -> Result<Principal, RestError> {
    let verifier = state.jwt_verifier.as_ref().ok_or_else(|| {
        RestError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "unavailable",
            "JWT verification is not configured for consulting",
        )
    })?;
    console_platform_request_context::resolve_principal(verifier, &state.pool, headers)
        .await
        .map_err(rest_error_from_request_context)
}
fn rest_error_from_request_context(error: RequestContextError) -> RestError {
    match error {
        RequestContextError::MissingBearer
        | RequestContextError::InvalidToken
        | RequestContextError::InvalidClaim(_) => RestError::new(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "missing, malformed, or invalid bearer token",
        ),
        RequestContextError::WrongTokenTier => RestError::kernel(KernelError::forbidden(
            "token is not authorized for consulting",
        )),
        RequestContextError::AccessScope(error) => RestError::kernel(error),
        RequestContextError::VerifierUnavailable => RestError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "unavailable",
            "JWT verification is not configured for consulting",
        ),
        RequestContextError::BranchScope(message)
        | RequestContextError::EffectivePolicy(message) => {
            RestError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal", message)
        }
        RequestContextError::MissingOrg => RestError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal",
            "no tenant context is bound",
        ),
    }
}
#[derive(Serialize)]
struct ErrorBody {
    error: ErrorPayload,
}
#[derive(Serialize)]
struct ErrorPayload {
    code: &'static str,
    message: String,
}
struct RestError {
    status: StatusCode,
    code: &'static str,
    message: String,
}
impl RestError {
    fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
        }
    }
    fn kernel(error: KernelError) -> Self {
        let status = match error.kind {
            ErrorKind::Validation => StatusCode::UNPROCESSABLE_ENTITY,
            ErrorKind::NotFound => StatusCode::NOT_FOUND,
            ErrorKind::Forbidden => StatusCode::FORBIDDEN,
            ErrorKind::Conflict | ErrorKind::InvalidTransition => StatusCode::CONFLICT,
            ErrorKind::Internal => StatusCode::INTERNAL_SERVER_ERROR,
        };
        Self::new(
            status,
            if status == StatusCode::CONFLICT {
                "conflict"
            } else {
                "error"
            },
            error.message,
        )
    }
    fn db(_: console_platform_db::DbError) -> Self {
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal",
            "internal server error",
        )
    }
    fn conflict_or_db(error: console_platform_db::DbError) -> Self {
        match error {
            console_platform_db::DbError::Sqlx(sqlx::Error::RowNotFound) => Self::new(
                StatusCode::CONFLICT,
                "conflict",
                "engagement changed or was not found; reload before retrying",
            ),
            console_platform_db::DbError::Sqlx(sqlx::Error::Protocol(message)) => {
                Self::new(StatusCode::CONFLICT, "conflict", message)
            }
            console_platform_db::DbError::Sqlx(sqlx::Error::Database(database))
                if database.code().as_deref() == Some("P0001")
                    && database.constraint() == Some("consulting_terminal_immutable")
                    && database.message() == "terminal consulting engagement is immutable" =>
            {
                Self::new(
                    StatusCode::CONFLICT,
                    "conflict",
                    "terminal consulting engagements are immutable",
                )
            }
            _ => Self::db(error),
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
                },
            }),
        )
            .into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_hash_is_stable_lowercase_sha256() {
        let body = CreateEngagement {
            customer_id: Uuid::from_u128(0x22222222_2222_2222_2222_222222222222),
            customer_document_id: Some(Uuid::from_u128(0x11111111_1111_1111_1111_111111111111)),
            ontology_instance_id: Some(Uuid::from_u128(0x33333333_3333_3333_3333_333333333333)),
            title: " Stabilize operations ".to_owned(),
            idempotency_key: "stable-hash-fixture".to_owned(),
        };

        let hash = request_hash(&body);

        assert_eq!(
            hash,
            "9b6be0adc1d8155f7b4f54313c7039792b8b31638f411ae7f041233dbe7e672c"
        );
        assert_eq!(hash.len(), 64);
        assert!(
            hash.bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        );
    }

    #[test]
    fn missing_org_request_context_fails_closed() {
        let error = rest_error_from_request_context(RequestContextError::MissingOrg);

        assert_eq!(error.status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(error.code, "internal");
        assert_eq!(error.message, "no tenant context is bound");
    }

    #[test]
    fn idempotency_payload_mismatch_is_a_conflict() {
        let error = RestError::conflict_or_db(DbError::Sqlx(sqlx::Error::Protocol(
            "idempotency key was already used with a different request payload".into(),
        )));

        assert_eq!(error.status, StatusCode::CONFLICT);
        assert_eq!(error.code, "conflict");
        assert_eq!(
            error.message,
            "idempotency key was already used with a different request payload"
        );
    }
}
