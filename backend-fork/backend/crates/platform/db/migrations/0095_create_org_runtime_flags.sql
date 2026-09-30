-- Org runtime feature flags: per-tenant strangler switches for the M2 workflow
-- runtime executor built on the ADR-0018 spine (migrations 0077/0078). This table
-- is deliberately NOT a workflow runtime table: it holds no run/node state and
-- introduces no FSM surface. It is the governance switchboard that decides whether
-- a tenant is routed through the new runtime at all.
--
-- Dark-landing / parity-only contract (M2 lands completely dark):
--   * The recognized flag is workflow_runtime_m2_strangler.
--   * enabled defaults FALSE, so a written row is OFF unless explicitly enabled.
--   * This migration ships ZERO enabled rows (no INSERT/UPDATE below).
--   * An ABSENT row resolves to FALSE via org_runtime_flag_enabled(), so every
--     tenant without an explicit enabled row drives the legacy path byte-for-byte.
--   * Because no tenant is enrolled at merge, workflow_runtime_m2_strangler resolves
--     FALSE for every tenant and production behavior is unchanged.
--
-- Enrolling a tenant later (the strangler roll-forward) is a deliberate, audited,
-- tenant-scoped INSERT/UPDATE performed under the console_rt role with app.current_org
-- armed; it is intentionally out of scope for this dark landing.

CREATE TABLE org_runtime_flags (
    id           UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id       UUID        NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    flag_key     TEXT        NOT NULL CHECK (flag_key ~ '^[a-z][a-z0-9_]{2,63}$'),
    enabled      BOOLEAN     NOT NULL DEFAULT FALSE,
    rollout_note TEXT        NULL CHECK (rollout_note IS NULL OR char_length(btrim(rollout_note)) BETWEEN 1 AND 500),
    set_by       UUID        NULL,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (id, org_id),
    UNIQUE (org_id, flag_key),
    FOREIGN KEY (set_by, org_id) REFERENCES users(id, org_id) ON DELETE RESTRICT
);

-- Only enabled rows need indexing; disabled/absent flags take the dark default path.
CREATE INDEX idx_org_runtime_flags_enabled
    ON org_runtime_flags (org_id, flag_key)
    WHERE enabled;

ALTER TABLE org_runtime_flags ENABLE ROW LEVEL SECURITY;
ALTER TABLE org_runtime_flags FORCE ROW LEVEL SECURITY;
CREATE POLICY org_isolation ON org_runtime_flags
    USING (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid)
    WITH CHECK (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid);

GRANT SELECT, INSERT, UPDATE ON org_runtime_flags TO console_rt;
-- Migration 0031's ALTER DEFAULT PRIVILEGES auto-grants FULL DML (incl. DELETE)
-- to console_rt on every table console_app creates, so the SELECT/INSERT/UPDATE grant
-- above is not sufficient: without this REVOKE the runtime role could DELETE its
-- own tenant's flag row under RLS, erasing governance history and silently
-- reverting an enabled flag to the absent-row OFF state. Governance flags are
-- append/update-only for the app role — never deletable by console_rt (mirrors the
-- deliberate organizations REVOKE in 0031). The owner (console_app) retains DELETE so
-- DEFINER tenant-removal functions can still cascade.
REVOKE DELETE ON org_runtime_flags FROM console_rt;

-- Keep updated_at honest without ever permitting DELETE of governance history.
CREATE OR REPLACE FUNCTION org_runtime_flags_touch_updated_at()
RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    NEW.updated_at := now();
    RETURN NEW;
END;
$$;

CREATE OR REPLACE FUNCTION org_runtime_flags_org_immutable()
RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF OLD.org_id <> NEW.org_id THEN
        RAISE EXCEPTION 'org_runtime_flags forbids org_id changes'
            USING ERRCODE = '23514';
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER trg_org_runtime_flags_touch_updated_at
    BEFORE UPDATE ON org_runtime_flags
    FOR EACH ROW EXECUTE FUNCTION org_runtime_flags_touch_updated_at();
CREATE TRIGGER trg_org_runtime_flags_org_immutable
    BEFORE UPDATE ON org_runtime_flags
    FOR EACH ROW EXECUTE FUNCTION org_runtime_flags_org_immutable();

-- Dark-by-default resolver and single source of truth for whether a tenant drives
-- the M2 workflow runtime. It runs SECURITY INVOKER, so it reads under the caller's
-- RLS (console_rt + app.current_org) and can only ever see the current tenant's row.
-- A missing row COALESCEs to FALSE, so with zero enabled rows shipped this returns
-- FALSE for every tenant.
CREATE OR REPLACE FUNCTION org_runtime_flag_enabled(p_flag_key TEXT)
RETURNS BOOLEAN
LANGUAGE sql
STABLE
AS $$
    SELECT COALESCE(
        (
            SELECT f.enabled
            FROM org_runtime_flags f
            WHERE f.org_id = NULLIF(current_setting('app.current_org', true), '')::uuid
              AND f.flag_key = p_flag_key
        ),
        FALSE
    );
$$;

GRANT EXECUTE ON FUNCTION org_runtime_flag_enabled(TEXT) TO console_rt;
