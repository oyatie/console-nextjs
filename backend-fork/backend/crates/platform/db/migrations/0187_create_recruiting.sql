-- Recruiting pipeline: postings → applicants → offers; hire links into the
-- HR-owned employees table (composite same-org FK). Archive-not-delete: no
-- DELETE grant anywhere; rejected applicants stay as the talent pool.
-- NOTE: provisionally numbered 0187 — renumber to the next free number
-- immediately before push (migration numbers collide across lanes).

-- Register the capabilities before any tenant policy can grant them. Routes
-- stay fail-closed: catalog presence is only a prerequisite for an explicit
-- ACTIVE custom-role assignment, never an implicit permission.
INSERT INTO feature_catalog (feature_key) VALUES
    ('recruiting_read'),
    ('recruiting_manage')
ON CONFLICT (feature_key) DO NOTHING;

CREATE TABLE recruit_postings (
    id              UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id          UUID        NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    posting_no      TEXT        NOT NULL CHECK (posting_no ~ '^JP-[0-9]{4,}$'),
    role_title      TEXT        NOT NULL CHECK (btrim(role_title) <> ''),
    company         TEXT        NOT NULL CHECK (btrim(company) <> ''),
    worksite        TEXT        NOT NULL CHECK (btrim(worksite) <> ''),
    employment_type TEXT        NOT NULL CHECK (employment_type IN ('REGULAR','RESIDENT_SHIFT','PART_TIME','POOL_DAILY')),
    scope           TEXT        NOT NULL CHECK (scope IN ('INTERNAL','EXTERNAL')),
    headcount       INTEGER     NOT NULL CHECK (headcount >= 1),
    hired_count     INTEGER     NOT NULL DEFAULT 0 CHECK (hired_count >= 0 AND hired_count <= headcount),
    deadline        DATE,                          -- NULL = open-ended (상시)
    requirements    JSONB       NOT NULL DEFAULT '[]'::jsonb CHECK (jsonb_typeof(requirements) = 'array'),
    position_ref    TEXT,                          -- optional ontology position instance ref
    status          TEXT        NOT NULL DEFAULT 'DRAFT' CHECK (status IN ('DRAFT','PUBLISHED','CLOSED')),
    exposure_attested_by UUID,
    exposure_attested_at TIMESTAMPTZ,
    published_by    UUID,
    published_at    TIMESTAMPTZ,
    closed_at       TIMESTAMPTZ,
    created_by      UUID        NOT NULL,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (org_id, posting_no),
    UNIQUE (id, org_id),
    FOREIGN KEY (created_by, org_id) REFERENCES users(id, org_id) ON DELETE RESTRICT,
    CHECK (status <> 'PUBLISHED' OR (published_at IS NOT NULL AND exposure_attested_at IS NOT NULL))
);
CREATE INDEX recruit_postings_org_status_idx ON recruit_postings (org_id, status);

CREATE TABLE recruit_applicants (
    id              UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id          UUID        NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    posting_id      UUID        NOT NULL,
    applicant_no    TEXT        NOT NULL CHECK (applicant_no ~ '^APL-[0-9]{4,}$'),
    name            TEXT        NOT NULL CHECK (btrim(name) <> ''),
    profile         JSONB       NOT NULL DEFAULT '[]'::jsonb CHECK (jsonb_typeof(profile) = 'array'),
    source_document TEXT,                          -- provenance filename only
    stage           TEXT        NOT NULL DEFAULT 'APPLIED' CHECK (stage IN ('APPLIED','SCREENING','INTERVIEW','OFFER','HIRED')),
    hold            BOOLEAN     NOT NULL DEFAULT FALSE,
    doc_requested   BOOLEAN     NOT NULL DEFAULT FALSE,
    rejected_at     TIMESTAMPTZ,
    reject_reason   TEXT        CHECK (reject_reason IN ('CAREER_SHORTFALL','ROLE_MISMATCH','COMP_MISMATCH','ACCEPTED_ELSEWHERE','OTHER')),
    reject_note     TEXT,
    assessment_score TEXT       CHECK (assessment_score IN ('SUITABLE','NEUTRAL','UNSUITABLE')),
    assessed_by     UUID,
    assessed_at     TIMESTAMPTZ,
    hired_employee_id UUID,
    created_by      UUID        NOT NULL,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (org_id, applicant_no),
    UNIQUE (id, org_id),
    FOREIGN KEY (posting_id, org_id) REFERENCES recruit_postings(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (hired_employee_id, org_id) REFERENCES employees(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (created_by, org_id) REFERENCES users(id, org_id) ON DELETE RESTRICT,
    CHECK ((rejected_at IS NULL) = (reject_reason IS NULL)),
    CHECK (stage <> 'HIRED' OR hired_employee_id IS NOT NULL),
    CHECK ((assessment_score IS NULL) = (assessed_by IS NULL))
);
CREATE INDEX recruit_applicants_org_posting_idx ON recruit_applicants (org_id, posting_id);
CREATE INDEX recruit_applicants_org_rejected_idx ON recruit_applicants (org_id, rejected_at) WHERE rejected_at IS NOT NULL;
CREATE UNIQUE INDEX recruit_applicants_hired_employee_uq ON recruit_applicants (org_id, hired_employee_id) WHERE hired_employee_id IS NOT NULL;

CREATE TABLE recruit_offers (
    id              UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id          UUID        NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    applicant_id    UUID        NOT NULL,
    version         INTEGER     NOT NULL CHECK (version >= 1),
    amount          NUMERIC(14,2) NOT NULL CHECK (amount >= 0),
    amount_period   TEXT        NOT NULL CHECK (amount_period IN ('MONTHLY','DAILY')),
    currency        TEXT        NOT NULL DEFAULT 'KRW' CHECK (currency = 'KRW'),
    reply_deadline  DATE        NOT NULL,
    status          TEXT        NOT NULL DEFAULT 'EXTENDED' CHECK (status IN ('EXTENDED','SUPERSEDED','WITHDRAWN','ACCEPTED','DECLINED')),
    withdraw_reason TEXT,
    extended_by     UUID        NOT NULL,
    extended_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    resolved_at     TIMESTAMPTZ,
    UNIQUE (org_id, applicant_id, version),
    UNIQUE (id, org_id),
    FOREIGN KEY (applicant_id, org_id) REFERENCES recruit_applicants(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (extended_by, org_id) REFERENCES users(id, org_id) ON DELETE RESTRICT,
    CHECK (status <> 'WITHDRAWN' OR withdraw_reason IS NOT NULL)
);
-- One live offer per applicant; history rows are terminal-status.
CREATE UNIQUE INDEX recruit_offers_live_uq ON recruit_offers (org_id, applicant_id) WHERE status = 'EXTENDED';

-- Domain history layer for the applicant timeline (platform audit_events stay
-- the tamper-evident stream; this serves the UI history without audit-read
-- privileges). Append-only.
CREATE TABLE recruit_stage_events (
    id              UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id          UUID        NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    applicant_id    UUID        NOT NULL,
    action          TEXT        NOT NULL CHECK (action IN ('APPLY','ADVANCE','ASSESS','HOLD','UNHOLD','REQUEST_DOCUMENTS','OFFER_EXTEND','OFFER_ADJUST','OFFER_WITHDRAW','OFFER_REPLY','REJECT','REINSTATE','HIRE')),
    from_stage      TEXT,
    to_stage        TEXT,
    reason          TEXT,
    actor           UUID        NOT NULL,
    occurred_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    FOREIGN KEY (applicant_id, org_id) REFERENCES recruit_applicants(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (actor, org_id) REFERENCES users(id, org_id) ON DELETE RESTRICT
);
CREATE INDEX recruit_stage_events_org_applicant_idx ON recruit_stage_events (org_id, applicant_id, occurred_at);

-- Tenant concealment on every table (house pattern: org RLS + FORCE).
ALTER TABLE recruit_postings ENABLE ROW LEVEL SECURITY; ALTER TABLE recruit_postings FORCE ROW LEVEL SECURITY;
ALTER TABLE recruit_applicants ENABLE ROW LEVEL SECURITY; ALTER TABLE recruit_applicants FORCE ROW LEVEL SECURITY;
ALTER TABLE recruit_offers ENABLE ROW LEVEL SECURITY; ALTER TABLE recruit_offers FORCE ROW LEVEL SECURITY;
ALTER TABLE recruit_stage_events ENABLE ROW LEVEL SECURITY; ALTER TABLE recruit_stage_events FORCE ROW LEVEL SECURITY;
DO $$ DECLARE t TEXT; BEGIN FOREACH t IN ARRAY ARRAY['recruit_postings','recruit_applicants','recruit_offers','recruit_stage_events'] LOOP
 EXECUTE format('CREATE POLICY org_isolation ON %I USING (org_id = NULLIF(current_setting(''app.current_org'', true), '''')::uuid) WITH CHECK (org_id = NULLIF(current_setting(''app.current_org'', true), '''')::uuid)', t);
END LOOP; END $$;
-- No DELETE grant anywhere (archive-not-delete); the stage-event history is
-- append-only for the runtime role.
GRANT SELECT, INSERT, UPDATE ON recruit_postings TO console_rt;
GRANT SELECT, INSERT, UPDATE ON recruit_applicants TO console_rt;
GRANT SELECT, INSERT, UPDATE ON recruit_offers TO console_rt;
GRANT SELECT, INSERT ON recruit_stage_events TO console_rt;

CREATE TRIGGER trg_recruit_postings_org_immutable BEFORE UPDATE ON recruit_postings FOR EACH ROW EXECUTE FUNCTION enforce_org_id_immutable();
CREATE TRIGGER trg_recruit_applicants_org_immutable BEFORE UPDATE ON recruit_applicants FOR EACH ROW EXECUTE FUNCTION enforce_org_id_immutable();
CREATE TRIGGER trg_recruit_offers_org_immutable BEFORE UPDATE ON recruit_offers FOR EACH ROW EXECUTE FUNCTION enforce_org_id_immutable();

-- Terminal-state immutability: a hired applicant never leaves HIRED and a
-- resolved offer never changes status again.
CREATE OR REPLACE FUNCTION recruit_applicant_terminal_immutable() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN
 IF OLD.stage = 'HIRED' AND NEW.stage <> OLD.stage THEN RAISE EXCEPTION 'hired applicant stage is immutable'; END IF; RETURN NEW; END $$;
CREATE TRIGGER trg_recruit_applicants_terminal BEFORE UPDATE ON recruit_applicants FOR EACH ROW EXECUTE FUNCTION recruit_applicant_terminal_immutable();
CREATE OR REPLACE FUNCTION recruit_offer_terminal_immutable() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN
 IF OLD.status IN ('SUPERSEDED','WITHDRAWN','ACCEPTED','DECLINED') AND NEW.status <> OLD.status THEN RAISE EXCEPTION 'resolved offer status is immutable'; END IF; RETURN NEW; END $$;
CREATE TRIGGER trg_recruit_offers_terminal BEFORE UPDATE ON recruit_offers FOR EACH ROW EXECUTE FUNCTION recruit_offer_terminal_immutable();
CREATE OR REPLACE FUNCTION recruit_stage_events_append_only() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'recruit stage history is immutable'; END $$;
CREATE TRIGGER trg_recruit_stage_events_no_update BEFORE UPDATE OR DELETE ON recruit_stage_events FOR EACH ROW EXECUTE FUNCTION recruit_stage_events_append_only();
