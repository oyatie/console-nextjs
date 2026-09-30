//! InboxDoc domain — the statutory-notice vault (개인 수신함).
//!
//! Pure value objects and validation only. Persistence, audit, passkey step-up,
//! and REST live in outer layers. A document is either a `payslip` (frictionless
//! self-view, never audited, never receipt-gated) or a `legal_notice` (근로계약/
//! 취업규칙/연차촉진/노무수령거부) whose body is LOCKED until the recipient confirms
//! receipt with a fresh passkey — that confirmation is the legal receipt
//! evidence (열람 = 법적 수령).
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

use console_kernel_core::KernelError;
use serde::{Deserialize, Serialize};

const TITLE_MAX: usize = 300;
const NOTICE_TYPE_MAX: usize = 64;
const LEGAL_BASIS_MAX: usize = 120;
const SOURCE_KIND_MAX: usize = 64;
const SOURCE_ID_MAX: usize = 200;

/// The two document classes in the vault. Whether receipt confirmation is
/// required is fully determined by the kind — there is no separate `legal`
/// flag to drift.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InboxDocKind {
    /// The recipient's own pay statement. Frictionless self-view; never audited,
    /// never receipt-gated.
    Payslip,
    /// A statutory notice requiring passkey-gated receipt confirmation.
    LegalNotice,
}

impl InboxDocKind {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Payslip => "payslip",
            Self::LegalNotice => "legal_notice",
        }
    }

    pub fn parse(value: &str) -> Result<Self, KernelError> {
        match value {
            "payslip" => Ok(Self::Payslip),
            "legal_notice" => Ok(Self::LegalNotice),
            other => Err(KernelError::validation(format!(
                "unknown inbox document kind: {other}"
            ))),
        }
    }

    /// A legal notice's body is receipt-gated (locked until confirmed); a
    /// payslip is always freely viewable by its recipient.
    #[must_use]
    pub fn requires_receipt(self) -> bool {
        matches!(self, Self::LegalNotice)
    }
}

/// Validate a required, bounded free-form field, returning the trimmed value.
fn bounded(value: &str, field: &str, max: usize) -> Result<String, KernelError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(KernelError::validation(format!("{field} is required")));
    }
    if trimmed.chars().count() > max {
        return Err(KernelError::validation(format!(
            "{field} must be at most {max} characters"
        )));
    }
    Ok(trimmed.to_owned())
}

/// Validate an optional, bounded free-form field. `Some("")`/whitespace → None.
fn bounded_opt(
    value: Option<&str>,
    field: &str,
    max: usize,
) -> Result<Option<String>, KernelError> {
    match value.map(str::trim).filter(|v| !v.is_empty()) {
        Some(v) => Ok(Some(bounded(v, field, max)?)),
        None => Ok(None),
    }
}

/// A validated new inbox document, ready for the write port.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewInboxDoc {
    pub kind: InboxDocKind,
    pub title: String,
    pub notice_type: Option<String>,
    pub legal_basis: Option<String>,
    pub source_kind: Option<String>,
    pub source_id: Option<String>,
    pub payload: serde_json::Value,
}

impl NewInboxDoc {
    /// Build + validate. Enforces the invariants the JSONB/CHECK columns cannot
    /// fully express: a legal notice must carry a `notice_type`; a payslip must
    /// not; and `payload` must be a JSON object.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        kind: InboxDocKind,
        title: &str,
        notice_type: Option<&str>,
        legal_basis: Option<&str>,
        source_kind: Option<&str>,
        source_id: Option<&str>,
        payload: serde_json::Value,
    ) -> Result<Self, KernelError> {
        let title = bounded(title, "inbox document title", TITLE_MAX)?;
        let notice_type = bounded_opt(notice_type, "notice type", NOTICE_TYPE_MAX)?;
        let legal_basis = bounded_opt(legal_basis, "legal basis", LEGAL_BASIS_MAX)?;
        let source_kind = bounded_opt(source_kind, "source kind", SOURCE_KIND_MAX)?;
        let source_id = bounded_opt(source_id, "source id", SOURCE_ID_MAX)?;

        match kind {
            InboxDocKind::LegalNotice if notice_type.is_none() => {
                return Err(KernelError::validation(
                    "a legal notice requires a notice_type (근로계약/취업규칙/연차촉진/노무수령거부)",
                ));
            }
            InboxDocKind::Payslip if notice_type.is_some() => {
                return Err(KernelError::validation(
                    "a payslip must not carry a notice_type",
                ));
            }
            _ => {}
        }
        if (source_kind.is_none()) != (source_id.is_none()) {
            return Err(KernelError::validation(
                "source_kind and source_id must be provided together",
            ));
        }
        if !payload.is_object() {
            return Err(KernelError::validation(
                "inbox document payload must be a JSON object",
            ));
        }

        Ok(Self {
            kind,
            title,
            notice_type,
            legal_basis,
            source_kind,
            source_id,
            payload,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn kind_roundtrip_and_receipt_semantics() {
        assert_eq!(
            InboxDocKind::parse("payslip").unwrap(),
            InboxDocKind::Payslip
        );
        assert_eq!(
            InboxDocKind::parse("legal_notice").unwrap(),
            InboxDocKind::LegalNotice
        );
        assert!(InboxDocKind::parse("other").is_err());
        assert!(!InboxDocKind::Payslip.requires_receipt());
        assert!(InboxDocKind::LegalNotice.requires_receipt());
    }

    #[test]
    fn legal_notice_requires_notice_type() {
        assert!(
            NewInboxDoc::new(
                InboxDocKind::LegalNotice,
                "연차 사용 촉진 통지",
                None,
                Some("근로기준법 §61"),
                None,
                None,
                json!({}),
            )
            .is_err()
        );
        assert!(
            NewInboxDoc::new(
                InboxDocKind::LegalNotice,
                "연차 사용 촉진 통지",
                Some("연차촉진"),
                Some("근로기준법 §61"),
                Some("workflow_run"),
                Some("AP-3111"),
                json!({ "paragraphs": ["..."] }),
            )
            .is_ok()
        );
    }

    #[test]
    fn payslip_rejects_notice_type_and_requires_object_payload() {
        assert!(
            NewInboxDoc::new(
                InboxDocKind::Payslip,
                "6월 급여명세",
                Some("연차촉진"),
                None,
                None,
                None,
                json!({}),
            )
            .is_err()
        );
        assert!(
            NewInboxDoc::new(
                InboxDocKind::Payslip,
                "6월 급여명세",
                None,
                None,
                None,
                None,
                json!([1, 2, 3]),
            )
            .is_err(),
            "non-object payload must be rejected"
        );
    }

    #[test]
    fn source_ref_must_be_paired() {
        assert!(
            NewInboxDoc::new(
                InboxDocKind::Payslip,
                "6월 급여명세",
                None,
                None,
                Some("payroll_run"),
                None,
                json!({}),
            )
            .is_err()
        );
    }

    #[test]
    fn blank_and_overlong_title_rejected() {
        assert!(
            NewInboxDoc::new(
                InboxDocKind::Payslip,
                "   ",
                None,
                None,
                None,
                None,
                json!({}),
            )
            .is_err()
        );
        assert!(
            NewInboxDoc::new(
                InboxDocKind::Payslip,
                &"x".repeat(TITLE_MAX + 1),
                None,
                None,
                None,
                None,
                json!({}),
            )
            .is_err()
        );
    }
}
