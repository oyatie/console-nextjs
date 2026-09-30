-- 0117_comms_email_account_claim_token_fencing.sql
-- FORWARD-ONLY fencing fix for the inbound webmail sync scheduler claim.
--
-- 0116 shipped the claim lease (`claimed_until` + a 3-arg `comms_due_email_accounts`
-- with FOR UPDATE SKIP LOCKED), but WITHOUT a fencing token. That leaves a reclaim
-- race: if a slow worker's lease expires and a second worker reclaims the row, the
-- first worker's late lease-clear (`record_sync_result`) still matches the row and
-- wipes the SECOND worker's fresh claim — reopening the overlapping-sync window the
-- lease was meant to close. 0116 is already applied and immutable, so this forwards
-- the schema to the fenced design instead of editing it:
--   1. `email_accounts.claim_token` — a per-claim fencing token. Each claim mints a
--      fresh `gen_random_uuid()`; the lease is released only by the worker still
--      holding the matching token (compare-and-clear in `record_sync_result` /
--      `release_claim`). A late clear from a superseded worker matches NO token and
--      wipes nothing, so it cannot stomp the reclaiming worker's fresh lease.
--   2. `comms_due_email_accounts(now, limit, lease)` is re-created to ALSO stamp a
--      fresh `claim_token` per claimed row and to RETURN the (org_id, account_id,
--      claim_token) triple. Changing the RETURNS TABLE columns requires DROP+CREATE
--      (CREATE OR REPLACE cannot change a function's result columns).
--   3. The 2-arg `comms_due_email_accounts(now, limit)` compat shim (0116 dropped
--      it) is re-created, delegating to the 3-arg with a default 600s lease for
--      rolling-deploy safety (old callers keep the HA-safe claim, drop the token
--      they don't consume). 600s mirrors the adapter's SYNC_CLAIM_LEASE_SECS.
--   4. A partial index on the due/claim scan so the cross-tenant scheduler read
--      does not full-scan `email_accounts` at scale.
--
-- Posture unchanged: id-only across tenants under SECURITY DEFINER (REVOKE PUBLIC,
-- GRANT console_rt); only the claim stamp is written and only identity + claim-token
-- are returned. No credential/host/business field crosses the tenant boundary. No
-- Korean copy.

ALTER TABLE email_accounts ADD COLUMN IF NOT EXISTS claim_token UUID;

-- Support the due/claim scan: the scheduler filters ACTIVE accounts on their lease
-- + last_sync_at, so a partial index keyed on the lease avoids a full scan.
CREATE INDEX IF NOT EXISTS email_accounts_due_claim_idx
    ON email_accounts (claimed_until)
    WHERE status = 'ACTIVE';

-- The 3-arg claimer's RETURNS TABLE gains `claim_token`, so the existing 0116
-- 3-arg (which returned only (org_id, account_id)) must be dropped before the new
-- one is created — a result-column change is not a CREATE OR REPLACE.
DROP FUNCTION IF EXISTS comms_due_email_accounts(TIMESTAMPTZ, INTEGER, INTEGER);

CREATE FUNCTION comms_due_email_accounts(
    p_now        TIMESTAMPTZ,
    p_limit      INTEGER,
    p_lease_secs INTEGER
)
RETURNS TABLE (org_id UUID, account_id UUID, claim_token UUID)
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
        SET claimed_until = p_now + make_interval(secs => GREATEST(p_lease_secs, 1)),
            -- Fresh fencing token per claimed row: the worker that receives this
            -- token is the ONLY one allowed to clear the lease it just took.
            claim_token = gen_random_uuid()
        FROM due
        WHERE a.id = due.id
        RETURNING a.org_id, a.id, a.claim_token;
    SET LOCAL row_security = on;
END;
$$;

REVOKE ALL ON FUNCTION comms_due_email_accounts(TIMESTAMPTZ, INTEGER, INTEGER) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION comms_due_email_accounts(TIMESTAMPTZ, INTEGER, INTEGER) TO console_rt;

-- compat shim — remove one release after all pods run the 3-arg path.
-- During a rolling deploy, pre-0116 pods still call the OLD 2-arg signature (0116
-- dropped it). Re-create it delegating to the 3-arg claimer with a sane default
-- lease so old callers still get the SAME HA-safe claim-and-lease behaviour (never
-- the old lock-free enumeration) and simply drop the claim token they don't yet
-- consume. The 600s default mirrors the adapter's SYNC_CLAIM_LEASE_SECS.
CREATE OR REPLACE FUNCTION comms_due_email_accounts(
    p_now   TIMESTAMPTZ,
    p_limit INTEGER
)
RETURNS TABLE (org_id UUID, account_id UUID)
LANGUAGE sql
SECURITY DEFINER
SET search_path = public, pg_temp
AS $$
    SELECT d.org_id, d.account_id
    FROM comms_due_email_accounts(p_now, p_limit, 600) AS d;
$$;

REVOKE ALL ON FUNCTION comms_due_email_accounts(TIMESTAMPTZ, INTEGER) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION comms_due_email_accounts(TIMESTAMPTZ, INTEGER) TO console_rt;
