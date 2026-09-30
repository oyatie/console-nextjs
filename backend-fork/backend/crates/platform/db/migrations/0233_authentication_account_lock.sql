-- The auth command role needs the same Account mutex as lifecycle operations,
-- without direct UPDATE on users. Keep this expand-only until callers move.
CREATE FUNCTION auth_security.lock_account_for_auth(p_company UUID, p_account UUID)
RETURNS BOOLEAN
LANGUAGE sql
SECURITY DEFINER
SET search_path = pg_catalog
SET row_security = on
AS $$
    SELECT u.is_active
    FROM public.users AS u
    WHERE u.id = p_account
      AND u.org_id = p_company
      AND u.org_id = NULLIF(pg_catalog.current_setting('app.current_org', true), '')::UUID
    FOR NO KEY UPDATE
$$;

REVOKE ALL ON FUNCTION auth_security.lock_account_for_auth(UUID, UUID)
    FROM PUBLIC, console_rt;
GRANT EXECUTE ON FUNCTION auth_security.lock_account_for_auth(UUID, UUID)
    TO console_auth_cmd;
