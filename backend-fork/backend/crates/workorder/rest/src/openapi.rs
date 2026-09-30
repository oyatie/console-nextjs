//! This face's slice of the published OpenAPI contract.
//!
//! Bodies live as YAML under `openapi/` and are pulled in via `include_str!`.
//! `console_contracts` re-indents them; composition rejects duplicate keys.

use console_contracts::{Fragment, NamedYaml, Operation, PathItem};

/// This face's contribution to the composed OpenAPI document.
pub const OPENAPI_FRAGMENT: Fragment = Fragment {
    source: "console-workorder-rest",
    paths: PATHS,
    schemas: SCHEMAS,
    parameters: &[],
    responses: &[],
    security_schemes: &[],
    external_schemas: EXTERNAL_SCHEMAS,
};

const EXTERNAL_SCHEMAS: &[&str] = &[
    "Date",
    "EquipmentLookupResponse",
    "ErrorBody",
    "MobilePasskeyStepUpBinding",
    "MobilePasskeyStepUpEnvelope",
    "MobileStepUpActionKind",
    "NamedEntity",
    "PasskeyStepUpAssertion",
    "PresignedUpload",
    "PriorityLevel",
    "Timestamp",
    "Uuid",
    "WorkOrderStatus",
];

const PATHS: &[PathItem] = &[
    PathItem {
        path: "/api/approval-items",
        operations: &[Operation {
            method: "get",
            body: include_str!("../openapi/paths/api__approval-items.get.yaml"),
        }],
    },
    PathItem {
        path: "/api/daily-work-plans",
        operations: &[
            Operation {
                method: "get",
                body: include_str!("../openapi/paths/api__daily-work-plans.get.yaml"),
            },
            Operation {
                method: "post",
                body: include_str!("../openapi/paths/api__daily-work-plans.post.yaml"),
            },
        ],
    },
    PathItem {
        path: "/api/daily-work-plans/{planId}",
        operations: &[Operation {
            method: "get",
            body: include_str!("../openapi/paths/api__daily-work-plans__planId.get.yaml"),
        }],
    },
    PathItem {
        path: "/api/daily-work-plans/{planId}/confirm",
        operations: &[Operation {
            method: "post",
            body: include_str!("../openapi/paths/api__daily-work-plans__planId__confirm.post.yaml"),
        }],
    },
    PathItem {
        path: "/api/daily-work-plans/{planId}/request-review",
        operations: &[Operation {
            method: "post",
            body: include_str!(
                "../openapi/paths/api__daily-work-plans__planId__request-review.post.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/daily-work-plans/{planId}/review",
        operations: &[Operation {
            method: "post",
            body: include_str!("../openapi/paths/api__daily-work-plans__planId__review.post.yaml"),
        }],
    },
    PathItem {
        path: "/api/target-change-requests/{requestId}/review",
        operations: &[Operation {
            method: "post",
            body: include_str!(
                "../openapi/paths/api__target-change-requests__requestId__review.post.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/devices",
        operations: &[Operation {
            method: "post",
            body: include_str!("../openapi/paths/api__v1__devices.post.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/equipment/lookup",
        operations: &[Operation {
            method: "get",
            body: include_str!("../openapi/paths/api__v1__equipment__lookup.get.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/evidence/presign",
        operations: &[Operation {
            method: "post",
            body: include_str!("../openapi/paths/api__v1__evidence__presign.post.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/evidence/staging-presign",
        operations: &[Operation {
            method: "post",
            body: include_str!("../openapi/paths/api__v1__evidence__staging-presign.post.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/evidence/{evidenceId}/confirm",
        operations: &[Operation {
            method: "post",
            body: include_str!("../openapi/paths/api__v1__evidence__evidenceId__confirm.post.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/evidence/{evidenceId}/status",
        operations: &[Operation {
            method: "get",
            body: include_str!("../openapi/paths/api__v1__evidence__evidenceId__status.get.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/mobile/work-orders/{workOrderId}/approve",
        operations: &[Operation {
            method: "post",
            body: include_str!(
                "../openapi/paths/api__v1__mobile__work-orders__workOrderId__approve.post.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/settlements/{settlementId}/review",
        operations: &[Operation {
            method: "post",
            body: include_str!(
                "../openapi/paths/api__v1__settlements__settlementId__review.post.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/settlements/{settlementId}/submit",
        operations: &[Operation {
            method: "post",
            body: include_str!(
                "../openapi/paths/api__v1__settlements__settlementId__submit.post.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/settlements/{settlementId}/void",
        operations: &[Operation {
            method: "post",
            body: include_str!(
                "../openapi/paths/api__v1__settlements__settlementId__void.post.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/sync",
        operations: &[Operation {
            method: "post",
            body: include_str!("../openapi/paths/api__v1__sync.post.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/work-orders",
        operations: &[Operation {
            method: "get",
            body: include_str!("../openapi/paths/api__v1__work-orders.get.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/work-orders/{workOrderId}",
        operations: &[Operation {
            method: "get",
            body: include_str!("../openapi/paths/api__v1__work-orders__workOrderId.get.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/work-orders/{workOrderId}/reject",
        operations: &[Operation {
            method: "post",
            body: include_str!(
                "../openapi/paths/api__v1__work-orders__workOrderId__reject.post.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/work-orders/{workOrderId}/settlement",
        operations: &[
            Operation {
                method: "get",
                body: include_str!(
                    "../openapi/paths/api__v1__work-orders__workOrderId__settlement.get.yaml"
                ),
            },
            Operation {
                method: "post",
                body: include_str!(
                    "../openapi/paths/api__v1__work-orders__workOrderId__settlement.post.yaml"
                ),
            },
        ],
    },
    PathItem {
        path: "/api/work-orders",
        operations: &[Operation {
            method: "post",
            body: include_str!("../openapi/paths/api__work-orders.post.yaml"),
        }],
    },
    PathItem {
        path: "/api/work-orders/{workOrderId}",
        operations: &[
            Operation {
                method: "get",
                body: include_str!("../openapi/paths/api__work-orders__workOrderId.get.yaml"),
            },
            Operation {
                method: "patch",
                body: include_str!("../openapi/paths/api__work-orders__workOrderId.patch.yaml"),
            },
        ],
    },
    PathItem {
        path: "/api/work-orders/{workOrderId}/approve",
        operations: &[Operation {
            method: "post",
            body: include_str!("../openapi/paths/api__work-orders__workOrderId__approve.post.yaml"),
        }],
    },
    PathItem {
        path: "/api/work-orders/{workOrderId}/assignments",
        operations: &[Operation {
            method: "put",
            body: include_str!(
                "../openapi/paths/api__work-orders__workOrderId__assignments.put.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/work-orders/{workOrderId}/outsource-works",
        operations: &[Operation {
            method: "post",
            body: include_str!(
                "../openapi/paths/api__work-orders__workOrderId__outsource-works.post.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/work-orders/{workOrderId}/priority",
        operations: &[Operation {
            method: "patch",
            body: include_str!(
                "../openapi/paths/api__work-orders__workOrderId__priority.patch.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/work-orders/{workOrderId}/report",
        operations: &[Operation {
            method: "post",
            body: include_str!("../openapi/paths/api__work-orders__workOrderId__report.post.yaml"),
        }],
    },
    PathItem {
        path: "/api/work-orders/{workOrderId}/start",
        operations: &[Operation {
            method: "post",
            body: include_str!("../openapi/paths/api__work-orders__workOrderId__start.post.yaml"),
        }],
    },
    PathItem {
        path: "/api/work-orders/{workOrderId}/target-change-requests",
        operations: &[Operation {
            method: "post",
            body: include_str!(
                "../openapi/paths/api__work-orders__workOrderId__target-change-requests.post.yaml"
            ),
        }],
    },
];

const SCHEMAS: &[NamedYaml] = &[
    NamedYaml {
        name: "ApprovalItem",
        body: include_str!("../openapi/schemas/ApprovalItem.yaml"),
    },
    NamedYaml {
        name: "ApprovalItemSource",
        body: include_str!("../openapi/schemas/ApprovalItemSource.yaml"),
    },
    NamedYaml {
        name: "ApprovalItemsPage",
        body: include_str!("../openapi/schemas/ApprovalItemsPage.yaml"),
    },
    NamedYaml {
        name: "ApprovalOntologyContext",
        body: include_str!("../openapi/schemas/ApprovalOntologyContext.yaml"),
    },
    NamedYaml {
        name: "ApprovalPolicyContext",
        body: include_str!("../openapi/schemas/ApprovalPolicyContext.yaml"),
    },
    NamedYaml {
        name: "ApprovalStepSummary",
        body: include_str!("../openapi/schemas/ApprovalStepSummary.yaml"),
    },
    NamedYaml {
        name: "ApprovalWorkflowContext",
        body: include_str!("../openapi/schemas/ApprovalWorkflowContext.yaml"),
    },
    NamedYaml {
        name: "ApproveWorkOrderRequest",
        body: include_str!("../openapi/schemas/ApproveWorkOrderRequest.yaml"),
    },
    NamedYaml {
        name: "AssignWorkOrderRequest",
        body: include_str!("../openapi/schemas/AssignWorkOrderRequest.yaml"),
    },
    NamedYaml {
        name: "AssignmentRole",
        body: include_str!("../openapi/schemas/AssignmentRole.yaml"),
    },
    NamedYaml {
        name: "AssignmentSummary",
        body: include_str!("../openapi/schemas/AssignmentSummary.yaml"),
    },
    NamedYaml {
        name: "AttachmentStage",
        body: include_str!("../openapi/schemas/AttachmentStage.yaml"),
    },
    NamedYaml {
        name: "CreateDailyPlanRequest",
        body: include_str!("../openapi/schemas/CreateDailyPlanRequest.yaml"),
    },
    NamedYaml {
        name: "CreateOutsourceWorkRequest",
        body: include_str!("../openapi/schemas/CreateOutsourceWorkRequest.yaml"),
    },
    NamedYaml {
        name: "CreateSettlementRequest",
        body: include_str!("../openapi/schemas/CreateSettlementRequest.yaml"),
    },
    NamedYaml {
        name: "CreateWorkOrderRequest",
        body: include_str!("../openapi/schemas/CreateWorkOrderRequest.yaml"),
    },
    NamedYaml {
        name: "DailyPlanItemSummary",
        body: include_str!("../openapi/schemas/DailyPlanItemSummary.yaml"),
    },
    NamedYaml {
        name: "DailyPlanListPage",
        body: include_str!("../openapi/schemas/DailyPlanListPage.yaml"),
    },
    NamedYaml {
        name: "DailyPlanStatus",
        body: include_str!("../openapi/schemas/DailyPlanStatus.yaml"),
    },
    NamedYaml {
        name: "DailyPlanSummary",
        body: include_str!("../openapi/schemas/DailyPlanSummary.yaml"),
    },
    NamedYaml {
        name: "DevicePlatform",
        body: include_str!("../openapi/schemas/DevicePlatform.yaml"),
    },
    NamedYaml {
        name: "DeviceRegistrationRequest",
        body: include_str!("../openapi/schemas/DeviceRegistrationRequest.yaml"),
    },
    NamedYaml {
        name: "DeviceRegistrationResponse",
        body: include_str!("../openapi/schemas/DeviceRegistrationResponse.yaml"),
    },
    NamedYaml {
        name: "EquipmentSummary",
        body: include_str!("../openapi/schemas/EquipmentSummary.yaml"),
    },
    NamedYaml {
        name: "EvidenceConfirmResponse",
        body: include_str!("../openapi/schemas/EvidenceConfirmResponse.yaml"),
    },
    NamedYaml {
        name: "EvidencePresignRequest",
        body: include_str!("../openapi/schemas/EvidencePresignRequest.yaml"),
    },
    NamedYaml {
        name: "EvidencePresignResponse",
        body: include_str!("../openapi/schemas/EvidencePresignResponse.yaml"),
    },
    NamedYaml {
        name: "EvidenceStagingPresignRequest",
        body: include_str!("../openapi/schemas/EvidenceStagingPresignRequest.yaml"),
    },
    NamedYaml {
        name: "EvidenceStagingPresignResponse",
        body: include_str!("../openapi/schemas/EvidenceStagingPresignResponse.yaml"),
    },
    NamedYaml {
        name: "EvidenceStatusResponse",
        body: include_str!("../openapi/schemas/EvidenceStatusResponse.yaml"),
    },
    NamedYaml {
        name: "EvidenceSummary",
        body: include_str!("../openapi/schemas/EvidenceSummary.yaml"),
    },
    NamedYaml {
        name: "MaintenanceCause",
        body: include_str!("../openapi/schemas/MaintenanceCause.yaml"),
    },
    NamedYaml {
        name: "MaintenanceType",
        body: include_str!("../openapi/schemas/MaintenanceType.yaml"),
    },
    NamedYaml {
        name: "MediaKind",
        body: include_str!("../openapi/schemas/MediaKind.yaml"),
    },
    NamedYaml {
        name: "MobileApproveWorkOrderRequest",
        body: include_str!("../openapi/schemas/MobileApproveWorkOrderRequest.yaml"),
    },
    NamedYaml {
        name: "OutsourceWorkSummary",
        body: include_str!("../openapi/schemas/OutsourceWorkSummary.yaml"),
    },
    NamedYaml {
        name: "ProcessingStatus",
        body: include_str!("../openapi/schemas/ProcessingStatus.yaml"),
    },
    NamedYaml {
        name: "RejectWorkOrderRequest",
        body: include_str!("../openapi/schemas/RejectWorkOrderRequest.yaml"),
    },
    NamedYaml {
        name: "ReviewDailyPlanRequest",
        body: include_str!("../openapi/schemas/ReviewDailyPlanRequest.yaml"),
    },
    NamedYaml {
        name: "ReviewSettlementRequest",
        body: include_str!("../openapi/schemas/ReviewSettlementRequest.yaml"),
    },
    NamedYaml {
        name: "ReviewTargetChangeRequest",
        body: include_str!("../openapi/schemas/ReviewTargetChangeRequest.yaml"),
    },
    NamedYaml {
        name: "SettlementLineKind",
        body: include_str!("../openapi/schemas/SettlementLineKind.yaml"),
    },
    NamedYaml {
        name: "SettlementLineRequest",
        body: include_str!("../openapi/schemas/SettlementLineRequest.yaml"),
    },
    NamedYaml {
        name: "SettlementLineSummary",
        body: include_str!("../openapi/schemas/SettlementLineSummary.yaml"),
    },
    NamedYaml {
        name: "SettlementStatus",
        body: include_str!("../openapi/schemas/SettlementStatus.yaml"),
    },
    NamedYaml {
        name: "SettlementSummary",
        body: include_str!("../openapi/schemas/SettlementSummary.yaml"),
    },
    NamedYaml {
        name: "SiteContact",
        body: include_str!("../openapi/schemas/SiteContact.yaml"),
    },
    NamedYaml {
        name: "StatusHistorySummary",
        body: include_str!("../openapi/schemas/StatusHistorySummary.yaml"),
    },
    NamedYaml {
        name: "SubmitReportRequest",
        body: include_str!("../openapi/schemas/SubmitReportRequest.yaml"),
    },
    NamedYaml {
        name: "SyncBatchRequest",
        body: include_str!("../openapi/schemas/SyncBatchRequest.yaml"),
    },
    NamedYaml {
        name: "SyncBatchResponse",
        body: include_str!("../openapi/schemas/SyncBatchResponse.yaml"),
    },
    NamedYaml {
        name: "SyncError",
        body: include_str!("../openapi/schemas/SyncError.yaml"),
    },
    NamedYaml {
        name: "SyncOperationKind",
        body: include_str!("../openapi/schemas/SyncOperationKind.yaml"),
    },
    NamedYaml {
        name: "SyncOperationRequest",
        body: include_str!("../openapi/schemas/SyncOperationRequest.yaml"),
    },
    NamedYaml {
        name: "SyncOperationResult",
        body: include_str!("../openapi/schemas/SyncOperationResult.yaml"),
    },
    NamedYaml {
        name: "SyncOperationStatus",
        body: include_str!("../openapi/schemas/SyncOperationStatus.yaml"),
    },
    NamedYaml {
        name: "SyncWorkOrderReportPayload",
        body: include_str!("../openapi/schemas/SyncWorkOrderReportPayload.yaml"),
    },
    NamedYaml {
        name: "SyncWorkOrderStartPayload",
        body: include_str!("../openapi/schemas/SyncWorkOrderStartPayload.yaml"),
    },
    NamedYaml {
        name: "TargetChangeDecision",
        body: include_str!("../openapi/schemas/TargetChangeDecision.yaml"),
    },
    NamedYaml {
        name: "TargetChangeRequest",
        body: include_str!("../openapi/schemas/TargetChangeRequest.yaml"),
    },
    NamedYaml {
        name: "TargetChangeRequestSummary",
        body: include_str!("../openapi/schemas/TargetChangeRequestSummary.yaml"),
    },
    NamedYaml {
        name: "UpdateWorkOrderIntakeRequest",
        body: include_str!("../openapi/schemas/UpdateWorkOrderIntakeRequest.yaml"),
    },
    NamedYaml {
        name: "VoidSettlementRequest",
        body: include_str!("../openapi/schemas/VoidSettlementRequest.yaml"),
    },
    NamedYaml {
        name: "WorkOrderDetail",
        body: include_str!("../openapi/schemas/WorkOrderDetail.yaml"),
    },
    NamedYaml {
        name: "WorkOrderFacetBucket",
        body: include_str!("../openapi/schemas/WorkOrderFacetBucket.yaml"),
    },
    NamedYaml {
        name: "WorkOrderHistogramBucket",
        body: include_str!("../openapi/schemas/WorkOrderHistogramBucket.yaml"),
    },
    NamedYaml {
        name: "WorkOrderLensAggregates",
        body: include_str!("../openapi/schemas/WorkOrderLensAggregates.yaml"),
    },
    NamedYaml {
        name: "WorkOrderLensFacets",
        body: include_str!("../openapi/schemas/WorkOrderLensFacets.yaml"),
    },
    NamedYaml {
        name: "WorkOrderLensHistograms",
        body: include_str!("../openapi/schemas/WorkOrderLensHistograms.yaml"),
    },
    NamedYaml {
        name: "WorkOrderLensListograms",
        body: include_str!("../openapi/schemas/WorkOrderLensListograms.yaml"),
    },
    NamedYaml {
        name: "WorkOrderListItem",
        body: include_str!("../openapi/schemas/WorkOrderListItem.yaml"),
    },
    NamedYaml {
        name: "WorkOrderListPage",
        body: include_str!("../openapi/schemas/WorkOrderListPage.yaml"),
    },
    NamedYaml {
        name: "WorkOrderNamedBucket",
        body: include_str!("../openapi/schemas/WorkOrderNamedBucket.yaml"),
    },
    NamedYaml {
        name: "WorkOrderObjectSetLens",
        body: include_str!("../openapi/schemas/WorkOrderObjectSetLens.yaml"),
    },
    NamedYaml {
        name: "WorkOrderSummary",
        body: include_str!("../openapi/schemas/WorkOrderSummary.yaml"),
    },
    NamedYaml {
        name: "WorkResultType",
        body: include_str!("../openapi/schemas/WorkResultType.yaml"),
    },
    NamedYaml {
        name: "WormReplicaStatus",
        body: include_str!("../openapi/schemas/WormReplicaStatus.yaml"),
    },
];
