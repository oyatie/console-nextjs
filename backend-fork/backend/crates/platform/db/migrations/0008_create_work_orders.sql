-- T1.3 work-order application storage.
-- State changes are audited by application adapters through with_audit; the
-- tables below hold operational state and normalized approval lines.

CREATE TABLE work_order_request_counters (
    request_date  DATE    PRIMARY KEY,
    last_sequence INTEGER NOT NULL CHECK (last_sequence > 0)
);

CREATE TABLE work_orders (
    id                 UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    request_no         TEXT        NOT NULL UNIQUE CHECK (request_no ~ '^[0-9]{8}-[0-9]{3}$'),
    branch_id          UUID        NOT NULL REFERENCES branches(id) ON DELETE RESTRICT,
    equipment_id       UUID        NOT NULL REFERENCES registry_equipment(id) ON DELETE RESTRICT,
    customer_id        UUID        NOT NULL REFERENCES registry_customers(id) ON DELETE RESTRICT,
    site_id            UUID        NOT NULL REFERENCES registry_sites(id) ON DELETE RESTRICT,
    requested_by       UUID        NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    status             TEXT        NOT NULL CHECK (
        status IN (
            'RECEIVED','UNASSIGNED','ASSIGNED','IN_PROGRESS','REPORT_SUBMITTED',
            'ADMIN_REVIEW','FINAL_COMPLETED','REJECTED','ON_HOLD','DELAYED',
            'TEMPORARY_ACTION','PART_WAITING','EQUIPMENT_IN_USE',
            'REVISIT_REQUIRED','ARCHIVED','CANCELLED'
        )
    ),
    priority           TEXT        NOT NULL DEFAULT 'UNSET' CHECK (
        priority IN ('P1','P2','P3','OUTSOURCE','UNSET')
    ),
    symptom            TEXT        NOT NULL CHECK (symptom <> ''),
    customer_request   TEXT,
    target_due_at      TIMESTAMPTZ,
    delay_reason       TEXT CHECK (
        delay_reason IS NULL OR delay_reason IN (
            'PART_WAITING','CUSTOMER_ABSENT','EQUIPMENT_IN_USE',
            'MECHANIC_OVERLOADED','OUTSOURCE_DELAY','ADDITIONAL_FAULT_FOUND',
            'SAFETY_ISSUE','OTHER'
        )
    ),
    delay_note         TEXT,
    result_type        TEXT        NOT NULL DEFAULT 'UNKNOWN' CHECK (
        result_type IN ('COMPLETED','TEMPORARY_ACTION','INCOMPLETE','REVISIT_REQUIRED','UNKNOWN')
    ),
    diagnosis          TEXT,
    action_taken       TEXT,
    report_submitted_by UUID       REFERENCES users(id) ON DELETE RESTRICT,
    report_submitted_at TIMESTAMPTZ,
    kpi_excluded       BOOLEAN     NOT NULL DEFAULT false,
    evidence_verified  BOOLEAN     NOT NULL DEFAULT false,
    created_at         TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at         TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX idx_work_orders_branch_status
    ON work_orders (branch_id, status, updated_at DESC);

CREATE INDEX idx_work_orders_equipment
    ON work_orders (equipment_id, created_at DESC);

CREATE TABLE work_order_approval_steps (
    id             UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    work_order_id  UUID        NOT NULL REFERENCES work_orders(id) ON DELETE CASCADE,
    step_order     SMALLINT    NOT NULL CHECK (step_order BETWEEN 1 AND 3),
    role           TEXT        NOT NULL CHECK (role IN ('MECHANIC','ADMIN','EXECUTIVE')),
    approver_id    UUID        REFERENCES users(id) ON DELETE RESTRICT,
    status         TEXT        NOT NULL CHECK (status IN ('NOT_STARTED','PENDING','APPROVED','REJECTED')),
    requested_at   TIMESTAMPTZ,
    approved_at    TIMESTAMPTZ,
    approved_by_id UUID        REFERENCES users(id) ON DELETE RESTRICT,
    created_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (work_order_id, role),
    UNIQUE (work_order_id, step_order)
);

CREATE INDEX idx_work_order_approval_steps_pending
    ON work_order_approval_steps (work_order_id, step_order)
    WHERE status = 'PENDING';

CREATE TABLE work_order_assignments (
    id            UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    work_order_id UUID        NOT NULL REFERENCES work_orders(id) ON DELETE CASCADE,
    mechanic_id   UUID        NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    role          TEXT        NOT NULL CHECK (role IN ('PRIMARY','SECONDARY')),
    assigned_at   TIMESTAMPTZ NOT NULL,
    UNIQUE (work_order_id, mechanic_id)
);

CREATE UNIQUE INDEX idx_work_order_assignments_one_primary
    ON work_order_assignments (work_order_id)
    WHERE role = 'PRIMARY';

CREATE TABLE work_order_status_history (
    id             UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    work_order_id  UUID        NOT NULL REFERENCES work_orders(id) ON DELETE CASCADE,
    actor          UUID        REFERENCES users(id) ON DELETE RESTRICT,
    action         TEXT        NOT NULL,
    from_status    TEXT,
    to_status      TEXT        NOT NULL,
    occurred_at    TIMESTAMPTZ NOT NULL,
    created_at     TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX idx_work_order_status_history_work_order
    ON work_order_status_history (work_order_id, occurred_at);

CREATE TABLE target_change_requests (
    id                      UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    work_order_id           UUID        NOT NULL REFERENCES work_orders(id) ON DELETE CASCADE,
    requested_by            UUID        NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    requested_target_due_at TIMESTAMPTZ NOT NULL,
    reason                  TEXT        NOT NULL CHECK (reason <> ''),
    status                  TEXT        NOT NULL CHECK (status IN ('REQUESTED','APPROVED','REJECTED')),
    reviewed_by             UUID        REFERENCES users(id) ON DELETE RESTRICT,
    reviewed_at             TIMESTAMPTZ,
    review_memo             TEXT,
    created_at              TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX idx_target_change_requests_work_order
    ON target_change_requests (work_order_id, created_at DESC);

CREATE TABLE daily_work_plans (
    id           UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    branch_id    UUID        NOT NULL REFERENCES branches(id) ON DELETE RESTRICT,
    mechanic_id  UUID        NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    plan_date    DATE        NOT NULL,
    status       TEXT        NOT NULL CHECK (
        status IN ('DRAFT','REQUESTED','APPROVED','REJECTED','FINAL_CONFIRMED')
    ),
    requested_at TIMESTAMPTZ,
    reviewed_by  UUID        REFERENCES users(id) ON DELETE RESTRICT,
    reviewed_at  TIMESTAMPTZ,
    review_memo  TEXT,
    confirmed_at TIMESTAMPTZ,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (mechanic_id, plan_date)
);

CREATE INDEX idx_daily_work_plans_branch_date
    ON daily_work_plans (branch_id, plan_date, status);

CREATE TABLE daily_work_plan_items (
    id            UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    plan_id       UUID        NOT NULL REFERENCES daily_work_plans(id) ON DELETE CASCADE,
    work_order_id UUID        REFERENCES work_orders(id) ON DELETE SET NULL,
    description   TEXT        NOT NULL CHECK (description <> ''),
    sort_order    INTEGER     NOT NULL CHECK (sort_order > 0),
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (plan_id, sort_order)
);

CREATE TABLE outsource_vendors (
    id         UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    branch_id  UUID        NOT NULL REFERENCES branches(id) ON DELETE RESTRICT,
    name       TEXT        NOT NULL CHECK (name <> ''),
    contact    TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (branch_id, name)
);

CREATE TABLE outsource_works (
    id                 UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    work_order_id      UUID        NOT NULL REFERENCES work_orders(id) ON DELETE CASCADE,
    vendor_id          UUID        NOT NULL REFERENCES outsource_vendors(id) ON DELETE RESTRICT,
    status             TEXT        NOT NULL CHECK (
        status IN ('REQUESTED','ASSIGNED','IN_PROGRESS','RESULT_SUBMITTED','COMPLETED','CANCELLED')
    ),
    reason             TEXT        NOT NULL CHECK (reason <> ''),
    result_description TEXT,
    cost_won           BIGINT,
    requested_at       TIMESTAMPTZ NOT NULL,
    completed_at       TIMESTAMPTZ,
    created_at         TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at         TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX idx_outsource_works_work_order
    ON outsource_works (work_order_id, created_at DESC);
