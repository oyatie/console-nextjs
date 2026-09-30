-- T6.4 inspection domain: regular inspection schedules and completed rounds.
-- 예방팀 is represented by users.team = '예방'; adapters enforce that
-- assigned mechanics are active MECHANIC users in the schedule branch.

-- console-gate: audited-table regular_inspection_schedules
CREATE TABLE regular_inspection_schedules (
    id            UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    branch_id     UUID        NOT NULL REFERENCES branches(id) ON DELETE RESTRICT,
    equipment_id  UUID        NOT NULL REFERENCES registry_equipment(id) ON DELETE RESTRICT,
    mechanic_id   UUID        NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    cycle         TEXT        NOT NULL CHECK (
        cycle IN ('DAILY','WEEKLY','MONTHLY','QUARTERLY','YEARLY','CUSTOM')
    ),
    interval_days INTEGER     NOT NULL CHECK (interval_days > 0),
    due_date      DATE        NOT NULL,
    status        TEXT        NOT NULL CHECK (status IN ('SCHEDULED','COMPLETED','CANCELLED')),
    completed_at  TIMESTAMPTZ,
    completed_by  UUID        REFERENCES users(id) ON DELETE RESTRICT,
    note          TEXT,
    created_by    UUID        NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    created_at    TIMESTAMPTZ NOT NULL,
    updated_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK (
        (status = 'COMPLETED' AND completed_at IS NOT NULL AND completed_by IS NOT NULL)
        OR (status <> 'COMPLETED' AND completed_at IS NULL AND completed_by IS NULL)
    ),
    UNIQUE (branch_id, equipment_id, due_date, cycle)
);

CREATE INDEX idx_regular_inspection_schedules_due
    ON regular_inspection_schedules (branch_id, due_date, status);

CREATE INDEX idx_regular_inspection_schedules_mechanic_due
    ON regular_inspection_schedules (mechanic_id, due_date);

-- console-gate: audited-table inspection_rounds
CREATE TABLE inspection_rounds (
    id            UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    schedule_id   UUID        NOT NULL REFERENCES regular_inspection_schedules(id) ON DELETE RESTRICT,
    branch_id     UUID        NOT NULL REFERENCES branches(id) ON DELETE RESTRICT,
    equipment_id  UUID        NOT NULL REFERENCES registry_equipment(id) ON DELETE RESTRICT,
    mechanic_id   UUID        NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    completed_by  UUID        NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    outcome       TEXT        NOT NULL CHECK (outcome IN ('COMPLETED','FOLLOW_UP_REQUIRED')),
    findings      TEXT        NOT NULL CHECK (findings <> ''),
    note          TEXT,
    completed_at  TIMESTAMPTZ NOT NULL,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (schedule_id)
);

CREATE INDEX idx_inspection_rounds_branch_completed
    ON inspection_rounds (branch_id, completed_at DESC);

CREATE INDEX idx_inspection_rounds_equipment_completed
    ON inspection_rounds (equipment_id, completed_at DESC);
