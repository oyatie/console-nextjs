-- Expand only: private custody of an original short-lived version-0 proof.
-- No root FK, runtime private-schema grant, v1 activation or historical rewrite.
CREATE TABLE auth_security.browser_sessions (
    token_hash BYTEA PRIMARY KEY CHECK (octet_length(token_hash) = 32),
    context_id UUID NOT NULL UNIQUE,
    account_id UUID NOT NULL,
    company_id UUID NOT NULL,
    family_id UUID NOT NULL UNIQUE,
    source_credential_id UUID NOT NULL,
    expires_at TIMESTAMPTZ NOT NULL CHECK (expires_at = date_trunc('second', expires_at)),
    codec_version INTEGER NOT NULL DEFAULT 1 CHECK (codec_version = 1),
    ciphertext BYTEA,
    nonce BYTEA,
    tag BYTEA,
    closed_at TIMESTAMPTZ,
    owner_removed_at TIMESTAMPTZ,
    CHECK ((ciphertext IS NULL AND nonce IS NULL AND tag IS NULL)
        OR (ciphertext IS NOT NULL AND nonce IS NOT NULL AND tag IS NOT NULL
            AND octet_length(ciphertext) BETWEEN 1 AND 1048576
            AND octet_length(nonce) = 12 AND octet_length(tag) = 16)),
    CHECK (closed_at IS NULL OR ciphertext IS NULL),
    CHECK (owner_removed_at IS NULL OR closed_at IS NOT NULL)
);
CREATE INDEX browser_session_expired_proof ON auth_security.browser_sessions(company_id, expires_at)
    WHERE ciphertext IS NOT NULL;
COMMENT ON TABLE auth_security.browser_sessions IS 'Short-lived encrypted proof staging and retained token-free reconciliation identity; staging and retained metadata have distinct record schedules.';
COMMENT ON COLUMN auth_security.browser_sessions.token_hash IS 'pd:personal — digest of an opaque browser credential; never expose';
COMMENT ON COLUMN auth_security.browser_sessions.context_id IS 'pd:personal — consumed login ceremony and public browser context';
COMMENT ON COLUMN auth_security.browser_sessions.account_id IS 'pd:personal — exact Account owner';
COMMENT ON COLUMN auth_security.browser_sessions.company_id IS 'pd:personal — exact legal Company scope';
COMMENT ON COLUMN auth_security.browser_sessions.family_id IS 'pd:personal — exact native session family';
COMMENT ON COLUMN auth_security.browser_sessions.source_credential_id IS 'pd:personal — proven source credential row';
COMMENT ON COLUMN auth_security.browser_sessions.expires_at IS 'pd:personal — original integer signed access deadline';
COMMENT ON COLUMN auth_security.browser_sessions.codec_version IS 'pd:personal — immutable local proof codec version';
COMMENT ON COLUMN auth_security.browser_sessions.ciphertext IS 'pd:personal — encrypted original signed proof staging; never log or disclose';
COMMENT ON COLUMN auth_security.browser_sessions.nonce IS 'pd:personal — AEAD proof nonce staging';
COMMENT ON COLUMN auth_security.browser_sessions.tag IS 'pd:personal — AEAD proof authentication tag staging';
COMMENT ON COLUMN auth_security.browser_sessions.closed_at IS 'pd:personal — confirmed native closure time';
COMMENT ON COLUMN auth_security.browser_sessions.owner_removed_at IS 'pd:personal — committed Account removal evidence';
ALTER TABLE auth_security.browser_sessions ENABLE ROW LEVEL SECURITY;
ALTER TABLE auth_security.browser_sessions FORCE ROW LEVEL SECURITY;
CREATE POLICY auth_owner_all ON auth_security.browser_sessions
    TO console_app USING (true) WITH CHECK (true);
CREATE POLICY auth_command_company ON auth_security.browser_sessions
    TO console_auth_cmd
    USING (company_id = NULLIF(pg_catalog.current_setting('app.current_org', true), '')::pg_catalog.uuid)
    WITH CHECK (company_id = NULLIF(pg_catalog.current_setting('app.current_org', true), '')::pg_catalog.uuid);
REVOKE ALL ON auth_security.browser_sessions FROM PUBLIC, console_rt, console_auth_cmd,
    console_leave_cmd, console_ontology_cmd, console_platform_force_cmd;

CREATE FUNCTION auth_security.guard_browser_session_update()
RETURNS TRIGGER LANGUAGE plpgsql
SET search_path = pg_catalog SET row_security = on
AS $guard$
BEGIN
    IF TG_OP = 'INSERT' THEN
        IF NEW.ciphertext IS NULL OR NEW.closed_at IS NOT NULL OR NEW.owner_removed_at IS NOT NULL THEN
            RAISE EXCEPTION 'browser custody must begin live' USING ERRCODE = '23514';
        END IF;
        RETURN NEW;
    END IF;
    IF NEW.token_hash IS DISTINCT FROM OLD.token_hash
       OR NEW.context_id IS DISTINCT FROM OLD.context_id
       OR NEW.account_id IS DISTINCT FROM OLD.account_id
       OR NEW.company_id IS DISTINCT FROM OLD.company_id
       OR NEW.family_id IS DISTINCT FROM OLD.family_id
       OR NEW.source_credential_id IS DISTINCT FROM OLD.source_credential_id
       OR NEW.expires_at IS DISTINCT FROM OLD.expires_at
       OR NEW.codec_version IS DISTINCT FROM OLD.codec_version
       OR (OLD.closed_at IS NOT NULL AND NEW.closed_at IS DISTINCT FROM OLD.closed_at)
       OR (OLD.owner_removed_at IS NOT NULL AND NEW.owner_removed_at IS DISTINCT FROM OLD.owner_removed_at)
       OR (NEW.ciphertext IS NOT NULL AND
           (NEW.ciphertext IS DISTINCT FROM OLD.ciphertext
            OR NEW.nonce IS DISTINCT FROM OLD.nonce OR NEW.tag IS DISTINCT FROM OLD.tag)) THEN
        RAISE EXCEPTION 'browser custody is immutable' USING ERRCODE = '23514';
    END IF;
    IF NEW.owner_removed_at IS DISTINCT FROM OLD.owner_removed_at THEN
        -- Only the nested Account DELETE retirement owner reaches this arm.
        -- No caller-controlled GUC authorizes a terminal removal marker.
        IF pg_catalog.pg_trigger_depth() <= 1 OR NOT EXISTS (
            SELECT 1 FROM auth_security.account_state a
            JOIN auth_security.account_id_reservations r ON r.account_id = a.account_id
            WHERE a.account_id = OLD.account_id AND a.home_org_id = OLD.company_id
              AND a.status = 'retired' AND r.retired
        ) THEN
            RAISE EXCEPTION 'browser owner removal requires Account retirement' USING ERRCODE = '23514';
        END IF;
    ELSIF NEW.closed_at IS DISTINCT FROM OLD.closed_at THEN
        IF NOT EXISTS (
            SELECT 1 FROM public.auth_refresh_token_families f
            WHERE f.id = OLD.family_id AND f.user_id = OLD.account_id AND f.org_id = OLD.company_id
              AND f.revoked_at IS NOT NULL
              AND f.xmin = pg_catalog.pg_current_xact_id()::pg_catalog.xid
        ) OR EXISTS (
            SELECT 1 FROM public.auth_refresh_tokens t WHERE t.family_id = OLD.family_id
              AND (t.user_id <> OLD.account_id OR t.org_id <> OLD.company_id
                   OR t.revoked_at IS NULL OR t.xmin <> pg_catalog.pg_current_xact_id()::pg_catalog.xid)
        ) THEN
            RAISE EXCEPTION 'browser closure requires native family revocation' USING ERRCODE = '23514';
        END IF;
    ELSIF OLD.ciphertext IS NOT NULL AND NEW.ciphertext IS NULL
          AND OLD.expires_at > pg_catalog.clock_timestamp() THEN
        RAISE EXCEPTION 'live browser proof cannot be cleared' USING ERRCODE = '23514';
    END IF;
    RETURN NEW;
END;
$guard$;
CREATE TRIGGER browser_session_monotonic BEFORE INSERT OR UPDATE
    ON auth_security.browser_sessions FOR EACH ROW
    EXECUTE FUNCTION auth_security.guard_browser_session_update();
REVOKE ALL ON FUNCTION auth_security.guard_browser_session_update() FROM PUBLIC,
    console_rt, console_auth_cmd, console_leave_cmd, console_ontology_cmd, console_platform_force_cmd;

CREATE FUNCTION public.platform_browser_session_company(p_hash pg_catalog.bytea, p_context pg_catalog.uuid)
RETURNS pg_catalog.uuid LANGUAGE sql SECURITY DEFINER
SET search_path = pg_catalog SET row_security = on
AS $function$
    SELECT s.company_id FROM auth_security.browser_sessions s
    WHERE s.token_hash = p_hash AND s.context_id = p_context AND pg_catalog.octet_length(p_hash) = 32
$function$;

CREATE FUNCTION public.platform_browser_session_read(p_company pg_catalog.uuid, p_hash pg_catalog.bytea, p_context pg_catalog.uuid)
RETURNS TABLE(account_id pg_catalog.uuid, company_id pg_catalog.uuid, family_id pg_catalog.uuid,
    source_credential_id pg_catalog.uuid, context_id pg_catalog.uuid, expires_at pg_catalog.timestamptz,
    codec_version pg_catalog.int4, ciphertext pg_catalog.bytea, nonce pg_catalog.bytea, tag pg_catalog.bytea,
    closed_at pg_catalog.timestamptz, owner_removed_at pg_catalog.timestamptz)
LANGUAGE sql SECURITY DEFINER SET search_path = pg_catalog SET row_security = on
AS $function$
    SELECT s.account_id, s.company_id, s.family_id, s.source_credential_id, s.context_id,
        s.expires_at, s.codec_version,
        CASE WHEN s.codec_version = 1 AND pg_catalog.octet_length(s.ciphertext) BETWEEN 1 AND 1048576
             AND pg_catalog.octet_length(s.nonce) = 12 AND pg_catalog.octet_length(s.tag) = 16
             AND NOT EXISTS (SELECT 1 FROM auth_security.credential_removals r
                 WHERE r.credential_row_id = s.source_credential_id
                   AND r.account_id = s.account_id AND r.company_id = s.company_id)
             THEN s.ciphertext ELSE NULL END,
        CASE WHEN pg_catalog.octet_length(s.nonce) = 12 THEN s.nonce ELSE NULL END,
        CASE WHEN pg_catalog.octet_length(s.tag) = 16 THEN s.tag ELSE NULL END,
        s.closed_at, s.owner_removed_at
    FROM auth_security.browser_sessions s
    WHERE s.company_id = p_company AND s.token_hash = p_hash AND s.context_id = p_context
      AND p_company = NULLIF(pg_catalog.current_setting('app.current_org', true), '')::pg_catalog.uuid
$function$;

CREATE FUNCTION public.platform_browser_session_insert(
    p_company pg_catalog.uuid, p_account pg_catalog.uuid, p_family pg_catalog.uuid,
    p_source pg_catalog.uuid, p_context pg_catalog.uuid, p_hash pg_catalog.bytea,
    p_expires pg_catalog.timestamptz, p_ciphertext pg_catalog.bytea, p_nonce pg_catalog.bytea, p_tag pg_catalog.bytea)
RETURNS pg_catalog.void LANGUAGE plpgsql SECURITY DEFINER
SET search_path = pg_catalog SET row_security = on
AS $function$
BEGIN
    IF p_company IS DISTINCT FROM NULLIF(pg_catalog.current_setting('app.current_org', true), '')::pg_catalog.uuid
       OR p_company IS NULL THEN
        RAISE EXCEPTION 'browser insert requires exact Company scope' USING ERRCODE = '42501';
    END IF;
    IF pg_catalog.octet_length(p_hash) IS DISTINCT FROM 32
       OR pg_catalog.octet_length(p_nonce) IS DISTINCT FROM 12
       OR pg_catalog.octet_length(p_tag) IS DISTINCT FROM 16
       OR p_ciphertext IS NULL OR pg_catalog.octet_length(p_ciphertext) NOT BETWEEN 1 AND 1048576
       OR p_expires IS NULL OR p_expires <> pg_catalog.date_trunc('second', p_expires) THEN
        RAISE EXCEPTION 'invalid browser custody shape' USING ERRCODE = '23514';
    END IF;
    -- The native successful outcome and Account/ceremony/source locks own call
    -- order. xmin checks transaction freshness, not protected v1 provenance.
    IF NOT EXISTS (
        SELECT 1 FROM public.auth_webauthn_ceremonies c
        WHERE c.id = p_context AND c.ceremony_kind = 'authentication'
          AND c.consumed_at IS NOT NULL AND c.consumed_at < c.expires_at
          AND c.xmin = pg_catalog.pg_current_xact_id()::pg_catalog.xid
    ) OR NOT EXISTS (
        SELECT 1 FROM public.users u JOIN public.organizations o ON o.id = u.org_id
        JOIN public.auth_webauthn_credentials k ON k.user_id = u.id AND k.org_id = u.org_id
        WHERE u.id = p_account AND u.org_id = p_company AND u.is_active AND o.status = 'ACTIVE'
          AND k.id = p_source
          AND NOT EXISTS (SELECT 1 FROM auth_security.credential_removals r
              WHERE r.credential_row_id = p_source AND r.account_id = p_account AND r.company_id = p_company)
    ) OR NOT EXISTS (
        SELECT 1 FROM public.auth_refresh_token_families f
        JOIN public.auth_refresh_tokens t ON t.family_id = f.id AND t.user_id = f.user_id AND t.org_id = f.org_id
        WHERE f.id = p_family AND f.user_id = p_account AND f.org_id = p_company
          AND f.provenance_version = 0 AND f.revoked_at IS NULL
          AND f.xmin = pg_catalog.pg_current_xact_id()::pg_catalog.xid
          AND t.xmin = pg_catalog.pg_current_xact_id()::pg_catalog.xid AND t.revoked_at IS NULL
          AND p_expires <= t.expires_at
          AND NOT EXISTS (SELECT 1 FROM public.auth_legacy_otp_family_sources b WHERE b.family_id = f.id)
    ) THEN
        RAISE EXCEPTION 'browser custody requires fresh native passkey issue' USING ERRCODE = '23514';
    END IF;
    INSERT INTO auth_security.browser_sessions(token_hash, context_id, account_id, company_id,
        family_id, source_credential_id, expires_at, ciphertext, nonce, tag)
    VALUES(p_hash, p_context, p_account, p_company, p_family, p_source, p_expires, p_ciphertext, p_nonce, p_tag);
END;
$function$;

CREATE FUNCTION public.platform_browser_session_lock(p_company pg_catalog.uuid, p_hash pg_catalog.bytea, p_context pg_catalog.uuid)
RETURNS TABLE(account_id pg_catalog.uuid, company_id pg_catalog.uuid, family_id pg_catalog.uuid,
    source_credential_id pg_catalog.uuid, context_id pg_catalog.uuid, expires_at pg_catalog.timestamptz,
    codec_version pg_catalog.int4, ciphertext pg_catalog.bytea, nonce pg_catalog.bytea, tag pg_catalog.bytea,
    closed_at pg_catalog.timestamptz, owner_removed_at pg_catalog.timestamptz)
LANGUAGE sql SECURITY DEFINER SET search_path = pg_catalog SET row_security = on
AS $function$
    SELECT s.account_id, s.company_id, s.family_id, s.source_credential_id, s.context_id,
        s.expires_at, s.codec_version, s.ciphertext, s.nonce, s.tag, s.closed_at, s.owner_removed_at
    FROM auth_security.browser_sessions s
    WHERE s.company_id = p_company AND s.token_hash = p_hash AND s.context_id = p_context
      AND p_company = NULLIF(pg_catalog.current_setting('app.current_org', true), '')::pg_catalog.uuid
    FOR UPDATE OF s
$function$;

CREATE FUNCTION public.platform_browser_session_close(p_company pg_catalog.uuid, p_hash pg_catalog.bytea, p_context pg_catalog.uuid)
RETURNS pg_catalog.bool LANGUAGE plpgsql SECURITY DEFINER
SET search_path = pg_catalog SET row_security = on
AS $function$
BEGIN
    IF p_company IS NULL OR p_company IS DISTINCT FROM
        NULLIF(pg_catalog.current_setting('app.current_org', true), '')::pg_catalog.uuid THEN RETURN false; END IF;
    UPDATE auth_security.browser_sessions s
    SET closed_at = pg_catalog.clock_timestamp(), ciphertext = NULL, nonce = NULL, tag = NULL
    WHERE s.company_id = p_company AND s.token_hash = p_hash AND s.context_id = p_context
      AND s.closed_at IS NULL AND s.owner_removed_at IS NULL
      AND EXISTS (SELECT 1 FROM public.auth_refresh_token_families f
          WHERE f.id = s.family_id AND f.user_id = s.account_id AND f.org_id = s.company_id
            AND f.revoked_at IS NOT NULL AND f.xmin = pg_catalog.pg_current_xact_id()::pg_catalog.xid)
      AND NOT EXISTS (SELECT 1 FROM public.auth_refresh_tokens t WHERE t.family_id = s.family_id
          AND (t.user_id <> s.account_id OR t.org_id <> s.company_id OR t.revoked_at IS NULL
               OR t.xmin <> pg_catalog.pg_current_xact_id()::pg_catalog.xid));
    RETURN FOUND;
END;
$function$;

CREATE FUNCTION public.platform_browser_session_cleanup_companies(p_after pg_catalog.uuid, p_limit pg_catalog.int4)
RETURNS TABLE(company_id pg_catalog.uuid) LANGUAGE sql SECURITY DEFINER
SET search_path = pg_catalog SET row_security = on
AS $function$
    SELECT DISTINCT s.company_id FROM auth_security.browser_sessions s
    WHERE s.expires_at <= pg_catalog.clock_timestamp()
      AND (p_after IS NULL OR s.company_id > p_after) AND p_limit BETWEEN 1 AND 1000
    ORDER BY s.company_id LIMIT GREATEST(0, LEAST(p_limit, 1000))
$function$;

CREATE FUNCTION public.platform_browser_session_cleanup(p_company pg_catalog.uuid, p_limit pg_catalog.int4)
RETURNS pg_catalog.int8 LANGUAGE plpgsql SECURITY DEFINER
SET search_path = pg_catalog SET row_security = on
AS $function$
DECLARE cleared pg_catalog.int8;
BEGIN
    IF p_company IS NULL OR p_company IS DISTINCT FROM
        NULLIF(pg_catalog.current_setting('app.current_org', true), '')::pg_catalog.uuid
        OR p_limit IS NULL OR p_limit NOT BETWEEN 1 AND 1000 THEN RETURN 0; END IF;
    WITH picked AS (
        SELECT s.token_hash FROM auth_security.browser_sessions s
        WHERE s.company_id = p_company AND s.ciphertext IS NOT NULL
          AND s.expires_at <= pg_catalog.clock_timestamp()
        ORDER BY s.expires_at, s.context_id LIMIT p_limit FOR UPDATE SKIP LOCKED
    )
    UPDATE auth_security.browser_sessions s SET ciphertext = NULL, nonce = NULL, tag = NULL
    FROM picked p WHERE s.company_id = p_company AND s.token_hash = p.token_hash
      AND s.expires_at <= pg_catalog.clock_timestamp();
    GET DIAGNOSTICS cleared = ROW_COUNT;
    RETURN cleared;
END;
$function$;

REVOKE ALL ON FUNCTION public.platform_browser_session_company(pg_catalog.bytea,pg_catalog.uuid),
    public.platform_browser_session_read(pg_catalog.uuid,pg_catalog.bytea,pg_catalog.uuid),
    public.platform_browser_session_insert(pg_catalog.uuid,pg_catalog.uuid,pg_catalog.uuid,pg_catalog.uuid,pg_catalog.uuid,pg_catalog.bytea,pg_catalog.timestamptz,pg_catalog.bytea,pg_catalog.bytea,pg_catalog.bytea),
    public.platform_browser_session_lock(pg_catalog.uuid,pg_catalog.bytea,pg_catalog.uuid),
    public.platform_browser_session_close(pg_catalog.uuid,pg_catalog.bytea,pg_catalog.uuid),
    public.platform_browser_session_cleanup_companies(pg_catalog.uuid,pg_catalog.int4),
    public.platform_browser_session_cleanup(pg_catalog.uuid,pg_catalog.int4)
    FROM PUBLIC, console_rt, console_auth_cmd, console_leave_cmd, console_ontology_cmd, console_platform_force_cmd;
GRANT EXECUTE ON FUNCTION public.platform_browser_session_company(pg_catalog.bytea,pg_catalog.uuid),
    public.platform_browser_session_read(pg_catalog.uuid,pg_catalog.bytea,pg_catalog.uuid),
    public.platform_browser_session_insert(pg_catalog.uuid,pg_catalog.uuid,pg_catalog.uuid,pg_catalog.uuid,pg_catalog.uuid,pg_catalog.bytea,pg_catalog.timestamptz,pg_catalog.bytea,pg_catalog.bytea,pg_catalog.bytea),
    public.platform_browser_session_lock(pg_catalog.uuid,pg_catalog.bytea,pg_catalog.uuid),
    public.platform_browser_session_close(pg_catalog.uuid,pg_catalog.bytea,pg_catalog.uuid),
    public.platform_browser_session_cleanup_companies(pg_catalog.uuid,pg_catalog.int4),
    public.platform_browser_session_cleanup(pg_catalog.uuid,pg_catalog.int4) TO console_rt;

-- Extend only the existing DELETE arm; retain all prior identity/retirement behavior.
CREATE OR REPLACE FUNCTION auth_security.account_identity_guard()
RETURNS TRIGGER LANGUAGE plpgsql SECURITY DEFINER
SET search_path = pg_catalog SET row_security = on
AS $function$
BEGIN
    IF TG_OP = 'INSERT' THEN
        INSERT INTO auth_security.account_id_reservations(account_id, retired) VALUES(NEW.id, false);
        INSERT INTO auth_security.account_state(account_id, home_org_id, ever_enrolled, status, classification_reason)
        VALUES(NEW.id, NEW.org_id, false, 'never_enrolled', 'fresh_insert');
        RETURN NEW;
    ELSIF TG_OP = 'UPDATE' THEN
        IF NEW.id IS DISTINCT FROM OLD.id OR NEW.org_id IS DISTINCT FROM OLD.org_id THEN
            RAISE EXCEPTION 'Account UUID and home Company are immutable' USING ERRCODE = '23514';
        END IF;
        RETURN NEW;
    ELSIF TG_OP = 'DELETE' THEN
        PERFORM auth_security.retire_account_id(OLD.id);
        UPDATE auth_security.account_state
        SET status = 'retired', retired_at = COALESCE(retired_at, pg_catalog.clock_timestamp()), revision = revision + 1
        WHERE account_id = OLD.id AND home_org_id = OLD.org_id;
        IF NOT FOUND THEN
            RAISE EXCEPTION 'Account security state missing' USING ERRCODE = '23514';
        END IF;
        UPDATE auth_security.browser_sessions
        SET ciphertext = NULL, nonce = NULL, tag = NULL,
            closed_at = COALESCE(closed_at, pg_catalog.clock_timestamp()),
            owner_removed_at = COALESCE(owner_removed_at, pg_catalog.clock_timestamp())
        WHERE account_id = OLD.id AND company_id = OLD.org_id;
        RETURN OLD;
    END IF;
    RAISE EXCEPTION 'unsupported Account identity transition' USING ERRCODE = '23514';
END;
$function$;
