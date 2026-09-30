-- 지역·지점 관리 — region/branch soft-delete (deactivation) for the full CRUD.
--
-- Regions and branches are FOUNDATIONAL: every operational row (users via
-- user_branches, equipment, work orders, sites, customers, inspections, …) FKs
-- to `branches.id`, and `branches.region_id` FKs to `regions.id`, all with
-- ON DELETE RESTRICT. A hard DELETE would therefore either be refused by the FK
-- (best case) or orphan/cascade live tenant data (worst case). So the console's
-- "삭제" affordance is a SOFT delete: we add a nullable `deactivated_at`
-- timestamp to both tables. An active row has `deactivated_at IS NULL`; a
-- deactivated row is hidden from the pickers and the org tree but preserved so
-- its historical references (audit, equipment, work orders) stay intact.
--
-- The application enforces the referential guard (refuse to deactivate a region
-- with active branches, or a branch with active users / equipment) and returns
-- a 409 rather than orphaning anything — this column only records the state.
--
-- RLS/grants: both tables are already ENABLE + FORCE ROW LEVEL SECURITY (0030)
-- with the `org_isolation` policy, and `console_rt` already holds
-- SELECT/INSERT/UPDATE/DELETE on them (0031). Adding a nullable column needs no
-- new grant or policy — the existing org_isolation USING/WITH CHECK clauses
-- continue to scope every read and the UPDATE that sets `deactivated_at`. The
-- 0031 `enforce_org_id_immutable` BEFORE UPDATE triggers also still apply, so a
-- deactivation can never move a row between tenants.
--
-- console-gate: audited-table regions
-- console-gate: audited-table branches

ALTER TABLE regions
    ADD COLUMN deactivated_at TIMESTAMPTZ NULL;

ALTER TABLE branches
    ADD COLUMN deactivated_at TIMESTAMPTZ NULL;

-- Partial indexes keep the hot "active only" list scans (the org tree + pickers)
-- cheap as deactivated rows accumulate.
CREATE INDEX idx_regions_active
    ON regions (org_id)
    WHERE deactivated_at IS NULL;

CREATE INDEX idx_branches_active
    ON branches (org_id, region_id)
    WHERE deactivated_at IS NULL;
