-- Maintenance history is appended only by append_equipment_maintenance_history,
-- a tenant-fenced SECURITY DEFINER function. Migration 0031's future-table
-- default DML grant pre-dates these relations, so revoke every direct mutation
-- verb explicitly. SELECT remains the runtime read surface; archive-gated
-- deletion remains exclusively inside platform_force_remove_organization.
REVOKE INSERT, UPDATE, DELETE, TRUNCATE ON
    equipment_maintenance_history,
    equipment_maintenance_history_evidence,
    equipment_maintenance_history_costs
FROM console_rt;
