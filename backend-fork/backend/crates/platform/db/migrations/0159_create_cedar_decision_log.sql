-- L-CEDAR decision feed (arch §5): an append-only, tenant-scoped ledger of the
-- point-decisions the `/api/v1/policy/authorize[/bulk]` path computes, so the
-- Integrity console can show recent authorize history. The authored-policy
-- studio (0103) persists the POLICIES; nothing persisted the live DECISIONS —
-- this table closes that gap.
--
-- FORCE-RLS org-isolated and append-only (no UPDATE/DELETE): a decision, once
-- recorded, is immutable. Runtime writes go through `with_org_conn` as console_rt
-- under `app.current_org`, so Postgres RLS scopes every row to the tenant.
--
-- ponytail: unbounded growth is capped by a retention prune (a periodic DELETE
-- older than N days by a maintenance job) — not built here. The append-only
-- trigger blocks the app role's DELETE, so the prune runs as the table owner /
-- a dedicated maintenance path when it lands.

CREATE TABLE cedar_decision_log (
    id                   UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id               UUID        NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    decided_at           TIMESTAMPTZ NOT NULL DEFAULT now(),
    -- the admin who invoked authorize (NULL only if a system path ever records one)
    actor                UUID        NULL,
    -- the decision's subject (SimRequest.subject.user_id), NOT necessarily the actor
    subject_ref          TEXT        NOT NULL CHECK (char_length(subject_ref) BETWEEN 1 AND 200),
    action               TEXT        NOT NULL CHECK (char_length(action) BETWEEN 1 AND 200),
    resource_type        TEXT        NOT NULL CHECK (char_length(resource_type) BETWEEN 1 AND 200),
    resource_id          TEXT        NULL CHECK (resource_id IS NULL OR char_length(resource_id) <= 200),
    effect               TEXT        NOT NULL CHECK (effect IN ('allow','deny')),
    -- Cedar `reason` policy ids that determined the decision (empty on deny-by-omission)
    determining_policies JSONB       NOT NULL DEFAULT '[]'::jsonb
                            CHECK (jsonb_typeof(determining_policies) = 'array'),
    reason               TEXT        NOT NULL CHECK (char_length(reason) <= 2000),
    created_at           TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (id, org_id)
);
-- The feed reads recent-first per tenant, optionally since a cursor instant.
CREATE INDEX idx_cedar_decision_log_feed
    ON cedar_decision_log (org_id, decided_at DESC);

-- FORCE RLS org isolation.
ALTER TABLE cedar_decision_log ENABLE ROW LEVEL SECURITY;
ALTER TABLE cedar_decision_log FORCE ROW LEVEL SECURITY;
CREATE POLICY org_isolation ON cedar_decision_log
    USING (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid)
    WITH CHECK (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid);

-- Append-only: a recorded decision can never be rewritten or removed (UPDATE +
-- DELETE both rejected — this also pins org_id immutable for free).
CREATE OR REPLACE FUNCTION cedar_decision_log_append_only()
RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION 'cedar_decision_log is append-only: % is forbidden (id=%)', TG_OP, OLD.id;
END;
$$;
CREATE TRIGGER trg_cedar_decision_log_no_update
    BEFORE UPDATE ON cedar_decision_log
    FOR EACH ROW EXECUTE FUNCTION cedar_decision_log_append_only();
CREATE TRIGGER trg_cedar_decision_log_no_delete
    BEFORE DELETE ON cedar_decision_log
    FOR EACH ROW EXECUTE FUNCTION cedar_decision_log_append_only();

-- Runtime role: append + read only. No hard delete on the app path.
GRANT SELECT, INSERT ON cedar_decision_log TO console_rt;
REVOKE UPDATE, DELETE ON cedar_decision_log FROM console_rt;
