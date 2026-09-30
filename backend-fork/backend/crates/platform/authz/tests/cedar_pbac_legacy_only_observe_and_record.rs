#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! AC3 — M2 workflow-runtime Cedar guards (node transitions + waiting-task
//! completion) are STRICTLY observe-and-record under `DualEngineMode::LegacyOnly`
//! (the pinned M2 mode).
//!
//! These are pure-function proofs over the *real* Cedar/PBAC boundary the M2
//! guards delegate to (`evaluate_cedar_pbac_boundary` / `evaluate_legacy_contract`
//! / `observe_cedar_pbac_decision`). No stubs, no mock decisions — the same
//! functions production wires at every node transition. They pin three
//! invariants that together define "observe-and-record":
//!
//!   1. LEGACY IS THE SOLE ENFORCER. Under `LegacyOnly` the boundary returns
//!      byte-for-byte what `evaluate_legacy_contract` returns, and the enforced
//!      decision is stamped `engine = Legacy`, `mode = LegacyOnly`.
//!   2. INERT CEDAR CAN NEVER DENY (nor grant). For a fixed request the enforced
//!      decision is invariant across EVERY `CedarEvaluation` a guard might feed
//!      it (`NotConfigured` / `Allow` / `Deny` / `Error`): a shadow Deny cannot
//!      flip a legacy Allow to Deny, and a shadow Allow cannot flip a legacy Deny
//!      to Allow.
//!   3. THE SHADOW VERDICT IS RECORDED, NOT ENFORCED. `observe_cedar_pbac_decision`
//!      returns exactly ONE audit event whose `.decision` equals the already-
//!      enforced (legacy) decision — observation is *fed* the decision, so it
//!      cannot mutate it — while the would-be Cedar verdict is preserved in the
//!      audit's `evaluated_*` fields for the forensic trail. That single event is
//!      what a guard hands to `with_audits`, landing as ONE `audit_events` row in
//!      the SAME transaction as the node/task state change.

use std::collections::BTreeSet;

use console_kernel_core::{BranchId, BranchScope, OrgId, UserId};
use console_platform_authz::{
    Action, AuthorizationRequest, AuthorizationResource, CedarEvaluation, CoexistenceMapEntry,
    CompiledBundleCacheKey, DecisionEffect, DecisionEngine, DecisionReason, DualEngineMode,
    Feature, Principal, Role, evaluate_cedar_pbac_boundary, evaluate_legacy_contract,
    observe_cedar_pbac_decision,
};

/// The M2 workflow-runtime guard domain. It only needs to be a non-empty string
/// that the coexistence entry and the request agree on so the boundary reaches
/// the enrolled arm; legacy enforcement itself never consults the domain.
const GUARD_DOMAIN: &str = "workflow.runtime";

fn principal(role: Role, scope: BranchScope) -> Principal {
    Principal::new(UserId::new(), OrgId::knl(), BTreeSet::from([role]), scope)
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

/// A coexistence-map entry pinned to `LegacyOnly` — the M2 mode. The bundle key
/// is present but, under `LegacyOnly`, Cedar is not required so it is never
/// consulted; it only rides the shadow audit event.
fn legacy_only_guard_entry() -> CoexistenceMapEntry {
    CoexistenceMapEntry::new(
        "workflow.runtime.priority_manage",
        GUARD_DOMAIN,
        Feature::PriorityManage,
        "work_order",
        DualEngineMode::LegacyOnly,
        Some(bundle_key()),
    )
}

/// A node-transition-shaped authorization request (guard the priority change on a
/// work-order node). Same shape the legacy `authorize` path already governs.
fn guard_request(role: Role, branch: BranchId) -> AuthorizationRequest {
    AuthorizationRequest::new(
        principal(role, BranchScope::single(branch)),
        Action::new(Feature::PriorityManage),
        AuthorizationResource::branch(OrgId::knl(), branch, "work_order")
            .with_resource_id("work_order:node-run-42"),
    )
    .with_policy_domain(GUARD_DOMAIN)
}

/// Every shape of the inert/shadow Cedar adapter output a guard could feed the
/// boundary at M2. Under `LegacyOnly` NONE may influence the enforced decision.
fn every_cedar_shape() -> Vec<CedarEvaluation> {
    vec![
        // The literal M2 posture: Cedar is not wired, so the adapter is inert.
        CedarEvaluation::NotConfigured,
        CedarEvaluation::Allow {
            bundle_key: bundle_key(),
        },
        CedarEvaluation::Deny {
            bundle_key: bundle_key(),
            reason: "shadow policy would deny node transition".to_owned(),
        },
        CedarEvaluation::Error {
            reason: "cedar schema validation failed".to_owned(),
        },
    ]
}

#[test]
fn legacy_only_boundary_is_byte_identical_to_legacy_for_a_would_be_allow() {
    let branch = BranchId::new();
    // ADMIN is allowed PriorityManage on a work-order in scope (legacy matrix).
    let request = guard_request(Role::Admin, branch);
    let entry = legacy_only_guard_entry();

    let legacy = evaluate_legacy_contract(&request);
    assert_eq!(legacy.effect, DecisionEffect::Allow);
    assert_eq!(legacy.engine, DecisionEngine::Legacy);
    assert_eq!(legacy.reason, DecisionReason::LegacyAllowed);
    assert_eq!(legacy.mode, Some(DualEngineMode::LegacyOnly));

    // No matter what the (inert) Cedar adapter would have said, the enforced
    // decision is EXACTLY the legacy decision. A shadow Deny cannot flip it.
    for cedar in every_cedar_shape() {
        let enforced = evaluate_cedar_pbac_boundary(&request, Some(&entry), cedar.clone());
        assert_eq!(
            enforced, legacy,
            "LegacyOnly enforcement must ignore the Cedar verdict {cedar:?}"
        );
        assert_eq!(enforced.effect, DecisionEffect::Allow);
        assert_eq!(enforced.engine, DecisionEngine::Legacy);
    }
}

#[test]
fn legacy_only_boundary_is_byte_identical_to_legacy_for_a_would_be_deny() {
    let branch = BranchId::new();
    // MEMBER is denied PriorityManage everywhere (legacy matrix).
    let request = guard_request(Role::Member, branch);
    let entry = legacy_only_guard_entry();

    let legacy = evaluate_legacy_contract(&request);
    assert_eq!(legacy.effect, DecisionEffect::Deny);
    assert_eq!(legacy.engine, DecisionEngine::Legacy);
    assert_eq!(legacy.reason, DecisionReason::LegacyDenied);
    assert_eq!(legacy.mode, Some(DualEngineMode::LegacyOnly));

    // A shadow Cedar Allow cannot grant what legacy denies.
    for cedar in every_cedar_shape() {
        let enforced = evaluate_cedar_pbac_boundary(&request, Some(&entry), cedar.clone());
        assert_eq!(
            enforced, legacy,
            "LegacyOnly enforcement must ignore the Cedar verdict {cedar:?}"
        );
        assert_eq!(enforced.effect, DecisionEffect::Deny);
        assert_eq!(enforced.engine, DecisionEngine::Legacy);
    }
}

#[test]
fn shadow_deny_is_recorded_but_carries_zero_enforcement_weight() {
    let branch = BranchId::new();
    let request = guard_request(Role::Admin, branch);
    let entry = legacy_only_guard_entry();

    // The inert Cedar would DENY, but the guard enforces the legacy verdict.
    let shadow = CedarEvaluation::Deny {
        bundle_key: bundle_key(),
        reason: "shadow policy would deny priority change on node".to_owned(),
    };
    let enforced = evaluate_cedar_pbac_boundary(&request, Some(&entry), shadow.clone());

    // Enforcement stays legacy-ALLOW even though the shadow Cedar would deny.
    assert_eq!(enforced.effect, DecisionEffect::Allow);
    assert_eq!(enforced.engine, DecisionEngine::Legacy);
    assert_eq!(enforced.mode, Some(DualEngineMode::LegacyOnly));

    // Observation is FED the already-enforced decision, so it records but never
    // mutates it. Exactly one audit event is produced.
    let audit =
        observe_cedar_pbac_decision(&request, Some(&entry), Some(&shadow), enforced.clone());
    assert_eq!(
        audit.decision, enforced,
        "observation must record, never mutate, the enforced decision"
    );

    // The metric projection of the single event reports the enforced (legacy)
    // outcome, not the shadow deny.
    let metric = audit.metric_labels();
    assert_eq!(metric.effect, DecisionEffect::Allow);
    assert_eq!(metric.engine, DecisionEngine::Legacy);
    assert_eq!(metric.mode, Some(DualEngineMode::LegacyOnly));

    // ...while the would-be Cedar deny IS captured for the forensic trail
    // (recorded, not enforced): its bundle identity and raw reason ride the
    // shadow audit event.
    assert_eq!(
        audit.evaluated_reason_detail.as_deref(),
        Some("shadow policy would deny priority change on node")
    );
    assert_eq!(
        audit
            .evaluated_bundle_key
            .as_ref()
            .map(|k| k.bundle_digest.as_str()),
        Some("sha256:bundle")
    );

    // The event is fully server-derived and tenant-scoped: the same org on both
    // sides is what makes it safe to persist through `with_audits` under RLS.
    assert_eq!(audit.request_domain, GUARD_DOMAIN);
    assert_eq!(
        audit.coexistence_entry_id.as_deref(),
        Some("workflow.runtime.priority_manage")
    );
    assert_eq!(audit.resource_type, "work_order");
    assert_eq!(audit.principal_org_id, OrgId::knl());
    assert_eq!(audit.resource_org_id, OrgId::knl());
}

#[test]
fn shadow_allow_on_a_denied_request_is_recorded_but_cannot_grant() {
    let branch = BranchId::new();
    let request = guard_request(Role::Member, branch);
    let entry = legacy_only_guard_entry();

    // The inert Cedar would ALLOW, but legacy denies MEMBER — the guard enforces
    // the deny.
    let shadow = CedarEvaluation::Allow {
        bundle_key: bundle_key(),
    };
    let enforced = evaluate_cedar_pbac_boundary(&request, Some(&entry), shadow.clone());
    assert_eq!(enforced.effect, DecisionEffect::Deny);
    assert_eq!(enforced.engine, DecisionEngine::Legacy);
    assert_eq!(enforced.reason, DecisionReason::LegacyDenied);

    let audit =
        observe_cedar_pbac_decision(&request, Some(&entry), Some(&shadow), enforced.clone());
    assert_eq!(audit.decision, enforced);
    assert_eq!(audit.metric_labels().effect, DecisionEffect::Deny);
    // A Cedar Allow carries no reason detail, but its bundle identity is still
    // recorded so a later cutover can compare shadow-allow vs legacy-deny.
    assert_eq!(audit.evaluated_reason_detail, None);
    assert_eq!(
        audit
            .evaluated_bundle_key
            .as_ref()
            .map(|k| k.bundle_digest.as_str()),
        Some("sha256:bundle")
    );
}

#[test]
fn observation_yields_exactly_one_audit_event_per_guarded_decision() {
    // The guard writes the shadow decision as ONE `audit_events` row inside the
    // same `with_audits` transaction. `observe_cedar_pbac_decision` is the
    // single-event source for that row: one call in, one event out. Proving the
    // one-to-one shape here means a guard that maps this event into the
    // `with_audits` events vec lands exactly one shadow row per decision.
    let branch = BranchId::new();
    let request = guard_request(Role::Admin, branch);
    let entry = legacy_only_guard_entry();

    for cedar in every_cedar_shape() {
        let enforced = evaluate_cedar_pbac_boundary(&request, Some(&entry), cedar.clone());
        let a = observe_cedar_pbac_decision(&request, Some(&entry), Some(&cedar), enforced.clone());
        let b = observe_cedar_pbac_decision(&request, Some(&entry), Some(&cedar), enforced.clone());
        // Observation is a pure, deterministic projection of one decision: the
        // same inputs always yield the same single event (no hidden fan-out,
        // no enforcement side effect).
        assert_eq!(a, b, "observation must be a deterministic single event");
        assert_eq!(a.decision, enforced);
    }
}
