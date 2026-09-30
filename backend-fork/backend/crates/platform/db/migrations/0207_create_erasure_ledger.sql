-- Append-only erasure ledger: a durable record of what personal data was
-- DELETED FROM THE LIVE CLUSTER, at what scope, when, by whom, and under what
-- authority.
--
-- console-gate: audited-table erasure_ledger
--
-- WORDING IS A CONSTRAINT HERE, not a style preference. ADR-0037 records that
-- the backup ObjectStore declares no retention policy, so a row named by this
-- ledger was deleted from the live cluster and remains reconstructable from the
-- WAL archive. Nothing in this migration — comment, function name or exception
-- message — calls that destruction. This migration ADOPTS NO OPTION from
-- ADR-0037 and decides nothing about what any statute requires; every Korea
-- control stays HOLD. It records erasures. It does not perform, authorise or
-- re-apply them, and it contains no retention period and no automatic deletion.
--
-- WHY A HASH CHAIN AND NOT A PLAIN LOG. The failure this table exists to make
-- detectable is a point-in-time restore that rolls the ledger back together
-- with the data it recorded — evidence that reads as intact while being empty.
-- A monotonic sequence alone does NOT catch it: restore-then-resume drives the
-- head back up to the witnessed sequence carrying different entries, and a
-- sequence comparison calls that healthy. Measured, on this schema's trigger
-- against PostgreSQL 18.4: witness_seq=3, head_seq=3, sequence-only verdict
-- `Consistent`, hash-chain verdict `Forked`. So each entry commits to its
-- predecessor's hash, and an external holder of one (org_id, seq, entry_hash)
-- triple can tell the three cases apart.
--
-- AND ONE THING A WITNESS ALONE STILL CANNOT SEE, which is why the schema owes
-- the reader a second invariant: a restore that loses entries ABOVE the witness
-- leaves the head past it and the witnessed row byte-identical, so every check
-- anchored at that one point agrees while recorded entries are gone. Measured
-- before it was closed: entries 1..3, witness at 1, entry 2 lost, verdict
-- `Consistent { head_seq: 3 }`. The trigger below assigns `seq` as
-- `max(seq) + 1` starting at 1 and nothing can remove a row, so a live ledger
-- always satisfies `count(*) = max(seq)` PER ORG. That identity is a contract
-- this file owes `classify`, which reads it to catch exactly this case; changing
-- how `seq` is assigned breaks a caller that cannot see this comment.
--
-- WHAT IT DOES NOT SOLVE, stated here because a design that claims to survive a
-- restore without saying how is the failure mode:
--   1. Nothing outside PostgreSQL holds a witness yet, so the mechanism is
--      INERT. A restore rolls back the ledger and every in-cluster copy of its
--      high-water mark together; detection is then structurally impossible,
--      not merely unfired.
--   2. It detects; it never prevents. No in-database construct survives a
--      point-in-time restore of its own cluster.
--   3. No signature. A caller holding INSERT can record false facts at append
--      time. The chain is tamper-evident against a ROLLBACK and against nothing
--      else. It is not tamper-proof. A signing key would live in OCI Vault,
--      which is deploy/** and outside this slice.
--
-- WHY THERE IS NO FOREIGN KEY TO organizations, and it is the sharpest call in
-- this migration. The DELETE trigger below is UNCONDITIONAL — it carries no
-- `app.platform_force_remove_org` bypass branch of the kind 0090:63 uses. With
-- an unconditional trigger, EITHER foreign-key action breaks tenant teardown:
-- an `a`/`r` FK is picked up by the closure loop's
-- `EXECUTE format('DELETE FROM %I.%I WHERE org_id = $1', ...)` at 0196:149, and
-- a CASCADE FK is skipped by that loop (0196:132 filters `confdeltype IN
-- ('a','r')`) only to be cascaded onto by `DELETE FROM organizations WHERE
-- id = p_id` at 0196:315. Both land on this trigger and raise. Adding the
-- bypass branch instead would create a REACHABLE delete path through a
-- SECURITY DEFINER and make the append-only proof a lie, so: no FK, no bypass.
--
-- THE CONSEQUENCE, recorded rather than hidden: erasure-ledger rows OUTLIVE
-- `platform_force_remove_organization` and are left holding an org_id that no
-- longer names a row in `organizations`. That is defensible — the record that
-- personal data was erased should outlive the tenant whose data it was — but it
-- is a decision about a compliance record at teardown, and it is written down
-- here so the next reader does not discover it from a dangling join.
--
-- HASH FRAMING, specified so an external verifier never has to read plpgsql:
--
--   entry_hash = sha256( 'console.erasure_ledger.v1' || F1 || F2 || ... || F11 )
--
-- where each Fi is the netstring `octet_length(field)::text || ':' || field`
-- over the UTF-8 encoding of, in this exact order:
--
--   1  org_id             text form of the uuid
--   2  seq                decimal
--   3  prev_entry_hash    lowercase hex, 64 chars
--   4  subject_kind       verbatim
--   5  subject_digest     lowercase hex, 64 chars
--   6  erased_relation    verbatim
--   7  erased_selector    verbatim
--   8  erased_row_count   decimal
--   9  actor              verbatim
--   10 authority          verbatim
--   11 effective_at       YYYY-MM-DDTHH24:MI:SS.usZ, rendered at UTC
--
-- Length prefixes rather than a separator: with a separator, ('ab','c') and
-- ('a','bc') hash alike whenever the separator can occur inside a field, and
-- `erased_selector` is free text. `recorded_at` is deliberately NOT hashed — it
-- is the server clock at insert, not a recorded fact.
--
--   subject_digest = sha256( 'console.erasure_ledger.subject.v1' || F1 || F2 || F3 )
--
-- over org_id, subject_kind, subject_id in that order, same framing. This is
-- PSEUDONYMISATION, not anonymisation: the controller holds its own UUID space
-- and could reverse the mapping by enumerating it. Its purpose is narrower —
-- the ledger names a subject without carrying the subject's identifier forward
-- into a record that by construction can never be corrected.
--
-- No CREATE ROLE anywhere: cluster-global roles are infrastructure-owned and
-- this migration fails closed on drift, exactly as 0165 and 0206 do. No new
-- schema, and no SECURITY DEFINER — the chain-assigning trigger runs as the
-- caller precisely so its own reads are under RLS.

-- ---------------------------------------------------------------------------
-- 1. Preconditions. `console_rt` must exist and must still be the low-privilege
--    runtime role: a BYPASSRLS or superuser runtime role would carry the org
--    floor away with it and every isolation test here would still pass.
-- ---------------------------------------------------------------------------
DO $$
DECLARE
    v_runtime OID := pg_catalog.to_regrole('console_rt');
BEGIN
    IF v_runtime IS NULL THEN
        RAISE EXCEPTION 'erasure ledger precondition failed: console_rt is not preprovisioned';
    END IF;
    IF EXISTS (
        SELECT 1 FROM pg_catalog.pg_roles WHERE oid = v_runtime
          AND (NOT rolcanlogin OR rolsuper OR rolbypassrls OR rolinherit
               OR rolcreatedb OR rolcreaterole OR rolreplication)
    ) THEN
        RAISE EXCEPTION 'erasure ledger precondition failed: console_rt is unsafe';
    END IF;
END
$$;

-- pgcrypto supplies digest(); 0021:27 already created it. Idempotent re-create
-- so this migration does not depend on the order of an unrelated file.
CREATE EXTENSION IF NOT EXISTS pgcrypto;

-- ---------------------------------------------------------------------------
-- 2. The table. Every column is NOT NULL: an entry that cannot say WHAT was
--    erased, when, by whom and under what authority is not evidence, and the
--    not-null constraints — not the hash function — are what enforce that.
--    (`string_agg` SKIPS null inputs, so a null field would VANISH from the
--    digest rather than poison it; measured: ARRAY['a',NULL,'ccc'] frames to
--    `1:a3:ccc`. The hash functions below are STRICT for the same reason.)
--
--    Scope is a scalar relation plus a scalar selector, one row per relation.
--    An array of relations would need its own cardinality and null-element
--    checks to say the same thing, and the replay contract is identical either
--    way: every entry after the witness, in seq order.
-- ---------------------------------------------------------------------------
CREATE TABLE erasure_ledger (
    -- No REFERENCES organizations(id). See the header; this is deliberate.
    org_id           UUID        NOT NULL,
    -- Assigned by the BEFORE INSERT trigger, never by the caller.
    seq              BIGINT      NOT NULL CHECK (seq >= 1),
    subject_kind     TEXT        NOT NULL CHECK (length(btrim(subject_kind)) > 0),
    subject_digest   BYTEA       NOT NULL CHECK (octet_length(subject_digest) = 32),
    erased_relation  TEXT        NOT NULL CHECK (length(btrim(erased_relation)) > 0),
    erased_selector  TEXT        NOT NULL CHECK (length(btrim(erased_selector)) > 0),
    erased_row_count BIGINT      NOT NULL CHECK (erased_row_count >= 0),
    effective_at     TIMESTAMPTZ NOT NULL,
    actor            TEXT        NOT NULL CHECK (length(btrim(actor)) > 0),
    -- Free text, and no vocabulary CHECK: enumerating legal bases would assert
    -- a conclusion about what a statute requires. This slice asserts none.
    authority        TEXT        NOT NULL CHECK (length(btrim(authority)) > 0),
    recorded_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    prev_entry_hash  BYTEA       NOT NULL CHECK (octet_length(prev_entry_hash) = 32),
    entry_hash       BYTEA       NOT NULL CHECK (octet_length(entry_hash) = 32),
    PRIMARY KEY (org_id, seq),
    -- Fork prevention, mirroring 0100:40: exactly one entry may claim any given
    -- predecessor, and since genesis carries 32 zero bytes there is exactly one
    -- genesis per org. It is also the concurrency control — see the trigger.
    UNIQUE (org_id, prev_entry_hash)
);

-- ---------------------------------------------------------------------------
-- 3. RLS, in the form the other 141 tenant tables use.
-- ---------------------------------------------------------------------------
ALTER TABLE erasure_ledger ENABLE ROW LEVEL SECURITY;
ALTER TABLE erasure_ledger FORCE ROW LEVEL SECURITY;
CREATE POLICY org_isolation ON erasure_ledger
    USING (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid)
    WITH CHECK (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid);

-- ---------------------------------------------------------------------------
-- 4. The canonical encodings. STRICT, so a null argument returns null and the
--    insert dies on a not-null constraint instead of hashing a shorter tuple.
--    STABLE rather than IMMUTABLE, honestly: `to_char(timestamptz, text)` is
--    itself STABLE (provolatile='s' in pg_proc on 18.4), and nothing here is
--    indexed or used in a generated column, so nothing needs the stronger
--    label. Claiming IMMUTABLE over a STABLE call is how a wrong index entry
--    gets built later.
-- ---------------------------------------------------------------------------
CREATE FUNCTION erasure_ledger_subject_digest(
    p_org_id UUID,
    p_subject_kind TEXT,
    p_subject_id UUID
) RETURNS BYTEA
LANGUAGE sql
STABLE
STRICT
SET search_path = public, pg_temp
AS $$
    SELECT digest(convert_to('console.erasure_ledger.subject.v1' || (
        SELECT string_agg(octet_length(v)::text || ':' || v, '' ORDER BY ord)
          FROM unnest(ARRAY[
              p_org_id::text,
              p_subject_kind,
              p_subject_id::text
          ]) WITH ORDINALITY AS t(v, ord)
    ), 'UTF8'), 'sha256');
$$;

CREATE FUNCTION erasure_ledger_entry_hash(
    p_org_id UUID,
    p_seq BIGINT,
    p_prev_entry_hash BYTEA,
    p_subject_kind TEXT,
    p_subject_digest BYTEA,
    p_erased_relation TEXT,
    p_erased_selector TEXT,
    p_erased_row_count BIGINT,
    p_actor TEXT,
    p_authority TEXT,
    p_effective_at TIMESTAMPTZ
) RETURNS BYTEA
LANGUAGE sql
STABLE
STRICT
SET search_path = public, pg_temp
AS $$
    SELECT digest(convert_to('console.erasure_ledger.v1' || (
        SELECT string_agg(octet_length(v)::text || ':' || v, '' ORDER BY ord)
          FROM unnest(ARRAY[
              p_org_id::text,
              p_seq::text,
              encode(p_prev_entry_hash, 'hex'),
              p_subject_kind,
              encode(p_subject_digest, 'hex'),
              p_erased_relation,
              p_erased_selector,
              p_erased_row_count::text,
              p_actor,
              p_authority,
              to_char(p_effective_at AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS.US"Z"')
          ]) WITH ORDINALITY AS t(v, ord)
    ), 'UTF8'), 'sha256');
$$;

-- ---------------------------------------------------------------------------
-- 5. The chain, assigned by the DATABASE. seq, prev_entry_hash and entry_hash
--    are overwritten unconditionally, whatever the caller sent: a caller
--    holding only INSERT must not be able to forge a chain, and the chain is
--    what restore detection rests on. Computing these in application code would
--    leave append-only covering the ABSENCE OF UPDATE but not the chain itself.
--
--    Deliberately NOT SECURITY DEFINER. It runs as the caller, so the max(seq)
--    read is under RLS: with `app.current_org` armed the caller sees exactly its
--    own org's entries, and with the GUC unset it sees none and the insert then
--    fails the policy's WITH CHECK. Fails closed either way.
--
--    Running as the caller is also why `SET search_path` is not optional here.
--    Without it this function inherits the CALLER's search_path and its own
--    unqualified `erasure_ledger` and `erasure_ledger_entry_hash` resolve
--    through it. Nothing in this schema revokes TEMP on the database — 0168
--    revokes only CREATE on schema `public` — so a caller holding INSERT and
--    nothing else can put its own `erasure_ledger` in `pg_temp`, name `pg_temp`
--    ahead of `public`, and have this trigger read the head from a table it
--    wrote; temp tables carry no RLS, so the org floor does not narrow that read
--    either. MEASURED before this line existed, as `console_rt`: a planted
--    `(seq 500)` decoy produced a REAL ledger row at `seq = 501` with a
--    caller-chosen predecessor. That is the forged chain the paragraph above
--    says a caller holding only INSERT cannot build, and with it an attacker
--    re-creates the exact `(seq, entry_hash)` an external holder witnessed, so
--    `classify` answers `Consistent` for a ledger that lost entries. `public`
--    first, so the real table always wins.
--    (`erasure_ledger_append_only` below deliberately has no such clause: it
--    resolves no name at all, it only RAISEs.)
-- ---------------------------------------------------------------------------
CREATE FUNCTION erasure_ledger_assign_chain()
RETURNS TRIGGER LANGUAGE plpgsql
SET search_path = public, pg_temp
AS $$
DECLARE
    v_genesis CONSTANT BYTEA := decode(repeat('00', 32), 'hex');
BEGIN
    NEW.seq := COALESCE(
        (SELECT max(seq) FROM erasure_ledger WHERE org_id = NEW.org_id), 0) + 1;
    NEW.prev_entry_hash := COALESCE(
        (SELECT entry_hash FROM erasure_ledger
          WHERE org_id = NEW.org_id AND seq = NEW.seq - 1), v_genesis);
    NEW.entry_hash := erasure_ledger_entry_hash(
        NEW.org_id, NEW.seq, NEW.prev_entry_hash,
        NEW.subject_kind, NEW.subject_digest,
        NEW.erased_relation, NEW.erased_selector, NEW.erased_row_count,
        NEW.actor, NEW.authority, NEW.effective_at);
    RETURN NEW;
END;
$$;

CREATE TRIGGER trg_erasure_ledger_assign_chain
    BEFORE INSERT ON erasure_ledger
    FOR EACH ROW EXECUTE FUNCTION erasure_ledger_assign_chain();

-- ponytail: per-org append serialises on a max(seq) read; two concurrent
-- appends collide on (org_id, seq) or (org_id, prev_entry_hash) and the loser
-- gets 23505, which the Rust caller retries. Correct and fail-closed at
-- data-subject-request volume. Add an advisory lock only if a bulk erasure path
-- ever exists — it does not, and it is explicitly out of this slice's scope.

-- ---------------------------------------------------------------------------
-- 6. Append-only, in the database. A table-specific message rather than the
--    shared guard: at 3am the useful error names the ledger, and roughly twenty
--    table-specific siblings already exist for that reason.
--
--    NO force-remove bypass branch. See the header — this is the one place the
--    convention at 0090:63 is deliberately not followed, because a reachable
--    delete path through a SECURITY DEFINER would make the append-only proof
--    describe something that is not true.
-- ---------------------------------------------------------------------------
CREATE FUNCTION erasure_ledger_append_only()
RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    -- Statement-level: there is no OLD row to name, and referencing one here
    -- would raise "record old is not assigned yet" instead of the refusal.
    IF TG_OP = 'TRUNCATE' THEN
        RAISE EXCEPTION 'erasure_ledger is append-only: TRUNCATE is forbidden';
    END IF;
    RAISE EXCEPTION
        'erasure_ledger is append-only: % is forbidden (org_id=%, seq=%)',
        TG_OP, OLD.org_id, OLD.seq;
END;
$$;

CREATE TRIGGER trg_erasure_ledger_no_update
    BEFORE UPDATE ON erasure_ledger
    FOR EACH ROW EXECUTE FUNCTION erasure_ledger_append_only();

CREATE TRIGGER trg_erasure_ledger_no_delete
    BEFORE DELETE ON erasure_ledger
    FOR EACH ROW EXECUTE FUNCTION erasure_ledger_append_only();

-- TRUNCATE is the one statement that empties the table without producing a row
-- for a row trigger to object to, so the two triggers above do not see it at
-- all. The REVOKE below stops console_rt; MEASURED before this trigger existed,
-- the OWNER's `TRUNCATE erasure_ledger` returned `rows_affected: 0` — success —
-- and left the ledger empty. A record whose stated property is that it cannot be
-- silently emptied has to refuse the statement whose purpose is to empty it.
CREATE TRIGGER trg_erasure_ledger_no_truncate
    BEFORE TRUNCATE ON erasure_ledger
    FOR EACH STATEMENT EXECUTE FUNCTION erasure_ledger_append_only();

-- ---------------------------------------------------------------------------
-- 7. Grants. BOTH lines are load-bearing and they fail in opposite
--    environments, which is why neither can be dropped as redundant:
--
--      * 0031:75's ALTER DEFAULT PRIVILEGES auto-grants FULL DML to console_rt
--        on every table console_app creates, so in PRODUCTION the REVOKE is the
--        only thing that makes the table append-only at the privilege layer.
--      * Under #[sqlx::test] the applier is a superuser, not console_app, so
--        that default never fires and the positive GRANT is the only thing that
--        lets console_rt read or append at all.
--
--    Omit the GRANT and CI goes red; omit the REVOKE and production is mutable
--    while CI stays green. 0100:59-60 documents exactly this trap.
-- ---------------------------------------------------------------------------
GRANT SELECT, INSERT ON erasure_ledger TO console_rt;
REVOKE UPDATE, DELETE, TRUNCATE ON erasure_ledger FROM console_rt, PUBLIC;

GRANT EXECUTE ON FUNCTION erasure_ledger_subject_digest(UUID, TEXT, UUID) TO console_rt;
