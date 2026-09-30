-- T7 support / help-desk domain: a support ticketing system distinct from the
-- 정비 work-order flow, with TWO intake channels (internal staff + external
-- customer) and notification fan-out on assignment / status / comment events.
--
-- Channel model:
--   * INTERNAL tickets are opened by an authenticated user and carry that
--     requester's branch (branch_id NOT NULL for internal rows).
--   * CUSTOMER tickets arrive on the unauthenticated intake endpoint and are
--     UNASSIGNED to any branch until a staff member triages them, so branch_id
--     is NULLABLE. They capture a free-form requester name and contact string.
--
-- PII: requester_contact is customer-supplied contact PII (phone/email). It is
-- never written to logs (enforced by the pii-no-logs gate over logging macros)
-- and never copied into audit_events snapshots.

-- console-gate: audited-table support_tickets
CREATE TABLE support_tickets (
    id                 UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    -- NULLABLE: customer-intake tickets are branch-less until triaged; internal
    -- tickets carry the requester's branch.
    branch_id          UUID        REFERENCES branches(id) ON DELETE RESTRICT,
    origin             TEXT        NOT NULL CHECK (origin IN ('INTERNAL', 'CUSTOMER')),
    category           TEXT        NOT NULL CHECK (
        category IN (
            'SYSTEM_BUG', 'ACCESS_REQUEST', 'OPERATIONAL',
            'EQUIPMENT_INQUIRY', 'COMPLAINT', 'OTHER'
        )
    ),
    priority           TEXT        NOT NULL CHECK (priority IN ('LOW', 'MEDIUM', 'HIGH', 'URGENT')),
    status             TEXT        NOT NULL CHECK (
        status IN ('OPEN', 'IN_PROGRESS', 'ON_HOLD', 'RESOLVED', 'CLOSED')
    ),
    title              TEXT        NOT NULL CHECK (btrim(title) <> ''),
    body               TEXT        NOT NULL CHECK (btrim(body) <> ''),
    -- Internal channel: the authenticated requester. Customer channel: NULL.
    requester_user_id  UUID        REFERENCES users(id) ON DELETE SET NULL,
    -- Customer channel: free-form name + contact PII. Internal channel: NULL.
    requester_name     TEXT,
    requester_contact  TEXT,
    assignee_user_id   UUID        REFERENCES users(id) ON DELETE SET NULL,
    -- SLA target derived from priority on create (URGENT 4h .. LOW 7d).
    due_at             TIMESTAMPTZ,
    created_at         TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at         TIMESTAMPTZ NOT NULL DEFAULT now(),
    resolved_at        TIMESTAMPTZ,
    closed_at          TIMESTAMPTZ,
    -- Exactly one requester identity per channel.
    CHECK (
        (origin = 'INTERNAL'
            AND requester_user_id IS NOT NULL
            AND branch_id IS NOT NULL
            AND requester_name IS NULL
            AND requester_contact IS NULL)
        OR
        (origin = 'CUSTOMER'
            AND requester_user_id IS NULL
            AND requester_name IS NOT NULL
            AND requester_contact IS NOT NULL)
    )
);

-- List-by-branch filtered on status (the primary staff queue view).
CREATE INDEX idx_support_tickets_branch_status
    ON support_tickets (branch_id, status, created_at DESC);

-- Assignee work-queue view.
CREATE INDEX idx_support_tickets_assignee
    ON support_tickets (assignee_user_id, status, created_at DESC);

-- Untriaged customer-intake queue (branch_id IS NULL).
CREATE INDEX idx_support_tickets_untriaged
    ON support_tickets (status, created_at DESC)
    WHERE branch_id IS NULL;

-- console-gate: audited-table support_ticket_comments
CREATE TABLE support_ticket_comments (
    id               UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    ticket_id        UUID        NOT NULL REFERENCES support_tickets(id) ON DELETE RESTRICT,
    -- NULL = authored by the customer or by the system (not a staff user).
    author_user_id   UUID        REFERENCES users(id) ON DELETE SET NULL,
    body             TEXT        NOT NULL CHECK (btrim(body) <> ''),
    -- TRUE = internal-only staff note (never returned on the customer-visible
    -- path); FALSE = customer-visible reply.
    is_internal_note BOOLEAN     NOT NULL DEFAULT FALSE,
    created_at       TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX idx_support_ticket_comments_ticket
    ON support_ticket_comments (ticket_id, created_at);
