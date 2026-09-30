-- 0053_create_comms_webmail.sql
-- Per-tenant corporate webmail (SMTP + IMAP). Postgres is the source of truth;
-- the IMAP mailbox is mirrored INTO these tables by a later sync worker (B-mail-3).
--
-- Credentials are stored under ENVELOPE AEAD: a per-row data key (DEK) is wrapped
-- by a master KEK (XChaCha20Poly1305), and the SMTP/IMAP passwords are encrypted
-- under that DEK. Only ciphertext + nonces + the wrapped DEK + a key_version are
-- persisted here — NEVER a plaintext password. KEK rotation re-wraps the small
-- DEK, not every secret. The cipher AAD binds (org_id, account_id, field) so a
-- copied ciphertext fails authentication.
--
-- Every table follows the post-multi-tenant house style verified against
-- 0035/0042/0012/0050: single-column `id UUID PRIMARY KEY`, `org_id NOT NULL
-- REFERENCES organizations(id) ON DELETE RESTRICT`, and a trailing DO $$ loop
-- that ENABLEs + FORCEs RLS, creates the `org_isolation` policy, GRANTs to the
-- runtime role `console_rt`, and attaches the `enforce_org_id_immutable()` trigger
-- for EVERY table. Composite (org_id, hot-key) indexes follow. No Korean copy.

-- console-gate: audited-table email_accounts
CREATE TABLE email_accounts (
    id                        UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id                    UUID NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    branch_id                 UUID REFERENCES branches(id) ON DELETE RESTRICT,  -- NULL = org-wide
    display_name              TEXT NOT NULL CHECK (btrim(display_name) <> '' AND length(display_name) <= 200),
    email_address             TEXT NOT NULL CHECK (btrim(email_address) <> '' AND length(email_address) <= 320),
    from_name                 TEXT CHECK (from_name IS NULL OR length(from_name) <= 200),
    -- IMAP (inbound)
    imap_host                 TEXT NOT NULL CHECK (btrim(imap_host) <> '' AND length(imap_host) <= 255),
    imap_port                 INTEGER NOT NULL CHECK (imap_port IN (143, 993)),
    imap_security             TEXT NOT NULL CHECK (imap_security IN ('TLS', 'STARTTLS')),
    imap_username             TEXT NOT NULL CHECK (btrim(imap_username) <> ''),
    -- SMTP (outbound)
    smtp_host                 TEXT NOT NULL CHECK (btrim(smtp_host) <> '' AND length(smtp_host) <= 255),
    smtp_port                 INTEGER NOT NULL CHECK (smtp_port IN (465, 587, 25)),
    smtp_security             TEXT NOT NULL CHECK (smtp_security IN ('TLS', 'STARTTLS')),
    smtp_username             TEXT NOT NULL CHECK (btrim(smtp_username) <> ''),
    -- envelope-encrypted credentials (ciphertext only — never plaintext)
    smtp_password_ct          BYTEA NOT NULL,
    smtp_password_nonce       BYTEA NOT NULL,
    imap_password_ct          BYTEA NOT NULL,
    imap_password_nonce       BYTEA NOT NULL,
    dek_wrapped               BYTEA NOT NULL,   -- XChaCha20Poly1305(KEK, DEK)
    dek_nonce                 BYTEA NOT NULL,
    key_version               SMALLINT NOT NULL DEFAULT 1,
    -- sync lifecycle
    status                    TEXT NOT NULL DEFAULT 'ACTIVE'
                              CHECK (status IN ('ACTIVE', 'PAUSED', 'ERROR', 'DISABLED')),
    last_sync_at              TIMESTAMPTZ,
    last_sync_error           TEXT,
    sync_status               TEXT NOT NULL DEFAULT 'NEVER_SYNCED'
                              CHECK (sync_status IN ('OK', 'AUTH_FAILED', 'UNREACHABLE', 'NEVER_SYNCED')),
    sync_cadence_secs         INTEGER NOT NULL DEFAULT 120 CHECK (sync_cadence_secs >= 30),
    backfill_window_days      INTEGER NOT NULL DEFAULT 90 CHECK (backfill_window_days BETWEEN 1 AND 3650),
    consecutive_auth_failures INTEGER NOT NULL DEFAULT 0 CHECK (consecutive_auth_failures >= 0),
    created_by                UUID NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    created_at                TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at                TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (org_id, email_address)
);

-- console-gate: audited-table email_folders
CREATE TABLE email_folders (
    id             UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id         UUID NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    account_id     UUID NOT NULL REFERENCES email_accounts(id) ON DELETE CASCADE,
    imap_path      TEXT NOT NULL CHECK (btrim(imap_path) <> ''),
    role           TEXT NOT NULL DEFAULT 'CUSTOM'
                   CHECK (role IN ('INBOX', 'SENT', 'DRAFTS', 'ARCHIVE', 'TRASH', 'JUNK', 'CUSTOM')),
    name           TEXT NOT NULL CHECK (btrim(name) <> ''),
    uid_validity   BIGINT,
    last_seen_uid  BIGINT NOT NULL DEFAULT 0 CHECK (last_seen_uid >= 0),
    highest_modseq BIGINT,
    unread_count   INTEGER NOT NULL DEFAULT 0 CHECK (unread_count >= 0),
    total_count    INTEGER NOT NULL DEFAULT 0 CHECK (total_count >= 0),
    last_synced_at TIMESTAMPTZ,
    created_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (org_id, account_id, imap_path)
);

-- console-gate: audited-table email_threads
CREATE TABLE email_threads (
    id                   UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id               UUID NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    account_id           UUID NOT NULL REFERENCES email_accounts(id) ON DELETE CASCADE,
    normalized_subject   TEXT NOT NULL,
    subject              TEXT NOT NULL,
    last_message_at      TIMESTAMPTZ NOT NULL,
    message_count        INTEGER NOT NULL DEFAULT 0 CHECK (message_count >= 0),
    unread_count         INTEGER NOT NULL DEFAULT 0 CHECK (unread_count >= 0),
    has_attachments      BOOLEAN NOT NULL DEFAULT FALSE,
    is_flagged           BOOLEAN NOT NULL DEFAULT FALSE,
    assigned_user_id     UUID REFERENCES users(id) ON DELETE SET NULL,
    linked_work_order_id UUID REFERENCES work_orders(id) ON DELETE SET NULL,
    linked_customer_id   UUID REFERENCES registry_customers(id) ON DELETE SET NULL,
    created_at           TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at           TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- console-gate: audited-table email_messages
CREATE TABLE email_messages (
    id                UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id            UUID NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    account_id        UUID NOT NULL REFERENCES email_accounts(id) ON DELETE CASCADE,
    folder_id         UUID NOT NULL REFERENCES email_folders(id) ON DELETE CASCADE,
    thread_id         UUID NOT NULL REFERENCES email_threads(id) ON DELETE CASCADE,
    imap_uid          BIGINT CHECK (imap_uid IS NULL OR imap_uid >= 0),
    imap_uid_validity BIGINT,
    message_id        TEXT,
    in_reply_to       TEXT,
    references_ids    TEXT[] NOT NULL DEFAULT '{}',
    direction         TEXT NOT NULL CHECK (direction IN ('IN', 'OUT')),
    from_address      TEXT NOT NULL,
    from_name         TEXT,
    to_addresses      JSONB NOT NULL DEFAULT '[]',
    cc_addresses      JSONB NOT NULL DEFAULT '[]',
    bcc_addresses     JSONB NOT NULL DEFAULT '[]',
    subject           TEXT NOT NULL DEFAULT '',
    snippet           TEXT NOT NULL DEFAULT '',
    body_text         TEXT,
    body_html         TEXT,
    seen              BOOLEAN NOT NULL DEFAULT FALSE,
    flagged           BOOLEAN NOT NULL DEFAULT FALSE,
    answered          BOOLEAN NOT NULL DEFAULT FALSE,
    draft             BOOLEAN NOT NULL DEFAULT FALSE,
    has_attachments   BOOLEAN NOT NULL DEFAULT FALSE,
    send_status       TEXT CHECK (send_status IN ('PENDING', 'SENT', 'FAILED')),  -- OUT only
    send_error        TEXT,
    received_at       TIMESTAMPTZ NOT NULL,
    sent_at           TIMESTAMPTZ,
    search_vector     TSVECTOR GENERATED ALWAYS AS (
                          to_tsvector('simple',
                              coalesce(subject, '') || ' ' || coalesce(snippet, '') || ' ' ||
                              coalesce(from_name, '') || ' ' || coalesce(from_address, ''))
                      ) STORED,
    created_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at        TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- console-gate: audited-table email_attachments
CREATE TABLE email_attachments (
    id           UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id       UUID NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    message_id   UUID NOT NULL REFERENCES email_messages(id) ON DELETE CASCADE,
    s3_key       TEXT NOT NULL CHECK (btrim(s3_key) <> ''),  -- orgs/{org}/mail/{account}/{message}/{n}-{filename}
    filename     TEXT NOT NULL CHECK (btrim(filename) <> ''),
    content_type TEXT NOT NULL CHECK (btrim(content_type) <> ''),
    size_bytes   BIGINT NOT NULL CHECK (size_bytes >= 0),
    content_id   TEXT,
    is_inline    BOOLEAN NOT NULL DEFAULT FALSE,
    upload_state TEXT NOT NULL DEFAULT 'CONFIRMED' CHECK (upload_state IN ('PENDING', 'CONFIRMED')),
    sort_order   SMALLINT NOT NULL CHECK (sort_order > 0),
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (org_id, message_id, sort_order)
);

-- RLS + immutable-org trigger on every table (the 0035/0042 inline pattern).
DO $$
DECLARE
    t TEXT;
    comms_tables TEXT[] := ARRAY[
        'email_accounts', 'email_folders', 'email_threads', 'email_messages', 'email_attachments'];
BEGIN
    FOREACH t IN ARRAY comms_tables LOOP
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

-- Indexes (composite, org_id-first).
CREATE UNIQUE INDEX idx_email_messages_imap_identity
    ON email_messages (org_id, account_id, folder_id, imap_uid_validity, imap_uid)
    WHERE imap_uid IS NOT NULL;
CREATE INDEX idx_email_messages_message_id
    ON email_messages (org_id, account_id, message_id) WHERE message_id IS NOT NULL;
CREATE INDEX idx_email_messages_thread_cursor
    ON email_messages (org_id, thread_id, received_at DESC, id DESC);
CREATE INDEX idx_email_messages_folder_list
    ON email_messages (org_id, folder_id, received_at DESC);
CREATE INDEX idx_email_messages_unseen
    ON email_messages (org_id, account_id) WHERE seen = FALSE;
CREATE INDEX idx_email_messages_search ON email_messages USING GIN (search_vector);
CREATE INDEX idx_email_threads_inbox
    ON email_threads (org_id, account_id, last_message_at DESC, id DESC);
CREATE INDEX idx_email_threads_assignee
    ON email_threads (org_id, assigned_user_id) WHERE assigned_user_id IS NOT NULL;
CREATE INDEX idx_email_threads_linked_wo
    ON email_threads (org_id, linked_work_order_id) WHERE linked_work_order_id IS NOT NULL;
CREATE INDEX idx_email_threads_linked_customer
    ON email_threads (org_id, linked_customer_id) WHERE linked_customer_id IS NOT NULL;
CREATE INDEX idx_email_threads_normsubj
    ON email_threads (org_id, account_id, normalized_subject);
CREATE INDEX idx_email_folders_account ON email_folders (org_id, account_id);
CREATE INDEX idx_email_accounts_org ON email_accounts (org_id, status);
CREATE INDEX idx_email_attachments_msg ON email_attachments (org_id, message_id);
