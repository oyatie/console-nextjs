//! This face's slice of the published OpenAPI contract.
//!
//! Bodies live as YAML under `openapi/` and are pulled in via `include_str!`.
//! `console_contracts` re-indents them; composition rejects duplicate keys.

use console_contracts::{Fragment, NamedYaml, Operation, PathItem};

/// This face's contribution to the composed OpenAPI document.
pub const OPENAPI_FRAGMENT: Fragment = Fragment {
    source: "console-evaluation-rest",
    paths: PATHS,
    schemas: SCHEMAS,
    parameters: &[],
    responses: &[],
    security_schemes: &[],
    external_schemas: EXTERNAL_SCHEMAS,
};

const EXTERNAL_SCHEMAS: &[&str] = &["Date", "Timestamp", "Uuid"];

const PATHS: &[PathItem] = &[
    PathItem {
        path: "/api/v1/evaluation/cycles",
        operations: &[
            Operation {
                method: "get",
                body: include_str!("../openapi/paths/api__v1__evaluation__cycles.get.yaml"),
            },
            Operation {
                method: "post",
                body: include_str!("../openapi/paths/api__v1__evaluation__cycles.post.yaml"),
            },
        ],
    },
    PathItem {
        path: "/api/v1/evaluation/cycles/{cycle_id}",
        operations: &[Operation {
            method: "get",
            body: include_str!("../openapi/paths/api__v1__evaluation__cycles__cycle_id.get.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/evaluation/cycles/{cycle_id}/archive",
        operations: &[Operation {
            method: "post",
            body: include_str!(
                "../openapi/paths/api__v1__evaluation__cycles__cycle_id__archive.post.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/evaluation/cycles/{cycle_id}/finalize",
        operations: &[Operation {
            method: "post",
            body: include_str!(
                "../openapi/paths/api__v1__evaluation__cycles__cycle_id__finalize.post.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/evaluation/cycles/{cycle_id}/open",
        operations: &[Operation {
            method: "post",
            body: include_str!(
                "../openapi/paths/api__v1__evaluation__cycles__cycle_id__open.post.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/evaluation/cycles/{cycle_id}/preflight",
        operations: &[Operation {
            method: "get",
            body: include_str!(
                "../openapi/paths/api__v1__evaluation__cycles__cycle_id__preflight.get.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/evaluation/cycles/{cycle_id}/start-calibration",
        operations: &[Operation {
            method: "post",
            body: include_str!(
                "../openapi/paths/api__v1__evaluation__cycles__cycle_id__start-calibration.post.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/evaluation/employees/{employee_id}/reviews",
        operations: &[Operation {
            method: "get",
            body: include_str!(
                "../openapi/paths/api__v1__evaluation__employees__employee_id__reviews.get.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/evaluation/my-tasks",
        operations: &[Operation {
            method: "get",
            body: include_str!("../openapi/paths/api__v1__evaluation__my-tasks.get.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/evaluation/subjects",
        operations: &[Operation {
            method: "post",
            body: include_str!("../openapi/paths/api__v1__evaluation__subjects.post.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/evaluation/subjects/{subject_id}",
        operations: &[Operation {
            method: "get",
            body: include_str!(
                "../openapi/paths/api__v1__evaluation__subjects__subject_id.get.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/evaluation/subjects/{subject_id}/calibrate",
        operations: &[Operation {
            method: "post",
            body: include_str!(
                "../openapi/paths/api__v1__evaluation__subjects__subject_id__calibrate.post.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/evaluation/subjects/{subject_id}/goals",
        operations: &[Operation {
            method: "put",
            body: include_str!(
                "../openapi/paths/api__v1__evaluation__subjects__subject_id__goals.put.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/evaluation/subjects/{subject_id}/reviews/{kind}",
        operations: &[Operation {
            method: "put",
            body: include_str!(
                "../openapi/paths/api__v1__evaluation__subjects__subject_id__reviews__kind.put.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/evaluation/subjects/{subject_id}/reviews/{kind}/submit",
        operations: &[Operation {
            method: "post",
            body: include_str!(
                "../openapi/paths/api__v1__evaluation__subjects__subject_id__reviews__kind__submit.post.yaml"
            ),
        }],
    },
];

const SCHEMAS: &[NamedYaml] = &[
    NamedYaml {
        name: "AddEvaluationSubjectRequest",
        body: include_str!("../openapi/schemas/AddEvaluationSubjectRequest.yaml"),
    },
    NamedYaml {
        name: "CalibrateEvaluationSubjectRequest",
        body: include_str!("../openapi/schemas/CalibrateEvaluationSubjectRequest.yaml"),
    },
    NamedYaml {
        name: "CreateEvaluationCycleRequest",
        body: include_str!("../openapi/schemas/CreateEvaluationCycleRequest.yaml"),
    },
    NamedYaml {
        name: "EvaluationCycleDetail",
        body: include_str!("../openapi/schemas/EvaluationCycleDetail.yaml"),
    },
    NamedYaml {
        name: "EvaluationCycleKind",
        body: include_str!("../openapi/schemas/EvaluationCycleKind.yaml"),
    },
    NamedYaml {
        name: "EvaluationCyclePage",
        body: include_str!("../openapi/schemas/EvaluationCyclePage.yaml"),
    },
    NamedYaml {
        name: "EvaluationCycleStage",
        body: include_str!("../openapi/schemas/EvaluationCycleStage.yaml"),
    },
    NamedYaml {
        name: "EvaluationCycleSummary",
        body: include_str!("../openapi/schemas/EvaluationCycleSummary.yaml"),
    },
    NamedYaml {
        name: "EvaluationCycleTransition",
        body: include_str!("../openapi/schemas/EvaluationCycleTransition.yaml"),
    },
    NamedYaml {
        name: "EvaluationEvidenceKind",
        body: include_str!("../openapi/schemas/EvaluationEvidenceKind.yaml"),
    },
    NamedYaml {
        name: "EvaluationEvidenceLink",
        body: include_str!("../openapi/schemas/EvaluationEvidenceLink.yaml"),
    },
    NamedYaml {
        name: "EvaluationEvidenceLinkInput",
        body: include_str!("../openapi/schemas/EvaluationEvidenceLinkInput.yaml"),
    },
    NamedYaml {
        name: "EvaluationGoal",
        body: include_str!("../openapi/schemas/EvaluationGoal.yaml"),
    },
    NamedYaml {
        name: "EvaluationGoalInput",
        body: include_str!("../openapi/schemas/EvaluationGoalInput.yaml"),
    },
    NamedYaml {
        name: "EvaluationGrade",
        body: include_str!("../openapi/schemas/EvaluationGrade.yaml"),
    },
    NamedYaml {
        name: "EvaluationLedgerEntry",
        body: include_str!("../openapi/schemas/EvaluationLedgerEntry.yaml"),
    },
    NamedYaml {
        name: "EvaluationLedgerPage",
        body: include_str!("../openapi/schemas/EvaluationLedgerPage.yaml"),
    },
    NamedYaml {
        name: "EvaluationMetricKind",
        body: include_str!("../openapi/schemas/EvaluationMetricKind.yaml"),
    },
    NamedYaml {
        name: "EvaluationPreflightItem",
        body: include_str!("../openapi/schemas/EvaluationPreflightItem.yaml"),
    },
    NamedYaml {
        name: "EvaluationPreflightReport",
        body: include_str!("../openapi/schemas/EvaluationPreflightReport.yaml"),
    },
    NamedYaml {
        name: "EvaluationReview",
        body: include_str!("../openapi/schemas/EvaluationReview.yaml"),
    },
    NamedYaml {
        name: "EvaluationReviewKind",
        body: include_str!("../openapi/schemas/EvaluationReviewKind.yaml"),
    },
    NamedYaml {
        name: "EvaluationReviewStatus",
        body: include_str!("../openapi/schemas/EvaluationReviewStatus.yaml"),
    },
    NamedYaml {
        name: "EvaluationSubjectDetail",
        body: include_str!("../openapi/schemas/EvaluationSubjectDetail.yaml"),
    },
    NamedYaml {
        name: "EvaluationSubjectState",
        body: include_str!("../openapi/schemas/EvaluationSubjectState.yaml"),
    },
    NamedYaml {
        name: "EvaluationSubjectSummary",
        body: include_str!("../openapi/schemas/EvaluationSubjectSummary.yaml"),
    },
    NamedYaml {
        name: "EvaluationTaskItem",
        body: include_str!("../openapi/schemas/EvaluationTaskItem.yaml"),
    },
    NamedYaml {
        name: "EvaluationTaskPage",
        body: include_str!("../openapi/schemas/EvaluationTaskPage.yaml"),
    },
    NamedYaml {
        name: "EvaluationUnitProgress",
        body: include_str!("../openapi/schemas/EvaluationUnitProgress.yaml"),
    },
    NamedYaml {
        name: "ReplaceEvaluationGoalsRequest",
        body: include_str!("../openapi/schemas/ReplaceEvaluationGoalsRequest.yaml"),
    },
    NamedYaml {
        name: "SaveEvaluationReviewRequest",
        body: include_str!("../openapi/schemas/SaveEvaluationReviewRequest.yaml"),
    },
];
