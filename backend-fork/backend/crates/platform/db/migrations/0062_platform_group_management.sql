-- Platform group management: audited REST handlers call these narrow SECURITY
-- DEFINER functions so the runtime role can manage group identity + membership
-- without raw access to owner-only group_memberships/group_role_grants.
--
-- Security posture stays the same as 0060:
--   * groups = global identity metadata only.
--   * group_memberships = owner-only auth/topology table; no raw console_rt access.
--   * platform functions return identity/member summaries only, never tenant rows.

CREATE OR REPLACE FUNCTION platform_list_groups()
RETURNS TABLE (
    id UUID,
    slug TEXT,
    name TEXT,
    status TEXT,
    created_at TIMESTAMPTZ,
    updated_at TIMESTAMPTZ,
    member_count BIGINT,
    members JSONB
)
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = public, pg_temp
AS $$
BEGIN
    SET LOCAL row_security = off;

    RETURN QUERY
        SELECT
            g.id,
            g.slug,
            g.name,
            g.status,
            g.created_at,
            g.updated_at,
            COUNT(o.id)::BIGINT AS member_count,
            COALESCE(
                jsonb_agg(
                    jsonb_build_object(
                        'id', o.id,
                        'slug', o.slug,
                        'name', o.name,
                        'status', o.status
                    )
                    ORDER BY o.created_at ASC, o.id ASC
                ) FILTER (WHERE o.id IS NOT NULL),
                '[]'::jsonb
            ) AS members
        FROM groups g
        LEFT JOIN organizations o
            ON o.group_id = g.id
           AND o.id <> '00000000-0000-0000-0000-00000000face'::uuid
        GROUP BY g.id
        ORDER BY g.created_at ASC, g.id ASC;

    SET LOCAL row_security = on;
EXCEPTION WHEN OTHERS THEN
    SET LOCAL row_security = on;
    RAISE;
END;
$$;
REVOKE ALL ON FUNCTION platform_list_groups() FROM PUBLIC;
GRANT EXECUTE ON FUNCTION platform_list_groups() TO console_rt;

CREATE OR REPLACE FUNCTION platform_get_group(p_id UUID)
RETURNS TABLE (
    id UUID,
    slug TEXT,
    name TEXT,
    status TEXT,
    created_at TIMESTAMPTZ,
    updated_at TIMESTAMPTZ,
    member_count BIGINT,
    members JSONB
)
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = public, pg_temp
AS $$
BEGIN
    SET LOCAL row_security = off;

    RETURN QUERY
        SELECT
            g.id,
            g.slug,
            g.name,
            g.status,
            g.created_at,
            g.updated_at,
            COUNT(o.id)::BIGINT AS member_count,
            COALESCE(
                jsonb_agg(
                    jsonb_build_object(
                        'id', o.id,
                        'slug', o.slug,
                        'name', o.name,
                        'status', o.status
                    )
                    ORDER BY o.created_at ASC, o.id ASC
                ) FILTER (WHERE o.id IS NOT NULL),
                '[]'::jsonb
            ) AS members
        FROM groups g
        LEFT JOIN organizations o
            ON o.group_id = g.id
           AND o.id <> '00000000-0000-0000-0000-00000000face'::uuid
        WHERE g.id = p_id
        GROUP BY g.id;

    SET LOCAL row_security = on;
EXCEPTION WHEN OTHERS THEN
    SET LOCAL row_security = on;
    RAISE;
END;
$$;
REVOKE ALL ON FUNCTION platform_get_group(UUID) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION platform_get_group(UUID) TO console_rt;

CREATE OR REPLACE FUNCTION platform_create_group(p_slug TEXT, p_name TEXT)
RETURNS UUID
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = public, pg_temp
AS $$
DECLARE
    v_id UUID;
BEGIN
    SET LOCAL row_security = off;

    INSERT INTO groups (slug, name)
    VALUES (btrim(p_slug), btrim(p_name))
    RETURNING id INTO v_id;

    SET LOCAL row_security = on;
    RETURN v_id;
EXCEPTION WHEN OTHERS THEN
    SET LOCAL row_security = on;
    RAISE;
END;
$$;
REVOKE ALL ON FUNCTION platform_create_group(TEXT, TEXT) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION platform_create_group(TEXT, TEXT) TO console_rt;

CREATE OR REPLACE FUNCTION platform_update_group(
    p_id UUID,
    p_name TEXT DEFAULT NULL,
    p_status TEXT DEFAULT NULL
)
RETURNS UUID
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = public, pg_temp
AS $$
DECLARE
    v_id UUID;
BEGIN
    SET LOCAL row_security = off;

    UPDATE groups
    SET
        name = COALESCE(NULLIF(btrim(p_name), ''), name),
        status = COALESCE(p_status, status),
        updated_at = now()
    WHERE id = p_id
    RETURNING id INTO v_id;

    SET LOCAL row_security = on;
    RETURN v_id;
EXCEPTION WHEN OTHERS THEN
    SET LOCAL row_security = on;
    RAISE;
END;
$$;
REVOKE ALL ON FUNCTION platform_update_group(UUID, TEXT, TEXT) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION platform_update_group(UUID, TEXT, TEXT) TO console_rt;

CREATE OR REPLACE FUNCTION platform_assign_org_to_group(p_group_id UUID, p_org_id UUID)
RETURNS UUID
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = public, pg_temp
AS $$
DECLARE
    v_group_status TEXT;
    v_org_id UUID;
BEGIN
    SET LOCAL row_security = off;

    SELECT status INTO v_group_status
    FROM groups
    WHERE id = p_group_id;
    IF v_group_status IS NULL THEN
        SET LOCAL row_security = on;
        RETURN NULL;
    END IF;
    IF v_group_status <> 'ACTIVE' THEN
        RAISE EXCEPTION 'group % is not active', p_group_id
            USING ERRCODE = '23514';
    END IF;

    SELECT id INTO v_org_id
    FROM organizations
    WHERE id = p_org_id
      AND id <> '00000000-0000-0000-0000-00000000face'::uuid;
    IF v_org_id IS NULL THEN
        SET LOCAL row_security = on;
        RETURN NULL;
    END IF;

    INSERT INTO group_memberships (group_id, org_id)
    VALUES (p_group_id, p_org_id)
    ON CONFLICT (org_id) DO UPDATE
        SET group_id = EXCLUDED.group_id,
            created_at = now();

    UPDATE organizations
    SET group_id = p_group_id,
        updated_at = now()
    WHERE id = p_org_id;

    SET LOCAL row_security = on;
    RETURN p_org_id;
EXCEPTION WHEN OTHERS THEN
    SET LOCAL row_security = on;
    RAISE;
END;
$$;
REVOKE ALL ON FUNCTION platform_assign_org_to_group(UUID, UUID) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION platform_assign_org_to_group(UUID, UUID) TO console_rt;

CREATE OR REPLACE FUNCTION platform_remove_org_from_group(p_group_id UUID, p_org_id UUID)
RETURNS UUID
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = public, pg_temp
AS $$
DECLARE
    v_org_id UUID;
BEGIN
    SET LOCAL row_security = off;

    SELECT id INTO v_org_id
    FROM organizations
    WHERE id = p_org_id
      AND id <> '00000000-0000-0000-0000-00000000face'::uuid;
    IF v_org_id IS NULL THEN
        SET LOCAL row_security = on;
        RETURN NULL;
    END IF;

    DELETE FROM group_memberships
    WHERE group_id = p_group_id
      AND org_id = p_org_id;

    UPDATE organizations
    SET group_id = NULL,
        updated_at = now()
    WHERE id = p_org_id
      AND group_id = p_group_id;

    SET LOCAL row_security = on;
    RETURN p_org_id;
EXCEPTION WHEN OTHERS THEN
    SET LOCAL row_security = on;
    RAISE;
END;
$$;
REVOKE ALL ON FUNCTION platform_remove_org_from_group(UUID, UUID) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION platform_remove_org_from_group(UUID, UUID) TO console_rt;
