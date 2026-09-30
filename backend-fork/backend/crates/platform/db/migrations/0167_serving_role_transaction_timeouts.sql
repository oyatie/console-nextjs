-- Bound normal transactions opened through the three serving roles. Migration
-- owner, offline, and operator writers remain outside this reconciliation and
-- startup correctness backstop;
-- gap-free sealing still requires quiescence/coordination or a future
-- xmin/snapshot watermark.
-- statement_timeout and idle_in_transaction_session_timeout each bound only one
-- activity state; PostgreSQL 17+'s transaction_timeout supplies the strict
-- whole-transaction bound required below the 60-second audit seal watermark.
-- These USERSET defaults are a reconciliation/startup correctness backstop, not a security
-- boundary: each login can change its own defaults after authentication.
DO $$
DECLARE
    override RECORD;
    serving_role TEXT;
BEGIN
    IF current_setting('server_version_num')::integer < 170000 THEN
        RAISE EXCEPTION 'serving role transaction_timeout requires PostgreSQL 17 or newer';
    END IF;
    IF current_setting('max_prepared_transactions')::integer <> 0
       OR EXISTS (SELECT 1 FROM pg_prepared_xacts) THEN
        RAISE EXCEPTION 'prepared transactions bypass transaction_timeout and must remain disabled';
    END IF;

    IF current_setting('is_superuser') = 'on' THEN
        -- These shared-catalog row locks serialize parallel sqlx test databases.
        PERFORM rolname
        FROM pg_authid
        WHERE rolname IN ('console_rt', 'console_leave_cmd', 'console_ontology_cmd')
        ORDER BY rolname
        FOR UPDATE;

        FOREACH serving_role IN ARRAY ARRAY['console_rt', 'console_leave_cmd', 'console_ontology_cmd']
        LOOP
            EXECUTE format('ALTER ROLE %I SET statement_timeout = ''30s''', serving_role);
            EXECUTE format('ALTER ROLE %I SET idle_in_transaction_session_timeout = ''30s''', serving_role);
            EXECUTE format('ALTER ROLE %I SET transaction_timeout = ''45s''', serving_role);
        END LOOP;

        FOR override IN
            SELECT DISTINCT role.rolname, database.datname
            FROM pg_db_role_setting settings
            JOIN pg_roles role ON role.oid = settings.setrole
            JOIN pg_database database ON database.oid = settings.setdatabase
            CROSS JOIN LATERAL unnest(settings.setconfig) setting
            WHERE role.rolname IN ('console_rt', 'console_leave_cmd', 'console_ontology_cmd')
              AND split_part(setting, '=', 1) IN (
                'statement_timeout',
                'idle_in_transaction_session_timeout',
                'transaction_timeout'
              )
        LOOP
            EXECUTE format(
                'ALTER ROLE %I IN DATABASE %I RESET statement_timeout',
                override.rolname,
                override.datname
            );
            EXECUTE format(
                'ALTER ROLE %I IN DATABASE %I RESET idle_in_transaction_session_timeout',
                override.rolname,
                override.datname
            );
            EXECUTE format(
                'ALTER ROLE %I IN DATABASE %I RESET transaction_timeout',
                override.rolname,
                override.datname
            );
        END LOOP;
    END IF;
END
$$;

-- Production migrations run as the non-superuser owner. Assert exact global
-- values and the absence of higher-precedence database-role overrides there.
DO $$
DECLARE
    missing_or_wrong INTEGER;
BEGIN
    SELECT count(*) INTO missing_or_wrong
    FROM (VALUES
      ('console_rt'), ('console_leave_cmd'), ('console_ontology_cmd')
    ) expected(role_name)
    WHERE NOT EXISTS (
      SELECT 1
      FROM pg_db_role_setting settings
      JOIN pg_roles role ON role.oid = settings.setrole
      WHERE role.rolname = expected.role_name
        AND settings.setdatabase = 0
        AND settings.setconfig @> ARRAY[
          'statement_timeout=30s',
          'idle_in_transaction_session_timeout=30s',
          'transaction_timeout=45s'
        ]
    );

    IF missing_or_wrong <> 0 OR EXISTS (
      SELECT 1
      FROM pg_db_role_setting settings
      JOIN pg_roles role ON role.oid = settings.setrole
      CROSS JOIN LATERAL unnest(settings.setconfig) setting
      WHERE role.rolname IN ('console_rt', 'console_leave_cmd', 'console_ontology_cmd')
        AND settings.setdatabase <> 0
        AND split_part(setting, '=', 1) IN (
          'statement_timeout',
          'idle_in_transaction_session_timeout',
          'transaction_timeout'
        )
    ) THEN
        RAISE EXCEPTION 'serving role transaction timeout defaults are missing, wrong, or overridden';
    END IF;
END
$$;
