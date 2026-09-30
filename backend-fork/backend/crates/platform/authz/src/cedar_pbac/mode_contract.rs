//! Locked truth table for the four [`DualEngineMode`] arms of
//! [`evaluate_cedar_pbac_boundary`].
//!
//! Test-only module. It pins WHO DECIDES in each mode and — for the three
//! Cedar-requiring modes — that the boundary still fails closed on an invalid
//! enrollment map, a stale identity, an RLS mismatch, or an unavailable bundle.
//! `cedar_shadow_legacy_enforce` is the load-bearing one: "observational" applies
//! to the Cedar POLICY OUTCOME, never to those four failure conditions, so a
//! refactor that loosens the shadow lane cannot silently drop a guard.

#![cfg(test)]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeSet;

use console_kernel_core::{BranchScope, OrgId, UserId};

use super::{
    AuthorizationRequest, AuthorizationResource, CedarEvaluation, CoexistenceMapEntry,
    CompiledBundleCacheKey, DecisionEffect, DecisionEngine, DecisionReason, DualEngineMode,
    RlsScopeProof, SubjectFreshness, SubjectFreshnessRequirement, evaluate_cedar_pbac_boundary,
};
use crate::{Action, Feature, Principal, Role};

const DOMAIN: &str = "identity.policy";
const RESOURCE_TYPE: &str = "identity.policy_role";

/// Every Cedar-requiring mode, derived from [`DualEngineMode::ALL`] — which is
/// generated from the same token list as the enum, so it cannot go stale — by
/// asking the production classifier itself. `LegacyOnly` falls out on purpose:
/// it never consults Cedar, so the Cedar-side fail-closed conditions do not
/// apply to it.
fn cedar_modes() -> impl Iterator<Item = DualEngineMode> {
    let modes: Vec<DualEngineMode> = DualEngineMode::ALL
        .iter()
        .copied()
        .filter(|mode| super::cedar_required(*mode))
        .collect();
    // A collapsed classifier would make every `for mode in cedar_modes()` loop
    // below pass vacuously, so the roster is sized here, at the one place all
    // four fail-closed tests draw it from. The expected size is derived, not a
    // literal: every mode but `LegacyOnly` is Cedar-requiring, so a mode added
    // to the enum must show up in these loops too.
    assert_eq!(modes.len(), DualEngineMode::ALL.len() - 1, "{modes:?}");
    modes.into_iter()
}

fn principal(role: Role) -> Principal {
    Principal::new(
        UserId::new(),
        OrgId::knl(),
        BTreeSet::from([role]),
        BranchScope::All,
    )
}

fn bundle_key() -> CompiledBundleCacheKey {
    CompiledBundleCacheKey::new(
        OrgId::knl(),
        7,
        "schema-v1",
        "sha256:bundle",
        "4.11.2",
        "4.5",
    )
    .unwrap()
}

fn stale_bundle_key() -> CompiledBundleCacheKey {
    CompiledBundleCacheKey::new(
        OrgId::knl(),
        7,
        "schema-v1",
        "sha256:stale-bundle",
        "4.11.2",
        "4.5",
    )
    .unwrap()
}

fn freshness() -> SubjectFreshness {
    SubjectFreshness {
        policy_version: 7,
        subject_version: 3,
        session_generation: 2,
        step_up_generation: Some(2),
    }
}

fn required_freshness() -> SubjectFreshnessRequirement {
    SubjectFreshnessRequirement {
        min_policy_version: 7,
        min_subject_version: 3,
        min_session_generation: 2,
        required_step_up_generation: Some(2),
    }
}

/// A request that clears every boundary precondition, so the only thing left to
/// decide is the mode's who-decides rule.
fn ready_request(role: Role) -> AuthorizationRequest {
    AuthorizationRequest::new(
        principal(role),
        Action::new(Feature::RoleManage),
        AuthorizationResource::org_wide(OrgId::knl(), RESOURCE_TYPE),
    )
    .with_policy_domain(DOMAIN)
    .with_subject_freshness(freshness())
    .requiring_freshness(required_freshness())
    .with_rls_scope_proof(RlsScopeProof::runtime_role_guc(OrgId::knl()))
}

fn entry(mode: DualEngineMode) -> CoexistenceMapEntry {
    CoexistenceMapEntry::new(
        format!("{DOMAIN}.role_manage"),
        DOMAIN,
        Feature::RoleManage,
        RESOURCE_TYPE,
        mode,
        Some(bundle_key()),
    )
}

fn cedar_allow() -> CedarEvaluation {
    CedarEvaluation::Allow {
        bundle_key: bundle_key(),
    }
}

fn cedar_deny() -> CedarEvaluation {
    CedarEvaluation::Deny {
        bundle_key: bundle_key(),
        reason: "cedar policy denied".to_owned(),
    }
}

// ---------------------------------------------------------------------------
// 1. legacy_only => legacy decides.
// ---------------------------------------------------------------------------

#[test]
fn legacy_only_lets_legacy_decide_and_cedar_can_neither_grant_nor_veto() {
    let mode = DualEngineMode::LegacyOnly;

    // Cedar Allow cannot grant what legacy denies.
    let granted = evaluate_cedar_pbac_boundary(
        &ready_request(Role::Member),
        Some(&entry(mode)),
        cedar_allow(),
    );
    assert_eq!(granted.effect, DecisionEffect::Deny);
    assert_eq!(granted.reason, DecisionReason::LegacyDenied);

    // Cedar Deny cannot veto what legacy allows.
    let vetoed = evaluate_cedar_pbac_boundary(
        &ready_request(Role::SuperAdmin),
        Some(&entry(mode)),
        cedar_deny(),
    );
    assert_eq!(vetoed.effect, DecisionEffect::Allow);
    assert_eq!(vetoed.reason, DecisionReason::LegacyAllowed);
    assert_eq!(vetoed.engine, DecisionEngine::Legacy);
}

// ---------------------------------------------------------------------------
// 2. cedar_shadow_legacy_enforce => legacy decides; the Cedar POLICY OUTCOME is
//    observational only.
// ---------------------------------------------------------------------------

#[test]
fn shadow_lets_legacy_enforce_and_the_cedar_policy_outcome_cannot_grant() {
    let mode = DualEngineMode::CedarShadowLegacyEnforce;

    // Legacy is the enforcer: a Cedar Allow cannot grant what legacy denies, and
    // the recorded reason names legacy as the decider.
    let granted = evaluate_cedar_pbac_boundary(
        &ready_request(Role::Member),
        Some(&entry(mode)),
        cedar_allow(),
    );
    assert_eq!(granted.effect, DecisionEffect::Deny);
    assert_eq!(granted.reason, DecisionReason::LegacyDenied);
    assert_eq!(granted.engine, DecisionEngine::DualEngine);

    // Legacy allow + Cedar allow is the only allowing combination, and it is
    // attributed to legacy — Cedar's allow did not decide it.
    let allowed = evaluate_cedar_pbac_boundary(
        &ready_request(Role::SuperAdmin),
        Some(&entry(mode)),
        cedar_allow(),
    );
    assert_eq!(allowed.effect, DecisionEffect::Allow);
    assert_eq!(allowed.reason, DecisionReason::LegacyAllowed);
    assert_eq!(allowed.engine, DecisionEngine::DualEngine);
}

/// The whole shadow row, across every Cedar evaluation shape.
///
/// `docs/specs/cedar-pbac-cutover.md:98` fixes the direction the Cedar policy
/// outcome may move a shadow decision: it "cannot grant". So legacy holds the
/// veto — no Cedar shape allows what legacy denies — and the single allowing
/// combination is credited to legacy, never to Cedar. A Cedar policy deny/error
/// still surfaces as the observation's effect, which is exactly why
/// `identity/rest`'s `authorize_org_manage_observed` returns the legacy `Result`
/// and never this effect.
#[test]
fn shadow_lets_no_cedar_shape_grant_and_credits_every_allow_to_legacy() {
    let mode = DualEngineMode::CedarShadowLegacyEnforce;
    let shapes = [
        (cedar_allow(), DecisionReason::LegacyAllowed),
        (cedar_deny(), DecisionReason::CedarDenied),
        (
            CedarEvaluation::Error {
                reason: "cedar evaluation panicked".to_owned(),
            },
            DecisionReason::CedarError,
        ),
        (
            CedarEvaluation::NotConfigured,
            DecisionReason::BundleUnavailable,
        ),
        (
            CedarEvaluation::Allow {
                bundle_key: stale_bundle_key(),
            },
            DecisionReason::StalePolicyBundle,
        ),
    ];

    for (cedar, reason_when_legacy_allows) in shapes {
        // Legacy DENIES: no Cedar shape, not even a clean Allow on the enrolled
        // bundle, can grant.
        let denied = evaluate_cedar_pbac_boundary(
            &ready_request(Role::Member),
            Some(&entry(mode)),
            cedar.clone(),
        );
        assert_eq!(denied.effect, DecisionEffect::Deny, "{cedar:?}");

        // Legacy ALLOWS: only a clean Cedar Allow survives, and that allow is
        // attributed to legacy — Cedar never appears as the decider.
        let decision = evaluate_cedar_pbac_boundary(
            &ready_request(Role::SuperAdmin),
            Some(&entry(mode)),
            cedar.clone(),
        );
        assert_eq!(decision.reason, reason_when_legacy_allows, "{cedar:?}");
        if decision.effect == DecisionEffect::Allow {
            assert_eq!(decision.reason, DecisionReason::LegacyAllowed, "{cedar:?}");
            assert_ne!(decision.engine, DecisionEngine::Cedar, "{cedar:?}");
        }
    }
}

// ---------------------------------------------------------------------------
// 3. cedar_enforce_legacy_compare => Cedar decides.
// ---------------------------------------------------------------------------

#[test]
fn compare_lets_cedar_decide() {
    let mode = DualEngineMode::CedarEnforceLegacyCompare;

    let allowed = evaluate_cedar_pbac_boundary(
        &ready_request(Role::SuperAdmin),
        Some(&entry(mode)),
        cedar_allow(),
    );
    assert_eq!(allowed.effect, DecisionEffect::Allow);
    assert_eq!(allowed.reason, DecisionReason::CedarAllowed);

    // Cedar deny stands even though legacy would allow.
    let denied = evaluate_cedar_pbac_boundary(
        &ready_request(Role::SuperAdmin),
        Some(&entry(mode)),
        cedar_deny(),
    );
    assert_eq!(denied.effect, DecisionEffect::Deny);
    assert_eq!(denied.reason, DecisionReason::CedarDenied);
    assert_eq!(denied.engine, DecisionEngine::Cedar);
}

// ---------------------------------------------------------------------------
// 4. cedar_only => Cedar decides, and a Cedar ERROR DENIES.
// ---------------------------------------------------------------------------

#[test]
fn cedar_only_lets_cedar_decide_and_a_cedar_error_denies() {
    let mode = DualEngineMode::CedarOnly;

    // Cedar grants without legacy: MEMBER is a legacy deny.
    let allowed = evaluate_cedar_pbac_boundary(
        &ready_request(Role::Member),
        Some(&entry(mode)),
        cedar_allow(),
    );
    assert_eq!(allowed.effect, DecisionEffect::Allow);
    assert_eq!(allowed.reason, DecisionReason::CedarAllowed);

    let denied = evaluate_cedar_pbac_boundary(
        &ready_request(Role::SuperAdmin),
        Some(&entry(mode)),
        cedar_deny(),
    );
    assert_eq!(denied.effect, DecisionEffect::Deny);
    assert_eq!(denied.reason, DecisionReason::CedarDenied);

    let errored = evaluate_cedar_pbac_boundary(
        &ready_request(Role::SuperAdmin),
        Some(&entry(mode)),
        CedarEvaluation::Error {
            reason: "cedar evaluation panicked".to_owned(),
        },
    );
    assert_eq!(errored.effect, DecisionEffect::Deny);
    assert_eq!(errored.reason, DecisionReason::CedarError);
    assert_eq!(errored.engine, DecisionEngine::Cedar);
}

// ---------------------------------------------------------------------------
// The four fail-closed conditions. Each case uses a LEGACY-ALLOWING principal
// and a Cedar ALLOW, so the only way the assertion can fail is the boundary
// permitting through the fault.
// ---------------------------------------------------------------------------

/// The single seam a "make the shadow lane purely observational" refactor would
/// touch first: [`super::cedar_required`] is what arms the Cedar-side
/// precondition and bundle-availability denials in
/// [`evaluate_cedar_pbac_boundary`]. Pinning the classifier directly means the
/// guarantee is checkable by reading and running, without mutating the source.
///
/// Stated as an equivalence over [`DualEngineMode::ALL`], not over a
/// hand-listed subset, so it can detect its own violation: a mode added to the
/// enum lands in `ALL` automatically, and the only posture that keeps this test
/// green is "Cedar-requiring", i.e. fully guarded. Declaring a new mode exempt
/// turns this RED instead of silently dropping all four guards.
#[test]
fn legacy_only_is_the_only_mode_exempt_from_the_fail_closed_guards() {
    for mode in DualEngineMode::ALL.iter().copied() {
        assert_eq!(
            super::cedar_required(mode),
            mode != DualEngineMode::LegacyOnly,
            "{mode:?}"
        );
    }
}

#[test]
fn every_cedar_mode_denies_an_invalid_enrollment_map() {
    // No map entry at all for an enrolled request.
    let missing =
        evaluate_cedar_pbac_boundary(&ready_request(Role::SuperAdmin), None, cedar_allow());
    assert_eq!(missing.effect, DecisionEffect::Deny);
    assert_eq!(missing.reason, DecisionReason::MissingCoexistenceMap);
    assert_eq!(missing.engine, DecisionEngine::BoundaryPreflight);

    for mode in cedar_modes() {
        // Entry present but bound to another action / domain / resource type.
        let mismatches = [
            CoexistenceMapEntry::new(
                format!("{DOMAIN}.user_manage"),
                DOMAIN,
                Feature::UserManage,
                RESOURCE_TYPE,
                mode,
                Some(bundle_key()),
            ),
            CoexistenceMapEntry::new(
                format!("{DOMAIN}.role_manage"),
                "workflow.guards",
                Feature::RoleManage,
                RESOURCE_TYPE,
                mode,
                Some(bundle_key()),
            ),
            CoexistenceMapEntry::new(
                format!("{DOMAIN}.role_manage"),
                DOMAIN,
                Feature::RoleManage,
                "work_order",
                mode,
                Some(bundle_key()),
            ),
        ];
        for mismatch in mismatches {
            let decision = evaluate_cedar_pbac_boundary(
                &ready_request(Role::SuperAdmin),
                Some(&mismatch),
                cedar_allow(),
            );
            assert_eq!(decision.effect, DecisionEffect::Deny, "{mode:?}");
            assert_eq!(
                decision.reason,
                DecisionReason::MalformedCoexistenceMap,
                "{mode:?}"
            );
            assert_eq!(decision.mode, Some(mode));
        }
    }
}

#[test]
fn every_cedar_mode_denies_a_stale_identity() {
    for mode in cedar_modes() {
        // Token snapshot below the DB-current requirement.
        let stale =
            ready_request(Role::SuperAdmin).requiring_freshness(SubjectFreshnessRequirement {
                min_policy_version: 8,
                ..required_freshness()
            });
        let decision = evaluate_cedar_pbac_boundary(&stale, Some(&entry(mode)), cedar_allow());
        assert_eq!(decision.effect, DecisionEffect::Deny, "{mode:?}");
        assert_eq!(decision.reason, DecisionReason::StaleSubject, "{mode:?}");
        assert_eq!(decision.mode, Some(mode));

        // No subject material at all is equally fatal for a Cedar-requiring mode.
        let bare = AuthorizationRequest::new(
            principal(Role::SuperAdmin),
            Action::new(Feature::RoleManage),
            AuthorizationResource::org_wide(OrgId::knl(), RESOURCE_TYPE),
        )
        .with_policy_domain(DOMAIN)
        .with_rls_scope_proof(RlsScopeProof::runtime_role_guc(OrgId::knl()));
        let decision = evaluate_cedar_pbac_boundary(&bare, Some(&entry(mode)), cedar_allow());
        assert_eq!(decision.effect, DecisionEffect::Deny, "{mode:?}");
        assert_eq!(
            decision.reason,
            DecisionReason::MissingSubjectFreshness,
            "{mode:?}"
        );
    }
}

#[test]
fn every_cedar_mode_denies_an_rls_mismatch() {
    for mode in cedar_modes() {
        // Principal org != resource org.
        let cross_org = AuthorizationRequest::new(
            principal(Role::SuperAdmin),
            Action::new(Feature::RoleManage),
            AuthorizationResource::org_wide(OrgId::platform(), RESOURCE_TYPE),
        )
        .with_policy_domain(DOMAIN)
        .with_subject_freshness(freshness())
        .requiring_freshness(required_freshness())
        .with_rls_scope_proof(RlsScopeProof::runtime_role_guc(OrgId::knl()));
        let decision = evaluate_cedar_pbac_boundary(&cross_org, Some(&entry(mode)), cedar_allow());
        assert_eq!(decision.effect, DecisionEffect::Deny, "{mode:?}");
        assert_eq!(
            decision.reason,
            DecisionReason::RlsBoundaryMismatch,
            "{mode:?}"
        );

        // Armed RLS scope belongs to a different tenant than the request.
        let wrong_proof = ready_request(Role::SuperAdmin)
            .with_rls_scope_proof(RlsScopeProof::runtime_role_guc(OrgId::platform()));
        let decision =
            evaluate_cedar_pbac_boundary(&wrong_proof, Some(&entry(mode)), cedar_allow());
        assert_eq!(decision.effect, DecisionEffect::Deny, "{mode:?}");
        assert_eq!(
            decision.reason,
            DecisionReason::RlsBoundaryMismatch,
            "{mode:?}"
        );

        // No proof that the DB reads ran under an armed scope at all.
        let no_proof = AuthorizationRequest::new(
            principal(Role::SuperAdmin),
            Action::new(Feature::RoleManage),
            AuthorizationResource::org_wide(OrgId::knl(), RESOURCE_TYPE),
        )
        .with_policy_domain(DOMAIN)
        .with_subject_freshness(freshness())
        .requiring_freshness(required_freshness());
        let decision = evaluate_cedar_pbac_boundary(&no_proof, Some(&entry(mode)), cedar_allow());
        assert_eq!(decision.effect, DecisionEffect::Deny, "{mode:?}");
        assert_eq!(
            decision.reason,
            DecisionReason::MissingRlsScopeProof,
            "{mode:?}"
        );
    }
}

#[test]
fn every_cedar_mode_denies_an_unavailable_bundle() {
    for mode in cedar_modes() {
        // The map entry carries no compiled bundle identity.
        let unbound = CoexistenceMapEntry::new(
            format!("{DOMAIN}.role_manage"),
            DOMAIN,
            Feature::RoleManage,
            RESOURCE_TYPE,
            mode,
            None,
        );
        let decision = evaluate_cedar_pbac_boundary(
            &ready_request(Role::SuperAdmin),
            Some(&unbound),
            cedar_allow(),
        );
        assert_eq!(decision.effect, DecisionEffect::Deny, "{mode:?}");
        assert_eq!(
            decision.reason,
            DecisionReason::BundleUnavailable,
            "{mode:?}"
        );
        assert_eq!(decision.mode, Some(mode));

        // No Cedar adapter was configured to evaluate against the bundle.
        let decision = evaluate_cedar_pbac_boundary(
            &ready_request(Role::SuperAdmin),
            Some(&entry(mode)),
            CedarEvaluation::NotConfigured,
        );
        assert_eq!(decision.effect, DecisionEffect::Deny, "{mode:?}");
        assert_eq!(
            decision.reason,
            DecisionReason::BundleUnavailable,
            "{mode:?}"
        );

        // Cedar answered from a bundle that is not the enrolled one.
        let decision = evaluate_cedar_pbac_boundary(
            &ready_request(Role::SuperAdmin),
            Some(&entry(mode)),
            CedarEvaluation::Allow {
                bundle_key: stale_bundle_key(),
            },
        );
        assert_eq!(decision.effect, DecisionEffect::Deny, "{mode:?}");
        assert_eq!(
            decision.reason,
            DecisionReason::StalePolicyBundle,
            "{mode:?}"
        );
    }
}
