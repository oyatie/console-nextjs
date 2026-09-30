//! This face's slice of the published OpenAPI contract.
//!
//! Bodies live as YAML under `openapi/` and are pulled in via `include_str!`.
//! `console_contracts` re-indents them; composition rejects duplicate keys.

use console_contracts::{Fragment, NamedYaml, Operation, PathItem};

/// This face's contribution to the composed OpenAPI document.
pub const OPENAPI_FRAGMENT: Fragment = Fragment {
    source: "console-dispatch-rest",
    paths: PATHS,
    schemas: SCHEMAS,
    parameters: &[],
    responses: &[],
    security_schemes: &[],
    external_schemas: EXTERNAL_SCHEMAS,
};

const EXTERNAL_SCHEMAS: &[&str] = &[
    "ErrorBody",
    "PriorityLevel",
    "Timestamp",
    "Uuid",
    "WorkOrderStatus",
];

const PATHS: &[PathItem] = &[
    PathItem {
        path: "/api/v1/console/dispatch/queue",
        operations: &[Operation {
            method: "get",
            body: include_str!("../openapi/paths/api__v1__console__dispatch__queue.get.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/me/dispatch-offers",
        operations: &[Operation {
            method: "get",
            body: include_str!("../openapi/paths/api__v1__me__dispatch-offers.get.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/p1-dispatches/{dispatchId}",
        operations: &[Operation {
            method: "get",
            body: include_str!("../openapi/paths/api__v1__p1-dispatches__dispatchId.get.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/p1-dispatches/{dispatchId}/candidates",
        operations: &[Operation {
            method: "get",
            body: include_str!(
                "../openapi/paths/api__v1__p1-dispatches__dispatchId__candidates.get.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/p1-dispatches/{dispatchId}/force-assign",
        operations: &[Operation {
            method: "post",
            body: include_str!(
                "../openapi/paths/api__v1__p1-dispatches__dispatchId__force-assign.post.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/p1-dispatches/{dispatchId}/responses",
        operations: &[
            Operation {
                method: "get",
                body: include_str!(
                    "../openapi/paths/api__v1__p1-dispatches__dispatchId__responses.get.yaml"
                ),
            },
            Operation {
                method: "post",
                body: include_str!(
                    "../openapi/paths/api__v1__p1-dispatches__dispatchId__responses.post.yaml"
                ),
            },
        ],
    },
    PathItem {
        path: "/api/v1/work-orders/{workOrderId}/p1-dispatch",
        operations: &[Operation {
            method: "post",
            body: include_str!(
                "../openapi/paths/api__v1__work-orders__workOrderId__p1-dispatch.post.yaml"
            ),
        }],
    },
];

const SCHEMAS: &[NamedYaml] = &[
    NamedYaml {
        name: "DispatchCandidatePage",
        body: include_str!("../openapi/schemas/DispatchCandidatePage.yaml"),
    },
    NamedYaml {
        name: "DispatchCandidateSummary",
        body: include_str!("../openapi/schemas/DispatchCandidateSummary.yaml"),
    },
    NamedYaml {
        name: "DispatchQueueDispatch",
        body: include_str!("../openapi/schemas/DispatchQueueDispatch.yaml"),
    },
    NamedYaml {
        name: "DispatchQueueItem",
        body: include_str!("../openapi/schemas/DispatchQueueItem.yaml"),
    },
    NamedYaml {
        name: "DispatchQueuePage",
        body: include_str!("../openapi/schemas/DispatchQueuePage.yaml"),
    },
    NamedYaml {
        name: "DispatchQueueStats",
        body: include_str!("../openapi/schemas/DispatchQueueStats.yaml"),
    },
    NamedYaml {
        name: "DispatchQueueStatus",
        body: include_str!("../openapi/schemas/DispatchQueueStatus.yaml"),
    },
    NamedYaml {
        name: "DispatchResponseKind",
        body: include_str!("../openapi/schemas/DispatchResponseKind.yaml"),
    },
    NamedYaml {
        name: "DispatchStatus",
        body: include_str!("../openapi/schemas/DispatchStatus.yaml"),
    },
    NamedYaml {
        name: "ForceAssignP1DispatchRequest",
        body: include_str!("../openapi/schemas/ForceAssignP1DispatchRequest.yaml"),
    },
    NamedYaml {
        name: "IncidentLocation",
        body: include_str!("../openapi/schemas/IncidentLocation.yaml"),
    },
    NamedYaml {
        name: "MyDispatchOffer",
        body: include_str!("../openapi/schemas/MyDispatchOffer.yaml"),
    },
    NamedYaml {
        name: "MyDispatchOfferPage",
        body: include_str!("../openapi/schemas/MyDispatchOfferPage.yaml"),
    },
    NamedYaml {
        name: "P1DispatchResponsePage",
        body: include_str!("../openapi/schemas/P1DispatchResponsePage.yaml"),
    },
    NamedYaml {
        name: "P1DispatchResponseSummary",
        body: include_str!("../openapi/schemas/P1DispatchResponseSummary.yaml"),
    },
    NamedYaml {
        name: "P1DispatchSummary",
        body: include_str!("../openapi/schemas/P1DispatchSummary.yaml"),
    },
    NamedYaml {
        name: "RespondP1DispatchRequest",
        body: include_str!("../openapi/schemas/RespondP1DispatchRequest.yaml"),
    },
    NamedYaml {
        name: "StartP1DispatchRequest",
        body: include_str!("../openapi/schemas/StartP1DispatchRequest.yaml"),
    },
];
