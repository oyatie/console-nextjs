-- Personal todos domain (UI-M3 Overview — Today/Plan panel).
--
-- Owner-scoped action items. `scopes` carries the scope chips (person/team/
-- site/entity refs) and `links` the object links (kind+id pairs) — both JSONB
-- arrays of {"kind","id","label"?} so new object kinds need no migration; the
-- domain layer validates every element before insert.
--
-- Tenant isolation follows the 0030_enable_rls idiom: RLS keyed on
-- `app.current_org`. There is no per-person GUC, so owner scoping is enforced
-- in application code from the authenticated principal, never request input.

-- console-gate: audited-table todos
CREATE TABLE todos (
    id            UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id        UUID        NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    owner_user_id UUID        NOT NULL,
    body          TEXT        NOT NULL
        CHECK (char_length(btrim(body)) BETWEEN 1 AND 500),
    scopes        JSONB       NOT NULL DEFAULT '[]'::jsonb
        CHECK (jsonb_typeof(scopes) = 'array'),
    links         JSONB       NOT NULL DEFAULT '[]'::jsonb
        CHECK (jsonb_typeof(links) = 'array'),
    done          BOOLEAN     NOT NULL DEFAULT false,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    done_at       TIMESTAMPTZ,
    UNIQUE (id, org_id),
    FOREIGN KEY (owner_user_id, org_id) REFERENCES users(id, org_id) ON DELETE RESTRICT
);

-- Backs the owner's Today/Plan list: open-first, newest-first, keyset on
-- (created_at, id).
CREATE INDEX idx_todos_owner
    ON todos (org_id, owner_user_id, done, created_at DESC, id DESC);

ALTER TABLE todos ENABLE ROW LEVEL SECURITY;
ALTER TABLE todos FORCE ROW LEVEL SECURITY;
CREATE POLICY org_isolation ON todos
    USING (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid)
    WITH CHECK (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid);

-- Todos are owner-owned scratch items: full CRUD for the runtime role
-- (delete is a first-class, audited operation — unlike notifications).
GRANT SELECT, INSERT, UPDATE, DELETE ON todos TO console_rt;
