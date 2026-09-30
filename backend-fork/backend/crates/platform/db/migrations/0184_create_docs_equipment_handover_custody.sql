-- Docs/Evidence owns the immutable custody link used by Equipment 3R handover.
-- A textual evidence:// reference is deliberately retired: Equipment stores a
-- typed object UUID, while this relation independently proves tenant, branch,
-- original-WORM, admissibility, and non-disposal eligibility at write time.
--
-- Existing handovers cannot be translated from an unverified string without
-- inventing evidence provenance. Fail closed before schema mutation so an
-- operator can register and bind each historical original through Docs first.
DO $$
BEGIN
    IF EXISTS (
        SELECT 1 FROM equipment_3r_rental_cases
         WHERE status IN ('HANDED_OVER', 'RETURNED', 'CLOSED')
    ) THEN
        RAISE EXCEPTION
            '0184 requires explicit Docs evidence custody remediation for existing Equipment handovers';
    END IF;
END $$;

ALTER TABLE equipment_3r_rental_cases
    ADD COLUMN handover_evidence_object_id UUID NULL;
ALTER TABLE equipment_3r_rental_cases
    ADD CONSTRAINT equipment_3r_cases_handover_evidence_object_org_fk
        FOREIGN KEY (handover_evidence_object_id, org_id)
        REFERENCES docs_evidence_objects(id, org_id) ON DELETE RESTRICT;
ALTER TABLE equipment_3r_rental_cases
    ADD CONSTRAINT equipment_3r_cases_handover_requires_evidence_object
        CHECK (
            status NOT IN ('HANDED_OVER', 'RETURNED', 'CLOSED')
            OR handover_evidence_object_id IS NOT NULL
        );
ALTER TABLE equipment_3r_rental_cases
    DROP COLUMN handover_evidence_reference;

-- console-gate: audited-table docs_equipment_handover_custody
CREATE TABLE docs_equipment_handover_custody (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id UUID NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    branch_id UUID NOT NULL,
    equipment_case_id UUID NOT NULL,
    evidence_object_id UUID NOT NULL,
    original_copy_id UUID NOT NULL,
    created_by UUID NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (id, org_id),
    UNIQUE (org_id, equipment_case_id),
    FOREIGN KEY (branch_id, org_id) REFERENCES branches(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (equipment_case_id, org_id)
        REFERENCES equipment_3r_rental_cases(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (evidence_object_id, org_id)
        REFERENCES docs_evidence_objects(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (original_copy_id, org_id)
        REFERENCES docs_evidence_copies(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (created_by, org_id) REFERENCES users(id, org_id) ON DELETE RESTRICT
);
CREATE INDEX idx_docs_equipment_handover_custody_evidence
    ON docs_equipment_handover_custody (org_id, evidence_object_id);

CREATE OR REPLACE FUNCTION docs_equipment_handover_custody_eligible()
RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE
    case_branch UUID;
BEGIN
    SELECT branch_id INTO case_branch
      FROM equipment_3r_rental_cases
     WHERE id = NEW.equipment_case_id AND org_id = NEW.org_id;
    IF case_branch IS NULL OR case_branch <> NEW.branch_id THEN
        RAISE EXCEPTION 'equipment handover case is not in the supplied branch'
            USING ERRCODE = '23514';
    END IF;

    IF NOT EXISTS (
        SELECT 1
          FROM docs_evidence_objects o
          JOIN docs_evidence_copies c
            ON c.id = NEW.original_copy_id
           AND c.org_id = NEW.org_id
           AND c.evidence_object_id = o.id
         WHERE o.id = NEW.evidence_object_id
           AND o.org_id = NEW.org_id
           AND o.admissibility_status = 'ADMISSIBLE'
           AND o.disposed_at IS NULL
           AND o.current_custody_stage <> 'DISPOSED'
           AND c.copy_kind = 'ORIGINAL'
           AND c.worm_status = 'VERIFIED'
           AND c.verified_at IS NOT NULL
    ) THEN
        RAISE EXCEPTION 'handover evidence must be an admissible, non-disposed original verified WORM copy'
            USING ERRCODE = '23514';
    END IF;
    RETURN NEW;
END $$;
CREATE TRIGGER trg_docs_equipment_handover_custody_eligible
    BEFORE INSERT ON docs_equipment_handover_custody
    FOR EACH ROW EXECUTE FUNCTION docs_equipment_handover_custody_eligible();

CREATE OR REPLACE FUNCTION docs_equipment_handover_custody_immutable()
RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION 'equipment handover custody is immutable' USING ERRCODE = '55000';
END $$;
CREATE TRIGGER trg_docs_equipment_handover_custody_no_mutation
    BEFORE UPDATE OR DELETE ON docs_equipment_handover_custody
    FOR EACH ROW EXECUTE FUNCTION docs_equipment_handover_custody_immutable();

ALTER TABLE docs_equipment_handover_custody ENABLE ROW LEVEL SECURITY;
ALTER TABLE docs_equipment_handover_custody FORCE ROW LEVEL SECURITY;
CREATE POLICY org_isolation ON docs_equipment_handover_custody
    USING (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid)
    WITH CHECK (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid);
GRANT SELECT, INSERT ON docs_equipment_handover_custody TO console_rt;
REVOKE UPDATE, DELETE ON docs_equipment_handover_custody FROM console_rt;
CREATE TRIGGER trg_docs_equipment_handover_custody_org_immutable
    BEFORE UPDATE ON docs_equipment_handover_custody
    FOR EACH ROW EXECUTE FUNCTION enforce_org_id_immutable();
