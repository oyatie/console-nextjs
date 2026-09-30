-- FORCE tenant hard-removal for the PLATFORM (vendor) tier — the DESTRUCTIVE
-- counterpart to the GUARDED removal in 0051.
--
-- 0051's platform_remove_organization REFUSES (blocked_has_data) the moment a
-- tenant owns any real operational row, so only an empty onboarding shell can be
-- removed. That is the right default. But a genuinely decommissioned tenant — a
-- rehearsal/PoC org that DID accumulate data and must now be permanently erased —
-- has no path under the guard. This function is that path: it deletes EVERY tenant
-- row (no has_data guard) and the org itself.
--
-- Because this is irreversible and crosses into real data, it is fail-closed in
-- two independent ways:
--
--   1. SAFETY RAIL (status gate): it refuses unless organizations.status =
--      'ARCHIVED' (returns 'blocked_active'). Archiving (0036's
--      platform_set_organization_status) is reversible and is the mandatory first
--      step; an ACTIVE or merely SUSPENDED tenant (e.g. KNL in production) can
--      therefore NEVER be force-wiped by a single call. The operator must
--      deliberately archive first, which is itself audited.
--
--   2. The platform sentinel (...00face) is refused explicitly (returns
--      'not_found'), exactly as in 0051, so a bad id can never wipe the platform
--      tier's own anchor row.
--
-- Discipline mirrors 0051 EXACTLY: SECURITY DEFINER owned by console_app, pinned
-- search_path, `SET LOCAL row_security = off` confined to the body and restored
-- before EVERY return path, EXECUTE granted to console_rt with PUBLIC revoked. The
-- audit trail is preserved the same way — the tenant's immutable audit_events are
-- RE-HOMED to the platform sentinel (content verbatim; only org_id/actor/branch_id
-- released) via the DEFINER-only `app.audit_rehome` GUC honored by the
-- removal-aware audit_events_immutable() trigger from 0051 — so the record of the
-- force-wipe and everything that preceded it survives the tenant.
--
-- THE DELETE ORDER. Every one of the ~63 tenant-table FKs to organizations is ON
-- DELETE RESTRICT (nothing cascades from the org row), and the inter-table FKs are
-- a mix of RESTRICT / NO ACTION (block a parent delete) and CASCADE / SET NULL (do
-- not). So we must delete every tenant row EXPLICITLY, children before parents.
-- The order below is the topological sort of the tenant tables by their
-- RESTRICT/NO-ACTION FK edges (derived from pg_constraint), leaves first. Two
-- substitutions vs a plain delete sweep:
--   * audit_events is RE-HOMED in place at its topological position (it is a child
--     of governance_findings via source_audit_event_id and a parent of
--     users/branches via actor/branch_id), NOT deleted — releasing its tenant
--     references unblocks the users/branches deletes that follow while preserving
--     the immutable content.
--   * auth_webauthn_ceremonies has no org_id (it CASCADEs from users); it is
--     cleared by the user join, like 0051.
-- A wrong/missing entry raises a RESTRICT FK error and the whole transaction rolls
-- back (fail-closed) — partial force-wipes are impossible.
--
-- Returns one of: 'removed' | 'not_found' | 'blocked_active'. The caller (the
-- platform provisioner) maps these to 204 / 404 / 409 and emits a DISTINCT
-- platform.tenant.force_remove audit event (org_id = NULL, platform-tier).

CREATE OR REPLACE FUNCTION platform_force_remove_organization(p_id UUID)
RETURNS TEXT
LANGUAGE plpgsql
SECURITY DEFINER
-- Pin search_path: a SECURITY DEFINER function must not resolve objects through a
-- caller-controlled search_path (privilege-escalation hardening).
SET search_path = public, pg_temp
AS $$
DECLARE
    sentinel_org CONSTANT UUID := '00000000-0000-0000-0000-00000000face'::uuid;
    org_status   TEXT;
BEGIN
    -- The platform sentinel is not a removable tenant; refuse it explicitly so a
    -- bad id can never wipe the platform tier's own anchor row.
    IF p_id = sentinel_org THEN
        RETURN 'not_found';
    END IF;

    -- Run the entire gate + cascade with RLS off, confined to this function body
    -- and restored before EVERY return path (a successful return that forgot to
    -- restore would poison the caller's console_rt transaction — see 0036/0051).
    SET LOCAL row_security = off;

    -- Existence + status in one read. A missing org is 'not_found'.
    SELECT status INTO org_status FROM organizations WHERE id = p_id;
    IF NOT FOUND THEN
        SET LOCAL row_security = on;
        RETURN 'not_found';
    END IF;

    -- SAFETY RAIL: refuse unless the tenant is ARCHIVED. Archiving is reversible
    -- and mandatory before a force-wipe, so an ACTIVE/SUSPENDED tenant can never be
    -- erased by one call. Change nothing.
    IF org_status <> 'ARCHIVED' THEN
        SET LOCAL row_security = on;
        RETURN 'blocked_active';
    END IF;

    -- NO has_data guard: erasing real data is the entire point of this path.

    -- ---- Tenant rows, children-first (topological order) -------------------
    -- (a) Auth + comms + finance + ops children, leaves first.
    DELETE FROM auth_bootstrap_credentials      WHERE org_id = p_id;
    DELETE FROM auth_refresh_tokens             WHERE org_id = p_id;
    DELETE FROM auth_refresh_token_families     WHERE org_id = p_id;
    DELETE FROM auth_webauthn_credentials       WHERE org_id = p_id;
    -- auth_webauthn_ceremonies has no org_id (CASCADEs from users); clear by user
    -- so a half-finished enrollment cannot linger or surprise the users delete.
    DELETE FROM auth_webauthn_ceremonies
        WHERE user_id IN (SELECT id FROM users WHERE org_id = p_id);

    DELETE FROM comms_send_rate                 WHERE org_id = p_id;

    DELETE FROM customer_inquiries              WHERE org_id = p_id;
    DELETE FROM daily_work_plan_items           WHERE org_id = p_id;
    DELETE FROM daily_work_plans                WHERE org_id = p_id;

    DELETE FROM email_attachments               WHERE org_id = p_id;
    DELETE FROM email_messages                  WHERE org_id = p_id;
    DELETE FROM email_threads                   WHERE org_id = p_id;
    DELETE FROM email_folders                   WHERE org_id = p_id;
    DELETE FROM email_accounts                  WHERE org_id = p_id;

    DELETE FROM equipment_cost_ledger           WHERE org_id = p_id;
    DELETE FROM equipment_substitutions         WHERE org_id = p_id;
    DELETE FROM excel_export_logs               WHERE org_id = p_id;

    DELETE FROM financial_purchase_history      WHERE org_id = p_id;
    DELETE FROM financial_purchase_requests     WHERE org_id = p_id;
    DELETE FROM financial_rental_quote_lines    WHERE org_id = p_id;
    DELETE FROM financial_rental_quotes         WHERE org_id = p_id;

    -- governance_findings RESTRICT-references audit_events(source_audit_event_id)
    -- and users(subject_user_id/reviewed_by); delete it before the audit re-home
    -- and before users.
    DELETE FROM governance_findings             WHERE org_id = p_id;

    -- (b) RE-HOME this tenant's immutable audit rows to the platform sentinel and
    -- drop their tenant FKs (org_id, actor → users, branch_id → branches). MUST
    -- come BEFORE the users/branches deletes: audit_events.actor/branch_id
    -- RESTRICT-reference them. Content is preserved verbatim; only the references
    -- are released, gated by the DEFINER-only GUC the removal-aware trigger honors.
    PERFORM set_config('app.audit_rehome', 'on', true);
    UPDATE audit_events
    SET org_id    = sentinel_org,
        actor     = NULL,
        branch_id = NULL
    WHERE org_id = p_id;
    PERFORM set_config('app.audit_rehome', 'off', true);

    DELETE FROM inspection_rounds               WHERE org_id = p_id;
    DELETE FROM kpi_exclusions                  WHERE org_id = p_id;

    DELETE FROM location_collection_logs        WHERE org_id = p_id;
    DELETE FROM location_consent_ledger         WHERE org_id = p_id;
    DELETE FROM location_consents               WHERE org_id = p_id;
    DELETE FROM location_pings                  WHERE org_id = p_id;

    DELETE FROM messenger_message_attachments   WHERE org_id = p_id;
    DELETE FROM messenger_read_receipts         WHERE org_id = p_id;
    DELETE FROM messenger_messages              WHERE org_id = p_id;
    DELETE FROM messenger_thread_members        WHERE org_id = p_id;
    DELETE FROM messenger_threads               WHERE org_id = p_id;

    DELETE FROM evidence_media                  WHERE org_id = p_id;

    DELETE FROM offline_sync_requests           WHERE org_id = p_id;

    DELETE FROM outsource_works                 WHERE org_id = p_id;
    DELETE FROM outsource_vendors               WHERE org_id = p_id;

    DELETE FROM p1_dispatch_alerts              WHERE org_id = p_id;
    DELETE FROM p1_dispatch_responses           WHERE org_id = p_id;
    DELETE FROM p1_dispatch_targets             WHERE org_id = p_id;
    DELETE FROM p1_dispatches                   WHERE org_id = p_id;

    DELETE FROM registered_devices              WHERE org_id = p_id;

    DELETE FROM regular_inspection_schedules    WHERE org_id = p_id;

    DELETE FROM sales_listing_media             WHERE org_id = p_id;
    DELETE FROM sales_listings                  WHERE org_id = p_id;

    DELETE FROM site_attendance_events          WHERE org_id = p_id;
    DELETE FROM site_geofence_presence          WHERE org_id = p_id;

    DELETE FROM support_ticket_comments         WHERE org_id = p_id;
    DELETE FROM support_tickets                 WHERE org_id = p_id;

    DELETE FROM target_change_requests          WHERE org_id = p_id;

    DELETE FROM user_branches                   WHERE org_id = p_id;

    DELETE FROM work_diary_drafts               WHERE org_id = p_id;
    DELETE FROM work_order_approval_steps       WHERE org_id = p_id;
    DELETE FROM work_order_assignments          WHERE org_id = p_id;
    DELETE FROM work_order_request_counters     WHERE org_id = p_id;
    DELETE FROM work_order_status_history       WHERE org_id = p_id;
    DELETE FROM work_orders                     WHERE org_id = p_id;

    -- (c) Registry, then the onboarding shell, parents now unblocked, children
    -- before parents.
    DELETE FROM registry_equipment              WHERE org_id = p_id;
    DELETE FROM registry_sites                  WHERE org_id = p_id;
    DELETE FROM registry_customers              WHERE org_id = p_id;

    DELETE FROM users    WHERE org_id = p_id;  -- cascades any remaining auth_* children
    DELETE FROM branches WHERE org_id = p_id;  -- RESTRICT → regions
    DELETE FROM regions  WHERE org_id = p_id;

    -- (d) Finally the org row itself. Every org_id FK is now satisfied.
    DELETE FROM organizations WHERE id = p_id;

    SET LOCAL row_security = on;
    RETURN 'removed';
END;
$$;

-- The runtime role may EXECUTE this (the app's platform path calls it), but still
-- cannot DELETE organizations (or any tenant rows cross-tenant) directly, and
-- still cannot UPDATE audit_events directly (the trigger rejects it without the
-- DEFINER-only GUC). PUBLIC gets no execute.
REVOKE ALL ON FUNCTION platform_force_remove_organization(UUID) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION platform_force_remove_organization(UUID) TO console_rt;
