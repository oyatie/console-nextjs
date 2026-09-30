-- BE-voucher-gl: general-ledger 전표(voucher) domain.
--
-- A voucher (VC-…) is a balanced set of 차/대 (debit/credit) lines that walks the
-- accounting FSM 기표(DRAFT) → 차대검증(BALANCE_CHECKED) → 승인(APPROVED) →
-- 전기(POSTED) → 역분개(REVERSED). Two hard invariants are enforced here at the
-- DB layer (fail-closed), independent of the use-case gate:
--
--   1. BALANCE GATE — a voucher cannot advance past 차대검증 (i.e. into
--      BALANCE_CHECKED/APPROVED/POSTED) unless Σ debit = Σ credit and the total
--      is > 0. Because a cross-row sum cannot be a column CHECK, it is a BEFORE
--      UPDATE trigger that recomputes the line sums.
--   2. POSTED IMMUTABILITY — once POSTED, the only permitted mutation is the
--      single transition to REVERSED (which stamps the linked contra voucher);
--      every other column change is rejected. Reversal never mutates posted
--      lines — it creates a NEW contra voucher whose lines swap 차↔대 so the pair
--      nets to zero.
--
-- Lines are append-only (INSERT/SELECT only, and only while the parent is DRAFT),
-- so a balanced voucher stays balanced through every later transition.

-- console-gate: audited-table finance_gl_vouchers
CREATE TABLE finance_gl_vouchers (
    id                        UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id                    UUID        NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    branch_id                 UUID        NOT NULL REFERENCES branches(id) ON DELETE RESTRICT,
    voucher_no                TEXT        NOT NULL CHECK (btrim(voucher_no) <> '' AND char_length(voucher_no) <= 40),
    status                    TEXT        NOT NULL CHECK (
        status IN ('DRAFT', 'BALANCE_CHECKED', 'APPROVED', 'POSTED', 'REVERSED')
    ),
    memo                      TEXT        NOT NULL CHECK (char_length(memo) <= 500),
    -- The source 기안/document this voucher was derived from (승인 → 전표 파생
    -- chain). A logical ref (object type + id), no FK — the source may live in any
    -- domain (지출결의/전표/구매). Both present or both null.
    source_object_type        TEXT        CHECK (source_object_type IS NULL OR btrim(source_object_type) <> ''),
    source_object_id          TEXT        CHECK (source_object_id IS NULL OR btrim(source_object_id) <> ''),
    -- Reversal linkage. A contra voucher points at the voucher it reverses; the
    -- reversed original points back at its contra. Both are tenant-local.
    reversal_of_voucher_id    UUID,
    reversed_by_voucher_id    UUID,
    created_by                UUID        NOT NULL,
    posted_at                 TIMESTAMPTZ,
    created_at                TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at                TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (id, org_id),
    UNIQUE (org_id, voucher_no),
    CHECK ((source_object_type IS NULL) = (source_object_id IS NULL)),
    FOREIGN KEY (created_by, org_id) REFERENCES users(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (reversal_of_voucher_id, org_id) REFERENCES finance_gl_vouchers(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (reversed_by_voucher_id, org_id) REFERENCES finance_gl_vouchers(id, org_id) ON DELETE RESTRICT
);
CREATE INDEX idx_finance_gl_vouchers_branch_status
    ON finance_gl_vouchers (org_id, branch_id, status, created_at DESC);
CREATE INDEX idx_finance_gl_vouchers_source
    ON finance_gl_vouchers (org_id, source_object_type, source_object_id);

-- console-gate: audited-table finance_gl_voucher_lines
CREATE TABLE finance_gl_voucher_lines (
    id            UUID     PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id        UUID     NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    voucher_id    UUID     NOT NULL,
    line_no       INTEGER  NOT NULL CHECK (line_no > 0),
    account_code  TEXT     NOT NULL CHECK (btrim(account_code) <> '' AND char_length(account_code) <= 40),
    side          TEXT     NOT NULL CHECK (side IN ('DEBIT', 'CREDIT')),
    amount_won    BIGINT   NOT NULL CHECK (amount_won > 0),
    memo          TEXT     NOT NULL DEFAULT '' CHECK (char_length(memo) <= 500),
    UNIQUE (voucher_id, line_no),
    FOREIGN KEY (voucher_id, org_id) REFERENCES finance_gl_vouchers(id, org_id) ON DELETE RESTRICT
);
CREATE INDEX idx_finance_gl_voucher_lines_voucher
    ON finance_gl_voucher_lines (voucher_id, line_no);
-- Account drill (voucher → source links, filtered by account).
CREATE INDEX idx_finance_gl_voucher_lines_account
    ON finance_gl_voucher_lines (org_id, account_code);

-- ---------------------------------------------------------------------------
-- Balance gate + posted immutability (fail-closed, at the DB layer).
-- ---------------------------------------------------------------------------
CREATE OR REPLACE FUNCTION finance_gl_enforce_voucher_rules()
RETURNS TRIGGER AS $$
DECLARE
    debit_total  BIGINT;
    credit_total BIGINT;
BEGIN
    -- POSTED is terminal except for the single POSTED → REVERSED transition.
    IF OLD.status = 'POSTED' AND NEW.status <> 'REVERSED' THEN
        RAISE EXCEPTION 'finance_gl voucher % is posted and immutable', OLD.id
            USING ERRCODE = 'raise_exception';
    END IF;
    IF OLD.status = 'REVERSED' THEN
        RAISE EXCEPTION 'finance_gl voucher % is reversed and immutable', OLD.id
            USING ERRCODE = 'raise_exception';
    END IF;

    -- Balance gate: any advance past 차대검증 requires balanced 차/대 with a
    -- positive total. Recompute from the (append-only) lines.
    IF NEW.status IN ('BALANCE_CHECKED', 'APPROVED', 'POSTED') THEN
        SELECT
            COALESCE(SUM(amount_won) FILTER (WHERE side = 'DEBIT'), 0),
            COALESCE(SUM(amount_won) FILTER (WHERE side = 'CREDIT'), 0)
        INTO debit_total, credit_total
        FROM finance_gl_voucher_lines
        WHERE voucher_id = NEW.id;

        IF debit_total <> credit_total OR debit_total = 0 THEN
            RAISE EXCEPTION
                'finance_gl voucher % is unbalanced (debit=%, credit=%); cannot advance to %',
                NEW.id, debit_total, credit_total, NEW.status
                USING ERRCODE = 'raise_exception';
        END IF;
    END IF;

    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER trg_finance_gl_vouchers_rules
    BEFORE UPDATE ON finance_gl_vouchers
    FOR EACH ROW EXECUTE FUNCTION finance_gl_enforce_voucher_rules();

-- Lines are append-only and may only be added while the parent is DRAFT, so a
-- voucher's balance is frozen the moment it clears 차대검증.
CREATE OR REPLACE FUNCTION finance_gl_enforce_line_draft_only()
RETURNS TRIGGER AS $$
DECLARE
    parent_status TEXT;
BEGIN
    SELECT status INTO parent_status FROM finance_gl_vouchers WHERE id = NEW.voucher_id;
    IF parent_status IS DISTINCT FROM 'DRAFT' THEN
        RAISE EXCEPTION
            'finance_gl voucher % is not DRAFT (status=%); lines are immutable',
            NEW.voucher_id, parent_status
            USING ERRCODE = 'raise_exception';
    END IF;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER trg_finance_gl_voucher_lines_draft_only
    BEFORE INSERT ON finance_gl_voucher_lines
    FOR EACH ROW EXECUTE FUNCTION finance_gl_enforce_line_draft_only();

-- ---------------------------------------------------------------------------
-- FORCE RLS org_isolation + org-immutable, on both tables.
-- ---------------------------------------------------------------------------
ALTER TABLE finance_gl_vouchers ENABLE ROW LEVEL SECURITY;
ALTER TABLE finance_gl_vouchers FORCE ROW LEVEL SECURITY;
CREATE POLICY org_isolation ON finance_gl_vouchers
    USING (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid)
    WITH CHECK (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid);
CREATE TRIGGER trg_finance_gl_vouchers_org_immutable BEFORE UPDATE ON finance_gl_vouchers
    FOR EACH ROW EXECUTE FUNCTION enforce_org_id_immutable();

ALTER TABLE finance_gl_voucher_lines ENABLE ROW LEVEL SECURITY;
ALTER TABLE finance_gl_voucher_lines FORCE ROW LEVEL SECURITY;
CREATE POLICY org_isolation ON finance_gl_voucher_lines
    USING (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid)
    WITH CHECK (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid);

-- Vouchers advance state (UPDATE) but are never hard-deleted; lines are strictly
-- append-only (no UPDATE, no DELETE).
GRANT SELECT, INSERT, UPDATE ON finance_gl_vouchers TO console_rt;
REVOKE DELETE ON finance_gl_vouchers FROM console_rt;
GRANT SELECT, INSERT ON finance_gl_voucher_lines TO console_rt;
REVOKE UPDATE, DELETE ON finance_gl_voucher_lines FROM console_rt;
