//! This face's slice of the published OpenAPI contract.
//!
//! Bodies live as YAML under `openapi/` and are pulled in via `include_str!`.
//! `console_contracts` re-indents them; composition rejects duplicate keys.

use console_contracts::{Fragment, NamedYaml, Operation, PathItem};

/// This face's contribution to the composed OpenAPI document.
pub const OPENAPI_FRAGMENT: Fragment = Fragment {
    source: "console-orgchange-rest",
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
        path: "/api/v1/employees",
        operations: &[
            Operation {
                method: "get",
                body: include_str!("../openapi/paths/api__v1__employees.get.yaml"),
            },
            Operation {
                method: "post",
                body: include_str!("../openapi/paths/api__v1__employees.post.yaml"),
            },
        ],
    },
    PathItem {
        path: "/api/v1/employees/export.csv",
        operations: &[Operation {
            method: "get",
            body: include_str!("../openapi/paths/api__v1__employees__export.csv.get.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/employees/import",
        operations: &[Operation {
            method: "post",
            body: include_str!("../openapi/paths/api__v1__employees__import.post.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/employees/import/preview",
        operations: &[Operation {
            method: "post",
            body: include_str!("../openapi/paths/api__v1__employees__import__preview.post.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/employees/import/{run_id}/apply",
        operations: &[Operation {
            method: "post",
            body: include_str!(
                "../openapi/paths/api__v1__employees__import__run_id__apply.post.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/employees/import/{run_id}/dry-run",
        operations: &[Operation {
            method: "post",
            body: include_str!(
                "../openapi/paths/api__v1__employees__import__run_id__dry-run.post.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/employees/{id}",
        operations: &[Operation {
            method: "get",
            body: include_str!("../openapi/paths/api__v1__employees__id.get.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/employees/{id}/home-branch",
        operations: &[Operation {
            method: "put",
            body: include_str!("../openapi/paths/api__v1__employees__id__home-branch.put.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/employees/{id}/lifecycle-events",
        operations: &[
            Operation {
                method: "get",
                body: include_str!(
                    "../openapi/paths/api__v1__employees__id__lifecycle-events.get.yaml"
                ),
            },
            Operation {
                method: "post",
                body: include_str!(
                    "../openapi/paths/api__v1__employees__id__lifecycle-events.post.yaml"
                ),
            },
        ],
    },
    PathItem {
        path: "/api/v1/hr/absence-exit-dashboard",
        operations: &[Operation {
            method: "get",
            body: include_str!("../openapi/paths/api__v1__hr__absence-exit-dashboard.get.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/hr/attendance-import/preview",
        operations: &[Operation {
            method: "post",
            body: include_str!(
                "../openapi/paths/api__v1__hr__attendance-import__preview.post.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/hr/attendance-import/summary",
        operations: &[Operation {
            method: "get",
            body: include_str!("../openapi/paths/api__v1__hr__attendance-import__summary.get.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/hr/attendance-import/{run_id}/apply",
        operations: &[Operation {
            method: "post",
            body: include_str!(
                "../openapi/paths/api__v1__hr__attendance-import__run_id__apply.post.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/hr/attendance-import/{run_id}/dry-run",
        operations: &[Operation {
            method: "post",
            body: include_str!(
                "../openapi/paths/api__v1__hr__attendance-import__run_id__dry-run.post.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/hr/attendance-records",
        operations: &[Operation {
            method: "get",
            body: include_str!("../openapi/paths/api__v1__hr__attendance-records.get.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/hr/attendance-records/me",
        operations: &[
            Operation {
                method: "get",
                body: include_str!("../openapi/paths/api__v1__hr__attendance-records__me.get.yaml"),
            },
            Operation {
                method: "post",
                body: include_str!(
                    "../openapi/paths/api__v1__hr__attendance-records__me.post.yaml"
                ),
            },
        ],
    },
    PathItem {
        path: "/api/v1/hr/attendance-summary",
        operations: &[Operation {
            method: "get",
            body: include_str!("../openapi/paths/api__v1__hr__attendance-summary.get.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/hr/exit-cases",
        operations: &[Operation {
            method: "post",
            body: include_str!("../openapi/paths/api__v1__hr__exit-cases.post.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/hr/exit-cases/{id}/approval-draft",
        operations: &[Operation {
            method: "post",
            body: include_str!(
                "../openapi/paths/api__v1__hr__exit-cases__id__approval-draft.post.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/hr/exit-cases/{id}/confirm",
        operations: &[Operation {
            method: "post",
            body: include_str!("../openapi/paths/api__v1__hr__exit-cases__id__confirm.post.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/hr/leave-balances",
        operations: &[Operation {
            method: "get",
            body: include_str!("../openapi/paths/api__v1__hr__leave-balances.get.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/hr/org-chart",
        operations: &[Operation {
            method: "get",
            body: include_str!("../openapi/paths/api__v1__hr__org-chart.get.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/hr/readiness-summary",
        operations: &[Operation {
            method: "get",
            body: include_str!("../openapi/paths/api__v1__hr__readiness-summary.get.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/org-changes",
        operations: &[
            Operation {
                method: "get",
                body: include_str!("../openapi/paths/api__v1__org-changes.get.yaml"),
            },
            Operation {
                method: "post",
                body: include_str!("../openapi/paths/api__v1__org-changes.post.yaml"),
            },
        ],
    },
    PathItem {
        path: "/api/v1/org-changes/{id}",
        operations: &[
            Operation {
                method: "get",
                body: include_str!("../openapi/paths/api__v1__org-changes__id.get.yaml"),
            },
            Operation {
                method: "patch",
                body: include_str!("../openapi/paths/api__v1__org-changes__id.patch.yaml"),
            },
        ],
    },
    PathItem {
        path: "/api/v1/org-changes/{id}/approval-steps/{stepId}/decision",
        operations: &[Operation {
            method: "post",
            body: include_str!(
                "../openapi/paths/api__v1__org-changes__id__approval-steps__stepId__decision.post.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/org-changes/{id}/archive",
        operations: &[Operation {
            method: "post",
            body: include_str!("../openapi/paths/api__v1__org-changes__id__archive.post.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/org-changes/{id}/cancel",
        operations: &[Operation {
            method: "post",
            body: include_str!("../openapi/paths/api__v1__org-changes__id__cancel.post.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/org-changes/{id}/effectuate",
        operations: &[Operation {
            method: "post",
            body: include_str!("../openapi/paths/api__v1__org-changes__id__effectuate.post.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/org-changes/{id}/preflight",
        operations: &[Operation {
            method: "post",
            body: include_str!("../openapi/paths/api__v1__org-changes__id__preflight.post.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/org-changes/{id}/settlement-items/{itemId}/complete",
        operations: &[Operation {
            method: "post",
            body: include_str!(
                "../openapi/paths/api__v1__org-changes__id__settlement-items__itemId__complete.post.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/org-changes/{id}/submit",
        operations: &[Operation {
            method: "post",
            body: include_str!("../openapi/paths/api__v1__org-changes__id__submit.post.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/org-entities",
        operations: &[Operation {
            method: "get",
            body: include_str!("../openapi/paths/api__v1__org-entities.get.yaml"),
        }],
    },
];

const SCHEMAS: &[NamedYaml] = &[
    NamedYaml {
        name: "AttendanceImportApplyReport",
        body: include_str!("../openapi/schemas/AttendanceImportApplyReport.yaml"),
    },
    NamedYaml {
        name: "AttendanceImportColumn",
        body: include_str!("../openapi/schemas/AttendanceImportColumn.yaml"),
    },
    NamedYaml {
        name: "AttendanceImportDryRunSummary",
        body: include_str!("../openapi/schemas/AttendanceImportDryRunSummary.yaml"),
    },
    NamedYaml {
        name: "AttendanceImportPreviewResponse",
        body: include_str!("../openapi/schemas/AttendanceImportPreviewResponse.yaml"),
    },
    NamedYaml {
        name: "AttendanceImportPreviewRow",
        body: include_str!("../openapi/schemas/AttendanceImportPreviewRow.yaml"),
    },
    NamedYaml {
        name: "AttendanceImportRowError",
        body: include_str!("../openapi/schemas/AttendanceImportRowError.yaml"),
    },
    NamedYaml {
        name: "AttendanceImportSummaryItem",
        body: include_str!("../openapi/schemas/AttendanceImportSummaryItem.yaml"),
    },
    NamedYaml {
        name: "AttendanceImportSummaryPage",
        body: include_str!("../openapi/schemas/AttendanceImportSummaryPage.yaml"),
    },
    NamedYaml {
        name: "AttendanceRecordKind",
        body: include_str!("../openapi/schemas/AttendanceRecordKind.yaml"),
    },
    NamedYaml {
        name: "AttendanceSummaryItem",
        body: include_str!("../openapi/schemas/AttendanceSummaryItem.yaml"),
    },
    NamedYaml {
        name: "AttendanceSummaryPage",
        body: include_str!("../openapi/schemas/AttendanceSummaryPage.yaml"),
    },
    NamedYaml {
        name: "CancelOrgChangeRequest",
        body: include_str!("../openapi/schemas/CancelOrgChangeRequest.yaml"),
    },
    NamedYaml {
        name: "CompleteOrgChangeSettlementItemRequest",
        body: include_str!("../openapi/schemas/CompleteOrgChangeSettlementItemRequest.yaml"),
    },
    NamedYaml {
        name: "CreateEmployeeAttendanceRecordRequest",
        body: include_str!("../openapi/schemas/CreateEmployeeAttendanceRecordRequest.yaml"),
    },
    NamedYaml {
        name: "CreateEmployeeLifecycleEventRequest",
        body: include_str!("../openapi/schemas/CreateEmployeeLifecycleEventRequest.yaml"),
    },
    NamedYaml {
        name: "CreateEmployeeRequest",
        body: include_str!("../openapi/schemas/CreateEmployeeRequest.yaml"),
    },
    NamedYaml {
        name: "CreateOrgChangeRequest",
        body: include_str!("../openapi/schemas/CreateOrgChangeRequest.yaml"),
    },
    NamedYaml {
        name: "Employee",
        body: include_str!("../openapi/schemas/Employee.yaml"),
    },
    NamedYaml {
        name: "EmployeeAttendanceRecord",
        body: include_str!("../openapi/schemas/EmployeeAttendanceRecord.yaml"),
    },
    NamedYaml {
        name: "EmployeeAttendanceRecordPage",
        body: include_str!("../openapi/schemas/EmployeeAttendanceRecordPage.yaml"),
    },
    NamedYaml {
        name: "EmployeeDetail",
        body: include_str!("../openapi/schemas/EmployeeDetail.yaml"),
    },
    NamedYaml {
        name: "EmployeeEmploymentDetail",
        body: include_str!("../openapi/schemas/EmployeeEmploymentDetail.yaml"),
    },
    NamedYaml {
        name: "EmployeeHomeBranch",
        body: include_str!("../openapi/schemas/EmployeeHomeBranch.yaml"),
    },
    NamedYaml {
        name: "EmployeeImportColumn",
        body: include_str!("../openapi/schemas/EmployeeImportColumn.yaml"),
    },
    NamedYaml {
        name: "EmployeeImportCompanySummary",
        body: include_str!("../openapi/schemas/EmployeeImportCompanySummary.yaml"),
    },
    NamedYaml {
        name: "EmployeeImportDryRunSummary",
        body: include_str!("../openapi/schemas/EmployeeImportDryRunSummary.yaml"),
    },
    NamedYaml {
        name: "EmployeeImportPreviewResponse",
        body: include_str!("../openapi/schemas/EmployeeImportPreviewResponse.yaml"),
    },
    NamedYaml {
        name: "EmployeeImportPreviewRow",
        body: include_str!("../openapi/schemas/EmployeeImportPreviewRow.yaml"),
    },
    NamedYaml {
        name: "EmployeeImportReport",
        body: include_str!("../openapi/schemas/EmployeeImportReport.yaml"),
    },
    NamedYaml {
        name: "EmployeeLifecycleEvent",
        body: include_str!("../openapi/schemas/EmployeeLifecycleEvent.yaml"),
    },
    NamedYaml {
        name: "EmployeeLifecycleEventPage",
        body: include_str!("../openapi/schemas/EmployeeLifecycleEventPage.yaml"),
    },
    NamedYaml {
        name: "EmployeeLifecycleSignoffs",
        body: include_str!("../openapi/schemas/EmployeeLifecycleSignoffs.yaml"),
    },
    NamedYaml {
        name: "EmployeePage",
        body: include_str!("../openapi/schemas/EmployeePage.yaml"),
    },
    NamedYaml {
        name: "HrOrgChartCompany",
        body: include_str!("../openapi/schemas/HrOrgChartCompany.yaml"),
    },
    NamedYaml {
        name: "HrOrgChartEmployee",
        body: include_str!("../openapi/schemas/HrOrgChartEmployee.yaml"),
    },
    NamedYaml {
        name: "HrOrgChartPosition",
        body: include_str!("../openapi/schemas/HrOrgChartPosition.yaml"),
    },
    NamedYaml {
        name: "HrOrgChartResponse",
        body: include_str!("../openapi/schemas/HrOrgChartResponse.yaml"),
    },
    NamedYaml {
        name: "HrOrgChartUnit",
        body: include_str!("../openapi/schemas/HrOrgChartUnit.yaml"),
    },
    NamedYaml {
        name: "ImportApplyRequest",
        body: include_str!("../openapi/schemas/ImportApplyRequest.yaml"),
    },
    NamedYaml {
        name: "LeaveBalanceItem",
        body: include_str!("../openapi/schemas/LeaveBalanceItem.yaml"),
    },
    NamedYaml {
        name: "LeaveBalancePage",
        body: include_str!("../openapi/schemas/LeaveBalancePage.yaml"),
    },
    NamedYaml {
        name: "LeaveBalanceSummary",
        body: include_str!("../openapi/schemas/LeaveBalanceSummary.yaml"),
    },
    NamedYaml {
        name: "OrgChangeApprovalRoleKey",
        body: include_str!("../openapi/schemas/OrgChangeApprovalRoleKey.yaml"),
    },
    NamedYaml {
        name: "OrgChangeApprovalStep",
        body: include_str!("../openapi/schemas/OrgChangeApprovalStep.yaml"),
    },
    NamedYaml {
        name: "OrgChangeDecisionRequest",
        body: include_str!("../openapi/schemas/OrgChangeDecisionRequest.yaml"),
    },
    NamedYaml {
        name: "OrgChangeDetail",
        body: include_str!("../openapi/schemas/OrgChangeDetail.yaml"),
    },
    NamedYaml {
        name: "OrgChangeEvent",
        body: include_str!("../openapi/schemas/OrgChangeEvent.yaml"),
    },
    NamedYaml {
        name: "OrgChangeKind",
        body: include_str!("../openapi/schemas/OrgChangeKind.yaml"),
    },
    NamedYaml {
        name: "OrgChangePage",
        body: include_str!("../openapi/schemas/OrgChangePage.yaml"),
    },
    NamedYaml {
        name: "OrgChangePreflightBlocker",
        body: include_str!("../openapi/schemas/OrgChangePreflightBlocker.yaml"),
    },
    NamedYaml {
        name: "OrgChangePreflightReport",
        body: include_str!("../openapi/schemas/OrgChangePreflightReport.yaml"),
    },
    NamedYaml {
        name: "OrgChangePreflightWarning",
        body: include_str!("../openapi/schemas/OrgChangePreflightWarning.yaml"),
    },
    NamedYaml {
        name: "OrgChangeSettlementItem",
        body: include_str!("../openapi/schemas/OrgChangeSettlementItem.yaml"),
    },
    NamedYaml {
        name: "OrgChangeSettlementKey",
        body: include_str!("../openapi/schemas/OrgChangeSettlementKey.yaml"),
    },
    NamedYaml {
        name: "OrgChangeStatus",
        body: include_str!("../openapi/schemas/OrgChangeStatus.yaml"),
    },
    NamedYaml {
        name: "OrgChangeStepDecision",
        body: include_str!("../openapi/schemas/OrgChangeStepDecision.yaml"),
    },
    NamedYaml {
        name: "OrgChangeSummary",
        body: include_str!("../openapi/schemas/OrgChangeSummary.yaml"),
    },
    NamedYaml {
        name: "OrgChangeTarget",
        body: include_str!("../openapi/schemas/OrgChangeTarget.yaml"),
    },
    NamedYaml {
        name: "OrgChangeTargetKind",
        body: include_str!("../openapi/schemas/OrgChangeTargetKind.yaml"),
    },
    NamedYaml {
        name: "OrgEntitySummary",
        body: include_str!("../openapi/schemas/OrgEntitySummary.yaml"),
    },
    NamedYaml {
        name: "OrgProposalOp",
        body: include_str!("../openapi/schemas/OrgProposalOp.yaml"),
    },
    NamedYaml {
        name: "OrgProposalOpCreateBranch",
        body: include_str!("../openapi/schemas/OrgProposalOpCreateBranch.yaml"),
    },
    NamedYaml {
        name: "OrgProposalOpCreateRegion",
        body: include_str!("../openapi/schemas/OrgProposalOpCreateRegion.yaml"),
    },
    NamedYaml {
        name: "OrgProposalOpCreateSite",
        body: include_str!("../openapi/schemas/OrgProposalOpCreateSite.yaml"),
    },
    NamedYaml {
        name: "OrgProposalOpDeactivateBranch",
        body: include_str!("../openapi/schemas/OrgProposalOpDeactivateBranch.yaml"),
    },
    NamedYaml {
        name: "OrgProposalOpDeactivateRegion",
        body: include_str!("../openapi/schemas/OrgProposalOpDeactivateRegion.yaml"),
    },
    NamedYaml {
        name: "OrgProposalOpReassignOrgUnit",
        body: include_str!("../openapi/schemas/OrgProposalOpReassignOrgUnit.yaml"),
    },
    NamedYaml {
        name: "OrgProposalOpRenameBranch",
        body: include_str!("../openapi/schemas/OrgProposalOpRenameBranch.yaml"),
    },
    NamedYaml {
        name: "OrgProposalOpRenameRegion",
        body: include_str!("../openapi/schemas/OrgProposalOpRenameRegion.yaml"),
    },
    NamedYaml {
        name: "OrgProposalOpUpdateSite",
        body: include_str!("../openapi/schemas/OrgProposalOpUpdateSite.yaml"),
    },
    NamedYaml {
        name: "OrgProposalReassignScope",
        body: include_str!("../openapi/schemas/OrgProposalReassignScope.yaml"),
    },
    NamedYaml {
        name: "SetEmployeeHomeBranchRequest",
        body: include_str!("../openapi/schemas/SetEmployeeHomeBranchRequest.yaml"),
    },
    NamedYaml {
        name: "UpdateOrgChangeDraftRequest",
        body: include_str!("../openapi/schemas/UpdateOrgChangeDraftRequest.yaml"),
    },
];
