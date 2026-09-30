-- Multi-tenant phase 1, step 3: seed tenant #1 and backfill every slice row.
--
-- All pre-existing data belongs to KNL Logistics (the only tenant before
-- multi-tenancy). Create that organization and stamp every slice row with its
-- id, leaving zero NULLs so 0029 can SET NOT NULL. The DO block captures the
-- freshly generated org id in a variable and reuses it across all updates.
--
-- Idempotent on the organization (ON CONFLICT on the unique slug); the UPDATEs
-- only touch rows whose org_id is still NULL, so re-running is a clean no-op.

-- KNL's org id is FIXED (not gen_random_uuid) so the application has a stable
-- compile-time constant for tenant #1 (console_kernel_core::OrgId::knl()). Every
-- single-tenant write path arms `app.current_org` with this exact value until
-- the per-request tenant resolver lands; the runtime constant and the seeded
-- row must therefore agree byte-for-byte.
DO $$
DECLARE
    knl_id UUID := '00000000-0000-0000-0000-0000000000a1';
BEGIN
    INSERT INTO organizations (id, slug, name)
    VALUES (knl_id, 'knl', 'KNL Logistics')
    ON CONFLICT (slug) DO UPDATE SET slug = EXCLUDED.slug
    RETURNING id INTO knl_id;

    UPDATE regions            SET org_id = knl_id WHERE org_id IS NULL;
    UPDATE branches           SET org_id = knl_id WHERE org_id IS NULL;
    UPDATE users              SET org_id = knl_id WHERE org_id IS NULL;
    UPDATE registry_customers SET org_id = knl_id WHERE org_id IS NULL;
    UPDATE registry_sites     SET org_id = knl_id WHERE org_id IS NULL;
    UPDATE registry_equipment SET org_id = knl_id WHERE org_id IS NULL;
    UPDATE work_orders        SET org_id = knl_id WHERE org_id IS NULL;
END
$$;
