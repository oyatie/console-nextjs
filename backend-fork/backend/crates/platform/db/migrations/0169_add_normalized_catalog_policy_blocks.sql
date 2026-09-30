-- Live object-policy reads lower the validated no-code blocks stored with the
-- enforced catalog entry. Keeping this immutable, structured source alongside
-- the generated Cedar text lets list filtering fail closed when it cannot prove
-- an SQL-equivalent residual.
--
-- Rollout protocol:
--   1. add the nullable column and record existing live rows as preflight
--      blockers; their legacy selectors are not proof of a canonical no-code
--      policy;
--   2. backfill each blocker only through a reviewed producer that has run the
--      authoritative validator and supplied its canonical normalized row;
--   3. validate the constraint in a later migration only after operations has
--      resolved the queue.
-- The runtime loader independently revalidates every row and fails closed on a
-- missing row, so this migration preserves safety without inventing policy
-- content or changing live enforcement during an upgrade.
ALTER TABLE cedar_policy_catalog_entries
    ADD COLUMN normalized_row JSONB NULL
        CHECK (normalized_row IS NULL OR jsonb_typeof(normalized_row) = 'object');

CREATE TABLE cedar_policy_catalog_normalization_blockers (
    id                UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id            UUID        NOT NULL,
    catalog_entry_id  UUID        NOT NULL,
    prior_status      TEXT        NOT NULL CHECK (prior_status IN ('enforced', 'shadow')),
    reason            TEXT        NOT NULL,
    recorded_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    resolved_at       TIMESTAMPTZ NULL,
    resolution_note   TEXT        NULL,
    UNIQUE (org_id, catalog_entry_id)
);

-- Preflight/reconciliation queue. Even a legacy row that resembles the
-- envelope is not backfilled here: the database cannot establish that it is
-- canonical or that its selectors pass the current authoring schema.
INSERT INTO cedar_policy_catalog_normalization_blockers
    (org_id, catalog_entry_id, prior_status, reason)
SELECT org_id,
       id,
       status,
       'missing a reviewed canonical no-code normalized_row; runtime fails closed pending remediation'
FROM cedar_policy_catalog_entries
WHERE status IN ('enforced', 'shadow') AND normalized_row IS NULL;

ALTER TABLE cedar_policy_catalog_entries
    ADD CONSTRAINT cedar_policy_catalog_enforced_normalized_row
    CHECK (status NOT IN ('enforced', 'shadow') OR normalized_row IS NOT NULL)
    NOT VALID;
