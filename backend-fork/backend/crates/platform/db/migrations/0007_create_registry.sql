-- T1.1 registry: equipment master, customers, and sites.
-- The master-list importer creates an HQ branch if roster provisioning has not
-- established one yet, then assigns imported equipment to that branch.

CREATE TABLE registry_customers (
    id         UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    branch_id  UUID        NOT NULL REFERENCES branches(id) ON DELETE RESTRICT,
    name       TEXT        NOT NULL CHECK (name <> ''),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (branch_id, name)
);

CREATE INDEX idx_registry_customers_branch
    ON registry_customers (branch_id, name);

CREATE TABLE registry_sites (
    id          UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    branch_id   UUID        NOT NULL REFERENCES branches(id) ON DELETE RESTRICT,
    customer_id UUID        NOT NULL REFERENCES registry_customers(id) ON DELETE RESTRICT,
    name        TEXT        NOT NULL CHECK (name <> ''),
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (branch_id, customer_id, name)
);

CREATE INDEX idx_registry_sites_branch_customer
    ON registry_sites (branch_id, customer_id, name);

CREATE TABLE registry_equipment (
    id                      UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    branch_id               UUID        NOT NULL REFERENCES branches(id) ON DELETE RESTRICT,
    customer_id             UUID        NOT NULL REFERENCES registry_customers(id) ON DELETE RESTRICT,
    site_id                 UUID        NOT NULL REFERENCES registry_sites(id) ON DELETE RESTRICT,
    equipment_no            TEXT        NOT NULL UNIQUE CHECK (equipment_no ~ '^[A-Z]{3}[A-Z0-9]{2}-[0-9]{4}$'),
    management_no           TEXT,
    manufacturer_code       TEXT        NOT NULL CHECK (manufacturer_code <> ''),
    kind_code               TEXT        NOT NULL CHECK (kind_code <> ''),
    power_code              TEXT        NOT NULL CHECK (power_code <> ''),
    power_label             TEXT,
    status                  TEXT        NOT NULL CHECK (status IN ('임대', '예비', '폐기', '대체', '매각')),
    manager_name            TEXT,
    placement_location      TEXT,
    placement_no            TEXT,
    operation_shift         TEXT,
    specification           TEXT        NOT NULL CHECK (specification <> ''),
    ton_text                TEXT        NOT NULL CHECK (ton_text <> ''),
    ton_milli               INTEGER,
    maker                   TEXT,
    model                   TEXT,
    vin                     TEXT,
    year                    DATE,
    hours                   BIGINT,
    vehicle_registration_no TEXT,
    insured                 BOOLEAN,
    insurer                 TEXT,
    policy_holder           TEXT,
    insured_party           TEXT,
    asset_owner             TEXT,
    asset_registered_on     DATE,
    rental_started_on       DATE,
    rental_fee              BIGINT,
    vehicle_value           BIGINT,
    residual_value          BIGINT,
    note                    TEXT,
    source_sheet            TEXT        NOT NULL,
    source_row              INTEGER     NOT NULL,
    created_at              TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at              TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX idx_registry_equipment_branch_management_no
    ON registry_equipment (branch_id, management_no)
    WHERE management_no IS NOT NULL;

CREATE INDEX idx_registry_equipment_site
    ON registry_equipment (site_id);

CREATE INDEX idx_registry_equipment_status_rented
    ON registry_equipment (branch_id, site_id)
    WHERE status = '임대';

CREATE INDEX idx_registry_equipment_status_spare
    ON registry_equipment (branch_id, ton_milli, specification, power_code)
    WHERE status = '예비';

CREATE INDEX idx_registry_equipment_status_replacement
    ON registry_equipment (branch_id, ton_milli, specification, power_code)
    WHERE status = '대체';
