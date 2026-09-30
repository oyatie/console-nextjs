-- BE-OBJ slice 3, surface 1: type-registry lifecycle state.
--
-- The prototype's ontology treats each object TYPE as itself an object with a
-- lifecycle (ontTypes: 11 active + 1 draft-proposed "거래처" + 1 archived-and-
-- migrated "배차"). The type-card renders that state as a chip/stepper. Type
-- PROPOSAL / transition flows are NOT in scope for this slice (they need
-- governance design — data-governance + executive-SoD review + instance
-- migration gates); this migration only adds the status column + a read
-- surface, so a seeded/imported type can carry draft/active/archived truthfully.
--
-- object_types is a GLOBAL reference table (no org_id, no RLS) — already
-- allowlisted in the tenant-isolation gate. This is a plain additive column.
ALTER TABLE object_types
    ADD COLUMN status TEXT NOT NULL DEFAULT 'active'
        CHECK (status IN ('draft', 'active', 'archived'));

-- Every kind seeded to date (migrations 0102/0113/0115) is a live, in-use kind
-- → active. The DEFAULT already applies 'active' to the existing rows; this is
-- an explicit belt-and-suspenders statement of the intended post-migration
-- state (and documents that no seeded kind starts as draft/archived).
UPDATE object_types SET status = 'active' WHERE status IS DISTINCT FROM 'active';
