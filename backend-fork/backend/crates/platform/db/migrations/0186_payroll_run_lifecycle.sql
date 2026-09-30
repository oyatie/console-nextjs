-- CAP-PAYROLL-CONSOLE: payroll run lifecycle (close → calculate → exception
-- review → SoD approval → operator-attested disbursement → gated payslip
-- issuance). Extends the 0074 readiness staging tables without touching their
-- readiness-only contract: draft lines still store no won amount; all computed
-- draft math lives in the new append-only, versioned
-- `payroll_line_calculations` and stays `payable = FALSE` until the
-- 노무사/세무사 release gate (console_payroll_domain::validate_release_gate) passes.
--
-- NOTE: migration number is provisional (lane rule) — the consolidation
-- integrator renumbers on merge if another lane claimed 0186 first.

-- ---------------------------------------------------------------------------
-- 1. Run lifecycle columns + widened FSM (every 0074 state stays valid).
-- ---------------------------------------------------------------------------
ALTER TABLE payroll_draft_runs
    ADD COLUMN close_receipt    JSONB NULL,
    ADD COLUMN submitted_by     UUID NULL REFERENCES users(id) ON DELETE RESTRICT,
    ADD COLUMN submitted_at     TIMESTAMPTZ NULL,
    ADD COLUMN decided_by       UUID NULL REFERENCES users(id) ON DELETE RESTRICT,
    ADD COLUMN decided_at       TIMESTAMPTZ NULL,
    ADD COLUMN decision_reason  TEXT NULL,
    ADD COLUMN approval_ref     UUID NULL;  -- future AP- workflow-engine object

ALTER TABLE payroll_draft_runs DROP CONSTRAINT payroll_draft_runs_status_check;
ALTER TABLE payroll_draft_runs ADD CONSTRAINT payroll_draft_runs_status_check CHECK (status IN
  ('STAGED','BLOCKED_LEGAL_GATE','READY_FOR_REVIEW','ATTENDANCE_CLOSED','CALCULATING',
   'CALCULATED','SUBMITTED','REJECTED','APPROVED','DISBURSEMENT_SCHEDULED','PAID','ISSUED','VOID'));

-- 0074's `calculation_enabled` gate must recognize the new lifecycle states,
-- or a run with the flag set could never advance past ATTENDANCE_CLOSED.
ALTER TABLE payroll_draft_runs DROP CONSTRAINT payroll_draft_runs_enabled_requires_review;
ALTER TABLE payroll_draft_runs ADD CONSTRAINT payroll_draft_runs_enabled_requires_review
  CHECK (calculation_enabled = FALSE OR status IN
    ('READY_FOR_REVIEW','ATTENDANCE_CLOSED','CALCULATING','CALCULATED','SUBMITTED',
     'REJECTED','APPROVED','DISBURSEMENT_SCHEDULED','PAID','ISSUED'));

-- Maker–checker separation of duties: the decider is never the submitter.
ALTER TABLE payroll_draft_runs ADD CONSTRAINT payroll_draft_runs_sod
  CHECK (decided_by IS NULL OR submitted_by IS NULL OR decided_by <> submitted_by);

-- ---------------------------------------------------------------------------
-- 2. Versioned draft calculations (append-only rows; only the `payable`
--    release-gate flip is a legal UPDATE).
-- ---------------------------------------------------------------------------
CREATE TABLE payroll_line_calculations (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id UUID NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    run_id UUID NOT NULL,
    line_id UUID NOT NULL REFERENCES payroll_draft_lines(id) ON DELETE CASCADE,
    version INTEGER NOT NULL CHECK (version >= 1),
    gross_won BIGINT NOT NULL CHECK (gross_won >= 0),
    -- [{code,label_ko,amount_won,source_url}] — every deduction carries its
    -- official source; income tax amounts come from the verified NTS source
    -- row, never estimated.
    deductions JSONB NOT NULL,
    total_deductions_won BIGINT NOT NULL CHECK (total_deductions_won >= 0),
    net_won BIGINT NOT NULL,
    tax_table_version TEXT NOT NULL CHECK (btrim(tax_table_version) <> ''),
    payable BOOLEAN NOT NULL DEFAULT FALSE,  -- true only after release-gate pass
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (org_id, line_id, version),
    FOREIGN KEY (run_id, org_id) REFERENCES payroll_draft_runs(id, org_id) ON DELETE CASCADE
);
CREATE INDEX payroll_line_calculations_run_idx
    ON payroll_line_calculations (org_id, run_id, line_id, version DESC);

-- ---------------------------------------------------------------------------
-- 3. Exception review queue (예외 검토). Produced by the calculate step from
--    real line/attendance signals, resolved CONFIRM/HOLD; HELD rows carry
--    forward to the next chronological run.
-- ---------------------------------------------------------------------------
CREATE TABLE payroll_run_exceptions (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id UUID NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    run_id UUID NOT NULL,
    line_id UUID NULL REFERENCES payroll_draft_lines(id) ON DELETE CASCADE,
    employee_id UUID NULL,
    employee_display_name TEXT NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('OVERTIME_ALLOWANCE','RETRO_ADJUSTMENT',
        'ABSENCE_DEDUCTION','PRORATION','ACCOUNT_VERIFICATION')),
    severity TEXT NOT NULL CHECK (severity IN ('info','warn','danger')),
    amount_delta_won BIGINT NULL,  -- NULL when not derivable from a verified source
    summary_ko TEXT NOT NULL CHECK (btrim(summary_ko) <> ''),
    detail JSONB NOT NULL DEFAULT '{}'::jsonb,
    linked_refs JSONB NOT NULL DEFAULT '[]'::jsonb,
    status TEXT NOT NULL DEFAULT 'OPEN' CHECK (status IN ('OPEN','CONFIRMED','HELD')),
    resolved_by UUID NULL REFERENCES users(id) ON DELETE RESTRICT,
    resolved_at TIMESTAMPTZ NULL,
    resolved_reason TEXT NULL,
    carried_from_run_id UUID NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT payroll_run_exceptions_resolution CHECK
      (status = 'OPEN' OR (resolved_by IS NOT NULL AND resolved_at IS NOT NULL)),
    CONSTRAINT payroll_run_exceptions_hold_reason CHECK
      (status <> 'HELD' OR btrim(coalesce(resolved_reason, '')) <> ''),
    FOREIGN KEY (run_id, org_id) REFERENCES payroll_draft_runs(id, org_id) ON DELETE CASCADE
);
CREATE INDEX payroll_run_exceptions_run_status_idx
    ON payroll_run_exceptions (org_id, run_id, status, severity, created_at);
-- Idempotent regeneration: one live exception per (run, line, kind).
CREATE UNIQUE INDEX payroll_run_exceptions_line_kind_uq
    ON payroll_run_exceptions (org_id, run_id, line_id, kind)
    WHERE line_id IS NOT NULL;

-- ---------------------------------------------------------------------------
-- 4. Disbursement record (no bank API exists — statuses are truthful
--    operator attestations, never simulated bank results).
-- ---------------------------------------------------------------------------
CREATE TABLE payroll_disbursements (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id UUID NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    run_id UUID NOT NULL UNIQUE,
    scheduled_at TIMESTAMPTZ NOT NULL,
    status TEXT NOT NULL DEFAULT 'SCHEDULED'
        CHECK (status IN ('SCHEDULED','SUBMITTED_TO_BANK','PAID','FAILED')),
    attested_by UUID NULL REFERENCES users(id) ON DELETE RESTRICT,
    attested_at TIMESTAMPTZ NULL,
    reason TEXT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT payroll_disbursements_failed_reason
        CHECK (status <> 'FAILED' OR btrim(coalesce(reason, '')) <> ''),
    FOREIGN KEY (run_id, org_id) REFERENCES payroll_draft_runs(id, org_id) ON DELETE CASCADE
);
CREATE INDEX payroll_disbursements_org_status_idx
    ON payroll_disbursements (org_id, status, scheduled_at);

-- ---------------------------------------------------------------------------
-- 5. Payslip delivery links (run line → personal inbox doc). Acknowledgement
--    is read back from inbox_docs.confirmed_at — no duplicated ack column.
-- ---------------------------------------------------------------------------
CREATE TABLE payroll_payslip_deliveries (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id UUID NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    run_id UUID NOT NULL,
    line_id UUID NOT NULL REFERENCES payroll_draft_lines(id) ON DELETE CASCADE,
    employee_id UUID NOT NULL REFERENCES employees(id) ON DELETE RESTRICT,
    inbox_doc_id UUID NOT NULL,
    issued_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (org_id, run_id, line_id),
    FOREIGN KEY (run_id, org_id) REFERENCES payroll_draft_runs(id, org_id) ON DELETE CASCADE,
    FOREIGN KEY (inbox_doc_id, org_id) REFERENCES inbox_docs(id, org_id) ON DELETE RESTRICT
);
CREATE INDEX payroll_payslip_deliveries_run_idx
    ON payroll_payslip_deliveries (org_id, run_id, issued_at);

-- ---------------------------------------------------------------------------
-- 6. RLS (FORCE) + runtime-role grants — the 0074 block, verbatim shape.
--    No DELETE grant anywhere: lifecycle rows are evidence, never hard-deleted.
-- ---------------------------------------------------------------------------
DO $$
DECLARE
    t TEXT;
    tenant_tables TEXT[] := ARRAY[
        'payroll_line_calculations',
        'payroll_run_exceptions',
        'payroll_disbursements',
        'payroll_payslip_deliveries'
    ];
BEGIN
    FOREACH t IN ARRAY tenant_tables LOOP
        EXECUTE format('ALTER TABLE %I ENABLE ROW LEVEL SECURITY', t);
        EXECUTE format('ALTER TABLE %I FORCE ROW LEVEL SECURITY', t);
        EXECUTE format(
            'CREATE POLICY org_isolation ON %I '
            || 'USING (org_id = NULLIF(current_setting(''app.current_org'', true), '''')::uuid) '
            || 'WITH CHECK (org_id = NULLIF(current_setting(''app.current_org'', true), '''')::uuid)',
            t
        );
    END LOOP;
END
$$;

GRANT SELECT, INSERT, UPDATE ON payroll_line_calculations TO console_rt;
GRANT SELECT, INSERT, UPDATE ON payroll_run_exceptions TO console_rt;
GRANT SELECT, INSERT, UPDATE ON payroll_disbursements TO console_rt;
GRANT SELECT, INSERT, UPDATE ON payroll_payslip_deliveries TO console_rt;
