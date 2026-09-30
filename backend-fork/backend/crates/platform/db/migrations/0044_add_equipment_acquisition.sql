-- Asset lifecycle & cost analytics, slice 0: acquisition as a first-class
-- accounting fact on the equipment master.
--
-- `acquisition_cost_won` is the price the asset was acquired for, and
-- `acquisition_date` is when. Both are NULLABLE: most of the fleet was
-- bulk-imported (0007) with no purchase request, so the column must tolerate
-- legacy rows that simply do not know their acquisition price yet. TCO falls
-- back to `vehicle_value` (the depreciation base) with a source tag when
-- acquisition is unknown.
--
-- This is ADDITIVE and additive only: no column or table is dropped, so the
-- migration-safety gate passes. registry_equipment is already RLS-ENABLED +
-- FORCED + org-immutable (0030) and carries org_id (0027/0029), so these
-- columns inherit the tenant policy with no new RLS / grant / index work.
--
-- DELIBERATELY distinct from `vehicle_value`: `vehicle_value` is bound as the
-- depreciation base in recompute_residual_value (financial adapter
-- append_cost_ledger_entry_tx); `acquisition_cost_won` is an auditable
-- accounting fact and MUST NEVER feed the residual engine. The CHECK mirrors
-- the bounded/non-negative money convention of 0025 and the existing
-- registry_equipment money columns (rental_fee / vehicle_value / residual_value
-- are all plain BIGINT; acquisition adds an explicit >= 0 guard since it is a
-- newly user-entered amount).

-- console-gate: audited-table registry_equipment
ALTER TABLE registry_equipment
    ADD COLUMN acquisition_cost_won BIGINT
        CHECK (acquisition_cost_won IS NULL OR acquisition_cost_won >= 0),
    ADD COLUMN acquisition_date DATE;
