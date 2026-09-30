-- Sales catalog (#6 지게차 매매/부가서비스): the public storefront's used-forklift
-- listings, their photos, and inbound customer inquiries/leads.
--
-- A "listing" is a distinct concept from a fleet asset (registry_equipment): it
-- carries a public asking price, photos, buyer-facing copy, a sale lifecycle, and
-- a public-visibility gate. It MAY optionally reference a real fleet unit via a
-- nullable (equipment_id, org_id) FK when the listed item is an owned asset.
--
-- All three tables are tenant-scoped (org_isolation RLS, FORCE) and audited.

-- ─────────────────────────────────────────────────────────────────────────────
-- sales_listings — a used forklift offered for sale and/or rental
-- ─────────────────────────────────────────────────────────────────────────────
-- console-gate: audited-table sales_listings
CREATE TABLE sales_listings (
    id              UUID        NOT NULL DEFAULT gen_random_uuid(),
    org_id          UUID        NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    -- Optional link to a real fleet unit (composite tenant FK); NULL for a
    -- catalog-only listing not tied to the internal asset register.
    equipment_id    UUID,
    kind            TEXT        NOT NULL CHECK (kind IN ('ELECTRIC', 'DIESEL', 'LPG', 'REACH')),
    model_name      TEXT        NOT NULL CHECK (char_length(model_name) BETWEEN 1 AND 200),
    -- Load capacity in milli-tons (reuses the registry Ton value object: 2.5 t = 2500).
    capacity_milli  BIGINT      CHECK (capacity_milli IS NULL OR capacity_milli > 0),
    model_year      INTEGER     CHECK (model_year IS NULL OR (model_year BETWEEN 1980 AND 2100)),
    usage_hours     INTEGER     CHECK (usage_hours IS NULL OR usage_hours >= 0),
    -- Public asking price in KRW (reuses MoneyWon). NULL = "price on inquiry".
    price_won       BIGINT      CHECK (price_won IS NULL OR price_won >= 0),
    badge           TEXT        CHECK (badge IS NULL OR char_length(badge) <= 60),
    usage_label     TEXT        CHECK (usage_label IS NULL OR char_length(usage_label) <= 80),
    condition_label TEXT        CHECK (condition_label IS NULL OR char_length(condition_label) <= 80),
    availability    TEXT        CHECK (availability IS NULL OR char_length(availability) <= 80),
    location        TEXT        CHECK (location IS NULL OR char_length(location) <= 120),
    description     TEXT        CHECK (description IS NULL OR char_length(description) <= 4000),
    listing_type    TEXT        NOT NULL DEFAULT 'SALE'
                                CHECK (listing_type IN ('SALE', 'RENTAL', 'BOTH')),
    status          TEXT        NOT NULL DEFAULT 'DRAFT'
                                CHECK (status IN ('DRAFT', 'PUBLISHED', 'RESERVED', 'SOLD', 'WITHDRAWN')),
    -- Higher sorts first in the public catalog.
    sort_weight     INTEGER     NOT NULL DEFAULT 0,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (id, org_id),
    FOREIGN KEY (equipment_id, org_id)
        REFERENCES registry_equipment (id, org_id) ON DELETE SET NULL
);
ALTER TABLE sales_listings ENABLE ROW LEVEL SECURITY;
ALTER TABLE sales_listings FORCE ROW LEVEL SECURITY;
CREATE POLICY org_isolation ON sales_listings
    USING (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid)
    WITH CHECK (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid);
CREATE TRIGGER trg_sales_listings_org_immutable
    BEFORE UPDATE ON sales_listings
    FOR EACH ROW EXECUTE FUNCTION enforce_org_id_immutable();
-- The public catalog reads only PUBLISHED listings, newest/weightiest first.
CREATE INDEX idx_sales_listings_org_public
    ON sales_listings (org_id, status, sort_weight DESC, created_at DESC);
CREATE INDEX idx_sales_listings_org_kind
    ON sales_listings (org_id, kind);

-- ─────────────────────────────────────────────────────────────────────────────
-- sales_listing_media — photos for a listing, stored in the object store
-- ─────────────────────────────────────────────────────────────────────────────
-- Bytes live in the object store (RustFS/S3) keyed by s3_key; this row is the
-- metadata + ordering, mirroring evidence_media (migration 0009).
-- console-gate: audited-table sales_listing_media
CREATE TABLE sales_listing_media (
    id           UUID        NOT NULL DEFAULT gen_random_uuid(),
    org_id       UUID        NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    listing_id   UUID        NOT NULL,
    s3_key       TEXT        NOT NULL CHECK (s3_key <> ''),
    content_type TEXT        NOT NULL CHECK (content_type <> ''),
    alt_text     TEXT        CHECK (alt_text IS NULL OR char_length(alt_text) <= 200),
    sort_order   INTEGER     NOT NULL DEFAULT 0,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (id, org_id),
    FOREIGN KEY (listing_id, org_id)
        REFERENCES sales_listings (id, org_id) ON DELETE CASCADE,
    UNIQUE (org_id, s3_key)
);
ALTER TABLE sales_listing_media ENABLE ROW LEVEL SECURITY;
ALTER TABLE sales_listing_media FORCE ROW LEVEL SECURITY;
CREATE POLICY org_isolation ON sales_listing_media
    USING (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid)
    WITH CHECK (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid);
CREATE TRIGGER trg_sales_listing_media_org_immutable
    BEFORE UPDATE ON sales_listing_media
    FOR EACH ROW EXECUTE FUNCTION enforce_org_id_immutable();
CREATE INDEX idx_sales_listing_media_listing
    ON sales_listing_media (org_id, listing_id, sort_order);

-- ─────────────────────────────────────────────────────────────────────────────
-- customer_inquiries — an inbound lead from the public storefront
-- ─────────────────────────────────────────────────────────────────────────────
-- name + phone are PII (위치정보법/개인정보보호법): stored here, NEVER logged
-- (the pii-no-logs gate is literal-only; handlers must not log these values).
-- console-gate: audited-table customer_inquiries
CREATE TABLE customer_inquiries (
    id           UUID        NOT NULL DEFAULT gen_random_uuid(),
    org_id       UUID        NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    name         TEXT        NOT NULL CHECK (char_length(name) BETWEEN 1 AND 100),
    phone        TEXT        NOT NULL CHECK (char_length(phone) BETWEEN 1 AND 40),
    topic        TEXT        NOT NULL CHECK (topic IN ('RENTAL', 'USED_SALES', 'MAINTENANCE', 'OTHER')),
    location     TEXT        CHECK (location IS NULL OR char_length(location) <= 120),
    message      TEXT        CHECK (message IS NULL OR char_length(message) <= 2000),
    -- Optional link to the listing the inquiry was made from.
    listing_id   UUID,
    status       TEXT        NOT NULL DEFAULT 'NEW'
                             CHECK (status IN ('NEW', 'CONTACTED', 'CLOSED')),
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (id, org_id),
    FOREIGN KEY (listing_id, org_id)
        REFERENCES sales_listings (id, org_id) ON DELETE SET NULL
);
ALTER TABLE customer_inquiries ENABLE ROW LEVEL SECURITY;
ALTER TABLE customer_inquiries FORCE ROW LEVEL SECURITY;
CREATE POLICY org_isolation ON customer_inquiries
    USING (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid)
    WITH CHECK (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid);
CREATE TRIGGER trg_customer_inquiries_org_immutable
    BEFORE UPDATE ON customer_inquiries
    FOR EACH ROW EXECUTE FUNCTION enforce_org_id_immutable();
CREATE INDEX idx_customer_inquiries_org_inbox
    ON customer_inquiries (org_id, status, created_at DESC);
