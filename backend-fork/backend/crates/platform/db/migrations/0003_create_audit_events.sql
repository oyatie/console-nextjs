-- Append-only audit log. Written in the SAME transaction as every state
-- mutation (plan §2.2 audit-first discipline). LocationPing rows are NOT stored
-- here (위치정보법 destruction requirement — plan §2.2 carve-out).
--
-- Immutability is enforced at two layers:
--   1. REVOKE UPDATE, DELETE on audit_events FROM PUBLIC (permission layer).
--   2. A BEFORE trigger raises an exception on any UPDATE or DELETE attempt
--      (defense-in-depth for superuser or future role grant oversights).

CREATE TABLE audit_events (
    id          UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    -- NULL = system-initiated (escalation timer, retention job, etc.)
    actor       UUID        REFERENCES users(id) ON DELETE RESTRICT,
    -- Dot-namespaced action code validated by AuditAction in kernel, e.g.
    -- 'work_order.approve'. CHECK mirrors the kernel regex.
    action      TEXT        NOT NULL CHECK (action ~ '^[a-z0-9_]+(\.[a-z0-9_]+)+$'),
    target_type TEXT        NOT NULL,
    target_id   TEXT        NOT NULL,
    -- NULL = organization-global event (roster import, etc.)
    branch_id   UUID        REFERENCES branches(id) ON DELETE RESTRICT,
    -- State snapshots before/after the mutation.
    before_snap JSONB,
    after_snap  JSONB,
    -- W3C traceparent field shapes: 32-hex trace_id, 16-hex span_id.
    trace_id    CHAR(32)    NOT NULL,
    span_id     CHAR(16)    NOT NULL,
    occurred_at TIMESTAMPTZ NOT NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Append-only enforcement: revoke mutating privileges from PUBLIC.
-- The migration role that runs this script retains its own grants but PUBLIC
-- (which all non-superuser roles inherit) cannot UPDATE or DELETE rows.
REVOKE UPDATE, DELETE ON audit_events FROM PUBLIC;

-- Defense-in-depth trigger: raise an exception even if a privileged role
-- attempts UPDATE or DELETE on this table at runtime.
CREATE OR REPLACE FUNCTION audit_events_immutable()
RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION
        'audit_events is append-only: % is forbidden (row id=%)',
        TG_OP, OLD.id;
END;
$$;

CREATE TRIGGER trg_audit_events_no_update
    BEFORE UPDATE ON audit_events
    FOR EACH ROW EXECUTE FUNCTION audit_events_immutable();

CREATE TRIGGER trg_audit_events_no_delete
    BEFORE DELETE ON audit_events
    FOR EACH ROW EXECUTE FUNCTION audit_events_immutable();

-- Index for common access patterns: lookup by target, actor, branch, time.
CREATE INDEX idx_audit_events_target ON audit_events (target_type, target_id);
CREATE INDEX idx_audit_events_actor  ON audit_events (actor) WHERE actor IS NOT NULL;
CREATE INDEX idx_audit_events_branch ON audit_events (branch_id, occurred_at DESC)
    WHERE branch_id IS NOT NULL;
