-- no-transaction
-- Data-import review read-path performance: run/status/source row order.
-- Keep one CONCURRENTLY statement per no-transaction migration; see 0084.

CREATE INDEX CONCURRENTLY data_import_rows_org_run_status_order_idx
    ON data_import_rows (org_id, run_id, row_status, source_sheet, source_row);
