//! This face's slice of the published OpenAPI contract.
//!
//! Bodies live as YAML under `openapi/` and are pulled in via `include_str!`.
//! `console_contracts` re-indents them; composition rejects duplicate keys.

use console_contracts::{Fragment, NamedYaml, Operation, PathItem};

/// This face's contribution to the composed OpenAPI document.
pub const OPENAPI_FRAGMENT: Fragment = Fragment {
    source: "console-notices-rest",
    paths: PATHS,
    schemas: SCHEMAS,
    parameters: &[],
    responses: &[],
    security_schemes: &[],
    external_schemas: EXTERNAL_SCHEMAS,
};

const EXTERNAL_SCHEMAS: &[&str] = &["NamedEntity", "Timestamp", "Uuid"];

const PATHS: &[PathItem] = &[
    PathItem {
        path: "/api/v1/notices",
        operations: &[
            Operation {
                method: "get",
                body: include_str!("../openapi/paths/api__v1__notices.get.yaml"),
            },
            Operation {
                method: "post",
                body: include_str!("../openapi/paths/api__v1__notices.post.yaml"),
            },
        ],
    },
    PathItem {
        path: "/api/v1/notices/{id}",
        operations: &[
            Operation {
                method: "get",
                body: include_str!("../openapi/paths/api__v1__notices__id.get.yaml"),
            },
            Operation {
                method: "patch",
                body: include_str!("../openapi/paths/api__v1__notices__id.patch.yaml"),
            },
        ],
    },
    PathItem {
        path: "/api/v1/notices/{id}/ack",
        operations: &[Operation {
            method: "post",
            body: include_str!("../openapi/paths/api__v1__notices__id__ack.post.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/notices/{id}/progress",
        operations: &[Operation {
            method: "get",
            body: include_str!("../openapi/paths/api__v1__notices__id__progress.get.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/notices/{id}/publish",
        operations: &[Operation {
            method: "post",
            body: include_str!("../openapi/paths/api__v1__notices__id__publish.post.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/notices/{id}/receipts",
        operations: &[Operation {
            method: "get",
            body: include_str!("../openapi/paths/api__v1__notices__id__receipts.get.yaml"),
        }],
    },
];

const SCHEMAS: &[NamedYaml] = &[
    NamedYaml {
        name: "CreateNoticeDraftRequest",
        body: include_str!("../openapi/schemas/CreateNoticeDraftRequest.yaml"),
    },
    NamedYaml {
        name: "NoticeAudienceInput",
        body: include_str!("../openapi/schemas/NoticeAudienceInput.yaml"),
    },
    NamedYaml {
        name: "NoticeCategory",
        body: include_str!("../openapi/schemas/NoticeCategory.yaml"),
    },
    NamedYaml {
        name: "NoticeMyReceipt",
        body: include_str!("../openapi/schemas/NoticeMyReceipt.yaml"),
    },
    NamedYaml {
        name: "NoticeProgress",
        body: include_str!("../openapi/schemas/NoticeProgress.yaml"),
    },
    NamedYaml {
        name: "NoticeReceipt",
        body: include_str!("../openapi/schemas/NoticeReceipt.yaml"),
    },
    NamedYaml {
        name: "NoticeReceiptPage",
        body: include_str!("../openapi/schemas/NoticeReceiptPage.yaml"),
    },
    NamedYaml {
        name: "NoticeSummary",
        body: include_str!("../openapi/schemas/NoticeSummary.yaml"),
    },
    NamedYaml {
        name: "UpdateNoticeDraftRequest",
        body: include_str!("../openapi/schemas/UpdateNoticeDraftRequest.yaml"),
    },
];
