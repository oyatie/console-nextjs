-- Slice A (anti-embezzlement, task #34): add the `is_org_lead` flag to users.
--
-- An org 대표(CEO/principal) is the only role that is permitted to self-approve
-- their own 기안 (purchase request). No higher approver exists in the chain for
-- this person, so blocking self-approval would permanently stall a purchase that
-- the 대표 alone can close.
--
-- Design constraints:
--   * At most one 대표 per org — enforced by a partial unique index on
--     (org_id, is_org_lead) WHERE is_org_lead = true. Non-leads are excluded
--     by the WHERE clause so duplicate false rows do not conflict.
--   * DEFAULT false: existing users are not leads; the flag is set by an ADMIN
--     via the user-manage flow.
--   * RLS: `users` already has `org_isolation` from migration 0030; this column
--     rides inside the same RLS-scoped row and needs no additional policy.
--
-- console-gate: audited-table users
ALTER TABLE users
    ADD COLUMN is_org_lead BOOLEAN NOT NULL DEFAULT false;

-- Enforce at-most-one 대표 per org (partial unique index on true rows only).
CREATE UNIQUE INDEX idx_users_org_lead
    ON users (org_id, is_org_lead)
    WHERE is_org_lead = true;
