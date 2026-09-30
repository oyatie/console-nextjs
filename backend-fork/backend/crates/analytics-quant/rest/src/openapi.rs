//! This face's slice of the published OpenAPI contract.
//!
//! Bodies live as YAML under `openapi/` and are pulled in via `include_str!`.
//! `console_contracts` re-indents them; composition rejects duplicate keys.

use console_contracts::{Fragment, NamedYaml, Operation, PathItem};

/// This face's contribution to the composed OpenAPI document.
pub const OPENAPI_FRAGMENT: Fragment = Fragment {
    source: "console-analytics-quant-rest",
    paths: PATHS,
    schemas: SCHEMAS,
    parameters: &[],
    responses: &[],
    security_schemes: &[],
    external_schemas: EXTERNAL_SCHEMAS,
};

const EXTERNAL_SCHEMAS: &[&str] = &["ErrorBody"];

const PATHS: &[PathItem] = &[PathItem {
    path: "/api/v1/analytics/projection",
    operations: &[Operation {
        method: "post",
        body: include_str!("../openapi/paths/api__v1__analytics__projection.post.yaml"),
    }],
}];

const SCHEMAS: &[NamedYaml] = &[
    NamedYaml {
        name: "ProjectionAssumptions",
        body: include_str!("../openapi/schemas/ProjectionAssumptions.yaml"),
    },
    NamedYaml {
        name: "ProjectionRequest",
        body: include_str!("../openapi/schemas/ProjectionRequest.yaml"),
    },
    NamedYaml {
        name: "ProjectionResult",
        body: include_str!("../openapi/schemas/ProjectionResult.yaml"),
    },
];
