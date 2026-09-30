-- no-transaction

-- Keep one CONCURRENTLY statement per no-transaction migration; omit
-- IF NOT EXISTS so a failed concurrent build leaves deploy/CI fail-closed.
CREATE INDEX CONCURRENTLY idx_messenger_messages_quoted
    ON messenger_messages (quoted_message_id)
    WHERE quoted_message_id IS NOT NULL;
