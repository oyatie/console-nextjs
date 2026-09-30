-- BE-OBJ slice 3, surface 2: SR- series domain.
--
-- The design's ontology lets a user "promote" a recurring instance-type object
-- into a SR- series (seriesCreate) and fold further instances in (seriesAttach)
-- — e.g. 임대료 회차, 급여 회차, 정비 이력. A series is an org-scoped object
-- with a canonical SR- code (the FIRST real adopter of the shared issue_code
-- helper, migration 0113); membership is a join against generic (kind, id)
-- object references, so ANY resolvable object kind can be a series member.
--
-- Both tables are tenant data: org_id + FORCE RLS keyed on app.current_org,
-- like every other tenant table.

-- Register the `series` kind so issue_code can mint SR- codes and so a series
-- can itself be a node/link endpoint in the graph. status defaults to active
-- (column added in 0121, which runs before this).
INSERT INTO object_types (kind, description, code_prefix) VALUES
    ('series', 'A user-authored series grouping recurring instances', 'SR-')
ON CONFLICT (kind) DO NOTHING;

-- ---------------------------------------------------------------------------
-- series — the series object (tenant data).
-- ---------------------------------------------------------------------------
-- console-gate: audited-table series
CREATE TABLE series (
    id          UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id      UUID        NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    -- Canonical SR- code issued from object_code_counters (unique per org).
    code        TEXT        NOT NULL CHECK (char_length(btrim(code)) BETWEEN 1 AND 64),
    label       TEXT        NOT NULL CHECK (char_length(btrim(label)) BETWEEN 1 AND 200),
    created_by  UUID        REFERENCES users(id) ON DELETE RESTRICT,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (org_id, code),
    -- Supports the series_instances composite FK so a membership row cannot
    -- point at a series in another tenant while carrying its own org_id.
    CONSTRAINT series_id_org_uk UNIQUE (id, org_id)
);

ALTER TABLE series ENABLE ROW LEVEL SECURITY;
ALTER TABLE series FORCE ROW LEVEL SECURITY;
CREATE POLICY org_isolation ON series
    USING (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid)
    WITH CHECK (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid);

-- Series are created and read; a series is archived (via lifecycle), never hard
-- deleted, so no DELETE grant. INSERT for create; no UPDATE (label edits are a
-- future revision-flow concern, out of scope here).
GRANT SELECT, INSERT ON series TO console_rt;

-- ---------------------------------------------------------------------------
-- series_instances — membership: which (kind, id) objects belong to a series.
-- ---------------------------------------------------------------------------
-- console-gate: audited-table series_instances
CREATE TABLE series_instances (
    id          UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id      UUID        NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    series_id   UUID        NOT NULL,
    -- The member object as a generic (kind, id) reference, mirroring
    -- object_links: member_kind FKs the seeded registry; member_id is the
    -- domain id/code string.
    member_kind TEXT        NOT NULL REFERENCES object_types(kind) ON DELETE RESTRICT,
    member_id   TEXT        NOT NULL CHECK (char_length(btrim(member_id)) BETWEEN 1 AND 200),
    added_by    UUID        REFERENCES users(id) ON DELETE RESTRICT,
    added_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    -- An instance belongs to at most ONE series per tenant (the "not-yet-in-a-
    -- series" promotion model): a second attach of the same object is rejected.
    UNIQUE (org_id, member_kind, member_id),
    FOREIGN KEY (series_id, org_id) REFERENCES series (id, org_id) ON DELETE RESTRICT
);

-- List-by-series (ordered timeline) is the primary read.
CREATE INDEX idx_series_instances_series
    ON series_instances (org_id, series_id, added_at, id);

ALTER TABLE series_instances ENABLE ROW LEVEL SECURITY;
ALTER TABLE series_instances FORCE ROW LEVEL SECURITY;
CREATE POLICY org_isolation ON series_instances
    USING (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid)
    WITH CHECK (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid);

-- Instances are attached (INSERT) and read; detach is a future concern (kept as
-- append-only membership for now), so no DELETE/UPDATE grant.
GRANT SELECT, INSERT ON series_instances TO console_rt;
