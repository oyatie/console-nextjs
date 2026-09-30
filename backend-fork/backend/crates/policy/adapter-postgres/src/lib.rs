//! Postgres adapter for Cedar policy catalog rows and draft staging.
//!
//! Reads always go through `with_org_conn`; draft mutations always go through
//! `with_audit`, which arms `app.current_org` from the audit event before SQL
//! runs. Draft saves create reviewable staging rows only and never write runtime
//! `policy_versions`, `shadow`, or `enforced` state.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

use console_kernel_core::{ErrorKind, KernelError, OrgId, UserId};
use console_platform_db::{DbError, with_audit, with_org_conn};
use console_platform_request_context::current_org;
use console_policy_application::{
    CedarPolicyCatalogPage, CedarPolicyCatalogQuery, CedarPolicyDraftSaveCommand,
    CedarPolicyDraftSaveResponse, EnforcementEffect, build_draft_artifact,
    draft_create_audit_event,
};
use console_policy_domain::{
    CedarPolicyBlocks, CedarPolicyCatalogRow, CedarPolicyDraft, CedarPolicyEffect,
    CedarPolicyReviewStatus, CedarPolicySource, CedarPolicyStatus, CedarValidationError,
    CedarValidationStatus,
};
use sqlx::{PgPool, Postgres, QueryBuilder, Row, postgres::PgRow};

#[derive(Debug, thiserror::Error)]
pub enum PgPolicyError {
    #[error(transparent)]
    Db(#[from] DbError),
    #[error(transparent)]
    Domain(#[from] KernelError),
}

impl From<sqlx::Error> for PgPolicyError {
    fn from(value: sqlx::Error) -> Self {
        Self::Db(DbError::Sqlx(value))
    }
}

impl PgPolicyError {
    #[must_use]
    pub const fn kind(&self) -> ErrorKind {
        match self {
            Self::Domain(error) => error.kind,
            Self::Db(DbError::Sqlx(sqlx::Error::RowNotFound)) => ErrorKind::NotFound,
            Self::Db(DbError::Sqlx(_))
            | Self::Db(DbError::Serialize(_))
            | Self::Db(DbError::CodeIssuance(_)) => ErrorKind::Internal,
        }
    }
}

#[derive(Debug, Clone)]
pub struct PgPolicyStore {
    pool: PgPool,
}

impl PgPolicyStore {
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    #[must_use]
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    pub async fn list_catalog_rows(
        &self,
        query: CedarPolicyCatalogQuery,
    ) -> Result<CedarPolicyCatalogPage, PgPolicyError> {
        let query = query.normalized()?;
        let total = self.count_catalog_rows(&query).await?;
        let mut builder = catalog_union_builder("SELECT * FROM (");
        builder.push(") rows WHERE ");
        push_catalog_filters(&mut builder, &query);
        builder.push(" ORDER BY updated_at DESC, id DESC LIMIT ");
        builder.push_bind(query.limit_value());
        builder.push(" OFFSET ");
        builder.push_bind(query.offset_value());

        let org = current_org().map_err(KernelError::from)?;
        let rows = with_org_conn::<_, _, PgPolicyError>(&self.pool, org, move |tx| {
            Box::pin(async move { Ok(builder.build().fetch_all(tx.as_mut()).await?) })
        })
        .await?;

        let mut items = Vec::with_capacity(rows.len());
        for row in rows {
            items.push(catalog_row_from_row(&row)?);
        }
        Ok(CedarPolicyCatalogPage {
            items,
            limit: query.limit_value(),
            offset: query.offset_value(),
            total,
        })
    }

    async fn count_catalog_rows(
        &self,
        query: &CedarPolicyCatalogQuery,
    ) -> Result<i64, PgPolicyError> {
        let mut builder = catalog_union_builder("SELECT COUNT(*) FROM (");
        builder.push(") rows WHERE ");
        push_catalog_filters(&mut builder, query);
        let org = current_org().map_err(KernelError::from)?;
        let total = with_org_conn::<_, _, PgPolicyError>(&self.pool, org, move |tx| {
            Box::pin(async move {
                Ok(builder
                    .build_query_scalar::<i64>()
                    .fetch_one(tx.as_mut())
                    .await?)
            })
        })
        .await?;
        Ok(total)
    }

    /// Persist a no-code draft as reviewable staging data only.
    ///
    /// The org is read from the bound request context, not from client input.
    /// `with_audit` arms the same org GUC before the INSERT, so FORCE RLS and
    /// same-org FKs are exercised by the runtime role.
    // console-gate: state-changing-handler
    pub async fn save_draft(
        &self,
        command: CedarPolicyDraftSaveCommand,
    ) -> Result<CedarPolicyDraftSaveResponse, PgPolicyError> {
        let org = current_org().map_err(KernelError::from)?;
        let actor = command.actor;
        let trace = command.trace.clone();
        let occurred_at = command.occurred_at;
        let audit_trace_id = command.trace.trace_id().to_owned();
        let draft = build_draft_artifact(org, command)?;
        let event = draft_create_audit_event(actor, trace, occurred_at, &draft)?.with_org(org);
        let returned = draft.clone();

        with_audit::<_, CedarPolicyDraft, PgPolicyError>(&self.pool, event, move |tx| {
            Box::pin(async move {
                let org_uuid = *returned.org_id.as_uuid();
                let actor_uuid = *actor.as_uuid();
                let normalized_row =
                    serde_json::to_value(&returned.catalog_row).map_err(|err| {
                        KernelError::internal(format!(
                            "failed to serialize Cedar normalized row: {err}"
                        ))
                    })?;
                let blocks = serde_json::to_value(&returned.blocks).map_err(|err| {
                    KernelError::internal(format!("failed to serialize Cedar draft blocks: {err}"))
                })?;
                let validation_errors =
                    serde_json::to_value(&returned.validation_errors).map_err(|err| {
                        KernelError::internal(format!(
                            "failed to serialize Cedar validation errors: {err}"
                        ))
                    })?;

                sqlx::query(
                    r#"
                    INSERT INTO cedar_policy_drafts (
                        id, org_id, draft_key, title, author_note, blocks,
                        normalized_row, generated_policy_text, generated_policy_digest,
                        validation_status, validation_errors, review_status,
                        reviewer_id, review_note, created_by, updated_by, created_at, updated_at
                    ) VALUES (
                        $1, $2, $3, $4, $5, $6,
                        $7, $8, $9,
                        $10, $11, $12,
                        $13, $14, $15, $16, $17, $18
                    )
                    "#,
                )
                .bind(returned.id)
                .bind(org_uuid)
                .bind(&returned.draft_key)
                .bind(&returned.title)
                .bind(&returned.author_note)
                .bind(blocks)
                .bind(normalized_row)
                .bind(&returned.generated_policy_text)
                .bind(&returned.generated_policy_digest)
                .bind(returned.validation_status.as_db_str())
                .bind(validation_errors)
                .bind(returned.review_status.as_db_str())
                .bind(returned.reviewer_id.map(|id| *id.as_uuid()))
                .bind(&returned.review_note)
                .bind(actor_uuid)
                .bind(actor_uuid)
                .bind(returned.created_at)
                .bind(returned.updated_at)
                .execute(tx.as_mut())
                .await?;

                Ok(returned)
            })
        })
        .await?;

        Ok(CedarPolicyDraftSaveResponse {
            draft,
            enforcement_effect: EnforcementEffect::None,
            audit_trace_id,
            next_actions: vec!["simulate".to_owned(), "submit_for_review".to_owned()],
        })
    }
}

fn catalog_union_builder(prefix: &str) -> QueryBuilder<Postgres> {
    let mut builder = QueryBuilder::<Postgres>::new(prefix);
    builder.push(
        r#"
        SELECT id, stable_key, title, natural_language_rule, effect, status, source,
               principal, action, resource, conditions, engine_mode, policy_version,
               schema_version, bundle_digest, cedar_sdk_version, cedar_language_version,
               validation_status, created_by, updated_by, created_at, updated_at
        FROM cedar_policy_catalog_entries
        UNION ALL
        SELECT id,
               draft_key AS stable_key,
               title,
               (normalized_row->>'natural_language_rule') AS natural_language_rule,
               (normalized_row->>'effect') AS effect,
               CASE review_status
                   WHEN 'draft' THEN 'draft'
                   WHEN 'review_pending' THEN 'review_pending'
                   WHEN 'approved_for_promotion' THEN 'review_pending'
                   WHEN 'rejected' THEN 'rejected'
               END AS status,
               'no_code_draft' AS source,
               normalized_row->'principal' AS principal,
               normalized_row->'action' AS action,
               normalized_row->'resource' AS resource,
               COALESCE(normalized_row->'conditions', '[]'::jsonb) AS conditions,
               NULL::text AS engine_mode,
               NULL::bigint AS policy_version,
               NULL::text AS schema_version,
               NULL::text AS bundle_digest,
               NULL::text AS cedar_sdk_version,
               NULL::text AS cedar_language_version,
               validation_status,
               created_by,
               updated_by,
               created_at,
               updated_at
        FROM cedar_policy_drafts
        "#,
    );
    builder
}

fn push_catalog_filters(builder: &mut QueryBuilder<Postgres>, query: &CedarPolicyCatalogQuery) {
    builder.push("TRUE");
    if let Some(status) = query.status {
        builder.push(" AND status = ");
        builder.push_bind(status.as_db_str());
    }
    if let Some(source) = query.source {
        builder.push(" AND source = ");
        builder.push_bind(source.as_db_str());
    }
    if let Some(effect) = query.effect {
        builder.push(" AND effect = ");
        builder.push_bind(effect.as_db_str());
    }
    if let Some(resource_type) = &query.resource_type {
        builder.push(" AND resource->>'resource_type' = ");
        builder.push_bind(resource_type);
    }
    if let Some(action_key) = &query.action_key {
        builder.push(" AND action->>'action_key' = ");
        builder.push_bind(action_key);
    }
}

fn catalog_row_from_row(row: &PgRow) -> Result<CedarPolicyCatalogRow, PgPolicyError> {
    let id: uuid::Uuid = row.try_get("id")?;
    let effect: String = row.try_get("effect")?;
    let status: String = row.try_get("status")?;
    let source: String = row.try_get("source")?;
    let validation_status: String = row.try_get("validation_status")?;
    let principal: serde_json::Value = row.try_get("principal")?;
    let action: serde_json::Value = row.try_get("action")?;
    let resource: serde_json::Value = row.try_get("resource")?;
    let conditions: serde_json::Value = row.try_get("conditions")?;
    let created_by: Option<uuid::Uuid> = row.try_get("created_by")?;
    let updated_by: Option<uuid::Uuid> = row.try_get("updated_by")?;

    Ok(CedarPolicyCatalogRow {
        id,
        stable_key: row.try_get("stable_key")?,
        title: row.try_get("title")?,
        natural_language_rule: row.try_get("natural_language_rule")?,
        effect: CedarPolicyEffect::from_db_str(&effect)?,
        status: CedarPolicyStatus::from_db_str(&status)?,
        source: CedarPolicySource::from_db_str(&source)?,
        principal: serde_json::from_value(principal).map_err(|err| {
            KernelError::internal(format!("failed to decode Cedar principal selector: {err}"))
        })?,
        action: serde_json::from_value(action).map_err(|err| {
            KernelError::internal(format!("failed to decode Cedar action selector: {err}"))
        })?,
        resource: serde_json::from_value(resource).map_err(|err| {
            KernelError::internal(format!("failed to decode Cedar resource selector: {err}"))
        })?,
        conditions: serde_json::from_value(conditions).map_err(|err| {
            KernelError::internal(format!("failed to decode Cedar conditions: {err}"))
        })?,
        engine_mode: row.try_get("engine_mode")?,
        policy_version: row.try_get("policy_version")?,
        schema_version: row.try_get("schema_version")?,
        bundle_digest: row.try_get("bundle_digest")?,
        cedar_sdk_version: row.try_get("cedar_sdk_version")?,
        cedar_language_version: row.try_get("cedar_language_version")?,
        validation_status: CedarValidationStatus::from_db_str(&validation_status)?,
        created_by: created_by.map(UserId::from_uuid),
        updated_by: updated_by.map(UserId::from_uuid),
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

#[allow(dead_code)]
fn draft_from_row(row: &PgRow, org_id: OrgId) -> Result<CedarPolicyDraft, PgPolicyError> {
    let blocks: serde_json::Value = row.try_get("blocks")?;
    let normalized_row: serde_json::Value = row.try_get("normalized_row")?;
    let validation_status: String = row.try_get("validation_status")?;
    let validation_errors: serde_json::Value = row.try_get("validation_errors")?;
    let review_status: String = row.try_get("review_status")?;
    let created_by: uuid::Uuid = row.try_get("created_by")?;
    let updated_by: uuid::Uuid = row.try_get("updated_by")?;
    let reviewer_id: Option<uuid::Uuid> = row.try_get("reviewer_id")?;

    Ok(CedarPolicyDraft {
        id: row.try_get("id")?,
        org_id,
        draft_key: row.try_get("draft_key")?,
        title: row.try_get("title")?,
        author_note: row.try_get("author_note")?,
        blocks: serde_json::from_value::<CedarPolicyBlocks>(blocks).map_err(|err| {
            KernelError::internal(format!("failed to decode Cedar draft blocks: {err}"))
        })?,
        catalog_row: serde_json::from_value::<CedarPolicyCatalogRow>(normalized_row).map_err(
            |err| KernelError::internal(format!("failed to decode Cedar normalized row: {err}")),
        )?,
        generated_policy_text: row.try_get("generated_policy_text")?,
        generated_policy_digest: row.try_get("generated_policy_digest")?,
        validation_status: CedarValidationStatus::from_db_str(&validation_status)?,
        validation_errors: serde_json::from_value::<Vec<CedarValidationError>>(validation_errors)
            .map_err(|err| {
            KernelError::internal(format!("failed to decode Cedar validation errors: {err}"))
        })?,
        review_status: CedarPolicyReviewStatus::from_db_str(&review_status)?,
        reviewer_id: reviewer_id.map(UserId::from_uuid),
        review_note: row.try_get("review_note")?,
        created_by: UserId::from_uuid(created_by),
        updated_by: UserId::from_uuid(updated_by),
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}
