-- Install the trigger before scanning existing rows. Its table lock closes the
-- upgrade race with concurrent INSERTs; a busy table makes this retryable.
SET LOCAL lock_timeout = '2s';

-- The adapter writes the immutable row before its audit in one transaction.
-- Check at COMMIT so that ordering remains valid. This prevents an unaudited
-- direct console_rt INSERT, but the shared role can still claim another actor
-- and write its own matching audit; authenticated principal binding remains a
-- separate owner-path requirement before any delivery or egress.
CREATE FUNCTION public.require_email_outbound_preparation_audit()
RETURNS TRIGGER
LANGUAGE plpgsql SECURITY DEFINER
SET search_path = pg_catalog SET row_security = on AS $function$
BEGIN
    IF (
        SELECT count(*) FROM public.audit_events a
        JOIN public.users u ON u.id = a.actor AND u.org_id = a.org_id AND u.is_active
        WHERE a.org_id = NEW.org_id
          AND a.action = 'email.send.prepare'
          AND a.target_type = 'email_outbound_operation'
          AND a.target_id = NEW.operation_id::TEXT
          AND a.actor = NEW.actor_user_id
          AND a.xmin = pg_catalog.pg_current_xact_id()::xid
    ) <> 1 THEN
        RAISE EXCEPTION 'prepared mail requires one current-transaction audit by an active actor'
            USING ERRCODE = '23514';
    END IF;
    RETURN NULL;
END;
$function$;

REVOKE ALL ON FUNCTION public.require_email_outbound_preparation_audit()
    FROM PUBLIC, console_rt;

CREATE CONSTRAINT TRIGGER trg_email_outbound_preparation_audit_required
    AFTER INSERT ON public.email_outbound_operations
    DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW EXECUTE FUNCTION public.require_email_outbound_preparation_audit();

-- Existing rows must not be silently blessed by a future-only trigger. A
-- matching historical audit is still a claim, not authenticated provenance.
DO $check$
BEGIN
    IF EXISTS (
        SELECT 1 FROM public.email_outbound_operations o
        WHERE (
            SELECT count(*) FROM public.audit_events a
            WHERE a.org_id = o.org_id
              AND a.action = 'email.send.prepare'
              AND a.target_type = 'email_outbound_operation'
              AND a.target_id = o.operation_id::TEXT
              AND a.actor = o.actor_user_id
        ) <> 1
    ) THEN
        RAISE EXCEPTION 'existing prepared mail lacks exactly one matching audit'
            USING ERRCODE = '23514';
    END IF;
END;
$check$;
