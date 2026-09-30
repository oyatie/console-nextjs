use std::str::FromStr;

use console_kernel_core::{BranchId, KernelError, OrgId, UserId};
use console_platform_authz::{AuthorizationAuditEvent, Feature, Principal};

use crate::authz_guard::{
    WAITING_COMPLETION_DOMAIN, build_guard_request, guard, workflow_coexistence_entry,
};

/// Finalization mode requested by the waiting-task completion API.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FinalizeMode {
    /// The original submitting author closes the approval document.
    Author,
    /// A policy-authorized delegate closes on the author's behalf and must state why.
    Delegate,
}

/// Server facts needed to enforce finalization policy before the task mutation.
#[derive(Debug)]
pub struct FinalizePolicyRequest<'a> {
    pub mode: FinalizeMode,
    pub reason: Option<&'a str>,
    pub required_policy: Option<&'a str>,
    pub principal: &'a Principal,
    pub org: OrgId,
    pub branch: BranchId,
    pub resource_type: &'a str,
    pub resource_id: String,
    pub initiated_by: UserId,
}

/// Policy evidence the persistence layer must carry into its audited mutation.
#[derive(Debug, Clone)]
pub struct FinalizePolicyOutcome {
    pub delegated_reason: Option<String>,
    pub guard_audit: Option<AuthorizationAuditEvent>,
}

/// Enforce author/delegate finalization rules before completing the waiting task.
///
/// Author finalization is only for the original initiator. Delegated finalization
/// is policy-gated through the existing LegacyOnly + inert Cedar-shadow guard and
/// requires a non-blank reason before the guard is evaluated.
pub fn enforce_finalize_policy(
    request: FinalizePolicyRequest<'_>,
) -> Result<FinalizePolicyOutcome, KernelError> {
    match request.mode {
        FinalizeMode::Author => {
            if request.principal.user_id != request.initiated_by {
                return Err(KernelError::forbidden(
                    "author finalize requires the initiating author",
                ));
            }
            Ok(FinalizePolicyOutcome {
                delegated_reason: None,
                guard_audit: None,
            })
        }
        FinalizeMode::Delegate => {
            let reason = request
                .reason
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| {
                    KernelError::validation("delegated finalize requires a non-empty reason")
                })?;
            let required_policy = request.required_policy.ok_or_else(|| {
                KernelError::forbidden("delegated finalize requires a configured policy")
            })?;
            let feature = Feature::from_str(required_policy)
                .map_err(|_| KernelError::forbidden("delegated finalize policy is unknown"))?;
            let authz_request = build_guard_request(
                request.principal,
                required_policy,
                request.org,
                request.branch,
                request.resource_type,
                &request.resource_id,
                WAITING_COMPLETION_DOMAIN,
            )
            .map_err(|_| KernelError::forbidden("delegated finalize policy denied"))?;
            let entry = workflow_coexistence_entry(
                "workflow.waiting_task.finalize.delegate",
                WAITING_COMPLETION_DOMAIN,
                feature,
                request.resource_type,
            );
            let outcome = guard(&authz_request, &entry);
            if !outcome.is_allowed() {
                return Err(KernelError::forbidden("delegated finalize policy denied"));
            }

            Ok(FinalizePolicyOutcome {
                delegated_reason: Some(reason.to_owned()),
                guard_audit: Some(outcome.audit),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use console_kernel_core::{BranchId, BranchScope, ErrorKind, OrgId, UserId};
    use console_platform_authz::{DecisionEngine, DecisionReason, Principal, Role};
    use uuid::Uuid;

    use super::*;

    fn principal(role: Role, scope: BranchScope) -> Principal {
        Principal::new(UserId::new(), OrgId::knl(), BTreeSet::from([role]), scope)
    }

    fn delegate_request<'a>(
        principal: &'a Principal,
        branch: BranchId,
        reason: Option<&'a str>,
    ) -> FinalizePolicyRequest<'a> {
        FinalizePolicyRequest {
            mode: FinalizeMode::Delegate,
            reason,
            required_policy: Some("approval_finalize"),
            principal,
            org: OrgId::knl(),
            branch,
            resource_type: "approval_document",
            resource_id: Uuid::new_v4().to_string(),
            initiated_by: UserId::new(),
        }
    }

    #[test]
    fn delegated_finalize_requires_non_blank_reason_before_authorization() {
        let branch = BranchId::new();
        let principal = principal(Role::SuperAdmin, BranchScope::single(branch));

        let err = enforce_finalize_policy(delegate_request(&principal, branch, Some("  ")))
            .expect_err("blank delegated-finalize reason must be rejected");

        assert_eq!(err.kind, ErrorKind::Validation);
        assert!(err.message.contains("reason"));
    }

    #[test]
    fn delegated_finalize_denies_when_legacy_guard_denies() {
        let branch = BranchId::new();
        let principal = principal(Role::Member, BranchScope::single(branch));

        let err = enforce_finalize_policy(delegate_request(
            &principal,
            branch,
            Some("author is unavailable"),
        ))
        .expect_err("member must not be allowed to delegated-finalize");

        assert_eq!(err.kind, ErrorKind::Forbidden);
    }

    #[test]
    fn delegated_finalize_allows_with_reason_and_records_legacy_shadow() {
        let branch = BranchId::new();
        let principal = principal(Role::SuperAdmin, BranchScope::single(branch));

        let outcome = enforce_finalize_policy(delegate_request(
            &principal,
            branch,
            Some("author is unavailable"),
        ))
        .expect("super admin can delegated-finalize with reason");

        assert_eq!(
            outcome.delegated_reason.as_deref(),
            Some("author is unavailable")
        );
        let audit = outcome
            .guard_audit
            .expect("delegated finalization is policy-gated");
        assert_eq!(audit.decision.engine, DecisionEngine::Legacy);
        assert_eq!(audit.decision.reason, DecisionReason::LegacyAllowed);
    }

    #[test]
    fn author_finalize_requires_the_initiating_author() {
        let branch = BranchId::new();
        let principal = principal(Role::Admin, BranchScope::single(branch));
        let err = enforce_finalize_policy(FinalizePolicyRequest {
            mode: FinalizeMode::Author,
            reason: None,
            required_policy: Some("approval_finalize"),
            principal: &principal,
            org: OrgId::knl(),
            branch,
            resource_type: "approval_document",
            resource_id: Uuid::new_v4().to_string(),
            initiated_by: UserId::new(),
        })
        .expect_err("non-author cannot use author finalize mode");

        assert_eq!(err.kind, ErrorKind::Forbidden);
    }
}
