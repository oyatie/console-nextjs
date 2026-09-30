-- no-transaction
CREATE INDEX CONCURRENTLY idx_work_orders_dispatch_queue_active
    ON work_orders (org_id, target_due_at ASC NULLS LAST, updated_at DESC, id ASC)
    WHERE status IN ('RECEIVED', 'UNASSIGNED', 'ASSIGNED', 'IN_PROGRESS', 'PART_WAITING', 'DELAYED');
