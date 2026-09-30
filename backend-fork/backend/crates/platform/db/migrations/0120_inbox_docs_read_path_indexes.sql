-- Access-path indexes for the inbox_docs read filters and idempotent emission.
--
-- Split from the table (0119) so the correctness schema and the query-tuning
-- indexes review independently.

-- At-most-once emission: a producer supplying a dedup_key can safely re-drain
-- (leave-promotion lane D, payslip backfill). Redelivery of the same source
-- event is a no-op via this partial unique index.
CREATE UNIQUE INDEX idx_inbox_docs_dedup
    ON inbox_docs (org_id, recipient_user_id, dedup_key)
    WHERE dedup_key IS NOT NULL;

-- Backs the recipient inbox list (전체 filter), newest-first, keyset-paginated
-- on (created_at, id).
CREATE INDEX idx_inbox_docs_recipient_created
    ON inbox_docs (org_id, recipient_user_id, created_at DESC, id DESC);

-- Backs the "확인 필요" (action-required) filter: unconfirmed legal notices for
-- the recipient. Partial so the sidebar badge count stays a cheap index scan.
CREATE INDEX idx_inbox_docs_recipient_unconfirmed_legal
    ON inbox_docs (org_id, recipient_user_id, created_at DESC, id DESC)
    WHERE kind = 'legal_notice' AND confirmed_at IS NULL;
