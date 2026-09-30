//! Workflow runtime engine.
//!
//! Application-style logic only — no sqlx/axum/tokio. All persistence goes through
//! [`console_workflow_domain::WorkflowRuntimePort`], which the Postgres adapter
//! implements, so this crate is DB-free and unit-testable. It provides:
//!
//! * [`idempotency`] — the run/node/outbox idempotency-key derivations (design §B)
//!   that back the spine's `UNIQUE(org_id, idempotency_key)` exactly-once guards.
//! * [`authz_guard`] — the Cedar/PBAC authorization-request builder + the
//!   observe-and-record guard (design §D). Pinned to `LegacyOnly` at M2: the
//!   boundary delegates to the legacy role matrix and only *records* the (inert)
//!   Cedar verdict.
//! * [`interpreter`] — the pure per-node interpreter that turns a typed node spec
//!   into a [`interpreter::NodeOutcome`] (succeed + emit / park waiting / fail).
//! * [`engine`] — the FSM-driven advance logic that walks a run/node through the
//!   domain transition tables and commits each step atomically via the port.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

pub mod authz_guard;
pub mod completion;
pub mod engine;
pub mod graph;
pub mod idempotency;
pub mod interpreter;
pub mod predicate;
pub mod trigger;

pub use authz_guard::{
    GuardOutcome, NODE_TRANSITION_DOMAIN, WAITING_COMPLETION_DOMAIN, build_guard_request, guard,
    workflow_coexistence_entry,
};
pub use completion::{
    FinalizeMode, FinalizePolicyOutcome, FinalizePolicyRequest, enforce_finalize_policy,
};
pub use engine::{
    AuditContext, NodeStepOutcome, ProcessNodeRequest, StartRunRequest, process_node, start_run,
};
pub use graph::{DriveOutcome, ExecGraph, drive_from, simulate_path};
pub use interpreter::{
    CHECKLIST_ATTESTATION_NODE_TYPE, ChecklistAttestationSpec, ChecklistItemKind,
    ChecklistItemSpec, EGRESS_POLICY_NODE_TYPE, EgressExternalRecipientPolicy, EgressKind,
    EgressPolicySpec, FOUR_EYES_PEER_REVIEW_NODE_TYPE, FourEyesPeerReviewSpec, HumanTaskSemantic,
    NodeKind, NodeOutcome, NodeSpec, OnMissingFact, SEGREGATION_OF_DUTIES_NODE_TYPE,
    SegregationOfDutiesSpec, SodMode, interpret_node,
};
pub use predicate::{CmpOp, Predicate};
pub use trigger::{TriggeredStart, start_bound_run};
