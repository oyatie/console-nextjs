-- Runtime tables and functions live in public, so table/function grants are
-- unusable unless the direct serving identity can also traverse the schema.
-- Do not depend on template-database defaults: self-hosted clusters may create
-- or restore public without the traditional PUBLIC USAGE grant.
--
-- PostgreSQL 15+ no longer grants CREATE on public to PUBLIC by default, but an
-- upgraded cluster can retain that historical privilege. Remove it globally,
-- then grant only the traversal privilege required by the application runtime.
REVOKE CREATE ON SCHEMA public FROM PUBLIC, console_rt;
GRANT USAGE ON SCHEMA public TO console_rt;

DO $$
BEGIN
    IF NOT has_schema_privilege('console_rt', 'public', 'USAGE')
       OR has_schema_privilege('console_rt', 'public', 'CREATE') THEN
        RAISE EXCEPTION 'console_rt public schema privileges must be USAGE without CREATE';
    END IF;
END
$$;
