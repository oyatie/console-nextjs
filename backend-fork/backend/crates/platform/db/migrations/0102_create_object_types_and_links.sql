-- Generic object layer: a seeded object-type registry and a generic,
-- org-scoped, audited edge store.
--
-- The design's Foundry-like ontology needs (a) a canonical set of object
-- "kinds" that the three existing free-form object_type columns
-- (collaboration_calendar_events, collaboration_polls, workflow_runs) were
-- validating only by slug regex, and (b) arbitrary A<->B links between objects
-- ("related objects" / pin-A-to-B panels) that today have no backend at all.
--
-- object_types is a GLOBAL reference table (platform-wide kinds, identical for
-- every tenant): no org_id, no RLS. It is allowlisted in the tenant-isolation
-- CI gate. object_links IS tenant data: org_id + FORCE RLS keyed on
-- app.current_org, exactly like every other tenant table.

-- ---------------------------------------------------------------------------
-- object_types — seeded kind registry (global reference data).
-- ---------------------------------------------------------------------------
CREATE TABLE object_types (
    -- Canonical snake_case kind slug; mirrors the existing object_type CHECK
    -- (^[a-z][a-z0-9_]{1,63}$) so links can only connect known kinds.
    kind        TEXT        PRIMARY KEY CHECK (kind ~ '^[a-z][a-z0-9_]{1,63}$'),
    description TEXT        NOT NULL CHECK (char_length(btrim(description)) BETWEEN 1 AND 200),
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Seed the known kinds: the design catalog's base set plus every kind already
-- referenced by an existing (object_type, object_id) column. New kinds are
-- added by appending a row in a later migration.
INSERT INTO object_types (kind, description) VALUES
    ('work_order',        'Maintenance/field work order'),
    ('support_ticket',    'Customer or internal support ticket'),
    ('person',            'Employee / person record'),
    ('org_unit',          'Organizational unit (region/branch/worksite)'),
    ('approval_run',      'Workflow-engine approval run instance'),
    ('notification',      'Recipient-scoped notification'),
    ('mail_thread',       'Webmail conversation thread'),
    ('messenger_thread',  'Messenger conversation thread'),
    ('equipment',         'Registered equipment / asset'),
    ('listing',           'Sales listing'),
    ('voucher',           'Financial voucher / expenditure'),
    ('document',          'Archived document'),
    ('approval_document', 'Approval-backed document'),
    ('asset_transfer',    'Asset ownership transfer'),
    ('payroll_period',    'Payroll period run'),
    ('purchase_request',  'Financial purchase request');

GRANT SELECT ON object_types TO console_rt;

-- ---------------------------------------------------------------------------
-- object_links — generic, audited, removable edges (tenant data).
-- ---------------------------------------------------------------------------
-- console-gate: audited-table object_links
CREATE TABLE object_links (
    id          UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id      UUID        NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    src_kind    TEXT        NOT NULL REFERENCES object_types(kind) ON DELETE RESTRICT,
    src_id      TEXT        NOT NULL CHECK (char_length(btrim(src_id)) BETWEEN 1 AND 200),
    dst_kind    TEXT        NOT NULL REFERENCES object_types(kind) ON DELETE RESTRICT,
    dst_id      TEXT        NOT NULL CHECK (char_length(btrim(dst_id)) BETWEEN 1 AND 200),
    -- Relationship label, slug-validated like a kind (e.g. authorized_by,
    -- relates_to, blocks). Free-form-but-validated so new link types need no
    -- migration.
    link_type   TEXT        NOT NULL CHECK (link_type ~ '^[a-z][a-z0-9_]{1,63}$'),
    created_by  UUID        REFERENCES users(id) ON DELETE RESTRICT,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    -- One edge of a given type between a given ordered pair, per tenant. A
    -- second identical link is rejected (the duplicate-link guarantee).
    UNIQUE (org_id, src_kind, src_id, dst_kind, dst_id, link_type)
);

-- The UNIQUE index above backs list-by-source (org_id, src_kind, src_id, ...).
-- This one backs list-by-destination so the reverse walk is equally cheap.
CREATE INDEX idx_object_links_dst
    ON object_links (org_id, dst_kind, dst_id);

ALTER TABLE object_links ENABLE ROW LEVEL SECURITY;
ALTER TABLE object_links FORCE ROW LEVEL SECURITY;
CREATE POLICY org_isolation ON object_links
    USING (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid)
    WITH CHECK (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid);

-- Links are removable (hard delete), and every removal is audited by the
-- application via with_audit (the audit event's before-snapshot preserves the
-- removed edge). console_rt therefore gets DELETE here, unlike append-only tables.
-- Links are immutable once created (re-link = delete + create), so no UPDATE.
GRANT SELECT, INSERT, DELETE ON object_links TO console_rt;
