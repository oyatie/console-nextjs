-- Platform group account management.
--
-- A Group is still NOT a tenant. These SECURITY DEFINER helpers expose only the
-- narrow operations the platform console needs:
--   * edit group identity (including slug);
--   * list group-role grants with the user's home organization identity;
--   * create one tenant-anchored user account and grant one group role;
--   * revoke one group role.
--
-- Raw group_memberships/group_role_grants remain owner-only. Runtime code never
-- receives direct access to those cross-tenant authorization tables.

CREATE OR REPLACE FUNCTION platform_update_group(
    p_id UUID,
    p_slug TEXT DEFAULT NULL,
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
        slug = COALESCE(NULLIF(btrim(p_slug), ''), slug),
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
REVOKE ALL ON FUNCTION platform_update_group(UUID, TEXT, TEXT, TEXT) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION platform_update_group(UUID, TEXT, TEXT, TEXT) TO console_rt;

CREATE OR REPLACE FUNCTION platform_list_group_accounts(p_group_id UUID)
RETURNS TABLE (
    user_id UUID,
    display_name TEXT,
    phone TEXT,
    tenant_roles TEXT[],
    is_active BOOLEAN,
    has_passkey BOOLEAN,
    account_status TEXT,
    org_id UUID,
    org_slug TEXT,
    org_name TEXT,
    group_roles TEXT[],
    created_at TIMESTAMPTZ
)
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = public, pg_temp
AS $$
BEGIN
    SET LOCAL row_security = off;

    RETURN QUERY
        SELECT
            u.id AS user_id,
            u.display_name,
            u.phone,
            u.roles AS tenant_roles,
            u.is_active,
            EXISTS (
                SELECT 1
                FROM auth_webauthn_credentials c
                WHERE c.user_id = u.id
            ) AS has_passkey,
            CASE
                WHEN NOT u.is_active THEN 'DEACTIVATED'
                WHEN EXISTS (
                    SELECT 1
                    FROM auth_webauthn_credentials c
                    WHERE c.user_id = u.id
                ) THEN 'ACTIVE'
                ELSE 'PENDING_SETUP'
            END AS account_status,
            o.id AS org_id,
            o.slug AS org_slug,
            o.name AS org_name,
            array_agg(gr.group_role ORDER BY gr.group_role) AS group_roles,
            u.created_at
        FROM group_role_grants gr
        JOIN group_memberships gm
          ON gm.group_id = gr.group_id
        JOIN users u
          ON u.id = gr.user_id
         AND u.org_id = gm.org_id
        JOIN organizations o
          ON o.id = u.org_id
         AND o.id <> '00000000-0000-0000-0000-00000000face'::uuid
        WHERE gr.group_id = p_group_id
        GROUP BY
            u.id,
            u.display_name,
            u.phone,
            u.roles,
            u.is_active,
            o.id,
            o.slug,
            o.name,
            u.created_at
        ORDER BY o.name ASC, u.display_name ASC, u.id ASC;

    SET LOCAL row_security = on;
EXCEPTION WHEN OTHERS THEN
    SET LOCAL row_security = on;
    RAISE;
END;
$$;
REVOKE ALL ON FUNCTION platform_list_group_accounts(UUID) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION platform_list_group_accounts(UUID) TO console_rt;

CREATE OR REPLACE FUNCTION platform_create_group_account(
    p_group_id UUID,
    p_org_id UUID,
    p_display_name TEXT,
    p_phone TEXT DEFAULT NULL,
    p_tenant_roles TEXT[] DEFAULT ARRAY['MEMBER']::TEXT[],
    p_group_role TEXT DEFAULT 'GROUP_ADMIN',
    p_granted_by UUID DEFAULT NULL
)
RETURNS UUID
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = public, pg_temp
AS $$
DECLARE
    v_group_status TEXT;
    v_org_id UUID;
    v_user_id UUID;
    v_roles TEXT[] := COALESCE(p_tenant_roles, ARRAY['MEMBER']::TEXT[]);
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

    SELECT o.id INTO v_org_id
    FROM organizations o
    JOIN group_memberships gm
      ON gm.org_id = o.id
     AND gm.group_id = p_group_id
    WHERE o.id = p_org_id
      AND o.status = 'ACTIVE'
      AND o.id <> '00000000-0000-0000-0000-00000000face'::uuid;
    IF v_org_id IS NULL THEN
        SET LOCAL row_security = on;
        RETURN NULL;
    END IF;

    IF NULLIF(btrim(p_display_name), '') IS NULL THEN
        RAISE EXCEPTION 'display_name is required'
            USING ERRCODE = '23514';
    END IF;
    IF cardinality(v_roles) = 0 THEN
        RAISE EXCEPTION 'tenant role is required'
            USING ERRCODE = '23514';
    END IF;

    PERFORM set_config('app.current_org', p_org_id::text, true);

    INSERT INTO users (display_name, phone, roles, is_active, org_id)
    VALUES (
        btrim(p_display_name),
        NULLIF(btrim(p_phone), ''),
        v_roles,
        true,
        p_org_id
    )
    RETURNING id INTO v_user_id;

    INSERT INTO group_role_grants (group_id, user_id, group_role, granted_by)
    VALUES (p_group_id, v_user_id, p_group_role, p_granted_by);

    SET LOCAL row_security = on;
    RETURN v_user_id;
EXCEPTION WHEN OTHERS THEN
    SET LOCAL row_security = on;
    RAISE;
END;
$$;
REVOKE ALL ON FUNCTION platform_create_group_account(UUID, UUID, TEXT, TEXT, TEXT[], TEXT, UUID) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION platform_create_group_account(UUID, UUID, TEXT, TEXT, TEXT[], TEXT, UUID) TO console_rt;

CREATE OR REPLACE FUNCTION platform_revoke_group_role(
    p_group_id UUID,
    p_user_id UUID,
    p_group_role TEXT
)
RETURNS UUID
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = public, pg_temp
AS $$
DECLARE
    v_user_org UUID;
    v_deleted UUID;
BEGIN
    SET LOCAL row_security = off;

    SELECT u.org_id INTO v_user_org
    FROM users u
    JOIN group_memberships gm
      ON gm.org_id = u.org_id
     AND gm.group_id = p_group_id
    WHERE u.id = p_user_id;
    IF v_user_org IS NULL THEN
        SET LOCAL row_security = on;
        RETURN NULL;
    END IF;

    DELETE FROM group_role_grants
    WHERE group_id = p_group_id
      AND user_id = p_user_id
      AND group_role = p_group_role
    RETURNING user_id INTO v_deleted;

    SET LOCAL row_security = on;
    RETURN v_deleted;
EXCEPTION WHEN OTHERS THEN
    SET LOCAL row_security = on;
    RAISE;
END;
$$;
REVOKE ALL ON FUNCTION platform_revoke_group_role(UUID, UUID, TEXT) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION platform_revoke_group_role(UUID, UUID, TEXT) TO console_rt;
