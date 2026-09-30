-- L-GOV M1 security fix: bind + consume four-eyes approvals.
--
-- Before this migration a four-eyes gate resolved approval with only
-- `SELECT decision FROM gov_approvals WHERE request_ref = $1` — no binding to the
-- action kind or the target object, and never consumed. ANY approved row in the
-- caller's org (a leave approval, an old unrelated decision) satisfied ANY
-- four-eyes gate, repeatedly, forever (replayable). This migration adds the two
-- missing halves:
--
--   1. BINDING — the approval carries the object it is FOR (`target_ref`); the
--      action kind is already the existing `kind` column. A gate now matches
--      (request_ref, kind, target_ref) against server-derived values, so an
--      approval decided for one (kind, object) can never satisfy a gate for a
--      different one.
--   2. CONSUMPTION — `gov_approval_consumptions` records the single use of an
--      approval. A `UNIQUE (org_id, approval_id)` makes a second consumption
--      impossible, so a replay of the same approved ref is denied.
--
-- gov_approvals stays append-only (REVOKE UPDATE/DELETE); consumption is a
-- separate append-only INSERT, so the decision record itself remains immutable.
--
-- STRICT backfill: existing approvals get a NULL `target_ref`. A gate only admits
-- an approval whose `target_ref` matches the action's server-derived target, so a
-- legacy row with a NULL target no longer satisfies a target-bound gate — those
-- in-flight approvals must be re-requested under the bound contract. This is the
-- intended fail-closed posture; the only production-wired four-eyes flow at author
-- time (evidence hold release) opens its approval fresh with the bound target.

-- The object each approval/request is FOR (a hold id, a workflow definition id, an
-- ontology instance id, …). Nullable: legacy rows and create-style actions with no
-- pre-existing target carry NULL and a gate matches NULL against a NULL expected
-- target only. No FK — like request_ref, it is a logical ref across lanes.
ALTER TABLE gov_approval_requests ADD COLUMN target_ref UUID;
ALTER TABLE gov_approvals         ADD COLUMN target_ref UUID;

-- One-per-approval consumption record. A four-eyes approval is single-use: the row
-- is inserted in the SAME transaction as the gated action (workflow/ontology) or in
-- its own committed step just before it (docs release / projected dispatch). The
-- unique key denies a second consumption (replay).
CREATE TABLE gov_approval_consumptions (
    id           UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id       UUID        NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    approval_id  UUID        NOT NULL,        -- the gov_approvals row consumed
    consumed_by  UUID        NOT NULL,        -- actor performing the gated action
    consumed_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (id, org_id),
    UNIQUE (org_id, approval_id),             -- single-use: replay denied
    FOREIGN KEY (approval_id, org_id) REFERENCES gov_approvals(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (consumed_by, org_id) REFERENCES users(id, org_id)         ON DELETE RESTRICT
);
CREATE INDEX idx_gov_approval_consumptions_approval
    ON gov_approval_consumptions (org_id, approval_id);

-- FORCE RLS org_isolation + org-immutable + append-only (UPDATE + DELETE both
-- rejected), reusing the helpers 0106/0153 defined.
ALTER TABLE gov_approval_consumptions ENABLE ROW LEVEL SECURITY;
ALTER TABLE gov_approval_consumptions FORCE ROW LEVEL SECURITY;
CREATE POLICY org_isolation ON gov_approval_consumptions
    USING (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid)
    WITH CHECK (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid);
CREATE TRIGGER trg_gov_approval_consumptions_org_immutable BEFORE UPDATE ON gov_approval_consumptions
    FOR EACH ROW EXECUTE FUNCTION enforce_org_id_immutable();
CREATE TRIGGER trg_gov_approval_consumptions_no_update
    BEFORE UPDATE ON gov_approval_consumptions
    FOR EACH ROW EXECUTE FUNCTION governance_append_only_record();
CREATE TRIGGER trg_gov_approval_consumptions_no_delete
    BEFORE DELETE ON gov_approval_consumptions
    FOR EACH ROW EXECUTE FUNCTION governance_append_only_record();

-- Append-only record: SELECT + INSERT only, never UPDATE/DELETE.
GRANT SELECT, INSERT ON gov_approval_consumptions TO console_rt;
REVOKE UPDATE, DELETE ON gov_approval_consumptions FROM console_rt;
