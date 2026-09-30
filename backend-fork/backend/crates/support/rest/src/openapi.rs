//! This face's slice of the published OpenAPI contract.
//!
//! Bodies live as YAML under `openapi/` and are pulled in via `include_str!`.
//! `console_contracts` re-indents them; composition rejects duplicate keys.

use console_contracts::{Fragment, NamedYaml, Operation, PathItem};

/// This face's contribution to the composed OpenAPI document.
pub const OPENAPI_FRAGMENT: Fragment = Fragment {
    source: "console-support-rest",
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
        path: "/api/v1/field/sites",
        operations: &[Operation {
            method: "get",
            body: include_str!("../openapi/paths/api__v1__field__sites.get.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/field/sites/{id}",
        operations: &[Operation {
            method: "get",
            body: include_str!("../openapi/paths/api__v1__field__sites__id.get.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/support/intake",
        operations: &[Operation {
            method: "post",
            body: include_str!("../openapi/paths/api__v1__support__intake.post.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/support/tickets",
        operations: &[
            Operation {
                method: "get",
                body: include_str!("../openapi/paths/api__v1__support__tickets.get.yaml"),
            },
            Operation {
                method: "post",
                body: include_str!("../openapi/paths/api__v1__support__tickets.post.yaml"),
            },
        ],
    },
    PathItem {
        path: "/api/v1/support/tickets/{id}",
        operations: &[Operation {
            method: "get",
            body: include_str!("../openapi/paths/api__v1__support__tickets__id.get.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/support/tickets/{id}/acceptance",
        operations: &[Operation {
            method: "post",
            body: include_str!(
                "../openapi/paths/api__v1__support__tickets__id__acceptance.post.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/support/tickets/{id}/assign",
        operations: &[Operation {
            method: "post",
            body: include_str!("../openapi/paths/api__v1__support__tickets__id__assign.post.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/support/tickets/{id}/comments",
        operations: &[Operation {
            method: "post",
            body: include_str!(
                "../openapi/paths/api__v1__support__tickets__id__comments.post.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/support/tickets/{id}/link",
        operations: &[Operation {
            method: "post",
            body: include_str!("../openapi/paths/api__v1__support__tickets__id__link.post.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/support/tickets/{id}/transition",
        operations: &[Operation {
            method: "post",
            body: include_str!(
                "../openapi/paths/api__v1__support__tickets__id__transition.post.yaml"
            ),
        }],
    },
];

const SCHEMAS: &[NamedYaml] = &[
    NamedYaml {
        name: "AddCommentRequest",
        body: include_str!("../openapi/schemas/AddCommentRequest.yaml"),
    },
    NamedYaml {
        name: "AssignTicketRequest",
        body: include_str!("../openapi/schemas/AssignTicketRequest.yaml"),
    },
    NamedYaml {
        name: "CreateInternalTicketRequest",
        body: include_str!("../openapi/schemas/CreateInternalTicketRequest.yaml"),
    },
    NamedYaml {
        name: "CustomerIntakeRequest",
        body: include_str!("../openapi/schemas/CustomerIntakeRequest.yaml"),
    },
    NamedYaml {
        name: "FieldAttendanceEvent",
        body: include_str!("../openapi/schemas/FieldAttendanceEvent.yaml"),
    },
    NamedYaml {
        name: "FieldSiteDetail",
        body: include_str!("../openapi/schemas/FieldSiteDetail.yaml"),
    },
    NamedYaml {
        name: "FieldSitePage",
        body: include_str!("../openapi/schemas/FieldSitePage.yaml"),
    },
    NamedYaml {
        name: "FieldSiteRow",
        body: include_str!("../openapi/schemas/FieldSiteRow.yaml"),
    },
    NamedYaml {
        name: "FieldSiteSummary",
        body: include_str!("../openapi/schemas/FieldSiteSummary.yaml"),
    },
    NamedYaml {
        name: "FieldSlaState",
        body: include_str!("../openapi/schemas/FieldSlaState.yaml"),
    },
    NamedYaml {
        name: "FieldSlaSummary",
        body: include_str!("../openapi/schemas/FieldSlaSummary.yaml"),
    },
    NamedYaml {
        name: "FieldWorkOrderRef",
        body: include_str!("../openapi/schemas/FieldWorkOrderRef.yaml"),
    },
    NamedYaml {
        name: "LinkSupportTicketRequest",
        body: include_str!("../openapi/schemas/LinkSupportTicketRequest.yaml"),
    },
    NamedYaml {
        name: "RecordSupportTicketAcceptanceRequest",
        body: include_str!("../openapi/schemas/RecordSupportTicketAcceptanceRequest.yaml"),
    },
    NamedYaml {
        name: "SupportIntakeAck",
        body: include_str!("../openapi/schemas/SupportIntakeAck.yaml"),
    },
    NamedYaml {
        name: "SupportTicketAcceptance",
        body: include_str!("../openapi/schemas/SupportTicketAcceptance.yaml"),
    },
    NamedYaml {
        name: "SupportTicketAcceptanceChannel",
        body: include_str!("../openapi/schemas/SupportTicketAcceptanceChannel.yaml"),
    },
    NamedYaml {
        name: "SupportTicketAcceptanceKind",
        body: include_str!("../openapi/schemas/SupportTicketAcceptanceKind.yaml"),
    },
    NamedYaml {
        name: "SupportTicketCategory",
        body: include_str!("../openapi/schemas/SupportTicketCategory.yaml"),
    },
    NamedYaml {
        name: "SupportTicketComment",
        body: include_str!("../openapi/schemas/SupportTicketComment.yaml"),
    },
    NamedYaml {
        name: "SupportTicketDetail",
        body: include_str!("../openapi/schemas/SupportTicketDetail.yaml"),
    },
    NamedYaml {
        name: "SupportTicketOrigin",
        body: include_str!("../openapi/schemas/SupportTicketOrigin.yaml"),
    },
    NamedYaml {
        name: "SupportTicketPage",
        body: include_str!("../openapi/schemas/SupportTicketPage.yaml"),
    },
    NamedYaml {
        name: "SupportTicketPriority",
        body: include_str!("../openapi/schemas/SupportTicketPriority.yaml"),
    },
    NamedYaml {
        name: "SupportTicketStatus",
        body: include_str!("../openapi/schemas/SupportTicketStatus.yaml"),
    },
    NamedYaml {
        name: "SupportTicketSummary",
        body: include_str!("../openapi/schemas/SupportTicketSummary.yaml"),
    },
    NamedYaml {
        name: "TransitionTicketRequest",
        body: include_str!("../openapi/schemas/TransitionTicketRequest.yaml"),
    },
];
