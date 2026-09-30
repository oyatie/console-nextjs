-- T0.5 platform-auth: passkeys, persisted WebAuthn ceremonies, and
-- rotating refresh-token families with reuse detection.

CREATE TABLE auth_webauthn_credentials (
    id              UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id         UUID        NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    credential_id   TEXT        NOT NULL UNIQUE,
    passkey_json    JSONB       NOT NULL,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_used_at    TIMESTAMPTZ
);

CREATE INDEX idx_auth_webauthn_credentials_user
    ON auth_webauthn_credentials (user_id, created_at DESC);

CREATE TABLE auth_webauthn_ceremonies (
    id             UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id        UUID        REFERENCES users(id) ON DELETE CASCADE,
    ceremony_kind  TEXT        NOT NULL CHECK (ceremony_kind IN ('registration', 'authentication')),
    challenge_json JSONB       NOT NULL,
    state_json     JSONB       NOT NULL,
    expires_at     TIMESTAMPTZ NOT NULL,
    consumed_at    TIMESTAMPTZ,
    created_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK (expires_at > created_at)
);

CREATE INDEX idx_auth_webauthn_ceremonies_user_active
    ON auth_webauthn_ceremonies (user_id, ceremony_kind, expires_at)
    WHERE consumed_at IS NULL;

CREATE TABLE auth_refresh_token_families (
    id              UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id         UUID        NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    created_at      TIMESTAMPTZ NOT NULL,
    revoked_at      TIMESTAMPTZ,
    revoked_reason  TEXT
);

CREATE INDEX idx_auth_refresh_token_families_user
    ON auth_refresh_token_families (user_id, created_at DESC);

CREATE TABLE auth_refresh_tokens (
    id                 UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    family_id          UUID        NOT NULL REFERENCES auth_refresh_token_families(id) ON DELETE CASCADE,
    user_id            UUID        NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    token_hash         BYTEA       NOT NULL UNIQUE,
    issued_at          TIMESTAMPTZ NOT NULL,
    expires_at         TIMESTAMPTZ NOT NULL,
    used_at            TIMESTAMPTZ,
    replaced_by        UUID        REFERENCES auth_refresh_tokens(id),
    revoked_at         TIMESTAMPTZ,
    reuse_detected_at  TIMESTAMPTZ,
    CHECK (expires_at > issued_at)
);

CREATE INDEX idx_auth_refresh_tokens_family
    ON auth_refresh_tokens (family_id, issued_at);

CREATE INDEX idx_auth_refresh_tokens_user_active
    ON auth_refresh_tokens (user_id, expires_at)
    WHERE used_at IS NULL AND revoked_at IS NULL;
