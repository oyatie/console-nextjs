-- L-GOV: approvals-CREATE — the pending request a four-eyes decision decides.
--
-- Today gov_approvals only records the *decision* (append-only). The console's
-- 팀 배포 / override flows need to OPEN an approval first (a requester records a
-- pending request; a distinct approver decides it later). This table is that
-- pending record: one open request per (org, request_ref). "Decided" is derived
-- by joining gov_approvals on request_ref — no mutable status column, so the
-- request row is fully immutable/append-only like gov_overrides.
--
-- ponytail: request_ref is a plain UUID (a logical ref to whatever is being
-- gated — a gov_overrides.id, an ontology instance id for a console_view team
-- deploy, an action-execute ref) — no FK, matching gov_overrides.target_id.

CREATE TABLE gov_approval_requests (
    id               UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id           UUID        NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    request_ref      UUID        NOT NULL,
    kind             TEXT        NOT NULL CHECK (btrim(kind) <> '' AND char_length(kind) <= 80),
    requested_by     UUID        NOT NULL,
    payload_summary  JSONB       NOT NULL CHECK (jsonb_typeof(payload_summary) = 'object'),
    created_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (id, org_id),
    UNIQUE (org_id, request_ref),                 -- one open request per ref
    FOREIGN KEY (requested_by, org_id) REFERENCES users(id, org_id) ON DELETE RESTRICT
);
CREATE INDEX idx_gov_approval_requests_request
    ON gov_approval_requests (org_id, request_ref);

-- FORCE RLS org_isolation + org-immutable + append-only (UPDATE + DELETE both
-- rejected), reusing the helpers 0106 already defined.
ALTER TABLE gov_approval_requests ENABLE ROW LEVEL SECURITY;
ALTER TABLE gov_approval_requests FORCE ROW LEVEL SECURITY;
CREATE POLICY org_isolation ON gov_approval_requests
    USING (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid)
    WITH CHECK (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid);
CREATE TRIGGER trg_gov_approval_requests_org_immutable BEFORE UPDATE ON gov_approval_requests
    FOR EACH ROW EXECUTE FUNCTION enforce_org_id_immutable();
CREATE TRIGGER trg_gov_approval_requests_no_update
    BEFORE UPDATE ON gov_approval_requests
    FOR EACH ROW EXECUTE FUNCTION governance_append_only_record();
CREATE TRIGGER trg_gov_approval_requests_no_delete
    BEFORE DELETE ON gov_approval_requests
    FOR EACH ROW EXECUTE FUNCTION governance_append_only_record();

-- Append-only record: SELECT + INSERT only, never UPDATE/DELETE.
GRANT SELECT, INSERT ON gov_approval_requests TO console_rt;
REVOKE UPDATE, DELETE ON gov_approval_requests FROM console_rt;
