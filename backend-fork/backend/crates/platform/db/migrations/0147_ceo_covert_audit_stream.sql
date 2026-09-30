-- B26b: CEO/top-clearance covert audit stream substrate.
--
-- The stream is tenant-scoped and deny-by-omission: no clearance assignment row
-- means no Cedar entity fact, and no stream label means an audit row is absent
-- from the covert stream. Runtime reads/writes still run as console_rt under
-- app.current_org, so Cedar capability checks never replace Postgres RLS.

INSERT INTO feature_catalog (feature_key) VALUES
    ('audit_stream_read'),
    ('audit_stream_access_log_read')
ON CONFLICT (feature_key) DO NOTHING;

-- console-gate: audited-table clearance_assignments
CREATE TABLE clearance_assignments (
    id             UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id         UUID        NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    user_id        UUID        NOT NULL,
    clearance_key  TEXT        NOT NULL CHECK (clearance_key ~ '^[a-z0-9_]+(\.[a-z0-9_]+)+$'),
    stream_key     TEXT        NOT NULL CHECK (stream_key ~ '^[a-z0-9_]+(\.[a-z0-9_]+)*$'),
    status         TEXT        NOT NULL CHECK (status IN ('ACTIVE','REVOKED','EXPIRED')),
    starts_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at     TIMESTAMPTZ NULL,
    granted_by     UUID        NULL,
    revoked_by     UUID        NULL,
    grant_reason   TEXT        NOT NULL CHECK (char_length(grant_reason) BETWEEN 1 AND 512),
    created_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (id, org_id),
    FOREIGN KEY (user_id, org_id) REFERENCES users(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (granted_by, org_id) REFERENCES users(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (revoked_by, org_id) REFERENCES users(id, org_id) ON DELETE RESTRICT
);

CREATE UNIQUE INDEX idx_clearance_assignments_active_unique
    ON clearance_assignments (org_id, user_id, stream_key, clearance_key)
    WHERE status = 'ACTIVE';
CREATE INDEX idx_clearance_assignments_lookup
    ON clearance_assignments (org_id, user_id, stream_key, status, starts_at, expires_at);

-- Composite key for same-org stream labels. audit_events.id remains the primary
-- key; this lets the label table prove the referenced audit event belongs to the
-- same tenant without querying across tenants.
ALTER TABLE audit_events
    ADD CONSTRAINT audit_events_org_id_id_key UNIQUE (org_id, id);

-- console-gate: audited-table audit_stream_event_labels
CREATE TABLE audit_stream_event_labels (
    org_id         UUID        NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    audit_event_id UUID        NOT NULL,
    stream_key     TEXT        NOT NULL CHECK (stream_key ~ '^[a-z0-9_]+(\.[a-z0-9_]+)*$'),
    sensitivity    TEXT        NOT NULL CHECK (sensitivity IN ('STANDARD','COVERT','CEO_COVERT')),
    created_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (org_id, audit_event_id, stream_key),
    FOREIGN KEY (org_id, audit_event_id) REFERENCES audit_events(org_id, id) ON DELETE RESTRICT
);
CREATE INDEX idx_audit_stream_event_labels_stream
    ON audit_stream_event_labels (org_id, stream_key, created_at DESC);

DO $$
DECLARE
    t TEXT;
    tenant_tables TEXT[] := ARRAY[
        'clearance_assignments',
        'audit_stream_event_labels'
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
        EXECUTE format(
            'CREATE TRIGGER trg_%I_org_immutable BEFORE UPDATE ON %I '
            || 'FOR EACH ROW EXECUTE FUNCTION enforce_org_id_immutable()',
            t, t
        );
    END LOOP;
END
$$;

GRANT SELECT, INSERT, UPDATE ON clearance_assignments TO console_rt;
REVOKE DELETE ON clearance_assignments FROM console_rt;

GRANT SELECT, INSERT ON audit_stream_event_labels TO console_rt;
REVOKE UPDATE, DELETE ON audit_stream_event_labels FROM console_rt;

CREATE OR REPLACE FUNCTION audit_stream_event_labels_append_only()
RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION
        'audit_stream_event_labels is append-only: % is forbidden (audit_event_id=%)',
        TG_OP, OLD.audit_event_id;
END;
$$;

CREATE TRIGGER trg_audit_stream_event_labels_no_update
    BEFORE UPDATE ON audit_stream_event_labels
    FOR EACH ROW EXECUTE FUNCTION audit_stream_event_labels_append_only();

CREATE TRIGGER trg_audit_stream_event_labels_no_delete
    BEFORE DELETE ON audit_stream_event_labels
    FOR EACH ROW EXECUTE FUNCTION audit_stream_event_labels_append_only();
