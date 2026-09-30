-- Compliance location store (ADR-0014).
--
-- Location consent lifecycle events are audited, but GPS coordinates are
-- deliberately kept out of audit_events. Pings and collection logs are
-- destructible so withdrawal can physically destroy location data and
-- collection records without touching the append-only audit table.

CREATE TABLE location_consents (
    id           UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id      UUID        NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    branch_id    UUID        NOT NULL REFERENCES branches(id) ON DELETE RESTRICT,
    status       TEXT        NOT NULL CHECK (status IN ('GRANTED', 'SUSPENDED', 'WITHDRAWN')),
    granted_at   TIMESTAMPTZ,
    suspended_at TIMESTAMPTZ,
    resumed_at   TIMESTAMPTZ,
    withdrawn_at TIMESTAMPTZ,
    updated_at   TIMESTAMPTZ NOT NULL,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (user_id)
);

CREATE INDEX idx_location_consents_branch
    ON location_consents (branch_id, status, updated_at DESC);

CREATE TABLE location_consent_ledger (
    id          UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    consent_id  UUID        NOT NULL REFERENCES location_consents(id) ON DELETE RESTRICT,
    user_id     UUID        NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    branch_id   UUID        NOT NULL REFERENCES branches(id) ON DELETE RESTRICT,
    actor       UUID        REFERENCES users(id) ON DELETE RESTRICT,
    action      TEXT        NOT NULL CHECK (
                    action IN ('consent.grant', 'consent.suspend', 'consent.resume', 'consent.withdraw')
                ),
    from_status TEXT        NOT NULL CHECK (
                    from_status IN ('NO_RECORD', 'GRANTED', 'SUSPENDED', 'WITHDRAWN')
                ),
    to_status   TEXT        NOT NULL CHECK (
                    to_status IN ('GRANTED', 'SUSPENDED', 'WITHDRAWN')
                ),
    occurred_at TIMESTAMPTZ NOT NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX idx_location_consent_ledger_user_time
    ON location_consent_ledger (user_id, occurred_at DESC);

CREATE TABLE location_pings (
    id          UUID             NOT NULL DEFAULT gen_random_uuid(),
    user_id     UUID             NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    branch_id   UUID             NOT NULL REFERENCES branches(id) ON DELETE RESTRICT,
    latitude    DOUBLE PRECISION NOT NULL CHECK (
                    latitude::text NOT IN ('NaN', 'Infinity', '-Infinity')
                    AND latitude BETWEEN -90 AND 90
                ),
    longitude   DOUBLE PRECISION NOT NULL CHECK (
                    longitude::text NOT IN ('NaN', 'Infinity', '-Infinity')
                    AND longitude BETWEEN -180 AND 180
                ),
    accuracy_m  DOUBLE PRECISION CHECK (
                    accuracy_m IS NULL
                    OR (
                        accuracy_m::text NOT IN ('NaN', 'Infinity', '-Infinity')
                        AND accuracy_m >= 0
                    )
                ),
    recorded_at TIMESTAMPTZ      NOT NULL,
    received_at TIMESTAMPTZ      NOT NULL DEFAULT now(),
    on_duty     BOOLEAN          NOT NULL,
    PRIMARY KEY (id, recorded_at)
) PARTITION BY RANGE (recorded_at);

CREATE INDEX idx_location_pings_user_time
    ON location_pings (user_id, recorded_at DESC);
CREATE INDEX idx_location_pings_branch_time
    ON location_pings (branch_id, recorded_at DESC);

CREATE TABLE location_collection_logs (
    id          UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id     UUID        NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    branch_id   UUID        NOT NULL REFERENCES branches(id) ON DELETE RESTRICT,
    ping_id     UUID        NOT NULL,
    recorded_at TIMESTAMPTZ NOT NULL,
    reason      TEXT        NOT NULL CHECK (reason <> ''),
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX idx_location_collection_logs_user_time
    ON location_collection_logs (user_id, recorded_at DESC);

CREATE OR REPLACE FUNCTION location_pings_create_day_partition(partition_day DATE)
RETURNS TEXT
LANGUAGE plpgsql
AS $$
DECLARE
    partition_name TEXT := 'location_pings_' || to_char(partition_day, 'YYYYMMDD');
    start_at TIMESTAMPTZ := partition_day::timestamp AT TIME ZONE 'UTC';
    end_at   TIMESTAMPTZ := (partition_day + 1)::timestamp AT TIME ZONE 'UTC';
BEGIN
    -- CREATE TABLE IF NOT EXISTS is not race-free for concurrent first pings.
    -- Serialize per-day partition creation while keeping different days independent.
    PERFORM pg_advisory_xact_lock(hashtextextended(partition_name, 0));

    EXECUTE format(
        'CREATE TABLE IF NOT EXISTS %I PARTITION OF location_pings FOR VALUES FROM (%L) TO (%L)',
        partition_name,
        start_at,
        end_at
    );

    RETURN partition_name;
END;
$$;

CREATE OR REPLACE FUNCTION location_pings_ensure_partition(recorded_at TIMESTAMPTZ)
RETURNS TEXT
LANGUAGE plpgsql
AS $$
DECLARE
    partition_day DATE := (recorded_at AT TIME ZONE 'UTC')::date;
BEGIN
    RETURN location_pings_create_day_partition(partition_day);
END;
$$;

CREATE OR REPLACE FUNCTION purge_expired_location_data(retain_after TIMESTAMPTZ)
RETURNS TABLE (
    dropped_ping_partitions INTEGER,
    deleted_collection_logs BIGINT
)
LANGUAGE plpgsql
AS $$
DECLARE
    cutoff_day DATE := (retain_after AT TIME ZONE 'UTC')::date;
    part RECORD;
    part_day DATE;
    deleted_logs BIGINT;
BEGIN
    DELETE FROM location_collection_logs
    WHERE recorded_at < retain_after;
    GET DIAGNOSTICS deleted_logs = ROW_COUNT;

    dropped_ping_partitions := 0;
    FOR part IN
        SELECT namespace.nspname, child.relname
        FROM pg_inherits
        JOIN pg_class parent ON pg_inherits.inhparent = parent.oid
        JOIN pg_class child ON pg_inherits.inhrelid = child.oid
        JOIN pg_namespace namespace ON child.relnamespace = namespace.oid
        WHERE parent.oid = 'location_pings'::regclass
    LOOP
        IF part.relname ~ '^location_pings_[0-9]{8}$' THEN
            part_day := to_date(right(part.relname, 8), 'YYYYMMDD');
            IF part_day < cutoff_day THEN
                EXECUTE format('DROP TABLE IF EXISTS %I.%I', part.nspname, part.relname);
                dropped_ping_partitions := dropped_ping_partitions + 1;
            END IF;
        END IF;
    END LOOP;

    DELETE FROM location_pings
    WHERE recorded_at < retain_after;

    deleted_collection_logs := deleted_logs;
    RETURN NEXT;
END;
$$;
