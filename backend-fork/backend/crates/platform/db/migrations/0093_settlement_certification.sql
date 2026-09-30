-- G009 severance-certification gate on employee_exit_settlement_packages.
--
-- Statutory money-path safety invariant. A settlement package starts life as an
-- UNCERTIFIED_DRAFT and may only be marked CERTIFIED once a 노무사/세무사 has
-- attached a ProfessionalValidation artifact (payroll/domain/src/lib.rs:195 — all
-- FOUR fields: reviewer_kind, reviewed_on, artifact_sha256, reviewer_reference)
-- AND that certification is bound to the exact package revision it reviewed via a
-- deterministic SHA-256 digest. The DB is the only guarantee here: the table-level
-- `GRANT SELECT, INSERT, UPDATE ON employee_exit_settlement_packages TO console_rt`
-- from 0092 is column-unrestricted, so any UPDATE path could otherwise flip status
-- to CERTIFIED with a null/forged artifact. The CHECKs below make that state
-- unrepresentable (fail-closed).
--
-- No new GRANT or POLICY is needed: 0092 enabled+forced ROW LEVEL SECURITY and
-- created the org_isolation policy at the TABLE level, and granted UPDATE without a
-- column list. New columns therefore inherit that RLS policy and the UPDATE grant
-- automatically. The enforce_org_id_immutable trigger (0092) only guards org_id, so
-- adding columns does not affect it. Pure ALTER (no CONCURRENTLY) → no
-- `-- no-transaction` header; this runs inside the migration's implicit transaction.

ALTER TABLE employee_exit_settlement_packages
    ADD COLUMN certification_status TEXT NOT NULL DEFAULT 'UNCERTIFIED_DRAFT'
        CHECK (certification_status IN ('UNCERTIFIED_DRAFT','CERTIFIED')),
    ADD COLUMN certification_artifact JSONB NULL,
    ADD COLUMN certified_package_digest TEXT NULL,
    -- Fail-closed pairing + digest coupling: CERTIFIED is impossible without BOTH a
    -- non-null artifact AND the digest of the revision that artifact certified.
    ADD CONSTRAINT employee_exit_settlement_packages_cert_pairing_chk
        CHECK (
            certification_status = 'UNCERTIFIED_DRAFT'
            OR (certification_artifact IS NOT NULL AND certified_package_digest IS NOT NULL)
        ),
    -- Shape the artifact whenever present (independent of status, so a malformed
    -- artifact can never be parked on a draft and later flipped to CERTIFIED). The
    -- four ProfessionalValidation keys must exist; artifact_sha256 must be a
    -- 64-hex-char SHA-256; reviewer_kind must be a known reviewer type. reviewer_kind
    -- is SCREAMING_SNAKE_CASE to match this table's status convention and the
    -- ProfessionalReviewerKind variants (LaborAttorney → LABOR_ATTORNEY,
    -- TaxAccountant → TAX_ACCOUNTANT); the digest/artifact-writing story must
    -- serialize to these exact tokens.
    ADD CONSTRAINT employee_exit_settlement_packages_cert_artifact_shape_chk
        CHECK (
            certification_artifact IS NULL
            OR (
                certification_artifact ?& ARRAY[
                    'reviewer_kind',
                    'reviewed_on',
                    'artifact_sha256',
                    'reviewer_reference'
                ]
                AND certification_artifact->>'artifact_sha256' ~ '^[0-9a-f]{64}$'
                AND certification_artifact->>'reviewer_kind' IN ('LABOR_ATTORNEY','TAX_ACCOUNTANT')
            )
        ),
    -- The bound digest, when present, must itself be a well-formed SHA-256.
    ADD CONSTRAINT employee_exit_settlement_packages_cert_digest_shape_chk
        CHECK (
            certified_package_digest IS NULL
            OR certified_package_digest ~ '^[0-9a-f]{64}$'
        );
