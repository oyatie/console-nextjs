-- no-transaction
-- BE-OBJ audit API target-id-only filter support.
-- `idx_audit_events_target` still covers the narrower target_type+target_id
-- lookup; GET /api/audit also allows target_id without target_type and orders by
-- the audit timeline cursor.
-- Keep one CONCURRENTLY statement per no-transaction migration; omit
-- IF NOT EXISTS so failed INVALID indexes fail closed and require explicit
-- DROP INDEX CONCURRENTLY repair.

CREATE INDEX CONCURRENTLY idx_audit_events_target_id_time
    ON audit_events (target_id, occurred_at DESC, id DESC);
