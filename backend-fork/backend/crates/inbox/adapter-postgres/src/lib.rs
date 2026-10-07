//! Postgres InboxDoc adapter.
//!
//! Recipient scoping is enforced here in code (there is no per-person GUC): the
//! caller passes the authenticated principal's `UserId`, and every query filters
//! `recipient_user_id`. RLS narrows to the tenant on top. A cross-user read or
//! confirm therefore returns *nothing* (or NotFound), never another recipient's
//! row — deny-by-omission.
//!
//! A locked legal notice (`legal_notice` && not yet confirmed) never yields its
//! `payload` through [`PgInboxStore::get`]; reading it is a pure read that never
//! auto-confirms. Confirmation is a separate, audited, idempotent UPDATE.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

use console_inbox_application::{
    ConfirmReceiptCommand, EmitInboxDocCommand, EmitInboxDocFuture, GetInboxDocQuery,
    InboxDocDetail, InboxDocFilter, InboxDocPage, InboxDocSink, InboxDocSummary,
    ListInboxDocsQuery, inbox_doc_audit_event,
};
use console_inbox_domain::{InboxDocKind, NewInboxDoc};
use console_kernel_core::{ErrorKind, InboxDocId, KernelError, OrgId, UserId};
use console_platform_db::{DbError, insert_audit_event, with_audit, with_org_conn};
use console_platform_request_context::current_org;
use sqlx::{PgPool, Postgres, QueryBuilder, Row, Transaction};

/// The summary column list (no `payload`). `get` appends `, payload` for the
/// single-document read. Kept as one `&'static str` so sqlx's SQL-injection
/// guard is satisfied and the shared shape is defined once.
const COLUMNS: &str = "id, recipient_user_id, kind, notice_type, title, legal_basis, \
     source_kind, source_id, confirmed_by, confirmed_at, created_at";

#[derive(Debug, thiserror::Error)]
pub enum PgInboxError {
    #[error(transparent)]
    Db(#[from] DbError),

    #[error(transparent)]
    Domain(#[from] KernelError),

    /// Legacy error variant retained for adapter consumers. Emission now
    /// validates dedup collisions within the caller's transaction.
    #[error("inbox document dedup conflict")]
    Dedup,
}

impl PgInboxError {
    #[must_use]
    pub fn kind(&self) -> ErrorKind {
        match self {
            Self::Domain(err) => err.kind,
            Self::Db(DbError::Sqlx(sqlx::Error::RowNotFound)) => ErrorKind::NotFound,
            Self::Dedup | Self::Db(_) => ErrorKind::Internal,
        }
    }
}

impl From<sqlx::Error> for PgInboxError {
    fn from(value: sqlx::Error) -> Self {
        Self::Db(DbError::Sqlx(value))
    }
}

impl From<PgInboxError> for KernelError {
    fn from(value: PgInboxError) -> Self {
        match value {
            PgInboxError::Domain(err) => err,
            PgInboxError::Dedup => {
                KernelError::internal("inbox document dedup conflict escaped emit")
            }
            PgInboxError::Db(err) => KernelError::internal(err.to_string()),
        }
    }
}

#[derive(Clone, Debug)]
pub struct PgInboxStore {
    pool: PgPool,
}

impl PgInboxStore {
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    #[must_use]
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    /// Deliver a validated document. Exact redelivery preserves the document
    /// and receipt without another audit; changed immutable input conflicts.
    pub async fn emit_inbox_doc(
        &self,
        command: EmitInboxDocCommand,
    ) -> Result<InboxDocSummary, PgInboxError> {
        let org = current_org().map_err(KernelError::from)?;
        with_org_conn::<_, _, PgInboxError>(&self.pool, org, move |tx| {
            Box::pin(Self::emit_inbox_doc_in_tx(tx, org, command))
        })
        .await
    }

    /// Compose delivery and its audit with an existing Company-armed
    /// transaction. This helper never commits or broadens the RLS context.
    pub async fn emit_inbox_doc_in_tx(
        tx: &mut Transaction<'_, Postgres>,
        org: OrgId,
        command: EmitInboxDocCommand,
    ) -> Result<InboxDocSummary, PgInboxError> {
        let recipient_uuid = *command.recipient.as_uuid();
        let doc = &command.doc;
        let id = InboxDocId::new();
        let inserted = sqlx::query(
            r#"
            INSERT INTO inbox_docs (
                id, org_id, recipient_user_id, kind, notice_type, title,
                payload, legal_basis, source_kind, source_id, dedup_key
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
            ON CONFLICT (org_id, recipient_user_id, dedup_key)
                WHERE dedup_key IS NOT NULL DO NOTHING
            RETURNING id, recipient_user_id, kind, notice_type, title, legal_basis,
                      source_kind, source_id, confirmed_by, confirmed_at, created_at
            "#,
        )
        .bind(id.as_uuid())
        .bind(org.as_uuid())
        .bind(recipient_uuid)
        .bind(doc.kind.as_str())
        .bind(&doc.notice_type)
        .bind(&doc.title)
        .bind(&doc.payload)
        .bind(&doc.legal_basis)
        .bind(&doc.source_kind)
        .bind(&doc.source_id)
        .bind(&command.dedup_key)
        .fetch_optional(tx.as_mut())
        .await?;

        let Some(row) = inserted else {
            // ON CONFLICT waits for concurrent insertion. The next statement
            // reads and locks its committed artifact, including its payload.
            let key = command
                .dedup_key
                .as_deref()
                .ok_or_else(|| KernelError::internal("dedup collision without a dedup key"))?;
            let row = sqlx::query(
                "SELECT id, recipient_user_id, kind, notice_type, title, legal_basis, \
                        source_kind, source_id, confirmed_by, confirmed_at, created_at, payload \
                 FROM inbox_docs WHERE org_id = $1 AND recipient_user_id = $2 AND dedup_key = $3 \
                 FOR UPDATE",
            )
            .bind(org.as_uuid())
            .bind(recipient_uuid)
            .bind(key)
            .fetch_optional(tx.as_mut())
            .await?
            .ok_or_else(|| KernelError::conflict("inbox document dedup conflict"))?;
            // Read raw fields: the constructor would trim corrupt historical
            // metadata and could turn a changed artifact into a matching one.
            let stored = NewInboxDoc {
                kind: InboxDocKind::parse(row.try_get::<String, _>("kind")?.as_str())?,
                title: row.try_get("title")?,
                notice_type: row.try_get("notice_type")?,
                legal_basis: row.try_get("legal_basis")?,
                source_kind: row.try_get("source_kind")?,
                source_id: row.try_get("source_id")?,
                payload: row.try_get("payload")?,
            };
            if stored != *doc {
                return Err(KernelError::conflict("inbox document dedup conflict").into());
            }
            return summary_from_row(&row);
        };
        let event = inbox_doc_audit_event(
            "inbox_doc.emit",
            command.actor,
            id,
            command.trace,
            command.occurred_at,
        )?
        .with_org(org)
        .with_snapshots(
            None,
            Some(serde_json::json!({
                "kind": doc.kind.as_str(),
                "notice_type": doc.notice_type,
                "recipient_user_id": recipient_uuid,
            })),
        );
        insert_audit_event(tx, &event).await?;
        summary_from_row(&row)
    }

    /// List the caller's documents, newest first, keyset-paginated. Metadata
    /// only — payloads never appear in the list, so a locked legal notice's body
    /// is never disclosed here.
    pub async fn list(&self, query: ListInboxDocsQuery) -> Result<InboxDocPage, PgInboxError> {
        self.list_inner(query, false).await
    }

    /// Payslip-only self-service. Other document kinds cannot anchor this view.
    pub async fn list_payslips(
        &self,
        recipient: UserId,
        before_id: Option<InboxDocId>,
        limit: i64,
    ) -> Result<InboxDocPage, PgInboxError> {
        self.list_inner(
            ListInboxDocsQuery {
                recipient,
                filter: InboxDocFilter::Payslip,
                before_id,
                limit,
            },
            true,
        )
        .await
    }

    async fn list_inner(
        &self,
        query: ListInboxDocsQuery,
        payslip_anchor: bool,
    ) -> Result<InboxDocPage, PgInboxError> {
        let limit = query.limit.clamp(1, 200);
        let recipient_uuid = *query.recipient.as_uuid();
        let org = current_org().map_err(KernelError::from)?;

        let rows = with_org_conn::<_, _, PgInboxError>(&self.pool, org, move |tx| {
            Box::pin(async move {
                let mut builder = QueryBuilder::<Postgres>::new(format!(
                    "SELECT {COLUMNS} FROM inbox_docs WHERE recipient_user_id = "
                ));
                builder.push_bind(recipient_uuid);
                match query.filter {
                    InboxDocFilter::ActionRequired => {
                        builder.push(" AND kind = 'legal_notice' AND confirmed_at IS NULL");
                    }
                    InboxDocFilter::Payslip => {
                        builder.push(" AND kind = 'payslip'");
                    }
                    InboxDocFilter::Done => {
                        builder.push(" AND confirmed_at IS NOT NULL");
                    }
                    InboxDocFilter::All => {}
                }
                if let Some(before_id) = query.before_id {
                    // Keyset: strictly older than the cursor row. A cursor that
                    // is not the caller's own row makes the subquery empty, so
                    // the comparison is NULL and the page is empty (fail-closed).
                    builder.push(" AND (created_at, id) < (SELECT created_at, id FROM inbox_docs WHERE id = ");
                    builder.push_bind(*before_id.as_uuid());
                    builder.push(" AND recipient_user_id = ");
                    builder.push_bind(recipient_uuid);
                    if payslip_anchor {
                        builder.push(" AND kind = 'payslip'");
                    }
                    builder.push(")");
                }
                builder.push(" ORDER BY created_at DESC, id DESC LIMIT ");
                // A full page is terminal unless another eligible row exists.
                builder.push_bind(limit + 1);
                Ok(builder.build().fetch_all(tx.as_mut()).await?)
            })
        })
        .await?;

        let items = rows
            .iter()
            .take(limit as usize)
            .map(summary_from_row)
            .collect::<Result<Vec<_>, _>>()?;
        let next_cursor = (rows.len() as i64 > limit)
            .then(|| items.last().map(|item| item.id))
            .flatten();
        Ok(InboxDocPage { items, next_cursor })
    }

    /// Read one of the caller's documents. Returns NotFound when the id is
    /// unknown *or* owned by another recipient (indistinguishable — the
    /// cross-user isolation guarantee). A LOCKED legal notice's `payload` is
    /// withheld (`None`); this is a pure read and never confirms receipt.
    pub async fn get(&self, query: GetInboxDocQuery) -> Result<InboxDocDetail, PgInboxError> {
        self.get_inner(query, None).await
    }

    /// Restrict kind in SQL before any legal-notice payload is retrieved.
    pub async fn get_payslip(
        &self,
        query: GetInboxDocQuery,
    ) -> Result<InboxDocDetail, PgInboxError> {
        self.get_inner(query, Some("payslip")).await
    }

    async fn get_inner(
        &self,
        query: GetInboxDocQuery,
        kind: Option<&'static str>,
    ) -> Result<InboxDocDetail, PgInboxError> {
        let org = current_org().map_err(KernelError::from)?;
        let recipient_uuid = *query.recipient.as_uuid();
        let id_uuid = *query.id.as_uuid();

        let row = with_org_conn::<_, _, PgInboxError>(&self.pool, org, move |tx| {
            Box::pin(async move {
                Ok(sqlx::query(
                    "SELECT id, recipient_user_id, kind, notice_type, title, legal_basis, \
                     source_kind, source_id, confirmed_by, confirmed_at, created_at, payload \
                     FROM inbox_docs WHERE id = $1 AND recipient_user_id = $2 \
                     AND ($3::text IS NULL OR kind = $3)",
                )
                .bind(id_uuid)
                .bind(recipient_uuid)
                .bind(kind)
                .fetch_optional(tx.as_mut())
                .await?)
            })
        })
        .await?;

        let row = row.ok_or_else(|| KernelError::not_found("inbox document not found"))?;
        let summary = summary_from_row(&row)?;
        // Lock gate: a legal notice's body is disclosed only after receipt.
        let payload = if summary.locked {
            None
        } else {
            Some(row.try_get::<serde_json::Value, _>("payload")?)
        };
        Ok(InboxDocDetail { summary, payload })
    }

    /// Confirm receipt of a legal notice — the legal receipt evidence. The REST
    /// layer verifies a fresh passkey step-up before calling this. Idempotent:
    /// a second confirm returns the existing stamp without re-auditing or
    /// changing `confirmed_at`. A payslip cannot be receipt-confirmed
    /// (validation error); another user's / unknown id is NotFound.
    pub async fn confirm_receipt(
        &self,
        command: ConfirmReceiptCommand,
    ) -> Result<InboxDocSummary, PgInboxError> {
        let org = current_org().map_err(KernelError::from)?;
        let recipient_uuid = *command.recipient.as_uuid();
        let id_uuid = *command.doc_id.as_uuid();

        // Read-first to classify the target before mutating: NotFound vs. a
        // non-legal doc vs. an already-confirmed (idempotent) doc. This keeps
        // the double-confirm path from emitting a second audit event.
        let existing = self
            .get(GetInboxDocQuery {
                recipient: command.recipient,
                id: command.doc_id,
            })
            .await?
            .summary;
        if existing.kind != InboxDocKind::LegalNotice {
            return Err(KernelError::validation(
                "only a legal notice can be receipt-confirmed; a payslip is a frictionless self-view",
            )
            .into());
        }
        if existing.confirmed_at.is_some() {
            // Already confirmed — idempotent no-op, no second receipt event.
            return Ok(existing);
        }

        let occurred_at = command.occurred_at;
        let event = inbox_doc_audit_event(
            "inbox_doc.confirm_receipt",
            Some(command.recipient),
            command.doc_id,
            command.trace,
            command.occurred_at,
        )?
        .with_org(org)
        .with_snapshots(
            Some(serde_json::json!({ "confirmed_at": null })),
            Some(serde_json::json!({
                "confirmed_by": recipient_uuid,
                "confirmed_at": occurred_at,
                "receipt": "self",
            })),
        );

        let updated =
            with_audit::<_, Option<InboxDocSummary>, PgInboxError>(&self.pool, event, move |tx| {
                Box::pin(async move {
                    let row = sqlx::query(
                        r#"
                        UPDATE inbox_docs
                        SET confirmed_by = $2, confirmed_at = $3
                        WHERE id = $1
                          AND recipient_user_id = $2
                          AND kind = 'legal_notice'
                          AND confirmed_at IS NULL
                        RETURNING id, recipient_user_id, kind, notice_type, title, legal_basis,
                                  source_kind, source_id, confirmed_by, confirmed_at, created_at
                        "#,
                    )
                    .bind(id_uuid)
                    .bind(recipient_uuid)
                    .bind(occurred_at)
                    .fetch_optional(tx.as_mut())
                    .await?;
                    row.as_ref().map(summary_from_row).transpose()
                })
            })
            .await?;

        match updated {
            Some(summary) => Ok(summary),
            // A concurrent confirm won the race between our read and update.
            // Re-read and return the winner's stamp (still idempotent).
            None => Ok(self
                .get(GetInboxDocQuery {
                    recipient: command.recipient,
                    id: command.doc_id,
                })
                .await?
                .summary),
        }
    }
}

impl InboxDocSink for PgInboxStore {
    fn emit(&self, command: EmitInboxDocCommand) -> EmitInboxDocFuture<'_> {
        Box::pin(async move {
            self.emit_inbox_doc(command)
                .await
                .map_err(KernelError::from)
        })
    }
}

fn summary_from_row(row: &sqlx::postgres::PgRow) -> Result<InboxDocSummary, PgInboxError> {
    let kind = InboxDocKind::parse(row.try_get::<String, _>("kind")?.as_str())?;
    let confirmed_at: Option<time::OffsetDateTime> = row.try_get("confirmed_at")?;
    let confirmed_by: Option<uuid::Uuid> = row.try_get("confirmed_by")?;
    // Lock predicate: a legal notice is locked until its receipt is confirmed.
    let locked = kind.requires_receipt() && confirmed_at.is_none();
    Ok(InboxDocSummary {
        id: InboxDocId::from_uuid(row.try_get("id")?),
        recipient_user_id: UserId::from_uuid(row.try_get("recipient_user_id")?),
        kind,
        notice_type: row.try_get("notice_type")?,
        title: row.try_get("title")?,
        legal_basis: row.try_get("legal_basis")?,
        source_kind: row.try_get("source_kind")?,
        source_id: row.try_get("source_id")?,
        locked,
        confirmed_by: confirmed_by.map(UserId::from_uuid),
        confirmed_at,
        created_at: row.try_get("created_at")?,
    })
}
