//! Constructs violations in throwaway trees and proves the gate rejects them.
//!
//! Shape copied from `backend/ci/gates/layer-boundary/tests/gate_detects_violation.rs`:
//! a gate that is never shown failing is not a gate.

use console_gate_writer_ownership::{KNOWN_SECOND_WRITERS, scan};
use console_ontology_canonical_domain::ObjectKey;
use std::fs;
use std::path::{Path, PathBuf};

fn temp_tree(name: &str) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let dir = std::env::temp_dir().join(format!(
        "console-writer-ownership-{name}-{}",
        std::process::id()
    ));
    if dir.exists() {
        fs::remove_dir_all(&dir)?;
    }
    fs::create_dir_all(&dir)?;
    Ok(dir)
}

fn write_file(path: &Path, content: &str) -> Result<(), Box<dyn std::error::Error>> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, content)?;
    Ok(())
}

/// Charged on any path that reads the file at all, so it separates "scanned"
/// from "not scanned at all" — a typo in the file name, a crate the tree skips.
const CANARY: &str = "pub const CANARY: &str = \"UPDATE organizations SET name = 'x'\";\n";

/// Charged ONLY on the raw-text path, so charging it means the probe did not
/// parse and the item walk never ran.
const PARSED_ONLY_CANARY: &str = "/// UPDATE org_units SET name = 'x'\npub struct ParsedCanary;\n";

/// Writes a crate whose package name is `name` at `dir`, with one source file.
fn crate_with_source(
    root: &Path,
    dir: &str,
    name: &str,
    rel_source: &str,
    body: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    write_file(
        &root.join(dir).join("Cargo.toml"),
        &format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\n"),
    )?;
    write_file(&root.join(dir).join(rel_source), body)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// RED: a second crate writing an owned table
// ---------------------------------------------------------------------------

#[test]
fn gate_detects_second_writer_of_an_owned_table() -> Result<(), Box<dyn std::error::Error>> {
    let root = temp_tree("second-writer")?;
    let owner = ObjectKey::Employment.owner_crate();

    crate_with_source(
        &root,
        "owner",
        owner,
        "src/lib.rs",
        "pub async fn ok(p: &sqlx::PgPool) {\n    sqlx::query(\"UPDATE employees SET org_unit = $1\").execute(p).await.ok();\n}\n",
    )?;
    crate_with_source(
        &root,
        "intruder",
        "console-some-other-adapter-postgres",
        "src/lib.rs",
        "pub async fn sneaky(p: &sqlx::PgPool) {\n    sqlx::query(\"UPDATE employees SET org_unit = $1\").execute(p).await.ok();\n}\n",
    )?;

    let report = scan(&root)?;
    assert!(
        !report.passed(),
        "expected the gate to FAIL on a second writer of `employees`, but it passed after scanning {} files",
        report.scanned_files
    );
    assert_eq!(
        report.unknown().len(),
        1,
        "an unratcheted second writer must reach the failing set: {:#?}",
        report.violations
    );
    let hit = report
        .violations
        .iter()
        .find(|v| v.table == "employees")
        .ok_or("expected an `employees` violation")?;
    assert_eq!(hit.offending_crate, "console-some-other-adapter-postgres");
    assert_eq!(hit.owner_crate, owner);
    assert_eq!(
        report.violations.len(),
        1,
        "the owning crate must not be reported: {:#?}",
        report.violations
    );
    Ok(())
}

/// The evasion the fabricated-branch gate concedes it cannot see. This gate is
/// not spelling-based: it matches the statement after whitespace collapse, so a
/// `format!`-built, line-broken, schema-qualified write is still a write.
#[test]
fn gate_detects_creatively_spelled_second_writer() -> Result<(), Box<dyn std::error::Error>> {
    let root = temp_tree("creative")?;
    crate_with_source(
        &root,
        "intruder",
        "console-some-other-adapter-postgres",
        "src/repo.rs",
        "pub fn sql(col: &str) -> String {\n    format!(\n        \"UPDATE\n             public.employees\n         SET {col} = $1\"\n    )\n}\n",
    )?;

    let report = scan(&root)?;
    assert!(
        !report.passed(),
        "line-broken, schema-qualified DML must still be seen; scanned {} files",
        report.scanned_files
    );
    Ok(())
}

/// Every spelling of a write against `employees` that a non-owner crate could
/// reach for. Each one is scanned on its own so a single case cannot be carried
/// by its neighbours, and each names the evasion it closes.
///
/// The two families here are different in kind. The first resolves the target
/// out of the statement (quoting, `ONLY`, a non-`public` schema, spaces around
/// the qualifier dot, `MERGE`/`TRUNCATE`/`COPY`, and any comment that closes
/// inside the literal holding it, whatever its body holds and whichever spelling
/// it is). The second cannot: the target is assembled at runtime, or a block
/// comment opens where the table should be and never closes inside its literal
/// — so the gate falls back on the fact that a write it could not resolve sits
/// in a file that names a canonical table. That fallback is what makes an
/// unresolved target fail CLOSED instead of silently passing.
///
/// A `--` comment IS here now, in the target position and in the separator
/// position, because it is resolved where its extent is known rather than
/// guessed at from the two bytes standing at a position — which is what made
/// charging it a measured false positive on `cargo update --workspace`
/// (`a_command_line_flag_after_a_verb_word_is_not_a_write`, still green).
#[test]
fn gate_detects_every_measured_evasion() -> Result<(), Box<dyn std::error::Error>> {
    let cases: &[(&str, &str)] = &[
        (
            "quoted-update",
            "pub const SQL: &str = \"UPDATE \\\"employees\\\" SET org_unit = $1\";",
        ),
        (
            "quoted-insert",
            "pub const SQL: &str = \"INSERT INTO \\\"employees\\\" (id) VALUES ($1)\";",
        ),
        (
            "merge",
            "pub const SQL: &str = \"MERGE INTO employees t USING s ON t.id = s.id WHEN MATCHED THEN UPDATE SET org_unit = s.org_unit\";",
        ),
        (
            "update-only",
            "pub const SQL: &str = \"UPDATE ONLY employees SET org_unit = $1\";",
        ),
        (
            "delete-from-only",
            "pub const SQL: &str = \"DELETE FROM ONLY employees WHERE id = $1\";",
        ),
        (
            "other-schema",
            "pub const SQL: &str = \"UPDATE hr.employees SET org_unit = $1\";",
        ),
        (
            "spaced-qualifier",
            "pub const SQL: &str = \"INSERT INTO public . employees (id) VALUES ($1)\";",
        ),
        (
            "truncate-table",
            "pub const SQL: &str = \"TRUNCATE TABLE employees\";",
        ),
        (
            // An inline comment is noise between the verb and its target, the
            // same as `ONLY`/`TABLE`, and is skipped by the same reading. This
            // is one static literal: nothing is assembled at either time.
            "inline-comment-before-target",
            "pub const SQL: &str = \"UPDATE /*+ IndexScan(employees) */ employees SET org_unit = $1\";",
        ),
        (
            // PostgreSQL block comments nest, so stopping at the first `*/`
            // would leave the target position pointing inside the comment.
            "nested-inline-comment-before-target",
            "pub const SQL: &str = \"DELETE FROM /* outer /* inner */ still a comment */ ONLY employees WHERE id = $1\";",
        ),
        (
            // A comment BODY is not a statement boundary, and both bytes this
            // reading uses to separate statements occur inside one. Bounding
            // the comment scan on them made this statement stop resolving and
            // fan out to every canonical table the file names.
            "quote-and-semicolon-in-the-comment-body",
            "pub const SQL: &str = \"UPDATE /* was: name = \\\"x\\\"; */ employees SET org_unit = $1\";",
        ),
        (
            // The `*/` here belongs to a DIFFERENT literal. Borrowing it read
            // `organizations` — a table this statement never targets — and let
            // the real, unreadable target go uncharged.
            "unclosed-comment-before-target",
            "pub fn sql(t: &str) -> String {\n    format!(\"UPDATE /* {t} SET x = 1\")\n}\npub const OTHER: &str = \"*/ organizations\";\npub mod employees {}",
        ),
        (
            // The same borrowed `*/`, inside ONE macro token stream — which
            // `Production::tokens` seals into ONE statement, so the two literals
            // are adjacent in the reading and only the per-literal resolution
            // keeps them apart.
            "unclosed-comment-and-a-later-literal-in-one-stream",
            "pub fn q() {\n    m!(\"UPDATE /* hint\", \"*/ organizations SET x = 1\");\n}\npub mod employees {}",
        ),
        (
            // And in a file `syn` cannot parse, where the raw-text fallback puts
            // the WHOLE file into one statement.
            "unclosed-comment-in-an-unparseable-file",
            "pub const SQL: &str = \"UPDATE /* SET x = 1\";\npub const OTHER: &str = \"*/ organizations\";\npub mod employees {}\npub fn broken( {",
        ),
        (
            // A `--` comment where the TABLE belongs. PostgreSQL runs this as
            // `UPDATE employees SET org_unit = $1`; the comment ends at the
            // newline inside this literal, which is where it is resolved.
            "line-comment-before-target",
            "pub const SQL: &str = \"UPDATE -- hint\nemployees SET org_unit = $1\";",
        ),
        (
            // The same reading through the OTHER carrier: a macro's tokens.
            // `sqlx::query!` is the ordinary way this repository spells SQL, and
            // its argument never reaches `Production::literal`.
            "comment-in-a-macro-argument",
            "pub fn q() {\n    sqlx::query!(\"UPDATE/*c*/employees SET org_unit = $1\");\n}",
        ),
        (
            "copy-from-stdin",
            "pub const SQL: &str = \"COPY employees (id, org_unit) FROM STDIN\";",
        ),
        (
            "format-local-binding",
            "pub fn sql(t: &str) -> String {\n    let t = if t.is_empty() { \"employees\" } else { t };\n    format!(\"UPDATE {t} SET org_unit = $1\")\n}",
        ),
        (
            "format-const",
            "const T: &str = \"employees\";\npub fn sql() -> String {\n    format!(\"UPDATE {T} SET org_unit = $1\")\n}",
        ),
        (
            "query-builder-split",
            "pub fn build(b: &mut sqlx::QueryBuilder<'_, sqlx::Postgres>) {\n    b.push(\"INSERT INTO \");\n    b.push(\"employees\");\n    b.push(\" (id) VALUES (1)\");\n}",
        ),
        (
            "concat-macro",
            "pub const SQL: &str = concat!(\"INSERT INTO \", \"employees\", \" (id) VALUES ($1)\");",
        ),
        (
            // `COPY` used to be excluded from the unresolved fallback, so this
            // scored zero violations while the identical `UPDATE` scored one.
            // `COPY … FROM STDIN` through `copy_in_raw` is a full bulk write.
            "format-copy-from-stdin",
            "const T: &str = \"employees\";\npub fn sql() -> String {\n    format!(\"COPY {T} (id) FROM STDIN\")\n}",
        ),
    ];

    let mut escaped = Vec::new();
    for (label, body) in cases {
        let root = temp_tree(&format!("evasion-{label}"))?;
        crate_with_source(
            &root,
            "intruder",
            "console-some-other-adapter-postgres",
            "src/repo.rs",
            &format!("{body}\n"),
        )?;
        let report = scan(&root)?;
        if !report
            .violations
            .iter()
            .any(|violation| violation.table == "employees")
        {
            escaped.push(*label);
        }
    }
    assert!(
        escaped.is_empty(),
        "these spellings wrote `employees` from a non-owner crate and the gate did not see them: {escaped:?}"
    );
    Ok(())
}

/// The fallback must not fire on a crate that merely reads. A write the gate
/// cannot resolve is a violation; a `SELECT` is not a write at all.
#[test]
fn unresolved_fallback_does_not_fire_on_reads() -> Result<(), Box<dyn std::error::Error>> {
    let root = temp_tree("reads-only")?;
    crate_with_source(
        &root,
        "reader",
        "console-some-other-adapter-postgres",
        "src/lib.rs",
        "pub fn sql(where_clause: &str) -> String {\n    format!(\"SELECT id FROM employees WHERE {where_clause}\")\n}\n",
    )?;
    let report = scan(&root)?;
    assert!(
        report.passed(),
        "reading a canonical table is not writing it: {:#?}",
        report.violations
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// GREEN: the owner may write; excluded surfaces are excluded
// ---------------------------------------------------------------------------

#[test]
fn gate_passes_when_only_the_owner_writes() -> Result<(), Box<dyn std::error::Error>> {
    let root = temp_tree("owner-only")?;
    crate_with_source(
        &root,
        "owner",
        ObjectKey::Employment.owner_crate(),
        "src/lib.rs",
        "pub const SQL: &str = \"UPDATE employees SET org_unit = $1\";\n",
    )?;
    let report = scan(&root)?;
    assert!(
        report.passed(),
        "the owning crate must be allowed to write its own table: {:#?}",
        report.violations
    );
    assert!(report.scanned_files > 0, "the owner file must be scanned");
    Ok(())
}

#[test]
fn integration_tests_and_migrations_are_excluded_from_the_static_gate()
-> Result<(), Box<dyn std::error::Error>> {
    let root = temp_tree("excluded")?;
    write_file(
        &root.join("intruder/Cargo.toml"),
        "[package]\nname = \"console-some-other-adapter-postgres\"\nversion = \"0.1.0\"\n",
    )?;
    // Integration test: excluded by path.
    write_file(
        &root.join("intruder/tests/employment_api.rs"),
        "const SQL: &str = \"UPDATE employees SET org_unit = $1\";\n",
    )?;
    // Migration: excluded by path (and .sql is not a scanned extension).
    write_file(
        &root.join("intruder/migrations/0999_x.sql"),
        "UPDATE employees SET org_unit = NULL;\n",
    )?;
    // Migration written as Rust, to prove the exclusion is by path not extension.
    write_file(
        &root.join("intruder/migrations/embedded.rs"),
        "const SQL: &str = \"UPDATE employees SET org_unit = $1\";\n",
    )?;

    let report = scan(&root)?;
    assert!(
        report.passed(),
        "tests/ and migrations/ are excluded from the static gate (handoff line 69): {:#?}",
        report.violations
    );
    Ok(())
}

#[test]
fn cfg_test_modules_inside_src_are_excluded() -> Result<(), Box<dyn std::error::Error>> {
    let root = temp_tree("cfg-test")?;
    crate_with_source(
        &root,
        "intruder",
        "console-some-other-adapter-postgres",
        "src/lib.rs",
        concat!(
            "pub fn production() {}\n",
            "\n",
            "#[cfg(test)]\n",
            "mod tests {\n",
            "    const SQL: &str = \"INSERT INTO organizations (id) VALUES ($1)\";\n",
            "}\n",
        ),
    )?;
    let report = scan(&root)?;
    assert!(
        report.passed(),
        "a #[cfg(test)] fixture is a test, not a production writer: {:#?}",
        report.violations
    );
    Ok(())
}

/// The exclusion must not swallow the rest of the file: production code after a
/// `#[cfg(test)]` module is still production code.
#[test]
fn production_code_after_a_cfg_test_module_is_still_scanned()
-> Result<(), Box<dyn std::error::Error>> {
    let root = temp_tree("after-cfg-test")?;
    crate_with_source(
        &root,
        "intruder",
        "console-some-other-adapter-postgres",
        "src/lib.rs",
        concat!(
            "#[cfg(test)]\n",
            "mod tests {\n",
            "    fn noop() {}\n",
            "}\n",
            "\n",
            "pub const SQL: &str = \"INSERT INTO organizations (id) VALUES ($1)\";\n",
        ),
    )?;
    let report = scan(&root)?;
    assert!(
        !report.passed(),
        "truncating at #[cfg(test)] would fail open; the trailing writer must be caught"
    );
    Ok(())
}

/// The same exclusion, with the closing brace carrying a trailing comment. The
/// consumption loop used to break only on the bare spelling `}`, so
/// `} // end tests` swallowed the entire rest of the file — every production
/// writer after a test module went unscanned, and the test above passed only
/// because it happened to use the bare spelling.
#[test]
fn a_commented_closing_brace_does_not_swallow_the_file() -> Result<(), Box<dyn std::error::Error>> {
    let root = temp_tree("commented-brace")?;
    crate_with_source(
        &root,
        "intruder",
        "console-some-other-adapter-postgres",
        "src/lib.rs",
        concat!(
            "#[cfg(test)]\n",
            "mod tests {\n",
            "    fn noop() {}\n",
            "} // end tests\n",
            "\n",
            "pub const SQL: &str = \"INSERT INTO organizations (id) VALUES ($1)\";\n",
        ),
    )?;
    let report = scan(&root)?;
    assert!(
        !report.passed(),
        "`}} // end tests` must close the test module, not the file: {:#?}",
        report.violations
    );
    Ok(())
}

/// The second and third spellings of the same bug. The consumption loop keyed
/// on braces found in the item's HEAD line, so a `#[cfg(test)]` on a `use` with
/// a brace list (`use std::collections::{A, B};`) set `opened = true` and then
/// swallowed every line to the next column-0 `}` — through the production
/// function that followed. A semicolon-terminated item is consumed to its
/// semicolon; a block item to its matching brace; and the attribute itself may
/// be `#[cfg(all(test, …))]` or be separated from its item by a blank line.
#[test]
fn a_cfg_test_item_is_consumed_by_its_own_shape() -> Result<(), Box<dyn std::error::Error>> {
    const WRITER: &str =
        "pub fn writes() {\n    let _ = \"UPDATE employees SET org_unit = $1\";\n}\n";
    let cases: [(&str, &str); 11] = [
        (
            // Not valid Rust, and the gate reads TEXT, not an AST: a file mid-edit
            // or machine-generated can leave a brace open. The depth count cannot
            // recover, so the consumption also stops at a `}` in column 0.
            "unbalanced-brace-in-test-item",
            "#[cfg(test)]\nmod tests {\n    fn broken() {\n}\n\n",
        ),
        (
            "use-with-brace-list",
            "#[cfg(test)]\nuse std::collections::{BTreeMap, BTreeSet};\n\n",
        ),
        (
            "use-with-multiline-brace-list",
            "#[cfg(test)]\nuse std::collections::{\n    BTreeMap,\n    BTreeSet,\n};\n\n",
        ),
        (
            "const-with-array-length",
            "#[cfg(test)]\nconst X: [u8; 2] = [1, 2];\n\n",
        ),
        (
            "cfg-all-test-and-feature",
            "#[cfg(all(test, feature = \"x\"))]\nuse std::collections::{BTreeMap, BTreeSet};\n\n",
        ),
        (
            "attribute-then-blank-line",
            "#[cfg(test)]\n\nmod tests {\n    fn noop() {}\n}\n\n",
        ),
        ("declared-module", "#[cfg(test)]\nmod tests;\n\n"),
        // The four token shapes a brace counter cannot count unless it LEXES
        // them. Each holds an unbalanced `{`, and each is spelled as a
        // semicolon-terminated item so that only the lexing can decide it —
        // there is no `}` in column 0 for the backstop above to rescue it with.
        // A counter that does not understand the token never reaches the
        // semicolon and swallows every production line that follows.
        (
            "char-literal-brace",
            "#[cfg(test)]\nconst OPEN: char = '{';\n\n",
        ),
        (
            "block-comment-brace",
            "#[cfg(test)]\nconst OPEN: u8 = 0; /* { */\n\n",
        ),
        (
            "raw-string-brace",
            "#[cfg(test)]\nconst Q: &str = r#\"a\"b{\"#;\n\n",
        ),
        (
            // A string literal is allowed to span lines, so the `{` inside one
            // is not syntax on either line it touches.
            "multiline-string-brace",
            "#[cfg(test)]\nconst S: &str = \"a\n{\";\n\n",
        ),
    ];
    for (label, prelude) in cases {
        let root = temp_tree(&format!("cfg-item-{label}"))?;
        crate_with_source(
            &root,
            "intruder",
            "console-x-adapter-postgres",
            "src/lib.rs",
            &format!("{prelude}{WRITER}"),
        )?;
        let report = scan(&root)?;
        assert!(
            !report.passed(),
            "{label}: the `#[cfg(test)]` item must be consumed by its own shape; \
             over-consuming it hides the production writer that follows"
        );
        assert_eq!(
            report.violations.len(),
            1,
            "{label}: exactly the trailing production write must be reported: {:#?}",
            report.violations
        );
    }
    Ok(())
}

/// The complement: a cfg predicate that NEGATES `test` marks PRODUCTION-only
/// code, so it must be kept. Dropping it would be the same fail-open in the
/// other direction, and a literal substring test for the one spelling
/// `not(test)` keeps only that one — the other four here were all consumed.
///
/// The last two are not negations at all: `feature = "test-utils"` is a STRING
/// (which is why string literals are stripped before the predicate is read), and
/// `latest` merely contains the letters (which is why the match is on a whole
/// token). Break either control and the attributed production writer vanishes.
#[test]
fn cfg_predicates_that_do_not_gate_on_test_are_production() -> Result<(), Box<dyn std::error::Error>>
{
    let cases: [(&str, &str); 8] = [
        ("not-test", "#[cfg(not(test))]"),
        ("not-all-test", "#[cfg(not(all(test)))]"),
        (
            "not-any-test-feature",
            "#[cfg(not(any(test, feature = \"mock\")))]",
        ),
        ("not-spaced-test", "#[cfg(not( test ))]"),
        (
            "not-all-test-feature",
            "#[cfg(not(all(test, feature = \"x\")))]",
        ),
        (
            "feature-named-test-utils",
            "#[cfg(feature = \"test-utils\")]",
        ),
        ("predicate-containing-test", "#[cfg(latest)]"),
        (
            "predicate-suffixed-test",
            "#[cfg(feature = \"x\", not_test)]",
        ),
    ];
    for (label, attribute) in cases {
        let root = temp_tree(&format!("cfg-production-{label}"))?;
        crate_with_source(
            &root,
            "intruder",
            "console-x-adapter-postgres",
            "src/lib.rs",
            &format!(
                "{attribute}\npub fn writes() {{\n    let _ = \"UPDATE employees SET org_unit = $1\";\n}}\n"
            ),
        )?;
        let report = scan(&root)?;
        assert!(
            !report.passed(),
            "{label}: `{attribute}` does not gate its item to test builds, so the \
             item is production code and must still be scanned: {:#?}",
            report.violations
        );
    }
    Ok(())
}

/// THE WHOLE cfg RULE, in BOTH directions, as one decidable predicate: an item
/// is skipped only when its cfg is UNSATISFIABLE with `test = false`.
///
/// The four rounds before this one shipped a LIST of spellings and lost a round
/// to each new one, and the last fix inverted the rule: `any(test, X)` is TRUE in
/// a production build whenever `X` holds, so items under it SHIP, and dropping
/// them from the scan is a fail-open. Here every case is stated against the rule,
/// not against a spelling, and both directions are asserted — over-scanning a
/// genuinely test-only item is a false positive that pressures maintainers into
/// ratchet entries, so it is a defect too.
#[test]
fn an_item_is_skipped_only_when_its_cfg_cannot_hold_without_test()
-> Result<(), Box<dyn std::error::Error>> {
    // (label, attribute, ships in a production build => must be SCANNED)
    let cases: [(&str, &str, bool); 16] = [
        // Satisfiable with test = false: production code.
        (
            "any-test-or-feature",
            "#[cfg(any(test, feature = \"x\"))]",
            true,
        ),
        ("any-test-or-unix", "#[cfg(any(test, unix))]", true),
        ("not-test", "#[cfg(not(test))]", true),
        ("not-all-test", "#[cfg(not(all(test)))]", true),
        (
            "not-any-test-feature",
            "#[cfg(not(any(test, feature = \"mock\")))]",
            true,
        ),
        ("not-spaced-test", "#[cfg(not( test ))]", true),
        (
            "not-all-test-feature",
            "#[cfg(not(all(test, feature = \"x\")))]",
            true,
        ),
        (
            "feature-named-test-utils",
            "#[cfg(feature = \"test-utils\")]",
            true,
        ),
        ("predicate-containing-test", "#[cfg(latest)]", true),
        ("no-cfg-at-all", "#[allow(dead_code)]", true),
        // An operator this gate does not model is UNKNOWN, and an unknown
        // predicate may hold in production. Fail CLOSED: scan it.
        ("unknown-operator", "#[cfg(version(\"1.80\"))]", true),
        // Unsatisfiable with test = false: test-only, skipped.
        ("bare-test", "#[cfg(test)]", false),
        (
            "all-test-feature",
            "#[cfg(all(test, feature = \"x\"))]",
            false,
        ),
        (
            "all-feature-test",
            "#[cfg(all(feature = \"x\", test))]",
            false,
        ),
        ("any-test-only", "#[cfg(any(test))]", false),
        ("double-negated-test", "#[cfg(not(not(test)))]", false),
    ];
    let mut wrong = Vec::new();
    for (label, attribute, ships) in cases {
        let root = temp_tree(&format!("cfg-rule-{label}"))?;
        crate_with_source(
            &root,
            "intruder",
            "console-x-adapter-postgres",
            "src/lib.rs",
            &format!(
                "{attribute}\npub fn writes() {{\n    let _ = \"UPDATE employees SET org_unit = $1\";\n}}\n"
            ),
        )?;
        let report = scan(&root)?;
        if report.passed() == ships {
            wrong.push(format!(
                "{label}: `{attribute}` {} in a production build, so the item must be \
                 {}; got {} violations",
                if ships { "CAN hold" } else { "cannot hold" },
                if ships { "SCANNED" } else { "SKIPPED" },
                report.violations.len()
            ));
        }
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
    Ok(())
}

/// FINDING 5: the attribute and its item on ONE line. A line-oriented consumer
/// starts on the FOLLOWING line at depth 0 and eats the next production item
/// whole.
#[test]
fn a_cfg_test_attribute_and_its_item_on_one_line_consume_only_that_item()
-> Result<(), Box<dyn std::error::Error>> {
    let root = temp_tree("one-line-cfg-item")?;
    crate_with_source(
        &root,
        "intruder",
        "console-x-adapter-postgres",
        "src/lib.rs",
        concat!(
            "#[cfg(test)] mod tests { fn noop() {} }\n",
            "pub fn writes() {\n",
            "    let _ = \"UPDATE employees SET org_unit = $1\";\n",
            "}\n",
        ),
    )?;
    let report = scan(&root)?;
    assert_eq!(
        report.violations.len(),
        1,
        "the one-line `#[cfg(test)]` item must be skipped and the production \
         function after it scanned: {:#?}",
        report.violations
    );
    Ok(())
}

/// A statement can be carried by any literal Rust can spell, not by the two the
/// gate happened to list. `c"…"` is `Lit::CStr`, a variant of its own, and
/// `PQexec` takes a `*const c_char`: an FFI caller is the natural home for that
/// spelling. Dropping a literal KIND is the same fail-open class as dropping a
/// token shape, and nothing here tested kinds before.
#[test]
fn every_literal_kind_that_can_carry_a_statement_is_read() -> Result<(), Box<dyn std::error::Error>>
{
    let cases: [(&str, &str); 6] = [
        (
            "str",
            "pub const Q: &str = \"UPDATE employees SET org_unit = $1\";",
        ),
        (
            "raw-str",
            "pub const Q: &str = r#\"UPDATE employees SET org_unit = $1\"#;",
        ),
        (
            "byte-str",
            "pub const Q: &[u8] = b\"UPDATE employees SET org_unit = $1\";",
        ),
        (
            "raw-byte-str",
            "pub const Q: &[u8] = br#\"UPDATE employees SET org_unit = $1\"#;",
        ),
        (
            "c-str",
            "pub const Q: &std::ffi::CStr = c\"UPDATE employees SET org_unit = $1\";",
        ),
        (
            "raw-c-str",
            "pub const Q: &std::ffi::CStr = cr#\"UPDATE employees SET org_unit = $1\"#;",
        ),
    ];
    let mut escaped = Vec::new();
    for (label, body) in cases {
        let root = temp_tree(&format!("literal-kind-{label}"))?;
        crate_with_source(
            &root,
            "intruder",
            "console-x-adapter-postgres",
            "src/lib.rs",
            &format!("{body}\n"),
        )?;
        let report = scan(&root)?;
        if report.passed() {
            escaped.push(label);
        }
    }
    assert!(
        escaped.is_empty(),
        "these literal kinds carried a full UPDATE to the driver and the gate did \
         not read them: {escaped:?}"
    );
    Ok(())
}

/// A `#` inside a macro's token stream is not an attribute marker. `quote!`'s
/// repetition form `#( … )*` is the ordinary way a proc-macro or codegen crate
/// emits a statement, and treating `#` as "skip the next group" drops the whole
/// repetition body — a token-shape rule rebuilt inside the walker that exists
/// because token-shape rules kept failing open.
#[test]
fn a_hash_in_a_macro_token_stream_does_not_hide_the_next_group()
-> Result<(), Box<dyn std::error::Error>> {
    let cases: [(&str, &str); 3] = [
        (
            "quote-repetition",
            "macro_rules! m { ($($t:tt)*) => {}; }\n\
             m!(#(sqlx::query(\"UPDATE employees SET org_unit = $1\"))*);\n",
        ),
        (
            "hash-group",
            "macro_rules! m { ($($t:tt)*) => {}; }\n\
             m!(# (\"UPDATE employees SET org_unit = $1\"));\n",
        ),
        (
            // An attribute's body is BRACKET delimited. A parenthesised group
            // whose first token happens to be `doc` is not an attribute, and
            // exempting it would hand back the evasion by spelling.
            "hash-paren-named-doc",
            "macro_rules! m { ($($t:tt)*) => {}; }\n\
             m!(#(doc \"UPDATE employees SET org_unit = $1\")*);\n",
        ),
    ];
    let mut escaped = Vec::new();
    for (label, body) in cases {
        let root = temp_tree(&format!("macro-hash-{label}"))?;
        crate_with_source(
            &root,
            "intruder",
            "console-x-adapter-postgres",
            "src/lib.rs",
            body,
        )?;
        let report = scan(&root)?;
        if report.passed() {
            escaped.push(label);
        }
    }
    assert!(
        escaped.is_empty(),
        "a `#`-prefixed group in a macro body still carries its tokens: {escaped:?}"
    );

    // The other direction, and the reason the `#` rule existed at all: `///`
    // inside a macro invocation arrives as `#[doc = "…"]`, and
    // `console-ontology-canonical-domain`'s own `object_keys!` documents which
    // crate holds `UPDATE employees SET …`. A doc comment is not compiled, so
    // it is not a writer — but ONLY a `doc` attribute's body is dropped.
    let root = temp_tree("macro-hash-doc-attribute")?;
    crate_with_source(
        &root,
        "intruder",
        "console-x-adapter-postgres",
        "src/lib.rs",
        "macro_rules! m { ($($t:tt)*) => {}; }\n\
         m!(#[doc = \"UPDATE employees SET org_unit = $1\"] struct X;);\n",
    )?;
    let report = scan(&root)?;
    assert!(
        report.passed(),
        "a doc comment inside a macro body cannot write a row: {:#?}",
        report.violations
    );
    Ok(())
}

/// The SAME rule as the macro-token walker above, at item level: only a `doc`
/// attribute is dropped. Every other attribute is compiled code — an attribute
/// proc-macro receives its tokens verbatim and can emit the statement, and a
/// derive helper is the ordinary place an ORM crate spells its table name — so
/// dropping the whole category is the token-shape rule this gate exists to
/// remove, just written as "it starts with `#`".
#[test]
fn only_a_doc_attribute_is_dropped_at_item_level() -> Result<(), Box<dyn std::error::Error>> {
    const WRITE: &str = "UPDATE employees SET org_unit = $1";
    // Each of these carries a full statement, or names the table for a write
    // whose target cannot be read. All must FAIL the gate.
    let caught: [(&str, String); 8] = [
        (
            "attribute-macro",
            format!("#[console_sql::exec(\"{WRITE}\")]\npub fn reassign() {{}}\n"),
        ),
        (
            "name-value",
            format!("#[sqlq = \"{WRITE}\"]\npub struct W;\n"),
        ),
        (
            "derive-helper",
            format!("#[derive(Q)]\n#[q(query = \"{WRITE}\")]\npub struct S;\n"),
        ),
        (
            "inner-attribute",
            format!("#![console_sql::exec(\"{WRITE}\")]\n"),
        ),
        (
            "struct-field",
            format!("pub struct S {{\n    #[q(query = \"{WRITE}\")]\n    pub a: u8,\n}}\n"),
        ),
        (
            "enum-variant",
            format!("pub enum E {{\n    #[q(query = \"{WRITE}\")]\n    A,\n}}\n"),
        ),
        (
            "fn-argument",
            format!("pub fn f(#[q(query = \"{WRITE}\")] _a: u8) {{}}\n"),
        ),
        (
            // The NAME reading, which needs no proc-macro authoring at all: the
            // write target is unreadable and the file's only mention of the
            // table is inside a derive helper.
            "name-only-in-attribute",
            "#[derive(Queryable)]\n#[diesel(table_name = employees)]\npub struct Row;\n\
             pub fn sql(qb: &mut Vec<String>, t: &str) {\n    \
             qb.push(\"UPDATE \".into());\n    qb.push(t.into());\n}\n"
                .to_owned(),
        ),
    ];
    let mut escaped = Vec::new();
    for (label, body) in &caught {
        let root = temp_tree(&format!("attr-{label}"))?;
        crate_with_source(
            &root,
            "intruder",
            "console-x-adapter-postgres",
            "src/lib.rs",
            body,
        )?;
        let report = scan(&root)?;
        if report.passed() {
            escaped.push(*label);
        }
    }
    assert!(
        escaped.is_empty(),
        "an attribute's tokens reach a proc macro verbatim, so these are \
         compiled writers the gate did not read: {escaped:?}"
    );

    // The other direction: a `doc` attribute is not compiled, and this
    // repository's own doc comments quote the statements the gate hunts for.
    // Over-scanning here would pressure maintainers into ratchet entries.
    for (label, body) in [
        ("doc-sugar", format!("/// {WRITE}\npub fn nothing() {{}}\n")),
        (
            "doc-desugared",
            format!("#[doc = \"{WRITE}\"]\npub fn nothing() {{}}\n"),
        ),
        ("doc-inner", format!("//! {WRITE}\npub fn nothing() {{}}\n")),
    ] {
        let root = temp_tree(&format!("attr-{label}"))?;
        crate_with_source(
            &root,
            "intruder",
            "console-x-adapter-postgres",
            "src/lib.rs",
            &body,
        )?;
        let report = scan(&root)?;
        assert!(
            report.passed(),
            "a doc comment cannot write a row ({label}): {:#?}",
            report.violations
        );
    }
    Ok(())
}

/// The doc-attribute rule is ONE rule, and this is the test that says so: the
/// same attribute spellings are put through BOTH positions an attribute can
/// appear in — at item level, and inside a macro's token stream — and the two
/// positions must return the SAME verdict.
///
/// They did not. Item level asked `path.is_ident("doc")` (a single-segment path
/// spelled `doc`); the macro walk asked whether the FIRST IDENT of the bracket
/// body was `doc`, which is a different question. `#[doc::hidden("UPDATE …")]`
/// is an attribute-macro invocation — COMPILED CODE that can emit the statement
/// — and it was dropped inside a macro body and read at item level.
#[test]
fn the_doc_attribute_rule_is_the_same_in_both_positions() -> Result<(), Box<dyn std::error::Error>>
{
    const WRITE: &str = "UPDATE employees SET org_unit = $1";
    // (label, attribute, is the attribute a doc comment and therefore dropped)
    let cases: [(&str, String, bool); 8] = [
        ("exact-doc", format!("#[doc = \"{WRITE}\"]"), true),
        ("doc-list", format!("#[doc(alias = \"{WRITE}\")]"), true),
        ("doc-bare-path", "#[doc]".to_owned(), true),
        // Everything below merely BEGINS with `doc`, or is spelled with a
        // leading `::`. None of them is the `doc` attribute; each is a
        // proc-macro invocation that receives its tokens verbatim.
        ("leading-colon", format!("#[::doc = \"{WRITE}\"]"), false),
        (
            "multi-segment",
            format!("#[doc::hidden(\"{WRITE}\")]"),
            false,
        ),
        (
            "first-segment-starts-with-doc",
            format!("#[doc_something::emit(\"{WRITE}\")]"),
            false,
        ),
        ("raw-ident", format!("#[r#doc = \"{WRITE}\"]"), false),
        ("longer-ident", format!("#[docs = \"{WRITE}\"]"), false),
    ];
    let mut wrong = Vec::new();
    for (label, attribute, dropped) in &cases {
        // Position 1: on an item. Position 2: inside a macro's token stream,
        // which is where `///` in `object_keys!` arrives as `#[doc = "…"]`.
        for (position, body) in [
            ("item", format!("{attribute}\npub fn nothing() {{}}\n")),
            ("macro", format!("m!({attribute} struct X;);\n")),
        ] {
            let root = temp_tree(&format!("doc-rule-{label}-{position}"))?;
            crate_with_source(
                &root,
                "intruder",
                "console-x-adapter-postgres",
                "src/lib.rs",
                &body,
            )?;
            let report = scan(&root)?;
            if report.passed() != *dropped {
                wrong.push(format!(
                    "{label} at {position}: expected dropped={dropped}, gate {}",
                    if report.passed() { "passed" } else { "failed" }
                ));
            }
        }
    }
    assert!(
        wrong.is_empty(),
        "one doc-attribute rule, two positions, one verdict: {wrong:#?}"
    );
    Ok(())
}

/// `syn::Item`, `syn::ImplItem` and `syn::TraitItem` are all `#[non_exhaustive]`
/// and each has a `Verbatim` arm holding tokens `syn` chose not to parse.
/// `syn::visit::visit_item` SKIPS that arm, so the fail-closed guarantee — an
/// unrecognised item reports no attributes and is therefore scanned — did not
/// hold: the item was reached and then read as nothing.
///
/// All THREE are exercised. Pinning only `Item::Verbatim` left the other two
/// arms unpinned: deleting them kept this suite green.
#[test]
fn a_verbatim_item_is_read_rather_than_skipped() -> Result<(), Box<dyn std::error::Error>> {
    let mut escaped = Vec::new();
    for (label, body) in [
        (
            "item",
            "pub macro m() {\n    let _ = \"UPDATE employees SET org_unit = $1\";\n}\n",
        ),
        (
            "trait-item",
            "pub trait T {\n    pub fn f() { let _ = \"UPDATE employees SET org_unit = $1\"; }\n}\n",
        ),
        (
            "impl-item",
            "pub struct S;\nimpl S {\n    const X: [u8; \"UPDATE employees SET org_unit = $1\".len()];\n}\n",
        ),
    ] {
        let root = temp_tree(&format!("verbatim-{label}"))?;
        crate_with_source(
            &root,
            "intruder",
            "console-x-adapter-postgres",
            "src/lib.rs",
            body,
        )?;
        if scan(&root)?.passed() {
            escaped.push(label);
        }
    }
    assert!(
        escaped.is_empty(),
        "a `Verbatim` item's tokens are still compiled code: {escaped:?}"
    );
    Ok(())
}

/// The four `Verbatim` arms that are NOT item positions: `Expr`, `Type`, `Pat`
/// and `TypeParamBound`. An earlier round enumerated the four ITEM positions by
/// hand and called the category closed; `syn` 2.0.117 has EIGHT `Verbatim`
/// arms, and these four were read as NOTHING — a fail-open, not an over-scan.
///
/// Each of these five files PARSES under the pinned `syn` (so the raw-text
/// fallback for an unparseable file does not cover them) and carries a full
/// `UPDATE` in a crate that does not own `employees`. Every one of them is
/// unstable or invalid on today's stable rustc, which is why the hole was
/// latent rather than exploitable — and exactly why a hand-maintained list
/// would never have grown the fifth entry.
///
/// This is the test that goes RED when the ONE override that replaced the
/// enumeration — `Visit::visit_token_stream`, which `syn` routes every
/// `Verbatim` arm through — is deleted or narrowed back to a list.
#[test]
fn every_verbatim_arm_syn_emits_is_read_rather_than_skipped()
-> Result<(), Box<dyn std::error::Error>> {
    const WRITE: &str = "UPDATE employees SET org_unit = $1";
    let mut escaped = Vec::new();
    for (label, body) in [
        (
            "expr-become",
            format!("pub fn f() {{ become g(\"{WRITE}\"); }}\n"),
        ),
        (
            "expr-builtin",
            format!("pub fn f() {{ let _ = builtin # offset_of(\"{WRITE}\"); }}\n"),
        ),
        (
            "pat-box",
            format!("pub fn f(v: &str) {{ if let box \"{WRITE}\" = v {{}} }}\n"),
        ),
        (
            "type-dyn-star",
            format!("pub type T = dyn* Tr<[u8; \"{WRITE}\".len()]>;\n"),
        ),
        (
            "type-bare-fn-arg",
            format!("pub type T = fn(mut self: [u8; \"{WRITE}\".len()]);\n"),
        ),
    ] {
        let root = temp_tree(&format!("verbatim-{label}"))?;
        crate_with_source(
            &root,
            "intruder",
            "console-x-adapter-postgres",
            "src/lib.rs",
            &body,
        )?;
        if scan(&root)?.passed() {
            escaped.push(label);
        }
    }
    assert!(
        escaped.is_empty(),
        "a `Verbatim` token stream is still compiled code, whichever node kind \
         holds it: {escaped:?}"
    );
    Ok(())
}

/// `ForeignItem` is one of the eight positions `syn`'s parser emits `Verbatim`
/// in, and `syn::visit::visit_foreign_item` skips it exactly as the others do.
/// Each of these three spellings was OBSERVED to parse to
/// `ForeignItem::Verbatim` with the pinned `syn`; unwalked they are read as
/// nothing.
#[test]
fn a_verbatim_foreign_item_is_read_rather_than_skipped() -> Result<(), Box<dyn std::error::Error>> {
    let mut escaped = Vec::new();
    for (label, body) in [
        (
            "static-with-initialiser",
            "unsafe extern \"C\" {\n    pub static SQL: &str = \"UPDATE employees SET org_unit = $1\";\n}\n",
        ),
        (
            "type-alias",
            "unsafe extern \"C\" {\n    type T = [u8; \"UPDATE employees SET org_unit = $1\".len()];\n}\n",
        ),
        (
            "fn-with-a-body",
            "unsafe extern \"C\" {\n    pub fn f() { let _ = \"UPDATE employees SET org_unit = $1\"; }\n}\n",
        ),
    ] {
        let root = temp_tree(&format!("verbatim-foreign-{label}"))?;
        crate_with_source(
            &root,
            "intruder",
            "console-x-adapter-postgres",
            "src/lib.rs",
            body,
        )?;
        if scan(&root)?.passed() {
            escaped.push(label);
        }
    }
    assert!(
        escaped.is_empty(),
        "a `ForeignItem::Verbatim`'s tokens are still compiled code: {escaped:?}"
    );
    Ok(())
}

/// A verb at the END of one token stream must not take the next node's text as
/// its target. Walking attributes put ordinary Rust into the STATEMENT reading,
/// and `#[derive(Debug, Clone, Copy)]` ends on a bare `Copy` — a SQL verb.
/// Flattened, the next thing appended became its target, so a `Copy` derive
/// above a `"organizations"` literal read as `COPY organizations`. Measured
/// against the real tree at backend/crates/platform/provisioning/src/lib.rs:999,
/// which reported an `organizations` violation it does not contain.
///
/// The boundary is the end of a token STREAM, whatever node kind holds it.
/// Narrowing it to an attribute's body — one round's answer to the same false
/// positive — left every stream an attribute body does not reach still gluing:
/// the last four cases below are ordinary Rust with no SQL in them, and each
/// reported an `organizations` violation. What is still charged despite the
/// boundary is pinned by
/// `a_write_whose_target_position_holds_a_placeholder_or_a_literal_boundary_is_charged_as_unresolved`,
/// and what is not by
/// `known_residual_a_write_that_misses_either_charging_condition_is_not_charged`.
#[test]
fn a_verb_ending_a_token_stream_does_not_take_the_next_node_as_its_target()
-> Result<(), Box<dyn std::error::Error>> {
    let mut manufactured = Vec::new();
    for (label, body) in [
        (
            "derive-copy-then-literal",
            "#[derive(Debug, Clone, Copy)]\npub struct S;\npub const Q: &str = \"organizations\";\n",
        ),
        (
            "derive-copy-then-attribute",
            "#[derive(Copy)]\n#[q(organizations)]\npub struct S;\n",
        ),
        (
            "derive-copy-then-macro",
            "#[derive(Copy)]\npub struct S;\nm!(organizations);\n",
        ),
        (
            // The same shape with `UPDATE`: an attribute argument that happens
            // to end on the verb.
            "attribute-ending-on-update",
            "#[q(mode = Update)]\npub struct S;\npub const Q: &str = \"organizations\";\n",
        ),
        // Below: the SAME glue, in streams that are not an item's attribute.
        // Terminating only an attribute's body left every one of these reading
        // the next node as the verb's target, on ordinary Rust with no SQL in
        // it at all.
        (
            "macro-ending-on-copy-then-literal",
            "m!(Debug, Clone, Copy);\npub const Q: &str = \"organizations\";\n",
        ),
        (
            "macro-ending-on-copy-then-macro",
            "m!(Copy);\nm2!(\"organizations\");\n",
        ),
        (
            "expression-macro-ending-on-update",
            "pub enum Op {\n    Update,\n    Read,\n}\npub fn f(op: &Op) -> bool {\n    matches!(op, Op::Update)\n}\npub const Q: &str = \"organizations\";\n",
        ),
        (
            // An attribute NESTED in a macro's tokens — the `quote!` shape.
            // An attribute-body boundary is reachable only from an ITEM's
            // attribute, so this one never saw a terminator.
            "attribute-inside-a-macro-then-literal",
            "pub fn f() {\n    let _ = quote::quote! { #[derive(Debug, Clone, Copy)] };\n}\npub const Q: &str = \"organizations\";\n",
        ),
    ] {
        let root = temp_tree(&format!("glue-{label}"))?;
        crate_with_source(
            &root,
            "intruder",
            "console-x-adapter-postgres",
            "src/lib.rs",
            body,
        )?;
        let report = scan(&root)?;
        if !report.passed() {
            manufactured.push((label, report.violations));
        }
    }
    assert!(
        manufactured.is_empty(),
        "a verb in one node and a name in the next are not one statement: {manufactured:#?}"
    );

    // The other direction, which is what makes the terminator a rule rather
    // than a mute: a statement written as BARE TOKENS inside one macro body is
    // still one statement and is still caught. Widening the terminator to every
    // token — making non-literal tokens mere separators — turns this green.
    //
    // A GROUP is not a node boundary either. Emitting the terminator from the
    // recursive token walk ended a statement at every `(`…`)`, so a verb and
    // its target separated by a group inside ONE macro body escaped — while the
    // ungrouped spelling above stayed caught.
    let mut escaped = Vec::new();
    for (label, body) in [
        ("bare-tokens", "m! { UPDATE employees SET org_unit = 1 }\n"),
        ("verb-in-a-group", "m! { (UPDATE) employees SET x = 1 }\n"),
        ("target-in-a-group", "m! { UPDATE (employees) SET x = 1 }\n"),
        (
            "both-in-one-group",
            "sql! { (INSERT INTO) employees VALUES (1) }\n",
        ),
    ] {
        let root = temp_tree(&format!("glue-one-macro-{label}"))?;
        crate_with_source(
            &root,
            "intruder",
            "console-x-adapter-postgres",
            "src/lib.rs",
            body,
        )?;
        if scan(&root)?.passed() {
            escaped.push(label);
        }
    }
    assert!(
        escaped.is_empty(),
        "a bare-token statement inside ONE macro body is still a write, \
         however it is grouped: {escaped:?}"
    );
    Ok(())
}

/// What the stream terminator COSTS — pinned here so the tradeoff is one test
/// rather than a paragraph of prose.
///
/// Ending a statement at every stream is one operation with the false positive
/// its sibling test pins: whether node N's text glues to node N+1's. Nothing in
/// a token stream separates "two adjacent nodes spelling one statement" from
/// ordinary Rust, so the two directions cannot both be had. The cost is that a
/// statement spelled as BARE TOKENS across two adjacent nodes is not read as
/// one. That cost is REAL and is a live fail-open, not a shape that cannot
/// reach a driver — see the residual test named below, which constructs one
/// that does.
///
/// What IS still charged is the shape this test's NAME describes, and nothing
/// wider: `write_targets` finds `update ` contiguously, and the byte in the
/// TARGET POSITION — the one after `skip_target_noise` has consumed any
/// `only `/`table `/`/*…*/` — is a `{` (a `format!` placeholder) or a `"` (a
/// literal's closing quote). That is an UNRESOLVED target, charged to every
/// canonical table the file names. Of the eight cases below, four hold `{` and
/// four hold `"`; `concat-fragments-at-compile-time` is assembled at COMPILE
/// time and is charged all the same, so the axis is not when the string is
/// built.
///
/// The last four cases are the ones the noise skip closed. They were filed as
/// residual for four rounds because the target position was read TWICE, once by
/// `read_target` (which consumed the prefix) and once by the marker test (which
/// re-read the byte in front of it, an `o` or a `t`).
///
/// The name says "placeholder or literal boundary" rather than "unreadable
/// target" because an unreadable target is NOT enough: an earlier name here
/// quantified over the trailing space alone, and a split inside the TARGET has
/// it and is charged nothing. That case is constructed in
/// `known_residual_a_write_that_misses_either_charging_condition_is_not_charged`.
/// The marker set `UNREADABLE_TARGET` has two more entries, `\` and `/*`,
/// charged the same way: `/*` has its case in
/// `gate_detects_every_measured_evasion` and `\` has none anywhere. The name
/// says `{` and `"` for that reason. Nothing here says a class is covered —
/// this test's scope is its name.
#[test]
fn a_write_whose_target_position_holds_a_placeholder_or_a_literal_boundary_is_charged_as_unresolved()
-> Result<(), Box<dyn std::error::Error>> {
    let mut escaped = Vec::new();
    for (label, body) in [
        (
            // Two adjacent macro invocations — the exact node boundary the
            // terminator now cuts — with the statement spelled as fragments.
            "two-adjacent-macro-streams",
            "m!(\"UPDATE \");\nm2!(employees);\n",
        ),
        (
            "builder-pushes-in-separate-statements",
            "pub fn build(b: &mut sqlx::QueryBuilder<'_, sqlx::Postgres>, t: &str) {\n    b.push(\"UPDATE \");\n    b.push(t);\n}\npub mod employees {}\n",
        ),
        (
            "runtime-target-in-a-format-placeholder",
            "pub fn sql(t: &str) -> String {\n    format!(\"UPDATE {t} SET x = 1\")\n}\npub mod employees {}\n",
        ),
        (
            // Compile time, not runtime, and still charged: the first fragment
            // ends AFTER the space.
            "concat-fragments-at-compile-time",
            "pub const Q: &str = concat!(\"UPDATE \", \"employees SET org_unit = 1\");\npub mod employees {}\n",
        ),
        // The four the noise skip closed: `ONLY`/`TABLE` in front of a target
        // that cannot be read.
        (
            "update-only-placeholder",
            "pub fn sql(t: &str) -> String {\n    format!(\"UPDATE ONLY {t} SET x = 1\")\n}\npub mod employees {}\n",
        ),
        (
            "truncate-table-placeholder",
            "pub fn sql(t: &str) -> String {\n    format!(\"TRUNCATE TABLE {t}\")\n}\npub mod employees {}\n",
        ),
        (
            "delete-from-only-placeholder",
            "pub fn sql(t: &str) -> String {\n    format!(\"DELETE FROM ONLY {t} WHERE id = 1\")\n}\npub mod employees {}\n",
        ),
        (
            "builder-update-only-then-push-target",
            "pub fn build(b: &mut sqlx::QueryBuilder<'_, sqlx::Postgres>, t: &str) {\n    b.push(\"UPDATE ONLY \");\n    b.push(t);\n}\npub mod employees {}\n",
        ),
    ] {
        let root = temp_tree(&format!("across-nodes-{label}"))?;
        crate_with_source(
            &root,
            "intruder",
            "console-x-adapter-postgres",
            "src/lib.rs",
            body,
        )?;
        let report = scan(&root)?;
        if !report
            .violations
            .iter()
            .any(|violation| violation.table == "employees")
        {
            escaped.push(label);
        }
    }
    assert!(
        escaped.is_empty(),
        "a `{{` or a `\"` in the target position is still charged as unresolved: {escaped:?}"
    );
    Ok(())
}

/// KNOWN RESIDUAL of the static half — residual 9 — constructed here so the
/// next reader finds it in the test file instead of rediscovering it.
///
/// The statement reading is TEXT, and `write_targets` charges a write only when
/// BOTH conditions hold:
///
/// 1. the verb AND the space after it arrive contiguously in the read text
///    (`update `, `delete from `, `truncate `, …), and
/// 2. the bytes in the TARGET POSITION — after `skip_target_noise` has consumed
///    any `only `/`table `/`/*…*/` — either read as an identifier whose last
///    component is the exact name of a canonical table, or fail to read as an
///    identifier while what stands there is an `UNREADABLE_TARGET` marker
///    (`{`, `"`, `\`, `/*`), which charges it as unresolved.
///
/// Miss either one and NOTHING is charged. Both misses are constructible, they
/// reach a driver, and every case below is built and scanned rather than
/// argued. Which condition each case misses was MEASURED by printing the read
/// text, not inferred from the shape of the source — the earlier version of
/// this doc filed the three `stringify!` cases under a miss of (1), and the
/// read text has `update ` contiguously in all three:
///
/// * MISSING (1) — the verb split across literals (`concat!("UPD", "ATE
///   employees …")`) and a verb literal that ENDS at the verb (`push("UPDATE")`
///   then `push(" ")`). Both are TWO fragments with the cut inside `update `.
/// * MISSING (2) with condition (1) held — a bare-token verb, which the token
///   walk renders WITH a trailing space and whose next byte is punctuation:
///   `stringify!` produces that inside one `concat!` (a `,`) and at the
///   per-stream boundary of [`Production::tokens`] (a `;`). Also a split inside
///   the TARGET (`concat!("UPDATE employ", "ees …")`), where `read_target`
///   succeeds with the non-canonical `employ` so the unresolved arm never runs.
///
/// So the residual is NOT "assembly at runtime" and NOT "a stream boundary".
/// `push("UPDATE ")` is runtime and IS charged, `concat!("UPDATE ", "employees
/// …")` is compile time and IS charged, and `stringify-in-one-stream` crosses
/// no boundary at all and is not charged.
///
/// The `;` at the stream boundary is therefore ONE spelling of the second miss,
/// not its cause. That was measured, not reasoned — deleting the `;` from
/// [`Production::tokens`] leaves every case below returning the same empty
/// report, so none of this is a regression from it. It is the standing cost of
/// reading tokens as written rather than as expanded (residual 2 in the module
/// doc).
///
/// What is NOT here any more: `UPDATE ONLY {t}`, `TRUNCATE TABLE {t}`, `DELETE
/// FROM ONLY {t}`, `push("UPDATE ONLY ")` and an inline `/*…*/` between verb
/// and target. Those were filed here while the target position was read twice
/// and the two readings disagreed; one shared `skip_target_noise` closed all
/// five, and they are now charged in
/// `a_write_whose_target_position_holds_a_placeholder_or_a_literal_boundary_is_charged_as_unresolved`
/// and in `gate_detects_every_measured_evasion`.
///
/// `"UPDATE -- hint\nemployees SET …"` is not here either, and it is not a
/// residual any longer: the comment is resolved inside its literal and the write
/// is charged, in `gate_detects_every_measured_evasion`. Every case that
/// remains here misses a condition because of what this half can SEE — tokens
/// as written rather than as expanded, and a target read as a whole identifier
/// — not because no one has written the code yet.
///
/// TWO canaries, in the same file as each probe, because one cannot see the two
/// ways a probe can prove nothing:
///
/// * `CANARY` writes `organizations` with a readable target. It is charged
///   whether the file parses or not, so it separates "scanned" from "not
///   scanned at all" — a typo in the file name, a crate the tree skips.
/// * `PARSED_ONLY_CANARY` writes `org_units` from inside a `///` doc comment,
///   which the item walk DROPS ([`is_doc_attribute`]) and the raw-text fallback
///   for an unparseable file KEEPS. It must NOT be charged. It is the only
///   thing here that separates "read as items" from "read as raw text", and
///   without it a probe body with an unbalanced paren reports the residual
///   still open on evidence from a code path the residual is not about.
///
/// Both canaries name tables other than `employees`, so neither can be confused
/// with the escape.
///
/// The assertion is deliberately the CURRENT behaviour. When something closes a
/// case — a real expansion, or a rule that survives the false positives
/// `a_verb_ending_a_token_stream_does_not_take_the_next_node_as_its_target`
/// pins — this test goes RED and that case must be moved, which is the point of
/// pinning it.
#[test]
fn known_residual_a_write_that_misses_either_charging_condition_is_not_charged()
-> Result<(), Box<dyn std::error::Error>> {
    let mut unread = Vec::new();
    let mut unparsed = Vec::new();
    let mut closed = Vec::new();
    for (label, body) in [
        // --- missing condition (1): the verb and its space are not contiguous.
        // --- Only these two miss it; both were measured by printing the read
        // --- text, not assumed from the shape of the source.
        (
            "verb-split-across-two-literals",
            "pub const Q: &str = concat!(\"UPD\", \"ATE employees SET org_unit = 1\");\n",
        ),
        (
            "verb-literal-ends-at-the-verb",
            "pub fn build(b: &mut sqlx::QueryBuilder<'_, sqlx::Postgres>, t: &str) {\n    b.push(\"UPDATE\");\n    b.push(\" \");\n    b.push(t);\n}\n",
        ),
        // --- missing condition (2): the verb IS contiguous with its space, and
        // --- the target position still resolves to nothing chargeable
        (
            // The bare token `UPDATE` is rendered with a trailing space by the
            // token walk, so (1) HOLDS; the byte after it is the `;` the stream
            // terminator emits.
            "stringify-in-a-sibling-macro-definition",
            "macro_rules! q { ($v:ident) => { concat!(stringify!($v), \" employees SET org_unit = 1\") }; }\npub const Q: &str = q!(UPDATE);\n",
        ),
        (
            "macro-export-from-a-module",
            "pub mod inner {\n    #[macro_export]\n    macro_rules! q2 { ($v:ident) => { concat!(stringify!($v), \" employees SET org_unit = 1\") }; }\n}\npub const Q: &str = q2!(UPDATE);\n",
        ),
        (
            // No stream boundary in this one: the byte in the target position
            // is the `,` between the two `concat!` arguments.
            "stringify-in-one-stream",
            "pub const Q: &str = concat!(stringify!(UPDATE), \" employees SET org_unit = 1\");\n",
        ),
        (
            "target-cut-across-two-literals",
            "pub const Q: &str = concat!(\"UPDATE employ\", \"ees SET org_unit = 1\");\n",
        ),
        (
            "target-cut-across-two-builder-pushes",
            "pub fn build(b: &mut sqlx::QueryBuilder<'_, sqlx::Postgres>) {\n    b.push(\"UPDATE employ\");\n    b.push(\"ees SET org_unit = 1\");\n}\n",
        ),
    ] {
        let escape = temp_tree(&format!("residual-{label}"))?;
        crate_with_source(
            &escape,
            "intruder",
            "console-x-adapter-postgres",
            "src/lib.rs",
            // `pub mod employees {}` names the table as an identifier, so
            // residual 1 — nothing to charge — cannot explain a green here.
            &format!("{CANARY}{PARSED_ONLY_CANARY}pub mod employees {{}}\n{body}"),
        )?;
        let escaped = scan(&escape)?;
        let charged = |table: &str| escaped.violations.iter().any(|v| v.table == table);
        if !charged("organizations") {
            unread.push((label, escaped.violations.clone()));
        } else if charged("org_units") {
            unparsed.push((label, escaped.violations.clone()));
        } else if charged("employees") {
            closed.push((label, escaped.violations.clone()));
        }
    }
    assert!(
        unread.is_empty(),
        "the canary write was not charged either: this probe tree was never read, \
         so it proves nothing about the residual: {unread:#?}"
    );
    assert!(
        unparsed.is_empty(),
        "the doc-comment canary was charged, so this probe file did not PARSE and \
         the gate scanned its raw text: the item walk never ran, so this probe \
         proves nothing about the residual: {unparsed:#?}"
    );
    assert!(
        closed.is_empty(),
        "KNOWN RESIDUAL closed — flip this test and the docs that name it: {closed:#?}"
    );
    Ok(())
}

/// A `--` in a string literal is a command-line flag far more often than it is
/// a PostgreSQL line comment, and at the target position NOTHING distinguishes
/// them: the marker test sees the two bytes standing there and no more. Putting
/// `--` in `UNREADABLE_TARGET` therefore charged this file — which merely NAMES
/// `employees` and writes nothing — one violation, where its parent commit
/// charged none.
///
/// A false positive is worse than the residual it removes. A gate that charges
/// `cargo update --workspace` is turned off by the next person it blocks, and
/// then it protects nothing.
///
/// The reading that replaced the marker has to survive the SAME file, and it
/// only does because it runs where the comment's extent is known. Both strings
/// here are stripped from the `--` to the end of the literal, which leaves a
/// verb word with nothing after it — and the byte put in its place is a `;`
/// rather than the literal's own closing quote, because that quote is an
/// `UNREADABLE_TARGET` marker and would fail this file closed all over again.
/// What that costs is residual 10, pinned next door in
/// `known_residual_a_line_comment_that_runs_off_the_end_of_its_literal`.
#[test]
fn a_command_line_flag_after_a_verb_word_is_not_a_write() -> Result<(), Box<dyn std::error::Error>>
{
    let root = temp_tree("cli-flag")?;
    crate_with_source(
        &root,
        "intruder",
        "console-x-adapter-postgres",
        "src/lib.rs",
        "pub mod employees {}\n\
         pub const CMD: &str = \"truncate --size=0 /var/log/app.log\";\n\
         pub const USAGE: &str = \"cargo update --workspace --locked\";\n",
    )?;
    let report = scan(&root)?;
    assert!(
        report.passed(),
        "a command-line flag is not a comment and not a write: {:#?}",
        report.violations
    );
    Ok(())
}

/// KNOWN RESIDUAL — a `--` comment that runs off the END of its literal, in a
/// statement assembled from more than one.
///
/// A comment is resolved inside the literal that holds it, and a `--` with no
/// newline left in that literal has taken the literal's own closing quote with
/// it. What stands in the target position is then a `;` — the rest of the LINE
/// is comment too, which is true of this literal and ASSUMED of the next one.
/// The assumption is wrong exactly when a later fragment supplies the newline,
/// which is the case below, and it is charged nothing.
///
/// It is the price of `a_command_line_flag_after_a_verb_word_is_not_a_write`:
/// `"truncate --size=0 …"` and `"cargo update --workspace --locked"` end the
/// same way and are not writes at all. Closing it needs the fragments JOINED,
/// which is a reading this half does not have (residual 9), not a byte at a
/// position.
///
/// Same two canaries as the residual test above, for the same two reasons.
#[test]
fn known_residual_a_line_comment_that_runs_off_the_end_of_its_literal()
-> Result<(), Box<dyn std::error::Error>> {
    let escape = temp_tree("residual-line-comment-off-the-end")?;
    crate_with_source(
        &escape,
        "intruder",
        "console-x-adapter-postgres",
        "src/lib.rs",
        &format!(
            "{CANARY}{PARSED_ONLY_CANARY}pub mod employees {{}}\n\
             pub fn build(b: &mut sqlx::QueryBuilder<'_, sqlx::Postgres>) {{\n    \
             b.push(\"UPDATE -- hint\");\n    \
             b.push(\"\\nemployees SET org_unit = 1\");\n}}\n"
        ),
    )?;
    let report = scan(&escape)?;
    let charged = |table: &str| report.violations.iter().any(|v| v.table == table);
    assert!(
        charged("organizations"),
        "the canary write was not charged either: this probe tree was never read, \
         so it proves nothing about the residual: {:#?}",
        report.violations
    );
    assert!(
        !charged("org_units"),
        "the doc-comment canary was charged, so this probe file did not PARSE and \
         the gate scanned its raw text: {:#?}",
        report.violations
    );
    assert!(
        !charged("employees"),
        "KNOWN RESIDUAL closed — flip this test and the docs that name it: {:#?}",
        report.violations
    );
    Ok(())
}

/// RESIDUAL 11 (CLOSED for `--` / `/*` via sqlparser): comment markers inside
/// single-quoted SQL DATA must NOT hide later statements in the SAME literal.
///
/// The sqlparser path tokenizes with string context so `'a -- b'` and `'a /* b'`
/// stay data. Cases below assert `employees` is charged when a later write
/// follows (`escapes=false`). A marker in one Rust literal still does not reach
/// a write in the NEXT one; a marker in data after the target is already
/// resolved before the comment opens.
///
/// Same two canaries as the residual test above, for the same two reasons.
#[test]
fn known_residual_a_dash_inside_quoted_sql_data_hides_a_later_statement()
-> Result<(), Box<dyn std::error::Error>> {
    for (label, body, escapes) in [
        (
            "update-then-update",
            "pub const SQL: &str = \"UPDATE t SET note = 'a -- b'; \
             UPDATE employees SET org_unit = 1\";",
            false,
        ),
        (
            "insert-then-update",
            "pub const SQL: &str = \"INSERT INTO t (note) VALUES ('x -- y'); \
             UPDATE employees SET org_unit = 1\";",
            false,
        ),
        (
            // The hole stops at the literal boundary: two separate literals,
            // and the `--` in the first does not reach the write in the second.
            "dash-in-a-different-literal",
            "pub const NOTE: &str = \"don't -- retry\";\n\
             pub const SQL: &str = \"UPDATE employees SET org_unit = 1\";",
            false,
        ),
        (
            // Data dash AFTER the target: the write is already resolved.
            "dash-after-the-target",
            "pub const SQL: &str = \"UPDATE employees SET note = 'a -- b'\";",
            false,
        ),
        // console-jth: `/*` inside quotes is the same extent question as `--`.
        (
            "block-comment-in-quoted-data",
            "pub const SQL: &str = \"UPDATE t SET note = 'a /* b'; \
             UPDATE employees SET org_unit = 1; SELECT 'x */ y'\";",
            false,
        ),
        (
            "unterminated-block-in-quoted-data",
            "pub const SQL: &str = \"UPDATE t SET note = 'a /* b'; \
             UPDATE employees SET org_unit = 1\";",
            false,
        ),
    ] {
        let escape = temp_tree(&format!("residual-quoted-data-dash-{label}"))?;
        crate_with_source(
            &escape,
            "intruder",
            "console-x-adapter-postgres",
            "src/lib.rs",
            &format!("{CANARY}{PARSED_ONLY_CANARY}pub mod employees {{}}\n{body}\n"),
        )?;
        let report = scan(&escape)?;
        let charged = |table: &str| report.violations.iter().any(|v| v.table == table);
        assert!(
            charged("organizations"),
            "[{label}] the canary write was not charged either: this probe tree was \
             never read, so it proves nothing about the residual: {:#?}",
            report.violations
        );
        assert!(
            !charged("org_units"),
            "[{label}] the doc-comment canary was charged, so this probe file did not \
             PARSE and the gate scanned its raw text: {:#?}",
            report.violations
        );
        assert_eq!(
            charged("employees"),
            !escapes,
            "[{label}] KNOWN RESIDUAL moved — flip this case and the docs that name \
             it: {:#?}",
            report.violations
        );
    }
    Ok(())
}

/// Escaped-newline line comment must NOT erase a later write (sqlparser path).
///
/// Source form `"UPDATE -- hint\\nemployees …"` is what `Literal::to_string()`
/// yields for a rustc string with an escape; with unescape off the lexer treated
/// the whole suffix as one EOF `--` comment (fail-OPEN). Fail CLOSED: charge.
#[test]
fn escaped_newline_line_comment_still_charges_the_write() -> Result<(), Box<dyn std::error::Error>>
{
    let escape = temp_tree("escaped-newline-line-comment")?;
    crate_with_source(
        &escape,
        "intruder",
        "console-x-adapter-postgres",
        "src/lib.rs",
        &format!(
            "{CANARY}{PARSED_ONLY_CANARY}pub mod employees {{}}\n\
             pub const SQL: &str = \"UPDATE -- hint\\nemployees SET org_unit = 1\";\n"
        ),
    )?;
    let report = scan(&escape)?;
    let charged = |table: &str| report.violations.iter().any(|v| v.table == table);
    assert!(
        charged("organizations"),
        "canary not charged — probe unread: {:#?}",
        report.violations
    );
    assert!(
        charged("employees"),
        "escaped-newline `--` must charge employees: {:#?}",
        report.violations
    );
    Ok(())
}

/// Quoted SQL DATA must not be scanned as a DML target (sqlparser path).
#[test]
fn quoted_sql_data_is_not_a_write_target() -> Result<(), Box<dyn std::error::Error>> {
    let escape = temp_tree("quoted-sql-data-not-target")?;
    crate_with_source(
        &escape,
        "intruder",
        "console-x-adapter-postgres",
        "src/lib.rs",
        &format!(
            "{CANARY}{PARSED_ONLY_CANARY}pub mod employees {{}}\n\
             pub mod notes {{}}\n\
             pub const SQL: &str = \"INSERT INTO notes(note) VALUES ('-- UPDATE employees SET x = 1')\";\n"
        ),
    )?;
    let report = scan(&escape)?;
    let charged = |table: &str| report.violations.iter().any(|v| v.table == table);
    assert!(
        charged("organizations"),
        "canary not charged — probe unread: {:#?}",
        report.violations
    );
    assert!(
        !charged("employees"),
        "quoted DATA must not charge employees: {:#?}",
        report.violations
    );
    Ok(())
}

/// The other half of the `--` decision, measured rather than argued by
/// symmetry: what `/*` in `UNREADABLE_TARGET` costs on ordinary Rust.
///
/// To reach the target position a `/*` must be spelled inside a string literal,
/// straight after a DML verb word and its space, and must not close later in
/// the same statement. Rust's own block comments never get there — `syn` drops
/// them, tokens and all — so the last case here is a full `UPDATE` inside one
/// and is charged nothing. The other three are the near misses: a glob that
/// closes, a command word that is not a DML verb, and the switch convention
/// Windows `copy` actually uses.
///
/// What DOES still charge is an unterminated absolute-root glob directly after
/// the verb word — `"copy /* to the clipboard"`, `"truncate /*.log"` — and that
/// is stated on `UNREADABLE_TARGET` as the standing cost of keeping `/*`. It is
/// not pinned as expected behaviour here; only the four zeroes are, because
/// those are the ones a change to the marker set must not break.
#[test]
fn ordinary_rust_that_puts_a_block_comment_marker_near_a_verb_is_not_charged()
-> Result<(), Box<dyn std::error::Error>> {
    let mut manufactured = Vec::new();
    for (label, body) in [
        (
            "glob-that-closes",
            "pub mod employees {}\npub const M: &str = \"copy /*.txt */ dest\";\n",
        ),
        (
            "command-word-that-is-not-a-dml-verb",
            "pub mod employees {}\npub const M: &str = \"cp -a /* /mnt\";\n",
        ),
        (
            "windows-copy-switch",
            "pub mod employees {}\npub const M: &str = \"copy /y src dst\";\n",
        ),
        (
            "rust-block-comment-holding-a-statement",
            "pub mod employees {}\npub const X: u8 = 1; /* UPDATE employees SET x = 1 */\n",
        ),
    ] {
        let root = temp_tree(&format!("slash-star-{label}"))?;
        crate_with_source(
            &root,
            "intruder",
            "console-x-adapter-postgres",
            "src/lib.rs",
            body,
        )?;
        let report = scan(&root)?;
        if !report.passed() {
            manufactured.push((label, report.violations));
        }
    }
    assert!(
        manufactured.is_empty(),
        "`/*` in the marker set manufactured a violation out of ordinary Rust: {manufactured:#?}"
    );
    Ok(())
}

/// A comment BODY may hold any byte, including the two an earlier bound tried to
/// stop the scan at. That bound was `"` or `;` — the separators the statement
/// reading emits — and both occur inside legitimate comment bodies, so the scan
/// aborted there: the statement stopped resolving to its real table, fell into
/// the unresolved fallback, and fanned out to every canonical table the file
/// names. Measured: the first case below charged `organizations` as well as
/// `employees`, and deleting the `;` from the comment dropped it back to one.
///
/// There is no such bound now. A comment is resolved inside the ONE literal
/// holding it, so the only thing that ends the search for its `*/` is the end of
/// that literal.
#[test]
fn a_block_comment_body_may_hold_a_quote_and_a_semicolon() -> Result<(), Box<dyn std::error::Error>>
{
    let mut wrong = Vec::new();
    for (label, body) in [
        (
            "semicolon-in-the-comment-body",
            "pub const SQL: &str = \"UPDATE /* was: SET x = 1; */ employees SET x = 1\";\npub mod organizations {}\n",
        ),
        (
            "quote-in-the-comment-body",
            "pub const SQL: &str = \"UPDATE /* was: name = \\\"x\\\" */ employees SET x = 1\";\npub mod organizations {}\n",
        ),
        (
            "both-in-the-comment-body",
            "pub const SQL: &str = \"DELETE FROM /* was: name = \\\"x\\\"; */ ONLY employees WHERE id = 1\";\npub mod organizations {}\n",
        ),
    ] {
        let root = temp_tree(&format!("comment-body-{label}"))?;
        crate_with_source(
            &root,
            "intruder",
            "console-x-adapter-postgres",
            "src/lib.rs",
            body,
        )?;
        let report = scan(&root)?;
        let tables: Vec<&str> = report
            .violations
            .iter()
            .map(|violation| violation.table.as_str())
            .collect();
        if tables != ["employees"] {
            wrong.push((label, tables.join(", ")));
        }
    }
    assert!(
        wrong.is_empty(),
        "the comment scan aborted inside its own body and the statement fanned out \
         to every canonical table the file names: {wrong:#?}"
    );
    Ok(())
}

/// A comment standing where the statement expects a SEPARATOR, and a `--`
/// comment standing where it expects the TABLE. Every one of these is a real
/// PostgreSQL write to `employees` from a crate that does not own it.
///
/// `write_targets` finds a verb by searching for it WITH its trailing space
/// (`update `, `insert into `), so a comment written in place of that space —
/// or in place of the space INSIDE `INSERT INTO` — defeats the verb match
/// outright unless the comment is resolved BEFORE the verb scan.
///
/// This was filed as a residual for two rounds, on the argument that resolving
/// comments would have to run over the whole flattened statement, where a `--`
/// in one literal would eat a real statement in the next one
/// (`m!("a--b", "UPDATE employees SET x = 1")`). That argument was about the
/// wrong GRANULARITY, not about the fix: a `--` comment ends at the newline
/// inside its own literal and a `/*…*/` closes inside its own literal, so
/// `without_sql_comments` runs on ONE literal at a time and cannot reach the
/// next one. `a_block_comment_does_not_close_on_a_later_literals_text` pins
/// that bound in all four ways two literals can meet, and
/// `a_command_line_flag_after_a_verb_word_is_not_a_write` pins the false
/// positive the marker test could not avoid.
///
/// Same two canaries as the residual test next door, for the same two reasons.
#[test]
fn a_comment_where_the_statement_expects_a_separator_or_a_table_is_charged()
-> Result<(), Box<dyn std::error::Error>> {
    let mut unread = Vec::new();
    let mut unparsed = Vec::new();
    let mut escaped = Vec::new();
    for (label, body) in [
        (
            "block-comment-for-the-space-after-the-verb",
            "pub const SQL: &str = \"UPDATE/*c*/employees SET org_unit = 1\";\n",
        ),
        (
            "line-comment-for-the-space-after-the-verb",
            "pub const SQL: &str = \"UPDATE--c\nemployees SET org_unit = 2\";\n",
        ),
        (
            "block-comment-for-the-space-after-only",
            "pub const SQL: &str = \"UPDATE ONLY/*c*/employees SET org_unit = 3\";\n",
        ),
        (
            "block-comment-inside-insert-into",
            "pub const SQL: &str = \"INSERT/*c*/INTO employees (id) VALUES (9)\";\n",
        ),
        (
            // The `--` at the TARGET position: PostgreSQL runs this as a write
            // to `employees`.
            "line-comment-in-the-target-position",
            "pub const SQL: &str = \"UPDATE -- hint\nemployees SET org_unit = 4\";\n",
        ),
        (
            // The other carrier: a macro's tokens never reach
            // `Production::literal`, and `sqlx::query!` is how this repository
            // ordinarily spells SQL.
            "block-comment-in-a-macro-argument",
            "pub fn q() {\n    sqlx::query!(\"UPDATE/*c*/employees SET org_unit = 5\");\n}\n",
        ),
    ] {
        let probe = temp_tree(&format!("separator-{label}"))?;
        crate_with_source(
            &probe,
            "intruder",
            "console-x-adapter-postgres",
            "src/lib.rs",
            &format!("{CANARY}{PARSED_ONLY_CANARY}pub mod employees {{}}\n{body}"),
        )?;
        let report = scan(&probe)?;
        let charged = |table: &str| report.violations.iter().any(|v| v.table == table);
        if !charged("organizations") {
            unread.push((label, report.violations.clone()));
        } else if charged("org_units") {
            unparsed.push((label, report.violations.clone()));
        } else if !charged("employees") {
            escaped.push((label, report.violations.clone()));
        }
    }
    assert!(
        unread.is_empty(),
        "the canary write was not charged either: this probe tree was never read, \
         so it proves nothing: {unread:#?}"
    );
    assert!(
        unparsed.is_empty(),
        "the doc-comment canary was charged, so this probe file did not PARSE and \
         the gate scanned its raw text: the item walk never ran, so this probe \
         proves nothing: {unparsed:#?}"
    );
    assert!(
        escaped.is_empty(),
        "these spellings wrote `employees` from a non-owner crate and the gate did \
         not see them: {escaped:#?}"
    );
    Ok(())
}

/// The unresolved fallback matches on a NAME, not on a statement. Only a literal
/// or a macro's tokens can carry a STATEMENT to the driver, but a table NAME can
/// also be a module, a type, or a `use` path — and that is the shape the
/// fallback exists for: a builder that pushes `"UPDATE "` and then a target it
/// cannot read, in a file that names the table as a Rust identifier.
#[test]
fn an_unreadable_target_is_charged_to_a_table_the_file_names_as_an_identifier()
-> Result<(), Box<dyn std::error::Error>> {
    let cases: [(&str, &str); 3] = [
        ("module", "pub mod employees {}\n"),
        ("type", "pub struct Employees;\n"),
        ("use-path", "use crate::schema::employees;\n"),
    ];
    let mut escaped = Vec::new();
    for (label, prelude) in cases {
        let root = temp_tree(&format!("ident-fallback-{label}"))?;
        crate_with_source(
            &root,
            "intruder",
            "console-x-adapter-postgres",
            "src/lib.rs",
            &format!(
                "{prelude}pub fn sql(qb: &mut Vec<String>, t: &str) {{\n    \
                 qb.push(\"UPDATE \".into());\n    qb.push(t.into());\n}}\n"
            ),
        )?;
        let report = scan(&root)?;
        if !report
            .violations
            .iter()
            .any(|violation| violation.table == "employees")
        {
            escaped.push(label);
        }
    }
    assert!(
        escaped.is_empty(),
        "a builder-assembled write in a file that names `employees` as an \
         identifier must fail CLOSED: {escaped:?}"
    );
    Ok(())
}

/// Source this gate cannot parse is not source it may ignore. A file rustc would
/// reject must be scanned in full, or "leave the file unparseable" is the
/// cheapest evasion there is.
#[test]
fn an_unparseable_file_is_scanned_rather_than_skipped() -> Result<(), Box<dyn std::error::Error>> {
    let root = temp_tree("unparseable")?;
    crate_with_source(
        &root,
        "intruder",
        "console-x-adapter-postgres",
        "src/lib.rs",
        "pub fn writes( {\n    let _ = \"UPDATE employees SET org_unit = $1\";\n",
    )?;
    let report = scan(&root)?;
    assert!(
        !report.passed(),
        "an unparseable file must fail CLOSED: {:#?}",
        report.violations
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// The two exclusions, keyed on the manifest tree rather than on the crate name
// ---------------------------------------------------------------------------

/// console-ugg: workspace-inherited `package =` renames must not fail-open.
///
/// `alias = { workspace = true }` with
/// `[workspace.dependencies] alias = { package = "real-pkg", … }` is a
/// production edge to `real-pkg`. A text scan that only sees the key `alias`
/// leaves `real-pkg` looking like a pure `[dev-dependencies]` crate when
/// another member also lists it under dev — and every source file of a
/// production writer is skipped. `cargo metadata` is the total primitive.
#[test]
fn workspace_inherited_package_rename_does_not_skip_a_shipped_writer()
-> Result<(), Box<dyn std::error::Error>> {
    let root = temp_tree("ugg-workspace-rename")?;
    write_file(
        &root.join("Cargo.toml"),
        r#"[workspace]
members = ["crates/*"]
resolver = "2"

[workspace.dependencies]
alias = { package = "real-pkg", path = "crates/real_pkg" }
"#,
    )?;
    write_file(
        &root.join("crates/real_pkg/Cargo.toml"),
        "[package]\nname = \"real-pkg\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )?;
    write_file(
        &root.join("crates/real_pkg/src/lib.rs"),
        &format!(
            "{CANARY}{PARSED_ONLY_CANARY}pub mod employees {{}}\n\
             pub const SQL: &str = \"UPDATE employees SET org_unit = 1\";\n"
        ),
    )?;
    write_file(
        &root.join("crates/producer/Cargo.toml"),
        "[package]\nname = \"producer\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n\
         [dependencies]\nalias = { workspace = true }\n",
    )?;
    write_file(
        &root.join("crates/producer/src/lib.rs"),
        "pub fn nothing() {}\n",
    )?;
    write_file(
        &root.join("crates/consumer/Cargo.toml"),
        "[package]\nname = \"consumer\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n\
         [dev-dependencies]\nreal-pkg = { path = \"../real_pkg\" }\n",
    )?;
    write_file(
        &root.join("crates/consumer/src/lib.rs"),
        "pub fn nothing() {}\n",
    )?;

    let report = scan(&root)?;
    let charged = |table: &str| report.violations.iter().any(|v| v.table == table);
    assert!(
        charged("organizations"),
        "canary must fire so the tree was read: {:#?}",
        report.violations
    );
    assert!(
        !charged("org_units"),
        "doc canary charged — parse failed: {:#?}",
        report.violations
    );
    assert!(
        charged("employees"),
        "workspace-inherited package= rename must keep real-pkg shipped so its \
         UPDATE employees is charged, not skipped as dev-only: {:#?}",
        report.violations
    );
    Ok(())
}

/// A crate every edge of which is a `[dev-dependencies]` edge is linked into
/// test binaries only. The exemption comes from the EDGE, not from the name:
/// `a_test_support_name_alone_buys_no_exemption` is the same crate without the
/// edge, and it is scanned.
#[test]
fn dev_dependency_only_crates_are_excluded() -> Result<(), Box<dyn std::error::Error>> {
    let root = temp_tree("dev-only")?;
    crate_with_source(
        &root,
        "support",
        "console-platform-test-support",
        "src/lib.rs",
        "pub const SQL: &str = \"INSERT INTO organizations (id, slug, name) VALUES ($1, $2, $3)\";\n",
    )?;
    // The consumer that makes it dev-only.
    write_file(
        &root.join("consumer/Cargo.toml"),
        "[package]\nname = \"console-consumer\"\nversion = \"0.1.0\"\n\n\
         [dev-dependencies]\nconsole-platform-test-support = { path = \"../support\" }\n",
    )?;
    write_file(&root.join("consumer/src/lib.rs"), "pub fn nothing() {}\n")?;

    let report = scan(&root)?;
    assert!(
        report.passed(),
        "a crate reached only through [dev-dependencies] is a fixture, not a \
         production writer: {:#?}",
        report.violations
    );
    Ok(())
}

/// The round-2 defect: the exclusion was `crate_name.ends_with("-test-support")`,
/// so any crate could opt itself out by choosing a name. It cannot now.
#[test]
fn a_test_support_name_alone_buys_no_exemption() -> Result<(), Box<dyn std::error::Error>> {
    let root = temp_tree("name-only-support")?;
    crate_with_source(
        &root,
        "support",
        "console-sneaky-test-support",
        "src/lib.rs",
        "pub const SQL: &str = \"INSERT INTO organizations (id, slug, name) VALUES ($1, $2, $3)\";\n",
    )?;
    let report = scan(&root)?;
    assert!(
        !report.passed(),
        "nothing in the tree declares this crate a dev-dependency, so the name \
         `-test-support` must not exempt it"
    );
    Ok(())
}

/// And the same for `console-gate-*`. `ci/gates/*` is a workspace member glob,
/// so a gate crate holding a real `UPDATE` used to be invisible by name alone.
/// Only the manifest LOCATION exempts it now.
#[test]
fn a_gate_name_outside_ci_gates_buys_no_exemption() -> Result<(), Box<dyn std::error::Error>> {
    let root = temp_tree("name-only-gate")?;
    crate_with_source(
        &root,
        "crates/sneaky",
        "console-gate-not-really",
        "src/lib.rs",
        "pub const SQL: &str = \"UPDATE employees SET org_unit = $1\";\n",
    )?;
    let report = scan(&root)?;
    assert!(
        !report.passed(),
        "a `console-gate-` name outside ci/gates/ must not exempt a real write"
    );

    // The same source, under ci/gates/, is a CI gate and is excluded.
    let gated = temp_tree("real-gate")?;
    crate_with_source(
        &gated,
        "ci/gates/not-really",
        "console-gate-not-really",
        "src/lib.rs",
        "pub const SQL: &str = \"UPDATE employees SET org_unit = $1\";\n",
    )?;
    let report = scan(&gated)?;
    assert!(
        report.passed(),
        "a crate whose manifest lives under ci/gates/ is a CI gate: {:#?}",
        report.violations
    );
    Ok(())
}

#[test]
fn comments_are_not_writers() -> Result<(), Box<dyn std::error::Error>> {
    let root = temp_tree("comments")?;
    crate_with_source(
        &root,
        "intruder",
        "console-some-other-adapter-postgres",
        "src/lib.rs",
        "//! Documents the `UPDATE employees SET org_unit = $1` this gate hunts.\n// INSERT INTO organizations (id) VALUES ($1)\npub fn nothing() {}\n",
    )?;
    let report = scan(&root)?;
    assert!(
        report.passed(),
        "a comment cannot write a row: {:#?}",
        report.violations
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// The ratchet: known dual writers are named exactly, and can only shrink
// ---------------------------------------------------------------------------

/// A ratchet entry that no longer matches a real violation fails the gate, so a
/// landed port lane must delete its entry rather than leave a live exemption.
/// The ratchet is EMPTY at this tip, so the old form of this test —
/// `stale_exemptions().len() == KNOWN_SECOND_WRITERS.len()` over a tree holding
/// none of the ratcheted writers — became `0 == 0` and would have passed against
/// a `stale_exemptions` that always returned nothing. What is still worth
/// proving, and is now STRONGER than what it replaced, is the consequence of the
/// list being empty: there is no longer any exemption path at all, so a second
/// writer of ANY canonical table is charged to [`Report::unknown`] and fails the
/// gate. The owner writing the same table in the same tree stays clean, which is
/// what stops this passing for the trivial reason that everything is charged.
#[test]
fn stale_ratchet_entries_fail_the_gate() -> Result<(), Box<dyn std::error::Error>> {
    assert!(
        KNOWN_SECOND_WRITERS.is_empty(),
        "the ratchet is empty at this tip; a re-added entry needs its own staleness proof, \
         not this test: {KNOWN_SECOND_WRITERS:#?}"
    );

    let root = temp_tree("stale")?;
    crate_with_source(
        &root,
        "owner",
        ObjectKey::Employment.owner_crate(),
        "src/lib.rs",
        "pub const SQL: &str = \"UPDATE employees SET org_unit = $1\";\n",
    )?;
    crate_with_source(
        &root,
        "intruder",
        "console-some-other-crate",
        "src/lib.rs",
        "pub const SQL: &str = \"INSERT INTO payroll_draft_runs (org_id) VALUES ($1)\";\n",
    )?;
    let report = scan(&root)?;

    assert_eq!(
        report.violations.len(),
        1,
        "only the non-owner is a violation; the owner's own write is not: {:#?}",
        report.violations
    );
    assert_eq!(
        report.unknown().len(),
        1,
        "with an empty ratchet every violation is unknown and fails the gate: {:#?}",
        report.violations
    );
    assert!(
        report.stale_exemptions().is_empty(),
        "an empty ratchet can hold no stale entry: {:#?}",
        report.stale_exemptions()
    );
    Ok(())
}

/// Every ratchet entry names a table that some object actually owns, and a lane
/// that removes it. An entry for an unowned table would be a silent no-op.
#[test]
fn ratchet_entries_are_well_formed() {
    let owned: std::collections::BTreeSet<&str> = ObjectKey::ALL
        .iter()
        .flat_map(|key| key.owned_tables().iter().copied())
        .collect();
    for known in KNOWN_SECOND_WRITERS {
        assert!(
            owned.contains(known.table),
            "{} is not owned by any object key, so ratcheting it does nothing",
            known.table
        );
        assert_ne!(
            known.offending_crate,
            ObjectKey::ALL
                .iter()
                .find(|key| key.owned_tables().contains(&known.table))
                .map_or("", |key| key.owner_crate()),
            "the owner of {} cannot be its own second writer",
            known.table
        );
        assert!(
            !known.removed_by.is_empty() && !known.source.is_empty(),
            "{known:?} must name its source and the lane that removes it"
        );
    }
}

/// The measured truth of this tree. This is what forces the ratchet to shrink:
/// when a port lane lands, its entry goes stale and this test turns red until
/// the entry is deleted.
#[test]
fn measured_tip_has_exactly_the_ratcheted_dual_writers() -> Result<(), Box<dyn std::error::Error>> {
    let backend = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let report = scan(&backend)?;
    assert!(
        report.scanned_files > 100,
        "the walk must reach the crate tree, scanned only {}",
        report.scanned_files
    );
    assert!(
        report.unknown().is_empty(),
        "unratcheted second writer in the tree: {:#?}",
        report.unknown()
    );
    assert!(
        report.stale_exemptions().is_empty(),
        "stale ratchet entry — the writer is gone, delete it: {:#?}",
        report.stale_exemptions()
    );
    assert_eq!(
        report.violations.len(),
        KNOWN_SECOND_WRITERS.len(),
        "measured dual writers: {:#?}",
        report.violations
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// The database half: its table roster may not drift from the registry
// ---------------------------------------------------------------------------

/// The `DO $canonical$ ... $canonical$;` body ONLY.
///
/// Slicing to end-of-file instead would make every assertion below unfalsifiable:
/// the same four command-role names reappear 20 lines after the block, in the
/// backend-termination step, so `contains(role)` would hold no matter what the
/// block itself said.
///
/// The opener is matched as a whole LINE. `DO $canonical$` also appears inside a
/// comment 400 lines earlier ("Read back by the DO $canonical$ block below"),
/// and splitting on the bare substring silently pulled those 400 lines of
/// unrelated role reconciliation into "the block" — which is how a
/// `!block.contains(…)` assertion can fail on text the block does not contain.
fn canonical_block() -> Result<String, Box<dyn std::error::Error>> {
    let script = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../../ops/postgres-reconcile-topology.sh");
    let text =
        fs::read_to_string(&script).map_err(|error| format!("{}: {error}", script.display()))?;
    let (_, rest) = text
        .split_once("\nDO $canonical$\n")
        .ok_or("topology script lost the canonical writer-ownership block")?;
    let (block, _) = rest
        .split_once("$canonical$;")
        .ok_or("the canonical writer-ownership block is not terminated")?;
    Ok(block.to_owned())
}

/// The text between a marker's BEGIN and END lines, the marker's own line
/// dropped. The BEGIN line is a comment, and callers that parse literals would
/// otherwise read an apostrophe in it ("the crate's tables") as one.
fn marked_region<'a>(block: &'a str, marker: &str) -> Result<&'a str, Box<dyn std::error::Error>> {
    let (_, rest) = block
        .split_once(&format!("canonical-writer-ownership: BEGIN {marker}"))
        .ok_or_else(|| format!("the canonical block lost its BEGIN {marker} marker"))?;
    let rest = rest.split_once('\n').map_or("", |(_, tail)| tail);
    let (region, _) = rest
        .split_once(&format!("canonical-writer-ownership: END {marker}"))
        .ok_or_else(|| format!("the canonical block lost its END {marker} marker"))?;
    Ok(region)
}

/// Every single-quoted literal between the two markers, comments dropped.
fn marked_list(block: &str, marker: &str) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let region = marked_region(block, marker)?;
    let mut found = Vec::new();
    for line in region.lines() {
        let line = line.trim();
        if line.starts_with("--") {
            continue;
        }
        let mut rest = line;
        while let Some((_, after)) = rest.split_once('\'') {
            let Some((literal, tail)) = after.split_once('\'') else {
                break;
            };
            found.push(literal.to_owned());
            rest = tail;
        }
    }
    Ok(found)
}

/// `ops/postgres-reconcile-topology.sh` carries the same table roster, because
/// SQL cannot import a Rust constant. This binds the two: a seventh object key
/// that never reaches the database half fails here.
#[test]
fn topology_script_table_roster_matches_the_registry() -> Result<(), Box<dyn std::error::Error>> {
    let in_script = marked_list(&canonical_block()?, "table roster")?;
    let registry: Vec<String> = ObjectKey::ALL
        .iter()
        .flat_map(|key| key.owned_tables().iter().map(|table| (*table).to_owned()))
        .collect();
    assert_eq!(
        in_script, registry,
        "the topology script's canonical table roster drifted from ObjectKey::owned_tables"
    );
    Ok(())
}

/// The database half must name, exactly, which roles lose DML and which writers
/// are still expected — as parsed lists inside the block, not as substrings of
/// the rest of the script.
///
/// Both assertions are set EQUALITIES. An inequality (`contains`) cannot fail
/// when a role is dropped from the check, which is the defect this replaces.
#[test]
fn topology_script_names_the_roles_that_lose_dml() -> Result<(), Box<dyn std::error::Error>> {
    let block = canonical_block()?;

    assert_eq!(
        marked_list(&block, "revoked roles")?,
        [
            "console_leave_cmd",
            "console_ontology_cmd",
            "console_platform_force_cmd",
        ],
        "these three command roles reach their data through SECURITY DEFINER \
         functions; the block must actively REVOKE table DML from every one"
    );

    // The measured writer surface of the canonical relations at this tip. Every
    // pair is a live holder with the lane that deletes it, or a role PostgreSQL
    // itself gives DML to. `console_app` OWNS every canonical table and
    // `pg_write_all_data` holds DML in every cluster: naming them is what makes
    // a rogue OWNER and a MEMBER of a write role ordinary findings instead of
    // structural blind spots.
    assert_eq!(
        marked_list(&block, "expected writers")?,
        [
            "console_app",
            "*",
            "console_rt",
            "*",
            "console_leave_definer",
            "employees",
            "pg_write_all_data",
            "*",
        ],
        "the expected-writer ratchet drifted; every role that can write a \
         canonical relation must be named here with the lane that removes it"
    );

    for verb in ["'INSERT'", "'UPDATE'", "'DELETE'", "'TRUNCATE'"] {
        assert!(
            block.contains(verb),
            "{verb} is not covered by the DML census"
        );
    }
    assert!(
        block.contains("REVOKE INSERT, UPDATE, DELETE, TRUNCATE"),
        "the database half must actually revoke, not merely describe a revoke"
    );
    assert!(
        !block.contains("RETURN;"),
        "an early RETURN makes the whole census a no-op; the block must run on \
         every reconcile against the tables that exist now"
    );

    // The hand-maintained catalog unions this round DELETED. Each was a
    // separate guard for one way a DML privilege can arrive, and each round
    // added the next one a reviewer named. `has_table_privilege` /
    // `has_any_column_privilege` answer all of them at once, so a re-appearance
    // of any of these names is the enumeration growing back.
    //
    // Deleting the `relowner` entry outright was measured, not argued: with
    // `LEFT JOIN pg_roles owner_role ON owner_role.oid = relation.relowner`
    // added to the census, every assertion in this test AND all 16 probes and
    // 14 mutations of tests/census_executes_against_postgres.rs stayed green
    // (134s against PostgreSQL 18.4). So this text assertion is kept.
    let census = marked_region(&block, "census statement")?;
    assert!(
        census.contains("has_table_privilege") && census.contains("AS census"),
        "the census-statement markers do not enclose the census, so every \
         assertion over that region would examine nothing"
    );
    for subsumed in [
        "aclexplode",
        "relacl",
        "attacl",
        "pg_auth_members",
        "rolsuper",
    ] {
        assert!(
            !block.contains(subsumed),
            "`{subsumed}` is subsumed by has_table_privilege/has_any_column_privilege \
             and must not be unioned back into the census"
        );
        assert!(
            !census.contains(subsumed),
            "`{subsumed}` must not be read inside the census statement"
        );
    }
    // The owner catalog may be read in exactly ONE place: the step-4 owner pin,
    // which carries its own markers so the position can be named. Scoping this
    // to the census REGION left a one-statement bypass — read `relowner` into a
    // variable above the BEGIN marker and join it inside the census — so the
    // assertion counts occurrences instead, which is total over position.
    // Reading ownership anywhere the census can reach puts OWNERSHIP behind the
    // census's ratchet, and that ratchet excludes a candidate by NAME before it
    // asks the privilege question: `console_rt` could then OWN `employees` and
    // pass, which is `ALTER TABLE … DISABLE ROW LEVEL SECURITY` on the runtime
    // login principal.
    let owner_pin = marked_region(&block, "owner pin")?;
    assert!(
        owner_pin.contains("relowner")
            && owner_pin.contains("expected_owners")
            && owner_pin.contains("topology.canonical_table_owner_failed"),
        "the owner-pin markers do not enclose the owner pin, so the `relowner` \
         count below would examine nothing"
    );
    assert_eq!(
        block.matches("relowner").count(),
        owner_pin.matches("relowner").count(),
        "`relowner` is read outside the step-4 owner pin; ownership read \
         anywhere the census can reach becomes subject to the write ratchet"
    );

    // The owner pin is a list of its own, so no expected-WRITER entry can widen
    // who may OWN.
    assert_eq!(
        marked_list(&block, "expected owners")?,
        ["console_app"],
        "migrations run as console_app, so it is the only expected owner of a \
         canonical relation; a role that may WRITE one may still not OWN it"
    );
    assert!(
        block.contains("topology.canonical_table_owner_failed"),
        "the owner pin must fail closed with a named error"
    );
    assert!(
        block.contains("has_table_privilege") && block.contains("has_any_column_privilege"),
        "the census must ASK PostgreSQL the privilege question"
    );
    assert!(
        block.contains("pg_inherits"),
        "the examined set must reach partition and inheritance children, whose \
         relname the roster does not contain"
    );
    assert!(
        block.contains("topology.canonical_writer_ownership_failed"),
        "the check must fail closed with a named error"
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// Totality: a seventh object key cannot skip the gate
// ---------------------------------------------------------------------------

/// Shape copied from `cedar_pbac/mode_contract.rs:36-46`: the roster the gate
/// loops over is derived from the single `ObjectKey::ALL` constant and its size
/// is asserted against `ALL.len()`, so a seventh key cannot make these loops
/// pass vacuously — it must appear here, with owned tables and an owner crate.
#[test]
fn gate_roster_covers_every_object_key() -> Result<(), Box<dyn std::error::Error>> {
    let roster = console_gate_writer_ownership::ownership_roster();
    assert_eq!(
        roster.len(),
        ObjectKey::ALL.len(),
        "every object key must contribute exactly one ownership rule: {roster:#?}"
    );
    for key in ObjectKey::ALL {
        let rule = roster
            .iter()
            .find(|r| r.object == *key)
            .ok_or_else(|| format!("{key:?} has no ownership rule"))?;
        assert!(
            !rule.tables.is_empty(),
            "{key:?} declares no owned table, so the gate would never look at it"
        );
        assert!(
            rule.owner_crate.starts_with("console-"),
            "{key:?} owner must be a workspace crate, got {}",
            rule.owner_crate
        );
    }
    let table_count: usize = roster.iter().map(|r| r.tables.len()).sum();
    let distinct: std::collections::BTreeSet<&str> = roster
        .iter()
        .flat_map(|r| r.tables.iter().copied())
        .collect();
    assert_eq!(
        distinct.len(),
        table_count,
        "a table may be owned by exactly one object key"
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// console-kmb: the Employment port took the write over
// ---------------------------------------------------------------------------

/// The lane marker for `EmploymentPort`. `measured_tip_has_exactly_the_ratcheted_dual_writers`
/// is self-BALANCING — it only asserts that the count of measured violations
/// equals the count of ratchet entries — so deleting the `employees`/`console-app`
/// entry AND leaving a different unratcheted writer of the same table behind
/// would keep it green. This one is not balanced: it names the crate and the
/// table, and it fails whether the entry comes back or the writer does.
///
/// Both halves, asserted separately:
///
///   1. `console-app` holds no production DML against any Employment table at
///      this tip — the writer really is gone, not merely un-ratcheted;
///   2. no ratchet entry names `employees` — the exemption really is gone, not
///      merely unused.
#[test]
fn console_app_no_longer_writes_the_employment_tables() -> Result<(), Box<dyn std::error::Error>> {
    let backend = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let report = scan(&backend)?;
    assert!(
        report.scanned_files > 100,
        "the walk must reach the crate tree, scanned only {}",
        report.scanned_files
    );

    let employment_tables = ObjectKey::Employment.owned_tables();
    let app_writes: Vec<_> = report
        .violations
        .iter()
        .filter(|violation| violation.offending_crate == "console-app")
        .collect();
    assert!(
        app_writes.is_empty(),
        "console-app must hold no production DML against a canonical table; the three \
         `employees` statements moved to console-ontology-canonical-adapter-postgres's \
         `src/employment.rs`: {app_writes:#?}"
    );

    let ratcheted: Vec<&str> = KNOWN_SECOND_WRITERS
        .iter()
        .filter(|known| employment_tables.contains(&known.table))
        .map(|known| known.table)
        .collect();
    assert!(
        ratcheted.is_empty(),
        "an Employment table is still ratcheted as a dual write, but the port owns it now: \
         {ratcheted:?}"
    );
    Ok(())
}

/// console-1qw.4 (L4-EMPL-RETARGET): the Employment owner is the canonical
/// adapter, not the interim orgchange adapter.
///
/// `console_app_no_longer_writes_the_employment_tables` proved `console-app`
/// stopped writing; this one pins the RETARGET itself. It names the owner
/// string and scans the tree, so it fails whether the roster is retyped
/// (`owner_crate` regresses) or the orgchange adapter regains production DML
/// against an Employment table (which, with the canonical adapter named as
/// owner, the gate now flags as a second writer).
#[test]
fn employment_is_owned_by_the_canonical_adapter() -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(
        ObjectKey::Employment.owner_crate(),
        "console-ontology-canonical-adapter-postgres",
        "the Employment owner must be the canonical adapter; the interim \
         console-orgchange-adapter-postgres owner is retargeted by console-1qw.4"
    );

    let backend = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let report = scan(&backend)?;
    assert!(
        report.scanned_files > 100,
        "the walk must reach the crate tree, scanned only {}",
        report.scanned_files
    );

    let employment_tables = ObjectKey::Employment.owned_tables();
    let orgchange_writes: Vec<_> = report
        .violations
        .iter()
        .filter(|violation| {
            violation.offending_crate == "console-orgchange-adapter-postgres"
                && employment_tables.contains(&violation.table.as_str())
        })
        .collect();
    assert!(
        orgchange_writes.is_empty(),
        "console-orgchange-adapter-postgres must hold no production DML against an \
         Employment table; the write path moved to the canonical adapter's \
         src/employment.rs: {orgchange_writes:#?}"
    );
    Ok(())
}

/// console-pees (P5 port-routing): the orgchange adapter must no longer
/// `#[path]`-compile the canonical adapter's `employment.rs`. That seam was the
/// interim way to reach `reassign_org_unit_via_transfers_in_tx` after the DML
/// relocated; the follow-up routes `ReassignOrgUnit` through an injected
/// Employment-transfer port, so the orgchange crate compiles no owning DML at
/// all. RED before the seam retires: the source still names the employment file
/// in its `#[path]` directive.
#[test]
fn orgchange_no_longer_compiles_the_employment_seam() -> Result<(), Box<dyn std::error::Error>> {
    let backend = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let lib = backend.join("crates/orgchange/adapter-postgres/src/lib.rs");
    let source = fs::read_to_string(&lib)?;
    assert!(
        !source.contains("canonical-adapter-postgres/src/employment.rs"),
        "console-orgchange-adapter-postgres must not #[path]-compile the canonical \
         employment module; ReassignOrgUnit routes through the injected \
         Employment-transfer port (console-pees)"
    );

    // The DML still has exactly one home: the canonical adapter's employment.rs.
    let owner = backend.join("crates/ontology/canonical-adapter-postgres/src/employment.rs");
    assert!(
        owner.is_file(),
        "the Employment DML must live in the canonical adapter, not be compiled \
         into the orgchange adapter"
    );
    Ok(())
}

/// console-0hq's half of the ratchet deletion, and the HEADLINE result of P4.
///
/// `measured_tip_has_exactly_the_ratcheted_dual_writers` is only a COUNT, and
/// with an empty ratchet it now asserts `0 == 0`: every one of its comparisons
/// against `KNOWN_SECOND_WRITERS.len()` is vacuous. This test is not a count. It
/// names the crate, the tables and the emptiness, and it fails whether the entry
/// comes back or the writer does.
///
/// Three things, asserted separately:
///
///   1. `console-workflow-runtime-adapter-postgres` holds no production DML
///      against any PayRun table at this tip — the `INSERT INTO
///      payroll_draft_runs` really is gone from the JOB outbox drain, not merely
///      un-ratcheted;
///   2. no ratchet entry names a PayRun table — the exemption really is gone;
///   3. the ratchet is EMPTY, so this holds for every canonical object, not just
///      this one. That is the property P4 set out to prove: each of the six
///      objects' tables has exactly one production writer crate, and it is the
///      crate `canonical-domain` names.
#[test]
fn console_workflow_no_longer_writes_the_pay_run_tables() -> Result<(), Box<dyn std::error::Error>>
{
    let backend = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let report = scan(&backend)?;
    assert!(
        report.scanned_files > 100,
        "the walk must reach the crate tree, scanned only {}",
        report.scanned_files
    );

    let pay_run_tables = ObjectKey::PayRun.owned_tables();
    let workflow_writes: Vec<_> = report
        .violations
        .iter()
        .filter(|violation| {
            violation.offending_crate == "console-workflow-runtime-adapter-postgres"
        })
        .collect();
    assert!(
        workflow_writes.is_empty(),
        "console-workflow-runtime-adapter-postgres must hold no production DML against a \
         canonical table; `drain_payroll_job_outbox`'s INSERT moved to \
         console-payroll-adapter-postgres's `src/pay_run.rs` and is reached through \
         console_workflow_domain::PayrollDraftStaging: {workflow_writes:#?}"
    );

    let ratcheted: Vec<&str> = KNOWN_SECOND_WRITERS
        .iter()
        .filter(|known| pay_run_tables.contains(&known.table))
        .map(|known| known.table)
        .collect();
    assert!(
        ratcheted.is_empty(),
        "a PayRun table is still ratcheted as a dual write, but the port owns it now: \
         {ratcheted:?}"
    );

    assert!(
        KNOWN_SECOND_WRITERS.is_empty(),
        "P4's result is an EMPTY ratchet — every canonical table has exactly one production \
         writer. A surviving entry means a lane is unfinished: {KNOWN_SECOND_WRITERS:#?}"
    );
    assert!(
        report.violations.is_empty(),
        "with an empty ratchet, ANY violation fails the gate: {:#?}",
        report.violations
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// SHARED AUTHORITY TABLE: the receipt store
//
// `ont_action_command_receipts` is the authority record for every dispatch
// target, so each object's owner writes ITS rows there. It has no single owner
// and therefore no entry in the writer-ownership registry — which meant, until
// this rule landed, that the gate never examined it at all. Measured before the
// fix: appending an `INSERT INTO ont_action_command_receipts` to
// `backend/app/src/hr.rs` left the suite at 48 passed / 0 failed, while the same
// probe against `employment_heads` turned three tests red.
//
// Collapsing the writers into one crate to reuse the single-owner rule was
// designed and REFUTED: retargeting `ObjectKey::PayRun.owner_crate` makes
// `scan`'s `owner_of` map constant across all twenty tables, so
// `**owner != crate_name` could never fire again for any cross-object pair. That
// is a false green, so the boundary here is a permitted SET.
// ---------------------------------------------------------------------------

use console_gate_writer_ownership::shared_authority_roster;
use console_ontology_canonical_domain::ReceiptOwner;

/// A crate outside the permitted set that writes the receipt store is charged.
#[test]
fn gate_detects_an_unpermitted_writer_of_the_shared_receipt_store()
-> Result<(), Box<dyn std::error::Error>> {
    let root = temp_tree("shared-unpermitted")?;
    crate_with_source(
        &root,
        "app",
        "console-app",
        "src/lib.rs",
        "pub async fn f(p: &sqlx::PgPool) {\n    \
         sqlx::query(\"INSERT INTO ont_action_command_receipts (org_id) VALUES ($1)\")\n        \
         .execute(p).await.unwrap();\n}\n",
    )?;
    let report = console_gate_writer_ownership::scan(&root)?;
    assert_eq!(
        report.shared_violations.len(),
        1,
        "expected exactly one shared-table violation: {:#?}",
        report.shared_violations
    );
    let violation = &report.shared_violations[0];
    assert_eq!(violation.offending_crate, "console-app");
    assert_eq!(violation.table, ReceiptOwner::RECEIPT_STORE);
    assert!(
        !report.passed(),
        "a shared-table violation must fail the gate"
    );
    Ok(())
}

/// Each permitted crate writes it freely, and is RECORDED as having written it.
#[test]
fn every_permitted_crate_may_write_the_shared_receipt_store()
-> Result<(), Box<dyn std::error::Error>> {
    for permitted in &shared_authority_roster()[0].permitted_crates {
        let root = temp_tree(&format!("shared-permitted-{permitted}"))?;
        crate_with_source(
            &root,
            "w",
            permitted,
            "src/lib.rs",
            "pub async fn f(p: &sqlx::PgPool) {\n    \
             sqlx::query(\"INSERT INTO ont_action_command_receipts (org_id) VALUES ($1)\")\n        \
             .execute(p).await.unwrap();\n}\n",
        )?;
        let report = console_gate_writer_ownership::scan(&root)?;
        assert!(
            report.shared_violations.is_empty(),
            "{permitted} is permitted and must not be charged: {:#?}",
            report.shared_violations
        );
        assert!(
            report
                .shared_writers
                .contains(&(ReceiptOwner::RECEIPT_STORE, (*permitted).to_owned())),
            "{permitted}'s write must be RECORDED, or stale_permitted_writers cannot ratchet"
        );
    }
    Ok(())
}

/// Reading the store is not writing it.
#[test]
fn selecting_from_the_shared_receipt_store_is_not_a_write() -> Result<(), Box<dyn std::error::Error>>
{
    let root = temp_tree("shared-select")?;
    crate_with_source(
        &root,
        "app",
        "console-app",
        "src/lib.rs",
        "pub async fn f(p: &sqlx::PgPool) {\n    \
         sqlx::query(\"SELECT receipt FROM ont_action_command_receipts WHERE org_id = $1\")\n        \
         .fetch_one(p).await.unwrap();\n}\n",
    )?;
    let report = console_gate_writer_ownership::scan(&root)?;
    assert!(
        report.shared_violations.is_empty(),
        "a read must not be charged: {:#?}",
        report.shared_violations
    );
    Ok(())
}

/// An unreadable write target that NAMES the store is still charged — the
/// `format!("INSERT INTO {t}")` evasion the owned-table pass already refuses.
#[test]
fn an_unresolved_write_naming_the_shared_store_is_still_charged()
-> Result<(), Box<dyn std::error::Error>> {
    let root = temp_tree("shared-unresolved")?;
    crate_with_source(
        &root,
        "app",
        "console-app",
        "src/lib.rs",
        "pub async fn f(p: &sqlx::PgPool, t: &str) {\n    \
         let _ = \"ont_action_command_receipts\";\n    \
         sqlx::query(&format!(\"INSERT INTO {t} (org_id) VALUES ($1)\"))\n        \
         .execute(p).await.unwrap();\n}\n",
    )?;
    let report = console_gate_writer_ownership::scan(&root)?;
    assert_eq!(
        report.shared_violations.len(),
        1,
        "an unresolved target naming the store must be charged: {:#?}",
        report.shared_violations
    );
    Ok(())
}

/// The permitted set is a RATCHET, not a growing allowance: a crate permitted to
/// write the store that holds no write fails the gate. Without this, declaring a
/// seventh object key would silently grant its owner DML on the receipt store.
#[test]
fn a_permitted_crate_that_writes_nothing_is_stale() -> Result<(), Box<dyn std::error::Error>> {
    let root = temp_tree("shared-stale-permission")?;
    // A tree where NOBODY writes the store: every permitted crate is stale.
    crate_with_source(&root, "x", "console-app", "src/lib.rs", "pub fn f() {}\n")?;
    let report = console_gate_writer_ownership::scan(&root)?;
    let stale = report.stale_permitted_writers();
    assert_eq!(
        stale.len(),
        shared_authority_roster()[0].permitted_crates.len(),
        "every permitted crate writes nothing here, so every one is stale: {stale:#?}"
    );
    Ok(())
}

/// The shared roster is DISJOINT from the owned-table registry, and that is
/// deliberate.
///
/// The executed PostgreSQL census in `ops/postgres-reconcile-topology.sh`
/// derives its roster from `ObjectKey::owned_tables()` and examines exactly
/// those twenty tables. The shared store is governed by the STATIC half only.
/// Pinning the disjointness makes that asymmetry a declared property rather than
/// a silent one: if anyone later adds the receipt store to an object's
/// `owned_tables`, this fails and forces the database half to be considered too.
#[test]
fn the_shared_roster_is_disjoint_from_the_owned_registry() {
    let owned: std::collections::BTreeSet<&str> = ObjectKey::ALL
        .iter()
        .flat_map(|key| key.owned_tables().iter().copied())
        .collect();
    for rule in shared_authority_roster() {
        assert!(
            !owned.contains(rule.table),
            "{} is in BOTH the shared roster and an object's owned_tables; the static gate would \
             charge it twice and the PostgreSQL census would silently disagree about its scope",
            rule.table
        );
    }
}

/// The permitted set is derived from `ReceiptOwner::ALL`, not hand-listed.
#[test]
fn the_permitted_set_is_derived_from_the_receipt_owner_roster() {
    let rule = &shared_authority_roster()[0];
    for owner in ReceiptOwner::ALL {
        assert!(
            rule.permitted_crates.contains(&owner.writer_crate()),
            "{:?}'s writer crate is not permitted to write the store it owns rows in",
            owner
        );
    }
    // Exactly the distinct writer crates, no extras.
    let mut expected: Vec<&str> = ReceiptOwner::ALL.iter().map(|o| o.writer_crate()).collect();
    expected.sort_unstable();
    expected.dedup();
    assert_eq!(rule.permitted_crates, expected);
}
