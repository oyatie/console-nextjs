-- §61 statutory leave push (연차 사용 촉진 1차/2차 + 노무수령거부).
--
-- Each row records one statutory push served to a target employee. Under
-- 근로기준법 §61 the employer promotes unused annual leave in two rounds
-- (round 1 = 사용 촉구, round 2 = 시기 지정); after round 2 the employer may serve
-- a 노무수령거부 notice to decline the labor.
--
-- The concrete legal delivery is the receipt-gated document placed in the
-- target's 개인 수신함 (inbox_docs, 0118): `inbox_doc_id` references it. Starting
-- the engine AP- run is gated on the 연차촉진 submittable definition (gap #1);
-- until it exists `ap_run_id` stays NULL and the push carries an honest
-- `pending_engine_definition` status — a run is never fabricated.
--
-- Depends on 0118_create_inbox_docs (the inbox vault) — this migration, and the
-- PR that ships it, must land AFTER the InboxDoc PR.
--
-- Tenant isolation: RLS keyed on `app.current_org`, FORCE-enabled (0118 idiom).

-- console-gate: audited-table leave_promotions
CREATE TABLE leave_promotions (
    id                  UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id              UUID        NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    branch_id           UUID        NOT NULL,
    -- The account that receives the receipt-gated notice.
    target_user_id      UUID        NOT NULL,
    -- The employee whose unused leave motivates the push.
    target_employee_id  UUID        NOT NULL,
    kind                TEXT        NOT NULL
        CHECK (kind IN ('promotion', 'refusal')),
    -- §61 round: 1 or 2 for a promotion; a refusal follows round 2.
    round               SMALLINT    NOT NULL
        CHECK (round IN (1, 2)),
    -- The delivered receipt-gated notice (open in the target's 개인 수신함).
    inbox_doc_id        UUID        NOT NULL,
    -- The engine AP- run, when the submittable definition exists (gap #1).
    ap_run_id           UUID,
    legal_basis         TEXT,
    created_by          UUID        NOT NULL,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (id, org_id),
    -- At-most-once per (target, kind, round): round 1, round 2, and a refusal
    -- are each servable once; a duplicate push is an idempotent no-op.
    UNIQUE (org_id, target_employee_id, kind, round),
    FOREIGN KEY (branch_id, org_id) REFERENCES branches (id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (target_user_id, org_id) REFERENCES users (id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (target_employee_id, org_id) REFERENCES employees (id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (inbox_doc_id, org_id) REFERENCES inbox_docs (id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (created_by, org_id) REFERENCES users (id, org_id) ON DELETE RESTRICT,
    -- A refusal is only ever served after a round-2 promotion.
    CONSTRAINT leave_promotions_refusal_after_round2
        CHECK (kind <> 'refusal' OR round = 2)
);

ALTER TABLE leave_promotions ENABLE ROW LEVEL SECURITY;
ALTER TABLE leave_promotions FORCE ROW LEVEL SECURITY;
CREATE POLICY org_isolation ON leave_promotions
    USING (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid)
    WITH CHECK (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid);

-- Statutory notices are legal evidence; the runtime role never hard-deletes.
GRANT SELECT, INSERT, UPDATE ON leave_promotions TO console_rt;
REVOKE DELETE ON leave_promotions FROM console_rt;
