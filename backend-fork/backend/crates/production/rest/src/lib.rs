//! Tenant-scoped production-subcontracting pilot REST surface.
//!
//! This slice owns production plan persistence only. Customer demand, inventory,
//! people/staffing, approvals, ontology, and reporting remain external ports;
//! their identifiers and check snapshots are recorded here without copying any
//! source-domain records.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

mod openapi;
pub use openapi::OPENAPI_FRAGMENT;

use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use base64::Engine as _;
use console_kernel_core::{
    AuditAction, AuditEvent, BranchId, ErrorKind, KernelError, OrgId, TraceContext, UserId,
};
use console_platform_auth::JwtVerifier;
use console_platform_authz::{
    Action, Feature, Principal, ServicePrincipal, authorize, authorize_service,
};
use console_platform_db::{DbError, insert_audit_event, with_audits, with_org_conn};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Row};
use time::OffsetDateTime;
use uuid::Uuid;

mod service_auth;

pub const PRODUCTION_PLANS_PATH: &str = "/api/v1/production/plans";
pub const PRODUCTION_CAPACITY_SLOTS_PATH: &str = "/api/v1/production/capacity-slots";
pub const PRODUCTION_PLAN_PATH: &str = "/api/v1/production/plans/{plan_id}";
pub const PRODUCTION_PLAN_RELEASE_PATH: &str = "/api/v1/production/plans/{plan_id}/release";
pub const PRODUCTION_OPERATION_RECORDS_PATH: &str =
    "/api/v1/production/plans/{plan_id}/operations/{operation_id}/records";
pub const PRODUCTION_SOURCE_INGRESS_PATH: &str = "/api/v1/production/source-ingress";
pub const PRODUCTION_SOURCE_SYSTEMS_PATH: &str = "/api/v1/production/source-systems";
pub const PRODUCTION_SOURCE_SYSTEM_ROTATE_PATH: &str =
    "/api/v1/production/source-systems/{source_system_id}/rotate";
pub const PRODUCTION_SOURCE_SYSTEM_DISABLE_PATH: &str =
    "/api/v1/production/source-systems/{source_system_id}/disable";
pub const PRODUCTION_ROUTE_PATHS: &[&str] = &[
    PRODUCTION_PLANS_PATH,
    PRODUCTION_CAPACITY_SLOTS_PATH,
    PRODUCTION_PLAN_PATH,
    PRODUCTION_PLAN_RELEASE_PATH,
    PRODUCTION_OPERATION_RECORDS_PATH,
    PRODUCTION_SOURCE_INGRESS_PATH,
    PRODUCTION_SOURCE_SYSTEMS_PATH,
    PRODUCTION_SOURCE_SYSTEM_ROTATE_PATH,
    PRODUCTION_SOURCE_SYSTEM_DISABLE_PATH,
];

#[derive(Clone)]
pub struct ProductionRestState {
    pool: PgPool,
    jwt_verifier: Option<JwtVerifier>,
    service_principal_hmac_key: Option<[u8; 32]>,
}
impl ProductionRestState {
    #[must_use]
    pub fn new(pool: PgPool, jwt_verifier: Option<JwtVerifier>) -> Self {
        Self {
            pool,
            jwt_verifier,
            service_principal_hmac_key: None,
        }
    }

    #[must_use]
    pub const fn with_service_principal_hmac_key(mut self, key: Option<[u8; 32]>) -> Self {
        self.service_principal_hmac_key = key;
        self
    }
}

pub fn router(state: ProductionRestState) -> Router {
    let verifier = state.jwt_verifier.clone();
    let pool = state.pool.clone();
    let human_router = Router::new()
        .route(PRODUCTION_PLANS_PATH, get(list_plans).post(create_plan))
        .route(PRODUCTION_CAPACITY_SLOTS_PATH, get(list_capacity_slots))
        .route(PRODUCTION_PLAN_PATH, get(get_plan))
        .route(PRODUCTION_PLAN_RELEASE_PATH, post(release_plan))
        .route(PRODUCTION_OPERATION_RECORDS_PATH, post(record_operation))
        .route(PRODUCTION_SOURCE_SYSTEMS_PATH, post(register_source_system))
        .route(
            PRODUCTION_SOURCE_SYSTEM_ROTATE_PATH,
            post(rotate_source_system),
        )
        .route(
            PRODUCTION_SOURCE_SYSTEM_DISABLE_PATH,
            post(disable_source_system),
        )
        .with_state(state.clone());
    let human_router =
        console_platform_request_context::with_request_context(human_router, verifier, pool);
    // Basic-auth ingress is deliberately outside JWT request-context middleware.
    // It authenticates the machine principal before parsing JSON, resolves the
    // tenant through the narrow SECURITY DEFINER function, then arms RLS itself.
    Router::new()
        .route(PRODUCTION_SOURCE_INGRESS_PATH, post(ingest_source))
        .with_state(state)
        .merge(human_router)
}

#[derive(Deserialize)]
struct ListQuery {
    branch_id: BranchId,
    #[serde(default = "default_limit")]
    limit: i64,
    #[serde(default)]
    offset: i64,
}
const fn default_limit() -> i64 {
    25
}
#[derive(Deserialize, Serialize)]
struct CreatePlan {
    branch_id: BranchId,
    customer_demand_id: Uuid,
    capacity_slot_id: Uuid,
    material_item_id: Uuid,
    quantity: i64,
    #[serde(with = "time::serde::rfc3339")]
    due_at: OffsetDateTime,
    idempotency_key: String,
    ontology_type_id: Uuid,
}
#[derive(Deserialize, Serialize)]
struct ReleasePlan {
    expected_version: i32,
    idempotency_key: String,
    approval_ref: Uuid,
}
/// The only accepted production machine-ingress shapes. The external wire
/// representation is intentionally stable and tagged by `kind`.
#[derive(Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum SourceIngress {
    Demand {
        id: Uuid,
        inquiry_id: Uuid,
        product_code: String,
        quantity: i64,
        #[serde(with = "time::serde::rfc3339")]
        due_at: OffsetDateTime,
        source_id: String,
        source_version: String,
    },
    Capacity {
        id: Uuid,
        site_id: Uuid,
        capacity_date: time::Date,
        available_quantity: i64,
        source_id: String,
        source_version: String,
    },
    Material {
        material_item_id: Uuid,
        quantity_on_hand_milli: i64,
        safety_stock_milli: i64,
        source_id: String,
        source_version: String,
    },
}
#[derive(Deserialize)]
struct RegisterSourceSystem {
    branch_id: BranchId,
    source_system: String,
}
#[derive(Deserialize)]
struct RotateSourceSystem {
    expected_generation: i32,
}
#[derive(Deserialize)]
struct DisableSourceSystem {
    expected_generation: i32,
}

/// A one-time credential disclosure. `secret` is standard base64 for exactly
/// 32 random server-generated bytes; only its HMAC verifier is persisted.
#[derive(Serialize)]
struct SourceSystemCredential {
    id: Uuid,
    source_system: String,
    enabled: bool,
    credential_generation: i32,
    secret: String,
}

#[derive(Serialize)]
struct SourceSystemReceipt {
    id: Uuid,
    enabled: bool,
    credential_generation: i32,
}

#[derive(Serialize, Deserialize)]
struct SourceIngressReceipt {
    kind: String,
    id: Uuid,
    source_version: String,
}

fn disabled_source_system_receipt(id: Uuid, generation: i32) -> SourceSystemReceipt {
    SourceSystemReceipt {
        id,
        enabled: false,
        credential_generation: generation,
    }
}
#[derive(Deserialize, Serialize)]
struct RecordOperation {
    expected_version: i32,
    idempotency_key: String,
    output_quantity: i64,
    scrap_quantity: i64,
    downtime_minutes: i32,
    quality_evidence_ref: String,
    quality_passed: bool,
    note: String,
}
#[derive(Deserialize)]
struct CapacityQuery {
    branch_id: BranchId,
    capacity_date: time::Date,
}
#[derive(Serialize)]
struct CapacitySlot {
    id: Uuid,
    branch_id: Uuid,
    site_id: Uuid,
    capacity_date: time::Date,
    available_quantity: i64,
    reserved_quantity: i64,
    version: i32,
    source_ref: String,
    evaluated_at: OffsetDateTime,
}

#[derive(Deserialize, Serialize)]
struct PlanSummary {
    id: Uuid,
    branch_id: Uuid,
    customer_demand_id: Uuid,
    product_code: String,
    quantity: i64,
    status: String,
    version: i32,
    first_operation_id: Uuid,
    #[serde(with = "time::serde::rfc3339")]
    created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    due_at: OffsetDateTime,
    plan_digest: String,
}
#[derive(Serialize)]
struct PlanDetail {
    #[serde(flatten)]
    plan: PlanSummary,
    checks: serde_json::Value,
    events: Vec<LifecycleEvent>,
    operation: OperationDetail,
}
#[derive(Serialize)]
struct LifecycleEvent {
    id: Uuid,
    event_type: String,
    actor_id: Uuid,
    payload: serde_json::Value,
    occurred_at: OffsetDateTime,
}
#[derive(Deserialize, Serialize)]
struct OperationDetail {
    id: Uuid,
    sequence: i32,
    status: String,
    output_quantity: i64,
    scrap_quantity: i64,
    downtime_minutes: i32,
    quality_evidence_ref: Option<String>,
    quality_passed: Option<bool>,
    version: i32,
}

async fn list_plans(
    State(state): State<ProductionRestState>,
    headers: HeaderMap,
    Query(query): Query<ListQuery>,
) -> Result<Json<Vec<PlanSummary>>, RestError> {
    if !(1..=100).contains(&query.limit) || query.offset < 0 {
        return Err(RestError::validation(
            "limit must be 1..100 and offset must be non-negative",
        ));
    }
    let principal = principal(&state, &headers).await?;
    authorize_daily_plan_read(&principal, query.branch_id)?;
    let org = console_platform_request_context::current_org()
        .map_err(|_| RestError::internal("tenant context is missing"))?;
    let mut tx = state.pool.begin().await.map_err(RestError::db)?;
    arm_tenant(&mut tx, *org.as_uuid()).await?;
    let rows = sqlx::query("SELECT id, branch_id, customer_demand_id, product_code, quantity, status, version, first_operation_id, created_at, due_at, plan_digest FROM production_plans WHERE branch_id=$1 ORDER BY created_at DESC, id DESC LIMIT $2 OFFSET $3")
        .bind(*query.branch_id.as_uuid()).bind(query.limit).bind(query.offset).fetch_all(&mut *tx).await.map_err(RestError::db)?;
    tx.commit().await.map_err(RestError::db)?;
    Ok(Json(
        rows.into_iter()
            .map(plan_summary)
            .collect::<Result<_, _>>()?,
    ))
}
async fn list_capacity_slots(
    State(state): State<ProductionRestState>,
    headers: HeaderMap,
    Query(query): Query<CapacityQuery>,
) -> Result<Json<Vec<CapacitySlot>>, RestError> {
    let principal = principal(&state, &headers).await?;
    authorize_daily_plan_read(&principal, query.branch_id)?;
    let org = console_platform_request_context::current_org()
        .map_err(|_| RestError::internal("tenant context is missing"))?;
    let mut tx = state.pool.begin().await.map_err(RestError::db)?;
    arm_tenant(&mut tx, *org.as_uuid()).await?;
    let rows = sqlx::query("SELECT id,branch_id,site_id,capacity_date,available_quantity,reserved_quantity,version,source_id AS source_ref,evaluated_at FROM production_capacity_slots WHERE branch_id=$1 AND capacity_date=$2 ORDER BY site_id")
        .bind(*query.branch_id.as_uuid()).bind(query.capacity_date).fetch_all(&mut *tx).await.map_err(RestError::db)?;
    tx.commit().await.map_err(RestError::db)?;
    Ok(Json(
        rows.into_iter()
            .map(capacity_slot)
            .collect::<Result<_, _>>()?,
    ))
}

async fn register_source_system(
    State(state): State<ProductionRestState>,
    headers: HeaderMap,
    Json(request): Json<RegisterSourceSystem>,
) -> Result<(StatusCode, Json<SourceSystemCredential>), RestError> {
    if request.source_system.trim().is_empty() || request.source_system.len() > 80 {
        return Err(RestError::validation(
            "source_system is required and must be at most 80 characters",
        ));
    }
    let hmac_key = state
        .service_principal_hmac_key
        .ok_or_else(|| RestError::unavailable("production source authentication is unavailable"))?;
    let principal = principal(&state, &headers).await?;
    authorize(
        &principal,
        Action::limited(Feature::RoleManage),
        request.branch_id,
    )
    .map_err(RestError::kernel)?;
    let org = console_platform_request_context::current_org()
        .map_err(|_| RestError::internal("tenant context is missing"))?;
    let id = Uuid::new_v4();
    let secret = generated_secret();
    let verifier = service_auth::verifier(
        &hmac_key,
        &secret,
        org,
        console_kernel_core::ServicePrincipalId::from_uuid(id),
        request.branch_id,
        1,
    );
    let source_system = request.source_system.trim().to_owned();
    let response = with_audits::<_, _, RestError>(&state.pool, org, move |tx| {
        Box::pin(async move {
            sqlx::query("INSERT INTO service_principals (id,org_id,branch_id,feature,display_name,verifier,created_by) VALUES ($1,$2,$3,'production_source_ingest',$4,$5,$6)")
                .bind(id).bind(*org.as_uuid()).bind(*request.branch_id.as_uuid()).bind(&source_system).bind(verifier.as_slice()).bind(*principal.user_id.as_uuid()).execute(tx.as_mut()).await.map_err(RestError::db)?;
            sqlx::query("INSERT INTO service_principal_audit_events (org_id,service_principal_id,event_type,actor_id,resulting_generation) VALUES ($1,$2,'REGISTERED',$3,1)").bind(*org.as_uuid()).bind(id).bind(*principal.user_id.as_uuid()).execute(tx.as_mut()).await.map_err(RestError::db)?;
            let audit_event = production_audit_event(
                Some(principal.user_id), org, Some(request.branch_id),
                "production.source_system_register", "production_source_system", id,
                serde_json::json!({"source_system": source_system, "credential_generation": 1}),
            )?;
            Ok(((StatusCode::CREATED, Json(SourceSystemCredential {
                id,
                source_system,
                enabled: true,
                credential_generation: 1,
                secret: base64::engine::general_purpose::STANDARD.encode(secret),
            })), vec![audit_event]))
        })
    }).await?;
    Ok(response)
}
async fn rotate_source_system(
    State(state): State<ProductionRestState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(request): Json<RotateSourceSystem>,
) -> Result<Json<SourceSystemCredential>, RestError> {
    rotate_source_system_credential(&state, &headers, id, request.expected_generation).await
}
async fn disable_source_system(
    State(state): State<ProductionRestState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(request): Json<DisableSourceSystem>,
) -> Result<Json<SourceSystemReceipt>, RestError> {
    disable_source_system_credential(&state, &headers, id, request.expected_generation).await
}
async fn source_system_lifecycle_context(
    state: &ProductionRestState,
    headers: &HeaderMap,
    id: Uuid,
) -> Result<(Principal, OrgId, BranchId, i32, String), RestError> {
    let principal = principal(state, headers).await?;
    let org = console_platform_request_context::current_org()
        .map_err(|_| RestError::internal("tenant context is missing"))?;
    let (branch_id, generation, source_system) =
        with_org_conn::<_, _, RestError>(&state.pool, org, move |tx| {
            Box::pin(async move {
                let source = sqlx::query(
                    "SELECT branch_id,generation,display_name FROM service_principals WHERE id=$1 AND feature=$2",
                )
                .bind(id)
                .bind(service_auth::PRODUCTION_SOURCE_INGEST_FEATURE)
                .fetch_optional(tx.as_mut())
                .await
                .map_err(RestError::db)?
                .ok_or_else(|| RestError::not_found("production service principal not found"))?;
                Ok((
                    BranchId::from_uuid(source.try_get("branch_id").map_err(RestError::db)?),
                    source.try_get("generation").map_err(RestError::db)?,
                    source.try_get("display_name").map_err(RestError::db)?,
                ))
            })
        })
        .await?;
    authorize(&principal, Action::limited(Feature::RoleManage), branch_id)
        .map_err(RestError::kernel)?;
    Ok((principal, org, branch_id, generation, source_system))
}

async fn rotate_source_system_credential(
    state: &ProductionRestState,
    headers: &HeaderMap,
    id: Uuid,
    expected_generation: i32,
) -> Result<Json<SourceSystemCredential>, RestError> {
    let (principal, org, branch_id, generation, source_system) =
        source_system_lifecycle_context(state, headers, id).await?;
    if expected_generation != generation {
        return Err(RestError::conflict("service principal generation changed"));
    }
    let hmac_key = state
        .service_principal_hmac_key
        .ok_or_else(|| RestError::unavailable("production source authentication is unavailable"))?;
    let secret = generated_secret();
    let next_generation = generation + 1;
    let verifier = service_auth::verifier(
        &hmac_key,
        &secret,
        org,
        console_kernel_core::ServicePrincipalId::from_uuid(id),
        branch_id,
        next_generation,
    );
    let response = with_audits::<_, _, RestError>(&state.pool, org, move |tx| {
        Box::pin(async move {
            let row = sqlx::query("UPDATE service_principals SET verifier=$1,generation=$2,rotated_by=$3,rotated_at=now() WHERE id=$4 AND feature=$5 AND state='ACTIVE' AND generation=$6 RETURNING generation")
                .bind(verifier.as_slice()).bind(next_generation).bind(*principal.user_id.as_uuid()).bind(id).bind(service_auth::PRODUCTION_SOURCE_INGEST_FEATURE).bind(expected_generation)
                .fetch_optional(tx.as_mut()).await.map_err(RestError::db)?
                .ok_or_else(|| RestError::conflict("service principal generation changed or source system is disabled"))?;
            let resulting_generation: i32 = row.try_get("generation").map_err(RestError::db)?;
            sqlx::query("INSERT INTO service_principal_audit_events (org_id,service_principal_id,event_type,actor_id,expected_generation,resulting_generation) VALUES ($1,$2,'ROTATED',$3,$4,$5)")
                .bind(*org.as_uuid()).bind(id).bind(*principal.user_id.as_uuid()).bind(expected_generation).bind(resulting_generation)
                .execute(tx.as_mut()).await.map_err(RestError::db)?;
            let audit_event = production_audit_event(
                Some(principal.user_id), org, Some(branch_id), "production.source_system_rotate",
                "production_source_system", id,
                serde_json::json!({"expected_generation": expected_generation, "resulting_generation": resulting_generation}),
            )?;
            Ok((Json(SourceSystemCredential {
                id, source_system, enabled: true, credential_generation: resulting_generation,
                secret: base64::engine::general_purpose::STANDARD.encode(secret),
            }), vec![audit_event]))
        })
    }).await?;
    Ok(response)
}

async fn disable_source_system_credential(
    state: &ProductionRestState,
    headers: &HeaderMap,
    id: Uuid,
    expected_generation: i32,
) -> Result<Json<SourceSystemReceipt>, RestError> {
    let (principal, org, branch_id, generation, _source_system) =
        source_system_lifecycle_context(state, headers, id).await?;
    if expected_generation != generation {
        return Err(RestError::conflict("service principal generation changed"));
    }
    let response = with_audits::<_, _, RestError>(&state.pool, org, move |tx| {
        Box::pin(async move {
            let row = sqlx::query("UPDATE service_principals SET state='DISABLED',disabled_by=$1,disabled_at=now() WHERE id=$2 AND feature=$3 AND state='ACTIVE' AND generation=$4 RETURNING generation")
                .bind(*principal.user_id.as_uuid()).bind(id).bind(service_auth::PRODUCTION_SOURCE_INGEST_FEATURE).bind(expected_generation)
                .fetch_optional(tx.as_mut()).await.map_err(RestError::db)?
                .ok_or_else(|| RestError::conflict("service principal generation changed or source system is already disabled"))?;
            let resulting_generation: i32 = row.try_get("generation").map_err(RestError::db)?;
            sqlx::query("INSERT INTO service_principal_audit_events (org_id,service_principal_id,event_type,actor_id,expected_generation,resulting_generation) VALUES ($1,$2,'DISABLED',$3,$4,$5)")
                .bind(*org.as_uuid()).bind(id).bind(*principal.user_id.as_uuid()).bind(expected_generation).bind(resulting_generation)
                .execute(tx.as_mut()).await.map_err(RestError::db)?;
            let audit_event = production_audit_event(
                Some(principal.user_id), org, Some(branch_id), "production.source_system_disable",
                "production_source_system", id,
                serde_json::json!({"expected_generation": expected_generation, "resulting_generation": resulting_generation}),
            )?;
            Ok((Json(disabled_source_system_receipt(id, resulting_generation)), vec![audit_event]))
        })
    }).await?;
    Ok(response)
}

/// Writes source-owned planning facts through an enabled registered workload.
/// The immutable source identity/version is the replay key; a reused version
/// with different bytes is a conflict and every accepted ingress retains its
/// actor and server-derived source-system registry identity.
async fn ingest_source(
    State(state): State<ProductionRestState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<SourceIngressReceipt>, RestError> {
    let hmac_key = state
        .service_principal_hmac_key
        .ok_or_else(|| RestError::unavailable("production source authentication is unavailable"))?;
    let credentials = service_auth::parse_basic_credentials(
        headers
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok()),
    )
    .ok_or_else(|| RestError::unauthorized("invalid production source credentials"))?;

    // Authenticate before parsing an untrusted JSON body. The resolver is the
    // sole pre-RLS bridge and exposes only the tenant UUID.
    let mut tx = state.pool.begin().await.map_err(RestError::db)?;
    let org_id: Option<Uuid> = sqlx::query_scalar("SELECT production_service_principal_org($1)")
        .bind(*credentials.client_id.as_uuid())
        .fetch_one(&mut *tx)
        .await
        .map_err(RestError::db)?;
    let org_id =
        org_id.ok_or_else(|| RestError::unauthorized("invalid production source credentials"))?;
    arm_tenant(&mut tx, org_id).await?;
    let source = sqlx::query("SELECT id, org_id, branch_id, feature, generation, verifier FROM service_principals WHERE id=$1 AND state='ACTIVE' FOR SHARE")
        .bind(*credentials.client_id.as_uuid())
        .fetch_optional(&mut *tx).await.map_err(RestError::db)?
        .ok_or_else(|| RestError::unauthorized("invalid production source credentials"))?;
    let service_principal_id: Uuid = source.try_get("id").map_err(RestError::db)?;
    let source_org: Uuid = source.try_get("org_id").map_err(RestError::db)?;
    let branch_id = BranchId::from_uuid(source.try_get("branch_id").map_err(RestError::db)?);
    let feature: String = source.try_get("feature").map_err(RestError::db)?;
    let generation: i32 = source.try_get("generation").map_err(RestError::db)?;
    let stored_verifier: Vec<u8> = source.try_get("verifier").map_err(RestError::db)?;
    let machine = ServicePrincipal::new(
        credentials.client_id,
        console_kernel_core::OrgId::from_uuid(source_org),
        branch_id,
        Feature::ProductionSourceIngest,
    );
    authorize_service(
        &machine,
        Action::limited(Feature::ProductionSourceIngest),
        branch_id,
    )
    .map_err(RestError::kernel)?;
    let expected = service_auth::verifier(
        &hmac_key,
        credentials.secret(),
        machine.org_id,
        machine.id,
        branch_id,
        generation,
    );
    if feature != service_auth::PRODUCTION_SOURCE_INGEST_FEATURE
        || !service_auth::verifier_matches(&stored_verifier, &expected)
    {
        return Err(RestError::unauthorized(
            "invalid production source credentials",
        ));
    }
    let request: SourceIngress = serde_json::from_slice(&body)
        .map_err(|_| RestError::validation("invalid production source ingress body"))?;
    let org = console_kernel_core::OrgId::from_uuid(org_id);
    let (kind, source_id, source_version) = match &request {
        SourceIngress::Demand {
            source_id,
            source_version,
            ..
        } => ("DEMAND", source_id.clone(), source_version.clone()),
        SourceIngress::Capacity {
            source_id,
            source_version,
            ..
        } => ("CAPACITY", source_id.clone(), source_version.clone()),
        SourceIngress::Material {
            source_id,
            source_version,
            ..
        } => ("MATERIAL", source_id.clone(), source_version.clone()),
    };
    if source_id.trim().is_empty()
        || source_version.trim().is_empty()
        || source_id.len() > 160
        || source_version.len() > 160
    {
        return Err(RestError::validation(
            "source id and version are required and must be at most 160 characters",
        ));
    }
    let request_hash = hash_request(&request)?;
    let source_system = format!("service-principal:{service_principal_id}");
    sqlx::query("INSERT INTO service_principal_ingress_claims (org_id,service_principal_id,kind,source_id,source_version,payload_hash) VALUES ($1,$2,$3,$4,$5,$6) ON CONFLICT DO NOTHING")
        .bind(*org.as_uuid()).bind(service_principal_id).bind(kind).bind(source_id.trim()).bind(source_version.trim()).bind(&request_hash).execute(&mut *tx).await.map_err(RestError::db)?;
    let claim = sqlx::query("SELECT payload_hash,response FROM service_principal_ingress_claims WHERE org_id=$1 AND service_principal_id=$2 AND kind=$3 AND source_id=$4 AND source_version=$5 FOR UPDATE")
        .bind(*org.as_uuid()).bind(service_principal_id).bind(kind).bind(source_id.trim()).bind(source_version.trim()).fetch_one(&mut *tx).await.map_err(RestError::db)?;
    let stored_hash: String = claim.try_get("payload_hash").map_err(RestError::db)?;
    if stored_hash != request_hash {
        return Err(RestError::conflict(
            "source version was already ingested with different content",
        ));
    }
    if let Some(response) = claim
        .try_get::<Option<serde_json::Value>, _>("response")
        .map_err(RestError::db)?
    {
        tx.commit().await.map_err(RestError::db)?;
        let receipt: SourceIngressReceipt = serde_json::from_value(response)
            .map_err(|_| RestError::internal("stored source ingress receipt is invalid"))?;
        return Ok(Json(receipt));
    }
    let receipt = match request {
        SourceIngress::Demand {
            id,
            inquiry_id,
            product_code,
            quantity,
            due_at,
            ..
        } => {
            if product_code.trim().is_empty() || quantity <= 0 {
                return Err(RestError::validation(
                    "demand product and positive quantity are required",
                ));
            }
            let persisted_id: Uuid = sqlx::query_scalar("INSERT INTO production_demand_contracts (id,org_id,inquiry_id,product_code,quantity,due_at,source_system,source_id,source_version,evaluated_at) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,now()) ON CONFLICT (org_id,source_system,source_id,source_version) DO UPDATE SET id=production_demand_contracts.id RETURNING id")
                .bind(id).bind(*org.as_uuid()).bind(inquiry_id).bind(product_code.trim()).bind(quantity).bind(due_at).bind(source_system.trim()).bind(source_id.trim()).bind(source_version.trim()).fetch_one(&mut *tx).await.map_err(RestError::db)?;
            SourceIngressReceipt {
                kind: "demand".to_owned(),
                id: persisted_id,
                source_version,
            }
        }
        SourceIngress::Capacity {
            id,
            site_id,
            capacity_date,
            available_quantity,
            ..
        } => {
            if available_quantity <= 0 {
                return Err(RestError::validation("capacity must be positive"));
            }
            let persisted_id: Uuid = sqlx::query_scalar("INSERT INTO production_capacity_slots (id,org_id,branch_id,site_id,capacity_date,available_quantity,source_system,source_id,source_version,evaluated_at) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,now()) ON CONFLICT (org_id,branch_id,site_id,capacity_date) DO UPDATE SET available_quantity=EXCLUDED.available_quantity, source_system=EXCLUDED.source_system, source_id=EXCLUDED.source_id, source_version=EXCLUDED.source_version, evaluated_at=now(), updated_at=now(), version=production_capacity_slots.version+1 WHERE production_capacity_slots.reserved_quantity <= EXCLUDED.available_quantity RETURNING id")
                .bind(id).bind(*org.as_uuid()).bind(*branch_id.as_uuid()).bind(site_id).bind(capacity_date).bind(available_quantity).bind(source_system.trim()).bind(source_id.trim()).bind(source_version.trim()).fetch_optional(&mut *tx).await.map_err(RestError::db)?
                .ok_or_else(|| RestError::conflict("capacity source conflicts with current reservations"))?;
            SourceIngressReceipt {
                kind: "capacity".to_owned(),
                id: persisted_id,
                source_version,
            }
        }
        SourceIngress::Material {
            material_item_id,
            quantity_on_hand_milli,
            safety_stock_milli,
            ..
        } => {
            if quantity_on_hand_milli < safety_stock_milli || safety_stock_milli < 0 {
                return Err(RestError::validation(
                    "material on-hand must cover non-negative safety stock",
                ));
            }
            let persisted_id: Uuid = sqlx::query_scalar("UPDATE inventory_items SET quantity_on_hand_milli=$1, safety_stock_milli=$2, updated_at=now() WHERE id=$3 AND branch_id=$4 AND status='ACTIVE' RETURNING id")
                .bind(quantity_on_hand_milli).bind(safety_stock_milli).bind(material_item_id).bind(*branch_id.as_uuid()).fetch_optional(&mut *tx).await.map_err(RestError::db)?
                .ok_or_else(|| RestError::not_found("active material source not found"))?;
            SourceIngressReceipt {
                kind: "material".to_owned(),
                id: persisted_id,
                source_version,
            }
        }
    };
    let response = serde_json::to_value(&receipt)
        .map_err(|_| RestError::internal("source ingress receipt could not be serialized"))?;
    sqlx::query("UPDATE service_principal_ingress_claims SET response=$1, completed_at=now() WHERE org_id=$2 AND service_principal_id=$3 AND kind=$4 AND source_id=$5 AND source_version=$6")
        .bind(&response).bind(*org.as_uuid()).bind(service_principal_id).bind(kind).bind(source_id.trim()).bind(receipt.source_version.trim()).execute(&mut *tx).await.map_err(RestError::db)?;
    let audit_event = production_audit_event(
        None,
        org,
        Some(branch_id),
        "production.source_ingest",
        "production_source_ingress",
        format!("{service_principal_id}:{kind}:{}", receipt.id),
        serde_json::json!({
            "service_principal_id": service_principal_id,
            "kind": kind,
            "source_id": source_id,
            "source_version": receipt.source_version,
            "receipt_id": receipt.id,
        }),
    )?;
    insert_audit_event(&mut tx, &audit_event)
        .await
        .map_err(RestError::from)?;
    tx.commit().await.map_err(RestError::db)?;
    Ok(Json(receipt))
}

async fn create_plan(
    State(state): State<ProductionRestState>,
    headers: HeaderMap,
    Json(request): Json<CreatePlan>,
) -> Result<(StatusCode, Json<PlanSummary>), RestError> {
    validate_create(&request)?;
    let principal = principal(&state, &headers).await?;
    authorize(
        &principal,
        Action::limited(Feature::DailyPlanRequest),
        request.branch_id,
    )
    .map_err(RestError::kernel)?;
    let org = console_platform_request_context::current_org()
        .map_err(|_| RestError::internal("tenant context is missing"))?;
    let response = with_audits::<_, _, RestError>(&state.pool, org, move |tx| {
        Box::pin(async move {
            let request_hash = hash_request(&request)?;
    sqlx::query("INSERT INTO production_idempotency_claims (org_id,operation,idempotency_key,request_hash) VALUES ($1,'CREATE_PLAN',$2,$3) ON CONFLICT DO NOTHING")
        .bind(*org.as_uuid()).bind(request.idempotency_key.trim()).bind(&request_hash).execute(tx.as_mut()).await.map_err(RestError::db)?;
    let claim = sqlx::query("SELECT request_hash,response FROM production_idempotency_claims WHERE org_id=$1 AND operation='CREATE_PLAN' AND idempotency_key=$2 FOR UPDATE")
        .bind(*org.as_uuid()).bind(request.idempotency_key.trim()).fetch_one(tx.as_mut()).await.map_err(RestError::db)?;
    let stored_hash: String = claim.try_get("request_hash").map_err(RestError::db)?;
    if stored_hash != request_hash {
        return Err(RestError::conflict(
            "idempotency key was already used for a different request",
        ));
    }
    if let Some(response) = claim
        .try_get::<Option<serde_json::Value>, _>("response")
        .map_err(RestError::db)?
    {
        let plan: PlanSummary = serde_json::from_value(response)
            .map_err(|_| RestError::internal("stored idempotency response is invalid"))?;
            return Ok(((StatusCode::OK, Json(plan)), Vec::new()));
    }
    let plan_id = Uuid::new_v4();
    let operation_id = Uuid::new_v4();
    let now = OffsetDateTime::now_utc();
    let sources = resolve_required_sources(tx, &request, *org.as_uuid()).await?;
    let plan_digest = hash_request(&(
        plan_id,
        request.branch_id,
        request.customer_demand_id,
        &sources.product_code,
        request.quantity,
        request.due_at,
        &sources.checks,
        &sources.snapshot,
        request.ontology_type_id,
    ))?;
    sqlx::query("INSERT INTO production_plans (id, org_id, branch_id, customer_demand_id, product_code, quantity, due_at, checks, source_snapshot, idempotency_key, ontology_type_id, first_operation_id, created_by, created_at, updated_at, plan_digest) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$14,$15)")
        .bind(plan_id).bind(*org.as_uuid()).bind(*request.branch_id.as_uuid()).bind(request.customer_demand_id).bind(&sources.product_code).bind(request.quantity).bind(request.due_at).bind(&sources.checks).bind(&sources.snapshot).bind(request.idempotency_key.trim()).bind(request.ontology_type_id).bind(operation_id).bind(*principal.user_id.as_uuid()).bind(now).bind(&plan_digest).execute(tx.as_mut()).await.map_err(RestError::db)?;
    sqlx::query("INSERT INTO production_operations (id, org_id, plan_id, sequence, status) VALUES ($1,$2,$3,1,'PENDING')").bind(operation_id).bind(*org.as_uuid()).bind(plan_id).execute(tx.as_mut()).await.map_err(RestError::db)?;
    event(
        tx,
        *org.as_uuid(),
        plan_id,
        principal.user_id.as_uuid(),
        "PLAN_CREATED",
        sources.snapshot,
    )
    .await?;
    let plan = PlanSummary {
        id: plan_id,
        branch_id: *request.branch_id.as_uuid(),
        customer_demand_id: request.customer_demand_id,
        product_code: sources.product_code,
        quantity: request.quantity,
        status: "DRAFT".to_owned(),
        version: 1,
        first_operation_id: operation_id,
        created_at: now,
        due_at: request.due_at,
        plan_digest,
    };
    sqlx::query("UPDATE production_idempotency_claims SET response=$1,completed_at=now() WHERE org_id=$2 AND operation='CREATE_PLAN' AND idempotency_key=$3")
        .bind(serde_json::to_value(&plan).map_err(|_| RestError::internal("could not serialize idempotency response"))?)
        .bind(*org.as_uuid()).bind(request.idempotency_key.trim()).execute(tx.as_mut()).await.map_err(RestError::db)?;
            let audit_event = production_audit_event(
                Some(principal.user_id),
                org,
                Some(request.branch_id),
                "production.plan_create",
                "production_plan",
                plan_id,
                serde_json::json!({"plan_id": plan_id, "quantity": plan.quantity, "due_at": plan.due_at}),
            )?;
            Ok(((StatusCode::CREATED, Json(plan)), vec![audit_event]))
        })
    })
    .await?;
    Ok(response)
}

async fn release_plan(
    State(state): State<ProductionRestState>,
    headers: HeaderMap,
    Path(plan_id): Path<Uuid>,
    Json(request): Json<ReleasePlan>,
) -> Result<Json<PlanSummary>, RestError> {
    valid_key(&request.idempotency_key)?;
    let principal = principal(&state, &headers).await?;
    let plan = plan_for_auth(&state.pool, plan_id).await?;
    authorize(
        &principal,
        Action::limited(Feature::DailyPlanReview),
        BranchId::from_uuid(plan.branch_id),
    )
    .map_err(RestError::kernel)?;
    let org = console_platform_request_context::current_org()
        .map_err(|_| RestError::internal("tenant context is missing"))?;
    let response = with_audits::<_, _, RestError>(&state.pool, org, move |tx| {
        Box::pin(async move {
    let request_hash = hash_request(&(plan_id, &request))?;
    if let Some(response) = claim_or_replay(
        tx,
        *org.as_uuid(),
        "RELEASE_PLAN",
        request.idempotency_key.trim(),
        &request_hash,
    )
    .await?
    {
        let plan: PlanSummary = serde_json::from_value(response)
            .map_err(|_| RestError::internal("stored idempotency response is invalid"))?;
            return Ok((Json(plan), Vec::new()));
    }
    {
        let approval_kind = format!("release:v{}:{}", request.expected_version, plan.plan_digest);
        // No row lock: `gov_approvals` is append-only and 0153:140 revokes UPDATE from
        // console_rt, which is exactly the privilege `FOR UPDATE` demands — the runtime
        // role could never run this read. Single use is owned by the
        // `gov_approval_consumptions` UNIQUE (org_id, approval_id) insert below
        // (0164:16-18), whose 23505 this handler already maps to a conflict.
        let approval = sqlx::query("SELECT requested_by, approver_id FROM gov_approvals WHERE id=$1 AND decision='approved' AND kind=$2 AND target_ref=$3")
            .bind(request.approval_ref).bind(&approval_kind).bind(plan_id).fetch_optional(tx.as_mut()).await.map_err(RestError::db)?
            .ok_or_else(|| RestError::conflict("release requires an approved plan-bound approval"))?;
        let requested_by: Uuid = approval.try_get("requested_by").map_err(RestError::db)?;
        let approver_id: Uuid = approval.try_get("approver_id").map_err(RestError::db)?;
        if requested_by != plan.created_by
            || approver_id != *principal.user_id.as_uuid()
            || approver_id == requested_by
        {
            return Err(RestError::conflict(
                "release approval is not bound to this planner and reviewer",
            ));
        }
        sqlx::query("INSERT INTO gov_approval_consumptions (org_id,approval_id,consumed_by) VALUES ($1,$2,$3)")
            .bind(*org.as_uuid()).bind(request.approval_ref).bind(*principal.user_id.as_uuid()).execute(tx.as_mut()).await.map_err(|error| {
                if error.as_database_error().and_then(|database| database.code()).is_some_and(|code| code == "23505") { RestError::conflict("production release approval was already consumed") } else { RestError::db(error) }
            })?;
        let updated = sqlx::query("UPDATE production_plans SET status='RELEASED', version=version+1, updated_at=now(), released_at=now(), released_by=$1, approval_ref=$2 WHERE id=$3 AND status='DRAFT' AND version=$4").bind(*principal.user_id.as_uuid()).bind(request.approval_ref).bind(plan_id).bind(request.expected_version).execute(tx.as_mut()).await.map_err(RestError::db)?;
        if updated.rows_affected() != 1 {
            return Err(RestError::conflict(
                "plan release conflicts with current lifecycle or version",
            ));
        }
        sqlx::query("UPDATE production_operations SET status='RELEASED', version=version+1 WHERE id=$1 AND status='PENDING'").bind(plan.first_operation_id).execute(tx.as_mut()).await.map_err(RestError::db)?;
        event_with_key(
            tx,
            *org.as_uuid(),
            plan_id,
            principal.user_id.as_uuid(),
            "PLAN_RELEASED",
            serde_json::json!({"version": request.expected_version + 1, "approval_ref": request.approval_ref}),
            request.idempotency_key.trim(),
        )
        .await?;
    }
    let plan = plan_for_auth_tx(tx, plan_id).await?;
    store_claim_response(
        tx,
        *org.as_uuid(),
        "RELEASE_PLAN",
        request.idempotency_key.trim(),
        &plan_summary_from_plan(&plan),
    )
    .await?;
            let summary = plan_summary_from_plan(&plan);
            let audit_event = production_audit_event(
                Some(principal.user_id),
                org,
                Some(BranchId::from_uuid(plan.branch_id)),
                "production.plan_release",
                "production_plan",
                plan_id,
                serde_json::json!({"plan_id": plan_id, "version": summary.version, "approval_ref": request.approval_ref}),
            )?;
            Ok((Json(summary), vec![audit_event]))
        })
    })
    .await?;
    Ok(response)
}

async fn record_operation(
    State(state): State<ProductionRestState>,
    headers: HeaderMap,
    Path((plan_id, operation_id)): Path<(Uuid, Uuid)>,
    Json(request): Json<RecordOperation>,
) -> Result<Json<OperationDetail>, RestError> {
    valid_key(&request.idempotency_key)?;
    if request.output_quantity < 0
        || request.scrap_quantity < 0
        || request.downtime_minutes < 0
        || request.note.len() > 500
        || request.quality_evidence_ref.trim().is_empty()
    {
        return Err(RestError::validation(
            "operation quantities must be non-negative and quality evidence is required",
        ));
    }
    let principal = principal(&state, &headers).await?;
    let plan = plan_for_auth(&state.pool, plan_id).await?;
    authorize(
        &principal,
        Action::limited(Feature::WorkReportSubmit),
        BranchId::from_uuid(plan.branch_id),
    )
    .map_err(RestError::kernel)?;
    let org = console_platform_request_context::current_org()
        .map_err(|_| RestError::internal("tenant context is missing"))?;
    let response = with_audits::<_, _, RestError>(&state.pool, org, move |tx| {
        Box::pin(async move {
    let request_hash = hash_request(&(plan_id, operation_id, &request))?;
    if let Some(response) = claim_or_replay(
        tx,
        *org.as_uuid(),
        "RECORD_OPERATION",
        request.idempotency_key.trim(),
        &request_hash,
    )
    .await?
    {
        let operation: OperationDetail = serde_json::from_value(response)
            .map_err(|_| RestError::internal("stored idempotency response is invalid"))?;
            return Ok((Json(operation), Vec::new()));
    }
    {
        let updated = sqlx::query("UPDATE production_operations SET output_quantity=output_quantity+$1, scrap_quantity=scrap_quantity+$2, downtime_minutes=downtime_minutes+$3, quality_evidence_ref=$4, quality_passed=$5, status='RECORDED', version=version+1 WHERE id=$6 AND plan_id=$7 AND status='RELEASED' AND version=$8").bind(request.output_quantity).bind(request.scrap_quantity).bind(request.downtime_minutes).bind(request.quality_evidence_ref.trim()).bind(request.quality_passed).bind(operation_id).bind(plan_id).bind(request.expected_version).execute(tx.as_mut()).await.map_err(RestError::db)?;
        if updated.rows_affected() != 1 {
            return Err(RestError::conflict(
                "operation record conflicts with current lifecycle or version",
            ));
        }
        event_with_key(tx, *org.as_uuid(), plan_id, principal.user_id.as_uuid(), "OPERATION_RECORDED", serde_json::json!({"operation_id": operation_id, "output_quantity": request.output_quantity, "scrap_quantity": request.scrap_quantity, "downtime_minutes": request.downtime_minutes, "quality_evidence_ref": request.quality_evidence_ref, "quality_passed": request.quality_passed, "note": request.note}), request.idempotency_key.trim()).await?;
    }
    let operation = operation_detail_tx(tx, operation_id).await?;
    store_claim_response(
        tx,
        *org.as_uuid(),
        "RECORD_OPERATION",
        request.idempotency_key.trim(),
        &operation,
    )
    .await?;
            let audit_event = production_audit_event(
                Some(principal.user_id),
                org,
                Some(BranchId::from_uuid(plan.branch_id)),
                "production.operation_record",
                "production_operation",
                operation_id,
                serde_json::json!({"plan_id": plan_id, "operation_id": operation_id, "output_quantity": request.output_quantity, "scrap_quantity": request.scrap_quantity, "downtime_minutes": request.downtime_minutes, "quality_passed": request.quality_passed}),
            )?;
            Ok((Json(operation), vec![audit_event]))
        })
    })
    .await?;
    Ok(response)
}

async fn get_plan(
    State(state): State<ProductionRestState>,
    headers: HeaderMap,
    Path(plan_id): Path<Uuid>,
) -> Result<Json<PlanDetail>, RestError> {
    let principal = principal(&state, &headers).await?;
    let plan = plan_for_auth(&state.pool, plan_id).await?;
    authorize_daily_plan_read(&principal, BranchId::from_uuid(plan.branch_id))?;
    let org = console_platform_request_context::current_org()
        .map_err(|_| RestError::internal("tenant context is missing"))?;
    let mut tx = state.pool.begin().await.map_err(RestError::db)?;
    arm_tenant(&mut tx, *org.as_uuid()).await?;
    let events = sqlx::query("SELECT id,event_type,actor_id,payload,occurred_at FROM production_plan_events WHERE plan_id=$1 ORDER BY occurred_at,id").bind(plan_id).fetch_all(&mut *tx).await.map_err(RestError::db)?.into_iter().map(|row| Ok(LifecycleEvent{id:row.try_get("id")?,event_type:row.try_get("event_type")?,actor_id:row.try_get("actor_id")?,payload:row.try_get("payload")?,occurred_at:row.try_get("occurred_at")?})).collect::<Result<Vec<_>,sqlx::Error>>().map_err(RestError::db)?;
    tx.commit().await.map_err(RestError::db)?;
    Ok(Json(PlanDetail {
        plan: PlanSummary {
            id: plan.id,
            branch_id: plan.branch_id,
            customer_demand_id: plan.customer_demand_id,
            product_code: plan.product_code,
            quantity: plan.quantity,
            status: plan.status,
            version: plan.version,
            first_operation_id: plan.first_operation_id,
            created_at: plan.created_at,
            due_at: plan.due_at,
            plan_digest: plan.plan_digest.clone(),
        },
        checks: plan.checks,
        events,
        operation: operation_detail(&state.pool, plan.first_operation_id).await?,
    }))
}

struct PlanRow {
    id: Uuid,
    branch_id: Uuid,
    customer_demand_id: Uuid,
    product_code: String,
    quantity: i64,
    status: String,
    version: i32,
    first_operation_id: Uuid,
    created_at: OffsetDateTime,
    due_at: OffsetDateTime,
    created_by: Uuid,
    plan_digest: String,
    checks: serde_json::Value,
}
struct ResolvedSources {
    product_code: String,
    checks: serde_json::Value,
    snapshot: serde_json::Value,
}

/// Resolves every planning prerequisite from its owning tenant store under the
/// same transaction as the capacity reservation. A missing or stale source is
/// an availability failure, never a caller-controlled green check.
async fn resolve_required_sources(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    request: &CreatePlan,
    org_id: Uuid,
) -> Result<ResolvedSources, RestError> {
    let demand = sqlx::query(
        // `FOR UPDATE OF d` locks the demand row, and only it — the joined inquiry is
        // read-only here and console_rt holds no UPDATE on it. This lock is what makes
        // the "planned exactly once" count below atomic; without it two concurrent
        // creates for one demand both read 0 under READ COMMITTED. console_rt holds
        // UPDATE on production_demand_contracts (0176:83 re-grants what 0173:147 revoked).
        "SELECT d.id,d.product_code,d.quantity,d.due_at,d.source_system,d.source_id,d.source_version,d.evaluated_at FROM production_demand_contracts d JOIN customer_inquiries i ON i.id=d.inquiry_id WHERE d.id=$1 AND i.status <> 'CLOSED' FOR UPDATE OF d",
    )
    .bind(request.customer_demand_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(RestError::db)?
    .ok_or_else(|| RestError::unavailable("customer demand source is unavailable"))?;
    let demand_product: String = demand.try_get("product_code").map_err(RestError::db)?;
    let demand_quantity: i64 = demand.try_get("quantity").map_err(RestError::db)?;
    let demand_due_at: OffsetDateTime = demand.try_get("due_at").map_err(RestError::db)?;
    if demand_quantity != request.quantity || demand_due_at != request.due_at {
        return Err(RestError::unavailable(
            "demand contract does not match quantity and due date",
        ));
    }
    // A demand contract is planned in full exactly once — the quantity check above
    // admits only a plan for the whole contract, so a second idempotency key would
    // reserve capacity and material for the same demand twice. Serialized by the
    // demand row lock taken above, which this count must stay after.
    let already_planned: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM production_plans WHERE org_id=$1 AND customer_demand_id=$2",
    )
    .bind(org_id)
    .bind(request.customer_demand_id)
    .fetch_one(&mut **tx)
    .await
    .map_err(RestError::db)?;
    if already_planned > 0 {
        return Err(RestError::conflict(
            "customer demand is already committed to a production plan",
        ));
    }
    let material = sqlx::query("SELECT id, iv_code, quantity_on_hand_milli, safety_stock_milli, updated_at FROM inventory_items WHERE id=$1 AND branch_id=$2 AND status='ACTIVE' FOR UPDATE")
        .bind(request.material_item_id).bind(*request.branch_id.as_uuid()).fetch_optional(&mut **tx).await.map_err(RestError::db)?
        .ok_or_else(|| RestError::unavailable("material source is unavailable"))?;
    let on_hand: i64 = material
        .try_get("quantity_on_hand_milli")
        .map_err(RestError::db)?;
    let material_product: String = material.try_get("iv_code").map_err(RestError::db)?;
    if material_product != demand_product {
        return Err(RestError::unavailable(
            "material does not match the demand contract product",
        ));
    }
    let safety: i64 = material
        .try_get("safety_stock_milli")
        .map_err(RestError::db)?;
    if on_hand - safety < request.quantity {
        return Err(RestError::unavailable(
            "material source has no allocable stock",
        ));
    }
    let capacity = sqlx::query("SELECT id, available_quantity, reserved_quantity, version, source_system, source_id, source_version, evaluated_at FROM production_capacity_slots WHERE id=$1 AND branch_id=$2 AND capacity_date = $3::date FOR UPDATE")
        .bind(request.capacity_slot_id).bind(*request.branch_id.as_uuid()).bind(request.due_at.date()).fetch_optional(&mut **tx).await.map_err(RestError::db)?
        .ok_or_else(|| RestError::unavailable("capacity source is unavailable"))?;
    let available: i64 = capacity
        .try_get("available_quantity")
        .map_err(RestError::db)?;
    let reserved: i64 = capacity
        .try_get("reserved_quantity")
        .map_err(RestError::db)?;
    if available - reserved < request.quantity {
        return Err(RestError::unavailable(
            "capacity source has insufficient available quantity",
        ));
    }
    let staffing: i64 = sqlx::query_scalar("SELECT count(*) FROM users u JOIN user_branches ub ON ub.user_id=u.id AND ub.org_id=u.org_id WHERE ub.branch_id=$1 AND u.is_active=true")
        .bind(*request.branch_id.as_uuid()).fetch_one(&mut **tx).await.map_err(RestError::db)?;
    if staffing < 1 {
        return Err(RestError::unavailable(
            "staffing source has no active assignee",
        ));
    }
    let ontology = sqlx::query("SELECT id, stable_key, schema_version, updated_at FROM ont_object_types WHERE id=$1 AND lifecycle_state='published'")
        .bind(request.ontology_type_id).fetch_optional(&mut **tx).await.map_err(RestError::db)?
        .ok_or_else(|| RestError::unavailable("ontology source is unavailable"))?;
    let capacity_version: i32 = capacity.try_get("version").map_err(RestError::db)?;
    let updated = sqlx::query("UPDATE production_capacity_slots SET reserved_quantity=reserved_quantity+$1, version=version+1, updated_at=now() WHERE id=$2 AND org_id=$3 AND version=$4")
        .bind(request.quantity).bind(request.capacity_slot_id).bind(org_id).bind(capacity_version).execute(&mut **tx).await.map_err(RestError::db)?;
    if updated.rows_affected() != 1 {
        return Err(RestError::conflict(
            "capacity source changed while reserving plan",
        ));
    }
    let material_updated = sqlx::query("UPDATE inventory_items SET quantity_on_hand_milli=quantity_on_hand_milli-$1, updated_at=now() WHERE id=$2 AND branch_id=$3 AND status='ACTIVE' AND updated_at=$4 AND quantity_on_hand_milli-safety_stock_milli >= $1")
        .bind(request.quantity).bind(request.material_item_id).bind(*request.branch_id.as_uuid()).bind(material.try_get::<OffsetDateTime,_>("updated_at").map_err(RestError::db)?).execute(&mut **tx).await.map_err(RestError::db)?;
    if material_updated.rows_affected() != 1 {
        return Err(RestError::conflict(
            "material source changed while reserving plan",
        ));
    }
    let checks = serde_json::json!({"capacity_ok":true,"material_ok":true,"staffing_ok":true});
    Ok(ResolvedSources {
        product_code: demand_product,
        checks,
        snapshot: serde_json::json!({
          "demand":{"id":demand.try_get::<Uuid,_>("id").map_err(RestError::db)?,"source_id":demand.try_get::<String,_>("source_id").map_err(RestError::db)?,"version":demand.try_get::<String,_>("source_version").map_err(RestError::db)?,"evaluated_at":demand.try_get::<OffsetDateTime,_>("evaluated_at").map_err(RestError::db)?,"result":"matched"},
          "capacity":{"id":request.capacity_slot_id,"source_id":capacity.try_get::<String,_>("source_id").map_err(RestError::db)?,"version":capacity.try_get::<String,_>("source_version").map_err(RestError::db)?,"evaluated_at":capacity.try_get::<OffsetDateTime,_>("evaluated_at").map_err(RestError::db)?,"result":"reserved"},
          "material":{"id":request.material_item_id,"version":material.try_get::<OffsetDateTime,_>("updated_at").map_err(RestError::db)?,"evaluated_at":OffsetDateTime::now_utc(),"result":"reserved"},
          "staffing":{"id":request.branch_id,"version":staffing,"evaluated_at":OffsetDateTime::now_utc(),"result":"available"},
          "ontology":{"id":request.ontology_type_id,"stable_key":ontology.try_get::<String,_>("stable_key").map_err(RestError::db)?,"version":ontology.try_get::<i64,_>("schema_version").map_err(RestError::db)?,"evaluated_at":ontology.try_get::<OffsetDateTime,_>("updated_at").map_err(RestError::db)?,"result":"published"}
        }),
    })
}
async fn plan_for_auth(pool: &PgPool, id: Uuid) -> Result<PlanRow, RestError> {
    let org = console_platform_request_context::current_org()
        .map_err(|_| RestError::internal("tenant context is missing"))?;
    let mut tx = pool.begin().await.map_err(RestError::db)?;
    arm_tenant(&mut tx, *org.as_uuid()).await?;
    let plan = plan_for_auth_tx(&mut tx, id).await?;
    tx.commit().await.map_err(RestError::db)?;
    Ok(plan)
}
async fn plan_for_auth_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    id: Uuid,
) -> Result<PlanRow, RestError> {
    let r=sqlx::query("SELECT id,branch_id,customer_demand_id,product_code,quantity,status,version,first_operation_id,created_at,due_at,checks,created_by,plan_digest FROM production_plans WHERE id=$1").bind(id).fetch_optional(&mut **tx).await.map_err(RestError::db)?.ok_or_else(||RestError::not_found("production plan not found"))?;
    Ok(PlanRow {
        id: r.try_get("id").map_err(RestError::db)?,
        branch_id: r.try_get("branch_id").map_err(RestError::db)?,
        customer_demand_id: r.try_get("customer_demand_id").map_err(RestError::db)?,
        product_code: r.try_get("product_code").map_err(RestError::db)?,
        quantity: r.try_get("quantity").map_err(RestError::db)?,
        status: r.try_get("status").map_err(RestError::db)?,
        version: r.try_get("version").map_err(RestError::db)?,
        first_operation_id: r.try_get("first_operation_id").map_err(RestError::db)?,
        created_at: r.try_get("created_at").map_err(RestError::db)?,
        due_at: r.try_get("due_at").map_err(RestError::db)?,
        created_by: r.try_get("created_by").map_err(RestError::db)?,
        plan_digest: r.try_get("plan_digest").map_err(RestError::db)?,
        checks: r.try_get("checks").map_err(RestError::db)?,
    })
}
async fn operation_detail(pool: &PgPool, id: Uuid) -> Result<OperationDetail, RestError> {
    let org = console_platform_request_context::current_org()
        .map_err(|_| RestError::internal("tenant context is missing"))?;
    let mut tx = pool.begin().await.map_err(RestError::db)?;
    arm_tenant(&mut tx, *org.as_uuid()).await?;
    let operation = operation_detail_tx(&mut tx, id).await?;
    tx.commit().await.map_err(RestError::db)?;
    Ok(operation)
}
async fn operation_detail_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    id: Uuid,
) -> Result<OperationDetail, RestError> {
    let r=sqlx::query("SELECT id,sequence,status,output_quantity,scrap_quantity,downtime_minutes,quality_evidence_ref,quality_passed,version FROM production_operations WHERE id=$1").bind(id).fetch_one(&mut **tx).await.map_err(RestError::db)?;
    Ok(OperationDetail {
        id: r.try_get("id").map_err(RestError::db)?,
        sequence: r.try_get("sequence").map_err(RestError::db)?,
        status: r.try_get("status").map_err(RestError::db)?,
        output_quantity: r.try_get("output_quantity").map_err(RestError::db)?,
        scrap_quantity: r.try_get("scrap_quantity").map_err(RestError::db)?,
        downtime_minutes: r.try_get("downtime_minutes").map_err(RestError::db)?,
        quality_evidence_ref: r.try_get("quality_evidence_ref").map_err(RestError::db)?,
        quality_passed: r.try_get("quality_passed").map_err(RestError::db)?,
        version: r.try_get("version").map_err(RestError::db)?,
    })
}
fn plan_summary(r: sqlx::postgres::PgRow) -> Result<PlanSummary, RestError> {
    Ok(PlanSummary {
        id: r.try_get("id").map_err(RestError::db)?,
        branch_id: r.try_get("branch_id").map_err(RestError::db)?,
        customer_demand_id: r.try_get("customer_demand_id").map_err(RestError::db)?,
        product_code: r.try_get("product_code").map_err(RestError::db)?,
        quantity: r.try_get("quantity").map_err(RestError::db)?,
        status: r.try_get("status").map_err(RestError::db)?,
        version: r.try_get("version").map_err(RestError::db)?,
        first_operation_id: r.try_get("first_operation_id").map_err(RestError::db)?,
        created_at: r.try_get("created_at").map_err(RestError::db)?,
        due_at: r.try_get("due_at").map_err(RestError::db)?,
        plan_digest: r.try_get("plan_digest").map_err(RestError::db)?,
    })
}
fn capacity_slot(r: sqlx::postgres::PgRow) -> Result<CapacitySlot, RestError> {
    Ok(CapacitySlot {
        id: r.try_get("id").map_err(RestError::db)?,
        branch_id: r.try_get("branch_id").map_err(RestError::db)?,
        site_id: r.try_get("site_id").map_err(RestError::db)?,
        capacity_date: r.try_get("capacity_date").map_err(RestError::db)?,
        available_quantity: r.try_get("available_quantity").map_err(RestError::db)?,
        reserved_quantity: r.try_get("reserved_quantity").map_err(RestError::db)?,
        version: r.try_get("version").map_err(RestError::db)?,
        source_ref: r.try_get("source_ref").map_err(RestError::db)?,
        evaluated_at: r.try_get("evaluated_at").map_err(RestError::db)?,
    })
}
fn plan_summary_from_plan(plan: &PlanRow) -> PlanSummary {
    PlanSummary {
        id: plan.id,
        branch_id: plan.branch_id,
        customer_demand_id: plan.customer_demand_id,
        product_code: plan.product_code.clone(),
        quantity: plan.quantity,
        status: plan.status.clone(),
        version: plan.version,
        first_operation_id: plan.first_operation_id,
        created_at: plan.created_at,
        due_at: plan.due_at,
        plan_digest: plan.plan_digest.clone(),
    }
}
fn production_audit_event(
    actor: Option<UserId>,
    org: OrgId,
    branch: Option<BranchId>,
    action: &str,
    target_type: &str,
    target_id: impl Into<String>,
    after: serde_json::Value,
) -> Result<AuditEvent, RestError> {
    let event = AuditEvent::new(
        actor,
        AuditAction::new(action).map_err(RestError::kernel)?,
        target_type,
        target_id,
        TraceContext::generate(),
        OffsetDateTime::now_utc(),
    )
    .with_org(org)
    .with_snapshots(None, Some(after));
    Ok(match branch {
        Some(branch) => event.with_branch(branch),
        None => event,
    })
}

async fn event(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    org: Uuid,
    plan: Uuid,
    actor: &Uuid,
    kind: &str,
    payload: serde_json::Value,
) -> Result<(), RestError> {
    event_with_key(
        tx,
        org,
        plan,
        actor,
        kind,
        payload,
        &Uuid::new_v4().to_string(),
    )
    .await
}
async fn event_with_key(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    org: Uuid,
    plan: Uuid,
    actor: &Uuid,
    kind: &str,
    payload: serde_json::Value,
    key: &str,
) -> Result<(), RestError> {
    sqlx::query("INSERT INTO production_plan_events (id,org_id,plan_id,event_type,actor_id,payload,idempotency_key) VALUES ($1,$2,$3,$4,$5,$6,$7)").bind(Uuid::new_v4()).bind(org).bind(plan).bind(kind).bind(actor).bind(payload).bind(key).execute(&mut **tx).await.map_err(RestError::db)?;
    Ok(())
}
async fn arm_tenant(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    org: Uuid,
) -> Result<(), RestError> {
    sqlx::query("SELECT set_config('app.current_org', $1, true)")
        .bind(org.to_string())
        .execute(&mut **tx)
        .await
        .map_err(RestError::db)?;
    Ok(())
}
fn validate_create(r: &CreatePlan) -> Result<(), RestError> {
    valid_key(&r.idempotency_key)?;
    if r.quantity <= 0 {
        return Err(RestError::validation("quantity must be positive"));
    }
    Ok(())
}
fn valid_key(k: &str) -> Result<(), RestError> {
    if k.trim().is_empty() || k.len() > 128 {
        Err(RestError::validation(
            "idempotency_key is required and must be at most 128 characters",
        ))
    } else {
        Ok(())
    }
}
fn hash_request<T: Serialize>(request: &T) -> Result<String, RestError> {
    let encoded = serde_json::to_vec(request)
        .map_err(|_| RestError::internal("could not canonicalize idempotency request"))?;
    Ok(Sha256::digest(encoded)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}
fn generated_secret() -> [u8; 32] {
    let first = Uuid::new_v4();
    let second = Uuid::new_v4();
    let mut secret = [0_u8; 32];
    secret[..16].copy_from_slice(first.as_bytes());
    secret[16..].copy_from_slice(second.as_bytes());
    secret
}
#[cfg(test)]
mod digest_tests {
    use super::{
        SourceIngress, SourceSystemCredential, disabled_source_system_receipt, hash_request,
    };
    use base64::Engine as _;
    use time::{Duration, OffsetDateTime};
    use uuid::Uuid;

    #[test]
    fn idempotency_digest_is_canonical_lowercase_sha256_bytes() {
        assert_eq!(
            hash_request(&"abc").expect("serializable request"),
            "6cc43f858fbb763301637b5af970e2a46b46f461f27e5a0f41e009c59b827b25"
        );
    }

    #[test]
    fn credential_dto_discloses_only_base64_server_secret_shape() {
        let secret = [7_u8; 32];
        let dto = SourceSystemCredential {
            id: Uuid::nil(),
            source_system: "ERP".to_owned(),
            enabled: true,
            credential_generation: 3,
            secret: base64::engine::general_purpose::STANDARD.encode(secret),
        };
        let value = serde_json::to_value(dto).expect("credential serializes");
        assert_eq!(value["id"], Uuid::nil().to_string());
        assert_eq!(value["source_system"], "ERP");
        assert_eq!(value["credential_generation"], 3);
        assert_eq!(
            base64::engine::general_purpose::STANDARD
                .decode(value["secret"].as_str().unwrap())
                .unwrap()
                .len(),
            32
        );
        assert!(value.get("verifier").is_none());
        assert!(value.get("mac").is_none());
    }

    #[test]
    fn ingress_wire_format_remains_tagged_and_disable_receipt_is_explicit() {
        let ingress = SourceIngress::Demand {
            id: Uuid::nil(),
            inquiry_id: Uuid::from_u128(1),
            product_code: "PROD-1".to_owned(),
            quantity: 1,
            due_at: OffsetDateTime::UNIX_EPOCH + Duration::days(1),
            source_id: "demand-1".to_owned(),
            source_version: "v1".to_owned(),
        };
        let value = serde_json::to_value(ingress).expect("ingress serializes");
        assert_eq!(value["kind"], "demand");
        assert!(value.get("source_system").is_none());
        let disabled = disabled_source_system_receipt(Uuid::nil(), 3);
        let disabled = serde_json::to_value(disabled).expect("receipt serializes");
        assert_eq!(disabled["enabled"], false);
        assert_eq!(disabled["credential_generation"], 3);
    }
}
async fn claim_or_replay(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    org: Uuid,
    operation: &str,
    key: &str,
    request_hash: &str,
) -> Result<Option<serde_json::Value>, RestError> {
    sqlx::query("INSERT INTO production_idempotency_claims (org_id,operation,idempotency_key,request_hash) VALUES ($1,$2,$3,$4) ON CONFLICT DO NOTHING")
        .bind(org).bind(operation).bind(key).bind(request_hash).execute(&mut **tx).await.map_err(RestError::db)?;
    let claim = sqlx::query("SELECT request_hash,response FROM production_idempotency_claims WHERE org_id=$1 AND operation=$2 AND idempotency_key=$3 FOR UPDATE")
        .bind(org).bind(operation).bind(key).fetch_one(&mut **tx).await.map_err(RestError::db)?;
    let stored_hash: String = claim.try_get("request_hash").map_err(RestError::db)?;
    if stored_hash != request_hash {
        return Err(RestError::conflict(
            "idempotency key was already used for a different request",
        ));
    }
    claim.try_get("response").map_err(RestError::db)
}
async fn store_claim_response<T: Serialize>(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    org: Uuid,
    operation: &str,
    key: &str,
    response: &T,
) -> Result<(), RestError> {
    let response = serde_json::to_value(response)
        .map_err(|_| RestError::internal("could not serialize idempotency response"))?;
    sqlx::query("UPDATE production_idempotency_claims SET response=$1,completed_at=now() WHERE org_id=$2 AND operation=$3 AND idempotency_key=$4")
        .bind(response).bind(org).bind(operation).bind(key).execute(&mut **tx).await.map_err(RestError::db)?;
    Ok(())
}
async fn principal(
    state: &ProductionRestState,
    headers: &HeaderMap,
) -> Result<Principal, RestError> {
    let verifier = state
        .jwt_verifier
        .as_ref()
        .ok_or_else(|| RestError::internal("JWT verification is not configured"))?;
    console_platform_request_context::resolve_principal(verifier, &state.pool, headers)
        .await
        .map_err(|_| RestError::unauthorized("missing, invalid, or unauthorized bearer token"))
}
fn authorize_daily_plan_read(principal: &Principal, branch_id: BranchId) -> Result<(), RestError> {
    authorize(
        principal,
        Action::limited(Feature::DailyPlanRequest),
        branch_id,
    )
    .or_else(|_| {
        authorize(
            principal,
            Action::limited(Feature::DailyPlanReview),
            branch_id,
        )
    })
    .map_err(RestError::kernel)
}
#[derive(Debug)]
struct RestError {
    status: StatusCode,
    kind: ErrorKind,
    message: String,
}
impl RestError {
    fn validation(m: impl Into<String>) -> Self {
        Self {
            status: StatusCode::UNPROCESSABLE_ENTITY,
            kind: ErrorKind::Validation,
            message: m.into(),
        }
    }
    fn unauthorized(m: impl Into<String>) -> Self {
        Self {
            status: StatusCode::UNAUTHORIZED,
            kind: ErrorKind::Forbidden,
            message: m.into(),
        }
    }
    fn internal(m: impl Into<String>) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            kind: ErrorKind::Internal,
            message: m.into(),
        }
    }
    fn unavailable(m: impl Into<String>) -> Self {
        Self {
            status: StatusCode::SERVICE_UNAVAILABLE,
            kind: ErrorKind::Internal,
            message: m.into(),
        }
    }
    fn not_found(m: impl Into<String>) -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            kind: ErrorKind::NotFound,
            message: m.into(),
        }
    }
    fn conflict(m: impl Into<String>) -> Self {
        Self {
            status: StatusCode::CONFLICT,
            kind: ErrorKind::Conflict,
            message: m.into(),
        }
    }
    fn kernel(e: KernelError) -> Self {
        Self {
            status: match e.kind {
                ErrorKind::Forbidden => StatusCode::FORBIDDEN,
                ErrorKind::NotFound => StatusCode::NOT_FOUND,
                ErrorKind::Conflict => StatusCode::CONFLICT,
                ErrorKind::Validation => StatusCode::UNPROCESSABLE_ENTITY,
                _ => StatusCode::INTERNAL_SERVER_ERROR,
            },
            kind: e.kind,
            message: e.message,
        }
    }
    fn db(e: sqlx::Error) -> Self {
        if matches!(e, sqlx::Error::RowNotFound) {
            Self::not_found("production record not found")
        } else {
            Self::internal("production persistence failed")
        }
    }
}
impl From<DbError> for RestError {
    fn from(_error: DbError) -> Self {
        Self::internal("production persistence failed")
    }
}

impl IntoResponse for RestError {
    fn into_response(self) -> Response {
        (self.status,Json(serde_json::json!({"error":{"code":format!("{:?}",self.kind).to_lowercase(),"message":self.message}}))).into_response()
    }
}
