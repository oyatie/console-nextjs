//! Tenant-scoped benefit-catalog REST API.
//!
//! The catalog owns only CRUD rows, tiers, and eligibility conditions. Lifecycle
//! transitions deliberately remain on the generic lifecycle router, so this
//! module cannot bypass four-eyes, retention, or lifecycle audit controls.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

mod openapi;
pub use openapi::OPENAPI_FRAGMENT;

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use console_benefit_adapter_postgres::{PgBenefitCatalogError, PgBenefitCatalogStore};
use console_benefit_application::{
    BenefitCatalogItemView, BenefitCatalogScopeDraft, BenefitConditionDraft, BenefitTierDraft,
    CreateBenefitCatalogItemCommand, GetBenefitCatalogItemQuery, ListBenefitCatalogItemsQuery,
    ReplaceBenefitConditionsCommand, ReplaceBenefitTiersCommand, UpdateBenefitCatalogItemCommand,
    UpdateBenefitCatalogItemFields,
};
use console_benefit_domain::BenefitCategory;
use console_kernel_core::{
    BenefitCatalogItemId, BranchId, BranchScope, ErrorKind, KernelError, SiteId, TraceContext,
};
use console_platform_auth::JwtVerifier;
use console_platform_authz::{
    Action, Feature, Principal, authorize_capability, authorize_org_wide,
};
use console_platform_request_context::RequestContextError;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use time::Date;
use uuid::Uuid;

mod iso_date_opt {
    use serde::{Deserialize, Deserializer, Serializer};
    use time::Date;
    use time::format_description::well_known::Iso8601;

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<Date>, D::Error> {
        let raw: Option<String> = Option::deserialize(deserializer)?;
        raw.map(|value| Date::parse(&value, &Iso8601::DATE))
            .transpose()
            .map_err(serde::de::Error::custom)
    }

    #[allow(dead_code)]
    pub fn serialize<S: Serializer>(
        value: &Option<Date>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        match value {
            Some(date) => serializer.serialize_some(
                &date
                    .format(&Iso8601::DATE)
                    .map_err(serde::ser::Error::custom)?,
            ),
            None => serializer.serialize_none(),
        }
    }
}

pub const BENEFIT_CATALOG_ITEMS_PATH: &str = "/api/v1/benefit-catalog/items";
pub const BENEFIT_CATALOG_ITEM_PATH_TEMPLATE: &str = "/api/v1/benefit-catalog/items/{benefit_id}";
pub const BENEFIT_CATALOG_TIERS_PATH_TEMPLATE: &str =
    "/api/v1/benefit-catalog/items/{benefit_id}/tiers";
pub const BENEFIT_CATALOG_CONDITIONS_PATH_TEMPLATE: &str =
    "/api/v1/benefit-catalog/items/{benefit_id}/conditions";
pub const BENEFIT_ROUTE_PATHS: &[&str] = &[
    BENEFIT_CATALOG_ITEMS_PATH,
    BENEFIT_CATALOG_ITEM_PATH_TEMPLATE,
    BENEFIT_CATALOG_TIERS_PATH_TEMPLATE,
    BENEFIT_CATALOG_CONDITIONS_PATH_TEMPLATE,
];

#[derive(Clone)]
pub struct BenefitRestState {
    store: PgBenefitCatalogStore,
    jwt_verifier: Option<JwtVerifier>,
}
impl BenefitRestState {
    #[must_use]
    pub fn new(store: PgBenefitCatalogStore, jwt_verifier: Option<JwtVerifier>) -> Self {
        Self {
            store,
            jwt_verifier,
        }
    }
}

pub fn router(state: BenefitRestState) -> Router {
    let verifier = state.jwt_verifier.clone();
    let pool = state.store.pool().clone();
    let router = Router::new()
        .route(
            BENEFIT_CATALOG_ITEMS_PATH,
            get(list_items).post(create_item),
        )
        .route(
            BENEFIT_CATALOG_ITEM_PATH_TEMPLATE,
            get(get_item).patch(update_item),
        )
        .route(
            BENEFIT_CATALOG_TIERS_PATH_TEMPLATE,
            axum::routing::put(replace_tiers),
        )
        .route(
            BENEFIT_CATALOG_CONDITIONS_PATH_TEMPLATE,
            axum::routing::put(replace_conditions),
        )
        .with_state(state);
    console_platform_request_context::with_request_context(router, verifier, pool)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ListParams {
    category: Option<String>,
    branch_id: Option<Uuid>,
    site_id: Option<Uuid>,
    lifecycle_state: Option<String>,
    q: Option<String>,
    limit: Option<i64>,
    offset: Option<i64>,
}

async fn list_items(
    State(state): State<BenefitRestState>,
    headers: HeaderMap,
    Query(params): Query<ListParams>,
) -> Result<Json<console_benefit_application::BenefitCatalogItemPage>, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    require_feature(&principal, Feature::BenefitCatalogRead)?;
    let category = params
        .category
        .as_deref()
        .map(BenefitCategory::parse)
        .transpose()
        .map_err(RestError::from_kernel)?;
    let page = state
        .store
        .list_items(ListBenefitCatalogItemsQuery {
            branch_scope: principal.branch_scope,
            category,
            branch_id: params.branch_id.map(BranchId::from_uuid),
            site_id: params.site_id.map(SiteId::from_uuid),
            lifecycle_state: params.lifecycle_state,
            q: params.q,
            limit: params.limit,
            offset: params.offset,
        })
        .await
        .map_err(RestError::from_store)?;
    Ok(Json(page))
}

async fn get_item(
    State(state): State<BenefitRestState>,
    headers: HeaderMap,
    Path(benefit_id): Path<Uuid>,
) -> Result<Json<BenefitCatalogItemView>, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    require_feature(&principal, Feature::BenefitCatalogRead)?;
    let item = state
        .store
        .get_item(GetBenefitCatalogItemQuery {
            branch_scope: principal.branch_scope,
            item_id: BenefitCatalogItemId::from_uuid(benefit_id),
        })
        .await
        .map_err(RestError::from_store)?;
    item.map(Json).ok_or_else(|| {
        RestError::from_kernel(KernelError::not_found("benefit-catalog item was not found"))
    })
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CreateItemBody {
    scope: BenefitCatalogScopeDraft,
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
    #[serde(default, with = "iso_date_opt")]
    effective_on: Option<Date>,
    #[serde(default, with = "iso_date_opt")]
    retires_on: Option<Date>,
    #[serde(default)]
    display_order: i32,
    #[serde(default = "empty_object")]
    metadata: Value,
    #[serde(default)]
    tiers: Vec<BenefitTierDraft>,
    #[serde(default)]
    conditions: Vec<BenefitConditionDraft>,
}
fn empty_object() -> Value {
    Value::Object(Default::default())
}

async fn create_item(
    State(state): State<BenefitRestState>,
    headers: HeaderMap,
    Json(body): Json<CreateItemBody>,
) -> Result<(StatusCode, Json<BenefitCatalogItemView>), RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    require_feature(&principal, Feature::BenefitCatalogManage)?;
    let item = state
        .store
        .create_item(CreateBenefitCatalogItemCommand {
            actor: principal.user_id,
            branch_scope: principal.branch_scope,
            scope: body.scope,
            category: body.category,
            name: body.name,
            coverage_label: body.coverage_label,
            covered_count: body.covered_count,
            cost_label: body.cost_label,
            estimated_annual_cost_won: body.estimated_annual_cost_won,
            employer_rate_bps: body.employer_rate_bps,
            note: body.note,
            legal_basis: body.legal_basis,
            related_domain: body.related_domain,
            related_object_id: body.related_object_id,
            effective_on: body.effective_on,
            retires_on: body.retires_on,
            display_order: body.display_order,
            metadata: body.metadata,
            tiers: body.tiers,
            conditions: body.conditions,
            trace: TraceContext::generate(),
            occurred_at: time::OffsetDateTime::now_utc(),
        })
        .await
        .map_err(RestError::from_store)?;
    Ok((StatusCode::CREATED, Json(item)))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct UpdateItemBody {
    category: Option<BenefitCategory>,
    name: Option<String>,
    scope: Option<BenefitCatalogScopeDraft>,
    coverage_label: Option<String>,
    #[serde(default, deserialize_with = "double_option")]
    covered_count: Option<Option<i32>>,
    cost_label: Option<String>,
    #[serde(default, deserialize_with = "double_option")]
    estimated_annual_cost_won: Option<Option<i64>>,
    #[serde(default, deserialize_with = "double_option")]
    employer_rate_bps: Option<Option<i32>>,
    #[serde(default, deserialize_with = "double_option")]
    note: Option<Option<String>>,
    #[serde(default, deserialize_with = "double_option")]
    legal_basis: Option<Option<String>>,
    #[serde(default, deserialize_with = "double_option")]
    related_domain: Option<Option<String>>,
    #[serde(default, deserialize_with = "double_option")]
    related_object_id: Option<Option<Uuid>>,
    #[serde(default, deserialize_with = "double_option_iso_date")]
    effective_on: Option<Option<Date>>,
    #[serde(default, deserialize_with = "double_option_iso_date")]
    retires_on: Option<Option<Date>>,
    display_order: Option<i32>,
    metadata: Option<Value>,
}

impl From<UpdateItemBody> for UpdateBenefitCatalogItemFields {
    fn from(value: UpdateItemBody) -> Self {
        Self {
            category: value.category,
            name: value.name,
            scope: value.scope,
            coverage_label: value.coverage_label,
            covered_count: value.covered_count,
            cost_label: value.cost_label,
            estimated_annual_cost_won: value.estimated_annual_cost_won,
            employer_rate_bps: value.employer_rate_bps,
            note: value.note,
            legal_basis: value.legal_basis,
            related_domain: value.related_domain,
            related_object_id: value.related_object_id,
            effective_on: value.effective_on,
            retires_on: value.retires_on,
            display_order: value.display_order,
            metadata: value.metadata,
        }
    }
}

fn double_option<'de, D, T>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer).map(Some)
}

fn double_option_iso_date<'de, D>(deserializer: D) -> Result<Option<Option<Date>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    match Option::<String>::deserialize(deserializer)? {
        Some(value) => Date::parse(&value, &time::format_description::well_known::Iso8601::DATE)
            .map(|date| Some(Some(date)))
            .map_err(serde::de::Error::custom),
        None => Ok(Some(None)),
    }
}

async fn update_item(
    State(state): State<BenefitRestState>,
    headers: HeaderMap,
    Path(benefit_id): Path<Uuid>,
    Json(body): Json<UpdateItemBody>,
) -> Result<Json<BenefitCatalogItemView>, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    require_feature(&principal, Feature::BenefitCatalogManage)?;
    let item = state
        .store
        .update_item(UpdateBenefitCatalogItemCommand {
            actor: principal.user_id,
            branch_scope: principal.branch_scope,
            item_id: BenefitCatalogItemId::from_uuid(benefit_id),
            fields: body.into(),
            trace: TraceContext::generate(),
            occurred_at: time::OffsetDateTime::now_utc(),
        })
        .await
        .map_err(RestError::from_store)?;
    Ok(Json(item))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ReplaceTiersBody {
    tiers: Vec<BenefitTierDraft>,
}
async fn replace_tiers(
    State(state): State<BenefitRestState>,
    headers: HeaderMap,
    Path(benefit_id): Path<Uuid>,
    Json(body): Json<ReplaceTiersBody>,
) -> Result<Json<BenefitCatalogItemView>, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    require_feature(&principal, Feature::BenefitCatalogManage)?;
    let item = state
        .store
        .replace_tiers(ReplaceBenefitTiersCommand {
            actor: principal.user_id,
            branch_scope: principal.branch_scope,
            item_id: BenefitCatalogItemId::from_uuid(benefit_id),
            tiers: body.tiers,
            trace: TraceContext::generate(),
            occurred_at: time::OffsetDateTime::now_utc(),
        })
        .await
        .map_err(RestError::from_store)?;
    Ok(Json(item))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ReplaceConditionsBody {
    conditions: Vec<BenefitConditionDraft>,
}
async fn replace_conditions(
    State(state): State<BenefitRestState>,
    headers: HeaderMap,
    Path(benefit_id): Path<Uuid>,
    Json(body): Json<ReplaceConditionsBody>,
) -> Result<Json<BenefitCatalogItemView>, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    require_feature(&principal, Feature::BenefitCatalogManage)?;
    let item = state
        .store
        .replace_conditions(ReplaceBenefitConditionsCommand {
            actor: principal.user_id,
            branch_scope: principal.branch_scope,
            item_id: BenefitCatalogItemId::from_uuid(benefit_id),
            conditions: body.conditions,
            trace: TraceContext::generate(),
            occurred_at: time::OffsetDateTime::now_utc(),
        })
        .await
        .map_err(RestError::from_store)?;
    Ok(Json(item))
}

/// Benefit tiers and conditions carry no `branch_id`, so the `Branches` arm is a
/// capability decision. The `All` arm deliberately stays `authorize_org_wide` for
/// the same reason as `leave/rest` — collapsing it would widen for the
/// `{Admin} + BranchScope::All` group-admin principal.
///
/// TODO(branch-scope): `benefit_catalog_items.branch_id` IS present but NULLABLE
/// (migration 0157) and unread here; threading it is a separate lane.
fn require_feature(principal: &Principal, feature: Feature) -> Result<(), RestError> {
    let action = Action::new(feature);
    let result = match &principal.branch_scope {
        BranchScope::All => authorize_org_wide(principal, action),
        BranchScope::Branches(_) => authorize_capability(principal, action),
    };
    result.map_err(RestError::from_kernel)
}

#[derive(Debug, Serialize)]
struct ErrorBody {
    error: ErrorPayload,
}
#[derive(Debug, Serialize)]
struct ErrorPayload {
    code: &'static str,
    message: String,
}
#[derive(Debug)]
struct RestError {
    status: StatusCode,
    code: &'static str,
    message: String,
}
impl RestError {
    fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
        }
    }
    fn from_kernel(error: KernelError) -> Self {
        match error.kind {
            ErrorKind::Validation => Self::new(
                StatusCode::UNPROCESSABLE_ENTITY,
                "validation",
                error.message,
            ),
            ErrorKind::NotFound => Self::new(StatusCode::NOT_FOUND, "not_found", error.message),
            ErrorKind::Forbidden => Self::new(StatusCode::FORBIDDEN, "forbidden", error.message),
            ErrorKind::Conflict | ErrorKind::InvalidTransition => {
                Self::new(StatusCode::CONFLICT, "conflict", error.message)
            }
            ErrorKind::Internal => {
                Self::new(StatusCode::INTERNAL_SERVER_ERROR, "internal", error.message)
            }
        }
    }
    fn from_store(error: PgBenefitCatalogError) -> Self {
        match error {
            PgBenefitCatalogError::Domain(error) => Self::from_kernel(error),
            PgBenefitCatalogError::Db(_) => {
                tracing::error!(error = %error, "benefit catalog store error");
                Self::new(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal",
                    "internal server error",
                )
            }
        }
    }
}
impl IntoResponse for RestError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(ErrorBody {
                error: ErrorPayload {
                    code: self.code,
                    message: self.message,
                },
            }),
        )
            .into_response()
    }
}

async fn principal_from_headers(
    state: &BenefitRestState,
    headers: &HeaderMap,
) -> Result<Principal, RestError> {
    let verifier = state.jwt_verifier.as_ref().ok_or_else(|| {
        RestError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "unavailable",
            "JWT verification is not configured for the benefit API",
        )
    })?;
    console_platform_request_context::resolve_principal(verifier, state.store.pool(), headers)
        .await
        .map_err(|error| match error {
            RequestContextError::MissingBearer
            | RequestContextError::InvalidToken
            | RequestContextError::InvalidClaim(_) => RestError::new(
                StatusCode::UNAUTHORIZED,
                "unauthorized",
                "missing, malformed, or invalid bearer token",
            ),
            RequestContextError::WrongTokenTier | RequestContextError::AccessScope(_) => {
                RestError::from_kernel(KernelError::forbidden(
                    "token is not authorized for this benefit route",
                ))
            }
            RequestContextError::VerifierUnavailable => RestError::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "JWT verification is not configured for the benefit API",
            ),
            RequestContextError::BranchScope(message)
            | RequestContextError::EffectivePolicy(message) => {
                RestError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal", message)
            }
            RequestContextError::MissingOrg => RestError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal",
                "no tenant context is bound to the current request",
            ),
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use console_kernel_core::OrgId;
    use console_platform_authz::Role;
    use std::collections::BTreeSet;

    fn principal(role: Role) -> Principal {
        Principal::new(
            console_kernel_core::UserId::new(),
            OrgId::knl(),
            BTreeSet::from([role]),
            BranchScope::All,
        )
    }
    #[test]
    fn member_is_denied_benefit_read_and_manage() {
        let principal = principal(Role::Member);
        assert_eq!(
            require_feature(&principal, Feature::BenefitCatalogRead)
                .unwrap_err()
                .status,
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            require_feature(&principal, Feature::BenefitCatalogManage)
                .unwrap_err()
                .status,
            StatusCode::FORBIDDEN
        );
    }
    #[test]
    fn create_body_rejects_caller_supplied_tenant_identity() {
        let body = serde_json::json!({
            "orgId": "00000000-0000-0000-0000-000000000001",
            "scope": { "scope_type": "ORG" },
            "category": "legal",
            "name": "국민연금",
            "coverageLabel": "전 직원",
            "costLabel": "법정 부담",
        });
        assert!(serde_json::from_value::<CreateItemBody>(body).is_err());
    }

    #[test]
    fn route_surface_has_no_benefit_specific_lifecycle_transition() {
        assert!(
            BENEFIT_ROUTE_PATHS
                .iter()
                .all(|path| !path.contains("transition"))
        );
    }
}
