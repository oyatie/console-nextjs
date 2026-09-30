//! This face's slice of the published OpenAPI contract.
//!
//! Bodies live as YAML under `openapi/` and are pulled in via `include_str!`.
//! `console_contracts` re-indents them; composition rejects duplicate keys.

use console_contracts::{Fragment, NamedYaml, Operation, PathItem};

/// This face's contribution to the composed OpenAPI document.
pub const OPENAPI_FRAGMENT: Fragment = Fragment {
    source: "console-financial-rest",
    paths: PATHS,
    schemas: SCHEMAS,
    parameters: &[],
    responses: &[],
    security_schemes: &[],
    external_schemas: EXTERNAL_SCHEMAS,
};

const EXTERNAL_SCHEMAS: &[&str] = &[
    "ErrorBody",
    "PasskeyStepUpAssertion",
    "PresignedUpload",
    "Timestamp",
    "Uuid",
];

const PATHS: &[PathItem] = &[
    PathItem {
        path: "/api/v1/financial/equipment/{equipmentId}/cost-ledger",
        operations: &[Operation {
            method: "get",
            body: include_str!(
                "../openapi/paths/api__v1__financial__equipment__equipmentId__cost-ledger.get.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/financial/equipment/{equipmentId}/cost-ledger/manual",
        operations: &[Operation {
            method: "post",
            body: include_str!(
                "../openapi/paths/api__v1__financial__equipment__equipmentId__cost-ledger__manual.post.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/financial/equipment/{equipmentId}/lifecycle-cost",
        operations: &[Operation {
            method: "get",
            body: include_str!(
                "../openapi/paths/api__v1__financial__equipment__equipmentId__lifecycle-cost.get.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/financial/purchase-requests",
        operations: &[
            Operation {
                method: "get",
                body: include_str!(
                    "../openapi/paths/api__v1__financial__purchase-requests.get.yaml"
                ),
            },
            Operation {
                method: "post",
                body: include_str!(
                    "../openapi/paths/api__v1__financial__purchase-requests.post.yaml"
                ),
            },
        ],
    },
    PathItem {
        path: "/api/v1/financial/purchase-requests/attachments/presign",
        operations: &[Operation {
            method: "post",
            body: include_str!(
                "../openapi/paths/api__v1__financial__purchase-requests__attachments__presign.post.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/financial/purchase-requests/attachments/{attachmentId}/confirm",
        operations: &[Operation {
            method: "post",
            body: include_str!(
                "../openapi/paths/api__v1__financial__purchase-requests__attachments__attachmentId__confirm.post.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/financial/purchase-requests/preferences",
        operations: &[
            Operation {
                method: "get",
                body: include_str!(
                    "../openapi/paths/api__v1__financial__purchase-requests__preferences.get.yaml"
                ),
            },
            Operation {
                method: "put",
                body: include_str!(
                    "../openapi/paths/api__v1__financial__purchase-requests__preferences.put.yaml"
                ),
            },
        ],
    },
    PathItem {
        path: "/api/v1/financial/purchase-requests/{purchaseRequestId}",
        operations: &[Operation {
            method: "get",
            body: include_str!(
                "../openapi/paths/api__v1__financial__purchase-requests__purchaseRequestId.get.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/financial/purchase-requests/{purchaseRequestId}/approve-admin",
        operations: &[Operation {
            method: "post",
            body: include_str!(
                "../openapi/paths/api__v1__financial__purchase-requests__purchaseRequestId__approve-admin.post.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/financial/purchase-requests/{purchaseRequestId}/approve-executive",
        operations: &[Operation {
            method: "post",
            body: include_str!(
                "../openapi/paths/api__v1__financial__purchase-requests__purchaseRequestId__approve-executive.post.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/financial/purchase-requests/{purchaseRequestId}/attachments/{attachmentId}/download",
        operations: &[Operation {
            method: "get",
            body: include_str!(
                "../openapi/paths/api__v1__financial__purchase-requests__purchaseRequestId__attachments__attachmentId__download.get.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/financial/purchase-requests/{purchaseRequestId}/execute",
        operations: &[Operation {
            method: "post",
            body: include_str!(
                "../openapi/paths/api__v1__financial__purchase-requests__purchaseRequestId__execute.post.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/financial/purchase-requests/{purchaseRequestId}/prepare-expenditure",
        operations: &[Operation {
            method: "post",
            body: include_str!(
                "../openapi/paths/api__v1__financial__purchase-requests__purchaseRequestId__prepare-expenditure.post.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/financial/purchase-requests/{purchaseRequestId}/reject",
        operations: &[Operation {
            method: "post",
            body: include_str!(
                "../openapi/paths/api__v1__financial__purchase-requests__purchaseRequestId__reject.post.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/financial/purchase-requests/{purchaseRequestId}/restart",
        operations: &[Operation {
            method: "post",
            body: include_str!(
                "../openapi/paths/api__v1__financial__purchase-requests__purchaseRequestId__restart.post.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/financial/purchase-requests/{purchaseRequestId}/submit",
        operations: &[Operation {
            method: "post",
            body: include_str!(
                "../openapi/paths/api__v1__financial__purchase-requests__purchaseRequestId__submit.post.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/financial/rental-quotes",
        operations: &[Operation {
            method: "post",
            body: include_str!("../openapi/paths/api__v1__financial__rental-quotes.post.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/financial/rental-quotes/compute",
        operations: &[Operation {
            method: "post",
            body: include_str!(
                "../openapi/paths/api__v1__financial__rental-quotes__compute.post.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/financial/rental-quotes/{quoteId}",
        operations: &[Operation {
            method: "get",
            body: include_str!(
                "../openapi/paths/api__v1__financial__rental-quotes__quoteId.get.yaml"
            ),
        }],
    },
];

const SCHEMAS: &[NamedYaml] = &[
    NamedYaml {
        name: "AcquisitionBasis",
        body: include_str!("../openapi/schemas/AcquisitionBasis.yaml"),
    },
    NamedYaml {
        name: "AppendManualCostLedgerRequest",
        body: include_str!("../openapi/schemas/AppendManualCostLedgerRequest.yaml"),
    },
    NamedYaml {
        name: "AssetLifecycleCostSummary",
        body: include_str!("../openapi/schemas/AssetLifecycleCostSummary.yaml"),
    },
    NamedYaml {
        name: "ComputeRentalQuoteRequest",
        body: include_str!("../openapi/schemas/ComputeRentalQuoteRequest.yaml"),
    },
    NamedYaml {
        name: "ComputedRentalQuote",
        body: include_str!("../openapi/schemas/ComputedRentalQuote.yaml"),
    },
    NamedYaml {
        name: "CostLedgerEntrySummary",
        body: include_str!("../openapi/schemas/CostLedgerEntrySummary.yaml"),
    },
    NamedYaml {
        name: "CostLedgerSource",
        body: include_str!("../openapi/schemas/CostLedgerSource.yaml"),
    },
    NamedYaml {
        name: "CreatePurchaseRequest",
        body: include_str!("../openapi/schemas/CreatePurchaseRequest.yaml"),
    },
    NamedYaml {
        name: "CreateRentalQuoteRequest",
        body: include_str!("../openapi/schemas/CreateRentalQuoteRequest.yaml"),
    },
    NamedYaml {
        name: "DepreciationMethod",
        body: include_str!("../openapi/schemas/DepreciationMethod.yaml"),
    },
    NamedYaml {
        name: "FinancialConfigSnapshot",
        body: include_str!("../openapi/schemas/FinancialConfigSnapshot.yaml"),
    },
    NamedYaml {
        name: "FinancialStepUpRequest",
        body: include_str!("../openapi/schemas/FinancialStepUpRequest.yaml"),
    },
    NamedYaml {
        name: "PrepareExpenditureRequest",
        body: include_str!("../openapi/schemas/PrepareExpenditureRequest.yaml"),
    },
    NamedYaml {
        name: "PurchaseAttachmentDownloadResponse",
        body: include_str!("../openapi/schemas/PurchaseAttachmentDownloadResponse.yaml"),
    },
    NamedYaml {
        name: "PurchaseAttachmentPresignRequest",
        body: include_str!("../openapi/schemas/PurchaseAttachmentPresignRequest.yaml"),
    },
    NamedYaml {
        name: "PurchaseAttachmentPresignResponse",
        body: include_str!("../openapi/schemas/PurchaseAttachmentPresignResponse.yaml"),
    },
    NamedYaml {
        name: "PurchaseAttachmentSummary",
        body: include_str!("../openapi/schemas/PurchaseAttachmentSummary.yaml"),
    },
    NamedYaml {
        name: "PurchaseAttachmentUploadRecord",
        body: include_str!("../openapi/schemas/PurchaseAttachmentUploadRecord.yaml"),
    },
    NamedYaml {
        name: "PurchaseFeaturePreferences",
        body: include_str!("../openapi/schemas/PurchaseFeaturePreferences.yaml"),
    },
    NamedYaml {
        name: "PurchasePolicySummary",
        body: include_str!("../openapi/schemas/PurchasePolicySummary.yaml"),
    },
    NamedYaml {
        name: "PurchaseRequestLineInput",
        body: include_str!("../openapi/schemas/PurchaseRequestLineInput.yaml"),
    },
    NamedYaml {
        name: "PurchaseRequestLineSummary",
        body: include_str!("../openapi/schemas/PurchaseRequestLineSummary.yaml"),
    },
    NamedYaml {
        name: "PurchaseRequestPage",
        body: include_str!("../openapi/schemas/PurchaseRequestPage.yaml"),
    },
    NamedYaml {
        name: "PurchaseRequestSummary",
        body: include_str!("../openapi/schemas/PurchaseRequestSummary.yaml"),
    },
    NamedYaml {
        name: "PurchaseRequesterSummary",
        body: include_str!("../openapi/schemas/PurchaseRequesterSummary.yaml"),
    },
    NamedYaml {
        name: "PurchaseStatus",
        body: include_str!("../openapi/schemas/PurchaseStatus.yaml"),
    },
    NamedYaml {
        name: "PurchaseType",
        body: include_str!("../openapi/schemas/PurchaseType.yaml"),
    },
    NamedYaml {
        name: "QuoteLine",
        body: include_str!("../openapi/schemas/QuoteLine.yaml"),
    },
    NamedYaml {
        name: "RejectPurchaseRequest",
        body: include_str!("../openapi/schemas/RejectPurchaseRequest.yaml"),
    },
    NamedYaml {
        name: "RentalQuoteSummary",
        body: include_str!("../openapi/schemas/RentalQuoteSummary.yaml"),
    },
    NamedYaml {
        name: "RestartPurchaseRequest",
        body: include_str!("../openapi/schemas/RestartPurchaseRequest.yaml"),
    },
    NamedYaml {
        name: "SavePurchasePreferencesRequest",
        body: include_str!("../openapi/schemas/SavePurchasePreferencesRequest.yaml"),
    },
];
