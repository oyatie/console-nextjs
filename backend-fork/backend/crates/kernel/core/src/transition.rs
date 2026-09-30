//! State-machine transition primitives shared by the WorkOrder FSM (16
//! inherited states) and the separate P1 dispatch accept-window FSM. Domains
//! own their transition tables; the kernel owns the result/error shapes so
//! audit and REST handle them uniformly.

use crate::error::{ErrorKind, KernelError};

/// A successfully validated transition, ready to be persisted + audited.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Transition<S> {
    pub from: S,
    pub to: S,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("illegal transition {from:?} → {to:?}")]
pub struct TransitionError<S: std::fmt::Debug> {
    pub from: S,
    pub to: S,
}

impl<S: std::fmt::Debug> From<TransitionError<S>> for KernelError {
    fn from(err: TransitionError<S>) -> Self {
        Self::new(ErrorKind::InvalidTransition, err.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Demo {
        Received,
        Assigned,
    }

    #[test]
    fn transition_error_maps_to_kernel_invalid_transition() {
        let err = TransitionError {
            from: Demo::Assigned,
            to: Demo::Received,
        };
        let kernel: KernelError = err.into();
        assert_eq!(kernel.kind, ErrorKind::InvalidTransition);
        assert!(kernel.message.contains("Assigned"));
        assert!(kernel.message.contains("Received"));
    }
}
