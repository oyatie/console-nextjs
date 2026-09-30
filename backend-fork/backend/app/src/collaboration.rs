use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use console_kernel_core::{
    AuditAction, AuditEvent, BranchScope, ErrorKind, KernelError, TraceContext,
};
use console_platform_auth::{
    JwtVerifier, MobilePasskeyStepUpBinding, MobilePasskeyStepUpEnvelope,
    MobilePasskeyStepUpVerificationError, PasskeyService,
};
use console_platform_authz::{Feature, PermissionLevel, Principal, permission_for};
use console_platform_db::{DbError, with_audit, with_org_conn};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::{PgPool, Postgres, Row, Transaction};
use std::collections::BTreeSet;
use time::OffsetDateTime;
use uuid::Uuid;

pub const CALENDAR_EVENTS_PATH: &str = "/api/v1/collaboration/calendar/events";
pub const POLLS_PATH: &str = "/api/v1/collaboration/polls";
pub const POLL_VOTE_PATH_TEMPLATE: &str = "/api/v1/collaboration/polls/{id}/vote";
pub const MOBILE_POLL_VOTE_PATH_TEMPLATE: &str = "/api/v1/mobile/collaboration/polls/{id}/vote";
pub const COLLABORATION_ROUTE_PATHS: &[&str] = &[
    CALENDAR_EVENTS_PATH,
    POLLS_PATH,
    POLL_VOTE_PATH_TEMPLATE,
    MOBILE_POLL_VOTE_PATH_TEMPLATE,
];

const COLLABORATION_REQUESTS_TOTAL: &str = "collaboration_requests_total";
const MAX_LIST_LIMIT: i64 = 100;
const DEFAULT_LIST_LIMIT: i64 = 30;
const MAX_POLL_OPTIONS: usize = 20;

#[derive(Clone)]
pub struct CollaborationState {
    pool: PgPool,
    jwt_verifier: Option<JwtVerifier>,
    passkey_step_up: Option<PasskeyService>,
}

impl CollaborationState {
    #[must_use]
    pub fn new(pool: PgPool, jwt_verifier: Option<JwtVerifier>) -> Self {
        Self {
            pool,
            jwt_verifier,
            passkey_step_up: None,
        }
    }

    #[must_use]
    pub fn with_passkey_step_up(mut self, passkey_step_up: Option<PasskeyService>) -> Self {
        self.passkey_step_up = passkey_step_up;
        self
    }
}

pub fn router(state: CollaborationState) -> Router {
    let verifier = state.jwt_verifier.clone();
    let pool = state.pool.clone();
    let router = Router::new()
        .route(
            CALENDAR_EVENTS_PATH,
            get(list_calendar_events).post(create_calendar_event),
        )
        .route(POLLS_PATH, get(list_polls).post(create_poll))
        .route(POLL_VOTE_PATH_TEMPLATE, post(vote_poll))
        .route(MOBILE_POLL_VOTE_PATH_TEMPLATE, post(vote_mobile_poll))
        .with_state(state);
    console_platform_request_context::with_request_context(router, verifier, pool)
}

#[derive(Debug, Deserialize)]
struct CalendarEventQuery {
    #[serde(default, with = "time::serde::rfc3339::option")]
    from: Option<OffsetDateTime>,
    #[serde(default, with = "time::serde::rfc3339::option")]
    to: Option<OffsetDateTime>,
    limit: Option<i64>,
}

#[derive(Debug, Serialize)]
struct CalendarEventListResponse {
    items: Vec<CalendarEventResponse>,
}

#[derive(Debug, Deserialize)]
struct CreateCalendarEventRequest {
    scope_type: ScopeType,
    #[serde(default)]
    scope_ref: Option<String>,
    title: String,
    #[serde(default)]
    description: String,
    #[serde(with = "time::serde::rfc3339")]
    starts_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    ends_at: OffsetDateTime,
    #[serde(default)]
    all_day: bool,
    #[serde(default)]
    object_type: Option<String>,
    #[serde(default)]
    object_id: Option<Uuid>,
}

#[derive(Debug, Serialize)]
struct CalendarEventResponse {
    id: Uuid,
    scope_type: ScopeType,
    #[serde(skip_serializing_if = "Option::is_none")]
    scope_ref: Option<String>,
    title: String,
    description: String,
    #[serde(with = "time::serde::rfc3339")]
    starts_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    ends_at: OffsetDateTime,
    all_day: bool,
    status: CalendarEventStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    object_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    object_id: Option<Uuid>,
    #[serde(skip_serializing_if = "Option::is_none")]
    created_by: Option<Uuid>,
    #[serde(with = "time::serde::rfc3339")]
    created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    updated_at: OffsetDateTime,
    policy: CollaborationScopePolicy,
}

#[derive(Debug, Deserialize)]
struct PollQuery {
    status: Option<PollStatus>,
    limit: Option<i64>,
}

#[derive(Debug, Serialize)]
struct PollListResponse {
    items: Vec<PollResponse>,
}

#[derive(Debug, Deserialize)]
struct CreatePollRequest {
    target_scope_type: ScopeType,
    #[serde(default)]
    target_scope_ref: Option<String>,
    title: String,
    question: String,
    #[serde(default = "default_poll_status")]
    status: PollStatus,
    #[serde(default = "default_anonymity")]
    anonymity: PollAnonymity,
    #[serde(default)]
    allow_multiple: bool,
    #[serde(default, with = "time::serde::rfc3339::option")]
    closes_at: Option<OffsetDateTime>,
    options: Vec<String>,
    #[serde(default)]
    object_type: Option<String>,
    #[serde(default)]
    object_id: Option<Uuid>,
}

#[derive(Debug, Deserialize)]
struct VotePollRequest {
    selected_option_ids: Vec<Uuid>,
}

#[derive(Debug, Deserialize)]
struct MobileVotePollRequest {
    selected_option_ids: Vec<Uuid>,
    #[serde(default)]
    step_up: Option<MobilePasskeyStepUpEnvelope>,
}

#[derive(Debug, Serialize)]
struct PollResponse {
    id: Uuid,
    target_scope_type: ScopeType,
    #[serde(skip_serializing_if = "Option::is_none")]
    target_scope_ref: Option<String>,
    title: String,
    question: String,
    status: PollStatus,
    anonymity: PollAnonymity,
    allow_multiple: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(with = "time::serde::rfc3339::option")]
    closes_at: Option<OffsetDateTime>,
    #[serde(skip_serializing_if = "Option::is_none")]
    object_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    object_id: Option<Uuid>,
    options: Vec<PollOptionResponse>,
    vote_count: i64,
    my_vote: PollMyVote,
    #[serde(skip_serializing_if = "Option::is_none")]
    created_by: Option<Uuid>,
    #[serde(with = "time::serde::rfc3339")]
    created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    updated_at: OffsetDateTime,
    policy: CollaborationScopePolicy,
}

#[derive(Debug, Serialize, Deserialize)]
struct PollOptionResponse {
    id: Uuid,
    label: String,
    position: i32,
    vote_count: i64,
}

#[derive(Debug, Serialize)]
struct PollMyVote {
    submitted: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    selected_option_ids: Option<Vec<Uuid>>,
}

#[derive(Debug, Serialize)]
struct CollaborationScopePolicy {
    enforcement: &'static str,
    scope_type: ScopeType,
    #[serde(skip_serializing_if = "Option::is_none")]
    scope_ref: Option<String>,
    visibility: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
enum ScopeType {
    Tenant,
    Org,
    Department,
    Team,
    Personal,
}

impl ScopeType {
    const fn as_db(self) -> &'static str {
        match self {
            Self::Tenant => "TENANT",
            Self::Org => "ORG",
            Self::Department => "DEPARTMENT",
            Self::Team => "TEAM",
            Self::Personal => "PERSONAL",
        }
    }

    fn from_db(raw: &str) -> Result<Self, CollaborationError> {
        match raw {
            "TENANT" => Ok(Self::Tenant),
            "ORG" => Ok(Self::Org),
            "DEPARTMENT" => Ok(Self::Department),
            "TEAM" => Ok(Self::Team),
            "PERSONAL" => Ok(Self::Personal),
            _ => Err(CollaborationError::validation(format!(
                "unknown collaboration scope type: {raw}"
            ))),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
enum CalendarEventStatus {
    Active,
    Cancelled,
}

impl CalendarEventStatus {
    fn from_db(raw: &str) -> Result<Self, CollaborationError> {
        match raw {
            "ACTIVE" => Ok(Self::Active),
            "CANCELLED" => Ok(Self::Cancelled),
            _ => Err(CollaborationError::validation(format!(
                "unknown calendar event status: {raw}"
            ))),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
enum PollStatus {
    Draft,
    Open,
    Closed,
    Archived,
}

impl PollStatus {
    const fn as_db(self) -> &'static str {
        match self {
            Self::Draft => "DRAFT",
            Self::Open => "OPEN",
            Self::Closed => "CLOSED",
            Self::Archived => "ARCHIVED",
        }
    }

    fn from_db(raw: &str) -> Result<Self, CollaborationError> {
        match raw {
            "DRAFT" => Ok(Self::Draft),
            "OPEN" => Ok(Self::Open),
            "CLOSED" => Ok(Self::Closed),
            "ARCHIVED" => Ok(Self::Archived),
            _ => Err(CollaborationError::validation(format!(
                "unknown poll status: {raw}"
            ))),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
enum PollAnonymity {
    Named,
    Anonymous,
}

impl PollAnonymity {
    const fn as_db(self) -> &'static str {
        match self {
            Self::Named => "NAMED",
            Self::Anonymous => "ANONYMOUS",
        }
    }

    fn from_db(raw: &str) -> Result<Self, CollaborationError> {
        match raw {
            "NAMED" => Ok(Self::Named),
            "ANONYMOUS" => Ok(Self::Anonymous),
            _ => Err(CollaborationError::validation(format!(
                "unknown poll anonymity: {raw}"
            ))),
        }
    }
}

fn default_poll_status() -> PollStatus {
    PollStatus::Open
}

fn default_anonymity() -> PollAnonymity {
    PollAnonymity::Named
}

async fn list_calendar_events(
    State(state): State<CollaborationState>,
    Extension(principal): Extension<Principal>,
    Query(query): Query<CalendarEventQuery>,
) -> Result<Json<CalendarEventListResponse>, CollaborationError> {
    let limit = normalize_limit(query.limit);
    let as_of = OffsetDateTime::now_utc();
    let from = query
        .from
        .unwrap_or_else(|| as_of - time::Duration::days(1));
    let to = query.to.unwrap_or_else(|| as_of + time::Duration::days(14));
    if to < from {
        return Err(CollaborationError::validation(
            "calendar query end must be after start",
        ));
    }
    let snapshot =
        collect_calendar_events(&state.pool, &principal, from, to, limit, as_of, false).await?;
    record_collaboration_request("calendar_list", "success");
    Ok(Json(CalendarEventListResponse {
        items: snapshot.items,
    }))
}

struct CalendarEventSnapshot {
    items: Vec<CalendarEventResponse>,
    total: usize,
    as_of: OffsetDateTime,
}

async fn collect_calendar_events(
    pool: &PgPool,
    principal: &Principal,
    from: OffsetDateTime,
    to: OffsetDateTime,
    limit: i64,
    as_of: OffsetDateTime,
    half_open_range: bool,
) -> Result<CalendarEventSnapshot, CollaborationError> {
    authorize_collaboration_member(principal)?;
    let org = principal.org_id;
    let user_ref = principal.user_id.as_uuid().to_string();
    let user_id = *principal.user_id.as_uuid();
    let (branch_all, branch_refs) = audience_branch_binds(principal);
    let audience = scope_visibility_sql(
        "scope_type",
        "scope_ref",
        "created_by",
        "$3",
        "$4",
        "$8",
        "$9",
    );
    let sql = format!(
        "SELECT id, scope_type, scope_ref, title, description, starts_at, ends_at, \
                all_day, status, object_type, object_id, created_by, created_at, updated_at, \
                COUNT(*) OVER() AS snapshot_total \
         FROM collaboration_calendar_events \
         WHERE status = 'ACTIVE' \
           AND ( \
               ($7 AND starts_at < $1 AND ends_at > $2) \
               OR (NOT $7 AND starts_at <= $1 AND ends_at >= $2) \
           ) \
           AND {audience} \
           AND created_at <= $5 \
           AND updated_at <= $5 \
         ORDER BY starts_at ASC, created_at DESC \
         LIMIT $6"
    );
    let rows = with_org_conn::<_, _, CollaborationError>(pool, org, move |tx| {
        Box::pin(async move {
            Ok(sqlx::query(sqlx::AssertSqlSafe(sql))
                .bind(to)
                .bind(from)
                .bind(user_ref)
                .bind(user_id)
                .bind(as_of)
                .bind(limit)
                .bind(half_open_range)
                .bind(branch_all)
                .bind(&branch_refs)
                .fetch_all(tx.as_mut())
                .await?)
        })
    })
    .await?;
    let total = rows
        .first()
        .map(|row| row.try_get::<i64, _>("snapshot_total"))
        .transpose()?
        .unwrap_or(0);
    let total = usize::try_from(total)
        .map_err(|_| CollaborationError::internal("calendar count exceeded supported range"))?;
    let items = rows
        .into_iter()
        .map(calendar_event_from_row)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(CalendarEventSnapshot {
        items,
        total,
        as_of,
    })
}

/// Native calendar-owner adapter for the workbench aggregate. It reuses the
/// authenticated collaboration visibility predicate, applies the aggregate's
/// exact half-open range and one request ceiling, and never persists a second
/// calendar projection.
pub(crate) async fn read_workbench_calendar(
    pool: &PgPool,
    principal: &Principal,
    range: crate::workbench::WorkbenchRange,
    limit: usize,
    as_of: OffsetDateTime,
) -> Result<crate::workbench::CalendarPage, crate::workbench::SourceFailure> {
    let limit = i64::try_from(limit).map_err(|_| crate::workbench::SourceFailure::Unavailable {
        code: "calendar_limit_invalid",
    })?;
    let snapshot =
        collect_calendar_events(pool, principal, range.from, range.to, limit, as_of, true)
            .await
            .map_err(|error| {
                if error.status == StatusCode::FORBIDDEN {
                    crate::workbench::SourceFailure::Denied {
                        code: "calendar_access_denied",
                    }
                } else {
                    crate::workbench::SourceFailure::Unavailable {
                        code: "calendar_unavailable",
                    }
                }
            })?;
    let items = snapshot
        .items
        .into_iter()
        .map(|item| crate::workbench::CalendarItem {
            id: item.id,
            title: item.title,
            starts_at: item.starts_at,
            ends_at: item.ends_at,
            target: crate::workbench::WorkbenchTarget {
                module: "overview".to_owned(),
                id: item.id.to_string(),
            },
        })
        .collect();
    Ok(crate::workbench::CalendarPage {
        as_of: snapshot.as_of,
        total: snapshot.total,
        items,
    })
}

async fn create_calendar_event(
    State(state): State<CollaborationState>,
    Extension(principal): Extension<Principal>,
    Json(body): Json<CreateCalendarEventRequest>,
) -> Result<Json<CalendarEventResponse>, CollaborationError> {
    authorize_collaboration_member(&principal)?;
    let normalized = normalize_calendar_event(body, principal.user_id.as_uuid())?;
    if let Err(denied) = authorize_scope_publisher(&principal, normalized.scope_type) {
        record_collaboration_request("calendar_create", "denied_scope");
        return Err(denied);
    }
    let event_id = Uuid::new_v4();
    let org = principal.org_id;
    let actor = principal.user_id;
    let trace = TraceContext::generate();
    let now = OffsetDateTime::now_utc();
    let audit_after = json!({
        "id": event_id,
        "scope_type": normalized.scope_type.as_db(),
        "scope_ref": normalized.scope_ref,
        "title": normalized.title,
        "object_type": normalized.object_type,
        "object_id": normalized.object_id,
    });
    let audit_event = AuditEvent::new(
        Some(actor),
        AuditAction::new("collaboration.calendar_event.create")?,
        "collaboration_calendar_event",
        event_id.to_string(),
        trace,
        now,
    )
    .with_org(org)
    .with_snapshots(None, Some(audit_after.clone()));
    let response = with_audit::<_, _, CollaborationError>(&state.pool, audit_event, move |tx| {
        Box::pin(async move {
            let row = sqlx::query(
                r#"
                INSERT INTO collaboration_calendar_events (
                    id, org_id, scope_type, scope_ref, title, description, starts_at, ends_at,
                    all_day, object_type, object_id, created_by, updated_by
                )
                VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $12)
                RETURNING id, scope_type, scope_ref, title, description, starts_at, ends_at,
                          all_day, status, object_type, object_id, created_by, created_at, updated_at
                "#,
            )
            .bind(event_id)
            .bind(*org.as_uuid())
            .bind(normalized.scope_type.as_db())
            .bind(&normalized.scope_ref)
            .bind(&normalized.title)
            .bind(&normalized.description)
            .bind(normalized.starts_at)
            .bind(normalized.ends_at)
            .bind(normalized.all_day)
            .bind(&normalized.object_type)
            .bind(normalized.object_id)
            .bind(*actor.as_uuid())
            .fetch_one(tx.as_mut())
            .await?;

            insert_calendar_lifecycle_event(
                tx,
                CalendarLifecycleEvent {
                    org,
                    event_id,
                    action: "collaboration.calendar_event.create",
                    actor: Some(actor),
                    summary: "일정 생성",
                    before_snap: None,
                    after_snap: Some(audit_after),
                },
            )
            .await?;

            calendar_event_from_row(row)
        })
    })
    .await?;
    record_collaboration_request("calendar_create", "success");
    Ok(Json(response))
}

async fn list_polls(
    State(state): State<CollaborationState>,
    Extension(principal): Extension<Principal>,
    Query(query): Query<PollQuery>,
) -> Result<Json<PollListResponse>, CollaborationError> {
    authorize_collaboration_member(&principal)?;
    let status = query.status.unwrap_or(PollStatus::Open);
    let limit = normalize_limit(query.limit);
    let org = principal.org_id;
    let user_ref = principal.user_id.as_uuid().to_string();
    let user_id = *principal.user_id.as_uuid();
    let (branch_all, branch_refs) = audience_branch_binds(&principal);
    let audience = scope_visibility_sql(
        "p.target_scope_type",
        "p.target_scope_ref",
        "p.created_by",
        "$2",
        "$3",
        "$5",
        "$6",
    );
    let sql = format!(
        "SELECT p.id, p.target_scope_type, p.target_scope_ref, p.title, p.question, \
                p.status, p.anonymity, p.allow_multiple, p.closes_at, \
                p.object_type, p.object_id, p.created_by, p.created_at, p.updated_at, \
                COALESCE(( \
                    SELECT jsonb_agg( \
                        jsonb_build_object( \
                            'id', o.id, \
                            'label', o.label, \
                            'position', o.position, \
                            'vote_count', COALESCE(( \
                                SELECT COUNT(*) \
                                FROM collaboration_poll_votes v \
                                WHERE v.poll_id = p.id \
                                  AND v.org_id = p.org_id \
                                  AND o.id = ANY(v.selected_option_ids) \
                            ), 0) \
                        ) \
                        ORDER BY o.position \
                    ) \
                    FROM collaboration_poll_options o \
                    WHERE o.poll_id = p.id \
                      AND o.org_id = p.org_id \
                ), '[]'::jsonb) AS options, \
                COALESCE(( \
                    SELECT COUNT(*) \
                    FROM collaboration_poll_votes v \
                    WHERE v.poll_id = p.id \
                      AND v.org_id = p.org_id \
                ), 0) AS vote_count, \
                ( \
                    SELECT v.selected_option_ids \
                    FROM collaboration_poll_votes v \
                    WHERE v.poll_id = p.id \
                      AND v.org_id = p.org_id \
                      AND v.voter_id = $3 \
                ) AS my_selected_option_ids \
         FROM collaboration_polls p \
         WHERE p.status = $1 \
           AND {audience} \
         ORDER BY p.created_at DESC \
         LIMIT $4"
    );
    let items = with_org_conn::<_, _, CollaborationError>(&state.pool, org, move |tx| {
        Box::pin(async move {
            let rows = sqlx::query(sqlx::AssertSqlSafe(sql))
                .bind(status.as_db())
                .bind(user_ref)
                .bind(user_id)
                .bind(limit)
                .bind(branch_all)
                .bind(&branch_refs)
                .fetch_all(tx.as_mut())
                .await?;
            rows.into_iter().map(poll_from_row).collect()
        })
    })
    .await?;
    record_collaboration_request("poll_list", "success");
    Ok(Json(PollListResponse { items }))
}

async fn create_poll(
    State(state): State<CollaborationState>,
    Extension(principal): Extension<Principal>,
    Json(body): Json<CreatePollRequest>,
) -> Result<Json<PollResponse>, CollaborationError> {
    authorize_collaboration_member(&principal)?;
    let normalized = normalize_poll(body, principal.user_id.as_uuid())?;
    if let Err(denied) = authorize_scope_publisher(&principal, normalized.target_scope_type) {
        record_collaboration_request("poll_create", "denied_scope");
        return Err(denied);
    }
    let poll_id = Uuid::new_v4();
    let org = principal.org_id;
    let actor = principal.user_id;
    let trace = TraceContext::generate();
    let now = OffsetDateTime::now_utc();
    let audit_after = json!({
        "id": poll_id,
        "target_scope_type": normalized.target_scope_type.as_db(),
        "target_scope_ref": normalized.target_scope_ref,
        "status": normalized.status.as_db(),
        "anonymity": normalized.anonymity.as_db(),
        "allow_multiple": normalized.allow_multiple,
        "object_type": normalized.object_type,
        "object_id": normalized.object_id,
    });
    let audit_event = AuditEvent::new(
        Some(actor),
        AuditAction::new("collaboration.poll.create")?,
        "collaboration_poll",
        poll_id.to_string(),
        trace,
        now,
    )
    .with_org(org)
    .with_snapshots(None, Some(audit_after.clone()));
    let response = with_audit::<_, _, CollaborationError>(&state.pool, audit_event, move |tx| {
        Box::pin(async move {
            sqlx::query(
                r#"
                INSERT INTO collaboration_polls (
                    id, org_id, target_scope_type, target_scope_ref, title, question,
                    status, anonymity, allow_multiple, closes_at, object_type, object_id,
                    created_by, updated_by
                )
                VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $13)
                "#,
            )
            .bind(poll_id)
            .bind(*org.as_uuid())
            .bind(normalized.target_scope_type.as_db())
            .bind(&normalized.target_scope_ref)
            .bind(&normalized.title)
            .bind(&normalized.question)
            .bind(normalized.status.as_db())
            .bind(normalized.anonymity.as_db())
            .bind(normalized.allow_multiple)
            .bind(normalized.closes_at)
            .bind(&normalized.object_type)
            .bind(normalized.object_id)
            .bind(*actor.as_uuid())
            .execute(tx.as_mut())
            .await?;

            for (position, label) in normalized.options.iter().enumerate() {
                sqlx::query(
                    r#"
                    INSERT INTO collaboration_poll_options (org_id, poll_id, label, position)
                    VALUES ($1, $2, $3, $4)
                    "#,
                )
                .bind(*org.as_uuid())
                .bind(poll_id)
                .bind(label)
                .bind(
                    i32::try_from(position)
                        .map_err(|_| CollaborationError::validation("too many poll options"))?,
                )
                .execute(tx.as_mut())
                .await?;
            }

            insert_poll_lifecycle_event(
                tx,
                PollLifecycleEvent {
                    org,
                    poll_id,
                    action: "collaboration.poll.create",
                    actor: Some(actor),
                    summary: "폴 생성",
                    before_snap: None,
                    after_snap: Some(audit_after),
                },
            )
            .await?;

            load_poll_response(tx, poll_id, *actor.as_uuid()).await
        })
    })
    .await?;
    record_collaboration_request("poll_create", "success");
    Ok(Json(response))
}

async fn vote_poll(
    State(state): State<CollaborationState>,
    Extension(principal): Extension<Principal>,
    Path(poll_id): Path<Uuid>,
    Json(body): Json<VotePollRequest>,
) -> Result<Json<PollResponse>, CollaborationError> {
    authorize_collaboration_member(&principal)?;
    let selected = normalize_selected_options(body.selected_option_ids)?;
    let response = submit_poll_vote(&state, &principal, poll_id, selected).await?;
    record_collaboration_request("poll_vote", "success");
    Ok(Json(response))
}

async fn vote_mobile_poll(
    State(state): State<CollaborationState>,
    Extension(principal): Extension<Principal>,
    Path(poll_id): Path<Uuid>,
    Json(body): Json<MobileVotePollRequest>,
) -> Result<Json<PollResponse>, CollaborationError> {
    authorize_collaboration_member(&principal)?;
    let selected = normalize_selected_options(body.selected_option_ids)?;
    verify_mobile_poll_step_up(
        &state,
        &principal,
        poll_id,
        body.step_up.ok_or_else(|| {
            CollaborationError::precondition_required(
                "passkey_step_up_required",
                "mobile poll vote requires a fresh passkey step-up",
            )
        })?,
    )
    .await?;
    let response = submit_poll_vote(&state, &principal, poll_id, selected).await?;
    record_collaboration_request("poll_vote", "success");
    Ok(Json(response))
}

async fn submit_poll_vote(
    state: &CollaborationState,
    principal: &Principal,
    poll_id: Uuid,
    selected: Vec<Uuid>,
) -> Result<PollResponse, CollaborationError> {
    let org = principal.org_id;
    let actor = principal.user_id;
    let user_ref = principal.user_id.as_uuid().to_string();
    let (branch_all, branch_refs) = audience_branch_binds(principal);
    let trace = TraceContext::generate();
    let now = OffsetDateTime::now_utc();
    let audit_after = json!({
        "id": poll_id,
        "selected_count": selected.len(),
    });
    let audit_event = AuditEvent::new(
        Some(actor),
        AuditAction::new("collaboration.poll.vote")?,
        "collaboration_poll",
        poll_id.to_string(),
        trace,
        now,
    )
    .with_org(org)
    .with_snapshots(None, Some(audit_after.clone()));
    with_audit::<_, _, CollaborationError>(&state.pool, audit_event, move |tx| {
        Box::pin(async move {
            let poll = load_poll_vote_policy(
                tx,
                poll_id,
                &user_ref,
                *actor.as_uuid(),
                branch_all,
                &branch_refs,
            )
            .await?;
            if poll.status != PollStatus::Open {
                return Err(CollaborationError::validation("poll is not open"));
            }
            if poll
                .closes_at
                .is_some_and(|closes_at| closes_at < OffsetDateTime::now_utc())
            {
                return Err(CollaborationError::validation("poll is closed"));
            }
            if !poll.allow_multiple && selected.len() != 1 {
                return Err(CollaborationError::validation(
                    "poll accepts exactly one option",
                ));
            }
            ensure_options_belong_to_poll(tx, poll_id, &selected).await?;
            sqlx::query(
                r#"
                INSERT INTO collaboration_poll_votes (
                    org_id, poll_id, voter_id, selected_option_ids
                )
                VALUES ($1, $2, $3, $4)
                ON CONFLICT (poll_id, voter_id) DO UPDATE SET
                    selected_option_ids = EXCLUDED.selected_option_ids,
                    updated_at = now()
                "#,
            )
            .bind(*org.as_uuid())
            .bind(poll_id)
            .bind(*actor.as_uuid())
            .bind(&selected)
            .execute(tx.as_mut())
            .await?;

            insert_poll_lifecycle_event(
                tx,
                PollLifecycleEvent {
                    org,
                    poll_id,
                    action: "collaboration.poll.vote",
                    actor: Some(actor),
                    summary: "투표 제출",
                    before_snap: None,
                    after_snap: Some(audit_after),
                },
            )
            .await?;

            load_poll_response(tx, poll_id, *actor.as_uuid()).await
        })
    })
    .await
}

async fn verify_mobile_poll_step_up(
    state: &CollaborationState,
    principal: &Principal,
    poll_id: Uuid,
    step_up: MobilePasskeyStepUpEnvelope,
) -> Result<(), CollaborationError> {
    step_up
        .binding
        .validate()
        .map_err(|err| CollaborationError::validation(err.to_string()))?;
    let expected_binding =
        MobilePasskeyStepUpBinding::poll_vote(poll_id, step_up.binding.replay_attempt);
    let verifier = state.passkey_step_up.as_ref().ok_or_else(|| {
        CollaborationError::unavailable("passkey step-up is not configured for collaboration API")
    })?;
    verifier
        .verify_mobile_step_up_for_user(
            &state.pool,
            step_up,
            *principal.user_id.as_uuid(),
            &expected_binding,
        )
        .await
        .map_err(collaboration_error_from_mobile_step_up)
}

fn collaboration_error_from_mobile_step_up(
    error: MobilePasskeyStepUpVerificationError,
) -> CollaborationError {
    match error {
        MobilePasskeyStepUpVerificationError::BindingMismatch => {
            CollaborationError::unauthorized_with_code(
                "passkey_step_up_binding_mismatch",
                "passkey step-up binding does not match the requested action",
            )
        }
        MobilePasskeyStepUpVerificationError::Auth(err) => {
            CollaborationError::unauthorized_with_code("passkey_step_up_failed", err.to_string())
        }
    }
}

#[derive(Debug)]
struct NormalizedCalendarEvent {
    scope_type: ScopeType,
    scope_ref: Option<String>,
    title: String,
    description: String,
    starts_at: OffsetDateTime,
    ends_at: OffsetDateTime,
    all_day: bool,
    object_type: Option<String>,
    object_id: Option<Uuid>,
}

#[derive(Debug)]
struct NormalizedPoll {
    target_scope_type: ScopeType,
    target_scope_ref: Option<String>,
    title: String,
    question: String,
    status: PollStatus,
    anonymity: PollAnonymity,
    allow_multiple: bool,
    closes_at: Option<OffsetDateTime>,
    options: Vec<String>,
    object_type: Option<String>,
    object_id: Option<Uuid>,
}

#[derive(Debug)]
struct PollVotePolicy {
    status: PollStatus,
    allow_multiple: bool,
    closes_at: Option<OffsetDateTime>,
}

fn normalize_calendar_event(
    body: CreateCalendarEventRequest,
    actor_id: &Uuid,
) -> Result<NormalizedCalendarEvent, CollaborationError> {
    if body.ends_at < body.starts_at {
        return Err(CollaborationError::validation(
            "calendar event end must be after start",
        ));
    }
    let (object_type, object_id) = normalize_object_link(body.object_type, body.object_id)?;
    Ok(NormalizedCalendarEvent {
        scope_type: body.scope_type,
        scope_ref: normalize_scope_ref(body.scope_type, body.scope_ref, actor_id)?,
        title: normalize_required_text(&body.title, "title", 160)?,
        description: normalize_optional_text(&body.description, 2000)?,
        starts_at: body.starts_at,
        ends_at: body.ends_at,
        all_day: body.all_day,
        object_type,
        object_id,
    })
}

fn normalize_poll(
    body: CreatePollRequest,
    actor_id: &Uuid,
) -> Result<NormalizedPoll, CollaborationError> {
    let (object_type, object_id) = normalize_object_link(body.object_type, body.object_id)?;
    let mut options = Vec::new();
    let mut seen = BTreeSet::new();
    for option in body.options {
        let label = normalize_required_text(&option, "poll option", 240)?;
        if !seen.insert(label.to_lowercase()) {
            return Err(CollaborationError::validation(
                "poll options must be unique",
            ));
        }
        options.push(label);
    }
    if options.len() < 2 {
        return Err(CollaborationError::validation(
            "poll requires at least two options",
        ));
    }
    if options.len() > MAX_POLL_OPTIONS {
        return Err(CollaborationError::validation("poll has too many options"));
    }
    Ok(NormalizedPoll {
        target_scope_type: body.target_scope_type,
        target_scope_ref: normalize_scope_ref(
            body.target_scope_type,
            body.target_scope_ref,
            actor_id,
        )?,
        title: normalize_required_text(&body.title, "title", 160)?,
        question: normalize_required_text(&body.question, "question", 1000)?,
        status: body.status,
        anonymity: body.anonymity,
        allow_multiple: body.allow_multiple,
        closes_at: body.closes_at,
        options,
        object_type,
        object_id,
    })
}

fn normalize_scope_ref(
    scope_type: ScopeType,
    scope_ref: Option<String>,
    actor_id: &Uuid,
) -> Result<Option<String>, CollaborationError> {
    if scope_type == ScopeType::Personal {
        return Ok(Some(actor_id.to_string()));
    }
    let normalized = scope_ref
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty());
    if normalized
        .as_ref()
        .is_some_and(|value| value.chars().count() > 160)
    {
        return Err(CollaborationError::validation(
            "scope_ref must be 160 characters or less",
        ));
    }
    match scope_type {
        // Tenant-/org-wide rows address everyone; a stored ref would be
        // decorative and would desync from the visibility predicate.
        ScopeType::Tenant | ScopeType::Org => Ok(None),
        // DEPARTMENT audiences are branch ids (the ontology maps 조직/부서 to
        // `branches`); anything that does not parse as a uuid could never
        // match a caller's BranchScope and would create an unreachable row.
        ScopeType::Department => {
            let value = normalized.ok_or_else(|| {
                CollaborationError::validation("DEPARTMENT scope requires scope_ref")
            })?;
            let branch = Uuid::parse_str(&value).map_err(|_| {
                CollaborationError::validation("DEPARTMENT scope_ref must be a branch id")
            })?;
            Ok(Some(branch.to_string()))
        }
        ScopeType::Team => normalized
            .map(Some)
            .ok_or_else(|| CollaborationError::validation("TEAM scope requires scope_ref")),
        ScopeType::Personal => unreachable!("handled above"),
    }
}

fn normalize_object_link(
    object_type: Option<String>,
    object_id: Option<Uuid>,
) -> Result<(Option<String>, Option<Uuid>), CollaborationError> {
    match (
        object_type
            .map(|value| value.trim().to_ascii_lowercase())
            .filter(|value| !value.is_empty()),
        object_id,
    ) {
        (None, None) => Ok((None, None)),
        (Some(kind), Some(id)) if is_safe_object_type(&kind) => Ok((Some(kind), Some(id))),
        (Some(_), Some(_)) => Err(CollaborationError::validation("invalid object_type")),
        _ => Err(CollaborationError::validation(
            "object_type and object_id must be supplied together",
        )),
    }
}

fn normalize_required_text(
    raw: &str,
    field: &'static str,
    max_chars: usize,
) -> Result<String, CollaborationError> {
    let value = raw.trim();
    if value.is_empty() {
        return Err(CollaborationError::validation(format!(
            "{field} is required"
        )));
    }
    if value.chars().count() > max_chars {
        return Err(CollaborationError::validation(format!(
            "{field} must be {max_chars} characters or less"
        )));
    }
    Ok(value.to_owned())
}

fn normalize_optional_text(raw: &str, max_chars: usize) -> Result<String, CollaborationError> {
    let value = raw.trim();
    if value.chars().count() > max_chars {
        return Err(CollaborationError::validation(format!(
            "description must be {max_chars} characters or less"
        )));
    }
    Ok(value.to_owned())
}

fn normalize_selected_options(mut selected: Vec<Uuid>) -> Result<Vec<Uuid>, CollaborationError> {
    selected.sort_unstable();
    selected.dedup();
    if selected.is_empty() {
        return Err(CollaborationError::validation(
            "selected_option_ids is required",
        ));
    }
    if selected.len() > MAX_POLL_OPTIONS {
        return Err(CollaborationError::validation(
            "selected_option_ids has too many entries",
        ));
    }
    Ok(selected)
}

fn is_safe_object_type(raw: &str) -> bool {
    let mut chars = raw.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    first.is_ascii_lowercase()
        && raw.len() <= 64
        && chars.all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_')
}

fn normalize_limit(limit: Option<i64>) -> i64 {
    limit.unwrap_or(DEFAULT_LIST_LIMIT).clamp(1, MAX_LIST_LIMIT)
}

fn calendar_event_from_row(
    row: sqlx::postgres::PgRow,
) -> Result<CalendarEventResponse, CollaborationError> {
    let scope_raw: String = row.try_get("scope_type")?;
    let status_raw: String = row.try_get("status")?;
    let scope_type = ScopeType::from_db(&scope_raw)?;
    let scope_ref: Option<String> = row.try_get("scope_ref")?;
    Ok(CalendarEventResponse {
        id: row.try_get("id")?,
        scope_type,
        scope_ref: scope_ref.clone(),
        title: row.try_get("title")?,
        description: row.try_get("description")?,
        starts_at: row.try_get("starts_at")?,
        ends_at: row.try_get("ends_at")?,
        all_day: row.try_get("all_day")?,
        status: CalendarEventStatus::from_db(&status_raw)?,
        object_type: row.try_get("object_type")?,
        object_id: row.try_get("object_id")?,
        created_by: row.try_get("created_by")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
        policy: scope_policy(scope_type, scope_ref),
    })
}

fn poll_from_row(row: sqlx::postgres::PgRow) -> Result<PollResponse, CollaborationError> {
    let scope_raw: String = row.try_get("target_scope_type")?;
    let status_raw: String = row.try_get("status")?;
    let anonymity_raw: String = row.try_get("anonymity")?;
    let scope_type = ScopeType::from_db(&scope_raw)?;
    let scope_ref: Option<String> = row.try_get("target_scope_ref")?;
    let options_json: Value = row.try_get("options")?;
    let options: Vec<PollOptionResponse> = serde_json::from_value(options_json).map_err(|err| {
        CollaborationError::validation(format!("invalid poll option payload: {err}"))
    })?;
    let my_selected_option_ids: Option<Vec<Uuid>> = row.try_get("my_selected_option_ids")?;
    Ok(PollResponse {
        id: row.try_get("id")?,
        target_scope_type: scope_type,
        target_scope_ref: scope_ref.clone(),
        title: row.try_get("title")?,
        question: row.try_get("question")?,
        status: PollStatus::from_db(&status_raw)?,
        anonymity: PollAnonymity::from_db(&anonymity_raw)?,
        allow_multiple: row.try_get("allow_multiple")?,
        closes_at: row.try_get("closes_at")?,
        object_type: row.try_get("object_type")?,
        object_id: row.try_get("object_id")?,
        options,
        vote_count: row.try_get("vote_count")?,
        my_vote: PollMyVote {
            submitted: my_selected_option_ids.is_some(),
            selected_option_ids: my_selected_option_ids,
        },
        created_by: row.try_get("created_by")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
        policy: scope_policy(scope_type, scope_ref),
    })
}

async fn load_poll_response(
    tx: &mut Transaction<'_, Postgres>,
    poll_id: Uuid,
    voter_id: Uuid,
) -> Result<PollResponse, CollaborationError> {
    let row = sqlx::query(
        r#"
        SELECT p.id, p.target_scope_type, p.target_scope_ref, p.title, p.question,
               p.status, p.anonymity, p.allow_multiple, p.closes_at,
               p.object_type, p.object_id, p.created_by, p.created_at, p.updated_at,
               COALESCE((
                   SELECT jsonb_agg(
                       jsonb_build_object(
                           'id', o.id,
                           'label', o.label,
                           'position', o.position,
                           'vote_count', COALESCE((
                               SELECT COUNT(*)
                               FROM collaboration_poll_votes v
                               WHERE v.poll_id = p.id
                                 AND v.org_id = p.org_id
                                 AND o.id = ANY(v.selected_option_ids)
                           ), 0)
                       )
                       ORDER BY o.position
                   )
                   FROM collaboration_poll_options o
                   WHERE o.poll_id = p.id
                     AND o.org_id = p.org_id
               ), '[]'::jsonb) AS options,
               COALESCE((
                   SELECT COUNT(*)
                   FROM collaboration_poll_votes v
                   WHERE v.poll_id = p.id
                     AND v.org_id = p.org_id
               ), 0) AS vote_count,
               (
                   SELECT v.selected_option_ids
                   FROM collaboration_poll_votes v
                   WHERE v.poll_id = p.id
                     AND v.org_id = p.org_id
                     AND v.voter_id = $2
               ) AS my_selected_option_ids
        FROM collaboration_polls p
        WHERE p.id = $1
        "#,
    )
    .bind(poll_id)
    .bind(voter_id)
    .fetch_optional(tx.as_mut())
    .await?
    .ok_or_else(|| CollaborationError::not_found("poll not found"))?;
    poll_from_row(row)
}

/// Loads the vote policy for a poll the caller is allowed to address. The
/// audience predicate is part of the lookup, so a poll outside the caller's
/// scope membership is indistinguishable from a missing poll (`not_found`,
/// no existence leak) and can never be voted on.
async fn load_poll_vote_policy(
    tx: &mut Transaction<'_, Postgres>,
    poll_id: Uuid,
    user_ref: &str,
    user_id: Uuid,
    branch_all: bool,
    branch_refs: &[String],
) -> Result<PollVotePolicy, CollaborationError> {
    let audience = scope_visibility_sql(
        "target_scope_type",
        "target_scope_ref",
        "created_by",
        "$2",
        "$3",
        "$4",
        "$5",
    );
    let sql = format!(
        "SELECT status, allow_multiple, closes_at \
         FROM collaboration_polls \
         WHERE id = $1 AND {audience}"
    );
    let row = sqlx::query(sqlx::AssertSqlSafe(sql))
        .bind(poll_id)
        .bind(user_ref)
        .bind(user_id)
        .bind(branch_all)
        .bind(branch_refs)
        .fetch_optional(tx.as_mut())
        .await?
        .ok_or_else(|| CollaborationError::not_found("poll not found"))?;
    let status_raw: String = row.try_get("status")?;
    Ok(PollVotePolicy {
        status: PollStatus::from_db(&status_raw)?,
        allow_multiple: row.try_get("allow_multiple")?,
        closes_at: row.try_get("closes_at")?,
    })
}

async fn ensure_options_belong_to_poll(
    tx: &mut Transaction<'_, Postgres>,
    poll_id: Uuid,
    selected: &[Uuid],
) -> Result<(), CollaborationError> {
    let count: i64 = sqlx::query_scalar(
        r#"
        SELECT COUNT(*)
        FROM collaboration_poll_options
        WHERE poll_id = $1
          AND id = ANY($2)
        "#,
    )
    .bind(poll_id)
    .bind(selected)
    .fetch_one(tx.as_mut())
    .await?;
    if usize::try_from(count).unwrap_or(0) != selected.len() {
        return Err(CollaborationError::validation(
            "selected options must belong to the poll",
        ));
    }
    Ok(())
}

struct CalendarLifecycleEvent {
    org: console_kernel_core::OrgId,
    event_id: Uuid,
    action: &'static str,
    actor: Option<console_kernel_core::UserId>,
    summary: &'static str,
    before_snap: Option<Value>,
    after_snap: Option<Value>,
}

async fn insert_calendar_lifecycle_event(
    tx: &mut Transaction<'_, Postgres>,
    event: CalendarLifecycleEvent,
) -> Result<(), CollaborationError> {
    sqlx::query(
        r#"
        INSERT INTO collaboration_calendar_event_events (
            org_id, event_id, action, actor_id, summary, before_snap, after_snap
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7)
        "#,
    )
    .bind(*event.org.as_uuid())
    .bind(event.event_id)
    .bind(event.action)
    .bind(event.actor.map(|user| *user.as_uuid()))
    .bind(event.summary)
    .bind(event.before_snap)
    .bind(event.after_snap)
    .execute(tx.as_mut())
    .await?;
    Ok(())
}

struct PollLifecycleEvent {
    org: console_kernel_core::OrgId,
    poll_id: Uuid,
    action: &'static str,
    actor: Option<console_kernel_core::UserId>,
    summary: &'static str,
    before_snap: Option<Value>,
    after_snap: Option<Value>,
}

async fn insert_poll_lifecycle_event(
    tx: &mut Transaction<'_, Postgres>,
    event: PollLifecycleEvent,
) -> Result<(), CollaborationError> {
    sqlx::query(
        r#"
        INSERT INTO collaboration_poll_events (
            org_id, poll_id, action, actor_id, summary, before_snap, after_snap
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7)
        "#,
    )
    .bind(*event.org.as_uuid())
    .bind(event.poll_id)
    .bind(event.action)
    .bind(event.actor.map(|user| *user.as_uuid()))
    .bind(event.summary)
    .bind(event.before_snap)
    .bind(event.after_snap)
    .execute(tx.as_mut())
    .await?;
    Ok(())
}

fn scope_policy(scope_type: ScopeType, scope_ref: Option<String>) -> CollaborationScopePolicy {
    CollaborationScopePolicy {
        enforcement: "server",
        scope_type,
        scope_ref,
        visibility: match scope_type {
            ScopeType::Personal => "creator_only",
            ScopeType::Tenant | ScopeType::Org => "org_members",
            ScopeType::Department => "department_target",
            ScopeType::Team => "team_target",
        },
    }
}

/// Bare tenant-membership gate (`Feature::Login`). This is deliberately only
/// the OUTER door: row visibility is decided per row by
/// [`scope_visibility_sql`], and creating rows above PERSONAL scope requires
/// the separate [`authorize_scope_publisher`] grant.
fn authorize_collaboration_member(principal: &Principal) -> Result<(), CollaborationError> {
    let allowed_by_role = principal
        .roles
        .iter()
        .any(|role| permission_for(*role, Feature::Login) == PermissionLevel::Allow);
    let allowed_by_custom_grant = principal
        .effective_feature_grants
        .iter()
        .any(|grant| grant.feature == Feature::Login && grant.permission == PermissionLevel::Allow);
    if allowed_by_role || allowed_by_custom_grant {
        return Ok(());
    }
    Err(CollaborationError::from_kernel(KernelError::forbidden(
        "collaboration requires an authenticated tenant member",
    )))
}

/// Addressing an audience wider than yourself is the announcement tier, not
/// bare login: TENANT/ORG/DEPARTMENT/TEAM creation requires
/// [`Feature::NoticeManage`] from the built-in role matrix (ADMIN, EXECUTIVE,
/// SUPER_ADMIN) or an explicit Allow custom grant. PERSONAL stays Login-tier;
/// its `scope_ref` is pinned to the actor by [`normalize_scope_ref`].
fn authorize_scope_publisher(
    principal: &Principal,
    scope_type: ScopeType,
) -> Result<(), CollaborationError> {
    if scope_type == ScopeType::Personal {
        return Ok(());
    }
    let allowed_by_role = principal
        .roles
        .iter()
        .any(|role| permission_for(*role, Feature::NoticeManage) == PermissionLevel::Allow);
    let allowed_by_custom_grant = principal.effective_feature_grants.iter().any(|grant| {
        grant.feature == Feature::NoticeManage && grant.permission == PermissionLevel::Allow
    });
    if allowed_by_role || allowed_by_custom_grant {
        return Ok(());
    }
    Err(CollaborationError::from_kernel(KernelError::forbidden(
        "creating shared-scope collaboration content requires notice-manage authority",
    )))
}

/// Bind values for [`scope_visibility_sql`], resolved from the caller's
/// kernel [`BranchScope`] (the platform's branch-membership authority, minted
/// from `user_branches` at token issuance and narrowed by claim scope).
/// `BranchScope::All` is the SUPER_ADMIN/EXECUTIVE org-wide rollup tier.
fn audience_branch_binds(principal: &Principal) -> (bool, Vec<String>) {
    match &principal.branch_scope {
        BranchScope::All => (true, Vec::new()),
        BranchScope::Branches(set) => (
            false,
            set.iter()
                .map(|branch| branch.as_uuid().to_string())
                .collect(),
        ),
    }
}

/// The ONE audience predicate for scoped collaboration rows (calendar events
/// and polls share it; per-query respellings of this rule are how §4-32
/// shipped, so add call sites instead of copies). Stated audience rules:
///
/// - `TENANT` / `ORG`: every authenticated member of the tenant. RLS
///   (`org_isolation` on `app.current_org`) pins the tenant; only the
///   notice-manage tier can create these rows.
/// - `DEPARTMENT`: `scope_ref` must be one of the caller's branch ids
///   (the ontology maps 조직/부서 to `branches`; membership authority is the
///   kernel `BranchScope`, `All` = org-wide rollup).
/// - `TEAM`: `scope_ref` must equal the caller's live `users.team` value.
/// - `PERSONAL`: the caller's own ref, or rows the caller created.
/// - Creators always see their own rows.
///
/// Every column/bind placeholder is a `'static` literal supplied by the call
/// site (the signature rejects runtime strings, so request data structurally
/// cannot reach the SQL text; the composed query is `AssertSqlSafe` on that
/// basis). Unknown scope values match no arm and NULL refs never compare
/// equal, so both fail closed to invisible.
fn scope_visibility_sql(
    scope_type_col: &'static str,
    scope_ref_col: &'static str,
    created_by_col: &'static str,
    user_ref_bind: &'static str,
    user_id_bind: &'static str,
    branch_all_bind: &'static str,
    branch_refs_bind: &'static str,
) -> String {
    format!(
        "(\
            {scope_type_col} IN ('TENANT','ORG') \
            OR ({scope_type_col} = 'DEPARTMENT' \
                AND ({branch_all_bind} OR {scope_ref_col} = ANY({branch_refs_bind}))) \
            OR ({scope_type_col} = 'TEAM' AND EXISTS (\
                SELECT 1 FROM users audience_member \
                WHERE audience_member.id = {user_id_bind} \
                  AND audience_member.team = {scope_ref_col})) \
            OR ({scope_type_col} = 'PERSONAL' \
                AND ({scope_ref_col} = {user_ref_bind} OR {created_by_col} = {user_id_bind})) \
            OR {created_by_col} = {user_id_bind}\
        )"
    )
}

fn record_collaboration_request(surface: &'static str, outcome: &'static str) {
    metrics::counter!(COLLABORATION_REQUESTS_TOTAL, "surface" => surface, "outcome" => outcome)
        .increment(1);
}

#[derive(Debug)]
struct CollaborationError {
    status: StatusCode,
    code: &'static str,
    message: String,
}

impl CollaborationError {
    fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
        }
    }

    fn from_kernel(error: KernelError) -> Self {
        let status = match error.kind {
            ErrorKind::Validation => StatusCode::UNPROCESSABLE_ENTITY,
            ErrorKind::NotFound => StatusCode::NOT_FOUND,
            ErrorKind::Forbidden => StatusCode::FORBIDDEN,
            ErrorKind::Conflict | ErrorKind::InvalidTransition => StatusCode::CONFLICT,
            ErrorKind::Internal => StatusCode::INTERNAL_SERVER_ERROR,
        };
        Self {
            status,
            code: error_code(error.kind),
            message: error.message,
        }
    }

    fn validation(message: impl Into<String>) -> Self {
        Self::from_kernel(KernelError::validation(message.into()))
    }

    fn not_found(message: impl Into<String>) -> Self {
        Self::from_kernel(KernelError::not_found(message.into()))
    }

    fn precondition_required(code: &'static str, message: impl Into<String>) -> Self {
        Self::new(StatusCode::PRECONDITION_REQUIRED, code, message)
    }

    fn unauthorized_with_code(code: &'static str, message: impl Into<String>) -> Self {
        Self::new(StatusCode::UNAUTHORIZED, code, message)
    }

    fn unavailable(message: impl Into<String>) -> Self {
        Self::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "service_unavailable",
            message,
        )
    }

    fn internal(message: impl Into<String>) -> Self {
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, "internal", message)
    }
}

impl From<KernelError> for CollaborationError {
    fn from(error: KernelError) -> Self {
        Self::from_kernel(error)
    }
}

impl From<DbError> for CollaborationError {
    fn from(value: DbError) -> Self {
        tracing::error!(error = %value, "collaboration database operation failed");
        Self::internal("collaboration request failed")
    }
}

impl From<sqlx::Error> for CollaborationError {
    fn from(value: sqlx::Error) -> Self {
        Self::from(DbError::Sqlx(value))
    }
}

impl IntoResponse for CollaborationError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(json!({ "error": { "code": self.code, "message": self.message } })),
        )
            .into_response()
    }
}

fn error_code(kind: ErrorKind) -> &'static str {
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

    #[cfg(not(feature = "test-postgres"))]
    #[test]
    fn personal_scope_is_pinned_to_actor() -> Result<(), String> {
        let actor = Uuid::parse_str("00000000-0000-4000-8000-000000000001")
            .map_err(|err| err.to_string())?;
        let scope = normalize_scope_ref(ScopeType::Personal, Some("attacker".to_owned()), &actor)
            .map_err(|err| err.message)?;
        assert_eq!(scope, Some(actor.to_string()));
        Ok(())
    }

    #[cfg(not(feature = "test-postgres"))]
    #[test]
    fn poll_options_are_unique_and_object_link_is_paired() -> Result<(), String> {
        let actor = Uuid::parse_str("00000000-0000-4000-8000-000000000001")
            .map_err(|err| err.to_string())?;
        let starts = OffsetDateTime::now_utc();
        let poll = CreatePollRequest {
            target_scope_type: ScopeType::Org,
            target_scope_ref: None,
            title: "중복".to_owned(),
            question: "선택".to_owned(),
            status: PollStatus::Open,
            anonymity: PollAnonymity::Named,
            allow_multiple: false,
            closes_at: Some(starts),
            options: vec!["A".to_owned(), " a ".to_owned()],
            object_type: Some("work_order".to_owned()),
            object_id: None,
        };

        let err = match normalize_poll(poll, &actor) {
            Ok(_) => return Err("duplicate poll options should fail validation".to_owned()),
            Err(err) => err,
        };

        assert_eq!(err.code, "validation");
        Ok(())
    }

    // ------------------------------------------------------------------
    // PostgreSQL scope-authorization coverage (console-l6c).
    //
    // These tests are the executable oracle for the §4-32 audience holes:
    // before the membership predicate and the publisher gate existed, every
    // authenticated tenant member read every non-PERSONAL row and could
    // create rows at any scope. Each test seeds rows AS ANOTHER USER so the
    // created_by ownership arm cannot mask a missing membership check.
    // ------------------------------------------------------------------

    #[cfg(feature = "test-postgres")]
    use console_kernel_core::{BranchId, OrgId, UserId};
    #[cfg(feature = "test-postgres")]
    use console_platform_authz::{EffectiveFeatureGrant, Role};
    #[cfg(feature = "test-postgres")]
    use std::collections::BTreeSet;

    #[cfg(feature = "test-postgres")]
    async fn seed_org(pool: &sqlx::PgPool) -> OrgId {
        let org = OrgId::new();
        sqlx::query("INSERT INTO organizations (id, slug, name) VALUES ($1, $2, $3)")
            .bind(*org.as_uuid())
            .bind(format!("collab-{}", &org.as_uuid().to_string()[..8]))
            .bind("Collaboration Scope Test")
            .execute(pool)
            .await
            .expect("seed organization");
        org
    }

    #[cfg(feature = "test-postgres")]
    async fn seed_user(pool: &sqlx::PgPool, org: OrgId, role: &str, team: Option<&str>) -> UserId {
        let user = UserId::new();
        sqlx::query(
            "INSERT INTO users (id, display_name, roles, team, is_active, org_id) \
             VALUES ($1, $2, ARRAY[$3]::TEXT[], $4, true, $5)",
        )
        .bind(*user.as_uuid())
        .bind(format!("collab-{}", &user.as_uuid().to_string()[..8]))
        .bind(role)
        .bind(team)
        .bind(*org.as_uuid())
        .execute(pool)
        .await
        .expect("seed user");
        user
    }

    #[cfg(feature = "test-postgres")]
    async fn seed_event(
        pool: &sqlx::PgPool,
        org: OrgId,
        scope_type: &str,
        scope_ref: Option<String>,
        created_by: UserId,
        title: &str,
    ) -> Uuid {
        let id = Uuid::new_v4();
        let starts = OffsetDateTime::now_utc();
        sqlx::query(
            "INSERT INTO collaboration_calendar_events (\
                id, org_id, scope_type, scope_ref, title, description, starts_at, ends_at, \
                all_day, status, created_by, updated_by\
             ) VALUES ($1, $2, $3, $4, $5, '', $6, $7, FALSE, 'ACTIVE', $8, $8)",
        )
        .bind(id)
        .bind(*org.as_uuid())
        .bind(scope_type)
        .bind(scope_ref)
        .bind(title)
        .bind(starts)
        .bind(starts + time::Duration::minutes(30))
        .bind(*created_by.as_uuid())
        .execute(pool)
        .await
        .expect("seed calendar event");
        id
    }

    #[cfg(feature = "test-postgres")]
    async fn seed_poll(
        pool: &sqlx::PgPool,
        org: OrgId,
        scope_type: &str,
        scope_ref: Option<String>,
        created_by: UserId,
        title: &str,
    ) -> (Uuid, Uuid) {
        let poll_id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO collaboration_polls (\
                id, org_id, target_scope_type, target_scope_ref, title, question, status, \
                anonymity, allow_multiple, created_by, updated_by\
             ) VALUES ($1, $2, $3, $4, $5, 'pick one', 'OPEN', 'NAMED', false, $6, $6)",
        )
        .bind(poll_id)
        .bind(*org.as_uuid())
        .bind(scope_type)
        .bind(scope_ref)
        .bind(title)
        .bind(*created_by.as_uuid())
        .execute(pool)
        .await
        .expect("seed poll");
        let option_id: Uuid = sqlx::query_scalar(
            "INSERT INTO collaboration_poll_options (org_id, poll_id, label, position) \
             VALUES ($1, $2, 'A', 0) RETURNING id",
        )
        .bind(*org.as_uuid())
        .bind(poll_id)
        .fetch_one(pool)
        .await
        .expect("seed poll option");
        sqlx::query(
            "INSERT INTO collaboration_poll_options (org_id, poll_id, label, position) \
             VALUES ($1, $2, 'B', 1)",
        )
        .bind(*org.as_uuid())
        .bind(poll_id)
        .execute(pool)
        .await
        .expect("seed second poll option");
        (poll_id, option_id)
    }

    #[cfg(feature = "test-postgres")]
    fn principal_of(
        user: UserId,
        org: OrgId,
        role: Role,
        branch_scope: console_kernel_core::BranchScope,
    ) -> Principal {
        Principal::new(user, org, BTreeSet::from([role]), branch_scope)
    }

    #[cfg(feature = "test-postgres")]
    fn collab_state(pool: &sqlx::PgPool) -> CollaborationState {
        CollaborationState::new(pool.clone(), None)
    }

    #[cfg(feature = "test-postgres")]
    async fn visible_calendar_ids(
        pool: &sqlx::PgPool,
        principal: &Principal,
    ) -> std::collections::BTreeSet<Uuid> {
        let now = OffsetDateTime::now_utc();
        let snapshot = collect_calendar_events(
            pool,
            principal,
            now - time::Duration::hours(1),
            now + time::Duration::hours(2),
            50,
            now + time::Duration::minutes(5),
            false,
        )
        .await
        .expect("calendar visibility query");
        snapshot.items.into_iter().map(|item| item.id).collect()
    }

    #[cfg(feature = "test-postgres")]
    #[sqlx::test(migrations = "../crates/platform/db/migrations")]
    async fn department_and_team_calendar_rows_are_hidden_outside_membership(pool: sqlx::PgPool) {
        let org = seed_org(&pool).await;
        let author = seed_user(&pool, org, "ADMIN", None).await;
        let viewer = seed_user(&pool, org, "MEMBER", Some("정비")).await;
        let branch_x = BranchId::new();
        let branch_y = BranchId::new();

        let tenant_row = seed_event(&pool, org, "TENANT", None, author, "tenant row").await;
        let dept_x_row = seed_event(
            &pool,
            org,
            "DEPARTMENT",
            Some(branch_x.as_uuid().to_string()),
            author,
            "department x row",
        )
        .await;
        let dept_y_row = seed_event(
            &pool,
            org,
            "DEPARTMENT",
            Some(branch_y.as_uuid().to_string()),
            author,
            "department y row",
        )
        .await;
        let team_mine_row = seed_event(
            &pool,
            org,
            "TEAM",
            Some("정비".to_owned()),
            author,
            "my team row",
        )
        .await;
        let team_other_row = seed_event(
            &pool,
            org,
            "TEAM",
            Some("예방".to_owned()),
            author,
            "other team row",
        )
        .await;
        let author_personal_row = seed_event(
            &pool,
            org,
            "PERSONAL",
            Some(author.as_uuid().to_string()),
            author,
            "author personal row",
        )
        .await;

        let member = principal_of(
            viewer,
            org,
            Role::Member,
            console_kernel_core::BranchScope::single(branch_x),
        );
        let visible = visible_calendar_ids(&pool, &member).await;
        assert!(
            visible.contains(&tenant_row),
            "tenant row must stay visible"
        );
        assert!(
            visible.contains(&dept_x_row),
            "own-department row must be visible"
        );
        assert!(
            visible.contains(&team_mine_row),
            "own-team row must be visible"
        );
        assert!(
            !visible.contains(&dept_y_row),
            "member of department X must NOT read department Y rows"
        );
        assert!(
            !visible.contains(&team_other_row),
            "member of team 정비 must NOT read team 예방 rows"
        );
        assert!(
            !visible.contains(&author_personal_row),
            "another user's personal row must stay hidden"
        );

        // SUPER_ADMIN/EXECUTIVE rollup (BranchScope::All) keeps org-wide reach.
        let executive = principal_of(
            seed_user(&pool, org, "EXECUTIVE", None).await,
            org,
            Role::Executive,
            console_kernel_core::BranchScope::All,
        );
        let rollup = visible_calendar_ids(&pool, &executive).await;
        assert!(rollup.contains(&dept_x_row) && rollup.contains(&dept_y_row));
        assert!(
            !rollup.contains(&author_personal_row),
            "rollup must not expose another user's personal row"
        );
    }

    #[cfg(feature = "test-postgres")]
    #[sqlx::test(migrations = "../crates/platform/db/migrations")]
    async fn calendar_create_above_personal_requires_notice_manage(pool: sqlx::PgPool) {
        let org = seed_org(&pool).await;
        let member_user = seed_user(&pool, org, "MEMBER", None).await;
        let admin_user = seed_user(&pool, org, "ADMIN", None).await;
        let branch = BranchId::new();
        let state = collab_state(&pool);

        let request =
            |scope_type: ScopeType, scope_ref: Option<String>| CreateCalendarEventRequest {
                scope_type,
                scope_ref,
                title: "scope legality".to_owned(),
                description: String::new(),
                starts_at: OffsetDateTime::now_utc(),
                ends_at: OffsetDateTime::now_utc() + time::Duration::minutes(30),
                all_day: false,
                object_type: None,
                object_id: None,
            };

        let member = principal_of(
            member_user,
            org,
            Role::Member,
            console_kernel_core::BranchScope::single(branch),
        );
        let denied = create_calendar_event(
            State(state.clone()),
            Extension(member.clone()),
            Json(request(ScopeType::Tenant, None)),
        )
        .await;
        match denied {
            Ok(_) => panic!("bare-login member must not create a TENANT-scoped event"),
            Err(err) => assert_eq!(
                err.status,
                StatusCode::FORBIDDEN,
                "expected forbidden, got {}: {}",
                err.status,
                err.message
            ),
        }

        let personal = create_calendar_event(
            State(state.clone()),
            Extension(member.clone()),
            Json(request(ScopeType::Personal, None)),
        )
        .await
        .expect("member keeps PERSONAL creation");
        assert_eq!(
            personal.0.scope_ref,
            Some(member_user.as_uuid().to_string()),
            "personal scope stays pinned to the actor"
        );

        let admin = principal_of(
            admin_user,
            org,
            Role::Admin,
            console_kernel_core::BranchScope::single(branch),
        );
        let created = create_calendar_event(
            State(state.clone()),
            Extension(admin.clone()),
            Json(request(ScopeType::Tenant, Some("ignored".to_owned()))),
        )
        .await
        .expect("admin holds the notice-manage tier");
        assert_eq!(
            created.0.scope_ref, None,
            "tenant/org rows must not store a decorative scope_ref"
        );

        // A custom NoticeManage grant (not the built-in matrix) also qualifies.
        let mut granted_member = principal_of(
            seed_user(&pool, org, "MEMBER", None).await,
            org,
            Role::Member,
            console_kernel_core::BranchScope::single(branch),
        );
        granted_member.effective_feature_grants = vec![EffectiveFeatureGrant::new(
            Feature::NoticeManage,
            PermissionLevel::Allow,
            console_kernel_core::BranchScope::All,
        )];
        let _ = create_calendar_event(
            State(state.clone()),
            Extension(granted_member.clone()),
            Json(request(
                ScopeType::Department,
                Some(branch.as_uuid().to_string()),
            )),
        )
        .await
        .expect("custom NoticeManage grant may create a department event");

        let junk_department = create_calendar_event(
            State(state.clone()),
            Extension(granted_member),
            Json(request(ScopeType::Department, Some("총무팀".to_owned()))),
        )
        .await;
        match junk_department {
            Ok(_) => panic!("department scope_ref must be a branch id"),
            Err(err) => assert_eq!(err.status, StatusCode::UNPROCESSABLE_ENTITY),
        }

        let missing_team_ref = create_calendar_event(
            State(state),
            Extension(admin),
            Json(request(ScopeType::Team, None)),
        )
        .await;
        match missing_team_ref {
            Ok(_) => panic!("team scope requires a scope_ref"),
            Err(err) => assert_eq!(err.status, StatusCode::UNPROCESSABLE_ENTITY),
        }
    }

    #[cfg(feature = "test-postgres")]
    #[sqlx::test(migrations = "../crates/platform/db/migrations")]
    async fn poll_visibility_and_create_follow_scope_membership(pool: sqlx::PgPool) {
        let org = seed_org(&pool).await;
        let author = seed_user(&pool, org, "ADMIN", None).await;
        let viewer = seed_user(&pool, org, "MEMBER", None).await;
        let branch_x = BranchId::new();
        let branch_y = BranchId::new();
        let state = collab_state(&pool);

        let (org_poll, _) = seed_poll(&pool, org, "ORG", None, author, "org poll").await;
        let (dept_x_poll, _) = seed_poll(
            &pool,
            org,
            "DEPARTMENT",
            Some(branch_x.as_uuid().to_string()),
            author,
            "dept x poll",
        )
        .await;
        let (dept_y_poll, _) = seed_poll(
            &pool,
            org,
            "DEPARTMENT",
            Some(branch_y.as_uuid().to_string()),
            author,
            "dept y poll",
        )
        .await;

        let member = principal_of(
            viewer,
            org,
            Role::Member,
            console_kernel_core::BranchScope::single(branch_x),
        );
        let listed = list_polls(
            State(state.clone()),
            Extension(member.clone()),
            Query(PollQuery {
                status: None,
                limit: None,
            }),
        )
        .await
        .expect("poll list");
        let listed_ids: std::collections::BTreeSet<Uuid> =
            listed.0.items.iter().map(|poll| poll.id).collect();
        assert!(listed_ids.contains(&org_poll), "org poll must stay listed");
        assert!(
            listed_ids.contains(&dept_x_poll),
            "own-department poll must be listed"
        );
        assert!(
            !listed_ids.contains(&dept_y_poll),
            "member of department X must NOT read department Y polls"
        );

        let poll_request = CreatePollRequest {
            target_scope_type: ScopeType::Org,
            target_scope_ref: None,
            title: "scope legality".to_owned(),
            question: "allowed?".to_owned(),
            status: PollStatus::Open,
            anonymity: PollAnonymity::Named,
            allow_multiple: false,
            closes_at: None,
            options: vec!["A".to_owned(), "B".to_owned()],
            object_type: None,
            object_id: None,
        };
        let denied = create_poll(State(state.clone()), Extension(member), Json(poll_request)).await;
        match denied {
            Ok(_) => panic!("bare-login member must not create an ORG-scoped poll"),
            Err(err) => assert_eq!(err.status, StatusCode::FORBIDDEN),
        }

        let admin = principal_of(
            author,
            org,
            Role::Admin,
            console_kernel_core::BranchScope::single(branch_x),
        );
        let _ = create_poll(
            State(state),
            Extension(admin),
            Json(CreatePollRequest {
                target_scope_type: ScopeType::Org,
                target_scope_ref: None,
                title: "admin poll".to_owned(),
                question: "allowed?".to_owned(),
                status: PollStatus::Open,
                anonymity: PollAnonymity::Named,
                allow_multiple: false,
                closes_at: None,
                options: vec!["A".to_owned(), "B".to_owned()],
                object_type: None,
                object_id: None,
            }),
        )
        .await
        .expect("admin creates org poll");
    }

    #[cfg(feature = "test-postgres")]
    #[sqlx::test(migrations = "../crates/platform/db/migrations")]
    async fn poll_vote_is_denied_outside_the_poll_audience(pool: sqlx::PgPool) {
        let org = seed_org(&pool).await;
        let author = seed_user(&pool, org, "ADMIN", None).await;
        let voter = seed_user(&pool, org, "MEMBER", None).await;
        let branch_x = BranchId::new();
        let branch_y = BranchId::new();
        let state = collab_state(&pool);

        let (dept_y_poll, dept_y_option) = seed_poll(
            &pool,
            org,
            "DEPARTMENT",
            Some(branch_y.as_uuid().to_string()),
            author,
            "dept y poll",
        )
        .await;
        let (org_poll, org_option) = seed_poll(&pool, org, "ORG", None, author, "org poll").await;

        let member = principal_of(
            voter,
            org,
            Role::Member,
            console_kernel_core::BranchScope::single(branch_x),
        );
        let denied = vote_poll(
            State(state.clone()),
            Extension(member.clone()),
            Path(dept_y_poll),
            Json(VotePollRequest {
                selected_option_ids: vec![dept_y_option],
            }),
        )
        .await;
        match denied {
            Ok(_) => panic!("member outside the department audience must not vote"),
            Err(err) => assert_eq!(
                err.status,
                StatusCode::NOT_FOUND,
                "out-of-audience polls must stay indistinguishable from missing ones"
            ),
        }

        let _ = vote_poll(
            State(state),
            Extension(member),
            Path(org_poll),
            Json(VotePollRequest {
                selected_option_ids: vec![org_option],
            }),
        )
        .await
        .expect("org poll stays votable by members");
    }
}
