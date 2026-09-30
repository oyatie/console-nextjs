//! The gate is only worth its CI minute if it still REJECTS.
//!
//! `cargo run -p console-gate-personal-data-classification` against this tree
//! exits 0, and so does a gate whose scan root stopped resolving — "green" and
//! "scanned nothing" are indistinguishable from the outside. These tests plant
//! each violation in a throwaway tree and assert the rejection, so a de-fanged
//! gate fails the job instead of passing it quietly.

use console_gate_personal_data_classification::{ViolationKind, check_tree};
use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

static COUNTER: AtomicUsize = AtomicUsize::new(0);

/// A throwaway tree holding one migration file.
fn tree(name: &str, sql: &str) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "console-pd-classification-{name}-{}-{unique}",
        std::process::id()
    ));
    if dir.exists() {
        fs::remove_dir_all(&dir)?;
    }
    let migrations = dir.join("migrations");
    fs::create_dir_all(&migrations)?;
    fs::write(migrations.join("0001_test.sql"), sql)?;
    Ok(dir)
}

fn empty_baseline() -> BTreeSet<String> {
    BTreeSet::new()
}

fn baseline(tables: &[&str]) -> BTreeSet<String> {
    tables.iter().map(|t| (*t).to_owned()).collect()
}

/// The headline behaviour: a column nobody classified fails the gate.
#[test]
fn gate_rejects_an_unclassified_column() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tree(
        "unclassified",
        "CREATE TABLE staff (id UUID PRIMARY KEY, name TEXT NOT NULL);
         COMMENT ON COLUMN staff.id IS 'pd:personal — surrogate key of a person row';",
    )?;
    let result = check_tree(&dir, &empty_baseline())?;
    assert!(!result.passed(), "expected the unclassified column to fail");
    assert!(
        result
            .violations
            .iter()
            .any(|v| v.kind == ViolationKind::UnclassifiedColumn
                && v.detail.contains("staff.name")),
        "expected UnclassifiedColumn for staff.name, got {:#?}",
        result.violations
    );
    Ok(())
}

/// The same tree passes once the column is classified — the gate is satisfiable
/// by classifying, not only by deleting the column.
#[test]
fn gate_accepts_a_fully_classified_table() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tree(
        "classified",
        "CREATE TABLE staff (id UUID PRIMARY KEY, name TEXT NOT NULL);
         COMMENT ON COLUMN staff.id IS 'pd:personal — surrogate key of a person row';
         COMMENT ON COLUMN staff.name IS 'pd:personal — direct identifier';",
    )?;
    let result = check_tree(&dir, &empty_baseline())?;
    assert!(
        result.passed(),
        "expected a fully classified table to pass, got {:#?}",
        result.violations
    );
    assert_eq!(result.classified_columns, 2);
    Ok(())
}

/// A column added by a LATER migration must be classified too. This is the
/// drift the baseline cannot absorb: the table is already off the baseline.
#[test]
fn gate_rejects_a_column_added_by_a_later_migration() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tree(
        "later-add",
        "CREATE TABLE staff (id UUID PRIMARY KEY);
         COMMENT ON COLUMN staff.id IS 'pd:personal — surrogate key of a person row';",
    )?;
    fs::write(
        dir.join("migrations").join("0002_add.sql"),
        "ALTER TABLE staff ADD COLUMN rrn TEXT, ADD COLUMN note TEXT;",
    )?;
    let result = check_tree(&dir, &empty_baseline())?;
    let unclassified: Vec<_> = result
        .violations
        .iter()
        .filter(|v| v.kind == ViolationKind::UnclassifiedColumn)
        .collect();
    assert_eq!(
        unclassified.len(),
        2,
        "the multi-ADD COLUMN form must yield BOTH columns, got {unclassified:#?}"
    );
    Ok(())
}

/// The vocabulary is closed. An invented token is not a classification.
#[test]
fn gate_rejects_a_token_outside_the_closed_vocabulary() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tree(
        "unknown-token",
        "CREATE TABLE staff (id UUID PRIMARY KEY);
         COMMENT ON COLUMN staff.id IS 'pd:top-secret — invented';",
    )?;
    let result = check_tree(&dir, &empty_baseline())?;
    assert!(
        result
            .violations
            .iter()
            .any(|v| v.kind == ViolationKind::UnknownToken),
        "expected UnknownToken, got {:#?}",
        result.violations
    );
    Ok(())
}

/// 개인정보 보호법 시행령 제19조 is a closed set of four, and 고시 제2026-9호
/// 제7조제3항제2호 makes 주민등록번호 unconditional while the other three may be
/// scoped. A bare `unique-id` erases that, so the gate refuses it.
#[test]
fn gate_rejects_unique_id_without_its_subtoken() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tree(
        "bare-unique-id",
        "CREATE TABLE staff (id UUID PRIMARY KEY);
         COMMENT ON COLUMN staff.id IS 'pd:unique-id — which of the four?';",
    )?;
    let result = check_tree(&dir, &empty_baseline())?;
    assert!(
        result
            .violations
            .iter()
            .any(|v| v.kind == ViolationKind::BadSubtoken),
        "expected BadSubtoken, got {:#?}",
        result.violations
    );
    Ok(())
}

/// A sub-token outside its class's list is not a classification either.
#[test]
fn gate_rejects_an_unknown_subtoken() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tree(
        "bad-subtoken",
        "CREATE TABLE staff (id UUID PRIMARY KEY);
         COMMENT ON COLUMN staff.id IS 'pd:unique-id/social-security — not a Korean identifier';",
    )?;
    let result = check_tree(&dir, &empty_baseline())?;
    assert!(
        result
            .violations
            .iter()
            .any(|v| v.kind == ViolationKind::BadSubtoken),
        "expected BadSubtoken, got {:#?}",
        result.violations
    );
    Ok(())
}

/// Defence in depth over Postgres's own migration-time error: a marker naming a
/// column that does not exist is caught here too, because `DROP COLUMN`
/// silently discards a comment and a drop-then-recreate would otherwise leave
/// the classification pointing at nothing.
#[test]
fn gate_rejects_a_marker_for_a_column_that_does_not_exist() -> Result<(), Box<dyn std::error::Error>>
{
    let dir = tree(
        "ghost-column",
        "CREATE TABLE staff (id UUID PRIMARY KEY);
         COMMENT ON COLUMN staff.id IS 'pd:personal — surrogate key of a person row';
         COMMENT ON COLUMN staff.vanished IS 'pd:sensitive/health — column was dropped';",
    )?;
    let result = check_tree(&dir, &empty_baseline())?;
    assert!(
        result
            .violations
            .iter()
            .any(|v| v.kind == ViolationKind::MarkerForUnknownColumn),
        "expected MarkerForUnknownColumn, got {:#?}",
        result.violations
    );
    Ok(())
}

/// The baseline is a ratchet. Once a listed table is fully classified the entry
/// must go, or the list would silently re-acquire slack for the next column.
#[test]
fn gate_rejects_a_baseline_entry_that_is_now_fully_classified()
-> Result<(), Box<dyn std::error::Error>> {
    let dir = tree(
        "ratchet",
        "CREATE TABLE staff (id UUID PRIMARY KEY);
         COMMENT ON COLUMN staff.id IS 'pd:personal — surrogate key of a person row';",
    )?;
    let result = check_tree(&dir, &baseline(&["staff"]))?;
    assert!(
        result
            .violations
            .iter()
            .any(|v| v.kind == ViolationKind::BaselineEntryFullyClassified),
        "expected BaselineEntryFullyClassified, got {:#?}",
        result.violations
    );
    Ok(())
}

/// The baseline may not rot into a list of tables nobody can find.
#[test]
fn gate_rejects_a_baseline_entry_for_a_table_that_no_longer_exists()
-> Result<(), Box<dyn std::error::Error>> {
    let dir = tree(
        "stale-baseline",
        "CREATE TABLE staff (id UUID PRIMARY KEY);
         COMMENT ON COLUMN staff.id IS 'pd:personal — surrogate key of a person row';",
    )?;
    let result = check_tree(&dir, &baseline(&["long_gone"]))?;
    assert!(
        result
            .violations
            .iter()
            .any(|v| v.kind == ViolationKind::BaselineEntryUnknownTable),
        "expected BaselineEntryUnknownTable, got {:#?}",
        result.violations
    );
    Ok(())
}

/// A baselined table's unclassified columns are debt, not a failure — otherwise
/// the gate could not land against an unclassified codebase at all.
#[test]
fn gate_tolerates_unclassified_columns_in_a_baselined_table()
-> Result<(), Box<dyn std::error::Error>> {
    let dir = tree(
        "baselined",
        "CREATE TABLE staff (id UUID PRIMARY KEY, name TEXT);",
    )?;
    let result = check_tree(&dir, &baseline(&["staff"]))?;
    assert!(
        result.passed(),
        "expected a baselined table to be tolerated, got {:#?}",
        result.violations
    );
    Ok(())
}

/// A dropped column is no longer classifiable and must not be demanded.
#[test]
fn gate_ignores_a_dropped_column() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tree(
        "dropped",
        "CREATE TABLE staff (id UUID PRIMARY KEY, legacy TEXT);
         COMMENT ON COLUMN staff.id IS 'pd:personal — surrogate key of a person row';
         ALTER TABLE staff DROP COLUMN legacy;",
    )?;
    let result = check_tree(&dir, &empty_baseline())?;
    assert!(
        result.passed(),
        "expected the dropped column to be ignored, got {:#?}",
        result.violations
    );
    Ok(())
}

/// THE FAIL-OPEN CLASS, which is the one that matters.
///
/// Every construct below used to make the parser return early WITHOUT
/// registering the table. A table that is never registered has no columns to
/// check, and a table with no columns to check passes — so the gate reported
/// green over relations it had not read. `CREATE TABLE … AS SELECT` and
/// `SELECT … INTO` were invisible; `(LIKE parent INCLUDING ALL)` was skipped as
/// if `LIKE` were a table constraint, discarding every inherited column.
///
/// For a gate whose entire purpose is "no personal-data column goes
/// unclassified", unparseable must mean FAIL. Each of these now names the
/// construct that defeated the parser.
#[test]
fn gate_rejects_ddl_it_cannot_parse_instead_of_passing_it() -> Result<(), Box<dyn std::error::Error>>
{
    // (case name, SQL, substring the finding must name)
    let cases: &[(&str, &str, &str)] = &[
        (
            "create-table-as-select",
            "CREATE TABLE staff_copy AS SELECT * FROM staff;",
            "AS SELECT",
        ),
        (
            "select-into",
            "SELECT * INTO staff_copy FROM staff;",
            "SELECT … INTO",
        ),
        (
            "create-table-like",
            "CREATE TABLE staff_copy (LIKE staff INCLUDING ALL);",
            "(LIKE … )",
        ),
        (
            "partition-of",
            "CREATE TABLE staff_2026 PARTITION OF staff FOR VALUES FROM ('2026-01-01') TO ('2027-01-01');",
            "PARTITION OF",
        ),
        (
            "inherits",
            "CREATE TABLE contractor (day_rate NUMERIC) INHERITS (staff);",
            "INHERITS",
        ),
        (
            "syntax-error",
            "CREATE TABLE staff_copy (id UUID PRIMARY KEY, name TEXT;",
            "balanced",
        ),
    ];

    for (name, sql, expected) in cases {
        let dir = tree(name, sql)?;
        let result = check_tree(&dir, &empty_baseline())?;
        assert!(
            !result.passed(),
            "'{name}' must FAIL the gate: a construct the parser cannot read is a table \
             whose columns cannot be proved classified. Got a pass."
        );
        assert!(
            result
                .violations
                .iter()
                .any(|v| v.kind == ViolationKind::UnsupportedDdl && v.detail.contains(expected)),
            "'{name}' must be reported as UnsupportedDdl naming '{expected}', got {:#?}",
            result.violations
        );
    }
    Ok(())
}

/// The baseline claims to be shrink-only. Without this it was not: the gate has
/// no memory of the previous baseline — CI checks out a single commit with no
/// history — so appending one line admitted a whole new personal-data table
/// with no signal at all.
///
/// Migration numbers supply the missing clock. A column introduced after the
/// freeze is outside what the backlog declared, whether it arrived on a new
/// table or on one already listed.
#[test]
fn gate_rejects_a_table_appended_to_the_baseline_after_the_freeze()
-> Result<(), Box<dyn std::error::Error>> {
    let dir = tree(
        "baseline-append",
        "CREATE TABLE staff (id UUID PRIMARY KEY);
         COMMENT ON COLUMN staff.id IS 'pd:personal — surrogate key of a person row';",
    )?;
    fs::write(
        dir.join("migrations").join("9999_new_table.sql"),
        "CREATE TABLE medical_notes (id UUID PRIMARY KEY, diagnosis TEXT NOT NULL);",
    )?;

    // Without the entry the new table simply fails as unclassified.
    let unlisted = check_tree(&dir, &empty_baseline())?;
    assert!(!unlisted.passed());

    // Appending it to the baseline must NOT buy silence.
    let listed = check_tree(&dir, &baseline(&["medical_notes"]))?;
    assert!(
        listed
            .violations
            .iter()
            .any(|v| v.kind == ViolationKind::BaselineGrew
                && v.detail.contains("medical_notes.diagnosis")),
        "appending a post-freeze table to the baseline must fail, got {:#?}",
        listed.violations
    );
    Ok(())
}

/// The same clock closes the hole the table-level baseline opened by
/// construction: a NEW column on an ALREADY-listed table used to need no
/// classification at all, because the shelter was granted per table.
#[test]
fn gate_rejects_a_new_column_on_an_already_baselined_table()
-> Result<(), Box<dyn std::error::Error>> {
    let dir = tree(
        "baseline-column-growth",
        "CREATE TABLE staff (id UUID PRIMARY KEY, legacy TEXT);",
    )?;
    fs::write(
        dir.join("migrations").join("9999_add_rrn.sql"),
        "ALTER TABLE staff ADD COLUMN rrn TEXT;",
    )?;
    let result = check_tree(&dir, &baseline(&["staff"]))?;
    assert!(
        result
            .violations
            .iter()
            .any(|v| v.kind == ViolationKind::BaselineGrew && v.detail.contains("staff.rrn")),
        "a post-freeze column on a baselined table must fail, got {:#?}",
        result.violations
    );
    assert!(
        !result
            .violations
            .iter()
            .any(|v| v.detail.contains("staff.legacy")),
        "a pre-freeze column on a baselined table is still debt, not a failure: {:#?}",
        result.violations
    );
    Ok(())
}

/// Build residue is not our schema, and a gate whose verdict depends on whether
/// anyone has run Buck locally is not reproducible.
///
/// Buck materialises every third-party crate's source under `buck-out`, and
/// several ship migrations of their own — `apalis-postgres` has
/// `migrations/*.sql`, `sqlx` has SQLite test fixtures under a `migrations`
/// directory. Before this was excluded the gate demanded classifications for
/// `jobs`, `workers`, `user`, `post` and `comment`: 31 violations on a developer
/// machine that had built once, and silence on a fresh CI checkout where
/// `buck-out` does not exist. Neither number was about this repo's data.
#[test]
fn gate_ignores_migrations_vendored_under_buck_out() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tree(
        "buck-out",
        "CREATE TABLE staff (id UUID PRIMARY KEY);
         COMMENT ON COLUMN staff.id IS 'pd:personal — surrogate key of a person row';",
    )?;
    let vendored = dir.join("buck-out/v2/art/root/third-party/rust/__apalis-postgres__/migrations");
    fs::create_dir_all(&vendored)?;
    fs::write(
        vendored.join("20220530084123_jobs_workers.sql"),
        "CREATE TABLE jobs (id TEXT PRIMARY KEY, attempts INTEGER NOT NULL);",
    )?;
    let result = check_tree(&dir, &empty_baseline())?;
    assert!(
        result.passed(),
        "a third-party migration under buck-out must be invisible, got {:#?}",
        result.violations
    );
    assert_eq!(
        result.total_tables, 1,
        "only the first-party table may be counted"
    );
    Ok(())
}

/// A table created inside a dollar-quoted body must FAIL.
///
/// This test used to assert `result.passed()`, under the reasoning that DDL
/// built inside plpgsql "is not schema". It is schema — `EXECUTE format('CREATE
/// TABLE %I (secret TEXT)', 'ghost')` creates a real relation holding real
/// columns, and the gate could not see one of them. Asserting that a hole is
/// correct behaviour is the most expensive kind of test, because it makes the
/// next reader believe the hole was considered and accepted.
///
/// The lexer still does not read plpgsql as SQL — doing so would invent tables
/// and drop real ones. It keeps the body instead of discarding it, so the body
/// can be refused rather than trusted.
#[test]
fn gate_rejects_a_table_created_inside_a_dollar_quoted_body()
-> Result<(), Box<dyn std::error::Error>> {
    let bodies: &[(&str, &str)] = &[
        (
            "plpgsql-function",
            "CREATE FUNCTION f() RETURNS VOID LANGUAGE plpgsql AS $$
             BEGIN
                 EXECUTE format('CREATE TABLE %I (secret TEXT)', 'ghost');
             END;
             $$;",
        ),
        (
            "do-block",
            "DO $$
             BEGIN
                 CREATE TABLE ghost (secret TEXT);
             END;
             $$;",
        ),
    ];

    for (name, body) in bodies {
        let dir = tree(
            name,
            &format!(
                "CREATE TABLE staff (id UUID PRIMARY KEY);
                 COMMENT ON COLUMN staff.id IS 'pd:personal — surrogate key of a person row';
                 {body}"
            ),
        )?;
        let result = check_tree(&dir, &empty_baseline())?;
        assert!(
            !result.passed(),
            "'{name}' must FAIL: a table created in a dollar-quoted body holds columns the \
             gate cannot enumerate. Got a pass."
        );
        assert!(
            result
                .violations
                .iter()
                .any(|v| v.kind == ViolationKind::UnsupportedDdl
                    && v.detail.contains("dollar-quoted body")),
            "'{name}' must be reported as UnsupportedDdl naming the body, got {:#?}",
            result.violations
        );
    }
    Ok(())
}

/// A `DO` whose body is a SINGLE-QUOTED string must FAIL, whatever is in it.
///
/// This is the hole the test above did not cover, and it was measured against a
/// migrated database rather than imagined. `DO 'BEGIN ALTER TABLE
/// leave_requests ADD COLUMN medical_certificate_no TEXT; END'` is a `Tok::Str`,
/// not a `Tok::Body`, so the body scan never touched it and the gate passed. Add
/// a `COMMENT ON COLUMN` on some other unclassified column of the same table in
/// the same migration and the catalog assertion's per-table COUNT did not move
/// either — both halves of the control green over a live 진단서 번호 column.
/// That second half is gone: the catalog baseline is a SET of column names now,
/// so the compensating comment no longer pays for the added column. This test
/// stays because the gate is defence in depth, not because the class depends on
/// it.
///
/// The refusal is on the QUOTING, not on what the body says: the third case
/// here creates nothing at all and is still rejected. Scanning the literal would
/// have closed the one spelling that was measured and left `E'…'`, `U&'…'` and
/// the next one for someone else to find.
#[test]
fn gate_rejects_a_do_with_a_single_quoted_body() -> Result<(), Box<dyn std::error::Error>> {
    let statements: &[(&str, &str)] = &[
        (
            "adds-a-column",
            "DO 'BEGIN ALTER TABLE staff ADD COLUMN medical_certificate_no TEXT; END';",
        ),
        (
            "creates-a-table",
            "DO 'BEGIN CREATE TABLE ghost (secret TEXT); END';",
        ),
        // Column-neutral, and still refused: the gate is judging the quoting.
        ("harmless", "DO 'BEGIN PERFORM 1; END';"),
        // The escape-string spelling of the same literal, which a scan written
        // for plain `'…'` would walk straight past.
        (
            "escape-string",
            "DO E'BEGIN ALTER TABLE staff ADD COLUMN medical_certificate_no TEXT; END';",
        ),
    ];

    for (name, statement) in statements {
        let dir = tree(
            name,
            &format!(
                "CREATE TABLE staff (id UUID PRIMARY KEY);
                 COMMENT ON COLUMN staff.id IS 'pd:personal — surrogate key of a person row';
                 {statement}"
            ),
        )?;
        let result = check_tree(&dir, &empty_baseline())?;
        assert!(
            !result.passed(),
            "'{name}' must FAIL: a single-quoted DO body is never scanned, so the gate cannot \
             prove what it did. Got a pass."
        );
        assert!(
            result
                .violations
                .iter()
                .any(|v| v.kind == ViolationKind::UnsupportedDdl
                    && v.detail.contains("single-quoted body")),
            "'{name}' must be reported as UnsupportedDdl naming the quoting, got {:#?}",
            result.violations
        );
    }
    Ok(())
}

/// THE SAME BLOCK, ONE HEAD OVER. A single-quoted body carrying table DDL must
/// fail under whatever statement carries it.
///
/// The first fix for the single-quoted body was written at the `DO` head, and
/// this is what walked past it — measured on a migrated PostgreSQL 18.4, gate
/// EXIT=0 and catalog assertion EXIT=0, over a `medical_certificate_no` column
/// live in `pg_attribute`. The scan reads `Tok::Str` alongside `Tok::Body`
/// instead, which is one level below the head, so there is no head left to
/// move to. This test is the thing that fails if the check is ever hoisted back
/// up to a head.
#[test]
fn gate_rejects_a_function_whose_single_quoted_body_builds_table_ddl()
-> Result<(), Box<dyn std::error::Error>> {
    let statements: &[(&str, &str)] = &[
        (
            "create-function",
            "CREATE FUNCTION hole() RETURNS void AS
                 'BEGIN ALTER TABLE staff ADD COLUMN medical_certificate_no TEXT; END'
                 LANGUAGE plpgsql;",
        ),
        (
            "create-or-replace-function",
            "CREATE OR REPLACE FUNCTION hole() RETURNS void AS
                 'BEGIN ALTER TABLE staff ADD COLUMN medical_certificate_no TEXT; END'
                 LANGUAGE plpgsql;",
        ),
        (
            "create-function-creating-a-table",
            "CREATE FUNCTION hole() RETURNS void AS
                 'BEGIN CREATE TABLE ghost (secret TEXT); END'
                 LANGUAGE plpgsql;",
        ),
    ];

    for (name, statement) in statements {
        let dir = tree(
            name,
            &format!(
                "CREATE TABLE staff (id UUID PRIMARY KEY);
                 COMMENT ON COLUMN staff.id IS 'pd:personal — surrogate key of a person row';
                 {statement}"
            ),
        )?;
        let result = check_tree(&dir, &empty_baseline())?;
        assert!(
            !result.passed(),
            "'{name}' must FAIL: the body builds DDL this parser does not read, and the head \
             carrying it changes nothing about that. Got a pass."
        );
        assert!(
            result
                .violations
                .iter()
                .any(|v| v.kind == ViolationKind::UnsupportedDdl
                    && v.detail.contains("single-quoted body builds")),
            "'{name}' must be reported as UnsupportedDdl naming the body, got {:#?}",
            result.violations
        );
    }
    Ok(())
}

/// And the cost of that scan on the corpus is zero, which is why it is a scan
/// and not a refusal-by-quoting like `DO`'s.
///
/// `0064_platform_group_accounts.sql` writes `DEFAULT ARRAY['MEMBER']` and
/// `DEFAULT 'GROUP_ADMIN'` in a `CREATE OR REPLACE FUNCTION` parameter list:
/// top-level `Tok::Str` tokens that are not bodies at all. Refusing that head on
/// its quoting would fail the gate on a statement that creates no column.
/// Measured across the 210 migrations: 3,488 top-level single-quoted literals,
/// zero flagged.
#[test]
fn gate_accepts_string_literals_that_are_not_bodies() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tree(
        "literals-that-are-not-bodies",
        "CREATE TABLE staff (id UUID PRIMARY KEY, role TEXT NOT NULL DEFAULT 'MEMBER');
         COMMENT ON COLUMN staff.id IS 'pd:personal — surrogate key of a person row';
         COMMENT ON COLUMN staff.role IS 'pd:none — 권한 등급, not a person';
         CREATE OR REPLACE FUNCTION platform_create_group_account(
             p_tenant_roles TEXT[] DEFAULT ARRAY['MEMBER']::TEXT[],
             p_group_role TEXT DEFAULT 'GROUP_ADMIN'
         ) RETURNS void LANGUAGE plpgsql AS $$ BEGIN RETURN; END $$;
         INSERT INTO staff (id, role) VALUES (gen_random_uuid(), 'MEMBER');",
    )?;
    let result = check_tree(&dir, &empty_baseline())?;
    assert!(
        result.passed(),
        "a literal that is a default, not a body, must not trip the scan, got {:#?}",
        result.violations
    );
    Ok(())
}

/// The repo's standard RLS-arming idiom must still pass, or every future
/// migration that arms a table needs a waiver — and a gate that demands a
/// waiver for the house idiom is a gate someone eventually deletes.
///
/// This is the other half of the test above: the body scan is not "dollar
/// quote means reject", it is "reject unless the DDL inside is one of the
/// column-neutral `ALTER TABLE` actions the parser already recognises".
#[test]
fn gate_accepts_a_body_that_only_arms_row_level_security() -> Result<(), Box<dyn std::error::Error>>
{
    let dir = tree(
        "rls-arming",
        "CREATE TABLE staff (id UUID PRIMARY KEY);
         COMMENT ON COLUMN staff.id IS 'pd:personal — surrogate key of a person row';
         DO $$ DECLARE t TEXT; tenant_tables TEXT[] := ARRAY['staff']; BEGIN
             FOREACH t IN ARRAY tenant_tables LOOP
                 EXECUTE format('ALTER TABLE %I ENABLE ROW LEVEL SECURITY', t);
                 EXECUTE format('ALTER TABLE %I FORCE ROW LEVEL SECURITY', t);
             END LOOP;
         END $$;",
    )?;
    let result = check_tree(&dir, &empty_baseline())?;
    assert!(
        result.passed(),
        "an RLS-arming body changes no column list and must pass, got {:#?}",
        result.violations
    );
    Ok(())
}

/// A body that adds a COLUMN, however, is not neutral — the same idiom shape,
/// the opposite verdict. Without this the arming exemption above would be a
/// blanket pass for anything spelled `ALTER TABLE` inside a body.
#[test]
fn gate_rejects_a_body_that_adds_a_column() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tree(
        "body-add-column",
        "CREATE TABLE staff (id UUID PRIMARY KEY);
         COMMENT ON COLUMN staff.id IS 'pd:personal — surrogate key of a person row';
         DO $$ BEGIN
             EXECUTE format('ALTER TABLE %I ADD COLUMN rrn TEXT', 'staff');
         END $$;",
    )?;
    let result = check_tree(&dir, &empty_baseline())?;
    assert!(
        result
            .violations
            .iter()
            .any(|v| v.kind == ViolationKind::UnsupportedDdl),
        "a body that adds a column must FAIL, got {:#?}",
        result.violations
    );
    Ok(())
}

/// Every comma-separated action in an opaque `ALTER TABLE` must be proven
/// column-neutral. A neutral first action cannot buy a pass for a later column
/// mutation, while a body made entirely of the existing neutral vocabulary
/// must remain accepted.
#[test]
fn gate_rejects_any_non_neutral_action_in_an_opaque_multi_action_alter()
-> Result<(), Box<dyn std::error::Error>> {
    let hostile_actions = [
        (
            "later-add-column",
            "ENABLE ROW LEVEL SECURITY, ADD COLUMN medical_certificate_no TEXT",
        ),
        (
            "later-drop-column",
            "ENABLE ROW LEVEL SECURITY, DROP COLUMN id",
        ),
        (
            "later-implicit-add",
            "ENABLE ROW LEVEL SECURITY, ADD medical_certificate_no TEXT",
        ),
        (
            "later-rename",
            "ENABLE ROW LEVEL SECURITY, RENAME COLUMN id TO staff_id",
        ),
        (
            "later-unknown",
            "ENABLE ROW LEVEL SECURITY, ATTACH PARTITION staff_archive",
        ),
        (
            "later-malformed",
            "ENABLE ROW LEVEL SECURITY, , FORCE ROW LEVEL SECURITY",
        ),
    ];

    for (name, actions) in hostile_actions {
        let dir = tree(
            name,
            &format!(
                "CREATE TABLE staff(id UUID PRIMARY KEY);
                 COMMENT ON COLUMN staff.id IS 'pd:personal — surrogate key of a person row';
                 DO $$ BEGIN
                     EXECUTE format('ALTER TABLE %I {actions}', 'staff');
                 END $$;"
            ),
        )?;
        let result = check_tree(&dir, &empty_baseline())?;
        assert!(
            !result.passed(),
            "opaque multi-action ALTER '{name}' must fail when any action can change columns; \
             got a pass"
        );
        assert!(
            result
                .violations
                .iter()
                .any(|v| v.kind == ViolationKind::UnsupportedDdl
                    && v.detail.contains("dollar-quoted body builds `alter table`")),
            "'{name}' must be UnsupportedDdl naming the opaque ALTER, got {:#?}",
            result.violations
        );
    }

    let nested_dollar_hostile_actions = [
        (
            "nested-dollar-later-add-column",
            "ENABLE ROW LEVEL SECURITY, ADD COLUMN medical_certificate_no TEXT",
        ),
        (
            "nested-dollar-later-drop-column",
            "ENABLE ROW LEVEL SECURITY, DROP COLUMN id",
        ),
        (
            "nested-dollar-later-implicit-add",
            "ENABLE ROW LEVEL SECURITY, ADD medical_certificate_no TEXT",
        ),
        (
            "nested-dollar-later-rename",
            "ENABLE ROW LEVEL SECURITY, RENAME COLUMN id TO staff_id",
        ),
        (
            "nested-dollar-later-unknown",
            "ENABLE ROW LEVEL SECURITY, ATTACH PARTITION staff_archive",
        ),
        (
            "nested-dollar-later-malformed",
            "ENABLE ROW LEVEL SECURITY, , FORCE ROW LEVEL SECURITY",
        ),
    ];
    for (name, actions) in nested_dollar_hostile_actions {
        let dir = tree(
            name,
            &format!(
                "CREATE TABLE staff(id UUID PRIMARY KEY);
                 COMMENT ON COLUMN staff.id IS 'pd:personal — surrogate key of a person row';
                 DO $outer$ BEGIN
                     EXECUTE $ddl$ALTER TABLE staff {actions}$ddl$;
                 END $outer$;"
            ),
        )?;
        let result = check_tree(&dir, &empty_baseline())?;
        assert!(
            !result.passed(),
            "opaque nested-dollar ALTER '{name}' must fail when any action can change columns; \
             got a pass"
        );
        assert!(
            result
                .violations
                .iter()
                .any(|v| v.kind == ViolationKind::UnsupportedDdl
                    && v.detail.contains("dollar-quoted body builds `alter table`")),
            "'{name}' must be UnsupportedDdl naming the nested opaque ALTER, got {:#?}",
            result.violations
        );
    }

    // Signal admission and classification must project the same lexer tokens.
    // The discarded contents of a quoted token cannot spend the modifier
    // window for one while being absent from the other.
    let shared_token_projection_hostiles = [
        (
            "opaque-dollar-token-cannot-spend-alter-window",
            "ALTER $noise$one two three$noise$ TABLE staff ENABLE ROW LEVEL SECURITY, ADD COLUMN \
             medical_certificate_no TEXT",
            "alter table",
        ),
        (
            "opaque-string-token-cannot-spend-alter-window",
            "ALTER 'one two three' TABLE staff ENABLE ROW LEVEL SECURITY, ADD COLUMN \
             medical_certificate_no TEXT",
            "alter table",
        ),
        (
            "opaque-dollar-token-cannot-spend-create-window",
            "CREATE $noise$one two three$noise$ TABLE shadow_staff (medical_certificate_no TEXT)",
            "create table",
        ),
        (
            "opaque-comments-do-not-spend-modifier-window",
            "CREATE GLOBAL /* one two three four */ TEMPORARY TABLE shadow_staff \
             (medical_certificate_no TEXT)",
            "create global temporary table",
        ),
    ];
    for (name, command, construct) in shared_token_projection_hostiles {
        let dir = tree(
            name,
            &format!(
                "CREATE TABLE staff(id UUID PRIMARY KEY);
                 COMMENT ON COLUMN staff.id IS 'pd:personal — surrogate key of a person row';
                 DO $outer$ BEGIN
                     EXECUTE $ddl${command}$ddl$;
                 END $outer$;"
            ),
        )?;
        let result = check_tree(&dir, &empty_baseline())?;
        assert!(
            !result.passed(),
            "quoted/comment tokens must not make the admission signal narrower than the DDL \
             classifier for '{name}'"
        );
        assert!(
            result
                .violations
                .iter()
                .any(|v| v.kind == ViolationKind::UnsupportedDdl
                    && v.detail.contains(&format!("body builds `{construct}`"))),
            "'{name}' must be UnsupportedDdl naming the shared token projection, got {:#?}",
            result.violations
        );
    }

    // One semicolon-delimited command slice may carry at most one table-DDL
    // location. Otherwise the action tail of one phrase can swallow a later
    // phrase and make a neutral head buy a pass for unparsed trailing SQL.
    let ambiguous_same_statement_commands = [
        (
            "opaque-repeated-neutral-alter-without-semicolon",
            "ALTER TABLE staff ENABLE ROW LEVEL SECURITY ALTER TABLE staff FORCE ROW LEVEL \
             SECURITY",
        ),
        (
            "opaque-first-neutral-later-hostile-without-semicolon",
            "ALTER TABLE staff ENABLE ROW LEVEL SECURITY ALTER TABLE staff ADD COLUMN secret \
             TEXT",
        ),
        (
            "opaque-overlapping-alter-create-table-locations",
            "ALTER CREATE TABLE shadow_staff (secret TEXT)",
        ),
    ];
    for (name, command) in ambiguous_same_statement_commands {
        let dir = tree(
            name,
            &format!(
                "CREATE TABLE staff(id UUID PRIMARY KEY);
                 COMMENT ON COLUMN staff.id IS 'pd:personal — surrogate key of a person row';
                 DO $outer$ BEGIN
                     EXECUTE $ddl${command}$ddl$;
                 END $outer$;"
            ),
        )?;
        let result = check_tree(&dir, &empty_baseline())?;
        assert!(
            !result.passed(),
            "multiple local table-DDL locations in one statement must fail closed for '{name}'"
        );
        assert!(
            result
                .violations
                .iter()
                .any(|v| v.kind == ViolationKind::UnsupportedDdl
                    && v.detail.contains("cannot be inspected safely")),
            "'{name}' must be reported as ambiguous opaque DDL before action classification, got \
             {:#?}",
            result.violations
        );
    }

    let repeated_neutral = (0..8192)
        .map(|_| "ALTER TABLE staff ENABLE ROW LEVEL SECURITY")
        .collect::<Vec<_>>()
        .join(" ");
    let large_repeated_locations = tree(
        "opaque-8192-repeated-table-ddl-locations",
        &format!(
            "CREATE TABLE staff(id UUID PRIMARY KEY);
             COMMENT ON COLUMN staff.id IS 'pd:personal — surrogate key of a person row';
             DO $outer$ BEGIN
                 EXECUTE $ddl${repeated_neutral}$ddl$;
             END $outer$;"
        ),
    )?;
    let result = check_tree(&large_repeated_locations, &empty_baseline())?;
    assert!(
        !result.passed(),
        "8192 repeated locations must be rejected without classifying 8192 overlapping tails"
    );
    assert!(
        result
            .violations
            .iter()
            .any(|v| v.kind == ViolationKind::UnsupportedDdl
                && v.detail.contains("cannot be inspected safely")),
        "large repeated locations must report bounded ambiguity, got {:#?}",
        result.violations
    );

    let semicolon_separated_neutral = tree(
        "opaque-semicolon-separated-neutral-table-ddl",
        "CREATE TABLE staff(id UUID PRIMARY KEY);
         COMMENT ON COLUMN staff.id IS 'pd:personal — surrogate key of a person row';
         DO $outer$ BEGIN
             EXECUTE $ddl$ALTER TABLE staff ENABLE ROW LEVEL SECURITY;
                 ALTER TABLE staff FORCE ROW LEVEL SECURITY$ddl$;
         END $outer$;",
    )?;
    let result = check_tree(&semicolon_separated_neutral, &empty_baseline())?;
    assert!(
        result.passed(),
        "separate neutral statements must each retain their one-location classification, got \
         {:#?}",
        result.violations
    );

    let quoted_window_malformed_hostiles = [
        (
            "opaque-dollar-window-signaled-incomplete-comment",
            "ALTER $noise$one two three$noise$ TABLE staff ADD COLUMN secret TEXT /* \
             unterminated",
        ),
        (
            "opaque-string-window-signaled-incomplete-quote",
            "ALTER 'one two three' TABLE staff ADD COLUMN secret TEXT 'unterminated",
        ),
    ];
    for (name, command) in quoted_window_malformed_hostiles {
        let dir = tree(
            name,
            &format!(
                "CREATE TABLE staff(id UUID PRIMARY KEY);
                 COMMENT ON COLUMN staff.id IS 'pd:personal — surrogate key of a person row';
                 DO $outer$ BEGIN
                     EXECUTE $ddl${command}$ddl$;
                 END $outer$;"
            ),
        )?;
        let result = check_tree(&dir, &empty_baseline())?;
        assert!(
            !result.passed(),
            "quoted-window DDL with an incomplete child construct must fail closed for '{name}'"
        );
        assert!(
            result
                .violations
                .iter()
                .any(|v| v.kind == ViolationKind::UnsupportedDdl
                    && v.detail.contains("cannot be inspected safely")),
            "'{name}' must report unreadable opaque DDL, got {:#?}",
            result.violations
        );
    }

    let comment_separated_signals = [
        (
            "nested-dollar-comment-separated-alter",
            "ALTER /* signal words inside this comment do not count */ TABLE staff ENABLE ROW \
             LEVEL SECURITY, ADD COLUMN medical_certificate_no TEXT",
            "alter table",
        ),
        (
            "nested-dollar-comment-separated-create",
            "CREATE /* signal words inside this comment do not count */ TABLE shadow_staff \
             (medical_certificate_no TEXT)",
            "create table",
        ),
    ];
    for (name, command, construct) in comment_separated_signals {
        let dir = tree(
            name,
            &format!(
                "CREATE TABLE staff(id UUID PRIMARY KEY);
                 COMMENT ON COLUMN staff.id IS 'pd:personal — surrogate key of a person row';
                 DO $outer$ BEGIN
                     EXECUTE $ddl${command}$ddl$;
                 END $outer$;"
            ),
        )?;
        let result = check_tree(&dir, &empty_baseline())?;
        assert!(
            !result.passed(),
            "comment-separated nested-dollar DDL '{name}' must remain visible to the signal and \
             classifier"
        );
        assert!(
            result
                .violations
                .iter()
                .any(|v| v.kind == ViolationKind::UnsupportedDdl
                    && v.detail.contains(&format!("body builds `{construct}`"))),
            "'{name}' must be UnsupportedDdl naming the nested command, got {:#?}",
            result.violations
        );
    }

    let multiply_nested = tree(
        "opaque-multiple-complete-dollar-tags",
        "CREATE TABLE staff(id UUID PRIMARY KEY);
         COMMENT ON COLUMN staff.id IS 'pd:personal — surrogate key of a person row';
         DO $outer$ BEGIN
             EXECUTE $command$DO $middle$ BEGIN
                 EXECUTE $ddl$ALTER TABLE staff ENABLE ROW LEVEL SECURITY,
                     ADD COLUMN medical_certificate_no TEXT$ddl$;
             END $middle$;$command$;
         END $outer$;",
    )?;
    let result = check_tree(&multiply_nested, &empty_baseline())?;
    assert!(
        !result.passed(),
        "complete multiply nested dollar tags must not hide their inner DDL"
    );
    assert!(
        result
            .violations
            .iter()
            .any(|v| v.kind == ViolationKind::UnsupportedDdl
                && v.detail.contains("body builds `alter table`")),
        "multiply nested DDL must retain the every-action classifier, got {:#?}",
        result.violations
    );

    let neutral_actions = [
        "ENABLE ROW LEVEL SECURITY, FORCE ROW LEVEL SECURITY",
        "ADD CONSTRAINT staff_id_check CHECK (id IS NOT NULL), \
         VALIDATE CONSTRAINT staff_id_check, DROP CONSTRAINT staff_id_check",
        "ALTER COLUMN id SET NOT NULL, OWNER TO console_app",
    ];
    for (index, actions) in neutral_actions.into_iter().enumerate() {
        let dir = tree(
            &format!("opaque-neutral-multi-action-{index}"),
            &format!(
                "CREATE TABLE staff(id UUID PRIMARY KEY);
                 COMMENT ON COLUMN staff.id IS 'pd:personal — surrogate key of a person row';
                 DO $$ BEGIN
                     EXECUTE format('ALTER TABLE %I {actions}', 'staff');
                 END $$;"
            ),
        )?;
        let result = check_tree(&dir, &empty_baseline())?;
        assert!(
            result.passed(),
            "opaque ALTER containing only existing neutral actions must pass, got {:#?}",
            result.violations
        );
    }

    let nested_dollar_neutral = tree(
        "opaque-nested-dollar-neutral-multi-action",
        "CREATE TABLE staff(id UUID PRIMARY KEY);
         COMMENT ON COLUMN staff.id IS 'pd:personal — surrogate key of a person row';
         DO $outer$ BEGIN
             EXECUTE $ddl$ALTER TABLE staff ENABLE ROW LEVEL SECURITY, FORCE ROW LEVEL SECURITY$ddl$;
         END $outer$;",
    )?;
    let result = check_tree(&nested_dollar_neutral, &empty_baseline())?;
    assert!(
        result.passed(),
        "nested-dollar opaque ALTER containing only existing neutral actions must pass, got \
         {:#?}",
        result.violations
    );

    let nested_dollar_neutral_literal_data = [
        (
            "opaque-nested-dollar-neutral-escaped-quote-data",
            "ADD CONSTRAINT staff_quote_check CHECK ('O''Reilly' = 'O''Reilly'), DROP \
             CONSTRAINT staff_quote_check",
        ),
        (
            "opaque-nested-dollar-neutral-comment-looking-data",
            "ADD CONSTRAINT staff_comment_check CHECK ('/* not a comment' = '/* not a comment'), \
             DROP CONSTRAINT staff_comment_check",
        ),
        (
            "opaque-nested-dollar-neutral-tag-looking-data",
            "ADD CONSTRAINT staff_tag_check CHECK ('$dangling$' = '$dangling$'), DROP CONSTRAINT \
             staff_tag_check",
        ),
    ];
    for (name, actions) in nested_dollar_neutral_literal_data {
        let dir = tree(
            name,
            &format!(
                "CREATE TABLE staff(id UUID PRIMARY KEY);
                 COMMENT ON COLUMN staff.id IS 'pd:personal — surrogate key of a person row';
                 DO $outer$ BEGIN
                     EXECUTE $ddl$ALTER TABLE staff {actions}$ddl$;
                 END $outer$;"
            ),
        )?;
        let result = check_tree(&dir, &empty_baseline())?;
        assert!(
            result.passed(),
            "neutral literal data in nested-dollar ALTER '{name}' must not be recursively \
             reinterpreted as malformed SQL, got {:#?}",
            result.violations
        );
    }

    let signaled_incomplete_fragments = [
        (
            "opaque-signaled-incomplete-quote",
            "ALTER TABLE staff ADD CONSTRAINT staff_bad_check CHECK ('unterminated)",
        ),
        (
            "opaque-signaled-incomplete-comment",
            "ALTER TABLE staff ENABLE ROW LEVEL SECURITY /* unterminated",
        ),
    ];
    for (name, command) in signaled_incomplete_fragments {
        let dir = tree(
            name,
            &format!(
                "CREATE TABLE staff(id UUID PRIMARY KEY);
                 COMMENT ON COLUMN staff.id IS 'pd:personal — surrogate key of a person row';
                 DO $outer$ BEGIN
                     EXECUTE $ddl${command}$ddl$;
                 END $outer$;"
            ),
        )?;
        let result = check_tree(&dir, &empty_baseline())?;
        assert!(
            !result.passed(),
            "a DDL-signaled fragment with incomplete quoting must fail closed"
        );
        assert!(
            result
                .violations
                .iter()
                .any(|v| v.kind == ViolationKind::UnsupportedDdl
                    && v.detail.contains("cannot be inspected safely")),
            "'{name}' must be reported as unreadable opaque DDL, got {:#?}",
            result.violations
        );
    }

    // The scanner is iterative and bounded even when hostile input is nested
    // far beyond the process stack. Exercise both a signal-free leaf and DDL at
    // depth so skipping the outer fragments cannot hide the inner command.
    let nest_fragment = |leaf: &str| {
        let mut fragment = leaf.to_owned();
        for depth in 0..4096 {
            fragment = format!("$deep_{depth}${fragment}$deep_{depth}$");
        }
        fragment
    };
    let deeply_nested_neutral = tree(
        "opaque-4096-level-neutral-fragments",
        &format!(
            "CREATE TABLE staff(id UUID PRIMARY KEY);
             COMMENT ON COLUMN staff.id IS 'pd:personal — surrogate key of a person row';
             DO $outer$ BEGIN
                 PERFORM {};
             END $outer$;",
            nest_fragment("literal data")
        ),
    )?;
    let result = check_tree(&deeply_nested_neutral, &empty_baseline())?;
    assert!(
        result.passed(),
        "4096 complete signal-free nested fragments must remain bounded and neutral, got {:#?}",
        result.violations
    );

    let deeply_nested_hostile = tree(
        "opaque-4096-level-hostile-fragments",
        &format!(
            "CREATE TABLE staff(id UUID PRIMARY KEY);
             COMMENT ON COLUMN staff.id IS 'pd:personal — surrogate key of a person row';
             DO $outer$ BEGIN
                 EXECUTE {};
             END $outer$;",
            nest_fragment("ALTER TABLE staff ENABLE ROW LEVEL SECURITY, ADD COLUMN secret TEXT")
        ),
    )?;
    let result = check_tree(&deeply_nested_hostile, &empty_baseline())?;
    assert!(
        !result.passed(),
        "4096 complete nested fragments must not hide their hostile DDL leaf"
    );
    assert!(
        result
            .violations
            .iter()
            .any(|v| v.kind == ViolationKind::UnsupportedDdl
                && v.detail.contains("body builds `alter table`")),
        "deep hostile DDL must retain the shared classifier, got {:#?}",
        result.violations
    );

    let adjacent_split = tree(
        "opaque-adjacent-split-remains-registered",
        "CREATE TABLE staff(id UUID PRIMARY KEY);
         COMMENT ON COLUMN staff.id IS 'pd:personal — surrogate key of a person row';
         DO $$ BEGIN
             EXECUTE 'ALTER TA' || 'BLE staff ENABLE ROW LEVEL SECURITY, ADD COLUMN secret TEXT';
         END $$;",
    )?;
    let result = check_tree(&adjacent_split, &empty_baseline())?;
    assert!(
        result.passed(),
        "independently inspected adjacent fragments must not be joined into a claimed fix, got \
         {:#?}",
        result.violations
    );

    let malformed_nested_dollar = tree(
        "opaque-malformed-nested-dollar",
        "CREATE TABLE staff(id UUID PRIMARY KEY);
         COMMENT ON COLUMN staff.id IS 'pd:personal — surrogate key of a person row';
         DO $outer$ BEGIN
             EXECUTE $ddl$ALTER TABLE staff ENABLE ROW LEVEL SECURITY;
         END $outer$;",
    )?;
    let result = check_tree(&malformed_nested_dollar, &empty_baseline())?;
    assert!(
        !result.passed(),
        "an incomplete nested dollar quote must fail closed instead of ending work early"
    );
    assert!(
        result
            .violations
            .iter()
            .any(|v| v.kind == ViolationKind::UnsupportedDdl
                && v.detail.contains("cannot be inspected safely")),
        "the malformed nested quote must be reported as unreadable opaque input, got {:#?}",
        result.violations
    );
    Ok(())
}

/// THE INVERSION ITSELF.
///
/// Round 2 made unparseable mean FAIL by enumerating the constructs known to
/// be dangerous, which is the same fail-open shape one step further along: the
/// dispatch still required `head[1] == "table"`, so every spelling that puts a
/// word in slot 1 walked straight past it. `apply_statement` now has no
/// fallthrough arm — the recognised set is the whole allow-list — and these
/// cases fail because nothing recognises them, not because anyone listed them.
///
/// Each case is one line of DDL that the previous dispatch accepted in silence.
#[test]
fn gate_rejects_every_table_spelling_the_dispatch_does_not_recognise()
-> Result<(), Box<dyn std::error::Error>> {
    let cases: &[(&str, &str)] = &[
        ("unlogged", "CREATE UNLOGGED TABLE ghost (secret TEXT);"),
        ("temp", "CREATE TEMP TABLE ghost (secret TEXT);"),
        ("temporary", "CREATE TEMPORARY TABLE ghost (secret TEXT);"),
        (
            "global-temporary",
            "CREATE GLOBAL TEMPORARY TABLE ghost (secret TEXT);",
        ),
        (
            "create-schema-element-list",
            "CREATE SCHEMA hr CREATE TABLE ghost (secret TEXT);",
        ),
        (
            "materialized-view",
            "CREATE MATERIALIZED VIEW ghost AS SELECT 1;",
        ),
        ("truncate", "TRUNCATE TABLE staff;"),
    ];

    for (name, sql) in cases {
        let dir = tree(
            name,
            &format!(
                "CREATE TABLE staff (id UUID PRIMARY KEY);
                 COMMENT ON COLUMN staff.id IS 'pd:personal — surrogate key of a person row';
                 {sql}"
            ),
        )?;
        let result = check_tree(&dir, &empty_baseline())?;
        assert!(
            !result.passed(),
            "'{name}' must FAIL the gate: the dispatch does not recognise it, and an \
             unrecognised statement may create columns nobody can prove classified. Got a pass."
        );
        assert!(
            result
                .violations
                .iter()
                .any(|v| v.kind == ViolationKind::UnsupportedDdl),
            "'{name}' must be reported as UnsupportedDdl, got {:#?}",
            result.violations
        );
    }
    Ok(())
}

/// The allow-list must not have swallowed the corpus. If every statement were
/// unsupported the gate would be trivially fail-closed and equally useless, so
/// this pins the forms the repo actually writes as still recognised.
#[test]
fn gate_accepts_the_column_neutral_statements_the_repo_writes()
-> Result<(), Box<dyn std::error::Error>> {
    let dir = tree(
        "column-neutral",
        "CREATE TABLE staff (id UUID PRIMARY KEY, org_id UUID NOT NULL);
         COMMENT ON COLUMN staff.id IS 'pd:personal — surrogate key of a person row';
         COMMENT ON COLUMN staff.org_id IS 'pd:none — tenant key, not a person';
         CREATE SCHEMA hr AUTHORIZATION console_app;
         CREATE EXTENSION IF NOT EXISTS pg_trgm;
         CREATE UNIQUE INDEX staff_org_idx ON staff (org_id, id);
         CREATE INDEX CONCURRENTLY staff_org ON staff (org_id);
         CREATE POLICY org_isolation ON staff USING (true);
         CREATE OR REPLACE FUNCTION hr.touch() RETURNS TRIGGER LANGUAGE plpgsql AS $$
         BEGIN RETURN NEW; END; $$;
         CREATE TRIGGER staff_touch BEFORE UPDATE ON staff
             FOR EACH ROW EXECUTE FUNCTION hr.touch();
         CREATE VIEW staff_public AS SELECT id FROM staff;
         ALTER TABLE staff ENABLE ROW LEVEL SECURITY;
         ALTER TABLE staff FORCE ROW LEVEL SECURITY;
         ALTER TABLE staff ADD CONSTRAINT staff_org_check CHECK (org_id IS NOT NULL);
         ALTER TABLE staff VALIDATE CONSTRAINT staff_org_check;
         ALTER TABLE staff ALTER COLUMN org_id SET NOT NULL;
         ALTER TABLE staff OWNER TO console_app;
         ALTER TABLE staff DROP CONSTRAINT staff_org_check;
         LOCK TABLE public.staff IN SHARE ROW EXCLUSIVE MODE;
         SET LOCAL lock_timeout = '2s';
         GRANT SELECT ON staff TO console_rt;
         REVOKE ALL ON staff FROM PUBLIC;
         INSERT INTO staff (id, org_id) VALUES (gen_random_uuid(), gen_random_uuid());
         UPDATE staff SET org_id = org_id;
         WITH ordered AS (SELECT id FROM staff) UPDATE staff SET org_id = org_id;
         SELECT pg_advisory_xact_lock(hashtext('x'));",
    )?;
    let result = check_tree(&dir, &empty_baseline())?;
    assert!(
        result.passed(),
        "the corpus's column-neutral forms must stay recognised, got {:#?}",
        result.violations
    );
    assert_eq!(result.total_tables, 1, "only `staff` is a table");
    Ok(())
}

#[test]
fn gate_rejects_other_lock_and_setting_heads() -> Result<(), Box<dyn std::error::Error>> {
    for (name, sql) in [
        ("unlisted-lock", "LOCK VIEW staff;"),
        ("unlisted-setting", "SET LOCAL search_path = public;"),
    ] {
        let dir = tree(name, sql)?;
        let result = check_tree(&dir, &empty_baseline())?;
        assert!(
            result
                .violations
                .iter()
                .any(|v| v.kind == ViolationKind::UnsupportedDdl),
            "{name} must remain unsupported, got {:#?}",
            result.violations
        );
    }
    Ok(())
}

/// The clock `BASELINE_FROZEN_AFTER_MIGRATION` reads is a filename prefix,
/// written by the same author as the migration. It is honest only while every
/// number at or below the freeze is taken and stays taken — then a new
/// migration cannot claim one without colliding.
#[test]
fn gate_rejects_a_reused_or_vacant_pre_freeze_migration_number()
-> Result<(), Box<dyn std::error::Error>> {
    let dir = tree(
        "clock-reuse",
        "CREATE TABLE staff (id UUID PRIMARY KEY);
         COMMENT ON COLUMN staff.id IS 'pd:personal — surrogate key of a person row';",
    )?;
    // A second file claiming 0001: the prefix now says two different things.
    fs::write(
        dir.join("migrations").join("0001_sneaky.sql"),
        "CREATE TABLE medical_notes (id UUID PRIMARY KEY);",
    )?;
    let reused = check_tree(&dir, &baseline(&["medical_notes"]))?;
    assert!(
        reused
            .violations
            .iter()
            .any(|v| v.kind == ViolationKind::MigrationNumberReused),
        "a reused pre-freeze number must fail, got {:#?}",
        reused.violations
    );

    // A vacancy is the same hole one step earlier: a free slot below the
    // freeze is a slot a later migration can occupy and be read as old.
    let gapped = tree(
        "clock-vacancy",
        "CREATE TABLE staff (id UUID PRIMARY KEY);
         COMMENT ON COLUMN staff.id IS 'pd:personal — surrogate key of a person row';",
    )?;
    fs::write(
        gapped.join("migrations").join("0009_later.sql"),
        "CREATE TABLE notes (id UUID PRIMARY KEY);
         COMMENT ON COLUMN notes.id IS 'pd:none — surrogate key';",
    )?;
    let result = check_tree(&gapped, &empty_baseline())?;
    assert!(
        result
            .violations
            .iter()
            .any(|v| v.kind == ViolationKind::MigrationNumberVacant),
        "a vacant pre-freeze number must fail, got {:#?}",
        result.violations
    );
    Ok(())
}

const CLASSIFIED_INDEXED_STAFF: &str = "
    CREATE TABLE staff (id UUID PRIMARY KEY);
    COMMENT ON COLUMN staff.id IS 'pd:personal — surrogate key of a person row';
    CREATE INDEX staff_idx ON staff (id);
";

#[test]
fn drop_index_preserves_the_classified_column_inventory() -> Result<(), Box<dyn std::error::Error>>
{
    for statement in [
        "DROP INDEX staff_idx;",
        "DROP INDEX IF EXISTS staff_idx;",
        "DROP INDEX CONCURRENTLY IF EXISTS public.staff_idx;",
    ] {
        let dir = tree(
            "drop-index",
            &format!("{CLASSIFIED_INDEXED_STAFF}{statement}"),
        )?;
        let result = check_tree(&dir, &empty_baseline())?;
        assert!(
            result.passed(),
            "column-neutral {statement}: {:#?}",
            result.violations
        );
        assert_eq!(result.total_tables, 1);
        assert_eq!(result.total_columns, 1);
    }
    Ok(())
}

#[test]
fn drop_index_does_not_hide_an_adjacent_unclassified_column()
-> Result<(), Box<dyn std::error::Error>> {
    let dir = tree(
        "drop-index-then-column",
        &format!(
            "{CLASSIFIED_INDEXED_STAFF} DROP INDEX IF EXISTS staff_idx; ALTER TABLE staff ADD COLUMN secret TEXT;"
        ),
    )?;
    let result = check_tree(&dir, &empty_baseline())?;
    assert!(!result.passed());
    assert!(
        result
            .violations
            .iter()
            .any(|v| v.kind == ViolationKind::UnclassifiedColumn
                && v.detail.contains("staff.secret")),
        "the adjacent column must still be classified: {:#?}",
        result.violations
    );
    assert_eq!(result.total_columns, 2);
    Ok(())
}

#[test]
fn drop_index_does_not_short_circuit_opaque_ddl_inspection()
-> Result<(), Box<dyn std::error::Error>> {
    let dir = tree(
        "drop-index-opaque-column",
        &format!(
            "{CLASSIFIED_INDEXED_STAFF} DO $$ BEGIN EXECUTE 'DROP INDEX IF EXISTS staff_idx; ALTER TABLE staff ADD COLUMN secret TEXT'; END $$;"
        ),
    )?;
    let result = check_tree(&dir, &empty_baseline())?;
    assert!(!result.passed());
    assert!(
        result
            .violations
            .iter()
            .any(|v| v.kind == ViolationKind::UnsupportedDdl && v.detail.contains("alter table")),
        "opaque table alteration must remain rejected: {:#?}",
        result.violations
    );
    Ok(())
}

#[test]
fn drop_index_does_not_admit_unknown_drop_statements() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tree(
        "drop-index-unknown-drop",
        &format!(
            "{CLASSIFIED_INDEXED_STAFF} DROP INDEX IF EXISTS staff_idx; DROP TYPE unknown_type;"
        ),
    )?;
    let result = check_tree(&dir, &empty_baseline())?;
    assert!(!result.passed());
    assert!(
        result
            .violations
            .iter()
            .any(|v| v.kind == ViolationKind::UnsupportedDdl && v.detail.contains("drop type")),
        "unknown DROP must remain rejected: {:#?}",
        result.violations
    );
    Ok(())
}
