-- renamed by L-WIRE 2026-07-10 to resolve version collision (was 0101_create_inventory).
-- Inventory module backend (IV-): tenant-scoped stock locations, items, and
-- append-only consumption events linked to work orders or P1 dispatches.

INSERT INTO feature_catalog (feature_key) VALUES
    ('inventory_read'),
    ('inventory_manage'),
    ('inventory_consume'),
    ('inventory_reorder')
ON CONFLICT (feature_key) DO NOTHING;

-- console-gate: audited-table inventory_stock_locations
CREATE TABLE inventory_stock_locations (
    id            UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id        UUID        NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    branch_id     UUID        NOT NULL,
    site_id       UUID        NULL,
    location_code TEXT        NULL CHECK (location_code IS NULL OR char_length(btrim(location_code)) BETWEEN 1 AND 80),
    label         TEXT        NOT NULL CHECK (char_length(btrim(label)) BETWEEN 1 AND 120),
    status        TEXT        NOT NULL DEFAULT 'ACTIVE' CHECK (status IN ('ACTIVE','ARCHIVED')),
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (id, org_id),
    FOREIGN KEY (branch_id, org_id) REFERENCES branches(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (site_id, org_id) REFERENCES registry_sites(id, org_id) ON DELETE RESTRICT
);

CREATE UNIQUE INDEX inventory_stock_locations_branch_null_site_label_key
    ON inventory_stock_locations (org_id, branch_id, label)
    WHERE site_id IS NULL;
CREATE UNIQUE INDEX inventory_stock_locations_branch_site_label_key
    ON inventory_stock_locations (org_id, branch_id, site_id, label)
    WHERE site_id IS NOT NULL;
CREATE INDEX idx_inventory_stock_locations_org_branch_status
    ON inventory_stock_locations (org_id, branch_id, status, label);
CREATE INDEX idx_inventory_stock_locations_org_site_status
    ON inventory_stock_locations (org_id, site_id, status)
    WHERE site_id IS NOT NULL;

-- console-gate: audited-table inventory_items
CREATE TABLE inventory_items (
    id                      UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id                  UUID        NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    branch_id               UUID        NOT NULL,
    stock_location_id       UUID        NOT NULL,
    site_id                 UUID        NULL,
    iv_code                 TEXT        NOT NULL CHECK (iv_code ~ '^IV-[A-Z0-9-]{3,40}$'),
    sku                     TEXT        NULL CHECK (sku IS NULL OR char_length(btrim(sku)) BETWEEN 1 AND 80),
    display_name            TEXT        NOT NULL CHECK (char_length(btrim(display_name)) BETWEEN 1 AND 120),
    description             TEXT        NULL CHECK (description IS NULL OR char_length(description) <= 1000),
    unit_code               TEXT        NOT NULL CHECK (unit_code ~ '^[A-Z][A-Z0-9_]{0,15}$'),
    quantity_on_hand_milli  BIGINT      NOT NULL DEFAULT 0 CHECK (quantity_on_hand_milli >= 0),
    safety_stock_milli      BIGINT      NOT NULL DEFAULT 0 CHECK (safety_stock_milli >= 0),
    unit_cost_won           BIGINT      NULL CHECK (unit_cost_won IS NULL OR unit_cost_won >= 0),
    status                  TEXT        NOT NULL DEFAULT 'ACTIVE' CHECK (status IN ('ACTIVE','ARCHIVED')),
    created_by              UUID        NOT NULL,
    created_at              TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at              TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (id, org_id),
    UNIQUE (org_id, iv_code),
    FOREIGN KEY (branch_id, org_id) REFERENCES branches(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (stock_location_id, org_id) REFERENCES inventory_stock_locations(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (site_id, org_id) REFERENCES registry_sites(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (created_by, org_id) REFERENCES users(id, org_id) ON DELETE RESTRICT
);

CREATE UNIQUE INDEX inventory_items_org_branch_sku_key
    ON inventory_items (org_id, branch_id, sku)
    WHERE sku IS NOT NULL;
CREATE INDEX idx_inventory_items_org_branch_status_updated
    ON inventory_items (org_id, branch_id, status, updated_at DESC);
CREATE INDEX idx_inventory_items_org_location_status
    ON inventory_items (org_id, stock_location_id, status);
CREATE INDEX idx_inventory_items_org_site_status
    ON inventory_items (org_id, site_id, status)
    WHERE site_id IS NOT NULL;
CREATE INDEX idx_inventory_items_org_low_stock
    ON inventory_items (org_id, branch_id, quantity_on_hand_milli, safety_stock_milli);

-- console-gate: audited-table inventory_consumption_events
CREATE TABLE inventory_consumption_events (
    id                         UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id                     UUID        NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    branch_id                  UUID        NOT NULL,
    item_id                    UUID        NOT NULL,
    stock_location_id          UUID        NOT NULL,
    source_kind                TEXT        NOT NULL CHECK (source_kind IN ('WORK_ORDER','P1_DISPATCH')),
    work_order_id              UUID        NOT NULL,
    dispatch_id                UUID        NULL,
    quantity_before_milli      BIGINT      NOT NULL CHECK (quantity_before_milli >= 0),
    quantity_consumed_milli    BIGINT      NOT NULL CHECK (quantity_consumed_milli > 0),
    quantity_after_milli       BIGINT      NOT NULL CHECK (quantity_after_milli >= 0),
    unit_cost_won              BIGINT      NULL CHECK (unit_cost_won IS NULL OR unit_cost_won >= 0),
    cost_won                   BIGINT      NULL CHECK (cost_won IS NULL OR cost_won >= 0),
    consumed_by                UUID        NOT NULL,
    occurred_at                TIMESTAMPTZ NOT NULL,
    memo                       TEXT        NULL CHECK (memo IS NULL OR char_length(memo) <= 1000),
    idempotency_key            TEXT        NOT NULL CHECK (char_length(btrim(idempotency_key)) BETWEEN 16 AND 200),
    request_fingerprint        TEXT        NOT NULL CHECK (request_fingerprint ~ '^[a-f0-9]{64}$'),
    created_at                 TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (id, org_id),
    UNIQUE (org_id, idempotency_key),
    CHECK (quantity_before_milli - quantity_consumed_milli = quantity_after_milli),
    CHECK ((source_kind = 'WORK_ORDER' AND dispatch_id IS NULL) OR (source_kind = 'P1_DISPATCH' AND dispatch_id IS NOT NULL)),
    FOREIGN KEY (branch_id, org_id) REFERENCES branches(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (item_id, org_id) REFERENCES inventory_items(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (stock_location_id, org_id) REFERENCES inventory_stock_locations(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (work_order_id, org_id) REFERENCES work_orders(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (dispatch_id, org_id) REFERENCES p1_dispatches(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (consumed_by, org_id) REFERENCES users(id, org_id) ON DELETE RESTRICT
);

CREATE INDEX idx_inventory_consumption_events_org_item_occurred
    ON inventory_consumption_events (org_id, item_id, occurred_at DESC);
CREATE INDEX idx_inventory_consumption_events_org_work_order_occurred
    ON inventory_consumption_events (org_id, work_order_id, occurred_at DESC);
CREATE INDEX idx_inventory_consumption_events_org_dispatch_occurred
    ON inventory_consumption_events (org_id, dispatch_id, occurred_at DESC)
    WHERE dispatch_id IS NOT NULL;
CREATE INDEX idx_inventory_consumption_events_org_branch_occurred
    ON inventory_consumption_events (org_id, branch_id, occurred_at DESC);

ALTER TABLE inventory_stock_locations ENABLE ROW LEVEL SECURITY;
ALTER TABLE inventory_stock_locations FORCE ROW LEVEL SECURITY;
CREATE POLICY org_isolation ON inventory_stock_locations
    USING (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid)
    WITH CHECK (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid);
GRANT SELECT, INSERT, UPDATE ON inventory_stock_locations TO console_rt;
CREATE TRIGGER trg_inventory_stock_locations_org_immutable
    BEFORE UPDATE ON inventory_stock_locations
    FOR EACH ROW EXECUTE FUNCTION enforce_org_id_immutable();

ALTER TABLE inventory_items ENABLE ROW LEVEL SECURITY;
ALTER TABLE inventory_items FORCE ROW LEVEL SECURITY;
CREATE POLICY org_isolation ON inventory_items
    USING (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid)
    WITH CHECK (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid);
GRANT SELECT, INSERT, UPDATE ON inventory_items TO console_rt;
CREATE TRIGGER trg_inventory_items_org_immutable
    BEFORE UPDATE ON inventory_items
    FOR EACH ROW EXECUTE FUNCTION enforce_org_id_immutable();

ALTER TABLE inventory_consumption_events ENABLE ROW LEVEL SECURITY;
ALTER TABLE inventory_consumption_events FORCE ROW LEVEL SECURITY;
CREATE POLICY org_isolation ON inventory_consumption_events
    USING (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid)
    WITH CHECK (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid);
GRANT SELECT, INSERT ON inventory_consumption_events TO console_rt;
CREATE TRIGGER trg_inventory_consumption_events_org_immutable
    BEFORE UPDATE ON inventory_consumption_events
    FOR EACH ROW EXECUTE FUNCTION enforce_org_id_immutable();
