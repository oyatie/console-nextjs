-- Object-policy attachment becomes an AUDITED database boundary, reachable by
-- one credential the application holds only on the audited path.
--
-- WHAT MOVED. 0205's six-argument routine is renamed to
-- `attach_object_policy_rows` and keeps its entire envelope — it is still the
-- only thing that writes the catalog row and the attachment. `console_rt` loses
-- EXECUTE on everything in this schema. A NEW eight-argument
-- `attach_object_policy` calls the row-writer and appends the audit row, and it
-- is the only routine here anyone but the owner may execute; the credential that
-- may execute it is `console_ontology_cmd`, which the application binds to the
-- attach route and to nothing else (`platform/authz-rest/src/store.rs`
-- `attach_object_policy`, reached through `command_pool()`).
--
-- WHY THE SPLIT AND NOT ONE ROUTINE. The audit row has to be unskippable by the
-- credential that can attach at all, or the trace requirement is advisory. The
-- row-writer therefore stays owner-only: `console_ontology_cmd` gets 42501 on it
-- and can only come in through the entrypoint that audits. This is 0165's shape
-- exactly (`0165:1224-1236`: helper routines owner-only, complete entrypoints
-- granted to `console_ontology_cmd`), not a second mechanism.
--
-- THE AUDIT INSERT IS LAST, deliberately. The catalog INSERT is the statement the
-- RLS org floor stops on a cross-org call, and that refusal is the only proof the
-- floor still applies inside the definer. Audit-first moves the refusal onto
-- `audit_events` and the org-floor proof off the policy catalog, while satisfying
-- any assertion that merely looks for "row-level security policy".
--
-- The audit row is written by the same NOBYPASSRLS `console_ontology_writer` that
-- writes the policy, in the CALLER's transaction, so an attach and its audit
-- claim commit or roll back together. `console_ontology_writer` already holds
-- INSERT on `audit_events` (0165's `GRANT SELECT, INSERT ON ... audit_events ...
-- TO console_ontology_writer`); no new table grant, and `console_ontology_cmd`
-- deliberately gets none: an INSERT privilege there would let the one credential
-- that may attach also write an audit row for an attach that never happened, and
-- the absent grant is the ONLY thing that stops it. Asserted by
-- `the_command_credential_holds_no_direct_write_on_the_tables_the_definer_writes`,
-- whose `has_table_privilege` vector over the three tables this routine writes
-- must stay empty.
--
-- NOT by a trigger, and an earlier version of this header said otherwise. 0165's
-- `ontology_api.protected_audit_writer_guard()` (trigger
-- `trg_audit_events_ontology_command_only`) `RETURN NEW`s immediately for any
-- action outside its four `ontology.object_type.*` entries, and
-- `ontology.object_policy.attach` is not one of them, so the
-- `ELSIF v_invoker <> 'console_ontology_cmd'` branch that raises
-- `ontology_audit.command_required` is never reached on this path. Measured on a
-- database with this migration applied: as `console_rt`, an INSERT of an
-- `ontology.object_policy.attach` row succeeds (`INSERT 0 1`), while the same
-- INSERT with `action = 'ontology.object_type.builtin_install'` raises
-- `ontology_audit.command_required` from that guard. The guard is live; it does
-- not cover this action.
--
-- RESIDUAL, therefore, and it is not what this migration set out to close: the
-- audit row is UNSKIPPABLE by the credential that can attach, because that
-- credential cannot reach `attach_object_policy_rows`. It is not UNFORGEABLE by
-- `console_rt`, which holds INSERT on `audit_events` because every other route's
-- audit row needs it. Closing that means adding this action to 0165's protected
-- list — reachable (`ontology_api.invoker_role()` reads `current_setting('role')`
-- and falls back to SESSION_USER, so the definer path presents
-- `console_ontology_cmd` and passes the ELSIF, while a direct `console_rt` INSERT
-- would hit the `NEW.target_type <> 'ont_object_types'` raise) but it is a change
-- to a shared guard for one action out of every audited action in the system, so
-- it is escalated here rather than smuggled in.
--
-- `ontology_api.write_audit` is NOT reused: its INSERT hardcodes
-- `target_type = 'ont_object_types'`, which is a lie about what was mutated here.
-- `ontology_api.assert_write_context` is NOT called: its
-- `app.current_org <> p_org_id` raise pre-empts RLS and makes the cross-org proof
-- unsatisfiable — 0205 forbids exactly this where it declines to re-implement the
-- org floor inside the writer, because the `row-level security policy` assertion
-- in `object_policy_attach_as_runtime_role.rs` is the only proof the floor still
-- applies inside a definer.
--
-- No `CREATE ROLE` anywhere: cluster-global roles are infrastructure-owned and
-- this migration fails closed on drift, exactly as 0165's
-- `ontology_role_topology.roles_not_preprovisioned` precondition does.
--
-- No routine is added to `ontology_api`: its routine count and namespace ACL are
-- pinned by ontology/adapter-postgres/tests/key_revision_migration_upgrade.rs.

-- ---------------------------------------------------------------------------
-- 1. Preconditions. 0205's `object-policy writer precondition failed: …` DO block
--    re-run, plus the command role this migration hands the attach capability to.
-- ---------------------------------------------------------------------------
DO $$
DECLARE
    v_migrator OID := pg_catalog.to_regrole('console_app');
    v_runtime OID := pg_catalog.to_regrole('console_rt');
    v_writer OID := pg_catalog.to_regrole('console_ontology_writer');
    v_command OID := pg_catalog.to_regrole('console_ontology_cmd');
    v_applier_is_superuser BOOLEAN;
BEGIN
    IF v_migrator IS NULL OR v_runtime IS NULL OR v_writer IS NULL OR v_command IS NULL THEN
        RAISE EXCEPTION 'object-policy command precondition failed: roles are not preprovisioned';
    END IF;
    SELECT rolsuper INTO v_applier_is_superuser
      FROM pg_catalog.pg_roles WHERE rolname = CURRENT_USER;
    IF NOT v_applier_is_superuser
       AND (CURRENT_USER <> 'console_app' OR SESSION_USER <> 'console_app') THEN
        RAISE EXCEPTION 'object-policy command precondition failed: console_app must apply directly';
    END IF;
    IF EXISTS (
        SELECT 1 FROM pg_catalog.pg_roles WHERE oid = v_writer
          AND (rolcanlogin OR rolsuper OR rolbypassrls OR rolinherit
               OR rolcreatedb OR rolcreaterole OR rolreplication)
    ) THEN
        RAISE EXCEPTION 'object-policy command precondition failed: console_ontology_writer is unsafe';
    END IF;
    IF EXISTS (
        SELECT 1 FROM pg_catalog.pg_roles WHERE oid = v_runtime
          AND (NOT rolcanlogin OR rolsuper OR rolbypassrls OR rolinherit
               OR rolcreatedb OR rolcreaterole OR rolreplication)
    ) THEN
        RAISE EXCEPTION 'object-policy command precondition failed: console_rt is unsafe';
    END IF;
    -- The role that inherits the attach capability is held to the same bar as the
    -- one that loses it: a BYPASSRLS or superuser command credential would carry
    -- the org floor away with it and every test here would still pass.
    IF EXISTS (
        SELECT 1 FROM pg_catalog.pg_roles WHERE oid = v_command
          AND (NOT rolcanlogin OR rolsuper OR rolbypassrls OR rolinherit
               OR rolcreatedb OR rolcreaterole OR rolreplication)
    ) THEN
        RAISE EXCEPTION 'object-policy command precondition failed: console_ontology_cmd is unsafe';
    END IF;
END
$$;

-- ---------------------------------------------------------------------------
-- 2. Schema USAGE for the command role. 0205's
--    `GRANT USAGE ON SCHEMA ont_policy_api TO console_rt` is the only USAGE grant
--    on this schema; `has_function_privilege` does not consult schema USAGE, so without
--    this every attach fails 42501 naming the SCHEMA while any per-role grant
--    probe reads true.
--
--    `console_rt` KEEPS its USAGE. Revoking it only degrades its refusal from
--    `permission denied for function attach_object_policy` to a schema-level
--    message, and USAGE alone grants nothing executable.
-- ---------------------------------------------------------------------------
GRANT USAGE ON SCHEMA ont_policy_api TO console_ontology_cmd;

-- ---------------------------------------------------------------------------
-- 3. 0205's body becomes the row-writer. RENAME rather than DROP + re-CREATE:
--    a second copy of ~100 lines of security-critical SQL in this file would
--    leave 0205's copy dead but authoritative-looking, and the two would drift.
--    RENAME preserves `prosecdef`, `proconfig`, `proowner` and the ACL. Asserted
--    rather than assumed: the definer-attribute probe in
--    `object_policy_attach_as_runtime_role.rs` compares those attributes as a TOTAL
--    vector over this schema, so `attach_object_policy_rows` keeps its
--    `SET search_path = pg_catalog` — the only thing between 0205 §4's eight
--    unqualified `pg_catalog` calls and a caller choosing what they resolve to.
--
--    If this statement is ever dropped, §5's REVOKE/ALTER on
--    `attach_object_policy_rows` errors "function does not exist" and the
--    migration fails closed rather than leaving 0205's routine granted.
-- ---------------------------------------------------------------------------
ALTER FUNCTION ont_policy_api.attach_object_policy(UUID, UUID, UUID, TEXT, JSONB, TEXT)
    RENAME TO attach_object_policy_rows;

-- ---------------------------------------------------------------------------
-- 4. The audited entrypoint. It performs NO envelope checks of its own: every
--    one of them lives in the row-writer, which is the single source. Adding a
--    copy here is how the two diverge.
-- ---------------------------------------------------------------------------
CREATE FUNCTION ont_policy_api.attach_object_policy(
    p_org_id UUID,
    p_created_by UUID,
    p_object_type_id UUID,
    p_effect TEXT,
    p_normalized_row JSONB,
    p_schema_version TEXT,
    p_trace_id TEXT,
    p_span_id TEXT
)
RETURNS UUID
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog
SET row_security = on
AS $$
DECLARE
    v_policy_id UUID;
    v_occurred_at TIMESTAMPTZ := pg_catalog.statement_timestamp();
BEGIN
    v_policy_id := ont_policy_api.attach_object_policy_rows(
        p_org_id, p_created_by, p_object_type_id, p_effect,
        p_normalized_row, p_schema_version);

    -- LAST. See the header: the catalog INSERT inside the row-writer is what the
    -- org floor refuses on a cross-org call, and moving this above it moves that
    -- proof onto `audit_events`.
    --
    -- `id` is omitted so the 0003:11 `DEFAULT gen_random_uuid()` supplies it:
    -- column defaults carry resolved function OIDs, so they are unaffected by
    -- this routine's pinned `search_path`.
    --
    -- The request-context columns (ip, user_agent, auth_method, device,
    -- classification) stay NULL, which is exactly what the application wrote:
    -- `audit_event` (`platform/authz-rest/src/store.rs`) built the event with
    -- `AuditEvent::new` and never attached a request context.
    INSERT INTO public.audit_events
        (actor, action, target_type, target_id, branch_id,
         before_snap, after_snap, trace_id, span_id, occurred_at, org_id)
    VALUES
        (p_created_by, 'ontology.object_policy.attach',
         'ont_object_policies', p_object_type_id::TEXT, NULL,
         NULL, p_normalized_row, p_trace_id, p_span_id, v_occurred_at, p_org_id);

    RETURN v_policy_id;
END;
$$;

-- ---------------------------------------------------------------------------
-- 5. Owner and ACL. Mirrors 0165:1224-1236 rather than 0205's per-overload
--    `REVOKE ALL ON FUNCTION … FROM PUBLIC` / `GRANT EXECUTE … TO console_rt`
--    pair: one blanket REVOKE over the whole schema, so the argument type list is
--    written once per routine instead of four times. It also strips any
--    accidentally surviving overload, which is the hazard 0205's comment about its
--    four-times-repeated argument type list could only warn about.
--
--    ALTER OWNER precedes the REVOKE deliberately: revoking from PUBLIC is what
--    materializes a NULL `proacl`, and the materialized default names the CURRENT
--    owner as both grantee and grantor.
-- ---------------------------------------------------------------------------
ALTER FUNCTION ont_policy_api.attach_object_policy_rows(
    UUID, UUID, UUID, TEXT, JSONB, TEXT
) OWNER TO console_ontology_writer;
ALTER FUNCTION ont_policy_api.attach_object_policy(
    UUID, UUID, UUID, TEXT, JSONB, TEXT, TEXT, TEXT
) OWNER TO console_ontology_writer;

REVOKE ALL ON ALL FUNCTIONS IN SCHEMA ont_policy_api
    FROM PUBLIC, console_rt, console_ontology_cmd;

GRANT EXECUTE ON FUNCTION ont_policy_api.attach_object_policy(
    UUID, UUID, UUID, TEXT, JSONB, TEXT, TEXT, TEXT
) TO console_ontology_cmd;

-- ---------------------------------------------------------------------------
-- 6. Owner pin, widened from 0205's `object-policy writer definer owner is %`
--    DO block to EVERY routine in the schema: there are two now, and 0205's
--    version filtered on `p.proname = 'attach_object_policy'`. A dropped
--    ALTER FUNCTION … OWNER TO leaves the applier as owner (console_app in
--    production, the bootstrap superuser under sqlx::test — both BYPASSRLS): the
--    org floor would be gone and every functional test would still pass. Fail
--    the migration instead.
-- ---------------------------------------------------------------------------
DO $$
DECLARE
    v_bad TEXT;
BEGIN
    SELECT string_agg(p.proname || ' owned by ' || r.rolname, ', ' ORDER BY p.proname)
      INTO v_bad
      FROM pg_catalog.pg_proc p
      JOIN pg_catalog.pg_namespace n ON n.oid = p.pronamespace
      JOIN pg_catalog.pg_roles r ON r.oid = p.proowner
     WHERE n.nspname = 'ont_policy_api'
       AND (r.rolname <> 'console_ontology_writer' OR r.rolsuper OR r.rolbypassrls);
    IF v_bad IS NOT NULL THEN
        RAISE EXCEPTION 'object-policy definer ownership is unsafe: %', v_bad;
    END IF;
END
$$;
