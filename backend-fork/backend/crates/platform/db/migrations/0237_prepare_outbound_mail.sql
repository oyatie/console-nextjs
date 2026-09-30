-- no-transaction
-- Expand step 1: the composite key needed by the outbound preparation FK.
-- Build without blocking writes on populated email_accounts. Keep this as the
-- only statement in the migration: a failed concurrent build may leave an
-- INVALID index, which requires an operator-inspected DROP INDEX CONCURRENTLY
-- before retrying. Never use IF NOT EXISTS to hide that failure.
CREATE UNIQUE INDEX CONCURRENTLY email_accounts_org_id_id_unique
    ON email_accounts (org_id, id);
