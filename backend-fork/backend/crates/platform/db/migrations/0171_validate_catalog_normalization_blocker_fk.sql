-- Schedule this validation only after 0170 has been deployed and the preflight
-- reports no legacy orphan rows. Keeping it separate prevents the 0170 deploy
-- from scanning a populated blocker queue under a stronger validation lock.
DO $$
BEGIN
    IF EXISTS (
        SELECT 1
        FROM cedar_policy_catalog_normalization_blockers blocker
        LEFT JOIN cedar_policy_catalog_entries catalog
          ON catalog.id = blocker.catalog_entry_id
         AND catalog.org_id = blocker.org_id
        WHERE catalog.id IS NULL
    ) THEN
        RAISE EXCEPTION
            'cannot validate blocker catalog FK: orphan catalog normalization blockers remain; remediate or delete them before scheduling migration 0171';
    END IF;
END;
$$;

ALTER TABLE cedar_policy_catalog_normalization_blockers
    VALIDATE CONSTRAINT fk_cedar_policy_catalog_normalization_blockers_catalog;
