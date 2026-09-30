-- Console People creation must use the same isolated command capability as
-- home-branch changes. The command owns every write in its idempotent create
-- transaction so a runtime-role caller cannot leave a partially created row.
GRANT SELECT, INSERT, UPDATE ON public.employee_create_idempotency TO console_leave_definer;
GRANT SELECT, INSERT ON public.employee_employment_profiles,
    public.employee_lifecycle_events TO console_leave_definer;

-- These pre-existing trigger bodies run inside this command's hardened
-- `pg_catalog` search path, so their relation references must remain explicit.
CREATE OR REPLACE FUNCTION public.employee_employment_profiles_same_org()
RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM public.employees e
        WHERE e.id = NEW.employee_id AND e.org_id = NEW.org_id
    ) THEN
        RAISE EXCEPTION 'employee employment profile must belong to the same org'
            USING ERRCODE = '23514';
    END IF;
    RETURN NEW;
END;
$$;

CREATE OR REPLACE FUNCTION public.console_employee_number_unique()
RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.source_filename = 'console' AND NEW.employee_number IS NOT NULL THEN
        PERFORM pg_catalog.pg_advisory_xact_lock(
            pg_catalog.hashtext(NEW.org_id::TEXT || ':' || NEW.employee_number)
        );
        IF EXISTS (
            SELECT 1 FROM public.employees e
            WHERE e.org_id = NEW.org_id
              AND e.employee_number = NEW.employee_number
        ) THEN
            RAISE EXCEPTION 'employee number already exists in this organization'
                USING ERRCODE = '23505';
        END IF;
    END IF;
    RETURN NEW;
END;
$$;

CREATE OR REPLACE FUNCTION public.employee_lifecycle_events_same_org()
RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM public.employees e
        WHERE e.id = NEW.employee_id AND e.org_id = NEW.org_id
    ) THEN
        RAISE EXCEPTION 'employee lifecycle event employee_id must belong to the same org'
            USING ERRCODE = '23514';
    END IF;
    RETURN NEW;
END;
$$;

-- Mirror `authorize_org_wide(EmployeeDirectoryManage)` at the command boundary:
-- SUPER_ADMIN has the built-in permission; a custom grant also needs the
-- resolver's live All scope plus an active, non-branch-narrowed allow policy.
CREATE FUNCTION leave_api.assert_employee_directory_manager(
    p_org_id UUID, p_actor UUID
) RETURNS VOID
LANGUAGE plpgsql SECURITY DEFINER
SET search_path = pg_catalog SET row_security = on AS $$
BEGIN
    IF NOT EXISTS (
        SELECT 1
        FROM public.users u
        WHERE u.id = p_actor
          AND u.org_id = p_org_id
          AND u.is_active
          AND (
              u.roles @> ARRAY['SUPER_ADMIN']::TEXT[]
              OR (
                  -- A custom grant can only be org-wide when the actor has
                  -- the resolver's live All scope (EXECUTIVE/SUPER_ADMIN).
                  (u.roles @> ARRAY['SUPER_ADMIN']::TEXT[]
                   OR u.roles @> ARRAY['EXECUTIVE']::TEXT[])
                  AND EXISTS (
                      SELECT 1
                      FROM public.user_role_assignments ura
                      JOIN public.policy_roles pr
                        ON pr.org_id = ura.org_id AND pr.id = ura.role_id
                      JOIN public.policy_role_permissions prp
                        ON prp.org_id = pr.org_id AND prp.role_id = pr.id
                      WHERE ura.org_id = p_org_id
                        AND ura.user_id = p_actor
                        AND pr.status = 'ACTIVE'
                        AND NOT pr.is_system
                        AND prp.feature_key = 'employee_directory_manage'
                        AND prp.permission_level = 'allow'
                        AND NOT EXISTS (
                            SELECT 1
                            FROM public.policy_role_conditions prc
                            WHERE prc.org_id = pr.org_id
                              AND prc.role_id = pr.id
                              AND (
                                  prc.operator NOT IN ('equals', 'in')
                                  OR prc.attribute = 'branch'
                                  OR prc.attribute <> 'team'
                                  OR u.team IS NULL
                                  OR NOT EXISTS (
                                      SELECT 1
                                      FROM pg_catalog.unnest(prc.condition_values) value
                                      WHERE pg_catalog.btrim(value) = u.team
                                         OR pg_catalog.upper(pg_catalog.btrim(value)) = CASE u.team
                                              WHEN '정비' THEN 'MAINTENANCE'
                                              WHEN '예방' THEN 'PREVENTION'
                                              WHEN '관리' THEN 'MANAGEMENT'
                                              WHEN '접수' THEN 'RECEPTION'
                                              ELSE NULL
                                            END
                                  )
                              )
                        )
                  )
              )
          )
    ) THEN
        RAISE EXCEPTION USING ERRCODE = '42501', MESSAGE = 'employee_create.actor_forbidden';
    END IF;
END;
$$;
ALTER FUNCTION leave_api.assert_employee_directory_manager(UUID, UUID) OWNER TO console_leave_definer;

CREATE FUNCTION leave_api.create_employee(
    p_org_id UUID, p_employee_id UUID, p_employee_number TEXT, p_name TEXT,
    p_company TEXT, p_employment_type TEXT, p_phone_e164 TEXT, p_org_unit TEXT,
    p_position TEXT, p_site TEXT, p_home_branch_id UUID, p_base_pay NUMERIC,
    p_idempotency_key TEXT, p_request_hash TEXT, p_actor UUID,
    p_trace_id TEXT, p_span_id TEXT
) RETURNS TABLE(employee_id UUID, replayed BOOLEAN)
LANGUAGE plpgsql SECURITY DEFINER
SET search_path = pg_catalog SET row_security = on AS $$
DECLARE
    v_stored_hash TEXT;
    v_existing_employee_id UUID;
BEGIN
    PERFORM leave_api.assert_context(p_org_id, p_actor, p_trace_id, p_span_id);
    PERFORM leave_api.assert_employee_directory_manager(p_org_id, p_actor);

    IF NOT EXISTS (
        SELECT 1 FROM public.branches b
        WHERE b.org_id = p_org_id
          AND b.id = p_home_branch_id
          AND b.deactivated_at IS NULL
    ) THEN
        RAISE EXCEPTION USING ERRCODE = 'P0001', MESSAGE = 'employee_create.active_branch_required';
    END IF;

    INSERT INTO public.employee_create_idempotency (org_id, idempotency_key, request_hash)
    VALUES (p_org_id, p_idempotency_key, p_request_hash)
    ON CONFLICT (org_id, idempotency_key) DO NOTHING;

    SELECT i.request_hash, i.employee_id
      INTO v_stored_hash, v_existing_employee_id
      FROM public.employee_create_idempotency i
     WHERE i.org_id = p_org_id AND i.idempotency_key = p_idempotency_key
     FOR UPDATE;
    IF v_stored_hash IS DISTINCT FROM p_request_hash THEN
        RAISE EXCEPTION USING ERRCODE = '23505', MESSAGE = 'employee_create.idempotency_conflict';
    END IF;
    IF v_existing_employee_id IS NOT NULL THEN
        RETURN QUERY SELECT v_existing_employee_id, true;
        RETURN;
    END IF;

    PERFORM pg_catalog.pg_advisory_xact_lock(
        pg_catalog.hashtext(p_org_id::TEXT || ':' || p_employee_number)
    );
    IF EXISTS (
        SELECT 1 FROM public.employees e
        WHERE e.org_id = p_org_id AND e.employee_number = p_employee_number
    ) THEN
        RAISE EXCEPTION USING ERRCODE = '23505', MESSAGE = 'employee_create.employee_number_conflict';
    END IF;

    INSERT INTO public.employees (
        id, org_id, company, name, employee_number, org_unit, position,
        worksite_name, home_branch_id, source_filename, source_sheet,
        source_row, source_key, raw_row, source_metadata,
        identity_resolution_strategy, identity_resolution_confidence,
        identity_review_required, identity_name_only_merge
    ) VALUES (
        p_employee_id, p_org_id, p_company, p_name, p_employee_number,
        p_org_unit, p_position, p_site, p_home_branch_id, 'console', 'people',
        1, 'console:' || p_employee_number, '{}'::jsonb, '{}'::jsonb,
        'employee_number', 'high', false, false
    );
    INSERT INTO public.employee_employment_profiles (
        employee_id, org_id, employment_type, phone_e164, base_pay,
        idempotency_key, request_hash, created_by
    ) VALUES (
        p_employee_id, p_org_id, p_employment_type, p_phone_e164, p_base_pay,
        p_idempotency_key, p_request_hash, p_actor
    );
    INSERT INTO public.employee_lifecycle_events (
        id, org_id, employee_id, event_type, to_status, to_company,
        to_org_unit, to_position, effective_date, comment, signoffs, created_by
    ) VALUES (
        pg_catalog.gen_random_uuid(), p_org_id, p_employee_id, 'ONBOARD', 'ACTIVE',
        p_company, p_org_unit, p_position, CURRENT_DATE::TEXT,
        'Created through People & Workforce', '{}'::jsonb, p_actor
    );
    UPDATE public.employee_create_idempotency
       SET employee_id = p_employee_id
     WHERE org_id = p_org_id AND idempotency_key = p_idempotency_key;
    INSERT INTO public.audit_events (
        actor, action, target_type, target_id, branch_id, before_snap, after_snap,
        trace_id, span_id, occurred_at, org_id
    ) VALUES (
        p_actor, 'employee.create', 'employee', p_employee_id::TEXT, p_home_branch_id,
        NULL, pg_catalog.jsonb_build_object(
            'employee_number', p_employee_number,
            'employment_type', p_employment_type,
            'home_branch_id', p_home_branch_id,
            'compensation_recorded', true,
            'phone_recorded', true
        ), p_trace_id, p_span_id, pg_catalog.statement_timestamp(), p_org_id
    );
    RETURN QUERY SELECT p_employee_id, false;
END;
$$;
ALTER FUNCTION leave_api.create_employee(
    UUID, UUID, TEXT, TEXT, TEXT, TEXT, TEXT, TEXT, TEXT, TEXT, UUID, NUMERIC,
    TEXT, TEXT, UUID, TEXT, TEXT
) OWNER TO console_leave_definer;
REVOKE ALL ON FUNCTION leave_api.create_employee(
    UUID, UUID, TEXT, TEXT, TEXT, TEXT, TEXT, TEXT, TEXT, TEXT, UUID, NUMERIC,
    TEXT, TEXT, UUID, TEXT, TEXT
) FROM PUBLIC, console_rt;
GRANT EXECUTE ON FUNCTION leave_api.create_employee(
    UUID, UUID, TEXT, TEXT, TEXT, TEXT, TEXT, TEXT, TEXT, TEXT, UUID, NUMERIC,
    TEXT, TEXT, UUID, TEXT, TEXT
) TO console_leave_cmd;
