-- Workflow Runtime Spine.
--
-- Workflow Studio definitions are not enough for a real enterprise operations
-- platform: published workflows need durable execution state, human waiting
-- tasks, side-effect outbox rows, and lock ownership. These tables form the
-- clean-room Rust runtime substrate for no-code corporate workflows while
-- keeping every run tenant-scoped, idempotent, auditable, and replay-safe.

-- console-gate: audited-table workflow_runs
CREATE TABLE workflow_runs (
    id                 UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id             UUID        NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    definition_id      UUID        NOT NULL,
    definition_version INTEGER     NOT NULL CHECK (definition_version >= 1),
    status             TEXT        NOT NULL DEFAULT 'STARTING'
        CHECK (status IN ('STARTING','RUNNING','WAITING','SUCCEEDED','FAILED','CANCELLED','DEAD_LETTERED')),
    trigger_type       TEXT        NOT NULL
        CHECK (trigger_type IN ('MANUAL','SCHEDULE','OBJECT_EVENT','IMPORT_EVENT','MAIL_EVENT','MESSENGER_EVENT','CALENDAR_EVENT','POLL_EVENT','API')),
    object_type        TEXT        NULL CHECK (object_type IS NULL OR object_type ~ '^[a-z][a-z0-9_]{1,63}$'),
    object_id          UUID        NULL,
    idempotency_key    TEXT        NOT NULL CHECK (char_length(btrim(idempotency_key)) BETWEEN 16 AND 200),
    correlation_id     TEXT        NOT NULL CHECK (char_length(btrim(correlation_id)) BETWEEN 8 AND 200),
    trace_id           TEXT        NULL CHECK (trace_id IS NULL OR char_length(btrim(trace_id)) BETWEEN 8 AND 200),
    input_payload      JSONB       NOT NULL DEFAULT '{}'::jsonb CHECK (jsonb_typeof(input_payload) = 'object'),
    context_payload    JSONB       NOT NULL DEFAULT '{}'::jsonb CHECK (jsonb_typeof(context_payload) = 'object'),
    output_payload     JSONB       NULL CHECK (output_payload IS NULL OR jsonb_typeof(output_payload) = 'object'),
    error_payload      JSONB       NULL CHECK (error_payload IS NULL OR jsonb_typeof(error_payload) = 'object'),
    initiated_by       UUID        NULL,
    started_at         TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at         TIMESTAMPTZ NOT NULL DEFAULT now(),
    completed_at       TIMESTAMPTZ NULL,
    failed_at          TIMESTAMPTZ NULL,
    UNIQUE (id, org_id),
    UNIQUE (org_id, idempotency_key),
    FOREIGN KEY (definition_id, org_id) REFERENCES workflow_definitions(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (definition_id, definition_version) REFERENCES workflow_definition_versions(definition_id, version) ON DELETE RESTRICT,
    FOREIGN KEY (initiated_by, org_id) REFERENCES users(id, org_id) ON DELETE RESTRICT,
    CHECK ((object_type IS NULL AND object_id IS NULL) OR (object_type IS NOT NULL AND object_id IS NOT NULL)),
    CHECK (completed_at IS NULL OR status IN ('SUCCEEDED','CANCELLED')),
    CHECK (failed_at IS NULL OR status IN ('FAILED','DEAD_LETTERED'))
);
CREATE INDEX idx_workflow_runs_definition
    ON workflow_runs (org_id, definition_id, definition_version, started_at DESC);
CREATE INDEX idx_workflow_runs_status
    ON workflow_runs (org_id, status, updated_at DESC);
CREATE INDEX idx_workflow_runs_object
    ON workflow_runs (org_id, object_type, object_id, started_at DESC)
    WHERE object_type IS NOT NULL;

-- console-gate: audited-table workflow_node_runs
CREATE TABLE workflow_node_runs (
    id              UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id          UUID        NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    run_id          UUID        NOT NULL,
    node_key        TEXT        NOT NULL CHECK (node_key ~ '^[a-z][a-z0-9_]*(\.[a-z][a-z0-9_]*)*$'),
    node_type       TEXT        NOT NULL CHECK (node_type ~ '^[a-z][a-z0-9_]{1,63}$'),
    status          TEXT        NOT NULL DEFAULT 'PENDING'
        CHECK (status IN ('PENDING','RUNNING','WAITING','SUCCEEDED','FAILED','SKIPPED','CANCELLED')),
    attempt         INTEGER     NOT NULL DEFAULT 1 CHECK (attempt >= 1),
    idempotency_key TEXT        NOT NULL CHECK (char_length(btrim(idempotency_key)) BETWEEN 16 AND 200),
    input_payload   JSONB       NOT NULL DEFAULT '{}'::jsonb CHECK (jsonb_typeof(input_payload) = 'object'),
    output_payload  JSONB       NULL CHECK (output_payload IS NULL OR jsonb_typeof(output_payload) = 'object'),
    error_payload   JSONB       NULL CHECK (error_payload IS NULL OR jsonb_typeof(error_payload) = 'object'),
    started_at      TIMESTAMPTZ NULL,
    finished_at     TIMESTAMPTZ NULL,
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (id, org_id),
    UNIQUE (org_id, run_id, node_key, attempt),
    UNIQUE (org_id, idempotency_key),
    FOREIGN KEY (run_id, org_id) REFERENCES workflow_runs(id, org_id) ON DELETE RESTRICT,
    CHECK (finished_at IS NULL OR status IN ('SUCCEEDED','FAILED','SKIPPED','CANCELLED'))
);
CREATE INDEX idx_workflow_node_runs_run
    ON workflow_node_runs (org_id, run_id, node_key, attempt);
CREATE INDEX idx_workflow_node_runs_status
    ON workflow_node_runs (org_id, status, updated_at DESC);

-- console-gate: audited-table workflow_waiting_tasks
CREATE TABLE workflow_waiting_tasks (
    id                  UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id              UUID        NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    run_id              UUID        NOT NULL,
    node_run_id         UUID        NULL,
    waiting_key         TEXT        NOT NULL CHECK (char_length(btrim(waiting_key)) BETWEEN 3 AND 160),
    title               TEXT        NOT NULL CHECK (char_length(btrim(title)) BETWEEN 1 AND 160),
    status              TEXT        NOT NULL DEFAULT 'OPEN'
        CHECK (status IN ('OPEN','CLAIMED','APPROVED','REJECTED','CANCELLED','EXPIRED')),
    assignee_user_id    UUID        NULL,
    assignee_role_key   TEXT        NULL CHECK (assignee_role_key IS NULL OR assignee_role_key ~ '^[a-z][a-z0-9_]*(\.[a-z][a-z0-9_]*)*$'),
    required_policy     TEXT        NULL CHECK (required_policy IS NULL OR required_policy ~ '^[a-z][a-z0-9_]*(\.[a-z][a-z0-9_]*)*$'),
    source_object_type  TEXT        NULL CHECK (source_object_type IS NULL OR source_object_type ~ '^[a-z][a-z0-9_]{1,63}$'),
    source_object_id    UUID        NULL,
    form_payload        JSONB       NOT NULL DEFAULT '{}'::jsonb CHECK (jsonb_typeof(form_payload) = 'object'),
    decision_payload    JSONB       NULL CHECK (decision_payload IS NULL OR jsonb_typeof(decision_payload) = 'object'),
    due_at              TIMESTAMPTZ NULL,
    claimed_by          UUID        NULL,
    claimed_at          TIMESTAMPTZ NULL,
    completed_by        UUID        NULL,
    completed_at        TIMESTAMPTZ NULL,
    passkey_assertion_id UUID       NULL,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (id, org_id),
    UNIQUE (org_id, run_id, waiting_key),
    FOREIGN KEY (run_id, org_id) REFERENCES workflow_runs(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (node_run_id, org_id) REFERENCES workflow_node_runs(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (assignee_user_id, org_id) REFERENCES users(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (claimed_by, org_id) REFERENCES users(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (completed_by, org_id) REFERENCES users(id, org_id) ON DELETE RESTRICT,
    CHECK ((assignee_user_id IS NOT NULL)::int + (assignee_role_key IS NOT NULL)::int + (required_policy IS NOT NULL)::int >= 1),
    CHECK ((source_object_type IS NULL AND source_object_id IS NULL) OR (source_object_type IS NOT NULL AND source_object_id IS NOT NULL)),
    CHECK (completed_at IS NULL OR status IN ('APPROVED','REJECTED','CANCELLED','EXPIRED')),
    CHECK (passkey_assertion_id IS NULL OR completed_by IS NOT NULL)
);
CREATE INDEX idx_workflow_waiting_tasks_run
    ON workflow_waiting_tasks (org_id, run_id, status, created_at DESC);
CREATE INDEX idx_workflow_waiting_tasks_user
    ON workflow_waiting_tasks (org_id, assignee_user_id, status, due_at)
    WHERE assignee_user_id IS NOT NULL;
CREATE INDEX idx_workflow_waiting_tasks_role_policy
    ON workflow_waiting_tasks (org_id, assignee_role_key, required_policy, status, due_at);
CREATE INDEX idx_workflow_waiting_tasks_source_object
    ON workflow_waiting_tasks (org_id, source_object_type, source_object_id)
    WHERE source_object_type IS NOT NULL;

-- console-gate: audited-table workflow_outbox_events
CREATE TABLE workflow_outbox_events (
    id                UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id            UUID        NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    run_id            UUID        NOT NULL,
    node_run_id       UUID        NULL,
    channel           TEXT        NOT NULL
        CHECK (channel IN ('NOTIFICATION','MAIL','MESSENGER','CALENDAR','POLL','AUDIT','OBJECT_EVENT','WEBHOOK','JOB')),
    destination_ref   TEXT        NULL CHECK (destination_ref IS NULL OR char_length(btrim(destination_ref)) BETWEEN 1 AND 300),
    idempotency_key   TEXT        NOT NULL CHECK (char_length(btrim(idempotency_key)) BETWEEN 16 AND 200),
    status            TEXT        NOT NULL DEFAULT 'PENDING'
        CHECK (status IN ('PENDING','IN_PROGRESS','DELIVERED','FAILED','DEAD_LETTERED','CANCELLED')),
    payload           JSONB       NOT NULL DEFAULT '{}'::jsonb CHECK (jsonb_typeof(payload) = 'object'),
    error_payload     JSONB       NULL CHECK (error_payload IS NULL OR jsonb_typeof(error_payload) = 'object'),
    attempt_count     INTEGER     NOT NULL DEFAULT 0 CHECK (attempt_count >= 0),
    next_attempt_at   TIMESTAMPTZ NULL,
    locked_by         TEXT        NULL CHECK (locked_by IS NULL OR char_length(btrim(locked_by)) BETWEEN 1 AND 160),
    locked_until      TIMESTAMPTZ NULL,
    delivered_at      TIMESTAMPTZ NULL,
    dead_lettered_at  TIMESTAMPTZ NULL,
    created_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (id, org_id),
    UNIQUE (org_id, idempotency_key),
    FOREIGN KEY (run_id, org_id) REFERENCES workflow_runs(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (node_run_id, org_id) REFERENCES workflow_node_runs(id, org_id) ON DELETE RESTRICT,
    CHECK (delivered_at IS NULL OR status = 'DELIVERED'),
    CHECK (dead_lettered_at IS NULL OR status = 'DEAD_LETTERED')
);
CREATE INDEX idx_workflow_outbox_events_pending
    ON workflow_outbox_events (org_id, status, next_attempt_at, created_at)
    WHERE status IN ('PENDING','FAILED');
CREATE INDEX idx_workflow_outbox_events_run
    ON workflow_outbox_events (org_id, run_id, created_at DESC);
CREATE INDEX idx_workflow_outbox_events_channel
    ON workflow_outbox_events (org_id, channel, status, created_at DESC);

-- console-gate: audited-table workflow_execution_locks
CREATE TABLE workflow_execution_locks (
    id          UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id      UUID        NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    lock_key    TEXT        NOT NULL CHECK (char_length(btrim(lock_key)) BETWEEN 8 AND 240),
    run_id      UUID        NULL,
    acquired_by TEXT        NOT NULL CHECK (char_length(btrim(acquired_by)) BETWEEN 1 AND 160),
    acquired_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at  TIMESTAMPTZ NOT NULL,
    heartbeat_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (id, org_id),
    UNIQUE (org_id, lock_key),
    FOREIGN KEY (run_id, org_id) REFERENCES workflow_runs(id, org_id) ON DELETE RESTRICT,
    CHECK (expires_at > acquired_at)
);
CREATE INDEX idx_workflow_execution_locks_expiry
    ON workflow_execution_locks (org_id, expires_at);

CREATE OR REPLACE FUNCTION workflow_runtime_no_delete()
RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION 'workflow runtime table % is durable: DELETE is forbidden (row id=%)', TG_TABLE_NAME, OLD.id
        USING ERRCODE = '55000';
END;
$$;

CREATE OR REPLACE FUNCTION workflow_runtime_org_immutable()
RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF OLD.org_id <> NEW.org_id THEN
        RAISE EXCEPTION 'workflow runtime table % forbids org_id changes', TG_TABLE_NAME
            USING ERRCODE = '23514';
    END IF;
    RETURN NEW;
END;
$$;

ALTER TABLE workflow_runs ENABLE ROW LEVEL SECURITY;
ALTER TABLE workflow_runs FORCE ROW LEVEL SECURITY;
CREATE POLICY org_isolation ON workflow_runs
    USING (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid)
    WITH CHECK (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid);

ALTER TABLE workflow_node_runs ENABLE ROW LEVEL SECURITY;
ALTER TABLE workflow_node_runs FORCE ROW LEVEL SECURITY;
CREATE POLICY org_isolation ON workflow_node_runs
    USING (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid)
    WITH CHECK (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid);

ALTER TABLE workflow_waiting_tasks ENABLE ROW LEVEL SECURITY;
ALTER TABLE workflow_waiting_tasks FORCE ROW LEVEL SECURITY;
CREATE POLICY org_isolation ON workflow_waiting_tasks
    USING (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid)
    WITH CHECK (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid);

ALTER TABLE workflow_outbox_events ENABLE ROW LEVEL SECURITY;
ALTER TABLE workflow_outbox_events FORCE ROW LEVEL SECURITY;
CREATE POLICY org_isolation ON workflow_outbox_events
    USING (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid)
    WITH CHECK (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid);

ALTER TABLE workflow_execution_locks ENABLE ROW LEVEL SECURITY;
ALTER TABLE workflow_execution_locks FORCE ROW LEVEL SECURITY;
CREATE POLICY org_isolation ON workflow_execution_locks
    USING (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid)
    WITH CHECK (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid);

GRANT SELECT, INSERT, UPDATE ON workflow_runs TO console_rt;
GRANT SELECT, INSERT, UPDATE ON workflow_node_runs TO console_rt;
GRANT SELECT, INSERT, UPDATE ON workflow_waiting_tasks TO console_rt;
GRANT SELECT, INSERT, UPDATE ON workflow_outbox_events TO console_rt;
GRANT SELECT, INSERT, UPDATE ON workflow_execution_locks TO console_rt;

CREATE TRIGGER trg_workflow_runs_org_immutable
    BEFORE UPDATE ON workflow_runs
    FOR EACH ROW EXECUTE FUNCTION workflow_runtime_org_immutable();
CREATE TRIGGER trg_workflow_runs_no_delete
    BEFORE DELETE ON workflow_runs
    FOR EACH ROW EXECUTE FUNCTION workflow_runtime_no_delete();

CREATE TRIGGER trg_workflow_node_runs_org_immutable
    BEFORE UPDATE ON workflow_node_runs
    FOR EACH ROW EXECUTE FUNCTION workflow_runtime_org_immutable();
CREATE TRIGGER trg_workflow_node_runs_no_delete
    BEFORE DELETE ON workflow_node_runs
    FOR EACH ROW EXECUTE FUNCTION workflow_runtime_no_delete();

CREATE TRIGGER trg_workflow_waiting_tasks_org_immutable
    BEFORE UPDATE ON workflow_waiting_tasks
    FOR EACH ROW EXECUTE FUNCTION workflow_runtime_org_immutable();
CREATE TRIGGER trg_workflow_waiting_tasks_no_delete
    BEFORE DELETE ON workflow_waiting_tasks
    FOR EACH ROW EXECUTE FUNCTION workflow_runtime_no_delete();

CREATE TRIGGER trg_workflow_outbox_events_org_immutable
    BEFORE UPDATE ON workflow_outbox_events
    FOR EACH ROW EXECUTE FUNCTION workflow_runtime_org_immutable();
CREATE TRIGGER trg_workflow_outbox_events_no_delete
    BEFORE DELETE ON workflow_outbox_events
    FOR EACH ROW EXECUTE FUNCTION workflow_runtime_no_delete();

CREATE TRIGGER trg_workflow_execution_locks_org_immutable
    BEFORE UPDATE ON workflow_execution_locks
    FOR EACH ROW EXECUTE FUNCTION workflow_runtime_org_immutable();
CREATE TRIGGER trg_workflow_execution_locks_no_delete
    BEFORE DELETE ON workflow_execution_locks
    FOR EACH ROW EXECUTE FUNCTION workflow_runtime_no_delete();
