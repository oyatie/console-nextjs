//! CAP-IFM-PILOT facilities boundary. It intentionally does not depend on the
//! legacy equipment-required work-order model.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

mod openapi;
pub use openapi::OPENAPI_FRAGMENT;

use axum::{
    Json, Router,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use console_kernel_core::{
    AuditAction, AuditEvent, BranchId, BranchScope, KernelError, OrgId, TraceContext,
};
use console_platform_auth::JwtVerifier;
use console_platform_authz::{Action, Feature, Principal, authorize, authorize_capability};
use console_platform_db::{DbError, with_audit, with_audits, with_org_conn};
use console_platform_request_context::{RequestContextError, resolve_principal};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Postgres, Row, Transaction};
use uuid::Uuid;

pub const FACILITIES_CASES_PATH: &str = "/api/v1/facilities/cases";
pub const FACILITIES_CASE_PATH: &str = "/api/v1/facilities/cases/{case_id}";
pub const FACILITIES_TRIAGE_PATH: &str = "/api/v1/facilities/cases/{case_id}/triage";
pub const FACILITIES_ASSIGN_PATH: &str = "/api/v1/facilities/cases/{case_id}/assign";
pub const FACILITIES_START_PATH: &str = "/api/v1/facilities/cases/{case_id}/start";
pub const FACILITIES_SUBMIT_PATH: &str = "/api/v1/facilities/cases/{case_id}/submit";
pub const FACILITIES_ACCEPT_PATH: &str = "/api/v1/facilities/cases/{case_id}/acceptance";
pub const FACILITIES_OBSERVATIONS_PATH: &str = "/api/v1/facilities/cases/{case_id}/observations";
pub const FACILITIES_ROUTE_PATHS: &[&str] = &[
    FACILITIES_CASES_PATH,
    FACILITIES_CASE_PATH,
    FACILITIES_TRIAGE_PATH,
    FACILITIES_ASSIGN_PATH,
    FACILITIES_START_PATH,
    FACILITIES_SUBMIT_PATH,
    FACILITIES_ACCEPT_PATH,
    FACILITIES_OBSERVATIONS_PATH,
];

#[derive(Clone)]
pub struct FacilitiesRestState {
    pool: PgPool,
    jwt_verifier: Option<JwtVerifier>,
}
impl FacilitiesRestState {
    #[must_use]
    pub fn new(pool: PgPool, jwt_verifier: Option<JwtVerifier>) -> Self {
        Self { pool, jwt_verifier }
    }
}

/// Materialize each due HVAC occurrence exactly once and advance its obligation
/// while the obligation row is locked. The unique occurrence key is a second
/// line of defense when more than one application process polls concurrently.
pub async fn poll_scheduled_hvac(pool: &PgPool) -> Result<u64, DbError> {
    let orgs: Vec<Uuid> = sqlx::query_scalar("SELECT id FROM platform_list_organizations()")
        // rls-arming: ok platform_list_organizations() is the SECURITY DEFINER id-only tenant discovery; every per-org facilities mutation below runs under with_audits
        .fetch_all(pool)
        .await
        .map_err(DbError::Sqlx)?;
    let now = time::OffsetDateTime::now_utc();
    let mut created = 0_u64;
    for org_uuid in orgs {
        let org = OrgId::from_uuid(org_uuid);
        created += with_audits::<_, _, DbError>(pool, org, move |tx| Box::pin(async move {
            let obligations = sqlx::query("SELECT id,branch_id,site_id,next_due_at,recurrence_days,response_due_seconds,completion_due_seconds,acceptance_due_seconds FROM facilities_obligations WHERE org_id=$1 AND active AND next_due_at <= $2 ORDER BY next_due_at FOR UPDATE SKIP LOCKED")
                .bind(org_uuid).bind(now).fetch_all(tx.as_mut()).await.map_err(DbError::Sqlx)?;
            let mut count = 0_u64;
            let mut audits = Vec::new();
            for obligation in obligations {
                let obligation_id: Uuid = obligation.try_get("id").map_err(DbError::Sqlx)?;
                let branch: Uuid = obligation.try_get("branch_id").map_err(DbError::Sqlx)?;
                let site: Uuid = obligation.try_get("site_id").map_err(DbError::Sqlx)?;
                let due: time::OffsetDateTime = obligation.try_get("next_due_at").map_err(DbError::Sqlx)?;
                let recurrence_days: i32 = obligation.try_get("recurrence_days").map_err(DbError::Sqlx)?;
                let response: i32 = obligation.try_get("response_due_seconds").map_err(DbError::Sqlx)?;
                let completion: i32 = obligation.try_get("completion_due_seconds").map_err(DbError::Sqlx)?;
                let acceptance: i32 = obligation.try_get("acceptance_due_seconds").map_err(DbError::Sqlx)?;
                let case = Uuid::new_v4();
                let inserted = sqlx::query("INSERT INTO facilities_cases(id,org_id,branch_id,site_id,obligation_id,status,response_due_at,completion_due_at,acceptance_due_at,occurrence_due_at,request_hash,idempotency_key) VALUES($1,$2,$3,$4,$5,'DUE',$6,$7,$8,$9,$10,$11) ON CONFLICT (org_id,obligation_id,occurrence_due_at) DO NOTHING")
                    .bind(case).bind(org_uuid).bind(branch).bind(site).bind(obligation_id)
                    .bind(due + time::Duration::seconds(i64::from(response)))
                    .bind(due + time::Duration::seconds(i64::from(completion)))
                    .bind(due + time::Duration::seconds(i64::from(acceptance)))
                    .bind(due).bind(format!("scheduled:{obligation_id}:{due}")).bind(format!("scheduled:{obligation_id}:{due}"))
                    .execute(tx.as_mut()).await.map_err(DbError::Sqlx)?;
                let next_due = due + time::Duration::days(i64::from(recurrence_days));
                sqlx::query("UPDATE facilities_obligations SET next_due_at=$1 WHERE id=$2 AND org_id=$3 AND next_due_at=$4")
                    .bind(next_due).bind(obligation_id).bind(org_uuid).bind(due).execute(tx.as_mut()).await.map_err(DbError::Sqlx)?;
                if inserted.rows_affected() == 1 {
                    sqlx::query("INSERT INTO facilities_case_history(org_id,case_id,from_status,to_status,actor_id,receipt) VALUES($1,$2,NULL,'DUE',NULL,jsonb_build_object('source','scheduled_hvac','occurrenceDueAt',$3))")
                        .bind(org_uuid).bind(case).bind(due).execute(tx.as_mut()).await.map_err(DbError::Sqlx)?;
                    let action = AuditAction::new("facilities.case.scheduled").map_err(|error| DbError::CodeIssuance(error.to_string()))?;
                    audits.push(AuditEvent::new(None, action, "facilities_case", case.to_string(), TraceContext::generate(), now).with_org(org).with_branch(BranchId::from_uuid(branch)).with_snapshots(None, Some(serde_json::json!({"status":"DUE","occurrenceDueAt":due}))));
                    count += 1;
                }
            }
            Ok((count, audits))
        })).await?;
    }
    Ok(created)
}
pub fn router(state: FacilitiesRestState) -> Router {
    let verifier = state.jwt_verifier.clone();
    let pool = state.pool.clone();
    console_platform_request_context::with_request_context(
        Router::new()
            .route(FACILITIES_CASES_PATH, get(list_cases).post(create_due_case))
            .route(FACILITIES_CASE_PATH, get(get_case))
            .route(FACILITIES_TRIAGE_PATH, post(triage))
            .route(FACILITIES_ASSIGN_PATH, post(assign))
            .route(FACILITIES_START_PATH, post(start))
            .route(FACILITIES_SUBMIT_PATH, post(submit))
            .route(FACILITIES_ACCEPT_PATH, post(acceptance))
            .route(FACILITIES_OBSERVATIONS_PATH, post(observe))
            .with_state(state),
        verifier,
        pool,
    )
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CaseView {
    id: Uuid,
    branch_id: Uuid,
    status: String,
    assignee_id: Option<Uuid>,
    // `openapi.yaml` declares these three, `scheduledFor` and `observedAt` below
    // as `{type: string, format: date-time}`. The default `OffsetDateTime` serde
    // representation is an internal nine-element numeric tuple: it emits an array
    // no spec-conformant client can read, and rejects the ISO 8601 string one sends.
    #[serde(with = "time::serde::rfc3339")]
    response_due_at: time::OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    completion_due_at: time::OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    acceptance_due_at: time::OffsetDateTime,
    energy_delta_kwh: Option<String>,
    total_cost_krw: i64,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DueCaseBody {
    obligation_id: Uuid,
    idempotency_key: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TriageBody {
    #[serde(with = "time::serde::rfc3339")]
    scheduled_for: time::OffsetDateTime,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AssignBody {
    assignee_id: Uuid,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SubmitBody {
    safety_checklist_evidence_id: Uuid,
    service_report_evidence_id: Uuid,
    photo_evidence_id: Option<Uuid>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AcceptanceBody {
    decision: String,
    reason: Option<String>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ObservationBody {
    pre_kwh: Option<String>,
    post_kwh: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    observed_at: time::OffsetDateTime,
    cost_krw: Option<i64>,
}

async fn principal(s: &FacilitiesRestState, h: &HeaderMap) -> Result<Principal, RestError> {
    let v = s.jwt_verifier.as_ref().ok_or_else(|| {
        RestError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "unavailable",
            "JWT verification is not configured",
        )
    })?;
    resolve_principal(v, &s.pool, h).await.map_err(|e| match e {
        RequestContextError::MissingBearer
        | RequestContextError::InvalidToken
        | RequestContextError::InvalidClaim(_) => RestError::new(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "invalid bearer token",
        ),
        _ => RestError::new(
            StatusCode::FORBIDDEN,
            "forbidden",
            "tenant context is not authorized",
        ),
    })
}
fn require_feature(p: &Principal, feature: Feature, branch: Uuid) -> Result<(), RestError> {
    authorize(p, Action::new(feature), BranchId::from_uuid(branch)).map_err(RestError::kernel)
}

/// Pre-gate for a route whose resource branch is only known after the row is
/// read (or, for a list, is the caller's whole scope); the concrete branch is
/// still enforced by [`require_feature`] on every row.
/// Not `authorize_org_wide`: that requires `BranchScope::All` and so collapses
/// `FacilitiesManage` (`[D,D,D,A,D,A]`) and `FacilitiesObserve` (`[D,A,A,A,A,A]`)
/// to SUPER_ADMIN, locking every branch-scoped ADMIN out of the intake.
fn require_capability(p: &Principal, feature: Feature) -> Result<(), RestError> {
    authorize_capability(p, Action::new(feature)).map_err(RestError::kernel)
}
fn hash<T: Serialize>(v: &T) -> String {
    let bytes = serde_json::to_vec(v).unwrap_or_default();
    hex::encode(Sha256::digest(bytes))
}
fn event(
    p: &Principal,
    action: &str,
    id: Uuid,
    branch: Uuid,
    before: Option<serde_json::Value>,
    after: Option<serde_json::Value>,
) -> Result<AuditEvent, RestError> {
    Ok(AuditEvent::new(
        Some(p.user_id),
        AuditAction::new(action).map_err(RestError::kernel)?,
        "facilities_case",
        id.to_string(),
        TraceContext::generate(),
        time::OffsetDateTime::now_utc(),
    )
    .with_org(p.org_id)
    .with_branch(BranchId::from_uuid(branch))
    .with_snapshots(before, after))
}

async fn create_due_case(
    State(s): State<FacilitiesRestState>,
    h: HeaderMap,
    Json(b): Json<DueCaseBody>,
) -> Result<Json<CaseView>, RestError> {
    let p = principal(&s, &h).await?;
    require_capability(&p, Feature::FacilitiesManage)?;
    if b.idempotency_key.len() < 16 {
        return Err(RestError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "validation",
            "idempotency_key must be at least 16 characters",
        ));
    }
    let req_hash = hash(&b);
    let org = p.org_id;
    let obligation_id = b.obligation_id;
    let row = with_org_conn::<_, _, RestError>(&s.pool, org, move |tx| Box::pin(async move {
        sqlx::query("SELECT o.branch_id,o.site_id,o.next_due_at,o.response_due_seconds,o.completion_due_seconds,o.acceptance_due_seconds FROM facilities_obligations o WHERE o.id=$1 AND o.org_id=$2 AND o.active FOR UPDATE")
            .bind(obligation_id).bind(*org.as_uuid()).fetch_optional(tx.as_mut()).await.map_err(RestError::db)
    })).await?.ok_or_else(||RestError::new(StatusCode::NOT_FOUND,"not_found","active HVAC obligation was not found"))?;
    let branch: Uuid = row.try_get("branch_id").map_err(RestError::db)?;
    require_feature(&p, Feature::FacilitiesManage, branch)?;
    let idempotency_key = b.idempotency_key.clone();
    let existing = with_org_conn::<_, _, RestError>(&s.pool, org, move |tx| {
        Box::pin(async move {
            sqlx::query("SELECT id,request_hash FROM facilities_cases WHERE org_id=$1 AND idempotency_key=$2")
                .bind(*org.as_uuid())
                .bind(idempotency_key)
                .fetch_optional(tx.as_mut())
                .await
                .map_err(RestError::db)
        })
    })
    .await?;
    if let Some(x) = existing {
        let existing_hash: String = x.try_get("request_hash").map_err(RestError::db)?;
        if existing_hash != req_hash {
            return Err(RestError::new(
                StatusCode::CONFLICT,
                "idempotency_conflict",
                "idempotency key was used with a different request",
            ));
        }
        let id: Uuid = x.try_get("id").map_err(RestError::db)?;
        return get_case_view(&s.pool, &p, id).await.map(Json);
    }
    let now = time::OffsetDateTime::now_utc();
    let due: time::OffsetDateTime = row.try_get("next_due_at").map_err(RestError::db)?;
    let case = Uuid::new_v4();
    let site: Uuid = row.try_get("site_id").map_err(RestError::db)?;
    let rd: i32 = row.try_get("response_due_seconds").map_err(RestError::db)?;
    let cd: i32 = row
        .try_get("completion_due_seconds")
        .map_err(RestError::db)?;
    let ad: i32 = row
        .try_get("acceptance_due_seconds")
        .map_err(RestError::db)?;
    let e = event(
        &p,
        "facilities.case.create",
        case,
        branch,
        None,
        Some(serde_json::json!({"status":"DUE"})),
    )?;
    let actor = p.clone();
    with_audit::<_,(),RestError>(&s.pool,e,|tx|Box::pin(async move {sqlx::query("INSERT INTO facilities_cases(id,org_id,branch_id,site_id,obligation_id,status,response_due_at,completion_due_at,acceptance_due_at,occurrence_due_at,request_hash,idempotency_key) VALUES($1,$2,$3,$4,$5,'DUE',$6,$7,$8,$9,$10,$11)").bind(case).bind(*actor.org_id.as_uuid()).bind(branch).bind(site).bind(b.obligation_id).bind(due+time::Duration::seconds(i64::from(rd))).bind(due+time::Duration::seconds(i64::from(cd))).bind(due+time::Duration::seconds(i64::from(ad))).bind(due).bind(req_hash).bind(b.idempotency_key).execute(tx.as_mut()).await.map_err(RestError::db)?; history(tx,&actor,case,None,"DUE").await?; Ok(())})).await?;
    let _ = now;
    get_case_view(&s.pool, &p, case).await.map(Json)
}

async fn list_cases(
    State(s): State<FacilitiesRestState>,
    h: HeaderMap,
) -> Result<Json<Vec<CaseView>>, RestError> {
    let p = principal(&s, &h).await?;
    require_capability(&p, Feature::FacilitiesObserve)?;
    let org = p.org_id;
    // The row gate below is `require_feature` per case, which ERRORS on a foreign
    // branch rather than skipping it, so an unfiltered org-wide select would make
    // the whole list 403 on the first row outside the caller's scope. Confine the
    // select to the caller's `branch_scope` first; empty scope binds an empty array
    // and selects nothing, which is the correct default deny.
    let (org_wide, branches) = match &p.branch_scope {
        BranchScope::All => (true, Vec::new()),
        BranchScope::Branches(b) => (false, b.iter().map(|id| *id.as_uuid()).collect()),
    };
    let ids = with_org_conn::<_, _, RestError>(&s.pool, org, move |tx| {
        Box::pin(async move {
            sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM facilities_cases WHERE org_id=$1 AND ($2::boolean OR branch_id = ANY($3::uuid[])) ORDER BY created_at DESC LIMIT 100",
    )
    .bind(*org.as_uuid()).bind(org_wide).bind(branches).fetch_all(tx.as_mut()).await.map_err(RestError::db)
        })
    })
    .await?;
    let mut out = Vec::new();
    for id in ids {
        out.push(get_case_view(&s.pool, &p, id).await?);
    }
    Ok(Json(out))
}
async fn get_case(
    State(s): State<FacilitiesRestState>,
    h: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Json<CaseView>, RestError> {
    let p = principal(&s, &h).await?;
    require_capability(&p, Feature::FacilitiesObserve)?;
    get_case_view(&s.pool, &p, id).await.map(Json)
}
async fn get_case_view(pool: &PgPool, p: &Principal, id: Uuid) -> Result<CaseView, RestError> {
    let org = p.org_id;
    let r = with_org_conn::<_, _, RestError>(pool, org, move |tx| Box::pin(async move { sqlx::query("SELECT c.id,c.status,c.assignee_id,c.response_due_at,c.completion_due_at,c.acceptance_due_at,c.branch_id, (SELECT post.kwh-pre.kwh FROM facilities_energy_observations pre JOIN facilities_energy_observations post ON post.case_id=pre.case_id AND post.phase='POST' WHERE pre.case_id=c.id AND pre.phase='PRE')::text energy_delta_kwh, COALESCE((SELECT sum(amount_krw) FROM facilities_cost_observations WHERE case_id=c.id),0)::bigint total_cost_krw FROM facilities_cases c WHERE c.id=$1 AND c.org_id=$2").bind(id).bind(*org.as_uuid()).fetch_optional(tx.as_mut()).await.map_err(RestError::db) })).await?.ok_or_else(||RestError::new(StatusCode::NOT_FOUND,"not_found","facilities case was not found"))?;
    let b: Uuid = r.try_get("branch_id").map_err(RestError::db)?;
    require_feature(p, Feature::FacilitiesObserve, b)?;
    Ok(CaseView {
        id: r.try_get("id").map_err(RestError::db)?,
        branch_id: b,
        status: r.try_get("status").map_err(RestError::db)?,
        assignee_id: r.try_get("assignee_id").map_err(RestError::db)?,
        response_due_at: r.try_get("response_due_at").map_err(RestError::db)?,
        completion_due_at: r.try_get("completion_due_at").map_err(RestError::db)?,
        acceptance_due_at: r.try_get("acceptance_due_at").map_err(RestError::db)?,
        energy_delta_kwh: r.try_get("energy_delta_kwh").map_err(RestError::db)?,
        total_cost_krw: r.try_get("total_cost_krw").map_err(RestError::db)?,
    })
}

#[allow(clippy::too_many_arguments)]
async fn transition(
    s: &FacilitiesRestState,
    h: &HeaderMap,
    id: Uuid,
    grant: Feature,
    to: &'static str,
    assignee: Option<Uuid>,
    scheduled: Option<time::OffsetDateTime>,
    require_assignee: bool,
) -> Result<Json<CaseView>, RestError> {
    let p = principal(s, h).await?;
    let org = p.org_id;
    let actor = p.clone();
    with_audits::<_, _, RestError>(&s.pool, org, move |tx| {
        Box::pin(async move {
            let r = sqlx::query("SELECT status,branch_id,assignee_id FROM facilities_cases WHERE id=$1 AND org_id=$2 FOR UPDATE")
                .bind(id).bind(*org.as_uuid()).fetch_optional(tx.as_mut()).await.map_err(RestError::db)?
                .ok_or_else(|| RestError::new(StatusCode::NOT_FOUND, "not_found", "facilities case was not found"))?;
            let branch: Uuid = r.try_get("branch_id").map_err(RestError::db)?;
            require_feature(&actor, grant, branch)?;
            let from: String = r.try_get("status").map_err(RestError::db)?;
            let current: Option<Uuid> = r.try_get("assignee_id").map_err(RestError::db)?;
            if require_assignee && current != Some(*actor.user_id.as_uuid()) {
                return Err(RestError::new(StatusCode::FORBIDDEN, "not_assignee", "only the assigned technician may perform this transition"));
            }
            if !legal(&from, to) {
                return Err(RestError::new(StatusCode::CONFLICT, "illegal_transition", "facilities case transition is not legal"));
            }
            let changed = sqlx::query("UPDATE facilities_cases SET status=$1, assignee_id=COALESCE($2,assignee_id), scheduled_for=COALESCE($3,scheduled_for), safety_acknowledged_at=CASE WHEN $1='IN_PROGRESS' THEN now() ELSE safety_acknowledged_at END, updated_at=now() WHERE id=$4 AND org_id=$5 AND status=$6")
                .bind(to).bind(assignee).bind(scheduled).bind(id).bind(*org.as_uuid()).bind(&from).execute(tx.as_mut()).await.map_err(RestError::db)?;
            if changed.rows_affected() != 1 {
                return Err(RestError::new(StatusCode::CONFLICT, "concurrent_transition", "facilities case changed concurrently"));
            }
            history(tx, &actor, id, Some(&from), to).await?;
            let audit = event(&actor, "facilities.case.transition", id, branch, Some(serde_json::json!({"status":from})), Some(serde_json::json!({"status":to})))?;
            Ok(((), vec![audit]))
        })
    })
    .await?;
    get_case_view(&s.pool, &p, id).await.map(Json)
}
fn legal(from: &str, to: &str) -> bool {
    matches!(
        (from, to),
        ("DUE", "TRIAGED")
            | ("DUE", "SCHEDULED")
            | ("TRIAGED", "SCHEDULED")
            | ("SCHEDULED", "ASSIGNED")
            | ("ASSIGNED", "IN_PROGRESS")
            | ("REWORK_REQUIRED", "IN_PROGRESS")
            | ("IN_PROGRESS", "AWAITING_ACCEPTANCE")
            | ("AWAITING_ACCEPTANCE", "REWORK_REQUIRED")
            | ("AWAITING_ACCEPTANCE", "CLOSED")
    )
}

#[cfg(test)]
mod transition_tests {
    use super::legal;

    #[test]
    fn allows_the_scheduled_hvac_happy_path_transitions() {
        for transition in [
            ("DUE", "SCHEDULED"),
            ("SCHEDULED", "ASSIGNED"),
            ("ASSIGNED", "IN_PROGRESS"),
            ("IN_PROGRESS", "AWAITING_ACCEPTANCE"),
            ("AWAITING_ACCEPTANCE", "CLOSED"),
        ] {
            assert!(legal(transition.0, transition.1));
        }
    }

    #[test]
    fn rejects_a_terminal_case_transition() {
        assert!(!legal("CLOSED", "IN_PROGRESS"));
    }

    #[test]
    fn permits_rework_only_from_a_rejected_acceptance() {
        assert!(legal("AWAITING_ACCEPTANCE", "REWORK_REQUIRED"));
        assert!(legal("REWORK_REQUIRED", "IN_PROGRESS"));
        assert!(!legal("IN_PROGRESS", "REWORK_REQUIRED"));
    }

    #[test]
    fn rejected_acceptance_requires_a_non_blank_reason() {
        assert!(super::acceptance_target("REJECTED", None).is_err());
        assert!(super::acceptance_target("REJECTED", Some("   ")).is_err());
    }
}
async fn triage(
    State(s): State<FacilitiesRestState>,
    h: HeaderMap,
    Path(id): Path<Uuid>,
    Json(b): Json<TriageBody>,
) -> Result<Json<CaseView>, RestError> {
    transition(
        &s,
        &h,
        id,
        Feature::FacilitiesDispatch,
        "SCHEDULED",
        None,
        Some(b.scheduled_for),
        false,
    )
    .await
}
async fn assign(
    State(s): State<FacilitiesRestState>,
    h: HeaderMap,
    Path(id): Path<Uuid>,
    Json(b): Json<AssignBody>,
) -> Result<Json<CaseView>, RestError> {
    transition(
        &s,
        &h,
        id,
        Feature::FacilitiesDispatch,
        "ASSIGNED",
        Some(b.assignee_id),
        None,
        false,
    )
    .await
}
async fn start(
    State(s): State<FacilitiesRestState>,
    h: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Json<CaseView>, RestError> {
    transition(
        &s,
        &h,
        id,
        Feature::FacilitiesExecute,
        "IN_PROGRESS",
        None,
        None,
        true,
    )
    .await
}
async fn submit(
    State(s): State<FacilitiesRestState>,
    h: HeaderMap,
    Path(id): Path<Uuid>,
    Json(b): Json<SubmitBody>,
) -> Result<Json<CaseView>, RestError> {
    let p = principal(&s, &h).await?;
    let org = p.org_id;
    let actor = p.clone();
    let mut evidence = vec![
        ("SAFETY_CHECKLIST", b.safety_checklist_evidence_id),
        ("SERVICE_REPORT", b.service_report_evidence_id),
    ];
    if let Some(photo) = b.photo_evidence_id {
        evidence.push(("PHOTO", photo));
    }
    with_audits::<_, _, RestError>(&s.pool, org, move |tx| Box::pin(async move {
        let row = sqlx::query("SELECT status,branch_id,assignee_id FROM facilities_cases WHERE id=$1 AND org_id=$2 FOR UPDATE")
            .bind(id).bind(*org.as_uuid()).fetch_optional(tx.as_mut()).await.map_err(RestError::db)?
            .ok_or_else(|| RestError::new(StatusCode::NOT_FOUND, "not_found", "facilities case was not found"))?;
        let branch: Uuid = row.try_get("branch_id").map_err(RestError::db)?;
        require_feature(&actor, Feature::FacilitiesExecute, branch)?;
        let status: String = row.try_get("status").map_err(RestError::db)?;
        let assignee: Option<Uuid> = row.try_get("assignee_id").map_err(RestError::db)?;
        if assignee != Some(*actor.user_id.as_uuid()) { return Err(RestError::new(StatusCode::FORBIDDEN, "not_assignee", "only the assigned technician may submit work")); }
        if status != "IN_PROGRESS" { return Err(RestError::new(StatusCode::CONFLICT, "illegal_transition", "facilities case transition is not legal")); }
        for (kind, evidence_id) in evidence {
            let ok: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM docs_evidence_objects WHERE id=$1 AND org_id=$2 AND admissibility_status='ADMISSIBLE')")
                .bind(evidence_id).bind(*org.as_uuid()).fetch_one(tx.as_mut()).await.map_err(RestError::db)?;
            if !ok { return Err(RestError::new(StatusCode::PRECONDITION_FAILED, "evidence_not_confirmed", "required evidence must be a real confirmed evidence object")); }
            sqlx::query("INSERT INTO facilities_execution_evidence_links(org_id,case_id,evidence_id,evidence_kind,linked_by) VALUES($1,$2,$3,$4,$5) ON CONFLICT (org_id,case_id,evidence_kind) DO UPDATE SET evidence_id=EXCLUDED.evidence_id, linked_by=EXCLUDED.linked_by, linked_at=now()")
                .bind(*org.as_uuid()).bind(id).bind(evidence_id).bind(kind).bind(*actor.user_id.as_uuid()).execute(tx.as_mut()).await.map_err(RestError::db)?;
        }
        sqlx::query("UPDATE facilities_cases SET status='AWAITING_ACCEPTANCE',updated_at=now() WHERE id=$1 AND org_id=$2 AND status='IN_PROGRESS'")
            .bind(id).bind(*org.as_uuid()).execute(tx.as_mut()).await.map_err(RestError::db)?;
        history(tx, &actor, id, Some("IN_PROGRESS"), "AWAITING_ACCEPTANCE").await?;
        let audit = event(&actor, "facilities.case.submit", id, branch, Some(serde_json::json!({"status":"IN_PROGRESS"})), Some(serde_json::json!({"status":"AWAITING_ACCEPTANCE"})))?;
        Ok(((), vec![audit]))
    })).await?;
    get_case_view(&s.pool, &p, id).await.map(Json)
}
fn acceptance_target(
    decision: &str,
    reason: Option<&str>,
) -> Result<(&'static str, Option<String>), RestError> {
    let reason = reason
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned);
    match decision {
        "ACCEPTED" => Ok(("CLOSED", reason)),
        "REJECTED" if reason.is_some() => Ok(("REWORK_REQUIRED", reason)),
        "REJECTED" => Err(RestError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "validation",
            "a rejection reason is required",
        )),
        _ => Err(RestError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "validation",
            "decision must be ACCEPTED or REJECTED",
        )),
    }
}

async fn acceptance(
    State(s): State<FacilitiesRestState>,
    h: HeaderMap,
    Path(id): Path<Uuid>,
    Json(b): Json<AcceptanceBody>,
) -> Result<Json<CaseView>, RestError> {
    let (to, reason) = acceptance_target(&b.decision, b.reason.as_deref())?;
    let p = principal(&s, &h).await?;
    let org = p.org_id;
    let actor = p.clone();
    let decision = b.decision.clone();
    with_audits::<_, _, RestError>(&s.pool, org, move |tx| Box::pin(async move {
        let row = sqlx::query("SELECT status,branch_id FROM facilities_cases WHERE id=$1 AND org_id=$2 FOR UPDATE")
            .bind(id).bind(*org.as_uuid()).fetch_optional(tx.as_mut()).await.map_err(RestError::db)?
            .ok_or_else(|| RestError::new(StatusCode::NOT_FOUND, "not_found", "facilities case was not found"))?;
        let branch: Uuid = row.try_get("branch_id").map_err(RestError::db)?;
        require_feature(&actor, Feature::FacilitiesAccept, branch)?;
        let from: String = row.try_get("status").map_err(RestError::db)?;
        if from != "AWAITING_ACCEPTANCE" { return Err(RestError::new(StatusCode::CONFLICT, "illegal_transition", "facilities case transition is not legal")); }
        sqlx::query("INSERT INTO facilities_acceptances(org_id,case_id,decision,reason,actor_id) VALUES($1,$2,$3,$4,$5)")
            .bind(*org.as_uuid()).bind(id).bind(&decision).bind(reason).bind(*actor.user_id.as_uuid()).execute(tx.as_mut()).await.map_err(RestError::db)?;
        sqlx::query("UPDATE facilities_cases SET status=$1,updated_at=now() WHERE id=$2 AND org_id=$3 AND status='AWAITING_ACCEPTANCE'")
            .bind(to).bind(id).bind(*org.as_uuid()).execute(tx.as_mut()).await.map_err(RestError::db)?;
        history(tx, &actor, id, Some("AWAITING_ACCEPTANCE"), to).await?;
        let audit = event(&actor, "facilities.case.acceptance", id, branch, Some(serde_json::json!({"status":"AWAITING_ACCEPTANCE"})), Some(serde_json::json!({"status":to,"decision":decision})))?;
        Ok(((), vec![audit]))
    })).await?;
    get_case_view(&s.pool, &p, id).await.map(Json)
}
async fn observe(
    State(s): State<FacilitiesRestState>,
    h: HeaderMap,
    Path(id): Path<Uuid>,
    Json(b): Json<ObservationBody>,
) -> Result<Json<CaseView>, RestError> {
    let p = principal(&s, &h).await?;
    let org = p.org_id;
    let actor = p.clone();
    with_audits::<_, _, RestError>(&s.pool, org, move |tx| Box::pin(async move {
        let row = sqlx::query("SELECT status,branch_id FROM facilities_cases WHERE id=$1 AND org_id=$2 FOR UPDATE")
            .bind(id).bind(*org.as_uuid()).fetch_optional(tx.as_mut()).await.map_err(RestError::db)?
            .ok_or_else(|| RestError::new(StatusCode::NOT_FOUND, "not_found", "facilities case was not found"))?;
        let branch: Uuid = row.try_get("branch_id").map_err(RestError::db)?;
        require_feature(&actor, Feature::FacilitiesObserve, branch)?;
        let status: String = row.try_get("status").map_err(RestError::db)?;
        if status != "IN_PROGRESS" {
            return Err(RestError::new(
                StatusCode::CONFLICT,
                "illegal_transition",
                "observations can be recorded only while work is in progress",
            ));
        }
        for (phase, value) in [("PRE", b.pre_kwh), ("POST", b.post_kwh)] {
            if let Some(kwh) = value {
                sqlx::query("INSERT INTO facilities_energy_observations(org_id,case_id,phase,source,observed_at,kwh,recorded_by) VALUES($1,$2,$3,'MANUAL',$4,$5::numeric,$6) ON CONFLICT (org_id,case_id,phase) DO UPDATE SET observed_at=EXCLUDED.observed_at,kwh=EXCLUDED.kwh,recorded_by=EXCLUDED.recorded_by")
                    .bind(*org.as_uuid()).bind(id).bind(phase).bind(b.observed_at).bind(kwh).bind(*actor.user_id.as_uuid()).execute(tx.as_mut()).await.map_err(RestError::db)?;
            }
        }
        if let Some(cost) = b.cost_krw {
            sqlx::query("INSERT INTO facilities_cost_observations(org_id,case_id,source,observed_at,currency,amount_krw,recorded_by) VALUES($1,$2,'MANUAL',$3,'KRW',$4,$5)")
                .bind(*org.as_uuid()).bind(id).bind(b.observed_at).bind(cost).bind(*actor.user_id.as_uuid()).execute(tx.as_mut()).await.map_err(RestError::db)?;
        }
        let audit = event(&actor, "facilities.case.observe", id, branch, None, Some(serde_json::json!({"observedAt":b.observed_at,"costKrw":b.cost_krw})))?;
        Ok(((), vec![audit]))
    })).await?;
    get_case_view(&s.pool, &p, id).await.map(Json)
}
async fn history(
    tx: &mut Transaction<'_, Postgres>,
    p: &Principal,
    id: Uuid,
    from: Option<&str>,
    to: &str,
) -> Result<(), RestError> {
    sqlx::query("INSERT INTO facilities_case_history(org_id,case_id,from_status,to_status,actor_id,receipt) VALUES($1,$2,$3,$4,$5,jsonb_build_object('actorId',$5,'transition',concat(coalesce($3,''),'->',$4)))").bind(*p.org_id.as_uuid()).bind(id).bind(from).bind(to).bind(*p.user_id.as_uuid()).execute(tx.as_mut()).await.map_err(RestError::db)?;
    Ok(())
}
#[derive(Debug)]
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
    fn db(e: sqlx::Error) -> Self {
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, "database", e.to_string())
    }
    fn kernel(e: KernelError) -> Self {
        Self::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "validation",
            e.to_string(),
        )
    }
}
impl From<DbError> for RestError {
    fn from(e: DbError) -> Self {
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, "database", e.to_string())
    }
}
impl IntoResponse for RestError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(serde_json::json!({"code":self.code,"message":self.message})),
        )
            .into_response()
    }
}
