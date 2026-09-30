//! This face's slice of the published OpenAPI contract.
//!
//! Bodies live as YAML under `openapi/` and are pulled in via `include_str!`.
//! `console_contracts` re-indents them; composition rejects duplicate keys.

use console_contracts::{Fragment, NamedYaml, Operation, PathItem};

/// This face's contribution to the composed OpenAPI document.
pub const OPENAPI_FRAGMENT: Fragment = Fragment {
    source: "console-leave-rest",
    paths: PATHS,
    schemas: SCHEMAS,
    parameters: &[],
    responses: &[],
    security_schemes: &[],
    external_schemas: EXTERNAL_SCHEMAS,
};

const EXTERNAL_SCHEMAS: &[&str] = &["ErrorBody", "Timestamp", "Uuid"];

const PATHS: &[PathItem] = &[
    PathItem {
        path: "/api/v1/leave/balances",
        operations: &[Operation {
            method: "get",
            body: include_str!("../openapi/paths/api__v1__leave__balances.get.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/leave/promotions",
        operations: &[Operation {
            method: "post",
            body: include_str!("../openapi/paths/api__v1__leave__promotions.post.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/leave/refusal-notices",
        operations: &[Operation {
            method: "post",
            body: include_str!("../openapi/paths/api__v1__leave__refusal-notices.post.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/leave/requests",
        operations: &[Operation {
            method: "get",
            body: include_str!("../openapi/paths/api__v1__leave__requests.get.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/leave/requests/{id}/decide",
        operations: &[Operation {
            method: "post",
            body: include_str!("../openapi/paths/api__v1__leave__requests__id__decide.post.yaml"),
        }],
    },
    PathItem {
        path: "/api/v2/leave/requests",
        operations: &[
            Operation {
                method: "get",
                body: include_str!("../openapi/paths/api__v2__leave__requests.get.yaml"),
            },
            Operation {
                method: "post",
                body: include_str!("../openapi/paths/api__v2__leave__requests.post.yaml"),
            },
        ],
    },
    PathItem {
        path: "/api/v2/leave/requests/{id}/alternate-dates",
        operations: &[Operation {
            method: "post",
            body: include_str!(
                "../openapi/paths/api__v2__leave__requests__id__alternate-dates.post.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v2/leave/requests/{id}/charge-resolution",
        operations: &[Operation {
            method: "post",
            body: include_str!(
                "../openapi/paths/api__v2__leave__requests__id__charge-resolution.post.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v2/leave/requests/{id}/decide",
        operations: &[Operation {
            method: "post",
            body: include_str!("../openapi/paths/api__v2__leave__requests__id__decide.post.yaml"),
        }],
    },
    PathItem {
        path: "/api/v2/me/leave",
        operations: &[Operation {
            method: "get",
            body: include_str!("../openapi/paths/api__v2__me__leave.get.yaml"),
        }],
    },
];

const SCHEMAS: &[NamedYaml] = &[
    NamedYaml {
        name: "LeaveBalanceAmount",
        body: include_str!("../openapi/schemas/LeaveBalanceAmount.yaml"),
    },
    NamedYaml {
        name: "LeaveChargeResolutionRequest",
        body: include_str!("../openapi/schemas/LeaveChargeResolutionRequest.yaml"),
    },
    NamedYaml {
        name: "LeaveChargeResolutionView",
        body: include_str!("../openapi/schemas/LeaveChargeResolutionView.yaml"),
    },
    NamedYaml {
        name: "LeaveChargeReviewReason",
        body: include_str!("../openapi/schemas/LeaveChargeReviewReason.yaml"),
    },
    NamedYaml {
        name: "LeaveCreateRequest",
        body: include_str!("../openapi/schemas/LeaveCreateRequest.yaml"),
    },
    NamedYaml {
        name: "LeaveDateCharge",
        body: include_str!("../openapi/schemas/LeaveDateCharge.yaml"),
    },
    NamedYaml {
        name: "LeaveDecideRequest",
        body: include_str!("../openapi/schemas/LeaveDecideRequest.yaml"),
    },
    NamedYaml {
        name: "LeaveDecideV2Request",
        body: include_str!("../openapi/schemas/LeaveDecideV2Request.yaml"),
    },
    NamedYaml {
        name: "LeavePromotionRequest",
        body: include_str!("../openapi/schemas/LeavePromotionRequest.yaml"),
    },
    NamedYaml {
        name: "LeaveProposeAlternateDatesRequest",
        body: include_str!("../openapi/schemas/LeaveProposeAlternateDatesRequest.yaml"),
    },
    NamedYaml {
        name: "LeaveRefusalRequest",
        body: include_str!("../openapi/schemas/LeaveRefusalRequest.yaml"),
    },
    NamedYaml {
        name: "LeaveRequestPage",
        body: include_str!("../openapi/schemas/LeaveRequestPage.yaml"),
    },
    NamedYaml {
        name: "LeaveRequestV2Page",
        body: include_str!("../openapi/schemas/LeaveRequestV2Page.yaml"),
    },
    NamedYaml {
        name: "LeaveRequestV2View",
        body: include_str!("../openapi/schemas/LeaveRequestV2View.yaml"),
    },
    NamedYaml {
        name: "LeaveRequestView",
        body: include_str!("../openapi/schemas/LeaveRequestView.yaml"),
    },
    NamedYaml {
        name: "LeaveRosterEntry",
        body: include_str!("../openapi/schemas/LeaveRosterEntry.yaml"),
    },
    NamedYaml {
        name: "LeaveRosterPage",
        body: include_str!("../openapi/schemas/LeaveRosterPage.yaml"),
    },
    NamedYaml {
        name: "LeaveSourceRevisionRef",
        body: include_str!("../openapi/schemas/LeaveSourceRevisionRef.yaml"),
    },
    NamedYaml {
        name: "LeaveStatutoryPushView",
        body: include_str!("../openapi/schemas/LeaveStatutoryPushView.yaml"),
    },
    NamedYaml {
        name: "LeaveUnits",
        body: include_str!("../openapi/schemas/LeaveUnits.yaml"),
    },
    NamedYaml {
        name: "MyLeaveV2Overview",
        body: include_str!("../openapi/schemas/MyLeaveV2Overview.yaml"),
    },
    NamedYaml {
        name: "SelfLeaveBalance",
        body: include_str!("../openapi/schemas/SelfLeaveBalance.yaml"),
    },
    NamedYaml {
        name: "TimeChangeCoverageEvidence",
        body: include_str!("../openapi/schemas/TimeChangeCoverageEvidence.yaml"),
    },
    NamedYaml {
        name: "TimeChangeGroundsCode",
        body: include_str!("../openapi/schemas/TimeChangeGroundsCode.yaml"),
    },
];
