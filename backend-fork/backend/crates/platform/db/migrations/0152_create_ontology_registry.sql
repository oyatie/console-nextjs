-- §18 Ontology registry (backbone) — object/property/link/action/analytic
-- definitions as config-as-governed-data.
--
-- Each object type is a VERSIONED, complete schema snapshot: one row per
-- (org, stable_key, schema_version), carrying its own lifecycle_state. The
-- children (property/link/action/analytic defs) belong to a specific object-type
-- VERSION row (object_type_id) and are append-only — a new revision is a new set
-- of child rows, never an edit of a published version's rows. A published
-- version's definitional content is therefore immutable and content-addressable
-- (free rollback + as-of schema); only its lifecycle_state advances along the
-- §3a ladder (draft → review_pending → published → superseded → retired).
--
-- All tables are tenant-scoped FORCE-RLS org-isolated. No hard delete anywhere
-- (§9.8): DELETE is revoked from the runtime role on every table.

-- console-gate: audited-table ont_object_types
CREATE TABLE ont_object_types (
    id                    UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id                UUID        NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    stable_key            TEXT        NOT NULL CHECK (stable_key ~ '^[a-z][a-z0-9_]*(\.[a-z][a-z0-9_]*)*$'),
    title                 TEXT        NOT NULL CHECK (char_length(title) BETWEEN 1 AND 120),
    title_property_key    TEXT        NULL CHECK (title_property_key IS NULL OR title_property_key ~ '^[a-z][a-z0-9_]*$'),
    backing_kind          TEXT        NOT NULL CHECK (backing_kind IN ('projected','instance')),
    backing_table         TEXT        NULL,   -- projected: allowlisted domain table name
    primary_key_property  TEXT        NULL,   -- projected: PK column
    schema_version        BIGINT      NOT NULL CHECK (schema_version >= 1),
    lifecycle_state       TEXT        NOT NULL CHECK (lifecycle_state IN ('draft','review_pending','published','superseded','retired')),
    created_by            UUID        NULL,
    created_at            TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at            TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (id, org_id),
    UNIQUE (org_id, stable_key, schema_version),
    -- projected types must name their backing table + PK; instance types must not.
    CHECK (
        (backing_kind = 'projected' AND backing_table IS NOT NULL AND primary_key_property IS NOT NULL)
        OR (backing_kind = 'instance' AND backing_table IS NULL AND primary_key_property IS NULL)
    ),
    FOREIGN KEY (created_by, org_id) REFERENCES users(id, org_id) ON DELETE RESTRICT
);
-- At most one live published version per key, and at most one in-flight draft.
CREATE UNIQUE INDEX idx_ont_object_types_one_published
    ON ont_object_types (org_id, stable_key) WHERE lifecycle_state = 'published';
CREATE UNIQUE INDEX idx_ont_object_types_one_draft
    ON ont_object_types (org_id, stable_key) WHERE lifecycle_state IN ('draft','review_pending');
CREATE INDEX idx_ont_object_types_list
    ON ont_object_types (org_id, backing_kind, lifecycle_state, updated_at DESC);

-- console-gate: audited-table ont_property_defs
CREATE TABLE ont_property_defs (
    id                 UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id             UUID        NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    object_type_id     UUID        NOT NULL,
    key                TEXT        NOT NULL CHECK (key ~ '^[a-z][a-z0-9_]*$'),
    title              TEXT        NOT NULL CHECK (char_length(title) BETWEEN 1 AND 120),
    type               TEXT        NOT NULL CHECK (char_length(type) BETWEEN 1 AND 64),  -- discriminated-union tag (§3c)
    config             JSONB       NOT NULL DEFAULT '{}'::jsonb CHECK (jsonb_typeof(config) = 'object'),
    backing_column     TEXT        NULL,      -- projected: source column
    required           BOOLEAN     NOT NULL DEFAULT false,
    in_property_policy  BOOLEAN     NOT NULL DEFAULT false,
    created_at         TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (object_type_id, key),
    FOREIGN KEY (object_type_id, org_id) REFERENCES ont_object_types(id, org_id) ON DELETE CASCADE
);
CREATE INDEX idx_ont_property_defs_owner ON ont_property_defs (org_id, object_type_id);

-- console-gate: audited-table ont_link_types
CREATE TABLE ont_link_types (
    id                    UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id                UUID        NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    object_type_id        UUID        NOT NULL,   -- the owning (from) object-type version snapshot
    stable_key            TEXT        NOT NULL CHECK (stable_key ~ '^[a-z][a-z0-9_]*$'),
    title                 TEXT        NOT NULL CHECK (char_length(title) BETWEEN 1 AND 120),
    -- Reverse (back-)link name; NULL = no reverse naming (design change-log 74:
    -- linkType is a 4-tuple [rel, toType, cardinality, rev]).
    reverse_title         TEXT        NULL CHECK (reverse_title IS NULL OR char_length(reverse_title) BETWEEN 1 AND 120),
    to_object_type_id     UUID        NULL,       -- target type (resolved at author time; NULL = unresolved)
    cardinality           TEXT        NOT NULL CHECK (cardinality IN ('one_one','one_many','many_many')),
    traversable           BOOLEAN     NOT NULL DEFAULT true,
    created_at            TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (object_type_id, stable_key),
    FOREIGN KEY (object_type_id, org_id) REFERENCES ont_object_types(id, org_id) ON DELETE CASCADE,
    FOREIGN KEY (to_object_type_id, org_id) REFERENCES ont_object_types(id, org_id) ON DELETE RESTRICT
);
CREATE INDEX idx_ont_link_types_owner ON ont_link_types (org_id, object_type_id);

-- console-gate: audited-table ont_action_types
CREATE TABLE ont_action_types (
    id                    UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id                UUID        NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    object_type_id        UUID        NOT NULL,
    stable_key            TEXT        NOT NULL CHECK (stable_key ~ '^[a-z][a-z0-9_]*$'),
    title                 TEXT        NOT NULL CHECK (char_length(title) BETWEEN 1 AND 120),
    params_schema         JSONB       NOT NULL DEFAULT '{}'::jsonb CHECK (jsonb_typeof(params_schema) = 'object'),
    edits                 JSONB       NOT NULL DEFAULT '[]'::jsonb CHECK (jsonb_typeof(edits) = 'array'),
    submission_criteria   JSONB       NOT NULL DEFAULT '[]'::jsonb CHECK (jsonb_typeof(submission_criteria) = 'array'),
    side_effects          JSONB       NOT NULL DEFAULT '[]'::jsonb CHECK (jsonb_typeof(side_effects) = 'array'),
    dispatch              TEXT        NOT NULL CHECK (dispatch IN ('projected_usecase','instance_revision')),
    dispatch_target       TEXT        NULL,       -- projected: which domain use-case
    control_points        JSONB       NOT NULL DEFAULT '[]'::jsonb CHECK (jsonb_typeof(control_points) = 'array'),
    created_at            TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (object_type_id, stable_key),
    FOREIGN KEY (object_type_id, org_id) REFERENCES ont_object_types(id, org_id) ON DELETE CASCADE
);
CREATE INDEX idx_ont_action_types_owner ON ont_action_types (org_id, object_type_id);

-- console-gate: audited-table ont_analytics
CREATE TABLE ont_analytics (
    id                 UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id             UUID        NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    object_type_id     UUID        NOT NULL,
    key                TEXT        NOT NULL CHECK (key ~ '^[a-z][a-z0-9_]*$'),
    title              TEXT        NOT NULL CHECK (char_length(title) BETWEEN 1 AND 120),
    formula            JSONB       NOT NULL DEFAULT '{}'::jsonb CHECK (jsonb_typeof(formula) = 'object'),
    result_type        JSONB       NOT NULL DEFAULT '{}'::jsonb CHECK (jsonb_typeof(result_type) = 'object'),
    created_at         TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (object_type_id, key),
    FOREIGN KEY (object_type_id, org_id) REFERENCES ont_object_types(id, org_id) ON DELETE CASCADE
);
CREATE INDEX idx_ont_analytics_owner ON ont_analytics (org_id, object_type_id);

-- FORCE-RLS org isolation on every registry table (copied from 0103).
DO $$
DECLARE
    t TEXT;
    tenant_tables TEXT[] := ARRAY[
        'ont_object_types',
        'ont_property_defs',
        'ont_link_types',
        'ont_action_types',
        'ont_analytics'
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

-- Only the object-type head advances state (UPDATE lifecycle_state), so guard its
-- org_id against rewrite. The child snapshot tables are append-only (below).
CREATE TRIGGER trg_ont_object_types_org_immutable BEFORE UPDATE ON ont_object_types
    FOR EACH ROW EXECUTE FUNCTION enforce_org_id_immutable();

-- Runtime-role grants. Production auto-grants these via ALTER DEFAULT PRIVILEGES
-- FOR ROLE console_app (0031); the #[sqlx::test] harness runs migrations as a
-- different superuser, so grant explicitly here (mirrors 0103).
--
-- Object-type head: SELECT + INSERT + UPDATE (lifecycle FSM), never DELETE.
GRANT SELECT, INSERT, UPDATE ON ont_object_types TO console_rt;
REVOKE DELETE ON ont_object_types FROM console_rt;
-- Definition children are append-only snapshots: SELECT + INSERT only. A new
-- revision appends a fresh child set; a published version's rows never change.
GRANT SELECT, INSERT ON ont_property_defs TO console_rt;
REVOKE UPDATE, DELETE ON ont_property_defs FROM console_rt;
GRANT SELECT, INSERT ON ont_link_types TO console_rt;
REVOKE UPDATE, DELETE ON ont_link_types FROM console_rt;
GRANT SELECT, INSERT ON ont_action_types TO console_rt;
REVOKE UPDATE, DELETE ON ont_action_types FROM console_rt;
GRANT SELECT, INSERT ON ont_analytics TO console_rt;
REVOKE UPDATE, DELETE ON ont_analytics FROM console_rt;
