-- no-transaction
-- B-EMP-A employment containment (console-0hf follow-ups), part 2 of 2:
--
-- DB-enforced uniqueness for the canonical-projected execute audit. One
-- CONCURRENTLY statement per no-transaction migration (see 0084 and the audit
-- read-path indexes 0101/0103/0104/0124): a plain `CREATE UNIQUE INDEX` takes a
-- lock that blocks `audit_events` inserts for the duration of the heap scan on a
-- populated table. The WHERE predicate restricts the index to
-- `ontology.canonical.execute`, the DISTINCT action `emit_canonical_projected_audit`
-- records under (never `ontology.action.execute`, which `instance_revision_writeback`
-- owns), so the two paths can never collide on the polymorphic `target_id`. This
-- unique index is the conflict key that closes the check-then-insert TOCTOU.

CREATE UNIQUE INDEX CONCURRENTLY idx_audit_events_canonical_projected_execute
    ON audit_events (org_id, action, target_id)
    WHERE action = 'ontology.canonical.execute';
