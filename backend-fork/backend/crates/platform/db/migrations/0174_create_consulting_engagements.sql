-- Consulting engagement pilot: an append-only, tenant-scoped evidence-to-benefit chain.
INSERT INTO feature_catalog (feature_key) VALUES ('consulting_read'), ('consulting_manage') ON CONFLICT DO NOTHING;

-- External systems have no common FK shape. Bind each reviewed source once to
-- this tenant-owned control plane, then reference the binding rather than an
-- unchecked caller UUID.
CREATE TABLE consulting_reference_bindings (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(), org_id UUID NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    source_kind TEXT NOT NULL CHECK (source_kind IN ('DOCUMENT','EVIDENCE','KPI_DEFINITION','ONTOLOGY_INSTANCE')),
    source_id UUID NOT NULL, source_version TEXT NOT NULL CHECK (btrim(source_version) <> ''),
    evaluated_at TIMESTAMPTZ NOT NULL DEFAULT now(), evaluated_by UUID NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    UNIQUE (id, org_id), UNIQUE (org_id, source_kind, source_id, source_version)
);

CREATE TABLE consulting_engagements (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(), org_id UUID NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    customer_id UUID NOT NULL, customer_document_id UUID NULL, ontology_instance_id UUID NULL,
    title TEXT NOT NULL CHECK (btrim(title) <> '' AND char_length(title) <= 180),
    status TEXT NOT NULL DEFAULT 'DRAFT' CHECK (status IN ('DRAFT','PROPOSED','APPROVED','IMPLEMENTED','MEASURED','SUSTAINED','CORRECTIVE')),
    approval_id UUID NULL, workflow_execution_id UUID NULL, version BIGINT NOT NULL DEFAULT 1 CHECK (version > 0),
    idempotency_key TEXT NOT NULL CHECK (btrim(idempotency_key) <> '' AND char_length(idempotency_key) <= 128),
    idempotency_request_hash TEXT NOT NULL CHECK (idempotency_request_hash ~ '^[0-9a-f]{64}$'),
    idempotency_response_status SMALLINT NOT NULL DEFAULT 201 CHECK (idempotency_response_status BETWEEN 200 AND 299),
    idempotency_response JSONB NOT NULL DEFAULT '{}'::jsonb CHECK (jsonb_typeof(idempotency_response) = 'object'),
    created_by UUID NOT NULL REFERENCES users(id) ON DELETE RESTRICT, created_at TIMESTAMPTZ NOT NULL DEFAULT now(), updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (org_id, idempotency_key), UNIQUE (id, org_id),
    FOREIGN KEY (customer_id, org_id) REFERENCES registry_customers(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (customer_document_id, org_id) REFERENCES consulting_reference_bindings(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (ontology_instance_id, org_id) REFERENCES consulting_reference_bindings(id, org_id) ON DELETE RESTRICT
);
CREATE TABLE consulting_diagnostics (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(), org_id UUID NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    engagement_id UUID NOT NULL, summary TEXT NOT NULL CHECK (btrim(summary) <> ''), document_id UUID NULL, created_by UUID NOT NULL REFERENCES users(id) ON DELETE RESTRICT, created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (id, org_id), FOREIGN KEY (engagement_id, org_id) REFERENCES consulting_engagements(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (document_id, org_id) REFERENCES consulting_reference_bindings(id, org_id) ON DELETE RESTRICT
);
CREATE TABLE consulting_findings (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(), org_id UUID NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    engagement_id UUID NOT NULL, diagnostic_id UUID NOT NULL, statement TEXT NOT NULL CHECK (btrim(statement) <> ''), evidence_id UUID NOT NULL, document_id UUID NULL, created_by UUID NOT NULL REFERENCES users(id) ON DELETE RESTRICT, created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (id, org_id), FOREIGN KEY (engagement_id, org_id) REFERENCES consulting_engagements(id, org_id) ON DELETE RESTRICT, FOREIGN KEY (diagnostic_id, org_id) REFERENCES consulting_diagnostics(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (evidence_id, org_id) REFERENCES consulting_reference_bindings(id, org_id) ON DELETE RESTRICT, FOREIGN KEY (document_id, org_id) REFERENCES consulting_reference_bindings(id, org_id) ON DELETE RESTRICT
);
CREATE TABLE consulting_initiatives (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(), org_id UUID NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    engagement_id UUID NOT NULL, finding_id UUID NOT NULL, title TEXT NOT NULL CHECK (btrim(title) <> ''), hypothesis TEXT NOT NULL CHECK (btrim(hypothesis) <> ''), kpi_definition_id UUID NOT NULL, target_direction TEXT NOT NULL CHECK (target_direction IN ('INCREASE','DECREASE')), created_by UUID NOT NULL REFERENCES users(id) ON DELETE RESTRICT, created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (id, org_id), FOREIGN KEY (engagement_id, org_id) REFERENCES consulting_engagements(id, org_id) ON DELETE RESTRICT, FOREIGN KEY (finding_id, org_id) REFERENCES consulting_findings(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (kpi_definition_id, org_id) REFERENCES consulting_reference_bindings(id, org_id) ON DELETE RESTRICT
);
CREATE TABLE consulting_benefit_observations (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(), org_id UUID NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    engagement_id UUID NOT NULL, initiative_id UUID NOT NULL, kpi_definition_id UUID NOT NULL, evidence_id UUID NOT NULL, observed_at TIMESTAMPTZ NOT NULL, note TEXT NOT NULL CHECK (btrim(note) <> ''), created_by UUID NOT NULL REFERENCES users(id) ON DELETE RESTRICT, created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (id, org_id), FOREIGN KEY (engagement_id, org_id) REFERENCES consulting_engagements(id, org_id) ON DELETE RESTRICT, FOREIGN KEY (initiative_id, org_id) REFERENCES consulting_initiatives(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (kpi_definition_id, org_id) REFERENCES consulting_reference_bindings(id, org_id) ON DELETE RESTRICT, FOREIGN KEY (evidence_id, org_id) REFERENCES consulting_reference_bindings(id, org_id) ON DELETE RESTRICT
);
CREATE TABLE consulting_engagement_history (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(), org_id UUID NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    engagement_id UUID NOT NULL, actor_id UUID NOT NULL REFERENCES users(id) ON DELETE RESTRICT, event_type TEXT NOT NULL, from_status TEXT NULL, to_status TEXT NULL, version BIGINT NOT NULL, payload JSONB NOT NULL DEFAULT '{}'::jsonb CHECK (jsonb_typeof(payload) = 'object'), occurred_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    FOREIGN KEY (engagement_id, org_id) REFERENCES consulting_engagements(id, org_id) ON DELETE RESTRICT
);
CREATE INDEX idx_consulting_engagements_org_updated ON consulting_engagements(org_id, updated_at DESC, id);
CREATE INDEX idx_consulting_history_engagement ON consulting_engagement_history(org_id, engagement_id, occurred_at);

-- A tenant match alone is not enough: each control-plane reference has a
-- distinct meaning. Keep that invariant in the database so every writer,
-- including future non-REST writers, fails closed.
CREATE OR REPLACE FUNCTION consulting_require_reference_kinds()
RETURNS TRIGGER LANGUAGE plpgsql AS $$
DECLARE
  pair_index INTEGER;
  binding_id UUID;
  actual_kind TEXT;
BEGIN
  FOR pair_index IN 0..(array_length(TG_ARGV, 1) / 2 - 1) LOOP
    binding_id := NULLIF(to_jsonb(NEW)->>TG_ARGV[pair_index * 2], '')::uuid;
    IF binding_id IS NULL THEN CONTINUE; END IF;
    SELECT source_kind INTO actual_kind FROM consulting_reference_bindings WHERE id = binding_id AND org_id = NEW.org_id;
    IF actual_kind IS DISTINCT FROM TG_ARGV[pair_index * 2 + 1] THEN
      RAISE EXCEPTION 'consulting reference % must be a % binding', TG_ARGV[pair_index * 2], TG_ARGV[pair_index * 2 + 1];
    END IF;
  END LOOP;
  RETURN NEW;
END;
$$;
CREATE TRIGGER trg_consulting_engagement_reference_kinds BEFORE INSERT OR UPDATE ON consulting_engagements
  FOR EACH ROW EXECUTE FUNCTION consulting_require_reference_kinds('customer_document_id', 'DOCUMENT', 'ontology_instance_id', 'ONTOLOGY_INSTANCE');
CREATE TRIGGER trg_consulting_diagnostic_reference_kinds BEFORE INSERT OR UPDATE ON consulting_diagnostics
  FOR EACH ROW EXECUTE FUNCTION consulting_require_reference_kinds('document_id', 'DOCUMENT');
CREATE TRIGGER trg_consulting_finding_reference_kinds BEFORE INSERT OR UPDATE ON consulting_findings
  FOR EACH ROW EXECUTE FUNCTION consulting_require_reference_kinds('evidence_id', 'EVIDENCE', 'document_id', 'DOCUMENT');
CREATE TRIGGER trg_consulting_initiative_reference_kinds BEFORE INSERT OR UPDATE ON consulting_initiatives
  FOR EACH ROW EXECUTE FUNCTION consulting_require_reference_kinds('kpi_definition_id', 'KPI_DEFINITION');
CREATE TRIGGER trg_consulting_observation_reference_kinds BEFORE INSERT OR UPDATE ON consulting_benefit_observations
  FOR EACH ROW EXECUTE FUNCTION consulting_require_reference_kinds('kpi_definition_id', 'KPI_DEFINITION', 'evidence_id', 'EVIDENCE');
CREATE OR REPLACE FUNCTION consulting_reject_terminal_engagement_write()
RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
  IF OLD.status IN ('SUSTAINED','CORRECTIVE') THEN
    RAISE EXCEPTION USING
      ERRCODE = 'P0001',
      MESSAGE = 'terminal consulting engagement is immutable',
      CONSTRAINT = 'consulting_terminal_immutable';
  END IF;
  IF TG_OP = 'DELETE' THEN RETURN OLD; END IF;
  RETURN NEW;
END;
$$;
CREATE TRIGGER trg_consulting_engagement_terminal
  BEFORE UPDATE OR DELETE ON consulting_engagements
  FOR EACH ROW EXECUTE FUNCTION consulting_reject_terminal_engagement_write();

CREATE OR REPLACE FUNCTION consulting_reject_terminal_child_write()
RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
  IF TG_OP <> 'INSERT' AND EXISTS (
    SELECT 1 FROM consulting_engagements
    WHERE id=OLD.engagement_id AND org_id=OLD.org_id AND status IN ('SUSTAINED','CORRECTIVE')
  ) THEN
    RAISE EXCEPTION USING
      ERRCODE = 'P0001',
      MESSAGE = 'terminal consulting engagement is immutable',
      CONSTRAINT = 'consulting_terminal_immutable';
  END IF;
  IF TG_OP <> 'DELETE' AND EXISTS (
    SELECT 1 FROM consulting_engagements
    WHERE id=NEW.engagement_id AND org_id=NEW.org_id AND status IN ('SUSTAINED','CORRECTIVE')
  ) THEN
    RAISE EXCEPTION USING
      ERRCODE = 'P0001',
      MESSAGE = 'terminal consulting engagement is immutable',
      CONSTRAINT = 'consulting_terminal_immutable';
  END IF;
  IF TG_OP = 'DELETE' THEN RETURN OLD; END IF;
  RETURN NEW;
END;
$$;
CREATE TRIGGER trg_consulting_diagnostics_terminal BEFORE INSERT OR UPDATE OR DELETE ON consulting_diagnostics
  FOR EACH ROW EXECUTE FUNCTION consulting_reject_terminal_child_write();
CREATE TRIGGER trg_consulting_findings_terminal BEFORE INSERT OR UPDATE OR DELETE ON consulting_findings
  FOR EACH ROW EXECUTE FUNCTION consulting_reject_terminal_child_write();
CREATE TRIGGER trg_consulting_initiatives_terminal BEFORE INSERT OR UPDATE OR DELETE ON consulting_initiatives
  FOR EACH ROW EXECUTE FUNCTION consulting_reject_terminal_child_write();
CREATE TRIGGER trg_consulting_observations_terminal BEFORE INSERT OR UPDATE OR DELETE ON consulting_benefit_observations
  FOR EACH ROW EXECUTE FUNCTION consulting_reject_terminal_child_write();
DO $$ DECLARE t TEXT; BEGIN FOREACH t IN ARRAY ARRAY['consulting_reference_bindings','consulting_engagements','consulting_diagnostics','consulting_findings','consulting_initiatives','consulting_benefit_observations','consulting_engagement_history'] LOOP
  EXECUTE format('ALTER TABLE %I ENABLE ROW LEVEL SECURITY', t); EXECUTE format('ALTER TABLE %I FORCE ROW LEVEL SECURITY', t);
  EXECUTE format('CREATE POLICY org_isolation ON %I USING (org_id = NULLIF(current_setting(''app.current_org'', true), '''')::uuid) WITH CHECK (org_id = NULLIF(current_setting(''app.current_org'', true), '''')::uuid)', t);
  EXECUTE format('CREATE TRIGGER trg_%I_org_immutable BEFORE UPDATE ON %I FOR EACH ROW EXECUTE FUNCTION enforce_org_id_immutable()', t, t);
END LOOP; END $$;
CREATE TRIGGER trg_consulting_history_no_update BEFORE UPDATE ON consulting_engagement_history
    FOR EACH ROW EXECUTE FUNCTION governance_append_only_record();
CREATE TRIGGER trg_consulting_history_no_delete BEFORE DELETE ON consulting_engagement_history
    FOR EACH ROW EXECUTE FUNCTION governance_append_only_record();

GRANT SELECT ON consulting_reference_bindings TO console_rt;
REVOKE INSERT, UPDATE, DELETE ON consulting_reference_bindings FROM console_rt;
GRANT SELECT, INSERT, UPDATE ON consulting_engagements, consulting_diagnostics, consulting_findings, consulting_initiatives, consulting_benefit_observations TO console_rt;
GRANT SELECT, INSERT ON consulting_engagement_history TO console_rt;
REVOKE DELETE ON consulting_engagements, consulting_diagnostics, consulting_findings, consulting_initiatives, consulting_benefit_observations, consulting_engagement_history FROM console_rt;
REVOKE UPDATE ON consulting_engagement_history FROM console_rt;
