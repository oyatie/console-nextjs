-- BE-voucher-sod: separation-of-duties on general-ledger 전표(voucher) approval.
--
-- Migration 0160 shipped the voucher FSM with NO segregation of duties: a single
-- user could drive 기표(DRAFT) → 차대검증 → 승인 → 전기 → 역분개 end-to-end, self-
-- approving their own vouchers. This adds the missing approver identity and the
-- hard invariant, fail-closed at the DB layer (independent of the use-case gate):
--
--   SoD — a voucher may only reach 승인(APPROVED)/전기(POSTED)/역분개(REVERSED)
--   once a DISTINCT principal has approved it: `approved_by` is recorded and must
--   differ from `created_by` (the 기표자). `approved_by` is write-once — set
--   exactly at 승인 and immutable thereafter (a reversal carries the original's
--   approver forward, so the contra also satisfies the invariant).
--
-- Mirrors the existing four-eyes idiom (gov_approvals `approver_id <> requested_by`
-- CHECK + store gate): store enforcement returns a clean 403, the DB CHECK is the
-- backstop against a raw write.

ALTER TABLE finance_gl_vouchers
    ADD COLUMN approved_by UUID,
    -- Same tenant-coupled composite FK style as created_by: an approver is a real
    -- user in the SAME tenant.
    ADD FOREIGN KEY (approved_by, org_id) REFERENCES users (id, org_id) ON DELETE RESTRICT,
    -- SoD invariant: any voucher past 차대검증 carries a distinct approver. DRAFT and
    -- BALANCE_CHECKED are exempt (no approver yet); the app sets approved_by at 승인.
    ADD CONSTRAINT finance_gl_vouchers_sod CHECK (
        status NOT IN ('APPROVED', 'POSTED', 'REVERSED')
        OR (approved_by IS NOT NULL AND approved_by <> created_by)
    );

-- Extend the existing rules trigger (balance gate + posted/reversed immutability)
-- with approved_by write-once immutability. Every other clause is byte-for-byte
-- the 0160 body so this replacement is additive.
CREATE OR REPLACE FUNCTION finance_gl_enforce_voucher_rules()
RETURNS TRIGGER AS $$
DECLARE
    debit_total  BIGINT;
    credit_total BIGINT;
BEGIN
    -- The approver is recorded once at 승인 and never rewritten. Clearing it or
    -- pointing it at a different principal after the fact is rejected.
    IF OLD.approved_by IS NOT NULL AND NEW.approved_by IS DISTINCT FROM OLD.approved_by THEN
        RAISE EXCEPTION 'finance_gl voucher % approver is immutable once recorded', OLD.id
            USING ERRCODE = 'raise_exception';
    END IF;

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
