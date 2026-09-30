//! This face's slice of the published OpenAPI contract.
//!
//! Bodies live as YAML under `openapi/` and are pulled in via `include_str!`.
//! `console_contracts` re-indents them; composition rejects duplicate keys.

use console_contracts::{Fragment, NamedYaml, Operation, PathItem};

/// This face's contribution to the composed OpenAPI document.
pub const OPENAPI_FRAGMENT: Fragment = Fragment {
    source: "console-reporting-rest",
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
        path: "/api/v1/exports/daily-status",
        operations: &[Operation {
            method: "get",
            body: include_str!("../openapi/paths/api__v1__exports__daily-status.get.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/exports/kpi",
        operations: &[Operation {
            method: "get",
            body: include_str!("../openapi/paths/api__v1__exports__kpi.get.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/exports/work-diary",
        operations: &[Operation {
            method: "get",
            body: include_str!("../openapi/paths/api__v1__exports__work-diary.get.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/kpi",
        operations: &[Operation {
            method: "get",
            body: include_str!("../openapi/paths/api__v1__kpi.get.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/ops/summary",
        operations: &[Operation {
            method: "get",
            body: include_str!("../openapi/paths/api__v1__ops__summary.get.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/reporting/work-diary",
        operations: &[
            Operation {
                method: "get",
                body: include_str!("../openapi/paths/api__v1__reporting__work-diary.get.yaml"),
            },
            Operation {
                method: "put",
                body: include_str!("../openapi/paths/api__v1__reporting__work-diary.put.yaml"),
            },
        ],
    },
    PathItem {
        path: "/api/v1/reporting/work-diary/confirm",
        operations: &[Operation {
            method: "post",
            body: include_str!(
                "../openapi/paths/api__v1__reporting__work-diary__confirm.post.yaml"
            ),
        }],
    },
];

const SCHEMAS: &[NamedYaml] = &[
    NamedYaml {
        name: "ExportSourceNote",
        body: include_str!("../openapi/schemas/ExportSourceNote.yaml"),
    },
    NamedYaml {
        name: "KpiMetric",
        body: include_str!("../openapi/schemas/KpiMetric.yaml"),
    },
    NamedYaml {
        name: "KpiReport",
        body: include_str!("../openapi/schemas/KpiReport.yaml"),
    },
    NamedYaml {
        name: "KpiRollup",
        body: include_str!("../openapi/schemas/KpiRollup.yaml"),
    },
    NamedYaml {
        name: "KpiRollupScope",
        body: include_str!("../openapi/schemas/KpiRollupScope.yaml"),
    },
    NamedYaml {
        name: "KpiScope",
        body: include_str!("../openapi/schemas/KpiScope.yaml"),
    },
    NamedYaml {
        name: "OpsEquipmentStatus",
        body: include_str!("../openapi/schemas/OpsEquipmentStatus.yaml"),
    },
    NamedYaml {
        name: "OpsFunnel",
        body: include_str!("../openapi/schemas/OpsFunnel.yaml"),
    },
    NamedYaml {
        name: "OpsMechanicLoad",
        body: include_str!("../openapi/schemas/OpsMechanicLoad.yaml"),
    },
    NamedYaml {
        name: "OpsSummary",
        body: include_str!("../openapi/schemas/OpsSummary.yaml"),
    },
    NamedYaml {
        name: "Period",
        body: include_str!("../openapi/schemas/Period.yaml"),
    },
    NamedYaml {
        name: "UnavailableMetric",
        body: include_str!("../openapi/schemas/UnavailableMetric.yaml"),
    },
    NamedYaml {
        name: "WorkDiaryActionEntry",
        body: include_str!("../openapi/schemas/WorkDiaryActionEntry.yaml"),
    },
    NamedYaml {
        name: "WorkDiaryBody",
        body: include_str!("../openapi/schemas/WorkDiaryBody.yaml"),
    },
    NamedYaml {
        name: "WorkDiaryDraft",
        body: include_str!("../openapi/schemas/WorkDiaryDraft.yaml"),
    },
    NamedYaml {
        name: "WorkDiaryStatus",
        body: include_str!("../openapi/schemas/WorkDiaryStatus.yaml"),
    },
    NamedYaml {
        name: "WorkDiaryUpdateRequest",
        body: include_str!("../openapi/schemas/WorkDiaryUpdateRequest.yaml"),
    },
];
