-- Multi-tenant phase 1: the tenant table.
--
-- A tenant = one maintenance company (정비 회사) and the hard isolation
-- boundary of the system (org → region → branch → user). Every tenant-scoped
-- row carries a non-null `org_id`, and Postgres RLS keyed on the
-- `app.current_org` GUC makes cross-tenant reads/writes impossible at the
-- storage layer — a second mandatory gate beneath app-level scoping that FAILS
-- CLOSED (denies all rows) when the GUC is unset. KNL Logistics is tenant #1.
--
-- This slice proves the mechanism end-to-end on a representative cut of the
-- schema (parent + child + denormalized scoping); it is NOT yet rolled out to
-- every table.

-- console-gate: audited-table organizations
CREATE TABLE organizations (
    id         UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    slug       TEXT        NOT NULL UNIQUE
                   CHECK (slug ~ '^[a-z0-9][a-z0-9-]{1,38}[a-z0-9]$'),
    name       TEXT        NOT NULL CHECK (name <> ''),
    status     TEXT        NOT NULL DEFAULT 'ACTIVE'
                   CHECK (status IN ('ACTIVE', 'SUSPENDED', 'ARCHIVED')),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- `console_app` is the database OWNER (CNPG bootstrap.initdb.owner). It owns every
-- table and is used ONLY to run migrations; the running application must NOT
-- connect as it. The unprivileged RUNTIME role the app connects as is `console_rt`,
-- created in 0031. This DO block only guarantees the owner role exists for local
-- (non-CNPG) bootstraps; on the cluster CNPG has already created it.
--
-- Roles live in CLUSTER-global pg_authid, so when sqlx::test applies every
-- migration into many FRESH databases in parallel, an unguarded IF NOT EXISTS +
-- CREATE ROLE TOCTOU-races the shared catalog and fails with a unique_violation
-- on pg_authid_rolname_index. Serialize with a cluster-wide advisory xact lock
-- (uncontended no-op for a single production applier) and also swallow the
-- race's unique_violation in case two sessions slip between check and insert.
SELECT pg_advisory_xact_lock(hashtext('console_app_role_setup'));

DO $$
BEGIN
    IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'console_app') THEN
        CREATE ROLE console_app NOLOGIN NOBYPASSRLS;
    END IF;
EXCEPTION
    WHEN duplicate_object OR unique_violation THEN NULL;
END
$$;
