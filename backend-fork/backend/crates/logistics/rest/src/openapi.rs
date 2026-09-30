//! This face's slice of the published OpenAPI contract.
//!
//! Bodies live as YAML under `openapi/` and are pulled in via `include_str!`.
//! `console_contracts` re-indents them; composition rejects duplicate keys.

use console_contracts::{Fragment, NamedYaml, Operation, PathItem};

/// This face's contribution to the composed OpenAPI document.
pub const OPENAPI_FRAGMENT: Fragment = Fragment {
    source: "console-logistics-rest",
    paths: PATHS,
    schemas: SCHEMAS,
    parameters: &[],
    responses: &[],
    security_schemes: &[],
    external_schemas: EXTERNAL_SCHEMAS,
};

const EXTERNAL_SCHEMAS: &[&str] = &["Uuid"];

const PATHS: &[PathItem] = &[
    PathItem {
        path: "/api/v1/logistics/asns",
        operations: &[Operation {
            method: "post",
            body: include_str!("../openapi/paths/api__v1__logistics__asns.post.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/logistics/asns/{asn_id}/putaway",
        operations: &[Operation {
            method: "post",
            body: include_str!(
                "../openapi/paths/api__v1__logistics__asns__asn_id__putaway.post.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/logistics/asns/{asn_id}/receipts",
        operations: &[Operation {
            method: "post",
            body: include_str!(
                "../openapi/paths/api__v1__logistics__asns__asn_id__receipts.post.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/logistics/fulfillments",
        operations: &[Operation {
            method: "post",
            body: include_str!("../openapi/paths/api__v1__logistics__fulfillments.post.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/logistics/fulfillments/{fulfillment_id}/dispatch",
        operations: &[Operation {
            method: "post",
            body: include_str!(
                "../openapi/paths/api__v1__logistics__fulfillments__fulfillment_id__dispatch.post.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/logistics/fulfillments/{fulfillment_id}/pack",
        operations: &[Operation {
            method: "post",
            body: include_str!(
                "../openapi/paths/api__v1__logistics__fulfillments__fulfillment_id__pack.post.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/logistics/fulfillments/{fulfillment_id}/pick",
        operations: &[Operation {
            method: "post",
            body: include_str!(
                "../openapi/paths/api__v1__logistics__fulfillments__fulfillment_id__pick.post.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/logistics/shipments/{shipment_id}/pod",
        operations: &[Operation {
            method: "post",
            body: include_str!(
                "../openapi/paths/api__v1__logistics__shipments__shipment_id__pod.post.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/logistics/shipments/{shipment_id}/settlements",
        operations: &[Operation {
            method: "post",
            body: include_str!(
                "../openapi/paths/api__v1__logistics__shipments__shipment_id__settlements.post.yaml"
            ),
        }],
    },
];

const SCHEMAS: &[NamedYaml] = &[
    NamedYaml {
        name: "LogisticsAsnCreated",
        body: include_str!("../openapi/schemas/LogisticsAsnCreated.yaml"),
    },
    NamedYaml {
        name: "LogisticsAsnPutaway",
        body: include_str!("../openapi/schemas/LogisticsAsnPutaway.yaml"),
    },
    NamedYaml {
        name: "LogisticsAsnReceipt",
        body: include_str!("../openapi/schemas/LogisticsAsnReceipt.yaml"),
    },
    NamedYaml {
        name: "LogisticsFulfillmentPacked",
        body: include_str!("../openapi/schemas/LogisticsFulfillmentPacked.yaml"),
    },
    NamedYaml {
        name: "LogisticsFulfillmentPicked",
        body: include_str!("../openapi/schemas/LogisticsFulfillmentPicked.yaml"),
    },
    NamedYaml {
        name: "LogisticsFulfillmentReleased",
        body: include_str!("../openapi/schemas/LogisticsFulfillmentReleased.yaml"),
    },
    NamedYaml {
        name: "LogisticsPodVerified",
        body: include_str!("../openapi/schemas/LogisticsPodVerified.yaml"),
    },
    NamedYaml {
        name: "LogisticsShipmentDispatched",
        body: include_str!("../openapi/schemas/LogisticsShipmentDispatched.yaml"),
    },
    NamedYaml {
        name: "LogisticsShipmentSettlement",
        body: include_str!("../openapi/schemas/LogisticsShipmentSettlement.yaml"),
    },
    NamedYaml {
        name: "LogisticsTimeTuple",
        body: include_str!("../openapi/schemas/LogisticsTimeTuple.yaml"),
    },
];
