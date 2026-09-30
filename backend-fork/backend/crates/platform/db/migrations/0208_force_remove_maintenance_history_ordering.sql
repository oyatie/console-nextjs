-- Force-removing an organization that has equipment maintenance history fails.
--
-- THIS IS A PRODUCTION DEFECT, not a test defect. It was found by wiring a test that had never
-- executed: workorder/adapter-postgres use_cases.rs, dark until 2026-07-31.
--
-- `platform_force_remove_organization` (0196:189-192) sets the force GUC and then calls the
-- catalog-driven closure `platform_force_remove_direct_org_children` BEFORE its hand-ordered
-- deletes at 0196:228-231. That closure sweeps every table holding a single-column `org_id` FK to
-- `organizations` with `confdeltype IN ('a','r')`.
--
-- The equipment maintenance-history subtree is invisible to that sweep, by two independent
-- filters: `equipment_maintenance_history` is `org_id ... ON DELETE CASCADE` (0193:19) and the
-- sweep admits only 'a' and 'r'; `_costs` and `_evidence` reach their parents through COMPOSITE
-- FKs and the sweep requires `cardinality(fk.conkey) = 1`.
--
-- So the sweep deletes tables that subtree still references, and raises 23001. Migration 0193
-- declares FOUR such composite RESTRICT foreign keys — onto `work_orders`, `registry_equipment`,
-- `evidence_media` and `equipment_cost_ledger` — and CI surfaced them one per run:
--
--   equipment_maintenance_history_costs_ledger_same_org_fk
--   equipment_maintenance_history_evidence_media_same_org_fk
--   equipment_maintenance_history_work_order_same_org_fk
--
-- TWO EARLIER ATTEMPTS ARE RECORDED HERE BECAUSE BOTH WERE WRONG IN INSTRUCTIVE WAYS.
--
-- Excluding `equipment_cost_ledger` from the sweep fixed the first constraint and moved the
-- failure to the second. Excluding the whole family — adding `registry_equipment`, `work_orders`
-- and `equipment_maintenance_history` — REGRESSED `platform-rest remove_tenant`, which had been
-- passing: the sweep deletes `registry_equipment` before `registry_sites` under its OID-descending
-- order, and removing it left `registry_sites` blocked by `registry_equipment_site_id_fkey`.
-- Presence in 0196's hand-ordered block is not the same as correct order, because that block runs
-- AFTER the sweep.
--
-- THE FIX is the one the original diagnosis recommended and this migration first declined: delete
-- the invisible subtree BEFORE the sweep, rather than removing tables from it. Three statements,
-- child-first, at the top of the function. Nothing is excluded, the sweep's ordering is untouched,
-- and the whole class is closed at once instead of one constraint per 71-minute cycle.
--
-- 0196's hand-ordered block still deletes these three later; by then they are empty. This is
-- additive to that block rather than a reordering of it.

CREATE OR REPLACE FUNCTION platform_force_remove_direct_org_children(p_id UUID)
RETURNS VOID
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = public, pg_temp
AS $$
DECLARE
    target RECORD;
BEGIN
    -- The maintenance-history subtree goes FIRST, before the catalog sweep below can reach any
    -- of the tables it points at.
    --
    -- These three are structurally invisible to that sweep: `equipment_maintenance_history` is
    -- `org_id ... ON DELETE CASCADE` (0193:19) and the sweep admits only 'a' and 'r', while
    -- `_costs` and `_evidence` reach their parents through COMPOSITE foreign keys and the sweep
    -- requires `cardinality(fk.conkey) = 1`. So the sweep deletes `work_orders`,
    -- `registry_equipment`, `evidence_media` and `equipment_cost_ledger` while rows in this
    -- subtree still reference them, and every one of those raises 23001 in turn.
    --
    -- Deleting the subtree here, child-first, removes the whole class in one place. 0196's
    -- hand-ordered block still deletes these three later; by then they are already empty, which
    -- is why this is additive rather than a reordering of that block.
    DELETE FROM equipment_maintenance_history_costs    WHERE org_id = p_id;
    DELETE FROM equipment_maintenance_history_evidence WHERE org_id = p_id;
    DELETE FROM equipment_maintenance_history          WHERE org_id = p_id;

    FOR target IN
        SELECT child_ns.nspname AS schema_name, child.relname AS relation_name
        FROM pg_catalog.pg_constraint AS fk
        JOIN pg_catalog.pg_class AS child ON child.oid = fk.conrelid
        JOIN pg_catalog.pg_namespace AS child_ns ON child_ns.oid = child.relnamespace
        JOIN pg_catalog.pg_class AS parent ON parent.oid = fk.confrelid
        JOIN pg_catalog.pg_namespace AS parent_ns ON parent_ns.oid = parent.relnamespace
        JOIN pg_catalog.pg_attribute AS child_attr
          ON child_attr.attrelid = child.oid
         AND child_attr.attnum = fk.conkey[1]
         AND NOT child_attr.attisdropped
        WHERE fk.contype = 'f'
          AND fk.confdeltype IN ('a', 'r')
          AND parent_ns.nspname = 'public'
          AND parent.relname = 'organizations'
          AND child_ns.nspname = 'public'
          AND child.relkind IN ('r', 'p')
          -- These roots have specialized ordering: the audit ledger must be
          -- re-homed, and employee/user/branch/region references are released
          -- only after their direct children have been closed by this pass.
          --
          -- equipment_cost_ledger joins them for the same reason and was missing:
          -- its children reach it by COMPOSITE FK and are therefore invisible to
          -- this catalog sweep, so deleting it here raises 23001 before the
          -- hand-ordered child-first block below can run.
          -- These roots have specialized ordering: the audit ledger must be
          -- re-homed, and employee/user/branch/region references are released
          -- only after their direct children have been closed by this pass.
          AND child.relname NOT IN (
              'audit_events', 'employees', 'users', 'branches', 'regions'
          )
          AND cardinality(fk.conkey) = 1
          AND child_attr.attname = 'org_id'
        -- New tenant-facing tables normally reference older roots.  Descending
        -- OID gives those children priority before their direct parents.
        ORDER BY child.oid DESC
    LOOP
        EXECUTE format('DELETE FROM %I.%I WHERE org_id = $1',
                       target.schema_name, target.relation_name)
            USING p_id;
    END LOOP;
END;
$$;

-- Keep the migration identity as owner, for the reason 0196 records: production
-- migrations run as console_app and isolated SQLx databases run as their own table
-- owner, so reassigning this SECURITY DEFINER would separate it from the tables it
-- must delete under FORCE RLS.
REVOKE ALL ON FUNCTION platform_force_remove_direct_org_children(UUID) FROM PUBLIC, console_rt, console_platform_force_cmd;
