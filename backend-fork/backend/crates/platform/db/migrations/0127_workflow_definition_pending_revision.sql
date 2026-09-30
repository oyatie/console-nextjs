-- BE-AUTO slice 2 — four-eyes definition publish (pendingRev staging).
--
-- Publishing a revision to an ALREADY-ACTIVE definition no longer applies
-- directly. It stages a pending revision: a new version row is inserted with
-- status 'PENDING' while the current active_version keeps serving, and a
-- SECOND distinct actor must approve the application (the publisher cannot
-- self-approve — mirrors the #205 workflow decide SoD, org-lead/SUPER_ADMIN
-- exempt-with-governance-finding). New definitions (active_version IS NULL)
-- keep direct activate.
--
-- workflow_definition_versions is strictly append-only (0069 forbids UPDATE),
-- so a version's status is fixed at insert. The staged revision is simply the
-- editing-produced DRAFT version (latest_version > active_version); staging only
-- records a pointer to it plus who staged it, on the mutable definitions row.
-- The active_version keeps serving until a SECOND actor approves, at which point
-- the approver's transaction appends the PUBLISHED version copied from the
-- pending DRAFT and flips active_version. Withdraw just clears the pointer.

-- Which version is staged and awaiting approval, and who staged it (the actor
-- barred from approving their own revision). Both cleared on approve/withdraw.
ALTER TABLE workflow_definitions
    ADD COLUMN pending_version INTEGER NULL
        CHECK (pending_version IS NULL OR pending_version >= 1),
    ADD COLUMN pending_staged_by UUID NULL
        REFERENCES users(id) ON DELETE SET NULL;
