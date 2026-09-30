-- Multi-tenant phase 1 rollout, step 1: add the nullable org_id discriminator
-- to EVERY remaining tenant-scoped table (the slice already carries it).
--
-- Additive / metadata-only: a nullable `org_id` on each table. Columns are
-- populated (0033) and then enforced NOT NULL with foreign keys + composite
-- same-org FKs + RLS (0034..) in separate steps so the backfill has somewhere
-- to write before the constraint bites. Nullable-add → backfill → set-not-null
-- is the safe online sequence proven on the slice (0027/0028/0029).
--
-- audit_events gets a NULLABLE org_id that STAYS nullable: platform-tier events
-- (roster import, retention jobs) legitimately have no tenant.

-- ── work-order domain children ──────────────────────────────────────────────
ALTER TABLE work_order_approval_steps  ADD COLUMN org_id UUID;
ALTER TABLE work_order_assignments     ADD COLUMN org_id UUID;
ALTER TABLE work_order_status_history  ADD COLUMN org_id UUID;
ALTER TABLE target_change_requests     ADD COLUMN org_id UUID;
ALTER TABLE daily_work_plans           ADD COLUMN org_id UUID;
ALTER TABLE daily_work_plan_items      ADD COLUMN org_id UUID;
ALTER TABLE outsource_vendors          ADD COLUMN org_id UUID;
ALTER TABLE outsource_works            ADD COLUMN org_id UUID;

-- ── evidence ────────────────────────────────────────────────────────────────
ALTER TABLE evidence_media             ADD COLUMN org_id UUID;

-- ── mobile sync + devices ───────────────────────────────────────────────────
ALTER TABLE offline_sync_requests      ADD COLUMN org_id UUID;
ALTER TABLE registered_devices         ADD COLUMN org_id UUID;

-- ── P1 dispatch ─────────────────────────────────────────────────────────────
ALTER TABLE p1_dispatches              ADD COLUMN org_id UUID;
ALTER TABLE p1_dispatch_targets        ADD COLUMN org_id UUID;
ALTER TABLE p1_dispatch_responses      ADD COLUMN org_id UUID;
ALTER TABLE p1_dispatch_alerts         ADD COLUMN org_id UUID;

-- ── messenger ───────────────────────────────────────────────────────────────
ALTER TABLE messenger_threads             ADD COLUMN org_id UUID;
ALTER TABLE messenger_thread_members      ADD COLUMN org_id UUID;
ALTER TABLE messenger_messages            ADD COLUMN org_id UUID;
ALTER TABLE messenger_message_attachments ADD COLUMN org_id UUID;
ALTER TABLE messenger_read_receipts       ADD COLUMN org_id UUID;

-- ── KPI / substitutions ─────────────────────────────────────────────────────
ALTER TABLE kpi_exclusions             ADD COLUMN org_id UUID;
ALTER TABLE equipment_substitutions    ADD COLUMN org_id UUID;

-- ── financial ───────────────────────────────────────────────────────────────
ALTER TABLE financial_rental_quotes        ADD COLUMN org_id UUID;
ALTER TABLE financial_rental_quote_lines   ADD COLUMN org_id UUID;
ALTER TABLE financial_purchase_requests    ADD COLUMN org_id UUID;
ALTER TABLE financial_purchase_history     ADD COLUMN org_id UUID;
ALTER TABLE equipment_cost_ledger          ADD COLUMN org_id UUID;

-- ── inspection ──────────────────────────────────────────────────────────────
ALTER TABLE regular_inspection_schedules   ADD COLUMN org_id UUID;
ALTER TABLE inspection_rounds              ADD COLUMN org_id UUID;

-- ── compliance / location ───────────────────────────────────────────────────
ALTER TABLE location_consents          ADD COLUMN org_id UUID;
ALTER TABLE location_consent_ledger    ADD COLUMN org_id UUID;
ALTER TABLE location_pings             ADD COLUMN org_id UUID;
ALTER TABLE location_collection_logs   ADD COLUMN org_id UUID;

-- ── reporting ───────────────────────────────────────────────────────────────
ALTER TABLE excel_export_logs          ADD COLUMN org_id UUID;
ALTER TABLE work_diary_drafts          ADD COLUMN org_id UUID;

-- ── support ─────────────────────────────────────────────────────────────────
ALTER TABLE support_tickets            ADD COLUMN org_id UUID;
ALTER TABLE support_ticket_comments    ADD COLUMN org_id UUID;

-- ── identity: user_branches + auth tables (one-user-one-org) ────────────────
-- auth_webauthn_ceremonies is deliberately EXCLUDED: its user_id is nullable
-- (an authentication ceremony exists before the user is resolved), so it is
-- transient pre-auth state like auth_rate_limit and stays GLOBAL (no org_id,
-- no RLS).
ALTER TABLE user_branches              ADD COLUMN org_id UUID;
ALTER TABLE auth_webauthn_credentials  ADD COLUMN org_id UUID;
ALTER TABLE auth_refresh_token_families ADD COLUMN org_id UUID;
ALTER TABLE auth_refresh_tokens        ADD COLUMN org_id UUID;
ALTER TABLE auth_bootstrap_credentials ADD COLUMN org_id UUID;

-- ── audit_events: NULLABLE org_id, stays nullable (platform events have none) ─
ALTER TABLE audit_events               ADD COLUMN org_id UUID;
