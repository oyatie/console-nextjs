-- Exact, evidence-pinned leave charging and explicit self-service routing.
--
-- Existing approved rows remain truthful legacy evidence; they are never
-- silently promoted to a calendar-verified charge. Pending rows require review.
--
-- This is the expand half of a two-release employee-import cutover. The f6ff236
-- runtime can be active while 0166 migrates, and can be restored during the
-- rollback window, so console_rt must temporarily retain its exact legacy employee
-- import writes. The new binary never falls back to those writes: it uses the
-- additive leave_api command functions and intrinsic receipts/audit below. A
-- later numbered contract migration may close the legacy console_rt import surface
-- only after this release is the proven rollback floor.

ALTER TABLE employees
    ADD COLUMN home_branch_id UUID NULL;

ALTER TABLE employees
    ADD CONSTRAINT employees_id_org_id_unique UNIQUE (id, org_id),
    ADD CONSTRAINT employees_home_branch_same_org_fk
    FOREIGN KEY (home_branch_id, org_id)
    REFERENCES branches (id, org_id) ON DELETE RESTRICT;

CREATE INDEX employees_org_home_branch_idx
    ON employees (org_id, home_branch_id)
    WHERE home_branch_id IS NOT NULL;

ALTER TABLE employees
    ALTER COLUMN leave_accrued TYPE NUMERIC(16,6) USING leave_accrued::NUMERIC(16,6),
    ALTER COLUMN leave_used TYPE NUMERIC(16,6) USING leave_used::NUMERIC(16,6),
    ALTER COLUMN leave_remaining TYPE NUMERIC(16,6) USING leave_remaining::NUMERIC(16,6);

ALTER TABLE leave_requests
    ADD CONSTRAINT leave_requests_org_id_id_unique UNIQUE (org_id, id),
    ALTER COLUMN days TYPE NUMERIC(16,6) USING days::NUMERIC(16,6),
    -- Expand, do not rename: a rollback can put the pre-0166 binary back
    -- against this schema, so its SELECT/INSERT contract must retain `days`.
    -- New code reads the explicit legacy alias while exact charges remain in
    -- charge_units. The generated alias makes the two names impossible to
    -- drift during the compatibility window.
    ADD COLUMN legacy_days NUMERIC(16,6) GENERATED ALWAYS AS (days) STORED,
    ADD COLUMN partial_day_period TEXT NULL
        CHECK (partial_day_period IN ('am', 'pm')),
    ADD COLUMN charge_state TEXT NOT NULL DEFAULT 'review_required'
        CHECK (charge_state IN ('review_required','resolved','not_required','legacy_unverified')),
    ADD COLUMN charge_review_reasons TEXT[] NOT NULL DEFAULT ARRAY['missing_calendar']::TEXT[],
    ADD COLUMN charge_units NUMERIC(16,6) NULL
        CHECK (charge_units IS NULL OR (charge_units > 0 AND charge_units <= 366)),
    ADD COLUMN submission_key UUID NULL,
    ADD COLUMN submission_digest CHAR(64) NULL
        CHECK (submission_digest IS NULL OR submission_digest ~ '^[0-9a-f]{64}$'),
    ADD COLUMN submission_initial_charge_version BIGINT NULL
        CHECK (submission_initial_charge_version IN (0, 1)),
    ADD COLUMN request_version BIGINT NOT NULL DEFAULT 1 CHECK (request_version > 0),
    ADD COLUMN charge_version BIGINT NOT NULL DEFAULT 0 CHECK (charge_version >= 0),
    ADD COLUMN current_charge_resolution_id UUID NULL;

UPDATE leave_requests
SET charge_state = CASE status
        WHEN 'pending' THEN 'review_required'
        WHEN 'approved' THEN 'legacy_unverified'
        ELSE 'not_required'
    END,
    charge_review_reasons = CASE status
        WHEN 'pending' THEN ARRAY['missing_calendar']::TEXT[]
        ELSE ARRAY[]::TEXT[]
    END,
    charge_units = CASE WHEN status = 'approved' THEN legacy_days ELSE NULL END;

ALTER TABLE leave_requests
    ADD CONSTRAINT leave_requests_charge_shape CHECK (
        (charge_state = 'review_required'
            AND cardinality(charge_review_reasons) > 0
            AND charge_units IS NULL
            AND current_charge_resolution_id IS NULL)
        OR
        (charge_state = 'resolved'
            AND cardinality(charge_review_reasons) = 0
            AND charge_units IS NOT NULL
            AND current_charge_resolution_id IS NOT NULL)
        OR
        (charge_state = 'legacy_unverified'
            AND cardinality(charge_review_reasons) = 0
            AND charge_units IS NOT NULL
            AND current_charge_resolution_id IS NULL)
        OR
        (charge_state = 'not_required'
            AND cardinality(charge_review_reasons) = 0
            AND charge_units IS NULL
            AND current_charge_resolution_id IS NULL)
    ),
    ADD CONSTRAINT leave_requests_submission_pair CHECK (
        (submission_key IS NULL) = (submission_digest IS NULL)
        AND (submission_key IS NULL) = (submission_initial_charge_version IS NULL)
    ),
    ADD CONSTRAINT leave_requests_partial_day_shape CHECK (
        legacy_days IS NOT NULL
        OR (leave_type = 'half_day') = (partial_day_period IS NOT NULL)
    ),
    ADD CONSTRAINT leave_requests_charge_review_reason_values CHECK (
        array_position(charge_review_reasons, NULL) IS NULL
        AND
        charge_review_reasons <@ ARRAY[
            'missing_calendar','ambiguous_calendar','calendar_source_unavailable',
            'missing_policy','ambiguous_policy','policy_source_unavailable'
        ]::TEXT[]
    ),
    ADD CONSTRAINT leave_requests_status_charge_state CHECK (
        (status = 'pending' AND charge_state IN ('review_required', 'resolved'))
        OR (status = 'approved' AND charge_state IN ('resolved', 'legacy_unverified'))
        OR (status IN ('returned', 'rejected') AND charge_state = 'not_required')
    );

CREATE UNIQUE INDEX leave_requests_requester_submission_key_uq
    ON leave_requests (org_id, requester_user_id, submission_key)
    WHERE submission_key IS NOT NULL;

-- console-gate: audited-table leave_charge_resolutions
CREATE TABLE leave_charge_resolutions (
    id                    UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id                UUID        NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    request_id            UUID        NOT NULL,
    charge_version        BIGINT      NOT NULL CHECK (charge_version > 0),
    home_branch_id        UUID        NOT NULL,
    charge_units          NUMERIC(16,6) NOT NULL CHECK (charge_units > 0 AND charge_units <= 366),
    date_charges          JSONB       NOT NULL CHECK (
        jsonb_typeof(date_charges) = 'array' AND jsonb_array_length(date_charges) > 0
    ),
    calendar_revision_ref JSONB       NOT NULL CHECK (jsonb_typeof(calendar_revision_ref) = 'object'),
    policy_revision_ref   JSONB       NOT NULL CHECK (jsonb_typeof(policy_revision_ref) = 'object'),
    supporting_source_refs JSONB      NOT NULL DEFAULT '[]'::jsonb CHECK (
        jsonb_typeof(supporting_source_refs) = 'array'
    ),
    snapshot              JSONB       NOT NULL CHECK (jsonb_typeof(snapshot) = 'object'),
    server_digest         CHAR(64)    NOT NULL CHECK (server_digest ~ '^[0-9a-f]{64}$'),
    resolution_origin     TEXT        NOT NULL CHECK (
        resolution_origin IN ('automated', 'manual')
    ),
    resolved_by           UUID        NULL,
    resolved_at           TIMESTAMPTZ NOT NULL,
    created_at            TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (org_id, request_id, charge_version),
    UNIQUE (org_id, id, request_id),
    FOREIGN KEY (org_id, request_id)
        REFERENCES leave_requests (org_id, id) ON DELETE RESTRICT,
    FOREIGN KEY (home_branch_id, org_id)
        REFERENCES branches (id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (resolved_by, org_id)
        REFERENCES users (id, org_id) ON DELETE RESTRICT,
    CHECK ((resolution_origin = 'automated') = (resolved_by IS NULL))
);

ALTER TABLE leave_charge_resolutions ENABLE ROW LEVEL SECURITY;
ALTER TABLE leave_charge_resolutions FORCE ROW LEVEL SECURITY;
CREATE POLICY org_isolation ON leave_charge_resolutions
    USING (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid)
    WITH CHECK (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid);

ALTER TABLE leave_requests
    ADD CONSTRAINT leave_requests_current_charge_resolution_fk
    FOREIGN KEY (org_id, current_charge_resolution_id, id)
    REFERENCES leave_charge_resolutions (org_id, id, request_id) ON DELETE RESTRICT;

CREATE OR REPLACE FUNCTION leave_charge_resolutions_immutable()
RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION 'leave_charge_resolutions is append-only: % is forbidden (row id=%)', TG_OP, OLD.id;
END;
$$;

CREATE TRIGGER trg_leave_charge_resolutions_no_update
    BEFORE UPDATE ON leave_charge_resolutions
    FOR EACH ROW EXECUTE FUNCTION leave_charge_resolutions_immutable();
CREATE TRIGGER trg_leave_charge_resolutions_no_delete
    BEFORE DELETE ON leave_charge_resolutions
    FOR EACH ROW EXECUTE FUNCTION leave_charge_resolutions_immutable();

-- Replay-safe receipts for imported/initial leave-balance snapshots. The
-- command payload digest binds one source-scoped idempotency key to exactly one
-- employee and exact balance tuple; receipts are append-only evidence, not an
-- alternate mutable ledger.
CREATE TABLE leave_balance_import_receipts (
    id                  UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id              UUID        NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    employee_id         UUID        NOT NULL,
    source_kind         TEXT        NOT NULL CHECK (source_kind = 'employee_import'),
    source_ref          TEXT        NOT NULL CHECK (char_length(btrim(source_ref)) BETWEEN 1 AND 256),
    idempotency_key     TEXT        NOT NULL CHECK (char_length(btrim(idempotency_key)) BETWEEN 16 AND 256),
    payload_digest      CHAR(64)    NOT NULL CHECK (payload_digest ~ '^[0-9a-f]{64}$'),
    result_updated_at   TIMESTAMPTZ NOT NULL,
    changed             BOOLEAN     NOT NULL,
    actor               UUID        NOT NULL,
    trace_id            CHAR(32)    NOT NULL CHECK (trace_id ~ '^[0-9a-f]{32}$'),
    span_id             CHAR(16)    NOT NULL CHECK (span_id ~ '^[0-9a-f]{16}$'),
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (org_id, idempotency_key),
    FOREIGN KEY (employee_id, org_id) REFERENCES employees(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (actor, org_id) REFERENCES users(id, org_id) ON DELETE RESTRICT
);

ALTER TABLE leave_balance_import_receipts ENABLE ROW LEVEL SECURITY;
ALTER TABLE leave_balance_import_receipts FORCE ROW LEVEL SECURITY;
CREATE POLICY org_isolation ON leave_balance_import_receipts
    USING (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid)
    WITH CHECK (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid);

CREATE TRIGGER trg_leave_balance_import_receipts_no_update
    BEFORE UPDATE ON leave_balance_import_receipts
    FOR EACH ROW EXECUTE FUNCTION leave_charge_resolutions_immutable();
CREATE TRIGGER trg_leave_balance_import_receipts_no_delete
    BEFORE DELETE ON leave_balance_import_receipts
    FOR EACH ROW EXECUTE FUNCTION leave_charge_resolutions_immutable();

CREATE OR REPLACE FUNCTION leave_requests_intent_routing_immutable()
RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.org_id IS DISTINCT FROM OLD.org_id
       OR NEW.branch_id IS DISTINCT FROM OLD.branch_id
       OR NEW.requester_user_id IS DISTINCT FROM OLD.requester_user_id
       OR NEW.subject_employee_id IS DISTINCT FROM OLD.subject_employee_id
       OR NEW.leave_type IS DISTINCT FROM OLD.leave_type
       OR NEW.start_date IS DISTINCT FROM OLD.start_date
       OR NEW.end_date IS DISTINCT FROM OLD.end_date
       OR NEW.reason IS DISTINCT FROM OLD.reason
       OR NEW.days IS DISTINCT FROM OLD.days
       OR NEW.partial_day_period IS DISTINCT FROM OLD.partial_day_period
       OR NEW.submission_key IS DISTINCT FROM OLD.submission_key
       OR NEW.submission_digest IS DISTINCT FROM OLD.submission_digest
       OR NEW.submission_initial_charge_version IS DISTINCT FROM OLD.submission_initial_charge_version THEN
        RAISE EXCEPTION 'leave request routing and intent are immutable';
    END IF;
    RETURN NEW;
END;
$$;

CREATE OR REPLACE FUNCTION leave_requests_charge_pointer_consistent()
RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.current_charge_resolution_id IS NOT NULL AND NOT EXISTS (
        SELECT 1
        FROM public.leave_charge_resolutions lcr
        WHERE lcr.id = NEW.current_charge_resolution_id
          AND lcr.org_id = NEW.org_id
          AND lcr.request_id = NEW.id
          AND lcr.home_branch_id = NEW.branch_id
          AND lcr.charge_units = NEW.charge_units
          AND lcr.charge_version = NEW.charge_version
    ) THEN
        RAISE EXCEPTION 'current leave charge resolution does not match request branch, amount, or version';
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER trg_leave_requests_intent_routing_immutable
    BEFORE UPDATE ON leave_requests
    FOR EACH ROW EXECUTE FUNCTION leave_requests_intent_routing_immutable();

CREATE TRIGGER trg_leave_requests_charge_pointer_consistent
    BEFORE INSERT OR UPDATE ON leave_requests
    FOR EACH ROW EXECUTE FUNCTION leave_requests_charge_pointer_consistent();

-- Complete single-writer command boundary for leave intent, charge evidence,
-- decisions, ledger movements, and employee approval routing.
--
-- console_rt cannot call these routines even after spoofing arbitrary app.* GUCs.
-- During this expand phase it retains only the guarded legacy request
-- INSERT/UPDATE bridge documented below so a pre-0166 binary can be restored;
-- exact-charge and all other writes remain command-only. A separate NOLOGIN
-- command capability has EXECUTE only, while every command mutation runs as a
-- pinned NOLOGIN definer which is itself subject to FORCE RLS under the
-- explicit org supplied to the command. The routines lock and derive all
-- routing/version/ledger authority and append exactly one intrinsic audit row
-- on every successful command.
-- Cluster-global roles are infrastructure-owned and must exist before this
-- non-CREATEROLE migration runs. Fail closed on any attribute or membership
-- drift instead of attempting CREATE/ALTER/REVOKE ROLE from console_app.
DO $$
DECLARE
    v_migrator OID := pg_catalog.to_regrole('console_app');
    v_runtime OID := pg_catalog.to_regrole('console_rt');
    v_writer OID := pg_catalog.to_regrole('console_leave_definer');
    v_command OID := pg_catalog.to_regrole('console_leave_cmd');
    v_applier_is_superuser BOOLEAN;
BEGIN
    IF v_migrator IS NULL OR v_runtime IS NULL OR v_writer IS NULL OR v_command IS NULL THEN
        RAISE EXCEPTION 'leave role precondition failed: roles are not preprovisioned';
    END IF;
    SELECT rolsuper INTO v_applier_is_superuser
      FROM pg_catalog.pg_roles WHERE rolname=CURRENT_USER;
    IF NOT v_applier_is_superuser
       AND (CURRENT_USER <> 'console_app' OR SESSION_USER <> 'console_app') THEN
        RAISE EXCEPTION 'leave role precondition failed: console_app must apply directly';
    END IF;
    IF EXISTS (
        SELECT 1 FROM pg_catalog.pg_roles WHERE oid=v_migrator
          AND (NOT rolcanlogin OR NOT rolinherit OR rolsuper OR NOT rolbypassrls OR rolcreatedb
               OR rolcreaterole OR rolreplication)
    ) THEN
        RAISE EXCEPTION 'leave role precondition failed: console_app is unsafe';
    END IF;
    IF EXISTS (
        SELECT 1 FROM pg_catalog.pg_roles WHERE oid=v_writer
          AND (rolcanlogin OR rolsuper OR rolbypassrls OR rolinherit
               OR rolcreatedb OR rolcreaterole OR rolreplication)
    ) THEN
        RAISE EXCEPTION 'leave role precondition failed: console_leave_definer is missing or unsafe';
    END IF;
    IF EXISTS (
        SELECT 1 FROM pg_catalog.pg_roles WHERE oid=v_command
          AND (NOT rolcanlogin OR rolsuper OR rolbypassrls OR rolinherit
               OR rolcreatedb OR rolcreaterole OR rolreplication)
    ) THEN
        RAISE EXCEPTION 'leave role precondition failed: console_leave_cmd is missing or unsafe';
    END IF;
    IF NOT EXISTS (
        SELECT 1 FROM pg_catalog.pg_auth_members
         WHERE roleid=v_writer AND member=v_migrator
           AND NOT admin_option AND inherit_option AND set_option
    ) OR EXISTS (
        SELECT 1 FROM pg_catalog.pg_auth_members
         WHERE (roleid=v_writer AND member<>v_migrator)
            OR member=v_writer
            OR member IN (v_runtime,v_command)
            OR roleid IN (v_runtime,v_command)
    ) THEN
        RAISE EXCEPTION 'leave role precondition failed: membership drift';
    END IF;
END
$$;

CREATE SCHEMA leave_api AUTHORIZATION console_leave_definer;
REVOKE ALL ON SCHEMA leave_api FROM PUBLIC, console_rt;
GRANT USAGE ON SCHEMA leave_api TO console_leave_cmd;
GRANT USAGE ON SCHEMA public TO console_leave_definer;

GRANT SELECT ON public.organizations, public.users, public.user_branches,
    public.branches, public.employees, public.leave_requests,
    public.leave_charge_resolutions, public.leave_balance_import_receipts,
    public.data_import_runs, public.data_import_rows, public.policy_roles,
    public.policy_role_permissions, public.policy_role_conditions,
    public.user_role_assignments, public.audit_events TO console_leave_definer;
GRANT INSERT, UPDATE ON public.employees, public.leave_requests TO console_leave_definer;
GRANT UPDATE ON public.data_import_runs TO console_leave_definer;
GRANT INSERT ON public.leave_charge_resolutions, public.leave_balance_import_receipts,
    public.audit_events TO console_leave_definer;

-- The general runtime identity may read leave data. During this expand phase
-- it also retains the pre-0166 binary's create/decide surface so an application
-- rollback does not fail against the migrated schema. The trigger below
-- accepts only legacy pending creation or a single pending-to-terminal
-- decision; exact-charge and other writes remain behind the command
-- capability. A later contract migration may remove this bridge
-- only after rollback to every pre-0166 binary is no longer supported.
GRANT SELECT ON public.leave_requests, public.leave_charge_resolutions TO console_rt;
GRANT INSERT ON public.leave_requests TO console_rt;
GRANT UPDATE ON public.leave_requests TO console_rt;
REVOKE DELETE, TRUNCATE ON public.leave_requests FROM console_rt, console_leave_cmd, PUBLIC;
REVOKE INSERT, UPDATE, DELETE, TRUNCATE ON public.leave_charge_resolutions
    FROM console_rt, console_leave_cmd, PUBLIC;

-- These invoker-rights guards fail closed if any future grant accidentally
-- restores an alternate write path. The command routines execute table DML as
-- console_leave_definer; neither console_rt nor console_leave_cmd can impersonate it.
CREATE FUNCTION leave_api.protected_request_writer_guard()
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
           OR NEW.status NOT IN ('approved', 'returned', 'rejected')
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
        IF NEW.status = 'approved' THEN
            NEW.charge_state := 'legacy_unverified';
            NEW.charge_review_reasons := ARRAY[]::TEXT[];
            NEW.charge_units := OLD.days;
        ELSE
            NEW.charge_state := 'not_required';
            NEW.charge_review_reasons := ARRAY[]::TEXT[];
            NEW.charge_units := NULL;
        END IF;
        RETURN NEW;
    ELSIF current_user <> 'console_leave_definer' THEN
        RAISE EXCEPTION USING ERRCODE = '42501', MESSAGE = 'leave_write.command_required';
    END IF;
    IF TG_OP = 'DELETE' THEN RETURN OLD; END IF;
    RETURN NEW;
END;
$$;
ALTER FUNCTION leave_api.protected_request_writer_guard() OWNER TO console_leave_definer;

CREATE TRIGGER trg_leave_requests_command_only
    BEFORE INSERT OR UPDATE OR DELETE ON public.leave_requests
    FOR EACH ROW EXECUTE FUNCTION leave_api.protected_request_writer_guard();
CREATE TRIGGER trg_leave_charge_resolutions_command_only
    BEFORE INSERT OR UPDATE OR DELETE ON public.leave_charge_resolutions
    FOR EACH ROW EXECUTE FUNCTION leave_api.protected_request_writer_guard();
CREATE TRIGGER trg_leave_balance_import_receipts_command_only
    BEFORE INSERT OR UPDATE OR DELETE ON public.leave_balance_import_receipts
    FOR EACH ROW EXECUTE FUNCTION leave_api.protected_request_writer_guard();

CREATE FUNCTION leave_api.employee_leave_writer_guard()
RETURNS TRIGGER LANGUAGE plpgsql SET search_path = pg_catalog AS $$
BEGIN
    IF current_user <> 'console_leave_definer' THEN
        IF current_user = 'console_rt' THEN
            -- Expand compatibility is intentionally limited to the surface the
            -- f6ff236 binary already owned. Its immediate and staged employee
            -- imports INSERT/UPSERT the protected balance columns directly, and
            -- its legacy leave approval updates used/remaining in-transaction.
            -- It never writes the additive home-branch authority introduced by
            -- 0166, which remains command-only throughout the mixed-version
            -- window. There is no trustworthy marker in the old SQL that can
            -- distinguish an import balance write from another console_rt balance
            -- write; pretending otherwise would break rollback or create a
            -- bypassable guard. The later contract migration removes this
            -- compatibility branch after f6ff236 is outside the rollback set.
            IF (TG_OP = 'INSERT' AND NEW.home_branch_id IS NOT NULL)
               OR (TG_OP = 'UPDATE'
                   AND NEW.home_branch_id IS DISTINCT FROM OLD.home_branch_id) THEN
                RAISE EXCEPTION USING ERRCODE = '42501', MESSAGE = 'leave_write.command_required';
            END IF;
        ELSIF TG_OP = 'INSERT' AND (
            NEW.home_branch_id IS NOT NULL
            OR NEW.leave_accrued IS NOT NULL
            OR NEW.leave_used IS NOT NULL
            OR NEW.leave_remaining IS NOT NULL
        ) THEN
            RAISE EXCEPTION USING ERRCODE = '42501', MESSAGE = 'leave_write.command_required';
        ELSIF TG_OP = 'UPDATE' AND (
            NEW.home_branch_id IS DISTINCT FROM OLD.home_branch_id
            OR NEW.leave_accrued IS DISTINCT FROM OLD.leave_accrued
            OR NEW.leave_used IS DISTINCT FROM OLD.leave_used
            OR NEW.leave_remaining IS DISTINCT FROM OLD.leave_remaining
        ) THEN
            RAISE EXCEPTION USING ERRCODE = '42501', MESSAGE = 'leave_write.command_required';
        END IF;
    END IF;
    RETURN NEW;
END;
$$;
ALTER FUNCTION leave_api.employee_leave_writer_guard() OWNER TO console_leave_definer;
CREATE TRIGGER trg_employees_leave_command_only
    BEFORE INSERT OR UPDATE ON public.employees
    FOR EACH ROW EXECUTE FUNCTION leave_api.employee_leave_writer_guard();

-- console_rt still owns the preview/dry-run workflow created in migration 0070.
-- During this expand release, the exact f6ff236 DRY_RUN -> APPLIED statement is
-- also retained so old replicas and rollback can finish staged employee imports.
-- The new binary uses leave_api.apply_employee_import_batch instead. A later
-- numbered contract migration removes this compatibility transition only after
-- this release is the rollback floor. Other employee_hr terminal mutations stay
-- fail-closed.
CREATE FUNCTION leave_api.employee_import_run_writer_guard()
RETURNS TRIGGER LANGUAGE plpgsql SET search_path = pg_catalog AS $$
BEGIN
    IF TG_OP = 'UPDATE' AND NEW.entity_type IS DISTINCT FROM OLD.entity_type THEN
        RAISE EXCEPTION USING ERRCODE = '23514', MESSAGE = 'data_import_run.entity_type_immutable';
    END IF;
    IF current_user <> 'console_leave_definer' THEN
        IF TG_OP = 'INSERT' AND NEW.entity_type = 'employee_hr' AND (
            NEW.status = 'APPLIED'
            OR NEW.apply_summary IS DISTINCT FROM '{}'::JSONB
            OR NEW.applied_by IS NOT NULL
            OR NEW.applied_at IS NOT NULL
        ) THEN
            RAISE EXCEPTION USING ERRCODE = '42501', MESSAGE = 'employee_import_run.command_required';
        ELSIF TG_OP = 'UPDATE'
           AND (OLD.entity_type = 'employee_hr' OR NEW.entity_type = 'employee_hr')
           AND (
                ((OLD.status = 'APPLIED' OR NEW.status = 'APPLIED')
                    AND NEW.status IS DISTINCT FROM OLD.status)
                OR NEW.apply_summary IS DISTINCT FROM OLD.apply_summary
                OR NEW.applied_by IS DISTINCT FROM OLD.applied_by
                OR NEW.applied_at IS DISTINCT FROM OLD.applied_at
           ) THEN
            IF current_user <> 'console_rt'
               OR OLD.entity_type <> 'employee_hr'
               OR NEW.entity_type <> 'employee_hr'
               OR OLD.status <> 'DRY_RUN'
               OR NEW.status <> 'APPLIED'
               OR NEW.applied_by IS NULL
               OR NEW.applied_at IS NULL
               OR (pg_catalog.to_jsonb(NEW) - ARRAY[
                    'status','apply_summary','applied_by','applied_at','updated_at'
                  ]::TEXT[]) IS DISTINCT FROM
                  (pg_catalog.to_jsonb(OLD) - ARRAY[
                    'status','apply_summary','applied_by','applied_at','updated_at'
                  ]::TEXT[]) THEN
                RAISE EXCEPTION USING ERRCODE = '42501', MESSAGE = 'employee_import_run.command_required';
            END IF;
        END IF;
    END IF;
    RETURN NEW;
END;
$$;
ALTER FUNCTION leave_api.employee_import_run_writer_guard() OWNER TO console_leave_definer;
CREATE TRIGGER trg_data_import_runs_employee_hr_command_only
    BEFORE INSERT OR UPDATE ON public.data_import_runs
    FOR EACH ROW EXECUTE FUNCTION leave_api.employee_import_run_writer_guard();

-- The old staged-import adapter updated the run before appending its audit.
-- A deferred check preserves that ordering without accepting a committed
-- APPLIED run unless exactly one correlated audit exists in the same database
-- transaction. The active same-tenant actor join also closes the historical
-- applied_by foreign-key shape, which references users(id) but not org_id.
CREATE FUNCTION leave_api.employee_import_apply_audit_required()
RETURNS TRIGGER
LANGUAGE plpgsql SECURITY DEFINER
SET search_path = pg_catalog SET row_security = on AS $$
DECLARE
    v_matching_audits BIGINT;
BEGIN
    IF OLD.entity_type = 'employee_hr'
       AND NEW.entity_type = 'employee_hr'
       AND OLD.status = 'DRY_RUN'
       AND NEW.status = 'APPLIED' THEN
        PERFORM pg_catalog.set_config('app.current_org', NEW.org_id::TEXT, true);
        SELECT pg_catalog.count(*)
          INTO v_matching_audits
          FROM public.audit_events a
          JOIN public.users u
            ON u.id = a.actor
           AND u.org_id = a.org_id
           AND u.is_active
         WHERE a.org_id = NEW.org_id
           AND a.action = 'data_import.apply'
           AND a.target_type = 'data_import_run'
           AND a.target_id = NEW.id::TEXT
           AND a.actor = NEW.applied_by
           AND a.xmin = pg_catalog.pg_current_xact_id()::xid;
        IF v_matching_audits <> 1 THEN
            RAISE EXCEPTION USING
                ERRCODE = '23514',
                MESSAGE = 'employee_import_run.current_transaction_audit_required';
        END IF;
    END IF;
    RETURN NEW;
END;
$$;
ALTER FUNCTION leave_api.employee_import_apply_audit_required() OWNER TO console_leave_definer;
CREATE CONSTRAINT TRIGGER trg_data_import_runs_employee_hr_audit_required
    AFTER UPDATE ON public.data_import_runs
    DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW EXECUTE FUNCTION leave_api.employee_import_apply_audit_required();

-- Protected leave audit actions are appendable only by the same pinned
-- command definer. This closes the tempting alternate path where console_rt forges
-- a success or blocked-decision event without executing the guarded command.
CREATE FUNCTION leave_api.protected_audit_writer_guard()
RETURNS TRIGGER LANGUAGE plpgsql SET search_path = pg_catalog AS $$
BEGIN
    -- The expand window also preserves the base adapter's atomic audit write.
    -- It may attest only the legacy create/decide mutation already visible in
    -- the same transaction; exact-charge and employee-import actions remain
    -- exclusive to the command definer.
    IF current_user = 'console_rt' AND NEW.action = 'leave_request.create'
       AND NEW.target_type = 'leave_request'
       AND EXISTS (
           SELECT 1 FROM public.leave_requests lr
           WHERE lr.org_id = NEW.org_id
             AND lr.id::TEXT = NEW.target_id
             AND lr.requester_user_id = NEW.actor
             AND lr.branch_id IS NOT DISTINCT FROM NEW.branch_id
             AND lr.status = 'pending'
             AND lr.days IS NOT NULL
             AND lr.xmin = pg_catalog.pg_current_xact_id()::xid
       ) THEN
        RETURN NEW;
    ELSIF current_user = 'console_rt' AND NEW.action = 'leave_request.decide'
       AND NEW.target_type = 'leave_request'
       AND EXISTS (
           SELECT 1 FROM public.leave_requests lr
           WHERE lr.org_id = NEW.org_id
             AND lr.id::TEXT = NEW.target_id
             AND lr.decided_by = NEW.actor
             AND lr.branch_id IS NOT DISTINCT FROM NEW.branch_id
             AND lr.status IN ('approved', 'returned', 'rejected')
             AND lr.xmin = pg_catalog.pg_current_xact_id()::xid
       ) THEN
        RETURN NEW;
    ELSIF current_user = 'console_rt' AND NEW.action = 'data_import.apply'
       AND NEW.target_type = 'data_import_run'
       AND NEW.target_id ~ '^[0-9a-fA-F-]{36}$'
       AND NEW.branch_id IS NULL
       AND NEW.before_snap IS NULL
       AND NEW.ip IS NULL
       AND NEW.user_agent IS NULL
       AND NEW.auth_method IS NULL
       AND NEW.device IS NULL
       AND NEW.classification_badges IS NULL
       AND NEW.anomaly IS NULL
       AND NEW.reason IS NULL
       AND NEW.after_snap IS NOT DISTINCT FROM pg_catalog.jsonb_build_object(
            'run_id', NEW.target_id::UUID,
            'entity_type', 'employee_hr',
            'gate_outcome', pg_catalog.jsonb_build_object(
                'gates', pg_catalog.jsonb_build_array(
                    pg_catalog.jsonb_build_object(
                        'gate', 'authority',
                        'status', pg_catalog.jsonb_build_object('status', 'not_required')
                    ),
                    pg_catalog.jsonb_build_object(
                        'gate', 'self_checklist',
                        'status', pg_catalog.jsonb_build_object('status', 'satisfied')
                    ),
                    pg_catalog.jsonb_build_object(
                        'gate', 'four_eyes',
                        'status', pg_catalog.jsonb_build_object('status', 'not_required')
                    ),
                    pg_catalog.jsonb_build_object(
                        'gate', 'egress_dlp',
                        'status', pg_catalog.jsonb_build_object('status', 'not_required')
                    )
                ),
                'allow', true
            )
       )
       AND EXISTS (
           SELECT 1 FROM public.data_import_runs r
           JOIN public.users u
             ON u.id = NEW.actor
            AND u.org_id = NEW.org_id
            AND u.is_active
           WHERE r.org_id = NEW.org_id
             AND r.id = NEW.target_id::UUID
             AND r.entity_type = 'employee_hr'
             AND r.status = 'APPLIED'
             AND r.applied_by = NEW.actor
             AND r.xmin = pg_catalog.pg_current_xact_id()::xid
       ) THEN
        -- Exact f6ff236 with_audit ordering: the legacy run transition is
        -- visible in this transaction before its run-level audit is appended.
        -- This does not authorize command-owned balance receipts or any other
        -- protected leave action.
        RETURN NEW;
    END IF;
    IF NEW.action = ANY (ARRAY[
        'employee.home_branch_set',
        'employee.leave_balance_import',
        'data_import.apply',
        'leave_request.create',
        'leave_request.charge_resolve',
        'leave_request.decide',
        'leave_request.approval_blocked'
    ]::TEXT[]) AND current_user <> 'console_leave_definer' THEN
        RAISE EXCEPTION USING ERRCODE = '42501', MESSAGE = 'leave_audit.command_required';
    END IF;
    RETURN NEW;
END;
$$;
ALTER FUNCTION leave_api.protected_audit_writer_guard() OWNER TO console_leave_definer;
CREATE TRIGGER trg_audit_events_leave_command_only
    BEFORE INSERT ON public.audit_events
    FOR EACH ROW EXECUTE FUNCTION leave_api.protected_audit_writer_guard();

-- Queue the obligation while the invoker is still the legacy runtime role.
-- PostgreSQL evaluates a constraint trigger's WHEN clause at the row change,
-- before deferring the trigger body, so command-definer writes are not confused
-- with the temporary N-1 compatibility bridge. At commit, require exactly one
-- audit row written by the same transaction. The immediate audit guard above
-- independently rejects delayed or mismatched receipts by binding to row xmin.
CREATE FUNCTION leave_api.legacy_leave_audit_required()
RETURNS TRIGGER
LANGUAGE plpgsql SECURITY DEFINER
SET search_path = pg_catalog SET row_security = on AS $$
DECLARE
    v_action TEXT;
    v_actor UUID;
    v_matching_audits BIGINT;
BEGIN
    IF TG_OP = 'INSERT' THEN
        v_action := 'leave_request.create';
        v_actor := NEW.requester_user_id;
    ELSIF TG_OP = 'UPDATE'
       AND OLD.status = 'pending'
       AND NEW.status IN ('approved', 'returned', 'rejected') THEN
        v_action := 'leave_request.decide';
        v_actor := NEW.decided_by;
    ELSE
        RETURN NEW;
    END IF;

    PERFORM pg_catalog.set_config('app.current_org', NEW.org_id::TEXT, true);
    SELECT pg_catalog.count(*)
      INTO v_matching_audits
      FROM public.audit_events a
     WHERE a.org_id = NEW.org_id
       AND a.action = v_action
       AND a.target_type = 'leave_request'
       AND a.target_id = NEW.id::TEXT
       AND a.actor = v_actor
       AND a.branch_id IS NOT DISTINCT FROM NEW.branch_id
       AND a.xmin = pg_catalog.pg_current_xact_id()::xid;
    IF v_matching_audits <> 1 THEN
        RAISE EXCEPTION USING
            ERRCODE = '23514',
            MESSAGE = 'leave_request.current_transaction_audit_required';
    END IF;
    RETURN NEW;
END;
$$;
ALTER FUNCTION leave_api.legacy_leave_audit_required() OWNER TO console_leave_definer;
CREATE CONSTRAINT TRIGGER trg_leave_requests_legacy_audit_required
    AFTER INSERT OR UPDATE ON public.leave_requests
    DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW
    WHEN (current_user = 'console_rt')
    EXECUTE FUNCTION leave_api.legacy_leave_audit_required();

CREATE FUNCTION leave_api.assert_context(
    p_org_id UUID, p_actor UUID, p_trace_id TEXT, p_span_id TEXT
) RETURNS VOID
LANGUAGE plpgsql SECURITY DEFINER
SET search_path = pg_catalog SET row_security = on AS $$
BEGIN
    IF p_org_id IS NULL OR p_actor IS NULL THEN
        RAISE EXCEPTION USING ERRCODE = '22023', MESSAGE = 'leave_write.context_required';
    END IF;
    PERFORM pg_catalog.set_config('app.current_org', p_org_id::TEXT, true);
    IF NOT EXISTS (
        SELECT 1 FROM public.users u
        WHERE u.id = p_actor AND u.org_id = p_org_id AND u.is_active
    ) THEN
        RAISE EXCEPTION USING ERRCODE = '42501', MESSAGE = 'leave_write.actor_forbidden';
    END IF;
    IF p_trace_id !~ '^[0-9a-f]{32}$' OR p_span_id !~ '^[0-9a-f]{16}$' THEN
        RAISE EXCEPTION USING ERRCODE = '22023', MESSAGE = 'leave_write.invalid_trace';
    END IF;
END;
$$;
ALTER FUNCTION leave_api.assert_context(UUID, UUID, TEXT, TEXT) OWNER TO console_leave_definer;

CREATE FUNCTION leave_api.assert_manager(
    p_org_id UUID, p_actor UUID, p_branch_id UUID
) RETURNS VOID
LANGUAGE plpgsql SECURITY DEFINER
SET search_path = pg_catalog SET row_security = on AS $$
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM public.users u
        WHERE u.id = p_actor AND u.org_id = p_org_id AND u.is_active
          AND (u.roles @> ARRAY['SUPER_ADMIN']::TEXT[]
               OR (u.roles @> ARRAY['ADMIN']::TEXT[] AND EXISTS (
                    SELECT 1 FROM public.user_branches ub
                    WHERE ub.org_id = p_org_id AND ub.user_id = p_actor
                      AND ub.branch_id = p_branch_id)))
    ) THEN
        RAISE EXCEPTION USING ERRCODE = '42501', MESSAGE = 'leave_write.branch_forbidden';
    END IF;
END;
$$;
ALTER FUNCTION leave_api.assert_manager(UUID, UUID, UUID) OWNER TO console_leave_definer;

CREATE FUNCTION leave_api.assert_org_admin(
    p_org_id UUID, p_actor UUID
) RETURNS VOID
LANGUAGE plpgsql SECURITY DEFINER
SET search_path = pg_catalog SET row_security = on AS $$
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM public.users u
        WHERE u.id = p_actor AND u.org_id = p_org_id AND u.is_active
          AND u.roles @> ARRAY['SUPER_ADMIN']::TEXT[]
    ) THEN
        RAISE EXCEPTION USING ERRCODE = '42501', MESSAGE = 'leave_write.org_admin_required';
    END IF;
END;
$$;
ALTER FUNCTION leave_api.assert_org_admin(UUID, UUID) OWNER TO console_leave_definer;

CREATE FUNCTION leave_api.assert_employee_importer(
    p_org_id UUID, p_actor UUID
) RETURNS VOID
LANGUAGE plpgsql SECURITY DEFINER
SET search_path = pg_catalog SET row_security = on AS $$
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM public.users u
        WHERE u.id = p_actor AND u.org_id = p_org_id AND u.is_active
          AND (
              u.roles @> ARRAY['SUPER_ADMIN']::TEXT[]
              OR (
                  -- The application can produce an org-wide custom grant only
                  -- from an EXECUTIVE principal's live All scope. Re-evaluate
                  -- the persisted grant here rather than trusting a caller
                  -- boolean at the SECURITY DEFINER boundary.
                  u.roles @> ARRAY['EXECUTIVE']::TEXT[]
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
                                  -- Branch conditions narrow All and therefore
                                  -- cannot authorize this org-wide command.
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
        RAISE EXCEPTION USING ERRCODE = '42501', MESSAGE = 'leave_balance_import.actor_forbidden';
    END IF;
END;
$$;
ALTER FUNCTION leave_api.assert_employee_importer(UUID, UUID) OWNER TO console_leave_definer;

-- Validate and canonicalize charge evidence entirely in the database. The
-- caller supplies source evidence, never the authoritative total, branch,
-- version, or digest. The returned snapshot is what is persisted and audited.
CREATE FUNCTION leave_api.canonical_charge_snapshot(
    p_home_branch_id UUID,
    p_leave_type TEXT,
    p_partial_day_period TEXT,
    p_start_date DATE,
    p_end_date DATE,
    p_date_charges JSONB,
    p_calendar_revision_ref JSONB,
    p_policy_revision_ref JSONB,
    p_supporting_source_refs JSONB
) RETURNS TABLE(snapshot JSONB, charge_units NUMERIC(16,6), server_digest TEXT)
LANGUAGE plpgsql SECURITY DEFINER
SET search_path = pg_catalog SET row_security = on AS $$
DECLARE
    v_expected_days INTEGER;
    v_total NUMERIC(16,6);
    v_scheduled INTEGER;
    v_body JSONB;
BEGIN
    IF p_leave_type NOT IN ('annual', 'half_day')
       OR p_end_date < p_start_date
       OR (p_leave_type = 'half_day') IS DISTINCT FROM (p_partial_day_period IS NOT NULL)
       OR (p_partial_day_period IS NOT NULL AND p_partial_day_period NOT IN ('am', 'pm')) THEN
        RAISE EXCEPTION USING ERRCODE = '22023', MESSAGE = 'leave_charge.invalid_intent';
    END IF;
    IF pg_catalog.jsonb_typeof(p_date_charges) <> 'array'
       OR pg_catalog.jsonb_array_length(p_date_charges) = 0
       OR pg_catalog.jsonb_typeof(p_calendar_revision_ref) <> 'object'
       OR pg_catalog.jsonb_typeof(p_policy_revision_ref) <> 'object'
       OR pg_catalog.jsonb_typeof(p_supporting_source_refs) <> 'array' THEN
        RAISE EXCEPTION USING ERRCODE = '22023', MESSAGE = 'leave_charge.invalid_evidence_shape';
    END IF;
    IF EXISTS (
        SELECT 1 FROM (VALUES (p_calendar_revision_ref), (p_policy_revision_ref)) r(doc)
        WHERE NULLIF(pg_catalog.btrim(doc->>'kind'), '') IS NULL
           OR NULLIF(pg_catalog.btrim(doc->>'reference'), '') IS NULL
           OR NULLIF(pg_catalog.btrim(doc->>'revision'), '') IS NULL
    ) OR EXISTS (
        SELECT 1 FROM pg_catalog.jsonb_array_elements(p_supporting_source_refs) x
        WHERE pg_catalog.jsonb_typeof(x) <> 'object'
           OR NULLIF(pg_catalog.btrim(x->>'kind'), '') IS NULL
           OR NULLIF(pg_catalog.btrim(x->>'reference'), '') IS NULL
           OR NULLIF(pg_catalog.btrim(x->>'revision'), '') IS NULL
    ) THEN
        RAISE EXCEPTION USING ERRCODE = '22023', MESSAGE = 'leave_charge.invalid_source_reference';
    END IF;

    v_expected_days := p_end_date - p_start_date + 1;
    IF pg_catalog.jsonb_array_length(p_date_charges) <> v_expected_days
       OR (SELECT pg_catalog.count(DISTINCT (x->>'date')::DATE)
           FROM pg_catalog.jsonb_array_elements(p_date_charges) x) <> v_expected_days
       OR (SELECT pg_catalog.min((x->>'date')::DATE)
           FROM pg_catalog.jsonb_array_elements(p_date_charges) x) <> p_start_date
       OR (SELECT pg_catalog.max((x->>'date')::DATE)
           FROM pg_catalog.jsonb_array_elements(p_date_charges) x) <> p_end_date THEN
        RAISE EXCEPTION USING ERRCODE = '22023', MESSAGE = 'leave_charge.incomplete_date_evidence';
    END IF;
    IF EXISTS (
        SELECT 1 FROM pg_catalog.jsonb_array_elements(p_date_charges) x
        WHERE pg_catalog.jsonb_typeof(x) <> 'object'
           OR x->>'kind' IS NOT NULL
           OR pg_catalog.jsonb_typeof(x->'obligation') <> 'object'
           OR (x->'obligation'->>'kind') NOT IN ('scheduled', 'not_scheduled')
           OR NULLIF(x->>'units', '') IS NULL
           OR (x->>'units') !~ '^[0-9]{1,3}(\.[0-9]{1,6})?$'
           OR (x->>'units')::NUMERIC < 0
           OR (x->>'units')::NUMERIC > 1
           OR ((x->'obligation'->>'kind') = 'scheduled'
               AND (COALESCE((x->'obligation'->>'minutes')::INTEGER, 0) <= 0
                    OR (x->>'units')::NUMERIC <= 0))
           OR ((x->'obligation'->>'kind') = 'not_scheduled'
               AND ((x->>'units')::NUMERIC <> 0
                    OR NULLIF(pg_catalog.btrim(x->'obligation'->>'basis'), '') IS NULL))
    ) THEN
        RAISE EXCEPTION USING ERRCODE = '22023', MESSAGE = 'leave_charge.invalid_date_evidence';
    END IF;

    SELECT COALESCE(pg_catalog.sum((x->>'units')::NUMERIC), 0),
           pg_catalog.count(*) FILTER (WHERE x->'obligation'->>'kind' = 'scheduled')
      INTO v_total, v_scheduled
      FROM pg_catalog.jsonb_array_elements(p_date_charges) x;
    IF v_total <= 0 OR v_total > 366 OR pg_catalog.scale(v_total) > 6 THEN
        RAISE EXCEPTION USING ERRCODE = '22023', MESSAGE = 'leave_charge.invalid_total';
    END IF;
    IF p_leave_type = 'annual' AND EXISTS (
        SELECT 1 FROM pg_catalog.jsonb_array_elements(p_date_charges) x
        WHERE x->'obligation'->>'kind' = 'scheduled' AND (x->>'units')::NUMERIC <> 1
    ) THEN
        RAISE EXCEPTION USING ERRCODE = '22023', MESSAGE = 'leave_charge.annual_units_must_be_one';
    END IF;
    IF p_leave_type = 'half_day'
       AND (p_start_date <> p_end_date OR v_scheduled <> 1 OR v_total >= 1) THEN
        RAISE EXCEPTION USING ERRCODE = '22023', MESSAGE = 'leave_charge.invalid_half_day';
    END IF;

    v_body := pg_catalog.jsonb_build_object(
        'home_branch_id', p_home_branch_id,
        'leave_type', p_leave_type,
        'partial_day_period', p_partial_day_period,
        'calendar_revision_ref', p_calendar_revision_ref,
        'policy_revision_ref', p_policy_revision_ref,
        'supporting_source_refs', p_supporting_source_refs,
        'date_charges', p_date_charges,
        'total_units', pg_catalog.to_char(v_total, 'FM999999999999990.000000')
    );
    server_digest := pg_catalog.encode(public.digest(
        pg_catalog.convert_to(v_body::TEXT, 'UTF8'), 'sha256'), 'hex');
    snapshot := v_body || pg_catalog.jsonb_build_object('server_digest', server_digest);
    charge_units := v_total;
    RETURN NEXT;
END;
$$;
ALTER FUNCTION leave_api.canonical_charge_snapshot(UUID, TEXT, TEXT, DATE, DATE, JSONB, JSONB, JSONB, JSONB)
    OWNER TO console_leave_definer;

CREATE FUNCTION leave_api.create_request(
    p_org_id UUID, p_request_id UUID, p_requester UUID,
    p_leave_type TEXT, p_start_date DATE, p_end_date DATE, p_reason TEXT,
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
       OR p_reason IS NULL OR char_length(pg_catalog.btrim(p_reason)) NOT BETWEEN 1 AND 500
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
            'reason', pg_catalog.btrim(p_reason),
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
         leave_type, start_date, end_date, reason, partial_day_period, status,
         charge_state, charge_review_reasons, charge_units, charge_version,
         submission_key, submission_digest, submission_initial_charge_version)
    VALUES
        (p_request_id, p_org_id, v_branch, p_requester, v_subject,
         CASE WHEN p_leave_type = 'half_day' THEN 0.5::NUMERIC
              ELSE (p_end_date - p_start_date + 1)::NUMERIC END,
         p_leave_type, p_start_date, p_end_date, pg_catalog.btrim(p_reason),
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
ALTER FUNCTION leave_api.create_request(UUID, UUID, UUID, TEXT, DATE, DATE, TEXT, TEXT, TEXT[], UUID, JSONB, JSONB, JSONB, JSONB, UUID, TEXT, TEXT)
    OWNER TO console_leave_definer;

CREATE FUNCTION leave_api.resolve_charge(
    p_org_id UUID, p_request_id UUID, p_resolver UUID, p_expected_version BIGINT,
    p_date_charges JSONB, p_calendar_revision_ref JSONB,
    p_policy_revision_ref JSONB, p_supporting_source_refs JSONB,
    p_trace_id TEXT, p_span_id TEXT
) RETURNS TABLE(request_id UUID, request_version BIGINT, charge_version BIGINT,
                charge_micros BIGINT, server_digest TEXT)
LANGUAGE plpgsql SECURITY DEFINER
SET search_path = pg_catalog SET row_security = on AS $$
DECLARE
    v_request public.leave_requests%ROWTYPE;
    v_snapshot JSONB;
    v_units NUMERIC(16,6);
    v_digest TEXT;
    v_next_request BIGINT;
    v_next_charge BIGINT;
    v_resolution_id UUID := public.gen_random_uuid();
BEGIN
    PERFORM leave_api.assert_context(p_org_id, p_resolver, p_trace_id, p_span_id);
    SELECT * INTO v_request FROM public.leave_requests lr
     WHERE lr.org_id = p_org_id AND lr.id = p_request_id FOR UPDATE;
    IF NOT FOUND THEN RAISE EXCEPTION USING ERRCODE='P0001', MESSAGE='leave_resolve.not_found'; END IF;
    PERFORM leave_api.assert_manager(p_org_id, p_resolver, v_request.branch_id);
    IF v_request.status <> 'pending' THEN
        RAISE EXCEPTION USING ERRCODE='P0001', MESSAGE='leave_resolve.not_pending';
    END IF;
    IF v_request.request_version <> p_expected_version THEN
        RAISE EXCEPTION USING ERRCODE='40001', MESSAGE='leave_resolve.concurrent_modification';
    END IF;
    IF v_request.requester_user_id = p_resolver THEN
        RAISE EXCEPTION USING ERRCODE='42501', MESSAGE='leave_resolve.requester_forbidden';
    END IF;
    SELECT c.snapshot, c.charge_units, c.server_digest
      INTO v_snapshot, v_units, v_digest
      FROM leave_api.canonical_charge_snapshot(v_request.branch_id,
           v_request.leave_type, v_request.partial_day_period,
           v_request.start_date, v_request.end_date, p_date_charges,
           p_calendar_revision_ref, p_policy_revision_ref,
           p_supporting_source_refs) c;
    v_next_request := v_request.request_version + 1;
    v_next_charge := v_request.charge_version + 1;
    IF v_next_request <= v_request.request_version OR v_next_charge <= v_request.charge_version THEN
        RAISE EXCEPTION USING ERRCODE='22003', MESSAGE='leave_resolve.version_exhausted';
    END IF;
    INSERT INTO public.leave_charge_resolutions
        (id, org_id, request_id, charge_version, home_branch_id, charge_units,
         date_charges, calendar_revision_ref, policy_revision_ref,
         supporting_source_refs, snapshot, server_digest, resolution_origin,
         resolved_by, resolved_at)
    VALUES
        (v_resolution_id, p_org_id, p_request_id, v_next_charge, v_request.branch_id,
         v_units, p_date_charges, p_calendar_revision_ref, p_policy_revision_ref,
         p_supporting_source_refs, v_snapshot, v_digest, 'manual', p_resolver, pg_catalog.statement_timestamp());
    UPDATE public.leave_requests
       SET charge_state='resolved', charge_review_reasons=ARRAY[]::TEXT[],
           charge_units=v_units, request_version=v_next_request,
           charge_version=v_next_charge,
           current_charge_resolution_id=v_resolution_id
     WHERE org_id=p_org_id AND id=p_request_id;
    INSERT INTO public.audit_events
        (actor, action, target_type, target_id, branch_id, before_snap, after_snap,
         trace_id, span_id, occurred_at, org_id)
    VALUES
        (p_resolver,'leave_request.charge_resolve','leave_request',p_request_id::TEXT,
         v_request.branch_id,
         pg_catalog.jsonb_build_object('charge_state',v_request.charge_state,
             'request_version',v_request.request_version,
             'charge_version',v_request.charge_version),
         pg_catalog.jsonb_build_object('charge_state','resolved',
             'request_version',v_next_request,'charge_version',v_next_charge,
             'charge_units',v_units,'server_digest',v_digest),
         p_trace_id,p_span_id,pg_catalog.statement_timestamp(),p_org_id);
    RETURN QUERY SELECT p_request_id,v_next_request,v_next_charge,
        (v_units * 1000000)::BIGINT,v_digest;
END;
$$;
ALTER FUNCTION leave_api.resolve_charge(UUID, UUID, UUID, BIGINT, JSONB, JSONB, JSONB, JSONB, TEXT, TEXT)
    OWNER TO console_leave_definer;

CREATE FUNCTION leave_api.decide_request(
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
    -- NULL is the v1 wire contract: the row lock plus pending-state predicate
    -- gives first-writer-wins behavior. Version-aware callers retain exact CAS.
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

CREATE FUNCTION leave_api.import_employee_leave_balance(
    p_org_id UUID, p_employee_id UUID, p_expected_updated_at TIMESTAMPTZ,
    p_leave_accrued TEXT, p_leave_used TEXT, p_leave_remaining TEXT,
    p_source_kind TEXT, p_source_ref TEXT, p_idempotency_key TEXT,
    p_actor UUID, p_trace_id TEXT, p_span_id TEXT
) RETURNS TABLE(employee_id UUID, updated_at TIMESTAMPTZ, changed BOOLEAN, replayed BOOLEAN)
LANGUAGE plpgsql SECURITY DEFINER
SET search_path = pg_catalog SET row_security = on AS $$
DECLARE
    v_old_accrued NUMERIC(16,6);
    v_old_used NUMERIC(16,6);
    v_old_remaining NUMERIC(16,6);
    v_old_updated TIMESTAMPTZ;
    v_new_accrued NUMERIC(16,6);
    v_new_used NUMERIC(16,6);
    v_new_remaining NUMERIC(16,6);
    v_new_updated TIMESTAMPTZ;
    v_payload JSONB;
    v_digest TEXT;
    v_receipt public.leave_balance_import_receipts%ROWTYPE;
    v_changed BOOLEAN;
BEGIN
    PERFORM leave_api.assert_context(p_org_id,p_actor,p_trace_id,p_span_id);
    PERFORM leave_api.assert_employee_importer(p_org_id,p_actor);
    IF p_source_kind <> 'employee_import'
       OR char_length(pg_catalog.btrim(p_source_ref)) NOT BETWEEN 1 AND 256
       OR char_length(pg_catalog.btrim(p_idempotency_key)) NOT BETWEEN 16 AND 256 THEN
        RAISE EXCEPTION USING ERRCODE='22023', MESSAGE='leave_balance_import.invalid_source';
    END IF;
    IF (p_leave_accrued IS NOT NULL AND p_leave_accrued !~ '^-?(0|[1-9][0-9]*)(\.[0-9]{1,6})?$')
       OR (p_leave_used IS NOT NULL AND p_leave_used !~ '^-?(0|[1-9][0-9]*)(\.[0-9]{1,6})?$')
       OR (p_leave_remaining IS NOT NULL AND p_leave_remaining !~ '^-?(0|[1-9][0-9]*)(\.[0-9]{1,6})?$') THEN
        RAISE EXCEPTION USING ERRCODE='22023', MESSAGE='leave_balance_import.invalid_amount';
    END IF;
    BEGIN
        v_new_accrued := p_leave_accrued::NUMERIC(16,6);
        v_new_used := p_leave_used::NUMERIC(16,6);
        v_new_remaining := p_leave_remaining::NUMERIC(16,6);
    EXCEPTION WHEN numeric_value_out_of_range THEN
        RAISE EXCEPTION USING ERRCODE='22003', MESSAGE='leave_balance_import.amount_out_of_range';
    END;
    v_payload := pg_catalog.jsonb_build_object(
        'employee_id',p_employee_id,
        'leave_accrued',CASE WHEN v_new_accrued IS NULL THEN NULL ELSE v_new_accrued::TEXT END,
        'leave_used',CASE WHEN v_new_used IS NULL THEN NULL ELSE v_new_used::TEXT END,
        'leave_remaining',CASE WHEN v_new_remaining IS NULL THEN NULL ELSE v_new_remaining::TEXT END,
        'source_kind',p_source_kind,'source_ref',pg_catalog.btrim(p_source_ref)
    );
    v_digest := pg_catalog.encode(public.digest(v_payload::TEXT,'sha256'),'hex');
    PERFORM pg_catalog.pg_advisory_xact_lock(
        pg_catalog.hashtextextended(p_org_id::TEXT || ':' || p_idempotency_key, 166)
    );
    SELECT * INTO v_receipt FROM public.leave_balance_import_receipts r
     WHERE r.org_id=p_org_id AND r.idempotency_key=p_idempotency_key;
    IF FOUND THEN
        IF v_receipt.employee_id IS DISTINCT FROM p_employee_id
           OR v_receipt.payload_digest IS DISTINCT FROM v_digest THEN
            RAISE EXCEPTION USING ERRCODE='22023', MESSAGE='leave_balance_import.idempotency_conflict';
        END IF;
        RETURN QUERY SELECT p_employee_id,v_receipt.result_updated_at,v_receipt.changed,true;
        RETURN;
    END IF;
    SELECT e.leave_accrued,e.leave_used,e.leave_remaining,e.updated_at
      INTO v_old_accrued,v_old_used,v_old_remaining,v_old_updated
      FROM public.employees e WHERE e.org_id=p_org_id AND e.id=p_employee_id FOR UPDATE;
    IF NOT FOUND THEN
        RAISE EXCEPTION USING ERRCODE='P0001', MESSAGE='leave_balance_import.employee_not_found';
    END IF;
    IF v_old_updated IS DISTINCT FROM p_expected_updated_at THEN
        RAISE EXCEPTION USING ERRCODE='40001', MESSAGE='leave_balance_import.concurrent_modification';
    END IF;
    v_changed := v_old_accrued IS DISTINCT FROM v_new_accrued
        OR v_old_used IS DISTINCT FROM v_new_used
        OR v_old_remaining IS DISTINCT FROM v_new_remaining;
    IF v_changed THEN
        UPDATE public.employees e
           SET leave_accrued=v_new_accrued,leave_used=v_new_used,
               leave_remaining=v_new_remaining,updated_at=pg_catalog.clock_timestamp()
         WHERE e.org_id=p_org_id AND e.id=p_employee_id
         RETURNING e.updated_at INTO v_new_updated;
        INSERT INTO public.audit_events
            (actor,action,target_type,target_id,branch_id,before_snap,after_snap,
             trace_id,span_id,occurred_at,org_id)
        VALUES
            (p_actor,'employee.leave_balance_import','employee',p_employee_id::TEXT,NULL,
             pg_catalog.jsonb_build_object(
                 'leave_accrued',v_old_accrued::TEXT,'leave_used',v_old_used::TEXT,
                 'leave_remaining',v_old_remaining::TEXT,'updated_at',v_old_updated),
             v_payload || pg_catalog.jsonb_build_object('updated_at',v_new_updated,
                 'idempotency_key',p_idempotency_key),
             p_trace_id,p_span_id,pg_catalog.statement_timestamp(),p_org_id);
    ELSE
        v_new_updated := v_old_updated;
    END IF;
    INSERT INTO public.leave_balance_import_receipts
        (org_id,employee_id,source_kind,source_ref,idempotency_key,payload_digest,
         result_updated_at,changed,actor,trace_id,span_id)
    VALUES
        (p_org_id,p_employee_id,p_source_kind,pg_catalog.btrim(p_source_ref),p_idempotency_key,
         v_digest,v_new_updated,v_changed,p_actor,p_trace_id,p_span_id);
    RETURN QUERY SELECT p_employee_id,v_new_updated,v_changed,false;
END;
$$;
ALTER FUNCTION leave_api.import_employee_leave_balance(
    UUID, UUID, TIMESTAMPTZ, TEXT, TEXT, TEXT, TEXT, TEXT, TEXT, UUID, TEXT, TEXT
) OWNER TO console_leave_definer;

-- Apply one complete employee import as one database transaction. The command
-- owns ordinary roster fields and delegates each protected balance snapshot to
-- the same receipt/audit command used by single-row imports. A staged run is
-- locked until roster rows, balances, receipts, per-change audits, APPLIED
-- state, and the run-level audit have all succeeded; any exception rolls the
-- entire statement back.
CREATE FUNCTION leave_api.apply_employee_import_batch(
    p_org_id UUID, p_run_id UUID, p_source_ref TEXT, p_rows JSONB,
    p_actor UUID, p_apply_audit JSONB, p_trace_id TEXT, p_span_id TEXT
) RETURNS TABLE(report JSONB, replayed BOOLEAN)
LANGUAGE plpgsql SECURITY DEFINER
SET search_path = pg_catalog SET row_security = on AS $$
DECLARE
    v_run public.data_import_runs%ROWTYPE;
    v_row JSONB;
    v_employee public.employees%ROWTYPE;
    v_employee_id UUID;
    v_employee_updated_at TIMESTAMPTZ;
    v_balance_result RECORD;
    v_source_key TEXT;
    v_company TEXT;
    v_name TEXT;
    v_idempotency_key TEXT;
    v_identity_strategy TEXT;
    v_identity_confidence TEXT;
    v_identity_review_required BOOLEAN;
    v_outcome TEXT;
    v_outcomes JSONB := '[]'::JSONB;
    v_report JSONB;
    v_input_rows INTEGER;
BEGIN
    PERFORM leave_api.assert_context(p_org_id,p_actor,p_trace_id,p_span_id);
    PERFORM leave_api.assert_employee_importer(p_org_id,p_actor);
    IF char_length(pg_catalog.btrim(p_source_ref)) NOT BETWEEN 1 AND 256
       OR jsonb_typeof(p_rows) IS DISTINCT FROM 'array' THEN
        RAISE EXCEPTION USING ERRCODE='22023', MESSAGE='employee_import_batch.invalid_payload';
    END IF;
    v_input_rows := jsonb_array_length(p_rows);

    IF p_run_id IS NOT NULL THEN
        SELECT * INTO v_run
          FROM public.data_import_runs r
         WHERE r.org_id=p_org_id AND r.id=p_run_id
         FOR UPDATE;
        IF NOT FOUND OR v_run.entity_type <> 'employee_hr' THEN
            RAISE EXCEPTION USING ERRCODE='P0001', MESSAGE='employee_import_batch.run_not_found';
        END IF;
        IF v_run.status = 'APPLIED' THEN
            SELECT v_run.apply_summary || pg_catalog.jsonb_build_object(
                'inserted',0,
                'updated',0,
                'skipped',coalesce((v_run.apply_summary->>'input_rows')::INTEGER,0),
                'companies',coalesce((
                    SELECT jsonb_agg(item || pg_catalog.jsonb_build_object(
                        'inserted',0,
                        'updated',0,
                        'skipped',coalesce((item->>'input_rows')::INTEGER,0)
                    ) ORDER BY item->>'company')
                    FROM jsonb_array_elements(
                        coalesce(v_run.apply_summary->'companies','[]'::JSONB)
                    ) item
                ),'[]'::JSONB)
            ) INTO v_report;
            RETURN QUERY SELECT v_report, true;
            RETURN;
        END IF;
        IF v_run.status <> 'DRY_RUN' THEN
            RAISE EXCEPTION USING ERRCODE='22023', MESSAGE='employee_import_batch.run_not_dry_run';
        END IF;
        IF p_source_ref <> 'run:' || p_run_id::TEXT
           OR v_input_rows <> v_run.candidate_rows
           OR (SELECT count(DISTINCT item->>'source_key')
                 FROM jsonb_array_elements(p_rows) item) <> v_run.candidate_rows
           OR EXISTS (
                SELECT 1
                  FROM jsonb_array_elements(p_rows) item
                 WHERE NOT EXISTS (
                    SELECT 1 FROM public.data_import_rows ir
                     WHERE ir.org_id=p_org_id AND ir.run_id=p_run_id
                       AND ir.row_status='CANDIDATE'
                       AND ir.source_key=item->>'source_key'
                 )
           )
           OR EXISTS (
                SELECT 1
                  FROM jsonb_array_elements(p_rows) item
                  JOIN public.data_import_rows ir
                    ON ir.org_id=p_org_id
                   AND ir.run_id=p_run_id
                   AND ir.row_status='CANDIDATE'
                   AND ir.source_key=item->>'source_key'
                 CROSS JOIN LATERAL (
                    SELECT CASE
                        WHEN ir.canonical_row#>>'{source_metadata,identity_resolution,strategy}'
                             IN ('employee_number','legal_identifier_hash',
                                 'birth_hire_fingerprint','source_row_fingerprint')
                        THEN ir.canonical_row#>>'{source_metadata,identity_resolution,strategy}'
                        ELSE 'source_row_fingerprint'
                    END AS strategy
                 ) identity
                 WHERE item IS DISTINCT FROM (
                    ir.canonical_row || pg_catalog.jsonb_build_object(
                        'raw_row',ir.raw_row,
                        'identity',pg_catalog.jsonb_build_object(
                            'strategy',identity.strategy,
                            'confidence',CASE identity.strategy
                                WHEN 'employee_number' THEN 'high'
                                WHEN 'legal_identifier_hash' THEN 'high'
                                WHEN 'birth_hire_fingerprint' THEN 'medium'
                                ELSE 'low'
                            END,
                            'review_required',NOT (
                                coalesce((ir.canonical_row#>>
                                    '{source_metadata,identity_resolution,manual_review_required}')
                                    ::BOOLEAN,true) = false
                                AND identity.strategy IN (
                                    'employee_number','legal_identifier_hash'
                                )
                            )
                        )
                    )
                 )
           ) THEN
            RAISE EXCEPTION USING ERRCODE='22023', MESSAGE='employee_import_batch.run_payload_mismatch';
        END IF;
    END IF;

    FOR v_row IN SELECT value FROM jsonb_array_elements(p_rows)
    LOOP
        v_company := pg_catalog.btrim(v_row->>'company');
        v_name := pg_catalog.btrim(v_row->>'name');
        v_source_key := pg_catalog.btrim(v_row->>'source_key');
        IF coalesce(v_company,'') = '' OR coalesce(v_name,'') = ''
           OR coalesce(v_source_key,'') = ''
           OR coalesce(pg_catalog.btrim(v_row->>'source_filename'),'') = ''
           OR coalesce(pg_catalog.btrim(v_row->>'source_sheet'),'') = ''
           OR coalesce((v_row->>'source_row')::INTEGER,0) <= 0
           OR jsonb_typeof(v_row->'raw_row') IS DISTINCT FROM 'object'
           OR jsonb_typeof(v_row->'source_metadata') IS DISTINCT FROM 'object'
           OR jsonb_typeof(v_row->'canonical') IS DISTINCT FROM 'object' THEN
            RAISE EXCEPTION USING ERRCODE='22023', MESSAGE='employee_import_batch.invalid_row';
        END IF;

        v_idempotency_key := 'employee-import:' || pg_catalog.encode(
            public.digest(p_source_ref || ':' || v_source_key,'sha256'),'hex'
        );
        SELECT * INTO v_employee
          FROM public.employees e
         WHERE e.org_id=p_org_id AND e.source_key=v_source_key
         FOR UPDATE;

        -- A prior receipt means this exact source row already committed. Call
        -- the protected command to verify its payload binding, but deliberately
        -- do not rewrite ordinary roster fields or updated_at on replay.
        IF FOUND AND EXISTS (
            SELECT 1 FROM public.leave_balance_import_receipts r
             WHERE r.org_id=p_org_id AND r.idempotency_key=v_idempotency_key
        ) THEN
            SELECT * INTO v_balance_result
              FROM leave_api.import_employee_leave_balance(
                p_org_id,v_employee.id,v_employee.updated_at,
                v_row#>>'{canonical,leave_accrued}',
                v_row#>>'{canonical,leave_used}',
                v_row#>>'{canonical,leave_remaining}',
                'employee_import',p_source_ref,v_idempotency_key,
                p_actor,p_trace_id,p_span_id
              );
            v_outcome := 'skipped';
        ELSE
            v_identity_strategy := coalesce(
                v_row#>>'{identity,strategy}','source_row_fingerprint'
            );
            v_identity_confidence := coalesce(v_row#>>'{identity,confidence}','low');
            v_identity_review_required := coalesce(
                (v_row#>>'{identity,review_required}')::BOOLEAN,true
            );
            INSERT INTO public.employees (
                org_id,company,name,source_filename,source_sheet,source_row,
                source_key,raw_row,source_metadata,employee_number,org_unit,job,
                position,worksite_name,worksite_address,hire_date,exit_date,
                employment_status,identity_resolution_strategy,
                identity_resolution_confidence,identity_review_required,
                identity_name_only_merge
            ) VALUES (
                p_org_id,v_company,v_name,v_row->>'source_filename',
                v_row->>'source_sheet',(v_row->>'source_row')::INTEGER,
                v_source_key,v_row->'raw_row',v_row->'source_metadata',
                v_row#>>'{canonical,employee_number}',
                v_row#>>'{canonical,org_unit}',v_row#>>'{canonical,job}',
                v_row#>>'{canonical,position}',v_row#>>'{canonical,worksite_name}',
                v_row#>>'{canonical,worksite_address}',v_row#>>'{canonical,hire_date}',
                v_row#>>'{canonical,exit_date}',
                coalesce(v_row#>>'{canonical,employment_status}','ACTIVE'),
                v_identity_strategy,v_identity_confidence,
                v_identity_review_required,false
            )
            ON CONFLICT (org_id,source_key) DO UPDATE SET
                company=EXCLUDED.company,name=EXCLUDED.name,
                source_filename=EXCLUDED.source_filename,
                source_sheet=EXCLUDED.source_sheet,source_row=EXCLUDED.source_row,
                raw_row=EXCLUDED.raw_row,source_metadata=EXCLUDED.source_metadata,
                employee_number=EXCLUDED.employee_number,org_unit=EXCLUDED.org_unit,
                job=EXCLUDED.job,position=EXCLUDED.position,
                worksite_name=EXCLUDED.worksite_name,
                worksite_address=EXCLUDED.worksite_address,
                hire_date=EXCLUDED.hire_date,exit_date=EXCLUDED.exit_date,
                employment_status=EXCLUDED.employment_status,
                identity_resolution_strategy=EXCLUDED.identity_resolution_strategy,
                identity_resolution_confidence=EXCLUDED.identity_resolution_confidence,
                identity_review_required=EXCLUDED.identity_review_required,
                identity_name_only_merge=false,updated_at=pg_catalog.clock_timestamp()
            RETURNING id,updated_at,
                CASE WHEN xmax=0 THEN 'inserted' ELSE 'updated' END
              INTO v_employee_id,v_employee_updated_at,v_outcome;

            SELECT * INTO v_balance_result
              FROM leave_api.import_employee_leave_balance(
                p_org_id,v_employee_id,v_employee_updated_at,
                v_row#>>'{canonical,leave_accrued}',
                v_row#>>'{canonical,leave_used}',
                v_row#>>'{canonical,leave_remaining}',
                'employee_import',p_source_ref,v_idempotency_key,
                p_actor,p_trace_id,p_span_id
              );
        END IF;
        v_outcomes := v_outcomes || pg_catalog.jsonb_build_array(
            pg_catalog.jsonb_build_object('company',v_company,'outcome',v_outcome)
        );
    END LOOP;

    SELECT pg_catalog.jsonb_build_object(
        'input_rows',v_input_rows,
        'inserted',(SELECT count(*) FROM jsonb_array_elements(v_outcomes) item
                    WHERE item->>'outcome'='inserted'),
        'updated',(SELECT count(*) FROM jsonb_array_elements(v_outcomes) item
                   WHERE item->>'outcome'='updated'),
        'skipped',(SELECT count(*) FROM jsonb_array_elements(v_outcomes) item
                   WHERE item->>'outcome'='skipped'),
        'companies',coalesce((
            SELECT jsonb_agg(pg_catalog.jsonb_build_object(
                'company',company,'input_rows',input_rows,
                'inserted',inserted,'updated',updated,'skipped',skipped
            ) ORDER BY company)
            FROM (
                SELECT item->>'company' AS company,count(*) AS input_rows,
                    count(*) FILTER (WHERE item->>'outcome'='inserted') AS inserted,
                    count(*) FILTER (WHERE item->>'outcome'='updated') AS updated,
                    count(*) FILTER (WHERE item->>'outcome'='skipped') AS skipped
                FROM jsonb_array_elements(v_outcomes) item
                GROUP BY item->>'company'
            ) companies
        ),'[]'::JSONB)
    ) INTO v_report;

    IF p_run_id IS NOT NULL THEN
        UPDATE public.data_import_runs r
           SET status='APPLIED',apply_summary=v_report,applied_by=p_actor,
               applied_at=pg_catalog.clock_timestamp(),updated_at=pg_catalog.clock_timestamp()
         WHERE r.org_id=p_org_id AND r.id=p_run_id;
    END IF;
    -- A successful first application always has intrinsic command-owned
    -- evidence, including legacy direct imports and roster-only/null-balance
    -- rows. Pure receipt/APPLIED replay is side-effect-free and reuses the
    -- original evidence rather than appending a misleading duplicate.
    IF p_run_id IS NOT NULL OR EXISTS (
        SELECT 1 FROM jsonb_array_elements(v_outcomes) item
        WHERE item->>'outcome' <> 'skipped'
    ) THEN
        INSERT INTO public.audit_events
            (actor,action,target_type,target_id,before_snap,after_snap,
             trace_id,span_id,occurred_at,org_id)
        VALUES (
            p_actor,'data_import.apply',
            CASE WHEN p_run_id IS NULL THEN 'employee_import_batch' ELSE 'data_import_run' END,
            coalesce(p_run_id::TEXT,pg_catalog.btrim(p_source_ref)),NULL,
            coalesce(p_apply_audit,'{}'::JSONB) || pg_catalog.jsonb_build_object(
                'run_id',p_run_id,'entity_type','employee_hr',
                'source_ref',pg_catalog.btrim(p_source_ref),'report',v_report
            ),p_trace_id,p_span_id,pg_catalog.statement_timestamp(),p_org_id
        );
    END IF;
    RETURN QUERY SELECT v_report,false;
END;
$$;
ALTER FUNCTION leave_api.apply_employee_import_batch(
    UUID, UUID, TEXT, JSONB, UUID, JSONB, TEXT, TEXT
) OWNER TO console_leave_definer;

CREATE FUNCTION leave_api.set_employee_home_branch(
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
    IF v_old_branch IS NULL OR v_old_branch = p_home_branch_id THEN
        PERFORM pg_catalog.pg_advisory_xact_lock(pg_catalog.hashtextextended(p_home_branch_id::TEXT, 166));
    ELSIF v_old_branch::TEXT < p_home_branch_id::TEXT THEN
        PERFORM pg_catalog.pg_advisory_xact_lock(pg_catalog.hashtextextended(v_old_branch::TEXT, 166));
        PERFORM pg_catalog.pg_advisory_xact_lock(pg_catalog.hashtextextended(p_home_branch_id::TEXT, 166));
    ELSE
        PERFORM pg_catalog.pg_advisory_xact_lock(pg_catalog.hashtextextended(p_home_branch_id::TEXT, 166));
        PERFORM pg_catalog.pg_advisory_xact_lock(pg_catalog.hashtextextended(v_old_branch::TEXT, 166));
    END IF;
    IF NOT EXISTS (SELECT 1 FROM public.branches b WHERE b.org_id=p_org_id
                   AND b.id=p_home_branch_id AND b.deactivated_at IS NULL) THEN
        RAISE EXCEPTION USING ERRCODE='P0001', MESSAGE='leave_home_branch.active_branch_required';
    END IF;
    IF v_old_branch IS NULL THEN
        -- An unassigned employee has no branch-scoped owner yet. Only the
        -- organization-wide SUPER_ADMIN capability may establish that first
        -- routing authority; a target-branch admin cannot claim the employee.
        PERFORM leave_api.assert_org_admin(p_org_id,p_actor);
    ELSE
        PERFORM leave_api.assert_manager(p_org_id,p_actor,v_old_branch);
        PERFORM leave_api.assert_manager(p_org_id,p_actor,p_home_branch_id);
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
ALTER FUNCTION leave_api.set_employee_home_branch(UUID, UUID, UUID, TIMESTAMPTZ, UUID, TEXT, TEXT)
    OWNER TO console_leave_definer;

REVOKE ALL ON ALL FUNCTIONS IN SCHEMA leave_api FROM PUBLIC, console_rt;
GRANT EXECUTE ON FUNCTION leave_api.create_request(UUID, UUID, UUID, TEXT, DATE, DATE, TEXT, TEXT, TEXT[], UUID, JSONB, JSONB, JSONB, JSONB, UUID, TEXT, TEXT) TO console_leave_cmd;
GRANT EXECUTE ON FUNCTION leave_api.resolve_charge(UUID, UUID, UUID, BIGINT, JSONB, JSONB, JSONB, JSONB, TEXT, TEXT) TO console_leave_cmd;
GRANT EXECUTE ON FUNCTION leave_api.decide_request(UUID, UUID, UUID, BIGINT, TEXT, TEXT, TEXT, TEXT) TO console_leave_cmd;
GRANT EXECUTE ON FUNCTION leave_api.import_employee_leave_balance(UUID, UUID, TIMESTAMPTZ, TEXT, TEXT, TEXT, TEXT, TEXT, TEXT, UUID, TEXT, TEXT) TO console_leave_cmd;
GRANT EXECUTE ON FUNCTION leave_api.apply_employee_import_batch(UUID, UUID, TEXT, JSONB, UUID, JSONB, TEXT, TEXT) TO console_leave_cmd;
GRANT EXECUTE ON FUNCTION leave_api.set_employee_home_branch(UUID, UUID, UUID, TIMESTAMPTZ, UUID, TEXT, TEXT) TO console_leave_cmd;
