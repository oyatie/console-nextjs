-- Approval decisions are signing-equivalent business events. Store the decision
-- comment on the approval step itself so approval lines, history, and audit
-- views can show who decided, when, and why without scraping audit JSON.
ALTER TABLE work_order_approval_steps
    ADD COLUMN decision_comment TEXT;

ALTER TABLE work_order_approval_steps
    ADD CONSTRAINT work_order_approval_steps_decision_comment_trimmed
        CHECK (
            decision_comment IS NULL
            OR (
                btrim(decision_comment) = decision_comment
                AND char_length(decision_comment) BETWEEN 1 AND 2000
            )
        );
