-- Expand only. Legacy proof and family rows stay version 0 and cannot acquire
-- business authority from this schema alone. The auth owner/ACL cutover follows.
LOCK TABLE public.users IN SHARE ROW EXCLUSIVE MODE;

CREATE TABLE auth_security.account_state (
    account_id UUID PRIMARY KEY,
    home_org_id UUID NOT NULL,
    auth_generation BIGINT NOT NULL DEFAULT 1 CHECK (auth_generation > 0),
    revision BIGINT NOT NULL DEFAULT 1 CHECK (revision > 0),
    ever_enrolled BOOLEAN,
    status TEXT NOT NULL CHECK (status IN (
        'never_enrolled', 'legacy_key_pending', 'unresolved',
        'recovery_required', 'enrolled', 'retired'
    )),
    classification_reason TEXT NOT NULL,
    classified_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    retired_at TIMESTAMPTZ,
    CHECK ((status = 'retired') = (retired_at IS NOT NULL)),
    CHECK (status <> 'never_enrolled' OR
           (ever_enrolled IS FALSE AND classification_reason = 'fresh_insert')),
    CHECK (status NOT IN ('legacy_key_pending', 'enrolled') OR ever_enrolled IS TRUE)
);

-- A key or recorded removal is evidence of enrollment history, not human proof.
-- Existing zero-key Accounts are never classified as new, even without records.
INSERT INTO auth_security.account_state (
    account_id, home_org_id, ever_enrolled, status, classification_reason
)
SELECT u.id, u.org_id,
       CASE WHEN evidence.has_key OR evidence.has_removal THEN true ELSE NULL END,
       CASE WHEN NOT u.is_active THEN 'recovery_required'
            WHEN evidence.has_key THEN 'legacy_key_pending'
            WHEN evidence.has_removal OR evidence.has_auth_history THEN 'recovery_required'
            ELSE 'unresolved' END,
       CASE WHEN NOT u.is_active THEN 'inactive_legacy'
            WHEN evidence.has_key THEN 'legacy_key_needs_codec_review'
            WHEN evidence.has_removal THEN 'legacy_credential_removal'
            WHEN evidence.has_auth_history THEN 'legacy_auth_history'
            ELSE 'legacy_history_unknown' END
FROM public.users u
CROSS JOIN LATERAL (
    SELECT EXISTS (SELECT 1 FROM public.auth_webauthn_credentials c WHERE c.user_id = u.id) AS has_key,
           EXISTS (SELECT 1 FROM auth_security.credential_removals r WHERE r.account_id = u.id) AS has_removal,
           EXISTS (SELECT 1 FROM public.auth_bootstrap_credentials b WHERE b.user_id = u.id
                   AND (b.consumed_at IS NOT NULL OR b.revoked_at IS NOT NULL))
           OR EXISTS (SELECT 1 FROM public.auth_refresh_token_families f WHERE f.user_id = u.id)
           AS has_auth_history
) evidence;

-- Extend the existing UUID guard, preserving reservation and immutable home
-- Company behavior. The private state is deliberately independent of FK cascades.
CREATE OR REPLACE FUNCTION auth_security.account_identity_guard()
RETURNS TRIGGER
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog
AS $$
BEGIN
    IF TG_OP = 'INSERT' THEN
        INSERT INTO auth_security.account_id_reservations (account_id, retired)
        VALUES (NEW.id, false);
        INSERT INTO auth_security.account_state (
            account_id, home_org_id, ever_enrolled, status, classification_reason
        ) VALUES (NEW.id, NEW.org_id, false, 'never_enrolled', 'fresh_insert');
        RETURN NEW;
    ELSIF TG_OP = 'UPDATE' THEN
        IF NEW.id IS DISTINCT FROM OLD.id OR NEW.org_id IS DISTINCT FROM OLD.org_id THEN
            RAISE EXCEPTION 'Account UUID and home Company are immutable' USING ERRCODE = '23514';
        END IF;
        RETURN NEW;
    ELSIF TG_OP = 'DELETE' THEN
        PERFORM auth_security.retire_account_id(OLD.id);
        UPDATE auth_security.account_state
        SET status = 'retired', retired_at = COALESCE(retired_at, clock_timestamp()),
            revision = revision + 1
        WHERE account_id = OLD.id AND home_org_id = OLD.org_id;
        IF NOT FOUND THEN
            RAISE EXCEPTION 'Account security state missing' USING ERRCODE = '23514';
        END IF;
        RETURN OLD;
    END IF;
    RAISE EXCEPTION 'unsupported Account identity transition' USING ERRCODE = '23514';
END;
$$;

-- The future command owner may advance security state, but cannot change home
-- identity, move generation/revision backward, forget known enrollment or
-- resurrect a retired Account. Pure OLD/NEW checks preserve Account lock order.
CREATE FUNCTION auth_security.guard_account_state_update()
RETURNS TRIGGER
LANGUAGE plpgsql
SET search_path = pg_catalog
AS $$
BEGIN
    IF NEW.account_id IS DISTINCT FROM OLD.account_id
       OR NEW.home_org_id IS DISTINCT FROM OLD.home_org_id
       OR NEW.auth_generation < OLD.auth_generation
       OR NEW.revision <= OLD.revision
       OR (OLD.ever_enrolled IS TRUE AND NEW.ever_enrolled IS NOT TRUE)
       OR (OLD.ever_enrolled IS FALSE AND NEW.ever_enrolled IS NULL)
       OR (OLD.ever_enrolled IS NULL AND NEW.ever_enrolled IS FALSE)
       OR (OLD.status = 'retired' AND
           (NEW.status <> 'retired' OR NEW.retired_at IS DISTINCT FROM OLD.retired_at)) THEN
        RAISE EXCEPTION 'Account security state cannot regress' USING ERRCODE = '23514';
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER account_security_state_monotonic
BEFORE UPDATE ON auth_security.account_state
FOR EACH ROW EXECUTE FUNCTION auth_security.guard_account_state_update();

ALTER TABLE public.auth_bootstrap_credentials
    ADD COLUMN issuance_version SMALLINT NOT NULL DEFAULT 0,
    ADD COLUMN issued_generation BIGINT,
    ADD COLUMN issuance_purpose TEXT,
    ADD COLUMN source_operation_id UUID,
    ADD CONSTRAINT auth_bootstrap_provenance_shape CHECK (
        (issuance_version = 0 AND issued_generation IS NULL
         AND issuance_purpose IS NULL AND source_operation_id IS NULL)
        OR (issuance_version = 1 AND issued_generation IS NOT NULL
            AND issued_generation > 0 AND issuance_purpose IS NOT NULL
            AND issuance_purpose = 'first_enrollment'
            AND source_operation_id IS NOT NULL)
    );
CREATE UNIQUE INDEX auth_bootstrap_source_operation_unique
    ON public.auth_bootstrap_credentials (source_operation_id)
    WHERE source_operation_id IS NOT NULL;

ALTER TABLE public.auth_refresh_token_families
    ADD COLUMN provenance_version SMALLINT NOT NULL DEFAULT 0,
    ADD COLUMN auth_generation BIGINT,
    ADD COLUMN session_purpose TEXT,
    ADD COLUMN source_kind TEXT,
    ADD COLUMN source_operation_id UUID,
    ADD CONSTRAINT auth_family_provenance_shape CHECK (
        (provenance_version = 0 AND auth_generation IS NULL
         AND session_purpose IS NULL AND source_kind IS NULL
         AND source_operation_id IS NULL)
        OR (provenance_version = 1 AND auth_generation IS NOT NULL
            AND auth_generation > 0 AND session_purpose IS NOT NULL
            AND source_kind IS NOT NULL
            AND source_operation_id IS NOT NULL
            AND ((session_purpose = 'enrollment' AND source_kind = 'bootstrap_otp')
              OR (session_purpose = 'normal'
                  AND source_kind IN ('passkey', 'device_handoff'))))
    );
CREATE UNIQUE INDEX auth_family_source_operation_unique
    ON public.auth_refresh_token_families (source_operation_id)
    WHERE source_operation_id IS NOT NULL;

-- Versioned source meaning cannot be retrofitted onto a legacy row or changed
-- after insertion. Existing version-0 token/OTP rotation remains available
-- until the owner cutover fences old writers and rejects version 0.
CREATE FUNCTION auth_security.guard_auth_provenance_update()
RETURNS TRIGGER
LANGUAGE plpgsql
SET search_path = pg_catalog
AS $$
BEGIN
    IF TG_TABLE_NAME = 'auth_bootstrap_credentials' THEN
        IF ROW(OLD.issuance_version, OLD.issued_generation, OLD.issuance_purpose,
               OLD.source_operation_id) IS DISTINCT FROM
           ROW(NEW.issuance_version, NEW.issued_generation, NEW.issuance_purpose,
               NEW.source_operation_id) THEN
            RAISE EXCEPTION 'bootstrap provenance is immutable' USING ERRCODE = '23514';
        END IF;
    ELSIF TG_TABLE_NAME = 'auth_refresh_token_families' THEN
        IF ROW(OLD.provenance_version, OLD.auth_generation, OLD.session_purpose,
               OLD.source_kind, OLD.source_operation_id) IS DISTINCT FROM
           ROW(NEW.provenance_version, NEW.auth_generation, NEW.session_purpose,
               NEW.source_kind, NEW.source_operation_id) THEN
            RAISE EXCEPTION 'family provenance is immutable' USING ERRCODE = '23514';
        END IF;
    ELSE
        RAISE EXCEPTION 'unsupported provenance table' USING ERRCODE = '23514';
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER auth_bootstrap_provenance_immutable
BEFORE UPDATE ON public.auth_bootstrap_credentials
FOR EACH ROW EXECUTE FUNCTION auth_security.guard_auth_provenance_update();
CREATE TRIGGER auth_family_provenance_immutable
BEFORE UPDATE ON public.auth_refresh_token_families
FOR EACH ROW EXECUTE FUNCTION auth_security.guard_auth_provenance_update();

REVOKE ALL ON TABLE auth_security.account_state FROM PUBLIC, console_rt,
    console_leave_cmd, console_ontology_cmd, console_platform_force_cmd,
    console_leave_definer, console_ontology_writer;
REVOKE ALL ON FUNCTION auth_security.account_identity_guard() FROM PUBLIC, console_rt,
    console_leave_cmd, console_ontology_cmd, console_platform_force_cmd,
    console_leave_definer, console_ontology_writer;
REVOKE ALL ON FUNCTION auth_security.guard_account_state_update() FROM PUBLIC, console_rt,
    console_leave_cmd, console_ontology_cmd, console_platform_force_cmd,
    console_leave_definer, console_ontology_writer;
REVOKE ALL ON FUNCTION auth_security.guard_auth_provenance_update() FROM PUBLIC, console_rt,
    console_leave_cmd, console_ontology_cmd, console_platform_force_cmd,
    console_leave_definer, console_ontology_writer;

COMMENT ON COLUMN auth_security.account_state.account_id IS 'pd:personal — immutable Account identity';
COMMENT ON COLUMN auth_security.account_state.home_org_id IS 'pd:personal — Account home Company';
COMMENT ON COLUMN auth_security.account_state.auth_generation IS 'pd:personal — protected authentication generation';
COMMENT ON COLUMN auth_security.account_state.revision IS 'pd:personal — protected Account state revision';
COMMENT ON COLUMN auth_security.account_state.ever_enrolled IS 'pd:personal — recorded enrollment history, not human proof';
COMMENT ON COLUMN auth_security.account_state.status IS 'pd:personal — enrollment/recovery classification';
COMMENT ON COLUMN auth_security.account_state.classification_reason IS 'pd:personal — classification evidence category';
COMMENT ON COLUMN auth_security.account_state.classified_at IS 'pd:personal — classification instant';
COMMENT ON COLUMN auth_security.account_state.retired_at IS 'pd:personal — Account retirement instant';
COMMENT ON COLUMN auth_security.account_id_reservations.account_id IS 'pd:personal — permanently allocated Account UUID';
COMMENT ON COLUMN auth_security.account_id_reservations.retired IS 'pd:personal — Account UUID retirement marker';
COMMENT ON COLUMN auth_security.credential_removals.event_id IS 'pd:personal — credential-removal evidence event';
COMMENT ON COLUMN auth_security.credential_removals.credential_row_id IS 'pd:personal — removed credential row';
COMMENT ON COLUMN auth_security.credential_removals.account_id IS 'pd:personal — removed credential Account';
COMMENT ON COLUMN auth_security.credential_removals.company_id IS 'pd:personal — removed credential Company context';
COMMENT ON COLUMN auth_security.credential_removals.credential_id IS 'pd:personal — removed WebAuthn credential identifier';
COMMENT ON COLUMN auth_security.credential_removals.credential_created_at IS 'pd:personal — removed credential creation instant';
COMMENT ON COLUMN auth_security.credential_removals.removed_at IS 'pd:personal — credential removal instant';
COMMENT ON COLUMN public.auth_bootstrap_credentials.issuance_version IS 'pd:personal — OTP provenance codec version';
COMMENT ON COLUMN public.auth_bootstrap_credentials.issued_generation IS 'pd:personal — OTP Account generation';
COMMENT ON COLUMN public.auth_bootstrap_credentials.issuance_purpose IS 'pd:personal — OTP purpose';
COMMENT ON COLUMN public.auth_bootstrap_credentials.source_operation_id IS 'pd:personal — OTP issuance operation identity';
COMMENT ON COLUMN public.auth_refresh_token_families.provenance_version IS 'pd:personal — session provenance codec version';
COMMENT ON COLUMN public.auth_refresh_token_families.auth_generation IS 'pd:personal — session Account generation';
COMMENT ON COLUMN public.auth_refresh_token_families.session_purpose IS 'pd:personal — session purpose';
COMMENT ON COLUMN public.auth_refresh_token_families.source_kind IS 'pd:personal — proven authentication source kind';
COMMENT ON COLUMN public.auth_refresh_token_families.source_operation_id IS 'pd:personal — proof operation identity';
