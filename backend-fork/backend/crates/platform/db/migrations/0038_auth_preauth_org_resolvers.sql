-- Pre-auth tenant resolvers for the passkey + OTP sign-in paths.
--
-- THE PROBLEM (launch-blocking): under the prod runtime role `console_rt`
-- (NOSUPERUSER, NOBYPASSRLS, FORCE RLS), three pre-auth auth paths read/write
-- FORCE-RLS org-scoped auth tables with the `app.current_org` GUC UNSET, so RLS
-- returns ZERO rows (reads) or rejects the WITH CHECK (writes) and the path is
-- broken in production:
--
--   * OTP first-sign-in        — reads `auth_bootstrap_credentials` by token_hash
--   * passkey LOGIN            — reads `auth_webauthn_credentials` by credential_id
--
-- Each needs the tenant BEFORE it can arm the GUC, but the tenant is exactly what
-- a FORCE-RLS lookup cannot return until the GUC is armed (chicken-and-egg). The
-- refresh-token path already solved this with the narrow SECURITY DEFINER
-- resolver `platform_resolve_token_org` (migration 0036): it returns ONLY the
-- row's `org_id` so the app can arm the GUC, THEN do the real RLS-gated read.
--
-- This migration adds the SAME narrow resolver for the two remaining lookups.
-- Each is SECURITY DEFINER (runs as the function owner — the table owner in every
-- environment: the superuser locally / in sqlx::test, `console_app` under CNPG),
-- pins `search_path` (DEFINER privilege-escalation hardening), and briefly runs
-- with `row_security = off` so the owner can read the one row across tenants —
-- restoring `row_security = on` BEFORE returning so the caller's transaction is
-- never left unscoped (these are called mid-transaction; a non-BYPASSRLS caller
-- cannot continue with row_security off). Each exposes NOTHING but the tenant id.
-- EXECUTE is granted only to `console_rt`; PUBLIC gets none.

-- ---------------------------------------------------------------------------
-- (1) platform_resolve_bootstrap_org(token_hash) -> org_id (or NULL).
-- The org of the bootstrap credential with that token hash, so OTP first
-- sign-in can arm the GUC before the RLS-gated read/audit/consume.
-- `token_hash` is BYTEA (migration 0004 / rollout column add).
-- ---------------------------------------------------------------------------
CREATE OR REPLACE FUNCTION platform_resolve_bootstrap_org(
    p_token_hash BYTEA
)
RETURNS UUID
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = public, pg_temp
AS $$
DECLARE
    resolved UUID;
BEGIN
    SET LOCAL row_security = off;
    SELECT org_id
    INTO resolved
    FROM auth_bootstrap_credentials
    WHERE token_hash = p_token_hash
    LIMIT 1;
    SET LOCAL row_security = on;
    RETURN resolved;  -- NULL when no such credential
END;
$$;
REVOKE ALL ON FUNCTION platform_resolve_bootstrap_org(BYTEA) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION platform_resolve_bootstrap_org(BYTEA) TO console_rt;

-- ---------------------------------------------------------------------------
-- (2) platform_resolve_credential_org(credential_id) -> org_id (or NULL).
-- The org of the webauthn credential with that credential id, so passkey LOGIN
-- can arm the GUC before the RLS-gated SELECT + last_used_at UPDATE.
-- `credential_id` is TEXT (the base64url-encoded credential id, migration 0004).
-- ---------------------------------------------------------------------------
CREATE OR REPLACE FUNCTION platform_resolve_credential_org(
    p_credential_id TEXT
)
RETURNS UUID
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = public, pg_temp
AS $$
DECLARE
    resolved UUID;
BEGIN
    SET LOCAL row_security = off;
    SELECT org_id
    INTO resolved
    FROM auth_webauthn_credentials
    WHERE credential_id = p_credential_id
    LIMIT 1;
    SET LOCAL row_security = on;
    RETURN resolved;  -- NULL when no such credential
END;
$$;
REVOKE ALL ON FUNCTION platform_resolve_credential_org(TEXT) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION platform_resolve_credential_org(TEXT) TO console_rt;

-- ---------------------------------------------------------------------------
-- (3) Runtime-role DML on the GLOBAL pre-auth tables.
--
-- `auth_webauthn_ceremonies` (migration 0004) and `auth_rate_limit` (0021) are
-- deliberately GLOBAL — no org_id, no RLS — because they hold transient pre-auth
-- state written before a tenant is known. But both predate the `console_app` DEFAULT
-- PRIVILEGES grant (migration 0031), so the non-owner runtime role `console_rt` was
-- never granted DML on them. As `console_rt` the passkey ceremonies (start/finish
-- register + login persist/claim a ceremony) and the per-client rate limiter
-- (every login/redeem/refresh attempt upserts a counter) would otherwise fail
-- "permission denied". Grant exactly the verbs the app uses: SELECT + INSERT +
-- UPDATE (the rate-limit upsert UPDATEs on conflict; ceremonies are claimed via
-- UPDATE). No DELETE is granted — neither table is pruned by the app.
GRANT SELECT, INSERT, UPDATE ON auth_webauthn_ceremonies TO console_rt;
GRANT SELECT, INSERT, UPDATE ON auth_rate_limit TO console_rt;
