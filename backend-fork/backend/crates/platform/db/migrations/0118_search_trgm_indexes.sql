-- 0117_search_trgm_indexes.sql
-- Index the substring-search fields behind GET /api/v1/search. The endpoint
-- uses escaped ILIKE '%query%' over these columns after org/RLS + branch
-- predicates. Plain btree indexes cannot serve contains matches, so each field
-- gets the same pg_trgm GIN shape already used by equipment autocomplete.

CREATE EXTENSION IF NOT EXISTS pg_trgm;

CREATE INDEX idx_users_display_name_trgm
    ON users USING gin (display_name gin_trgm_ops)
    WHERE is_active;

CREATE INDEX idx_work_orders_request_no_trgm
    ON work_orders USING gin (request_no gin_trgm_ops);

CREATE INDEX idx_support_tickets_title_trgm
    ON support_tickets USING gin (title gin_trgm_ops);

CREATE INDEX idx_branches_name_trgm
    ON branches USING gin (name gin_trgm_ops);

CREATE INDEX idx_registry_equipment_manager_name_trgm
    ON registry_equipment USING gin (manager_name gin_trgm_ops)
    WHERE manager_name IS NOT NULL;
