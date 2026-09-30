-- Notice board (게시판 NT- 공지): org-wide announcements with a
-- draft -> published lifecycle and per-recipient 수령확인 (receipt
-- acknowledgment) tracking. Distinct from the personal `notifications`
-- pointer table: a notice is the durable document; publishing it fans out one
-- notifications-table pointer per recipient (via the notifications write
-- port) so it surfaces on the comms rail like any other notification.
--
-- Recipients are snapshotted into notice_receipts at publish time (today: every
-- org member — narrower audience scoping is future work, ponytail: add a
-- branch/region filter column when a producer needs it).

-- console-gate: audited-table notices
CREATE TABLE notices (
    id               UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id           UUID        NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    author_user_id   UUID        NOT NULL,
    -- Issued via the shared object-code counter (kind = 'notification', prefix
    -- NT-) at publish time; NULL while the notice is still a draft.
    code             TEXT,
    title            TEXT        NOT NULL CHECK (char_length(btrim(title)) BETWEEN 1 AND 300),
    body             TEXT        NOT NULL CHECK (char_length(btrim(body)) BETWEEN 1 AND 20000),
    status           TEXT        NOT NULL DEFAULT 'draft' CHECK (status IN ('draft', 'published')),
    published_at     TIMESTAMPTZ,
    created_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (id, org_id),
    UNIQUE (org_id, code),
    FOREIGN KEY (author_user_id, org_id) REFERENCES users(id, org_id) ON DELETE RESTRICT
);

ALTER TABLE notices ENABLE ROW LEVEL SECURITY;
ALTER TABLE notices FORCE ROW LEVEL SECURITY;
CREATE POLICY org_isolation ON notices
    USING (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid)
    WITH CHECK (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid);

GRANT SELECT, INSERT, UPDATE ON notices TO console_rt;
REVOKE DELETE ON notices FROM console_rt;

-- console-gate: audited-table notice_receipts
CREATE TABLE notice_receipts (
    id                 UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id             UUID        NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    notice_id          UUID        NOT NULL,
    recipient_user_id  UUID        NOT NULL,
    acknowledged_at    TIMESTAMPTZ,
    created_at         TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (notice_id, recipient_user_id),
    FOREIGN KEY (notice_id, org_id) REFERENCES notices(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (recipient_user_id, org_id) REFERENCES users(id, org_id) ON DELETE RESTRICT
);

CREATE INDEX idx_notice_receipts_progress
    ON notice_receipts (notice_id, acknowledged_at);
CREATE INDEX idx_notice_receipts_recipient
    ON notice_receipts (org_id, recipient_user_id, notice_id);

ALTER TABLE notice_receipts ENABLE ROW LEVEL SECURITY;
ALTER TABLE notice_receipts FORCE ROW LEVEL SECURITY;
CREATE POLICY org_isolation ON notice_receipts
    USING (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid)
    WITH CHECK (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid);

GRANT SELECT, INSERT, UPDATE ON notice_receipts TO console_rt;
REVOKE DELETE ON notice_receipts FROM console_rt;
