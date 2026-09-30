-- Statutory-notice vault (개인 수신함 / InboxDoc domain).
--
-- Recipient-scoped legal-document mailbox. Two kinds live here:
--   * `payslip`      — the recipient's own pay statement. Frictionless
--                      self-view; NOT audited and NEVER receipt-gated
--                      (근로기준법 self-access, POLICIES p3).
--   * `legal_notice` — a statutory notice (근로계약/취업규칙/연차촉진/노무수령거부).
--                      Its body is LOCKED until the recipient confirms receipt
--                      with a fresh passkey step-up; that confirmation is the
--                      legal evidence of receipt (열람 = 법적 수령).
--
-- `notice_type` is a validated free-form string (not an enum) so new statutory
-- notice types are added by producers without a migration. Whether a document
-- requires receipt confirmation is fully determined by `kind`
-- (`legal_notice` ⇒ gated, `payslip` ⇒ frictionless), so there is no separate
-- redundant `legal` column to drift.
--
-- Tenant isolation follows the 0099_create_notifications idiom: RLS keyed on
-- `app.current_org`, FORCE-enabled. There is no per-person GUC, so recipient
-- scoping is enforced in application code from the authenticated principal,
-- never from request input. A cross-user read/confirm therefore returns nothing
-- (or NotFound) — deny-by-omission, never another recipient's row.

-- console-gate: audited-table inbox_docs
CREATE TABLE inbox_docs (
    id                 UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id             UUID        NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    recipient_user_id  UUID        NOT NULL,
    kind               TEXT        NOT NULL
        CHECK (kind IN ('payslip', 'legal_notice')),
    -- Statutory subtype for legal notices (근로계약/취업규칙/연차촉진/노무수령거부),
    -- free-form. NULL for payslips.
    notice_type        TEXT
        CHECK (notice_type IS NULL OR char_length(btrim(notice_type)) BETWEEN 1 AND 64),
    title              TEXT        NOT NULL
        CHECK (char_length(btrim(title)) BETWEEN 1 AND 300),
    -- Rendered document payload: legal-notice prose paragraphs, or a payslip
    -- figure breakdown. JSONB so both shapes share one column and evolve
    -- without a migration; the domain validates shape before insert. For a
    -- LOCKED legal notice the REST layer withholds this from the reader until
    -- receipt is confirmed.
    payload            JSONB       NOT NULL DEFAULT '{}'::jsonb
        CHECK (jsonb_typeof(payload) = 'object'),
    -- Statutory basis surfaced in the passkey gate (e.g. "근로기준법 §61"). Optional.
    legal_basis        TEXT
        CHECK (legal_basis IS NULL OR char_length(btrim(legal_basis)) BETWEEN 1 AND 120),
    -- Provenance of the document: the object that produced it (e.g. the AP-
    -- workflow_run for a 연차촉진 notice, or a payroll run). `source_kind` +
    -- `source_id` are free-form so any producing domain can reference its own
    -- object; backs the receipt→approval finalize tie-back (lane D).
    source_kind        TEXT
        CHECK (source_kind IS NULL OR char_length(btrim(source_kind)) BETWEEN 1 AND 64),
    source_id          TEXT
        CHECK (source_id IS NULL OR char_length(btrim(source_id)) BETWEEN 1 AND 200),
    -- Receipt confirmation stamp — the legal evidence. NULL until confirmed.
    -- Set together (both null, or both present).
    confirmed_by       UUID,
    confirmed_at       TIMESTAMPTZ,
    -- Stable key for at-most-once emission from an at-least-once producer.
    dedup_key          TEXT
        CHECK (dedup_key IS NULL OR char_length(btrim(dedup_key)) BETWEEN 1 AND 200),
    created_at         TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (id, org_id),
    FOREIGN KEY (recipient_user_id, org_id) REFERENCES users(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (confirmed_by, org_id) REFERENCES users(id, org_id) ON DELETE RESTRICT,
    -- Receipt stamp is atomic.
    CONSTRAINT inbox_docs_receipt_stamp_atomic
        CHECK ((confirmed_by IS NULL) = (confirmed_at IS NULL)),
    -- Only a legal notice can carry a receipt confirmation; a payslip is
    -- frictionless self-view and is never receipt-confirmed.
    CONSTRAINT inbox_docs_only_legal_confirmed
        CHECK (confirmed_by IS NULL OR kind = 'legal_notice'),
    -- The confirming principal is always the recipient (self-receipt); a legal
    -- receipt cannot be stamped on behalf of another user.
    CONSTRAINT inbox_docs_receipt_is_self
        CHECK (confirmed_by IS NULL OR confirmed_by = recipient_user_id)
);

ALTER TABLE inbox_docs ENABLE ROW LEVEL SECURITY;
ALTER TABLE inbox_docs FORCE ROW LEVEL SECURITY;
CREATE POLICY org_isolation ON inbox_docs
    USING (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid)
    WITH CHECK (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid);

-- Inbox documents are recipient-owned evidence and are never hard-deleted from
-- the runtime role; receipt confirmation is an UPDATE. console_rt gets
-- SELECT/INSERT/UPDATE, no DELETE.
GRANT SELECT, INSERT, UPDATE ON inbox_docs TO console_rt;
REVOKE DELETE ON inbox_docs FROM console_rt;
