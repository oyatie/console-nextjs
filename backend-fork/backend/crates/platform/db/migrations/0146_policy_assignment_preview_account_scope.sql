-- Extend assignment preview receipts so account/person role and branch-scope
-- mutations can reuse the same server-side receipt gate as custom role writes.
-- The defaults backfill any existing unconsumed receipts safely; new writes always
-- bind exact current/requested system-role and branch sets.
ALTER TABLE policy_assignment_preview_receipts
    ADD COLUMN current_system_roles TEXT[] NOT NULL DEFAULT '{}',
    ADD COLUMN branch_ids UUID[] NOT NULL DEFAULT '{}',
    ADD COLUMN system_roles TEXT[] NOT NULL DEFAULT '{}';
