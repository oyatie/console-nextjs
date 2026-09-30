-- L-GOV: lifecycle + guardrails governance engine (arch §3 / §15 / §16).
--
-- Three tenant-owned tables under FORCE RLS org_isolation:
--   * gov_lifecycle_transitions — per-object-type instance-lifecycle FSM config
--     (which edges are legal + what each edge requires). Mutable config.
--   * gov_overrides            — append-only record of a post-draft edit override
--     (reason + before-snapshot). No hard delete, content immutable.
--   * gov_approvals            — append-only four-eyes decisions. approver_id
--     MUST differ from requested_by (self-approval blocked at the DB CHECK).
--
-- ponytail: object_type_id / target_id are plain UUIDs (logical refs to the
-- ontology registry, which is a separate lane and not built yet) — no FK, so
-- this lane stays independent of `crates/ontology/*`. Add the FK when the
-- registry migration lands.

INSERT INTO feature_catalog (feature_key) VALUES
    ('governance_lifecycle_manage'),
    ('governance_override_manage'),
    ('governance_approval_decide')
ON CONFLICT (feature_key) DO NOTHING;

-- Per-object-type instance-lifecycle FSM config. A configured edge is legal;
-- the requires_* flags feed the §16 guardrail gate chain for that transition.
CREATE TABLE gov_lifecycle_transitions (
    id                UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id            UUID        NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    object_type_id    UUID        NOT NULL,
    from_state        TEXT        NOT NULL CHECK (from_state IN ('DRAFT','ACTIVE','LOCKED','ARCHIVED','DISPOSED')),
    to_state          TEXT        NOT NULL CHECK (to_state   IN ('DRAFT','ACTIVE','LOCKED','ARCHIVED','DISPOSED')),
    requires_reason      BOOLEAN  NOT NULL DEFAULT false,
    requires_four_eyes   BOOLEAN  NOT NULL DEFAULT false,
    requires_checklist   BOOLEAN  NOT NULL DEFAULT false,
    created_by        UUID        NOT NULL,
    created_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK (from_state <> to_state),
    UNIQUE (id, org_id),
    UNIQUE (org_id, object_type_id, from_state, to_state),
    FOREIGN KEY (created_by, org_id) REFERENCES users(id, org_id) ON DELETE RESTRICT
);
CREATE INDEX idx_gov_lifecycle_transitions_type
    ON gov_lifecycle_transitions (org_id, object_type_id, from_state);

-- Append-only override record: editing a non-draft instance requires a reason
-- and a before-value snapshot. Effectiveness (four-eyes) is derived from the
-- gov_approvals decision keyed by request_ref = this row's id, so the override
-- content itself is fully immutable.
CREATE TABLE gov_overrides (
    id               UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id           UUID        NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    target_type      TEXT        NOT NULL CHECK (btrim(target_type) <> '' AND char_length(target_type) <= 80),
    target_id        UUID        NOT NULL,
    actor            UUID        NOT NULL,        -- requester of the override
    reason           TEXT        NOT NULL CHECK (btrim(reason) <> '' AND char_length(reason) <= 4000),
    before_snapshot  JSONB       NOT NULL CHECK (jsonb_typeof(before_snapshot) = 'object'),
    created_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (id, org_id),
    FOREIGN KEY (actor, org_id) REFERENCES users(id, org_id) ON DELETE RESTRICT
);
CREATE INDEX idx_gov_overrides_target
    ON gov_overrides (org_id, target_type, target_id, created_at DESC);

-- Append-only four-eyes decision. One final decision per request_ref. The DB
-- CHECK makes self-approval impossible regardless of application code.
CREATE TABLE gov_approvals (
    id               UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id           UUID        NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    request_ref      UUID        NOT NULL,        -- e.g. gov_overrides.id or an action-execute ref
    kind             TEXT        NOT NULL CHECK (btrim(kind) <> '' AND char_length(kind) <= 80),
    requested_by     UUID        NOT NULL,
    approver_id      UUID        NOT NULL,
    decision         TEXT        NOT NULL CHECK (decision IN ('pending','approved','rejected')),
    decided_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK (approver_id <> requested_by),          -- self-approval blocked
    UNIQUE (id, org_id),
    UNIQUE (org_id, request_ref),                 -- one decision per request
    FOREIGN KEY (requested_by, org_id) REFERENCES users(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (approver_id, org_id)  REFERENCES users(id, org_id) ON DELETE RESTRICT
);
CREATE INDEX idx_gov_approvals_request
    ON gov_approvals (org_id, request_ref);

CREATE OR REPLACE FUNCTION governance_append_only_record()
RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION 'append-only governance record % forbids %', TG_TABLE_NAME, TG_OP;
END;
$$;

-- FORCE RLS org_isolation + org-immutable on every table; append-only triggers
-- (UPDATE + DELETE both rejected) on the two record tables.
DO $$
DECLARE
    t TEXT;
    tenant_tables TEXT[] := ARRAY[
        'gov_lifecycle_transitions',
        'gov_overrides',
        'gov_approvals'
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

CREATE TRIGGER trg_gov_overrides_no_update
    BEFORE UPDATE ON gov_overrides
    FOR EACH ROW EXECUTE FUNCTION governance_append_only_record();
CREATE TRIGGER trg_gov_overrides_no_delete
    BEFORE DELETE ON gov_overrides
    FOR EACH ROW EXECUTE FUNCTION governance_append_only_record();
CREATE TRIGGER trg_gov_approvals_no_update
    BEFORE UPDATE ON gov_approvals
    FOR EACH ROW EXECUTE FUNCTION governance_append_only_record();
CREATE TRIGGER trg_gov_approvals_no_delete
    BEFORE DELETE ON gov_approvals
    FOR EACH ROW EXECUTE FUNCTION governance_append_only_record();

-- Runtime role: config table is mutable; record tables are append-only. No hard
-- DELETE anywhere in the engine.
GRANT SELECT, INSERT, UPDATE ON gov_lifecycle_transitions TO console_rt;
GRANT SELECT, INSERT         ON gov_overrides             TO console_rt;
GRANT SELECT, INSERT         ON gov_approvals             TO console_rt;

REVOKE DELETE         ON gov_lifecycle_transitions FROM console_rt;
REVOKE UPDATE, DELETE ON gov_overrides             FROM console_rt;
REVOKE UPDATE, DELETE ON gov_approvals             FROM console_rt;
