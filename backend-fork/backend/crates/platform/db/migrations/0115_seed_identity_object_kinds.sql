-- Identity Console (UI-M13) slice S0 / charter G-b: register the identity
-- object kinds so account/passkey/consent chips, `!`-code deref, and the
-- person/account card history tab work through the ONE BE-OBJ registry
-- (never a second registry — charter §4 collision note).
--
-- `person` already exists (migration 0102). This appends the three remaining
-- identity kinds. All three are id/version-referenced (like `person`/`org_unit`),
-- NOT sequence-code-issuable, so `code_prefix` stays NULL (migration 0113 rule:
-- kinds referenced by id/name get no prefix; `issue_code` returns
-- DbError::CodeIssuance for them). Resolvability + per-kind visibility live in
-- Rust (backend/app/src/objects.rs resolve_head); this migration only seeds the
-- global kind registry so links/derefs can name them.
INSERT INTO object_types (kind, description) VALUES
    ('account', 'Login account (user credential subject)'),
    ('passkey', 'WebAuthn passkey credential'),
    ('consent', 'Versioned privacy/policy consent')
ON CONFLICT (kind) DO NOTHING;
