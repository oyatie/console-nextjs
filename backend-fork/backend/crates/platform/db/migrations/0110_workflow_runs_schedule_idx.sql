-- no-transaction

-- Keep one CONCURRENTLY statement per no-transaction migration; omit
-- IF NOT EXISTS so a failed concurrent build leaves CI/deploy fail-closed.
CREATE INDEX CONCURRENTLY idx_workflow_runs_schedule
    ON workflow_runs (org_id, schedule_id, started_at DESC)
    WHERE schedule_id IS NOT NULL;
