-- no-transaction

-- Channel discovery within a branch (the joinable browse list scans this).
-- Keep one CONCURRENTLY statement per no-transaction migration.
CREATE INDEX CONCURRENTLY idx_messenger_threads_channel
    ON messenger_threads (branch_id, updated_at DESC)
    WHERE visibility = 'channel';
