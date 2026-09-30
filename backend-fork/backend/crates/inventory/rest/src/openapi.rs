//! This face's slice of the published OpenAPI contract.
//!
//! Bodies live as YAML under `openapi/` and are pulled in via `include_str!`.
//! `console_contracts` re-indents them; composition rejects duplicate keys.

use console_contracts::{Fragment, NamedYaml, Operation, PathItem};

/// This face's contribution to the composed OpenAPI document.
pub const OPENAPI_FRAGMENT: Fragment = Fragment {
    source: "console-inventory-rest",
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
        path: "/api/v1/inventory/cycle-counts",
        operations: &[
            Operation {
                method: "get",
                body: include_str!("../openapi/paths/api__v1__inventory__cycle-counts.get.yaml"),
            },
            Operation {
                method: "post",
                body: include_str!("../openapi/paths/api__v1__inventory__cycle-counts.post.yaml"),
            },
        ],
    },
    PathItem {
        path: "/api/v1/inventory/cycle-counts/{count_id}",
        operations: &[Operation {
            method: "get",
            body: include_str!(
                "../openapi/paths/api__v1__inventory__cycle-counts__count_id.get.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/inventory/cycle-counts/{count_id}/cancel",
        operations: &[Operation {
            method: "post",
            body: include_str!(
                "../openapi/paths/api__v1__inventory__cycle-counts__count_id__cancel.post.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/inventory/cycle-counts/{count_id}/decision",
        operations: &[Operation {
            method: "post",
            body: include_str!(
                "../openapi/paths/api__v1__inventory__cycle-counts__count_id__decision.post.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/inventory/cycle-counts/{count_id}/lines",
        operations: &[Operation {
            method: "post",
            body: include_str!(
                "../openapi/paths/api__v1__inventory__cycle-counts__count_id__lines.post.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/inventory/cycle-counts/{count_id}/submit",
        operations: &[Operation {
            method: "post",
            body: include_str!(
                "../openapi/paths/api__v1__inventory__cycle-counts__count_id__submit.post.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/inventory/items",
        operations: &[Operation {
            method: "get",
            body: include_str!("../openapi/paths/api__v1__inventory__items.get.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/inventory/items/{item_id}",
        operations: &[Operation {
            method: "get",
            body: include_str!("../openapi/paths/api__v1__inventory__items__item_id.get.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/inventory/items/{item_id}/consumptions",
        operations: &[
            Operation {
                method: "get",
                body: include_str!(
                    "../openapi/paths/api__v1__inventory__items__item_id__consumptions.get.yaml"
                ),
            },
            Operation {
                method: "post",
                body: include_str!(
                    "../openapi/paths/api__v1__inventory__items__item_id__consumptions.post.yaml"
                ),
            },
        ],
    },
    PathItem {
        path: "/api/v1/inventory/items/{item_id}/movements",
        operations: &[Operation {
            method: "get",
            body: include_str!(
                "../openapi/paths/api__v1__inventory__items__item_id__movements.get.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/inventory/items/{item_id}/receipts",
        operations: &[Operation {
            method: "post",
            body: include_str!(
                "../openapi/paths/api__v1__inventory__items__item_id__receipts.post.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/inventory/mrp",
        operations: &[Operation {
            method: "get",
            body: include_str!("../openapi/paths/api__v1__inventory__mrp.get.yaml"),
        }],
    },
];

const SCHEMAS: &[NamedYaml] = &[
    NamedYaml {
        name: "ConsumeInventoryItemRequest",
        body: include_str!("../openapi/schemas/ConsumeInventoryItemRequest.yaml"),
    },
    NamedYaml {
        name: "CycleCount",
        body: include_str!("../openapi/schemas/CycleCount.yaml"),
    },
    NamedYaml {
        name: "CycleCountDetail",
        body: include_str!("../openapi/schemas/CycleCountDetail.yaml"),
    },
    NamedYaml {
        name: "CycleCountLine",
        body: include_str!("../openapi/schemas/CycleCountLine.yaml"),
    },
    NamedYaml {
        name: "CycleCountPage",
        body: include_str!("../openapi/schemas/CycleCountPage.yaml"),
    },
    NamedYaml {
        name: "CycleCountVersionRequest",
        body: include_str!("../openapi/schemas/CycleCountVersionRequest.yaml"),
    },
    NamedYaml {
        name: "DecideCycleCountRequest",
        body: include_str!("../openapi/schemas/DecideCycleCountRequest.yaml"),
    },
    NamedYaml {
        name: "InventoryConsumptionEvent",
        body: include_str!("../openapi/schemas/InventoryConsumptionEvent.yaml"),
    },
    NamedYaml {
        name: "InventoryConsumptionResult",
        body: include_str!("../openapi/schemas/InventoryConsumptionResult.yaml"),
    },
    NamedYaml {
        name: "InventoryConsumptionSource",
        body: include_str!("../openapi/schemas/InventoryConsumptionSource.yaml"),
    },
    NamedYaml {
        name: "InventoryItem",
        body: include_str!("../openapi/schemas/InventoryItem.yaml"),
    },
    NamedYaml {
        name: "InventoryItemPage",
        body: include_str!("../openapi/schemas/InventoryItemPage.yaml"),
    },
    NamedYaml {
        name: "InventoryMovement",
        body: include_str!("../openapi/schemas/InventoryMovement.yaml"),
    },
    NamedYaml {
        name: "InventoryMovementSource",
        body: include_str!("../openapi/schemas/InventoryMovementSource.yaml"),
    },
    NamedYaml {
        name: "InventoryMovementSourceCycleCount",
        body: include_str!("../openapi/schemas/InventoryMovementSourceCycleCount.yaml"),
    },
    NamedYaml {
        name: "InventoryMovementSourceExternalRef",
        body: include_str!("../openapi/schemas/InventoryMovementSourceExternalRef.yaml"),
    },
    NamedYaml {
        name: "InventoryMovementSourceP1Dispatch",
        body: include_str!("../openapi/schemas/InventoryMovementSourceP1Dispatch.yaml"),
    },
    NamedYaml {
        name: "InventoryMovementSourceWorkOrder",
        body: include_str!("../openapi/schemas/InventoryMovementSourceWorkOrder.yaml"),
    },
    NamedYaml {
        name: "InventoryMrpLine",
        body: include_str!("../openapi/schemas/InventoryMrpLine.yaml"),
    },
    NamedYaml {
        name: "InventoryReceiptResult",
        body: include_str!("../openapi/schemas/InventoryReceiptResult.yaml"),
    },
    NamedYaml {
        name: "InventoryStockLocationSummary",
        body: include_str!("../openapi/schemas/InventoryStockLocationSummary.yaml"),
    },
    NamedYaml {
        name: "OpenCycleCountRequest",
        body: include_str!("../openapi/schemas/OpenCycleCountRequest.yaml"),
    },
    NamedYaml {
        name: "RecordInventoryReceiptRequest",
        body: include_str!("../openapi/schemas/RecordInventoryReceiptRequest.yaml"),
    },
    NamedYaml {
        name: "UpsertCycleCountLineRequest",
        body: include_str!("../openapi/schemas/UpsertCycleCountLineRequest.yaml"),
    },
];
