-- Bounded logistics pilot.  This is deliberately independent of inventory,
-- work orders, and P1 dispatch: it tracks physical pilot stock only.
-- Register every capability before any tenant policy can grant it. The routes
-- remain fail-closed: catalog presence is only a prerequisite for an explicit
-- ACTIVE custom-role assignment, never an implicit permission.
INSERT INTO feature_catalog (feature_key) VALUES
    ('logistics_receive'),
    ('logistics_putaway'),
    ('logistics_release'),
    ('logistics_pick_pack'),
    ('logistics_dispatch'),
    ('logistics_pod'),
    ('logistics_settle')
ON CONFLICT (feature_key) DO NOTHING;

CREATE TABLE logistics_asns (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id UUID NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    branch_id UUID NOT NULL,
    warehouse_code TEXT NOT NULL CHECK (char_length(btrim(warehouse_code)) BETWEEN 1 AND 80),
    external_reference TEXT NOT NULL CHECK (char_length(btrim(external_reference)) BETWEEN 1 AND 120),
    sku TEXT NOT NULL CHECK (char_length(btrim(sku)) BETWEEN 1 AND 80),
    expected_quantity BIGINT NOT NULL CHECK (expected_quantity > 0),
    received_quantity BIGINT NOT NULL DEFAULT 0 CHECK (received_quantity >= 0 AND received_quantity <= expected_quantity),
    status TEXT NOT NULL DEFAULT 'EXPECTED' CHECK (status IN ('EXPECTED','PARTIAL_RECEIVED','RECEIVED','PUTAWAY')),
    created_by UUID NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(), updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (id, org_id), UNIQUE (org_id, external_reference),
    FOREIGN KEY (branch_id, org_id) REFERENCES branches(id, org_id) ON DELETE RESTRICT
);
CREATE TABLE logistics_receipts (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(), org_id UUID NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    branch_id UUID NOT NULL, asn_id UUID NOT NULL, received_quantity BIGINT NOT NULL CHECK (received_quantity > 0),
    exception_code TEXT NULL CHECK (exception_code IS NULL OR exception_code IN ('PARTIAL_RECEIPT')),
    received_by UUID NOT NULL REFERENCES users(id) ON DELETE RESTRICT, received_at TIMESTAMPTZ NOT NULL,
    idempotency_key TEXT NOT NULL CHECK (char_length(btrim(idempotency_key)) BETWEEN 16 AND 200), request_fingerprint TEXT NOT NULL CHECK (request_fingerprint ~ '^[a-f0-9]{64}$'),
    UNIQUE (id, org_id), UNIQUE (org_id, idempotency_key), FOREIGN KEY (branch_id, org_id) REFERENCES branches(id, org_id), FOREIGN KEY (asn_id, org_id) REFERENCES logistics_asns(id, org_id)
);
CREATE TABLE logistics_stock (
    org_id UUID NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT, branch_id UUID NOT NULL,
    warehouse_code TEXT NOT NULL, sku TEXT NOT NULL, quantity_on_hand BIGINT NOT NULL DEFAULT 0 CHECK (quantity_on_hand >= 0),
    quantity_reserved BIGINT NOT NULL DEFAULT 0 CHECK (quantity_reserved >= 0 AND quantity_reserved <= quantity_on_hand),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(), PRIMARY KEY (org_id, branch_id, warehouse_code, sku),
    FOREIGN KEY (branch_id, org_id) REFERENCES branches(id, org_id) ON DELETE RESTRICT
);
CREATE TABLE logistics_fulfillments (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(), org_id UUID NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    branch_id UUID NOT NULL, warehouse_code TEXT NOT NULL, sku TEXT NOT NULL, requested_quantity BIGINT NOT NULL CHECK (requested_quantity > 0),
    reserved_quantity BIGINT NOT NULL CHECK (reserved_quantity >= 0 AND reserved_quantity <= requested_quantity), picked_quantity BIGINT NOT NULL DEFAULT 0 CHECK (picked_quantity >= 0 AND picked_quantity <= reserved_quantity),
    status TEXT NOT NULL DEFAULT 'RELEASED' CHECK (status IN ('RELEASED','PICKED','SHORT_PICK','PACKED','DISPATCHED','DELIVERED','SETTLED')),
    due_at TIMESTAMPTZ NOT NULL, created_by UUID NOT NULL REFERENCES users(id) ON DELETE RESTRICT, created_at TIMESTAMPTZ NOT NULL DEFAULT now(), updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (id, org_id), FOREIGN KEY (branch_id, org_id) REFERENCES branches(id, org_id)
);
CREATE TABLE logistics_shipments (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(), org_id UUID NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    branch_id UUID NOT NULL, fulfillment_id UUID NOT NULL, carrier_name TEXT NOT NULL CHECK (char_length(btrim(carrier_name)) BETWEEN 1 AND 120), vehicle_reference TEXT NOT NULL CHECK (char_length(btrim(vehicle_reference)) BETWEEN 1 AND 120),
    dispatched_at TIMESTAMPTZ NOT NULL, status TEXT NOT NULL DEFAULT 'DISPATCHED' CHECK (status IN ('DISPATCHED','DELIVERED','SETTLED')),
    UNIQUE (id, org_id), UNIQUE (org_id, fulfillment_id), FOREIGN KEY (branch_id, org_id) REFERENCES branches(id, org_id), FOREIGN KEY (fulfillment_id, org_id) REFERENCES logistics_fulfillments(id, org_id)
);
CREATE TABLE logistics_pod_evidence (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(), org_id UUID NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    branch_id UUID NOT NULL, shipment_id UUID NOT NULL, recipient_name TEXT NOT NULL CHECK (char_length(btrim(recipient_name)) BETWEEN 1 AND 160),
    evidence_reference TEXT NOT NULL CHECK (evidence_reference ~ '^evidence://[A-Za-z0-9._/-]{8,400}$'), confirmed_at TIMESTAMPTZ NOT NULL,
    UNIQUE (id, org_id), UNIQUE (org_id, shipment_id), UNIQUE (org_id, evidence_reference), FOREIGN KEY (branch_id, org_id) REFERENCES branches(id, org_id), FOREIGN KEY (shipment_id, org_id) REFERENCES logistics_shipments(id, org_id)
);
CREATE TABLE logistics_operational_cost_settlements (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(), org_id UUID NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    branch_id UUID NOT NULL, shipment_id UUID NOT NULL, currency_code TEXT NOT NULL CHECK (currency_code = 'KRW'), amount_minor BIGINT NOT NULL CHECK (amount_minor >= 0),
    settled_at TIMESTAMPTZ NOT NULL, UNIQUE (id, org_id), UNIQUE (org_id, shipment_id), FOREIGN KEY (branch_id, org_id) REFERENCES branches(id, org_id), FOREIGN KEY (shipment_id, org_id) REFERENCES logistics_shipments(id, org_id)
);
CREATE TABLE logistics_history (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(), org_id UUID NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT, branch_id UUID NOT NULL,
    aggregate_kind TEXT NOT NULL, aggregate_id UUID NOT NULL, transition TEXT NOT NULL, actor_id UUID NOT NULL REFERENCES users(id), occurred_at TIMESTAMPTZ NOT NULL, trace_id UUID NOT NULL,
    UNIQUE (id, org_id), FOREIGN KEY (branch_id, org_id) REFERENCES branches(id, org_id)
);

-- Every logistics object is tenant concealed, including stock and immutable proof.
ALTER TABLE logistics_asns ENABLE ROW LEVEL SECURITY; ALTER TABLE logistics_asns FORCE ROW LEVEL SECURITY;
ALTER TABLE logistics_receipts ENABLE ROW LEVEL SECURITY; ALTER TABLE logistics_receipts FORCE ROW LEVEL SECURITY;
ALTER TABLE logistics_stock ENABLE ROW LEVEL SECURITY; ALTER TABLE logistics_stock FORCE ROW LEVEL SECURITY;
ALTER TABLE logistics_fulfillments ENABLE ROW LEVEL SECURITY; ALTER TABLE logistics_fulfillments FORCE ROW LEVEL SECURITY;
ALTER TABLE logistics_shipments ENABLE ROW LEVEL SECURITY; ALTER TABLE logistics_shipments FORCE ROW LEVEL SECURITY;
ALTER TABLE logistics_pod_evidence ENABLE ROW LEVEL SECURITY; ALTER TABLE logistics_pod_evidence FORCE ROW LEVEL SECURITY;
ALTER TABLE logistics_operational_cost_settlements ENABLE ROW LEVEL SECURITY; ALTER TABLE logistics_operational_cost_settlements FORCE ROW LEVEL SECURITY;
ALTER TABLE logistics_history ENABLE ROW LEVEL SECURITY; ALTER TABLE logistics_history FORCE ROW LEVEL SECURITY;
DO $$ DECLARE t TEXT; BEGIN FOREACH t IN ARRAY ARRAY['logistics_asns','logistics_receipts','logistics_stock','logistics_fulfillments','logistics_shipments','logistics_pod_evidence','logistics_operational_cost_settlements','logistics_history'] LOOP
 EXECUTE format('CREATE POLICY org_isolation ON %I USING (org_id = NULLIF(current_setting(''app.current_org'', true), '''')::uuid) WITH CHECK (org_id = NULLIF(current_setting(''app.current_org'', true), '''')::uuid)', t);
 EXECUTE format('GRANT SELECT, INSERT, UPDATE ON %I TO console_rt', t); END LOOP; END $$;
CREATE TRIGGER trg_logistics_asns_org_immutable BEFORE UPDATE ON logistics_asns FOR EACH ROW EXECUTE FUNCTION enforce_org_id_immutable();
CREATE TRIGGER trg_logistics_receipts_org_immutable BEFORE UPDATE ON logistics_receipts FOR EACH ROW EXECUTE FUNCTION enforce_org_id_immutable();
CREATE TRIGGER trg_logistics_fulfillments_org_immutable BEFORE UPDATE ON logistics_fulfillments FOR EACH ROW EXECUTE FUNCTION enforce_org_id_immutable();
CREATE TRIGGER trg_logistics_shipments_org_immutable BEFORE UPDATE ON logistics_shipments FOR EACH ROW EXECUTE FUNCTION enforce_org_id_immutable();
CREATE TRIGGER trg_logistics_pod_org_immutable BEFORE UPDATE ON logistics_pod_evidence FOR EACH ROW EXECUTE FUNCTION enforce_org_id_immutable();
CREATE TRIGGER trg_logistics_cost_org_immutable BEFORE UPDATE ON logistics_operational_cost_settlements FOR EACH ROW EXECUTE FUNCTION enforce_org_id_immutable();
CREATE TRIGGER trg_logistics_history_org_immutable BEFORE UPDATE ON logistics_history FOR EACH ROW EXECUTE FUNCTION enforce_org_id_immutable();
CREATE OR REPLACE FUNCTION logistics_terminal_immutable() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN
 IF OLD.status IN ('DELIVERED','SETTLED') AND NEW.status <> OLD.status THEN RAISE EXCEPTION 'terminal logistics state is immutable'; END IF; RETURN NEW; END $$;
CREATE TRIGGER trg_logistics_fulfillments_terminal BEFORE UPDATE ON logistics_fulfillments FOR EACH ROW EXECUTE FUNCTION logistics_terminal_immutable();
CREATE TRIGGER trg_logistics_shipments_terminal BEFORE UPDATE ON logistics_shipments FOR EACH ROW EXECUTE FUNCTION logistics_terminal_immutable();
CREATE OR REPLACE FUNCTION logistics_history_append_only() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'logistics history is immutable'; END $$;
CREATE TRIGGER trg_logistics_history_no_update BEFORE UPDATE OR DELETE ON logistics_history FOR EACH ROW EXECUTE FUNCTION logistics_history_append_only();
