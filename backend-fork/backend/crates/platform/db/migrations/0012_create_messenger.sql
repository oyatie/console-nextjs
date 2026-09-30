-- T3.1 audit-grade messenger storage.
-- Postgres is the source of truth for threads, messages, membership, read
-- receipts, and search. Realtime fan-out is intentionally outside this
-- migration and arrives in T3.2.

-- console-gate: audited-table messenger_threads
CREATE TABLE messenger_threads (
    id            UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    kind          TEXT        NOT NULL CHECK (kind IN ('work_order','team','dm','group')),
    branch_id     UUID        NOT NULL REFERENCES branches(id) ON DELETE RESTRICT,
    work_order_id UUID        REFERENCES work_orders(id) ON DELETE CASCADE,
    title         TEXT        CHECK (title IS NULL OR btrim(title) <> ''),
    created_by    UUID        NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK (
        (kind = 'work_order' AND work_order_id IS NOT NULL)
        OR (kind <> 'work_order' AND work_order_id IS NULL)
    )
);

CREATE UNIQUE INDEX idx_messenger_threads_work_order
    ON messenger_threads (work_order_id)
    WHERE kind = 'work_order';

CREATE INDEX idx_messenger_threads_branch_kind
    ON messenger_threads (branch_id, kind, updated_at DESC);

-- console-gate: audited-table messenger_thread_members
CREATE TABLE messenger_thread_members (
    thread_id UUID        NOT NULL REFERENCES messenger_threads(id) ON DELETE CASCADE,
    user_id   UUID        NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    role      TEXT        NOT NULL DEFAULT 'MEMBER' CHECK (role IN ('OWNER','MEMBER')),
    joined_at TIMESTAMPTZ NOT NULL,
    PRIMARY KEY (thread_id, user_id)
);

CREATE INDEX idx_messenger_thread_members_user
    ON messenger_thread_members (user_id, thread_id);

-- console-gate: audited-table messenger_messages
CREATE TABLE messenger_messages (
    id            UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    thread_id     UUID        NOT NULL REFERENCES messenger_threads(id) ON DELETE CASCADE,
    branch_id     UUID        NOT NULL REFERENCES branches(id) ON DELETE RESTRICT,
    sender_id     UUID        NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    body          TEXT        NOT NULL CHECK (btrim(body) <> ''),
    search_vector TSVECTOR    GENERATED ALWAYS AS (to_tsvector('simple', body)) STORED,
    sent_at       TIMESTAMPTZ NOT NULL,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX idx_messenger_messages_thread_cursor
    ON messenger_messages (thread_id, sent_at DESC, id DESC);

CREATE INDEX idx_messenger_messages_branch_sent
    ON messenger_messages (branch_id, sent_at DESC);

CREATE INDEX idx_messenger_messages_search
    ON messenger_messages USING GIN (search_vector);

-- Message attachments reference already-audited evidence media rows. Upload
-- and WORM verification stay in the evidence pipeline; messenger only stores
-- the relationship to a persisted object.
-- console-gate: audited-table messenger_message_attachments
CREATE TABLE messenger_message_attachments (
    message_id  UUID     NOT NULL REFERENCES messenger_messages(id) ON DELETE CASCADE,
    evidence_id UUID     NOT NULL REFERENCES evidence_media(id) ON DELETE RESTRICT,
    sort_order  SMALLINT NOT NULL CHECK (sort_order > 0),
    PRIMARY KEY (message_id, evidence_id),
    UNIQUE (message_id, sort_order)
);

-- console-gate: audited-table messenger_read_receipts
CREATE TABLE messenger_read_receipts (
    thread_id            UUID        NOT NULL REFERENCES messenger_threads(id) ON DELETE CASCADE,
    user_id              UUID        NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    last_read_message_id UUID        NOT NULL REFERENCES messenger_messages(id) ON DELETE RESTRICT,
    read_at              TIMESTAMPTZ NOT NULL,
    updated_at           TIMESTAMPTZ NOT NULL,
    PRIMARY KEY (thread_id, user_id)
);

CREATE INDEX idx_messenger_read_receipts_message
    ON messenger_read_receipts (last_read_message_id);
