-- Issue #13 — site arrival/departure (geofence) tracking.
--
-- When a mechanic on duty (location consent GRANTED) pings GPS within a site's
-- geofence radius (registry_sites.geofence_radius_m / the 300 m default), the
-- ping-ingest path records an ARRIVAL; leaving the radius records a DEPARTURE.
--
-- TWO tables with deliberately different retention, to honour the location-data
-- carve-out (Korean 위치정보법 / GDPR — consent withdrawal physically erases raw
-- location data, see compliance transition_consent):
--
--  * site_attendance_events — the DURABLE business fact: "user arrived at / left
--    the site for work order X at time T". It carries NO coordinates, so it is a
--    work record (like a timesheet), NOT location data, and therefore SURVIVES
--    consent withdrawal. Append-only + audited (site.arrival / site.departure).
--
--  * site_geofence_presence — the TRANSIENT inside/outside state per
--    (user × work order × site), needed only to detect the next crossing edge.
--    It IS location-derived, so it is hard-deleted on consent withdrawal
--    alongside location_pings (the compliance adapter's withdrawal DELETE block).
--
-- Both are full tenant tables created post-multi-tenant, so they bake in org_id +
-- FORCE RLS org_isolation + the immutable-org trigger + a composite (id, org_id)
-- key inline (the rollout 0027-0035 retrofitted these onto legacy tables).

-- console-gate: audited-table site_attendance_events
CREATE TABLE site_attendance_events (
    id            UUID        NOT NULL DEFAULT gen_random_uuid(),
    org_id        UUID        NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    user_id       UUID        NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    branch_id     UUID        NOT NULL REFERENCES branches(id) ON DELETE RESTRICT,
    work_order_id UUID        NOT NULL REFERENCES work_orders(id) ON DELETE RESTRICT,
    site_id       UUID        NOT NULL REFERENCES registry_sites(id) ON DELETE RESTRICT,
    kind          TEXT        NOT NULL CHECK (kind IN ('ARRIVAL', 'DEPARTURE')),
    occurred_at   TIMESTAMPTZ NOT NULL,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (id, org_id)
);

ALTER TABLE site_attendance_events ENABLE ROW LEVEL SECURITY;
ALTER TABLE site_attendance_events FORCE ROW LEVEL SECURITY;
CREATE POLICY org_isolation ON site_attendance_events
    USING (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid)
    WITH CHECK (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid);
CREATE TRIGGER trg_site_attendance_events_org_immutable
    BEFORE UPDATE ON site_attendance_events
    FOR EACH ROW EXECUTE FUNCTION enforce_org_id_immutable();
CREATE INDEX idx_site_attendance_events_org_branch_time
    ON site_attendance_events (org_id, branch_id, occurred_at DESC);
CREATE INDEX idx_site_attendance_events_org_user_time
    ON site_attendance_events (org_id, user_id, occurred_at DESC);

-- NOT an audited-table: site_geofence_presence is TRANSIENT, location-derived
-- state, not a durable audit record. It is hard-deleted on consent withdrawal
-- and time-purged with the raw pings (the location-data carve-out), so it must
-- stay droppable — marking it `audited-table` would wrongly pin it as
-- un-droppable durable data. Only site_attendance_events (above) is audited.
CREATE TABLE site_geofence_presence (
    id            UUID        NOT NULL DEFAULT gen_random_uuid(),
    org_id        UUID        NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    user_id       UUID        NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    work_order_id UUID        NOT NULL REFERENCES work_orders(id) ON DELETE RESTRICT,
    site_id       UUID        NOT NULL REFERENCES registry_sites(id) ON DELETE RESTRICT,
    inside        BOOLEAN     NOT NULL,
    since         TIMESTAMPTZ NOT NULL,
    updated_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (id, org_id),
    -- One presence row per (user, work order, site) within a tenant; the geofence
    -- eval does a SELECT FOR UPDATE / upsert keyed on this to be race-free.
    UNIQUE (org_id, user_id, work_order_id, site_id)
);

ALTER TABLE site_geofence_presence ENABLE ROW LEVEL SECURITY;
ALTER TABLE site_geofence_presence FORCE ROW LEVEL SECURITY;
CREATE POLICY org_isolation ON site_geofence_presence
    USING (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid)
    WITH CHECK (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid);
CREATE TRIGGER trg_site_geofence_presence_org_immutable
    BEFORE UPDATE ON site_geofence_presence
    FOR EACH ROW EXECUTE FUNCTION enforce_org_id_immutable();
