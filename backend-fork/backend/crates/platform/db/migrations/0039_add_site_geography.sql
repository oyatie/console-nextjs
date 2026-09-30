-- Issue #12 — geographic dispatch map: give registry_sites a real location.
--
-- registry_sites was name-only. The dispatch map needs a per-site coordinate so
-- it can drop a real pin, plus a Korean administrative address (시도 / 시군구) for
-- the ungeocoded-site list and operator review. EVERY column added here is
-- NULLable and there is NO backfill: a site shows on the map only once an admin
-- has entered its coordinates through PATCH /api/v1/sites/{id}. Until then it is
-- listed as "ungeocoded" rather than rendered at a fabricated location. There is
-- no geocoding service, so a coordinate exists only because a human typed it.
--
-- registry_sites already carries org_id, a FORCE-ROW-LEVEL-SECURITY
-- `org_isolation` policy (0030), the shared immutable-org trigger (0031), and the
-- (id, org_id) unique key (0034). New columns inherit that row policy, so this
-- migration adds NO new policy, trigger, or org column — only the geo columns,
-- their value/consistency CHECKs, and one partial lookup index.
--
-- console-gate: audited-table registry_sites
ALTER TABLE registry_sites
    ADD COLUMN address     TEXT,
    ADD COLUMN province    TEXT,   -- 시도 (administrative level 1, e.g. 경기도)
    ADD COLUMN city        TEXT,   -- 시군구 (administrative level 2, e.g. 안산시)
    ADD COLUMN postal_code TEXT,
    ADD COLUMN latitude    DOUBLE PRECISION,
    ADD COLUMN longitude   DOUBLE PRECISION;

-- Coordinate value bounds. WGS84 latitude is [-90, 90] and longitude
-- [-180, 180]; anything else is a data-entry error, not a place on Earth. The
-- checks pass on NULL (an ungeocoded site), so they only constrain real values.
ALTER TABLE registry_sites
    ADD CONSTRAINT registry_sites_latitude_range
        CHECK (latitude IS NULL OR (latitude >= -90 AND latitude <= 90)),
    ADD CONSTRAINT registry_sites_longitude_range
        CHECK (longitude IS NULL OR (longitude >= -180 AND longitude <= 180)),
    -- A pin needs BOTH coordinates or NEITHER. Storing one without the other
    -- would yield a half-located site the map could neither pin nor cleanly list
    -- as ungeocoded, so the pair is enforced to rise and fall together.
    ADD CONSTRAINT registry_sites_lat_lon_paired
        CHECK ((latitude IS NULL) = (longitude IS NULL));

-- Bounded-text length CHECKs (defense-in-depth, mirroring
-- 0025_bounded_text_constraints.sql). The PATCH /sites handler bounds these in
-- the domain layer (crates/kernel/core/src/validation.rs); these constraints are
-- the backstop so the bound still holds if a future code path ever writes to
-- registry_sites without going through that validation. char_length() counts
-- Unicode code points, matching the Rust `str::chars().count()` bound exactly.
-- All checks pass on NULL, since every geo column is optional.
ALTER TABLE registry_sites
    ADD CONSTRAINT registry_sites_address_max_chars
        CHECK (address IS NULL OR char_length(address) <= 500),
    ADD CONSTRAINT registry_sites_province_max_chars
        CHECK (province IS NULL OR char_length(province) <= 100),
    ADD CONSTRAINT registry_sites_city_max_chars
        CHECK (city IS NULL OR char_length(city) <= 100),
    ADD CONSTRAINT registry_sites_postal_code_max_chars
        CHECK (postal_code IS NULL OR char_length(postal_code) <= 20);

-- The map's by-location read groups geocoded sites by 시도/시군구 within a tenant.
-- The partial index covers exactly that access path (org_id then province/city)
-- and excludes the ungeocoded rows the map never pins, keeping it small.
CREATE INDEX idx_registry_sites_geo
    ON registry_sites (org_id, province, city)
    WHERE latitude IS NOT NULL;
