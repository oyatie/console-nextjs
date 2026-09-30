-- no-transaction

-- The by-object-kind panel's hot read: all bindings (enabled and disabled) acting on one kind.
-- Keep the index build outside 0121's ALTER TABLE transaction so writes are not blocked.
-- Omit IF NOT EXISTS so reruns fail closed if a failed concurrent build left an
-- INVALID index that needs DROP INDEX CONCURRENTLY.
CREATE INDEX CONCURRENTLY idx_workflow_trigger_bindings_subject_kind
    ON workflow_trigger_bindings (org_id, subject_kind, created_at DESC)
    WHERE subject_kind IS NOT NULL;
