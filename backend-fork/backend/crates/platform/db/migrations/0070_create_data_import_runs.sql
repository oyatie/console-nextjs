-- G005 Data Exchange: immutable raw import ledger with explicit dry-run/apply.
-- Specialized importers may keep compatibility endpoints, but production writes
-- should flow through a tenant-scoped run so raw source rows, mappings, dry-run
-- summaries, and apply evidence remain reviewable and auditable.

CREATE TABLE data_import_runs (
    id                UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id            UUID        NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    entity_type       TEXT        NOT NULL CHECK (entity_type IN ('employee_hr')),
    status            TEXT        NOT NULL CHECK (status IN ('PREVIEWED','DRY_RUN','APPLIED','FAILED')),
    source_filename   TEXT        NOT NULL CHECK (btrim(source_filename) <> ''),
    source_format     TEXT        NOT NULL CHECK (source_format IN ('xlsx','csv')),
    source_sha256     TEXT        NOT NULL CHECK (source_sha256 ~ '^[a-f0-9]{64}$'),
    mapping_profile   JSONB       NOT NULL DEFAULT '{}'::jsonb,
    dry_run_summary   JSONB       NOT NULL DEFAULT '{}'::jsonb,
    apply_summary     JSONB       NOT NULL DEFAULT '{}'::jsonb,
    input_rows        INTEGER     NOT NULL DEFAULT 0 CHECK (input_rows >= 0),
    candidate_rows    INTEGER     NOT NULL DEFAULT 0 CHECK (candidate_rows >= 0),
    preserved_rows    INTEGER     NOT NULL DEFAULT 0 CHECK (preserved_rows >= 0),
    created_by        UUID        NULL REFERENCES users(id) ON DELETE SET NULL,
    applied_by        UUID        NULL REFERENCES users(id) ON DELETE SET NULL,
    created_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    applied_at        TIMESTAMPTZ NULL,
    UNIQUE (id, org_id)
);

CREATE INDEX data_import_runs_org_created_idx
    ON data_import_runs (org_id, created_at DESC);
CREATE INDEX data_import_runs_org_status_idx
    ON data_import_runs (org_id, status, created_at DESC);

CREATE TABLE data_import_rows (
    id              UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id          UUID        NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    run_id          UUID        NOT NULL,
    source_sheet    TEXT        NOT NULL CHECK (btrim(source_sheet) <> ''),
    source_row      INTEGER     NOT NULL CHECK (source_row > 0),
    source_key      TEXT        NOT NULL CHECK (btrim(source_key) <> ''),
    row_status      TEXT        NOT NULL CHECK (row_status IN ('CANDIDATE','PRESERVED','ERROR')),
    raw_row         JSONB       NOT NULL DEFAULT '{}'::jsonb,
    canonical_row   JSONB       NOT NULL DEFAULT '{}'::jsonb,
    validation      JSONB       NOT NULL DEFAULT '{}'::jsonb,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (run_id, source_key),
    FOREIGN KEY (run_id, org_id) REFERENCES data_import_runs(id, org_id) ON DELETE CASCADE
);

CREATE INDEX data_import_rows_run_idx
    ON data_import_rows (org_id, run_id, source_sheet, source_row);
CREATE INDEX data_import_rows_status_idx
    ON data_import_rows (org_id, run_id, row_status);

ALTER TABLE data_import_runs ENABLE ROW LEVEL SECURITY;
ALTER TABLE data_import_runs FORCE ROW LEVEL SECURITY;
CREATE POLICY org_isolation ON data_import_runs
    USING (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid)
    WITH CHECK (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid);

ALTER TABLE data_import_rows ENABLE ROW LEVEL SECURITY;
ALTER TABLE data_import_rows FORCE ROW LEVEL SECURITY;
CREATE POLICY org_isolation ON data_import_rows
    USING (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid)
    WITH CHECK (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid);

CREATE OR REPLACE FUNCTION data_import_rows_append_only()
RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION
        'data_import_rows is append-only: % is forbidden (row id=%)',
        TG_OP, OLD.id;
END;
$$;

CREATE TRIGGER trg_data_import_rows_no_update
    BEFORE UPDATE ON data_import_rows
    FOR EACH ROW EXECUTE FUNCTION data_import_rows_append_only();

CREATE TRIGGER trg_data_import_rows_no_delete
    BEFORE DELETE ON data_import_rows
    FOR EACH ROW EXECUTE FUNCTION data_import_rows_append_only();

GRANT SELECT, INSERT, UPDATE ON data_import_runs TO console_rt;
GRANT SELECT, INSERT ON data_import_rows TO console_rt;
