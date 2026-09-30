//! Unified my-action-inbox read model (`GET /api/v1/me/action-inbox`).
//!
//! The HTTP surface is a composition root. Source visibility and persistence
//! predicates stay in their owning adapters; source-neutral cursor, urgency,
//! deterministic merge, total, and pagination semantics live in
//! `console-action-inbox-application`.

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Extension, Json, Router};
use console_action_inbox_application::{
    ActionInboxItem as ApplicationActionInboxItem, ActionInboxLink, ActionInboxSource,
    ActionInboxSourceFuture, ActionInboxSourceItem, ActionInboxSourcePage, ActionInboxSourcePort,
    ActionInboxSourceQuery, CompleteActionInboxError, ListActionInboxQuery,
    canonical_action_link_kind, list_action_inbox as run_action_inbox, list_complete_action_inbox,
};
use console_dispatch_adapter_postgres::PgDispatchStore;
use console_governance_adapter_postgres::PgGovernanceStore;
use console_kernel_core::{ErrorKind, KernelError};
use console_payroll_adapter_postgres::PgPayrollStore;
use console_platform_auth::JwtVerifier;
use console_platform_authz::{Action, Feature, Principal, authorize_org_wide};
use console_support_adapter_postgres::PgSupportStore;
use console_workorder_adapter_postgres::PgWorkOrderStore;
use console_workorder_application::{
    ActionInboxPosition as WorkOrderActionInboxPosition, ActionInboxWorkOrderPort,
};
use serde::Deserialize;
use sqlx::PgPool;
use time::OffsetDateTime;

use crate::workbench::{
    ActionInboxItem as WorkbenchActionInboxItem, ActionInboxPage as WorkbenchActionInboxPage,
    SourceFailure as WorkbenchSourceFailure, WorkbenchSourceRef, WorkbenchTarget, WorkbenchUrgency,
};
use crate::workflow_studio;

pub const ME_ACTION_INBOX_PATH: &str = "/api/v1/me/action-inbox";

#[derive(Clone)]
pub struct ActionInboxState {
    pool: PgPool,
    jwt_verifier: Option<JwtVerifier>,
}

impl ActionInboxState {
    #[must_use]
    pub fn new(pool: PgPool, jwt_verifier: Option<JwtVerifier>) -> Self {
        Self { pool, jwt_verifier }
    }
}

pub fn router(state: ActionInboxState) -> Router {
    let verifier = state.jwt_verifier.clone();
    let pool = state.pool.clone();
    let router = Router::new()
        .route(ME_ACTION_INBOX_PATH, get(list_action_inbox))
        .with_state(state);
    console_platform_request_context::with_request_context(router, verifier, pool)
}

#[derive(Debug, Default, Deserialize)]
struct ActionInboxQuery {
    limit: Option<usize>,
    cursor: Option<String>,
}

async fn list_action_inbox(
    State(state): State<ActionInboxState>,
    Extension(principal): Extension<Principal>,
    Query(query): Query<ActionInboxQuery>,
) -> Result<Json<console_action_inbox_application::ActionInboxPage>, InboxError> {
    let sources = PgActionInboxSources {
        pool: state.pool,
        principal,
    };
    let page = run_action_inbox(
        &sources,
        ListActionInboxQuery {
            limit: query.limit,
            cursor: query.cursor,
        },
        OffsetDateTime::now_utc(),
    )
    .await?;
    Ok(Json(page))
}

/// Reuses the native action-inbox application boundary for the aggregate
/// workbench. The workbench must not query source tables or reproduce source
/// authorization; it only narrows the already-authorized projection into its
/// frozen response contract.
pub(crate) async fn read_workbench_action_inbox(
    pool: &PgPool,
    principal: &Principal,
    as_of: OffsetDateTime,
    _limit: usize,
) -> Result<WorkbenchActionInboxPage, WorkbenchSourceFailure> {
    let sources = PgActionInboxSources {
        pool: pool.clone(),
        principal: principal.clone(),
    };
    let page = list_complete_action_inbox(&sources, as_of)
        .await
        .map_err(workbench_complete_source_error)?;
    let items = page
        .items
        .into_iter()
        .map(project_workbench_action)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(WorkbenchActionInboxPage {
        as_of,
        total: page.total,
        items,
    })
}

fn project_workbench_action(
    item: ApplicationActionInboxItem,
) -> Result<WorkbenchActionInboxItem, WorkbenchSourceFailure> {
    let source_id = item
        .id
        .rsplit_once(':')
        .and_then(|(_, value)| uuid::Uuid::parse_str(value).ok())
        .ok_or(WorkbenchSourceFailure::Unavailable {
            code: "action_inbox_unavailable",
        })?;
    let urgency = match item.urg {
        "now" => WorkbenchUrgency::Now,
        "today" => WorkbenchUrgency::Today,
        "wait" => WorkbenchUrgency::Wait,
        _ => {
            return Err(WorkbenchSourceFailure::Unavailable {
                code: "action_inbox_unavailable",
            });
        }
    };
    let (target_module, target_id) = item
        .links
        .first()
        .filter(|link| !link.kind.trim().is_empty() && !link.id.trim().is_empty())
        .map(|link| {
            (
                canonical_action_link_kind(&link.kind).to_owned(),
                link.id.clone(),
            )
        })
        .ok_or(WorkbenchSourceFailure::Unavailable {
            code: "action_inbox_unavailable",
        })?;
    if item.kind.trim().is_empty() || item.title.trim().is_empty() {
        return Err(WorkbenchSourceFailure::Unavailable {
            code: "action_inbox_unavailable",
        });
    }
    Ok(WorkbenchActionInboxItem {
        id: item.id,
        urgency,
        title: item.title,
        due_at: item.due,
        source: WorkbenchSourceRef {
            kind: item.kind,
            id: source_id,
        },
        target: WorkbenchTarget {
            module: target_module,
            id: target_id,
        },
    })
}

fn workbench_complete_source_error(error: CompleteActionInboxError) -> WorkbenchSourceFailure {
    match error {
        CompleteActionInboxError::Source(error) => workbench_source_error(error),
        CompleteActionInboxError::TotalInexact => WorkbenchSourceFailure::Unavailable {
            code: "action_inbox_total_inexact",
        },
        CompleteActionInboxError::BudgetExceeded => WorkbenchSourceFailure::Unavailable {
            code: "action_inbox_scan_budget_exceeded",
        },
        CompleteActionInboxError::TotalDrift => WorkbenchSourceFailure::Unavailable {
            code: "action_inbox_membership_changed",
        },
        CompleteActionInboxError::RepeatedCursor | CompleteActionInboxError::DuplicateId => {
            WorkbenchSourceFailure::Unavailable {
                code: "action_inbox_unavailable",
            }
        }
    }
}

fn workbench_source_error(error: KernelError) -> WorkbenchSourceFailure {
    if error.kind == ErrorKind::Forbidden {
        WorkbenchSourceFailure::Denied {
            code: "action_inbox_access_denied",
        }
    } else {
        WorkbenchSourceFailure::Unavailable {
            code: "action_inbox_unavailable",
        }
    }
}

/// Temporary composition-root adapter. This intentionally proves the inward
/// dependency seam only; it does not claim the source adapters have been split
/// into a dedicated infrastructure crate.
struct PgActionInboxSources {
    pool: PgPool,
    principal: Principal,
}

impl ActionInboxSourcePort for PgActionInboxSources {
    fn list_source_page(
        &self,
        source: ActionInboxSource,
        query: ActionInboxSourceQuery,
    ) -> ActionInboxSourceFuture<'_> {
        Box::pin(async move {
            match source {
                ActionInboxSource::Workflow => self.workflow_page(query).await,
                ActionInboxSource::Dispatch => self.dispatch_page(query).await,
                ActionInboxSource::Support => self.support_page(query).await,
                ActionInboxSource::WorkOrder => self.work_order_page(query).await,
                ActionInboxSource::Payroll => self.payroll_page(query).await,
                ActionInboxSource::Governance => self.governance_page(query).await,
            }
        })
    }
}

impl PgActionInboxSources {
    async fn workflow_page(
        &self,
        query: ActionInboxSourceQuery,
    ) -> Result<ActionInboxSourcePage, KernelError> {
        let after = query
            .after
            .as_ref()
            .map(|position| (position.created_at, position.id.clone()));
        let (tasks, has_more) = workflow_studio::my_action_inbox_tasks_page(
            &self.pool,
            &self.principal,
            query.as_of,
            after,
            query.limit,
        )
        .await
        .map_err(|error| source_failure("workflow", error))?;
        let (total, total_is_exact) =
            workflow_studio::my_action_inbox_task_count(&self.pool, &self.principal, query.as_of)
                .await
                .map_err(|error| source_failure("workflow", error))?;
        let items = tasks
            .into_iter()
            .map(|task| {
                let ref_code = task
                    .object_id
                    .map(|id| id.to_string())
                    .unwrap_or_else(|| task.run_id.to_string());
                let links = if let (Some(object_type), Some(object_id)) =
                    (task.object_type, task.object_id)
                {
                    vec![ActionInboxLink {
                        kind: canonical_action_link_kind(&object_type).to_owned(),
                        id: object_id.to_string(),
                        label: None,
                    }]
                } else {
                    vec![ActionInboxLink {
                        kind: "approval_run".to_owned(),
                        id: task.run_id.to_string(),
                        label: None,
                    }]
                };
                ActionInboxSourceItem {
                    id: format!("approval:{}", task.task_id),
                    kind: "approval".to_owned(),
                    ref_code,
                    title: task.title,
                    site: None,
                    who: None,
                    due: task.due_at,
                    submitted: None,
                    links,
                    created_at: task.created_at,
                }
            })
            .collect();
        Ok(ActionInboxSourcePage {
            items,
            total,
            total_is_exact,
            has_more,
        })
    }

    async fn dispatch_page(
        &self,
        query: ActionInboxSourceQuery,
    ) -> Result<ActionInboxSourcePage, KernelError> {
        let after = query
            .after
            .map(|position| (position.created_at, position.id));
        let source_limit = i64::try_from(query.limit).unwrap_or(200);
        let (offers, total, has_more) = PgDispatchStore::new(self.pool.clone())
            .list_my_pending_offers_action_page(
                self.principal.user_id,
                &self.principal.branch_scope,
                query.now,
                query.as_of,
                after,
                source_limit,
            )
            .await
            .map_err(|error| source_failure("dispatch", error))?;
        let items = offers
            .into_iter()
            .map(|offer| ActionInboxSourceItem {
                id: format!("dispatch:{}", offer.dispatch_id),
                kind: "dispatch".to_owned(),
                ref_code: offer.request_no.clone(),
                title: offer.request_no,
                site: None,
                who: Some("미배정".to_owned()),
                due: Some(offer.accept_window_ends_at),
                submitted: Some(offer.accept_window_started_at),
                links: vec![ActionInboxLink {
                    kind: "work_order".to_owned(),
                    id: offer.work_order_id.to_string(),
                    label: None,
                }],
                created_at: offer.created_at,
            })
            .collect();
        Ok(ActionInboxSourcePage {
            items,
            total: usize::try_from(total).unwrap_or(0),
            total_is_exact: true,
            has_more,
        })
    }

    async fn support_page(
        &self,
        query: ActionInboxSourceQuery,
    ) -> Result<ActionInboxSourcePage, KernelError> {
        let after = query
            .after
            .map(|position| (position.created_at, position.id));
        let source_limit = i64::try_from(query.limit).unwrap_or(200);
        let (tickets, total, has_more) = PgSupportStore::new(self.pool.clone())
            .list_assigned_action_inbox_page(
                self.principal.branch_scope.clone(),
                self.principal.user_id,
                query.as_of,
                after,
                source_limit,
            )
            .await
            .map_err(|error| source_failure("support", error))?;
        let items = tickets
            .into_iter()
            .map(|ticket| ActionInboxSourceItem {
                id: format!("support:{}", ticket.id),
                kind: "support".to_owned(),
                ref_code: ticket.id.to_string(),
                title: ticket.title,
                site: None,
                who: ticket.assignee_name,
                due: ticket.due_at,
                submitted: Some(ticket.created_at),
                links: vec![ActionInboxLink {
                    kind: "support_ticket".to_owned(),
                    id: ticket.id.to_string(),
                    label: None,
                }],
                created_at: ticket.created_at,
            })
            .collect();
        Ok(ActionInboxSourcePage {
            items,
            total: usize::try_from(total).unwrap_or(0),
            total_is_exact: true,
            has_more,
        })
    }

    async fn work_order_page(
        &self,
        query: ActionInboxSourceQuery,
    ) -> Result<ActionInboxSourcePage, KernelError> {
        let source_limit = i64::try_from(query.limit).unwrap_or(200);
        let page = PgWorkOrderStore::new(self.pool.clone())
            .list_assigned_action_inbox_page(
                self.principal.org_id,
                self.principal.branch_scope.clone(),
                self.principal.user_id,
                query.as_of,
                query.after.map(|position| WorkOrderActionInboxPosition {
                    created_at: position.created_at,
                    id: position.id,
                }),
                source_limit,
            )
            .await
            .map_err(|error| source_failure("workorder", error))?;
        let items = page
            .items
            .into_iter()
            .map(|row| ActionInboxSourceItem {
                id: format!("work:{}", row.id),
                kind: "work".to_owned(),
                ref_code: row.request_no.clone(),
                title: row.request_no,
                site: Some(row.site_name),
                who: None,
                due: row.target_due_at,
                submitted: Some(row.created_at),
                links: vec![ActionInboxLink {
                    kind: "work_order".to_owned(),
                    id: row.id.to_string(),
                    label: None,
                }],
                created_at: row.created_at,
            })
            .collect();
        Ok(ActionInboxSourcePage {
            items,
            total: usize::try_from(page.total).unwrap_or(0),
            total_is_exact: true,
            has_more: page.has_more,
        })
    }

    async fn payroll_page(
        &self,
        query: ActionInboxSourceQuery,
    ) -> Result<ActionInboxSourcePage, KernelError> {
        // Inbox is a merged read: callers without org-wide PayrollRunManage
        // (built-in EXECUTIVE/SUPER_ADMIN) get an empty source, not 403.
        if authorize_org_wide(&self.principal, Action::new(Feature::PayrollRunManage)).is_err() {
            return Ok(empty_source_page());
        }
        let after = query
            .after
            .map(|position| (position.created_at, position.id));
        let source_limit = i64::try_from(query.limit).unwrap_or(200);
        let (runs, total, has_more) = PgPayrollStore::new(self.pool.clone())
            .list_submitted_action_inbox_page(
                self.principal.user_id,
                query.as_of,
                after,
                source_limit,
            )
            .await
            .map_err(|error| source_failure("payroll", error))?;
        let items = runs
            .into_iter()
            .map(|run| ActionInboxSourceItem {
                id: format!("payroll:{}", run.id),
                kind: "payroll".to_owned(),
                ref_code: run.source_label.clone(),
                title: run.source_label,
                site: None,
                who: None,
                due: None,
                submitted: Some(run.submitted_at),
                links: vec![ActionInboxLink {
                    kind: "pay_run".to_owned(),
                    id: run.id.to_string(),
                    label: None,
                }],
                created_at: run.submitted_at,
            })
            .collect();
        Ok(ActionInboxSourcePage {
            items,
            total: usize::try_from(total).unwrap_or(0),
            total_is_exact: true,
            has_more,
        })
    }

    async fn governance_page(
        &self,
        query: ActionInboxSourceQuery,
    ) -> Result<ActionInboxSourcePage, KernelError> {
        // Match the decide REST gate so pending four-eyes do not leak to
        // callers who cannot act.
        if authorize_org_wide(&self.principal, Action::new(Feature::RoleManage)).is_err() {
            return Ok(empty_source_page());
        }
        let after = query
            .after
            .map(|position| (position.created_at, position.id));
        let source_limit = i64::try_from(query.limit).unwrap_or(200);
        let (requests, total, has_more) = PgGovernanceStore::new(self.pool.clone())
            .list_pending_action_inbox_page(
                self.principal.user_id,
                query.as_of,
                after,
                source_limit,
            )
            .await
            .map_err(|error| source_failure("governance", error))?;
        let items = requests
            .into_iter()
            .map(|request| ActionInboxSourceItem {
                id: format!("governance:{}", request.id),
                kind: "governance".to_owned(),
                ref_code: request.request_ref.to_string(),
                title: request.kind,
                site: None,
                who: None,
                due: None,
                submitted: Some(request.created_at),
                links: vec![ActionInboxLink {
                    kind: "approval_request".to_owned(),
                    id: request.request_ref.to_string(),
                    label: None,
                }],
                created_at: request.created_at,
            })
            .collect();
        Ok(ActionInboxSourcePage {
            items,
            total: usize::try_from(total).unwrap_or(0),
            total_is_exact: true,
            has_more,
        })
    }
}

fn empty_source_page() -> ActionInboxSourcePage {
    ActionInboxSourcePage {
        items: Vec::new(),
        total: 0,
        total_is_exact: true,
        has_more: false,
    }
}

fn source_failure(source: &'static str, error: impl std::fmt::Display) -> KernelError {
    tracing::error!(source, error = %error, "action-inbox source read failed");
    KernelError::internal("failed to load action inbox")
}

#[derive(Debug)]
struct InboxError {
    status: StatusCode,
    code: &'static str,
    message: String,
}

impl From<KernelError> for InboxError {
    fn from(error: KernelError) -> Self {
        let code = match error.kind {
            ErrorKind::Validation => "validation",
            ErrorKind::Internal => "internal",
            _ => "error",
        };
        let status = match error.kind {
            ErrorKind::Validation => StatusCode::UNPROCESSABLE_ENTITY,
            ErrorKind::Forbidden => StatusCode::FORBIDDEN,
            ErrorKind::NotFound => StatusCode::NOT_FOUND,
            ErrorKind::Conflict | ErrorKind::InvalidTransition => StatusCode::CONFLICT,
            ErrorKind::Internal => StatusCode::INTERNAL_SERVER_ERROR,
        };
        Self {
            status,
            code,
            message: error.message,
        }
    }
}

impl IntoResponse for InboxError {
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
    use console_kernel_core::KernelError;

    use super::InboxError;
    use console_action_inbox_application::canonical_action_link_kind;

    #[cfg(not(feature = "test-postgres"))]
    #[test]
    fn normalizes_the_legacy_workflow_run_alias() {
        assert_eq!(canonical_action_link_kind("workflow_run"), "approval_run");
        assert_eq!(canonical_action_link_kind("approval_run"), "approval_run");
        assert_eq!(canonical_action_link_kind("work_order"), "work_order");
    }

    #[cfg(not(feature = "test-postgres"))]
    #[test]
    fn preserves_cursor_validation_and_source_failure_error_codes() {
        assert_eq!(
            InboxError::from(KernelError::validation("bad cursor")).code,
            "validation"
        );
        assert_eq!(
            InboxError::from(KernelError::internal("source failed")).code,
            "internal"
        );
    }
}
