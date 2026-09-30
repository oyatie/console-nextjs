-- The purchase-request console queue is always tenant- and branch-scoped,
-- ordered by most recently updated request with a UUID tie-breaker. This index
-- serves the bounded queue page under FORCE RLS without changing lifecycle
-- semantics or widening access.
CREATE INDEX idx_financial_purchase_requests_queue_page
    ON financial_purchase_requests (org_id, branch_id, updated_at DESC, id DESC);
