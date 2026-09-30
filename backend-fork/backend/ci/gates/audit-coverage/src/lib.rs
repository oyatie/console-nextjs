//! Audit-coverage gate.
//!
//! Source files mark state-changing handlers with:
//!
//! ```text
//! // console-gate: state-changing-handler
//! ```
//!
//! Such handlers must construct an `AuditEvent` and route the mutation through
//! `with_audit`. There are three allowed carve-outs, and
//! `allowed_audit_exclusions()` below is the authority: `record_location_ping`,
//! `purge_expired_location_data`, and `insert_route_telemetry`. Raw coordinates
//! must remain destructible and must never enter audit_events; the retention
//! purge that ERASES them cannot itself write a row naming what it erased; and
//! high-volume RUM/route-selection telemetry must not flood the durable business
//! audit trail.
//!
//! Handler surfaces include path components `application` | `rest` | `worker` |
//! `app` (so `backend/app/src` is scanned — console-937 / gh#396), plus the
//! bound compliance LocationPing writer.

use std::path::{Path, PathBuf};
use std::{collections::HashMap, fs};

/// An allowed audit carve-out, bound to the exact writer it covers.
///
/// The exemption is keyed on the repo-relative source file AND the function
/// name, not on the reason string alone. Binding to the real writer is what
/// enforces ADR-0014's "exactly one path" invariant: the carve-out cannot
/// silently apply to a different handler that merely reuses the reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AuditExclusion {
    pub reason: &'static str,
    /// Repo-relative path of the file that owns the exempt writer, e.g.
    /// `crates/compliance/adapter-postgres/src/lib.rs`.
    pub file: &'static str,
    /// Name of the exempt function, e.g. `record_location_ping`.
    pub function: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ViolationKind {
    MissingAuditEvent,
    UnknownAuditExclusion,
    DuplicateAuditExclusion,
    ExemptionWithoutStateChangingHandler,
    /// A known exemption reason was used on a file/function that is not the
    /// writer it is bound to.
    MisboundAuditExclusion,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Violation {
    pub kind: ViolationKind,
    pub file: PathBuf,
    pub function_name: Option<String>,
    pub detail: String,
}

impl std::fmt::Display for Violation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let function = self
            .function_name
            .as_ref()
            .map(|name| format!("::{name}"))
            .unwrap_or_default();
        write!(
            f,
            "[{:?}] {}{}: {}",
            self.kind,
            self.file.display(),
            function,
            self.detail
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObservedExclusion {
    pub reason: String,
    pub file: PathBuf,
    pub function_name: String,
}

#[derive(Debug, Default)]
pub struct GateResult {
    pub violations: Vec<Violation>,
    pub observed_exclusions: Vec<ObservedExclusion>,
}

impl GateResult {
    #[must_use]
    pub fn passed(&self) -> bool {
        self.violations.is_empty()
    }
}

#[must_use]
pub fn allowed_audit_exclusions() -> &'static [AuditExclusion] {
    &[
        AuditExclusion {
            reason: "location_ping_ingestion",
            file: "crates/compliance/adapter-postgres/src/lib.rs",
            function: "record_location_ping",
        },
        // The retention purge ERASES expired location-derived data (ping
        // partitions, collection logs, geofence presence) to honour the
        // retention window; it is data-lifecycle maintenance, not an auditable
        // business write, and never touches the durable attendance facts.
        AuditExclusion {
            reason: "location_data_retention_purge",
            file: "crates/compliance/adapter-postgres/src/lib.rs",
            function: "purge_expired_location_data",
        },
        // Cardinality-safe console RUM / route-selection telemetry. High-volume
        // instrumentation must not flood `audit_events`; the writer is bound to
        // the single app ingestion helper (console-937 / gh#396 expanded app
        // surfaces into this gate).
        AuditExclusion {
            reason: "console_route_telemetry_ingestion",
            file: "app/src/console_telemetry.rs",
            function: "insert_route_telemetry",
        },
    ]
}

pub fn check_workspace(workspace_dir: &Path) -> Result<GateResult, String> {
    let app_src = workspace_dir.join("app").join("src");
    if !app_src.is_dir() {
        return Err(format!(
            "audit-coverage: expected app/src scan root missing at {}",
            app_src.display()
        ));
    }
    let files = collect_rust_files(workspace_dir)?;
    let app_files = files
        .iter()
        .filter(|path| path_contains_components(path, &["app", "src"]))
        .count();
    if app_files == 0 {
        return Err("audit-coverage: examined zero app/src Rust files (fail closed)".into());
    }
    Ok(check_files(files))
}

#[must_use]
pub fn check_source_tree(root: &Path) -> GateResult {
    match collect_rust_files(root) {
        Ok(files) => check_files(files),
        Err(e) => GateResult {
            violations: vec![Violation {
                kind: ViolationKind::MissingAuditEvent,
                file: root.to_path_buf(),
                function_name: None,
                detail: e,
            }],
            observed_exclusions: Vec::new(),
        },
    }
}

fn check_files(files: Vec<PathBuf>) -> GateResult {
    let mut result = GateResult::default();
    let mut exclusion_counts: HashMap<String, usize> = HashMap::new();

    for file in files {
        let Ok(source) = fs::read_to_string(&file) else {
            result.violations.push(Violation {
                kind: ViolationKind::MissingAuditEvent,
                file,
                function_name: None,
                detail: "cannot read Rust source file".to_string(),
            });
            continue;
        };
        check_source_file(&file, &source, &mut result, &mut exclusion_counts);
    }

    result
}

fn check_source_file(
    file: &Path,
    source: &str,
    result: &mut GateResult,
    exclusion_counts: &mut HashMap<String, usize>,
) {
    let lines: Vec<&str> = source.lines().collect();
    let in_test = compute_test_mask(&lines);
    let mut pending_state_changing = false;
    let mut pending_exemption: Option<String> = None;
    let mut byte_offset = 0usize;
    let mut line_idx = 0usize;

    for line in source.lines() {
        if in_test.get(line_idx).copied().unwrap_or(false) {
            pending_state_changing = false;
            pending_exemption = None;
            byte_offset += line.len() + 1;
            line_idx += 1;
            continue;
        }

        let trimmed = line.trim_start();
        if trimmed.starts_with("//") && trimmed.contains("console-gate: state-changing-handler") {
            pending_state_changing = true;
        }
        if trimmed.starts_with("//")
            && let Some(reason) = parse_exemption_reason(trimmed)
        {
            pending_exemption = Some(reason);
        }

        if let Some(function_name) = parse_function_name(line) {
            let signature = extract_function_signature(source, byte_offset).unwrap_or_default();
            let body = extract_function_body(source, byte_offset).unwrap_or_default();
            let is_state_changing = pending_state_changing
                || (!accepts_transaction(&signature)
                    && detects_unmarked_state_changing_handler(file, &body));
            let exemption = pending_exemption.take();

            if let Some(reason) = exemption {
                if !is_state_changing {
                    result.violations.push(Violation {
                        kind: ViolationKind::ExemptionWithoutStateChangingHandler,
                        file: file.to_path_buf(),
                        function_name: Some(function_name.clone()),
                        detail: format!(
                            "audit exemption '{reason}' is attached to a handler without the state-changing marker"
                        ),
                    });
                } else if let Some(exclusion) = allowed_exclusion_for_reason(&reason) {
                    if exclusion_matches_site(exclusion, file, &function_name) {
                        let count = exclusion_counts.entry(reason.clone()).or_insert(0);
                        if *count > 0 {
                            result.violations.push(Violation {
                                kind: ViolationKind::DuplicateAuditExclusion,
                                file: file.to_path_buf(),
                                function_name: Some(function_name.clone()),
                                detail: format!(
                                    "audit exemption '{reason}' was already used; only one LocationPing ingestion path is allowed"
                                ),
                            });
                        }
                        *count += 1;
                        result.observed_exclusions.push(ObservedExclusion {
                            reason,
                            file: file.to_path_buf(),
                            function_name: function_name.clone(),
                        });
                    } else {
                        result.violations.push(Violation {
                            kind: ViolationKind::MisboundAuditExclusion,
                            file: file.to_path_buf(),
                            function_name: Some(function_name.clone()),
                            detail: format!(
                                "audit exemption '{reason}' is bound to {}::{} and may not be applied here",
                                exclusion.file, exclusion.function
                            ),
                        });
                    }
                } else {
                    result.violations.push(Violation {
                        kind: ViolationKind::UnknownAuditExclusion,
                        file: file.to_path_buf(),
                        function_name: Some(function_name.clone()),
                        detail: format!(
                            "audit exemption '{reason}' is not in the hard-coded allowed set"
                        ),
                    });
                }
            } else if is_state_changing && !has_audit_emission(&body) {
                result.violations.push(Violation {
                    kind: ViolationKind::MissingAuditEvent,
                    file: file.to_path_buf(),
                    function_name: Some(function_name.clone()),
                    detail: "state-changing handler must construct AuditEvent and call with_audit"
                        .to_string(),
                });
            }

            pending_state_changing = false;
        }

        byte_offset += line.len() + 1;
        line_idx += 1;
    }
}

fn parse_exemption_reason(line: &str) -> Option<String> {
    let (_prefix, reason) = line.split_once("console-gate: audit-exempt")?;
    let reason = reason.trim();
    if reason.is_empty() {
        None
    } else {
        Some(reason.to_string())
    }
}

fn allowed_exclusion_for_reason(reason: &str) -> Option<&'static AuditExclusion> {
    allowed_audit_exclusions()
        .iter()
        .find(|exclusion| exclusion.reason == reason)
}

/// An observed exemption site matches a bound exclusion when the function name
/// is identical and the source file ends with the exclusion's repo-relative
/// path. Suffix matching keeps the binding stable regardless of the absolute
/// workspace root the gate is invoked from, while still pinning the exemption to
/// one specific writer.
fn exclusion_matches_site(exclusion: &AuditExclusion, file: &Path, function_name: &str) -> bool {
    if exclusion.function != function_name {
        return false;
    }
    path_ends_with_repo_relative(file, exclusion.file)
}

fn path_ends_with_repo_relative(file: &Path, repo_relative: &str) -> bool {
    let expected: Vec<&str> = repo_relative.split('/').filter(|s| !s.is_empty()).collect();
    let actual: Vec<String> = file
        .components()
        .map(|component| component.as_os_str().to_string_lossy().into_owned())
        .collect();
    if expected.len() > actual.len() {
        return false;
    }
    actual[actual.len() - expected.len()..]
        .iter()
        .zip(expected.iter())
        .all(|(actual_part, expected_part)| actual_part == expected_part)
}

fn parse_function_name(line: &str) -> Option<String> {
    let bytes = line.as_bytes();
    let mut index = 0usize;
    while index + 2 <= bytes.len() {
        if bytes[index] == b'f'
            && bytes.get(index + 1) == Some(&b'n')
            && is_word_boundary(bytes.get(index.wrapping_sub(1)).copied())
            && bytes.get(index + 2).is_some_and(u8::is_ascii_whitespace)
        {
            let mut name_start = index + 3;
            while bytes.get(name_start).is_some_and(u8::is_ascii_whitespace) {
                name_start += 1;
            }
            let mut name_end = name_start;
            while bytes.get(name_end).is_some_and(is_ident_byte) {
                name_end += 1;
            }
            if name_end > name_start {
                return Some(line[name_start..name_end].to_string());
            }
        }
        index += 1;
    }
    None
}

fn is_word_boundary(byte: Option<u8>) -> bool {
    match byte {
        None => true,
        Some(b) => !is_ident_byte(&b),
    }
}

fn is_ident_byte(byte: &u8) -> bool {
    byte.is_ascii_alphanumeric() || *byte == b'_'
}

fn extract_function_body(source: &str, from: usize) -> Option<String> {
    let rest = source.get(from..)?;
    let open_rel = rest.find('{')?;
    let open = from + open_rel;
    let close = find_matching_brace(source, open)?;
    source.get(open..=close).map(str::to_string)
}

fn extract_function_signature(source: &str, from: usize) -> Option<String> {
    let rest = source.get(from..)?;
    let open_rel = rest.find('{')?;
    rest.get(..open_rel).map(str::to_string)
}

fn accepts_transaction(signature: &str) -> bool {
    let compact: String = signature
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect();
    compact.contains("&mutTransaction<") || compact.contains("&mutsqlx::Transaction<")
}

fn find_matching_brace(source: &str, open: usize) -> Option<usize> {
    let bytes = source.as_bytes();
    let mut index = open;
    let mut depth = 0usize;
    let mut in_line_comment = false;
    let mut in_block_comment = false;
    let mut in_string = false;
    let mut in_char = false;
    let mut escaped = false;

    while index < bytes.len() {
        let b = bytes[index];
        let next = bytes.get(index + 1).copied();

        if in_line_comment {
            if b == b'\n' {
                in_line_comment = false;
            }
            index += 1;
            continue;
        }
        if in_block_comment {
            if b == b'*' && next == Some(b'/') {
                in_block_comment = false;
                index += 2;
            } else {
                index += 1;
            }
            continue;
        }
        if in_string {
            if escaped {
                escaped = false;
            } else if b == b'\\' {
                escaped = true;
            } else if b == b'"' {
                in_string = false;
            }
            index += 1;
            continue;
        }
        if in_char {
            if escaped {
                escaped = false;
            } else if b == b'\\' {
                escaped = true;
            } else if b == b'\'' {
                in_char = false;
            }
            index += 1;
            continue;
        }

        if b == b'/' && next == Some(b'/') {
            in_line_comment = true;
            index += 2;
            continue;
        }
        if b == b'/' && next == Some(b'*') {
            in_block_comment = true;
            index += 2;
            continue;
        }
        if b == b'"' {
            in_string = true;
            index += 1;
            continue;
        }
        if b == b'\'' {
            in_char = true;
            index += 1;
            continue;
        }
        if b == b'{' {
            depth += 1;
        } else if b == b'}' {
            depth = depth.saturating_sub(1);
            if depth == 0 {
                return Some(index);
            }
        }
        index += 1;
    }
    None
}

fn has_audit_emission(body: &str) -> bool {
    let executable = strip_comments_and_strings(body);
    // Routing through a transactional audit wrapper is what structurally
    // guarantees an `audit_events` row in the same transaction as the mutation.
    // Keying on the mechanism (rather than the `AuditEvent` type name) also
    // covers handlers that build the event via a helper, e.g.
    // `consent_audit_event`, where the literal type never appears in the body.
    executable.contains("with_audit")
        || executable.contains("with_audits")
        || executable.contains("insert_audit_event")
}

fn detects_unmarked_state_changing_handler(file: &Path, body: &str) -> bool {
    is_handler_surface(file) && contains_state_mutating_sql(body)
}

fn is_handler_surface(file: &Path) -> bool {
    // `app` covers `backend/app/src/**` composition-root handlers/pollers that
    // talk to Postgres directly (collaboration, workflow studio, HR, …). This is
    // a path-component mechanism, not a one-file allowlist.
    file.components().any(|component| {
        let part = component.as_os_str().to_string_lossy();
        matches!(part.as_ref(), "application" | "rest" | "worker" | "app")
    }) || is_compliance_location_ping_writer_surface(file)
}

/// The real LocationPing writer lives in the compliance Postgres adapter, not in
/// an `application`/`rest`/`worker`/`app` crate. We scan that adapter as a handler
/// surface so the writer is detected as state-changing and must carry the bound
/// audit exemption — closing the gap where the actual writer escaped coverage.
fn is_compliance_location_ping_writer_surface(file: &Path) -> bool {
    path_ends_with_repo_relative(file, "crates/compliance/adapter-postgres/src/lib.rs")
}

fn path_contains_components(path: &Path, needle: &[&str]) -> bool {
    let parts: Vec<String> = path
        .components()
        .map(|component| component.as_os_str().to_string_lossy().into_owned())
        .collect();
    parts
        .windows(needle.len())
        .any(|window| window.iter().zip(needle.iter()).all(|(a, b)| a == b))
}

/// Mask `cfg(test)` / `cfg(all(test, …))` attribute modules so inline fixture SQL
/// under `app/src/**` does not false-positive as unaudited handlers.
fn compute_test_mask(lines: &[&str]) -> Vec<bool> {
    let mut mask = vec![false; lines.len()];
    let mut i = 0;
    while i < lines.len() {
        let l = lines[i].trim_start();
        if is_cfg_test_attr(l) {
            let mut j = i;
            while j < lines.len() && !lines[j].contains('{') {
                j += 1;
            }
            if j < lines.len() {
                let mut depth = 0i32;
                let mut k = j;
                loop {
                    if k >= lines.len() {
                        break;
                    }
                    for ch in lines[k].chars() {
                        if ch == '{' {
                            depth += 1;
                        } else if ch == '}' {
                            depth -= 1;
                        }
                    }
                    mask[k] = true;
                    if depth <= 0 {
                        break;
                    }
                    k += 1;
                }
                for masked in mask.iter_mut().take(k + 1).skip(i) {
                    *masked = true;
                }
                i = k + 1;
                continue;
            }
        }
        i += 1;
    }
    mask
}

fn is_cfg_test_attr(line: &str) -> bool {
    let trimmed = line.trim_start();
    if !trimmed.starts_with("#[cfg(") {
        return false;
    }
    let bytes = trimmed.as_bytes();
    let needle = b"test";
    let mut idx = 0usize;
    while idx + needle.len() <= bytes.len() {
        if &bytes[idx..idx + needle.len()] == needle {
            let before = idx.checked_sub(1).map(|i| bytes[i]);
            let after = bytes.get(idx + needle.len()).copied();
            let boundary_before = matches!(
                before,
                None | Some(b'(') | Some(b',') | Some(b' ') | Some(b'\t')
            );
            let boundary_after = matches!(
                after,
                None | Some(b')') | Some(b',') | Some(b' ') | Some(b'\t')
            );
            if boundary_before && boundary_after {
                return true;
            }
        }
        idx += 1;
    }
    false
}

fn contains_state_mutating_sql(body: &str) -> bool {
    // sqlx call sites live in executable code; SQL verbs must appear inside
    // string literals. Searching the mixed keep-strings view false-positives on
    // Rust methods such as `headers_mut().insert(...)` in otherwise read-only
    // handlers (console-937 app-surface expansion).
    let executable = strip_comments_and_strings(body).to_ascii_lowercase();
    let sql_literals = string_literal_contents(body).to_ascii_lowercase();
    let uses_sqlx_query = executable.contains("sqlx::query")
        || executable.contains("query!")
        || executable.contains("query_as!")
        || executable.contains("query_scalar!");
    uses_sqlx_query
        && (["insert", "delete", "merge", "truncate"]
            .iter()
            .any(|keyword| contains_ascii_word(&sql_literals, keyword))
            || contains_non_locking_update(&sql_literals))
}

/// Emit only the contents of Rust string / raw-string literals (comments and
/// non-string code become spaces). Used to find SQL verbs without matching
/// homonymous Rust identifiers.
fn string_literal_contents(source: &str) -> String {
    let bytes = source.as_bytes();
    let mut output = String::with_capacity(source.len());
    let mut index = 0usize;
    let mut in_line_comment = false;
    let mut in_block_comment = false;
    let mut in_string = false;
    let mut in_char = false;
    let mut escaped = false;

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
        if in_string {
            if escaped {
                escaped = false;
                output.push(b as char);
            } else if b == b'\\' {
                escaped = true;
                output.push(' ');
            } else if b == b'"' {
                in_string = false;
                output.push(' ');
            } else {
                output.push(if b == b'\n' { '\n' } else { b as char });
            }
            index += 1;
            continue;
        }
        if in_char {
            if escaped {
                escaped = false;
            } else if b == b'\\' {
                escaped = true;
            } else if b == b'\'' {
                in_char = false;
            }
            output.push(if b == b'\n' { '\n' } else { ' ' });
            index += 1;
            continue;
        }

        if b == b'/' && next == Some(b'/') {
            in_line_comment = true;
            output.push_str("  ");
            index += 2;
        } else if b == b'/' && next == Some(b'*') {
            in_block_comment = true;
            output.push_str("  ");
            index += 2;
        } else if b == b'"' {
            in_string = true;
            output.push(' ');
            index += 1;
        } else if b == b'\'' {
            in_char = true;
            output.push(' ');
            index += 1;
        } else {
            output.push(if b == b'\n' { '\n' } else { ' ' });
            index += 1;
        }
    }

    output
}

fn contains_non_locking_update(searchable: &str) -> bool {
    ascii_word_starts(searchable, "update")
        .into_iter()
        .any(|start| previous_ascii_word(searchable, start).as_deref() != Some("for"))
}

fn ascii_word_starts(haystack: &str, needle: &str) -> Vec<usize> {
    let bytes = haystack.as_bytes();
    let needle_bytes = needle.as_bytes();
    if bytes.len() < needle_bytes.len() {
        return Vec::new();
    }
    (0..=bytes.len() - needle_bytes.len())
        .filter(|start| {
            bytes[*start..*start + needle_bytes.len()] == *needle_bytes
                && is_word_boundary(bytes.get(start.wrapping_sub(1)).copied())
                && is_word_boundary(bytes.get(*start + needle_bytes.len()).copied())
        })
        .collect()
}

fn previous_ascii_word(haystack: &str, before: usize) -> Option<String> {
    let bytes = haystack.as_bytes();
    let mut end = before;
    while end > 0 && !bytes[end - 1].is_ascii_alphanumeric() && bytes[end - 1] != b'_' {
        end -= 1;
    }
    let mut start = end;
    while start > 0 && (bytes[start - 1].is_ascii_alphanumeric() || bytes[start - 1] == b'_') {
        start -= 1;
    }
    (start < end).then(|| haystack[start..end].to_string())
}

fn contains_ascii_word(haystack: &str, needle: &str) -> bool {
    !ascii_word_starts(haystack, needle).is_empty()
}

fn strip_comments_and_strings(source: &str) -> String {
    strip_rust(source, false)
}

fn strip_rust(source: &str, keep_string_contents: bool) -> String {
    let bytes = source.as_bytes();
    let mut output = String::with_capacity(source.len());
    let mut index = 0usize;
    let mut in_line_comment = false;
    let mut in_block_comment = false;
    let mut in_string = false;
    let mut in_char = false;
    let mut escaped = false;

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
        if in_string {
            if escaped {
                escaped = false;
                output.push(if keep_string_contents { b as char } else { ' ' });
            } else if b == b'\\' {
                escaped = true;
                output.push(' ');
            } else if b == b'"' {
                in_string = false;
                output.push(' ');
            } else {
                output.push(if keep_string_contents {
                    b as char
                } else if b == b'\n' {
                    '\n'
                } else {
                    ' '
                });
            }
            index += 1;
            continue;
        }
        if in_char {
            if escaped {
                escaped = false;
            } else if b == b'\\' {
                escaped = true;
            } else if b == b'\'' {
                in_char = false;
            }
            output.push(if b == b'\n' { '\n' } else { ' ' });
            index += 1;
            continue;
        }

        if b == b'/' && next == Some(b'/') {
            in_line_comment = true;
            output.push_str("  ");
            index += 2;
        } else if b == b'/' && next == Some(b'*') {
            in_block_comment = true;
            output.push_str("  ");
            index += 2;
        } else if b == b'"' {
            in_string = true;
            output.push(' ');
            index += 1;
        } else if b == b'\'' {
            in_char = true;
            output.push(' ');
            index += 1;
        } else {
            output.push(b as char);
            index += 1;
        }
    }

    output
}

fn collect_rust_files(root: &Path) -> Result<Vec<PathBuf>, String> {
    let mut files = Vec::new();
    collect_rust_files_inner(root, &mut files)?;
    Ok(files)
}

fn collect_rust_files_inner(dir: &Path, files: &mut Vec<PathBuf>) -> Result<(), String> {
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
            collect_rust_files_inner(&path, files)?;
        } else if file_type.is_file() && path.extension().is_some_and(|ext| ext == "rs") {
            files.push(path);
        }
    }
    Ok(())
}

fn should_skip_dir(path: &Path) -> bool {
    let components: Vec<String> = path
        .components()
        .map(|component| component.as_os_str().to_string_lossy().into_owned())
        .collect();

    components
        .iter()
        // `tests` dirs hold integration tests, never handlers — auditing them for
        // AuditEvent/with_audit is a false positive (a test that exercises a
        // state-changing path is not itself a state-changing handler).
        .any(|part| part == "target" || part == ".git" || part == "tests")
        || components
            .windows(2)
            .any(|window| window[0] == "ci" && window[1] == "gates")
}
