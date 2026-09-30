-- BE-OBJ slice 3: validate the object_links -> link_types FK separately.
--
-- 0122 creates the registry, backfills every in-use link_type, and adds the FK
-- as NOT VALID to avoid coupling the write-blocking validation scan to the
-- registry/backfill transaction. Keep this validation in its own migration so
-- deploy tooling can run/retry/schedule the scan independently.
ALTER TABLE object_links
    VALIDATE CONSTRAINT object_links_link_type_fkey;
