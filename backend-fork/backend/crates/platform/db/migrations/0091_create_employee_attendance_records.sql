-- G004 employee self-service attendance records and payroll material lineage.
--
-- These are durable employee work facts entered directly from mobile/PC. They are
-- intentionally separate from site_attendance_events: the site table is derived
-- from geofence crossings for work orders, while these rows are explicit payroll
-- readiness inputs linked to a trusted users.employee_id mapping.

-- console-gate: audited-table employee_attendance_records
CREATE TABLE employee_attendance_records (
    id              UUID        NOT NULL DEFAULT gen_random_uuid(),
    org_id          UUID        NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    employee_id     UUID        NOT NULL REFERENCES employees(id) ON DELETE RESTRICT,
    actor_user_id   UUID        NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    kind            TEXT        NOT NULL CHECK (kind IN ('CLOCK_IN','OUT_FOR_WORK','BUSINESS_TRIP','RETURNED','CLOCK_OUT')),
    occurred_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    work_date       DATE        NOT NULL DEFAULT ((now() AT TIME ZONE 'Asia/Seoul')::DATE),
    state_after     TEXT        NOT NULL CHECK (state_after IN ('CLOCKED_IN','OUT_FOR_WORK','BUSINESS_TRIP','OFF_DUTY')),
    note            TEXT        NULL CHECK (note IS NULL OR char_length(note) <= 500),
    idempotency_key TEXT        NOT NULL CHECK (btrim(idempotency_key) <> '' AND char_length(idempotency_key) <= 128),
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (id, org_id),
    FOREIGN KEY (employee_id, org_id) REFERENCES employees(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (actor_user_id, org_id) REFERENCES users(id, org_id) ON DELETE RESTRICT
);

CREATE UNIQUE INDEX employee_attendance_records_id_org_unique
    ON employee_attendance_records (id, org_id);
CREATE UNIQUE INDEX employee_attendance_records_idempotency_idx
    ON employee_attendance_records (org_id, employee_id, idempotency_key);
CREATE INDEX employee_attendance_records_employee_time_idx
    ON employee_attendance_records (org_id, employee_id, occurred_at DESC, id DESC);
CREATE INDEX employee_attendance_records_actor_time_idx
    ON employee_attendance_records (org_id, actor_user_id, occurred_at DESC, id DESC);
CREATE INDEX employee_attendance_records_work_date_idx
    ON employee_attendance_records (org_id, work_date DESC, employee_id);

ALTER TABLE employee_attendance_records ENABLE ROW LEVEL SECURITY;
ALTER TABLE employee_attendance_records FORCE ROW LEVEL SECURITY;
CREATE POLICY org_isolation ON employee_attendance_records
    USING (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid)
    WITH CHECK (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid);
CREATE TRIGGER trg_employee_attendance_records_org_immutable
    BEFORE UPDATE ON employee_attendance_records
    FOR EACH ROW EXECUTE FUNCTION enforce_org_id_immutable();

CREATE OR REPLACE FUNCTION forbid_employee_attendance_records_mutation()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
    RAISE EXCEPTION 'employee_attendance_records is append-only: % is forbidden (row id=%)', TG_OP, OLD.id
        USING ERRCODE = '55000';
END;
$$;
CREATE TRIGGER trg_employee_attendance_records_append_only
    BEFORE UPDATE OR DELETE ON employee_attendance_records
    FOR EACH ROW EXECUTE FUNCTION forbid_employee_attendance_records_mutation();

-- console-gate: audited-table payroll_attendance_material_refs
CREATE TABLE payroll_attendance_material_refs (
    id                   UUID        NOT NULL DEFAULT gen_random_uuid(),
    org_id               UUID        NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    attendance_record_id UUID        NOT NULL,
    employee_id          UUID        NOT NULL REFERENCES employees(id) ON DELETE RESTRICT,
    work_date            DATE        NOT NULL,
    source_type          TEXT        NOT NULL DEFAULT 'employee_self_service'
        CHECK (source_type IN ('employee_self_service')),
    source_digest        TEXT        NOT NULL CHECK (btrim(source_digest) <> ''),
    created_at           TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (id, org_id),
    UNIQUE (org_id, attendance_record_id),
    FOREIGN KEY (attendance_record_id, org_id) REFERENCES employee_attendance_records(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (employee_id, org_id) REFERENCES employees(id, org_id) ON DELETE RESTRICT
);

CREATE INDEX payroll_attendance_material_refs_employee_date_idx
    ON payroll_attendance_material_refs (org_id, employee_id, work_date DESC);
CREATE INDEX payroll_attendance_material_refs_date_idx
    ON payroll_attendance_material_refs (org_id, work_date DESC);

ALTER TABLE payroll_attendance_material_refs ENABLE ROW LEVEL SECURITY;
ALTER TABLE payroll_attendance_material_refs FORCE ROW LEVEL SECURITY;
CREATE POLICY org_isolation ON payroll_attendance_material_refs
    USING (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid)
    WITH CHECK (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid);
CREATE TRIGGER trg_payroll_attendance_material_refs_org_immutable
    BEFORE UPDATE ON payroll_attendance_material_refs
    FOR EACH ROW EXECUTE FUNCTION enforce_org_id_immutable();

CREATE OR REPLACE FUNCTION forbid_payroll_attendance_material_refs_mutation()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
    RAISE EXCEPTION 'payroll_attendance_material_refs is append-only: % is forbidden (row id=%)', TG_OP, OLD.id
        USING ERRCODE = '55000';
END;
$$;
CREATE TRIGGER trg_payroll_attendance_material_refs_append_only
    BEFORE UPDATE OR DELETE ON payroll_attendance_material_refs
    FOR EACH ROW EXECUTE FUNCTION forbid_payroll_attendance_material_refs_mutation();

GRANT SELECT, INSERT ON employee_attendance_records TO console_rt;
GRANT SELECT, INSERT ON payroll_attendance_material_refs TO console_rt;
REVOKE UPDATE, DELETE ON employee_attendance_records FROM console_rt;
REVOKE UPDATE, DELETE ON payroll_attendance_material_refs FROM console_rt;
