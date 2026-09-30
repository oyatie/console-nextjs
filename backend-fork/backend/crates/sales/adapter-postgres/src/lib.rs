//! Postgres adapter for the sales catalog (#6).
//!
//! Every read runs through a tenant-scoped transaction and every write through `with_audit`,
//! so the `app.current_org` GUC is armed and RLS scopes rows to the tenant
//! (the `console-gate-rls-arming` gate forbids bare-pool reads). All SQL is built
//! with `QueryBuilder` (runtime), so no `.sqlx` cache entries are needed.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::too_many_lines))]

use std::collections::HashMap;

use console_kernel_core::{CustomerInquiryId, EquipmentId, KernelError, SalesListingId};
use console_platform_db::{DbError, with_audit, with_audits, with_org_conn, with_org_snapshot};
use console_platform_request_context::current_org;
use console_sales_application::{
    CatalogQuery, CreateListingCommand, CustomerInquiryPage, CustomerInquiryView,
    DeleteListingCommand, InquiryInboxQuery, ListingMediaView, SalesListingPage, SalesListingView,
    SubmitInquiryCommand, UpdateInquiryStatusCommand, UpdateListingCommand, UpdateListingFields,
    inquiry_status_audit_event, inquiry_submit_audit_event, listing_create_audit_event,
    listing_delete_audit_event, listing_update_audit_event,
};
use console_sales_domain::{
    InquiryStatus, InquiryTopic, ListingCondition, ListingKind, ListingStatus, ListingType,
};
use sqlx::{PgPool, Postgres, QueryBuilder, Row, Transaction};
use time::OffsetDateTime;

#[derive(Debug, thiserror::Error)]
pub enum PgSalesError {
    #[error(transparent)]
    Db(#[from] DbError),
    #[error(transparent)]
    Domain(#[from] KernelError),
}

impl From<sqlx::Error> for PgSalesError {
    fn from(value: sqlx::Error) -> Self {
        Self::Db(DbError::Sqlx(value))
    }
}

#[derive(Debug, Clone)]
pub struct PgSalesStore {
    pool: PgPool,
}

const LISTING_COLUMNS: &str = "id, equipment_id, kind, condition, model_name, capacity_milli, \
     model_year, usage_hours, price_won, badge, usage_label, condition_label, availability, \
     location, description, listing_type, status, sort_weight, created_at, updated_at";

impl PgSalesStore {
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    #[must_use]
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    // ── Catalog reads ─────────────────────────────────────────────────────────

    /// Read the catalog, tenant-scoped + filtered. The public storefront leaves
    /// `include_non_public` false so only published/reserved listings surface;
    /// the admin console sets it true to see drafts/sold/withdrawn too.
    pub async fn list_listings(
        &self,
        query: CatalogQuery,
    ) -> Result<SalesListingPage, PgSalesError> {
        let mut count = QueryBuilder::<Postgres>::new("SELECT COUNT(*) FROM sales_listings WHERE ");
        push_listing_filters(&mut count, &query);
        let mut builder = QueryBuilder::<Postgres>::new("SELECT ");
        builder.push(LISTING_COLUMNS);
        builder.push(" FROM sales_listings WHERE ");
        push_listing_filters(&mut builder, &query);
        builder.push(" ORDER BY sort_weight DESC, created_at DESC, id DESC LIMIT ");
        builder.push_bind(query.limit);
        builder.push(" OFFSET ");
        builder.push_bind(query.offset);

        let org = current_org().map_err(KernelError::from)?;
        with_org_snapshot::<_, _, PgSalesError>(&self.pool, org, move |tx| {
            Box::pin(async move {
                let total: i64 = count.build_query_scalar().fetch_one(tx.as_mut()).await?;
                let rows = builder.build().fetch_all(tx.as_mut()).await?;
                let mut items = Vec::with_capacity(rows.len());
                let mut ids = Vec::with_capacity(rows.len());
                for row in &rows {
                    let view = listing_from_row(row)?;
                    ids.push(*view.id.as_uuid());
                    items.push(view);
                }
                let media = listing_media_tx(tx, ids).await?;
                for view in &mut items {
                    if let Some(list) = media.get(view.id.as_uuid()) {
                        view.media = list.clone();
                    }
                }
                Ok(SalesListingPage {
                    items,
                    limit: query.limit,
                    offset: query.offset,
                    total,
                })
            })
        })
        .await
    }

    /// Read a single listing (with media), tenant-scoped. `include_non_public`
    /// gates whether a non-published listing is visible (public detail 404s it).
    pub async fn get_listing(
        &self,
        listing_id: SalesListingId,
        include_non_public: bool,
    ) -> Result<Option<SalesListingView>, PgSalesError> {
        let id = *listing_id.as_uuid();
        let mut builder = QueryBuilder::<Postgres>::new("SELECT ");
        builder.push(LISTING_COLUMNS);
        builder.push(" FROM sales_listings WHERE id = ");
        builder.push_bind(id);
        if !include_non_public {
            builder.push(" AND status IN ('PUBLISHED', 'RESERVED')");
        }
        let org = current_org().map_err(KernelError::from)?;
        with_org_snapshot::<_, _, PgSalesError>(&self.pool, org, move |tx| {
            Box::pin(async move {
                let row = builder.build().fetch_optional(tx.as_mut()).await?;
                let Some(row) = row else { return Ok(None) };
                let mut view = listing_from_row(&row)?;
                let media = listing_media_tx(tx, vec![id]).await?;
                if let Some(list) = media.get(&id) {
                    view.media = list.clone();
                }
                Ok(Some(view))
            })
        })
        .await
    }

    /// Resolve one public listing's media object location for the storefront
    /// serve route: returns `(s3_key, content_type)` for `media_id` only when it
    /// belongs to `listing_id` AND that listing is storefront-visible
    /// (published/reserved). Tenant-scoped via `with_org_conn`, so RLS is armed.
    pub async fn public_media_object(
        &self,
        listing_id: SalesListingId,
        media_id: uuid::Uuid,
    ) -> Result<Option<(String, String)>, PgSalesError> {
        let listing_uuid = *listing_id.as_uuid();
        let org = current_org().map_err(KernelError::from)?;
        let row = with_org_conn::<_, _, PgSalesError>(&self.pool, org, move |tx| {
            Box::pin(async move {
                Ok(sqlx::query(
                    "SELECT m.s3_key, m.content_type \
                     FROM sales_listing_media m \
                     JOIN sales_listings l ON l.id = m.listing_id AND l.org_id = m.org_id \
                     WHERE m.id = $1 AND m.listing_id = $2 \
                       AND l.status IN ('PUBLISHED', 'RESERVED')",
                )
                .bind(media_id)
                .bind(listing_uuid)
                .fetch_optional(tx.as_mut())
                .await?)
            })
        })
        .await?;
        let Some(row) = row else { return Ok(None) };
        Ok(Some((row.try_get("s3_key")?, row.try_get("content_type")?)))
    }

    // ── Admin listing writes (audited) ────────────────────────────────────────

    /// Create a listing. Audited; org-armed so RLS WITH CHECK passes.
    // console-gate: state-changing-handler
    pub async fn create_listing(&self, command: CreateListingCommand) -> Result<(), PgSalesError> {
        let org = current_org().map_err(KernelError::from)?;
        let org_uuid = *org.as_uuid();
        let listing_uuid = *command.listing_id.as_uuid();
        let input = command.input;
        let after = listing_input_snapshot(&input);
        let event = listing_create_audit_event(
            command.actor,
            command.listing_id,
            after,
            command.trace,
            command.occurred_at,
        )?
        .with_org(org);

        with_audit::<_, (), PgSalesError>(&self.pool, event, move |tx| {
            Box::pin(async move {
                let mut b = QueryBuilder::<Postgres>::new(
                    "INSERT INTO sales_listings (id, org_id, equipment_id, kind, condition, \
                     model_name, capacity_milli, model_year, usage_hours, price_won, badge, \
                     usage_label, condition_label, availability, location, description, \
                     listing_type, status, sort_weight) VALUES (",
                );
                let mut sep = b.separated(", ");
                sep.push_bind(listing_uuid);
                sep.push_bind(org_uuid);
                sep.push_bind(input.equipment_id.map(|e| *e.as_uuid()));
                sep.push_bind(input.kind.as_db_str());
                sep.push_bind(input.condition.as_db_str());
                sep.push_bind(input.model_name);
                sep.push_bind(input.capacity_milli);
                sep.push_bind(input.model_year);
                sep.push_bind(input.usage_hours);
                sep.push_bind(input.price_won);
                sep.push_bind(input.badge);
                sep.push_bind(input.usage_label);
                sep.push_bind(input.condition_label);
                sep.push_bind(input.availability);
                sep.push_bind(input.location);
                sep.push_bind(input.description);
                sep.push_bind(input.listing_type.as_db_str());
                sep.push_bind(input.status.as_db_str());
                sep.push_bind(input.sort_weight);
                b.push(")");
                b.build().execute(tx.as_mut()).await?;
                Ok(())
            })
        })
        .await
    }

    /// Update a listing (partial; double-option clears nullable columns).
    // console-gate: state-changing-handler
    pub async fn update_listing(&self, command: UpdateListingCommand) -> Result<(), PgSalesError> {
        if command.fields.is_empty() {
            return Err(KernelError::validation("no listing fields to update").into());
        }
        let org = current_org().map_err(KernelError::from)?;
        let existing = self
            .get_listing(command.listing_id, true)
            .await?
            .ok_or_else(|| KernelError::not_found("listing was not found"))?;
        let before = listing_view_snapshot(&existing);
        let after = listing_after_snapshot(&existing, &command.fields);
        let event = listing_update_audit_event(
            command.actor,
            command.listing_id,
            before,
            after,
            command.trace,
            command.occurred_at,
        )?
        .with_org(org);
        let listing_uuid = *command.listing_id.as_uuid();
        let fields = command.fields;

        with_audit::<_, (), PgSalesError>(&self.pool, event, move |tx| {
            Box::pin(async move {
                let mut b = QueryBuilder::<Postgres>::new("UPDATE sales_listings SET ");
                let mut sep = b.separated(", ");
                push_listing_assignments(&mut sep, &fields);
                sep.push_unseparated(", updated_at = now()");
                b.push(" WHERE id = ");
                b.push_bind(listing_uuid);
                b.build().execute(tx.as_mut()).await?;
                Ok(())
            })
        })
        .await
    }

    /// Archive a listing by moving it to WITHDRAWN (soft delete). The platform
    /// convention is soft archival, never a hard DELETE (audit gap 23): a
    /// disposed listing must stay in the object graph so its code never dangles
    /// and its history stays reconstructable. The media rows are preserved with
    /// it. WITHDRAWN is already excluded from the public catalog, so the
    /// endpoint's effect is unchanged for consumers.
    ///
    /// No-op guard: a listing already WITHDRAWN is left untouched and audits
    /// nothing — a repeat/retried delete call must not write an identical
    /// before==after audit event every time.
    // console-gate: state-changing-handler
    pub async fn delete_listing(&self, command: DeleteListingCommand) -> Result<(), PgSalesError> {
        let org = current_org().map_err(KernelError::from)?;
        let existing = self
            .get_listing(command.listing_id, true)
            .await?
            .ok_or_else(|| KernelError::not_found("listing was not found"))?;
        if existing.status == ListingStatus::Withdrawn {
            return Ok(());
        }
        let before = listing_view_snapshot(&existing);
        let mut after = before.clone();
        if let Some(obj) = after.as_object_mut() {
            obj.insert("status".into(), ListingStatus::Withdrawn.as_db_str().into());
        }
        let event = listing_delete_audit_event(
            command.actor,
            command.listing_id,
            before,
            after,
            command.trace,
            command.occurred_at,
        )?
        .with_org(org);
        let listing_uuid = *command.listing_id.as_uuid();

        with_audit::<_, (), PgSalesError>(&self.pool, event, move |tx| {
            Box::pin(async move {
                sqlx::query(
                    "UPDATE sales_listings SET status = $2, updated_at = now() WHERE id = $1",
                )
                .bind(listing_uuid)
                .bind(ListingStatus::Withdrawn.as_db_str())
                .execute(tx.as_mut())
                .await?;
                Ok(())
            })
        })
        .await
    }

    // ── Inquiries ─────────────────────────────────────────────────────────────

    /// Record a public inquiry. No actor (public submit); the audit snapshot is
    /// PII-light (topic/listing/status only — never the name/phone/message).
    // console-gate: state-changing-handler
    pub async fn submit_inquiry(&self, command: SubmitInquiryCommand) -> Result<(), PgSalesError> {
        let org = current_org().map_err(KernelError::from)?;
        let org_uuid = *org.as_uuid();
        let inquiry_uuid = *command.inquiry_id.as_uuid();
        let after = serde_json::json!({
            "topic": command.topic.as_db_str(),
            "listing_id": command.listing_id.map(|l| l.as_uuid().to_string()),
            "status": InquiryStatus::New.as_db_str(),
        });
        let event = inquiry_submit_audit_event(
            command.inquiry_id,
            after,
            command.trace,
            command.occurred_at,
        )?
        .with_org(org);
        let SubmitInquiryCommand {
            name,
            phone,
            topic,
            location,
            message,
            listing_id,
            ..
        } = command;

        with_audit::<_, (), PgSalesError>(&self.pool, event, move |tx| {
            Box::pin(async move {
                if let Some(listing_id) = listing_id {
                    // Hold a share lock through the audited insert so a
                    // concurrent withdrawal cannot change the chosen object.
                    let public: Option<i32> = sqlx::query_scalar(
                        "SELECT 1 FROM sales_listings \
                         WHERE id = $1 AND org_id = $2 \
                           AND status IN ('PUBLISHED', 'RESERVED') FOR SHARE",
                    )
                    .bind(*listing_id.as_uuid())
                    .bind(org_uuid)
                    .fetch_optional(tx.as_mut())
                    .await?;
                    if public.is_none() {
                        return Err(KernelError::conflict(
                            "listing is no longer available for this inquiry",
                        )
                        .into());
                    }
                }
                let mut b = QueryBuilder::<Postgres>::new(
                    "INSERT INTO customer_inquiries (id, org_id, name, phone, topic, location, \
                     message, listing_id, status) VALUES (",
                );
                let mut sep = b.separated(", ");
                sep.push_bind(inquiry_uuid);
                sep.push_bind(org_uuid);
                sep.push_bind(name);
                sep.push_bind(phone);
                sep.push_bind(topic.as_db_str());
                sep.push_bind(location);
                sep.push_bind(message);
                sep.push_bind(listing_id.map(|l| *l.as_uuid()));
                sep.push_bind(InquiryStatus::New.as_db_str());
                b.push(")");
                b.build().execute(tx.as_mut()).await?;
                Ok(())
            })
        })
        .await
    }

    /// Read the inquiry inbox, tenant-scoped + status-filtered, newest first.
    pub async fn list_inquiries(
        &self,
        query: InquiryInboxQuery,
    ) -> Result<CustomerInquiryPage, PgSalesError> {
        let total = self.count_inquiries(&query).await?;
        let mut builder = QueryBuilder::<Postgres>::new(
            "SELECT id, name, phone, topic, location, message, listing_id, status, created_at, \
             updated_at FROM customer_inquiries WHERE ",
        );
        push_inquiry_filters(&mut builder, &query);
        builder.push(" ORDER BY created_at DESC LIMIT ");
        builder.push_bind(query.limit);
        builder.push(" OFFSET ");
        builder.push_bind(query.offset);
        let org = current_org().map_err(KernelError::from)?;
        let rows = with_org_conn::<_, _, PgSalesError>(&self.pool, org, move |tx| {
            Box::pin(async move { Ok(builder.build().fetch_all(tx.as_mut()).await?) })
        })
        .await?;
        let mut items = Vec::with_capacity(rows.len());
        for row in &rows {
            items.push(inquiry_from_row(row)?);
        }
        Ok(CustomerInquiryPage {
            items,
            limit: query.limit,
            offset: query.offset,
            total,
        })
    }

    async fn count_inquiries(&self, query: &InquiryInboxQuery) -> Result<i64, PgSalesError> {
        let mut builder =
            QueryBuilder::<Postgres>::new("SELECT COUNT(*) FROM customer_inquiries WHERE ");
        push_inquiry_filters(&mut builder, query);
        let org = current_org().map_err(KernelError::from)?;
        let total: i64 = with_org_conn::<_, _, PgSalesError>(&self.pool, org, move |tx| {
            Box::pin(async move { Ok(builder.build_query_scalar().fetch_one(tx.as_mut()).await?) })
        })
        .await?;
        Ok(total)
    }

    /// Triage an inquiry (NEW → CONTACTED → CLOSED). Audited.
    // console-gate: state-changing-handler
    pub async fn update_inquiry_status(
        &self,
        command: UpdateInquiryStatusCommand,
    ) -> Result<(), PgSalesError> {
        let org = current_org().map_err(KernelError::from)?;
        let inquiry_uuid = *command.inquiry_id.as_uuid();
        let new_status = command.status;
        let actor = command.actor;
        let inquiry_id = command.inquiry_id;
        let trace = command.trace;
        let occurred_at = command.occurred_at;

        // Read the prior status with `FOR UPDATE` and apply the UPDATE in ONE
        // transaction, so the audit before-snapshot is consistent with the row
        // the UPDATE mutates (no read/write skew between two connections). The
        // audit event is computed from the locked row, so `with_audits` is the
        // right primitive.
        with_audits::<_, (), PgSalesError>(&self.pool, org, move |tx| {
            Box::pin(async move {
                let before_status: Option<String> = sqlx::query_scalar::<_, String>(
                    "SELECT status FROM customer_inquiries WHERE id = $1 FOR UPDATE",
                )
                .bind(inquiry_uuid)
                .fetch_optional(tx.as_mut())
                .await?;
                let before_status =
                    before_status.ok_or_else(|| KernelError::not_found("inquiry was not found"))?;

                sqlx::query(
                    "UPDATE customer_inquiries SET status = $2, updated_at = now() WHERE id = $1",
                )
                .bind(inquiry_uuid)
                .bind(new_status.as_db_str())
                .execute(tx.as_mut())
                .await?;

                let before = serde_json::json!({ "status": before_status });
                let after = serde_json::json!({ "status": new_status.as_db_str() });
                let event = inquiry_status_audit_event(
                    actor,
                    inquiry_id,
                    before,
                    after,
                    trace,
                    occurred_at,
                )?
                .with_org(org);
                Ok(((), vec![event]))
            })
        })
        .await
    }

    /// Atomically increment (or insert) the fixed-window rate-limit counter for
    /// one bucket and return the new attempt count. Shares the `auth_rate_limit`
    /// table and the same UPSERT semantics the auth/support endpoints use; the
    /// `endpoint` key (e.g. `sales_inquiry`) isolates the sales buckets.
    ///
    /// This is a coarse counter, not an audited state change — it deliberately
    /// lives in the adapter (not a REST handler surface) so it is exempt from the
    /// audit-coverage gate, exactly as the auth/support crates' identical counter
    /// is.
    pub async fn increment_rate_bucket(
        &self,
        client_key: &str,
        endpoint: &str,
        window_start: OffsetDateTime,
    ) -> Result<i64, PgSalesError> {
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
}

/// Build the public storefront serve path for one listing photo. Stable and
/// id-based so it can be cached/CDN-fronted; the route streams the object bytes
/// from the store after re-checking the listing's storefront visibility.
fn storefront_media_url(listing_id: uuid::Uuid, media_id: uuid::Uuid) -> String {
    format!("/api/v1/storefront/listings/{listing_id}/media/{media_id}")
}

async fn listing_media_tx(
    tx: &mut Transaction<'_, Postgres>,
    listing_ids: Vec<uuid::Uuid>,
) -> Result<HashMap<uuid::Uuid, Vec<ListingMediaView>>, PgSalesError> {
    if listing_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let mut builder = QueryBuilder::<Postgres>::new(
        "SELECT id, listing_id, content_type, alt_text, sort_order \
         FROM sales_listing_media WHERE listing_id = ANY(",
    );
    builder.push_bind(listing_ids);
    builder.push(") ORDER BY listing_id, sort_order");
    let rows = builder.build().fetch_all(tx.as_mut()).await?;
    let mut out: HashMap<uuid::Uuid, Vec<ListingMediaView>> = HashMap::new();
    for row in rows {
        let listing_id: uuid::Uuid = row.try_get("listing_id")?;
        let id: uuid::Uuid = row.try_get("id")?;
        out.entry(listing_id).or_default().push(ListingMediaView {
            id: id.to_string(),
            url: storefront_media_url(listing_id, id),
            content_type: row.try_get("content_type")?,
            alt_text: row.try_get("alt_text")?,
            sort_order: row.try_get("sort_order")?,
        });
    }
    Ok(out)
}

// ── Row mapping + filters ────────────────────────────────────────────────────

fn listing_from_row(row: &sqlx::postgres::PgRow) -> Result<SalesListingView, PgSalesError> {
    let kind: String = row.try_get("kind")?;
    let condition: String = row.try_get("condition")?;
    let listing_type: String = row.try_get("listing_type")?;
    let status: String = row.try_get("status")?;
    let equipment_id: Option<uuid::Uuid> = row.try_get("equipment_id")?;
    Ok(SalesListingView {
        id: SalesListingId::from_uuid(row.try_get("id")?),
        equipment_id: equipment_id.map(EquipmentId::from_uuid),
        kind: ListingKind::parse(&kind)?,
        condition: ListingCondition::parse(&condition)?,
        model_name: row.try_get("model_name")?,
        capacity_milli: row.try_get("capacity_milli")?,
        model_year: row.try_get("model_year")?,
        usage_hours: row.try_get("usage_hours")?,
        price_won: row.try_get("price_won")?,
        badge: row.try_get("badge")?,
        usage_label: row.try_get("usage_label")?,
        condition_label: row.try_get("condition_label")?,
        availability: row.try_get("availability")?,
        location: row.try_get("location")?,
        description: row.try_get("description")?,
        listing_type: ListingType::parse(&listing_type)?,
        status: ListingStatus::parse(&status)?,
        sort_weight: row.try_get("sort_weight")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
        media: Vec::new(),
    })
}

fn inquiry_from_row(row: &sqlx::postgres::PgRow) -> Result<CustomerInquiryView, PgSalesError> {
    let topic: String = row.try_get("topic")?;
    let status: String = row.try_get("status")?;
    let listing_id: Option<uuid::Uuid> = row.try_get("listing_id")?;
    Ok(CustomerInquiryView {
        id: CustomerInquiryId::from_uuid(row.try_get("id")?),
        name: row.try_get("name")?,
        phone: row.try_get("phone")?,
        topic: InquiryTopic::parse(&topic)?,
        location: row.try_get("location")?,
        message: row.try_get("message")?,
        listing_id: listing_id.map(SalesListingId::from_uuid),
        status: InquiryStatus::parse(&status)?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

fn push_listing_filters(builder: &mut QueryBuilder<Postgres>, query: &CatalogQuery) {
    if query.include_non_public {
        builder.push("TRUE");
    } else {
        builder.push("status IN ('PUBLISHED', 'RESERVED')");
    }
    if let Some(kind) = query.kind {
        builder.push(" AND kind = ");
        builder.push_bind(kind.as_db_str());
    }
    if let Some(condition) = query.condition {
        builder.push(" AND condition = ");
        builder.push_bind(condition.as_db_str());
    }
    if let Some(listing_type) = query.listing_type {
        builder.push(" AND listing_type = ");
        builder.push_bind(listing_type.as_db_str());
    }
}

fn push_inquiry_filters(builder: &mut QueryBuilder<Postgres>, query: &InquiryInboxQuery) {
    builder.push("TRUE");
    if let Some(status) = query.status {
        builder.push(" AND status = ");
        builder.push_bind(status.as_db_str());
    }
}

fn push_listing_assignments(
    sep: &mut sqlx::query_builder::Separated<'_, Postgres, &'static str>,
    fields: &UpdateListingFields,
) {
    if let Some(kind) = fields.kind {
        sep.push("kind = ");
        sep.push_bind_unseparated(kind.as_db_str());
    }
    if let Some(condition) = fields.condition {
        sep.push("condition = ");
        sep.push_bind_unseparated(condition.as_db_str());
    }
    if let Some(model_name) = &fields.model_name {
        sep.push("model_name = ");
        sep.push_bind_unseparated(model_name.clone());
    }
    if let Some(listing_type) = fields.listing_type {
        sep.push("listing_type = ");
        sep.push_bind_unseparated(listing_type.as_db_str());
    }
    if let Some(status) = fields.status {
        sep.push("status = ");
        sep.push_bind_unseparated(status.as_db_str());
    }
    if let Some(sort_weight) = fields.sort_weight {
        sep.push("sort_weight = ");
        sep.push_bind_unseparated(sort_weight);
    }
    if let Some(equipment_id) = &fields.equipment_id {
        sep.push("equipment_id = ");
        sep.push_bind_unseparated(equipment_id.map(|e| *e.as_uuid()));
    }
    push_opt_i64(sep, "capacity_milli", &fields.capacity_milli);
    push_opt_i32(sep, "model_year", &fields.model_year);
    push_opt_i32(sep, "usage_hours", &fields.usage_hours);
    push_opt_i64(sep, "price_won", &fields.price_won);
    push_opt_text(sep, "badge", &fields.badge);
    push_opt_text(sep, "usage_label", &fields.usage_label);
    push_opt_text(sep, "condition_label", &fields.condition_label);
    push_opt_text(sep, "availability", &fields.availability);
    push_opt_text(sep, "location", &fields.location);
    push_opt_text(sep, "description", &fields.description);
}

fn push_opt_text(
    sep: &mut sqlx::query_builder::Separated<'_, Postgres, &'static str>,
    column: &str,
    change: &Option<Option<String>>,
) {
    if let Some(value) = change {
        sep.push(format!("{column} = "));
        sep.push_bind_unseparated(value.clone());
    }
}

fn push_opt_i64(
    sep: &mut sqlx::query_builder::Separated<'_, Postgres, &'static str>,
    column: &str,
    change: &Option<Option<i64>>,
) {
    if let Some(value) = change {
        sep.push(format!("{column} = "));
        sep.push_bind_unseparated(*value);
    }
}

fn push_opt_i32(
    sep: &mut sqlx::query_builder::Separated<'_, Postgres, &'static str>,
    column: &str,
    change: &Option<Option<i32>>,
) {
    if let Some(value) = change {
        sep.push(format!("{column} = "));
        sep.push_bind_unseparated(*value);
    }
}

// ── Audit snapshots (sales listings are not PII; inquiries kept PII-light) ────

fn listing_input_snapshot(input: &console_sales_application::ListingInput) -> serde_json::Value {
    serde_json::json!({
        "kind": input.kind.as_db_str(),
        "condition": input.condition.as_db_str(),
        "model_name": input.model_name,
        "capacity_milli": input.capacity_milli,
        "model_year": input.model_year,
        "usage_hours": input.usage_hours,
        "price_won": input.price_won,
        "listing_type": input.listing_type.as_db_str(),
        "status": input.status.as_db_str(),
        "sort_weight": input.sort_weight,
    })
}

fn listing_view_snapshot(view: &SalesListingView) -> serde_json::Value {
    serde_json::json!({
        "kind": view.kind.as_db_str(),
        "condition": view.condition.as_db_str(),
        "model_name": view.model_name,
        "capacity_milli": view.capacity_milli,
        "model_year": view.model_year,
        "usage_hours": view.usage_hours,
        "price_won": view.price_won,
        "listing_type": view.listing_type.as_db_str(),
        "status": view.status.as_db_str(),
        "sort_weight": view.sort_weight,
    })
}

fn listing_after_snapshot(
    existing: &SalesListingView,
    fields: &UpdateListingFields,
) -> serde_json::Value {
    let mut snap = listing_view_snapshot(existing);
    if let Some(obj) = snap.as_object_mut() {
        if let Some(kind) = fields.kind {
            obj.insert("kind".into(), kind.as_db_str().into());
        }
        if let Some(condition) = fields.condition {
            obj.insert("condition".into(), condition.as_db_str().into());
        }
        if let Some(model_name) = &fields.model_name {
            obj.insert("model_name".into(), model_name.clone().into());
        }
        if let Some(listing_type) = fields.listing_type {
            obj.insert("listing_type".into(), listing_type.as_db_str().into());
        }
        if let Some(status) = fields.status {
            obj.insert("status".into(), status.as_db_str().into());
        }
        if let Some(sort_weight) = fields.sort_weight {
            obj.insert("sort_weight".into(), sort_weight.into());
        }
        if let Some(price) = &fields.price_won {
            obj.insert("price_won".into(), (*price).into());
        }
    }
    snap
}
