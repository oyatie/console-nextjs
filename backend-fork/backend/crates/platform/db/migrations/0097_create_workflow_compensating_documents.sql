-- Workflow compensating documents.
--
-- Post-finalization rejection is not a workflow_run transition: terminal runs
-- stay terminal. This table records the compensating AP/document linked to the
-- original finalized run, with the same tenant isolation and durability posture
-- as the workflow runtime spine.

-- console-gate: audited-table workflow_compensating_documents
CREATE TABLE workflow_compensating_documents (
    id                 UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id             UUID        NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    original_run_id    UUID        NOT NULL,
    compensation_type  TEXT        NOT NULL
        CHECK (compensation_type IN ('POST_FINALIZATION_REJECTION')),
    status             TEXT        NOT NULL DEFAULT 'CREATED'
        CHECK (status IN ('CREATED')),
    reason             TEXT        NOT NULL CHECK (char_length(btrim(reason)) BETWEEN 1 AND 1000),
    idempotency_key    TEXT        NOT NULL CHECK (char_length(btrim(idempotency_key)) BETWEEN 16 AND 200),
    payload            JSONB       NOT NULL DEFAULT '{}'::jsonb CHECK (jsonb_typeof(payload) = 'object'),
    created_by         UUID        NOT NULL,
    created_at         TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (id, org_id),
    UNIQUE (org_id, idempotency_key),
    FOREIGN KEY (original_run_id, org_id) REFERENCES workflow_runs(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (created_by, org_id) REFERENCES users(id, org_id) ON DELETE RESTRICT
);

CREATE INDEX idx_workflow_compensating_documents_original_run
    ON workflow_compensating_documents (org_id, original_run_id, created_at DESC);

ALTER TABLE workflow_compensating_documents ENABLE ROW LEVEL SECURITY;
ALTER TABLE workflow_compensating_documents FORCE ROW LEVEL SECURITY;
CREATE POLICY org_isolation ON workflow_compensating_documents
    USING (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid)
    WITH CHECK (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid);

GRANT SELECT, INSERT, UPDATE ON workflow_compensating_documents TO console_rt;
REVOKE DELETE ON workflow_compensating_documents FROM console_rt;

CREATE TRIGGER trg_workflow_compensating_documents_org_immutable
    BEFORE UPDATE ON workflow_compensating_documents
    FOR EACH ROW EXECUTE FUNCTION workflow_runtime_org_immutable();
CREATE TRIGGER trg_workflow_compensating_documents_no_delete
    BEFORE DELETE ON workflow_compensating_documents
    FOR EACH ROW EXECUTE FUNCTION workflow_runtime_no_delete();
