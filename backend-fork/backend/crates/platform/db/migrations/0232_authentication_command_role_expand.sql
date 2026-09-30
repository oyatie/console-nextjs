-- Expand-only authentication command custody. The portable topology reconciler
-- preprovisions this role as NOLOGIN with no password or memberships. No caller
-- uses it yet; console_rt keeps its existing auth grants for rollback. The later
-- reviewed cutover must attach a distinct credential, move the complete auth
-- transactions, and only then revoke console_rt access to protected material.
DO $block$
DECLARE
    command_role OID := pg_catalog.to_regrole('console_auth_cmd');
    expected_owner OID := (SELECT oid FROM pg_catalog.pg_roles WHERE rolname = CURRENT_USER);
BEGIN
    IF command_role IS NULL OR EXISTS (
        SELECT 1 FROM pg_catalog.pg_roles
        WHERE oid = command_role
          AND (rolcanlogin OR rolsuper OR rolbypassrls OR rolinherit
               OR rolcreatedb OR rolcreaterole OR rolreplication)
    ) THEN
        RAISE EXCEPTION 'auth_command_role.missing_or_unsafe' USING ERRCODE = '42501';
    END IF;
    IF EXISTS (
        SELECT 1 FROM pg_catalog.pg_auth_members
        WHERE roleid = command_role OR member = command_role
    ) THEN
        RAISE EXCEPTION 'auth_command_role.membership_forbidden' USING ERRCODE = '42501';
    END IF;
    IF EXISTS (
        SELECT 1 FROM pg_catalog.pg_class relation,
            LATERAL pg_catalog.aclexplode(relation.relacl) privilege
        WHERE privilege.grantee = command_role
    ) OR EXISTS (
        SELECT 1 FROM pg_catalog.pg_attribute attribute,
            LATERAL pg_catalog.aclexplode(attribute.attacl) privilege
        WHERE privilege.grantee = command_role
    ) OR EXISTS (
        SELECT 1 FROM pg_catalog.pg_namespace namespace,
            LATERAL pg_catalog.aclexplode(namespace.nspacl) privilege
        WHERE privilege.grantee = command_role
    ) OR EXISTS (
        SELECT 1 FROM pg_catalog.pg_proc routine,
            LATERAL pg_catalog.aclexplode(routine.proacl) privilege
        WHERE privilege.grantee = command_role
    ) THEN
        RAISE EXCEPTION 'auth_command_role.preexisting_grants_require_review' USING ERRCODE = '42501';
    END IF;
    IF CURRENT_USER <> 'console_app' AND NOT (
        CURRENT_USER = 'console_buck_admin'
        AND SESSION_USER = CURRENT_USER
        AND current_setting('console.sqlx_test_bootstrap', true) = 'buck-sqlx-superuser-v1'
        AND CURRENT_DATABASE() ~ '^_sqlx_test_[A-Za-z0-9_]{52}$'
    ) THEN
        RAISE EXCEPTION 'auth_command_role.migration_owner_required' USING ERRCODE = '42501';
    END IF;
    IF EXISTS (
        SELECT 1 FROM pg_catalog.pg_class relation
        WHERE relation.oid = ANY (ARRAY[
            'public.users'::regclass,
            'public.auth_webauthn_credentials'::regclass,
            'public.auth_webauthn_ceremonies'::regclass,
            'public.auth_webauthn_ceremony_bindings'::regclass,
            'public.auth_bootstrap_credentials'::regclass,
            'public.auth_refresh_token_families'::regclass,
            'public.auth_refresh_tokens'::regclass,
            'public.auth_device_login_handoffs'::regclass,
            'auth_security.account_state'::regclass,
            'auth_security.credential_removals'::regclass,
            'auth_security.session_admissions'::regclass,
            'auth_security.registration_intents'::regclass
        ])
        AND relation.relowner <> expected_owner
    ) OR (SELECT nspowner FROM pg_catalog.pg_namespace
          WHERE nspname = 'auth_security') <> expected_owner THEN
        RAISE EXCEPTION 'auth_command_role.auth_owner_drift' USING ERRCODE = '42501';
    END IF;
END
$block$;

GRANT USAGE ON SCHEMA public, auth_security TO console_auth_cmd;

-- Remove any stale direct grants first. No PUBLIC or runtime grant is changed.
REVOKE ALL ON
    auth_security.account_id_reservations,
    auth_security.account_state,
    auth_security.credential_removals,
    auth_security.session_admissions,
    auth_security.registration_intents
FROM console_auth_cmd;
REVOKE ALL ON
    public.auth_webauthn_credentials,
    public.auth_webauthn_ceremonies,
    public.auth_webauthn_ceremony_bindings,
    public.auth_bootstrap_credentials,
    public.auth_refresh_token_families,
    public.auth_refresh_tokens,
    public.auth_device_login_handoffs,
    public.users
FROM console_auth_cmd;

GRANT SELECT, UPDATE ON auth_security.account_state TO console_auth_cmd;
GRANT SELECT ON auth_security.credential_removals TO console_auth_cmd;
GRANT SELECT, INSERT, UPDATE ON
    auth_security.session_admissions,
    auth_security.registration_intents
TO console_auth_cmd;

GRANT SELECT, INSERT, UPDATE, DELETE ON public.auth_webauthn_credentials TO console_auth_cmd;
GRANT SELECT, INSERT, UPDATE ON
    public.auth_webauthn_ceremonies,
    public.auth_bootstrap_credentials,
    public.auth_refresh_token_families,
    public.auth_refresh_tokens,
    public.auth_device_login_handoffs
TO console_auth_cmd;
GRANT SELECT, INSERT ON public.auth_webauthn_ceremony_bindings TO console_auth_cmd;
GRANT SELECT (id, org_id, is_active) ON public.users TO console_auth_cmd;

-- These existing SECURITY DEFINER locators disclose only a matching row's
-- Company ID so pre-auth operations can set RLS before reading the real row.
GRANT EXECUTE ON FUNCTION public.platform_resolve_bootstrap_org(BYTEA),
    public.platform_resolve_credential_org(TEXT),
    public.platform_resolve_token_org(BYTEA)
TO console_auth_cmd;

-- Shared rate-limit and audit tables are intentionally excluded. Their scoped
-- owner operations, along with narrow signup/profile writes, precede activation.
