-- Issue #13 — direct customer/site creation: bound the customer/site name length.
--
-- registry_customers.name and registry_sites.name were only constrained to be
-- non-empty (`name <> ''`, migration 0007). The bulk importer derives names from
-- a curated workbook, so an unbounded name never mattered. The new direct-create
-- endpoints (POST /api/v1/customers, POST /api/v1/sites) accept admin-typed names,
-- so add an UPPER length bound as defense-in-depth, mirroring the bounded-text
-- CHECKs on the site address/contact columns (migrations 0039/0040). The REST
-- handler bounds the same length in the domain layer
-- (crates/kernel/core/src/validation.rs: CUSTOMER_SITE_NAME_MAX_CHARS = 200) so an
-- over-long value is a 422 at the edge; this constraint is the backstop if a
-- future code path ever writes a name without going through that validation.
--
-- char_length() counts Unicode code points, matching the Rust `str::chars().count()`
-- bound exactly. Both tables already carry org_id, a FORCE-ROW-LEVEL-SECURITY
-- `org_isolation` policy (0030), the immutable-org trigger (0031), and their
-- composite keys (0029/0034); this migration adds NO new policy, trigger, or
-- column — only the two length CHECKs.
--
-- console-gate: audited-table registry_customers
-- console-gate: audited-table registry_sites
ALTER TABLE registry_customers
    ADD CONSTRAINT registry_customers_name_max_chars
        CHECK (char_length(name) <= 200);

ALTER TABLE registry_sites
    ADD CONSTRAINT registry_sites_name_max_chars
        CHECK (char_length(name) <= 200);
