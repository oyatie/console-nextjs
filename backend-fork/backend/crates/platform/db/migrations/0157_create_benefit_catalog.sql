-- renamed by L-WIRE 2026-07-10 to resolve version collision (was 0103_create_benefit_catalog).
-- Benefit catalog module backend (BF-): tenant-scoped legal/extra benefit rows,
-- tier details, and eligibility/condition rows. Runtime deletes are deliberately
-- not granted; child replacement retires prior rows logically.

INSERT INTO feature_catalog (feature_key) VALUES
    ('benefit_catalog_read'),
    ('benefit_catalog_manage')
ON CONFLICT (feature_key) DO NOTHING;

CREATE TABLE benefit_code_counters (
    org_id        UUID        NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    object_prefix TEXT        NOT NULL CHECK (object_prefix = 'BF'),
    next_value    BIGINT      NOT NULL DEFAULT 1 CHECK (next_value >= 1),
    updated_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (org_id, object_prefix)
);

-- console-gate: audited-table benefit_catalog_items
CREATE TABLE benefit_catalog_items (
    id                        UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id                    UUID        NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    benefit_code              TEXT        NOT NULL CHECK (benefit_code ~ '^BF-[0-9]{4,}$'),
    category                  TEXT        NOT NULL CHECK (category IN ('LEGAL','EXTRA')),
    name                      TEXT        NOT NULL CHECK (btrim(name) <> '' AND char_length(name) <= 120),
    scope_type                TEXT        NOT NULL DEFAULT 'ORG' CHECK (scope_type IN ('ORG','BRANCH','SITE','TEAM','ROLE','EMPLOYEE_SEGMENT')),
    scope_ref                 UUID        NULL,
    branch_id                 UUID        NULL,
    site_id                   UUID        NULL,
    coverage_label            TEXT        NOT NULL CHECK (btrim(coverage_label) <> '' AND char_length(coverage_label) <= 80),
    covered_count             INTEGER     NULL CHECK (covered_count IS NULL OR covered_count >= 0),
    cost_label                TEXT        NOT NULL CHECK (btrim(cost_label) <> '' AND char_length(cost_label) <= 80),
    estimated_annual_cost_won BIGINT      NULL CHECK (estimated_annual_cost_won IS NULL OR estimated_annual_cost_won >= 0),
    employer_rate_bps         INTEGER     NULL CHECK (employer_rate_bps IS NULL OR employer_rate_bps BETWEEN 0 AND 10000),
    note                      TEXT        NULL CHECK (note IS NULL OR char_length(note) <= 500),
    legal_basis               TEXT        NULL CHECK (legal_basis IS NULL OR char_length(legal_basis) <= 300),
    related_domain            TEXT        NULL CHECK (related_domain IS NULL OR related_domain ~ '^[a-z][a-z0-9_]{1,63}$'),
    related_object_id         UUID        NULL,
    effective_on              DATE        NULL,
    retires_on                DATE        NULL CHECK (retires_on IS NULL OR effective_on IS NULL OR retires_on >= effective_on),
    display_order             INTEGER     NOT NULL DEFAULT 0,
    metadata                  JSONB       NOT NULL DEFAULT '{}'::jsonb CHECK (jsonb_typeof(metadata) = 'object'),
    created_by                UUID        NOT NULL,
    updated_by                UUID        NOT NULL,
    created_at                TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at                TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (id, org_id),
    UNIQUE (org_id, benefit_code),
    FOREIGN KEY (branch_id, org_id) REFERENCES branches(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (site_id, org_id) REFERENCES registry_sites(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (created_by, org_id) REFERENCES users(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (updated_by, org_id) REFERENCES users(id, org_id) ON DELETE RESTRICT,
    CHECK (
        (scope_type = 'ORG' AND scope_ref IS NULL AND branch_id IS NULL AND site_id IS NULL)
        OR (scope_type = 'BRANCH' AND branch_id IS NOT NULL AND scope_ref = branch_id AND site_id IS NULL)
        OR (scope_type = 'SITE' AND site_id IS NOT NULL AND branch_id IS NOT NULL AND scope_ref = site_id)
        OR (scope_type IN ('TEAM','ROLE','EMPLOYEE_SEGMENT') AND scope_ref IS NOT NULL)
    )
);

CREATE INDEX idx_benefit_catalog_items_category_order
    ON benefit_catalog_items (org_id, category, display_order, name);
CREATE INDEX idx_benefit_catalog_items_scope
    ON benefit_catalog_items (org_id, scope_type, scope_ref);
CREATE INDEX idx_benefit_catalog_items_branch_site
    ON benefit_catalog_items (org_id, branch_id, site_id)
    WHERE branch_id IS NOT NULL;
CREATE UNIQUE INDEX idx_benefit_catalog_items_natural_unique
    ON benefit_catalog_items (
        org_id,
        category,
        lower(btrim(name)),
        scope_type,
        COALESCE(scope_ref, '00000000-0000-0000-0000-000000000000'::uuid)
    );

-- console-gate: audited-table benefit_catalog_tiers
CREATE TABLE benefit_catalog_tiers (
    id            UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id        UUID        NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    benefit_id    UUID        NOT NULL,
    tier_basis    TEXT        NOT NULL CHECK (btrim(tier_basis) <> '' AND char_length(tier_basis) <= 80),
    tier_key      TEXT        NOT NULL CHECK (btrim(tier_key) <> '' AND char_length(tier_key) <= 120),
    value_label   TEXT        NOT NULL CHECK (btrim(value_label) <> '' AND char_length(value_label) <= 300),
    amount_won    BIGINT      NULL CHECK (amount_won IS NULL OR amount_won >= 0),
    limit_period  TEXT        NULL CHECK (limit_period IS NULL OR limit_period IN ('MONTH','QUARTER','YEAR','EVENT','TENURE_MILESTONE')),
    criteria      JSONB       NOT NULL DEFAULT '{}'::jsonb CHECK (jsonb_typeof(criteria) = 'object'),
    display_order INTEGER     NOT NULL DEFAULT 0,
    status        TEXT        NOT NULL DEFAULT 'ACTIVE' CHECK (status IN ('ACTIVE','RETIRED')),
    created_by    UUID        NOT NULL,
    updated_by    UUID        NOT NULL,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (id, org_id),
    FOREIGN KEY (benefit_id, org_id) REFERENCES benefit_catalog_items(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (created_by, org_id) REFERENCES users(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (updated_by, org_id) REFERENCES users(id, org_id) ON DELETE RESTRICT
);

CREATE INDEX idx_benefit_catalog_tiers_item
    ON benefit_catalog_tiers (org_id, benefit_id, status, display_order);
CREATE UNIQUE INDEX idx_benefit_catalog_tiers_active_basis_key
    ON benefit_catalog_tiers (org_id, benefit_id, tier_basis, tier_key)
    WHERE status = 'ACTIVE';

-- console-gate: audited-table benefit_catalog_conditions
CREATE TABLE benefit_catalog_conditions (
    id               UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id           UUID        NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    benefit_id       UUID        NOT NULL,
    condition_kind   TEXT        NOT NULL CHECK (condition_kind IN ('ORG','BRANCH','SITE','TEAM','ROLE','POSITION','TENURE','AGE','GENDER','EMPLOYMENT_TYPE','CONTRACT','COST_CENTER','CUSTOM')),
    operator         TEXT        NOT NULL CHECK (operator IN ('eq','in','not_in','gte','lte','range','exists','custom_policy')),
    condition_key    TEXT        NOT NULL CHECK (condition_key ~ '^[a-z][a-z0-9_]{1,63}$'),
    condition_value  JSONB       NOT NULL CHECK (jsonb_typeof(condition_value) IN ('object','array','string','number','boolean')),
    display_label    TEXT        NOT NULL CHECK (btrim(display_label) <> '' AND char_length(display_label) <= 200),
    cedar_policy_ref TEXT        NULL CHECK (cedar_policy_ref IS NULL OR char_length(cedar_policy_ref) <= 200),
    display_order    INTEGER     NOT NULL DEFAULT 0,
    status           TEXT        NOT NULL DEFAULT 'ACTIVE' CHECK (status IN ('ACTIVE','RETIRED')),
    created_by       UUID        NOT NULL,
    updated_by       UUID        NOT NULL,
    created_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (id, org_id),
    FOREIGN KEY (benefit_id, org_id) REFERENCES benefit_catalog_items(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (created_by, org_id) REFERENCES users(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (updated_by, org_id) REFERENCES users(id, org_id) ON DELETE RESTRICT
);

CREATE INDEX idx_benefit_catalog_conditions_item
    ON benefit_catalog_conditions (org_id, benefit_id, status, display_order);
CREATE INDEX idx_benefit_catalog_conditions_kind
    ON benefit_catalog_conditions (org_id, condition_kind, condition_key)
    WHERE status = 'ACTIVE';

DO $$
DECLARE
    t TEXT;
    tenant_tables TEXT[] := ARRAY[
        'benefit_code_counters',
        'benefit_catalog_items',
        'benefit_catalog_tiers',
        'benefit_catalog_conditions'
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
            'CREATE TRIGGER trg_%I_org_immutable BEFORE UPDATE ON %I '
            || 'FOR EACH ROW EXECUTE FUNCTION enforce_org_id_immutable()',
            t, t
        );
    END LOOP;
END
$$;

GRANT SELECT, INSERT, UPDATE ON benefit_code_counters TO console_rt;
GRANT SELECT, INSERT, UPDATE ON benefit_catalog_items TO console_rt;
GRANT SELECT, INSERT, UPDATE ON benefit_catalog_tiers TO console_rt;
GRANT SELECT, INSERT, UPDATE ON benefit_catalog_conditions TO console_rt;
REVOKE DELETE ON benefit_code_counters FROM console_rt;
REVOKE DELETE ON benefit_catalog_items FROM console_rt;
REVOKE DELETE ON benefit_catalog_tiers FROM console_rt;
REVOKE DELETE ON benefit_catalog_conditions FROM console_rt;

-- BE-LC may land on another branch before this migration. Seed the canonical
-- benefit object rules when the generic lifecycle table is present, but do not
-- create or duplicate the lifecycle substrate here.
DO $$
BEGIN
    IF to_regclass('public.lifecycle_transition_rules') IS NOT NULL THEN
        EXECUTE $benefit_lifecycle_seed$
            INSERT INTO lifecycle_transition_rules (object_type, from_state, to_state) VALUES
                ('benefit_catalog_item', 'draft', 'pending'),
                ('benefit_catalog_item', 'pending', 'finalized'),
                ('benefit_catalog_item', 'finalized', 'implemented'),
                ('benefit_catalog_item', 'implemented', 'retiring'),
                ('benefit_catalog_item', 'retiring', 'retired')
            ON CONFLICT DO NOTHING
        $benefit_lifecycle_seed$;
    END IF;
END
$$;
