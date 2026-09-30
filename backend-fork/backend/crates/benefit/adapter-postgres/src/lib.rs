//! Postgres adapter for benefit-catalog storage and mutations.
//!
//! Reads run through `with_org_conn`; every mutation runs through `with_audits`
//! with an org-attached audit event so data changes and audit rows commit in the
//! same tenant-armed transaction. Client input never supplies org/tenant scope.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

use console_benefit_application::{
    BenefitCatalogConditionView, BenefitCatalogItemPage, BenefitCatalogItemView,
    BenefitCatalogLifecycleBinding, BenefitCatalogScopeDraft, BenefitCatalogTierView,
    BenefitConditionDraft, BenefitTierDraft, CreateBenefitCatalogItemCommand,
    GetBenefitCatalogItemQuery, ListBenefitCatalogItemsQuery, ReplaceBenefitConditionsCommand,
    ReplaceBenefitTiersCommand, UpdateBenefitCatalogItemCommand, UpdateBenefitCatalogItemFields,
    benefit_catalog_audit_event,
};
use console_benefit_domain::{
    BenefitCategory, BenefitCode, BenefitConditionKind, BenefitConditionOperator, BenefitScopeKind,
    MoneyWon, RateBasisPoints, normalize_optional_text, normalize_related_domain,
    normalize_required_text, validate_condition_value, validate_metadata_object,
};
use console_kernel_core::{
    BenefitCatalogConditionId, BenefitCatalogItemId, BenefitCatalogTierId, BranchId, BranchScope,
    ErrorKind, KernelError, OrgId, SiteId,
};
use console_platform_db::{DbError, lifecycle::INITIAL_STATE, with_audits, with_org_conn};
use console_platform_request_context::current_org;
use serde_json::Value;
use sqlx::{PgPool, Postgres, QueryBuilder, Row, Transaction, postgres::PgRow};
use time::{Date, OffsetDateTime};
use uuid::Uuid;

const DEFAULT_LIMIT: i64 = 50;
const MAX_LIMIT: i64 = 100;
const ITEM_COLUMNS: &str = "i.id, i.benefit_code, i.category, i.name, i.scope_type, i.scope_ref, \
     i.branch_id, i.site_id, i.coverage_label, i.covered_count, i.cost_label, \
     i.estimated_annual_cost_won, i.employer_rate_bps, i.note, i.legal_basis, \
     i.related_domain, i.related_object_id, i.effective_on, i.retires_on, \
     i.display_order, i.metadata, i.created_by, i.updated_by, i.created_at, i.updated_at";

#[derive(Debug, thiserror::Error)]
pub enum PgBenefitCatalogError {
    #[error(transparent)]
    Db(#[from] DbError),
    #[error(transparent)]
    Domain(#[from] KernelError),
}

impl From<sqlx::Error> for PgBenefitCatalogError {
    fn from(value: sqlx::Error) -> Self {
        Self::Db(DbError::Sqlx(value))
    }
}

impl PgBenefitCatalogError {
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

#[derive(Debug, Clone)]
pub struct PgBenefitCatalogStore {
    pool: PgPool,
}

impl PgBenefitCatalogStore {
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    #[must_use]
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    pub async fn list_items(
        &self,
        query: ListBenefitCatalogItemsQuery,
    ) -> Result<BenefitCatalogItemPage, PgBenefitCatalogError> {
        if query
            .lifecycle_state
            .as_ref()
            .is_some_and(|state| !state.trim().is_empty())
        {
            return Err(KernelError::validation(
                "benefit lifecycle filtering is owned by the generic lifecycle substrate",
            )
            .into());
        }
        let limit = normalized_limit(query.limit);
        let offset = query.offset.unwrap_or(0).max(0);

        let mut count_builder =
            QueryBuilder::<Postgres>::new("SELECT COUNT(*) FROM benefit_catalog_items i WHERE ");
        push_item_filters(&mut count_builder, &query)?;

        let mut builder = QueryBuilder::<Postgres>::new("SELECT ");
        builder.push(ITEM_COLUMNS);
        builder.push(" FROM benefit_catalog_items i WHERE ");
        push_item_filters(&mut builder, &query)?;
        builder.push(" ORDER BY i.category, i.display_order, i.name, i.id LIMIT ");
        builder.push_bind(limit);
        builder.push(" OFFSET ");
        builder.push_bind(offset);

        let org = current_org().map_err(KernelError::from)?;
        let (total, rows) = with_org_conn::<_, _, PgBenefitCatalogError>(&self.pool, org, |tx| {
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
            let base = base_item_from_row(row)?;
            let item = with_org_conn::<_, _, PgBenefitCatalogError>(&self.pool, org, |tx| {
                Box::pin(async move { hydrate_item_tx(tx, base).await })
            })
            .await?;
            items.push(item);
        }

        Ok(BenefitCatalogItemPage {
            items,
            limit,
            offset,
            total,
        })
    }

    pub async fn get_item(
        &self,
        query: GetBenefitCatalogItemQuery,
    ) -> Result<Option<BenefitCatalogItemView>, PgBenefitCatalogError> {
        let org = current_org().map_err(KernelError::from)?;
        with_org_conn::<_, _, PgBenefitCatalogError>(&self.pool, org, |tx| {
            Box::pin(
                async move { fetch_item_scoped_tx(tx, query.item_id, &query.branch_scope).await },
            )
        })
        .await
    }

    // console-gate: state-changing-handler
    pub async fn create_item(
        &self,
        command: CreateBenefitCatalogItemCommand,
    ) -> Result<BenefitCatalogItemView, PgBenefitCatalogError> {
        let input = NormalizedCreateInput::try_from(command)?;
        ensure_scope_mutation_allowed(&input.branch_scope, input.scope.branch_id)?;
        let org = current_org().map_err(KernelError::from)?;
        let org_uuid = *org.as_uuid();
        let item_id = BenefitCatalogItemId::new();

        with_audits::<_, BenefitCatalogItemView, PgBenefitCatalogError>(&self.pool, org, |tx| {
            Box::pin(async move {
                let benefit_code = issue_benefit_code_tx(tx, org_uuid).await?;
                insert_item_tx(tx, org_uuid, item_id, &benefit_code, &input).await?;
                create_lifecycle_tx(tx, org_uuid, item_id).await?;
                insert_tiers_tx(
                    tx,
                    org_uuid,
                    item_id,
                    input.actor,
                    input.occurred_at,
                    &input.tiers,
                )
                .await?;
                insert_conditions_tx(
                    tx,
                    org_uuid,
                    item_id,
                    input.actor,
                    input.occurred_at,
                    &input.conditions,
                )
                .await?;

                let view = fetch_item_tx(tx, item_id).await?.ok_or_else(|| {
                    KernelError::internal("created benefit-catalog item was not readable")
                })?;
                let event = benefit_catalog_audit_event(
                    "benefit_catalog.item.create",
                    Some(input.actor),
                    view.scope.branch_id,
                    item_id,
                    input.trace,
                    input.occurred_at,
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
        command: UpdateBenefitCatalogItemCommand,
    ) -> Result<BenefitCatalogItemView, PgBenefitCatalogError> {
        if command.fields.is_empty() {
            return Err(KernelError::validation("no benefit-catalog item fields to update").into());
        }
        let input = NormalizedUpdateInput::try_from(command)?;
        let org = current_org().map_err(KernelError::from)?;

        with_audits::<_, BenefitCatalogItemView, PgBenefitCatalogError>(&self.pool, org, |tx| {
            Box::pin(async move {
                let before = fetch_item_for_update_tx(tx, input.item_id)
                    .await?
                    .ok_or_else(|| KernelError::not_found("benefit-catalog item was not found"))?;
                ensure_scope_mutation_allowed(&input.branch_scope, before.scope.branch_id)?;
                if let Some(scope) = &input.fields.scope {
                    ensure_scope_mutation_allowed(&input.branch_scope, scope.branch_id)?;
                }

                update_item_tx(tx, &input).await?;
                let after = fetch_item_tx(tx, input.item_id).await?.ok_or_else(|| {
                    KernelError::internal("updated benefit-catalog item was not readable")
                })?;
                let event = benefit_catalog_audit_event(
                    "benefit_catalog.item.update",
                    Some(input.actor),
                    after.scope.branch_id.or(before.scope.branch_id),
                    input.item_id,
                    input.trace,
                    input.occurred_at,
                )?
                .with_org(org)
                .with_snapshots(Some(item_snapshot(&before)), Some(item_snapshot(&after)));
                Ok((after, vec![event]))
            })
        })
        .await
    }

    // console-gate: state-changing-handler
    pub async fn replace_tiers(
        &self,
        command: ReplaceBenefitTiersCommand,
    ) -> Result<BenefitCatalogItemView, PgBenefitCatalogError> {
        let tiers = normalize_tiers(command.tiers)?;
        let org = current_org().map_err(KernelError::from)?;
        let org_uuid = *org.as_uuid();

        with_audits::<_, BenefitCatalogItemView, PgBenefitCatalogError>(&self.pool, org, |tx| {
            Box::pin(async move {
                let before = fetch_item_for_update_tx(tx, command.item_id)
                    .await?
                    .ok_or_else(|| KernelError::not_found("benefit-catalog item was not found"))?;
                ensure_scope_mutation_allowed(&command.branch_scope, before.scope.branch_id)?;

                sqlx::query(
                    r#"
                    UPDATE benefit_catalog_tiers
                    SET status = 'RETIRED', updated_by = $2, updated_at = $3
                    WHERE benefit_id = $1 AND status = 'ACTIVE'
                    "#,
                )
                .bind(*command.item_id.as_uuid())
                .bind(*command.actor.as_uuid())
                .bind(command.occurred_at)
                .execute(tx.as_mut())
                .await?;
                insert_tiers_tx(
                    tx,
                    org_uuid,
                    command.item_id,
                    command.actor,
                    command.occurred_at,
                    &tiers,
                )
                .await?;

                let after = fetch_item_tx(tx, command.item_id).await?.ok_or_else(|| {
                    KernelError::internal(
                        "benefit-catalog item was not readable after tier replacement",
                    )
                })?;
                let event = benefit_catalog_audit_event(
                    "benefit_catalog.tiers.replace",
                    Some(command.actor),
                    before.scope.branch_id,
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
    pub async fn replace_conditions(
        &self,
        command: ReplaceBenefitConditionsCommand,
    ) -> Result<BenefitCatalogItemView, PgBenefitCatalogError> {
        let conditions = normalize_conditions(command.conditions)?;
        let org = current_org().map_err(KernelError::from)?;
        let org_uuid = *org.as_uuid();

        with_audits::<_, BenefitCatalogItemView, PgBenefitCatalogError>(&self.pool, org, |tx| {
            Box::pin(async move {
                let before = fetch_item_for_update_tx(tx, command.item_id)
                    .await?
                    .ok_or_else(|| KernelError::not_found("benefit-catalog item was not found"))?;
                ensure_scope_mutation_allowed(&command.branch_scope, before.scope.branch_id)?;

                sqlx::query(
                    r#"
                    UPDATE benefit_catalog_conditions
                    SET status = 'RETIRED', updated_by = $2, updated_at = $3
                    WHERE benefit_id = $1 AND status = 'ACTIVE'
                    "#,
                )
                .bind(*command.item_id.as_uuid())
                .bind(*command.actor.as_uuid())
                .bind(command.occurred_at)
                .execute(tx.as_mut())
                .await?;
                insert_conditions_tx(
                    tx,
                    org_uuid,
                    command.item_id,
                    command.actor,
                    command.occurred_at,
                    &conditions,
                )
                .await?;

                let after = fetch_item_tx(tx, command.item_id).await?.ok_or_else(|| {
                    KernelError::internal(
                        "benefit-catalog item was not readable after condition replacement",
                    )
                })?;
                let event = benefit_catalog_audit_event(
                    "benefit_catalog.conditions.replace",
                    Some(command.actor),
                    before.scope.branch_id,
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
}

#[derive(Debug, Clone, PartialEq)]
struct NormalizedScope {
    scope_type: BenefitScopeKind,
    scope_ref: Option<Uuid>,
    branch_id: Option<BranchId>,
    site_id: Option<SiteId>,
}

impl From<NormalizedScope> for BenefitCatalogScopeDraft {
    fn from(value: NormalizedScope) -> Self {
        Self {
            scope_type: value.scope_type,
            scope_ref: value.scope_ref,
            branch_id: value.branch_id,
            site_id: value.site_id,
        }
    }
}

#[derive(Debug, Clone)]
struct NormalizedCreateInput {
    actor: console_kernel_core::UserId,
    branch_scope: BranchScope,
    scope: NormalizedScope,
    category: BenefitCategory,
    name: String,
    coverage_label: String,
    covered_count: Option<i32>,
    cost_label: String,
    estimated_annual_cost_won: Option<i64>,
    employer_rate_bps: Option<i32>,
    note: Option<String>,
    legal_basis: Option<String>,
    related_domain: Option<String>,
    related_object_id: Option<Uuid>,
    effective_on: Option<Date>,
    retires_on: Option<Date>,
    display_order: i32,
    metadata: Value,
    tiers: Vec<NormalizedTier>,
    conditions: Vec<NormalizedCondition>,
    trace: console_kernel_core::TraceContext,
    occurred_at: OffsetDateTime,
}

impl TryFrom<CreateBenefitCatalogItemCommand> for NormalizedCreateInput {
    type Error = KernelError;

    fn try_from(command: CreateBenefitCatalogItemCommand) -> Result<Self, Self::Error> {
        let metadata = command.metadata;
        validate_metadata_object(&metadata)?;
        validate_dates(command.effective_on, command.retires_on)?;
        Ok(Self {
            actor: command.actor,
            branch_scope: command.branch_scope,
            scope: normalize_scope(command.scope)?,
            category: command.category,
            name: normalize_required_text(&command.name, 120, "benefit name")?,
            coverage_label: normalize_required_text(&command.coverage_label, 80, "coverage_label")?,
            covered_count: normalize_non_negative_i32(command.covered_count, "covered_count")?,
            cost_label: normalize_required_text(&command.cost_label, 80, "cost_label")?,
            estimated_annual_cost_won: command
                .estimated_annual_cost_won
                .map(MoneyWon::new)
                .transpose()?
                .map(MoneyWon::value),
            employer_rate_bps: command
                .employer_rate_bps
                .map(RateBasisPoints::new)
                .transpose()?
                .map(RateBasisPoints::value),
            note: normalize_optional_text(command.note, 500, "note")?,
            legal_basis: normalize_optional_text(command.legal_basis, 300, "legal_basis")?,
            related_domain: normalize_related_domain(command.related_domain)?,
            related_object_id: command.related_object_id,
            effective_on: command.effective_on,
            retires_on: command.retires_on,
            display_order: command.display_order,
            metadata,
            tiers: normalize_tiers(command.tiers)?,
            conditions: normalize_conditions(command.conditions)?,
            trace: command.trace,
            occurred_at: command.occurred_at,
        })
    }
}

#[derive(Debug, Clone)]
struct NormalizedUpdateInput {
    actor: console_kernel_core::UserId,
    branch_scope: BranchScope,
    item_id: BenefitCatalogItemId,
    fields: NormalizedUpdateFields,
    trace: console_kernel_core::TraceContext,
    occurred_at: OffsetDateTime,
}

impl TryFrom<UpdateBenefitCatalogItemCommand> for NormalizedUpdateInput {
    type Error = KernelError;

    fn try_from(command: UpdateBenefitCatalogItemCommand) -> Result<Self, Self::Error> {
        Ok(Self {
            actor: command.actor,
            branch_scope: command.branch_scope,
            item_id: command.item_id,
            fields: normalize_update_fields(command.fields)?,
            trace: command.trace,
            occurred_at: command.occurred_at,
        })
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
struct NormalizedUpdateFields {
    category: Option<BenefitCategory>,
    name: Option<String>,
    scope: Option<NormalizedScope>,
    coverage_label: Option<String>,
    covered_count: Option<Option<i32>>,
    cost_label: Option<String>,
    estimated_annual_cost_won: Option<Option<i64>>,
    employer_rate_bps: Option<Option<i32>>,
    note: Option<Option<String>>,
    legal_basis: Option<Option<String>>,
    related_domain: Option<Option<String>>,
    related_object_id: Option<Option<Uuid>>,
    effective_on: Option<Option<Date>>,
    retires_on: Option<Option<Date>>,
    display_order: Option<i32>,
    metadata: Option<Value>,
}

#[derive(Debug, Clone, PartialEq)]
struct NormalizedTier {
    tier_basis: String,
    tier_key: String,
    value_label: String,
    amount_won: Option<i64>,
    limit_period: Option<String>,
    criteria: Value,
    display_order: i32,
}

#[derive(Debug, Clone, PartialEq)]
struct NormalizedCondition {
    condition_kind: BenefitConditionKind,
    operator: BenefitConditionOperator,
    condition_key: String,
    condition_value: Value,
    display_label: String,
    cedar_policy_ref: Option<String>,
    display_order: i32,
}

fn normalized_limit(limit: Option<i64>) -> i64 {
    limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT)
}

fn normalize_non_negative_i32(value: Option<i32>, field: &str) -> Result<Option<i32>, KernelError> {
    if value.is_some_and(|value| value < 0) {
        Err(KernelError::validation(format!(
            "{field} must be non-negative"
        )))
    } else {
        Ok(value)
    }
}

fn validate_dates(effective_on: Option<Date>, retires_on: Option<Date>) -> Result<(), KernelError> {
    if let (Some(effective_on), Some(retires_on)) = (effective_on, retires_on)
        && retires_on < effective_on
    {
        return Err(KernelError::validation(
            "retires_on must be on or after effective_on",
        ));
    }
    Ok(())
}

fn normalize_scope(scope: BenefitCatalogScopeDraft) -> Result<NormalizedScope, KernelError> {
    match scope.scope_type {
        BenefitScopeKind::Org => {
            if scope.scope_ref.is_some() || scope.branch_id.is_some() || scope.site_id.is_some() {
                return Err(KernelError::validation(
                    "ORG benefit scope cannot include scope_ref, branch_id, or site_id",
                ));
            }
            Ok(NormalizedScope {
                scope_type: BenefitScopeKind::Org,
                scope_ref: None,
                branch_id: None,
                site_id: None,
            })
        }
        BenefitScopeKind::Branch => {
            let branch_id = scope.branch_id.ok_or_else(|| {
                KernelError::validation("BRANCH benefit scope requires branch_id")
            })?;
            let branch_uuid = *branch_id.as_uuid();
            if scope.site_id.is_some() {
                return Err(KernelError::validation(
                    "BRANCH benefit scope cannot include site_id",
                ));
            }
            if scope
                .scope_ref
                .is_some_and(|scope_ref| scope_ref != branch_uuid)
            {
                return Err(KernelError::validation(
                    "BRANCH benefit scope_ref must match branch_id",
                ));
            }
            Ok(NormalizedScope {
                scope_type: BenefitScopeKind::Branch,
                scope_ref: Some(branch_uuid),
                branch_id: Some(branch_id),
                site_id: None,
            })
        }
        BenefitScopeKind::Site => {
            let branch_id = scope
                .branch_id
                .ok_or_else(|| KernelError::validation("SITE benefit scope requires branch_id"))?;
            let site_id = scope
                .site_id
                .ok_or_else(|| KernelError::validation("SITE benefit scope requires site_id"))?;
            let site_uuid = *site_id.as_uuid();
            if scope
                .scope_ref
                .is_some_and(|scope_ref| scope_ref != site_uuid)
            {
                return Err(KernelError::validation(
                    "SITE benefit scope_ref must match site_id",
                ));
            }
            Ok(NormalizedScope {
                scope_type: BenefitScopeKind::Site,
                scope_ref: Some(site_uuid),
                branch_id: Some(branch_id),
                site_id: Some(site_id),
            })
        }
        BenefitScopeKind::Team | BenefitScopeKind::Role | BenefitScopeKind::EmployeeSegment => {
            let scope_ref = scope.scope_ref.ok_or_else(|| {
                KernelError::validation(
                    "TEAM/ROLE/EMPLOYEE_SEGMENT benefit scopes require scope_ref",
                )
            })?;
            if scope.site_id.is_some() && scope.branch_id.is_none() {
                return Err(KernelError::validation(
                    "site-scoped benefit conditions require branch_id",
                ));
            }
            Ok(NormalizedScope {
                scope_type: scope.scope_type,
                scope_ref: Some(scope_ref),
                branch_id: scope.branch_id,
                site_id: scope.site_id,
            })
        }
    }
}

fn ensure_scope_mutation_allowed(
    branch_scope: &BranchScope,
    branch_id: Option<BranchId>,
) -> Result<(), KernelError> {
    match branch_id {
        Some(branch_id) if branch_scope.allows(branch_id) => Ok(()),
        Some(_) => Err(KernelError::not_found(
            "benefit-catalog scope is outside principal branch scope",
        )),
        None if matches!(branch_scope, BranchScope::All) => Ok(()),
        None => Err(KernelError::forbidden(
            "org-wide benefit-catalog mutations require org-wide branch scope",
        )),
    }
}

fn normalize_tiers(tiers: Vec<BenefitTierDraft>) -> Result<Vec<NormalizedTier>, KernelError> {
    tiers
        .into_iter()
        .map(|tier| {
            let criteria = tier.criteria;
            validate_metadata_object(&criteria)?;
            let limit_period = normalize_limit_period(tier.limit_period)?;
            Ok(NormalizedTier {
                tier_basis: normalize_required_text(&tier.tier_basis, 80, "tier_basis")?,
                tier_key: normalize_required_text(&tier.tier_key, 120, "tier_key")?,
                value_label: normalize_required_text(&tier.value_label, 300, "value_label")?,
                amount_won: tier
                    .amount_won
                    .map(MoneyWon::new)
                    .transpose()?
                    .map(MoneyWon::value),
                limit_period,
                criteria,
                display_order: tier.display_order,
            })
        })
        .collect()
}

fn normalize_conditions(
    conditions: Vec<BenefitConditionDraft>,
) -> Result<Vec<NormalizedCondition>, KernelError> {
    conditions
        .into_iter()
        .map(|condition| {
            validate_condition_value(&condition.condition_value)?;
            let condition_key = normalize_condition_key(&condition.condition_key)?;
            Ok(NormalizedCondition {
                condition_kind: condition.condition_kind,
                operator: condition.operator,
                condition_key,
                condition_value: condition.condition_value,
                display_label: normalize_required_text(
                    &condition.display_label,
                    200,
                    "display_label",
                )?,
                cedar_policy_ref: normalize_optional_text(
                    condition.cedar_policy_ref,
                    200,
                    "cedar_policy_ref",
                )?,
                display_order: condition.display_order,
            })
        })
        .collect()
}

fn normalize_limit_period(value: Option<String>) -> Result<Option<String>, KernelError> {
    let Some(value) = normalize_optional_text(value, 32, "limit_period")? else {
        return Ok(None);
    };
    let normalized = value.to_ascii_uppercase();
    if matches!(
        normalized.as_str(),
        "MONTH" | "QUARTER" | "YEAR" | "EVENT" | "TENURE_MILESTONE"
    ) {
        Ok(Some(normalized))
    } else {
        Err(KernelError::validation("unknown benefit tier limit_period"))
    }
}

fn normalize_condition_key(value: &str) -> Result<String, KernelError> {
    let key = normalize_required_text(value, 64, "condition_key")?;
    let mut chars = key.chars();
    let Some(first) = chars.next() else {
        return Err(KernelError::validation("condition_key is required"));
    };
    let valid = first.is_ascii_lowercase()
        && key.len() >= 2
        && chars.all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_');
    if valid {
        Ok(key)
    } else {
        Err(KernelError::validation(
            "condition_key must match ^[a-z][a-z0-9_]{1,63}$",
        ))
    }
}

fn normalize_update_fields(
    fields: UpdateBenefitCatalogItemFields,
) -> Result<NormalizedUpdateFields, KernelError> {
    let metadata = fields.metadata;
    if let Some(metadata) = &metadata {
        validate_metadata_object(metadata)?;
    }
    let effective_on = fields.effective_on;
    let retires_on = fields.retires_on;
    if let (Some(Some(effective_on)), Some(Some(retires_on))) = (effective_on, retires_on) {
        validate_dates(Some(effective_on), Some(retires_on))?;
    }
    Ok(NormalizedUpdateFields {
        category: fields.category,
        name: fields
            .name
            .map(|value| normalize_required_text(&value, 120, "benefit name"))
            .transpose()?,
        scope: fields.scope.map(normalize_scope).transpose()?,
        coverage_label: fields
            .coverage_label
            .map(|value| normalize_required_text(&value, 80, "coverage_label"))
            .transpose()?,
        covered_count: fields
            .covered_count
            .map(|value| normalize_non_negative_i32(value, "covered_count"))
            .transpose()?,
        cost_label: fields
            .cost_label
            .map(|value| normalize_required_text(&value, 80, "cost_label"))
            .transpose()?,
        estimated_annual_cost_won: fields
            .estimated_annual_cost_won
            .map(|value| {
                value
                    .map(MoneyWon::new)
                    .transpose()
                    .map(|value| value.map(MoneyWon::value))
            })
            .transpose()?,
        employer_rate_bps: fields
            .employer_rate_bps
            .map(|value| {
                value
                    .map(RateBasisPoints::new)
                    .transpose()
                    .map(|value| value.map(RateBasisPoints::value))
            })
            .transpose()?,
        note: fields
            .note
            .map(|value| normalize_optional_text(value, 500, "note"))
            .transpose()?,
        legal_basis: fields
            .legal_basis
            .map(|value| normalize_optional_text(value, 300, "legal_basis"))
            .transpose()?,
        related_domain: fields
            .related_domain
            .map(normalize_related_domain)
            .transpose()?,
        related_object_id: fields.related_object_id,
        effective_on,
        retires_on,
        display_order: fields.display_order,
        metadata,
    })
}

async fn issue_benefit_code_tx(
    tx: &mut Transaction<'_, Postgres>,
    org_uuid: Uuid,
) -> Result<String, PgBenefitCatalogError> {
    let issued: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO benefit_code_counters (org_id, object_prefix, next_value)
        VALUES ($1, 'BF', 2)
        ON CONFLICT (org_id, object_prefix)
        DO UPDATE SET next_value = benefit_code_counters.next_value + 1,
                      updated_at = now()
        RETURNING next_value - 1
        "#,
    )
    .bind(org_uuid)
    .fetch_one(tx.as_mut())
    .await?;
    Ok(BenefitCode::new(format!("BF-{issued:04}"))?.into_string())
}

async fn create_lifecycle_tx(
    tx: &mut Transaction<'_, Postgres>,
    org_id: Uuid,
    item_id: BenefitCatalogItemId,
) -> Result<(), PgBenefitCatalogError> {
    sqlx::query(
        "INSERT INTO object_lifecycles (org_id, object_type, object_id, current_state) \
         VALUES ($1, 'benefit_catalog_item', $2, $3) \
         ON CONFLICT (org_id, object_type, object_id) DO NOTHING",
    )
    .bind(org_id)
    .bind(*item_id.as_uuid())
    .bind(INITIAL_STATE)
    .execute(tx.as_mut())
    .await?;
    Ok(())
}

async fn insert_item_tx(
    tx: &mut Transaction<'_, Postgres>,
    org_uuid: Uuid,
    item_id: BenefitCatalogItemId,
    benefit_code: &str,
    input: &NormalizedCreateInput,
) -> Result<(), PgBenefitCatalogError> {
    sqlx::query(
        r#"
        INSERT INTO benefit_catalog_items (
            id, org_id, benefit_code, category, name, scope_type, scope_ref, branch_id,
            site_id, coverage_label, covered_count, cost_label, estimated_annual_cost_won,
            employer_rate_bps, note, legal_basis, related_domain, related_object_id,
            effective_on, retires_on, display_order, metadata, created_by, updated_by,
            created_at, updated_at
        ) VALUES (
            $1, $2, $3, $4, $5, $6, $7, $8,
            $9, $10, $11, $12, $13,
            $14, $15, $16, $17, $18,
            $19, $20, $21, $22, $23, $23,
            $24, $24
        )
        "#,
    )
    .bind(*item_id.as_uuid())
    .bind(org_uuid)
    .bind(benefit_code)
    .bind(input.category.as_db_str())
    .bind(&input.name)
    .bind(input.scope.scope_type.as_db_str())
    .bind(input.scope.scope_ref)
    .bind(input.scope.branch_id.map(|id| *id.as_uuid()))
    .bind(input.scope.site_id.map(|id| *id.as_uuid()))
    .bind(&input.coverage_label)
    .bind(input.covered_count)
    .bind(&input.cost_label)
    .bind(input.estimated_annual_cost_won)
    .bind(input.employer_rate_bps)
    .bind(input.note.as_deref())
    .bind(input.legal_basis.as_deref())
    .bind(input.related_domain.as_deref())
    .bind(input.related_object_id)
    .bind(input.effective_on)
    .bind(input.retires_on)
    .bind(input.display_order)
    .bind(&input.metadata)
    .bind(*input.actor.as_uuid())
    .bind(input.occurred_at)
    .execute(tx.as_mut())
    .await?;
    Ok(())
}

async fn update_item_tx(
    tx: &mut Transaction<'_, Postgres>,
    input: &NormalizedUpdateInput,
) -> Result<(), PgBenefitCatalogError> {
    let mut builder = QueryBuilder::<Postgres>::new("UPDATE benefit_catalog_items SET ");
    let mut sep = builder.separated(", ");
    if let Some(category) = input.fields.category {
        sep.push("category = ");
        sep.push_bind_unseparated(category.as_db_str());
    }
    if let Some(name) = input.fields.name.clone() {
        sep.push("name = ");
        sep.push_bind_unseparated(name);
    }
    if let Some(scope) = input.fields.scope.clone() {
        sep.push("scope_type = ");
        sep.push_bind_unseparated(scope.scope_type.as_db_str());
        sep.push("scope_ref = ");
        sep.push_bind_unseparated(scope.scope_ref);
        sep.push("branch_id = ");
        sep.push_bind_unseparated(scope.branch_id.map(|id| *id.as_uuid()));
        sep.push("site_id = ");
        sep.push_bind_unseparated(scope.site_id.map(|id| *id.as_uuid()));
    }
    if let Some(coverage_label) = input.fields.coverage_label.clone() {
        sep.push("coverage_label = ");
        sep.push_bind_unseparated(coverage_label);
    }
    if let Some(covered_count) = input.fields.covered_count {
        sep.push("covered_count = ");
        sep.push_bind_unseparated(covered_count);
    }
    if let Some(cost_label) = input.fields.cost_label.clone() {
        sep.push("cost_label = ");
        sep.push_bind_unseparated(cost_label);
    }
    if let Some(cost) = input.fields.estimated_annual_cost_won {
        sep.push("estimated_annual_cost_won = ");
        sep.push_bind_unseparated(cost);
    }
    if let Some(rate) = input.fields.employer_rate_bps {
        sep.push("employer_rate_bps = ");
        sep.push_bind_unseparated(rate);
    }
    if let Some(note) = input.fields.note.clone() {
        sep.push("note = ");
        sep.push_bind_unseparated(note);
    }
    if let Some(legal_basis) = input.fields.legal_basis.clone() {
        sep.push("legal_basis = ");
        sep.push_bind_unseparated(legal_basis);
    }
    if let Some(related_domain) = input.fields.related_domain.clone() {
        sep.push("related_domain = ");
        sep.push_bind_unseparated(related_domain);
    }
    if let Some(related_object_id) = input.fields.related_object_id {
        sep.push("related_object_id = ");
        sep.push_bind_unseparated(related_object_id);
    }
    if let Some(effective_on) = input.fields.effective_on {
        sep.push("effective_on = ");
        sep.push_bind_unseparated(effective_on);
    }
    if let Some(retires_on) = input.fields.retires_on {
        sep.push("retires_on = ");
        sep.push_bind_unseparated(retires_on);
    }
    if let Some(display_order) = input.fields.display_order {
        sep.push("display_order = ");
        sep.push_bind_unseparated(display_order);
    }
    if let Some(metadata) = input.fields.metadata.clone() {
        sep.push("metadata = ");
        sep.push_bind_unseparated(metadata);
    }
    sep.push("updated_by = ");
    sep.push_bind_unseparated(*input.actor.as_uuid());
    sep.push("updated_at = ");
    sep.push_bind_unseparated(input.occurred_at);
    builder.push(" WHERE id = ");
    builder.push_bind(*input.item_id.as_uuid());
    builder.build().execute(tx.as_mut()).await?;
    Ok(())
}

async fn insert_tiers_tx(
    tx: &mut Transaction<'_, Postgres>,
    org_uuid: Uuid,
    item_id: BenefitCatalogItemId,
    actor: console_kernel_core::UserId,
    occurred_at: OffsetDateTime,
    tiers: &[NormalizedTier],
) -> Result<(), PgBenefitCatalogError> {
    for tier in tiers {
        let tier_id = BenefitCatalogTierId::new();
        sqlx::query(
            r#"
            INSERT INTO benefit_catalog_tiers (
                id, org_id, benefit_id, tier_basis, tier_key, value_label, amount_won,
                limit_period, criteria, display_order, status, created_by, updated_by,
                created_at, updated_at
            ) VALUES ($1, $2, $3, $4, $5, $6, $7,
                      $8, $9, $10, 'ACTIVE', $11, $11, $12, $12)
            "#,
        )
        .bind(*tier_id.as_uuid())
        .bind(org_uuid)
        .bind(*item_id.as_uuid())
        .bind(&tier.tier_basis)
        .bind(&tier.tier_key)
        .bind(&tier.value_label)
        .bind(tier.amount_won)
        .bind(tier.limit_period.as_deref())
        .bind(&tier.criteria)
        .bind(tier.display_order)
        .bind(*actor.as_uuid())
        .bind(occurred_at)
        .execute(tx.as_mut())
        .await?;
    }
    Ok(())
}

async fn insert_conditions_tx(
    tx: &mut Transaction<'_, Postgres>,
    org_uuid: Uuid,
    item_id: BenefitCatalogItemId,
    actor: console_kernel_core::UserId,
    occurred_at: OffsetDateTime,
    conditions: &[NormalizedCondition],
) -> Result<(), PgBenefitCatalogError> {
    for condition in conditions {
        let condition_id = BenefitCatalogConditionId::new();
        sqlx::query(
            r#"
            INSERT INTO benefit_catalog_conditions (
                id, org_id, benefit_id, condition_kind, operator, condition_key,
                condition_value, display_label, cedar_policy_ref, display_order, status,
                created_by, updated_by, created_at, updated_at
            ) VALUES ($1, $2, $3, $4, $5, $6,
                      $7, $8, $9, $10, 'ACTIVE', $11, $11, $12, $12)
            "#,
        )
        .bind(*condition_id.as_uuid())
        .bind(org_uuid)
        .bind(*item_id.as_uuid())
        .bind(condition.condition_kind.as_db_str())
        .bind(condition.operator.as_db_str())
        .bind(&condition.condition_key)
        .bind(&condition.condition_value)
        .bind(&condition.display_label)
        .bind(condition.cedar_policy_ref.as_deref())
        .bind(condition.display_order)
        .bind(*actor.as_uuid())
        .bind(occurred_at)
        .execute(tx.as_mut())
        .await?;
    }
    Ok(())
}

fn push_scope_visibility(builder: &mut QueryBuilder<Postgres>, branch_scope: &BranchScope) {
    match branch_scope {
        BranchScope::All => builder.push("TRUE"),
        BranchScope::Branches(branches) if branches.is_empty() => {
            builder.push("i.scope_type = 'ORG'")
        }
        BranchScope::Branches(branches) => {
            let ids = branches
                .iter()
                .map(|branch| *branch.as_uuid())
                .collect::<Vec<_>>();
            builder.push("(i.scope_type = 'ORG' OR i.branch_id = ANY(");
            builder.push_bind(ids);
            builder.push("))")
        }
    };
}

fn push_item_filters(
    builder: &mut QueryBuilder<Postgres>,
    query: &ListBenefitCatalogItemsQuery,
) -> Result<(), KernelError> {
    push_scope_visibility(builder, &query.branch_scope);
    if let Some(category) = query.category {
        builder.push(" AND i.category = ");
        builder.push_bind(category.as_db_str());
    }
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
    if let Some(q) = query.q.as_ref().map(|q| q.trim()).filter(|q| !q.is_empty()) {
        let pattern = format!("%{q}%");
        builder.push(" AND (i.benefit_code ILIKE ");
        builder.push_bind(pattern.clone());
        builder.push(" OR i.name ILIKE ");
        builder.push_bind(pattern.clone());
        builder.push(" OR i.coverage_label ILIKE ");
        builder.push_bind(pattern.clone());
        builder.push(" OR i.cost_label ILIKE ");
        builder.push_bind(pattern);
        builder.push(")");
    }
    Ok(())
}

async fn fetch_item_scoped_tx(
    tx: &mut Transaction<'_, Postgres>,
    item_id: BenefitCatalogItemId,
    branch_scope: &BranchScope,
) -> Result<Option<BenefitCatalogItemView>, PgBenefitCatalogError> {
    let mut builder = QueryBuilder::<Postgres>::new("SELECT ");
    builder.push(ITEM_COLUMNS);
    builder.push(" FROM benefit_catalog_items i WHERE i.id = ");
    builder.push_bind(*item_id.as_uuid());
    builder.push(" AND ");
    push_scope_visibility(&mut builder, branch_scope);
    let row = builder.build().fetch_optional(tx.as_mut()).await?;
    let Some(row) = row else {
        return Ok(None);
    };
    let base = base_item_from_row(&row)?;
    hydrate_item_tx(tx, base).await.map(Some)
}

async fn fetch_item_tx(
    tx: &mut Transaction<'_, Postgres>,
    item_id: BenefitCatalogItemId,
) -> Result<Option<BenefitCatalogItemView>, PgBenefitCatalogError> {
    fetch_item_inner_tx(tx, item_id, false).await
}

async fn fetch_item_for_update_tx(
    tx: &mut Transaction<'_, Postgres>,
    item_id: BenefitCatalogItemId,
) -> Result<Option<BenefitCatalogItemView>, PgBenefitCatalogError> {
    fetch_item_inner_tx(tx, item_id, true).await
}

async fn fetch_item_inner_tx(
    tx: &mut Transaction<'_, Postgres>,
    item_id: BenefitCatalogItemId,
    for_update: bool,
) -> Result<Option<BenefitCatalogItemView>, PgBenefitCatalogError> {
    let mut sql = format!("SELECT {ITEM_COLUMNS} FROM benefit_catalog_items i WHERE i.id = $1");
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
    let Some(row) = row else {
        return Ok(None);
    };
    let base = base_item_from_row(&row)?;
    hydrate_item_tx(tx, base).await.map(Some)
}

async fn hydrate_item_tx(
    tx: &mut Transaction<'_, Postgres>,
    mut item: BenefitCatalogItemView,
) -> Result<BenefitCatalogItemView, PgBenefitCatalogError> {
    item.tiers = fetch_tiers_tx(tx, item.id).await?;
    item.conditions = fetch_conditions_tx(tx, item.id).await?;
    item.lifecycle = fetch_lifecycle_binding_tx(tx, item.id).await?;
    Ok(item)
}

async fn fetch_lifecycle_binding_tx(
    tx: &mut Transaction<'_, Postgres>,
    item_id: BenefitCatalogItemId,
) -> Result<BenefitCatalogLifecycleBinding, PgBenefitCatalogError> {
    let row = sqlx::query(
        "SELECT current_state, legal_hold, retention_until \
         FROM object_lifecycles \
         WHERE object_type = 'benefit_catalog_item' AND object_id = $1",
    )
    .bind(*item_id.as_uuid())
    .fetch_optional(tx.as_mut())
    .await?;
    let Some(row) = row else {
        return Ok(BenefitCatalogLifecycleBinding::new(item_id));
    };
    let retention_until: Option<Date> = row.try_get("retention_until")?;
    Ok(BenefitCatalogLifecycleBinding {
        object_type: "benefit_catalog_item".to_owned(),
        object_id: item_id,
        current_state: Some(row.try_get("current_state")?),
        legal_hold: Some(row.try_get("legal_hold")?),
        retention_until: retention_until
            .map(|date| date.with_time(time::Time::MIDNIGHT).assume_utc()),
    })
}

async fn fetch_tiers_tx(
    tx: &mut Transaction<'_, Postgres>,
    item_id: BenefitCatalogItemId,
) -> Result<Vec<BenefitCatalogTierView>, PgBenefitCatalogError> {
    let rows = sqlx::query(
        r#"
        SELECT id, benefit_id, tier_basis, tier_key, value_label, amount_won,
               limit_period, criteria, display_order
        FROM benefit_catalog_tiers
        WHERE benefit_id = $1 AND status = 'ACTIVE'
        ORDER BY display_order, tier_basis, tier_key, id
        "#,
    )
    .bind(*item_id.as_uuid())
    .fetch_all(tx.as_mut())
    .await?;
    rows.iter().map(tier_from_row).collect()
}

async fn fetch_conditions_tx(
    tx: &mut Transaction<'_, Postgres>,
    item_id: BenefitCatalogItemId,
) -> Result<Vec<BenefitCatalogConditionView>, PgBenefitCatalogError> {
    let rows = sqlx::query(
        r#"
        SELECT id, benefit_id, condition_kind, operator, condition_key,
               condition_value, display_label, cedar_policy_ref, display_order
        FROM benefit_catalog_conditions
        WHERE benefit_id = $1 AND status = 'ACTIVE'
        ORDER BY display_order, condition_kind, condition_key, id
        "#,
    )
    .bind(*item_id.as_uuid())
    .fetch_all(tx.as_mut())
    .await?;
    rows.iter().map(condition_from_row).collect()
}

fn base_item_from_row(row: &PgRow) -> Result<BenefitCatalogItemView, PgBenefitCatalogError> {
    let id = BenefitCatalogItemId::from_uuid(row.try_get("id")?);
    let category: String = row.try_get("category")?;
    let scope_type: String = row.try_get("scope_type")?;
    let branch_id: Option<Uuid> = row.try_get("branch_id")?;
    let site_id: Option<Uuid> = row.try_get("site_id")?;
    Ok(BenefitCatalogItemView {
        id,
        benefit_code: row.try_get("benefit_code")?,
        category: BenefitCategory::parse(&category)?,
        name: row.try_get("name")?,
        scope: BenefitCatalogScopeDraft {
            scope_type: BenefitScopeKind::parse(&scope_type)?,
            scope_ref: row.try_get("scope_ref")?,
            branch_id: branch_id.map(BranchId::from_uuid),
            site_id: site_id.map(SiteId::from_uuid),
        },
        coverage_label: row.try_get("coverage_label")?,
        covered_count: row.try_get("covered_count")?,
        cost_label: row.try_get("cost_label")?,
        estimated_annual_cost_won: row.try_get("estimated_annual_cost_won")?,
        employer_rate_bps: row.try_get("employer_rate_bps")?,
        note: row.try_get("note")?,
        legal_basis: row.try_get("legal_basis")?,
        related_domain: row.try_get("related_domain")?,
        related_object_id: row.try_get("related_object_id")?,
        effective_on: row.try_get("effective_on")?,
        retires_on: row.try_get("retires_on")?,
        display_order: row.try_get("display_order")?,
        metadata: row.try_get("metadata")?,
        tiers: Vec::new(),
        conditions: Vec::new(),
        lifecycle: BenefitCatalogLifecycleBinding::new(id),
        created_by: console_kernel_core::UserId::from_uuid(row.try_get("created_by")?),
        updated_by: console_kernel_core::UserId::from_uuid(row.try_get("updated_by")?),
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

fn tier_from_row(row: &PgRow) -> Result<BenefitCatalogTierView, PgBenefitCatalogError> {
    Ok(BenefitCatalogTierView {
        id: BenefitCatalogTierId::from_uuid(row.try_get("id")?),
        benefit_id: BenefitCatalogItemId::from_uuid(row.try_get("benefit_id")?),
        tier_basis: row.try_get("tier_basis")?,
        tier_key: row.try_get("tier_key")?,
        value_label: row.try_get("value_label")?,
        amount_won: row.try_get("amount_won")?,
        limit_period: row.try_get("limit_period")?,
        criteria: row.try_get("criteria")?,
        display_order: row.try_get("display_order")?,
    })
}

fn condition_from_row(row: &PgRow) -> Result<BenefitCatalogConditionView, PgBenefitCatalogError> {
    let condition_kind: String = row.try_get("condition_kind")?;
    let operator: String = row.try_get("operator")?;
    Ok(BenefitCatalogConditionView {
        id: BenefitCatalogConditionId::from_uuid(row.try_get("id")?),
        benefit_id: BenefitCatalogItemId::from_uuid(row.try_get("benefit_id")?),
        condition_kind: BenefitConditionKind::parse(&condition_kind)?,
        operator: BenefitConditionOperator::parse(&operator)?,
        condition_key: row.try_get("condition_key")?,
        condition_value: row.try_get("condition_value")?,
        display_label: row.try_get("display_label")?,
        cedar_policy_ref: row.try_get("cedar_policy_ref")?,
        display_order: row.try_get("display_order")?,
    })
}

fn item_snapshot(item: &BenefitCatalogItemView) -> Value {
    serde_json::json!({
        "id": item.id.to_string(),
        "benefit_code": item.benefit_code,
        "category": item.category.as_wire_str(),
        "name": item.name,
        "scope": {
            "type": item.scope.scope_type.as_db_str(),
            "ref": item.scope.scope_ref,
            "branch_id": item.scope.branch_id.map(|id| id.to_string()),
            "site_id": item.scope.site_id.map(|id| id.to_string()),
        },
        "coverage_label": item.coverage_label,
        "covered_count": item.covered_count,
        "cost_label": item.cost_label,
        "estimated_annual_cost_won": item.estimated_annual_cost_won,
        "employer_rate_bps": item.employer_rate_bps,
        "effective_on": item.effective_on.map(|date| date.to_string()),
        "retires_on": item.retires_on.map(|date| date.to_string()),
        "tier_count": item.tiers.len(),
        "condition_count": item.conditions.len(),
        "lifecycle": {
            "object_type": item.lifecycle.object_type,
            "object_id": item.lifecycle.object_id.to_string(),
        },
    })
}

#[allow(dead_code)]
fn _org_id_type_anchor(_: OrgId) {}
