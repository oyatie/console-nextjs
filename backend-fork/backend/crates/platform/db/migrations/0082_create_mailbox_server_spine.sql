-- Standalone corporate mailbox server spine.
--
-- This is the metadata substrate for hosting corporate mailboxes ourselves
-- (authoritative MX + JMAP + IMAP) rather than only mirroring an external
-- tenant IMAP/SMTP account. It intentionally exposes no public port and creates
-- no network listener; public MX rollout is gated by DNS/TLS/abuse/backup/
-- observability checks in docs/specs/standalone-corporate-mailbox-server.md.

-- console-gate: audited-table mailbox_domains
CREATE TABLE mailbox_domains (
    id                   UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id               UUID        NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    group_id             UUID        NULL REFERENCES groups(id) ON DELETE RESTRICT,
    domain               TEXT        NOT NULL CHECK (
        domain = lower(domain)
        AND char_length(domain) BETWEEN 4 AND 253
        AND domain ~ '^[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?(\.[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?)+$'
    ),
    status               TEXT        NOT NULL DEFAULT 'DRAFT'
        CHECK (status IN ('DRAFT','VERIFYING','ACTIVE','PAUSED','DISABLED','FAILED')),
    verification_status  TEXT        NOT NULL DEFAULT 'NOT_STARTED'
        CHECK (verification_status IN ('NOT_STARTED','PENDING_DNS','VERIFIED','FAILED','EXPIRED')),
    mx_verified          BOOLEAN     NOT NULL DEFAULT FALSE,
    spf_verified         BOOLEAN     NOT NULL DEFAULT FALSE,
    dkim_verified        BOOLEAN     NOT NULL DEFAULT FALSE,
    dmarc_verified       BOOLEAN     NOT NULL DEFAULT FALSE,
    mta_sts_verified     BOOLEAN     NOT NULL DEFAULT FALSE,
    tls_rpt_verified     BOOLEAN     NOT NULL DEFAULT FALSE,
    dkim_selector        TEXT        NULL CHECK (dkim_selector IS NULL OR dkim_selector ~ '^[a-z0-9][a-z0-9_-]{0,62}$'),
    dkim_public_key_ref  TEXT        NULL CHECK (dkim_public_key_ref IS NULL OR char_length(btrim(dkim_public_key_ref)) BETWEEN 8 AND 300),
    dkim_private_key_ref TEXT        NULL CHECK (dkim_private_key_ref IS NULL OR char_length(btrim(dkim_private_key_ref)) BETWEEN 8 AND 300),
    dns_last_checked_at  TIMESTAMPTZ NULL,
    last_error_code      TEXT        NULL CHECK (last_error_code IS NULL OR last_error_code ~ '^[a-z][a-z0-9_]{1,80}$'),
    created_by           UUID        NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    created_at           TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at           TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (id, org_id),
    UNIQUE (domain),
    UNIQUE (org_id, domain),
    CHECK (status <> 'ACTIVE' OR (verification_status = 'VERIFIED' AND mx_verified AND spf_verified AND dkim_verified AND dmarc_verified))
);

-- console-gate: audited-table mailboxes
CREATE TABLE mailboxes (
    id                   UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id               UUID        NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    domain_id            UUID        NOT NULL,
    local_part           TEXT        NOT NULL CHECK (
        char_length(local_part) BETWEEN 1 AND 64
        AND local_part = lower(local_part)
        AND local_part ~ '^[a-z0-9]([a-z0-9._+-]{0,62}[a-z0-9])?$'
        AND local_part !~ '\.\.'
    ),
    owner_user_id        UUID        NULL,
    display_name         TEXT        NOT NULL CHECK (char_length(btrim(display_name)) BETWEEN 1 AND 200),
    mailbox_kind         TEXT        NOT NULL DEFAULT 'USER'
        CHECK (mailbox_kind IN ('USER','SHARED','ROLE','SYSTEM','ARCHIVE')),
    status               TEXT        NOT NULL DEFAULT 'ACTIVE'
        CHECK (status IN ('ACTIVE','LOCKED','DISABLED','OFFBOARDED','ARCHIVED')),
    quota_bytes          BIGINT      NOT NULL DEFAULT 1073741824 CHECK (quota_bytes BETWEEN 1048576 AND 1099511627776),
    retention_policy_key TEXT        NULL CHECK (retention_policy_key IS NULL OR retention_policy_key ~ '^[a-z][a-z0-9_]*(\.[a-z][a-z0-9_]*)*$'),
    legal_hold           BOOLEAN     NOT NULL DEFAULT FALSE,
    created_by           UUID        NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    created_at           TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at           TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (id, org_id),
    UNIQUE (id, org_id, domain_id),
    UNIQUE (org_id, domain_id, local_part),
    UNIQUE (domain_id, local_part),
    FOREIGN KEY (domain_id, org_id) REFERENCES mailbox_domains(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (owner_user_id, org_id) REFERENCES users(id, org_id) ON DELETE RESTRICT,
    CHECK ((mailbox_kind = 'USER' AND owner_user_id IS NOT NULL) OR mailbox_kind <> 'USER')
);

-- console-gate: audited-table mailbox_aliases
CREATE TABLE mailbox_aliases (
    id                UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id            UUID        NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    domain_id         UUID        NOT NULL,
    target_mailbox_id UUID        NOT NULL,
    local_part        TEXT        NOT NULL CHECK (
        char_length(local_part) BETWEEN 1 AND 64
        AND local_part = lower(local_part)
        AND (local_part = '*' OR (
            local_part ~ '^[a-z0-9]([a-z0-9._+-]{0,62}[a-z0-9])?$'
            AND local_part !~ '\.\.'
        ))
    ),
    alias_kind        TEXT        NOT NULL DEFAULT 'DIRECT'
        CHECK (alias_kind IN ('DIRECT','SHARED','ROLE','GROUP','CATCH_ALL')),
    status            TEXT        NOT NULL DEFAULT 'ACTIVE'
        CHECK (status IN ('ACTIVE','DISABLED')),
    created_by        UUID        NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    created_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (id, org_id),
    UNIQUE (id, org_id, domain_id),
    UNIQUE (org_id, domain_id, local_part),
    UNIQUE (domain_id, local_part),
    FOREIGN KEY (domain_id, org_id) REFERENCES mailbox_domains(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (target_mailbox_id, org_id) REFERENCES mailboxes(id, org_id) ON DELETE RESTRICT,
    CHECK ((alias_kind = 'CATCH_ALL' AND local_part = '*') OR (alias_kind <> 'CATCH_ALL' AND local_part <> '*'))
);

-- console-gate: audited-table mailbox_messages
CREATE TABLE mailbox_messages (
    id                UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id            UUID        NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    mailbox_id        UUID        NOT NULL,
    domain_id         UUID        NOT NULL,
    folder_role       TEXT        NOT NULL DEFAULT 'INBOX'
        CHECK (folder_role IN ('INBOX','SENT','DRAFTS','ARCHIVE','TRASH','JUNK','CUSTOM')),
    direction         TEXT        NOT NULL CHECK (direction IN ('IN','OUT')),
    rfc_message_id    TEXT        NULL CHECK (rfc_message_id IS NULL OR char_length(rfc_message_id) <= 998),
    in_reply_to       TEXT        NULL CHECK (in_reply_to IS NULL OR char_length(in_reply_to) <= 998),
    references_ids    TEXT[]      NOT NULL DEFAULT '{}',
    normalized_subject TEXT       NOT NULL DEFAULT '' CHECK (char_length(normalized_subject) <= 998),
    raw_object_key    TEXT        NOT NULL CHECK (char_length(btrim(raw_object_key)) BETWEEN 8 AND 600),
    raw_size_bytes    BIGINT      NOT NULL CHECK (raw_size_bytes >= 0),
    has_attachments   BOOLEAN     NOT NULL DEFAULT FALSE,
    seen              BOOLEAN     NOT NULL DEFAULT FALSE,
    flagged           BOOLEAN     NOT NULL DEFAULT FALSE,
    answered          BOOLEAN     NOT NULL DEFAULT FALSE,
    draft             BOOLEAN     NOT NULL DEFAULT FALSE,
    sensitivity       TEXT        NOT NULL DEFAULT 'INTERNAL'
        CHECK (sensitivity IN ('PUBLIC','INTERNAL','CONFIDENTIAL','HR','PAYROLL','LEGAL','SECRET')),
    received_at       TIMESTAMPTZ NULL,
    sent_at           TIMESTAMPTZ NULL,
    created_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (id, org_id),
    FOREIGN KEY (mailbox_id, org_id, domain_id) REFERENCES mailboxes(id, org_id, domain_id) ON DELETE RESTRICT,
    FOREIGN KEY (domain_id, org_id) REFERENCES mailbox_domains(id, org_id) ON DELETE RESTRICT,
    CHECK ((direction = 'IN' AND received_at IS NOT NULL) OR direction = 'OUT')
);

-- console-gate: audited-table mailbox_deliveries
CREATE TABLE mailbox_deliveries (
    id                    UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id                UUID        NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    mailbox_message_id    UUID        NULL,
    direction             TEXT        NOT NULL CHECK (direction IN ('IN','OUT')),
    status                TEXT        NOT NULL DEFAULT 'PENDING'
        CHECK (status IN ('PENDING','ACCEPTED','STORED','QUEUED','DELIVERED','REJECTED','FAILED','BOUNCED','DEAD_LETTERED','CANCELLED')),
    envelope_from         TEXT        NULL CHECK (envelope_from IS NULL OR char_length(envelope_from) <= 320),
    recipient_domain      TEXT        NOT NULL CHECK (
        recipient_domain = lower(recipient_domain)
        AND char_length(recipient_domain) BETWEEN 4 AND 253
        AND recipient_domain ~ '^[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?(\.[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?)+$'
    ),
    recipient_local_part  TEXT        NOT NULL CHECK (
        char_length(recipient_local_part) BETWEEN 1 AND 64
        AND recipient_local_part = lower(recipient_local_part)
        AND recipient_local_part ~ '^[a-z0-9]([a-z0-9._+-]{0,62}[a-z0-9])?$'
        AND recipient_local_part !~ '\.\.'
    ),
    remote_addr_hash      TEXT        NULL CHECK (remote_addr_hash IS NULL OR char_length(remote_addr_hash) BETWEEN 16 AND 128),
    queue_key             TEXT        NOT NULL CHECK (char_length(btrim(queue_key)) BETWEEN 16 AND 200),
    attempt_count         INTEGER     NOT NULL DEFAULT 0 CHECK (attempt_count >= 0),
    next_attempt_at       TIMESTAMPTZ NULL,
    locked_by             TEXT        NULL CHECK (locked_by IS NULL OR char_length(btrim(locked_by)) BETWEEN 1 AND 160),
    locked_until          TIMESTAMPTZ NULL,
    rejection_reason      TEXT        NULL CHECK (rejection_reason IS NULL OR rejection_reason ~ '^[a-z][a-z0-9_]{1,80}$'),
    error_payload         JSONB       NULL CHECK (error_payload IS NULL OR jsonb_typeof(error_payload) = 'object'),
    accepted_at           TIMESTAMPTZ NULL,
    completed_at          TIMESTAMPTZ NULL,
    created_at            TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at            TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (id, org_id),
    UNIQUE (org_id, queue_key),
    FOREIGN KEY (mailbox_message_id, org_id) REFERENCES mailbox_messages(id, org_id) ON DELETE RESTRICT,
    CHECK (completed_at IS NULL OR status IN ('STORED','DELIVERED','REJECTED','FAILED','BOUNCED','DEAD_LETTERED','CANCELLED')),
    CHECK (rejection_reason IS NULL OR status IN ('REJECTED','FAILED','BOUNCED','DEAD_LETTERED'))
);

DO $$
DECLARE
    t TEXT;
    mailbox_tables TEXT[] := ARRAY[
        'mailbox_domains',
        'mailboxes',
        'mailbox_aliases',
        'mailbox_messages',
        'mailbox_deliveries'
    ];
BEGIN
    FOREACH t IN ARRAY mailbox_tables LOOP
        EXECUTE format('ALTER TABLE %I ENABLE ROW LEVEL SECURITY', t);
        EXECUTE format('ALTER TABLE %I FORCE ROW LEVEL SECURITY', t);
        EXECUTE format(
            'CREATE POLICY org_isolation ON %I '
            || 'USING (org_id = NULLIF(current_setting(''app.current_org'', true), '''')::uuid) '
            || 'WITH CHECK (org_id = NULLIF(current_setting(''app.current_org'', true), '''')::uuid)', t);
        EXECUTE format('GRANT SELECT, INSERT, UPDATE, DELETE ON %I TO console_rt', t);
        EXECUTE format(
            'CREATE TRIGGER trg_%s_org_immutable BEFORE UPDATE ON %I '
            || 'FOR EACH ROW EXECUTE FUNCTION enforce_org_id_immutable()', t, t);
    END LOOP;
END $$;

CREATE INDEX idx_mailbox_domains_org_status ON mailbox_domains (org_id, status, updated_at DESC);
CREATE INDEX idx_mailbox_domains_group ON mailbox_domains (group_id, status) WHERE group_id IS NOT NULL;
CREATE INDEX idx_mailboxes_owner ON mailboxes (org_id, owner_user_id, status) WHERE owner_user_id IS NOT NULL;
CREATE INDEX idx_mailboxes_domain ON mailboxes (org_id, domain_id, status, local_part);
CREATE INDEX idx_mailbox_aliases_target ON mailbox_aliases (org_id, target_mailbox_id, status);
CREATE INDEX idx_mailbox_messages_mailbox_cursor ON mailbox_messages (org_id, mailbox_id, folder_role, created_at DESC, id DESC);
CREATE INDEX idx_mailbox_messages_message_id ON mailbox_messages (org_id, domain_id, rfc_message_id) WHERE rfc_message_id IS NOT NULL;
CREATE INDEX idx_mailbox_messages_sensitivity ON mailbox_messages (org_id, sensitivity, created_at DESC);
CREATE INDEX idx_mailbox_deliveries_queue ON mailbox_deliveries (org_id, status, next_attempt_at, created_at)
    WHERE status IN ('PENDING','QUEUED','FAILED');
CREATE INDEX idx_mailbox_deliveries_recipient ON mailbox_deliveries (org_id, recipient_domain, recipient_local_part, created_at DESC);
