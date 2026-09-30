//! Console-owned fail-closed AIP draft seam. Not a tenant Intelligence product, not a clone of the Intelligence repository, not a Palantir client.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

use std::future::Future;
use std::pin::Pin;

use console_kernel_core::{BranchScope, OrgId, UserId};
use console_ontology_canonical_domain::DispatchTarget;
use serde_json::Value;
use uuid::Uuid;

/// Company-scoped actor. Projection of `console_platform_authz::Principal`
/// at a layer that is allowed to exist here. Never a Group id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DraftActor {
    pub user_id: UserId,
    pub org_id: OrgId,
    pub branch_scope: BranchScope,
}

/// Propose a reviewable draft for an EXISTING roster verb.
/// `DispatchTarget` is the thirteen-member enum; `intelligence.draft` cannot be constructed.
#[derive(Debug, Clone, PartialEq)]
pub struct IntelligenceDraftQuery {
    pub dispatch_target: DispatchTarget,
    pub subject_id: Option<Uuid>,
    pub params: Value,
    pub reason: Option<String>,
}

/// Reviewable proposal. Not a receipt. Not a write. No files, no Palantir RIDs.
#[derive(Debug, Clone, PartialEq)]
pub struct IntelligenceDraft {
    pub dispatch_target: DispatchTarget,
    pub proposed_params: Value,
    pub summary: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IntelligenceDraftError {
    /// Production refusal this slice. Also the typed ontology outcome for an
    /// unwired projected target (`ActionError::NotWiredYet`).
    NotWiredYet,
    InvalidRequest(String),
}

impl std::fmt::Display for IntelligenceDraftError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotWiredYet => write!(f, "not_wired_yet"),
            Self::InvalidRequest(message) => {
                write!(f, "invalid intelligence draft request: {message}")
            }
        }
    }
}

impl std::error::Error for IntelligenceDraftError {}

pub type IntelligenceDraftResult<T> = Result<T, IntelligenceDraftError>;
pub type IntelligenceDraftFuture<'a, T> =
    Pin<Box<dyn Future<Output = IntelligenceDraftResult<T>> + Send + 'a>>;

/// Fail-closed AIP-shaped draft seam. Dyn-compatible for a later real adapter
/// at the composition root without changing use-cases. No HTTP, no Palantir.
pub trait IntelligenceDraftPort: Send + Sync {
    fn draft<'a>(
        &'a self,
        actor: &'a DraftActor,
        query: IntelligenceDraftQuery,
    ) -> IntelligenceDraftFuture<'a, IntelligenceDraft>;
}

/// The only implementation in this slice. Not a mock: it never invents text.
/// Landable tests-only SHA includes this impl so `--lib` CI is not compile-red.
pub struct NotWiredIntelligenceDraftPort;

impl IntelligenceDraftPort for NotWiredIntelligenceDraftPort {
    fn draft<'a>(
        &'a self,
        _actor: &'a DraftActor,
        _query: IntelligenceDraftQuery,
    ) -> IntelligenceDraftFuture<'a, IntelligenceDraft> {
        Box::pin(async { Err(IntelligenceDraftError::NotWiredYet) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::future::Future;
    use std::str::FromStr;
    use std::task::{Context, Poll, Waker};

    fn accepts(_: &dyn IntelligenceDraftPort) {}

    fn sample_actor() -> DraftActor {
        DraftActor {
            user_id: UserId::new(),
            org_id: OrgId::new(),
            branch_scope: BranchScope::All,
        }
    }

    fn sample_query() -> IntelligenceDraftQuery {
        IntelligenceDraftQuery {
            dispatch_target: DispatchTarget::PayrollCreateRun,
            subject_id: None,
            params: Value::Object(Default::default()),
            reason: None,
        }
    }

    fn toml_table_keys(manifest: &str, table: &str) -> Option<Vec<String>> {
        let header = format!("[{table}]");
        let mut lines = manifest.lines();
        lines.find(|line| line.trim() == header)?;
        let mut keys = Vec::new();
        for line in lines {
            let trimmed = line.trim();
            if trimmed.starts_with('[') {
                break;
            }
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }
            let key = trimmed.split('=').next()?.trim();
            if !key.is_empty() {
                keys.push(key.to_owned());
            }
        }
        Some(keys)
    }

    #[test]
    fn intelligence_draft_port_is_object_safe() {
        let _: fn(&dyn IntelligenceDraftPort) = accepts;
    }

    #[test]
    fn not_wired_port_returns_not_wired_yet() {
        let actor = sample_actor();
        let query = sample_query();
        let mut fut = NotWiredIntelligenceDraftPort.draft(&actor, query);
        let waker = Waker::noop();
        let mut cx = Context::from_waker(waker);
        let poll = Future::poll(fut.as_mut(), &mut cx);
        assert!(
            matches!(poll, Poll::Ready(Err(IntelligenceDraftError::NotWiredYet))),
            "expected Ready(Err(NotWiredYet)), got {poll:?}"
        );
        assert!(
            !matches!(poll, Poll::Pending),
            "NotWired future must be immediately ready"
        );
    }

    #[test]
    fn draft_actor_has_no_group_id() {
        let actor = sample_actor();
        let DraftActor {
            user_id,
            org_id,
            branch_scope,
        } = actor;
        let _ = (user_id, org_id, branch_scope);
    }

    #[test]
    fn draft_query_dispatch_target_is_roster_member() {
        let query = IntelligenceDraftQuery {
            dispatch_target: DispatchTarget::PayrollCreateRun,
            subject_id: None,
            params: Value::Object(Default::default()),
            reason: None,
        };
        assert_eq!(query.dispatch_target, DispatchTarget::PayrollCreateRun);
        assert!(DispatchTarget::from_str("intelligence.draft").is_err());
    }

    #[test]
    fn draft_dto_has_no_file_or_rid_fields() {
        let draft = IntelligenceDraft {
            dispatch_target: DispatchTarget::PayrollCreateRun,
            proposed_params: Value::Object(Default::default()),
            summary: "none".to_owned(),
        };
        let IntelligenceDraft {
            dispatch_target,
            proposed_params,
            summary,
        } = draft;
        let _ = (dispatch_target, proposed_params, summary);
    }

    #[test]
    fn not_wired_error_display_is_stable_token() {
        let err = IntelligenceDraftError::NotWiredYet;
        assert_eq!(err.to_string(), "not_wired_yet");
        fn as_error(_: &dyn std::error::Error) {}
        as_error(&err);
    }

    #[test]
    fn crate_manifest_forbids_io_clients() {
        const MANIFEST: &str = include_str!("../Cargo.toml");
        let keys = toml_table_keys(MANIFEST, "dependencies").expect("dependencies table");
        assert_eq!(
            keys,
            vec![
                "console-kernel-core",
                "console-ontology-canonical-domain",
                "serde_json",
                "uuid",
            ]
        );
        assert!(
            toml_table_keys(MANIFEST, "dev-dependencies").is_none(),
            "this crate must not declare [dev-dependencies]"
        );
        let forbidden = [
            "sqlx", "axum", "tokio", "reqwest", "hyper", "futures", "pollster",
        ];
        for key in &keys {
            let lower = key.to_ascii_lowercase();
            assert!(
                !forbidden.iter().any(|name| lower == *name) && !lower.contains("palantir"),
                "forbidden dependency key {key}"
            );
        }
    }
}
