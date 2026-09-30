-- Employee leave balances: the leave-owned ledger that relocates the three
-- `employees` balance columns (leave_accrued/leave_used/leave_remaining, added
-- by 0066 and widened by 0166) off the canonical `employees` table as part of
-- the leave-writer removal (console-hee2).
--
-- PR-1 IS SCHEMA-ONLY. This migration creates the table, arms tenant isolation,
-- and grants least-privilege access. Nothing reads or writes it yet, so the
-- change is provably behavior-neutral. The backfill and the writer re-pointing
-- land atomically in 0219 (PR-2); the `employees` balance columns and the
-- console_leave_definer INSERT/UPDATE grant on employees are dropped in 0220
-- (PR-4).
--
-- NOT IN THE CANONICAL ROSTER. This is leave-domain state, not a canonical
-- ObjectKey table, so the writer-ownership census
-- (ops/postgres-reconcile-topology.sh, scoped to `canonical_tables` =
-- ObjectKey::owned_tables) does not examine it and no expected-writer allowlist
-- entry is needed. Ownership follows the canonical convention: migrations run
-- as console_app, so it owns the table by default.
--
-- RLS FORCE IS THE CANONICAL OWNER-BOUNDARY FLOOR (0030's idiom): FORCE
-- subjects a non-BYPASSRLS table owner to its own policies. console_app is
-- BYPASSRLS (migration-only) and therefore bypasses RLS with or without FORCE;
-- that exemption is intentional and scoped to DDL/backfill. The enforced tenant
-- boundary is the NOBYPASSRLS serving roles: console_rt (runtime reads) and the
-- console_leave_definer SECURITY DEFINER functions, which already
-- `SET row_security = on` and set_config 'app.current_org', so their reads and
-- writes are tenant-filtered under ENABLE + FORCE.

-- console-gate: audited-table employee_leave_balances
CREATE TABLE employee_leave_balances (
    org_id          UUID          NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    employee_id     UUID          NOT NULL,
    -- NUMERIC(16,6) matches the widened employees columns (0166:27-30).
    -- NOT NULL DEFAULT 0 makes "no balance yet" an explicit zero; 0219's
    -- backfill must COALESCE employees' nullable source columns.
    leave_accrued   NUMERIC(16,6) NOT NULL DEFAULT 0,
    leave_used      NUMERIC(16,6) NOT NULL DEFAULT 0,
    leave_remaining NUMERIC(16,6) NOT NULL DEFAULT 0,
    -- Replaces employees.updated_at as the balance CAS basis (design §3.1).
    updated_at      TIMESTAMPTZ   NOT NULL DEFAULT now(),
    PRIMARY KEY (org_id, employee_id),
    -- (employee_id, org_id) matches employees' UNIQUE (id, org_id) (0166:18).
    -- ON DELETE CASCADE: a deleted employee's balance is personal data and must
    -- be erasable with the row (consistent with the erasure posture).
    FOREIGN KEY (employee_id, org_id) REFERENCES employees (id, org_id) ON DELETE CASCADE
);

ALTER TABLE employee_leave_balances ENABLE ROW LEVEL SECURITY;
ALTER TABLE employee_leave_balances FORCE ROW LEVEL SECURITY;
CREATE POLICY org_isolation ON employee_leave_balances
    USING (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid)
    WITH CHECK (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid);

-- console_app's default privileges GRANT SELECT, INSERT, UPDATE, DELETE to
-- console_rt on every table it creates, so an omitted verb is NOT withheld.
-- REVOKE first, then GRANT is the whole truth (the 0213 idiom).
REVOKE ALL ON employee_leave_balances FROM PUBLIC;
REVOKE ALL ON employee_leave_balances FROM console_rt;
GRANT SELECT ON employee_leave_balances TO console_rt;
-- The leave writer holds SELECT/INSERT/UPDATE so its SECURITY DEFINER functions
-- (import_employee_leave_balance, decide_request — re-pointed in 0219) can
-- write the ledger, mirroring 0166's leave_requests grant shape. No DELETE:
-- balances are erased only via the employees cascade.
GRANT SELECT, INSERT, UPDATE ON employee_leave_balances TO console_leave_definer;

-- Personal-data classification (Rule A of 0211): the row IS a natural person's
-- leave balance, so every column is at least `personal`.
-- BASELINE_FROZEN_AFTER_MIGRATION = 209 means a new table cannot be sheltered
-- by the unclassified baseline.
COMMENT ON COLUMN employee_leave_balances.org_id IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN employee_leave_balances.employee_id IS 'pd:personal — 직원 레코드 식별자 - employees.id';
COMMENT ON COLUMN employee_leave_balances.leave_accrued IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN employee_leave_balances.leave_used IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN employee_leave_balances.leave_remaining IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN employee_leave_balances.updated_at IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
