-- Dedicated authorization and forward-compatible direct-tenant FK closure for
-- destructive platform tenant removal.  The caller receives only EXECUTE on
-- this one command; it is never available through the general console_rt pool.
DO $block$
DECLARE
    v_owner OID := pg_catalog.to_regrole('console_app');
    v_runtime OID := pg_catalog.to_regrole('console_rt');
    v_command OID := pg_catalog.to_regrole('console_platform_force_cmd');
    v_applier OID;
    v_applier_is_superuser BOOLEAN;
    v_database_owner OID;
    v_expected_owner OID;
    v_sqlx_test_bootstrap TEXT;
BEGIN
    IF v_owner IS NULL OR v_runtime IS NULL OR v_command IS NULL THEN
        RAISE EXCEPTION USING ERRCODE = '42501',
            MESSAGE = 'platform_force_role_topology.roles_not_preprovisioned';
    END IF;
    SELECT oid, rolsuper INTO v_applier, v_applier_is_superuser
    FROM pg_catalog.pg_roles
    WHERE rolname = CURRENT_USER;
    SELECT datdba INTO v_database_owner
    FROM pg_catalog.pg_database
    WHERE datname = CURRENT_DATABASE();
    v_sqlx_test_bootstrap :=
        pg_catalog.current_setting('console.sqlx_test_bootstrap', true);

    IF v_applier_is_superuser THEN
        -- Buck's disposable PostgreSQL harness puts an explicit startup marker
        -- only on the SQLx test process connection. SQLx then creates a
        -- deterministically named per-test database. Require both invariants,
        -- the dedicated disposable admin identity, and aligned database
        -- ownership; a production DBA session cannot match this accidentally.
        IF SESSION_USER <> CURRENT_USER
           OR CURRENT_USER <> 'console_buck_admin'
           OR v_sqlx_test_bootstrap IS DISTINCT FROM 'buck-sqlx-superuser-v1'
           OR CURRENT_DATABASE() !~ '^_sqlx_test_[A-Za-z0-9_]{52}$'
           OR v_database_owner <> v_applier
        THEN
            RAISE EXCEPTION USING ERRCODE = '42501',
                MESSAGE = 'platform_force_role_topology.superuser_test_bootstrap_required';
        END IF;
        v_expected_owner := v_applier;
    ELSE
        IF CURRENT_USER <> 'console_app' OR SESSION_USER <> 'console_app'
           OR v_database_owner <> v_owner THEN
            RAISE EXCEPTION USING ERRCODE = '42501',
                MESSAGE = 'platform_force_role_topology.console_app_must_apply_directly';
        END IF;
        v_expected_owner := v_owner;
    END IF;

    -- The definer deletes every public tenant relation carrying org_id,
    -- deletes organizations itself, and deletes WebAuthn ceremonies indirectly
    -- through users. A partial owner drift would make force removal fail only
    -- after it had begun; reject the migration before installing the definers.
    IF EXISTS (
        SELECT 1
        FROM pg_catalog.pg_class AS relation
        JOIN pg_catalog.pg_namespace AS namespace
          ON namespace.oid = relation.relnamespace
        WHERE namespace.nspname = 'public'
          AND relation.relkind IN ('r', 'p')
          AND (
              relation.relname IN ('organizations', 'auth_webauthn_ceremonies')
              OR EXISTS (
                  SELECT 1
                  FROM pg_catalog.pg_attribute AS attribute
                  WHERE attribute.attrelid = relation.oid
                    AND attribute.attname = 'org_id'
                    AND NOT attribute.attisdropped
              )
          )
          AND relation.relowner <> v_expected_owner
    ) THEN
        RAISE EXCEPTION USING ERRCODE = '42501',
            MESSAGE = 'platform_force_role_topology.force_table_owner_drift';
    END IF;
    IF EXISTS (
        SELECT 1 FROM pg_catalog.pg_roles
        WHERE oid = v_owner
          AND (NOT rolcanlogin OR NOT rolinherit OR rolsuper OR NOT rolbypassrls
               OR rolcreatedb OR rolcreaterole OR rolreplication)
    ) THEN
        RAISE EXCEPTION USING ERRCODE = '42501',
            MESSAGE = 'platform_force_role_topology.console_app_not_hardened';
    END IF;
    IF EXISTS (
        SELECT 1 FROM pg_catalog.pg_roles
        WHERE oid = v_command
          AND (NOT rolcanlogin OR rolsuper OR rolbypassrls OR rolinherit
               OR rolcreatedb OR rolcreaterole OR rolreplication)
    ) THEN
        RAISE EXCEPTION USING ERRCODE = '42501',
            MESSAGE = 'platform_force_role_topology.command_not_hardened';
    END IF;
    IF EXISTS (
        SELECT 1 FROM pg_catalog.pg_auth_members
        WHERE member = v_command OR roleid = v_command
    ) THEN
        RAISE EXCEPTION USING ERRCODE = '42501',
            MESSAGE = 'platform_force_role_topology.command_membership_forbidden';
    END IF;
END
$block$;

-- The direct-org-child closure is data-driven from pg_constraint, not a stale
-- handwritten list.  It deliberately handles only direct organization FKs;
-- the established explicit child-first deletions above it preserve specialized
-- guards/audit behavior for indirect graphs and immutable records.
CREATE OR REPLACE FUNCTION platform_force_remove_direct_org_children(p_id UUID)
RETURNS VOID
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = public, pg_temp
AS $$
DECLARE
    target RECORD;
BEGIN
    FOR target IN
        SELECT child_ns.nspname AS schema_name, child.relname AS relation_name
        FROM pg_catalog.pg_constraint AS fk
        JOIN pg_catalog.pg_class AS child ON child.oid = fk.conrelid
        JOIN pg_catalog.pg_namespace AS child_ns ON child_ns.oid = child.relnamespace
        JOIN pg_catalog.pg_class AS parent ON parent.oid = fk.confrelid
        JOIN pg_catalog.pg_namespace AS parent_ns ON parent_ns.oid = parent.relnamespace
        JOIN pg_catalog.pg_attribute AS child_attr
          ON child_attr.attrelid = child.oid
         AND child_attr.attnum = fk.conkey[1]
         AND NOT child_attr.attisdropped
        WHERE fk.contype = 'f'
          AND fk.confdeltype IN ('a', 'r')
          AND parent_ns.nspname = 'public'
          AND parent.relname = 'organizations'
          AND child_ns.nspname = 'public'
          AND child.relkind IN ('r', 'p')
          -- These roots have specialized ordering: the audit ledger must be
          -- re-homed, and employee/user/branch/region references are released
          -- only after their direct children have been closed by this pass.
          AND child.relname NOT IN (
              'audit_events', 'employees', 'users', 'branches', 'regions'
          )
          AND cardinality(fk.conkey) = 1
          AND child_attr.attname = 'org_id'
        -- New tenant-facing tables normally reference older roots.  Descending
        -- OID gives those children priority before their direct parents.
        ORDER BY child.oid DESC
    LOOP
        EXECUTE format('DELETE FROM %I.%I WHERE org_id = $1',
                       target.schema_name, target.relation_name)
            USING p_id;
    END LOOP;
END;
$$;
-- Keep the migration identity as owner. Production migrations run directly as
-- console_app; isolated SQLx databases run as their own table owner. Reassigning
-- only this SECURITY DEFINER to console_app would separate it from the tables it
-- must delete under FORCE RLS and break the command with insufficient_privilege.
REVOKE ALL ON FUNCTION platform_force_remove_direct_org_children(UUID) FROM PUBLIC, console_rt, console_platform_force_cmd;

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
REVOKE ALL ON FUNCTION platform_force_remove_organization(UUID) FROM PUBLIC, console_rt;
GRANT EXECUTE ON FUNCTION platform_force_remove_organization(UUID) TO console_platform_force_cmd;

-- A new direct tenant FK must be consumable by the closure. Fail migration
-- rather than shipping a force-remove function that can only fail at operator
-- time. Composite/non-org_id organization references need an explicit,
-- separately reviewed child-first deletion.
DO $block$
BEGIN
    IF EXISTS (
        SELECT 1
        FROM pg_catalog.pg_constraint AS fk
        JOIN pg_catalog.pg_class AS child ON child.oid = fk.conrelid
        JOIN pg_catalog.pg_namespace AS child_ns ON child_ns.oid = child.relnamespace
        JOIN pg_catalog.pg_class AS parent ON parent.oid = fk.confrelid
        JOIN pg_catalog.pg_namespace AS parent_ns ON parent_ns.oid = parent.relnamespace
        LEFT JOIN pg_catalog.pg_attribute AS child_attr
          ON child_attr.attrelid = child.oid
         AND child_attr.attnum = fk.conkey[1]
         AND NOT child_attr.attisdropped
        WHERE fk.contype = 'f'
          AND fk.confdeltype IN ('a', 'r')
          AND parent_ns.nspname = 'public'
          AND parent.relname = 'organizations'
          AND child_ns.nspname = 'public'
          AND (cardinality(fk.conkey) <> 1 OR child_attr.attname IS DISTINCT FROM 'org_id')
    ) THEN
        RAISE EXCEPTION USING ERRCODE = '23503',
            MESSAGE = 'platform_force_remove.direct_org_fk_requires_explicit_closure';
    END IF;
END
$block$;


-- Atomic receipt command: the command login has no table privileges. It can
-- only invoke this pinned definer, which reads the target, removes it, and
-- appends the platform-level immutable receipt in the same transaction.
CREATE OR REPLACE FUNCTION platform_force_remove_organization_command(
    p_id UUID, p_actor UUID, p_trace_id CHAR(32), p_span_id CHAR(16), p_occurred_at TIMESTAMPTZ
) RETURNS TEXT
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = public, pg_temp
AS $$
DECLARE
    v_slug TEXT;
    v_name TEXT;
    v_outcome TEXT;
    v_wiped JSONB;
BEGIN
    SET LOCAL row_security = off;
    SELECT slug, name INTO v_slug, v_name FROM organizations WHERE id = p_id;
    SELECT jsonb_build_object(
        'users', (SELECT count(*) FROM users WHERE org_id = p_id),
        'registry_customers', (SELECT count(*) FROM registry_customers WHERE org_id = p_id),
        'registry_sites', (SELECT count(*) FROM registry_sites WHERE org_id = p_id),
        'registry_equipment', (SELECT count(*) FROM registry_equipment WHERE org_id = p_id),
        'work_orders', (SELECT count(*) FROM work_orders WHERE org_id = p_id),
        'financial_rental_quotes', (SELECT count(*) FROM financial_rental_quotes WHERE org_id = p_id),
        'financial_purchase_requests', (SELECT count(*) FROM financial_purchase_requests WHERE org_id = p_id),
        'messenger_threads', (SELECT count(*) FROM messenger_threads WHERE org_id = p_id),
        'evidence_media', (SELECT count(*) FROM evidence_media WHERE org_id = p_id),
        'audit_events', (SELECT count(*) FROM audit_events WHERE org_id = p_id)
    ) INTO v_wiped;
    v_outcome := platform_force_remove_organization(p_id);
    IF v_outcome <> 'removed' THEN
        RETURN v_outcome;
    END IF;
    INSERT INTO audit_events (
        org_id, actor, action, target_type, target_id, branch_id,
        before_snap, after_snap, trace_id, span_id, occurred_at
    ) VALUES (
        NULL, p_actor, 'platform.tenant.force_remove', 'organizations', p_id::text, NULL,
        jsonb_build_object('org_id', p_id, 'slug', v_slug, 'name', v_name, 'wiped', v_wiped), NULL,
        p_trace_id, p_span_id, p_occurred_at
    );
    RETURN 'removed';
END;
$$;
REVOKE ALL ON FUNCTION platform_force_remove_organization_command(UUID, UUID, CHAR(32), CHAR(16), TIMESTAMPTZ) FROM PUBLIC, console_rt;
GRANT EXECUTE ON FUNCTION platform_force_remove_organization_command(UUID, UUID, CHAR(32), CHAR(16), TIMESTAMPTZ) TO console_platform_force_cmd;
REVOKE EXECUTE ON FUNCTION platform_force_remove_organization(UUID) FROM console_platform_force_cmd;
