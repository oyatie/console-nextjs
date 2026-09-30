-- Multi-tenant phase 1, step 5: Row Level Security — the second mandatory gate.
--
-- For each slice table:
--   * ENABLE ROW LEVEL SECURITY turns policies on for non-owner roles.
--   * FORCE ROW LEVEL SECURITY makes the table OWNER subject to them too. This
--     is critical: the application may connect as the table owner, and without
--     FORCE the owner silently bypasses every policy. (Superusers and BYPASSRLS
--     roles still bypass RLS — that is why the app, and the isolation test,
--     run as the unprivileged `console_app` role.)
--   * POLICY org_isolation gates BOTH visibility (USING) and writes (WITH
--     CHECK) on org_id = the per-transaction GUC `app.current_org`.
--
-- FAIL-CLOSED: current_setting('app.current_org', true) passes missing_ok=true,
-- so an UNSET GUC yields NULL — and a GUC that was set earlier in the session
-- then reset yields the empty string ''. NULLIF(..., '') collapses BOTH to NULL,
-- NULL::uuid stays NULL, and `org_id = NULL` is never TRUE — every row is
-- filtered out and every write is rejected. A request that forgets to set the
-- tenant sees nothing rather than everything (and never errors mid-policy on a
-- bad ''::uuid cast).
--
-- The GUC is set transaction-locally (set_config(..., true)) by the
-- with_audit / tenant_scoped_read helpers immediately after BEGIN.

-- Runtime-role privileges live in 0031 and target `console_rt` (the non-owner role
-- the application actually connects as) — NOT the owner `console_app`. Granting the
-- owner privileges on its own tables is a no-op, and worse, conflates the
-- migration identity with the runtime identity. This file only turns RLS on.

-- organizations itself is tenant data: a tenant may only see its own org row.
-- (Provisioning a new tenant is a privileged/owner operation, not an console_app one.)
ALTER TABLE organizations ENABLE ROW LEVEL SECURITY;
ALTER TABLE organizations FORCE ROW LEVEL SECURITY;
CREATE POLICY org_isolation ON organizations
    USING (id = NULLIF(current_setting('app.current_org', true), '')::uuid)
    WITH CHECK (id = NULLIF(current_setting('app.current_org', true), '')::uuid);

ALTER TABLE regions ENABLE ROW LEVEL SECURITY;
ALTER TABLE regions FORCE ROW LEVEL SECURITY;
CREATE POLICY org_isolation ON regions
    USING (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid)
    WITH CHECK (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid);

ALTER TABLE branches ENABLE ROW LEVEL SECURITY;
ALTER TABLE branches FORCE ROW LEVEL SECURITY;
CREATE POLICY org_isolation ON branches
    USING (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid)
    WITH CHECK (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid);

ALTER TABLE users ENABLE ROW LEVEL SECURITY;
ALTER TABLE users FORCE ROW LEVEL SECURITY;
CREATE POLICY org_isolation ON users
    USING (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid)
    WITH CHECK (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid);

ALTER TABLE registry_customers ENABLE ROW LEVEL SECURITY;
ALTER TABLE registry_customers FORCE ROW LEVEL SECURITY;
CREATE POLICY org_isolation ON registry_customers
    USING (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid)
    WITH CHECK (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid);

ALTER TABLE registry_sites ENABLE ROW LEVEL SECURITY;
ALTER TABLE registry_sites FORCE ROW LEVEL SECURITY;
CREATE POLICY org_isolation ON registry_sites
    USING (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid)
    WITH CHECK (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid);

ALTER TABLE registry_equipment ENABLE ROW LEVEL SECURITY;
ALTER TABLE registry_equipment FORCE ROW LEVEL SECURITY;
CREATE POLICY org_isolation ON registry_equipment
    USING (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid)
    WITH CHECK (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid);

ALTER TABLE work_orders ENABLE ROW LEVEL SECURITY;
ALTER TABLE work_orders FORCE ROW LEVEL SECURITY;
CREATE POLICY org_isolation ON work_orders
    USING (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid)
    WITH CHECK (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid);

ALTER TABLE work_order_request_counters ENABLE ROW LEVEL SECURITY;
ALTER TABLE work_order_request_counters FORCE ROW LEVEL SECURITY;
CREATE POLICY org_isolation ON work_order_request_counters
    USING (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid)
    WITH CHECK (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid);
