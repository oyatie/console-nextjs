//! This face's slice of the published OpenAPI contract.
//!
//! Bodies live as YAML under `openapi/` and are pulled in via `include_str!`.
//! `console_contracts` re-indents them; composition rejects duplicate keys.

use console_contracts::{Fragment, NamedYaml, Operation, PathItem};

/// This face's contribution to the composed OpenAPI document.
pub const OPENAPI_FRAGMENT: Fragment = Fragment {
    source: "console-todos-rest",
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
        path: "/api/v1/me/todos",
        operations: &[
            Operation {
                method: "get",
                body: include_str!("../openapi/paths/api__v1__me__todos.get.yaml"),
            },
            Operation {
                method: "post",
                body: include_str!("../openapi/paths/api__v1__me__todos.post.yaml"),
            },
        ],
    },
    PathItem {
        path: "/api/v1/me/todos/{todoId}",
        operations: &[Operation {
            method: "delete",
            body: include_str!("../openapi/paths/api__v1__me__todos__todoId.delete.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/me/todos/{todoId}/done",
        operations: &[Operation {
            method: "post",
            body: include_str!("../openapi/paths/api__v1__me__todos__todoId__done.post.yaml"),
        }],
    },
];

const SCHEMAS: &[NamedYaml] = &[
    NamedYaml {
        name: "CreateTodoRequest",
        body: include_str!("../openapi/schemas/CreateTodoRequest.yaml"),
    },
    NamedYaml {
        name: "SetTodoDoneRequest",
        body: include_str!("../openapi/schemas/SetTodoDoneRequest.yaml"),
    },
    NamedYaml {
        name: "TodoPage",
        body: include_str!("../openapi/schemas/TodoPage.yaml"),
    },
    NamedYaml {
        name: "TodoRef",
        body: include_str!("../openapi/schemas/TodoRef.yaml"),
    },
    NamedYaml {
        name: "TodoSummary",
        body: include_str!("../openapi/schemas/TodoSummary.yaml"),
    },
];
