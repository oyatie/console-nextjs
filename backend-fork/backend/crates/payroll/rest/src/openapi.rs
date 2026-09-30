//! This face's slice of the published OpenAPI contract.
//!
//! Bodies live as YAML under `openapi/` and are pulled in via `include_str!`.
//! `console_contracts` re-indents them; composition rejects duplicate keys.

use console_contracts::{Fragment, NamedYaml, Operation, PathItem};

/// This face's contribution to the composed OpenAPI document.
pub const OPENAPI_FRAGMENT: Fragment = Fragment {
    source: "console-payroll-rest",
    paths: PATHS,
    schemas: SCHEMAS,
    parameters: &[],
    responses: &[],
    security_schemes: &[],
    external_schemas: EXTERNAL_SCHEMAS,
};

const EXTERNAL_SCHEMAS: &[&str] = &["Timestamp", "Uuid"];

const PATHS: &[PathItem] = &[
    PathItem {
        path: "/api/v1/payroll/employees/{employeeId}/contract-wages",
        operations: &[Operation {
            method: "post",
            body: include_str!(
                "../openapi/paths/api__v1__payroll__employees__employeeId__contract-wages.post.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/payroll/employees/{employeeId}/payslip-draft",
        operations: &[Operation {
            method: "get",
            body: include_str!(
                "../openapi/paths/api__v1__payroll__employees__employeeId__payslip-draft.get.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/payroll/payslips/me",
        operations: &[Operation {
            method: "get",
            body: include_str!("../openapi/paths/api__v1__payroll__payslips__me.get.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/payroll/runs",
        operations: &[Operation {
            method: "get",
            body: include_str!("../openapi/paths/api__v1__payroll__runs.get.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/payroll/runs/{id}",
        operations: &[Operation {
            method: "get",
            body: include_str!("../openapi/paths/api__v1__payroll__runs__id.get.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/payroll/runs/{id}/calculate",
        operations: &[Operation {
            method: "post",
            body: include_str!("../openapi/paths/api__v1__payroll__runs__id__calculate.post.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/payroll/runs/{id}/close-attendance",
        operations: &[Operation {
            method: "post",
            body: include_str!(
                "../openapi/paths/api__v1__payroll__runs__id__close-attendance.post.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/payroll/runs/{id}/close-preflight",
        operations: &[Operation {
            method: "get",
            body: include_str!(
                "../openapi/paths/api__v1__payroll__runs__id__close-preflight.get.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/payroll/runs/{id}/decision",
        operations: &[Operation {
            method: "post",
            body: include_str!("../openapi/paths/api__v1__payroll__runs__id__decision.post.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/payroll/runs/{id}/disbursement/attest",
        operations: &[Operation {
            method: "post",
            body: include_str!(
                "../openapi/paths/api__v1__payroll__runs__id__disbursement__attest.post.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/payroll/runs/{id}/exceptions",
        operations: &[Operation {
            method: "get",
            body: include_str!("../openapi/paths/api__v1__payroll__runs__id__exceptions.get.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/payroll/runs/{id}/exceptions/{exceptionId}/resolve",
        operations: &[Operation {
            method: "post",
            body: include_str!(
                "../openapi/paths/api__v1__payroll__runs__id__exceptions__exceptionId__resolve.post.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/payroll/runs/{id}/issue-payslips",
        operations: &[Operation {
            method: "post",
            body: include_str!(
                "../openapi/paths/api__v1__payroll__runs__id__issue-payslips.post.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/payroll/runs/{id}/payslip-delivery",
        operations: &[Operation {
            method: "get",
            body: include_str!(
                "../openapi/paths/api__v1__payroll__runs__id__payslip-delivery.get.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/payroll/runs/{id}/schedule-disbursement",
        operations: &[Operation {
            method: "post",
            body: include_str!(
                "../openapi/paths/api__v1__payroll__runs__id__schedule-disbursement.post.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/payroll/runs/{id}/submit",
        operations: &[Operation {
            method: "post",
            body: include_str!("../openapi/paths/api__v1__payroll__runs__id__submit.post.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/payroll/runs/{id}/withdraw",
        operations: &[Operation {
            method: "post",
            body: include_str!("../openapi/paths/api__v1__payroll__runs__id__withdraw.post.yaml"),
        }],
    },
];

const SCHEMAS: &[NamedYaml] = &[
    NamedYaml {
        name: "AttestPayrollDisbursementRequest",
        body: include_str!("../openapi/schemas/AttestPayrollDisbursementRequest.yaml"),
    },
    NamedYaml {
        name: "ClosePayrollAttendanceRequest",
        body: include_str!("../openapi/schemas/ClosePayrollAttendanceRequest.yaml"),
    },
    NamedYaml {
        name: "DecidePayrollRunRequest",
        body: include_str!("../openapi/schemas/DecidePayrollRunRequest.yaml"),
    },
    NamedYaml {
        name: "MyPayrollLine",
        body: include_str!("../openapi/schemas/MyPayrollLine.yaml"),
    },
    NamedYaml {
        name: "MyPayrollLinePage",
        body: include_str!("../openapi/schemas/MyPayrollLinePage.yaml"),
    },
    NamedYaml {
        name: "PayrollClosePreflight",
        body: include_str!("../openapi/schemas/PayrollClosePreflight.yaml"),
    },
    NamedYaml {
        name: "PayrollDisbursement",
        body: include_str!("../openapi/schemas/PayrollDisbursement.yaml"),
    },
    NamedYaml {
        name: "PayrollException",
        body: include_str!("../openapi/schemas/PayrollException.yaml"),
    },
    NamedYaml {
        name: "PayrollExceptionPage",
        body: include_str!("../openapi/schemas/PayrollExceptionPage.yaml"),
    },
    NamedYaml {
        name: "PayrollLineSummary",
        body: include_str!("../openapi/schemas/PayrollLineSummary.yaml"),
    },
    NamedYaml {
        name: "PayrollLinkedRef",
        body: include_str!("../openapi/schemas/PayrollLinkedRef.yaml"),
    },
    NamedYaml {
        name: "PayrollPayslipDeliveryItem",
        body: include_str!("../openapi/schemas/PayrollPayslipDeliveryItem.yaml"),
    },
    NamedYaml {
        name: "PayrollPayslipDeliverySummary",
        body: include_str!("../openapi/schemas/PayrollPayslipDeliverySummary.yaml"),
    },
    NamedYaml {
        name: "PayrollPayslipDraft",
        body: include_str!("../openapi/schemas/PayrollPayslipDraft.yaml"),
    },
    NamedYaml {
        name: "PayrollPayslipDraftDeduction",
        body: include_str!("../openapi/schemas/PayrollPayslipDraftDeduction.yaml"),
    },
    NamedYaml {
        name: "PayrollPreflightCheck",
        body: include_str!("../openapi/schemas/PayrollPreflightCheck.yaml"),
    },
    NamedYaml {
        name: "PayrollRunCalcSummary",
        body: include_str!("../openapi/schemas/PayrollRunCalcSummary.yaml"),
    },
    NamedYaml {
        name: "PayrollRunDetail",
        body: include_str!("../openapi/schemas/PayrollRunDetail.yaml"),
    },
    NamedYaml {
        name: "PayrollRunPage",
        body: include_str!("../openapi/schemas/PayrollRunPage.yaml"),
    },
    NamedYaml {
        name: "PayrollRunSummary",
        body: include_str!("../openapi/schemas/PayrollRunSummary.yaml"),
    },
    NamedYaml {
        name: "PayrollStatutoryCitation",
        body: include_str!("../openapi/schemas/PayrollStatutoryCitation.yaml"),
    },
    NamedYaml {
        name: "PayrollStatutoryInstrument",
        body: include_str!("../openapi/schemas/PayrollStatutoryInstrument.yaml"),
    },
    NamedYaml {
        name: "ResolvePayrollExceptionRequest",
        body: include_str!("../openapi/schemas/ResolvePayrollExceptionRequest.yaml"),
    },
    NamedYaml {
        name: "SchedulePayrollDisbursementRequest",
        body: include_str!("../openapi/schemas/SchedulePayrollDisbursementRequest.yaml"),
    },
];
