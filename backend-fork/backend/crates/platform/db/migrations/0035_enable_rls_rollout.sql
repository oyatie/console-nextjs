-- Multi-tenant phase 1 rollout, step 4: Row Level Security on every rolled-out
-- table + the org_id immutability trigger + composite (org_id, hot-key) indexes.
-- Mirrors slice 0030/0031 exactly.
--
-- Each table: ENABLE + FORCE RLS, POLICY org_isolation gating USING and WITH
-- CHECK on org_id = the per-tx GUC app.current_org (fail-closed on unset/empty).
-- The enforce_org_id_immutable() function already exists (slice 0031); we only
-- attach the BEFORE UPDATE trigger here.
--
-- audit_events is the deliberate exception (see its block at the end): nullable
-- platform tier, append-only, INSERT/SELECT-only for console_rt.

-- Helper note: console_rt already has DML on these tables via the ALTER DEFAULT
-- PRIVILEGES grant from slice 0031 (owner-created tables auto-grant to console_rt),
-- but we GRANT explicitly per table so the privilege does not depend on the
-- default-privileges timing.

-- ===========================================================================
-- RLS policies (org_id = GUC) on every tenant-scoped rolled-out table.
-- ===========================================================================
DO $$
DECLARE
    t TEXT;
    tenant_tables TEXT[] := ARRAY[
        'user_branches',
        'auth_webauthn_credentials',
        'auth_refresh_token_families',
        'auth_refresh_tokens',
        'auth_bootstrap_credentials',
        'work_order_approval_steps',
        'work_order_assignments',
        'work_order_status_history',
        'target_change_requests',
        'daily_work_plans',
        'daily_work_plan_items',
        'outsource_vendors',
        'outsource_works',
        'evidence_media',
        'offline_sync_requests',
        'registered_devices',
        'p1_dispatches',
        'p1_dispatch_targets',
        'p1_dispatch_responses',
        'p1_dispatch_alerts',
        'messenger_threads',
        'messenger_thread_members',
        'messenger_messages',
        'messenger_message_attachments',
        'messenger_read_receipts',
        'kpi_exclusions',
        'equipment_substitutions',
        'financial_rental_quotes',
        'financial_rental_quote_lines',
        'financial_purchase_requests',
        'financial_purchase_history',
        'equipment_cost_ledger',
        'regular_inspection_schedules',
        'inspection_rounds',
        'location_consents',
        'location_consent_ledger',
        'location_pings',
        'location_collection_logs',
        'excel_export_logs',
        'work_diary_drafts',
        'support_tickets',
        'support_ticket_comments'
    ];
BEGIN
    FOREACH t IN ARRAY tenant_tables LOOP
        EXECUTE format('ALTER TABLE %I ENABLE ROW LEVEL SECURITY', t);
        EXECUTE format('ALTER TABLE %I FORCE ROW LEVEL SECURITY', t);
        EXECUTE format(
            'CREATE POLICY org_isolation ON %I '
            || 'USING (org_id = NULLIF(current_setting(''app.current_org'', true), '''')::uuid) '
            || 'WITH CHECK (org_id = NULLIF(current_setting(''app.current_org'', true), '''')::uuid)',
            t
        );
        EXECUTE format(
            'GRANT SELECT, INSERT, UPDATE, DELETE ON %I TO console_rt', t
        );
        EXECUTE format(
            'CREATE TRIGGER trg_%I_org_immutable BEFORE UPDATE ON %I '
            || 'FOR EACH ROW EXECUTE FUNCTION enforce_org_id_immutable()',
            t, t
        );
    END LOOP;
END
$$;

-- ===========================================================================
-- audit_events: NULLABLE org_id, append-only, INSERT/SELECT-only.
--   * RLS USING (org_id = GUC): a tenant session sees ONLY its own audit rows.
--     Platform-tier rows (org_id IS NULL) are invisible to tenants — they are
--     read later via the owner/bypass path, never leaked to a tenant GUC.
--   * WITH CHECK (org_id = GUC OR org_id IS NULL): tenant-scoped audited writes
--     (org_id stamped) pass; platform-tier audit inserts (org_id NULL, e.g. a
--     retention job that never armed a tenant GUC) also pass. This is what keeps
--     every existing audited mutation green while the read-path tenant resolver
--     is still pending.
--   * INSERT/SELECT grants only — append-only is preserved (no UPDATE/DELETE),
--     so the migration-safety gate has nothing to flag.
--   * NO org_id immutability trigger: audit rows are append-only already, and a
--     platform row (NULL) must never be forced to look tenant-owned.
-- ===========================================================================
ALTER TABLE audit_events ENABLE ROW LEVEL SECURITY;
ALTER TABLE audit_events FORCE ROW LEVEL SECURITY;
CREATE POLICY org_isolation ON audit_events
    USING (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid)
    WITH CHECK (
        org_id = NULLIF(current_setting('app.current_org', true), '')::uuid
        OR org_id IS NULL
    );
-- audit_events already had INSERT,SELECT granted to console_rt in slice 0031; keep
-- it INSERT/SELECT only (no re-grant needed, stated here for intent).

-- ===========================================================================
-- Composite (org_id, hot-key) indexes mirroring each table's existing hot
-- single-key indexes, so tenant-scoped read paths stay index-served.
-- ===========================================================================
CREATE INDEX idx_work_order_assignments_org ON work_order_assignments (org_id, work_order_id);
CREATE INDEX idx_work_order_approval_steps_org ON work_order_approval_steps (org_id, work_order_id);
CREATE INDEX idx_work_order_status_history_org ON work_order_status_history (org_id, work_order_id);
CREATE INDEX idx_daily_work_plans_org ON daily_work_plans (org_id, branch_id, plan_date);
CREATE INDEX idx_outsource_works_org ON outsource_works (org_id, work_order_id);
CREATE INDEX idx_evidence_media_org ON evidence_media (org_id, work_order_id);
CREATE INDEX idx_offline_sync_requests_org ON offline_sync_requests (org_id, user_id);
CREATE INDEX idx_registered_devices_org ON registered_devices (org_id, user_id);
CREATE INDEX idx_p1_dispatches_org ON p1_dispatches (org_id, branch_id, status);
CREATE INDEX idx_p1_dispatch_targets_org ON p1_dispatch_targets (org_id, user_id);
CREATE INDEX idx_p1_dispatch_responses_org ON p1_dispatch_responses (org_id, dispatch_id);
CREATE INDEX idx_p1_dispatch_alerts_org ON p1_dispatch_alerts (org_id, dispatch_id);
CREATE INDEX idx_messenger_threads_org ON messenger_threads (org_id, branch_id);
CREATE INDEX idx_messenger_messages_org ON messenger_messages (org_id, thread_id);
CREATE INDEX idx_kpi_exclusions_org ON kpi_exclusions (org_id, branch_id);
CREATE INDEX idx_equipment_substitutions_org ON equipment_substitutions (org_id, branch_id);
CREATE INDEX idx_financial_rental_quotes_org ON financial_rental_quotes (org_id, branch_id);
CREATE INDEX idx_financial_purchase_requests_org ON financial_purchase_requests (org_id, branch_id, status);
CREATE INDEX idx_equipment_cost_ledger_org ON equipment_cost_ledger (org_id, equipment_id);
CREATE INDEX idx_regular_inspection_schedules_org ON regular_inspection_schedules (org_id, branch_id, due_date);
CREATE INDEX idx_inspection_rounds_org ON inspection_rounds (org_id, branch_id);
CREATE INDEX idx_location_consents_org ON location_consents (org_id, branch_id);
CREATE INDEX idx_location_pings_org ON location_pings (org_id, user_id);
CREATE INDEX idx_excel_export_logs_org ON excel_export_logs (org_id, export_kind);
CREATE INDEX idx_work_diary_drafts_org ON work_diary_drafts (org_id, diary_date);
CREATE INDEX idx_support_tickets_org ON support_tickets (org_id, status);
CREATE INDEX idx_audit_events_org ON audit_events (org_id, occurred_at DESC) WHERE org_id IS NOT NULL;
