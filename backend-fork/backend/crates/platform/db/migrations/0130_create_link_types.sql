-- BE-OBJ slice 3, surface 3: edge-type registry.
--
-- Today object_links.link_type is free-text (slug-validated only): any tenant
-- can mint an arbitrary relationship label. The design's "edge-type registry"
-- formalizes the vocabulary the way object_types formalizes node kinds — a
-- seeded, platform-wide lookup that object_links FKs to, so the graph's edge
-- labels are a closed, dereferenceable set instead of stringly-typed fiction.
--
-- Migration safety: object_links already holds live rows with arbitrary
-- link_types. We MUST NOT fail the FK add on an in-use value that is not in the
-- seed. So the order is: (1) create + seed the registry, (2) backfill every
-- distinct link_type already in use that the seed missed (added as active rows
-- so no existing edge is orphaned), (3) only THEN add the FK. This is the
-- verify-existing-first / add-unknown-to-seed rule, not fail-on-unknown.
--
-- link_types is a GLOBAL reference table (no org_id, no RLS), exactly like
-- object_types — the same closed vocabulary for every tenant. It is added to
-- the tenant-isolation gate's global allowlist.

-- ---------------------------------------------------------------------------
-- link_types — seeded relationship-label registry (global reference data).
-- ---------------------------------------------------------------------------
-- console-gate: global-table link_types (rationale: canonical edge-type vocabulary, seeded platform-wide, no tenant data)
CREATE TABLE link_types (
    -- Same slug shape as object_links.link_type's existing CHECK.
    link_type   TEXT        PRIMARY KEY CHECK (link_type ~ '^[a-z][a-z0-9_]{1,63}$'),
    description TEXT        NOT NULL CHECK (char_length(btrim(description)) BETWEEN 1 AND 200),
    status      TEXT        NOT NULL DEFAULT 'active'
        CHECK (status IN ('draft', 'active', 'archived')),
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Seed the canonical vocabulary. These are the relationship kinds the design's
-- ontology traversal (계약→인원→근태→급여→수익성 chain) and the object-card
-- link panels draw: a small, generic, direction-carrying set. New edge types
-- are added by appending a row in a later migration (like object_types).
INSERT INTO link_types (link_type, description) VALUES
    ('relates_to',    'Generic undirected association between two objects'),
    ('references',    'Source cites/points at the destination'),
    ('depends_on',    'Source requires the destination to proceed'),
    ('blocks',        'Source blocks the destination from proceeding'),
    ('part_of',       'Source is a component of the destination'),
    ('belongs_to',    'Source is owned by / assigned to the destination'),
    ('derived_from',  'Source was produced from the destination (lineage)'),
    ('authorized_by', 'Source was authorized/approved by the destination'),
    ('attached_to',   'Source (evidence/attachment) is attached to the destination'),
    ('member_of',     'Source instance is a member of the destination series/set'),
    ('uses',          'Source uses/consumes the destination (e.g. work order uses equipment)'),
    ('supersedes',    'Source replaces/supersedes the destination');

REVOKE ALL ON link_types FROM PUBLIC;
GRANT SELECT ON link_types TO console_rt;

-- ---------------------------------------------------------------------------
-- Backfill: adopt any in-use link_type the seed did not already cover, so the
-- FK add below cannot orphan an existing edge. Runs as the migration role (not
-- console_rt), so it sees every tenant's rows regardless of RLS.
-- ---------------------------------------------------------------------------
INSERT INTO link_types (link_type, description)
SELECT DISTINCT ol.link_type,
       'Backfilled from an in-use object_links edge (pre-registry)'
FROM object_links ol
WHERE NOT EXISTS (
    SELECT 1 FROM link_types lt WHERE lt.link_type = ol.link_type
);

-- ---------------------------------------------------------------------------
-- Constrain: now that every existing value is registered, add the FK metadata
-- without validating in this migration transaction. PostgreSQL's validation scan
-- can take a write-blocking lock on object_links; a follow-up migration performs
-- VALIDATE CONSTRAINT separately so deploy tooling can schedule/retry it without
-- coupling table creation/backfill to the scan.
-- ---------------------------------------------------------------------------
ALTER TABLE object_links
    ADD CONSTRAINT object_links_link_type_fkey
        FOREIGN KEY (link_type) REFERENCES link_types (link_type) ON DELETE RESTRICT
        NOT VALID;
