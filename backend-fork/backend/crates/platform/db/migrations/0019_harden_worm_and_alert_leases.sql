-- Harden wave 2 schema changes.
--
-- FIX 3: defense-in-depth WORM completion invariant — a DB trigger that rejects
--   AFTER/REPORT evidence_media inserts when the parent work order is terminal.
-- FIX 4: P1 alert delivery exactly-once — lease columns + a stable provider
--   idempotency key on p1_dispatch_alerts so a crashed worker cannot double-send.

-- ---------------------------------------------------------------------------
-- FIX 3 — WORM terminal-status trigger
-- ---------------------------------------------------------------------------
-- After a work order reaches a terminal status (FINAL_COMPLETED / ARCHIVED /
-- CANCELLED) its completion evidence set is frozen. The REST + storage layers
-- already lock the parent work_orders row and reject AFTER/REPORT presign +
-- insert for terminal work orders; this trigger is a second, writer-agnostic
-- barrier that rejects the INSERT regardless of which code path attempts it.
--
-- Only AFTER and REPORT stages feed the evidence_verified completion interlock,
-- so only those stages are guarded here. BEFORE/DURING/REQUEST/OUTSOURCE_RESULT
-- evidence remains insertable for audit corrections.

CREATE OR REPLACE FUNCTION evidence_media_reject_terminal_completion()
RETURNS TRIGGER
LANGUAGE plpgsql
AS $$
DECLARE
    parent_status TEXT;
BEGIN
    IF NEW.stage NOT IN ('AFTER', 'REPORT') THEN
        RETURN NEW;
    END IF;

    SELECT status
    INTO parent_status
    FROM work_orders
    WHERE id = NEW.work_order_id;

    IF parent_status IN ('FINAL_COMPLETED', 'ARCHIVED', 'CANCELLED') THEN
        RAISE EXCEPTION
            'cannot attach % evidence to work order % in terminal status %',
            NEW.stage, NEW.work_order_id, parent_status
            USING ERRCODE = 'check_violation';
    END IF;

    RETURN NEW;
END;
$$;

CREATE TRIGGER trg_evidence_media_reject_terminal_completion
    BEFORE INSERT ON evidence_media
    FOR EACH ROW
    EXECUTE FUNCTION evidence_media_reject_terminal_completion();

-- ---------------------------------------------------------------------------
-- FIX 4 — P1 alert delivery leases + idempotency key
-- ---------------------------------------------------------------------------
-- Workers must claim an alert before calling the push/Alimtalk provider and may
-- only mark it SENT/FAILED while still holding the lease. A crash after the
-- provider call but before the status update leaves the alert in SENDING with a
-- lease that another worker reclaims only once it has expired. The stable
-- idempotency_key (dispatch_id:alert_id) lets the provider dedupe even if a
-- duplicate send slips through.

ALTER TABLE p1_dispatch_alerts
    DROP CONSTRAINT p1_dispatch_alerts_status_check;

ALTER TABLE p1_dispatch_alerts
    ADD CONSTRAINT p1_dispatch_alerts_status_check
    CHECK (status IN ('PENDING','SENDING','SENT','SKIPPED','FAILED'));

ALTER TABLE p1_dispatch_alerts
    ADD COLUMN lease_token       UUID,
    ADD COLUMN lease_expires_at  TIMESTAMPTZ,
    ADD COLUMN idempotency_key   TEXT;

-- A SENDING alert must always hold a lease token + expiry; terminal/idle states
-- must not.
ALTER TABLE p1_dispatch_alerts
    ADD CONSTRAINT p1_dispatch_alerts_lease_consistency
    CHECK (
        (status = 'SENDING' AND lease_token IS NOT NULL AND lease_expires_at IS NOT NULL)
        OR (status <> 'SENDING')
    );

-- Backfill a deterministic idempotency key for any pre-existing alerts.
UPDATE p1_dispatch_alerts
SET idempotency_key = dispatch_id::text || ':' || id::text
WHERE idempotency_key IS NULL;

-- Reclaim index: find SENDING alerts whose lease has expired (crash recovery).
CREATE INDEX idx_p1_dispatch_alerts_expired_lease
    ON p1_dispatch_alerts (lease_expires_at)
    WHERE status = 'SENDING';

-- ---------------------------------------------------------------------------
-- FIX 1 + FIX 2 — offline sync replay binding + crash recovery
-- ---------------------------------------------------------------------------
-- FIX 1: bind the idempotency cache to the operation payload. A canonical
--   sha256 payload hash is stored on the sync row; a replay of the same
--   (device_hash, request_id) with a DIFFERENT payload is rejected instead of
--   silently returning the stale response.
-- FIX 2: persist the canonical request payload so a sync row that was claimed
--   but never completed (worker crash between the business mutation commit and
--   the completion mark) can be reconciled on replay — the target work-order
--   state is inspected and the correct final response is re-derived.

ALTER TABLE offline_sync_requests
    ADD COLUMN payload_hash    TEXT
        CHECK (payload_hash IS NULL OR payload_hash ~ '^[a-f0-9]{64}$'),
    ADD COLUMN request_payload JSONB;
