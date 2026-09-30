-- Immutable per-tenant receipts make an accepted instance action command
-- replay-safe. The unique (org_id, command_id) key is the authority boundary;
-- RLS deliberately makes a foreign tenant's command invisible.
CREATE TABLE ont_action_command_receipts (
    org_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    command_id UUID NOT NULL,
    actor_id UUID NOT NULL,
    payload_digest BYTEA NOT NULL CHECK (octet_length(payload_digest) = 32),
    receipt JSONB NOT NULL,
    created_at TIMESTAMPTZ NOT NULL,
    PRIMARY KEY (org_id, command_id),
    FOREIGN KEY (actor_id, org_id) REFERENCES users(id, org_id) ON DELETE RESTRICT
);

ALTER TABLE ont_action_command_receipts ENABLE ROW LEVEL SECURITY;
ALTER TABLE ont_action_command_receipts FORCE ROW LEVEL SECURITY;
CREATE POLICY org_isolation ON ont_action_command_receipts
    USING (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid)
    WITH CHECK (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid);

CREATE FUNCTION ont_action_command_receipts_immutable()
RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION 'ontology action command receipts are immutable';
END;
$$;
CREATE TRIGGER trg_ont_action_command_receipts_immutable
    BEFORE UPDATE OR DELETE ON ont_action_command_receipts
    FOR EACH ROW EXECUTE FUNCTION ont_action_command_receipts_immutable();

REVOKE ALL ON ont_action_command_receipts FROM PUBLIC;
GRANT SELECT, INSERT ON ont_action_command_receipts TO console_rt;
