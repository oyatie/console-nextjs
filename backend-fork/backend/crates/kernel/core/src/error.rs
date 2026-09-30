//! Shared error taxonomy. Domain crates define their own error enums and map
//! into these kinds at the application boundary; REST maps kinds to HTTP
//! status codes in exactly one place.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorKind {
    /// Input failed validation (→ 422).
    Validation,
    /// Entity does not exist or is outside the caller's branch scope (→ 404).
    NotFound,
    /// Authenticated but not allowed (→ 403).
    Forbidden,
    /// State conflict, e.g. optimistic-lock or duplicate key (→ 409).
    Conflict,
    /// Illegal state-machine transition (→ 409 with transition detail).
    InvalidTransition,
    /// Unexpected internal failure (→ 500; details logged, not returned).
    Internal,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error, serde::Serialize, serde::Deserialize)]
#[error("{kind:?}: {message}")]
pub struct KernelError {
    pub kind: ErrorKind,
    pub message: String,
}

impl KernelError {
    #[must_use]
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    #[must_use]
    pub fn validation(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Validation, message)
    }

    #[must_use]
    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::NotFound, message)
    }

    #[must_use]
    pub fn forbidden(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Forbidden, message)
    }

    #[must_use]
    pub fn conflict(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Conflict, message)
    }

    #[must_use]
    pub fn invalid_transition(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::InvalidTransition, message)
    }

    #[must_use]
    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Internal, message)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_includes_kind_and_message() {
        let err = KernelError::forbidden("cross-branch access denied");
        let text = err.to_string();
        assert!(text.contains("Forbidden"));
        assert!(text.contains("cross-branch access denied"));
    }

    #[test]
    fn kind_serializes_snake_case() {
        let json = serde_json::to_string(&ErrorKind::InvalidTransition).unwrap();
        assert_eq!(json, "\"invalid_transition\"");
    }
}
