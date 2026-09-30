-- Forward-only hardening for the 0173 production pilot. 0172-0175 are owned
-- by other integration lanes and must never be repurposed.
CREATE TABLE production_source_systems (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id UUID NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    branch_id UUID NOT NULL,
    principal_id UUID NOT NULL,
    source_system TEXT NOT NULL CHECK (btrim(source_system) <> '' AND char_length(source_system) <= 80),
    credential_hash TEXT NOT NULL CHECK (credential_hash ~ '^[a-f0-9]{64}$'),
    enabled BOOLEAN NOT NULL DEFAULT true,
    credential_generation INTEGER NOT NULL DEFAULT 1 CHECK (credential_generation > 0),
    registered_by UUID NOT NULL,
    registered_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    rotated_by UUID,
    rotated_at TIMESTAMPTZ,
    disabled_by UUID,
    disabled_at TIMESTAMPTZ,
    UNIQUE (id, org_id),
    UNIQUE (org_id, principal_id),
    UNIQUE (org_id, source_system),
    FOREIGN KEY (branch_id, org_id) REFERENCES branches(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (principal_id, org_id) REFERENCES users(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (registered_by, org_id) REFERENCES users(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (rotated_by, org_id) REFERENCES users(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (disabled_by, org_id) REFERENCES users(id, org_id) ON DELETE RESTRICT
);
CREATE TABLE production_source_ingress_claims (
    org_id UUID NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    source_system_id UUID NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('DEMAND', 'CAPACITY', 'MATERIAL')),
    source_id TEXT NOT NULL CHECK (btrim(source_id) <> '' AND char_length(source_id) <= 160),
    source_version TEXT NOT NULL CHECK (btrim(source_version) <> '' AND char_length(source_version) <= 160),
    payload_hash TEXT NOT NULL CHECK (payload_hash ~ '^[a-f0-9]{64}$'),
    response JSONB,
    ingested_by UUID NOT NULL,
    ingested_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    completed_at TIMESTAMPTZ,
    PRIMARY KEY (org_id, source_system_id, kind, source_id, source_version),
    FOREIGN KEY (source_system_id, org_id) REFERENCES production_source_systems(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (ingested_by, org_id) REFERENCES users(id, org_id) ON DELETE RESTRICT
);

ALTER TABLE production_plans ADD COLUMN plan_digest TEXT;
UPDATE production_plans SET plan_digest = encode(digest(convert_to(source_snapshot::text, 'UTF8'), 'sha256'), 'hex') WHERE plan_digest IS NULL;
ALTER TABLE production_plans ALTER COLUMN plan_digest SET NOT NULL;
ALTER TABLE production_plans ADD CONSTRAINT production_plans_plan_digest_sha256 CHECK (plan_digest ~ '^[a-f0-9]{64}$');

CREATE OR REPLACE FUNCTION production_plan_immutable_content()
RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.branch_id IS DISTINCT FROM OLD.branch_id
       OR NEW.customer_demand_id IS DISTINCT FROM OLD.customer_demand_id
       OR NEW.product_code IS DISTINCT FROM OLD.product_code
       OR NEW.quantity IS DISTINCT FROM OLD.quantity
       OR NEW.due_at IS DISTINCT FROM OLD.due_at
       OR NEW.checks IS DISTINCT FROM OLD.checks
       OR NEW.source_snapshot IS DISTINCT FROM OLD.source_snapshot
       OR NEW.idempotency_key IS DISTINCT FROM OLD.idempotency_key
       OR NEW.ontology_type_id IS DISTINCT FROM OLD.ontology_type_id
       OR NEW.first_operation_id IS DISTINCT FROM OLD.first_operation_id
       OR NEW.created_by IS DISTINCT FROM OLD.created_by
       OR NEW.plan_digest IS DISTINCT FROM OLD.plan_digest THEN
        RAISE EXCEPTION 'production plan content is immutable';
    END IF;
    IF OLD.status <> 'DRAFT' OR NEW.status NOT IN ('DRAFT', 'RELEASED')
       OR (NEW.status = 'RELEASED' AND (NEW.approval_ref IS NULL OR NEW.released_by IS NULL OR NEW.released_at IS NULL)) THEN
        RAISE EXCEPTION 'invalid production plan terminal transition';
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER trg_production_plans_immutable_content BEFORE UPDATE ON production_plans
FOR EACH ROW EXECUTE FUNCTION production_plan_immutable_content();

ALTER TABLE production_source_systems ENABLE ROW LEVEL SECURITY;
ALTER TABLE production_source_systems FORCE ROW LEVEL SECURITY;
CREATE POLICY org_isolation ON production_source_systems USING (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid) WITH CHECK (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid);
ALTER TABLE production_source_ingress_claims ENABLE ROW LEVEL SECURITY;
ALTER TABLE production_source_ingress_claims FORCE ROW LEVEL SECURITY;
CREATE POLICY org_isolation ON production_source_ingress_claims USING (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid) WITH CHECK (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid);

GRANT SELECT, INSERT, UPDATE ON production_source_systems, production_source_ingress_claims TO console_rt;
GRANT INSERT, UPDATE ON production_demand_contracts, production_capacity_slots TO console_rt;
REVOKE DELETE ON production_source_systems, production_source_ingress_claims, production_demand_contracts, production_capacity_slots FROM console_rt;
