-- Collaboration calendar and poll foundations.
--
-- The collaboration hub must not be a static demo surface. Calendar events and
-- polls are tenant-owned business objects with explicit audience scope,
-- optional source-object links, lifecycle evidence, and forced RLS.

-- console-gate: audited-table collaboration_calendar_events
CREATE TABLE collaboration_calendar_events (
    id              UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id          UUID        NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    scope_type      TEXT        NOT NULL CHECK (scope_type IN ('TENANT','ORG','DEPARTMENT','TEAM','PERSONAL')),
    scope_ref       TEXT        NULL CHECK (scope_ref IS NULL OR char_length(scope_ref) BETWEEN 1 AND 160),
    title           TEXT        NOT NULL CHECK (char_length(title) BETWEEN 1 AND 160),
    description     TEXT        NOT NULL DEFAULT '' CHECK (char_length(description) <= 2000),
    starts_at       TIMESTAMPTZ NOT NULL,
    ends_at         TIMESTAMPTZ NOT NULL,
    all_day         BOOLEAN     NOT NULL DEFAULT false,
    status          TEXT        NOT NULL DEFAULT 'ACTIVE' CHECK (status IN ('ACTIVE','CANCELLED')),
    object_type     TEXT        NULL CHECK (object_type IS NULL OR object_type ~ '^[a-z][a-z0-9_]{1,63}$'),
    object_id       UUID        NULL,
    created_by      UUID        NULL REFERENCES users(id) ON DELETE SET NULL,
    updated_by      UUID        NULL REFERENCES users(id) ON DELETE SET NULL,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (id, org_id),
    CHECK (ends_at >= starts_at),
    CHECK ((object_type IS NULL AND object_id IS NULL) OR (object_type IS NOT NULL AND object_id IS NOT NULL))
);
CREATE INDEX idx_collaboration_calendar_events_scope
    ON collaboration_calendar_events (org_id, scope_type, scope_ref, starts_at, status);
CREATE INDEX idx_collaboration_calendar_events_object
    ON collaboration_calendar_events (org_id, object_type, object_id)
    WHERE object_type IS NOT NULL;

-- console-gate: audited-table collaboration_calendar_event_events
CREATE TABLE collaboration_calendar_event_events (
    id           UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id       UUID        NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    event_id     UUID        NOT NULL,
    action       TEXT        NOT NULL CHECK (action ~ '^[a-z][a-z0-9_]*(\.[a-z][a-z0-9_]*)+$'),
    actor_id     UUID        NULL REFERENCES users(id) ON DELETE SET NULL,
    summary      TEXT        NOT NULL CHECK (char_length(summary) BETWEEN 1 AND 512),
    before_snap  JSONB       NULL,
    after_snap   JSONB       NULL,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    FOREIGN KEY (event_id, org_id) REFERENCES collaboration_calendar_events(id, org_id) ON DELETE CASCADE
);
CREATE INDEX idx_collaboration_calendar_event_events_history
    ON collaboration_calendar_event_events (org_id, event_id, created_at DESC);

-- console-gate: audited-table collaboration_polls
CREATE TABLE collaboration_polls (
    id                UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id            UUID        NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    target_scope_type TEXT        NOT NULL CHECK (target_scope_type IN ('TENANT','ORG','DEPARTMENT','TEAM','PERSONAL')),
    target_scope_ref  TEXT        NULL CHECK (target_scope_ref IS NULL OR char_length(target_scope_ref) BETWEEN 1 AND 160),
    title             TEXT        NOT NULL CHECK (char_length(title) BETWEEN 1 AND 160),
    question          TEXT        NOT NULL CHECK (char_length(question) BETWEEN 1 AND 1000),
    status            TEXT        NOT NULL DEFAULT 'OPEN' CHECK (status IN ('DRAFT','OPEN','CLOSED','ARCHIVED')),
    anonymity         TEXT        NOT NULL DEFAULT 'NAMED' CHECK (anonymity IN ('NAMED','ANONYMOUS')),
    allow_multiple    BOOLEAN     NOT NULL DEFAULT false,
    closes_at         TIMESTAMPTZ NULL,
    object_type       TEXT        NULL CHECK (object_type IS NULL OR object_type ~ '^[a-z][a-z0-9_]{1,63}$'),
    object_id         UUID        NULL,
    created_by        UUID        NULL REFERENCES users(id) ON DELETE SET NULL,
    updated_by        UUID        NULL REFERENCES users(id) ON DELETE SET NULL,
    created_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (id, org_id),
    CHECK ((object_type IS NULL AND object_id IS NULL) OR (object_type IS NOT NULL AND object_id IS NOT NULL))
);
CREATE INDEX idx_collaboration_polls_scope
    ON collaboration_polls (org_id, target_scope_type, target_scope_ref, status, created_at DESC);
CREATE INDEX idx_collaboration_polls_object
    ON collaboration_polls (org_id, object_type, object_id)
    WHERE object_type IS NOT NULL;

-- console-gate: audited-table collaboration_poll_options
CREATE TABLE collaboration_poll_options (
    id          UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id      UUID        NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    poll_id     UUID        NOT NULL,
    label       TEXT        NOT NULL CHECK (char_length(label) BETWEEN 1 AND 240),
    position    INTEGER     NOT NULL CHECK (position >= 0),
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (poll_id, position),
    FOREIGN KEY (poll_id, org_id) REFERENCES collaboration_polls(id, org_id) ON DELETE CASCADE
);
CREATE INDEX idx_collaboration_poll_options_poll
    ON collaboration_poll_options (org_id, poll_id, position);

-- console-gate: audited-table collaboration_poll_votes
CREATE TABLE collaboration_poll_votes (
    id                   UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id               UUID        NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    poll_id              UUID        NOT NULL,
    voter_id             UUID        NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    selected_option_ids  UUID[]      NOT NULL,
    created_at           TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at           TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (poll_id, voter_id),
    FOREIGN KEY (poll_id, org_id) REFERENCES collaboration_polls(id, org_id) ON DELETE CASCADE,
    CHECK (cardinality(selected_option_ids) >= 1)
);
CREATE INDEX idx_collaboration_poll_votes_poll
    ON collaboration_poll_votes (org_id, poll_id);

-- console-gate: audited-table collaboration_poll_events
CREATE TABLE collaboration_poll_events (
    id           UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id       UUID        NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    poll_id      UUID        NOT NULL,
    action       TEXT        NOT NULL CHECK (action ~ '^[a-z][a-z0-9_]*(\.[a-z][a-z0-9_]*)+$'),
    actor_id     UUID        NULL REFERENCES users(id) ON DELETE SET NULL,
    summary      TEXT        NOT NULL CHECK (char_length(summary) BETWEEN 1 AND 512),
    before_snap  JSONB       NULL,
    after_snap   JSONB       NULL,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    FOREIGN KEY (poll_id, org_id) REFERENCES collaboration_polls(id, org_id) ON DELETE CASCADE
);
CREATE INDEX idx_collaboration_poll_events_history
    ON collaboration_poll_events (org_id, poll_id, created_at DESC);

CREATE OR REPLACE FUNCTION collaboration_append_only_immutable()
RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION 'collaboration append-only table % forbids %', TG_TABLE_NAME, TG_OP
        USING ERRCODE = '25006';
END;
$$;

DO $$
DECLARE
    t TEXT;
    tenant_tables TEXT[] := ARRAY[
        'collaboration_calendar_events',
        'collaboration_calendar_event_events',
        'collaboration_polls',
        'collaboration_poll_options',
        'collaboration_poll_votes',
        'collaboration_poll_events'
    ];
BEGIN
    FOREACH t IN ARRAY tenant_tables LOOP
        EXECUTE format('ALTER TABLE %I ENABLE ROW LEVEL SECURITY', t);
        EXECUTE format('ALTER TABLE %I FORCE ROW LEVEL SECURITY', t);
        EXECUTE format(
            'CREATE POLICY org_isolation ON %I '
            || 'USING (org_id = NULLIF(current_setting(''app.current_org'', true), '''')::uuid) '
            || 'WITH CHECK (org_id = NULLIF(current_setting(''app.current_org'', true), '''')::uuid)',
            t
        );
    END LOOP;
END
$$;

GRANT SELECT, INSERT, UPDATE ON collaboration_calendar_events TO console_rt;
GRANT SELECT, INSERT ON collaboration_calendar_event_events TO console_rt;
GRANT SELECT, INSERT, UPDATE ON collaboration_polls TO console_rt;
GRANT SELECT, INSERT ON collaboration_poll_options TO console_rt;
GRANT SELECT, INSERT, UPDATE ON collaboration_poll_votes TO console_rt;
GRANT SELECT, INSERT ON collaboration_poll_events TO console_rt;

CREATE TRIGGER trg_collaboration_calendar_events_org_immutable
    BEFORE UPDATE ON collaboration_calendar_events
    FOR EACH ROW EXECUTE FUNCTION enforce_org_id_immutable();
CREATE TRIGGER trg_collaboration_calendar_events_no_delete
    BEFORE DELETE ON collaboration_calendar_events
    FOR EACH ROW EXECUTE FUNCTION collaboration_append_only_immutable();

CREATE TRIGGER trg_collaboration_calendar_event_events_no_update
    BEFORE UPDATE ON collaboration_calendar_event_events
    FOR EACH ROW EXECUTE FUNCTION collaboration_append_only_immutable();
CREATE TRIGGER trg_collaboration_calendar_event_events_no_delete
    BEFORE DELETE ON collaboration_calendar_event_events
    FOR EACH ROW EXECUTE FUNCTION collaboration_append_only_immutable();
CREATE TRIGGER trg_collaboration_calendar_event_events_org_immutable
    BEFORE UPDATE ON collaboration_calendar_event_events
    FOR EACH ROW EXECUTE FUNCTION enforce_org_id_immutable();

CREATE TRIGGER trg_collaboration_polls_org_immutable
    BEFORE UPDATE ON collaboration_polls
    FOR EACH ROW EXECUTE FUNCTION enforce_org_id_immutable();
CREATE TRIGGER trg_collaboration_polls_no_delete
    BEFORE DELETE ON collaboration_polls
    FOR EACH ROW EXECUTE FUNCTION collaboration_append_only_immutable();

CREATE TRIGGER trg_collaboration_poll_options_no_update
    BEFORE UPDATE ON collaboration_poll_options
    FOR EACH ROW EXECUTE FUNCTION collaboration_append_only_immutable();
CREATE TRIGGER trg_collaboration_poll_options_no_delete
    BEFORE DELETE ON collaboration_poll_options
    FOR EACH ROW EXECUTE FUNCTION collaboration_append_only_immutable();
CREATE TRIGGER trg_collaboration_poll_options_org_immutable
    BEFORE UPDATE ON collaboration_poll_options
    FOR EACH ROW EXECUTE FUNCTION enforce_org_id_immutable();

CREATE TRIGGER trg_collaboration_poll_votes_org_immutable
    BEFORE UPDATE ON collaboration_poll_votes
    FOR EACH ROW EXECUTE FUNCTION enforce_org_id_immutable();
CREATE TRIGGER trg_collaboration_poll_votes_no_delete
    BEFORE DELETE ON collaboration_poll_votes
    FOR EACH ROW EXECUTE FUNCTION collaboration_append_only_immutable();

CREATE TRIGGER trg_collaboration_poll_events_no_update
    BEFORE UPDATE ON collaboration_poll_events
    FOR EACH ROW EXECUTE FUNCTION collaboration_append_only_immutable();
CREATE TRIGGER trg_collaboration_poll_events_no_delete
    BEFORE DELETE ON collaboration_poll_events
    FOR EACH ROW EXECUTE FUNCTION collaboration_append_only_immutable();
CREATE TRIGGER trg_collaboration_poll_events_org_immutable
    BEFORE UPDATE ON collaboration_poll_events
    FOR EACH ROW EXECUTE FUNCTION enforce_org_id_immutable();
