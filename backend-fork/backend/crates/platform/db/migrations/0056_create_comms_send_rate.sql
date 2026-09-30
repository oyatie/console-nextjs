-- 0056_create_comms_send_rate.sql
-- B-mail-2 security fast-follow (M1): a persisted, per-org + per-user rolling
-- (fixed-window) rate limiter for outbound webmail.
--
-- The webmail send/reply/forward and SMTP test-connection paths make an outbound
-- network call on behalf of a tenant user. Without a bound, a compromised or
-- abusive account could pump unbounded mail (spam / resource exhaustion) or use
-- the test-connection probe as an SSRF/port-scan amplifier. A per-process,
-- in-memory limiter is unsound here because the deployment is multi-instance, so
-- the counter is persisted in Postgres and incremented inside the consuming
-- request — the cap therefore holds across every app instance.
--
-- Unlike the GLOBAL pre-auth `auth_rate_limit` table (no org_id, no RLS — it
-- counts UNAUTHENTICATED attempts keyed by a coarse client identifier), this
-- counter is for AUTHENTICATED tenant actors, so it is org-scoped and RLS-armed
-- exactly like every other tenant table: one row per
-- (org_id, actor_user_id, endpoint, window_start). The app arms `app.current_org`
-- via with_org_conn before the UPSERT; an unset GUC fails closed (zero rows /
-- rejected write), so one org's counter can never read or bump another's.
--
-- This is a coarse counter, NOT an audited state change: like auth_rate_limit /
-- the sales inquiry limiter it is deliberately exempt from the audit-coverage
-- gate (it carries no business meaning and must stay cheap on the hot path).
--
-- Follows the post-multi-tenant house style (verified against 0053/0042/0035):
-- the natural key is the 4-tuple, so the composite PRIMARY KEY IS that 4-tuple
-- (there is no surrogate `id` column — this is a coarse counter, not an entity)
-- and `org_id` is the leading column for the org_isolation policy. The shared
-- enforce_org_id_immutable() trigger is deliberately NOT attached here: it
-- references OLD.id (which this id-less table lacks), and the RLS WITH CHECK
-- policy below already forbids an UPDATE from moving a row to another org (the
-- armed app.current_org must equal the row's org_id on both read and write). No
-- Korean copy.

-- console-gate: audited-table comms_send_rate
CREATE TABLE comms_send_rate (
    org_id          UUID        NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    actor_user_id   UUID        NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    -- The limited operation + its window size, e.g. 'mail_send:1m', 'mail_send:1h',
    -- 'mail_test:1m'. Encoding the window size in the key keeps multiple
    -- concurrent windows (per-minute AND per-hour) independent.
    endpoint        TEXT        NOT NULL CHECK (btrim(endpoint) <> '' AND length(endpoint) <= 64),
    -- The caller's clock floored to the window size; the UPSERT increments the
    -- counter for the live window.
    window_start    TIMESTAMPTZ NOT NULL,
    attempts        INTEGER     NOT NULL DEFAULT 0 CHECK (attempts >= 0),
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (org_id, actor_user_id, endpoint, window_start)
);

-- RLS (the 0035/0053 inline pattern; immutable-org enforced by WITH CHECK here).
ALTER TABLE comms_send_rate ENABLE ROW LEVEL SECURITY;
ALTER TABLE comms_send_rate FORCE ROW LEVEL SECURITY;
CREATE POLICY org_isolation ON comms_send_rate
    USING (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid)
    WITH CHECK (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid);
GRANT SELECT, INSERT, UPDATE, DELETE ON comms_send_rate TO console_rt;

-- Sweep index for pruning expired windows (a later janitor deletes old rows).
CREATE INDEX idx_comms_send_rate_window ON comms_send_rate (window_start);
