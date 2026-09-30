-- The THREE MISSING tables the canonical contract assigns to
-- `ObjectKey::Employment`. The fourth, `employees`, already exists (migration
-- 0063) and is NOT recreated here.
--
-- Quoted verbatim from backend/crates/ontology/canonical-domain/src/lib.rs:135-146:
--
--     /// reassignment emits canonical Employment transfer commands. Until the
--     /// `EmploymentPort` lane lands, orgchange is named here as the sole owner so
--     /// the gate still rejects any *additional* writer; that lane retargets this
--     /// entry to the canonical adapter.
--     Employment => "employment",
--         owner = "console-orgchange-adapter-postgres",
--         tables = [
--             "employees",
--             "employment_heads",
--             "employment_revisions",
--             "employment_source_bindings",
--         ];
--
-- and the doc comment immediately above it, which is the temporal contract:
--
--     /// `employees` remains the legacy compatibility head; heads/revisions carry
--     /// non-overlapping `[valid_from, valid_to)` history.
--
-- MEASURED BEFORE WRITING. `CREATE TABLE` over this directory returns exactly
-- one match for the four roster names — `0063_create_employees.sql:2`. The
-- other three have zero creators, which is the only remaining migration blocker
-- in Wave 2. Migration 0213 created the Person tables and did nothing for these.
--
-- WHY ONE WRITER CREATES THEM. Identical to 0213: migration numbers are
-- assigned at MERGE, not at authoring, so two lanes that each pick their own
-- break the contiguity check in console-gate-migration-safety. The integration
-- owner lands the tables and the Employment port lane (console-kmb) fills
-- `orgchange/adapter-postgres/src/employment.rs` against a schema that already
-- exists. Only Employment is created here. `company_revisions`, `org_units`,
-- `org_unit_revisions`, `org_unit_source_bindings`, `job_positions` and
-- `job_position_revisions` are the other lanes' rows in the same roster and are
-- deliberately absent. The six payroll_* tables of `ObjectKey::PayRun` are all
-- already present (0074 and 0186) and need nothing.
--
-- SHAPE. 0213_create_canonical_person_tables.sql is the reference — itself a
-- copy of 0177_ontology_action_command_receipts.sql — and every one of its six
-- properties is carried here:
--
--   1. PRIMARY KEY carrying `org_id`, so uniqueness is tenant-global.
--   2. FOREIGN KEY (actor_id, org_id) -> users(id, org_id), wherever a row
--      records who acted.
--   3. `octet_length(payload_digest) = 32`, wherever a row records the command
--      payload that produced it.
--   4. A BEFORE trigger that RAISEs, on the rows that are immutable.
--   5. JSONB for the replayable stored result.
--   6. RLS ENABLE **and** FORCE with the `org_isolation` policy. FORCE is the
--      load-bearing half: without it the table OWNER bypasses tenancy.
--
-- THE TRIGGER IS THE ENFORCEMENT, NOT THE GRANT. `ops/postgres-reconcile-topology.sh`
-- runs `ALTER DEFAULT PRIVILEGES FOR ROLE console_app IN SCHEMA public GRANT
-- SELECT, INSERT, UPDATE, DELETE ON TABLES TO console_rt` before migrations, so
-- the runtime role already holds UPDATE on every table this file creates and
-- the `GRANT SELECT, INSERT` lines below neither add nor subtract that. 0177 and
-- 0213 have exactly the same property; it is why immutability is a trigger and
-- not a privilege, and it is copied rather than improved on.

-- ---------------------------------------------------------------------------
-- employment_heads — the stable identity of one employment relationship.
-- ---------------------------------------------------------------------------
-- An identity anchor plus the ONE piece of state a head must carry that a
-- revision cannot: the closing bound of the employment window. Every attribute
-- of the employment lives in `employment_revisions`, so the Person-table
-- sentence applies unchanged — the port lane adds a denormalised head column if
-- it can show it needs one.
--
-- WHY `valid_to` LIVES HERE AND `valid_from` LIVES ON BOTH. The contract says
-- heads/revisions carry non-overlapping `[valid_from, valid_to)` history. On an
-- APPEND-ONLY revision table a stored per-revision `valid_to` is unwritable:
-- closing revision N when N+1 arrives is exactly the UPDATE the immutability
-- trigger below refuses. So non-overlap is made STRUCTURAL instead of
-- constrained — each revision stores only its own `valid_from`, unique per
-- employment, and its interval ends where the next revision's `valid_from`
-- begins. Consecutive half-open intervals over a totally ordered set cannot
-- overlap, so there is nothing left for an EXCLUDE constraint to reject. The
-- employment's own termination is a head-level fact (there is no succeeding
-- revision to derive it from), which is why `valid_to` is here and is the one
-- mutable column in this file.
--
-- The alternative — EXCLUDE USING gist over tstzrange on the revisions — was
-- rejected because it is not merely redundant but CONTRADICTORY: with the
-- append-only trigger in place, an open-ended revision can never be closed, so
-- the exclusion would reject every second revision and cap the history at one
-- row per employment.
CREATE TABLE employment_heads (
    org_id     UUID        NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    id         UUID        NOT NULL DEFAULT gen_random_uuid(),
    -- The employment window. `valid_to` NULL means still open.
    valid_from TIMESTAMPTZ NOT NULL,
    valid_to   TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    -- (1) tenant-global uniqueness.
    PRIMARY KEY (org_id, id),
    -- Half-open `[valid_from, valid_to)`: an empty or inverted window is not a
    -- window, so it is made unrepresentable rather than discouraged.
    CONSTRAINT employment_heads_window_is_half_open
        CHECK (valid_to IS NULL OR valid_to > valid_from)
);

-- ---------------------------------------------------------------------------
-- employment_revisions — the append-only history of one employment.
-- ---------------------------------------------------------------------------
CREATE TABLE employment_revisions (
    org_id         UUID        NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    id             UUID        NOT NULL DEFAULT gen_random_uuid(),
    employment_id  UUID        NOT NULL,
    version        BIGINT      NOT NULL CHECK (version >= 1),
    -- The accepted command. Unique per tenant, so applying it twice cannot
    -- append a second revision; the stored `receipt` is replayed instead. Same
    -- authority boundary as 0177's (org_id, command_id).
    command_id     UUID        NOT NULL,
    actor_id       UUID        NOT NULL,
    -- (3) 32 bytes, the width of the digest the command carries.
    payload_digest BYTEA       NOT NULL CHECK (octet_length(payload_digest) = 32),
    -- The instant this revision takes effect. Its half-open interval ends at
    -- the next revision's `valid_from`, or at the head's `valid_to`. See the
    -- head's comment for why the upper bound is not stored per revision.
    valid_from     TIMESTAMPTZ NOT NULL,
    -- (5) The employment's canonical state at this version, and the receipt
    -- handed back on replay. The attribute schema belongs to the port, not to
    -- the table; typed columns invented here (company, org_unit, position,
    -- employment_status) would be re-migrated by the lane that actually defines
    -- them, and `employees` already carries the legacy spelling of all four.
    attributes     JSONB       NOT NULL CHECK (jsonb_typeof(attributes) = 'object'),
    receipt        JSONB       NOT NULL,
    created_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    -- (1) tenant-global uniqueness.
    PRIMARY KEY (org_id, id),
    UNIQUE (org_id, command_id),
    UNIQUE (org_id, employment_id, version),
    -- The structural half of non-overlap: two revisions of one employment
    -- cannot start at the same instant, so the derived intervals are a
    -- partition of the employment window rather than a set that may overlap.
    UNIQUE (org_id, employment_id, valid_from),
    -- RESTRICT, not CASCADE: the immutability trigger below refuses the DELETE
    -- a cascade would issue, so CASCADE here would only turn a clear
    -- foreign-key error into an opaque trigger exception.
    FOREIGN KEY (org_id, employment_id) REFERENCES employment_heads (org_id, id) ON DELETE RESTRICT,
    -- (2) actor binding.
    FOREIGN KEY (actor_id, org_id) REFERENCES users (id, org_id) ON DELETE RESTRICT
);

-- ---------------------------------------------------------------------------
-- employment_source_bindings — which legacy `employees` row an employment IS.
-- ---------------------------------------------------------------------------
-- `employees` remains the legacy compatibility head, so every canonical
-- employment has to be resolvable back to the row the rest of the tree still
-- reads and writes. That mapping is only sound if one employee record cannot
-- carry two bindings, so a duplicate binding is made UNREPRESENTABLE by the
-- primary key rather than discouraged by convention: `PRIMARY KEY (org_id,
-- employee_id)` rejects both a second binding for the same employee AND a
-- re-insert of the identical (employee, employment) pair. A rehire opens a new
-- head on a new `employees` row, so employee-side uniqueness still admits a
-- second employment. `employment_id` is only indexed, not unique — that permits
-- an ambiguous N:1 binding which readers must refuse. Pointing two historical
-- employments at one source record is exactly what this primary key forbids.
--
-- Same shape as 0213's `employee_person_bindings`, for the same reason: this is
-- the surface a reader crosses to go from the legacy row to the canonical
-- object, and it is the surface console-kmb's port writes when it appoints.
CREATE TABLE employment_source_bindings (
    org_id         UUID        NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    employee_id    UUID        NOT NULL,
    employment_id  UUID        NOT NULL,
    actor_id       UUID        NOT NULL,
    -- (3)
    payload_digest BYTEA       NOT NULL CHECK (octet_length(payload_digest) = 32),
    created_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    -- (1) tenant-global uniqueness, and the duplicate-binding constraint.
    PRIMARY KEY (org_id, employee_id),
    FOREIGN KEY (employee_id, org_id) REFERENCES employees (id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (org_id, employment_id) REFERENCES employment_heads (org_id, id) ON DELETE RESTRICT,
    -- (2) actor binding.
    FOREIGN KEY (actor_id, org_id) REFERENCES users (id, org_id) ON DELETE RESTRICT
);

-- The reverse lookup — every employment naming its legacy source row. Also the
-- index the (org_id, employment_id) foreign key needs so a head delete does not
-- sequential-scan.
CREATE INDEX employment_source_bindings_employment_idx
    ON employment_source_bindings (org_id, employment_id);

-- The revisions of one employment in effective order, which is the read every
-- temporal query starts from and the index the (org_id, employment_id) foreign
-- key needs.
CREATE INDEX employment_revisions_effective_idx
    ON employment_revisions (org_id, employment_id, valid_from);

-- ---------------------------------------------------------------------------
-- (4) Immutability.
-- ---------------------------------------------------------------------------
-- One function, two triggers, `TG_TABLE_NAME`/`TG_OP` in the message so the
-- error names the table and the operation that was refused. A second function
-- rather than reusing 0213's `canonical_person_row_immutable()`: the message
-- text is part of the contract the port's tests assert on, and "canonical
-- person table employment_revisions" would be a lie.
--
-- `employment_revisions` refuses UPDATE and DELETE: it is the append-only
-- history, and it is the table the derived `[valid_from, valid_to)` intervals
-- are read out of, so an edited `valid_from` silently rewrites the window of
-- the revision BEFORE it as well.
-- `employment_source_bindings` refuses UPDATE only. Re-pointing an employment
-- at a different legacy row by editing a column is precisely the silent change
-- an audit must not tolerate, so a rebind has to be an explicit DELETE then
-- INSERT; DELETE stays available because a binding names a natural person's
-- employment record and must remain erasable.
--
-- `employment_heads` carries a NARROWER trigger rather than none. The claim
-- "`valid_to` is the one legitimate mutation" was stated in this file and
-- enforced by nothing: `GRANT ... UPDATE, DELETE ON employment_heads` permits an
-- org-armed `console_rt` session to rewrite `valid_from` or `created_at` on an
-- open head, which preserves `org_id` and so passes the RLS policy untouched.
-- A stated invariant that the grant does not restrict is a claim about a control
-- rather than a control. No code updates this table today -- the port only
-- INSERTs (employment.rs:392) -- so enforcing the claim costs nothing and stops
-- the first update that ever arrives from silently being a different one.
--
-- DELETE stays available and is NOT an oversight: an employment head names a
-- natural person's employment record and must remain erasable, the same reason
-- `employment_source_bindings` keeps its DELETE.
CREATE FUNCTION canonical_employment_row_immutable()
RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION 'canonical employment table %: % is refused, the row is immutable',
        TG_TABLE_NAME, TG_OP;
END;
$$;

CREATE TRIGGER trg_employment_revisions_immutable
    BEFORE UPDATE OR DELETE ON employment_revisions
    FOR EACH ROW EXECUTE FUNCTION canonical_employment_row_immutable();

CREATE TRIGGER trg_employment_source_bindings_immutable
    BEFORE UPDATE ON employment_source_bindings
    FOR EACH ROW EXECUTE FUNCTION canonical_employment_row_immutable();

CREATE FUNCTION employment_head_only_valid_to_moves()
RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    -- Compare the whole row with `valid_to` normalised away: anything else that
    -- differs is refused. Stated over the ROW rather than as a column list, so a
    -- column added to this table in a later migration is covered on the day it
    -- is added rather than when someone remembers to extend an enumeration.
    IF to_jsonb(NEW) - 'valid_to' IS DISTINCT FROM to_jsonb(OLD) - 'valid_to' THEN
        RAISE EXCEPTION
            'employment_heads: only valid_to may change, but % also moved',
            (SELECT string_agg(key, ', ' ORDER BY key)
             FROM jsonb_each(to_jsonb(NEW) - 'valid_to') n
             WHERE n.value IS DISTINCT FROM (to_jsonb(OLD) - 'valid_to') -> n.key);
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER trg_employment_heads_only_valid_to_moves
    BEFORE UPDATE ON employment_heads
    FOR EACH ROW EXECUTE FUNCTION employment_head_only_valid_to_moves();

-- ---------------------------------------------------------------------------
-- (6) Tenant isolation: ENABLE **and** FORCE, with the org_isolation policy.
-- ---------------------------------------------------------------------------
ALTER TABLE employment_heads ENABLE ROW LEVEL SECURITY;
ALTER TABLE employment_heads FORCE ROW LEVEL SECURITY;
CREATE POLICY org_isolation ON employment_heads
    USING (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid)
    WITH CHECK (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid);

ALTER TABLE employment_revisions ENABLE ROW LEVEL SECURITY;
ALTER TABLE employment_revisions FORCE ROW LEVEL SECURITY;
CREATE POLICY org_isolation ON employment_revisions
    USING (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid)
    WITH CHECK (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid);

ALTER TABLE employment_source_bindings ENABLE ROW LEVEL SECURITY;
ALTER TABLE employment_source_bindings FORCE ROW LEVEL SECURITY;
CREATE POLICY org_isolation ON employment_source_bindings
    USING (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid)
    WITH CHECK (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid);

REVOKE ALL ON employment_heads FROM PUBLIC;
-- ...and from console_rt itself. `FROM PUBLIC` does not touch a role that already
-- holds the privilege through console_app's default privileges, so omitting a verb
-- from the GRANT below did NOT withhold it. With this line the GRANT is the whole
-- truth about what console_rt has on this table, which is what every comment in
-- this file already assumed it was.
REVOKE ALL ON employment_heads FROM console_rt;
GRANT SELECT, INSERT, UPDATE, DELETE ON employment_heads TO console_rt;

REVOKE ALL ON employment_revisions FROM PUBLIC;
-- ...and from console_rt itself. `FROM PUBLIC` does not touch a role that already
-- holds the privilege through console_app's default privileges, so omitting a verb
-- from the GRANT below did NOT withhold it. With this line the GRANT is the whole
-- truth about what console_rt has on this table, which is what every comment in
-- this file already assumed it was.
REVOKE ALL ON employment_revisions FROM console_rt;
GRANT SELECT, INSERT ON employment_revisions TO console_rt;

REVOKE ALL ON employment_source_bindings FROM PUBLIC;
-- ...and from console_rt itself. `FROM PUBLIC` does not touch a role that already
-- holds the privilege through console_app's default privileges, so omitting a verb
-- from the GRANT below did NOT withhold it. With this line the GRANT is the whole
-- truth about what console_rt has on this table, which is what every comment in
-- this file already assumed it was.
REVOKE ALL ON employment_source_bindings FROM console_rt;
GRANT SELECT, INSERT, DELETE ON employment_source_bindings TO console_rt;

-- ---------------------------------------------------------------------------
-- Personal-data classification.
-- ---------------------------------------------------------------------------
-- Required, not optional: `BASELINE_FROZEN_AFTER_MIGRATION = 209`, so nothing
-- this migration creates can be sheltered by the unclassified baseline. Rule A
-- of 0211 applies to all three tables — an employment record is the employment
-- of one identified natural person, so every column is at least `personal`
-- (개인정보 보호법 제2조제1호나목: information that identifies a specific
-- individual in ready combination with other information). The two JSONB
-- columns additionally carry `undeclared` under Rule C, because unbounded JSONB
-- is an admission that the content is not known.

COMMENT ON COLUMN employment_heads.org_id IS 'pd:personal — 특정 자연인의 고용관계 행; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN employment_heads.id IS 'pd:personal — 고용관계의 정규 식별자';
COMMENT ON COLUMN employment_heads.valid_from IS 'pd:personal — 특정 자연인의 고용 개시 시점';
COMMENT ON COLUMN employment_heads.valid_to IS 'pd:personal — 특정 자연인의 고용 종료 시점';
COMMENT ON COLUMN employment_heads.created_at IS 'pd:personal — 특정 자연인의 고용관계 행; 개보법 제2조제1호나목 결합 식별';

COMMENT ON COLUMN employment_revisions.org_id IS 'pd:personal — 특정 자연인의 고용관계 행; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN employment_revisions.id IS 'pd:personal — 특정 자연인의 고용관계 행; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN employment_revisions.employment_id IS 'pd:personal — 고용관계의 정규 식별자';
COMMENT ON COLUMN employment_revisions.version IS 'pd:personal — 특정 자연인의 고용관계 행; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN employment_revisions.command_id IS 'pd:personal — 특정 자연인의 고용관계 행; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN employment_revisions.actor_id IS 'pd:personal — 처리자 식별자 - users.id';
COMMENT ON COLUMN employment_revisions.payload_digest IS 'pd:personal — 고용 페이로드의 다이제스트. 원문은 아니나 후보 대조가 가능하므로 none으로 두지 않음';
COMMENT ON COLUMN employment_revisions.valid_from IS 'pd:personal — 해당 리비전의 효력 발생 시점, 특정 자연인의 고용 이력';
COMMENT ON COLUMN employment_revisions.attributes IS 'pd:personal,undeclared — 고용관계의 정규 속성 전체. 스키마가 포트 소유이므로 내용은 비한정 JSONB';
COMMENT ON COLUMN employment_revisions.receipt IS 'pd:personal,undeclared — 재실행 시 반환되는 저장 결과. 고용 속성을 인용할 수 있는 비한정 JSONB';
COMMENT ON COLUMN employment_revisions.created_at IS 'pd:personal — 특정 자연인의 고용관계 행; 개보법 제2조제1호나목 결합 식별';

COMMENT ON COLUMN employment_source_bindings.org_id IS 'pd:personal — 특정 자연인의 고용관계 행; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN employment_source_bindings.employee_id IS 'pd:personal — 직원 레코드 식별자 - employees.id';
COMMENT ON COLUMN employment_source_bindings.employment_id IS 'pd:personal — 고용관계의 정규 식별자';
COMMENT ON COLUMN employment_source_bindings.actor_id IS 'pd:personal — 처리자 식별자 - users.id';
COMMENT ON COLUMN employment_source_bindings.payload_digest IS 'pd:personal — 결속 페이로드의 다이제스트. 원문은 아니나 후보 대조가 가능하므로 none으로 두지 않음';
COMMENT ON COLUMN employment_source_bindings.created_at IS 'pd:personal — 특정 자연인의 고용관계 행; 개보법 제2조제1호나목 결합 식별';
