-- u6ih part-1 reviewer HIGH-1 (orphan row behind 403): the Employment-port
-- reroute moved the employee ROW write onto `console_rt` (whose
-- `trg_employees_leave_command_only` guard keeps `home_branch_id` command-only),
-- so the first home-branch routing authority is established post-commit through
-- `leave_api.set_employee_home_branch`. That function authorized a FIRST
-- assignment with `assert_org_admin` (SUPER_ADMIN only), which is NARROWER than
-- the create pre-flight / recheck predicate `assert_employee_directory_manager`
-- (SUPER_ADMIN, or EXECUTIVE/SUPER_ADMIN with a live org-wide
-- `employee_directory_manage` allow grant). The mismatch meant an EXECUTIVE with
-- an org-wide directory-manage grant — whom the create flow legitimately admits
-- (201) — committed the row and was then refused 403 at routing, leaving an
-- orphan employee with `home_branch_id IS NULL` and a consumed idempotency key.
--
-- Fix (conductor ruling: NARROW the widening to the create flow's OWN employee):
-- the general `leave_api.set_employee_home_branch` route function is UNTOUCHED
-- (its first-assignment stays `assert_org_admin`). This migration adds a DEDICATED
-- create-path function, `leave_api.set_employee_home_branch_create`, that the
-- create flow's post-commit routing calls. It is create-scoped by SERVER-SIDE
-- creation evidence — an `employee.create` audit by p_actor for THIS employee with
-- a matching `requested_home_branch_id` — so a `console_leave_cmd` caller cannot
-- invoke the widened directory-manager authority for an arbitrary imported /
-- historical / residual branchless employee.

CREATE FUNCTION leave_api.set_employee_home_branch_create(
    p_org_id UUID, p_employee_id UUID, p_home_branch_id UUID,
    p_expected_updated_at TIMESTAMPTZ, p_actor UUID,
    p_trace_id TEXT, p_span_id TEXT
) RETURNS TABLE(employee_id UUID, home_branch_id UUID, updated_at TIMESTAMPTZ)
LANGUAGE plpgsql SECURITY DEFINER
SET search_path = pg_catalog SET row_security = on AS $$
DECLARE
    v_old_branch UUID;
    v_old_updated TIMESTAMPTZ;
    v_new_updated TIMESTAMPTZ;
BEGIN
    PERFORM leave_api.assert_context(p_org_id,p_actor,p_trace_id,p_span_id);
    SELECT e.home_branch_id,e.updated_at INTO v_old_branch,v_old_updated
      FROM public.employees e WHERE e.org_id=p_org_id AND e.id=p_employee_id FOR UPDATE;
    IF NOT FOUND THEN RAISE EXCEPTION USING ERRCODE='P0001', MESSAGE='leave_home_branch.employee_not_found'; END IF;
    IF v_old_updated IS DISTINCT FROM p_expected_updated_at THEN
        RAISE EXCEPTION USING ERRCODE='40001', MESSAGE='leave_home_branch.concurrent_modification';
    END IF;
    IF v_old_branch IS NOT NULL THEN
        RAISE EXCEPTION USING ERRCODE='40001', MESSAGE='leave_home_branch.already_assigned';
    END IF;
    -- Server-side creation evidence: the actor must have CREATED this employee via
    -- the create flow (an `employee.create` audit by p_actor for THIS employee with
    -- the requested branch). Not caller-controllable, so the create-scoped
    -- directory-manager authority cannot be claimed for an arbitrary employee.
    IF NOT EXISTS (
        SELECT 1 FROM public.audit_events a
        WHERE a.org_id = p_org_id
          AND a.action = 'employee.create'
          AND a.actor = p_actor
          AND a.target_type = 'employee'
          AND a.target_id = p_employee_id::TEXT
          AND a.after_snap->>'requested_home_branch_id' = p_home_branch_id::TEXT
    ) THEN
        RAISE EXCEPTION USING ERRCODE = '42501', MESSAGE = 'employee_create.home_branch_create_required';
    END IF;
    -- The SAME predicate the in-transaction create recheck ran.
    PERFORM leave_api.assert_employee_directory_manager(p_org_id,p_actor);
    PERFORM pg_catalog.pg_advisory_xact_lock(pg_catalog.hashtextextended(p_home_branch_id::TEXT, 166));
    IF NOT EXISTS (SELECT 1 FROM public.branches b WHERE b.org_id=p_org_id
                   AND b.id=p_home_branch_id AND b.deactivated_at IS NULL) THEN
        RAISE EXCEPTION USING ERRCODE='P0001', MESSAGE='leave_home_branch.active_branch_required';
    END IF;
    UPDATE public.employees e SET home_branch_id=p_home_branch_id,updated_at=pg_catalog.now()
     WHERE e.org_id=p_org_id AND e.id=p_employee_id RETURNING e.updated_at INTO v_new_updated;
    INSERT INTO public.audit_events
        (actor,action,target_type,target_id,branch_id,before_snap,after_snap,
         trace_id,span_id,occurred_at,org_id)
    VALUES
        (p_actor,'employee.home_branch_set','employee',p_employee_id::TEXT,p_home_branch_id,
         pg_catalog.jsonb_build_object('home_branch_id',v_old_branch,'updated_at',v_old_updated),
         pg_catalog.jsonb_build_object('home_branch_id',p_home_branch_id,'updated_at',v_new_updated),
         p_trace_id,p_span_id,pg_catalog.statement_timestamp(),p_org_id);
    RETURN QUERY SELECT p_employee_id,p_home_branch_id,v_new_updated;
END;
$$;
ALTER FUNCTION leave_api.set_employee_home_branch_create(UUID, UUID, UUID, TIMESTAMPTZ, UUID, TEXT, TEXT)
    OWNER TO console_leave_definer;
REVOKE ALL ON FUNCTION leave_api.set_employee_home_branch_create(UUID, UUID, UUID, TIMESTAMPTZ, UUID, TEXT, TEXT)
    FROM PUBLIC, console_rt;
GRANT EXECUTE ON FUNCTION leave_api.set_employee_home_branch_create(UUID, UUID, UUID, TIMESTAMPTZ, UUID, TEXT, TEXT)
    TO console_leave_cmd;
