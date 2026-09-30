-- REG-P4: make `payroll_line_calculations` append-only in the database, not in
-- review.
--
-- Migration 0186 declares these rows "append-only ... only the `payable` flip
-- is a legal update", and `payable` gates nothing the runtime role may set.
-- Neither property was enforced: `console_rt` -- the role the application
-- actually runs as -- held table-wide INSERT and UPDATE. Verified against a
-- PostgreSQL 18.4 replica of the CI topology, as `console_rt`:
-- `UPDATE payroll_line_calculations SET payable = TRUE` succeeded.
--
-- DELETE MUST BE REVOKED TOO, and it is not granted by 0186 -- it arrives
-- silently. `0031_runtime_role_and_immutable_org.sql:75` sets ALTER DEFAULT
-- PRIVILEGES granting SELECT, INSERT, UPDATE, DELETE on every future table to
-- `console_rt`, so `payroll_line_calculations` received DELETE the moment 0186
-- created it, without any GRANT naming it. Append-only that revokes UPDATE and
-- leaves DELETE is not append-only. The same file already applies the correct
-- pattern to `audit_events`: "INSERT + SELECT only -- NO update/delete
-- (append-only is preserved at the grant layer)".
--
-- NO UPDATE IS GRANTED BACK. An earlier revision of this migration re-granted
-- UPDATE on every non-`payable` column to avoid disturbing the writer. That was
-- wrong: nothing in the tree updates this table -- the only writer
-- (payroll/adapter-postgres lifecycle.rs) is a plain INSERT with an explicit
-- column list, and there is no `ON CONFLICT DO UPDATE` anywhere against it. So
-- re-granting UPDATE preserved a privilege no caller uses, and left
-- `gross_won`, `deductions`, `net_won` and `tax_table_version` rewritable after
-- calculation and review by any tenant-scoped session. Those columns are read
-- straight into payslip issuance. Append-only is the declared contract; this
-- grants what the contract allows and nothing more.
--
-- WHY A TABLE-LEVEL REVOKE AND A COLUMN-LEVEL RE-GRANT:
-- a column-level REVOKE cannot subtract from a table-level grant. Running
-- `REVOKE INSERT (payable), UPDATE (payable) ... FROM console_rt` against the
-- table-wide grant is a silent no-op -- measured, not assumed: `UPDATE ... SET
-- payable = TRUE` still succeeded afterwards. The privilege has to be removed
-- at table scope and re-granted per column.
--
-- WHAT THIS DOES NOT CLOSE. `console_rt` keeps column-level INSERT, because
-- the calculation writer needs it, and issuance reads the HIGHEST version per
-- line: `load_payslip_issuance_in_tx` selects
-- `DISTINCT ON (c.line_id) ... ORDER BY c.line_id, c.version DESC` and does not
-- consult `payable`. A compromised tenant-scoped session can therefore append a
-- row at a higher `version` with arbitrary money fields after approval and
-- before issuance, and that row becomes the payslip -- the same outcome an
-- in-place rewrite would have produced. Revoking UPDATE and DELETE closes
-- mutation and erasure; it does not close forgery by append. Closing that
-- requires routing INSERT through a status-checked writer that refuses a new
-- version once the run leaves the calculating state, which is a change to the
-- calculation path rather than a grant change, and is tracked as an open HOLD
-- on this lane rather than claimed here.
--
-- `console_app` (the migration owner) deliberately keeps its privileges. The
-- release path `payable` exists for is a later, separately reviewed decision;
-- this removes the ambient ability, it does not decide who may eventually
-- grant it.
DO $$
DECLARE
    v_insert_columns TEXT;
BEGIN
    SELECT string_agg(quote_ident(column_name), ', ' ORDER BY ordinal_position)
      INTO v_insert_columns
      FROM information_schema.columns
     WHERE table_schema = 'public'
       AND table_name = 'payroll_line_calculations'
       AND column_name <> 'payable';

    IF v_insert_columns IS NULL THEN
        RAISE EXCEPTION USING
            ERRCODE = '42P01',
            MESSAGE = 'payroll_payable.table_absent_or_columnless';
    END IF;

    EXECUTE 'REVOKE INSERT, UPDATE, DELETE ON public.payroll_line_calculations FROM console_rt';
    -- Revoking DELETE on the child alone does not make it append-only. Both
    -- foreign keys cascade (0186: line_id -> payroll_draft_lines, and
    -- (run_id, org_id) -> payroll_draft_runs, each ON DELETE CASCADE), and 0074
    -- grants the parents SELECT, INSERT, UPDATE while DELETE arrives from
    -- 0031's default ACL and is never revoked. A tenant-scoped session could
    -- therefore erase calculations by deleting a parent, without ever holding
    -- DELETE here. Nothing in the tree deletes either parent.
    EXECUTE 'REVOKE DELETE ON public.payroll_draft_runs FROM console_rt';
    EXECUTE 'REVOKE DELETE ON public.payroll_draft_lines FROM console_rt';
    EXECUTE format(
        'GRANT INSERT (%s) ON public.payroll_line_calculations TO console_rt', v_insert_columns);
END $$;

-- The migration proves its own effect rather than asserting it. All three
-- clauses matter: dropping too much would break the calculation writer, which
-- inserts an explicit column list excluding `payable`. A migration that fails
-- closed on payroll writes is not a fix.
DO $$
BEGIN
    IF EXISTS (
        SELECT 1 FROM information_schema.column_privileges
         WHERE table_schema = 'public'
           AND table_name = 'payroll_line_calculations'
           AND grantee = 'console_rt'
           AND privilege_type = 'UPDATE'
    ) THEN
        RAISE EXCEPTION USING
            ERRCODE = '42501',
            MESSAGE = 'payroll_payable.runtime_update_not_revoked';
    END IF;

    IF EXISTS (
        SELECT 1 FROM information_schema.table_privileges
         WHERE table_name = 'payroll_line_calculations'
           AND grantee = 'console_rt'
           AND privilege_type = 'DELETE'
    ) THEN
        RAISE EXCEPTION USING
            ERRCODE = '42501',
            MESSAGE = 'payroll_payable.runtime_delete_not_revoked';
    END IF;

    IF EXISTS (
        SELECT 1 FROM information_schema.column_privileges
         WHERE table_schema = 'public'
           AND table_name = 'payroll_line_calculations'
           AND column_name = 'payable'
           AND grantee = 'console_rt'
           AND privilege_type = 'INSERT'
    ) THEN
        RAISE EXCEPTION USING
            ERRCODE = '42501',
            MESSAGE = 'payroll_payable.runtime_insert_not_revoked';
    END IF;

    IF EXISTS (
        SELECT 1 FROM information_schema.table_privileges
         WHERE table_schema = 'public'
           AND table_name IN ('payroll_draft_runs', 'payroll_draft_lines')
           AND grantee = 'console_rt'
           AND privilege_type = 'DELETE'
    ) THEN
        RAISE EXCEPTION USING
            ERRCODE = '42501',
            MESSAGE = 'payroll_payable.cascade_parent_delete_not_revoked';
    END IF;

    IF NOT EXISTS (
        SELECT 1 FROM information_schema.column_privileges
         WHERE table_schema = 'public'
           AND table_name = 'payroll_line_calculations'
           AND column_name = 'net_won'
           AND grantee = 'console_rt'
           AND privilege_type = 'INSERT'
    ) THEN
        RAISE EXCEPTION USING
            ERRCODE = '42501',
            MESSAGE = 'payroll_payable.runtime_lost_legitimate_insert';
    END IF;
END $$;
