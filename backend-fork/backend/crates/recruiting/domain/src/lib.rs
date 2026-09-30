//! Recruiting-pipeline finite-state vocabulary.
//!
//! Three machines: posting (DRAFT→PUBLISHED→CLOSED), applicant
//! (APPLIED→SCREENING→INTERVIEW→OFFER→HIRED with reject/reinstate archive),
//! offer (EXTENDED→SUPERSEDED|WITHDRAWN|ACCEPTED|DECLINED). HIRED is set
//! exclusively by the hire handshake through the owning HR use-case; `advance`
//! can never reach it, and INTERVIEW→OFFER happens only via an offer extension.
use console_kernel_core::KernelError;
use serde::{Deserialize, Serialize};

/// Publish preflight check keys (server-evaluated, fail-closed).
pub const PREFLIGHT_ROLE_DEFINED: &str = "role_defined";
pub const PREFLIGHT_QUOTA_DEFINED: &str = "quota_defined";
pub const PREFLIGHT_NO_DUPLICATE_OPEN: &str = "no_duplicate_open";
pub const PREFLIGHT_EXPOSURE_ATTESTED: &str = "exposure_attested";

macro_rules! db_enum {
    ($(#[$meta:meta])* $name:ident { $($variant:ident => $db:literal),+ $(,)? } $unknown:literal) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
        #[serde(rename_all = "SCREAMING_SNAKE_CASE")]
        pub enum $name { $($variant),+ }
        impl $name {
            #[must_use]
            pub const fn as_db(self) -> &'static str {
                match self { $(Self::$variant => $db),+ }
            }
            /// Decode a database value. Rows are CHECK-constrained, so failure
            /// is an internal anomaly, never caller input.
            pub fn from_db(value: &str) -> Result<Self, KernelError> {
                match value {
                    $($db => Ok(Self::$variant),)+
                    _ => Err(KernelError::internal(concat!("stored ", $unknown, " is outside the vocabulary"))),
                }
            }
            /// Parse caller input (422 on unknown values).
            pub fn from_input(value: &str) -> Result<Self, KernelError> {
                match value {
                    $($db => Ok(Self::$variant),)+
                    _ => Err(KernelError::validation(concat!($unknown, " must be one of ", $($db, " "),+))),
                }
            }
        }
    };
}

db_enum!(PostingStatus { Draft => "DRAFT", Published => "PUBLISHED", Closed => "CLOSED" } "posting status");
db_enum!(PostingScope { Internal => "INTERNAL", External => "EXTERNAL" } "posting scope");
db_enum!(
    EmploymentType {
        Regular => "REGULAR",
        ResidentShift => "RESIDENT_SHIFT",
        PartTime => "PART_TIME",
        PoolDaily => "POOL_DAILY",
    } "employment_type"
);
db_enum!(
    ApplicantStage {
        Applied => "APPLIED",
        Screening => "SCREENING",
        Interview => "INTERVIEW",
        Offer => "OFFER",
        Hired => "HIRED",
    } "applicant stage"
);
db_enum!(
    RejectReason {
        CareerShortfall => "CAREER_SHORTFALL",
        RoleMismatch => "ROLE_MISMATCH",
        CompMismatch => "COMP_MISMATCH",
        AcceptedElsewhere => "ACCEPTED_ELSEWHERE",
        Other => "OTHER",
    } "reject reason"
);
db_enum!(AssessmentScore { Suitable => "SUITABLE", Neutral => "NEUTRAL", Unsuitable => "UNSUITABLE" } "assessment score");
db_enum!(
    OfferStatus {
        Extended => "EXTENDED",
        Superseded => "SUPERSEDED",
        Withdrawn => "WITHDRAWN",
        Accepted => "ACCEPTED",
        Declined => "DECLINED",
    } "offer status"
);
db_enum!(AmountPeriod { Monthly => "MONTHLY", Daily => "DAILY" } "amount_period");

impl PostingStatus {
    pub fn can_transition_to(self, next: Self) -> Result<(), KernelError> {
        match (self, next) {
            (Self::Draft, Self::Published) | (Self::Published, Self::Closed) => Ok(()),
            _ => Err(KernelError::conflict("illegal posting state transition")),
        }
    }
}

impl EmploymentType {
    /// The HR employment-profile vocabulary the hire handshake maps into.
    /// `POOL_DAILY` maps to nothing: pool postings never create an employee
    /// (재직 명부 비합산) until the workforce-pool registry exists.
    #[must_use]
    pub const fn hr_employment_type(self) -> Option<&'static str> {
        match self {
            // A resident-shift posting hires a regular employee stationed at
            // the customer site; the posting distinguishes work pattern, not
            // contract form.
            Self::Regular | Self::ResidentShift => Some("REGULAR"),
            Self::PartTime => Some("PART_TIME"),
            Self::PoolDaily => None,
        }
    }
}

impl ApplicantStage {
    /// The single-step `advance` target. INTERVIEW→OFFER is offer-only and
    /// OFFER→HIRED is hire-only — both fail closed here.
    pub fn advance_target(self) -> Result<Self, KernelError> {
        match self {
            Self::Applied => Ok(Self::Screening),
            Self::Screening => Ok(Self::Interview),
            Self::Interview => Err(KernelError::validation(
                "interview stage advances only through an offer extension",
            )),
            Self::Offer => Err(KernelError::validation(
                "offer stage resolves only through the hire handshake",
            )),
            Self::Hired => Err(KernelError::validation("hired stage is terminal")),
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn posting_transitions_are_publish_then_close_only() {
        PostingStatus::Draft
            .can_transition_to(PostingStatus::Published)
            .unwrap();
        PostingStatus::Published
            .can_transition_to(PostingStatus::Closed)
            .unwrap();
        assert!(
            PostingStatus::Draft
                .can_transition_to(PostingStatus::Closed)
                .is_err()
        );
        assert!(
            PostingStatus::Closed
                .can_transition_to(PostingStatus::Published)
                .is_err()
        );
        assert!(
            PostingStatus::Published
                .can_transition_to(PostingStatus::Draft)
                .is_err()
        );
    }

    #[test]
    fn advance_walks_single_steps_and_fails_closed_at_offer_and_hire_edges() {
        assert_eq!(
            ApplicantStage::Applied.advance_target().unwrap(),
            ApplicantStage::Screening
        );
        assert_eq!(
            ApplicantStage::Screening.advance_target().unwrap(),
            ApplicantStage::Interview
        );
        for terminal in [
            ApplicantStage::Interview,
            ApplicantStage::Offer,
            ApplicantStage::Hired,
        ] {
            assert!(terminal.advance_target().is_err());
        }
    }

    #[test]
    fn pool_daily_never_maps_into_the_hr_employment_vocabulary() {
        assert_eq!(
            EmploymentType::Regular.hr_employment_type(),
            Some("REGULAR")
        );
        assert_eq!(
            EmploymentType::ResidentShift.hr_employment_type(),
            Some("REGULAR")
        );
        assert_eq!(
            EmploymentType::PartTime.hr_employment_type(),
            Some("PART_TIME")
        );
        assert_eq!(EmploymentType::PoolDaily.hr_employment_type(), None);
    }

    #[test]
    fn database_values_round_trip_and_unknown_values_fail_by_origin() {
        for stage in [
            ApplicantStage::Applied,
            ApplicantStage::Screening,
            ApplicantStage::Interview,
            ApplicantStage::Offer,
            ApplicantStage::Hired,
        ] {
            assert_eq!(ApplicantStage::from_db(stage.as_db()).unwrap(), stage);
        }
        for reason in [
            RejectReason::CareerShortfall,
            RejectReason::RoleMismatch,
            RejectReason::CompMismatch,
            RejectReason::AcceptedElsewhere,
            RejectReason::Other,
        ] {
            assert_eq!(RejectReason::from_db(reason.as_db()).unwrap(), reason);
        }
        for status in [
            OfferStatus::Extended,
            OfferStatus::Superseded,
            OfferStatus::Withdrawn,
            OfferStatus::Accepted,
            OfferStatus::Declined,
        ] {
            assert_eq!(OfferStatus::from_db(status.as_db()).unwrap(), status);
        }
        assert_eq!(
            ApplicantStage::from_db("UNKNOWN").unwrap_err().kind,
            console_kernel_core::ErrorKind::Internal
        );
        assert_eq!(
            ApplicantStage::from_input("UNKNOWN").unwrap_err().kind,
            console_kernel_core::ErrorKind::Validation
        );
    }

    #[test]
    fn posting_vocab_roundtrips_and_input_vs_internal_errors() {
        for status in [
            PostingStatus::Draft,
            PostingStatus::Published,
            PostingStatus::Closed,
        ] {
            assert_eq!(PostingStatus::from_db(status.as_db()).unwrap(), status);
            assert_eq!(PostingStatus::from_input(status.as_db()).unwrap(), status);
        }
        assert_eq!(
            PostingStatus::from_db("OPEN").unwrap_err().kind,
            console_kernel_core::ErrorKind::Internal
        );
        assert_eq!(
            PostingStatus::from_input("OPEN").unwrap_err().kind,
            console_kernel_core::ErrorKind::Validation
        );

        for scope in [PostingScope::Internal, PostingScope::External] {
            assert_eq!(PostingScope::from_db(scope.as_db()).unwrap(), scope);
            assert_eq!(PostingScope::from_input(scope.as_db()).unwrap(), scope);
        }
        assert!(PostingScope::from_input("BOTH").is_err());

        for et in [
            EmploymentType::Regular,
            EmploymentType::ResidentShift,
            EmploymentType::PartTime,
            EmploymentType::PoolDaily,
        ] {
            assert_eq!(EmploymentType::from_db(et.as_db()).unwrap(), et);
            assert_eq!(EmploymentType::from_input(et.as_db()).unwrap(), et);
        }
        assert!(EmploymentType::from_input("CONTRACTOR").is_err());

        for score in [
            AssessmentScore::Suitable,
            AssessmentScore::Neutral,
            AssessmentScore::Unsuitable,
        ] {
            assert_eq!(AssessmentScore::from_db(score.as_db()).unwrap(), score);
        }
    }
}
