-- Index the work_order_assignments.mechanic_id foreign key.
--
-- "What is assigned to this mechanic?" is a hot path: the mobile app's home
-- screen, dispatch reassignment, and per-mechanic workload all filter
-- work_order_assignments by mechanic_id. The only B-tree that leads with a
-- usable column is the UNIQUE (work_order_id, mechanic_id) — its leading
-- column is work_order_id, so it cannot serve a mechanic_id predicate without
-- a full scan. As assignment history accumulates this degrades to a
-- sequential scan per lookup.
--
-- A plain B-tree on mechanic_id makes those lookups index-assisted. The
-- migration runner applies each file in a transaction, so this uses a regular
-- (transactional) CREATE INDEX rather than CONCURRENTLY.
CREATE INDEX idx_work_order_assignments_mechanic_id
    ON work_order_assignments (mechanic_id);
