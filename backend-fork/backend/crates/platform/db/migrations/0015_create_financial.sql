-- T5.2/T5.3/T5.4 financial domain.
-- Formula defaults are configuration snapshots pending 경리 validation; values
-- are intentionally stored per quote/request rather than hard-coded.

-- console-gate: audited-table financial_rental_quotes
CREATE TABLE financial_rental_quotes (
    id                              UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    branch_id                       UUID        NOT NULL REFERENCES branches(id) ON DELETE RESTRICT,
    equipment_id                    UUID        NOT NULL REFERENCES registry_equipment(id) ON DELETE RESTRICT,
    created_by                      UUID        NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    acquisition_value_won           BIGINT      NOT NULL CHECK (acquisition_value_won >= 0),
    current_residual_value_won      BIGINT      NOT NULL,
    effective_residual_value_won    BIGINT      NOT NULL CHECK (effective_residual_value_won >= 0),
    residual_was_floored            BOOLEAN     NOT NULL DEFAULT false,
    cumulative_repair_cost_won      BIGINT      NOT NULL CHECK (cumulative_repair_cost_won >= 0),
    depreciation_method             TEXT        NOT NULL CHECK (
        depreciation_method IN ('STRAIGHT_LINE', 'DECLINING_BALANCE')
    ),
    useful_life_months              INTEGER     NOT NULL CHECK (useful_life_months > 0),
    residual_rate_bps               INTEGER     NOT NULL CHECK (residual_rate_bps BETWEEN 0 AND 10000),
    declining_balance_rate_bps      INTEGER     NOT NULL CHECK (declining_balance_rate_bps BETWEEN 0 AND 10000),
    management_fee_rate_bps         INTEGER     NOT NULL CHECK (management_fee_rate_bps BETWEEN 0 AND 10000),
    profit_rate_bps                 INTEGER     NOT NULL CHECK (profit_rate_bps BETWEEN 0 AND 10000),
    floor_negative_quote_residual   BOOLEAN     NOT NULL,
    monthly_total_won               BIGINT      NOT NULL CHECK (monthly_total_won >= 0),
    created_at                      TIMESTAMPTZ NOT NULL,
    updated_at                      TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX idx_financial_rental_quotes_equipment
    ON financial_rental_quotes (branch_id, equipment_id, created_at DESC);

CREATE TABLE financial_rental_quote_lines (
    id           UUID     PRIMARY KEY DEFAULT gen_random_uuid(),
    quote_id     UUID     NOT NULL REFERENCES financial_rental_quotes(id) ON DELETE CASCADE,
    line_order   SMALLINT NOT NULL CHECK (line_order > 0),
    code         TEXT     NOT NULL CHECK (code <> ''),
    label        TEXT     NOT NULL CHECK (label <> ''),
    amount_won   BIGINT   NOT NULL CHECK (amount_won >= 0),
    UNIQUE (quote_id, line_order),
    UNIQUE (quote_id, code)
);

-- console-gate: audited-table equipment_cost_ledger
CREATE TABLE equipment_cost_ledger (
    id                   UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    branch_id            UUID        NOT NULL REFERENCES branches(id) ON DELETE RESTRICT,
    equipment_id         UUID        NOT NULL REFERENCES registry_equipment(id) ON DELETE RESTRICT,
    work_order_id        UUID        REFERENCES work_orders(id) ON DELETE SET NULL,
    purchase_request_id  UUID,
    source               TEXT        NOT NULL CHECK (source IN ('MANUAL_ADMIN', 'PURCHASE_EXECUTION')),
    amount_won           BIGINT      NOT NULL CHECK (amount_won > 0),
    memo                 TEXT        NOT NULL CHECK (memo <> ''),
    residual_before_won  BIGINT      NOT NULL,
    residual_after_won   BIGINT      NOT NULL,
    entry_at             TIMESTAMPTZ NOT NULL,
    created_by           UUID        NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    created_at           TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX idx_equipment_cost_ledger_equipment
    ON equipment_cost_ledger (branch_id, equipment_id, entry_at DESC);

-- console-gate: audited-table financial_purchase_requests
CREATE TABLE financial_purchase_requests (
    id                              UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    branch_id                       UUID        NOT NULL REFERENCES branches(id) ON DELETE RESTRICT,
    equipment_id                    UUID        NOT NULL REFERENCES registry_equipment(id) ON DELETE RESTRICT,
    work_order_id                   UUID        REFERENCES work_orders(id) ON DELETE SET NULL,
    statement_evidence_id           UUID        NOT NULL REFERENCES evidence_media(id) ON DELETE RESTRICT,
    vendor_name                     TEXT        NOT NULL CHECK (vendor_name <> ''),
    amount_won                      BIGINT      NOT NULL CHECK (amount_won > 0),
    memo                            TEXT        NOT NULL CHECK (memo <> ''),
    status                          TEXT        NOT NULL CHECK (
        status IN (
            'STATEMENT_ATTACHED', 'REQUEST_SUBMITTED', 'ADMIN_APPROVED',
            'EXECUTIVE_PENDING', 'READY_TO_EXECUTE', 'EXECUTED', 'REJECTED'
        )
    ),
    expenditure_no                  TEXT,
    requested_by                    UUID        NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    submitted_by                    UUID        REFERENCES users(id) ON DELETE RESTRICT,
    admin_approved_by               UUID        REFERENCES users(id) ON DELETE RESTRICT,
    executive_approved_by           UUID        REFERENCES users(id) ON DELETE RESTRICT,
    executed_by                     UUID        REFERENCES users(id) ON DELETE RESTRICT,
    rejected_by                     UUID        REFERENCES users(id) ON DELETE RESTRICT,
    rejection_memo                  TEXT,
    depreciation_method             TEXT        NOT NULL CHECK (
        depreciation_method IN ('STRAIGHT_LINE', 'DECLINING_BALANCE')
    ),
    useful_life_months              INTEGER     NOT NULL CHECK (useful_life_months > 0),
    residual_rate_bps               INTEGER     NOT NULL CHECK (residual_rate_bps BETWEEN 0 AND 10000),
    declining_balance_rate_bps      INTEGER     NOT NULL CHECK (declining_balance_rate_bps BETWEEN 0 AND 10000),
    management_fee_rate_bps         INTEGER     NOT NULL CHECK (management_fee_rate_bps BETWEEN 0 AND 10000),
    profit_rate_bps                 INTEGER     NOT NULL CHECK (profit_rate_bps BETWEEN 0 AND 10000),
    floor_negative_quote_residual   BOOLEAN     NOT NULL,
    executive_threshold_won         BIGINT      NOT NULL CHECK (executive_threshold_won >= 0),
    created_at                      TIMESTAMPTZ NOT NULL,
    updated_at                      TIMESTAMPTZ NOT NULL DEFAULT now()
);

ALTER TABLE equipment_cost_ledger
    ADD CONSTRAINT fk_equipment_cost_ledger_purchase_request
    FOREIGN KEY (purchase_request_id)
    REFERENCES financial_purchase_requests(id)
    ON DELETE SET NULL;

CREATE INDEX idx_financial_purchase_requests_branch_status
    ON financial_purchase_requests (branch_id, status, updated_at DESC);

-- console-gate: audited-table financial_purchase_history
CREATE TABLE financial_purchase_history (
    id                   UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    purchase_request_id  UUID        NOT NULL REFERENCES financial_purchase_requests(id) ON DELETE CASCADE,
    actor                UUID        NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    action               TEXT        NOT NULL CHECK (action <> ''),
    from_status          TEXT,
    to_status            TEXT        NOT NULL,
    memo                 TEXT,
    occurred_at          TIMESTAMPTZ NOT NULL,
    created_at           TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX idx_financial_purchase_history_request
    ON financial_purchase_history (purchase_request_id, occurred_at, created_at);
