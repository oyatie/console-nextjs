//! TWO CHECKS GUARD PERSONAL-DATA CLASSIFICATION. THIS IS ONE OF THEM.
//!
//! The other is `every_application_column_is_classified_or_its_table_is_declared`
//! in `backend/crates/platform/db/tests/personal_data_classification.rs`.
//! THIS GATE IS DEFENCE IN DEPTH. Six rounds of hardening its parser each closed
//! one spelling and each was followed by another, so the other check no longer
//! depends on this parser being complete. Its baseline used to pin a per-table
//! unclassified COUNT, and a count is payable: every one of those criticals
//! needed two things at once — a spelling this parser misreads, AND a
//! compensating classification in the same migration that left the count where
//! it was. That baseline is now the SET of unclassified column NAMES, which has
//! nothing to trade with. Bounded, measured parser corrections still strengthen
//! the earlier write-time edge without replacing that independent control.
//!
//! What follows from that, stated as narrowly as the measurement supports: for
//! ANY RELATION THE CATALOG SWEEP READS, a blind spot in this parser no longer
//! composes into a silent live unclassified column. It is not a claim about this
//! parser, which is still blind in the ways listed below and will acquire more.
//! It is a claim about the composite, and it stops at the edge of what that
//! sweep reads — see WHERE THE CATALOG ASSERTION IS BLIND.
//!
//! SO WHY KEEP THIS GATE. Two reasons, both load-bearing:
//!
//! * **It is the only thing that sees a column at WRITE time.** Before any
//!   database exists, before anything is applied, on a developer's machine in
//!   milliseconds. The catalog assertion needs CI to stand a database up.
//! * **The catalog is consulted only for relations it knows**, and it knows only
//!   what `./migrations` built. This reads the text, including text that builds
//!   relations the catalog sweep will never enumerate.
//!
//! WHAT THE CATALOG ASSERTION READS. Every column of every application table out
//! of `pg_attribute`/`pg_class`/`pg_namespace` after the migrations have actually
//! run. `UNLOGGED`, quoted mixed case, schema qualification, a single-quoted `DO`
//! body, `LIKE INCLUDING ALL`, `PARTITION OF`, `INHERITS`, a multi-action
//! `ALTER TABLE` and a keyword assembled out of concatenated fragments all land
//! in `pg_attribute` identically, because the catalog holds what the DDL PRODUCED
//! and not how it was spelled. What that reaches is bounded by which relations
//! are in it, not by syntax.
//!
//! WHERE THIS GATE IS BLIND — constructs it reads WRONG, so it prints PASSED on
//! a column that is really unclassified. This list is a RESIDUAL REGISTER, not
//! standing authority to widen a correction. The schema, quoted-identifier and
//! concatenation cases were planted as migrations and confirmed by execution;
//! the other two are read from this file's own code, not planted:
//!
//! * **Schema qualification is discarded.** `read_qualified_name` keeps only
//!   the last component, so `shadow.employees` registers as `employees` and
//!   inherits the real table's markers. The corpus already creates non-public
//!   schemas (`leave_api`, `ontology_api`, `ont_policy_api`).
//! * **Quoted identifiers are case-folded.** `lex` lowercases everything, so
//!   `"Employees"."RAW_ROW"` is read as the already-classified
//!   `employees.raw_row`. PostgreSQL treats those as different relations.
//! * **A column named `check`, `unique` or `primary`** — legal, quoted — is
//!   read as the start of a table constraint and vanishes from the column set.
//! * **`check_freeze_clock` scans vacancies over `1..highest`**, so slot `0000`
//!   is never examined. A migration numbered `0000` would sit below every
//!   existing one and be read as pre-freeze.
//! * **A body that splits a keyword across a concatenation.**
//!   `DO $$ BEGIN EXECUTE 'ALTER TA' || 'BLE leave_requests ADD COLUMN
//!   medical_certificate_no TEXT'; END $$;`. `body_builds_table_ddl` reads words,
//!   finds `alter`, finds no `table` in the four-word window after it, and
//!   returns `None`. The `Tok::Str` scan does not reach it either: the keyword is
//!   split across two literals and neither holds it whole.
//!   `'CREA' || 'TE TABLE'` is the same hole. Planted with a compensating
//!   `COMMENT ON COLUMN` under the count baseline and measured: gate EXIT=0 AND
//!   catalog EXIT=0, one extra LIVE column, every per-table pin unmoved. Replanted
//!   against the SET baseline: gate EXIT=0, **catalog EXIT=101**, naming
//!   `medical_certificate_no` as landed and `reason` as no longer unclassified.
//!   This parser still cannot reach it; it no longer needs to.
//!
//! WHAT THIS CORRECTION CLOSES. A multi-action `ALTER TABLE` used to be judged
//! from its FIRST action. `alter_action_is_column_neutral` read one action and
//! ruled on the whole statement, so this repository's house idiom —
//! `EXECUTE format('ALTER TABLE %I ENABLE ROW LEVEL SECURITY, ADD COLUMN
//! medical_certificate_no TEXT', 'leave_requests')`, the shape used in 25
//! migrations, with one comma appended — was read as column-neutral. Nothing
//! was concatenated and every keyword was spelled whole. Under the count
//! baseline it measured gate EXIT=0, catalog EXIT=0, `3268 / 668 / 2600`
//! against a clean `3267 / 667 / 2600`, with all 243 pins unmoved and the column
//! confirmed live in `pg_attribute`. Replanted against the SET baseline, the
//! gate still exited 0 while the catalog exited 101 and named both halves. The
//! quoted SQL fragment is now lexed without discarding its commas, and every
//! top-level action must independently match the neutral allow-list.
//!
//! WHAT THE QUOTED-BODY SCAN COVERS. `DO 'BEGIN CREATE TABLE … ; END'` used to
//! be on the list above: the scan ran on `Tok::Body` alone, so a single-quoted
//! body was never looked inside, and measured against a migrated database that
//! one form defeated BOTH halves of the control at once, leaving a live
//! `medical_certificate_no` column classified by nothing. Two mechanisms close
//! it, and they cover different amounts:
//!
//! * The scan at the top of `apply_statement` reads `Tok::Str` as well as
//!   `Tok::Body`. A body whose text builds table DDL is `unsupported-ddl` under
//!   either quoting, under whatever head carries it. It is written below the
//!   head for a measured reason: the first attempt was written AT the `DO` head,
//!   and `CREATE FUNCTION f() RETURNS void AS 'BEGIN ALTER TABLE … END'
//!   LANGUAGE plpgsql` — the identical block, one head over — passed the gate
//!   and the catalog assertion both, EXIT=0 each, over a live column.
//! * The `["do", ..]` arm additionally refuses ANY `Tok::Str`, DDL or not, so
//!   `DO 'BEGIN PERFORM 1; END'` fails too although it carries nothing for the
//!   scan to find.
//!
//! That refusal-by-quoting is NOT extended to `CREATE FUNCTION`: a function
//! statement's literals are not all bodies. `0064_platform_group_accounts.sql`
//! writes `DEFAULT ARRAY['MEMBER']` and `DEFAULT 'GROUP_ADMIN'` in a parameter
//! list, and refusing on quoting there would fail the gate on a statement that
//! creates no column. So `do` is judged on its quoting, every other head on what
//! its literals say — and a body that never spells a keyword whole, the fifth
//! bullet above, is read by neither. See `apply_statement`.
//!
//! Distinct from those, and NOT a blind spot: a spelling `apply_statement` does
//! not recognise at all — `CREATE UNLOGGED TABLE`, `CREATE TEMP TABLE`,
//! `CREATE SCHEMA x CREATE TABLE …` — raises `unsupported-ddl` and FAILS. The
//! gate cannot read those, and says so loudly instead of passing. See
//! UNPARSEABLE MEANS FAIL below.
//!
//! WHERE THE CATALOG ASSERTION IS BLIND, so that neither side is read as total:
//!
//! * **Only the relations it can enumerate.** This is the whole of what is left
//!   on that side for the column-membership class, and it is a question about
//!   WHICH RELATIONS, not about syntax.
//! * **Anything not yet applied.** It needs a migrated database, so it says
//!   nothing until CI has stood one up. This gate runs in milliseconds with no
//!   database, on a developer's machine, before the push.
//! * **The retention reader is deliberately aligned, not a second scope.**
//!   `personal_data_columns()` now takes this same non-`pg_catalog`,
//!   non-temporary `r/p/m/f` universe and returns schema-qualified identities.
//!   PostgreSQL integration tests plant sensitive markers in a non-public
//!   table, a materialized view, and a foreign table; each must raise the
//!   derived floor. A future predicate change on either reader must preserve
//!   that equality.
//! * **A relation created at RUNTIME rather than by a migration.** Neither
//!   check sees one: it is in no migration text and in no migrated catalog.
//!   THIS ALREADY SHIPS. `0005_create_compliance_location_store.sql:90-121`
//!   defines `location_pings_create_day_partition()`, which runs
//!   `EXECUTE format('CREATE TABLE IF NOT EXISTS %I PARTITION OF location_pings
//!   …')` (0005:103), and `location_pings_ensure_partition()` (0005:114), which
//!   calls it once per day. All ten columns of `location_pings` are
//!   `pd:personal — 개인위치정보` (0211:306-315), but every ping row lands in a
//!   partition that exists in neither reader: not in the migration text, whose
//!   partition name is computed at runtime, and not in a catalog built from
//!   `./migrations`, which holds only the partitions the migration itself made.
//!   The parent's classification is the only thing standing there, and no check
//!   here proves the child inherited it.
//!
//! * **Below superuser catalog-write, nothing there defends.** Not a blind spot
//!   in the sweep but the floor under it: a migrating session with superuser can
//!   `INSERT INTO pg_description` and forge a `pd:` marker, so an assertion that
//!   READS the catalog has nothing left to read correctly. Production migrates
//!   as `console_app` (`NOSUPERUSER`), which is refused at every step of that
//!   with `42501`/`42939`; the pgtest and CI containers migrate as the initdb
//!   superuser, which is why the probes land there. Stated in full in the module
//!   doc of `personal_data_classification.rs`; not repeated here.
//!
//! That list was short two entries until 2026-08, and neither was a divergence.
//! One was a bare predicate: `application_columns` filtered
//! `n.nspname <> 'information_schema'`, explained nowhere. As the initdb
//! superuser — who migrates in the pgtest and CI containers, though not the
//! `console_app` role production uses — a planted
//! `information_schema.pd_leak (rrn, employee_name)` left two live columns and
//! catalog counts byte-identical to a clean tree. The predicate is deleted, not
//! documented: PostgreSQL 18.4 ships four real tables there under
//! `relkind IN ('r','p','m','f')` (`sql_features`, `sql_implementation_info`,
//! `sql_parts`, `sql_sizing`; 21 columns, none commented; the rest of the schema
//! is `relkind = 'v'`), and those are declared in the catalog side's baseline
//! like any other unclassified table. Replanted after the fix, that plant gives
//! **catalog EXIT=101** naming the table and both columns.
//!
//! The other was `n.nspname NOT LIKE 'pg\_%'`, justified by the claim that the
//! prefix is reserved so nothing can hide behind it. `allow_system_table_mods`
//! is a `superuser`-context GUC and setting it lifts that reservation, so a
//! planted `pg_evil.pd_leak (rrn, employee_name)` holding a 주민등록번호 gave
//! gate EXIT=0 and catalog EXIT=0 with output byte-identical to a clean tree.
//! Deleted rather than documented, for zero baseline entries: the sweep now
//! reads `n.nspname <> 'pg_catalog' AND c.relpersistence <> 't'`, which is
//! stricter than the name enumeration it replaces. Replanted after the fix:
//! **catalog EXIT=101**.
//!
//! WHAT THIS GATE ENFORCES, WITHIN THOSE LIMITS. Every column the parser
//! believes a migration created, in a table NOT listed in the baseline, must
//! carry a `COMMENT ON COLUMN <table>.<column> IS 'pd:<tokens> …'` marker whose
//! tokens all come from a closed vocabulary. The baseline is the declared,
//! shrink-only backlog of tables nobody has classified yet. See
//! `BASELINE_FROZEN_AFTER_MIGRATION` for how "never join it" is enforced with
//! no access to the previous baseline.
//!
//! UNPARSEABLE MEANS FAIL, AND THAT IS THE DEFAULT, NOT A LIST. A table whose
//! DDL the parser cannot read has no columns to check, and a table with no
//! columns to check would pass — so an unsupported construct is a silent hole
//! in exactly the guarantee this gate exists to give. The first attempt at
//! closing it enumerated the constructs known to be dangerous, which is the
//! same fail-open shape one step further along: the next unlisted spelling
//! walks past. So `apply_statement` has NO fallthrough arm. Every statement the
//! parser does not positively recognise is `UnsupportedDdl` and fails, named.
//! The recognised set is the allow-list; see `apply_statement`.
//!
//! WHAT THIS DOES NOT DO, stated here because a gate whose scope is guessed at
//! is worse than none. It asserts nothing about whether any statutory
//! obligation is met, and moves no compliance control out of HOLD. A
//! classification is an INPUT to that question. Deciding that a given column is
//! 민감정보 under 개인정보 보호법 제23조제1항 is a legal judgement made by a
//! human and recorded in the migration; this gate only checks that the
//! judgement was recorded, is spelled from the closed vocabulary, and points at
//! a column that exists.
//!
//! WHY `COMMENT ON COLUMN` AND NOT A REGISTRY TABLE OR A MANIFEST. Drift has
//! two directions and both must be mechanical, not "discouraged":
//!
//! * classification points at a column that does not exist — Postgres closes
//!   this itself. `COMMENT ON COLUMN foo.bar` where `bar` is absent raises
//!   `ERROR: column "bar" of relation "foo" does not exist` and the migration
//!   aborts. No registry table can have a foreign key into `pg_attribute`, so
//!   a stale registry row is silent forever. This gate re-checks the same
//!   condition as defence in depth.
//! * column exists, classification absent — that is this gate.
//!
//! `ALTER TABLE ... DROP COLUMN` silently discards the column's comment, so a
//! drop-and-recreate loses the classification. That case lands as a column
//! present with no marker, which both this gate and the catalog assertion
//! reject.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

/// Baseline path relative to the workspace root.
pub const BASELINE_RELATIVE_PATH: &str =
    "backend/ci/gates/personal-data-classification/unclassified-tables.txt";

/// Highest migration number that existed when the baseline was frozen.
///
/// This is what makes the baseline SHRINK-ONLY rather than merely say it is.
/// The gate has no memory of the previous baseline — CI checks out one commit
/// with no history (`actions/checkout` without `fetch-depth: 0`), so there is
/// no earlier version to diff against. Migration numbers supply the missing
/// clock: a column introduced at or before this number already existed when
/// the backlog was declared and may be sheltered by it; a column introduced
/// after it may not.
///
/// That closes appending. Every table outside the baseline today is fully
/// classified, so naming one here already fails `BaselineEntryFullyClassified`,
/// and a name that matches nothing fails `BaselineEntryUnknownTable`. The only
/// remaining way to grow the backlog was a NEW table — or a new column on an
/// already-listed table, which the table-level baseline used to shelter for
/// free. Both now fail.
pub const BASELINE_FROZEN_AFTER_MIGRATION: u32 = 209;

/// Prefix that marks a column comment as a personal-data classification.
pub const MARKER_PREFIX: &str = "pd:";

/// One statement the parser cannot read, waived by name with the reason it is
/// safe to leave unread.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnsupportedWaiver {
    /// Migration file name, without directory.
    pub migration: &'static str,
    /// Substring of the finding that identifies the construct being waived.
    pub construct: &'static str,
    /// Why this unreadable statement introduces no unclassified column.
    pub reason: &'static str,
}

/// Every statement in the existing corpus that the inverted parser cannot read.
///
/// Inverting the dispatch turned 210 migrations into 27 findings; teaching the
/// body scan the same column-neutral `ALTER TABLE` actions the parser already
/// knows (`COLUMN_NEUTRAL_ALTER_ACTIONS`) took that to one. This is the one.
///
/// SHRINK-ONLY, and mechanically: a waiver whose migration is present but whose
/// finding is gone is itself a violation (`WaiverMatchesNothing`), so an entry
/// cannot outlive the statement it excuses. Growing the list is a Rust edit in
/// the same commit as the migration, which is the review surface a data file
/// does not give.
pub const UNSUPPORTED_WAIVERS: &[UnsupportedWaiver] = &[UnsupportedWaiver {
    migration: "0005_create_compliance_location_store.sql",
    construct: "a dollar-quoted body builds `create table`",
    reason: "location_pings_ensure_partition() runs EXECUTE format('CREATE TABLE IF NOT EXISTS \
             %I PARTITION OF location_pings …') with a name computed per day, so no parse can \
             name the relation. A partition declares no columns of its own; it takes \
             location_pings', and location_pings is in the parsed schema and checked like any \
             other table. NOT covered: the partition relations themselves carry no markers, \
             because COMMENT ON COLUMN is per-relation.",
}];

// ---------------------------------------------------------------------------
// Closed vocabulary, each token carrying the instrument it rests on.
// ---------------------------------------------------------------------------

/// One classification token and the instrument that creates the distinction.
///
/// The citation lives here, in executable code, rather than only in prose, so
/// that a rejection message names the instrument a author must read. `verified`
/// is the 시행일자 of the version checked against the official legislation
/// portal (law.go.kr) on 2026-08-01.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClassCitation {
    /// Vocabulary token, e.g. `unique-id`.
    pub token: &'static str,
    /// Instrument name and version.
    pub instrument: &'static str,
    /// Article the classification rests on.
    pub article: &'static str,
    /// 시행일자 of the version verified.
    pub effective: &'static str,
    /// Sub-tokens this class requires, empty when the class takes none.
    pub subtokens: &'static [&'static str],
}

/// 고유식별정보 — 개인정보 보호법 시행령 제19조 is a CLOSED set of exactly four,
/// each pinned to its own defining Act. That is why this is an enum and not an
/// open category, and why a column claiming membership must name which of the
/// four: 고시 제2026-9호 제7조제3항제2호 makes 주민등록번호 unconditional while
/// the other three may be scoped by a documented 위험도 분석, so `rrn` is
/// legally distinct from its three siblings and a bare `unique-id` would erase
/// that distinction.
const UNIQUE_ID_SUBTOKENS: &[&str] = &[
    // 「주민등록법」 제7조의2제1항에 따른 주민등록번호
    "rrn",
    // 「여권법」 제7조제1항제1호에 따른 여권번호
    "passport",
    // 「도로교통법」 제80조에 따른 운전면허의 면허번호
    "driver-license",
    // 「출입국관리법」 제31조제5항에 따른 외국인등록번호
    "arc",
];

/// 민감정보 — unlike 고유식별정보 this list is NOT closed. 개인정보 보호법
/// 제23조제1항 says `사상ㆍ신념 … 성생활 등에 관한 정보` and then adds a
/// 사생활 현저 침해 catch-all, so assigning a column to this class is a legal
/// judgement. Encoding the sub-category makes each judgement auditable per
/// column instead of hiding nine different decisions behind one boolean.
const SENSITIVE_SUBTOKENS: &[&str] = &[
    // 제23조제1항 본문
    "belief",
    "union",
    "political",
    "health",
    "sex-life",
    // 시행령 제18조
    "genetic",
    "criminal-record",
    "biometric-id",
    "race-ethnicity",
];

/// The closed vocabulary. Order is the order the gate prints them in.
pub const VOCABULARY: &[ClassCitation] = &[
    ClassCitation {
        token: "none",
        instrument: "— (no instrument: asserts no personal data is held)",
        article: "—",
        effective: "—",
        subtokens: &[],
    },
    ClassCitation {
        token: "personal",
        instrument: "개인정보 보호법 (법률 제20897호)",
        article: "제2조제1호 (정의), 제21조제1항 (파기: 지체 없이)",
        effective: "2025-10-02 (제21조 unchanged by 법률 제21445호, 시행 2026-09-11)",
        subtokens: &[],
    },
    ClassCitation {
        token: "sensitive",
        instrument: "개인정보 보호법 + 시행령 (대통령령 제36121호)",
        article: "법 제23조제1항, 영 제18조",
        effective: "영 2026-08-20 (영 제18조 개정 2020.8.4; 법 제23조제1항 unchanged)",
        subtokens: SENSITIVE_SUBTOKENS,
    },
    ClassCitation {
        token: "unique-id",
        instrument: "개인정보 보호법 + 시행령 (대통령령 제36121호)",
        article: "법 제24조제1항, 영 제19조",
        effective: "영 2026-08-20 (법 제24조제3항 reworded to 「유출등」 on 2026-09-11; scope unchanged)",
        subtokens: UNIQUE_ID_SUBTOKENS,
    },
    ClassCitation {
        token: "credit",
        instrument: "신용정보의 이용 및 보호에 관한 법률 (법률 제21646호)",
        article: "제2조제2호, 제20조의2제2항 (개보법 제21조제1항을 명시적으로 배제)",
        effective: "2026-08-13 (제20조의2 본조신설 2015.3.11, 개정 2020.2.4 — 현행)",
        subtokens: &[],
    },
    ClassCitation {
        token: "pseudonymous",
        instrument: "개인정보 보호법 (법률 제21445호)",
        article: "제28조의2, 제28조의4제2항ㆍ제3항, 제28조의7 (적용 제외)",
        effective: "2026-09-11 (제28조의7이 제34조제2항까지 적용 제외로 확대)",
        subtokens: &[],
    },
    ClassCitation {
        token: "undeclared",
        instrument: "— (no instrument: an admission, not a classification)",
        article: "—",
        effective: "—",
        subtokens: &[],
    },
];

/// Look a token up in the closed vocabulary.
#[must_use]
pub fn citation_for(token: &str) -> Option<&'static ClassCitation> {
    VOCABULARY.iter().find(|entry| entry.token == token)
}

// ---------------------------------------------------------------------------
// Violations
// ---------------------------------------------------------------------------

/// The distinct ways classification can be wrong or absent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViolationKind {
    /// A column in a non-baselined table carries no `pd:` marker.
    UnclassifiedColumn,
    /// A marker token is outside the closed vocabulary.
    UnknownToken,
    /// `unique-id` or `sensitive` used without its required sub-token, or with
    /// a sub-token outside that class's list.
    BadSubtoken,
    /// A marker names a table or column the migration parse says does not exist.
    MarkerForUnknownColumn,
    /// A marker string starts with `pd:` but carries no tokens at all.
    EmptyMarker,
    /// Two markers classify the same column.
    DuplicateMarker,
    /// A baselined table is now fully classified: the baseline must shrink.
    BaselineEntryFullyClassified,
    /// A baseline entry names a table that no longer exists.
    BaselineEntryUnknownTable,
    /// The baseline is sheltering a column introduced after it was frozen —
    /// a new table, or a new column on an already-listed table.
    BaselineGrew,
    /// A DDL statement the parser cannot read. Fails the gate rather than
    /// yielding a table with nothing to classify.
    UnsupportedDdl,
    /// A waiver in `UNSUPPORTED_WAIVERS` excuses a statement that no longer
    /// exists. Waivers may only shrink.
    WaiverMatchesNothing,
    /// Two migrations claim the same number at or below the freeze — the clock
    /// `BASELINE_FROZEN_AFTER_MIGRATION` reads is a filename, and this is the
    /// filename lying.
    MigrationNumberReused,
    /// A number at or below the freeze has no migration — a vacancy a later
    /// migration could move into and be read as pre-freeze.
    MigrationNumberVacant,
}

impl ViolationKind {
    #[must_use]
    fn label(self) -> &'static str {
        match self {
            Self::UnclassifiedColumn => "unclassified-column",
            Self::UnknownToken => "unknown-token",
            Self::BadSubtoken => "bad-subtoken",
            Self::MarkerForUnknownColumn => "marker-for-unknown-column",
            Self::EmptyMarker => "empty-marker",
            Self::DuplicateMarker => "duplicate-marker",
            Self::BaselineEntryFullyClassified => "baseline-entry-fully-classified",
            Self::BaselineEntryUnknownTable => "baseline-entry-unknown-table",
            Self::BaselineGrew => "baseline-grew",
            Self::UnsupportedDdl => "unsupported-ddl",
            Self::WaiverMatchesNothing => "waiver-matches-nothing",
            Self::MigrationNumberReused => "migration-number-reused",
            Self::MigrationNumberVacant => "migration-number-vacant",
        }
    }
}

/// A single gate finding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Violation {
    /// Which rule was broken.
    pub kind: ViolationKind,
    /// Migration file the finding is anchored to, when there is one.
    pub file: Option<PathBuf>,
    /// Human-readable detail.
    pub detail: String,
}

impl fmt::Display for Violation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.file {
            Some(path) => write!(
                f,
                "[{}] {}: {}",
                self.kind.label(),
                path.display(),
                self.detail
            ),
            None => write!(f, "[{}] {}", self.kind.label(), self.detail),
        }
    }
}

/// Outcome of one gate run.
#[derive(Debug, Default)]
pub struct GateResult {
    /// Findings, empty when the gate passes.
    pub violations: Vec<Violation>,
    /// Columns carrying a well-formed marker.
    pub classified_columns: usize,
    /// Columns in the parsed post-migration schema.
    pub total_columns: usize,
    /// Tables in the parsed post-migration schema.
    pub total_tables: usize,
    /// Tables still listed in the baseline.
    pub baselined_tables: usize,
    /// Statements the parser could not read that a waiver excused.
    pub waived_statements: usize,
}

impl GateResult {
    /// True when nothing was found.
    #[must_use]
    pub fn passed(&self) -> bool {
        self.violations.is_empty()
    }

    fn push(&mut self, kind: ViolationKind, file: Option<PathBuf>, detail: String) {
        self.violations.push(Violation { kind, file, detail });
    }
}

// ---------------------------------------------------------------------------
// Lexer
// ---------------------------------------------------------------------------
//
// One lexer feeds both halves of the gate — the DDL parse that yields the
// column set, and the marker scan that yields the classification set. Two
// separate scanners would be free to disagree about what a string literal or a
// dollar-quoted plpgsql body is, and a gate whose two halves disagree reports
// drift that is its own.

#[derive(Debug, Clone, PartialEq, Eq)]
enum Tok {
    /// Identifier or number, lowercased.
    Word(String),
    /// Single-quoted string body, case preserved.
    Str(String),
    /// A `$tag$ … $tag$` body, verbatim and unparsed.
    ///
    /// It used to be discarded and replaced by an empty string, which made a
    /// table created inside a `DO` block or a plpgsql function body invisible —
    /// one of the two critical fail-open holes. The text is kept so
    /// `body_builds_table_ddl` can refuse to call the body harmless.
    Body(String),
    /// Any other single character.
    Punct(char),
}

impl Tok {
    fn word(&self) -> Option<&str> {
        match self {
            Self::Word(w) => Some(w.as_str()),
            _ => None,
        }
    }

    fn is_punct(&self, ch: char) -> bool {
        matches!(self, Self::Punct(c) if *c == ch)
    }
}

/// Lex SQL into words, string literals, dollar-quoted bodies and punctuation.
///
/// Comments are dropped. A dollar-quoted body is NOT lexed into — it holds
/// plpgsql, and this repo's plpgsql builds DDL with
/// `EXECUTE format('DROP TABLE …')` (0005 does exactly that for
/// `location_pings` day partitions), so reading it as SQL would invent tables
/// and drop real ones. It is kept whole as `Tok::Body` instead of discarded,
/// because a body that is discarded is a body that cannot be refused.
fn lex(sql: &str) -> Vec<Tok> {
    lex_with_completeness(sql).0
}

/// Lex SQL while retaining whether every quoted construct and block comment
/// closed. Ordinary schema parsing preserves its existing token-only behavior;
/// opaque-body recursion uses the second value to fail closed instead of
/// treating a truncated nested fragment as harmless.
fn lex_with_completeness(sql: &str) -> (Vec<Tok>, bool) {
    let bytes = sql.as_bytes();
    let mut tokens = Vec::new();
    let mut complete = true;
    let mut i = 0usize;

    while i < bytes.len() {
        let b = bytes[i];
        let next = bytes.get(i + 1).copied();

        // -- line comment
        if b == b'-' && next == Some(b'-') {
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        // /* block comment */, nestable per the SQL standard and per Postgres.
        if b == b'/' && next == Some(b'*') {
            let mut depth = 1usize;
            i += 2;
            while i < bytes.len() && depth > 0 {
                if bytes[i] == b'/' && bytes.get(i + 1) == Some(&b'*') {
                    depth += 1;
                    i += 2;
                } else if bytes[i] == b'*' && bytes.get(i + 1) == Some(&b'/') {
                    depth -= 1;
                    i += 2;
                } else {
                    i += 1;
                }
            }
            if depth > 0 {
                complete = false;
            }
            continue;
        }
        // $tag$ … $tag$
        if b == b'$'
            && let Some(tag_end) = dollar_tag_end(bytes, i)
        {
            let tag = &sql[i..tag_end];
            let body_start = tag_end;
            let (body_end, closed) = match sql[body_start..].find(tag) {
                Some(offset) => (body_start + offset, true),
                None => (bytes.len(), false),
            };
            complete &= closed;
            i = body_end.saturating_add(tag.len()).min(bytes.len());
            tokens.push(Tok::Body(sql[body_start..body_end].to_owned()));
            continue;
        }
        // 'string', with '' as the embedded quote.
        if b == b'\'' {
            let mut body = String::new();
            let mut closed = false;
            i += 1;
            while i < bytes.len() {
                if bytes[i] == b'\'' {
                    if bytes.get(i + 1) == Some(&b'\'') {
                        body.push('\'');
                        i += 2;
                        continue;
                    }
                    i += 1;
                    closed = true;
                    break;
                }
                let ch_start = i;
                let mut ch_end = i + 1;
                while ch_end < bytes.len() && (bytes[ch_end] & 0xC0) == 0x80 {
                    ch_end += 1;
                }
                body.push_str(&sql[ch_start..ch_end]);
                i = ch_end;
            }
            complete &= closed;
            tokens.push(Tok::Str(body));
            continue;
        }
        // "quoted identifier"
        if b == b'"' {
            let mut body = String::new();
            let mut closed = false;
            i += 1;
            while i < bytes.len() {
                if bytes[i] == b'"' {
                    if bytes.get(i + 1) == Some(&b'"') {
                        body.push('"');
                        i += 2;
                        continue;
                    }
                    i += 1;
                    closed = true;
                    break;
                }
                body.push(bytes[i] as char);
                i += 1;
            }
            complete &= closed;
            tokens.push(Tok::Word(body.to_ascii_lowercase()));
            continue;
        }
        if b.is_ascii_alphanumeric() || b == b'_' {
            let start = i;
            while i < bytes.len()
                && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_' || bytes[i] == b'$')
            {
                i += 1;
            }
            tokens.push(Tok::Word(sql[start..i].to_ascii_lowercase()));
            continue;
        }
        if b.is_ascii_whitespace() {
            i += 1;
            continue;
        }
        tokens.push(Tok::Punct(b as char));
        i += 1;
    }

    (tokens, complete)
}

/// If a `$` at `start` opens a dollar quote, return the index just past its tag.
fn dollar_tag_end(bytes: &[u8], start: usize) -> Option<usize> {
    let mut i = start + 1;
    while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
        i += 1;
    }
    if bytes.get(i) == Some(&b'$') {
        Some(i + 1)
    } else {
        None
    }
}

/// Split a token stream into statements on top-level `;`.
fn statements(tokens: &[Tok]) -> Vec<&[Tok]> {
    let mut out = Vec::new();
    let mut start = 0usize;
    for (index, token) in tokens.iter().enumerate() {
        if token.is_punct(';') {
            if index > start {
                out.push(&tokens[start..index]);
            }
            start = index + 1;
        }
    }
    if start < tokens.len() {
        out.push(&tokens[start..]);
    }
    out
}

// ---------------------------------------------------------------------------
// Schema parse
// ---------------------------------------------------------------------------

/// The post-migration column set, plus where each column was introduced.
#[derive(Debug, Default)]
pub struct Schema {
    /// table -> ordered column names.
    pub tables: BTreeMap<String, BTreeSet<String>>,
    /// (table, column) -> migration file that introduced it.
    pub origins: BTreeMap<(String, String), PathBuf>,
    /// Statements the parser could not read, as (file, named construct).
    ///
    /// THE GATE FAILS OPEN WITHOUT THIS. A table the parser cannot understand
    /// used to become a table with nothing to classify, and a table with
    /// nothing to classify passes. For a gate whose whole purpose is "no
    /// personal-data column goes unclassified", unparseable must mean FAIL.
    pub unsupported: Vec<(PathBuf, String)>,
}

impl Schema {
    /// Total columns across all tables.
    #[must_use]
    pub fn column_count(&self) -> usize {
        self.tables.values().map(BTreeSet::len).sum()
    }

    fn unsupported(&mut self, file: &Path, detail: impl Into<String>) {
        self.unsupported.push((file.to_path_buf(), detail.into()));
    }
}

/// Words that begin a table-level constraint rather than a column definition.
///
/// `like` is deliberately NOT here. `CREATE TABLE x (LIKE parent INCLUDING
/// ALL)` copies every column of `parent`, and treating the clause as a
/// constraint discarded all of them — the table then held zero columns and
/// satisfied the gate trivially. It is now an unsupported construct.
const CONSTRAINT_HEADS: &[&str] = &[
    "constraint",
    "primary",
    "unique",
    "foreign",
    "check",
    "exclude",
];

/// `ALTER TABLE` actions that change access, storage, defaults or constraints
/// but never the column list: `ALTER COLUMN … SET/DROP …`, `ENABLE`/`DISABLE`/
/// `FORCE`/`NO FORCE ROW LEVEL SECURITY`, `VALIDATE CONSTRAINT`, `OWNER TO`.
///
/// Read from two places on purpose — `apply_alter_table` on parsed SQL, and
/// `body_builds_table_ddl` on the raw text of a dollar-quoted body — so the two
/// cannot drift into disagreeing about what a column-neutral action is. `add`,
/// `rename` and a bare `drop` are absent: those are the three that move
/// columns.
const COLUMN_NEUTRAL_ALTER_ACTIONS: &[&str] = &[
    "alter", "enable", "disable", "force", "no", "validate", "owner",
];

/// Parse `CREATE TABLE`, `ALTER TABLE … ADD/DROP COLUMN` and `DROP TABLE`
/// across every migration file, in filename order, into a post-migration schema.
#[must_use]
pub fn parse_schema(files: &[PathBuf]) -> Schema {
    let mut schema = Schema::default();
    for file in files {
        let Ok(content) = fs::read_to_string(file) else {
            continue;
        };
        let tokens = lex(&content);
        for statement in statements(&tokens) {
            apply_statement(statement, file, &mut schema);
        }
    }
    schema
}

/// Dispatch one statement. **There is no fallthrough arm, and that is the fix.**
///
/// The previous shape recognised `CREATE TABLE` / `ALTER TABLE` / `DROP TABLE`
/// plus a hand-list of dangerous constructs, and sent everything else to a
/// silent `_ => {}`. That is fail-open by construction, and the list did not
/// save it: the dispatch required `head[1] == "table"`, so `CREATE UNLOGGED
/// TABLE`, `CREATE TEMP TABLE` and `CREATE GLOBAL TEMPORARY TABLE` put the
/// modifier in slot 1 and walked past; `CREATE SCHEMA x CREATE TABLE …` walked
/// past on `head[1] == "schema"`; and a table built inside a `DO` block walked
/// past because the body had been discarded. The gate then reported green over
/// relations it had never read.
///
/// Inverted, the recognised set below is the whole allow-list and everything
/// else fails. It holds two kinds of form, and the difference is the point:
///
/// * forms that CHANGE a table's column set — parsed, and failing when the
///   parse does not consume them (see `apply_create_table`);
/// * forms that provably create no column. Each is an assertion about SQL, not
///   a convenience: an index, a policy, a trigger, a view, an extension, a
///   grant, a comment and a DML statement all leave every table's column list
///   exactly as they found it.
///
/// The list is deliberately only what this repo's 210 migrations actually use.
/// A construct nobody has written yet is a construct nobody has thought about,
/// and the fail-closed reading of "not thought about" is "reject".
fn apply_statement(statement: &[Tok], file: &Path, schema: &mut Schema) {
    // Before the head is even read: a quoted body is opaque, and the head of
    // the statement carrying it says nothing about what it does.
    //
    // `Tok::Str` is scanned alongside `Tok::Body` because PostgreSQL accepts
    // BOTH quotings for the same body, and an earlier round closed only the
    // head it had measured: `DO '…'` was refused while
    // `CREATE FUNCTION … AS 'BEGIN ALTER TABLE … END' LANGUAGE plpgsql` — the
    // identical block, one head over — still passed. Scanning here, below the
    // head, is what makes the two quotings equal; a refusal written per head
    // has to be written again for every head that can carry a body.
    for token in statement {
        let (body, quoting) = match token {
            Tok::Body(body) => (body, "dollar-quoted"),
            Tok::Str(body) => (body, "single-quoted"),
            _ => continue,
        };
        match body_builds_table_ddl(body) {
            Ok(Some(construct)) => schema.unsupported(
                file,
                format!(
                    "a {quoting} body builds `{construct}` — DDL inside plpgsql is not read by \
                     this parser, so the columns it creates cannot be proved classified"
                ),
            ),
            Ok(None) => {}
            Err(reason) => schema.unsupported(
                file,
                format!("a {quoting} body cannot be inspected safely — {reason}"),
            ),
        }
    }

    let head: Vec<&str> = statement.iter().filter_map(Tok::word).take(4).collect();
    match head.as_slice() {
        // ---- forms that change the column set: parsed ----
        ["create", "table", ..] => apply_create_table(statement, file, schema),
        ["alter", "table", ..] => apply_alter_table(statement, file, schema),
        ["drop", "table", ..] => {
            for table in drop_table_targets(statement) {
                schema.tables.remove(&table);
                schema.origins.retain(|(t, _), _| t != &table);
            }
        }

        // ---- forms that create no column ----
        // `COMMENT ON …` is read separately by `parse_markers`; the rest touch
        // access, execution or rows, never a column list.
        ["comment", "on", ..]
        | ["create", "index", ..]
        | ["create", "unique", "index", ..]
        | ["create", "trigger", ..]
        | ["create", "constraint", "trigger", ..]
        | ["create", "policy", ..]
        | ["create", "extension", ..]
        | ["create", "function", ..]
        | ["create", "or", "replace", "function"]
        | ["create", "view", ..]
        | ["alter", "function", ..]
        | ["alter", "default", "privileges", ..]
        | ["drop", "index", ..]
        | ["drop", "function", ..]
        | ["drop", "trigger", ..]
        | ["lock", "table", ..]
        | ["set", "local", "lock_timeout", ..]
        | ["grant", ..]
        | ["revoke", ..] => {}

        // `DO` runs an anonymous block, and the block is where DDL hides. A
        // `$$ … $$` body arrives as `Tok::Body`; the SAME block written
        // `DO 'BEGIN … END'` arrives as `Tok::Str`. The scan at the top of this
        // function now reads both, which is where the quoting stopped mattering
        // — and where it had to be fixed, because writing the fix here left
        // `CREATE FUNCTION … AS 'BEGIN ALTER TABLE … END'` passing one head
        // over, measured EXIT=0 on both halves of the control.
        //
        // This arm adds what the scan cannot: it refuses a `DO` carrying ANY
        // `Tok::Str` whatever the literal says, so `DO 'BEGIN PERFORM 1; END'`
        // — no DDL in it for the scan to find — fails too, as does any spelling
        // of the literal the scan reads but does not understand. It stays
        // scoped to `do` because a `DO` literal IS the program, whereas
        // `CREATE FUNCTION`'s literals include parameter defaults (0064 writes
        // `DEFAULT ARRAY['MEMBER']`, `DEFAULT 'GROUP_ADMIN'`), so the same rule
        // one head over would fail the gate on a statement that creates no
        // column. No migration here writes the form — all 52 `DO` statements
        // are `$$`/`$block$` — so it costs nothing today.
        //
        // NOT covered by this arm: a DOLLAR-quoted `DO` body, which carries no
        // `Tok::Str` and is judged only by the scan. A body that assembles its
        // keywords from fragments (`EXECUTE 'ALTER TA' || 'BLE …'`) passes the
        // scan and passes here. See the crate doc's blind-spot list.
        //
        // ponytail: the test is "a `DO` statement carrying ANY `Tok::Str`", so
        // the legacy `DO $$ … $$ LANGUAGE 'plpgsql'` would be refused too even
        // though its literal is a language name. Known ceiling, unused here,
        // and it errs closed. Upgrade path is to look only at the token that
        // follows `do`/`language`, which is more parser than this is worth.
        ["do", ..] => {
            if statement.iter().any(|t| matches!(t, Tok::Str(_))) {
                schema.unsupported(
                    file,
                    "DO with a single-quoted body — a `DO` literal is the whole program and this \
                     parser reads no plpgsql, so the block is refused on its quoting rather than \
                     on what it says. Write it as `DO $$ … $$`",
                );
            }
        }

        // `CREATE SCHEMA` is column-neutral only in its bare form. The standard
        // also admits a schema element list — `CREATE SCHEMA x CREATE TABLE
        // y (…)` — which creates tables the old dispatch never saw, because it
        // tested `head[1] == "table"` and this statement heads with `schema`.
        ["create", "schema", ..] => {
            if statement.iter().skip(1).any(|t| t.word() == Some("create")) {
                schema.unsupported(
                    file,
                    "CREATE SCHEMA … CREATE TABLE … creates tables inside a schema element list",
                );
            }
        }

        // DML. The only DML that creates a relation is `SELECT … INTO target`;
        // `INSERT INTO` is the other spelling of the same word and creates
        // nothing. plpgsql's `SELECT … INTO variable` is not reachable here —
        // it lives in a body, which is never lexed as SQL.
        ["select", ..] | ["with", ..] | ["insert", ..] | ["update", ..] => {
            if let Some(target) = select_into_target(statement) {
                schema.unsupported(
                    file,
                    format!("SELECT … INTO {target} creates a table from a query"),
                );
            }
        }

        [] => {}
        _ => schema.unsupported(
            file,
            format!(
                "unrecognised statement `{}` — the parser cannot tell whether it changes a \
                 table's column set",
                head.join(" ")
            ),
        ),
    }
}

/// The target of a relation-creating `INTO`, or `None`.
///
/// `INSERT INTO t` is excluded by its preceding word, which is the only reason
/// `into` alone cannot be the test.
fn select_into_target(statement: &[Tok]) -> Option<String> {
    let mut previous_word: Option<&str> = None;
    for (index, token) in statement.iter().enumerate() {
        if let Some(word) = token.word() {
            if word == "into" && previous_word != Some("insert") {
                return read_qualified_name(statement, index + 1).map(|(name, _)| name);
            }
            previous_word = Some(word);
        }
    }
    None
}

/// Does an opaque quoted body build table DDL?
///
/// Called on every `Tok::Body` AND every `Tok::Str` in a statement, because
/// PostgreSQL accepts both quotings for the same plpgsql body and a check that
/// reads only one of them is a check written against a spelling.
///
/// A body is plpgsql, this gate has no plpgsql parser, and under the inverted
/// default a statement that is not consumed fails. What the gate can still do
/// is refuse to call a body harmless. The scan runs over the body's whole text,
/// code and string literals alike, because this repo builds DDL by string —
/// `EXECUTE format('CREATE TABLE %I …')` — so the DDL is inside a literal and a
/// scan that skipped literals would see nothing. Every single- or dollar-quoted
/// fragment is lexed once and inspected independently; adjacent fragments are
/// never joined, preserving the registered concatenation-split ceiling. The
/// local signal and classifier share one projection of the resulting
/// `Tok::Word`s, so quoted contents cannot spend one word window while being
/// absent from the other. Every quoted child is still queued even when its
/// parent has no local signal, because complete nested commands can sit behind
/// an otherwise-neutral wrapper.
///
/// Completeness is carried down that worklist. A local or descendant signal
/// beneath an incomplete quote/comment fails closed. Completely signal-free
/// decoded data may remain incomplete when interpreted as fresh SQL —
/// `O'Reilly`, `/* not a comment`, or `$tag$` are data, not malformed commands
/// in their containing statement — and is ignored only after all of its quoted
/// children have also proved signal-free.
///
/// A leading `DROP … TABLE` is deliberately not flagged. A body that drops a
/// table leaves the parser believing in a table that is gone, so the gate
/// demands one classification too many: the fail-closed direction, not a hole.
///
/// `ALTER … TABLE` is flagged unless the action that follows is one
/// `COLUMN_NEUTRAL_ALTER_ACTIONS` names — the same vocabulary
/// `apply_alter_table` applies to parsed SQL. Without that, the repo's standard
/// RLS-arming idiom (`EXECUTE format('ALTER TABLE %I ENABLE ROW LEVEL
/// SECURITY', t)`, in 25 migrations) would need a waiver every time it is
/// written, and a gate that demands a waiver for the house idiom is a gate
/// someone eventually deletes.
///
/// ponytail: token/word-window scan, not a plpgsql parse. Known ceiling — a body that
/// assembles the keyword from fragments (`'CREA' || 'TE TABLE'`,
/// `'ALTER TA' || 'BLE …'`), or one whose action verb sits more than three words
/// past `TABLE`, is not read correctly; the second case errs closed. The first
/// is a measured blind spot of the whole gate, not a private note: it is the
/// fifth bullet in the crate doc's list, and it is repeated in
/// `docs/CI-GATES.md` and `unclassified-tables.txt` so the residual stated there
/// is the residual this code has. The upgrade path is a real plpgsql parser,
/// which nothing here justifies.
///
/// The explicit worklist avoids recursive stack growth. Every nested item must
/// be strictly smaller in bytes than its parent. Fixed item and total-byte
/// budgets make even branching hostile input finite; exhausting either budget
/// or observing a non-shrinking item is an error the caller records as
/// unsupported DDL.
fn body_builds_table_ddl(body: &str) -> Result<Option<String>, &'static str> {
    const MAX_WORK_ITEMS: usize = 8192;
    const MAX_LEXED_BYTES: usize = 256 * 1024 * 1024;

    struct WorkItem {
        fragment: String,
        parent_size: Option<usize>,
        ancestor_incomplete: bool,
    }

    let mut pending = vec![WorkItem {
        fragment: body.to_owned(),
        parent_size: None,
        ancestor_incomplete: false,
    }];
    let mut work_items = 0usize;
    let mut lexed_bytes = 0usize;
    while let Some(WorkItem {
        fragment,
        parent_size,
        ancestor_incomplete,
    }) = pending.pop()
    {
        if parent_size.is_some_and(|size| fragment.len() >= size) {
            return Err("a nested quoted fragment did not strictly shrink");
        }
        work_items = work_items
            .checked_add(1)
            .ok_or("opaque fragment inspection exceeded its work-item limit")?;
        if work_items > MAX_WORK_ITEMS {
            return Err("opaque fragment inspection exceeded its work-item limit");
        }
        lexed_bytes = lexed_bytes
            .checked_add(fragment.len())
            .ok_or("opaque fragment inspection exceeded its byte limit")?;
        if lexed_bytes > MAX_LEXED_BYTES {
            return Err("opaque fragment inspection exceeded its byte limit");
        }

        let fragment_size = fragment.len();
        let (tokens, complete) = lex_with_completeness(&fragment);
        let incomplete = ancestor_incomplete || !complete;
        let mut local_signal = false;
        let mut unsafe_construct = None;
        for statement in statements(&tokens) {
            let inspection = inspect_token_stream_table_ddl(statement)?;
            local_signal |= inspection.signaled;
            if unsafe_construct.is_none() {
                unsafe_construct = inspection.unsafe_construct;
            }
        }
        if local_signal && incomplete {
            return Err("a nested quoted fragment or comment is incomplete");
        }
        if let Some(construct) = unsafe_construct {
            return Ok(Some(construct));
        }

        // DDL assembled by `EXECUTE format('ALTER TABLE …', ...)` or by a
        // complete dollar-quoted command fragment lives in an opaque token.
        // Push in reverse so the worklist still inspects fragments in lexical
        // order, one at a time, without ever joining neighbors.
        for token in tokens.into_iter().rev() {
            if let Tok::Str(nested) | Tok::Body(nested) = token {
                if nested.len() >= fragment_size {
                    return Err("a nested quoted fragment did not strictly shrink");
                }
                pending.push(WorkItem {
                    fragment: nested,
                    parent_size: Some(fragment_size),
                    ancestor_incomplete: incomplete,
                });
            }
        }
    }
    Ok(None)
}

struct TableDdlInspection {
    signaled: bool,
    unsafe_construct: Option<String>,
}

struct TableDdlLocation {
    creates: bool,
    after_table: usize,
    phrase: String,
}

enum TableDdlLocations {
    None,
    One(TableDdlLocation),
    Multiple,
}

/// Locate zero or one local CREATE/ALTER-to-TABLE phrase from one exact token
/// projection. A second location stops the scan immediately: one statement
/// slice cannot safely assign the first phrase's action tail when another table
/// command follows without a semicolon. Signal admission and classification
/// consume this same result, so neither can have a wider word vocabulary or
/// modifier window.
fn locate_table_ddl(tokens: &[Tok]) -> TableDdlLocations {
    let words: Vec<(usize, &str)> = tokens
        .iter()
        .enumerate()
        .filter_map(|(index, token)| token.word().map(|word| (index, word)))
        .collect();
    let mut location = None;
    for (index, (_, word)) in words.iter().enumerate() {
        let creates = word.eq_ignore_ascii_case("create");
        if !creates && !word.eq_ignore_ascii_case("alter") {
            continue;
        }
        // Wide enough for `CREATE GLOBAL TEMPORARY TABLE`, the longest
        // modifier run Postgres accepts before the keyword.
        let window_end = index.saturating_add(4).min(words.len());
        let Some(offset) = words[index + 1..window_end]
            .iter()
            .position(|(_, word)| word.eq_ignore_ascii_case("table"))
        else {
            continue;
        };
        let table_word = index + 1 + offset;
        if location.is_some() {
            return TableDdlLocations::Multiple;
        }
        let phrase = words[index..=table_word]
            .iter()
            .map(|(_, word)| *word)
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase();
        location = Some(TableDdlLocation {
            creates,
            after_table: words[table_word].0 + 1,
            phrase,
        });
    }
    match location {
        Some(location) => TableDdlLocations::One(location),
        None => TableDdlLocations::None,
    }
}

fn inspect_token_stream_table_ddl(tokens: &[Tok]) -> Result<TableDdlInspection, &'static str> {
    let location = match locate_table_ddl(tokens) {
        TableDdlLocations::None => {
            return Ok(TableDdlInspection {
                signaled: false,
                unsafe_construct: None,
            });
        }
        TableDdlLocations::One(location) => location,
        TableDdlLocations::Multiple => {
            return Err("an opaque statement contains multiple table DDL locations");
        }
    };
    // Any `CREATE … TABLE` makes a relation whose name this parser never
    // learns — 0005 computes a partition name per day. The sole ALTER location
    // is classified once through the end of this semicolon-delimited slice.
    let unsafe_construct = if location.creates
        || !opaque_alter_actions_are_column_neutral(&tokens[location.after_table..])
    {
        Some(location.phrase)
    } else {
        None
    };
    Ok(TableDdlInspection {
        signaled: true,
        unsafe_construct,
    })
}

/// `tokens` begins just past `ALTER … TABLE`. The table name is one to three
/// words (`%I`, `foo`, `public.foo` after lexing), so the action verb is searched
/// for rather than assumed to sit first. Once found, every top-level
/// comma-separated action must independently match the neutral allow-list.
fn opaque_alter_actions_are_column_neutral(tokens: &[Tok]) -> bool {
    let action_start = tokens
        .iter()
        .enumerate()
        .filter_map(|(index, token)| token.word().map(|word| (index, word)))
        .take(3)
        .find(|(_, word)| {
            COLUMN_NEUTRAL_ALTER_ACTIONS.contains(word)
                || matches!(*word, "add" | "drop" | "rename")
        })
        .map(|(index, _)| index);
    let Some(action_start) = action_start else {
        return false;
    };
    let action_tokens = &tokens[action_start..];
    let Some(expected_actions) = top_level_action_count(action_tokens) else {
        return false;
    };
    let actions = split_top_level_commas(action_tokens);
    actions.len() == expected_actions && actions.into_iter().all(alter_action_is_column_neutral)
}

fn top_level_action_count(tokens: &[Tok]) -> Option<usize> {
    if tokens.is_empty() {
        return None;
    }
    let mut depth = 0usize;
    let mut commas = 0usize;
    for token in tokens {
        if token.is_punct('(') {
            depth += 1;
        } else if token.is_punct(')') {
            depth = depth.checked_sub(1)?;
        } else if token.is_punct(',') && depth == 0 {
            commas += 1;
        }
    }
    (depth == 0).then_some(commas + 1)
}

fn alter_action_is_column_neutral(action: &[Tok]) -> bool {
    let mut words = action.iter().filter_map(Tok::word);
    let Some(head) = words.next() else {
        return false;
    };
    if COLUMN_NEUTRAL_ALTER_ACTIONS.contains(&head) {
        return true;
    }
    // `ADD`/`DROP CONSTRAINT` is neutral, `ADD`/`DROP COLUMN` is not, and a
    // bare `ADD colname TYPE` is the implicit column form: the same distinction
    // `apply_alter_table` draws on parsed SQL, drawn here on opaque text.
    matches!(head, "add" | "drop")
        && words
            .next()
            .is_some_and(|next| CONSTRAINT_HEADS.contains(&next))
}

/// Consume `[IF NOT EXISTS]` / `[IF EXISTS]` and return the index of the name.
fn skip_if_clause(statement: &[Tok], mut index: usize) -> usize {
    while let Some(word) = statement.get(index).and_then(Tok::word) {
        if matches!(word, "if" | "not" | "exists" | "only") {
            index += 1;
        } else {
            break;
        }
    }
    index
}

/// Read a possibly schema-qualified name and return `(name, next_index)`.
fn read_qualified_name(statement: &[Tok], index: usize) -> Option<(String, usize)> {
    let mut name = statement.get(index)?.word()?.to_owned();
    let mut cursor = index + 1;
    while statement.get(cursor).is_some_and(|t| t.is_punct('.')) {
        if let Some(part) = statement.get(cursor + 1).and_then(Tok::word) {
            name = part.to_owned();
            cursor += 2;
        } else {
            break;
        }
    }
    Some((name, cursor))
}

/// Parse one `CREATE TABLE`, or name the construct that defeated the parser.
///
/// Every early return here used to be silent, and a silent return meant the
/// table was never registered at all — so it could not be checked, and the
/// gate passed. Each is now an `UnsupportedDdl` finding naming the construct.
fn apply_create_table(statement: &[Tok], file: &Path, schema: &mut Schema) {
    let index = skip_if_clause(statement, 2);
    let Some((table, after_name)) = read_qualified_name(statement, index) else {
        schema.unsupported(file, "CREATE TABLE with no readable table name");
        return;
    };
    match statement.get(after_name).and_then(Tok::word) {
        // `CREATE TABLE x PARTITION OF y` takes its parent's whole column set.
        // This repo builds partitions only inside plpgsql (0005), which the
        // lexer drops, so a hand-written one is a construct nobody has taught
        // the gate to resolve — not a table with no columns.
        Some("partition") => {
            schema.unsupported(
                file,
                format!("CREATE TABLE {table} PARTITION OF … inherits its parent's columns"),
            );
            return;
        }
        // `CREATE TABLE x AS SELECT …` — the column set comes from the query.
        Some("as") => {
            schema.unsupported(
                file,
                format!("CREATE TABLE {table} AS SELECT … takes its columns from a query"),
            );
            return;
        }
        _ => {}
    }
    let Some((open, close)) = parenthesised_span(statement, after_name) else {
        schema.unsupported(
            file,
            format!("CREATE TABLE {table} has no balanced ( … ) column list"),
        );
        return;
    };
    // `INHERITS (parent)` follows the column list and adds the parent's
    // columns to this table.
    if statement[close + 1..]
        .iter()
        .any(|t| t.word() == Some("inherits"))
    {
        schema.unsupported(
            file,
            format!("CREATE TABLE {table} … INHERITS (…) adds its parent's columns"),
        );
        return;
    }
    let entry = schema.tables.entry(table.clone()).or_default();
    let mut copied_from_elsewhere = false;
    for item in split_top_level_commas(&statement[open + 1..close]) {
        let Some(first) = item.first().and_then(Tok::word) else {
            continue;
        };
        // `LIKE parent INCLUDING ALL` copies parent's columns into this table.
        if first == "like" {
            copied_from_elsewhere = true;
            continue;
        }
        if CONSTRAINT_HEADS.contains(&first) {
            continue;
        }
        entry.insert(first.to_owned());
        schema
            .origins
            .insert((table.clone(), first.to_owned()), file.to_path_buf());
    }
    if copied_from_elsewhere {
        schema.unsupported(
            file,
            format!("CREATE TABLE {table} (LIKE … ) copies another table's columns"),
        );
    } else if schema.tables.get(&table).is_some_and(BTreeSet::is_empty) {
        // Catch-all for a form nobody enumerated: a real table always declares
        // at least one column, so zero columns means the parser missed them.
        schema.unsupported(
            file,
            format!("CREATE TABLE {table} yielded no columns — the column list did not parse"),
        );
    }
}

fn apply_alter_table(statement: &[Tok], file: &Path, schema: &mut Schema) {
    let index = skip_if_clause(statement, 2);
    let Some((table, after_name)) = read_qualified_name(statement, index) else {
        schema.unsupported(file, "ALTER TABLE with no readable table name");
        return;
    };
    if !schema.tables.contains_key(&table) {
        schema.unsupported(
            file,
            format!("ALTER TABLE {table} names a table no parsed migration creates"),
        );
        return;
    }
    // Each comma-separated action is parsed independently. 0066 uses the
    // multi-`ADD COLUMN` form (twelve columns in one statement); a parser that
    // reads only the first action undercounts `employees` by eleven.
    for action in split_top_level_commas(&statement[after_name..]) {
        let words: Vec<&str> = action.iter().take(6).filter_map(Tok::word).collect();
        match words.first().copied() {
            Some("add") => {
                let mut cursor = 1usize;
                if action.get(cursor).and_then(Tok::word) == Some("column") {
                    cursor += 1;
                } else if action
                    .get(cursor)
                    .and_then(Tok::word)
                    .is_some_and(|w| CONSTRAINT_HEADS.contains(&w))
                {
                    continue;
                }
                cursor = skip_if_clause(action, cursor);
                if let Some(column) = action.get(cursor).and_then(Tok::word) {
                    schema
                        .tables
                        .entry(table.clone())
                        .or_default()
                        .insert(column.to_owned());
                    schema
                        .origins
                        .insert((table.clone(), column.to_owned()), file.to_path_buf());
                }
            }
            Some("drop") => {
                let mut cursor = 1usize;
                if action.get(cursor).and_then(Tok::word) == Some("column") {
                    cursor += 1;
                } else if action
                    .get(cursor)
                    .and_then(Tok::word)
                    .is_some_and(|w| CONSTRAINT_HEADS.contains(&w))
                {
                    continue;
                } else {
                    schema.unsupported(
                        file,
                        format!(
                            "ALTER TABLE {table} DROP {} is not a form this parser reads",
                            words.get(1).copied().unwrap_or("…")
                        ),
                    );
                    continue;
                }
                cursor = skip_if_clause(action, cursor);
                if let Some(column) = action.get(cursor).and_then(Tok::word)
                    && let Some(columns) = schema.tables.get_mut(&table)
                {
                    columns.remove(column);
                    schema.origins.remove(&(table.clone(), column.to_owned()));
                }
            }
            // Actions that change access, storage, defaults or constraints, but
            // never the column list: `ALTER COLUMN … SET/DROP …`, `ENABLE`/
            // `DISABLE`/`FORCE`/`NO FORCE ROW LEVEL SECURITY`, `VALIDATE
            // CONSTRAINT`, `OWNER TO`. Same allow-list discipline as
            // `apply_statement`: named, or rejected.
            Some(action) if COLUMN_NEUTRAL_ALTER_ACTIONS.contains(&action) => {}
            // `RENAME COLUMN a TO b` is rejected because THIS PARSER would lose
            // the column, not because PostgreSQL does. An earlier version of
            // this comment said the marker keeps naming `a`; that is false, and
            // execution says so: `pg_description` is keyed on
            // `(objoid, objsubid)` where `objsubid` is `attnum`, and a rename
            // touches neither, so the comment follows the column to `b`
            // untouched. The catalog side is therefore fine. What breaks is
            // here: this gate models a table as a set of NAMES, so after a
            // rename it would hold `a` — a column that no longer exists — and
            // not `b`, whose marker it would then read as pointing at nothing.
            // Rejecting is the fail-closed reading of a text model that cannot
            // follow the identity Postgres actually tracks. No migration uses
            // the form today; the day one does, the gate says so instead of
            // quietly reporting the old column set.
            Some("rename") => {
                schema.unsupported(
                    file,
                    format!(
                        "ALTER TABLE {table} RENAME … — this parser tracks columns by name, so it \
                         would keep the old one and never see the new"
                    ),
                );
            }
            // No fallthrough. `ATTACH PARTITION` and `INHERIT parent` both
            // widen a table's effective column set, and both used to land here
            // silently.
            _ => schema.unsupported(
                file,
                format!(
                    "ALTER TABLE {table} {} is not an action this parser reads",
                    words.first().copied().unwrap_or("…")
                ),
            ),
        }
    }
}

fn drop_table_targets(statement: &[Tok]) -> Vec<String> {
    let index = skip_if_clause(statement, 2);
    let mut targets = Vec::new();
    let mut cursor = index;
    while let Some((name, next)) = read_qualified_name(statement, cursor) {
        targets.push(name);
        if statement.get(next).is_some_and(|t| t.is_punct(',')) {
            cursor = next + 1;
        } else {
            break;
        }
    }
    targets
}

/// Return `(open, close)` indices of the first balanced `( … )` at or after
/// `from`. `None` when there is no `(`, or when it is never closed.
fn parenthesised_span(statement: &[Tok], from: usize) -> Option<(usize, usize)> {
    let open = (from..statement.len()).find(|i| statement[*i].is_punct('('))?;
    let mut depth = 0usize;
    for (index, token) in statement.iter().enumerate().skip(open) {
        if token.is_punct('(') {
            depth += 1;
        } else if token.is_punct(')') {
            depth -= 1;
            if depth == 0 {
                return Some((open, index));
            }
        }
    }
    None
}

fn split_top_level_commas(tokens: &[Tok]) -> Vec<&[Tok]> {
    let mut out = Vec::new();
    let mut depth = 0usize;
    let mut start = 0usize;
    for (index, token) in tokens.iter().enumerate() {
        if token.is_punct('(') {
            depth += 1;
        } else if token.is_punct(')') {
            depth = depth.saturating_sub(1);
        } else if token.is_punct(',') && depth == 0 {
            if index > start {
                out.push(&tokens[start..index]);
            }
            start = index + 1;
        }
    }
    if start < tokens.len() {
        out.push(&tokens[start..]);
    }
    out
}

// ---------------------------------------------------------------------------
// Marker parse
// ---------------------------------------------------------------------------

/// One `COMMENT ON COLUMN … IS 'pd:…'` marker.
#[derive(Debug, Clone)]
pub struct Marker {
    /// Table the marker names.
    pub table: String,
    /// Column the marker names.
    pub column: String,
    /// Tokens, verbatim, before validation.
    pub tokens: Vec<String>,
    /// Free-text note after the token list.
    pub note: String,
    /// Migration file holding the marker.
    pub file: PathBuf,
}

/// Collect every classification marker across the given migration files.
#[must_use]
pub fn parse_markers(files: &[PathBuf]) -> Vec<Marker> {
    let mut markers = Vec::new();
    for file in files {
        let Ok(content) = fs::read_to_string(file) else {
            continue;
        };
        let tokens = lex(&content);
        for statement in statements(&tokens) {
            let head: Vec<&str> = statement.iter().take(3).filter_map(Tok::word).collect();
            if head.first().copied() != Some("comment")
                || head.get(1).copied() != Some("on")
                || head.get(2).copied() != Some("column")
            {
                continue;
            }
            let Some(marker) = read_marker(statement, file) else {
                continue;
            };
            markers.push(marker);
        }
    }
    markers
}

fn read_marker(statement: &[Tok], file: &Path) -> Option<Marker> {
    // COMMENT ON COLUMN [schema.]table.column IS 'body'
    let mut parts: Vec<String> = Vec::new();
    let mut cursor = 3usize;
    loop {
        let word = statement.get(cursor)?.word()?;
        if word == "is" {
            break;
        }
        parts.push(word.to_owned());
        cursor += 1;
        if statement.get(cursor).is_some_and(|t| t.is_punct('.')) {
            cursor += 1;
        } else {
            break;
        }
    }
    while statement.get(cursor).and_then(Tok::word) != Some("is") {
        cursor += 1;
        if cursor >= statement.len() {
            return None;
        }
    }
    let body = match statement.get(cursor + 1)? {
        Tok::Str(s) => s.clone(),
        _ => return None,
    };
    let column = parts.pop()?;
    let table = parts.pop()?;

    let rest = body.strip_prefix(MARKER_PREFIX)?;
    let (token_text, note) = match rest.find(char::is_whitespace) {
        Some(at) => (&rest[..at], rest[at..].trim().to_owned()),
        None => (rest, String::new()),
    };
    let tokens = token_text
        .split(',')
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(str::to_owned)
        .collect();

    Some(Marker {
        table,
        column,
        tokens,
        note,
        file: file.to_path_buf(),
    })
}

/// Validate one token against the closed vocabulary.
///
/// Returns the offending detail when the token is not usable.
fn validate_token(token: &str) -> Result<&'static ClassCitation, (ViolationKind, String)> {
    let (head, sub) = match token.split_once('/') {
        Some((head, sub)) => (head, Some(sub)),
        None => (token, None),
    };
    let Some(citation) = citation_for(head) else {
        return Err((
            ViolationKind::UnknownToken,
            format!(
                "token '{token}' is not in the closed vocabulary [{}]",
                VOCABULARY
                    .iter()
                    .map(|c| c.token)
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        ));
    };
    if citation.subtokens.is_empty() {
        if sub.is_some() {
            return Err((
                ViolationKind::BadSubtoken,
                format!("class '{head}' takes no sub-token but got '{token}'"),
            ));
        }
        return Ok(citation);
    }
    let Some(sub) = sub else {
        return Err((
            ViolationKind::BadSubtoken,
            format!(
                "class '{head}' requires a sub-token — one of [{}] — because {} ({}) draws \
                 distinctions this gate will not collapse",
                citation.subtokens.join(", "),
                citation.instrument,
                citation.article,
            ),
        ));
    };
    if !citation.subtokens.contains(&sub) {
        return Err((
            ViolationKind::BadSubtoken,
            format!(
                "sub-token '{sub}' is not valid for class '{head}'; {} ({}) admits only [{}]",
                citation.instrument,
                citation.article,
                citation.subtokens.join(", "),
            ),
        ));
    }
    Ok(citation)
}

// ---------------------------------------------------------------------------
// Gate
// ---------------------------------------------------------------------------

/// Leading numeric prefix of a migration filename, e.g. `0209_foo.sql` -> 209.
///
/// `None` for a filename with no numeric prefix, which the caller treats as
/// "after the freeze": an unnumbered migration is one this gate cannot place
/// on the clock, and the fail-closed reading is the one that rejects.
fn migration_number(path: &Path) -> Option<u32> {
    let name = path.file_name()?.to_str()?;
    let digits: String = name.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

/// Prove the migration-number clock cannot be lied to.
///
/// `BASELINE_FROZEN_AFTER_MIGRATION` decides whether a column predates the
/// backlog, and it decides it by reading a FILENAME PREFIX — written by the
/// same author as the migration it labels. Nothing above stops a new migration
/// from being called `0042_…` and being read as pre-freeze, which puts the
/// whole baseline ratchet back on the honour system.
///
/// What makes the prefix honest is that every number at or below the freeze is
/// already taken and stays taken. Then a new migration cannot claim one: it
/// collides. So this checks exactly that, and nothing more —
///
/// * no two migrations share a number at or below the freeze, and
/// * no number below the highest one present is vacant, since a vacancy is a
///   slot a later migration could move into and be read as pre-freeze.
///
/// Bounded to `<= BASELINE_FROZEN_AFTER_MIGRATION` on purpose: numbers above
/// the freeze are already outside the baseline's shelter, so a collision there
/// changes no verdict here. `console-gate-migration-safety` checks both
/// properties across the whole range; this gate re-checks the part its own
/// correctness rests on, because a silent dependency on a sibling gate is a
/// dependency that breaks silently.
fn check_freeze_clock(files: &[PathBuf], result: &mut GateResult) {
    let mut by_number: BTreeMap<u32, Vec<&PathBuf>> = BTreeMap::new();
    for file in files {
        if let Some(number) =
            migration_number(file).filter(|n| *n <= BASELINE_FROZEN_AFTER_MIGRATION)
        {
            by_number.entry(number).or_default().push(file);
        }
    }
    for (number, sharing) in &by_number {
        if sharing.len() > 1 {
            result.push(
                ViolationKind::MigrationNumberReused,
                sharing.first().map(|f| (*f).clone()),
                format!(
                    "migration number {number:04} is claimed by {} files — the clock \
                     {BASELINE_FROZEN_AFTER_MIGRATION:04} is read from this prefix, so a reused \
                     number lets a new migration present itself as pre-freeze and be sheltered \
                     by the baseline",
                    sharing.len()
                ),
            );
        }
    }
    let Some(highest) = by_number.keys().next_back().copied() else {
        return;
    };
    for number in 1..highest {
        if !by_number.contains_key(&number) {
            result.push(
                ViolationKind::MigrationNumberVacant,
                None,
                format!(
                    "migration number {number:04} is vacant below {highest:04} — a vacancy at or \
                     below the freeze is a slot a new migration can occupy and be read as \
                     pre-freeze"
                ),
            );
        }
    }
}

/// Parse a baseline file into its table set, ignoring blanks and `#` comments.
#[must_use]
pub fn parse_baseline(text: &str) -> BTreeSet<String> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(str::to_ascii_lowercase)
        .collect()
}

/// Run the gate against a workspace root, reading the checked-in baseline.
///
/// # Errors
/// Returns an error when the migration tree or the baseline file cannot be read.
pub fn check_workspace(start_dir: &Path) -> Result<GateResult, String> {
    let root = workspace_root(start_dir)?;
    let baseline_path = root.join(BASELINE_RELATIVE_PATH);
    let baseline = fs::read_to_string(&baseline_path)
        .map_err(|e| format!("cannot read baseline {}: {e}", baseline_path.display()))?;
    let files = collect_migration_files(&root)?;
    Ok(check_files(&files, &parse_baseline(&baseline)))
}

/// Find the workspace root by walking up from `start` until the baseline is
/// found.
///
/// The CI job this gate runs in sets `defaults.run.working-directory: backend`,
/// so `current_dir()` is NOT the repo root and resolving the baseline against
/// it produced `backend/backend/ci/…`: the gate exited 1 on every run. Every
/// sibling gate resolves only paths that happen to sit under `backend/`, so
/// none of them noticed. Anchoring on the baseline file makes the gate answer
/// the same from any directory inside the checkout.
///
/// # Errors
/// Returns an error when no ancestor of `start` holds the baseline file.
pub fn workspace_root(start: &Path) -> Result<PathBuf, String> {
    start
        .ancestors()
        .find(|dir| dir.join(BASELINE_RELATIVE_PATH).is_file())
        .map(Path::to_path_buf)
        .ok_or_else(|| {
            format!(
                "no ancestor of {} holds {BASELINE_RELATIVE_PATH} — run this from inside the \
                 repository checkout",
                start.display()
            )
        })
}

/// Run the gate against an explicit migration tree and baseline set.
///
/// # Errors
/// Returns an error when the tree cannot be walked.
pub fn check_tree(root: &Path, baseline: &BTreeSet<String>) -> Result<GateResult, String> {
    let files = collect_migration_files(root)?;
    Ok(check_files(&files, baseline))
}

/// Run the gate over an explicit, ordered file list.
#[must_use]
pub fn check_files(files: &[PathBuf], baseline: &BTreeSet<String>) -> GateResult {
    let schema = parse_schema(files);
    let markers = parse_markers(files);

    let mut result = GateResult {
        total_tables: schema.tables.len(),
        total_columns: schema.column_count(),
        baselined_tables: baseline.len(),
        ..GateResult::default()
    };

    // 0. Anything the parser could not read. First, because every later check
    //    reasons over a column set that this says is incomplete.
    let mut waiver_matched = vec![false; UNSUPPORTED_WAIVERS.len()];
    for (file, detail) in &schema.unsupported {
        let name = file
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();
        if let Some(index) = UNSUPPORTED_WAIVERS
            .iter()
            .position(|waiver| waiver.migration == name && detail.contains(waiver.construct))
        {
            waiver_matched[index] = true;
            result.waived_statements += 1;
            continue;
        }
        result.push(
            ViolationKind::UnsupportedDdl,
            Some(file.clone()),
            format!(
                "{detail} — the gate cannot enumerate this table's columns, so it cannot prove \
                 they are classified. Teach the parser this form, or write the table out in \
                 explicit column definitions"
            ),
        );
    }
    // A waiver may only leave this list. It is checked against the tree it
    // names, so a synthetic tree that does not contain the migration says
    // nothing about the waiver either way.
    for (waiver, matched) in UNSUPPORTED_WAIVERS.iter().zip(&waiver_matched) {
        let present = files
            .iter()
            .any(|file| file.file_name().and_then(|n| n.to_str()) == Some(waiver.migration));
        if present && !matched {
            result.push(
                ViolationKind::WaiverMatchesNothing,
                None,
                format!(
                    "the waiver for {} ('{}') no longer matches anything the parser rejects — \
                     remove it from UNSUPPORTED_WAIVERS so the list keeps shrinking",
                    waiver.migration, waiver.construct
                ),
            );
        }
    }

    check_freeze_clock(files, &mut result);

    // 1. Marker well-formedness, existence and uniqueness.
    let mut classified: BTreeMap<(String, String), &Marker> = BTreeMap::new();
    for marker in &markers {
        let key = (marker.table.clone(), marker.column.clone());
        let exists = schema
            .tables
            .get(&marker.table)
            .is_some_and(|columns| columns.contains(&marker.column));
        if !exists {
            result.push(
                ViolationKind::MarkerForUnknownColumn,
                Some(marker.file.clone()),
                format!(
                    "marker classifies {}.{}, which the migration parse says does not exist",
                    marker.table, marker.column
                ),
            );
            continue;
        }
        if marker.tokens.is_empty() {
            result.push(
                ViolationKind::EmptyMarker,
                Some(marker.file.clone()),
                format!(
                    "marker on {}.{} carries the '{MARKER_PREFIX}' prefix but no tokens",
                    marker.table, marker.column
                ),
            );
            continue;
        }
        let mut token_ok = true;
        for token in &marker.tokens {
            if let Err((kind, detail)) = validate_token(token) {
                result.push(
                    kind,
                    Some(marker.file.clone()),
                    format!("{}.{}: {detail}", marker.table, marker.column),
                );
                token_ok = false;
            }
        }
        if !token_ok {
            continue;
        }
        if let Some(previous) = classified.insert(key, marker) {
            result.push(
                ViolationKind::DuplicateMarker,
                Some(marker.file.clone()),
                format!(
                    "{}.{} is classified twice (also {})",
                    marker.table,
                    marker.column,
                    previous.file.display()
                ),
            );
        }
    }
    result.classified_columns = classified.len();

    // 2. Completeness for every table outside the baseline. A NEW table is
    //    never in the baseline, so this is what forces classification at
    //    creation.
    for (table, columns) in &schema.tables {
        if baseline.contains(table) {
            continue;
        }
        for column in columns {
            if classified.contains_key(&(table.clone(), column.clone())) {
                continue;
            }
            let origin = schema
                .origins
                .get(&(table.clone(), column.clone()))
                .cloned();
            result.push(
                ViolationKind::UnclassifiedColumn,
                origin,
                format!(
                    "{table}.{column} has no personal-data classification — add \
                     `COMMENT ON COLUMN {table}.{column} IS '{MARKER_PREFIX}<tokens> …';` in the \
                     migration that creates it, or list '{table}' in {BASELINE_RELATIVE_PATH}"
                ),
            );
        }
    }

    // 3. The baseline is a ratchet: it may only shrink, it may not rot, and it
    //    may not shelter anything that did not exist when it was frozen.
    for table in baseline {
        let Some(columns) = schema.tables.get(table) else {
            result.push(
                ViolationKind::BaselineEntryUnknownTable,
                None,
                format!(
                    "baseline lists '{table}', which no migration creates — remove it from \
                     {BASELINE_RELATIVE_PATH}"
                ),
            );
            continue;
        };
        let unclassified: Vec<&String> = columns
            .iter()
            .filter(|column| !classified.contains_key(&(table.clone(), (*column).clone())))
            .collect();
        // Anything introduced after the freeze is outside what the backlog
        // declared. This is what makes "a table may only leave this list"
        // enforced rather than asserted.
        for column in &unclassified {
            let origin = schema.origins.get(&(table.clone(), (*column).clone()));
            let number = origin.and_then(|path| migration_number(path));
            if number.is_none_or(|n| n > BASELINE_FROZEN_AFTER_MIGRATION) {
                result.push(
                    ViolationKind::BaselineGrew,
                    origin.cloned(),
                    format!(
                        "{table}.{column} was introduced after the baseline froze at migration \
                         {BASELINE_FROZEN_AFTER_MIGRATION:04}, so '{table}' being listed in \
                         {BASELINE_RELATIVE_PATH} does not cover it — classify it, or the \
                         backlog is growing"
                    ),
                );
            }
        }
        if unclassified.is_empty() {
            result.push(
                ViolationKind::BaselineEntryFullyClassified,
                None,
                format!(
                    "all {} column(s) of '{table}' are now classified — remove '{table}' from \
                     {BASELINE_RELATIVE_PATH} so the ratchet holds",
                    columns.len()
                ),
            );
        }
    }

    result
}

// ---------------------------------------------------------------------------
// File discovery — same walk as the migration-safety gate.
// ---------------------------------------------------------------------------

/// Collect every `.sql` file under any directory named `migrations`, sorted.
///
/// # Errors
/// Returns an error when a directory cannot be read.
pub fn collect_migration_files(root: &Path) -> Result<Vec<PathBuf>, String> {
    let mut files = Vec::new();
    collect_inner(root, false, &mut files)?;
    files.sort();
    Ok(files)
}

/// Directories whose contents are never first-party migrations.
///
/// `buck-out` is load-bearing, not defensive tidiness. Buck materialises every
/// third-party crate's source there, and several ship migrations of their own —
/// `apalis-postgres` has `migrations/*.sql`, `sqlx` has SQLite test fixtures.
/// Walking into it makes the gate classify `jobs`, `workers`, `user`, `post` and
/// `comment` as if they were ours, and makes the result depend on whether anyone
/// has run Buck locally: 31 violations on a developer's machine, silent on a
/// fresh CI checkout where `buck-out` does not exist yet. A gate whose verdict
/// depends on build residue is worse than no gate, because the green is not
/// reproducible.
const SKIP_DIRS: &[&str] = &["target", ".git", "buck-out", "node_modules"];

fn collect_inner(dir: &Path, in_migrations: bool, files: &mut Vec<PathBuf>) -> Result<(), String> {
    if dir
        .components()
        .any(|c| SKIP_DIRS.contains(&c.as_os_str().to_string_lossy().as_ref()))
    {
        return Ok(());
    }
    let entries =
        fs::read_dir(dir).map_err(|e| format!("cannot read directory {}: {e}", dir.display()))?;
    for entry in entries {
        let entry = entry.map_err(|e| format!("cannot read entry in {}: {e}", dir.display()))?;
        let path = entry.path();
        let file_type = entry
            .file_type()
            .map_err(|e| format!("cannot stat {}: {e}", path.display()))?;
        if file_type.is_dir() {
            let is_migrations = entry.file_name().to_string_lossy() == "migrations";
            collect_inner(&path, in_migrations || is_migrations, files)?;
        } else if file_type.is_file()
            && in_migrations
            && path.extension().is_some_and(|ext| ext == "sql")
        {
            files.push(path);
        }
    }
    Ok(())
}
