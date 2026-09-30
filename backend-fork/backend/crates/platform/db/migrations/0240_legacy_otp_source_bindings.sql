-- Bind only newly redeemed version-0 OTP sessions to the proof actually
-- verified. Historical families remain unclassified and cannot acquire an
-- inferred source. Version-1 admission and bearer release stay dormant.
CREATE TABLE public.auth_legacy_otp_family_sources (
    family_id UUID PRIMARY KEY REFERENCES public.auth_refresh_token_families(id) ON DELETE CASCADE,
    source_id UUID NOT NULL,
    user_id UUID NOT NULL,
    org_id UUID NOT NULL,
    UNIQUE (family_id, source_id, user_id, org_id)
);

CREATE TABLE public.auth_legacy_registration_bindings (
    ceremony_id UUID PRIMARY KEY REFERENCES public.auth_webauthn_ceremonies(id) ON DELETE CASCADE,
    family_id UUID NOT NULL REFERENCES public.auth_refresh_token_families(id) ON DELETE CASCADE,
    source_id UUID,
    user_id UUID NOT NULL,
    org_id UUID NOT NULL,
    purpose TEXT NOT NULL CHECK (purpose IN ('legacy_otp', 'add_device')),
    CHECK ((purpose = 'legacy_otp') = (source_id IS NOT NULL)),
    FOREIGN KEY (family_id, source_id, user_id, org_id)
        REFERENCES public.auth_legacy_otp_family_sources(family_id, source_id, user_id, org_id)
);

-- Migration 0031 grants new public tables full runtime DML by default.
REVOKE ALL ON public.auth_legacy_otp_family_sources,
    public.auth_legacy_registration_bindings FROM PUBLIC, console_rt;

-- This is HTTP race containment for the existing v0 writer, not protected v1
-- provenance: console_rt still writes legacy auth tables and can forge rows if
-- compromised. V1 activation requires the separate command owner and old-writer
-- fence. Direct runtime UPDATE/DELETE on these bindings is denied, but
-- console_rt can delete a parent family and cascade the binding away. The
-- offline owner retains restore authority.
ALTER TABLE public.auth_legacy_otp_family_sources ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.auth_legacy_otp_family_sources FORCE ROW LEVEL SECURITY;
CREATE POLICY legacy_otp_family_runtime ON public.auth_legacy_otp_family_sources
    TO console_rt
    USING (org_id = NULLIF(current_setting('app.current_org', true), '')::UUID)
    WITH CHECK (org_id = NULLIF(current_setting('app.current_org', true), '')::UUID);
CREATE POLICY legacy_otp_family_owner ON public.auth_legacy_otp_family_sources
    TO console_app USING (true) WITH CHECK (true);
GRANT SELECT, INSERT ON public.auth_legacy_otp_family_sources TO console_rt;

ALTER TABLE public.auth_legacy_registration_bindings ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.auth_legacy_registration_bindings FORCE ROW LEVEL SECURITY;
CREATE POLICY legacy_registration_runtime ON public.auth_legacy_registration_bindings
    TO console_rt
    USING (org_id = NULLIF(current_setting('app.current_org', true), '')::UUID)
    WITH CHECK (org_id = NULLIF(current_setting('app.current_org', true), '')::UUID);
CREATE POLICY legacy_registration_owner ON public.auth_legacy_registration_bindings
    TO console_app USING (true) WITH CHECK (true);
GRANT SELECT, INSERT ON public.auth_legacy_registration_bindings TO console_rt;

COMMENT ON COLUMN public.auth_legacy_otp_family_sources.family_id IS 'pd:personal — issued session family identity';
COMMENT ON COLUMN public.auth_legacy_otp_family_sources.source_id IS 'pd:personal — exact legacy OTP source proven during family issue';
COMMENT ON COLUMN public.auth_legacy_otp_family_sources.user_id IS 'pd:personal — session owner identity';
COMMENT ON COLUMN public.auth_legacy_otp_family_sources.org_id IS 'pd:personal — session owner Company';
COMMENT ON COLUMN public.auth_legacy_registration_bindings.ceremony_id IS 'pd:personal — registration ceremony identity';
COMMENT ON COLUMN public.auth_legacy_registration_bindings.family_id IS 'pd:personal — initiating session family identity';
COMMENT ON COLUMN public.auth_legacy_registration_bindings.source_id IS 'pd:personal — exact legacy OTP source accepted when ceremony began';
COMMENT ON COLUMN public.auth_legacy_registration_bindings.user_id IS 'pd:personal — registration owner identity';
COMMENT ON COLUMN public.auth_legacy_registration_bindings.org_id IS 'pd:personal — registration owner Company';
COMMENT ON COLUMN public.auth_legacy_registration_bindings.purpose IS 'pd:personal — first-enrollment or add-device purpose';
