-- Audit context enrichment for issue #314 / B17.
--
-- All columns are nullable for backward compatibility with existing append-only
-- rows and with platform/system events that do not have request context.

ALTER TABLE audit_events
    ADD COLUMN ip TEXT,
    ADD COLUMN user_agent TEXT,
    ADD COLUMN auth_method TEXT,
    ADD COLUMN device TEXT,
    ADD COLUMN classification_badges TEXT[],
    ADD COLUMN anomaly BOOLEAN,
    ADD COLUMN reason TEXT;
