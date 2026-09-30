-- no-transaction
-- HR/payroll readiness read-path performance: status + employee ordering.
-- Keep one CONCURRENTLY statement per no-transaction migration; see 0084.

CREATE INDEX CONCURRENTLY payroll_draft_lines_org_status_employee_idx
    ON payroll_draft_lines (
        org_id,
        calculation_status,
        employee_company,
        employee_display_name,
        run_id
    );
