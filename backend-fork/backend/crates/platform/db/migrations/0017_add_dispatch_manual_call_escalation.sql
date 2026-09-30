-- T2.5 P1 escalation chain manual-call wall-board flag.
--
-- `manual_call_required_at` is the active exception-strip signal. Force-assign
-- clears the signal with `manual_call_cleared_at` while preserving the audit
-- timeline and the original required-at timestamp for review.

ALTER TABLE p1_dispatches
    ADD COLUMN manual_call_required_at TIMESTAMPTZ,
    ADD COLUMN manual_call_cleared_at TIMESTAMPTZ,
    ADD CONSTRAINT p1_dispatches_manual_call_clear_requires_required_at
        CHECK (manual_call_cleared_at IS NULL OR manual_call_required_at IS NOT NULL);

CREATE INDEX idx_p1_dispatches_manual_call_required
    ON p1_dispatches (branch_id, manual_call_required_at DESC)
    WHERE manual_call_required_at IS NOT NULL
      AND manual_call_cleared_at IS NULL;
