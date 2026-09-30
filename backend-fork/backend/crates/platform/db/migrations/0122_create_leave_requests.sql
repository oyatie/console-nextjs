-- Leave-request queue + decisions (연차/반차 신청·결재).
--
-- The request/decide layer over the existing leave ledger: the aggregate
-- `employees.leave_accrued/used/remaining` columns (0066) remain the source of
-- truth for balances, and an APPROVAL writes that ledger in the SAME audited
-- transaction as the status change (see the adapter). This table is NOT a
-- second balance store — it is the workflow of moving a request through
-- pending → approved/returned/rejected.
--
-- Branch-scoped: the approval queue is confined to the approver's branches in
-- application code from the resolved principal scope (there is no per-branch
-- GUC). Tenant isolation follows the 0118_create_inbox_docs idiom: RLS keyed on
-- `app.current_org`, FORCE-enabled.
--
-- Separation of duties is enforced BOTH in code and here: `decided_by` can
-- never equal `requester_user_id`, so a bug cannot stamp a self-approval.

-- console-gate: audited-table leave_requests
CREATE TABLE leave_requests (
    id                  UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id              UUID        NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    -- The branch the request is routed to for approval (queue scoping).
    branch_id           UUID        NOT NULL,
    -- The principal who filed the request (SoD subject).
    requester_user_id   UUID        NOT NULL,
    -- The employee whose leave balance an approval moves.
    subject_employee_id UUID        NOT NULL,
    leave_type          TEXT        NOT NULL
        CHECK (leave_type IN ('annual', 'half_day')),
    -- Requested days; a 반차 is 0.5. NUMERIC(4,1) covers a full fiscal year.
    days                NUMERIC(4, 1) NOT NULL
        CHECK (days > 0 AND days <= 366),
    start_date          DATE        NOT NULL,
    end_date            DATE        NOT NULL,
    reason              TEXT        NOT NULL
        CHECK (char_length(btrim(reason)) BETWEEN 1 AND 500),
    status              TEXT        NOT NULL DEFAULT 'pending'
        CHECK (status IN ('pending', 'approved', 'returned', 'rejected')),
    -- Decision stamp. NULL until decided; set atomically with decided_at.
    decided_by          UUID,
    decided_at          TIMESTAMPTZ,
    decision_comment    TEXT
        CHECK (decision_comment IS NULL
               OR char_length(btrim(decision_comment)) BETWEEN 1 AND 500),
    -- The engine AP- run started for this request, when the 연차촉진 / 기안
    -- submittable definition exists (gap #1). Soft reference; NULL until then.
    ap_run_id           UUID,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK (end_date >= start_date),
    UNIQUE (id, org_id),
    FOREIGN KEY (branch_id, org_id) REFERENCES branches (id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (requester_user_id, org_id) REFERENCES users (id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (subject_employee_id, org_id) REFERENCES employees (id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (decided_by, org_id) REFERENCES users (id, org_id) ON DELETE RESTRICT,
    -- Decision stamp is atomic.
    CONSTRAINT leave_requests_decision_atomic
        CHECK ((decided_by IS NULL) = (decided_at IS NULL)),
    -- A pending request has no decider; a decided request has one.
    CONSTRAINT leave_requests_pending_iff_undecided
        CHECK ((status = 'pending') = (decided_by IS NULL)),
    -- Separation of duties: the decider is never the requester.
    CONSTRAINT leave_requests_sod
        CHECK (decided_by IS NULL OR decided_by <> requester_user_id)
);

ALTER TABLE leave_requests ENABLE ROW LEVEL SECURITY;
ALTER TABLE leave_requests FORCE ROW LEVEL SECURITY;
CREATE POLICY org_isolation ON leave_requests
    USING (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid)
    WITH CHECK (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid);

-- Backs the branch-scoped queue: pending-first, newest-first within a branch.
CREATE INDEX leave_requests_org_branch_status_idx
    ON leave_requests (org_id, branch_id, status, created_at DESC);

-- Leave requests are approval evidence; the runtime role never hard-deletes.
GRANT SELECT, INSERT, UPDATE ON leave_requests TO console_rt;
REVOKE DELETE ON leave_requests FROM console_rt;
