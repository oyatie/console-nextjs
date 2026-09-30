//! Deterministic idempotency/natural keys for the three exactly-once levels
//! (design §B).
//!
//! Each key backs a reused spine `UNIQUE` constraint:
//! * run → `workflow_runs.UNIQUE(org_id, idempotency_key)` (0077:34)
//! * node → `workflow_node_runs.UNIQUE(org_id, idempotency_key)` (0077:69)
//! * outbox → `workflow_outbox_events.UNIQUE(org_id, idempotency_key)` (0077:149)
//!
//! All keys are ≥16 chars, satisfying the
//! `char_length(btrim(...)) BETWEEN 16 AND 200` CHECKs.

use console_kernel_core::WorkOrderId;
use uuid::Uuid;

/// Run-level key for a work-order completion run: `run:work_order:{id}:completion:v1`.
#[must_use]
pub fn run_completion_key(work_order_id: WorkOrderId) -> String {
    format!("run:work_order:{work_order_id}:completion:v1")
}

/// Node-level key: `node:{run_id}:{node_key}:{attempt}`. A retry is a new row at
/// `attempt + 1`, so it derives a distinct key under
/// `UNIQUE(org_id, run_id, node_key, attempt)`.
#[must_use]
pub fn node_attempt_key(run_id: Uuid, node_key: &str, attempt: i32) -> String {
    format!("node:{run_id}:{node_key}:{attempt}")
}

/// Outbox-level key: `outbox:{run_id}:{node_run_id}:{channel}:{logical}`.
#[must_use]
pub fn outbox_key(run_id: Uuid, node_run_id: Uuid, channel: &str, logical: &str) -> String {
    format!("outbox:{run_id}:{node_run_id}:{channel}:{logical}")
}

/// Convenience for a JOB-channel outbox emission, e.g.
/// `outbox:{run_id}:{node_run_id}:job:payroll_draft`.
#[must_use]
pub fn outbox_job_key(run_id: Uuid, node_run_id: Uuid, logical: &str) -> String {
    outbox_key(run_id, node_run_id, "job", logical)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_meet_the_spine_length_floor() {
        let run = Uuid::from_u128(0x1234);
        let node = Uuid::from_u128(0x5678);
        let wo = WorkOrderId::from_uuid(Uuid::from_u128(0x9abc));
        assert!(run_completion_key(wo).len() >= 16);
        assert!(node_attempt_key(run, "payroll.draft_gate", 1).len() >= 16);
        assert_eq!(
            outbox_job_key(run, node, "payroll_draft"),
            format!("outbox:{run}:{node}:job:payroll_draft")
        );
    }
}
