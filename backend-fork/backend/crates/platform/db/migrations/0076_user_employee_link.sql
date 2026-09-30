-- Explicit 사용자 ↔ 직원 link.
--
-- The platform account (`users`) and HR employee record (`employees`) are
-- different lifecycle objects. Do not infer that link by name: live data has
-- many same-name collisions. This migration adds a nullable, same-tenant FK so
-- admins can link an account to exactly one employee record only when they have
-- a trusted identifier or have manually reviewed the row.

ALTER TABLE employees
    ADD CONSTRAINT employees_id_org_key UNIQUE (id, org_id);

ALTER TABLE users
    ADD COLUMN employee_id UUID;

ALTER TABLE users
    ADD CONSTRAINT users_employee_same_org_fk
        FOREIGN KEY (employee_id, org_id)
        REFERENCES employees (id, org_id)
        ON DELETE RESTRICT;

CREATE UNIQUE INDEX users_org_employee_unique_idx
    ON users (org_id, employee_id)
    WHERE employee_id IS NOT NULL;

CREATE INDEX users_org_employee_idx
    ON users (org_id, employee_id)
    WHERE employee_id IS NOT NULL;

-- Promote only tenant-unique non-empty employee numbers. This fixes imported
-- rows that predate the identity-resolution metadata without using names,
-- phones, or other ambiguous fields. Duplicate employee numbers remain review
-- required.
WITH normalized AS (
    SELECT
        id,
        org_id,
        NULLIF(btrim(employee_number), '') AS employee_number
    FROM employees
),
unique_employee_numbers AS (
    SELECT org_id, employee_number
    FROM normalized
    WHERE employee_number IS NOT NULL
    GROUP BY org_id, employee_number
    HAVING count(*) = 1
)
UPDATE employees e
SET
    identity_resolution_strategy = 'employee_number',
    identity_resolution_confidence = 'high',
    identity_review_required = FALSE,
    identity_name_only_merge = FALSE
FROM unique_employee_numbers unique_numbers
WHERE e.org_id = unique_numbers.org_id
  AND NULLIF(btrim(e.employee_number), '') = unique_numbers.employee_number
  AND e.identity_name_only_merge = FALSE;
