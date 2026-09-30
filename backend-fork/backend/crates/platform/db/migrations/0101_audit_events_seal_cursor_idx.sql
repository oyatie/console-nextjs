-- no-transaction
-- L20 audit-chain read-path performance: tenant seal cursor order.
-- Keep one CONCURRENTLY statement per no-transaction migration; see 0084.
-- Existing audit indexes serve target/actor/branch/occurred_at reads, not the
-- seal worker/verifier's tenant-visible `(created_at, id)` cursor scans.

CREATE INDEX CONCURRENTLY idx_audit_events_org_seal_cursor
    ON audit_events (org_id, created_at ASC, id ASC)
    WHERE org_id IS NOT NULL;
