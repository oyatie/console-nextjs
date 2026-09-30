//! Postgres financial adapter.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

use std::collections::HashMap;

use console_financial_application::{
    AppendCostLedgerEntryCommand, AssetLifecycleCostSummary,
    ConfirmPurchaseAttachmentUploadCommand, CostLedgerEntrySummary, CostLedgerSource,
    CreatePurchaseRequestCommand, CreateRentalQuoteCommand, ExecutePurchaseCommand,
    FinancialConfigSnapshot, ListPurchaseRequestsQuery, PrepareExpenditureCommand,
    PreparePurchaseAttachmentUploadCommand, PurchaseApprovalCommand, PurchaseAttachmentDownload,
    PurchaseAttachmentSummary, PurchaseAttachmentUploadRecord, PurchaseFeaturePreferences,
    PurchasePolicySummary, PurchaseRequestLineInput, PurchaseRequestLineSummary,
    PurchaseRequestPage, PurchaseRequestSummary, PurchaseRequesterSummary, PurchaseRestartCommand,
    PurchaseSubmitCommand, PurchaseType, RejectPurchaseCommand, RentalQuoteSummary,
    financial_audit_event,
};
use console_financial_domain::{
    AcquisitionAnchor, MoneyInput, PurchaseActor, PurchaseStatus, PurchaseTransition,
    RentalQuoteInput, ResidualRecomputeInput, compute_rental_quote, cost_per_hour_won,
    cost_per_month_won, gross_margin_won, recompute_residual_value, tco_won,
    validate_purchase_transition,
};
use console_kernel_core::{
    AuditEvent, BranchId, EquipmentId, KernelError, OrgId, PurchaseRequestId, QuoteId,
    TraceContext, UserId, WorkOrderId,
};
use console_platform_db::{DbError, with_audit, with_audits, with_org_conn};
use console_platform_request_context::current_org;
use sqlx::{PgConnection, PgPool, Postgres, Row, Transaction};
use time::{Date, OffsetDateTime};

#[derive(Debug, thiserror::Error)]
pub enum PgFinancialError {
    #[error(transparent)]
    Db(#[from] DbError),

    #[error(transparent)]
    Domain(#[from] KernelError),
}

impl From<sqlx::Error> for PgFinancialError {
    fn from(value: sqlx::Error) -> Self {
        Self::Db(DbError::Sqlx(value))
    }
}

#[derive(Debug, Clone)]
pub struct PgFinancialStore {
    pool: PgPool,
}

impl PgFinancialStore {
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    #[must_use]
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    pub async fn prepare_purchase_attachment_upload(
        &self,
        command: PreparePurchaseAttachmentUploadCommand,
    ) -> Result<PurchaseAttachmentUploadRecord, PgFinancialError> {
        validate_required(&command.file_name, "attachment file name")?;
        validate_required(&command.content_type, "attachment content type")?;
        validate_required(&command.role, "attachment role")?;
        validate_required(&command.s3_bucket, "attachment bucket")?;
        validate_required(&command.s3_key, "attachment storage key")?;
        if command.size_bytes <= 0 {
            return Err(KernelError::validation("attachment size must be positive").into());
        }
        if command.size_bytes > 25 * 1024 * 1024 {
            return Err(KernelError::validation("purchase attachment exceeds 25 MiB").into());
        }
        if !matches!(command.role.as_str(), "QUOTE" | "INVOICE" | "OTHER") {
            return Err(KernelError::validation("unsupported purchase attachment role").into());
        }

        let attachment_id = uuid::Uuid::new_v4();
        let org = current_org().map_err(KernelError::from)?;
        let org_uuid = *org.as_uuid();
        let event = financial_audit_event(
            "purchase.attachment.presign",
            command.actor,
            command.branch_id,
            "financial_purchase_attachment",
            attachment_id,
            command.trace,
            command.occurred_at,
        )?
        .with_org(org);

        with_audit::<_, PurchaseAttachmentUploadRecord, PgFinancialError>(&self.pool, event, |tx| {
            Box::pin(async move {
                ensure_branch_exists_tx(tx, command.branch_id).await?;
                sqlx::query(
                    r#"
                        INSERT INTO financial_purchase_attachments (
                            id, branch_id, uploaded_by, role, file_name, content_type,
                            size_bytes, s3_bucket, s3_key, checksum_sha256, upload_state,
                            created_at, org_id
                        )
                        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, 'PENDING', $11, $12)
                        "#,
                )
                .bind(attachment_id)
                .bind(*command.branch_id.as_uuid())
                .bind(*command.actor.as_uuid())
                .bind(command.role.trim())
                .bind(command.file_name.trim())
                .bind(command.content_type.trim())
                .bind(command.size_bytes)
                .bind(command.s3_bucket.trim())
                .bind(command.s3_key.trim())
                .bind(command.checksum_sha256.as_deref())
                .bind(command.occurred_at)
                .bind(org_uuid)
                .execute(tx.as_mut())
                .await?;
                purchase_attachment_upload_record_tx(tx, attachment_id).await
            })
        })
        .await
    }

    pub async fn purchase_attachment_upload_record(
        &self,
        attachment_id: uuid::Uuid,
    ) -> Result<PurchaseAttachmentUploadRecord, PgFinancialError> {
        let org = current_org().map_err(KernelError::from)?;
        with_org_conn::<_, _, PgFinancialError>(&self.pool, org, move |conn| {
            Box::pin(
                async move { purchase_attachment_upload_record_conn(conn, attachment_id).await },
            )
        })
        .await
    }
    pub async fn confirm_purchase_attachment_upload(
        &self,
        command: ConfirmPurchaseAttachmentUploadCommand,
    ) -> Result<PurchaseAttachmentUploadRecord, PgFinancialError> {
        let org = current_org().map_err(KernelError::from)?;
        with_audits::<_, PurchaseAttachmentUploadRecord, PgFinancialError>(&self.pool, org, |tx| {
            Box::pin(async move {
                let record =
                    purchase_attachment_upload_record_tx(tx, command.attachment_id).await?;
                let result = sqlx::query(
                    r#"
                    UPDATE financial_purchase_attachments
                    SET upload_state = 'CONFIRMED'
                    WHERE id = $1
                      AND uploaded_by = $2
                      AND upload_state IN ('PENDING', 'CONFIRMED')
                    "#,
                )
                .bind(command.attachment_id)
                .bind(*command.actor.as_uuid())
                .execute(tx.as_mut())
                .await?;
                if result.rows_affected() != 1 {
                    return Err(KernelError::forbidden(
                        "purchase attachment is not owned by this user or cannot be confirmed",
                    )
                    .into());
                }
                let updated =
                    purchase_attachment_upload_record_tx(tx, command.attachment_id).await?;
                let event = financial_audit_event(
                    "purchase.attachment.confirm",
                    command.actor,
                    record.branch_id,
                    "financial_purchase_attachment",
                    command.attachment_id,
                    command.trace,
                    command.occurred_at,
                )?
                .with_org(org);
                Ok((updated, vec![event]))
            })
        })
        .await
    }

    pub async fn purchase_attachment_download(
        &self,
        purchase_request_id: PurchaseRequestId,
        attachment_id: uuid::Uuid,
    ) -> Result<PurchaseAttachmentDownload, PgFinancialError> {
        let org = current_org().map_err(KernelError::from)?;
        with_org_conn::<_, PurchaseAttachmentDownload, PgFinancialError>(&self.pool, org, |tx| {
            Box::pin(async move {
                let row = sqlx::query(
                    r#"
                    SELECT file_name, content_type, s3_bucket, s3_key
                    FROM financial_purchase_attachments
                    WHERE id = $1
                      AND purchase_request_id = $2
                      AND upload_state = 'CONFIRMED'
                    "#,
                )
                .bind(attachment_id)
                .bind(*purchase_request_id.as_uuid())
                .fetch_optional(tx.as_mut())
                .await?
                .ok_or_else(|| KernelError::not_found("purchase attachment not found"))?;
                Ok(PurchaseAttachmentDownload {
                    file_name: row.try_get("file_name")?,
                    content_type: row.try_get("content_type")?,
                    s3_bucket: row.try_get("s3_bucket")?,
                    s3_key: row.try_get("s3_key")?,
                })
            })
        })
        .await
    }

    pub async fn purchase_feature_preferences(
        &self,
        user_id: UserId,
        feature_key: &str,
    ) -> Result<PurchaseFeaturePreferences, PgFinancialError> {
        validate_feature_key(feature_key)?;
        let feature_key = feature_key.to_owned();
        let org = current_org().map_err(KernelError::from)?;
        with_org_conn::<_, PurchaseFeaturePreferences, PgFinancialError>(&self.pool, org, |tx| {
            Box::pin(async move {
                let row = sqlx::query(
                    r#"
                    SELECT schema_version, preferences_json
                    FROM user_feature_preferences
                    WHERE user_id = $1 AND feature_key = $2
                    "#,
                )
                .bind(*user_id.as_uuid())
                .bind(&feature_key)
                .fetch_optional(tx.as_mut())
                .await?;
                if let Some(row) = row {
                    Ok(PurchaseFeaturePreferences {
                        feature_key,
                        schema_version: row.try_get("schema_version")?,
                        preferences: row.try_get("preferences_json")?,
                    })
                } else {
                    Ok(PurchaseFeaturePreferences {
                        feature_key,
                        schema_version: 1,
                        preferences: serde_json::json!({}),
                    })
                }
            })
        })
        .await
    }

    pub async fn save_purchase_feature_preferences(
        &self,
        user_id: UserId,
        feature_key: &str,
        schema_version: i32,
        preferences: serde_json::Value,
    ) -> Result<PurchaseFeaturePreferences, PgFinancialError> {
        validate_feature_key(feature_key)?;
        validate_purchase_preferences(schema_version, &preferences)?;
        let feature_key = feature_key.to_owned();
        let org = current_org().map_err(KernelError::from)?;
        let org_uuid = *org.as_uuid();
        with_org_conn::<_, PurchaseFeaturePreferences, PgFinancialError>(&self.pool, org, |tx| {
            Box::pin(async move {
                sqlx::query(
                    r#"
                    INSERT INTO user_feature_preferences (
                        user_id, feature_key, preferences_json, schema_version,
                        created_at, updated_at, org_id
                    )
                    VALUES ($1, $2, $3, $4, now(), now(), $5)
                    ON CONFLICT (org_id, user_id, feature_key)
                    DO UPDATE SET
                        preferences_json = EXCLUDED.preferences_json,
                        schema_version = EXCLUDED.schema_version,
                        updated_at = now()
                    "#,
                )
                .bind(*user_id.as_uuid())
                .bind(&feature_key)
                .bind(&preferences)
                .bind(schema_version)
                .bind(org_uuid)
                .execute(tx.as_mut())
                .await?;
                Ok(PurchaseFeaturePreferences {
                    feature_key,
                    schema_version,
                    preferences,
                })
            })
        })
        .await
    }

    pub async fn create_rental_quote(
        &self,
        command: CreateRentalQuoteCommand,
    ) -> Result<RentalQuoteSummary, PgFinancialError> {
        let quote_id = QuoteId::new();
        let org = current_org().map_err(KernelError::from)?;
        let org_uuid = *org.as_uuid();
        let event = financial_audit_event(
            "financial.quote.create",
            command.actor,
            command.branch_id,
            "financial_rental_quote",
            quote_id,
            command.trace,
            command.occurred_at,
        )?
        .with_org(org);

        with_audit::<_, RentalQuoteSummary, PgFinancialError>(&self.pool, event, |tx| {
            Box::pin(async move {
                let equipment = equipment_economics_tx(tx, command.equipment_id).await?;
                ensure_branch(equipment.branch_id, command.branch_id)?;
                let vehicle_value_won = equipment.require_vehicle_value()?;
                let cumulative_repair_cost =
                    cumulative_cost_tx(tx, command.equipment_id, None).await?;
                let quote = compute_rental_quote(RentalQuoteInput {
                    acquisition_value: MoneyInput::won(vehicle_value_won),
                    current_residual_value: MoneyInput::won(equipment.residual_value_won),
                    cumulative_repair_cost: MoneyInput::won(cumulative_repair_cost),
                    config: command.config.quote_config(),
                })?;

                insert_quote_tx(
                    tx,
                    quote_id,
                    command.actor,
                    command.branch_id,
                    command.equipment_id,
                    vehicle_value_won,
                    equipment.residual_value_won,
                    cumulative_repair_cost,
                    &command.config,
                    &quote,
                    command.occurred_at,
                    org_uuid,
                )
                .await?;
                rental_quote_by_id_tx(tx, quote_id).await
            })
        })
        .await
    }

    pub async fn append_cost_ledger_entry(
        &self,
        command: AppendCostLedgerEntryCommand,
    ) -> Result<CostLedgerEntrySummary, PgFinancialError> {
        self.append_cost_ledger_entry_with_purchase(command, None)
            .await
    }

    pub async fn create_purchase_request(
        &self,
        command: CreatePurchaseRequestCommand,
    ) -> Result<PurchaseRequestSummary, PgFinancialError> {
        validate_required(&command.vendor_name, "vendor name")?;
        validate_required(&command.memo, "purchase memo")?;
        let computed_lines = compute_purchase_lines(command.amount_won, &command.lines)?;
        let amount_won = purchase_total(&computed_lines)?;

        let purchase_request_id = PurchaseRequestId::new();
        let org = current_org().map_err(KernelError::from)?;
        let org_uuid = *org.as_uuid();
        let event = financial_audit_event(
            "purchase.request.create",
            command.actor,
            command.branch_id,
            "financial_purchase_request",
            purchase_request_id,
            command.trace,
            command.occurred_at,
        )?
        .with_org(org);

        with_audit::<_, PurchaseRequestSummary, PgFinancialError>(&self.pool, event, |tx| {
            Box::pin(async move {
                ensure_branch_exists_tx(tx, command.branch_id).await?;
                let equipment_required = purchase_equipment_required(org_uuid);
                if equipment_required && command.equipment_id.is_none() {
                    return Err(KernelError::validation(
                        "equipment is required for KNL maintenance purchases",
                    )
                    .into());
                }

                if let Some(equipment_id) = command.equipment_id {
                    let equipment = equipment_economics_tx(tx, equipment_id).await?;
                    ensure_branch(equipment.branch_id, command.branch_id)?;
                    if let Some(work_order_id) = command.work_order_id {
                        ensure_work_order_matches_tx(
                            tx,
                            work_order_id,
                            command.branch_id,
                            equipment_id,
                        )
                        .await?;
                    }
                }

                let statement = if let Some(statement_evidence_id) = command.statement_evidence_id {
                    let equipment_id = command.equipment_id.ok_or_else(|| {
                        KernelError::validation(
                            "statement evidence requires an equipment-scoped purchase",
                        )
                    })?;
                    Some(
                        ensure_statement_evidence_tx(
                            tx,
                            statement_evidence_id,
                            command.branch_id,
                            equipment_id,
                            command.work_order_id,
                        )
                        .await?,
                    )
                } else {
                    None
                };

                if command.equipment_id.is_some() && command.statement_evidence_id.is_none() {
                    return Err(KernelError::validation(
                        "statement evidence is required for equipment purchases",
                    )
                    .into());
                }

                let work_order_id = statement
                    .as_ref()
                    .map(|link| link.work_order_id)
                    .or(command.work_order_id);

                let policy = purchase_policy_flags_tx(
                    tx,
                    command.branch_id,
                    command.purchase_type,
                    &command.vendor_name,
                    &computed_lines,
                    !command.quote_attachment_ids.is_empty(),
                )
                .await?;

                sqlx::query(
                    r#"
                    INSERT INTO financial_purchase_requests (
                        id, branch_id, equipment_id, work_order_id, statement_evidence_id,
                        purchase_type, vendor_name, amount_won, memo, status, requested_by,
                        depreciation_method, useful_life_months, residual_rate_bps,
                        declining_balance_rate_bps, management_fee_rate_bps,
                        profit_rate_bps, floor_negative_quote_residual,
                        executive_threshold_won, price_anomaly, quote_update_required,
                        created_at, updated_at, org_id
                    )
                    VALUES (
                        $1, $2, $3, $4, $5,
                        $6, $7, $8, $9, $10, $11,
                        $12, $13, $14, $15, $16, $17, $18,
                        $19, $20, $21, $22, $22, $23
                    )
                    "#,
                )
                .bind(*purchase_request_id.as_uuid())
                .bind(*command.branch_id.as_uuid())
                .bind(command.equipment_id.map(|id| *id.as_uuid()))
                .bind(work_order_id.map(|id| *id.as_uuid()))
                .bind(command.statement_evidence_id.map(|id| *id.as_uuid()))
                .bind(command.purchase_type.as_db_str())
                .bind(command.vendor_name.trim())
                .bind(amount_won)
                .bind(command.memo.trim())
                .bind(PurchaseStatus::StatementAttached.as_db_str())
                .bind(*command.actor.as_uuid())
                .bind(command.config.depreciation_method.as_db_str())
                .bind(
                    i32::try_from(command.config.useful_life_months).map_err(|_| {
                        KernelError::validation("useful life months overflowed i32")
                    })?,
                )
                .bind(command.config.residual_rate_bps)
                .bind(command.config.declining_balance_rate_bps)
                .bind(command.config.management_fee_rate_bps)
                .bind(command.config.profit_rate_bps)
                .bind(command.config.floor_negative_quote_residual)
                .bind(command.config.executive_approval_threshold_won)
                .bind(policy.price_anomaly)
                .bind(policy.quote_update_required)
                .bind(command.occurred_at)
                .bind(org_uuid)
                .execute(tx.as_mut())
                .await?;

                insert_purchase_lines_tx(tx, purchase_request_id, &computed_lines, org_uuid)
                    .await?;
                attach_purchase_attachments_tx(
                    tx,
                    purchase_request_id,
                    command.branch_id,
                    &command.quote_attachment_ids,
                )
                .await?;
                if command.purchase_type == PurchaseType::Regular
                    && !policy.quote_update_required
                    && !command.quote_attachment_ids.is_empty()
                {
                    upsert_regular_purchase_prices_tx(
                        tx,
                        RegularPurchasePriceUpsert {
                            purchase_request_id,
                            branch_id: command.branch_id,
                            vendor_name: &command.vendor_name,
                            lines: &computed_lines,
                            quote_attachment_id: command.quote_attachment_ids.first().copied(),
                            updated_at: command.occurred_at,
                            org_uuid,
                        },
                    )
                    .await?;
                }

                insert_purchase_history_tx(
                    tx,
                    purchase_request_id,
                    command.actor,
                    "purchase.request.create",
                    None,
                    PurchaseStatus::StatementAttached,
                    Some(command.memo.trim()),
                    command.occurred_at,
                    org_uuid,
                )
                .await?;
                purchase_by_id_tx(tx, purchase_request_id).await
            })
        })
        .await
    }

    pub async fn submit_purchase_request(
        &self,
        command: PurchaseSubmitCommand,
    ) -> Result<PurchaseRequestSummary, PgFinancialError> {
        self.transition_purchase(
            command.actor,
            command.purchase_request_id,
            "purchase.submit",
            PurchaseStatus::RequestSubmitted,
            None,
            None,
            command.trace,
            command.occurred_at,
        )
        .await
    }

    pub async fn approve_purchase_admin(
        &self,
        command: PurchaseApprovalCommand,
    ) -> Result<PurchaseRequestSummary, PgFinancialError> {
        self.transition_purchase(
            command.actor,
            command.purchase_request_id,
            "purchase.admin.approve",
            PurchaseStatus::AdminApproved,
            None,
            None,
            command.trace,
            command.occurred_at,
        )
        .await
    }

    pub async fn prepare_expenditure(
        &self,
        command: PrepareExpenditureCommand,
    ) -> Result<PurchaseRequestSummary, PgFinancialError> {
        validate_required(&command.expenditure_no, "expenditure number")?;
        // The target status is derived under the FOR UPDATE lock inside
        // `transition_purchase` (it recomputes ExecutivePending vs ReadyToExecute
        // from the locked row whenever the request set is one of those two), so we
        // pass either as the requested target and let the in-lock recompute win.
        // This avoids a redundant unlocked read of the purchase + threshold here.
        self.transition_purchase(
            command.actor,
            command.purchase_request_id,
            "purchase.expenditure.prepare",
            PurchaseStatus::ReadyToExecute,
            Some(command.expenditure_no),
            None,
            command.trace,
            command.occurred_at,
        )
        .await
    }

    pub async fn approve_purchase_executive(
        &self,
        command: PurchaseApprovalCommand,
    ) -> Result<PurchaseRequestSummary, PgFinancialError> {
        self.transition_purchase(
            command.actor,
            command.purchase_request_id,
            "purchase.executive.approve",
            PurchaseStatus::ReadyToExecute,
            None,
            None,
            command.trace,
            command.occurred_at,
        )
        .await
    }

    pub async fn reject_purchase_request(
        &self,
        command: RejectPurchaseCommand,
    ) -> Result<PurchaseRequestSummary, PgFinancialError> {
        validate_required(&command.memo, "reject memo")?;
        self.transition_purchase(
            command.actor,
            command.purchase_request_id,
            "purchase.reject",
            PurchaseStatus::Rejected,
            None,
            Some(command.memo),
            command.trace,
            command.occurred_at,
        )
        .await
    }

    pub async fn restart_purchase_request(
        &self,
        command: PurchaseRestartCommand,
    ) -> Result<PurchaseRequestSummary, PgFinancialError> {
        validate_required(&command.memo, "restart memo")?;
        let computed_lines = compute_purchase_lines(command.amount_won, &command.lines)?;
        let amount_won = purchase_total(&computed_lines)?;
        let event_purchase = self.purchase_request(command.purchase_request_id).await?;
        let org = current_org().map_err(KernelError::from)?;
        let org_uuid = *org.as_uuid();
        let event = financial_audit_event(
            "purchase.restart",
            command.actor,
            event_purchase.branch_id,
            "financial_purchase_request",
            command.purchase_request_id,
            command.trace,
            command.occurred_at,
        )?
        .with_org(org);

        with_audit::<_, PurchaseRequestSummary, PgFinancialError>(&self.pool, event, |tx| {
            Box::pin(async move {
                let row = lock_purchase_tx(tx, command.purchase_request_id).await?;
                let from = row.status;
                validate_purchase_transition(PurchaseTransition {
                    from,
                    to: PurchaseStatus::StatementAttached,
                    actor: purchase_actor_for_user_tx(tx, command.actor).await?,
                    amount_won,
                    executive_threshold_won: row.executive_threshold_won,
                })?;

                let statement = if let Some(statement_evidence_id) = command.statement_evidence_id {
                    let equipment_id = row.equipment_id.ok_or_else(|| {
                        KernelError::validation(
                            "statement evidence requires an equipment-scoped purchase",
                        )
                    })?;
                    Some(
                        ensure_statement_evidence_tx(
                            tx,
                            statement_evidence_id,
                            row.branch_id,
                            equipment_id,
                            row.work_order_id,
                        )
                        .await?,
                    )
                } else {
                    None
                };
                if row.equipment_id.is_some() && command.statement_evidence_id.is_none() {
                    return Err(KernelError::validation(
                        "statement evidence is required for equipment purchases",
                    )
                    .into());
                }
                let work_order_id = statement
                    .as_ref()
                    .map(|link| link.work_order_id)
                    .or(row.work_order_id);

                let policy = purchase_policy_flags_tx(
                    tx,
                    row.branch_id,
                    event_purchase.purchase_type,
                    &row.vendor_name,
                    &computed_lines,
                    !command.quote_attachment_ids.is_empty(),
                )
                .await?;

                sqlx::query(
                    r#"
                    UPDATE financial_purchase_requests
                    SET status = 'STATEMENT_ATTACHED',
                        statement_evidence_id = $2,
                        amount_won = $3,
                        memo = $4,
                        work_order_id = $6,
                        price_anomaly = $7,
                        quote_update_required = $8,
                        expenditure_no = NULL,
                        submitted_by = NULL,
                        admin_approved_by = NULL,
                        executive_approved_by = NULL,
                        executed_by = NULL,
                        rejected_by = NULL,
                        rejection_memo = NULL,
                        updated_at = $5
                    WHERE id = $1
                    "#,
                )
                .bind(*command.purchase_request_id.as_uuid())
                .bind(command.statement_evidence_id.map(|id| *id.as_uuid()))
                .bind(amount_won)
                .bind(command.memo.trim())
                .bind(command.occurred_at)
                .bind(work_order_id.map(|id| *id.as_uuid()))
                .bind(policy.price_anomaly)
                .bind(policy.quote_update_required)
                .execute(tx.as_mut())
                .await?;

                replace_purchase_lines_tx(
                    tx,
                    command.purchase_request_id,
                    &computed_lines,
                    org_uuid,
                )
                .await?;
                attach_purchase_attachments_tx(
                    tx,
                    command.purchase_request_id,
                    row.branch_id,
                    &command.quote_attachment_ids,
                )
                .await?;
                if event_purchase.purchase_type == PurchaseType::Regular
                    && !policy.quote_update_required
                    && !command.quote_attachment_ids.is_empty()
                {
                    upsert_regular_purchase_prices_tx(
                        tx,
                        RegularPurchasePriceUpsert {
                            purchase_request_id: command.purchase_request_id,
                            branch_id: row.branch_id,
                            vendor_name: &row.vendor_name,
                            lines: &computed_lines,
                            quote_attachment_id: command.quote_attachment_ids.first().copied(),
                            updated_at: command.occurred_at,
                            org_uuid,
                        },
                    )
                    .await?;
                }
                insert_purchase_history_tx(
                    tx,
                    command.purchase_request_id,
                    command.actor,
                    "purchase.restart",
                    Some(from),
                    PurchaseStatus::StatementAttached,
                    Some(command.memo.trim()),
                    command.occurred_at,
                    org_uuid,
                )
                .await?;
                purchase_by_id_tx(tx, command.purchase_request_id).await
            })
        })
        .await
    }

    pub async fn execute_purchase(
        &self,
        command: ExecutePurchaseCommand,
    ) -> Result<PurchaseRequestSummary, PgFinancialError> {
        let org = current_org().map_err(KernelError::from)?;
        let org_uuid = *org.as_uuid();
        with_audits::<_, PurchaseRequestSummary, PgFinancialError>(&self.pool, org, |tx| {
            Box::pin(async move {
                let row = lock_purchase_tx(tx, command.purchase_request_id).await?;
                validate_purchase_transition(PurchaseTransition {
                    from: row.status,
                    to: PurchaseStatus::Executed,
                    actor: purchase_actor_for_user_tx(tx, command.actor).await?,
                    amount_won: row.amount_won,
                    executive_threshold_won: row.executive_threshold_won,
                })?;

                sqlx::query(
                    r#"
                    UPDATE financial_purchase_requests
                    SET status = 'EXECUTED',
                        executed_by = $2,
                        updated_at = $3
                    WHERE id = $1
                    "#,
                )
                .bind(*command.purchase_request_id.as_uuid())
                .bind(*command.actor.as_uuid())
                .bind(command.occurred_at)
                .execute(tx.as_mut())
                .await?;
                insert_purchase_history_tx(
                    tx,
                    command.purchase_request_id,
                    command.actor,
                    "purchase.execute",
                    Some(row.status),
                    PurchaseStatus::Executed,
                    None,
                    command.occurred_at,
                    org_uuid,
                )
                .await?;

                let purchase = purchase_by_id_tx(tx, command.purchase_request_id).await?;
                let purchase_event = financial_audit_event(
                    "purchase.execute",
                    command.actor,
                    row.branch_id,
                    "financial_purchase_request",
                    command.purchase_request_id,
                    command.trace.clone(),
                    command.occurred_at,
                )?
                .with_org(org);

                if let Some(equipment_id) = row.equipment_id {
                    let ledger_command = AppendCostLedgerEntryCommand {
                        actor: command.actor,
                        branch_id: row.branch_id,
                        equipment_id,
                        work_order_id: row.work_order_id,
                        source: CostLedgerSource::PurchaseExecution,
                        amount_won: row.amount_won,
                        memo: format!("purchase execution {}", command.purchase_request_id),
                        config: row.config,
                        trace: command.trace,
                        occurred_at: command.occurred_at,
                    };
                    let (_, residual_event) = append_cost_ledger_entry_tx(
                        tx,
                        ledger_command,
                        Some(command.purchase_request_id),
                        org_uuid,
                    )
                    .await?;
                    Ok((purchase, vec![purchase_event, residual_event]))
                } else {
                    let expense_event = insert_expense_ledger_tx(
                        tx,
                        ExpenseLedgerInsert {
                            purchase_request_id: command.purchase_request_id,
                            actor: command.actor,
                            branch_id: row.branch_id,
                            vendor_name: &row.vendor_name,
                            amount_won: row.amount_won,
                            expenditure_no: row.expenditure_no.as_deref(),
                            occurred_at: command.occurred_at,
                            org_uuid,
                        },
                    )
                    .await?;
                    Ok((purchase, vec![purchase_event, expense_event]))
                }
            })
        })
        .await
    }

    pub async fn purchase_request(
        &self,
        purchase_request_id: PurchaseRequestId,
    ) -> Result<PurchaseRequestSummary, PgFinancialError> {
        purchase_by_id(&self.pool, purchase_request_id).await
    }

    /// Lists only rows already narrowed by the caller's tenant RLS context and
    /// by the branch/requester predicate supplied at the authorization boundary.
    pub async fn list_purchase_requests(
        &self,
        query: ListPurchaseRequestsQuery,
    ) -> Result<PurchaseRequestPage, PgFinancialError> {
        let org = current_org().map_err(KernelError::from)?;
        with_org_conn::<_, _, PgFinancialError>(&self.pool, org, move |tx| {
            Box::pin(async move { list_purchase_requests_tx(tx, query).await })
        })
        .await
    }

    pub async fn rental_quote(
        &self,
        quote_id: QuoteId,
    ) -> Result<RentalQuoteSummary, PgFinancialError> {
        rental_quote_by_id(&self.pool, quote_id).await
    }

    pub async fn cost_ledger_for_equipment(
        &self,
        equipment_id: EquipmentId,
    ) -> Result<Vec<CostLedgerEntrySummary>, PgFinancialError> {
        cost_ledger_for_equipment(&self.pool, equipment_id).await
    }

    pub async fn lifecycle_cost_for_equipment(
        &self,
        equipment_id: EquipmentId,
    ) -> Result<AssetLifecycleCostSummary, PgFinancialError> {
        lifecycle_cost_for_equipment(&self.pool, equipment_id).await
    }

    pub async fn equipment_branch(
        &self,
        equipment_id: EquipmentId,
    ) -> Result<BranchId, PgFinancialError> {
        Ok(equipment_economics(&self.pool, equipment_id)
            .await?
            .branch_id)
    }

    async fn append_cost_ledger_entry_with_purchase(
        &self,
        command: AppendCostLedgerEntryCommand,
        purchase_request_id: Option<PurchaseRequestId>,
    ) -> Result<CostLedgerEntrySummary, PgFinancialError> {
        validate_required(&command.memo, "cost ledger memo")?;
        if command.amount_won <= 0 {
            return Err(KernelError::validation("cost ledger amount must be positive").into());
        }

        let org = current_org().map_err(KernelError::from)?;
        let org_uuid = *org.as_uuid();
        with_audits::<_, CostLedgerEntrySummary, PgFinancialError>(&self.pool, org, |tx| {
            Box::pin(async move {
                let (entry, event) =
                    append_cost_ledger_entry_tx(tx, command, purchase_request_id, org_uuid).await?;
                Ok((entry, vec![event]))
            })
        })
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn transition_purchase(
        &self,
        actor: UserId,
        purchase_request_id: PurchaseRequestId,
        action: &'static str,
        requested_to: PurchaseStatus,
        expenditure_no: Option<String>,
        memo: Option<String>,
        trace: console_kernel_core::TraceContext,
        occurred_at: OffsetDateTime,
    ) -> Result<PurchaseRequestSummary, PgFinancialError> {
        let purchase = self.purchase_request(purchase_request_id).await?;
        let org = current_org().map_err(KernelError::from)?;
        let org_uuid = *org.as_uuid();
        let event = financial_audit_event(
            action,
            actor,
            purchase.branch_id,
            "financial_purchase_request",
            purchase_request_id,
            trace,
            occurred_at,
        )?
        .with_org(org);

        with_audit::<_, PurchaseRequestSummary, PgFinancialError>(&self.pool, event, |tx| {
            Box::pin(async move {
                let row = lock_purchase_tx(tx, purchase_request_id).await?;
                // These methods represent different acts even when their target is identical.
                // Check the locked source before threshold normalization.
                if (action == "purchase.expenditure.prepare" && row.status != PurchaseStatus::AdminApproved)
                    || (action == "purchase.executive.approve" && row.status != PurchaseStatus::ExecutivePending)
                {
                    return Err(KernelError::validation("purchase action is not valid in the current state").into());
                }
                let to = if row.status == PurchaseStatus::AdminApproved
                    && matches!(
                        requested_to,
                        PurchaseStatus::ReadyToExecute | PurchaseStatus::ExecutivePending
                    ) {
                    if row.amount_won > row.executive_threshold_won {
                        PurchaseStatus::ExecutivePending
                    } else {
                        PurchaseStatus::ReadyToExecute
                    }
                } else {
                    requested_to
                };
                validate_purchase_transition(PurchaseTransition {
                    from: row.status,
                    to,
                    actor: purchase_actor_for_user_tx(tx, actor).await?,
                    amount_won: row.amount_won,
                    executive_threshold_won: row.executive_threshold_won,
                })?;

                // ── Deferred WORM compliance gate (SUBMIT only) ───────────────
                // A purchase may be CREATED against still-replicating evidence,
                // but it must not enter the approval pipeline until the linked
                // 거래명세표 is durably preserved (worm_replica_status VERIFIED).
                // Guarded strictly on the submit target so the check never leaks
                // into approve/execute/reject, which share this method.
                if to == PurchaseStatus::RequestSubmitted {
                    if row.quote_update_required {
                        return Err(KernelError::validation(
                            "quote update required before submitting this purchase request",
                        )
                        .into());
                    }
                    if let Some(statement_evidence_id) = row.statement_evidence_id {
                        ensure_statement_evidence_verified_tx(tx, statement_evidence_id).await?;
                    }
                }

                if matches!((row.status, to),
                    (PurchaseStatus::RequestSubmitted, PurchaseStatus::AdminApproved)
                    | (PurchaseStatus::ExecutivePending, PurchaseStatus::ReadyToExecute))
                {
                    check_self_approval(&row, actor)?;
                }
                if action == "purchase.expenditure.prepare" {
                    let submitted_by = row.submitted_by.ok_or_else(|| {
                        KernelError::forbidden("상신자를 확인할 수 없어 지출을 준비할 수 없습니다")
                    })?;
                    let approver = row.admin_approved_by.ok_or_else(|| {
                        KernelError::forbidden("독립된 관리자 승인 기록이 필요합니다")
                    })?;
                    if approver == row.requested_by || approver == submitted_by {
                        return Err(KernelError::forbidden("독립된 관리자 승인 기록이 필요합니다").into());
                    }
                }

                let memo_trimmed = memo.as_deref().map(str::trim).filter(|value| !value.is_empty());
                let expenditure_trimmed = expenditure_no
                    .as_deref()
                    .map(str::trim)
                    .filter(|value| !value.is_empty());
                sqlx::query(
                    r#"
                    UPDATE financial_purchase_requests
                    SET status = $2,
                        expenditure_no = COALESCE($3, expenditure_no),
                        submitted_by = CASE WHEN $2 = 'REQUEST_SUBMITTED' THEN $4 ELSE submitted_by END,
                        admin_approved_by = CASE WHEN $2 = 'ADMIN_APPROVED' THEN $4 ELSE admin_approved_by END,
                        executive_approved_by = CASE
                            WHEN $2 = 'READY_TO_EXECUTE' AND $5 = 'EXECUTIVE_PENDING' THEN $4
                            ELSE executive_approved_by
                        END,
                        executed_by = CASE WHEN $2 = 'EXECUTED' THEN $4 ELSE executed_by END,
                        rejected_by = CASE WHEN $2 = 'REJECTED' THEN $4 ELSE rejected_by END,
                        rejection_memo = CASE WHEN $2 = 'REJECTED' THEN $6 ELSE rejection_memo END,
                        updated_at = $7
                    WHERE id = $1
                    "#,
                )
                .bind(*purchase_request_id.as_uuid())
                .bind(to.as_db_str())
                .bind(expenditure_trimmed)
                .bind(*actor.as_uuid())
                .bind(row.status.as_db_str())
                .bind(memo_trimmed)
                .bind(occurred_at)
                .execute(tx.as_mut())
                .await?;
                insert_purchase_history_tx(
                    tx,
                    purchase_request_id,
                    actor,
                    action,
                    Some(row.status),
                    to,
                    memo_trimmed,
                    occurred_at,
                    org_uuid,
                )
                .await?;
                purchase_by_id_tx(tx, purchase_request_id).await
            })
        })
        .await
    }
}

#[derive(Debug, Clone, Copy)]
struct EquipmentEconomics {
    branch_id: BranchId,
    /// Depreciation acquisition base. `None` for a bare asset that carries no
    /// `vehicle_value`; read paths that only need `branch_id` (e.g. the
    /// lifecycle-cost endpoint's branch lookup) tolerate this, while the
    /// depreciation/quote write paths demand it via
    /// [`EquipmentEconomics::require_vehicle_value`].
    vehicle_value_won: Option<i64>,
    residual_value_won: i64,
    asset_registered_on: Option<Date>,
}

impl EquipmentEconomics {
    /// The depreciation acquisition base, required by the quote/residual write
    /// paths. A bare asset with no `vehicle_value` cannot be depreciated, so this
    /// is a validation error there — but it is NEVER reached by the read paths
    /// that only need `branch_id`.
    fn require_vehicle_value(&self) -> Result<i64, PgFinancialError> {
        self.vehicle_value_won
            .ok_or_else(|| KernelError::validation("equipment vehicle value is required").into())
    }
}

#[derive(Debug, Clone)]
struct LockedPurchase {
    requested_by: uuid::Uuid,
    submitted_by: Option<uuid::Uuid>,
    admin_approved_by: Option<uuid::Uuid>,
    branch_id: BranchId,
    equipment_id: Option<EquipmentId>,
    work_order_id: Option<WorkOrderId>,
    statement_evidence_id: Option<console_kernel_core::EvidenceId>,
    status: PurchaseStatus,
    amount_won: i64,
    executive_threshold_won: i64,
    config: FinancialConfigSnapshot,
    quote_update_required: bool,
    vendor_name: String,
    expenditure_no: Option<String>,
}

#[derive(Debug, Clone)]
struct ComputedPurchaseLine {
    line_no: i32,
    item: String,
    quantity: i32,
    unit_supply_price_won: i64,
    vat_won: i64,
    vat_overridden: bool,
    line_total_won: i64,
}

#[derive(Debug, Clone)]
struct PurchasePolicyFlags {
    price_anomaly: bool,
    quote_update_required: bool,
}

#[derive(Debug, Clone, Copy)]
struct StatementEvidenceLink {
    work_order_id: WorkOrderId,
}

async fn equipment_economics(
    pool: &PgPool,
    equipment_id: EquipmentId,
) -> Result<EquipmentEconomics, PgFinancialError> {
    let org = current_org().map_err(KernelError::from)?;
    let row = with_org_conn::<_, _, PgFinancialError>(pool, org, move |tx| {
        Box::pin(async move {
            Ok(sqlx::query(
                r#"
        SELECT branch_id, vehicle_value, residual_value, asset_registered_on
        FROM registry_equipment
        WHERE id = $1
        "#,
            )
            .bind(*equipment_id.as_uuid())
            .fetch_optional(tx.as_mut())
            .await?)
        })
    })
    .await?
    .ok_or_else(|| KernelError::not_found("equipment was not found"))?;
    equipment_economics_from_row(&row)
}

async fn equipment_economics_tx(
    tx: &mut Transaction<'_, Postgres>,
    equipment_id: EquipmentId,
) -> Result<EquipmentEconomics, PgFinancialError> {
    let row = sqlx::query(
        r#"
        SELECT branch_id, vehicle_value, residual_value, asset_registered_on
        FROM registry_equipment
        WHERE id = $1
        FOR UPDATE
        "#,
    )
    .bind(*equipment_id.as_uuid())
    .fetch_optional(tx.as_mut())
    .await?
    .ok_or_else(|| KernelError::not_found("equipment was not found"))?;
    equipment_economics_from_row(&row)
}

fn equipment_economics_from_row(
    row: &sqlx::postgres::PgRow,
) -> Result<EquipmentEconomics, PgFinancialError> {
    let vehicle_value_won: Option<i64> = row.try_get("vehicle_value")?;
    let residual_value_won: Option<i64> = row.try_get("residual_value")?;
    Ok(EquipmentEconomics {
        branch_id: BranchId::from_uuid(row.try_get("branch_id")?),
        // Kept OPTIONAL on purpose: a bare acquisition-only asset (vehicle_value
        // NULL) must still resolve its branch for the lifecycle-cost read. Only
        // the depreciation/quote write paths demand it (require_vehicle_value).
        vehicle_value_won,
        residual_value_won: residual_value_won.unwrap_or(0),
        asset_registered_on: row.try_get("asset_registered_on")?,
    })
}

async fn cumulative_cost_tx(
    tx: &mut Transaction<'_, Postgres>,
    equipment_id: EquipmentId,
    excluding_purchase: Option<PurchaseRequestId>,
) -> Result<i64, PgFinancialError> {
    let total: Option<i64> = sqlx::query_scalar(
        r#"
        SELECT COALESCE(SUM(amount_won), 0)::BIGINT
        FROM equipment_cost_ledger
        WHERE equipment_id = $1
          AND ($2::UUID IS NULL OR purchase_request_id IS DISTINCT FROM $2)
        "#,
    )
    .bind(*equipment_id.as_uuid())
    .bind(excluding_purchase.map(|id| *id.as_uuid()))
    .fetch_one(tx.as_mut())
    .await?;
    Ok(total.unwrap_or(0))
}

async fn append_cost_ledger_entry_tx(
    tx: &mut Transaction<'_, Postgres>,
    command: AppendCostLedgerEntryCommand,
    purchase_request_id: Option<PurchaseRequestId>,
    org_uuid: uuid::Uuid,
) -> Result<(CostLedgerEntrySummary, AuditEvent), PgFinancialError> {
    // Freeze-window gate: EVERY accounting ledger write (manual admin entry and
    // purchase execution both funnel through here) must land outside a locked
    // accounting period. Fails closed with a 409-mapping conflict.
    console_platform_db::assert_period_open(
        tx,
        console_platform_db::PeriodLockDomain::Accounting,
        command.occurred_at.date(),
    )
    .await
    .map_err(PgFinancialError::Domain)?;
    let locked = equipment_economics_tx(tx, command.equipment_id).await?;
    ensure_branch(locked.branch_id, command.branch_id)?;
    if let Some(work_order_id) = command.work_order_id {
        ensure_work_order_matches_tx(tx, work_order_id, command.branch_id, command.equipment_id)
            .await?;
    }

    let vehicle_value_won = locked.require_vehicle_value()?;
    let previous_cost = cumulative_cost_tx(tx, command.equipment_id, None).await?;
    let cumulative_cost = previous_cost.saturating_add(command.amount_won);
    let months_elapsed = months_elapsed(locked.asset_registered_on, command.occurred_at.date());
    let residual_after = recompute_residual_value(ResidualRecomputeInput {
        acquisition_value: MoneyInput::won(vehicle_value_won),
        months_elapsed,
        cumulative_cost: MoneyInput::won(cumulative_cost),
        config: command.config.quote_config(),
    })?
    .amount();

    let entry_id = uuid::Uuid::new_v4();
    sqlx::query(
        r#"
        INSERT INTO equipment_cost_ledger (
            id, branch_id, equipment_id, work_order_id, purchase_request_id,
            source, amount_won, memo, residual_before_won, residual_after_won,
            entry_at, created_by, org_id
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)
        "#,
    )
    .bind(entry_id)
    .bind(*command.branch_id.as_uuid())
    .bind(*command.equipment_id.as_uuid())
    .bind(command.work_order_id.map(|id| *id.as_uuid()))
    .bind(purchase_request_id.map(|id| *id.as_uuid()))
    .bind(command.source.as_db_str())
    .bind(command.amount_won)
    .bind(command.memo.trim())
    .bind(locked.residual_value_won)
    .bind(residual_after)
    .bind(command.occurred_at)
    .bind(*command.actor.as_uuid())
    .bind(org_uuid)
    .execute(tx.as_mut())
    .await?;

    sqlx::query("UPDATE registry_equipment SET residual_value = $2, updated_at = $3 WHERE id = $1")
        .bind(*command.equipment_id.as_uuid())
        .bind(residual_after)
        .bind(command.occurred_at)
        .execute(tx.as_mut())
        .await?;

    let before = serde_json::json!({
        "residual_value_won": locked.residual_value_won,
        "cumulative_cost_won": previous_cost,
    });
    let after = serde_json::json!({
        "residual_value_won": residual_after,
        "cumulative_cost_won": cumulative_cost,
        "months_elapsed": months_elapsed,
    });
    let event = financial_audit_event(
        "equipment.residual.recompute",
        command.actor,
        command.branch_id,
        "registry_equipment",
        command.equipment_id,
        command.trace,
        command.occurred_at,
    )?
    .with_snapshots(Some(before), Some(after))
    .with_org(OrgId::from_uuid(org_uuid));
    let entry = cost_ledger_entry_by_id_tx(tx, entry_id).await?;
    Ok((entry, event))
}

async fn ensure_work_order_matches_tx(
    tx: &mut Transaction<'_, Postgres>,
    work_order_id: WorkOrderId,
    branch_id: BranchId,
    equipment_id: EquipmentId,
) -> Result<(), PgFinancialError> {
    let row = sqlx::query(
        r#"
        SELECT branch_id, equipment_id
        FROM work_orders
        WHERE id = $1
        "#,
    )
    .bind(*work_order_id.as_uuid())
    .fetch_optional(tx.as_mut())
    .await?
    .ok_or_else(|| KernelError::not_found("work order was not found"))?;
    let actual_branch = BranchId::from_uuid(row.try_get("branch_id")?);
    let actual_equipment = EquipmentId::from_uuid(row.try_get("equipment_id")?);
    if actual_branch != branch_id || actual_equipment != equipment_id {
        return Err(
            KernelError::forbidden("work order is outside the equipment financial scope").into(),
        );
    }
    Ok(())
}

/// Scope-check the statement evidence linked to a purchase request WITHOUT
/// requiring the WORM replica to be verified.
///
/// This is the create/restart-time check. It still enforces that the evidence is
/// REQUEST-stage 거래명세표 belonging to the same branch/equipment/work-order as
/// the purchase — the financial-scope invariants — but it deliberately does NOT
/// require `worm_replica_status == 'VERIFIED'`. The WORM-replica state is set
/// asynchronously by the replication worker (`replicate_once`), with no retry
/// driver, so a still-replicating (PENDING/FAILED) replica must not permanently
/// bar a legitimate purchase request from being *created*. The durable-WORM
/// precondition is instead enforced at SUBMIT (see
/// [`ensure_statement_evidence_verified_tx`]), the first state that promotes the
/// request into the approval pipeline. No money or ledger entry moves until
/// `execute_purchase`, so a StatementAttached row referencing not-yet-verified
/// evidence carries no financial-integrity risk.
async fn ensure_statement_evidence_tx(
    tx: &mut Transaction<'_, Postgres>,
    evidence_id: console_kernel_core::EvidenceId,
    branch_id: BranchId,
    equipment_id: EquipmentId,
    expected_work_order_id: Option<WorkOrderId>,
) -> Result<StatementEvidenceLink, PgFinancialError> {
    let row = sqlx::query(
        r#"
        SELECT e.work_order_id, e.stage,
               w.branch_id, w.equipment_id
        FROM evidence_media e
        JOIN work_orders w ON w.id = e.work_order_id
        WHERE e.id = $1
        "#,
    )
    .bind(*evidence_id.as_uuid())
    .fetch_optional(tx.as_mut())
    .await?
    .ok_or_else(|| KernelError::not_found("statement evidence was not found"))?;

    let work_order_id = WorkOrderId::from_uuid(row.try_get("work_order_id")?);
    let actual_branch = BranchId::from_uuid(row.try_get("branch_id")?);
    let actual_equipment = EquipmentId::from_uuid(row.try_get("equipment_id")?);
    if actual_branch != branch_id || actual_equipment != equipment_id {
        return Err(KernelError::forbidden(
            "statement evidence is outside the equipment financial scope",
        )
        .into());
    }
    if expected_work_order_id.is_some_and(|expected| expected != work_order_id) {
        return Err(
            KernelError::forbidden("statement evidence belongs to a different work order").into(),
        );
    }

    let stage: String = row.try_get("stage")?;
    if stage != "REQUEST" {
        return Err(
            KernelError::validation("statement evidence must be REQUEST-stage evidence").into(),
        );
    }

    Ok(StatementEvidenceLink { work_order_id })
}

/// Assert the linked statement evidence's WORM replica is durably VERIFIED.
///
/// The deferred compliance gate enforced at SUBMIT: a purchase request may be
/// created against still-replicating evidence, but it may not enter the approval
/// pipeline until the 거래명세표 is durably preserved under COMPLIANCE retention
/// (`worm_replica_status == 'VERIFIED'`). Surfaces a clear caller-facing reason so
/// the operator learns the request is waiting on WORM verification rather than
/// seeing a silent failure.
async fn ensure_statement_evidence_verified_tx(
    tx: &mut Transaction<'_, Postgres>,
    evidence_id: console_kernel_core::EvidenceId,
) -> Result<(), PgFinancialError> {
    let status: Option<String> =
        sqlx::query_scalar("SELECT worm_replica_status FROM evidence_media WHERE id = $1")
            .bind(*evidence_id.as_uuid())
            .fetch_optional(tx.as_mut())
            .await?;
    let status =
        status.ok_or_else(|| KernelError::not_found("statement evidence was not found"))?;
    if status != "VERIFIED" {
        return Err(KernelError::validation(
            "거래명세표가 아직 보존 검증 중입니다. 잠시 후 다시 상신하세요. \
             (statement evidence is still being WORM-verified)",
        )
        .into());
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn insert_quote_tx(
    tx: &mut Transaction<'_, Postgres>,
    quote_id: QuoteId,
    actor: UserId,
    branch_id: BranchId,
    equipment_id: EquipmentId,
    acquisition_value_won: i64,
    residual_value_won: i64,
    cumulative_repair_cost_won: i64,
    config: &FinancialConfigSnapshot,
    quote: &console_financial_domain::ComputedRentalQuote,
    occurred_at: OffsetDateTime,
    org_uuid: uuid::Uuid,
) -> Result<(), PgFinancialError> {
    sqlx::query(
        r#"
        INSERT INTO financial_rental_quotes (
            id, branch_id, equipment_id, created_by,
            acquisition_value_won, current_residual_value_won,
            effective_residual_value_won, residual_was_floored,
            cumulative_repair_cost_won, depreciation_method,
            useful_life_months, residual_rate_bps, declining_balance_rate_bps,
            management_fee_rate_bps, profit_rate_bps, floor_negative_quote_residual,
            monthly_total_won, created_at, updated_at, org_id
        )
        VALUES (
            $1, $2, $3, $4,
            $5, $6, $7, $8,
            $9, $10, $11, $12, $13,
            $14, $15, $16,
            $17, $18, $18, $19
        )
        "#,
    )
    .bind(*quote_id.as_uuid())
    .bind(*branch_id.as_uuid())
    .bind(*equipment_id.as_uuid())
    .bind(*actor.as_uuid())
    .bind(acquisition_value_won)
    .bind(residual_value_won)
    .bind(quote.effective_residual_value.amount())
    .bind(quote.residual_was_floored)
    .bind(cumulative_repair_cost_won)
    .bind(config.depreciation_method.as_db_str())
    .bind(
        i32::try_from(config.useful_life_months)
            .map_err(|_| KernelError::validation("useful life months overflowed i32"))?,
    )
    .bind(config.residual_rate_bps)
    .bind(config.declining_balance_rate_bps)
    .bind(config.management_fee_rate_bps)
    .bind(config.profit_rate_bps)
    .bind(config.floor_negative_quote_residual)
    .bind(quote.monthly_total.amount())
    .bind(occurred_at)
    .bind(org_uuid)
    .execute(tx.as_mut())
    .await?;

    for (index, line) in quote.lines.iter().enumerate() {
        let line_order = i16::try_from(index + 1)
            .map_err(|_| KernelError::validation("quote line order overflowed i16"))?;
        sqlx::query(
            r#"
            INSERT INTO financial_rental_quote_lines (
                quote_id, line_order, code, label, amount_won, org_id
            )
            VALUES ($1, $2, $3, $4, $5, $6)
            "#,
        )
        .bind(*quote_id.as_uuid())
        .bind(line_order)
        .bind(&line.code)
        .bind(&line.label)
        .bind(line.amount.amount())
        .bind(org_uuid)
        .execute(tx.as_mut())
        .await?;
    }
    Ok(())
}

async fn rental_quote_by_id_tx(
    tx: &mut Transaction<'_, Postgres>,
    quote_id: QuoteId,
) -> Result<RentalQuoteSummary, PgFinancialError> {
    let row = sqlx::query(
        r#"
        SELECT id, branch_id, equipment_id, acquisition_value_won,
               current_residual_value_won, effective_residual_value_won,
               residual_was_floored, cumulative_repair_cost_won,
               monthly_total_won, created_at
        FROM financial_rental_quotes
        WHERE id = $1
        "#,
    )
    .bind(*quote_id.as_uuid())
    .fetch_one(tx.as_mut())
    .await?;
    let lines = quote_lines_tx(tx, quote_id).await?;
    rental_quote_from_row(&row, lines)
}

async fn rental_quote_by_id(
    pool: &PgPool,
    quote_id: QuoteId,
) -> Result<RentalQuoteSummary, PgFinancialError> {
    // Both reads (the quote and its lines) run in ONE tenant-scoped transaction
    // so they are consistent and both narrowed to the request's org by RLS.
    let org = current_org().map_err(KernelError::from)?;
    let (row, lines) = with_org_conn::<_, _, PgFinancialError>(pool, org, move |tx| {
        Box::pin(async move {
            let row = sqlx::query(
                r#"
        SELECT id, branch_id, equipment_id, acquisition_value_won,
               current_residual_value_won, effective_residual_value_won,
               residual_was_floored, cumulative_repair_cost_won,
               monthly_total_won, created_at
        FROM financial_rental_quotes
        WHERE id = $1
        "#,
            )
            .bind(*quote_id.as_uuid())
            .fetch_optional(tx.as_mut())
            .await?
            .ok_or_else(|| KernelError::not_found("rental quote was not found"))?;
            let lines = quote_lines_tx(tx, quote_id).await?;
            Ok((row, lines))
        })
    })
    .await?;
    rental_quote_from_row(&row, lines)
}

fn rental_quote_from_row(
    row: &sqlx::postgres::PgRow,
    lines: Vec<console_financial_domain::QuoteLine>,
) -> Result<RentalQuoteSummary, PgFinancialError> {
    Ok(RentalQuoteSummary {
        id: QuoteId::from_uuid(row.try_get("id")?),
        branch_id: BranchId::from_uuid(row.try_get("branch_id")?),
        equipment_id: EquipmentId::from_uuid(row.try_get("equipment_id")?),
        acquisition_value: MoneyInput::won(row.try_get("acquisition_value_won")?),
        current_residual_value: MoneyInput::won(row.try_get("current_residual_value_won")?),
        effective_residual_value: MoneyInput::won(row.try_get("effective_residual_value_won")?),
        residual_was_floored: row.try_get("residual_was_floored")?,
        cumulative_repair_cost: MoneyInput::won(row.try_get("cumulative_repair_cost_won")?),
        monthly_total: MoneyInput::won(row.try_get("monthly_total_won")?),
        lines,
        created_at: row.try_get("created_at")?,
    })
}

async fn quote_lines_tx(
    tx: &mut Transaction<'_, Postgres>,
    quote_id: QuoteId,
) -> Result<Vec<console_financial_domain::QuoteLine>, PgFinancialError> {
    let rows = sqlx::query(
        r#"
        SELECT code, label, amount_won
        FROM financial_rental_quote_lines
        WHERE quote_id = $1
        ORDER BY line_order
        "#,
    )
    .bind(*quote_id.as_uuid())
    .fetch_all(tx.as_mut())
    .await?;
    rows.into_iter()
        .map(|row| {
            Ok(console_financial_domain::QuoteLine {
                code: row.try_get("code")?,
                label: row.try_get("label")?,
                amount: MoneyInput::won(row.try_get("amount_won")?),
            })
        })
        .collect()
}

fn purchase_equipment_required(org_uuid: uuid::Uuid) -> bool {
    org_uuid == *OrgId::knl().as_uuid()
}

fn normalize_purchase_key(value: &str) -> String {
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

fn purchase_policy_messages(price_anomaly: bool, quote_update_required: bool) -> Vec<String> {
    let mut messages = Vec::new();
    if price_anomaly {
        messages.push("기존 정기구매 단가와 1원 이상 차이가 있습니다.".to_owned());
    }
    if quote_update_required {
        messages.push("견적서 업데이트가 필요합니다. 견적서를 첨부한 뒤 상신하세요.".to_owned());
    }
    messages
}

fn compute_purchase_lines(
    client_amount_won: Option<i64>,
    inputs: &[PurchaseRequestLineInput],
) -> Result<Vec<ComputedPurchaseLine>, PgFinancialError> {
    if inputs.is_empty() {
        let amount = client_amount_won
            .filter(|amount| *amount > 0)
            .ok_or_else(|| KernelError::validation("at least one purchase line is required"))?;
        return Ok(vec![ComputedPurchaseLine {
            line_no: 1,
            item: "LEGACY_MANUAL".to_owned(),
            quantity: 1,
            unit_supply_price_won: amount,
            vat_won: 0,
            vat_overridden: true,
            line_total_won: amount,
        }]);
    }

    let mut lines = Vec::with_capacity(inputs.len());
    for (index, input) in inputs.iter().enumerate() {
        validate_required(&input.item, "purchase line item")?;
        if input.quantity <= 0 {
            return Err(KernelError::validation("purchase line quantity must be positive").into());
        }
        if input.unit_supply_price_won < 0 {
            return Err(
                KernelError::validation("purchase line unit price must be non-negative").into(),
            );
        }
        let supply_total = input
            .unit_supply_price_won
            .checked_mul(i64::from(input.quantity))
            .ok_or_else(|| KernelError::validation("purchase line supply total overflowed"))?;
        let auto_vat = supply_total / 10;
        let (vat_won, vat_overridden) = match input.vat_won {
            Some(vat) if vat >= 0 => (vat, true),
            Some(_) => {
                return Err(
                    KernelError::validation("purchase line VAT must be non-negative").into(),
                );
            }
            None => (auto_vat, false),
        };
        let line_total_won = supply_total
            .checked_add(vat_won)
            .ok_or_else(|| KernelError::validation("purchase line total overflowed"))?;
        if line_total_won <= 0 {
            return Err(KernelError::validation("purchase line total must be positive").into());
        }
        let line_no = i32::try_from(index + 1)
            .map_err(|_| KernelError::validation("purchase line count overflowed"))?;
        lines.push(ComputedPurchaseLine {
            line_no,
            item: input.item.trim().to_owned(),
            quantity: input.quantity,
            unit_supply_price_won: input.unit_supply_price_won,
            vat_won,
            vat_overridden,
            line_total_won,
        });
    }

    let total = purchase_total(&lines)?;
    if let Some(client_total) = client_amount_won
        && client_total != total
    {
        return Err(KernelError::validation(
            "purchase amount must equal the server-calculated line total",
        )
        .into());
    }
    Ok(lines)
}

fn purchase_total(lines: &[ComputedPurchaseLine]) -> Result<i64, PgFinancialError> {
    let mut total = 0_i64;
    for line in lines {
        total = total
            .checked_add(line.line_total_won)
            .ok_or_else(|| KernelError::validation("purchase total overflowed"))?;
    }
    if total <= 0 {
        return Err(KernelError::validation("purchase amount must be positive").into());
    }
    Ok(total)
}

async fn insert_purchase_lines_tx(
    tx: &mut Transaction<'_, Postgres>,
    purchase_request_id: PurchaseRequestId,
    lines: &[ComputedPurchaseLine],
    org_uuid: uuid::Uuid,
) -> Result<(), PgFinancialError> {
    for line in lines {
        sqlx::query(
            r#"
            INSERT INTO financial_purchase_request_lines (
                purchase_request_id, line_no, item, quantity, unit_supply_price_won,
                vat_won, vat_overridden, line_total_won, org_id
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
            "#,
        )
        .bind(*purchase_request_id.as_uuid())
        .bind(line.line_no)
        .bind(&line.item)
        .bind(line.quantity)
        .bind(line.unit_supply_price_won)
        .bind(line.vat_won)
        .bind(line.vat_overridden)
        .bind(line.line_total_won)
        .bind(org_uuid)
        .execute(tx.as_mut())
        .await?;
    }
    Ok(())
}

async fn replace_purchase_lines_tx(
    tx: &mut Transaction<'_, Postgres>,
    purchase_request_id: PurchaseRequestId,
    lines: &[ComputedPurchaseLine],
    org_uuid: uuid::Uuid,
) -> Result<(), PgFinancialError> {
    sqlx::query("DELETE FROM financial_purchase_request_lines WHERE purchase_request_id = $1")
        .bind(*purchase_request_id.as_uuid())
        .execute(tx.as_mut())
        .await?;
    insert_purchase_lines_tx(tx, purchase_request_id, lines, org_uuid).await
}

async fn purchase_lines_tx(
    tx: &mut Transaction<'_, Postgres>,
    purchase_request_id: PurchaseRequestId,
) -> Result<Vec<PurchaseRequestLineSummary>, PgFinancialError> {
    let rows = sqlx::query(
        r#"
        SELECT id, line_no, item, quantity, unit_supply_price_won,
               vat_won, vat_overridden, line_total_won
        FROM financial_purchase_request_lines
        WHERE purchase_request_id = $1
        ORDER BY line_no
        "#,
    )
    .bind(*purchase_request_id.as_uuid())
    .fetch_all(tx.as_mut())
    .await?;

    if rows.is_empty() {
        let amount: i64 =
            sqlx::query_scalar("SELECT amount_won FROM financial_purchase_requests WHERE id = $1")
                .bind(*purchase_request_id.as_uuid())
                .fetch_one(tx.as_mut())
                .await?;
        return Ok(vec![PurchaseRequestLineSummary {
            id: uuid::Uuid::nil(),
            line_no: 1,
            item: "LEGACY_MANUAL".to_owned(),
            quantity: 1,
            unit_supply_price_won: amount,
            vat_won: 0,
            vat_overridden: true,
            line_total_won: amount,
        }]);
    }

    rows.into_iter()
        .map(|row| {
            Ok(PurchaseRequestLineSummary {
                id: row.try_get("id")?,
                line_no: row.try_get("line_no")?,
                item: row.try_get("item")?,
                quantity: row.try_get("quantity")?,
                unit_supply_price_won: row.try_get("unit_supply_price_won")?,
                vat_won: row.try_get("vat_won")?,
                vat_overridden: row.try_get("vat_overridden")?,
                line_total_won: row.try_get("line_total_won")?,
            })
        })
        .collect()
}

/// Batch-hydrates normalized lines for one already-authorized page.  Together
/// with the sibling attachment query this keeps a 100-row collection at four
/// database queries total (count + rows + lines + attachments).
async fn purchase_lines_for_page_tx(
    tx: &mut Transaction<'_, Postgres>,
    purchase_ids: &[uuid::Uuid],
) -> Result<HashMap<uuid::Uuid, Vec<PurchaseRequestLineSummary>>, PgFinancialError> {
    if purchase_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let rows = sqlx::query(
        r#"
        SELECT purchase_request_id, id, line_no, item, quantity, unit_supply_price_won,
               vat_won, vat_overridden, line_total_won
        FROM financial_purchase_request_lines
        WHERE purchase_request_id = ANY($1)
        ORDER BY purchase_request_id, line_no
        "#,
    )
    .bind(purchase_ids)
    .fetch_all(tx.as_mut())
    .await?;
    let mut by_purchase = HashMap::<uuid::Uuid, Vec<PurchaseRequestLineSummary>>::new();
    for row in rows {
        let purchase_id: uuid::Uuid = row.try_get("purchase_request_id")?;
        by_purchase
            .entry(purchase_id)
            .or_default()
            .push(PurchaseRequestLineSummary {
                id: row.try_get("id")?,
                line_no: row.try_get("line_no")?,
                item: row.try_get("item")?,
                quantity: row.try_get("quantity")?,
                unit_supply_price_won: row.try_get("unit_supply_price_won")?,
                vat_won: row.try_get("vat_won")?,
                vat_overridden: row.try_get("vat_overridden")?,
                line_total_won: row.try_get("line_total_won")?,
            });
    }
    Ok(by_purchase)
}

async fn purchase_attachments_tx(
    tx: &mut Transaction<'_, Postgres>,
    purchase_request_id: PurchaseRequestId,
) -> Result<Vec<PurchaseAttachmentSummary>, PgFinancialError> {
    let rows = sqlx::query(
        r#"
        SELECT id, file_name, content_type, size_bytes, role, created_at
        FROM financial_purchase_attachments
        WHERE purchase_request_id = $1
          AND upload_state = 'CONFIRMED'
        ORDER BY created_at DESC, id DESC
        "#,
    )
    .bind(*purchase_request_id.as_uuid())
    .fetch_all(tx.as_mut())
    .await?;

    rows.into_iter()
        .map(|row| {
            let id: uuid::Uuid = row.try_get("id")?;
            Ok(PurchaseAttachmentSummary {
                id,
                file_name: row.try_get("file_name")?,
                content_type: row.try_get("content_type")?,
                size_bytes: row.try_get("size_bytes")?,
                role: row.try_get("role")?,
                download_url: format!(
                    "/api/v1/financial/purchase-requests/{}/attachments/{}/download",
                    purchase_request_id, id
                ),
                created_at: row.try_get("created_at")?,
            })
        })
        .collect()
}

async fn purchase_attachments_for_page_tx(
    tx: &mut Transaction<'_, Postgres>,
    purchase_ids: &[uuid::Uuid],
) -> Result<HashMap<uuid::Uuid, Vec<PurchaseAttachmentSummary>>, PgFinancialError> {
    if purchase_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let rows = sqlx::query(
        r#"
        SELECT purchase_request_id, id, file_name, content_type, size_bytes, role, created_at
        FROM financial_purchase_attachments
        WHERE purchase_request_id = ANY($1)
          AND upload_state = 'CONFIRMED'
        ORDER BY purchase_request_id, created_at DESC, id DESC
        "#,
    )
    .bind(purchase_ids)
    .fetch_all(tx.as_mut())
    .await?;
    let mut by_purchase = HashMap::<uuid::Uuid, Vec<PurchaseAttachmentSummary>>::new();
    for row in rows {
        let purchase_id: uuid::Uuid = row.try_get("purchase_request_id")?;
        let id: uuid::Uuid = row.try_get("id")?;
        by_purchase
            .entry(purchase_id)
            .or_default()
            .push(PurchaseAttachmentSummary {
                id,
                file_name: row.try_get("file_name")?,
                content_type: row.try_get("content_type")?,
                size_bytes: row.try_get("size_bytes")?,
                role: row.try_get("role")?,
                download_url: format!(
                    "/api/v1/financial/purchase-requests/{}/attachments/{}/download",
                    purchase_id, id
                ),
                created_at: row.try_get("created_at")?,
            });
    }
    Ok(by_purchase)
}

async fn attach_purchase_attachments_tx(
    tx: &mut Transaction<'_, Postgres>,
    purchase_request_id: PurchaseRequestId,
    branch_id: BranchId,
    attachment_ids: &[uuid::Uuid],
) -> Result<(), PgFinancialError> {
    for attachment_id in attachment_ids {
        let result = sqlx::query(
            r#"
            UPDATE financial_purchase_attachments
            SET purchase_request_id = $1
            WHERE id = $2
              AND branch_id = $3
              AND role = 'QUOTE'
              AND upload_state = 'CONFIRMED'
              AND (purchase_request_id IS NULL OR purchase_request_id = $1)
            "#,
        )
        .bind(*purchase_request_id.as_uuid())
        .bind(*attachment_id)
        .bind(*branch_id.as_uuid())
        .execute(tx.as_mut())
        .await?;
        if result.rows_affected() != 1 {
            return Err(KernelError::validation(
                "quote attachment is not available for this purchase request",
            )
            .into());
        }
    }
    Ok(())
}

async fn purchase_attachment_upload_record_conn(
    conn: &mut PgConnection,
    attachment_id: uuid::Uuid,
) -> Result<PurchaseAttachmentUploadRecord, PgFinancialError> {
    let row = sqlx::query(
        r#"
        SELECT id, branch_id, file_name, content_type, size_bytes, role,
               upload_state, created_at
        FROM financial_purchase_attachments
        WHERE id = $1
        "#,
    )
    .bind(attachment_id)
    .fetch_optional(conn)
    .await?
    .ok_or_else(|| KernelError::not_found("purchase attachment not found"))?;

    Ok(PurchaseAttachmentUploadRecord {
        id: row.try_get("id")?,
        branch_id: BranchId::from_uuid(row.try_get("branch_id")?),
        file_name: row.try_get("file_name")?,
        content_type: row.try_get("content_type")?,
        size_bytes: row.try_get("size_bytes")?,
        role: row.try_get("role")?,
        upload_state: row.try_get("upload_state")?,
        created_at: row.try_get("created_at")?,
    })
}

async fn purchase_attachment_upload_record_tx(
    tx: &mut Transaction<'_, Postgres>,
    attachment_id: uuid::Uuid,
) -> Result<PurchaseAttachmentUploadRecord, PgFinancialError> {
    purchase_attachment_upload_record_conn(tx.as_mut(), attachment_id).await
}
async fn purchase_policy_flags_tx(
    tx: &mut Transaction<'_, Postgres>,
    branch_id: BranchId,
    purchase_type: PurchaseType,
    vendor_name: &str,
    lines: &[ComputedPurchaseLine],
    has_quote_attachment: bool,
) -> Result<PurchasePolicyFlags, PgFinancialError> {
    let mut price_anomaly = false;

    if purchase_type == PurchaseType::Regular {
        let vendor = normalize_purchase_key(vendor_name);
        for line in lines {
            let item = normalize_purchase_key(&line.item);
            let previous: Option<i64> = sqlx::query_scalar(
                r#"
                SELECT last_unit_supply_price_won
                FROM financial_regular_purchase_prices
                WHERE branch_id = $1
                  AND vendor_name_norm = $2
                  AND item_norm = $3
                "#,
            )
            .bind(*branch_id.as_uuid())
            .bind(&vendor)
            .bind(&item)
            .fetch_optional(tx.as_mut())
            .await?;
            if previous.is_some_and(|stored| stored != line.unit_supply_price_won) {
                price_anomaly = true;
            }
        }
    }

    let quote_update_required = price_anomaly && !has_quote_attachment;
    Ok(PurchasePolicyFlags {
        price_anomaly,
        quote_update_required,
    })
}

struct RegularPurchasePriceUpsert<'a> {
    purchase_request_id: PurchaseRequestId,
    branch_id: BranchId,
    vendor_name: &'a str,
    lines: &'a [ComputedPurchaseLine],
    quote_attachment_id: Option<uuid::Uuid>,
    updated_at: OffsetDateTime,
    org_uuid: uuid::Uuid,
}

async fn upsert_regular_purchase_prices_tx(
    tx: &mut Transaction<'_, Postgres>,
    input: RegularPurchasePriceUpsert<'_>,
) -> Result<(), PgFinancialError> {
    let vendor = normalize_purchase_key(input.vendor_name);
    for line in input.lines {
        let item = normalize_purchase_key(&line.item);
        sqlx::query(
            r#"
            INSERT INTO financial_regular_purchase_prices (
                branch_id, vendor_name_norm, item_norm, last_unit_supply_price_won,
                quote_attachment_id, updated_from_purchase_request_id, updated_at, org_id
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
            ON CONFLICT (org_id, branch_id, vendor_name_norm, item_norm)
            DO UPDATE SET
                last_unit_supply_price_won = EXCLUDED.last_unit_supply_price_won,
                quote_attachment_id = EXCLUDED.quote_attachment_id,
                updated_from_purchase_request_id = EXCLUDED.updated_from_purchase_request_id,
                updated_at = EXCLUDED.updated_at
            "#,
        )
        .bind(*input.branch_id.as_uuid())
        .bind(&vendor)
        .bind(&item)
        .bind(line.unit_supply_price_won)
        .bind(input.quote_attachment_id)
        .bind(*input.purchase_request_id.as_uuid())
        .bind(input.updated_at)
        .bind(input.org_uuid)
        .execute(tx.as_mut())
        .await?;
    }
    Ok(())
}

struct ExpenseLedgerInsert<'a> {
    purchase_request_id: PurchaseRequestId,
    actor: UserId,
    branch_id: BranchId,
    vendor_name: &'a str,
    amount_won: i64,
    expenditure_no: Option<&'a str>,
    occurred_at: OffsetDateTime,
    org_uuid: uuid::Uuid,
}

async fn insert_expense_ledger_tx(
    tx: &mut Transaction<'_, Postgres>,
    input: ExpenseLedgerInsert<'_>,
) -> Result<AuditEvent, PgFinancialError> {
    sqlx::query(
        r#"
        INSERT INTO financial_expense_ledger (
            branch_id, purchase_request_id, vendor_name, amount_won, memo,
            expenditure_no, executed_by, executed_at, org_id
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
        "#,
    )
    .bind(*input.branch_id.as_uuid())
    .bind(*input.purchase_request_id.as_uuid())
    .bind(input.vendor_name.trim())
    .bind(input.amount_won)
    .bind(format!(
        "purchase expense execution {}",
        input.purchase_request_id
    ))
    .bind(input.expenditure_no)
    .bind(*input.actor.as_uuid())
    .bind(input.occurred_at)
    .bind(input.org_uuid)
    .execute(tx.as_mut())
    .await?;

    financial_audit_event(
        "financial.expense.execute",
        input.actor,
        input.branch_id,
        "financial_expense_ledger",
        input.purchase_request_id,
        TraceContext::generate(),
        input.occurred_at,
    )
    .map(|event| event.with_org(OrgId::from_uuid(input.org_uuid)))
    .map_err(PgFinancialError::from)
}

async fn ensure_branch_exists_tx(
    tx: &mut Transaction<'_, Postgres>,
    branch_id: BranchId,
) -> Result<(), PgFinancialError> {
    let exists: Option<uuid::Uuid> = sqlx::query_scalar("SELECT id FROM branches WHERE id = $1")
        .bind(*branch_id.as_uuid())
        .fetch_optional(tx.as_mut())
        .await?;
    if exists.is_none() {
        return Err(KernelError::not_found("branch was not found").into());
    }
    Ok(())
}

async fn lock_purchase_tx(
    tx: &mut Transaction<'_, Postgres>,
    purchase_request_id: PurchaseRequestId,
) -> Result<LockedPurchase, PgFinancialError> {
    let row = sqlx::query(
        r#"
        SELECT requested_by, submitted_by, admin_approved_by, branch_id, equipment_id, work_order_id, statement_evidence_id,
               status, amount_won, vendor_name, expenditure_no,
               executive_threshold_won, depreciation_method, useful_life_months,
               residual_rate_bps, declining_balance_rate_bps,
               management_fee_rate_bps, profit_rate_bps,
               floor_negative_quote_residual, quote_update_required
        FROM financial_purchase_requests
        WHERE id = $1
        FOR UPDATE
        "#,
    )
    .bind(*purchase_request_id.as_uuid())
    .fetch_optional(tx.as_mut())
    .await?
    .ok_or_else(|| KernelError::not_found("purchase request was not found"))?;
    let status: String = row.try_get("status")?;
    let method: String = row.try_get("depreciation_method")?;
    let equipment_id: Option<uuid::Uuid> = row.try_get("equipment_id")?;
    let work_order_id: Option<uuid::Uuid> = row.try_get("work_order_id")?;
    let statement_evidence_id: Option<uuid::Uuid> = row.try_get("statement_evidence_id")?;
    let useful_life_months: i32 = row.try_get("useful_life_months")?;
    let executive_threshold_won = row.try_get("executive_threshold_won")?;
    Ok(LockedPurchase {
        requested_by: row.try_get("requested_by")?,
        submitted_by: row.try_get("submitted_by")?,
        admin_approved_by: row.try_get("admin_approved_by")?,
        branch_id: BranchId::from_uuid(row.try_get("branch_id")?),
        equipment_id: equipment_id.map(EquipmentId::from_uuid),
        work_order_id: work_order_id.map(WorkOrderId::from_uuid),
        statement_evidence_id: statement_evidence_id
            .map(console_kernel_core::EvidenceId::from_uuid),
        status: PurchaseStatus::from_db_str(&status)?,
        amount_won: row.try_get("amount_won")?,
        executive_threshold_won,
        config: FinancialConfigSnapshot {
            depreciation_method: console_financial_domain::DepreciationMethod::from_db_str(
                &method,
            )?,
            useful_life_months: u32::try_from(useful_life_months)
                .map_err(|_| KernelError::validation("stored useful life months is negative"))?,
            residual_rate_bps: row.try_get("residual_rate_bps")?,
            declining_balance_rate_bps: row.try_get("declining_balance_rate_bps")?,
            management_fee_rate_bps: row.try_get("management_fee_rate_bps")?,
            profit_rate_bps: row.try_get("profit_rate_bps")?,
            floor_negative_quote_residual: row.try_get("floor_negative_quote_residual")?,
            executive_approval_threshold_won: executive_threshold_won,
        },
        quote_update_required: row.try_get("quote_update_required")?,
        vendor_name: row.try_get("vendor_name")?,
        expenditure_no: row.try_get("expenditure_no")?,
    })
}

async fn purchase_by_id(
    pool: &PgPool,
    purchase_request_id: PurchaseRequestId,
) -> Result<PurchaseRequestSummary, PgFinancialError> {
    let org = current_org().map_err(KernelError::from)?;
    with_org_conn::<_, _, PgFinancialError>(pool, org, move |tx| {
        Box::pin(async move {
            let row = sqlx::query(purchase_select_sql())
                .bind(*purchase_request_id.as_uuid())
                .fetch_optional(tx.as_mut())
                .await?
                .ok_or_else(|| KernelError::not_found("purchase request was not found"))?;
            purchase_from_row_tx(tx, &row).await
        })
    })
    .await
}

async fn purchase_by_id_tx(
    tx: &mut Transaction<'_, Postgres>,
    purchase_request_id: PurchaseRequestId,
) -> Result<PurchaseRequestSummary, PgFinancialError> {
    let row = sqlx::query(purchase_select_sql())
        .bind(*purchase_request_id.as_uuid())
        .fetch_one(tx.as_mut())
        .await?;
    purchase_from_row_tx(tx, &row).await
}

async fn list_purchase_requests_tx(
    tx: &mut Transaction<'_, Postgres>,
    query: ListPurchaseRequestsQuery,
) -> Result<PurchaseRequestPage, PgFinancialError> {
    let statuses: Vec<&str> = query
        .statuses
        .iter()
        .map(|status| status.as_db_str())
        .collect();
    let filter_status = !statuses.is_empty();
    let branch_id = *query.branch_id.as_uuid();
    let requester_id = query.requester_id.map(|id| *id.as_uuid());

    let total: i64 = sqlx::query_scalar(
        r#"
        SELECT COUNT(*)
        FROM financial_purchase_requests p
        WHERE p.branch_id = $1
          AND (NOT $2 OR p.status = ANY($3))
          AND ($4::uuid IS NULL OR p.requested_by = $4)
        "#,
    )
    .bind(branch_id)
    .bind(filter_status)
    .bind(&statuses)
    .bind(requester_id)
    .fetch_one(tx.as_mut())
    .await?;

    // A page is hydrated in four bounded queries: count, purchase rows, lines,
    // and confirmed attachments.  Do not regress this into per-row detail
    // reads: a full 100-row page used to issue roughly 302 SQL queries.
    let rows = sqlx::query(
        r#"
        SELECT p.id, p.branch_id, p.equipment_id, p.work_order_id, p.statement_evidence_id,
               p.purchase_type, p.vendor_name, p.amount_won, p.status,
               p.requested_by, u.display_name AS requester_display_name,
               p.price_anomaly, p.quote_update_required,
               p.expenditure_no, p.rejection_memo, p.created_at, p.updated_at
        FROM financial_purchase_requests p
        JOIN users u ON u.id = p.requested_by
        WHERE p.branch_id = $1
          AND (NOT $2 OR p.status = ANY($3))
          AND ($4::uuid IS NULL OR p.requested_by = $4)
        ORDER BY p.updated_at DESC, p.id DESC
        LIMIT $5 OFFSET $6
        "#,
    )
    .bind(branch_id)
    .bind(filter_status)
    .bind(&statuses)
    .bind(requester_id)
    .bind(query.limit)
    .bind(query.offset)
    .fetch_all(tx.as_mut())
    .await?;

    let purchase_ids = rows
        .iter()
        .map(|row| row.try_get("id"))
        .collect::<Result<Vec<uuid::Uuid>, sqlx::Error>>()?;
    let lines_by_purchase = purchase_lines_for_page_tx(tx, &purchase_ids).await?;
    let attachments_by_purchase = purchase_attachments_for_page_tx(tx, &purchase_ids).await?;
    let org = current_org().map_err(KernelError::from)?;
    let equipment_required = purchase_equipment_required(*org.as_uuid());
    let mut items = Vec::with_capacity(rows.len());
    for row in &rows {
        let purchase_id: uuid::Uuid = row.try_get("id")?;
        let lines = match lines_by_purchase.get(&purchase_id) {
            Some(lines) => lines.clone(),
            None => vec![legacy_purchase_line_from_row(row)?],
        };
        let attachments = attachments_by_purchase
            .get(&purchase_id)
            .cloned()
            .unwrap_or_default();
        items.push(purchase_from_parts(
            row,
            lines,
            attachments,
            equipment_required,
        )?);
    }
    Ok(PurchaseRequestPage {
        items,
        limit: query.limit,
        offset: query.offset,
        total,
    })
}

fn purchase_select_sql() -> &'static str {
    r#"
    SELECT p.id, p.branch_id, p.equipment_id, p.work_order_id, p.statement_evidence_id,
           p.purchase_type, p.vendor_name, p.amount_won, p.status,
           p.requested_by, u.display_name AS requester_display_name,
           p.price_anomaly, p.quote_update_required,
           p.expenditure_no, p.rejection_memo, p.created_at, p.updated_at
    FROM financial_purchase_requests p
    JOIN users u ON u.id = p.requested_by
    WHERE p.id = $1
    "#
}

async fn purchase_from_row_tx(
    tx: &mut Transaction<'_, Postgres>,
    row: &sqlx::postgres::PgRow,
) -> Result<PurchaseRequestSummary, PgFinancialError> {
    let purchase_id = PurchaseRequestId::from_uuid(row.try_get("id")?);
    let lines = purchase_lines_tx(tx, purchase_id).await?;
    let attachments = purchase_attachments_tx(tx, purchase_id).await?;
    let org = current_org().map_err(KernelError::from)?;
    let equipment_required = purchase_equipment_required(*org.as_uuid());
    purchase_from_parts(row, lines, attachments, equipment_required)
}

fn purchase_from_parts(
    row: &sqlx::postgres::PgRow,
    lines: Vec<PurchaseRequestLineSummary>,
    attachments: Vec<PurchaseAttachmentSummary>,
    equipment_required: bool,
) -> Result<PurchaseRequestSummary, PgFinancialError> {
    let status: String = row.try_get("status")?;
    let purchase_type_raw: String = row.try_get("purchase_type")?;
    let equipment_id: Option<uuid::Uuid> = row.try_get("equipment_id")?;
    let work_order_id: Option<uuid::Uuid> = row.try_get("work_order_id")?;
    let statement_evidence_id: Option<uuid::Uuid> = row.try_get("statement_evidence_id")?;
    let purchase_id = PurchaseRequestId::from_uuid(row.try_get("id")?);
    let price_anomaly: bool = row.try_get("price_anomaly")?;
    let quote_update_required: bool = row.try_get("quote_update_required")?;
    let policy = PurchasePolicySummary {
        equipment_required,
        statement_evidence_required: equipment_id.is_some(),
        price_anomaly,
        quote_update_required,
        submit_blocked: quote_update_required,
        messages: purchase_policy_messages(price_anomaly, quote_update_required),
    };
    Ok(PurchaseRequestSummary {
        id: purchase_id,
        branch_id: BranchId::from_uuid(row.try_get("branch_id")?),
        equipment_id: equipment_id.map(EquipmentId::from_uuid),
        work_order_id: work_order_id.map(WorkOrderId::from_uuid),
        statement_evidence_id: statement_evidence_id
            .map(console_kernel_core::EvidenceId::from_uuid),
        purchase_type: PurchaseType::from_db_str(&purchase_type_raw)?,
        vendor_name: row.try_get("vendor_name")?,
        amount_won: row.try_get("amount_won")?,
        status: PurchaseStatus::from_db_str(&status)?,
        requester: PurchaseRequesterSummary {
            user_id: UserId::from_uuid(row.try_get("requested_by")?),
            display_name: row.try_get("requester_display_name")?,
        },
        lines,
        quote_attachments: attachments,
        policy,
        expenditure_no: row.try_get("expenditure_no")?,
        rejection_memo: row.try_get("rejection_memo")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

fn legacy_purchase_line_from_row(
    row: &sqlx::postgres::PgRow,
) -> Result<PurchaseRequestLineSummary, PgFinancialError> {
    Ok(PurchaseRequestLineSummary {
        id: uuid::Uuid::nil(),
        line_no: 1,
        item: "LEGACY_MANUAL".to_owned(),
        quantity: 1,
        unit_supply_price_won: row.try_get("amount_won")?,
        vat_won: 0,
        vat_overridden: true,
        line_total_won: row.try_get("amount_won")?,
    })
}

#[allow(clippy::too_many_arguments)]
async fn insert_purchase_history_tx(
    tx: &mut Transaction<'_, Postgres>,
    purchase_request_id: PurchaseRequestId,
    actor: UserId,
    action: &str,
    from_status: Option<PurchaseStatus>,
    to_status: PurchaseStatus,
    memo: Option<&str>,
    occurred_at: OffsetDateTime,
    org_uuid: uuid::Uuid,
) -> Result<(), PgFinancialError> {
    sqlx::query(
        r#"
        INSERT INTO financial_purchase_history (
            purchase_request_id, actor, action, from_status, to_status, memo, occurred_at, org_id
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
        "#,
    )
    .bind(*purchase_request_id.as_uuid())
    .bind(*actor.as_uuid())
    .bind(action)
    .bind(from_status.map(PurchaseStatus::as_db_str))
    .bind(to_status.as_db_str())
    .bind(memo)
    .bind(occurred_at)
    .bind(org_uuid)
    .execute(tx.as_mut())
    .await?;
    Ok(())
}

async fn cost_ledger_entry_by_id_tx(
    tx: &mut Transaction<'_, Postgres>,
    entry_id: uuid::Uuid,
) -> Result<CostLedgerEntrySummary, PgFinancialError> {
    let row = sqlx::query(
        r#"
        SELECT id, branch_id, equipment_id, work_order_id, purchase_request_id,
               source, amount_won, memo, residual_before_won, residual_after_won,
               entry_at
        FROM equipment_cost_ledger
        WHERE id = $1
        "#,
    )
    .bind(entry_id)
    .fetch_one(tx.as_mut())
    .await?;
    cost_ledger_from_row(&row)
}

async fn cost_ledger_for_equipment(
    pool: &PgPool,
    equipment_id: EquipmentId,
) -> Result<Vec<CostLedgerEntrySummary>, PgFinancialError> {
    let org = current_org().map_err(KernelError::from)?;
    let rows = with_org_conn::<_, _, PgFinancialError>(pool, org, move |tx| {
        Box::pin(async move {
            Ok(sqlx::query(
                r#"
        SELECT id, branch_id, equipment_id, work_order_id, purchase_request_id,
               source, amount_won, memo, residual_before_won, residual_after_won,
               entry_at
        FROM equipment_cost_ledger
        WHERE equipment_id = $1
        ORDER BY entry_at DESC, id DESC
        "#,
            )
            .bind(*equipment_id.as_uuid())
            .fetch_all(tx.as_mut())
            .await?)
        })
    })
    .await?;
    rows.into_iter()
        .map(|row| cost_ledger_from_row(&row))
        .collect()
}

fn cost_ledger_from_row(
    row: &sqlx::postgres::PgRow,
) -> Result<CostLedgerEntrySummary, PgFinancialError> {
    let source: String = row.try_get("source")?;
    let work_order_id: Option<uuid::Uuid> = row.try_get("work_order_id")?;
    let purchase_request_id: Option<uuid::Uuid> = row.try_get("purchase_request_id")?;
    Ok(CostLedgerEntrySummary {
        id: row.try_get("id")?,
        branch_id: BranchId::from_uuid(row.try_get("branch_id")?),
        equipment_id: EquipmentId::from_uuid(row.try_get("equipment_id")?),
        work_order_id: work_order_id.map(WorkOrderId::from_uuid),
        purchase_request_id: purchase_request_id.map(PurchaseRequestId::from_uuid),
        source: CostLedgerSource::from_db_str(&source)?,
        amount_won: row.try_get("amount_won")?,
        memo: row.try_get("memo")?,
        residual_before_won: row.try_get("residual_before_won")?,
        residual_after_won: row.try_get("residual_after_won")?,
        entry_at: row.try_get("entry_at")?,
    })
}

/// Per-asset lifecycle / TCO rollup.
///
/// Every SELECT runs inside ONE `with_org_conn` closure so `app.current_org` is
/// armed once and the `org_isolation` RLS policy narrows every tenant table in
/// the same transaction — no cross-tenant leak. `current_org()` fails closed
/// (unset GUC -> `MissingOrg`), and the bare pool is never used as an executor
/// (only `tx.as_mut()`), so the rls-arming gate is satisfied.
///
/// Tenancy notes per table: `registry_equipment`, `equipment_cost_ledger`, and
/// `sales_listings` are RLS-FORCED on `org_id` and filter themselves. The
/// `outsource_works` table has NO `org_id`/RLS of its own; it is reached only
/// through `work_orders` (RLS-FORCED), so joining outsource -> work_orders ->
/// this asset keeps the outsource read tenant-scoped. Outsource cost is surfaced
/// read-only and is NEVER summed into `tco_won` (double-count guard).
async fn lifecycle_cost_for_equipment(
    pool: &PgPool,
    equipment_id: EquipmentId,
) -> Result<AssetLifecycleCostSummary, PgFinancialError> {
    let org = current_org().map_err(KernelError::from)?;
    let equipment_uuid = *equipment_id.as_uuid();
    let (master, source_totals, outsource, sale, timeline) =
        with_org_conn::<_, _, PgFinancialError>(pool, org, move |tx| {
            Box::pin(async move {
                // Equipment master: acquisition fact + depreciation base + hours
                // + status + equipment_no. RLS limits this to the armed tenant.
                let master = sqlx::query(
                    r#"
        SELECT equipment_no, status, acquisition_cost_won, acquisition_date,
               vehicle_value, residual_value, hours
        FROM registry_equipment
        WHERE id = $1
        "#,
                )
                .bind(equipment_uuid)
                .fetch_optional(tx.as_mut())
                .await?;

                // Σ maintenance split by source, plus a single total + entry
                // count, off the per-asset cost spine.
                let source_totals = sqlx::query(
                    r#"
        SELECT source, COALESCE(SUM(amount_won), 0)::BIGINT AS total_won, COUNT(*)::BIGINT AS entry_count
        FROM equipment_cost_ledger
        WHERE equipment_id = $1
        GROUP BY source
        "#,
                )
                .bind(equipment_uuid)
                .fetch_all(tx.as_mut())
                .await?;

                // Outsource cost is reached ONLY through work_orders (which is
                // RLS-FORCED), so the join inherits tenant isolation even though
                // outsource_works itself has no org_id.
                let outsource: Option<i64> = sqlx::query_scalar(
                    r#"
        SELECT COALESCE(SUM(ow.cost_won), 0)::BIGINT
        FROM outsource_works ow
        JOIN work_orders wo ON wo.id = ow.work_order_id
        WHERE wo.equipment_id = $1
          AND ow.cost_won IS NOT NULL
        "#,
                )
                .bind(equipment_uuid)
                .fetch_one(tx.as_mut())
                .await?;

                // Latest realized sale price for a SOLD listing on this asset.
                let sale = sqlx::query(
                    r#"
        SELECT price_won, updated_at
        FROM sales_listings
        WHERE equipment_id = $1 AND status = 'SOLD'
        ORDER BY updated_at DESC, id DESC
        LIMIT 1
        "#,
                )
                .bind(equipment_uuid)
                .fetch_optional(tx.as_mut())
                .await?;

                let timeline = sqlx::query(
                    r#"
        SELECT id, branch_id, equipment_id, work_order_id, purchase_request_id,
               source, amount_won, memo, residual_before_won, residual_after_won,
               entry_at
        FROM equipment_cost_ledger
        WHERE equipment_id = $1
        ORDER BY entry_at DESC, id DESC
        "#,
                )
                .bind(equipment_uuid)
                .fetch_all(tx.as_mut())
                .await?;

                Ok((master, source_totals, outsource, sale, timeline))
            })
        })
        .await?;

    let master = master.ok_or_else(|| KernelError::not_found("equipment was not found"))?;
    let equipment_no: String = master.try_get("equipment_no")?;
    let status: String = master.try_get("status")?;
    let acquisition_cost_won: Option<i64> = master.try_get("acquisition_cost_won")?;
    let acquisition_date: Option<Date> = master.try_get("acquisition_date")?;
    let vehicle_value_won: Option<i64> = master.try_get("vehicle_value")?;
    let residual_value_won: Option<i64> = master.try_get("residual_value")?;
    let hours: Option<i64> = master.try_get("hours")?;

    // Split Σ maintenance by source. Slice 0 surfaces only the existing sources;
    // `maintenance_total_won` is the sum across ALL ledger rows so a future
    // source still rolls into the total without code changes here.
    let mut manual_total_won = 0_i64;
    let mut purchase_total_won = 0_i64;
    let mut maintenance_total_won = 0_i64;
    let mut entry_count = 0_i64;
    for row in &source_totals {
        let source: String = row.try_get("source")?;
        let total_won: i64 = row.try_get("total_won")?;
        let count: i64 = row.try_get("entry_count")?;
        maintenance_total_won = maintenance_total_won.saturating_add(total_won);
        entry_count = entry_count.saturating_add(count);
        match CostLedgerSource::from_db_str(&source)? {
            CostLedgerSource::ManualAdmin => manual_total_won = total_won,
            CostLedgerSource::PurchaseExecution => purchase_total_won = total_won,
        }
    }

    let outsource_unlinked_won = match outsource {
        Some(value) if value > 0 => Some(value),
        _ => None,
    };

    let (sale_price_won, sold_at) = match sale {
        Some(row) => {
            let price: Option<i64> = row.try_get("price_won")?;
            let updated_at: OffsetDateTime = row.try_get("updated_at")?;
            (price, Some(updated_at.date()))
        }
        None => (None, None),
    };

    let anchor = AcquisitionAnchor::resolve(acquisition_cost_won, vehicle_value_won);
    let tco_total = tco_won(anchor, maintenance_total_won);
    let gross_margin = gross_margin_won(sale_price_won, tco_total);
    let months_since_acquisition = acquisition_date.map(months_between);
    let cost_per_month = cost_per_month_won(maintenance_total_won, months_since_acquisition);
    let cost_per_hour = cost_per_hour_won(maintenance_total_won, hours);

    let timeline = timeline
        .iter()
        .map(cost_ledger_from_row)
        .collect::<Result<Vec<_>, _>>()?;

    Ok(AssetLifecycleCostSummary {
        equipment_id,
        equipment_no,
        status,
        acquisition_cost_won,
        acquisition_date,
        acquisition_source: anchor.basis,
        maintenance_total_won,
        manual_total_won,
        purchase_total_won,
        entry_count,
        outsource_unlinked_won,
        residual_value_won: residual_value_won.unwrap_or(0),
        sale_price_won,
        sold_at,
        gross_margin_won: gross_margin,
        tco_won: tco_total,
        cost_per_month_won: cost_per_month,
        cost_per_hour_won: cost_per_hour,
        timeline,
    })
}

/// Whole calendar months elapsed from `acquisition` to today (UTC), floored at
/// the day granularity. A future acquisition date yields a negative span, which
/// the per-month math treats as "unknown" (returns `None`) rather than a
/// quotient.
fn months_between(acquisition: Date) -> i64 {
    let today = OffsetDateTime::now_utc().date();
    let mut months = (i64::from(today.year()) - i64::from(acquisition.year())) * 12
        + (i64::from(u8::from(today.month())) - i64::from(u8::from(acquisition.month())));
    if today.day() < acquisition.day() {
        months -= 1;
    }
    months
}

async fn purchase_actor_for_user_tx(
    tx: &mut Transaction<'_, Postgres>,
    user_id: UserId,
) -> Result<PurchaseActor, PgFinancialError> {
    let roles: Vec<String> = sqlx::query_scalar("SELECT roles FROM users WHERE id = $1")
        .bind(*user_id.as_uuid())
        .fetch_optional(tx.as_mut())
        .await?
        .ok_or_else(|| KernelError::not_found("user was not found"))?;

    if roles.iter().any(|role| role == "SUPER_ADMIN") {
        Ok(PurchaseActor::SuperAdmin)
    } else if roles.iter().any(|role| role == "EXECUTIVE") {
        Ok(PurchaseActor::Executive)
    } else if roles.iter().any(|role| role == "ADMIN") {
        Ok(PurchaseActor::Admin)
    } else if roles.iter().any(|role| role == "RECEPTIONIST") {
        Ok(PurchaseActor::Receptionist)
    } else if roles.iter().any(|role| role == "MECHANIC") {
        Ok(PurchaseActor::Mechanic)
    } else {
        Err(KernelError::forbidden("user has no purchase workflow role").into())
    }
}

fn ensure_branch(actual: BranchId, expected: BranchId) -> Result<(), PgFinancialError> {
    if actual == expected {
        Ok(())
    } else {
        Err(KernelError::forbidden("equipment is outside branch scope").into())
    }
}

fn validate_required(value: &str, field: &str) -> Result<(), PgFinancialError> {
    if value.trim().is_empty() {
        Err(KernelError::validation(format!("{field} is required")).into())
    } else {
        Ok(())
    }
}

fn validate_feature_key(feature_key: &str) -> Result<(), PgFinancialError> {
    if feature_key == "purchase_requests" {
        Ok(())
    } else {
        Err(KernelError::validation("unsupported feature preference key").into())
    }
}

fn validate_purchase_preferences(
    schema_version: i32,
    preferences: &serde_json::Value,
) -> Result<(), PgFinancialError> {
    if schema_version != 1 {
        return Err(
            KernelError::validation("unsupported purchase preference schema version").into(),
        );
    }
    let serde_json::Value::Object(map) = preferences else {
        return Err(KernelError::validation("purchase preferences must be a JSON object").into());
    };
    let raw = serde_json::to_string(preferences)
        .map_err(|err| KernelError::validation(format!("invalid purchase preferences: {err}")))?;
    if raw.len() > 16 * 1024 {
        return Err(KernelError::validation("purchase preferences exceed 16 KiB").into());
    }
    const ALLOWED: &[&str] = &[
        "density",
        "sidebar_collapsed",
        "sidebar_width",
        "line_columns",
        "line_column_order",
        "default_purchase_type",
        "quote_panel",
        "collapsed_sections",
    ];
    for key in map.keys() {
        if !ALLOWED.iter().any(|allowed| allowed == key) {
            return Err(KernelError::validation(format!(
                "unsupported purchase preference field {key}"
            ))
            .into());
        }
    }
    Ok(())
}

fn months_elapsed(from: Option<Date>, to: Date) -> u32 {
    let Some(from) = from else {
        return 0;
    };
    let month_delta = (to.year() - from.year()) * 12
        + (i32::from(u8::from(to.month())) - i32::from(u8::from(from.month())));
    let adjusted = if to.day() < from.day() {
        month_delta - 1
    } else {
        month_delta
    };
    u32::try_from(adjusted.max(0)).unwrap_or(0)
}

/// Segregation-of-duties: self-approval guard.
///
/// Both maker facts must be known and distinct from the approver, regardless of title.
fn check_self_approval(row: &LockedPurchase, actor: UserId) -> Result<(), PgFinancialError> {
    let submitted_by = row
        .submitted_by
        .ok_or_else(|| KernelError::forbidden("상신자를 확인할 수 없어 결재할 수 없습니다"))?;
    let actor = *actor.as_uuid();
    if actor == row.requested_by || actor == submitted_by {
        return Err(KernelError::validation("본인이 상신/요청한 건은 결재할 수 없습니다").into());
    }
    Ok(())
}
