-- G011 HR core: promote safe employee/setup fields from preserved raw imports.
-- Payroll, bank/account, resident-registration, insurance, disability/protected
-- status, and retirement-settlement values remain in employees.raw_row until
-- dedicated high-sensitivity payroll/HR schemas and permissions are approved.

ALTER TABLE employees
    ADD COLUMN employee_number TEXT,
    ADD COLUMN org_unit TEXT,
    ADD COLUMN job TEXT,
    ADD COLUMN position TEXT,
    ADD COLUMN worksite_name TEXT,
    ADD COLUMN worksite_address TEXT,
    ADD COLUMN hire_date TEXT,
    ADD COLUMN exit_date TEXT,
    ADD COLUMN employment_status TEXT NOT NULL DEFAULT 'ACTIVE'
        CHECK (employment_status IN ('ACTIVE', 'EXITED', 'UNKNOWN')),
    ADD COLUMN leave_accrued NUMERIC(10, 2),
    ADD COLUMN leave_used NUMERIC(10, 2),
    ADD COLUMN leave_remaining NUMERIC(10, 2);

UPDATE employees
SET
    employee_number = NULLIF(btrim(raw_row->>'사번'), ''),
    org_unit = COALESCE(NULLIF(btrim(raw_row->>'부서명'), ''), NULLIF(btrim(raw_row->>'소속'), '')),
    job = NULLIF(btrim(raw_row->>'업무'), ''),
    position = COALESCE(NULLIF(btrim(raw_row->>'직책'), ''), NULLIF(btrim(raw_row->>'직위'), '')),
    worksite_name = NULLIF(btrim(raw_row->>'근무지'), ''),
    worksite_address = NULLIF(btrim(raw_row->>'근무지(주소)'), ''),
    hire_date = NULLIF(btrim(raw_row->>'입사일'), ''),
    exit_date = NULLIF(btrim(raw_row->>'퇴사일'), ''),
    employment_status = CASE
        WHEN NULLIF(btrim(raw_row->>'퇴사일'), '') IS NULL THEN 'ACTIVE'
        ELSE 'EXITED'
    END,
    leave_accrued = CASE
        WHEN NULLIF(btrim(raw_row->>'발생연차'), '') ~ '^-?[0-9]+(\.[0-9]+)?$'
        THEN (raw_row->>'발생연차')::NUMERIC(10, 2)
        ELSE NULL
    END,
    leave_used = CASE
        WHEN NULLIF(btrim(raw_row->>'사용연차'), '') ~ '^-?[0-9]+(\.[0-9]+)?$'
        THEN (raw_row->>'사용연차')::NUMERIC(10, 2)
        ELSE NULL
    END,
    leave_remaining = CASE
        WHEN NULLIF(btrim(raw_row->>'잔여연차'), '') ~ '^-?[0-9]+(\.[0-9]+)?$'
        THEN (raw_row->>'잔여연차')::NUMERIC(10, 2)
        ELSE NULL
    END;

CREATE INDEX employees_org_unit_position_idx
    ON employees (org_id, company, org_unit, position);
CREATE INDEX employees_org_status_idx
    ON employees (org_id, employment_status);
