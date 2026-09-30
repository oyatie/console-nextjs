//! Pure P1 dispatch FSM and candidate scoring.
//!
//! ADR-0006 keeps these states separate from the 16-state work-order FSM:
//! accepting a P1 broadcast decides who should be assigned, then the work-order
//! adapter performs the ordinary `ASSIGNED` transition.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

use console_kernel_core::{
    KernelError, P1DispatchId, Timestamp, Transition, TransitionError, UserId, WorkOrderId,
};
use serde::{Deserialize, Serialize};
use time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum DispatchStatus {
    Broadcasting,
    AutoAssigned,
    ManagerForcePending,
}

impl DispatchStatus {
    #[must_use]
    pub const fn as_db_str(self) -> &'static str {
        match self {
            Self::Broadcasting => "BROADCASTING",
            Self::AutoAssigned => "AUTO_ASSIGNED",
            Self::ManagerForcePending => "MANAGER_FORCE_PENDING",
        }
    }

    pub fn from_db_str(value: &str) -> Result<Self, KernelError> {
        match value {
            "BROADCASTING" => Ok(Self::Broadcasting),
            "AUTO_ASSIGNED" => Ok(Self::AutoAssigned),
            "MANAGER_FORCE_PENDING" => Ok(Self::ManagerForcePending),
            other => Err(KernelError::validation(format!(
                "unknown dispatch status {other:?}"
            ))),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum DispatchResponseKind {
    Accept,
    Decline,
}

impl DispatchResponseKind {
    #[must_use]
    pub const fn as_db_str(self) -> &'static str {
        match self {
            Self::Accept => "ACCEPT",
            Self::Decline => "DECLINE",
        }
    }

    pub fn from_db_str(value: &str) -> Result<Self, KernelError> {
        match value {
            "ACCEPT" => Ok(Self::Accept),
            "DECLINE" => Ok(Self::Decline),
            other => Err(KernelError::validation(format!(
                "unknown dispatch response {other:?}"
            ))),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum DispatchTargetRole {
    Technician,
    Manager,
}

impl DispatchTargetRole {
    #[must_use]
    pub const fn as_db_str(self) -> &'static str {
        match self {
            Self::Technician => "TECHNICIAN",
            Self::Manager => "MANAGER",
        }
    }

    pub fn from_db_str(value: &str) -> Result<Self, KernelError> {
        match value {
            "TECHNICIAN" => Ok(Self::Technician),
            "MANAGER" => Ok(Self::Manager),
            other => Err(KernelError::validation(format!(
                "unknown dispatch target role {other:?}"
            ))),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DispatchTimerConfig {
    pub accept_window: Duration,
    pub force_assign_alert_after: Duration,
    pub alimtalk_no_ack_after: Duration,
    pub gps_ping_freshness: Duration,
}

impl Default for DispatchTimerConfig {
    fn default() -> Self {
        Self {
            accept_window: Duration::minutes(5),
            force_assign_alert_after: Duration::minutes(10),
            alimtalk_no_ack_after: Duration::minutes(2),
            gps_ping_freshness: Duration::minutes(15),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct GeoPoint {
    pub latitude: f64,
    pub longitude: f64,
}

impl GeoPoint {
    pub fn new(latitude: f64, longitude: f64) -> Result<Self, KernelError> {
        if !latitude.is_finite() || !(-90.0..=90.0).contains(&latitude) {
            return Err(KernelError::validation(format!(
                "latitude must be finite and within -90..=90, got {latitude}"
            )));
        }
        if !longitude.is_finite() || !(-180.0..=180.0).contains(&longitude) {
            return Err(KernelError::validation(format!(
                "longitude must be finite and within -180..=180, got {longitude}"
            )));
        }
        Ok(Self {
            latitude,
            longitude,
        })
    }

    #[must_use]
    pub fn distance_meters_to(self, other: Self) -> i64 {
        // Single kernel home for the haversine primitive (also used by the
        // compliance geofence eval); domain crates can't depend on one another.
        console_kernel_core::haversine_meters(
            self.latitude,
            self.longitude,
            other.latitude,
            other.longitude,
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct P1Dispatch {
    pub id: P1DispatchId,
    pub work_order_id: WorkOrderId,
    pub status: DispatchStatus,
    pub accept_window_started_at: Timestamp,
    pub accept_window_ends_at: Timestamp,
}

impl P1Dispatch {
    pub fn start(
        id: P1DispatchId,
        work_order_id: WorkOrderId,
        now: Timestamp,
        timers: DispatchTimerConfig,
    ) -> Result<Self, KernelError> {
        let accept_window_ends_at = now
            .checked_add(timers.accept_window)
            .ok_or_else(|| KernelError::validation("dispatch accept window overflows time"))?;
        Ok(Self {
            id,
            work_order_id,
            status: DispatchStatus::Broadcasting,
            accept_window_started_at: now,
            accept_window_ends_at,
        })
    }

    pub fn record_response(
        &self,
        response: DispatchResponseKind,
        at: Timestamp,
    ) -> Result<(), KernelError> {
        if self.status != DispatchStatus::Broadcasting {
            return Err(KernelError::conflict(
                "dispatch responses are only allowed while broadcasting",
            ));
        }
        if response == DispatchResponseKind::Accept && at > self.accept_window_ends_at {
            return Err(KernelError::conflict("dispatch accept window has expired"));
        }
        Ok(())
    }

    pub fn resolve_with_accepts(
        &mut self,
        accepted_count: usize,
    ) -> Result<Transition<DispatchStatus>, KernelError> {
        if self.status != DispatchStatus::Broadcasting {
            return Err(illegal(self.status, DispatchStatus::AutoAssigned));
        }
        let to = if accepted_count >= 2 {
            DispatchStatus::AutoAssigned
        } else {
            DispatchStatus::ManagerForcePending
        };
        let from = self.status;
        self.status = to;
        Ok(Transition { from, to })
    }

    pub fn force_assign(&mut self) -> Result<Transition<DispatchStatus>, KernelError> {
        if self.status != DispatchStatus::ManagerForcePending {
            return Err(illegal(self.status, DispatchStatus::AutoAssigned));
        }
        let from = self.status;
        self.status = DispatchStatus::AutoAssigned;
        Ok(Transition {
            from,
            to: self.status,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TechnicianLoad {
    pub p1: i64,
    pub p2: i64,
    pub p3: i64,
    pub other: i64,
}

impl TechnicianLoad {
    #[must_use]
    pub const fn empty() -> Self {
        Self {
            p1: 0,
            p2: 0,
            p3: 0,
            other: 0,
        }
    }

    #[must_use]
    pub const fn priority_weight(self) -> i64 {
        (self.p1 * 10_000) + (self.p2 * 3_000) + (self.p3 * 1_000) + (self.other * 500)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct DispatchCandidate {
    pub mechanic_id: UserId,
    pub latest_location: Option<GeoPoint>,
    pub incident_location: Option<GeoPoint>,
    pub location_recorded_at: Option<Timestamp>,
    pub location_consent_granted: bool,
    pub workload: TechnicianLoad,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CandidateScore {
    pub mechanic_id: UserId,
    pub score_milli: i64,
    pub gps_ranked: bool,
    pub distance_meters: Option<i64>,
    pub workload_weight: i64,
}

impl CandidateScore {
    #[must_use]
    pub fn reason(self) -> &'static str {
        if self.gps_ranked {
            "GPS_DISTANCE_PLUS_PRIORITY_LOAD"
        } else {
            "SCHEDULE_FALLBACK_PRIORITY_LOAD"
        }
    }
}

pub fn score_candidate(
    candidate: DispatchCandidate,
    now: Timestamp,
    freshness: Duration,
) -> CandidateScore {
    let workload_weight = candidate.workload.priority_weight();
    let gps = match (
        candidate.location_consent_granted,
        candidate.incident_location,
        candidate.latest_location,
        candidate.location_recorded_at,
    ) {
        (true, Some(incident), Some(latest), Some(recorded_at))
            if recorded_at >= now - freshness =>
        {
            Some(latest.distance_meters_to(incident))
        }
        _ => None,
    };

    match gps {
        Some(distance_meters) => CandidateScore {
            mechanic_id: candidate.mechanic_id,
            score_milli: (distance_meters * 1_000) + workload_weight,
            gps_ranked: true,
            distance_meters: Some(distance_meters),
            workload_weight,
        },
        None => CandidateScore {
            mechanic_id: candidate.mechanic_id,
            score_milli: 1_000_000_000 + (workload_weight * 1_000),
            gps_ranked: false,
            distance_meters: None,
            workload_weight,
        },
    }
}

fn illegal(from: DispatchStatus, to: DispatchStatus) -> KernelError {
    TransitionError { from, to }.into()
}

impl std::fmt::Display for DispatchStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_db_str())
    }
}

#[cfg(test)]
mod tests {
    use console_kernel_core::WorkOrderId;
    use time::macros::datetime;

    use super::*;

    #[test]
    fn auto_assign_requires_two_acceptances() {
        let mut dispatch = P1Dispatch::start(
            P1DispatchId::new(),
            WorkOrderId::new(),
            datetime!(2026-06-12 09:00 UTC),
            DispatchTimerConfig::default(),
        )
        .unwrap();

        let transition = dispatch.resolve_with_accepts(2).unwrap();

        assert_eq!(transition.from, DispatchStatus::Broadcasting);
        assert_eq!(transition.to, DispatchStatus::AutoAssigned);
    }

    #[test]
    fn no_accepts_escalate_to_manager_force_pending() {
        let mut dispatch = P1Dispatch::start(
            P1DispatchId::new(),
            WorkOrderId::new(),
            datetime!(2026-06-12 09:00 UTC),
            DispatchTimerConfig::default(),
        )
        .unwrap();

        let transition = dispatch.resolve_with_accepts(0).unwrap();

        assert_eq!(transition.to, DispatchStatus::ManagerForcePending);
    }

    #[test]
    fn no_consent_candidate_uses_schedule_fallback_not_gps_rank() {
        let now = datetime!(2026-06-12 09:00 UTC);
        let scored = score_candidate(
            DispatchCandidate {
                mechanic_id: UserId::new(),
                latest_location: Some(GeoPoint::new(37.5665, 126.9780).unwrap()),
                incident_location: Some(GeoPoint::new(37.5651, 126.9895).unwrap()),
                location_recorded_at: Some(now),
                location_consent_granted: false,
                workload: TechnicianLoad::empty(),
            },
            now,
            DispatchTimerConfig::default().gps_ping_freshness,
        );

        assert!(!scored.gps_ranked);
        assert_eq!(scored.distance_meters, None);
        assert_eq!(scored.reason(), "SCHEDULE_FALLBACK_PRIORITY_LOAD");
    }

    #[test]
    fn dispatch_wire_enums_roundtrip_db_str_and_reject_unknown() {
        for status in [
            DispatchStatus::Broadcasting,
            DispatchStatus::AutoAssigned,
            DispatchStatus::ManagerForcePending,
        ] {
            assert_eq!(
                DispatchStatus::from_db_str(status.as_db_str()).unwrap(),
                status
            );
        }
        assert!(DispatchStatus::from_db_str("broadcasting").is_err());
        assert!(DispatchStatus::from_db_str("ASSIGNED").is_err());

        for kind in [DispatchResponseKind::Accept, DispatchResponseKind::Decline] {
            assert_eq!(
                DispatchResponseKind::from_db_str(kind.as_db_str()).unwrap(),
                kind
            );
        }
        assert!(DispatchResponseKind::from_db_str("YES").is_err());

        for role in [DispatchTargetRole::Technician, DispatchTargetRole::Manager] {
            assert_eq!(
                DispatchTargetRole::from_db_str(role.as_db_str()).unwrap(),
                role
            );
        }
        assert!(DispatchTargetRole::from_db_str("tech").is_err());
    }

    #[test]
    fn dispatch_status_display_uses_db_wire_tag() {
        assert_eq!(
            DispatchStatus::ManagerForcePending.to_string(),
            "MANAGER_FORCE_PENDING"
        );
    }
}
