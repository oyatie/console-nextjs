-- Requirement 2: direct attendance import ledger and append-only facts.
-- These rows are coordinate-free attendance facts sourced from workbooks/CSV, not
-- geofence telemetry. They preserve data-import lineage for later payroll
-- readiness without creating payroll payable lines.

ALTER TABLE data_import_runs
    DROP CONSTRAINT data_import_runs_entity_type_check,
    ADD CONSTRAINT data_import_runs_entity_type_check
        CHECK (entity_type IN ('employee_hr', 'attendance_direct'));

ALTER TABLE data_import_rows
    ADD CONSTRAINT data_import_rows_id_org_key UNIQUE (id, org_id);


-- console-gate: audited-table attendance_direct_import_events
CREATE TABLE attendance_direct_import_events (
    id              UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id          UUID        NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    run_id          UUID        NOT NULL,
    import_row_id   UUID        NOT NULL,
    employee_id     UUID        NOT NULL,
    branch_id       UUID        NOT NULL,
    source_sheet    TEXT        NOT NULL CHECK (btrim(source_sheet) <> ''),
    source_row      INTEGER     NOT NULL CHECK (source_row > 0),
    source_key      TEXT        NOT NULL CHECK (btrim(source_key) <> ''),
    source_sha256   TEXT        NOT NULL CHECK (source_sha256 ~ '^[a-f0-9]{64}$'),
    fact_key        TEXT        NOT NULL CHECK (btrim(fact_key) <> ''),
    employee_number TEXT        NULL CHECK (employee_number IS NULL OR btrim(employee_number) <> ''),
    employee_name   TEXT        NOT NULL CHECK (btrim(employee_name) <> ''),
    branch_name     TEXT        NOT NULL CHECK (btrim(branch_name) <> ''),
    work_date       TEXT        NOT NULL CHECK (work_date ~ '^[0-9]{4}-[0-9]{2}-[0-9]{2}$'),
    check_in_at     TEXT        NULL CHECK (check_in_at IS NULL OR check_in_at ~ '^([01][0-9]|2[0-3]):[0-5][0-9]$'),
    check_out_at    TEXT        NULL CHECK (check_out_at IS NULL OR check_out_at ~ '^([01][0-9]|2[0-3]):[0-5][0-9]$'),
    minutes_worked  INTEGER     NULL CHECK (minutes_worked IS NULL OR minutes_worked >= 0),
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (org_id, run_id, source_key),
    UNIQUE (org_id, source_sha256, source_key),
    UNIQUE (org_id, fact_key),
    FOREIGN KEY (run_id, org_id) REFERENCES data_import_runs(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (import_row_id, org_id) REFERENCES data_import_rows(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (employee_id, org_id) REFERENCES employees(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (branch_id, org_id) REFERENCES branches(id, org_id) ON DELETE RESTRICT
);

CREATE INDEX attendance_direct_import_events_org_branch_date_idx
    ON attendance_direct_import_events (org_id, branch_id, work_date DESC, employee_id);
CREATE INDEX attendance_direct_import_events_org_employee_date_idx
    ON attendance_direct_import_events (org_id, employee_id, work_date DESC);
CREATE INDEX attendance_direct_import_events_run_idx
    ON attendance_direct_import_events (org_id, run_id, source_row);

ALTER TABLE attendance_direct_import_events ENABLE ROW LEVEL SECURITY;
ALTER TABLE attendance_direct_import_events FORCE ROW LEVEL SECURITY;
CREATE POLICY org_isolation ON attendance_direct_import_events
    USING (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid)
    WITH CHECK (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid);


CREATE OR REPLACE FUNCTION attendance_direct_import_events_append_only()
RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP = 'DELETE'
       AND current_setting('app.platform_force_remove_org', true) = 'on' THEN
        RETURN OLD;
    END IF;

    RAISE EXCEPTION
        'attendance_direct_import_events is append-only: % is forbidden (row id=%)',
        TG_OP, OLD.id;
END;
$$;

CREATE TRIGGER trg_attendance_direct_import_events_no_update
    BEFORE UPDATE ON attendance_direct_import_events
    FOR EACH ROW EXECUTE FUNCTION attendance_direct_import_events_append_only();

CREATE TRIGGER trg_attendance_direct_import_events_no_delete
    BEFORE DELETE ON attendance_direct_import_events
    FOR EACH ROW EXECUTE FUNCTION attendance_direct_import_events_append_only();

GRANT SELECT, INSERT ON attendance_direct_import_events TO console_rt;
-- Keep direct attendance import rows compatible with the platform force-remove
-- erasure path without weakening append-only behavior for normal runtime access.
CREATE OR REPLACE FUNCTION data_import_rows_append_only()
RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP = 'DELETE'
       AND current_setting('app.platform_force_remove_org', true) = 'on' THEN
        RETURN OLD;
    END IF;

    RAISE EXCEPTION
        'data_import_rows is append-only: % is forbidden (row id=%)',
        TG_OP, OLD.id;
END;
$$;
-- Keep employee lifecycle rows compatible with the same force-remove erasure
-- window used for other append-only HR/import ledgers.
CREATE OR REPLACE FUNCTION employee_lifecycle_events_append_only()
RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP = 'DELETE'
       AND current_setting('app.platform_force_remove_org', true) = 'on' THEN
        RETURN OLD;
    END IF;

    RAISE EXCEPTION
        'employee_lifecycle_events is append-only: % is forbidden (row id=%)',
        TG_OP, OLD.id
        USING ERRCODE = '55000';
END;
$$;

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

    SELECT status INTO org_status FROM organizations WHERE id = p_id;
    IF NOT FOUND THEN
        SET LOCAL row_security = on;
        RETURN 'not_found';
    END IF;

    IF org_status <> 'ARCHIVED' THEN
        SET LOCAL row_security = on;
        RETURN 'blocked_active';
    END IF;

    PERFORM set_config('app.platform_force_remove_org', 'on', true);
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

REVOKE ALL ON FUNCTION platform_force_remove_organization(UUID) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION platform_force_remove_organization(UUID) TO console_rt;
