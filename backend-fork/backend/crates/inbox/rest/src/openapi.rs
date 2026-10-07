//! This face's slice of the published OpenAPI contract.
//!
//! Bodies live as YAML under `openapi/` and are pulled in via `include_str!`.
//! `console_contracts` re-indents them; composition rejects duplicate keys.

use console_contracts::{Fragment, NamedYaml, Operation, PathItem};

/// This face's contribution to the composed OpenAPI document.
pub const OPENAPI_FRAGMENT: Fragment = Fragment {
    source: "console-inbox-rest",
    paths: PATHS,
    schemas: SCHEMAS,
    parameters: &[],
    responses: &[],
    security_schemes: &[],
    external_schemas: EXTERNAL_SCHEMAS,
};

const EXTERNAL_SCHEMAS: &[&str] = &[
    "BrowserSessionIdentityRequest",
    "ErrorBody",
    "PasskeyStepUpAssertion",
    "Timestamp",
    "Uuid",
];

const PATHS: &[PathItem] = &[
    PathItem {
        path: "/api/v1/me/browser-session/payslips",
        operations: &[Operation {
            method: "post",
            body: include_str!("../openapi/paths/api__v1__me__browser-session__payslips.post.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/me/browser-session/payslips/{id}",
        operations: &[Operation {
            method: "post",
            body: include_str!(
                "../openapi/paths/api__v1__me__browser-session__payslips__id.post.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/me/inbox-docs",
        operations: &[Operation {
            method: "get",
            body: include_str!("../openapi/paths/api__v1__me__inbox-docs.get.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/me/inbox-docs/{id}",
        operations: &[Operation {
            method: "get",
            body: include_str!("../openapi/paths/api__v1__me__inbox-docs__id.get.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/me/inbox-docs/{id}/confirm-receipt",
        operations: &[Operation {
            method: "post",
            body: include_str!(
                "../openapi/paths/api__v1__me__inbox-docs__id__confirm-receipt.post.yaml"
            ),
        }],
    },
];

const SCHEMAS: &[NamedYaml] = &[
    NamedYaml {
        name: "BrowserPayslipDeduction",
        body: include_str!("../openapi/schemas/BrowserPayslipDeduction.yaml"),
    },
    NamedYaml {
        name: "BrowserPayslipDetailResponse",
        body: include_str!("../openapi/schemas/BrowserPayslipDetailResponse.yaml"),
    },
    NamedYaml {
        name: "BrowserPayslipDocument",
        body: include_str!("../openapi/schemas/BrowserPayslipDocument.yaml"),
    },
    NamedYaml {
        name: "BrowserPayslipListRequest",
        body: include_str!("../openapi/schemas/BrowserPayslipListRequest.yaml"),
    },
    NamedYaml {
        name: "BrowserPayslipListResponse",
        body: include_str!("../openapi/schemas/BrowserPayslipListResponse.yaml"),
    },
    NamedYaml {
        name: "BrowserPayslipMoney",
        body: include_str!("../openapi/schemas/BrowserPayslipMoney.yaml"),
    },
    NamedYaml {
        name: "BrowserPayslipPayload",
        body: include_str!("../openapi/schemas/BrowserPayslipPayload.yaml"),
    },
    NamedYaml {
        name: "BrowserPayslipSummary",
        body: include_str!("../openapi/schemas/BrowserPayslipSummary.yaml"),
    },
    NamedYaml {
        name: "InboxDocConfirmReceiptRequest",
        body: include_str!("../openapi/schemas/InboxDocConfirmReceiptRequest.yaml"),
    },
    NamedYaml {
        name: "InboxDocDetail",
        body: include_str!("../openapi/schemas/InboxDocDetail.yaml"),
    },
    NamedYaml {
        name: "InboxDocPage",
        body: include_str!("../openapi/schemas/InboxDocPage.yaml"),
    },
    NamedYaml {
        name: "InboxDocSummary",
        body: include_str!("../openapi/schemas/InboxDocSummary.yaml"),
    },
];
