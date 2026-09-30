-- Production subcontracting pilot: owns plan/operation execution and immutable
-- lifecycle evidence only. Customer demand, people, inventory, approvals,
-- ontology, and reporting remain referenced ports, never copied stores.
-- Capacity and demand contracts are ingress-owned facts. Runtime planners can
-- read and reserve them, but cannot forge availability or customer demand.
CREATE TABLE production_demand_contracts (
    id UUID PRIMARY KEY,
    org_id UUID NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    inquiry_id UUID NOT NULL,
    product_code TEXT NOT NULL CHECK (btrim(product_code) <> '' AND char_length(product_code) <= 80),
    quantity BIGINT NOT NULL CHECK (quantity > 0),
    due_at TIMESTAMPTZ NOT NULL,
    source_system TEXT NOT NULL CHECK (btrim(source_system) <> '' AND char_length(source_system) <= 80),
    source_id TEXT NOT NULL CHECK (btrim(source_id) <> '' AND char_length(source_id) <= 160),
    source_version TEXT NOT NULL CHECK (btrim(source_version) <> '' AND char_length(source_version) <= 160),
    evaluated_at TIMESTAMPTZ NOT NULL,
    ingested_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (id, org_id),
    UNIQUE (org_id, source_system, source_id, source_version),
    FOREIGN KEY (inquiry_id, org_id) REFERENCES customer_inquiries(id, org_id) ON DELETE RESTRICT
);
CREATE TABLE production_capacity_slots (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id UUID NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    branch_id UUID NOT NULL,
    site_id UUID NOT NULL,
    capacity_date DATE NOT NULL,
    available_quantity BIGINT NOT NULL CHECK (available_quantity > 0),
    reserved_quantity BIGINT NOT NULL DEFAULT 0 CHECK (reserved_quantity >= 0 AND reserved_quantity <= available_quantity),
    version INTEGER NOT NULL DEFAULT 1 CHECK (version > 0),
    source_system TEXT NOT NULL CHECK (btrim(source_system) <> '' AND char_length(source_system) <= 80),
    source_id TEXT NOT NULL CHECK (btrim(source_id) <> '' AND char_length(source_id) <= 160),
    source_version TEXT NOT NULL CHECK (btrim(source_version) <> '' AND char_length(source_version) <= 160),
    evaluated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    ingested_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (id, org_id),
    UNIQUE (org_id, branch_id, site_id, capacity_date),
    FOREIGN KEY (branch_id, org_id) REFERENCES branches(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (site_id, org_id) REFERENCES registry_sites(id, org_id) ON DELETE RESTRICT
);
CREATE INDEX idx_production_capacity_slots_available ON production_capacity_slots (org_id, branch_id, capacity_date) WHERE reserved_quantity < available_quantity;

CREATE TABLE production_plans (
    id UUID PRIMARY KEY,
    org_id UUID NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    branch_id UUID NOT NULL,
    customer_demand_id UUID NOT NULL,
    product_code TEXT NOT NULL CHECK (btrim(product_code) <> '' AND char_length(product_code) <= 80),
    quantity BIGINT NOT NULL CHECK (quantity > 0),
    due_at TIMESTAMPTZ NOT NULL,
    checks JSONB NOT NULL,
    source_snapshot JSONB NOT NULL,
    status TEXT NOT NULL DEFAULT 'DRAFT' CHECK (status IN ('DRAFT', 'RELEASED')),
    version INTEGER NOT NULL DEFAULT 1 CHECK (version > 0),
    idempotency_key TEXT NOT NULL CHECK (btrim(idempotency_key) <> '' AND char_length(idempotency_key) <= 128),
    approval_ref UUID,
    ontology_type_id UUID NOT NULL,
    first_operation_id UUID NOT NULL UNIQUE,
    created_by UUID NOT NULL,
    released_by UUID,
    released_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (id, org_id),
    UNIQUE (org_id, idempotency_key),
    FOREIGN KEY (branch_id, org_id) REFERENCES branches(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (ontology_type_id, org_id) REFERENCES ont_object_types(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (created_by, org_id) REFERENCES users(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (released_by, org_id) REFERENCES users(id, org_id) ON DELETE RESTRICT,
    CHECK ((status = 'DRAFT' AND released_by IS NULL AND released_at IS NULL) OR (status = 'RELEASED' AND released_by IS NOT NULL AND released_at IS NOT NULL))
);
CREATE INDEX idx_production_plans_branch_created ON production_plans (org_id, branch_id, created_at DESC, id DESC);

CREATE TABLE production_operations (
    id UUID PRIMARY KEY,
    org_id UUID NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    plan_id UUID NOT NULL,
    sequence INTEGER NOT NULL CHECK (sequence > 0),
    status TEXT NOT NULL CHECK (status IN ('PENDING', 'RELEASED', 'RECORDED')),
    output_quantity BIGINT NOT NULL DEFAULT 0 CHECK (output_quantity >= 0),
    scrap_quantity BIGINT NOT NULL DEFAULT 0 CHECK (scrap_quantity >= 0),
    downtime_minutes INTEGER NOT NULL DEFAULT 0 CHECK (downtime_minutes >= 0),
    quality_evidence_ref TEXT CHECK (quality_evidence_ref IS NULL OR btrim(quality_evidence_ref) <> ''),
    quality_passed BOOLEAN,
    version INTEGER NOT NULL DEFAULT 1 CHECK (version > 0),
    UNIQUE (id, org_id),
    UNIQUE (plan_id, sequence),
    FOREIGN KEY (plan_id, org_id) REFERENCES production_plans(id, org_id) ON DELETE RESTRICT,
    CHECK ((status = 'RECORDED') = (quality_evidence_ref IS NOT NULL AND quality_passed IS NOT NULL))
);

CREATE TABLE production_plan_events (
    id UUID PRIMARY KEY,
    org_id UUID NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    plan_id UUID NOT NULL,
    event_type TEXT NOT NULL CHECK (event_type IN ('PLAN_CREATED', 'PLAN_RELEASED', 'OPERATION_RECORDED')),
    actor_id UUID NOT NULL,
    payload JSONB NOT NULL,
    idempotency_key TEXT NOT NULL,
    occurred_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (org_id, plan_id, idempotency_key),
    FOREIGN KEY (plan_id, org_id) REFERENCES production_plans(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (actor_id, org_id) REFERENCES users(id, org_id) ON DELETE RESTRICT
);
CREATE INDEX idx_production_plan_events_lineage ON production_plan_events (org_id, plan_id, occurred_at, id);

CREATE TABLE production_idempotency_claims (
    org_id UUID NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    operation TEXT NOT NULL CHECK (operation IN ('CREATE_PLAN','RELEASE_PLAN','RECORD_OPERATION')),
    idempotency_key TEXT NOT NULL CHECK (btrim(idempotency_key) <> '' AND char_length(idempotency_key) <= 128),
    request_hash TEXT NOT NULL CHECK (request_hash ~ '^[a-f0-9]{64}$'),
    response JSONB NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    completed_at TIMESTAMPTZ NULL,
    PRIMARY KEY (org_id, operation, idempotency_key)
);

ALTER TABLE production_plans ENABLE ROW LEVEL SECURITY;
ALTER TABLE production_plans FORCE ROW LEVEL SECURITY;
CREATE POLICY org_isolation ON production_plans USING (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid) WITH CHECK (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid);
CREATE TRIGGER trg_production_plans_org_immutable BEFORE UPDATE ON production_plans FOR EACH ROW EXECUTE FUNCTION enforce_org_id_immutable();
ALTER TABLE production_demand_contracts ENABLE ROW LEVEL SECURITY;
ALTER TABLE production_demand_contracts FORCE ROW LEVEL SECURITY;
CREATE POLICY org_isolation ON production_demand_contracts USING (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid) WITH CHECK (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid);
CREATE TRIGGER trg_production_demand_contracts_org_immutable BEFORE UPDATE ON production_demand_contracts FOR EACH ROW EXECUTE FUNCTION enforce_org_id_immutable();
ALTER TABLE production_capacity_slots ENABLE ROW LEVEL SECURITY;
ALTER TABLE production_capacity_slots FORCE ROW LEVEL SECURITY;
CREATE POLICY org_isolation ON production_capacity_slots USING (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid) WITH CHECK (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid);
CREATE TRIGGER trg_production_capacity_slots_org_immutable BEFORE UPDATE ON production_capacity_slots FOR EACH ROW EXECUTE FUNCTION enforce_org_id_immutable();
ALTER TABLE production_operations ENABLE ROW LEVEL SECURITY;
ALTER TABLE production_operations FORCE ROW LEVEL SECURITY;
CREATE POLICY org_isolation ON production_operations USING (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid) WITH CHECK (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid);
ALTER TABLE production_plan_events ENABLE ROW LEVEL SECURITY;
ALTER TABLE production_plan_events FORCE ROW LEVEL SECURITY;
CREATE POLICY org_isolation ON production_plan_events USING (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid) WITH CHECK (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid);
ALTER TABLE production_idempotency_claims ENABLE ROW LEVEL SECURITY;
ALTER TABLE production_idempotency_claims FORCE ROW LEVEL SECURITY;
CREATE POLICY org_isolation ON production_idempotency_claims USING (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid) WITH CHECK (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid);

GRANT SELECT, INSERT, UPDATE ON production_plans TO console_rt;
GRANT SELECT ON production_demand_contracts, production_capacity_slots TO console_rt;
GRANT SELECT, INSERT, UPDATE ON production_operations TO console_rt;
GRANT SELECT, INSERT ON production_plan_events TO console_rt;
GRANT SELECT, INSERT, UPDATE ON production_idempotency_claims TO console_rt;
REVOKE DELETE ON production_plans, production_operations, production_plan_events FROM console_rt;
REVOKE INSERT, UPDATE, DELETE ON production_demand_contracts, production_capacity_slots FROM console_rt;
REVOKE DELETE ON production_idempotency_claims FROM console_rt;
