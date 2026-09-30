-- Equipment autocomplete: index-assisted case-insensitive prefix search.
--
-- The equipment autocomplete endpoint filters
--   management_no ILIKE 'raw%' OR equipment_no ILIKE 'raw%' OR model ILIKE 'raw%'
-- on every keystroke. The default-collation B-tree (branch_id, management_no)
-- cannot serve ILIKE, so each call sequentially scans the branch's equipment
-- (and, for an all-branches scope, the whole table) applying three-column OR
-- ILIKE row-by-row.
--
-- pg_trgm GIN indexes make ILIKE 'raw%' index-assisted on each column. One GIN
-- trigram index per column so each arm of the OR can be satisfied independently
-- and the planner can BitmapOr them together.

CREATE EXTENSION IF NOT EXISTS pg_trgm;

CREATE INDEX idx_registry_equipment_management_no_trgm
    ON registry_equipment USING gin (management_no gin_trgm_ops)
    WHERE management_no IS NOT NULL;

CREATE INDEX idx_registry_equipment_equipment_no_trgm
    ON registry_equipment USING gin (equipment_no gin_trgm_ops);

CREATE INDEX idx_registry_equipment_model_trgm
    ON registry_equipment USING gin (model gin_trgm_ops)
    WHERE model IS NOT NULL;
