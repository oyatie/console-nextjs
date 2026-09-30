-- P1 emergency dispatch engine (ADR-0006).
--
-- Dispatch accepts and manager escalation live in a separate FSM from
-- work_orders.status. Location pings remain in the destructible compliance
-- store; dispatch tables keep only ranking facts such as distance and score.

-- console-gate: audited-table p1_dispatches
CREATE TABLE p1_dispatches (
    id                        UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    work_order_id             UUID        NOT NULL REFERENCES work_orders(id) ON DELETE CASCADE,
    branch_id                 UUID        NOT NULL REFERENCES branches(id) ON DELETE RESTRICT,
    status                    TEXT        NOT NULL CHECK (
        status IN ('BROADCASTING','AUTO_ASSIGNED','MANAGER_FORCE_PENDING')
    ),
    incident_latitude         DOUBLE PRECISION CHECK (
        incident_latitude IS NULL OR (
            incident_latitude::text NOT IN ('NaN', 'Infinity', '-Infinity')
            AND incident_latitude BETWEEN -90 AND 90
        )
    ),
    incident_longitude        DOUBLE PRECISION CHECK (
        incident_longitude IS NULL OR (
            incident_longitude::text NOT IN ('NaN', 'Infinity', '-Infinity')
            AND incident_longitude BETWEEN -180 AND 180
        )
    ),
    include_region            BOOLEAN     NOT NULL DEFAULT false,
    accept_window_started_at  TIMESTAMPTZ NOT NULL,
    accept_window_ends_at     TIMESTAMPTZ NOT NULL,
    auto_assigned_mechanic_id UUID        REFERENCES users(id) ON DELETE RESTRICT,
    manager_force_pending_at  TIMESTAMPTZ,
    created_by                UUID        NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    created_at                TIMESTAMPTZ NOT NULL,
    updated_at                TIMESTAMPTZ NOT NULL,
    UNIQUE (work_order_id),
    CHECK (
        (incident_latitude IS NULL AND incident_longitude IS NULL)
        OR (incident_latitude IS NOT NULL AND incident_longitude IS NOT NULL)
    )
);

CREATE INDEX idx_p1_dispatches_branch_status
    ON p1_dispatches (branch_id, status, updated_at DESC);

CREATE INDEX idx_p1_dispatches_accept_window
    ON p1_dispatches (status, accept_window_ends_at)
    WHERE status = 'BROADCASTING';

-- console-gate: audited-table p1_dispatch_targets
CREATE TABLE p1_dispatch_targets (
    id                UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    dispatch_id       UUID        NOT NULL REFERENCES p1_dispatches(id) ON DELETE CASCADE,
    user_id           UUID        NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    target_role       TEXT        NOT NULL CHECK (target_role IN ('TECHNICIAN','MANAGER')),
    push_token_count  INTEGER     NOT NULL DEFAULT 0 CHECK (push_token_count >= 0),
    fanout_created_at TIMESTAMPTZ NOT NULL,
    last_pushed_at    TIMESTAMPTZ,
    UNIQUE (dispatch_id, user_id)
);

CREATE INDEX idx_p1_dispatch_targets_user
    ON p1_dispatch_targets (user_id, fanout_created_at DESC);

-- console-gate: audited-table p1_dispatch_responses
CREATE TABLE p1_dispatch_responses (
    id              UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    dispatch_id     UUID        NOT NULL REFERENCES p1_dispatches(id) ON DELETE CASCADE,
    user_id         UUID        NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    response        TEXT        NOT NULL CHECK (response IN ('ACCEPT','DECLINE')),
    responded_at    TIMESTAMPTZ NOT NULL,
    score_milli     BIGINT,
    gps_ranked      BOOLEAN     NOT NULL DEFAULT false,
    distance_meters BIGINT,
    workload_weight BIGINT      NOT NULL DEFAULT 0,
    score_reason    TEXT,
    UNIQUE (dispatch_id, user_id),
    CHECK ((gps_ranked = true AND distance_meters IS NOT NULL) OR gps_ranked = false)
);

CREATE INDEX idx_p1_dispatch_responses_dispatch
    ON p1_dispatch_responses (dispatch_id, response, responded_at);

-- console-gate: audited-table p1_dispatch_alerts
CREATE TABLE p1_dispatch_alerts (
    id                  UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    dispatch_id          UUID        NOT NULL REFERENCES p1_dispatches(id) ON DELETE CASCADE,
    recipient_user_id    UUID        REFERENCES users(id) ON DELETE RESTRICT,
    alert_type           TEXT        NOT NULL CHECK (
        alert_type IN ('FCM_PUSH','ALIMTALK_NO_ACK','MANAGER_FORCE_ASSIGN')
    ),
    status               TEXT        NOT NULL CHECK (
        status IN ('PENDING','SENT','SKIPPED','FAILED')
    ),
    provider_message_id  TEXT,
    failure_reason       TEXT,
    created_at           TIMESTAMPTZ NOT NULL,
    sent_at              TIMESTAMPTZ
);

CREATE INDEX idx_p1_dispatch_alerts_dispatch
    ON p1_dispatch_alerts (dispatch_id, alert_type, created_at);
