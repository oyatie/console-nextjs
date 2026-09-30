-- §1b Owned effective-dated instance store for user-authored ('instance'-backed)
-- object types (§18 registry lane built the schema side in 0105; this is the data
-- side). Current state is a FOLD over immutable revisions, never an in-place
-- mutate: each edit stages a new revision and closes the prior one's validity
-- interval. Every revision is fixity-stamped into a per-(org,instance) hash chain
-- so instance history is tamper-evident for free. `ont_links` are the
-- effective-dated edges the §2 search-around traversal walks.
--
-- All three tables are tenant-scoped FORCE-RLS org-isolated (copied from 0105).
-- `ont_instance_revisions` is append-only (REVOKE UPDATE/DELETE + trigger): the
-- hash chain is only tamper-evident if a stored revision can never be rewritten,
-- and REVOKE alone is bypassed by the table owner, so the trigger is the real
-- guarantee. No hard delete anywhere (§9.8) — dispose is a terminal soft state.

-- console-gate: audited-table ont_instances
CREATE TABLE ont_instances (
    id                    UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id                UUID        NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    -- the object-type VERSION snapshot this instance conforms to (0105 head).
    object_type_id        UUID        NOT NULL,
    title                 TEXT        NOT NULL CHECK (char_length(title) BETWEEN 1 AND 200),
    -- pointer to the head revision; app-maintained within the same tx that
    -- appends the revision, so it is never dangling (NULL only mid-insert). Not a
    -- FK: instances <-> revisions is a mutual reference, and the app keeps it
    -- consistent transactionally.
    current_revision_id   UUID        NULL,
    lifecycle_state       TEXT        NOT NULL CHECK (lifecycle_state IN ('draft','active','locked','archived','disposed')),
    created_at            TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at            TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (id, org_id),
    FOREIGN KEY (object_type_id, org_id) REFERENCES ont_object_types(id, org_id) ON DELETE RESTRICT
);
CREATE INDEX idx_ont_instances_list
    ON ont_instances (org_id, object_type_id, lifecycle_state);

-- console-gate: audited-table ont_instance_revisions
CREATE TABLE ont_instance_revisions (
    id              UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id          UUID        NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    instance_id     UUID        NOT NULL,
    version         BIGINT      NOT NULL CHECK (version >= 1),
    -- JSONB attribute bag validated against the property schema (NOT EAV).
    attributes      JSONB       NOT NULL DEFAULT '{}'::jsonb CHECK (jsonb_typeof(attributes) = 'object'),
    valid_from      TIMESTAMPTZ NOT NULL,   -- effective-dating; future-dating (valid_from > now) allowed
    valid_to        TIMESTAMPTZ NULL,       -- NULL = current (open) revision
    action_type_id  UUID        NULL,       -- the action that produced this revision
    actor           UUID        NULL,
    reason          TEXT        NULL CHECK (reason IS NULL OR char_length(reason) <= 2000),
    prev_hash       CHAR(64)    NOT NULL,   -- previous revision's row_hash (64 zeros = genesis for v1)
    row_hash        CHAR(64)    NOT NULL,   -- canonical_hash(prev_hash || canonical(revision))
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (instance_id, version),
    CHECK (valid_to IS NULL OR valid_to > valid_from),
    FOREIGN KEY (instance_id, org_id) REFERENCES ont_instances(id, org_id) ON DELETE CASCADE
);
-- Exactly one open (head) revision per instance.
CREATE UNIQUE INDEX idx_ont_instance_revisions_one_open
    ON ont_instance_revisions (instance_id) WHERE valid_to IS NULL;
CREATE INDEX idx_ont_instance_revisions_asof
    ON ont_instance_revisions (org_id, instance_id, valid_from);
-- GIN for property filters over the attribute bag (§1b).
CREATE INDEX idx_ont_instance_revisions_attrs
    ON ont_instance_revisions USING GIN (attributes);

-- console-gate: audited-table ont_links
CREATE TABLE ont_links (
    id                UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id            UUID        NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    link_type_id      UUID        NOT NULL REFERENCES ont_link_types(id) ON DELETE RESTRICT,
    from_instance_id  UUID        NOT NULL,
    to_instance_id    UUID        NOT NULL,
    valid_from        TIMESTAMPTZ NOT NULL,
    valid_to          TIMESTAMPTZ NULL,     -- NULL = live edge; setting it soft-closes the link
    created_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK (valid_to IS NULL OR valid_to > valid_from),
    FOREIGN KEY (from_instance_id, org_id) REFERENCES ont_instances(id, org_id) ON DELETE CASCADE,
    FOREIGN KEY (to_instance_id, org_id) REFERENCES ont_instances(id, org_id) ON DELETE CASCADE
);
CREATE INDEX idx_ont_links_from ON ont_links (org_id, from_instance_id) WHERE valid_to IS NULL;
CREATE INDEX idx_ont_links_to ON ont_links (org_id, to_instance_id) WHERE valid_to IS NULL;

-- FORCE-RLS org isolation on every instance table (copied from 0105).
DO $$
DECLARE
    t TEXT;
    tenant_tables TEXT[] := ARRAY[
        'ont_instances',
        'ont_instance_revisions',
        'ont_links'
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

-- The instance head and links accept UPDATE (head pointer / lifecycle / link
-- close), so guard their org_id against rewrite (reuses 0031's function).
CREATE TRIGGER trg_ont_instances_org_immutable BEFORE UPDATE ON ont_instances
    FOR EACH ROW EXECUTE FUNCTION enforce_org_id_immutable();
CREATE TRIGGER trg_ont_links_org_immutable BEFORE UPDATE ON ont_links
    FOR EACH ROW EXECUTE FUNCTION enforce_org_id_immutable();

-- Revisions are an append-only fixity ledger (mirrors 0071/0091): a correction is
-- a NEW revision, never a rewrite of a stored one. The ONE legitimate mutation is
-- closing an open interval (`valid_to` NULL -> value) when the next revision is
-- staged — `valid_to` is metadata, deliberately NOT covered by the fixity hash.
-- Every fixity-covered column stays immutable, which is what keeps the
-- per-(org,instance) hash chain tamper-evident even against the table owner.
CREATE OR REPLACE FUNCTION ont_instance_revisions_append_only()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
    IF TG_OP = 'DELETE' THEN
        RAISE EXCEPTION 'ont_instance_revisions is append-only: DELETE is forbidden (row id=%)', OLD.id
            USING ERRCODE = '55000';
    END IF;
    -- UPDATE: only an open row may change, and only to close its interval.
    IF OLD.valid_to IS NOT NULL THEN
        RAISE EXCEPTION 'ont_instance_revisions is append-only: a closed revision (id=%) is forbidden to modify', OLD.id
            USING ERRCODE = '55000';
    END IF;
    IF NEW.valid_to IS NULL THEN
        RAISE EXCEPTION 'ont_instance_revisions: closing a revision must set a non-null valid_to (row id=%)', OLD.id
            USING ERRCODE = '55000';
    END IF;
    IF NEW.id <> OLD.id
        OR NEW.org_id <> OLD.org_id
        OR NEW.instance_id <> OLD.instance_id
        OR NEW.version <> OLD.version
        OR NEW.attributes::text <> OLD.attributes::text
        OR NEW.valid_from <> OLD.valid_from
        OR NEW.prev_hash <> OLD.prev_hash
        OR NEW.row_hash <> OLD.row_hash
        OR NEW.created_at <> OLD.created_at
        OR NEW.actor IS DISTINCT FROM OLD.actor
        OR NEW.action_type_id IS DISTINCT FROM OLD.action_type_id
        OR NEW.reason IS DISTINCT FROM OLD.reason
    THEN
        RAISE EXCEPTION 'ont_instance_revisions is append-only: only valid_to may be set to close an interval (row id=%)', OLD.id
            USING ERRCODE = '55000';
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER trg_ont_instance_revisions_no_update
    BEFORE UPDATE ON ont_instance_revisions
    FOR EACH ROW EXECUTE FUNCTION ont_instance_revisions_append_only();
CREATE TRIGGER trg_ont_instance_revisions_no_delete
    BEFORE DELETE ON ont_instance_revisions
    FOR EACH ROW EXECUTE FUNCTION ont_instance_revisions_append_only();

-- Runtime-role grants (mirrors 0105: the #[sqlx::test] harness runs migrations as
-- a superuser, so grant console_rt explicitly; production auto-grants via 0031).
--
-- Instance head: SELECT + INSERT + UPDATE (head pointer + lifecycle FSM), never DELETE.
GRANT SELECT, INSERT, UPDATE ON ont_instances TO console_rt;
REVOKE DELETE ON ont_instances FROM console_rt;
-- Revisions: SELECT + INSERT + UPDATE, but the trigger constrains UPDATE to the
-- single valid_to close (fixity columns immutable); never DELETE.
GRANT SELECT, INSERT, UPDATE ON ont_instance_revisions TO console_rt;
REVOKE DELETE ON ont_instance_revisions FROM console_rt;
-- Links: SELECT + INSERT + UPDATE (soft-close via valid_to), never DELETE.
GRANT SELECT, INSERT, UPDATE ON ont_links TO console_rt;
REVOKE DELETE ON ont_links FROM console_rt;
