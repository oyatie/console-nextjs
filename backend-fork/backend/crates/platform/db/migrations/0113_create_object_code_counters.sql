-- Shared object-code issuance (BE-OBJ slice 2, item 1).
--
-- Today the ONLY canonical code issuer is the work-order request counter
-- (work_order_request_counters, YYYYMMDD-NNN, 999/day cap). Every other domain
-- either uses UUIDs or accepts caller-supplied free text, so the frontend
-- objectRegistry fabricates prefixes (AP-, CS-, …) it cannot dereference.
--
-- This generalizes the workorder counter into ONE org-scoped, monotonic,
-- concurrency-safe issuer keyed by object kind. Each kind's canonical code
-- prefix lives on the (global) object_types registry; the per-org sequence
-- lives in object_code_counters (tenant data, FORCE RLS). Issued codes are
-- `<prefix><sequence>` (e.g. AP-1, AP-2, …): monotonic per (org, kind),
-- gap-free NOT guaranteed (a rolled-back tx burns a number, exactly like the
-- workorder counter), concurrency-safe via the same INSERT … ON CONFLICT DO
-- UPDATE … RETURNING row-lock pattern.

-- ---------------------------------------------------------------------------
-- object_types.code_prefix — canonical per-kind code prefix (global config).
-- ---------------------------------------------------------------------------
-- Uppercase letters/digits then a trailing '-' (e.g. AP-, CS-, MSG-). NULL for
-- kinds referenced by id/name rather than an issued code (person, org_unit),
-- or that already have their own canonical scheme (work_order) — matching the
-- frontend registry, which gives person/org no codePrefix either.
ALTER TABLE object_types
    ADD COLUMN code_prefix TEXT
        CHECK (code_prefix IS NULL OR code_prefix ~ '^[A-Z][A-Z0-9]*-$');

-- Backfill canonical prefixes. Overlapping kinds mirror the frontend
-- objectRegistry prefixes (CS-/AP-/PS-) so a code issued here renders
-- identically in the console; the rest are assigned canonical, non-colliding
-- prefixes for the domains that will adopt issuance as their screens ship.
-- work_order deliberately has NO row here: it already has its own canonical,
-- date-based scheme (work_order_request_counters, YYYYMMDD-NNN) — seeding a
-- WO- prefix on this sequence would invite a second, competing issuance path
-- for the same kind.
UPDATE object_types SET code_prefix = 'CS-'  WHERE kind = 'support_ticket';
UPDATE object_types SET code_prefix = 'AP-'  WHERE kind = 'approval_run';
UPDATE object_types SET code_prefix = 'PS-'  WHERE kind = 'payroll_period';
UPDATE object_types SET code_prefix = 'PR-'  WHERE kind = 'purchase_request';
UPDATE object_types SET code_prefix = 'V-'   WHERE kind = 'voucher';
UPDATE object_types SET code_prefix = 'LS-'  WHERE kind = 'listing';
UPDATE object_types SET code_prefix = 'DOC-' WHERE kind = 'document';
UPDATE object_types SET code_prefix = 'AD-'  WHERE kind = 'approval_document';
UPDATE object_types SET code_prefix = 'AX-'  WHERE kind = 'asset_transfer';
UPDATE object_types SET code_prefix = 'EQ-'  WHERE kind = 'equipment';
UPDATE object_types SET code_prefix = 'ML-'  WHERE kind = 'mail_thread';
UPDATE object_types SET code_prefix = 'MSG-' WHERE kind = 'messenger_thread';
UPDATE object_types SET code_prefix = 'NT-'  WHERE kind = 'notification';
-- person, org_unit intentionally keep code_prefix = NULL.

-- ---------------------------------------------------------------------------
-- object_code_counters — per-org, per-kind monotonic sequence (tenant data).
-- ---------------------------------------------------------------------------
-- console-gate: audited-table object_code_counters
CREATE TABLE object_code_counters (
    org_id        UUID        NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    kind          TEXT        NOT NULL REFERENCES object_types(kind) ON DELETE RESTRICT,
    last_sequence BIGINT      NOT NULL DEFAULT 0 CHECK (last_sequence >= 0),
    PRIMARY KEY (org_id, kind)
);

ALTER TABLE object_code_counters ENABLE ROW LEVEL SECURITY;
ALTER TABLE object_code_counters FORCE ROW LEVEL SECURITY;
CREATE POLICY org_isolation ON object_code_counters
    USING (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid)
    WITH CHECK (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid);

-- The counter is bumped in place (INSERT … ON CONFLICT DO UPDATE), so console_rt
-- needs INSERT + UPDATE here. It is not an append-only audit table and it is
-- never deleted (a counter only ever moves forward), so no DELETE.
GRANT SELECT, INSERT, UPDATE ON object_code_counters TO console_rt;
