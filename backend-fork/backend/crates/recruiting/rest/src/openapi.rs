//! This face's slice of the published OpenAPI contract.
//!
//! Bodies live as YAML under `openapi/` and are pulled in via `include_str!`.
//! `console_contracts` re-indents them; composition rejects duplicate keys.

use console_contracts::{Fragment, NamedYaml, Operation, PathItem};

/// This face's contribution to the composed OpenAPI document.
pub const OPENAPI_FRAGMENT: Fragment = Fragment {
    source: "console-recruiting-rest",
    paths: PATHS,
    schemas: SCHEMAS,
    parameters: &[],
    responses: &[],
    security_schemes: &[],
    external_schemas: EXTERNAL_SCHEMAS,
};

const EXTERNAL_SCHEMAS: &[&str] = &["Date", "ErrorBody", "Timestamp", "Uuid"];

const PATHS: &[PathItem] = &[
    PathItem {
        path: "/api/v1/recruiting/applicants/{applicantId}",
        operations: &[Operation {
            method: "get",
            body: include_str!(
                "../openapi/paths/api__v1__recruiting__applicants__applicantId.get.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/recruiting/applicants/{applicantId}/advance",
        operations: &[Operation {
            method: "post",
            body: include_str!(
                "../openapi/paths/api__v1__recruiting__applicants__applicantId__advance.post.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/recruiting/applicants/{applicantId}/assess",
        operations: &[Operation {
            method: "post",
            body: include_str!(
                "../openapi/paths/api__v1__recruiting__applicants__applicantId__assess.post.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/recruiting/applicants/{applicantId}/hire",
        operations: &[Operation {
            method: "post",
            body: include_str!(
                "../openapi/paths/api__v1__recruiting__applicants__applicantId__hire.post.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/recruiting/applicants/{applicantId}/hold",
        operations: &[Operation {
            method: "post",
            body: include_str!(
                "../openapi/paths/api__v1__recruiting__applicants__applicantId__hold.post.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/recruiting/applicants/{applicantId}/offer",
        operations: &[Operation {
            method: "post",
            body: include_str!(
                "../openapi/paths/api__v1__recruiting__applicants__applicantId__offer.post.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/recruiting/applicants/{applicantId}/reinstate",
        operations: &[Operation {
            method: "post",
            body: include_str!(
                "../openapi/paths/api__v1__recruiting__applicants__applicantId__reinstate.post.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/recruiting/applicants/{applicantId}/reject",
        operations: &[Operation {
            method: "post",
            body: include_str!(
                "../openapi/paths/api__v1__recruiting__applicants__applicantId__reject.post.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/recruiting/applicants/{applicantId}/request-documents",
        operations: &[Operation {
            method: "post",
            body: include_str!(
                "../openapi/paths/api__v1__recruiting__applicants__applicantId__request-documents.post.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/recruiting/offers/{offerId}/adjust",
        operations: &[Operation {
            method: "post",
            body: include_str!(
                "../openapi/paths/api__v1__recruiting__offers__offerId__adjust.post.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/recruiting/offers/{offerId}/record-reply",
        operations: &[Operation {
            method: "post",
            body: include_str!(
                "../openapi/paths/api__v1__recruiting__offers__offerId__record-reply.post.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/recruiting/offers/{offerId}/withdraw",
        operations: &[Operation {
            method: "post",
            body: include_str!(
                "../openapi/paths/api__v1__recruiting__offers__offerId__withdraw.post.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/recruiting/postings",
        operations: &[
            Operation {
                method: "get",
                body: include_str!("../openapi/paths/api__v1__recruiting__postings.get.yaml"),
            },
            Operation {
                method: "post",
                body: include_str!("../openapi/paths/api__v1__recruiting__postings.post.yaml"),
            },
        ],
    },
    PathItem {
        path: "/api/v1/recruiting/postings/{postingId}",
        operations: &[
            Operation {
                method: "get",
                body: include_str!(
                    "../openapi/paths/api__v1__recruiting__postings__postingId.get.yaml"
                ),
            },
            Operation {
                method: "put",
                body: include_str!(
                    "../openapi/paths/api__v1__recruiting__postings__postingId.put.yaml"
                ),
            },
        ],
    },
    PathItem {
        path: "/api/v1/recruiting/postings/{postingId}/applicants",
        operations: &[Operation {
            method: "post",
            body: include_str!(
                "../openapi/paths/api__v1__recruiting__postings__postingId__applicants.post.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/recruiting/postings/{postingId}/close",
        operations: &[Operation {
            method: "post",
            body: include_str!(
                "../openapi/paths/api__v1__recruiting__postings__postingId__close.post.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/recruiting/postings/{postingId}/preflight",
        operations: &[Operation {
            method: "post",
            body: include_str!(
                "../openapi/paths/api__v1__recruiting__postings__postingId__preflight.post.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/recruiting/postings/{postingId}/publish",
        operations: &[Operation {
            method: "post",
            body: include_str!(
                "../openapi/paths/api__v1__recruiting__postings__postingId__publish.post.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/recruiting/talent-pool",
        operations: &[Operation {
            method: "get",
            body: include_str!("../openapi/paths/api__v1__recruiting__talent-pool.get.yaml"),
        }],
    },
];

const SCHEMAS: &[NamedYaml] = &[
    NamedYaml {
        name: "AdjustRecruitOfferRequest",
        body: include_str!("../openapi/schemas/AdjustRecruitOfferRequest.yaml"),
    },
    NamedYaml {
        name: "AdvanceRecruitApplicantRequest",
        body: include_str!("../openapi/schemas/AdvanceRecruitApplicantRequest.yaml"),
    },
    NamedYaml {
        name: "AssessRecruitApplicantRequest",
        body: include_str!("../openapi/schemas/AssessRecruitApplicantRequest.yaml"),
    },
    NamedYaml {
        name: "CloseRecruitPostingRequest",
        body: include_str!("../openapi/schemas/CloseRecruitPostingRequest.yaml"),
    },
    NamedYaml {
        name: "CreateRecruitApplicantRequest",
        body: include_str!("../openapi/schemas/CreateRecruitApplicantRequest.yaml"),
    },
    NamedYaml {
        name: "CreateRecruitPostingRequest",
        body: include_str!("../openapi/schemas/CreateRecruitPostingRequest.yaml"),
    },
    NamedYaml {
        name: "ExtendRecruitOfferRequest",
        body: include_str!("../openapi/schemas/ExtendRecruitOfferRequest.yaml"),
    },
    NamedYaml {
        name: "HireRecruitApplicantRequest",
        body: include_str!("../openapi/schemas/HireRecruitApplicantRequest.yaml"),
    },
    NamedYaml {
        name: "HireRecruitApplicantResponse",
        body: include_str!("../openapi/schemas/HireRecruitApplicantResponse.yaml"),
    },
    NamedYaml {
        name: "HoldRecruitApplicantRequest",
        body: include_str!("../openapi/schemas/HoldRecruitApplicantRequest.yaml"),
    },
    NamedYaml {
        name: "PublishRecruitPostingRequest",
        body: include_str!("../openapi/schemas/PublishRecruitPostingRequest.yaml"),
    },
    NamedYaml {
        name: "RecordRecruitOfferReplyRequest",
        body: include_str!("../openapi/schemas/RecordRecruitOfferReplyRequest.yaml"),
    },
    NamedYaml {
        name: "RecruitAmountPeriod",
        body: include_str!("../openapi/schemas/RecruitAmountPeriod.yaml"),
    },
    NamedYaml {
        name: "RecruitApplicant",
        body: include_str!("../openapi/schemas/RecruitApplicant.yaml"),
    },
    NamedYaml {
        name: "RecruitApplicantDetailResponse",
        body: include_str!("../openapi/schemas/RecruitApplicantDetailResponse.yaml"),
    },
    NamedYaml {
        name: "RecruitApplicantStage",
        body: include_str!("../openapi/schemas/RecruitApplicantStage.yaml"),
    },
    NamedYaml {
        name: "RecruitApplicantSummary",
        body: include_str!("../openapi/schemas/RecruitApplicantSummary.yaml"),
    },
    NamedYaml {
        name: "RecruitAssessment",
        body: include_str!("../openapi/schemas/RecruitAssessment.yaml"),
    },
    NamedYaml {
        name: "RecruitAssessmentScore",
        body: include_str!("../openapi/schemas/RecruitAssessmentScore.yaml"),
    },
    NamedYaml {
        name: "RecruitEmploymentType",
        body: include_str!("../openapi/schemas/RecruitEmploymentType.yaml"),
    },
    NamedYaml {
        name: "RecruitHireConflictResponse",
        body: include_str!("../openapi/schemas/RecruitHireConflictResponse.yaml"),
    },
    NamedYaml {
        name: "RecruitOffer",
        body: include_str!("../openapi/schemas/RecruitOffer.yaml"),
    },
    NamedYaml {
        name: "RecruitOfferStatus",
        body: include_str!("../openapi/schemas/RecruitOfferStatus.yaml"),
    },
    NamedYaml {
        name: "RecruitPosting",
        body: include_str!("../openapi/schemas/RecruitPosting.yaml"),
    },
    NamedYaml {
        name: "RecruitPostingDetailResponse",
        body: include_str!("../openapi/schemas/RecruitPostingDetailResponse.yaml"),
    },
    NamedYaml {
        name: "RecruitPostingListResponse",
        body: include_str!("../openapi/schemas/RecruitPostingListResponse.yaml"),
    },
    NamedYaml {
        name: "RecruitPostingPreflightResponse",
        body: include_str!("../openapi/schemas/RecruitPostingPreflightResponse.yaml"),
    },
    NamedYaml {
        name: "RecruitPostingScope",
        body: include_str!("../openapi/schemas/RecruitPostingScope.yaml"),
    },
    NamedYaml {
        name: "RecruitPostingStatus",
        body: include_str!("../openapi/schemas/RecruitPostingStatus.yaml"),
    },
    NamedYaml {
        name: "RecruitPostingSummary",
        body: include_str!("../openapi/schemas/RecruitPostingSummary.yaml"),
    },
    NamedYaml {
        name: "RecruitPreflightCheck",
        body: include_str!("../openapi/schemas/RecruitPreflightCheck.yaml"),
    },
    NamedYaml {
        name: "RecruitPublishFailedResponse",
        body: include_str!("../openapi/schemas/RecruitPublishFailedResponse.yaml"),
    },
    NamedYaml {
        name: "RecruitRejectReason",
        body: include_str!("../openapi/schemas/RecruitRejectReason.yaml"),
    },
    NamedYaml {
        name: "RecruitStageCounts",
        body: include_str!("../openapi/schemas/RecruitStageCounts.yaml"),
    },
    NamedYaml {
        name: "RecruitStageEvent",
        body: include_str!("../openapi/schemas/RecruitStageEvent.yaml"),
    },
    NamedYaml {
        name: "RecruitStageEventAction",
        body: include_str!("../openapi/schemas/RecruitStageEventAction.yaml"),
    },
    NamedYaml {
        name: "RecruitTalentPoolEntry",
        body: include_str!("../openapi/schemas/RecruitTalentPoolEntry.yaml"),
    },
    NamedYaml {
        name: "RecruitTalentPoolListResponse",
        body: include_str!("../openapi/schemas/RecruitTalentPoolListResponse.yaml"),
    },
    NamedYaml {
        name: "RejectRecruitApplicantRequest",
        body: include_str!("../openapi/schemas/RejectRecruitApplicantRequest.yaml"),
    },
    NamedYaml {
        name: "UpdateRecruitPostingRequest",
        body: include_str!("../openapi/schemas/UpdateRecruitPostingRequest.yaml"),
    },
    NamedYaml {
        name: "WithdrawRecruitOfferRequest",
        body: include_str!("../openapi/schemas/WithdrawRecruitOfferRequest.yaml"),
    },
];
