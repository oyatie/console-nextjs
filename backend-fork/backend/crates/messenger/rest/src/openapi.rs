//! This face's slice of the published OpenAPI contract.
//!
//! Bodies live as YAML under `openapi/` and are pulled in via `include_str!`.
//! `console_contracts` re-indents them; composition rejects duplicate keys.

use console_contracts::{Fragment, NamedYaml, Operation, PathItem};

/// This face's contribution to the composed OpenAPI document.
pub const OPENAPI_FRAGMENT: Fragment = Fragment {
    source: "console-messenger-rest",
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
        path: "/api/messenger/channels",
        operations: &[Operation {
            method: "get",
            body: include_str!("../openapi/paths/api__messenger__channels.get.yaml"),
        }],
    },
    PathItem {
        path: "/api/messenger/members",
        operations: &[Operation {
            method: "get",
            body: include_str!("../openapi/paths/api__messenger__members.get.yaml"),
        }],
    },
    PathItem {
        path: "/api/messenger/members/{userId}",
        operations: &[Operation {
            method: "get",
            body: include_str!("../openapi/paths/api__messenger__members__userId.get.yaml"),
        }],
    },
    PathItem {
        path: "/api/messenger/messages/{messageId}/ack",
        operations: &[Operation {
            method: "post",
            body: include_str!(
                "../openapi/paths/api__messenger__messages__messageId__ack.post.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/messenger/search",
        operations: &[Operation {
            method: "get",
            body: include_str!("../openapi/paths/api__messenger__search.get.yaml"),
        }],
    },
    PathItem {
        path: "/api/messenger/threads",
        operations: &[
            Operation {
                method: "get",
                body: include_str!("../openapi/paths/api__messenger__threads.get.yaml"),
            },
            Operation {
                method: "post",
                body: include_str!("../openapi/paths/api__messenger__threads.post.yaml"),
            },
        ],
    },
    PathItem {
        path: "/api/messenger/threads/{threadId}/join",
        operations: &[Operation {
            method: "post",
            body: include_str!(
                "../openapi/paths/api__messenger__threads__threadId__join.post.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/messenger/threads/{threadId}/messages",
        operations: &[
            Operation {
                method: "get",
                body: include_str!(
                    "../openapi/paths/api__messenger__threads__threadId__messages.get.yaml"
                ),
            },
            Operation {
                method: "post",
                body: include_str!(
                    "../openapi/paths/api__messenger__threads__threadId__messages.post.yaml"
                ),
            },
        ],
    },
    PathItem {
        path: "/api/messenger/threads/{threadId}/mute",
        operations: &[Operation {
            method: "put",
            body: include_str!("../openapi/paths/api__messenger__threads__threadId__mute.put.yaml"),
        }],
    },
    PathItem {
        path: "/api/messenger/threads/{threadId}/presence",
        operations: &[Operation {
            method: "get",
            body: include_str!(
                "../openapi/paths/api__messenger__threads__threadId__presence.get.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/messenger/threads/{threadId}/read-receipt",
        operations: &[Operation {
            method: "put",
            body: include_str!(
                "../openapi/paths/api__messenger__threads__threadId__read-receipt.put.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/ws",
        operations: &[Operation {
            method: "get",
            body: include_str!("../openapi/paths/api__v1__ws.get.yaml"),
        }],
    },
];

const SCHEMAS: &[NamedYaml] = &[
    NamedYaml {
        name: "CreateMessengerThreadRequest",
        body: include_str!("../openapi/schemas/CreateMessengerThreadRequest.yaml"),
    },
    NamedYaml {
        name: "MarkMessengerThreadReadRequest",
        body: include_str!("../openapi/schemas/MarkMessengerThreadReadRequest.yaml"),
    },
    NamedYaml {
        name: "MessengerAckSummary",
        body: include_str!("../openapi/schemas/MessengerAckSummary.yaml"),
    },
    NamedYaml {
        name: "MessengerMemberListResponse",
        body: include_str!("../openapi/schemas/MessengerMemberListResponse.yaml"),
    },
    NamedYaml {
        name: "MessengerMemberPresence",
        body: include_str!("../openapi/schemas/MessengerMemberPresence.yaml"),
    },
    NamedYaml {
        name: "MessengerMemberPresenceListResponse",
        body: include_str!("../openapi/schemas/MessengerMemberPresenceListResponse.yaml"),
    },
    NamedYaml {
        name: "MessengerMemberSummary",
        body: include_str!("../openapi/schemas/MessengerMemberSummary.yaml"),
    },
    NamedYaml {
        name: "MessengerMessageListResponse",
        body: include_str!("../openapi/schemas/MessengerMessageListResponse.yaml"),
    },
    NamedYaml {
        name: "MessengerMessagePage",
        body: include_str!("../openapi/schemas/MessengerMessagePage.yaml"),
    },
    NamedYaml {
        name: "MessengerMessageSummary",
        body: include_str!("../openapi/schemas/MessengerMessageSummary.yaml"),
    },
    NamedYaml {
        name: "MessengerPresenceStatus",
        body: include_str!("../openapi/schemas/MessengerPresenceStatus.yaml"),
    },
    NamedYaml {
        name: "MessengerReadReceiptSummary",
        body: include_str!("../openapi/schemas/MessengerReadReceiptSummary.yaml"),
    },
    NamedYaml {
        name: "MessengerThreadKind",
        body: include_str!("../openapi/schemas/MessengerThreadKind.yaml"),
    },
    NamedYaml {
        name: "MessengerThreadListResponse",
        body: include_str!("../openapi/schemas/MessengerThreadListResponse.yaml"),
    },
    NamedYaml {
        name: "MessengerThreadMuteSummary",
        body: include_str!("../openapi/schemas/MessengerThreadMuteSummary.yaml"),
    },
    NamedYaml {
        name: "MessengerThreadSummary",
        body: include_str!("../openapi/schemas/MessengerThreadSummary.yaml"),
    },
    NamedYaml {
        name: "MessengerThreadVisibility",
        body: include_str!("../openapi/schemas/MessengerThreadVisibility.yaml"),
    },
    NamedYaml {
        name: "SendMessengerMessageRequest",
        body: include_str!("../openapi/schemas/SendMessengerMessageRequest.yaml"),
    },
    NamedYaml {
        name: "SetMessengerThreadMuteRequest",
        body: include_str!("../openapi/schemas/SetMessengerThreadMuteRequest.yaml"),
    },
];
