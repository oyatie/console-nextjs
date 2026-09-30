-- CAP-IFM-PILOT: facilities is deliberately separate from equipment-required
-- legacy work_orders.  Slice 1 is scheduled HVAC preventive maintenance only.
INSERT INTO feature_catalog (feature_key) VALUES
    ('facilities_manage'), ('facilities_dispatch'), ('facilities_execute'),
    ('facilities_accept'), ('facilities_observe')
ON CONFLICT (feature_key) DO NOTHING;

CREATE TABLE facilities_spaces (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(), org_id UUID NOT NULL REFERENCES organizations(id),
    branch_id UUID NOT NULL, site_id UUID NOT NULL, name TEXT NOT NULL CHECK (btrim(name) <> ''),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (id, org_id), UNIQUE (org_id, site_id, name),
    FOREIGN KEY (branch_id, org_id) REFERENCES branches(id, org_id),
    FOREIGN KEY (site_id, org_id) REFERENCES registry_sites(id, org_id)
);
CREATE TABLE facilities_catalog_services (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(), org_id UUID NOT NULL REFERENCES organizations(id),
    service_key TEXT NOT NULL CHECK (service_key = 'HVAC_PREVENTIVE_MAINTENANCE'),
    name TEXT NOT NULL CHECK (btrim(name) <> ''), created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (id, org_id), UNIQUE (org_id, service_key)
);
CREATE TABLE facilities_assets (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(), org_id UUID NOT NULL REFERENCES organizations(id),
    branch_id UUID NOT NULL, site_id UUID NOT NULL, space_id UUID NULL, catalog_service_id UUID NOT NULL,
    asset_tag TEXT NOT NULL CHECK (btrim(asset_tag) <> ''), name TEXT NOT NULL CHECK (btrim(name) <> ''),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(), UNIQUE (id, org_id), UNIQUE (org_id, asset_tag),
    FOREIGN KEY (branch_id, org_id) REFERENCES branches(id, org_id),
    FOREIGN KEY (site_id, org_id) REFERENCES registry_sites(id, org_id),
    FOREIGN KEY (space_id, org_id) REFERENCES facilities_spaces(id, org_id),
    FOREIGN KEY (catalog_service_id, org_id) REFERENCES facilities_catalog_services(id, org_id)
);
CREATE TABLE facilities_obligations (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(), org_id UUID NOT NULL REFERENCES organizations(id),
    branch_id UUID NOT NULL, site_id UUID NOT NULL, asset_id UUID NOT NULL, catalog_service_id UUID NOT NULL,
    recurrence_days INTEGER NOT NULL CHECK (recurrence_days > 0), next_due_at TIMESTAMPTZ NOT NULL,
    response_due_seconds INTEGER NOT NULL CHECK (response_due_seconds > 0),
    completion_due_seconds INTEGER NOT NULL CHECK (completion_due_seconds > 0),
    acceptance_due_seconds INTEGER NOT NULL CHECK (acceptance_due_seconds > 0),
    customer_acceptance_required BOOLEAN NOT NULL DEFAULT true,
    target_energy_kwh NUMERIC(14,3) NOT NULL CHECK (target_energy_kwh >= 0),
    energy_formula TEXT NOT NULL DEFAULT 'post_kwh - pre_kwh' CHECK (energy_formula = 'post_kwh - pre_kwh'),
    active BOOLEAN NOT NULL DEFAULT true, created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (id, org_id), FOREIGN KEY (branch_id, org_id) REFERENCES branches(id, org_id),
    FOREIGN KEY (site_id, org_id) REFERENCES registry_sites(id, org_id),
    FOREIGN KEY (asset_id, org_id) REFERENCES facilities_assets(id, org_id),
    FOREIGN KEY (catalog_service_id, org_id) REFERENCES facilities_catalog_services(id, org_id)
);
CREATE TABLE facilities_cases (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(), org_id UUID NOT NULL REFERENCES organizations(id),
    branch_id UUID NOT NULL, site_id UUID NOT NULL, obligation_id UUID NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('DUE','TRIAGED','SCHEDULED','ASSIGNED','IN_PROGRESS','SUBMITTED','REWORK_REQUIRED','AWAITING_ACCEPTANCE','CLOSED')),
    assignee_id UUID NULL, scheduled_for TIMESTAMPTZ NULL, safety_acknowledged_at TIMESTAMPTZ NULL,
    response_due_at TIMESTAMPTZ NOT NULL, completion_due_at TIMESTAMPTZ NOT NULL, acceptance_due_at TIMESTAMPTZ NOT NULL,
    occurrence_due_at TIMESTAMPTZ NOT NULL, request_hash TEXT NOT NULL, idempotency_key TEXT NOT NULL, created_at TIMESTAMPTZ NOT NULL DEFAULT now(), updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (id, org_id), UNIQUE (org_id, obligation_id, occurrence_due_at), UNIQUE (org_id, idempotency_key),
    FOREIGN KEY (branch_id, org_id) REFERENCES branches(id, org_id), FOREIGN KEY (site_id, org_id) REFERENCES registry_sites(id, org_id),
    FOREIGN KEY (obligation_id, org_id) REFERENCES facilities_obligations(id, org_id),
    FOREIGN KEY (assignee_id, org_id) REFERENCES users(id, org_id)
);
CREATE TABLE facilities_case_history (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(), org_id UUID NOT NULL REFERENCES organizations(id), case_id UUID NOT NULL,
    from_status TEXT NULL, to_status TEXT NOT NULL, actor_id UUID NULL, occurred_at TIMESTAMPTZ NOT NULL DEFAULT now(), receipt JSONB NOT NULL DEFAULT '{}'::jsonb,
    UNIQUE (id, org_id), FOREIGN KEY (case_id, org_id) REFERENCES facilities_cases(id, org_id), FOREIGN KEY (actor_id, org_id) REFERENCES users(id, org_id)
);
CREATE TABLE facilities_execution_evidence_links (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(), org_id UUID NOT NULL REFERENCES organizations(id), case_id UUID NOT NULL,
    evidence_id UUID NOT NULL, evidence_kind TEXT NOT NULL CHECK (evidence_kind IN ('SAFETY_CHECKLIST','SERVICE_REPORT','PHOTO')),
    linked_by UUID NOT NULL, linked_at TIMESTAMPTZ NOT NULL DEFAULT now(), UNIQUE (org_id, case_id, evidence_kind),
    UNIQUE (id, org_id), FOREIGN KEY (case_id, org_id) REFERENCES facilities_cases(id, org_id), FOREIGN KEY (linked_by, org_id) REFERENCES users(id, org_id)
);
CREATE TABLE facilities_acceptances (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(), org_id UUID NOT NULL REFERENCES organizations(id), case_id UUID NOT NULL,
    decision TEXT NOT NULL CHECK (decision IN ('ACCEPTED','REJECTED')), reason TEXT NULL, actor_id UUID NOT NULL, decided_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (id, org_id), FOREIGN KEY (case_id, org_id) REFERENCES facilities_cases(id, org_id), FOREIGN KEY (actor_id, org_id) REFERENCES users(id, org_id)
);
CREATE TABLE facilities_energy_observations (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(), org_id UUID NOT NULL REFERENCES organizations(id), case_id UUID NOT NULL,
    phase TEXT NOT NULL CHECK (phase IN ('PRE','POST')), source TEXT NOT NULL CHECK (source = 'MANUAL'), observed_at TIMESTAMPTZ NOT NULL,
    kwh NUMERIC(14,3) NOT NULL CHECK (kwh >= 0), recorded_by UUID NOT NULL, created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (org_id, case_id, phase), UNIQUE (id, org_id), FOREIGN KEY (case_id, org_id) REFERENCES facilities_cases(id, org_id), FOREIGN KEY (recorded_by, org_id) REFERENCES users(id, org_id)
);
CREATE TABLE facilities_cost_observations (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(), org_id UUID NOT NULL REFERENCES organizations(id), case_id UUID NOT NULL,
    source TEXT NOT NULL CHECK (source = 'MANUAL'), observed_at TIMESTAMPTZ NOT NULL, currency TEXT NOT NULL CHECK (currency = 'KRW'),
    amount_krw BIGINT NOT NULL CHECK (amount_krw >= 0), recorded_by UUID NOT NULL, created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (id, org_id), FOREIGN KEY (case_id, org_id) REFERENCES facilities_cases(id, org_id), FOREIGN KEY (recorded_by, org_id) REFERENCES users(id, org_id)
);

DO $$ DECLARE t TEXT; tenant_tables TEXT[] := ARRAY['facilities_spaces','facilities_catalog_services','facilities_assets','facilities_obligations','facilities_cases','facilities_case_history','facilities_execution_evidence_links','facilities_acceptances','facilities_energy_observations','facilities_cost_observations']; BEGIN
 FOREACH t IN ARRAY tenant_tables LOOP
  EXECUTE format('ALTER TABLE %I ENABLE ROW LEVEL SECURITY', t); EXECUTE format('ALTER TABLE %I FORCE ROW LEVEL SECURITY', t);
  EXECUTE format('CREATE POLICY org_isolation ON %I USING (org_id = NULLIF(current_setting(''app.current_org'', true), '''')::uuid) WITH CHECK (org_id = NULLIF(current_setting(''app.current_org'', true), '''')::uuid)', t);
  EXECUTE format('CREATE TRIGGER trg_%I_org_immutable BEFORE UPDATE ON %I FOR EACH ROW EXECUTE FUNCTION enforce_org_id_immutable()', t, t);
 END LOOP;
END $$;
REVOKE DELETE, UPDATE ON facilities_case_history, facilities_execution_evidence_links, facilities_acceptances, facilities_energy_observations, facilities_cost_observations FROM console_rt;
GRANT SELECT, INSERT, UPDATE ON facilities_spaces, facilities_catalog_services, facilities_assets, facilities_obligations, facilities_cases TO console_rt;
GRANT SELECT, INSERT ON facilities_case_history, facilities_execution_evidence_links, facilities_acceptances, facilities_energy_observations, facilities_cost_observations TO console_rt;
