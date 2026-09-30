-- Expose group identity on platform-only tenant list / ops reads.
--
-- Group identity is global metadata (id/slug/name/status) created in 0060. These
-- platform SECURITY DEFINER functions still return only org/group identity and
-- aggregate counts, never row-level tenant data. The Rust layer audits every
-- call exactly as before.

DROP FUNCTION IF EXISTS platform_list_organizations();
CREATE FUNCTION platform_list_organizations()
RETURNS TABLE (
    id UUID,
    slug TEXT,
    name TEXT,
    status TEXT,
    created_at TIMESTAMPTZ,
    updated_at TIMESTAMPTZ,
    group_id UUID,
    group_slug TEXT,
    group_name TEXT
)
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = public, pg_temp
AS $$
BEGIN
    SET LOCAL row_security = off;

    RETURN QUERY
        SELECT
            o.id,
            o.slug,
            o.name,
            o.status,
            o.created_at,
            o.updated_at,
            g.id AS group_id,
            g.slug AS group_slug,
            g.name AS group_name
        FROM organizations o
        LEFT JOIN groups g ON g.id = o.group_id
        -- The platform sentinel is not a tenant; never list it.
        WHERE o.id <> '00000000-0000-0000-0000-00000000face'::uuid
        ORDER BY o.created_at ASC, o.id ASC;

    SET LOCAL row_security = on;
EXCEPTION WHEN OTHERS THEN
    SET LOCAL row_security = on;
    RAISE;
END;
$$;
REVOKE ALL ON FUNCTION platform_list_organizations() FROM PUBLIC;
GRANT EXECUTE ON FUNCTION platform_list_organizations() TO console_rt;

DROP FUNCTION IF EXISTS platform_get_organization(UUID);
CREATE FUNCTION platform_get_organization(p_id UUID)
RETURNS TABLE (
    id UUID,
    slug TEXT,
    name TEXT,
    status TEXT,
    created_at TIMESTAMPTZ,
    updated_at TIMESTAMPTZ,
    group_id UUID,
    group_slug TEXT,
    group_name TEXT
)
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = public, pg_temp
AS $$
BEGIN
    SET LOCAL row_security = off;

    RETURN QUERY
        SELECT
            o.id,
            o.slug,
            o.name,
            o.status,
            o.created_at,
            o.updated_at,
            g.id AS group_id,
            g.slug AS group_slug,
            g.name AS group_name
        FROM organizations o
        LEFT JOIN groups g ON g.id = o.group_id
        WHERE o.id = p_id
          AND o.id <> '00000000-0000-0000-0000-00000000face'::uuid;

    SET LOCAL row_security = on;
EXCEPTION WHEN OTHERS THEN
    SET LOCAL row_security = on;
    RAISE;
END;
$$;
REVOKE ALL ON FUNCTION platform_get_organization(UUID) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION platform_get_organization(UUID) TO console_rt;

DROP FUNCTION IF EXISTS platform_org_health();
CREATE FUNCTION platform_org_health()
RETURNS TABLE (
    id                  UUID,
    slug                TEXT,
    name                TEXT,
    status              TEXT,
    group_id            UUID,
    group_slug          TEXT,
    group_name          TEXT,
    user_count          BIGINT,
    active_user_count   BIGINT,
    active_work_orders  BIGINT,
    open_work_orders    BIGINT,
    last_activity_at    TIMESTAMPTZ
)
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = public, pg_temp
AS $$
BEGIN
    SET LOCAL row_security = off;

    RETURN QUERY
        SELECT
            o.id,
            o.slug,
            o.name,
            o.status,
            g.id AS group_id,
            g.slug AS group_slug,
            g.name AS group_name,
            COALESCE(u.total, 0)        AS user_count,
            COALESCE(u.active, 0)       AS active_user_count,
            COALESCE(w.active, 0)       AS active_work_orders,
            COALESCE(w.open_total, 0)   AS open_work_orders,
            GREATEST(u.last_user_at, w.last_wo_at, a.last_audit_at) AS last_activity_at
        FROM organizations o
        LEFT JOIN groups g ON g.id = o.group_id
        LEFT JOIN (
            SELECT
                users.org_id                             AS org_id,
                COUNT(*)                                 AS total,
                COUNT(*) FILTER (WHERE users.is_active)  AS active,
                MAX(users.created_at)                    AS last_user_at
            FROM users
            GROUP BY users.org_id
        ) u ON u.org_id = o.id
        LEFT JOIN (
            SELECT
                work_orders.org_id AS org_id,
                COUNT(*) FILTER (
                    WHERE work_orders.status IN (
                        'IN_PROGRESS', 'REPORT_SUBMITTED', 'ADMIN_REVIEW', 'ASSIGNED'
                    )
                ) AS active,
                COUNT(*) FILTER (
                    WHERE work_orders.status NOT IN (
                        'FINAL_COMPLETED', 'REJECTED', 'ARCHIVED', 'CANCELLED'
                    )
                ) AS open_total,
                MAX(work_orders.updated_at) AS last_wo_at
            FROM work_orders
            GROUP BY work_orders.org_id
        ) w ON w.org_id = o.id
        LEFT JOIN (
            SELECT
                audit_events.org_id AS org_id,
                MAX(audit_events.occurred_at) AS last_audit_at
            FROM audit_events
            WHERE audit_events.org_id IS NOT NULL
            GROUP BY audit_events.org_id
        ) a ON a.org_id = o.id
        -- The platform sentinel is not a tenant; never report on it.
        WHERE o.id <> '00000000-0000-0000-0000-00000000face'::uuid
        ORDER BY o.created_at ASC, o.id ASC;

    SET LOCAL row_security = on;
EXCEPTION WHEN OTHERS THEN
    SET LOCAL row_security = on;
    RAISE;
END;
$$;
REVOKE ALL ON FUNCTION platform_org_health() FROM PUBLIC;
GRANT EXECUTE ON FUNCTION platform_org_health() TO console_rt;
