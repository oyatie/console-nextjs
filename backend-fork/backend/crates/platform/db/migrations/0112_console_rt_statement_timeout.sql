-- L20 audit-chain F2: bound every console_rt transaction below the seal watermark.
--
-- The audit-chain "no gaps by construction" invariant (charter §2.2) rests on
-- SEAL_LAG (60s) exceeding the maximum audited-transaction duration: a row is
-- only sealed once `created_at <= now() - SEAL_LAG`, so its transaction has
-- certainly committed. Post-merge review F2 found that bound DID NOT EXIST for
-- background writers — `console_rt` carried no statement_timeout (only the app's 30s
-- tower HTTP TimeoutLayer, which never covers the dispatch worker, workflow
-- drainer, mail sync, apalis, or the seal worker itself). A >60s background txn
-- could commit a row below an already-advanced cursor and make verify report a
-- FALSE-POSITIVE CoverageGap.
--
-- Fix: pin a role-level statement_timeout + idle_in_transaction_session_timeout
-- on `console_rt` at 30s — matching the HTTP layer and comfortably below SEAL_LAG.
-- No legitimate background transaction exceeds this: the drainer, schedule
-- poller, mail sync, and apalis dispatch are all per-item/per-bounded-batch
-- txns, and the seal worker caps one txn at SEAL_BATCH_MAX (500) rows of one
-- SELECT + one INSERT. If a long-running audited txn is ever genuinely needed,
-- raise BOTH this timeout and SEAL_LAG together (and prefer the xmin-snapshot
-- watermark the crate's ponytail comment names).
--
-- These are ROLE-DEFAULT GUCs applied when a session authenticates AS console_rt
-- (the production login role); a superuser session that merely `SET ROLE console_rt`
-- (the sqlx test harness) does NOT inherit them, so this does not perturb tests.

-- ---------------------------------------------------------------------------
-- Superuser envs SELF-APPLY; the owner-run prod migrate only ASSERTS.
--
-- Applying the GUCs requires superuser: `ALTER ROLE console_rt SET ...` writes
-- console_rt's cluster-global pg_db_role_setting tuple, and the pg_authid row lock
-- that serializes concurrent writers (see the race note below) reads a
-- superuser-only shared catalog. So the apply + lock are gated on
-- `current_setting('is_superuser')` and split by environment:
--
--   * CI, the sqlx test harness, and local dev all migrate AS the postgres
--     superuser → they take the guarded branch, self-apply both GUCs, and the
--     FOR UPDATE lock serializes the parallel #[sqlx::test] databases.
--
--   * Production migrates AS the non-superuser table owner console_app
--     (deploy/apps/console/base/migrate-job.yaml) — it cannot read
--     pg_authid nor ALTER a role it does not own, so it SKIPS the apply branch.
--     There the GUCs are applied out-of-band by a superuser during database
--     provisioning; the exact statements an operator must run are:
--
--         ALTER ROLE console_rt SET statement_timeout = '30s';
--         ALTER ROLE console_rt SET idle_in_transaction_session_timeout = '30s';
--
-- The post-migration assertion (final DO block) runs UNCONDITIONALLY in every
-- environment. In owner-run prod it is the F2 enforcement point: if provisioning
-- did not apply the GUCs, the migration fails loudly rather than silently
-- leaving console_rt unbounded.
-- ---------------------------------------------------------------------------

-- ALTER ROLE ... SET writes the cluster-global pg_db_role_setting tuple for
-- console_rt (pg_authid/pg_db_role_setting are shared catalogs, not per-database).
-- When sqlx::test applies every migration into N fresh databases in parallel,
-- concurrent writers race on that one shared tuple ("tuple concurrently
-- updated", XX000) unless actually serialized.
--
-- `pg_advisory_xact_lock` does NOT serialize this, despite 0031 using it for an
-- analogous race: advisory locks are scoped to the CONNECTING SESSION'S
-- DATABASE, not the cluster — verified empirically (a lock held by a session in
-- database A does not block an identical lock request from a session in
-- database B; a real `FOR UPDATE` row lock on a shared catalog, by contrast,
-- does block cross-database). So the advisory lock is a no-op across parallel
-- `#[sqlx::test]` databases — exactly the case that matters here.
--
-- Lock the ACTUAL shared row instead: `pg_authid` is a genuinely global
-- catalog (one physical table, not per-database; it backs both role attributes
-- AND `pg_db_role_setting`'s role-default GUCs). `SELECT ... FOR UPDATE` against
-- console_rt's own row blocks a concurrent session in ANY database on the same
-- Postgres instance until this transaction commits (verified empirically:
-- session B in a different database timed out waiting on the row lock held by
-- session A in this database). This makes the ALTERs and the post-migration
-- assertion below genuinely race-free — no exception handler needed. The lock +
-- ALTERs run inside the DO block below, which executes in the surrounding
-- migration transaction, so the FOR UPDATE row-lock is held until this migration
-- commits exactly as a top-level statement would be. PERFORM is the PL/pgSQL
-- form of a result-discarding SELECT and preserves the FOR UPDATE lock.
DO $$
BEGIN
    IF current_setting('is_superuser') = 'on' THEN
        PERFORM rolname FROM pg_authid WHERE rolname = 'console_rt' FOR UPDATE;

        EXECUTE 'ALTER ROLE console_rt SET statement_timeout = ''30s''';
        EXECUTE 'ALTER ROLE console_rt SET idle_in_transaction_session_timeout = ''30s''';
    END IF;
END
$$;

-- Post-migration assertion: confirm both GUCs actually landed on console_rt's
-- role-default settings, and fail the migration loudly if not. Runs
-- UNCONDITIONALLY in every environment (superuser envs just self-applied them
-- above; owner-run prod asserts what provisioning applied — this is the F2
-- enforcement point). Safe to assert without retry/tolerance — in the superuser
-- case the FOR UPDATE lock above means no other session can be concurrently
-- writing this row, so a genuine ALTER failure can no longer silently defeat the
-- F2 invariant. pg_db_role_setting and pg_roles are readable by non-superusers,
-- so the owner-run prod migrate can perform this assertion.
DO $$
DECLARE
    settings TEXT[];
BEGIN
    SELECT drs.setconfig INTO settings
    FROM pg_db_role_setting drs
    JOIN pg_roles r ON r.oid = drs.setrole
    WHERE r.rolname = 'console_rt' AND drs.setdatabase = 0;

    IF settings IS NULL
        OR NOT EXISTS (SELECT 1 FROM unnest(settings) AS s WHERE s LIKE 'statement_timeout=%')
        OR NOT EXISTS (
            SELECT 1 FROM unnest(settings) AS s WHERE s LIKE 'idle_in_transaction_session_timeout=%'
        )
    THEN
        RAISE EXCEPTION
            'console_rt statement_timeout / idle_in_transaction_session_timeout did not land (pg_db_role_setting.setconfig = %) — F2 invariant not enforced',
            settings;
    END IF;
END
$$;
