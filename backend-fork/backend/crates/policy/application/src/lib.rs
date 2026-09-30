//! Cedar policy application layer.
//!
//! Owns commands, validation/orchestration, generated staging artifacts, audit
//! event builders, and storage ports. It deliberately does not depend on SQL,
//! request context, or platform authorization crates so clean-architecture layer
//! gates stay intact.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

use std::future::Future;
use std::pin::Pin;

use console_kernel_core::{
    AuditAction, AuditEvent, KernelError, OrgId, Timestamp, TraceContext, UserId,
};
use console_policy_domain::{
    CedarActionSelector, CedarCondition, CedarPolicyBlocks, CedarPolicyCatalogRow,
    CedarPolicyDraft, CedarPolicyEffect, CedarPolicyReviewStatus, CedarPolicySource,
    CedarPolicyStatus, CedarPrincipalSelector, CedarResourceSelector, CedarValidationError,
    CedarValidationStatus,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub type PolicyPortFuture<'a, T> =
    Pin<Box<dyn Future<Output = Result<T, KernelError>> + Send + 'a>>;

/// Storage port implemented by outer adapters.
pub trait CedarPolicyStore {
    fn list_catalog_rows<'a>(
        &'a self,
        query: CedarPolicyCatalogQuery,
    ) -> PolicyPortFuture<'a, CedarPolicyCatalogPage>;

    fn save_draft<'a>(
        &'a self,
        command: CedarPolicyDraftSaveCommand,
    ) -> PolicyPortFuture<'a, CedarPolicyDraftSaveResponse>;
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CedarPolicyCatalogQuery {
    pub status: Option<CedarPolicyStatus>,
    pub source: Option<CedarPolicySource>,
    pub resource_type: Option<String>,
    pub action_key: Option<String>,
    pub effect: Option<CedarPolicyEffect>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

impl CedarPolicyCatalogQuery {
    pub fn normalized(self) -> Result<Self, KernelError> {
        let limit = self.limit.unwrap_or(50).clamp(1, 100);
        let offset = self.offset.unwrap_or(0);
        if offset < 0 {
            return Err(KernelError::validation(
                "catalog offset must be non-negative",
            ));
        }
        validate_optional_filter_key("resource_type", self.resource_type.as_deref())?;
        validate_optional_filter_key("action_key", self.action_key.as_deref())?;
        Ok(Self {
            limit: Some(limit),
            offset: Some(offset),
            ..self
        })
    }

    #[must_use]
    pub fn limit_value(&self) -> i64 {
        self.limit.unwrap_or(50)
    }

    #[must_use]
    pub fn offset_value(&self) -> i64 {
        self.offset.unwrap_or(0)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CedarPolicyCatalogPage {
    pub items: Vec<CedarPolicyCatalogRow>,
    pub limit: i64,
    pub offset: i64,
    pub total: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CedarPolicyDraftSaveMode {
    Draft,
    ReviewPending,
}

impl CedarPolicyDraftSaveMode {
    #[must_use]
    pub const fn review_status(self) -> CedarPolicyReviewStatus {
        match self {
            Self::Draft => CedarPolicyReviewStatus::Draft,
            Self::ReviewPending => CedarPolicyReviewStatus::ReviewPending,
        }
    }
}

#[derive(Debug, Clone)]
pub struct CedarPolicyDraftSaveCommand {
    pub actor: UserId,
    pub title: String,
    pub author_note: Option<String>,
    pub principal: CedarPrincipalSelector,
    pub action: CedarActionSelector,
    pub resource: CedarResourceSelector,
    pub effect: CedarPolicyEffect,
    pub conditions: Vec<CedarCondition>,
    pub save_mode: CedarPolicyDraftSaveMode,
    pub trace: TraceContext,
    pub occurred_at: Timestamp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EnforcementEffect {
    None,
}

impl EnforcementEffect {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CedarPolicyDraftSaveResponse {
    pub draft: CedarPolicyDraft,
    pub enforcement_effect: EnforcementEffect,
    pub audit_trace_id: String,
    pub next_actions: Vec<String>,
}

impl CedarPolicyDraftSaveResponse {
    #[must_use]
    pub fn enforcement_effect(&self) -> &'static str {
        self.enforcement_effect.as_str()
    }
}

pub fn build_draft_artifact(
    org_id: OrgId,
    command: CedarPolicyDraftSaveCommand,
) -> Result<CedarPolicyDraft, KernelError> {
    let validation_errors = validate_for_review(&command);
    let title = normalize_title(command.title)?;
    let author_note = normalize_author_note(command.author_note)?;
    let validation_status = if validation_errors.is_empty() {
        CedarValidationStatus::Valid
    } else {
        CedarValidationStatus::Invalid
    };
    if command.save_mode == CedarPolicyDraftSaveMode::ReviewPending
        && validation_status != CedarValidationStatus::Valid
    {
        return Err(KernelError::validation(
            "review_pending Cedar policy drafts require strict validation",
        ));
    }

    let id = uuid::Uuid::new_v4();
    let blocks = CedarPolicyBlocks {
        principal: command.principal,
        action: command.action,
        resource: command.resource,
        effect: command.effect,
        conditions: command.conditions,
    };
    let stable_key = stable_catalog_key(&blocks);
    let draft_key = format!("{stable_key}.draft.{}", &id.simple().to_string()[..12]);
    let natural_language_rule = natural_language_rule(&blocks);
    let generated_policy_text = generated_policy_text(&blocks, &natural_language_rule);
    let generated_policy_digest = sha256_digest(&generated_policy_text);
    let review_status = command.save_mode.review_status();
    let catalog_status = review_status.catalog_status();
    let catalog_row = CedarPolicyCatalogRow {
        id,
        stable_key: draft_key.clone(),
        title: title.clone(),
        natural_language_rule,
        effect: blocks.effect,
        status: catalog_status,
        source: CedarPolicySource::NoCodeDraft,
        principal: blocks.principal.clone(),
        action: blocks.action.clone(),
        resource: blocks.resource.clone(),
        conditions: blocks.conditions.clone(),
        engine_mode: None,
        policy_version: None,
        schema_version: None,
        bundle_digest: None,
        cedar_sdk_version: None,
        cedar_language_version: None,
        validation_status,
        created_by: Some(command.actor),
        updated_by: Some(command.actor),
        created_at: command.occurred_at,
        updated_at: command.occurred_at,
    };

    Ok(CedarPolicyDraft {
        id,
        org_id,
        draft_key,
        title,
        author_note,
        blocks,
        catalog_row,
        generated_policy_text,
        generated_policy_digest,
        validation_status,
        validation_errors,
        review_status,
        reviewer_id: None,
        review_note: None,
        created_by: command.actor,
        updated_by: command.actor,
        created_at: command.occurred_at,
        updated_at: command.occurred_at,
    })
}

pub fn draft_create_audit_event(
    actor: UserId,
    trace: TraceContext,
    occurred_at: Timestamp,
    draft: &CedarPolicyDraft,
) -> Result<AuditEvent, KernelError> {
    let after = draft_audit_snapshot(draft)?;
    Ok(AuditEvent::new(
        Some(actor),
        AuditAction::new("policy.cedar_draft.create")?,
        "cedar_policy_draft",
        draft.id.to_string(),
        trace,
        occurred_at,
    )
    .with_snapshots(None, Some(after)))
}

fn draft_audit_snapshot(draft: &CedarPolicyDraft) -> Result<serde_json::Value, KernelError> {
    serde_json::to_value(serde_json::json!({
        "draft_id": draft.id,
        "draft_key": draft.draft_key,
        "title": draft.title,
        "effect": draft.blocks.effect.as_db_str(),
        "review_status": draft.review_status.as_db_str(),
        "validation_status": draft.validation_status.as_db_str(),
        "validation_errors": draft.validation_errors,
        "generated_policy_digest": draft.generated_policy_digest,
        "enforcement_effect": EnforcementEffect::None.as_str(),
    }))
    .map_err(|err| {
        KernelError::internal(format!(
            "failed to serialize policy draft audit snapshot: {err}"
        ))
    })
}

fn normalize_title(value: String) -> Result<String, KernelError> {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.chars().count() > 120 || trimmed.contains('\0') {
        return Err(KernelError::validation(
            "title must be 1..120 characters and contain no NUL byte",
        ));
    }
    Ok(trimmed.to_owned())
}

fn normalize_author_note(value: Option<String>) -> Result<Option<String>, KernelError> {
    value
        .map(|note| {
            let trimmed = note.trim();
            if trimmed.is_empty() {
                return Ok(None);
            }
            if trimmed.chars().count() > 512 || trimmed.contains('\0') {
                return Err(KernelError::validation(
                    "author_note must be <=512 characters and contain no NUL byte",
                ));
            }
            Ok(Some(trimmed.to_owned()))
        })
        .transpose()
        .map(Option::flatten)
}

fn validate_for_review(command: &CedarPolicyDraftSaveCommand) -> Vec<CedarValidationError> {
    let mut errors = Vec::new();
    if command.save_mode == CedarPolicyDraftSaveMode::ReviewPending && command.conditions.len() > 16
    {
        errors.push(CedarValidationError {
            field: "conditions".to_owned(),
            message: "review_pending drafts support at most 16 conditions in v1".to_owned(),
        });
    }
    errors
}

fn stable_catalog_key(blocks: &CedarPolicyBlocks) -> String {
    let principal_key = blocks
        .principal
        .key()
        .map(ToOwned::to_owned)
        .or_else(|| blocks.principal.user_id().map(|id| id.to_string()))
        .unwrap_or_else(|| blocks.principal.kind().as_db_str().to_owned());
    format!(
        "{}.{}.{}.{}",
        blocks.resource.resource_type(),
        sanitize_key_segment(&principal_key),
        sanitize_key_segment(blocks.action.action_key()),
        blocks.effect.as_db_str()
    )
}

fn sanitize_key_segment(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_' || ch == '.' {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

fn natural_language_rule(blocks: &CedarPolicyBlocks) -> String {
    let verb = match blocks.effect {
        CedarPolicyEffect::Permit => "can perform",
        CedarPolicyEffect::Forbid => "must not perform",
    };
    format!(
        "{} {verb} {} on {}.",
        blocks.principal.display_label(),
        blocks.action.display_label(),
        blocks.resource.display_label()
    )
}

fn generated_policy_text(blocks: &CedarPolicyBlocks, natural_language_rule: &str) -> String {
    let effect = match blocks.effect {
        CedarPolicyEffect::Permit => "permit",
        CedarPolicyEffect::Forbid => "forbid",
    };
    format!(
        "// Generated staging-only Cedar policy; not live enforcement.\n// {natural_language_rule}\n{effect}(principal, action, resource) when {{ context.action_key == \"{}\" && context.resource_type == \"{}\" }};\n",
        blocks.action.action_key(),
        blocks.resource.resource_type()
    )
}

fn sha256_digest(value: &str) -> String {
    let digest = Sha256::digest(value.as_bytes());
    format!("sha256:{}", hex::encode(digest))
}

fn validate_optional_filter_key(field: &str, value: Option<&str>) -> Result<(), KernelError> {
    if let Some(value) = value {
        let valid = !value.is_empty()
            && value.len() <= 128
            && value
                .chars()
                .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_' || ch == '.');
        if !valid {
            return Err(KernelError::validation(format!(
                "{field} filter must be a lowercase ascii key"
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use console_kernel_core::{OrgId, TraceContext, UserId};
    use console_policy_domain::{
        CedarActionSelector, CedarPolicyEffect, CedarPrincipalKind, CedarPrincipalSelector,
        CedarResourceScope, CedarResourceSelector,
    };
    use time::macros::datetime;

    #[test]
    fn draft_save_builds_reviewable_artifact_without_enforcement_effect() {
        let command = CedarPolicyDraftSaveCommand {
            actor: UserId::from_uuid(uuid::Uuid::from_u128(0xaa)),
            title: "팀장 소속팀 근태 열람".to_owned(),
            author_note: Some("canvas draft".to_owned()),
            principal: CedarPrincipalSelector::new(
                CedarPrincipalKind::JobFunction,
                Some("team_lead".to_owned()),
                None,
                "직책 · 팀장",
            )
            .unwrap(),
            action: CedarActionSelector::new("attendance_read", "근태 열람").unwrap(),
            resource: CedarResourceSelector::new(
                "attendance_record",
                None,
                CedarResourceScope::Team,
                "소속 팀원 근태",
            )
            .unwrap(),
            effect: CedarPolicyEffect::Permit,
            conditions: Vec::new(),
            save_mode: CedarPolicyDraftSaveMode::Draft,
            trace: TraceContext::generate(),
            occurred_at: datetime!(2026-07-09 12:00 UTC),
        };

        let draft = build_draft_artifact(OrgId::from_uuid(uuid::Uuid::from_u128(0xa1)), command)
            .expect("draft artifact");

        assert_eq!(draft.review_status.as_db_str(), "draft");
        assert_eq!(draft.catalog_row.status.as_db_str(), "draft");
        assert!(!draft.catalog_row.status.is_runtime_enforced());
        assert!(draft.generated_policy_digest.starts_with("sha256:"));
        assert!(draft.generated_policy_text.contains("staging-only"));
    }
}
