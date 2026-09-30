-- BE-LC slice 1: period locks, generic object versioning (0069 pattern
-- extracted), and the object-lifecycle engine MVP.
--
-- 1. period_locks — enforceable payroll/accounting freeze windows. A write that
--    stamps a date inside an active (unlocked_at IS NULL) lock must fail closed
--    via `console_platform_db::period_lock::assert_period_open`.
-- 2. registry_equipment_versions — first adoption of the generalized 0069
--    versioning shape (append-only versions + trigger protection +
--    rollback-as-new-version). The reusable SQL template lives in
--    `console_platform_db::versioning` module docs; a domain adopts it with one
--    migration exactly like this block.
-- 3. object_lifecycles / object_lifecycle_transitions / lifecycle_transition_rules
--    — generic per-object FSM keyed by (object_type, object_id) with an
--    append-only transition log, legal hold and retention gating.

-- ---------------------------------------------------------------------------
-- Shared append-only trigger (generic sibling of 0069's workflow-specific fn).
-- ---------------------------------------------------------------------------
CREATE OR REPLACE FUNCTION platform_append_only_immutable()
RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION 'append-only table % forbids %', TG_TABLE_NAME, TG_OP
        USING ERRCODE = '25006';
END;
$$;

-- ---------------------------------------------------------------------------
-- 1. Period locks (freeze windows).
-- ---------------------------------------------------------------------------
-- console-gate: audited-table period_locks
CREATE TABLE period_locks (
    id            UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id        UUID        NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    domain        TEXT        NOT NULL CHECK (domain IN ('payroll', 'accounting')),
    period_start  DATE        NOT NULL,
    period_end    DATE        NOT NULL,
    reason        TEXT        NOT NULL CHECK (btrim(reason) <> ''),
    locked_by     UUID        NULL REFERENCES users(id) ON DELETE SET NULL,
    locked_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    unlocked_by   UUID        NULL REFERENCES users(id) ON DELETE SET NULL,
    unlocked_at   TIMESTAMPTZ NULL,
    unlock_reason TEXT        NULL,
    UNIQUE (id, org_id),
    CONSTRAINT period_locks_valid_period CHECK (period_end >= period_start),
    CONSTRAINT period_locks_unlock_pair CHECK (
        (unlocked_at IS NULL AND unlock_reason IS NULL)
        OR (unlocked_at IS NOT NULL AND btrim(coalesce(unlock_reason, '')) <> '')
    )
);

CREATE INDEX idx_period_locks_active
    ON period_locks (org_id, domain, period_start, period_end)
    WHERE unlocked_at IS NULL;
CREATE INDEX idx_period_locks_org_domain
    ON period_locks (org_id, domain, locked_at DESC);

-- The ONLY legal UPDATE is the one-shot unlock: everything else is immutable
-- history (re-locking a period appends a new row).
CREATE OR REPLACE FUNCTION period_locks_unlock_only()
RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF OLD.unlocked_at IS NOT NULL THEN
        RAISE EXCEPTION 'period lock % is already unlocked and immutable', OLD.id
            USING ERRCODE = '25006';
    END IF;
    IF NEW.id IS DISTINCT FROM OLD.id
        OR NEW.org_id IS DISTINCT FROM OLD.org_id
        OR NEW.domain IS DISTINCT FROM OLD.domain
        OR NEW.period_start IS DISTINCT FROM OLD.period_start
        OR NEW.period_end IS DISTINCT FROM OLD.period_end
        OR NEW.reason IS DISTINCT FROM OLD.reason
        OR NEW.locked_by IS DISTINCT FROM OLD.locked_by
        OR NEW.locked_at IS DISTINCT FROM OLD.locked_at
        OR NEW.unlocked_at IS NULL
    THEN
        RAISE EXCEPTION 'period lock rows only permit the one-shot unlock update'
            USING ERRCODE = '25006';
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER trg_period_locks_unlock_only
    BEFORE UPDATE ON period_locks
    FOR EACH ROW EXECUTE FUNCTION period_locks_unlock_only();
CREATE TRIGGER trg_period_locks_no_delete
    BEFORE DELETE ON period_locks
    FOR EACH ROW EXECUTE FUNCTION platform_append_only_immutable();

ALTER TABLE period_locks ENABLE ROW LEVEL SECURITY;
ALTER TABLE period_locks FORCE ROW LEVEL SECURITY;
CREATE POLICY org_isolation ON period_locks
    USING (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid)
    WITH CHECK (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid);

GRANT SELECT, INSERT, UPDATE ON period_locks TO console_rt;

-- ---------------------------------------------------------------------------
-- 2. Generic versioning, first adoption: registry_equipment.
--    (Template: replace `registry_equipment` with your base table. Rust side:
--    `console_platform_db::versioning::ObjectVersions`.)
-- ---------------------------------------------------------------------------
-- console-gate: audited-table registry_equipment_versions
CREATE TABLE registry_equipment_versions (
    id             UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id         UUID        NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    object_id      UUID        NOT NULL REFERENCES registry_equipment(id) ON DELETE CASCADE,
    version        INTEGER     NOT NULL CHECK (version >= 1),
    status         TEXT        NOT NULL CHECK (status IN ('CAPTURED', 'ROLLBACK')),
    source_version INTEGER     NULL CHECK (source_version IS NULL OR source_version >= 1),
    content        JSONB       NOT NULL CHECK (jsonb_typeof(content) = 'object'),
    created_by     UUID        NULL REFERENCES users(id) ON DELETE SET NULL,
    created_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (org_id, object_id, version)
);

CREATE INDEX idx_registry_equipment_versions_object
    ON registry_equipment_versions (org_id, object_id, version DESC);

CREATE TRIGGER trg_registry_equipment_versions_no_update
    BEFORE UPDATE ON registry_equipment_versions
    FOR EACH ROW EXECUTE FUNCTION platform_append_only_immutable();
CREATE TRIGGER trg_registry_equipment_versions_no_delete
    BEFORE DELETE ON registry_equipment_versions
    FOR EACH ROW EXECUTE FUNCTION platform_append_only_immutable();

ALTER TABLE registry_equipment_versions ENABLE ROW LEVEL SECURITY;
ALTER TABLE registry_equipment_versions FORCE ROW LEVEL SECURITY;
CREATE POLICY org_isolation ON registry_equipment_versions
    USING (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid)
    WITH CHECK (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid);

GRANT SELECT, INSERT ON registry_equipment_versions TO console_rt;

-- ---------------------------------------------------------------------------
-- 3. Object lifecycle engine MVP.
-- ---------------------------------------------------------------------------
-- Global FSM rule seed: which (object_type, from, to) transitions are legal.
-- No tenant data — reference rows exactly like feature_catalog.
-- console-gate: global-table lifecycle_transition_rules (rationale: global FSM rule seed, no tenant data)
CREATE TABLE lifecycle_transition_rules (
    object_type TEXT NOT NULL CHECK (object_type ~ '^[a-z][a-z0-9_]{1,63}$'),
    from_state  TEXT NOT NULL CHECK (from_state ~ '^[a-z][a-z0-9_]{1,63}$'),
    to_state    TEXT NOT NULL CHECK (to_state ~ '^[a-z][a-z0-9_]{1,63}$'),
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (object_type, from_state, to_state)
);

REVOKE ALL ON lifecycle_transition_rules FROM PUBLIC;
GRANT SELECT ON lifecycle_transition_rules TO console_rt;

INSERT INTO lifecycle_transition_rules (object_type, from_state, to_state) VALUES
    ('document', 'draft',     'submitted'),
    ('document', 'submitted', 'approved'),
    ('document', 'approved',  'active'),
    ('document', 'active',    'revised'),
    ('document', 'revised',   'archived'),
    ('document', 'archived',  'disposed')
ON CONFLICT DO NOTHING;

-- console-gate: audited-table object_lifecycles
CREATE TABLE object_lifecycles (
    id              UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id          UUID        NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    object_type     TEXT        NOT NULL CHECK (object_type ~ '^[a-z][a-z0-9_]{1,63}$'),
    object_id       UUID        NOT NULL,
    current_state   TEXT        NOT NULL CHECK (current_state ~ '^[a-z][a-z0-9_]{1,63}$'),
    legal_hold      BOOLEAN     NOT NULL DEFAULT FALSE,
    retention_until DATE        NULL,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (id, org_id),
    UNIQUE (org_id, object_type, object_id)
);

CREATE INDEX idx_object_lifecycles_state
    ON object_lifecycles (org_id, object_type, current_state, updated_at DESC);

-- console-gate: audited-table object_lifecycle_transitions
CREATE TABLE object_lifecycle_transitions (
    id           UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id       UUID        NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    lifecycle_id UUID        NOT NULL,
    from_state   TEXT        NOT NULL,
    to_state     TEXT        NOT NULL,
    actor        UUID        NULL REFERENCES users(id) ON DELETE SET NULL,
    reason       TEXT        NOT NULL CHECK (btrim(reason) <> ''),
    occurred_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    FOREIGN KEY (lifecycle_id, org_id) REFERENCES object_lifecycles(id, org_id) ON DELETE CASCADE
);

CREATE INDEX idx_object_lifecycle_transitions_history
    ON object_lifecycle_transitions (org_id, lifecycle_id, occurred_at DESC);

CREATE TRIGGER trg_object_lifecycle_transitions_no_update
    BEFORE UPDATE ON object_lifecycle_transitions
    FOR EACH ROW EXECUTE FUNCTION platform_append_only_immutable();
CREATE TRIGGER trg_object_lifecycle_transitions_no_delete
    BEFORE DELETE ON object_lifecycle_transitions
    FOR EACH ROW EXECUTE FUNCTION platform_append_only_immutable();

ALTER TABLE object_lifecycles ENABLE ROW LEVEL SECURITY;
ALTER TABLE object_lifecycles FORCE ROW LEVEL SECURITY;
CREATE POLICY org_isolation ON object_lifecycles
    USING (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid)
    WITH CHECK (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid);

ALTER TABLE object_lifecycle_transitions ENABLE ROW LEVEL SECURITY;
ALTER TABLE object_lifecycle_transitions FORCE ROW LEVEL SECURITY;
CREATE POLICY org_isolation ON object_lifecycle_transitions
    USING (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid)
    WITH CHECK (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid);

GRANT SELECT, INSERT, UPDATE ON object_lifecycles TO console_rt;
GRANT SELECT, INSERT ON object_lifecycle_transitions TO console_rt;

CREATE TRIGGER trg_period_locks_org_immutable
    BEFORE UPDATE ON period_locks
    FOR EACH ROW EXECUTE FUNCTION enforce_org_id_immutable();
CREATE TRIGGER trg_object_lifecycles_org_immutable
    BEFORE UPDATE ON object_lifecycles
    FOR EACH ROW EXECUTE FUNCTION enforce_org_id_immutable();

-- New authority features for the console policy layer.
INSERT INTO feature_catalog (feature_key) VALUES
    ('period_lock_manage'),
    ('lifecycle_manage')
ON CONFLICT (feature_key) DO NOTHING;
