-- Account UUID allocation and retirement
-- Additive ownership prerequisite: no enrollment, humanity or recovery inference.
-- Block concurrent writers through backfill and trigger installation/commit.
LOCK TABLE public.users IN SHARE ROW EXCLUSIVE MODE;

CREATE SCHEMA auth_security;
CREATE TABLE auth_security.account_id_reservations (
    account_id UUID PRIMARY KEY,
    retired BOOLEAN NOT NULL
);
-- Minimal permanent allocation/denial set. No Company, profile, key or secret.
-- Keep outside public child scans and all deletable-root foreign keys.
INSERT INTO auth_security.account_id_reservations (account_id, retired)
SELECT id, false FROM public.users;

CREATE FUNCTION auth_security.retire_account_id(p_id UUID)
RETURNS VOID
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog
AS $$
BEGIN
    UPDATE auth_security.account_id_reservations SET retired = true WHERE account_id = p_id;
    IF NOT FOUND THEN
        RAISE EXCEPTION 'account identity reservation missing' USING ERRCODE = '23514';
    END IF;
END;
$$;

CREATE FUNCTION auth_security.account_identity_guard()
RETURNS TRIGGER
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog
AS $$
BEGIN
    IF TG_OP = 'INSERT' THEN
        -- AFTER INSERT excludes conflict paths that did not insert an Account.
        -- An unconditional unique reservation also rejects old-snapshot reuse.
        INSERT INTO auth_security.account_id_reservations (account_id, retired)
        VALUES (NEW.id, false);
        RETURN NEW;
    ELSIF TG_OP = 'UPDATE' THEN
        -- Pure OLD/NEW guard: no reverse Account/security or child-row lock.
        IF NEW.id IS DISTINCT FROM OLD.id OR NEW.org_id IS DISTINCT FROM OLD.org_id THEN
            RAISE EXCEPTION 'Account UUID and home Company are immutable' USING ERRCODE = '23514';
        END IF;
        RETURN NEW;
    ELSIF TG_OP = 'DELETE' THEN
        PERFORM auth_security.retire_account_id(OLD.id);
        RETURN OLD;
    END IF;
    RAISE EXCEPTION 'unsupported Account identity transition' USING ERRCODE = '23514';
END;
$$;

CREATE TRIGGER account_identity_reservation
AFTER INSERT ON public.users
FOR EACH ROW EXECUTE FUNCTION auth_security.account_identity_guard();
CREATE TRIGGER account_identity_immutable
BEFORE UPDATE ON public.users
FOR EACH ROW EXECUTE FUNCTION auth_security.account_identity_guard();
CREATE TRIGGER account_identity_retirement
BEFORE DELETE ON public.users
FOR EACH ROW EXECUTE FUNCTION auth_security.account_identity_guard();

CREATE FUNCTION auth_security.retire_company_accounts(p_org UUID)
RETURNS VOID
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog
SET row_security = off
AS $$
DECLARE
    account UUID;
BEGIN
    -- Both callers already hold the complete sorted Account set from0227.
    -- Eligibility has passed; these marks roll back with any later failure.
    FOR account IN SELECT id FROM public.users WHERE org_id = p_org ORDER BY id LOOP
        PERFORM auth_security.retire_account_id(account);
    END LOOP;
END;
$$;

REVOKE ALL ON SCHEMA auth_security FROM PUBLIC, console_rt, console_leave_cmd,
    console_ontology_cmd, console_platform_force_cmd, console_leave_definer, console_ontology_writer;
REVOKE ALL ON ALL TABLES IN SCHEMA auth_security FROM PUBLIC, console_rt, console_leave_cmd,
    console_ontology_cmd, console_platform_force_cmd, console_leave_definer, console_ontology_writer;
REVOKE ALL (account_id, retired) ON auth_security.account_id_reservations
    FROM PUBLIC, console_rt, console_leave_cmd, console_ontology_cmd,
    console_platform_force_cmd, console_leave_definer, console_ontology_writer;
REVOKE ALL ON ALL FUNCTIONS IN SCHEMA auth_security FROM PUBLIC, console_rt, console_leave_cmd,
    console_ontology_cmd, console_platform_force_cmd, console_leave_definer, console_ontology_writer;
REVOKE TRUNCATE ON public.users FROM PUBLIC, console_rt, console_leave_cmd,
    console_ontology_cmd, console_platform_force_cmd, console_leave_definer, console_ontology_writer;

CREATE OR REPLACE FUNCTION platform_remove_organization(p_id UUID)
RETURNS TEXT
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = public, pg_temp
AS $$
DECLARE
    sentinel_org CONSTANT UUID := '00000000-0000-0000-0000-00000000face'::uuid;
    org_exists   BOOLEAN;
    has_data     BOOLEAN;
BEGIN
    IF p_id = sentinel_org THEN
        RETURN 'not_found';
    END IF;

    SET LOCAL row_security = off;

    org_exists := platform_lock_organization_accounts_for_removal(p_id) IS NOT NULL;
    IF NOT org_exists THEN
        SET LOCAL row_security = on;
        RETURN 'not_found';
    END IF;

    SELECT
        EXISTS (SELECT 1 FROM registry_equipment           WHERE org_id = p_id)
     OR EXISTS (SELECT 1 FROM work_orders                   WHERE org_id = p_id)
     OR EXISTS (SELECT 1 FROM registry_sites               WHERE org_id = p_id)
     OR EXISTS (SELECT 1 FROM registry_customers           WHERE org_id = p_id)
     OR EXISTS (SELECT 1 FROM inspection_rounds            WHERE org_id = p_id)
     OR EXISTS (SELECT 1 FROM regular_inspection_schedules WHERE org_id = p_id)
     OR EXISTS (SELECT 1 FROM sales_listings               WHERE org_id = p_id)
     OR EXISTS (SELECT 1 FROM customer_inquiries           WHERE org_id = p_id)
     OR EXISTS (SELECT 1 FROM financial_rental_quotes      WHERE org_id = p_id)
     OR EXISTS (SELECT 1 FROM financial_purchase_requests  WHERE org_id = p_id)
     OR EXISTS (SELECT 1 FROM financial_purchase_attachments WHERE org_id = p_id)
     OR EXISTS (SELECT 1 FROM financial_regular_purchase_prices WHERE org_id = p_id)
     OR EXISTS (SELECT 1 FROM financial_expense_ledger      WHERE org_id = p_id)
     OR EXISTS (SELECT 1 FROM equipment_cost_ledger        WHERE org_id = p_id)
     OR EXISTS (SELECT 1 FROM messenger_threads            WHERE org_id = p_id)
     OR EXISTS (SELECT 1 FROM location_consents            WHERE org_id = p_id)
     OR EXISTS (SELECT 1 FROM site_attendance_events       WHERE org_id = p_id)
     OR EXISTS (SELECT 1 FROM governance_findings          WHERE org_id = p_id)
     OR EXISTS (SELECT 1 FROM registered_devices rd
                JOIN users u ON u.id = rd.user_id WHERE u.org_id = p_id)
    INTO has_data;

    IF has_data THEN
        SET LOCAL row_security = on;
        RETURN 'blocked_has_data';
    END IF;

    PERFORM auth_security.retire_company_accounts(p_id);

    DELETE FROM auth_refresh_tokens         WHERE org_id = p_id;
    DELETE FROM auth_refresh_token_families WHERE org_id = p_id;
    DELETE FROM auth_webauthn_credentials   WHERE org_id = p_id;
    DELETE FROM auth_webauthn_ceremonies
        WHERE user_id IN (SELECT id FROM users WHERE org_id = p_id);
    DELETE FROM auth_bootstrap_credentials  WHERE org_id = p_id;

    DELETE FROM user_branches WHERE org_id = p_id;

    PERFORM set_config('app.audit_rehome', 'on', true);
    UPDATE audit_events
    SET org_id    = sentinel_org,
        actor     = NULL,
        branch_id = NULL
    WHERE org_id = p_id;
    PERFORM set_config('app.audit_rehome', 'off', true);

    DELETE FROM users    WHERE org_id = p_id;
    DELETE FROM branches WHERE org_id = p_id;
    DELETE FROM regions  WHERE org_id = p_id;

    DELETE FROM organizations WHERE id = p_id;

    SET LOCAL row_security = on;
    RETURN 'removed';
END;
$$;

REVOKE ALL ON FUNCTION platform_remove_organization(UUID) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION platform_remove_organization(UUID) TO console_rt;

CREATE OR REPLACE FUNCTION platform_force_remove_organization(p_id UUID)
RETURNS TEXT
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = public, pg_temp
AS $$
DECLARE
    sentinel_org CONSTANT UUID := '00000000-0000-0000-0000-00000000face'::uuid;
    org_status   TEXT;
BEGIN
    IF p_id = sentinel_org THEN
        RETURN 'not_found';
    END IF;

    SET LOCAL row_security = off;
    PERFORM set_config('app.maintenance_force_remove', 'on', true);

    org_status := platform_lock_organization_accounts_for_removal(p_id);
    IF org_status IS NULL THEN
        SET LOCAL row_security = on;
        RETURN 'not_found';
    END IF;

    IF org_status <> 'ARCHIVED' THEN
        SET LOCAL row_security = on;
        RETURN 'blocked_active';
    END IF;

    PERFORM auth_security.retire_company_accounts(p_id);

    PERFORM set_config('app.platform_force_remove_org', 'on', true);
    -- Close direct restrictive tenant edges before deleting employee/user roots.
    -- The catalog query excludes only the explicitly ordered roots below.
    PERFORM platform_force_remove_direct_org_children(p_id);
    DELETE FROM attendance_direct_import_events  WHERE org_id = p_id;
    DELETE FROM data_import_rows                 WHERE org_id = p_id;
    DELETE FROM data_import_runs                 WHERE org_id = p_id;
    DELETE FROM payroll_draft_lines             WHERE org_id = p_id;
    DELETE FROM annual_leave_obligations        WHERE org_id = p_id;
    DELETE FROM payroll_draft_runs              WHERE org_id = p_id;
    DELETE FROM employee_lifecycle_events       WHERE org_id = p_id;
    UPDATE users SET employee_id = NULL         WHERE org_id = p_id;
    DELETE FROM employees                       WHERE org_id = p_id;
    PERFORM set_config('app.platform_force_remove_org', 'off', true);

    DELETE FROM auth_bootstrap_credentials      WHERE org_id = p_id;
    DELETE FROM auth_refresh_tokens             WHERE org_id = p_id;
    DELETE FROM auth_refresh_token_families     WHERE org_id = p_id;
    DELETE FROM auth_webauthn_credentials       WHERE org_id = p_id;
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
    DELETE FROM mailbox_deliveries             WHERE org_id = p_id;
    DELETE FROM mailbox_messages               WHERE org_id = p_id;
    DELETE FROM mailbox_aliases                WHERE org_id = p_id;
    DELETE FROM mailboxes                      WHERE org_id = p_id;
    DELETE FROM mailbox_domains                WHERE org_id = p_id;

    DELETE FROM equipment_maintenance_history_costs WHERE org_id = p_id;
    DELETE FROM equipment_maintenance_history_evidence WHERE org_id = p_id;
    DELETE FROM equipment_maintenance_history WHERE org_id = p_id;
    DELETE FROM equipment_cost_ledger           WHERE org_id = p_id;
    DELETE FROM equipment_substitutions         WHERE org_id = p_id;
    DELETE FROM excel_export_logs               WHERE org_id = p_id;

    DELETE FROM user_feature_preferences        WHERE org_id = p_id;

    DELETE FROM financial_regular_purchase_prices WHERE org_id = p_id;
    DELETE FROM financial_expense_ledger          WHERE org_id = p_id;
    DELETE FROM financial_purchase_attachments    WHERE org_id = p_id;
    DELETE FROM financial_purchase_request_lines  WHERE org_id = p_id;

    DELETE FROM financial_purchase_history      WHERE org_id = p_id;
    DELETE FROM financial_purchase_requests     WHERE org_id = p_id;
    DELETE FROM financial_rental_quote_lines    WHERE org_id = p_id;
    DELETE FROM financial_rental_quotes         WHERE org_id = p_id;

    DELETE FROM governance_findings             WHERE org_id = p_id;

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

    DELETE FROM registry_equipment              WHERE org_id = p_id;
    DELETE FROM registry_sites                  WHERE org_id = p_id;
    DELETE FROM registry_customers              WHERE org_id = p_id;

    DELETE FROM users    WHERE org_id = p_id;
    DELETE FROM branches WHERE org_id = p_id;
    DELETE FROM regions  WHERE org_id = p_id;

    DELETE FROM organizations WHERE id = p_id;

    SET LOCAL row_security = on;
    RETURN 'removed';
END;
$$;
-- Preserve the final0196 ACL: only the receipt command is callable.
REVOKE ALL ON FUNCTION platform_force_remove_organization(UUID)
    FROM PUBLIC, console_rt, console_platform_force_cmd;

