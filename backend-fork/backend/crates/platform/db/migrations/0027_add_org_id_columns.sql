-- Multi-tenant phase 1, step 2: add the tenant discriminator column.
--
-- Additive / metadata-only: a nullable `org_id` on each slice table. The column
-- is populated (0028) and then enforced NOT NULL with foreign keys (0029) in
-- separate steps so the backfill has somewhere to write before the constraint
-- bites. Nullable-add → backfill → set-not-null is the safe online sequence.

ALTER TABLE regions             ADD COLUMN org_id UUID;
ALTER TABLE branches            ADD COLUMN org_id UUID;
ALTER TABLE users               ADD COLUMN org_id UUID;
ALTER TABLE registry_customers  ADD COLUMN org_id UUID;
ALTER TABLE registry_sites      ADD COLUMN org_id UUID;
ALTER TABLE registry_equipment  ADD COLUMN org_id UUID;
ALTER TABLE work_orders         ADD COLUMN org_id UUID;
