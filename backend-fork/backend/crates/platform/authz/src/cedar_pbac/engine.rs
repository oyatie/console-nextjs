//! Real Cedar bundle compilation + evaluation for the Cedar/PBAC boundary.
//!
//! This module compiles schema-validated Cedar bundles from the legacy
//! permission matrix (one bundle per enrolled [`Feature`]) and evaluates an
//! [`AuthorizationRequest`] into a real [`CedarEvaluation`]. Most production
//! paths still use the legacy boundary or shadow lane; selected
//! high-sensitivity surfaces (for example the CEO/top-clearance covert audit
//! stream) can opt into Cedar-only enforcement with server-loaded facts.
//!
//! Two safety properties are load-bearing:
//!  * **Fail-closed compile.** [`compile_bundle`] rejects the bundle on ANY
//!    strict-validation error OR warning, so a schema/policy that does not
//!    typecheck can never activate.
//!  * **Nothing escapes evaluation.** [`evaluate`] wraps the whole Cedar call in
//!    a `Result` plus [`std::panic::catch_unwind`]; any error, parse failure, or
//!    Cedar panic becomes a recorded [`CedarEvaluation::Error`] rather than a
//!    thrown exception that could alter a live outcome.

use std::collections::{HashMap, HashSet};
use std::panic::AssertUnwindSafe;
use std::str::FromStr;

use cedar_policy::{
    Authorizer, Context, Decision, Entities, Entity, EntityId, EntityTypeName, EntityUid,
    PolicySet, Request, RestrictedExpression, Schema, ValidationMode, Validator,
};
use console_kernel_core::{KernelError, OrgId};
use sha2::{Digest, Sha256};

use super::{AuthorizationRequest, CedarEvaluation, CompiledBundleCacheKey};
use crate::{Feature, PermissionLevel, Role, permission_for};

/// Cedar SDK version pinned in `authz/Cargo.toml` (`cedar-policy = "=4.11.2"`).
/// Recorded on the compiled-bundle cache key so a Cedar upgrade re-keys bundles.
pub const CEDAR_SDK_VERSION: &str = "4.11.2";

/// Cedar policy language version implemented by [`CEDAR_SDK_VERSION`].
pub const CEDAR_LANGUAGE_VERSION: &str = "4.5";

/// Hand-picked schema identity for the RoleManage pilot bundle. Bump this string
/// whenever [`ROLE_MANAGE_SCHEMA`] changes so cache keys never collide across
/// schema shapes.
pub const ROLE_MANAGE_SCHEMA_VERSION: &str = "2026-07-role-manage-v1";

macro_rules! shared_entity_schema {
    () => {
        r#"entity Subject = {
  "org": String,
  "roles": Set<String>,
  "subject_version": Long,
  "clearance_keys": Set<String>
};

entity Resource = {
  "org": String,
  "resource_type": String,
  "resource_id"?: String,
  "branch"?: String
};"#
    };
}

/// Shared server-derived entity definitions used by every enrolled Cedar schema.
pub const SHARED_ENTITY_SCHEMA: &str = shared_entity_schema!();

const CONTEXT_SCHEMA: &str = r#"context: {
  "purpose"?: String,
  "channel"?: String,
  "step_up"?: Bool
}"#;

/// Cedar schema (human-readable `.cedarschema` format) for the RoleManage pilot.
///
/// Entities and context carry only SERVER-derived material: a UI/JWT projection
/// can never fabricate the `roles`/`subject_version`/`clearance_keys` a permit
/// depends on. The action id is [`Feature::as_str`] (`role_manage`) so the
/// generated policy and the evaluated request address the same Cedar action.
pub const ROLE_MANAGE_SCHEMA: &str = concat!(
    shared_entity_schema!(),
    "\n\n",
    r#"action role_manage appliesTo {
  principal: [Subject],
  resource: [Resource],
  context: {
    "purpose"?: String,
    "channel"?: String,
    "step_up"?: Bool
  }
};
"#,
);

/// Schema identity for the B26b CEO/top-clearance covert audit stream bundle.
pub const AUDIT_STREAM_SCHEMA_VERSION: &str = "2026-07-ceo-covert-audit-stream-v1";

/// Tenant-owned stream id for the first covert audit stream. The public REST
/// path uses the hyphenated slug; Cedar/resource labels stay snake_case.
pub const CEO_COVERT_AUDIT_STREAM_KEY: &str = "ceo_covert_audit";

/// Clearance fact required to read the CEO/top-clearance covert stream.
pub const CEO_COVERT_AUDIT_CLEARANCE_KEY: &str = "audit.ceo_covert.read";

pub const AUDIT_STREAM_SCHEMA: &str = r#"entity Subject = {
  "org": String,
  "roles": Set<String>,
  "subject_version": Long,
  "clearance_keys": Set<String>
};

entity Resource = {
  "org": String,
  "resource_type": String,
  "resource_id": String,
  "branch"?: String
};

action audit_stream_read appliesTo {
  principal: [Subject],
  resource: [Resource],
  context: {
    "purpose"?: String,
    "channel"?: String
  }
};

action audit_stream_access_log_read appliesTo {
  principal: [Subject],
  resource: [Resource],
  context: {
    "purpose"?: String,
    "channel"?: String
  }
};
"#;

/// Parsed + strict-validated Cedar bundle material for one enrolled action.
///
/// This is bundle identity + compiled artifacts, not a decision cache. The key
/// is the only thing the coexistence map compares; `schema`/`policies` are the
/// inputs [`evaluate`] runs the authorizer against.
#[derive(Debug, Clone)]
pub struct CompiledBundle {
    pub key: CompiledBundleCacheKey,
    schema: Schema,
    policies: PolicySet,
}

/// Generate the Cedar policy text for `feature` directly from the legacy
/// permission matrix.
///
/// One `permit` is emitted per role whose matrix cell for `feature` is
/// [`PermissionLevel::Allow`] (`Role::ALL` order). Generating from
/// [`permission_for`] — rather than hand-authoring a parallel ruleset — keeps
/// Cedar and the legacy matrix provably equivalent. For [`Feature::RoleManage`]
/// this is exactly one rule (SUPER_ADMIN).
#[must_use]
pub fn generate_policies(feature: Feature) -> String {
    let action = feature.as_str();
    let mut policies = String::new();
    for role in Role::ALL {
        if permission_for(role, feature) == PermissionLevel::Allow {
            policies.push_str(&format!(
                "permit(\n  principal,\n  action == Action::\"{action}\",\n  resource\n)\nwhen {{ principal.roles.contains(\"{role}\") }};\n",
                role = role.as_str()
            ));
        }
    }
    policies
}

/// Compile the RoleManage pilot bundle for `org_id` at `policy_version`.
///
/// Fail-closed: returns `Err` on any parse error or on ANY strict-validation
/// error OR warning (see [`compile_bundle_from_sources`]).
///
/// Panic-safe like [`evaluate`]: a Cedar-SDK panic during schema/policy compile or
/// strict validation is caught and recorded as an `Err`, never unwound out of the
/// shadow lane. This upholds the ADR-0021 invariant that the shadow lane can never
/// alter (or abort) a live authorization outcome.
pub fn compile_bundle(org_id: OrgId, policy_version: u64) -> Result<CompiledBundle, KernelError> {
    match std::panic::catch_unwind(AssertUnwindSafe(|| {
        compile_bundle_inner(org_id, policy_version)
    })) {
        Ok(result) => result,
        Err(_) => Err(KernelError::validation(
            "cedar bundle compilation panicked".to_owned(),
        )),
    }
}

/// Cedar schema for an arbitrary enrolled `feature`, mirroring
/// [`ROLE_MANAGE_SCHEMA`] but with the declared action id bound to
/// [`Feature::as_str`]. The server-derived `Subject`/`Resource`/context shape is
/// identical; only the action name changes, so an enrolled feature addresses
/// exactly its own Cedar action and cannot match another feature's permits.
#[must_use]
pub fn feature_schema(feature: Feature) -> String {
    format!(
        "{SHARED_ENTITY_SCHEMA}\n\naction \"{action}\" appliesTo {{\n  principal: [Subject],\n  resource: [Resource],\n  {CONTEXT_SCHEMA}\n}};\n",
        action = feature.as_str()
    )
}

/// Schema identity for a per-feature bundle, distinct per feature so cache keys
/// never collide across features/actions.
#[must_use]
pub fn feature_schema_version(feature: Feature) -> String {
    format!("2026-07-feature-{}-v1", feature.as_str())
}

/// Compile a strict-validated Cedar bundle for an arbitrary enrolled `feature`,
/// with `permit`s generated from the legacy permission matrix (so Cedar and the
/// matrix stay provably equivalent). Panic-safe and fail-closed exactly like
/// [`compile_bundle`]; this is the enrollment-wave-2 generalization of the
/// RoleManage pilot to the workflow decide-path and object-resolve features.
pub fn compile_bundle_for_feature(
    org_id: OrgId,
    policy_version: u64,
    feature: Feature,
) -> Result<CompiledBundle, KernelError> {
    match std::panic::catch_unwind(AssertUnwindSafe(|| {
        compile_bundle_from_sources(
            org_id,
            policy_version,
            &feature_schema_version(feature),
            &feature_schema(feature),
            &generate_policies(feature),
        )
    })) {
        Ok(result) => result,
        Err(_) => Err(KernelError::validation(
            "cedar feature bundle compilation panicked".to_owned(),
        )),
    }
}

/// Compile the B26b CEO/top-clearance covert audit stream bundle.
///
/// This bundle has no legacy role-matrix allow path: both stream read actions
/// require the server-loaded clearance key on the Cedar subject, the same tenant
/// org on subject/resource, and the fixed CEO covert audit stream resource id.
pub fn compile_audit_stream_bundle(
    org_id: OrgId,
    policy_version: u64,
) -> Result<CompiledBundle, KernelError> {
    match std::panic::catch_unwind(AssertUnwindSafe(|| {
        compile_audit_stream_bundle_inner(org_id, policy_version)
    })) {
        Ok(result) => result,
        Err(_) => Err(KernelError::validation(
            "cedar audit-stream bundle compilation panicked".to_owned(),
        )),
    }
}

fn compile_audit_stream_bundle_inner(
    org_id: OrgId,
    policy_version: u64,
) -> Result<CompiledBundle, KernelError> {
    compile_bundle_from_sources(
        org_id,
        policy_version,
        AUDIT_STREAM_SCHEMA_VERSION,
        AUDIT_STREAM_SCHEMA,
        &generate_audit_stream_policies(),
    )
}

#[must_use]
pub fn generate_audit_stream_policies() -> String {
    let mut policies = String::new();
    for feature in [Feature::AuditStreamRead, Feature::AuditStreamAccessLogRead] {
        let action = feature.as_str();
        policies.push_str(&format!(
            "permit(\n  principal,\n  action == Action::\"{action}\",\n  resource\n)\nwhen {{\n  principal.org == resource.org &&\n  resource.resource_type == \"audit_stream\" &&\n  resource.resource_id == \"{stream}\" &&\n  principal.clearance_keys.contains(\"{clearance}\")\n}};\n",
            stream = CEO_COVERT_AUDIT_STREAM_KEY,
            clearance = CEO_COVERT_AUDIT_CLEARANCE_KEY
        ));
    }
    policies
}

fn compile_bundle_inner(org_id: OrgId, policy_version: u64) -> Result<CompiledBundle, KernelError> {
    compile_bundle_from_sources(
        org_id,
        policy_version,
        ROLE_MANAGE_SCHEMA_VERSION,
        ROLE_MANAGE_SCHEMA,
        &generate_policies(Feature::RoleManage),
    )
}

/// Parse `schema_src` + `policy_src`, run strict validation, and build the
/// compiled bundle with its cache key.
///
/// Any parse failure, or ANY strict-validation error OR warning, returns `Err`
/// (the schema-backed rejection): the bundle is "not activated" and callers
/// treat a missing bundle as `BundleUnavailable`.
pub fn compile_bundle_from_sources(
    org_id: OrgId,
    policy_version: u64,
    schema_version: &str,
    schema_src: &str,
    policy_src: &str,
) -> Result<CompiledBundle, KernelError> {
    let schema = Schema::from_str(schema_src)
        .map_err(|err| KernelError::validation(format!("cedar schema parse failed: {err}")))?;
    let policies = PolicySet::from_str(policy_src)
        .map_err(|err| KernelError::validation(format!("cedar policy parse failed: {err}")))?;

    let result = Validator::new(schema.clone()).validate(&policies, ValidationMode::Strict);
    if !result.validation_passed_without_warnings() {
        let detail = result
            .validation_errors()
            .map(ToString::to_string)
            .chain(result.validation_warnings().map(ToString::to_string))
            .collect::<Vec<_>>()
            .join("; ");
        return Err(KernelError::validation(format!(
            "cedar bundle failed strict validation: {detail}"
        )));
    }

    let key = CompiledBundleCacheKey::new(
        org_id,
        policy_version,
        schema_version,
        bundle_digest(schema_src, policy_src),
        CEDAR_SDK_VERSION,
        CEDAR_LANGUAGE_VERSION,
    )?;

    Ok(CompiledBundle {
        key,
        schema,
        policies,
    })
}

/// Evaluate `request` against a compiled `bundle` into a real Cedar decision.
///
/// Nothing escapes: any construction/parse error becomes
/// [`CedarEvaluation::Error`], and a Cedar panic is caught and recorded the same
/// way so a Cedar bug can never propagate as an exception into a live path.
#[must_use]
pub fn evaluate(request: &AuthorizationRequest, bundle: &CompiledBundle) -> CedarEvaluation {
    match std::panic::catch_unwind(AssertUnwindSafe(|| evaluate_inner(request, bundle))) {
        Ok(Ok(evaluation)) => evaluation,
        Ok(Err(reason)) => CedarEvaluation::Error { reason },
        Err(_) => CedarEvaluation::Error {
            reason: "cedar evaluation panicked".to_owned(),
        },
    }
}

fn evaluate_inner(
    request: &AuthorizationRequest,
    bundle: &CompiledBundle,
) -> Result<CedarEvaluation, String> {
    let principal = &request.subject.principal;

    // Subject entity — server-derived roles + freshness + clearance only.
    let subject_uid = entity_uid("Subject", &principal.user_id.to_string())?;
    let roles = principal
        .roles
        .iter()
        .map(|role| RestrictedExpression::new_string(role.as_str().to_owned()));
    let subject_version = i64::try_from(request.subject.freshness.subject_version)
        .map_err(|_| "subject_version exceeds i64 range".to_owned())?;
    let subject_attrs = HashMap::from([
        (
            "org".to_owned(),
            RestrictedExpression::new_string(principal.org_id.to_string()),
        ),
        ("roles".to_owned(), RestrictedExpression::new_set(roles)),
        (
            "subject_version".to_owned(),
            RestrictedExpression::new_long(subject_version),
        ),
        (
            "clearance_keys".to_owned(),
            RestrictedExpression::new_set(
                request
                    .subject
                    .clearance_keys
                    .iter()
                    .cloned()
                    .map(RestrictedExpression::new_string),
            ),
        ),
    ]);
    let subject = Entity::new(subject_uid.clone(), subject_attrs, HashSet::new())
        .map_err(|err| format!("cedar subject entity failed: {err}"))?;

    // Resource entity — from the server-loaded resource scope.
    let resource = &request.resource;
    let resource_eid = resource
        .resource_id
        .clone()
        .filter(|id| !id.trim().is_empty())
        .unwrap_or_else(|| resource.resource_type.clone());
    let resource_uid = entity_uid("Resource", &resource_eid)?;
    let mut resource_attrs = HashMap::from([
        (
            "org".to_owned(),
            RestrictedExpression::new_string(resource.org_id.to_string()),
        ),
        (
            "resource_type".to_owned(),
            RestrictedExpression::new_string(resource.resource_type.clone()),
        ),
    ]);
    if let Some(resource_id) = &resource.resource_id {
        resource_attrs.insert(
            "resource_id".to_owned(),
            RestrictedExpression::new_string(resource_id.clone()),
        );
    }
    if let Some(branch_id) = resource.branch_id {
        resource_attrs.insert(
            "branch".to_owned(),
            RestrictedExpression::new_string(branch_id.to_string()),
        );
    }
    let resource_entity = Entity::new(resource_uid.clone(), resource_attrs, HashSet::new())
        .map_err(|err| format!("cedar resource entity failed: {err}"))?;

    // Action id == Feature::as_str; an unenrolled feature is not a declared
    // schema action, so request construction fails closed below.
    let action_uid = entity_uid("Action", request.action.feature().as_str())?;

    // Context — non-authoritative attributes only; all optional in the schema.
    let mut context_pairs: Vec<(String, RestrictedExpression)> = Vec::new();
    if let Some(purpose) = &request.context.purpose {
        context_pairs.push((
            "purpose".to_owned(),
            RestrictedExpression::new_string(purpose.clone()),
        ));
    }
    if let Some(channel) = &request.context.channel {
        context_pairs.push((
            "channel".to_owned(),
            RestrictedExpression::new_string(channel.clone()),
        ));
    }
    let context =
        Context::from_pairs(context_pairs).map_err(|err| format!("cedar context failed: {err}"))?;

    let entities = Entities::from_entities([subject, resource_entity], Some(&bundle.schema))
        .map_err(|err| format!("cedar entities failed: {err}"))?;
    let cedar_request = Request::new(
        subject_uid,
        action_uid,
        resource_uid,
        context,
        Some(&bundle.schema),
    )
    .map_err(|err| format!("cedar request failed: {err}"))?;

    let response = Authorizer::new().is_authorized(&cedar_request, &bundle.policies, &entities);
    match response.decision() {
        Decision::Allow => Ok(CedarEvaluation::Allow {
            bundle_key: bundle.key.clone(),
        }),
        Decision::Deny => {
            let errors = response
                .diagnostics()
                .errors()
                .map(ToString::to_string)
                .collect::<Vec<_>>();
            let reason = if errors.is_empty() {
                "cedar denied: no matching permit".to_owned()
            } else {
                format!("cedar denied with evaluation errors: {}", errors.join("; "))
            };
            Ok(CedarEvaluation::Deny {
                bundle_key: bundle.key.clone(),
                reason,
            })
        }
    }
}

fn entity_uid(type_name: &str, id: &str) -> Result<EntityUid, String> {
    let parsed = EntityTypeName::from_str(type_name)
        .map_err(|err| format!("cedar entity type '{type_name}' invalid: {err}"))?;
    Ok(EntityUid::from_type_name_and_id(parsed, EntityId::new(id)))
}

/// Deterministic bundle digest: `hex(sha256(schema_src ‖ policy_src))`.
fn bundle_digest(schema_src: &str, policy_src: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(schema_src.as_bytes());
    hasher.update(policy_src.as_bytes());
    hex::encode(hasher.finalize())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use std::collections::BTreeSet;

    use console_kernel_core::{BranchScope, OrgId, UserId};

    use super::{
        ROLE_MANAGE_SCHEMA, ROLE_MANAGE_SCHEMA_VERSION, compile_bundle, compile_bundle_for_feature,
        compile_bundle_from_sources, evaluate, generate_policies,
    };
    use crate::{
        Action, AuthorizationRequest, AuthorizationResource, CedarEvaluation, Feature, Principal,
        Role,
    };

    fn role_manage_request(role: Role) -> AuthorizationRequest {
        let principal = Principal::new(
            UserId::new(),
            OrgId::knl(),
            BTreeSet::from([role]),
            BranchScope::All,
        );
        AuthorizationRequest::new(
            principal,
            Action::new(Feature::RoleManage),
            AuthorizationResource::org_wide(OrgId::knl(), "role"),
        )
    }

    fn resource_kind_for(feature: Feature) -> &'static str {
        match feature {
            Feature::UserManage => "user",
            Feature::CompletionReview | Feature::ApprovalFinalize => "workflow_run",
            _ => "work_order",
        }
    }

    fn feature_request(feature: Feature, role: Role) -> AuthorizationRequest {
        let principal = Principal::new(
            UserId::new(),
            OrgId::knl(),
            BTreeSet::from([role]),
            BranchScope::All,
        );
        AuthorizationRequest::new(
            principal,
            Action::new(feature),
            AuthorizationResource::org_wide(OrgId::knl(), resource_kind_for(feature)),
        )
    }

    /// The enrollment-wave-2 generalization: a per-feature bundle compiles under
    /// strict validation and its Cedar verdict matches the legacy matrix for the
    /// enrolled feature (allow the matrix-allowed role, deny the denied one) — the
    /// property parity measurement depends on.
    #[test]
    fn per_feature_bundle_matches_the_legacy_matrix() {
        for feature in [
            Feature::WorkOrderReadAll,
            Feature::UserManage,
            Feature::CompletionReview,
            Feature::ApprovalFinalize,
        ] {
            let bundle = compile_bundle_for_feature(OrgId::knl(), 1, feature)
                .expect("per-feature bundle must compile + strict-validate");

            // SUPER_ADMIN is Allow for every enrolled feature above.
            let allow = evaluate(&feature_request(feature, Role::SuperAdmin), &bundle);
            assert!(
                matches!(allow, CedarEvaluation::Allow { .. }),
                "{feature:?}: SUPER_ADMIN must be allowed, got {allow:?}"
            );

            // MEMBER (bottom of the matrix) is Deny for every one of them.
            let deny = evaluate(&feature_request(feature, Role::Member), &bundle);
            assert!(
                matches!(deny, CedarEvaluation::Deny { .. }),
                "{feature:?}: MEMBER must be denied, got {deny:?}"
            );
        }
    }

    #[test]
    fn per_feature_bundle_keys_differ_across_features() {
        let a = compile_bundle_for_feature(OrgId::knl(), 1, Feature::WorkOrderReadAll).unwrap();
        let b = compile_bundle_for_feature(OrgId::knl(), 1, Feature::UserManage).unwrap();
        assert_ne!(
            a.key, b.key,
            "distinct features must produce distinct bundle identities"
        );
    }

    #[test]
    fn matrix_generated_role_manage_policy_is_single_super_admin_rule() {
        let policies = generate_policies(Feature::RoleManage);
        assert_eq!(
            policies.matches("permit(").count(),
            1,
            "RoleManage must generate exactly one permit rule: {policies}"
        );
        assert!(
            policies.contains("principal.roles.contains(\"SUPER_ADMIN\")"),
            "the single rule must gate on SUPER_ADMIN: {policies}"
        );
        for role in [
            Role::Member,
            Role::Receptionist,
            Role::Mechanic,
            Role::Admin,
            Role::Executive,
        ] {
            assert!(
                !policies.contains(&format!("contains(\"{}\")", role.as_str())),
                "non-SUPER_ADMIN role {} must not appear in RoleManage policy",
                role.as_str()
            );
        }
    }

    #[test]
    fn matrix_generated_bundle_compiles_and_passes_strict_validation() {
        let bundle =
            compile_bundle(OrgId::knl(), 7).expect("bundle must compile + strict-validate");
        assert_eq!(bundle.key.org_id, OrgId::knl());
        assert_eq!(bundle.key.policy_version, 7);
        assert_eq!(bundle.key.schema_version, ROLE_MANAGE_SCHEMA_VERSION);
        assert_eq!(bundle.key.cedar_sdk_version, "4.11.2");
        assert_eq!(bundle.key.cedar_language_version, "4.5");
        assert!(!bundle.key.bundle_digest.is_empty());
    }

    #[test]
    fn invalid_policy_fails_strict_validation_and_is_not_activated() {
        // Real schema, but a policy that reads an attribute the schema does not
        // declare on Subject — strict validation must reject it fail-closed.
        let bad_policy = "permit(\n  principal,\n  action == Action::\"role_manage\",\n  resource\n)\nwhen { principal.nonexistent_attr == \"x\" };\n";
        let result = compile_bundle_from_sources(
            OrgId::knl(),
            1,
            ROLE_MANAGE_SCHEMA_VERSION,
            ROLE_MANAGE_SCHEMA,
            bad_policy,
        );
        assert!(
            result.is_err(),
            "a policy referencing an undeclared attribute must fail strict validation"
        );
    }

    #[test]
    fn invalid_schema_is_not_activated() {
        // A schema whose action references an undeclared entity type must not
        // compile into an activated bundle.
        let bad_schema = "entity Subject = { \"org\": String };\naction role_manage appliesTo {\n  principal: [Subject],\n  resource: [DoesNotExist]\n};\n";
        let result = compile_bundle_from_sources(
            OrgId::knl(),
            1,
            ROLE_MANAGE_SCHEMA_VERSION,
            bad_schema,
            &generate_policies(Feature::RoleManage),
        );
        assert!(
            result.is_err(),
            "a schema referencing an undeclared entity type must fail closed"
        );
    }

    #[test]
    fn evaluate_allows_super_admin_and_denies_others() {
        let bundle = compile_bundle(OrgId::knl(), 1).unwrap();

        let allow = evaluate(&role_manage_request(Role::SuperAdmin), &bundle);
        match allow {
            CedarEvaluation::Allow { bundle_key } => {
                assert_eq!(
                    bundle_key, bundle.key,
                    "allow must carry the bundle's own compiled key"
                );
            }
            other => panic!("SUPER_ADMIN must be allowed role_manage, got {other:?}"),
        }

        for role in [
            Role::Member,
            Role::Receptionist,
            Role::Mechanic,
            Role::Admin,
            Role::Executive,
        ] {
            let denied = evaluate(&role_manage_request(role), &bundle);
            assert!(
                matches!(denied, CedarEvaluation::Deny { .. }),
                "{} must be denied role_manage, got {denied:?}",
                role.as_str()
            );
        }
    }

    #[test]
    fn bundle_digest_is_deterministic_across_compiles() {
        let first = compile_bundle(OrgId::knl(), 1).unwrap();
        let second = compile_bundle(OrgId::knl(), 1).unwrap();
        assert_eq!(
            first.key.bundle_digest, second.key.bundle_digest,
            "same schema + policy inputs must yield an identical digest"
        );

        // A different policy_version keeps the same digest (same inputs) but is a
        // distinct cache key so a version bump re-keys the bundle.
        let bumped = compile_bundle(OrgId::knl(), 2).unwrap();
        assert_eq!(first.key.bundle_digest, bumped.key.bundle_digest);
        assert_ne!(first.key, bumped.key);
    }
}
