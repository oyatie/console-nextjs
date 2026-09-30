-- Scoped audiences + typed 유형 for the notice board (STORY-BOARD-001).
-- Audience granularity is branches: user_branches is the schema's only
-- org-membership primitive (sites are customer sites, not employment units).
--
-- PROVISIONAL number 0197 (lane CAP-BOARD-CONSOLE); the consolidation
-- integrator renumbers to the next free slot.

ALTER TABLE notices
    ADD COLUMN category TEXT NOT NULL DEFAULT 'general'
        CHECK (category IN ('general', 'legal', 'hr_order', 'training')),
    ADD COLUMN audience_scope TEXT NOT NULL DEFAULT 'org'
        CHECK (audience_scope IN ('org', 'branches'));

-- console-gate: audited-table notice_audience_branches
CREATE TABLE notice_audience_branches (
    org_id     UUID NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    notice_id  UUID NOT NULL,
    branch_id  UUID NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (notice_id, branch_id),
    FOREIGN KEY (notice_id, org_id) REFERENCES notices(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (branch_id, org_id) REFERENCES branches(id, org_id) ON DELETE RESTRICT
);
CREATE INDEX idx_notice_audience_branches_branch
    ON notice_audience_branches (org_id, branch_id);

ALTER TABLE notice_audience_branches ENABLE ROW LEVEL SECURITY;
ALTER TABLE notice_audience_branches FORCE ROW LEVEL SECURITY;
CREATE POLICY org_isolation ON notice_audience_branches
    USING (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid)
    WITH CHECK (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid);

-- DELETE granted: audience rows are replaceable while the notice is a draft
-- (application holds `SELECT … FOR UPDATE` on the notice and rejects mutation
-- once published); receipts stay the immutable record. UPDATE is revoked
-- explicitly because 0031's ALTER DEFAULT PRIVILEGES auto-grants full DML in
-- production — a row is inserted or deleted whole, never edited.
GRANT SELECT, INSERT, DELETE ON notice_audience_branches TO console_rt;
REVOKE UPDATE ON notice_audience_branches FROM console_rt;

-- The branch-scoped publish snapshot joins user_branches (created in 0002,
-- before the 0031 default-privileges cutover — no runtime grant exists yet).
GRANT SELECT ON user_branches TO console_rt;
