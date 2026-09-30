-- Provenance: originated in a parallel session's worktree (commit e6764cdb);
-- adopted here and AC-verified (org-scoped RLS as console_rt, 64KiB + object
-- CHECK, audited PUT, handler-bound user rows, boundary tests) as part of the
-- UI-M1b slice.
--
-- Per-(org,user) workspace layout profile for the Oyatie Console window engine
-- (UI-M1b). The console persists each person's window/panel arrangement (pinned
-- object panels, popped-out float geometry, minimized tray chips) so the layout
-- survives navigation and reload. AD-4 of the oyatie-console plan: a server-owned
-- per-user profile. The DB policy enforces tenant/org isolation; the `/me`
-- handler and adapter bind the current principal's user_id for per-person rows.
--
-- CONTRACT: the `layout` jsonb is OPAQUE to the backend — the frontend owns its
-- shape (schema-versioned + sanitized on load). The server stores and returns it
-- verbatim. Backend guarantees only: it is a JSON object and bounded in size.
--
-- GET /api/v1/me/workspace -> { layout }  (absent row => {} default)
-- PUT /api/v1/me/workspace { layout } -> upsert for the current (org,user)
--
-- Mirrors the RLS + FORCE + REVOKE-DELETE governance pattern of 0096
-- (subject_authz_versions) and 0095 (org_runtime_flags): a runtime role may read
-- and upsert only rows in the armed org and may never DELETE them. The route
-- layer scopes `/api/v1/me/workspace` to the authenticated principal's user_id.

CREATE TABLE me_workspace_layouts (
    org_id     UUID        NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    user_id    UUID        NOT NULL,
    layout     JSONB       NOT NULL DEFAULT '{}'::jsonb
                   CHECK (jsonb_typeof(layout) = 'object'),
    -- Bound the user-writable blob (defense at the trust boundary). A console
    -- layout is a handful of panels; 64KiB is generous headroom.
    -- ponytail: fixed 64KiB cap; raise here if a real layout ever approaches it.
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (org_id, user_id),
    FOREIGN KEY (user_id, org_id) REFERENCES users(id, org_id) ON DELETE CASCADE,
    -- Defense-in-depth backstop only. pg_column_size() measures the
    -- TOAST-COMPRESSED size, so it rejects far fewer payloads than a raw byte
    -- count; the authoritative size limit is the REST handler's serialized
    -- byte-length guard (put_workspace), which returns a clean 422.
    CONSTRAINT me_workspace_layouts_size CHECK (pg_column_size(layout) <= 65536)
);

ALTER TABLE me_workspace_layouts ENABLE ROW LEVEL SECURITY;
ALTER TABLE me_workspace_layouts FORCE ROW LEVEL SECURITY;
CREATE POLICY org_isolation ON me_workspace_layouts
    USING (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid)
    WITH CHECK (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid);

GRANT SELECT, INSERT, UPDATE ON me_workspace_layouts TO console_rt;
-- Migration 0031's ALTER DEFAULT PRIVILEGES auto-grants FULL DML (incl. DELETE)
-- to console_rt on every table console_app creates, so the SELECT/INSERT/UPDATE grant
-- above is not sufficient. A workspace layout is per-user personal state; the
-- runtime role upserts it but must never DELETE it out from under RLS (mirrors
-- 0095/0096). The owner (console_app) retains DELETE so DEFINER tenant/user-removal
-- functions can still cascade.
REVOKE DELETE ON me_workspace_layouts FROM console_rt;

-- Keep updated_at honest on every upsert without permitting DELETE of state.
CREATE OR REPLACE FUNCTION me_workspace_layouts_touch_updated_at()
RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    NEW.updated_at := now();
    RETURN NEW;
END;
$$;

CREATE TRIGGER trg_me_workspace_layouts_touch_updated_at
    BEFORE UPDATE ON me_workspace_layouts
    FOR EACH ROW EXECUTE FUNCTION me_workspace_layouts_touch_updated_at();

-- Shared immutability guard: an UPDATE can never move a row across tenants even
-- if RLS WITH CHECK were relaxed (defense-in-depth, mirrors 0096).
CREATE TRIGGER trg_me_workspace_layouts_org_immutable
    BEFORE UPDATE ON me_workspace_layouts
    FOR EACH ROW EXECUTE FUNCTION enforce_org_id_immutable();
