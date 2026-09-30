//! This face's slice of the published OpenAPI contract.
//!
//! Bodies live as YAML under `openapi/` and are pulled in via `include_str!`.
//! `console_contracts` re-indents them; composition rejects duplicate keys.

use console_contracts::{Fragment, NamedYaml, Operation, PathItem};

/// This face's contribution to the composed OpenAPI document.
pub const OPENAPI_FRAGMENT: Fragment = Fragment {
    source: "console-governance-rest",
    paths: PATHS,
    schemas: SCHEMAS,
    parameters: &[],
    responses: &[],
    security_schemes: &[],
    external_schemas: EXTERNAL_SCHEMAS,
};

const EXTERNAL_SCHEMAS: &[&str] = &["ErrorBody", "Timestamp", "Uuid"];

const PATHS: &[PathItem] = &[
    PathItem {
        path: "/api/audit",
        operations: &[Operation {
            method: "get",
            body: include_str!("../openapi/paths/api__audit.get.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/audit/attestation",
        operations: &[Operation {
            method: "get",
            body: include_str!("../openapi/paths/api__v1__audit__attestation.get.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/governance/approvals",
        operations: &[Operation {
            method: "post",
            body: include_str!("../openapi/paths/api__v1__governance__approvals.post.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/governance/approvals/decide",
        operations: &[Operation {
            method: "post",
            body: include_str!("../openapi/paths/api__v1__governance__approvals__decide.post.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/governance/lifecycle/preflight",
        operations: &[Operation {
            method: "post",
            body: include_str!(
                "../openapi/paths/api__v1__governance__lifecycle__preflight.post.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/governance/lifecycle/transitions",
        operations: &[Operation {
            method: "post",
            body: include_str!(
                "../openapi/paths/api__v1__governance__lifecycle__transitions.post.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/governance/overrides",
        operations: &[Operation {
            method: "post",
            body: include_str!("../openapi/paths/api__v1__governance__overrides.post.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/integrity/findings",
        operations: &[Operation {
            method: "get",
            body: include_str!("../openapi/paths/api__v1__integrity__findings.get.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/integrity/findings/{id}/triage",
        operations: &[Operation {
            method: "post",
            body: include_str!(
                "../openapi/paths/api__v1__integrity__findings__id__triage.post.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/lifecycles/{objectType}/{objectId}",
        operations: &[Operation {
            method: "get",
            body: include_str!(
                "../openapi/paths/api__v1__lifecycles__objectType__objectId.get.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/lifecycles/{objectType}/{objectId}/hold",
        operations: &[Operation {
            method: "post",
            body: include_str!(
                "../openapi/paths/api__v1__lifecycles__objectType__objectId__hold.post.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/lifecycles/{objectType}/{objectId}/transition",
        operations: &[Operation {
            method: "post",
            body: include_str!(
                "../openapi/paths/api__v1__lifecycles__objectType__objectId__transition.post.yaml"
            ),
        }],
    },
];

const SCHEMAS: &[NamedYaml] = &[
    NamedYaml {
        name: "AuditChainAttestation",
        body: include_str!("../openapi/schemas/AuditChainAttestation.yaml"),
    },
    NamedYaml {
        name: "FindingSeverity",
        body: include_str!("../openapi/schemas/FindingSeverity.yaml"),
    },
    NamedYaml {
        name: "FindingStatus",
        body: include_str!("../openapi/schemas/FindingStatus.yaml"),
    },
    NamedYaml {
        name: "GovernanceConfigureTransitionRequest",
        body: include_str!("../openapi/schemas/GovernanceConfigureTransitionRequest.yaml"),
    },
    NamedYaml {
        name: "GovernanceDecideApprovalRequest",
        body: include_str!("../openapi/schemas/GovernanceDecideApprovalRequest.yaml"),
    },
    NamedYaml {
        name: "GovernanceFinding",
        body: include_str!("../openapi/schemas/GovernanceFinding.yaml"),
    },
    NamedYaml {
        name: "GovernanceLifecyclePreflightRequest",
        body: include_str!("../openapi/schemas/GovernanceLifecyclePreflightRequest.yaml"),
    },
    NamedYaml {
        name: "GovernanceOpenOverrideRequest",
        body: include_str!("../openapi/schemas/GovernanceOpenOverrideRequest.yaml"),
    },
    NamedYaml {
        name: "LifecycleState",
        body: include_str!("../openapi/schemas/LifecycleState.yaml"),
    },
    NamedYaml {
        name: "ObjectLifecycle",
        body: include_str!("../openapi/schemas/ObjectLifecycle.yaml"),
    },
    NamedYaml {
        name: "ObjectLifecycleTransition",
        body: include_str!("../openapi/schemas/ObjectLifecycleTransition.yaml"),
    },
    NamedYaml {
        name: "SetLifecycleHoldRequest",
        body: include_str!("../openapi/schemas/SetLifecycleHoldRequest.yaml"),
    },
    NamedYaml {
        name: "TransitionLifecycleRequest",
        body: include_str!("../openapi/schemas/TransitionLifecycleRequest.yaml"),
    },
    NamedYaml {
        name: "TriageFindingRequest",
        body: include_str!("../openapi/schemas/TriageFindingRequest.yaml"),
    },
];
