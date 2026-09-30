-- Auth OTP hardening + cold-start admin bootstrap.
--
-- Two concerns, both security-critical because the registration/sign-in OTP
-- (admin-issued bootstrap token, and the fixed cold-start secret) mints the
-- first session for a pre-provisioned user:
--
--   1. Cross-instance per-client rate limiting. The deployment is multi-instance
--      (messenger uses LISTEN/NOTIFY across instances), so a per-process
--      in-memory limiter is unsound. A DB-backed fixed-window counter keyed by
--      (client_key, endpoint, window_start) bounds unauthenticated attempts on
--      the OTP-redeem / login / refresh endpoints across all instances. The rate
--      limit — NOT a per-OTP lock — is the brute-force bound: a per-OTP
--      attempt cap was deliberately rejected because locking an OTP after N
--      failures lets an attacker burn a legitimate user's OTP (targeted DoS). A
--      wrong guess therefore never consumes or invalidates an OTP; only a correct
--      redemption consumes it (single-use on success).
--
--   2. Cold-start admin. A single SUPER_ADMIN user and a single-use bootstrap
--      credential whose token is the fixed first-boot secret "coss0000" are
--      seeded idempotently so the very first administrator can sign in once, enroll
--      a passkey in initial settings, and then issue OTPs for everyone else. The
--      token is stored as the SAME sha256(token-bytes) BYTEA the application verify
--      path computes, so no parallel hashing scheme is introduced.

-- pgcrypto provides digest() for the cold-start seed below, computing the SAME
-- sha256(token-bytes) the application's hash_token() produces.
CREATE EXTENSION IF NOT EXISTS pgcrypto;

-- ---------------------------------------------------------------------------
-- 1. Cross-instance fixed-window rate limiter for unauthenticated auth calls.
-- ---------------------------------------------------------------------------
-- One row per (client_key, endpoint, window_start). The window_start is the
-- caller's clock floored to the window size; an UPSERT increments the counter
-- inside the consuming request, so the cap holds across every app instance.
-- client_key is a coarse client identifier (derived proxy-trusted IP), never
-- logged, and stored only for the rolling retention window.
CREATE TABLE auth_rate_limit (
    client_key   TEXT        NOT NULL,
    endpoint     TEXT        NOT NULL,
    window_start TIMESTAMPTZ NOT NULL,
    attempts     INTEGER     NOT NULL DEFAULT 0 CHECK (attempts >= 0),
    PRIMARY KEY (client_key, endpoint, window_start)
);

-- Sweep index for pruning expired windows.
CREATE INDEX idx_auth_rate_limit_window
    ON auth_rate_limit (window_start);

-- ---------------------------------------------------------------------------
-- 2. Cold-start SUPER_ADMIN + fixed "coss0000" single-use bootstrap credential.
-- ---------------------------------------------------------------------------
-- Idempotent: re-running this migration (or running it against a DB that
-- already has the cold-start admin) inserts nothing new. The credential is
-- single-use and expiring exactly like any OTP; once the first admin redeems
-- "coss0000" the row is consumed and dead. "coss0000" is a PUBLICLY-KNOWN,
-- intentionally low-entropy first-boot secret — it exists only to sign in the
-- very first administrator and must never be relied on past cold start. Its
-- safety rests entirely on single-use (consumed only on a correct redeem) + the
-- per-client (IP/device) rate limit + the TTL below. There is deliberately NO
-- per-OTP attempt cap: locking an OTP after N failures would let an attacker burn
-- a legitimate user's OTP (targeted DoS), so a wrong guess never consumes it.
--
-- The token_hash is digest('coss0000','sha256') which equals the application's
-- hash_token() = Sha256::digest("coss0000".as_bytes()), so the production verify
-- path consumes it with no special-casing.
DO $$
DECLARE
    cold_admin_id UUID;
BEGIN
    -- Reuse an existing cold-start admin row if present (idempotent).
    SELECT id INTO cold_admin_id
    FROM users
    WHERE display_name = 'Cold Start Admin'
      AND roles @> ARRAY['SUPER_ADMIN']::TEXT[]
    LIMIT 1;

    IF cold_admin_id IS NULL THEN
        INSERT INTO users (display_name, roles, is_active)
        VALUES ('Cold Start Admin', ARRAY['SUPER_ADMIN']::TEXT[], true)
        RETURNING id INTO cold_admin_id;
    END IF;

    -- Seed the fixed cold-start OTP only when the admin has neither a passkey nor
    -- an already-open bootstrap credential. This keeps the seed single-use and
    -- idempotent: after redemption (consumed_at set) or if a credential is still
    -- open, nothing is re-seeded.
    IF NOT EXISTS (
        SELECT 1 FROM auth_webauthn_credentials WHERE user_id = cold_admin_id
    ) AND NOT EXISTS (
        SELECT 1
        FROM auth_bootstrap_credentials
        WHERE user_id = cold_admin_id
          AND consumed_at IS NULL
          AND revoked_at IS NULL
    ) AND NOT EXISTS (
        -- Never re-seed once the fixed secret has ever been issued for this admin.
        SELECT 1
        FROM auth_bootstrap_credentials
        WHERE user_id = cold_admin_id
          AND token_hash = digest('coss0000', 'sha256')
    ) THEN
        INSERT INTO auth_bootstrap_credentials (
            user_id, token_hash, issued_at, expires_at
        )
        VALUES (
            cold_admin_id,
            digest('coss0000', 'sha256'),
            now(),
            -- Default 24h TTL: the fixed secret is first-boot-only. 24h gives an
            -- operator room to complete cold start without leaving a known OTP
            -- alive longer than necessary.
            now() + INTERVAL '24 hours'
        );
    END IF;
END $$;
