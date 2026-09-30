#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! The field-level personal-data classification, and the obligation DERIVED
//! from it, proved against a real migrated database.
//!
//! WHY THIS TEST EXISTS AND NOT JUST THE GATE. The CI gate reads the migration
//! files as text. These assertions read `pg_attribute` after the migrations have
//! actually run, which is the only place the classification is real: a
//! `COMMENT ON COLUMN` naming a column that does not exist aborts the migration,
//! and a `DROP COLUMN` silently discards a comment. Text and catalog can
//! disagree, and the catalog is what the derivation reads. The classification
//! reader and the completeness reader intentionally cover the same persistent
//! table/partition/materialized-view/foreign-table universe across schemas.
//!
//! TWO CHECKS, AND SINCE THE BASELINE BECAME A SET OF NAMES THIS ONE CARRIES
//! THE CLASS. The gate reads migration TEXT and sees a column the moment it is
//! WRITTEN, before any database exists; that is worth keeping and it stays. What
//! it no longer has to be right about is this: the baseline below used to pin a
//! per-table unclassified COUNT, so a column added in a spelling the gate
//! misparsed slipped past both sides whenever the same change classified some
//! other column and left the count where it was. Six rounds of parser hardening
//! were each defeated by exactly that trade. The baseline is now the SET of
//! unclassified column NAMES, which has nothing to trade with — any change to
//! membership fails here, whatever DDL produced it, FOR ANY RELATION THIS SWEEP
//! READS. What that excludes is written out below and is not small: relations
//! created at RUNTIME (0005's per-day `location_pings` partitions). See the
//! crate doc of `console-gate-personal-data-classification` for the same
//! statement from the other side.
//!
//! WHAT IS ASSERTED, AND WHAT IS NOT. These tests assert that the classification
//! is present and that the derivation computes from it. They assert nothing
//! about whether any statutory obligation is met — nothing here observes how
//! long access logs are actually retained — and they move no compliance control
//! off HOLD.
//!
//! THE FLOOR: THIS ASSUMES A MIGRATION THAT DOES NOT EXERCISE SUPERUSER
//! CATALOG-WRITE PRIVILEGE. Two escapes found in this sweep needed the same
//! thing and it is worth naming once rather than discovering a third time. Both
//! `CREATE TABLE information_schema.pd_leak` and, after
//! `set_config('allow_system_table_mods','on',true)`, `CREATE SCHEMA pg_evil`
//! require superuser. At that privilege the migrating session does not have to
//! hide a table at all: it can `INSERT INTO pg_description` and forge a `pd:`
//! marker on any column, or drop the assertion outright. So there is a level
//! below which no assertion that READS the catalog can defend, because at that
//! level the catalog is writable — measured on the pinned `postgres:18.4`, where
//! the initdb superuser's direct `INSERT INTO pg_description` succeeds.
//!
//! Where that level sits, measured rather than assumed. Production migrates as
//! `console_app` — the role `deploy/apps/console/base/migrate-job.yaml` supplies
//! via the `console-db-app` secret, created `NOSUPERUSER` by
//! `ops/postgres-reconcile-topology.sh:294`. As `console_app`, against the same
//! topology this file's containers build:
//!
//! * `set_config('allow_system_table_mods','on',true)`
//!   → `42501 permission denied to set parameter "allow_system_table_mods"`
//! * `CREATE SCHEMA pg_evil` → `42939 unacceptable schema name "pg_evil"`,
//!   with no way to lift it
//! * `CREATE TABLE pg_catalog.pd_leak` → `42501 permission denied for schema
//!   pg_catalog`
//! * `INSERT INTO pg_description` → `42501 permission denied for table
//!   pg_description`
//!
//! The pgtest and CI containers migrate as the initdb superuser instead, which
//! is why every probe in this lane lands there and why these escapes are
//! reachable at all in a test. That is a real boundary and it is stated so the
//! texts stop implying there is none. It is NOT a reason to leave a reachable
//! predicate unjustified: the `pg_` prefix filter was closed anyway, because
//! closing it cost zero baseline entries. See `application_columns`.

use std::collections::{BTreeMap, BTreeSet};

use sqlx::{PgPool, Row};

// ---------------------------------------------------------------------------
// THE CLOSED VOCABULARY, and the ONE place a marker is judged.
//
// This used to be two places that disagreed, and the disagreement was a hole.
// `classified` was `(d.description LIKE 'pd:%') IS TRUE` — any string starting
// `pd:` counted, so `COMMENT ON COLUMN x IS 'pd:lol'` was a classification. The
// only vocabulary check in the file read `personal_data_columns()`, which is
// scoped `nspname = 'public'`, so a bogus token in any other schema was checked
// by nothing at all — including the non-public-schema case this file exists to
// catch.
//
// So the vocabulary is now applied where `classified` is decided, over the same
// row set, which spans every application schema. A marker outside the
// vocabulary is not a weaker classification; it is a violation, and unlike a
// MISSING marker no baseline entry shelters it.
//
// 개인정보 보호법 시행령 제19조 fixes the four 고유식별정보; 제23조제1항 and
// 시행령 제18조 fix the 민감정보 sub-categories. Both must name WHICH, because
// the whole point of the marker is that the judgement is auditable.
// ---------------------------------------------------------------------------

const CLASSES: [&str; 7] = [
    "none",
    "personal",
    "sensitive",
    "unique-id",
    "credit",
    "pseudonymous",
    "undeclared",
];
const UNIQUE_ID_SUBTOKENS: [&str; 4] = ["rrn", "passport", "driver-license", "arc"];
const SENSITIVE_SUBTOKENS: [&str; 9] = [
    "belief",
    "union",
    "political",
    "health",
    "sex-life",
    "genetic",
    "criminal-record",
    "biometric-id",
    "race-ethnicity",
];

/// What a column comment says about classification.
#[derive(Debug, PartialEq, Eq)]
enum Marker {
    /// No `pd:` marker at all. Unclassified, and shelterable by the baseline.
    Absent,
    /// A `pd:` marker every one of whose tokens is in the closed vocabulary.
    Valid,
    /// A `pd:` marker carrying a token that is not. Never counts as classified,
    /// and is a violation wherever it appears.
    Invalid(String),
}

/// Judge a column comment exactly as `personal_data_columns()` reads it — the
/// `pd:` prefix, then a comma-separated token list running to the first
/// whitespace — against the closed vocabulary.
///
/// The split is `char::is_whitespace` and not `split_whitespace`, so that it
/// ports `regexp_match(description, '^pd:([^[:space:]]*)')` faithfully: a
/// marker written `pd: none` yields an EMPTY token list here just as it does in
/// SQL, and an empty token is outside the vocabulary. `split` always yields at
/// least one item, so the `unwrap_or` below is unreachable rather than a
/// default.
fn read_marker(comment: Option<&str>) -> Marker {
    let Some(rest) = comment.and_then(|c| c.strip_prefix("pd:")) else {
        return Marker::Absent;
    };
    for token in rest
        .split(char::is_whitespace)
        .next()
        .unwrap_or("")
        .split(',')
    {
        let (head, sub) = match token.split_once('/') {
            Some((head, sub)) => (head, Some(sub)),
            None => (token, None),
        };
        let in_vocabulary = match head {
            "unique-id" => sub.is_some_and(|s| UNIQUE_ID_SUBTOKENS.contains(&s)),
            "sensitive" => sub.is_some_and(|s| SENSITIVE_SUBTOKENS.contains(&s)),
            _ => CLASSES.contains(&head) && sub.is_none(),
        };
        if !in_vocabulary {
            return Marker::Invalid(token.to_owned());
        }
    }
    Marker::Valid
}

/// Remove every classification marker, so a single probe column can be the only
/// input to the derivation. `#[sqlx::test]` hands each test its own database, so
/// this mutates nothing another test can see.
async fn strip_all_markers(pool: &PgPool) {
    sqlx::query(
        r"
        DO $$
        DECLARE target RECORD;
        BEGIN
            FOR target IN
                SELECT schema_name, rel_name, col_name
                FROM personal_data_columns()
            LOOP
                EXECUTE format('COMMENT ON COLUMN %I.%I.%I IS NULL',
                               target.schema_name, target.rel_name, target.col_name);
            END LOOP;
        END $$;
        ",
    )
    .execute(pool)
    .await
    .unwrap();

    let remaining: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM personal_data_columns()")
        .fetch_one(pool)
        .await
        .unwrap();
    assert_eq!(remaining, 0, "strip_all_markers left markers behind");
}

async fn floor_years(pool: &PgPool) -> i32 {
    sqlx::query_scalar("SELECT access_log_retention_floor_years()")
        .fetch_one(pool)
        .await
        .unwrap()
}

/// Assert that neither an otherwise ungranted role nor the application runtime
/// can execute either SECURITY DEFINER catalog reader.
async fn assert_schema_derivation_owner_only(pool: &PgPool) {
    let ungranted = format!("pd_ungranted_{}", uuid::Uuid::new_v4().simple());
    sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
        "CREATE ROLE \"{ungranted}\" NOLOGIN"
    )))
    .execute(pool)
    .await
    .unwrap();

    for role in [&ungranted, "console_rt"] {
        for probe in [
            "SELECT access_log_retention_floor_years()::BIGINT",
            "SELECT COUNT(*) FROM personal_data_columns()",
        ] {
            let mut tx = pool.begin().await.unwrap();
            sqlx::raw_sql(sqlx::AssertSqlSafe(format!("SET LOCAL ROLE \"{role}\"")))
                .execute(tx.as_mut())
                .await
                .unwrap();
            let refused = sqlx::query_scalar::<_, i64>(probe)
                .fetch_one(tx.as_mut())
                .await
                .expect_err("owner-only schema introspection must refuse this role");
            assert_eq!(
                refused
                    .as_database_error()
                    .and_then(|e| e.code())
                    .as_deref(),
                Some("42501"),
                "expected insufficient_privilege for role `{role}` and `{probe}`, got {refused:?}"
            );
            tx.rollback().await.unwrap();
        }
    }

    sqlx::raw_sql(sqlx::AssertSqlSafe(format!("DROP ROLE \"{ungranted}\"")))
        .execute(pool)
        .await
        .unwrap();
}

/// Create a one-column probe table carrying exactly the given classification.
///
/// `comment_sql` is a `&'static str` because sqlx 0.9 refuses a built query
/// string outright — which is the right default, and costs nothing here since
/// every probe classification is a literal.
async fn probe_with(pool: &PgPool, comment_sql: &'static str) {
    sqlx::query("CREATE TABLE pd_probe (probe_column TEXT)")
        .execute(pool)
        .await
        .unwrap();
    sqlx::query(comment_sql).execute(pool).await.unwrap();
}

const PROBE_SENSITIVE_HEALTH: &str =
    "COMMENT ON COLUMN pd_probe.probe_column IS 'pd:sensitive/health — probe'";
const PROBE_UNIQUE_ID_RRN: &str =
    "COMMENT ON COLUMN pd_probe.probe_column IS 'pd:unique-id/rrn — probe'";
const PROBE_PSEUDONYMOUS: &str =
    "COMMENT ON COLUMN pd_probe.probe_column IS 'pd:pseudonymous — probe'";
const PROBE_UNDECLARED: &str = "COMMENT ON COLUMN pd_probe.probe_column IS 'pd:undeclared — probe'";

/// The plaintext 주민등록번호 finding lives in the schema, not only in a
/// document. `0066_hr_core_employee_fields.sql` states in its own header that
/// resident-registration and disability values remain in `employees.raw_row`,
/// and `hr.rs` classifies the 주민 and 장애 import headers `restricted` under a
/// `retain_raw_mask_preview` policy — the mask applies to the preview
/// projection, not to storage. If that ever stops being true, this assertion is
/// where it gets noticed.
#[sqlx::test(migrations = "./migrations")]
async fn raw_import_rows_name_known_restricted_and_unknown_content(pool: PgPool) {
    let expected = BTreeSet::from([
        "sensitive/health".to_owned(),
        "undeclared".to_owned(),
        "unique-id/rrn".to_owned(),
    ]);

    for relation in ["employees", "data_import_rows"] {
        let tokens: Vec<String> = sqlx::query_scalar(
            "SELECT tokens FROM personal_data_columns()
              WHERE schema_name = 'public' AND rel_name = $1 AND col_name = 'raw_row'",
        )
        .bind(relation)
        .fetch_one(&pool)
        .await
        .unwrap();
        let actual: BTreeSet<String> = tokens.into_iter().collect();
        assert_eq!(
            actual, expected,
            "{relation}.raw_row must retain the known RRN and health classes while admitting arbitrary workbook content"
        );
    }
}

/// Rule C is semantic, so its five non-obvious JSONB cases are mutation-locked
/// here instead of relying on prose in the migration. The two raw workbook rows
/// are asserted above because they also carry known restricted classes.
#[sqlx::test(migrations = "./migrations")]
async fn user_supplied_json_objects_are_classified_undeclared(pool: PgPool) {
    for (relation, column) in [
        ("todos", "scopes"),
        ("docs_evidence_custody_events", "from_custodian"),
        ("docs_evidence_custody_events", "to_custodian"),
    ] {
        let tokens: Vec<String> = sqlx::query_scalar(
            "SELECT tokens FROM personal_data_columns()
              WHERE schema_name = 'public' AND rel_name = $1 AND col_name = $2",
        )
        .bind(relation)
        .bind(column)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(
            tokens.into_iter().collect::<BTreeSet<_>>(),
            BTreeSet::from(["personal".to_owned(), "undeclared".to_owned()]),
            "{relation}.{column} accepts user-controlled JSON content and must admit that its contents are not closed"
        );
    }
}

/// The whole point of the control: the obligation is computed from the schema,
/// not hand-maintained.
#[sqlx::test(migrations = "./migrations")]
async fn shipped_classification_derives_the_two_year_floor(pool: PgPool) {
    assert_eq!(
        floor_years(&pool).await,
        2,
        "a schema classified 고유식별정보 must derive the 2-year 접속기록 floor \
         (고시 제2026-9호 제8조제1항제2호)"
    );
}

/// THE CORRECTION, and the reason this test is not optional.
///
/// 「개인정보의 안전성 확보조치 기준」 제8조제1항제2호 reads
/// `고유식별정보 **또는** 민감정보`. A derivation that fires the two-year floor
/// only off 고유식별정보 UNDER-RETAINS for a system holding 민감정보 and no
/// 고유식별정보 — and under-retention is the legally dangerous direction. This
/// asserts the 민감정보 limb fires on its own, with every 고유식별정보 marker
/// removed from the database.
#[sqlx::test(migrations = "./migrations")]
async fn sensitive_alone_raises_the_floor_with_no_unique_id_present(pool: PgPool) {
    strip_all_markers(&pool).await;
    probe_with(&pool, PROBE_SENSITIVE_HEALTH).await;

    let unique_id_columns: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM personal_data_columns() AS pdc,
                LATERAL unnest(pdc.tokens) AS token
          WHERE token LIKE 'unique-id%'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        unique_id_columns, 0,
        "the 민감정보 limb must be proved with no 고유식별정보 anywhere"
    );

    assert_eq!(
        floor_years(&pool).await,
        2,
        "민감정보 alone must raise the floor to 2 years"
    );
}

/// The retention reader covers non-public schemas, not only `public`.
#[sqlx::test(migrations = "./migrations")]
async fn sensitive_in_a_non_public_schema_raises_the_floor(pool: PgPool) {
    strip_all_markers(&pool).await;
    sqlx::query("CREATE SCHEMA shadow")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("CREATE TABLE shadow.pd_probe (probe_column TEXT)")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("COMMENT ON COLUMN shadow.pd_probe.probe_column IS 'pd:sensitive/health — probe'")
        .execute(&pool)
        .await
        .unwrap();

    let identity: (String, String, String) =
        sqlx::query_as("SELECT schema_name, rel_name, col_name FROM personal_data_columns()")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        identity,
        (
            "shadow".to_owned(),
            "pd_probe".to_owned(),
            "probe_column".to_owned()
        )
    );
    assert_eq!(floor_years(&pool).await, 2);
}

/// A materialized view stores rows and is part of the same classification and
/// derivation universe as ordinary tables.
#[sqlx::test(migrations = "./migrations")]
async fn sensitive_on_a_materialized_view_raises_the_floor(pool: PgPool) {
    strip_all_markers(&pool).await;
    sqlx::query(
        "CREATE MATERIALIZED VIEW pd_probe_mv AS
         SELECT ''::TEXT AS probe_column WITH NO DATA",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("COMMENT ON COLUMN pd_probe_mv.probe_column IS 'pd:sensitive/health — probe'")
        .execute(&pool)
        .await
        .unwrap();

    let relkind: String = sqlx::query_scalar(
        "SELECT c.relkind::TEXT
           FROM pg_catalog.pg_class AS c
           JOIN pg_catalog.pg_namespace AS n ON n.oid = c.relnamespace
          WHERE n.nspname = 'public' AND c.relname = 'pd_probe_mv'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(relkind, "m");
    assert_eq!(floor_years(&pool).await, 2);
}

/// A foreign table can expose remote personal rows and accepts column comments;
/// its marker must participate in the same conservative floor derivation.
#[sqlx::test(migrations = "./migrations")]
async fn sensitive_on_a_foreign_table_raises_the_floor(pool: PgPool) {
    strip_all_markers(&pool).await;
    sqlx::query("CREATE EXTENSION file_fdw")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("CREATE SERVER pd_probe_server FOREIGN DATA WRAPPER file_fdw")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        "CREATE FOREIGN TABLE pd_probe_foreign (probe_column TEXT)
         SERVER pd_probe_server OPTIONS (filename '/dev/null', format 'text')",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("COMMENT ON COLUMN pd_probe_foreign.probe_column IS 'pd:sensitive/health — probe'")
        .execute(&pool)
        .await
        .unwrap();

    let relkind: String = sqlx::query_scalar(
        "SELECT c.relkind::TEXT
           FROM pg_catalog.pg_class AS c
           JOIN pg_catalog.pg_namespace AS n ON n.oid = c.relnamespace
          WHERE n.nspname = 'public' AND c.relname = 'pd_probe_foreign'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(relkind, "f");
    assert_eq!(floor_years(&pool).await, 2);
}

/// The 고유식별정보 limb, likewise on its own.
#[sqlx::test(migrations = "./migrations")]
async fn unique_id_alone_raises_the_floor(pool: PgPool) {
    strip_all_markers(&pool).await;
    probe_with(&pool, PROBE_UNIQUE_ID_RRN).await;
    assert_eq!(floor_years(&pool).await, 2);
}

/// 가명정보 is exempt from a list of duties that widens on 2026-09-11
/// (개보법 제28조의7, 개정 2026.3.10). It must not be what raises the floor.
#[sqlx::test(migrations = "./migrations")]
async fn pseudonymous_alone_does_not_raise_the_floor(pool: PgPool) {
    strip_all_markers(&pool).await;
    probe_with(&pool, PROBE_PSEUDONYMOUS).await;
    assert_eq!(
        floor_years(&pool).await,
        1,
        "가명정보 must not trigger 제8조제1항제2호"
    );
}

/// `undeclared` is an admission that the content is unknown. It must not be
/// read as a clean bill — but it is also not 민감정보, so it does not fabricate
/// a 2-year floor either. The floor falls back to 제8조제1항 본문's 1 year.
#[sqlx::test(migrations = "./migrations")]
async fn undeclared_alone_does_not_fabricate_a_two_year_floor(pool: PgPool) {
    strip_all_markers(&pool).await;
    probe_with(&pool, PROBE_UNDECLARED).await;
    assert_eq!(floor_years(&pool).await, 1);
}

/// With nothing classified at all the derivation must not invent an obligation.
/// This is the lower-bound property stated plainly: an empty classification
/// yields 제8조제1항 본문's floor, never the 제2호 floor.
#[sqlx::test(migrations = "./migrations")]
async fn an_empty_classification_derives_the_base_floor(pool: PgPool) {
    strip_all_markers(&pool).await;
    assert_eq!(floor_years(&pool).await, 1);
}

/// Every marker on a relation this sweep reads is in the closed vocabulary, in
/// every application schema.
///
/// Scoped deliberately to the SAME relation set the completeness assertion
/// takes — `relkind IN ('r','p','m','f')` — not to the whole catalog. A `pd:`
/// marker on a view or a composite-type column is ACCEPTED by PostgreSQL and
/// lands in `pg_description` where this never looks, so a bogus token there
/// passes. That is outside both readers rather than a derivation gap:
/// `personal_data_columns()` uses this same non-temporary `r/p/m/f` relation
/// universe, so nothing the retention derivation consumes can carry an
/// unchecked token. Stated because "every marker in the catalog" would be a
/// wider claim than the query makes.
///
/// This used to read `personal_data_columns()`, which is scoped
/// `nspname = 'public'`. A bogus token in `shadow` or `leave_api` was therefore
/// checked by nothing — the same non-public-schema blind spot the completeness
/// assertion below exists to close. It now reads the same catalog sweep that
/// decides `classified`, so the two cannot drift apart.
#[sqlx::test(migrations = "./migrations")]
async fn every_catalog_token_is_in_the_closed_vocabulary(pool: PgPool) {
    let columns = application_columns(&pool).await;
    let mut valid = 0_usize;
    let mut outside = Vec::new();
    for column in &columns {
        match read_marker(column.comment.as_deref()) {
            Marker::Valid => valid += 1,
            Marker::Absent => {}
            Marker::Invalid(token) => outside.push(format!(
                "{}.{}: '{token}'",
                column.relation(),
                column.column
            )),
        }
    }

    assert!(valid > 0, "no classification reached the catalog at all");
    assert!(
        outside.is_empty(),
        "{} marker(s) outside the closed vocabulary — a `pd:` prefix is not a \
         classification:\n{}",
        outside.len(),
        outside.join("\n")
    );
}

/// 신정법 제2조제7호's applicability to an HR and payroll console is unresolved,
/// so `credit` is a token a human must not yet use. The gate accepts it by
/// design; this asserts nobody has.
#[sqlx::test(migrations = "./migrations")]
async fn no_column_is_classified_credit_while_the_scope_question_is_open(pool: PgPool) {
    let credit_columns: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM personal_data_columns() AS pdc,
                LATERAL unnest(pdc.tokens) AS token
          WHERE token = 'credit' OR token LIKE 'credit/%'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        credit_columns, 0,
        "assigning 개인신용정보 is a legal act; 신용정보의 이용 및 보호에 관한 법률 \
         제2조제7호 scope is a question for counsel and is not answered in code"
    );
}

/// Schema introspection stays owner-only: PUBLIC and the application runtime
/// role are both refused.
///
/// An earlier version of this file asserted only that `console_rt` may execute
/// the two functions, and the accompanying report called that "proven by test,
/// not assumed". It was not. Both functions are SECURITY DEFINER, and
/// PostgreSQL grants EXECUTE on a new function to PUBLIC by default; the
/// migration carried no `REVOKE ALL … FROM PUBLIC`, so every role in the
/// cluster already held EXECUTE. The migration now revokes first and exposes no
/// runtime product route, so both an otherwise ungranted role and `console_rt`
/// must receive `42501 insufficient_privilege`.
#[sqlx::test(migrations = "./migrations")]
async fn schema_derivation_is_owner_only(pool: PgPool) {
    assert_schema_derivation_owner_only(&pool).await;
}

// ---------------------------------------------------------------------------
// THE COMPLETENESS ASSERTION. Half the enforcement; the gate is the other half.
//
// The CI gate parses migration TEXT to decide which columns exist. Six rounds
// of hardening it produced a new escape each time, and the criticals were all
// one shape: a construct whose Rust re-implementation of PostgreSQL's parser
// disagrees with PostgreSQL — a schema qualifier dropped, a quoted identifier
// case-folded, a body the scanner did not recognise as a body, a multi-action
// `ALTER TABLE` judged from its FIRST action. Every one of them dissolves if the
// question is asked AFTER the DDL has run, because then the answer is not
// derived from syntax; it is read out of the catalog that the DDL produced.
//
// So this asks it there. Enumerate every column of every table in the
// application schemas from `pg_attribute`/`pg_class`/`pg_namespace`, and require
// each to carry a vocabulary-valid `pd:` marker or be NAMED in its table's entry
// in the declared backlog. `UNLOGGED`, quoting, schema qualification, `DO`
// blocks with single-quoted bodies, `LIKE INCLUDING ALL`, `PARTITION OF`,
// `INHERITS` and a keyword assembled out of concatenated fragments all land in
// `pg_attribute` the same way, because the catalog holds the RESULT and not the
// spelling.
//
// Five were planted as migrations and each failed HERE. Two of those five —
// `shadow.employees(raw_row)` and `"Employees"."RAW_ROW"` — the text gate
// reported PASSED on, because it reads both as the already-classified
// `employees.raw_row`. A third, `DO 'BEGIN CREATE TABLE …; END'`, it also
// passed at the time of that measurement and no longer does: a single-quoted
// `DO` body is now `unsupported-ddl` there. The other two, `CREATE UNLOGGED
// TABLE` and `(LIKE … INCLUDING ALL)`, it caught then and catches now.
//
// WHAT THE BASELINE BEING A SET, RATHER THAN A COUNT, CHANGED. A count is
// payable. Classifying one existing column in the same migration that adds an
// unclassified one leaves the number where it was, so this side saw nothing and
// only the gate's parser stood between that trade and a live unclassified
// column. Each of the six rounds was defeated by exactly that composite, most
// recently by this repository's own house idiom with one comma appended —
// `EXECUTE format('ALTER TABLE %I ENABLE ROW LEVEL SECURITY, ADD COLUMN
// medical_certificate_no TEXT', 'leave_requests')` — which measured gate EXIT=0,
// catalog EXIT=0, `3268 / 668 / 2600` against a clean `3267 / 667 / 2600`.
// Set membership has nothing to pay with: the same plant now fails here naming
// both halves, and so does the concatenation-split form the parser could never
// reach. For any relation this sweep reads, a parser blind spot no longer
// composes into a silent live column.
//
// THE GATE IS NOT REDUNDANT AND IS NOT BEING HARDENED FURTHER. It is the only
// thing that sees a column at WRITE time, before any database exists, and the
// catalog is consulted only for relations it knows. It stays as defence in
// depth. What it has stopped being is the thing this class depends on.
//
// WHAT THIS DOES NOT SEE, so that the scope is a sentence and not a guess:
//
// * `TEMP` tables, dropped by `c.relpersistence <> 't'` — PostgreSQL's own tag
//   on the relation, NOT the shape of its schema name. No `pg_` namespace is
//   skipped: `pg_catalog` is the single schema excluded by name, and a planted
//   `pg_evil`, `pg_`, `pg_catalog2` or `pg_toast` table is SEEN and fails. Do
//   not restore a `pg_%` prefix filter here — one used to live in this query
//   and it was exactly the hiding place those plants proved. Temp relations die
//   with the session that created them, so no migration can leave one holding
//   data, but a session that creates one at runtime is outside this assertion.
// * ANY relation created at RUNTIME rather than by a migration, temporary or
//   not. This runs against a database built from `./migrations` and nothing
//   else, so a permanent table that application code creates on the fly is
//   never in the catalog it reads. The gate cannot see one either — it is not
//   in the migration text — so this is a hole in BOTH checks and is closed by
//   neither. IT IS NOT HYPOTHETICAL; IT SHIPS TODAY.
//   `0005_create_compliance_location_store.sql:90-121` defines
//   `location_pings_create_day_partition()` / `location_pings_ensure_partition()`,
//   which run `EXECUTE format('CREATE TABLE IF NOT EXISTS %I PARTITION OF
//   location_pings …')` once per day. All ten columns of `location_pings` carry
//   `pd:personal — 개인위치정보` (0211:306-315), and every ping row is stored in
//   a day partition that carries no COMMENT of its own and appears in neither
//   reader: the migration text names it nowhere, and a catalog built from
//   `./migrations` holds only what the migrations made.
// * plain views. `relkind` takes `r`, `p`, `m` and `f`; `v` is excluded because
//   a view stores no row of its own and its content is its base tables', which
//   ARE checked. A MATERIALIZED view (`m`) does store rows and IS checked, as
//   is a foreign table (`f`) — whose rows live in another system entirely, so a
//   marker on one records a judgement about data this database does not hold.
// * whether any statutory obligation is met. A classification is an INPUT to
//   that question. Nothing here moves a compliance control off HOLD.
// ---------------------------------------------------------------------------

/// Tables carrying at least one unclassified column: the declared backlog, and
/// the SET of unclassified column NAMES in each. 247 tables, 2,621 names,
/// generated from the live catalog by `print_unclassified_baseline` below and
/// never typed by hand.
///
/// Listing a table here admits nobody has classified it. It is NOT a statement
/// that the table holds no personal data, and nothing derived from the
/// classification covers it.
///
/// WHY NAMES AND NOT A COUNT. This entry used to pin the NUMBER of unclassified
/// columns per table, and that is stated as a measurement rather than a
/// preference because the count was defeated. Six rounds of hardening the text
/// gate's parser each closed one spelling and each was followed by another, and
/// EVERY one of those escapes needed two things at once: a spelling the parser
/// misreads, AND a net count that did not move. The count is payable — classify
/// one existing column in the same migration that adds an unclassified one and
/// the table's number is unchanged. The last of them, measured on this tree, was
/// this repository's own house idiom with one comma appended:
///
/// ```sql
/// EXECUTE format('ALTER TABLE %I ENABLE ROW LEVEL SECURITY,
///                 ADD COLUMN medical_certificate_no TEXT', 'leave_requests')
/// ```
///
/// A SET has nothing to pay with. Adding an unclassified column makes the set
/// GAIN a name; classifying one in the same change makes membership MOVE. Both
/// fail, and how the DDL was spelled never enters the question — which is why
/// this side no longer depends on the parser for this class at all.
///
/// TWO-SIDED, and the second side is not bookkeeping. A name listed here that is
/// no longer unclassified — because it was classified, or dropped — also fails
/// and must be removed in the SAME change. A stale entry is a slot the next
/// unclassified column takes silently, which is the same hole one commit later.
///
/// SHRINK-ONLY, BY A MECHANISM AND NOT BY A RULE — AND ONE ROUTE IS NOT SHUT.
/// This list has no memory of its previous self and needs none: the catalog says
/// which columns exist and which carry a marker, so a name that is no longer
/// unclassified, an entry naming a table that is now fully classified, and an
/// entry naming a table that does not exist at all are each a failure. All three
/// are checked on every run.
///
/// The route that stays open is named here rather than papered over: a genuinely
/// NEW unclassified table, or a new name added to an existing entry, in the same
/// commit that creates the column, is accepted. No mechanism available to a test
/// that reads only the current catalog can distinguish that from an entry that
/// has been here since the backlog was declared — a catalog has no "when". What
/// is left is a diff hunk adding a line to a file whose first paragraph says what
/// adding a line means: a review surface, not an enforcement. The gate's
/// `BASELINE_FROZEN_AFTER_MIGRATION` clock aims at that same route over migration
/// text, where a "when" exists, with the blind spots its own doc names.
///
/// FIVE ENTRIES HERE ARE NOT APPLICATION TABLES, AND THAT IS THE POINT OF NOT
/// SPECIAL-CASING THEM. `_sqlx_migrations` is the migration ledger: sqlx creates
/// it, no migration in this tree does. The four `information_schema.*` entries
/// are the real tables PostgreSQL 18.4 ships there under
/// `relkind IN ('r','p','m','f')` — `sql_features`, `sql_implementation_info`,
/// `sql_parts`, `sql_sizing`, 21 columns, none of them commented; the rest of
/// that schema is `relkind = 'v'`. The sweep used to drop the whole schema with
/// an unexplained predicate, which is how a planted
/// `information_schema.pd_leak (rrn, employee_name)` produced counts
/// byte-identical to a clean tree. Declaring them costs five lines and makes an
/// undeclared table there fail like any other. A PostgreSQL version that ships a
/// different set fails the two-sided check by name — `names no table in the
/// migrated database`, or a table with no entry — rather than passing quietly.
const UNCLASSIFIED_TABLE_BASELINE: &[(&str, &[&str])] = &[
    (
        "information_schema.sql_features",
        &[
            "comments",
            "feature_id",
            "feature_name",
            "is_supported",
            "is_verified_by",
            "sub_feature_id",
            "sub_feature_name",
        ],
    ),
    (
        "information_schema.sql_implementation_info",
        &[
            "character_value",
            "comments",
            "implementation_info_id",
            "implementation_info_name",
            "integer_value",
        ],
    ),
    (
        "information_schema.sql_parts",
        &[
            "comments",
            "feature_id",
            "feature_name",
            "is_supported",
            "is_verified_by",
        ],
    ),
    (
        "information_schema.sql_sizing",
        &["comments", "sizing_id", "sizing_name", "supported_value"],
    ),
    (
        "_sqlx_migrations",
        &[
            "checksum",
            "description",
            "execution_time",
            "installed_on",
            "success",
            "version",
        ],
    ),
    (
        "annual_leave_obligations",
        &[
            "created_at",
            "employee_id",
            "id",
            "leave_accrued",
            "leave_remaining",
            "leave_used",
            "leave_year",
            "notification_plan",
            "org_id",
            "status",
            "statutory_basis",
            "updated_at",
            "workflow_object_id",
        ],
    ),
    (
        "attendance_close_amendments",
        &[
            "actor_user_id",
            "close_id",
            "created_at",
            "detail",
            "id",
            "idempotency_key",
            "org_id",
            "reason",
            "ref",
            "request_fingerprint",
        ],
    ),
    (
        "attendance_exception_resolutions",
        &[
            "action",
            "actor_user_id",
            "exception_id",
            "id",
            "linked_work_ref",
            "org_id",
            "ot_hours",
            "reason",
            "resolved_at",
        ],
    ),
    (
        "attendance_exceptions",
        &[
            "branch_id",
            "code",
            "created_at",
            "created_by",
            "detail",
            "employee_id",
            "evidence",
            "id",
            "idempotency_key",
            "kind",
            "links",
            "occurred_at",
            "org_id",
            "request_fingerprint",
            "status",
            "work_date",
        ],
    ),
    (
        "attendance_month_closes",
        &[
            "attested_at",
            "attested_by",
            "branch_id",
            "checks",
            "closed_at",
            "id",
            "month",
            "org_id",
            "period_lock_id",
        ],
    ),
    (
        "attendance_week52_acknowledgements",
        &[
            "acknowledged_at",
            "acknowledged_by_user_id",
            "employee_id",
            "id",
            "org_id",
            "week_start",
        ],
    ),
    (
        "audit_chain_seals",
        &[
            "batch_hash",
            "from_created_at",
            "from_event_id",
            "key_ref",
            "org_id",
            "prev_seal_hash",
            "row_count",
            "seal_hash",
            "sealed_at",
            "seq",
            "signature",
            "to_created_at",
            "to_event_id",
        ],
    ),
    (
        "audit_stream_event_labels",
        &[
            "audit_event_id",
            "created_at",
            "org_id",
            "sensitivity",
            "stream_key",
        ],
    ),
    (
        "auth_bootstrap_credentials",
        &[
            "consumed_at",
            "created_at",
            "expires_at",
            "id",
            "issued_at",
            "org_id",
            "registration_ceremony_id",
            "registration_started_at",
            "revoked_at",
            "revoked_reason",
            "token_hash",
            "user_id",
        ],
    ),
    (
        "auth_device_login_handoffs",
        &[
            "approve_token_hash",
            "approved_at",
            "approved_org_id",
            "approved_passkey_id",
            "approved_user_id",
            "consumed_at",
            "created_at",
            "expires_at",
            "id",
            "issued_at",
            "poll_token_hash",
            "target_org_id",
            "target_user_id",
        ],
    ),
    (
        "auth_rate_limit",
        &["attempts", "client_key", "endpoint", "window_start"],
    ),
    (
        "auth_refresh_token_families",
        &[
            "created_at",
            "id",
            "org_id",
            "revoked_at",
            "revoked_reason",
            "user_id",
        ],
    ),
    (
        "auth_refresh_tokens",
        &[
            "expires_at",
            "family_id",
            "id",
            "issued_at",
            "org_id",
            "replaced_by",
            "reuse_detected_at",
            "revoked_at",
            "token_hash",
            "used_at",
            "user_id",
        ],
    ),
    (
        "auth_webauthn_ceremonies",
        &[
            "ceremony_kind",
            "challenge_json",
            "consumed_at",
            "created_at",
            "expires_at",
            "id",
            "state_json",
            "user_id",
        ],
    ),
    (
        "auth_webauthn_ceremony_bindings",
        &[
            "action_kind",
            "ceremony_id",
            "created_at",
            "object_id",
            "reason_key",
            "replay_attempt",
        ],
    ),
    (
        "auth_webauthn_credentials",
        &[
            "created_at",
            "credential_id",
            "id",
            "last_used_at",
            "org_id",
            "passkey_json",
            "user_id",
        ],
    ),
    (
        "benefit_catalog_conditions",
        &[
            "benefit_id",
            "cedar_policy_ref",
            "condition_key",
            "condition_kind",
            "condition_value",
            "created_at",
            "created_by",
            "display_label",
            "display_order",
            "id",
            "operator",
            "org_id",
            "status",
            "updated_at",
            "updated_by",
        ],
    ),
    (
        "benefit_catalog_items",
        &[
            "benefit_code",
            "branch_id",
            "category",
            "cost_label",
            "coverage_label",
            "covered_count",
            "created_at",
            "created_by",
            "display_order",
            "effective_on",
            "employer_rate_bps",
            "estimated_annual_cost_won",
            "id",
            "legal_basis",
            "metadata",
            "name",
            "note",
            "org_id",
            "related_domain",
            "related_object_id",
            "retires_on",
            "scope_ref",
            "scope_type",
            "site_id",
            "updated_at",
            "updated_by",
        ],
    ),
    (
        "benefit_catalog_tiers",
        &[
            "amount_won",
            "benefit_id",
            "created_at",
            "created_by",
            "criteria",
            "display_order",
            "id",
            "limit_period",
            "org_id",
            "status",
            "tier_basis",
            "tier_key",
            "updated_at",
            "updated_by",
            "value_label",
        ],
    ),
    (
        "benefit_code_counters",
        &["next_value", "object_prefix", "org_id", "updated_at"],
    ),
    (
        "branches",
        &[
            "created_at",
            "deactivated_at",
            "id",
            "name",
            "org_id",
            "region_id",
        ],
    ),
    (
        "cedar_decision_log",
        &[
            "action",
            "actor",
            "created_at",
            "decided_at",
            "determining_policies",
            "effect",
            "id",
            "org_id",
            "reason",
            "resource_id",
            "resource_type",
            "subject_ref",
        ],
    ),
    (
        "cedar_policy_catalog_entries",
        &[
            "action",
            "bundle_digest",
            "cedar_language_version",
            "cedar_sdk_version",
            "conditions",
            "created_at",
            "created_by",
            "effect",
            "engine_mode",
            "generated_policy_text",
            "id",
            "natural_language_rule",
            "normalized_row",
            "org_id",
            "policy_version",
            "principal",
            "resource",
            "schema_version",
            "source",
            "stable_key",
            "status",
            "title",
            "updated_at",
            "updated_by",
            "validation_status",
        ],
    ),
    (
        "cedar_policy_catalog_normalization_blockers",
        &[
            "catalog_entry_id",
            "id",
            "org_id",
            "prior_status",
            "reason",
            "recorded_at",
            "resolution_note",
            "resolved_at",
        ],
    ),
    (
        "cedar_policy_drafts",
        &[
            "author_note",
            "blocks",
            "created_at",
            "created_by",
            "draft_key",
            "generated_policy_digest",
            "generated_policy_text",
            "id",
            "normalized_row",
            "org_id",
            "review_note",
            "review_status",
            "reviewer_id",
            "title",
            "updated_at",
            "updated_by",
            "validation_errors",
            "validation_status",
        ],
    ),
    (
        "clearance_assignments",
        &[
            "clearance_key",
            "created_at",
            "expires_at",
            "grant_reason",
            "granted_by",
            "id",
            "org_id",
            "revoked_by",
            "starts_at",
            "status",
            "stream_key",
            "updated_at",
            "user_id",
        ],
    ),
    (
        "collaboration_calendar_event_events",
        &[
            "action",
            "actor_id",
            "after_snap",
            "before_snap",
            "created_at",
            "event_id",
            "id",
            "org_id",
            "summary",
        ],
    ),
    (
        "collaboration_calendar_events",
        &[
            "all_day",
            "created_at",
            "created_by",
            "description",
            "ends_at",
            "id",
            "object_id",
            "object_type",
            "org_id",
            "scope_ref",
            "scope_type",
            "starts_at",
            "status",
            "title",
            "updated_at",
            "updated_by",
        ],
    ),
    (
        "collaboration_poll_events",
        &[
            "action",
            "actor_id",
            "after_snap",
            "before_snap",
            "created_at",
            "id",
            "org_id",
            "poll_id",
            "summary",
        ],
    ),
    (
        "collaboration_poll_options",
        &["created_at", "id", "label", "org_id", "poll_id", "position"],
    ),
    (
        "collaboration_poll_votes",
        &[
            "created_at",
            "id",
            "org_id",
            "poll_id",
            "selected_option_ids",
            "updated_at",
            "voter_id",
        ],
    ),
    (
        "collaboration_polls",
        &[
            "allow_multiple",
            "anonymity",
            "closes_at",
            "created_at",
            "created_by",
            "id",
            "object_id",
            "object_type",
            "org_id",
            "question",
            "status",
            "target_scope_ref",
            "target_scope_type",
            "title",
            "updated_at",
            "updated_by",
        ],
    ),
    (
        "comms_send_rate",
        &[
            "actor_user_id",
            "attempts",
            "created_at",
            "endpoint",
            "org_id",
            "updated_at",
            "window_start",
        ],
    ),
    (
        "compliance_code_counters",
        &["next_value", "object_prefix", "org_id", "updated_at"],
    ),
    (
        "compliance_control_obligations",
        &[
            "control_id",
            "coverage_level",
            "coverage_rationale",
            "created_at",
            "created_by",
            "id",
            "obligation_id",
            "org_id",
            "status",
            "updated_at",
            "updated_by",
        ],
    ),
    (
        "compliance_controls",
        &[
            "cadence",
            "control_key",
            "control_type",
            "created_at",
            "created_by",
            "evidence_requirements",
            "framework_id",
            "id",
            "objective",
            "org_id",
            "owner_user_id",
            "status",
            "title",
            "updated_at",
            "updated_by",
        ],
    ),
    (
        "compliance_evidence_bindings",
        &[
            "collected_at",
            "collected_by",
            "confidence",
            "control_id",
            "created_at",
            "created_by",
            "evidence_target_id",
            "evidence_target_type",
            "hash_sha256",
            "id",
            "metadata",
            "obligation_id",
            "org_id",
            "source_audit_event_id",
            "status",
            "updated_at",
            "updated_by",
            "valid_from",
            "valid_to",
        ],
    ),
    (
        "compliance_frameworks",
        &[
            "code",
            "created_at",
            "created_by",
            "effective_from",
            "effective_to",
            "framework_kind",
            "id",
            "metadata",
            "name",
            "org_id",
            "owner_user_id",
            "status",
            "updated_at",
            "updated_by",
            "version_label",
        ],
    ),
    (
        "compliance_obligation_regulations",
        &[
            "created_at",
            "created_by",
            "id",
            "obligation_id",
            "org_id",
            "rationale",
            "regulation_impact_id",
            "relationship",
        ],
    ),
    (
        "compliance_obligations",
        &[
            "branch_id",
            "code",
            "created_at",
            "created_by",
            "description",
            "effective_from",
            "effective_to",
            "id",
            "metadata",
            "next_review_on",
            "obligation_type",
            "org_id",
            "owner_user_id",
            "review_cadence",
            "scope_ref",
            "scope_type",
            "severity",
            "site_id",
            "status",
            "title",
            "updated_at",
            "updated_by",
        ],
    ),
    (
        "compliance_regulation_impacts",
        &[
            "citation",
            "code",
            "created_at",
            "created_by",
            "effective_from",
            "effective_to",
            "id",
            "impact_area",
            "impact_summary",
            "jurisdiction",
            "metadata",
            "org_id",
            "owner_user_id",
            "regulator",
            "review_due_on",
            "risk_level",
            "source_url",
            "status",
            "title",
            "updated_at",
            "updated_by",
        ],
    ),
    (
        "console_route_telemetry",
        &[
            "duration_ms",
            "error_name",
            "event_kind",
            "id",
            "occurred_at",
            "org_id",
            "release_cycle",
            "route_path",
            "route_surface",
            "user_id",
        ],
    ),
    (
        "consulting_benefit_observations",
        &[
            "created_at",
            "created_by",
            "engagement_id",
            "evidence_id",
            "id",
            "initiative_id",
            "kpi_definition_id",
            "note",
            "observed_at",
            "org_id",
        ],
    ),
    (
        "consulting_diagnostics",
        &[
            "created_at",
            "created_by",
            "document_id",
            "engagement_id",
            "id",
            "org_id",
            "summary",
        ],
    ),
    (
        "consulting_engagement_history",
        &[
            "actor_id",
            "engagement_id",
            "event_type",
            "from_status",
            "id",
            "occurred_at",
            "org_id",
            "payload",
            "to_status",
            "version",
        ],
    ),
    (
        "consulting_engagements",
        &[
            "approval_id",
            "created_at",
            "created_by",
            "customer_document_id",
            "customer_id",
            "id",
            "idempotency_key",
            "idempotency_request_hash",
            "idempotency_response",
            "idempotency_response_status",
            "ontology_instance_id",
            "org_id",
            "status",
            "title",
            "updated_at",
            "version",
            "workflow_execution_id",
        ],
    ),
    (
        "consulting_findings",
        &[
            "created_at",
            "created_by",
            "diagnostic_id",
            "document_id",
            "engagement_id",
            "evidence_id",
            "id",
            "org_id",
            "statement",
        ],
    ),
    (
        "consulting_initiatives",
        &[
            "created_at",
            "created_by",
            "engagement_id",
            "finding_id",
            "hypothesis",
            "id",
            "kpi_definition_id",
            "org_id",
            "target_direction",
            "title",
        ],
    ),
    (
        "consulting_reference_bindings",
        &[
            "evaluated_at",
            "evaluated_by",
            "id",
            "org_id",
            "source_id",
            "source_kind",
            "source_version",
        ],
    ),
    (
        "daily_work_plan_items",
        &[
            "created_at",
            "description",
            "id",
            "org_id",
            "plan_id",
            "sort_order",
            "work_order_id",
        ],
    ),
    (
        "daily_work_plans",
        &[
            "branch_id",
            "confirmed_at",
            "created_at",
            "id",
            "mechanic_id",
            "org_id",
            "plan_date",
            "requested_at",
            "review_memo",
            "reviewed_at",
            "reviewed_by",
            "status",
            "updated_at",
        ],
    ),
    (
        "data_import_runs",
        &[
            "applied_at",
            "applied_by",
            "apply_summary",
            "candidate_rows",
            "created_at",
            "created_by",
            "dry_run_summary",
            "entity_type",
            "id",
            "input_rows",
            "mapping_profile",
            "org_id",
            "preserved_rows",
            "source_filename",
            "source_format",
            "source_sha256",
            "status",
            "updated_at",
        ],
    ),
    (
        "docs_equipment_handover_custody",
        &[
            "branch_id",
            "created_at",
            "created_by",
            "equipment_case_id",
            "evidence_object_id",
            "id",
            "org_id",
            "original_copy_id",
        ],
    ),
    (
        "docs_evidence_code_counters",
        &["next_value", "object_prefix", "org_id", "updated_at"],
    ),
    (
        "docs_evidence_copies",
        &[
            "content_type",
            "copy_kind",
            "created_at",
            "created_by",
            "derivative_kind",
            "digest_sha256",
            "evidence_object_id",
            "evidentiary_status",
            "id",
            "org_id",
            "parent_copy_id",
            "size_bytes",
            "source_evidence_media_id",
            "storage_key_ref",
            "storage_object_id",
            "storage_provider",
            "storage_version_id",
            "verified_at",
            "worm_status",
        ],
    ),
    (
        "docs_evidence_exports",
        &[
            "audit_event_id",
            "custody_event_id",
            "evidence_object_id",
            "export_reason",
            "exported_at",
            "exported_by",
            "id",
            "manifest_digest_sha256",
            "org_id",
            "signature_algorithm",
            "signature_ref",
        ],
    ),
    (
        "docs_evidence_legal_holds",
        &[
            "applied_at",
            "applied_by",
            "audit_event_id",
            "basis",
            "case_ref",
            "evidence_object_id",
            "id",
            "org_id",
            "reason",
            "release_reason",
            "released_at",
            "released_by",
            "status",
        ],
    ),
    (
        "docs_evidence_objects",
        &[
            "admissibility_inputs",
            "admissibility_reasons",
            "admissibility_status",
            "classification",
            "code",
            "created_at",
            "created_by",
            "current_custody_stage",
            "description",
            "disposal_reason",
            "disposed_at",
            "disposed_by",
            "id",
            "legal_hold_state",
            "org_id",
            "record_owner_user_id",
            "register_sequence",
            "source_code",
            "source_id",
            "source_type",
            "title",
            "updated_at",
            "updated_by",
        ],
    ),
    (
        "docs_evidence_tsa_proofs",
        &[
            "accuracy_millis",
            "copy_id",
            "created_at",
            "created_by",
            "evidence_object_id",
            "failure_reason",
            "generated_at",
            "hash_algorithm",
            "id",
            "message_imprint_sha256",
            "ordering",
            "org_id",
            "policy_oid",
            "provider",
            "serial_number",
            "status",
            "token_digest_sha256",
            "token_storage_key_ref",
            "token_storage_object_id",
            "token_storage_provider",
            "token_storage_version_id",
            "tsa_cert_fingerprint_sha256",
            "verified_at",
        ],
    ),
    (
        "document_versions",
        &[
            "byte_size",
            "content_hash",
            "created_at",
            "created_by",
            "document_ref",
            "file_type",
            "id",
            "org_id",
            "restored_from",
            "source_key",
            "storage_key",
            "version_no",
        ],
    ),
    (
        "email_folders",
        &[
            "account_id",
            "created_at",
            "highest_modseq",
            "id",
            "imap_path",
            "last_seen_uid",
            "last_synced_at",
            "name",
            "org_id",
            "role",
            "total_count",
            "uid_validity",
            "unread_count",
            "updated_at",
        ],
    ),
    (
        "employee_absence_alerts",
        &[
            "acknowledged_at",
            "acknowledged_by",
            "audience_roles",
            "branch_id",
            "created_at",
            "detected_at",
            "employee_id",
            "id",
            "linked_exit_case_id",
            "org_id",
            "severity",
            "signal_payload",
            "source",
            "status",
            "updated_at",
            "work_date",
        ],
    ),
    (
        "employee_attendance_records",
        &[
            "actor_user_id",
            "created_at",
            "employee_id",
            "id",
            "idempotency_key",
            "kind",
            "note",
            "occurred_at",
            "org_id",
            "state_after",
            "work_date",
        ],
    ),
    (
        "employee_create_idempotency",
        &[
            "created_at",
            "employee_id",
            "idempotency_key",
            "org_id",
            "request_hash",
        ],
    ),
    (
        "employee_exit_cases",
        &[
            "absence_alert_id",
            "approval_submitted_at",
            "approval_submitted_by",
            "branch_id",
            "confirmation_note",
            "created_at",
            "effective_exit_date",
            "employee_id",
            "hq_confirmed_at",
            "hq_confirmed_by",
            "hr_confirmed_at",
            "hr_confirmed_by",
            "id",
            "org_id",
            "reported_at",
            "reported_by",
            "site_manager_note",
            "status",
            "updated_at",
        ],
    ),
    (
        "employee_lifecycle_events",
        &[
            "comment",
            "created_at",
            "created_by",
            "effective_date",
            "employee_id",
            "event_type",
            "from_company",
            "from_org_unit",
            "from_position",
            "from_status",
            "id",
            "org_id",
            "signoffs",
            "to_company",
            "to_org_unit",
            "to_position",
            "to_status",
        ],
    ),
    (
        "equipment_3r_history",
        &[
            "actor_id",
            "aggregate_id",
            "aggregate_kind",
            "branch_id",
            "id",
            "occurred_at",
            "org_id",
            "trace_id",
            "transition",
        ],
    ),
    (
        "equipment_3r_inspections",
        &[
            "branch_id",
            "case_id",
            "findings",
            "id",
            "inspected_at",
            "inspected_by",
            "maintenance_note",
            "org_id",
            "outcome",
        ],
    ),
    (
        "equipment_3r_return_assessments",
        &[
            "assessed_at",
            "assessed_by",
            "branch_id",
            "case_id",
            "condition_grade",
            "disposition",
            "findings",
            "id",
            "org_id",
        ],
    ),
    (
        "equipment_3r_units",
        &[
            "acquisition_cost_minor",
            "availability",
            "branch_id",
            "capacity_class",
            "created_at",
            "created_by",
            "id",
            "model_name",
            "org_id",
            "serial_no",
            "updated_at",
        ],
    ),
    (
        "equipment_cost_ledger",
        &[
            "amount_won",
            "branch_id",
            "created_at",
            "created_by",
            "entry_at",
            "equipment_id",
            "id",
            "memo",
            "org_id",
            "purchase_request_id",
            "residual_after_won",
            "residual_before_won",
            "source",
            "work_order_id",
        ],
    ),
    (
        "equipment_maintenance_history",
        &[
            "completed_at",
            "created_at",
            "equipment_id",
            "id",
            "org_id",
            "work_order_id",
        ],
    ),
    (
        "equipment_maintenance_history_costs",
        &["equipment_cost_ledger_id", "history_id", "org_id"],
    ),
    (
        "equipment_maintenance_history_evidence",
        &["evidence_media_id", "history_id", "org_id"],
    ),
    (
        "equipment_ownership_transfer_events",
        &[
            "action",
            "actor_id",
            "comment",
            "created_at",
            "id",
            "org_id",
            "request_id",
            "snapshot",
            "step_key",
        ],
    ),
    (
        "equipment_ownership_transfer_requests",
        &[
            "approval_line",
            "branch_id",
            "completed_at",
            "current_step",
            "decided_at",
            "equipment_id",
            "from_owner",
            "id",
            "org_id",
            "reason",
            "requested_at",
            "requested_by",
            "status",
            "to_owner",
            "updated_at",
        ],
    ),
    (
        "equipment_substitutions",
        &[
            "assigned_at",
            "assigned_by",
            "assigned_to",
            "assignment_location",
            "branch_id",
            "created_at",
            "id",
            "org_id",
            "return_note",
            "returned_at",
            "returned_by",
            "source_equipment_id",
            "substitute_equipment_id",
            "updated_at",
        ],
    ),
    (
        "erasure_ledger",
        &[
            "actor",
            "authority",
            "effective_at",
            "entry_hash",
            "erased_relation",
            "erased_row_count",
            "erased_selector",
            "org_id",
            "prev_entry_hash",
            "recorded_at",
            "seq",
            "subject_digest",
            "subject_kind",
        ],
    ),
    ("evaluation_code_counters", &["next_value", "org_id"]),
    (
        "evaluation_cycles",
        &[
            "archived_at",
            "calibration_started_at",
            "created_at",
            "created_by",
            "due_date",
            "finalized_at",
            "id",
            "kind",
            "name",
            "opened_at",
            "org_id",
            "period_label",
            "stage",
            "updated_at",
        ],
    ),
    (
        "evaluation_evidence_links",
        &[
            "created_at",
            "id",
            "label",
            "object_kind",
            "object_ref",
            "org_id",
            "review_id",
            "sort_order",
        ],
    ),
    (
        "evaluation_goals",
        &[
            "created_at",
            "id",
            "metric_kind",
            "org_id",
            "sort_order",
            "subject_id",
            "target_label",
            "title",
            "weight_pct",
        ],
    ),
    (
        "evaluation_reviews",
        &[
            "created_at",
            "evaluator_user_id",
            "grade",
            "id",
            "kind",
            "note",
            "org_id",
            "status",
            "subject_id",
            "submitted_at",
            "updated_at",
        ],
    ),
    (
        "evaluation_subjects",
        &[
            "calibrated_at",
            "calibrated_by",
            "calibrated_grade",
            "calibration_reason",
            "created_at",
            "cycle_id",
            "employee_id",
            "final_grade",
            "finalized_at",
            "id",
            "manager_user_id",
            "org_id",
            "rv_code",
            "updated_at",
        ],
    ),
    (
        "evidence_media",
        &[
            "checksum_sha256",
            "confirmed_by",
            "content_type",
            "created_at",
            "id",
            "last_error",
            "next_retry_at",
            "org_id",
            "original_content_type",
            "processed_at",
            "processing_error",
            "processing_status",
            "retry_count",
            "s3_key",
            "size_bytes",
            "stage",
            "staging_s3_key",
            "thumbnail_s3_key",
            "updated_at",
            "upload_confirmed_at",
            "uploaded_by",
            "verified_at",
            "work_order_id",
            "worm_replica_status",
        ],
    ),
    (
        "excel_export_logs",
        &[
            "actor",
            "branch_id",
            "created_at",
            "export_date",
            "export_kind",
            "file_name",
            "id",
            "org_id",
            "scope_key",
            "source_notes",
        ],
    ),
    (
        "facilities_acceptances",
        &[
            "actor_id",
            "case_id",
            "decided_at",
            "decision",
            "id",
            "org_id",
            "reason",
        ],
    ),
    (
        "facilities_assets",
        &[
            "asset_tag",
            "branch_id",
            "catalog_service_id",
            "created_at",
            "id",
            "name",
            "org_id",
            "site_id",
            "space_id",
        ],
    ),
    (
        "facilities_case_history",
        &[
            "actor_id",
            "case_id",
            "from_status",
            "id",
            "occurred_at",
            "org_id",
            "receipt",
            "to_status",
        ],
    ),
    (
        "facilities_cases",
        &[
            "acceptance_due_at",
            "assignee_id",
            "branch_id",
            "completion_due_at",
            "created_at",
            "id",
            "idempotency_key",
            "obligation_id",
            "occurrence_due_at",
            "org_id",
            "request_hash",
            "response_due_at",
            "safety_acknowledged_at",
            "scheduled_for",
            "site_id",
            "status",
            "updated_at",
        ],
    ),
    (
        "facilities_catalog_services",
        &["created_at", "id", "name", "org_id", "service_key"],
    ),
    (
        "facilities_cost_observations",
        &[
            "amount_krw",
            "case_id",
            "created_at",
            "currency",
            "id",
            "observed_at",
            "org_id",
            "recorded_by",
            "source",
        ],
    ),
    (
        "facilities_energy_observations",
        &[
            "case_id",
            "created_at",
            "id",
            "kwh",
            "observed_at",
            "org_id",
            "phase",
            "recorded_by",
            "source",
        ],
    ),
    (
        "facilities_execution_evidence_links",
        &[
            "case_id",
            "evidence_id",
            "evidence_kind",
            "id",
            "linked_at",
            "linked_by",
            "org_id",
        ],
    ),
    (
        "facilities_obligations",
        &[
            "acceptance_due_seconds",
            "active",
            "asset_id",
            "branch_id",
            "catalog_service_id",
            "completion_due_seconds",
            "created_at",
            "customer_acceptance_required",
            "energy_formula",
            "id",
            "next_due_at",
            "org_id",
            "recurrence_days",
            "response_due_seconds",
            "site_id",
            "target_energy_kwh",
        ],
    ),
    (
        "facilities_spaces",
        &["branch_id", "created_at", "id", "name", "org_id", "site_id"],
    ),
    ("feature_catalog", &["created_at", "feature_key"]),
    (
        "finance_gl_voucher_lines",
        &[
            "account_code",
            "amount_won",
            "id",
            "line_no",
            "memo",
            "org_id",
            "side",
            "voucher_id",
        ],
    ),
    (
        "finance_gl_vouchers",
        &[
            "approved_by",
            "branch_id",
            "created_at",
            "created_by",
            "id",
            "memo",
            "org_id",
            "posted_at",
            "reversal_of_voucher_id",
            "reversed_by_voucher_id",
            "source_object_id",
            "source_object_type",
            "status",
            "updated_at",
            "voucher_no",
        ],
    ),
    (
        "financial_expense_ledger",
        &[
            "amount_won",
            "branch_id",
            "created_at",
            "executed_at",
            "executed_by",
            "expenditure_no",
            "id",
            "memo",
            "org_id",
            "purchase_request_id",
            "vendor_name",
        ],
    ),
    (
        "financial_purchase_attachments",
        &[
            "branch_id",
            "checksum_sha256",
            "content_type",
            "created_at",
            "file_name",
            "id",
            "org_id",
            "purchase_request_id",
            "role",
            "s3_bucket",
            "s3_key",
            "size_bytes",
            "upload_state",
            "uploaded_by",
        ],
    ),
    (
        "financial_purchase_history",
        &[
            "action",
            "actor",
            "created_at",
            "from_status",
            "id",
            "memo",
            "occurred_at",
            "org_id",
            "purchase_request_id",
            "to_status",
        ],
    ),
    (
        "financial_purchase_request_lines",
        &[
            "created_at",
            "id",
            "item",
            "line_no",
            "line_total_won",
            "org_id",
            "purchase_request_id",
            "quantity",
            "unit_supply_price_won",
            "vat_overridden",
            "vat_won",
        ],
    ),
    (
        "financial_purchase_requests",
        &[
            "admin_approved_by",
            "amount_won",
            "branch_id",
            "created_at",
            "declining_balance_rate_bps",
            "depreciation_method",
            "equipment_id",
            "executed_by",
            "executive_approved_by",
            "executive_threshold_won",
            "expenditure_no",
            "floor_negative_quote_residual",
            "id",
            "management_fee_rate_bps",
            "memo",
            "org_id",
            "price_anomaly",
            "profit_rate_bps",
            "purchase_type",
            "quote_update_required",
            "rejected_by",
            "rejection_memo",
            "requested_by",
            "residual_rate_bps",
            "statement_evidence_id",
            "status",
            "submitted_by",
            "updated_at",
            "useful_life_months",
            "vendor_name",
            "work_order_id",
        ],
    ),
    (
        "financial_regular_purchase_prices",
        &[
            "branch_id",
            "id",
            "item_norm",
            "last_unit_supply_price_won",
            "org_id",
            "quote_attachment_id",
            "updated_at",
            "updated_from_purchase_request_id",
            "vendor_name_norm",
        ],
    ),
    (
        "financial_rental_quote_lines",
        &[
            "amount_won",
            "code",
            "id",
            "label",
            "line_order",
            "org_id",
            "quote_id",
        ],
    ),
    (
        "financial_rental_quotes",
        &[
            "acquisition_value_won",
            "branch_id",
            "created_at",
            "created_by",
            "cumulative_repair_cost_won",
            "current_residual_value_won",
            "declining_balance_rate_bps",
            "depreciation_method",
            "effective_residual_value_won",
            "equipment_id",
            "floor_negative_quote_residual",
            "id",
            "management_fee_rate_bps",
            "monthly_total_won",
            "org_id",
            "profit_rate_bps",
            "residual_rate_bps",
            "residual_was_floored",
            "updated_at",
            "useful_life_months",
        ],
    ),
    (
        "gov_approval_consumptions",
        &["approval_id", "consumed_at", "consumed_by", "id", "org_id"],
    ),
    (
        "gov_approval_requests",
        &[
            "created_at",
            "id",
            "kind",
            "org_id",
            "payload_summary",
            "request_ref",
            "requested_by",
            "target_ref",
        ],
    ),
    (
        "gov_approvals",
        &[
            "approver_id",
            "decided_at",
            "decision",
            "id",
            "kind",
            "org_id",
            "request_ref",
            "requested_by",
            "target_ref",
        ],
    ),
    (
        "gov_lifecycle_transitions",
        &[
            "created_at",
            "created_by",
            "from_state",
            "id",
            "object_type_id",
            "org_id",
            "requires_checklist",
            "requires_four_eyes",
            "requires_reason",
            "to_state",
            "updated_at",
        ],
    ),
    (
        "gov_overrides",
        &[
            "actor",
            "before_snapshot",
            "created_at",
            "id",
            "org_id",
            "reason",
            "target_id",
            "target_type",
        ],
    ),
    (
        "governance_findings",
        &[
            "created_at",
            "detected_at",
            "detector_id",
            "entity_id",
            "entity_type",
            "evidence",
            "id",
            "org_id",
            "review_memo",
            "reviewed_at",
            "reviewed_by",
            "score",
            "severity",
            "source_audit_event_id",
            "status",
            "subject_user_id",
            "updated_at",
        ],
    ),
    ("group_memberships", &["created_at", "group_id", "org_id"]),
    (
        "group_role_grants",
        &[
            "created_at",
            "granted_by",
            "group_id",
            "group_role",
            "id",
            "user_id",
        ],
    ),
    (
        "groups",
        &["created_at", "id", "name", "slug", "status", "updated_at"],
    ),
    (
        "inbox_docs",
        &[
            "confirmed_at",
            "confirmed_by",
            "created_at",
            "dedup_key",
            "id",
            "kind",
            "legal_basis",
            "notice_type",
            "org_id",
            "payload",
            "recipient_user_id",
            "source_id",
            "source_kind",
            "title",
        ],
    ),
    (
        "inspection_rounds",
        &[
            "branch_id",
            "completed_at",
            "completed_by",
            "created_at",
            "equipment_id",
            "findings",
            "id",
            "mechanic_id",
            "note",
            "org_id",
            "outcome",
            "schedule_id",
        ],
    ),
    (
        "inventory_consumption_events",
        &[
            "branch_id",
            "consumed_by",
            "cost_won",
            "created_at",
            "dispatch_id",
            "id",
            "idempotency_key",
            "item_id",
            "memo",
            "occurred_at",
            "org_id",
            "quantity_after_milli",
            "quantity_before_milli",
            "quantity_consumed_milli",
            "request_fingerprint",
            "source_kind",
            "stock_location_id",
            "unit_cost_won",
            "work_order_id",
        ],
    ),
    ("inventory_cycle_count_counters", &["last_value", "org_id"]),
    (
        "inventory_cycle_count_lines",
        &[
            "count_id",
            "counted_quantity_milli",
            "created_at",
            "id",
            "item_id",
            "note",
            "org_id",
            "reason",
            "recorded_at",
            "recorded_by",
            "system_quantity_milli",
            "updated_at",
            "variance_milli",
        ],
    ),
    (
        "inventory_cycle_counts",
        &[
            "branch_id",
            "cc_code",
            "created_at",
            "decided_at",
            "decided_by",
            "decision_idempotency_key",
            "decision_memo",
            "decision_request_fingerprint",
            "id",
            "opened_by",
            "org_id",
            "status",
            "stock_location_id",
            "submitted_at",
            "submitted_by",
            "updated_at",
            "version",
        ],
    ),
    (
        "inventory_items",
        &[
            "branch_id",
            "created_at",
            "created_by",
            "description",
            "display_name",
            "id",
            "iv_code",
            "org_id",
            "quantity_on_hand_milli",
            "safety_stock_milli",
            "site_id",
            "sku",
            "status",
            "stock_location_id",
            "unit_code",
            "unit_cost_won",
            "updated_at",
        ],
    ),
    (
        "inventory_movements",
        &[
            "actor_id",
            "branch_id",
            "created_at",
            "cycle_count_id",
            "id",
            "idempotency_key",
            "item_id",
            "kind",
            "memo",
            "occurred_at",
            "org_id",
            "quantity_after_milli",
            "quantity_before_milli",
            "quantity_delta_milli",
            "request_fingerprint",
            "source_ref",
            "stock_location_id",
        ],
    ),
    (
        "inventory_stock_locations",
        &[
            "branch_id",
            "created_at",
            "id",
            "label",
            "location_code",
            "org_id",
            "site_id",
            "status",
            "updated_at",
        ],
    ),
    (
        "kpi_exclusions",
        &[
            "branch_id",
            "created_at",
            "excluded_at",
            "excluded_by",
            "id",
            "org_id",
            "reason",
            "revoked_at",
            "revoked_by",
            "scope",
            "target_id",
            "updated_at",
        ],
    ),
    (
        "leave_balance_import_receipts",
        &[
            "actor",
            "changed",
            "created_at",
            "employee_id",
            "id",
            "idempotency_key",
            "org_id",
            "payload_digest",
            "result_updated_at",
            "source_kind",
            "source_ref",
            "span_id",
            "trace_id",
        ],
    ),
    (
        "leave_charge_resolutions",
        &[
            "calendar_revision_ref",
            "charge_units",
            "charge_version",
            "created_at",
            "date_charges",
            "home_branch_id",
            "id",
            "org_id",
            "policy_revision_ref",
            "request_id",
            "resolution_origin",
            "resolved_at",
            "resolved_by",
            "server_digest",
            "snapshot",
            "supporting_source_refs",
        ],
    ),
    (
        "leave_promotions",
        &[
            "ap_run_id",
            "branch_id",
            "created_at",
            "created_by",
            "id",
            "inbox_doc_id",
            "kind",
            "legal_basis",
            "org_id",
            "round",
            "target_employee_id",
            "target_user_id",
        ],
    ),
    (
        "leave_requests",
        &[
            "ap_run_id",
            "branch_id",
            "charge_review_reasons",
            "charge_state",
            "charge_units",
            "charge_version",
            "created_at",
            "current_charge_resolution_id",
            "days",
            "decided_at",
            "decided_by",
            "decision_comment",
            "end_date",
            "id",
            "leave_type",
            "legacy_days",
            "org_id",
            "partial_day_period",
            "reason",
            "request_version",
            "requester_user_id",
            "start_date",
            "status",
            "subject_employee_id",
            "submission_digest",
            "submission_initial_charge_version",
            "submission_key",
        ],
    ),
    (
        "lifecycle_transition_rules",
        &["created_at", "from_state", "object_type", "to_state"],
    ),
    (
        "link_types",
        &["created_at", "description", "link_type", "status"],
    ),
    (
        "location_collection_logs",
        &[
            "branch_id",
            "created_at",
            "id",
            "org_id",
            "ping_id",
            "reason",
            "recorded_at",
            "user_id",
        ],
    ),
    (
        "logistics_asns",
        &[
            "branch_id",
            "created_at",
            "created_by",
            "expected_quantity",
            "external_reference",
            "id",
            "org_id",
            "received_quantity",
            "sku",
            "status",
            "updated_at",
            "warehouse_code",
        ],
    ),
    (
        "logistics_fulfillments",
        &[
            "branch_id",
            "created_at",
            "created_by",
            "due_at",
            "id",
            "org_id",
            "picked_quantity",
            "requested_quantity",
            "reserved_quantity",
            "sku",
            "status",
            "updated_at",
            "warehouse_code",
        ],
    ),
    (
        "logistics_history",
        &[
            "actor_id",
            "aggregate_id",
            "aggregate_kind",
            "branch_id",
            "id",
            "occurred_at",
            "org_id",
            "trace_id",
            "transition",
        ],
    ),
    (
        "logistics_operational_cost_settlements",
        &[
            "amount_minor",
            "branch_id",
            "currency_code",
            "id",
            "org_id",
            "settled_at",
            "shipment_id",
        ],
    ),
    (
        "logistics_pod_evidence",
        &[
            "branch_id",
            "confirmed_at",
            "evidence_reference",
            "id",
            "org_id",
            "recipient_name",
            "shipment_id",
        ],
    ),
    (
        "logistics_receipts",
        &[
            "asn_id",
            "branch_id",
            "exception_code",
            "id",
            "idempotency_key",
            "org_id",
            "received_at",
            "received_by",
            "received_quantity",
            "request_fingerprint",
        ],
    ),
    (
        "logistics_shipments",
        &[
            "branch_id",
            "carrier_name",
            "dispatched_at",
            "fulfillment_id",
            "id",
            "org_id",
            "status",
            "vehicle_reference",
        ],
    ),
    (
        "logistics_stock",
        &[
            "branch_id",
            "org_id",
            "quantity_on_hand",
            "quantity_reserved",
            "sku",
            "updated_at",
            "warehouse_code",
        ],
    ),
    (
        "mailbox_aliases",
        &[
            "alias_kind",
            "created_at",
            "created_by",
            "domain_id",
            "id",
            "local_part",
            "org_id",
            "status",
            "target_mailbox_id",
            "updated_at",
        ],
    ),
    (
        "mailbox_domains",
        &[
            "created_at",
            "created_by",
            "dkim_private_key_ref",
            "dkim_public_key_ref",
            "dkim_selector",
            "dkim_verified",
            "dmarc_verified",
            "dns_last_checked_at",
            "domain",
            "group_id",
            "id",
            "last_error_code",
            "mta_sts_verified",
            "mx_verified",
            "org_id",
            "spf_verified",
            "status",
            "tls_rpt_verified",
            "updated_at",
            "verification_status",
        ],
    ),
    (
        "me_workspace_layouts",
        &["layout", "org_id", "updated_at", "user_id"],
    ),
    (
        "message_refs",
        &[
            "created_at",
            "id",
            "message_id",
            "org_id",
            "ref_code",
            "ref_kind",
        ],
    ),
    (
        "messenger_message_acks",
        &["acked_at", "message_id", "org_id", "user_id"],
    ),
    (
        "messenger_message_attachments",
        &["evidence_id", "message_id", "org_id", "sort_order"],
    ),
    (
        "messenger_presence",
        &["last_activity_at", "org_id", "updated_at", "user_id"],
    ),
    (
        "messenger_read_receipts",
        &[
            "last_read_message_id",
            "org_id",
            "read_at",
            "thread_id",
            "updated_at",
            "user_id",
        ],
    ),
    (
        "messenger_thread_members",
        &["joined_at", "org_id", "role", "thread_id", "user_id"],
    ),
    (
        "messenger_thread_mutes",
        &["muted_at", "org_id", "thread_id", "user_id"],
    ),
    (
        "messenger_threads",
        &[
            "branch_id",
            "created_at",
            "created_by",
            "id",
            "kind",
            "org_id",
            "title",
            "updated_at",
            "visibility",
            "work_order_id",
        ],
    ),
    (
        "notice_audience_branches",
        &["branch_id", "created_at", "notice_id", "org_id"],
    ),
    (
        "notice_receipts",
        &[
            "acknowledged_at",
            "created_at",
            "id",
            "notice_id",
            "org_id",
            "recipient_user_id",
        ],
    ),
    (
        "notification_policies",
        &[
            "action",
            "category",
            "created_at",
            "id",
            "link",
            "org_id",
            "scope",
            "updated_at",
            "user_id",
        ],
    ),
    ("object_code_counters", &["kind", "last_sequence", "org_id"]),
    (
        "object_lifecycle_transitions",
        &[
            "actor",
            "from_state",
            "id",
            "lifecycle_id",
            "occurred_at",
            "org_id",
            "reason",
            "to_state",
        ],
    ),
    (
        "object_lifecycles",
        &[
            "created_at",
            "current_state",
            "id",
            "legal_hold",
            "object_id",
            "object_type",
            "org_id",
            "retention_until",
            "updated_at",
        ],
    ),
    (
        "object_links",
        &[
            "created_at",
            "created_by",
            "dst_id",
            "dst_kind",
            "id",
            "link_type",
            "org_id",
            "src_id",
            "src_kind",
        ],
    ),
    (
        "object_types",
        &["code_prefix", "created_at", "description", "kind", "status"],
    ),
    (
        "ont_action_command_receipts",
        &[
            "actor_id",
            "command_id",
            "created_at",
            "org_id",
            "payload_digest",
            "receipt",
        ],
    ),
    (
        "ont_action_types",
        &[
            "control_points",
            "created_at",
            "dispatch",
            "dispatch_target",
            "edits",
            "id",
            "object_type_id",
            "org_id",
            "params_schema",
            "side_effects",
            "stable_key",
            "submission_criteria",
            "title",
        ],
    ),
    (
        "ont_analytics",
        &[
            "created_at",
            "formula",
            "id",
            "key",
            "object_type_id",
            "org_id",
            "result_type",
            "title",
        ],
    ),
    (
        "ont_builtin_catalog_allowlist",
        &["catalog_version", "created_at", "manifest_digest"],
    ),
    (
        "ont_builtin_catalog_installs",
        &[
            "catalog_version",
            "installed_at",
            "installed_by",
            "manifest_digest",
            "org_id",
        ],
    ),
    (
        "ont_instance_revisions",
        &[
            "action_type_id",
            "actor",
            "attributes",
            "created_at",
            "id",
            "instance_id",
            "org_id",
            "prev_hash",
            "reason",
            "row_hash",
            "valid_from",
            "valid_to",
            "version",
        ],
    ),
    (
        "ont_instances",
        &[
            "created_at",
            "current_revision_id",
            "id",
            "lifecycle_state",
            "object_type_id",
            "org_id",
            "title",
            "updated_at",
        ],
    ),
    (
        "ont_link_types",
        &[
            "cardinality",
            "created_at",
            "id",
            "object_type_id",
            "org_id",
            "reverse_title",
            "stable_key",
            "title",
            "to_object_type_id",
            "traversable",
        ],
    ),
    (
        "ont_links",
        &[
            "created_at",
            "from_instance_id",
            "id",
            "link_type_id",
            "org_id",
            "to_instance_id",
            "valid_from",
            "valid_to",
        ],
    ),
    (
        "ont_object_policies",
        &[
            "cedar_policy_id",
            "created_at",
            "created_by",
            "effect",
            "id",
            "object_type_id",
            "org_id",
        ],
    ),
    (
        "ont_object_type_key_revisions",
        &[
            "created_at",
            "org_id",
            "revision",
            "stable_key",
            "updated_at",
            "validator_id",
        ],
    ),
    (
        "ont_object_types",
        &[
            "backing_kind",
            "backing_table",
            "created_at",
            "created_by",
            "id",
            "lifecycle_state",
            "org_id",
            "primary_key_property",
            "schema_version",
            "stable_key",
            "title",
            "title_property_key",
            "updated_at",
        ],
    ),
    (
        "ont_property_defs",
        &[
            "backing_column",
            "config",
            "created_at",
            "id",
            "in_property_policy",
            "key",
            "object_type_id",
            "org_id",
            "required",
            "title",
            "type",
        ],
    ),
    (
        "ont_property_policies",
        &[
            "cedar_policy_id",
            "created_at",
            "created_by",
            "id",
            "org_id",
            "property_def_id",
        ],
    ),
    (
        "org_change_approval_steps",
        &[
            "decided_at",
            "decided_by",
            "decision",
            "id",
            "memo",
            "org_id",
            "request_id",
            "role_key",
            "step_order",
        ],
    ),
    (
        "org_change_events",
        &[
            "action",
            "actor",
            "created_at",
            "from_status",
            "id",
            "org_id",
            "reason",
            "request_id",
            "to_status",
        ],
    ),
    (
        "org_change_requests",
        &[
            "code",
            "created_at",
            "drafted_by",
            "effective_date",
            "headcount",
            "id",
            "idempotency_key",
            "kind",
            "org_id",
            "preflight",
            "proposal",
            "reason",
            "request_fingerprint",
            "site_count",
            "status",
            "supersedes_id",
            "target_kind",
            "target_label",
            "target_ref",
            "team_count",
            "updated_at",
        ],
    ),
    (
        "org_change_settlement_items",
        &[
            "done",
            "done_at",
            "done_by",
            "id",
            "item_key",
            "memo",
            "org_id",
            "request_id",
        ],
    ),
    (
        "org_runtime_flags",
        &[
            "created_at",
            "enabled",
            "flag_key",
            "id",
            "org_id",
            "rollout_note",
            "set_by",
            "updated_at",
        ],
    ),
    (
        "organizations",
        &[
            "created_at",
            "group_id",
            "id",
            "name",
            "slug",
            "status",
            "updated_at",
        ],
    ),
    (
        "outsource_works",
        &[
            "completed_at",
            "cost_won",
            "created_at",
            "id",
            "org_id",
            "reason",
            "requested_at",
            "result_description",
            "status",
            "updated_at",
            "vendor_id",
            "work_order_id",
        ],
    ),
    (
        "p1_dispatch_alerts",
        &[
            "alert_type",
            "created_at",
            "dispatch_id",
            "failure_reason",
            "id",
            "idempotency_key",
            "lease_expires_at",
            "lease_token",
            "org_id",
            "provider_message_id",
            "recipient_user_id",
            "sent_at",
            "status",
        ],
    ),
    (
        "p1_dispatch_responses",
        &[
            "dispatch_id",
            "distance_meters",
            "gps_ranked",
            "id",
            "org_id",
            "responded_at",
            "response",
            "score_milli",
            "score_reason",
            "user_id",
            "workload_weight",
        ],
    ),
    (
        "p1_dispatch_targets",
        &[
            "dispatch_id",
            "fanout_created_at",
            "id",
            "last_pushed_at",
            "org_id",
            "push_token_count",
            "target_role",
            "user_id",
        ],
    ),
    (
        "payroll_attendance_material_refs",
        &[
            "attendance_record_id",
            "created_at",
            "employee_id",
            "id",
            "org_id",
            "source_digest",
            "source_type",
            "work_date",
        ],
    ),
    (
        "payroll_disbursements",
        &[
            "attested_at",
            "attested_by",
            "created_at",
            "id",
            "org_id",
            "reason",
            "run_id",
            "scheduled_at",
            "status",
            "updated_at",
        ],
    ),
    (
        "payroll_draft_runs",
        &[
            "approval_ref",
            "approved_at",
            "approved_by",
            "calculation_enabled",
            "close_receipt",
            "created_at",
            "created_by",
            "decided_at",
            "decided_by",
            "decision_reason",
            "id",
            "legal_basis",
            "org_id",
            "period_end",
            "period_start",
            "source_label",
            "source_summary",
            "status",
            "submitted_at",
            "submitted_by",
            "updated_at",
        ],
    ),
    (
        "payroll_line_calculations",
        &[
            "created_at",
            "deductions",
            "gross_won",
            "id",
            "line_id",
            "net_won",
            "org_id",
            "payable",
            "run_id",
            "tax_table_version",
            "total_deductions_won",
            "version",
        ],
    ),
    (
        "payroll_payslip_deliveries",
        &[
            "employee_id",
            "id",
            "inbox_doc_id",
            "issued_at",
            "line_id",
            "org_id",
            "run_id",
        ],
    ),
    (
        "period_locks",
        &[
            "domain",
            "id",
            "locked_at",
            "locked_by",
            "org_id",
            "period_end",
            "period_start",
            "reason",
            "unlock_reason",
            "unlocked_at",
            "unlocked_by",
        ],
    ),
    (
        "policy_assignment_preview_receipts",
        &[
            "actor_id",
            "branch_ids",
            "consumed_at",
            "created_at",
            "current_branch_ids",
            "current_role_ids",
            "current_system_roles",
            "expires_at",
            "id",
            "org_id",
            "policy_version",
            "role_ids",
            "system_roles",
            "user_id",
        ],
    ),
    (
        "policy_role_conditions",
        &[
            "attribute",
            "condition_key",
            "condition_values",
            "created_at",
            "id",
            "operator",
            "org_id",
            "role_id",
        ],
    ),
    (
        "policy_role_permissions",
        &[
            "created_at",
            "feature_key",
            "id",
            "org_id",
            "permission_level",
            "role_id",
        ],
    ),
    (
        "policy_roles",
        &[
            "created_at",
            "created_by",
            "description",
            "display_name",
            "id",
            "is_system",
            "org_id",
            "role_key",
            "status",
            "updated_at",
            "updated_by",
        ],
    ),
    ("policy_versions", &["org_id", "updated_at", "version"]),
    (
        "production_capacity_slots",
        &[
            "available_quantity",
            "branch_id",
            "capacity_date",
            "evaluated_at",
            "id",
            "ingested_at",
            "org_id",
            "reserved_quantity",
            "site_id",
            "source_id",
            "source_system",
            "source_version",
            "updated_at",
            "version",
        ],
    ),
    (
        "production_demand_contracts",
        &[
            "due_at",
            "evaluated_at",
            "id",
            "ingested_at",
            "inquiry_id",
            "org_id",
            "product_code",
            "quantity",
            "source_id",
            "source_system",
            "source_version",
        ],
    ),
    (
        "production_idempotency_claims",
        &[
            "completed_at",
            "created_at",
            "idempotency_key",
            "operation",
            "org_id",
            "request_hash",
            "response",
        ],
    ),
    (
        "production_operations",
        &[
            "downtime_minutes",
            "id",
            "org_id",
            "output_quantity",
            "plan_id",
            "quality_evidence_ref",
            "quality_passed",
            "scrap_quantity",
            "sequence",
            "status",
            "version",
        ],
    ),
    (
        "production_plan_events",
        &[
            "actor_id",
            "event_type",
            "id",
            "idempotency_key",
            "occurred_at",
            "org_id",
            "payload",
            "plan_id",
        ],
    ),
    (
        "production_plans",
        &[
            "approval_ref",
            "branch_id",
            "checks",
            "created_at",
            "created_by",
            "customer_demand_id",
            "due_at",
            "first_operation_id",
            "id",
            "idempotency_key",
            "ontology_type_id",
            "org_id",
            "plan_digest",
            "product_code",
            "quantity",
            "released_at",
            "released_by",
            "source_snapshot",
            "status",
            "updated_at",
            "version",
        ],
    ),
    (
        "production_source_ingress_claims",
        &[
            "completed_at",
            "ingested_at",
            "ingested_by",
            "kind",
            "org_id",
            "payload_hash",
            "response",
            "source_id",
            "source_system_id",
            "source_version",
        ],
    ),
    (
        "production_source_systems",
        &[
            "branch_id",
            "credential_generation",
            "credential_hash",
            "credential_state",
            "disabled_at",
            "disabled_by",
            "enabled",
            "id",
            "org_id",
            "principal_id",
            "registered_at",
            "registered_by",
            "rotated_at",
            "rotated_by",
            "source_system",
        ],
    ),
    (
        "recruit_offers",
        &[
            "amount",
            "amount_period",
            "applicant_id",
            "currency",
            "extended_at",
            "extended_by",
            "id",
            "org_id",
            "reply_deadline",
            "resolved_at",
            "status",
            "version",
            "withdraw_reason",
        ],
    ),
    (
        "recruit_postings",
        &[
            "closed_at",
            "company",
            "created_at",
            "created_by",
            "deadline",
            "employment_type",
            "exposure_attested_at",
            "exposure_attested_by",
            "headcount",
            "hired_count",
            "id",
            "org_id",
            "position_ref",
            "posting_no",
            "published_at",
            "published_by",
            "requirements",
            "role_title",
            "scope",
            "status",
            "updated_at",
            "worksite",
        ],
    ),
    (
        "recruit_stage_events",
        &[
            "action",
            "actor",
            "applicant_id",
            "from_stage",
            "id",
            "occurred_at",
            "org_id",
            "reason",
            "to_stage",
        ],
    ),
    (
        "regions",
        &["created_at", "deactivated_at", "id", "name", "org_id"],
    ),
    (
        "registry_equipment_versions",
        &[
            "content",
            "created_at",
            "created_by",
            "id",
            "object_id",
            "org_id",
            "source_version",
            "status",
            "version",
        ],
    ),
    (
        "regular_inspection_schedules",
        &[
            "branch_id",
            "completed_at",
            "completed_by",
            "created_at",
            "created_by",
            "cycle",
            "due_date",
            "equipment_id",
            "id",
            "interval_days",
            "mechanic_id",
            "note",
            "org_id",
            "status",
            "updated_at",
        ],
    ),
    (
        "sales_listing_media",
        &[
            "alt_text",
            "content_type",
            "created_at",
            "id",
            "listing_id",
            "org_id",
            "s3_key",
            "sort_order",
        ],
    ),
    (
        "sales_listings",
        &[
            "availability",
            "badge",
            "capacity_milli",
            "condition",
            "condition_label",
            "created_at",
            "description",
            "equipment_id",
            "id",
            "kind",
            "listing_type",
            "location",
            "model_name",
            "model_year",
            "org_id",
            "price_won",
            "sort_weight",
            "status",
            "updated_at",
            "usage_hours",
            "usage_label",
        ],
    ),
    (
        "series",
        &["code", "created_at", "created_by", "id", "label", "org_id"],
    ),
    (
        "series_instances",
        &[
            "added_at",
            "added_by",
            "id",
            "member_id",
            "member_kind",
            "org_id",
            "series_id",
        ],
    ),
    (
        "service_principal_audit_events",
        &[
            "actor_id",
            "event_type",
            "expected_generation",
            "id",
            "occurred_at",
            "org_id",
            "resulting_generation",
            "service_principal_id",
        ],
    ),
    (
        "service_principal_ingress_claims",
        &[
            "completed_at",
            "ingested_at",
            "kind",
            "org_id",
            "payload_hash",
            "response",
            "service_principal_id",
            "source_id",
            "source_version",
        ],
    ),
    (
        "service_principals",
        &[
            "branch_id",
            "created_at",
            "created_by",
            "disabled_at",
            "disabled_by",
            "display_name",
            "feature",
            "generation",
            "id",
            "org_id",
            "rotated_at",
            "rotated_by",
            "state",
            "verifier",
        ],
    ),
    (
        "site_attendance_events",
        &[
            "branch_id",
            "created_at",
            "id",
            "kind",
            "occurred_at",
            "org_id",
            "site_id",
            "user_id",
            "work_order_id",
        ],
    ),
    (
        "site_geofence_presence",
        &[
            "id",
            "inside",
            "org_id",
            "since",
            "site_id",
            "updated_at",
            "user_id",
            "work_order_id",
        ],
    ),
    (
        "subject_authz_versions",
        &[
            "org_id",
            "session_generation",
            "updated_at",
            "user_id",
            "version",
        ],
    ),
    (
        "support_ticket_acceptances",
        &[
            "accepted_by",
            "channel",
            "created_at",
            "id",
            "idempotency_key",
            "kind",
            "note",
            "occurred_at",
            "org_id",
            "recorded_by_user_id",
            "request_fingerprint",
            "ticket_id",
        ],
    ),
    (
        "target_change_requests",
        &[
            "created_at",
            "id",
            "org_id",
            "reason",
            "requested_by",
            "requested_target_due_at",
            "review_memo",
            "reviewed_at",
            "reviewed_by",
            "status",
            "work_order_id",
        ],
    ),
    ("user_branches", &["branch_id", "org_id", "user_id"]),
    (
        "user_feature_preferences",
        &[
            "created_at",
            "feature_key",
            "org_id",
            "preferences_json",
            "schema_version",
            "updated_at",
            "user_id",
        ],
    ),
    (
        "user_role_assignments",
        &[
            "assigned_by",
            "created_at",
            "id",
            "org_id",
            "role_id",
            "user_id",
        ],
    ),
    (
        "work_order_approval_steps",
        &[
            "approved_at",
            "approved_by_id",
            "approver_id",
            "created_at",
            "decision_comment",
            "id",
            "org_id",
            "requested_at",
            "role",
            "status",
            "step_order",
            "updated_at",
            "work_order_id",
        ],
    ),
    (
        "work_order_assignments",
        &[
            "assigned_at",
            "id",
            "mechanic_id",
            "org_id",
            "role",
            "work_order_id",
        ],
    ),
    (
        "work_order_request_counters",
        &["last_sequence", "org_id", "request_date"],
    ),
    (
        "work_order_settlement_lines",
        &[
            "amount_krw",
            "id",
            "kind",
            "label",
            "org_id",
            "settlement_id",
            "sort_order",
            "source_ref",
        ],
    ),
    (
        "work_order_settlements",
        &[
            "approved_at",
            "approved_by",
            "branch_id",
            "created_at",
            "created_by",
            "id",
            "idempotency_key",
            "note",
            "org_id",
            "request_hash",
            "status",
            "submitted_at",
            "submitted_by",
            "total_amount_krw",
            "updated_at",
            "voucher_ref",
            "work_order_id",
        ],
    ),
    (
        "work_order_status_history",
        &[
            "action",
            "actor",
            "created_at",
            "from_status",
            "id",
            "occurred_at",
            "org_id",
            "to_status",
            "work_order_id",
        ],
    ),
    (
        "work_orders",
        &[
            "action_taken",
            "branch_id",
            "created_at",
            "customer_id",
            "customer_request",
            "delay_note",
            "delay_reason",
            "diagnosis",
            "equipment_id",
            "evidence_verified",
            "id",
            "kpi_excluded",
            "maintenance_cause",
            "maintenance_type",
            "org_id",
            "priority",
            "report_submitted_at",
            "report_submitted_by",
            "request_no",
            "requested_by",
            "result_type",
            "site_id",
            "status",
            "symptom",
            "target_due_at",
            "updated_at",
        ],
    ),
    (
        "workflow_compensating_documents",
        &[
            "compensation_type",
            "created_at",
            "created_by",
            "id",
            "idempotency_key",
            "org_id",
            "original_run_id",
            "payload",
            "reason",
            "status",
        ],
    ),
    (
        "workflow_definition_events",
        &[
            "action",
            "actor_id",
            "after_snap",
            "before_snap",
            "created_at",
            "definition_id",
            "id",
            "org_id",
            "summary",
            "version",
        ],
    ),
    (
        "workflow_definition_versions",
        &[
            "action_allowlist",
            "approval_line",
            "created_at",
            "created_by",
            "definition",
            "definition_id",
            "id",
            "notification_rules",
            "org_id",
            "payment_line",
            "required_approval_line",
            "required_payment_line",
            "status",
            "version",
        ],
    ),
    (
        "workflow_definitions",
        &[
            "active_version",
            "created_at",
            "created_by",
            "display_name",
            "id",
            "latest_version",
            "object_type",
            "org_id",
            "pending_staged_by",
            "pending_version",
            "status",
            "updated_at",
            "updated_by",
            "workflow_key",
        ],
    ),
    (
        "workflow_execution_locks",
        &[
            "acquired_at",
            "acquired_by",
            "expires_at",
            "heartbeat_at",
            "id",
            "lock_key",
            "org_id",
            "run_id",
        ],
    ),
    (
        "workflow_node_runs",
        &[
            "attempt",
            "error_payload",
            "finished_at",
            "id",
            "idempotency_key",
            "input_payload",
            "node_key",
            "node_type",
            "org_id",
            "output_payload",
            "run_id",
            "started_at",
            "status",
            "updated_at",
        ],
    ),
    (
        "workflow_outbox_events",
        &[
            "attempt_count",
            "channel",
            "created_at",
            "dead_lettered_at",
            "delivered_at",
            "destination_ref",
            "error_payload",
            "id",
            "idempotency_key",
            "locked_by",
            "locked_until",
            "next_attempt_at",
            "node_run_id",
            "org_id",
            "payload",
            "run_id",
            "status",
            "updated_at",
        ],
    ),
    (
        "workflow_runs",
        &[
            "completed_at",
            "context_payload",
            "correlation_id",
            "definition_id",
            "definition_version",
            "error_payload",
            "failed_at",
            "id",
            "idempotency_key",
            "initiated_by",
            "input_payload",
            "object_id",
            "object_type",
            "org_id",
            "output_payload",
            "schedule_id",
            "started_at",
            "status",
            "trace_id",
            "trigger_type",
            "updated_at",
        ],
    ),
    (
        "workflow_schedules",
        &[
            "created_at",
            "created_by",
            "cron_expr",
            "definition_id",
            "enabled",
            "id",
            "label",
            "last_run_at",
            "last_status",
            "next_run_at",
            "org_id",
            "timezone",
            "updated_at",
            "updated_by",
        ],
    ),
    (
        "workflow_trigger_bindings",
        &[
            "created_at",
            "created_by",
            "definition_id",
            "enabled",
            "event_key",
            "id",
            "org_id",
            "subject_kind",
            "trigger_type",
            "updated_at",
            "updated_by",
        ],
    ),
    (
        "workflow_waiting_tasks",
        &[
            "assignee_role_key",
            "assignee_user_id",
            "claimed_at",
            "claimed_by",
            "completed_at",
            "completed_by",
            "created_at",
            "decision_payload",
            "due_at",
            "form_payload",
            "id",
            "node_run_id",
            "org_id",
            "passkey_assertion_id",
            "required_policy",
            "run_id",
            "source_object_id",
            "source_object_type",
            "status",
            "title",
            "updated_at",
            "waiting_key",
        ],
    ),
];

/// One column, exactly as `pg_attribute` reports it after the migrations ran.
///
/// The COMMENT is carried, not a `classified` boolean, because deciding
/// "classified" is where the vocabulary has to be applied — a `pd:` prefix over
/// a token nobody defined is not a classification, and a struct holding only a
/// bool has already thrown away what it takes to notice.
#[derive(Debug)]
struct CatalogColumn {
    schema: String,
    table: String,
    column: String,
    comment: Option<String>,
}

impl CatalogColumn {
    fn relation(&self) -> String {
        format!("{}.{}", self.schema, self.table)
    }
}

/// A bare baseline entry means the `public` schema; anything else names its own.
///
/// ponytail: a table whose NAME contains a dot would be misread as qualified.
/// That misreading fails as `names no table`, which is the safe direction, and
/// no such table exists.
fn qualify(entry: &str) -> String {
    if entry.contains('.') {
        entry.to_owned()
    } else {
        format!("public.{entry}")
    }
}

/// Columns carrying NO marker, grouped by relation. The single definition of
/// "unclassified" this file has.
///
/// It exists because there were two. The assertion grouped on `Marker::Absent`
/// and the println beside it filtered on `!= Marker::Valid`, so one output
/// described a `Marker::Invalid` column as unclassified in one number and as
/// neither classified nor unclassified in the next. `Absent` is the one that is
/// right for both: an INVALID marker is its own violation, named by
/// `completeness_violations` and never sheltered by a baseline entry, so folding
/// it into this set would count the same column twice under two different
/// rules. Nothing was weakened — no assertion read the print.
fn unclassified_by_relation(columns: &[CatalogColumn]) -> BTreeMap<String, Vec<&str>> {
    let mut unclassified: BTreeMap<String, Vec<&str>> = BTreeMap::new();
    for column in columns {
        if read_marker(column.comment.as_deref()) == Marker::Absent {
            unclassified
                .entry(column.relation())
                .or_default()
                .push(&column.column);
        }
    }
    unclassified
}

/// The whole control, as a pure function over what the catalog said.
///
/// Pure so that every property below — the stale-baseline pair, the per-column
/// ratchet in both directions, and the vocabulary — can be proved with a
/// hand-written catalog instead of by editing the real baseline and reverting
/// it. An assertion nobody can run is not evidence.
///
/// Three verdicts, and the order they are applied in is the design:
///
/// 1. A marker outside the closed vocabulary is ALWAYS a violation. Declaring a
///    table in the baseline is an admission that nobody has classified it; it
///    is not permission to write `pd:lol`. So this is checked before the
///    baseline is consulted at all, and no entry shelters it.
/// 2. An unclassified column in an undeclared table is a violation, named.
/// 3. An unclassified column NAME the entry does not list is a violation, and so
///    is a listed name that is no longer unclassified — up because a new column
///    landed unclassified, down because a gain not written down is a slot the
///    next one takes for free. Set membership, not a count: classifying one
///    column does not pay for adding another, so no spelling of the DDL that
///    added it matters here.
fn completeness_violations(columns: &[CatalogColumn], baseline: &[(&str, &[&str])]) -> Vec<String> {
    let mut relations: BTreeSet<String> = BTreeSet::new();
    let mut violations = Vec::new();
    let unclassified = unclassified_by_relation(columns);
    for column in columns {
        relations.insert(column.relation());
        match read_marker(column.comment.as_deref()) {
            Marker::Valid | Marker::Absent => {}
            Marker::Invalid(token) => violations.push(format!(
                "{}.{}: marker token '{token}' is outside the closed vocabulary — a `pd:` \
                 prefix is not a classification. The baseline shelters a MISSING marker, \
                 never a wrong one.",
                column.relation(),
                column.column,
            )),
        }
    }
    let baselined: BTreeMap<String, BTreeSet<&str>> = baseline
        .iter()
        .map(|(entry, names)| (qualify(entry), names.iter().copied().collect()))
        .collect();
    // A repeated table key would let the LATER entry overwrite the earlier one
    // silently, and the union of two entries is a WIDER pin than either — which
    // is the one mistake 3,600 lines of machine-pasted data makes plausibly and
    // this structure absorbs without a word. The only other trace is a println,
    // and nothing reads a println.
    assert_eq!(
        baselined.len(),
        baseline.len(),
        "duplicate table entry in UNCLASSIFIED_TABLE_BASELINE"
    );

    for (relation, columns) in &unclassified {
        let Some(declared) = baselined.get(relation) else {
            violations.push(format!(
                "{relation}: {} column(s) carry no pd: marker [{}] — classify each with \
                 COMMENT ON COLUMN … IS 'pd:…', or declare the table in \
                 UNCLASSIFIED_TABLE_BASELINE",
                columns.len(),
                columns.join(", ")
            ));
            continue;
        };
        let live: BTreeSet<&str> = columns.iter().copied().collect();

        let landed: Vec<&str> = live.difference(declared).copied().collect();
        if !landed.is_empty() {
            violations.push(format!(
                "{relation}: {} column(s) LANDED UNCLASSIFIED [{}] — classify each with \
                 COMMENT ON COLUMN … IS 'pd:…'. The baseline is a SET of names, not a count, \
                 so classifying some other column in the same change does not pay for these.",
                landed.len(),
                landed.join(", ")
            ));
        }

        let gone: Vec<&str> = declared.difference(&live).copied().collect();
        if !gone.is_empty() {
            violations.push(format!(
                "{relation}: {} baseline name(s) are no longer unclassified [{}] — each was \
                 classified or dropped. Remove them from UNCLASSIFIED_TABLE_BASELINE in this \
                 same change, or the entry is a slot the next unclassified column takes \
                 silently.",
                gone.len(),
                gone.join(", ")
            ));
        }
    }
    for entry in baselined.keys() {
        if !relations.contains(entry) {
            violations.push(format!(
                "baseline entry `{entry}` names no table in the migrated database — remove it"
            ));
        } else if !unclassified.contains_key(entry) {
            violations.push(format!(
                "baseline entry `{entry}` is fully classified now — remove it"
            ));
        }
    }
    violations
}

/// Read every application column out of the catalog.
///
/// `attnum > 0` drops the system columns; `attisdropped` drops the tombstones a
/// `DROP COLUMN` leaves in `pg_attribute`.
///
/// THE `pg_` PREFIX PREDICATE IS GONE, AND ITS JUSTIFICATION WAS FALSE. This
/// filtered `n.nspname NOT LIKE 'pg\_%'`, and the sentence that stood here said
/// the prefix is reserved so no application schema can hide behind it —
/// `CREATE SCHEMA pg_x` refused even to a superuser. The refusal is real but
/// liftable: `allow_system_table_mods` is a `superuser`-context GUC, and with it
/// on, measured against the pinned `postgres:18.4`, `CREATE SCHEMA pg_evil`
/// succeeds where it otherwise raises `42939 unacceptable schema name`. A
/// migration doing `PERFORM set_config('allow_system_table_mods','on',true)`
/// then a concat-split `CREATE SCHEMA pg_evil` / `CREATE TABLE pg_evil.pd_leak
/// (rrn TEXT, employee_name TEXT)` / `INSERT` of a 주민등록번호 gave gate EXIT=0
/// AND catalog EXIT=0, printing counts byte-identical to a clean tree.
///
/// So the sweep now names what it drops instead of matching a prefix, and the
/// two predicates it takes are the measured ones rather than the obvious ones:
///
/// * `n.nspname <> 'pg_catalog'` — the 64 system catalogs. This one is a NAME,
///   and it is the floor: see the boundary paragraph in the module doc.
/// * `c.relpersistence <> 't'` — session-temporary relations, tagged by
///   PostgreSQL itself rather than recognised by the shape of a schema name.
///
/// `pg_toast`, `pg_temp_N` and `pg_toast_temp_N` are NOT excluded, and dropping
/// them from the exclusion made the sweep stricter for zero baseline entries.
/// The enumeration those names stand in for was measured on the pinned image,
/// not assumed:
///
/// * `pg_toast` on a clean server holds `relkind` `t` (43) and `i` (43) and
///   nothing else, so the `relkind` predicate already drops all of it — while
///   `CREATE TABLE pg_toast.pd_leak (rrn TEXT)` under the same lifted GUC
///   SUCCEEDS and lands a `relkind = 'r'`. Naming `pg_toast` in the exclusion
///   would therefore have bought nothing and cost exactly the hiding place the
///   `pg_evil` plant just demonstrated. Measured both ways: an exclusion reading
///   `NOT IN ('pg_catalog','pg_toast')` plus two temp `LIKE`s sees
///   `pg_evil.pd_leak` and misses `pg_toast.pd_leak`; the two predicates above
///   see both.
/// * `pg_temp_N` and `pg_toast_temp_N` need no name predicate because a second
///   refusal stands there that the GUC does NOT lift: `CREATE TABLE
///   pg_temp_99.pd_leak` raises `42P16 cannot create relations in temporary
///   schemas of other sessions` with `allow_system_table_mods` on. What can live
///   under those names is a genuine temp relation, which `relpersistence = 't'`
///   drops by its catalog tag.
///
/// THE `relkind` SET, AUDITED RATHER THAN INHERITED. `relkind` takes `r`, `p`,
/// `m`, `f`. The full domain on PostgreSQL 18.4 is `r i S t v m c f p I` — ten,
/// not the nine usually listed; `I` is a partitioned index. Each excluded kind
/// was built on the pinned image and probed for whether it can hold application
/// row data that a column comment could classify:
///
/// * `i`, `I` — `COMMENT ON COLUMN` is REFUSED, `42809 cannot set comment on
///   relation`. No marker can be attached, so nothing here is expressible.
///   Separately, every index in `pg_class` has a `pg_index` row naming its base
///   table (0 without one), and that table is swept; an expression index over
///   `(rrn || name)` stores a projection of columns this sweep already reads.
/// * `S` — `COMMENT ON COLUMN` REFUSED `42809`, and `ALTER TABLE … ADD COLUMN`
///   on a sequence REFUSED `42809`, so its shape is fixed at `last_value`,
///   `log_cnt`, `is_called`. Worth naming because it is not vacuous:
///   `setval('s', 9001011234567)` is accepted, so a sequence CAN carry
///   RRN-shaped digits. It is outside this control not because it cannot hold
///   them but because no column comment can classify them — see the boundary.
/// * `t` — TOAST. Fixed system columns `chunk_id`, `chunk_seq`, `chunk_data`,
///   and 0 TOAST tables exist without a base table pointing at them via
///   `reltoastrelid`. Its content is a swept table's own out-of-line values.
/// * `v` — `relfilenode = 0`, `pg_relation_size = 0`, `INSERT` refused `55000`.
///   It stores no row. `CREATE VIEW v AS SELECT '900101-…'::text AS rrn` does
///   put a 주민등록번호 in the catalog, but in the rewrite rule, not in a heap;
///   a view over base relations reads relations this sweep already has.
/// * `c` — composite type. `relfilenode = 0`; `INSERT` and `SELECT` both refused
///   `42809 cannot open relation`. It is a type definition, not storage.
///
/// And the four that are IN, for the same measurement: `r` and `m` have a real
/// `relfilenode` and non-zero size; `p` has `relfilenode = 0` and 0 bytes and
/// stores its rows in `r` leaves, which are swept in their own right (a leaf
/// does NOT inherit the parent's column comments — measured, and the reason
/// runtime partitions stay on the residual list); `f` has no local storage but
/// exposes remote rows and accepts a `COMMENT ON COLUMN`.
///
/// So no `relkind` outside the four can hold application row data that a column
/// comment could classify, and the `WHERE` clause has no predicate left that is
/// justified by something other than a measurement.
///
/// `information_schema` IS SWEPT, AND USED NOT TO BE. The query carried a third
/// predicate, `n.nspname <> 'information_schema'`, that this comment never
/// explained and the four scope statements never listed. It was a hole with a
/// name: as the initdb superuser — which is exactly who migrates in the pgtest
/// and CI containers — `CREATE TABLE information_schema.pd_leak (rrn TEXT,
/// employee_name TEXT)` produced two live `attnum > 0` columns that this sweep
/// did not read, so the catalog counts came out byte-identical to a clean tree.
/// The predicate is gone. What PostgreSQL 18.4 actually ships there under
/// `relkind IN ('r','p','m','f')` is four tables — `sql_features`,
/// `sql_implementation_info`, `sql_parts`, `sql_sizing`, 21 columns, no
/// `COMMENT` on any of them — and they are declared in
/// `UNCLASSIFIED_TABLE_BASELINE` like any other unclassified table. Everything
/// else in `information_schema` is `relkind = 'v'` and was already out. A
/// version that ships a different set of real tables there fails the two-sided
/// check by name rather than passing quietly, which is the direction to fail in.
///
/// EVERY APPLICATION SCHEMA, AND THAT IS LOAD-BEARING. The whole reason the
/// completeness question moved to the catalog was a planted
/// `shadow.employees(raw_row)` the text gate reported PASSED on. The
/// `personal_data_columns()` reader now takes these exact namespace,
/// persistence, relkind, live-attribute, and marker predicates, returning the
/// schema-qualified identity that `strip_all_markers` needs. That alignment is
/// executable: the retention tests plant a sensitive marker in a non-public
/// ordinary table, a materialized view, and a foreign table, and each must
/// derive the two-year floor. This query returns the raw comment so the caller
/// applies the closed vocabulary over that same relation universe.
async fn application_columns(pool: &PgPool) -> Vec<CatalogColumn> {
    sqlx::query(
        r"
        SELECT n.nspname::TEXT AS schema_name,
               c.relname::TEXT AS table_name,
               a.attname::TEXT AS column_name,
               d.description::TEXT AS column_comment
        FROM pg_catalog.pg_class AS c
        JOIN pg_catalog.pg_namespace AS n ON n.oid = c.relnamespace
        JOIN pg_catalog.pg_attribute AS a ON a.attrelid = c.oid
        LEFT JOIN pg_catalog.pg_description AS d
               ON d.objoid = c.oid AND d.objsubid = a.attnum
        WHERE n.nspname <> 'pg_catalog'
          AND c.relpersistence <> 't'
          AND c.relkind IN ('r', 'p', 'm', 'f')
          AND a.attnum > 0
          AND NOT a.attisdropped
        ORDER BY 1, 2, 3
        ",
    )
    .fetch_all(pool)
    .await
    .unwrap()
    .into_iter()
    .map(|row| CatalogColumn {
        schema: row.get("schema_name"),
        table: row.get("table_name"),
        column: row.get("column_name"),
        comment: row.get("column_comment"),
    })
    .collect()
}

/// No column of any application table escapes classification without being
/// declared, and no declared table quietly grows another unclassified column.
#[sqlx::test(migrations = "./migrations")]
async fn every_application_column_is_classified_or_its_table_is_declared(pool: PgPool) {
    let columns = application_columns(&pool).await;

    let relations: BTreeSet<String> = columns.iter().map(CatalogColumn::relation).collect();
    let classified = columns
        .iter()
        .filter(|c| read_marker(c.comment.as_deref()) == Marker::Valid)
        .count();
    let unclassified_relations = unclassified_by_relation(&columns);
    let named: usize = UNCLASSIFIED_TABLE_BASELINE
        .iter()
        .map(|(_, names)| names.len())
        .sum();
    println!(
        "catalog completeness: {} columns across {} tables; {classified} classified, {} not; \
         {} tables fully classified, {} declared in the baseline ({} entries naming {named} \
         unclassified columns)",
        columns.len(),
        relations.len(),
        columns.len() - classified,
        relations.len() - unclassified_relations.len(),
        unclassified_relations.len(),
        UNCLASSIFIED_TABLE_BASELINE.len(),
    );

    assert!(
        columns.len() > 1000,
        "the catalog reported only {} columns — the migrations did not run, and an assertion \
         over an empty schema passes while proving nothing",
        columns.len()
    );

    let violations = completeness_violations(&columns, UNCLASSIFIED_TABLE_BASELINE);
    assert!(
        violations.is_empty(),
        "personal-data classification is incomplete ({} violation(s)):\n{}",
        violations.len(),
        violations.join("\n")
    );
}

/// THE GENERATOR. `UNCLASSIFIED_TABLE_BASELINE` is 2,621 column names; they are
/// read out of the live catalog and pasted, never typed. A hand-written second
/// inventory would be free to disagree with the catalog about what a column is,
/// which is the failure mode this whole file exists to remove.
///
/// From the repository root, with the disposable container:
///
/// ```text
/// tools/lanes/pgtest.sh "$PWD" cargo test -p console-platform-db \
///   --test personal_data_classification -- --ignored --nocapture --exact \
///   print_unclassified_baseline
/// ```
///
/// Paste its stdout over the body of the constant. `#[sqlx::test]` embeds the
/// migration set at COMPILE time, so `touch` this file first or a stale binary
/// will print a stale baseline in a fraction of a second.
#[ignore = "generator, not an assertion"]
#[sqlx::test(migrations = "./migrations")]
async fn print_unclassified_baseline(pool: PgPool) {
    let columns = application_columns(&pool).await;
    for (relation, names) in unclassified_by_relation(&columns) {
        let entry = relation.strip_prefix("public.").unwrap_or(&relation);
        let names: Vec<String> = names.iter().map(|n| format!("\"{n}\"")).collect();
        println!("    (\"{entry}\", &[{}]),", names.join(", "));
    }
}

/// One catalog row, spelled short so the properties below are readable.
fn col(schema: &str, table: &str, column: &str, comment: Option<&str>) -> CatalogColumn {
    CatalogColumn {
        schema: schema.into(),
        table: table.into(),
        column: column.into(),
        comment: comment.map(str::to_owned),
    }
}

const RRN: Option<&str> = Some("pd:unique-id/rrn — 주민등록번호");

/// A baseline entry for a table that is now fully classified must fail.
///
/// This is one of the two halves that make the list shrink-only, so it is
/// proved rather than asserted in prose.
#[test]
fn stale_baseline_entry_for_a_now_classified_table_fails() {
    let columns = vec![col("public", "employees", "raw_row", RRN)];
    let violations = completeness_violations(&columns, &[("employees", &["raw_row"])]);
    assert_eq!(violations.len(), 1, "{violations:?}");
    assert!(
        violations[0].contains("public.employees") && violations[0].contains("fully classified"),
        "{violations:?}"
    );
}

/// A baseline entry for a table that does not exist must fail — the other half.
#[test]
fn stale_baseline_entry_for_a_table_that_does_not_exist_fails() {
    let columns = vec![col("public", "employees", "raw_row", RRN)];
    let violations = completeness_violations(
        &columns,
        &[
            ("employees", &["raw_row"]),
            ("table_removed_last_year", &["a", "b", "c"]),
        ],
    );
    assert!(
        violations
            .iter()
            .any(|v| v.contains("public.table_removed_last_year") && v.contains("names no table")),
        "{violations:?}"
    );
}

/// An unclassified column in a table nobody declared must fail, and the failure
/// must name the column. Without this the assertion above could pass by being
/// unable to see anything at all.
#[test]
fn an_undeclared_table_with_an_unclassified_column_fails() {
    let columns = vec![
        col("shadow", "Employees", "RRN", None),
        col("public", "employees", "raw_row", RRN),
    ];
    let violations = completeness_violations(&columns, &[]);
    assert_eq!(violations.len(), 1, "{violations:?}");
    assert!(
        violations[0].contains("shadow.Employees") && violations[0].contains("RRN"),
        "{violations:?}"
    );
}

/// A table named twice in the baseline must abort, not silently widen the pin.
///
/// `baselined` is a `BTreeMap`, so a repeated key keeps the LATER entry and
/// drops the earlier one without a word. The constant is 3,600 lines of
/// machine-pasted data; a paste artifact leaving one table declared twice is the
/// plausible mistake, and the union of the two entries pins MORE names than
/// either — the second entry below declares `medical_certificate_no` and would
/// have landed it green. The only other trace was a println, and no assertion
/// reads a println.
#[test]
#[should_panic(expected = "duplicate table entry in UNCLASSIFIED_TABLE_BASELINE")]
fn a_table_declared_twice_in_the_baseline_fails() {
    let columns = vec![col("public", "leave_requests", "reason", None)];
    completeness_violations(
        &columns,
        &[
            ("leave_requests", &["reason"]),
            ("leave_requests", &["reason", "medical_certificate_no"]),
        ],
    );
}

// ---------------------------------------------------------------------------
// THE PER-COLUMN RATCHET, proved in every direction that has been exploited.
//
// Before any per-column baseline, 243 of 282 tables were sheltered whole: a new
// personal-data column on any of them landed unclassified and every assertion
// in this file still passed. The first fix pinned a per-table COUNT, and the
// count was payable — the third test below is the exact trade that paid it, and
// is why the pin is a SET of names now.
// ---------------------------------------------------------------------------

/// THE 86% GAP, CLOSED. A NEW unclassified column on a BASELINED table fails.
///
/// This is the case the table-granular baseline could not see. Nothing in the
/// catalog says `medical_certificate_no` is newer than `reason`; the set gained
/// a name, and that is enough.
#[test]
fn a_new_unclassified_column_on_a_baselined_table_fails() {
    let columns = vec![
        col("public", "leave_requests", "reason", None),
        col("public", "leave_requests", "medical_certificate_no", None),
    ];
    let violations = completeness_violations(&columns, &[("leave_requests", &["reason"])]);
    assert_eq!(violations.len(), 1, "{violations:?}");
    assert!(
        violations[0].contains("public.leave_requests")
            && violations[0].contains("LANDED UNCLASSIFIED")
            && violations[0].contains("medical_certificate_no"),
        "{violations:?}"
    );
}

/// A baseline name that is no longer unclassified also fails: a gain must be
/// written down in the change that earns it, or the freed slot silently absorbs
/// the next one.
#[test]
fn a_baseline_name_that_is_no_longer_unclassified_fails() {
    let columns = vec![
        col("public", "leave_requests", "reason", RRN),
        col("public", "leave_requests", "note", None),
    ];
    let violations = completeness_violations(&columns, &[("leave_requests", &["note", "reason"])]);
    assert_eq!(violations.len(), 1, "{violations:?}");
    assert!(
        violations[0].contains("no longer unclassified")
            && violations[0].contains("reason")
            && violations[0].contains("same change"),
        "{violations:?}"
    );
}

/// THE TRADE THAT BEAT THE COUNT, AND MUST NOT BEAT THE SET.
///
/// One column classified, one added unclassified, in one change. The net count
/// is unmoved — `2 -> 2` — so the pin this baseline replaced saw nothing, and
/// six rounds of parser hardening were each defeated by exactly this shape
/// wrapped in one more spelling. Membership MOVED, so both halves are named.
#[test]
fn classifying_one_column_does_not_pay_for_a_new_unclassified_one() {
    let columns = vec![
        // Was unclassified, is classified now.
        col("public", "leave_requests", "reason", RRN),
        col("public", "leave_requests", "note", None),
        // Arrived unclassified in the same change.
        col("public", "leave_requests", "medical_certificate_no", None),
    ];
    let violations = completeness_violations(&columns, &[("leave_requests", &["note", "reason"])]);
    assert_eq!(violations.len(), 2, "{violations:?}");
    assert!(
        violations
            .iter()
            .any(|v| v.contains("LANDED UNCLASSIFIED") && v.contains("medical_certificate_no")),
        "{violations:?}"
    );
    assert!(
        violations
            .iter()
            .any(|v| v.contains("no longer unclassified") && v.contains("reason")),
        "{violations:?}"
    );
}

/// THE FALSE POSITIVE THAT WOULD MAKE THIS UNUSABLE. A column added AND
/// classified in the same change touches the unclassified set not at all, so it
/// must be silent. Without this, every new column would have to be declared
/// unclassified first and the control would be routed around within a week.
#[test]
fn a_new_column_classified_in_the_same_change_passes() {
    let columns = vec![
        col("public", "leave_requests", "reason", None),
        col("public", "leave_requests", "medical_certificate_no", RRN),
    ];
    assert!(
        completeness_violations(&columns, &[("leave_requests", &["reason"])]).is_empty(),
        "classifying a new column at birth must be silent"
    );
}

/// A baseline set that matches passes, so the failures above are about
/// membership and not about the entry existing at all.
#[test]
fn a_baseline_set_matching_the_catalog_passes() {
    let columns = vec![
        col("public", "leave_requests", "reason", None),
        col("public", "leave_requests", "approved_by", RRN),
    ];
    assert!(
        completeness_violations(&columns, &[("leave_requests", &["reason"])]).is_empty(),
        "a matching set must be silent"
    );
}

// ---------------------------------------------------------------------------
// THE VOCABULARY, applied where `classified` is decided.
// ---------------------------------------------------------------------------

/// `pd:lol` is not a classification, and being on a DECLARED table does not make
/// it one. Both halves matter: the count is unmoved, so only the vocabulary
/// check can catch this.
#[test]
fn a_marker_outside_the_vocabulary_fails_even_on_a_baselined_table() {
    let columns = vec![col("public", "leave_requests", "reason", Some("pd:lol"))];
    let violations = completeness_violations(&columns, &[("leave_requests", &["reason"])]);
    assert!(
        violations
            .iter()
            .any(|v| v.contains("public.leave_requests.reason")
                && v.contains("'lol'")
                && v.contains("outside the closed vocabulary")),
        "{violations:?}"
    );
}

/// THE HOLE F2 NAMED. A bogus token in a NON-PUBLIC schema must fail.
///
/// The old vocabulary check read `personal_data_columns()`, scoped
/// `nspname = 'public'`, so this exact row was checked by nothing — in the one
/// schema shape the catalog assertion was built to catch.
#[test]
fn a_bogus_token_in_a_non_public_schema_fails() {
    let columns = vec![col(
        "shadow",
        "employees",
        "rrn",
        Some("pd:unique-id/social-security — not one of the four"),
    )];
    let violations = completeness_violations(&columns, &[("shadow.employees", &["rrn"])]);
    assert!(
        violations
            .iter()
            .any(|v| v.contains("shadow.employees.rrn") && v.contains("unique-id/social-security")),
        "{violations:?}"
    );
}

/// The vocabulary reader itself, at its edges. Each of these was a way for a
/// string beginning `pd:` to count as a classification.
#[test]
fn read_marker_accepts_only_the_closed_vocabulary() {
    assert_eq!(read_marker(None), Marker::Absent);
    assert_eq!(read_marker(Some("the employee's surname")), Marker::Absent);

    assert_eq!(read_marker(Some("pd:none")), Marker::Valid);
    assert_eq!(read_marker(Some("pd:personal — 이름")), Marker::Valid);
    assert_eq!(
        read_marker(Some("pd:unique-id/rrn,sensitive/health — 원본 행")),
        Marker::Valid
    );

    // A class that must name WHICH, given bare.
    assert!(matches!(
        read_marker(Some("pd:sensitive")),
        Marker::Invalid(_)
    ));
    assert!(matches!(
        read_marker(Some("pd:unique-id")),
        Marker::Invalid(_)
    ));
    // A class that takes no sub-token, given one.
    assert!(matches!(
        read_marker(Some("pd:personal/name")),
        Marker::Invalid(_)
    ));
    // Invented tokens, bare and in a list beside a good one.
    assert!(matches!(read_marker(Some("pd:lol")), Marker::Invalid(_)));
    assert!(matches!(
        read_marker(Some("pd:none,lol")),
        Marker::Invalid(_)
    ));
    // Empty, both spellings — `pd:` alone and `pd: ` with the list after the
    // whitespace, which the SQL regex also reads as empty.
    assert!(matches!(read_marker(Some("pd:")), Marker::Invalid(_)));
    assert!(matches!(read_marker(Some("pd: none")), Marker::Invalid(_)));
}
