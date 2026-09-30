//! Read-only workflow-run context for the first executable object/workflow slice.
//!
//! This module deliberately supports only the two subjects with an established
//! source visibility rule: `work_order` and `support_ticket`.  It is not an
//! ontology registry, does not infer graph edges, and has no mutation route.
//! The serial composition owner mounts [`router`] after adding this module to
//! `lib.rs`; keeping that mount separate avoids concurrent edits to the shared
//! composition root.

use axum::extract::State;
use axum::http::{StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Extension, Json, Router};
use console_kernel_core::{BranchId, BranchScope, ErrorKind, KernelError};
use console_platform_auth::JwtVerifier;
use console_platform_authz::{
    Action, Feature, PermissionLevel, Principal, authorize, authorize_org_wide, permission_for,
};
use console_platform_db::{DbError, with_org_conn};
use serde::Serialize;
use serde_json::json;
use sqlx::{PgPool, Postgres, Row, Transaction};
use time::OffsetDateTime;
use url::form_urlencoded;
use uuid::Uuid;

/// Frozen Wave-1 route.  It must be mounted as an exact route rather than
/// beneath the existing `{run_id}` route so `for-object` is never parsed as an
/// identifier.
pub const WORKFLOW_RUNS_FOR_OBJECT_PATH: &str = "/api/v1/workflow-runs/for-object";

const DEFAULT_LIMIT: i64 = 25;
const MAX_LIMIT: i64 = 100;

/// Router state is intentionally narrow: this bridge needs only an RLS pool
/// and the normal request-context verifier.  It does not depend on a workflow
/// command store and therefore cannot create a run/task/audit/outbox row.
#[derive(Clone)]
pub struct WorkflowObjectContextState {
    pool: PgPool,
    jwt_verifier: Option<JwtVerifier>,
}

impl WorkflowObjectContextState {
    #[must_use]
    pub fn new(pool: PgPool, jwt_verifier: Option<JwtVerifier>) -> Self {
        Self { pool, jwt_verifier }
    }
}

/// Integration hook for the serial composition owner.
///
/// Mount the returned router unchanged.  `with_request_context` supplies the
/// authenticated [`Principal`] extension and arms no database state itself;
/// every database read below is still wrapped by `with_org_conn`.
pub fn router(state: WorkflowObjectContextState) -> Router {
    let verifier = state.jwt_verifier.clone();
    let pool = state.pool.clone();
    let router = Router::new()
        .route(
            WORKFLOW_RUNS_FOR_OBJECT_PATH,
            get(list_workflow_runs_for_object),
        )
        .with_state(state);
    console_platform_request_context::with_request_context(router, verifier, pool)
}

/// The only subjects this bounded bridge accepts.  Keeping this type closed is
/// the guard against accidentally turning a future table name into a generic
/// object resolver.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowObjectKind {
    WorkOrder,
    SupportTicket,
}

impl WorkflowObjectKind {
    fn parse(value: &str) -> Result<Self, WorkflowObjectContextError> {
        match value {
            "work_order" => Ok(Self::WorkOrder),
            "support_ticket" => Ok(Self::SupportTicket),
            _ => Err(WorkflowObjectContextError::validation(
                "object_type must be work_order or support_ticket",
            )),
        }
    }

    const fn as_str(self) -> &'static str {
        match self {
            Self::WorkOrder => "work_order",
            Self::SupportTicket => "support_ticket",
        }
    }
}

/// Closed response subject.  No source head, history, lifecycle, or actions
/// are synthesized by this bridge.
#[derive(Debug, Serialize)]
pub struct WorkflowObjectSubject {
    pub object_type: WorkflowObjectKind,
    pub object_id: Uuid,
}

/// Server-issued detail discriminator.  The client must still apply its own
/// current-route policy; this target carries no arbitrary URL and no command.
#[derive(Debug, Serialize)]
pub struct WorkflowRunDetailTarget {
    pub kind: &'static str,
    pub run_id: Uuid,
}

/// Authoritative workflow-run fields available in the runtime table.  Fields
/// intentionally absent include object history, action eligibility, audit
/// correlation, and native-effect/simulation results.
#[derive(Debug, Serialize)]
pub struct WorkflowRunForObjectSummary {
    pub run_id: Uuid,
    pub definition_id: Uuid,
    pub definition_version: i32,
    pub status: String,
    pub trigger_type: String,
    pub object_type: WorkflowObjectKind,
    pub object_id: Uuid,
    #[serde(with = "time::serde::rfc3339")]
    pub started_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
    #[serde(
        with = "time::serde::rfc3339::option",
        skip_serializing_if = "Option::is_none"
    )]
    pub completed_at: Option<OffsetDateTime>,
    pub detail_target: WorkflowRunDetailTarget,
}

/// Frozen page response.  `next_before` is an opaque UUID keyset cursor bound
/// server-side to organization, caller, and the exact subject pair.
#[derive(Debug, Serialize)]
pub struct WorkflowRunsForObjectResponse {
    pub subject: WorkflowObjectSubject,
    #[serde(with = "time::serde::rfc3339")]
    pub as_of: OffsetDateTime,
    pub items: Vec<WorkflowRunForObjectSummary>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_before: Option<Uuid>,
}

#[derive(Debug)]
struct ParsedQuery {
    object_type: WorkflowObjectKind,
    object_id: Uuid,
    limit: i64,
    before: Option<Uuid>,
}

/// Parses the URI directly so malformed UUIDs, limits, duplicate keys, and
/// unknown query parameters all receive the frozen `422` validation envelope
/// instead of extractor-specific `400` responses.
fn parse_query(uri: &Uri) -> Result<ParsedQuery, WorkflowObjectContextError> {
    let mut object_type = None;
    let mut object_id = None;
    let mut limit = None;
    let mut before = None;

    for (key, value) in form_urlencoded::parse(uri.query().unwrap_or_default().as_bytes()) {
        let slot = match key.as_ref() {
            "object_type" => &mut object_type,
            "object_id" => &mut object_id,
            "limit" => &mut limit,
            "before" => &mut before,
            _ => {
                return Err(WorkflowObjectContextError::validation(
                    "unknown query parameter",
                ));
            }
        };
        if slot.is_some() {
            return Err(WorkflowObjectContextError::validation(
                "query parameters may not be repeated",
            ));
        }
        *slot = Some(value.into_owned());
    }

    let object_type = object_type
        .as_deref()
        .ok_or_else(|| WorkflowObjectContextError::validation("object_type is required"))
        .and_then(WorkflowObjectKind::parse)?;
    let object_id = object_id
        .as_deref()
        .ok_or_else(|| WorkflowObjectContextError::validation("object_id is required"))
        .and_then(parse_uuid("object_id"))?;
    let limit = match limit.as_deref() {
        None => DEFAULT_LIMIT,
        Some(raw) => raw.parse::<i64>().map_err(|_| {
            WorkflowObjectContextError::validation("limit must be an integer between 1 and 100")
        })?,
    };
    if !(1..=MAX_LIMIT).contains(&limit) {
        return Err(WorkflowObjectContextError::validation(
            "limit must be between 1 and 100",
        ));
    }
    let before = before.as_deref().map(parse_uuid("before")).transpose()?;

    Ok(ParsedQuery {
        object_type,
        object_id,
        limit,
        before,
    })
}

fn parse_uuid(
    field: &'static str,
) -> impl FnOnce(&str) -> Result<Uuid, WorkflowObjectContextError> {
    move |value| {
        Uuid::parse_str(value.trim())
            .map_err(|_| WorkflowObjectContextError::validation(format!("{field} must be a UUID")))
    }
}

async fn list_workflow_runs_for_object(
    State(state): State<WorkflowObjectContextState>,
    Extension(principal): Extension<Principal>,
    uri: Uri,
) -> Result<Json<WorkflowRunsForObjectResponse>, WorkflowObjectContextError> {
    let query = parse_query(&uri)?;
    let as_of = OffsetDateTime::now_utc();
    let org = principal.org_id;
    let caller = *principal.user_id.as_uuid();
    let is_workflow_admin =
        authorize_org_wide(&principal, Action::new(Feature::RoleManage)).is_ok();

    let response = with_org_conn::<_, _, WorkflowObjectContextError>(&state.pool, org, move |tx| {
        Box::pin(async move {
            // The subject check is intentionally first.  Missing, cross-org,
            // cross-branch, or feature-invisible subjects all become the same
            // 404 before a workflow row/cursor is considered.
            let subject_branch = resolve_subject_branch(tx, query.object_type, query.object_id)
                .await?
                .filter(|branch| subject_authorized(&principal, query.object_type, *branch));
            let Some(subject_branch) = subject_branch else {
                return Err(WorkflowObjectContextError::not_found(
                    "workflow subject not found",
                ));
            };
            // Routed workflow authority is evaluated against the actual subject
            // branch read above, never an arbitrary representative branch. A
            // branch-less support ticket has no concrete branch authority, so
            // only the initiator or an org-wide workflow administrator can see
            // its associated runs (fail closed for role-routed visibility).
            let held_role_keys = subject_branch.map_or_else(Vec::new, |branch| {
                crate::workflow_studio::held_authority_role_keys(&principal, org, Some(branch))
            });

            let cursor = match query.before {
                None => None,
                Some(before) => Some(
                    load_visible_cursor(
                        tx,
                        before,
                        query.object_type,
                        query.object_id,
                        caller,
                        &held_role_keys,
                        is_workflow_admin,
                    )
                    .await?
                    .ok_or_else(WorkflowObjectContextError::invalid_cursor)?,
                ),
            };

            let rows = list_visible_runs(
                tx,
                query.object_type,
                query.object_id,
                caller,
                &held_role_keys,
                is_workflow_admin,
                cursor,
                query.limit,
            )
            .await?;
            let next_before = (rows.len() as i64 == query.limit)
                .then(|| rows.last().map(|item| item.run_id))
                .flatten();

            Ok(WorkflowRunsForObjectResponse {
                subject: WorkflowObjectSubject {
                    object_type: query.object_type,
                    object_id: query.object_id,
                },
                as_of,
                items: rows,
                next_before,
            })
        })
    })
    .await?;

    Ok(Json(response))
}

/// Loads the source row's actual branch inside the armed tenant transaction.
/// `Some(None)` is a branch-less support ticket; the caller must have all
/// branch scope to see it, and it cannot be used to mint routed authority.
async fn resolve_subject_branch(
    tx: &mut Transaction<'_, Postgres>,
    object_type: WorkflowObjectKind,
    object_id: Uuid,
) -> Result<Option<Option<BranchId>>, WorkflowObjectContextError> {
    let branch_id: Option<Option<Uuid>> = match object_type {
        WorkflowObjectKind::WorkOrder => {
            sqlx::query_scalar("SELECT branch_id FROM work_orders WHERE id = $1")
                .bind(object_id)
                .fetch_optional(tx.as_mut())
                .await?
        }
        WorkflowObjectKind::SupportTicket => {
            sqlx::query_scalar("SELECT branch_id FROM support_tickets WHERE id = $1")
                .bind(object_id)
                .fetch_optional(tx.as_mut())
                .await?
        }
    };
    Ok(branch_id.map(|branch| branch.map(BranchId::from_uuid)))
}

/// Subject authorization is computed after loading the actual source branch.
/// `authorize` intersects the principal's live branch scope with any custom
/// grant's branch scope, so an A-only grant cannot read a B subject.
fn subject_authorized(
    principal: &Principal,
    object_type: WorkflowObjectKind,
    branch: Option<BranchId>,
) -> bool {
    let feature = match object_type {
        WorkflowObjectKind::WorkOrder => Feature::WorkOrderReadAll,
        WorkflowObjectKind::SupportTicket => Feature::Login,
    };
    match branch {
        Some(branch) => authorize(principal, Action::new(feature), branch).is_ok(),
        // A nullable support-ticket branch is tenant-wide. It is deliberately
        // unavailable to a branch-scoped caller, even if a Login grant itself
        // names an individual branch.
        None => {
            principal.branch_scope == BranchScope::All
                && (principal
                    .roles
                    .iter()
                    .any(|role| permission_for(*role, feature) == PermissionLevel::Allow)
                    || principal.effective_feature_grants.iter().any(|grant| {
                        grant.feature == feature
                            && grant.permission == PermissionLevel::Allow
                            && grant.branch_scope == BranchScope::All
                    }))
        }
    }
}

async fn load_visible_cursor(
    tx: &mut Transaction<'_, Postgres>,
    before: Uuid,
    object_type: WorkflowObjectKind,
    object_id: Uuid,
    caller: Uuid,
    held_role_keys: &[String],
    is_workflow_admin: bool,
) -> Result<Option<(OffsetDateTime, Uuid)>, WorkflowObjectContextError> {
    let row = sqlx::query(
        r#"
        SELECT r.updated_at, r.id
        FROM workflow_runs r
        WHERE r.id = $1
          AND r.object_type = $2
          AND r.object_id = $3
          AND (
                $4
                OR r.initiated_by = $5
                OR EXISTS (
                    SELECT 1
                    FROM workflow_waiting_tasks t
                    WHERE t.run_id = r.id AND t.org_id = r.org_id
                      AND t.status IN ('OPEN', 'CLAIMED')
                      AND (t.claimed_by = $5 OR t.assignee_role_key = ANY($6))
                )
          )
        "#,
    )
    .bind(before)
    .bind(object_type.as_str())
    .bind(object_id)
    .bind(is_workflow_admin)
    .bind(caller)
    .bind(held_role_keys)
    .fetch_optional(tx.as_mut())
    .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    let updated_at = row.try_get("updated_at")?;
    let id = row.try_get("id")?;
    Ok(Some((updated_at, id)))
}

#[allow(clippy::too_many_arguments)]
async fn list_visible_runs(
    tx: &mut Transaction<'_, Postgres>,
    object_type: WorkflowObjectKind,
    object_id: Uuid,
    caller: Uuid,
    held_role_keys: &[String],
    is_workflow_admin: bool,
    before: Option<(OffsetDateTime, Uuid)>,
    limit: i64,
) -> Result<Vec<WorkflowRunForObjectSummary>, WorkflowObjectContextError> {
    let (before_updated_at, before_id) =
        before.map_or((None, None), |(at, id)| (Some(at), Some(id)));
    let rows = sqlx::query(
        r#"
        SELECT r.id AS run_id, r.definition_id, r.definition_version, r.status,
               r.trigger_type, r.started_at, r.updated_at, r.completed_at
        FROM workflow_runs r
        WHERE r.object_type = $1
          AND r.object_id = $2
          AND (
                $3
                OR r.initiated_by = $4
                OR EXISTS (
                    SELECT 1
                    FROM workflow_waiting_tasks t
                    WHERE t.run_id = r.id AND t.org_id = r.org_id
                      AND t.status IN ('OPEN', 'CLAIMED')
                      AND (t.claimed_by = $4 OR t.assignee_role_key = ANY($5))
                )
          )
          AND (
                $6::timestamptz IS NULL
                OR (r.updated_at, r.id) < ($6, $7)
          )
        ORDER BY r.updated_at DESC, r.id DESC
        LIMIT $8
        "#,
    )
    .bind(object_type.as_str())
    .bind(object_id)
    .bind(is_workflow_admin)
    .bind(caller)
    .bind(held_role_keys)
    .bind(before_updated_at)
    .bind(before_id)
    .bind(limit)
    .fetch_all(tx.as_mut())
    .await?;

    rows.into_iter()
        .map(|row| {
            let run_id: Uuid = row.try_get("run_id")?;
            Ok(WorkflowRunForObjectSummary {
                run_id,
                definition_id: row.try_get("definition_id")?,
                definition_version: row.try_get("definition_version")?,
                status: row.try_get("status")?,
                trigger_type: row.try_get("trigger_type")?,
                object_type,
                object_id,
                started_at: row.try_get("started_at")?,
                updated_at: row.try_get("updated_at")?,
                completed_at: row.try_get("completed_at")?,
                detail_target: WorkflowRunDetailTarget {
                    kind: "workflow_run_detail",
                    run_id,
                },
            })
        })
        .collect::<Result<Vec<_>, sqlx::Error>>()
        .map_err(WorkflowObjectContextError::from)
}

#[derive(Debug)]
struct WorkflowObjectContextError {
    status: StatusCode,
    code: &'static str,
    message: String,
}

impl WorkflowObjectContextError {
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
            code: match error.kind {
                ErrorKind::Validation => "validation",
                ErrorKind::NotFound => "not_found",
                ErrorKind::Forbidden => "forbidden",
                ErrorKind::Conflict => "conflict",
                ErrorKind::InvalidTransition => "invalid_transition",
                ErrorKind::Internal => "internal",
            },
            message: error.message,
        }
    }

    fn validation(message: impl Into<String>) -> Self {
        Self::from_kernel(KernelError::validation(message.into()))
    }

    fn not_found(message: impl Into<String>) -> Self {
        Self::from_kernel(KernelError::not_found(message.into()))
    }

    fn invalid_cursor() -> Self {
        Self {
            status: StatusCode::UNPROCESSABLE_ENTITY,
            code: "invalid_cursor",
            message: "before must identify a visible workflow run for this subject".to_owned(),
        }
    }

    fn internal(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            code: "internal",
            message: message.into(),
        }
    }
}

impl From<DbError> for WorkflowObjectContextError {
    fn from(error: DbError) -> Self {
        tracing::error!(error = %error, "workflow object context database operation failed");
        Self::internal("workflow object context request failed")
    }
}

impl From<sqlx::Error> for WorkflowObjectContextError {
    fn from(error: sqlx::Error) -> Self {
        Self::from(DbError::Sqlx(error))
    }
}

impl IntoResponse for WorkflowObjectContextError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(json!({ "error": { "code": self.code, "message": self.message } })),
        )
            .into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    use console_kernel_core::{OrgId, UserId};
    use console_platform_authz::{EffectiveFeatureGrant, Role};

    #[cfg(not(feature = "test-postgres"))]
    #[test]
    fn query_is_closed_and_bounded() {
        let uri: Uri = "/api/v1/workflow-runs/for-object?object_type=work_order&object_id=00000000-0000-0000-0000-000000000001&limit=100"
            .parse()
            .unwrap();
        let parsed = parse_query(&uri).unwrap();
        assert_eq!(parsed.object_type, WorkflowObjectKind::WorkOrder);
        assert_eq!(parsed.limit, 100);

        let unknown: Uri = "/api/v1/workflow-runs/for-object?object_type=work_order&object_id=00000000-0000-0000-0000-000000000001&include=history"
            .parse()
            .unwrap();
        assert_eq!(
            parse_query(&unknown).unwrap_err().status,
            StatusCode::UNPROCESSABLE_ENTITY
        );

        let unsupported: Uri = "/api/v1/workflow-runs/for-object?object_type=dispatch&object_id=00000000-0000-0000-0000-000000000001"
            .parse()
            .unwrap();
        assert_eq!(
            parse_query(&unsupported).unwrap_err().status,
            StatusCode::UNPROCESSABLE_ENTITY
        );
    }

    #[cfg(not(feature = "test-postgres"))]
    #[test]
    fn scoped_custom_grant_cannot_cross_from_branch_a_to_b() {
        let org = OrgId::new();
        let branch_a = BranchId::new();
        let branch_b = BranchId::new();
        let principal = Principal::new(
            UserId::new(),
            org,
            BTreeSet::from([Role::Member]),
            BranchScope::single(branch_a),
        )
        .with_effective_feature_grants(vec![EffectiveFeatureGrant::new(
            Feature::WorkOrderReadAll,
            PermissionLevel::Allow,
            BranchScope::single(branch_a),
        )]);

        assert!(subject_authorized(
            &principal,
            WorkflowObjectKind::WorkOrder,
            Some(branch_a)
        ));
        assert!(
            !subject_authorized(&principal, WorkflowObjectKind::WorkOrder, Some(branch_b)),
            "an A-scoped custom grant must never authorize a B subject"
        );
    }

    #[cfg(not(feature = "test-postgres"))]
    #[test]
    fn routed_authority_is_resolved_against_the_subject_branch() {
        let org = OrgId::new();
        let branch_a = BranchId::new();
        let branch_b = BranchId::new();
        let principal = Principal::new(
            UserId::new(),
            org,
            BTreeSet::from([Role::Admin]),
            BranchScope::single(branch_a),
        );

        assert!(
            !crate::workflow_studio::held_authority_role_keys(&principal, org, Some(branch_a))
                .is_empty(),
            "ADMIN can hold a routed review authority in its actual branch"
        );
        assert!(
            crate::workflow_studio::held_authority_role_keys(&principal, org, Some(branch_b))
                .is_empty(),
            "no arbitrary representative branch may turn A authority into B authority"
        );
    }

    #[cfg(not(feature = "test-postgres"))]
    #[test]
    fn nullable_ticket_is_never_visible_to_a_branch_scoped_principal() {
        let org = OrgId::new();
        let branch = BranchId::new();
        let scoped = Principal::new(
            UserId::new(),
            org,
            BTreeSet::from([Role::Admin]),
            BranchScope::single(branch),
        );
        let org_scoped = Principal::new(
            UserId::new(),
            org,
            BTreeSet::from([Role::Admin]),
            BranchScope::All,
        );

        assert!(!subject_authorized(
            &scoped,
            WorkflowObjectKind::SupportTicket,
            None
        ));
        assert!(subject_authorized(
            &org_scoped,
            WorkflowObjectKind::SupportTicket,
            None
        ));
    }
}
