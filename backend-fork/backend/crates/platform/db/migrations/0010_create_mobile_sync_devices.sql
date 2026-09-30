-- T1.3c mobile sync, evidence confirmation, and device registration.
-- Raw client device identifiers are never stored; REST hashes X-Device-Id
-- server-side before writing these tables.

-- console-gate: audited-table offline_sync_requests
CREATE TABLE offline_sync_requests (
    id                UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id           UUID        NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    device_hash       TEXT        NOT NULL CHECK (device_hash ~ '^[a-f0-9]{64}$'),
    request_id        TEXT        NOT NULL CHECK (request_id <> ''),
    sync_id           TEXT        NOT NULL CHECK (sync_id <> ''),
    operation_type    TEXT        NOT NULL CHECK (
        operation_type IN ('WORK_ORDER_START','WORK_ORDER_REPORT')
    ),
    client_created_at TIMESTAMPTZ NOT NULL,
    branch_id         UUID        REFERENCES branches(id) ON DELETE RESTRICT,
    work_order_id     UUID        REFERENCES work_orders(id) ON DELETE SET NULL,
    status            TEXT        NOT NULL CHECK (status IN ('IN_PROGRESS','APPLIED','FAILED')),
    http_status       INTEGER     CHECK (http_status BETWEEN 100 AND 599),
    response_body     JSONB,
    received_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    completed_at      TIMESTAMPTZ,
    UNIQUE (device_hash, request_id)
);

CREATE INDEX idx_offline_sync_requests_user_received
    ON offline_sync_requests (user_id, received_at DESC);

CREATE INDEX idx_offline_sync_requests_work_order
    ON offline_sync_requests (work_order_id, received_at DESC)
    WHERE work_order_id IS NOT NULL;

-- console-gate: audited-table registered_devices
CREATE TABLE registered_devices (
    id                 UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id            UUID        NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    device_hash        TEXT        NOT NULL CHECK (device_hash ~ '^[a-f0-9]{64}$'),
    platform           TEXT        NOT NULL CHECK (platform IN ('IOS','ANDROID')),
    push_token         TEXT,
    app_version        TEXT        NOT NULL CHECK (app_version <> ''),
    last_registered_at TIMESTAMPTZ NOT NULL,
    created_at         TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at         TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (user_id, device_hash)
);

CREATE INDEX idx_registered_devices_user_updated
    ON registered_devices (user_id, updated_at DESC);

ALTER TABLE evidence_media
    ADD COLUMN upload_confirmed_at TIMESTAMPTZ,
    ADD COLUMN confirmed_by UUID REFERENCES users(id) ON DELETE RESTRICT;
