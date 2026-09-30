//! This face's slice of the published OpenAPI contract.
//!
//! Bodies live as YAML under `openapi/` and are pulled in via `include_str!`.
//! `console_contracts` re-indents them; composition rejects duplicate keys.

use console_contracts::{Fragment, NamedYaml, Operation, PathItem};

/// This face's contribution to the composed OpenAPI document.
pub const OPENAPI_FRAGMENT: Fragment = Fragment {
    source: "console-ontology-rest",
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
        path: "/api/objects/{kind}/{id}",
        operations: &[Operation {
            method: "get",
            body: include_str!("../openapi/paths/api__objects__kind__id.get.yaml"),
        }],
    },
    PathItem {
        path: "/api/objects/{kind}/{id}/graph",
        operations: &[Operation {
            method: "get",
            body: include_str!("../openapi/paths/api__objects__kind__id__graph.get.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/link-types",
        operations: &[Operation {
            method: "get",
            body: include_str!("../openapi/paths/api__v1__link-types.get.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/object-links",
        operations: &[
            Operation {
                method: "get",
                body: include_str!("../openapi/paths/api__v1__object-links.get.yaml"),
            },
            Operation {
                method: "post",
                body: include_str!("../openapi/paths/api__v1__object-links.post.yaml"),
            },
        ],
    },
    PathItem {
        path: "/api/v1/object-links/{id}",
        operations: &[Operation {
            method: "delete",
            body: include_str!("../openapi/paths/api__v1__object-links__id.delete.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/object-types",
        operations: &[Operation {
            method: "get",
            body: include_str!("../openapi/paths/api__v1__object-types.get.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/object-types/{kind}",
        operations: &[Operation {
            method: "get",
            body: include_str!("../openapi/paths/api__v1__object-types__kind.get.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/ontology/actions/{action_key}/execute",
        operations: &[Operation {
            method: "post",
            body: include_str!(
                "../openapi/paths/api__v1__ontology__actions__action_key__execute.post.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/ontology/actions/{action_key}/preflight",
        operations: &[Operation {
            method: "post",
            body: include_str!(
                "../openapi/paths/api__v1__ontology__actions__action_key__preflight.post.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/ontology/instances",
        operations: &[Operation {
            method: "get",
            body: include_str!("../openapi/paths/api__v1__ontology__instances.get.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/ontology/instances/aggregate",
        operations: &[Operation {
            method: "get",
            body: include_str!("../openapi/paths/api__v1__ontology__instances__aggregate.get.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/ontology/instances/{id}",
        operations: &[Operation {
            method: "get",
            body: include_str!("../openapi/paths/api__v1__ontology__instances__id.get.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/ontology/instances/{id}/acting",
        operations: &[Operation {
            method: "get",
            body: include_str!(
                "../openapi/paths/api__v1__ontology__instances__id__acting.get.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/ontology/instances/{id}/history",
        operations: &[Operation {
            method: "get",
            body: include_str!(
                "../openapi/paths/api__v1__ontology__instances__id__history.get.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/ontology/instances/{id}/lifecycle",
        operations: &[Operation {
            method: "post",
            body: include_str!(
                "../openapi/paths/api__v1__ontology__instances__id__lifecycle.post.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/ontology/instances/{id}/traverse",
        operations: &[Operation {
            method: "get",
            body: include_str!(
                "../openapi/paths/api__v1__ontology__instances__id__traverse.get.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/ontology/object-types",
        operations: &[
            Operation {
                method: "get",
                body: include_str!("../openapi/paths/api__v1__ontology__object-types.get.yaml"),
            },
            Operation {
                method: "post",
                body: include_str!("../openapi/paths/api__v1__ontology__object-types.post.yaml"),
            },
        ],
    },
    PathItem {
        path: "/api/v1/ontology/object-types/{key}",
        operations: &[
            Operation {
                method: "get",
                body: include_str!(
                    "../openapi/paths/api__v1__ontology__object-types__key.get.yaml"
                ),
            },
            Operation {
                method: "put",
                body: include_str!(
                    "../openapi/paths/api__v1__ontology__object-types__key.put.yaml"
                ),
            },
        ],
    },
    PathItem {
        path: "/api/v1/ontology/object-types/{key}/acting",
        operations: &[Operation {
            method: "get",
            body: include_str!(
                "../openapi/paths/api__v1__ontology__object-types__key__acting.get.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/ontology/object-types/{key}/lifecycle",
        operations: &[Operation {
            method: "post",
            body: include_str!(
                "../openapi/paths/api__v1__ontology__object-types__key__lifecycle.post.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/ontology/object-types/{key}/policies",
        operations: &[Operation {
            method: "post",
            body: include_str!(
                "../openapi/paths/api__v1__ontology__object-types__key__policies.post.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/ontology/resolve",
        operations: &[Operation {
            method: "get",
            body: include_str!("../openapi/paths/api__v1__ontology__resolve.get.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/search",
        operations: &[Operation {
            method: "get",
            body: include_str!("../openapi/paths/api__v1__search.get.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/series",
        operations: &[Operation {
            method: "post",
            body: include_str!("../openapi/paths/api__v1__series.post.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/series/by-instance",
        operations: &[Operation {
            method: "get",
            body: include_str!("../openapi/paths/api__v1__series__by-instance.get.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/series/{id}",
        operations: &[Operation {
            method: "get",
            body: include_str!("../openapi/paths/api__v1__series__id.get.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/series/{id}/instances",
        operations: &[Operation {
            method: "post",
            body: include_str!("../openapi/paths/api__v1__series__id__instances.post.yaml"),
        }],
    },
];

const SCHEMAS: &[NamedYaml] = &[
    NamedYaml {
        name: "ActingRule",
        body: include_str!("../openapi/schemas/ActingRule.yaml"),
    },
    NamedYaml {
        name: "AttachInstanceAck",
        body: include_str!("../openapi/schemas/AttachInstanceAck.yaml"),
    },
    NamedYaml {
        name: "AttachInstanceRequest",
        body: include_str!("../openapi/schemas/AttachInstanceRequest.yaml"),
    },
    NamedYaml {
        name: "CreateObjectLinkRequest",
        body: include_str!("../openapi/schemas/CreateObjectLinkRequest.yaml"),
    },
    NamedYaml {
        name: "CreateObjectTypeDraft",
        body: include_str!("../openapi/schemas/CreateObjectTypeDraft.yaml"),
    },
    NamedYaml {
        name: "CreateSeriesRequest",
        body: include_str!("../openapi/schemas/CreateSeriesRequest.yaml"),
    },
    NamedYaml {
        name: "GateChainConfig",
        body: include_str!("../openapi/schemas/GateChainConfig.yaml"),
    },
    NamedYaml {
        name: "GateChainOutcome",
        body: include_str!("../openapi/schemas/GateChainOutcome.yaml"),
    },
    NamedYaml {
        name: "InstanceHead",
        body: include_str!("../openapi/schemas/InstanceHead.yaml"),
    },
    NamedYaml {
        name: "InstanceLifecycleState",
        body: include_str!("../openapi/schemas/InstanceLifecycleState.yaml"),
    },
    NamedYaml {
        name: "LifecycleOutcome",
        body: include_str!("../openapi/schemas/LifecycleOutcome.yaml"),
    },
    NamedYaml {
        name: "LifecycleRequest",
        body: include_str!("../openapi/schemas/LifecycleRequest.yaml"),
    },
    NamedYaml {
        name: "LinkTypeResponse",
        body: include_str!("../openapi/schemas/LinkTypeResponse.yaml"),
    },
    NamedYaml {
        name: "ObjectGraphResponse",
        body: include_str!("../openapi/schemas/ObjectGraphResponse.yaml"),
    },
    NamedYaml {
        name: "ObjectHead",
        body: include_str!("../openapi/schemas/ObjectHead.yaml"),
    },
    NamedYaml {
        name: "ObjectLinkResponse",
        body: include_str!("../openapi/schemas/ObjectLinkResponse.yaml"),
    },
    NamedYaml {
        name: "ObjectLinksListResponse",
        body: include_str!("../openapi/schemas/ObjectLinksListResponse.yaml"),
    },
    NamedYaml {
        name: "ObjectTypeResponse",
        body: include_str!("../openapi/schemas/ObjectTypeResponse.yaml"),
    },
    NamedYaml {
        name: "ObjectTypeSummary",
        body: include_str!("../openapi/schemas/ObjectTypeSummary.yaml"),
    },
    NamedYaml {
        name: "OntologyActionCommandReceipt",
        body: include_str!("../openapi/schemas/OntologyActionCommandReceipt.yaml"),
    },
    NamedYaml {
        name: "OntologyActionExecuteOutcome",
        body: include_str!("../openapi/schemas/OntologyActionExecuteOutcome.yaml"),
    },
    NamedYaml {
        name: "OntologyActionRequest",
        body: include_str!("../openapi/schemas/OntologyActionRequest.yaml"),
    },
    NamedYaml {
        name: "OntologyInstanceAggregateBucket",
        body: include_str!("../openapi/schemas/OntologyInstanceAggregateBucket.yaml"),
    },
    NamedYaml {
        name: "ResolvedInstance",
        body: include_str!("../openapi/schemas/ResolvedInstance.yaml"),
    },
    NamedYaml {
        name: "SearchResponse",
        body: include_str!("../openapi/schemas/SearchResponse.yaml"),
    },
    NamedYaml {
        name: "SeriesByInstanceResponse",
        body: include_str!("../openapi/schemas/SeriesByInstanceResponse.yaml"),
    },
    NamedYaml {
        name: "SeriesDetailResponse",
        body: include_str!("../openapi/schemas/SeriesDetailResponse.yaml"),
    },
    NamedYaml {
        name: "SeriesHead",
        body: include_str!("../openapi/schemas/SeriesHead.yaml"),
    },
];
