//! This face's slice of the published OpenAPI contract.
//!
//! Bodies live as YAML under `openapi/` and are pulled in via `include_str!`.
//! `console_contracts` re-indents them; composition rejects duplicate keys.

use console_contracts::{Fragment, NamedYaml, Operation, PathItem};

/// This face's contribution to the composed OpenAPI document.
pub const OPENAPI_FRAGMENT: Fragment = Fragment {
    source: "console-equipment-rest",
    paths: PATHS,
    schemas: SCHEMAS,
    parameters: &[],
    responses: &[],
    security_schemes: &[],
    external_schemas: EXTERNAL_SCHEMAS,
};

const EXTERNAL_SCHEMAS: &[&str] = &[
    "EquipmentLookupResponse",
    "EquipmentStatus",
    "NamedEntity",
    "Uuid",
];

const PATHS: &[PathItem] = &[
    PathItem {
        path: "/api/v1/equipment",
        operations: &[
            Operation {
                method: "get",
                body: include_str!("../openapi/paths/api__v1__equipment.get.yaml"),
            },
            Operation {
                method: "post",
                body: include_str!("../openapi/paths/api__v1__equipment.post.yaml"),
            },
        ],
    },
    PathItem {
        path: "/api/v1/equipment-3r/dispositions/{disposition_id}/completion",
        operations: &[Operation {
            method: "post",
            body: include_str!(
                "../openapi/paths/api__v1__equipment-3r__dispositions__disposition_id__completion.post.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/equipment-3r/rental-cases",
        operations: &[
            Operation {
                method: "get",
                body: include_str!("../openapi/paths/api__v1__equipment-3r__rental-cases.get.yaml"),
            },
            Operation {
                method: "post",
                body: include_str!(
                    "../openapi/paths/api__v1__equipment-3r__rental-cases.post.yaml"
                ),
            },
        ],
    },
    PathItem {
        path: "/api/v1/equipment-3r/rental-cases/{case_id}",
        operations: &[Operation {
            method: "get",
            body: include_str!(
                "../openapi/paths/api__v1__equipment-3r__rental-cases__case_id.get.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/equipment-3r/rental-cases/{case_id}/approval",
        operations: &[Operation {
            method: "post",
            body: include_str!(
                "../openapi/paths/api__v1__equipment-3r__rental-cases__case_id__approval.post.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/equipment-3r/rental-cases/{case_id}/assessment",
        operations: &[Operation {
            method: "post",
            body: include_str!(
                "../openapi/paths/api__v1__equipment-3r__rental-cases__case_id__assessment.post.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/equipment-3r/rental-cases/{case_id}/dispatch",
        operations: &[Operation {
            method: "post",
            body: include_str!(
                "../openapi/paths/api__v1__equipment-3r__rental-cases__case_id__dispatch.post.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/equipment-3r/rental-cases/{case_id}/handover",
        operations: &[Operation {
            method: "post",
            body: include_str!(
                "../openapi/paths/api__v1__equipment-3r__rental-cases__case_id__handover.post.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/equipment-3r/rental-cases/{case_id}/inspections",
        operations: &[Operation {
            method: "post",
            body: include_str!(
                "../openapi/paths/api__v1__equipment-3r__rental-cases__case_id__inspections.post.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/equipment-3r/rental-cases/{case_id}/return",
        operations: &[Operation {
            method: "post",
            body: include_str!(
                "../openapi/paths/api__v1__equipment-3r__rental-cases__case_id__return.post.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/equipment-3r/units",
        operations: &[
            Operation {
                method: "get",
                body: include_str!("../openapi/paths/api__v1__equipment-3r__units.get.yaml"),
            },
            Operation {
                method: "post",
                body: include_str!("../openapi/paths/api__v1__equipment-3r__units.post.yaml"),
            },
        ],
    },
    PathItem {
        path: "/api/v1/equipment-3r/units/{unit_id}",
        operations: &[Operation {
            method: "get",
            body: include_str!("../openapi/paths/api__v1__equipment-3r__units__unit_id.get.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/equipment-3r/units/{unit_id}/history",
        operations: &[Operation {
            method: "get",
            body: include_str!(
                "../openapi/paths/api__v1__equipment-3r__units__unit_id__history.get.yaml"
            ),
        }],
    },
];

const SCHEMAS: &[NamedYaml] = &[
    NamedYaml {
        name: "CreateEquipmentRequest",
        body: include_str!("../openapi/schemas/CreateEquipmentRequest.yaml"),
    },
    NamedYaml {
        name: "CreateEquipmentResponse",
        body: include_str!("../openapi/schemas/CreateEquipmentResponse.yaml"),
    },
    NamedYaml {
        name: "Equipment3rCaseDetailView",
        body: include_str!("../openapi/schemas/Equipment3rCaseDetailView.yaml"),
    },
    NamedYaml {
        name: "Equipment3rCaseView",
        body: include_str!("../openapi/schemas/Equipment3rCaseView.yaml"),
    },
    NamedYaml {
        name: "Equipment3rDispositionView",
        body: include_str!("../openapi/schemas/Equipment3rDispositionView.yaml"),
    },
    NamedYaml {
        name: "Equipment3rHistoryEntry",
        body: include_str!("../openapi/schemas/Equipment3rHistoryEntry.yaml"),
    },
    NamedYaml {
        name: "Equipment3rInspectionView",
        body: include_str!("../openapi/schemas/Equipment3rInspectionView.yaml"),
    },
    NamedYaml {
        name: "Equipment3rUnitDetailView",
        body: include_str!("../openapi/schemas/Equipment3rUnitDetailView.yaml"),
    },
    NamedYaml {
        name: "Equipment3rUnitView",
        body: include_str!("../openapi/schemas/Equipment3rUnitView.yaml"),
    },
    NamedYaml {
        name: "EquipmentAutocompletePage",
        body: include_str!("../openapi/schemas/EquipmentAutocompletePage.yaml"),
    },
];
