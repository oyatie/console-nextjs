//! Migration-safety gate.

use std::path::{Path, PathBuf};
use std::{
    collections::{BTreeMap, HashSet},
    fs,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ViolationKind {
    DuplicateMigrationVersion,
    NonContiguousMigrationVersion,
    ConcurrentIndexIfNotExists,
    NoTransactionMigrationMultipleStatements,
    DropAuditedTable,
    DropAuditedColumn,
    GrantAuditEventsMutation,
    DisableAuditEventsTrigger,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Violation {
    pub kind: ViolationKind,
    pub file: PathBuf,
    pub detail: String,
}

impl std::fmt::Display for Violation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "[{:?}] {}: {}",
            self.kind,
            self.file.display(),
            self.detail
        )
    }
}

#[derive(Debug, Default)]
pub struct GateResult {
    pub violations: Vec<Violation>,
}

impl GateResult {
    #[must_use]
    pub fn passed(&self) -> bool {
        self.violations.is_empty()
    }
}

pub fn check_workspace(workspace_dir: &Path) -> Result<GateResult, String> {
    let files = collect_migration_files(workspace_dir)?;
    // A gate that examined NO subject must fail, not pass. `check_files` reports
    // violations, so over zero migrations it reports none and the gate prints
    // PASSED -- measured: run from an empty directory this binary exits 0 saying
    // "console-gate-migration-safety: PASSED". The workspace holds hundreds of
    // migrations, so zero means the scan did not find them (a moved directory, a
    // wrong cwd), never that they are all safe.
    //
    // This is the rule the repository already applies elsewhere:
    // `topology.canonical_enforcement` refuses to claim enforcement over zero
    // tables, and `tools/ci/gate-sweep.mjs` refuses a manifest declaring zero
    // gates. Payroll's close preflight did NOT, and an empty roster silently
    // satisfied it.
    if files.is_empty() {
        return Err(format!(
            "examined no migration files under {} -- a gate that examines nothing \
             cannot report safety",
            workspace_dir.display()
        ));
    }
    Ok(check_files(files))
}

#[must_use]
pub fn check_migrations_root(root: &Path) -> GateResult {
    match collect_migration_files(root) {
        Ok(files) => check_files(files),
        Err(e) => GateResult {
            violations: vec![Violation {
                kind: ViolationKind::DropAuditedTable,
                file: root.to_path_buf(),
                detail: e,
            }],
        },
    }
}

fn check_files(files: Vec<PathBuf>) -> GateResult {
    let mut audited_tables: HashSet<String> = built_in_audited_tables()
        .iter()
        .map(|name| name.to_string())
        .collect();
    let mut readable_files = Vec::new();
    let mut result = GateResult::default();

    check_migration_versions(&files, &mut result);

    for file in files {
        match fs::read_to_string(&file) {
            Ok(content) => {
                discover_audited_tables(&content, &mut audited_tables);
                readable_files.push((file, content));
            }
            Err(e) => result.violations.push(Violation {
                kind: ViolationKind::DropAuditedTable,
                file,
                detail: format!("cannot read migration file: {e}"),
            }),
        }
    }

    for (file, content) in readable_files {
        check_migration_file(&file, &content, &audited_tables, &mut result);
    }

    result
}

fn check_migration_versions(files: &[PathBuf], result: &mut GateResult) {
    let mut by_root: BTreeMap<PathBuf, BTreeMap<u32, Vec<PathBuf>>> = BTreeMap::new();

    for file in files {
        let Some(version) = migration_version(file) else {
            continue;
        };
        let root = file.parent().unwrap_or_else(|| Path::new("")).to_path_buf();
        by_root
            .entry(root)
            .or_default()
            .entry(version)
            .or_default()
            .push(file.clone());
    }

    for (root, versions) in by_root {
        for (version, files) in &versions {
            if files.len() > 1 {
                result.violations.push(Violation {
                    kind: ViolationKind::DuplicateMigrationVersion,
                    file: files[0].clone(),
                    detail: format!(
                        "migration version {version:04} is used more than once: {}",
                        display_file_list(files)
                    ),
                });
            }
        }

        let mut expected = 1u32;
        for version in versions.keys().copied() {
            while expected < version {
                result.violations.push(Violation {
                    kind: ViolationKind::NonContiguousMigrationVersion,
                    file: root.clone(),
                    detail: format!("missing migration version {expected:04} before {version:04}"),
                });
                expected += 1;
            }
            expected = version.saturating_add(1);
        }
    }
}

fn migration_version(file: &Path) -> Option<u32> {
    let name = file.file_name()?.to_str()?;
    let (prefix, _) = name.split_once('_')?;
    if prefix.len() != 4 || !prefix.chars().all(|ch| ch.is_ascii_digit()) {
        return None;
    }
    prefix.parse().ok()
}

fn display_file_list(files: &[PathBuf]) -> String {
    files
        .iter()
        .map(|file| file.display().to_string())
        .collect::<Vec<_>>()
        .join(", ")
}

fn built_in_audited_tables() -> &'static [&'static str] {
    &[
        "audit_events",
        "regions",
        "branches",
        "users",
        "user_branches",
    ]
}

fn discover_audited_tables(content: &str, audited_tables: &mut HashSet<String>) {
    for line in content.lines() {
        let trimmed = line.trim_start();
        if !trimmed.starts_with("--") {
            continue;
        }
        if let Some((_prefix, table)) = trimmed.split_once("console-gate: audited-table") {
            let table = normalize_identifier(table.trim());
            if !table.is_empty() {
                audited_tables.insert(table);
            }
        }
    }
}

fn check_migration_file(
    file: &Path,
    content: &str,
    audited_tables: &HashSet<String>,
    result: &mut GateResult,
) {
    let sanitized = sanitize_sql(content);
    let statements = sanitized
        .split(';')
        .filter_map(|statement| {
            let tokens = tokenize_sql(statement);
            (!tokens.is_empty()).then_some((statement, tokens))
        })
        .collect::<Vec<_>>();

    check_no_transaction_statement_count(file, content, statements.len(), result);

    for (statement, tokens) in &statements {
        check_drop_table(file, tokens, audited_tables, result);
        check_drop_column(file, statement, audited_tables, result);
        check_concurrent_index_if_not_exists(file, tokens, result);
        check_audit_events_grants(file, tokens, result);
        check_audit_events_trigger(file, tokens, result);
    }
}

fn check_no_transaction_statement_count(
    file: &Path,
    content: &str,
    statement_count: usize,
    result: &mut GateResult,
) {
    if content.starts_with("-- no-transaction") && statement_count > 1 {
        result.violations.push(Violation {
            kind: ViolationKind::NoTransactionMigrationMultipleStatements,
            file: file.to_path_buf(),
            detail: "-- no-transaction migrations must contain exactly one SQL statement; PostgreSQL treats multi-statement simple-query messages as a transaction block, which rejects CREATE INDEX CONCURRENTLY"
                .to_string(),
        });
    }
}

fn check_concurrent_index_if_not_exists(file: &Path, tokens: &[String], result: &mut GateResult) {
    for (index, token) in tokens.iter().enumerate() {
        if token != "create" {
            continue;
        }

        let mut cursor = index + 1;
        if tokens.get(cursor).is_some_and(|token| token == "unique") {
            cursor += 1;
        }

        let uses_concurrent_if_not_exists =
            tokens.get(cursor).is_some_and(|token| token == "index")
                && tokens
                    .get(cursor + 1)
                    .is_some_and(|token| token == "concurrently")
                && tokens.get(cursor + 2).is_some_and(|token| token == "if")
                && tokens.get(cursor + 3).is_some_and(|token| token == "not")
                && tokens
                    .get(cursor + 4)
                    .is_some_and(|token| token == "exists");

        if uses_concurrent_if_not_exists {
            result.violations.push(Violation {
                kind: ViolationKind::ConcurrentIndexIfNotExists,
                file: file.to_path_buf(),
                detail: "CREATE INDEX CONCURRENTLY IF NOT EXISTS can mask INVALID indexes left by a failed concurrent build; omit IF NOT EXISTS so reruns fail closed"
                    .to_string(),
            });
        }
    }
}

fn check_drop_table(
    file: &Path,
    tokens: &[String],
    audited_tables: &HashSet<String>,
    result: &mut GateResult,
) {
    for (index, token) in tokens.iter().enumerate() {
        if token != "drop" || tokens.get(index + 1).is_none_or(|next| next != "table") {
            continue;
        }
        for candidate in tokens.iter().skip(index + 2) {
            if ["if", "exists", "cascade", "restrict"].contains(&candidate.as_str()) {
                continue;
            }
            if audited_tables.contains(candidate) {
                result.violations.push(Violation {
                    kind: ViolationKind::DropAuditedTable,
                    file: file.to_path_buf(),
                    detail: format!("DROP TABLE touches audited table '{candidate}'"),
                });
            }
        }
    }
}

fn check_drop_column(
    file: &Path,
    statement: &str,
    audited_tables: &HashSet<String>,
    result: &mut GateResult,
) {
    let tokens = tokenize_table_reference_sql(statement);
    for (index, token) in tokens.iter().enumerate() {
        if token != "alter" || tokens.get(index + 1).is_none_or(|next| next != "table") {
            continue;
        }
        let Some((table, action_start)) = table_name_after_alter_table(&tokens, index + 2) else {
            continue;
        };
        let drops_column = tokens[action_start..]
            .windows(2)
            .any(|window| window[0] == "drop" && window[1] == "column");
        if drops_column && audited_tables.contains(table) {
            result.violations.push(Violation {
                kind: ViolationKind::DropAuditedColumn,
                file: file.to_path_buf(),
                detail: format!("ALTER TABLE DROP COLUMN touches audited table '{table}'"),
            });
        }
    }
}

fn table_name_after_alter_table(tokens: &[String], start: usize) -> Option<(&str, usize)> {
    let mut index = start;
    if tokens.get(index).is_some_and(|token| token == "if")
        && tokens.get(index + 1).is_some_and(|token| token == "exists")
    {
        index += 2;
    }
    if tokens.get(index).is_some_and(|token| token == "only") {
        index += 1;
    }

    let mut table = tokens.get(index)?.as_str();
    if table == "." {
        return None;
    }
    index += 1;

    while tokens.get(index).is_some_and(|token| token == ".") {
        table = tokens.get(index + 1)?.as_str();
        if table == "." {
            return None;
        }
        index += 2;
    }

    Some((table, index))
}

fn check_audit_events_grants(file: &Path, tokens: &[String], result: &mut GateResult) {
    if !tokens.iter().any(|token| token == "grant")
        || !tokens.iter().any(|token| token == "audit_events")
    {
        return;
    }

    let on_index = tokens.iter().position(|token| token == "on");
    let grants_mutation = tokens
        .iter()
        .take(on_index.unwrap_or(tokens.len()))
        .any(|token| token == "update" || token == "delete");

    if grants_mutation {
        result.violations.push(Violation {
            kind: ViolationKind::GrantAuditEventsMutation,
            file: file.to_path_buf(),
            detail: "GRANT UPDATE/DELETE on audit_events is forbidden".to_string(),
        });
    }
}

fn check_audit_events_trigger(file: &Path, tokens: &[String], result: &mut GateResult) {
    let disables_trigger = tokens
        .windows(2)
        .any(|window| window[0].as_str() == "disable" && window[1].as_str() == "trigger");
    if disables_trigger && tokens.iter().any(|token| token == "audit_events") {
        result.violations.push(Violation {
            kind: ViolationKind::DisableAuditEventsTrigger,
            file: file.to_path_buf(),
            detail: "DISABLE TRIGGER on audit_events is forbidden".to_string(),
        });
    }
}

fn sanitize_sql(content: &str) -> String {
    let bytes = content.as_bytes();
    let mut output = String::with_capacity(content.len());
    let mut index = 0usize;
    let mut in_line_comment = false;
    let mut in_block_comment = false;
    let mut in_single_quote = false;
    let mut in_double_quote = false;

    while index < bytes.len() {
        let b = bytes[index];
        let next = bytes.get(index + 1).copied();

        if in_line_comment {
            if b == b'\n' {
                in_line_comment = false;
                output.push('\n');
            } else {
                output.push(' ');
            }
            index += 1;
            continue;
        }
        if in_block_comment {
            if b == b'*' && next == Some(b'/') {
                in_block_comment = false;
                output.push_str("  ");
                index += 2;
            } else {
                output.push(if b == b'\n' { '\n' } else { ' ' });
                index += 1;
            }
            continue;
        }
        if in_single_quote {
            if b == b'\'' && next == Some(b'\'') {
                output.push_str("  ");
                index += 2;
            } else if b == b'\'' {
                in_single_quote = false;
                output.push(' ');
                index += 1;
            } else {
                output.push(if b == b'\n' { '\n' } else { ' ' });
                index += 1;
            }
            continue;
        }
        if in_double_quote {
            if b == b'"' {
                in_double_quote = false;
                output.push(' ');
            } else {
                output.push((b as char).to_ascii_lowercase());
            }
            index += 1;
            continue;
        }

        if b == b'-' && next == Some(b'-') {
            in_line_comment = true;
            output.push_str("  ");
            index += 2;
        } else if b == b'/' && next == Some(b'*') {
            in_block_comment = true;
            output.push_str("  ");
            index += 2;
        } else if b == b'\'' {
            in_single_quote = true;
            output.push(' ');
            index += 1;
        } else if b == b'"' {
            in_double_quote = true;
            output.push(' ');
            index += 1;
        } else {
            output.push((b as char).to_ascii_lowercase());
            index += 1;
        }
    }

    output
}

fn tokenize_sql(statement: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();

    for ch in statement.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' {
            current.push(ch);
        } else if !current.is_empty() {
            tokens.push(normalize_identifier(&current));
            current.clear();
        }
    }
    if !current.is_empty() {
        tokens.push(normalize_identifier(&current));
    }

    tokens
}

fn tokenize_table_reference_sql(statement: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();

    for ch in statement.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' {
            current.push(ch);
            continue;
        }

        if !current.is_empty() {
            tokens.push(normalize_identifier(&current));
            current.clear();
        }
        if ch == '.' {
            tokens.push(".".to_string());
        }
    }
    if !current.is_empty() {
        tokens.push(normalize_identifier(&current));
    }

    tokens
}

fn normalize_identifier(identifier: &str) -> String {
    identifier
        .trim_matches(|ch: char| !ch.is_ascii_alphanumeric() && ch != '_')
        .rsplit('.')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase()
}

fn collect_migration_files(root: &Path) -> Result<Vec<PathBuf>, String> {
    let mut files = Vec::new();
    collect_migration_files_inner(root, false, &mut files)?;
    Ok(files)
}

fn collect_migration_files_inner(
    dir: &Path,
    in_migrations_dir: bool,
    files: &mut Vec<PathBuf>,
) -> Result<(), String> {
    if should_skip_dir(dir) {
        return Ok(());
    }

    let entries =
        fs::read_dir(dir).map_err(|e| format!("cannot read directory {}: {e}", dir.display()))?;
    for entry in entries {
        let entry =
            entry.map_err(|e| format!("cannot read directory entry in {}: {e}", dir.display()))?;
        let path = entry.path();
        let file_type = entry
            .file_type()
            .map_err(|e| format!("cannot read file type for {}: {e}", path.display()))?;
        if file_type.is_dir() {
            let is_migrations = entry.file_name().to_string_lossy() == "migrations";
            collect_migration_files_inner(&path, in_migrations_dir || is_migrations, files)?;
        } else if file_type.is_file()
            && in_migrations_dir
            && path.extension().is_some_and(|ext| ext == "sql")
        {
            files.push(path);
        }
    }
    Ok(())
}

fn should_skip_dir(path: &Path) -> bool {
    path.components().any(|component| {
        let part = component.as_os_str().to_string_lossy();
        part == "target" || part == ".git"
    })
}
