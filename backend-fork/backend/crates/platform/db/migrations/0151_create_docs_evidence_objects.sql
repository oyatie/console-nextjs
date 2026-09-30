-- BE-DOCS Evidence EV object persistence: WORM copies, TSA proofs, custody,
-- legal holds, signed exports, FORCE RLS, and runtime-role grants.

INSERT INTO feature_catalog (feature_key) VALUES
    ('evidence_read'),
    ('evidence_register'),
    ('evidence_manage_custody'),
    ('evidence_manage_legal_hold'),
    ('evidence_export'),
    ('evidence_admin_review')
ON CONFLICT (feature_key) DO NOTHING;

CREATE TABLE docs_evidence_code_counters (
    org_id        UUID        NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    object_prefix TEXT        NOT NULL CHECK (object_prefix = 'EV'),
    next_value    BIGINT      NOT NULL DEFAULT 1 CHECK (next_value >= 1),
    updated_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (org_id, object_prefix)
);

-- console-gate: audited-table docs_evidence_objects
CREATE TABLE docs_evidence_objects (
    id                       UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id                   UUID        NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    code                     TEXT        NOT NULL CHECK (code ~ '^EV-[A-Z0-9-]{3,40}$'),
    title                    TEXT        NOT NULL CHECK (char_length(btrim(title)) BETWEEN 1 AND 200),
    description              TEXT        NULL CHECK (description IS NULL OR char_length(description) <= 4000),
    source_type              TEXT        NOT NULL CHECK (source_type IN ('record_archive','inbox_doc','mail_attachment','ingest_job','work_order_evidence_media','external_document')),
    source_id                TEXT        NOT NULL CHECK (char_length(btrim(source_id)) BETWEEN 1 AND 200),
    source_code              TEXT        NULL CHECK (source_code IS NULL OR char_length(btrim(source_code)) BETWEEN 1 AND 120),
    classification           TEXT        NOT NULL CHECK (classification IN ('GENERAL','INTERNAL','SENSITIVE','CONFIDENTIAL','SECRET')),
    record_owner_user_id     UUID        NULL,
    current_custody_stage    TEXT        NOT NULL DEFAULT 'REGISTERED' CHECK (current_custody_stage IN (
        'REGISTERED','HASH_RECORDED','TSA_SUBMITTED','TSA_VERIFIED','WORM_REPLICATED','CUSTODY_TRANSFERRED',
        'UNDER_REVIEW','ADMISSIBILITY_EVALUATED','LEGAL_HOLD_APPLIED','LEGAL_HOLD_RELEASED','EXPORTED','ARCHIVED',
        'DISPOSAL_REQUESTED','DISPOSED'
    )),
    legal_hold_state         TEXT        NOT NULL DEFAULT 'CLEAR' CHECK (legal_hold_state IN ('CLEAR','ACTIVE')),
    admissibility_status     TEXT        NOT NULL DEFAULT 'REVIEW_NEEDED' CHECK (admissibility_status IN ('ADMISSIBLE','REVIEW_NEEDED','BLOCKED','INADMISSIBLE')),
    admissibility_reasons    JSONB       NOT NULL DEFAULT '[]'::jsonb CHECK (jsonb_typeof(admissibility_reasons) = 'array'),
    admissibility_inputs     JSONB       NOT NULL DEFAULT '{}'::jsonb CHECK (jsonb_typeof(admissibility_inputs) = 'object'),
    created_by               UUID        NOT NULL,
    updated_by               UUID        NOT NULL,
    created_at               TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at               TIMESTAMPTZ NOT NULL DEFAULT now(),
    disposed_at              TIMESTAMPTZ NULL,
    disposal_reason          TEXT        NULL CHECK (disposal_reason IS NULL OR char_length(btrim(disposal_reason)) BETWEEN 1 AND 2000),
    disposed_by              UUID        NULL,
    UNIQUE (id, org_id),
    UNIQUE (org_id, code),
    FOREIGN KEY (record_owner_user_id, org_id) REFERENCES users(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (created_by, org_id) REFERENCES users(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (updated_by, org_id) REFERENCES users(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (disposed_by, org_id) REFERENCES users(id, org_id) ON DELETE RESTRICT,
    CHECK ((current_custody_stage = 'DISPOSED') = (disposed_at IS NOT NULL))
);
CREATE INDEX idx_docs_evidence_objects_org_source
    ON docs_evidence_objects (org_id, source_type, source_id, updated_at DESC);
CREATE INDEX idx_docs_evidence_objects_org_admissibility
    ON docs_evidence_objects (org_id, admissibility_status, updated_at DESC);
CREATE INDEX idx_docs_evidence_objects_org_legal_hold
    ON docs_evidence_objects (org_id, legal_hold_state, updated_at DESC);
CREATE INDEX idx_docs_evidence_objects_org_classification
    ON docs_evidence_objects (org_id, classification, updated_at DESC);

-- console-gate: audited-table docs_evidence_copies
CREATE TABLE docs_evidence_copies (
    id                       UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id                   UUID        NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    evidence_object_id        UUID        NOT NULL,
    copy_kind                TEXT        NOT NULL CHECK (copy_kind IN ('ORIGINAL','DERIVATIVE')),
    derivative_kind          TEXT        NULL CHECK (derivative_kind IS NULL OR derivative_kind IN ('REDACTED','THUMBNAIL','TRANSCODED','EXCERPT','EXPORT_MANIFEST','NORMALIZED_TEXT','OTHER')),
    parent_copy_id           UUID        NULL,
    storage_provider         TEXT        NOT NULL CHECK (char_length(btrim(storage_provider)) BETWEEN 1 AND 80),
    storage_object_id        TEXT        NOT NULL CHECK (char_length(btrim(storage_object_id)) BETWEEN 1 AND 300),
    storage_key_ref          TEXT        NULL CHECK (storage_key_ref IS NULL OR char_length(btrim(storage_key_ref)) BETWEEN 1 AND 500),
    storage_version_id       TEXT        NULL CHECK (storage_version_id IS NULL OR char_length(btrim(storage_version_id)) BETWEEN 1 AND 200),
    source_evidence_media_id UUID        NULL,
    digest_sha256            TEXT        NOT NULL CHECK (digest_sha256 ~ '^[a-f0-9]{64}$'),
    content_type             TEXT        NOT NULL CHECK (char_length(btrim(content_type)) BETWEEN 1 AND 160),
    size_bytes               BIGINT      NOT NULL CHECK (size_bytes >= 0),
    worm_status              TEXT        NOT NULL DEFAULT 'PENDING' CHECK (worm_status IN ('PENDING','VERIFIED','FAILED')),
    created_by               UUID        NOT NULL,
    created_at               TIMESTAMPTZ NOT NULL DEFAULT now(),
    verified_at              TIMESTAMPTZ NULL,
    UNIQUE (id, org_id),
    UNIQUE (org_id, evidence_object_id, digest_sha256, copy_kind),
    CHECK ((copy_kind = 'ORIGINAL' AND parent_copy_id IS NULL AND derivative_kind IS NULL)
        OR (copy_kind = 'DERIVATIVE' AND parent_copy_id IS NOT NULL AND derivative_kind IS NOT NULL)),
    CHECK ((worm_status = 'VERIFIED') = (verified_at IS NOT NULL)),
    FOREIGN KEY (evidence_object_id, org_id) REFERENCES docs_evidence_objects(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (parent_copy_id, org_id) REFERENCES docs_evidence_copies(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (source_evidence_media_id, org_id) REFERENCES evidence_media(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (created_by, org_id) REFERENCES users(id, org_id) ON DELETE RESTRICT
);
CREATE UNIQUE INDEX docs_evidence_copies_one_original_per_object
    ON docs_evidence_copies (org_id, evidence_object_id)
    WHERE copy_kind = 'ORIGINAL' AND parent_copy_id IS NULL;
CREATE INDEX idx_docs_evidence_copies_object_created
    ON docs_evidence_copies (org_id, evidence_object_id, created_at DESC);
CREATE INDEX idx_docs_evidence_copies_worm_status
    ON docs_evidence_copies (org_id, worm_status, created_at)
    WHERE worm_status IN ('PENDING','FAILED');

-- console-gate: audited-table docs_evidence_tsa_proofs
CREATE TABLE docs_evidence_tsa_proofs (
    id                              UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id                          UUID        NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    evidence_object_id               UUID        NOT NULL,
    copy_id                         UUID        NOT NULL,
    status                          TEXT        NOT NULL CHECK (status IN ('MISSING','PENDING','VERIFIED','FAILED','REVOKED','EXPIRED_CA')),
    provider                        TEXT        NOT NULL CHECK (char_length(btrim(provider)) BETWEEN 1 AND 120),
    policy_oid                      TEXT        NULL CHECK (policy_oid IS NULL OR char_length(btrim(policy_oid)) BETWEEN 1 AND 120),
    serial_number                   TEXT        NULL CHECK (serial_number IS NULL OR char_length(btrim(serial_number)) BETWEEN 1 AND 200),
    hash_algorithm                  TEXT        NOT NULL CHECK (hash_algorithm = 'SHA-256'),
    message_imprint_sha256          TEXT        NULL CHECK (message_imprint_sha256 IS NULL OR message_imprint_sha256 ~ '^[a-f0-9]{64}$'),
    generated_at                    TIMESTAMPTZ NULL,
    accuracy_millis                 BIGINT      NULL CHECK (accuracy_millis IS NULL OR accuracy_millis >= 0),
    ordering                        BOOLEAN     NULL,
    tsa_cert_fingerprint_sha256     TEXT        NULL CHECK (tsa_cert_fingerprint_sha256 IS NULL OR tsa_cert_fingerprint_sha256 ~ '^[a-f0-9]{64}$'),
    token_digest_sha256             TEXT        NULL CHECK (token_digest_sha256 IS NULL OR token_digest_sha256 ~ '^[a-f0-9]{64}$'),
    token_storage_provider          TEXT        NULL CHECK (token_storage_provider IS NULL OR char_length(btrim(token_storage_provider)) BETWEEN 1 AND 80),
    token_storage_object_id         TEXT        NULL CHECK (token_storage_object_id IS NULL OR char_length(btrim(token_storage_object_id)) BETWEEN 1 AND 300),
    token_storage_key_ref           TEXT        NULL CHECK (token_storage_key_ref IS NULL OR char_length(btrim(token_storage_key_ref)) BETWEEN 1 AND 500),
    token_storage_version_id        TEXT        NULL CHECK (token_storage_version_id IS NULL OR char_length(btrim(token_storage_version_id)) BETWEEN 1 AND 200),
    verified_at                     TIMESTAMPTZ NULL,
    failure_reason                  TEXT        NULL CHECK (failure_reason IS NULL OR char_length(failure_reason) <= 2000),
    created_by                      UUID        NOT NULL,
    created_at                      TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (id, org_id),
    CHECK (status <> 'VERIFIED' OR (message_imprint_sha256 IS NOT NULL AND verified_at IS NOT NULL)),
    CHECK ((token_storage_provider IS NULL) = (token_storage_object_id IS NULL)),
    FOREIGN KEY (evidence_object_id, org_id) REFERENCES docs_evidence_objects(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (copy_id, org_id) REFERENCES docs_evidence_copies(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (created_by, org_id) REFERENCES users(id, org_id) ON DELETE RESTRICT
);
CREATE INDEX idx_docs_evidence_tsa_proofs_copy_created
    ON docs_evidence_tsa_proofs (org_id, copy_id, created_at DESC);
CREATE INDEX idx_docs_evidence_tsa_proofs_status
    ON docs_evidence_tsa_proofs (org_id, status, created_at DESC);

-- console-gate: audited-table docs_evidence_custody_events
CREATE TABLE docs_evidence_custody_events (
    id                       UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id                   UUID        NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    evidence_object_id        UUID        NOT NULL,
    stage                    TEXT        NOT NULL CHECK (stage IN (
        'REGISTERED','HASH_RECORDED','TSA_SUBMITTED','TSA_VERIFIED','WORM_REPLICATED','CUSTODY_TRANSFERRED',
        'UNDER_REVIEW','ADMISSIBILITY_EVALUATED','LEGAL_HOLD_APPLIED','LEGAL_HOLD_RELEASED','EXPORTED','ARCHIVED',
        'DISPOSAL_REQUESTED','DISPOSED'
    )),
    actor_user_id            UUID        NOT NULL,
    from_custodian           JSONB       NULL CHECK (from_custodian IS NULL OR jsonb_typeof(from_custodian) = 'object'),
    to_custodian             JSONB       NULL CHECK (to_custodian IS NULL OR jsonb_typeof(to_custodian) = 'object'),
    location_label           TEXT        NULL CHECK (location_label IS NULL OR char_length(btrim(location_label)) BETWEEN 1 AND 200),
    reason                   TEXT        NOT NULL CHECK (char_length(btrim(reason)) BETWEEN 1 AND 2000),
    source_ref               JSONB       NULL CHECK (source_ref IS NULL OR jsonb_typeof(source_ref) = 'object'),
    audit_event_id           UUID        NULL,
    previous_event_id        UUID        NULL,
    event_digest_sha256      TEXT        NOT NULL CHECK (event_digest_sha256 ~ '^[a-f0-9]{64}$'),
    occurred_at              TIMESTAMPTZ NOT NULL,
    created_at               TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (id, org_id),
    FOREIGN KEY (evidence_object_id, org_id) REFERENCES docs_evidence_objects(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (actor_user_id, org_id) REFERENCES users(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (org_id, audit_event_id) REFERENCES audit_events(org_id, id) ON DELETE RESTRICT,
    FOREIGN KEY (previous_event_id, org_id) REFERENCES docs_evidence_custody_events(id, org_id) ON DELETE RESTRICT
);
CREATE INDEX idx_docs_evidence_custody_events_object_occurred
    ON docs_evidence_custody_events (org_id, evidence_object_id, occurred_at DESC, id DESC);

-- console-gate: audited-table docs_evidence_legal_holds
CREATE TABLE docs_evidence_legal_holds (
    id                 UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id             UUID        NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    evidence_object_id  UUID        NOT NULL,
    status             TEXT        NOT NULL DEFAULT 'ACTIVE' CHECK (status IN ('ACTIVE','RELEASED')),
    case_ref           TEXT        NOT NULL CHECK (char_length(btrim(case_ref)) BETWEEN 1 AND 200),
    basis              TEXT        NOT NULL CHECK (char_length(btrim(basis)) BETWEEN 1 AND 2000),
    reason             TEXT        NOT NULL CHECK (char_length(btrim(reason)) BETWEEN 1 AND 2000),
    applied_by         UUID        NOT NULL,
    applied_at         TIMESTAMPTZ NOT NULL,
    released_by        UUID        NULL,
    released_at        TIMESTAMPTZ NULL,
    release_reason     TEXT        NULL CHECK (release_reason IS NULL OR char_length(btrim(release_reason)) BETWEEN 1 AND 2000),
    audit_event_id     UUID        NULL,
    UNIQUE (id, org_id),
    CHECK ((status = 'ACTIVE' AND released_by IS NULL AND released_at IS NULL AND release_reason IS NULL)
        OR (status = 'RELEASED' AND released_by IS NOT NULL AND released_at IS NOT NULL AND release_reason IS NOT NULL)),
    FOREIGN KEY (evidence_object_id, org_id) REFERENCES docs_evidence_objects(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (applied_by, org_id) REFERENCES users(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (released_by, org_id) REFERENCES users(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (org_id, audit_event_id) REFERENCES audit_events(org_id, id) ON DELETE RESTRICT
);
CREATE UNIQUE INDEX docs_evidence_legal_holds_active_case_key
    ON docs_evidence_legal_holds (org_id, evidence_object_id, case_ref)
    WHERE status = 'ACTIVE';
CREATE INDEX idx_docs_evidence_legal_holds_object_status
    ON docs_evidence_legal_holds (org_id, evidence_object_id, status, applied_at DESC);

-- console-gate: audited-table docs_evidence_exports
CREATE TABLE docs_evidence_exports (
    id                     UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id                 UUID        NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    evidence_object_id      UUID        NOT NULL,
    manifest_digest_sha256 TEXT        NOT NULL CHECK (manifest_digest_sha256 ~ '^[a-f0-9]{64}$'),
    signature_algorithm    TEXT        NOT NULL CHECK (char_length(btrim(signature_algorithm)) BETWEEN 1 AND 80),
    signature_ref          TEXT        NULL CHECK (signature_ref IS NULL OR char_length(btrim(signature_ref)) BETWEEN 1 AND 500),
    export_reason          TEXT        NOT NULL CHECK (char_length(btrim(export_reason)) BETWEEN 1 AND 2000),
    exported_by            UUID        NOT NULL,
    exported_at            TIMESTAMPTZ NOT NULL,
    audit_event_id         UUID        NULL,
    custody_event_id       UUID        NULL,
    UNIQUE (id, org_id),
    UNIQUE (org_id, evidence_object_id, manifest_digest_sha256),
    FOREIGN KEY (evidence_object_id, org_id) REFERENCES docs_evidence_objects(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (exported_by, org_id) REFERENCES users(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (org_id, audit_event_id) REFERENCES audit_events(org_id, id) ON DELETE RESTRICT,
    FOREIGN KEY (custody_event_id, org_id) REFERENCES docs_evidence_custody_events(id, org_id) ON DELETE RESTRICT
);
CREATE INDEX idx_docs_evidence_exports_object_exported
    ON docs_evidence_exports (org_id, evidence_object_id, exported_at DESC);

CREATE OR REPLACE FUNCTION docs_evidence_object_immutable_fields()
RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.code IS DISTINCT FROM OLD.code
        OR NEW.source_type IS DISTINCT FROM OLD.source_type
        OR NEW.source_id IS DISTINCT FROM OLD.source_id
        OR NEW.source_code IS DISTINCT FROM OLD.source_code
        OR NEW.created_by IS DISTINCT FROM OLD.created_by
        OR NEW.created_at IS DISTINCT FROM OLD.created_at THEN
        RAISE EXCEPTION 'immutable EV object fields cannot be changed for id=%', OLD.id;
    END IF;
    IF OLD.legal_hold_state = 'ACTIVE' AND NEW.current_custody_stage = 'DISPOSED' THEN
        RAISE EXCEPTION 'active legal hold blocks EV object disposal for id=%', OLD.id;
    END IF;
    RETURN NEW;
END;
$$;

CREATE OR REPLACE FUNCTION docs_evidence_copy_worm_guard()
RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP = 'DELETE' THEN
        RAISE EXCEPTION 'EV copies are WORM append-only and cannot be deleted id=%', OLD.id;
    END IF;
    IF NEW.copy_kind IS DISTINCT FROM OLD.copy_kind
        OR NEW.derivative_kind IS DISTINCT FROM OLD.derivative_kind
        OR NEW.parent_copy_id IS DISTINCT FROM OLD.parent_copy_id
        OR NEW.storage_provider IS DISTINCT FROM OLD.storage_provider
        OR NEW.storage_object_id IS DISTINCT FROM OLD.storage_object_id
        OR NEW.storage_key_ref IS DISTINCT FROM OLD.storage_key_ref
        OR NEW.storage_version_id IS DISTINCT FROM OLD.storage_version_id
        OR NEW.source_evidence_media_id IS DISTINCT FROM OLD.source_evidence_media_id
        OR NEW.digest_sha256 IS DISTINCT FROM OLD.digest_sha256
        OR NEW.content_type IS DISTINCT FROM OLD.content_type
        OR NEW.size_bytes IS DISTINCT FROM OLD.size_bytes
        OR NEW.created_by IS DISTINCT FROM OLD.created_by
        OR NEW.created_at IS DISTINCT FROM OLD.created_at THEN
        RAISE EXCEPTION 'immutable EV copy fields cannot be changed for id=%', OLD.id;
    END IF;
    IF OLD.worm_status = 'VERIFIED' AND (
        NEW.worm_status IS DISTINCT FROM OLD.worm_status OR NEW.verified_at IS DISTINCT FROM OLD.verified_at
    ) THEN
        RAISE EXCEPTION 'verified EV WORM copy cannot be mutated id=%', OLD.id;
    END IF;
    RETURN NEW;
END;
$$;

CREATE OR REPLACE FUNCTION docs_evidence_append_only_guard()
RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION 'append-only EV table % forbids %', TG_TABLE_NAME, TG_OP;
END;
$$;

CREATE OR REPLACE FUNCTION docs_evidence_legal_hold_release_guard()
RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP = 'DELETE' THEN
        RAISE EXCEPTION 'EV legal holds cannot be deleted id=%', OLD.id;
    END IF;
    IF NEW.evidence_object_id IS DISTINCT FROM OLD.evidence_object_id
        OR NEW.case_ref IS DISTINCT FROM OLD.case_ref
        OR NEW.basis IS DISTINCT FROM OLD.basis
        OR NEW.reason IS DISTINCT FROM OLD.reason
        OR NEW.applied_by IS DISTINCT FROM OLD.applied_by
        OR NEW.applied_at IS DISTINCT FROM OLD.applied_at
        OR NEW.audit_event_id IS DISTINCT FROM OLD.audit_event_id THEN
        RAISE EXCEPTION 'immutable EV legal hold fields cannot be changed id=%', OLD.id;
    END IF;
    IF OLD.status = 'RELEASED' THEN
        RAISE EXCEPTION 'released EV legal hold cannot be mutated id=%', OLD.id;
    END IF;
    IF NEW.status <> 'RELEASED' THEN
        RAISE EXCEPTION 'EV legal hold updates may only release an active hold id=%', OLD.id;
    END IF;
    RETURN NEW;
END;
$$;

DO $$
DECLARE
    t TEXT;
    tenant_tables TEXT[] := ARRAY[
        'docs_evidence_code_counters',
        'docs_evidence_objects',
        'docs_evidence_copies',
        'docs_evidence_tsa_proofs',
        'docs_evidence_custody_events',
        'docs_evidence_legal_holds',
        'docs_evidence_exports'
    ];
BEGIN
    FOREACH t IN ARRAY tenant_tables LOOP
        EXECUTE format('ALTER TABLE %I ENABLE ROW LEVEL SECURITY', t);
        EXECUTE format('ALTER TABLE %I FORCE ROW LEVEL SECURITY', t);
        EXECUTE format(
            'CREATE POLICY org_isolation ON %I '
            || 'USING (org_id = NULLIF(current_setting(''app.current_org'', true), '''')::uuid) '
            || 'WITH CHECK (org_id = NULLIF(current_setting(''app.current_org'', true), '''')::uuid)',
            t
        );
        EXECUTE format(
            'CREATE TRIGGER trg_%s_org_immutable BEFORE UPDATE ON %I '
            || 'FOR EACH ROW EXECUTE FUNCTION enforce_org_id_immutable()', t, t);
    END LOOP;
END
$$;

CREATE TRIGGER trg_docs_evidence_objects_immutable
    BEFORE UPDATE ON docs_evidence_objects
    FOR EACH ROW EXECUTE FUNCTION docs_evidence_object_immutable_fields();
CREATE TRIGGER trg_docs_evidence_copies_worm_update
    BEFORE UPDATE ON docs_evidence_copies
    FOR EACH ROW EXECUTE FUNCTION docs_evidence_copy_worm_guard();
CREATE TRIGGER trg_docs_evidence_copies_worm_delete
    BEFORE DELETE ON docs_evidence_copies
    FOR EACH ROW EXECUTE FUNCTION docs_evidence_copy_worm_guard();
CREATE TRIGGER trg_docs_evidence_tsa_proofs_no_update
    BEFORE UPDATE ON docs_evidence_tsa_proofs
    FOR EACH ROW EXECUTE FUNCTION docs_evidence_append_only_guard();
CREATE TRIGGER trg_docs_evidence_tsa_proofs_no_delete
    BEFORE DELETE ON docs_evidence_tsa_proofs
    FOR EACH ROW EXECUTE FUNCTION docs_evidence_append_only_guard();
CREATE TRIGGER trg_docs_evidence_custody_events_no_update
    BEFORE UPDATE ON docs_evidence_custody_events
    FOR EACH ROW EXECUTE FUNCTION docs_evidence_append_only_guard();
CREATE TRIGGER trg_docs_evidence_custody_events_no_delete
    BEFORE DELETE ON docs_evidence_custody_events
    FOR EACH ROW EXECUTE FUNCTION docs_evidence_append_only_guard();
CREATE TRIGGER trg_docs_evidence_legal_holds_release_only
    BEFORE UPDATE ON docs_evidence_legal_holds
    FOR EACH ROW EXECUTE FUNCTION docs_evidence_legal_hold_release_guard();
CREATE TRIGGER trg_docs_evidence_legal_holds_no_delete
    BEFORE DELETE ON docs_evidence_legal_holds
    FOR EACH ROW EXECUTE FUNCTION docs_evidence_legal_hold_release_guard();
CREATE TRIGGER trg_docs_evidence_exports_no_update
    BEFORE UPDATE ON docs_evidence_exports
    FOR EACH ROW EXECUTE FUNCTION docs_evidence_append_only_guard();
CREATE TRIGGER trg_docs_evidence_exports_no_delete
    BEFORE DELETE ON docs_evidence_exports
    FOR EACH ROW EXECUTE FUNCTION docs_evidence_append_only_guard();

GRANT SELECT, INSERT, UPDATE ON docs_evidence_code_counters TO console_rt;
GRANT SELECT, INSERT, UPDATE ON docs_evidence_objects TO console_rt;
GRANT SELECT, INSERT, UPDATE ON docs_evidence_copies TO console_rt;
GRANT SELECT, INSERT ON docs_evidence_tsa_proofs TO console_rt;
GRANT SELECT, INSERT ON docs_evidence_custody_events TO console_rt;
GRANT SELECT, INSERT, UPDATE ON docs_evidence_legal_holds TO console_rt;
GRANT SELECT, INSERT ON docs_evidence_exports TO console_rt;

REVOKE DELETE ON docs_evidence_code_counters FROM console_rt;
REVOKE DELETE ON docs_evidence_objects FROM console_rt;
REVOKE DELETE ON docs_evidence_copies FROM console_rt;
REVOKE UPDATE, DELETE ON docs_evidence_tsa_proofs FROM console_rt;
REVOKE UPDATE, DELETE ON docs_evidence_custody_events FROM console_rt;
REVOKE DELETE ON docs_evidence_legal_holds FROM console_rt;
REVOKE UPDATE, DELETE ON docs_evidence_exports FROM console_rt;
