-- T0.12 platform provisioning: roster idempotency and passkey cold-start
-- bootstrap credentials.

-- Roster imports use phone as the stable natural key. The roster format
-- requires phone; this partial unique index preserves compatibility with any
-- future system-created accounts that intentionally have no phone.
CREATE UNIQUE INDEX idx_users_phone_unique_present
    ON users (phone)
    WHERE phone IS NOT NULL;

CREATE TABLE auth_bootstrap_credentials (
    id                       UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id                  UUID        NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    token_hash               BYTEA       NOT NULL UNIQUE,
    issued_at                TIMESTAMPTZ NOT NULL,
    expires_at               TIMESTAMPTZ NOT NULL,
    registration_ceremony_id UUID        UNIQUE REFERENCES auth_webauthn_ceremonies(id) ON DELETE SET NULL,
    registration_started_at  TIMESTAMPTZ,
    consumed_at              TIMESTAMPTZ,
    revoked_at               TIMESTAMPTZ,
    revoked_reason           TEXT,
    created_at               TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK (expires_at > issued_at),
    CHECK (
        registration_ceremony_id IS NULL
        OR registration_started_at IS NOT NULL
    )
);

CREATE UNIQUE INDEX idx_auth_bootstrap_credentials_one_open_per_user
    ON auth_bootstrap_credentials (user_id)
    WHERE consumed_at IS NULL AND revoked_at IS NULL;

CREATE INDEX idx_auth_bootstrap_credentials_user
    ON auth_bootstrap_credentials (user_id, issued_at DESC);

CREATE INDEX idx_auth_bootstrap_credentials_active_hash
    ON auth_bootstrap_credentials (token_hash, expires_at)
    WHERE consumed_at IS NULL AND revoked_at IS NULL;
