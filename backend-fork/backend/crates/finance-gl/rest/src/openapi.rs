//! This face's slice of the published OpenAPI contract.
//!
//! Bodies live as YAML under `openapi/` and are pulled in via `include_str!`.
//! `console_contracts` re-indents them; composition rejects duplicate keys.

use console_contracts::{Fragment, NamedYaml, Operation, PathItem};

/// This face's contribution to the composed OpenAPI document.
pub const OPENAPI_FRAGMENT: Fragment = Fragment {
    source: "console-finance-gl-rest",
    paths: PATHS,
    schemas: SCHEMAS,
    parameters: &[],
    responses: &[],
    security_schemes: &[],
    external_schemas: EXTERNAL_SCHEMAS,
};

const EXTERNAL_SCHEMAS: &[&str] = &["Timestamp", "Uuid"];

const PATHS: &[PathItem] = &[
    PathItem {
        path: "/api/v1/finance-gl/accounts/{account_code}/entries",
        operations: &[Operation {
            method: "get",
            body: include_str!(
                "../openapi/paths/api__v1__finance-gl__accounts__account_code__entries.get.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/finance-gl/vouchers",
        operations: &[
            Operation {
                method: "get",
                body: include_str!("../openapi/paths/api__v1__finance-gl__vouchers.get.yaml"),
            },
            Operation {
                method: "post",
                body: include_str!("../openapi/paths/api__v1__finance-gl__vouchers.post.yaml"),
            },
        ],
    },
    PathItem {
        path: "/api/v1/finance-gl/vouchers/{voucher_id}",
        operations: &[Operation {
            method: "get",
            body: include_str!(
                "../openapi/paths/api__v1__finance-gl__vouchers__voucher_id.get.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/finance-gl/vouchers/{voucher_id}/approve",
        operations: &[Operation {
            method: "post",
            body: include_str!(
                "../openapi/paths/api__v1__finance-gl__vouchers__voucher_id__approve.post.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/finance-gl/vouchers/{voucher_id}/post",
        operations: &[Operation {
            method: "post",
            body: include_str!(
                "../openapi/paths/api__v1__finance-gl__vouchers__voucher_id__post.post.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/finance-gl/vouchers/{voucher_id}/reverse",
        operations: &[Operation {
            method: "post",
            body: include_str!(
                "../openapi/paths/api__v1__finance-gl__vouchers__voucher_id__reverse.post.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/finance-gl/vouchers/{voucher_id}/submit",
        operations: &[Operation {
            method: "post",
            body: include_str!(
                "../openapi/paths/api__v1__finance-gl__vouchers__voucher_id__submit.post.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/period-locks",
        operations: &[
            Operation {
                method: "get",
                body: include_str!("../openapi/paths/api__v1__period-locks.get.yaml"),
            },
            Operation {
                method: "post",
                body: include_str!("../openapi/paths/api__v1__period-locks.post.yaml"),
            },
        ],
    },
    PathItem {
        path: "/api/v1/period-locks/{lockId}/unlock",
        operations: &[Operation {
            method: "post",
            body: include_str!("../openapi/paths/api__v1__period-locks__lockId__unlock.post.yaml"),
        }],
    },
];

const SCHEMAS: &[NamedYaml] = &[
    NamedYaml {
        name: "AccountDrillEntry",
        body: include_str!("../openapi/schemas/AccountDrillEntry.yaml"),
    },
    NamedYaml {
        name: "CreatePeriodLockRequest",
        body: include_str!("../openapi/schemas/CreatePeriodLockRequest.yaml"),
    },
    NamedYaml {
        name: "CreateVoucherRequest",
        body: include_str!("../openapi/schemas/CreateVoucherRequest.yaml"),
    },
    NamedYaml {
        name: "DebitCredit",
        body: include_str!("../openapi/schemas/DebitCredit.yaml"),
    },
    NamedYaml {
        name: "PeriodLock",
        body: include_str!("../openapi/schemas/PeriodLock.yaml"),
    },
    NamedYaml {
        name: "PeriodLockList",
        body: include_str!("../openapi/schemas/PeriodLockList.yaml"),
    },
    NamedYaml {
        name: "ReverseVoucherRequest",
        body: include_str!("../openapi/schemas/ReverseVoucherRequest.yaml"),
    },
    NamedYaml {
        name: "UnlockPeriodLockRequest",
        body: include_str!("../openapi/schemas/UnlockPeriodLockRequest.yaml"),
    },
    NamedYaml {
        name: "VoucherLineInput",
        body: include_str!("../openapi/schemas/VoucherLineInput.yaml"),
    },
    NamedYaml {
        name: "VoucherLineSummary",
        body: include_str!("../openapi/schemas/VoucherLineSummary.yaml"),
    },
    NamedYaml {
        name: "VoucherStatus",
        body: include_str!("../openapi/schemas/VoucherStatus.yaml"),
    },
    NamedYaml {
        name: "VoucherSummary",
        body: include_str!("../openapi/schemas/VoucherSummary.yaml"),
    },
];
