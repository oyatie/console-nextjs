-- no-transaction
-- Data-import review read-path performance: tenant/status/recent-created order.
-- Keep one CONCURRENTLY statement per no-transaction migration; see 0084.

CREATE INDEX CONCURRENTLY data_import_rows_org_status_created_idx
    ON data_import_rows (org_id, row_status, created_at DESC);
