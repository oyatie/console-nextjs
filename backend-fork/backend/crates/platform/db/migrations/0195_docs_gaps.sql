-- Make the evidentiary meaning of every EV copy explicit and database-derived.
--
-- WORM replication proves storage immutability; it does not promote a derived
-- rendering to the legal/evidentiary status of the original.  A stored generated
-- column is intentionally used so no command handler, client, or backfill can
-- claim that equivalence by supplying a mutable flag.
ALTER TABLE docs_evidence_copies
    ADD COLUMN evidentiary_status TEXT GENERATED ALWAYS AS (
        CASE
            WHEN copy_kind = 'DERIVATIVE' THEN 'NON_EVIDENTIARY_DERIVATIVE'
            WHEN worm_status = 'VERIFIED' THEN 'VERIFIED_ORIGINAL'
            ELSE 'ORIGINAL_UNVERIFIED'
        END
    ) STORED;

ALTER TABLE docs_evidence_copies
    ADD CONSTRAINT docs_evidence_copies_evidentiary_status_check
    CHECK (evidentiary_status IN (
        'VERIFIED_ORIGINAL',
        'ORIGINAL_UNVERIFIED',
        'NON_EVIDENTIARY_DERIVATIVE'
    ));

CREATE INDEX idx_docs_evidence_copies_evidentiary_status
    ON docs_evidence_copies (org_id, evidence_object_id, evidentiary_status, created_at DESC);


-- A command may describe a proposed storage state, but it cannot assert
-- verification.  The only promotion authority is the storage adapter's
-- successful WORM-replica record in `evidence_media`.  This trigger binds the
-- immutable EV row to that server-written attestation by tenant, media id,
-- replica object key, canonical SHA-256, and the storage verification time.
-- Without all of those facts an EV copy is durably PENDING, regardless of
-- caller-provided `worm_status` or `verified_at` values.
CREATE OR REPLACE FUNCTION docs_evidence_copy_bind_storage_attestation()
RETURNS TRIGGER
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, public
AS $$
DECLARE
    attested_at TIMESTAMPTZ;
BEGIN
    IF NEW.source_evidence_media_id IS NOT NULL THEN
        SELECT media.verified_at
        INTO attested_at
        FROM evidence_media AS media
        WHERE media.id = NEW.source_evidence_media_id
          AND media.org_id = NEW.org_id
          AND media.worm_replica_status = 'VERIFIED'
          AND media.verified_at IS NOT NULL
          AND media.s3_key = NEW.storage_object_id
          AND media.checksum_sha256 = encode(decode(NEW.digest_sha256, 'hex'), 'base64');
    END IF;

    IF attested_at IS NOT NULL THEN
        NEW.worm_status := 'VERIFIED';
        NEW.verified_at := attested_at;
    ELSE
        NEW.worm_status := 'PENDING';
        NEW.verified_at := NULL;
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER docs_evidence_copies_bind_storage_attestation
    BEFORE INSERT ON docs_evidence_copies
    FOR EACH ROW EXECUTE FUNCTION docs_evidence_copy_bind_storage_attestation();

-- `created_at` is a business-event timestamp and can legitimately predate the
-- command that registers it. Complete register scans therefore use this
-- database-assigned identity instead: a post-snapshot insertion cannot claim a
-- sequence at or below an already-issued cursor boundary.
--
-- This migration is deliberately expand/backfill/contract rather than a direct
-- NOT NULL identity addition. Production databases can already contain EV rows:
-- give those rows a deterministic, global registration order first, then enforce
-- non-nullability (required by PostgreSQL identity columns), attach the identity
-- sequence, and reseed it above the backfill. `created_at, id` is stable and
-- total for legacy rows.
ALTER TABLE docs_evidence_objects
    ADD COLUMN register_sequence BIGINT;

WITH ordered_legacy_rows AS (
    SELECT id,
           row_number() OVER (ORDER BY created_at ASC, id ASC)::BIGINT AS sequence_value
    FROM docs_evidence_objects
)
UPDATE docs_evidence_objects AS evidence
SET register_sequence = ordered_legacy_rows.sequence_value
FROM ordered_legacy_rows
WHERE evidence.id = ordered_legacy_rows.id;

ALTER TABLE docs_evidence_objects
    ALTER COLUMN register_sequence SET NOT NULL;

ALTER TABLE docs_evidence_objects
    ALTER COLUMN register_sequence ADD GENERATED ALWAYS AS IDENTITY;

SELECT setval(
    pg_get_serial_sequence('docs_evidence_objects', 'register_sequence'),
    COALESCE((SELECT MAX(register_sequence) FROM docs_evidence_objects), 1),
    EXISTS (SELECT 1 FROM docs_evidence_objects)
);

ALTER TABLE docs_evidence_objects
    ADD CONSTRAINT docs_evidence_objects_register_sequence_positive
        CHECK (register_sequence > 0);

CREATE UNIQUE INDEX docs_evidence_objects_org_register_sequence
    ON docs_evidence_objects (org_id, register_sequence);

CREATE OR REPLACE FUNCTION docs_evidence_object_register_sequence_guard()
RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.register_sequence IS DISTINCT FROM OLD.register_sequence THEN
        RAISE EXCEPTION 'EV registration sequence is immutable for id=%', OLD.id;
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER docs_evidence_objects_register_sequence_immutable
    BEFORE UPDATE OF register_sequence ON docs_evidence_objects
    FOR EACH ROW EXECUTE FUNCTION docs_evidence_object_register_sequence_guard();


-- EV copies are append-only. Runtime code only registers copies; a direct
-- UPDATE would let an authenticated tenant forge PENDING -> VERIFIED outside
-- the storage-attestation insert path above. Keep the trigger function private
-- as defence in depth and remove the unused table UPDATE capability.
REVOKE ALL ON FUNCTION docs_evidence_copy_bind_storage_attestation() FROM PUBLIC, console_rt;
REVOKE UPDATE ON docs_evidence_copies FROM console_rt;
