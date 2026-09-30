-- T4.4 scoped KPI exclusion audit table.
-- Work orders still carry the legacy fast-path boolean `work_orders.kpi_excluded`;
-- this table records revokable WORK_ORDER/OUTSOURCE exclusions with scope.

-- console-gate: audited-table kpi_exclusions
CREATE TABLE kpi_exclusions (
    id           UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    branch_id    UUID        NOT NULL REFERENCES branches(id) ON DELETE RESTRICT,
    scope        TEXT        NOT NULL CHECK (scope IN ('WORK_ORDER','OUTSOURCE')),
    target_id    UUID        NOT NULL,
    reason       TEXT        NOT NULL CHECK (reason <> ''),
    excluded_by  UUID        NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    excluded_at  TIMESTAMPTZ NOT NULL,
    revoked_by   UUID        REFERENCES users(id) ON DELETE RESTRICT,
    revoked_at   TIMESTAMPTZ,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK (
        (revoked_at IS NULL AND revoked_by IS NULL)
        OR (revoked_at IS NOT NULL AND revoked_by IS NOT NULL)
    )
);

CREATE UNIQUE INDEX idx_kpi_exclusions_active_target
    ON kpi_exclusions (scope, target_id)
    WHERE revoked_at IS NULL;

CREATE INDEX idx_kpi_exclusions_branch_scope
    ON kpi_exclusions (branch_id, scope, excluded_at DESC);
