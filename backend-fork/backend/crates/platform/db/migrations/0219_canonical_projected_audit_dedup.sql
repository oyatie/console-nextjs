-- B-EMP-A employment containment (console-0hf follow-ups), part 1 of 2:
--
-- The accepted ontology action key AND object type id are bound into the
-- immutable command receipt so a replay can reject a retry that reuses the same
-- `command_id` through a DIFFERENT action OR a different object type (an action
-- key is unique only per object type). See `execute_action`'s replay
-- short-circuit in `ontology/rest/src/lib.rs`. Both columns are NULLABLE so
-- legacy receipts written before this migration stay replayable (a NULL value
-- cannot be compared, which is the documented migration limitation). The
-- partial unique index that closes the check-then-insert TOCTOU for the
-- canonical-projected execute audit lives in 0220 (a no-transaction migration,
-- because `CREATE INDEX CONCURRENTLY` cannot run inside a transaction block).

ALTER TABLE ont_action_command_receipts
    ADD COLUMN action_key TEXT,
    ADD COLUMN object_type_id UUID;

COMMENT ON COLUMN ont_action_command_receipts.action_key IS 'pd:none — ontology action stable key, not personal data';
COMMENT ON COLUMN ont_action_command_receipts.object_type_id IS 'pd:none — ontology object type id, not personal data';
