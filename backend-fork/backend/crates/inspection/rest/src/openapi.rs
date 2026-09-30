//! This face's slice of the published OpenAPI contract.
//!
//! Bodies live as YAML under `openapi/` and are pulled in via `include_str!`.
//! `console_contracts` re-indents them; composition rejects duplicate keys.

use console_contracts::{Fragment, NamedYaml, Operation, PathItem};

/// This face's contribution to the composed OpenAPI document.
pub const OPENAPI_FRAGMENT: Fragment = Fragment {
    source: "console-inspection-rest",
    paths: PATHS,
    schemas: SCHEMAS,
    parameters: &[],
    responses: &[],
    security_schemes: &[],
    external_schemas: EXTERNAL_SCHEMAS,
};

const EXTERNAL_SCHEMAS: &[&str] = &["Date", "ErrorBody", "Timestamp", "Uuid"];

const PATHS: &[PathItem] = &[
    PathItem {
        path: "/api/v1/inspections/my-schedules",
        operations: &[Operation {
            method: "get",
            body: include_str!("../openapi/paths/api__v1__inspections__my-schedules.get.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/inspections/schedules",
        operations: &[
            Operation {
                method: "get",
                body: include_str!("../openapi/paths/api__v1__inspections__schedules.get.yaml"),
            },
            Operation {
                method: "post",
                body: include_str!("../openapi/paths/api__v1__inspections__schedules.post.yaml"),
            },
        ],
    },
    PathItem {
        path: "/api/v1/inspections/schedules/{schedule_id}/rounds",
        operations: &[Operation {
            method: "post",
            body: include_str!(
                "../openapi/paths/api__v1__inspections__schedules__schedule_id__rounds.post.yaml"
            ),
        }],
    },
];

const SCHEMAS: &[NamedYaml] = &[
    NamedYaml {
        name: "CompleteInspectionRoundRequest",
        body: include_str!("../openapi/schemas/CompleteInspectionRoundRequest.yaml"),
    },
    NamedYaml {
        name: "CreateInspectionScheduleRequest",
        body: include_str!("../openapi/schemas/CreateInspectionScheduleRequest.yaml"),
    },
    NamedYaml {
        name: "InspectionCycle",
        body: include_str!("../openapi/schemas/InspectionCycle.yaml"),
    },
    NamedYaml {
        name: "InspectionRoundOutcome",
        body: include_str!("../openapi/schemas/InspectionRoundOutcome.yaml"),
    },
    NamedYaml {
        name: "InspectionRoundSummary",
        body: include_str!("../openapi/schemas/InspectionRoundSummary.yaml"),
    },
    NamedYaml {
        name: "InspectionSchedulePage",
        body: include_str!("../openapi/schemas/InspectionSchedulePage.yaml"),
    },
    NamedYaml {
        name: "InspectionScheduleStatus",
        body: include_str!("../openapi/schemas/InspectionScheduleStatus.yaml"),
    },
    NamedYaml {
        name: "InspectionScheduleSummary",
        body: include_str!("../openapi/schemas/InspectionScheduleSummary.yaml"),
    },
];
