-- Multi-tenant phase 1, step 6: de-own the runtime identity + lock org_id.
--
-- The application must NOT connect as the database owner (`console_app`). An owner
-- is immune to GRANT/REVOKE on its own tables, can DROP POLICY / DISABLE ROW
-- LEVEL SECURITY, and (without FORCE) bypasses RLS entirely — so a compromised
-- app connected as the owner could turn the whole tenant boundary off. This
-- migration introduces `console_rt`, a least-privilege NON-OWNER role the app
-- connects as, and makes `org_id` immutable so a row can never be moved between
-- tenants by UPDATE.

-- --------------------------------------------------------------------------
-- (1a) The runtime role. NOLOGIN here — CNPG attaches LOGIN + a managed
-- password out of band (deploy/.../database.yaml managed.roles). The explicit
-- ALTER ROLE pins the security-critical attributes regardless of who created
-- the role (CNPG, a prior partial apply, or this block), so it can never drift
-- into SUPERUSER/BYPASSRLS.
-- --------------------------------------------------------------------------
-- Roles live in CLUSTER-global pg_authid, so when sqlx::test applies every
-- migration into N fresh databases in parallel, an unconditional CREATE/ALTER
-- ROLE races the same catalog tuple ("tuple concurrently updated"). Two defenses:
--   1. pg_advisory_xact_lock — cluster-wide (not per-DB) and auto-released on
--      commit, so it strictly orders this role setup across concurrent applies.
--      In production (a single migration applier) it is an uncontended no-op.
--   2. Idempotent create/alter — only write pg_authid when the role is missing
--      or an attribute has actually drifted, so repeat applies touch nothing and
--      cannot conflict even if the lock were unavailable.
SELECT pg_advisory_xact_lock(hashtext('console_rt_role_setup'));

-- The advisory lock above is best-effort (a transaction-scoped lock is ineffective
-- if the migration runner applies statements in autocommit), so the catch below is
-- the real guard: on a FRESH cluster every parallel sqlx::test DB races to CREATE
-- console_rt, and the losers must swallow the pg_authid unique_violation. Whoever wins
-- the CREATE sets the full attribute set, so the losers can safely no-op.
DO $$
BEGIN
    IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'console_rt') THEN
        CREATE ROLE console_rt NOLOGIN NOSUPERUSER NOBYPASSRLS NOCREATEDB NOCREATEROLE;
    ELSIF EXISTS (
        SELECT 1 FROM pg_roles
        WHERE rolname = 'console_rt'
          AND (rolsuper OR rolbypassrls OR rolcreatedb OR rolcreaterole)
    ) THEN
        ALTER ROLE console_rt NOSUPERUSER NOBYPASSRLS NOCREATEDB NOCREATEROLE;
    END IF;
EXCEPTION
    WHEN duplicate_object OR unique_violation THEN NULL;
END
$$;

-- --------------------------------------------------------------------------
-- (1b) Least-privilege table grants to the RUNTIME role (NOT the owner).
-- The 9 slice tenant tables + the request-counter get full DML (RLS narrows
-- what is actually visible/writable). `organizations` is SELECT-ONLY:
-- provisioning a tenant is an owner operation, so the runtime role must never
-- INSERT/UPDATE/DELETE org rows. We REVOKE the write verbs defensively in case
-- a default/earlier grant handed them out.
-- --------------------------------------------------------------------------
GRANT SELECT, INSERT, UPDATE, DELETE ON
    regions,
    branches,
    users,
    registry_customers,
    registry_sites,
    registry_equipment,
    work_orders,
    work_order_request_counters
TO console_rt;

GRANT SELECT ON organizations TO console_rt;
REVOKE INSERT, UPDATE, DELETE ON organizations FROM console_rt;

-- Future tables created by the owner auto-grant DML to the runtime role, so the
-- rollout to the remaining ~55 tables does not have to remember a GRANT per
-- table. (organizations is the one deliberate exception above.)
ALTER DEFAULT PRIVILEGES FOR ROLE console_app IN SCHEMA public
    GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO console_rt;

-- --------------------------------------------------------------------------
-- (1c) audit_events: INSERT + SELECT only — NO update/delete (append-only is
-- preserved at the grant layer, beneath the trigger from 0003). PUBLIC has no
-- implicit grant here and console_rt is a non-owner, so WITHOUT this every audited
-- write (which INSERTs an audit row in the same tx) would fail permission-
-- denied. Deliberately omitting UPDATE/DELETE keeps the table append-only.
-- --------------------------------------------------------------------------
GRANT INSERT, SELECT ON audit_events TO console_rt;

-- --------------------------------------------------------------------------
-- (4) org_id is immutable after insert. The RLS WITH CHECK already rejects an
-- UPDATE that moves a row to ANOTHER visible-but-wrong org, but a single shared
-- BEFORE UPDATE trigger makes the invariant total and explicit: any statement
-- that changes org_id — to any value — aborts. Re-stating the SAME org_id is a
-- no-op and allowed (NEW IS DISTINCT FROM OLD is false).
-- --------------------------------------------------------------------------
CREATE OR REPLACE FUNCTION enforce_org_id_immutable()
RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.org_id IS DISTINCT FROM OLD.org_id THEN
        RAISE EXCEPTION
            'org_id is immutable: cannot change tenant from % to % on % (id=%)',
            OLD.org_id, NEW.org_id, TG_TABLE_NAME, OLD.id;
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER trg_regions_org_immutable
    BEFORE UPDATE ON regions
    FOR EACH ROW EXECUTE FUNCTION enforce_org_id_immutable();
CREATE TRIGGER trg_branches_org_immutable
    BEFORE UPDATE ON branches
    FOR EACH ROW EXECUTE FUNCTION enforce_org_id_immutable();
CREATE TRIGGER trg_users_org_immutable
    BEFORE UPDATE ON users
    FOR EACH ROW EXECUTE FUNCTION enforce_org_id_immutable();
CREATE TRIGGER trg_registry_customers_org_immutable
    BEFORE UPDATE ON registry_customers
    FOR EACH ROW EXECUTE FUNCTION enforce_org_id_immutable();
CREATE TRIGGER trg_registry_sites_org_immutable
    BEFORE UPDATE ON registry_sites
    FOR EACH ROW EXECUTE FUNCTION enforce_org_id_immutable();
CREATE TRIGGER trg_registry_equipment_org_immutable
    BEFORE UPDATE ON registry_equipment
    FOR EACH ROW EXECUTE FUNCTION enforce_org_id_immutable();
CREATE TRIGGER trg_work_orders_org_immutable
    BEFORE UPDATE ON work_orders
    FOR EACH ROW EXECUTE FUNCTION enforce_org_id_immutable();
CREATE TRIGGER trg_work_order_request_counters_org_immutable
    BEFORE UPDATE ON work_order_request_counters
    FOR EACH ROW EXECUTE FUNCTION enforce_org_id_immutable();

-- organizations keys its tenant id on `id` (not org_id); its identity is locked
-- by the primary key already, so it does not get the org_id trigger.
