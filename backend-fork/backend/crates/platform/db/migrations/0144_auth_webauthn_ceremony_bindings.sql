-- Persist action bindings for authenticated mobile passkey step-up ceremonies.
--
-- WebAuthn DiscoverableAuthentication state stays serialized exactly as produced
-- by webauthn-rs. The mobile-sensitive action binding lives in this side table,
-- keyed 1:1 to auth_webauthn_ceremonies, so finish/verify paths can compare the
-- endpoint-derived binding before any approval/poll mutation is allowed.

CREATE TABLE auth_webauthn_ceremony_bindings (
    ceremony_id    UUID        PRIMARY KEY REFERENCES auth_webauthn_ceremonies(id) ON DELETE CASCADE,
    action_kind    TEXT        NOT NULL CHECK (action_kind IN ('APPROVAL_DECISION', 'POLL_VOTE')),
    object_id      UUID        NOT NULL,
    reason_key     TEXT        NOT NULL CHECK (reason_key IN ('operations_passkey_approval_decision', 'operations_passkey_poll_vote')),
    replay_attempt INTEGER     CHECK (replay_attempt IS NULL OR replay_attempt >= 1),
    created_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK (
        (action_kind = 'APPROVAL_DECISION' AND reason_key = 'operations_passkey_approval_decision')
        OR (action_kind = 'POLL_VOTE' AND reason_key = 'operations_passkey_poll_vote')
    )
);

CREATE INDEX idx_auth_webauthn_ceremony_bindings_action
    ON auth_webauthn_ceremony_bindings (action_kind, object_id, replay_attempt);

-- auth_webauthn_ceremony_bindings is global transient auth state like
-- auth_webauthn_ceremonies. Runtime may insert/read bindings; deletion happens
-- only via the ceremony FK cascade.
GRANT SELECT, INSERT ON auth_webauthn_ceremony_bindings TO console_rt;
