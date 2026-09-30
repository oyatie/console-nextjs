-- no-transaction
-- Cedar parity report scans newest audit_events rows by action.
-- Keep one CONCURRENTLY statement per no-transaction migration.

CREATE INDEX CONCURRENTLY idx_audit_events_action_created_at
    ON audit_events (action, created_at DESC);
