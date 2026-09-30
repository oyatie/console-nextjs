//! This face's slice of the published OpenAPI contract.
//!
//! Bodies live as YAML under `openapi/` and are pulled in via `include_str!`.
//! `console_contracts` re-indents them; composition rejects duplicate keys.

use console_contracts::{Fragment, NamedYaml, Operation, PathItem};

/// This face's contribution to the composed OpenAPI document.
pub const OPENAPI_FRAGMENT: Fragment = Fragment {
    source: "console-consulting-rest",
    paths: PATHS,
    schemas: SCHEMAS,
    parameters: &[],
    responses: &[],
    security_schemes: &[],
    external_schemas: EXTERNAL_SCHEMAS,
};

const EXTERNAL_SCHEMAS: &[&str] = &["ErrorBody", "Uuid"];

const PATHS: &[PathItem] = &[
    PathItem {
        path: "/api/v1/consulting/engagements",
        operations: &[
            Operation {
                method: "get",
                body: include_str!("../openapi/paths/api__v1__consulting__engagements.get.yaml"),
            },
            Operation {
                method: "post",
                body: include_str!("../openapi/paths/api__v1__consulting__engagements.post.yaml"),
            },
        ],
    },
    PathItem {
        path: "/api/v1/consulting/engagements/{engagement_id}",
        operations: &[Operation {
            method: "get",
            body: include_str!(
                "../openapi/paths/api__v1__consulting__engagements__engagement_id.get.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/consulting/engagements/{engagement_id}/diagnostics",
        operations: &[Operation {
            method: "post",
            body: include_str!(
                "../openapi/paths/api__v1__consulting__engagements__engagement_id__diagnostics.post.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/consulting/engagements/{engagement_id}/findings",
        operations: &[Operation {
            method: "post",
            body: include_str!(
                "../openapi/paths/api__v1__consulting__engagements__engagement_id__findings.post.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/consulting/engagements/{engagement_id}/history",
        operations: &[Operation {
            method: "get",
            body: include_str!(
                "../openapi/paths/api__v1__consulting__engagements__engagement_id__history.get.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/consulting/engagements/{engagement_id}/initiatives",
        operations: &[Operation {
            method: "post",
            body: include_str!(
                "../openapi/paths/api__v1__consulting__engagements__engagement_id__initiatives.post.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/consulting/engagements/{engagement_id}/observations",
        operations: &[Operation {
            method: "post",
            body: include_str!(
                "../openapi/paths/api__v1__consulting__engagements__engagement_id__observations.post.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/consulting/engagements/{engagement_id}/transition",
        operations: &[Operation {
            method: "post",
            body: include_str!(
                "../openapi/paths/api__v1__consulting__engagements__engagement_id__transition.post.yaml"
            ),
        }],
    },
];

const SCHEMAS: &[NamedYaml] = &[
    NamedYaml {
        name: "ConsultingBenefitObservation",
        body: include_str!("../openapi/schemas/ConsultingBenefitObservation.yaml"),
    },
    NamedYaml {
        name: "ConsultingDiagnostic",
        body: include_str!("../openapi/schemas/ConsultingDiagnostic.yaml"),
    },
    NamedYaml {
        name: "ConsultingDiagnosticCreateRequest",
        body: include_str!("../openapi/schemas/ConsultingDiagnosticCreateRequest.yaml"),
    },
    NamedYaml {
        name: "ConsultingEngagement",
        body: include_str!("../openapi/schemas/ConsultingEngagement.yaml"),
    },
    NamedYaml {
        name: "ConsultingEngagementCreateRequest",
        body: include_str!("../openapi/schemas/ConsultingEngagementCreateRequest.yaml"),
    },
    NamedYaml {
        name: "ConsultingEngagementDetail",
        body: include_str!("../openapi/schemas/ConsultingEngagementDetail.yaml"),
    },
    NamedYaml {
        name: "ConsultingEngagementPage",
        body: include_str!("../openapi/schemas/ConsultingEngagementPage.yaml"),
    },
    NamedYaml {
        name: "ConsultingFinding",
        body: include_str!("../openapi/schemas/ConsultingFinding.yaml"),
    },
    NamedYaml {
        name: "ConsultingFindingCreateRequest",
        body: include_str!("../openapi/schemas/ConsultingFindingCreateRequest.yaml"),
    },
    NamedYaml {
        name: "ConsultingHistoryEntry",
        body: include_str!("../openapi/schemas/ConsultingHistoryEntry.yaml"),
    },
    NamedYaml {
        name: "ConsultingInitiative",
        body: include_str!("../openapi/schemas/ConsultingInitiative.yaml"),
    },
    NamedYaml {
        name: "ConsultingInitiativeCreateRequest",
        body: include_str!("../openapi/schemas/ConsultingInitiativeCreateRequest.yaml"),
    },
    NamedYaml {
        name: "ConsultingObservationCreateRequest",
        body: include_str!("../openapi/schemas/ConsultingObservationCreateRequest.yaml"),
    },
    NamedYaml {
        name: "ConsultingTransitionRequest",
        body: include_str!("../openapi/schemas/ConsultingTransitionRequest.yaml"),
    },
];
