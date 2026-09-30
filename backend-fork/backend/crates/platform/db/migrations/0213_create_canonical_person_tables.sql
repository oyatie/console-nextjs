-- The three tables the canonical contract assigns to `ObjectKey::Person`.
--
-- Quoted verbatim from backend/crates/ontology/canonical-domain/src/lib.rs:124:
--
--     /// `persons`/`person_revisions` plus `employee_person_bindings`.
--     Person => "person",
--         owner = "console-ontology-canonical-adapter-postgres",
--         tables = ["persons", "person_revisions", "employee_person_bindings"];
--
-- WHY ONE WRITER CREATES THEM. Migration numbers are assigned at MERGE, not at
-- authoring; two lanes that each pick their own break the contiguity check in
-- console-gate-migration-safety. So the integration owner lands the tables and
-- the Person port lane (console-e0v) fills
-- `canonical-adapter-postgres/src/person.rs` against a schema that already
-- exists. Only Person is created here. `company_revisions`, `org_units`,
-- `org_unit_revisions`, `org_unit_source_bindings`, `job_positions` and
-- `job_position_revisions` are the other lanes' rows in the same roster and are
-- deliberately absent.
--
-- SHAPE. 0177_ontology_action_command_receipts.sql is the reference and every
-- one of its six properties is carried here:
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
-- the `GRANT SELECT, INSERT` lines below neither add nor subtract that. 0177 has
-- exactly the same property; it is why immutability there is a trigger and not
-- a privilege, and it is copied rather than improved on.

-- ---------------------------------------------------------------------------
-- persons — the stable identity of a natural person.
-- ---------------------------------------------------------------------------
-- Deliberately just an identity anchor: every attribute of the person lives in
-- `person_revisions`, so there is nothing here to update and no head pointer to
-- leave dangling. The Person port lane adds a denormalised head column if it
-- can show it needs one.
CREATE TABLE persons (
    org_id     UUID        NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    id         UUID        NOT NULL DEFAULT gen_random_uuid(),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    -- (1) tenant-global uniqueness.
    PRIMARY KEY (org_id, id)
);

-- ---------------------------------------------------------------------------
-- person_revisions — the append-only history of one person.
-- ---------------------------------------------------------------------------
CREATE TABLE person_revisions (
    org_id         UUID        NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    id             UUID        NOT NULL DEFAULT gen_random_uuid(),
    person_id      UUID        NOT NULL,
    version        BIGINT      NOT NULL CHECK (version >= 1),
    -- The accepted command. Unique per tenant, so applying it twice cannot
    -- append a second revision; the stored `receipt` is replayed instead. Same
    -- authority boundary as 0177's (org_id, command_id).
    command_id     UUID        NOT NULL,
    actor_id       UUID        NOT NULL,
    -- (3) 32 bytes, the width of the digest the command carries.
    payload_digest BYTEA       NOT NULL CHECK (octet_length(payload_digest) = 32),
    -- (5) The person's canonical state at this version, and the receipt handed
    -- back on replay. The attribute schema belongs to the port, not to the
    -- table; typed columns invented here would be re-migrated by the lane that
    -- actually defines them.
    attributes     JSONB       NOT NULL CHECK (jsonb_typeof(attributes) = 'object'),
    receipt        JSONB       NOT NULL,
    created_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    -- (1) tenant-global uniqueness.
    PRIMARY KEY (org_id, id),
    UNIQUE (org_id, command_id),
    UNIQUE (org_id, person_id, version),
    -- RESTRICT, not CASCADE: the immutability trigger below refuses the DELETE
    -- a cascade would issue, so CASCADE here would only turn a clear
    -- foreign-key error into an opaque trigger exception.
    FOREIGN KEY (org_id, person_id) REFERENCES persons (org_id, id) ON DELETE RESTRICT,
    -- (2) actor binding.
    FOREIGN KEY (actor_id, org_id) REFERENCES users (id, org_id) ON DELETE RESTRICT
);

-- ---------------------------------------------------------------------------
-- employee_person_bindings — which natural person an employee record IS.
-- ---------------------------------------------------------------------------
-- This is the surface console-dgo.1 reads: "requester and approver are distinct
-- natural persons" is decided by mapping each employee to a `person_id` and
-- comparing. That decision is only sound if an employee cannot carry two
-- bindings, so a duplicate binding is made UNREPRESENTABLE by the primary key
-- rather than discouraged by convention: `PRIMARY KEY (org_id, employee_id)`
-- rejects both a second binding for the same employee AND a re-insert of the
-- identical (employee, person) pair. One person may still be bound to several
-- employee records — a person holding two employment records is legitimate, and
-- it is exactly the case the four-eyes bar has to catch.
CREATE TABLE employee_person_bindings (
    org_id         UUID        NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    employee_id    UUID        NOT NULL,
    person_id      UUID        NOT NULL,
    actor_id       UUID        NOT NULL,
    -- (3)
    payload_digest BYTEA       NOT NULL CHECK (octet_length(payload_digest) = 32),
    created_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    -- (1) tenant-global uniqueness, and the duplicate-binding constraint.
    PRIMARY KEY (org_id, employee_id),
    FOREIGN KEY (employee_id, org_id) REFERENCES employees (id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (org_id, person_id) REFERENCES persons (org_id, id) ON DELETE RESTRICT,
    -- (2) actor binding.
    FOREIGN KEY (actor_id, org_id) REFERENCES users (id, org_id) ON DELETE RESTRICT
);

-- The reverse lookup — every employee record naming one person. Also the index
-- the (org_id, person_id) foreign key needs so a person delete does not
-- sequential-scan.
CREATE INDEX employee_person_bindings_person_idx
    ON employee_person_bindings (org_id, person_id);

-- ---------------------------------------------------------------------------
-- (4) Immutability.
-- ---------------------------------------------------------------------------
-- One function, two triggers, `TG_TABLE_NAME`/`TG_OP` in the message so the
-- error names the table and the operation that was refused.
--
-- `person_revisions` refuses UPDATE and DELETE: it is the append-only history.
-- `employee_person_bindings` refuses UPDATE only. Re-pointing an employee at a
-- different natural person by editing a row is precisely the silent change the
-- four-eyes bar must not tolerate, so a rebind has to be an explicit DELETE
-- then INSERT; DELETE stays available because a binding is personal data that
-- must remain erasable.
CREATE FUNCTION canonical_person_row_immutable()
RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION 'canonical person table %: % is refused, the row is immutable',
        TG_TABLE_NAME, TG_OP;
END;
$$;

CREATE TRIGGER trg_person_revisions_immutable
    BEFORE UPDATE OR DELETE ON person_revisions
    FOR EACH ROW EXECUTE FUNCTION canonical_person_row_immutable();

CREATE TRIGGER trg_employee_person_bindings_immutable
    BEFORE UPDATE ON employee_person_bindings
    FOR EACH ROW EXECUTE FUNCTION canonical_person_row_immutable();

-- ---------------------------------------------------------------------------
-- (6) Tenant isolation: ENABLE **and** FORCE, with the org_isolation policy.
-- ---------------------------------------------------------------------------
ALTER TABLE persons ENABLE ROW LEVEL SECURITY;
ALTER TABLE persons FORCE ROW LEVEL SECURITY;
CREATE POLICY org_isolation ON persons
    USING (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid)
    WITH CHECK (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid);

ALTER TABLE person_revisions ENABLE ROW LEVEL SECURITY;
ALTER TABLE person_revisions FORCE ROW LEVEL SECURITY;
CREATE POLICY org_isolation ON person_revisions
    USING (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid)
    WITH CHECK (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid);

ALTER TABLE employee_person_bindings ENABLE ROW LEVEL SECURITY;
ALTER TABLE employee_person_bindings FORCE ROW LEVEL SECURITY;
CREATE POLICY org_isolation ON employee_person_bindings
    USING (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid)
    WITH CHECK (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid);

REVOKE ALL ON persons FROM PUBLIC;
-- ...and from console_rt itself. `FROM PUBLIC` does not touch a role that already
-- holds the privilege through console_app's default privileges, so omitting a verb
-- from the GRANT below did NOT withhold it. With this line the GRANT is the whole
-- truth about what console_rt has on this table, which is what every comment in
-- this file already assumed it was.
REVOKE ALL ON persons FROM console_rt;
GRANT SELECT, INSERT, DELETE ON persons TO console_rt;

REVOKE ALL ON person_revisions FROM PUBLIC;
-- ...and from console_rt itself. `FROM PUBLIC` does not touch a role that already
-- holds the privilege through console_app's default privileges, so omitting a verb
-- from the GRANT below did NOT withhold it. With this line the GRANT is the whole
-- truth about what console_rt has on this table, which is what every comment in
-- this file already assumed it was.
REVOKE ALL ON person_revisions FROM console_rt;
GRANT SELECT, INSERT ON person_revisions TO console_rt;

REVOKE ALL ON employee_person_bindings FROM PUBLIC;
-- ...and from console_rt itself. `FROM PUBLIC` does not touch a role that already
-- holds the privilege through console_app's default privileges, so omitting a verb
-- from the GRANT below did NOT withhold it. With this line the GRANT is the whole
-- truth about what console_rt has on this table, which is what every comment in
-- this file already assumed it was.
REVOKE ALL ON employee_person_bindings FROM console_rt;
GRANT SELECT, INSERT, DELETE ON employee_person_bindings TO console_rt;

-- ---------------------------------------------------------------------------
-- Personal-data classification.
-- ---------------------------------------------------------------------------
-- Required, not optional: `BASELINE_FROZEN_AFTER_MIGRATION = 209`, so nothing
-- this migration creates can be sheltered by the unclassified baseline. Rule A
-- of 0211 applies to all three tables — the row IS a natural person, or the
-- identity of one, so every column is at least `personal` (개인정보 보호법
-- 제2조제1호나목: information that identifies a specific individual in ready
-- combination with other information). The two JSONB columns additionally carry
-- `undeclared` under Rule C, because unbounded JSONB is an admission that the
-- content is not known.

COMMENT ON COLUMN persons.org_id IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN persons.id IS 'pd:personal — 자연인의 정규 식별자';
COMMENT ON COLUMN persons.created_at IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';

COMMENT ON COLUMN person_revisions.org_id IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN person_revisions.id IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN person_revisions.person_id IS 'pd:personal — 자연인의 정규 식별자';
COMMENT ON COLUMN person_revisions.version IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN person_revisions.command_id IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN person_revisions.actor_id IS 'pd:personal — 처리자 식별자 - users.id';
COMMENT ON COLUMN person_revisions.payload_digest IS 'pd:personal — 자연인 페이로드의 다이제스트. 원문은 아니나 후보 대조가 가능하므로 none으로 두지 않음';
COMMENT ON COLUMN person_revisions.attributes IS 'pd:personal,undeclared — 자연인의 정규 속성 전체. 스키마가 포트 소유이므로 내용은 비한정 JSONB';
COMMENT ON COLUMN person_revisions.receipt IS 'pd:personal,undeclared — 재실행 시 반환되는 저장 결과. 자연인 속성을 인용할 수 있는 비한정 JSONB';
COMMENT ON COLUMN person_revisions.created_at IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';

COMMENT ON COLUMN employee_person_bindings.org_id IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN employee_person_bindings.employee_id IS 'pd:personal — 직원 레코드 식별자 - employees.id';
COMMENT ON COLUMN employee_person_bindings.person_id IS 'pd:personal — 자연인의 정규 식별자';
COMMENT ON COLUMN employee_person_bindings.actor_id IS 'pd:personal — 처리자 식별자 - users.id';
COMMENT ON COLUMN employee_person_bindings.payload_digest IS 'pd:personal — 결속 페이로드의 다이제스트. 원문은 아니나 후보 대조가 가능하므로 none으로 두지 않음';
COMMENT ON COLUMN employee_person_bindings.created_at IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
