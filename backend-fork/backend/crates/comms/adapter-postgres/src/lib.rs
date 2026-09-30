//! Postgres adapter for the webmail store.
//!
//! Every method is org-scoped: reads go through `with_org_conn(current_org()?)`
//! and writes through `with_audit` (which arms `app.current_org` for the audited
//! transaction). A missing request-context org fails closed. The adapter never
//! touches an `email_*` table on the raw pool.
//!
//! # Write-only credentials
//!
//! `upsert_account` persists ONLY the ciphertext the cipher produced
//! (`smtp_password_ct/nonce` + `dek_wrapped/dek_nonce` for the SMTP secret;
//! `imap_password_ct/nonce` + `imap_dek_wrapped/imap_dek_nonce` for the IMAP
//! secret — each secret keeps its own wrapped per-row DEK). `get_account` reads
//! those sealed columns back (the service needs them to decrypt for a send) but
//! the REST layer projects them to a DTO that omits every secret, so the
//! password is never returned to a client.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

use console_comms_application::{
    AccountUpsert, AddressAccount, AddressLookup, AttachmentRef, AttachmentView, DueAccount,
    EmailAccountId, EmailMessageId, FolderCursor, FolderView, ImapFolder, InboundUpsert,
    MailFuture, MailNotifier, MailReadStore, MailServiceError, MailStore, MessageView,
    OutboundPreparation, SealedCredential, StoredAccount, StoredAttachment, ThreadDetail,
    ThreadQuery, ThreadView, address_ambiguous_audit_event, thread_grouping_key,
};
use console_comms_domain::{MailSecurity, MessageAddress};
use console_kernel_core::{
    AuditEvent, ErrorKind, KernelError, OrgId, Timestamp, TraceContext, UserId,
};
use console_platform_db::{DbError, with_audit, with_audits, with_org_conn};
use console_platform_request_context::current_org;
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Postgres, Row, Transaction};

/// Adapter error. Mirrors `PgOrgError`: domain failures carry safe messages;
/// sqlx errors are mapped to a coarse [`ErrorKind`] for the REST surface and
/// never leaked verbatim.
#[derive(Debug, thiserror::Error)]
pub enum PgMailError {
    #[error(transparent)]
    Db(#[from] DbError),
    #[error(transparent)]
    Domain(#[from] KernelError),
    #[error("mail operation conflicts with an earlier request")]
    OperationConflict,
}

impl PgMailError {
    #[must_use]
    pub fn kind(&self) -> ErrorKind {
        match self {
            Self::Domain(error) => error.kind,
            Self::OperationConflict => ErrorKind::Conflict,
            Self::Db(DbError::Sqlx(sqlx::Error::RowNotFound)) => ErrorKind::NotFound,
            Self::Db(DbError::Sqlx(sqlx::Error::Database(error)))
                if error.code().is_some_and(|code| code == "23505") =>
            {
                ErrorKind::Conflict
            }
            Self::Db(DbError::Sqlx(sqlx::Error::Database(error)))
                if error.code().is_some_and(|code| code == "23503") =>
            {
                ErrorKind::Validation
            }
            Self::Db(_) => ErrorKind::Internal,
        }
    }
}

impl From<sqlx::Error> for PgMailError {
    fn from(value: sqlx::Error) -> Self {
        Self::Db(DbError::Sqlx(value))
    }
}

impl From<PgMailError> for MailServiceError {
    fn from(value: PgMailError) -> Self {
        match value {
            PgMailError::Domain(err) => MailServiceError::Domain(err),
            PgMailError::OperationConflict => MailServiceError::OperationConflict,
            // Db errors are server-internal; the application surface collapses
            // them into a coarse, caller-safe variant.
            PgMailError::Db(_) => MailServiceError::Store,
        }
    }
}

#[derive(Debug, Clone)]
pub struct PgMailStore {
    pool: PgPool,
}

impl PgMailStore {
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    #[must_use]
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    async fn get_account_inner(&self) -> Result<Option<StoredAccount>, PgMailError> {
        let org = current_org().map_err(KernelError::from)?;
        let row = with_org_conn::<_, _, PgMailError>(&self.pool, org, move |tx| {
            Box::pin(async move {
                let row = sqlx::query(ACCOUNT_SELECT_LATEST)
                    .fetch_optional(tx.as_mut())
                    .await?;
                Ok(row)
            })
        })
        .await?;
        row.map(account_from_row).transpose()
    }

    async fn upsert_account_inner(
        &self,
        upsert: AccountUpsert,
        audit: AuditEvent,
    ) -> Result<StoredAccount, PgMailError> {
        let org = audit
            .org_id
            .ok_or_else(|| KernelError::internal("account upsert audit is missing the org id"))?;
        let org_uuid = *org.as_uuid();
        let account_uuid = *upsert.id.as_uuid();

        with_audit::<_, StoredAccount, PgMailError>(&self.pool, audit, move |tx| {
            Box::pin(async move {
                let exists = sqlx::query_scalar::<_, i64>(
                    "SELECT COUNT(*) FROM email_accounts WHERE id = $1",
                )
                .bind(account_uuid)
                .fetch_one(tx.as_mut())
                .await?
                    > 0;

                if exists {
                    update_account_tx(tx, &upsert).await?;
                } else {
                    // A brand-new account requires both sealed secrets (the DB
                    // columns are NOT NULL); the service already enforced this.
                    let smtp = upsert.smtp_password.as_ref().ok_or_else(|| {
                        KernelError::validation("new mailbox requires an SMTP password")
                    })?;
                    let imap = upsert.imap_password.as_ref().ok_or_else(|| {
                        KernelError::validation("new mailbox requires an IMAP password")
                    })?;
                    insert_account_tx(tx, org_uuid, &upsert, smtp, imap).await?;
                }
                fetch_account_tx(tx, account_uuid).await
            })
        })
        .await
    }

    /// UPSERT the org-scoped, per-user fixed-window rate counter for one bucket
    /// and return the new attempt count. RLS-armed via `with_org_conn` (the GUC
    /// is bound from the request-context org); the UPSERT runs on `tx.as_mut()`,
    /// never a bare pool, so one org's counter can never touch another's. The
    /// `WITH CHECK` org_isolation policy stamps the row's org_id implicitly — we
    /// bind it explicitly too so the INSERT column is non-null.
    async fn increment_send_rate_inner(
        &self,
        actor: UserId,
        endpoint: &'static str,
        window_start: Timestamp,
    ) -> Result<i64, PgMailError> {
        let org = current_org().map_err(KernelError::from)?;
        let org_uuid = *org.as_uuid();
        let actor_uuid = *actor.as_uuid();
        let attempts = with_org_conn::<_, i32, PgMailError>(&self.pool, org, move |tx| {
            Box::pin(async move {
                let attempts: i32 = sqlx::query_scalar(
                    r#"
                    INSERT INTO comms_send_rate (org_id, actor_user_id, endpoint, window_start, attempts)
                    VALUES ($1, $2, $3, $4, 1)
                    ON CONFLICT (org_id, actor_user_id, endpoint, window_start)
                    DO UPDATE SET attempts = comms_send_rate.attempts + 1, updated_at = now()
                    RETURNING attempts
                    "#,
                )
                .bind(org_uuid)
                .bind(actor_uuid)
                .bind(endpoint)
                .bind(window_start)
                .fetch_one(tx.as_mut())
                .await?;
                Ok(attempts)
            })
        })
        .await?;
        Ok(i64::from(attempts))
    }

    async fn prepare_outbound_inner(
        &self,
        preparation: OutboundPreparation,
        audit: AuditEvent,
    ) -> Result<(), PgMailError> {
        let org = audit
            .org_id
            .ok_or_else(|| KernelError::internal("mail preparation audit is missing Company"))?;
        if current_org().map_err(KernelError::from)? != org {
            return Err(KernelError::forbidden("mail preparation Company mismatch").into());
        }
        if audit.actor != Some(preparation.actor)
            || audit.target_id != preparation.operation_id.to_string()
        {
            return Err(KernelError::forbidden("mail preparation audit mismatch").into());
        }
        let actual_payload_hash: [u8; 32] = Sha256::digest(&preparation.payload.bytes).into();
        if preparation.payload_hash != actual_payload_hash {
            return Err(KernelError::validation("prepared mail digest mismatch").into());
        }
        let rfc_message_id = preparation
            .payload
            .rfc_message_id
            .as_deref()
            .ok_or_else(|| KernelError::validation("prepared SMTP mail needs Message-ID"))?
            .to_owned();
        let org_uuid = *org.as_uuid();
        with_audits::<_, (), PgMailError>(&self.pool, org, move |tx| {
            Box::pin(async move {
                let inserted: Option<uuid::Uuid> = sqlx::query_scalar(
                    r#"
                    INSERT INTO email_outbound_operations (
                        org_id, operation_id, account_id, actor_user_id,
                        request_codec_version, request_hash, payload_hash,
                        payload_format, payload_bytes, envelope_from,
                        envelope_recipients, rfc_message_id
                    ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)
                    ON CONFLICT (org_id, operation_id) DO NOTHING
                    RETURNING operation_id
                    "#,
                )
                .bind(org_uuid)
                .bind(preparation.operation_id)
                .bind(*preparation.account_id.as_uuid())
                .bind(*preparation.actor.as_uuid())
                .bind(preparation.request_codec_version)
                .bind(preparation.request_hash.as_slice())
                .bind(preparation.payload_hash.as_slice())
                .bind(preparation.payload.format.as_db_str())
                .bind(&preparation.payload.bytes)
                .bind(&preparation.from_address)
                .bind(&preparation.envelope_recipients)
                .bind(rfc_message_id)
                .fetch_optional(tx.as_mut())
                .await?;
                if inserted.is_some() {
                    return Ok(((), vec![audit]));
                }

                // The conflicting INSERT waits for an in-flight same-key
                // transaction. Its immutable row is now visible in this
                // READ COMMITTED statement; never overwrite its MIME/Message-ID.
                let existing = sqlx::query(
                    "SELECT request_codec_version, request_hash FROM email_outbound_operations WHERE org_id = $1 AND operation_id = $2",
                )
                .bind(org_uuid)
                .bind(preparation.operation_id)
                .fetch_one(tx.as_mut())
                .await?;
                let codec: i16 = existing.try_get("request_codec_version")?;
                let hash: Vec<u8> = existing.try_get("request_hash")?;
                if codec != preparation.request_codec_version || hash != preparation.request_hash {
                    return Err(PgMailError::OperationConflict);
                }
                Ok(((), vec![]))
            })
        })
        .await
    }
}

impl MailStore for PgMailStore {
    fn get_account(
        &self,
    ) -> console_comms_application::MailFuture<'_, Result<Option<StoredAccount>, MailServiceError>>
    {
        Box::pin(async move { self.get_account_inner().await.map_err(Into::into) })
    }

    fn upsert_account(
        &self,
        upsert: AccountUpsert,
        audit: AuditEvent,
    ) -> console_comms_application::MailFuture<'_, Result<StoredAccount, MailServiceError>> {
        Box::pin(async move {
            self.upsert_account_inner(upsert, audit)
                .await
                .map_err(Into::into)
        })
    }

    fn prepare_outbound(
        &self,
        preparation: OutboundPreparation,
        audit: AuditEvent,
    ) -> console_comms_application::MailFuture<'_, Result<(), MailServiceError>> {
        Box::pin(async move {
            self.prepare_outbound_inner(preparation, audit)
                .await
                .map_err(Into::into)
        })
    }

    fn increment_send_rate(
        &self,
        actor: UserId,
        endpoint: &'static str,
        window_start: Timestamp,
    ) -> console_comms_application::MailFuture<'_, Result<i64, MailServiceError>> {
        Box::pin(async move {
            self.increment_send_rate_inner(actor, endpoint, window_start)
                .await
                .map_err(Into::into)
        })
    }
}

// ===========================================================================
// MailReadStore — inbound sync + read API (B-mail-3).
//
// Every method is org-scoped via `with_org_conn` (reads) or `with_audit` (the
// inbound upsert writes an `email.sync.message` audit row). The ONLY exception is
// `list_due_accounts`, which calls the `comms_due_email_accounts` SECURITY
// DEFINER function on the raw pool — it returns id-only pairs across tenants so
// the scheduler can dispatch per-(org, account) jobs that then arm RLS.
// ===========================================================================

impl PgMailStore {
    async fn upsert_folders_inner(
        &self,
        org: OrgId,
        account: EmailAccountId,
        folders: &[ImapFolder],
    ) -> Result<Vec<FolderCursor>, PgMailError> {
        let account_uuid = *account.as_uuid();
        let owned: Vec<ImapFolder> = folders.to_vec();
        with_org_conn::<_, Vec<FolderCursor>, PgMailError>(&self.pool, org, move |tx| {
            Box::pin(async move {
                let org_uuid = *org.as_uuid();
                let mut cursors = Vec::with_capacity(owned.len());
                for folder in &owned {
                    let row = sqlx::query(
                        r#"
                        INSERT INTO email_folders (org_id, account_id, imap_path, role, name)
                        VALUES ($1, $2, $3, $4, $5)
                        ON CONFLICT (org_id, account_id, imap_path)
                        DO UPDATE SET role = EXCLUDED.role, name = EXCLUDED.name, updated_at = now()
                        RETURNING id, uid_validity, last_seen_uid
                        "#,
                    )
                    .bind(org_uuid)
                    .bind(account_uuid)
                    .bind(&folder.imap_path)
                    .bind(folder.role.as_db_str())
                    .bind(&folder.name)
                    .fetch_one(tx.as_mut())
                    .await?;
                    cursors.push(FolderCursor {
                        folder_id: row.try_get("id")?,
                        imap_path: folder.imap_path.clone(),
                        uid_validity: row.try_get("uid_validity")?,
                        last_seen_uid: row.try_get("last_seen_uid")?,
                    });
                }
                Ok(cursors)
            })
        })
        .await
    }

    async fn reset_folder_cursor_inner(
        &self,
        org: OrgId,
        folder_id: uuid::Uuid,
        uid_validity: i64,
    ) -> Result<(), PgMailError> {
        with_org_conn::<_, (), PgMailError>(&self.pool, org, move |tx| {
            Box::pin(async move {
                sqlx::query(
                    r#"
                    UPDATE email_folders
                    SET uid_validity = $2, last_seen_uid = 0, updated_at = now()
                    WHERE id = $1
                    "#,
                )
                .bind(folder_id)
                .bind(uid_validity)
                .execute(tx.as_mut())
                .await?;
                Ok(())
            })
        })
        .await
    }

    async fn upsert_inbound_inner(
        &self,
        org: OrgId,
        upsert: InboundUpsert,
    ) -> Result<bool, PgMailError> {
        // Build the audit BEFORE the transaction so `with_audit` arms the org.
        let audit = console_comms_application::inbound_sync_audit_event(
            upsert.id,
            console_kernel_core::TraceContext::generate(),
            Timestamp::now_utc(),
        )
        .map_err(PgMailError::Domain)?
        .with_org(org);

        with_audit::<_, bool, PgMailError>(&self.pool, audit, move |tx| {
            Box::pin(async move {
                let org_uuid = *org.as_uuid();
                let account_uuid = *upsert.account_id.as_uuid();

                // 1. Idempotency gate on the IMAP identity: if a row already
                // exists for (account, folder, uid_validity, uid), this is a
                // re-sync — refresh its mutable flags and return `false` (no dup).
                let existing: Option<uuid::Uuid> = sqlx::query_scalar(
                    r#"
                    SELECT id FROM email_messages
                    WHERE account_id = $1 AND folder_id = $2
                      AND imap_uid_validity = $3 AND imap_uid = $4
                    LIMIT 1
                    "#,
                )
                .bind(account_uuid)
                .bind(upsert.folder_id)
                .bind(upsert.uid_validity)
                .bind(i64::from(upsert.message.imap_uid))
                .fetch_optional(tx.as_mut())
                .await?;

                if let Some(existing_id) = existing {
                    sqlx::query(
                        r#"
                        UPDATE email_messages
                        SET seen = $2, flagged = $3, answered = $4, draft = $5, updated_at = now()
                        WHERE id = $1
                        "#,
                    )
                    .bind(existing_id)
                    .bind(upsert.message.seen)
                    .bind(upsert.message.flagged)
                    .bind(upsert.message.answered)
                    .bind(upsert.message.draft)
                    .execute(tx.as_mut())
                    .await?;
                    return Ok(false);
                }

                // 2. Secondary dedupe on Message-ID within the account: a message
                // already mirrored under another folder (e.g. a Sent copy) is not
                // re-inserted; we still return `false`.
                if let Some(ref mid) = upsert.message.message_id
                    && message_id_seen(tx, account_uuid, mid).await?
                {
                    return Ok(false);
                }

                // 3. Resolve the thread by the References/subject grouping key.
                let thread_id =
                    resolve_inbound_thread_tx(tx, org_uuid, account_uuid, &upsert).await?;

                // 4. Insert the IN message. If a concurrent webhook redelivery won
                // the same IMAP-identity race, the insert is an idempotent no-op
                // and the handler reports `ingested=false` instead of surfacing
                // a unique-constraint service error.
                if !insert_inbound_message_tx(tx, org_uuid, account_uuid, thread_id, &upsert)
                    .await?
                {
                    return Ok(false);
                }

                // 5. Insert its attachment rows.
                for att in &upsert.stored_attachments {
                    insert_attachment_tx(tx, org_uuid, *upsert.id.as_uuid(), att).await?;
                }

                // 6. Maintain the thread aggregate + the folder high-water mark.
                update_thread_aggregate_tx(tx, thread_id, &upsert).await?;
                advance_folder_cursor_tx(
                    tx,
                    upsert.folder_id,
                    upsert.uid_validity,
                    upsert.message.imap_uid,
                )
                .await?;

                Ok(true)
            })
        })
        .await
    }

    async fn record_sync_result_inner(
        &self,
        org: OrgId,
        account: EmailAccountId,
        claim_token: uuid::Uuid,
        status: &str,
        error: Option<&str>,
    ) -> Result<(), PgMailError> {
        let account_uuid = *account.as_uuid();
        let status = status.to_owned();
        let error = error.map(ToOwned::to_owned);
        with_org_conn::<_, (), PgMailError>(&self.pool, org, move |tx| {
            Box::pin(async move {
                sqlx::query(
                    r#"
                    UPDATE email_accounts
                    SET last_sync_at = now(),
                        sync_status = $2,
                        last_sync_error = $3,
                        -- Release the scheduler's claim lease so the account is
                        -- eligible again on its next cadence. FENCED: only the
                        -- worker still holding this claim's token clears it, so a
                        -- stale worker whose lease was reclaimed (a newer token)
                        -- matches no row and wipes NOTHING — it cannot stomp the
                        -- reclaimer's fresh lease and cause an overlapping sync.
                        claimed_until = NULL,
                        claim_token = NULL,
                        consecutive_auth_failures = CASE
                            WHEN $2 = 'AUTH_FAILED' THEN consecutive_auth_failures + 1
                            WHEN $2 = 'OK' THEN 0
                            ELSE consecutive_auth_failures
                        END,
                        updated_at = now()
                    WHERE id = $1 AND claim_token = $4
                    "#,
                )
                .bind(account_uuid)
                .bind(&status)
                .bind(error.as_deref())
                .bind(claim_token)
                .execute(tx.as_mut())
                .await?;
                Ok(())
            })
        })
        .await
    }

    /// Release the claim lease with NO lifecycle write — a fenced compare-and-
    /// clear that touches only `claimed_until`/`claim_token`, never
    /// `last_sync_at`/`sync_status`/`last_sync_error`/auth-failure counters.
    async fn release_claim_inner(
        &self,
        org: OrgId,
        account: EmailAccountId,
        claim_token: uuid::Uuid,
    ) -> Result<(), PgMailError> {
        let account_uuid = *account.as_uuid();
        with_org_conn::<_, (), PgMailError>(&self.pool, org, move |tx| {
            Box::pin(async move {
                sqlx::query(
                    r#"
                    UPDATE email_accounts
                    SET claimed_until = NULL, claim_token = NULL
                    WHERE id = $1 AND claim_token = $2
                    "#,
                )
                .bind(account_uuid)
                .bind(claim_token)
                .execute(tx.as_mut())
                .await?;
                Ok(())
            })
        })
        .await
    }

    async fn list_due_accounts_inner(
        &self,
        now: Timestamp,
        limit: i32,
    ) -> Result<Vec<DueAccount>, PgMailError> {
        // Cross-tenant id-only CLAIM via the SECURITY DEFINER function. Runs on the
        // raw pool (NO org armed) — the function REVOKEs PUBLIC and only console_rt may
        // EXECUTE it; it atomically locks the due, unclaimed rows with FOR UPDATE
        // SKIP LOCKED, stamps a `claimed_until` lease + a fresh `claim_token` on
        // each, and returns ONLY the (org_id, account_id, claim_token) triples.
        // Concurrent workers therefore claim DISJOINT batches; a crashed worker's
        // lease is reclaimable once it expires, and the token fences the release so
        // a reclaimed worker cannot clear the reclaimer's fresh lease.
        //
        // `limit` is the caller's available start-immediately capacity (see the
        // trait doc); SYNC_DISPATCH_LIMIT is only a defense-in-depth ceiling in
        // case a caller ever passes something absurd.
        let rows = sqlx::query(
            "SELECT org_id, account_id, claim_token FROM comms_due_email_accounts($1, $2, $3)",
        )
        .bind(now)
        .bind(limit.clamp(0, SYNC_DISPATCH_LIMIT))
        .bind(SYNC_CLAIM_LEASE_SECS)
        // rls-arming: ok comms_due_email_accounts is a SECURITY DEFINER function (REVOKE PUBLIC, GRANT console_rt) that returns id-only (org_id, account_id, claim_token) triples across tenants for the scheduler; it cannot be org-armed (it predates knowing the orgs) and exposes no tenant data — it only stamps its own claim lease + fencing token
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|row| {
                Ok(DueAccount {
                    org_id: OrgId::from_uuid(row.try_get("org_id")?),
                    account_id: EmailAccountId::from_uuid(row.try_get("account_id")?),
                    claim_token: row.try_get("claim_token")?,
                })
            })
            .collect()
    }

    async fn find_account_by_address_inner(
        &self,
        address: &str,
    ) -> Result<AddressLookup, PgMailError> {
        // Cross-tenant id-only lookup via 0122_comms_account_by_address.sql. Runs
        // on the raw pool (NO org armed) — the function REVOKEs PUBLIC and only
        // console_rt may EXECUTE it; it returns ONLY the (org_id, account_id) pairs of
        // every ACTIVE account matching this recipient address. `email_accounts`
        // is unique only per (org_id, email_address), so >1 row means the SAME
        // address exists under DIFFERENT orgs — a tenant-boundary anomaly, not a
        // pick-one situation. The webhook arms the single resolved org for the
        // audited inbound upsert; an ambiguous address is refused for ALL of them.
        let rows: Vec<sqlx::postgres::PgRow> =
            sqlx::query("SELECT org_id, account_id FROM comms_account_by_address($1)")
                .bind(address)
                // rls-arming: ok comms_account_by_address is a SECURITY DEFINER function (REVOKE PUBLIC, GRANT console_rt) returning id-only (org_id, account_id) for the mox delivery webhook, which has no principal to arm an org with; it exposes no tenant data
                .fetch_all(&self.pool)
                .await?;
        match rows.len() {
            0 => Ok(AddressLookup::NotFound),
            1 => {
                let row = &rows[0];
                Ok(AddressLookup::Found(AddressAccount {
                    org_id: OrgId::from_uuid(row.try_get("org_id")?),
                    account_id: EmailAccountId::from_uuid(row.try_get("account_id")?),
                }))
            }
            _ => {
                tracing::error!(
                    match_count = rows.len(),
                    "mox webhook: recipient address matched ACTIVE accounts in more than one org; quarantining delivery (no message content logged)"
                );
                let audit = address_ambiguous_audit_event(
                    address,
                    TraceContext::generate(),
                    Timestamp::now_utc(),
                )
                .map_err(PgMailError::Domain)?;
                // No `org` to arm (that is exactly the anomaly) — insert the
                // platform-tier audit row on the raw pool. `with_audit` leaves
                // `app.current_org` unset when `event.org_id` is `None`; the
                // `audit_events` RLS `WITH CHECK (org_id IS NULL)` clause admits it.
                with_audit::<_, (), PgMailError>(&self.pool, audit, |_tx| {
                    Box::pin(async move { Ok(()) })
                })
                .await?;
                Ok(AddressLookup::Ambiguous)
            }
        }
    }

    async fn list_folders_inner(
        &self,
        org: OrgId,
        account: EmailAccountId,
    ) -> Result<Vec<FolderView>, PgMailError> {
        let account_uuid = *account.as_uuid();
        with_org_conn::<_, Vec<FolderView>, PgMailError>(&self.pool, org, move |tx| {
            Box::pin(async move {
                let rows = sqlx::query(
                    r#"
                    SELECT id, role, name, unread_count, total_count
                    FROM email_folders
                    WHERE account_id = $1
                    ORDER BY
                        CASE role
                            WHEN 'INBOX' THEN 0 WHEN 'SENT' THEN 1 WHEN 'DRAFTS' THEN 2
                            WHEN 'ARCHIVE' THEN 3 WHEN 'JUNK' THEN 4 WHEN 'TRASH' THEN 5
                            ELSE 6
                        END,
                        name ASC
                    "#,
                )
                .bind(account_uuid)
                .fetch_all(tx.as_mut())
                .await?;
                rows.into_iter()
                    .map(|row| {
                        Ok(FolderView {
                            id: row.try_get("id")?,
                            role: row.try_get("role")?,
                            name: row.try_get("name")?,
                            unread_count: i64::from(row.try_get::<i32, _>("unread_count")?),
                            total_count: i64::from(row.try_get::<i32, _>("total_count")?),
                        })
                    })
                    .collect()
            })
        })
        .await
    }

    async fn list_threads_inner(
        &self,
        org: OrgId,
        account: EmailAccountId,
        query: &ThreadQuery,
    ) -> Result<Vec<ThreadView>, PgMailError> {
        let account_uuid = *account.as_uuid();
        let query = query.clone();
        with_org_conn::<_, Vec<ThreadView>, PgMailError>(&self.pool, org, move |tx| {
            Box::pin(async move {
                let limit = query.limit.clamp(1, MAX_THREAD_PAGE);
                let mut builder = sqlx::QueryBuilder::<Postgres>::new(
                    r#"
                    SELECT t.id, t.subject, t.last_message_at, t.message_count,
                           t.unread_count, t.has_attachments, t.is_flagged
                    FROM email_threads t
                    WHERE t.account_id =
                    "#,
                );
                builder.push_bind(account_uuid);
                if query.unread_only {
                    builder.push(" AND t.unread_count > 0");
                }
                if let Some(before) = query.before {
                    builder.push(" AND t.last_message_at < ");
                    builder.push_bind(before);
                }
                if let Some(search) = query.search.as_ref().filter(|s| !s.trim().is_empty()) {
                    // FTS over the thread's messages' search_vector, with an ILIKE
                    // fallback so a partial/Korean term still matches the subject.
                    let term = search.trim().to_owned();
                    builder.push(
                        r#" AND (
                            t.subject ILIKE '%' || "#,
                    );
                    builder.push_bind(term.clone());
                    builder.push(
                        r#" || '%'
                            OR EXISTS (
                                SELECT 1 FROM email_messages m
                                WHERE m.thread_id = t.id
                                  AND m.search_vector @@ plainto_tsquery('simple', "#,
                    );
                    builder.push_bind(term);
                    builder.push("))) ");
                }
                builder.push(" ORDER BY t.last_message_at DESC, t.id DESC LIMIT ");
                builder.push_bind(limit);

                let rows = builder.build().fetch_all(tx.as_mut()).await?;
                rows.into_iter()
                    .map(|row| {
                        Ok(ThreadView {
                            id: row.try_get("id")?,
                            subject: row.try_get("subject")?,
                            last_message_at: row.try_get("last_message_at")?,
                            message_count: i64::from(row.try_get::<i32, _>("message_count")?),
                            unread_count: i64::from(row.try_get::<i32, _>("unread_count")?),
                            has_attachments: row.try_get("has_attachments")?,
                            is_flagged: row.try_get("is_flagged")?,
                        })
                    })
                    .collect()
            })
        })
        .await
    }

    async fn get_thread_inner(
        &self,
        org: OrgId,
        thread_id: uuid::Uuid,
    ) -> Result<Option<ThreadDetail>, PgMailError> {
        with_org_conn::<_, Option<ThreadDetail>, PgMailError>(&self.pool, org, move |tx| {
            Box::pin(async move {
                let Some(thread) =
                    sqlx::query("SELECT id, subject FROM email_threads WHERE id = $1")
                        .bind(thread_id)
                        .fetch_optional(tx.as_mut())
                        .await?
                else {
                    return Ok(None);
                };
                let message_rows = sqlx::query(MESSAGE_SELECT_BY_THREAD)
                    .bind(thread_id)
                    .fetch_all(tx.as_mut())
                    .await?;
                let mut messages = Vec::with_capacity(message_rows.len());
                for row in message_rows {
                    let id: uuid::Uuid = row.try_get("id")?;
                    let attachments = load_attachments_tx(tx, id).await?;
                    messages.push(message_view_from_row(&row, attachments)?);
                }
                Ok(Some(ThreadDetail {
                    id: thread.try_get("id")?,
                    subject: thread.try_get("subject")?,
                    messages,
                }))
            })
        })
        .await
    }

    async fn get_message_inner(
        &self,
        org: OrgId,
        message_id: EmailMessageId,
    ) -> Result<Option<MessageView>, PgMailError> {
        let message_uuid = *message_id.as_uuid();
        with_org_conn::<_, Option<MessageView>, PgMailError>(&self.pool, org, move |tx| {
            Box::pin(async move {
                let Some(row) = sqlx::query(MESSAGE_SELECT_BY_ID)
                    .bind(message_uuid)
                    .fetch_optional(tx.as_mut())
                    .await?
                else {
                    return Ok(None);
                };
                let attachments = load_attachments_tx(tx, message_uuid).await?;
                Ok(Some(message_view_from_row(&row, attachments)?))
            })
        })
        .await
    }

    async fn get_attachment_key_inner(
        &self,
        org: OrgId,
        attachment_id: uuid::Uuid,
    ) -> Result<Option<AttachmentRef>, PgMailError> {
        with_org_conn::<_, Option<AttachmentRef>, PgMailError>(&self.pool, org, move |tx| {
            Box::pin(async move {
                let row = sqlx::query(
                    "SELECT s3_key, filename, content_type FROM email_attachments WHERE id = $1",
                )
                .bind(attachment_id)
                .fetch_optional(tx.as_mut())
                .await?;
                row.map(|row| {
                    Ok(AttachmentRef {
                        s3_key: row.try_get("s3_key")?,
                        filename: row.try_get("filename")?,
                        content_type: row.try_get("content_type")?,
                    })
                })
                .transpose()
            })
        })
        .await
    }

    async fn set_thread_seen_inner(
        &self,
        org: OrgId,
        thread_id: uuid::Uuid,
        seen: bool,
        audit: AuditEvent,
    ) -> Result<bool, PgMailError> {
        let audit_org = audit
            .org_id
            .ok_or_else(|| KernelError::internal("mail thread read-state audit is missing org"))?;
        if audit_org != org {
            return Err(KernelError::internal("mail thread read-state audit org mismatch").into());
        }

        with_audit::<_, bool, PgMailError>(&self.pool, audit, move |tx| {
            Box::pin(async move {
                let folder_ids: Vec<uuid::Uuid> = sqlx::query_scalar(
                    "SELECT DISTINCT folder_id FROM email_messages WHERE thread_id = $1",
                )
                .bind(thread_id)
                .fetch_all(tx.as_mut())
                .await?;
                let thread_exists: bool =
                    sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM email_threads WHERE id = $1)")
                        .bind(thread_id)
                        .fetch_one(tx.as_mut())
                        .await?;
                if !thread_exists {
                    return Ok(false);
                }

                sqlx::query(
                    r#"
                    UPDATE email_messages
                    SET seen = $2, updated_at = now()
                    WHERE thread_id = $1
                      AND direction = 'IN'
                      AND seen <> $2
                    "#,
                )
                .bind(thread_id)
                .bind(seen)
                .execute(tx.as_mut())
                .await?;

                recompute_thread_read_state_tx(tx, thread_id).await?;
                if !folder_ids.is_empty() {
                    recompute_folder_counts_tx(tx, &folder_ids).await?;
                }
                Ok(true)
            })
        })
        .await
    }
}

impl MailReadStore for PgMailStore {
    fn upsert_folders<'a>(
        &'a self,
        org: OrgId,
        account: EmailAccountId,
        folders: &'a [ImapFolder],
    ) -> console_comms_application::MailFuture<'a, Result<Vec<FolderCursor>, MailServiceError>>
    {
        Box::pin(async move {
            self.upsert_folders_inner(org, account, folders)
                .await
                .map_err(Into::into)
        })
    }

    fn reset_folder_cursor<'a>(
        &'a self,
        org: OrgId,
        folder_id: uuid::Uuid,
        uid_validity: i64,
    ) -> console_comms_application::MailFuture<'a, Result<(), MailServiceError>> {
        Box::pin(async move {
            self.reset_folder_cursor_inner(org, folder_id, uid_validity)
                .await
                .map_err(Into::into)
        })
    }

    fn upsert_inbound<'a>(
        &'a self,
        org: OrgId,
        upsert: InboundUpsert,
    ) -> console_comms_application::MailFuture<'a, Result<bool, MailServiceError>> {
        Box::pin(async move {
            self.upsert_inbound_inner(org, upsert)
                .await
                .map_err(Into::into)
        })
    }

    fn record_sync_result<'a>(
        &'a self,
        org: OrgId,
        account: EmailAccountId,
        claim_token: uuid::Uuid,
        status: &'a str,
        error: Option<&'a str>,
    ) -> console_comms_application::MailFuture<'a, Result<(), MailServiceError>> {
        Box::pin(async move {
            self.record_sync_result_inner(org, account, claim_token, status, error)
                .await
                .map_err(Into::into)
        })
    }

    fn release_claim<'a>(
        &'a self,
        org: OrgId,
        account: EmailAccountId,
        claim_token: uuid::Uuid,
    ) -> console_comms_application::MailFuture<'a, Result<(), MailServiceError>> {
        Box::pin(async move {
            self.release_claim_inner(org, account, claim_token)
                .await
                .map_err(Into::into)
        })
    }

    fn list_due_accounts(
        &self,
        now: Timestamp,
        limit: i32,
    ) -> console_comms_application::MailFuture<'_, Result<Vec<DueAccount>, MailServiceError>> {
        Box::pin(async move {
            self.list_due_accounts_inner(now, limit)
                .await
                .map_err(Into::into)
        })
    }

    fn find_account_by_address<'a>(
        &'a self,
        address: &'a str,
    ) -> console_comms_application::MailFuture<'a, Result<AddressLookup, MailServiceError>> {
        Box::pin(async move {
            self.find_account_by_address_inner(address)
                .await
                .map_err(Into::into)
        })
    }

    fn list_folders<'a>(
        &'a self,
        org: OrgId,
        account: EmailAccountId,
    ) -> console_comms_application::MailFuture<'a, Result<Vec<FolderView>, MailServiceError>> {
        Box::pin(async move {
            self.list_folders_inner(org, account)
                .await
                .map_err(Into::into)
        })
    }

    fn list_threads<'a>(
        &'a self,
        org: OrgId,
        account: EmailAccountId,
        query: &'a ThreadQuery,
    ) -> console_comms_application::MailFuture<'a, Result<Vec<ThreadView>, MailServiceError>> {
        Box::pin(async move {
            self.list_threads_inner(org, account, query)
                .await
                .map_err(Into::into)
        })
    }

    fn get_thread<'a>(
        &'a self,
        org: OrgId,
        thread_id: uuid::Uuid,
    ) -> console_comms_application::MailFuture<'a, Result<Option<ThreadDetail>, MailServiceError>>
    {
        Box::pin(async move {
            self.get_thread_inner(org, thread_id)
                .await
                .map_err(Into::into)
        })
    }

    fn get_message<'a>(
        &'a self,
        org: OrgId,
        message_id: EmailMessageId,
    ) -> console_comms_application::MailFuture<'a, Result<Option<MessageView>, MailServiceError>>
    {
        Box::pin(async move {
            self.get_message_inner(org, message_id)
                .await
                .map_err(Into::into)
        })
    }

    fn get_attachment_key<'a>(
        &'a self,
        org: OrgId,
        attachment_id: uuid::Uuid,
    ) -> console_comms_application::MailFuture<'a, Result<Option<AttachmentRef>, MailServiceError>>
    {
        Box::pin(async move {
            self.get_attachment_key_inner(org, attachment_id)
                .await
                .map_err(Into::into)
        })
    }

    fn set_thread_seen<'a>(
        &'a self,
        org: OrgId,
        thread_id: uuid::Uuid,
        seen: bool,
        audit: AuditEvent,
    ) -> console_comms_application::MailFuture<'a, Result<bool, MailServiceError>> {
        Box::pin(async move {
            self.set_thread_seen_inner(org, thread_id, seen, audit)
                .await
                .map_err(Into::into)
        })
    }
}

/// Absolute defense-in-depth ceiling on one claim call, regardless of the
/// caller-supplied `limit`. The caller (the worker's tick) is expected to pass
/// its actual start-immediately capacity (bounded by `MAX_CONCURRENT_SYNCS`,
/// currently 4) — this constant only guards against a caller bug passing an
/// unbounded value.
const SYNC_DISPATCH_LIMIT: i32 = 100;
/// Lease (seconds) stamped on a claimed account. The worker bounds each pass
/// (`mail_sync::SYNC_ACCOUNT_TIMEOUT`, 480s) SAFELY below this so a slow pass is
/// aborted before the lease can be reclaimed — no overlapping syncs. Short enough
/// that a crashed worker's claim is reclaimable promptly. `record_sync_result`
/// fenced-clears the lease on completion; this is the crash-recovery ceiling, not
/// the steady-state hold. Keep in sync with `mail_sync::SYNC_CLAIM_LEASE_SECS`.
const SYNC_CLAIM_LEASE_SECS: i32 = 600;
/// Max threads returned in one thread-list page.
const MAX_THREAD_PAGE: i64 = 100;

/// The read-API message column projection shared by the thread + single-message
/// reads. Spelled out twice as full `&'static str` queries because sqlx 0.9
/// requires a statically-known SQL string (no runtime `format!`).
const MESSAGE_SELECT_BY_THREAD: &str = r#"
    SELECT id, thread_id, direction, message_id, in_reply_to,
           from_address, from_name, to_addresses, cc_addresses,
           subject, snippet, body_text, body_html,
           seen, flagged, answered, has_attachments, received_at
    FROM email_messages
    WHERE thread_id = $1
    ORDER BY received_at ASC, id ASC
"#;

const MESSAGE_SELECT_BY_ID: &str = r#"
    SELECT id, thread_id, direction, message_id, in_reply_to,
           from_address, from_name, to_addresses, cc_addresses,
           subject, snippet, body_text, body_html,
           seen, flagged, answered, has_attachments, received_at
    FROM email_messages
    WHERE id = $1
"#;

/// True when a message with `message_id` already exists for this account (the
/// secondary dedupe — a Sent copy mirrored under another folder, etc.).
async fn message_id_seen(
    tx: &mut Transaction<'_, Postgres>,
    account_uuid: uuid::Uuid,
    message_id: &str,
) -> Result<bool, PgMailError> {
    let exists: Option<uuid::Uuid> = sqlx::query_scalar(
        "SELECT id FROM email_messages WHERE account_id = $1 AND message_id = $2 LIMIT 1",
    )
    .bind(account_uuid)
    .bind(message_id)
    .fetch_optional(tx.as_mut())
    .await?;
    Ok(exists.is_some())
}

/// Resolve (or create) the thread an inbound message belongs to. Threads are
/// matched by the References/subject grouping key: we first try to attach to a
/// thread whose newest message shares the same grouping key (via the parent
/// message-id chain or the normalized subject), else open a new thread.
async fn resolve_inbound_thread_tx(
    tx: &mut Transaction<'_, Postgres>,
    org_uuid: uuid::Uuid,
    account_uuid: uuid::Uuid,
    upsert: &InboundUpsert,
) -> Result<uuid::Uuid, PgMailError> {
    let key = thread_grouping_key(&upsert.message, &upsert.normalized_subject);

    // If the message references a parent we already stored, join that parent's
    // thread directly (the strongest signal).
    for parent in upsert
        .message
        .references
        .iter()
        .rev()
        .chain(upsert.message.in_reply_to.iter())
    {
        if let Some(thread_id) = sqlx::query_scalar::<_, uuid::Uuid>(
            r#"
            SELECT thread_id FROM email_messages
            WHERE account_id = $1 AND message_id = $2
            ORDER BY received_at DESC LIMIT 1
            "#,
        )
        .bind(account_uuid)
        .bind(parent)
        .fetch_optional(tx.as_mut())
        .await?
        {
            let _ = key; // grouping key is the fallback; the parent link wins
            return Ok(thread_id);
        }
    }

    // Subject fallback: attach to the most recent same-normalized-subject thread.
    if !upsert.normalized_subject.is_empty()
        && let Some(thread_id) = sqlx::query_scalar::<_, uuid::Uuid>(
            r#"
            SELECT id FROM email_threads
            WHERE account_id = $1 AND normalized_subject = $2
            ORDER BY last_message_at DESC LIMIT 1
            "#,
        )
        .bind(account_uuid)
        .bind(&upsert.normalized_subject)
        .fetch_optional(tx.as_mut())
        .await?
    {
        return Ok(thread_id);
    }

    // Otherwise open a fresh thread.
    let subject = if upsert.message.subject.trim().is_empty() {
        "(no subject)".to_owned()
    } else {
        upsert.message.subject.clone()
    };
    let id = sqlx::query_scalar::<_, uuid::Uuid>(
        r#"
        INSERT INTO email_threads (
            org_id, account_id, normalized_subject, subject, last_message_at, message_count
        ) VALUES ($1, $2, $3, $4, $5, 0)
        RETURNING id
        "#,
    )
    .bind(org_uuid)
    .bind(account_uuid)
    .bind(&upsert.normalized_subject)
    .bind(&subject)
    .bind(upsert.message.received_at)
    .fetch_one(tx.as_mut())
    .await?;
    Ok(id)
}

async fn insert_inbound_message_tx(
    tx: &mut Transaction<'_, Postgres>,
    org_uuid: uuid::Uuid,
    account_uuid: uuid::Uuid,
    thread_id: uuid::Uuid,
    upsert: &InboundUpsert,
) -> Result<bool, PgMailError> {
    let m = &upsert.message;
    let snippet: String = m
        .body_text
        .as_deref()
        .unwrap_or("")
        .chars()
        .take(200)
        .collect();
    let from_address = m.from.as_ref().map_or("", |a| a.address.as_str());
    let from_name = m.from.as_ref().and_then(|a| a.name.as_deref());
    let to_json = addresses_json(&m.to)?;
    let cc_json = addresses_json(&m.cc)?;
    let references = m.references.clone();
    let has_attachments = !upsert.stored_attachments.is_empty();

    let inserted: Option<uuid::Uuid> = sqlx::query_scalar(
        r#"
        INSERT INTO email_messages (
            id, org_id, account_id, folder_id, thread_id,
            imap_uid, imap_uid_validity, message_id, in_reply_to, references_ids,
            direction, from_address, from_name, to_addresses, cc_addresses,
            subject, snippet, body_text, body_html,
            seen, flagged, answered, draft, has_attachments, received_at
        ) VALUES (
            $1, $2, $3, $4, $5,
            $6, $7, $8, $9, $10,
            'IN', $11, $12, $13, $14,
            $15, $16, $17, $18,
            $19, $20, $21, $22, $23, $24
        )
        ON CONFLICT (org_id, account_id, folder_id, imap_uid_validity, imap_uid)
            WHERE imap_uid IS NOT NULL
            DO NOTHING
        RETURNING id
        "#,
    )
    .bind(*upsert.id.as_uuid())
    .bind(org_uuid)
    .bind(account_uuid)
    .bind(upsert.folder_id)
    .bind(thread_id)
    .bind(i64::from(m.imap_uid))
    .bind(upsert.uid_validity)
    .bind(m.message_id.as_deref())
    .bind(m.in_reply_to.as_deref())
    .bind(&references)
    .bind(from_address)
    .bind(from_name)
    .bind(&to_json)
    .bind(&cc_json)
    .bind(&m.subject)
    .bind(&snippet)
    .bind(m.body_text.as_deref())
    .bind(m.body_html.as_deref())
    .bind(m.seen)
    .bind(m.flagged)
    .bind(m.answered)
    .bind(m.draft)
    .bind(has_attachments)
    .bind(m.received_at)
    .fetch_optional(tx.as_mut())
    .await?;
    Ok(inserted.is_some())
}

async fn insert_attachment_tx(
    tx: &mut Transaction<'_, Postgres>,
    org_uuid: uuid::Uuid,
    message_uuid: uuid::Uuid,
    att: &StoredAttachment,
) -> Result<(), PgMailError> {
    sqlx::query(
        r#"
        INSERT INTO email_attachments (
            org_id, message_id, s3_key, filename, content_type,
            size_bytes, content_id, is_inline, upload_state, sort_order
        ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, 'CONFIRMED', $9)
        ON CONFLICT (org_id, message_id, sort_order) DO NOTHING
        "#,
    )
    .bind(org_uuid)
    .bind(message_uuid)
    .bind(&att.s3_key)
    .bind(&att.filename)
    .bind(&att.content_type)
    .bind(att.size_bytes)
    .bind(att.content_id.as_deref())
    .bind(att.is_inline)
    .bind(att.sort_order)
    .execute(tx.as_mut())
    .await?;
    Ok(())
}

async fn update_thread_aggregate_tx(
    tx: &mut Transaction<'_, Postgres>,
    thread_id: uuid::Uuid,
    upsert: &InboundUpsert,
) -> Result<(), PgMailError> {
    let m = &upsert.message;
    let unread_delta = i32::from(!m.seen);
    let has_attachments = !upsert.stored_attachments.is_empty();
    sqlx::query(
        r#"
        UPDATE email_threads
        SET message_count = message_count + 1,
            unread_count = unread_count + $2,
            last_message_at = GREATEST(last_message_at, $3),
            has_attachments = has_attachments OR $4,
            is_flagged = is_flagged OR $5,
            updated_at = now()
        WHERE id = $1
        "#,
    )
    .bind(thread_id)
    .bind(unread_delta)
    .bind(m.received_at)
    .bind(has_attachments)
    .bind(m.flagged)
    .execute(tx.as_mut())
    .await?;
    Ok(())
}

async fn advance_folder_cursor_tx(
    tx: &mut Transaction<'_, Postgres>,
    folder_id: uuid::Uuid,
    uid_validity: i64,
    imap_uid: u32,
) -> Result<(), PgMailError> {
    sqlx::query(
        r#"
        UPDATE email_folders
        SET uid_validity = $2,
            last_seen_uid = GREATEST(last_seen_uid, $3),
            unread_count = (
                SELECT COUNT(*) FROM email_messages
                WHERE folder_id = $1 AND seen = FALSE
            ),
            total_count = (
                SELECT COUNT(*) FROM email_messages WHERE folder_id = $1
            ),
            last_synced_at = now(),
            updated_at = now()
        WHERE id = $1
        "#,
    )
    .bind(folder_id)
    .bind(uid_validity)
    .bind(i64::from(imap_uid))
    .execute(tx.as_mut())
    .await?;
    Ok(())
}

async fn recompute_thread_read_state_tx(
    tx: &mut Transaction<'_, Postgres>,
    thread_id: uuid::Uuid,
) -> Result<(), PgMailError> {
    sqlx::query(
        r#"
        UPDATE email_threads
        SET unread_count = (
                SELECT COUNT(*)::INTEGER FROM email_messages
                WHERE thread_id = $1 AND direction = 'IN' AND seen = FALSE
            ),
            is_flagged = COALESCE((
                SELECT BOOL_OR(flagged) FROM email_messages WHERE thread_id = $1
            ), FALSE),
            updated_at = now()
        WHERE id = $1
        "#,
    )
    .bind(thread_id)
    .execute(tx.as_mut())
    .await?;
    Ok(())
}

async fn recompute_folder_counts_tx(
    tx: &mut Transaction<'_, Postgres>,
    folder_ids: &[uuid::Uuid],
) -> Result<(), PgMailError> {
    sqlx::query(
        r#"
        UPDATE email_folders f
        SET unread_count = (
                SELECT COUNT(*)::INTEGER FROM email_messages
                WHERE folder_id = f.id AND seen = FALSE
            ),
            total_count = (
                SELECT COUNT(*)::INTEGER FROM email_messages WHERE folder_id = f.id
            ),
            updated_at = now()
        WHERE f.id = ANY($1)
        "#,
    )
    .bind(folder_ids)
    .execute(tx.as_mut())
    .await?;
    Ok(())
}

async fn load_attachments_tx(
    tx: &mut Transaction<'_, Postgres>,
    message_uuid: uuid::Uuid,
) -> Result<Vec<AttachmentView>, PgMailError> {
    let rows = sqlx::query(
        r#"
        SELECT id, filename, content_type, size_bytes, is_inline
        FROM email_attachments
        WHERE message_id = $1
        ORDER BY sort_order ASC
        "#,
    )
    .bind(message_uuid)
    .fetch_all(tx.as_mut())
    .await?;
    rows.into_iter()
        .map(|row| {
            Ok(AttachmentView {
                id: row.try_get("id")?,
                filename: row.try_get("filename")?,
                content_type: row.try_get("content_type")?,
                size_bytes: row.try_get("size_bytes")?,
                is_inline: row.try_get("is_inline")?,
            })
        })
        .collect()
}

/// Build a [`MessageView`] from a message row + its attachments.
fn message_view_from_row(
    row: &sqlx::postgres::PgRow,
    attachments: Vec<AttachmentView>,
) -> Result<MessageView, PgMailError> {
    let to: Vec<MessageAddress> = serde_json::from_value(row.try_get("to_addresses")?)
        .map_err(|_| PgMailError::Domain(KernelError::internal("could not decode recipients")))?;
    let cc: Vec<MessageAddress> = serde_json::from_value(row.try_get("cc_addresses")?)
        .map_err(|_| PgMailError::Domain(KernelError::internal("could not decode cc")))?;
    Ok(MessageView {
        id: EmailMessageId::from_uuid(row.try_get("id")?),
        thread_id: row.try_get("thread_id")?,
        direction: row.try_get("direction")?,
        message_id: row.try_get("message_id")?,
        in_reply_to: row.try_get("in_reply_to")?,
        from_address: row.try_get("from_address")?,
        from_name: row.try_get("from_name")?,
        to,
        cc,
        subject: row.try_get("subject")?,
        snippet: row.try_get("snippet")?,
        body_text: row.try_get("body_text")?,
        body_html: row.try_get("body_html")?,
        seen: row.try_get("seen")?,
        flagged: row.try_get("flagged")?,
        answered: row.try_get("answered")?,
        has_attachments: row.try_get("has_attachments")?,
        received_at: row.try_get("received_at")?,
        attachments,
    })
}

fn addresses_json(addresses: &[MessageAddress]) -> Result<serde_json::Value, PgMailError> {
    serde_json::to_value(addresses)
        .map_err(|_| PgMailError::Domain(KernelError::internal("could not encode addresses")))
}

// ---------------------------------------------------------------------------
// SQL
// ---------------------------------------------------------------------------

/// The full sealed-account column list. Both selects below are spelled out as
/// `&'static str` (sqlx 0.9 requires a statically-known SQL string).
const ACCOUNT_SELECT_LATEST: &str = r#"
    SELECT id, org_id, display_name, email_address, from_name,
           imap_host, imap_port, imap_security, imap_username,
           smtp_host, smtp_port, smtp_security, smtp_username,
           smtp_password_ct, smtp_password_nonce, dek_wrapped, dek_nonce,
           imap_password_ct, imap_password_nonce, imap_dek_wrapped, imap_dek_nonce,
           key_version, status
    FROM email_accounts
    ORDER BY created_at ASC
    LIMIT 1
"#;

const ACCOUNT_SELECT_BY_ID: &str = r#"
    SELECT id, org_id, display_name, email_address, from_name,
           imap_host, imap_port, imap_security, imap_username,
           smtp_host, smtp_port, smtp_security, smtp_username,
           smtp_password_ct, smtp_password_nonce, dek_wrapped, dek_nonce,
           imap_password_ct, imap_password_nonce, imap_dek_wrapped, imap_dek_nonce,
           key_version, status
    FROM email_accounts
    WHERE id = $1
"#;

async fn insert_account_tx(
    tx: &mut Transaction<'_, Postgres>,
    org_uuid: uuid::Uuid,
    upsert: &AccountUpsert,
    smtp: &SealedCredential,
    imap: &SealedCredential,
) -> Result<(), PgMailError> {
    // Both secrets share one `key_version` (the cipher stamps the same current
    // version on every fresh seal in a single configure call).
    sqlx::query(
        r#"
        INSERT INTO email_accounts (
            id, org_id, display_name, email_address, from_name,
            imap_host, imap_port, imap_security, imap_username,
            smtp_host, smtp_port, smtp_security, smtp_username,
            smtp_password_ct, smtp_password_nonce, dek_wrapped, dek_nonce,
            imap_password_ct, imap_password_nonce, imap_dek_wrapped, imap_dek_nonce,
            key_version, created_by
        ) VALUES (
            $1, $2, $3, $4, $5,
            $6, $7, $8, $9,
            $10, $11, $12, $13,
            $14, $15, $16, $17,
            $18, $19, $20, $21,
            $22, $23
        )
        "#,
    )
    .bind(*upsert.id.as_uuid())
    .bind(org_uuid)
    .bind(&upsert.display_name)
    .bind(&upsert.email_address)
    .bind(upsert.from_name.as_deref())
    .bind(&upsert.imap_host)
    .bind(i32::from(upsert.imap_port))
    .bind(upsert.imap_security.as_db_str())
    .bind(&upsert.imap_username)
    .bind(&upsert.smtp_host)
    .bind(i32::from(upsert.smtp_port))
    .bind(upsert.smtp_security.as_db_str())
    .bind(&upsert.smtp_username)
    .bind(&smtp.ciphertext)
    .bind(&smtp.nonce)
    .bind(&smtp.dek_wrapped)
    .bind(&smtp.dek_nonce)
    .bind(&imap.ciphertext)
    .bind(&imap.nonce)
    .bind(&imap.dek_wrapped)
    .bind(&imap.dek_nonce)
    .bind(smtp.key_version)
    .bind(*upsert.actor.as_uuid())
    .execute(tx.as_mut())
    .await?;
    Ok(())
}

async fn update_account_tx(
    tx: &mut Transaction<'_, Postgres>,
    upsert: &AccountUpsert,
) -> Result<(), PgMailError> {
    sqlx::query(
        r#"
        UPDATE email_accounts
        SET display_name = $2, email_address = $3, from_name = $4,
            imap_host = $5, imap_port = $6, imap_security = $7, imap_username = $8,
            smtp_host = $9, smtp_port = $10, smtp_security = $11, smtp_username = $12,
            updated_at = now()
        WHERE id = $1
        "#,
    )
    .bind(*upsert.id.as_uuid())
    .bind(&upsert.display_name)
    .bind(&upsert.email_address)
    .bind(upsert.from_name.as_deref())
    .bind(&upsert.imap_host)
    .bind(i32::from(upsert.imap_port))
    .bind(upsert.imap_security.as_db_str())
    .bind(&upsert.imap_username)
    .bind(&upsert.smtp_host)
    .bind(i32::from(upsert.smtp_port))
    .bind(upsert.smtp_security.as_db_str())
    .bind(&upsert.smtp_username)
    .execute(tx.as_mut())
    .await?;

    // Secrets update ONLY when a fresh sealed credential was supplied; a `None`
    // leaves the stored ciphertext untouched (write-only contract).
    if let Some(smtp) = &upsert.smtp_password {
        sqlx::query(
            r#"
            UPDATE email_accounts
            SET smtp_password_ct = $2, smtp_password_nonce = $3,
                dek_wrapped = $4, dek_nonce = $5, key_version = $6, updated_at = now()
            WHERE id = $1
            "#,
        )
        .bind(*upsert.id.as_uuid())
        .bind(&smtp.ciphertext)
        .bind(&smtp.nonce)
        .bind(&smtp.dek_wrapped)
        .bind(&smtp.dek_nonce)
        .bind(smtp.key_version)
        .execute(tx.as_mut())
        .await?;
    }
    if let Some(imap) = &upsert.imap_password {
        sqlx::query(
            r#"
            UPDATE email_accounts
            SET imap_password_ct = $2, imap_password_nonce = $3,
                imap_dek_wrapped = $4, imap_dek_nonce = $5, updated_at = now()
            WHERE id = $1
            "#,
        )
        .bind(*upsert.id.as_uuid())
        .bind(&imap.ciphertext)
        .bind(&imap.nonce)
        .bind(&imap.dek_wrapped)
        .bind(&imap.dek_nonce)
        .execute(tx.as_mut())
        .await?;
    }
    Ok(())
}

async fn fetch_account_tx(
    tx: &mut Transaction<'_, Postgres>,
    account_uuid: uuid::Uuid,
) -> Result<StoredAccount, PgMailError> {
    let row = sqlx::query(ACCOUNT_SELECT_BY_ID)
        .bind(account_uuid)
        .fetch_one(tx.as_mut())
        .await?;
    account_from_row(row)
}

fn account_from_row(row: sqlx::postgres::PgRow) -> Result<StoredAccount, PgMailError> {
    let imap_security = MailSecurity::parse(row.try_get::<String, _>("imap_security")?.as_str())?;
    let smtp_security = MailSecurity::parse(row.try_get::<String, _>("smtp_security")?.as_str())?;
    let key_version: i16 = row.try_get("key_version")?;

    let smtp_password = SealedCredential {
        ciphertext: row.try_get("smtp_password_ct")?,
        nonce: row.try_get("smtp_password_nonce")?,
        dek_wrapped: row.try_get("dek_wrapped")?,
        dek_nonce: row.try_get("dek_nonce")?,
        key_version,
    };
    let imap_password = SealedCredential {
        ciphertext: row.try_get("imap_password_ct")?,
        nonce: row.try_get("imap_password_nonce")?,
        // The IMAP secret keeps its OWN wrapped DEK (0054); both columns are
        // NOT NULL and always written by `insert_account_tx`/`update_account_tx`.
        dek_wrapped: row.try_get("imap_dek_wrapped")?,
        dek_nonce: row.try_get("imap_dek_nonce")?,
        key_version,
    };

    Ok(StoredAccount {
        id: EmailAccountId::from_uuid(row.try_get("id")?),
        org_id: OrgId::from_uuid(row.try_get("org_id")?),
        display_name: row.try_get("display_name")?,
        email_address: row.try_get("email_address")?,
        from_name: row.try_get("from_name")?,
        imap_host: row.try_get("imap_host")?,
        imap_port: u16::try_from(row.try_get::<i32, _>("imap_port")?).unwrap_or(993),
        imap_security,
        imap_username: row.try_get("imap_username")?,
        smtp_host: row.try_get("smtp_host")?,
        smtp_port: u16::try_from(row.try_get::<i32, _>("smtp_port")?).unwrap_or(587),
        smtp_security,
        smtp_username: row.try_get("smtp_username")?,
        smtp_password,
        imap_password,
        status: row.try_get("status")?,
    })
}

/// The LISTEN/NOTIFY channel a webmail message-posted signal rides on. Mirrors
/// the messenger realtime channel: the payload carries only the account id (no
/// message content), and a background listener fans it out to the tenant's UI.
pub const MAIL_POSTED_CHANNEL: &str = "mail_posted";

/// A realtime [`MailNotifier`] over PostgreSQL `pg_notify`. Emitted when the mox
/// delivery webhook ingests a NEW inbound message so the tenant's mailbox UI can
/// refresh. Best-effort: a notify failure never fails the enclosing ingest.
#[derive(Debug, Clone)]
pub struct PgMailNotifier {
    pool: PgPool,
}

impl PgMailNotifier {
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

impl MailNotifier for PgMailNotifier {
    fn notify_posted(&self, account_id: EmailAccountId) -> MailFuture<'_, ()> {
        Box::pin(async move {
            // pg_notify carries only the account id — no tenant-table row is read
            // or written, so no org arming is needed here.
            if let Err(err) = sqlx::query("SELECT pg_notify($1, $2)")
                .bind(MAIL_POSTED_CHANNEL)
                .bind(account_id.to_string())
                // rls-arming: ok pg_notify carries an id only; it is not a tenant-table read/write
                .execute(&self.pool)
                .await
            {
                tracing::warn!(error = %err, "mail posted notify failed (best-effort)");
            }
        })
    }
}
