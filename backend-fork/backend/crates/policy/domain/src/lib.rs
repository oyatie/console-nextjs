//! Cedar policy catalog and draft-staging domain models.
//!
//! This crate is deliberately pure: no SQL, no request context, and no live
//! authorization switch. It models reviewable policy catalog/draft data only;
//! promotion to `shadow`/`enforced` belongs to a later governance lane.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

use console_kernel_core::{KernelError, OrgId, Timestamp, UserId};

macro_rules! wire_enum {
    (
        $(#[$enum_meta:meta])*
        pub enum $name:ident {
            $(
                $(#[$variant_meta:meta])*
                $variant:ident => $wire:literal
            ),+ $(,)?
        }
    ) => {
        $(#[$enum_meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
        #[serde(rename_all = "snake_case")]
        pub enum $name {
            $($(#[$variant_meta])* $variant,)+
        }

        impl $name {
            #[must_use]
            pub const fn as_db_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $wire,)+
                }
            }

            pub fn from_db_str(value: &str) -> Result<Self, KernelError> {
                match value {
                    $($wire => Ok(Self::$variant),)+
                    other => Err(KernelError::validation(format!(
                        "unknown {} value {other:?}",
                        stringify!($name)
                    ))),
                }
            }
        }
    };
}

wire_enum! {
    /// Cedar permit/forbid effect. Absence of a permit is still deny-by-omission.
    pub enum CedarPolicyEffect {
        Permit => "permit",
        Forbid => "forbid",
    }
}

wire_enum! {
    /// Catalog row lifecycle/source status.
    pub enum CedarPolicyStatus {
        Enforced => "enforced",
        Shadow => "shadow",
        Draft => "draft",
        ReviewPending => "review_pending",
        Rejected => "rejected",
        Retired => "retired",
    }
}

impl CedarPolicyStatus {
    /// True only for rows the live policy engine may consult after a separate
    /// promotion/governance lane. B16 draft saves must never create these.
    #[must_use]
    pub const fn is_runtime_enforced(self) -> bool {
        matches!(self, Self::Enforced | Self::Shadow)
    }
}

wire_enum! {
    /// Where a catalog row came from.
    pub enum CedarPolicySource {
        SystemGenerated => "system_generated",
        NoCodeDraft => "no_code_draft",
        PromotedPolicy => "promoted_policy",
        ImportedFixture => "imported_fixture",
    }
}

wire_enum! {
    /// Validation outcome for generated policy material.
    pub enum CedarValidationStatus {
        Valid => "valid",
        Invalid => "invalid",
    }
}

wire_enum! {
    /// Review lifecycle for staged no-code drafts.
    pub enum CedarPolicyReviewStatus {
        Draft => "draft",
        ReviewPending => "review_pending",
        Rejected => "rejected",
        ApprovedForPromotion => "approved_for_promotion",
    }
}

impl CedarPolicyReviewStatus {
    #[must_use]
    pub const fn catalog_status(self) -> CedarPolicyStatus {
        match self {
            Self::Draft => CedarPolicyStatus::Draft,
            Self::ReviewPending | Self::ApprovedForPromotion => CedarPolicyStatus::ReviewPending,
            Self::Rejected => CedarPolicyStatus::Rejected,
        }
    }
}

wire_enum! {
    pub enum CedarPrincipalKind {
        Role => "role",
        JobFunction => "job_function",
        User => "user",
        Team => "team",
        Branch => "branch",
        SelfPrincipal => "self",
        AllVisibleUsers => "all_visible_users",
    }
}

wire_enum! {
    pub enum CedarResourceScope {
        Org => "org",
        Branch => "branch",
        Team => "team",
        SelfScope => "self",
        Object => "object",
    }
}

wire_enum! {
    pub enum CedarConditionAttribute {
        Org => "org",
        Branch => "branch",
        Team => "team",
        EmploymentStatus => "employment_status",
        Purpose => "purpose",
        Location => "location",
        DevicePosture => "device_posture",
        SensitiveAction => "sensitive_action",
        ObjectLifecycle => "object_lifecycle",
        Classification => "classification",
    }
}

wire_enum! {
    pub enum CedarConditionOperator {
        Equals => "equals",
        NotEquals => "not_equals",
        In => "in",
        Contains => "contains",
        Present => "present",
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CedarPrincipalSelector {
    kind: CedarPrincipalKind,
    key: Option<String>,
    user_id: Option<UserId>,
    display_label: String,
}

impl CedarPrincipalSelector {
    pub fn new(
        kind: CedarPrincipalKind,
        key: Option<String>,
        user_id: Option<UserId>,
        display_label: impl Into<String>,
    ) -> Result<Self, KernelError> {
        let display_label =
            validate_display_label("principal.display_label", display_label.into())?;
        match kind {
            CedarPrincipalKind::User => {
                if user_id.is_none() {
                    return Err(KernelError::validation(
                        "user principal selector requires user_id",
                    ));
                }
                if key.is_some() {
                    return Err(KernelError::validation(
                        "user principal selector must not carry a client authority key",
                    ));
                }
            }
            CedarPrincipalKind::SelfPrincipal | CedarPrincipalKind::AllVisibleUsers => {
                if key.is_some() || user_id.is_some() {
                    return Err(KernelError::validation(
                        "self/all-visible principal selectors must not carry client authority ids",
                    ));
                }
            }
            CedarPrincipalKind::Role
            | CedarPrincipalKind::JobFunction
            | CedarPrincipalKind::Team
            | CedarPrincipalKind::Branch => {
                let Some(ref key) = key else {
                    return Err(KernelError::validation(
                        "role/job/team/branch principal selector requires key",
                    ));
                };
                validate_key("principal.key", key)?;
                if user_id.is_some() {
                    return Err(KernelError::validation(
                        "non-user principal selector must not carry user_id",
                    ));
                }
            }
        }
        Ok(Self {
            kind,
            key,
            user_id,
            display_label,
        })
    }

    #[must_use]
    pub const fn kind(&self) -> CedarPrincipalKind {
        self.kind
    }

    #[must_use]
    pub fn key(&self) -> Option<&str> {
        self.key.as_deref()
    }

    #[must_use]
    pub const fn user_id(&self) -> Option<UserId> {
        self.user_id
    }

    #[must_use]
    pub fn display_label(&self) -> &str {
        &self.display_label
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CedarActionSelector {
    action_key: String,
    display_label: String,
}

impl CedarActionSelector {
    pub fn new(
        action_key: impl Into<String>,
        display_label: impl Into<String>,
    ) -> Result<Self, KernelError> {
        let action_key = action_key.into();
        validate_key("action.action_key", &action_key)?;
        let display_label = validate_display_label("action.display_label", display_label.into())?;
        Ok(Self {
            action_key,
            display_label,
        })
    }

    #[must_use]
    pub fn action_key(&self) -> &str {
        &self.action_key
    }

    #[must_use]
    pub fn display_label(&self) -> &str {
        &self.display_label
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CedarResourceSelector {
    resource_type: String,
    resource_id: Option<String>,
    scope: CedarResourceScope,
    display_label: String,
}

impl CedarResourceSelector {
    pub fn new(
        resource_type: impl Into<String>,
        resource_id: Option<String>,
        scope: CedarResourceScope,
        display_label: impl Into<String>,
    ) -> Result<Self, KernelError> {
        let resource_type = resource_type.into();
        validate_key("resource.resource_type", &resource_type)?;
        if let Some(ref resource_id) = resource_id {
            validate_external_id("resource.resource_id", resource_id)?;
        }
        let display_label = validate_display_label("resource.display_label", display_label.into())?;
        Ok(Self {
            resource_type,
            resource_id,
            scope,
            display_label,
        })
    }

    #[must_use]
    pub fn resource_type(&self) -> &str {
        &self.resource_type
    }

    #[must_use]
    pub fn resource_id(&self) -> Option<&str> {
        self.resource_id.as_deref()
    }

    #[must_use]
    pub const fn scope(&self) -> CedarResourceScope {
        self.scope
    }

    #[must_use]
    pub fn display_label(&self) -> &str {
        &self.display_label
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CedarCondition {
    condition_key: String,
    attribute: CedarConditionAttribute,
    operator: CedarConditionOperator,
    values: Vec<String>,
    display_label: String,
}

impl CedarCondition {
    pub fn new(
        condition_key: impl Into<String>,
        attribute: CedarConditionAttribute,
        operator: CedarConditionOperator,
        values: Vec<String>,
        display_label: impl Into<String>,
    ) -> Result<Self, KernelError> {
        let condition_key = condition_key.into();
        validate_key("condition.condition_key", &condition_key)?;
        if operator != CedarConditionOperator::Present && values.is_empty() {
            return Err(KernelError::validation(
                "condition values are required unless operator is present",
            ));
        }
        for value in &values {
            validate_condition_value(value)?;
        }
        let display_label =
            validate_display_label("condition.display_label", display_label.into())?;
        Ok(Self {
            condition_key,
            attribute,
            operator,
            values,
            display_label,
        })
    }

    #[must_use]
    pub fn condition_key(&self) -> &str {
        &self.condition_key
    }

    #[must_use]
    pub const fn attribute(&self) -> CedarConditionAttribute {
        self.attribute
    }

    #[must_use]
    pub const fn operator(&self) -> CedarConditionOperator {
        self.operator
    }

    #[must_use]
    pub fn values(&self) -> &[String] {
        &self.values
    }

    #[must_use]
    pub fn display_label(&self) -> &str {
        &self.display_label
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CedarValidationError {
    pub field: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CedarPolicyBlocks {
    pub principal: CedarPrincipalSelector,
    pub action: CedarActionSelector,
    pub resource: CedarResourceSelector,
    pub effect: CedarPolicyEffect,
    pub conditions: Vec<CedarCondition>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CedarPolicyCatalogRow {
    pub id: uuid::Uuid,
    pub stable_key: String,
    pub title: String,
    pub natural_language_rule: String,
    pub effect: CedarPolicyEffect,
    pub status: CedarPolicyStatus,
    pub source: CedarPolicySource,
    pub principal: CedarPrincipalSelector,
    pub action: CedarActionSelector,
    pub resource: CedarResourceSelector,
    pub conditions: Vec<CedarCondition>,
    pub engine_mode: Option<String>,
    pub policy_version: Option<i64>,
    pub schema_version: Option<String>,
    pub bundle_digest: Option<String>,
    pub cedar_sdk_version: Option<String>,
    pub cedar_language_version: Option<String>,
    pub validation_status: CedarValidationStatus,
    pub created_by: Option<UserId>,
    pub updated_by: Option<UserId>,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
}

impl CedarPolicyCatalogRow {
    #[must_use]
    pub fn rule_text_effect_status(&self) -> (&str, CedarPolicyEffect, CedarPolicyStatus) {
        (&self.natural_language_rule, self.effect, self.status)
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CedarPolicyDraft {
    pub id: uuid::Uuid,
    pub org_id: OrgId,
    pub draft_key: String,
    pub title: String,
    pub author_note: Option<String>,
    pub blocks: CedarPolicyBlocks,
    pub catalog_row: CedarPolicyCatalogRow,
    pub generated_policy_text: String,
    pub generated_policy_digest: String,
    pub validation_status: CedarValidationStatus,
    pub validation_errors: Vec<CedarValidationError>,
    pub review_status: CedarPolicyReviewStatus,
    pub reviewer_id: Option<UserId>,
    pub review_note: Option<String>,
    pub created_by: UserId,
    pub updated_by: UserId,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
}

fn validate_key(field: &str, value: &str) -> Result<(), KernelError> {
    let valid = !value.is_empty()
        && value.len() <= 128
        && value
            .chars()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_' || ch == '.');
    if valid {
        Ok(())
    } else {
        Err(KernelError::validation(format!(
            "{field} must be 1..128 lowercase ascii key characters"
        )))
    }
}

fn validate_external_id(field: &str, value: &str) -> Result<(), KernelError> {
    let valid = !value.trim().is_empty() && value.len() <= 160 && !value.contains('\0');
    if valid {
        Ok(())
    } else {
        Err(KernelError::validation(format!(
            "{field} must be non-empty, <=160 chars, and contain no NUL byte"
        )))
    }
}

fn validate_display_label(field: &str, value: String) -> Result<String, KernelError> {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.chars().count() > 120 || trimmed.contains('\0') {
        return Err(KernelError::validation(format!(
            "{field} must be 1..120 display characters"
        )));
    }
    Ok(trimmed.to_owned())
}

fn validate_condition_value(value: &str) -> Result<(), KernelError> {
    if value.trim().is_empty() || value.chars().count() > 120 || value.contains('\0') {
        Err(KernelError::validation(
            "condition values must be 1..120 chars and contain no NUL byte",
        ))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn draft_statuses_are_never_runtime_enforced() {
        assert!(!CedarPolicyStatus::Draft.is_runtime_enforced());
        assert!(!CedarPolicyStatus::ReviewPending.is_runtime_enforced());
        assert!(CedarPolicyStatus::Enforced.is_runtime_enforced());
    }

    #[test]
    fn selectors_reject_client_supplied_authority_fields() {
        let resource = CedarResourceSelector::new(
            "attendance_record",
            None,
            CedarResourceScope::Team,
            "소속 팀원 근태",
        )
        .expect("valid resource selector");
        assert_eq!(resource.resource_type(), "attendance_record");

        let invalid = CedarPrincipalSelector::new(
            CedarPrincipalKind::SelfPrincipal,
            Some("admin".to_owned()),
            None,
            "나",
        );
        assert!(invalid.is_err());
    }

    #[test]
    fn cedar_policy_status_wire_roundtrips_and_runtime_enforcement() {
        for st in [
            CedarPolicyStatus::Enforced,
            CedarPolicyStatus::Shadow,
            CedarPolicyStatus::Draft,
            CedarPolicyStatus::ReviewPending,
            CedarPolicyStatus::Rejected,
            CedarPolicyStatus::Retired,
        ] {
            assert_eq!(CedarPolicyStatus::from_db_str(st.as_db_str()).unwrap(), st);
        }
        assert!(CedarPolicyStatus::Shadow.is_runtime_enforced());
        assert!(!CedarPolicyStatus::Rejected.is_runtime_enforced());
        assert!(!CedarPolicyStatus::Retired.is_runtime_enforced());
        assert!(CedarPolicyStatus::from_db_str("not_a_status").is_err());
    }

    #[test]
    fn validate_key_rejects_empty_uppercase_and_overlong() {
        assert!(validate_key("action", "clock.in").is_ok());
        assert!(validate_key("action", "").is_err());
        assert!(validate_key("action", "ClockIn").is_err());
        assert!(validate_key("action", &"a".repeat(129)).is_err());
    }

    #[test]
    fn effect_and_validation_status_roundtrip() {
        assert_eq!(
            CedarPolicyEffect::from_db_str(CedarPolicyEffect::Permit.as_db_str()).unwrap(),
            CedarPolicyEffect::Permit
        );
        assert_eq!(
            CedarPolicyEffect::from_db_str(CedarPolicyEffect::Forbid.as_db_str()).unwrap(),
            CedarPolicyEffect::Forbid
        );
        assert!(CedarPolicyEffect::from_db_str("allow").is_err());
        assert_eq!(
            CedarValidationStatus::from_db_str("valid").unwrap(),
            CedarValidationStatus::Valid
        );
        assert_eq!(
            CedarValidationStatus::from_db_str("invalid").unwrap(),
            CedarValidationStatus::Invalid
        );
    }

    /// Fail-closed staging invariant: review lifecycle never maps into catalog
    /// statuses the live engine may consult (`enforced`/`shadow`). Promotion is
    /// a separate governance lane; domain mapping must not short-circuit it.
    #[test]
    fn review_status_catalog_mapping_never_runtime_enforced() {
        for review in [
            CedarPolicyReviewStatus::Draft,
            CedarPolicyReviewStatus::ReviewPending,
            CedarPolicyReviewStatus::Rejected,
            CedarPolicyReviewStatus::ApprovedForPromotion,
        ] {
            let catalog = review.catalog_status();
            assert!(
                !catalog.is_runtime_enforced(),
                "review_status {review:?} must not map to runtime-enforced catalog status {catalog:?}"
            );
        }
        assert_eq!(
            CedarPolicyReviewStatus::ApprovedForPromotion.catalog_status(),
            CedarPolicyStatus::ReviewPending
        );
        // Wire values that would smuggle engine status into review lifecycle fail closed.
        assert!(CedarPolicyReviewStatus::from_db_str("enforced").is_err());
        assert!(CedarPolicyReviewStatus::from_db_str("shadow").is_err());
        assert!(CedarPolicyReviewStatus::from_db_str("not_a_review").is_err());
    }

    /// Constructors reject incomplete authority/condition material rather than
    /// constructing a half-open policy block that could later be treated as valid.
    #[test]
    fn condition_and_principal_constructors_fail_closed() {
        assert!(
            CedarCondition::new(
                "org_eq",
                CedarConditionAttribute::Org,
                CedarConditionOperator::Equals,
                Vec::new(),
                "org equals",
            )
            .is_err(),
            "non-present operators require at least one value"
        );
        assert!(
            CedarCondition::new(
                "device_posture",
                CedarConditionAttribute::DevicePosture,
                CedarConditionOperator::Present,
                Vec::new(),
                "device posture present",
            )
            .is_ok(),
            "present operator may omit values"
        );
        assert!(
            CedarCondition::new(
                "org_eq",
                CedarConditionAttribute::Org,
                CedarConditionOperator::Equals,
                vec!["   ".to_owned()],
                "org equals",
            )
            .is_err(),
            "blank condition values fail closed"
        );

        assert!(
            CedarPrincipalSelector::new(CedarPrincipalKind::User, None, None, "user").is_err(),
            "user principal requires user_id"
        );
        assert!(
            CedarPrincipalSelector::new(CedarPrincipalKind::Role, None, None, "role").is_err(),
            "role principal requires key"
        );
        assert!(
            CedarPrincipalSelector::new(
                CedarPrincipalKind::User,
                Some("spoofed_role".to_owned()),
                Some(UserId::from_uuid(uuid::Uuid::from_u128(0x11))),
                "user",
            )
            .is_err(),
            "user principal must not carry a client authority key"
        );

        assert!(CedarPrincipalKind::from_db_str("superadmin").is_err());
        assert!(CedarConditionOperator::from_db_str("regex").is_err());
        assert!(CedarResourceScope::from_db_str("universe").is_err());
    }
}
