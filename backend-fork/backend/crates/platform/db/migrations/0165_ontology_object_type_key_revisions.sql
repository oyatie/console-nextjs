-- Tenant/key-scoped optimistic-concurrency token and the complete ontology
-- object-type single-writer boundary.

-- These cluster-global identities are provisioned before migrations run. 0165
-- deliberately has no CREATEROLE/ALTER ROLE fallback: ownership transfer is
-- safe only when the exact capability topology is already present. A
-- superuser is accepted solely for the sqlx migration harness; every
-- non-superuser application must run directly as the migration owner.
DO $$
DECLARE
    v_migrator OID := pg_catalog.to_regrole('console_app');
    v_runtime OID := pg_catalog.to_regrole('console_rt');
    v_writer OID := pg_catalog.to_regrole('console_ontology_writer');
    v_leave_writer OID := pg_catalog.to_regrole('console_leave_definer');
    v_command OID := pg_catalog.to_regrole('console_ontology_cmd');
    v_applier_is_superuser BOOLEAN;
BEGIN
    IF v_migrator IS NULL OR v_runtime IS NULL OR v_writer IS NULL
       OR v_leave_writer IS NULL OR v_command IS NULL THEN
        RAISE EXCEPTION USING
            ERRCODE = '42501',
            MESSAGE = 'ontology_role_topology.roles_not_preprovisioned';
    END IF;

    SELECT rolsuper INTO v_applier_is_superuser
    FROM pg_catalog.pg_roles
    WHERE rolname = CURRENT_USER;
    IF NOT v_applier_is_superuser
       AND (CURRENT_USER <> 'console_app' OR SESSION_USER <> 'console_app') THEN
        RAISE EXCEPTION USING
            ERRCODE = '42501',
            MESSAGE = 'ontology_role_topology.console_app_must_apply_directly';
    END IF;

    IF EXISTS (
        SELECT 1 FROM pg_catalog.pg_roles
        WHERE oid = v_migrator
          AND (NOT rolcanlogin OR NOT rolinherit OR rolsuper OR NOT rolbypassrls OR rolcreatedb
               OR rolcreaterole OR rolreplication)
    ) THEN
        RAISE EXCEPTION USING
            ERRCODE = '42501',
            MESSAGE = 'ontology_role_topology.console_app_not_hardened';
    END IF;

    IF EXISTS (
        SELECT 1 FROM pg_catalog.pg_roles
        WHERE oid = v_writer
          AND (rolcanlogin OR rolsuper OR rolbypassrls OR rolinherit
               OR rolcreatedb OR rolcreaterole OR rolreplication)
    ) THEN
        RAISE EXCEPTION USING
            ERRCODE = '42501',
            MESSAGE = 'ontology_role_topology.writer_not_hardened';
    END IF;

    IF EXISTS (
        SELECT 1 FROM pg_catalog.pg_roles
        WHERE oid = v_command
          AND (NOT rolcanlogin OR rolsuper OR rolbypassrls OR rolinherit
               OR rolcreatedb OR rolcreaterole OR rolreplication)
    ) THEN
        RAISE EXCEPTION USING
            ERRCODE = '42501',
            MESSAGE = 'ontology_role_topology.command_not_hardened';
    END IF;

    -- console_app receives only the two direct SET+INHERIT edges PostgreSQL requires
    -- to assign and operate objects owned by the preprovisioned NOLOGIN
    -- capability writers. It cannot administer either role, and no runtime or
    -- command identity participates in any membership edge.
    IF (SELECT COUNT(*) FROM pg_catalog.pg_auth_members
        WHERE member = v_migrator
          AND roleid IN (v_writer, v_leave_writer)
          AND NOT admin_option AND inherit_option AND set_option) <> 2
    OR EXISTS (
        SELECT 1 FROM pg_catalog.pg_auth_members
        WHERE (roleid IN (v_writer, v_leave_writer) AND member <> v_migrator)
           OR member IN (v_writer, v_leave_writer)
           OR (member = v_migrator AND roleid NOT IN (v_writer, v_leave_writer))
           OR roleid = v_migrator
           OR member IN (v_runtime, v_command)
           OR roleid IN (v_runtime, v_command)
    ) THEN
        RAISE EXCEPTION USING
            ERRCODE = '42501',
            MESSAGE = 'ontology_role_topology.membership_drift';
    END IF;
END
$$;

CREATE TABLE ont_object_type_key_revisions (
    org_id       UUID        NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    stable_key   TEXT        NOT NULL CHECK (stable_key ~ '^[a-z][a-z0-9_]*(\.[a-z][a-z0-9_]*)*$'),
    validator_id UUID        NOT NULL DEFAULT gen_random_uuid(),
    revision     BIGINT      NOT NULL DEFAULT 1 CHECK (revision >= 1),
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (org_id, stable_key),
    UNIQUE (validator_id),
    UNIQUE (org_id, stable_key, validator_id)
);

-- Legacy keys receive a conservative monotone baseline. The isolated
-- migration-only console_app identity is explicitly BYPASSRLS so data migrations
-- can upgrade every tenant without manufacturing application tenant context.
-- New keys start at r1.
INSERT INTO ont_object_type_key_revisions (
    org_id, stable_key, revision, created_at, updated_at
)
SELECT org_id, stable_key, MAX(schema_version), MIN(created_at), MAX(updated_at)
FROM ont_object_types
GROUP BY org_id, stable_key;

-- Migration-owned allowlist for exact built-in catalog manifests, plus one
-- immutable install marker per tenant. The allowlist is intentionally not tenant
-- writable; runtime can only present a manifest whose canonical JSONB digest was
-- pinned by a migration.
-- Platform-global control-plane reference data (owner/capability-only): no
-- direct PUBLIC or console_rt access; ontology commands read it through the writer.
CREATE TABLE ont_builtin_catalog_allowlist (
    catalog_version TEXT PRIMARY KEY CHECK (btrim(catalog_version) <> ''),
    manifest_digest BYTEA NOT NULL CHECK (octet_length(manifest_digest) = 32),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
INSERT INTO ont_builtin_catalog_allowlist (catalog_version, manifest_digest)
VALUES (
    '2026-07-19.1',
    decode('e2b5fdff9a03d4d798344cac2496acab412ffc21e2be84c03e7345a328123247', 'hex')
);
ALTER TABLE ont_builtin_catalog_allowlist OWNER TO console_app;
REVOKE ALL ON ont_builtin_catalog_allowlist FROM PUBLIC, console_rt;

CREATE TABLE ont_builtin_catalog_installs (
    org_id UUID PRIMARY KEY REFERENCES organizations(id) ON DELETE RESTRICT,
    catalog_version TEXT NOT NULL,
    manifest_digest BYTEA NOT NULL CHECK (octet_length(manifest_digest) = 32),
    installed_by UUID NOT NULL,
    installed_at TIMESTAMPTZ NOT NULL,
    UNIQUE (org_id, catalog_version, manifest_digest),
    FOREIGN KEY (catalog_version) REFERENCES ont_builtin_catalog_allowlist(catalog_version) ON DELETE RESTRICT,
    FOREIGN KEY (installed_by, org_id) REFERENCES users(id, org_id) ON DELETE RESTRICT
);
ALTER TABLE ont_builtin_catalog_installs ENABLE ROW LEVEL SECURITY;
ALTER TABLE ont_builtin_catalog_installs FORCE ROW LEVEL SECURITY;
CREATE POLICY org_isolation ON ont_builtin_catalog_installs
    USING (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid)
    WITH CHECK (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid);
CREATE TRIGGER trg_ont_builtin_catalog_installs_org_immutable
    BEFORE UPDATE ON ont_builtin_catalog_installs
    FOR EACH ROW EXECUTE FUNCTION enforce_org_id_immutable();

ALTER TABLE ont_object_types
    ADD CONSTRAINT fk_ont_object_types_key_revision
    FOREIGN KEY (org_id, stable_key)
    REFERENCES ont_object_type_key_revisions (org_id, stable_key)
    ON DELETE RESTRICT;

ALTER TABLE ont_object_type_key_revisions ENABLE ROW LEVEL SECURITY;
ALTER TABLE ont_object_type_key_revisions FORCE ROW LEVEL SECURITY;
CREATE POLICY org_isolation ON ont_object_type_key_revisions
    USING (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid)
    WITH CHECK (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid);
CREATE TRIGGER trg_ont_object_type_key_revisions_org_immutable
    BEFORE UPDATE ON ont_object_type_key_revisions
    FOR EACH ROW EXECUTE FUNCTION enforce_org_id_immutable();

-- Cluster-global, non-login capability owner. It is never granted to console_rt;
-- console_ontology_cmd receives only USAGE on ontology_api and EXECUTE on the four
-- complete mutation+audit entrypoints below.
CREATE SCHEMA ontology_api AUTHORIZATION console_ontology_writer;
REVOKE ALL ON SCHEMA ontology_api FROM PUBLIC, console_rt;
GRANT USAGE ON SCHEMA ontology_api TO console_ontology_cmd;
GRANT USAGE ON SCHEMA public TO console_ontology_writer;

-- The new binary writes only through ontology_api. During one blue/green
-- compatibility window, however, the retained pre-0165 ReplicaSet still emits
-- the exact 0152 parent/child DML followed by an audit row in one transaction.
-- Keep only that old shape: parent INSERT/lifecycle UPDATE and append-only child
-- INSERT. The triggers below make the legacy transaction audit-mandatory and
-- advance the key sidecar exactly once; arbitrary draft UPDATE, child append to
-- an unrelated transaction, DELETE, and TRUNCATE remain impossible. A later
-- contract migration may remove these compatibility grants after old pods are
-- proven absent.
GRANT SELECT ON
    ont_object_types,
    ont_object_type_key_revisions,
    ont_property_defs,
    ont_link_types,
    ont_action_types,
    ont_analytics,
    ont_builtin_catalog_installs
TO console_rt;
REVOKE ALL PRIVILEGES ON
    ont_object_types,
    ont_object_type_key_revisions,
    ont_property_defs,
    ont_link_types,
    ont_action_types,
    ont_analytics,
    ont_builtin_catalog_allowlist,
    ont_builtin_catalog_installs,
    audit_events,
    users,
    gov_approval_requests,
    gov_approvals,
    gov_approval_consumptions
FROM console_ontology_cmd;
GRANT INSERT, UPDATE ON ont_object_types TO console_rt;
GRANT INSERT ON ont_property_defs, ont_link_types, ont_action_types, ont_analytics TO console_rt;
REVOKE DELETE, TRUNCATE ON ont_object_types FROM console_rt, PUBLIC;
REVOKE INSERT, UPDATE, DELETE, TRUNCATE ON ont_object_type_key_revisions,
    ont_builtin_catalog_installs FROM console_rt, PUBLIC;
REVOKE UPDATE, DELETE, TRUNCATE ON ont_property_defs, ont_link_types,
    ont_action_types, ont_analytics FROM console_rt, PUBLIC;

GRANT SELECT, INSERT, UPDATE ON
    ont_object_types,
    ont_object_type_key_revisions
TO console_ontology_writer;
GRANT SELECT, INSERT ON
    ont_property_defs,
    ont_link_types,
    ont_action_types,
    ont_analytics,
    audit_events,
    gov_approval_consumptions,
    ont_builtin_catalog_installs
TO console_ontology_writer;
GRANT SELECT ON users, gov_approval_requests, ont_builtin_catalog_allowlist TO console_ontology_writer;
-- PostgreSQL requires UPDATE privilege for SELECT ... FOR UPDATE even though
-- the transition routine never changes the approval row itself.
GRANT SELECT, UPDATE ON gov_approvals TO console_ontology_writer;

-- Ontology success events are proof that a DB-owned command completed. The
-- general runtime role must not be able to forge those audit facts directly.
CREATE FUNCTION ontology_api.invoker_role()
RETURNS NAME
LANGUAGE sql
STABLE
SET search_path = pg_catalog
AS $$
    SELECT CASE
        WHEN pg_catalog.current_setting('role', true) IS NOT NULL
         AND pg_catalog.current_setting('role', true) <> 'none'
        THEN pg_catalog.current_setting('role', true)::NAME
        ELSE SESSION_USER::NAME
    END
$$;
ALTER FUNCTION ontology_api.invoker_role() OWNER TO console_ontology_writer;

-- The old binary must be able to insert a new parent before the new sidecar FK
-- is checked. It may update only lifecycle_state+updated_at; content updates are
-- exclusively owned by the new command functions. The shared org lock also
-- closes the install-empty-check race for both generations of writer.
CREATE FUNCTION ontology_api.prepare_legacy_object_type_write()
RETURNS TRIGGER
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog
SET row_security = on
AS $$
BEGIN
    IF ontology_api.invoker_role() <> 'console_rt'::NAME THEN
        RETURN NEW;
    END IF;

    PERFORM pg_catalog.pg_advisory_xact_lock(
        pg_catalog.hashtextextended('ontology-bootstrap:' || NEW.org_id::TEXT, 0)
    );
    IF TG_OP = 'INSERT' THEN
        INSERT INTO public.ont_object_type_key_revisions (org_id, stable_key)
        VALUES (NEW.org_id, NEW.stable_key)
        ON CONFLICT (org_id, stable_key) DO NOTHING;
    ELSIF ROW(
        NEW.id, NEW.org_id, NEW.stable_key, NEW.title, NEW.title_property_key,
        NEW.backing_kind, NEW.backing_table, NEW.primary_key_property,
        NEW.schema_version, NEW.created_by, NEW.created_at
    ) IS DISTINCT FROM ROW(
        OLD.id, OLD.org_id, OLD.stable_key, OLD.title, OLD.title_property_key,
        OLD.backing_kind, OLD.backing_table, OLD.primary_key_property,
        OLD.schema_version, OLD.created_by, OLD.created_at
    ) OR NEW.lifecycle_state IS NOT DISTINCT FROM OLD.lifecycle_state
      OR NEW.updated_at IS NOT DISTINCT FROM OLD.updated_at THEN
        RAISE EXCEPTION USING
            ERRCODE = '42501',
            MESSAGE = 'ontology_legacy.lifecycle_update_only';
    END IF;
    RETURN NEW;
END;
$$;
ALTER FUNCTION ontology_api.prepare_legacy_object_type_write() OWNER TO console_ontology_writer;
CREATE TRIGGER trg_ont_object_types_legacy_write_guard
    BEFORE INSERT OR UPDATE ON public.ont_object_types
    FOR EACH ROW EXECUTE FUNCTION ontology_api.prepare_legacy_object_type_write();

CREATE FUNCTION ontology_api.protected_audit_writer_guard()
RETURNS TRIGGER
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog
SET row_security = on
AS $$
DECLARE
    v_invoker NAME := ontology_api.invoker_role();
    v_stable_key TEXT;
BEGIN
    IF NEW.action <> ALL (ARRAY[
        'ontology.object_type.create',
        'ontology.object_type.stage_revision',
        'ontology.object_type.transition',
        'ontology.object_type.builtin_install'
    ]::TEXT[]) THEN
        RETURN NEW;
    END IF;

    -- Direct command credentials cannot INSERT audit_events. When an approved
    -- command reaches this trigger it is nested inside the writer-owned
    -- SECURITY DEFINER routine, while the old compatibility path arrives as
    -- console_rt and must prove a matching parent mutation in this transaction.
    IF v_invoker = 'console_rt'::NAME THEN
        IF NEW.action = 'ontology.object_type.builtin_install'
           OR NEW.target_type <> 'ont_object_types'
           OR NOT EXISTS (
               SELECT 1
               FROM public.users u
               WHERE u.id = NEW.actor AND u.org_id = NEW.org_id AND u.is_active
           ) THEN
            RAISE EXCEPTION USING ERRCODE = '42501', MESSAGE = 'ontology_audit.command_required';
        END IF;

        SELECT o.stable_key INTO v_stable_key
        FROM public.ont_object_types o
        WHERE o.org_id = NEW.org_id
          AND o.id::TEXT = NEW.target_id
          AND o.updated_at = NEW.occurred_at
          AND o.xmin = pg_catalog.pg_current_xact_id()::xid
          AND (
              (NEW.action = 'ontology.object_type.create' AND o.schema_version = 1 AND o.created_at = NEW.occurred_at)
              OR (NEW.action = 'ontology.object_type.stage_revision' AND o.schema_version > 1 AND o.created_at = NEW.occurred_at)
              OR NEW.action = 'ontology.object_type.transition'
          );
        IF v_stable_key IS NULL THEN
            RAISE EXCEPTION USING ERRCODE = '42501', MESSAGE = 'ontology_audit.command_required';
        END IF;
        IF NEW.action IN ('ontology.object_type.stage_revision', 'ontology.object_type.transition') THEN
            UPDATE public.ont_object_type_key_revisions k
               SET revision = k.revision + 1, updated_at = NEW.occurred_at
             WHERE k.org_id = NEW.org_id AND k.stable_key = v_stable_key;
            IF NOT FOUND THEN
                RAISE EXCEPTION USING ERRCODE = '23503', MESSAGE = 'ontology_legacy.key_revision_missing';
            END IF;
        END IF;
    ELSIF v_invoker <> 'console_ontology_cmd'::NAME THEN
        RAISE EXCEPTION USING
            ERRCODE = '42501',
            MESSAGE = 'ontology_audit.command_required';
    END IF;
    RETURN NEW;
END;
$$;
ALTER FUNCTION ontology_api.protected_audit_writer_guard() OWNER TO console_ontology_writer;
CREATE TRIGGER trg_audit_events_ontology_command_only
    BEFORE INSERT ON public.audit_events
    FOR EACH ROW EXECUTE FUNCTION ontology_api.protected_audit_writer_guard();

-- Every parent/child mutation must be accompanied by exactly one protected
-- audit row inserted by the same database transaction. This is deferred so the
-- retained binary may keep its historical mutation-then-audit ordering. xmin
-- is used only as transaction-local evidence, never as a durable business
-- identifier.
CREATE FUNCTION ontology_api.require_current_transaction_audit()
RETURNS TRIGGER
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog
SET row_security = on
AS $$
DECLARE
    v_parent_id UUID;
    v_org_id UUID;
    v_stable_key TEXT;
    v_parent_is_current BOOLEAN;
BEGIN
    IF TG_TABLE_NAME = 'ont_object_types' THEN
        v_parent_id := NEW.id;
        v_org_id := NEW.org_id;
        v_stable_key := NEW.stable_key;
        v_parent_is_current := TRUE;
    ELSE
        v_parent_id := NEW.object_type_id;
        v_org_id := NEW.org_id;
        SELECT o.stable_key,
               o.xmin = pg_catalog.pg_current_xact_id()::xid
          INTO v_stable_key, v_parent_is_current
          FROM public.ont_object_types o
         WHERE o.id = v_parent_id AND o.org_id = v_org_id;
    END IF;

    IF COALESCE(v_parent_is_current, FALSE) AND TG_TABLE_NAME <> 'ont_object_types' THEN
        RETURN NEW;
    END IF;
    IF (
        SELECT COUNT(*) = 1
        FROM public.audit_events e
        LEFT JOIN public.ont_object_types target
          ON target.org_id = e.org_id AND target.id::TEXT = e.target_id
        WHERE e.org_id = v_org_id
          AND e.action = ANY (ARRAY[
              'ontology.object_type.create',
              'ontology.object_type.stage_revision',
              'ontology.object_type.transition',
              'ontology.object_type.builtin_install'
          ]::TEXT[])
          AND e.xmin = pg_catalog.pg_current_xact_id()::xid
          AND (
              e.target_id = v_parent_id::TEXT
              OR (e.action = 'ontology.object_type.transition' AND target.stable_key = v_stable_key)
          )
    ) THEN
        RETURN NEW;
    END IF;
    RAISE EXCEPTION USING
        ERRCODE = '23514',
        MESSAGE = 'ontology_write.exactly_one_current_transaction_audit_required';
END;
$$;
ALTER FUNCTION ontology_api.require_current_transaction_audit() OWNER TO console_ontology_writer;
CREATE CONSTRAINT TRIGGER trg_ont_object_types_current_audit
    AFTER INSERT OR UPDATE ON public.ont_object_types
    DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW EXECUTE FUNCTION ontology_api.require_current_transaction_audit();
CREATE CONSTRAINT TRIGGER trg_ont_property_defs_current_audit
    AFTER INSERT ON public.ont_property_defs
    DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW EXECUTE FUNCTION ontology_api.require_current_transaction_audit();
CREATE CONSTRAINT TRIGGER trg_ont_link_types_current_audit
    AFTER INSERT ON public.ont_link_types
    DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW EXECUTE FUNCTION ontology_api.require_current_transaction_audit();
CREATE CONSTRAINT TRIGGER trg_ont_action_types_current_audit
    AFTER INSERT ON public.ont_action_types
    DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW EXECUTE FUNCTION ontology_api.require_current_transaction_audit();
CREATE CONSTRAINT TRIGGER trg_ont_analytics_current_audit
    AFTER INSERT ON public.ont_analytics
    DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW EXECUTE FUNCTION ontology_api.require_current_transaction_audit();

CREATE FUNCTION ontology_api.assert_write_context(
    p_org_id UUID,
    p_actor UUID,
    p_trace_id TEXT,
    p_span_id TEXT
)
RETURNS VOID
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog
SET row_security = on
AS $$
DECLARE
    v_current_org UUID := NULLIF(pg_catalog.current_setting('app.current_org', true), '')::UUID;
BEGIN
    IF v_current_org IS NULL OR v_current_org <> p_org_id THEN
        RAISE EXCEPTION USING ERRCODE = '42501', MESSAGE = 'ontology_write.tenant_context_mismatch';
    END IF;
    IF NOT EXISTS (
        SELECT 1 FROM public.users u
        WHERE u.id = p_actor AND u.org_id = p_org_id AND u.is_active
    ) THEN
        RAISE EXCEPTION USING ERRCODE = '42501', MESSAGE = 'ontology_write.actor_forbidden';
    END IF;
    IF p_trace_id !~ '^[0-9a-f]{32}$' OR p_span_id !~ '^[0-9a-f]{16}$' THEN
        RAISE EXCEPTION USING ERRCODE = '22023', MESSAGE = 'ontology_write.invalid_trace';
    END IF;
END;
$$;

CREATE FUNCTION ontology_api.insert_children(
    p_org_id UUID,
    p_object_type_id UUID,
    p_snapshot JSONB,
    p_allow_existing BOOLEAN
)
RETURNS VOID
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog
SET row_security = on
AS $$
DECLARE
    v_item JSONB;
    v_existing JSONB;
    v_key TEXT;
BEGIN
    IF pg_catalog.jsonb_typeof(p_snapshot) IS DISTINCT FROM 'object'
       OR pg_catalog.jsonb_typeof(p_snapshot->'properties') IS DISTINCT FROM 'array'
       OR pg_catalog.jsonb_typeof(p_snapshot->'links') IS DISTINCT FROM 'array'
       OR pg_catalog.jsonb_typeof(p_snapshot->'actions') IS DISTINCT FROM 'array'
       OR pg_catalog.jsonb_typeof(p_snapshot->'analytics') IS DISTINCT FROM 'array' THEN
        RAISE EXCEPTION USING ERRCODE = '22023', MESSAGE = 'ontology_write.invalid_snapshot_shape';
    END IF;

    -- A staged draft is append-only for child identities. Its submitted full
    -- snapshot must retain every existing child byte-semantically after the
    -- same trim/default canonicalization used for incoming rows.
    IF p_allow_existing AND EXISTS (
        SELECT 1 FROM public.ont_property_defs d
        WHERE d.org_id = p_org_id AND d.object_type_id = p_object_type_id
          AND NOT EXISTS (
              SELECT 1 FROM pg_catalog.jsonb_array_elements(COALESCE(p_snapshot->'properties', '[]'::JSONB)) i
              WHERE pg_catalog.btrim(i->>'key') = d.key
                AND pg_catalog.jsonb_build_object(
                    'key', pg_catalog.btrim(i->>'key'),
                    'title', pg_catalog.btrim(i->>'title'),
                    'field_type', pg_catalog.btrim(i->>'field_type'),
                    'config', COALESCE(i->'config', '{}'::JSONB),
                    'backing_column', i->'backing_column',
                    'required', COALESCE((i->>'required')::BOOLEAN, FALSE),
                    'in_property_policy', COALESCE((i->>'in_property_policy')::BOOLEAN, FALSE)
                ) = pg_catalog.jsonb_build_object(
                    'key', d.key, 'title', d.title, 'field_type', d.type,
                    'config', d.config, 'backing_column', pg_catalog.to_jsonb(d.backing_column),
                    'required', d.required, 'in_property_policy', d.in_property_policy
                )
          )
    ) THEN
        RAISE EXCEPTION USING ERRCODE = '23505', MESSAGE = 'ontology_write.property_snapshot_conflict';
    END IF;

    IF p_allow_existing AND EXISTS (
        SELECT 1 FROM public.ont_link_types d
        WHERE d.org_id = p_org_id AND d.object_type_id = p_object_type_id
          AND NOT EXISTS (
              SELECT 1 FROM pg_catalog.jsonb_array_elements(COALESCE(p_snapshot->'links', '[]'::JSONB)) i
              WHERE pg_catalog.btrim(i->>'stable_key') = d.stable_key
                AND pg_catalog.jsonb_build_object(
                    'stable_key', pg_catalog.btrim(i->>'stable_key'),
                    'title', pg_catalog.btrim(i->>'title'),
                    'reverse_title', CASE WHEN i->>'reverse_title' IS NULL THEN 'null'::JSONB ELSE pg_catalog.to_jsonb(pg_catalog.btrim(i->>'reverse_title')) END,
                    'to_object_type_id', i->'to_object_type_id',
                    'cardinality', i->'cardinality',
                    'traversable', COALESCE((i->>'traversable')::BOOLEAN, TRUE)
                ) = pg_catalog.jsonb_build_object(
                    'stable_key', d.stable_key, 'title', d.title,
                    'reverse_title', pg_catalog.to_jsonb(d.reverse_title),
                    'to_object_type_id', pg_catalog.to_jsonb(d.to_object_type_id),
                    'cardinality', d.cardinality, 'traversable', d.traversable
                )
          )
    ) THEN
        RAISE EXCEPTION USING ERRCODE = '23505', MESSAGE = 'ontology_write.link_snapshot_conflict';
    END IF;

    IF p_allow_existing AND EXISTS (
        SELECT 1 FROM public.ont_action_types d
        WHERE d.org_id = p_org_id AND d.object_type_id = p_object_type_id
          AND NOT EXISTS (
              SELECT 1 FROM pg_catalog.jsonb_array_elements(COALESCE(p_snapshot->'actions', '[]'::JSONB)) i
              WHERE pg_catalog.btrim(i->>'stable_key') = d.stable_key
                AND pg_catalog.jsonb_build_object(
                    'stable_key', pg_catalog.btrim(i->>'stable_key'),
                    'title', pg_catalog.btrim(i->>'title'),
                    'params_schema', COALESCE(i->'params_schema', '{}'::JSONB),
                    'edits', COALESCE(i->'edits', '[]'::JSONB),
                    'submission_criteria', COALESCE(i->'submission_criteria', '[]'::JSONB),
                    'side_effects', COALESCE(i->'side_effects', '[]'::JSONB),
                    'dispatch', i->'dispatch',
                    'dispatch_target', i->'dispatch_target',
                    'control_points', COALESCE(i->'control_points', '[]'::JSONB)
                ) = pg_catalog.jsonb_build_object(
                    'stable_key', d.stable_key, 'title', d.title,
                    'params_schema', d.params_schema, 'edits', d.edits,
                    'submission_criteria', d.submission_criteria,
                    'side_effects', d.side_effects, 'dispatch', d.dispatch,
                    'dispatch_target', pg_catalog.to_jsonb(d.dispatch_target),
                    'control_points', d.control_points
                )
          )
    ) THEN
        RAISE EXCEPTION USING ERRCODE = '23505', MESSAGE = 'ontology_write.action_snapshot_conflict';
    END IF;

    IF p_allow_existing AND EXISTS (
        SELECT 1 FROM public.ont_analytics d
        WHERE d.org_id = p_org_id AND d.object_type_id = p_object_type_id
          AND NOT EXISTS (
              SELECT 1 FROM pg_catalog.jsonb_array_elements(COALESCE(p_snapshot->'analytics', '[]'::JSONB)) i
              WHERE pg_catalog.btrim(i->>'key') = d.key
                AND pg_catalog.jsonb_build_object(
                    'key', pg_catalog.btrim(i->>'key'),
                    'title', pg_catalog.btrim(i->>'title'),
                    'formula', COALESCE(i->'formula', '{}'::JSONB),
                    'result_type', COALESCE(i->'result_type', '{}'::JSONB)
                ) = pg_catalog.jsonb_build_object(
                    'key', d.key, 'title', d.title,
                    'formula', d.formula, 'result_type', d.result_type
                )
          )
    ) THEN
        RAISE EXCEPTION USING ERRCODE = '23505', MESSAGE = 'ontology_write.analytic_snapshot_conflict';
    END IF;

    FOR v_item IN SELECT value FROM pg_catalog.jsonb_array_elements(COALESCE(p_snapshot->'properties', '[]'::JSONB)) LOOP
        v_key := pg_catalog.btrim(v_item->>'key');
        SELECT pg_catalog.jsonb_build_object(
                   'key', d.key, 'title', d.title, 'field_type', d.type,
                   'config', d.config, 'backing_column', pg_catalog.to_jsonb(d.backing_column),
                   'required', d.required, 'in_property_policy', d.in_property_policy)
          INTO v_existing
          FROM public.ont_property_defs d
         WHERE d.org_id = p_org_id AND d.object_type_id = p_object_type_id AND d.key = v_key;
        IF FOUND THEN
            IF NOT p_allow_existing OR v_existing <> pg_catalog.jsonb_build_object(
                'key', v_key, 'title', pg_catalog.btrim(v_item->>'title'),
                'field_type', pg_catalog.btrim(v_item->>'field_type'),
                'config', COALESCE(v_item->'config', '{}'::JSONB),
                'backing_column', v_item->'backing_column',
                'required', COALESCE((v_item->>'required')::BOOLEAN, FALSE),
                'in_property_policy', COALESCE((v_item->>'in_property_policy')::BOOLEAN, FALSE)) THEN
                RAISE EXCEPTION USING ERRCODE = '23505', MESSAGE = 'ontology_write.property_key_conflict';
            END IF;
        ELSE
            INSERT INTO public.ont_property_defs
                (id, org_id, object_type_id, key, title, type, config, backing_column, required, in_property_policy)
            VALUES
                (public.gen_random_uuid(), p_org_id, p_object_type_id, v_key,
                 pg_catalog.btrim(v_item->>'title'), pg_catalog.btrim(v_item->>'field_type'),
                 COALESCE(v_item->'config', '{}'::JSONB), NULLIF(v_item->>'backing_column', ''),
                 COALESCE((v_item->>'required')::BOOLEAN, FALSE),
                 COALESCE((v_item->>'in_property_policy')::BOOLEAN, FALSE));
        END IF;
    END LOOP;

    FOR v_item IN SELECT value FROM pg_catalog.jsonb_array_elements(COALESCE(p_snapshot->'links', '[]'::JSONB)) LOOP
        v_key := pg_catalog.btrim(v_item->>'stable_key');
        SELECT pg_catalog.jsonb_build_object(
                   'stable_key', d.stable_key, 'title', d.title,
                   'reverse_title', pg_catalog.to_jsonb(d.reverse_title),
                   'to_object_type_id', pg_catalog.to_jsonb(d.to_object_type_id),
                   'cardinality', d.cardinality, 'traversable', d.traversable)
          INTO v_existing
          FROM public.ont_link_types d
         WHERE d.org_id = p_org_id AND d.object_type_id = p_object_type_id AND d.stable_key = v_key;
        IF FOUND THEN
            IF NOT p_allow_existing OR v_existing <> pg_catalog.jsonb_build_object(
                'stable_key', v_key, 'title', pg_catalog.btrim(v_item->>'title'),
                'reverse_title', CASE WHEN v_item->>'reverse_title' IS NULL THEN 'null'::JSONB ELSE pg_catalog.to_jsonb(pg_catalog.btrim(v_item->>'reverse_title')) END,
                'to_object_type_id', v_item->'to_object_type_id', 'cardinality', v_item->'cardinality',
                'traversable', COALESCE((v_item->>'traversable')::BOOLEAN, TRUE)) THEN
                RAISE EXCEPTION USING ERRCODE = '23505', MESSAGE = 'ontology_write.link_key_conflict';
            END IF;
        ELSE
            INSERT INTO public.ont_link_types
                (id, org_id, object_type_id, stable_key, title, reverse_title, to_object_type_id, cardinality, traversable)
            VALUES
                (public.gen_random_uuid(), p_org_id, p_object_type_id, v_key,
                 pg_catalog.btrim(v_item->>'title'), NULLIF(pg_catalog.btrim(v_item->>'reverse_title'), ''),
                 NULLIF(v_item->>'to_object_type_id', '')::UUID, v_item->>'cardinality',
                 COALESCE((v_item->>'traversable')::BOOLEAN, TRUE));
        END IF;
    END LOOP;

    FOR v_item IN SELECT value FROM pg_catalog.jsonb_array_elements(COALESCE(p_snapshot->'actions', '[]'::JSONB)) LOOP
        v_key := pg_catalog.btrim(v_item->>'stable_key');
        SELECT pg_catalog.jsonb_build_object(
                   'stable_key', d.stable_key, 'title', d.title,
                   'params_schema', d.params_schema, 'edits', d.edits,
                   'submission_criteria', d.submission_criteria, 'side_effects', d.side_effects,
                   'dispatch', d.dispatch, 'dispatch_target', pg_catalog.to_jsonb(d.dispatch_target),
                   'control_points', d.control_points)
          INTO v_existing
          FROM public.ont_action_types d
         WHERE d.org_id = p_org_id AND d.object_type_id = p_object_type_id AND d.stable_key = v_key;
        IF FOUND THEN
            IF NOT p_allow_existing OR v_existing <> pg_catalog.jsonb_build_object(
                'stable_key', v_key, 'title', pg_catalog.btrim(v_item->>'title'),
                'params_schema', COALESCE(v_item->'params_schema', '{}'::JSONB),
                'edits', COALESCE(v_item->'edits', '[]'::JSONB),
                'submission_criteria', COALESCE(v_item->'submission_criteria', '[]'::JSONB),
                'side_effects', COALESCE(v_item->'side_effects', '[]'::JSONB),
                'dispatch', v_item->'dispatch', 'dispatch_target', v_item->'dispatch_target',
                'control_points', COALESCE(v_item->'control_points', '[]'::JSONB)) THEN
                RAISE EXCEPTION USING ERRCODE = '23505', MESSAGE = 'ontology_write.action_key_conflict';
            END IF;
        ELSE
            INSERT INTO public.ont_action_types
                (id, org_id, object_type_id, stable_key, title, params_schema, edits,
                 submission_criteria, side_effects, dispatch, dispatch_target, control_points)
            VALUES
                (public.gen_random_uuid(), p_org_id, p_object_type_id, v_key,
                 pg_catalog.btrim(v_item->>'title'), COALESCE(v_item->'params_schema', '{}'::JSONB),
                 COALESCE(v_item->'edits', '[]'::JSONB), COALESCE(v_item->'submission_criteria', '[]'::JSONB),
                 COALESCE(v_item->'side_effects', '[]'::JSONB), v_item->>'dispatch',
                 NULLIF(v_item->>'dispatch_target', ''), COALESCE(v_item->'control_points', '[]'::JSONB));
        END IF;
    END LOOP;

    FOR v_item IN SELECT value FROM pg_catalog.jsonb_array_elements(COALESCE(p_snapshot->'analytics', '[]'::JSONB)) LOOP
        v_key := pg_catalog.btrim(v_item->>'key');
        SELECT pg_catalog.jsonb_build_object('key', d.key, 'title', d.title, 'formula', d.formula, 'result_type', d.result_type)
          INTO v_existing
          FROM public.ont_analytics d
         WHERE d.org_id = p_org_id AND d.object_type_id = p_object_type_id AND d.key = v_key;
        IF FOUND THEN
            IF NOT p_allow_existing OR v_existing <> pg_catalog.jsonb_build_object(
                'key', v_key, 'title', pg_catalog.btrim(v_item->>'title'),
                'formula', COALESCE(v_item->'formula', '{}'::JSONB),
                'result_type', COALESCE(v_item->'result_type', '{}'::JSONB)) THEN
                RAISE EXCEPTION USING ERRCODE = '23505', MESSAGE = 'ontology_write.analytic_key_conflict';
            END IF;
        ELSE
            INSERT INTO public.ont_analytics
                (id, org_id, object_type_id, key, title, formula, result_type)
            VALUES
                (public.gen_random_uuid(), p_org_id, p_object_type_id, v_key,
                 pg_catalog.btrim(v_item->>'title'), COALESCE(v_item->'formula', '{}'::JSONB),
                 COALESCE(v_item->'result_type', '{}'::JSONB));
        END IF;
    END LOOP;
END;
$$;

CREATE FUNCTION ontology_api.write_audit(
    p_org_id UUID,
    p_actor UUID,
    p_action TEXT,
    p_object_type_id UUID,
    p_before JSONB,
    p_after JSONB,
    p_trace_id TEXT,
    p_span_id TEXT,
    p_occurred_at TIMESTAMPTZ
)
RETURNS VOID
LANGUAGE sql
SECURITY DEFINER
SET search_path = pg_catalog
SET row_security = on
AS $$
    INSERT INTO public.audit_events
        (id, actor, action, target_type, target_id, branch_id, before_snap, after_snap,
         trace_id, span_id, occurred_at, org_id)
    VALUES
        (public.gen_random_uuid(), p_actor, p_action, 'ont_object_types', p_object_type_id::TEXT,
         NULL, p_before, p_after, p_trace_id, p_span_id, p_occurred_at, p_org_id)
$$;

CREATE FUNCTION ontology_api.create_object_type(
    p_org_id UUID,
    p_snapshot JSONB,
    p_actor UUID,
    p_trace_id TEXT,
    p_span_id TEXT
)
RETURNS TABLE(
    object_type_id UUID,
    stable_key TEXT,
    title TEXT,
    backing_kind TEXT,
    schema_version BIGINT,
    lifecycle_state TEXT,
    key_write_validator_id UUID,
    key_write_revision BIGINT
)
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog
SET row_security = on
AS $$
DECLARE
    v_id UUID := public.gen_random_uuid();
    v_validator UUID;
    v_stable_key TEXT := pg_catalog.btrim(p_snapshot->>'stable_key');
    v_title TEXT := pg_catalog.btrim(p_snapshot->>'title');
    v_backing_kind TEXT := p_snapshot->>'backing_kind';
    v_occurred_at TIMESTAMPTZ := pg_catalog.statement_timestamp();
BEGIN
    PERFORM ontology_api.assert_write_context(p_org_id, p_actor, p_trace_id, p_span_id);
    IF pg_catalog.jsonb_typeof(p_snapshot) <> 'object' THEN
        RAISE EXCEPTION USING ERRCODE = '22023', MESSAGE = 'ontology_write.invalid_snapshot_shape';
    END IF;

    -- Serialize the only two paths that can create a tenant's first registry
    -- row. The installer must never pass its empty-org check concurrently with
    -- an ordinary create, regardless of catalog version.
    PERFORM pg_catalog.pg_advisory_xact_lock(
        pg_catalog.hashtextextended('ontology-bootstrap:' || p_org_id::TEXT, 0)
    );

    INSERT INTO public.ont_object_type_key_revisions (org_id, stable_key)
    VALUES (p_org_id, v_stable_key)
    RETURNING ont_object_type_key_revisions.validator_id INTO v_validator;

    INSERT INTO public.ont_object_types
        (id, org_id, stable_key, title, title_property_key, backing_kind,
         backing_table, primary_key_property, schema_version, lifecycle_state,
         created_by, created_at, updated_at)
    VALUES
        (v_id, p_org_id, v_stable_key, v_title, NULLIF(p_snapshot->>'title_property_key', ''),
         v_backing_kind, NULLIF(p_snapshot->>'backing_table', ''),
         NULLIF(p_snapshot->>'primary_key_property', ''), 1, 'draft', p_actor,
         v_occurred_at, v_occurred_at);

    PERFORM ontology_api.insert_children(p_org_id, v_id, p_snapshot, FALSE);
    PERFORM ontology_api.write_audit(
        p_org_id, p_actor, 'ontology.object_type.create', v_id, NULL,
        pg_catalog.jsonb_build_object('stable_key', v_stable_key, 'schema_version', 1, 'lifecycle_state', 'draft'),
        p_trace_id, p_span_id, v_occurred_at);
    RETURN QUERY SELECT v_id, v_stable_key, v_title, v_backing_kind,
                        1::BIGINT, 'draft'::TEXT, v_validator, 1::BIGINT;
END;
$$;

CREATE FUNCTION ontology_api.stage_object_type(
    p_org_id UUID,
    p_stable_key TEXT,
    p_expected_validator UUID,
    p_expected_revision BIGINT,
    p_snapshot JSONB,
    p_actor UUID,
    p_trace_id TEXT,
    p_span_id TEXT
)
RETURNS TABLE(
    object_type_id UUID,
    stable_key TEXT,
    title TEXT,
    backing_kind TEXT,
    schema_version BIGINT,
    lifecycle_state TEXT,
    key_write_validator_id UUID,
    key_write_revision BIGINT
)
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog
SET row_security = on
AS $$
DECLARE
    v_id UUID;
    v_validator UUID;
    v_revision BIGINT;
    v_schema_version BIGINT;
    v_state TEXT;
    v_snapshot_key TEXT := pg_catalog.btrim(p_snapshot->>'stable_key');
    v_occurred_at TIMESTAMPTZ := pg_catalog.statement_timestamp();
BEGIN
    PERFORM ontology_api.assert_write_context(p_org_id, p_actor, p_trace_id, p_span_id);
    IF v_snapshot_key IS DISTINCT FROM p_stable_key THEN
        RAISE EXCEPTION USING ERRCODE = '22023', MESSAGE = 'ontology_write.stable_key_mismatch';
    END IF;

    SELECT k.validator_id, k.revision INTO v_validator, v_revision
      FROM public.ont_object_type_key_revisions k
     WHERE k.org_id = p_org_id AND k.stable_key = p_stable_key
     FOR UPDATE;
    IF NOT FOUND THEN
        RAISE EXCEPTION USING ERRCODE = 'P0002', MESSAGE = 'ontology_write.key_not_found';
    END IF;
    IF v_validator <> p_expected_validator OR v_revision <> p_expected_revision THEN
        RETURN;
    END IF;

    SELECT o.id, o.schema_version, o.lifecycle_state
      INTO v_id, v_schema_version, v_state
      FROM public.ont_object_types o
     WHERE o.org_id = p_org_id AND o.stable_key = p_stable_key
       AND o.lifecycle_state IN ('draft', 'review_pending')
     FOR UPDATE;

    IF FOUND THEN
        IF v_state <> 'draft' THEN
            RAISE EXCEPTION USING ERRCODE = '23514', MESSAGE = 'ontology_write.review_pending_immutable';
        END IF;
        UPDATE public.ont_object_types o
           SET title = pg_catalog.btrim(p_snapshot->>'title'),
               title_property_key = NULLIF(p_snapshot->>'title_property_key', ''),
               backing_kind = p_snapshot->>'backing_kind',
               backing_table = NULLIF(p_snapshot->>'backing_table', ''),
               primary_key_property = NULLIF(p_snapshot->>'primary_key_property', ''),
               updated_at = v_occurred_at
         WHERE o.id = v_id AND o.org_id = p_org_id AND o.lifecycle_state = 'draft';
        PERFORM ontology_api.insert_children(p_org_id, v_id, p_snapshot, TRUE);
    ELSE
        SELECT pg_catalog.max(o.schema_version) + 1 INTO v_schema_version
          FROM public.ont_object_types o
         WHERE o.org_id = p_org_id AND o.stable_key = p_stable_key;
        IF v_schema_version IS NULL THEN
            RAISE EXCEPTION USING ERRCODE = 'P0002', MESSAGE = 'ontology_write.key_not_found';
        END IF;
        v_id := public.gen_random_uuid();
        INSERT INTO public.ont_object_types
            (id, org_id, stable_key, title, title_property_key, backing_kind,
             backing_table, primary_key_property, schema_version, lifecycle_state,
             created_by, created_at, updated_at)
        VALUES
            (v_id, p_org_id, p_stable_key, pg_catalog.btrim(p_snapshot->>'title'),
             NULLIF(p_snapshot->>'title_property_key', ''), p_snapshot->>'backing_kind',
             NULLIF(p_snapshot->>'backing_table', ''), NULLIF(p_snapshot->>'primary_key_property', ''),
             v_schema_version, 'draft', p_actor, v_occurred_at, v_occurred_at);
        PERFORM ontology_api.insert_children(p_org_id, v_id, p_snapshot, FALSE);
    END IF;

    UPDATE public.ont_object_type_key_revisions k
       SET revision = k.revision + 1, updated_at = v_occurred_at
     WHERE k.org_id = p_org_id AND k.stable_key = p_stable_key
    RETURNING k.revision INTO v_revision;

    PERFORM ontology_api.write_audit(
        p_org_id, p_actor, 'ontology.object_type.stage_revision', v_id, NULL,
        pg_catalog.jsonb_build_object('stable_key', p_stable_key,
                                      'schema_version', v_schema_version,
                                      'lifecycle_state', 'draft'),
        p_trace_id, p_span_id, v_occurred_at);
    RETURN QUERY SELECT v_id, p_stable_key, pg_catalog.btrim(p_snapshot->>'title'),
                        p_snapshot->>'backing_kind', v_schema_version, 'draft'::TEXT,
                        v_validator, v_revision;
END;
$$;

CREATE FUNCTION ontology_api.transition_object_type(
    p_org_id UUID,
    p_object_type_id UUID,
    p_expected_validator UUID,
    p_expected_revision BIGINT,
    p_to_state TEXT,
    p_actor UUID,
    p_trace_id TEXT,
    p_span_id TEXT
)
RETURNS TABLE(
    object_type_id UUID,
    stable_key TEXT,
    title TEXT,
    backing_kind TEXT,
    schema_version BIGINT,
    lifecycle_state TEXT,
    key_write_validator_id UUID,
    key_write_revision BIGINT
)
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog
SET row_security = on
AS $$
DECLARE
    v_stable_key TEXT;
    v_title TEXT;
    v_from_state TEXT;
    v_backing_kind TEXT;
    v_schema_version BIGINT;
    v_validator UUID;
    v_revision BIGINT;
    v_approval_id UUID;
    v_params JSONB;
    v_edits JSONB;
    v_occurred_at TIMESTAMPTZ := pg_catalog.statement_timestamp();
BEGIN
    PERFORM ontology_api.assert_write_context(p_org_id, p_actor, p_trace_id, p_span_id);

    SELECT o.stable_key, o.title, o.lifecycle_state, o.backing_kind, o.schema_version
      INTO v_stable_key, v_title, v_from_state, v_backing_kind, v_schema_version
      FROM public.ont_object_types o
     WHERE o.id = p_object_type_id AND o.org_id = p_org_id;
    IF NOT FOUND THEN
        RAISE EXCEPTION USING ERRCODE = 'P0002', MESSAGE = 'ontology_write.object_type_not_found';
    END IF;

    SELECT k.validator_id, k.revision INTO v_validator, v_revision
      FROM public.ont_object_type_key_revisions k
     WHERE k.org_id = p_org_id AND k.stable_key = v_stable_key
     FOR UPDATE;
    IF v_validator <> p_expected_validator OR v_revision <> p_expected_revision THEN
        RETURN;
    END IF;

    PERFORM 1 FROM public.ont_object_types o
     WHERE o.id = p_object_type_id AND o.org_id = p_org_id
       AND o.lifecycle_state = v_from_state
     FOR UPDATE;
    IF NOT FOUND THEN
        RAISE EXCEPTION USING ERRCODE = '40001', MESSAGE = 'ontology_write.lifecycle_changed';
    END IF;

    IF v_from_state = 'draft' AND p_to_state = 'published' THEN
        RAISE EXCEPTION USING ERRCODE = '23514', MESSAGE = 'ontology_write.review_required';
    ELSIF v_from_state = 'review_pending' AND p_to_state = 'published' THEN
        SELECT ga.id INTO v_approval_id
          FROM public.gov_approvals ga
          JOIN public.gov_approval_requests gr
            ON gr.org_id = ga.org_id AND gr.request_ref = ga.request_ref
           AND gr.kind = ga.kind AND gr.target_ref = ga.target_ref
           AND gr.requested_by = ga.requested_by
          LEFT JOIN public.gov_approval_consumptions gc
            ON gc.org_id = ga.org_id AND gc.approval_id = ga.id
         WHERE ga.org_id = p_org_id
           AND ga.kind = 'ontology.schema.publish'
           AND ga.target_ref = p_object_type_id
           AND ga.requested_by = p_actor
           AND ga.decision = 'approved'
           AND (gr.payload_summary->>'key_revision')::BIGINT = v_revision
           AND gc.id IS NULL
         ORDER BY ga.decided_at DESC, ga.id
         LIMIT 1
         FOR UPDATE OF ga;
        IF v_approval_id IS NULL THEN
            RAISE EXCEPTION USING ERRCODE = '42501', MESSAGE = 'ontology_write.publish_approval_required';
        END IF;
        INSERT INTO public.gov_approval_consumptions
            (id, org_id, approval_id, consumed_by, consumed_at)
        VALUES
            (public.gen_random_uuid(), p_org_id, v_approval_id, p_actor, v_occurred_at);
    ELSIF NOT (
        (v_from_state = 'draft' AND p_to_state = 'review_pending')
        OR (v_from_state = 'review_pending' AND p_to_state = 'draft')
        OR (v_from_state = 'published' AND p_to_state IN ('superseded', 'retired'))
        OR (v_from_state = 'superseded' AND p_to_state = 'retired')
    ) THEN
        RAISE EXCEPTION USING ERRCODE = '23514', MESSAGE = 'ontology_write.illegal_lifecycle_transition';
    END IF;

    IF p_to_state = 'published' AND v_backing_kind = 'instance'
       AND NOT EXISTS (
           SELECT 1 FROM public.ont_action_types a
           WHERE a.org_id = p_org_id AND a.object_type_id = p_object_type_id
             AND a.dispatch = 'instance_revision'
       ) THEN
        SELECT COALESCE(pg_catalog.jsonb_object_agg(p.key, pg_catalog.jsonb_build_object('required', p.required)), '{}'::JSONB),
               COALESCE(pg_catalog.jsonb_agg(pg_catalog.jsonb_build_object('property', p.key, 'param', p.key) ORDER BY p.key), '[]'::JSONB)
          INTO v_params, v_edits
          FROM public.ont_property_defs p
         WHERE p.org_id = p_org_id AND p.object_type_id = p_object_type_id;
        INSERT INTO public.ont_action_types
            (id, org_id, object_type_id, stable_key, title, params_schema, edits,
             submission_criteria, side_effects, dispatch, dispatch_target, control_points)
        VALUES
            (public.gen_random_uuid(), p_org_id, p_object_type_id, 'create', '저장',
             v_params, v_edits, '[]'::JSONB, '[]'::JSONB, 'instance_revision', NULL,
             '["authority"]'::JSONB);
    END IF;

    IF p_to_state = 'published' THEN
        UPDATE public.ont_object_types o
           SET lifecycle_state = 'superseded', updated_at = v_occurred_at
         WHERE o.org_id = p_org_id AND o.stable_key = v_stable_key
           AND o.lifecycle_state = 'published' AND o.id <> p_object_type_id;
    END IF;
    UPDATE public.ont_object_types o
       SET lifecycle_state = p_to_state, updated_at = v_occurred_at
     WHERE o.id = p_object_type_id AND o.org_id = p_org_id
       AND o.lifecycle_state = v_from_state;
    IF NOT FOUND THEN
        RAISE EXCEPTION USING ERRCODE = '40001', MESSAGE = 'ontology_write.lifecycle_changed';
    END IF;

    UPDATE public.ont_object_type_key_revisions k
       SET revision = k.revision + 1, updated_at = v_occurred_at
     WHERE k.org_id = p_org_id AND k.stable_key = v_stable_key
    RETURNING k.revision INTO v_revision;

    PERFORM ontology_api.write_audit(
        p_org_id, p_actor, 'ontology.object_type.transition', p_object_type_id,
        pg_catalog.jsonb_build_object('stable_key', v_stable_key,
                                      'schema_version', v_schema_version,
                                      'lifecycle_state', v_from_state),
        pg_catalog.jsonb_build_object('stable_key', v_stable_key,
                                      'schema_version', v_schema_version,
                                      'lifecycle_state', p_to_state),
        p_trace_id, p_span_id, v_occurred_at);
    RETURN QUERY SELECT p_object_type_id, v_stable_key, v_title, v_backing_kind,
                        v_schema_version, p_to_state, v_validator, v_revision;
END;
$$;

CREATE FUNCTION ontology_api.install_builtin_catalog(
    p_org_id UUID,
    p_catalog_version TEXT,
    p_manifest JSONB,
    p_actor UUID,
    p_trace_id TEXT,
    p_span_id TEXT
)
RETURNS TABLE(installed BOOLEAN, object_type_count BIGINT)
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog
SET row_security = on
AS $$
DECLARE
    v_digest BYTEA;
    v_allowed_digest BYTEA;
    v_existing_version TEXT;
    v_existing_digest BYTEA;
    v_snapshot JSONB;
    v_link JSONB;
    v_links JSONB;
    v_stable_key TEXT;
    v_target_key TEXT;
    v_target_id UUID;
    v_id UUID;
    v_count BIGINT;
    v_occurred_at TIMESTAMPTZ := pg_catalog.statement_timestamp();
BEGIN
    PERFORM ontology_api.assert_write_context(p_org_id, p_actor, p_trace_id, p_span_id);
    IF pg_catalog.jsonb_typeof(p_manifest) <> 'object'
       OR p_manifest->>'catalog_version' IS DISTINCT FROM p_catalog_version
       OR pg_catalog.jsonb_typeof(p_manifest->'object_types') <> 'array' THEN
        RAISE EXCEPTION USING ERRCODE = '22023', MESSAGE = 'ontology_builtin.invalid_manifest_shape';
    END IF;

    v_digest := public.digest(pg_catalog.convert_to(p_manifest::TEXT, 'UTF8'), 'sha256');
    SELECT a.manifest_digest INTO v_allowed_digest
      FROM public.ont_builtin_catalog_allowlist a
     WHERE a.catalog_version = p_catalog_version;
    IF v_allowed_digest IS NULL OR v_allowed_digest <> v_digest THEN
        RAISE EXCEPTION USING ERRCODE = '42501', MESSAGE = 'ontology_builtin.manifest_not_allowlisted';
    END IF;

    -- Share the org-scoped bootstrap/write lock with ordinary creation. The
    -- lock intentionally excludes catalog version so future versions cannot
    -- race one another or race a first custom type.
    PERFORM pg_catalog.pg_advisory_xact_lock(
        pg_catalog.hashtextextended('ontology-bootstrap:' || p_org_id::TEXT, 0)
    );

    SELECT i.catalog_version, i.manifest_digest
      INTO v_existing_version, v_existing_digest
      FROM public.ont_builtin_catalog_installs i
     WHERE i.org_id = p_org_id;
    IF FOUND THEN
        IF v_existing_version = p_catalog_version AND v_existing_digest = v_digest THEN
            v_count := pg_catalog.jsonb_array_length(p_manifest->'object_types')::BIGINT;
            RETURN QUERY SELECT FALSE, v_count;
            RETURN;
        END IF;
        RAISE EXCEPTION USING ERRCODE = '23505', MESSAGE = 'ontology_builtin.different_catalog_already_installed';
    END IF;
    IF EXISTS (SELECT 1 FROM public.ont_object_types o WHERE o.org_id = p_org_id) THEN
        RAISE EXCEPTION USING ERRCODE = '23514', MESSAGE = 'ontology_builtin.empty_org_required';
    END IF;

    -- Pass 1 creates every type and all non-link children. IDs are generated in
    -- the database, and every built-in starts published without exposing a
    -- general draft->published capability.
    FOR v_snapshot IN
        SELECT value FROM pg_catalog.jsonb_array_elements(p_manifest->'object_types')
    LOOP
        v_stable_key := pg_catalog.btrim(v_snapshot->>'stable_key');
        IF EXISTS (
            SELECT 1 FROM pg_catalog.jsonb_array_elements(COALESCE(v_snapshot->'links', '[]'::JSONB)) l
            WHERE l ? 'to_object_type_id' AND l->>'to_object_type_id' IS NOT NULL
        ) THEN
            RAISE EXCEPTION USING ERRCODE = '22023', MESSAGE = 'ontology_builtin.physical_link_id_forbidden';
        END IF;
        v_id := public.gen_random_uuid();
        INSERT INTO public.ont_object_type_key_revisions (org_id, stable_key)
        VALUES (p_org_id, v_stable_key);
        INSERT INTO public.ont_object_types
            (id, org_id, stable_key, title, title_property_key, backing_kind,
             backing_table, primary_key_property, schema_version, lifecycle_state,
             created_by, created_at, updated_at)
        VALUES
            (v_id, p_org_id, v_stable_key, pg_catalog.btrim(v_snapshot->>'title'),
             NULLIF(v_snapshot->>'title_property_key', ''), v_snapshot->>'backing_kind',
             NULLIF(v_snapshot->>'backing_table', ''), NULLIF(v_snapshot->>'primary_key_property', ''),
             1, 'published', p_actor, v_occurred_at, v_occurred_at);
        PERFORM ontology_api.insert_children(
            p_org_id, v_id, pg_catalog.jsonb_set(v_snapshot, '{links}', '[]'::JSONB, TRUE), FALSE);
        PERFORM ontology_api.write_audit(
            p_org_id, p_actor, 'ontology.object_type.builtin_install', v_id, NULL,
            pg_catalog.jsonb_build_object('stable_key', v_stable_key,
                                          'schema_version', 1,
                                          'lifecycle_state', 'published',
                                          'catalog_version', p_catalog_version,
                                          'manifest_digest', pg_catalog.encode(v_digest, 'hex')),
            p_trace_id, p_span_id, v_occurred_at);
    END LOOP;

    -- Pass 2 resolves logical link targets only against this tenant's freshly
    -- installed catalog, then enters the same private child validator.
    FOR v_snapshot IN
        SELECT value FROM pg_catalog.jsonb_array_elements(p_manifest->'object_types')
    LOOP
        v_stable_key := pg_catalog.btrim(v_snapshot->>'stable_key');
        SELECT o.id INTO v_id
          FROM public.ont_object_types o
         WHERE o.org_id = p_org_id AND o.stable_key = v_stable_key AND o.schema_version = 1;
        v_links := '[]'::JSONB;
        FOR v_link IN
            SELECT value FROM pg_catalog.jsonb_array_elements(COALESCE(v_snapshot->'links', '[]'::JSONB))
        LOOP
            v_target_key := NULLIF(pg_catalog.btrim(v_link->>'to_stable_key'), '');
            v_target_id := NULL;
            IF v_target_key IS NOT NULL THEN
                SELECT o.id INTO v_target_id
                  FROM public.ont_object_types o
                 WHERE o.org_id = p_org_id AND o.stable_key = v_target_key AND o.schema_version = 1;
                IF v_target_id IS NULL THEN
                    RAISE EXCEPTION USING ERRCODE = '23503', MESSAGE = 'ontology_builtin.link_target_not_found';
                END IF;
            END IF;
            v_links := v_links || pg_catalog.jsonb_build_array(
                (v_link - 'to_stable_key' - 'to_object_type_id')
                || pg_catalog.jsonb_build_object('to_object_type_id', v_target_id)
            );
        END LOOP;
        PERFORM ontology_api.insert_children(
            p_org_id, v_id, pg_catalog.jsonb_set(v_snapshot, '{links}', v_links, TRUE), TRUE);
    END LOOP;

    SELECT pg_catalog.jsonb_array_length(p_manifest->'object_types')::BIGINT INTO v_count;
    INSERT INTO public.ont_builtin_catalog_installs
        (org_id, catalog_version, manifest_digest, installed_by, installed_at)
    VALUES
        (p_org_id, p_catalog_version, v_digest, p_actor, v_occurred_at);
    RETURN QUERY SELECT TRUE, v_count;
END;
$$;

-- Pin every definer routine to the restricted capability role. Helpers remain
-- private; only the complete mutation+audit entrypoints are executable by console_rt.
ALTER FUNCTION ontology_api.assert_write_context(UUID, UUID, TEXT, TEXT) OWNER TO console_ontology_writer;
ALTER FUNCTION ontology_api.insert_children(UUID, UUID, JSONB, BOOLEAN) OWNER TO console_ontology_writer;
ALTER FUNCTION ontology_api.write_audit(UUID, UUID, TEXT, UUID, JSONB, JSONB, TEXT, TEXT, TIMESTAMPTZ) OWNER TO console_ontology_writer;
ALTER FUNCTION ontology_api.create_object_type(UUID, JSONB, UUID, TEXT, TEXT) OWNER TO console_ontology_writer;
ALTER FUNCTION ontology_api.stage_object_type(UUID, TEXT, UUID, BIGINT, JSONB, UUID, TEXT, TEXT) OWNER TO console_ontology_writer;
ALTER FUNCTION ontology_api.transition_object_type(UUID, UUID, UUID, BIGINT, TEXT, UUID, TEXT, TEXT) OWNER TO console_ontology_writer;
ALTER FUNCTION ontology_api.install_builtin_catalog(UUID, TEXT, JSONB, UUID, TEXT, TEXT) OWNER TO console_ontology_writer;

REVOKE ALL ON ALL FUNCTIONS IN SCHEMA ontology_api FROM PUBLIC, console_rt, console_ontology_cmd;
GRANT EXECUTE ON FUNCTION ontology_api.create_object_type(UUID, JSONB, UUID, TEXT, TEXT) TO console_ontology_cmd;
GRANT EXECUTE ON FUNCTION ontology_api.stage_object_type(UUID, TEXT, UUID, BIGINT, JSONB, UUID, TEXT, TEXT) TO console_ontology_cmd;
GRANT EXECUTE ON FUNCTION ontology_api.transition_object_type(UUID, UUID, UUID, BIGINT, TEXT, UUID, TEXT, TEXT) TO console_ontology_cmd;
GRANT EXECUTE ON FUNCTION ontology_api.install_builtin_catalog(UUID, TEXT, JSONB, UUID, TEXT, TEXT) TO console_ontology_cmd;
