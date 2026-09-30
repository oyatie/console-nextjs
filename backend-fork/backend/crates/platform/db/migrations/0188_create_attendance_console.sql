-- Canonical Attendance console persistence.  This migration owns evidence and
-- replay-safe writes only; it deliberately does not create or mutate period
-- locks.  A later command layer creates an org close's payroll lock and stores
-- that reference atomically.

INSERT INTO object_types (kind, description, code_prefix) VALUES
    ('attendance_exception', 'Employee attendance exception', 'AT-')
ON CONFLICT (kind) DO NOTHING;

-- console-gate: audited-table attendance_exceptions
CREATE TABLE attendance_exceptions (
    id UUID NOT NULL DEFAULT gen_random_uuid(),
    org_id UUID NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    code TEXT NOT NULL CHECK (char_length(btrim(code)) BETWEEN 1 AND 64),
    kind TEXT NOT NULL CHECK (kind IN ('LATE', 'NO_SHOW', 'UNAPPROVED_OVERTIME', 'EARLY_LEAVE')),
    status TEXT NOT NULL DEFAULT 'OPEN' CHECK (status IN ('OPEN', 'RESOLVED')),
    employee_id UUID NOT NULL,
    branch_id UUID NULL,
    work_date DATE NOT NULL,
    occurred_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    detail TEXT NOT NULL CHECK (btrim(detail) <> ''),
    evidence JSONB NOT NULL DEFAULT '[]'::jsonb CHECK (jsonb_typeof(evidence) = 'array'),
    links JSONB NOT NULL DEFAULT '[]'::jsonb CHECK (jsonb_typeof(links) = 'array'),
    idempotency_key TEXT NOT NULL CHECK (char_length(btrim(idempotency_key)) BETWEEN 16 AND 200),
    request_fingerprint TEXT NOT NULL CHECK (request_fingerprint ~ '^[a-f0-9]{64}$'),
    created_by UUID NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (id, org_id),
    UNIQUE (org_id, code),
    UNIQUE (org_id, idempotency_key),
    FOREIGN KEY (employee_id, org_id) REFERENCES employees(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (branch_id, org_id) REFERENCES branches(id, org_id) ON DELETE SET NULL,
    FOREIGN KEY (created_by, org_id) REFERENCES users(id, org_id) ON DELETE RESTRICT
);
CREATE INDEX attendance_exceptions_day_idx ON attendance_exceptions (org_id, work_date, status);
CREATE INDEX attendance_exceptions_emp_idx ON attendance_exceptions (org_id, employee_id, work_date DESC);

-- console-gate: audited-table attendance_exception_resolutions
CREATE TABLE attendance_exception_resolutions (
    id UUID NOT NULL DEFAULT gen_random_uuid(),
    org_id UUID NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    exception_id UUID NOT NULL,
    action TEXT NOT NULL CHECK (action IN ('CONFIRM', 'APPROVE_OVERTIME')),
    reason TEXT NOT NULL CHECK (btrim(reason) <> ''),
    linked_work_ref TEXT NULL CHECK (linked_work_ref IS NULL OR btrim(linked_work_ref) <> ''),
    ot_hours NUMERIC(5,2) NULL CHECK (ot_hours IS NULL OR ot_hours > 0),
    actor_user_id UUID NOT NULL,
    resolved_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (id, org_id),
    UNIQUE (org_id, exception_id),
    FOREIGN KEY (exception_id, org_id) REFERENCES attendance_exceptions(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (actor_user_id, org_id) REFERENCES users(id, org_id) ON DELETE RESTRICT,
    CHECK (action <> 'APPROVE_OVERTIME' OR linked_work_ref IS NOT NULL)
);

-- console-gate: audited-table attendance_substitutions
CREATE TABLE attendance_substitutions (
    id UUID NOT NULL DEFAULT gen_random_uuid(),
    org_id UUID NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    site TEXT NOT NULL CHECK (btrim(site) <> ''),
    branch_id UUID NULL,
    role TEXT NOT NULL CHECK (btrim(role) <> ''),
    cover_date DATE NOT NULL,
    from_minutes INTEGER NOT NULL CHECK (from_minutes BETWEEN 0 AND 1439),
    to_minutes INTEGER NOT NULL CHECK (to_minutes BETWEEN 1 AND 1440 AND to_minutes > from_minutes),
    covered_employee_id UUID NOT NULL,
    reason_kind TEXT NOT NULL CHECK (reason_kind IN ('NO_SHOW', 'APPROVED_LEAVE', 'HALF_DAY', 'LONG_TERM', 'OTHER')),
    reason_detail TEXT NULL CHECK (reason_detail IS NULL OR btrim(reason_detail) <> ''),
    worker_employee_id UUID NULL,
    worker_name TEXT NOT NULL CHECK (btrim(worker_name) <> ''),
    worker_type TEXT NOT NULL CHECK (btrim(worker_type) <> ''),
    worker_rate TEXT NULL CHECK (worker_rate IS NULL OR btrim(worker_rate) <> ''),
    status TEXT NOT NULL DEFAULT 'ASSIGNED' CHECK (status IN ('ASSIGNED', 'CANCELLED')),
    cancel_reason TEXT NULL,
    approval_ref TEXT NULL CHECK (approval_ref IS NULL OR btrim(approval_ref) <> ''),
    contract_ref TEXT NULL CHECK (contract_ref IS NULL OR btrim(contract_ref) <> ''),
    exception_id UUID NULL,
    idempotency_key TEXT NOT NULL CHECK (char_length(btrim(idempotency_key)) BETWEEN 16 AND 200),
    request_fingerprint TEXT NOT NULL CHECK (request_fingerprint ~ '^[a-f0-9]{64}$'),
    created_by UUID NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (id, org_id),
    UNIQUE (org_id, idempotency_key),
    FOREIGN KEY (branch_id, org_id) REFERENCES branches(id, org_id) ON DELETE SET NULL,
    FOREIGN KEY (covered_employee_id, org_id) REFERENCES employees(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (worker_employee_id, org_id) REFERENCES employees(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (exception_id, org_id) REFERENCES attendance_exceptions(id, org_id) ON DELETE SET NULL,
    FOREIGN KEY (created_by, org_id) REFERENCES users(id, org_id) ON DELETE RESTRICT,
    CHECK (status <> 'CANCELLED' OR btrim(coalesce(cancel_reason, '')) <> '')
);
CREATE INDEX attendance_substitutions_date_idx ON attendance_substitutions (org_id, cover_date, status);
CREATE INDEX attendance_substitutions_cov_idx ON attendance_substitutions (org_id, covered_employee_id, cover_date);

-- console-gate: audited-table attendance_month_closes
CREATE TABLE attendance_month_closes (
    id UUID NOT NULL DEFAULT gen_random_uuid(),
    org_id UUID NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    month DATE NOT NULL CHECK (date_trunc('month', month)::date = month),
    branch_id UUID NULL,
    checks JSONB NOT NULL CHECK (jsonb_typeof(checks) = 'object'),
    attested_by UUID NOT NULL,
    attested_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    period_lock_id UUID NULL,
    closed_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (id, org_id),
    UNIQUE NULLS NOT DISTINCT (org_id, month, branch_id),
    FOREIGN KEY (branch_id, org_id) REFERENCES branches(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (attested_by, org_id) REFERENCES users(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (period_lock_id, org_id) REFERENCES period_locks(id, org_id) ON DELETE RESTRICT,
    CHECK ((branch_id IS NULL AND period_lock_id IS NOT NULL) OR (branch_id IS NOT NULL AND period_lock_id IS NULL))
);
CREATE INDEX attendance_month_closes_month_idx ON attendance_month_closes (org_id, month DESC, branch_id);

-- console-gate: audited-table attendance_close_amendments
CREATE TABLE attendance_close_amendments (
    id UUID NOT NULL DEFAULT gen_random_uuid(),
    org_id UUID NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    close_id UUID NOT NULL,
    reason TEXT NOT NULL CHECK (btrim(reason) <> ''),
    detail TEXT NOT NULL CHECK (btrim(detail) <> ''),
    ref TEXT NULL CHECK (ref IS NULL OR btrim(ref) <> ''),
    idempotency_key TEXT NOT NULL CHECK (char_length(btrim(idempotency_key)) BETWEEN 16 AND 200),
    request_fingerprint TEXT NOT NULL CHECK (request_fingerprint ~ '^[a-f0-9]{64}$'),
    actor_user_id UUID NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (id, org_id),
    UNIQUE (org_id, idempotency_key),
    FOREIGN KEY (close_id, org_id) REFERENCES attendance_month_closes(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (actor_user_id, org_id) REFERENCES users(id, org_id) ON DELETE RESTRICT
);
CREATE INDEX attendance_close_amendments_close_idx ON attendance_close_amendments (org_id, close_id, created_at DESC);

-- console-gate: audited-table attendance_week52_acknowledgements
CREATE TABLE attendance_week52_acknowledgements (
    id UUID NOT NULL DEFAULT gen_random_uuid(),
    org_id UUID NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    employee_id UUID NOT NULL,
    week_start DATE NOT NULL CHECK (extract(isodow FROM week_start) = 1),
    acknowledged_by_user_id UUID NOT NULL,
    acknowledged_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (id, org_id),
    UNIQUE (org_id, employee_id, week_start),
    FOREIGN KEY (employee_id, org_id) REFERENCES employees(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (acknowledged_by_user_id, org_id) REFERENCES users(id, org_id) ON DELETE RESTRICT
);

DO $$
DECLARE
    table_name TEXT;
BEGIN
    FOREACH table_name IN ARRAY ARRAY[
        'attendance_exceptions', 'attendance_exception_resolutions',
        'attendance_substitutions', 'attendance_month_closes',
        'attendance_close_amendments', 'attendance_week52_acknowledgements'
    ] LOOP
        EXECUTE format('ALTER TABLE %I ENABLE ROW LEVEL SECURITY', table_name);
        EXECUTE format('ALTER TABLE %I FORCE ROW LEVEL SECURITY', table_name);
        EXECUTE format(
            'CREATE POLICY org_isolation ON %I USING (org_id = NULLIF(current_setting(''app.current_org'', true), '''')::uuid) WITH CHECK (org_id = NULLIF(current_setting(''app.current_org'', true), '''')::uuid)',
            table_name
        );
        EXECUTE format('CREATE TRIGGER trg_%I_org_immutable BEFORE UPDATE ON %I FOR EACH ROW EXECUTE FUNCTION enforce_org_id_immutable()', table_name, table_name);
    END LOOP;
END $$;

CREATE OR REPLACE FUNCTION attendance_exception_resolution_only()
RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF OLD.status <> 'OPEN' OR NEW.status <> 'RESOLVED'
       OR NEW.id IS DISTINCT FROM OLD.id OR NEW.org_id IS DISTINCT FROM OLD.org_id
       OR NEW.code IS DISTINCT FROM OLD.code OR NEW.kind IS DISTINCT FROM OLD.kind
       OR NEW.employee_id IS DISTINCT FROM OLD.employee_id OR NEW.branch_id IS DISTINCT FROM OLD.branch_id
       OR NEW.work_date IS DISTINCT FROM OLD.work_date OR NEW.occurred_at IS DISTINCT FROM OLD.occurred_at
       OR NEW.detail IS DISTINCT FROM OLD.detail OR NEW.evidence IS DISTINCT FROM OLD.evidence
       OR NEW.links IS DISTINCT FROM OLD.links OR NEW.idempotency_key IS DISTINCT FROM OLD.idempotency_key
       OR NEW.request_fingerprint IS DISTINCT FROM OLD.request_fingerprint
       OR NEW.created_by IS DISTINCT FROM OLD.created_by OR NEW.created_at IS DISTINCT FROM OLD.created_at
    THEN
        RAISE EXCEPTION 'attendance exception only permits OPEN to RESOLVED transition' USING ERRCODE = '25006';
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER trg_attendance_exceptions_resolution_only
    BEFORE UPDATE ON attendance_exceptions FOR EACH ROW EXECUTE FUNCTION attendance_exception_resolution_only();

CREATE OR REPLACE FUNCTION attendance_substitution_transition_only()
RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF OLD.status <> 'ASSIGNED' OR NEW.status <> 'CANCELLED'
       OR NEW.id IS DISTINCT FROM OLD.id OR NEW.org_id IS DISTINCT FROM OLD.org_id
       OR NEW.site IS DISTINCT FROM OLD.site OR NEW.branch_id IS DISTINCT FROM OLD.branch_id
       OR NEW.role IS DISTINCT FROM OLD.role OR NEW.cover_date IS DISTINCT FROM OLD.cover_date
       OR NEW.from_minutes IS DISTINCT FROM OLD.from_minutes OR NEW.to_minutes IS DISTINCT FROM OLD.to_minutes
       OR NEW.covered_employee_id IS DISTINCT FROM OLD.covered_employee_id
       OR NEW.reason_kind IS DISTINCT FROM OLD.reason_kind OR NEW.reason_detail IS DISTINCT FROM OLD.reason_detail
       OR NEW.worker_employee_id IS DISTINCT FROM OLD.worker_employee_id OR NEW.worker_name IS DISTINCT FROM OLD.worker_name
       OR NEW.worker_type IS DISTINCT FROM OLD.worker_type OR NEW.worker_rate IS DISTINCT FROM OLD.worker_rate
       OR NEW.approval_ref IS DISTINCT FROM OLD.approval_ref OR NEW.contract_ref IS DISTINCT FROM OLD.contract_ref
       OR NEW.exception_id IS DISTINCT FROM OLD.exception_id OR NEW.idempotency_key IS DISTINCT FROM OLD.idempotency_key
       OR NEW.request_fingerprint IS DISTINCT FROM OLD.request_fingerprint
       OR NEW.created_by IS DISTINCT FROM OLD.created_by OR NEW.created_at IS DISTINCT FROM OLD.created_at
    THEN
        RAISE EXCEPTION 'attendance substitution only permits ASSIGNED to CANCELLED transition' USING ERRCODE = '25006';
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER trg_attendance_substitutions_transition_only
    BEFORE UPDATE ON attendance_substitutions FOR EACH ROW EXECUTE FUNCTION attendance_substitution_transition_only();

-- A resolution and its terminal exception state are one atomic fact.  Either
-- write can arrive first inside the command transaction, so enforce the pair
-- at COMMIT rather than rejecting the intermediate row order.
CREATE OR REPLACE FUNCTION attendance_exception_resolution_consistent()
RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE
    checked_org_id UUID;
    checked_exception_id UUID;
    exception_status TEXT;
    resolution_count INTEGER;
BEGIN
    IF TG_OP = 'DELETE' THEN
        checked_org_id := OLD.org_id;
        IF TG_TABLE_NAME = 'attendance_exceptions' THEN
            checked_exception_id := OLD.id;
        ELSE
            checked_exception_id := OLD.exception_id;
        END IF;
    ELSE
        checked_org_id := NEW.org_id;
        IF TG_TABLE_NAME = 'attendance_exceptions' THEN
            checked_exception_id := NEW.id;
        ELSE
            checked_exception_id := NEW.exception_id;
        END IF;
    END IF;

    SELECT status INTO exception_status
      FROM attendance_exceptions
     WHERE org_id = checked_org_id AND id = checked_exception_id;
    IF NOT FOUND THEN
        RETURN NULL;
    END IF;
    SELECT count(*) INTO resolution_count
      FROM attendance_exception_resolutions
     WHERE org_id = checked_org_id AND exception_id = checked_exception_id;
    IF (exception_status = 'RESOLVED') <> (resolution_count = 1) THEN
        RAISE EXCEPTION 'attendance exception % status/resolution mismatch', checked_exception_id
            USING ERRCODE = '23514';
    END IF;
    RETURN NULL;
END;
$$;
CREATE CONSTRAINT TRIGGER trg_attendance_exception_resolution_consistent
    AFTER INSERT OR UPDATE OR DELETE ON attendance_exceptions
    DEFERRABLE INITIALLY DEFERRED FOR EACH ROW
    EXECUTE FUNCTION attendance_exception_resolution_consistent();
CREATE CONSTRAINT TRIGGER trg_attendance_resolution_exception_consistent
    AFTER INSERT OR UPDATE OR DELETE ON attendance_exception_resolutions
    DEFERRABLE INITIALLY DEFERRED FOR EACH ROW
    EXECUTE FUNCTION attendance_exception_resolution_consistent();

-- A close is evidence of one currently active payroll lock for exactly its
-- organization-month.  Branch closes intentionally have no lock; an org close
-- is rejected if a referenced lock is later unlocked or rewritten in the same
-- transaction.
CREATE OR REPLACE FUNCTION attendance_month_close_lock_consistent()
RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE
    checked_close attendance_month_closes%ROWTYPE;
    lock_org_id UUID;
    lock_domain TEXT;
    lock_start DATE;
    lock_end DATE;
    lock_unlocked_at TIMESTAMPTZ;
    changed_lock_id UUID;
    changed_lock_org_id UUID;
BEGIN
    IF TG_TABLE_NAME = 'attendance_month_closes' THEN
        IF TG_OP = 'DELETE' THEN
            RETURN NULL;
        END IF;
        SELECT * INTO checked_close
          FROM attendance_month_closes
         WHERE org_id = NEW.org_id AND id = NEW.id;
        IF NOT FOUND THEN
            RETURN NULL;
        END IF;
        IF checked_close.branch_id IS NOT NULL THEN
            RETURN NULL;
        END IF;
        SELECT org_id, domain, period_start, period_end, unlocked_at
          INTO lock_org_id, lock_domain, lock_start, lock_end, lock_unlocked_at
          FROM period_locks
         WHERE id = checked_close.period_lock_id AND org_id = checked_close.org_id;
        IF NOT FOUND
           OR lock_domain <> 'payroll'
           OR lock_start <> checked_close.month
           OR lock_end <> (checked_close.month + INTERVAL '1 month - 1 day')::date
           OR lock_unlocked_at IS NOT NULL
        THEN
            RAISE EXCEPTION 'attendance org close requires an active same-org payroll lock for its exact month'
                USING ERRCODE = '23514';
        END IF;
        RETURN NULL;
    END IF;

    IF TG_OP = 'DELETE' THEN
        changed_lock_id := OLD.id;
        changed_lock_org_id := OLD.org_id;
    ELSE
        changed_lock_id := NEW.id;
        changed_lock_org_id := NEW.org_id;
    END IF;
    FOR checked_close IN
        SELECT * FROM attendance_month_closes
         WHERE period_lock_id = changed_lock_id
           AND org_id = changed_lock_org_id
    LOOP
        SELECT org_id, domain, period_start, period_end, unlocked_at
          INTO lock_org_id, lock_domain, lock_start, lock_end, lock_unlocked_at
          FROM period_locks
         WHERE id = checked_close.period_lock_id AND org_id = checked_close.org_id;
        IF NOT FOUND
           OR lock_domain <> 'payroll'
           OR lock_start <> checked_close.month
           OR lock_end <> (checked_close.month + INTERVAL '1 month - 1 day')::date
           OR lock_unlocked_at IS NOT NULL
        THEN
            RAISE EXCEPTION 'attendance org close requires an active same-org payroll lock for its exact month'
                USING ERRCODE = '23514';
        END IF;
    END LOOP;
    RETURN NULL;
END;
$$;
CREATE CONSTRAINT TRIGGER trg_attendance_month_close_lock_consistent
    AFTER INSERT OR UPDATE OR DELETE ON attendance_month_closes
    DEFERRABLE INITIALLY DEFERRED FOR EACH ROW
    EXECUTE FUNCTION attendance_month_close_lock_consistent();
CREATE CONSTRAINT TRIGGER trg_period_lock_attendance_close_consistent
    AFTER INSERT OR UPDATE OR DELETE ON period_locks
    DEFERRABLE INITIALLY DEFERRED FOR EACH ROW
    EXECUTE FUNCTION attendance_month_close_lock_consistent();

CREATE TRIGGER trg_attendance_exception_resolutions_append_only
    BEFORE UPDATE OR DELETE ON attendance_exception_resolutions
    FOR EACH ROW EXECUTE FUNCTION platform_append_only_immutable();
CREATE TRIGGER trg_attendance_month_closes_append_only
    BEFORE UPDATE OR DELETE ON attendance_month_closes
    FOR EACH ROW EXECUTE FUNCTION platform_append_only_immutable();
CREATE TRIGGER trg_attendance_close_amendments_append_only
    BEFORE UPDATE OR DELETE ON attendance_close_amendments
    FOR EACH ROW EXECUTE FUNCTION platform_append_only_immutable();
CREATE TRIGGER trg_attendance_week52_acknowledgements_append_only
    BEFORE UPDATE OR DELETE ON attendance_week52_acknowledgements
    FOR EACH ROW EXECUTE FUNCTION platform_append_only_immutable();

GRANT SELECT, INSERT, UPDATE ON attendance_exceptions, attendance_substitutions TO console_rt;
GRANT SELECT, INSERT ON attendance_exception_resolutions, attendance_month_closes,
    attendance_close_amendments, attendance_week52_acknowledgements TO console_rt;
REVOKE DELETE ON attendance_exceptions, attendance_substitutions, attendance_exception_resolutions,
    attendance_month_closes, attendance_close_amendments, attendance_week52_acknowledgements FROM console_rt;
REVOKE UPDATE ON attendance_exception_resolutions, attendance_month_closes,
    attendance_close_amendments, attendance_week52_acknowledgements FROM console_rt;
