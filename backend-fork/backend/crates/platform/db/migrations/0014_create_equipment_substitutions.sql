-- T5.1 substitute-equipment assignment lifecycle.
-- Active rows (returned_at IS NULL) reserve the substitute unit so it cannot
-- be offered to another down unit until the return transition is audited.

CREATE TABLE equipment_substitutions (
    id                      UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    branch_id               UUID        NOT NULL REFERENCES branches(id) ON DELETE RESTRICT,
    source_equipment_id     UUID        NOT NULL REFERENCES registry_equipment(id) ON DELETE RESTRICT,
    substitute_equipment_id UUID        NOT NULL REFERENCES registry_equipment(id) ON DELETE RESTRICT,
    assigned_by             UUID        NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    assigned_to             UUID        REFERENCES users(id) ON DELETE RESTRICT,
    assignment_location     TEXT        NOT NULL CHECK (assignment_location <> ''),
    assigned_at             TIMESTAMPTZ NOT NULL,
    returned_by             UUID        REFERENCES users(id) ON DELETE RESTRICT,
    returned_at             TIMESTAMPTZ,
    return_note             TEXT,
    created_at              TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at              TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK (source_equipment_id <> substitute_equipment_id),
    CHECK (
        (returned_at IS NULL AND returned_by IS NULL)
        OR
        (returned_at IS NOT NULL AND returned_by IS NOT NULL)
    )
);

CREATE UNIQUE INDEX idx_equipment_substitutions_active_source
    ON equipment_substitutions (source_equipment_id)
    WHERE returned_at IS NULL;

CREATE UNIQUE INDEX idx_equipment_substitutions_active_substitute
    ON equipment_substitutions (substitute_equipment_id)
    WHERE returned_at IS NULL;

CREATE INDEX idx_equipment_substitutions_branch_active
    ON equipment_substitutions (branch_id, assigned_at DESC)
    WHERE returned_at IS NULL;

CREATE INDEX idx_equipment_substitutions_substitute_history
    ON equipment_substitutions (substitute_equipment_id, assigned_at DESC);
