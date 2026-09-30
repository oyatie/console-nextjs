-- Performance-review cycles (CAP-EVALUATION-CONSOLE). Design authority:
-- docs/design/oyatie-console (screen "review", HANDOFF §15/§16); buildable
-- contract in docs/evidence/console/CAP-EVALUATION-CONSOLE/design-contract.md.
-- RLS / grant / trigger conventions copied from 0172 and 0179.

-- Register the capability keys before any tenant policy can grant them. The
-- routes stay fail-closed: catalog presence is only a prerequisite for an
-- explicit ACTIVE custom-role assignment, never an implicit permission.
INSERT INTO feature_catalog (feature_key) VALUES
    ('evaluation_read'),
    ('evaluation_manage'),
    ('evaluation_submit')
ON CONFLICT (feature_key) DO NOTHING;

CREATE TABLE evaluation_cycles (
    id            UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id        UUID NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    name          TEXT NOT NULL CHECK (btrim(name) <> '' AND char_length(name) <= 120),
    kind          TEXT NOT NULL CHECK (kind IN ('REGULAR', 'PROBATION')),
    period_label  TEXT NOT NULL CHECK (btrim(period_label) <> '' AND char_length(period_label) <= 60),
    due_date      DATE NOT NULL,
    stage         TEXT NOT NULL DEFAULT 'DRAFT'
                  CHECK (stage IN ('DRAFT', 'OPEN', 'CALIBRATION', 'FINALIZED', 'ARCHIVED')),
    created_by    UUID NOT NULL,
    opened_at     TIMESTAMPTZ,
    calibration_started_at TIMESTAMPTZ,
    finalized_at  TIMESTAMPTZ,
    archived_at   TIMESTAMPTZ,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (id, org_id),
    FOREIGN KEY (created_by, org_id) REFERENCES users(id, org_id) ON DELETE RESTRICT
);
CREATE INDEX evaluation_cycles_org_stage_idx ON evaluation_cycles (org_id, stage, due_date);

CREATE TABLE evaluation_subjects (
    id                 UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id             UUID NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    cycle_id           UUID NOT NULL,
    employee_id        UUID NOT NULL,
    manager_user_id    UUID NOT NULL,
    calibrated_grade   TEXT CHECK (calibrated_grade IN ('S', 'A', 'B', 'C', 'D')),
    calibration_reason TEXT CHECK (calibration_reason IS NULL
                                   OR (btrim(calibration_reason) <> '' AND char_length(calibration_reason) <= 500)),
    calibrated_by      UUID,
    calibrated_at      TIMESTAMPTZ,
    final_grade        TEXT CHECK (final_grade IN ('S', 'A', 'B', 'C', 'D')),
    rv_code            TEXT CHECK (rv_code ~ '^RV-[0-9]{4,}$'),
    finalized_at       TIMESTAMPTZ,
    created_at         TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at         TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (id, org_id),
    UNIQUE (cycle_id, employee_id),
    UNIQUE (org_id, rv_code),
    FOREIGN KEY (cycle_id, org_id)        REFERENCES evaluation_cycles(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (employee_id, org_id)     REFERENCES employees(id, org_id)         ON DELETE RESTRICT,
    FOREIGN KEY (manager_user_id, org_id) REFERENCES users(id, org_id)             ON DELETE RESTRICT,
    FOREIGN KEY (calibrated_by, org_id)   REFERENCES users(id, org_id)             ON DELETE RESTRICT,
    CHECK ((calibrated_grade IS NULL) = (calibrated_by IS NULL)),
    CHECK ((calibrated_grade IS NULL) = (calibrated_at IS NULL)),
    CHECK ((final_grade IS NULL) = (rv_code IS NULL)),
    CHECK ((final_grade IS NULL) = (finalized_at IS NULL))
);
CREATE INDEX evaluation_subjects_org_cycle_idx    ON evaluation_subjects (org_id, cycle_id);
CREATE INDEX evaluation_subjects_org_employee_idx ON evaluation_subjects (org_id, employee_id)
    WHERE finalized_at IS NOT NULL;
CREATE INDEX evaluation_subjects_org_manager_idx  ON evaluation_subjects (org_id, manager_user_id);

CREATE TABLE evaluation_goals (
    id           UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id       UUID NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    subject_id   UUID NOT NULL,
    title        TEXT NOT NULL CHECK (btrim(title) <> '' AND char_length(title) <= 200),
    metric_kind  TEXT NOT NULL CHECK (metric_kind IN ('KPI', 'ATTENDANCE', 'TASK', 'CUSTOM')),
    target_label TEXT NOT NULL CHECK (btrim(target_label) <> '' AND char_length(target_label) <= 200),
    weight_pct   SMALLINT NOT NULL CHECK (weight_pct BETWEEN 0 AND 100),
    sort_order   INTEGER NOT NULL CHECK (sort_order >= 0),
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (subject_id, sort_order),
    FOREIGN KEY (subject_id, org_id) REFERENCES evaluation_subjects(id, org_id) ON DELETE RESTRICT
);

CREATE TABLE evaluation_reviews (
    id                UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id            UUID NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    subject_id        UUID NOT NULL,
    kind              TEXT NOT NULL CHECK (kind IN ('SELF', 'MANAGER')),
    status            TEXT NOT NULL DEFAULT 'DRAFT' CHECK (status IN ('DRAFT', 'SUBMITTED')),
    evaluator_user_id UUID NOT NULL,
    grade             TEXT CHECK (grade IN ('S', 'A', 'B', 'C', 'D')),
    note              TEXT CHECK (note IS NULL OR char_length(note) <= 2000),
    submitted_at      TIMESTAMPTZ,
    created_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (id, org_id),
    UNIQUE (subject_id, kind),
    FOREIGN KEY (subject_id, org_id)        REFERENCES evaluation_subjects(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (evaluator_user_id, org_id) REFERENCES users(id, org_id)               ON DELETE RESTRICT,
    CHECK (status = 'DRAFT' OR (grade IS NOT NULL AND submitted_at IS NOT NULL))
);

CREATE TABLE evaluation_evidence_links (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id      UUID NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    review_id   UUID NOT NULL,
    object_kind TEXT NOT NULL CHECK (object_kind IN ('ATTENDANCE', 'WORK_ORDER', 'APPROVAL', 'KPI', 'OTHER')),
    object_ref  TEXT NOT NULL CHECK (btrim(object_ref) <> '' AND char_length(object_ref) <= 120),
    label       TEXT NOT NULL CHECK (btrim(label) <> '' AND char_length(label) <= 200),
    sort_order  INTEGER NOT NULL DEFAULT 0 CHECK (sort_order >= 0),
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (review_id, sort_order),
    FOREIGN KEY (review_id, org_id) REFERENCES evaluation_reviews(id, org_id) ON DELETE RESTRICT
);

-- RV- code issuance: per-org monotone counter, locked FOR UPDATE at finalize.
-- Deliberately no RLS bypass: the finalize transaction runs under
-- app.current_org, so the counter row is only visible inside the owning tenant.
CREATE TABLE evaluation_code_counters (
    org_id     UUID PRIMARY KEY REFERENCES organizations(id) ON DELETE RESTRICT,
    next_value INTEGER NOT NULL DEFAULT 2500 CHECK (next_value > 0)
);

-- Every evaluation object is tenant-concealed (HR-sensitive).
ALTER TABLE evaluation_cycles         ENABLE ROW LEVEL SECURITY;
ALTER TABLE evaluation_cycles         FORCE  ROW LEVEL SECURITY;
ALTER TABLE evaluation_subjects       ENABLE ROW LEVEL SECURITY;
ALTER TABLE evaluation_subjects       FORCE  ROW LEVEL SECURITY;
ALTER TABLE evaluation_goals          ENABLE ROW LEVEL SECURITY;
ALTER TABLE evaluation_goals          FORCE  ROW LEVEL SECURITY;
ALTER TABLE evaluation_reviews        ENABLE ROW LEVEL SECURITY;
ALTER TABLE evaluation_reviews        FORCE  ROW LEVEL SECURITY;
ALTER TABLE evaluation_evidence_links ENABLE ROW LEVEL SECURITY;
ALTER TABLE evaluation_evidence_links FORCE  ROW LEVEL SECURITY;
ALTER TABLE evaluation_code_counters  ENABLE ROW LEVEL SECURITY;
ALTER TABLE evaluation_code_counters  FORCE  ROW LEVEL SECURITY;

CREATE POLICY org_isolation ON evaluation_cycles
    USING (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid)
    WITH CHECK (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid);

CREATE POLICY org_isolation ON evaluation_subjects
    USING (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid)
    WITH CHECK (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid);

CREATE POLICY org_isolation ON evaluation_goals
    USING (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid)
    WITH CHECK (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid);

CREATE POLICY org_isolation ON evaluation_reviews
    USING (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid)
    WITH CHECK (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid);

CREATE POLICY org_isolation ON evaluation_evidence_links
    USING (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid)
    WITH CHECK (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid);

CREATE POLICY org_isolation ON evaluation_code_counters
    USING (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid)
    WITH CHECK (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid);

CREATE TRIGGER trg_evaluation_cycles_org_immutable    BEFORE UPDATE ON evaluation_cycles         FOR EACH ROW EXECUTE FUNCTION enforce_org_id_immutable();
CREATE TRIGGER trg_evaluation_subjects_org_immutable  BEFORE UPDATE ON evaluation_subjects       FOR EACH ROW EXECUTE FUNCTION enforce_org_id_immutable();
CREATE TRIGGER trg_evaluation_reviews_org_immutable   BEFORE UPDATE ON evaluation_reviews        FOR EACH ROW EXECUTE FUNCTION enforce_org_id_immutable();

-- No DELETE on lifecycle objects (archive-not-delete); goals and evidence
-- links are replace-set editable while their parent is still mutable.
GRANT SELECT, INSERT, UPDATE         ON evaluation_cycles         TO console_rt;
GRANT SELECT, INSERT, UPDATE         ON evaluation_subjects       TO console_rt;
GRANT SELECT, INSERT, UPDATE, DELETE ON evaluation_goals          TO console_rt;
GRANT SELECT, INSERT, UPDATE         ON evaluation_reviews        TO console_rt;
GRANT SELECT, INSERT, UPDATE, DELETE ON evaluation_evidence_links TO console_rt;
GRANT SELECT, INSERT, UPDATE         ON evaluation_code_counters  TO console_rt;
