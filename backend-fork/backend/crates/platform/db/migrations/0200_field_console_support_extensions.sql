-- CAP-FIELD-CONSOLE: bind support intake to the field object chain
-- (site / customer / work order) and record customer acceptance as the
-- audited closure evidence for field visits.
--
-- NOTE: 0194 is a PROVISIONAL number claimed by the CAP-FIELD-CONSOLE lane
-- (worktree HEAD = 0180); the consolidation integrator renumbers this file to
-- the next free number at integration (migration collision protocol, PR #223).
--
-- support_tickets already carries org_id + FORCE RLS org_isolation (0032/0035),
-- so the new columns inherit the row policy; no new policy on that table.

-- console-gate: audited-table support_tickets
ALTER TABLE support_tickets
    ADD COLUMN site_id       UUID REFERENCES registry_sites(id)     ON DELETE RESTRICT,
    ADD COLUMN customer_id   UUID REFERENCES registry_customers(id) ON DELETE RESTRICT,
    ADD COLUMN work_order_id UUID REFERENCES work_orders(id)        ON DELETE RESTRICT;

-- A work-order link presupposes the site link (a visit is dispatched to the
-- ticket's site); enforced app-side too (409), the constraint is the backstop.
ALTER TABLE support_tickets
    ADD CONSTRAINT support_tickets_wo_requires_site
        CHECK (work_order_id IS NULL OR site_id IS NOT NULL);

-- Field console per-site queue + site filter on the ticket list.
CREATE INDEX idx_support_tickets_site
    ON support_tickets (site_id, status, created_at DESC)
    WHERE site_id IS NOT NULL;

-- Customer acceptance: append-only closure evidence per ticket.
-- Full tenant table born post-multi-tenant: org_id + FORCE RLS + immutable-org
-- trigger + composite (id, org_id) key inline (0042 pattern), and explicit
-- console_rt grants (0058 lesson: RLS is meaningless if the runtime role has no
-- table privilege — verify as console_rt, not superuser).
--
-- idempotency_key + request_fingerprint mirror the logistics-pilot receipt
-- semantics (0179): a replay with the same key and fingerprint returns the
-- stored acceptance; a reuse with a different request is a 409.

-- console-gate: audited-table support_ticket_acceptances
CREATE TABLE support_ticket_acceptances (
    id                  UUID        NOT NULL DEFAULT gen_random_uuid(),
    org_id              UUID        NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    ticket_id           UUID        NOT NULL REFERENCES support_tickets(id) ON DELETE RESTRICT,
    kind                TEXT        NOT NULL CHECK (kind IN ('CUSTOMER_ACCEPTED', 'CUSTOMER_DECLINED')),
    channel             TEXT        NOT NULL CHECK (channel IN ('IN_PERSON', 'PHONE', 'EMAIL', 'MESSENGER')),
    -- Customer-side acknowledger; business fact like requester_name (never logged).
    accepted_by         TEXT        NOT NULL CHECK (btrim(accepted_by) <> '' AND char_length(accepted_by) <= 200),
    note                TEXT        CHECK (note IS NULL OR char_length(note) <= 2000),
    recorded_by_user_id UUID        NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    occurred_at         TIMESTAMPTZ NOT NULL,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    idempotency_key     TEXT        NOT NULL CHECK (char_length(btrim(idempotency_key)) BETWEEN 16 AND 200),
    request_fingerprint TEXT        NOT NULL CHECK (request_fingerprint ~ '^[a-f0-9]{64}$'),
    PRIMARY KEY (id, org_id),
    UNIQUE (org_id, idempotency_key)
);

ALTER TABLE support_ticket_acceptances ENABLE ROW LEVEL SECURITY;
ALTER TABLE support_ticket_acceptances FORCE ROW LEVEL SECURITY;
CREATE POLICY org_isolation ON support_ticket_acceptances
    USING (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid)
    WITH CHECK (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid);
CREATE TRIGGER trg_support_ticket_acceptances_org_immutable
    BEFORE UPDATE ON support_ticket_acceptances
    FOR EACH ROW EXECUTE FUNCTION enforce_org_id_immutable();

CREATE INDEX idx_support_ticket_acceptances_ticket
    ON support_ticket_acceptances (ticket_id, occurred_at DESC);

-- Append-only evidence: the runtime role may read and insert, never mutate or
-- erase (HANDOFF §20: hard delete forbidden).
GRANT SELECT, INSERT ON support_ticket_acceptances TO console_rt;
REVOKE UPDATE, DELETE ON support_ticket_acceptances FROM console_rt;
