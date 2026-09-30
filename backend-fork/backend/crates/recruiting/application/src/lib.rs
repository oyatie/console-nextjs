//! HTTP-independent recruiting commands.
//!
//! Every command validates and normalizes raw caller input into a shape the
//! adapter can persist without re-checking. Tenant/actor identity is absent by
//! design: the adapter derives it from the authenticated request context.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

use console_kernel_core::KernelError;
use console_recruiting_domain::{
    AmountPeriod, AssessmentScore, EmploymentType, PostingScope, RejectReason,
};
use time::{Date, macros::format_description};

/// A validated posting draft (create and DRAFT-only update share this shape).
#[derive(Debug, Clone)]
pub struct PostingDraft {
    pub role_title: String,
    pub company: String,
    pub worksite: String,
    pub employment_type: EmploymentType,
    pub scope: PostingScope,
    pub headcount: i32,
    pub deadline: Option<Date>,
    pub requirements: Vec<String>,
    pub position_ref: Option<String>,
}

impl PostingDraft {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        role_title: String,
        company: String,
        worksite: String,
        employment_type: &str,
        scope: &str,
        headcount: i32,
        deadline: Option<&str>,
        requirements: Vec<String>,
        position_ref: Option<String>,
    ) -> Result<Self, KernelError> {
        if !(1..=999).contains(&headcount) {
            return Err(KernelError::validation(
                "headcount must be between 1 and 999",
            ));
        }
        if requirements.len() > 20 {
            return Err(KernelError::validation(
                "requirements are limited to 20 items",
            ));
        }
        Ok(Self {
            role_title: required_text(role_title, "role_title", 200)?,
            company: required_text(company, "company", 200)?,
            worksite: required_text(worksite, "worksite", 200)?,
            employment_type: EmploymentType::from_input(employment_type)?,
            scope: PostingScope::from_input(scope)?,
            headcount,
            deadline: deadline.map(parse_date).transpose()?,
            requirements: requirements
                .into_iter()
                .map(|item| required_text(item, "requirements item", 300))
                .collect::<Result<_, _>>()?,
            position_ref: optional_text(position_ref, "position_ref", 200)?,
        })
    }
}

/// A validated recruiter-intake applicant registration.
#[derive(Debug, Clone)]
pub struct ApplicantIntake {
    pub name: String,
    pub profile_lines: Vec<String>,
    pub source_document: Option<String>,
}

impl ApplicantIntake {
    pub fn new(
        name: String,
        profile_lines: Vec<String>,
        source_document: Option<String>,
    ) -> Result<Self, KernelError> {
        if profile_lines.len() > 40 {
            return Err(KernelError::validation(
                "profile_lines are limited to 40 items",
            ));
        }
        Ok(Self {
            name: required_text(name, "name", 200)?,
            profile_lines: profile_lines
                .into_iter()
                .map(|line| required_text(line, "profile_lines item", 500))
                .collect::<Result<_, _>>()?,
            source_document: optional_text(source_document, "source_document", 300)?,
        })
    }
}

/// Validated offer terms shared by extend and adjust.
#[derive(Debug, Clone)]
pub struct OfferTerms {
    /// Canonical `NUMERIC(14,2)` decimal string (KRW).
    pub amount: String,
    pub amount_period: AmountPeriod,
    pub reply_deadline: Date,
}

impl OfferTerms {
    pub fn new(
        amount: &str,
        amount_period: &str,
        reply_deadline: &str,
    ) -> Result<Self, KernelError> {
        Ok(Self {
            amount: canonical_amount(amount)?,
            amount_period: AmountPeriod::from_input(amount_period)?,
            reply_deadline: parse_date(reply_deadline)?,
        })
    }
}

/// Validated enum-reason rejection.
#[derive(Debug, Clone)]
pub struct Rejection {
    pub reason: RejectReason,
    pub note: Option<String>,
}

impl Rejection {
    pub fn new(reason: &str, note: Option<String>) -> Result<Self, KernelError> {
        Ok(Self {
            reason: RejectReason::from_input(reason)?,
            note: optional_text(note, "note", 500)?,
        })
    }
}

pub fn parse_assessment(score: &str) -> Result<AssessmentScore, KernelError> {
    AssessmentScore::from_input(score)
}

pub fn required_text(value: String, field: &str, max: usize) -> Result<String, KernelError> {
    let normalized = value.trim().to_owned();
    if normalized.is_empty() || normalized.chars().count() > max {
        return Err(KernelError::validation(format!(
            "{field} is required and must be {max} characters or fewer"
        )));
    }
    Ok(normalized)
}

pub fn optional_text(
    value: Option<String>,
    field: &str,
    max: usize,
) -> Result<Option<String>, KernelError> {
    value
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
        .map(|value| {
            if value.chars().count() > max {
                Err(KernelError::validation(format!(
                    "{field} must be {max} characters or fewer"
                )))
            } else {
                Ok(value)
            }
        })
        .transpose()
}

pub fn parse_date(value: &str) -> Result<Date, KernelError> {
    Date::parse(value.trim(), format_description!("[year]-[month]-[day]"))
        .map_err(|_| KernelError::validation("date must be formatted YYYY-MM-DD"))
}

/// Produce a fixed-scale decimal accepted by PostgreSQL `NUMERIC(14,2)` without
/// float rounding, exponent notation, leading zeros, or silent truncation
/// (same canonical form as the HR `base_pay` normalization).
pub fn canonical_amount(value: &str) -> Result<String, KernelError> {
    let value = value.trim();
    let (whole, fraction) = value
        .split_once('.')
        .map_or((value, None), |(whole, fraction)| (whole, Some(fraction)));
    let valid_whole = whole == "0"
        || (whole.len() <= 12
            && !whole.starts_with('0')
            && !whole.is_empty()
            && whole.chars().all(|character| character.is_ascii_digit()));
    let valid_fraction = fraction.is_none_or(|fraction| {
        !fraction.is_empty()
            && fraction.len() <= 2
            && fraction.chars().all(|character| character.is_ascii_digit())
    });
    if !valid_whole || !valid_fraction {
        return Err(KernelError::validation(
            "amount must be a canonical NUMERIC(14,2) decimal",
        ));
    }
    let fraction = fraction.unwrap_or("0");
    Ok(format!("{whole}.{fraction:0<2}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_amount_accepts_bounds_and_rejects_non_canonical_forms() {
        assert_eq!(canonical_amount("0").unwrap(), "0.00");
        assert_eq!(canonical_amount("3200000").unwrap(), "3200000.00");
        assert_eq!(
            canonical_amount("999999999999.99").unwrap(),
            "999999999999.99"
        );
        assert_eq!(canonical_amount("185000.5").unwrap(), "185000.50");
        for invalid in ["", "1.001", "1000000000000", "01.00", "1e2", "-1", "1."] {
            assert!(
                canonical_amount(invalid).is_err(),
                "{invalid} must be rejected"
            );
        }
    }

    #[test]
    fn posting_draft_validates_enums_bounds_and_deadline() {
        let draft = PostingDraft::new(
            " 지게차 정비 기술자 ".to_owned(),
            "KNL".to_owned(),
            "창원 성산".to_owned(),
            "REGULAR",
            "EXTERNAL",
            2,
            Some("2026-08-31"),
            vec!["경력 3년".to_owned()],
            None,
        )
        .unwrap();
        assert_eq!(draft.role_title, "지게차 정비 기술자");
        assert_eq!(draft.deadline.unwrap().to_string(), "2026-08-31");
        assert!(
            PostingDraft::new(
                "role".into(),
                "co".into(),
                "site".into(),
                "FREELANCE",
                "EXTERNAL",
                1,
                None,
                vec![],
                None
            )
            .is_err()
        );
        assert!(
            PostingDraft::new(
                "role".into(),
                "co".into(),
                "site".into(),
                "REGULAR",
                "EXTERNAL",
                0,
                None,
                vec![],
                None
            )
            .is_err()
        );
        assert!(
            PostingDraft::new(
                "role".into(),
                "co".into(),
                "site".into(),
                "REGULAR",
                "EXTERNAL",
                1,
                Some("31-08-2026"),
                vec![],
                None
            )
            .is_err()
        );
    }

    #[test]
    fn applicant_intake_trims_and_bounds_profile_lines() {
        let intake = ApplicantIntake::new(
            "김지원".to_owned(),
            vec![" 경력 5년 ".to_owned()],
            Some("resume.pdf".to_owned()),
        )
        .unwrap();
        assert_eq!(intake.profile_lines, vec!["경력 5년".to_owned()]);
        assert!(ApplicantIntake::new(" ".to_owned(), vec![], None).is_err());
        assert!(ApplicantIntake::new("name".to_owned(), vec!["".to_owned()], None).is_err());
    }

    #[test]
    fn offer_terms_and_rejection_parse_their_enums() {
        let terms = OfferTerms::new("3500000", "MONTHLY", "2026-08-15").unwrap();
        assert_eq!(terms.amount, "3500000.00");
        assert!(OfferTerms::new("3500000", "WEEKLY", "2026-08-15").is_err());
        let rejection = Rejection::new("ROLE_MISMATCH", Some("  ".to_owned())).unwrap();
        assert!(rejection.note.is_none());
        assert!(Rejection::new("NOT_A_REASON", None).is_err());
    }
}
