-- B16 Cedar policy catalog and no-code draft staging substrate.
--
-- These tables are tenant-scoped, FORCE-RLS protected staging/read-model storage.
-- Draft saves are reviewable artifacts only: they do not bump legacy
-- policy_versions and cannot create live/shadow enforcement rows.

-- console-gate: audited-table cedar_policy_catalog_entries
CREATE TABLE cedar_policy_catalog_entries (
    id                       UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id                   UUID        NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    stable_key               TEXT        NOT NULL CHECK (stable_key ~ '^[a-z0-9_]+(\.[a-z0-9_]+)+$'),
    title                    TEXT        NOT NULL CHECK (char_length(title) BETWEEN 1 AND 120),
    natural_language_rule    TEXT        NOT NULL CHECK (char_length(natural_language_rule) BETWEEN 1 AND 1000),
    effect                   TEXT        NOT NULL CHECK (effect IN ('permit','forbid')),
    status                   TEXT        NOT NULL CHECK (status IN ('enforced','shadow','draft','review_pending','rejected','retired')),
    source                   TEXT        NOT NULL CHECK (source IN ('system_generated','no_code_draft','promoted_policy','imported_fixture')),
    principal                JSONB       NOT NULL CHECK (jsonb_typeof(principal) = 'object'),
    action                   JSONB       NOT NULL CHECK (jsonb_typeof(action) = 'object'),
    resource                 JSONB       NOT NULL CHECK (jsonb_typeof(resource) = 'object'),
    conditions               JSONB       NOT NULL DEFAULT '[]'::jsonb CHECK (jsonb_typeof(conditions) = 'array'),
    engine_mode              TEXT        NULL CHECK (
        engine_mode IS NULL OR engine_mode IN (
            'legacy_only',
            'cedar_shadow_legacy_enforce',
            'cedar_enforce_legacy_compare',
            'cedar_only'
        )
    ),
    policy_version           BIGINT      NULL CHECK (policy_version IS NULL OR policy_version > 0),
    schema_version           TEXT        NULL,
    bundle_digest            TEXT        NULL CHECK (bundle_digest IS NULL OR bundle_digest ~ '^sha256:[a-f0-9]{64}$'),
    cedar_sdk_version        TEXT        NULL,
    cedar_language_version   TEXT        NULL,
    validation_status        TEXT        NOT NULL CHECK (validation_status IN ('valid','invalid')),
    created_by               UUID        NULL,
    updated_by               UUID        NULL,
    created_at               TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at               TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (id, org_id),
    UNIQUE (org_id, stable_key, status),
    FOREIGN KEY (created_by, org_id) REFERENCES users(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (updated_by, org_id) REFERENCES users(id, org_id) ON DELETE RESTRICT,
    CHECK (
        status NOT IN ('enforced','shadow')
        OR (policy_version IS NOT NULL AND schema_version IS NOT NULL AND bundle_digest IS NOT NULL)
    )
);

-- console-gate: audited-table cedar_policy_drafts
CREATE TABLE cedar_policy_drafts (
    id                       UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id                   UUID        NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    draft_key                TEXT        NOT NULL CHECK (draft_key ~ '^[a-z0-9_]+(\.[a-z0-9_]+)+$'),
    title                    TEXT        NOT NULL CHECK (char_length(title) BETWEEN 1 AND 120),
    author_note              TEXT        NULL CHECK (author_note IS NULL OR char_length(author_note) <= 512),
    blocks                   JSONB       NOT NULL CHECK (jsonb_typeof(blocks) = 'object'),
    normalized_row           JSONB       NOT NULL CHECK (jsonb_typeof(normalized_row) = 'object'),
    generated_policy_text    TEXT        NOT NULL CHECK (char_length(generated_policy_text) > 0),
    generated_policy_digest  TEXT        NOT NULL CHECK (generated_policy_digest ~ '^sha256:[a-f0-9]{64}$'),
    validation_status        TEXT        NOT NULL CHECK (validation_status IN ('valid','invalid')),
    validation_errors        JSONB       NOT NULL DEFAULT '[]'::jsonb CHECK (jsonb_typeof(validation_errors) = 'array'),
    review_status            TEXT        NOT NULL CHECK (review_status IN ('draft','review_pending','rejected','approved_for_promotion')),
    reviewer_id              UUID        NULL,
    review_note              TEXT        NULL CHECK (review_note IS NULL OR char_length(review_note) <= 1000),
    created_by               UUID        NOT NULL,
    updated_by               UUID        NOT NULL,
    created_at               TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at               TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (id, org_id),
    UNIQUE (org_id, draft_key),
    FOREIGN KEY (created_by, org_id) REFERENCES users(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (updated_by, org_id) REFERENCES users(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (reviewer_id, org_id) REFERENCES users(id, org_id) ON DELETE RESTRICT,
    CHECK (review_status <> 'review_pending' OR validation_status = 'valid'),
    CHECK ((normalized_row->>'status') IS NULL OR normalized_row->>'status' NOT IN ('enforced','shadow')),
    CHECK ((normalized_row->>'policy_version') IS NULL),
    CHECK ((normalized_row->>'bundle_digest') IS NULL)
);

CREATE INDEX idx_cedar_policy_catalog_entries_list
    ON cedar_policy_catalog_entries (org_id, status, source, updated_at DESC);
CREATE INDEX idx_cedar_policy_catalog_entries_action
    ON cedar_policy_catalog_entries (org_id, ((action->>'action_key')), ((resource->>'resource_type')));
CREATE INDEX idx_cedar_policy_drafts_list
    ON cedar_policy_drafts (org_id, review_status, updated_at DESC);
CREATE INDEX idx_cedar_policy_drafts_action
    ON cedar_policy_drafts (org_id, ((normalized_row->'action'->>'action_key')), ((normalized_row->'resource'->>'resource_type')));

DO $$
DECLARE
    t TEXT;
    tenant_tables TEXT[] := ARRAY[
        'cedar_policy_catalog_entries',
        'cedar_policy_drafts'
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

-- The runtime role may READ catalog entries. This slice has no promotion lane,
-- so it deliberately cannot write catalog rows with shadow/enforced behavior.
GRANT SELECT ON cedar_policy_catalog_entries TO console_rt;
REVOKE INSERT, UPDATE, DELETE ON cedar_policy_catalog_entries FROM console_rt;
GRANT SELECT, INSERT, UPDATE ON cedar_policy_drafts TO console_rt;
REVOKE DELETE ON cedar_policy_drafts FROM console_rt;
