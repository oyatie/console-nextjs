-- Close the duplicated-effect seam: an attachment is an immutable link whose
-- effect must agree with its catalog policy at write time. The runtime loader
-- independently checks legacy/corrupt rows before lowering, so this trigger is
-- preventive rather than a reason to trust persisted data blindly.
CREATE OR REPLACE FUNCTION enforce_ont_object_policy_effect_matches_catalog()
RETURNS TRIGGER LANGUAGE plpgsql AS $$
DECLARE
    catalog_effect TEXT;
BEGIN
    SELECT effect INTO catalog_effect
    FROM cedar_policy_catalog_entries
    WHERE id = NEW.cedar_policy_id AND org_id = NEW.org_id;

    IF catalog_effect IS NULL THEN
        RAISE EXCEPTION 'object policy attachment requires a same-org catalog entry';
    END IF;
    IF NEW.effect <> catalog_effect THEN
        RAISE EXCEPTION 'object policy attachment effect % must match catalog effect %', NEW.effect, catalog_effect;
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER trg_ont_object_policies_effect_matches_catalog
BEFORE INSERT ON ont_object_policies
FOR EACH ROW EXECUTE FUNCTION enforce_ont_object_policy_effect_matches_catalog();

-- The blocker queue is tenant data, not an unscoped operational scratch table.
-- The catalog already owns UNIQUE (id, org_id), so this FK makes stale blockers
-- impossible and cascades a queue row away when its catalog record is removed.
ALTER TABLE cedar_policy_catalog_normalization_blockers
    ADD CONSTRAINT fk_cedar_policy_catalog_normalization_blockers_catalog
    FOREIGN KEY (catalog_entry_id, org_id)
    REFERENCES cedar_policy_catalog_entries (id, org_id)
    ON DELETE CASCADE
    NOT VALID;

ALTER TABLE cedar_policy_catalog_normalization_blockers ENABLE ROW LEVEL SECURITY;
ALTER TABLE cedar_policy_catalog_normalization_blockers FORCE ROW LEVEL SECURITY;

CREATE POLICY org_isolation ON cedar_policy_catalog_normalization_blockers
    USING (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid)
    WITH CHECK (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid);

CREATE TRIGGER trg_cedar_policy_catalog_normalization_blockers_org_immutable
BEFORE UPDATE ON cedar_policy_catalog_normalization_blockers
FOR EACH ROW EXECUTE FUNCTION enforce_org_id_immutable();

GRANT SELECT ON cedar_policy_catalog_normalization_blockers TO console_rt;
