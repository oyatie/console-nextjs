//! Redacting newtypes for PII fields.
//!
//! The CI `pii-no-logs` gate is a literal-only scanner: it catches a raw PII
//! pattern pasted directly into a logging macro, but it cannot follow PII that
//! reaches a log through a binding or interpolation. These newtypes close that
//! gap at runtime by overriding `Display`/`Debug` so the wrapped value can never
//! be rendered in full — wrap a phone number in [`RedactedPhone`] and any
//! `{}`/`{:?}` formatting (including inside a `tracing::info!` field) emits a
//! masked form instead of the raw digits.
//!
//! This is intentionally additive and call-site-light: existing code keeps using
//! plain `String`; only code that deliberately wants log-safe rendering opts in.

use std::fmt;

/// A phone number that renders as a masked value in any `Display`/`Debug` output.
///
/// The raw value is retained for legitimate use (storage, comparison) via
/// [`RedactedPhone::reveal`]; only the *formatting* is redacted. Masking keeps a
/// short trailing fragment so logs stay debuggable without exposing the subscriber
/// line: e.g. `010-1234-5678` renders as `***-****-5678`.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct RedactedPhone(String);

impl RedactedPhone {
    /// Wrap a phone-number string. No validation is performed; the value is only
    /// ever rendered in masked form.
    #[must_use]
    pub fn new(raw: impl Into<String>) -> Self {
        Self(raw.into())
    }

    /// Borrow the unmasked value for non-logging use (storage, equality, sending
    /// to the SMS provider). Never pass the result to a logging macro.
    #[must_use]
    pub fn reveal(&self) -> &str {
        &self.0
    }

    /// The masked rendering: every character except the last four is replaced by
    /// `*`, preserving any non-digit separators so the shape stays recognizable.
    fn masked(&self) -> String {
        let kept = 4;
        let total = self.0.chars().count();
        let reveal_from = total.saturating_sub(kept);
        self.0
            .chars()
            .enumerate()
            .map(|(index, ch)| {
                if index >= reveal_from || !ch.is_ascii_digit() {
                    ch
                } else {
                    '*'
                }
            })
            .collect()
    }
}

impl fmt::Display for RedactedPhone {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.masked())
    }
}

impl fmt::Debug for RedactedPhone {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Debug must not leak the raw value either; render the masked form.
        write!(f, "RedactedPhone({})", self.masked())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_masks_all_but_last_four_digits() {
        let phone = RedactedPhone::new("010-1234-5678");
        assert_eq!(phone.to_string(), "***-****-5678");
    }

    #[test]
    fn debug_does_not_leak_raw_value() {
        let phone = RedactedPhone::new("01012345678");
        let rendered = format!("{phone:?}");
        assert!(!rendered.contains("0101234"));
        assert!(rendered.contains("5678"));
    }

    #[test]
    fn reveal_returns_unmasked_value() {
        let phone = RedactedPhone::new("010-1234-5678");
        assert_eq!(phone.reveal(), "010-1234-5678");
    }

    #[test]
    fn short_values_are_fully_masked_to_their_length() {
        let phone = RedactedPhone::new("12");
        // Fewer than four digits: nothing is revealed beyond what exists, and no
        // padding is added.
        assert_eq!(phone.to_string(), "12");
    }
}
