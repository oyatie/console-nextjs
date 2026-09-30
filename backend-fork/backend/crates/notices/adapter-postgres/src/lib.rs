//! Postgres notice-board adapter.
//!
//! Publishing is the pivot: it issues the canonical NT- code (shared
//! object-code counter, kind = `notification`), snapshots the notice's
//! effective audience (org-wide, or the members of its audience branches via
//! `user_branches`) into `notice_receipts` (one audited bulk insert), and —
//! best-effort, post-commit like the notifications realtime notifier — fans
//! out one `notifications`-table pointer per recipient via the
//! [`NotificationSink`] write port, so a published notice shows up on the
//! comms rail exactly like any other notification. The audience is frozen at
//! publish: drafts are editable ([`PgNoticeStore::update_draft`]), published
//! notices are not (their receipts are the record).
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

use std::sync::Arc;

use console_kernel_core::{BranchId, ErrorKind, KernelError, NoticeId, UserId};
use console_notices_application::{
    AcknowledgeNoticeCommand, CreateDraftNoticeCommand, GetNoticeQuery, ListNoticeReceiptsQuery,
    ListNoticesQuery, NoticeAudienceBranch, NoticeProgress, NoticeProgressQuery, NoticeReceipt,
    NoticeReceiptPage, NoticeReceiptState, NoticeSummary, PublishNoticeCommand,
    UpdateDraftNoticeCommand, notice_audit_event,
};
use console_notices_domain::{NewNotice, NoticeAudience, NoticeBody, NoticeCategory, NoticeTitle};
use console_notifications_application::{EmitNotificationCommand, NotificationSink};
use console_notifications_domain::NotificationLink;
use console_platform_db::{DbError, issue_code, with_audit, with_audits, with_org_conn};
use console_platform_request_context::current_org;
use sqlx::{PgPool, Row};
use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
pub enum PgNoticeError {
    #[error(transparent)]
    Db(#[from] DbError),

    #[error(transparent)]
    Domain(#[from] KernelError),
}

impl PgNoticeError {
    #[must_use]
    pub fn kind(&self) -> ErrorKind {
        match self {
            Self::Domain(err) => err.kind,
            Self::Db(DbError::Sqlx(sqlx::Error::RowNotFound)) => ErrorKind::NotFound,
            Self::Db(_) => ErrorKind::Internal,
        }
    }
}

impl From<sqlx::Error> for PgNoticeError {
    fn from(value: sqlx::Error) -> Self {
        Self::Db(DbError::Sqlx(value))
    }
}

impl From<PgNoticeError> for KernelError {
    fn from(value: PgNoticeError) -> Self {
        match value {
            PgNoticeError::Domain(err) => err,
            PgNoticeError::Db(err) => KernelError::internal(err.to_string()),
        }
    }
}

/// Shared hydrated-summary SELECT: audience branch names (대상), the viewer's
/// own receipt state (확인), and per-notice 수령확인 progress — one query, no
/// N+1. `$1` is always the viewer. Progress is computed for every row and
/// dropped in Rust for non-manager callers before serialization.
// ponytail: progress is an indexed per-row aggregate at LIMIT<=200; split the
// SQL per caller tier only if list latency ever measurably says so.
const SUMMARY_SELECT: &str = r"
    SELECT n.id, n.code, n.author_user_id, n.title, n.body, n.status,
           n.published_at, n.created_at, n.category, n.audience_scope,
           ab.branch_ids AS audience_branch_ids,
           ab.branch_names AS audience_branch_names,
           (my.recipient_user_id IS NOT NULL) AS my_is_recipient,
           my.acknowledged_at AS my_acknowledged_at,
           prog.total AS progress_total,
           prog.acknowledged AS progress_acknowledged
    FROM notices n
    LEFT JOIN notice_receipts my
           ON my.notice_id = n.id AND my.recipient_user_id = $1
    LEFT JOIN LATERAL (
        SELECT array_agg(b.id ORDER BY b.name, b.id) AS branch_ids,
               array_agg(b.name ORDER BY b.name, b.id) AS branch_names
        FROM notice_audience_branches nab
        JOIN branches b ON b.id = nab.branch_id AND b.org_id = nab.org_id
        WHERE nab.notice_id = n.id
    ) ab ON TRUE
    LEFT JOIN LATERAL (
        SELECT COUNT(*) AS total,
               COUNT(*) FILTER (WHERE r.acknowledged_at IS NOT NULL) AS acknowledged
        FROM notice_receipts r
        WHERE r.notice_id = n.id
    ) prog ON TRUE
";

#[derive(Clone)]
pub struct PgNoticeStore {
    pool: PgPool,
    notification_sink: Option<Arc<dyn NotificationSink>>,
}

impl std::fmt::Debug for PgNoticeStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PgNoticeStore")
            .field("pool", &self.pool)
            .field("has_notification_sink", &self.notification_sink.is_some())
            .finish()
    }
}

impl PgNoticeStore {
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self {
            pool,
            notification_sink: None,
        }
    }

    #[must_use]
    pub fn with_notification_sink(mut self, sink: Arc<dyn NotificationSink>) -> Self {
        self.notification_sink = Some(sink);
        self
    }

    #[must_use]
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    pub async fn create_draft(
        &self,
        command: CreateDraftNoticeCommand,
    ) -> Result<NoticeSummary, PgNoticeError> {
        let category = command
            .category
            .as_deref()
            .map(NoticeCategory::parse)
            .transpose()?
            .unwrap_or_default();
        let audience = command
            .audience
            .map(|input| NoticeAudience::new(&input.scope, input.branch_ids))
            .transpose()?
            .unwrap_or_default();
        let new_notice = NewNotice::new(&command.title, &command.body, category, audience)?;
        let org = current_org().map_err(KernelError::from)?;
        let notice_id = NoticeId::new();
        let notice_uuid = *notice_id.as_uuid();
        let author_uuid = *command.author.as_uuid();
        let org_uuid = *org.as_uuid();

        let event = notice_audit_event(
            "notice.create_draft",
            Some(command.author),
            notice_id,
            command.trace,
            command.occurred_at,
        )?
        .with_org(org);

        let title = new_notice.title.into_string();
        let body = new_notice.body.into_string();
        let category_str = new_notice.category.as_str();
        let scope_str = new_notice.audience.scope_str();
        let branch_uuids = branch_uuids(&new_notice.audience);
        let occurred_at = command.occurred_at;

        with_audit::<_, NoticeSummary, PgNoticeError>(&self.pool, event, move |tx| {
            Box::pin(async move {
                sqlx::query(
                    r"
                    INSERT INTO notices
                        (id, org_id, author_user_id, title, body, category,
                         audience_scope, created_at, updated_at)
                    VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $8)
                    ",
                )
                .bind(notice_uuid)
                .bind(org_uuid)
                .bind(author_uuid)
                .bind(title)
                .bind(body)
                .bind(category_str)
                .bind(scope_str)
                .bind(occurred_at)
                .execute(tx.as_mut())
                .await?;
                insert_audience_rows(tx.as_mut(), org_uuid, notice_uuid, &branch_uuids).await?;
                require_summary(tx.as_mut(), author_uuid, notice_uuid, true).await
            })
        })
        .await
    }

    /// Draft-only edit: every field optional, the audience replaced whole.
    /// The row is locked (`FOR UPDATE`) so an edit can never race a publish;
    /// once published the notice is frozen and the edit is a Conflict.
    pub async fn update_draft(
        &self,
        command: UpdateDraftNoticeCommand,
    ) -> Result<NoticeSummary, PgNoticeError> {
        let title = command
            .title
            .as_deref()
            .map(NoticeTitle::new)
            .transpose()?
            .map(NoticeTitle::into_string);
        let body = command
            .body
            .as_deref()
            .map(NoticeBody::new)
            .transpose()?
            .map(NoticeBody::into_string);
        let category = command
            .category
            .as_deref()
            .map(NoticeCategory::parse)
            .transpose()?
            .map(NoticeCategory::as_str);
        let audience = command
            .audience
            .map(|input| NoticeAudience::new(&input.scope, input.branch_ids))
            .transpose()?;

        let org = current_org().map_err(KernelError::from)?;
        let org_uuid = *org.as_uuid();
        let notice_uuid = *command.notice_id.as_uuid();
        let editor_uuid = *command.editor.as_uuid();
        let occurred_at = command.occurred_at;

        let event = notice_audit_event(
            "notice.update_draft",
            Some(command.editor),
            command.notice_id,
            command.trace,
            command.occurred_at,
        )?
        .with_org(org);

        let audience_replace = audience
            .as_ref()
            .map(|audience| (audience.scope_str(), branch_uuids(audience)));

        with_audit::<_, NoticeSummary, PgNoticeError>(&self.pool, event, move |tx| {
            Box::pin(async move {
                let current_status: Option<String> =
                    sqlx::query_scalar("SELECT status FROM notices WHERE id = $1 FOR UPDATE")
                        .bind(notice_uuid)
                        .fetch_optional(tx.as_mut())
                        .await?;
                match current_status.as_deref() {
                    None => return Err(KernelError::not_found("notice not found").into()),
                    Some("draft") => {}
                    Some(_) => {
                        return Err(KernelError::conflict(
                            "a published notice is frozen and cannot be edited",
                        )
                        .into());
                    }
                }

                let (scope, branches) = match &audience_replace {
                    Some((scope, branches)) => (Some(*scope), Some(branches)),
                    None => (None, None),
                };
                sqlx::query(
                    r"
                    UPDATE notices
                    SET title = COALESCE($2, title),
                        body = COALESCE($3, body),
                        category = COALESCE($4, category),
                        audience_scope = COALESCE($5, audience_scope),
                        updated_at = $6
                    WHERE id = $1
                    ",
                )
                .bind(notice_uuid)
                .bind(title)
                .bind(body)
                .bind(category)
                .bind(scope)
                .bind(occurred_at)
                .execute(tx.as_mut())
                .await?;

                if let Some(branches) = branches {
                    sqlx::query("DELETE FROM notice_audience_branches WHERE notice_id = $1")
                        .bind(notice_uuid)
                        .execute(tx.as_mut())
                        .await?;
                    insert_audience_rows(tx.as_mut(), org_uuid, notice_uuid, branches).await?;
                }
                require_summary(tx.as_mut(), editor_uuid, notice_uuid, true).await
            })
        })
        .await
    }

    /// `manager` = the caller holds the publish tier ([`Feature::NoticeManage`]
    /// resolved at REST): it gates draft visibility (a non-manager gets
    /// NotFound, not Forbidden, for a draft — the notifications cross-user
    /// isolation idiom) AND per-row `progress` hydration.
    pub async fn get(
        &self,
        query: GetNoticeQuery,
        manager: bool,
    ) -> Result<NoticeSummary, PgNoticeError> {
        let org = current_org().map_err(KernelError::from)?;
        let notice_uuid = *query.notice_id.as_uuid();
        let viewer_uuid = *query.viewer.as_uuid();

        let summary = with_org_conn::<_, _, PgNoticeError>(&self.pool, org, move |tx| {
            Box::pin(
                async move { fetch_summary(tx.as_mut(), viewer_uuid, notice_uuid, manager).await },
            )
        })
        .await?;

        let summary = summary.ok_or_else(|| KernelError::not_found("notice not found"))?;
        if summary.status == "draft" && !manager {
            return Err(KernelError::not_found("notice not found").into());
        }
        Ok(summary)
    }

    pub async fn list(&self, query: ListNoticesQuery) -> Result<Vec<NoticeSummary>, PgNoticeError> {
        let org = current_org().map_err(KernelError::from)?;
        let limit = query.limit.clamp(1, 200);
        let include_drafts = query.include_drafts;
        let viewer_uuid = *query.viewer.as_uuid();

        with_org_conn::<_, _, PgNoticeError>(&self.pool, org, move |tx| {
            Box::pin(async move {
                // Composed purely from the SUMMARY_SELECT const — no
                // request-derived text — so AssertSqlSafe is sound.
                let sql = sqlx::AssertSqlSafe(format!(
                    "{SUMMARY_SELECT} WHERE ($2 OR n.status = 'published') \
                     ORDER BY n.created_at DESC LIMIT $3"
                ));
                let rows = sqlx::query(sql)
                    .bind(viewer_uuid)
                    .bind(include_drafts)
                    .bind(limit)
                    .fetch_all(tx.as_mut())
                    .await?;
                rows.iter()
                    .map(|row| summary_from_row(row, include_drafts))
                    .collect()
            })
        })
        .await
    }

    /// Transition a draft to published in ONE audited transaction: lock the
    /// row (`FOR UPDATE` — 404 missing / 409 already published), snapshot the
    /// effective audience (org-wide, or the members of its audience branches
    /// via `user_branches`) into `notice_receipts`, fail closed if the
    /// snapshot came out empty, issue the canonical NT- code, and flip the
    /// status. A failure at ANY step rolls the whole publish back — there is
    /// no window where a notice is published with zero receipts and its
    /// republish path dead on the 409 guard. Then — best-effort, post-commit —
    /// fan out a notification to each snapshotted recipient.
    pub async fn publish(
        &self,
        command: PublishNoticeCommand,
    ) -> Result<NoticeSummary, PgNoticeError> {
        let org = current_org().map_err(KernelError::from)?;
        let org_uuid = *org.as_uuid();
        let notice_uuid = *command.notice_id.as_uuid();
        let publisher_uuid = *command.publisher.as_uuid();
        let occurred_at = command.occurred_at;
        let notify_trace = command.trace.clone();

        let publish_event = notice_audit_event(
            "notice.publish",
            Some(command.publisher),
            command.notice_id,
            command.trace.clone(),
            command.occurred_at,
        )?
        .with_org(org);
        let recipients_event = notice_audit_event(
            "notice.publish_recipients",
            Some(command.publisher),
            command.notice_id,
            command.trace,
            command.occurred_at,
        )?
        .with_org(org);

        let (title, recipient_ids) =
            with_audits::<_, (String, Vec<UserId>), PgNoticeError>(&self.pool, org, move |tx| {
                Box::pin(async move {
                    let current: Option<(String, String)> = sqlx::query_as(
                        "SELECT status, audience_scope FROM notices WHERE id = $1 FOR UPDATE",
                    )
                    .bind(notice_uuid)
                    .fetch_optional(tx.as_mut())
                    .await?;
                    let audience_scope = match current {
                        None => return Err(KernelError::not_found("notice not found").into()),
                        Some((status, scope)) if status == "draft" => scope,
                        Some(_) => {
                            return Err(KernelError::conflict("notice is already published").into());
                        }
                    };

                    let snapshot_sql = if audience_scope == "branches" {
                        r"
                        INSERT INTO notice_receipts (org_id, notice_id, recipient_user_id)
                        SELECT DISTINCT $1, $2, u.id
                        FROM users u
                        JOIN user_branches ub
                          ON ub.user_id = u.id AND ub.org_id = u.org_id
                        JOIN notice_audience_branches nab
                          ON nab.notice_id = $2 AND nab.org_id = $1
                         AND nab.branch_id = ub.branch_id
                        WHERE u.org_id = $1 AND u.is_active = true
                        ON CONFLICT (notice_id, recipient_user_id) DO NOTHING
                        RETURNING recipient_user_id
                        "
                    } else {
                        r"
                        INSERT INTO notice_receipts (org_id, notice_id, recipient_user_id)
                        SELECT $1, $2, id FROM users WHERE org_id = $1 AND is_active = true
                        ON CONFLICT (notice_id, recipient_user_id) DO NOTHING
                        RETURNING recipient_user_id
                        "
                    };
                    let rows = sqlx::query(snapshot_sql)
                        .bind(org_uuid)
                        .bind(notice_uuid)
                        .fetch_all(tx.as_mut())
                        .await?;
                    let recipient_ids = rows
                        .iter()
                        .map(|row| Ok(UserId::from_uuid(row.try_get("recipient_user_id")?)))
                        .collect::<Result<Vec<_>, PgNoticeError>>()?;

                    // Fail closed: publishing to nobody is a validation error,
                    // never a silent empty snapshot. The rollback un-publishes
                    // the notice AND discards the receipt rows atomically.
                    if recipient_ids.is_empty() {
                        return Err(KernelError::validation(
                            "the notice's effective audience is empty",
                        )
                        .into());
                    }

                    let code = issue_code(tx, org, "notification").await?;
                    let title: String = sqlx::query_scalar(
                        r"
                        UPDATE notices
                        SET status = 'published', code = $2, published_at = $3, updated_at = $3
                        WHERE id = $1
                        RETURNING title
                        ",
                    )
                    .bind(notice_uuid)
                    .bind(code)
                    .bind(occurred_at)
                    .fetch_one(tx.as_mut())
                    .await?;
                    Ok((
                        (title, recipient_ids),
                        Vec::from([publish_event, recipients_event]),
                    ))
                })
            })
            .await?;

        if let Some(sink) = &self.notification_sink {
            let link = NotificationLink::Object {
                kind: "notice".to_owned(),
                id: notice_uuid.to_string(),
            };
            for recipient in recipient_ids {
                let _ = sink
                    .emit(EmitNotificationCommand {
                        actor: Some(command.publisher),
                        recipient,
                        category: "공지".to_owned(),
                        kind: "info".to_owned(),
                        text: title.clone(),
                        link: link.clone(),
                        dedup_key: Some(format!("notice-publish-{notice_uuid}-{recipient}")),
                        trace: notify_trace.clone(),
                        occurred_at,
                    })
                    .await;
            }
        }

        let summary = with_org_conn::<_, _, PgNoticeError>(&self.pool, org, move |tx| {
            Box::pin(
                async move { fetch_summary(tx.as_mut(), publisher_uuid, notice_uuid, true).await },
            )
        })
        .await?;
        summary.ok_or_else(|| KernelError::internal("published notice vanished mid-read").into())
    }

    /// Record a recipient's 수령확인. NotFound when the caller was never
    /// snapshotted as a recipient (unpublished notice, outside the audience,
    /// or not an org member at publish time) — mirrors the notifications
    /// cross-user isolation idiom rather than distinguishing "not found"
    /// from "not yours".
    pub async fn acknowledge(
        &self,
        command: AcknowledgeNoticeCommand,
    ) -> Result<(), PgNoticeError> {
        let org = current_org().map_err(KernelError::from)?;
        let notice_uuid = *command.notice_id.as_uuid();
        let recipient_uuid = *command.recipient.as_uuid();
        let occurred_at = command.occurred_at;

        let event = notice_audit_event(
            "notice.acknowledge",
            Some(command.recipient),
            command.notice_id,
            command.trace,
            command.occurred_at,
        )?
        .with_org(org);

        with_audit::<_, (), PgNoticeError>(&self.pool, event, move |tx| {
            Box::pin(async move {
                let result = sqlx::query(
                    r"
                    UPDATE notice_receipts
                    SET acknowledged_at = COALESCE(acknowledged_at, $3)
                    WHERE notice_id = $1 AND recipient_user_id = $2
                    ",
                )
                .bind(notice_uuid)
                .bind(recipient_uuid)
                .bind(occurred_at)
                .execute(tx.as_mut())
                .await?;
                if result.rows_affected() == 0 {
                    return Err(KernelError::not_found(
                        "notice not found, or you are not a recipient",
                    )
                    .into());
                }
                Ok(())
            })
        })
        .await
    }

    pub async fn progress(
        &self,
        query: NoticeProgressQuery,
    ) -> Result<NoticeProgress, PgNoticeError> {
        let org = current_org().map_err(KernelError::from)?;
        let notice_uuid = *query.notice_id.as_uuid();

        with_org_conn::<_, _, PgNoticeError>(&self.pool, org, move |tx| {
            Box::pin(async move {
                // 404 for a notice that does not exist (in this org) — never a
                // fabricated 0/0 (openapi declares 404 on the progress route).
                let exists: Option<i32> = sqlx::query_scalar("SELECT 1 FROM notices WHERE id = $1")
                    .bind(notice_uuid)
                    .fetch_optional(tx.as_mut())
                    .await?;
                if exists.is_none() {
                    return Err(KernelError::not_found("notice not found").into());
                }

                let row = sqlx::query(
                    r"
                    SELECT COUNT(*) AS total,
                           COUNT(*) FILTER (WHERE acknowledged_at IS NOT NULL) AS acknowledged
                    FROM notice_receipts
                    WHERE notice_id = $1
                    ",
                )
                .bind(notice_uuid)
                .fetch_one(tx.as_mut())
                .await?;
                Ok(NoticeProgress {
                    total: row.try_get("total")?,
                    acknowledged: row.try_get("acknowledged")?,
                })
            })
        })
        .await
    }

    /// Manager-only receipts drill: every snapshotted recipient with their
    /// display name and ack state; `acknowledged = Some(false)` is the
    /// outstanding chase list. Newest-ack-first, then name.
    pub async fn list_receipts(
        &self,
        query: ListNoticeReceiptsQuery,
    ) -> Result<NoticeReceiptPage, PgNoticeError> {
        let org = current_org().map_err(KernelError::from)?;
        let notice_uuid = *query.notice_id.as_uuid();
        let acknowledged = query.acknowledged;
        let limit = query.limit.clamp(1, 200);
        let offset = query.offset.max(0);

        with_org_conn::<_, _, PgNoticeError>(&self.pool, org, move |tx| {
            Box::pin(async move {
                let exists: Option<i32> = sqlx::query_scalar("SELECT 1 FROM notices WHERE id = $1")
                    .bind(notice_uuid)
                    .fetch_optional(tx.as_mut())
                    .await?;
                if exists.is_none() {
                    return Err(KernelError::not_found("notice not found").into());
                }

                let total: i64 = sqlx::query_scalar(
                    r"
                    SELECT COUNT(*)
                    FROM notice_receipts
                    WHERE notice_id = $1
                      AND ($2::boolean IS NULL OR (acknowledged_at IS NOT NULL) = $2)
                    ",
                )
                .bind(notice_uuid)
                .bind(acknowledged)
                .fetch_one(tx.as_mut())
                .await?;

                let rows = sqlx::query(
                    r"
                    SELECT r.recipient_user_id, u.display_name, r.acknowledged_at
                    FROM notice_receipts r
                    JOIN users u ON u.id = r.recipient_user_id AND u.org_id = r.org_id
                    WHERE r.notice_id = $1
                      AND ($2::boolean IS NULL OR (r.acknowledged_at IS NOT NULL) = $2)
                    ORDER BY r.acknowledged_at DESC NULLS LAST,
                             u.display_name, r.recipient_user_id
                    LIMIT $3 OFFSET $4
                    ",
                )
                .bind(notice_uuid)
                .bind(acknowledged)
                .bind(limit)
                .bind(offset)
                .fetch_all(tx.as_mut())
                .await?;
                let items = rows
                    .iter()
                    .map(|row| {
                        Ok(NoticeReceipt {
                            recipient_user_id: UserId::from_uuid(row.try_get("recipient_user_id")?),
                            display_name: row.try_get("display_name")?,
                            acknowledged_at: row.try_get("acknowledged_at")?,
                        })
                    })
                    .collect::<Result<Vec<_>, PgNoticeError>>()?;
                Ok(NoticeReceiptPage { items, total })
            })
        })
        .await
    }
}

fn branch_uuids(audience: &NoticeAudience) -> Vec<Uuid> {
    audience
        .branch_ids()
        .iter()
        .map(|branch| *branch.as_uuid())
        .collect()
}

/// Insert the audience rows for a draft. A branch id outside the caller's org
/// trips the composite `(branch_id, org_id)` FK (RLS keeps foreign rows
/// invisible anyway) and surfaces as fail-closed validation, not a 500.
async fn insert_audience_rows(
    conn: &mut sqlx::PgConnection,
    org_uuid: Uuid,
    notice_uuid: Uuid,
    branch_uuids: &[Uuid],
) -> Result<(), PgNoticeError> {
    if branch_uuids.is_empty() {
        return Ok(());
    }
    sqlx::query(
        r"
        INSERT INTO notice_audience_branches (org_id, notice_id, branch_id)
        SELECT $1, $2, unnest($3::uuid[])
        ",
    )
    .bind(org_uuid)
    .bind(notice_uuid)
    .bind(branch_uuids)
    .execute(conn)
    .await
    .map_err(|err| -> PgNoticeError {
        match &err {
            sqlx::Error::Database(db) if db.code().as_deref() == Some("23503") => {
                PgNoticeError::Domain(KernelError::validation(
                    "audience branch does not belong to this organization",
                ))
            }
            _ => err.into(),
        }
    })?;
    Ok(())
}

async fn fetch_summary(
    conn: &mut sqlx::PgConnection,
    viewer_uuid: Uuid,
    notice_uuid: Uuid,
    include_progress: bool,
) -> Result<Option<NoticeSummary>, PgNoticeError> {
    // Composed purely from the SUMMARY_SELECT const — no request-derived
    // text — so AssertSqlSafe is sound.
    let sql = sqlx::AssertSqlSafe(format!("{SUMMARY_SELECT} WHERE n.id = $2"));
    let row = sqlx::query(sql)
        .bind(viewer_uuid)
        .bind(notice_uuid)
        .fetch_optional(conn)
        .await?;
    row.as_ref()
        .map(|row| summary_from_row(row, include_progress))
        .transpose()
}

async fn require_summary(
    conn: &mut sqlx::PgConnection,
    viewer_uuid: Uuid,
    notice_uuid: Uuid,
    include_progress: bool,
) -> Result<NoticeSummary, PgNoticeError> {
    fetch_summary(conn, viewer_uuid, notice_uuid, include_progress)
        .await?
        .ok_or_else(|| KernelError::internal("notice vanished inside its own transaction").into())
}

fn summary_from_row(
    row: &sqlx::postgres::PgRow,
    include_progress: bool,
) -> Result<NoticeSummary, PgNoticeError> {
    let branch_ids: Option<Vec<Uuid>> = row.try_get("audience_branch_ids")?;
    let branch_names: Option<Vec<String>> = row.try_get("audience_branch_names")?;
    let audience_branches = match (branch_ids, branch_names) {
        (Some(ids), Some(names)) => ids
            .into_iter()
            .zip(names)
            .map(|(id, name)| NoticeAudienceBranch {
                id: BranchId::from_uuid(id),
                name,
            })
            .collect(),
        _ => Vec::new(),
    };
    let my_receipt = if row.try_get::<bool, _>("my_is_recipient")? {
        Some(NoticeReceiptState {
            acknowledged_at: row.try_get("my_acknowledged_at")?,
        })
    } else {
        None
    };
    let progress = if include_progress {
        Some(NoticeProgress {
            total: row.try_get("progress_total")?,
            acknowledged: row.try_get("progress_acknowledged")?,
        })
    } else {
        None
    };
    Ok(NoticeSummary {
        id: NoticeId::from_uuid(row.try_get("id")?),
        code: row.try_get("code")?,
        author_user_id: UserId::from_uuid(row.try_get("author_user_id")?),
        title: row.try_get("title")?,
        body: row.try_get("body")?,
        status: row.try_get("status")?,
        published_at: row.try_get("published_at")?,
        created_at: row.try_get("created_at")?,
        category: row.try_get("category")?,
        audience_scope: row.try_get("audience_scope")?,
        audience_branches,
        my_receipt,
        progress,
    })
}
