-- G008 HR/payroll readiness: regulated staging only. These tables make the live
-- COSS Group import usable for payroll preparation and annual-leave review while
-- failing closed before payable payroll calculation/issuance.

CREATE TABLE payroll_draft_runs (
    id                  UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id              UUID        NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    period_start        DATE        NOT NULL,
    period_end          DATE        NOT NULL,
    source_label        TEXT        NOT NULL CHECK (btrim(source_label) <> ''),
    status              TEXT        NOT NULL DEFAULT 'BLOCKED_LEGAL_GATE'
        CHECK (status IN ('STAGED','BLOCKED_LEGAL_GATE','READY_FOR_REVIEW','APPROVED','ISSUED','VOID')),
    calculation_enabled BOOLEAN     NOT NULL DEFAULT FALSE,
    legal_basis         JSONB       NOT NULL DEFAULT '{}'::jsonb,
    source_summary      JSONB       NOT NULL DEFAULT '{}'::jsonb,
    created_by          UUID        NULL REFERENCES users(id) ON DELETE SET NULL,
    approved_by         UUID        NULL REFERENCES users(id) ON DELETE RESTRICT,
    approved_at         TIMESTAMPTZ NULL,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (id, org_id),
    UNIQUE (org_id, period_start, period_end, source_label),
    CONSTRAINT payroll_draft_runs_valid_period CHECK (period_end >= period_start),
    CONSTRAINT payroll_draft_runs_enabled_requires_review
        CHECK (calculation_enabled = FALSE OR status IN ('READY_FOR_REVIEW','APPROVED','ISSUED'))
);

CREATE INDEX payroll_draft_runs_org_period_idx
    ON payroll_draft_runs (org_id, period_start DESC, period_end DESC, status);

CREATE TABLE payroll_draft_lines (
    id                         UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id                     UUID        NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    run_id                     UUID        NOT NULL,
    employee_id                UUID        NULL REFERENCES employees(id) ON DELETE RESTRICT,
    employee_source_key        TEXT        NOT NULL CHECK (btrim(employee_source_key) <> ''),
    employee_display_name      TEXT        NOT NULL CHECK (btrim(employee_display_name) <> ''),
    employee_company           TEXT        NOT NULL CHECK (btrim(employee_company) <> ''),
    payroll_source_row_count   INTEGER     NOT NULL DEFAULT 0 CHECK (payroll_source_row_count >= 0),
    attendance_source_row_count INTEGER    NOT NULL DEFAULT 0 CHECK (attendance_source_row_count >= 0),
    attendance_event_count     INTEGER     NOT NULL DEFAULT 0 CHECK (attendance_event_count >= 0),
    work_days                  NUMERIC(10,2) NULL,
    regular_hours              NUMERIC(10,2) NULL,
    overtime_hours             NUMERIC(10,2) NULL,
    night_hours                NUMERIC(10,2) NULL,
    holiday_hours              NUMERIC(10,2) NULL,
    leave_used                 NUMERIC(10,2) NULL,
    leave_remaining            NUMERIC(10,2) NULL,
    gross_pay_source_present   BOOLEAN     NOT NULL DEFAULT FALSE,
    net_pay_source_present     BOOLEAN     NOT NULL DEFAULT FALSE,
    nts_tax_row_status         TEXT        NOT NULL DEFAULT 'REQUIRED_NOT_SUPPLIED'
        CHECK (nts_tax_row_status IN ('REQUIRED_NOT_SUPPLIED','SUPPLIED_UNVERIFIED','VERIFIED_SOURCE_ROW')),
    calculation_status         TEXT        NOT NULL DEFAULT 'BLOCKED_LEGAL_GATE'
        CHECK (calculation_status IN ('BLOCKED_LEGAL_GATE','READY_FOR_REVIEW','APPROVED','ISSUED','VOID')),
    blockers                   JSONB       NOT NULL DEFAULT '[]'::jsonb,
    source_data_import_row_ids UUID[]      NOT NULL DEFAULT '{}',
    created_at                 TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at                 TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (org_id, run_id, employee_source_key),
    FOREIGN KEY (run_id, org_id) REFERENCES payroll_draft_runs(id, org_id) ON DELETE CASCADE,
    CONSTRAINT payroll_draft_lines_same_org_employee
        CHECK (employee_id IS NULL OR org_id IS NOT NULL)
);

CREATE INDEX payroll_draft_lines_run_idx
    ON payroll_draft_lines (org_id, run_id, employee_company, employee_display_name);
CREATE INDEX payroll_draft_lines_employee_idx
    ON payroll_draft_lines (org_id, employee_id) WHERE employee_id IS NOT NULL;

CREATE TABLE annual_leave_obligations (
    id                 UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id             UUID        NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    employee_id        UUID        NOT NULL REFERENCES employees(id) ON DELETE RESTRICT,
    leave_year         INTEGER     NOT NULL CHECK (leave_year BETWEEN 2000 AND 2100),
    leave_accrued      NUMERIC(10,2) NULL,
    leave_used         NUMERIC(10,2) NULL,
    leave_remaining    NUMERIC(10,2) NULL,
    status             TEXT        NOT NULL
        CHECK (status IN ('NEEDS_HR_REVIEW','USAGE_PROMOTION_DRAFT_REQUIRED','PROMOTION_SENT','PAYOUT_REVIEW_REQUIRED','CLOSED')),
    statutory_basis    JSONB       NOT NULL DEFAULT '{}'::jsonb,
    notification_plan  JSONB       NOT NULL DEFAULT '{}'::jsonb,
    workflow_object_id UUID        NULL,
    created_at         TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at         TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (org_id, employee_id, leave_year)
);

CREATE INDEX annual_leave_obligations_org_status_idx
    ON annual_leave_obligations (org_id, leave_year DESC, status);

CREATE OR REPLACE FUNCTION payroll_readiness_employee_same_org()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
    IF NEW.employee_id IS NOT NULL AND NOT EXISTS (
        SELECT 1 FROM employees e WHERE e.id = NEW.employee_id AND e.org_id = NEW.org_id
    ) THEN
        RAISE EXCEPTION 'payroll readiness employee_id must belong to the same org'
            USING ERRCODE = '23514';
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER trg_payroll_draft_lines_employee_same_org
    BEFORE INSERT OR UPDATE ON payroll_draft_lines
    FOR EACH ROW EXECUTE FUNCTION payroll_readiness_employee_same_org();

CREATE TRIGGER trg_annual_leave_obligations_employee_same_org
    BEFORE INSERT OR UPDATE ON annual_leave_obligations
    FOR EACH ROW EXECUTE FUNCTION payroll_readiness_employee_same_org();

DO $$
DECLARE
    t TEXT;
    tenant_tables TEXT[] := ARRAY[
        'payroll_draft_runs',
        'payroll_draft_lines',
        'annual_leave_obligations'
    ];
BEGIN
    FOREACH t IN ARRAY tenant_tables LOOP
        EXECUTE format('ALTER TABLE %I ENABLE ROW LEVEL SECURITY', t);
        EXECUTE format('ALTER TABLE %I FORCE ROW LEVEL SECURITY', t);
        EXECUTE format(
            'CREATE POLICY org_isolation ON %I '
            || 'USING (org_id = NULLIF(current_setting(''app.current_org'', true), '''')::uuid) '
            || 'WITH CHECK (org_id = NULLIF(current_setting(''app.current_org'', true), '''')::uuid)',
            t
        );
    END LOOP;
END
$$;

GRANT SELECT, INSERT, UPDATE ON payroll_draft_runs TO console_rt;
GRANT SELECT, INSERT, UPDATE ON payroll_draft_lines TO console_rt;
GRANT SELECT, INSERT, UPDATE ON annual_leave_obligations TO console_rt;
