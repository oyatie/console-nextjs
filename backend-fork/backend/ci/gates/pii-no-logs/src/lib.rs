//! PII-in-logs gate.
//!
//! # Scope and limitations (literal-only)
//!
//! This gate is a **static, literal-pattern** scanner. It parses each Rust source
//! file, isolates the bodies of logging macros (`tracing::{trace,debug,info,warn,
//! error,event}` / `log::*`), and flags bodies that contain a literal PII pattern:
//! a Korean mobile number (`010-XXXX-XXXX`), a resident-registration number
//! (`NNNNNN-NNNNNNN`), or a GPS coordinate pair. It is a cheap, deterministic
//! tripwire against the obvious mistake of pasting raw PII into a log line.
//!
//! It deliberately does **not** perform data-flow analysis, so it CANNOT catch
//! PII that reaches a log through a binding or interpolation, e.g.
//! `tracing::info!(phone = %customer.phone, "...")` or
//! `let p = customer.phone; warn!("contact {p}")`. Those carry no literal pattern
//! and pass the gate. Treat a green gate as "no obvious literal leak", not "no PII
//! in logs".
//!
//! The complementary runtime defense is to wrap sensitive fields in a redacting
//! newtype so their `Display`/`Debug` never renders the raw value (see
//! `console_kernel_core::RedactedPhone`). The newtype closes the interpolation gap
//! this gate cannot see; the gate closes the raw-literal gap. They are layered,
//! not redundant.

use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ViolationKind {
    KoreanPhoneNumber,
    GpsCoordinatePair,
    ResidentRegistrationNumber,
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
    let files = collect_rust_files(workspace_dir)?;
    // A gate that examined NO subject must fail, not pass. Measured: run from an
    // empty directory this binary exits 0 saying "console-gate-pii-no-logs:
    // PASSED", because zero files yield zero violations. The workspace holds
    // thousands of Rust files, so zero means the scan did not find them, never
    // that none of them log personal data.
    //
    // Same rule as `topology.canonical_enforcement` (refuses zero tables) and
    // `tools/ci/gate-sweep.mjs` (refuses zero gates).
    if files.is_empty() {
        return Err(format!(
            "examined no Rust source files under {} -- a gate that examines nothing \
             cannot report absence",
            workspace_dir.display()
        ));
    }
    Ok(check_files(files))
}

#[must_use]
pub fn check_source_tree(root: &Path) -> GateResult {
    match collect_rust_files(root) {
        Ok(files) => check_files(files),
        Err(e) => GateResult {
            violations: vec![Violation {
                kind: ViolationKind::KoreanPhoneNumber,
                file: root.to_path_buf(),
                detail: e,
            }],
        },
    }
}

fn check_files(files: Vec<PathBuf>) -> GateResult {
    let mut result = GateResult::default();
    for file in files {
        match fs::read_to_string(&file) {
            Ok(source) => check_source_file(&file, &source, &mut result),
            Err(e) => result.violations.push(Violation {
                kind: ViolationKind::KoreanPhoneNumber,
                file,
                detail: format!("cannot read Rust source file: {e}"),
            }),
        }
    }
    result
}

fn check_source_file(file: &Path, source: &str, result: &mut GateResult) {
    for macro_body in logging_macro_bodies(source) {
        if contains_korean_phone_number(&macro_body) {
            result.violations.push(Violation {
                kind: ViolationKind::KoreanPhoneNumber,
                file: file.to_path_buf(),
                detail: "Korean mobile phone pattern inside logging macro".to_string(),
            });
        }
        if contains_gps_coordinate_pair(&macro_body) {
            result.violations.push(Violation {
                kind: ViolationKind::GpsCoordinatePair,
                file: file.to_path_buf(),
                detail: "GPS coordinate pair inside logging macro".to_string(),
            });
        }
        if contains_resident_registration_number(&macro_body) {
            result.violations.push(Violation {
                kind: ViolationKind::ResidentRegistrationNumber,
                file: file.to_path_buf(),
                detail: "resident registration number pattern inside logging macro".to_string(),
            });
        }
    }
}

fn logging_macro_bodies(source: &str) -> Vec<String> {
    let bytes = source.as_bytes();
    let mut bodies = Vec::new();
    let mut index = 0usize;

    while index < bytes.len() {
        if bytes[index] != b'!' {
            index += 1;
            continue;
        }

        let Some((macro_path, macro_start)) = macro_path_before_bang(source, index) else {
            index += 1;
            continue;
        };
        if !is_logging_macro_path(&macro_path) {
            index += 1;
            continue;
        }

        let Some((open_index, open_delim)) = next_open_delimiter(bytes, index + 1) else {
            index += 1;
            continue;
        };
        let close_delim = matching_delimiter(open_delim);
        let Some(close_index) =
            find_matching_delimiter(source, open_index, open_delim, close_delim)
        else {
            index += 1;
            continue;
        };
        if let Some(body) = source.get(open_index + 1..close_index) {
            bodies.push(body.to_string());
        }

        index = close_index.max(macro_start) + 1;
    }

    bodies
}

fn macro_path_before_bang(source: &str, bang_index: usize) -> Option<(String, usize)> {
    let bytes = source.as_bytes();
    let mut start = bang_index;
    while start > 0 {
        let prev = bytes[start - 1];
        if prev.is_ascii_alphanumeric() || prev == b'_' || prev == b':' {
            start -= 1;
        } else {
            break;
        }
    }
    if start == bang_index {
        return None;
    }
    source
        .get(start..bang_index)
        .map(|path| (path.to_string(), start))
}

fn is_logging_macro_path(path: &str) -> bool {
    let macro_name = path.rsplit("::").next().unwrap_or(path);
    let is_log_name = matches!(
        macro_name,
        "trace" | "debug" | "info" | "warn" | "error" | "event"
    );
    if !is_log_name {
        return false;
    }
    !path.contains("::") || path.starts_with("tracing::") || path.starts_with("log::")
}

fn next_open_delimiter(bytes: &[u8], mut index: usize) -> Option<(usize, u8)> {
    while index < bytes.len() && bytes[index].is_ascii_whitespace() {
        index += 1;
    }
    let delimiter = *bytes.get(index)?;
    if matches!(delimiter, b'(' | b'{' | b'[') {
        Some((index, delimiter))
    } else {
        None
    }
}

fn matching_delimiter(open: u8) -> u8 {
    match open {
        b'(' => b')',
        b'{' => b'}',
        b'[' => b']',
        _ => open,
    }
}

fn find_matching_delimiter(
    source: &str,
    open: usize,
    open_delim: u8,
    close_delim: u8,
) -> Option<usize> {
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

        if b == open_delim {
            depth += 1;
        } else if b == close_delim {
            depth = depth.saturating_sub(1);
            if depth == 0 {
                return Some(index);
            }
        }
        index += 1;
    }
    None
}

fn contains_korean_phone_number(text: &str) -> bool {
    let bytes = text.as_bytes();
    if bytes.len() < 13 {
        return false;
    }
    (0..=bytes.len() - 13).any(|index| {
        bytes.get(index..index + 4) == Some(b"010-")
            && bytes[index + 4..index + 8].iter().all(u8::is_ascii_digit)
            && bytes[index + 8] == b'-'
            && bytes[index + 9..index + 13].iter().all(u8::is_ascii_digit)
    })
}

fn contains_resident_registration_number(text: &str) -> bool {
    let bytes = text.as_bytes();
    if bytes.len() < 14 {
        return false;
    }
    (0..=bytes.len() - 14).any(|index| {
        bytes[index..index + 6].iter().all(u8::is_ascii_digit)
            && bytes[index + 6] == b'-'
            && bytes[index + 7..index + 14].iter().all(u8::is_ascii_digit)
    })
}

fn contains_gps_coordinate_pair(text: &str) -> bool {
    let bytes = text.as_bytes();
    let mut index = 0usize;

    while index < bytes.len() {
        let Some(first) = parse_decimal(text, index) else {
            index += 1;
            continue;
        };
        let mut cursor = first.end;
        while bytes.get(cursor).is_some_and(u8::is_ascii_whitespace) {
            cursor += 1;
        }
        if bytes.get(cursor) != Some(&b',') {
            index = first.end;
            continue;
        }
        cursor += 1;
        while bytes.get(cursor).is_some_and(u8::is_ascii_whitespace) {
            cursor += 1;
        }
        if let Some(second) = parse_decimal(text, cursor)
            && first.fraction_digits >= 3
            && second.fraction_digits >= 3
            && is_coordinate_pair(first.value, second.value)
        {
            return true;
        }
        index = first.end;
    }

    false
}

fn is_coordinate_pair(first: f64, second: f64) -> bool {
    ((-90.0..=90.0).contains(&first) && (-180.0..=180.0).contains(&second))
        || ((-180.0..=180.0).contains(&first) && (-90.0..=90.0).contains(&second))
}

#[derive(Debug)]
struct ParsedDecimal {
    value: f64,
    end: usize,
    fraction_digits: usize,
}

fn parse_decimal(text: &str, start: usize) -> Option<ParsedDecimal> {
    let bytes = text.as_bytes();
    let mut index = start;
    if bytes.get(index) == Some(&b'-') {
        index += 1;
    }

    let int_start = index;
    while bytes.get(index).is_some_and(u8::is_ascii_digit) {
        index += 1;
    }
    if index == int_start || bytes.get(index) != Some(&b'.') {
        return None;
    }
    index += 1;
    let frac_start = index;
    while bytes.get(index).is_some_and(u8::is_ascii_digit) {
        index += 1;
    }
    let fraction_digits = index.saturating_sub(frac_start);
    if fraction_digits == 0 {
        return None;
    }

    let value = text.get(start..index)?.parse::<f64>().ok()?;
    Some(ParsedDecimal {
        value,
        end: index,
        fraction_digits,
    })
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
        .any(|part| part == "target" || part == ".git")
        || components
            .windows(2)
            .any(|window| window[0] == "ci" && window[1] == "gates")
}
