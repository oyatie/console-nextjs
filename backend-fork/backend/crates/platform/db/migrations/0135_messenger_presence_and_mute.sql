-- Messenger Slack-parity, part 3: presence read model + per-thread mute.
--
-- Presence: ONE row per user, its last_activity_at bumped on real actions the
-- user already takes (send a message, mark a thread read, ack a message) — NOT
-- a heartbeat table. online/away/offline is derived at read time from the age
-- of last_activity_at (see rest layer messenger_presence_status), so staleness
-- is explicit: "online" only means "acted within the freshness window". This
-- deliberately prefers a durable read model over the in-process WS connection
-- set in console-platform-realtime, which is per-replica and lost on restart.
-- console-gate: audited-table messenger_presence
CREATE TABLE messenger_presence (
    user_id          UUID        NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    org_id           UUID        NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    last_activity_at TIMESTAMPTZ NOT NULL,
    updated_at       TIMESTAMPTZ NOT NULL,
    PRIMARY KEY (user_id)
);

ALTER TABLE messenger_presence ENABLE ROW LEVEL SECURITY;
ALTER TABLE messenger_presence FORCE ROW LEVEL SECURITY;
CREATE POLICY org_isolation ON messenger_presence
    USING (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid)
    WITH CHECK (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid);
GRANT SELECT, INSERT, UPDATE ON messenger_presence TO console_rt;

-- Per-thread personal mute (DESIGN §3.9.0 whitelist ①, direct-save personal
-- setting). One row per (thread, user) means muted; absence means unmuted, so
-- the toggle is an idempotent insert/delete. A muted thread still records every
-- message; it only (a) suppresses THIS user's mention-notification fan-out and
-- (b) is excluded from their unread badge total (surfaced as ThreadSummary.muted
-- so the client drops it from the tab count).
-- console-gate: audited-table messenger_thread_mutes
CREATE TABLE messenger_thread_mutes (
    thread_id UUID        NOT NULL REFERENCES messenger_threads(id) ON DELETE CASCADE,
    user_id   UUID        NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    org_id    UUID        NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    muted_at  TIMESTAMPTZ NOT NULL,
    PRIMARY KEY (thread_id, user_id)
);

ALTER TABLE messenger_thread_mutes ENABLE ROW LEVEL SECURITY;
ALTER TABLE messenger_thread_mutes FORCE ROW LEVEL SECURITY;
CREATE POLICY org_isolation ON messenger_thread_mutes
    USING (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid)
    WITH CHECK (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid);
GRANT SELECT, INSERT, DELETE ON messenger_thread_mutes TO console_rt;
