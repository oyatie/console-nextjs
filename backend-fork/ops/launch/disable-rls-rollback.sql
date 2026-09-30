-- EMERGENCY ROLLBACK — disable RLS so the pre-cutover owner-app works again.
--
-- Use ONLY during a failed multi-tenant cutover (see
-- ops/launch/multi-tenant-cutover-runbook.md §3, option 2), when a full CNPG
-- PITR restore would be too slow. Run as the database OWNER (console_app) through the
-- admin tunnel.
--
-- Why this is needed: migrations 0030/0035 set FORCE ROW LEVEL SECURITY, which
-- subjects even the owner to RLS. The old app connects as the owner and never
-- sets `app.current_org`, so with RLS on it reads zero rows. Disabling RLS makes
-- the owner-app functional again. The additive `org_id` columns, backfill, FKs,
-- per-org uniques and immutable-org_id triggers are LEFT IN PLACE — they are
-- harmless to the old single-tenant app (every row is KNL) and let you re-attempt
-- the cutover later without redoing the schema work.
--
-- After running this: revert the prod overlay image digest to the previous
-- (pre-cutover) console-app/console-web and let Argo sync the old app back.
--
-- This is dynamic (covers every RLS-enabled table, including the rollout's ~50)
-- so it cannot drift from the migrations.

DO $$
DECLARE
    t regclass;
    n int := 0;
BEGIN
    FOR t IN
        SELECT c.oid::regclass
        FROM pg_class c
        JOIN pg_namespace ns ON ns.oid = c.relnamespace
        WHERE c.relkind = 'r'
          AND ns.nspname = 'public'
          AND c.relrowsecurity            -- RLS enabled
    LOOP
        EXECUTE format('ALTER TABLE %s NO FORCE ROW LEVEL SECURITY', t);
        EXECUTE format('ALTER TABLE %s DISABLE ROW LEVEL SECURITY', t);
        n := n + 1;
    END LOOP;
    RAISE NOTICE 'RLS disabled on % tables (owner-app can read again)', n;
END
$$;

-- Sanity: should return zero rows (no table still forces/enables RLS).
SELECT relname FROM pg_class
WHERE relkind = 'r'
  AND relnamespace = 'public'::regnamespace
  AND (relrowsecurity OR relforcerowsecurity)
ORDER BY relname;
