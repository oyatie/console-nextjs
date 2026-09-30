-- Issue #38 — open self-service signup: add the lowest-privilege MEMBER role.
--
-- Anyone can now self-register (POST /api/v1/auth/signup): the new account lands
-- in the default (KNL) org with a single MEMBER role and sees almost nothing
-- until an admin elevates it (authz matrix: MEMBER is default-DENY everywhere
-- but Login). The `users.roles` column CHECK from 0002 hard-codes the original
-- 5-role set, so a MEMBER insert would violate it. Extend the allowed set to
-- include MEMBER.
--
-- 0002 declared the CHECK inline on the column, so Postgres auto-named it
-- `users_roles_check`. Drop that and re-add the same `roles <@ ARRAY[...]`
-- containment check with MEMBER appended, keeping every existing role valid.
--
-- console-gate: audited-table users
ALTER TABLE users
    DROP CONSTRAINT users_roles_check,
    ADD CONSTRAINT users_roles_check CHECK (
        roles <@ ARRAY['SUPER_ADMIN','ADMIN','MECHANIC','RECEPTIONIST','EXECUTIVE','MEMBER']
    );
