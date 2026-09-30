-- Widen the receipt store so a stored receipt says WHOSE it is.
--
-- `ont_action_command_receipts` (0177) is the authority record for all thirteen
-- dispatch targets, but it carries no attribution: three production crates write
-- it -- console-ontology-canonical-adapter-postgres, console-ontology-rest and
-- console-payroll-adapter-postgres -- and nothing records which object a row
-- belongs to. PR #829 bounded WHICH crates may write the table; it could not
-- express per-row attribution, because there was no column to attribute to.
--
-- The exact form below is the one specified in
-- backend/crates/ontology/canonical-domain/src/lib.rs, and the two CHECK bodies
-- are COPIED FROM the roster there rather than retyped:
-- `ReceiptOwner::owner_check_constraint_sql()` and
-- `target_check_constraint_sql()` generate these strings, and a test asserts the
-- constraint this migration installed still matches what they generate. Before
-- this migration those functions described columns PostgreSQL had never seen --
-- unit tests asserted the generated STRING and nothing executed it.
--
-- WHY THIS SHAPE AND NOT `ADD COLUMN owner TEXT NOT NULL`.
-- A plain NOT NULL add fails on any database that already holds a receipt, and
-- it would break the live writer at crates/ontology/rest/src/lib.rs, whose INSERT
-- names its columns explicitly and would not supply `owner`. It also could not be
-- repaired with a backfill UPDATE, because 0177's BEFORE UPDATE OR DELETE trigger
-- RAISEs on every row.
--
-- `ADD COLUMN ... DEFAULT` is DDL, so no row trigger fires, and `ADD CONSTRAINT`
-- then validates the pre-existing rows exactly as they already stand. The DEFAULT
-- is what lets this land BEFORE any caller edit.
--
-- ORDERING, and what is deliberately NOT here: the follow-up that drops the
-- DEFAULT belongs to a separate change, once the REST writer passes `owner`
-- explicitly. Dropping it now would break that writer on the next deploy.

ALTER TABLE ont_action_command_receipts
    ADD COLUMN owner  TEXT NOT NULL DEFAULT 'ontology.action',
    ADD COLUMN target TEXT;

ALTER TABLE ont_action_command_receipts
    ADD CONSTRAINT ont_action_command_receipts_owner_check
        CHECK (owner IN ('ontology.action', 'company', 'org_unit', 'job_position', 'person', 'employment', 'pay_run')),
    ADD CONSTRAINT ont_action_command_receipts_target_check
        CHECK ((owner = 'ontology.action') = (target IS NULL) AND (target IS NULL OR target IN ('company.revise', 'organization.create_org_unit', 'organization.revise_org_unit', 'organization.create_job_position', 'organization.revise_job_position', 'people.create_person', 'people.revise_person', 'hr.appoint', 'hr.promote', 'hr.transfer', 'payroll.create_run', 'payroll.submit_run', 'payroll.decide_run')));

-- Personal-data classification is REQUIRED for columns introduced after the
-- baseline froze at 0209, even though `ont_action_command_receipts` is itself on
-- the unclassified-tables backlog: the backlog covers what was already there, and
-- must not be allowed to grow. Both columns are closed rosters of system
-- identifiers -- an ObjectKey slug and a DispatchTarget slug -- naming WHICH
-- object and WHICH action a receipt belongs to. Neither carries or narrows to a
-- natural person; the person on a receipt is `actor_id`, which is classified
-- already.
COMMENT ON COLUMN ont_action_command_receipts.owner IS
    'pd:none — structural or non-personal attribute of a non-person row; closed ObjectKey roster naming which object owns this receipt';
COMMENT ON COLUMN ont_action_command_receipts.target IS
    'pd:none — structural or non-personal attribute of a non-person row; closed DispatchTarget roster, present exactly when the owner is canonical';
