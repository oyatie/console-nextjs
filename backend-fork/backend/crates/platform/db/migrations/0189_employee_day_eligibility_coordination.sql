-- Cross-domain employee/day serialization for attendance substitute eligibility.
-- The key is a compatibility contract shared by attendance and leave writers.
CREATE FUNCTION public.console_employee_day_eligibility_lock(
    p_org_id UUID,
    p_employee_id UUID,
    p_work_date DATE
) RETURNS VOID
LANGUAGE plpgsql
VOLATILE
PARALLEL UNSAFE
SET search_path = pg_catalog
AS $$
BEGIN
    PERFORM pg_catalog.pg_advisory_xact_lock(
        pg_catalog.hashtextextended(
            'attendance-substitution-eligibility-v1|' || p_org_id::TEXT || '|' ||
            p_employee_id::TEXT || '|' || pg_catalog.to_char(p_work_date, 'YYYY-MM-DD'),
            0
        )
    );
END;
$$;
REVOKE ALL ON FUNCTION public.console_employee_day_eligibility_lock(UUID, UUID, DATE) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION public.console_employee_day_eligibility_lock(UUID, UUID, DATE)
    TO console_rt, console_leave_definer;

CREATE FUNCTION public.console_attendance_exception_eligibility_lock()
RETURNS trigger
LANGUAGE plpgsql
SET search_path = pg_catalog
AS $$
BEGIN
    IF (TG_OP = 'INSERT' AND NEW.kind = 'NO_SHOW' AND NEW.status = 'OPEN')
       OR (TG_OP = 'UPDATE' AND OLD.kind = 'NO_SHOW' AND OLD.status = 'OPEN'
           AND NEW.status = 'RESOLVED') THEN
        PERFORM public.console_employee_day_eligibility_lock(NEW.org_id, NEW.employee_id, NEW.work_date);
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER trg_attendance_exceptions_eligibility_lock
    BEFORE INSERT OR UPDATE OF status ON public.attendance_exceptions
    FOR EACH ROW EXECUTE FUNCTION public.console_attendance_exception_eligibility_lock();

CREATE FUNCTION public.console_attendance_substitution_eligibility_guard()
RETURNS trigger
LANGUAGE plpgsql
SET search_path = pg_catalog
AS $$
BEGIN
    IF NEW.worker_employee_id IS NULL
       OR NOT ((TG_OP = 'INSERT' AND NEW.status = 'ASSIGNED')
               OR (TG_OP = 'UPDATE' AND OLD.status = 'ASSIGNED' AND NEW.status = 'CANCELLED')) THEN
        RETURN NEW;
    END IF;

    PERFORM public.console_employee_day_eligibility_lock(
        NEW.org_id, NEW.worker_employee_id, NEW.cover_date
    );

    IF TG_OP = 'INSERT' AND (
        EXISTS (
            SELECT 1
              FROM public.attendance_substitutions s
             WHERE s.org_id = NEW.org_id
               AND s.worker_employee_id = NEW.worker_employee_id
               AND s.cover_date = NEW.cover_date
               AND s.status = 'ASSIGNED'
               AND s.from_minutes < NEW.to_minutes
               AND s.to_minutes > NEW.from_minutes
        )
        OR EXISTS (
            SELECT 1
              FROM public.leave_requests lr
             WHERE lr.org_id = NEW.org_id
               AND lr.subject_employee_id = NEW.worker_employee_id
               AND lr.status = 'approved'
               AND NEW.cover_date BETWEEN lr.start_date AND lr.end_date
        )
        OR EXISTS (
            SELECT 1
              FROM public.attendance_exceptions e
             WHERE e.org_id = NEW.org_id
               AND e.employee_id = NEW.worker_employee_id
               AND e.work_date = NEW.cover_date
               AND e.kind = 'NO_SHOW'
               AND e.status = 'OPEN'
        )
    ) THEN
        RAISE EXCEPTION USING
            ERRCODE = '23514',
            MESSAGE = 'attendance_substitutions_worker_eligibility_guard';
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER trg_attendance_substitutions_eligibility_guard
    BEFORE INSERT OR UPDATE OF status ON public.attendance_substitutions
    FOR EACH ROW EXECUTE FUNCTION public.console_attendance_substitution_eligibility_guard();

CREATE FUNCTION public.console_leave_request_eligibility_lock()
RETURNS trigger
LANGUAGE plpgsql
SET search_path = pg_catalog
AS $$
DECLARE
    v_work_date DATE;
BEGIN
    IF OLD.status IS DISTINCT FROM NEW.status
       AND (OLD.status = 'approved' OR NEW.status = 'approved') THEN
        FOR v_work_date IN
            SELECT work_date::DATE
              FROM pg_catalog.generate_series(NEW.start_date, NEW.end_date, INTERVAL '1 day') AS work_date
             ORDER BY work_date
        LOOP
            PERFORM public.console_employee_day_eligibility_lock(
                NEW.org_id, NEW.subject_employee_id, v_work_date
            );
        END LOOP;
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER trg_leave_requests_eligibility_lock
    BEFORE UPDATE OF status ON public.leave_requests
    FOR EACH ROW EXECUTE FUNCTION public.console_leave_request_eligibility_lock();

CREATE OR REPLACE FUNCTION leave_api.decide_request(
    p_org_id UUID, p_request_id UUID, p_decider UUID, p_expected_version BIGINT,
    p_decision TEXT, p_comment TEXT, p_trace_id TEXT, p_span_id TEXT
) RETURNS TABLE(request_id UUID, request_version BIGINT,
                charge_version BIGINT, outcome TEXT)
LANGUAGE plpgsql SECURITY DEFINER
SET search_path = pg_catalog SET row_security = on AS $$
DECLARE
    v_request public.leave_requests%ROWTYPE;
    v_resolution public.leave_charge_resolutions%ROWTYPE;
    v_new_version BIGINT;
    v_new_status TEXT;
    v_ledger_before JSONB;
    v_ledger_after JSONB;
    v_work_date DATE;
BEGIN
    PERFORM leave_api.assert_context(p_org_id,p_decider,p_trace_id,p_span_id);
    IF p_decision NOT IN ('approve','return','reject')
       OR (p_decision IN ('return','reject') AND NULLIF(pg_catalog.btrim(p_comment),'') IS NULL)
       OR (p_comment IS NOT NULL AND char_length(pg_catalog.btrim(p_comment)) > 500) THEN
        RAISE EXCEPTION USING ERRCODE='22023', MESSAGE='leave_decide.invalid_decision';
    END IF;
    SELECT * INTO v_request FROM public.leave_requests lr
     WHERE lr.org_id=p_org_id AND lr.id=p_request_id FOR UPDATE;
    IF NOT FOUND THEN RAISE EXCEPTION USING ERRCODE='P0001', MESSAGE='leave_decide.not_found'; END IF;
    PERFORM leave_api.assert_manager(p_org_id,p_decider,v_request.branch_id);
    IF v_request.status <> 'pending' THEN
        RAISE EXCEPTION USING ERRCODE='P0001', MESSAGE='leave_decide.not_pending';
    END IF;
    IF p_expected_version IS NOT NULL
       AND v_request.request_version <> p_expected_version THEN
        RAISE EXCEPTION USING ERRCODE='40001', MESSAGE='leave_decide.concurrent_modification';
    END IF;
    IF v_request.requester_user_id=p_decider THEN
        RAISE EXCEPTION USING ERRCODE='42501', MESSAGE='leave_decide.requester_forbidden';
    END IF;

    IF p_decision='approve' THEN
        IF v_request.current_charge_resolution_id IS NOT NULL THEN
            PERFORM pg_catalog.pg_advisory_xact_lock(pg_catalog.hashtextextended(v_request.current_charge_resolution_id::TEXT, 166));
            SELECT * INTO v_resolution FROM public.leave_charge_resolutions r
             WHERE r.org_id=p_org_id AND r.id=v_request.current_charge_resolution_id
               AND r.request_id=p_request_id;
        END IF;
        IF v_request.charge_state <> 'resolved' OR NOT FOUND
           OR v_resolution.home_branch_id IS DISTINCT FROM v_request.branch_id
           OR v_resolution.charge_units IS DISTINCT FROM v_request.charge_units
           OR v_resolution.charge_version IS DISTINCT FROM v_request.charge_version THEN
            INSERT INTO public.audit_events
                (actor,action,target_type,target_id,branch_id,before_snap,after_snap,
                 trace_id,span_id,occurred_at,org_id)
            VALUES
                (p_decider,'leave_request.approval_blocked','leave_request',p_request_id::TEXT,
                 v_request.branch_id,
                 pg_catalog.jsonb_build_object('status',v_request.status,
                    'charge_state',v_request.charge_state,
                    'request_version',v_request.request_version,
                    'charge_version',v_request.charge_version),
                 pg_catalog.jsonb_build_object('outcome','blocked_no_mutation'),
                 p_trace_id,p_span_id,pg_catalog.statement_timestamp(),p_org_id);
            RETURN QUERY SELECT p_request_id,v_request.request_version,
                v_request.charge_version,'charge_review_required'::TEXT;
            RETURN;
        END IF;
        IF v_resolution.resolved_by=p_decider THEN
            RAISE EXCEPTION USING ERRCODE='42501', MESSAGE='leave_decide.resolver_forbidden';
        END IF;
        FOR v_work_date IN
            SELECT work_date::DATE
              FROM pg_catalog.generate_series(v_request.start_date, v_request.end_date, INTERVAL '1 day') AS work_date
             ORDER BY work_date
        LOOP
            PERFORM public.console_employee_day_eligibility_lock(
                v_request.org_id, v_request.subject_employee_id, v_work_date
            );
        END LOOP;
        SELECT pg_catalog.jsonb_build_object('leave_used',e.leave_used,
                   'leave_remaining',e.leave_remaining)
          INTO v_ledger_before
          FROM public.employees e
         WHERE e.org_id=p_org_id AND e.id=v_request.subject_employee_id
         FOR UPDATE;
        IF NOT FOUND THEN
            RAISE EXCEPTION USING ERRCODE='P0001', MESSAGE='leave_decide.employee_not_found';
        END IF;
        UPDATE public.employees e
           SET leave_used=COALESCE(e.leave_used,0)+v_resolution.charge_units,
               leave_remaining=COALESCE(e.leave_remaining,0)-v_resolution.charge_units,
               updated_at=pg_catalog.now()
         WHERE e.org_id=p_org_id AND e.id=v_request.subject_employee_id
           AND COALESCE(e.leave_remaining,0)>=v_resolution.charge_units
        RETURNING pg_catalog.jsonb_build_object('leave_used',e.leave_used,
                    'leave_remaining',e.leave_remaining) INTO v_ledger_after;
        IF NOT FOUND THEN
            RAISE EXCEPTION USING ERRCODE='22023', MESSAGE='leave_decide.insufficient_balance';
        END IF;
    END IF;

    v_new_version := v_request.request_version + 1;
    IF v_new_version <= v_request.request_version THEN
        RAISE EXCEPTION USING ERRCODE='22003', MESSAGE='leave_decide.version_exhausted';
    END IF;
    v_new_status := CASE p_decision WHEN 'approve' THEN 'approved'
                         WHEN 'return' THEN 'returned' ELSE 'rejected' END;
    UPDATE public.leave_requests
       SET status=v_new_status, decided_by=p_decider, decided_at=pg_catalog.statement_timestamp(),
           decision_comment=NULLIF(pg_catalog.btrim(p_comment),''),
           charge_state=CASE WHEN p_decision='approve' THEN charge_state ELSE 'not_required' END,
           charge_review_reasons=CASE WHEN p_decision='approve' THEN charge_review_reasons ELSE ARRAY[]::TEXT[] END,
           charge_units=CASE WHEN p_decision='approve' THEN charge_units ELSE NULL END,
           current_charge_resolution_id=CASE WHEN p_decision='approve' THEN current_charge_resolution_id ELSE NULL END,
           request_version=v_new_version
     WHERE org_id=p_org_id AND id=p_request_id;
    INSERT INTO public.audit_events
        (actor,action,target_type,target_id,branch_id,before_snap,after_snap,
         trace_id,span_id,occurred_at,org_id)
    VALUES
        (p_decider,'leave_request.decide','leave_request',p_request_id::TEXT,
         v_request.branch_id,
         pg_catalog.jsonb_build_object('status',v_request.status,
             'charge_state',v_request.charge_state,
             'request_version',v_request.request_version,
             'charge_version',v_request.charge_version,
             'ledger',v_ledger_before),
         pg_catalog.jsonb_build_object('status',v_new_status,'decision',p_decision,
             'request_version',v_new_version,
             'charge_version',v_request.charge_version,'ledger',v_ledger_after),
         p_trace_id,p_span_id,pg_catalog.statement_timestamp(),p_org_id);
    RETURN QUERY SELECT p_request_id,v_new_version,v_request.charge_version,'decided'::TEXT;
END;
$$;
ALTER FUNCTION leave_api.decide_request(UUID, UUID, UUID, BIGINT, TEXT, TEXT, TEXT, TEXT)
    OWNER TO console_leave_definer;
REVOKE ALL ON FUNCTION leave_api.decide_request(UUID, UUID, UUID, BIGINT, TEXT, TEXT, TEXT, TEXT)
    FROM PUBLIC, console_rt;
GRANT EXECUTE ON FUNCTION leave_api.decide_request(UUID, UUID, UUID, BIGINT, TEXT, TEXT, TEXT, TEXT)
    TO console_leave_cmd;
