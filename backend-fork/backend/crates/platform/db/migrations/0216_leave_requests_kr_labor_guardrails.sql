-- 근로기준법 §60 guardrail for leave_requests (charter §4-31, bead console-cm3):
-- "연차=사유란 없음·거부 불가 (시기변경 협의만)".
--
-- This table holds only 연차 variants (0122: leave_type IN ('annual','half_day')),
-- so the whole table is subject to the guardrail:
--
--   1. NO REASON. 0122 shipped `reason TEXT NOT NULL CHECK (btrim 1..500)` and
--      leave_api.create_request (0166) enforced it again — an employee could
--      not file 연차 without stating a 사유, which §60 forbids (the worker
--      holds the 시기지정권) and which leaks refusal grounds into appraisal
--      surfaces ("거절이 평가·배정에 불이익으로 이어지면 안 된다").
--   2. NO REFUSAL. 0122 shipped 'returned'/'rejected' terminal statuses and
--      0189's decide_request produced them. A refused 연차 request is not a
--      legal state; the §60⑤ proviso only lets the employer open a 시기변경
--      협의 (schedule-change consultation).
--
-- Retention ruling (보관=숨김, charter §3.9): historical rows KEEP their
-- stored reason and their 'returned'/'rejected' statuses — hard deletion is
-- itself governed and this migration destroys no records. The guardrail is a
-- one-way ratchet on NEW writes, enforced fail-closed at three layers that do
-- not trust each other: the REST/domain types (no reason slot, no refusal
-- decision), the leave_api command functions (replaced below), and the
-- trigger guard (below), which binds even console_leave_definer so a future
-- buggy definer routine cannot reintroduce the forbidden states.
--
-- The only lawful non-approve decision becomes 'time_change' → terminal
-- status 'time_change_consult'. Its mandatory comment is the EMPLOYER's §60⑤
-- grounds (사업 운영에 막대한 지장), not a worker 사유.

-- ---------------------------------------------------------------------------
-- 1. Column: reason becomes nullable; the mandatory-content CHECK goes away.
--    (Auto-generated name of the 0122 inline CHECK: leave_requests_reason_check.)
-- ---------------------------------------------------------------------------
ALTER TABLE leave_requests ALTER COLUMN reason DROP NOT NULL;
ALTER TABLE leave_requests DROP CONSTRAINT leave_requests_reason_check;

-- ---------------------------------------------------------------------------
-- 2. Status domain: add the lawful 'time_change_consult'. The retired
--    'returned'/'rejected' values stay in the CHECK because historical rows
--    still carry them; the trigger below makes them unreachable for new
--    writes. (Auto-generated name of the 0122 inline CHECK:
--    leave_requests_status_check.)
-- ---------------------------------------------------------------------------
ALTER TABLE leave_requests DROP CONSTRAINT leave_requests_status_check;
ALTER TABLE leave_requests ADD CONSTRAINT leave_requests_status_check
    CHECK (status IN ('pending', 'approved', 'time_change_consult', 'returned', 'rejected'));

ALTER TABLE leave_requests DROP CONSTRAINT leave_requests_status_charge_state;
ALTER TABLE leave_requests ADD CONSTRAINT leave_requests_status_charge_state
    CHECK (
        (status = 'pending' AND charge_state IN ('review_required', 'resolved'))
        OR (status = 'approved' AND charge_state IN ('resolved', 'legacy_unverified'))
        OR (status = 'time_change_consult' AND charge_state = 'not_required')
        OR (status IN ('returned', 'rejected') AND charge_state = 'not_required')
    );

-- ---------------------------------------------------------------------------
-- 3. Fail-closed guard on every writer, including console_leave_definer.
--    INSERTs may never carry a reason or a refusal status; UPDATEs may never
--    introduce a reason (NULLing one remains possible for the governed
--    erasure path) and may never transition INTO a refusal status. Historical
--    rows are untouched: an in-place update that does not change these fields
--    passes.
-- ---------------------------------------------------------------------------
CREATE FUNCTION leave_api.kr_labor_guardrails_guard()
RETURNS TRIGGER LANGUAGE plpgsql SET search_path = pg_catalog AS $$
BEGIN
    IF TG_OP = 'INSERT' THEN
        IF NEW.reason IS NOT NULL THEN
            RAISE EXCEPTION USING ERRCODE = '23514', MESSAGE = 'leave_write.kr_annual_no_reason';
        END IF;
        IF NEW.status IN ('returned', 'rejected') THEN
            RAISE EXCEPTION USING ERRCODE = '23514', MESSAGE = 'leave_write.kr_no_refusal';
        END IF;
    ELSIF TG_OP = 'UPDATE' THEN
        IF NEW.reason IS NOT NULL AND NEW.reason IS DISTINCT FROM OLD.reason THEN
            RAISE EXCEPTION USING ERRCODE = '23514', MESSAGE = 'leave_write.kr_annual_no_reason';
        END IF;
        IF NEW.status IN ('returned', 'rejected') AND NEW.status IS DISTINCT FROM OLD.status THEN
            RAISE EXCEPTION USING ERRCODE = '23514', MESSAGE = 'leave_write.kr_no_refusal';
        END IF;
    END IF;
    RETURN NEW;
END;
$$;
ALTER FUNCTION leave_api.kr_labor_guardrails_guard() OWNER TO console_leave_definer;
-- Deny-by-default like every other leave_api function: the trigger fires
-- without any role holding EXECUTE, and the privilege-matrix census counts a
-- PUBLIC-executable (NULL proacl) function as a defect.
REVOKE ALL ON FUNCTION leave_api.kr_labor_guardrails_guard() FROM PUBLIC;
CREATE TRIGGER trg_leave_requests_kr_labor_guardrails
    BEFORE INSERT OR UPDATE ON public.leave_requests
    FOR EACH ROW EXECUTE FUNCTION leave_api.kr_labor_guardrails_guard();

-- ---------------------------------------------------------------------------
-- 4. create_request loses its p_reason parameter entirely (a dead parameter
--    would keep modelling the field). The old 17-argument signature is
--    dropped, so a pre-guardrail binary fails loudly instead of demanding a
--    사유 from employees during a mixed-version window. The idempotency
--    digest is likewise reason-free: an in-flight pre-0216 retry replaying
--    across this migration surfaces as leave_create.idempotency_conflict
--    (409, no duplicate row) rather than silently rebinding.
-- ---------------------------------------------------------------------------
DROP FUNCTION leave_api.create_request(UUID, UUID, UUID, TEXT, DATE, DATE, TEXT, TEXT, TEXT[], UUID, JSONB, JSONB, JSONB, JSONB, UUID, TEXT, TEXT);

CREATE FUNCTION leave_api.create_request(
    p_org_id UUID, p_request_id UUID, p_requester UUID,
    p_leave_type TEXT, p_start_date DATE, p_end_date DATE,
    p_partial_day_period TEXT, p_review_reasons TEXT[],
    p_evidence_home_branch_id UUID, p_date_charges JSONB, p_calendar_revision_ref JSONB,
    p_policy_revision_ref JSONB, p_supporting_source_refs JSONB,
    p_submission_key UUID,
    p_trace_id TEXT, p_span_id TEXT
) RETURNS TABLE(request_id UUID, subject_employee_id UUID, branch_id UUID,
                request_version BIGINT, charge_version BIGINT,
                charge_units NUMERIC(16,6), server_digest TEXT)
LANGUAGE plpgsql SECURITY DEFINER
SET search_path = pg_catalog SET row_security = on AS $$
DECLARE
    v_subject UUID;
    v_branch UUID;
    v_snapshot JSONB;
    v_units NUMERIC(16,6);
    v_digest TEXT;
    v_submission_digest TEXT;
    v_resolved BOOLEAN := p_date_charges IS NOT NULL;
    v_resolution_id UUID;
    v_existing public.leave_requests%ROWTYPE;
    v_existing_server_digest TEXT;
BEGIN
    PERFORM leave_api.assert_context(p_org_id, p_requester, p_trace_id, p_span_id);
    IF p_submission_key IS NULL
       OR p_end_date < p_start_date
       OR p_leave_type NOT IN ('annual','half_day')
       OR (p_leave_type = 'half_day') IS DISTINCT FROM (p_partial_day_period IS NOT NULL) THEN
        RAISE EXCEPTION USING ERRCODE = '22023', MESSAGE = 'leave_create.invalid_intent';
    END IF;

    -- Idempotency is bound only to normalized client intent. Mutable routing
    -- and evidence validation deliberately occur after this replay check so a
    -- committed response can be recovered even if home branch, calendar, or
    -- policy state changed before the retry arrived.
    v_submission_digest := pg_catalog.encode(public.digest(
        pg_catalog.convert_to(pg_catalog.jsonb_build_object(
            'leave_type', p_leave_type,
            'start_date', p_start_date,
            'end_date', p_end_date,
            'partial_day_period', p_partial_day_period
        )::TEXT, 'UTF8'), 'sha256'), 'hex');
    PERFORM pg_catalog.pg_advisory_xact_lock(pg_catalog.hashtextextended(
        p_org_id::TEXT || ':' || p_requester::TEXT || ':' || p_submission_key::TEXT, 166
    ));
    SELECT * INTO v_existing
      FROM public.leave_requests lr
     WHERE lr.org_id = p_org_id
       AND lr.requester_user_id = p_requester
       AND lr.submission_key = p_submission_key;
    IF FOUND THEN
        IF v_existing.submission_digest IS DISTINCT FROM v_submission_digest THEN
            RAISE EXCEPTION USING ERRCODE='22023', MESSAGE='leave_create.idempotency_conflict';
        END IF;
        SELECT r.server_digest INTO v_existing_server_digest
          FROM public.leave_charge_resolutions r
         WHERE r.org_id = p_org_id
           AND r.request_id = v_existing.id
           AND r.charge_version = 1
           AND v_existing.submission_initial_charge_version = 1;
        RETURN QUERY SELECT v_existing.id, v_existing.subject_employee_id,
            v_existing.branch_id, 1::BIGINT,
            v_existing.submission_initial_charge_version,
            CASE WHEN v_existing.submission_initial_charge_version = 0 THEN NULL::NUMERIC
                 ELSE (SELECT r.charge_units
                         FROM public.leave_charge_resolutions r
                        WHERE r.org_id = p_org_id
                          AND r.request_id = v_existing.id
                          AND r.charge_version = 1) END,
            v_existing_server_digest;
        RETURN;
    END IF;

    PERFORM pg_catalog.pg_advisory_xact_lock(pg_catalog.hashtextextended(p_requester::TEXT, 166));
    SELECT e.id, e.home_branch_id INTO v_subject, v_branch
      FROM public.users u
      JOIN public.employees e ON e.id = u.employee_id AND e.org_id = u.org_id
     WHERE u.id = p_requester AND u.org_id = p_org_id AND u.is_active
       AND e.employment_status = 'ACTIVE'
     FOR UPDATE OF e;
    IF NOT FOUND THEN
        RAISE EXCEPTION USING ERRCODE = '42501', MESSAGE = 'leave_create.self_employee_required';
    END IF;
    IF v_branch IS NOT NULL THEN
        PERFORM pg_catalog.pg_advisory_xact_lock(pg_catalog.hashtextextended(v_branch::TEXT, 166));
    END IF;
    IF v_branch IS NULL OR NOT EXISTS (
        SELECT 1 FROM public.branches b
        WHERE b.id = v_branch AND b.org_id = p_org_id AND b.deactivated_at IS NULL
    ) THEN
        RAISE EXCEPTION USING ERRCODE = 'P0001', MESSAGE = 'leave_create.home_branch_required';
    END IF;
    IF v_resolved IS DISTINCT FROM (p_review_reasons IS NULL OR cardinality(p_review_reasons) = 0)
       OR (v_resolved AND p_evidence_home_branch_id IS DISTINCT FROM v_branch)
       OR (NOT v_resolved AND (p_evidence_home_branch_id IS NOT NULL
           OR p_date_charges IS NOT NULL
           OR p_calendar_revision_ref IS NOT NULL
           OR p_policy_revision_ref IS NOT NULL
           OR p_supporting_source_refs IS NOT NULL)) THEN
        RAISE EXCEPTION USING ERRCODE = '22023', MESSAGE = 'leave_create.invalid_charge_choice';
    END IF;
    IF NOT v_resolved AND (
        cardinality(p_review_reasons) = 0 OR p_review_reasons IS NULL
        OR pg_catalog.array_position(p_review_reasons, NULL) IS NOT NULL
        OR NOT p_review_reasons <@ ARRAY['missing_calendar','ambiguous_calendar',
            'calendar_source_unavailable','missing_policy','ambiguous_policy',
            'policy_source_unavailable']::TEXT[]
    ) THEN
        RAISE EXCEPTION USING ERRCODE = '22023', MESSAGE = 'leave_create.invalid_review_reasons';
    END IF;

    IF v_resolved THEN
        SELECT c.snapshot, c.charge_units, c.server_digest
          INTO v_snapshot, v_units, v_digest
          FROM leave_api.canonical_charge_snapshot(v_branch, p_leave_type,
               p_partial_day_period, p_start_date, p_end_date, p_date_charges,
               p_calendar_revision_ref, p_policy_revision_ref,
               p_supporting_source_refs) c;
    END IF;

    INSERT INTO public.leave_requests
        (id, org_id, branch_id, requester_user_id, subject_employee_id, days,
         leave_type, start_date, end_date, partial_day_period, status,
         charge_state, charge_review_reasons, charge_units, charge_version,
         submission_key, submission_digest, submission_initial_charge_version)
    VALUES
        (p_request_id, p_org_id, v_branch, p_requester, v_subject,
         CASE WHEN p_leave_type = 'half_day' THEN 0.5::NUMERIC
              ELSE (p_end_date - p_start_date + 1)::NUMERIC END,
         p_leave_type, p_start_date, p_end_date,
         p_partial_day_period, 'pending', 'review_required',
         CASE WHEN v_resolved THEN ARRAY['missing_calendar']::TEXT[] ELSE p_review_reasons END,
         NULL, 0, p_submission_key, v_submission_digest,
         CASE WHEN v_resolved THEN 1 ELSE 0 END);

    IF v_resolved THEN
        v_resolution_id := public.gen_random_uuid();
        INSERT INTO public.leave_charge_resolutions
            (id, org_id, request_id, charge_version, home_branch_id, charge_units,
             date_charges, calendar_revision_ref, policy_revision_ref,
             supporting_source_refs, snapshot, server_digest, resolution_origin,
             resolved_by, resolved_at)
        VALUES
            (v_resolution_id, p_org_id, p_request_id, 1, v_branch, v_units,
             p_date_charges, p_calendar_revision_ref, p_policy_revision_ref,
             p_supporting_source_refs, v_snapshot, v_digest, 'automated', NULL, pg_catalog.statement_timestamp());
        UPDATE public.leave_requests
           SET charge_state = 'resolved',
               charge_review_reasons = ARRAY[]::TEXT[],
               charge_units = v_units,
               charge_version = 1,
               current_charge_resolution_id = v_resolution_id
         WHERE org_id = p_org_id AND id = p_request_id;
    END IF;

    INSERT INTO public.audit_events
        (actor, action, target_type, target_id, branch_id, before_snap, after_snap,
         trace_id, span_id, occurred_at, org_id)
    VALUES
        (p_requester, 'leave_request.create', 'leave_request', p_request_id::TEXT,
         v_branch, NULL,
         pg_catalog.jsonb_build_object('status','pending','leave_type',p_leave_type,
            'subject_employee_id',v_subject,'branch_id',v_branch,
            'charge_state',CASE WHEN v_resolved THEN 'resolved' ELSE 'review_required' END,
            'request_version',1,
            'charge_version',CASE WHEN v_resolved THEN 1 ELSE 0 END,
            'server_digest',v_digest),
         p_trace_id, p_span_id, pg_catalog.statement_timestamp(), p_org_id);

    RETURN QUERY SELECT p_request_id, v_subject, v_branch, 1::BIGINT,
        CASE WHEN v_resolved THEN 1::BIGINT ELSE 0::BIGINT END, v_units, v_digest;
END;
$$;
ALTER FUNCTION leave_api.create_request(UUID, UUID, UUID, TEXT, DATE, DATE, TEXT, TEXT[], UUID, JSONB, JSONB, JSONB, JSONB, UUID, TEXT, TEXT)
    OWNER TO console_leave_definer;
REVOKE ALL ON FUNCTION leave_api.create_request(UUID, UUID, UUID, TEXT, DATE, DATE, TEXT, TEXT[], UUID, JSONB, JSONB, JSONB, JSONB, UUID, TEXT, TEXT)
    FROM PUBLIC, console_rt;
GRANT EXECUTE ON FUNCTION leave_api.create_request(UUID, UUID, UUID, TEXT, DATE, DATE, TEXT, TEXT[], UUID, JSONB, JSONB, JSONB, JSONB, UUID, TEXT, TEXT)
    TO console_leave_cmd;

-- ---------------------------------------------------------------------------
-- 5. decide_request: the decision domain becomes approve | time_change. The
--    signature is unchanged; a pre-guardrail binary sending 'return'/'reject'
--    gets the existing leave_decide.invalid_decision error (fail-closed). The
--    mandatory comment on 'time_change' is the employer's §60⑤ grounds.
-- ---------------------------------------------------------------------------
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

-- ---------------------------------------------------------------------------
-- 6. The console_rt expand-compatibility bridge (0166) allowed a legacy
--    pending → returned/rejected decision UPDATE. Refusal is no longer a
--    decision anywhere, so the bridge narrows to approve-only. Its legacy
--    INSERT arm is left textually unchanged: every pre-0166 binary write
--    carried a NOT NULL reason, which the kr_labor_guardrails trigger now
--    refuses, so that path is closed by the guard above.
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
           OR NEW.current_charge_resolution_id IS NOT NULL THEN
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
           OR NEW.current_charge_resolution_id IS DISTINCT FROM OLD.current_charge_resolution_id THEN
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
    IF TG_OP = 'DELETE' THEN RETURN OLD; END IF;
    RETURN NEW;
END;
$$;
ALTER FUNCTION leave_api.protected_request_writer_guard() OWNER TO console_leave_definer;
