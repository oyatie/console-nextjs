-- 0055_comms_drop_smtp_port_25.sql
-- B-mail-2 security fast-follow (L3). Webmail performs ONLY authenticated message
-- submission, so the SMTP port allowlist is narrowed to 587 (STARTTLS submission)
-- and 465 (implicit TLS). Port 25 is the unauthenticated MTA-relay port — it is
-- the classic open-relay / SSRF abuse target and is never needed for submission,
-- so it is removed from the CHECK that 0053 set (smtp_port IN (465, 587, 25)).
--
-- The feature is unreleased on this branch (no email_accounts rows exist yet), so
-- there is nothing to migrate; this only tightens the constraint. The application
-- `ALLOWED_SMTP_PORTS` const (console-comms-application) mirrors this exact set.
--
-- 0053's inline `CHECK (smtp_port IN (465, 587, 25))` is auto-named by Postgres
-- (email_accounts_smtp_port_check). We drop that constraint by introspection
-- (matching the column it references) so we are robust to the auto-generated
-- name, then add an explicitly-named replacement. No Korean copy.

DO $$
DECLARE
    cons_name TEXT;
BEGIN
    -- Drop EVERY existing CHECK constraint on email_accounts that references the
    -- smtp_port column (there is exactly one: the 0053 inline check), so the
    -- migration does not depend on the auto-generated constraint name.
    FOR cons_name IN
        SELECT con.conname
        FROM pg_constraint con
        JOIN pg_class rel ON rel.oid = con.conrelid
        JOIN pg_namespace nsp ON nsp.oid = rel.relnamespace
        WHERE rel.relname = 'email_accounts'
          AND con.contype = 'c'
          AND pg_get_constraintdef(con.oid) ILIKE '%smtp_port%'
    LOOP
        EXECUTE format('ALTER TABLE email_accounts DROP CONSTRAINT %I', cons_name);
    END LOOP;

    ALTER TABLE email_accounts
        ADD CONSTRAINT email_accounts_smtp_port_check
        CHECK (smtp_port IN (465, 587));
END $$;
