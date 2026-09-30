-- Multi-tenant phase 1 rollout, step 3: enforce the discriminator on every
-- rolled-out table (mirrors slice 0029).
--   * SET NOT NULL on org_id (audit_events EXCLUDED — stays nullable);
--   * FK org_id -> organizations(id) ON DELETE RESTRICT;
--   * parents referenced by a tenant child get UNIQUE(id, org_id);
--   * each FK from a child to a tenant-scoped parent gains a composite
--     (parent_id, org_id) -> parent(id, org_id) same-org guard;
--   * per-tenant business uniques replace any global ones.
-- All additive (ADD CONSTRAINT / SET NOT NULL); no audited table/column dropped.

-- ===========================================================================
-- (A) org_id NOT NULL + FK to organizations on every rolled-out table.
--     audit_events is intentionally absent (nullable platform tier).
-- ===========================================================================
ALTER TABLE user_branches              ALTER COLUMN org_id SET NOT NULL,
    ADD CONSTRAINT user_branches_org_fk FOREIGN KEY (org_id) REFERENCES organizations(id) ON DELETE RESTRICT;
ALTER TABLE auth_webauthn_credentials  ALTER COLUMN org_id SET NOT NULL,
    ADD CONSTRAINT auth_webauthn_credentials_org_fk FOREIGN KEY (org_id) REFERENCES organizations(id) ON DELETE RESTRICT;
ALTER TABLE auth_refresh_token_families ALTER COLUMN org_id SET NOT NULL,
    ADD CONSTRAINT auth_refresh_token_families_org_fk FOREIGN KEY (org_id) REFERENCES organizations(id) ON DELETE RESTRICT;
ALTER TABLE auth_refresh_tokens        ALTER COLUMN org_id SET NOT NULL,
    ADD CONSTRAINT auth_refresh_tokens_org_fk FOREIGN KEY (org_id) REFERENCES organizations(id) ON DELETE RESTRICT;
ALTER TABLE auth_bootstrap_credentials ALTER COLUMN org_id SET NOT NULL,
    ADD CONSTRAINT auth_bootstrap_credentials_org_fk FOREIGN KEY (org_id) REFERENCES organizations(id) ON DELETE RESTRICT;

ALTER TABLE work_order_approval_steps  ALTER COLUMN org_id SET NOT NULL,
    ADD CONSTRAINT work_order_approval_steps_org_fk FOREIGN KEY (org_id) REFERENCES organizations(id) ON DELETE RESTRICT;
ALTER TABLE work_order_assignments     ALTER COLUMN org_id SET NOT NULL,
    ADD CONSTRAINT work_order_assignments_org_fk FOREIGN KEY (org_id) REFERENCES organizations(id) ON DELETE RESTRICT;
ALTER TABLE work_order_status_history  ALTER COLUMN org_id SET NOT NULL,
    ADD CONSTRAINT work_order_status_history_org_fk FOREIGN KEY (org_id) REFERENCES organizations(id) ON DELETE RESTRICT;
ALTER TABLE target_change_requests     ALTER COLUMN org_id SET NOT NULL,
    ADD CONSTRAINT target_change_requests_org_fk FOREIGN KEY (org_id) REFERENCES organizations(id) ON DELETE RESTRICT;
ALTER TABLE daily_work_plans           ALTER COLUMN org_id SET NOT NULL,
    ADD CONSTRAINT daily_work_plans_org_fk FOREIGN KEY (org_id) REFERENCES organizations(id) ON DELETE RESTRICT;
ALTER TABLE daily_work_plan_items      ALTER COLUMN org_id SET NOT NULL,
    ADD CONSTRAINT daily_work_plan_items_org_fk FOREIGN KEY (org_id) REFERENCES organizations(id) ON DELETE RESTRICT;
ALTER TABLE outsource_vendors          ALTER COLUMN org_id SET NOT NULL,
    ADD CONSTRAINT outsource_vendors_org_fk FOREIGN KEY (org_id) REFERENCES organizations(id) ON DELETE RESTRICT;
ALTER TABLE outsource_works            ALTER COLUMN org_id SET NOT NULL,
    ADD CONSTRAINT outsource_works_org_fk FOREIGN KEY (org_id) REFERENCES organizations(id) ON DELETE RESTRICT;

ALTER TABLE evidence_media             ALTER COLUMN org_id SET NOT NULL,
    ADD CONSTRAINT evidence_media_org_fk FOREIGN KEY (org_id) REFERENCES organizations(id) ON DELETE RESTRICT;

ALTER TABLE offline_sync_requests      ALTER COLUMN org_id SET NOT NULL,
    ADD CONSTRAINT offline_sync_requests_org_fk FOREIGN KEY (org_id) REFERENCES organizations(id) ON DELETE RESTRICT;
ALTER TABLE registered_devices         ALTER COLUMN org_id SET NOT NULL,
    ADD CONSTRAINT registered_devices_org_fk FOREIGN KEY (org_id) REFERENCES organizations(id) ON DELETE RESTRICT;

ALTER TABLE p1_dispatches              ALTER COLUMN org_id SET NOT NULL,
    ADD CONSTRAINT p1_dispatches_org_fk FOREIGN KEY (org_id) REFERENCES organizations(id) ON DELETE RESTRICT;
ALTER TABLE p1_dispatch_targets        ALTER COLUMN org_id SET NOT NULL,
    ADD CONSTRAINT p1_dispatch_targets_org_fk FOREIGN KEY (org_id) REFERENCES organizations(id) ON DELETE RESTRICT;
ALTER TABLE p1_dispatch_responses      ALTER COLUMN org_id SET NOT NULL,
    ADD CONSTRAINT p1_dispatch_responses_org_fk FOREIGN KEY (org_id) REFERENCES organizations(id) ON DELETE RESTRICT;
ALTER TABLE p1_dispatch_alerts         ALTER COLUMN org_id SET NOT NULL,
    ADD CONSTRAINT p1_dispatch_alerts_org_fk FOREIGN KEY (org_id) REFERENCES organizations(id) ON DELETE RESTRICT;

ALTER TABLE messenger_threads             ALTER COLUMN org_id SET NOT NULL,
    ADD CONSTRAINT messenger_threads_org_fk FOREIGN KEY (org_id) REFERENCES organizations(id) ON DELETE RESTRICT;
ALTER TABLE messenger_thread_members      ALTER COLUMN org_id SET NOT NULL,
    ADD CONSTRAINT messenger_thread_members_org_fk FOREIGN KEY (org_id) REFERENCES organizations(id) ON DELETE RESTRICT;
ALTER TABLE messenger_messages            ALTER COLUMN org_id SET NOT NULL,
    ADD CONSTRAINT messenger_messages_org_fk FOREIGN KEY (org_id) REFERENCES organizations(id) ON DELETE RESTRICT;
ALTER TABLE messenger_message_attachments ALTER COLUMN org_id SET NOT NULL,
    ADD CONSTRAINT messenger_message_attachments_org_fk FOREIGN KEY (org_id) REFERENCES organizations(id) ON DELETE RESTRICT;
ALTER TABLE messenger_read_receipts       ALTER COLUMN org_id SET NOT NULL,
    ADD CONSTRAINT messenger_read_receipts_org_fk FOREIGN KEY (org_id) REFERENCES organizations(id) ON DELETE RESTRICT;

ALTER TABLE kpi_exclusions             ALTER COLUMN org_id SET NOT NULL,
    ADD CONSTRAINT kpi_exclusions_org_fk FOREIGN KEY (org_id) REFERENCES organizations(id) ON DELETE RESTRICT;
ALTER TABLE equipment_substitutions    ALTER COLUMN org_id SET NOT NULL,
    ADD CONSTRAINT equipment_substitutions_org_fk FOREIGN KEY (org_id) REFERENCES organizations(id) ON DELETE RESTRICT;

ALTER TABLE financial_rental_quotes        ALTER COLUMN org_id SET NOT NULL,
    ADD CONSTRAINT financial_rental_quotes_org_fk FOREIGN KEY (org_id) REFERENCES organizations(id) ON DELETE RESTRICT;
ALTER TABLE financial_rental_quote_lines   ALTER COLUMN org_id SET NOT NULL,
    ADD CONSTRAINT financial_rental_quote_lines_org_fk FOREIGN KEY (org_id) REFERENCES organizations(id) ON DELETE RESTRICT;
ALTER TABLE financial_purchase_requests    ALTER COLUMN org_id SET NOT NULL,
    ADD CONSTRAINT financial_purchase_requests_org_fk FOREIGN KEY (org_id) REFERENCES organizations(id) ON DELETE RESTRICT;
ALTER TABLE financial_purchase_history     ALTER COLUMN org_id SET NOT NULL,
    ADD CONSTRAINT financial_purchase_history_org_fk FOREIGN KEY (org_id) REFERENCES organizations(id) ON DELETE RESTRICT;
ALTER TABLE equipment_cost_ledger          ALTER COLUMN org_id SET NOT NULL,
    ADD CONSTRAINT equipment_cost_ledger_org_fk FOREIGN KEY (org_id) REFERENCES organizations(id) ON DELETE RESTRICT;

ALTER TABLE regular_inspection_schedules   ALTER COLUMN org_id SET NOT NULL,
    ADD CONSTRAINT regular_inspection_schedules_org_fk FOREIGN KEY (org_id) REFERENCES organizations(id) ON DELETE RESTRICT;
ALTER TABLE inspection_rounds              ALTER COLUMN org_id SET NOT NULL,
    ADD CONSTRAINT inspection_rounds_org_fk FOREIGN KEY (org_id) REFERENCES organizations(id) ON DELETE RESTRICT;

ALTER TABLE location_consents          ALTER COLUMN org_id SET NOT NULL,
    ADD CONSTRAINT location_consents_org_fk FOREIGN KEY (org_id) REFERENCES organizations(id) ON DELETE RESTRICT;
ALTER TABLE location_consent_ledger    ALTER COLUMN org_id SET NOT NULL,
    ADD CONSTRAINT location_consent_ledger_org_fk FOREIGN KEY (org_id) REFERENCES organizations(id) ON DELETE RESTRICT;
ALTER TABLE location_pings             ALTER COLUMN org_id SET NOT NULL,
    ADD CONSTRAINT location_pings_org_fk FOREIGN KEY (org_id) REFERENCES organizations(id) ON DELETE RESTRICT;
ALTER TABLE location_collection_logs   ALTER COLUMN org_id SET NOT NULL,
    ADD CONSTRAINT location_collection_logs_org_fk FOREIGN KEY (org_id) REFERENCES organizations(id) ON DELETE RESTRICT;

ALTER TABLE excel_export_logs          ALTER COLUMN org_id SET NOT NULL,
    ADD CONSTRAINT excel_export_logs_org_fk FOREIGN KEY (org_id) REFERENCES organizations(id) ON DELETE RESTRICT;
ALTER TABLE work_diary_drafts          ALTER COLUMN org_id SET NOT NULL,
    ADD CONSTRAINT work_diary_drafts_org_fk FOREIGN KEY (org_id) REFERENCES organizations(id) ON DELETE RESTRICT;

ALTER TABLE support_tickets            ALTER COLUMN org_id SET NOT NULL,
    ADD CONSTRAINT support_tickets_org_fk FOREIGN KEY (org_id) REFERENCES organizations(id) ON DELETE RESTRICT;
ALTER TABLE support_ticket_comments    ALTER COLUMN org_id SET NOT NULL,
    ADD CONSTRAINT support_ticket_comments_org_fk FOREIGN KEY (org_id) REFERENCES organizations(id) ON DELETE RESTRICT;

ALTER TABLE audit_events
    ADD CONSTRAINT audit_events_org_fk FOREIGN KEY (org_id) REFERENCES organizations(id) ON DELETE RESTRICT;

-- ===========================================================================
-- (B) Parents addressable by (id, org_id) so children pin the tenant via a
--     composite FK. (Slice parents regions/branches/registry_customers already
--     have this; registry_equipment/sites/work_orders et al. get it here.)
-- ===========================================================================
ALTER TABLE registry_equipment ADD CONSTRAINT registry_equipment_id_org_key UNIQUE (id, org_id);
ALTER TABLE registry_sites     ADD CONSTRAINT registry_sites_id_org_key     UNIQUE (id, org_id);
ALTER TABLE work_orders        ADD CONSTRAINT work_orders_id_org_key        UNIQUE (id, org_id);
ALTER TABLE users              ADD CONSTRAINT users_id_org_key              UNIQUE (id, org_id);
ALTER TABLE daily_work_plans   ADD CONSTRAINT daily_work_plans_id_org_key   UNIQUE (id, org_id);
ALTER TABLE outsource_vendors  ADD CONSTRAINT outsource_vendors_id_org_key  UNIQUE (id, org_id);
ALTER TABLE p1_dispatches      ADD CONSTRAINT p1_dispatches_id_org_key      UNIQUE (id, org_id);
ALTER TABLE messenger_threads  ADD CONSTRAINT messenger_threads_id_org_key  UNIQUE (id, org_id);
ALTER TABLE messenger_messages ADD CONSTRAINT messenger_messages_id_org_key UNIQUE (id, org_id);
ALTER TABLE evidence_media     ADD CONSTRAINT evidence_media_id_org_key     UNIQUE (id, org_id);
ALTER TABLE support_tickets    ADD CONSTRAINT support_tickets_id_org_key    UNIQUE (id, org_id);
ALTER TABLE location_consents  ADD CONSTRAINT location_consents_id_org_key  UNIQUE (id, org_id);
ALTER TABLE financial_rental_quotes     ADD CONSTRAINT financial_rental_quotes_id_org_key     UNIQUE (id, org_id);
ALTER TABLE financial_purchase_requests ADD CONSTRAINT financial_purchase_requests_id_org_key UNIQUE (id, org_id);
ALTER TABLE regular_inspection_schedules ADD CONSTRAINT regular_inspection_schedules_id_org_key UNIQUE (id, org_id);

-- ===========================================================================
-- (C) Composite same-org FKs from every child to its tenant-scoped parent(s).
--     The original single-column FKs stay; these pin the tenant on top.
-- ===========================================================================
-- identity
ALTER TABLE user_branches
    ADD CONSTRAINT user_branches_user_same_org_fk   FOREIGN KEY (user_id, org_id)   REFERENCES users(id, org_id)    ON DELETE RESTRICT,
    ADD CONSTRAINT user_branches_branch_same_org_fk FOREIGN KEY (branch_id, org_id) REFERENCES branches(id, org_id) ON DELETE RESTRICT;
ALTER TABLE auth_webauthn_credentials
    ADD CONSTRAINT auth_webauthn_credentials_user_same_org_fk FOREIGN KEY (user_id, org_id) REFERENCES users(id, org_id) ON DELETE RESTRICT;
ALTER TABLE auth_refresh_token_families
    ADD CONSTRAINT auth_refresh_token_families_user_same_org_fk FOREIGN KEY (user_id, org_id) REFERENCES users(id, org_id) ON DELETE RESTRICT;
ALTER TABLE auth_refresh_tokens
    ADD CONSTRAINT auth_refresh_tokens_user_same_org_fk FOREIGN KEY (user_id, org_id) REFERENCES users(id, org_id) ON DELETE RESTRICT;
ALTER TABLE auth_bootstrap_credentials
    ADD CONSTRAINT auth_bootstrap_credentials_user_same_org_fk FOREIGN KEY (user_id, org_id) REFERENCES users(id, org_id) ON DELETE RESTRICT;

-- work-order children -> work_orders
ALTER TABLE work_order_approval_steps
    ADD CONSTRAINT work_order_approval_steps_wo_same_org_fk FOREIGN KEY (work_order_id, org_id) REFERENCES work_orders(id, org_id) ON DELETE RESTRICT;
ALTER TABLE work_order_assignments
    ADD CONSTRAINT work_order_assignments_wo_same_org_fk FOREIGN KEY (work_order_id, org_id) REFERENCES work_orders(id, org_id) ON DELETE RESTRICT;
ALTER TABLE work_order_status_history
    ADD CONSTRAINT work_order_status_history_wo_same_org_fk FOREIGN KEY (work_order_id, org_id) REFERENCES work_orders(id, org_id) ON DELETE RESTRICT;
ALTER TABLE target_change_requests
    ADD CONSTRAINT target_change_requests_wo_same_org_fk FOREIGN KEY (work_order_id, org_id) REFERENCES work_orders(id, org_id) ON DELETE RESTRICT;
ALTER TABLE outsource_works
    ADD CONSTRAINT outsource_works_wo_same_org_fk     FOREIGN KEY (work_order_id, org_id) REFERENCES work_orders(id, org_id) ON DELETE RESTRICT,
    ADD CONSTRAINT outsource_works_vendor_same_org_fk FOREIGN KEY (vendor_id, org_id)     REFERENCES outsource_vendors(id, org_id) ON DELETE RESTRICT;
ALTER TABLE evidence_media
    ADD CONSTRAINT evidence_media_wo_same_org_fk FOREIGN KEY (work_order_id, org_id) REFERENCES work_orders(id, org_id) ON DELETE RESTRICT;

-- branch-rooted -> branches
ALTER TABLE outsource_vendors
    ADD CONSTRAINT outsource_vendors_branch_same_org_fk FOREIGN KEY (branch_id, org_id) REFERENCES branches(id, org_id) ON DELETE RESTRICT;
ALTER TABLE daily_work_plans
    ADD CONSTRAINT daily_work_plans_branch_same_org_fk FOREIGN KEY (branch_id, org_id) REFERENCES branches(id, org_id) ON DELETE RESTRICT;
ALTER TABLE kpi_exclusions
    ADD CONSTRAINT kpi_exclusions_branch_same_org_fk FOREIGN KEY (branch_id, org_id) REFERENCES branches(id, org_id) ON DELETE RESTRICT;
ALTER TABLE equipment_substitutions
    ADD CONSTRAINT equipment_substitutions_branch_same_org_fk FOREIGN KEY (branch_id, org_id) REFERENCES branches(id, org_id) ON DELETE RESTRICT,
    ADD CONSTRAINT equipment_substitutions_source_same_org_fk FOREIGN KEY (source_equipment_id, org_id) REFERENCES registry_equipment(id, org_id) ON DELETE RESTRICT,
    ADD CONSTRAINT equipment_substitutions_substitute_same_org_fk FOREIGN KEY (substitute_equipment_id, org_id) REFERENCES registry_equipment(id, org_id) ON DELETE RESTRICT;
ALTER TABLE financial_rental_quotes
    ADD CONSTRAINT financial_rental_quotes_branch_same_org_fk FOREIGN KEY (branch_id, org_id) REFERENCES branches(id, org_id) ON DELETE RESTRICT,
    ADD CONSTRAINT financial_rental_quotes_equipment_same_org_fk FOREIGN KEY (equipment_id, org_id) REFERENCES registry_equipment(id, org_id) ON DELETE RESTRICT;
ALTER TABLE financial_purchase_requests
    ADD CONSTRAINT financial_purchase_requests_branch_same_org_fk FOREIGN KEY (branch_id, org_id) REFERENCES branches(id, org_id) ON DELETE RESTRICT,
    ADD CONSTRAINT financial_purchase_requests_equipment_same_org_fk FOREIGN KEY (equipment_id, org_id) REFERENCES registry_equipment(id, org_id) ON DELETE RESTRICT,
    ADD CONSTRAINT financial_purchase_requests_evidence_same_org_fk FOREIGN KEY (statement_evidence_id, org_id) REFERENCES evidence_media(id, org_id) ON DELETE RESTRICT;
ALTER TABLE equipment_cost_ledger
    ADD CONSTRAINT equipment_cost_ledger_branch_same_org_fk FOREIGN KEY (branch_id, org_id) REFERENCES branches(id, org_id) ON DELETE RESTRICT,
    ADD CONSTRAINT equipment_cost_ledger_equipment_same_org_fk FOREIGN KEY (equipment_id, org_id) REFERENCES registry_equipment(id, org_id) ON DELETE RESTRICT;
ALTER TABLE regular_inspection_schedules
    ADD CONSTRAINT regular_inspection_schedules_branch_same_org_fk FOREIGN KEY (branch_id, org_id) REFERENCES branches(id, org_id) ON DELETE RESTRICT,
    ADD CONSTRAINT regular_inspection_schedules_equipment_same_org_fk FOREIGN KEY (equipment_id, org_id) REFERENCES registry_equipment(id, org_id) ON DELETE RESTRICT;
ALTER TABLE inspection_rounds
    ADD CONSTRAINT inspection_rounds_branch_same_org_fk FOREIGN KEY (branch_id, org_id) REFERENCES branches(id, org_id) ON DELETE RESTRICT,
    ADD CONSTRAINT inspection_rounds_schedule_same_org_fk FOREIGN KEY (schedule_id, org_id) REFERENCES regular_inspection_schedules(id, org_id) ON DELETE RESTRICT,
    ADD CONSTRAINT inspection_rounds_equipment_same_org_fk FOREIGN KEY (equipment_id, org_id) REFERENCES registry_equipment(id, org_id) ON DELETE RESTRICT;
ALTER TABLE p1_dispatches
    ADD CONSTRAINT p1_dispatches_branch_same_org_fk FOREIGN KEY (branch_id, org_id) REFERENCES branches(id, org_id) ON DELETE RESTRICT,
    ADD CONSTRAINT p1_dispatches_wo_same_org_fk FOREIGN KEY (work_order_id, org_id) REFERENCES work_orders(id, org_id) ON DELETE RESTRICT;
ALTER TABLE messenger_threads
    ADD CONSTRAINT messenger_threads_branch_same_org_fk FOREIGN KEY (branch_id, org_id) REFERENCES branches(id, org_id) ON DELETE RESTRICT;
ALTER TABLE messenger_messages
    ADD CONSTRAINT messenger_messages_branch_same_org_fk FOREIGN KEY (branch_id, org_id) REFERENCES branches(id, org_id) ON DELETE RESTRICT,
    ADD CONSTRAINT messenger_messages_thread_same_org_fk FOREIGN KEY (thread_id, org_id) REFERENCES messenger_threads(id, org_id) ON DELETE RESTRICT;

-- second-hop -> their parent
ALTER TABLE daily_work_plan_items
    ADD CONSTRAINT daily_work_plan_items_plan_same_org_fk FOREIGN KEY (plan_id, org_id) REFERENCES daily_work_plans(id, org_id) ON DELETE RESTRICT;
ALTER TABLE financial_rental_quote_lines
    ADD CONSTRAINT financial_rental_quote_lines_quote_same_org_fk FOREIGN KEY (quote_id, org_id) REFERENCES financial_rental_quotes(id, org_id) ON DELETE RESTRICT;
ALTER TABLE financial_purchase_history
    ADD CONSTRAINT financial_purchase_history_request_same_org_fk FOREIGN KEY (purchase_request_id, org_id) REFERENCES financial_purchase_requests(id, org_id) ON DELETE RESTRICT;
ALTER TABLE p1_dispatch_targets
    ADD CONSTRAINT p1_dispatch_targets_dispatch_same_org_fk FOREIGN KEY (dispatch_id, org_id) REFERENCES p1_dispatches(id, org_id) ON DELETE RESTRICT;
ALTER TABLE p1_dispatch_responses
    ADD CONSTRAINT p1_dispatch_responses_dispatch_same_org_fk FOREIGN KEY (dispatch_id, org_id) REFERENCES p1_dispatches(id, org_id) ON DELETE RESTRICT;
ALTER TABLE p1_dispatch_alerts
    ADD CONSTRAINT p1_dispatch_alerts_dispatch_same_org_fk FOREIGN KEY (dispatch_id, org_id) REFERENCES p1_dispatches(id, org_id) ON DELETE RESTRICT;
ALTER TABLE messenger_thread_members
    ADD CONSTRAINT messenger_thread_members_thread_same_org_fk FOREIGN KEY (thread_id, org_id) REFERENCES messenger_threads(id, org_id) ON DELETE RESTRICT;
ALTER TABLE messenger_message_attachments
    ADD CONSTRAINT messenger_message_attachments_message_same_org_fk FOREIGN KEY (message_id, org_id) REFERENCES messenger_messages(id, org_id) ON DELETE RESTRICT,
    ADD CONSTRAINT messenger_message_attachments_evidence_same_org_fk FOREIGN KEY (evidence_id, org_id) REFERENCES evidence_media(id, org_id) ON DELETE RESTRICT;
ALTER TABLE messenger_read_receipts
    ADD CONSTRAINT messenger_read_receipts_thread_same_org_fk FOREIGN KEY (thread_id, org_id) REFERENCES messenger_threads(id, org_id) ON DELETE RESTRICT,
    ADD CONSTRAINT messenger_read_receipts_message_same_org_fk FOREIGN KEY (last_read_message_id, org_id) REFERENCES messenger_messages(id, org_id) ON DELETE RESTRICT;
ALTER TABLE support_ticket_comments
    ADD CONSTRAINT support_ticket_comments_ticket_same_org_fk FOREIGN KEY (ticket_id, org_id) REFERENCES support_tickets(id, org_id) ON DELETE RESTRICT;

-- compliance children -> consent
ALTER TABLE location_consent_ledger
    ADD CONSTRAINT location_consent_ledger_consent_same_org_fk FOREIGN KEY (consent_id, org_id) REFERENCES location_consents(id, org_id) ON DELETE RESTRICT;

-- ===========================================================================
-- (D) Global business uniques become per-tenant uniques. Two tenants may reuse
--     the same vendor name, plan date, consent (per user), etc.
-- ===========================================================================
-- location_consents had a GLOBAL UNIQUE(user_id); user is already org-scoped so
-- this is effectively per-tenant, but make it explicit per-org.
ALTER TABLE location_consents
    DROP CONSTRAINT location_consents_user_id_key,
    ADD CONSTRAINT location_consents_org_user_key UNIQUE (org_id, user_id);
