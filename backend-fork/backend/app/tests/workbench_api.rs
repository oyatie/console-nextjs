#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
//! Contract tests for the workbench composition seam.

// Source-included rather than imported (the module is private to `console-app`),
// so items this contract test does not exercise read as dead here.
#[path = "../src/workbench.rs"]
#[allow(dead_code)]
mod workbench;

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

use axum::response::IntoResponse;
use console_kernel_core::{BranchId, BranchScope, OrgId, UserId};
use console_platform_authz::Principal;
use time::OffsetDateTime;
use uuid::Uuid;
use workbench::{
    ActionInboxItem, ActionInboxPage, CalendarItem, CalendarPage, EffectiveScope, ScopeFailure,
    ScopeFuture, SourceEnvelope, SourceFailure, SourceFuture, TodoItem, TodoPage, WorkbenchLimits,
    WorkbenchRange, WorkbenchReadContext, WorkbenchReaders, WorkbenchSourceRef, WorkbenchTarget,
    WorkbenchUrgency, compose, preflight_explicit_branch,
};

#[derive(Clone)]
struct Readers {
    action: Result<ActionInboxPage, SourceFailure>,
    todo: Result<TodoPage, SourceFailure>,
    calendar: Result<CalendarPage, SourceFailure>,
    action_scope: Result<BranchScope, ScopeFailure>,
    todo_scope: Result<BranchScope, ScopeFailure>,
    calendar_scope: Result<BranchScope, ScopeFailure>,
    pending_action: bool,
    contexts: Arc<Mutex<Vec<WorkbenchReadContext>>>,
}

impl Readers {
    fn record(&self, context: WorkbenchReadContext) {
        self.contexts.lock().unwrap().push(context);
    }
}

impl WorkbenchReaders for Readers {
    fn action_scope(&self, _: Principal) -> ScopeFuture<'_> {
        Box::pin(std::future::ready(self.action_scope.clone()))
    }

    fn todo_scope(&self, _: Principal) -> ScopeFuture<'_> {
        Box::pin(std::future::ready(self.todo_scope.clone()))
    }

    fn calendar_scope(&self, _: Principal) -> ScopeFuture<'_> {
        Box::pin(std::future::ready(self.calendar_scope.clone()))
    }

    fn action_inbox(&self, context: WorkbenchReadContext) -> SourceFuture<'_, ActionInboxPage> {
        self.record(context);
        if self.pending_action {
            Box::pin(std::future::pending())
        } else {
            Box::pin(std::future::ready(self.action.clone()))
        }
    }

    fn todos(&self, context: WorkbenchReadContext) -> SourceFuture<'_, TodoPage> {
        self.record(context);
        Box::pin(std::future::ready(self.todo.clone()))
    }

    fn calendar(&self, context: WorkbenchReadContext) -> SourceFuture<'_, CalendarPage> {
        self.record(context);
        Box::pin(std::future::ready(self.calendar.clone()))
    }
}

fn now() -> OffsetDateTime {
    OffsetDateTime::parse(
        "2026-07-02T00:00:00Z",
        &time::format_description::well_known::Rfc3339,
    )
    .unwrap()
}

fn context() -> WorkbenchReadContext {
    WorkbenchReadContext {
        principal: Principal::new(
            UserId::new(),
            OrgId::knl(),
            BTreeSet::new(),
            BranchScope::All,
        ),
        range: WorkbenchRange {
            from: now(),
            to: now() + time::Duration::days(1),
        },
        scope: EffectiveScope::All {
            selected_branch_id: None,
        },
        limits: WorkbenchLimits {
            action: 2,
            todo: 2,
            calendar: 2,
        },
        as_of: now(),
    }
}

fn target(id: impl Into<String>) -> WorkbenchTarget {
    WorkbenchTarget {
        module: "console".to_owned(),
        id: id.into(),
    }
}

fn action(id: &str, urgency: WorkbenchUrgency, due_hours: i64) -> ActionInboxItem {
    ActionInboxItem {
        id: id.to_owned(),
        urgency,
        title: id.to_owned(),
        due_at: Some(now() + time::Duration::hours(due_hours)),
        source: WorkbenchSourceRef {
            kind: "workflow_task".to_owned(),
            id: Uuid::new_v4(),
        },
        target: target(id),
    }
}

fn todo(order: u64) -> TodoItem {
    let id = Uuid::new_v4();
    TodoItem {
        id,
        text: format!("todo-{order}"),
        done: false,
        source_order: order,
        target: target(id.to_string()),
    }
}

fn calendar(hours: i64) -> CalendarItem {
    let id = Uuid::new_v4();
    CalendarItem {
        id,
        title: id.to_string(),
        starts_at: now() + time::Duration::hours(hours),
        ends_at: now() + time::Duration::hours(hours + 1),
        target: target(id.to_string()),
    }
}

fn ok_readers() -> Readers {
    Readers {
        action: Ok(ActionInboxPage {
            as_of: now(),
            total: 3,
            items: vec![
                action("wait", WorkbenchUrgency::Wait, 1),
                action("today", WorkbenchUrgency::Today, 3),
                action("now", WorkbenchUrgency::Now, 5),
            ],
        }),
        todo: Ok(TodoPage {
            as_of: now(),
            total: 3,
            items: vec![todo(2), todo(1), todo(3)],
        }),
        calendar: Ok(CalendarPage {
            as_of: now(),
            total: 3,
            items: vec![calendar(3), calendar(1), calendar(2)],
        }),
        action_scope: Ok(BranchScope::All),
        todo_scope: Ok(BranchScope::All),
        calendar_scope: Ok(BranchScope::All),
        pending_action: false,
        contexts: Arc::new(Mutex::new(Vec::new())),
    }
}

#[tokio::test]
async fn sorts_truncates_and_propagates_one_identical_context_to_every_source() {
    let readers = ok_readers();
    let expected = context();
    let result = compose(&readers, expected.clone()).await.unwrap();
    assert!(!result.partial);
    let SourceEnvelope::Ok {
        items, truncated, ..
    } = result.action_inbox
    else {
        panic!("action source must succeed")
    };
    assert_eq!(
        items
            .iter()
            .map(|item| item.id.as_str())
            .collect::<Vec<_>>(),
        vec!["now", "today"]
    );
    assert!(truncated);
    let recorded = readers.contexts.lock().unwrap();
    assert_eq!(recorded.len(), 3);
    for actual in recorded.iter() {
        assert_eq!(actual.as_of, expected.as_of);
        assert_eq!(actual.range, expected.range);
        assert_eq!(actual.scope, expected.scope);
        assert_eq!(actual.limits, expected.limits);
    }
}

#[tokio::test]
async fn partial_denied_or_unavailable_never_returns_stale_items_or_raw_errors() {
    let mut readers = ok_readers();
    readers.todo = Err(SourceFailure::Denied {
        code: "todo_access_denied",
    });
    readers.calendar = Err(SourceFailure::Unavailable {
        code: "calendar_timeout",
    });
    let json = serde_json::to_value(compose(&readers, context()).await.unwrap()).unwrap();
    assert_eq!(json["partial"], true);
    assert_eq!(
        json["todos"],
        serde_json::json!({"status":"denied","code":"todo_access_denied"})
    );
    assert_eq!(
        json["calendar"],
        serde_json::json!({"status":"unavailable","code":"calendar_timeout"})
    );
    assert!(!json.to_string().contains("SELECT"));
}

#[tokio::test]
async fn hung_source_times_out_while_other_envelopes_remain_authoritative() {
    let mut readers = ok_readers();
    readers.pending_action = true;
    let json = serde_json::to_value(compose(&readers, context()).await.unwrap()).unwrap();
    assert_eq!(json["partial"], true);
    assert_eq!(
        json["action_inbox"],
        serde_json::json!({"status":"unavailable","code":"source_timeout"})
    );
    assert_eq!(json["todos"]["status"], "ok");
    assert_eq!(json["calendar"]["status"], "ok");
}

#[test]
fn action_without_due_time_omits_due_at_instead_of_serializing_null() {
    let mut item = action("undated", WorkbenchUrgency::Wait, 1);
    item.due_at = None;

    let json = serde_json::to_value(item).unwrap();
    assert!(json.get("due_at").is_none());
}

#[tokio::test]
async fn all_unavailable_fails_closed_instead_of_fabricating_a_partial_payload() {
    let mut readers = ok_readers();
    readers.action = Err(SourceFailure::Unavailable {
        code: "owner_unavailable",
    });
    readers.todo = Err(SourceFailure::Unavailable {
        code: "owner_unavailable",
    });
    readers.calendar = Err(SourceFailure::Unavailable {
        code: "owner_unavailable",
    });
    assert!(compose(&readers, context()).await.is_err());
}

#[tokio::test]
async fn explicit_branch_requires_every_native_source_scope_not_ordinary_partial_denial() {
    let selected = BranchId::new();
    let mut readers = ok_readers();
    readers.todo_scope = Ok(BranchScope::none());
    let scope = EffectiveScope::Branches {
        branch_ids: vec![*selected.as_uuid()],
        selected_branch_id: Some(*selected.as_uuid()),
    };
    let error = preflight_explicit_branch(&readers, &context().principal, &scope)
        .await
        .unwrap_err();
    assert_eq!(
        error.into_response().status(),
        axum::http::StatusCode::FORBIDDEN
    );

    readers.todo_scope = Ok(BranchScope::All);
    readers.todo = Err(SourceFailure::Denied {
        code: "todo_access_denied",
    });
    assert!(
        preflight_explicit_branch(&readers, &context().principal, &scope)
            .await
            .is_ok()
    );
}

#[tokio::test]
async fn excludes_source_rows_from_a_snapshot_newer_than_the_request_ceiling() {
    let mut readers = ok_readers();
    readers.action.as_mut().unwrap().as_of = now() + time::Duration::seconds(1);
    let result = serde_json::to_value(compose(&readers, context()).await.unwrap()).unwrap();
    assert_eq!(result["partial"], true);
    assert_eq!(result["action_inbox"]["status"], "unavailable");
    assert!(result["action_inbox"].get("items").is_none());
}
