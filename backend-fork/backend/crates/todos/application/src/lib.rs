//! Todos application contracts.
//!
//! Owner-scoped CRUD shapes the Postgres adapter implements. Every command
//! carries the owner bound from the authenticated principal — never from
//! request input — mirroring the notifications recipient-scoping idiom.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

use console_kernel_core::{
    AuditAction, AuditEvent, KernelError, Timestamp, TodoId, TraceContext, UserId,
};
use console_todos_domain::TodoRef;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateTodoCommand {
    /// Bound from the authenticated principal, never from request input.
    pub owner: UserId,
    pub text: String,
    /// Scope chips: person/team/site/entity refs.
    pub scopes: Vec<TodoRef>,
    /// Object links: kind+id pairs into the object registry.
    pub links: Vec<TodoRef>,
    pub trace: TraceContext,
    pub occurred_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetTodoDoneCommand {
    pub owner: UserId,
    pub todo_id: TodoId,
    /// Explicit target state so the same endpoint supports done AND undo.
    pub done: bool,
    pub trace: TraceContext,
    pub occurred_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeleteTodoCommand {
    pub owner: UserId,
    pub todo_id: TodoId,
    pub trace: TraceContext,
    pub occurred_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListTodosQuery {
    pub owner: UserId,
    /// `false` = open items only (the Today/Plan panel default).
    pub include_done: bool,
    pub limit: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TodoSummary {
    pub id: TodoId,
    pub owner_user_id: UserId,
    pub text: String,
    pub scopes: Vec<TodoRef>,
    pub links: Vec<TodoRef>,
    pub done: bool,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: Timestamp,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: Timestamp,
    #[serde(with = "time::serde::rfc3339::option")]
    pub done_at: Option<Timestamp>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TodoPage {
    pub items: Vec<TodoSummary>,
}

/// Build the audit event for an owner self-action on a todo.
pub fn todo_audit_event(
    action: &str,
    actor: UserId,
    todo_id: TodoId,
    trace: TraceContext,
    occurred_at: Timestamp,
) -> Result<AuditEvent, KernelError> {
    Ok(AuditEvent::new(
        Some(actor),
        AuditAction::new(action)?,
        "todo",
        todo_id.to_string(),
        trace,
        occurred_at,
    ))
}
