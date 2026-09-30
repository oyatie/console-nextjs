-- Expand step 2: inert outbound admission. A PREPARED row is local intent,
-- never evidence of two-site durability or external delivery. No claim/send
-- function, runtime UPDATE/DELETE grant, or read API is installed here.
-- 0237 built this index concurrently; attaching it is a short metadata step.
-- Fail fast on a busy populated table rather than queuing an upgrade lock
-- behind a long transaction and pausing later writers. SQLx retries 0238 on
-- the next migration run; 0237's valid index remains in place.
SET LOCAL lock_timeout = '2s';
ALTER TABLE email_accounts
    ADD CONSTRAINT email_accounts_org_id_id_unique
    UNIQUE USING INDEX email_accounts_org_id_id_unique;

-- console-gate: audited-table email_outbound_operations
CREATE TABLE email_outbound_operations (
    org_id                UUID NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    operation_id          UUID NOT NULL CHECK (operation_id <> '00000000-0000-0000-0000-000000000000'::UUID),
    account_id            UUID NOT NULL,
    actor_user_id         UUID NOT NULL,
    request_codec_version SMALLINT NOT NULL CHECK (request_codec_version > 0),
    request_hash          BYTEA NOT NULL CHECK (octet_length(request_hash) = 32),
    payload_hash          BYTEA NOT NULL CHECK (octet_length(payload_hash) = 32),
    payload_format        TEXT NOT NULL CHECK (payload_format = 'SMTP_MIME'),
    payload_bytes         BYTEA NOT NULL CHECK (octet_length(payload_bytes) BETWEEN 1 AND 41943040),
    envelope_from         TEXT NOT NULL CHECK (btrim(envelope_from) <> ''),
    envelope_recipients   TEXT[] NOT NULL CHECK (cardinality(envelope_recipients) BETWEEN 1 AND 50),
    rfc_message_id        TEXT NOT NULL CHECK (char_length(rfc_message_id) BETWEEN 1 AND 998),
    state                 TEXT NOT NULL DEFAULT 'PREPARED' CHECK (state = 'PREPARED'),
    created_at            TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (org_id, operation_id),
    FOREIGN KEY (org_id, account_id) REFERENCES email_accounts(org_id, id) ON DELETE RESTRICT,
    FOREIGN KEY (actor_user_id, org_id) REFERENCES users(id, org_id) ON DELETE RESTRICT
);

ALTER TABLE email_outbound_operations ENABLE ROW LEVEL SECURITY;
ALTER TABLE email_outbound_operations FORCE ROW LEVEL SECURITY;
CREATE POLICY org_isolation ON email_outbound_operations
    USING (org_id = NULLIF(current_setting('app.current_org', true), '')::UUID)
    WITH CHECK (org_id = NULLIF(current_setting('app.current_org', true), '')::UUID);
-- 0031's default privileges grant new public tables broadly. Narrow this
-- table before any runtime code can touch it: retries need only digest fields.
REVOKE ALL PRIVILEGES ON email_outbound_operations FROM PUBLIC, console_rt;
GRANT INSERT ON email_outbound_operations TO console_rt;
GRANT SELECT (org_id, operation_id, request_codec_version, request_hash)
    ON email_outbound_operations TO console_rt;
CREATE TRIGGER trg_email_outbound_operations_org_immutable
    BEFORE UPDATE ON email_outbound_operations
    FOR EACH ROW EXECUTE FUNCTION enforce_org_id_immutable();

-- Every field in this sender/recipient operation is linkable to people. MIME
-- content is arbitrary and may carry more restricted categories, so its class
-- remains undeclared until content-specific inspection. Direct console_rt
-- INSERT can establish only an untrusted PREPARED record, never sender proof,
-- two-site durability, delivery or an egress entitlement.
COMMENT ON TABLE email_outbound_operations IS 'Outbound mail preparation; PREPARED is not a delivery or durability receipt';
COMMENT ON COLUMN email_outbound_operations.org_id IS 'pd:personal — operation Company context';
COMMENT ON COLUMN email_outbound_operations.operation_id IS 'pd:personal — person-linked operation identity';
COMMENT ON COLUMN email_outbound_operations.account_id IS 'pd:personal — sending mailbox';
COMMENT ON COLUMN email_outbound_operations.actor_user_id IS 'pd:personal — initiating account';
COMMENT ON COLUMN email_outbound_operations.request_codec_version IS 'pd:personal — version in a person-linked operation';
COMMENT ON COLUMN email_outbound_operations.request_hash IS 'pd:personal — stable person-linked request digest';
COMMENT ON COLUMN email_outbound_operations.payload_hash IS 'pd:personal — stable MIME digest';
COMMENT ON COLUMN email_outbound_operations.payload_format IS 'pd:personal — format in a person-linked operation';
COMMENT ON COLUMN email_outbound_operations.payload_bytes IS 'pd:undeclared — exact arbitrary MIME, restricted and never logged';
COMMENT ON COLUMN email_outbound_operations.envelope_from IS 'pd:personal — sender address';
COMMENT ON COLUMN email_outbound_operations.envelope_recipients IS 'pd:personal — includes Bcc, never emitted in MIME';
COMMENT ON COLUMN email_outbound_operations.rfc_message_id IS 'pd:personal — person-linked mail identity';
COMMENT ON COLUMN email_outbound_operations.state IS 'pd:personal — person-linked operation state';
COMMENT ON COLUMN email_outbound_operations.created_at IS 'pd:personal — person-linked preparation time';
