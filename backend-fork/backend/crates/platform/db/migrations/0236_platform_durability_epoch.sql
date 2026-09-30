-- Inert physical-writer marker. A separately witnessed/fenced coordinator must
-- install and advance this row; migration, signup, and normal runtime do not.
CREATE TABLE platform_durability_epoch (
    singleton BOOLEAN PRIMARY KEY DEFAULT TRUE CHECK (singleton),
    writer_epoch BIGINT NOT NULL CHECK (writer_epoch > 0),
    system_identifier TEXT NOT NULL CHECK (system_identifier ~ '^[0-9]+$'),
    timeline_id BIGINT NOT NULL CHECK (timeline_id BETWEEN 1 AND 4294967295),
    primary_id TEXT NOT NULL CHECK (length(primary_id) > 0)
);

REVOKE ALL ON platform_durability_epoch FROM PUBLIC, console_rt;

CREATE FUNCTION platform_durability_epoch_guard() RETURNS TRIGGER
LANGUAGE plpgsql AS $body$
BEGIN
    IF TG_OP = 'DELETE' THEN
        RAISE EXCEPTION 'platform_durability_epoch.delete_forbidden' USING ERRCODE = '42501';
    END IF;
    IF NEW.writer_epoch <= OLD.writer_epoch THEN
        RAISE EXCEPTION 'platform_durability_epoch.nonmonotonic' USING ERRCODE = '23514';
    END IF;
    RETURN NEW;
END
$body$;
REVOKE ALL ON FUNCTION platform_durability_epoch_guard() FROM PUBLIC, console_rt;

CREATE TRIGGER platform_durability_epoch_guard
    BEFORE UPDATE OR DELETE ON platform_durability_epoch
    FOR EACH ROW EXECUTE FUNCTION platform_durability_epoch_guard();

COMMENT ON COLUMN platform_durability_epoch.singleton IS 'pd:none — infrastructure singleton key';
COMMENT ON COLUMN platform_durability_epoch.writer_epoch IS 'pd:none — fenced writer generation';
COMMENT ON COLUMN platform_durability_epoch.system_identifier IS 'pd:none — PostgreSQL physical cluster identifier';
COMMENT ON COLUMN platform_durability_epoch.timeline_id IS 'pd:none — PostgreSQL WAL timeline';
COMMENT ON COLUMN platform_durability_epoch.primary_id IS 'pd:none — infrastructure node identifier';
