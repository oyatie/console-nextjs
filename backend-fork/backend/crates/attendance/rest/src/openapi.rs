//! This face's slice of the published OpenAPI contract.
//!
//! Bodies live as YAML under `openapi/` and are pulled in via `include_str!`.
//! `console_contracts` re-indents them; composition rejects duplicate keys.

use console_contracts::{Fragment, NamedYaml, Operation, PathItem};

/// This face's contribution to the composed OpenAPI document.
pub const OPENAPI_FRAGMENT: Fragment = Fragment {
    source: "console-attendance-rest",
    paths: PATHS,
    schemas: SCHEMAS,
    parameters: &[],
    responses: &[],
    security_schemes: &[],
    external_schemas: EXTERNAL_SCHEMAS,
};

const EXTERNAL_SCHEMAS: &[&str] = &["Uuid"];

const PATHS: &[PathItem] = &[
    PathItem {
        path: "/api/v1/attendance/closes",
        operations: &[
            Operation {
                method: "get",
                body: include_str!("../openapi/paths/api__v1__attendance__closes.get.yaml"),
            },
            Operation {
                method: "post",
                body: include_str!("../openapi/paths/api__v1__attendance__closes.post.yaml"),
            },
        ],
    },
    PathItem {
        path: "/api/v1/attendance/closes/preflight",
        operations: &[Operation {
            method: "post",
            body: include_str!("../openapi/paths/api__v1__attendance__closes__preflight.post.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/attendance/closes/{close_id}/amendments",
        operations: &[Operation {
            method: "post",
            body: include_str!(
                "../openapi/paths/api__v1__attendance__closes__close_id__amendments.post.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/attendance/exceptions",
        operations: &[
            Operation {
                method: "get",
                body: include_str!("../openapi/paths/api__v1__attendance__exceptions.get.yaml"),
            },
            Operation {
                method: "post",
                body: include_str!("../openapi/paths/api__v1__attendance__exceptions.post.yaml"),
            },
        ],
    },
    PathItem {
        path: "/api/v1/attendance/exceptions/{exception_id}",
        operations: &[Operation {
            method: "get",
            body: include_str!(
                "../openapi/paths/api__v1__attendance__exceptions__exception_id.get.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/attendance/exceptions/{exception_id}/resolve",
        operations: &[Operation {
            method: "post",
            body: include_str!(
                "../openapi/paths/api__v1__attendance__exceptions__exception_id__resolve.post.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/attendance/me/exceptions",
        operations: &[Operation {
            method: "get",
            body: include_str!("../openapi/paths/api__v1__attendance__me__exceptions.get.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/attendance/me/week52",
        operations: &[Operation {
            method: "get",
            body: include_str!("../openapi/paths/api__v1__attendance__me__week52.get.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/attendance/substitution-candidates",
        operations: &[Operation {
            method: "get",
            body: include_str!(
                "../openapi/paths/api__v1__attendance__substitution-candidates.get.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/attendance/substitutions",
        operations: &[
            Operation {
                method: "get",
                body: include_str!("../openapi/paths/api__v1__attendance__substitutions.get.yaml"),
            },
            Operation {
                method: "post",
                body: include_str!("../openapi/paths/api__v1__attendance__substitutions.post.yaml"),
            },
        ],
    },
    PathItem {
        path: "/api/v1/attendance/substitutions/{substitution_id}/cancel",
        operations: &[Operation {
            method: "post",
            body: include_str!(
                "../openapi/paths/api__v1__attendance__substitutions__substitution_id__cancel.post.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/attendance/week52",
        operations: &[Operation {
            method: "get",
            body: include_str!("../openapi/paths/api__v1__attendance__week52.get.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/attendance/week52/acks",
        operations: &[Operation {
            method: "post",
            body: include_str!("../openapi/paths/api__v1__attendance__week52__acks.post.yaml"),
        }],
    },
];

const SCHEMAS: &[NamedYaml] = &[
    NamedYaml {
        name: "AssignAttendanceSubstituteRequest",
        body: include_str!("../openapi/schemas/AssignAttendanceSubstituteRequest.yaml"),
    },
    NamedYaml {
        name: "AttendanceCloseAmendment",
        body: include_str!("../openapi/schemas/AttendanceCloseAmendment.yaml"),
    },
    NamedYaml {
        name: "AttendanceCloseAmendmentRequest",
        body: include_str!("../openapi/schemas/AttendanceCloseAmendmentRequest.yaml"),
    },
    NamedYaml {
        name: "AttendanceCloseBoard",
        body: include_str!("../openapi/schemas/AttendanceCloseBoard.yaml"),
    },
    NamedYaml {
        name: "AttendanceCloseCheck",
        body: include_str!("../openapi/schemas/AttendanceCloseCheck.yaml"),
    },
    NamedYaml {
        name: "AttendanceClosePreflight",
        body: include_str!("../openapi/schemas/AttendanceClosePreflight.yaml"),
    },
    NamedYaml {
        name: "AttendanceCloseRequest",
        body: include_str!("../openapi/schemas/AttendanceCloseRequest.yaml"),
    },
    NamedYaml {
        name: "AttendanceException",
        body: include_str!("../openapi/schemas/AttendanceException.yaml"),
    },
    NamedYaml {
        name: "AttendanceExceptionEvidence",
        body: include_str!("../openapi/schemas/AttendanceExceptionEvidence.yaml"),
    },
    NamedYaml {
        name: "AttendanceExceptionLink",
        body: include_str!("../openapi/schemas/AttendanceExceptionLink.yaml"),
    },
    NamedYaml {
        name: "AttendanceExceptionPage",
        body: include_str!("../openapi/schemas/AttendanceExceptionPage.yaml"),
    },
    NamedYaml {
        name: "AttendanceExceptionResolution",
        body: include_str!("../openapi/schemas/AttendanceExceptionResolution.yaml"),
    },
    NamedYaml {
        name: "AttendanceMonthClose",
        body: include_str!("../openapi/schemas/AttendanceMonthClose.yaml"),
    },
    NamedYaml {
        name: "AttendanceMonthCloseItem",
        body: include_str!("../openapi/schemas/AttendanceMonthCloseItem.yaml"),
    },
    NamedYaml {
        name: "AttendanceSubstitution",
        body: include_str!("../openapi/schemas/AttendanceSubstitution.yaml"),
    },
    NamedYaml {
        name: "AttendanceSubstitutionCandidate",
        body: include_str!("../openapi/schemas/AttendanceSubstitutionCandidate.yaml"),
    },
    NamedYaml {
        name: "AttendanceSubstitutionCandidatePage",
        body: include_str!("../openapi/schemas/AttendanceSubstitutionCandidatePage.yaml"),
    },
    NamedYaml {
        name: "AttendanceSubstitutionPage",
        body: include_str!("../openapi/schemas/AttendanceSubstitutionPage.yaml"),
    },
    NamedYaml {
        name: "AttendanceWeek52AckRequest",
        body: include_str!("../openapi/schemas/AttendanceWeek52AckRequest.yaml"),
    },
    NamedYaml {
        name: "AttendanceWeek52Board",
        body: include_str!("../openapi/schemas/AttendanceWeek52Board.yaml"),
    },
    NamedYaml {
        name: "AttendanceWeek52Row",
        body: include_str!("../openapi/schemas/AttendanceWeek52Row.yaml"),
    },
    NamedYaml {
        name: "CancelAttendanceSubstitutionRequest",
        body: include_str!("../openapi/schemas/CancelAttendanceSubstitutionRequest.yaml"),
    },
    NamedYaml {
        name: "OwnAttendanceException",
        body: include_str!("../openapi/schemas/OwnAttendanceException.yaml"),
    },
    NamedYaml {
        name: "OwnAttendanceExceptionPage",
        body: include_str!("../openapi/schemas/OwnAttendanceExceptionPage.yaml"),
    },
    NamedYaml {
        name: "OwnAttendanceExceptionResolution",
        body: include_str!("../openapi/schemas/OwnAttendanceExceptionResolution.yaml"),
    },
    NamedYaml {
        name: "OwnAttendanceWeek52",
        body: include_str!("../openapi/schemas/OwnAttendanceWeek52.yaml"),
    },
    NamedYaml {
        name: "OwnAttendanceWeek52Response",
        body: include_str!("../openapi/schemas/OwnAttendanceWeek52Response.yaml"),
    },
    NamedYaml {
        name: "RaiseAttendanceExceptionRequest",
        body: include_str!("../openapi/schemas/RaiseAttendanceExceptionRequest.yaml"),
    },
    NamedYaml {
        name: "ResolveAttendanceExceptionRequest",
        body: include_str!("../openapi/schemas/ResolveAttendanceExceptionRequest.yaml"),
    },
];
