-- First-class HR employee directory. Employees are tenant data, not auth users.
CREATE TABLE employees (
    id              UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id          UUID        NOT NULL REFERENCES organizations(id),
    company         TEXT        NOT NULL CHECK (btrim(company) <> ''),
    name            TEXT        NOT NULL CHECK (btrim(name) <> ''),
    source_filename TEXT        NOT NULL CHECK (btrim(source_filename) <> ''),
    source_sheet    TEXT        NOT NULL CHECK (btrim(source_sheet) <> ''),
    source_row      INTEGER     NOT NULL CHECK (source_row > 0),
    source_key      TEXT        NOT NULL CHECK (btrim(source_key) <> ''),
    raw_row         JSONB       NOT NULL DEFAULT '{}'::jsonb,
    source_metadata JSONB       NOT NULL DEFAULT '{}'::jsonb,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (org_id, source_key)
);

CREATE INDEX employees_org_company_name_idx ON employees (org_id, company, name);

ALTER TABLE employees ENABLE ROW LEVEL SECURITY;
ALTER TABLE employees FORCE ROW LEVEL SECURITY;
CREATE POLICY org_isolation ON employees
    USING (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid)
    WITH CHECK (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid);

GRANT SELECT, INSERT, UPDATE, DELETE ON employees TO console_rt;
