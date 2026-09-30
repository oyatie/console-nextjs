-- General notifications domain.
--
-- Recipient-scoped pointer objects: a notification points a single user at
-- something that happened (an approval waiting, a mention, a document, a
-- notice, attendance, payroll — `category` is an extensible free-form string,
-- not an enum, so new producers add categories without a migration).
--
-- Persistence is the source of truth. Realtime LISTEN/NOTIFY carries only the
-- id + org; the WebSocket hub re-reads the row before fan-out. Tenant isolation
-- follows the 0030_enable_rls idiom: RLS keyed on `app.current_org`. There is
-- no per-person GUC, so recipient scoping is enforced in application code from
-- the authenticated principal, never from request input.

-- console-gate: audited-table notifications
CREATE TABLE notifications (
    id                 UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id             UUID        NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    recipient_user_id  UUID        NOT NULL,
    category           TEXT        NOT NULL
        CHECK (char_length(btrim(category)) BETWEEN 1 AND 64),
    body               TEXT        NOT NULL
        CHECK (char_length(btrim(body)) BETWEEN 1 AND 2000),
    -- Deep-link target: either an object reference {"kind","id"} or a screen
    -- {"screen"}. Stored as JSONB so the shape can evolve without a migration;
    -- the domain enum validates it before insert.
    link               JSONB       NOT NULL DEFAULT '{}'::jsonb
        CHECK (jsonb_typeof(link) = 'object'),
    unread             BOOLEAN     NOT NULL DEFAULT true,
    -- Optional stable key for at-most-once emission from an at-least-once
    -- producer (the outbox drain). Redelivery of the same source event is a
    -- no-op via the partial unique index below.
    dedup_key          TEXT        CHECK (dedup_key IS NULL OR char_length(btrim(dedup_key)) BETWEEN 1 AND 200),
    created_at         TIMESTAMPTZ NOT NULL DEFAULT now(),
    read_at            TIMESTAMPTZ,
    UNIQUE (id, org_id),
    FOREIGN KEY (recipient_user_id, org_id) REFERENCES users(id, org_id) ON DELETE RESTRICT
);

-- At-most-once emission: a producer supplying a dedup_key can safely re-drain.
CREATE UNIQUE INDEX idx_notifications_dedup
    ON notifications (org_id, recipient_user_id, dedup_key)
    WHERE dedup_key IS NOT NULL;

-- Backs the recipient inbox read path: unread-first, newest-first, keyset
-- paginated on (created_at, id). Partial-friendly composite ordered for the
-- common "my unread, newest first" list.
CREATE INDEX idx_notifications_recipient_unread
    ON notifications (org_id, recipient_user_id, unread, created_at DESC, id DESC);

ALTER TABLE notifications ENABLE ROW LEVEL SECURITY;
ALTER TABLE notifications FORCE ROW LEVEL SECURITY;
CREATE POLICY org_isolation ON notifications
    USING (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid)
    WITH CHECK (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid);

-- Notifications are recipient-owned and never hard-deleted from the runtime
-- role; read-marking is an UPDATE. console_rt gets SELECT/INSERT/UPDATE, no DELETE.
GRANT SELECT, INSERT, UPDATE ON notifications TO console_rt;
REVOKE DELETE ON notifications FROM console_rt;
