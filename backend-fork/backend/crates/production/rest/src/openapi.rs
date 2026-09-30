//! This face's slice of the published OpenAPI contract.
//!
//! Bodies live as YAML under `openapi/` and are pulled in via `include_str!`.
//! `console_contracts` re-indents them; composition rejects duplicate keys.

use console_contracts::{Fragment, NamedYaml, Operation, PathItem};

/// This face's contribution to the composed OpenAPI document.
pub const OPENAPI_FRAGMENT: Fragment = Fragment {
    source: "console-production-rest",
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
        path: "/api/v1/production/capacity-slots",
        operations: &[Operation {
            method: "get",
            body: include_str!("../openapi/paths/api__v1__production__capacity-slots.get.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/production/plans",
        operations: &[
            Operation {
                method: "get",
                body: include_str!("../openapi/paths/api__v1__production__plans.get.yaml"),
            },
            Operation {
                method: "post",
                body: include_str!("../openapi/paths/api__v1__production__plans.post.yaml"),
            },
        ],
    },
    PathItem {
        path: "/api/v1/production/plans/{plan_id}",
        operations: &[Operation {
            method: "get",
            body: include_str!("../openapi/paths/api__v1__production__plans__plan_id.get.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/production/plans/{plan_id}/operations/{operation_id}/records",
        operations: &[Operation {
            method: "post",
            body: include_str!(
                "../openapi/paths/api__v1__production__plans__plan_id__operations__operation_id__records.post.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/production/plans/{plan_id}/release",
        operations: &[Operation {
            method: "post",
            body: include_str!(
                "../openapi/paths/api__v1__production__plans__plan_id__release.post.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/production/source-ingress",
        operations: &[Operation {
            method: "post",
            body: include_str!("../openapi/paths/api__v1__production__source-ingress.post.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/production/source-systems",
        operations: &[Operation {
            method: "post",
            body: include_str!("../openapi/paths/api__v1__production__source-systems.post.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/production/source-systems/{source_system_id}/disable",
        operations: &[Operation {
            method: "post",
            body: include_str!(
                "../openapi/paths/api__v1__production__source-systems__source_system_id__disable.post.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/production/source-systems/{source_system_id}/rotate",
        operations: &[Operation {
            method: "post",
            body: include_str!(
                "../openapi/paths/api__v1__production__source-systems__source_system_id__rotate.post.yaml"
            ),
        }],
    },
];

const SCHEMAS: &[NamedYaml] = &[
    NamedYaml {
        name: "CreateProductionPlan",
        body: include_str!("../openapi/schemas/CreateProductionPlan.yaml"),
    },
    NamedYaml {
        name: "ProductionCapacityIngress",
        body: include_str!("../openapi/schemas/ProductionCapacityIngress.yaml"),
    },
    NamedYaml {
        name: "ProductionCapacitySlot",
        body: include_str!("../openapi/schemas/ProductionCapacitySlot.yaml"),
    },
    NamedYaml {
        name: "ProductionDemandIngress",
        body: include_str!("../openapi/schemas/ProductionDemandIngress.yaml"),
    },
    NamedYaml {
        name: "ProductionMaterialIngress",
        body: include_str!("../openapi/schemas/ProductionMaterialIngress.yaml"),
    },
    NamedYaml {
        name: "ProductionOperation",
        body: include_str!("../openapi/schemas/ProductionOperation.yaml"),
    },
    NamedYaml {
        name: "ProductionPlan",
        body: include_str!("../openapi/schemas/ProductionPlan.yaml"),
    },
    NamedYaml {
        name: "ProductionPlanDetail",
        body: include_str!("../openapi/schemas/ProductionPlanDetail.yaml"),
    },
    NamedYaml {
        name: "ProductionSourceIngress",
        body: include_str!("../openapi/schemas/ProductionSourceIngress.yaml"),
    },
    NamedYaml {
        name: "ProductionSourceIngressReceipt",
        body: include_str!("../openapi/schemas/ProductionSourceIngressReceipt.yaml"),
    },
    NamedYaml {
        name: "ProductionSourceSystemCredential",
        body: include_str!("../openapi/schemas/ProductionSourceSystemCredential.yaml"),
    },
    NamedYaml {
        name: "ProductionSourceSystemGenerationRequest",
        body: include_str!("../openapi/schemas/ProductionSourceSystemGenerationRequest.yaml"),
    },
    NamedYaml {
        name: "ProductionSourceSystemReceipt",
        body: include_str!("../openapi/schemas/ProductionSourceSystemReceipt.yaml"),
    },
    NamedYaml {
        name: "RecordProductionOperation",
        body: include_str!("../openapi/schemas/RecordProductionOperation.yaml"),
    },
    NamedYaml {
        name: "RegisterProductionSourceSystem",
        body: include_str!("../openapi/schemas/RegisterProductionSourceSystem.yaml"),
    },
    NamedYaml {
        name: "ReleaseProductionPlan",
        body: include_str!("../openapi/schemas/ReleaseProductionPlan.yaml"),
    },
];
