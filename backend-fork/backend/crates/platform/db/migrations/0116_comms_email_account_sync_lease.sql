-- 0116_comms_email_account_sync_lease.sql
-- HA-safety for the inbound webmail sync SCHEDULER (B-mail-3). Before this, the
-- scheduler enumerated due accounts with a plain cross-tenant SELECT (0057) that
-- held NO lock and stamped NO lease, so EVERY replica running the ticker would
-- enumerate the SAME due accounts and sync each mailbox concurrently (duplicate
-- IMAP work + racing lifecycle writes). The ticker is now spawned only under the
-- worker role, but a worker can itself be horizontally scaled, so the claim must
-- be correct with >1 worker replica.
--
-- Fix (both parts live here):
--   1. `email_accounts.claimed_until` — a lease expiry. A row is claimable when
--      it is due AND its lease is absent or already expired. A crashed worker's
--      claim self-heals: once `claimed_until` passes, the row is due again.
--   2. `comms_due_email_accounts` becomes an ATOMIC CLAIM: it selects the due,
--      unclaimed rows `FOR UPDATE SKIP LOCKED` (a competing worker's in-flight
--      claim is skipped, never blocked) and stamps a fresh `claimed_until` lease
--      on each before returning its id-only (org_id, account_id) pair. Two
--      workers ticking together therefore claim DISJOINT sets. The lease is
--      cleared by the sync pass on completion (`record_sync_result`).
--
-- Still id-only across tenants under SECURITY DEFINER (REVOKE PUBLIC, GRANT
-- console_rt): no credential/host/business field crosses the tenant boundary — only
-- the claim stamp is written, and only identity pairs are returned. No Korean copy.

ALTER TABLE email_accounts ADD COLUMN IF NOT EXISTS claimed_until TIMESTAMPTZ;

-- The signature changes (adds p_lease_secs), so drop the old 2-arg enumerator
-- before creating the 3-arg claimer.
DROP FUNCTION IF EXISTS comms_due_email_accounts(TIMESTAMPTZ, INTEGER);

CREATE FUNCTION comms_due_email_accounts(
    p_now        TIMESTAMPTZ,
    p_limit      INTEGER,
    p_lease_secs INTEGER
)
RETURNS TABLE (org_id UUID, account_id UUID)
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = public, pg_temp
AS $$
BEGIN
    SET LOCAL row_security = off;
    RETURN QUERY
        WITH due AS (
            SELECT a.id
            FROM email_accounts a
            WHERE a.status = 'ACTIVE'
              AND (
                    a.last_sync_at IS NULL
                 OR a.last_sync_at <= p_now - make_interval(secs => a.sync_cadence_secs)
              )
              AND (a.claimed_until IS NULL OR a.claimed_until <= p_now)
            ORDER BY a.last_sync_at ASC NULLS FIRST
            LIMIT GREATEST(p_limit, 0)
            FOR UPDATE SKIP LOCKED
        )
        UPDATE email_accounts a
        SET claimed_until = p_now + make_interval(secs => GREATEST(p_lease_secs, 1))
        FROM due
        WHERE a.id = due.id
        RETURNING a.org_id, a.id;
    SET LOCAL row_security = on;
END;
$$;

REVOKE ALL ON FUNCTION comms_due_email_accounts(TIMESTAMPTZ, INTEGER, INTEGER) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION comms_due_email_accounts(TIMESTAMPTZ, INTEGER, INTEGER) TO console_rt;
