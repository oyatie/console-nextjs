-- CAP-MAINTENANCE-CONSOLE gap closure (design-contract §2).
-- PROVISIONAL number 0193: integrator renumbers to the next free migration
-- number immediately before push (migration-collision memory).

-- G2: typed maintenance classification. Nullable — legacy rows stay unset and
-- render as absent chips; the console intake requires both at the client gate.
ALTER TABLE work_orders
    ADD COLUMN maintenance_type TEXT CHECK (maintenance_type IS NULL OR maintenance_type IN
        ('EMERGENCY','CORRECTIVE','PREVENTIVE','INSPECTION')),
    ADD COLUMN maintenance_cause TEXT CHECK (maintenance_cause IS NULL OR maintenance_cause IN
        ('BREAKDOWN','RETURN_PREP','SCHEDULED','INSPECTION_FINDING','OTHER'));

CREATE INDEX idx_work_orders_maintenance_type
    ON work_orders (branch_id, maintenance_type, status)
    WHERE maintenance_type IS NOT NULL;

-- G3: cost settlement closing the order into cost (정산 → 전표).
-- Order-owned; finance handoff is a truthful text ref (voucher_ref) until the
-- finance-gl lane wires real vouchers. No hard delete: VOID is the terminal
-- correction state and frees the one-live-settlement slot.
CREATE TABLE work_order_settlements (
    id               UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id           UUID        NOT NULL REFERENCES organizations(id),
    work_order_id    UUID        NOT NULL,
    branch_id        UUID        NOT NULL,
    status           TEXT        NOT NULL DEFAULT 'DRAFT'
        CHECK (status IN ('DRAFT','SUBMITTED','APPROVED','VOID')),
    total_amount_krw BIGINT      NOT NULL DEFAULT 0 CHECK (total_amount_krw >= 0),
    voucher_ref      TEXT,
    note             TEXT,
    idempotency_key  TEXT        NOT NULL CHECK (char_length(idempotency_key) >= 16),
    request_hash     TEXT        NOT NULL,
    created_by       UUID        NOT NULL,
    submitted_by     UUID,
    submitted_at     TIMESTAMPTZ,
    approved_by      UUID,
    approved_at      TIMESTAMPTZ,
    created_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (id, org_id),
    UNIQUE (org_id, idempotency_key),
    FOREIGN KEY (work_order_id, org_id) REFERENCES work_orders(id, org_id),
    FOREIGN KEY (branch_id, org_id) REFERENCES branches(id, org_id),
    FOREIGN KEY (created_by, org_id) REFERENCES users(id, org_id),
    FOREIGN KEY (submitted_by, org_id) REFERENCES users(id, org_id),
    FOREIGN KEY (approved_by, org_id) REFERENCES users(id, org_id)
);

-- Exactly one live (non-VOID) settlement per work order.
CREATE UNIQUE INDEX work_order_settlements_live_key
    ON work_order_settlements (work_order_id)
    WHERE status <> 'VOID';

CREATE TABLE work_order_settlement_lines (
    id            UUID   PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id        UUID   NOT NULL REFERENCES organizations(id),
    settlement_id UUID   NOT NULL,
    kind          TEXT   NOT NULL CHECK (kind IN ('LABOR','PART','OUTSOURCE','OTHER')),
    label         TEXT   NOT NULL CHECK (btrim(label) <> ''),
    amount_krw    BIGINT NOT NULL CHECK (amount_krw >= 0),
    source_ref    TEXT,
    sort_order    INT    NOT NULL DEFAULT 0,
    UNIQUE (id, org_id),
    FOREIGN KEY (settlement_id, org_id) REFERENCES work_order_settlements(id, org_id)
);

CREATE INDEX idx_work_order_settlement_lines_settlement
    ON work_order_settlement_lines (settlement_id, sort_order, id);

-- RLS: FORCE + org policy + immutable org, identical arming to work_orders.
DO $$ DECLARE t TEXT; tenant_tables TEXT[] := ARRAY['work_order_settlements','work_order_settlement_lines']; BEGIN
 FOREACH t IN ARRAY tenant_tables LOOP
  EXECUTE format('ALTER TABLE %I ENABLE ROW LEVEL SECURITY', t);
  EXECUTE format('ALTER TABLE %I FORCE ROW LEVEL SECURITY', t);
  EXECUTE format('CREATE POLICY org_isolation ON %I USING (org_id = NULLIF(current_setting(''app.current_org'', true), '''')::uuid) WITH CHECK (org_id = NULLIF(current_setting(''app.current_org'', true), '''')::uuid)', t);
  EXECUTE format('CREATE TRIGGER trg_%I_org_immutable BEFORE UPDATE ON %I FOR EACH ROW EXECUTE FUNCTION enforce_org_id_immutable()', t, t);
 END LOOP;
END $$;

-- Runtime role: settlements transition in place (status/submitted/approved
-- columns), lines are immutable once written; nothing is ever hard-deleted.
GRANT SELECT, INSERT, UPDATE ON work_order_settlements TO console_rt;
GRANT SELECT, INSERT ON work_order_settlement_lines TO console_rt;
