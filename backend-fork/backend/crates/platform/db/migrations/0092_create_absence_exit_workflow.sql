-- G009 HR absence-to-exit workflow.
--
-- Absence alerts are materialized from imported attendance facts so site
-- managers, HR, HQ HR, payroll, and 4-insurance loss reporters see the same
-- warning object. Exit cases then carry the site-manager report, HR/HQ
-- confirmation, insurance-loss package, severance calculation inputs, and
-- approval-draft payload without guessing payroll-sensitive figures.

-- console-gate: audited-table employee_absence_alerts
CREATE TABLE employee_absence_alerts (
    id                  UUID        NOT NULL DEFAULT gen_random_uuid(),
    org_id              UUID        NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    employee_id          UUID        NOT NULL,
    branch_id            UUID        NULL REFERENCES branches(id) ON DELETE SET NULL,
    work_date            DATE        NOT NULL,
    source               TEXT        NOT NULL DEFAULT 'attendance_direct_import'
        CHECK (source IN ('attendance_direct_import','employee_self_service_gap','manual')),
    status               TEXT        NOT NULL DEFAULT 'OPEN'
        CHECK (status IN ('OPEN','ACKNOWLEDGED','LINKED_EXIT','RESOLVED')),
    severity             TEXT        NOT NULL DEFAULT 'WARNING'
        CHECK (severity IN ('INFO','WARNING','CRITICAL')),
    audience_roles       TEXT[]      NOT NULL DEFAULT ARRAY[
        'site_manager',
        'employee_hr_manager',
        'hq_hr_manager',
        'payroll_manager',
        'insurance_loss_reporter'
    ]::TEXT[],
    signal_payload       JSONB       NOT NULL DEFAULT '{}'::jsonb,
    detected_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    acknowledged_by      UUID        NULL REFERENCES users(id) ON DELETE SET NULL,
    acknowledged_at      TIMESTAMPTZ NULL,
    linked_exit_case_id  UUID        NULL,
    created_at           TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at           TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (id, org_id),
    UNIQUE (org_id, employee_id, work_date, source),
    FOREIGN KEY (employee_id, org_id) REFERENCES employees(id, org_id) ON DELETE CASCADE
);

CREATE INDEX employee_absence_alerts_org_status_idx
    ON employee_absence_alerts (org_id, status, work_date DESC, employee_id);
CREATE INDEX employee_absence_alerts_branch_status_idx
    ON employee_absence_alerts (org_id, branch_id, status, work_date DESC)
    WHERE branch_id IS NOT NULL;
CREATE TRIGGER trg_employee_absence_alerts_org_immutable
    BEFORE UPDATE ON employee_absence_alerts
    FOR EACH ROW EXECUTE FUNCTION enforce_org_id_immutable();

-- console-gate: audited-table employee_exit_cases
CREATE TABLE employee_exit_cases (
    id                   UUID        NOT NULL DEFAULT gen_random_uuid(),
    org_id               UUID        NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    employee_id           UUID        NOT NULL,
    branch_id             UUID        NULL REFERENCES branches(id) ON DELETE SET NULL,
    absence_alert_id      UUID        NULL,
    status                TEXT        NOT NULL DEFAULT 'REPORTED'
        CHECK (status IN (
            'REPORTED',
            'HR_CONFIRMED',
            'HQ_CONFIRMED',
            'SETTLEMENT_READY',
            'APPROVAL_DRAFTED',
            'SUBMITTED',
            'REJECTED',
            'CANCELLED'
        )),
    effective_exit_date   DATE        NOT NULL,
    site_manager_note     TEXT        NOT NULL CHECK (btrim(site_manager_note) <> '' AND char_length(site_manager_note) <= 1000),
    reported_by           UUID        NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    reported_at           TIMESTAMPTZ NOT NULL DEFAULT now(),
    hr_confirmed_by       UUID        NULL REFERENCES users(id) ON DELETE RESTRICT,
    hr_confirmed_at       TIMESTAMPTZ NULL,
    hq_confirmed_by       UUID        NULL REFERENCES users(id) ON DELETE RESTRICT,
    hq_confirmed_at       TIMESTAMPTZ NULL,
    confirmation_note     TEXT        NULL CHECK (confirmation_note IS NULL OR char_length(confirmation_note) <= 1000),
    approval_submitted_by UUID        NULL REFERENCES users(id) ON DELETE RESTRICT,
    approval_submitted_at TIMESTAMPTZ NULL,
    created_at            TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at            TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (id, org_id),
    FOREIGN KEY (employee_id, org_id) REFERENCES employees(id, org_id) ON DELETE CASCADE,
    FOREIGN KEY (absence_alert_id, org_id) REFERENCES employee_absence_alerts(id, org_id) ON DELETE CASCADE
);

CREATE UNIQUE INDEX employee_exit_cases_active_employee_idx
    ON employee_exit_cases (org_id, employee_id)
    WHERE status NOT IN ('REJECTED','CANCELLED','SUBMITTED');
CREATE INDEX employee_exit_cases_org_status_idx
    ON employee_exit_cases (org_id, status, effective_exit_date DESC);
CREATE INDEX employee_exit_cases_branch_status_idx
    ON employee_exit_cases (org_id, branch_id, status, effective_exit_date DESC)
    WHERE branch_id IS NOT NULL;
CREATE TRIGGER trg_employee_exit_cases_org_immutable
    BEFORE UPDATE ON employee_exit_cases
    FOR EACH ROW EXECUTE FUNCTION enforce_org_id_immutable();

ALTER TABLE employee_absence_alerts
    ADD CONSTRAINT employee_absence_alerts_linked_exit_same_org_fk
    FOREIGN KEY (linked_exit_case_id, org_id) REFERENCES employee_exit_cases(id, org_id) ON DELETE CASCADE;

-- console-gate: audited-table employee_exit_settlement_packages
CREATE TABLE employee_exit_settlement_packages (
    id                            UUID        NOT NULL DEFAULT gen_random_uuid(),
    org_id                        UUID        NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    exit_case_id                  UUID        NOT NULL,
    employee_id                   UUID        NOT NULL,
    status                        TEXT        NOT NULL DEFAULT 'NEEDS_SOURCE'
        CHECK (status IN ('NEEDS_SOURCE','READY_FOR_APPROVAL','APPROVAL_DRAFTED','SUBMITTED')),
    service_days                  INTEGER     NULL CHECK (service_days IS NULL OR service_days >= 0),
    average_wage_period_start     DATE        NULL,
    average_wage_period_end       DATE        NULL,
    average_wage_calendar_days    INTEGER     NULL CHECK (average_wage_calendar_days IS NULL OR average_wage_calendar_days > 0),
    average_wage_total_won        BIGINT      NULL CHECK (average_wage_total_won IS NULL OR average_wage_total_won >= 0),
    average_daily_wage_milliwon   BIGINT      NULL CHECK (average_daily_wage_milliwon IS NULL OR average_daily_wage_milliwon >= 0),
    severance_pay_won             BIGINT      NULL CHECK (severance_pay_won IS NULL OR severance_pay_won >= 0),
    missing_source_fields         TEXT[]      NOT NULL DEFAULT '{}',
    statutory_basis               JSONB       NOT NULL DEFAULT '{}'::jsonb,
    insurance_loss_payload        JSONB       NOT NULL DEFAULT '{}'::jsonb,
    approval_payload              JSONB       NOT NULL DEFAULT '{}'::jsonb,
    generated_at                  TIMESTAMPTZ NOT NULL DEFAULT now(),
    submitted_by                  UUID        NULL REFERENCES users(id) ON DELETE RESTRICT,
    submitted_at                  TIMESTAMPTZ NULL,
    created_at                    TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at                    TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (id, org_id),
    UNIQUE (org_id, exit_case_id),
    FOREIGN KEY (exit_case_id, org_id) REFERENCES employee_exit_cases(id, org_id) ON DELETE CASCADE,
    FOREIGN KEY (employee_id, org_id) REFERENCES employees(id, org_id) ON DELETE CASCADE
);

CREATE INDEX employee_exit_settlement_packages_status_idx
    ON employee_exit_settlement_packages (org_id, status, generated_at DESC);
CREATE TRIGGER trg_employee_exit_settlement_packages_org_immutable
    BEFORE UPDATE ON employee_exit_settlement_packages
    FOR EACH ROW EXECUTE FUNCTION enforce_org_id_immutable();

DO $$
DECLARE
    t TEXT;
    tenant_tables TEXT[] := ARRAY[
        'employee_absence_alerts',
        'employee_exit_cases',
        'employee_exit_settlement_packages'
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

GRANT SELECT, INSERT, UPDATE ON employee_absence_alerts TO console_rt;
GRANT SELECT, INSERT, UPDATE ON employee_exit_cases TO console_rt;
GRANT SELECT, INSERT, UPDATE ON employee_exit_settlement_packages TO console_rt;
