-- Tenant console route/RUM telemetry used by the carbon-copy ramp gates.
--
-- Browser clients send only cardinality-safe route templates and bounded labels.
-- Org/user identity is derived from the authenticated tenant token at ingestion;
-- platform operators read per-org aggregates through the SECURITY DEFINER rollup
-- below, never by scanning row-level telemetry from the platform API.

CREATE TABLE console_route_telemetry (
    id             UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id         UUID        NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    user_id        UUID        NOT NULL,
    event_kind     TEXT        NOT NULL CHECK (event_kind IN ('route_selection', 'rum_error', 'rum_perf')),
    route_surface  TEXT        NOT NULL CHECK (route_surface IN ('console', 'legacy')),
    route_path     TEXT        NOT NULL CHECK (
        char_length(route_path) BETWEEN 1 AND 120
        AND route_path ~ '^/[A-Za-z0-9._:/-]*$'
    ),
    release_cycle  TEXT        NOT NULL CHECK (
        char_length(release_cycle) BETWEEN 1 AND 80
        AND release_cycle ~ '^[A-Za-z0-9._:-]+$'
    ),
    duration_ms    INTEGER     NULL CHECK (duration_ms IS NULL OR duration_ms BETWEEN 0 AND 600000),
    error_name     TEXT        NULL CHECK (
        error_name IS NULL
        OR (char_length(error_name) BETWEEN 1 AND 80 AND error_name ~ '^[A-Za-z0-9._:-]+$')
    ),
    occurred_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    FOREIGN KEY (user_id, org_id) REFERENCES users(id, org_id) ON DELETE CASCADE
);

CREATE INDEX idx_console_route_telemetry_org_release
    ON console_route_telemetry (org_id, release_cycle, route_surface, event_kind);
CREATE INDEX idx_console_route_telemetry_org_occurred
    ON console_route_telemetry (org_id, occurred_at DESC);

ALTER TABLE console_route_telemetry ENABLE ROW LEVEL SECURITY;
ALTER TABLE console_route_telemetry FORCE ROW LEVEL SECURITY;
CREATE POLICY org_isolation ON console_route_telemetry
    USING (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid)
    WITH CHECK (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid);

GRANT SELECT, INSERT ON console_route_telemetry TO console_rt;
-- Migration 0031 default privileges grant broader table DML to console_rt. Telemetry
-- is append-only from the runtime role: no updates/deletes that could rewrite ramp
-- evidence for the D5 "two release cycles of zero legacy traffic" gate.
REVOKE UPDATE, DELETE ON console_route_telemetry FROM console_rt;

CREATE OR REPLACE FUNCTION platform_console_route_adoption()
RETURNS TABLE (
    org_id                UUID,
    release_cycle         TEXT,
    console_route_events  BIGINT,
    legacy_route_events   BIGINT,
    rum_error_events      BIGINT,
    rum_perf_p95_ms       BIGINT,
    last_event_at         TIMESTAMPTZ
)
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = public, pg_temp
AS $$
BEGIN
    SET LOCAL row_security = off;

    RETURN QUERY
        SELECT
            t.org_id,
            t.release_cycle,
            COUNT(*) FILTER (
                WHERE t.event_kind = 'route_selection' AND t.route_surface = 'console'
            )::BIGINT AS console_route_events,
            COUNT(*) FILTER (
                WHERE t.event_kind = 'route_selection' AND t.route_surface = 'legacy'
            )::BIGINT AS legacy_route_events,
            COUNT(*) FILTER (WHERE t.event_kind = 'rum_error')::BIGINT AS rum_error_events,
            (percentile_disc(0.95) WITHIN GROUP (ORDER BY t.duration_ms) FILTER (
                WHERE t.duration_ms IS NOT NULL
                  AND t.event_kind IN ('route_selection', 'rum_perf')
            ))::BIGINT AS rum_perf_p95_ms,
            MAX(t.occurred_at) AS last_event_at
        FROM console_route_telemetry t
        GROUP BY t.org_id, t.release_cycle
        ORDER BY MAX(t.occurred_at) DESC, t.release_cycle DESC;

    SET LOCAL row_security = on;
EXCEPTION WHEN OTHERS THEN
    SET LOCAL row_security = on;
    RAISE;
END;
$$;
REVOKE ALL ON FUNCTION platform_console_route_adoption() FROM PUBLIC;
GRANT EXECUTE ON FUNCTION platform_console_route_adoption() TO console_rt;
