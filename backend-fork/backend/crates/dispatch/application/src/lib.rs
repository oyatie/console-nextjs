//! Dispatch application layer: commands, DTOs, and audit-event builders.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use console_dispatch_domain::{DispatchResponseKind, DispatchStatus, TechnicianLoad};
use console_kernel_core::{
    AuditAction, AuditEvent, BranchId, BranchScope, KernelError, P1DispatchId, Timestamp,
    TraceContext, UserId, WorkOrderId,
};
use console_workorder_domain::{PriorityLevel, WorkOrderStatus};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct IncidentLocationInput {
    pub latitude: f64,
    pub longitude: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StartP1DispatchCommand {
    pub actor: UserId,
    pub work_order_id: WorkOrderId,
    pub incident_location: Option<IncidentLocationInput>,
    pub include_region: bool,
    pub trace: TraceContext,
    pub occurred_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RespondP1DispatchCommand {
    pub actor: UserId,
    pub dispatch_id: P1DispatchId,
    pub response: DispatchResponseKind,
    pub trace: TraceContext,
    pub occurred_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExpireP1DispatchCommand {
    pub dispatch_id: P1DispatchId,
    pub trace: TraceContext,
    pub occurred_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForceAssignP1DispatchCommand {
    pub actor: UserId,
    pub dispatch_id: P1DispatchId,
    pub mechanic_id: UserId,
    pub trace: TraceContext,
    pub occurred_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct P1DispatchSummary {
    pub id: P1DispatchId,
    pub work_order_id: WorkOrderId,
    pub branch_id: BranchId,
    pub status: DispatchStatus,
    pub incident_location: Option<IncidentLocationInput>,
    #[serde(with = "time::serde::rfc3339")]
    pub accept_window_started_at: Timestamp,
    #[serde(with = "time::serde::rfc3339")]
    pub accept_window_ends_at: Timestamp,
    pub auto_assigned_mechanic_id: Option<UserId>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub manager_force_pending_at: Option<Timestamp>,
    pub manual_call_required: bool,
    #[serde(with = "time::serde::rfc3339::option")]
    pub manual_call_required_at: Option<Timestamp>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub manual_call_cleared_at: Option<Timestamp>,
    pub target_count: i64,
    pub accepted_count: i64,
    pub declined_count: i64,
}

/// One pending P1 offer for the signed-in mechanic (UI-M3 overview inbox): a
/// BROADCASTING dispatch that fanned out to the caller, still inside its
/// accept window, with no response from the caller yet. Person-scoped by
/// construction — the owner is always bound from the authenticated principal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MyDispatchOffer {
    pub dispatch_id: P1DispatchId,
    pub work_order_id: WorkOrderId,
    pub branch_id: BranchId,
    pub request_no: String,
    #[serde(with = "time::serde::rfc3339")]
    pub accept_window_started_at: Timestamp,
    #[serde(with = "time::serde::rfc3339")]
    pub accept_window_ends_at: Timestamp,
}

/// Internal person-scoped projection for immutable action-inbox traversal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActionInboxDispatchOffer {
    pub dispatch_id: P1DispatchId,
    pub work_order_id: WorkOrderId,
    pub request_no: String,
    pub created_at: Timestamp,
    pub accept_window_started_at: Timestamp,
    pub accept_window_ends_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MyDispatchOfferPage {
    pub items: Vec<MyDispatchOffer>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct P1DispatchResponseSummary {
    pub dispatch_id: P1DispatchId,
    pub user_id: UserId,
    pub response: DispatchResponseKind,
    pub responded_at: Timestamp,
    pub score_milli: Option<i64>,
    pub gps_ranked: bool,
    pub distance_meters: Option<i64>,
    pub score_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct P1DispatchTargetSummary {
    pub dispatch_id: P1DispatchId,
    pub user_id: UserId,
    pub role: String,
    pub push_token_count: i64,
}

pub fn dispatch_audit_event(
    action: &str,
    actor: Option<UserId>,
    branch_id: BranchId,
    dispatch_id: P1DispatchId,
    trace: TraceContext,
    occurred_at: Timestamp,
) -> Result<AuditEvent, KernelError> {
    Ok(AuditEvent::new(
        actor,
        AuditAction::new(action)?,
        "p1_dispatch",
        dispatch_id.to_string(),
        trace,
        occurred_at,
    )
    .with_branch(branch_id))
}

#[must_use]
pub fn start_after_snapshot(
    work_order_id: WorkOrderId,
    target_count: i64,
    include_region: bool,
) -> serde_json::Value {
    serde_json::json!({
        "work_order_id": work_order_id,
        "status": DispatchStatus::Broadcasting,
        "target_count": target_count,
        "include_region": include_region,
    })
}

#[must_use]
pub fn response_after_snapshot(response: DispatchResponseKind) -> serde_json::Value {
    serde_json::json!({
        "response": response,
    })
}

#[must_use]
pub fn resolution_after_snapshot(
    status: DispatchStatus,
    accepted_count: i64,
    mechanic_id: Option<UserId>,
) -> serde_json::Value {
    serde_json::json!({
        "status": status,
        "accepted_count": accepted_count,
        "assigned_mechanic_id": mechanic_id,
    })
}

/// Server-authoritative subset permitted in the console dispatch queue.  The
/// type deliberately excludes terminal and approval-only work-order states.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum DispatchQueueStatus {
    Received,
    Unassigned,
    Assigned,
    InProgress,
    PartWaiting,
    Delayed,
}

impl DispatchQueueStatus {
    #[must_use]
    pub const fn as_db_str(self) -> &'static str {
        match self {
            Self::Received => "RECEIVED",
            Self::Unassigned => "UNASSIGNED",
            Self::Assigned => "ASSIGNED",
            Self::InProgress => "IN_PROGRESS",
            Self::PartWaiting => "PART_WAITING",
            Self::Delayed => "DELAYED",
        }
    }

    pub fn parse_csv(input: Option<&str>) -> Result<Vec<Self>, KernelError> {
        let Some(input) = input else {
            return Ok(vec![
                Self::Received,
                Self::Unassigned,
                Self::Assigned,
                Self::InProgress,
                Self::PartWaiting,
                Self::Delayed,
            ]);
        };
        let mut values = Vec::new();
        for token in input.split(',') {
            let value = match token.trim() {
                "RECEIVED" => Self::Received,
                "UNASSIGNED" => Self::Unassigned,
                "ASSIGNED" => Self::Assigned,
                "IN_PROGRESS" => Self::InProgress,
                "PART_WAITING" => Self::PartWaiting,
                "DELAYED" => Self::Delayed,
                _ => {
                    return Err(KernelError::validation(
                        "dispatch queue contains an unsupported status",
                    ));
                }
            };
            if !values.contains(&value) {
                values.push(value);
            }
        }
        if values.is_empty() {
            return Err(KernelError::validation(
                "dispatch queue status cannot be empty",
            ));
        }
        Ok(values)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DispatchQueueCursorPayload {
    #[serde(with = "time::serde::rfc3339")]
    as_of: Timestamp,
    #[serde(with = "time::serde::rfc3339::option")]
    target_due_at: Option<Timestamp>,
    #[serde(with = "time::serde::rfc3339")]
    updated_at: Timestamp,
    work_order_id: WorkOrderId,
}

/// Strict, opaque four-key cursor.  Base64url JSON makes client parsing
/// unnecessary while serde rejects missing/unknown fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DispatchQueueCursor(DispatchQueueCursorPayload);

impl DispatchQueueCursor {
    pub fn encode(
        as_of: Timestamp,
        target_due_at: Option<Timestamp>,
        updated_at: Timestamp,
        work_order_id: WorkOrderId,
    ) -> Result<String, KernelError> {
        let payload = DispatchQueueCursorPayload {
            as_of,
            target_due_at,
            updated_at,
            work_order_id,
        };
        let bytes = serde_json::to_vec(&payload)
            .map_err(|_| KernelError::internal("dispatch queue cursor serialization failed"))?;
        Ok(URL_SAFE_NO_PAD.encode(bytes))
    }
    pub fn decode(value: &str, now: Timestamp) -> Result<Self, KernelError> {
        let bytes = URL_SAFE_NO_PAD
            .decode(value)
            .map_err(|_| KernelError::validation("invalid dispatch queue cursor"))?;
        let payload: DispatchQueueCursorPayload = serde_json::from_slice(&bytes)
            .map_err(|_| KernelError::validation("invalid dispatch queue cursor"))?;
        if payload.as_of > now {
            return Err(KernelError::validation(
                "dispatch queue cursor is from the future",
            ));
        }
        Ok(Self(payload))
    }
    #[must_use]
    pub fn as_of(&self) -> Timestamp {
        self.0.as_of
    }
    #[must_use]
    pub fn target_due_at(&self) -> Option<Timestamp> {
        self.0.target_due_at
    }
    #[must_use]
    pub fn updated_at(&self) -> Timestamp {
        self.0.updated_at
    }
    #[must_use]
    pub fn work_order_id(&self) -> WorkOrderId {
        self.0.work_order_id
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListDispatchQueueQuery {
    pub branch_scope: BranchScope,
    pub statuses: Vec<DispatchQueueStatus>,
    pub limit: i64,
    pub after: Option<DispatchQueueCursor>,
    pub now: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DispatchQueueDispatch {
    pub id: P1DispatchId,
    pub status: DispatchStatus,
    #[serde(with = "time::serde::rfc3339")]
    pub accept_window_ends_at: Timestamp,
    pub target_count: i64,
    pub accepted_count: i64,
    pub declined_count: i64,
    pub manual_call_required: bool,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DispatchQueueItem {
    pub work_order_id: WorkOrderId,
    pub request_no: String,
    pub branch_id: BranchId,
    pub status: WorkOrderStatus,
    pub priority: PriorityLevel,
    pub symptom: String,
    pub equipment_id: uuid::Uuid,
    pub customer_id: uuid::Uuid,
    pub site_id: uuid::Uuid,
    #[serde(with = "time::serde::rfc3339::option")]
    pub target_due_at: Option<Timestamp>,
    pub assigned_mechanic_id: Option<UserId>,
    pub dispatch: Option<DispatchQueueDispatch>,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: Timestamp,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DispatchQueueStats {
    pub unassigned_count: i64,
    pub sla_due_count: i64,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DispatchQueuePage {
    pub items: Vec<DispatchQueueItem>,
    pub next_after: Option<String>,
    pub stats: DispatchQueueStats,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DispatchCandidateSummary {
    pub mechanic_id: UserId,
    pub score_milli: i64,
    pub gps_ranked: bool,
    pub distance_meters: Option<i64>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub location_recorded_at: Option<Timestamp>,
    pub workload: TechnicianLoad,
    pub score_reason: String,
    pub response: Option<DispatchResponseKind>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub responded_at: Option<Timestamp>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DispatchCandidatePage {
    pub items: Vec<DispatchCandidateSummary>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct P1DispatchResponsePage {
    pub items: Vec<P1DispatchResponseSummary>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use console_kernel_core::ErrorKind;

    #[test]
    fn queue_cursor_rejects_malformed_unknown_and_future_payloads() {
        let now = time::OffsetDateTime::UNIX_EPOCH;
        assert_eq!(
            DispatchQueueCursor::decode("not-base64", now)
                .expect_err("malformed cursors must not be accepted")
                .kind,
            ErrorKind::Validation
        );

        let cursor = DispatchQueueCursor::encode(now, None, now, WorkOrderId::new())
            .expect("cursor fields must serialize");
        assert_eq!(
            DispatchQueueCursor::decode(&cursor, now)
                .expect("complete opaque cursor must round-trip")
                .as_of(),
            now
        );

        let unknown = URL_SAFE_NO_PAD.encode(
            br#"{"as_of":"1970-01-01T00:00:00Z","target_due_at":null,"updated_at":"1970-01-01T00:00:00Z","work_order_id":"00000000-0000-0000-0000-000000000001","ignored":true}"#,
        );
        assert_eq!(
            DispatchQueueCursor::decode(&unknown, now)
                .expect_err("cursor fields are an exact contract")
                .kind,
            ErrorKind::Validation
        );

        let future = DispatchQueueCursor::encode(
            now + time::Duration::seconds(1),
            None,
            now,
            WorkOrderId::new(),
        )
        .expect("cursor fields must serialize");
        assert_eq!(
            DispatchQueueCursor::decode(&future, now)
                .expect_err("future snapshot cursors are invalid")
                .kind,
            ErrorKind::Validation
        );
    }

    #[test]
    fn queue_statuses_are_bounded_and_deduplicated() {
        let parsed = DispatchQueueStatus::parse_csv(Some("RECEIVED,DELAYED,RECEIVED")).unwrap();
        assert_eq!(
            parsed,
            vec![DispatchQueueStatus::Received, DispatchQueueStatus::Delayed]
        );
        assert_eq!(
            DispatchQueueStatus::parse_csv(Some("CLOSED"))
                .expect_err("queue excludes terminal statuses")
                .kind,
            ErrorKind::Validation
        );
    }
}
