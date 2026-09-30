//! Composition-root adapters from the workbench aggregate to native owners.
//!
//! This module contains no persistence or authorization policy of its own. It
//! narrows the verified principal to the aggregate's effective branch scope,
//! then delegates to the action-inbox, todo, and collaboration-calendar owners.

use console_kernel_core::{BranchScope, ErrorKind};
use console_platform_authz::Principal;
use console_todos_adapter_postgres::PgTodoStore;
use sqlx::PgPool;

use crate::workbench::{
    ActionInboxPage, CalendarPage, ScopeFuture, SourceFailure, SourceFuture, TodoItem, TodoPage,
    WorkbenchReadContext, WorkbenchReaders, WorkbenchTarget,
};

#[derive(Clone)]
pub(crate) struct NativeWorkbenchReaders {
    pool: PgPool,
}

impl NativeWorkbenchReaders {
    pub(crate) fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

impl WorkbenchReaders for NativeWorkbenchReaders {
    fn action_scope(&self, principal: Principal) -> ScopeFuture<'_> {
        Box::pin(std::future::ready(Ok(principal.branch_scope)))
    }

    fn todo_scope(&self, _: Principal) -> ScopeFuture<'_> {
        // Todos are owner-scoped and have no branch dimension. `All` here is
        // the identity for explicit-branch preflight, not a data-read grant.
        Box::pin(std::future::ready(Ok(BranchScope::All)))
    }

    fn calendar_scope(&self, _: Principal) -> ScopeFuture<'_> {
        // Collaboration events are organization/audience scoped and have no
        // branch column. The native calendar read still performs membership and
        // personal-audience authorization before returning any row.
        Box::pin(std::future::ready(Ok(BranchScope::All)))
    }

    fn action_inbox(&self, context: WorkbenchReadContext) -> SourceFuture<'_, ActionInboxPage> {
        let pool = self.pool.clone();
        Box::pin(async move {
            let mut principal = context.principal;
            principal.branch_scope = context.scope.as_branch_scope();
            crate::action_inbox::read_workbench_action_inbox(
                &pool,
                &principal,
                context.as_of,
                context.limits.action,
            )
            .await
        })
    }

    fn todos(&self, context: WorkbenchReadContext) -> SourceFuture<'_, TodoPage> {
        let pool = self.pool.clone();
        Box::pin(async move {
            let owner = context.principal.user_id;
            let limit =
                i64::try_from(context.limits.todo).map_err(|_| SourceFailure::Unavailable {
                    code: "todo_limit_invalid",
                })?;
            let snapshot = PgTodoStore::new(pool)
                .list_snapshot(owner, true, limit, context.as_of)
                .await
                .map_err(|error| match error.kind() {
                    ErrorKind::Forbidden => SourceFailure::Denied {
                        code: "todo_access_denied",
                    },
                    _ => SourceFailure::Unavailable {
                        code: "todo_unavailable",
                    },
                })?;
            let items = snapshot
                .items
                .into_iter()
                .enumerate()
                .map(|(source_order, item)| {
                    let id = *item.id.as_uuid();
                    Ok(TodoItem {
                        id,
                        text: item.text,
                        done: item.done,
                        source_order: u64::try_from(source_order).map_err(|_| {
                            SourceFailure::Unavailable {
                                code: "todo_order_invalid",
                            }
                        })?,
                        target: WorkbenchTarget {
                            module: "mywork".to_owned(),
                            id: id.to_string(),
                        },
                    })
                })
                .collect::<Result<Vec<_>, SourceFailure>>()?;
            Ok(TodoPage {
                as_of: snapshot.as_of,
                total: snapshot.total,
                items,
            })
        })
    }

    fn calendar(&self, context: WorkbenchReadContext) -> SourceFuture<'_, CalendarPage> {
        let pool = self.pool.clone();
        Box::pin(async move {
            crate::collaboration::read_workbench_calendar(
                &pool,
                &context.principal,
                context.range,
                context.limits.calendar,
                context.as_of,
            )
            .await
        })
    }
}
