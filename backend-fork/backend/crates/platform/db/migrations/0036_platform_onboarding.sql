-- Platform tier + tenant onboarding (phase 1, platform-admin slice).
--
-- The SaaS-vendor PLATFORM tier sits ABOVE every tenant. Two cross-tenant
-- operations need a privileged, AUDITED path that the unprivileged runtime role
-- `console_rt` cannot otherwise perform under RLS:
--
--   1. CREATE a new tenant `organizations` row. `console_rt` is SELECT-ONLY on
--      `organizations` (migration 0031) and the table is FORCE RLS with
--      `WITH CHECK (id = app.current_org)` (0030). A fresh org's id equals no
--      pre-set GUC, so even a write-granted role could not satisfy the policy
--      the normal way. We expose a SECURITY DEFINER function (owned by the
--      migration applier — the table OWNER in every environment: the superuser
--      locally / in sqlx::test, `console_app` under CNPG) that generates the id, arms
--      the GUC to THAT id transaction-locally, and inserts — so the WITH CHECK
--      passes for exactly the row being created and nothing else. Because DEFINER
--      runs as the table owner it can INSERT; because it sets the GUC to the new
--      id (FORCE RLS still applies to the owner), it can only ever create the
--      single row it just minted. `console_rt` is GRANTed EXECUTE so the app can call
--      it, but the app still cannot INSERT `organizations` directly.
--
--   2. RESOLVE the org of a refresh-token family from the token hash, BEFORE the
--      tenant GUC is known. `auth_refresh_tokens` is FORCE RLS (0035), so the
--      app's `console_rt` lookup-by-hash returns ZERO rows until the GUC is armed —
--      but the org IS the thing we need to arm it. A narrow SECURITY DEFINER
--      resolver returns only the family's `org_id` for a given token hash,
--      breaking the chicken-and-egg without widening any read surface.
--
-- Both functions are the ONLY privileged escapes and are deliberately tiny and
-- single-purpose. The app audits the create via `with_audits` around the call.

-- ---------------------------------------------------------------------------
-- (1) platform_create_organization(slug, name) -> organizations row id.
-- SECURITY DEFINER so it runs as the function owner (`console_app`, the table
-- owner). It arms `app.current_org` to the freshly-generated id so the FORCE-RLS
-- WITH CHECK accepts exactly this row, then inserts. Returns the new id.
-- ---------------------------------------------------------------------------
CREATE OR REPLACE FUNCTION platform_create_organization(
    p_slug TEXT,
    p_name TEXT
)
RETURNS UUID
LANGUAGE plpgsql
SECURITY DEFINER
-- Pin search_path: a SECURITY DEFINER function must not resolve objects through
-- a caller-controlled search_path (privilege-escalation hardening).
SET search_path = public, pg_temp
AS $$
DECLARE
    new_id UUID := gen_random_uuid();
BEGIN
    -- Arm the tenant GUC to the new id, transaction-locally, so the FORCE-RLS
    -- WITH CHECK on `organizations` accepts this exact row (and only it).
    PERFORM set_config('app.current_org', new_id::text, true);
    INSERT INTO organizations (id, slug, name)
    VALUES (new_id, p_slug, p_name);
    RETURN new_id;
END;
$$;

-- The runtime role may EXECUTE the function (the app's platform path calls it),
-- but still cannot INSERT `organizations` directly. PUBLIC gets no execute.
REVOKE ALL ON FUNCTION platform_create_organization(TEXT, TEXT) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION platform_create_organization(TEXT, TEXT) TO console_rt;

-- ---------------------------------------------------------------------------
-- (2) platform_resolve_token_org(token_hash) -> org_id (or NULL).
-- SECURITY DEFINER narrow resolver: returns ONLY the org_id of the refresh-token
-- family owning a given token hash, so the app can arm the GUC before the
-- RLS-gated token read/rotate. Exposes nothing but the tenant id.
-- ---------------------------------------------------------------------------
CREATE OR REPLACE FUNCTION platform_resolve_token_org(
    p_token_hash BYTEA
)
RETURNS UUID
LANGUAGE sql
SECURITY DEFINER
SET search_path = public, pg_temp
AS $$
    SELECT org_id
    FROM auth_refresh_tokens
    WHERE token_hash = p_token_hash
    LIMIT 1;
$$;

REVOKE ALL ON FUNCTION platform_resolve_token_org(BYTEA) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION platform_resolve_token_org(BYTEA) TO console_rt;

-- ---------------------------------------------------------------------------
-- (2b) Cross-tenant org reads/writes for the PLATFORM tier. `organizations` is
-- SELECT-only + FORCE RLS for `console_rt` and a tenant only ever sees its own row,
-- so the platform tenant-list / status APIs need these SECURITY DEFINER escapes.
-- Each runs as the owner with `row_security = off` so it can see/modify across
-- tenants; the app audits every call. EXECUTE is granted only to `console_rt`.
-- ---------------------------------------------------------------------------
-- IMPORTANT: these DEFINER functions are called MID-TRANSACTION by the app (the
-- app's `console_rt` role then runs more statements in the SAME transaction). A bare
-- `SET LOCAL row_security = off` would persist past the function and POISON the
-- caller's transaction (a non-BYPASSRLS role cannot run with row_security off
-- when RLS applies → "query would be affected by row-level security policy").
-- So each function sets it off, does its work, and RESETs it back to on BEFORE
-- returning — the off window is confined to the function body. `pg_temp` is
-- pinned in search_path for DEFINER hardening.

CREATE OR REPLACE FUNCTION platform_list_organizations()
RETURNS TABLE (
    id UUID, slug TEXT, name TEXT, status TEXT,
    created_at TIMESTAMPTZ, updated_at TIMESTAMPTZ
)
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = public, pg_temp
AS $$
DECLARE
    result_rows organizations[];
BEGIN
    SET LOCAL row_security = off;
    SELECT array_agg(o ORDER BY o.created_at ASC, o.id ASC)
    INTO result_rows
    FROM organizations o
    -- The platform sentinel is not a tenant; never list it.
    WHERE o.id <> '00000000-0000-0000-0000-00000000face'::uuid;
    SET LOCAL row_security = on;

    RETURN QUERY
        SELECT r.id, r.slug, r.name, r.status, r.created_at, r.updated_at
        FROM unnest(COALESCE(result_rows, ARRAY[]::organizations[])) AS r;
END;
$$;
REVOKE ALL ON FUNCTION platform_list_organizations() FROM PUBLIC;
GRANT EXECUTE ON FUNCTION platform_list_organizations() TO console_rt;

CREATE OR REPLACE FUNCTION platform_get_organization(p_id UUID)
RETURNS TABLE (
    id UUID, slug TEXT, name TEXT, status TEXT,
    created_at TIMESTAMPTZ, updated_at TIMESTAMPTZ
)
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = public, pg_temp
AS $$
DECLARE
    found organizations;
BEGIN
    SET LOCAL row_security = off;
    SELECT o.* INTO found FROM organizations o WHERE o.id = p_id;
    SET LOCAL row_security = on;

    IF found.id IS NULL THEN
        RETURN;
    END IF;
    RETURN QUERY
        SELECT found.id, found.slug, found.name, found.status,
               found.created_at, found.updated_at;
END;
$$;
REVOKE ALL ON FUNCTION platform_get_organization(UUID) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION platform_get_organization(UUID) TO console_rt;

CREATE OR REPLACE FUNCTION platform_set_organization_status(
    p_id UUID,
    p_status TEXT
)
RETURNS UUID
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = public, pg_temp
AS $$
DECLARE
    updated_id UUID;
BEGIN
    IF p_status NOT IN ('ACTIVE', 'SUSPENDED', 'ARCHIVED') THEN
        RAISE EXCEPTION 'invalid tenant status %', p_status;
    END IF;
    SET LOCAL row_security = off;
    UPDATE organizations
    SET status = p_status, updated_at = now()
    WHERE id = p_id
    RETURNING id INTO updated_id;
    SET LOCAL row_security = on;
    RETURN updated_id;  -- NULL when no such org
END;
$$;
REVOKE ALL ON FUNCTION platform_set_organization_status(UUID, TEXT) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION platform_set_organization_status(UUID, TEXT) TO console_rt;

-- ---------------------------------------------------------------------------
-- (3) Re-home the cold-start admin to the PLATFORM tier.
--
-- The deploy-time global `CONSOLE_COLDSTART_OTP` now bootstraps the PLATFORM admin
-- (the first account ABOVE all tenants), NOT a KNL tenant admin. The cold-start
-- admin row was created org-less in 0021 and backfilled to KNL in 0028; move it
-- to the platform sentinel org `...00face` (console_kernel_core::OrgId::platform()),
-- which is deliberately NOT a real tenant. A user whose `org_id` is this sentinel
-- is the platform admin; login mints a PLATFORM token for it (auth-rest stamps
-- `platform = true` when the user's org is the sentinel).
--
-- KNL's OWN tenant admin is NOT this row: KNL is onboarded as tenant #1 through
-- the platform onboarding flow (POST /platform/orgs), which seeds a fresh
-- per-org SUPER_ADMIN + a per-org cold-start OTP. See deploy/SECRETS.md.
--
-- `users` is FORCE RLS (0030) AND carries the org_id-immutability BEFORE UPDATE
-- trigger (0031), and the cold-start admin currently carries the KNL org. So we
-- cannot UPDATE its org_id to the sentinel (the trigger aborts) and cannot span
-- two org values under one RLS GUC. Re-home it by DELETE + re-INSERT instead
-- (the trigger is UPDATE-only; DELETE/INSERT are unaffected), running as the
-- migration owner with `row_security = off` so the privileged one-off bootstrap
-- re-home is not blocked by RLS. This is a real `DO` block so the SET LOCAL has a
-- transaction. Idempotent: only the KNL-homed cold-start admin is moved, and the
-- re-INSERT preserves the same row id so any (not-yet-existing at first boot)
-- references stay valid.
DO $$
DECLARE
    cold_admin RECORD;
BEGIN
    SET LOCAL row_security = off;

    -- The platform sentinel needs an `organizations` row so the FK from
    -- `users.org_id` is satisfiable for the platform admin. It is NOT a tenant:
    -- its id is the reserved sentinel and no tenant data ever carries it, but a
    -- physical row must exist for referential integrity. Status ARCHIVED so it can
    -- never be mistaken for an onboardable/active tenant in the platform list.
    INSERT INTO organizations (id, slug, name, status)
    VALUES (
        '00000000-0000-0000-0000-00000000face'::uuid,
        'platform', 'Platform (vendor tier)', 'ARCHIVED'
    )
    ON CONFLICT (id) DO NOTHING;

    SELECT id, display_name, phone, team, roles, is_active
    INTO cold_admin
    FROM users
    WHERE display_name = 'Cold Start Admin'
      AND roles @> ARRAY['SUPER_ADMIN']::TEXT[]
      AND org_id = '00000000-0000-0000-0000-0000000000a1'::uuid
    LIMIT 1;

    IF cold_admin.id IS NOT NULL THEN
        -- The re-home is a DELETE+re-INSERT (org_id is immutable to UPDATE), but
        -- the cold admin owns an already-revoked auth_bootstrap_credentials row
        -- (seeded in 0021, revoked-not-removed in 0023). That row's same-org
        -- composite FK is ON DELETE RESTRICT, so it blocks the user delete. Remove
        -- it first: it is dead (revoked) and the boot-time seeder re-issues a fresh
        -- platform-admin OTP under the sentinel org. Any other future child of this
        -- bootstrap-only user would need the same treatment, but it has none yet.
        DELETE FROM auth_bootstrap_credentials WHERE user_id = cold_admin.id;
        DELETE FROM users WHERE id = cold_admin.id;
        INSERT INTO users (id, display_name, phone, team, roles, is_active, org_id)
        VALUES (
            cold_admin.id, cold_admin.display_name, cold_admin.phone,
            cold_admin.team, cold_admin.roles, cold_admin.is_active,
            '00000000-0000-0000-0000-00000000face'::uuid
        );
    END IF;
END
$$;
