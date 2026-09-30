-- Workflow Studio: tenant-owned no-code workflow definition catalog.
--
-- Definitions are the mutable pointer (name/status/latest/active version).
-- Versions and events are append-only: every publish/pause/rollback/clone
-- appends evidence instead of rewriting prior business logic. Runtime execution
-- can later bind object events to a specific (definition_id, version) pair.

-- console-gate: audited-table workflow_definitions
CREATE TABLE workflow_definitions (
    id             UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id         UUID        NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    workflow_key   TEXT        NOT NULL CHECK (workflow_key ~ '^[a-z][a-z0-9_]*(\.[a-z][a-z0-9_]*)+$'),
    display_name   TEXT        NOT NULL CHECK (char_length(display_name) BETWEEN 1 AND 120),
    object_type    TEXT        NOT NULL CHECK (object_type ~ '^[a-z][a-z0-9_]{1,63}$'),
    status         TEXT        NOT NULL DEFAULT 'DRAFT' CHECK (status IN ('DRAFT','ACTIVE','PAUSED','RETIRED')),
    latest_version INTEGER     NOT NULL DEFAULT 1 CHECK (latest_version >= 1),
    active_version INTEGER     NULL CHECK (active_version IS NULL OR active_version >= 1),
    created_by     UUID        NULL REFERENCES users(id) ON DELETE SET NULL,
    updated_by     UUID        NULL REFERENCES users(id) ON DELETE SET NULL,
    created_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (id, org_id),
    UNIQUE (org_id, workflow_key)
);
CREATE INDEX idx_workflow_definitions_org_status
    ON workflow_definitions (org_id, status, object_type, updated_at DESC);

-- console-gate: audited-table workflow_definition_versions
CREATE TABLE workflow_definition_versions (
    id                     UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id                 UUID        NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    definition_id          UUID        NOT NULL,
    version                INTEGER     NOT NULL CHECK (version >= 1),
    status                 TEXT        NOT NULL CHECK (status IN ('DRAFT','PUBLISHED','PAUSED','ROLLED_BACK','CLONED')),
    definition             JSONB       NOT NULL DEFAULT '{}'::jsonb CHECK (jsonb_typeof(definition) = 'object'),
    approval_line          JSONB       NOT NULL DEFAULT '[]'::jsonb CHECK (jsonb_typeof(approval_line) = 'array'),
    payment_line           JSONB       NOT NULL DEFAULT '[]'::jsonb CHECK (jsonb_typeof(payment_line) = 'array'),
    notification_rules     JSONB       NOT NULL DEFAULT '[]'::jsonb CHECK (jsonb_typeof(notification_rules) = 'array'),
    action_allowlist       JSONB       NOT NULL DEFAULT '[]'::jsonb CHECK (jsonb_typeof(action_allowlist) = 'array'),
    required_approval_line BOOLEAN     NOT NULL DEFAULT false,
    required_payment_line  BOOLEAN     NOT NULL DEFAULT false,
    created_by             UUID        NULL REFERENCES users(id) ON DELETE SET NULL,
    created_at             TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (definition_id, version),
    FOREIGN KEY (definition_id, org_id) REFERENCES workflow_definitions(id, org_id) ON DELETE CASCADE
);
CREATE INDEX idx_workflow_definition_versions_definition
    ON workflow_definition_versions (org_id, definition_id, version DESC);

-- console-gate: audited-table workflow_definition_events
CREATE TABLE workflow_definition_events (
    id              UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id          UUID        NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    definition_id   UUID        NOT NULL,
    version         INTEGER     NULL CHECK (version IS NULL OR version >= 1),
    action          TEXT        NOT NULL CHECK (action ~ '^[a-z][a-z0-9_]*(\.[a-z][a-z0-9_]*)+$'),
    actor_id        UUID        NULL REFERENCES users(id) ON DELETE SET NULL,
    summary         TEXT        NOT NULL CHECK (char_length(summary) BETWEEN 1 AND 512),
    before_snap     JSONB       NULL,
    after_snap      JSONB       NULL,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    FOREIGN KEY (definition_id, org_id) REFERENCES workflow_definitions(id, org_id) ON DELETE CASCADE
);
CREATE INDEX idx_workflow_definition_events_history
    ON workflow_definition_events (org_id, definition_id, created_at DESC);

CREATE OR REPLACE FUNCTION workflow_studio_append_only_immutable()
RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION 'workflow studio append-only table % forbids %', TG_TABLE_NAME, TG_OP
        USING ERRCODE = '25006';
END;
$$;

DO $$
DECLARE
    t TEXT;
    tenant_tables TEXT[] := ARRAY[
        'workflow_definitions',
        'workflow_definition_versions',
        'workflow_definition_events'
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
    END LOOP;
END
$$;

GRANT SELECT, INSERT, UPDATE, DELETE ON workflow_definitions TO console_rt;
GRANT SELECT, INSERT ON workflow_definition_versions TO console_rt;
GRANT SELECT, INSERT ON workflow_definition_events TO console_rt;

CREATE TRIGGER trg_workflow_definitions_org_immutable
    BEFORE UPDATE ON workflow_definitions
    FOR EACH ROW EXECUTE FUNCTION enforce_org_id_immutable();

CREATE TRIGGER trg_workflow_definition_versions_no_update
    BEFORE UPDATE ON workflow_definition_versions
    FOR EACH ROW EXECUTE FUNCTION workflow_studio_append_only_immutable();
CREATE TRIGGER trg_workflow_definition_versions_no_delete
    BEFORE DELETE ON workflow_definition_versions
    FOR EACH ROW EXECUTE FUNCTION workflow_studio_append_only_immutable();
CREATE TRIGGER trg_workflow_definition_versions_org_immutable
    BEFORE UPDATE ON workflow_definition_versions
    FOR EACH ROW EXECUTE FUNCTION enforce_org_id_immutable();

CREATE TRIGGER trg_workflow_definition_events_no_update
    BEFORE UPDATE ON workflow_definition_events
    FOR EACH ROW EXECUTE FUNCTION workflow_studio_append_only_immutable();
CREATE TRIGGER trg_workflow_definition_events_no_delete
    BEFORE DELETE ON workflow_definition_events
    FOR EACH ROW EXECUTE FUNCTION workflow_studio_append_only_immutable();
CREATE TRIGGER trg_workflow_definition_events_org_immutable
    BEFORE UPDATE ON workflow_definition_events
    FOR EACH ROW EXECUTE FUNCTION enforce_org_id_immutable();
