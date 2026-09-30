//! This face's slice of the published OpenAPI contract.
//!
//! Bodies live as YAML under `openapi/` and are pulled in via `include_str!`.
//! `console_contracts` re-indents them; composition rejects duplicate keys.

use console_contracts::{Fragment, NamedYaml, Operation, PathItem};

/// This face's contribution to the composed OpenAPI document.
pub const OPENAPI_FRAGMENT: Fragment = Fragment {
    source: "console-registry-rest",
    paths: PATHS,
    schemas: SCHEMAS,
    parameters: &[],
    responses: &[],
    security_schemes: &[],
    external_schemas: EXTERNAL_SCHEMAS,
};

const EXTERNAL_SCHEMAS: &[&str] = &[
    "Date",
    "EquipmentStatus",
    "ErrorBody",
    "PasskeyStepUpAssertion",
    "Timestamp",
    "Uuid",
];

const PATHS: &[PathItem] = &[
    PathItem {
        path: "/api/v1/approval-inbox/bulk-tasks",
        operations: &[Operation {
            method: "get",
            body: include_str!("../openapi/paths/api__v1__approval-inbox__bulk-tasks.get.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/customers",
        operations: &[Operation {
            method: "post",
            body: include_str!("../openapi/paths/api__v1__customers.post.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/equipment-by-location",
        operations: &[Operation {
            method: "get",
            body: include_str!("../openapi/paths/api__v1__equipment-by-location.get.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/equipment-substitutions",
        operations: &[Operation {
            method: "post",
            body: include_str!("../openapi/paths/api__v1__equipment-substitutions.post.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/equipment-substitutions/{id}/return",
        operations: &[Operation {
            method: "post",
            body: include_str!(
                "../openapi/paths/api__v1__equipment-substitutions__id__return.post.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/equipment/import",
        operations: &[Operation {
            method: "post",
            body: include_str!("../openapi/paths/api__v1__equipment__import.post.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/equipment/list",
        operations: &[Operation {
            method: "get",
            body: include_str!("../openapi/paths/api__v1__equipment__list.get.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/equipment/ownership-transfer-requests/{id}/decisions",
        operations: &[Operation {
            method: "post",
            body: include_str!(
                "../openapi/paths/api__v1__equipment__ownership-transfer-requests__id__decisions.post.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/equipment/{id}",
        operations: &[
            Operation {
                method: "delete",
                body: include_str!("../openapi/paths/api__v1__equipment__id.delete.yaml"),
            },
            Operation {
                method: "get",
                body: include_str!("../openapi/paths/api__v1__equipment__id.get.yaml"),
            },
            Operation {
                method: "patch",
                body: include_str!("../openapi/paths/api__v1__equipment__id.patch.yaml"),
            },
        ],
    },
    PathItem {
        path: "/api/v1/equipment/{id}/ownership-transfer-requests",
        operations: &[
            Operation {
                method: "get",
                body: include_str!(
                    "../openapi/paths/api__v1__equipment__id__ownership-transfer-requests.get.yaml"
                ),
            },
            Operation {
                method: "post",
                body: include_str!(
                    "../openapi/paths/api__v1__equipment__id__ownership-transfer-requests.post.yaml"
                ),
            },
        ],
    },
    PathItem {
        path: "/api/v1/equipment/{id}/substitutes",
        operations: &[Operation {
            method: "get",
            body: include_str!("../openapi/paths/api__v1__equipment__id__substitutes.get.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/equipment/{id}/timeline-graph",
        operations: &[Operation {
            method: "get",
            body: include_str!("../openapi/paths/api__v1__equipment__id__timeline-graph.get.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/equipment/{id}/versions",
        operations: &[Operation {
            method: "get",
            body: include_str!("../openapi/paths/api__v1__equipment__id__versions.get.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/equipment/{id}/versions/{version}/rollback",
        operations: &[Operation {
            method: "post",
            body: include_str!(
                "../openapi/paths/api__v1__equipment__id__versions__version__rollback.post.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/object-actions/catalog",
        operations: &[Operation {
            method: "get",
            body: include_str!("../openapi/paths/api__v1__object-actions__catalog.get.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/object-actions/execute",
        operations: &[Operation {
            method: "post",
            body: include_str!("../openapi/paths/api__v1__object-actions__execute.post.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/sites",
        operations: &[Operation {
            method: "post",
            body: include_str!("../openapi/paths/api__v1__sites.post.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/sites/{id}",
        operations: &[Operation {
            method: "patch",
            body: include_str!("../openapi/paths/api__v1__sites__id.patch.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/workflow-runs",
        operations: &[
            Operation {
                method: "get",
                body: include_str!("../openapi/paths/api__v1__workflow-runs.get.yaml"),
            },
            Operation {
                method: "post",
                body: include_str!("../openapi/paths/api__v1__workflow-runs.post.yaml"),
            },
        ],
    },
    PathItem {
        path: "/api/v1/workflow-runs/for-object",
        operations: &[Operation {
            method: "get",
            body: include_str!("../openapi/paths/api__v1__workflow-runs__for-object.get.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/workflow-runs/mine",
        operations: &[Operation {
            method: "get",
            body: include_str!("../openapi/paths/api__v1__workflow-runs__mine.get.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/workflow-runs/{run_id}",
        operations: &[Operation {
            method: "get",
            body: include_str!("../openapi/paths/api__v1__workflow-runs__run_id.get.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/workflow-runs/{run_id}/post-finalization-rejection",
        operations: &[Operation {
            method: "post",
            body: include_str!(
                "../openapi/paths/api__v1__workflow-runs__run_id__post-finalization-rejection.post.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/workflow-studio/catalog",
        operations: &[Operation {
            method: "get",
            body: include_str!("../openapi/paths/api__v1__workflow-studio__catalog.get.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/workflow-studio/definitions",
        operations: &[
            Operation {
                method: "get",
                body: include_str!(
                    "../openapi/paths/api__v1__workflow-studio__definitions.get.yaml"
                ),
            },
            Operation {
                method: "post",
                body: include_str!(
                    "../openapi/paths/api__v1__workflow-studio__definitions.post.yaml"
                ),
            },
        ],
    },
    PathItem {
        path: "/api/v1/workflow-studio/definitions/by-object-kind/{kind}",
        operations: &[Operation {
            method: "get",
            body: include_str!(
                "../openapi/paths/api__v1__workflow-studio__definitions__by-object-kind__kind.get.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/workflow-studio/definitions/{id}",
        operations: &[
            Operation {
                method: "delete",
                body: include_str!(
                    "../openapi/paths/api__v1__workflow-studio__definitions__id.delete.yaml"
                ),
            },
            Operation {
                method: "patch",
                body: include_str!(
                    "../openapi/paths/api__v1__workflow-studio__definitions__id.patch.yaml"
                ),
            },
        ],
    },
    PathItem {
        path: "/api/v1/workflow-studio/definitions/{id}/clone",
        operations: &[Operation {
            method: "post",
            body: include_str!(
                "../openapi/paths/api__v1__workflow-studio__definitions__id__clone.post.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/workflow-studio/definitions/{id}/history",
        operations: &[Operation {
            method: "get",
            body: include_str!(
                "../openapi/paths/api__v1__workflow-studio__definitions__id__history.get.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/workflow-studio/definitions/{id}/pause",
        operations: &[Operation {
            method: "post",
            body: include_str!(
                "../openapi/paths/api__v1__workflow-studio__definitions__id__pause.post.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/workflow-studio/definitions/{id}/publish",
        operations: &[Operation {
            method: "post",
            body: include_str!(
                "../openapi/paths/api__v1__workflow-studio__definitions__id__publish.post.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/workflow-studio/definitions/{id}/resume",
        operations: &[Operation {
            method: "post",
            body: include_str!(
                "../openapi/paths/api__v1__workflow-studio__definitions__id__resume.post.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/workflow-studio/definitions/{id}/revisions/{rev}/approve",
        operations: &[Operation {
            method: "post",
            body: include_str!(
                "../openapi/paths/api__v1__workflow-studio__definitions__id__revisions__rev__approve.post.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/workflow-studio/definitions/{id}/revisions/{rev}/withdraw",
        operations: &[Operation {
            method: "post",
            body: include_str!(
                "../openapi/paths/api__v1__workflow-studio__definitions__id__revisions__rev__withdraw.post.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/workflow-studio/definitions/{id}/rollback",
        operations: &[Operation {
            method: "post",
            body: include_str!(
                "../openapi/paths/api__v1__workflow-studio__definitions__id__rollback.post.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/workflow-studio/definitions/{id}/run",
        operations: &[Operation {
            method: "post",
            body: include_str!(
                "../openapi/paths/api__v1__workflow-studio__definitions__id__run.post.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/workflow-studio/definitions/{id}/run-log",
        operations: &[Operation {
            method: "get",
            body: include_str!(
                "../openapi/paths/api__v1__workflow-studio__definitions__id__run-log.get.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/workflow-studio/definitions/{id}/simulate",
        operations: &[Operation {
            method: "post",
            body: include_str!(
                "../openapi/paths/api__v1__workflow-studio__definitions__id__simulate.post.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/workflow-studio/schedules",
        operations: &[
            Operation {
                method: "get",
                body: include_str!("../openapi/paths/api__v1__workflow-studio__schedules.get.yaml"),
            },
            Operation {
                method: "post",
                body: include_str!(
                    "../openapi/paths/api__v1__workflow-studio__schedules.post.yaml"
                ),
            },
        ],
    },
    PathItem {
        path: "/api/v1/workflow-studio/schedules/preview-next-runs",
        operations: &[Operation {
            method: "post",
            body: include_str!(
                "../openapi/paths/api__v1__workflow-studio__schedules__preview-next-runs.post.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/workflow-studio/schedules/{id}",
        operations: &[Operation {
            method: "patch",
            body: include_str!(
                "../openapi/paths/api__v1__workflow-studio__schedules__id.patch.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/workflow-studio/schedules/{id}/runs",
        operations: &[Operation {
            method: "get",
            body: include_str!(
                "../openapi/paths/api__v1__workflow-studio__schedules__id__runs.get.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/workflow-studio/submittable-definitions",
        operations: &[Operation {
            method: "get",
            body: include_str!(
                "../openapi/paths/api__v1__workflow-studio__submittable-definitions.get.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/workflow-studio/trigger-bindings",
        operations: &[
            Operation {
                method: "get",
                body: include_str!(
                    "../openapi/paths/api__v1__workflow-studio__trigger-bindings.get.yaml"
                ),
            },
            Operation {
                method: "post",
                body: include_str!(
                    "../openapi/paths/api__v1__workflow-studio__trigger-bindings.post.yaml"
                ),
            },
        ],
    },
    PathItem {
        path: "/api/v1/workflow-studio/trigger-bindings/{id}/disable",
        operations: &[Operation {
            method: "post",
            body: include_str!(
                "../openapi/paths/api__v1__workflow-studio__trigger-bindings__id__disable.post.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/workflow-studio/trigger-bindings/{id}/enable",
        operations: &[Operation {
            method: "post",
            body: include_str!(
                "../openapi/paths/api__v1__workflow-studio__trigger-bindings__id__enable.post.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/workflow-tasks",
        operations: &[Operation {
            method: "get",
            body: include_str!("../openapi/paths/api__v1__workflow-tasks.get.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/workflow-tasks/{task_id}/claim",
        operations: &[Operation {
            method: "post",
            body: include_str!(
                "../openapi/paths/api__v1__workflow-tasks__task_id__claim.post.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/workflow-tasks/{task_id}/decide",
        operations: &[Operation {
            method: "post",
            body: include_str!(
                "../openapi/paths/api__v1__workflow-tasks__task_id__decide.post.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/workflow-tasks/{task_id}/finalize",
        operations: &[Operation {
            method: "post",
            body: include_str!(
                "../openapi/paths/api__v1__workflow-tasks__task_id__finalize.post.yaml"
            ),
        }],
    },
];

const SCHEMAS: &[NamedYaml] = &[
    NamedYaml {
        name: "AdminWorkflowRunListResponse",
        body: include_str!("../openapi/schemas/AdminWorkflowRunListResponse.yaml"),
    },
    NamedYaml {
        name: "AssignSubstituteRequest",
        body: include_str!("../openapi/schemas/AssignSubstituteRequest.yaml"),
    },
    NamedYaml {
        name: "BulkApprovalCapability",
        body: include_str!("../openapi/schemas/BulkApprovalCapability.yaml"),
    },
    NamedYaml {
        name: "BulkApprovalInboxResponse",
        body: include_str!("../openapi/schemas/BulkApprovalInboxResponse.yaml"),
    },
    NamedYaml {
        name: "BulkApprovalTask",
        body: include_str!("../openapi/schemas/BulkApprovalTask.yaml"),
    },
    NamedYaml {
        name: "ClaimWorkflowTaskRequest",
        body: include_str!("../openapi/schemas/ClaimWorkflowTaskRequest.yaml"),
    },
    NamedYaml {
        name: "ClaimWorkflowTaskResponse",
        body: include_str!("../openapi/schemas/ClaimWorkflowTaskResponse.yaml"),
    },
    NamedYaml {
        name: "ClaimedWorkflowTask",
        body: include_str!("../openapi/schemas/ClaimedWorkflowTask.yaml"),
    },
    NamedYaml {
        name: "CloneWorkflowDefinitionRequest",
        body: include_str!("../openapi/schemas/CloneWorkflowDefinitionRequest.yaml"),
    },
    NamedYaml {
        name: "CreateCustomerRequest",
        body: include_str!("../openapi/schemas/CreateCustomerRequest.yaml"),
    },
    NamedYaml {
        name: "CreateOwnershipTransferRequest",
        body: include_str!("../openapi/schemas/CreateOwnershipTransferRequest.yaml"),
    },
    NamedYaml {
        name: "CreateSiteRequest",
        body: include_str!("../openapi/schemas/CreateSiteRequest.yaml"),
    },
    NamedYaml {
        name: "CreateTriggerBindingRequest",
        body: include_str!("../openapi/schemas/CreateTriggerBindingRequest.yaml"),
    },
    NamedYaml {
        name: "CreateWorkflowDefinitionRequest",
        body: include_str!("../openapi/schemas/CreateWorkflowDefinitionRequest.yaml"),
    },
    NamedYaml {
        name: "CreateWorkflowScheduleRequest",
        body: include_str!("../openapi/schemas/CreateWorkflowScheduleRequest.yaml"),
    },
    NamedYaml {
        name: "CreatedCustomer",
        body: include_str!("../openapi/schemas/CreatedCustomer.yaml"),
    },
    NamedYaml {
        name: "CreatedSite",
        body: include_str!("../openapi/schemas/CreatedSite.yaml"),
    },
    NamedYaml {
        name: "DecideOwnershipTransferRequest",
        body: include_str!("../openapi/schemas/DecideOwnershipTransferRequest.yaml"),
    },
    NamedYaml {
        name: "DecideWorkflowTaskRequest",
        body: include_str!("../openapi/schemas/DecideWorkflowTaskRequest.yaml"),
    },
    NamedYaml {
        name: "DecideWorkflowTaskResponse",
        body: include_str!("../openapi/schemas/DecideWorkflowTaskResponse.yaml"),
    },
    NamedYaml {
        name: "DecidedWorkflowTask",
        body: include_str!("../openapi/schemas/DecidedWorkflowTask.yaml"),
    },
    NamedYaml {
        name: "DefinitionsByObjectKindResponse",
        body: include_str!("../openapi/schemas/DefinitionsByObjectKindResponse.yaml"),
    },
    NamedYaml {
        name: "EquipmentByLocationPage",
        body: include_str!("../openapi/schemas/EquipmentByLocationPage.yaml"),
    },
    NamedYaml {
        name: "EquipmentGraphEdge",
        body: include_str!("../openapi/schemas/EquipmentGraphEdge.yaml"),
    },
    NamedYaml {
        name: "EquipmentGraphNode",
        body: include_str!("../openapi/schemas/EquipmentGraphNode.yaml"),
    },
    NamedYaml {
        name: "EquipmentLifecycleEvent",
        body: include_str!("../openapi/schemas/EquipmentLifecycleEvent.yaml"),
    },
    NamedYaml {
        name: "EquipmentListItem",
        body: include_str!("../openapi/schemas/EquipmentListItem.yaml"),
    },
    NamedYaml {
        name: "EquipmentListPage",
        body: include_str!("../openapi/schemas/EquipmentListPage.yaml"),
    },
    NamedYaml {
        name: "EquipmentRelationshipGraph",
        body: include_str!("../openapi/schemas/EquipmentRelationshipGraph.yaml"),
    },
    NamedYaml {
        name: "EquipmentRollbackResult",
        body: include_str!("../openapi/schemas/EquipmentRollbackResult.yaml"),
    },
    NamedYaml {
        name: "EquipmentSortBy",
        body: include_str!("../openapi/schemas/EquipmentSortBy.yaml"),
    },
    NamedYaml {
        name: "EquipmentTimelineEquipment",
        body: include_str!("../openapi/schemas/EquipmentTimelineEquipment.yaml"),
    },
    NamedYaml {
        name: "EquipmentTimelineGraph",
        body: include_str!("../openapi/schemas/EquipmentTimelineGraph.yaml"),
    },
    NamedYaml {
        name: "EquipmentVersion",
        body: include_str!("../openapi/schemas/EquipmentVersion.yaml"),
    },
    NamedYaml {
        name: "EquipmentVersionList",
        body: include_str!("../openapi/schemas/EquipmentVersionList.yaml"),
    },
    NamedYaml {
        name: "ExecuteObjectActionRequest",
        body: include_str!("../openapi/schemas/ExecuteObjectActionRequest.yaml"),
    },
    NamedYaml {
        name: "FinalizeWorkflowTaskRequest",
        body: include_str!("../openapi/schemas/FinalizeWorkflowTaskRequest.yaml"),
    },
    NamedYaml {
        name: "FinalizeWorkflowTaskResponse",
        body: include_str!("../openapi/schemas/FinalizeWorkflowTaskResponse.yaml"),
    },
    NamedYaml {
        name: "FinalizedWorkflowRun",
        body: include_str!("../openapi/schemas/FinalizedWorkflowRun.yaml"),
    },
    NamedYaml {
        name: "FinalizedWorkflowTask",
        body: include_str!("../openapi/schemas/FinalizedWorkflowTask.yaml"),
    },
    NamedYaml {
        name: "ObjectActionCatalogResponse",
        body: include_str!("../openapi/schemas/ObjectActionCatalogResponse.yaml"),
    },
    NamedYaml {
        name: "ObjectActionDescriptor",
        body: include_str!("../openapi/schemas/ObjectActionDescriptor.yaml"),
    },
    NamedYaml {
        name: "ObjectActionExecutionResponse",
        body: include_str!("../openapi/schemas/ObjectActionExecutionResponse.yaml"),
    },
    NamedYaml {
        name: "ObjectActionFieldDescriptor",
        body: include_str!("../openapi/schemas/ObjectActionFieldDescriptor.yaml"),
    },
    NamedYaml {
        name: "ObjectActionFieldOption",
        body: include_str!("../openapi/schemas/ObjectActionFieldOption.yaml"),
    },
    NamedYaml {
        name: "OwnershipTransfer",
        body: include_str!("../openapi/schemas/OwnershipTransfer.yaml"),
    },
    NamedYaml {
        name: "OwnershipTransferPage",
        body: include_str!("../openapi/schemas/OwnershipTransferPage.yaml"),
    },
    NamedYaml {
        name: "OwnershipTransferStep",
        body: include_str!("../openapi/schemas/OwnershipTransferStep.yaml"),
    },
    NamedYaml {
        name: "PostFinalizationRejectionDocument",
        body: include_str!("../openapi/schemas/PostFinalizationRejectionDocument.yaml"),
    },
    NamedYaml {
        name: "PostFinalizationRejectionRequest",
        body: include_str!("../openapi/schemas/PostFinalizationRejectionRequest.yaml"),
    },
    NamedYaml {
        name: "PostFinalizationRejectionResponse",
        body: include_str!("../openapi/schemas/PostFinalizationRejectionResponse.yaml"),
    },
    NamedYaml {
        name: "PreviewScheduleRequest",
        body: include_str!("../openapi/schemas/PreviewScheduleRequest.yaml"),
    },
    NamedYaml {
        name: "PreviewScheduleResponse",
        body: include_str!("../openapi/schemas/PreviewScheduleResponse.yaml"),
    },
    NamedYaml {
        name: "RegistryImportReport",
        body: include_str!("../openapi/schemas/RegistryImportReport.yaml"),
    },
    NamedYaml {
        name: "RegistryRowError",
        body: include_str!("../openapi/schemas/RegistryRowError.yaml"),
    },
    NamedYaml {
        name: "ReturnSubstituteRequest",
        body: include_str!("../openapi/schemas/ReturnSubstituteRequest.yaml"),
    },
    NamedYaml {
        name: "RollbackWorkflowDefinitionRequest",
        body: include_str!("../openapi/schemas/RollbackWorkflowDefinitionRequest.yaml"),
    },
    NamedYaml {
        name: "ScheduleRunItem",
        body: include_str!("../openapi/schemas/ScheduleRunItem.yaml"),
    },
    NamedYaml {
        name: "ScheduleRunListResponse",
        body: include_str!("../openapi/schemas/ScheduleRunListResponse.yaml"),
    },
    NamedYaml {
        name: "SimulateWorkflowDefinitionRequest",
        body: include_str!("../openapi/schemas/SimulateWorkflowDefinitionRequest.yaml"),
    },
    NamedYaml {
        name: "SiteLocationGroup",
        body: include_str!("../openapi/schemas/SiteLocationGroup.yaml"),
    },
    NamedYaml {
        name: "StartWorkflowRunRequest",
        body: include_str!("../openapi/schemas/StartWorkflowRunRequest.yaml"),
    },
    NamedYaml {
        name: "StartWorkflowRunResponse",
        body: include_str!("../openapi/schemas/StartWorkflowRunResponse.yaml"),
    },
    NamedYaml {
        name: "SubmittableDefinitionListResponse",
        body: include_str!("../openapi/schemas/SubmittableDefinitionListResponse.yaml"),
    },
    NamedYaml {
        name: "SubmittableDefinitionResponse",
        body: include_str!("../openapi/schemas/SubmittableDefinitionResponse.yaml"),
    },
    NamedYaml {
        name: "SubstituteAssignment",
        body: include_str!("../openapi/schemas/SubstituteAssignment.yaml"),
    },
    NamedYaml {
        name: "SubstituteCandidate",
        body: include_str!("../openapi/schemas/SubstituteCandidate.yaml"),
    },
    NamedYaml {
        name: "SubstituteCandidatePage",
        body: include_str!("../openapi/schemas/SubstituteCandidatePage.yaml"),
    },
    NamedYaml {
        name: "SubstituteMatchKind",
        body: include_str!("../openapi/schemas/SubstituteMatchKind.yaml"),
    },
    NamedYaml {
        name: "TriggerBindingListResponse",
        body: include_str!("../openapi/schemas/TriggerBindingListResponse.yaml"),
    },
    NamedYaml {
        name: "TriggerBindingResponse",
        body: include_str!("../openapi/schemas/TriggerBindingResponse.yaml"),
    },
    NamedYaml {
        name: "TriggerWorkflowRunRequest",
        body: include_str!("../openapi/schemas/TriggerWorkflowRunRequest.yaml"),
    },
    NamedYaml {
        name: "UpdateEquipmentRequest",
        body: include_str!("../openapi/schemas/UpdateEquipmentRequest.yaml"),
    },
    NamedYaml {
        name: "UpdateSiteRequest",
        body: include_str!("../openapi/schemas/UpdateSiteRequest.yaml"),
    },
    NamedYaml {
        name: "UpdateWorkflowDefinitionRequest",
        body: include_str!("../openapi/schemas/UpdateWorkflowDefinitionRequest.yaml"),
    },
    NamedYaml {
        name: "UpdateWorkflowScheduleRequest",
        body: include_str!("../openapi/schemas/UpdateWorkflowScheduleRequest.yaml"),
    },
    NamedYaml {
        name: "WorkflowActionAllowlistEntry",
        body: include_str!("../openapi/schemas/WorkflowActionAllowlistEntry.yaml"),
    },
    NamedYaml {
        name: "WorkflowConnectorDescriptor",
        body: include_str!("../openapi/schemas/WorkflowConnectorDescriptor.yaml"),
    },
    NamedYaml {
        name: "WorkflowDefinitionEventResponse",
        body: include_str!("../openapi/schemas/WorkflowDefinitionEventResponse.yaml"),
    },
    NamedYaml {
        name: "WorkflowDefinitionHistoryResponse",
        body: include_str!("../openapi/schemas/WorkflowDefinitionHistoryResponse.yaml"),
    },
    NamedYaml {
        name: "WorkflowDefinitionListResponse",
        body: include_str!("../openapi/schemas/WorkflowDefinitionListResponse.yaml"),
    },
    NamedYaml {
        name: "WorkflowDefinitionResponse",
        body: include_str!("../openapi/schemas/WorkflowDefinitionResponse.yaml"),
    },
    NamedYaml {
        name: "WorkflowObjectKind",
        body: include_str!("../openapi/schemas/WorkflowObjectKind.yaml"),
    },
    NamedYaml {
        name: "WorkflowObjectSubject",
        body: include_str!("../openapi/schemas/WorkflowObjectSubject.yaml"),
    },
    NamedYaml {
        name: "WorkflowRunDetailResponse",
        body: include_str!("../openapi/schemas/WorkflowRunDetailResponse.yaml"),
    },
    NamedYaml {
        name: "WorkflowRunDetailRun",
        body: include_str!("../openapi/schemas/WorkflowRunDetailRun.yaml"),
    },
    NamedYaml {
        name: "WorkflowRunDetailTarget",
        body: include_str!("../openapi/schemas/WorkflowRunDetailTarget.yaml"),
    },
    NamedYaml {
        name: "WorkflowRunForObjectSummary",
        body: include_str!("../openapi/schemas/WorkflowRunForObjectSummary.yaml"),
    },
    NamedYaml {
        name: "WorkflowRunListItem",
        body: include_str!("../openapi/schemas/WorkflowRunListItem.yaml"),
    },
    NamedYaml {
        name: "WorkflowRunListResponse",
        body: include_str!("../openapi/schemas/WorkflowRunListResponse.yaml"),
    },
    NamedYaml {
        name: "WorkflowRunLogResponse",
        body: include_str!("../openapi/schemas/WorkflowRunLogResponse.yaml"),
    },
    NamedYaml {
        name: "WorkflowRunResponse",
        body: include_str!("../openapi/schemas/WorkflowRunResponse.yaml"),
    },
    NamedYaml {
        name: "WorkflowRunSummary",
        body: include_str!("../openapi/schemas/WorkflowRunSummary.yaml"),
    },
    NamedYaml {
        name: "WorkflowRunTimelineStep",
        body: include_str!("../openapi/schemas/WorkflowRunTimelineStep.yaml"),
    },
    NamedYaml {
        name: "WorkflowRunsForObjectResponse",
        body: include_str!("../openapi/schemas/WorkflowRunsForObjectResponse.yaml"),
    },
    NamedYaml {
        name: "WorkflowScheduleListResponse",
        body: include_str!("../openapi/schemas/WorkflowScheduleListResponse.yaml"),
    },
    NamedYaml {
        name: "WorkflowScheduleResponse",
        body: include_str!("../openapi/schemas/WorkflowScheduleResponse.yaml"),
    },
    NamedYaml {
        name: "WorkflowSimulationFinding",
        body: include_str!("../openapi/schemas/WorkflowSimulationFinding.yaml"),
    },
    NamedYaml {
        name: "WorkflowSimulationResponse",
        body: include_str!("../openapi/schemas/WorkflowSimulationResponse.yaml"),
    },
    NamedYaml {
        name: "WorkflowStepUpRequest",
        body: include_str!("../openapi/schemas/WorkflowStepUpRequest.yaml"),
    },
    NamedYaml {
        name: "WorkflowStudioCatalogResponse",
        body: include_str!("../openapi/schemas/WorkflowStudioCatalogResponse.yaml"),
    },
    NamedYaml {
        name: "WorkflowTaskListResponse",
        body: include_str!("../openapi/schemas/WorkflowTaskListResponse.yaml"),
    },
    NamedYaml {
        name: "WorkflowTaskSummary",
        body: include_str!("../openapi/schemas/WorkflowTaskSummary.yaml"),
    },
    NamedYaml {
        name: "WorkflowTemplateDescriptor",
        body: include_str!("../openapi/schemas/WorkflowTemplateDescriptor.yaml"),
    },
];
