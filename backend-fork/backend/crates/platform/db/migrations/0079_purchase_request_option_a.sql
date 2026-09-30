-- Purchase request Option A: normalized lines, quote attachments, AP expense execution,
-- regular-price anomaly gates, and DB-backed personal presentation preferences.
-- Existing rows remain readable as LEGACY_MANUAL single-amount requests; no data
-- backfill is performed in this migration.

ALTER TABLE financial_purchase_requests
    ALTER COLUMN equipment_id DROP NOT NULL,
    ALTER COLUMN statement_evidence_id DROP NOT NULL;

ALTER TABLE financial_purchase_requests
    ADD COLUMN purchase_type TEXT NOT NULL DEFAULT 'LEGACY_MANUAL'
        CHECK (purchase_type IN ('REGULAR', 'ONE_OFF', 'OTHER', 'LEGACY_MANUAL')),
    ADD COLUMN price_anomaly BOOLEAN NOT NULL DEFAULT false,
    ADD COLUMN quote_update_required BOOLEAN NOT NULL DEFAULT false;

CREATE INDEX idx_financial_purchase_requests_quote_gate
    ON financial_purchase_requests (org_id, branch_id, quote_update_required, status)
    WHERE quote_update_required;

CREATE TABLE financial_purchase_request_lines (
    id                         UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    purchase_request_id        UUID        NOT NULL REFERENCES financial_purchase_requests(id) ON DELETE CASCADE,
    line_no                    INTEGER     NOT NULL CHECK (line_no > 0),
    item                       TEXT        NOT NULL CHECK (btrim(item) <> ''),
    quantity                   INTEGER     NOT NULL CHECK (quantity > 0),
    unit_supply_price_won      BIGINT      NOT NULL CHECK (unit_supply_price_won >= 0),
    vat_won                    BIGINT      NOT NULL CHECK (vat_won >= 0),
    vat_overridden             BOOLEAN     NOT NULL DEFAULT false,
    line_total_won             BIGINT      NOT NULL CHECK (line_total_won > 0),
    created_at                 TIMESTAMPTZ NOT NULL DEFAULT now(),
    org_id                     UUID        NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    UNIQUE (purchase_request_id, line_no),
    CONSTRAINT financial_purchase_request_lines_request_same_org_fk
        FOREIGN KEY (purchase_request_id, org_id)
        REFERENCES financial_purchase_requests(id, org_id) ON DELETE CASCADE
);

CREATE INDEX idx_financial_purchase_request_lines_org
    ON financial_purchase_request_lines (org_id, purchase_request_id, line_no);

CREATE TABLE financial_purchase_attachments (
    id                    UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    branch_id             UUID        NOT NULL REFERENCES branches(id) ON DELETE RESTRICT,
    purchase_request_id   UUID        REFERENCES financial_purchase_requests(id) ON DELETE SET NULL,
    uploaded_by           UUID        NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    role                  TEXT        NOT NULL CHECK (role IN ('QUOTE', 'INVOICE', 'OTHER')),
    file_name             TEXT        NOT NULL CHECK (btrim(file_name) <> ''),
    content_type          TEXT        NOT NULL CHECK (btrim(content_type) <> ''),
    size_bytes            BIGINT      NOT NULL CHECK (size_bytes > 0),
    s3_bucket             TEXT        NOT NULL CHECK (btrim(s3_bucket) <> ''),
    s3_key                TEXT        NOT NULL CHECK (btrim(s3_key) <> ''),
    checksum_sha256       TEXT,
    upload_state          TEXT        NOT NULL DEFAULT 'PENDING'
        CHECK (upload_state IN ('PENDING', 'CONFIRMED', 'FAILED')),
    created_at            TIMESTAMPTZ NOT NULL DEFAULT now(),
    org_id                UUID        NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    CONSTRAINT financial_purchase_attachments_branch_same_org_fk
        FOREIGN KEY (branch_id, org_id) REFERENCES branches(id, org_id) ON DELETE RESTRICT,
    CONSTRAINT financial_purchase_attachments_request_same_org_fk
        FOREIGN KEY (purchase_request_id, org_id)
        REFERENCES financial_purchase_requests(id, org_id) ON DELETE SET NULL,
    CONSTRAINT financial_purchase_attachments_user_same_org_fk
        FOREIGN KEY (uploaded_by, org_id) REFERENCES users(id, org_id) ON DELETE RESTRICT,
    CONSTRAINT financial_purchase_attachments_id_org_key UNIQUE (id, org_id)
);

CREATE INDEX idx_financial_purchase_attachments_org_request
    ON financial_purchase_attachments (org_id, purchase_request_id, created_at DESC);
CREATE INDEX idx_financial_purchase_attachments_org_branch
    ON financial_purchase_attachments (org_id, branch_id, created_at DESC);

CREATE TABLE financial_regular_purchase_prices (
    id                                  UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    branch_id                           UUID        NOT NULL REFERENCES branches(id) ON DELETE RESTRICT,
    vendor_name_norm                    TEXT        NOT NULL CHECK (vendor_name_norm <> ''),
    item_norm                           TEXT        NOT NULL CHECK (item_norm <> ''),
    last_unit_supply_price_won          BIGINT      NOT NULL CHECK (last_unit_supply_price_won >= 0),
    quote_attachment_id                 UUID        REFERENCES financial_purchase_attachments(id) ON DELETE SET NULL,
    updated_from_purchase_request_id    UUID        REFERENCES financial_purchase_requests(id) ON DELETE SET NULL,
    updated_at                          TIMESTAMPTZ NOT NULL DEFAULT now(),
    org_id                              UUID        NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    UNIQUE (org_id, branch_id, vendor_name_norm, item_norm),
    CONSTRAINT financial_regular_prices_branch_same_org_fk
        FOREIGN KEY (branch_id, org_id) REFERENCES branches(id, org_id) ON DELETE RESTRICT,
    CONSTRAINT financial_regular_prices_quote_same_org_fk
        FOREIGN KEY (quote_attachment_id, org_id) REFERENCES financial_purchase_attachments(id, org_id) ON DELETE SET NULL,
    CONSTRAINT financial_regular_prices_request_same_org_fk
        FOREIGN KEY (updated_from_purchase_request_id, org_id)
        REFERENCES financial_purchase_requests(id, org_id) ON DELETE SET NULL
);

CREATE INDEX idx_financial_regular_purchase_prices_org_branch
    ON financial_regular_purchase_prices (org_id, branch_id, vendor_name_norm, item_norm);

CREATE TABLE financial_expense_ledger (
    id                   UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    branch_id            UUID        NOT NULL REFERENCES branches(id) ON DELETE RESTRICT,
    purchase_request_id  UUID        NOT NULL UNIQUE REFERENCES financial_purchase_requests(id) ON DELETE RESTRICT,
    vendor_name          TEXT        NOT NULL CHECK (btrim(vendor_name) <> ''),
    amount_won           BIGINT      NOT NULL CHECK (amount_won > 0),
    memo                 TEXT        NOT NULL CHECK (btrim(memo) <> ''),
    expenditure_no       TEXT,
    executed_by          UUID        NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    executed_at          TIMESTAMPTZ NOT NULL,
    created_at           TIMESTAMPTZ NOT NULL DEFAULT now(),
    org_id               UUID        NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    CONSTRAINT financial_expense_ledger_branch_same_org_fk
        FOREIGN KEY (branch_id, org_id) REFERENCES branches(id, org_id) ON DELETE RESTRICT,
    CONSTRAINT financial_expense_ledger_request_same_org_fk
        FOREIGN KEY (purchase_request_id, org_id)
        REFERENCES financial_purchase_requests(id, org_id) ON DELETE RESTRICT,
    CONSTRAINT financial_expense_ledger_user_same_org_fk
        FOREIGN KEY (executed_by, org_id) REFERENCES users(id, org_id) ON DELETE RESTRICT
);

CREATE INDEX idx_financial_expense_ledger_org_branch
    ON financial_expense_ledger (org_id, branch_id, executed_at DESC);

CREATE TABLE user_feature_preferences (
    user_id          UUID        NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    feature_key      TEXT        NOT NULL CHECK (feature_key <> ''),
    preferences_json JSONB       NOT NULL DEFAULT '{}'::jsonb,
    schema_version   INTEGER     NOT NULL CHECK (schema_version > 0),
    created_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    org_id           UUID        NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    PRIMARY KEY (org_id, user_id, feature_key),
    CONSTRAINT user_feature_preferences_user_same_org_fk
        FOREIGN KEY (user_id, org_id) REFERENCES users(id, org_id) ON DELETE CASCADE
);

CREATE INDEX idx_user_feature_preferences_org_user
    ON user_feature_preferences (org_id, user_id, feature_key);

DO $$
DECLARE
    tenant_tables TEXT[] := ARRAY[
        'financial_purchase_request_lines',
        'financial_purchase_attachments',
        'financial_regular_purchase_prices',
        'financial_expense_ledger',
        'user_feature_preferences'
    ];
    t TEXT;
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
        EXECUTE format('GRANT SELECT, INSERT, UPDATE, DELETE ON %I TO console_rt', t);
        EXECUTE format(
            'CREATE TRIGGER %I BEFORE UPDATE ON %I '
            || 'FOR EACH ROW EXECUTE FUNCTION enforce_org_id_immutable()',
            'trg_' || t || '_org_immutable', t
        );
    END LOOP;
END
$$;
