-- Bounded equipment 3R pilot. Deliberately independent of registry_equipment,
-- work orders, inventory, financial quotes, and finance-gl.
-- Register every capability before any tenant policy can grant it. The routes
-- remain fail-closed: catalog presence is only a prerequisite for an explicit
-- ACTIVE custom-role assignment, never an implicit permission.
INSERT INTO feature_catalog (feature_key) VALUES
    ('equipment_3r_registry'), ('equipment_3r_quote'), ('equipment_3r_approve'),
    ('equipment_3r_dispatch'), ('equipment_3r_inspect'), ('equipment_3r_assess'),
    ('equipment_3r_disposition'), ('equipment_3r_observe')
ON CONFLICT (feature_key) DO NOTHING;

CREATE TABLE equipment_3r_units (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id UUID NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    branch_id UUID NOT NULL,
    serial_no TEXT NOT NULL CHECK (char_length(btrim(serial_no)) BETWEEN 1 AND 80),
    model_name TEXT NOT NULL CHECK (char_length(btrim(model_name)) BETWEEN 1 AND 120),
    capacity_class TEXT NOT NULL CHECK (char_length(btrim(capacity_class)) BETWEEN 1 AND 40),
    acquisition_cost_minor BIGINT NOT NULL CHECK (acquisition_cost_minor >= 0),
    availability TEXT NOT NULL DEFAULT 'AVAILABLE' CHECK (availability IN
        ('AVAILABLE','RESERVED','ON_RENT','IN_ASSESSMENT','IN_REPAIR','IN_REFURBISHMENT','FOR_SALE','SOLD')),
    created_by UUID NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(), updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (id, org_id), UNIQUE (org_id, serial_no),
    FOREIGN KEY (branch_id, org_id) REFERENCES branches(id, org_id) ON DELETE RESTRICT
);
CREATE TABLE equipment_3r_rental_cases (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id UUID NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    branch_id UUID NOT NULL, unit_id UUID NOT NULL,
    customer_name TEXT NOT NULL CHECK (char_length(btrim(customer_name)) BETWEEN 1 AND 160),
    site_reference TEXT NOT NULL CHECK (char_length(btrim(site_reference)) BETWEEN 1 AND 200),
    monthly_rate_minor BIGINT NOT NULL CHECK (monthly_rate_minor > 0),
    duration_months INTEGER NOT NULL CHECK (duration_months BETWEEN 1 AND 120),
    currency_code TEXT NOT NULL CHECK (currency_code = 'KRW'),
    status TEXT NOT NULL DEFAULT 'QUOTED' CHECK (status IN
        ('QUOTED','APPROVED','DECLINED','DISPATCHED','HANDED_OVER','RETURNED','CLOSED')),
    approval_decision TEXT NULL CHECK (approval_decision IS NULL OR approval_decision IN ('APPROVED','DECLINED')),
    approval_reason TEXT NULL CHECK (approval_reason IS NULL OR char_length(btrim(approval_reason)) BETWEEN 1 AND 500),
    approved_by UUID NULL REFERENCES users(id) ON DELETE RESTRICT, approved_at TIMESTAMPTZ NULL,
    carrier_name TEXT NULL CHECK (carrier_name IS NULL OR char_length(btrim(carrier_name)) BETWEEN 1 AND 120),
    vehicle_reference TEXT NULL CHECK (vehicle_reference IS NULL OR char_length(btrim(vehicle_reference)) BETWEEN 1 AND 120),
    dispatched_at TIMESTAMPTZ NULL,
    recipient_name TEXT NULL CHECK (recipient_name IS NULL OR char_length(btrim(recipient_name)) BETWEEN 1 AND 160),
    -- Postgres caps regex repetition bounds at 255, so the 8..400 length
    -- window lives in char_length ('evidence://' scheme is 11 characters).
    handover_evidence_reference TEXT NULL CHECK (handover_evidence_reference IS NULL
        OR (handover_evidence_reference ~ '^evidence://[A-Za-z0-9._/-]+$'
            AND char_length(handover_evidence_reference) BETWEEN 19 AND 411)),
    handed_over_at TIMESTAMPTZ NULL, returned_at TIMESTAMPTZ NULL,
    idempotency_key TEXT NOT NULL CHECK (char_length(btrim(idempotency_key)) BETWEEN 16 AND 200),
    request_fingerprint TEXT NOT NULL CHECK (request_fingerprint ~ '^[a-f0-9]{64}$'),
    created_by UUID NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(), updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (id, org_id), UNIQUE (org_id, idempotency_key),
    FOREIGN KEY (branch_id, org_id) REFERENCES branches(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (unit_id, org_id) REFERENCES equipment_3r_units(id, org_id) ON DELETE RESTRICT
);
CREATE INDEX idx_equipment_3r_cases_unit ON equipment_3r_rental_cases (org_id, unit_id, created_at DESC);
CREATE UNIQUE INDEX uq_equipment_3r_cases_active_unit ON equipment_3r_rental_cases (org_id, unit_id)
    WHERE status IN ('APPROVED','DISPATCHED','HANDED_OVER','RETURNED');
CREATE TABLE equipment_3r_inspections (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id UUID NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    branch_id UUID NOT NULL, case_id UUID NOT NULL,
    outcome TEXT NOT NULL CHECK (outcome IN ('PASS','MAINTENANCE_PERFORMED')),
    findings TEXT NOT NULL CHECK (char_length(btrim(findings)) BETWEEN 1 AND 2000),
    maintenance_note TEXT NULL CHECK (maintenance_note IS NULL OR char_length(btrim(maintenance_note)) BETWEEN 1 AND 2000),
    inspected_by UUID NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    inspected_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (id, org_id),
    FOREIGN KEY (branch_id, org_id) REFERENCES branches(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (case_id, org_id) REFERENCES equipment_3r_rental_cases(id, org_id) ON DELETE RESTRICT,
    CONSTRAINT maintenance_note_matches_outcome CHECK (
        (outcome = 'MAINTENANCE_PERFORMED') = (maintenance_note IS NOT NULL))
);
CREATE TABLE equipment_3r_return_assessments (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id UUID NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    branch_id UUID NOT NULL, case_id UUID NOT NULL,
    condition_grade TEXT NOT NULL CHECK (condition_grade IN ('A','B','C','D')),
    findings TEXT NOT NULL CHECK (char_length(btrim(findings)) BETWEEN 1 AND 2000),
    disposition TEXT NOT NULL CHECK (disposition IN ('REPAIR','REFURBISH','RESALE','REDEPLOY')),
    assessed_by UUID NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    assessed_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (id, org_id), UNIQUE (org_id, case_id),
    FOREIGN KEY (branch_id, org_id) REFERENCES branches(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (case_id, org_id) REFERENCES equipment_3r_rental_cases(id, org_id) ON DELETE RESTRICT
);
CREATE TABLE equipment_3r_dispositions (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id UUID NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    branch_id UUID NOT NULL, unit_id UUID NOT NULL, case_id UUID NOT NULL, assessment_id UUID NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('REPAIR','REFURBISH','RESALE','REDEPLOY')),
    status TEXT NOT NULL DEFAULT 'OPEN' CHECK (status IN ('OPEN','COMPLETED')),
    cost_minor BIGINT NULL CHECK (cost_minor IS NULL OR cost_minor >= 0),
    sale_amount_minor BIGINT NULL CHECK (sale_amount_minor IS NULL OR sale_amount_minor >= 0),
    buyer_name TEXT NULL CHECK (buyer_name IS NULL OR char_length(btrim(buyer_name)) BETWEEN 1 AND 160),
    completed_by UUID NULL REFERENCES users(id) ON DELETE RESTRICT, completed_at TIMESTAMPTZ NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(), updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (id, org_id), UNIQUE (org_id, assessment_id),
    FOREIGN KEY (branch_id, org_id) REFERENCES branches(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (unit_id, org_id) REFERENCES equipment_3r_units(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (case_id, org_id) REFERENCES equipment_3r_rental_cases(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (assessment_id, org_id) REFERENCES equipment_3r_return_assessments(id, org_id) ON DELETE RESTRICT
);
CREATE UNIQUE INDEX uq_equipment_3r_dispositions_open_unit ON equipment_3r_dispositions (org_id, unit_id)
    WHERE status = 'OPEN';
CREATE TABLE equipment_3r_history (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id UUID NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT, branch_id UUID NOT NULL,
    aggregate_kind TEXT NOT NULL CHECK (aggregate_kind IN ('unit','case','disposition')),
    aggregate_id UUID NOT NULL, transition TEXT NOT NULL,
    actor_id UUID NOT NULL REFERENCES users(id), occurred_at TIMESTAMPTZ NOT NULL, trace_id UUID NOT NULL,
    UNIQUE (id, org_id), FOREIGN KEY (branch_id, org_id) REFERENCES branches(id, org_id) ON DELETE RESTRICT
);
CREATE INDEX idx_equipment_3r_history_aggregate ON equipment_3r_history (org_id, aggregate_id, occurred_at DESC);

-- RLS: every pilot object is tenant-concealed.
DO $$ DECLARE t TEXT; BEGIN FOREACH t IN ARRAY ARRAY['equipment_3r_units','equipment_3r_rental_cases',
 'equipment_3r_inspections','equipment_3r_return_assessments','equipment_3r_dispositions','equipment_3r_history'] LOOP
 EXECUTE format('ALTER TABLE %I ENABLE ROW LEVEL SECURITY', t);
 EXECUTE format('ALTER TABLE %I FORCE ROW LEVEL SECURITY', t);
 EXECUTE format('CREATE POLICY org_isolation ON %I USING (org_id = NULLIF(current_setting(''app.current_org'', true), '''')::uuid) WITH CHECK (org_id = NULLIF(current_setting(''app.current_org'', true), '''')::uuid)', t);
 EXECUTE format('GRANT SELECT, INSERT, UPDATE ON %I TO console_rt', t);
 EXECUTE format('CREATE TRIGGER trg_%s_org_immutable BEFORE UPDATE ON %I FOR EACH ROW EXECUTE FUNCTION enforce_org_id_immutable()', t, t);
END LOOP; END $$;
-- Field access must stay per-table: PL/pgSQL resolves OLD.<field> for the
-- whole expression, so a units row would error on OLD.status even behind
-- a false TG_TABLE_NAME guard.
CREATE OR REPLACE FUNCTION equipment_3r_terminal_immutable() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN
 IF TG_TABLE_NAME = 'equipment_3r_units' THEN
   IF OLD.availability = 'SOLD' AND NEW.availability <> OLD.availability
     THEN RAISE EXCEPTION 'sold equipment unit is immutable'; END IF;
 ELSIF TG_TABLE_NAME = 'equipment_3r_rental_cases' THEN
   IF OLD.status IN ('DECLINED','CLOSED') AND NEW.status <> OLD.status
     THEN RAISE EXCEPTION 'terminal rental case is immutable'; END IF;
 ELSIF TG_TABLE_NAME = 'equipment_3r_dispositions' THEN
   IF OLD.status = 'COMPLETED' AND NEW.status <> OLD.status
     THEN RAISE EXCEPTION 'completed disposition is immutable'; END IF;
 END IF;
 RETURN NEW; END $$;
CREATE TRIGGER trg_equipment_3r_units_terminal BEFORE UPDATE ON equipment_3r_units FOR EACH ROW EXECUTE FUNCTION equipment_3r_terminal_immutable();
CREATE TRIGGER trg_equipment_3r_cases_terminal BEFORE UPDATE ON equipment_3r_rental_cases FOR EACH ROW EXECUTE FUNCTION equipment_3r_terminal_immutable();
CREATE TRIGGER trg_equipment_3r_dispositions_terminal BEFORE UPDATE ON equipment_3r_dispositions FOR EACH ROW EXECUTE FUNCTION equipment_3r_terminal_immutable();
CREATE OR REPLACE FUNCTION equipment_3r_history_append_only() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'equipment 3R history is immutable'; END $$;
CREATE TRIGGER trg_equipment_3r_history_no_update BEFORE UPDATE OR DELETE ON equipment_3r_history FOR EACH ROW EXECUTE FUNCTION equipment_3r_history_append_only();
