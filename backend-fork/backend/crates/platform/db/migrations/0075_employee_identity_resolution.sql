-- Employee identity standardization: distinguish confirmed identifiers from
-- review-required source-row records. This is intentionally additive; it does
-- not delete, merge, or link employees/users by name.

ALTER TABLE employees
    ADD COLUMN identity_resolution_strategy TEXT NOT NULL DEFAULT 'source_row_fingerprint'
        CHECK (identity_resolution_strategy IN (
            'employee_number',
            'legal_identifier_hash',
            'birth_hire_fingerprint',
            'source_row_fingerprint'
        )),
    ADD COLUMN identity_resolution_confidence TEXT NOT NULL DEFAULT 'low'
        CHECK (identity_resolution_confidence IN ('high', 'medium', 'low')),
    ADD COLUMN identity_review_required BOOLEAN NOT NULL DEFAULT TRUE,
    ADD COLUMN identity_name_only_merge BOOLEAN NOT NULL DEFAULT FALSE
        CHECK (identity_name_only_merge = FALSE);

UPDATE employees
SET
    identity_resolution_strategy = CASE source_metadata #>> '{identity_resolution,strategy}'
        WHEN 'employee_number' THEN 'employee_number'
        WHEN 'legal_identifier_hash' THEN 'legal_identifier_hash'
        WHEN 'birth_hire_fingerprint' THEN 'birth_hire_fingerprint'
        WHEN 'source_row_fingerprint' THEN 'source_row_fingerprint'
        ELSE 'source_row_fingerprint'
    END,
    identity_resolution_confidence = CASE source_metadata #>> '{identity_resolution,strategy}'
        WHEN 'employee_number' THEN 'high'
        WHEN 'legal_identifier_hash' THEN 'high'
        WHEN 'birth_hire_fingerprint' THEN 'medium'
        ELSE 'low'
    END,
    identity_review_required = CASE
        WHEN source_metadata #>> '{identity_resolution,manual_review_required}' = 'false'
             AND source_metadata #>> '{identity_resolution,strategy}' IN ('employee_number', 'legal_identifier_hash')
        THEN FALSE
        ELSE TRUE
    END,
    identity_name_only_merge = FALSE;

CREATE INDEX employees_identity_review_idx
    ON employees (org_id, identity_review_required, identity_resolution_strategy)
    WHERE identity_review_required;
