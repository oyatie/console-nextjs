-- Expand only. The dormant auth command may append one fixed admission audit
-- after proving that the consumed source, enrollment family and committed
-- admission agree. It never receives direct INSERT on the general audit log.
-- The admission row lock serializes repeated calls without an online-index
-- build on the potentially large live audit table.

-- No earlier owner could complete this v1 transaction. An already consumed
-- proof, enrollment family or committed admission requires reconciliation;
-- installing future-only triggers must not silently bless it on upgrade.
DO $block$
BEGIN
    IF EXISTS (
        SELECT 1 FROM public.auth_bootstrap_credentials
        WHERE issuance_version = 1 AND consumed_at IS NOT NULL
    ) OR EXISTS (
        SELECT 1 FROM public.auth_refresh_token_families
        WHERE provenance_version = 1 AND session_purpose = 'enrollment'
    ) OR EXISTS (
        SELECT 1 FROM auth_security.session_admissions
        WHERE session_purpose = 'enrollment' AND status = 'committed'
    ) THEN
        RAISE EXCEPTION 'first enrollment v1 effects require reconciliation before owner migration'
            USING ERRCODE = '23514';
    END IF;
END;
$block$;

CREATE FUNCTION auth_security.append_first_enrollment_audit(p_operation UUID)
RETURNS UUID
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog
SET row_security = on
AS $function$
DECLARE
    admission auth_security.session_admissions%ROWTYPE;
    event_id UUID := pg_catalog.gen_random_uuid();
    company UUID := NULLIF(pg_catalog.current_setting('app.current_org', true), '')::UUID;
BEGIN
    IF company IS NULL THEN
        RAISE EXCEPTION 'first enrollment audit requires Company context'
            USING ERRCODE = '42501';
    END IF;

    SELECT * INTO admission
    FROM auth_security.session_admissions a
    WHERE a.operation_id = p_operation
      AND a.home_org_id = company
      AND a.status = 'committed'
    FOR UPDATE;
    IF NOT FOUND OR NOT EXISTS (
        SELECT 1
        FROM public.auth_bootstrap_credentials b
        JOIN public.auth_refresh_token_families f
          ON f.id = admission.family_id
         AND f.user_id = admission.account_id
         AND f.org_id = admission.home_org_id
        WHERE b.id = admission.source_row_id
          AND b.user_id = admission.account_id
          AND b.org_id = admission.home_org_id
          AND b.issuance_version = 1
          AND b.issued_generation = admission.auth_generation
          AND b.issuance_purpose = 'first_enrollment'
          AND b.source_operation_id = admission.source_operation_id
          AND b.consumed_at IS NOT NULL
          AND f.provenance_version = 1
          AND f.auth_generation = admission.auth_generation
          AND f.session_purpose = 'enrollment'
          AND f.source_kind = 'bootstrap_otp'
          AND f.source_operation_id = admission.operation_id
          AND EXISTS (
              SELECT 1 FROM public.auth_refresh_tokens t
              WHERE t.family_id = f.id
                AND t.user_id = admission.account_id
                AND t.org_id = admission.home_org_id
          )
    ) THEN
        RAISE EXCEPTION 'first enrollment audit evidence mismatch'
            USING ERRCODE = '42501';
    END IF;
    IF EXISTS (
        SELECT 1 FROM public.audit_events a
        WHERE a.action = 'auth.first_enrollment.admit'
          AND a.target_type = 'auth_security.session_admission'
          AND a.target_id = admission.operation_id::TEXT
    ) THEN
        RAISE EXCEPTION 'first enrollment audit already appended'
            USING ERRCODE = '23505';
    END IF;

    INSERT INTO public.audit_events (
        id, actor, action, target_type, target_id, after_snap,
        trace_id, span_id, occurred_at, org_id
    ) VALUES (
        event_id, admission.account_id, 'auth.first_enrollment.admit',
        'auth_security.session_admission', admission.operation_id::TEXT,
        pg_catalog.jsonb_build_object(
            'source_id', admission.source_row_id,
            'source_operation_id', admission.source_operation_id,
            'family_id', admission.family_id,
            'receipt_id', admission.receipt_id,
            'auth_generation', admission.auth_generation,
            'purpose', admission.session_purpose
        ),
        pg_catalog.replace(pg_catalog.gen_random_uuid()::TEXT, '-', ''),
        pg_catalog.substring(pg_catalog.replace(pg_catalog.gen_random_uuid()::TEXT, '-', ''), 1, 16),
        pg_catalog.clock_timestamp(), admission.home_org_id
    );
    RETURN event_id;
END;
$function$;

REVOKE ALL ON FUNCTION auth_security.append_first_enrollment_audit(UUID)
    FROM PUBLIC, console_rt;
GRANT EXECUTE ON FUNCTION auth_security.append_first_enrollment_audit(UUID)
    TO console_auth_cmd;

COMMENT ON FUNCTION auth_security.append_first_enrollment_audit(UUID) IS
    'Append one fixed, source-bound first-enrollment admission audit under current Company; no general audit write grant';

-- Direct callers of the low-level OTP/family/admission helpers must not commit
-- any one part without the same consumed source, bearer family, token and bound
-- audit. Check at COMMIT so the owner can build all parts in one transaction
-- without exposing a half-finished row between statements.
CREATE FUNCTION auth_security.require_first_enrollment_admission()
RETURNS TRIGGER
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog
SET row_security = on
AS $function$
DECLARE
    target_source_id UUID;
    target_family_id UUID;
    target_operation_id UUID;
BEGIN
    IF TG_TABLE_NAME = 'auth_bootstrap_credentials' THEN
        target_source_id := NEW.id;
    ELSIF TG_TABLE_NAME = 'auth_refresh_token_families' THEN
        target_family_id := NEW.id;
    ELSIF TG_TABLE_NAME = 'session_admissions' THEN
        target_operation_id := NEW.operation_id;
    ELSE
        RAISE EXCEPTION 'unsupported first enrollment evidence table'
            USING ERRCODE = '23514';
    END IF;
    IF NOT EXISTS (
        SELECT 1
        FROM auth_security.session_admissions a
        JOIN public.auth_bootstrap_credentials b
          ON b.id = a.source_row_id
         AND b.user_id = a.account_id
         AND b.org_id = a.home_org_id
        JOIN public.auth_refresh_token_families f
          ON f.id = a.family_id
         AND f.user_id = a.account_id
         AND f.org_id = a.home_org_id
        WHERE a.status = 'committed'
          AND a.session_purpose = 'enrollment'
          AND a.source_kind = 'bootstrap_otp'
          AND a.source_operation_id = b.source_operation_id
          AND a.auth_generation = b.issued_generation
          AND a.auth_generation = f.auth_generation
          AND a.operation_id = f.source_operation_id
          AND b.issuance_version = 1
          AND b.issuance_purpose = 'first_enrollment'
          AND b.consumed_at IS NOT NULL
          AND f.provenance_version = 1
          AND f.session_purpose = 'enrollment'
          AND f.source_kind = 'bootstrap_otp'
          AND (target_source_id IS NULL OR b.id = target_source_id)
          AND (target_family_id IS NULL OR f.id = target_family_id)
          AND (target_operation_id IS NULL OR a.operation_id = target_operation_id)
          AND EXISTS (
              SELECT 1 FROM public.auth_refresh_tokens t
              WHERE t.family_id = f.id
                AND t.user_id = a.account_id
                AND t.org_id = a.home_org_id
          )
          AND EXISTS (
              SELECT 1 FROM public.audit_events e
              WHERE e.action = 'auth.first_enrollment.admit'
                AND e.target_type = 'auth_security.session_admission'
                AND e.target_id = a.operation_id::TEXT
                AND e.org_id = a.home_org_id
          )
    ) THEN
        RAISE EXCEPTION 'version-1 enrollment source or family lacks committed admission'
            USING ERRCODE = '23514';
    END IF;
    RETURN NULL;
END;
$function$;

CREATE CONSTRAINT TRIGGER first_enrollment_source_insert_complete
AFTER INSERT ON public.auth_bootstrap_credentials
DEFERRABLE INITIALLY DEFERRED
FOR EACH ROW
WHEN (NEW.issuance_version = 1 AND NEW.consumed_at IS NOT NULL)
EXECUTE FUNCTION auth_security.require_first_enrollment_admission();

CREATE CONSTRAINT TRIGGER first_enrollment_source_consume_complete
AFTER UPDATE ON public.auth_bootstrap_credentials
DEFERRABLE INITIALLY DEFERRED
FOR EACH ROW
WHEN (NEW.issuance_version = 1 AND OLD.consumed_at IS NULL
      AND NEW.consumed_at IS NOT NULL)
EXECUTE FUNCTION auth_security.require_first_enrollment_admission();

CREATE CONSTRAINT TRIGGER first_enrollment_family_complete
AFTER INSERT ON public.auth_refresh_token_families
DEFERRABLE INITIALLY DEFERRED
FOR EACH ROW
WHEN (NEW.provenance_version = 1 AND NEW.session_purpose = 'enrollment')
EXECUTE FUNCTION auth_security.require_first_enrollment_admission();

-- The source/family triggers do not fire when a caller forges only the private
-- receipt row. The trusted restore owner may INSERT an accepted snapshot, but
-- every serving pending-to-committed transition must close this evidence set.
CREATE CONSTRAINT TRIGGER first_enrollment_admission_complete
AFTER UPDATE ON auth_security.session_admissions
DEFERRABLE INITIALLY DEFERRED
FOR EACH ROW
WHEN (OLD.status = 'pending' AND NEW.status = 'committed'
      AND NEW.session_purpose = 'enrollment')
EXECUTE FUNCTION auth_security.require_first_enrollment_admission();

REVOKE ALL ON FUNCTION auth_security.require_first_enrollment_admission()
    FROM PUBLIC, console_rt, console_auth_cmd;

COMMENT ON FUNCTION auth_security.require_first_enrollment_admission() IS
    'Deferred evidence closure for consumed version-1 first-enrollment proofs, families and committed admissions';

-- A consumed version-1 bearer proof cannot be made open again. Version-0
-- issuance/replacement semantics remain under their existing owner.
CREATE FUNCTION auth_security.keep_v1_source_consumed()
RETURNS TRIGGER
LANGUAGE plpgsql
SET search_path = pg_catalog
AS $function$
BEGIN
    IF OLD.issuance_version = 1
       AND OLD.consumed_at IS NOT NULL
       AND NEW.consumed_at IS DISTINCT FROM OLD.consumed_at THEN
        RAISE EXCEPTION 'version-1 bootstrap consumption is irreversible'
            USING ERRCODE = '23514';
    END IF;
    RETURN NEW;
END;
$function$;

CREATE TRIGGER first_enrollment_source_consumption_monotonic
BEFORE UPDATE ON public.auth_bootstrap_credentials
FOR EACH ROW
EXECUTE FUNCTION auth_security.keep_v1_source_consumed();

REVOKE ALL ON FUNCTION auth_security.keep_v1_source_consumed()
    FROM PUBLIC, console_rt, console_auth_cmd;
