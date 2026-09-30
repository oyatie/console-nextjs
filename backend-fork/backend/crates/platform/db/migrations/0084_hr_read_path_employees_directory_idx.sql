-- no-transaction
-- HR read-path performance: 직원명부 directory order.
-- One CONCURRENTLY statement per no-transaction migration: PostgreSQL treats a
-- multi-statement simple-query message as a transaction block, which rejects
-- CREATE INDEX CONCURRENTLY. Omit IF NOT EXISTS so reruns fail closed if a
-- failed concurrent build left an INVALID index that needs DROP INDEX CONCURRENTLY.

CREATE INDEX CONCURRENTLY employees_org_directory_order_idx
    ON employees (org_id, company, name, source_sheet, source_row);
