//! Postgres messenger adapter.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

use std::collections::HashMap;
use std::sync::Arc;

use console_kernel_core::{
    BranchId, BranchScope, ErrorKind, EvidenceId, KernelError, MessageId, ThreadId, TraceContext,
    UserId, WorkOrderId,
};
use console_messenger_application::{
    AckSummary, CreateThreadCommand, EnsureWorkOrderThreadCommand, JoinThreadCommand,
    ListChannelsQuery, ListMembersQuery, ListThreadsQuery, MarkThreadReadCommand, MemberPresence,
    MemberProfileQuery, MemberSummary, MessageAckNotification, MessageNotifier, MessagePage,
    MessagePageQuery, MessagePostedNotification, MessageQuery, MessageSummary, ReadReceiptSummary,
    SearchMessagesQuery, SendMessageCommand, SetThreadMuteCommand, ThreadMuteSummary,
    ThreadPresenceQuery, ThreadSummary, ToggleAckCommand, messenger_audit_event,
};
use console_messenger_domain::{
    MessageBody, ThreadKind, ThreadVisibility, extract_mention_user_ids, extract_object_code_refs,
    presence_status_for_age,
};
use console_notifications_application::{EmitNotificationCommand, NotificationSink};
use console_notifications_domain::NotificationLink;
use console_platform_db::{DbError, with_audit, with_org_conn};
use console_platform_request_context::current_org;
use console_workorder_application::{
    WorkOrderCreatedEvent, WorkOrderCreatedFuture, WorkOrderCreatedListener,
};
use sqlx::{PgPool, Postgres, QueryBuilder, Row, Transaction};

#[derive(Debug, thiserror::Error)]
pub enum PgMessengerError {
    #[error(transparent)]
    Db(#[from] DbError),

    #[error(transparent)]
    Domain(#[from] KernelError),
}

impl PgMessengerError {
    #[must_use]
    pub fn kind(&self) -> ErrorKind {
        match self {
            Self::Domain(err) => err.kind,
            Self::Db(DbError::Sqlx(sqlx::Error::RowNotFound)) => ErrorKind::NotFound,
            Self::Db(DbError::Sqlx(sqlx::Error::Database(err)))
                if err.code().is_some_and(|code| code == "23505") =>
            {
                ErrorKind::Conflict
            }
            Self::Db(_) => ErrorKind::Internal,
        }
    }
}

impl From<sqlx::Error> for PgMessengerError {
    fn from(value: sqlx::Error) -> Self {
        Self::Db(DbError::Sqlx(value))
    }
}

#[derive(Clone)]
pub struct PgMessengerStore {
    pool: PgPool,
    notifier: Option<Arc<dyn MessageNotifier>>,
    notification_sink: Option<Arc<dyn NotificationSink>>,
}

impl std::fmt::Debug for PgMessengerStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PgMessengerStore")
            .field("pool", &self.pool)
            .field("has_notifier", &self.notifier.is_some())
            .field("has_notification_sink", &self.notification_sink.is_some())
            .finish()
    }
}

impl PgMessengerStore {
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self {
            pool,
            notifier: None,
            notification_sink: None,
        }
    }

    #[must_use]
    pub fn with_notifier(mut self, notifier: Arc<dyn MessageNotifier>) -> Self {
        self.notifier = Some(notifier);
        self
    }

    /// Wire the notification-center write port so an `@`-mention creates a
    /// recipient notification row (post-commit, best-effort).
    #[must_use]
    pub fn with_notification_sink(mut self, sink: Arc<dyn NotificationSink>) -> Self {
        self.notification_sink = Some(sink);
        self
    }

    #[must_use]
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    pub async fn create_thread(
        &self,
        command: CreateThreadCommand,
    ) -> Result<ThreadSummary, PgMessengerError> {
        ensure_branch_scope(&command.branch_scope, command.branch_id)?;
        let visibility = resolve_visibility(&command)?;
        let member_ids = normalized_members(command.actor, &command.member_ids)?;
        let thread_id = ThreadId::new();
        let branch_id = command.branch_id;
        let actor = command.actor;
        let org = current_org().map_err(KernelError::from)?;
        let org_uuid = *org.as_uuid();
        let event = messenger_audit_event(
            "message_thread.create",
            command.actor,
            branch_id,
            "message_thread",
            thread_id,
            command.trace,
            command.occurred_at,
        )?
        .with_org(org);

        with_audit::<_, ThreadSummary, PgMessengerError>(&self.pool, event, |tx| {
            Box::pin(async move {
                if let Some(work_order_id) = command.work_order_id {
                    ensure_work_order_branch_tx(tx, work_order_id, branch_id).await?;
                }
                ensure_members_active_in_branch_tx(tx, branch_id, command.actor, &member_ids)
                    .await?;
                insert_thread_tx(
                    tx,
                    NewThread {
                        id: thread_id,
                        kind: command.kind,
                        visibility,
                        branch_id,
                        work_order_id: command.work_order_id,
                        title: command.title.as_deref().map(str::trim),
                        actor: command.actor,
                        occurred_at: command.occurred_at,
                    },
                    &member_ids,
                    org_uuid,
                )
                .await?;
                fetch_thread_summary_tx(tx, thread_id, actor).await
            })
        })
        .await
    }

    pub async fn ensure_work_order_thread(
        &self,
        command: EnsureWorkOrderThreadCommand,
    ) -> Result<ThreadSummary, PgMessengerError> {
        if let Some(existing) =
            fetch_work_order_thread_pool(&self.pool, command.work_order_id, command.actor).await?
        {
            return Ok(existing);
        }

        let request_no = work_order_request_no(&self.pool, command.work_order_id).await?;
        ensure_work_order_branch_pool(&self.pool, command.work_order_id, command.branch_id).await?;
        let thread_id = ThreadId::new();
        let org = current_org().map_err(KernelError::from)?;
        let org_uuid = *org.as_uuid();
        let event = messenger_audit_event(
            "message_thread.create",
            command.actor,
            command.branch_id,
            "message_thread",
            thread_id,
            command.trace,
            command.occurred_at,
        )?
        .with_org(org);
        let title = format!("WO {request_no}");

        let result = with_audit::<_, ThreadSummary, PgMessengerError>(&self.pool, event, |tx| {
            Box::pin(async move {
                insert_thread_tx(
                    tx,
                    NewThread {
                        id: thread_id,
                        kind: ThreadKind::WorkOrder,
                        visibility: ThreadVisibility::Direct,
                        branch_id: command.branch_id,
                        work_order_id: Some(command.work_order_id),
                        title: Some(&title),
                        actor: command.actor,
                        occurred_at: command.occurred_at,
                    },
                    &[command.actor],
                    org_uuid,
                )
                .await?;
                fetch_thread_summary_tx(tx, thread_id, command.actor).await
            })
        })
        .await;

        match result {
            Ok(summary) => Ok(summary),
            Err(err) if err.kind() == ErrorKind::Conflict => {
                fetch_work_order_thread_pool(&self.pool, command.work_order_id, command.actor)
                    .await?
                    .ok_or_else(|| {
                        KernelError::conflict("work-order messenger thread create raced").into()
                    })
            }
            Err(err) => Err(err),
        }
    }

    pub async fn send_message(
        &self,
        command: SendMessageCommand,
    ) -> Result<MessageSummary, PgMessengerError> {
        let body = MessageBody::new(command.body)?;
        let access = require_thread_access(
            &self.pool,
            command.thread_id,
            command.actor,
            &command.branch_scope,
        )
        .await?;
        let message_id = MessageId::new();
        let attachment_ids = command.attachment_evidence_ids;
        let quoted_message_id = command.quoted_message_id;
        let actor = command.actor;
        let org = current_org().map_err(KernelError::from)?;
        let org_uuid = *org.as_uuid();
        let event = messenger_audit_event(
            "message.send",
            command.actor,
            access.branch_id,
            "message",
            message_id,
            command.trace,
            command.occurred_at,
        )?
        .with_org(org)
        .with_snapshots(
            None,
            Some(serde_json::json!({
                "thread_id": command.thread_id,
                "sender_id": command.actor,
                "attachment_count": attachment_ids.len(),
            })),
        );

        let summary = with_audit::<_, MessageSummary, PgMessengerError>(&self.pool, event, |tx| {
            Box::pin(async move {
                // Reply-quote: the quoted message must live in the same thread
                // (a CHECK cannot span rows). Reject a cross-thread or unknown
                // quote before persisting.
                if let Some(quoted_message_id) = quoted_message_id {
                    ensure_message_in_thread_tx(tx, quoted_message_id, command.thread_id).await?;
                }
                sqlx::query(
                    r#"
                        INSERT INTO messenger_messages (
                            id, thread_id, branch_id, sender_id, body,
                            quoted_message_id, sent_at, created_at, org_id
                        )
                        VALUES ($1, $2, $3, $4, $5, $6, $7, $7, $8)
                        "#,
                )
                .bind(*message_id.as_uuid())
                .bind(*command.thread_id.as_uuid())
                .bind(*access.branch_id.as_uuid())
                .bind(*command.actor.as_uuid())
                .bind(body.as_str())
                .bind(quoted_message_id.map(|id| *id.as_uuid()))
                .bind(command.occurred_at)
                .bind(org_uuid)
                .execute(tx.as_mut())
                .await?;

                for (index, evidence_id) in attachment_ids.iter().enumerate() {
                    let sort_order = i16::try_from(index + 1)
                        .map_err(|_| KernelError::validation("too many message attachments"))?;
                    sqlx::query(
                        r#"
                            INSERT INTO messenger_message_attachments (
                                message_id, evidence_id, sort_order, org_id
                            )
                            VALUES ($1, $2, $3, $4)
                            "#,
                    )
                    .bind(*message_id.as_uuid())
                    .bind(*evidence_id.as_uuid())
                    .bind(sort_order)
                    .bind(org_uuid)
                    .execute(tx.as_mut())
                    .await?;
                }

                sqlx::query("UPDATE messenger_threads SET updated_at = $2 WHERE id = $1")
                    .bind(*command.thread_id.as_uuid())
                    .bind(command.occurred_at)
                    .execute(tx.as_mut())
                    .await?;

                // Parse-on-write: persist `#`-object-code refs so the object's
                // inbound "referenced by" chain / graph traversal has a real
                // edge. Only tokens whose prefix is a known object_types code
                // prefix are stored (drops `#hashtag` noise); the target is
                // resolved under policy at read time. `#`-refs never notify
                // (DESIGN §4.7-7). Written in-tx so refs commit with the message.
                persist_message_refs(tx, org_uuid, message_id, body.as_str()).await?;

                // Presence read model: sending a message is a real activity
                // signal (not a heartbeat), so bump the sender's last_activity.
                upsert_presence_tx(tx, org_uuid, actor, command.occurred_at).await?;

                fetch_message_summary_tx(tx, message_id, actor).await
            })
        })
        .await?;

        if let Some(notifier) = &self.notifier {
            notifier
                .message_posted(MessagePostedNotification {
                    message_id: summary.id,
                    thread_id: summary.thread_id,
                    branch_id: summary.branch_id,
                })
                .await;
        }

        // DESIGN §4.7-7: an `@`-mention notifies its target; `#`/`!` links do
        // not. Resolve the body's `@<uuid>` tokens to real thread members (minus
        // the sender) and emit one notification-center row per recipient — ids /
        // refs only, no message body (no PII on the wire). Best-effort: the
        // message is already committed, so a failed emit is logged, not fatal.
        // The stable dedup key makes a retried emit a no-op.
        if let Some(sink) = &self.notification_sink {
            let recipients = match resolve_mention_recipients(
                &self.pool,
                summary.thread_id,
                command.actor,
                summary.body.as_str(),
            )
            .await
            {
                Ok(recipients) => recipients,
                Err(err) => {
                    tracing::warn!(
                        message_id = %summary.id,
                        error = %err,
                        "messenger mention resolution failed; skipping notifications this send"
                    );
                    Vec::new()
                }
            };
            for recipient in recipients {
                let emit = EmitNotificationCommand {
                    actor: Some(command.actor),
                    recipient,
                    category: "메신저".to_owned(),
                    // ponytail: unrelated-lane unblock (BE-ingest-checklist-gates
                    // needed a green `cargo check -p console-app`) — `kind` landed on
                    // `EmitNotificationCommand` without updating this call site;
                    // "info" mirrors the notices crate's generic default. The
                    // notifications-kind lane should replace with a precise kind.
                    kind: "info".to_owned(),
                    text: "메신저에서 회원님을 멘션했습니다".to_owned(),
                    link: NotificationLink::Object {
                        kind: "messenger_thread".to_owned(),
                        id: summary.thread_id.to_string(),
                    },
                    dedup_key: Some(format!("msg-mention:{}:{}", summary.id, recipient)),
                    trace: TraceContext::generate(),
                    occurred_at: time::OffsetDateTime::now_utc(),
                };
                if let Err(err) = sink.emit(emit).await {
                    tracing::warn!(
                        message_id = %summary.id,
                        %recipient,
                        error = %err,
                        "messenger mention notification emit failed"
                    );
                }
            }
        }

        Ok(summary)
    }

    /// Coalesce read receipts at the thread/user level and audit one
    /// `message.read` event per explicit read command rather than per message.
    /// That preserves who acknowledged which latest message while avoiding an
    /// audit row explosion for long catch-up pages.
    pub async fn mark_thread_read(
        &self,
        command: MarkThreadReadCommand,
    ) -> Result<ReadReceiptSummary, PgMessengerError> {
        let access = require_thread_access(
            &self.pool,
            command.thread_id,
            command.actor,
            &command.branch_scope,
        )
        .await?;
        ensure_message_in_thread_pool(&self.pool, command.last_read_message_id, command.thread_id)
            .await?;
        let org = current_org().map_err(KernelError::from)?;
        let org_uuid = *org.as_uuid();
        let event = messenger_audit_event(
            "message.read",
            command.actor,
            access.branch_id,
            "message_thread",
            command.thread_id,
            command.trace,
            command.occurred_at,
        )?
        .with_org(org)
        .with_snapshots(
            None,
            Some(serde_json::json!({
                "last_read_message_id": command.last_read_message_id,
            })),
        );

        with_audit::<_, ReadReceiptSummary, PgMessengerError>(&self.pool, event, |tx| {
            Box::pin(async move {
                sqlx::query(
                    r#"
                    INSERT INTO messenger_read_receipts (
                        thread_id, user_id, last_read_message_id, read_at, updated_at, org_id
                    )
                    VALUES ($1, $2, $3, $4, $4, $5)
                    ON CONFLICT (thread_id, user_id) DO UPDATE
                    SET last_read_message_id = EXCLUDED.last_read_message_id,
                        read_at = EXCLUDED.read_at,
                        updated_at = EXCLUDED.updated_at
                    WHERE EXISTS (
                        SELECT 1
                        FROM messenger_messages incoming
                        JOIN messenger_messages current_receipt_message
                          ON current_receipt_message.id = messenger_read_receipts.last_read_message_id
                        WHERE incoming.id = EXCLUDED.last_read_message_id
                          AND (incoming.sent_at, incoming.id) >= (
                              current_receipt_message.sent_at,
                              current_receipt_message.id
                          )
                    )
                    "#,
                )
                .bind(*command.thread_id.as_uuid())
                .bind(*command.actor.as_uuid())
                .bind(*command.last_read_message_id.as_uuid())
                .bind(command.occurred_at)
                .bind(org_uuid)
                .execute(tx.as_mut())
                .await?;
                upsert_presence_tx(tx, org_uuid, command.actor, command.occurred_at).await?;
                fetch_read_receipt_tx(tx, command.thread_id, command.actor).await
            })
        })
        .await
    }

    pub async fn list_threads(
        &self,
        query: ListThreadsQuery,
    ) -> Result<Vec<ThreadSummary>, PgMessengerError> {
        let limit = normalized_limit(query.limit);
        let mut builder = QueryBuilder::<Postgres>::new(
            r#"
            SELECT t.id, t.kind, t.visibility, t.branch_id, t.work_order_id, t.title,
                   t.created_at, t.updated_at,
                   lm.id AS last_message_id,
                   lm.sent_at AS last_message_at,
                   COUNT(DISTINCT tm_all.user_id)::BIGINT AS member_count,
                   COALESCE(unread.unread_count, 0)::BIGINT AS unread_count,
                   bool_or(mute.user_id IS NOT NULL) AS muted
            FROM messenger_threads t
            JOIN messenger_thread_members tm_actor
              ON tm_actor.thread_id = t.id
             AND tm_actor.user_id =
            "#,
        );
        builder.push_bind(*query.actor.as_uuid());
        builder.push(
            r#"
            LEFT JOIN messenger_thread_mutes mute
              ON mute.thread_id = t.id
             AND mute.user_id =
            "#,
        );
        builder.push_bind(*query.actor.as_uuid());
        builder.push(
            r#"
            LEFT JOIN messenger_read_receipts rr
              ON rr.thread_id = t.id
             AND rr.user_id =
            "#,
        );
        builder.push_bind(*query.actor.as_uuid());
        builder.push(
            r#"
            LEFT JOIN messenger_messages rr_msg ON rr_msg.id = rr.last_read_message_id
            LEFT JOIN LATERAL (
                SELECT id, sent_at
                FROM messenger_messages
                WHERE thread_id = t.id
                ORDER BY sent_at DESC, id DESC
                LIMIT 1
            ) lm ON true
            LEFT JOIN LATERAL (
                SELECT COUNT(*)::BIGINT AS unread_count
                FROM messenger_messages unread_message
                WHERE unread_message.thread_id = t.id
                  AND unread_message.sender_id <>
            "#,
        );
        builder.push_bind(*query.actor.as_uuid());
        builder.push(
            r#"
                  AND (
                    rr.last_read_message_id IS NULL
                    OR rr_msg.id IS NULL
                    OR (unread_message.sent_at, unread_message.id) > (rr_msg.sent_at, rr_msg.id)
                  )
            ) unread ON true
            LEFT JOIN messenger_thread_members tm_all ON tm_all.thread_id = t.id
            WHERE true
            "#,
        );
        push_scope_filter(&mut builder, "t.branch_id", &query.branch_scope)?;
        builder.push(
            r#"
            GROUP BY t.id, lm.id, lm.sent_at, unread.unread_count
            ORDER BY COALESCE(lm.sent_at, t.updated_at) DESC, t.id DESC
            LIMIT
            "#,
        );
        builder.push_bind(limit);

        let org = current_org().map_err(KernelError::from)?;
        let rows = with_org_conn::<_, _, PgMessengerError>(&self.pool, org, move |tx| {
            Box::pin(async move { Ok(builder.build().fetch_all(tx.as_mut()).await?) })
        })
        .await?;
        rows.iter().map(thread_summary_from_row).collect()
    }

    /// Discover joinable channels in the caller's branch scope, membership
    /// notwithstanding, so a member can find a room to join. Deny-by-omission:
    /// only `channel`-visibility threads inside a branch the principal is scoped
    /// to are returned. `unread_count` is the caller's own (0 if not a member);
    /// `muted` reflects the caller's setting.
    pub async fn list_channels(
        &self,
        query: ListChannelsQuery,
    ) -> Result<Vec<ThreadSummary>, PgMessengerError> {
        let limit = normalized_limit(query.limit);
        let mut builder = thread_summary_builder(query.actor);
        builder.push(" WHERE t.visibility = 'channel'");
        push_scope_filter(&mut builder, "t.branch_id", &query.branch_scope)?;
        builder.push(" GROUP BY t.id, t.visibility, lm.id, lm.sent_at");
        builder.push(" ORDER BY COALESCE(lm.sent_at, t.updated_at) DESC, t.id DESC LIMIT ");
        builder.push_bind(limit);

        let org = current_org().map_err(KernelError::from)?;
        let rows = with_org_conn::<_, _, PgMessengerError>(&self.pool, org, move |tx| {
            Box::pin(async move { Ok(builder.build().fetch_all(tx.as_mut()).await?) })
        })
        .await?;
        rows.iter().map(thread_summary_from_row).collect()
    }

    /// Join a `channel`-visibility thread the caller can see in scope. A DM or
    /// other `direct` thread is not joinable (its member set is fixed), and a
    /// channel outside the caller's branch scope is `not_found` — a non-member
    /// cannot enumerate or join it. Idempotent: re-joining is a no-op.
    pub async fn join_thread(
        &self,
        command: JoinThreadCommand,
    ) -> Result<ThreadSummary, PgMessengerError> {
        let (branch_id, visibility) =
            thread_branch_and_visibility(&self.pool, command.thread_id).await?;
        if !command.branch_scope.allows(branch_id) {
            return Err(KernelError::not_found("messenger thread was not found").into());
        }
        if visibility != ThreadVisibility::Channel {
            return Err(KernelError::forbidden("messenger thread is not joinable").into());
        }
        let actor = command.actor;
        let org = current_org().map_err(KernelError::from)?;
        let org_uuid = *org.as_uuid();
        let event = messenger_audit_event(
            "message_thread.join",
            command.actor,
            branch_id,
            "message_thread",
            command.thread_id,
            command.trace,
            command.occurred_at,
        )?
        .with_org(org);

        with_audit::<_, ThreadSummary, PgMessengerError>(&self.pool, event, move |tx| {
            Box::pin(async move {
                sqlx::query(
                    r#"
                    INSERT INTO messenger_thread_members (thread_id, user_id, role, joined_at, org_id)
                    VALUES ($1, $2, 'MEMBER', $3, $4)
                    ON CONFLICT (thread_id, user_id) DO NOTHING
                    "#,
                )
                .bind(*command.thread_id.as_uuid())
                .bind(*actor.as_uuid())
                .bind(command.occurred_at)
                .bind(org_uuid)
                .execute(tx.as_mut())
                .await?;
                fetch_thread_summary_tx(tx, command.thread_id, actor).await
            })
        })
        .await
    }

    /// Toggle the caller's ack on a message (idempotent): if present it is
    /// removed, else inserted. The caller must be a member of the message's
    /// thread — a non-member cannot ack. Returns the post-toggle state and live
    /// count; a realtime `message_ack` event fans the new count to subscribers.
    pub async fn toggle_ack(
        &self,
        command: ToggleAckCommand,
    ) -> Result<AckSummary, PgMessengerError> {
        let (thread_id, _branch_id) =
            message_thread_and_branch(&self.pool, command.message_id).await?;
        let access =
            require_thread_access(&self.pool, thread_id, command.actor, &command.branch_scope)
                .await?;
        let actor = command.actor;
        let message_id = command.message_id;
        let org = current_org().map_err(KernelError::from)?;
        let org_uuid = *org.as_uuid();
        let event = messenger_audit_event(
            "message.ack",
            command.actor,
            access.branch_id,
            "message",
            command.message_id,
            command.trace,
            command.occurred_at,
        )?
        .with_org(org);

        let summary = with_audit::<_, AckSummary, PgMessengerError>(&self.pool, event, move |tx| {
            Box::pin(async move {
                let deleted = sqlx::query(
                    "DELETE FROM messenger_message_acks WHERE message_id = $1 AND user_id = $2",
                )
                .bind(*message_id.as_uuid())
                .bind(*actor.as_uuid())
                .execute(tx.as_mut())
                .await?
                .rows_affected();
                let acked = if deleted == 0 {
                    // ON CONFLICT DO NOTHING: a concurrent toggle (double-tap,
                    // two tabs) can race this same DELETE-then-INSERT and win
                    // the INSERT first. Either way a row now exists for
                    // (message_id, user_id), so the caller is acked — without
                    // this, the loser's INSERT hits the primary key and the
                    // request 500s instead of landing idempotently.
                    sqlx::query(
                        r#"
                        INSERT INTO messenger_message_acks (message_id, user_id, org_id, acked_at)
                        VALUES ($1, $2, $3, $4)
                        ON CONFLICT (message_id, user_id) DO NOTHING
                        "#,
                    )
                    .bind(*message_id.as_uuid())
                    .bind(*actor.as_uuid())
                    .bind(org_uuid)
                    .bind(command.occurred_at)
                    .execute(tx.as_mut())
                    .await?;
                    true
                } else {
                    false
                };
                // Acking is a real activity signal (Slack "확인").
                upsert_presence_tx(tx, org_uuid, actor, command.occurred_at).await?;
                let ack_count: i64 = sqlx::query_scalar(
                    "SELECT COUNT(*)::BIGINT FROM messenger_message_acks WHERE message_id = $1",
                )
                .bind(*message_id.as_uuid())
                .fetch_one(tx.as_mut())
                .await?;
                Ok(AckSummary {
                    message_id,
                    thread_id,
                    acked,
                    ack_count,
                })
            })
        })
        .await?;

        if let Some(notifier) = &self.notifier {
            notifier
                .message_ack_toggled(MessageAckNotification {
                    message_id: summary.message_id,
                    thread_id: summary.thread_id,
                    branch_id: access.branch_id,
                })
                .await;
        }

        Ok(summary)
    }

    /// Direct-save the caller's personal mute for a thread (DESIGN §3.9.0
    /// whitelist ①). The caller must be a thread member. Idempotent: setting the
    /// current state again is a no-op that returns the same summary.
    pub async fn set_thread_mute(
        &self,
        command: SetThreadMuteCommand,
    ) -> Result<ThreadMuteSummary, PgMessengerError> {
        let access = require_thread_access(
            &self.pool,
            command.thread_id,
            command.actor,
            &command.branch_scope,
        )
        .await?;
        let actor = command.actor;
        let muted = command.muted;
        let org = current_org().map_err(KernelError::from)?;
        let org_uuid = *org.as_uuid();
        let event = messenger_audit_event(
            "message_thread.mute",
            command.actor,
            access.branch_id,
            "message_thread",
            command.thread_id,
            command.trace,
            command.occurred_at,
        )?
        .with_org(org)
        .with_snapshots(None, Some(serde_json::json!({ "muted": muted })));

        with_audit::<_, ThreadMuteSummary, PgMessengerError>(&self.pool, event, move |tx| {
            Box::pin(async move {
                if muted {
                    sqlx::query(
                        r#"
                        INSERT INTO messenger_thread_mutes (thread_id, user_id, org_id, muted_at)
                        VALUES ($1, $2, $3, $4)
                        ON CONFLICT (thread_id, user_id) DO NOTHING
                        "#,
                    )
                    .bind(*command.thread_id.as_uuid())
                    .bind(*actor.as_uuid())
                    .bind(org_uuid)
                    .bind(command.occurred_at)
                    .execute(tx.as_mut())
                    .await?;
                } else {
                    sqlx::query(
                        "DELETE FROM messenger_thread_mutes WHERE thread_id = $1 AND user_id = $2",
                    )
                    .bind(*command.thread_id.as_uuid())
                    .bind(*actor.as_uuid())
                    .execute(tx.as_mut())
                    .await?;
                }
                Ok(ThreadMuteSummary {
                    thread_id: command.thread_id,
                    muted,
                })
            })
        })
        .await
    }

    /// Activity-derived presence for every member of a thread the caller belongs
    /// to. A non-member gets `forbidden` (require_thread_access) and sees no
    /// presence at all. Status is derived from last_activity age at read time;
    /// a member who has never acted is `offline` with a null timestamp.
    pub async fn thread_presence(
        &self,
        query: ThreadPresenceQuery,
    ) -> Result<Vec<MemberPresence>, PgMessengerError> {
        require_thread_access(
            &self.pool,
            query.thread_id,
            query.actor,
            &query.branch_scope,
        )
        .await?;
        let thread_id = query.thread_id;
        let org = current_org().map_err(KernelError::from)?;
        let now = time::OffsetDateTime::now_utc();
        let rows = with_org_conn::<_, _, PgMessengerError>(&self.pool, org, move |tx| {
            Box::pin(async move {
                Ok(sqlx::query(
                    r#"
                    SELECT tm.user_id,
                           u.display_name,
                           p.last_activity_at
                    FROM messenger_thread_members tm
                    LEFT JOIN users u ON u.id = tm.user_id
                    LEFT JOIN messenger_presence p ON p.user_id = tm.user_id
                    WHERE tm.thread_id = $1
                    ORDER BY p.last_activity_at DESC NULLS LAST, tm.user_id
                    "#,
                )
                .bind(*thread_id.as_uuid())
                .fetch_all(tx.as_mut())
                .await?)
            })
        })
        .await?;
        rows.iter()
            .map(|row| {
                let last_activity_at: Option<time::OffsetDateTime> =
                    row.try_get("last_activity_at")?;
                let age_seconds = last_activity_at.map(|at| (now - at).whole_seconds());
                Ok(MemberPresence {
                    user_id: UserId::from_uuid(row.try_get("user_id")?),
                    display_name: row.try_get("display_name")?,
                    last_activity_at,
                    status: presence_status_for_age(age_seconds),
                })
            })
            .collect()
    }

    pub async fn list_members(
        &self,
        query: ListMembersQuery,
    ) -> Result<Vec<MemberSummary>, PgMessengerError> {
        ensure_branch_scope(&query.branch_scope, query.branch_id)?;
        let limit = normalized_limit(query.limit);
        let branch_id = query.branch_id;
        let org = current_org().map_err(KernelError::from)?;
        let rows = with_org_conn::<_, _, PgMessengerError>(&self.pool, org, move |tx| {
            Box::pin(async move {
                Ok(sqlx::query(
                    r#"
                    SELECT u.id, u.display_name, u.team
                    FROM users u
                    JOIN user_branches ub
                      ON ub.user_id = u.id
                     AND ub.branch_id = $1
                    WHERE u.is_active = true
                    ORDER BY lower(u.display_name), u.created_at DESC, u.id
                    LIMIT $2
                    "#,
                )
                .bind(*branch_id.as_uuid())
                .bind(limit)
                .fetch_all(tx.as_mut())
                .await?)
            })
        })
        .await?;
        rows.iter()
            .map(|row| {
                Ok(MemberSummary {
                    id: UserId::from_uuid(row.try_get("id")?),
                    display_name: row.try_get("display_name")?,
                    team: row.try_get("team")?,
                })
            })
            .collect()
    }

    /// Fetch one branch member's summary for a person pin panel (UI-M2a AC).
    /// Viewing another person records a `person.view` audit event inside the
    /// read transaction — so the "열람 — 기록 남음" evidence and the read commit
    /// or roll back together. A self-view records no audit. A target that is not
    /// a visible active member of the branch yields `not_found` and (for the
    /// audited path) rolls back, so an unauthorized view leaves no audit trail.
    pub async fn member_profile(
        &self,
        query: MemberProfileQuery,
    ) -> Result<MemberSummary, PgMessengerError> {
        ensure_branch_scope(&query.branch_scope, query.branch_id)?;
        let org = current_org().map_err(KernelError::from)?;
        let branch_id = query.branch_id;
        let target = query.user_id;

        if query.actor == target {
            // Self-view: plain branch-scoped read, no audit.
            let member = with_org_conn::<_, _, PgMessengerError>(&self.pool, org, move |tx| {
                Box::pin(async move { fetch_branch_member_tx(tx, branch_id, target).await })
            })
            .await?;
            return member.ok_or_else(|| {
                KernelError::not_found("member was not found in the branch").into()
            });
        }

        let event = messenger_audit_event(
            "person.view",
            query.actor,
            branch_id,
            "person",
            target,
            query.trace,
            query.occurred_at,
        )?
        .with_org(org);
        with_audit::<_, MemberSummary, PgMessengerError>(&self.pool, event, move |tx| {
            Box::pin(async move {
                fetch_branch_member_tx(tx, branch_id, target)
                    .await?
                    .ok_or_else(|| {
                        KernelError::not_found("member was not found in the branch").into()
                    })
            })
        })
        .await
    }

    /// Materialize one message only within the actor's current membership and scope.
    pub async fn message(
        &self,
        query: MessageQuery,
    ) -> Result<Option<MessageSummary>, PgMessengerError> {
        let mut builder = message_select_builder(query.actor);
        builder.push(" WHERE m.id = ");
        builder.push_bind(*query.message_id.as_uuid());
        push_message_access(&mut builder, query.actor, &query.branch_scope)?;
        builder.push(MESSAGE_GROUP_BY);
        let org = current_org().map_err(KernelError::from)?;
        let row = with_org_conn::<_, _, PgMessengerError>(&self.pool, org, move |tx| {
            Box::pin(async move {
                sqlx::query("SET LOCAL statement_timeout = '5s'")
                    .execute(tx.as_mut())
                    .await?;
                Ok(builder.build().fetch_optional(tx.as_mut()).await?)
            })
        })
        .await?;
        row.as_ref().map(message_summary_from_row).transpose()
    }

    pub async fn message_page(
        &self,
        query: MessagePageQuery,
    ) -> Result<MessagePage, PgMessengerError> {
        require_thread_access(
            &self.pool,
            query.thread_id,
            query.actor,
            &query.branch_scope,
        )
        .await?;
        let limit = normalized_limit(query.limit);
        let page_limit = limit + 1;
        let before = match query.before_message_id {
            Some(message_id) => {
                Some(message_cursor(&self.pool, message_id, query.thread_id).await?)
            }
            None => None,
        };

        let mut builder = message_select_builder(query.actor);
        builder.push(" WHERE m.thread_id = ");
        builder.push_bind(*query.thread_id.as_uuid());
        push_message_access(&mut builder, query.actor, &query.branch_scope)?;
        if let Some((sent_at, id)) = before {
            builder.push(" AND (m.sent_at, m.id) < (");
            builder.push_bind(sent_at);
            builder.push(", ");
            builder.push_bind(id);
            builder.push(")");
        }
        builder.push(MESSAGE_GROUP_BY);
        builder.push(" ORDER BY m.sent_at DESC, m.id DESC LIMIT ");
        builder.push_bind(page_limit);

        let org = current_org().map_err(KernelError::from)?;
        let rows = with_org_conn::<_, _, PgMessengerError>(&self.pool, org, move |tx| {
            Box::pin(async move { Ok(builder.build().fetch_all(tx.as_mut()).await?) })
        })
        .await?;
        let mut items: Vec<MessageSummary> = rows
            .iter()
            .map(message_summary_from_row)
            .collect::<Result<_, _>>()?;
        let has_more = i64::try_from(items.len()).unwrap_or(0) > limit;
        if has_more {
            let _ = items.pop();
        }
        let next_cursor = if has_more {
            items.last().map(|message| message.id)
        } else {
            None
        };

        Ok(MessagePage { items, next_cursor })
    }

    /// Search primarily uses the `search_vector` GIN index. PostgreSQL's
    /// built-in `simple` configuration has limited Korean tokenization without
    /// a mecab-style dictionary, so this T3.1 slice also applies an `ILIKE`
    /// substring fallback for Korean terms. That keeps polling search useful
    /// without pretending the FTS stack is linguistically complete.
    pub async fn search_messages(
        &self,
        query: SearchMessagesQuery,
    ) -> Result<Vec<MessageSummary>, PgMessengerError> {
        let search = query.query.trim();
        if search.is_empty() {
            return Err(KernelError::validation("search query is required").into());
        }
        let limit = normalized_limit(query.limit);
        let mut builder = message_select_builder(query.actor);
        builder.push(" WHERE (m.search_vector @@ plainto_tsquery('simple', ");
        builder.push_bind(search);
        builder.push(") OR m.body ILIKE ");
        builder.push_bind(format!("%{search}%"));
        builder.push(")");
        push_message_access(&mut builder, query.actor, &query.branch_scope)?;
        builder.push(MESSAGE_GROUP_BY);
        builder.push(" ORDER BY m.sent_at DESC, m.id DESC LIMIT ");
        builder.push_bind(limit);

        let org = current_org().map_err(KernelError::from)?;
        let rows = with_org_conn::<_, _, PgMessengerError>(&self.pool, org, move |tx| {
            Box::pin(async move { Ok(builder.build().fetch_all(tx.as_mut()).await?) })
        })
        .await?;
        rows.iter().map(message_summary_from_row).collect()
    }
}

impl WorkOrderCreatedListener for PgMessengerStore {
    fn work_order_created(&self, event: WorkOrderCreatedEvent) -> WorkOrderCreatedFuture<'_> {
        Box::pin(async move {
            self.ensure_work_order_thread(EnsureWorkOrderThreadCommand {
                actor: event.actor,
                branch_id: event.branch_id,
                work_order_id: event.work_order_id,
                trace: event.trace,
                occurred_at: event.occurred_at,
            })
            .await
            .map(|_| ())
            .map_err(kernel_error_from_messenger_error)
        })
    }
}

#[derive(Debug, Clone, Copy)]
struct ThreadAccess {
    branch_id: BranchId,
}

#[derive(Debug, Clone, Copy)]
struct NewThread<'a> {
    id: ThreadId,
    kind: ThreadKind,
    visibility: ThreadVisibility,
    branch_id: BranchId,
    work_order_id: Option<WorkOrderId>,
    title: Option<&'a str>,
    actor: UserId,
    occurred_at: time::OffsetDateTime,
}

fn validate_thread_shape(command: &CreateThreadCommand) -> Result<(), PgMessengerError> {
    match command.kind {
        ThreadKind::WorkOrder if command.work_order_id.is_none() => Err(KernelError::validation(
            "work-order messenger thread requires work_order_id",
        )
        .into()),
        ThreadKind::WorkOrder => Ok(()),
        ThreadKind::Team | ThreadKind::Dm | ThreadKind::Group
            if command.work_order_id.is_some() =>
        {
            Err(KernelError::validation(
                "only work-order messenger threads may carry work_order_id",
            )
            .into())
        }
        ThreadKind::Dm if normalized_members(command.actor, &command.member_ids)?.len() != 2 => {
            Err(KernelError::validation("DM thread requires exactly two members").into())
        }
        ThreadKind::Group if normalized_members(command.actor, &command.member_ids)?.len() < 3 => {
            Err(KernelError::validation("group thread requires at least three members").into())
        }
        _ => Ok(()),
    }
}

/// Validate the thread shape, then resolve the taxonomy `visibility`: use the
/// caller's explicit choice if given, else the `kind`/title default. Enforces
/// the DB invariants up front with clear errors: a channel must be named, and
/// DMs, groups, and work-order threads are always direct.
fn resolve_visibility(command: &CreateThreadCommand) -> Result<ThreadVisibility, PgMessengerError> {
    validate_thread_shape(command)?;
    let has_title = command
        .title
        .as_deref()
        .is_some_and(|t| !t.trim().is_empty());
    let visibility = command
        .visibility
        .unwrap_or_else(|| ThreadVisibility::default_for(command.kind, has_title));
    if visibility == ThreadVisibility::Channel {
        if !has_title {
            return Err(KernelError::validation("a channel thread requires a title").into());
        }
        if matches!(
            command.kind,
            ThreadKind::Dm | ThreadKind::Group | ThreadKind::WorkOrder
        ) {
            return Err(KernelError::validation(
                "DM, group, and work-order threads are always direct, not channels",
            )
            .into());
        }
    }
    Ok(visibility)
}

fn normalized_members(actor: UserId, members: &[UserId]) -> Result<Vec<UserId>, PgMessengerError> {
    let mut members = members.to_vec();
    members.push(actor);
    members.sort();
    members.dedup();
    if members.is_empty() {
        return Err(KernelError::validation("messenger thread requires members").into());
    }
    Ok(members)
}

fn ensure_branch_scope(scope: &BranchScope, branch_id: BranchId) -> Result<(), PgMessengerError> {
    if scope.allows(branch_id) {
        Ok(())
    } else {
        Err(KernelError::forbidden("messenger branch is outside principal scope").into())
    }
}

fn normalized_limit(limit: i64) -> i64 {
    limit.clamp(1, 100)
}

async fn ensure_members_active_in_branch_tx(
    tx: &mut Transaction<'_, Postgres>,
    branch_id: BranchId,
    actor: UserId,
    member_ids: &[UserId],
) -> Result<(), PgMessengerError> {
    let requested: Vec<uuid::Uuid> = member_ids
        .iter()
        .copied()
        .filter(|member_id| *member_id != actor)
        .map(|member_id| *member_id.as_uuid())
        .collect();
    if requested.is_empty() {
        return Ok(());
    }

    let active_member_count: i64 = sqlx::query_scalar(
        r#"
        SELECT COUNT(DISTINCT u.id)
        FROM users u
        JOIN user_branches ub
          ON ub.user_id = u.id
         AND ub.branch_id = $1
        WHERE u.id = ANY($2)
          AND u.is_active = true
        "#,
    )
    .bind(*branch_id.as_uuid())
    .bind(&requested)
    .fetch_one(tx.as_mut())
    .await?;

    if usize::try_from(active_member_count).ok() == Some(requested.len()) {
        Ok(())
    } else {
        Err(
            KernelError::validation("messenger members must be active users in the thread branch")
                .into(),
        )
    }
}

async fn insert_thread_tx(
    tx: &mut Transaction<'_, Postgres>,
    thread: NewThread<'_>,
    member_ids: &[UserId],
    org_uuid: uuid::Uuid,
) -> Result<(), PgMessengerError> {
    sqlx::query(
        r#"
        INSERT INTO messenger_threads (
            id, kind, visibility, branch_id, work_order_id, title,
            created_by, created_at, updated_at, org_id
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $8, $9)
        "#,
    )
    .bind(*thread.id.as_uuid())
    .bind(thread.kind.as_db_str())
    .bind(thread.visibility.as_db_str())
    .bind(*thread.branch_id.as_uuid())
    .bind(thread.work_order_id.map(|id| *id.as_uuid()))
    .bind(thread.title)
    .bind(*thread.actor.as_uuid())
    .bind(thread.occurred_at)
    .bind(org_uuid)
    .execute(tx.as_mut())
    .await?;

    for member_id in member_ids {
        let role = if *member_id == thread.actor {
            "OWNER"
        } else {
            "MEMBER"
        };
        sqlx::query(
            r#"
            INSERT INTO messenger_thread_members (thread_id, user_id, role, joined_at, org_id)
            VALUES ($1, $2, $3, $4, $5)
            ON CONFLICT (thread_id, user_id) DO NOTHING
            "#,
        )
        .bind(*thread.id.as_uuid())
        .bind(*member_id.as_uuid())
        .bind(role)
        .bind(thread.occurred_at)
        .bind(org_uuid)
        .execute(tx.as_mut())
        .await?;
    }

    Ok(())
}

/// Resolve which `@`-mentioned users are real, reachable recipients for a
/// posted message: parsed `@<uuid>` tokens, kept only if they are members of
/// this thread, with the sender removed (no self-notify). Order follows the
/// body's first-seen order. Deny-by-omission — a mention of a non-member (or a
/// nonexistent user) yields nothing, so it neither links nor notifies.
async fn resolve_mention_recipients(
    pool: &PgPool,
    thread_id: ThreadId,
    actor: UserId,
    body: &str,
) -> Result<Vec<UserId>, PgMessengerError> {
    let mentioned = extract_mention_user_ids(body);
    if mentioned.is_empty() {
        return Ok(Vec::new());
    }
    let candidate_uuids: Vec<uuid::Uuid> = mentioned
        .iter()
        .filter(|id| **id != actor)
        .map(|id| *id.as_uuid())
        .collect();
    if candidate_uuids.is_empty() {
        return Ok(Vec::new());
    }
    let org = current_org().map_err(KernelError::from)?;
    let member_uuids: std::collections::HashSet<uuid::Uuid> =
        with_org_conn::<_, _, PgMessengerError>(pool, org, move |tx| {
            Box::pin(async move {
                // A member who muted this thread is dropped here, so a mute
                // suppresses the mention-notification fan-out (the message is
                // still recorded; only the notification is withheld).
                let rows = sqlx::query(
                    r#"
                    SELECT tm.user_id
                    FROM messenger_thread_members tm
                    WHERE tm.thread_id = $1
                      AND tm.user_id = ANY($2)
                      AND NOT EXISTS (
                          SELECT 1 FROM messenger_thread_mutes mu
                          WHERE mu.thread_id = tm.thread_id
                            AND mu.user_id = tm.user_id
                      )
                    "#,
                )
                .bind(*thread_id.as_uuid())
                .bind(&candidate_uuids)
                .fetch_all(tx.as_mut())
                .await?;
                rows.into_iter()
                    .map(|row| Ok(row.try_get::<uuid::Uuid, _>("user_id")?))
                    .collect::<Result<std::collections::HashSet<uuid::Uuid>, PgMessengerError>>()
            })
        })
        .await?;
    Ok(mentioned
        .into_iter()
        // `member_uuids` is a subset of `candidate_uuids`, which already
        // excluded `actor` above — so membership alone enforces no self-notify.
        .filter(|id| member_uuids.contains(id.as_uuid()))
        .collect())
}

/// Persist the `#`-object-code references written in a message body. Runs
/// inside the send transaction. Each parsed code (already capped at
/// `MAX_OBJECT_CODE_REFS` by the parser) is kept only if its prefix matches a
/// seeded `object_types.code_prefix` (so the ref always names a known kind and
/// `#hashtag` noise is dropped) -- checked with ONE batched lookup rather than
/// one SELECT per token. `ON CONFLICT DO NOTHING` makes a re-sent identical
/// body idempotent alongside the parse's own dedup.
async fn persist_message_refs(
    tx: &mut Transaction<'_, Postgres>,
    org_uuid: uuid::Uuid,
    message_id: MessageId,
    body: &str,
) -> Result<(), PgMessengerError> {
    let codes = extract_object_code_refs(body);
    if codes.is_empty() {
        return Ok(());
    }
    // is_code_shaped guarantees every code has a '-'; the prefix is up to and
    // including it. Duplicate prefixes in the batch are harmless for ANY($1).
    let prefixes: Vec<&str> = codes
        .iter()
        .filter_map(|code| code.find('-').map(|dash| &code[..=dash]))
        .collect();
    let known: HashMap<String, String> =
        sqlx::query("SELECT code_prefix, kind FROM object_types WHERE code_prefix = ANY($1)")
            .bind(&prefixes)
            .fetch_all(tx.as_mut())
            .await?
            .iter()
            .map(|row| {
                Ok::<_, PgMessengerError>((
                    row.try_get::<String, _>("code_prefix")?,
                    row.try_get::<String, _>("kind")?,
                ))
            })
            .collect::<Result<_, _>>()?;

    for code in codes {
        let Some(dash) = code.find('-') else { continue };
        let Some(kind) = known.get(&code[..=dash]) else {
            continue;
        };
        sqlx::query(
            r#"
            INSERT INTO message_refs (org_id, message_id, ref_kind, ref_code)
            VALUES ($1, $2, $3, $4)
            ON CONFLICT (org_id, message_id, ref_code) DO NOTHING
            "#,
        )
        .bind(org_uuid)
        .bind(*message_id.as_uuid())
        .bind(kind)
        .bind(&code)
        .execute(tx.as_mut())
        .await?;
    }
    Ok(())
}

async fn fetch_branch_member_tx(
    tx: &mut Transaction<'_, Postgres>,
    branch_id: BranchId,
    user_id: UserId,
) -> Result<Option<MemberSummary>, PgMessengerError> {
    let row = sqlx::query(
        r#"
        SELECT u.id, u.display_name, u.team
        FROM users u
        JOIN user_branches ub
          ON ub.user_id = u.id
         AND ub.branch_id = $1
        WHERE u.is_active = true
          AND u.id = $2
        "#,
    )
    .bind(*branch_id.as_uuid())
    .bind(*user_id.as_uuid())
    .fetch_optional(tx.as_mut())
    .await?;
    row.map(|row| {
        Ok(MemberSummary {
            id: UserId::from_uuid(row.try_get("id")?),
            display_name: row.try_get("display_name")?,
            team: row.try_get("team")?,
        })
    })
    .transpose()
}

async fn require_thread_access(
    pool: &PgPool,
    thread_id: ThreadId,
    actor: UserId,
    branch_scope: &BranchScope,
) -> Result<ThreadAccess, PgMessengerError> {
    let org = current_org().map_err(KernelError::from)?;
    let row = with_org_conn::<_, _, PgMessengerError>(pool, org, move |tx| {
        Box::pin(async move {
            Ok(sqlx::query(
                r#"
        SELECT t.branch_id,
               EXISTS (
                   SELECT 1
                   FROM messenger_thread_members tm
                   WHERE tm.thread_id = t.id
                     AND tm.user_id = $2
               ) AS is_member
        FROM messenger_threads t
        WHERE t.id = $1
        "#,
            )
            .bind(*thread_id.as_uuid())
            .bind(*actor.as_uuid())
            .fetch_optional(tx.as_mut())
            .await?)
        })
    })
    .await?
    .ok_or_else(|| KernelError::not_found("messenger thread was not found"))?;

    let branch_id = BranchId::from_uuid(row.try_get("branch_id")?);
    ensure_branch_scope(branch_scope, branch_id)?;
    let is_member: bool = row
        .try_get::<Option<bool>, _>("is_member")?
        .unwrap_or(false);
    if !is_member {
        return Err(KernelError::forbidden("actor is not a messenger thread member").into());
    }
    Ok(ThreadAccess { branch_id })
}

async fn ensure_work_order_branch_pool(
    pool: &PgPool,
    work_order_id: WorkOrderId,
    branch_id: BranchId,
) -> Result<(), PgMessengerError> {
    let org = current_org().map_err(KernelError::from)?;
    let actual: uuid::Uuid = with_org_conn::<_, _, PgMessengerError>(pool, org, move |tx| {
        Box::pin(async move {
            Ok(
                sqlx::query_scalar("SELECT branch_id FROM work_orders WHERE id = $1")
                    .bind(*work_order_id.as_uuid())
                    .fetch_optional(tx.as_mut())
                    .await?,
            )
        })
    })
    .await?
    .ok_or_else(|| KernelError::not_found("work order was not found"))?;
    if actual == *branch_id.as_uuid() {
        Ok(())
    } else {
        Err(KernelError::forbidden("work order belongs to a different branch").into())
    }
}

async fn ensure_work_order_branch_tx(
    tx: &mut Transaction<'_, Postgres>,
    work_order_id: WorkOrderId,
    branch_id: BranchId,
) -> Result<(), PgMessengerError> {
    let actual: uuid::Uuid = sqlx::query_scalar("SELECT branch_id FROM work_orders WHERE id = $1")
        .bind(*work_order_id.as_uuid())
        .fetch_optional(tx.as_mut())
        .await?
        .ok_or_else(|| KernelError::not_found("work order was not found"))?;
    if actual == *branch_id.as_uuid() {
        Ok(())
    } else {
        Err(KernelError::forbidden("work order belongs to a different branch").into())
    }
}

async fn work_order_request_no(
    pool: &PgPool,
    work_order_id: WorkOrderId,
) -> Result<String, PgMessengerError> {
    let org = current_org().map_err(KernelError::from)?;
    with_org_conn::<_, _, PgMessengerError>(pool, org, move |tx| {
        Box::pin(async move {
            sqlx::query_scalar("SELECT request_no FROM work_orders WHERE id = $1")
                .bind(*work_order_id.as_uuid())
                .fetch_optional(tx.as_mut())
                .await?
                .ok_or_else(|| KernelError::not_found("work order was not found").into())
        })
    })
    .await
}

async fn fetch_work_order_thread_pool(
    pool: &PgPool,
    work_order_id: WorkOrderId,
    actor: UserId,
) -> Result<Option<ThreadSummary>, PgMessengerError> {
    let mut builder = thread_summary_builder(actor);
    builder.push(" WHERE t.work_order_id = ");
    builder.push_bind(*work_order_id.as_uuid());
    builder.push(" GROUP BY t.id, t.visibility, lm.id, lm.sent_at");
    let org = current_org().map_err(KernelError::from)?;
    let row = with_org_conn::<_, _, PgMessengerError>(pool, org, move |tx| {
        Box::pin(async move { Ok(builder.build().fetch_optional(tx.as_mut()).await?) })
    })
    .await?;
    row.as_ref().map(thread_summary_from_row).transpose()
}

async fn fetch_thread_summary_tx(
    tx: &mut Transaction<'_, Postgres>,
    thread_id: ThreadId,
    actor: UserId,
) -> Result<ThreadSummary, PgMessengerError> {
    let mut builder = thread_summary_builder(actor);
    builder.push(" WHERE t.id = ");
    builder.push_bind(*thread_id.as_uuid());
    builder.push(" GROUP BY t.id, t.visibility, lm.id, lm.sent_at");
    let row = builder.build().fetch_one(tx.as_mut()).await?;
    thread_summary_from_row(&row)
}

/// Thread summary SELECT with `member_count`, last-message, `visibility`, and
/// the caller's `muted` flag (via a bound `actor` EXISTS). `unread_count` is 0
/// here — the caller's real unread lives on the `list_threads` path; this
/// builder backs create/join/work-order fetch and channel discovery, where a
/// zero-unread summary is correct or immaterial.
fn thread_summary_builder(actor: UserId) -> QueryBuilder<Postgres> {
    let mut builder = QueryBuilder::<Postgres>::new(
        r#"
        SELECT t.id, t.kind, t.visibility, t.branch_id, t.work_order_id, t.title,
               t.created_at, t.updated_at,
               lm.id AS last_message_id,
               lm.sent_at AS last_message_at,
               COUNT(tm_all.user_id)::BIGINT AS member_count,
               0::BIGINT AS unread_count,
               EXISTS (
                   SELECT 1 FROM messenger_thread_mutes mu
                   WHERE mu.thread_id = t.id AND mu.user_id =
        "#,
    );
    builder.push_bind(*actor.as_uuid());
    builder.push(
        r#") AS muted
        FROM messenger_threads t
        LEFT JOIN LATERAL (
            SELECT id, sent_at
            FROM messenger_messages
            WHERE thread_id = t.id
            ORDER BY sent_at DESC, id DESC
            LIMIT 1
        ) lm ON true
        LEFT JOIN messenger_thread_members tm_all ON tm_all.thread_id = t.id
        "#,
    );
    builder
}

fn thread_summary_from_row(row: &sqlx::postgres::PgRow) -> Result<ThreadSummary, PgMessengerError> {
    let kind: String = row.try_get("kind")?;
    let visibility: String = row.try_get("visibility")?;
    Ok(ThreadSummary {
        id: ThreadId::from_uuid(row.try_get("id")?),
        kind: ThreadKind::from_db_str(&kind)?,
        visibility: ThreadVisibility::from_db_str(&visibility)?,
        muted: row.try_get("muted")?,
        branch_id: BranchId::from_uuid(row.try_get("branch_id")?),
        title: row.try_get("title")?,
        work_order_id: row
            .try_get::<Option<uuid::Uuid>, _>("work_order_id")?
            .map(WorkOrderId::from_uuid),
        last_message_id: row
            .try_get::<Option<uuid::Uuid>, _>("last_message_id")?
            .map(MessageId::from_uuid),
        last_message_at: row.try_get("last_message_at")?,
        member_count: row.try_get("member_count")?,
        unread_count: row.try_get("unread_count")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

/// Builds the message read-model SELECT. `actor` is bound for the per-caller
/// `acked_by_me` flag; ack count, reply-quote preview, read progress, and
/// attachments stay isolated from read/ack fan-out so one row is one `MessageSummary`.
fn message_select_builder(actor: UserId) -> QueryBuilder<Postgres> {
    let mut builder = QueryBuilder::<Postgres>::new(
        r#"
        SELECT m.id, m.thread_id, m.branch_id, m.sender_id, m.body,
               quoted.id AS quoted_message_id,
               quoted.body AS quoted_body,
               quoted_sender.display_name AS quoted_sender_name,
               m.sent_at, m.created_at, sender.display_name AS sender_name,
               COALESCE(att.attachment_evidence_ids, ARRAY[]::uuid[]) AS attachment_evidence_ids,
               COUNT(DISTINCT tm_read_target.user_id)::BIGINT AS read_target_count,
               COUNT(DISTINCT tm_read_target.user_id) FILTER (
                   WHERE read_receipt_message.id IS NOT NULL
                     AND (read_receipt_message.sent_at, read_receipt_message.id) >= (m.sent_at, m.id)
               )::BIGINT AS read_count,
               (SELECT COUNT(*) FROM messenger_message_acks ack
                WHERE ack.message_id = m.id)::BIGINT AS ack_count,
               EXISTS (
                   SELECT 1 FROM messenger_message_acks ack
                   WHERE ack.message_id = m.id AND ack.user_id =
        "#,
    );
    builder.push_bind(*actor.as_uuid());
    builder.push(
        r#") AS acked_by_me
        FROM messenger_messages m
        LEFT JOIN LATERAL (
            SELECT array_agg(a.evidence_id ORDER BY a.sort_order) AS attachment_evidence_ids
            FROM messenger_message_attachments a
            WHERE a.message_id = m.id
        ) att ON true
        LEFT JOIN messenger_thread_members tm_read_target
          ON tm_read_target.thread_id = m.thread_id
         AND tm_read_target.user_id <> m.sender_id
        LEFT JOIN messenger_read_receipts rr_read_target
          ON rr_read_target.thread_id = m.thread_id
         AND rr_read_target.user_id = tm_read_target.user_id
        LEFT JOIN messenger_messages read_receipt_message
          ON read_receipt_message.id = rr_read_target.last_read_message_id
        -- A corrupt cross-thread quote must reveal neither content nor its ID.
        LEFT JOIN messenger_messages quoted
          ON quoted.id = m.quoted_message_id AND quoted.thread_id = m.thread_id
        LEFT JOIN users quoted_sender ON quoted_sender.id = quoted.sender_id
        -- Same-org JOIN: `users` is RLS-scoped to app.current_org just like
        -- messenger_messages, so this can only resolve a sender in the caller's
        -- tenant. A cross-tenant or hard-deleted sender simply yields NULL.
        LEFT JOIN users sender ON sender.id = m.sender_id
        "#,
    );
    builder
}

const MESSAGE_GROUP_BY: &str = " GROUP BY m.id, sender.display_name, quoted.id, quoted.body, quoted_sender.display_name, att.attachment_evidence_ids";

fn push_message_access(
    builder: &mut QueryBuilder<Postgres>,
    actor: UserId,
    scope: &BranchScope,
) -> Result<(), PgMessengerError> {
    builder.push(" AND EXISTS (SELECT 1 FROM messenger_thread_members tm_actor WHERE tm_actor.thread_id = m.thread_id AND tm_actor.user_id = ");
    builder.push_bind(*actor.as_uuid());
    builder.push(")");
    push_scope_filter(builder, "m.branch_id", scope)
}

async fn fetch_message_summary_tx(
    tx: &mut Transaction<'_, Postgres>,
    message_id: MessageId,
    actor: UserId,
) -> Result<MessageSummary, PgMessengerError> {
    let mut builder = message_select_builder(actor);
    builder.push(" WHERE m.id = ");
    builder.push_bind(*message_id.as_uuid());
    builder.push(MESSAGE_GROUP_BY);
    let row = builder.build().fetch_one(tx.as_mut()).await?;
    message_summary_from_row(&row)
}

fn message_summary_from_row(
    row: &sqlx::postgres::PgRow,
) -> Result<MessageSummary, PgMessengerError> {
    let attachment_ids: Vec<uuid::Uuid> = row.try_get("attachment_evidence_ids")?;
    Ok(MessageSummary {
        id: MessageId::from_uuid(row.try_get("id")?),
        thread_id: ThreadId::from_uuid(row.try_get("thread_id")?),
        branch_id: BranchId::from_uuid(row.try_get("branch_id")?),
        sender_id: UserId::from_uuid(row.try_get("sender_id")?),
        sender_name: row.try_get("sender_name")?,
        body: row.try_get("body")?,
        attachment_evidence_ids: attachment_ids
            .into_iter()
            .map(EvidenceId::from_uuid)
            .collect(),
        read_count: row.try_get("read_count")?,
        read_target_count: row.try_get("read_target_count")?,
        ack_count: row.try_get("ack_count")?,
        acked_by_me: row
            .try_get::<Option<bool>, _>("acked_by_me")?
            .unwrap_or(false),
        quoted_message_id: row
            .try_get::<Option<uuid::Uuid>, _>("quoted_message_id")?
            .map(MessageId::from_uuid),
        quoted_body: row.try_get("quoted_body")?,
        quoted_sender_name: row.try_get("quoted_sender_name")?,
        sent_at: row.try_get("sent_at")?,
        created_at: row.try_get("created_at")?,
    })
}

/// Bump a user's presence activity timestamp inside an already-audited
/// transaction. Only ever moves the timestamp forward, so an out-of-order
/// action (a late-arriving read) never rewinds a fresher signal.
async fn upsert_presence_tx(
    tx: &mut Transaction<'_, Postgres>,
    org_uuid: uuid::Uuid,
    user_id: UserId,
    occurred_at: time::OffsetDateTime,
) -> Result<(), PgMessengerError> {
    sqlx::query(
        r#"
        INSERT INTO messenger_presence (user_id, org_id, last_activity_at, updated_at)
        VALUES ($1, $2, $3, $3)
        ON CONFLICT (user_id) DO UPDATE
        SET last_activity_at = GREATEST(messenger_presence.last_activity_at, EXCLUDED.last_activity_at),
            updated_at = EXCLUDED.updated_at
        "#,
    )
    .bind(*user_id.as_uuid())
    .bind(org_uuid)
    .bind(occurred_at)
    .execute(tx.as_mut())
    .await?;
    Ok(())
}

/// Same-thread guard for a reply-quote, inside the send transaction. A quoted
/// message that is missing or lives in another thread is rejected.
async fn ensure_message_in_thread_tx(
    tx: &mut Transaction<'_, Postgres>,
    message_id: MessageId,
    thread_id: ThreadId,
) -> Result<(), PgMessengerError> {
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM messenger_messages WHERE id = $1 AND thread_id = $2)",
    )
    .bind(*message_id.as_uuid())
    .bind(*thread_id.as_uuid())
    .fetch_one(tx.as_mut())
    .await?;
    if exists {
        Ok(())
    } else {
        Err(KernelError::validation("quoted message is not in this thread").into())
    }
}

/// Resolve a message's thread + branch (org-scoped), for ack membership checks.
async fn message_thread_and_branch(
    pool: &PgPool,
    message_id: MessageId,
) -> Result<(ThreadId, BranchId), PgMessengerError> {
    let org = current_org().map_err(KernelError::from)?;
    let row = with_org_conn::<_, _, PgMessengerError>(pool, org, move |tx| {
        Box::pin(async move {
            Ok(
                sqlx::query("SELECT thread_id, branch_id FROM messenger_messages WHERE id = $1")
                    .bind(*message_id.as_uuid())
                    .fetch_optional(tx.as_mut())
                    .await?,
            )
        })
    })
    .await?
    .ok_or_else(|| KernelError::not_found("message was not found"))?;
    Ok((
        ThreadId::from_uuid(row.try_get("thread_id")?),
        BranchId::from_uuid(row.try_get("branch_id")?),
    ))
}

/// Resolve a thread's branch + visibility (org-scoped), for the join guard.
async fn thread_branch_and_visibility(
    pool: &PgPool,
    thread_id: ThreadId,
) -> Result<(BranchId, ThreadVisibility), PgMessengerError> {
    let org = current_org().map_err(KernelError::from)?;
    let row = with_org_conn::<_, _, PgMessengerError>(pool, org, move |tx| {
        Box::pin(async move {
            Ok(
                sqlx::query("SELECT branch_id, visibility FROM messenger_threads WHERE id = $1")
                    .bind(*thread_id.as_uuid())
                    .fetch_optional(tx.as_mut())
                    .await?,
            )
        })
    })
    .await?
    .ok_or_else(|| KernelError::not_found("messenger thread was not found"))?;
    let visibility: String = row.try_get("visibility")?;
    Ok((
        BranchId::from_uuid(row.try_get("branch_id")?),
        ThreadVisibility::from_db_str(&visibility)?,
    ))
}

async fn ensure_message_in_thread_pool(
    pool: &PgPool,
    message_id: MessageId,
    thread_id: ThreadId,
) -> Result<(), PgMessengerError> {
    let org = current_org().map_err(KernelError::from)?;
    let exists: bool = with_org_conn::<_, _, PgMessengerError>(pool, org, move |tx| {
        Box::pin(async move {
            Ok(sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM messenger_messages WHERE id = $1 AND thread_id = $2)",
            )
            .bind(*message_id.as_uuid())
            .bind(*thread_id.as_uuid())
            .fetch_one(tx.as_mut())
            .await?)
        })
    })
    .await?;
    if exists {
        Ok(())
    } else {
        Err(KernelError::not_found("message was not found in thread").into())
    }
}

async fn message_cursor(
    pool: &PgPool,
    message_id: MessageId,
    thread_id: ThreadId,
) -> Result<(time::OffsetDateTime, uuid::Uuid), PgMessengerError> {
    let org = current_org().map_err(KernelError::from)?;
    let row = with_org_conn::<_, _, PgMessengerError>(pool, org, move |tx| {
        Box::pin(async move {
            Ok(sqlx::query(
                "SELECT sent_at, id FROM messenger_messages WHERE id = $1 AND thread_id = $2",
            )
            .bind(*message_id.as_uuid())
            .bind(*thread_id.as_uuid())
            .fetch_optional(tx.as_mut())
            .await?)
        })
    })
    .await?
    .ok_or_else(|| KernelError::not_found("message cursor was not found"))?;
    Ok((row.try_get("sent_at")?, row.try_get("id")?))
}

async fn fetch_read_receipt_tx(
    tx: &mut Transaction<'_, Postgres>,
    thread_id: ThreadId,
    user_id: UserId,
) -> Result<ReadReceiptSummary, PgMessengerError> {
    let row = sqlx::query(
        r#"
        SELECT thread_id, user_id, last_read_message_id, read_at, updated_at
        FROM messenger_read_receipts
        WHERE thread_id = $1 AND user_id = $2
        "#,
    )
    .bind(*thread_id.as_uuid())
    .bind(*user_id.as_uuid())
    .fetch_one(tx.as_mut())
    .await?;
    Ok(ReadReceiptSummary {
        thread_id: ThreadId::from_uuid(row.try_get("thread_id")?),
        user_id: UserId::from_uuid(row.try_get("user_id")?),
        last_read_message_id: MessageId::from_uuid(row.try_get("last_read_message_id")?),
        read_at: row.try_get("read_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

fn push_scope_filter(
    builder: &mut QueryBuilder<Postgres>,
    column: &str,
    scope: &BranchScope,
) -> Result<(), PgMessengerError> {
    match scope {
        BranchScope::All => Ok(()),
        BranchScope::Branches(branches) if branches.is_empty() => {
            builder.push(" AND false");
            Ok(())
        }
        BranchScope::Branches(branches) => {
            let branch_ids: Vec<uuid::Uuid> =
                branches.iter().map(|branch| *branch.as_uuid()).collect();
            builder.push(" AND ");
            builder.push(column);
            builder.push(" = ANY(");
            builder.push_bind(branch_ids);
            builder.push(")");
            Ok(())
        }
    }
}

fn kernel_error_from_messenger_error(err: PgMessengerError) -> KernelError {
    match err {
        PgMessengerError::Domain(err) => err,
        PgMessengerError::Db(err) => KernelError::new(
            ErrorKind::Internal,
            format!("messenger store failed: {err}"),
        ),
    }
}
