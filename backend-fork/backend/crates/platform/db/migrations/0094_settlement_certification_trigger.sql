-- G009 statutory money-path hardening (PR #166 completion review, HIGH #1/#2 + MEDIUM #4).
--
-- Three defects in 0092/0093 are closed here:
--
--   HIGH #1 — atomic re-uncertification was only an APPLICATION convention (two
--   hand-written UPDATE statements in hr.rs reset the certification columns).
--   Any other write path — direct SQL, a future endpoint, a bulk fix — could
--   mutate a certification-covered field and leave a CERTIFIED row bound to a
--   stale severance figure. This migration makes the invariant a TABLE invariant
--   via a BEFORE UPDATE trigger, so it holds for EVERY UPDATE regardless of path.
--
--   HIGH #2 — the certified digest omitted the 통상임금 (ordinary-wage) basis. The
--   ordinary wage can change while the average wage still governs, or two ordinary
--   inputs can floor to the same severance, so binding it only transitively via
--   severance_pay_won was false. The three ordinary-wage basis figures are now
--   persisted columns, folded into the digest (hr.rs compute_certified_package_digest)
--   AND into the certification-covered set of the trigger below.
--
--   MEDIUM #4 — the 0093 artifact-shape CHECK required the four ProfessionalValidation
--   keys to EXIST but tolerated extra keys. It is dropped and recreated to require
--   an object with EXACTLY those four keys.
--
-- Pure ALTER + CREATE FUNCTION/TRIGGER (no CONCURRENTLY) → no `-- no-transaction`
-- header; every statement runs inside the migration's implicit transaction.

-- ---------------------------------------------------------------------------
-- HIGH #2 — persist the 통상임금 (ordinary-wage) statutory basis.
--
-- monthly_ordinary_wage_won      : 월 통상임금 input (the figure a reviewer signs).
-- ordinary_daily_wage_won        : 통상일급 derived via the 209h/8h rule (hr.rs).
-- statutory_daily_wage_milliwon  : the daily wage that ACTUALLY governed severance
--                                  = max(average_daily, ordinary_daily) (payroll kernel).
-- These make the money trail auditable (which ordinary input was compared against
-- the average wage) and are certification-covered fields (see the trigger below).
-- ---------------------------------------------------------------------------
ALTER TABLE employee_exit_settlement_packages
    ADD COLUMN monthly_ordinary_wage_won    BIGINT NULL
        CHECK (monthly_ordinary_wage_won IS NULL OR monthly_ordinary_wage_won >= 0),
    ADD COLUMN ordinary_daily_wage_won      BIGINT NULL
        CHECK (ordinary_daily_wage_won IS NULL OR ordinary_daily_wage_won >= 0),
    ADD COLUMN statutory_daily_wage_milliwon BIGINT NULL
        CHECK (statutory_daily_wage_milliwon IS NULL OR statutory_daily_wage_milliwon >= 0);

-- ---------------------------------------------------------------------------
-- HIGH #1 — DB-enforced atomic re-uncertification.
--
-- If ANY certification-covered column changes on an UPDATE, the certification is
-- reset in the SAME write, so a CERTIFIED flag can never outlive the numbers it
-- certified — no matter which path issues the UPDATE. The certification-recording
-- action (v1 ships none yet) updates ONLY the three certification columns, which
-- are NOT in the covered set below, so the trigger leaves a genuine certification
-- intact.
--
-- COVERED-COLUMN SET (MUST stay byte-identical to the field set hashed by
-- hr.rs::compute_certified_package_digest — the two are cross-referenced):
--   severance_pay_won, statutory_basis, insurance_loss_payload, approval_payload,
--   average_wage_period_start, average_wage_period_end, average_wage_calendar_days,
--   average_wage_total_won, average_daily_wage_milliwon, service_days,
--   monthly_ordinary_wage_won, ordinary_daily_wage_won, statutory_daily_wage_milliwon.
-- NOT covered (so a certification write survives): certification_status,
-- certification_artifact, certified_package_digest, status, missing_source_fields,
-- generated_at, submitted_by, submitted_at, timestamps, org_id, id, *_case_id, employee_id.
-- ---------------------------------------------------------------------------
CREATE OR REPLACE FUNCTION enforce_settlement_certification_reset()
RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.severance_pay_won             IS DISTINCT FROM OLD.severance_pay_won
    OR NEW.statutory_basis               IS DISTINCT FROM OLD.statutory_basis
    OR NEW.insurance_loss_payload        IS DISTINCT FROM OLD.insurance_loss_payload
    OR NEW.approval_payload              IS DISTINCT FROM OLD.approval_payload
    OR NEW.average_wage_period_start     IS DISTINCT FROM OLD.average_wage_period_start
    OR NEW.average_wage_period_end       IS DISTINCT FROM OLD.average_wage_period_end
    OR NEW.average_wage_calendar_days    IS DISTINCT FROM OLD.average_wage_calendar_days
    OR NEW.average_wage_total_won        IS DISTINCT FROM OLD.average_wage_total_won
    OR NEW.average_daily_wage_milliwon   IS DISTINCT FROM OLD.average_daily_wage_milliwon
    OR NEW.service_days                  IS DISTINCT FROM OLD.service_days
    OR NEW.monthly_ordinary_wage_won     IS DISTINCT FROM OLD.monthly_ordinary_wage_won
    OR NEW.ordinary_daily_wage_won       IS DISTINCT FROM OLD.ordinary_daily_wage_won
    OR NEW.statutory_daily_wage_milliwon IS DISTINCT FROM OLD.statutory_daily_wage_milliwon
    THEN
        NEW.certification_status := 'UNCERTIFIED_DRAFT';
        NEW.certification_artifact := NULL;
        NEW.certified_package_digest := NULL;
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER trg_employee_exit_settlement_packages_cert_reset
    BEFORE UPDATE ON employee_exit_settlement_packages
    FOR EACH ROW EXECUTE FUNCTION enforce_settlement_certification_reset();

-- ---------------------------------------------------------------------------
-- MEDIUM #4 — tighten the artifact-shape CHECK to EXACTLY four keys.
--
-- Postgres 18 has no jsonb_object_length(), and a CHECK cannot contain a
-- subquery, so "exactly four keys" is expressed as: object type + the four keys
-- exist (?&) + removing those four keys leaves an empty object ({}). The `-`
-- deletion operator raises on scalars, so it is guarded by a CASE that only runs
-- it once the value is confirmed to be an object (AND is not guaranteed to
-- short-circuit in a CHECK). SHA-256 and reviewer_kind checks are preserved.
-- ---------------------------------------------------------------------------
ALTER TABLE employee_exit_settlement_packages
    DROP CONSTRAINT employee_exit_settlement_packages_cert_artifact_shape_chk;

ALTER TABLE employee_exit_settlement_packages
    ADD CONSTRAINT employee_exit_settlement_packages_cert_artifact_shape_chk
    CHECK (
        certification_artifact IS NULL
        OR (
            jsonb_typeof(certification_artifact) = 'object'
            AND certification_artifact ?& ARRAY[
                'reviewer_kind',
                'reviewed_on',
                'artifact_sha256',
                'reviewer_reference'
            ]
            AND certification_artifact->>'artifact_sha256' ~ '^[0-9a-f]{64}$'
            AND certification_artifact->>'reviewer_kind' IN ('LABOR_ATTORNEY','TAX_ACCOUNTANT')
            AND CASE
                WHEN jsonb_typeof(certification_artifact) = 'object'
                THEN certification_artifact - ARRAY[
                    'reviewer_kind',
                    'reviewed_on',
                    'artifact_sha256',
                    'reviewer_reference'
                ]::text[] = '{}'::jsonb
                ELSE false
            END
        )
    );
