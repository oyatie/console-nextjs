-- Production source ingress is machine-only.  Do not extend `users` or the
-- role matrix: a service principal has a different lifecycle and authority.
CREATE TABLE service_principals (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id UUID NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    branch_id UUID NOT NULL,
    feature TEXT NOT NULL CHECK (feature = 'production_source_ingest'),
    display_name TEXT NOT NULL CHECK (btrim(display_name) <> '' AND char_length(display_name) <= 80),
    verifier BYTEA NOT NULL CHECK (octet_length(verifier) = 32),
    generation INTEGER NOT NULL DEFAULT 1 CHECK (generation > 0),
    state TEXT NOT NULL DEFAULT 'ACTIVE' CHECK (state IN ('ACTIVE', 'DISABLED', 'ROTATION_REQUIRED')),
    created_by UUID NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    rotated_by UUID,
    rotated_at TIMESTAMPTZ,
    disabled_by UUID,
    disabled_at TIMESTAMPTZ,
    UNIQUE (id, org_id),
    UNIQUE (org_id, display_name),
    FOREIGN KEY (branch_id, org_id) REFERENCES branches(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (created_by, org_id) REFERENCES users(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (rotated_by, org_id) REFERENCES users(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (disabled_by, org_id) REFERENCES users(id, org_id) ON DELETE RESTRICT
);

CREATE TABLE service_principal_audit_events (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id UUID NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    service_principal_id UUID NOT NULL,
    event_type TEXT NOT NULL CHECK (event_type IN ('REGISTERED', 'ROTATED', 'DISABLED', 'LEGACY_ROTATION_REQUIRED')),
    actor_id UUID,
    expected_generation INTEGER,
    resulting_generation INTEGER NOT NULL,
    occurred_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    FOREIGN KEY (service_principal_id, org_id) REFERENCES service_principals(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (actor_id, org_id) REFERENCES users(id, org_id) ON DELETE RESTRICT
);

CREATE TABLE service_principal_ingress_claims (
    org_id UUID NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    service_principal_id UUID NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('DEMAND', 'CAPACITY', 'MATERIAL')),
    source_id TEXT NOT NULL CHECK (btrim(source_id) <> '' AND char_length(source_id) <= 160),
    source_version TEXT NOT NULL CHECK (btrim(source_version) <> '' AND char_length(source_version) <= 160),
    payload_hash TEXT NOT NULL CHECK (payload_hash ~ '^[a-f0-9]{64}$'),
    response JSONB,
    ingested_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    completed_at TIMESTAMPTZ,
    PRIMARY KEY (org_id, service_principal_id, kind, source_id, source_version),
    FOREIGN KEY (service_principal_id, org_id) REFERENCES service_principals(id, org_id) ON DELETE RESTRICT
);

ALTER TABLE service_principals ENABLE ROW LEVEL SECURITY;
ALTER TABLE service_principals FORCE ROW LEVEL SECURITY;
CREATE POLICY org_isolation ON service_principals
    USING (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid)
    WITH CHECK (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid);
ALTER TABLE service_principal_audit_events ENABLE ROW LEVEL SECURITY;
ALTER TABLE service_principal_audit_events FORCE ROW LEVEL SECURITY;
CREATE POLICY org_isolation ON service_principal_audit_events
    USING (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid)
    WITH CHECK (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid);
ALTER TABLE service_principal_ingress_claims ENABLE ROW LEVEL SECURITY;
ALTER TABLE service_principal_ingress_claims FORCE ROW LEVEL SECURITY;
CREATE POLICY org_isolation ON service_principal_ingress_claims
    USING (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid)
    WITH CHECK (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid);

-- Existing user-backed registrations cannot be safely transformed: their
-- client secret was never server-generated nor bound to a machine identity.
-- Fail closed and make operators register a new service principal.
ALTER TABLE production_source_systems
    ADD COLUMN credential_state TEXT NOT NULL DEFAULT 'ROTATION_REQUIRED'
        CHECK (credential_state = 'ROTATION_REQUIRED');
UPDATE production_source_systems SET enabled = false WHERE enabled;

-- This resolver is intentionally narrow: it is the sole pre-RLS bridge for a
-- Basic client id and returns only the owning org.  The caller then arms the
-- regular request context before touching tenant-scoped data.  It does not
-- return verifier material, branch scope, generation, or state.
CREATE OR REPLACE FUNCTION production_service_principal_org(p_principal_id UUID)
RETURNS UUID
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, public
SET row_security = on
AS $$
DECLARE
    resolved_org UUID;
BEGIN
    SELECT org_id INTO resolved_org
      FROM public.service_principals
     WHERE id = p_principal_id AND state = 'ACTIVE';
    RETURN resolved_org;
END;
$$;

REVOKE ALL ON FUNCTION production_service_principal_org(UUID) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION production_service_principal_org(UUID) TO console_rt;
REVOKE ALL ON service_principals, service_principal_audit_events, service_principal_ingress_claims FROM PUBLIC;
GRANT SELECT, INSERT, UPDATE ON service_principals, service_principal_audit_events, service_principal_ingress_claims TO console_rt;
REVOKE DELETE ON service_principals, service_principal_audit_events, service_principal_ingress_claims FROM console_rt;
