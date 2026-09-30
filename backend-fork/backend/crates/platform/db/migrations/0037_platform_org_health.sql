-- Platform ops dashboard: a cross-tenant org-health rollup.
--
-- The platform tier needs per-tenant health/usage numbers (user count, active
-- work-order count, last activity) for EVERY tenant in one read. A tenant-scoped
-- query under RLS can only ever see one org, so this aggregation runs as a
-- SECURITY DEFINER function that briefly disables row_security to scan all
-- tenants — exactly like the existing `platform_list_organizations()`.
--
-- SECURITY: this is the ONLY sanctioned cross-tenant read path for ops health.
-- It NEVER returns row-level tenant data, only per-org COUNTs and a timestamp,
-- and the platform sentinel org is excluded. The calling Rust layer
-- (`PlatformProvisioner::list_tenant_health`) wraps the call in an audited
-- transaction (`platform.tenant.health` audit event), so every cross-tenant read
-- is recorded. `row_security` is restored to `on` immediately after the scan so
-- nothing else in the transaction runs unscoped.
CREATE OR REPLACE FUNCTION platform_org_health()
RETURNS TABLE (
    id                  UUID,
    slug                TEXT,
    name                TEXT,
    status              TEXT,
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
            COALESCE(u.total, 0)        AS user_count,
            COALESCE(u.active, 0)       AS active_user_count,
            COALESCE(w.active, 0)       AS active_work_orders,
            COALESCE(w.open_total, 0)   AS open_work_orders,
            GREATEST(u.last_user_at, w.last_wo_at, a.last_audit_at) AS last_activity_at
        FROM organizations o
        LEFT JOIN (
            SELECT
                users.org_id                          AS org_id,
                COUNT(*)                              AS total,
                COUNT(*) FILTER (WHERE users.is_active) AS active,
                MAX(users.created_at)                 AS last_user_at
            FROM users
            GROUP BY users.org_id
        ) u ON u.org_id = o.id
        LEFT JOIN (
            SELECT
                work_orders.org_id                    AS org_id,
                COUNT(*) FILTER (
                    WHERE work_orders.status IN (
                        'IN_PROGRESS', 'REPORT_SUBMITTED', 'ADMIN_REVIEW', 'ASSIGNED'
                    )
                )                                     AS active,
                COUNT(*) FILTER (
                    WHERE work_orders.status NOT IN (
                        'FINAL_COMPLETED', 'REJECTED', 'ARCHIVED', 'CANCELLED'
                    )
                )                                     AS open_total,
                MAX(work_orders.updated_at)           AS last_wo_at
            FROM work_orders
            GROUP BY work_orders.org_id
        ) w ON w.org_id = o.id
        LEFT JOIN (
            SELECT
                audit_events.org_id                   AS org_id,
                MAX(audit_events.occurred_at)         AS last_audit_at
            FROM audit_events
            WHERE audit_events.org_id IS NOT NULL
            GROUP BY audit_events.org_id
        ) a ON a.org_id = o.id
        -- The platform sentinel is not a tenant; never report on it.
        WHERE o.id <> '00000000-0000-0000-0000-00000000face'::uuid
        ORDER BY o.created_at ASC, o.id ASC;

    SET LOCAL row_security = on;
END;
$$;
REVOKE ALL ON FUNCTION platform_org_health() FROM PUBLIC;
GRANT EXECUTE ON FUNCTION platform_org_health() TO console_rt;
