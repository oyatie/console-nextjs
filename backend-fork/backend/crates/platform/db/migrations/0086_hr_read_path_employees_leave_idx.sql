-- no-transaction
-- HR read-path performance: 연차 balances ordered by tenant/company/name.
-- Keep one CONCURRENTLY statement per no-transaction migration; see 0084.

CREATE INDEX CONCURRENTLY employees_org_leave_order_idx
    ON employees (org_id, company, name, source_sheet, source_row)
    WHERE leave_accrued IS NOT NULL
       OR leave_used IS NOT NULL
       OR leave_remaining IS NOT NULL;
