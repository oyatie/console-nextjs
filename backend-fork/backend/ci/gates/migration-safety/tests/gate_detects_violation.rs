use console_gate_migration_safety::{ViolationKind, check_migrations_root};
use std::fs;
use std::path::{Path, PathBuf};

fn temp_workspace(name: &str) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let dir = std::env::temp_dir().join(format!(
        "console-migration-gate-test-{name}-{}",
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

#[test]
fn gate_rejects_drop_table_on_audited_table() -> Result<(), Box<dyn std::error::Error>> {
    let ws = temp_workspace("drop-table")?;
    write_file(
        &ws.join("crates/platform/db/migrations/0001_bad.sql"),
        r#"
-- console-gate: audited-table work_orders
DROP TABLE IF EXISTS work_orders;
"#,
    )?;

    let result = check_migrations_root(&ws);
    assert!(!result.passed(), "expected DROP TABLE violation");
    assert!(
        result
            .violations
            .iter()
            .any(|v| v.kind == ViolationKind::DropAuditedTable),
        "expected DropAuditedTable, got {:#?}",
        result.violations
    );
    Ok(())
}

#[test]
fn gate_rejects_drop_table_on_built_in_audited_table_without_marker()
-> Result<(), Box<dyn std::error::Error>> {
    let ws = temp_workspace("drop-built-in-table")?;
    write_file(
        &ws.join("crates/platform/db/migrations/0001_bad.sql"),
        "DROP TABLE users;\n",
    )?;

    let result = check_migrations_root(&ws);
    assert!(
        !result.passed(),
        "expected built-in audited table violation"
    );
    assert!(
        result
            .violations
            .iter()
            .any(|v| v.kind == ViolationKind::DropAuditedTable),
        "expected DropAuditedTable, got {:#?}",
        result.violations
    );
    Ok(())
}

#[test]
fn gate_rejects_drop_column_on_audited_table() -> Result<(), Box<dyn std::error::Error>> {
    let ws = temp_workspace("drop-column")?;
    write_file(
        &ws.join("crates/platform/db/migrations/0001_bad.sql"),
        r#"
-- console-gate: audited-table work_orders
ALTER TABLE work_orders DROP COLUMN status;
"#,
    )?;

    let result = check_migrations_root(&ws);
    assert!(!result.passed(), "expected DROP COLUMN violation");
    assert!(
        result
            .violations
            .iter()
            .any(|v| v.kind == ViolationKind::DropAuditedColumn),
        "expected DropAuditedColumn, got {:#?}",
        result.violations
    );
    Ok(())
}

#[test]
fn gate_rejects_drop_column_on_only_audited_table() -> Result<(), Box<dyn std::error::Error>> {
    let ws = temp_workspace("drop-column-only")?;
    write_file(
        &ws.join("crates/platform/db/migrations/0001_bad.sql"),
        "ALTER TABLE ONLY audit_events DROP COLUMN payload;\n",
    )?;

    let result = check_migrations_root(&ws);
    assert!(
        result
            .violations
            .iter()
            .any(|v| v.kind == ViolationKind::DropAuditedColumn),
        "expected DropAuditedColumn, got {:#?}",
        result.violations
    );
    Ok(())
}

#[test]
fn gate_rejects_drop_column_on_schema_qualified_audited_table()
-> Result<(), Box<dyn std::error::Error>> {
    let ws = temp_workspace("drop-column-qualified")?;
    write_file(
        &ws.join("crates/platform/db/migrations/0001_bad.sql"),
        "ALTER TABLE IF EXISTS public.audit_events DROP COLUMN payload;\n",
    )?;

    let result = check_migrations_root(&ws);
    assert!(
        result
            .violations
            .iter()
            .any(|v| v.kind == ViolationKind::DropAuditedColumn),
        "expected DropAuditedColumn, got {:#?}",
        result.violations
    );
    Ok(())
}

#[test]
fn gate_rejects_drop_column_with_only_and_quoted_keyword_schema()
-> Result<(), Box<dyn std::error::Error>> {
    let ws = temp_workspace("drop-column-qualified-quoted")?;
    write_file(
        &ws.join("crates/platform/db/migrations/0001_bad.sql"),
        "ALTER TABLE IF EXISTS ONLY \"owner\".\"audit_events\" DROP COLUMN payload;\n",
    )?;

    let result = check_migrations_root(&ws);
    assert!(
        result
            .violations
            .iter()
            .any(|v| v.kind == ViolationKind::DropAuditedColumn),
        "expected DropAuditedColumn, got {:#?}",
        result.violations
    );
    Ok(())
}

#[test]
fn gate_rejects_drop_column_after_another_alter_action() -> Result<(), Box<dyn std::error::Error>> {
    let ws = temp_workspace("drop-column-after-add")?;
    write_file(
        &ws.join("crates/platform/db/migrations/0001_bad.sql"),
        "ALTER TABLE public.audit_events ADD COLUMN note text, DROP COLUMN payload;\n",
    )?;

    let result = check_migrations_root(&ws);
    assert!(
        result
            .violations
            .iter()
            .any(|v| v.kind == ViolationKind::DropAuditedColumn),
        "expected DropAuditedColumn, got {:#?}",
        result.violations
    );
    Ok(())
}

#[test]
fn gate_allows_drop_column_on_schema_qualified_non_audited_table()
-> Result<(), Box<dyn std::error::Error>> {
    let ws = temp_workspace("drop-column-qualified-safe")?;
    write_file(
        &ws.join("crates/platform/db/migrations/0001_safe.sql"),
        "ALTER TABLE ONLY scratch.transient_rows DROP COLUMN payload;\n",
    )?;

    let result = check_migrations_root(&ws);
    assert!(
        result.passed(),
        "unexpected violations: {:#?}",
        result.violations
    );
    Ok(())
}

#[test]
fn gate_rejects_update_or_delete_grants_on_audit_events() -> Result<(), Box<dyn std::error::Error>>
{
    let ws = temp_workspace("grant")?;
    write_file(
        &ws.join("crates/platform/db/migrations/0001_bad.sql"),
        "GRANT SELECT, UPDATE, DELETE ON TABLE audit_events TO app_user;\n",
    )?;

    let result = check_migrations_root(&ws);
    assert!(!result.passed(), "expected GRANT mutation violation");
    assert!(
        result
            .violations
            .iter()
            .any(|v| v.kind == ViolationKind::GrantAuditEventsMutation),
        "expected GrantAuditEventsMutation, got {:#?}",
        result.violations
    );
    Ok(())
}

#[test]
fn gate_rejects_disable_trigger_on_audit_events() -> Result<(), Box<dyn std::error::Error>> {
    let ws = temp_workspace("disable-trigger")?;
    write_file(
        &ws.join("crates/platform/db/migrations/0001_bad.sql"),
        "ALTER TABLE audit_events DISABLE TRIGGER trg_audit_events_no_update;\n",
    )?;

    let result = check_migrations_root(&ws);
    assert!(!result.passed(), "expected DISABLE TRIGGER violation");
    assert!(
        result
            .violations
            .iter()
            .any(|v| v.kind == ViolationKind::DisableAuditEventsTrigger),
        "expected DisableAuditEventsTrigger, got {:#?}",
        result.violations
    );
    Ok(())
}

#[test]
fn gate_rejects_duplicate_migration_versions() -> Result<(), Box<dyn std::error::Error>> {
    let ws = temp_workspace("duplicate-version")?;
    write_file(
        &ws.join("crates/platform/db/migrations/0001_first.sql"),
        "SELECT 1;\n",
    )?;
    write_file(
        &ws.join("crates/platform/db/migrations/0001_second.sql"),
        "SELECT 2;\n",
    )?;

    let result = check_migrations_root(&ws);
    assert!(
        !result.passed(),
        "expected duplicate migration version violation"
    );
    assert!(
        result
            .violations
            .iter()
            .any(|v| v.kind == ViolationKind::DuplicateMigrationVersion),
        "expected DuplicateMigrationVersion, got {:#?}",
        result.violations
    );
    Ok(())
}

#[test]
fn gate_rejects_non_contiguous_migration_versions() -> Result<(), Box<dyn std::error::Error>> {
    let ws = temp_workspace("non-contiguous")?;
    write_file(
        &ws.join("crates/platform/db/migrations/0001_first.sql"),
        "SELECT 1;\n",
    )?;
    write_file(
        &ws.join("crates/platform/db/migrations/0003_third.sql"),
        "SELECT 3;\n",
    )?;

    let result = check_migrations_root(&ws);
    assert!(
        !result.passed(),
        "expected non-contiguous migration version violation"
    );
    assert!(
        result
            .violations
            .iter()
            .any(|v| v.kind == ViolationKind::NonContiguousMigrationVersion),
        "expected NonContiguousMigrationVersion, got {:#?}",
        result.violations
    );
    Ok(())
}

#[test]
fn gate_rejects_concurrent_index_if_not_exists() -> Result<(), Box<dyn std::error::Error>> {
    let ws = temp_workspace("concurrent-index-if-not-exists")?;
    write_file(
        &ws.join("crates/platform/db/migrations/0001_bad.sql"),
        r#"
-- no-transaction
CREATE INDEX CONCURRENTLY IF NOT EXISTS work_orders_status_idx
    ON work_orders (status);
"#,
    )?;

    let result = check_migrations_root(&ws);
    assert!(
        !result.passed(),
        "expected concurrent index IF NOT EXISTS violation"
    );
    assert!(
        result
            .violations
            .iter()
            .any(|v| v.kind == ViolationKind::ConcurrentIndexIfNotExists),
        "expected ConcurrentIndexIfNotExists, got {:#?}",
        result.violations
    );
    Ok(())
}

#[test]
fn gate_rejects_multi_statement_no_transaction_migration() -> Result<(), Box<dyn std::error::Error>>
{
    let ws = temp_workspace("multi-statement-no-transaction")?;
    write_file(
        &ws.join("crates/platform/db/migrations/0001_bad.sql"),
        r#"-- no-transaction
CREATE INDEX CONCURRENTLY work_orders_status_idx
    ON work_orders (status);
CREATE INDEX CONCURRENTLY work_orders_created_idx
    ON work_orders (created_at DESC);
"#,
    )?;

    let result = check_migrations_root(&ws);
    assert!(
        !result.passed(),
        "expected multi-statement no-transaction violation"
    );
    assert!(
        result
            .violations
            .iter()
            .any(|v| v.kind == ViolationKind::NoTransactionMigrationMultipleStatements),
        "expected NoTransactionMigrationMultipleStatements, got {:#?}",
        result.violations
    );
    Ok(())
}

#[test]
fn gate_passes_safe_migration() -> Result<(), Box<dyn std::error::Error>> {
    let ws = temp_workspace("safe")?;
    write_file(
        &ws.join("crates/platform/db/migrations/0001_safe.sql"),
        r#"
CREATE TABLE work_orders (
    id UUID PRIMARY KEY,
    status TEXT NOT NULL
);
GRANT SELECT ON TABLE audit_events TO app_user;
"#,
    )?;

    let result = check_migrations_root(&ws);
    assert!(
        result.passed(),
        "expected safe migration to pass, got {:#?}",
        result.violations
    );
    Ok(())
}

/// A gate that examined NO migration must fail, not pass.
///
/// `check_files` reports violations, so over zero migrations it reports none and
/// the binary printed "PASSED" — measured by running it from an empty directory
/// before this floor existed. The workspace holds hundreds of migrations, so zero
/// means the scan did not find them (a moved directory, a wrong cwd), never that
/// they are all safe.
///
/// The repository already applies this rule where it matters:
/// `topology.canonical_enforcement` refuses to claim enforcement over zero tables
/// and `tools/ci/gate-sweep.mjs` refuses a manifest declaring zero gates. Payroll's
/// close preflight did not, and an empty roster silently satisfied it.
#[test]
fn examining_no_migration_is_refused_not_passed() -> Result<(), Box<dyn std::error::Error>> {
    let dir = temp_workspace("empty-subject-set")?;
    // No `panic!`: clippy forbids it here. An Ok result collapses to an empty
    // string, which fails the `contains` below with the result printed.
    let outcome = match console_gate_migration_safety::check_workspace(&dir) {
        Ok(_) => String::new(),
        Err(message) => message,
    };
    assert!(
        outcome.contains("examined no migration files"),
        "an empty workspace must be REFUSED, naming the empty subject set; got {outcome:?}"
    );
    Ok(())
}

/// The positive control: a workspace WITH a migration still passes.
///
/// Without this, the floor above could be satisfied by a gate that refuses
/// everything, which would block every migration change — worse than the hole.
#[test]
fn a_workspace_with_a_safe_migration_still_passes() -> Result<(), Box<dyn std::error::Error>> {
    let dir = temp_workspace("floor-positive-control")?;
    write_file(
        &dir.join("backend/crates/platform/db/migrations/0001_create_widgets.sql"),
        "CREATE TABLE widgets (id UUID PRIMARY KEY);\n",
    )?;
    let result = console_gate_migration_safety::check_workspace(&dir)?;
    assert!(
        result.violations.is_empty(),
        "a safe migration must not be charged: {:#?}",
        result.violations
    );
    Ok(())
}
