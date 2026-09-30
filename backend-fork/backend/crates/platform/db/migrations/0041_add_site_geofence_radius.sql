-- Issue #13 — per-site geofence radius for arrival/departure detection.
--
-- registry_sites already carries lat/long (0039); the geofence radius is how far
-- from that coordinate a mechanic's on-duty GPS counts as "arrived" at the site.
-- NULL means "use the system default" (console_kernel_core::DEFAULT_GEOFENCE_RADIUS_M
-- = 300 m); an admin can override per site via PATCH /api/v1/sites/{id}. The
-- column is NULLable with no backfill and inherits registry_sites' existing
-- org_isolation RLS, immutable-org trigger, and (id, org_id) key — so this
-- migration adds NO new policy/trigger/org column, only the column + its CHECK.
--
-- console-gate: audited-table registry_sites
ALTER TABLE registry_sites
    ADD COLUMN geofence_radius_m DOUBLE PRECISION;

-- A radius is a positive distance; bound it so a typo cannot store an absurd
-- value (100 km is already far beyond any real yard). NULL passes (use default).
ALTER TABLE registry_sites
    ADD CONSTRAINT registry_sites_geofence_radius_positive
        CHECK (geofence_radius_m IS NULL
               OR (geofence_radius_m > 0 AND geofence_radius_m <= 100000));
