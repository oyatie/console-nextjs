-- Mandatory Group-of-one: every organization has a real groups row and
-- organizations.group_id is NOT NULL. Backfill ungrouped rows, INSERT
-- triggers mint Group-of-one for subsequent owner INSERTs, and
-- platform_remove_org_from_group remints instead of SET NULL.
--
-- Production writer of organizations remains platform_create_organization
-- (body unchanged). Mint DML lives in ungranted DEFINER helpers.

CREATE OR REPLACE FUNCTION platform_mint_group_row(
    p_org_id UUID,
    p_name TEXT,
    p_status TEXT
)
RETURNS UUID
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = public, pg_temp
AS $$
DECLARE
    v_row_security TEXT := current_setting('row_security');
    v_hex TEXT := replace(p_org_id::text, '-', '');
    v_slug TEXT;
    v_group_id UUID;
    v_status TEXT;
    v_i INT;
    v_sentinel UUID := '00000000-0000-0000-0000-00000000face';
BEGIN
    SET LOCAL row_security = off;

    v_status := CASE
        WHEN p_org_id = v_sentinel THEN 'ARCHIVED'
        ELSE CASE
            WHEN COALESCE(p_status, '') IN ('ACTIVE', 'SUSPENDED', 'ARCHIVED')
            THEN p_status
            ELSE 'ACTIVE'
        END
    END;

    -- i=1 → go-{32hex}; i=2..9 → g{i}-{32hex}. All 35 chars, CHECK-valid.
    FOR v_i IN 1..9 LOOP
        IF v_i = 1 THEN
            v_slug := 'go-' || v_hex;
        ELSE
            v_slug := 'g' || v_i::text || '-' || v_hex;
        END IF;

        SELECT g.id INTO v_group_id
        FROM groups g
        WHERE g.slug = v_slug
          AND NOT EXISTS (
              SELECT 1 FROM group_memberships m
              WHERE m.group_id = g.id AND m.org_id <> p_org_id
          );
        IF v_group_id IS NOT NULL THEN
            PERFORM set_config('row_security', v_row_security, true);
            RETURN v_group_id;
        END IF;

        INSERT INTO groups (slug, name, status)
        VALUES (v_slug, p_name, v_status)
        ON CONFLICT (slug) DO NOTHING
        RETURNING id INTO v_group_id;
        IF v_group_id IS NOT NULL THEN
            PERFORM set_config('row_security', v_row_security, true);
            RETURN v_group_id;
        END IF;

        -- Lost the INSERT (concurrent winner or foreign occupant).
        -- Reuse if the occupant is empty/self; else next suffix.
        SELECT g.id INTO v_group_id
        FROM groups g
        WHERE g.slug = v_slug
          AND NOT EXISTS (
              SELECT 1 FROM group_memberships m
              WHERE m.group_id = g.id AND m.org_id <> p_org_id
          );
        IF v_group_id IS NOT NULL THEN
            PERFORM set_config('row_security', v_row_security, true);
            RETURN v_group_id;
        END IF;
    END LOOP;

    PERFORM set_config('row_security', v_row_security, true);
    RAISE EXCEPTION 'unable to mint group-of-one slug for org %', p_org_id
        USING ERRCODE = '23505';
EXCEPTION WHEN OTHERS THEN
    PERFORM set_config('row_security', v_row_security, true);
    RAISE;
END;
$$;
REVOKE ALL ON FUNCTION platform_mint_group_row(UUID, TEXT, TEXT) FROM PUBLIC;

CREATE OR REPLACE FUNCTION platform_attach_membership(
    p_group_id UUID,
    p_org_id UUID
)
RETURNS void
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = public, pg_temp
AS $$
DECLARE
    v_row_security TEXT := current_setting('row_security');
BEGIN
    SET LOCAL row_security = off;
    INSERT INTO group_memberships (group_id, org_id)
    VALUES (p_group_id, p_org_id)
    ON CONFLICT (org_id) DO UPDATE
        SET group_id = EXCLUDED.group_id,
            created_at = CASE
                WHEN group_memberships.group_id = EXCLUDED.group_id
                THEN group_memberships.created_at
                ELSE now()
            END;
    PERFORM set_config('row_security', v_row_security, true);
EXCEPTION WHEN OTHERS THEN
    PERFORM set_config('row_security', v_row_security, true);
    RAISE;
END;
$$;
REVOKE ALL ON FUNCTION platform_attach_membership(UUID, UUID) FROM PUBLIC;

CREATE OR REPLACE FUNCTION platform_attach_group_of_one(p_org_id UUID)
RETURNS UUID
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = public, pg_temp
AS $$
DECLARE
    v_row_security TEXT := current_setting('row_security');
    v_org organizations%ROWTYPE;
    v_group_id UUID;
BEGIN
    SET LOCAL row_security = off;

    SELECT * INTO v_org FROM organizations WHERE id = p_org_id;
    IF v_org.id IS NULL THEN
        PERFORM set_config('row_security', v_row_security, true);
        RAISE EXCEPTION 'organization % not found', p_org_id USING ERRCODE = '23503';
    END IF;

    v_group_id := platform_mint_group_row(p_org_id, v_org.name, v_org.status);

    UPDATE organizations
       SET group_id = v_group_id, updated_at = now()
     WHERE id = p_org_id;

    PERFORM platform_attach_membership(v_group_id, p_org_id);

    PERFORM set_config('row_security', v_row_security, true);
    RETURN v_group_id;
EXCEPTION WHEN OTHERS THEN
    PERFORM set_config('row_security', v_row_security, true);
    RAISE;
END;
$$;
REVOKE ALL ON FUNCTION platform_attach_group_of_one(UUID) FROM PUBLIC;

CREATE OR REPLACE FUNCTION platform_mint_missing_group_of_one(p_org_id UUID)
RETURNS UUID
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = public, pg_temp
AS $$
DECLARE
    v_row_security TEXT := current_setting('row_security');
    v_group_id UUID;
BEGIN
    SET LOCAL row_security = off;
    SELECT group_id INTO v_group_id FROM organizations WHERE id = p_org_id;
    IF v_group_id IS NOT NULL THEN
        PERFORM set_config('row_security', v_row_security, true);
        RETURN v_group_id;
    END IF;
    v_group_id := platform_attach_group_of_one(p_org_id);
    PERFORM set_config('row_security', v_row_security, true);
    RETURN v_group_id;
EXCEPTION WHEN OTHERS THEN
    PERFORM set_config('row_security', v_row_security, true);
    RAISE;
END;
$$;
REVOKE ALL ON FUNCTION platform_mint_missing_group_of_one(UUID) FROM PUBLIC;

CREATE OR REPLACE FUNCTION organizations_before_insert_mint_group()
RETURNS TRIGGER
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = public, pg_temp
AS $$
BEGIN
    IF NEW.group_id IS NULL THEN
        NEW.group_id := platform_mint_group_row(NEW.id, NEW.name, NEW.status);
    END IF;
    RETURN NEW;
END;
$$;
REVOKE ALL ON FUNCTION organizations_before_insert_mint_group() FROM PUBLIC;

CREATE OR REPLACE FUNCTION organizations_after_insert_attach_membership()
RETURNS TRIGGER
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = public, pg_temp
AS $$
BEGIN
    PERFORM platform_attach_membership(NEW.group_id, NEW.id);
    RETURN NEW;
END;
$$;
REVOKE ALL ON FUNCTION organizations_after_insert_attach_membership() FROM PUBLIC;

CREATE TRIGGER trg_organizations_before_insert_mint_group
    BEFORE INSERT ON organizations
    FOR EACH ROW
    WHEN (NEW.group_id IS NULL)
    EXECUTE FUNCTION organizations_before_insert_mint_group();

CREATE TRIGGER trg_organizations_after_insert_attach_membership
    AFTER INSERT ON organizations
    FOR EACH ROW
    EXECUTE FUNCTION organizations_after_insert_attach_membership();

DO $$
DECLARE
    r RECORD;
BEGIN
    SET LOCAL row_security = off;

    FOR r IN
        SELECT id
        FROM organizations
        WHERE group_id IS NULL
        ORDER BY id
    LOOP
        PERFORM platform_mint_missing_group_of_one(r.id);
    END LOOP;

    INSERT INTO group_memberships (group_id, org_id)
    SELECT group_id, id
    FROM organizations
    WHERE group_id IS NOT NULL
    ON CONFLICT (org_id) DO UPDATE
        SET group_id = EXCLUDED.group_id;

    -- SET LOCAL off lasts until the outer sqlx migration transaction commits.
    -- Do not SET LOCAL row_security = on here.
END
$$;

ALTER TABLE organizations ALTER COLUMN group_id SET NOT NULL;

DROP INDEX IF EXISTS idx_organizations_group;
CREATE INDEX idx_organizations_group ON organizations (group_id);

CREATE OR REPLACE FUNCTION platform_remove_org_from_group(p_group_id UUID, p_org_id UUID)
RETURNS UUID
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = public, pg_temp
AS $$
DECLARE
    v_org_id UUID;
    v_deleted INT;
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
    GET DIAGNOSTICS v_deleted = ROW_COUNT;

    IF v_deleted = 0 THEN
        SET LOCAL row_security = on;
        RETURN p_org_id;
    END IF;

    PERFORM platform_attach_group_of_one(p_org_id);

    SET LOCAL row_security = on;
    RETURN p_org_id;
EXCEPTION WHEN OTHERS THEN
    SET LOCAL row_security = on;
    RAISE;
END;
$$;
REVOKE ALL ON FUNCTION platform_remove_org_from_group(UUID, UUID) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION platform_remove_org_from_group(UUID, UUID) TO console_rt;

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
        WHERE g.id IS DISTINCT FROM (
            SELECT o2.group_id
            FROM organizations o2
            WHERE o2.id = '00000000-0000-0000-0000-00000000face'::uuid
        )
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

COMMENT ON FUNCTION platform_create_organization(TEXT, TEXT) IS
    'Creates a tenant organization by INSERT (id, slug, name) only. A BEFORE INSERT trigger fills NEW.group_id via platform_mint_group_row; an AFTER INSERT trigger attaches group_memberships. Do not PERFORM platform_attach_group_of_one from this body.';
