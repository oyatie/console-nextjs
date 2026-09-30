-- Cross-device passkey login handoff.
--
-- This is deliberately separate from auth_bootstrap_credentials / enrollment OTPs.
-- A desktop without a local/native passkey starts a short-lived login handoff and
-- keeps a poll token that is NEVER placed in the QR. The QR carries a distinct
-- approve token; the phone must prove possession of an existing passkey, and only
-- then can the desktop's poll token be exchanged for a normal web session.
-- This mirrors OAuth device-authorization split-token semantics and prevents a
-- scanned QR bearer from stealing the desktop session after approval.
CREATE TABLE auth_device_login_handoffs (
    id                   UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    poll_token_hash      BYTEA       NOT NULL UNIQUE,
    approve_token_hash   BYTEA       NOT NULL UNIQUE,
    issued_at            TIMESTAMPTZ NOT NULL,
    expires_at           TIMESTAMPTZ NOT NULL,
    target_user_id       UUID,
    target_org_id        UUID,
    approved_at          TIMESTAMPTZ,
    approved_user_id     UUID,
    approved_org_id      UUID,
    approved_passkey_id  UUID,
    consumed_at          TIMESTAMPTZ,
    created_at           TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK (expires_at > issued_at),
    CHECK ((target_user_id IS NULL) = (target_org_id IS NULL)),
    CHECK (
        (approved_at IS NULL AND approved_user_id IS NULL AND approved_org_id IS NULL)
        OR (approved_at IS NOT NULL AND approved_user_id IS NOT NULL AND approved_org_id IS NOT NULL)
    )
);

CREATE INDEX idx_auth_device_login_handoffs_poll_active
    ON auth_device_login_handoffs (poll_token_hash, expires_at)
    WHERE consumed_at IS NULL;

CREATE INDEX idx_auth_device_login_handoffs_approve_active
    ON auth_device_login_handoffs (approve_token_hash, expires_at)
    WHERE approved_at IS NULL AND consumed_at IS NULL;

CREATE INDEX idx_auth_device_login_handoffs_target
    ON auth_device_login_handoffs (target_user_id, issued_at DESC)
    WHERE target_user_id IS NOT NULL;

CREATE INDEX idx_auth_device_login_handoffs_user
    ON auth_device_login_handoffs (approved_user_id, approved_at DESC)
    WHERE approved_user_id IS NOT NULL;

-- Global pre-auth transient table: no org_id/RLS because the desktop poll starts
-- before a tenant is known. Grant exactly the verbs the runtime uses; no DELETE.
GRANT SELECT, INSERT, UPDATE ON auth_device_login_handoffs TO console_rt;
