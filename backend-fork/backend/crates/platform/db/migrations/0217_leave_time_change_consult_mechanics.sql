-- 근로기준법 §60⑤ consult mechanics (charter §4-31, bead console-we1):
--   · 요건 자동 판정 — time_change is refused unless branch coverage evidence
--     shows granting the request would leave fewer than minimum_on_duty (1)
--     ACTIVE home-branch employees available (employment_status='ACTIVE' only;
--     EXITED stamps must not inflate headcount);
--   · 대체 일자는 근로자 선택 — only the original requester may write
--     alternate_* columns on a time_change_consult row;
--   · 반복 행사=감사 집계 — the Nth exercise (N≥2) in the same leave-year
--     for a subject emits leave_request.time_change_repeat_audit.
--
-- cm3 (0216) shipped the fail-closed reshape (no reason / no refusal /
-- time_change→time_change_consult). This migration completes the consult
-- workflow that 0216 deliberately deferred.

-- ---------------------------------------------------------------------------
-- 1. Consult columns on leave_requests.
-- ---------------------------------------------------------------------------
ALTER TABLE leave_requests
    ADD COLUMN time_change_grounds TEXT
        CHECK (
            time_change_grounds IS NULL
            OR time_change_grounds IN ('branch_coverage_shortfall')
        ),
    ADD COLUMN time_change_evidence JSONB,
    ADD COLUMN alternate_start_date DATE,
    ADD COLUMN alternate_end_date DATE,
    ADD COLUMN alternate_partial_day_period TEXT
        CHECK (
            alternate_partial_day_period IS NULL
            OR alternate_partial_day_period IN ('am', 'pm')
        ),
    ADD COLUMN alternate_proposed_at TIMESTAMPTZ;

ALTER TABLE leave_requests
    ADD CONSTRAINT leave_requests_time_change_grounds_atomic
        CHECK (
            (time_change_grounds IS NULL) = (time_change_evidence IS NULL)
        ),
    ADD CONSTRAINT leave_requests_alternate_dates_atomic
        CHECK (
            (alternate_start_date IS NULL) = (alternate_end_date IS NULL)
            AND (alternate_start_date IS NULL) = (alternate_proposed_at IS NULL)
            AND (
                alternate_start_date IS NULL
                OR alternate_end_date >= alternate_start_date
            )
        ),
    ADD CONSTRAINT leave_requests_time_change_fields_status
        CHECK (
            status = 'time_change_consult'
            OR (
                time_change_grounds IS NULL
                AND alternate_start_date IS NULL
            )
        );

-- Personal-data classification for post-0209 columns on baselined leave_requests.
-- leave_requests rows identify a natural person (subject_employee_id); dates/grounds
-- are personal by 개보법 제2조제1호나목 combination. Evidence JSONB is undeclared.
COMMENT ON COLUMN leave_requests.time_change_grounds IS 'pd:personal — §60⑤ 시기변경 협의 사유 (branch_coverage_shortfall); row identifies subject employee';
COMMENT ON COLUMN leave_requests.time_change_evidence IS 'pd:undeclared — §60⑤ coverage evidence JSONB; schema not fully bounded';
COMMENT ON COLUMN leave_requests.alternate_start_date IS 'pd:personal — worker-proposed alternate leave start; row identifies subject employee';
COMMENT ON COLUMN leave_requests.alternate_end_date IS 'pd:personal — worker-proposed alternate leave end; row identifies subject employee';
COMMENT ON COLUMN leave_requests.alternate_partial_day_period IS 'pd:personal — worker-proposed am/pm partial-day period; row identifies subject employee';
COMMENT ON COLUMN leave_requests.alternate_proposed_at IS 'pd:personal — timestamp of alternate-date proposal; row identifies subject employee';

-- ---------------------------------------------------------------------------
-- 2. decide_request: gate time_change on automatic coverage eligibility and
--    emit the repeat-audit rollup when the leave-year exercise count ≥ 2.
-- ---------------------------------------------------------------------------
CREATE OR REPLACE FUNCTION leave_api.decide_request(
    p_org_id UUID, p_request_id UUID, p_decider UUID,
    p_expected_version BIGINT, p_decision TEXT, p_comment TEXT,
    p_trace_id TEXT, p_span_id TEXT
) RETURNS TABLE(request_id UUID, request_version BIGINT, charge_version BIGINT, outcome TEXT)
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
    v_headcount INT;
    v_already_out INT;
    v_minimum_on_duty INT := 1;
    v_projected BIGINT;
    v_grounds TEXT;
    v_evidence JSONB;
    v_exercises INT;
    v_leave_year INT;
BEGIN
    PERFORM leave_api.assert_context(p_org_id,p_decider,p_trace_id,p_span_id);
    IF p_decision NOT IN ('approve','time_change')
       OR (p_decision = 'time_change' AND NULLIF(pg_catalog.btrim(p_comment),'') IS NULL)
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
    ELSE
        -- §60⑤ 요건 자동 판정 (domain::judge_time_change_eligibility).
        -- Headcount is ACTIVE roster only — EXITED rows retaining home_branch_id
        -- must not inflate coverage and refuse a real shortfall (console-we1 critic).
        SELECT count(*)::INT INTO v_headcount
          FROM public.employees e
         WHERE e.org_id = p_org_id
           AND e.home_branch_id = v_request.branch_id
           AND e.employment_status = 'ACTIVE';
        SELECT count(DISTINCT lr.subject_employee_id)::INT INTO v_already_out
          FROM public.leave_requests lr
         WHERE lr.org_id = p_org_id
           AND lr.branch_id = v_request.branch_id
           AND lr.status = 'approved'
           AND lr.subject_employee_id IS DISTINCT FROM v_request.subject_employee_id
           AND lr.start_date <= v_request.end_date
           AND lr.end_date >= v_request.start_date;
        v_projected := v_headcount::BIGINT - v_already_out::BIGINT - 1;
        v_evidence := pg_catalog.jsonb_build_object(
            'headcount', v_headcount,
            'already_out', v_already_out,
            'minimum_on_duty', v_minimum_on_duty,
            'projected_available', v_projected
        );
        IF v_headcount = 0 OR v_projected >= v_minimum_on_duty THEN
            RAISE EXCEPTION USING ERRCODE='22023',
                MESSAGE='leave_decide.time_change_ineligible';
        END IF;
        v_grounds := 'branch_coverage_shortfall';
    END IF;

    v_new_version := v_request.request_version + 1;
    IF v_new_version <= v_request.request_version THEN
        RAISE EXCEPTION USING ERRCODE='22003', MESSAGE='leave_decide.version_exhausted';
    END IF;
    v_new_status := CASE p_decision WHEN 'approve' THEN 'approved'
                         ELSE 'time_change_consult' END;
    UPDATE public.leave_requests
       SET status=v_new_status, decided_by=p_decider, decided_at=pg_catalog.statement_timestamp(),
           decision_comment=NULLIF(pg_catalog.btrim(p_comment),''),
           charge_state=CASE WHEN p_decision='approve' THEN charge_state ELSE 'not_required' END,
           charge_review_reasons=CASE WHEN p_decision='approve' THEN charge_review_reasons ELSE ARRAY[]::TEXT[] END,
           charge_units=CASE WHEN p_decision='approve' THEN charge_units ELSE NULL END,
           current_charge_resolution_id=CASE WHEN p_decision='approve' THEN current_charge_resolution_id ELSE NULL END,
           time_change_grounds=CASE WHEN p_decision='time_change' THEN v_grounds ELSE NULL END,
           time_change_evidence=CASE WHEN p_decision='time_change' THEN v_evidence ELSE NULL END,
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
             'charge_version',v_request.charge_version,'ledger',v_ledger_after,
             'time_change_grounds',v_grounds,
             'time_change_evidence',v_evidence),
         p_trace_id,p_span_id,pg_catalog.statement_timestamp(),p_org_id);

    IF p_decision = 'time_change' THEN
        v_leave_year := pg_catalog.date_part('year', v_request.start_date)::INT;
        SELECT count(*)::INT INTO v_exercises
          FROM public.leave_requests lr
         WHERE lr.org_id = p_org_id
           AND lr.subject_employee_id = v_request.subject_employee_id
           AND lr.status = 'time_change_consult'
           AND pg_catalog.date_part('year', lr.start_date)::INT = v_leave_year;
        IF v_exercises >= 2 THEN
            INSERT INTO public.audit_events
                (actor,action,target_type,target_id,branch_id,before_snap,after_snap,
                 trace_id,span_id,occurred_at,org_id)
            VALUES
                (p_decider,'leave_request.time_change_repeat_audit','leave_request',
                 p_request_id::TEXT, v_request.branch_id,
                 pg_catalog.jsonb_build_object('leave_year', v_leave_year),
                 pg_catalog.jsonb_build_object(
                    'subject_employee_id', v_request.subject_employee_id,
                    'leave_year', v_leave_year,
                    'exercises_in_leave_year', v_exercises,
                    'audit_flag', true,
                    'grounds', v_grounds
                 ),
                 p_trace_id,p_span_id,pg_catalog.statement_timestamp(),p_org_id);
        END IF;
    END IF;

    RETURN QUERY SELECT p_request_id,v_new_version,v_request.charge_version,'decided'::TEXT;
END;
$$;
ALTER FUNCTION leave_api.decide_request(UUID, UUID, UUID, BIGINT, TEXT, TEXT, TEXT, TEXT)
    OWNER TO console_leave_definer;
REVOKE ALL ON FUNCTION leave_api.decide_request(UUID, UUID, UUID, BIGINT, TEXT, TEXT, TEXT, TEXT)
    FROM PUBLIC, console_rt;
GRANT EXECUTE ON FUNCTION leave_api.decide_request(UUID, UUID, UUID, BIGINT, TEXT, TEXT, TEXT, TEXT)
    TO console_leave_cmd;

-- ---------------------------------------------------------------------------
-- 3. Worker-only alternate-date proposal.
-- ---------------------------------------------------------------------------
CREATE FUNCTION leave_api.propose_alternate_dates(
    p_org_id UUID, p_request_id UUID, p_proposer UUID,
    p_start_date DATE, p_end_date DATE,
    p_trace_id TEXT, p_span_id TEXT
) RETURNS TABLE(
    request_id UUID,
    request_version BIGINT,
    alternate_start_date DATE,
    alternate_end_date DATE,
    alternate_partial_day_period TEXT,
    alternate_proposed_at TIMESTAMPTZ
)
LANGUAGE plpgsql SECURITY DEFINER
SET search_path = pg_catalog SET row_security = on AS $$
DECLARE
    v_request public.leave_requests%ROWTYPE;
    v_proposed_at TIMESTAMPTZ := pg_catalog.statement_timestamp();
BEGIN
    PERFORM leave_api.assert_context(p_org_id,p_proposer,p_trace_id,p_span_id);
    IF p_end_date < p_start_date THEN
        RAISE EXCEPTION USING ERRCODE='22023', MESSAGE='leave_alternate.invalid_dates';
    END IF;
    IF (p_end_date - p_start_date) >= 366 THEN
        RAISE EXCEPTION USING ERRCODE='22023', MESSAGE='leave_alternate.invalid_dates';
    END IF;
    SELECT * INTO v_request FROM public.leave_requests lr
     WHERE lr.org_id=p_org_id AND lr.id=p_request_id FOR UPDATE;
    IF NOT FOUND THEN
        RAISE EXCEPTION USING ERRCODE='P0001', MESSAGE='leave_alternate.not_found';
    END IF;
    IF v_request.status <> 'time_change_consult' THEN
        RAISE EXCEPTION USING ERRCODE='P0001', MESSAGE='leave_alternate.not_consult';
    END IF;
    IF v_request.requester_user_id <> p_proposer THEN
        -- 대체 일자는 근로자 선택 — employer/manager writes are unrepresentable.
        RAISE EXCEPTION USING ERRCODE='42501', MESSAGE='leave_alternate.requester_only';
    END IF;
    IF v_request.alternate_start_date IS NOT NULL THEN
        RAISE EXCEPTION USING ERRCODE='P0001', MESSAGE='leave_alternate.already_proposed';
    END IF;
    IF p_start_date = v_request.start_date AND p_end_date = v_request.end_date THEN
        RAISE EXCEPTION USING ERRCODE='22023', MESSAGE='leave_alternate.same_as_original';
    END IF;
    IF v_request.leave_type = 'half_day' AND p_start_date <> p_end_date THEN
        RAISE EXCEPTION USING ERRCODE='22023', MESSAGE='leave_alternate.invalid_dates';
    END IF;

    UPDATE public.leave_requests
       SET alternate_start_date = p_start_date,
           alternate_end_date = p_end_date,
           alternate_partial_day_period = v_request.partial_day_period,
           alternate_proposed_at = v_proposed_at,
           request_version = v_request.request_version + 1
     WHERE org_id = p_org_id AND id = p_request_id
    RETURNING * INTO v_request;

    INSERT INTO public.audit_events
        (actor,action,target_type,target_id,branch_id,before_snap,after_snap,
         trace_id,span_id,occurred_at,org_id)
    VALUES
        (p_proposer,'leave_request.alternate_dates_proposed','leave_request',
         p_request_id::TEXT, v_request.branch_id,
         pg_catalog.jsonb_build_object(
            'start_date', v_request.start_date,
            'end_date', v_request.end_date),
         pg_catalog.jsonb_build_object(
            'alternate_start_date', p_start_date,
            'alternate_end_date', p_end_date,
            'alternate_partial_day_period', v_request.partial_day_period,
            'request_version', v_request.request_version),
         p_trace_id,p_span_id,v_proposed_at,p_org_id);

    RETURN QUERY SELECT
        v_request.id,
        v_request.request_version,
        v_request.alternate_start_date,
        v_request.alternate_end_date,
        v_request.alternate_partial_day_period,
        v_request.alternate_proposed_at;
END;
$$;
ALTER FUNCTION leave_api.propose_alternate_dates(UUID, UUID, UUID, DATE, DATE, TEXT, TEXT)
    OWNER TO console_leave_definer;
REVOKE ALL ON FUNCTION leave_api.propose_alternate_dates(UUID, UUID, UUID, DATE, DATE, TEXT, TEXT)
    FROM PUBLIC, console_rt;
GRANT EXECUTE ON FUNCTION leave_api.propose_alternate_dates(UUID, UUID, UUID, DATE, DATE, TEXT, TEXT)
    TO console_leave_cmd;

-- ---------------------------------------------------------------------------
-- 4. Narrow the console_rt bridge so new consult columns stay command-only.
-- ---------------------------------------------------------------------------
CREATE OR REPLACE FUNCTION leave_api.protected_request_writer_guard()
RETURNS TRIGGER LANGUAGE plpgsql SET search_path = pg_catalog AS $$
BEGIN
    IF current_user = 'console_rt' AND TG_OP = 'INSERT' THEN
        IF NEW.days IS NULL
           OR NEW.status <> 'pending'
           OR NEW.decided_by IS NOT NULL
           OR NEW.decided_at IS NOT NULL
           OR NEW.decision_comment IS NOT NULL
           OR NEW.ap_run_id IS NOT NULL
           OR NEW.partial_day_period IS NOT NULL
           OR NEW.charge_state <> 'review_required'
           OR NEW.charge_review_reasons <> ARRAY['missing_calendar']::TEXT[]
           OR NEW.charge_units IS NOT NULL
           OR NEW.submission_key IS NOT NULL
           OR NEW.submission_digest IS NOT NULL
           OR NEW.submission_initial_charge_version IS NOT NULL
           OR NEW.request_version <> 1
           OR NEW.charge_version <> 0
           OR NEW.current_charge_resolution_id IS NOT NULL
           OR NEW.time_change_grounds IS NOT NULL
           OR NEW.time_change_evidence IS NOT NULL
           OR NEW.alternate_start_date IS NOT NULL
           OR NEW.alternate_end_date IS NOT NULL
           OR NEW.alternate_partial_day_period IS NOT NULL
           OR NEW.alternate_proposed_at IS NOT NULL THEN
            RAISE EXCEPTION USING ERRCODE = '42501', MESSAGE = 'leave_write.command_required';
        END IF;
        RETURN NEW;
    ELSIF current_user = 'console_rt' AND TG_OP = 'UPDATE' THEN
        IF OLD.status <> 'pending'
           OR NEW.status <> 'approved'
           OR NEW.status IS NOT DISTINCT FROM OLD.status
           OR NEW.decided_by IS NULL
           OR NEW.decided_at IS NULL
           OR NEW.requester_user_id = NEW.decided_by
           OR NEW.id IS DISTINCT FROM OLD.id
           OR NEW.org_id IS DISTINCT FROM OLD.org_id
           OR NEW.branch_id IS DISTINCT FROM OLD.branch_id
           OR NEW.requester_user_id IS DISTINCT FROM OLD.requester_user_id
           OR NEW.subject_employee_id IS DISTINCT FROM OLD.subject_employee_id
           OR NEW.leave_type IS DISTINCT FROM OLD.leave_type
           OR NEW.days IS DISTINCT FROM OLD.days
           OR NEW.start_date IS DISTINCT FROM OLD.start_date
           OR NEW.end_date IS DISTINCT FROM OLD.end_date
           OR NEW.reason IS DISTINCT FROM OLD.reason
           OR NEW.ap_run_id IS DISTINCT FROM OLD.ap_run_id
           OR NEW.partial_day_period IS DISTINCT FROM OLD.partial_day_period
           OR NEW.charge_state IS DISTINCT FROM OLD.charge_state
           OR NEW.charge_review_reasons IS DISTINCT FROM OLD.charge_review_reasons
           OR NEW.charge_units IS DISTINCT FROM OLD.charge_units
           OR NEW.submission_key IS DISTINCT FROM OLD.submission_key
           OR NEW.submission_digest IS DISTINCT FROM OLD.submission_digest
           OR NEW.submission_initial_charge_version IS DISTINCT FROM OLD.submission_initial_charge_version
           OR NEW.request_version IS DISTINCT FROM OLD.request_version
           OR NEW.charge_version IS DISTINCT FROM OLD.charge_version
           OR NEW.current_charge_resolution_id IS DISTINCT FROM OLD.current_charge_resolution_id
           OR NEW.time_change_grounds IS DISTINCT FROM OLD.time_change_grounds
           OR NEW.time_change_evidence IS DISTINCT FROM OLD.time_change_evidence
           OR NEW.alternate_start_date IS DISTINCT FROM OLD.alternate_start_date
           OR NEW.alternate_end_date IS DISTINCT FROM OLD.alternate_end_date
           OR NEW.alternate_partial_day_period IS DISTINCT FROM OLD.alternate_partial_day_period
           OR NEW.alternate_proposed_at IS DISTINCT FROM OLD.alternate_proposed_at THEN
            RAISE EXCEPTION USING ERRCODE = '42501', MESSAGE = 'leave_write.command_required';
        END IF;
        NEW.request_version := OLD.request_version + 1;
        NEW.charge_state := 'legacy_unverified';
        NEW.charge_review_reasons := ARRAY[]::TEXT[];
        NEW.charge_units := OLD.days;
        RETURN NEW;
    ELSIF current_user <> 'console_leave_definer' THEN
        RAISE EXCEPTION USING ERRCODE = '42501', MESSAGE = 'leave_write.command_required';
    END IF;
    RETURN NEW;
END;
$$;
ALTER FUNCTION leave_api.protected_request_writer_guard() OWNER TO console_leave_definer;
