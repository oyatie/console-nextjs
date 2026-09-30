//! Pure inspection domain.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

use console_kernel_core::KernelError;
use serde::{Deserialize, Serialize};

macro_rules! inspection_enum {
    (
        pub enum $name:ident {
            $($variant:ident => ($wire:literal, $label_ko:literal)),+ $(,)?
        }
    ) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(rename_all = "SCREAMING_SNAKE_CASE")]
        pub enum $name {
            $($variant,)+
        }

        impl $name {
            #[must_use]
            pub const fn as_db_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $wire,)+
                }
            }

            #[must_use]
            pub const fn label_ko(self) -> &'static str {
                match self {
                    $(Self::$variant => $label_ko,)+
                }
            }

            pub fn from_db_str(value: &str) -> Result<Self, KernelError> {
                match value {
                    $($wire => Ok(Self::$variant),)+
                    other => Err(KernelError::validation(format!(
                        "unknown {} value {other:?}",
                        stringify!($name)
                    ))),
                }
            }
        }
    };
}

inspection_enum! {
    pub enum InspectionCycle {
        Daily => ("DAILY", "일간"),
        Weekly => ("WEEKLY", "주간"),
        Monthly => ("MONTHLY", "월간"),
        Quarterly => ("QUARTERLY", "분기"),
        Yearly => ("YEARLY", "연간"),
        Custom => ("CUSTOM", "사용자 지정"),
    }
}

inspection_enum! {
    pub enum InspectionScheduleStatus {
        Scheduled => ("SCHEDULED", "예정"),
        Completed => ("COMPLETED", "완료"),
        Cancelled => ("CANCELLED", "취소"),
    }
}

inspection_enum! {
    pub enum InspectionRoundOutcome {
        Completed => ("COMPLETED", "완료"),
        FollowUpRequired => ("FOLLOW_UP_REQUIRED", "후속 조치 필요"),
    }
}

pub fn validate_interval_days(interval_days: i32) -> Result<(), KernelError> {
    if interval_days <= 0 {
        return Err(KernelError::validation(
            "inspection interval_days must be positive",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inspection_enums_roundtrip_db_str_and_reject_unknown() {
        for cycle in [
            InspectionCycle::Daily,
            InspectionCycle::Weekly,
            InspectionCycle::Monthly,
            InspectionCycle::Quarterly,
            InspectionCycle::Yearly,
            InspectionCycle::Custom,
        ] {
            assert_eq!(
                InspectionCycle::from_db_str(cycle.as_db_str()).unwrap(),
                cycle
            );
            assert!(!cycle.label_ko().is_empty());
        }
        assert!(InspectionCycle::from_db_str("HOURLY").is_err());

        for status in [
            InspectionScheduleStatus::Scheduled,
            InspectionScheduleStatus::Completed,
            InspectionScheduleStatus::Cancelled,
        ] {
            assert_eq!(
                InspectionScheduleStatus::from_db_str(status.as_db_str()).unwrap(),
                status
            );
        }
        assert!(InspectionScheduleStatus::from_db_str("pending").is_err());

        for outcome in [
            InspectionRoundOutcome::Completed,
            InspectionRoundOutcome::FollowUpRequired,
        ] {
            assert_eq!(
                InspectionRoundOutcome::from_db_str(outcome.as_db_str()).unwrap(),
                outcome
            );
        }
        assert!(InspectionRoundOutcome::from_db_str("FAILED").is_err());
    }

    #[test]
    fn db_wire_tags_are_stable_screaming_snake() {
        assert_eq!(InspectionCycle::Daily.as_db_str(), "DAILY");
        assert_eq!(InspectionCycle::Custom.as_db_str(), "CUSTOM");
        assert_eq!(InspectionScheduleStatus::Scheduled.as_db_str(), "SCHEDULED");
        assert_eq!(
            InspectionRoundOutcome::FollowUpRequired.as_db_str(),
            "FOLLOW_UP_REQUIRED"
        );
    }

    #[test]
    fn validate_interval_days_is_fail_closed_for_non_positive() {
        assert!(validate_interval_days(1).is_ok());
        assert!(validate_interval_days(30).is_ok());
        assert!(validate_interval_days(0).is_err());
        assert!(validate_interval_days(-3).is_err());
    }
}
