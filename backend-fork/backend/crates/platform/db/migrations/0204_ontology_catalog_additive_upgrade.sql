-- Additive upgrade path for the digest-allowlisted built-in ontology catalog.
--
-- 0165 gave the catalog installer two fail-closed guards and no way past them:
-- a tenant that had installed catalog version A could never receive version B
-- (`ontology_builtin.different_catalog_already_installed`), so every seeded
-- environment was frozen at the key set of its first install. Adding a 28th
-- built-in object type was therefore impossible for any live tenant.
--
-- This migration replaces the installer body with an ADDITIVE one. The digest
-- chain is unchanged and still the sole authority: a manifest is accepted only
-- when its canonical JSONB sha256 equals the migration-owned allowlist row for
-- the presented catalog version. What changes is what the installer does once
-- the manifest is trusted:
--
--   * keys the tenant does not hold are created exactly as before (published
--     schema_version 1, full child snapshot, one audit row each);
--   * keys the tenant already holds are LEFT ALONE — never updated, never
--     restaged, never renumbered, never republished;
--   * the pass-2 link resolver binds a new type's logical targets to the
--     tenant's PUBLISHED head for that key, so a new type may link to a type
--     installed by an earlier catalog version;
--   * install markers become append-only history — one row per (org, version) —
--     so "already applied" is decided by the recorded (version, digest) pair and
--     re-application of any recorded version stays a no-op.
--
-- The pristine-tenant guard is deliberately KEPT for a tenant's FIRST catalog
-- install: bootstrap must still never interleave with hand-authored types
-- (`ontology_builtin.empty_org_required`). It is only once a tenant is on the
-- catalog that later versions install additively.
--
-- Retaining a key unexamined would let a hand-authored type silently stand in
-- for a built-in of the same name, which for a `projected` type means reads
-- against a different physical table. So the one thing a retained key may not
-- do is contradict the manifest's projection contract
-- (`backing_kind`/`backing_table`/`primary_key_property`): that fails closed
-- with `ontology_builtin.existing_key_projection_conflict` and the whole
-- install rolls back.

-- Install markers become an append-only per-tenant history of applied catalog
-- versions. console_ontology_writer keeps SELECT+INSERT and still has no UPDATE:
-- an upgrade appends, it does not rewrite the tenant's install record.
ALTER TABLE ont_builtin_catalog_installs
    DROP CONSTRAINT ont_builtin_catalog_installs_pkey;
ALTER TABLE ont_builtin_catalog_installs
    ADD CONSTRAINT ont_builtin_catalog_installs_pkey PRIMARY KEY (org_id, catalog_version);

CREATE OR REPLACE FUNCTION ontology_api.install_builtin_catalog(
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
    v_snapshot JSONB;
    v_link JSONB;
    v_links JSONB;
    v_stable_key TEXT;
    v_target_key TEXT;
    v_target_id UUID;
    v_id UUID;
    v_count BIGINT;
    v_upgrade BOOLEAN;
    v_new_keys TEXT[] := ARRAY[]::TEXT[];
    v_retained_keys TEXT[] := ARRAY[]::TEXT[];
    v_existing_backing_kind TEXT;
    v_existing_backing_table TEXT;
    v_existing_primary_key TEXT;
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
    -- lock intentionally excludes catalog version so two versions cannot race
    -- one another or race a first custom type.
    PERFORM pg_catalog.pg_advisory_xact_lock(
        pg_catalog.hashtextextended('ontology-bootstrap:' || p_org_id::TEXT, 0)
    );

    v_count := pg_catalog.jsonb_array_length(p_manifest->'object_types')::BIGINT;

    -- Exact re-application of a version this tenant already recorded is a
    -- database-owned no-op, which is what makes install idempotent under retry.
    IF EXISTS (
        SELECT 1 FROM public.ont_builtin_catalog_installs i
         WHERE i.org_id = p_org_id
           AND i.catalog_version = p_catalog_version
           AND i.manifest_digest = v_digest
    ) THEN
        RETURN QUERY SELECT FALSE, v_count;
        RETURN;
    END IF;

    v_upgrade := EXISTS (
        SELECT 1 FROM public.ont_builtin_catalog_installs i WHERE i.org_id = p_org_id
    );
    IF NOT v_upgrade AND EXISTS (
        SELECT 1 FROM public.ont_object_types o WHERE o.org_id = p_org_id
    ) THEN
        RAISE EXCEPTION USING ERRCODE = '23514', MESSAGE = 'ontology_builtin.empty_org_required';
    END IF;

    -- Pass 1 creates every key this tenant does not already hold, plus that
    -- key's non-link children. IDs are generated in the database, and every
    -- built-in starts published without exposing a general draft->published
    -- capability. Keys the tenant already holds are recorded as retained and
    -- otherwise untouched.
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

        SELECT o.backing_kind, o.backing_table, o.primary_key_property
          INTO v_existing_backing_kind, v_existing_backing_table, v_existing_primary_key
          FROM public.ont_object_types o
         WHERE o.org_id = p_org_id AND o.stable_key = v_stable_key
         ORDER BY o.schema_version DESC
         LIMIT 1;
        IF FOUND THEN
            IF v_existing_backing_kind IS DISTINCT FROM v_snapshot->>'backing_kind'
               OR v_existing_backing_table IS DISTINCT FROM NULLIF(v_snapshot->>'backing_table', '')
               OR v_existing_primary_key IS DISTINCT FROM NULLIF(v_snapshot->>'primary_key_property', '') THEN
                RAISE EXCEPTION USING ERRCODE = '23514',
                    MESSAGE = 'ontology_builtin.existing_key_projection_conflict';
            END IF;
            v_retained_keys := v_retained_keys || v_stable_key;
            CONTINUE;
        END IF;

        v_new_keys := v_new_keys || v_stable_key;
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

    -- Pass 2 resolves logical link targets for the newly installed types only,
    -- against this tenant's published heads — which may have arrived with an
    -- earlier catalog version — then enters the same private child validator.
    -- Types the tenant already held gain no links: retained means untouched.
    FOR v_snapshot IN
        SELECT value FROM pg_catalog.jsonb_array_elements(p_manifest->'object_types')
    LOOP
        v_stable_key := pg_catalog.btrim(v_snapshot->>'stable_key');
        IF NOT (v_stable_key = ANY (v_new_keys)) THEN
            CONTINUE;
        END IF;
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
                 WHERE o.org_id = p_org_id
                   AND o.stable_key = v_target_key
                   AND o.lifecycle_state = 'published';
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

    INSERT INTO public.ont_builtin_catalog_installs
        (org_id, catalog_version, manifest_digest, installed_by, installed_at)
    VALUES
        (p_org_id, p_catalog_version, v_digest, p_actor, v_occurred_at);

    -- The marker append is itself a governed state change — on an upgrade that
    -- adds no key it is the ONLY change — and the retained-key list is the
    -- record of what the installer deliberately did not touch. Neither action
    -- name is in the protected set enforced by
    -- ontology_api.protected_audit_writer_guard, and neither satisfies the
    -- per-mutation audit requirement: the per-type builtin_install rows above
    -- still do that on their own.
    INSERT INTO public.audit_events
        (id, actor, action, target_type, target_id, branch_id, before_snap, after_snap,
         trace_id, span_id, occurred_at, org_id)
    VALUES
        (public.gen_random_uuid(), p_actor,
         CASE WHEN v_upgrade THEN 'ontology.builtin_catalog.upgrade'
              ELSE 'ontology.builtin_catalog.install' END,
         'ont_builtin_catalog_installs', p_org_id::TEXT, NULL, NULL,
         pg_catalog.jsonb_build_object('catalog_version', p_catalog_version,
                                       'manifest_digest', pg_catalog.encode(v_digest, 'hex'),
                                       'installed_keys', pg_catalog.to_jsonb(v_new_keys),
                                       'retained_keys', pg_catalog.to_jsonb(v_retained_keys)),
         p_trace_id, p_span_id, v_occurred_at, p_org_id);

    RETURN QUERY SELECT TRUE, v_count;
END;
$$;

-- CREATE OR REPLACE preserves ownership and ACL; re-assert both so the
-- capability pinning of 0165 is readable at this migration too.
ALTER FUNCTION ontology_api.install_builtin_catalog(UUID, TEXT, JSONB, UUID, TEXT, TEXT)
    OWNER TO console_ontology_writer;
REVOKE ALL ON FUNCTION ontology_api.install_builtin_catalog(UUID, TEXT, JSONB, UUID, TEXT, TEXT)
    FROM PUBLIC, console_rt;
GRANT EXECUTE ON FUNCTION ontology_api.install_builtin_catalog(UUID, TEXT, JSONB, UUID, TEXT, TEXT)
    TO console_ontology_cmd;
