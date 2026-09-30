//! Principal-scoped workbench read composition.
//!
//! This module owns only the aggregate boundary for `GET /api/v1/me/workbench`.
//! It deliberately does not query durable tables or reproduce source authorization:
//! the serial application integrator supplies [`WorkbenchReaders`] adapters over the
//! action inbox, personal-todo, and collaboration-calendar owners.  Each adapter
//! must apply its native policy before returning a bounded page.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use axum::extract::State;
use axum::http::{StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Extension, Json, Router};
use console_kernel_core::{BranchId, BranchScope};
use console_platform_auth::JwtVerifier;
use console_platform_authz::Principal;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use time::{Duration, OffsetDateTime, Time, macros::offset};
use uuid::Uuid;

pub const ME_WORKBENCH_PATH: &str = "/api/v1/me/workbench";
pub const WORKBENCH_TIMEZONE: &str = "Asia/Seoul";
const DEFAULT_LIMIT: usize = 50;
const MAX_LIMIT: usize = 100;
const MAX_RANGE: Duration = Duration::days(31);
const SOURCE_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(750);

pub type SourceFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, SourceFailure>> + Send + 'a>>;
pub type ScopeFuture<'a> =
    Pin<Box<dyn Future<Output = Result<BranchScope, ScopeFailure>> + Send + 'a>>;

/// Source adapter seam. Implementations stay in the serial composition root so
/// this module cannot accidentally become a second policy or storage authority.
pub trait WorkbenchReaders: Send + Sync + 'static {
    /// Returns the source's independently authorized branch scope. This is used
    /// only to preflight an explicit branch selection; it must never widen the
    /// principal scope or substitute a data-read denial.
    fn action_scope(&self, principal: Principal) -> ScopeFuture<'_>;
    fn todo_scope(&self, principal: Principal) -> ScopeFuture<'_>;
    fn calendar_scope(&self, principal: Principal) -> ScopeFuture<'_>;

    fn action_inbox(&self, context: WorkbenchReadContext) -> SourceFuture<'_, ActionInboxPage>;
    fn todos(&self, context: WorkbenchReadContext) -> SourceFuture<'_, TodoPage>;
    fn calendar(&self, context: WorkbenchReadContext) -> SourceFuture<'_, CalendarPage>;
}

#[derive(Clone)]
pub struct WorkbenchState {
    readers: Arc<dyn WorkbenchReaders>,
    pool: PgPool,
    jwt_verifier: Option<JwtVerifier>,
}

impl WorkbenchState {
    #[must_use]
    pub fn new(
        readers: Arc<dyn WorkbenchReaders>,
        pool: PgPool,
        jwt_verifier: Option<JwtVerifier>,
    ) -> Self {
        Self {
            readers,
            pool,
            jwt_verifier,
        }
    }
}

/// Router is wrapped in the same verified request-context layer as the native
/// source routes, so a missing/invalid session is rejected before composition.
pub fn router(state: WorkbenchState) -> Router {
    let verifier = state.jwt_verifier.clone();
    let pool = state.pool.clone();
    let router = Router::new()
        .route(ME_WORKBENCH_PATH, get(get_my_workbench))
        .with_state(state);
    console_platform_request_context::with_request_context(router, verifier, pool)
}

async fn get_my_workbench(
    State(state): State<WorkbenchState>,
    Extension(principal): Extension<Principal>,
    uri: Uri,
) -> Result<Json<MyWorkbenchResponse>, WorkbenchError> {
    let request_now = OffsetDateTime::now_utc();
    let query = WorkbenchQuery::parse(uri.query(), request_now)?;
    let scope = EffectiveScope::from_query(&principal.branch_scope, query.branch_id)?;
    preflight_explicit_branch(&*state.readers, &principal, &scope).await?;
    let context = build_context(principal, query, scope, request_now);
    let response = compose(&*state.readers, context).await?;
    Ok(Json(response))
}

fn build_context(
    principal: Principal,
    query: WorkbenchQuery,
    scope: EffectiveScope,
    request_now: OffsetDateTime,
) -> WorkbenchReadContext {
    WorkbenchReadContext {
        principal,
        range: query.range,
        scope,
        limits: query.limits,
        as_of: request_now,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkbenchLimits {
    pub action: usize,
    pub todo: usize,
    pub calendar: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkbenchRange {
    pub from: OffsetDateTime,
    pub to: OffsetDateTime,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EffectiveScope {
    All {
        #[serde(skip_serializing_if = "Option::is_none")]
        selected_branch_id: Option<Uuid>,
    },
    Branches {
        branch_ids: Vec<Uuid>,
        #[serde(skip_serializing_if = "Option::is_none")]
        selected_branch_id: Option<Uuid>,
    },
}

impl EffectiveScope {
    fn from_query(scope: &BranchScope, selected: Option<BranchId>) -> Result<Self, WorkbenchError> {
        if let Some(branch_id) = selected {
            if !scope.allows(branch_id) {
                return Err(WorkbenchError::forbidden("branch_out_of_scope"));
            }
            return Ok(Self::Branches {
                branch_ids: vec![*branch_id.as_uuid()],
                selected_branch_id: Some(*branch_id.as_uuid()),
            });
        }
        Ok(match scope {
            BranchScope::All => Self::All {
                selected_branch_id: None,
            },
            BranchScope::Branches(branches) => Self::Branches {
                branch_ids: branches.iter().map(|branch| *branch.as_uuid()).collect(),
                selected_branch_id: None,
            },
        })
    }

    fn selected_branch(&self) -> Option<BranchId> {
        match self {
            Self::All { .. } => None,
            Self::Branches {
                selected_branch_id: Some(branch_id),
                ..
            } => Some(BranchId::from_uuid(*branch_id)),
            Self::Branches { .. } => None,
        }
    }

    pub(crate) fn as_branch_scope(&self) -> BranchScope {
        match self {
            Self::All { .. } => BranchScope::All,
            Self::Branches { branch_ids, .. } => BranchScope::Branches(
                branch_ids
                    .iter()
                    .copied()
                    .map(BranchId::from_uuid)
                    .collect(),
            ),
        }
    }
}

#[derive(Debug, Clone)]
pub struct WorkbenchReadContext {
    pub principal: Principal,
    pub range: WorkbenchRange,
    pub scope: EffectiveScope,
    pub limits: WorkbenchLimits,
    /// A single ceiling passed to every source. Adapters must exclude rows newer
    /// than this moment instead of returning a stale or future snapshot.
    pub as_of: OffsetDateTime,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceFailure {
    /// Ordinary source-read denial: the aggregate remains a partial 200 when
    /// another source succeeds. It is intentionally distinct from preflight
    /// explicit-branch scope denial.
    Denied {
        code: &'static str,
    },
    Unavailable {
        code: &'static str,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScopeFailure {
    // Matched in `preflight_explicit_branch` but never constructed: every
    // current `ScopeFuture` yields `Ok` or `Unavailable`, so a source-level
    // denial cannot presently be distinguished from a source outage. Kept
    // deliberately — the match arm is the intended 403 path.
    #[allow(dead_code)]
    Denied {
        code: &'static str,
    },
    Unavailable {
        code: &'static str,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ActionInboxItem {
    pub id: String,
    pub urgency: WorkbenchUrgency,
    pub title: String,
    #[serde(
        with = "time::serde::rfc3339::option",
        skip_serializing_if = "Option::is_none"
    )]
    pub due_at: Option<OffsetDateTime>,
    pub source: WorkbenchSourceRef,
    pub target: WorkbenchTarget,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TodoItem {
    pub id: Uuid,
    pub text: String,
    pub done: bool,
    /// Source order is assigned by the todo owner; only an equal order is
    /// resolved by stable id during aggregate serialization.
    pub source_order: u64,
    pub target: WorkbenchTarget,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CalendarItem {
    pub id: Uuid,
    pub title: String,
    #[serde(with = "time::serde::rfc3339")]
    pub starts_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub ends_at: OffsetDateTime,
    pub target: WorkbenchTarget,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WorkbenchUrgency {
    Now,
    Today,
    Wait,
}

impl WorkbenchUrgency {
    const fn rank(&self) -> u8 {
        match self {
            Self::Now => 0,
            Self::Today => 1,
            Self::Wait => 2,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkbenchSourceRef {
    pub kind: String,
    pub id: Uuid,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkbenchTarget {
    pub module: String,
    pub id: String,
}

#[derive(Debug, Clone)]
pub struct ActionInboxPage {
    pub as_of: OffsetDateTime,
    pub total: usize,
    pub items: Vec<ActionInboxItem>,
}

#[derive(Debug, Clone)]
pub struct TodoPage {
    pub as_of: OffsetDateTime,
    pub total: usize,
    pub items: Vec<TodoItem>,
}

#[derive(Debug, Clone)]
pub struct CalendarPage {
    pub as_of: OffsetDateTime,
    pub total: usize,
    pub items: Vec<CalendarItem>,
}

#[derive(Debug, Serialize)]
pub struct MyWorkbenchResponse {
    #[serde(with = "time::serde::rfc3339")]
    pub as_of: OffsetDateTime,
    pub timezone: &'static str,
    pub range: WorkbenchRangeResponse,
    pub scope: EffectiveScope,
    pub partial: bool,
    pub action_inbox: SourceEnvelope<ActionInboxItem>,
    pub todos: SourceEnvelope<TodoItem>,
    pub calendar: SourceEnvelope<CalendarItem>,
}

#[derive(Debug, Serialize)]
pub struct WorkbenchRangeResponse {
    #[serde(with = "time::serde::rfc3339")]
    pub from: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub to: OffsetDateTime,
}

#[derive(Debug, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum SourceEnvelope<T> {
    Ok {
        #[serde(with = "time::serde::rfc3339")]
        as_of: OffsetDateTime,
        items: Vec<T>,
        total: usize,
        truncated: bool,
    },
    Denied {
        code: &'static str,
    },
    Unavailable {
        code: &'static str,
    },
}

pub async fn compose(
    readers: &dyn WorkbenchReaders,
    context: WorkbenchReadContext,
) -> Result<MyWorkbenchResponse, WorkbenchError> {
    let (actions, todos, calendar) = futures::join!(
        bounded_source(readers.action_inbox(context.clone())),
        bounded_source(readers.todos(context.clone())),
        bounded_source(readers.calendar(context.clone()))
    );

    let mut usable_source = false;
    let action_inbox = action_envelope(
        actions,
        context.limits.action,
        context.as_of,
        &mut usable_source,
    );
    let todos = todo_envelope(
        todos,
        context.limits.todo,
        context.as_of,
        &mut usable_source,
    );
    let calendar = calendar_envelope(
        calendar,
        context.limits.calendar,
        context.as_of,
        &mut usable_source,
    );
    if !usable_source {
        return Err(WorkbenchError::unavailable("workbench_sources_unavailable"));
    }
    let partial = !matches!(action_inbox, SourceEnvelope::Ok { .. })
        || !matches!(todos, SourceEnvelope::Ok { .. })
        || !matches!(calendar, SourceEnvelope::Ok { .. });
    Ok(MyWorkbenchResponse {
        as_of: context.as_of,
        timezone: WORKBENCH_TIMEZONE,
        range: WorkbenchRangeResponse {
            from: context.range.from,
            to: context.range.to,
        },
        scope: context.scope,
        partial,
        action_inbox,
        todos,
        calendar,
    })
}

async fn bounded_source<T>(future: SourceFuture<'_, T>) -> Result<T, SourceFailure> {
    match tokio::time::timeout(SOURCE_TIMEOUT, future).await {
        Ok(result) => result,
        Err(_) => Err(SourceFailure::Unavailable {
            code: "source_timeout",
        }),
    }
}

async fn bounded_scope(future: ScopeFuture<'_>) -> Result<BranchScope, ScopeFailure> {
    match tokio::time::timeout(SOURCE_TIMEOUT, future).await {
        Ok(result) => result,
        Err(_) => Err(ScopeFailure::Unavailable {
            code: "source_scope_timeout",
        }),
    }
}

/// An explicit branch is legal only if it survives *every* source's native
/// scope. This is a request-scope authorization decision (403), not a source
/// data denial that may be represented by a partial response.
pub async fn preflight_explicit_branch(
    readers: &dyn WorkbenchReaders,
    principal: &Principal,
    scope: &EffectiveScope,
) -> Result<(), WorkbenchError> {
    let Some(selected) = scope.selected_branch() else {
        return Ok(());
    };
    let (action, todo, calendar) = futures::join!(
        bounded_scope(readers.action_scope(principal.clone())),
        bounded_scope(readers.todo_scope(principal.clone())),
        bounded_scope(readers.calendar_scope(principal.clone()))
    );
    for result in [action, todo, calendar] {
        match result {
            Ok(source_scope) if source_scope.allows(selected) => {}
            Ok(_) => return Err(WorkbenchError::forbidden("source_branch_out_of_scope")),
            Err(ScopeFailure::Denied { .. }) => {
                return Err(WorkbenchError::forbidden("source_scope_denied"));
            }
            Err(ScopeFailure::Unavailable { .. }) => {
                return Err(WorkbenchError::unavailable("source_scope_unavailable"));
            }
        }
    }
    Ok(())
}

fn action_envelope(
    result: Result<ActionInboxPage, SourceFailure>,
    limit: usize,
    ceiling: OffsetDateTime,
    usable: &mut bool,
) -> SourceEnvelope<ActionInboxItem> {
    match result {
        Ok(mut page) if page.as_of <= ceiling => {
            *usable = true;
            page.items.sort_by(|left, right| {
                left.urgency
                    .rank()
                    .cmp(&right.urgency.rank())
                    .then_with(|| due_sort(left.due_at, right.due_at))
                    .then_with(|| left.id.cmp(&right.id))
            });
            let truncated = page.items.len() > limit || page.total > limit;
            page.items.truncate(limit);
            SourceEnvelope::Ok {
                as_of: page.as_of,
                items: page.items,
                total: page.total,
                truncated,
            }
        }
        Ok(_) => SourceEnvelope::Unavailable {
            code: "source_snapshot_after_request",
        },
        Err(SourceFailure::Denied { code }) => SourceEnvelope::Denied { code },
        Err(SourceFailure::Unavailable { code }) => SourceEnvelope::Unavailable { code },
    }
}

fn todo_envelope(
    result: Result<TodoPage, SourceFailure>,
    limit: usize,
    ceiling: OffsetDateTime,
    usable: &mut bool,
) -> SourceEnvelope<TodoItem> {
    match result {
        Ok(mut page) if page.as_of <= ceiling => {
            *usable = true;
            page.items.sort_by(|left, right| {
                left.source_order
                    .cmp(&right.source_order)
                    .then_with(|| left.id.cmp(&right.id))
            });
            let truncated = page.items.len() > limit || page.total > limit;
            page.items.truncate(limit);
            SourceEnvelope::Ok {
                as_of: page.as_of,
                items: page.items,
                total: page.total,
                truncated,
            }
        }
        Ok(_) => SourceEnvelope::Unavailable {
            code: "source_snapshot_after_request",
        },
        Err(SourceFailure::Denied { code }) => SourceEnvelope::Denied { code },
        Err(SourceFailure::Unavailable { code }) => SourceEnvelope::Unavailable { code },
    }
}

fn calendar_envelope(
    result: Result<CalendarPage, SourceFailure>,
    limit: usize,
    ceiling: OffsetDateTime,
    usable: &mut bool,
) -> SourceEnvelope<CalendarItem> {
    match result {
        Ok(mut page) if page.as_of <= ceiling => {
            *usable = true;
            page.items.sort_by(|left, right| {
                left.starts_at
                    .cmp(&right.starts_at)
                    .then_with(|| left.id.cmp(&right.id))
            });
            let truncated = page.items.len() > limit || page.total > limit;
            page.items.truncate(limit);
            SourceEnvelope::Ok {
                as_of: page.as_of,
                items: page.items,
                total: page.total,
                truncated,
            }
        }
        Ok(_) => SourceEnvelope::Unavailable {
            code: "source_snapshot_after_request",
        },
        Err(SourceFailure::Denied { code }) => SourceEnvelope::Denied { code },
        Err(SourceFailure::Unavailable { code }) => SourceEnvelope::Unavailable { code },
    }
}

fn due_sort(left: Option<OffsetDateTime>, right: Option<OffsetDateTime>) -> std::cmp::Ordering {
    match (left, right) {
        (Some(left), Some(right)) => left.cmp(&right),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => std::cmp::Ordering::Equal,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct WorkbenchQuery {
    range: WorkbenchRange,
    branch_id: Option<BranchId>,
    limits: WorkbenchLimits,
}

impl WorkbenchQuery {
    fn parse(raw: Option<&str>, now: OffsetDateTime) -> Result<Self, WorkbenchError> {
        let mut from = None;
        let mut to = None;
        let mut branch_id = None;
        let mut action_limit = None;
        let mut todo_limit = None;
        let mut calendar_limit = None;
        for (key, value) in parse_pairs(raw)? {
            let slot = match key.as_str() {
                "from" => &mut from,
                "to" => &mut to,
                "branch_id" => &mut branch_id,
                "action_limit" => &mut action_limit,
                "todo_limit" => &mut todo_limit,
                "calendar_limit" => &mut calendar_limit,
                _ => return Err(WorkbenchError::validation("unknown_query_parameter")),
            };
            if slot.is_some() {
                return Err(WorkbenchError::validation("duplicate_query_parameter"));
            }
            match key.as_str() {
                "from" | "to" => {
                    *slot = Some(value);
                }
                "branch_id" => {
                    *slot = Some(value);
                }
                "action_limit" | "todo_limit" | "calendar_limit" => {
                    *slot = Some(value);
                }
                _ => unreachable!("unknown keys returned early"),
            }
        }
        let range = match (from, to) {
            (None, None) => default_kst_day(now),
            (Some(from), Some(to)) => WorkbenchRange {
                from: parse_instant(&from)?,
                to: parse_instant(&to)?,
            },
            _ => return Err(WorkbenchError::validation("range_requires_from_and_to")),
        };
        if range.to <= range.from {
            return Err(WorkbenchError::validation("range_must_be_positive"));
        }
        if range.to - range.from > MAX_RANGE {
            return Err(WorkbenchError::validation("range_exceeds_31_days"));
        }
        Ok(Self {
            range,
            branch_id: branch_id
                .map(|value| {
                    Uuid::parse_str(&value)
                        .map(BranchId::from_uuid)
                        .map_err(|_| WorkbenchError::validation("invalid_branch_id"))
                })
                .transpose()?,
            limits: WorkbenchLimits {
                action: parse_limit(action_limit, "invalid_action_limit")?,
                todo: parse_limit(todo_limit, "invalid_todo_limit")?,
                calendar: parse_limit(calendar_limit, "invalid_calendar_limit")?,
            },
        })
    }
}

fn parse_limit(value: Option<String>, code: &'static str) -> Result<usize, WorkbenchError> {
    match value {
        None => Ok(DEFAULT_LIMIT),
        Some(value) => value
            .parse::<usize>()
            .ok()
            .filter(|value| (1..=MAX_LIMIT).contains(value))
            .ok_or_else(|| WorkbenchError::validation(code)),
    }
}

fn parse_instant(value: &str) -> Result<OffsetDateTime, WorkbenchError> {
    OffsetDateTime::parse(value, &time::format_description::well_known::Rfc3339)
        .map_err(|_| WorkbenchError::validation("invalid_rfc3339_instant"))
}

fn default_kst_day(now: OffsetDateTime) -> WorkbenchRange {
    let local_start = now.to_offset(offset!(+9)).replace_time(Time::MIDNIGHT);
    WorkbenchRange {
        from: local_start,
        to: local_start + Duration::days(1),
    }
}

/// Minimal strict query decoder. Query keys are ASCII-only, and malformed
/// percent escapes fail validation rather than being silently normalized.
fn parse_pairs(raw: Option<&str>) -> Result<Vec<(String, String)>, WorkbenchError> {
    let Some(raw) = raw else {
        return Ok(Vec::new());
    };
    if raw.is_empty() {
        return Ok(Vec::new());
    }
    raw.split('&')
        .map(|pair| {
            let (key, value) = pair
                .split_once('=')
                .ok_or_else(|| WorkbenchError::validation("invalid_query_encoding"))?;
            Ok((percent_decode(key)?, percent_decode(value)?))
        })
        .collect()
}

fn percent_decode(value: &str) -> Result<String, WorkbenchError> {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'+' => decoded.push(b' '),
            b'%' if index + 2 < bytes.len() => {
                let high = hex(bytes[index + 1])?;
                let low = hex(bytes[index + 2])?;
                decoded.push((high << 4) | low);
                index += 2;
            }
            b'%' => return Err(WorkbenchError::validation("invalid_query_encoding")),
            byte => decoded.push(byte),
        }
        index += 1;
    }
    String::from_utf8(decoded).map_err(|_| WorkbenchError::validation("invalid_query_encoding"))
}

fn hex(value: u8) -> Result<u8, WorkbenchError> {
    match value {
        b'0'..=b'9' => Ok(value - b'0'),
        b'a'..=b'f' => Ok(value - b'a' + 10),
        b'A'..=b'F' => Ok(value - b'A' + 10),
        _ => Err(WorkbenchError::validation("invalid_query_encoding")),
    }
}

#[derive(Debug)]
pub struct WorkbenchError {
    status: StatusCode,
    code: &'static str,
    message: &'static str,
}

impl WorkbenchError {
    fn validation(code: &'static str) -> Self {
        Self {
            status: StatusCode::UNPROCESSABLE_ENTITY,
            code,
            message: "workbench request is invalid",
        }
    }

    fn forbidden(code: &'static str) -> Self {
        Self {
            status: StatusCode::FORBIDDEN,
            code,
            message: "workbench access is not permitted",
        }
    }

    fn unavailable(code: &'static str) -> Self {
        Self {
            status: StatusCode::SERVICE_UNAVAILABLE,
            code,
            message: "workbench is temporarily unavailable",
        }
    }
}

impl IntoResponse for WorkbenchError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(serde_json::json!({
                "error": { "code": self.code, "message": self.message }
            })),
        )
            .into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    use console_kernel_core::{OrgId, UserId};

    #[cfg(not(feature = "test-postgres"))]
    #[test]
    fn rejects_unknown_one_sided_malformed_and_overlong_queries() {
        let now = OffsetDateTime::UNIX_EPOCH;
        for raw in [
            "org_id=x",
            "from=2026-07-01T00:00:00Z",
            "from=bad&to=2026-07-02T00:00:00Z",
            "from=2026-07-01T00:00:00Z&to=2026-08-02T00:00:01Z",
            "action_limit=0",
            "todo_limit=101",
            "calendar_limit=x",
        ] {
            assert!(WorkbenchQuery::parse(Some(raw), now).is_err(), "{raw}");
        }
    }

    #[cfg(not(feature = "test-postgres"))]
    #[test]
    fn defaults_to_the_kst_calendar_day() {
        let now = OffsetDateTime::parse(
            "2026-07-01T16:30:00Z",
            &time::format_description::well_known::Rfc3339,
        )
        .unwrap();
        let query = WorkbenchQuery::parse(None, now).unwrap();
        assert_eq!(query.range.from.offset().whole_hours(), 9);
        assert_eq!(query.range.from.hour(), 0);
        assert_eq!(query.range.from.date().to_string(), "2026-07-02");
        assert_eq!(query.range.to - query.range.from, Duration::days(1));
    }

    #[cfg(not(feature = "test-postgres"))]
    #[test]
    fn one_request_instant_drives_both_kst_default_range_and_as_of_at_rollover() {
        let request_now = OffsetDateTime::parse(
            "2026-07-01T14:59:59Z",
            &time::format_description::well_known::Rfc3339,
        )
        .unwrap();
        let query = WorkbenchQuery::parse(None, request_now).unwrap();
        let scope = EffectiveScope::from_query(&BranchScope::All, None).unwrap();
        let context = build_context(
            Principal::new(
                UserId::new(),
                OrgId::knl(),
                BTreeSet::new(),
                BranchScope::All,
            ),
            query,
            scope,
            request_now,
        );
        assert_eq!(context.as_of, request_now);
        assert_eq!(context.range.from.offset().whole_hours(), 9);
        assert_eq!(context.range.from.date().to_string(), "2026-07-01");
        assert_eq!(context.range.from.hour(), 0);
        assert_eq!(context.range.to.date().to_string(), "2026-07-02");
        assert_eq!(context.range.to.hour(), 0);
    }

    #[cfg(not(feature = "test-postgres"))]
    #[test]
    fn explicit_branch_must_intersect_effective_scope() {
        let allowed = BranchId::new();
        let denied = BranchId::new();
        assert!(EffectiveScope::from_query(&BranchScope::single(allowed), Some(allowed)).is_ok());
        assert!(EffectiveScope::from_query(&BranchScope::single(allowed), Some(denied)).is_err());
    }

    #[cfg(not(feature = "test-postgres"))]
    #[tokio::test]
    async fn workbench_errors_match_the_standard_error_body_contract() {
        for (error, status, code, message) in [
            (
                WorkbenchError::validation("invalid_action_limit"),
                StatusCode::UNPROCESSABLE_ENTITY,
                "invalid_action_limit",
                "workbench request is invalid",
            ),
            (
                WorkbenchError::forbidden("branch_out_of_scope"),
                StatusCode::FORBIDDEN,
                "branch_out_of_scope",
                "workbench access is not permitted",
            ),
            (
                WorkbenchError::unavailable("workbench_sources_unavailable"),
                StatusCode::SERVICE_UNAVAILABLE,
                "workbench_sources_unavailable",
                "workbench is temporarily unavailable",
            ),
        ] {
            let response = error.into_response();
            assert_eq!(response.status(), status);
            let body = axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap();
            assert_eq!(
                serde_json::from_slice::<serde_json::Value>(&body).unwrap(),
                serde_json::json!({ "error": { "code": code, "message": message } })
            );
        }
    }
}
