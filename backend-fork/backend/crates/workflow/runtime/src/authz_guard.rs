//! Cedar/PBAC authorization for workflow-runtime guards (design §D).
//!
//! Two guard call sites exist: every business-mutating node transition, and every
//! waiting-task completion. Both build an [`AuthorizationRequest`] from SERVER
//! data only and run it through the real `console_platform_authz` boundary.
//!
//! ## M2 posture: `LegacyOnly`, observe-and-record
//! The coexistence-map entry is pinned to [`DualEngineMode::LegacyOnly`]. Cedar is
//! not wired yet, so the adapter verdict is inert ([`CedarEvaluation::NotConfigured`]).
//! Under `LegacyOnly` the boundary returns exactly what the legacy role matrix
//! decides; the inert Cedar verdict is only recorded in the audit event, never
//! enforced. Enrolling workflow in a `cedar_*` mode today would deny every
//! transition (`BundleUnavailable`) because no compiled bundle / subject freshness
//! exists — that flip is a later, separate charter.

use std::str::FromStr;

use console_kernel_core::{BranchId, KernelError, OrgId};
use console_platform_authz::{
    Action, AuthorizationAuditEvent, AuthorizationDecision, AuthorizationRequest,
    AuthorizationResource, CedarEvaluation, CoexistenceMapEntry, DualEngineMode, Feature,
    Principal, RlsScopeProof, evaluate_cedar_pbac_boundary, observe_cedar_pbac_decision,
};

/// Policy domain for a business-mutating node transition guard.
pub const NODE_TRANSITION_DOMAIN: &str = "workflow.node_transition";

/// Policy domain for a waiting-task completion guard.
pub const WAITING_COMPLETION_DOMAIN: &str = "workflow.waiting_task_completion";

/// Build the pinned-`LegacyOnly` coexistence-map entry for a workflow guard.
///
/// The bundle key is intentionally `None`: under `LegacyOnly` Cedar is not
/// required, so no compiled bundle is consulted. Flipping the mode later is the
/// only place a real bundle key is attached.
#[must_use]
pub fn workflow_coexistence_entry(
    id: impl Into<String>,
    domain: impl Into<String>,
    feature: Feature,
    resource_type: impl Into<String>,
) -> CoexistenceMapEntry {
    CoexistenceMapEntry::new(
        id,
        domain,
        feature,
        resource_type,
        DualEngineMode::LegacyOnly,
        None,
    )
}

/// Build a node-transition / waiting-task authorization request from server facts.
///
/// * `required_policy` — the waiting task's `required_policy` (or the node's
///   required feature). An unknown policy fails closed here (→ `Err`), which the
///   caller treats as a deny, matching the boundary's "unknown → deny" contract.
/// * `resource` — `branch(org, branch_id, resource_type).with_resource_id(object_id)`
///   from `workflow_runs.object_type/object_id`.
/// * the RLS scope proof witnesses that the DB reads used to build this request
///   ran under the armed `console_rt`/`app.current_org` scope for `org`; the boundary
///   rejects any org mismatch between principal, resource, and proof.
pub fn build_guard_request(
    principal: &Principal,
    required_policy: &str,
    org: OrgId,
    branch_id: BranchId,
    resource_type: &str,
    object_id: &str,
    domain: &str,
) -> Result<AuthorizationRequest, KernelError> {
    let feature = Feature::from_str(required_policy)?;
    let resource = AuthorizationResource::branch(org, branch_id, resource_type.to_owned())
        .with_resource_id(object_id.to_owned());
    Ok(
        AuthorizationRequest::new(principal.clone(), Action::new(feature), resource)
            .with_policy_domain(domain.to_owned())
            .with_rls_scope_proof(RlsScopeProof::runtime_role_guc(org)),
    )
}

/// The enforced decision plus the single audit event a guard hands to
/// `with_audits`.
#[derive(Debug, Clone)]
pub struct GuardOutcome {
    pub decision: AuthorizationDecision,
    pub audit: AuthorizationAuditEvent,
}

impl GuardOutcome {
    /// Whether the enforced decision allows the guarded action.
    #[must_use]
    pub fn is_allowed(&self) -> bool {
        self.decision.effect.is_allow()
    }
}

/// Evaluate a workflow guard and produce the observe-and-record outcome.
///
/// Cedar is inert at M2 ([`CedarEvaluation::NotConfigured`]); under `LegacyOnly`
/// the enforced decision is the legacy verdict, and observation is *fed* that
/// already-enforced decision so it can record but never mutate it. The returned
/// audit event carries the enforced decision plus the (inert) Cedar shadow detail
/// for the forensic trail — exactly one event, ready for `with_audits`.
#[must_use]
pub fn guard(request: &AuthorizationRequest, entry: &CoexistenceMapEntry) -> GuardOutcome {
    let cedar = CedarEvaluation::NotConfigured;
    let decision = evaluate_cedar_pbac_boundary(request, Some(entry), cedar.clone());
    let audit = observe_cedar_pbac_decision(request, Some(entry), Some(&cedar), decision.clone());
    GuardOutcome { decision, audit }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use console_kernel_core::{BranchScope, UserId};
    use console_platform_authz::{DecisionEngine, DecisionReason, Role};

    use super::*;

    fn admin_principal(branch: BranchId) -> Principal {
        Principal::new(
            UserId::new(),
            OrgId::knl(),
            BTreeSet::from([Role::Admin]),
            BranchScope::single(branch),
        )
    }

    #[test]
    fn legacy_only_allow_is_recorded_not_mutated() {
        let branch = BranchId::new();
        let entry = workflow_coexistence_entry(
            "workflow.node_transition.completion_review",
            NODE_TRANSITION_DOMAIN,
            Feature::CompletionReview,
            "work_order",
        );
        let request = build_guard_request(
            &admin_principal(branch),
            "completion_review",
            OrgId::knl(),
            branch,
            "work_order",
            "work_order:node-run-1",
            NODE_TRANSITION_DOMAIN,
        )
        .unwrap();

        let outcome = guard(&request, &entry);
        assert!(outcome.is_allowed());
        assert_eq!(outcome.decision.engine, DecisionEngine::Legacy);
        assert_eq!(outcome.decision.reason, DecisionReason::LegacyAllowed);
        assert_eq!(outcome.decision.mode, Some(DualEngineMode::LegacyOnly));
        // observation records exactly the enforced decision.
        assert_eq!(outcome.audit.decision, outcome.decision);
    }

    #[test]
    fn unknown_policy_fails_closed() {
        let branch = BranchId::new();
        let err = build_guard_request(
            &admin_principal(branch),
            "payroll.legal_gate", // not a Feature key
            OrgId::knl(),
            branch,
            "work_order",
            "work_order:node-run-1",
            NODE_TRANSITION_DOMAIN,
        );
        assert!(err.is_err(), "an unknown required_policy must fail closed");
    }
}
