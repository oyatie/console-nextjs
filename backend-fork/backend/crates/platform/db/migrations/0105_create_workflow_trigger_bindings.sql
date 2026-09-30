-- Workflow trigger bindings (BE-AUTO slice 1, closes adequacy-audit gap 8).
--
-- "When domain event X happens, start workflow Y." Until now the ONLY producer
-- of a non-manual workflow run was the hardcoded work-order-completion inline
-- start (workorder rest m2_strangler) — no rule table, no evaluation. This
-- table is the rule substrate: an org-scoped binding from a registered domain
-- event key (e.g. 'work_order.completed') to a workflow definition. At the
-- audited-mutation commit points the event dispatcher loads the ENABLED
-- bindings for the event and starts one idempotent run per binding through the
-- existing start_run path (trigger_type = the binding's reserved TriggerType
-- value from the 0077 run-spine CHECK list).
--
-- MANUAL / SCHEDULE / API are deliberately NOT bindable trigger types: manual
-- and API starts go through POST /api/v1/workflow-runs, schedules through
-- workflow_schedules (0106). Bindings cover the event-shaped sources only.

-- console-gate: audited-table workflow_trigger_bindings
CREATE TABLE workflow_trigger_bindings (
    id            UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id        UUID        NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    definition_id UUID        NOT NULL,
    trigger_type  TEXT        NOT NULL
        CHECK (trigger_type IN ('OBJECT_EVENT','IMPORT_EVENT','MAIL_EVENT','MESSENGER_EVENT','CALENDAR_EVENT','POLL_EVENT')),
    event_key     TEXT        NOT NULL
        CHECK (event_key ~ '^[a-z][a-z0-9_]*\.[a-z][a-z0-9_]*$'),
    enabled       BOOLEAN     NOT NULL DEFAULT TRUE,
    created_by    UUID        NOT NULL,
    updated_by    UUID        NOT NULL,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (id, org_id),
    -- One binding per (definition, event): re-binding the same pair is an
    -- update (enable/disable), never a duplicate rule that double-fires.
    UNIQUE (org_id, definition_id, event_key),
    FOREIGN KEY (definition_id, org_id) REFERENCES workflow_definitions(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (created_by, org_id) REFERENCES users(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (updated_by, org_id) REFERENCES users(id, org_id) ON DELETE RESTRICT
);

-- The dispatcher's hot read: enabled bindings for one event key.
CREATE INDEX idx_workflow_trigger_bindings_event
    ON workflow_trigger_bindings (org_id, event_key)
    WHERE enabled;

-- Durable rule objects: disable instead of delete (run provenance/audit trail
-- must stay dereferenceable). Reuses the 0077 runtime guard functions.
CREATE TRIGGER workflow_trigger_bindings_no_delete
    BEFORE DELETE ON workflow_trigger_bindings
    FOR EACH ROW EXECUTE FUNCTION workflow_runtime_no_delete();
CREATE TRIGGER workflow_trigger_bindings_org_immutable
    BEFORE UPDATE ON workflow_trigger_bindings
    FOR EACH ROW EXECUTE FUNCTION workflow_runtime_org_immutable();

ALTER TABLE workflow_trigger_bindings ENABLE ROW LEVEL SECURITY;
ALTER TABLE workflow_trigger_bindings FORCE ROW LEVEL SECURITY;
CREATE POLICY org_isolation ON workflow_trigger_bindings
    USING (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid)
    WITH CHECK (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid);

GRANT SELECT, INSERT, UPDATE ON workflow_trigger_bindings TO console_rt;
