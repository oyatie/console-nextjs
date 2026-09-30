-- Audited object-policy attachment: the DB-owned vehicle that lets an org author
-- one enforced catalog policy and attach it to one object type, without ever
-- giving the general runtime role INSERT on the policy catalog.
--
-- `console_rt` keeps exactly the capability 0150:117-118 gave it (SELECT on
-- cedar_policy_catalog_entries, no INSERT). The catalog write happens inside a
-- SECURITY DEFINER routine owned by `console_ontology_writer`, which is NOLOGIN,
-- NOSUPERUSER and — decisively — NOBYPASSRLS
-- (ops/postgres-reconcile-topology.sh:382-384, re-asserted at :567). The org
-- floor therefore still applies inside the definer: it may only narrow.
--
-- THE DEFINER IS A SECURITY BOUNDARY IN ITS OWN RIGHT, NOT ONLY THE ROUTE. It is
-- EXECUTE-granted to a database login, so anyone holding that credential calls it
-- directly and skips every check `attach_object_policy`
-- (`ontology/rest/src/lib.rs`) performs. 0206 narrows WHICH login (see the note
-- below §4's grants) but does not remove the property: `console_ontology_cmd` is
-- still a login the application holds. Two shapes of answer are applied in §4:
--
--   * DELETION over validation. `stable_key`, `title`, `natural_language_rule`
--     and the bundle digest are all DERIVABLE, so they are no longer accepted —
--     the routine generates them. You cannot forge what you cannot supply.
--   * an envelope check on everything that remains checkable in SQL.
--
-- `generated_policy_text` is deleted for the OPPOSITE reason: it is the one
-- parameter that is neither derivable in SQL (rendering Cedar here would be a
-- second copy of `generate_cedar_text_with` living in a migration) nor bounded by
-- any predicate (a condition literal may legitimately contain `;`). So it is not
-- accepted at all — the routine stores NULL. Nothing needs it: object-policy
-- ENFORCEMENT is `load_enforced_object_policy_blocks`, which re-derives
-- everything from `normalized_row` and never reads the text. Both consumers that
-- DO read it — `load_enforced_policies` and `OBJECT_POLICY_SELECT`, the latter
-- behind `POST /policy/authorize` with an `object_type_id` — already filter
-- `generated_policy_text IS NOT NULL`, so a definer call cannot choose the Cedar
-- source of any decision. Measured before this: a hand-crafted call carrying
-- `permit(principal, action == Action::"view", resource);` made
-- `authorize_object_row` return `Allow` for a principal that owned nothing.
--
-- ACCEPTED CONSEQUENCE, stated rather than hidden: `POST /policy/authorize` with
-- an `object_type_id` now denies for policies authored through the attach route.
-- Restoring that what-if would mean re-deriving the text from `normalized_row`,
-- which needs the object type's declared properties (`generate_cedar_text_with`'s
-- second argument) — owned by the ontology crate, not by the policy store. Deny
-- is the fail-closed side of that gap, and row visibility never depended on it.
--
-- WHAT STAYS ROUTE-ONLY, and why that is acceptable: the HTTP principal
-- (`authorize_ontology`, no SQL equivalent) and Cedar's strict validator verdict
-- (`authoring::validate_blocks_with` — re-implementing it in SQL would be a
-- second copy of the validator living in a migration, which is the divergence
-- that rots).
--
-- THE AUDIT ROW IS NOT IN THAT LIST. 0206 renames this routine to
-- `attach_object_policy_rows`, keeps it owner-only, and puts an audited
-- entrypoint in front of it; the credential that may attach cannot reach this
-- body at all, so it cannot attach untraced. The residual this migration
-- escalated — an attachment with no audit event — is discharged there, not here.
--
-- The residual capability that remains is therefore only "a row coherent in
-- every checkable respect that Cedar's validator would nonetheless reject",
-- which is now always accompanied by an audit event naming who attached it.
--
-- >> LIVE CONSTRAINT, not a general safety claim: that remaining residual is
-- >> bounded by `PgCedarPolicyStore::load_enforced_object_policy_blocks`
-- >> (`platform/authz-rest/src/store.rs`), whose FOUR arms — the stored row
-- >> deserializes, the validator verdict, the canonicality comparison, and the
-- >> effect agreement across blocks, catalog row and attachment — all run on EVERY
-- >> read and error the whole load. It is DEFENCE IN DEPTH after 0206 rather than
-- >> the sole justification it was before, and it stays: 0206 narrowed who may
-- >> mint a non-canonical row, it did not make one readable. Delete any one of the
-- >> four and a forged row is served.
-- >>
-- >> All four are now EXECUTED, which two of them were not when this paragraph
-- >> was written: the deserialization and effect-agreement arms measured ZERO with
-- >> the whole suite green, so either could have been deleted without a red test.
-- >> They are unreachable through the definer and reachable by every other writer
-- >> of these two tables, and
-- >> `a_catalog_row_whose_blocks_disagree_with_its_effect_is_refused_on_every_read`
-- >> plus `a_catalog_row_whose_normalized_row_is_unparseable_is_refused_on_every_read`
-- >> are what make this paragraph true rather than intended.
--
-- DISCLOSURE (owner reuse): `console_ontology_writer` also owns the eleven
-- `ontology_api` routines, so those inherit the grants below: INSERT on the
-- policy catalog and its attachment table, and SELECT on `ont_object_types`
-- (§4 resolves the attached type's `stable_key` there). Their bodies are
-- migration-fixed and none of them touches any of the three, so the reachable
-- capability set is unchanged. `ont_object_types` is FORCE-RLS org-scoped
-- (0152:122-144), so the added SELECT reaches nothing outside the armed org.
-- `console_app` is DISQUALIFIED as the owner
-- because it is `rolbypassrls = t`: an implicitly-owned definer would silently
-- evaporate the org floor while every test stayed green (0196:155-158 is the
-- seductive precedent — do not copy it).
--
-- No `CREATE ROLE` anywhere: cluster-global roles are infrastructure-owned and
-- this migration fails closed on any drift (0165:4-8, 0165:18-23).
--
-- No routine is added to the `ontology_api` schema: its routine count and
-- namespace ACL are pinned by
-- ontology/adapter-postgres/tests/key_revision_migration_upgrade.rs:571,681-688.

-- ---------------------------------------------------------------------------
-- 1. Preconditions. Modelled on 0166:277-314.
-- ---------------------------------------------------------------------------
DO $$
DECLARE
    v_migrator OID := pg_catalog.to_regrole('console_app');
    v_runtime OID := pg_catalog.to_regrole('console_rt');
    v_writer OID := pg_catalog.to_regrole('console_ontology_writer');
    v_applier_is_superuser BOOLEAN;
BEGIN
    IF v_migrator IS NULL OR v_runtime IS NULL OR v_writer IS NULL THEN
        RAISE EXCEPTION 'object-policy writer precondition failed: roles are not preprovisioned';
    END IF;
    SELECT rolsuper INTO v_applier_is_superuser
      FROM pg_catalog.pg_roles WHERE rolname = CURRENT_USER;
    IF NOT v_applier_is_superuser
       AND (CURRENT_USER <> 'console_app' OR SESSION_USER <> 'console_app') THEN
        RAISE EXCEPTION 'object-policy writer precondition failed: console_app must apply directly';
    END IF;
    IF EXISTS (
        SELECT 1 FROM pg_catalog.pg_roles WHERE oid = v_writer
          AND (rolcanlogin OR rolsuper OR rolbypassrls OR rolinherit
               OR rolcreatedb OR rolcreaterole OR rolreplication)
    ) THEN
        RAISE EXCEPTION 'object-policy writer precondition failed: console_ontology_writer is unsafe';
    END IF;
    IF EXISTS (
        SELECT 1 FROM pg_catalog.pg_roles WHERE oid = v_runtime
          AND (NOT rolcanlogin OR rolsuper OR rolbypassrls OR rolinherit
               OR rolcreatedb OR rolcreaterole OR rolreplication)
    ) THEN
        RAISE EXCEPTION 'object-policy writer precondition failed: console_rt is unsafe';
    END IF;
END
$$;

-- ---------------------------------------------------------------------------
-- 2. The 0170 attachment trigger reads the catalog UNQUALIFIED and pins no
--    search_path (0170:5-22). Fired from inside a definer that runs under
--    `SET search_path = pg_catalog`, it raises 42P01 and every attach fails.
--    CREATE OR REPLACE keeps the 0170:24-26 trigger binding intact; a second
--    CREATE TRIGGER would double-fire the guard.
-- ---------------------------------------------------------------------------
CREATE OR REPLACE FUNCTION enforce_ont_object_policy_effect_matches_catalog()
RETURNS TRIGGER
LANGUAGE plpgsql
SET search_path = pg_catalog
AS $$
DECLARE
    catalog_effect TEXT;
BEGIN
    SELECT effect INTO catalog_effect
    FROM public.cedar_policy_catalog_entries
    WHERE id = NEW.cedar_policy_id AND org_id = NEW.org_id;

    IF catalog_effect IS NULL THEN
        RAISE EXCEPTION 'object policy attachment requires a same-org catalog entry';
    END IF;
    IF NEW.effect <> catalog_effect THEN
        RAISE EXCEPTION 'object policy attachment effect % must match catalog effect %', NEW.effect, catalog_effect;
    END IF;
    RETURN NEW;
END;
$$;

-- ---------------------------------------------------------------------------
-- 3. The vehicle schema. Precedent: 0166:331-334.
-- ---------------------------------------------------------------------------
CREATE SCHEMA ont_policy_api AUTHORIZATION console_ontology_writer;
REVOKE ALL ON SCHEMA ont_policy_api FROM PUBLIC;
GRANT USAGE ON SCHEMA ont_policy_api TO console_rt;

-- The writer needs SELECT for the trigger's same-org catalog lookup and INSERT
-- for the two rows the routine appends. It gets no UPDATE and no DELETE: both
-- tables are append-only by construction (0154:90-99).
GRANT SELECT, INSERT ON public.cedar_policy_catalog_entries TO console_ontology_writer;
GRANT SELECT, INSERT ON public.ont_object_policies TO console_ontology_writer;
-- §4 resolves the attached object type's `stable_key` to derive the catalog
-- stable_key and to check `normalized_row.resource_type`. Read-only, and FORCE
-- RLS (0152:122-144) keeps it inside the armed org.
GRANT SELECT ON public.ont_object_types TO console_ontology_writer;

-- `0154` granted `console_rt` INSERT on the attachment table outright, so an
-- attacker holding the runtime role could bind ANY existing enforced catalog
-- policy to ANY object type with one bare statement — the definer was not even
-- needed and every check in it was optional. Take it back: the audited writer
-- below is now the only way a row reaches this table. SELECT is untouched (the
-- read path joins it on every policy-filtered read).
REVOKE INSERT ON public.ont_object_policies FROM console_rt;

-- ---------------------------------------------------------------------------
-- 4. The audited writer's DB half: one enforced catalog row plus its
--    attachment, in the CALLER's transaction. 0206 renames this routine to
--    `attach_object_policy_rows` and appends the audit row in the wrapper that
--    calls it, still inside that same transaction, so a policy and the audit
--    claim that one exists commit or roll back together.
--
--    Everything shape-related is already guaranteed by the schema and is NOT
--    re-implemented here: 0150:11 pins the dotted stable_key, 0150:14-16 the
--    effect/status/source enums, 0150:43-46 the enforced-row NOT NULLs, and
--    0169:45-48's constraint is NOT VALID only for the pre-existing back-scan —
--    every INSERT is still checked.
-- ---------------------------------------------------------------------------
--    The four derivable parameters are GONE, not validated: `stable_key`,
--    `title`, `natural_language_rule` and the bundle digest are all functions of
--    the object type and of the stored row, so the routine computes them. A
--    caller cannot forge a value it cannot supply, and there is no check left to
--    get wrong. `generated_policy_text` is gone too — see the header: it is the
--    one parameter that is neither derivable nor boundable, so it is not taken.
--
--    The org floor is deliberately NOT re-implemented here: no
--    `p_org_id = current_setting('app.current_org')` and no `org_id` filter on
--    the type lookup. Either one pre-empts RLS on the only path that reaches the
--    floor, which would make the cross-org refusal in
--    `object_policy_attach_as_runtime_role.rs` (the `row-level security policy`
--    assertion) unsatisfiable — a real proof traded for a redundant check. RLS
--    already scopes the lookup to the armed org.
CREATE FUNCTION ont_policy_api.attach_object_policy(
    p_org_id UUID,
    p_created_by UUID,
    p_object_type_id UUID,
    p_effect TEXT,
    p_normalized_row JSONB,
    p_schema_version TEXT
)
RETURNS UUID
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog
SET row_security = on
AS $$
DECLARE
    v_policy_id UUID;
    v_type_key TEXT;
BEGIN
    -- The attached type, resolved under the caller's org floor. There is no FK
    -- on ont_object_policies.object_type_id (0154:29), so an unresolvable or
    -- foreign id would otherwise attach an enforced policy to nothing.
    SELECT stable_key INTO v_type_key
      FROM public.ont_object_types
     WHERE id = p_object_type_id;
    IF v_type_key IS NULL THEN
        RAISE EXCEPTION 'attach refused: unknown object type %', p_object_type_id;
    END IF;

    -- A row whose resource_type disagrees with the type it is attached to never
    -- matches `applicable_object_policies`, so it denies forever at HTTP 200 []
    -- — a silent failure no post-hoc test can see.
    IF p_normalized_row ->> 'resource_type' IS DISTINCT FROM v_type_key THEN
        RAISE EXCEPTION 'attach refused: normalized_row resource_type % is not the attached object type %',
            p_normalized_row ->> 'resource_type', v_type_key;
    END IF;

    -- The route pins this to `authoring::OBJECT_POLICY_ACTION`; so does the
    -- read path's `applicable_object_policies` filter.
    IF p_normalized_row ->> 'action' IS DISTINCT FROM 'view' THEN
        RAISE EXCEPTION 'attach refused: normalized_row action % is not the object-policy action',
            p_normalized_row ->> 'action';
    END IF;

    -- The 0170 trigger only compares CATALOG to ATTACHMENT, and both of those
    -- are bound from p_effect below, so a blocks/catalog disagreement sails
    -- through it. This is the only place that comparison happens.
    IF p_normalized_row ->> 'effect' IS DISTINCT FROM p_effect THEN
        RAISE EXCEPTION 'attach refused: normalized_row effect % disagrees with the attachment effect %',
            p_normalized_row ->> 'effect', p_effect;
    END IF;

    -- Mirrors MAX_ATTACHED_CONDITIONS (`ontology/rest/src/lib.rs:494`). Both
    -- tables are append-only, so an oversized list is permanent work charged to
    -- every later read of the type.
    IF jsonb_typeof(p_normalized_row -> 'conditions') IS DISTINCT FROM 'array'
       OR jsonb_array_length(p_normalized_row -> 'conditions') > 32 THEN
        RAISE EXCEPTION 'attach refused: conditions must be an array of at most 32 entries';
    END IF;

    INSERT INTO public.cedar_policy_catalog_entries
        (org_id, stable_key, title, natural_language_rule, effect, status, source,
         principal, action, resource, conditions,
         policy_version, schema_version, bundle_digest,
         validation_status, normalized_row, generated_policy_text,
         created_by, updated_by)
    VALUES
        (p_org_id,
         -- Dotted with >= 2 segments (0150:11) and UNIQUE per (org, key, status),
         -- so it carries the type it policies plus a fresh discriminator: two
         -- policies on one type is a supported shape, not a conflict.
         'object_policy.' || v_type_key || '.' || replace(gen_random_uuid()::text, '-', ''),
         -- title is bounded to 120 chars and the rule to 1000 (0150:12-13),
         -- while an object-type key has no length CHECK at all (0152:20 is
         -- shape-only). Bound the human-readable label; never the matching key.
         'Object policy: ' || left(v_type_key, 80),
         CASE p_effect WHEN 'permit' THEN 'Permit' ELSE 'Forbid' END
             || ' viewing rows of ' || left(v_type_key, 80)
             || ' when every authored condition holds.',
         p_effect,
         'enforced', 'no_code_draft',
         '{}'::jsonb, '{}'::jsonb, '{}'::jsonb, '[]'::jsonb,
         1, p_schema_version,
         -- Derived from the row actually stored beside it. A supplied digest is a
         -- stored false attestation that nothing ever re-checks. `normalized_row`
         -- and not the policy text, because the policy text is no longer stored:
         -- a digest over a NULL is NULL and fails the enforced-row CHECK, and a
         -- digest over the empty string would attest nothing at all.
         -- `jsonb::text` is canonical (keys sorted, whitespace fixed), so this is
         -- stable for a given row.
         'sha256:' || encode(sha256(convert_to(p_normalized_row::text, 'UTF8')), 'hex'),
         'valid', p_normalized_row, NULL,
         p_created_by, p_created_by)
    RETURNING id INTO v_policy_id;

    INSERT INTO public.ont_object_policies
        (org_id, object_type_id, cedar_policy_id, effect, created_by)
    VALUES
        (p_org_id, p_object_type_id, v_policy_id, p_effect, p_created_by);

    RETURN v_policy_id;
END;
$$;

-- The argument type list is repeated FOUR times. Missing one leaves the old
-- overload EXECUTE-granted beside the new one: the entire hardening bypassed,
-- with every test still green. 0206 retires this hazard by using 0165's blanket
-- `REVOKE ALL ON ALL FUNCTIONS IN SCHEMA` form instead, and moves the GRANT below
-- from `console_rt` to `console_ontology_cmd`.
ALTER FUNCTION ont_policy_api.attach_object_policy(
    UUID, UUID, UUID, TEXT, JSONB, TEXT
) OWNER TO console_ontology_writer;
REVOKE ALL ON FUNCTION ont_policy_api.attach_object_policy(
    UUID, UUID, UUID, TEXT, JSONB, TEXT
) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION ont_policy_api.attach_object_policy(
    UUID, UUID, UUID, TEXT, JSONB, TEXT
) TO console_rt;

-- ---------------------------------------------------------------------------
-- 5. Owner pin. A dropped ALTER FUNCTION … OWNER TO leaves the applier as owner
--    (console_app in production, the bootstrap superuser under sqlx::test — both
--    BYPASSRLS): the org floor would be gone and every functional test would
--    still pass. Fail the migration instead.
-- ---------------------------------------------------------------------------
DO $$
DECLARE
    v_owner NAME;
    v_unsafe BOOLEAN;
BEGIN
    SELECT r.rolname, (r.rolsuper OR r.rolbypassrls)
      INTO v_owner, v_unsafe
      FROM pg_catalog.pg_proc p
      JOIN pg_catalog.pg_namespace n ON n.oid = p.pronamespace
      JOIN pg_catalog.pg_roles r ON r.oid = p.proowner
     WHERE n.nspname = 'ont_policy_api' AND p.proname = 'attach_object_policy';
    IF v_owner IS DISTINCT FROM 'console_ontology_writer'::NAME OR v_unsafe THEN
        RAISE EXCEPTION 'object-policy writer definer owner is % (unsafe=%)', v_owner, v_unsafe;
    END IF;
END
$$;
