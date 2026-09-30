-- 0122_comms_account_by_address.sql
-- mox integration (slice 1): the mox delivery WEBHOOK ingests an incoming message
-- pushed by our own mail server. That request carries NO tenant principal — the
-- only tenant selector is the recipient address on the delivery. The runtime role
-- `console_rt` is NOBYPASSRLS + FORCE RLS, so a plain SELECT against `email_accounts`
-- keyed on the address returns ZERO rows (fail-closed) unless an org is already
-- armed — which the webhook cannot do before it knows the org.
--
-- Mirrors the 0057 `comms_due_email_accounts` pattern exactly: a narrow SECURITY
-- DEFINER function that returns ONLY the (org_id, account_id) identity of the
-- ACTIVE account(s) whose `email_address` matches — NEVER a credential, host, or
-- any business field. The webhook then ARMS app.current_org to that org and
-- performs the audited inbound UPSERT under RLS. The function pins search_path,
-- toggles row_security only for the id-only read, and is EXECUTE-granted to
-- console_rt alone (REVOKE FROM PUBLIC). Match is case-insensitive on the address
-- (email is case-insensitive in practice for the local delivery path).
--
-- Returns ALL matching rows (not LIMIT 1): `email_accounts` is unique only on
-- (org_id, email_address), so the same address CAN exist under two different
-- orgs. Silently picking one with LIMIT 1 would route inbound mail to the wrong
-- tenant nondeterministically (a PG plan/order artifact, not a real selection).
-- The caller (find_account_by_address_inner) treats >1 row as an ambiguous
-- tenant-boundary anomaly and refuses delivery to all matches rather than guess.
-- ORDER BY is defense-in-depth for any single-row consumer, not the correctness
-- fix — the correctness fix is the caller's row-count check. No Korean copy.

CREATE INDEX IF NOT EXISTS idx_email_accounts_active_lower_email_address
    ON email_accounts (lower(email_address))
    WHERE status = 'ACTIVE';

CREATE OR REPLACE FUNCTION comms_account_by_address(
    p_address TEXT
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
          AND lower(a.email_address) = lower(btrim(p_address))
        ORDER BY a.org_id, a.id;
    SET LOCAL row_security = on;
END;
$$;

REVOKE ALL ON FUNCTION comms_account_by_address(TEXT) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION comms_account_by_address(TEXT) TO console_rt;
