-- Generic detect -> assign -> resolve chain for notifications (change-log 99).
--
-- `kind` classifies a notification's behavioral type (extensible free-form
-- string like `category`, default 'info'). A resolvable notification (e.g.
-- kind = 'slo_violation') carries a `link` target (already on the row) that a
-- LATER domain event can match to auto-resolve it: the producing domain calls
-- the generic resolve-by-link path with the same `link` shape it emitted, and
-- every still-open notification pointing at that target is marked resolved,
-- audited, in one shot. Nothing here is hardcoded to any one producer (e.g.
-- 미편성 결원 coverage breach -> 대근 편성); the mechanism is link-shaped only.

ALTER TABLE notifications
    ADD COLUMN kind TEXT NOT NULL DEFAULT 'info'
        CHECK (char_length(btrim(kind)) BETWEEN 1 AND 64),
    ADD COLUMN resolved_at TIMESTAMPTZ,
    ADD COLUMN resolved_by UUID;

ALTER TABLE notifications
    ADD CONSTRAINT notifications_resolved_by_fkey
        FOREIGN KEY (resolved_by, org_id) REFERENCES users (id, org_id) ON DELETE RESTRICT;

-- Backs "find every still-open notification for this target" during a resolve
-- sweep. ponytail: no index on `link` itself (JSONB equality scan) — add a
-- functional/GIN index if resolve-by-link volume ever makes this hot.
CREATE INDEX idx_notifications_unresolved
    ON notifications (org_id, kind)
    WHERE resolved_at IS NULL;
