-- L20 tamper-evident audit-chain seals (design charter §3).
--
-- PROVISIONAL NUMBER: 0100 is the first free slot after 0099, but the lead
-- reserved 0100+ for a concurrent lane. Re-confirm the migrations dir is
-- contiguous and renumber to the next free slot at merge.
--
-- A per-org, append-only, cryptographically-sealed hash chain over
-- `audit_events`. A background worker (console-platform-audit-chain) seals batches
-- of audit rows in `(created_at, id)` order; `verify_org_chain` recomputes and
-- compares. Detection defends against a party with direct DB write access (the
-- console_app owner, a leaked superuser, an edited backup restore) who can bypass
-- the append-only triggers/grants on audit_events — a row edit/delete/insert or
-- reorder recomputes a divergent batch_hash/seal_hash. It does NOT defend
-- against a party who ALSO holds the seal signing key (custody boundary — the
-- private key lives in OCI Vault, never in-crate; §4).
--
-- Governance mirrors 0096_create_subject_authz_versions.sql EXACTLY (RLS +
-- explicit GRANT + org-immutability trigger), tightened: seals are FULLY
-- immutable evidence, so console_rt gets SELECT+INSERT only and is REVOKEd both
-- UPDATE and DELETE (0096 kept UPDATE for its bump counters; seals never bump).

CREATE TABLE audit_chain_seals (
    org_id          UUID        NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    seq             BIGINT      NOT NULL CHECK (seq >= 1),
    from_event_id   UUID        NOT NULL,          -- first row in range (inclusive)
    from_created_at TIMESTAMPTZ NOT NULL,
    to_event_id     UUID        NOT NULL,          -- last row in range (inclusive) = new cursor
    to_created_at   TIMESTAMPTZ NOT NULL,
    row_count       BIGINT      NOT NULL CHECK (row_count >= 1),
    batch_hash      BYTEA       NOT NULL,          -- 32 bytes (SHA-256 over the row hashes)
    prev_seal_hash  BYTEA       NOT NULL,          -- 32 bytes ([0;32] at genesis)
    seal_hash       BYTEA       NOT NULL,          -- 32 bytes (commits to prev_seal_hash → chain)
    signature       BYTEA       NOT NULL,          -- signer signature over seal_hash
    key_ref         TEXT        NOT NULL,          -- opaque signer key identifier (verify uses THIS key)
    sealed_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (org_id, seq),
    -- continuity uniqueness: exactly one seal starts where the previous ended,
    -- so two seals can never both claim the same predecessor (chain fork). At
    -- genesis prev_seal_hash = [0;32], so there is exactly one genesis per org.
    UNIQUE (org_id, prev_seal_hash)
);

ALTER TABLE audit_chain_seals ENABLE ROW LEVEL SECURITY;
ALTER TABLE audit_chain_seals FORCE ROW LEVEL SECURITY;
CREATE POLICY org_isolation ON audit_chain_seals
    USING (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid)
    WITH CHECK (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid);

-- Seals are append-only evidence: SELECT + INSERT only. 0031's ALTER DEFAULT
-- PRIVILEGES auto-grants FULL DML (incl. UPDATE + DELETE) to console_rt on every
-- table console_app creates, so BOTH must be revoked here — without it the runtime
-- role that writes seals could silently rewrite a seal_hash/batch_hash or DELETE
-- a seal and re-point the chain, laundering a tampered audit_events. The
-- explicit positive GRANT is required in the test DB (tables owned by the test
-- superuser, where the ALTER DEFAULT PRIVILEGES clause never fires); the REVOKE
-- of a not-yet-held privilege is a harmless no-op there and the real tightening
-- in production. The owner (console_app) retains DELETE for ON DELETE CASCADE
-- tenant teardown.
GRANT SELECT, INSERT ON audit_chain_seals TO console_rt;
REVOKE UPDATE, DELETE ON audit_chain_seals FROM console_rt;

-- org_id is in the PK and never rewritten, but keep an immutability guard so a
-- future owner UPDATE can never move a seal across tenants even if RLS WITH
-- CHECK were relaxed (defense-in-depth, mirrors 0096). Use a table-specific
-- function because the shared 0031 function reports `OLD.id`, while this table's
-- stable identity is `(org_id, seq)`.
CREATE OR REPLACE FUNCTION audit_chain_seals_org_id_immutable()
RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.org_id IS DISTINCT FROM OLD.org_id THEN
        RAISE EXCEPTION
            'audit_chain_seals org_id is immutable: cannot change tenant from % to % on seq=%',
            OLD.org_id, NEW.org_id, OLD.seq;
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER trg_audit_chain_seals_org_immutable
    BEFORE UPDATE ON audit_chain_seals
    FOR EACH ROW EXECUTE FUNCTION audit_chain_seals_org_id_immutable();

-- ponytail: no separate (org_id, seq DESC) index. The PRIMARY KEY (org_id, seq)
-- btree already serves the only hot read — MAX(seq)/head lookup — via a backward
-- index scan, so a second index would only write-amplify the append-only insert
-- path. Add one only if a range scan pattern that the PK cannot serve appears.
