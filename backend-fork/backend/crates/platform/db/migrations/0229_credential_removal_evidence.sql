-- Credential removal evidence
-- Observed OLD row identity, not verified enrollment, recovery or human authority.
-- Detailed security-history retention applies; this is not the permanent UUID set.
CREATE TABLE auth_security.credential_removals (
    event_id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    credential_row_id UUID NOT NULL,
    account_id UUID NOT NULL,
    company_id UUID NOT NULL,
    credential_id TEXT NOT NULL,
    credential_created_at TIMESTAMPTZ NOT NULL,
    removed_at TIMESTAMPTZ NOT NULL
);

CREATE FUNCTION auth_security.record_credential_removal()
RETURNS TRIGGER
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog
AS $$
BEGIN
    -- DELETE already holds a credential lock: never acquire Account/security
    -- locks or parent FK locks here. Distinct removals append distinct events.
    INSERT INTO auth_security.credential_removals (
        credential_row_id, account_id, company_id, credential_id,
        credential_created_at, removed_at
    ) VALUES (
        OLD.id, OLD.user_id, OLD.org_id, OLD.credential_id,
        OLD.created_at, clock_timestamp()
    );
    RETURN NULL;
END;
$$;

CREATE TRIGGER auth_webauthn_credential_removal_evidence
AFTER DELETE ON public.auth_webauthn_credentials
FOR EACH ROW EXECUTE FUNCTION auth_security.record_credential_removal();

REVOKE ALL ON SCHEMA auth_security FROM PUBLIC, console_rt, console_leave_cmd,
    console_ontology_cmd, console_platform_force_cmd, console_leave_definer, console_ontology_writer;
REVOKE ALL ON TABLE auth_security.credential_removals FROM PUBLIC, console_rt, console_leave_cmd,
    console_ontology_cmd, console_platform_force_cmd, console_leave_definer, console_ontology_writer;
REVOKE ALL (event_id, credential_row_id, account_id, company_id, credential_id,
    credential_created_at, removed_at) ON auth_security.credential_removals
    FROM PUBLIC, console_rt, console_leave_cmd, console_ontology_cmd,
    console_platform_force_cmd, console_leave_definer, console_ontology_writer;
REVOKE ALL ON FUNCTION auth_security.record_credential_removal()
    FROM PUBLIC, console_rt, console_leave_cmd, console_ontology_cmd,
    console_platform_force_cmd, console_leave_definer, console_ontology_writer;
REVOKE TRUNCATE ON public.auth_webauthn_credentials
    FROM PUBLIC, console_rt, console_leave_cmd, console_ontology_cmd,
    console_platform_force_cmd, console_leave_definer, console_ontology_writer;
