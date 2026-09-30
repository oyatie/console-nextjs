-- G016 Policy Studio: tenant-owned custom role definitions and custom-role
-- assignments.
--
-- ACTIVE custom-role assignments are runtime-effective through the central
-- request principal resolver; DRAFT/RETIRED roles remain inert governance data.
-- Custom roles still do not replace users.roles: built-in roles stay in the
-- token/system matrix, while this tenant-scoped, RLS-protected substrate adds
-- audited, FK-validated, versioned feature grants.

-- console-gate: global-table feature_catalog (rationale: canonical feature keys only; no tenant data)
CREATE TABLE feature_catalog (
    feature_key TEXT PRIMARY KEY CHECK (feature_key ~ '^[a-z][a-z0-9_]*$'),
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

INSERT INTO feature_catalog (feature_key) VALUES
    ('login'),
    ('work_order_create'),
    ('work_order_edit_intake'),
    ('work_order_read_all'),
    ('work_order_start'),
    ('work_report_submit'),
    ('evidence_attach'),
    ('priority_manage'),
    ('assignee_manage'),
    ('target_manage'),
    ('completion_review'),
    ('daily_plan_request'),
    ('daily_plan_review'),
    ('org_wide_queue_triage'),
    ('kpi_read'),
    ('kpi_exclusion_manage'),
    ('user_manage'),
    ('subordinate_user_create'),
    ('elevated_role_grant'),
    ('role_manage'),
    ('region_manage'),
    ('branch_manage'),
    ('equipment_manage'),
    ('master_list_import'),
    ('rental_quote_manage'),
    ('equipment_cost_ledger_read'),
    ('equipment_cost_ledger_write'),
    ('purchase_request_create'),
    ('purchase_request_read'),
    ('purchase_request_approve'),
    ('purchase_final_approve'),
    ('purchase_execute'),
    ('inspection_schedule_manage'),
    ('inspection_round_complete'),
    ('audit_log_read'),
    ('excel_download'),
    ('ops_dashboard_read'),
    ('sales_manage'),
    ('ai_assist'),
    ('integrity_findings_read'),
    ('integrity_finding_triage'),
    ('mail_account_manage'),
    ('mail_use'),
    ('employee_directory_read'),
    ('employee_directory_manage')
ON CONFLICT (feature_key) DO NOTHING;

REVOKE ALL ON feature_catalog FROM PUBLIC;
GRANT SELECT ON feature_catalog TO console_rt;

-- console-gate: audited-table policy_roles
CREATE TABLE policy_roles (
    id           UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id       UUID        NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    role_key     TEXT        NOT NULL CHECK (role_key ~ '^[a-z][a-z0-9_]+$'),
    display_name TEXT        NOT NULL CHECK (char_length(display_name) BETWEEN 1 AND 80),
    description  TEXT        NULL CHECK (description IS NULL OR char_length(description) <= 512),
    status       TEXT        NOT NULL DEFAULT 'DRAFT' CHECK (status IN ('DRAFT','ACTIVE','RETIRED')),
    is_system    BOOLEAN     NOT NULL DEFAULT false,
    created_by   UUID        NULL,
    updated_by   UUID        NULL,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (id, org_id),
    UNIQUE (org_id, role_key),
    FOREIGN KEY (created_by) REFERENCES users(id) ON DELETE SET NULL,
    FOREIGN KEY (updated_by) REFERENCES users(id) ON DELETE SET NULL
);
CREATE INDEX idx_policy_roles_org ON policy_roles (org_id, status, role_key);

-- console-gate: audited-table policy_role_permissions
CREATE TABLE policy_role_permissions (
    id               UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id           UUID        NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    role_id          UUID        NOT NULL,
    feature_key      TEXT        NOT NULL REFERENCES feature_catalog(feature_key) ON DELETE RESTRICT,
    permission_level TEXT        NOT NULL CHECK (permission_level IN ('deny','request_only','limited','allow')),
    created_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (role_id, feature_key),
    FOREIGN KEY (role_id, org_id) REFERENCES policy_roles(id, org_id) ON DELETE CASCADE
);
CREATE INDEX idx_policy_role_permissions_org_role ON policy_role_permissions (org_id, role_id);

-- Optional ABAC/PBAC condition metadata attached to a custom role definition.
-- The runtime resolver currently consumes branch equals/in conditions as
-- fail-closed scope narrowers; unsupported conditions remain review/audit
-- metadata until a richer evaluator lands.
-- console-gate: audited-table policy_role_conditions
CREATE TABLE policy_role_conditions (
    id                UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id            UUID        NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    role_id           UUID        NOT NULL,
    condition_key     TEXT        NOT NULL CHECK (condition_key ~ '^[a-z][a-z0-9_]{1,63}$'),
    attribute         TEXT        NOT NULL CHECK (attribute IN (
        'group',
        'tenant',
        'organization',
        'org',
        'department',
        'team',
        'position',
        'employment_status',
        'assignment',
        'location',
        'site',
        'branch',
        'device_posture',
        'purpose',
        'action',
        'resource',
        'sensitive_action'
    )),
    operator          TEXT        NOT NULL CHECK (operator IN ('equals','not_equals','in')),
    condition_values  TEXT[]      NOT NULL CHECK (cardinality(condition_values) BETWEEN 1 AND 20),
    created_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (role_id, condition_key),
    FOREIGN KEY (role_id, org_id) REFERENCES policy_roles(id, org_id) ON DELETE CASCADE
);
CREATE INDEX idx_policy_role_conditions_org_role ON policy_role_conditions (org_id, role_id);

-- Custom-role assignments. ACTIVE assigned roles are runtime-effective via the
-- central authz resolver; DRAFT/RETIRED assigned roles do not grant authority.
-- Assignments do not change users.roles.
-- console-gate: audited-table user_role_assignments
CREATE TABLE user_role_assignments (
    id          UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id      UUID        NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    user_id     UUID        NOT NULL,
    role_id     UUID        NOT NULL,
    assigned_by UUID        NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (org_id, user_id, role_id),
    FOREIGN KEY (user_id, org_id) REFERENCES users(id, org_id) ON DELETE CASCADE,
    FOREIGN KEY (role_id, org_id) REFERENCES policy_roles(id, org_id) ON DELETE CASCADE,
    FOREIGN KEY (assigned_by) REFERENCES users(id) ON DELETE SET NULL
);
CREATE INDEX idx_user_role_assignments_org_user ON user_role_assignments (org_id, user_id);

-- Short-lived server-side receipts proving the actor reviewed an assignment
-- impact preview for the exact target user and role set before attempting the
-- passkey-gated replacement write.
-- console-gate: audited-table policy_assignment_preview_receipts
CREATE TABLE policy_assignment_preview_receipts (
    id                 UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id             UUID        NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    actor_id           UUID        NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    user_id            UUID        NOT NULL,
    current_branch_ids UUID[]      NOT NULL,
    current_role_ids   UUID[]      NOT NULL,
    role_ids           UUID[]      NOT NULL,
    policy_version     BIGINT      NOT NULL CHECK (policy_version >= 0),
    expires_at         TIMESTAMPTZ NOT NULL,
    consumed_at        TIMESTAMPTZ NULL,
    created_at         TIMESTAMPTZ NOT NULL DEFAULT now(),
    FOREIGN KEY (user_id, org_id) REFERENCES users(id, org_id) ON DELETE CASCADE
);
CREATE INDEX idx_policy_assignment_preview_receipts_lookup
    ON policy_assignment_preview_receipts (org_id, actor_id, user_id, expires_at);

-- Per-org policy version. The first write inserts version 1, every role write bumps it.
CREATE TABLE policy_versions (
    org_id     UUID        PRIMARY KEY REFERENCES organizations(id) ON DELETE CASCADE,
    version    BIGINT      NOT NULL DEFAULT 1 CHECK (version >= 1),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

DO $$
DECLARE
    t TEXT;
    tenant_tables TEXT[] := ARRAY[
        'policy_roles',
        'policy_role_permissions',
        'policy_role_conditions',
        'user_role_assignments',
        'policy_assignment_preview_receipts',
        'policy_versions'
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
        EXECUTE format('GRANT SELECT, INSERT, UPDATE, DELETE ON %I TO console_rt', t);
    END LOOP;
END
$$;

CREATE TRIGGER trg_policy_roles_org_immutable
    BEFORE UPDATE ON policy_roles
    FOR EACH ROW EXECUTE FUNCTION enforce_org_id_immutable();
CREATE TRIGGER trg_policy_role_permissions_org_immutable
    BEFORE UPDATE ON policy_role_permissions
    FOR EACH ROW EXECUTE FUNCTION enforce_org_id_immutable();
CREATE TRIGGER trg_policy_role_conditions_org_immutable
    BEFORE UPDATE ON policy_role_conditions
    FOR EACH ROW EXECUTE FUNCTION enforce_org_id_immutable();
CREATE TRIGGER trg_user_role_assignments_org_immutable
    BEFORE UPDATE ON user_role_assignments
    FOR EACH ROW EXECUTE FUNCTION enforce_org_id_immutable();
CREATE TRIGGER trg_policy_assignment_preview_receipts_org_immutable
    BEFORE UPDATE ON policy_assignment_preview_receipts
    FOR EACH ROW EXECUTE FUNCTION enforce_org_id_immutable();
