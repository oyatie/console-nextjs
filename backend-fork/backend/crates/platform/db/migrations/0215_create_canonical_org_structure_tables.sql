-- The LAST SIX canonical tables: everything `ObjectKey::Company`,
-- `ObjectKey::OrgUnit` and `ObjectKey::JobPosition` own that does not already
-- exist. After this file every name in the writer-ownership registry resolves
-- to a real relation.
--
-- Quoted verbatim from backend/crates/ontology/canonical-domain/src/lib.rs:99-122:
--
--     /// `organizations` stays the tenant/current head; `company_revisions` is the
--     /// append-only history. No `companies` table is created.
--     ...
--     Company => "company",
--         owner = "console-ontology-canonical-adapter-postgres",
--         tables = ["organizations", "company_revisions"];
--
--     /// New heads/revisions plus unique source-kind/source-ID bindings. Sites stay
--     /// operational and are not OrgUnits.
--     OrgUnit => "org_unit",
--         owner = "console-ontology-canonical-adapter-postgres",
--         tables = ["org_units", "org_unit_revisions", "org_unit_source_bindings"];
--
--     /// Heads/revisions referencing OrgUnit. Recruiting postings and employee
--     /// position strings are not canonical positions.
--     JobPosition => "job_position",
--         owner = "console-ontology-canonical-adapter-postgres",
--         tables = ["job_positions", "job_position_revisions"];
--
-- MEASURED BEFORE WRITING. `CREATE TABLE` over this directory returns exactly
-- one match for the seven roster names of these three objects —
-- `0026_create_organizations.sql:15`. `organizations` is therefore NOT
-- recreated, and the other six have zero creators. Migrations 0213 (Person) and
-- 0214 (Employment) did nothing for them.
--
-- WHY ONE WRITER CREATES ALL SIX. Identical to 0213 and 0214, and this is the
-- file that makes the point pay: migration numbers are assigned at MERGE, not
-- at authoring, so the three port lanes (console-ivo Company, console-kyh
-- OrgUnit, console-6pl JobPosition) each picking their own number would break
-- the contiguity check in console-gate-migration-safety, and all three would
-- append to the SAME three shared registries and collide. The integration owner
-- lands the schema once; the three lanes then fill
-- `canonical-adapter-postgres/src/company.rs`, `src/org_unit.rs` and
-- `src/job_position.rs` — already declared `pub` in that crate's `lib.rs` — as
-- three path-disjoint edits.
--
-- THE OWNER IS `console-ontology-canonical-adapter-postgres`, the crate at
-- backend/crates/ontology/canonical-adapter-postgres. Its sibling
-- backend/crates/ontology/adapter-postgres is package
-- `console-ontology-adapter-postgres` — the ontology METAMODEL adapter, one
-- hyphenated word away and NOT the owner of anything created here. There is no
-- per-object crate: every port is a MODULE of the canonical adapter.
--
-- SHAPE. 0213 and 0214 are the reference — themselves copies of
-- 0177_ontology_action_command_receipts.sql — and every one of their six
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
-- WHICH TABLES ARE APPEND-ONLY AND WHICH ARE HEADS, decided per table rather
-- than copied. 0214 established the asymmetry deliberately and it is followed,
-- not imitated:
--
--   company_revisions        APPEND-ONLY. Refuses UPDATE and DELETE. It is the
--                            legal history of the tenant's own company record.
--   org_units                IDENTITY ANCHOR, like 0213's `persons`. It holds no
--                            mutable state at all — every attribute lives in
--                            `org_unit_revisions` — so there is nothing for a
--                            trigger to protect and none is created. The
--                            hierarchy is deliberately NOT a column here: the
--                            contract names no parent edge for OrgUnit, and a
--                            `parent_id` invented at this layer would be
--                            re-migrated by the lane that actually defines the
--                            tree.
--   org_unit_revisions       APPEND-ONLY. Refuses UPDATE and DELETE.
--   org_unit_source_bindings REFUSES UPDATE, PERMITS DELETE — the
--                            `employment_source_bindings` rule of 0214, for the
--                            same reason: silently re-pointing a legacy source
--                            record at a different org unit by editing a column
--                            is exactly what an audit must not tolerate, so a
--                            rebind is an explicit DELETE then INSERT, while
--                            DELETE itself stays available for erasure.
--   job_positions            TEMPORAL/CURRENT HEAD, like 0214's
--                            `employment_heads`, and for the same reason: it
--                            carries the ONE piece of state a revision cannot,
--                            `org_unit_id`. The contract says
--                            "Heads/revisions referencing OrgUnit" — a
--                            reference is a FOREIGN KEY or it is not enforced,
--                            and a key cannot be declared on a JSONB member. A
--                            reorganisation moves a position between units, so
--                            that column is mutable and this table therefore
--                            carries NO append-only trigger. Its history is in
--                            `job_position_revisions`.
--   job_position_revisions   APPEND-ONLY. Refuses UPDATE and DELETE.
--
-- THE TRIGGER IS THE ENFORCEMENT, NOT THE GRANT. `ops/postgres-reconcile-topology.sh`
-- runs `ALTER DEFAULT PRIVILEGES FOR ROLE console_app IN SCHEMA public GRANT
-- SELECT, INSERT, UPDATE, DELETE ON TABLES TO console_rt` before migrations, so
-- the runtime role already holds UPDATE on every table this file creates and
-- the `GRANT` lines below neither add nor subtract that. 0177, 0213 and 0214
-- have exactly the same property; it is copied rather than improved on.

-- ---------------------------------------------------------------------------
-- company_revisions — the append-only history of the tenant's company.
-- ---------------------------------------------------------------------------
-- No `company_id`. `organizations` IS the company and IS the tenant, which is
-- what the contract's "`organizations` stays the tenant/current head" says, so a
-- column that could only ever equal `org_id` would be a second spelling of the
-- same fact and a second place for it to go wrong. `version` is therefore
-- unique per tenant, not per company.
CREATE TABLE company_revisions (
    org_id         UUID        NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    id             UUID        NOT NULL DEFAULT gen_random_uuid(),
    version        BIGINT      NOT NULL CHECK (version >= 1),
    -- The accepted command. Unique per tenant, so applying it twice cannot
    -- append a second revision; the stored `receipt` is replayed instead. Same
    -- authority boundary as 0177's (org_id, command_id).
    command_id     UUID        NOT NULL,
    actor_id       UUID        NOT NULL,
    -- (3) 32 bytes, the width of the digest the command carries.
    payload_digest BYTEA       NOT NULL CHECK (octet_length(payload_digest) = 32),
    -- (5) The company's canonical state at this version, and the receipt handed
    -- back on replay. The attribute schema belongs to the port, not to the
    -- table; typed columns invented here would be re-migrated by console-ivo.
    attributes     JSONB       NOT NULL CHECK (jsonb_typeof(attributes) = 'object'),
    receipt        JSONB       NOT NULL,
    created_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    -- (1) tenant-global uniqueness.
    PRIMARY KEY (org_id, id),
    UNIQUE (org_id, command_id),
    UNIQUE (org_id, version),
    -- (2) actor binding.
    FOREIGN KEY (actor_id, org_id) REFERENCES users (id, org_id) ON DELETE RESTRICT
);

-- ---------------------------------------------------------------------------
-- org_units — the stable identity of one organisational unit.
-- ---------------------------------------------------------------------------
-- Deliberately just an identity anchor, exactly as 0213's `persons`: every
-- attribute of the unit lives in `org_unit_revisions`, so there is nothing here
-- to update and no head pointer to leave dangling. console-kyh adds a
-- denormalised head column if it can show it needs one.
CREATE TABLE org_units (
    org_id     UUID        NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    id         UUID        NOT NULL DEFAULT gen_random_uuid(),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    -- (1) tenant-global uniqueness.
    PRIMARY KEY (org_id, id)
);

-- ---------------------------------------------------------------------------
-- org_unit_revisions — the append-only history of one org unit.
-- ---------------------------------------------------------------------------
CREATE TABLE org_unit_revisions (
    org_id         UUID        NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    id             UUID        NOT NULL DEFAULT gen_random_uuid(),
    org_unit_id    UUID        NOT NULL,
    version        BIGINT      NOT NULL CHECK (version >= 1),
    command_id     UUID        NOT NULL,
    actor_id       UUID        NOT NULL,
    -- (3)
    payload_digest BYTEA       NOT NULL CHECK (octet_length(payload_digest) = 32),
    -- (5)
    attributes     JSONB       NOT NULL CHECK (jsonb_typeof(attributes) = 'object'),
    receipt        JSONB       NOT NULL,
    created_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    -- (1) tenant-global uniqueness.
    PRIMARY KEY (org_id, id),
    UNIQUE (org_id, command_id),
    -- Also the index the (org_id, org_unit_id) foreign key needs, by prefix, so
    -- an org-unit delete does not sequential-scan.
    UNIQUE (org_id, org_unit_id, version),
    -- RESTRICT, not CASCADE: the immutability trigger below refuses the DELETE
    -- a cascade would issue, so CASCADE here would only turn a clear
    -- foreign-key error into an opaque trigger exception.
    FOREIGN KEY (org_id, org_unit_id) REFERENCES org_units (org_id, id) ON DELETE RESTRICT,
    -- (2) actor binding.
    FOREIGN KEY (actor_id, org_id) REFERENCES users (id, org_id) ON DELETE RESTRICT
);

-- ---------------------------------------------------------------------------
-- org_unit_source_bindings — which legacy record an org unit WAS built from.
-- ---------------------------------------------------------------------------
-- The contract's "unique source-kind/source-ID bindings". The uniqueness that
-- has to hold is that ONE legacy record resolves to at most ONE canonical org
-- unit, so it is made UNREPRESENTABLE by the primary key rather than
-- discouraged by convention: `PRIMARY KEY (org_id, source_kind, source_id)`
-- rejects both a second binding for the same source record AND a re-insert of
-- the identical (source, unit) triple. The reverse direction is deliberately
-- NOT unique — one canonical unit legitimately absorbs several legacy records,
-- and a merge must not need a schema change.
--
-- `source_kind` and `source_id` are TEXT and are constrained only to be
-- non-empty. The closed set of kinds belongs to console-kyh: an enum invented
-- here would be re-migrated by the lane that actually enumerates the legacy
-- systems, and legacy identifiers are not all UUIDs.
CREATE TABLE org_unit_source_bindings (
    org_id         UUID        NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    source_kind    TEXT        NOT NULL CHECK (source_kind <> ''),
    source_id      TEXT        NOT NULL CHECK (source_id <> ''),
    org_unit_id    UUID        NOT NULL,
    actor_id       UUID        NOT NULL,
    -- (3)
    payload_digest BYTEA       NOT NULL CHECK (octet_length(payload_digest) = 32),
    created_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    -- (1) tenant-global uniqueness, and the duplicate-binding constraint.
    PRIMARY KEY (org_id, source_kind, source_id),
    FOREIGN KEY (org_id, org_unit_id) REFERENCES org_units (org_id, id) ON DELETE RESTRICT,
    -- (2) actor binding.
    FOREIGN KEY (actor_id, org_id) REFERENCES users (id, org_id) ON DELETE RESTRICT
);

-- The reverse lookup — every legacy record naming one canonical unit. Also the
-- index the (org_id, org_unit_id) foreign key needs so an org-unit delete does
-- not sequential-scan.
CREATE INDEX org_unit_source_bindings_org_unit_idx
    ON org_unit_source_bindings (org_id, org_unit_id);

-- ---------------------------------------------------------------------------
-- job_positions — the current head of one canonical position.
-- ---------------------------------------------------------------------------
-- See the header for why `org_unit_id` lives here and why this table carries no
-- append-only trigger. RESTRICT on the unit: a unit that still has positions is
-- not deletable, which is the error the caller should see rather than a silent
-- orphaning.
CREATE TABLE job_positions (
    org_id      UUID        NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    id          UUID        NOT NULL DEFAULT gen_random_uuid(),
    -- The contract's "referencing OrgUnit", as a key rather than as prose.
    -- Mutable: a reorganisation moves the position.
    org_unit_id UUID        NOT NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    -- (1) tenant-global uniqueness.
    PRIMARY KEY (org_id, id),
    FOREIGN KEY (org_id, org_unit_id) REFERENCES org_units (org_id, id) ON DELETE RESTRICT
);

-- Every position of one unit, which is the read an org chart starts from, and
-- the index the (org_id, org_unit_id) foreign key needs.
CREATE INDEX job_positions_org_unit_idx
    ON job_positions (org_id, org_unit_id);

-- ---------------------------------------------------------------------------
-- job_position_revisions — the append-only history of one position.
-- ---------------------------------------------------------------------------
CREATE TABLE job_position_revisions (
    org_id          UUID        NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    id              UUID        NOT NULL DEFAULT gen_random_uuid(),
    job_position_id UUID        NOT NULL,
    version         BIGINT      NOT NULL CHECK (version >= 1),
    command_id      UUID        NOT NULL,
    actor_id        UUID        NOT NULL,
    -- (3)
    payload_digest  BYTEA       NOT NULL CHECK (octet_length(payload_digest) = 32),
    -- (5)
    attributes      JSONB       NOT NULL CHECK (jsonb_typeof(attributes) = 'object'),
    receipt         JSONB       NOT NULL,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    -- (1) tenant-global uniqueness.
    PRIMARY KEY (org_id, id),
    UNIQUE (org_id, command_id),
    -- Also the index the (org_id, job_position_id) foreign key needs, by prefix.
    UNIQUE (org_id, job_position_id, version),
    -- RESTRICT, for the reason org_unit_revisions records.
    FOREIGN KEY (org_id, job_position_id) REFERENCES job_positions (org_id, id) ON DELETE RESTRICT,
    -- (2) actor binding.
    FOREIGN KEY (actor_id, org_id) REFERENCES users (id, org_id) ON DELETE RESTRICT
);

-- ---------------------------------------------------------------------------
-- (4) Immutability.
-- ---------------------------------------------------------------------------
-- ONE function for all three objects, not three. 0214 justified a second
-- function because reusing 0213's would have made the message say "canonical
-- person table employment_revisions", which is a lie the port tests assert on.
-- That argument bounds the name to the truth, and `org-structure` is true of
-- Company, OrgUnit and JobPosition alike; `TG_TABLE_NAME` and `TG_OP` carry the
-- rest, so three copies would differ in nothing but the count of things to keep
-- in step.
CREATE FUNCTION canonical_org_structure_row_immutable()
RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION 'canonical org-structure table %: % is refused, the row is immutable',
        TG_TABLE_NAME, TG_OP;
END;
$$;

CREATE TRIGGER trg_company_revisions_immutable
    BEFORE UPDATE OR DELETE ON company_revisions
    FOR EACH ROW EXECUTE FUNCTION canonical_org_structure_row_immutable();

CREATE TRIGGER trg_org_unit_revisions_immutable
    BEFORE UPDATE OR DELETE ON org_unit_revisions
    FOR EACH ROW EXECUTE FUNCTION canonical_org_structure_row_immutable();

CREATE TRIGGER trg_job_position_revisions_immutable
    BEFORE UPDATE OR DELETE ON job_position_revisions
    FOR EACH ROW EXECUTE FUNCTION canonical_org_structure_row_immutable();

CREATE TRIGGER trg_org_unit_source_bindings_immutable
    BEFORE UPDATE ON org_unit_source_bindings
    FOR EACH ROW EXECUTE FUNCTION canonical_org_structure_row_immutable();

-- ---------------------------------------------------------------------------
-- (6) Tenant isolation: ENABLE **and** FORCE, with the org_isolation policy.
-- ---------------------------------------------------------------------------
ALTER TABLE company_revisions ENABLE ROW LEVEL SECURITY;
ALTER TABLE company_revisions FORCE ROW LEVEL SECURITY;
CREATE POLICY org_isolation ON company_revisions
    USING (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid)
    WITH CHECK (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid);

ALTER TABLE org_units ENABLE ROW LEVEL SECURITY;
ALTER TABLE org_units FORCE ROW LEVEL SECURITY;
CREATE POLICY org_isolation ON org_units
    USING (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid)
    WITH CHECK (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid);

ALTER TABLE org_unit_revisions ENABLE ROW LEVEL SECURITY;
ALTER TABLE org_unit_revisions FORCE ROW LEVEL SECURITY;
CREATE POLICY org_isolation ON org_unit_revisions
    USING (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid)
    WITH CHECK (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid);

ALTER TABLE org_unit_source_bindings ENABLE ROW LEVEL SECURITY;
ALTER TABLE org_unit_source_bindings FORCE ROW LEVEL SECURITY;
CREATE POLICY org_isolation ON org_unit_source_bindings
    USING (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid)
    WITH CHECK (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid);

ALTER TABLE job_positions ENABLE ROW LEVEL SECURITY;
ALTER TABLE job_positions FORCE ROW LEVEL SECURITY;
CREATE POLICY org_isolation ON job_positions
    USING (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid)
    WITH CHECK (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid);

ALTER TABLE job_position_revisions ENABLE ROW LEVEL SECURITY;
ALTER TABLE job_position_revisions FORCE ROW LEVEL SECURITY;
CREATE POLICY org_isolation ON job_position_revisions
    USING (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid)
    WITH CHECK (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid);

REVOKE ALL ON company_revisions FROM PUBLIC;
-- ...and from console_rt itself. `FROM PUBLIC` does not touch a role that already
-- holds the privilege through console_app's default privileges, so omitting a verb
-- from the GRANT below did NOT withhold it. With this line the GRANT is the whole
-- truth about what console_rt has on this table, which is what every comment in
-- this file already assumed it was.
REVOKE ALL ON company_revisions FROM console_rt;
GRANT SELECT, INSERT ON company_revisions TO console_rt;

REVOKE ALL ON org_units FROM PUBLIC;
-- ...and from console_rt itself. `FROM PUBLIC` does not touch a role that already
-- holds the privilege through console_app's default privileges, so omitting a verb
-- from the GRANT below did NOT withhold it. With this line the GRANT is the whole
-- truth about what console_rt has on this table, which is what every comment in
-- this file already assumed it was.
REVOKE ALL ON org_units FROM console_rt;
GRANT SELECT, INSERT, DELETE ON org_units TO console_rt;

REVOKE ALL ON org_unit_revisions FROM PUBLIC;
-- ...and from console_rt itself. `FROM PUBLIC` does not touch a role that already
-- holds the privilege through console_app's default privileges, so omitting a verb
-- from the GRANT below did NOT withhold it. With this line the GRANT is the whole
-- truth about what console_rt has on this table, which is what every comment in
-- this file already assumed it was.
REVOKE ALL ON org_unit_revisions FROM console_rt;
GRANT SELECT, INSERT ON org_unit_revisions TO console_rt;

REVOKE ALL ON org_unit_source_bindings FROM PUBLIC;
-- ...and from console_rt itself. `FROM PUBLIC` does not touch a role that already
-- holds the privilege through console_app's default privileges, so omitting a verb
-- from the GRANT below did NOT withhold it. With this line the GRANT is the whole
-- truth about what console_rt has on this table, which is what every comment in
-- this file already assumed it was.
REVOKE ALL ON org_unit_source_bindings FROM console_rt;
GRANT SELECT, INSERT, DELETE ON org_unit_source_bindings TO console_rt;

REVOKE ALL ON job_positions FROM PUBLIC;
-- ...and from console_rt itself. `FROM PUBLIC` does not touch a role that already
-- holds the privilege through console_app's default privileges, so omitting a verb
-- from the GRANT below did NOT withhold it. With this line the GRANT is the whole
-- truth about what console_rt has on this table, which is what every comment in
-- this file already assumed it was.
REVOKE ALL ON job_positions FROM console_rt;
GRANT SELECT, INSERT, UPDATE, DELETE ON job_positions TO console_rt;

REVOKE ALL ON job_position_revisions FROM PUBLIC;
-- ...and from console_rt itself. `FROM PUBLIC` does not touch a role that already
-- holds the privilege through console_app's default privileges, so omitting a verb
-- from the GRANT below did NOT withhold it. With this line the GRANT is the whole
-- truth about what console_rt has on this table, which is what every comment in
-- this file already assumed it was.
REVOKE ALL ON job_position_revisions FROM console_rt;
GRANT SELECT, INSERT ON job_position_revisions TO console_rt;

-- ---------------------------------------------------------------------------
-- Personal-data classification.
-- ---------------------------------------------------------------------------
-- Required, not optional: `BASELINE_FROZEN_AFTER_MIGRATION = 209`, so nothing
-- this migration creates can be sheltered by the unclassified baseline.
--
-- Rule A of 0211 does NOT apply here, and that is the difference from 0213 and
-- 0214: a company, an organisational unit and a job position are not natural
-- persons and a row of any of these tables identifies none. Copying
-- `pd:personal` across every column because the neighbouring migrations do
-- would make the marker mean nothing. Three classes are used instead:
--
--   pd:personal    `actor_id` only — it IS a natural person, `users.id`.
--   pd:undeclared  the two JSONB columns and the digest taken over the payload
--                  they came from. Rule C: unbounded JSONB is an admission that
--                  the content is not known, and the schema is the port's. It is
--                  NOT additionally marked `personal`: that would assert a fact
--                  about content nobody has declared, where `undeclared` states
--                  exactly what is known.
--   pd:none        every structural column of a non-person row.

COMMENT ON COLUMN company_revisions.org_id IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN company_revisions.id IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN company_revisions.version IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN company_revisions.command_id IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN company_revisions.actor_id IS 'pd:personal — 처리자 식별자 - users.id';
COMMENT ON COLUMN company_revisions.payload_digest IS 'pd:undeclared — 포트 소유 스키마의 페이로드에 대한 다이제스트. 원문 내용이 선언되지 않았으므로 none으로 두지 않음';
COMMENT ON COLUMN company_revisions.attributes IS 'pd:undeclared — 회사의 정규 속성 전체. 스키마가 포트 소유이므로 내용은 비한정 JSONB';
COMMENT ON COLUMN company_revisions.receipt IS 'pd:undeclared — 재실행 시 반환되는 저장 결과. 비한정 JSONB';
COMMENT ON COLUMN company_revisions.created_at IS 'pd:none — structural or non-personal attribute of a non-person row';

COMMENT ON COLUMN org_units.org_id IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN org_units.id IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN org_units.created_at IS 'pd:none — structural or non-personal attribute of a non-person row';

COMMENT ON COLUMN org_unit_revisions.org_id IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN org_unit_revisions.id IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN org_unit_revisions.org_unit_id IS 'pd:none — 조직 단위의 정규 식별자';
COMMENT ON COLUMN org_unit_revisions.version IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN org_unit_revisions.command_id IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN org_unit_revisions.actor_id IS 'pd:personal — 처리자 식별자 - users.id';
COMMENT ON COLUMN org_unit_revisions.payload_digest IS 'pd:undeclared — 포트 소유 스키마의 페이로드에 대한 다이제스트. 원문 내용이 선언되지 않았으므로 none으로 두지 않음';
COMMENT ON COLUMN org_unit_revisions.attributes IS 'pd:undeclared — 조직 단위의 정규 속성 전체. 스키마가 포트 소유이므로 내용은 비한정 JSONB';
COMMENT ON COLUMN org_unit_revisions.receipt IS 'pd:undeclared — 재실행 시 반환되는 저장 결과. 비한정 JSONB';
COMMENT ON COLUMN org_unit_revisions.created_at IS 'pd:none — structural or non-personal attribute of a non-person row';

COMMENT ON COLUMN org_unit_source_bindings.org_id IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN org_unit_source_bindings.source_kind IS 'pd:none — 레거시 원천 시스템의 종류';
COMMENT ON COLUMN org_unit_source_bindings.source_id IS 'pd:none — 레거시 원천 레코드의 식별자. 조직 단위이므로 자연인 식별자가 아님';
COMMENT ON COLUMN org_unit_source_bindings.org_unit_id IS 'pd:none — 조직 단위의 정규 식별자';
COMMENT ON COLUMN org_unit_source_bindings.actor_id IS 'pd:personal — 처리자 식별자 - users.id';
COMMENT ON COLUMN org_unit_source_bindings.payload_digest IS 'pd:undeclared — 포트 소유 스키마의 페이로드에 대한 다이제스트. 원문 내용이 선언되지 않았으므로 none으로 두지 않음';
COMMENT ON COLUMN org_unit_source_bindings.created_at IS 'pd:none — structural or non-personal attribute of a non-person row';

COMMENT ON COLUMN job_positions.org_id IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN job_positions.id IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN job_positions.org_unit_id IS 'pd:none — 조직 단위의 정규 식별자';
COMMENT ON COLUMN job_positions.created_at IS 'pd:none — structural or non-personal attribute of a non-person row';

COMMENT ON COLUMN job_position_revisions.org_id IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN job_position_revisions.id IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN job_position_revisions.job_position_id IS 'pd:none — 직위의 정규 식별자';
COMMENT ON COLUMN job_position_revisions.version IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN job_position_revisions.command_id IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN job_position_revisions.actor_id IS 'pd:personal — 처리자 식별자 - users.id';
COMMENT ON COLUMN job_position_revisions.payload_digest IS 'pd:undeclared — 포트 소유 스키마의 페이로드에 대한 다이제스트. 원문 내용이 선언되지 않았으므로 none으로 두지 않음';
COMMENT ON COLUMN job_position_revisions.attributes IS 'pd:undeclared — 직위의 정규 속성 전체. 스키마가 포트 소유이므로 내용은 비한정 JSONB';
COMMENT ON COLUMN job_position_revisions.receipt IS 'pd:undeclared — 재실행 시 반환되는 저장 결과. 비한정 JSONB';
COMMENT ON COLUMN job_position_revisions.created_at IS 'pd:none — structural or non-personal attribute of a non-person row';
