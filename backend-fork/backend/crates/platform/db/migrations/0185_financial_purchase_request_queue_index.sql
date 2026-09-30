-- Serialize the already-required purchase-request queue access path into the
-- historical 0185 slot. Some deployed tenants received the same index under
-- the earlier 0181 consolidation number, so preserve their schema while still
-- enforcing the exact tenant/branch queue invariant for fresh installs.
DO $$
BEGIN
    IF NOT EXISTS (
        SELECT 1
        FROM pg_catalog.pg_class AS index_relation
        JOIN pg_catalog.pg_namespace AS schema
          ON schema.oid = index_relation.relnamespace
        WHERE schema.nspname = 'public'
          AND index_relation.relname = 'idx_financial_purchase_requests_queue_page'
          AND index_relation.relkind = 'i'
    ) THEN
        CREATE INDEX idx_financial_purchase_requests_queue_page
            ON financial_purchase_requests (org_id, branch_id, updated_at DESC, id DESC);
    END IF;
END $$;
