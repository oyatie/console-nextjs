-- L-CEDAR-authoring (arch §5b): object-policy (row) + property-policy (field)
-- attachments. One catalog policy can decide row visibility (object policy) and
-- field visibility (property policy); `forbid` catalog entries are the shape for
-- tenant-isolation / legal-hold guardrails (forbid always wins, un-out-permittable).
--
-- Both tables reference a `cedar_policy_catalog_entries` row (the authored
-- policy, from 0103) with a hard, org-scoped FK. `object_type_id` /
-- `property_def_id` are LOGICAL refs to the ontology registry (0105) kept as
-- plain UUIDs WITHOUT a hard FK — exactly as 0106 governance does — so this lane
-- stays independent of the `crates/ontology/*` migration ordering. Add the ont
-- FKs in the L-WIRE integration once both lanes are merged.
--
-- ponytail: attachments are append-only link records (SELECT+INSERT only, no
-- hard delete anywhere in the engine). A soft-detach status column is the
-- upgrade path if reconfiguration churn ever needs it.

-- Extend the 0103 catalog with the compiled Cedar text a promoted policy carries,
-- so the point-decision evaluator (simulate/authorize) can load a policy's source
-- directly instead of re-deriving it from the structured no-code columns. Nullable:
-- system-generated entries may have none.
ALTER TABLE cedar_policy_catalog_entries
    ADD COLUMN generated_policy_text TEXT NULL;

-- Object policy: which authored catalog policy decides row visibility for an
-- object type. Deny (no matching permit) ⇒ the instance row is hidden.
CREATE TABLE ont_object_policies (
    id               UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id           UUID        NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    object_type_id   UUID        NOT NULL,   -- logical ref to ont_object_types (no hard FK; independent lane)
    cedar_policy_id  UUID        NOT NULL,
    effect           TEXT        NOT NULL CHECK (effect IN ('permit','forbid')),
    created_by       UUID        NULL,
    created_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (id, org_id),
    UNIQUE (org_id, object_type_id, cedar_policy_id),
    FOREIGN KEY (cedar_policy_id, org_id) REFERENCES cedar_policy_catalog_entries(id, org_id) ON DELETE CASCADE,
    FOREIGN KEY (created_by, org_id) REFERENCES users(id, org_id) ON DELETE RESTRICT
);
CREATE INDEX idx_ont_object_policies_type
    ON ont_object_policies (org_id, object_type_id);

-- Property policy: the (Foundry: at most one) authored catalog policy that
-- decides field visibility for a property. Deny ⇒ the field value is nulled.
CREATE TABLE ont_property_policies (
    id               UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id           UUID        NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    property_def_id  UUID        NOT NULL,   -- logical ref to ont_property_defs (no hard FK; independent lane)
    cedar_policy_id  UUID        NOT NULL,
    created_by       UUID        NULL,
    created_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (id, org_id),
    UNIQUE (org_id, property_def_id),        -- ≤1 property policy per property (Foundry)
    FOREIGN KEY (cedar_policy_id, org_id) REFERENCES cedar_policy_catalog_entries(id, org_id) ON DELETE CASCADE,
    FOREIGN KEY (created_by, org_id) REFERENCES users(id, org_id) ON DELETE RESTRICT
);
CREATE INDEX idx_ont_property_policies_prop
    ON ont_property_policies (org_id, property_def_id);

CREATE OR REPLACE FUNCTION cedar_policy_attach_append_only()
RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION 'append-only policy attachment % forbids %', TG_TABLE_NAME, TG_OP;
END;
$$;

-- FORCE RLS org_isolation + org-immutable on both tables; append-only (UPDATE +
-- DELETE rejected) since a policy attachment is an immutable link record.
DO $$
DECLARE
    t TEXT;
    tenant_tables TEXT[] := ARRAY[
        'ont_object_policies',
        'ont_property_policies'
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
        EXECUTE format(
            'CREATE TRIGGER trg_%I_no_update BEFORE UPDATE ON %I '
            || 'FOR EACH ROW EXECUTE FUNCTION cedar_policy_attach_append_only()',
            t, t
        );
        EXECUTE format(
            'CREATE TRIGGER trg_%I_no_delete BEFORE DELETE ON %I '
            || 'FOR EACH ROW EXECUTE FUNCTION cedar_policy_attach_append_only()',
            t, t
        );
    END LOOP;
END
$$;

-- Runtime role: attach (INSERT) + read only. No hard delete anywhere.
GRANT SELECT, INSERT ON ont_object_policies   TO console_rt;
GRANT SELECT, INSERT ON ont_property_policies TO console_rt;
REVOKE UPDATE, DELETE ON ont_object_policies   FROM console_rt;
REVOKE UPDATE, DELETE ON ont_property_policies FROM console_rt;
