//! Postgres notifications adapter.
//!
//! Recipient scoping is enforced here in code (there is no per-person GUC): the
//! caller passes the authenticated principal's `UserId`, and every query
//! filters `recipient_user_id`. RLS narrows to the tenant on top of that. A
//! cross-user read or read-mark therefore returns *nothing* (or NotFound),
//! never another user's row.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

use std::sync::Arc;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use console_kernel_core::{
    AuditAction, AuditEvent, ErrorKind, KernelError, NotificationId, Timestamp, UserId,
};
use console_notifications_application::{
    DeleteNotificationPolicyCommand, EmitNotificationCommand, EmitNotificationFuture,
    ListNotificationObjectGroupsQuery, ListNotificationPoliciesQuery, ListNotificationsQuery,
    MarkAllNotificationsReadCommand, MarkNotificationReadCommand, MarkNotificationUnreadCommand,
    NotificationCategoryCount, NotificationCountsSummary, NotificationCountsSummaryQuery,
    NotificationCreatedNotification, NotificationNotifier, NotificationObjectGroup,
    NotificationObjectGroupPage, NotificationPage, NotificationPolicySummary, NotificationQuery,
    NotificationResolver, NotificationSink, NotificationSummary, ResolveNotificationsByLinkCommand,
    ResolveNotificationsFuture, UnreadNotificationCountQuery, UpsertNotificationPolicyCommand,
    notification_audit_event,
};
use console_notifications_domain::{
    NotificationBody, NotificationCategory, NotificationKind, NotificationLink,
    NotificationPolicyId,
};
use console_platform_db::{DbError, with_audit, with_audits, with_org_conn};
use console_platform_request_context::current_org;
use serde::{Deserialize, Serialize};
use sqlx::{PgPool, Postgres, QueryBuilder, Row};

/// §4-19 single chokepoint: THE one SQL predicate deciding whether a
/// `notifications` row is muted for its recipient. Reused verbatim by every
/// path that annotates or excludes on mute (list, by-object latest, counts,
/// summary, emit's realtime skip) so the routing semantics can never fork.
/// Binds against the unqualified `notifications` relation of the enclosing
/// statement (SELECT row, UPDATE/INSERT RETURNING row).
const MUTED_PREDICATE_SQL: &str = "EXISTS (\
    SELECT 1 FROM notification_policies p \
    WHERE p.org_id = notifications.org_id \
      AND p.user_id = notifications.recipient_user_id \
      AND p.action = 'mute' \
      AND (p.scope = 'all' \
        OR (p.scope = 'category' AND p.category = notifications.category) \
        OR (p.scope = 'object' AND p.link = notifications.link)))";

#[derive(Debug, thiserror::Error)]
pub enum PgNotificationError {
    #[error(transparent)]
    Db(#[from] DbError),

    #[error(transparent)]
    Domain(#[from] KernelError),

    /// Internal sentinel: a `dedup_key` INSERT lost the race to a concurrent
    /// emit. Never surfaced to callers — `emit_notification` catches it and
    /// returns the already-committed row.
    #[error("notification dedup conflict")]
    Dedup,
}

impl PgNotificationError {
    #[must_use]
    pub fn kind(&self) -> ErrorKind {
        match self {
            Self::Domain(err) => err.kind,
            Self::Db(DbError::Sqlx(sqlx::Error::RowNotFound)) => ErrorKind::NotFound,
            Self::Dedup | Self::Db(_) => ErrorKind::Internal,
        }
    }
}

impl From<sqlx::Error> for PgNotificationError {
    fn from(value: sqlx::Error) -> Self {
        Self::Db(DbError::Sqlx(value))
    }
}

impl From<PgNotificationError> for KernelError {
    fn from(value: PgNotificationError) -> Self {
        match value {
            PgNotificationError::Domain(err) => err,
            PgNotificationError::Dedup => {
                KernelError::internal("notification dedup conflict escaped emit")
            }
            PgNotificationError::Db(err) => KernelError::internal(err.to_string()),
        }
    }
}

#[derive(Clone)]
pub struct PgNotificationStore {
    pool: PgPool,
    notifier: Option<Arc<dyn NotificationNotifier>>,
}

impl std::fmt::Debug for PgNotificationStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PgNotificationStore")
            .field("pool", &self.pool)
            .field("has_notifier", &self.notifier.is_some())
            .finish()
    }
}

impl PgNotificationStore {
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self {
            pool,
            notifier: None,
        }
    }

    #[must_use]
    pub fn with_notifier(mut self, notifier: Arc<dyn NotificationNotifier>) -> Self {
        self.notifier = Some(notifier);
        self
    }

    #[must_use]
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    /// Emit a recipient-scoped notification. Validates the domain invariants,
    /// inserts one row (audited), and — only for a genuinely new row — fires the
    /// realtime notifier. A `dedup_key` redelivery is a no-op that returns the
    /// existing row without re-auditing or re-notifying.
    pub async fn emit_notification(
        &self,
        command: EmitNotificationCommand,
    ) -> Result<NotificationSummary, PgNotificationError> {
        let category = NotificationCategory::new(command.category)?;
        let kind = NotificationKind::new(command.kind)?;
        let body = NotificationBody::new(command.text)?;
        let link = NotificationLink::validated(command.link)?;
        let link_json = serde_json::to_value(&link).map_err(|err| {
            KernelError::internal(format!("notification link is not JSON: {err}"))
        })?;

        let org = current_org().map_err(KernelError::from)?;
        let org_uuid = *org.as_uuid();
        let notification_id = NotificationId::new();
        let recipient_uuid = *command.recipient.as_uuid();
        let dedup_key = command.dedup_key.clone();

        // Fast path for a redelivered event: return the existing row untouched.
        if let Some(key) = &dedup_key
            && let Some(existing) = self.find_by_dedup(org, command.recipient, key).await?
        {
            return Ok(existing);
        }

        let event = notification_audit_event(
            "notification.emit",
            command.actor,
            notification_id,
            command.trace,
            command.occurred_at,
        )?
        .with_org(org);

        let insert = {
            let category = category.into_string();
            let kind = kind.into_string();
            let body = body.into_string();
            let link_json = link_json.clone();
            let dedup_key = dedup_key.clone();
            with_audit::<_, Option<NotificationSummary>, PgNotificationError>(
                &self.pool,
                event,
                move |tx| {
                    Box::pin(async move {
                        let row = sqlx::query(sqlx::AssertSqlSafe(format!(
                            r#"
                            INSERT INTO notifications (
                                id, org_id, recipient_user_id, category, kind, body, link, dedup_key
                            )
                            VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
                            ON CONFLICT (org_id, recipient_user_id, dedup_key)
                                WHERE dedup_key IS NOT NULL DO NOTHING
                            RETURNING id, recipient_user_id, category, kind, body, link,
                                      unread, created_at, read_at, resolved_at,
                                      {MUTED_PREDICATE_SQL} AS muted
                            "#,
                        )))
                        .bind(notification_id.as_uuid())
                        .bind(org_uuid)
                        .bind(recipient_uuid)
                        .bind(category)
                        .bind(kind)
                        .bind(body)
                        .bind(link_json)
                        .bind(dedup_key)
                        .fetch_optional(tx.as_mut())
                        .await?;
                        // No row => a concurrent emit already committed this
                        // dedup_key. Roll back (no audit) via the sentinel; the
                        // caller reads the committed row back.
                        row.as_ref()
                            .map(summary_from_row)
                            .transpose()?
                            .map_or(Err(PgNotificationError::Dedup), |summary| Ok(Some(summary)))
                    })
                },
            )
            .await
        };

        let summary = match insert {
            Ok(Some(summary)) => summary,
            Ok(None) => unreachable!("insert closure returns Some or the Dedup sentinel"),
            Err(PgNotificationError::Dedup) => {
                // The sentinel is only ever returned on the ON CONFLICT path,
                // which requires a dedup_key; read the winner back.
                return match dedup_key {
                    Some(key) => self
                        .find_by_dedup(org, command.recipient, &key)
                        .await?
                        .ok_or_else(|| {
                            KernelError::internal("dedup conflict but no existing notification")
                                .into()
                        }),
                    None => Err(KernelError::internal("dedup sentinel without a dedup_key").into()),
                };
            }
            Err(other) => return Err(other),
        };

        // Routing per user policy: a muted row is persisted and audited like
        // any other (mute suppresses ATTENTION, never data) but the realtime
        // notifier stays silent — no toast, no badge push.
        if !summary.muted
            && let Some(notifier) = &self.notifier
        {
            notifier
                .notification_created(NotificationCreatedNotification {
                    notification_id: summary.id,
                    recipient_user_id: summary.recipient_user_id,
                })
                .await;
        }
        Ok(summary)
    }

    async fn find_by_dedup(
        &self,
        org: console_kernel_core::OrgId,
        recipient: UserId,
        dedup_key: &str,
    ) -> Result<Option<NotificationSummary>, PgNotificationError> {
        let recipient_uuid = *recipient.as_uuid();
        let dedup_key = dedup_key.to_owned();
        let row = with_org_conn::<_, _, PgNotificationError>(&self.pool, org, move |tx| {
            Box::pin(async move {
                Ok(sqlx::query(sqlx::AssertSqlSafe(format!(
                    r#"
                    SELECT id, recipient_user_id, category, kind, body, link,
                           unread, created_at, read_at, resolved_at,
                           {MUTED_PREDICATE_SQL} AS muted
                    FROM notifications
                    WHERE recipient_user_id = $1 AND dedup_key = $2
                    "#,
                )))
                .bind(recipient_uuid)
                .bind(dedup_key)
                .fetch_optional(tx.as_mut())
                .await?)
            })
        })
        .await?;
        row.as_ref().map(summary_from_row).transpose()
    }

    /// Re-read a single notification with the same recipient and mute rules as lists.
    pub async fn notification(
        &self,
        query: NotificationQuery,
    ) -> Result<Option<NotificationSummary>, PgNotificationError> {
        let org = current_org().map_err(KernelError::from)?;
        let row = with_org_conn::<_, _, PgNotificationError>(&self.pool, org, move |tx| {
            Box::pin(async move {
                sqlx::query("SET LOCAL statement_timeout = '5s'").execute(tx.as_mut()).await?;
                Ok(sqlx::query(sqlx::AssertSqlSafe(format!(
                    "SELECT id, recipient_user_id, category, kind, body, link, unread, created_at, read_at, resolved_at, {MUTED_PREDICATE_SQL} AS muted FROM notifications WHERE id = $1 AND recipient_user_id = $2"
                )))
                .bind(*query.notification_id.as_uuid())
                .bind(*query.recipient.as_uuid())
                .fetch_optional(tx.as_mut()).await?)
            })
        }).await?;
        row.as_ref().map(summary_from_row).transpose()
    }

    /// List the caller's notifications, newest first, keyset-paginated.
    pub async fn list(
        &self,
        query: ListNotificationsQuery,
    ) -> Result<NotificationPage, PgNotificationError> {
        let limit = query.limit.clamp(1, 200);
        let recipient_uuid = *query.recipient.as_uuid();
        let org = current_org().map_err(KernelError::from)?;

        let rows = with_org_conn::<_, _, PgNotificationError>(&self.pool, org, move |tx| {
            Box::pin(async move {
                let mut builder = QueryBuilder::<Postgres>::new(format!(
                    r#"
                    SELECT id, recipient_user_id, category, kind, body, link,
                           unread, created_at, read_at, resolved_at,
                           {MUTED_PREDICATE_SQL} AS muted
                    FROM notifications
                    WHERE recipient_user_id =
                    "#,
                ));
                builder.push_bind(recipient_uuid);
                if query.unread_only {
                    builder.push(" AND unread = true");
                }
                if let Some(before_id) = query.before_id {
                    // Keyset: strictly older than the cursor row. A cursor that
                    // is not the caller's own row makes the subquery empty, so
                    // the comparison is NULL and the page is empty (fail-closed).
                    builder.push(" AND (created_at, id) < (SELECT created_at, id FROM notifications WHERE id = ");
                    builder.push_bind(*before_id.as_uuid());
                    builder.push(" AND recipient_user_id = ");
                    builder.push_bind(recipient_uuid);
                    builder.push(")");
                }
                builder.push(" ORDER BY created_at DESC, id DESC LIMIT ");
                builder.push_bind(limit);
                Ok(builder.build().fetch_all(tx.as_mut()).await?)
            })
        })
        .await?;

        let items = rows
            .iter()
            .map(summary_from_row)
            .collect::<Result<Vec<_>, _>>()?;
        let next_cursor = (items.len() as i64 == limit)
            .then(|| items.last().map(|item| item.id))
            .flatten();
        Ok(NotificationPage { items, next_cursor })
    }

    /// Count the caller's unread notifications that WANT attention: rows
    /// suppressed by the caller's mute policies are excluded (badge truth =
    /// attention truth; the mute-suppressed tally travels on
    /// [`summary`](Self::summary) as `muted_unread`). The comms-rail badge
    /// needs an exact figure; paging the list and counting breaks past the
    /// page clamp. Recipient-scoped in code exactly like [`list`](Self::list);
    /// RLS narrows to the tenant on top, so another user's (or tenant's) rows
    /// never count.
    pub async fn unread_count(
        &self,
        query: UnreadNotificationCountQuery,
    ) -> Result<i64, PgNotificationError> {
        let recipient_uuid = *query.recipient.as_uuid();
        let org = current_org().map_err(KernelError::from)?;

        with_org_conn::<_, _, PgNotificationError>(&self.pool, org, move |tx| {
            Box::pin(async move {
                let count: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
                    r#"
                    SELECT COUNT(*)
                    FROM notifications
                    WHERE recipient_user_id = $1 AND unread = true
                      AND NOT {MUTED_PREDICATE_SQL}
                    "#,
                )))
                .bind(recipient_uuid)
                .fetch_one(tx.as_mut())
                .await?;
                Ok(count)
            })
        })
        .await
    }

    /// Mark one of the caller's notifications read. Returns NotFound when the id
    /// is unknown *or* owned by another user — the two are indistinguishable to
    /// the caller, which is the cross-user isolation guarantee.
    pub async fn mark_read(
        &self,
        command: MarkNotificationReadCommand,
    ) -> Result<NotificationSummary, PgNotificationError> {
        let org = current_org().map_err(KernelError::from)?;
        let recipient_uuid = *command.recipient.as_uuid();
        let notification_uuid = *command.notification_id.as_uuid();
        let occurred_at = command.occurred_at;
        let event = notification_audit_event(
            "notification.read",
            Some(command.recipient),
            command.notification_id,
            command.trace,
            command.occurred_at,
        )?
        .with_org(org);

        with_audit::<_, NotificationSummary, PgNotificationError>(&self.pool, event, move |tx| {
            Box::pin(async move {
                let row = sqlx::query(sqlx::AssertSqlSafe(format!(
                    r#"
                    UPDATE notifications
                    SET unread = false, read_at = COALESCE(read_at, $3)
                    WHERE id = $1 AND recipient_user_id = $2
                    RETURNING id, recipient_user_id, category, kind, body, link,
                              unread, created_at, read_at, resolved_at,
                              {MUTED_PREDICATE_SQL} AS muted
                    "#,
                )))
                .bind(notification_uuid)
                .bind(recipient_uuid)
                .bind(occurred_at)
                .fetch_optional(tx.as_mut())
                .await?;
                match row {
                    Some(row) => summary_from_row(&row),
                    None => Err(KernelError::not_found("notification not found").into()),
                }
            })
        })
        .await
    }

    /// Flip one of the caller's notifications back to unread — the reverse arc
    /// of [`mark_read`](Self::mark_read) (swipe/secondary action is a TOGGLE).
    /// `read_at` is deliberately left untouched: it stays the forensic
    /// first-read timestamp. Cross-user ids are NotFound, indistinguishable
    /// from absent.
    pub async fn mark_unread(
        &self,
        command: MarkNotificationUnreadCommand,
    ) -> Result<NotificationSummary, PgNotificationError> {
        let org = current_org().map_err(KernelError::from)?;
        let recipient_uuid = *command.recipient.as_uuid();
        let notification_uuid = *command.notification_id.as_uuid();
        let event = notification_audit_event(
            "notification.unread",
            Some(command.recipient),
            command.notification_id,
            command.trace,
            command.occurred_at,
        )?
        .with_org(org);

        with_audit::<_, NotificationSummary, PgNotificationError>(&self.pool, event, move |tx| {
            Box::pin(async move {
                let row = sqlx::query(sqlx::AssertSqlSafe(format!(
                    r#"
                    UPDATE notifications
                    SET unread = true
                    WHERE id = $1 AND recipient_user_id = $2
                    RETURNING id, recipient_user_id, category, kind, body, link,
                              unread, created_at, read_at, resolved_at,
                              {MUTED_PREDICATE_SQL} AS muted
                    "#,
                )))
                .bind(notification_uuid)
                .bind(recipient_uuid)
                .fetch_optional(tx.as_mut())
                .await?;
                match row {
                    Some(row) => summary_from_row(&row),
                    None => Err(KernelError::not_found("notification not found").into()),
                }
            })
        })
        .await
    }

    /// Mark all of the caller's unread notifications read. Returns the count.
    pub async fn mark_all_read(
        &self,
        command: MarkAllNotificationsReadCommand,
    ) -> Result<u64, PgNotificationError> {
        let org = current_org().map_err(KernelError::from)?;
        let recipient_uuid = *command.recipient.as_uuid();
        let occurred_at = command.occurred_at;
        let event = notification_audit_event(
            "notification.read_all",
            Some(command.recipient),
            command.recipient,
            command.trace,
            command.occurred_at,
        )?
        .with_org(org);

        with_audit::<_, u64, PgNotificationError>(&self.pool, event, move |tx| {
            Box::pin(async move {
                let result = sqlx::query(
                    r#"
                    UPDATE notifications
                    SET unread = false, read_at = COALESCE(read_at, $2)
                    WHERE recipient_user_id = $1 AND unread = true
                    "#,
                )
                .bind(recipient_uuid)
                .bind(occurred_at)
                .execute(tx.as_mut())
                .await?;
                Ok(result.rows_affected())
            })
        })
        .await
    }

    /// Per-category unread breakdown for the comms-rail badge, plus the total.
    /// `category` doubles as the "surface" grouping (결재/멘션/문서/공지/근태/급여,
    /// extensible) — a new producer category needs no schema change to appear
    /// here. Mute-suppressed rows are excluded from `total_unread` and
    /// `by_category` and tallied honestly as `muted_unread` instead.
    pub async fn summary(
        &self,
        query: NotificationCountsSummaryQuery,
    ) -> Result<NotificationCountsSummary, PgNotificationError> {
        let recipient_uuid = *query.recipient.as_uuid();
        let org = current_org().map_err(KernelError::from)?;

        let rows = with_org_conn::<_, _, PgNotificationError>(&self.pool, org, move |tx| {
            Box::pin(async move {
                Ok(sqlx::query(sqlx::AssertSqlSafe(format!(
                    r#"
                    SELECT category,
                           COUNT(*) FILTER (WHERE NOT {MUTED_PREDICATE_SQL}) AS unread,
                           COUNT(*) FILTER (WHERE {MUTED_PREDICATE_SQL}) AS muted_unread
                    FROM notifications
                    WHERE recipient_user_id = $1 AND unread = true
                    GROUP BY category
                    ORDER BY category
                    "#,
                )))
                .bind(recipient_uuid)
                .fetch_all(tx.as_mut())
                .await?)
            })
        })
        .await?;

        let mut by_category = Vec::with_capacity(rows.len());
        let mut muted_unread = 0i64;
        for row in &rows {
            let unread: i64 = row.try_get("unread")?;
            muted_unread += row.try_get::<i64, _>("muted_unread")?;
            // A category whose every unread row is muted has no attention to
            // ask for — it is absent from the breakdown, not shown as zero.
            if unread > 0 {
                by_category.push(NotificationCategoryCount {
                    category: row.try_get("category")?,
                    unread,
                });
            }
        }
        let total_unread = by_category.iter().map(|c| c.unread).sum();
        Ok(NotificationCountsSummary {
            total_unread,
            by_category,
            muted_unread,
        })
    }

    /// Mark every still-open notification pointing at `command.link` as
    /// resolved, in one audited sweep. Generic across recipients and
    /// producers — matches solely on the `link` shape. Returns the number of
    /// rows resolved (0 when nothing matched, which is not an error: a
    /// resolving event may race ahead of, or arrive after, the detector).
    pub async fn resolve_notifications_by_link(
        &self,
        command: ResolveNotificationsByLinkCommand,
    ) -> Result<u64, PgNotificationError> {
        let org = current_org().map_err(KernelError::from)?;
        let link = NotificationLink::validated(command.link)?;
        let link_json = serde_json::to_value(&link).map_err(|err| {
            KernelError::internal(format!("notification link is not JSON: {err}"))
        })?;
        let resolved_by_uuid = command.resolved_by.map(|id| *id.as_uuid());
        let occurred_at = command.occurred_at;

        let target = serde_json::to_string(&link_json)
            .unwrap_or_else(|_| "<unserializable link>".to_owned());
        let event = notification_audit_event(
            "notification.resolve",
            command.resolved_by,
            target,
            command.trace,
            command.occurred_at,
        )?
        .with_org(org);

        with_audit::<_, u64, PgNotificationError>(&self.pool, event, move |tx| {
            Box::pin(async move {
                let result = sqlx::query(
                    r#"
                    UPDATE notifications
                    SET resolved_at = $2, resolved_by = $3
                    WHERE org_id = $4 AND link = $1 AND resolved_at IS NULL
                    "#,
                )
                .bind(&link_json)
                .bind(occurred_at)
                .bind(resolved_by_uuid)
                .bind(*org.as_uuid())
                .execute(tx.as_mut())
                .await?;
                Ok(result.rows_affected())
            })
        })
        .await
    }

    /// Aggregate the caller's notifications by source object (개체별 view):
    /// one row per distinct `link`, newest activity first, with total/unread
    /// tallies, a per-category unread breakdown, the latest row for preview,
    /// and the caller's object-level mute state. Keyset-paginated on
    /// `(latest activity, link)` behind an opaque cursor; an undecodable or
    /// foreign cursor yields an empty page (fail-closed) and recipient scoping
    /// means a replayed cursor can only ever page the caller's own rows.
    pub async fn list_object_groups(
        &self,
        query: ListNotificationObjectGroupsQuery,
    ) -> Result<NotificationObjectGroupPage, PgNotificationError> {
        let limit = query.limit.clamp(1, 200);
        let recipient_uuid = *query.recipient.as_uuid();
        let org = current_org().map_err(KernelError::from)?;

        let cursor = match query.before.as_deref().map(GroupCursor::decode) {
            None => None,
            Some(Ok(cursor)) => Some(cursor),
            // Fail closed on a cursor this server never issued: an empty page,
            // never an error channel that would let cursor text probe state.
            Some(Err(())) => {
                return Ok(NotificationObjectGroupPage {
                    items: Vec::new(),
                    next_cursor: None,
                });
            }
        };
        let (cursor_at, cursor_link) = match cursor {
            Some(GroupCursor { at, link }) => (Some(at), Some(link)),
            None => (None, None),
        };
        let unread_only = query.unread_only;

        let rows = with_org_conn::<_, _, PgNotificationError>(&self.pool, org, move |tx| {
            Box::pin(async move {
                Ok(sqlx::query(sqlx::AssertSqlSafe(format!(
                    r#"
                    SELECT
                        g.link AS group_link,
                        g.total,
                        g.unread_count,
                        g.latest_at,
                        latest.id, latest.recipient_user_id, latest.category, latest.kind,
                        latest.body, latest.link, latest.unread, latest.created_at,
                        latest.read_at, latest.resolved_at, latest.muted,
                        cats.categories,
                        EXISTS (
                            SELECT 1 FROM notification_policies p
                            WHERE p.user_id = $1
                              AND p.action = 'mute'
                              AND (p.scope = 'all'
                                OR (p.scope = 'object' AND p.link = g.link))
                        ) AS group_muted
                    FROM (
                        SELECT link, COUNT(*) AS total,
                               COUNT(*) FILTER (WHERE unread) AS unread_count,
                               MAX(created_at) AS latest_at
                        FROM notifications
                        WHERE recipient_user_id = $1
                        GROUP BY link
                        HAVING ($2::timestamptz IS NULL
                                OR (MAX(created_at), link::text)
                                   < ($2::timestamptz, ($3::jsonb)::text))
                           AND (NOT $4 OR COUNT(*) FILTER (WHERE unread) > 0)
                        ORDER BY latest_at DESC, link::text DESC
                        LIMIT $5
                    ) g
                    JOIN LATERAL (
                        SELECT id, recipient_user_id, category, kind, body, link,
                               unread, created_at, read_at, resolved_at,
                               {MUTED_PREDICATE_SQL} AS muted
                        FROM notifications
                        WHERE recipient_user_id = $1 AND link = g.link
                        ORDER BY created_at DESC, id DESC
                        LIMIT 1
                    ) latest ON true
                    JOIN LATERAL (
                        SELECT COALESCE(
                                   jsonb_agg(jsonb_build_object(
                                       'category', c.category, 'unread', c.unread)
                                       ORDER BY c.category),
                                   '[]'::jsonb) AS categories
                        FROM (
                            SELECT category, COUNT(*) AS unread
                            FROM notifications
                            WHERE recipient_user_id = $1 AND link = g.link
                              AND unread = true
                            GROUP BY category
                        ) c
                    ) cats ON true
                    ORDER BY g.latest_at DESC, g.link::text DESC
                    "#,
                )))
                .bind(recipient_uuid)
                .bind(cursor_at)
                .bind(cursor_link)
                .bind(unread_only)
                .bind(limit)
                .fetch_all(tx.as_mut())
                .await?)
            })
        })
        .await?;

        let mut items = Vec::with_capacity(rows.len());
        let mut last_key: Option<GroupCursor> = None;
        for row in &rows {
            let link_json: serde_json::Value = row.try_get("group_link")?;
            let link: NotificationLink =
                serde_json::from_value(link_json.clone()).map_err(|err| {
                    KernelError::internal(format!("stored notification link is invalid: {err}"))
                })?;
            let categories_json: serde_json::Value = row.try_get("categories")?;
            let categories: Vec<NotificationCategoryCount> =
                serde_json::from_value(categories_json).map_err(|err| {
                    KernelError::internal(format!("group category breakdown is invalid: {err}"))
                })?;
            last_key = Some(GroupCursor {
                at: row.try_get("latest_at")?,
                link: link_json,
            });
            items.push(NotificationObjectGroup {
                link,
                total: row.try_get("total")?,
                unread: row.try_get("unread_count")?,
                categories,
                latest: summary_from_row(row)?,
                muted: row.try_get("group_muted")?,
            });
        }
        let next_cursor = if items.len() as i64 == limit {
            last_key.map(|key| key.encode()).transpose()?
        } else {
            None
        };
        Ok(NotificationObjectGroupPage { items, next_cursor })
    }

    /// Upsert one of the caller's mute policies (PUT semantics: setting the
    /// same target twice returns the same row). Direct-apply personal setting,
    /// audited as `notification.policy_set` with the REAL policy id — the
    /// audit event is computed inside the transaction, after the upsert.
    pub async fn upsert_policy(
        &self,
        command: UpsertNotificationPolicyCommand,
    ) -> Result<NotificationPolicySummary, PgNotificationError> {
        let org = current_org().map_err(KernelError::from)?;
        let org_uuid = *org.as_uuid();
        let recipient = command.recipient;
        let recipient_uuid = *recipient.as_uuid();
        let policy_id = NotificationPolicyId::new();
        let scope_str = command.scope.as_scope_str();
        let category = command.scope.category().map(str::to_owned);
        let link_json = command
            .scope
            .link()
            .map(serde_json::to_value)
            .transpose()
            .map_err(|err| {
                KernelError::internal(format!("notification policy link is not JSON: {err}"))
            })?;
        let trace = command.trace;
        let occurred_at = command.occurred_at;

        with_audits::<_, NotificationPolicySummary, PgNotificationError>(
            &self.pool,
            org,
            move |tx| {
                Box::pin(async move {
                    let row = sqlx::query(
                        r#"
                        INSERT INTO notification_policies (id, org_id, user_id, scope, category, link)
                        VALUES ($1, $2, $3, $4, $5, $6)
                        ON CONFLICT (org_id, user_id, action, scope,
                                     COALESCE(category, ''), COALESCE(link::text, ''))
                        DO UPDATE SET updated_at = now()
                        RETURNING id, scope, category, link, action, created_at
                        "#,
                    )
                    .bind(policy_id.as_uuid())
                    .bind(org_uuid)
                    .bind(recipient_uuid)
                    .bind(scope_str)
                    .bind(category)
                    .bind(link_json)
                    .fetch_one(tx.as_mut())
                    .await?;
                    let summary = policy_summary_from_row(&row)?;
                    let event = AuditEvent::new(
                        Some(recipient),
                        AuditAction::new("notification.policy_set")
                            .map_err(PgNotificationError::Domain)?,
                        "notification_policy",
                        summary.id.to_string(),
                        trace,
                        occurred_at,
                    )
                    .with_org(org);
                    Ok((summary, vec![event]))
                })
            },
        )
        .await
    }

    /// Delete (= unmute) one of the caller's policies. Cross-user ids are
    /// NotFound, indistinguishable from absent; the audit row only lands when
    /// a row was actually removed (error path rolls the transaction back).
    pub async fn delete_policy(
        &self,
        command: DeleteNotificationPolicyCommand,
    ) -> Result<(), PgNotificationError> {
        let org = current_org().map_err(KernelError::from)?;
        let recipient_uuid = *command.recipient.as_uuid();
        let policy_uuid = *command.policy_id.as_uuid();
        let event = AuditEvent::new(
            Some(command.recipient),
            AuditAction::new("notification.policy_clear")?,
            "notification_policy",
            command.policy_id.to_string(),
            command.trace,
            command.occurred_at,
        )
        .with_org(org);

        with_audit::<_, (), PgNotificationError>(&self.pool, event, move |tx| {
            Box::pin(async move {
                let result =
                    sqlx::query("DELETE FROM notification_policies WHERE id = $1 AND user_id = $2")
                        .bind(policy_uuid)
                        .bind(recipient_uuid)
                        .execute(tx.as_mut())
                        .await?;
                if result.rows_affected() == 0 {
                    return Err(KernelError::not_found("notification policy not found").into());
                }
                Ok(())
            })
        })
        .await
    }

    /// List the caller's routing policies, newest first.
    pub async fn list_policies(
        &self,
        query: ListNotificationPoliciesQuery,
    ) -> Result<Vec<NotificationPolicySummary>, PgNotificationError> {
        let recipient_uuid = *query.recipient.as_uuid();
        let org = current_org().map_err(KernelError::from)?;

        let rows = with_org_conn::<_, _, PgNotificationError>(&self.pool, org, move |tx| {
            Box::pin(async move {
                Ok(sqlx::query(
                    r#"
                    SELECT id, scope, category, link, action, created_at
                    FROM notification_policies
                    WHERE user_id = $1
                    ORDER BY created_at DESC, id DESC
                    "#,
                )
                .bind(recipient_uuid)
                .fetch_all(tx.as_mut())
                .await?)
            })
        })
        .await?;
        rows.iter().map(policy_summary_from_row).collect()
    }
}

/// Opaque keyset cursor for [`PgNotificationStore::list_object_groups`]:
/// base64url(JSON `{at, link}`) of the last group on the page. The link is
/// carried as raw JSON and compared via Postgres' own `::text` rendering on
/// BOTH sides — never against a Rust-serialized string, whose key order and
/// spacing differ from jsonb's canonical form.
#[derive(Debug, Serialize, Deserialize)]
struct GroupCursor {
    #[serde(with = "time::serde::rfc3339")]
    at: Timestamp,
    link: serde_json::Value,
}

impl GroupCursor {
    fn encode(&self) -> Result<String, PgNotificationError> {
        let json = serde_json::to_vec(self).map_err(|err| {
            KernelError::internal(format!("group cursor is not JSON-serializable: {err}"))
        })?;
        Ok(URL_SAFE_NO_PAD.encode(json))
    }

    /// A cursor this server never issued decodes to `Err(())` — the caller
    /// maps it to an empty page, deliberately without detail.
    fn decode(value: &str) -> Result<Self, ()> {
        let bytes = URL_SAFE_NO_PAD.decode(value).map_err(|_| ())?;
        serde_json::from_slice(&bytes).map_err(|_| ())
    }
}

impl NotificationSink for PgNotificationStore {
    fn emit(&self, command: EmitNotificationCommand) -> EmitNotificationFuture<'_> {
        Box::pin(async move {
            self.emit_notification(command)
                .await
                .map_err(KernelError::from)
        })
    }
}

impl NotificationResolver for PgNotificationStore {
    fn resolve_by_link(
        &self,
        command: ResolveNotificationsByLinkCommand,
    ) -> ResolveNotificationsFuture<'_> {
        Box::pin(async move {
            self.resolve_notifications_by_link(command)
                .await
                .map_err(KernelError::from)
        })
    }
}

fn summary_from_row(
    row: &sqlx::postgres::PgRow,
) -> Result<NotificationSummary, PgNotificationError> {
    let link_json: serde_json::Value = row.try_get("link")?;
    let link: NotificationLink = serde_json::from_value(link_json).map_err(|err| {
        KernelError::internal(format!("stored notification link is invalid: {err}"))
    })?;
    Ok(NotificationSummary {
        id: NotificationId::from_uuid(row.try_get("id")?),
        recipient_user_id: UserId::from_uuid(row.try_get("recipient_user_id")?),
        category: row.try_get("category")?,
        kind: row.try_get("kind")?,
        text: row.try_get("body")?,
        link,
        unread: row.try_get("unread")?,
        created_at: row.try_get("created_at")?,
        read_at: row.try_get("read_at")?,
        resolved_at: row.try_get("resolved_at")?,
        muted: row.try_get("muted")?,
    })
}

fn policy_summary_from_row(
    row: &sqlx::postgres::PgRow,
) -> Result<NotificationPolicySummary, PgNotificationError> {
    let link_json: Option<serde_json::Value> = row.try_get("link")?;
    let link = link_json
        .map(serde_json::from_value::<NotificationLink>)
        .transpose()
        .map_err(|err| {
            KernelError::internal(format!("stored notification policy link is invalid: {err}"))
        })?;
    Ok(NotificationPolicySummary {
        id: NotificationPolicyId::from_uuid(row.try_get("id")?),
        scope: row.try_get("scope")?,
        category: row.try_get("category")?,
        link,
        action: row.try_get("action")?,
        created_at: row.try_get("created_at")?,
    })
}
