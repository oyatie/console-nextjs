-- T4.2/T4.3 reporting Excel exports and editable work-diary drafts.

-- console-gate: audited-table excel_export_logs
CREATE TABLE excel_export_logs (
    id           UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    actor        UUID        NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    branch_id    UUID        REFERENCES branches(id) ON DELETE RESTRICT,
    scope_key    TEXT        NOT NULL CHECK (scope_key <> ''),
    export_kind  TEXT        NOT NULL CHECK (export_kind IN ('daily_status','work_diary')),
    export_date  DATE        NOT NULL,
    file_name    TEXT        NOT NULL CHECK (file_name <> ''),
    source_notes JSONB       NOT NULL DEFAULT '[]'::jsonb,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX idx_excel_export_logs_kind_date
    ON excel_export_logs (export_kind, export_date DESC, created_at DESC);

CREATE INDEX idx_excel_export_logs_actor
    ON excel_export_logs (actor, created_at DESC);

-- console-gate: audited-table work_diary_drafts
CREATE TABLE work_diary_drafts (
    id           UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    diary_date   DATE        NOT NULL,
    branch_id    UUID        REFERENCES branches(id) ON DELETE RESTRICT,
    scope_key    TEXT        NOT NULL CHECK (scope_key <> ''),
    status       TEXT        NOT NULL CHECK (status IN ('DRAFT','CONFIRMED')),
    body         JSONB       NOT NULL,
    generated_by UUID        NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    generated_at TIMESTAMPTZ NOT NULL,
    edited_by    UUID        REFERENCES users(id) ON DELETE RESTRICT,
    edited_at    TIMESTAMPTZ,
    confirmed_by UUID        REFERENCES users(id) ON DELETE RESTRICT,
    confirmed_at TIMESTAMPTZ,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (diary_date, scope_key),
    CHECK (
        (status = 'DRAFT' AND confirmed_by IS NULL AND confirmed_at IS NULL)
        OR (status = 'CONFIRMED' AND confirmed_by IS NOT NULL AND confirmed_at IS NOT NULL)
    )
);

CREATE INDEX idx_work_diary_drafts_date
    ON work_diary_drafts (diary_date DESC, scope_key);
