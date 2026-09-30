-- Messenger Slack-parity, part 2: per-message ack reactions + reply-quote.
--
-- Ack ("확인"): one row per (message, user). Toggling is an idempotent
-- insert/delete; the live count is COUNT(*) over the rows. A realtime
-- `message_ack` event (same LISTEN/NOTIFY pattern as message_posted) lets the
-- count chip update without a poll.
-- console-gate: audited-table messenger_message_acks
CREATE TABLE messenger_message_acks (
    message_id UUID        NOT NULL REFERENCES messenger_messages(id) ON DELETE CASCADE,
    user_id    UUID        NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    org_id     UUID        NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    acked_at   TIMESTAMPTZ NOT NULL,
    -- (message_id, user_id) PK: message_id leads, so COUNT-by-message and the
    -- toggle's point lookup both use the primary key; no extra index needed.
    PRIMARY KEY (message_id, user_id)
);

ALTER TABLE messenger_message_acks ENABLE ROW LEVEL SECURITY;
ALTER TABLE messenger_message_acks FORCE ROW LEVEL SECURITY;
CREATE POLICY org_isolation ON messenger_message_acks
    USING (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid)
    WITH CHECK (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid);
GRANT SELECT, INSERT, DELETE ON messenger_message_acks TO console_rt;

-- Reply-quote: a message may quote an earlier message. The same-thread
-- invariant (a quote only targets a message in its own thread) is enforced in
-- the send path, since a CHECK cannot read the quoted row's thread. ON DELETE
-- SET NULL keeps a reply intact if the quoted message is later removed.
ALTER TABLE messenger_messages
    ADD COLUMN quoted_message_id UUID;

ALTER TABLE messenger_messages
    ADD CONSTRAINT messenger_messages_quoted_message_fk
        FOREIGN KEY (quoted_message_id)
        REFERENCES messenger_messages(id)
        ON DELETE SET NULL
        NOT VALID;

ALTER TABLE messenger_messages VALIDATE CONSTRAINT messenger_messages_quoted_message_fk;
