-- B21c: CP/RG/FW compliance module persistence.
--
-- Tenant-owned compliance objects live in the compliance clean-architecture stack.
-- Codes are server-issued per tenant and immutable; all business rows run under
-- FORCE RLS and runtime-role grants deliberately omit DELETE.

INSERT INTO feature_catalog (feature_key) VALUES
    ('compliance_domain_read'),
    ('compliance_domain_manage'),
    ('compliance_evidence_link')
ON CONFLICT (feature_key) DO NOTHING;

CREATE TABLE compliance_code_counters (
    org_id        UUID   NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    object_prefix TEXT   NOT NULL CHECK (object_prefix IN ('CP','RG','FW')),
    next_value    BIGINT NOT NULL DEFAULT 1 CHECK (next_value >= 1),
    updated_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (org_id, object_prefix)
);

-- console-gate: audited-table compliance_regulation_impacts
CREATE TABLE compliance_regulation_impacts (
    id              UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id          UUID        NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    code            TEXT        NOT NULL CHECK (code ~ '^RG-[0-9]{4,}$'),
    title           TEXT        NOT NULL CHECK (btrim(title) <> '' AND char_length(title) <= 200),
    jurisdiction    TEXT        NOT NULL CHECK (btrim(jurisdiction) <> '' AND char_length(jurisdiction) <= 80),
    regulator       TEXT        NULL CHECK (regulator IS NULL OR (btrim(regulator) <> '' AND char_length(regulator) <= 160)),
    citation        TEXT        NOT NULL CHECK (btrim(citation) <> '' AND char_length(citation) <= 300),
    source_url      TEXT        NULL CHECK (source_url IS NULL OR (btrim(source_url) <> '' AND char_length(source_url) <= 500)),
    impact_area     TEXT        NOT NULL CHECK (btrim(impact_area) <> '' AND char_length(impact_area) <= 80),
    impact_summary  TEXT        NOT NULL CHECK (btrim(impact_summary) <> '' AND char_length(impact_summary) <= 4000),
    risk_level      TEXT        NOT NULL CHECK (risk_level IN ('INFO','LOW','MEDIUM','HIGH','CRITICAL')),
    status          TEXT        NOT NULL DEFAULT 'DRAFT' CHECK (status IN ('DRAFT','ACTIVE','SUPERSEDED','ARCHIVED')),
    effective_from  DATE        NULL,
    effective_to    DATE        NULL CHECK (effective_to IS NULL OR effective_from IS NULL OR effective_to >= effective_from),
    review_due_on   DATE        NULL,
    owner_user_id   UUID        NULL,
    metadata        JSONB       NOT NULL DEFAULT '{}'::jsonb CHECK (jsonb_typeof(metadata) = 'object'),
    created_by      UUID        NOT NULL,
    updated_by      UUID        NOT NULL,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (id, org_id),
    UNIQUE (org_id, code),
    FOREIGN KEY (owner_user_id, org_id) REFERENCES users(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (created_by, org_id) REFERENCES users(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (updated_by, org_id) REFERENCES users(id, org_id) ON DELETE RESTRICT
);
CREATE INDEX idx_compliance_regulation_impacts_status
    ON compliance_regulation_impacts (org_id, status, risk_level, updated_at DESC);
CREATE INDEX idx_compliance_regulation_impacts_review_due
    ON compliance_regulation_impacts (org_id, review_due_on)
    WHERE review_due_on IS NOT NULL AND status = 'ACTIVE';

-- console-gate: audited-table compliance_obligations
CREATE TABLE compliance_obligations (
    id               UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id           UUID        NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    code             TEXT        NOT NULL CHECK (code ~ '^CP-[0-9]{4,}$'),
    title            TEXT        NOT NULL CHECK (btrim(title) <> '' AND char_length(title) <= 200),
    description      TEXT        NOT NULL CHECK (btrim(description) <> '' AND char_length(description) <= 4000),
    obligation_type  TEXT        NOT NULL CHECK (obligation_type IN ('LEGAL','REGULATORY','CONTRACTUAL','INTERNAL_POLICY','CONTROL_REQUIREMENT')),
    scope_type       TEXT        NOT NULL CHECK (scope_type IN ('ORG','BRANCH','SITE','TEAM','ROLE')),
    scope_ref        UUID        NULL,
    branch_id        UUID        NULL,
    site_id          UUID        NULL,
    owner_user_id    UUID        NULL,
    severity         TEXT        NOT NULL CHECK (severity IN ('INFO','LOW','MEDIUM','HIGH','CRITICAL')),
    status           TEXT        NOT NULL DEFAULT 'DRAFT' CHECK (status IN ('DRAFT','ACTIVE','WAIVED','SUPERSEDED','ARCHIVED')),
    effective_from   DATE        NULL,
    effective_to     DATE        NULL CHECK (effective_to IS NULL OR effective_from IS NULL OR effective_to >= effective_from),
    review_cadence   TEXT        NULL CHECK (review_cadence IS NULL OR review_cadence IN ('MONTHLY','QUARTERLY','SEMI_ANNUAL','ANNUAL','EVENT_DRIVEN')),
    next_review_on   DATE        NULL,
    metadata         JSONB       NOT NULL DEFAULT '{}'::jsonb CHECK (jsonb_typeof(metadata) = 'object'),
    created_by       UUID        NOT NULL,
    updated_by       UUID        NOT NULL,
    created_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (id, org_id),
    UNIQUE (org_id, code),
    FOREIGN KEY (branch_id, org_id) REFERENCES branches(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (site_id, org_id) REFERENCES registry_sites(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (owner_user_id, org_id) REFERENCES users(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (created_by, org_id) REFERENCES users(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (updated_by, org_id) REFERENCES users(id, org_id) ON DELETE RESTRICT,
    CHECK (
        (scope_type = 'ORG' AND scope_ref IS NULL AND branch_id IS NULL AND site_id IS NULL)
        OR (scope_type = 'BRANCH' AND branch_id IS NOT NULL AND scope_ref = branch_id AND site_id IS NULL)
        OR (scope_type = 'SITE' AND site_id IS NOT NULL AND branch_id IS NOT NULL AND scope_ref = site_id)
        OR (scope_type IN ('TEAM','ROLE') AND scope_ref IS NOT NULL)
    )
);
CREATE INDEX idx_compliance_obligations_status
    ON compliance_obligations (org_id, status, severity, next_review_on);
CREATE INDEX idx_compliance_obligations_branch
    ON compliance_obligations (org_id, branch_id, status) WHERE branch_id IS NOT NULL;
CREATE INDEX idx_compliance_obligations_site
    ON compliance_obligations (org_id, site_id, status) WHERE site_id IS NOT NULL;

-- console-gate: audited-table compliance_obligation_regulations
CREATE TABLE compliance_obligation_regulations (
    id                    UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id                UUID        NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    obligation_id         UUID        NOT NULL,
    regulation_impact_id  UUID        NOT NULL,
    relationship          TEXT        NOT NULL CHECK (relationship IN ('DERIVED_FROM','AMENDED_BY','SUPERSEDED_BY','INTERPRETS','EVIDENCES')),
    rationale             TEXT        NULL CHECK (rationale IS NULL OR char_length(rationale) <= 2000),
    created_by            UUID        NOT NULL,
    created_at            TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (id, org_id),
    UNIQUE (org_id, obligation_id, regulation_impact_id, relationship),
    FOREIGN KEY (obligation_id, org_id) REFERENCES compliance_obligations(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (regulation_impact_id, org_id) REFERENCES compliance_regulation_impacts(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (created_by, org_id) REFERENCES users(id, org_id) ON DELETE RESTRICT
);

-- console-gate: audited-table compliance_frameworks
CREATE TABLE compliance_frameworks (
    id              UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id          UUID        NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    code            TEXT        NOT NULL CHECK (code ~ '^FW-[0-9]{4,}$'),
    name            TEXT        NOT NULL CHECK (btrim(name) <> '' AND char_length(name) <= 200),
    version_label   TEXT        NOT NULL CHECK (btrim(version_label) <> '' AND char_length(version_label) <= 80),
    framework_kind  TEXT        NOT NULL CHECK (framework_kind IN ('LEGAL_BASELINE','INTERNAL_CONTROL','CUSTOMER_CONTROL','SECURITY_STANDARD','SAFETY_STANDARD','AUDIT_PROGRAM')),
    status          TEXT        NOT NULL DEFAULT 'DRAFT' CHECK (status IN ('DRAFT','ACTIVE','RETIRED','ARCHIVED')),
    owner_user_id   UUID        NULL,
    effective_from  DATE        NULL,
    effective_to    DATE        NULL CHECK (effective_to IS NULL OR effective_from IS NULL OR effective_to >= effective_from),
    metadata        JSONB       NOT NULL DEFAULT '{}'::jsonb CHECK (jsonb_typeof(metadata) = 'object'),
    created_by      UUID        NOT NULL,
    updated_by      UUID        NOT NULL,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (id, org_id),
    UNIQUE (org_id, code),
    UNIQUE (org_id, name, version_label),
    FOREIGN KEY (owner_user_id, org_id) REFERENCES users(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (created_by, org_id) REFERENCES users(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (updated_by, org_id) REFERENCES users(id, org_id) ON DELETE RESTRICT
);
CREATE INDEX idx_compliance_frameworks_status
    ON compliance_frameworks (org_id, status, updated_at DESC);

-- console-gate: audited-table compliance_controls
CREATE TABLE compliance_controls (
    id                     UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id                 UUID        NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    framework_id           UUID        NOT NULL,
    control_key            TEXT        NOT NULL CHECK (control_key ~ '^[A-Z0-9][A-Z0-9._-]{0,63}$'),
    title                  TEXT        NOT NULL CHECK (btrim(title) <> '' AND char_length(title) <= 200),
    objective              TEXT        NOT NULL CHECK (btrim(objective) <> '' AND char_length(objective) <= 4000),
    control_type           TEXT        NOT NULL CHECK (control_type IN ('PREVENTIVE','DETECTIVE','CORRECTIVE','DIRECTIVE','COMPENSATING')),
    cadence                TEXT        NULL CHECK (cadence IS NULL OR cadence IN ('CONTINUOUS','DAILY','WEEKLY','MONTHLY','QUARTERLY','ANNUAL','EVENT_DRIVEN')),
    status                 TEXT        NOT NULL DEFAULT 'DRAFT' CHECK (status IN ('DRAFT','ACTIVE','RETIRED','ARCHIVED')),
    evidence_requirements  JSONB       NOT NULL DEFAULT '[]'::jsonb CHECK (jsonb_typeof(evidence_requirements) = 'array'),
    owner_user_id          UUID        NULL,
    created_by             UUID        NOT NULL,
    updated_by             UUID        NOT NULL,
    created_at             TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at             TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (id, org_id),
    UNIQUE (org_id, framework_id, control_key),
    FOREIGN KEY (framework_id, org_id) REFERENCES compliance_frameworks(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (owner_user_id, org_id) REFERENCES users(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (created_by, org_id) REFERENCES users(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (updated_by, org_id) REFERENCES users(id, org_id) ON DELETE RESTRICT
);
CREATE INDEX idx_compliance_controls_framework_status
    ON compliance_controls (org_id, framework_id, status, control_key);

-- console-gate: audited-table compliance_control_obligations
CREATE TABLE compliance_control_obligations (
    id                   UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id               UUID        NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    control_id           UUID        NOT NULL,
    obligation_id        UUID        NOT NULL,
    coverage_level       TEXT        NOT NULL CHECK (coverage_level IN ('PRIMARY','PARTIAL','SUPPORTING','COMPENSATING')),
    coverage_rationale   TEXT        NULL CHECK (coverage_rationale IS NULL OR char_length(coverage_rationale) <= 2000),
    status               TEXT        NOT NULL DEFAULT 'ACTIVE' CHECK (status IN ('ACTIVE','RETIRED')),
    created_by           UUID        NOT NULL,
    updated_by           UUID        NOT NULL,
    created_at           TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at           TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (id, org_id),
    UNIQUE (org_id, control_id, obligation_id),
    FOREIGN KEY (control_id, org_id) REFERENCES compliance_controls(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (obligation_id, org_id) REFERENCES compliance_obligations(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (created_by, org_id) REFERENCES users(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (updated_by, org_id) REFERENCES users(id, org_id) ON DELETE RESTRICT
);
CREATE INDEX idx_compliance_control_obligations_obligation
    ON compliance_control_obligations (org_id, obligation_id, status);
CREATE INDEX idx_compliance_control_obligations_control
    ON compliance_control_obligations (org_id, control_id, status);

-- console-gate: audited-table compliance_evidence_bindings
CREATE TABLE compliance_evidence_bindings (
    id                    UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id                UUID        NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    control_id            UUID        NOT NULL,
    obligation_id         UUID        NULL,
    evidence_target_type  TEXT        NOT NULL CHECK (evidence_target_type IN ('audit_event','evidence_media','workflow_run','workflow_task','object_link','governance_finding','external_document','future_ev_object')),
    evidence_target_id    TEXT        NOT NULL CHECK (btrim(evidence_target_id) <> '' AND char_length(evidence_target_id) <= 200),
    source_audit_event_id UUID        NULL,
    status                TEXT        NOT NULL DEFAULT 'PROPOSED' CHECK (status IN ('PROPOSED','ACCEPTED','REJECTED','EXPIRED','RETRACTED')),
    confidence            TEXT        NOT NULL DEFAULT 'MEDIUM' CHECK (confidence IN ('LOW','MEDIUM','HIGH','SYSTEM')),
    collected_at          TIMESTAMPTZ NULL,
    collected_by          UUID        NULL,
    valid_from            DATE        NULL,
    valid_to              DATE        NULL CHECK (valid_to IS NULL OR valid_from IS NULL OR valid_to >= valid_from),
    hash_sha256           TEXT        NULL CHECK (hash_sha256 IS NULL OR hash_sha256 ~ '^[A-Fa-f0-9]{64}$'),
    metadata              JSONB       NOT NULL DEFAULT '{}'::jsonb CHECK (jsonb_typeof(metadata) = 'object'),
    created_by            UUID        NOT NULL,
    updated_by            UUID        NOT NULL,
    created_at            TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at            TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (id, org_id),
    FOREIGN KEY (control_id, org_id) REFERENCES compliance_controls(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (obligation_id, org_id) REFERENCES compliance_obligations(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (org_id, source_audit_event_id) REFERENCES audit_events(org_id, id) ON DELETE RESTRICT,
    FOREIGN KEY (collected_by, org_id) REFERENCES users(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (created_by, org_id) REFERENCES users(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (updated_by, org_id) REFERENCES users(id, org_id) ON DELETE RESTRICT
);
CREATE UNIQUE INDEX idx_compliance_evidence_bindings_unique_with_obligation
    ON compliance_evidence_bindings (org_id, control_id, obligation_id, evidence_target_type, evidence_target_id)
    WHERE obligation_id IS NOT NULL;
CREATE UNIQUE INDEX idx_compliance_evidence_bindings_unique_no_obligation
    ON compliance_evidence_bindings (org_id, control_id, evidence_target_type, evidence_target_id)
    WHERE obligation_id IS NULL;
CREATE INDEX idx_compliance_evidence_bindings_control
    ON compliance_evidence_bindings (org_id, control_id, status);
CREATE INDEX idx_compliance_evidence_bindings_target
    ON compliance_evidence_bindings (org_id, evidence_target_type, evidence_target_id);

CREATE OR REPLACE FUNCTION enforce_compliance_code_immutable()
RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.code IS DISTINCT FROM OLD.code THEN
        RAISE EXCEPTION 'compliance code is immutable for table % id=%', TG_TABLE_NAME, OLD.id;
    END IF;
    RETURN NEW;
END;
$$;

CREATE OR REPLACE FUNCTION compliance_append_only_relation()
RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION 'append-only compliance relation % forbids %', TG_TABLE_NAME, TG_OP;
END;
$$;

DO $$
DECLARE
    t TEXT;
    tenant_tables TEXT[] := ARRAY[
        'compliance_code_counters',
        'compliance_regulation_impacts',
        'compliance_obligations',
        'compliance_obligation_regulations',
        'compliance_frameworks',
        'compliance_controls',
        'compliance_control_obligations',
        'compliance_evidence_bindings'
    ];
BEGIN
    FOREACH t IN ARRAY tenant_tables LOOP
        EXECUTE format('ALTER TABLE %I ENABLE ROW LEVEL SECURITY', t);
        EXECUTE format('ALTER TABLE %I FORCE ROW LEVEL SECURITY', t);
        EXECUTE format(
            'CREATE POLICY org_isolation ON %I '
            || 'USING (org_id = NULLIF(current_setting(''app.current_org'', true), '''')::uuid) '
            || 'WITH CHECK (org_id = NULLIF(current_setting(''app.current_org'', true), '''')::uuid)',
            t
        );
        EXECUTE format(
            'CREATE TRIGGER trg_%I_org_immutable BEFORE UPDATE ON %I '
            || 'FOR EACH ROW EXECUTE FUNCTION enforce_org_id_immutable()',
            t, t
        );
    END LOOP;
END
$$;

CREATE TRIGGER trg_compliance_regulation_impacts_code_immutable
    BEFORE UPDATE ON compliance_regulation_impacts
    FOR EACH ROW EXECUTE FUNCTION enforce_compliance_code_immutable();
CREATE TRIGGER trg_compliance_obligations_code_immutable
    BEFORE UPDATE ON compliance_obligations
    FOR EACH ROW EXECUTE FUNCTION enforce_compliance_code_immutable();
CREATE TRIGGER trg_compliance_frameworks_code_immutable
    BEFORE UPDATE ON compliance_frameworks
    FOR EACH ROW EXECUTE FUNCTION enforce_compliance_code_immutable();

CREATE TRIGGER trg_compliance_obligation_regulations_no_update
    BEFORE UPDATE ON compliance_obligation_regulations
    FOR EACH ROW EXECUTE FUNCTION compliance_append_only_relation();
CREATE TRIGGER trg_compliance_obligation_regulations_no_delete
    BEFORE DELETE ON compliance_obligation_regulations
    FOR EACH ROW EXECUTE FUNCTION compliance_append_only_relation();

GRANT SELECT, INSERT, UPDATE ON compliance_code_counters TO console_rt;
GRANT SELECT, INSERT, UPDATE ON compliance_regulation_impacts TO console_rt;
GRANT SELECT, INSERT, UPDATE ON compliance_obligations TO console_rt;
GRANT SELECT, INSERT ON compliance_obligation_regulations TO console_rt;
GRANT SELECT, INSERT, UPDATE ON compliance_frameworks TO console_rt;
GRANT SELECT, INSERT, UPDATE ON compliance_controls TO console_rt;
GRANT SELECT, INSERT, UPDATE ON compliance_control_obligations TO console_rt;
GRANT SELECT, INSERT, UPDATE ON compliance_evidence_bindings TO console_rt;

REVOKE DELETE ON compliance_code_counters FROM console_rt;
REVOKE DELETE ON compliance_regulation_impacts FROM console_rt;
REVOKE DELETE ON compliance_obligations FROM console_rt;
REVOKE UPDATE, DELETE ON compliance_obligation_regulations FROM console_rt;
REVOKE DELETE ON compliance_frameworks FROM console_rt;
REVOKE DELETE ON compliance_controls FROM console_rt;
REVOKE DELETE ON compliance_control_obligations FROM console_rt;
REVOKE DELETE ON compliance_evidence_bindings FROM console_rt;
