-- Multi-tenant phase 1 rollout, step 2: backfill org_id on every table from
-- step 1. All pre-existing rows belong to KNL Logistics (tenant #1), so each
-- value is derived from the row's tenant-scoped parent (which the slice already
-- stamped) or, for tables rooted directly at a user/branch already carrying
-- org_id, copied straight across. audit_events backfills to KNL but STAYS
-- nullable.
--
-- Ordering matters: a child can only derive from a parent that already has a
-- non-null org_id. The slice tables (branches, users, work_orders, registry_*)
-- are already backfilled by 0028, so first-hop children resolve immediately;
-- second-hop children (e.g. daily_work_plan_items → daily_work_plans) run after
-- their parent is filled below.

-- ── identity: user_branches + auth tables derive from their user ────────────
UPDATE user_branches c SET org_id = u.org_id
    FROM users u WHERE u.id = c.user_id AND c.org_id IS NULL;
UPDATE auth_webauthn_credentials c SET org_id = u.org_id
    FROM users u WHERE u.id = c.user_id AND c.org_id IS NULL;
UPDATE auth_refresh_token_families c SET org_id = u.org_id
    FROM users u WHERE u.id = c.user_id AND c.org_id IS NULL;
UPDATE auth_refresh_tokens c SET org_id = u.org_id
    FROM users u WHERE u.id = c.user_id AND c.org_id IS NULL;
UPDATE auth_bootstrap_credentials c SET org_id = u.org_id
    FROM users u WHERE u.id = c.user_id AND c.org_id IS NULL;

-- ── work-order children derive from their work_order ────────────────────────
UPDATE work_order_approval_steps c SET org_id = w.org_id
    FROM work_orders w WHERE w.id = c.work_order_id AND c.org_id IS NULL;
UPDATE work_order_assignments c SET org_id = w.org_id
    FROM work_orders w WHERE w.id = c.work_order_id AND c.org_id IS NULL;
UPDATE work_order_status_history c SET org_id = w.org_id
    FROM work_orders w WHERE w.id = c.work_order_id AND c.org_id IS NULL;
UPDATE target_change_requests c SET org_id = w.org_id
    FROM work_orders w WHERE w.id = c.work_order_id AND c.org_id IS NULL;
UPDATE outsource_works c SET org_id = w.org_id
    FROM work_orders w WHERE w.id = c.work_order_id AND c.org_id IS NULL;
UPDATE evidence_media c SET org_id = w.org_id
    FROM work_orders w WHERE w.id = c.work_order_id AND c.org_id IS NULL;

-- ── branch-rooted tables derive from their branch ───────────────────────────
UPDATE outsource_vendors c SET org_id = b.org_id
    FROM branches b WHERE b.id = c.branch_id AND c.org_id IS NULL;
UPDATE daily_work_plans c SET org_id = b.org_id
    FROM branches b WHERE b.id = c.branch_id AND c.org_id IS NULL;
UPDATE kpi_exclusions c SET org_id = b.org_id
    FROM branches b WHERE b.id = c.branch_id AND c.org_id IS NULL;
UPDATE equipment_substitutions c SET org_id = b.org_id
    FROM branches b WHERE b.id = c.branch_id AND c.org_id IS NULL;
UPDATE financial_rental_quotes c SET org_id = b.org_id
    FROM branches b WHERE b.id = c.branch_id AND c.org_id IS NULL;
UPDATE financial_purchase_requests c SET org_id = b.org_id
    FROM branches b WHERE b.id = c.branch_id AND c.org_id IS NULL;
UPDATE equipment_cost_ledger c SET org_id = b.org_id
    FROM branches b WHERE b.id = c.branch_id AND c.org_id IS NULL;
UPDATE regular_inspection_schedules c SET org_id = b.org_id
    FROM branches b WHERE b.id = c.branch_id AND c.org_id IS NULL;
UPDATE inspection_rounds c SET org_id = b.org_id
    FROM branches b WHERE b.id = c.branch_id AND c.org_id IS NULL;
UPDATE p1_dispatches c SET org_id = b.org_id
    FROM branches b WHERE b.id = c.branch_id AND c.org_id IS NULL;
UPDATE messenger_threads c SET org_id = b.org_id
    FROM branches b WHERE b.id = c.branch_id AND c.org_id IS NULL;
UPDATE messenger_messages c SET org_id = b.org_id
    FROM branches b WHERE b.id = c.branch_id AND c.org_id IS NULL;

-- ── user-rooted compliance / sync / device tables derive from their user ────
UPDATE location_consents c SET org_id = u.org_id
    FROM users u WHERE u.id = c.user_id AND c.org_id IS NULL;
UPDATE location_consent_ledger c SET org_id = u.org_id
    FROM users u WHERE u.id = c.user_id AND c.org_id IS NULL;
UPDATE location_pings c SET org_id = u.org_id
    FROM users u WHERE u.id = c.user_id AND c.org_id IS NULL;
UPDATE location_collection_logs c SET org_id = u.org_id
    FROM users u WHERE u.id = c.user_id AND c.org_id IS NULL;
UPDATE offline_sync_requests c SET org_id = u.org_id
    FROM users u WHERE u.id = c.user_id AND c.org_id IS NULL;
UPDATE registered_devices c SET org_id = u.org_id
    FROM users u WHERE u.id = c.user_id AND c.org_id IS NULL;

-- ── second-hop children derive from their now-filled parent ─────────────────
UPDATE daily_work_plan_items c SET org_id = p.org_id
    FROM daily_work_plans p WHERE p.id = c.plan_id AND c.org_id IS NULL;
UPDATE financial_rental_quote_lines c SET org_id = q.org_id
    FROM financial_rental_quotes q WHERE q.id = c.quote_id AND c.org_id IS NULL;
UPDATE financial_purchase_history c SET org_id = r.org_id
    FROM financial_purchase_requests r WHERE r.id = c.purchase_request_id AND c.org_id IS NULL;
UPDATE p1_dispatch_targets c SET org_id = d.org_id
    FROM p1_dispatches d WHERE d.id = c.dispatch_id AND c.org_id IS NULL;
UPDATE p1_dispatch_responses c SET org_id = d.org_id
    FROM p1_dispatches d WHERE d.id = c.dispatch_id AND c.org_id IS NULL;
UPDATE p1_dispatch_alerts c SET org_id = d.org_id
    FROM p1_dispatches d WHERE d.id = c.dispatch_id AND c.org_id IS NULL;
UPDATE messenger_thread_members c SET org_id = t.org_id
    FROM messenger_threads t WHERE t.id = c.thread_id AND c.org_id IS NULL;
UPDATE messenger_message_attachments c SET org_id = m.org_id
    FROM messenger_messages m WHERE m.id = c.message_id AND c.org_id IS NULL;
UPDATE messenger_read_receipts c SET org_id = t.org_id
    FROM messenger_threads t WHERE t.id = c.thread_id AND c.org_id IS NULL;

-- ── support: internal tickets derive from branch; customer-intake tickets are
-- branch-less, so stamp the remainder to KNL. Comments derive from the ticket. ─
UPDATE support_tickets c SET org_id = b.org_id
    FROM branches b WHERE b.id = c.branch_id AND c.org_id IS NULL;
UPDATE support_tickets SET org_id = '00000000-0000-0000-0000-0000000000a1'
    WHERE org_id IS NULL;
UPDATE support_ticket_comments c SET org_id = t.org_id
    FROM support_tickets t WHERE t.id = c.ticket_id AND c.org_id IS NULL;

-- ── reporting: branch-scoped exports/drafts derive from branch; branch-less
-- (org-wide) rows fall back to KNL. ────────────────────────────────────────────
UPDATE excel_export_logs c SET org_id = b.org_id
    FROM branches b WHERE b.id = c.branch_id AND c.org_id IS NULL;
UPDATE excel_export_logs SET org_id = '00000000-0000-0000-0000-0000000000a1'
    WHERE org_id IS NULL;
UPDATE work_diary_drafts c SET org_id = b.org_id
    FROM branches b WHERE b.id = c.branch_id AND c.org_id IS NULL;
UPDATE work_diary_drafts SET org_id = '00000000-0000-0000-0000-0000000000a1'
    WHERE org_id IS NULL;

-- ── audit_events: stamp every existing row to KNL, but the column STAYS
-- nullable (future platform-tier events legitimately have no tenant). ─────────
UPDATE audit_events SET org_id = '00000000-0000-0000-0000-0000000000a1'
    WHERE org_id IS NULL;
