-- Equipment ownership transfer workflow: legal owner changes are not ordinary
-- PATCH edits. They require a durable request, ordered two-party/legal/accounting
-- signoff line, append-only workflow events, and a final audited asset-owner
-- update. The registry row's org_id stays immutable; this workflow changes the
-- legal owner label/fact only after signoff.

-- console-gate: audited-table equipment_ownership_transfer_requests
CREATE TABLE equipment_ownership_transfer_requests (
    id             UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id         UUID        NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    equipment_id   UUID        NOT NULL,
    branch_id      UUID        NOT NULL,
    from_owner     TEXT        NOT NULL CHECK (btrim(from_owner) <> ''),
    to_owner       TEXT        NOT NULL CHECK (btrim(to_owner) <> ''),
    reason         TEXT        NOT NULL CHECK (char_length(btrim(reason)) BETWEEN 1 AND 1000),
    status         TEXT        NOT NULL CHECK (status IN ('PENDING','APPROVED','REJECTED')),
    current_step   TEXT        NULL CHECK (current_step IS NULL OR current_step IN ('sending_org_admin','receiving_org_admin','legal_signoff','accounting_signoff')),
    approval_line  JSONB       NOT NULL CHECK (jsonb_typeof(approval_line) = 'array'),
    requested_by   UUID        NULL REFERENCES users(id) ON DELETE SET NULL,
    requested_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    decided_at     TIMESTAMPTZ NULL,
    completed_at   TIMESTAMPTZ NULL,
    updated_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (id, org_id),
    FOREIGN KEY (equipment_id, org_id) REFERENCES registry_equipment(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (branch_id, org_id) REFERENCES branches(id, org_id) ON DELETE RESTRICT
);

CREATE INDEX idx_equipment_ownership_transfer_equipment
    ON equipment_ownership_transfer_requests (org_id, equipment_id, requested_at DESC);
CREATE INDEX idx_equipment_ownership_transfer_status
    ON equipment_ownership_transfer_requests (org_id, status, requested_at DESC);

-- console-gate: audited-table equipment_ownership_transfer_events
CREATE TABLE equipment_ownership_transfer_events (
    id          UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id      UUID        NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    request_id  UUID        NOT NULL,
    action      TEXT        NOT NULL CHECK (action ~ '^equipment\.ownership_transfer\.[a-z_]+$'),
    actor_id    UUID        NULL REFERENCES users(id) ON DELETE SET NULL,
    step_key    TEXT        NULL CHECK (step_key IS NULL OR step_key IN ('sending_org_admin','receiving_org_admin','legal_signoff','accounting_signoff')),
    comment     TEXT        NULL CHECK (comment IS NULL OR char_length(btrim(comment)) BETWEEN 1 AND 1000),
    snapshot    JSONB       NOT NULL DEFAULT '{}'::jsonb CHECK (jsonb_typeof(snapshot) = 'object'),
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    FOREIGN KEY (request_id, org_id) REFERENCES equipment_ownership_transfer_requests(id, org_id) ON DELETE RESTRICT
);

CREATE INDEX idx_equipment_ownership_transfer_events_request
    ON equipment_ownership_transfer_events (org_id, request_id, created_at ASC);

ALTER TABLE equipment_ownership_transfer_requests ENABLE ROW LEVEL SECURITY;
ALTER TABLE equipment_ownership_transfer_requests FORCE ROW LEVEL SECURITY;
CREATE POLICY org_isolation ON equipment_ownership_transfer_requests
    USING (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid)
    WITH CHECK (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid);

ALTER TABLE equipment_ownership_transfer_events ENABLE ROW LEVEL SECURITY;
ALTER TABLE equipment_ownership_transfer_events FORCE ROW LEVEL SECURITY;
CREATE POLICY org_isolation ON equipment_ownership_transfer_events
    USING (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid)
    WITH CHECK (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid);

GRANT SELECT, INSERT, UPDATE ON equipment_ownership_transfer_requests TO console_rt;
GRANT SELECT, INSERT ON equipment_ownership_transfer_events TO console_rt;

CREATE OR REPLACE FUNCTION equipment_ownership_transfer_requests_no_delete()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
    RAISE EXCEPTION 'equipment ownership transfer requests are a durable approval ledger: DELETE is forbidden (row id=%)', OLD.id
        USING ERRCODE = '55000';
END;
$$;

CREATE OR REPLACE FUNCTION equipment_ownership_transfer_events_append_only()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
    RAISE EXCEPTION 'equipment ownership transfer events are append-only: %', TG_OP
        USING ERRCODE = 'check_violation';
END;
$$;

CREATE TRIGGER trg_equipment_ownership_transfer_events_no_update
    BEFORE UPDATE ON equipment_ownership_transfer_events
    FOR EACH ROW EXECUTE FUNCTION equipment_ownership_transfer_events_append_only();
CREATE TRIGGER trg_equipment_ownership_transfer_events_no_delete
    BEFORE DELETE ON equipment_ownership_transfer_events
    FOR EACH ROW EXECUTE FUNCTION equipment_ownership_transfer_events_append_only();
CREATE TRIGGER trg_equipment_ownership_transfer_requests_no_delete
    BEFORE DELETE ON equipment_ownership_transfer_requests
    FOR EACH ROW EXECUTE FUNCTION equipment_ownership_transfer_requests_no_delete();

CREATE TRIGGER trg_equipment_ownership_transfer_requests_org_immutable
    BEFORE UPDATE ON equipment_ownership_transfer_requests
    FOR EACH ROW EXECUTE FUNCTION enforce_org_id_immutable();
CREATE TRIGGER trg_equipment_ownership_transfer_events_org_immutable
    BEFORE UPDATE ON equipment_ownership_transfer_events
    FOR EACH ROW EXECUTE FUNCTION enforce_org_id_immutable();
