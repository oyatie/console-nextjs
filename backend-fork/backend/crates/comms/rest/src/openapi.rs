//! This face's slice of the published OpenAPI contract.
//!
//! Bodies live as YAML under `openapi/` and are pulled in via `include_str!`.
//! `console_contracts` re-indents them; composition rejects duplicate keys.

use console_contracts::{Fragment, NamedYaml, Operation, PathItem};

/// This face's contribution to the composed OpenAPI document.
pub const OPENAPI_FRAGMENT: Fragment = Fragment {
    source: "console-comms-rest",
    paths: PATHS,
    schemas: SCHEMAS,
    parameters: &[],
    responses: &[],
    security_schemes: &[],
    external_schemas: EXTERNAL_SCHEMAS,
};

const EXTERNAL_SCHEMAS: &[&str] = &[
    "MobilePasskeyStepUpBinding",
    "MobilePasskeyStepUpEnvelope",
    "MobileStepUpActionKind",
    "PasskeyStepUpAssertion",
    "Timestamp",
    "Uuid",
];

const PATHS: &[PathItem] = &[
    PathItem {
        path: "/api/v1/collaboration/calendar/events",
        operations: &[
            Operation {
                method: "get",
                body: include_str!(
                    "../openapi/paths/api__v1__collaboration__calendar__events.get.yaml"
                ),
            },
            Operation {
                method: "post",
                body: include_str!(
                    "../openapi/paths/api__v1__collaboration__calendar__events.post.yaml"
                ),
            },
        ],
    },
    PathItem {
        path: "/api/v1/collaboration/polls",
        operations: &[
            Operation {
                method: "get",
                body: include_str!("../openapi/paths/api__v1__collaboration__polls.get.yaml"),
            },
            Operation {
                method: "post",
                body: include_str!("../openapi/paths/api__v1__collaboration__polls.post.yaml"),
            },
        ],
    },
    PathItem {
        path: "/api/v1/collaboration/polls/{id}/vote",
        operations: &[Operation {
            method: "post",
            body: include_str!(
                "../openapi/paths/api__v1__collaboration__polls__id__vote.post.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/mail/account",
        operations: &[
            Operation {
                method: "get",
                body: include_str!("../openapi/paths/api__v1__mail__account.get.yaml"),
            },
            Operation {
                method: "put",
                body: include_str!("../openapi/paths/api__v1__mail__account.put.yaml"),
            },
        ],
    },
    PathItem {
        path: "/api/v1/mail/account/test",
        operations: &[Operation {
            method: "post",
            body: include_str!("../openapi/paths/api__v1__mail__account__test.post.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/mail/attachments/{id}/download",
        operations: &[Operation {
            method: "get",
            body: include_str!(
                "../openapi/paths/api__v1__mail__attachments__id__download.get.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/mail/folders",
        operations: &[Operation {
            method: "get",
            body: include_str!("../openapi/paths/api__v1__mail__folders.get.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/mail/forward",
        operations: &[Operation {
            method: "post",
            body: include_str!("../openapi/paths/api__v1__mail__forward.post.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/mail/messages/{id}",
        operations: &[Operation {
            method: "get",
            body: include_str!("../openapi/paths/api__v1__mail__messages__id.get.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/mail/reply",
        operations: &[Operation {
            method: "post",
            body: include_str!("../openapi/paths/api__v1__mail__reply.post.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/mail/send",
        operations: &[Operation {
            method: "post",
            body: include_str!("../openapi/paths/api__v1__mail__send.post.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/mail/threads",
        operations: &[Operation {
            method: "get",
            body: include_str!("../openapi/paths/api__v1__mail__threads.get.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/mail/threads/{id}",
        operations: &[Operation {
            method: "get",
            body: include_str!("../openapi/paths/api__v1__mail__threads__id.get.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/mail/threads/{id}/read-state",
        operations: &[Operation {
            method: "patch",
            body: include_str!(
                "../openapi/paths/api__v1__mail__threads__id__read-state.patch.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/mobile/collaboration/polls/{id}/vote",
        operations: &[Operation {
            method: "post",
            body: include_str!(
                "../openapi/paths/api__v1__mobile__collaboration__polls__id__vote.post.yaml"
            ),
        }],
    },
];

const SCHEMAS: &[NamedYaml] = &[
    NamedYaml {
        name: "CalendarEventListResponse",
        body: include_str!("../openapi/schemas/CalendarEventListResponse.yaml"),
    },
    NamedYaml {
        name: "CalendarEventResponse",
        body: include_str!("../openapi/schemas/CalendarEventResponse.yaml"),
    },
    NamedYaml {
        name: "CalendarEventStatus",
        body: include_str!("../openapi/schemas/CalendarEventStatus.yaml"),
    },
    NamedYaml {
        name: "CollaborationScopePolicy",
        body: include_str!("../openapi/schemas/CollaborationScopePolicy.yaml"),
    },
    NamedYaml {
        name: "CollaborationScopeType",
        body: include_str!("../openapi/schemas/CollaborationScopeType.yaml"),
    },
    NamedYaml {
        name: "ConfigureMailAccountRequest",
        body: include_str!("../openapi/schemas/ConfigureMailAccountRequest.yaml"),
    },
    NamedYaml {
        name: "CreateCalendarEventRequest",
        body: include_str!("../openapi/schemas/CreateCalendarEventRequest.yaml"),
    },
    NamedYaml {
        name: "CreatePollRequest",
        body: include_str!("../openapi/schemas/CreatePollRequest.yaml"),
    },
    NamedYaml {
        name: "MailAccountView",
        body: include_str!("../openapi/schemas/MailAccountView.yaml"),
    },
    NamedYaml {
        name: "MailAddress",
        body: include_str!("../openapi/schemas/MailAddress.yaml"),
    },
    NamedYaml {
        name: "MailAttachment",
        body: include_str!("../openapi/schemas/MailAttachment.yaml"),
    },
    NamedYaml {
        name: "MailAttachmentDownload",
        body: include_str!("../openapi/schemas/MailAttachmentDownload.yaml"),
    },
    NamedYaml {
        name: "MailAttachmentView",
        body: include_str!("../openapi/schemas/MailAttachmentView.yaml"),
    },
    NamedYaml {
        name: "MailFolderView",
        body: include_str!("../openapi/schemas/MailFolderView.yaml"),
    },
    NamedYaml {
        name: "MailMessageView",
        body: include_str!("../openapi/schemas/MailMessageView.yaml"),
    },
    NamedYaml {
        name: "MailSecurity",
        body: include_str!("../openapi/schemas/MailSecurity.yaml"),
    },
    NamedYaml {
        name: "MailTestConnectionResult",
        body: include_str!("../openapi/schemas/MailTestConnectionResult.yaml"),
    },
    NamedYaml {
        name: "MailThreadDetail",
        body: include_str!("../openapi/schemas/MailThreadDetail.yaml"),
    },
    NamedYaml {
        name: "MailThreadReadStateRequest",
        body: include_str!("../openapi/schemas/MailThreadReadStateRequest.yaml"),
    },
    NamedYaml {
        name: "MailThreadView",
        body: include_str!("../openapi/schemas/MailThreadView.yaml"),
    },
    NamedYaml {
        name: "MobileVotePollRequest",
        body: include_str!("../openapi/schemas/MobileVotePollRequest.yaml"),
    },
    NamedYaml {
        name: "PollAnonymity",
        body: include_str!("../openapi/schemas/PollAnonymity.yaml"),
    },
    NamedYaml {
        name: "PollListResponse",
        body: include_str!("../openapi/schemas/PollListResponse.yaml"),
    },
    NamedYaml {
        name: "PollMyVote",
        body: include_str!("../openapi/schemas/PollMyVote.yaml"),
    },
    NamedYaml {
        name: "PollOptionResponse",
        body: include_str!("../openapi/schemas/PollOptionResponse.yaml"),
    },
    NamedYaml {
        name: "PollResponse",
        body: include_str!("../openapi/schemas/PollResponse.yaml"),
    },
    NamedYaml {
        name: "PollStatus",
        body: include_str!("../openapi/schemas/PollStatus.yaml"),
    },
    NamedYaml {
        name: "SendMailRequest",
        body: include_str!("../openapi/schemas/SendMailRequest.yaml"),
    },
    NamedYaml {
        name: "SendMailResult",
        body: include_str!("../openapi/schemas/SendMailResult.yaml"),
    },
    NamedYaml {
        name: "VotePollRequest",
        body: include_str!("../openapi/schemas/VotePollRequest.yaml"),
    },
];
