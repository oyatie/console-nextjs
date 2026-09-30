-- Canonical employment facts created by the People & Workforce console.
-- Compensation deliberately lives outside the ordinary directory row so list
-- readers cannot receive it accidentally.
CREATE TABLE employee_employment_profiles (
    employee_id       UUID PRIMARY KEY,
    org_id            UUID NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    employment_type   TEXT NOT NULL CHECK (employment_type IN ('REGULAR', 'CONTRACT', 'PART_TIME', 'INTERN')),
    phone_e164        TEXT NOT NULL CHECK (phone_e164 ~ '^\+[1-9][0-9]{7,14}$'),
    base_pay          NUMERIC(14, 2) NOT NULL CHECK (base_pay >= 0),
    currency          TEXT NOT NULL DEFAULT 'KRW' CHECK (currency = 'KRW'),
    idempotency_key   TEXT NOT NULL CHECK (btrim(idempotency_key) <> ''),
    request_hash      TEXT NOT NULL CHECK (request_hash ~ '^[0-9a-f]{64}$'),
    created_by        UUID NOT NULL,
    created_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (org_id, idempotency_key),
    FOREIGN KEY (employee_id, org_id) REFERENCES employees(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (created_by, org_id) REFERENCES users(id, org_id) ON DELETE RESTRICT
);

-- Imported duplicates remain valid historical rows. New console-created
-- identities are checked by the advisory-locking trigger below, so this
-- migration never fails an existing tenant merely because 0076 retained a
-- review-required duplicate number.
CREATE INDEX employees_org_employee_number_idx
    ON employees (org_id, employee_number)
    WHERE employee_number IS NOT NULL;
CREATE INDEX employee_employment_profiles_org_employee_idx
    ON employee_employment_profiles (org_id, employee_id);

-- Reserving the key before any employee row is written makes same-key retries
-- serializable: a concurrent caller waits on this row, then replays the one
-- committed employee instead of racing the employee-number uniqueness rule.
CREATE TABLE employee_create_idempotency (
    org_id          UUID NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    idempotency_key TEXT NOT NULL CHECK (btrim(idempotency_key) <> ''),
    request_hash    TEXT NOT NULL CHECK (request_hash ~ '^[0-9a-f]{64}$'),
    employee_id     UUID,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (org_id, idempotency_key),
    FOREIGN KEY (employee_id, org_id) REFERENCES employees(id, org_id) ON DELETE RESTRICT
);
ALTER TABLE employee_create_idempotency ENABLE ROW LEVEL SECURITY;
ALTER TABLE employee_create_idempotency FORCE ROW LEVEL SECURITY;
CREATE POLICY org_isolation ON employee_create_idempotency
    USING (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid)
    WITH CHECK (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid);
GRANT SELECT, INSERT, UPDATE ON employee_create_idempotency TO console_rt;

ALTER TABLE employee_employment_profiles ENABLE ROW LEVEL SECURITY;
ALTER TABLE employee_employment_profiles FORCE ROW LEVEL SECURITY;
CREATE POLICY org_isolation ON employee_employment_profiles
    USING (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid)
    WITH CHECK (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid);

CREATE OR REPLACE FUNCTION employee_employment_profiles_same_org()
RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF NOT EXISTS (SELECT 1 FROM employees e WHERE e.id = NEW.employee_id AND e.org_id = NEW.org_id) THEN
        RAISE EXCEPTION 'employee employment profile must belong to the same org' USING ERRCODE = '23514';
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER trg_employee_employment_profiles_same_org
    BEFORE INSERT OR UPDATE ON employee_employment_profiles
    FOR EACH ROW EXECUTE FUNCTION employee_employment_profiles_same_org();

CREATE OR REPLACE FUNCTION console_employee_number_unique()
RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.source_filename = 'console' AND NEW.employee_number IS NOT NULL THEN
        PERFORM pg_advisory_xact_lock(hashtext(NEW.org_id::text || ':' || NEW.employee_number));
        IF EXISTS (
            SELECT 1 FROM employees e
            WHERE e.org_id = NEW.org_id
              AND e.employee_number = NEW.employee_number
        ) THEN
            RAISE EXCEPTION 'employee number already exists in this organization' USING ERRCODE = '23505';
        END IF;
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER trg_console_employee_number_unique
    BEFORE INSERT ON employees
    FOR EACH ROW EXECUTE FUNCTION console_employee_number_unique();

GRANT SELECT, INSERT ON employee_employment_profiles TO console_rt;
