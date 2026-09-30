//! This face's slice of the published OpenAPI contract.
//!
//! Bodies live as YAML under `openapi/` and are pulled in via `include_str!`.
//! `console_contracts` re-indents them; composition rejects duplicate keys.

use console_contracts::{Fragment, NamedYaml, Operation, PathItem};

/// This face's contribution to the composed OpenAPI document.
pub const OPENAPI_FRAGMENT: Fragment = Fragment {
    source: "console-facilities-rest",
    paths: PATHS,
    schemas: SCHEMAS,
    parameters: &[],
    responses: &[],
    security_schemes: &[],
    external_schemas: EXTERNAL_SCHEMAS,
};

const EXTERNAL_SCHEMAS: &[&str] = &["ErrorBody", "Uuid"];

const PATHS: &[PathItem] = &[
    PathItem {
        path: "/api/v1/facilities/cases",
        operations: &[
            Operation {
                method: "get",
                body: include_str!("../openapi/paths/api__v1__facilities__cases.get.yaml"),
            },
            Operation {
                method: "post",
                body: include_str!("../openapi/paths/api__v1__facilities__cases.post.yaml"),
            },
        ],
    },
    PathItem {
        path: "/api/v1/facilities/cases/{case_id}",
        operations: &[Operation {
            method: "get",
            body: include_str!("../openapi/paths/api__v1__facilities__cases__case_id.get.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/facilities/cases/{case_id}/acceptance",
        operations: &[Operation {
            method: "post",
            body: include_str!(
                "../openapi/paths/api__v1__facilities__cases__case_id__acceptance.post.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/facilities/cases/{case_id}/assign",
        operations: &[Operation {
            method: "post",
            body: include_str!(
                "../openapi/paths/api__v1__facilities__cases__case_id__assign.post.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/facilities/cases/{case_id}/observations",
        operations: &[Operation {
            method: "post",
            body: include_str!(
                "../openapi/paths/api__v1__facilities__cases__case_id__observations.post.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/facilities/cases/{case_id}/start",
        operations: &[Operation {
            method: "post",
            body: include_str!(
                "../openapi/paths/api__v1__facilities__cases__case_id__start.post.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/facilities/cases/{case_id}/submit",
        operations: &[Operation {
            method: "post",
            body: include_str!(
                "../openapi/paths/api__v1__facilities__cases__case_id__submit.post.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/facilities/cases/{case_id}/triage",
        operations: &[Operation {
            method: "post",
            body: include_str!(
                "../openapi/paths/api__v1__facilities__cases__case_id__triage.post.yaml"
            ),
        }],
    },
];

const SCHEMAS: &[NamedYaml] = &[
    NamedYaml {
        name: "FacilitiesAcceptanceRequest",
        body: include_str!("../openapi/schemas/FacilitiesAcceptanceRequest.yaml"),
    },
    NamedYaml {
        name: "FacilitiesAssignRequest",
        body: include_str!("../openapi/schemas/FacilitiesAssignRequest.yaml"),
    },
    NamedYaml {
        name: "FacilitiesCase",
        body: include_str!("../openapi/schemas/FacilitiesCase.yaml"),
    },
    NamedYaml {
        name: "FacilitiesDueCaseRequest",
        body: include_str!("../openapi/schemas/FacilitiesDueCaseRequest.yaml"),
    },
    NamedYaml {
        name: "FacilitiesObservationRequest",
        body: include_str!("../openapi/schemas/FacilitiesObservationRequest.yaml"),
    },
    NamedYaml {
        name: "FacilitiesSubmitRequest",
        body: include_str!("../openapi/schemas/FacilitiesSubmitRequest.yaml"),
    },
    NamedYaml {
        name: "FacilitiesTriageRequest",
        body: include_str!("../openapi/schemas/FacilitiesTriageRequest.yaml"),
    },
];
