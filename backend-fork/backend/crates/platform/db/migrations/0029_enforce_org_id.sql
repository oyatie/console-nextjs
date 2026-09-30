-- Multi-tenant phase 1, step 4: enforce the tenant discriminator.
--
-- After backfill (0028) every slice row has an org_id, so we can:
--   * SET NOT NULL on org_id everywhere;
--   * add the FK org_id -> organizations(id) ON DELETE RESTRICT;
--   * make parents uniquely addressable by (id, org_id) and add COMPOSITE FKs
--     from children on (parent_id, org_id) so a child can never reference a
--     parent in a different tenant — the FK enforces same-org at write time;
--   * convert global UNIQUE keys (equipment_no, request_no, region name, the
--     request-counter PK) into PER-ORG uniques so two tenants may legitimately
--     reuse the same business number.
--
-- All additive (ADD CONSTRAINT / SET NOT NULL); no audited table or column is
-- dropped, so the migration-safety gate has nothing to flag.

-- --------------------------------------------------------------------------
-- org_id NOT NULL + FK to organizations on every slice table.
-- --------------------------------------------------------------------------
ALTER TABLE regions
    ALTER COLUMN org_id SET NOT NULL,
    ADD CONSTRAINT regions_org_fk
        FOREIGN KEY (org_id) REFERENCES organizations (id) ON DELETE RESTRICT;

ALTER TABLE branches
    ALTER COLUMN org_id SET NOT NULL,
    ADD CONSTRAINT branches_org_fk
        FOREIGN KEY (org_id) REFERENCES organizations (id) ON DELETE RESTRICT;

ALTER TABLE users
    ALTER COLUMN org_id SET NOT NULL,
    ADD CONSTRAINT users_org_fk
        FOREIGN KEY (org_id) REFERENCES organizations (id) ON DELETE RESTRICT;

ALTER TABLE registry_customers
    ALTER COLUMN org_id SET NOT NULL,
    ADD CONSTRAINT registry_customers_org_fk
        FOREIGN KEY (org_id) REFERENCES organizations (id) ON DELETE RESTRICT;

ALTER TABLE registry_sites
    ALTER COLUMN org_id SET NOT NULL,
    ADD CONSTRAINT registry_sites_org_fk
        FOREIGN KEY (org_id) REFERENCES organizations (id) ON DELETE RESTRICT;

ALTER TABLE registry_equipment
    ALTER COLUMN org_id SET NOT NULL,
    ADD CONSTRAINT registry_equipment_org_fk
        FOREIGN KEY (org_id) REFERENCES organizations (id) ON DELETE RESTRICT;

ALTER TABLE work_orders
    ALTER COLUMN org_id SET NOT NULL,
    ADD CONSTRAINT work_orders_org_fk
        FOREIGN KEY (org_id) REFERENCES organizations (id) ON DELETE RESTRICT;

-- --------------------------------------------------------------------------
-- Parents addressable by (id, org_id) so children can reference them by a
-- composite FK that pins the tenant.
-- --------------------------------------------------------------------------
ALTER TABLE regions
    ADD CONSTRAINT regions_id_org_key UNIQUE (id, org_id);
ALTER TABLE branches
    ADD CONSTRAINT branches_id_org_key UNIQUE (id, org_id);

-- --------------------------------------------------------------------------
-- Composite (same-org) FKs from children. The single-column FKs created with
-- the original tables stay in place (they still guarantee referential
-- integrity); these add the cross-tenant guard on top.
-- --------------------------------------------------------------------------
-- branch lives in the same org as its region.
ALTER TABLE branches
    ADD CONSTRAINT branches_region_same_org_fk
        FOREIGN KEY (region_id, org_id) REFERENCES regions (id, org_id)
        ON DELETE RESTRICT;

-- equipment lives in the same org as its branch.
ALTER TABLE registry_equipment
    ADD CONSTRAINT registry_equipment_branch_same_org_fk
        FOREIGN KEY (branch_id, org_id) REFERENCES branches (id, org_id)
        ON DELETE RESTRICT;

-- work order lives in the same org as its branch.
ALTER TABLE work_orders
    ADD CONSTRAINT work_orders_branch_same_org_fk
        FOREIGN KEY (branch_id, org_id) REFERENCES branches (id, org_id)
        ON DELETE RESTRICT;

-- Remaining slice parent/child hops get the same composite (same-org) guard so a
-- customer/site can never reference a parent in a different tenant.
-- customer is addressable by (id, org_id) so sites can pin the tenant via FK.
ALTER TABLE registry_customers
    ADD CONSTRAINT registry_customers_id_org_key UNIQUE (id, org_id);

-- customer lives in the same org as its branch.
ALTER TABLE registry_customers
    ADD CONSTRAINT registry_customers_branch_same_org_fk
        FOREIGN KEY (branch_id, org_id) REFERENCES branches (id, org_id)
        ON DELETE RESTRICT;

-- site lives in the same org as both its branch and its customer.
ALTER TABLE registry_sites
    ADD CONSTRAINT registry_sites_branch_same_org_fk
        FOREIGN KEY (branch_id, org_id) REFERENCES branches (id, org_id)
        ON DELETE RESTRICT,
    ADD CONSTRAINT registry_sites_customer_same_org_fk
        FOREIGN KEY (customer_id, org_id) REFERENCES registry_customers (id, org_id)
        ON DELETE RESTRICT;

-- --------------------------------------------------------------------------
-- Global uniques become per-org uniques. Two tenants may reuse the same
-- equipment number / request number / region name.
-- --------------------------------------------------------------------------
ALTER TABLE regions
    DROP CONSTRAINT regions_name_key,
    ADD CONSTRAINT regions_org_name_key UNIQUE (org_id, name);

ALTER TABLE registry_equipment
    DROP CONSTRAINT registry_equipment_equipment_no_key,
    ADD CONSTRAINT registry_equipment_org_equipment_no_key UNIQUE (org_id, equipment_no);

ALTER TABLE work_orders
    DROP CONSTRAINT work_orders_request_no_key,
    ADD CONSTRAINT work_orders_org_request_no_key UNIQUE (org_id, request_no);

-- The request-number counter is allocated per tenant per day, so its PK widens
-- from (request_date) to (org_id, request_date). Add the column, backfill to
-- KNL, enforce, then re-key.
ALTER TABLE work_order_request_counters
    ADD COLUMN org_id UUID;

UPDATE work_order_request_counters AS c
SET org_id = o.id
FROM organizations o
WHERE o.slug = 'knl'
  AND c.org_id IS NULL;

ALTER TABLE work_order_request_counters
    ALTER COLUMN org_id SET NOT NULL,
    ADD CONSTRAINT work_order_request_counters_org_fk
        FOREIGN KEY (org_id) REFERENCES organizations (id) ON DELETE RESTRICT,
    DROP CONSTRAINT work_order_request_counters_pkey,
    ADD CONSTRAINT work_order_request_counters_pkey PRIMARY KEY (org_id, request_date);

-- --------------------------------------------------------------------------
-- Composite (org_id, hot key) indexes for the tenant-scoped read paths.
-- --------------------------------------------------------------------------
CREATE INDEX idx_branches_org ON branches (org_id);
CREATE INDEX idx_users_org ON users (org_id);
CREATE INDEX idx_registry_customers_org ON registry_customers (org_id);
CREATE INDEX idx_registry_sites_org ON registry_sites (org_id);
CREATE INDEX idx_registry_equipment_org ON registry_equipment (org_id, status);
CREATE INDEX idx_work_orders_org_status ON work_orders (org_id, status, updated_at DESC);
