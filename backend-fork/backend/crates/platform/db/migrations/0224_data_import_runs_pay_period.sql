-- Give the import ledger a real pay period, so a payroll roster can be scoped by
-- something other than a hand-typed filename.
--
-- `scripts/stage_coss_group_payroll_readiness.sql` scopes its roster with
-- `source_filename LIKE '2026/5월/%'`. `source_filename` is whatever the operator
-- named the upload (backend/app/src/hr.rs sets it from `upload.filename`); there
-- is no convention, documented or enforced. That literal was one operator's
-- folder layout, and it is the only thing standing between a payroll roster and
-- material from the wrong month.
--
-- WHY THE RUN AND NOT THE ROW. Three facts, each sufficient:
--   * `data_import_rows` is APPEND-ONLY (`trg_data_import_rows_no_update`, 0070),
--     so a mis-parsed per-row period could never be corrected.
--   * The only repair would be re-import, and the staging script SUMS across runs
--     (`sum(rm.work_days_value)` and friends), so re-importing DOUBLES every hour
--     and day figure rather than replacing it.
--   * There is nothing reliable to parse anyway: `지급일` is a RESTRICTED header,
--     masked in the very preview meant to review it.
-- A run-level period is one declaration, made once, visible, and freezable.
--
-- NOT NULL WITH NO DEFAULT, and no sentinel. There is no live data and nothing is
-- deployed, so there is no row to accommodate and no backfill to invent. A
-- DEFAULT here would be worse than useless: it would let an upload acquire a pay
-- period nobody chose, which is exactly the fabricated provenance this column
-- exists to remove. Every writer must now say which period it is importing.
--
-- IMMUTABLE AFTER INSERT. `console_rt` holds table-wide UPDATE on this table
-- (0070:83, never revoked) and 0166's `employee_import_run_writer_guard` only
-- inspects `entity_type` changes and INSERT-with-APPLIED -- a period-only UPDATE
-- is permitted today. Without the trigger below the roster's scope would be a
-- mutable, unaudited pointer sitting on top of immutable material: the rows could
-- not be changed, but which rows a payroll run sees could be, silently.

ALTER TABLE data_import_runs
    ADD COLUMN pay_period_start DATE NOT NULL,
    ADD COLUMN pay_period_end   DATE NOT NULL;

ALTER TABLE data_import_runs
    ADD CONSTRAINT data_import_runs_pay_period_order_check
        CHECK (pay_period_end >= pay_period_start);

COMMENT ON COLUMN data_import_runs.pay_period_start IS
    'pd:none — structural or non-personal attribute of a non-person row; the pay period this upload covers, declared by the uploader';
COMMENT ON COLUMN data_import_runs.pay_period_end IS
    'pd:none — structural or non-personal attribute of a non-person row; the pay period this upload covers, declared by the uploader';

CREATE OR REPLACE FUNCTION data_import_runs_pay_period_immutable()
RETURNS TRIGGER LANGUAGE plpgsql SET search_path = pg_catalog AS $$
BEGIN
    IF NEW.pay_period_start IS DISTINCT FROM OLD.pay_period_start
       OR NEW.pay_period_end IS DISTINCT FROM OLD.pay_period_end THEN
        RAISE EXCEPTION
            'data_import_runs.pay_period is immutable: run % attempted % .. % -> % .. %',
            OLD.id, OLD.pay_period_start, OLD.pay_period_end,
            NEW.pay_period_start, NEW.pay_period_end;
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER trg_data_import_runs_pay_period_immutable
    BEFORE UPDATE ON data_import_runs
    FOR EACH ROW EXECUTE FUNCTION data_import_runs_pay_period_immutable();
