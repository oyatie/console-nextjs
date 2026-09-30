-- 0057_comms_due_account_enum.sql
-- B-mail-3: the webmail sync SCHEDULER must enumerate, across ALL tenants, which
-- email_accounts are due for an incremental IMAP sync pass — a chicken-and-egg
-- read that cannot itself be org-armed (the scheduler does not yet know which
-- orgs exist). The runtime role `console_rt` is NOBYPASSRLS + FORCE RLS, so a plain
-- cross-tenant SELECT against `email_accounts` returns ZERO rows (fail-closed).
--
-- A narrow SECURITY DEFINER function is the established pattern here (mirrors the
-- 0038 preauth org resolvers): it returns ONLY the (org_id, account_id) identity
-- pairs of due accounts — NEVER a credential, host, or any business field — so
-- the scheduler learns just enough to dispatch a per-(org, account) sync job that
-- then ARMS app.current_org and re-reads the full sealed account under RLS. The
-- function pins search_path, toggles row_security only for the id-only read, and
-- is EXECUTE-granted to console_rt alone (REVOKE FROM PUBLIC).
--
-- "Due" = an ACTIVE account whose last_sync_at is NULL (never synced) or older
-- than its own sync_cadence_secs. The result is bounded by p_limit so one tick
-- dispatches a bounded batch. No Korean copy.

CREATE OR REPLACE FUNCTION comms_due_email_accounts(
    p_now   TIMESTAMPTZ,
    p_limit INTEGER
)
RETURNS TABLE (org_id UUID, account_id UUID)
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = public, pg_temp
AS $$
BEGIN
    SET LOCAL row_security = off;
    RETURN QUERY
        SELECT a.org_id, a.id
        FROM email_accounts a
        WHERE a.status = 'ACTIVE'
          AND (
                a.last_sync_at IS NULL
             OR a.last_sync_at <= p_now - make_interval(secs => a.sync_cadence_secs)
          )
        ORDER BY a.last_sync_at ASC NULLS FIRST
        LIMIT GREATEST(p_limit, 0);
    SET LOCAL row_security = on;
END;
$$;

REVOKE ALL ON FUNCTION comms_due_email_accounts(TIMESTAMPTZ, INTEGER) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION comms_due_email_accounts(TIMESTAMPTZ, INTEGER) TO console_rt;
