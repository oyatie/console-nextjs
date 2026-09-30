//! Postgres adapter for the support-ticket domain.
//!
//! Every mutation routes through `with_audit`/`with_audits` so an `audit_events`
//! row lands in the SAME transaction as the state change (audit-coverage gate).
//! All reads are branch-scoped; `BranchScope::All` (SUPER_ADMIN/EXECUTIVE) sees
//! cross-branch rollups like reporting.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

use console_kernel_core::{
    BranchId, BranchScope, CustomerId, ErrorKind, KernelError, SiteId, SupportTicketCommentId,
    SupportTicketId, UserId, WorkOrderId,
};
use console_platform_db::{DbError, with_audit, with_audits, with_org_conn};
use console_platform_request_context::current_org;
use console_support_application::{
    AddCommentCommand, AssignTicketCommand, CommentAudience, CommentView,
    CreateCustomerIntakeCommand, CreateInternalTicketCommand, FieldAttendanceEvent,
    FieldSiteDetail, FieldSitePage, FieldSiteRow, FieldSiteSummary, FieldSlaSummary,
    FieldWorkOrderRef, LinkTicketCommand, ListFieldSitesQuery, ListTicketsQuery,
    RecordAcceptanceCommand, TicketAcceptanceView, TicketDetail, TicketNotification,
    TicketNotificationKind, TicketPage, TicketSummary, TransitionTicketCommand,
    support_audit_event,
};
use console_support_domain::{
    AcceptanceChannel, AcceptanceKind, FieldSlaState, SlaPolicy, TicketCategory, TicketOrigin,
    TicketPriority, TicketStatus,
};
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Postgres, QueryBuilder, Row, Transaction};
use time::OffsetDateTime;

#[derive(Debug, thiserror::Error)]
pub enum PgSupportError {
    #[error(transparent)]
    Db(#[from] DbError),

    #[error(transparent)]
    Domain(#[from] KernelError),
}

impl PgSupportError {
    #[must_use]
    pub fn kind(&self) -> ErrorKind {
        match self {
            Self::Domain(error) => error.kind,
            Self::Db(DbError::Sqlx(sqlx::Error::RowNotFound)) => ErrorKind::NotFound,
            Self::Db(DbError::Sqlx(sqlx::Error::Database(error)))
                if error.code().is_some_and(|code| code == "23505") =>
            {
                ErrorKind::Conflict
            }
            Self::Db(_) => ErrorKind::Internal,
        }
    }
}

impl From<sqlx::Error> for PgSupportError {
    fn from(value: sqlx::Error) -> Self {
        Self::Db(DbError::Sqlx(value))
    }
}

/// Single source for the [`TicketSummary`] projection (used by list/detail/
/// mutation-return/action-inbox reads alike, so a column added once shows up
/// everywhere `summary_from_row` decodes). The correlated subqueries are
/// same-org lookups (RLS-scoped to `app.current_org`, exactly like the LEFT
/// JOINs they stand in for) resolving display names — NULL when the reference
/// is unset or the referent is gone; the web renders them through `safeLabel`
/// so a missing name never leaks a UUID.
const TICKET_SUMMARY_SELECT: &str = "SELECT \
     id, branch_id, origin, category, priority, status, title, \
     requester_user_id, requester_name, assignee_user_id, \
     (SELECT u.display_name FROM users u \
       WHERE u.id = support_tickets.assignee_user_id) AS assignee_name, \
     due_at, created_at, updated_at, resolved_at, closed_at, \
     site_id, customer_id, work_order_id, \
     (SELECT rs.name FROM registry_sites rs \
       WHERE rs.id = support_tickets.site_id) AS site_name, \
     (SELECT rc.name FROM registry_customers rc \
       WHERE rc.id = support_tickets.customer_id) AS customer_name \
     FROM support_tickets";

/// Deterministic per-site SLA state (§4-28 no-AI; single evaluation site shared
/// by the overview rows, the `sla` filter, and the detail rollup): BREACHED if
/// any open ticket is past `due_at`; else AT_RISK if any is due within 24h;
/// else OK. Open = OPEN/IN_PROGRESS/ON_HOLD. Expects `breached_ticket_count`
/// and `next_due_at` columns in scope.
const FIELD_SLA_CASE: &str = "CASE \
     WHEN breached_ticket_count > 0 THEN 'BREACHED' \
     WHEN next_due_at < now() + interval '24 hours' THEN 'AT_RISK' \
     ELSE 'OK' END";

/// Work-order statuses that no longer count as an active visit (the workorder
/// crate's terminal vocabulary; rendered read-only here).
const WORK_ORDER_TERMINAL_STATUSES: &str = "('FINAL_COMPLETED','REJECTED','ARCHIVED','CANCELLED')";

/// Single source for the [`TicketAcceptanceView`] projection (replay lookup,
/// detail history, and post-insert readback all decode the same columns via
/// [`acceptance_from_row`]). Aliased `a` so history reads can join the ticket.
const ACCEPTANCE_SELECT: &str = "SELECT \
     a.id, a.ticket_id, a.kind, a.channel, a.accepted_by, a.note, \
     a.recorded_by_user_id, \
     (SELECT u.display_name FROM users u \
       WHERE u.id = a.recorded_by_user_id) AS recorded_by_name, \
     a.occurred_at, a.request_fingerprint \
     FROM support_ticket_acceptances a";

#[derive(Debug, Clone)]
pub struct PgSupportStore {
    pool: PgPool,
    sla: SlaPolicy,
}

impl PgSupportStore {
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self {
            pool,
            sla: SlaPolicy::default(),
        }
    }

    /// Override the default priority→SLA mapping (deployment-configurable).
    #[must_use]
    pub fn with_sla_policy(mut self, sla: SlaPolicy) -> Self {
        self.sla = sla;
        self
    }

    #[must_use]
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    // -----------------------------------------------------------------------
    // create_internal_ticket
    // -----------------------------------------------------------------------
    pub async fn create_internal_ticket(
        &self,
        command: CreateInternalTicketCommand,
    ) -> Result<TicketSummary, PgSupportError> {
        let org = current_org().map_err(KernelError::from)?;
        let org_uuid = *org.as_uuid();
        let title = require_non_empty(&command.title, "support ticket title is required")?;
        require_max_chars(&title, MAX_TITLE_CHARS, "support ticket title is too long")?;
        let body = require_non_empty(&command.body, "support ticket body is required")?;
        require_max_chars(&body, MAX_BODY_CHARS, "support ticket body is too long")?;
        let ticket_id = SupportTicketId::new();
        let due_at = self.sla.due_at(command.priority, command.occurred_at)?;
        let event = support_audit_event(
            "support.ticket.create_internal",
            Some(command.actor),
            Some(command.branch_id),
            "support_ticket",
            ticket_id,
            command.trace.clone(),
            command.occurred_at,
        )?
        .with_snapshots(
            None,
            Some(serde_json::json!({
                "origin": TicketOrigin::Internal.as_db_str(),
                "category": command.category.as_db_str(),
                "priority": command.priority.as_db_str(),
                "status": TicketStatus::Open.as_db_str(),
                "branch_id": command.branch_id.to_string(),
            })),
        )
        // Arm the tenant on the audit event so `with_audit` binds
        // `app.current_org` BEFORE the closure runs. Without it, the
        // ensure_active_user_in_branch SELECT + the INSERT execute under FORCE
        // RLS with an unset GUC and fail closed as the real `console_rt` role.
        .with_org(org);

        with_audit::<_, TicketSummary, PgSupportError>(&self.pool, event, |tx| {
            Box::pin(async move {
                ensure_active_user_in_branch(tx, command.actor, command.branch_id).await?;
                sqlx::query(
                    r#"
                    INSERT INTO support_tickets (
                        id, branch_id, origin, category, priority, status,
                        title, body, requester_user_id, due_at, created_at, updated_at, org_id
                    )
                    VALUES ($1, $2, 'INTERNAL', $3, $4, 'OPEN', $5, $6, $7, $8, $9, $9, $10)
                    "#,
                )
                .bind(*ticket_id.as_uuid())
                .bind(*command.branch_id.as_uuid())
                .bind(command.category.as_db_str())
                .bind(command.priority.as_db_str())
                .bind(&title)
                .bind(&body)
                .bind(*command.actor.as_uuid())
                .bind(due_at)
                .bind(command.occurred_at)
                .bind(org_uuid)
                .execute(tx.as_mut())
                .await?;

                fetch_summary_tx(tx, ticket_id).await
            })
        })
        .await
    }

    // -----------------------------------------------------------------------
    // create_customer_intake (unauthenticated channel)
    // -----------------------------------------------------------------------
    pub async fn create_customer_intake(
        &self,
        command: CreateCustomerIntakeCommand,
    ) -> Result<TicketSummary, PgSupportError> {
        let org = current_org().map_err(KernelError::from)?;
        let org_uuid = *org.as_uuid();
        let title = require_non_empty(&command.title, "support ticket title is required")?;
        require_max_chars(&title, MAX_TITLE_CHARS, "support ticket title is too long")?;
        let body = require_non_empty(&command.body, "support ticket body is required")?;
        require_max_chars(&body, MAX_BODY_CHARS, "support ticket body is too long")?;
        let requester_name =
            require_non_empty(&command.requester_name, "requester name is required")?;
        require_max_chars(
            &requester_name,
            MAX_REQUESTER_NAME_CHARS,
            "requester name is too long",
        )?;
        let requester_contact =
            require_non_empty(&command.requester_contact, "requester contact is required")?;
        require_max_chars(
            &requester_contact,
            MAX_REQUESTER_CONTACT_CHARS,
            "requester contact is too long",
        )?;
        let ticket_id = SupportTicketId::new();
        let due_at = self.sla.due_at(command.priority, command.occurred_at)?;
        // No actor, no branch: the audit snapshot deliberately omits the PII
        // contact — it is never copied into audit_events.
        let event = support_audit_event(
            "support.ticket.create_customer",
            None,
            None,
            "support_ticket",
            ticket_id,
            command.trace.clone(),
            command.occurred_at,
        )?
        .with_snapshots(
            None,
            Some(serde_json::json!({
                "origin": TicketOrigin::Customer.as_db_str(),
                "category": command.category.as_db_str(),
                "priority": command.priority.as_db_str(),
                "status": TicketStatus::Open.as_db_str(),
            })),
        )
        // Arm the tenant on the audit event so `with_audit` binds
        // `app.current_org` BEFORE the closure runs; otherwise the customer-row
        // INSERT executes under FORCE RLS with an unset GUC and fails closed.
        .with_org(org);

        with_audit::<_, TicketSummary, PgSupportError>(&self.pool, event, |tx| {
            Box::pin(async move {
                sqlx::query(
                    r#"
                    INSERT INTO support_tickets (
                        id, branch_id, origin, category, priority, status,
                        title, body, requester_name, requester_contact,
                        due_at, created_at, updated_at, org_id
                    )
                    VALUES ($1, NULL, 'CUSTOMER', $2, $3, 'OPEN', $4, $5, $6, $7, $8, $9, $9, $10)
                    "#,
                )
                .bind(*ticket_id.as_uuid())
                .bind(command.category.as_db_str())
                .bind(command.priority.as_db_str())
                .bind(&title)
                .bind(&body)
                .bind(&requester_name)
                .bind(&requester_contact)
                .bind(due_at)
                .bind(command.occurred_at)
                .bind(org_uuid)
                .execute(tx.as_mut())
                .await?;

                fetch_summary_tx(tx, ticket_id).await
            })
        })
        .await
    }

    // -----------------------------------------------------------------------
    // assign_ticket
    // -----------------------------------------------------------------------
    pub async fn assign_ticket(
        &self,
        command: AssignTicketCommand,
    ) -> Result<(TicketSummary, Vec<TicketNotification>), PgSupportError> {
        let org = current_org().map_err(KernelError::from)?;
        with_audits::<_, (TicketSummary, Vec<TicketNotification>), PgSupportError>(
            &self.pool,
            org,
            |tx| {
                Box::pin(async move {
                    let ticket = lock_ticket_tx(tx, command.ticket_id).await?;
                    // Resolve the effective branch: a branch-less customer ticket
                    // must be triaged into a branch on assignment.
                    let effective_branch = match ticket.branch_id {
                        Some(branch) => branch,
                        None => command.branch_id.ok_or_else(|| {
                            KernelError::validation(
                                "branch_id is required to triage an untriaged customer ticket",
                            )
                        })?,
                    };
                    ensure_active_user_in_branch(tx, command.assignee_user_id, effective_branch)
                        .await?;

                    sqlx::query(
                        r#"
                        UPDATE support_tickets
                        SET assignee_user_id = $2,
                            branch_id = $3,
                            updated_at = $4
                        WHERE id = $1
                        "#,
                    )
                    .bind(*command.ticket_id.as_uuid())
                    .bind(*command.assignee_user_id.as_uuid())
                    .bind(*effective_branch.as_uuid())
                    .bind(command.occurred_at)
                    .execute(tx.as_mut())
                    .await?;

                    let summary = fetch_summary_tx(tx, command.ticket_id).await?;
                    let event = support_audit_event(
                        "support.ticket.assign",
                        Some(command.actor),
                        Some(effective_branch),
                        "support_ticket",
                        command.ticket_id,
                        command.trace.clone(),
                        command.occurred_at,
                    )?
                    .with_snapshots(
                        Some(serde_json::json!({
                            "assignee_user_id": ticket.assignee_user_id.map(|id| id.to_string()),
                            "branch_id": ticket.branch_id.map(|id| id.to_string()),
                        })),
                        Some(serde_json::json!({
                            "assignee_user_id": command.assignee_user_id.to_string(),
                            "branch_id": effective_branch.to_string(),
                        })),
                    )
                    .with_org(org);
                    let notifications = vec![TicketNotification::new(
                        command.ticket_id,
                        command.assignee_user_id,
                        TicketNotificationKind::Assigned,
                        "A support ticket was assigned to you.",
                    )];
                    Ok(((summary, notifications), vec![event]))
                })
            },
        )
        .await
    }

    // -----------------------------------------------------------------------
    // transition_status
    // -----------------------------------------------------------------------
    pub async fn transition_status(
        &self,
        command: TransitionTicketCommand,
    ) -> Result<(TicketSummary, Vec<TicketNotification>), PgSupportError> {
        let org = current_org().map_err(KernelError::from)?;
        with_audits::<_, (TicketSummary, Vec<TicketNotification>), PgSupportError>(
            &self.pool,
            org,
            |tx| {
                Box::pin(async move {
                    let ticket = lock_ticket_tx(tx, command.ticket_id).await?;
                    // FSM enforcement lives in the pure domain.
                    let transition = ticket.status.transition_to(command.to_status)?;
                    let resolved_at = resolved_timestamp(
                        ticket.resolved_at,
                        command.to_status,
                        command.occurred_at,
                    );
                    let closed_at =
                        closed_timestamp(ticket.closed_at, command.to_status, command.occurred_at);

                    sqlx::query(
                        r#"
                        UPDATE support_tickets
                        SET status = $2,
                            resolved_at = $3,
                            closed_at = $4,
                            updated_at = $5
                        WHERE id = $1
                        "#,
                    )
                    .bind(*command.ticket_id.as_uuid())
                    .bind(command.to_status.as_db_str())
                    .bind(resolved_at)
                    .bind(closed_at)
                    .bind(command.occurred_at)
                    .execute(tx.as_mut())
                    .await?;

                    let summary = fetch_summary_tx(tx, command.ticket_id).await?;
                    let event = support_audit_event(
                        "support.ticket.transition",
                        Some(command.actor),
                        ticket.branch_id,
                        "support_ticket",
                        command.ticket_id,
                        command.trace.clone(),
                        command.occurred_at,
                    )?
                    .with_snapshots(
                        Some(serde_json::json!({ "status": transition.from.as_db_str() })),
                        Some(serde_json::json!({ "status": transition.to.as_db_str() })),
                    )
                    .with_org(org);
                    let notifications =
                        status_change_notifications(&ticket, command.ticket_id, command.to_status);
                    Ok(((summary, notifications), vec![event]))
                })
            },
        )
        .await
    }

    // -----------------------------------------------------------------------
    // add_comment
    // -----------------------------------------------------------------------
    pub async fn add_comment(
        &self,
        command: AddCommentCommand,
    ) -> Result<(CommentView, Vec<TicketNotification>), PgSupportError> {
        let org = current_org().map_err(KernelError::from)?;
        let org_uuid = *org.as_uuid();
        let body = require_non_empty(&command.body, "comment body is required")?;
        let comment_id = SupportTicketCommentId::new();

        with_audits::<_, (CommentView, Vec<TicketNotification>), PgSupportError>(&self.pool, org, |tx| {
            Box::pin(async move {
                let ticket = lock_ticket_tx(tx, command.ticket_id).await?;
                ensure_author_visible_to_ticket(tx, command.actor, &ticket).await?;

                sqlx::query(
                    r#"
                        INSERT INTO support_ticket_comments (
                            id, ticket_id, author_user_id, body, is_internal_note, created_at, org_id
                        )
                        VALUES ($1, $2, $3, $4, $5, $6, $7)
                        "#,
                )
                .bind(*comment_id.as_uuid())
                .bind(*command.ticket_id.as_uuid())
                .bind(*command.actor.as_uuid())
                .bind(&body)
                .bind(command.is_internal_note)
                .bind(command.occurred_at)
                .bind(org_uuid)
                .execute(tx.as_mut())
                .await?;

                sqlx::query("UPDATE support_tickets SET updated_at = $2 WHERE id = $1")
                    .bind(*command.ticket_id.as_uuid())
                    .bind(command.occurred_at)
                    .execute(tx.as_mut())
                    .await?;

                let view = fetch_comment_tx(tx, comment_id).await?;
                let event = support_audit_event(
                    "support.ticket.comment",
                    Some(command.actor),
                    ticket.branch_id,
                    "support_ticket_comment",
                    comment_id,
                    command.trace.clone(),
                    command.occurred_at,
                )?
                .with_snapshots(
                    None,
                    Some(serde_json::json!({
                        "ticket_id": command.ticket_id.to_string(),
                        "is_internal_note": command.is_internal_note,
                    })),
                )
                .with_org(org);
                // Internal notes do not notify the requester; non-internal
                // comments notify requester + assignee (excluding the author).
                let notifications = if command.is_internal_note {
                    Vec::new()
                } else {
                    comment_notifications(&ticket, command.ticket_id, command.actor)
                };
                Ok(((view, notifications), vec![event]))
            })
        })
        .await
    }

    // -----------------------------------------------------------------------
    // list_tickets (branch-scoped + filters)
    // -----------------------------------------------------------------------
    pub async fn list_tickets(
        &self,
        query: ListTicketsQuery,
    ) -> Result<TicketPage, PgSupportError> {
        // Always clamp to a hard server-side cap so an unbounded fetch is
        // impossible, even when the client sends no limit.
        let limit = normalized_limit(query.limit);
        // Resolve the keyset cursor up front; an unknown cursor is a not-found.
        let cursor = match query.cursor {
            Some(cursor_id) => Some(ticket_cursor(&self.pool, cursor_id).await?),
            None => None,
        };

        // Branch scope + filters are shared by the page query and the COUNT.
        // The COUNT must NOT apply the keyset cursor, so the total is the same
        // across every page of the same filter set.
        let branch_scope = query.branch_scope.clone();
        let include_untriaged = query.include_untriaged;
        let status = query.status;
        let priority = query.priority;
        let category = query.category;
        let origin = query.origin;
        let assignee_user_id = query.assignee_user_id;
        let site_id = query.site_id;
        let push_filters = move |builder: &mut QueryBuilder<Postgres>| {
            builder.push("(");
            push_branch_scope(builder, &branch_scope, include_untriaged);
            builder.push(")");
            if let Some(status) = status {
                builder.push(" AND status = ");
                builder.push_bind(status.as_db_str());
            }
            if let Some(priority) = priority {
                builder.push(" AND priority = ");
                builder.push_bind(priority.as_db_str());
            }
            if let Some(category) = category {
                builder.push(" AND category = ");
                builder.push_bind(category.as_db_str());
            }
            if let Some(origin) = origin {
                builder.push(" AND origin = ");
                builder.push_bind(origin.as_db_str());
            }
            if let Some(assignee) = assignee_user_id {
                builder.push(" AND assignee_user_id = ");
                builder.push_bind(*assignee.as_uuid());
            }
            if let Some(site) = site_id {
                // Field-console per-site queue (partial index in 0194).
                builder.push(" AND site_id = ");
                builder.push_bind(*site.as_uuid());
            }
        };

        let mut count_builder =
            QueryBuilder::<Postgres>::new("SELECT COUNT(*) FROM support_tickets WHERE ");
        push_filters(&mut count_builder);

        let mut builder = QueryBuilder::<Postgres>::new(format!("{TICKET_SUMMARY_SELECT} WHERE "));
        push_filters(&mut builder);
        // Keyset: strictly after the cursor on the (created_at DESC, id) order.
        if let Some((created_at, id)) = cursor {
            builder.push(" AND (created_at, id) < (");
            builder.push_bind(created_at);
            builder.push(", ");
            builder.push_bind(id);
            builder.push(")");
        }
        // Fetch one extra row to know whether a further page exists; the extra
        // row (if any) becomes the keyset cursor and is not returned.
        builder.push(" ORDER BY created_at DESC, id DESC LIMIT ");
        builder.push_bind(limit + 1);

        let org = current_org().map_err(KernelError::from)?;
        let (total, rows) = with_org_conn::<_, _, PgSupportError>(&self.pool, org, move |tx| {
            Box::pin(async move {
                let total: i64 = count_builder
                    .build_query_scalar()
                    .fetch_one(tx.as_mut())
                    .await?;
                let rows = builder.build().fetch_all(tx.as_mut()).await?;
                Ok((total, rows))
            })
        })
        .await?;

        let mut items = rows
            .iter()
            .map(summary_from_row)
            .collect::<Result<Vec<_>, _>>()?;
        let next_cursor = if i64::try_from(items.len()).unwrap_or(0) > limit {
            // Drop the look-ahead row and expose the last KEPT id as the cursor.
            items.truncate(usize::try_from(limit).unwrap_or(items.len()));
            items.last().map(|ticket| ticket.id)
        } else {
            None
        };
        Ok(TicketPage {
            items,
            next_cursor,
            total,
        })
    }

    /// Bounded due/id keyset page for the unified action inbox. It reuses the
    /// canonical branch-scope builder and assignee predicate from
    /// [`Self::list_tickets`], while excluding terminal tickets in SQL.
    pub async fn list_assigned_action_inbox_page(
        &self,
        branch_scope: BranchScope,
        assignee: UserId,
        as_of: OffsetDateTime,
        after: Option<(OffsetDateTime, String)>,
        limit: i64,
    ) -> Result<(Vec<TicketSummary>, i64, bool), PgSupportError> {
        let limit = limit.clamp(1, 200);
        let (after_created_at, after_id) = after.map_or((None, None), |(created_at, id)| {
            (Some(created_at), Some(id))
        });
        let push_filters = move |builder: &mut QueryBuilder<Postgres>| {
            builder.push("(");
            push_branch_scope(builder, &branch_scope, false);
            builder.push(") AND assignee_user_id = ");
            builder.push_bind(*assignee.as_uuid());
            builder.push(" AND status NOT IN ('RESOLVED', 'CLOSED') AND created_at <= ");
            builder.push_bind(as_of);
        };
        let mut count_builder =
            QueryBuilder::<Postgres>::new("SELECT COUNT(*) FROM support_tickets WHERE ");
        push_filters(&mut count_builder);
        let mut builder = QueryBuilder::<Postgres>::new(format!("{TICKET_SUMMARY_SELECT} WHERE "));
        push_filters(&mut builder);
        if let (Some(after_created_at), Some(after_id)) = (after_created_at, after_id) {
            builder.push(" AND (created_at > ");
            builder.push_bind(after_created_at);
            builder.push(" OR (created_at = ");
            builder.push_bind(after_created_at);
            builder.push(" AND ('support:' || id::text) > ");
            builder.push_bind(after_id);
            builder.push("))");
        }
        builder.push(" ORDER BY created_at ASC, ('support:' || id::text) ASC LIMIT ");
        builder.push_bind(limit.clamp(1, 200) + 1);
        let org = current_org().map_err(KernelError::from)?;
        let (total, rows) = with_org_conn::<_, _, PgSupportError>(&self.pool, org, move |tx| {
            Box::pin(async move {
                let total = count_builder
                    .build_query_scalar::<i64>()
                    .fetch_one(tx.as_mut())
                    .await?;
                let rows = builder.build().fetch_all(tx.as_mut()).await?;
                Ok((total, rows))
            })
        })
        .await?;
        let has_more = i64::try_from(rows.len()).unwrap_or(0) > limit;
        let items = rows
            .iter()
            .take(usize::try_from(limit).unwrap_or(rows.len()))
            .map(summary_from_row)
            .collect::<Result<Vec<_>, _>>()?;
        Ok((items, total, has_more))
    }

    // -----------------------------------------------------------------------
    // get_ticket (+ comments, audience-filtered)
    // -----------------------------------------------------------------------
    pub async fn get_ticket(
        &self,
        ticket_id: SupportTicketId,
        branch_scope: &BranchScope,
        audience: CommentAudience,
    ) -> Result<TicketDetail, PgSupportError> {
        let mut builder =
            QueryBuilder::<Postgres>::new(format!("{TICKET_SUMMARY_SELECT} WHERE id = "));
        builder.push_bind(*ticket_id.as_uuid());
        builder.push(" AND (");
        // A branch-less customer ticket is only visible to cross-branch staff.
        push_branch_scope(&mut builder, branch_scope, true);
        builder.push(")");
        let org = current_org().map_err(KernelError::from)?;
        let row = with_org_conn::<_, _, PgSupportError>(&self.pool, org, move |tx| {
            Box::pin(async move { Ok(builder.build().fetch_optional(tx.as_mut()).await?) })
        })
        .await?
        .ok_or_else(|| KernelError::not_found("support ticket was not found"))?;
        let ticket = summary_from_row(&row)?;

        let comments = self.list_comments(ticket_id, audience).await?;
        Ok(TicketDetail { ticket, comments })
    }

    async fn list_comments(
        &self,
        ticket_id: SupportTicketId,
        audience: CommentAudience,
    ) -> Result<Vec<CommentView>, PgSupportError> {
        let mut builder = QueryBuilder::<Postgres>::new(
            r#"
            SELECT id, ticket_id, author_user_id, body, is_internal_note, created_at,
                -- Same-org correlated lookup (RLS-scoped to app.current_org):
                -- the comment author's display name, NULL when authorless/deleted.
                (SELECT u.display_name FROM users u
                  WHERE u.id = support_ticket_comments.author_user_id) AS author_name
            FROM support_ticket_comments
            WHERE ticket_id =
            "#,
        );
        builder.push_bind(*ticket_id.as_uuid());
        if !audience.shows_internal_notes() {
            // Customer-visible path never returns internal staff notes.
            builder.push(" AND is_internal_note = FALSE");
        }
        builder.push(" ORDER BY created_at, id");
        let org = current_org().map_err(KernelError::from)?;
        let rows = with_org_conn::<_, _, PgSupportError>(&self.pool, org, move |tx| {
            Box::pin(async move { Ok(builder.build().fetch_all(tx.as_mut()).await?) })
        })
        .await?;
        rows.iter().map(comment_from_row).collect()
    }

    /// Atomically increment (or insert) the fixed-window rate-limit counter for
    /// one bucket and return the new attempt count. Shares the `auth_rate_limit`
    /// table and the same UPSERT semantics the auth endpoints use; the
    /// `endpoint` key (e.g. `support_intake`) isolates the support buckets.
    ///
    /// This is a coarse counter, not an audited state change — it deliberately
    /// lives in the adapter (not a REST handler surface) so it is exempt from the
    /// audit-coverage gate, exactly as the auth crate's identical counter is.
    pub async fn increment_rate_bucket(
        &self,
        client_key: &str,
        endpoint: &str,
        window_start: OffsetDateTime,
    ) -> Result<i64, PgSupportError> {
        let attempts: i32 = sqlx::query_scalar(
            r#"
            INSERT INTO auth_rate_limit (client_key, endpoint, window_start, attempts)
            VALUES ($1, $2, $3, 1)
            ON CONFLICT (client_key, endpoint, window_start)
            DO UPDATE SET attempts = auth_rate_limit.attempts + 1
            RETURNING attempts
            "#,
        )
        .bind(client_key)
        .bind(endpoint)
        .bind(window_start)
        // rls-arming: ok auth_rate_limit is a global table (no org_id, no RLS)
        .fetch_one(&self.pool)
        .await?;
        Ok(i64::from(attempts))
    }

    /// Active push tokens for a staff recipient, for notification fan-out.
    pub async fn active_push_tokens(&self, user_id: UserId) -> Result<Vec<String>, PgSupportError> {
        let org = current_org().map_err(KernelError::from)?;
        let tokens: Vec<String> =
            with_org_conn::<_, _, PgSupportError>(&self.pool, org, move |tx| {
                Box::pin(async move {
                    Ok(sqlx::query_scalar(
                        r#"
            SELECT push_token
            FROM registered_devices
            WHERE user_id = $1
              AND push_token IS NOT NULL
              AND btrim(push_token) <> ''
            "#,
                    )
                    .bind(*user_id.as_uuid())
                    .fetch_all(tx.as_mut())
                    .await?)
                })
            })
            .await?;
        Ok(tokens)
    }

    /// Resolve the branch of a ticket within scope, for REST authorization.
    pub async fn ticket_branch_in_scope(
        &self,
        ticket_id: SupportTicketId,
        branch_scope: &BranchScope,
    ) -> Result<Option<BranchId>, PgSupportError> {
        let mut builder =
            QueryBuilder::<Postgres>::new("SELECT branch_id FROM support_tickets WHERE id = ");
        builder.push_bind(*ticket_id.as_uuid());
        builder.push(" AND (");
        push_branch_scope(&mut builder, branch_scope, true);
        builder.push(")");
        let org = current_org().map_err(KernelError::from)?;
        let row = with_org_conn::<_, _, PgSupportError>(&self.pool, org, move |tx| {
            Box::pin(async move { Ok(builder.build().fetch_optional(tx.as_mut()).await?) })
        })
        .await?
        .ok_or_else(|| KernelError::not_found("support ticket was not found"))?;
        let branch_id: Option<uuid::Uuid> = row.try_get("branch_id")?;
        Ok(branch_id.map(BranchId::from_uuid))
    }

    /// Resolve a site's branch within the principal's scope, for REST
    /// authorization of the field detail route. Out-of-scope sites are a
    /// not-found (404, never 403) so their existence does not leak.
    pub async fn site_branch_in_scope(
        &self,
        site_id: SiteId,
        branch_scope: &BranchScope,
    ) -> Result<BranchId, PgSupportError> {
        let mut builder =
            QueryBuilder::<Postgres>::new("SELECT branch_id FROM registry_sites WHERE id = ");
        builder.push_bind(*site_id.as_uuid());
        builder.push(" AND (");
        push_site_scope(&mut builder, branch_scope);
        builder.push(")");
        let org = current_org().map_err(KernelError::from)?;
        let row = with_org_conn::<_, _, PgSupportError>(&self.pool, org, move |tx| {
            Box::pin(async move { Ok(builder.build().fetch_optional(tx.as_mut()).await?) })
        })
        .await?
        .ok_or_else(|| KernelError::not_found("site was not found"))?;
        Ok(BranchId::from_uuid(row.try_get("branch_id")?))
    }

    // -----------------------------------------------------------------------
    // list_field_sites (field-console overview; branch-scoped aggregation)
    // -----------------------------------------------------------------------
    /// One keyset page of the per-site field overview. Every count and the SLA
    /// state derive from the same aggregation the rows come from, so the stat
    /// bar can never disagree with the list (§4-11). Cross-crate reads of
    /// `work_orders` / `site_attendance_events` / `registry_*` are documented
    /// coupling (gap-analysis §6.1); writes/DDL stay with their owners.
    pub async fn list_field_sites(
        &self,
        query: ListFieldSitesQuery,
    ) -> Result<FieldSitePage, PgSupportError> {
        let limit = normalized_limit(query.limit);
        // Resolve the keyset cursor up front, scope-confined: an out-of-scope
        // or unknown cursor is a not-found, not an information leak.
        let cursor = match query.cursor {
            Some(cursor_id) => {
                Some(field_site_cursor(&self.pool, cursor_id, &query.branch_scope).await?)
            }
            None => None,
        };
        let needle = query
            .q
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(escape_like);

        // COUNT shares scope + filters (including the derived-SLA filter) but
        // never the cursor, so `total` is stable across pages.
        let mut count_builder = QueryBuilder::<Postgres>::new("SELECT COUNT(*) FROM (");
        push_field_sites_inner(
            &mut count_builder,
            &query.branch_scope,
            needle.as_deref(),
            query.customer_id,
            None,
        );
        count_builder.push(") s");
        if let Some(sla) = query.sla {
            count_builder.push(format!(" WHERE {FIELD_SLA_CASE} = "));
            count_builder.push_bind(sla.as_db_str());
        }

        let mut builder =
            QueryBuilder::<Postgres>::new(format!("SELECT s.*, {FIELD_SLA_CASE} AS sla FROM ("));
        push_field_sites_inner(
            &mut builder,
            &query.branch_scope,
            needle.as_deref(),
            query.customer_id,
            cursor.as_ref(),
        );
        builder.push(") s");
        if let Some(sla) = query.sla {
            builder.push(format!(" WHERE {FIELD_SLA_CASE} = "));
            builder.push_bind(sla.as_db_str());
        }
        // Fetch one extra row to know whether a further page exists.
        builder.push(" ORDER BY site_name, site_id LIMIT ");
        builder.push_bind(limit + 1);

        let org = current_org().map_err(KernelError::from)?;
        let (total, rows) = with_org_conn::<_, _, PgSupportError>(&self.pool, org, move |tx| {
            Box::pin(async move {
                let total: i64 = count_builder
                    .build_query_scalar()
                    .fetch_one(tx.as_mut())
                    .await?;
                let rows = builder.build().fetch_all(tx.as_mut()).await?;
                Ok((total, rows))
            })
        })
        .await?;

        let mut items = rows
            .iter()
            .map(field_site_row_from_row)
            .collect::<Result<Vec<_>, _>>()?;
        let next_cursor = if i64::try_from(items.len()).unwrap_or(0) > limit {
            items.truncate(usize::try_from(limit).unwrap_or(items.len()));
            items.last().map(|site| site.site_id)
        } else {
            None
        };
        Ok(FieldSitePage {
            items,
            next_cursor,
            total,
        })
    }

    // -----------------------------------------------------------------------
    // field_site_detail (object + history layers)
    // -----------------------------------------------------------------------
    /// The field detail panel: site master facts, the SLA rollup, and the
    /// traversable history chain (tickets, work orders, attendance,
    /// acceptances — each capped at 50). Scope-confined: an out-of-scope site
    /// is a not-found.
    pub async fn field_site_detail(
        &self,
        site_id: SiteId,
        branch_scope: &BranchScope,
    ) -> Result<FieldSiteDetail, PgSupportError> {
        let mut site_builder = QueryBuilder::<Postgres>::new(
            "SELECT id, name, branch_id, customer_id, \
             (SELECT rc.name FROM registry_customers rc \
               WHERE rc.id = registry_sites.customer_id) AS customer_name, \
             address, province, city, postal_code, latitude, longitude, \
             geofence_radius_m, contact_name, contact_phone \
             FROM registry_sites WHERE id = ",
        );
        site_builder.push_bind(*site_id.as_uuid());
        site_builder.push(" AND (");
        push_site_scope(&mut site_builder, branch_scope);
        site_builder.push(")");

        let org = current_org().map_err(KernelError::from)?;
        let site_uuid = *site_id.as_uuid();
        let (site_row, sla_row, ticket_rows, wo_rows, att_rows, acc_rows) =
            with_org_conn::<_, _, PgSupportError>(&self.pool, org, move |tx| {
                Box::pin(async move {
                    let site_row = site_builder
                        .build()
                        .fetch_optional(tx.as_mut())
                        .await?
                        .ok_or_else(|| {
                            PgSupportError::from(KernelError::not_found("site was not found"))
                        })?;
                    // SLA rollup: current open/breached/next-due plus the 90-day
                    // resolution record. Tickets without a due date are excluded
                    // from the 90-day split rather than guessed (§4-28).
                    let sla_sql = format!(
                        "SELECT open_ticket_count, breached_ticket_count, next_due_at, \
                         resolved_within_sla_90d, resolved_breached_90d, {FIELD_SLA_CASE} AS sla \
                         FROM ( \
                           SELECT \
                             COUNT(*) FILTER (WHERE status IN ('OPEN','IN_PROGRESS','ON_HOLD')) AS open_ticket_count, \
                             COUNT(*) FILTER (WHERE status IN ('OPEN','IN_PROGRESS','ON_HOLD') AND due_at < now()) AS breached_ticket_count, \
                             MIN(due_at) FILTER (WHERE status IN ('OPEN','IN_PROGRESS','ON_HOLD')) AS next_due_at, \
                             COUNT(*) FILTER (WHERE resolved_at IS NOT NULL \
                               AND resolved_at >= now() - interval '90 days' \
                               AND due_at IS NOT NULL AND resolved_at <= due_at) AS resolved_within_sla_90d, \
                             COUNT(*) FILTER (WHERE resolved_at IS NOT NULL \
                               AND resolved_at >= now() - interval '90 days' \
                               AND due_at IS NOT NULL AND resolved_at > due_at) AS resolved_breached_90d \
                           FROM support_tickets WHERE site_id = $1 \
                         ) x"
                    );
                    let sla_row = sqlx::query(sqlx::AssertSqlSafe(sla_sql))
                        .bind(site_uuid)
                        .fetch_one(tx.as_mut())
                        .await?;
                    // History layers, most relevant first: open tickets before
                    // closed, then newest.
                    let tickets_sql = format!(
                        "{TICKET_SUMMARY_SELECT} WHERE site_id = $1 \
                         ORDER BY (status IN ('OPEN','IN_PROGRESS','ON_HOLD')) DESC, \
                         created_at DESC LIMIT 50"
                    );
                    let ticket_rows = sqlx::query(sqlx::AssertSqlSafe(tickets_sql))
                        .bind(site_uuid)
                        .fetch_all(tx.as_mut())
                        .await?;
                    let wo_rows = sqlx::query(
                        "SELECT id, request_no, status, priority, target_due_at, \
                         report_submitted_at, result_type, created_at \
                         FROM work_orders WHERE site_id = $1 \
                         ORDER BY created_at DESC LIMIT 50",
                    )
                    .bind(site_uuid)
                    .fetch_all(tx.as_mut())
                    .await?;
                    let att_rows = sqlx::query(
                        "SELECT user_id, \
                         (SELECT u.display_name FROM users u \
                           WHERE u.id = site_attendance_events.user_id) AS user_name, \
                         work_order_id, kind, occurred_at \
                         FROM site_attendance_events WHERE site_id = $1 \
                         ORDER BY occurred_at DESC, id DESC LIMIT 50",
                    )
                    .bind(site_uuid)
                    .fetch_all(tx.as_mut())
                    .await?;
                    let acc_sql = format!(
                        "{ACCEPTANCE_SELECT} JOIN support_tickets st ON st.id = a.ticket_id \
                         WHERE st.site_id = $1 ORDER BY a.occurred_at DESC LIMIT 50"
                    );
                    let acc_rows = sqlx::query(sqlx::AssertSqlSafe(acc_sql))
                        .bind(site_uuid)
                        .fetch_all(tx.as_mut())
                        .await?;
                    Ok((site_row, sla_row, ticket_rows, wo_rows, att_rows, acc_rows))
                })
            })
            .await?;

        let sla_state_raw: String = sla_row.try_get("sla")?;
        Ok(FieldSiteDetail {
            site: FieldSiteSummary {
                id: SiteId::from_uuid(site_row.try_get("id")?),
                name: site_row.try_get("name")?,
                branch_id: BranchId::from_uuid(site_row.try_get("branch_id")?),
                customer_id: CustomerId::from_uuid(site_row.try_get("customer_id")?),
                customer_name: site_row.try_get("customer_name")?,
                address: site_row.try_get("address")?,
                province: site_row.try_get("province")?,
                city: site_row.try_get("city")?,
                postal_code: site_row.try_get("postal_code")?,
                latitude: site_row.try_get("latitude")?,
                longitude: site_row.try_get("longitude")?,
                geofence_radius_m: site_row.try_get("geofence_radius_m")?,
                contact_name: site_row.try_get("contact_name")?,
                contact_phone: site_row.try_get("contact_phone")?,
            },
            sla: FieldSlaSummary {
                state: FieldSlaState::from_db_str(&sla_state_raw)?,
                open: sla_row.try_get("open_ticket_count")?,
                breached: sla_row.try_get("breached_ticket_count")?,
                next_due_at: sla_row.try_get("next_due_at")?,
                resolved_within_sla_90d: sla_row.try_get("resolved_within_sla_90d")?,
                resolved_breached_90d: sla_row.try_get("resolved_breached_90d")?,
            },
            tickets: ticket_rows
                .iter()
                .map(summary_from_row)
                .collect::<Result<Vec<_>, _>>()?,
            work_orders: wo_rows
                .iter()
                .map(work_order_ref_from_row)
                .collect::<Result<Vec<_>, _>>()?,
            attendance: att_rows
                .iter()
                .map(attendance_event_from_row)
                .collect::<Result<Vec<_>, _>>()?,
            acceptances: acc_rows
                .iter()
                .map(acceptance_from_row)
                .collect::<Result<Vec<_>, _>>()?,
        })
    }

    // -----------------------------------------------------------------------
    // link_ticket (bind intake to the field object chain)
    // -----------------------------------------------------------------------
    /// Bind a ticket to a customer site and/or the work order dispatched for
    /// the visit. Linking a site to an untriaged customer ticket also sets its
    /// branch (= triage). `customer_id` is always denormalized from the site.
    pub async fn link_ticket(
        &self,
        command: LinkTicketCommand,
    ) -> Result<TicketSummary, PgSupportError> {
        if command.site_id.is_none() && command.work_order_id.is_none() {
            return Err(KernelError::validation(
                "at least one of site_id or work_order_id is required",
            )
            .into());
        }
        let org = current_org().map_err(KernelError::from)?;
        with_audits::<_, TicketSummary, PgSupportError>(&self.pool, org, |tx| {
            Box::pin(async move {
                let ticket = lock_ticket_tx(tx, command.ticket_id).await?;
                if ticket.status == TicketStatus::Closed {
                    return Err(
                        KernelError::conflict("links cannot change on a closed ticket").into(),
                    );
                }

                // Resolve the target site, confined to the principal's branch
                // scope (deny-by-omission: an out-of-scope site is a 404).
                let (new_site, linked_site_branch, new_customer) = match command.site_id {
                    None => (ticket.site_id, None, ticket.customer_id),
                    Some(None) => (None, None, None),
                    Some(Some(site_id)) => {
                        let mut builder = QueryBuilder::<Postgres>::new(
                            "SELECT branch_id, customer_id FROM registry_sites WHERE id = ",
                        );
                        builder.push_bind(*site_id.as_uuid());
                        builder.push(" AND (");
                        push_site_scope(&mut builder, &command.branch_scope);
                        builder.push(")");
                        let row = builder
                            .build()
                            .fetch_optional(tx.as_mut())
                            .await?
                            .ok_or_else(|| KernelError::not_found("site was not found"))?;
                        (
                            Some(site_id),
                            Some(BranchId::from_uuid(row.try_get("branch_id")?)),
                            Some(CustomerId::from_uuid(row.try_get("customer_id")?)),
                        )
                    }
                };

                // A linked work order must be dispatched to the ticket's site
                // (referential guardrail; the 0194 CHECK is the backstop). The
                // lookup is scope-confined like the site lookup: an
                // out-of-scope work order is a 404, never a 409 existence leak.
                let new_work_order = match command.work_order_id {
                    None => ticket.work_order_id,
                    Some(None) => None,
                    Some(Some(work_order_id)) => {
                        let mut builder = QueryBuilder::<Postgres>::new(
                            "SELECT site_id FROM work_orders WHERE id = ",
                        );
                        builder.push_bind(*work_order_id.as_uuid());
                        builder.push(" AND (");
                        push_site_scope(&mut builder, &command.branch_scope);
                        builder.push(")");
                        let row = builder
                            .build()
                            .fetch_optional(tx.as_mut())
                            .await?
                            .ok_or_else(|| KernelError::not_found("work order was not found"))?;
                        let work_order_site = SiteId::from_uuid(row.try_get("site_id")?);
                        if new_site != Some(work_order_site) {
                            return Err(KernelError::conflict(
                                "work order is not dispatched to the ticket's linked site",
                            )
                            .into());
                        }
                        Some(work_order_id)
                    }
                };
                if new_work_order.is_some() && new_site.is_none() {
                    return Err(
                        KernelError::conflict("a work-order link requires a site link").into(),
                    );
                }

                // Branch: linking a site to an untriaged ticket IS triage; a
                // triaged ticket can only link sites of its own branch.
                let new_branch = match (ticket.branch_id, linked_site_branch) {
                    (Some(branch), Some(site_branch)) => {
                        if site_branch != branch {
                            return Err(KernelError::conflict(
                                "site belongs to a different branch than the ticket",
                            )
                            .into());
                        }
                        Some(branch)
                    }
                    (None, Some(site_branch)) => Some(site_branch),
                    (branch, None) => branch,
                };

                sqlx::query(
                    r#"
                    UPDATE support_tickets
                    SET site_id = $2,
                        customer_id = $3,
                        work_order_id = $4,
                        branch_id = $5,
                        updated_at = $6
                    WHERE id = $1
                    "#,
                )
                .bind(*command.ticket_id.as_uuid())
                .bind(new_site.map(|id| *id.as_uuid()))
                .bind(new_customer.map(|id| *id.as_uuid()))
                .bind(new_work_order.map(|id| *id.as_uuid()))
                .bind(new_branch.map(|id| *id.as_uuid()))
                .bind(command.occurred_at)
                .execute(tx.as_mut())
                .await?;

                let summary = fetch_summary_tx(tx, command.ticket_id).await?;
                let event = support_audit_event(
                    "support.ticket.linked",
                    Some(command.actor),
                    new_branch,
                    "support_ticket",
                    command.ticket_id,
                    command.trace.clone(),
                    command.occurred_at,
                )?
                .with_snapshots(
                    Some(serde_json::json!({
                        "site_id": ticket.site_id.map(|id| id.to_string()),
                        "customer_id": ticket.customer_id.map(|id| id.to_string()),
                        "work_order_id": ticket.work_order_id.map(|id| id.to_string()),
                        "branch_id": ticket.branch_id.map(|id| id.to_string()),
                    })),
                    Some(serde_json::json!({
                        "site_id": new_site.map(|id| id.to_string()),
                        "customer_id": new_customer.map(|id| id.to_string()),
                        "work_order_id": new_work_order.map(|id| id.to_string()),
                        "branch_id": new_branch.map(|id| id.to_string()),
                    })),
                )
                .with_org(org);
                Ok((summary, vec![event]))
            })
        })
        .await
    }

    // -----------------------------------------------------------------------
    // record_acceptance (audited closure evidence; idempotent)
    // -----------------------------------------------------------------------
    /// Record the customer's acceptance verdict for a RESOLVED ticket and drive
    /// the existing FSM edge (accept ⇒ CLOSED, decline ⇒ reopen IN_PROGRESS +
    /// customer-visible comment). Sibling-pilot idempotency semantics: a replay
    /// with the same `Idempotency-Key` and request returns the stored
    /// acceptance (`replayed = true`); reuse with a different request is a
    /// conflict. Returns `(view, notifications, replayed)`.
    pub async fn record_acceptance(
        &self,
        command: RecordAcceptanceCommand,
    ) -> Result<(TicketAcceptanceView, Vec<TicketNotification>, bool), PgSupportError> {
        let accepted_by = require_non_empty(&command.accepted_by, "accepted_by is required")?;
        require_max_chars(
            &accepted_by,
            MAX_ACCEPTED_BY_CHARS,
            "accepted_by is too long",
        )?;
        let note = match command.note.as_deref().map(str::trim) {
            Some("") | None => None,
            Some(trimmed) => {
                require_max_chars(
                    trimmed,
                    MAX_ACCEPTANCE_NOTE_CHARS,
                    "acceptance note is too long",
                )?;
                Some(trimmed.to_owned())
            }
        };
        // Fail-closed form (§4-27): a decline reopens the ticket, and the
        // customer-visible comment carrying the customer's reason is the note —
        // so a decline without a note is rejected, not silently commentless.
        if command.kind == AcceptanceKind::CustomerDeclined && note.is_none() {
            return Err(KernelError::validation(
                "a declined acceptance requires a note with the customer's reason",
            )
            .into());
        }
        let key = command.idempotency_key.trim().to_owned();
        // Character count, not bytes — the 0194 CHECK is char_length, so a
        // multibyte key must fail here as validation, not later as a 500.
        let key_chars = key.chars().count();
        if !(16..=200).contains(&key_chars) {
            return Err(
                KernelError::validation("Idempotency-Key must be 16..=200 characters").into(),
            );
        }
        let fingerprint = acceptance_fingerprint(
            command.ticket_id,
            command.kind,
            command.channel,
            &accepted_by,
            note.as_deref(),
        );
        let org = current_org().map_err(KernelError::from)?;
        let org_uuid = *org.as_uuid();
        let acceptance_id = uuid::Uuid::new_v4();

        with_audits::<_, (TicketAcceptanceView, Vec<TicketNotification>, bool), PgSupportError>(
            &self.pool,
            org,
            |tx| {
                Box::pin(async move {
                    // Idempotent replay: same key + same request returns the
                    // stored acceptance without re-transitioning or re-auditing.
                    let replay_sql = format!("{ACCEPTANCE_SELECT} WHERE a.idempotency_key = $1");
                    if let Some(row) = sqlx::query(sqlx::AssertSqlSafe(replay_sql))
                        .bind(&key)
                        .fetch_optional(tx.as_mut())
                        .await?
                    {
                        let prior_fingerprint: String = row.try_get("request_fingerprint")?;
                        let view = acceptance_from_row(&row)?;
                        if prior_fingerprint != fingerprint || view.ticket_id != command.ticket_id {
                            return Err(KernelError::conflict(
                                "idempotency key was reused with a different request",
                            )
                            .into());
                        }
                        return Ok(((view, Vec::new(), true), Vec::new()));
                    }

                    let ticket = lock_ticket_tx(tx, command.ticket_id).await?;
                    if ticket.status != TicketStatus::Resolved {
                        return Err(KernelError::conflict(
                            "acceptance can only be recorded on a RESOLVED ticket",
                        )
                        .into());
                    }
                    // FSM enforcement stays in the pure domain: acceptance only
                    // drives the existing RESOLVED edges.
                    let transition = ticket
                        .status
                        .transition_to(command.kind.transition_target())?;

                    sqlx::query(
                        r#"
                        INSERT INTO support_ticket_acceptances (
                            id, org_id, ticket_id, kind, channel, accepted_by, note,
                            recorded_by_user_id, occurred_at, created_at,
                            idempotency_key, request_fingerprint
                        )
                        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $9, $10, $11)
                        "#,
                    )
                    .bind(acceptance_id)
                    .bind(org_uuid)
                    .bind(*command.ticket_id.as_uuid())
                    .bind(command.kind.as_db_str())
                    .bind(command.channel.as_db_str())
                    .bind(&accepted_by)
                    .bind(note.as_deref())
                    .bind(*command.actor.as_uuid())
                    .bind(command.occurred_at)
                    .bind(&key)
                    .bind(&fingerprint)
                    .execute(tx.as_mut())
                    .await?;

                    let resolved_at =
                        resolved_timestamp(ticket.resolved_at, transition.to, command.occurred_at);
                    let closed_at =
                        closed_timestamp(ticket.closed_at, transition.to, command.occurred_at);
                    sqlx::query(
                        r#"
                        UPDATE support_tickets
                        SET status = $2,
                            resolved_at = $3,
                            closed_at = $4,
                            updated_at = $5
                        WHERE id = $1
                        "#,
                    )
                    .bind(*command.ticket_id.as_uuid())
                    .bind(transition.to.as_db_str())
                    .bind(resolved_at)
                    .bind(closed_at)
                    .bind(command.occurred_at)
                    .execute(tx.as_mut())
                    .await?;

                    // `accepted_by` is a business fact (like requester_name):
                    // stored in the row, never copied into audit snapshots.
                    let acceptance_event = support_audit_event(
                        "support.ticket.acceptance",
                        Some(command.actor),
                        ticket.branch_id,
                        "support_ticket_acceptance",
                        acceptance_id,
                        command.trace.clone(),
                        command.occurred_at,
                    )?
                    .with_snapshots(
                        None,
                        Some(serde_json::json!({
                            "ticket_id": command.ticket_id.to_string(),
                            "kind": command.kind.as_db_str(),
                            "channel": command.channel.as_db_str(),
                            "status_from": transition.from.as_db_str(),
                            "status_to": transition.to.as_db_str(),
                        })),
                    )
                    .with_org(org);
                    let transition_event = support_audit_event(
                        "support.ticket.transition",
                        Some(command.actor),
                        ticket.branch_id,
                        "support_ticket",
                        command.ticket_id,
                        command.trace.clone(),
                        command.occurred_at,
                    )?
                    .with_snapshots(
                        Some(serde_json::json!({ "status": transition.from.as_db_str() })),
                        Some(serde_json::json!({ "status": transition.to.as_db_str() })),
                    )
                    .with_org(org);
                    let mut events = vec![acceptance_event, transition_event];

                    // Decline: the note becomes a customer-visible comment so
                    // the requester sees why the ticket reopened.
                    if command.kind == AcceptanceKind::CustomerDeclined
                        && let Some(note) = note.as_deref()
                    {
                        let comment_id = SupportTicketCommentId::new();
                        sqlx::query(
                            r#"
                            INSERT INTO support_ticket_comments (
                                id, ticket_id, author_user_id, body,
                                is_internal_note, created_at, org_id
                            )
                            VALUES ($1, $2, $3, $4, FALSE, $5, $6)
                            "#,
                        )
                        .bind(*comment_id.as_uuid())
                        .bind(*command.ticket_id.as_uuid())
                        .bind(*command.actor.as_uuid())
                        .bind(note)
                        .bind(command.occurred_at)
                        .bind(org_uuid)
                        .execute(tx.as_mut())
                        .await?;
                        events.push(
                            support_audit_event(
                                "support.ticket.comment",
                                Some(command.actor),
                                ticket.branch_id,
                                "support_ticket_comment",
                                comment_id,
                                command.trace.clone(),
                                command.occurred_at,
                            )?
                            .with_snapshots(
                                None,
                                Some(serde_json::json!({
                                    "ticket_id": command.ticket_id.to_string(),
                                    "is_internal_note": false,
                                })),
                            )
                            .with_org(org),
                        );
                    }

                    let view = fetch_acceptance_tx(tx, acceptance_id).await?;
                    let notifications =
                        status_change_notifications(&ticket, command.ticket_id, transition.to);
                    Ok(((view, notifications, false), events))
                })
            },
        )
        .await
    }
}

// ---------------------------------------------------------------------------
// Locked-row model + tx helpers
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct LockedTicket {
    branch_id: Option<BranchId>,
    origin: TicketOrigin,
    status: TicketStatus,
    requester_user_id: Option<UserId>,
    assignee_user_id: Option<UserId>,
    resolved_at: Option<OffsetDateTime>,
    closed_at: Option<OffsetDateTime>,
    site_id: Option<SiteId>,
    customer_id: Option<CustomerId>,
    work_order_id: Option<WorkOrderId>,
}

async fn lock_ticket_tx(
    tx: &mut Transaction<'_, Postgres>,
    ticket_id: SupportTicketId,
) -> Result<LockedTicket, PgSupportError> {
    let row = sqlx::query(
        r#"
        SELECT branch_id, origin, status, requester_user_id, assignee_user_id,
               resolved_at, closed_at, site_id, customer_id, work_order_id
        FROM support_tickets
        WHERE id = $1
        FOR UPDATE
        "#,
    )
    .bind(*ticket_id.as_uuid())
    .fetch_one(tx.as_mut())
    .await?;

    let origin_raw: String = row.try_get("origin")?;
    let status_raw: String = row.try_get("status")?;
    Ok(LockedTicket {
        branch_id: row
            .try_get::<Option<uuid::Uuid>, _>("branch_id")?
            .map(BranchId::from_uuid),
        origin: TicketOrigin::from_db_str(&origin_raw)?,
        status: TicketStatus::from_db_str(&status_raw)?,
        requester_user_id: row
            .try_get::<Option<uuid::Uuid>, _>("requester_user_id")?
            .map(UserId::from_uuid),
        assignee_user_id: row
            .try_get::<Option<uuid::Uuid>, _>("assignee_user_id")?
            .map(UserId::from_uuid),
        resolved_at: row.try_get("resolved_at")?,
        closed_at: row.try_get("closed_at")?,
        site_id: row
            .try_get::<Option<uuid::Uuid>, _>("site_id")?
            .map(SiteId::from_uuid),
        customer_id: row
            .try_get::<Option<uuid::Uuid>, _>("customer_id")?
            .map(CustomerId::from_uuid),
        work_order_id: row
            .try_get::<Option<uuid::Uuid>, _>("work_order_id")?
            .map(WorkOrderId::from_uuid),
    })
}

async fn ensure_active_user_in_branch(
    tx: &mut Transaction<'_, Postgres>,
    user_id: UserId,
    branch_id: BranchId,
) -> Result<(), PgSupportError> {
    let valid: bool = sqlx::query_scalar(
        r#"
        SELECT EXISTS (
            SELECT 1
            FROM users u
            JOIN user_branches ub ON ub.user_id = u.id
            WHERE u.id = $1
              AND ub.branch_id = $2
              AND u.is_active = TRUE
        )
        "#,
    )
    .bind(*user_id.as_uuid())
    .bind(*branch_id.as_uuid())
    .fetch_one(tx.as_mut())
    .await?;
    if valid {
        Ok(())
    } else {
        Err(
            KernelError::validation("support ticket user must be an active member of the branch")
                .into(),
        )
    }
}

/// A comment author must be an active staff user able to act on the ticket:
/// either a cross-branch admin, or a member of the ticket's branch. Branch-less
/// customer tickets accept any active staff user (triage stage).
async fn ensure_author_visible_to_ticket(
    tx: &mut Transaction<'_, Postgres>,
    author: UserId,
    ticket: &LockedTicket,
) -> Result<(), PgSupportError> {
    match ticket.branch_id {
        Some(branch) => ensure_active_user_in_branch(tx, author, branch).await,
        None => {
            let active: bool = sqlx::query_scalar(
                "SELECT EXISTS (SELECT 1 FROM users WHERE id = $1 AND is_active = TRUE)",
            )
            .bind(*author.as_uuid())
            .fetch_one(tx.as_mut())
            .await?;
            if active {
                Ok(())
            } else {
                Err(KernelError::validation("comment author must be an active user").into())
            }
        }
    }
}

async fn fetch_summary_tx(
    tx: &mut Transaction<'_, Postgres>,
    ticket_id: SupportTicketId,
) -> Result<TicketSummary, PgSupportError> {
    // Composed from compile-time const fragments only — audit-sound.
    let sql = format!("{TICKET_SUMMARY_SELECT} WHERE id = $1");
    let row = sqlx::query(sqlx::AssertSqlSafe(sql))
        .bind(*ticket_id.as_uuid())
        .fetch_one(tx.as_mut())
        .await?;
    summary_from_row(&row)
}

async fn fetch_comment_tx(
    tx: &mut Transaction<'_, Postgres>,
    comment_id: SupportTicketCommentId,
) -> Result<CommentView, PgSupportError> {
    let row = sqlx::query(
        r#"
        SELECT id, ticket_id, author_user_id, body, is_internal_note, created_at,
            (SELECT u.display_name FROM users u
              WHERE u.id = support_ticket_comments.author_user_id) AS author_name
        FROM support_ticket_comments
        WHERE id = $1
        "#,
    )
    .bind(*comment_id.as_uuid())
    .fetch_one(tx.as_mut())
    .await?;
    comment_from_row(&row)
}

/// Resolve the `(created_at, id)` keyset coordinates for a cursor ticket so
/// `list_tickets` can page strictly after it. Mirrors messenger's
/// `message_cursor`. An unknown cursor is a not-found.
async fn ticket_cursor(
    pool: &PgPool,
    ticket_id: SupportTicketId,
) -> Result<(OffsetDateTime, uuid::Uuid), PgSupportError> {
    let org = current_org().map_err(KernelError::from)?;
    let row = with_org_conn::<_, _, PgSupportError>(pool, org, move |tx| {
        Box::pin(async move {
            Ok(
                sqlx::query("SELECT created_at, id FROM support_tickets WHERE id = $1")
                    .bind(*ticket_id.as_uuid())
                    .fetch_optional(tx.as_mut())
                    .await?,
            )
        })
    })
    .await?
    .ok_or_else(|| KernelError::not_found("support ticket cursor was not found"))?;
    Ok((row.try_get("created_at")?, row.try_get("id")?))
}

// ---------------------------------------------------------------------------
// Pure helpers
// ---------------------------------------------------------------------------

/// Max-length bounds (in characters) for unauthenticated-intake and internal
/// ticket free-text fields, enforced server-side so the public intake channel
/// cannot store unbounded blobs.
pub const MAX_TITLE_CHARS: usize = 200;
pub const MAX_BODY_CHARS: usize = 8000;
pub const MAX_REQUESTER_NAME_CHARS: usize = 200;
pub const MAX_REQUESTER_CONTACT_CHARS: usize = 200;
/// Acceptance bounds mirror the 0194 CHECK constraints exactly.
pub const MAX_ACCEPTED_BY_CHARS: usize = 200;
pub const MAX_ACCEPTANCE_NOTE_CHARS: usize = 2000;

fn require_non_empty(value: &str, message: &'static str) -> Result<String, PgSupportError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        Err(KernelError::validation(message).into())
    } else {
        Ok(trimmed.to_owned())
    }
}

/// Reject a field that exceeds `max` characters (Unicode scalar values, not
/// bytes). Used after [`require_non_empty`] so empty/whitespace is caught first.
fn require_max_chars(value: &str, max: usize, message: &'static str) -> Result<(), PgSupportError> {
    if value.chars().count() > max {
        Err(KernelError::validation(message).into())
    } else {
        Ok(())
    }
}

/// Default page size for [`PgSupportStore::list_tickets`] when the caller sends
/// no limit. The clamp in [`normalized_limit`] guarantees a hard server-side cap
/// regardless of the requested value.
const DEFAULT_LIST_LIMIT: i64 = 50;

/// Clamp a requested page size to `1..=100`, mirroring the messenger adapter, so
/// `list_tickets` can never issue an unbounded fetch.
fn normalized_limit(limit: Option<i64>) -> i64 {
    limit.unwrap_or(DEFAULT_LIST_LIMIT).clamp(1, 100)
}

/// Set `resolved_at` the first time a ticket enters RESOLVED; otherwise preserve
/// the existing value.
fn resolved_timestamp(
    existing: Option<OffsetDateTime>,
    to: TicketStatus,
    now: OffsetDateTime,
) -> Option<OffsetDateTime> {
    match to {
        TicketStatus::Resolved => existing.or(Some(now)),
        _ => existing,
    }
}

/// Set `closed_at` when a ticket enters CLOSED; otherwise preserve.
fn closed_timestamp(
    existing: Option<OffsetDateTime>,
    to: TicketStatus,
    now: OffsetDateTime,
) -> Option<OffsetDateTime> {
    match to {
        TicketStatus::Closed => existing.or(Some(now)),
        _ => existing,
    }
}

/// Status-change notifications: the assignee always, plus the internal requester
/// (customer requesters are not staff push recipients). The acting user is not
/// notified twice — but the actor isn't known here, so dedup is by recipient set.
fn status_change_notifications(
    ticket: &LockedTicket,
    ticket_id: SupportTicketId,
    to: TicketStatus,
) -> Vec<TicketNotification> {
    let mut recipients: Vec<UserId> = Vec::new();
    if let Some(assignee) = ticket.assignee_user_id {
        recipients.push(assignee);
    }
    if ticket.origin == TicketOrigin::Internal
        && let Some(requester) = ticket.requester_user_id
        && !recipients.contains(&requester)
    {
        recipients.push(requester);
    }
    recipients
        .into_iter()
        .map(|recipient| {
            TicketNotification::new(
                ticket_id,
                recipient,
                TicketNotificationKind::StatusChanged,
                format!("Support ticket status changed to {}.", to.as_db_str()),
            )
        })
        .collect()
}

/// Comment notifications for a non-internal comment: requester (if internal) and
/// assignee, excluding the comment author.
fn comment_notifications(
    ticket: &LockedTicket,
    ticket_id: SupportTicketId,
    author: UserId,
) -> Vec<TicketNotification> {
    let mut recipients: Vec<UserId> = Vec::new();
    if let Some(assignee) = ticket.assignee_user_id
        && assignee != author
    {
        recipients.push(assignee);
    }
    if ticket.origin == TicketOrigin::Internal
        && let Some(requester) = ticket.requester_user_id
        && requester != author
        && !recipients.contains(&requester)
    {
        recipients.push(requester);
    }
    recipients
        .into_iter()
        .map(|recipient| {
            TicketNotification::new(
                ticket_id,
                recipient,
                TicketNotificationKind::Commented,
                "A new reply was added to a support ticket.",
            )
        })
        .collect()
}

/// Branch-scope predicate. Untriaged (`branch_id IS NULL`) visibility is a
/// CROSS-BRANCH privilege: only a `BranchScope::All` principal ever sees
/// branch-less customer intake, and only when `include_untriaged` is set.
/// Branch-scoped principals never match NULL-branch rows regardless of the flag,
/// so the invariant is enforced here at the data layer, not just in REST.
fn push_branch_scope(
    builder: &mut QueryBuilder<Postgres>,
    branch_scope: &BranchScope,
    include_untriaged: bool,
) {
    match branch_scope {
        BranchScope::All => {
            if include_untriaged {
                // Every ticket, including untriaged intake.
                builder.push("TRUE");
            } else {
                // Cross-branch rollup, but exclude the untriaged queue.
                builder.push("branch_id IS NOT NULL");
            }
        }
        BranchScope::Branches(branches) if branches.is_empty() => {
            builder.push("FALSE");
        }
        BranchScope::Branches(branches) => {
            let branch_ids = branches
                .iter()
                .map(|branch_id| *branch_id.as_uuid())
                .collect::<Vec<_>>();
            builder.push("branch_id = ANY(");
            builder.push_bind(branch_ids);
            builder.push(")");
        }
    }
}

/// Branch-scope predicate for `registry_sites` / `work_orders` reads. Those
/// rows always carry a branch (NOT NULL), so unlike [`push_branch_scope`] there
/// is no untriaged escape hatch: `All` sees every row, a branch list sees
/// exactly its branches, and an empty list sees nothing (fail-closed).
fn push_site_scope(builder: &mut QueryBuilder<Postgres>, branch_scope: &BranchScope) {
    match branch_scope {
        BranchScope::All => {
            builder.push("TRUE");
        }
        BranchScope::Branches(branches) if branches.is_empty() => {
            builder.push("FALSE");
        }
        BranchScope::Branches(branches) => {
            let branch_ids = branches
                .iter()
                .map(|branch_id| *branch_id.as_uuid())
                .collect::<Vec<_>>();
            builder.push("branch_id = ANY(");
            builder.push_bind(branch_ids);
            builder.push(")");
        }
    }
}

/// Escape LIKE metacharacters so a user literally searching `100%` or `A_1`
/// matches those characters instead of wildcarding. Pair with `ESCAPE '\'`.
fn escape_like(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

/// Resolve the keyset cursor for [`PgSupportStore::list_field_sites`] to its
/// `(site_name, site_id)` sort key, confined to the principal's scope: an
/// unknown or out-of-scope cursor is a not-found, never an information leak.
async fn field_site_cursor(
    pool: &PgPool,
    cursor_id: SiteId,
    branch_scope: &BranchScope,
) -> Result<(String, uuid::Uuid), PgSupportError> {
    let mut builder = QueryBuilder::<Postgres>::new("SELECT name FROM registry_sites WHERE id = ");
    builder.push_bind(*cursor_id.as_uuid());
    builder.push(" AND (");
    push_site_scope(&mut builder, branch_scope);
    builder.push(")");
    let org = current_org().map_err(KernelError::from)?;
    let row = with_org_conn::<_, _, PgSupportError>(pool, org, move |tx| {
        Box::pin(async move { Ok(builder.build().fetch_optional(tx.as_mut()).await?) })
    })
    .await?
    .ok_or_else(|| KernelError::not_found("cursor was not found"))?;
    Ok((row.try_get("name")?, *cursor_id.as_uuid()))
}

/// Push the inner per-site aggregation of the field overview: one row per
/// in-scope `registry_sites` row with correlated same-org counts over
/// `support_tickets` / `work_orders` / `site_attendance_events` (documented
/// cross-crate READS; all RLS-scoped to `app.current_org`). Emits a complete
/// SELECT (with WHERE) so callers can wrap it as a subquery for the derived
/// SLA filter and pagination.
fn push_field_sites_inner(
    builder: &mut QueryBuilder<Postgres>,
    branch_scope: &BranchScope,
    needle: Option<&str>,
    customer_id: Option<CustomerId>,
    cursor: Option<&(String, uuid::Uuid)>,
) {
    builder.push(format!(
        "SELECT id AS site_id, name AS site_name, branch_id, customer_id, \
         (SELECT rc.name FROM registry_customers rc \
           WHERE rc.id = registry_sites.customer_id) AS customer_name, \
         address, latitude, longitude, \
         (SELECT COUNT(*) FROM support_tickets t WHERE t.site_id = registry_sites.id \
           AND t.status IN ('OPEN','IN_PROGRESS','ON_HOLD')) AS open_ticket_count, \
         (SELECT COUNT(*) FROM support_tickets t WHERE t.site_id = registry_sites.id \
           AND t.status IN ('OPEN','IN_PROGRESS','ON_HOLD') \
           AND t.due_at < now()) AS breached_ticket_count, \
         (SELECT MIN(t.due_at) FROM support_tickets t WHERE t.site_id = registry_sites.id \
           AND t.status IN ('OPEN','IN_PROGRESS','ON_HOLD')) AS next_due_at, \
         (SELECT COUNT(*) FROM work_orders w WHERE w.site_id = registry_sites.id \
           AND w.status NOT IN {WORK_ORDER_TERMINAL_STATUSES}) AS active_work_order_count, \
         (SELECT MAX(e.occurred_at) FROM site_attendance_events e \
           WHERE e.site_id = registry_sites.id \
           AND e.kind = 'ARRIVAL') AS last_arrival_at \
         FROM registry_sites WHERE ("
    ));
    push_site_scope(builder, branch_scope);
    builder.push(")");
    if let Some(needle) = needle {
        let pattern = format!("%{needle}%");
        builder.push(" AND (name ILIKE ");
        builder.push_bind(pattern.clone());
        builder.push(
            " ESCAPE '\\' OR EXISTS (SELECT 1 FROM registry_customers rc \
             WHERE rc.id = registry_sites.customer_id AND rc.name ILIKE ",
        );
        builder.push_bind(pattern);
        builder.push(" ESCAPE '\\'))");
    }
    if let Some(customer) = customer_id {
        builder.push(" AND customer_id = ");
        builder.push_bind(*customer.as_uuid());
    }
    if let Some((cursor_name, cursor_id)) = cursor {
        // Keyset: strictly after the cursor on the (name, id) ascending order.
        builder.push(" AND (name, id) > (");
        builder.push_bind(cursor_name.clone());
        builder.push(", ");
        builder.push_bind(*cursor_id);
        builder.push(")");
    }
}

fn field_site_row_from_row(row: &sqlx::postgres::PgRow) -> Result<FieldSiteRow, PgSupportError> {
    let sla_raw: String = row.try_get("sla")?;
    Ok(FieldSiteRow {
        site_id: SiteId::from_uuid(row.try_get("site_id")?),
        site_name: row.try_get("site_name")?,
        branch_id: BranchId::from_uuid(row.try_get("branch_id")?),
        customer_id: CustomerId::from_uuid(row.try_get("customer_id")?),
        customer_name: row.try_get("customer_name")?,
        address: row.try_get("address")?,
        latitude: row.try_get("latitude")?,
        longitude: row.try_get("longitude")?,
        open_ticket_count: row.try_get("open_ticket_count")?,
        breached_ticket_count: row.try_get("breached_ticket_count")?,
        next_due_at: row.try_get("next_due_at")?,
        active_work_order_count: row.try_get("active_work_order_count")?,
        last_arrival_at: row.try_get("last_arrival_at")?,
        sla: FieldSlaState::from_db_str(&sla_raw)?,
    })
}

fn work_order_ref_from_row(
    row: &sqlx::postgres::PgRow,
) -> Result<FieldWorkOrderRef, PgSupportError> {
    Ok(FieldWorkOrderRef {
        id: WorkOrderId::from_uuid(row.try_get("id")?),
        request_no: row.try_get("request_no")?,
        status: row.try_get("status")?,
        priority: row.try_get("priority")?,
        target_due_at: row.try_get("target_due_at")?,
        report_submitted_at: row.try_get("report_submitted_at")?,
        result_type: row.try_get("result_type")?,
        created_at: row.try_get("created_at")?,
    })
}

fn attendance_event_from_row(
    row: &sqlx::postgres::PgRow,
) -> Result<FieldAttendanceEvent, PgSupportError> {
    Ok(FieldAttendanceEvent {
        user_id: UserId::from_uuid(row.try_get("user_id")?),
        user_name: row.try_get("user_name")?,
        work_order_id: WorkOrderId::from_uuid(row.try_get("work_order_id")?),
        kind: row.try_get("kind")?,
        occurred_at: row.try_get("occurred_at")?,
    })
}

fn acceptance_from_row(
    row: &sqlx::postgres::PgRow,
) -> Result<TicketAcceptanceView, PgSupportError> {
    let kind_raw: String = row.try_get("kind")?;
    let channel_raw: String = row.try_get("channel")?;
    Ok(TicketAcceptanceView {
        id: row.try_get("id")?,
        ticket_id: SupportTicketId::from_uuid(row.try_get("ticket_id")?),
        kind: AcceptanceKind::from_db_str(&kind_raw)?,
        channel: AcceptanceChannel::from_db_str(&channel_raw)?,
        accepted_by: row.try_get("accepted_by")?,
        note: row.try_get("note")?,
        recorded_by_user_id: UserId::from_uuid(row.try_get("recorded_by_user_id")?),
        recorded_by_name: row.try_get("recorded_by_name")?,
        occurred_at: row.try_get("occurred_at")?,
    })
}

async fn fetch_acceptance_tx(
    tx: &mut Transaction<'_, Postgres>,
    acceptance_id: uuid::Uuid,
) -> Result<TicketAcceptanceView, PgSupportError> {
    // Composed from compile-time const fragments only — audit-sound.
    let sql = format!("{ACCEPTANCE_SELECT} WHERE a.id = $1");
    let row = sqlx::query(sqlx::AssertSqlSafe(sql))
        .bind(acceptance_id)
        .fetch_one(tx.as_mut())
        .await?;
    acceptance_from_row(&row)
}

/// Canonical request fingerprint for acceptance idempotency: SHA-256 (lowercase
/// hex, matching the 0194 CHECK) over the semantic fields, unit-separated so no
/// two distinct requests can collide by concatenation. A replay must match this
/// exactly; a same-key request with a different fingerprint is a conflict.
fn acceptance_fingerprint(
    ticket_id: SupportTicketId,
    kind: AcceptanceKind,
    channel: AcceptanceChannel,
    accepted_by: &str,
    note: Option<&str>,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(ticket_id.to_string().as_bytes());
    hasher.update([0x1f]);
    hasher.update(kind.as_db_str().as_bytes());
    hasher.update([0x1f]);
    hasher.update(channel.as_db_str().as_bytes());
    hasher.update([0x1f]);
    hasher.update(accepted_by.as_bytes());
    hasher.update([0x1f]);
    hasher.update(note.unwrap_or_default().as_bytes());
    hex::encode(hasher.finalize())
}

fn summary_from_row(row: &sqlx::postgres::PgRow) -> Result<TicketSummary, PgSupportError> {
    let origin_raw: String = row.try_get("origin")?;
    let category_raw: String = row.try_get("category")?;
    let priority_raw: String = row.try_get("priority")?;
    let status_raw: String = row.try_get("status")?;
    Ok(TicketSummary {
        id: SupportTicketId::from_uuid(row.try_get("id")?),
        branch_id: row
            .try_get::<Option<uuid::Uuid>, _>("branch_id")?
            .map(BranchId::from_uuid),
        origin: TicketOrigin::from_db_str(&origin_raw)?,
        category: TicketCategory::from_db_str(&category_raw)?,
        priority: TicketPriority::from_db_str(&priority_raw)?,
        status: TicketStatus::from_db_str(&status_raw)?,
        title: row.try_get("title")?,
        requester_user_id: row
            .try_get::<Option<uuid::Uuid>, _>("requester_user_id")?
            .map(UserId::from_uuid),
        requester_name: row.try_get("requester_name")?,
        assignee_user_id: row
            .try_get::<Option<uuid::Uuid>, _>("assignee_user_id")?
            .map(UserId::from_uuid),
        assignee_name: row.try_get("assignee_name")?,
        due_at: row.try_get("due_at")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
        resolved_at: row.try_get("resolved_at")?,
        closed_at: row.try_get("closed_at")?,
        site_id: row
            .try_get::<Option<uuid::Uuid>, _>("site_id")?
            .map(SiteId::from_uuid),
        site_name: row.try_get("site_name")?,
        customer_id: row
            .try_get::<Option<uuid::Uuid>, _>("customer_id")?
            .map(CustomerId::from_uuid),
        customer_name: row.try_get("customer_name")?,
        work_order_id: row
            .try_get::<Option<uuid::Uuid>, _>("work_order_id")?
            .map(WorkOrderId::from_uuid),
    })
}

fn comment_from_row(row: &sqlx::postgres::PgRow) -> Result<CommentView, PgSupportError> {
    Ok(CommentView {
        id: SupportTicketCommentId::from_uuid(row.try_get("id")?),
        ticket_id: SupportTicketId::from_uuid(row.try_get("ticket_id")?),
        author_user_id: row
            .try_get::<Option<uuid::Uuid>, _>("author_user_id")?
            .map(UserId::from_uuid),
        author_name: row.try_get("author_name")?,
        body: row.try_get("body")?,
        is_internal_note: row.try_get("is_internal_note")?,
        created_at: row.try_get("created_at")?,
    })
}

#[cfg(test)]
mod tests {
    use super::{ErrorKind, MAX_BODY_CHARS, MAX_TITLE_CHARS, normalized_limit, require_max_chars};

    #[test]
    fn require_max_chars_accepts_at_the_boundary() {
        let at_limit = "x".repeat(MAX_TITLE_CHARS);
        assert!(require_max_chars(&at_limit, MAX_TITLE_CHARS, "too long").is_ok());
    }

    #[test]
    fn require_max_chars_rejects_over_the_boundary_with_validation() {
        let too_long = "x".repeat(MAX_TITLE_CHARS + 1);
        let err = require_max_chars(&too_long, MAX_TITLE_CHARS, "too long")
            .expect_err("over-limit value must be rejected");
        assert_eq!(err.kind(), ErrorKind::Validation);
    }

    #[test]
    fn require_max_chars_counts_unicode_scalars_not_bytes() {
        // Each Korean syllable is 3 bytes but 1 char; a string of MAX chars must
        // pass even though its byte length far exceeds the char limit.
        let korean = "가".repeat(MAX_BODY_CHARS);
        assert!(korean.len() > MAX_BODY_CHARS);
        assert!(require_max_chars(&korean, MAX_BODY_CHARS, "too long").is_ok());
        let over = "가".repeat(MAX_BODY_CHARS + 1);
        assert!(require_max_chars(&over, MAX_BODY_CHARS, "too long").is_err());
    }

    #[test]
    fn normalized_limit_clamps_and_defaults() {
        assert_eq!(normalized_limit(None), 50);
        assert_eq!(normalized_limit(Some(0)), 1);
        assert_eq!(normalized_limit(Some(-10)), 1);
        assert_eq!(normalized_limit(Some(50)), 50);
        assert_eq!(normalized_limit(Some(100)), 100);
        assert_eq!(normalized_limit(Some(1_000)), 100);
    }
}
