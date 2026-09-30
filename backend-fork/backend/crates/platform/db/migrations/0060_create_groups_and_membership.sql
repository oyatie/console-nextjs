-- Org hierarchy P0: Group -> 법인 membership + owner-only group authorization.
--
-- Security posture:
--   * A Group is NOT a tenant and never arms `app.current_org`.
--   * `groups` is global identity metadata only (id/slug/name/status).
--   * `group_memberships` and `group_role_grants` are owner-only authorization
--     tables. Runtime `console_rt` has NO raw SELECT on them; it only gets narrow
--     SECURITY DEFINER resolvers that restore row_security before returning.
--   * Consolidated reads still run as N per-member `with_org_conn` calls later;
--     this migration only provides the identity/member resolver inputs.

-- console-gate: global-table groups (rationale: group identity metadata only; no tenant data or auth grants)
CREATE TABLE groups (
    id         UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    slug       TEXT        NOT NULL UNIQUE
                   CHECK (slug ~ '^[a-z0-9][a-z0-9-]{1,38}[a-z0-9]$'),
    name       TEXT        NOT NULL CHECK (name <> ''),
    status     TEXT        NOT NULL DEFAULT 'ACTIVE'
                   CHECK (status IN ('ACTIVE', 'SUSPENDED', 'ARCHIVED')),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Nullable/backward-compatible: an ungrouped 법인 behaves exactly as before.
-- Membership authorization is still resolved through the owner-only table below.
ALTER TABLE organizations
    ADD COLUMN group_id UUID NULL REFERENCES groups(id) ON DELETE RESTRICT;
CREATE INDEX idx_organizations_group ON organizations (group_id) WHERE group_id IS NOT NULL;

-- console-gate: owner-only-table group_memberships (rationale: cross-tenant membership authorization; runtime resolves through group_member_org_ids only)
CREATE TABLE group_memberships (
    group_id   UUID        NOT NULL REFERENCES groups(id) ON DELETE CASCADE,
    org_id     UUID        NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (group_id, org_id),
    UNIQUE (org_id)
);

-- console-gate: owner-only-table group_role_grants (rationale: cross-tenant group-role authorization; runtime resolves own grants only)
CREATE TABLE group_role_grants (
    id         UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    group_id   UUID        NOT NULL REFERENCES groups(id) ON DELETE CASCADE,
    user_id    UUID        NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    group_role TEXT        NOT NULL
                   CHECK (group_role IN ('GROUP_ADMIN', 'GROUP_VIEWER', 'GROUP_FINANCE')),
    granted_by UUID        NULL REFERENCES users(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (group_id, user_id, group_role)
);
CREATE INDEX idx_group_role_grants_user ON group_role_grants (user_id);

-- Migration 0031 set future-table default privileges to console_rt. Revoke those
-- inherited grants on the owner-only authorization tables, then grant only the
-- safe identity columns from `groups`.
REVOKE ALL ON groups, group_memberships, group_role_grants FROM console_rt;
REVOKE ALL ON groups, group_memberships, group_role_grants FROM PUBLIC;
GRANT SELECT (id, slug, name, status) ON groups TO console_rt;

-- rls-arming: ok identity-only SECURITY DEFINER resolver
CREATE OR REPLACE FUNCTION group_member_org_ids(p_group UUID, p_actor UUID)
RETURNS TABLE (org_id UUID, slug TEXT, name TEXT, status TEXT)
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
        JOIN group_memberships m ON m.org_id = o.id
    WHERE m.group_id = p_group
      AND o.status = 'ACTIVE'
      AND o.id <> '00000000-0000-0000-0000-00000000face'::uuid
      AND EXISTS (
          SELECT 1
          FROM group_role_grants g
              JOIN users u ON u.id = g.user_id
          WHERE g.group_id = p_group
            AND g.user_id = p_actor
            AND u.is_active
      );
    SET LOCAL row_security = on;

    RETURN QUERY
        SELECT r.id, r.slug, r.name, r.status
        FROM unnest(COALESCE(result_rows, ARRAY[]::organizations[])) AS r;
EXCEPTION WHEN OTHERS THEN
    SET LOCAL row_security = on;
    RAISE;
END;
$$;
REVOKE ALL ON FUNCTION group_member_org_ids(UUID, UUID) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION group_member_org_ids(UUID, UUID) TO console_rt;

-- rls-arming: ok identity-only SECURITY DEFINER resolver
CREATE OR REPLACE FUNCTION group_role_grants_for_user(p_user UUID)
RETURNS TABLE (group_id UUID, group_role TEXT)
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = public, pg_temp
AS $$
DECLARE
    result_rows group_role_grants[];
BEGIN
    SET LOCAL row_security = off;
    SELECT array_agg(g ORDER BY g.group_id, g.group_role)
    INTO result_rows
    FROM group_role_grants g
        JOIN users u ON u.id = g.user_id
    WHERE g.user_id = p_user
      AND u.is_active;
    SET LOCAL row_security = on;

    RETURN QUERY
        SELECT r.group_id, r.group_role
        FROM unnest(COALESCE(result_rows, ARRAY[]::group_role_grants[])) AS r;
EXCEPTION WHEN OTHERS THEN
    SET LOCAL row_security = on;
    RAISE;
END;
$$;
REVOKE ALL ON FUNCTION group_role_grants_for_user(UUID) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION group_role_grants_for_user(UUID) TO console_rt;
