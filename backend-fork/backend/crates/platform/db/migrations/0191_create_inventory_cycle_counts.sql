-- Inventory movement documents + cycle-count reconciliation (CAP-INVENTORY-CONSOLE).
--
-- Stock invariant: on-hand is a cached projection; every change is a movement
-- document carrying before/delta/after with a DB CHECK proving the ledger sum
-- explains the cache (현재고 산출 = 이동 문서 합계). ISSUE rows stay in
-- inventory_consumption_events (0156); RECEIPT/ADJUSTMENT rows live here.
-- Cycle counts reconcile counted-vs-system variances through a maker≠checker
-- approval into ADJUSTMENT movements with full audit lineage.

-- console-gate: audited-table inventory_cycle_counts
CREATE TABLE inventory_cycle_counts (
    id                           UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id                       UUID        NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    branch_id                    UUID        NOT NULL,
    stock_location_id            UUID        NOT NULL,
    cc_code                      TEXT        NOT NULL CHECK (cc_code ~ '^IC-[0-9]{4,10}$'),
    status                       TEXT        NOT NULL DEFAULT 'DRAFT'
                                             CHECK (status IN ('DRAFT','SUBMITTED','APPROVED','REJECTED','CANCELLED')),
    version                      INTEGER     NOT NULL DEFAULT 1 CHECK (version >= 1),
    opened_by                    UUID        NOT NULL,
    submitted_by                 UUID        NULL,
    submitted_at                 TIMESTAMPTZ NULL,
    decided_by                   UUID        NULL,
    decided_at                   TIMESTAMPTZ NULL,
    decision_memo                TEXT        NULL CHECK (decision_memo IS NULL OR char_length(decision_memo) <= 1000),
    -- Approve idempotency: replays of the same approval return the stored
    -- outcome; a reused key with a different payload is a conflict.
    decision_idempotency_key     TEXT        NULL CHECK (decision_idempotency_key IS NULL
                                                 OR char_length(btrim(decision_idempotency_key)) BETWEEN 16 AND 200),
    decision_request_fingerprint TEXT        NULL CHECK (decision_request_fingerprint IS NULL
                                                 OR decision_request_fingerprint ~ '^[a-f0-9]{64}$'),
    created_at                   TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at                   TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (id, org_id),
    UNIQUE (org_id, cc_code),
    FOREIGN KEY (branch_id, org_id) REFERENCES branches(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (stock_location_id, org_id) REFERENCES inventory_stock_locations(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (opened_by, org_id) REFERENCES users(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (submitted_by, org_id) REFERENCES users(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (decided_by, org_id) REFERENCES users(id, org_id) ON DELETE RESTRICT,
    CHECK ((status IN ('SUBMITTED','APPROVED','REJECTED')) = (submitted_by IS NOT NULL)),
    CHECK ((submitted_by IS NULL) = (submitted_at IS NULL)),
    CHECK ((status IN ('APPROVED','REJECTED')) = (decided_by IS NOT NULL)),
    CHECK ((decided_by IS NULL) = (decided_at IS NULL)),
    -- Separation of duties: the submitter can never decide their own count.
    CHECK (decided_by IS NULL OR decided_by <> submitted_by),
    CHECK (status <> 'REJECTED' OR decision_memo IS NOT NULL),
    CHECK (decision_idempotency_key IS NULL OR status = 'APPROVED'),
    CHECK ((decision_idempotency_key IS NULL) = (decision_request_fingerprint IS NULL))
);

CREATE INDEX idx_inventory_cycle_counts_org_branch_status
    ON inventory_cycle_counts (org_id, branch_id, status, updated_at DESC);
CREATE UNIQUE INDEX inventory_cycle_counts_org_decision_key
    ON inventory_cycle_counts (org_id, decision_idempotency_key)
    WHERE decision_idempotency_key IS NOT NULL;

-- Per-org IC- code issuance, serialized by the row lock the upsert takes.
CREATE TABLE inventory_cycle_count_counters (
    org_id     UUID   PRIMARY KEY REFERENCES organizations(id) ON DELETE RESTRICT,
    last_value BIGINT NOT NULL CHECK (last_value >= 1)
);

-- console-gate: audited-table inventory_cycle_count_lines
CREATE TABLE inventory_cycle_count_lines (
    id                     UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id                 UUID        NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    count_id               UUID        NOT NULL,
    item_id                UUID        NOT NULL,
    system_quantity_milli  BIGINT      NOT NULL CHECK (system_quantity_milli >= 0),
    counted_quantity_milli BIGINT      NOT NULL CHECK (counted_quantity_milli >= 0),
    variance_milli         BIGINT      GENERATED ALWAYS AS (counted_quantity_milli - system_quantity_milli) STORED,
    reason                 TEXT        NULL CHECK (reason IS NULL OR reason IN ('DAMAGE','LOSS','MISCOUNT','FOUND','OTHER')),
    note                   TEXT        NULL CHECK (note IS NULL OR char_length(note) <= 500),
    recorded_by            UUID        NOT NULL,
    recorded_at            TIMESTAMPTZ NOT NULL,
    created_at             TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at             TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (count_id, item_id),
    FOREIGN KEY (count_id, org_id) REFERENCES inventory_cycle_counts(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (item_id, org_id) REFERENCES inventory_items(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (recorded_by, org_id) REFERENCES users(id, org_id) ON DELETE RESTRICT,
    -- Typed-variance discipline: a nonzero variance requires a reason.
    CHECK (counted_quantity_milli = system_quantity_milli OR reason IS NOT NULL)
);

CREATE INDEX idx_inventory_cycle_count_lines_org_count
    ON inventory_cycle_count_lines (org_id, count_id);

-- console-gate: audited-table inventory_movements
CREATE TABLE inventory_movements (
    id                    UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id                UUID        NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    branch_id             UUID        NOT NULL,
    item_id               UUID        NOT NULL,
    stock_location_id     UUID        NOT NULL,
    kind                  TEXT        NOT NULL CHECK (kind IN ('RECEIPT','ADJUSTMENT')),
    quantity_delta_milli  BIGINT      NOT NULL CHECK (quantity_delta_milli <> 0),
    quantity_before_milli BIGINT      NOT NULL CHECK (quantity_before_milli >= 0),
    quantity_after_milli  BIGINT      NOT NULL CHECK (quantity_after_milli >= 0),
    cycle_count_id        UUID        NULL,
    source_ref            TEXT        NULL CHECK (source_ref IS NULL OR source_ref ~ '^[A-Z]{1,4}-[A-Z0-9-]{1,40}$'),
    memo                  TEXT        NULL CHECK (memo IS NULL OR char_length(memo) <= 1000),
    actor_id              UUID        NOT NULL,
    occurred_at           TIMESTAMPTZ NOT NULL,
    idempotency_key       TEXT        NULL CHECK (idempotency_key IS NULL
                                          OR char_length(btrim(idempotency_key)) BETWEEN 16 AND 200),
    request_fingerprint   TEXT        NULL CHECK (request_fingerprint IS NULL
                                          OR request_fingerprint ~ '^[a-f0-9]{64}$'),
    created_at            TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (id, org_id),
    UNIQUE (org_id, idempotency_key),
    FOREIGN KEY (branch_id, org_id) REFERENCES branches(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (item_id, org_id) REFERENCES inventory_items(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (stock_location_id, org_id) REFERENCES inventory_stock_locations(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (cycle_count_id, org_id) REFERENCES inventory_cycle_counts(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (actor_id, org_id) REFERENCES users(id, org_id) ON DELETE RESTRICT,
    -- The ledger explains the cache.
    CHECK (quantity_before_milli + quantity_delta_milli = quantity_after_milli),
    CHECK (kind <> 'RECEIPT' OR quantity_delta_milli > 0),
    -- v1: adjustments only ever originate from an approved cycle count.
    CHECK ((kind = 'ADJUSTMENT') = (cycle_count_id IS NOT NULL)),
    -- Receipts are client-idempotent; adjustment idempotency is governed by
    -- the owning count's decision_idempotency_key.
    CHECK ((kind = 'RECEIPT') = (idempotency_key IS NOT NULL)),
    CHECK ((idempotency_key IS NULL) = (request_fingerprint IS NULL))
);

CREATE INDEX idx_inventory_movements_org_item_occurred
    ON inventory_movements (org_id, item_id, occurred_at DESC, created_at DESC);
CREATE INDEX idx_inventory_movements_org_count
    ON inventory_movements (org_id, cycle_count_id)
    WHERE cycle_count_id IS NOT NULL;

ALTER TABLE inventory_cycle_counts ENABLE ROW LEVEL SECURITY;
ALTER TABLE inventory_cycle_counts FORCE ROW LEVEL SECURITY;
CREATE POLICY org_isolation ON inventory_cycle_counts
    USING (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid)
    WITH CHECK (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid);
GRANT SELECT, INSERT, UPDATE ON inventory_cycle_counts TO console_rt;
CREATE TRIGGER trg_inventory_cycle_counts_org_immutable
    BEFORE UPDATE ON inventory_cycle_counts
    FOR EACH ROW EXECUTE FUNCTION enforce_org_id_immutable();

ALTER TABLE inventory_cycle_count_counters ENABLE ROW LEVEL SECURITY;
ALTER TABLE inventory_cycle_count_counters FORCE ROW LEVEL SECURITY;
CREATE POLICY org_isolation ON inventory_cycle_count_counters
    USING (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid)
    WITH CHECK (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid);
GRANT SELECT, INSERT, UPDATE ON inventory_cycle_count_counters TO console_rt;

ALTER TABLE inventory_cycle_count_lines ENABLE ROW LEVEL SECURITY;
ALTER TABLE inventory_cycle_count_lines FORCE ROW LEVEL SECURITY;
CREATE POLICY org_isolation ON inventory_cycle_count_lines
    USING (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid)
    WITH CHECK (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid);
GRANT SELECT, INSERT, UPDATE ON inventory_cycle_count_lines TO console_rt;
CREATE TRIGGER trg_inventory_cycle_count_lines_org_immutable
    BEFORE UPDATE ON inventory_cycle_count_lines
    FOR EACH ROW EXECUTE FUNCTION enforce_org_id_immutable();

ALTER TABLE inventory_movements ENABLE ROW LEVEL SECURITY;
ALTER TABLE inventory_movements FORCE ROW LEVEL SECURITY;
CREATE POLICY org_isolation ON inventory_movements
    USING (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid)
    WITH CHECK (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid);
-- Append-only: no UPDATE grant.
GRANT SELECT, INSERT ON inventory_movements TO console_rt;
CREATE TRIGGER trg_inventory_movements_org_immutable
    BEFORE UPDATE ON inventory_movements
    FOR EACH ROW EXECUTE FUNCTION enforce_org_id_immutable();
