-- Workflow schedules (BE-AUTO slice 1, closes adequacy-audit gap 9).
--
-- Recurring cron schedules that start workflow runs. The platform previously
-- had NO recurring substrate at all (platform/jobs is one-shot delayed only;
-- the SCHEDULE TriggerType was a reserved CHECK value never produced). A
-- background poller (backend/app workflow_schedules, mirroring the
-- workflow_drain loop) finds due rows (enabled AND next_run_at <= now),
-- starts one run per fire with a deterministic idempotency key
-- ('schedule:{id}:{fire_unix}' — the 0077 UNIQUE(org_id, idempotency_key)
-- run-spine guard makes a concurrent double-poll start exactly one run), then
-- advances next_run_at guarded on the fire it claimed.
--
-- cron_expr is a standard cron pattern evaluated in `timezone` (IANA name,
-- default Asia/Seoul — a Korean operations platform's "매일 아침 9시" must
-- mean 09:00 KST, not UTC).

-- console-gate: audited-table workflow_schedules
CREATE TABLE workflow_schedules (
    id            UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id        UUID        NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    label         TEXT        NOT NULL CHECK (char_length(btrim(label)) BETWEEN 1 AND 120),
    cron_expr     TEXT        NOT NULL CHECK (char_length(btrim(cron_expr)) BETWEEN 5 AND 120),
    timezone      TEXT        NOT NULL DEFAULT 'Asia/Seoul'
        CHECK (char_length(btrim(timezone)) BETWEEN 1 AND 64),
    definition_id UUID        NOT NULL,
    enabled       BOOLEAN     NOT NULL DEFAULT TRUE,
    next_run_at   TIMESTAMPTZ NULL,
    last_run_at   TIMESTAMPTZ NULL,
    last_status   TEXT        NULL CHECK (last_status IN ('STARTED','SKIPPED','FAILED')),
    created_by    UUID        NOT NULL,
    updated_by    UUID        NOT NULL,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (id, org_id),
    FOREIGN KEY (definition_id, org_id) REFERENCES workflow_definitions(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (created_by, org_id) REFERENCES users(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (updated_by, org_id) REFERENCES users(id, org_id) ON DELETE RESTRICT
);

-- The poller's hot read: due schedules.
CREATE INDEX idx_workflow_schedules_due
    ON workflow_schedules (org_id, next_run_at)
    WHERE enabled AND next_run_at IS NOT NULL;

-- Durable schedule objects: disable instead of delete (started runs FK back to
-- the schedule for run history). Reuses the 0077 runtime guard functions.
CREATE TRIGGER workflow_schedules_no_delete
    BEFORE DELETE ON workflow_schedules
    FOR EACH ROW EXECUTE FUNCTION workflow_runtime_no_delete();
CREATE TRIGGER workflow_schedules_org_immutable
    BEFORE UPDATE ON workflow_schedules
    FOR EACH ROW EXECUTE FUNCTION workflow_runtime_org_immutable();

ALTER TABLE workflow_schedules ENABLE ROW LEVEL SECURITY;
ALTER TABLE workflow_schedules FORCE ROW LEVEL SECURITY;
CREATE POLICY org_isolation ON workflow_schedules
    USING (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid)
    WITH CHECK (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid);

GRANT SELECT, INSERT, UPDATE ON workflow_schedules TO console_rt;

-- Additive provenance column: which schedule started a run (NULL for every
-- non-scheduled run). Backs the per-schedule run-history REST surface.
ALTER TABLE workflow_runs ADD COLUMN schedule_id UUID NULL;
ALTER TABLE workflow_runs ADD CONSTRAINT workflow_runs_schedule_fk
    FOREIGN KEY (schedule_id, org_id) REFERENCES workflow_schedules(id, org_id) ON DELETE RESTRICT
    NOT VALID;
