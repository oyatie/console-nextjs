-- G006 HR lifecycle: auditable employee onboarding/offboarding/termination/
-- transfer events with Korean labor/privacy/payroll signoff evidence.
--
-- Employees remain tenant data under `employees.org_id`; this table records the
-- state-transition ledger for those rows. Payroll/account/resident identifiers
-- are deliberately not modeled here: sensitive values stay in the governed raw
-- import ledger until a dedicated payroll module is implemented.

CREATE TABLE employee_lifecycle_events (
    id             UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id         UUID        NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    employee_id    UUID        NOT NULL REFERENCES employees(id) ON DELETE RESTRICT,
    event_type     TEXT        NOT NULL
        CHECK (event_type IN ('ONBOARD', 'OFFBOARD', 'TERMINATE', 'TRANSFER')),
    from_status    TEXT,
    to_status      TEXT        NOT NULL
        CHECK (to_status IN ('ACTIVE', 'EXITED', 'UNKNOWN')),
    from_company   TEXT,
    to_company     TEXT,
    from_org_unit  TEXT,
    to_org_unit    TEXT,
    from_position  TEXT,
    to_position    TEXT,
    effective_date TEXT        NOT NULL CHECK (btrim(effective_date) <> ''),
    comment        TEXT        NOT NULL CHECK (btrim(comment) <> ''),
    signoffs       JSONB       NOT NULL DEFAULT '{}'::jsonb,
    created_by     UUID        NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    created_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT employee_lifecycle_signoffs_object
        CHECK (jsonb_typeof(signoffs) = 'object')
);

CREATE INDEX employee_lifecycle_events_employee_idx
    ON employee_lifecycle_events (org_id, employee_id, created_at DESC);
CREATE INDEX employee_lifecycle_events_event_type_idx
    ON employee_lifecycle_events (org_id, event_type, created_at DESC);

CREATE OR REPLACE FUNCTION employee_lifecycle_events_same_org()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
    IF NOT EXISTS (
        SELECT 1
        FROM employees e
        WHERE e.id = NEW.employee_id
          AND e.org_id = NEW.org_id
    ) THEN
        RAISE EXCEPTION 'employee lifecycle event employee_id must belong to the same org'
            USING ERRCODE = '23514';
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER trg_employee_lifecycle_events_same_org
    BEFORE INSERT ON employee_lifecycle_events
    FOR EACH ROW EXECUTE FUNCTION employee_lifecycle_events_same_org();

ALTER TABLE employee_lifecycle_events ENABLE ROW LEVEL SECURITY;
ALTER TABLE employee_lifecycle_events FORCE ROW LEVEL SECURITY;
CREATE POLICY org_isolation ON employee_lifecycle_events
    USING (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid)
    WITH CHECK (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid);

-- Append-only ledger: corrections are new events, not UPDATE/DELETE.
CREATE OR REPLACE FUNCTION employee_lifecycle_events_append_only()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
    RAISE EXCEPTION 'employee_lifecycle_events is append-only: % is forbidden (row id=%)', TG_OP, OLD.id
        USING ERRCODE = '55000';
END;
$$;

CREATE TRIGGER trg_employee_lifecycle_events_no_update
    BEFORE UPDATE ON employee_lifecycle_events
    FOR EACH ROW EXECUTE FUNCTION employee_lifecycle_events_append_only();

CREATE TRIGGER trg_employee_lifecycle_events_no_delete
    BEFORE DELETE ON employee_lifecycle_events
    FOR EACH ROW EXECUTE FUNCTION employee_lifecycle_events_append_only();

GRANT SELECT, INSERT ON employee_lifecycle_events TO console_rt;
