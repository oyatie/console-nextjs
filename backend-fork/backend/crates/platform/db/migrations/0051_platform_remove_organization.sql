-- GUARDED tenant hard-removal for the PLATFORM (vendor) tier.
--
-- The platform console can ARCHIVE a tenant (status change, migration 0036's
-- platform_set_organization_status), but archiving keeps the org and all its rows
-- forever. A throwaway/test tenant created to rehearse onboarding has to be
-- genuinely REMOVED — the org row AND its bootstrap shell — not archived.
--
-- This is a privileged, cross-tenant, AUDITED operation, so it follows the exact
-- pattern of the other platform org escapes (platform_create_organization /
-- platform_set_organization_status, 0036): a SECURITY DEFINER function owned by
-- the table owner that briefly disables row_security CONFINED TO ITS OWN BODY and
-- restores it before returning. `console_rt` is SELECT-only on `organizations` and
-- FORCE-RLS-scoped on every tenant table, so it could never run this cascade
-- itself; the app calls this function and audits the result.
--
-- TWO HARD INVARIANTS shape the design:
--
--   1. NEVER hard-delete a tenant that is in real use. The function REFUSES
--      (returns 'blocked_has_data') if the tenant owns ANY business/operational
--      row beyond the empty onboarding shell — registry_*, work_orders,
--      inspections, sales, financial, messenger, location consents, attendance, or
--      governance findings. Only an empty/test tenant (org + the onboarding shell:
--      one SUPER_ADMIN user, its auth credentials, user_branches, branches,
--      regions) may be removed. The guard runs inside the SAME transaction as the
--      delete, so a tenant that gains real data between the API read and this call
--      is still refused atomically.
--
--   2. The audit trail is PERMANENT and its CONTENT IMMUTABLE (migration 0003:
--      append-only triggers + revoked UPDATE/DELETE). A removed tenant's
--      audit_events must NOT be destroyed — that would erase the record of what
--      happened, violating the audit-first discipline. But audit_events.org_id,
--      .actor and .branch_id are ON DELETE RESTRICT, so they would block deleting
--      the org/users/branches. We RESOLVE this by RE-HOMING the tenant's audit
--      rows to the platform sentinel org (...00face, an existing organizations
--      row) and NULLing their actor/branch_id FKs: the immutable record of the
--      action survives verbatim under the platform tier, while the tenant shell
--      becomes deletable.
--
--      The append-only trigger would abort that UPDATE. Rather than DISABLE the
--      trigger (a global weakening; ALTER TABLE is non-transaction-local DDL) or
--      use session_replication_role = replica (which requires SUPERUSER — the
--      DEFINER owner `console_app` is NOT a superuser under CNPG, so that path would
--      pass in a superuser sqlx::test and FAIL in production), we make the trigger
--      REMOVAL-AWARE: it permits an UPDATE that changes ONLY org_id/actor/branch_id
--      when the transaction-local GUC `app.audit_rehome = 'on'` is armed — settable
--      only inside this DEFINER function — and STILL rejects every change to the
--      audit CONTENT (action/target/snapshots/trace/occurred_at/id) and STILL
--      rejects ALL deletes, always. Audit content can never be rewritten; only the
--      tenant REFERENCES may be released, exactly once, during sanctioned removal,
--      gated by a DEFINER-only GUC — the same GUC-gating idiom the codebase already
--      uses for `app.current_org`. No superuser, no DDL.
--
-- Returns one of: 'removed' | 'blocked_has_data' | 'not_found'. The caller (the
-- platform provisioner) maps these to 200 / 409 / 404 and emits the
-- platform.tenant.remove audit event (org_id = NULL, platform-tier — the removed
-- org no longer exists, so its FK could not be satisfied).

-- ---------------------------------------------------------------------------
-- (1) Make the audit-immutability trigger removal-aware.
--
-- Unchanged behavior: ANY DELETE aborts; ANY UPDATE that touches a CONTENT column
-- aborts. NEW behavior: an UPDATE that touches ONLY org_id/actor/branch_id is
-- permitted IFF `app.audit_rehome = 'on'` is armed transaction-locally (only the
-- platform_remove_organization DEFINER sets it). This keeps the audit CONTENT
-- immutable forever while allowing the one sanctioned reference-release.
--
-- `audit_events_immutable()` is referenced ONLY by audit_events' two triggers
-- (0003), so redefining it affects nothing else.
-- ---------------------------------------------------------------------------
CREATE OR REPLACE FUNCTION audit_events_immutable()
RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    -- DELETE is never permitted under any circumstance.
    IF TG_OP = 'DELETE' THEN
        RAISE EXCEPTION
            'audit_events is append-only: % is forbidden (row id=%)',
            TG_OP, OLD.id;
    END IF;

    -- UPDATE: permit the sanctioned tenant-removal re-home (only the reference
    -- columns change, only when the DEFINER-armed GUC is on). Content is frozen.
    IF current_setting('app.audit_rehome', true) = 'on'
        AND NEW.id          IS NOT DISTINCT FROM OLD.id
        AND NEW.action      IS NOT DISTINCT FROM OLD.action
        AND NEW.target_type IS NOT DISTINCT FROM OLD.target_type
        AND NEW.target_id   IS NOT DISTINCT FROM OLD.target_id
        AND NEW.before_snap IS NOT DISTINCT FROM OLD.before_snap
        AND NEW.after_snap  IS NOT DISTINCT FROM OLD.after_snap
        AND NEW.trace_id    IS NOT DISTINCT FROM OLD.trace_id
        AND NEW.span_id     IS NOT DISTINCT FROM OLD.span_id
        AND NEW.occurred_at IS NOT DISTINCT FROM OLD.occurred_at
        AND NEW.created_at  IS NOT DISTINCT FROM OLD.created_at
    THEN
        RETURN NEW;
    END IF;

    RAISE EXCEPTION
        'audit_events is append-only: % is forbidden (row id=%)',
        TG_OP, OLD.id;
END;
$$;

-- ---------------------------------------------------------------------------
-- (2) platform_remove_organization(p_id) -> 'removed' | 'blocked_has_data'
--                                            | 'not_found'.
-- ---------------------------------------------------------------------------
CREATE OR REPLACE FUNCTION platform_remove_organization(p_id UUID)
RETURNS TEXT
LANGUAGE plpgsql
SECURITY DEFINER
-- Pin search_path: a SECURITY DEFINER function must not resolve objects through a
-- caller-controlled search_path (privilege-escalation hardening).
SET search_path = public, pg_temp
AS $$
DECLARE
    sentinel_org CONSTANT UUID := '00000000-0000-0000-0000-00000000face'::uuid;
    org_exists   BOOLEAN;
    has_data     BOOLEAN;
BEGIN
    -- The platform sentinel is not a removable tenant; refuse it explicitly so a
    -- bad id can never wipe the platform tier's own anchor row.
    IF p_id = sentinel_org THEN
        RETURN 'not_found';
    END IF;

    -- Run the entire guard + cascade with RLS off, confined to this function body
    -- and restored before EVERY return path (a successful return that forgot to
    -- restore would poison the caller's console_rt transaction — see 0036).
    SET LOCAL row_security = off;

    SELECT EXISTS (SELECT 1 FROM organizations WHERE id = p_id) INTO org_exists;
    IF NOT org_exists THEN
        SET LOCAL row_security = on;
        RETURN 'not_found';
    END IF;

    -- GUARD: any REAL business/operational row means this tenant is in use —
    -- refuse and change nothing. The set is the union of every table an
    -- admin/worker populates through real operation, EXCLUDING the onboarding
    -- shell (org/users/user_branches/branches/regions/auth_*), machine-generated
    -- counters, and audit_events (which is re-homed, not a "use" signal). Ordered
    -- cheapest/most-likely first for fast rejection.
    SELECT
        EXISTS (SELECT 1 FROM registry_equipment           WHERE org_id = p_id)
     OR EXISTS (SELECT 1 FROM work_orders                   WHERE org_id = p_id)
     OR EXISTS (SELECT 1 FROM registry_sites               WHERE org_id = p_id)
     OR EXISTS (SELECT 1 FROM registry_customers           WHERE org_id = p_id)
     OR EXISTS (SELECT 1 FROM inspection_rounds            WHERE org_id = p_id)
     OR EXISTS (SELECT 1 FROM regular_inspection_schedules WHERE org_id = p_id)
     OR EXISTS (SELECT 1 FROM sales_listings               WHERE org_id = p_id)
     OR EXISTS (SELECT 1 FROM customer_inquiries           WHERE org_id = p_id)
     OR EXISTS (SELECT 1 FROM financial_rental_quotes      WHERE org_id = p_id)
     OR EXISTS (SELECT 1 FROM financial_purchase_requests  WHERE org_id = p_id)
     OR EXISTS (SELECT 1 FROM equipment_cost_ledger        WHERE org_id = p_id)
     OR EXISTS (SELECT 1 FROM messenger_threads            WHERE org_id = p_id)
     OR EXISTS (SELECT 1 FROM location_consents            WHERE org_id = p_id)
     OR EXISTS (SELECT 1 FROM site_attendance_events       WHERE org_id = p_id)
     OR EXISTS (SELECT 1 FROM governance_findings          WHERE org_id = p_id)
     -- registered_devices has no org_id and is the one ON DELETE CASCADE child
     -- of users; guard it via the user join so a device-paired tenant returns a
     -- clean 409 instead of silently cascading the pairing away on user delete.
     OR EXISTS (SELECT 1 FROM registered_devices rd
                JOIN users u ON u.id = rd.user_id WHERE u.org_id = p_id)
    INTO has_data;

    IF has_data THEN
        SET LOCAL row_security = on;
        RETURN 'blocked_has_data';
    END IF;

    -- (a) Delete the auth children of the tenant's users. Each carries BOTH a
    -- single-column ON DELETE CASCADE FK AND a composite (user_id, org_id) ON
    -- DELETE RESTRICT FK (0034); the RESTRICT FK is what forces children-first, so
    -- delete them explicitly before users.
    DELETE FROM auth_refresh_tokens         WHERE org_id = p_id;
    DELETE FROM auth_refresh_token_families WHERE org_id = p_id;
    DELETE FROM auth_webauthn_credentials   WHERE org_id = p_id;
    -- auth_webauthn_ceremonies has no org_id (excluded in 0032) and CASCADEs from
    -- users; clear it explicitly by user so a half-finished enrollment row cannot
    -- linger or surprise the users delete.
    DELETE FROM auth_webauthn_ceremonies
        WHERE user_id IN (SELECT id FROM users WHERE org_id = p_id);
    DELETE FROM auth_bootstrap_credentials  WHERE org_id = p_id;

    -- (b) user_branches (composite RESTRICT to both users and branches).
    DELETE FROM user_branches WHERE org_id = p_id;

    -- (c) RE-HOME this tenant's immutable audit rows to the platform sentinel and
    -- drop their tenant FKs (actor → users, branch_id → branches). This MUST come
    -- BEFORE the users delete: audit_events.actor RESTRICT-references users, so a
    -- non-null actor would block deleting the very user it points at. The trail
    -- CONTENT is preserved verbatim; only the references are released, gated by the
    -- DEFINER-only GUC the removal-aware trigger above honors.
    PERFORM set_config('app.audit_rehome', 'on', true);
    UPDATE audit_events
    SET org_id    = sentinel_org,
        actor     = NULL,
        branch_id = NULL
    WHERE org_id = p_id;
    PERFORM set_config('app.audit_rehome', 'off', true);

    -- (d) The shell, parents now unblocked, children-first.
    DELETE FROM users    WHERE org_id = p_id;  -- cascades any remaining auth_* children
    DELETE FROM branches WHERE org_id = p_id;  -- RESTRICT → regions
    DELETE FROM regions  WHERE org_id = p_id;

    -- (e) Finally the org row itself. Every org_id FK is now satisfied.
    DELETE FROM organizations WHERE id = p_id;

    SET LOCAL row_security = on;
    RETURN 'removed';
END;
$$;

-- The runtime role may EXECUTE this (the app's platform path calls it), but still
-- cannot DELETE organizations (or any tenant rows cross-tenant) directly, and
-- still cannot UPDATE audit_events directly (the trigger rejects it without the
-- DEFINER-only GUC). PUBLIC gets no execute.
REVOKE ALL ON FUNCTION platform_remove_organization(UUID) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION platform_remove_organization(UUID) TO console_rt;
