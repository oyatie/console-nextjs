-- Per-(org,user) subject authorization freshness counters for the Cedar/PBAC
-- activation program (ADR-0021). This table is the DB-current side of the
-- token-snapshot vs DB-current freshness model in `cedar_pbac::SubjectFreshness`:
--   * `version` bumps on authorization-relevant subject changes (role writes),
--   * `session_generation` bumps on credential/session events that must
--     invalidate previously minted sessions (e.g. offboarding revocation).
-- An access token snapshots these at mint time; a later Cedar slice compares the
-- carried snapshot against the current row and DENIES a stale subject.
--
-- SLICE-2 CONTRACT (this migration lands with NO live authorization change):
--   * No Cedar mode is enabled and no authorization decision consults this table
--     yet. Freshness is only SOURCED here so a later slice can consult it.
--   * An ABSENT row is the "no bump yet" baseline; the mint path reads it as
--     version/session_generation 0 (safe: 0-carrying tokens are only ever denied
--     on the still-unreachable Cedar path).
--
-- NOTE (migration numbering): 0095 is taken by the parallel M2 workflow-runtime
-- branch (PR #179, 0095_create_org_runtime_flags), which is already merged into
-- this branch's history, so this file is 0096. Mirrors the RLS + REVOKE-DELETE
-- governance pattern of 0065 (policy_versions) and 0095 (org_runtime_flags).

CREATE TABLE subject_authz_versions (
    org_id             UUID        NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    user_id            UUID        NOT NULL,
    version            BIGINT      NOT NULL DEFAULT 1 CHECK (version >= 1),
    session_generation BIGINT      NOT NULL DEFAULT 1 CHECK (session_generation >= 1),
    updated_at         TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (org_id, user_id),
    FOREIGN KEY (user_id, org_id) REFERENCES users(id, org_id) ON DELETE CASCADE
);

ALTER TABLE subject_authz_versions ENABLE ROW LEVEL SECURITY;
ALTER TABLE subject_authz_versions FORCE ROW LEVEL SECURITY;
CREATE POLICY org_isolation ON subject_authz_versions
    USING (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid)
    WITH CHECK (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid);

GRANT SELECT, INSERT, UPDATE ON subject_authz_versions TO console_rt;
-- Migration 0031's ALTER DEFAULT PRIVILEGES auto-grants FULL DML (incl. DELETE)
-- to console_rt on every table console_app creates, so the SELECT/INSERT/UPDATE grant
-- above is not sufficient: without this REVOKE the runtime role could DELETE a
-- subject's freshness row under RLS, silently reverting session_generation/
-- version to the absent-row "0" baseline and defeating a stale-subject deny.
-- Freshness counters are monotonic bump-only for the app role — never deletable
-- by console_rt (mirrors 0065/0095). The owner (console_app) retains DELETE so DEFINER
-- tenant/user-removal functions can still cascade.
REVOKE DELETE ON subject_authz_versions FROM console_rt;

-- org_id is part of the primary key and never rewritten by the bump helpers, but
-- keep the shared immutability guard so a future UPDATE can never move a row
-- across tenants even if RLS WITH CHECK were relaxed (defense-in-depth, mirrors
-- the 0065 policy tables).
CREATE TRIGGER trg_subject_authz_versions_org_immutable
    BEFORE UPDATE ON subject_authz_versions
    FOR EACH ROW EXECUTE FUNCTION enforce_org_id_immutable();
