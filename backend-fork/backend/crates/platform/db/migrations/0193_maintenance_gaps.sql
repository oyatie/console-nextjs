-- Maintenance history is a distinct, append-only operational record. It is not
-- an equipment-registry version: a completed work order records an auditable
-- maintenance event, while registry_equipment_versions remains reserved for
-- snapshots of the equipment aggregate itself.
--
-- The work-order adapter writes this history only after the existing final
-- completion FSM transition. The database repeats that terminal/evidence
-- interlock so direct writers cannot create a premature maintenance record.

-- The older cost ledger predates composite tenant foreign keys. Add the key
-- required to make an immutable, tenant-pinned reference from the history
-- snapshot without copying or fabricating any monetary value.
ALTER TABLE equipment_cost_ledger
    ADD CONSTRAINT equipment_cost_ledger_id_org_key UNIQUE (id, org_id);

-- console-gate: audited-table equipment_maintenance_history
CREATE TABLE equipment_maintenance_history (
    id            UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id        UUID        NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    equipment_id  UUID        NOT NULL,
    work_order_id UUID        NOT NULL,
    completed_at  TIMESTAMPTZ NOT NULL,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (id, org_id),
    UNIQUE (org_id, work_order_id),
    CONSTRAINT equipment_maintenance_history_equipment_same_org_fk
        FOREIGN KEY (equipment_id, org_id)
        REFERENCES registry_equipment (id, org_id) ON DELETE RESTRICT,
    CONSTRAINT equipment_maintenance_history_work_order_same_org_fk
        FOREIGN KEY (work_order_id, org_id)
        REFERENCES work_orders (id, org_id) ON DELETE RESTRICT
);

CREATE INDEX idx_equipment_maintenance_history_equipment
    ON equipment_maintenance_history (org_id, equipment_id, completed_at DESC);

-- The evidence references comprise the exact AFTER/REPORT evidence set that
-- passed the final-completion interlock. They remain append-only even if
-- subsequent operational remediation creates additional evidence elsewhere.
CREATE TABLE equipment_maintenance_history_evidence (
    history_id        UUID NOT NULL,
    org_id            UUID NOT NULL,
    evidence_media_id UUID NOT NULL,
    PRIMARY KEY (history_id, evidence_media_id),
    CONSTRAINT equipment_maintenance_history_evidence_history_same_org_fk
        FOREIGN KEY (history_id, org_id)
        REFERENCES equipment_maintenance_history (id, org_id) ON DELETE RESTRICT,
    CONSTRAINT equipment_maintenance_history_evidence_media_same_org_fk
        FOREIGN KEY (evidence_media_id, org_id)
        REFERENCES evidence_media (id, org_id) ON DELETE RESTRICT
);

CREATE INDEX idx_equipment_maintenance_history_evidence_org
    ON equipment_maintenance_history_evidence (org_id, evidence_media_id);

-- A cost reference is optional. It only links an already-posted cost ledger
-- entry for this work order/equipment; it never stores a new amount or creates
-- a ledger movement as a side effect of completion.
CREATE TABLE equipment_maintenance_history_costs (
    history_id                 UUID NOT NULL,
    org_id                     UUID NOT NULL,
    equipment_cost_ledger_id   UUID NOT NULL,
    PRIMARY KEY (history_id, equipment_cost_ledger_id),
    CONSTRAINT equipment_maintenance_history_costs_history_same_org_fk
        FOREIGN KEY (history_id, org_id)
        REFERENCES equipment_maintenance_history (id, org_id) ON DELETE RESTRICT,
    CONSTRAINT equipment_maintenance_history_costs_ledger_same_org_fk
        FOREIGN KEY (equipment_cost_ledger_id, org_id)
        REFERENCES equipment_cost_ledger (id, org_id) ON DELETE RESTRICT
);

CREATE INDEX idx_equipment_maintenance_history_costs_org
    ON equipment_maintenance_history_costs (org_id, equipment_cost_ledger_id);

CREATE OR REPLACE FUNCTION equipment_maintenance_history_require_final_completion()
RETURNS TRIGGER
LANGUAGE plpgsql
AS $$
BEGIN
    IF NOT EXISTS (
        SELECT 1
        FROM work_orders w
        WHERE w.id = NEW.work_order_id
          AND w.org_id = NEW.org_id
          AND w.equipment_id = NEW.equipment_id
          AND w.status = 'FINAL_COMPLETED'
          AND w.result_type = 'COMPLETED'
          AND EXISTS (
              SELECT 1
              FROM evidence_media e
              WHERE e.work_order_id = w.id
                AND e.org_id = w.org_id
                AND e.stage IN ('AFTER', 'REPORT')
                AND e.worm_replica_status = 'VERIFIED'
          )
          AND NOT EXISTS (
              SELECT 1
              FROM evidence_media e
              WHERE e.work_order_id = w.id
                AND e.org_id = w.org_id
                AND e.stage IN ('AFTER', 'REPORT')
                AND e.worm_replica_status <> 'VERIFIED'
          )
    ) THEN
        RAISE EXCEPTION
            'maintenance history requires a FINAL_COMPLETED work order with a verified completion evidence set'
            USING ERRCODE = 'check_violation';
    END IF;
    RETURN NEW;
END;
$$;

CREATE OR REPLACE FUNCTION equipment_maintenance_history_evidence_require_verified_snapshot()
RETURNS TRIGGER
LANGUAGE plpgsql
AS $$
BEGIN
    IF NOT EXISTS (
        SELECT 1
        FROM equipment_maintenance_history h
        JOIN evidence_media e
          ON e.id = NEW.evidence_media_id
         AND e.org_id = NEW.org_id
        WHERE h.id = NEW.history_id
          AND h.org_id = NEW.org_id
          AND e.work_order_id = h.work_order_id
          AND e.stage IN ('AFTER', 'REPORT')
          AND e.worm_replica_status = 'VERIFIED'
    ) THEN
        RAISE EXCEPTION
            'maintenance history evidence must be verified completion evidence for the recorded work order'
            USING ERRCODE = 'check_violation';
    END IF;
    RETURN NEW;
END;
$$;

CREATE OR REPLACE FUNCTION equipment_maintenance_history_cost_require_matching_ledger()
RETURNS TRIGGER
LANGUAGE plpgsql
AS $$
BEGIN
    IF NOT EXISTS (
        SELECT 1
        FROM equipment_maintenance_history h
        JOIN equipment_cost_ledger l
          ON l.id = NEW.equipment_cost_ledger_id
         AND l.org_id = NEW.org_id
        WHERE h.id = NEW.history_id
          AND h.org_id = NEW.org_id
          AND l.equipment_id = h.equipment_id
          AND l.work_order_id = h.work_order_id
    ) THEN
        RAISE EXCEPTION
            'maintenance history costs must reference an existing ledger entry for the recorded equipment and work order'
            USING ERRCODE = 'check_violation';
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER trg_equipment_maintenance_history_final_completion
    BEFORE INSERT ON equipment_maintenance_history
    FOR EACH ROW EXECUTE FUNCTION equipment_maintenance_history_require_final_completion();
CREATE TRIGGER trg_equipment_maintenance_history_evidence_verified_snapshot
    BEFORE INSERT ON equipment_maintenance_history_evidence
    FOR EACH ROW EXECUTE FUNCTION equipment_maintenance_history_evidence_require_verified_snapshot();
CREATE TRIGGER trg_equipment_maintenance_history_cost_matching_ledger
    BEFORE INSERT ON equipment_maintenance_history_costs
    FOR EACH ROW EXECUTE FUNCTION equipment_maintenance_history_cost_require_matching_ledger();

-- Every history relation is append-only; corrections are new work-order events,
-- never mutation or deletion of the completed evidence/cost snapshot.
CREATE TRIGGER trg_equipment_maintenance_history_no_update
    BEFORE UPDATE ON equipment_maintenance_history
    FOR EACH ROW EXECUTE FUNCTION platform_append_only_immutable();
CREATE TRIGGER trg_equipment_maintenance_history_no_delete
    BEFORE DELETE ON equipment_maintenance_history
    FOR EACH ROW EXECUTE FUNCTION platform_append_only_immutable();
CREATE TRIGGER trg_equipment_maintenance_history_evidence_no_update
    BEFORE UPDATE ON equipment_maintenance_history_evidence
    FOR EACH ROW EXECUTE FUNCTION platform_append_only_immutable();
CREATE TRIGGER trg_equipment_maintenance_history_evidence_no_delete
    BEFORE DELETE ON equipment_maintenance_history_evidence
    FOR EACH ROW EXECUTE FUNCTION platform_append_only_immutable();
CREATE TRIGGER trg_equipment_maintenance_history_costs_no_update
    BEFORE UPDATE ON equipment_maintenance_history_costs
    FOR EACH ROW EXECUTE FUNCTION platform_append_only_immutable();
CREATE TRIGGER trg_equipment_maintenance_history_costs_no_delete
    BEFORE DELETE ON equipment_maintenance_history_costs
    FOR EACH ROW EXECUTE FUNCTION platform_append_only_immutable();

ALTER TABLE equipment_maintenance_history ENABLE ROW LEVEL SECURITY;
ALTER TABLE equipment_maintenance_history FORCE ROW LEVEL SECURITY;
CREATE POLICY org_isolation ON equipment_maintenance_history
    USING (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid)
    WITH CHECK (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid);

ALTER TABLE equipment_maintenance_history_evidence ENABLE ROW LEVEL SECURITY;
ALTER TABLE equipment_maintenance_history_evidence FORCE ROW LEVEL SECURITY;
CREATE POLICY org_isolation ON equipment_maintenance_history_evidence
    USING (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid)
    WITH CHECK (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid);

ALTER TABLE equipment_maintenance_history_costs ENABLE ROW LEVEL SECURITY;
ALTER TABLE equipment_maintenance_history_costs FORCE ROW LEVEL SECURITY;
CREATE POLICY org_isolation ON equipment_maintenance_history_costs
    USING (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid)
    WITH CHECK (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid);

GRANT SELECT, INSERT ON equipment_maintenance_history TO console_rt;
GRANT SELECT, INSERT ON equipment_maintenance_history_evidence TO console_rt;
GRANT SELECT, INSERT ON equipment_maintenance_history_costs TO console_rt;


-- Only the archived-tenant force-removal procedure may bypass append-only deletion.
CREATE OR REPLACE FUNCTION equipment_maintenance_history_delete_guard()
RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
  IF current_setting('app.maintenance_force_remove', true) = 'on' THEN RETURN OLD; END IF;
  RETURN platform_append_only_immutable();
END; $$;
DROP TRIGGER trg_equipment_maintenance_history_no_delete ON equipment_maintenance_history;
DROP TRIGGER trg_equipment_maintenance_history_evidence_no_delete ON equipment_maintenance_history_evidence;
DROP TRIGGER trg_equipment_maintenance_history_costs_no_delete ON equipment_maintenance_history_costs;
CREATE TRIGGER trg_equipment_maintenance_history_no_delete BEFORE DELETE ON equipment_maintenance_history FOR EACH ROW EXECUTE FUNCTION equipment_maintenance_history_delete_guard();
CREATE TRIGGER trg_equipment_maintenance_history_evidence_no_delete BEFORE DELETE ON equipment_maintenance_history_evidence FOR EACH ROW EXECUTE FUNCTION equipment_maintenance_history_delete_guard();
CREATE TRIGGER trg_equipment_maintenance_history_costs_no_delete BEFORE DELETE ON equipment_maintenance_history_costs FOR EACH ROW EXECUTE FUNCTION equipment_maintenance_history_delete_guard();

-- Redefine the latest force-removal procedure with only the maintenance additions.
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

REVOKE ALL ON FUNCTION platform_force_remove_organization(UUID) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION platform_force_remove_organization(UUID) TO console_rt;

-- Runtime callers cannot build a partial snapshot: this single, org-fenced
-- operation inserts the parent and its complete immutable reference sets.
CREATE OR REPLACE FUNCTION append_equipment_maintenance_history(
    p_org_id UUID, p_equipment_id UUID, p_work_order_id UUID, p_completed_at TIMESTAMPTZ
) RETURNS UUID
LANGUAGE plpgsql SECURITY DEFINER SET search_path = public, pg_temp AS $$
DECLARE v_history_id UUID;
BEGIN
  IF p_org_id IS DISTINCT FROM NULLIF(current_setting('app.current_org', true), '')::uuid THEN
    RAISE EXCEPTION 'maintenance history org does not match runtime org' USING ERRCODE = 'insufficient_privilege';
  END IF;
  INSERT INTO equipment_maintenance_history (org_id,equipment_id,work_order_id,completed_at)
  VALUES (p_org_id,p_equipment_id,p_work_order_id,p_completed_at) RETURNING id INTO v_history_id;
  INSERT INTO equipment_maintenance_history_evidence (history_id,org_id,evidence_media_id)
  SELECT v_history_id,p_org_id,e.id FROM evidence_media e
  WHERE e.org_id=p_org_id AND e.work_order_id=p_work_order_id
    AND e.stage IN ('AFTER','REPORT') AND e.worm_replica_status='VERIFIED';
  INSERT INTO equipment_maintenance_history_costs (history_id,org_id,equipment_cost_ledger_id)
  SELECT v_history_id,p_org_id,l.id FROM equipment_cost_ledger l
  WHERE l.org_id=p_org_id AND l.work_order_id=p_work_order_id AND l.equipment_id=p_equipment_id;
  RETURN v_history_id;
END; $$;
REVOKE INSERT ON equipment_maintenance_history, equipment_maintenance_history_evidence, equipment_maintenance_history_costs FROM console_rt;
REVOKE ALL ON FUNCTION append_equipment_maintenance_history(UUID, UUID, UUID, TIMESTAMPTZ) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION append_equipment_maintenance_history(UUID, UUID, UUID, TIMESTAMPTZ) TO console_rt;
