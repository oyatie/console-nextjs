-- CAP-ORG-CONSOLE: org-change lifecycle engine (STORY-ORG-001, HANDOFF §15/§16).
-- NOTE: number is PROVISIONAL (repo head 0180 at scout time) — the consolidation
-- integrator renumbers to the next free number immediately before push.
--
-- Register every capability before any tenant policy can grant it; catalog
-- presence is a prerequisite, never an implicit permission (routes stay
-- fail-closed on explicit role floors until grants exist).
INSERT INTO feature_catalog (feature_key) VALUES
    ('org_change_read'),
    ('org_change_draft'),
    ('org_change_approve'),
    ('org_change_apply')
ON CONFLICT (feature_key) DO NOTHING;

CREATE TABLE org_change_requests (
    id                  UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id              UUID        NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    code                TEXT        NOT NULL CHECK (code ~ '^OC-[0-9]{4}-[0-9]{4,}$'),
    kind                TEXT        NOT NULL CHECK (kind IN ('NEW','REORG','DISSOLVE')),
    status              TEXT        NOT NULL DEFAULT 'DRAFT' CHECK (status IN
                            ('DRAFT','PRECHECKED','IN_APPROVAL','APPROVED','APPLIED',
                             'SETTLING','ARCHIVED','REJECTED','CANCELLED')),
    target_kind         TEXT        NOT NULL CHECK (target_kind IN ('ENTITY','REGION','BRANCH','SITE','ORG_UNIT')),
    target_ref          TEXT        NOT NULL CHECK (btrim(target_ref) <> '' AND char_length(target_ref) <= 200),
    target_label        TEXT        NOT NULL CHECK (btrim(target_label) <> '' AND char_length(target_label) <= 200),
    effective_date      DATE        NOT NULL,
    reason              TEXT        NOT NULL CHECK (btrim(reason) <> '' AND char_length(reason) <= 4000),
    proposal            JSONB       NOT NULL DEFAULT '[]'::jsonb CHECK (jsonb_typeof(proposal) = 'array'),
    preflight           JSONB       CHECK (preflight IS NULL OR jsonb_typeof(preflight) = 'object'),
    headcount           BIGINT      NOT NULL DEFAULT 0 CHECK (headcount >= 0),
    site_count          BIGINT      NOT NULL DEFAULT 0 CHECK (site_count >= 0),
    team_count          BIGINT      NOT NULL DEFAULT 0 CHECK (team_count >= 0),
    supersedes_id       UUID,
    drafted_by          UUID        NOT NULL,
    idempotency_key     TEXT        NOT NULL CHECK (char_length(btrim(idempotency_key)) BETWEEN 16 AND 200),
    request_fingerprint TEXT        NOT NULL CHECK (request_fingerprint ~ '^[a-f0-9]{64}$'),
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (id, org_id),
    UNIQUE (org_id, code),
    UNIQUE (org_id, idempotency_key),
    FOREIGN KEY (drafted_by, org_id) REFERENCES users(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (supersedes_id, org_id) REFERENCES org_change_requests(id, org_id) ON DELETE RESTRICT
);
CREATE INDEX idx_org_change_requests_status
    ON org_change_requests (org_id, status, created_at DESC);

-- Ordered SoD chain (HR → 재무 → 법무 → 임원). Each decision is ALSO recorded in
-- gov_approvals keyed request_ref = the step id, so the DB-level
-- approver <> requester CHECK is the second self-approval net; this table owns
-- ordering + role binding + the denormalized decision the console reads.
CREATE TABLE org_change_approval_steps (
    id          UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id      UUID        NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    request_id  UUID        NOT NULL,
    step_order  SMALLINT    NOT NULL CHECK (step_order BETWEEN 1 AND 8),
    role_key    TEXT        NOT NULL CHECK (role_key IN ('hr','finance','legal','executive')),
    decision    TEXT        NOT NULL DEFAULT 'PENDING' CHECK (decision IN ('PENDING','APPROVED','REJECTED')),
    decided_by  UUID,
    decided_at  TIMESTAMPTZ,
    memo        TEXT        CHECK (memo IS NULL OR char_length(memo) <= 2000),
    UNIQUE (id, org_id),
    UNIQUE (org_id, request_id, step_order),
    FOREIGN KEY (request_id, org_id) REFERENCES org_change_requests(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (decided_by, org_id) REFERENCES users(id, org_id) ON DELETE RESTRICT
);

-- The six dissolve settlement items (§3.9.3), seeded at effectuate.
CREATE TABLE org_change_settlement_items (
    id          UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id      UUID        NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    request_id  UUID        NOT NULL,
    item_key    TEXT        NOT NULL CHECK (item_key IN ('TRANSFER_EMPLOYEES','POSITIONS','COST_CENTERS',
                    'CLOSE_OPEN_DOCS','ASSETS','PAYROLL_SOCIAL_FINAL')),
    done        BOOLEAN     NOT NULL DEFAULT false,
    done_by     UUID,
    done_at     TIMESTAMPTZ,
    memo        TEXT        CHECK (memo IS NULL OR char_length(memo) <= 2000),
    UNIQUE (id, org_id),
    UNIQUE (org_id, request_id, item_key),
    FOREIGN KEY (request_id, org_id) REFERENCES org_change_requests(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (done_by, org_id) REFERENCES users(id, org_id) ON DELETE RESTRICT
);

-- Append-only transition log (who/when/from→to/reason) powering the modal
-- history strip; complements the audit spine, never replaces it.
CREATE TABLE org_change_events (
    id          UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id      UUID        NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    request_id  UUID        NOT NULL,
    actor       UUID        NOT NULL,
    action      TEXT        NOT NULL CHECK (btrim(action) <> '' AND char_length(action) <= 80),
    from_status TEXT,
    to_status   TEXT,
    reason      TEXT        CHECK (reason IS NULL OR char_length(reason) <= 4000),
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (id, org_id),
    FOREIGN KEY (request_id, org_id) REFERENCES org_change_requests(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (actor, org_id) REFERENCES users(id, org_id) ON DELETE RESTRICT
);
CREATE INDEX idx_org_change_events_request
    ON org_change_events (org_id, request_id, created_at);

-- FORCE RLS org_isolation + org-immutable on every table.
DO $$
DECLARE t TEXT;
BEGIN
    FOREACH t IN ARRAY ARRAY[
        'org_change_requests','org_change_approval_steps',
        'org_change_settlement_items','org_change_events'
    ] LOOP
        EXECUTE format('ALTER TABLE %I ENABLE ROW LEVEL SECURITY', t);
        EXECUTE format('ALTER TABLE %I FORCE ROW LEVEL SECURITY', t);
        EXECUTE format(
            'CREATE POLICY org_isolation ON %I USING (org_id = NULLIF(current_setting(''app.current_org'', true), '''')::uuid) WITH CHECK (org_id = NULLIF(current_setting(''app.current_org'', true), '''')::uuid)',
            t);
        EXECUTE format(
            'CREATE TRIGGER trg_%s_org_immutable BEFORE UPDATE ON %I FOR EACH ROW EXECUTE FUNCTION enforce_org_id_immutable()',
            t, t);
    END LOOP;
END $$;

-- Terminal statuses are immutable (no resurrection of applied/archived/
-- rejected/cancelled requests; a rejected request is revised as a NEW row
-- via supersedes_id).
CREATE OR REPLACE FUNCTION org_change_terminal_immutable() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    IF OLD.status IN ('APPLIED','ARCHIVED','REJECTED','CANCELLED') THEN
        RAISE EXCEPTION 'terminal org-change request is immutable';
    END IF;
    RETURN NEW;
END $$;
CREATE TRIGGER trg_org_change_requests_terminal
    BEFORE UPDATE ON org_change_requests
    FOR EACH ROW EXECUTE FUNCTION org_change_terminal_immutable();

-- Events are append-only (reuse the governance helper from 0153).
CREATE TRIGGER trg_org_change_events_no_update
    BEFORE UPDATE ON org_change_events
    FOR EACH ROW EXECUTE FUNCTION governance_append_only_record();
CREATE TRIGGER trg_org_change_events_no_delete
    BEFORE DELETE ON org_change_events
    FOR EACH ROW EXECUTE FUNCTION governance_append_only_record();

-- No hard delete anywhere; events additionally never UPDATE.
GRANT SELECT, INSERT, UPDATE ON org_change_requests TO console_rt;
GRANT SELECT, INSERT, UPDATE ON org_change_approval_steps TO console_rt;
GRANT SELECT, INSERT, UPDATE ON org_change_settlement_items TO console_rt;
GRANT SELECT, INSERT ON org_change_events TO console_rt;
REVOKE UPDATE, DELETE ON org_change_events FROM console_rt;
