//! Governance domain — pure lifecycle FSM + §16 guardrail gate-chain logic.
//!
//! No I/O, no sqlx, no authz crate: this layer only knows kernel types (the
//! layer-boundary gate restricts domain crates to `console-kernel-core`). The
//! Authority gate consumes an [`AuthorityEffect`] that the outer layers derive
//! from the Cedar evaluator's `DecisionEffect`; the gate chain itself is pure so
//! it is trivially unit-testable and re-runnable inside a writeback transaction.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

use console_kernel_core::KernelError;
use serde::{Deserialize, Serialize};

// ===========================================================================
// §15 / §20 instance lifecycle FSM.
// ===========================================================================

/// Instance lifecycle state (arch §3b): `draft → active → (locked?) → archived
/// → disposed`. Soft-archive is reversible (`archived → active`); dispose is
/// terminal (no edge leaves it). There is **no hard delete** — dispose is the
/// only terminal state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum LifecycleState {
    Draft,
    Active,
    Locked,
    Archived,
    Disposed,
}

impl LifecycleState {
    #[must_use]
    pub const fn as_db_str(self) -> &'static str {
        match self {
            Self::Draft => "DRAFT",
            Self::Active => "ACTIVE",
            Self::Locked => "LOCKED",
            Self::Archived => "ARCHIVED",
            Self::Disposed => "DISPOSED",
        }
    }

    pub fn from_db_str(value: &str) -> Result<Self, KernelError> {
        match value {
            "DRAFT" => Ok(Self::Draft),
            "ACTIVE" => Ok(Self::Active),
            "LOCKED" => Ok(Self::Locked),
            "ARCHIVED" => Ok(Self::Archived),
            "DISPOSED" => Ok(Self::Disposed),
            other => Err(KernelError::validation(format!(
                "unknown lifecycle state {other:?}"
            ))),
        }
    }

    /// `true` once the object is disposed — the terminal soft state. A disposed
    /// object can never transition again (append-only, no resurrection).
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Disposed)
    }

    /// A non-draft state edits to which require a post-draft override
    /// (`{reason, four-eyes, before-snapshot}`, arch §3b).
    #[must_use]
    pub const fn requires_override_to_edit(self) -> bool {
        matches!(self, Self::Active | Self::Locked)
    }
}

/// Base legal FSM edges. A per-object-type config (`gov_lifecycle_transitions`)
/// may only be a **subset** of these — it can never legalize an edge the base
/// FSM forbids (e.g. `disposed → active`, which no row here permits).
pub const LIFECYCLE_TRANSITIONS: &[(LifecycleState, LifecycleState)] = &[
    (LifecycleState::Draft, LifecycleState::Active),
    (LifecycleState::Draft, LifecycleState::Archived),
    (LifecycleState::Active, LifecycleState::Locked),
    (LifecycleState::Active, LifecycleState::Archived),
    (LifecycleState::Locked, LifecycleState::Active),
    (LifecycleState::Locked, LifecycleState::Archived),
    // Soft-archive is reversible.
    (LifecycleState::Archived, LifecycleState::Active),
    // Dispose is terminal (four-eyes + retention gated at the guardrail layer).
    (LifecycleState::Archived, LifecycleState::Disposed),
];

/// Validate a transition against the base FSM. Callers must ALSO confirm the
/// edge is configured for the object type (`gov_lifecycle_transitions`) and run
/// the guardrail gate chain for its `requires_*` flags.
pub fn validate_lifecycle_transition(
    from: LifecycleState,
    to: LifecycleState,
) -> Result<(), KernelError> {
    if from.is_terminal() {
        return Err(KernelError::conflict(format!(
            "object is disposed (terminal); no transition to {} is possible",
            to.as_db_str()
        )));
    }
    if LIFECYCLE_TRANSITIONS
        .iter()
        .any(|&(f, t)| f == from && t == to)
    {
        Ok(())
    } else {
        Err(KernelError::invalid_transition(format!(
            "illegal lifecycle transition {} -> {}",
            from.as_db_str(),
            to.as_db_str()
        )))
    }
}

/// The `requires_*` flags a configured transition carries; each maps to a
/// guardrail gate that must be satisfied before the transition commits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransitionRequirements {
    pub requires_reason: bool,
    pub requires_four_eyes: bool,
    pub requires_checklist: bool,
}

// ===========================================================================
// §16 guardrail gate chain — ordered, fail-closed.
// ===========================================================================

/// The four fixed gates, in evaluation order (arch §16). Order is a hard
/// invariant: cheaper/authority-first, egress last.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GateKind {
    /// Cedar `authorize(...)` — the outer layers feed in the evaluator's effect.
    Authority,
    /// Required self-acknowledgement checklist all checked.
    SelfChecklist,
    /// A four-eyes approval by a principal distinct from the requester.
    FourEyes,
    /// Outbound side-effects pass the egress / DLP classifier.
    EgressDlp,
}

impl GateKind {
    /// Fixed evaluation order.
    pub const ORDER: [GateKind; 4] = [
        GateKind::Authority,
        GateKind::SelfChecklist,
        GateKind::FourEyes,
        GateKind::EgressDlp,
    ];
}

/// Which gates are active for one action (arch: `ont_action_types.control_points`).
/// A gate that is not required is skipped (`NotRequired`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct GateChainConfig {
    pub authority: bool,
    pub self_checklist: bool,
    pub four_eyes: bool,
    pub egress_dlp: bool,
}

impl GateChainConfig {
    #[must_use]
    pub const fn requires(&self, gate: GateKind) -> bool {
        match gate {
            GateKind::Authority => self.authority,
            GateKind::SelfChecklist => self.self_checklist,
            GateKind::FourEyes => self.four_eyes,
            GateKind::EgressDlp => self.egress_dlp,
        }
    }
}

/// Cedar decision effect, mirrored into the domain so this crate stays
/// kernel-only. Outer layers map `console_platform_authz::cedar_pbac::DecisionEffect`
/// onto this.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthorityEffect {
    Allow,
    Deny,
}

/// Evidence gathered for each gate. Every field is optional: `None` means the
/// gate could not be evaluated (not wired / evidence absent), which the chain
/// treats as **fail-closed deny** for any required gate — a missing gate can
/// never pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct GateEvidence {
    /// Cedar authorize result.
    pub authority: Option<AuthorityEffect>,
    /// `Some(true)` iff every required checklist item is acknowledged.
    pub checklist_all_acknowledged: Option<bool>,
    /// A four-eyes approval exists, is `approved`, and the approver differs from
    /// the requester (the DB CHECK also enforces distinctness).
    pub four_eyes_approved: Option<bool>,
    /// Outbound side-effects cleared the egress / DLP classifier.
    pub egress_cleared: Option<bool>,
}

/// Per-gate outcome. `Pending` is a recoverable "not yet satisfied" (awaiting a
/// checklist ack or an approver); `Denied` is a hard stop (authority/egress deny
/// or a required-but-unevaluated gate).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum GateStatus {
    NotRequired,
    Satisfied,
    Pending { reason: String },
    Denied { reason: String },
}

impl GateStatus {
    #[must_use]
    pub const fn passes(&self) -> bool {
        matches!(self, Self::NotRequired | Self::Satisfied)
    }
}

/// One gate's evaluated line for preflight display + the final verdict.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GateOutcome {
    pub gate: GateKind,
    pub status: GateStatus,
}

/// The whole chain's result. `allow` is true only when every gate `passes()`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GateChainOutcome {
    pub gates: Vec<GateOutcome>,
    pub allow: bool,
}

impl GateChainOutcome {
    /// The first gate that blocked (for a concise deny message), if any.
    #[must_use]
    pub fn first_blocking(&self) -> Option<&GateOutcome> {
        self.gates.iter().find(|g| !g.status.passes())
    }
}

/// Evaluate the ordered gate chain, fail-closed.
///
/// Invariants proved by the unit tests:
/// * gates are reported in fixed [`GateKind::ORDER`];
/// * a required gate with no evidence (`None`) ⇒ `Denied` ⇒ overall deny —
///   nothing a caller can do makes a missing gate pass;
/// * the verdict is `allow` iff every gate passes.
#[must_use]
pub fn evaluate_gate_chain(config: GateChainConfig, evidence: &GateEvidence) -> GateChainOutcome {
    let gates: Vec<GateOutcome> = GateKind::ORDER
        .iter()
        .map(|&gate| GateOutcome {
            gate,
            status: evaluate_gate(gate, config, evidence),
        })
        .collect();
    let allow = gates.iter().all(|g| g.status.passes());
    GateChainOutcome { gates, allow }
}

fn evaluate_gate(gate: GateKind, config: GateChainConfig, evidence: &GateEvidence) -> GateStatus {
    if !config.requires(gate) {
        return GateStatus::NotRequired;
    }
    match gate {
        GateKind::Authority => match evidence.authority {
            Some(AuthorityEffect::Allow) => GateStatus::Satisfied,
            Some(AuthorityEffect::Deny) => GateStatus::Denied {
                reason: "Cedar authorize denied the action".to_owned(),
            },
            None => fail_closed("authority gate was not evaluated"),
        },
        GateKind::SelfChecklist => match evidence.checklist_all_acknowledged {
            Some(true) => GateStatus::Satisfied,
            Some(false) => GateStatus::Pending {
                reason: "required checklist items are not all acknowledged".to_owned(),
            },
            None => fail_closed("checklist gate was not evaluated"),
        },
        GateKind::FourEyes => match evidence.four_eyes_approved {
            Some(true) => GateStatus::Satisfied,
            Some(false) => GateStatus::Pending {
                reason: "awaiting four-eyes approval from a distinct principal".to_owned(),
            },
            None => fail_closed("four-eyes gate was not evaluated"),
        },
        GateKind::EgressDlp => match evidence.egress_cleared {
            Some(true) => GateStatus::Satisfied,
            Some(false) => GateStatus::Denied {
                reason: "outbound side-effect failed the egress / DLP classifier".to_owned(),
            },
            None => fail_closed("egress / DLP gate was not evaluated"),
        },
    }
}

/// A required gate with no evidence is a hard deny — never a silent pass.
fn fail_closed(reason: &str) -> GateStatus {
    GateStatus::Denied {
        reason: format!("fail-closed: {reason}"),
    }
}

// ===========================================================================
// Distinct-natural-person four-eyes scope (console-dgo.1 / PRODUCT invariant).
// ===========================================================================

/// Kind prefixes whose open/decide path must resolve requester and approver to
/// distinct natural persons via `users.employee_id → employee_person_bindings →
/// Person`.
///
/// Scoped to **new Company / HR / Payroll** surfaces only. Generic kinds
/// (`override`, `ontology.lifecycle`, `console_view.team_deploy`, …) stay on the
/// account-level `approver_id <> requested_by` bar — retrofitting the Person
/// resolution onto them fails every normal account whose `users.employee_id` is
/// NULL (migration 0076).
#[must_use]
pub fn requires_natural_person_four_eyes(kind: &str) -> bool {
    let kind = kind.trim();
    kind.starts_with("company.") || kind.starts_with("hr.") || kind.starts_with("payroll.")
}

// ===========================================================================
// Impact preflight (arch §15) — dependency scan before archive/dispose.
// ===========================================================================

/// How a dependent edge reacts when its target is archived/disposed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OnDelete {
    /// Blocks the transition while the dependent exists.
    Restrict,
    /// Allowed to remain / cascade-detach; does not block.
    Detach,
}

/// A dependent discovered by the (ontology-owned) dependency scan, passed into
/// the pure impact assessment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Dependent {
    pub kind: String,
    pub id: String,
    pub on_delete: OnDelete,
}

/// Result of an impact preflight.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImpactAssessment {
    /// Transition is safe only if no `Restrict` dependents block it.
    pub allow: bool,
    pub blocking: Vec<Dependent>,
    pub total_dependents: usize,
}

/// Fail if any `on_delete = restrict` dependent exists (arch §15).
#[must_use]
pub fn assess_impact(dependents: Vec<Dependent>) -> ImpactAssessment {
    let blocking: Vec<Dependent> = dependents
        .iter()
        .filter(|d| d.on_delete == OnDelete::Restrict)
        .cloned()
        .collect();
    ImpactAssessment {
        allow: blocking.is_empty(),
        total_dependents: dependents.len(),
        blocking,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_fsm_allows_documented_edges_and_rejects_the_rest() {
        assert!(
            validate_lifecycle_transition(LifecycleState::Draft, LifecycleState::Active).is_ok()
        );
        assert!(
            validate_lifecycle_transition(LifecycleState::Archived, LifecycleState::Active).is_ok(),
            "soft-archive must be reversible"
        );
        assert!(
            validate_lifecycle_transition(LifecycleState::Archived, LifecycleState::Disposed)
                .is_ok()
        );
        // No resurrection from the terminal state.
        assert!(
            validate_lifecycle_transition(LifecycleState::Disposed, LifecycleState::Active)
                .is_err()
        );
        // Draft cannot jump straight to disposed.
        assert!(
            validate_lifecycle_transition(LifecycleState::Draft, LifecycleState::Disposed).is_err()
        );
    }

    fn require_all() -> GateChainConfig {
        GateChainConfig {
            authority: true,
            self_checklist: true,
            four_eyes: true,
            egress_dlp: true,
        }
    }

    #[test]
    fn gates_are_reported_in_fixed_order() {
        let outcome = evaluate_gate_chain(GateChainConfig::default(), &GateEvidence::default());
        let order: Vec<GateKind> = outcome.gates.iter().map(|g| g.gate).collect();
        assert_eq!(order, GateKind::ORDER.to_vec());
    }

    #[test]
    fn all_satisfied_allows() {
        let evidence = GateEvidence {
            authority: Some(AuthorityEffect::Allow),
            checklist_all_acknowledged: Some(true),
            four_eyes_approved: Some(true),
            egress_cleared: Some(true),
        };
        let outcome = evaluate_gate_chain(require_all(), &evidence);
        assert!(outcome.allow);
        assert!(outcome.first_blocking().is_none());
    }

    #[test]
    fn missing_required_gate_fails_closed() {
        // Only four-eyes required, but no evidence supplied for it.
        let config = GateChainConfig {
            four_eyes: true,
            ..GateChainConfig::default()
        };
        let outcome = evaluate_gate_chain(config, &GateEvidence::default());
        assert!(!outcome.allow, "a missing required gate must deny");
        let blocking = outcome.first_blocking().unwrap();
        assert_eq!(blocking.gate, GateKind::FourEyes);
        assert!(matches!(blocking.status, GateStatus::Denied { .. }));
    }

    #[test]
    fn authority_deny_is_hard_stop() {
        let evidence = GateEvidence {
            authority: Some(AuthorityEffect::Deny),
            checklist_all_acknowledged: Some(true),
            four_eyes_approved: Some(true),
            egress_cleared: Some(true),
        };
        let outcome = evaluate_gate_chain(require_all(), &evidence);
        assert!(!outcome.allow);
        assert_eq!(outcome.first_blocking().unwrap().gate, GateKind::Authority);
    }

    #[test]
    fn unrequired_gates_do_not_block() {
        // Nothing required → allow with no evidence at all.
        let outcome = evaluate_gate_chain(GateChainConfig::default(), &GateEvidence::default());
        assert!(outcome.allow);
        assert!(
            outcome
                .gates
                .iter()
                .all(|g| g.status == GateStatus::NotRequired)
        );
    }

    #[test]
    fn restrict_dependent_blocks_impact() {
        let dependents = vec![
            Dependent {
                kind: "ont_link".to_owned(),
                id: "a".to_owned(),
                on_delete: OnDelete::Detach,
            },
            Dependent {
                kind: "ont_link".to_owned(),
                id: "b".to_owned(),
                on_delete: OnDelete::Restrict,
            },
        ];
        let assessment = assess_impact(dependents);
        assert!(!assessment.allow);
        assert_eq!(assessment.blocking.len(), 1);
        assert_eq!(assessment.total_dependents, 2);
    }

    #[test]
    fn four_eyes_false_is_pending_not_allow() {
        // Distinct-human gate: not yet approved is pending, not a silent pass.
        let config = GateChainConfig {
            four_eyes: true,
            ..GateChainConfig::default()
        };
        let evidence = GateEvidence {
            four_eyes_approved: Some(false),
            ..GateEvidence::default()
        };
        let outcome = evaluate_gate_chain(config, &evidence);
        assert!(!outcome.allow);
        let blocking = outcome.first_blocking().unwrap();
        assert_eq!(blocking.gate, GateKind::FourEyes);
        assert!(matches!(blocking.status, GateStatus::Pending { .. }));
    }

    #[test]
    fn impact_preflight_allows_when_only_detach_dependents() {
        let dependents = vec![Dependent {
            kind: "ont_link".to_owned(),
            id: "x".to_owned(),
            on_delete: OnDelete::Detach,
        }];
        let assessment = assess_impact(dependents);
        assert!(assessment.allow);
        assert!(assessment.blocking.is_empty());
        assert_eq!(assessment.total_dependents, 1);
    }

    #[test]
    fn gate_kind_order_is_authority_checklist_four_eyes_egress() {
        assert_eq!(
            GateKind::ORDER.as_slice(),
            [
                GateKind::Authority,
                GateKind::SelfChecklist,
                GateKind::FourEyes,
                GateKind::EgressDlp,
            ]
            .as_slice()
        );
    }

    #[test]
    fn natural_person_four_eyes_scopes_only_company_hr_payroll_kinds() {
        for kind in [
            "company.revise",
            "hr.appoint",
            "hr.promote",
            "hr.transfer",
            "payroll.create_run",
            "payroll.submit_run",
            "payroll.decide_run",
            "  payroll.create_run  ",
        ] {
            assert!(
                requires_natural_person_four_eyes(kind),
                "{kind} must require natural-person four-eyes"
            );
        }
        for kind in [
            "override",
            "ontology.lifecycle",
            "console_view.team_deploy",
            "ontology.schema.publish",
            "organization.create_org_unit",
            "organization.revise_org_unit",
            "organization.create_job_position",
            "organization.revise_job_position",
            "organization.",
            "people.create_person",
            "people.revise_person",
            "people.",
            "Company.revise",
            "",
            "company",
            "hr",
        ] {
            assert!(
                !requires_natural_person_four_eyes(kind),
                "{kind} must NOT retrofit natural-person four-eyes"
            );
        }
    }
}
