-- Revoke the committed, publicly-known cold-start OTP seed.
--
-- Migration 0021 seeded a SUPER_ADMIN "Cold Start Admin" together with a
-- bootstrap credential whose token_hash = digest('coss0000', 'sha256') — i.e. a
-- publicly-known SUPER_ADMIN sign-in OTP baked into version control. That is a
-- credential-in-the-repo finding: anyone who can read the migration can mint the
-- first super-admin session on any fresh deploy.
--
-- Cold-start is now seeded at APP BOOT from the deploy-time secret
-- CONSOLE_COLDSTART_OTP (see provisioning::BootstrapCredentialStore::seed_cold_start_credential
-- and the app composition root), never from a committed constant. This migration
-- closes the old hole on any environment that already ran 0021:
--
--   * It REVOKES (sets revoked_at = now()) every STILL-OPEN bootstrap credential
--     whose token_hash matches the fixed 'coss0000' digest, so the known secret
--     can no longer redeem. Only open rows (consumed_at IS NULL AND
--     revoked_at IS NULL) are touched; already-consumed or already-revoked rows
--     are left exactly as they are.
--   * It KEEPS the Cold Start Admin user row. The boot-time seeder re-issues a
--     fresh, operator-supplied OTP for that admin if (and only if) it still has
--     no passkey and no open credential.
--
-- Idempotent: re-running matches no open 'coss0000' rows the second time (they
-- are revoked already), so it is a clean no-op. Only an UPDATE — no schema change
-- — so the migration-safety gate has nothing to flag.

-- pgcrypto provides digest(); 0021 already created it, but keep this idempotent
-- so the migration is self-contained on a partially-applied database.
CREATE EXTENSION IF NOT EXISTS pgcrypto;

UPDATE auth_bootstrap_credentials
SET revoked_at = now(),
    revoked_reason = 'coldstart_fixed_seed_revoked'
WHERE token_hash = digest('coss0000', 'sha256')
  AND consumed_at IS NULL
  AND revoked_at IS NULL;
