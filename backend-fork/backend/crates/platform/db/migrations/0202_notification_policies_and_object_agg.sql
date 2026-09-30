-- Per-user notification routing policy: mute by scope (all | category | object).
-- Personal settings (DESIGN §3.9.0-①): direct-apply, recipient-owned, audited in
-- code. Muting suppresses ATTENTION (badge counts, realtime fan-out), never
-- data: rows are still persisted and listable. `action` is extensible
-- ('mute' now; 'watch' later).
--
-- NOTE: migration number 0196 is PROVISIONAL (charter-assigned; local head is
-- 0180). The consolidation integrator renumbers to the next free number
-- immediately before push, per the cross-lane collision rule.

-- console-gate: audited-table notification_policies
CREATE TABLE notification_policies (
    id          UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id      UUID        NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    user_id     UUID        NOT NULL,
    scope       TEXT        NOT NULL CHECK (scope IN ('all', 'category', 'object')),
    -- Required iff scope='category'; same bounds as notifications.category.
    category    TEXT        CHECK (category IS NULL OR char_length(btrim(category)) BETWEEN 1 AND 64),
    -- Required iff scope='object'; same JSONB link shape as notifications.link.
    link        JSONB       CHECK (link IS NULL OR jsonb_typeof(link) = 'object'),
    action      TEXT        NOT NULL DEFAULT 'mute' CHECK (action IN ('mute')),
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK (
        (scope = 'all'      AND category IS NULL AND link IS NULL) OR
        (scope = 'category' AND category IS NOT NULL AND link IS NULL) OR
        (scope = 'object'   AND category IS NULL AND link IS NOT NULL)
    ),
    UNIQUE (id, org_id),
    FOREIGN KEY (user_id, org_id) REFERENCES users(id, org_id) ON DELETE RESTRICT
);

-- One policy per exact target per user (upsert key).
CREATE UNIQUE INDEX idx_notification_policies_target
    ON notification_policies (org_id, user_id, action, scope,
                              COALESCE(category, ''), COALESCE(link::text, ''));

ALTER TABLE notification_policies ENABLE ROW LEVEL SECURITY;
ALTER TABLE notification_policies FORCE ROW LEVEL SECURITY;
CREATE POLICY org_isolation ON notification_policies
    USING (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid)
    WITH CHECK (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid);

-- Personal settings: delete = unmute (a real removal, not a governed archive).
GRANT SELECT, INSERT, UPDATE, DELETE ON notification_policies TO console_rt;

-- Backs GROUP BY link (aggregate-by-object read path) and resolve-by-link sweeps.
CREATE INDEX idx_notifications_recipient_link
    ON notifications (org_id, recipient_user_id, link);
