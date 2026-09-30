-- Private admission custody. No serving role can read or write these records
-- until the proof, family and audit owners move into one restricted transaction.
-- Source identities are snapshots: later source retention must not erase an
-- accepted admission or registration intent.
CREATE TABLE auth_security.session_admissions (
    operation_id UUID PRIMARY KEY,
    status_secret_hash BYTEA NOT NULL UNIQUE CHECK (octet_length(status_secret_hash) = 32),
    account_id UUID NOT NULL,
    home_org_id UUID NOT NULL,
    auth_generation BIGINT NOT NULL CHECK (auth_generation > 0),
    session_purpose TEXT NOT NULL CHECK (session_purpose IN ('enrollment', 'normal')),
    status TEXT NOT NULL DEFAULT 'pending' CHECK (status IN ('pending', 'committed', 'rejected')),
    source_kind TEXT CHECK (source_kind IN ('bootstrap_otp', 'passkey', 'device_handoff')),
    source_row_id UUID,
    source_operation_id UUID,
    proof_ceremony_id UUID,
    family_id UUID UNIQUE,
    receipt_id UUID UNIQUE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    expires_at TIMESTAMPTZ NOT NULL,
    finalized_at TIMESTAMPTZ,
    CHECK (expires_at > created_at),
    CHECK (finalized_at IS NULL OR finalized_at >= created_at),
    CHECK (status <> 'committed' OR finalized_at < expires_at),
    CHECK (
        (status = 'pending' AND source_kind IS NULL AND source_row_id IS NULL
         AND source_operation_id IS NULL AND proof_ceremony_id IS NULL
         AND family_id IS NULL AND receipt_id IS NULL AND finalized_at IS NULL)
        OR (status = 'committed' AND source_kind IS NOT NULL AND source_row_id IS NOT NULL
            AND source_operation_id IS NOT NULL AND family_id IS NOT NULL
            AND receipt_id IS NOT NULL AND finalized_at IS NOT NULL
            AND ((session_purpose = 'enrollment' AND source_kind = 'bootstrap_otp')
              OR (session_purpose = 'normal'
                  AND source_kind IN ('passkey', 'device_handoff'))))
        OR (status = 'rejected' AND family_id IS NULL AND receipt_id IS NOT NULL
            AND finalized_at IS NOT NULL)
    )
);

CREATE TABLE auth_security.registration_intents (
    ceremony_id UUID PRIMARY KEY,
    account_id UUID NOT NULL,
    home_org_id UUID NOT NULL,
    auth_generation BIGINT NOT NULL CHECK (auth_generation > 0),
    family_id UUID NOT NULL,
    purpose TEXT NOT NULL CHECK (purpose IN ('first_enrollment', 'add_device')),
    source_bootstrap_id UUID,
    source_issuance_operation_id UUID,
    step_up_ceremony_id UUID,
    step_up_credential_id UUID,
    created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    expires_at TIMESTAMPTZ NOT NULL,
    consumed_at TIMESTAMPTZ,
    CHECK (expires_at > created_at),
    CHECK (consumed_at IS NULL OR consumed_at >= created_at),
    CHECK (consumed_at IS NULL OR consumed_at < expires_at),
    CHECK (
        (purpose = 'first_enrollment' AND source_bootstrap_id IS NOT NULL
         AND source_issuance_operation_id IS NOT NULL
         AND step_up_ceremony_id IS NULL AND step_up_credential_id IS NULL)
        OR (purpose = 'add_device' AND source_bootstrap_id IS NULL
            AND source_issuance_operation_id IS NULL
            AND step_up_ceremony_id IS NOT NULL AND step_up_credential_id IS NOT NULL)
    )
);

CREATE FUNCTION auth_security.guard_session_admission()
RETURNS TRIGGER
LANGUAGE plpgsql
SET search_path = pg_catalog
AS $$
BEGIN
    -- The database owner needs to restore accepted backup rows. Serving command
    -- roles may create only a pending operation before examining a proof.
    IF TG_OP = 'INSERT' THEN
        IF current_user <> 'console_app' AND NEW.status <> 'pending' THEN
            RAISE EXCEPTION 'session admission must begin pending'
                USING ERRCODE = '23514';
        END IF;
        RETURN NEW;
    END IF;
    IF ROW(OLD.operation_id, OLD.status_secret_hash, OLD.account_id,
           OLD.home_org_id, OLD.auth_generation, OLD.session_purpose,
           OLD.created_at, OLD.expires_at)
       IS DISTINCT FROM
       ROW(NEW.operation_id, NEW.status_secret_hash, NEW.account_id,
           NEW.home_org_id, NEW.auth_generation, NEW.session_purpose,
           NEW.created_at, NEW.expires_at)
       OR OLD.status <> 'pending'
       OR NEW.status = 'pending'
       OR NEW.finalized_at < OLD.created_at THEN
        RAISE EXCEPTION 'session admission cannot be repointed or reopened'
            USING ERRCODE = '23514';
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER session_admission_immutable
BEFORE INSERT OR UPDATE ON auth_security.session_admissions
FOR EACH ROW EXECUTE FUNCTION auth_security.guard_session_admission();

-- Intent identity and proof may never be repointed; completion is one-way.
CREATE FUNCTION auth_security.guard_registration_intent()
RETURNS TRIGGER
LANGUAGE plpgsql
SET search_path = pg_catalog
AS $$
BEGIN
    -- Restore may replay already consumed rows under the offline owner.
    IF TG_OP = 'INSERT' THEN
        IF current_user <> 'console_app' AND NEW.consumed_at IS NOT NULL THEN
            RAISE EXCEPTION 'registration intent must begin unconsumed'
                USING ERRCODE = '23514';
        END IF;
        RETURN NEW;
    END IF;
    IF ROW(OLD.ceremony_id, OLD.account_id, OLD.home_org_id, OLD.auth_generation,
           OLD.family_id, OLD.purpose, OLD.source_bootstrap_id,
           OLD.source_issuance_operation_id, OLD.step_up_ceremony_id,
           OLD.step_up_credential_id, OLD.created_at, OLD.expires_at)
       IS DISTINCT FROM
       ROW(NEW.ceremony_id, NEW.account_id, NEW.home_org_id, NEW.auth_generation,
           NEW.family_id, NEW.purpose, NEW.source_bootstrap_id,
           NEW.source_issuance_operation_id, NEW.step_up_ceremony_id,
           NEW.step_up_credential_id, NEW.created_at, NEW.expires_at)
       OR (OLD.consumed_at IS NOT NULL AND NEW.consumed_at IS DISTINCT FROM OLD.consumed_at)
       OR (OLD.consumed_at IS NULL AND NEW.consumed_at IS NOT NULL
           AND NEW.consumed_at < OLD.created_at) THEN
        RAISE EXCEPTION 'registration intent cannot be repointed or reopened'
            USING ERRCODE = '23514';
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER registration_intent_immutable
BEFORE INSERT OR UPDATE ON auth_security.registration_intents
FOR EACH ROW EXECUTE FUNCTION auth_security.guard_registration_intent();

REVOKE ALL ON TABLE auth_security.session_admissions,
    auth_security.registration_intents FROM PUBLIC, console_rt,
    console_leave_cmd, console_ontology_cmd, console_platform_force_cmd,
    console_leave_definer, console_ontology_writer;
REVOKE ALL ON FUNCTION auth_security.guard_registration_intent() FROM PUBLIC,
    console_rt, console_leave_cmd, console_ontology_cmd,
    console_platform_force_cmd, console_leave_definer, console_ontology_writer;
REVOKE ALL ON FUNCTION auth_security.guard_session_admission() FROM PUBLIC,
    console_rt, console_leave_cmd, console_ontology_cmd,
    console_platform_force_cmd, console_leave_definer, console_ontology_writer;

COMMENT ON TABLE auth_security.session_admissions IS
    'pd:personal — private exact proof-to-family admission and status custody';
COMMENT ON TABLE auth_security.registration_intents IS
    'pd:personal — private exact passkey enrollment and add-device proof binding';
COMMENT ON COLUMN auth_security.session_admissions.operation_id IS 'pd:personal — admission operation identity';
COMMENT ON COLUMN auth_security.session_admissions.status_secret_hash IS 'pd:personal — private status secret verifier';
COMMENT ON COLUMN auth_security.session_admissions.account_id IS 'pd:personal — Account identity';
COMMENT ON COLUMN auth_security.session_admissions.home_org_id IS 'pd:personal — Account Company';
COMMENT ON COLUMN auth_security.session_admissions.auth_generation IS 'pd:personal — captured authentication generation';
COMMENT ON COLUMN auth_security.session_admissions.session_purpose IS 'pd:personal — session authority purpose';
COMMENT ON COLUMN auth_security.session_admissions.status IS 'pd:personal — admission outcome';
COMMENT ON COLUMN auth_security.session_admissions.source_kind IS 'pd:personal — verified proof kind';
COMMENT ON COLUMN auth_security.session_admissions.source_row_id IS 'pd:personal — exact proof source';
COMMENT ON COLUMN auth_security.session_admissions.source_operation_id IS 'pd:personal — source issuance identity';
COMMENT ON COLUMN auth_security.session_admissions.proof_ceremony_id IS 'pd:personal — proof ceremony identity';
COMMENT ON COLUMN auth_security.session_admissions.family_id IS 'pd:personal — issued session family';
COMMENT ON COLUMN auth_security.session_admissions.receipt_id IS 'pd:personal — admission receipt identity';
COMMENT ON COLUMN auth_security.session_admissions.created_at IS 'pd:personal — admission creation time';
COMMENT ON COLUMN auth_security.session_admissions.expires_at IS 'pd:personal — admission deadline';
COMMENT ON COLUMN auth_security.session_admissions.finalized_at IS 'pd:personal — terminal outcome time';
COMMENT ON COLUMN auth_security.registration_intents.ceremony_id IS 'pd:personal — registration ceremony identity';
COMMENT ON COLUMN auth_security.registration_intents.account_id IS 'pd:personal — Account identity';
COMMENT ON COLUMN auth_security.registration_intents.home_org_id IS 'pd:personal — Account Company';
COMMENT ON COLUMN auth_security.registration_intents.auth_generation IS 'pd:personal — captured authentication generation';
COMMENT ON COLUMN auth_security.registration_intents.family_id IS 'pd:personal — initiating session family';
COMMENT ON COLUMN auth_security.registration_intents.purpose IS 'pd:personal — first-enrollment or add-device purpose';
COMMENT ON COLUMN auth_security.registration_intents.source_bootstrap_id IS 'pd:personal — exact OTP source';
COMMENT ON COLUMN auth_security.registration_intents.source_issuance_operation_id IS 'pd:personal — OTP issuance operation';
COMMENT ON COLUMN auth_security.registration_intents.step_up_ceremony_id IS 'pd:personal — UV step-up ceremony';
COMMENT ON COLUMN auth_security.registration_intents.step_up_credential_id IS 'pd:personal — UV step-up credential';
COMMENT ON COLUMN auth_security.registration_intents.created_at IS 'pd:personal — intent creation time';
COMMENT ON COLUMN auth_security.registration_intents.expires_at IS 'pd:personal — intent deadline';
COMMENT ON COLUMN auth_security.registration_intents.consumed_at IS 'pd:personal — registration completion time';
