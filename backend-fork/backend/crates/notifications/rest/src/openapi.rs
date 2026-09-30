//! This face's slice of the published OpenAPI contract.
//!
//! Bodies live as YAML under `openapi/` and are pulled in via `include_str!`.
//! `console_contracts` re-indents them; composition rejects duplicate keys.

use console_contracts::{Fragment, NamedYaml, Operation, PathItem};

/// This face's contribution to the composed OpenAPI document.
pub const OPENAPI_FRAGMENT: Fragment = Fragment {
    source: "console-notifications-rest",
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
        path: "/api/v1/me/notification-policies",
        operations: &[
            Operation {
                method: "get",
                body: include_str!("../openapi/paths/api__v1__me__notification-policies.get.yaml"),
            },
            Operation {
                method: "put",
                body: include_str!("../openapi/paths/api__v1__me__notification-policies.put.yaml"),
            },
        ],
    },
    PathItem {
        path: "/api/v1/me/notification-policies/{id}",
        operations: &[Operation {
            method: "delete",
            body: include_str!(
                "../openapi/paths/api__v1__me__notification-policies__id.delete.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/me/notifications",
        operations: &[Operation {
            method: "get",
            body: include_str!("../openapi/paths/api__v1__me__notifications.get.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/me/notifications/by-object",
        operations: &[Operation {
            method: "get",
            body: include_str!("../openapi/paths/api__v1__me__notifications__by-object.get.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/me/notifications/read-all",
        operations: &[Operation {
            method: "post",
            body: include_str!("../openapi/paths/api__v1__me__notifications__read-all.post.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/me/notifications/summary",
        operations: &[Operation {
            method: "get",
            body: include_str!("../openapi/paths/api__v1__me__notifications__summary.get.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/me/notifications/unread-count",
        operations: &[Operation {
            method: "get",
            body: include_str!(
                "../openapi/paths/api__v1__me__notifications__unread-count.get.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/me/notifications/{id}/read",
        operations: &[Operation {
            method: "post",
            body: include_str!("../openapi/paths/api__v1__me__notifications__id__read.post.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/me/notifications/{id}/unread",
        operations: &[Operation {
            method: "post",
            body: include_str!("../openapi/paths/api__v1__me__notifications__id__unread.post.yaml"),
        }],
    },
];

const SCHEMAS: &[NamedYaml] = &[
    NamedYaml {
        name: "NotificationCategoryCount",
        body: include_str!("../openapi/schemas/NotificationCategoryCount.yaml"),
    },
    NamedYaml {
        name: "NotificationCountsSummary",
        body: include_str!("../openapi/schemas/NotificationCountsSummary.yaml"),
    },
    NamedYaml {
        name: "NotificationLink",
        body: include_str!("../openapi/schemas/NotificationLink.yaml"),
    },
    NamedYaml {
        name: "NotificationObjectGroup",
        body: include_str!("../openapi/schemas/NotificationObjectGroup.yaml"),
    },
    NamedYaml {
        name: "NotificationObjectGroupPage",
        body: include_str!("../openapi/schemas/NotificationObjectGroupPage.yaml"),
    },
    NamedYaml {
        name: "NotificationPage",
        body: include_str!("../openapi/schemas/NotificationPage.yaml"),
    },
    NamedYaml {
        name: "NotificationPolicyList",
        body: include_str!("../openapi/schemas/NotificationPolicyList.yaml"),
    },
    NamedYaml {
        name: "NotificationPolicySummary",
        body: include_str!("../openapi/schemas/NotificationPolicySummary.yaml"),
    },
    NamedYaml {
        name: "NotificationReadAllResponse",
        body: include_str!("../openapi/schemas/NotificationReadAllResponse.yaml"),
    },
    NamedYaml {
        name: "NotificationSummary",
        body: include_str!("../openapi/schemas/NotificationSummary.yaml"),
    },
    NamedYaml {
        name: "UnreadNotificationCountResponse",
        body: include_str!("../openapi/schemas/UnreadNotificationCountResponse.yaml"),
    },
    NamedYaml {
        name: "UpsertNotificationPolicyRequest",
        body: include_str!("../openapi/schemas/UpsertNotificationPolicyRequest.yaml"),
    },
];
