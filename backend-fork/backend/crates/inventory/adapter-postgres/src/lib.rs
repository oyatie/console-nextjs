//! Postgres adapter for the inventory IV module.
//!
//! Reads run through `with_org_conn`; mutations run through `with_audits` and
//! attach `org_id` to every emitted audit event. QueryBuilder/runtime SQL keeps
//! this crate SQLx-offline friendly: no new `.sqlx` cache entries are required.

use console_inventory_application::{
    CancelCycleCountCommand, ConsumeInventoryCommand, ConsumeInventorySource,
    CreateInventoryItemCommand, CreateStockLocationCommand, CycleCountDecision, CycleCountDetail,
    CycleCountLineView, CycleCountPage, CycleCountView, DecideCycleCountCommand,
    InventoryConsumptionEventView, InventoryConsumptionResult, InventoryItemPage,
    InventoryItemView, InventoryMovementView, InventoryReceiptResult,
    InventoryStockLocationSummary, InventoryStockLocationView, ListConsumptionEventsQuery,
    ListCycleCountsQuery, ListInventoryItemsQuery, ListMovementsQuery, MovementSourceView,
    MrpLineView, MrpQuery, OpenCycleCountCommand, RecordReceiptCommand, SubmitCycleCountCommand,
    UpdateInventoryItemCommand, UpdateInventoryItemFields, UpsertCountLineCommand,
    inventory_audit_event,
};
use console_inventory_domain::{
    CycleCountStatus, InventoryCode, InventoryConsumptionSource, InventoryItemState,
    InventoryItemStatus, MoneyWon, MovementKind, PositiveQuantityMilli, QuantityMilli,
    SafetyStockMilli, UnitCode, VarianceReason,
};
use console_kernel_core::{
    BranchId, BranchScope, ErrorKind, InventoryConsumptionEventId, InventoryItemId,
    InventoryStockLocationId, KernelError, OrgId, P1DispatchId, SiteId, UserId, WorkOrderId,
};
use console_platform_db::{DbError, with_audits, with_org_conn};
use console_platform_request_context::current_org;
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Postgres, QueryBuilder, Row, Transaction};

const DEFAULT_LIMIT: i64 = 50;
const MAX_LIMIT: i64 = 100;
const ITEM_COLUMNS: &str = "i.id, i.branch_id, i.stock_location_id, i.site_id, i.iv_code, i.sku, \
     i.display_name, i.description, i.unit_code, i.quantity_on_hand_milli, \
     i.safety_stock_milli, i.unit_cost_won, i.status, i.created_by, i.created_at, \
     i.updated_at, l.label AS stock_location_label";

#[derive(Debug, thiserror::Error)]
pub enum PgInventoryError {
    #[error(transparent)]
    Db(#[from] DbError),
    #[error(transparent)]
    Domain(#[from] KernelError),
}

impl PgInventoryError {
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

impl From<sqlx::Error> for PgInventoryError {
    fn from(value: sqlx::Error) -> Self {
        Self::Db(DbError::Sqlx(value))
    }
}

#[derive(Debug, Clone)]
pub struct PgInventoryStore {
    pool: PgPool,
}

impl PgInventoryStore {
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    #[must_use]
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    // console-gate: state-changing-handler
    pub async fn create_stock_location(
        &self,
        command: CreateStockLocationCommand,
    ) -> Result<InventoryStockLocationView, PgInventoryError> {
        ensure_branch_allowed(&command.branch_scope, command.branch_id)?;
        let label = normalize_required_text(&command.label, 120, "stock location label")?;
        let location_code = normalize_optional_text(command.location_code, 80, "location_code")?;
        let org = current_org().map_err(KernelError::from)?;
        let org_uuid = *org.as_uuid();
        let location_id = InventoryStockLocationId::new();

        with_audits::<_, InventoryStockLocationView, PgInventoryError>(&self.pool, org, |tx| {
            Box::pin(async move {
                sqlx::query(
                    r#"
                    INSERT INTO inventory_stock_locations (
                        id, org_id, branch_id, site_id, location_code, label, status, created_at, updated_at
                    ) VALUES ($1, $2, $3, $4, $5, $6, 'ACTIVE', $7, $7)
                    "#,
                )
                .bind(*location_id.as_uuid())
                .bind(org_uuid)
                .bind(*command.branch_id.as_uuid())
                .bind(command.site_id.map(|site| *site.as_uuid()))
                .bind(location_code.as_deref())
                .bind(&label)
                .bind(command.occurred_at)
                .execute(tx.as_mut())
                .await?;

                let view = fetch_location_tx(tx, location_id).await?.ok_or_else(|| {
                    KernelError::internal("created inventory stock location was not readable")
                })?;
                let event = inventory_audit_event(
                    "inventory.location.create",
                    Some(command.actor),
                    Some(command.branch_id),
                    "inventory_stock_location",
                    location_id,
                    command.trace,
                    command.occurred_at,
                )?
                .with_org(org)
                .with_snapshots(None, Some(location_snapshot(&view)));
                Ok((view, vec![event]))
            })
        })
        .await
    }

    pub async fn get_item(
        &self,
        item_id: InventoryItemId,
        branch_scope: BranchScope,
    ) -> Result<Option<InventoryItemView>, PgInventoryError> {
        let org = current_org().map_err(KernelError::from)?;
        with_org_conn::<_, _, PgInventoryError>(&self.pool, org, move |tx| {
            Box::pin(async move { fetch_item_scoped_tx(tx, item_id, &branch_scope).await })
        })
        .await
    }

    pub async fn list_items(
        &self,
        query: ListInventoryItemsQuery,
    ) -> Result<InventoryItemPage, PgInventoryError> {
        let limit = normalized_limit(query.limit);
        let offset = query.offset.unwrap_or(0).max(0);
        let query_for_count = query.clone();
        let mut count_builder = QueryBuilder::<Postgres>::new(
            "SELECT COUNT(*) FROM inventory_items i JOIN inventory_stock_locations l \
             ON l.id = i.stock_location_id AND l.org_id = i.org_id WHERE ",
        );
        push_item_filters(&mut count_builder, &query_for_count)?;

        let mut builder = QueryBuilder::<Postgres>::new("SELECT ");
        builder.push(ITEM_COLUMNS);
        builder.push(
            " FROM inventory_items i JOIN inventory_stock_locations l \
             ON l.id = i.stock_location_id AND l.org_id = i.org_id WHERE ",
        );
        push_item_filters(&mut builder, &query)?;
        builder.push(" ORDER BY i.updated_at DESC, i.id DESC LIMIT ");
        builder.push_bind(limit);
        builder.push(" OFFSET ");
        builder.push_bind(offset);

        let org = current_org().map_err(KernelError::from)?;
        let (total, rows) = with_org_conn::<_, _, PgInventoryError>(&self.pool, org, move |tx| {
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

        let mut items = Vec::with_capacity(rows.len());
        for row in &rows {
            items.push(item_from_row(row)?);
        }
        Ok(InventoryItemPage {
            items,
            limit,
            offset,
            total,
        })
    }

    // console-gate: state-changing-handler
    pub async fn create_item(
        &self,
        command: CreateInventoryItemCommand,
    ) -> Result<InventoryItemView, PgInventoryError> {
        ensure_branch_allowed(&command.branch_scope, command.branch_id)?;
        let display_name = normalize_required_text(&command.display_name, 120, "display_name")?;
        let sku = normalize_optional_text(command.sku, 80, "sku")?;
        let description = normalize_optional_text(command.description, 1_000, "description")?;
        let unit_code = UnitCode::new(command.unit_code)?;
        let quantity = QuantityMilli::new(command.quantity_on_hand_milli)?;
        let safety = SafetyStockMilli::new(command.safety_stock_milli)?;
        let unit_cost = command.unit_cost_won.map(MoneyWon::new).transpose()?;
        let org = current_org().map_err(KernelError::from)?;
        let org_uuid = *org.as_uuid();
        let item_id = InventoryItemId::new();
        let iv_code = issue_inventory_code(item_id)?;

        with_audits::<_, InventoryItemView, PgInventoryError>(&self.pool, org, |tx| {
            Box::pin(async move {
                let location = fetch_location_tx(tx, command.stock_location_id)
                    .await?
                    .ok_or_else(|| KernelError::not_found("stock location was not found"))?;
                if location.branch_id != command.branch_id {
                    return Err(KernelError::conflict(
                        "stock location branch does not match item branch",
                    )
                    .into());
                }

                sqlx::query(
                    r#"
                    INSERT INTO inventory_items (
                        id, org_id, branch_id, stock_location_id, site_id, iv_code, sku,
                        display_name, description, unit_code, quantity_on_hand_milli,
                        safety_stock_milli, unit_cost_won, status, created_by, created_at, updated_at
                    ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13,
                              'ACTIVE', $14, $15, $15)
                    "#,
                )
                .bind(*item_id.as_uuid())
                .bind(org_uuid)
                .bind(*command.branch_id.as_uuid())
                .bind(*command.stock_location_id.as_uuid())
                .bind(location.site_id.map(|site| *site.as_uuid()))
                .bind(iv_code.as_str())
                .bind(sku.as_deref())
                .bind(&display_name)
                .bind(description.as_deref())
                .bind(unit_code.as_str())
                .bind(quantity.value())
                .bind(safety.value())
                .bind(unit_cost.map(MoneyWon::value))
                .bind(*command.actor.as_uuid())
                .bind(command.occurred_at)
                .execute(tx.as_mut())
                .await?;

                let view = fetch_item_tx(tx, item_id).await?.ok_or_else(|| {
                    KernelError::internal("created inventory item was not readable")
                })?;
                let event = inventory_audit_event(
                    "inventory_item.create",
                    Some(command.actor),
                    Some(command.branch_id),
                    "inventory_item",
                    item_id,
                    command.trace,
                    command.occurred_at,
                )?
                .with_org(org)
                .with_snapshots(None, Some(item_snapshot(&view)));
                Ok((view, vec![event]))
            })
        })
        .await
    }

    // console-gate: state-changing-handler
    pub async fn update_item(
        &self,
        command: UpdateInventoryItemCommand,
    ) -> Result<InventoryItemView, PgInventoryError> {
        if command.fields.is_empty() {
            return Err(KernelError::validation("no inventory item fields to update").into());
        }
        let fields = normalize_update_fields(command.fields)?;
        let org = current_org().map_err(KernelError::from)?;

        with_audits::<_, InventoryItemView, PgInventoryError>(&self.pool, org, |tx| {
            Box::pin(async move {
                let before = fetch_item_for_update_tx(tx, command.item_id)
                    .await?
                    .ok_or_else(|| KernelError::not_found("inventory item was not found"))?;
                ensure_branch_allowed(&command.branch_scope, before.branch_id)?;

                let mut builder = QueryBuilder::<Postgres>::new("UPDATE inventory_items SET ");
                let mut sep = builder.separated(", ");
                if let Some(sku) = fields.sku.clone() {
                    sep.push("sku = ");
                    sep.push_bind_unseparated(sku);
                }
                if let Some(display_name) = fields.display_name.clone() {
                    sep.push("display_name = ");
                    sep.push_bind_unseparated(display_name);
                }
                if let Some(description) = fields.description.clone() {
                    sep.push("description = ");
                    sep.push_bind_unseparated(description);
                }
                if let Some(safety_stock_milli) = fields.safety_stock_milli {
                    sep.push("safety_stock_milli = ");
                    sep.push_bind_unseparated(safety_stock_milli);
                }
                if let Some(status) = fields.status {
                    sep.push("status = ");
                    sep.push_bind_unseparated(status.as_db_str());
                }
                sep.push("updated_at = ");
                sep.push_bind_unseparated(command.occurred_at);
                builder.push(" WHERE id = ");
                builder.push_bind(*command.item_id.as_uuid());
                builder.build().execute(tx.as_mut()).await?;

                let after = fetch_item_tx(tx, command.item_id).await?.ok_or_else(|| {
                    KernelError::internal("updated inventory item was not readable")
                })?;
                let action = if fields.status == Some(InventoryItemStatus::Archived) {
                    "inventory_item.archive"
                } else {
                    "inventory_item.update"
                };
                let event = inventory_audit_event(
                    action,
                    Some(command.actor),
                    Some(before.branch_id),
                    "inventory_item",
                    command.item_id,
                    command.trace,
                    command.occurred_at,
                )?
                .with_org(org)
                .with_snapshots(Some(item_snapshot(&before)), Some(item_snapshot(&after)));
                Ok((after, vec![event]))
            })
        })
        .await
    }

    // console-gate: state-changing-handler
    pub async fn consume_item(
        &self,
        command: ConsumeInventoryCommand,
    ) -> Result<InventoryConsumptionResult, PgInventoryError> {
        let quantity = PositiveQuantityMilli::new(command.quantity_consumed_milli)?;
        let idempotency_key = normalize_idempotency_key(&command.idempotency_key)?;
        let memo = normalize_optional_text(command.memo, 1_000, "memo")?;
        let occurred_at = command.occurred_at.unwrap_or(command.requested_at);
        let org = current_org().map_err(KernelError::from)?;
        let org_uuid = *org.as_uuid();

        with_audits::<_, InventoryConsumptionResult, PgInventoryError>(&self.pool, org, |tx| {
            Box::pin(async move {
                let source = resolve_consumption_source_tx(tx, command.source).await?;
                let fingerprint = request_fingerprint(
                    command.item_id,
                    source.source,
                    quantity,
                    memo.as_deref(),
                    command.occurred_at,
                );

                // The event uniqueness constraint is the durable backstop, but a
                // read-before-insert replay check alone leaves concurrent callers
                // racing to that constraint. Serialize one org/key pair for this
                // transaction so the follower sees the committed event and can
                // return its stored result (or a fingerprint conflict) instead.
                lock_consumption_idempotency_key_tx(tx, org, &idempotency_key).await?;

                if let Some((event, existing_fingerprint)) =
                    fetch_event_by_idempotency_key_tx(tx, &idempotency_key).await?
                {
                    if existing_fingerprint != fingerprint {
                        return Err(KernelError::conflict(
                            "idempotency key was already used with a different inventory consumption payload",
                        )
                        .into());
                    }
                    let item = fetch_item_tx(tx, command.item_id).await?.ok_or_else(|| {
                        KernelError::internal("idempotent inventory item result was not readable")
                    })?;
                    ensure_branch_allowed(&command.branch_scope, item.branch_id)?;
                    return Ok((InventoryConsumptionResult { event, item }, Vec::new()));
                }

                let before = fetch_item_for_update_tx(tx, command.item_id)
                    .await?
                    .ok_or_else(|| KernelError::not_found("inventory item was not found"))?;
                ensure_branch_allowed(&command.branch_scope, before.branch_id)?;
                if source.branch_id != before.branch_id {
                    return Err(KernelError::conflict(
                        "inventory consumption source branch must match item branch",
                    )
                    .into());
                }

                let state = item_state(&before)?;
                let outcome = state.consume(quantity)?;
                let event_id = InventoryConsumptionEventId::new();

                sqlx::query(
                    r#"
                    INSERT INTO inventory_consumption_events (
                        id, org_id, branch_id, item_id, stock_location_id, source_kind,
                        work_order_id, dispatch_id, quantity_before_milli,
                        quantity_consumed_milli, quantity_after_milli, unit_cost_won,
                        cost_won, consumed_by, occurred_at, memo, idempotency_key,
                        request_fingerprint, created_at
                    ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12,
                              $13, $14, $15, $16, $17, $18, $19)
                    "#,
                )
                .bind(*event_id.as_uuid())
                .bind(org_uuid)
                .bind(*before.branch_id.as_uuid())
                .bind(*command.item_id.as_uuid())
                .bind(*before.stock_location.id.as_uuid())
                .bind(source.source.kind_db_str())
                .bind(*source.source.work_order_id().as_uuid())
                .bind(source.source.dispatch_id().map(|id| *id.as_uuid()))
                .bind(outcome.quantity_before_milli.value())
                .bind(outcome.quantity_consumed_milli.value())
                .bind(outcome.quantity_after_milli.value())
                .bind(before.unit_cost_won)
                .bind(outcome.cost_won.map(MoneyWon::value))
                .bind(*command.actor.as_uuid())
                .bind(occurred_at)
                .bind(memo.as_deref())
                .bind(&idempotency_key)
                .bind(&fingerprint)
                .bind(command.requested_at)
                .execute(tx.as_mut())
                .await?;

                sqlx::query(
                    "UPDATE inventory_items SET quantity_on_hand_milli = $2, updated_at = $3 WHERE id = $1",
                )
                .bind(*command.item_id.as_uuid())
                .bind(outcome.quantity_after_milli.value())
                .bind(command.requested_at)
                .execute(tx.as_mut())
                .await?;

                let event = fetch_event_tx(tx, event_id).await?.ok_or_else(|| {
                    KernelError::internal("created inventory consumption event was not readable")
                })?;
                let item = fetch_item_tx(tx, command.item_id).await?.ok_or_else(|| {
                    KernelError::internal("consumed inventory item was not readable")
                })?;
                let audit = inventory_audit_event(
                    "inventory.consume",
                    Some(command.actor),
                    Some(before.branch_id),
                    "inventory_consumption_event",
                    event_id,
                    command.trace,
                    command.requested_at,
                )?
                .with_org(org)
                .with_snapshots(
                    Some(serde_json::json!({
                        "item_id": command.item_id.to_string(),
                        "iv_code": before.iv_code,
                        "quantity_on_hand_milli": outcome.quantity_before_milli.value(),
                    })),
                    Some(serde_json::json!({
                        "item_id": command.item_id.to_string(),
                        "event_id": event_id.to_string(),
                        "source": event.source,
                        "quantity_consumed_milli": outcome.quantity_consumed_milli.value(),
                        "quantity_on_hand_milli": outcome.quantity_after_milli.value(),
                        "low_stock": outcome.low_stock_after,
                        "idempotency_key_sha256": sha256_hex(&idempotency_key),
                    })),
                );
                Ok((InventoryConsumptionResult { event, item }, vec![audit]))
            })
        })
        .await
    }

    pub async fn list_consumption_events(
        &self,
        query: ListConsumptionEventsQuery,
    ) -> Result<Vec<InventoryConsumptionEventView>, PgInventoryError> {
        let limit = normalized_limit(query.limit);
        let offset = query.offset.unwrap_or(0).max(0);
        let source_kind = normalize_source_kind(query.source_kind)?;
        let mut builder = QueryBuilder::<Postgres>::new(
            "SELECT e.id, e.item_id, i.iv_code, e.branch_id, e.stock_location_id, \
             e.source_kind, e.work_order_id, e.dispatch_id, e.quantity_before_milli, \
             e.quantity_consumed_milli, e.quantity_after_milli, e.unit_cost_won, e.cost_won, \
             e.consumed_by, e.occurred_at, e.memo, e.created_at \
             FROM inventory_consumption_events e \
             JOIN inventory_items i ON i.id = e.item_id AND i.org_id = e.org_id \
             WHERE e.item_id = ",
        );
        builder.push_bind(*query.item_id.as_uuid());
        builder.push(" AND ");
        push_branch_scope(&mut builder, &query.branch_scope, "e.branch_id");
        if let Some(source_kind) = source_kind {
            builder.push(" AND e.source_kind = ");
            builder.push_bind(source_kind);
        }
        if let Some(work_order_id) = query.work_order_id {
            builder.push(" AND e.work_order_id = ");
            builder.push_bind(*work_order_id.as_uuid());
        }
        if let Some(dispatch_id) = query.dispatch_id {
            builder.push(" AND e.dispatch_id = ");
            builder.push_bind(*dispatch_id.as_uuid());
        }
        builder.push(" ORDER BY e.occurred_at DESC, e.id DESC LIMIT ");
        builder.push_bind(limit);
        builder.push(" OFFSET ");
        builder.push_bind(offset);

        let org = current_org().map_err(KernelError::from)?;
        let rows = with_org_conn::<_, _, PgInventoryError>(&self.pool, org, move |tx| {
            Box::pin(async move { Ok(builder.build().fetch_all(tx.as_mut()).await?) })
        })
        .await?;
        let mut out = Vec::with_capacity(rows.len());
        for row in &rows {
            out.push(event_from_row(row)?);
        }
        Ok(out)
    }

    pub async fn record_receipt(
        &self,
        command: RecordReceiptCommand,
    ) -> Result<InventoryReceiptResult, PgInventoryError> {
        let quantity = PositiveQuantityMilli::new(command.quantity_received_milli)?;
        let key = normalize_idempotency_key(&command.idempotency_key)?;
        let source_ref = normalize_source_ref(command.source_ref)?;
        let memo = normalize_optional_text(command.memo, 1_000, "memo")?;
        let org = current_org().map_err(KernelError::from)?;
        let org_uuid = *org.as_uuid();
        with_audits::<_, InventoryReceiptResult, PgInventoryError>(&self.pool, org, |tx| {
            Box::pin(async move {
                lock_inventory_idempotency_key_tx(tx, org, &key).await?;
                let fingerprint = receipt_fingerprint(command.item_id, quantity, source_ref.as_deref(), memo.as_deref());
                if let Some((movement, existing)) = fetch_movement_by_idempotency_tx(tx, &key).await? {
                    if existing != fingerprint { return Err(KernelError::conflict("idempotency key was already used with a different inventory receipt payload").into()); }
                    let item = fetch_item_tx(tx, command.item_id).await?.ok_or_else(|| KernelError::internal("idempotent inventory item result was not readable"))?;
                    ensure_branch_allowed(&command.branch_scope, item.branch_id)?;
                    return Ok((InventoryReceiptResult { movement, item }, Vec::new()));
                }
                let before = fetch_item_for_update_tx(tx, command.item_id).await?.ok_or_else(|| KernelError::not_found("inventory item was not found"))?;
                ensure_branch_allowed(&command.branch_scope, before.branch_id)?;
                let after = before.quantity_on_hand_milli.checked_add(quantity.value()).ok_or_else(|| KernelError::validation("inventory receipt overflows quantity"))?;
                let id = uuid::Uuid::new_v4();
                sqlx::query(r#"INSERT INTO inventory_movements (id, org_id, branch_id, item_id, stock_location_id, kind, quantity_delta_milli, quantity_before_milli, quantity_after_milli, source_ref, memo, actor_id, occurred_at, idempotency_key, request_fingerprint)
                    VALUES ($1,$2,$3,$4,$5,'RECEIPT',$6,$7,$8,$9,$10,$11,$12,$13,$14)"#)
                    .bind(id).bind(org_uuid).bind(*before.branch_id.as_uuid()).bind(*command.item_id.as_uuid()).bind(*before.stock_location.id.as_uuid())
                    .bind(quantity.value()).bind(before.quantity_on_hand_milli).bind(after).bind(source_ref.as_deref()).bind(memo.as_deref()).bind(*command.actor.as_uuid()).bind(command.requested_at).bind(&key).bind(&fingerprint).execute(tx.as_mut()).await?;
                sqlx::query("UPDATE inventory_items SET quantity_on_hand_milli=$2, updated_at=$3 WHERE id=$1")
                    .bind(*command.item_id.as_uuid()).bind(after).bind(command.requested_at).execute(tx.as_mut()).await?;
                let movement = fetch_movement_tx(tx, id).await?.ok_or_else(|| KernelError::internal("created inventory receipt was not readable"))?;
                let item = fetch_item_tx(tx, command.item_id).await?.ok_or_else(|| KernelError::internal("received inventory item was not readable"))?;
                let audit = inventory_audit_event("inventory.receipt", Some(command.actor), Some(before.branch_id), "inventory_movement", id, command.trace, command.requested_at)?
                    .with_org(org).with_snapshots(Some(item_snapshot(&before)), Some(serde_json::json!({"movement_id":id,"quantity_before_milli":before.quantity_on_hand_milli,"quantity_delta_milli":quantity.value(),"quantity_after_milli":after,"idempotency_key_sha256":sha256_hex(&key)})));
                Ok((InventoryReceiptResult { movement, item }, vec![audit]))
            })
        }).await
    }

    pub async fn list_movements(
        &self,
        query: ListMovementsQuery,
    ) -> Result<Vec<InventoryMovementView>, PgInventoryError> {
        let limit = normalized_limit(query.limit);
        let offset = query.offset.unwrap_or(0).max(0);
        let org = current_org().map_err(KernelError::from)?;
        with_org_conn::<_, _, PgInventoryError>(&self.pool, org, move |tx| Box::pin(async move {
            fetch_item_scoped_tx(tx, query.item_id, &query.branch_scope)
                .await?
                .ok_or_else(|| KernelError::not_found("inventory item was not found"))?;
            let mut builder = QueryBuilder::<Postgres>::new(r#"SELECT * FROM (
                SELECT e.id, e.item_id, i.iv_code, 'ISSUE'::text AS kind, -e.quantity_consumed_milli AS quantity_delta_milli, e.quantity_before_milli, e.quantity_after_milli, e.work_order_id, e.dispatch_id, NULL::uuid AS cycle_count_id, NULL::text AS cc_code, NULL::text AS source_ref, e.consumed_by AS actor_id, e.occurred_at, e.memo
                FROM inventory_consumption_events e JOIN inventory_items i ON i.id=e.item_id AND i.org_id=e.org_id WHERE e.item_id = "#);
            builder.push_bind(*query.item_id.as_uuid()); builder.push(" AND "); push_branch_scope(&mut builder, &query.branch_scope, "e.branch_id");
            builder.push(r#" UNION ALL SELECT m.id,m.item_id,i.iv_code,m.kind,m.quantity_delta_milli,m.quantity_before_milli,m.quantity_after_milli,NULL::uuid,NULL::uuid,m.cycle_count_id,c.cc_code,m.source_ref,m.actor_id,m.occurred_at,m.memo FROM inventory_movements m JOIN inventory_items i ON i.id=m.item_id AND i.org_id=m.org_id LEFT JOIN inventory_cycle_counts c ON c.id=m.cycle_count_id AND c.org_id=m.org_id WHERE m.item_id = "#);
            builder.push_bind(*query.item_id.as_uuid()); builder.push(" AND "); push_branch_scope(&mut builder, &query.branch_scope, "m.branch_id");
            builder.push(") movement ORDER BY occurred_at DESC, id DESC LIMIT "); builder.push_bind(limit); builder.push(" OFFSET "); builder.push_bind(offset);
            let rows=builder.build().fetch_all(tx.as_mut()).await?; let mut result=Vec::with_capacity(rows.len()); for row in &rows { result.push(movement_from_row(row)?); } Ok(result)
        })).await
    }

    pub async fn mrp(&self, query: MrpQuery) -> Result<Vec<MrpLineView>, PgInventoryError> {
        ensure_branch_allowed(&query.branch_scope, query.branch_id)?;
        let org = current_org().map_err(KernelError::from)?;
        with_org_conn::<_,_,PgInventoryError>(&self.pool,org,move|tx|Box::pin(async move {
            let rows=sqlx::query(r#"SELECT i.id,i.iv_code,i.display_name,i.unit_code,i.quantity_on_hand_milli,i.safety_stock_milli,
              COALESCE((SELECT SUM(e.quantity_consumed_milli) FROM inventory_consumption_events e WHERE e.item_id=i.id AND e.occurred_at >= now()-interval '90 days'),0)::bigint AS usage
              FROM inventory_items i WHERE i.branch_id=$1 AND i.status='ACTIVE' ORDER BY i.iv_code"#).bind(*query.branch_id.as_uuid()).fetch_all(tx.as_mut()).await?;
            let mut out=Vec::with_capacity(rows.len()); for row in &rows { let on_hand:i64=row.try_get("quantity_on_hand_milli")?; let safety:i64=row.try_get("safety_stock_milli")?; let usage:i64=row.try_get("usage")?; let monthly=usage/3; let available=on_hand; let short=on_hand<safety; out.push(MrpLineView { item_id:InventoryItemId::from_uuid(row.try_get("id")?), iv_code:row.try_get("iv_code")?,display_name:row.try_get("display_name")?,unit_code:row.try_get("unit_code")?,quantity_on_hand_milli:on_hand,safety_stock_milli:safety,inbound_expected_milli:0,reserved_outbound_milli:0,monthly_usage_milli:monthly,cover_months_centi:(monthly>0).then_some(available.saturating_mul(100)/monthly),short,proposed_order_milli:if short {(safety+monthly-available).max(0)}else{0} }); } Ok(out)
        })).await
    }

    pub async fn list_cycle_counts(
        &self,
        query: ListCycleCountsQuery,
    ) -> Result<CycleCountPage, PgInventoryError> {
        ensure_branch_allowed(&query.branch_scope, query.branch_id)?;
        let limit = normalized_limit(query.limit);
        let offset = query.offset.unwrap_or(0).max(0);
        let org = current_org().map_err(KernelError::from)?;
        with_org_conn::<_,_,PgInventoryError>(&self.pool,org,move|tx|Box::pin(async move {
            let total:i64=sqlx::query_scalar("SELECT count(*) FROM inventory_cycle_counts WHERE branch_id=$1 AND ($2::text IS NULL OR status=$2)").bind(*query.branch_id.as_uuid()).bind(query.status.map(CycleCountStatus::as_db_str)).fetch_one(tx.as_mut()).await?;
            let rows=sqlx::query(r#"SELECT c.id,c.cc_code,c.branch_id,c.status,c.version,c.opened_by,c.submitted_by,c.submitted_at,c.decided_by,c.decided_at,c.decision_memo,c.created_at,c.updated_at,l.id AS stock_location_id,l.label AS stock_location_label, count(cl.id)::bigint AS line_count, count(cl.id) FILTER (WHERE cl.variance_milli <> 0)::bigint AS variance_line_count FROM inventory_cycle_counts c JOIN inventory_stock_locations l ON l.id=c.stock_location_id AND l.org_id=c.org_id LEFT JOIN inventory_cycle_count_lines cl ON cl.count_id=c.id AND cl.org_id=c.org_id WHERE c.branch_id=$1 AND ($2::text IS NULL OR c.status=$2) GROUP BY c.id,l.id,l.label ORDER BY c.updated_at DESC,c.id DESC LIMIT $3 OFFSET $4"#).bind(*query.branch_id.as_uuid()).bind(query.status.map(CycleCountStatus::as_db_str)).bind(limit).bind(offset).fetch_all(tx.as_mut()).await?;
            let mut items=Vec::with_capacity(rows.len()); for row in &rows { items.push(cycle_count_from_row(row)?); } Ok(CycleCountPage{items,limit,offset,total})
        })).await
    }

    pub async fn open_cycle_count(
        &self,
        command: OpenCycleCountCommand,
    ) -> Result<CycleCountDetail, PgInventoryError> {
        ensure_branch_allowed(&command.branch_scope, command.branch_id)?;
        let org = current_org().map_err(KernelError::from)?;
        let org_uuid = *org.as_uuid();
        with_audits::<_,CycleCountDetail,PgInventoryError>(&self.pool,org,|tx|Box::pin(async move {
          let location:Option<uuid::Uuid>=sqlx::query_scalar("SELECT id FROM inventory_stock_locations WHERE id=$1 AND branch_id=$2 AND status='ACTIVE'").bind(*command.stock_location_id.as_uuid()).bind(*command.branch_id.as_uuid()).fetch_optional(tx.as_mut()).await?; if location.is_none(){return Err(KernelError::not_found("active stock location was not found in branch").into())}
          let next:i64=sqlx::query_scalar(r#"INSERT INTO inventory_cycle_count_counters(org_id,last_value) VALUES($1,1) ON CONFLICT(org_id) DO UPDATE SET last_value=inventory_cycle_count_counters.last_value+1 RETURNING last_value"#).bind(org_uuid).fetch_one(tx.as_mut()).await?;
          let id=uuid::Uuid::new_v4(); let code=format!("IC-{next:04}");
          sqlx::query("INSERT INTO inventory_cycle_counts(id,org_id,branch_id,stock_location_id,cc_code,status,opened_by,created_at,updated_at) VALUES($1,$2,$3,$4,$5,'DRAFT',$6,$7,$7)").bind(id).bind(org_uuid).bind(*command.branch_id.as_uuid()).bind(*command.stock_location_id.as_uuid()).bind(&code).bind(*command.actor.as_uuid()).bind(command.occurred_at).execute(tx.as_mut()).await?;
          let detail=fetch_cycle_detail_tx(tx,id).await?.ok_or_else(||KernelError::internal("opened cycle count was not readable"))?;
          let audit=inventory_audit_event("inventory.cycle_count.open",Some(command.actor),Some(command.branch_id),"inventory_cycle_count",id,command.trace,command.occurred_at)?.with_org(org); Ok((detail,vec![audit]))
        })).await
    }

    pub async fn get_cycle_count(
        &self,
        count_id: uuid::Uuid,
        scope: BranchScope,
    ) -> Result<Option<CycleCountDetail>, PgInventoryError> {
        let org = current_org().map_err(KernelError::from)?;
        with_org_conn::<_, _, PgInventoryError>(&self.pool, org, move |tx| {
            Box::pin(async move {
                let detail = fetch_cycle_detail_tx(tx, count_id).await?;
                if let Some(detail) = detail {
                    ensure_branch_allowed(&scope, detail.count.branch_id)?;
                    Ok(Some(detail))
                } else {
                    Ok(None)
                }
            })
        })
        .await
    }

    pub async fn upsert_cycle_count_line(
        &self,
        command: UpsertCountLineCommand,
    ) -> Result<CycleCountDetail, PgInventoryError> {
        let counted = QuantityMilli::new(command.counted_quantity_milli)?;
        let note = normalize_optional_text(command.note, 500, "note")?;
        let org = current_org().map_err(KernelError::from)?;
        let org_uuid = *org.as_uuid();
        with_audits::<_, CycleCountDetail, PgInventoryError>(&self.pool, org, |tx| {
            Box::pin(async move {
                let count = lock_cycle_count_tx(tx, command.count_id)
                    .await?
                    .ok_or_else(|| KernelError::not_found("cycle count was not found"))?;
                ensure_branch_allowed(&command.branch_scope, count.branch_id)?;
                if count.status != CycleCountStatus::Draft || count.version != command.expected_version {
                    return Err(KernelError::conflict("cycle count draft version no longer matches").into());
                }
                let item = fetch_item_for_update_tx(tx, command.item_id)
                    .await?
                    .ok_or_else(|| KernelError::not_found("inventory item was not found"))?;
                if item.branch_id != count.branch_id || item.stock_location.id != count.stock_location.id {
                    return Err(KernelError::conflict("cycle count item must belong to count location and branch").into());
                }
                if counted.value() != item.quantity_on_hand_milli && command.reason.is_none() {
                    return Err(KernelError::validation("variance reason is required").into());
                }
                sqlx::query(r#"INSERT INTO inventory_cycle_count_lines(id,org_id,count_id,item_id,system_quantity_milli,counted_quantity_milli,reason,note,recorded_by,recorded_at,created_at,updated_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$10,$10) ON CONFLICT(count_id,item_id) DO UPDATE SET system_quantity_milli=EXCLUDED.system_quantity_milli,counted_quantity_milli=EXCLUDED.counted_quantity_milli,reason=EXCLUDED.reason,note=EXCLUDED.note,recorded_by=EXCLUDED.recorded_by,recorded_at=EXCLUDED.recorded_at,updated_at=EXCLUDED.updated_at"#)
                    .bind(uuid::Uuid::new_v4()).bind(org_uuid).bind(command.count_id).bind(*command.item_id.as_uuid())
                    .bind(item.quantity_on_hand_milli).bind(counted.value()).bind(command.reason.map(VarianceReason::as_db_str))
                    .bind(note).bind(*command.actor.as_uuid()).bind(command.occurred_at).execute(tx.as_mut()).await?;
                sqlx::query("UPDATE inventory_cycle_counts SET version=version+1,updated_at=$2 WHERE id=$1")
                    .bind(command.count_id).bind(command.occurred_at).execute(tx.as_mut()).await?;
                let detail = fetch_cycle_detail_tx(tx, command.count_id).await?
                    .ok_or_else(|| KernelError::internal("cycle count was not readable after line update"))?;
                let audit = inventory_audit_event("inventory.cycle_count.line.upsert", Some(command.actor), Some(count.branch_id), "inventory_cycle_count", command.count_id, command.trace, command.occurred_at)?.with_org(org);
                Ok((detail, vec![audit]))
            })
        }).await
    }

    pub async fn submit_cycle_count(
        &self,
        command: SubmitCycleCountCommand,
    ) -> Result<CycleCountDetail, PgInventoryError> {
        let org = current_org().map_err(KernelError::from)?;
        with_audits::<_,CycleCountDetail,PgInventoryError>(&self.pool,org,|tx|Box::pin(async move{let count=lock_cycle_count_tx(tx,command.count_id).await?.ok_or_else(||KernelError::not_found("cycle count was not found"))?;ensure_branch_allowed(&command.branch_scope,count.branch_id)?;if count.status!=CycleCountStatus::Draft||count.version!=command.expected_version{return Err(KernelError::conflict("cycle count draft version no longer matches").into())};let lines:i64=sqlx::query_scalar("SELECT count(*) FROM inventory_cycle_count_lines WHERE count_id=$1").bind(command.count_id).fetch_one(tx.as_mut()).await?;if lines==0{return Err(KernelError::validation("cycle count requires at least one line").into())};sqlx::query("UPDATE inventory_cycle_counts SET status='SUBMITTED',submitted_by=$2,submitted_at=$3,version=version+1,updated_at=$3 WHERE id=$1").bind(command.count_id).bind(*command.actor.as_uuid()).bind(command.occurred_at).execute(tx.as_mut()).await?;let detail=fetch_cycle_detail_tx(tx,command.count_id).await?.ok_or_else(||KernelError::internal("submitted count missing"))?;let audit=inventory_audit_event("inventory.cycle_count.submit",Some(command.actor),Some(count.branch_id),"inventory_cycle_count",command.count_id,command.trace,command.occurred_at)?.with_org(org);Ok((detail,vec![audit]))})).await
    }

    pub async fn cancel_cycle_count(
        &self,
        command: CancelCycleCountCommand,
    ) -> Result<CycleCountDetail, PgInventoryError> {
        let org = current_org().map_err(KernelError::from)?;
        with_audits::<_, CycleCountDetail, PgInventoryError>(&self.pool, org, |tx| {
            Box::pin(async move {
                let count = lock_cycle_count_tx(tx, command.count_id).await?
                    .ok_or_else(|| KernelError::not_found("cycle count was not found"))?;
                ensure_branch_allowed(&command.branch_scope, count.branch_id)?;
                if !matches!(count.status, CycleCountStatus::Draft | CycleCountStatus::Submitted)
                    || count.version != command.expected_version {
                    return Err(KernelError::conflict("cycle count version or state no longer permits cancellation").into());
                }
                sqlx::query("UPDATE inventory_cycle_counts SET status='CANCELLED',version=version+1,updated_at=$2 WHERE id=$1")
                    .bind(command.count_id).bind(command.occurred_at).execute(tx.as_mut()).await?;
                let detail = fetch_cycle_detail_tx(tx, command.count_id).await?
                    .ok_or_else(|| KernelError::internal("cancelled count missing"))?;
                let audit = inventory_audit_event("inventory.cycle_count.cancel", Some(command.actor), Some(count.branch_id), "inventory_cycle_count", command.count_id, command.trace, command.occurred_at)?.with_org(org);
                Ok((detail, vec![audit]))
            })
        }).await
    }

    pub async fn decide_cycle_count(
        &self,
        command: DecideCycleCountCommand,
    ) -> Result<CycleCountDetail, PgInventoryError> {
        let memo = normalize_optional_text(command.memo, 1_000, "decision memo")?;
        let org = current_org().map_err(KernelError::from)?;
        let org_uuid = *org.as_uuid();
        with_audits::<_, CycleCountDetail, PgInventoryError>(&self.pool, org, |tx| {
            Box::pin(async move {
                let count = lock_cycle_count_tx(tx, command.count_id).await?
                    .ok_or_else(|| KernelError::not_found("cycle count was not found"))?;
                ensure_branch_allowed(&command.branch_scope, count.branch_id)?;

                // An approve retry is evaluated against its whole canonical payload before
                // mutable state/version checks. This keeps retries safe after the first
                // transaction advanced the aggregate version.
                if matches!(command.decision, CycleCountDecision::Approve) {
                    let key = normalize_idempotency_key(command.idempotency_key.as_deref()
                        .ok_or_else(|| KernelError::validation("approval idempotency key is required"))?)?;
                    lock_inventory_idempotency_key_tx(tx, org, &key).await?;
                    let fingerprint = cycle_approval_fingerprint(command.count_id, command.expected_version, command.actor, memo.as_deref());
                    if let Some((existing_count_id, existing_fingerprint)) = sqlx::query_as::<_, (uuid::Uuid, String)>(
                        "SELECT id, decision_request_fingerprint FROM inventory_cycle_counts WHERE decision_idempotency_key=$1"
                    ).bind(&key).fetch_optional(tx.as_mut()).await? {
                        if existing_count_id != command.count_id || existing_fingerprint != fingerprint {
                            return Err(KernelError::conflict("approval idempotency key was already used with a different cycle decision payload").into());
                        }
                        let detail = fetch_cycle_detail_tx(tx, command.count_id).await?
                            .ok_or_else(|| KernelError::internal("idempotent cycle approval was not readable"))?;
                        return Ok((detail, Vec::new()));
                    }
                    if count.status != CycleCountStatus::Submitted || count.version != command.expected_version {
                        return Err(KernelError::conflict("cycle count submitted version no longer matches").into());
                    }
                    let submitter: uuid::Uuid = sqlx::query_scalar("SELECT submitted_by FROM inventory_cycle_counts WHERE id=$1")
                        .bind(command.count_id).fetch_one(tx.as_mut()).await?;
                    if submitter == *command.actor.as_uuid() {
                        return Err(KernelError::forbidden("cycle count submitter cannot decide the count").into());
                    }
                    sqlx::query("UPDATE inventory_cycle_counts SET status='APPROVED',decided_by=$2,decided_at=$3,decision_memo=$4,decision_idempotency_key=$5,decision_request_fingerprint=$6,version=version+1,updated_at=$3 WHERE id=$1")
                        .bind(command.count_id).bind(*command.actor.as_uuid()).bind(command.occurred_at).bind(memo)
                        .bind(&key).bind(&fingerprint).execute(tx.as_mut()).await?;
                    let lines = sqlx::query("SELECT item_id, variance_milli FROM inventory_cycle_count_lines WHERE count_id=$1 AND variance_milli<>0 ORDER BY item_id FOR UPDATE")
                        .bind(command.count_id).fetch_all(tx.as_mut()).await?;
                    for line in lines {
                        let item_id: uuid::Uuid = line.try_get("item_id")?;
                        // The recorded variance is immutable evidence. Apply it to the
                        // currently locked stock so receipts/issues after counting survive.
                        let delta: i64 = line.try_get("variance_milli")?;
                        let item = fetch_item_for_update_tx(tx, InventoryItemId::from_uuid(item_id)).await?
                            .ok_or_else(|| KernelError::internal("cycle count line item disappeared"))?;
                        if item.branch_id != count.branch_id || item.stock_location.id != count.stock_location.id {
                            return Err(KernelError::conflict("cycle count item moved outside count location").into());
                        }
                        let after = item.quantity_on_hand_milli.checked_add(delta).filter(|value| *value >= 0)
                            .ok_or_else(|| KernelError::conflict("cycle count adjustment would make stock negative"))?;
                        let movement_id = uuid::Uuid::new_v4();
                        sqlx::query("INSERT INTO inventory_movements(id,org_id,branch_id,item_id,stock_location_id,kind,quantity_delta_milli,quantity_before_milli,quantity_after_milli,cycle_count_id,actor_id,occurred_at) VALUES($1,$2,$3,$4,$5,'ADJUSTMENT',$6,$7,$8,$9,$10,$11)")
                            .bind(movement_id).bind(org_uuid).bind(*count.branch_id.as_uuid()).bind(item_id).bind(*count.stock_location.id.as_uuid())
                            .bind(delta).bind(item.quantity_on_hand_milli).bind(after).bind(command.count_id).bind(*command.actor.as_uuid()).bind(command.occurred_at)
                            .execute(tx.as_mut()).await?;
                        sqlx::query("UPDATE inventory_items SET quantity_on_hand_milli=$2,updated_at=$3 WHERE id=$1")
                            .bind(item_id).bind(after).bind(command.occurred_at).execute(tx.as_mut()).await?;
                    }
                } else {
                    if count.status != CycleCountStatus::Submitted || count.version != command.expected_version {
                        return Err(KernelError::conflict("cycle count submitted version no longer matches").into());
                    }
                    let submitter: uuid::Uuid = sqlx::query_scalar("SELECT submitted_by FROM inventory_cycle_counts WHERE id=$1")
                        .bind(command.count_id).fetch_one(tx.as_mut()).await?;
                    if submitter == *command.actor.as_uuid() { return Err(KernelError::forbidden("cycle count submitter cannot decide the count").into()); }
                    if memo.is_none() { return Err(KernelError::validation("rejection memo is required").into()); }
                    sqlx::query("UPDATE inventory_cycle_counts SET status='REJECTED',decided_by=$2,decided_at=$3,decision_memo=$4,version=version+1,updated_at=$3 WHERE id=$1")
                        .bind(command.count_id).bind(*command.actor.as_uuid()).bind(command.occurred_at).bind(memo).execute(tx.as_mut()).await?;
                }
                let detail = fetch_cycle_detail_tx(tx, command.count_id).await?
                    .ok_or_else(|| KernelError::internal("decided count missing"))?;
                let audit = inventory_audit_event("inventory.cycle_count.decide", Some(command.actor), Some(count.branch_id), "inventory_cycle_count", command.count_id, command.trace, command.occurred_at)?.with_org(org);
                Ok((detail, vec![audit]))
            })
        }).await
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ResolvedConsumptionSource {
    source: InventoryConsumptionSource,
    branch_id: BranchId,
}

fn normalized_limit(limit: Option<i64>) -> i64 {
    limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT)
}

fn ensure_branch_allowed(scope: &BranchScope, branch_id: BranchId) -> Result<(), KernelError> {
    if scope.allows(branch_id) {
        Ok(())
    } else {
        Err(KernelError::forbidden(
            "inventory branch is outside principal scope",
        ))
    }
}

fn normalize_required_text(
    value: &str,
    max_chars: usize,
    field: &str,
) -> Result<String, KernelError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(KernelError::validation(format!("{field} is required")));
    }
    if trimmed.chars().count() > max_chars {
        return Err(KernelError::validation(format!("{field} is too long")));
    }
    Ok(trimmed.to_owned())
}

fn normalize_optional_text(
    value: Option<String>,
    max_chars: usize,
    field: &str,
) -> Result<Option<String>, KernelError> {
    value
        .map(|value| {
            let trimmed = value.trim();
            if trimmed.is_empty() {
                Ok(None)
            } else if trimmed.chars().count() > max_chars {
                Err(KernelError::validation(format!("{field} is too long")))
            } else {
                Ok(Some(trimmed.to_owned()))
            }
        })
        .transpose()
        .map(Option::flatten)
}

fn normalize_idempotency_key(value: &str) -> Result<String, KernelError> {
    let trimmed = value.trim();
    if !(16..=200).contains(&trimmed.chars().count()) {
        return Err(KernelError::validation(
            "idempotency_key must be between 16 and 200 characters",
        ));
    }
    Ok(trimmed.to_owned())
}

fn normalize_source_kind(value: Option<String>) -> Result<Option<String>, KernelError> {
    value
        .map(|value| {
            let normalized = value.trim().to_ascii_uppercase();
            if normalized.is_empty() {
                Ok(None)
            } else if matches!(normalized.as_str(), "WORK_ORDER" | "P1_DISPATCH") {
                Ok(Some(normalized))
            } else {
                Err(KernelError::validation("unknown inventory source kind"))
            }
        })
        .transpose()
        .map(Option::flatten)
}

fn normalize_update_fields(
    fields: UpdateInventoryItemFields,
) -> Result<UpdateInventoryItemFields, KernelError> {
    Ok(UpdateInventoryItemFields {
        sku: fields
            .sku
            .map(|value| normalize_optional_text(value, 80, "sku"))
            .transpose()?,
        display_name: fields
            .display_name
            .map(|value| normalize_required_text(&value, 120, "display_name"))
            .transpose()?,
        description: fields
            .description
            .map(|value| normalize_optional_text(value, 1_000, "description"))
            .transpose()?,
        safety_stock_milli: fields
            .safety_stock_milli
            .map(|value| SafetyStockMilli::new(value).map(SafetyStockMilli::value))
            .transpose()?,
        status: fields.status,
    })
}

fn issue_inventory_code(item_id: InventoryItemId) -> Result<InventoryCode, KernelError> {
    let raw = item_id.as_uuid().simple().to_string();
    let suffix = raw
        .chars()
        .take(12)
        .collect::<String>()
        .to_ascii_uppercase();
    InventoryCode::new(format!("IV-{suffix}"))
}

fn push_branch_scope(builder: &mut QueryBuilder<Postgres>, scope: &BranchScope, column: &str) {
    match scope {
        BranchScope::All => builder.push("TRUE"),
        BranchScope::Branches(branches) if branches.is_empty() => builder.push("FALSE"),
        BranchScope::Branches(branches) => {
            let ids = branches
                .iter()
                .map(|branch| *branch.as_uuid())
                .collect::<Vec<_>>();
            builder.push(column);
            builder.push(" = ANY(");
            builder.push_bind(ids);
            builder.push(")")
        }
    };
}

fn push_item_filters(
    builder: &mut QueryBuilder<Postgres>,
    query: &ListInventoryItemsQuery,
) -> Result<(), KernelError> {
    push_branch_scope(builder, &query.branch_scope, "i.branch_id");
    if let Some(branch_id) = query.branch_id {
        if !query.branch_scope.allows(branch_id) {
            builder.push(" AND FALSE");
        } else {
            builder.push(" AND i.branch_id = ");
            builder.push_bind(*branch_id.as_uuid());
        }
    }
    if let Some(site_id) = query.site_id {
        builder.push(" AND i.site_id = ");
        builder.push_bind(*site_id.as_uuid());
    }
    if let Some(stock_location_id) = query.stock_location_id {
        builder.push(" AND i.stock_location_id = ");
        builder.push_bind(*stock_location_id.as_uuid());
    }
    if let Some(status) = query.status {
        builder.push(" AND i.status = ");
        builder.push_bind(status.as_db_str());
    }
    if let Some(low_stock) = query.low_stock {
        if low_stock {
            builder.push(" AND i.quantity_on_hand_milli <= i.safety_stock_milli");
        } else {
            builder.push(" AND i.quantity_on_hand_milli > i.safety_stock_milli");
        }
    }
    if let Some(q) = query.q.as_ref().map(|q| q.trim()).filter(|q| !q.is_empty()) {
        let pattern = format!("%{q}%");
        builder.push(" AND (i.iv_code ILIKE ");
        builder.push_bind(pattern.clone());
        builder.push(" OR i.sku ILIKE ");
        builder.push_bind(pattern.clone());
        builder.push(" OR i.display_name ILIKE ");
        builder.push_bind(pattern);
        builder.push(")");
    }
    Ok(())
}

async fn fetch_location_tx(
    tx: &mut Transaction<'_, Postgres>,
    location_id: InventoryStockLocationId,
) -> Result<Option<InventoryStockLocationView>, PgInventoryError> {
    let row = sqlx::query(
        r#"
        SELECT id, branch_id, site_id, location_code, label, status, created_at, updated_at
        FROM inventory_stock_locations
        WHERE id = $1
        "#,
    )
    .bind(*location_id.as_uuid())
    .fetch_optional(tx.as_mut())
    .await?;
    row.as_ref().map(location_from_row).transpose()
}

async fn fetch_item_tx(
    tx: &mut Transaction<'_, Postgres>,
    item_id: InventoryItemId,
) -> Result<Option<InventoryItemView>, PgInventoryError> {
    fetch_item_inner_tx(tx, item_id, false).await
}

async fn fetch_item_for_update_tx(
    tx: &mut Transaction<'_, Postgres>,
    item_id: InventoryItemId,
) -> Result<Option<InventoryItemView>, PgInventoryError> {
    fetch_item_inner_tx(tx, item_id, true).await
}

async fn fetch_item_scoped_tx(
    tx: &mut Transaction<'_, Postgres>,
    item_id: InventoryItemId,
    branch_scope: &BranchScope,
) -> Result<Option<InventoryItemView>, PgInventoryError> {
    let mut builder = QueryBuilder::<Postgres>::new("SELECT ");
    builder.push(ITEM_COLUMNS);
    builder.push(
        " FROM inventory_items i JOIN inventory_stock_locations l \
         ON l.id = i.stock_location_id AND l.org_id = i.org_id WHERE i.id = ",
    );
    builder.push_bind(*item_id.as_uuid());
    builder.push(" AND ");
    push_branch_scope(&mut builder, branch_scope, "i.branch_id");
    let row = builder.build().fetch_optional(tx.as_mut()).await?;
    row.as_ref().map(item_from_row).transpose()
}

async fn fetch_item_inner_tx(
    tx: &mut Transaction<'_, Postgres>,
    item_id: InventoryItemId,
    for_update: bool,
) -> Result<Option<InventoryItemView>, PgInventoryError> {
    let mut sql = format!(
        "SELECT {ITEM_COLUMNS} FROM inventory_items i JOIN inventory_stock_locations l \
         ON l.id = i.stock_location_id AND l.org_id = i.org_id WHERE i.id = $1"
    );
    if for_update {
        sql.push_str(" FOR UPDATE OF i");
    }
    // Audited SQL-safe: `sql` is composed only from the `ITEM_COLUMNS` const and a
    // fixed `FOR UPDATE OF i` clause gated on the `for_update` bool. No runtime value
    // is interpolated — `item_id` is bound as $1 below.
    let row = sqlx::query(sqlx::AssertSqlSafe(sql))
        .bind(*item_id.as_uuid())
        .fetch_optional(tx.as_mut())
        .await?;
    row.as_ref().map(item_from_row).transpose()
}

async fn resolve_consumption_source_tx(
    tx: &mut Transaction<'_, Postgres>,
    source: ConsumeInventorySource,
) -> Result<ResolvedConsumptionSource, PgInventoryError> {
    match source {
        ConsumeInventorySource::WorkOrder { work_order_id } => {
            let branch_id: Option<uuid::Uuid> =
                sqlx::query_scalar("SELECT branch_id FROM work_orders WHERE id = $1")
                    .bind(*work_order_id.as_uuid())
                    .fetch_optional(tx.as_mut())
                    .await?;
            let branch_id = branch_id
                .map(BranchId::from_uuid)
                .ok_or_else(|| KernelError::not_found("work order was not found"))?;
            Ok(ResolvedConsumptionSource {
                source: InventoryConsumptionSource::WorkOrder { work_order_id },
                branch_id,
            })
        }
        ConsumeInventorySource::P1Dispatch { dispatch_id } => {
            let row =
                sqlx::query("SELECT branch_id, work_order_id FROM p1_dispatches WHERE id = $1")
                    .bind(*dispatch_id.as_uuid())
                    .fetch_optional(tx.as_mut())
                    .await?;
            let row = row.ok_or_else(|| KernelError::not_found("P1 dispatch was not found"))?;
            let branch_id = BranchId::from_uuid(row.try_get("branch_id")?);
            let work_order_id = WorkOrderId::from_uuid(row.try_get("work_order_id")?);
            Ok(ResolvedConsumptionSource {
                source: InventoryConsumptionSource::P1Dispatch {
                    dispatch_id,
                    work_order_id,
                },
                branch_id,
            })
        }
    }
}

async fn fetch_event_tx(
    tx: &mut Transaction<'_, Postgres>,
    event_id: InventoryConsumptionEventId,
) -> Result<Option<InventoryConsumptionEventView>, PgInventoryError> {
    let row = sqlx::query(
        r#"
        SELECT e.id, e.item_id, i.iv_code, e.branch_id, e.stock_location_id,
               e.source_kind, e.work_order_id, e.dispatch_id,
               e.quantity_before_milli, e.quantity_consumed_milli, e.quantity_after_milli,
               e.unit_cost_won, e.cost_won, e.consumed_by, e.occurred_at, e.memo, e.created_at
        FROM inventory_consumption_events e
        JOIN inventory_items i ON i.id = e.item_id AND i.org_id = e.org_id
        WHERE e.id = $1
        "#,
    )
    .bind(*event_id.as_uuid())
    .fetch_optional(tx.as_mut())
    .await?;
    row.as_ref().map(event_from_row).transpose()
}

async fn fetch_event_by_idempotency_key_tx(
    tx: &mut Transaction<'_, Postgres>,
    idempotency_key: &str,
) -> Result<Option<(InventoryConsumptionEventView, String)>, PgInventoryError> {
    let row = sqlx::query(
        r#"
        SELECT e.id, e.item_id, i.iv_code, e.branch_id, e.stock_location_id,
               e.source_kind, e.work_order_id, e.dispatch_id,
               e.quantity_before_milli, e.quantity_consumed_milli, e.quantity_after_milli,
               e.unit_cost_won, e.cost_won, e.consumed_by, e.occurred_at, e.memo, e.created_at,
               e.request_fingerprint
        FROM inventory_consumption_events e
        JOIN inventory_items i ON i.id = e.item_id AND i.org_id = e.org_id
        WHERE e.idempotency_key = $1
        "#,
    )
    .bind(idempotency_key)
    .fetch_optional(tx.as_mut())
    .await?;
    row.as_ref()
        .map(|row| Ok((event_from_row(row)?, row.try_get("request_fingerprint")?)))
        .transpose()
}

async fn lock_consumption_idempotency_key_tx(
    tx: &mut Transaction<'_, Postgres>,
    org: OrgId,
    idempotency_key: &str,
) -> Result<(), PgInventoryError> {
    // Transaction-scoped advisory locks release automatically on both commit
    // and rollback. PostgreSQL calculates one stable 64-bit identity from an
    // unambiguous, length-delimited raw `(org, normalized key)` composite.
    // Hash collisions can only over-serialize requests: raw event lookup,
    // `(org_id, idempotency_key)` uniqueness, and RLS still prevent a collision
    // from replaying or mutating another tenant/key's result.
    sqlx::query(
        "SELECT pg_advisory_xact_lock(hashtextextended(\
            char_length($1::text)::text || ':' || $1::text || \
            char_length($2::text)::text || ':' || $2::text, \
            0\
        ))",
    )
    .bind(org.to_string())
    .bind(idempotency_key)
    .execute(tx.as_mut())
    .await?;
    Ok(())
}

fn location_from_row(
    row: &sqlx::postgres::PgRow,
) -> Result<InventoryStockLocationView, PgInventoryError> {
    let status: String = row.try_get("status")?;
    let site_id: Option<uuid::Uuid> = row.try_get("site_id")?;
    Ok(InventoryStockLocationView {
        id: InventoryStockLocationId::from_uuid(row.try_get("id")?),
        branch_id: BranchId::from_uuid(row.try_get("branch_id")?),
        site_id: site_id.map(SiteId::from_uuid),
        location_code: row.try_get("location_code")?,
        label: row.try_get("label")?,
        status: InventoryItemStatus::parse(&status)?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

fn item_from_row(row: &sqlx::postgres::PgRow) -> Result<InventoryItemView, PgInventoryError> {
    let status: String = row.try_get("status")?;
    let site_id: Option<uuid::Uuid> = row.try_get("site_id")?;
    let quantity_on_hand_milli: i64 = row.try_get("quantity_on_hand_milli")?;
    let safety_stock_milli: i64 = row.try_get("safety_stock_milli")?;
    let id = InventoryItemId::from_uuid(row.try_get("id")?);
    Ok(InventoryItemView {
        id,
        branch_id: BranchId::from_uuid(row.try_get("branch_id")?),
        site_id: site_id.map(SiteId::from_uuid),
        stock_location: InventoryStockLocationSummary {
            id: InventoryStockLocationId::from_uuid(row.try_get("stock_location_id")?),
            label: row.try_get("stock_location_label")?,
        },
        iv_code: row.try_get("iv_code")?,
        sku: row.try_get("sku")?,
        display_name: row.try_get("display_name")?,
        description: row.try_get("description")?,
        unit_code: row.try_get("unit_code")?,
        quantity_on_hand_milli,
        safety_stock_milli,
        unit_cost_won: row.try_get("unit_cost_won")?,
        low_stock: quantity_on_hand_milli <= safety_stock_milli,
        status: InventoryItemStatus::parse(&status)?,
        href: format!("/inventory/items/{id}"),
        created_by: UserId::from_uuid(row.try_get("created_by")?),
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

fn event_from_row(
    row: &sqlx::postgres::PgRow,
) -> Result<InventoryConsumptionEventView, PgInventoryError> {
    let source_kind: String = row.try_get("source_kind")?;
    let work_order_id = WorkOrderId::from_uuid(row.try_get("work_order_id")?);
    let dispatch_id: Option<uuid::Uuid> = row.try_get("dispatch_id")?;
    let source = match source_kind.as_str() {
        "WORK_ORDER" => InventoryConsumptionSource::WorkOrder { work_order_id },
        "P1_DISPATCH" => InventoryConsumptionSource::P1Dispatch {
            dispatch_id: dispatch_id
                .map(P1DispatchId::from_uuid)
                .ok_or_else(|| KernelError::internal("P1 dispatch event missing dispatch_id"))?,
            work_order_id,
        },
        _ => return Err(KernelError::internal("unknown inventory event source_kind").into()),
    };
    Ok(InventoryConsumptionEventView {
        id: InventoryConsumptionEventId::from_uuid(row.try_get("id")?),
        item_id: InventoryItemId::from_uuid(row.try_get("item_id")?),
        iv_code: row.try_get("iv_code")?,
        branch_id: BranchId::from_uuid(row.try_get("branch_id")?),
        stock_location_id: InventoryStockLocationId::from_uuid(row.try_get("stock_location_id")?),
        source,
        quantity_before_milli: row.try_get("quantity_before_milli")?,
        quantity_consumed_milli: row.try_get("quantity_consumed_milli")?,
        quantity_after_milli: row.try_get("quantity_after_milli")?,
        unit_cost_won: row.try_get("unit_cost_won")?,
        cost_won: row.try_get("cost_won")?,
        consumed_by: UserId::from_uuid(row.try_get("consumed_by")?),
        occurred_at: row.try_get("occurred_at")?,
        memo: row.try_get("memo")?,
        created_at: row.try_get("created_at")?,
    })
}

fn item_state(item: &InventoryItemView) -> Result<InventoryItemState, KernelError> {
    Ok(InventoryItemState::new(
        item.id,
        item.branch_id,
        item.stock_location.id,
        item.status,
        QuantityMilli::new(item.quantity_on_hand_milli)?,
        SafetyStockMilli::new(item.safety_stock_milli)?,
        item.unit_cost_won.map(MoneyWon::new).transpose()?,
    ))
}

fn item_snapshot(item: &InventoryItemView) -> serde_json::Value {
    serde_json::json!({
        "id": item.id.to_string(),
        "iv_code": item.iv_code,
        "branch_id": item.branch_id.to_string(),
        "site_id": item.site_id.map(|id| id.to_string()),
        "stock_location_id": item.stock_location.id.to_string(),
        "sku": item.sku,
        "display_name": item.display_name,
        "unit_code": item.unit_code,
        "quantity_on_hand_milli": item.quantity_on_hand_milli,
        "safety_stock_milli": item.safety_stock_milli,
        "low_stock": item.low_stock,
        "status": item.status.as_db_str(),
    })
}

fn location_snapshot(location: &InventoryStockLocationView) -> serde_json::Value {
    serde_json::json!({
        "id": location.id.to_string(),
        "branch_id": location.branch_id.to_string(),
        "site_id": location.site_id.map(|id| id.to_string()),
        "location_code": location.location_code,
        "label": location.label,
        "status": location.status.as_db_str(),
    })
}

fn request_fingerprint(
    item_id: InventoryItemId,
    source: InventoryConsumptionSource,
    quantity: PositiveQuantityMilli,
    memo: Option<&str>,
    occurred_at: Option<time::OffsetDateTime>,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(item_id.as_uuid().as_bytes());
    hasher.update(source.kind_db_str().as_bytes());
    hasher.update(source.work_order_id().as_uuid().as_bytes());
    if let Some(dispatch_id) = source.dispatch_id() {
        hasher.update(dispatch_id.as_uuid().as_bytes());
    }
    hasher.update(quantity.value().to_be_bytes());
    if let Some(memo) = memo {
        hasher.update(memo.as_bytes());
    }
    match occurred_at {
        Some(occurred_at) => {
            hasher.update(b"occurred_at:explicit");
            hasher.update(occurred_at.unix_timestamp_nanos().to_be_bytes());
        }
        // `requested_at` is server metadata, not a caller-controlled payload
        // field. Omitted `occurred_at` therefore has a stable presence tag and
        // deliberately contributes no generated timestamp to the fingerprint.
        None => hasher.update(b"occurred_at:omitted"),
    }
    hex::encode(hasher.finalize())
}

fn sha256_hex(value: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(value.as_bytes());
    hex::encode(hasher.finalize())
}

fn normalize_source_ref(value: Option<String>) -> Result<Option<String>, KernelError> {
    let Some(value) = normalize_optional_text(value, 44, "source_ref")? else {
        return Ok(None);
    };
    let valid = value.len() >= 3
        && value.len() <= 45
        && value
            .bytes()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'-')
        && value.split_once('-').is_some_and(|(prefix, suffix)| {
            !prefix.is_empty() && prefix.len() <= 4 && !suffix.is_empty()
        });
    if valid {
        Ok(Some(value))
    } else {
        Err(KernelError::validation("source_ref has invalid format"))
    }
}

fn cycle_approval_fingerprint(
    count_id: uuid::Uuid,
    expected_version: i32,
    actor: UserId,
    memo: Option<&str>,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"cycle-approval:v2");
    hasher.update(count_id.as_bytes());
    hasher.update(expected_version.to_be_bytes());
    hasher.update(actor.as_uuid().as_bytes());
    match memo {
        Some(value) => {
            hasher.update([1]);
            hasher.update(value.as_bytes());
        }
        None => hasher.update([0]),
    }
    hex::encode(hasher.finalize())
}

fn receipt_fingerprint(
    item_id: InventoryItemId,
    quantity: PositiveQuantityMilli,
    source_ref: Option<&str>,
    memo: Option<&str>,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"receipt:v1");
    hasher.update(item_id.as_uuid().as_bytes());
    hasher.update(quantity.value().to_be_bytes());
    for value in [source_ref, memo] {
        match value {
            Some(value) => {
                hasher.update([1]);
                hasher.update(value.as_bytes());
            }
            None => hasher.update([0]),
        }
    }
    hex::encode(hasher.finalize())
}

async fn lock_inventory_idempotency_key_tx(
    tx: &mut Transaction<'_, Postgres>,
    org: OrgId,
    key: &str,
) -> Result<(), PgInventoryError> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended(char_length($1::text)::text || ':' || $1::text || char_length($2::text)::text || ':' || $2::text, 0))")
      .bind(org.to_string()).bind(key).execute(tx.as_mut()).await?;
    Ok(())
}

async fn fetch_movement_by_idempotency_tx(
    tx: &mut Transaction<'_, Postgres>,
    key: &str,
) -> Result<Option<(InventoryMovementView, String)>, PgInventoryError> {
    let row=sqlx::query(r#"SELECT m.id,m.item_id,i.iv_code,m.kind,m.quantity_delta_milli,m.quantity_before_milli,m.quantity_after_milli,NULL::uuid AS work_order_id,NULL::uuid AS dispatch_id,m.cycle_count_id,c.cc_code,m.source_ref,m.actor_id,m.occurred_at,m.memo,m.request_fingerprint FROM inventory_movements m JOIN inventory_items i ON i.id=m.item_id AND i.org_id=m.org_id LEFT JOIN inventory_cycle_counts c ON c.id=m.cycle_count_id AND c.org_id=m.org_id WHERE m.idempotency_key=$1"#).bind(key).fetch_optional(tx.as_mut()).await?;
    row.as_ref()
        .map(|row| Ok((movement_from_row(row)?, row.try_get("request_fingerprint")?)))
        .transpose()
}

async fn fetch_movement_tx(
    tx: &mut Transaction<'_, Postgres>,
    id: uuid::Uuid,
) -> Result<Option<InventoryMovementView>, PgInventoryError> {
    let row=sqlx::query(r#"SELECT m.id,m.item_id,i.iv_code,m.kind,m.quantity_delta_milli,m.quantity_before_milli,m.quantity_after_milli,NULL::uuid AS work_order_id,NULL::uuid AS dispatch_id,m.cycle_count_id,c.cc_code,m.source_ref,m.actor_id,m.occurred_at,m.memo FROM inventory_movements m JOIN inventory_items i ON i.id=m.item_id AND i.org_id=m.org_id LEFT JOIN inventory_cycle_counts c ON c.id=m.cycle_count_id AND c.org_id=m.org_id WHERE m.id=$1"#).bind(id).fetch_optional(tx.as_mut()).await?;
    row.as_ref().map(movement_from_row).transpose()
}

fn movement_from_row(
    row: &sqlx::postgres::PgRow,
) -> Result<InventoryMovementView, PgInventoryError> {
    let kind: String = row.try_get("kind")?;
    let kind = match kind.as_str() {
        "ISSUE" => MovementKind::Issue,
        "RECEIPT" => MovementKind::Receipt,
        "ADJUSTMENT" => MovementKind::Adjustment,
        _ => return Err(KernelError::validation("unknown inventory movement kind").into()),
    };
    // ISSUE is represented by the legacy consumption ledger; its negative signed delta is authoritative.
    let source =
        if let Some(work_order_id) = row.try_get::<Option<uuid::Uuid>, _>("work_order_id")? {
            if let Some(dispatch_id) = row.try_get::<Option<uuid::Uuid>, _>("dispatch_id")? {
                MovementSourceView::P1Dispatch {
                    dispatch_id: P1DispatchId::from_uuid(dispatch_id),
                    work_order_id: WorkOrderId::from_uuid(work_order_id),
                }
            } else {
                MovementSourceView::WorkOrder {
                    work_order_id: WorkOrderId::from_uuid(work_order_id),
                }
            }
        } else if let Some(count_id) = row.try_get::<Option<uuid::Uuid>, _>("cycle_count_id")? {
            MovementSourceView::CycleCount {
                cycle_count_id: count_id,
                cc_code: row.try_get("cc_code")?,
            }
        } else {
            MovementSourceView::ExternalRef {
                source_ref: row.try_get("source_ref")?,
            }
        };
    Ok(InventoryMovementView {
        id: row.try_get("id")?,
        item_id: InventoryItemId::from_uuid(row.try_get("item_id")?),
        iv_code: row.try_get("iv_code")?,
        kind,
        quantity_delta_milli: row.try_get("quantity_delta_milli")?,
        quantity_before_milli: row.try_get("quantity_before_milli")?,
        quantity_after_milli: row.try_get("quantity_after_milli")?,
        source,
        actor: UserId::from_uuid(row.try_get("actor_id")?),
        occurred_at: row.try_get("occurred_at")?,
        memo: row.try_get("memo")?,
    })
}

#[derive(Debug, Clone)]
struct LockedCycleCount {
    branch_id: BranchId,
    stock_location: InventoryStockLocationSummary,
    status: CycleCountStatus,
    version: i32,
}

async fn lock_cycle_count_tx(
    tx: &mut Transaction<'_, Postgres>,
    id: uuid::Uuid,
) -> Result<Option<LockedCycleCount>, PgInventoryError> {
    let row=sqlx::query("SELECT c.branch_id,c.status,c.version,c.stock_location_id,l.label AS stock_location_label FROM inventory_cycle_counts c JOIN inventory_stock_locations l ON l.id=c.stock_location_id AND l.org_id=c.org_id WHERE c.id=$1 FOR UPDATE OF c").bind(id).fetch_optional(tx.as_mut()).await?;
    row.as_ref()
        .map(|r| {
            Ok(LockedCycleCount {
                branch_id: BranchId::from_uuid(r.try_get("branch_id")?),
                stock_location: InventoryStockLocationSummary {
                    id: InventoryStockLocationId::from_uuid(r.try_get("stock_location_id")?),
                    label: r.try_get("stock_location_label")?,
                },
                status: CycleCountStatus::parse(&r.try_get::<String, _>("status")?)?,
                version: r.try_get("version")?,
            })
        })
        .transpose()
}

async fn fetch_cycle_detail_tx(
    tx: &mut Transaction<'_, Postgres>,
    id: uuid::Uuid,
) -> Result<Option<CycleCountDetail>, PgInventoryError> {
    let row=sqlx::query(r#"SELECT c.id,c.cc_code,c.branch_id,c.status,c.version,c.opened_by,c.submitted_by,c.submitted_at,c.decided_by,c.decided_at,c.decision_memo,c.created_at,c.updated_at,l.id AS stock_location_id,l.label AS stock_location_label,count(cl.id)::bigint AS line_count,count(cl.id) FILTER(WHERE cl.variance_milli<>0)::bigint AS variance_line_count FROM inventory_cycle_counts c JOIN inventory_stock_locations l ON l.id=c.stock_location_id AND l.org_id=c.org_id LEFT JOIN inventory_cycle_count_lines cl ON cl.count_id=c.id AND cl.org_id=c.org_id WHERE c.id=$1 GROUP BY c.id,l.id,l.label"#).bind(id).fetch_optional(tx.as_mut()).await?;
    let Some(row) = row else { return Ok(None) };
    let count = cycle_count_from_row(&row)?;
    let lines=sqlx::query("SELECT cl.id,cl.item_id,i.iv_code,i.display_name,i.unit_code,cl.system_quantity_milli,cl.counted_quantity_milli,cl.variance_milli,cl.reason,cl.note,cl.recorded_by,cl.recorded_at FROM inventory_cycle_count_lines cl JOIN inventory_items i ON i.id=cl.item_id AND i.org_id=cl.org_id WHERE cl.count_id=$1 ORDER BY i.iv_code").bind(id).fetch_all(tx.as_mut()).await?;
    let mut mapped = Vec::with_capacity(lines.len());
    for row in &lines {
        mapped.push(cycle_line_from_row(row)?)
    }
    let ids = sqlx::query_scalar(
        "SELECT id FROM inventory_movements WHERE cycle_count_id=$1 ORDER BY id",
    )
    .bind(id)
    .fetch_all(tx.as_mut())
    .await?;
    Ok(Some(CycleCountDetail {
        count,
        lines: mapped,
        applied_movement_ids: ids,
    }))
}

fn cycle_count_from_row(row: &sqlx::postgres::PgRow) -> Result<CycleCountView, PgInventoryError> {
    Ok(CycleCountView {
        id: row.try_get("id")?,
        cc_code: row.try_get("cc_code")?,
        branch_id: BranchId::from_uuid(row.try_get("branch_id")?),
        stock_location: InventoryStockLocationSummary {
            id: InventoryStockLocationId::from_uuid(row.try_get("stock_location_id")?),
            label: row.try_get("stock_location_label")?,
        },
        status: CycleCountStatus::parse(&row.try_get::<String, _>("status")?)?,
        version: row.try_get("version")?,
        opened_by: UserId::from_uuid(row.try_get("opened_by")?),
        submitted_by: row
            .try_get::<Option<uuid::Uuid>, _>("submitted_by")?
            .map(UserId::from_uuid),
        submitted_at: row.try_get("submitted_at")?,
        decided_by: row
            .try_get::<Option<uuid::Uuid>, _>("decided_by")?
            .map(UserId::from_uuid),
        decided_at: row.try_get("decided_at")?,
        decision_memo: row.try_get("decision_memo")?,
        line_count: row.try_get("line_count")?,
        variance_line_count: row.try_get("variance_line_count")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}
fn cycle_line_from_row(
    row: &sqlx::postgres::PgRow,
) -> Result<CycleCountLineView, PgInventoryError> {
    let reason: Option<String> = row.try_get("reason")?;
    Ok(CycleCountLineView {
        id: row.try_get("id")?,
        item_id: InventoryItemId::from_uuid(row.try_get("item_id")?),
        iv_code: row.try_get("iv_code")?,
        display_name: row.try_get("display_name")?,
        unit_code: row.try_get("unit_code")?,
        system_quantity_milli: row.try_get("system_quantity_milli")?,
        counted_quantity_milli: row.try_get("counted_quantity_milli")?,
        variance_milli: row.try_get("variance_milli")?,
        reason: reason.map(|x| VarianceReason::parse(&x)).transpose()?,
        note: row.try_get("note")?,
        recorded_by: UserId::from_uuid(row.try_get("recorded_by")?),
        recorded_at: row.try_get("recorded_at")?,
    })
}

#[allow(dead_code)]
fn _org_id_type_anchor(_: OrgId) {}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use time::UtcOffset;

    #[test]
    fn receipt_fingerprint_is_payload_stable_and_source_ref_is_fail_closed() {
        let item = InventoryItemId::from_uuid(uuid::Uuid::from_u128(11));
        let quantity = PositiveQuantityMilli::new(1_000).unwrap();
        assert_eq!(
            receipt_fingerprint(item, quantity, Some("PO-118"), None),
            receipt_fingerprint(item, quantity, Some("PO-118"), None)
        );
        assert_ne!(
            receipt_fingerprint(item, quantity, Some("PO-118"), None),
            receipt_fingerprint(item, quantity, Some("PO-119"), None)
        );
        assert_eq!(
            normalize_source_ref(Some("PO-118".to_owned()))
                .unwrap()
                .as_deref(),
            Some("PO-118")
        );
        assert!(normalize_source_ref(Some("po-118".to_owned())).is_err());
    }

    #[test]
    fn cycle_approval_fingerprint_covers_actor_memo_and_optimistic_payload() {
        let count = uuid::Uuid::from_u128(7);
        let actor = UserId::from_uuid(uuid::Uuid::from_u128(8));
        let same = cycle_approval_fingerprint(count, 4, actor, Some("verified"));
        assert_eq!(
            same,
            cycle_approval_fingerprint(count, 4, actor, Some("verified"))
        );
        assert_ne!(
            same,
            cycle_approval_fingerprint(count, 5, actor, Some("verified"))
        );
        assert_ne!(
            same,
            cycle_approval_fingerprint(
                count,
                4,
                UserId::from_uuid(uuid::Uuid::from_u128(9)),
                Some("verified")
            )
        );
        assert_ne!(
            same,
            cycle_approval_fingerprint(count, 4, actor, Some("amended"))
        );
    }

    #[test]
    fn occurrence_time_fingerprint_distinguishes_presence_and_canonicalizes_instants() {
        let item_id = InventoryItemId::from_uuid(uuid::Uuid::from_u128(1));
        let source = InventoryConsumptionSource::WorkOrder {
            work_order_id: WorkOrderId::from_uuid(uuid::Uuid::from_u128(2)),
        };
        let quantity = PositiveQuantityMilli::new(1).unwrap();
        let instant = time::OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap();
        let same_instant_with_offset = instant.to_offset(UtcOffset::from_hms(9, 0, 0).unwrap());

        let omitted = request_fingerprint(item_id, source, quantity, None, None);
        let explicit = request_fingerprint(item_id, source, quantity, None, Some(instant));
        let canonical = request_fingerprint(
            item_id,
            source,
            quantity,
            None,
            Some(same_instant_with_offset),
        );
        assert_ne!(omitted, explicit, "presence is part of the fingerprint");
        assert_eq!(explicit, canonical, "the same instant hashes canonically");
    }
}
