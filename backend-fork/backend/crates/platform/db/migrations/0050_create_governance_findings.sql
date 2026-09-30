-- Slice B (anti-embezzlement, task #34): governance_findings table.
--
-- This table is the foundation of the integrity engine. Detectors write
-- findings here; EXECUTIVE / SUPER_ADMIN users read and triage them.
-- Findings are framed as "검토 필요" (review needed), NOT as "fraud".
--
-- Schema design:
--   * detector_id — dot-namespaced code e.g. "integrity.price_outlier",
--     "integrity.self_approval". CHECK mirrors the audit_events.action regex.
--   * entity_type / entity_id — the domain entity the finding is about.
--   * source_audit_event_id — nullable FK to audit_events; links the finding to
--     the write that triggered it (OnWrite detectors set this; batch detectors
--     may leave it NULL).
--   * subject_user_id — nullable FK to users; the person whose action was
--     flagged. NULL for non-person findings.
--   * score — a non-negative numeric severity score from the detector (e.g.
--     z-score, MAD ratio). Stored for audit; the severity enum is the primary
--     triage signal.
--   * severity — INFO / LOW / MEDIUM / HIGH / CRITICAL.
--   * status — lifecycle: OPEN → REVIEWED / DISMISSED / ESCALATED.
--   * evidence — JSONB bag: raw detector output (stats, percentiles, peers).
--   * review fields — who triaged, when, and a bounded memo.
--   * UNIQUE (org_id, detector_id, entity_type, entity_id) — idempotent upsert:
--     a detector re-running on the same entity updates the existing finding
--     rather than creating duplicates. Status reset to OPEN on re-open is
--     handled at the application layer.
--
-- RLS: org_isolation policy + FORCE RLS + enforce_org_id_immutable trigger,
-- identical to the pattern established in migrations 0030/0031/0035.
--
-- console-gate: audited-table governance_findings
CREATE TABLE governance_findings (
    id                  UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id              UUID        NOT NULL
                            REFERENCES organizations(id) ON DELETE RESTRICT,

    -- Detector identity (dot-namespaced, e.g. "integrity.price_outlier").
    -- CHECK mirrors the audit_events.action regex for consistency.
    detector_id         TEXT        NOT NULL
                            CHECK (detector_id ~ '^[a-z0-9_]+(\.[a-z0-9_]+)+$'),

    -- The domain entity this finding concerns.
    entity_type         TEXT        NOT NULL
                            CHECK (btrim(entity_type) <> '')
                            CHECK (char_length(entity_type) <= 100),
    entity_id           TEXT        NOT NULL
                            CHECK (btrim(entity_id) <> '')
                            CHECK (char_length(entity_id) <= 100),

    -- Link to the write event that triggered this finding (OnWrite detectors).
    source_audit_event_id UUID      REFERENCES audit_events(id) ON DELETE RESTRICT,

    -- The user whose action was flagged (NULL for non-person findings).
    subject_user_id     UUID        REFERENCES users(id) ON DELETE RESTRICT,

    -- Detector output.
    score               DOUBLE PRECISION NOT NULL
                            CHECK (score >= 0.0),
    severity            TEXT        NOT NULL
                            CHECK (severity IN ('INFO','LOW','MEDIUM','HIGH','CRITICAL')),
    evidence            JSONB       NOT NULL DEFAULT '{}',

    -- Lifecycle.
    status              TEXT        NOT NULL DEFAULT 'OPEN'
                            CHECK (status IN ('OPEN','REVIEWED','DISMISSED','ESCALATED')),
    detected_at         TIMESTAMPTZ NOT NULL,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT now(),

    -- Triage (review) fields.
    reviewed_by         UUID        REFERENCES users(id) ON DELETE RESTRICT,
    reviewed_at         TIMESTAMPTZ,
    review_memo         TEXT
                            CHECK (review_memo IS NULL OR btrim(review_memo) <> '')
                            CHECK (review_memo IS NULL OR char_length(review_memo) <= 2000),

    -- Consistency: if reviewed_by is set, reviewed_at must be set and vice versa.
    CHECK (
        (reviewed_by IS NULL AND reviewed_at IS NULL)
        OR
        (reviewed_by IS NOT NULL AND reviewed_at IS NOT NULL)
    )
);

-- Idempotent upsert key: one open finding per (org, detector, entity).
CREATE UNIQUE INDEX idx_governance_findings_idempotent
    ON governance_findings (org_id, detector_id, entity_type, entity_id);

-- Hot read path: list open findings by org, severity descending.
CREATE INDEX idx_governance_findings_org_status
    ON governance_findings (org_id, status, severity, detected_at DESC);

-- Link from subject user to their findings (for the "must not read own finding" check).
CREATE INDEX idx_governance_findings_subject
    ON governance_findings (org_id, subject_user_id)
    WHERE subject_user_id IS NOT NULL;

-- ---------------------------------------------------------------------------
-- RLS: org_isolation (mirrors 0030/0035 pattern exactly).
-- ---------------------------------------------------------------------------
ALTER TABLE governance_findings ENABLE ROW LEVEL SECURITY;
ALTER TABLE governance_findings FORCE ROW LEVEL SECURITY;

CREATE POLICY org_isolation ON governance_findings
    USING (
        org_id = NULLIF(current_setting('app.current_org', true), '')::uuid
    )
    WITH CHECK (
        org_id = NULLIF(current_setting('app.current_org', true), '')::uuid
    );

GRANT SELECT, INSERT, UPDATE, DELETE ON governance_findings TO console_rt;

-- ---------------------------------------------------------------------------
-- org_id immutability trigger (mirrors 0031/0035 pattern).
-- ---------------------------------------------------------------------------
CREATE TRIGGER trg_governance_findings_org_immutable
    BEFORE UPDATE ON governance_findings
    FOR EACH ROW EXECUTE FUNCTION enforce_org_id_immutable();
