-- Regions and branches: top-level organizational units.
-- Every operational row (work orders, users, equipment, KPIs, chat channels)
-- carries a non-null branch_id — see plan §2.3 branch-scoped authorization.

CREATE TABLE regions (
    id         UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    name       TEXT        NOT NULL UNIQUE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE branches (
    id         UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    region_id  UUID        NOT NULL REFERENCES regions(id),
    name       TEXT        NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (region_id, name)
);
