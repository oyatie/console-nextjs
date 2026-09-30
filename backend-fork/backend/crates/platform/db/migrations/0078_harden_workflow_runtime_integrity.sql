-- Harden workflow runtime trace integrity and terminal outbox semantics.
--
-- Migration 0077 established the durable org-scoped runtime spine. This
-- follow-up closes the remaining integrity gap: a human waiting task or outbox
-- event must not be able to point at a node_run_id from another run in the same
-- org. It also makes terminal outbox states self-evident for audit and retry
-- operations.

ALTER TABLE workflow_node_runs
    ADD CONSTRAINT workflow_node_runs_org_run_id_key UNIQUE (org_id, run_id, id);

ALTER TABLE workflow_waiting_tasks
    ADD CONSTRAINT workflow_waiting_tasks_node_run_matches_run_fkey
    FOREIGN KEY (org_id, run_id, node_run_id)
    REFERENCES workflow_node_runs(org_id, run_id, id)
    ON DELETE RESTRICT;

ALTER TABLE workflow_outbox_events
    ADD CONSTRAINT workflow_outbox_events_node_run_matches_run_fkey
    FOREIGN KEY (org_id, run_id, node_run_id)
    REFERENCES workflow_node_runs(org_id, run_id, id)
    ON DELETE RESTRICT;

UPDATE workflow_outbox_events
   SET delivered_at = COALESCE(delivered_at, updated_at, now())
 WHERE status = 'DELIVERED'
   AND delivered_at IS NULL;

UPDATE workflow_outbox_events
   SET dead_lettered_at = COALESCE(dead_lettered_at, updated_at, now()),
       attempt_count = GREATEST(attempt_count, 1),
       error_payload = CASE
           WHEN error_payload IS NULL OR error_payload = '{}'::jsonb THEN
               jsonb_build_object('reason', 'legacy_terminal_state_without_error_payload')
           ELSE error_payload
       END
 WHERE status = 'DEAD_LETTERED'
   AND (
       dead_lettered_at IS NULL
       OR attempt_count < 1
       OR error_payload IS NULL
       OR error_payload = '{}'::jsonb
   );

ALTER TABLE workflow_outbox_events
    ADD CONSTRAINT workflow_outbox_events_delivered_requires_timestamp
    CHECK (status <> 'DELIVERED' OR delivered_at IS NOT NULL);

ALTER TABLE workflow_outbox_events
    ADD CONSTRAINT workflow_outbox_events_dead_letter_requires_evidence
    CHECK (
        status <> 'DEAD_LETTERED'
        OR (
            dead_lettered_at IS NOT NULL
            AND attempt_count >= 1
            AND error_payload IS NOT NULL
            AND jsonb_typeof(error_payload) = 'object'
            AND error_payload <> '{}'::jsonb
        )
    );
