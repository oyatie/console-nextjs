-- no-transaction
-- BE-OBJ audit API trace-id filter support.
-- The route compares `trace_id::text = $1` because the column is CHAR(32), so
-- the index uses the same expression and audit timeline cursor order.
-- Keep one CONCURRENTLY statement per no-transaction migration; omit
-- IF NOT EXISTS so failed INVALID indexes fail closed and require explicit
-- DROP INDEX CONCURRENTLY repair.

CREATE INDEX CONCURRENTLY idx_audit_events_trace_id_text_time
    ON audit_events ((trace_id::text), occurred_at DESC, id DESC);
