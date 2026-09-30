//! This face's slice of the published OpenAPI contract.
//!
//! Bodies live as YAML under `openapi/` and are pulled in via `include_str!`.
//! `console_contracts` re-indents them; composition rejects duplicate keys.

use console_contracts::{Fragment, NamedYaml, Operation, PathItem};

/// This face's contribution to the composed OpenAPI document.
pub const OPENAPI_FRAGMENT: Fragment = Fragment {
    source: "console-docs-rest",
    paths: PATHS,
    schemas: SCHEMAS,
    parameters: &[],
    responses: &[],
    security_schemes: &[],
    external_schemas: EXTERNAL_SCHEMAS,
};

const EXTERNAL_SCHEMAS: &[&str] = &["ErrorBody"];

const PATHS: &[PathItem] = &[
    PathItem {
        path: "/api/v1/evidence/objects",
        operations: &[Operation {
            method: "get",
            body: include_str!("../openapi/paths/api__v1__evidence__objects.get.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/evidence/objects/{id}",
        operations: &[Operation {
            method: "get",
            body: include_str!("../openapi/paths/api__v1__evidence__objects__id.get.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/evidence/objects/{id}/hold",
        operations: &[Operation {
            method: "post",
            body: include_str!("../openapi/paths/api__v1__evidence__objects__id__hold.post.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/evidence/objects/{id}/verify",
        operations: &[Operation {
            method: "post",
            body: include_str!("../openapi/paths/api__v1__evidence__objects__id__verify.post.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/office/callback",
        operations: &[Operation {
            method: "post",
            body: include_str!("../openapi/paths/api__v1__office__callback.post.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/office/documents/{documentRef}/versions",
        operations: &[Operation {
            method: "get",
            body: include_str!(
                "../openapi/paths/api__v1__office__documents__documentRef__versions.get.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/office/documents/{documentRef}/versions/{versionNo}/restore",
        operations: &[Operation {
            method: "post",
            body: include_str!(
                "../openapi/paths/api__v1__office__documents__documentRef__versions__versionNo__restore.post.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/office/sessions",
        operations: &[Operation {
            method: "post",
            body: include_str!("../openapi/paths/api__v1__office__sessions.post.yaml"),
        }],
    },
];

const SCHEMAS: &[NamedYaml] = &[
    NamedYaml {
        name: "AdmissibilityStatus",
        body: include_str!("../openapi/schemas/AdmissibilityStatus.yaml"),
    },
    NamedYaml {
        name: "CopyVerification",
        body: include_str!("../openapi/schemas/CopyVerification.yaml"),
    },
    NamedYaml {
        name: "CustodyEventView",
        body: include_str!("../openapi/schemas/CustodyEventView.yaml"),
    },
    NamedYaml {
        name: "CustodyStage",
        body: include_str!("../openapi/schemas/CustodyStage.yaml"),
    },
    NamedYaml {
        name: "DocumentVersion",
        body: include_str!("../openapi/schemas/DocumentVersion.yaml"),
    },
    NamedYaml {
        name: "EvidenceClassification",
        body: include_str!("../openapi/schemas/EvidenceClassification.yaml"),
    },
    NamedYaml {
        name: "EvidenceCopyEvidentiaryStatus",
        body: include_str!("../openapi/schemas/EvidenceCopyEvidentiaryStatus.yaml"),
    },
    NamedYaml {
        name: "EvidenceCopyKind",
        body: include_str!("../openapi/schemas/EvidenceCopyKind.yaml"),
    },
    NamedYaml {
        name: "EvidenceCopyView",
        body: include_str!("../openapi/schemas/EvidenceCopyView.yaml"),
    },
    NamedYaml {
        name: "EvidenceExportView",
        body: include_str!("../openapi/schemas/EvidenceExportView.yaml"),
    },
    NamedYaml {
        name: "EvidenceHoldRequest",
        body: include_str!("../openapi/schemas/EvidenceHoldRequest.yaml"),
    },
    NamedYaml {
        name: "EvidenceObjectDetail",
        body: include_str!("../openapi/schemas/EvidenceObjectDetail.yaml"),
    },
    NamedYaml {
        name: "EvidenceObjectPage",
        body: include_str!("../openapi/schemas/EvidenceObjectPage.yaml"),
    },
    NamedYaml {
        name: "EvidenceObjectView",
        body: include_str!("../openapi/schemas/EvidenceObjectView.yaml"),
    },
    NamedYaml {
        name: "EvidenceSourceRef",
        body: include_str!("../openapi/schemas/EvidenceSourceRef.yaml"),
    },
    NamedYaml {
        name: "EvidenceSourceType",
        body: include_str!("../openapi/schemas/EvidenceSourceType.yaml"),
    },
    NamedYaml {
        name: "EvidenceStorageRef",
        body: include_str!("../openapi/schemas/EvidenceStorageRef.yaml"),
    },
    NamedYaml {
        name: "EvidenceVerifyReport",
        body: include_str!("../openapi/schemas/EvidenceVerifyReport.yaml"),
    },
    NamedYaml {
        name: "FixityStatus",
        body: include_str!("../openapi/schemas/FixityStatus.yaml"),
    },
    NamedYaml {
        name: "LegalHoldRecordView",
        body: include_str!("../openapi/schemas/LegalHoldRecordView.yaml"),
    },
    NamedYaml {
        name: "LegalHoldState",
        body: include_str!("../openapi/schemas/LegalHoldState.yaml"),
    },
    NamedYaml {
        name: "LegalHoldStatus",
        body: include_str!("../openapi/schemas/LegalHoldStatus.yaml"),
    },
    NamedYaml {
        name: "TimestampAuthorityProofView",
        body: include_str!("../openapi/schemas/TimestampAuthorityProofView.yaml"),
    },
    NamedYaml {
        name: "TsaProofStatus",
        body: include_str!("../openapi/schemas/TsaProofStatus.yaml"),
    },
    NamedYaml {
        name: "VerifyOutcome",
        body: include_str!("../openapi/schemas/VerifyOutcome.yaml"),
    },
    NamedYaml {
        name: "WormStorageStatus",
        body: include_str!("../openapi/schemas/WormStorageStatus.yaml"),
    },
];
