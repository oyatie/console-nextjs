//! The DATABASE half, bound to an EXECUTED PostgreSQL — not to the script's text.
//!
//! `gate_detects_violation.rs` asserts things about the *characters* in
//! `ops/postgres-reconcile-topology.sh`. That is the same failure class as a
//! grep-shaped gate over Rust: changing `IF leaked IS NOT NULL THEN` to
//! `IF leaked IS NOT NULL AND false THEN` kills the entire deny-by-default
//! census and every text assertion stays green. This file runs the real script
//! against a real PostgreSQL and asserts on what the database ends up doing.
//!
//! ## What runs here
//!
//! `census_binds_to_an_executed_database` drives one container through:
//!
//! 1. NO canonical table exists and the run CLAIMS to enforce
//!    (`CONSOLE_TOPOLOGY_REQUIRE_CANONICAL_TABLES=1`) — the run must FAIL.
//! 2. Migrations applied, then enforcement — must PASS, and the SET of tables
//!    it examined must be exactly [`EXPECTED_REQUIRED_TABLES`]. Not "more than
//!    zero": renaming one canonical table away used to shrink the scope from
//!    eight to seven and still pass.
//! 3. [`PROBES`] — one disposable probe database each, one per shape a writer
//!    can take: a table grant, a COLUMN grant (invisible to
//!    `has_table_privilege`), a TRUNCATE-only grant (which has no column form),
//!    a grant on a PARTITIONED canonical table, one on a partition CHILD
//!    (whose relname the roster does not contain) and one on a relation a
//!    canonical table is made a CHILD of, an unexpected table OWNER, an owner
//!    that is already on the expected-WRITER ratchet (ownership is DDL
//!    authority, not DML, so the ratchet may not authorise it),
//!    membership in `pg_write_all_data` both INHERIT and NOINHERIT, an
//!    unexpected SUPERUSER, a canonical table RENAMED away, MOVED to another
//!    schema behind a decoy, parked in `information_schema`, and replaced by a
//!    VIEW.
//!
//!    A probe that needs a roster name in a shape migrations do not produce now
//!    TAKES the name — `DROP TABLE IF EXISTS ... CASCADE` and recreate. It used
//!    to borrow a roster name no migration had created yet, and that was moved
//!    twice (off `persons` at 0213, off `employment_heads` at 0214) before
//!    migration 0215 completed the roster and left nothing to borrow.
//!
//!    The tenth shape, membership in `console_app` itself, is REPAIRED by the
//!    reconcile rather than rejected by it — the exact-topology REVOKE strips
//!    every membership edge touching an application role before the canonical
//!    block runs — so phase 2b proves it by breaking that REVOKE instead.
//! 4. [`MUTATIONS`] — each one applied to a COPY of the script, run against the
//!    probe that is supposed to catch it, and required to FLIP that probe's
//!    verdict. A control that keeps its verdict when the control is broken is
//!    not testing anything. These used to live in a `mutation-check.sh` that
//!    nothing invoked, so they ran only when a human remembered. Two are
//!    measured against an UNARMED reconcile, with their own committed-script
//!    baseline; [`Mutation::armed`] says why the completed roster leaves them no
//!    armed configuration in which they flip anything.
//!
//! `pgtest_harness_executes_the_enforcement` and
//! `cargo_needs_postgres_harness_executes_the_enforcement` prove the WIRING by
//! running each harness and observing the enforcement's own output line. The
//! previous spelling was `harness_text.contains("canonical-enforce.sh")`, which
//! a `#` in front of the invocation satisfies — a grep-shaped gate rebuilt
//! inside the guard meant to prove the gate is not grep-shaped.
//!
//! ## Docker
//!
//! Every test here REQUIRES Docker and FAILS without it. It used to `return
//! Ok(())` after an `eprintln!` banner, but cargo captures stdio from PASSING
//! tests and replays it only for failures, so the banner was never printed: a
//! machine with no Docker reported `test result: ok. 2 passed`, byte-identical
//! to a fully verified run, having executed nothing. An unexecuted census must
//! not be able to look like a verified one, so it is a failure instead.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Same digest the two harnesses pin.
const IMAGE: &str =
    "postgres:18.4@sha256:65f70a152846cf504dff86e807007e9aeac98c3aeb7b62541b2c55ab9d264e56";

/// The canonical tables a fully migrated database has TODAY, and therefore the
/// exact set the census must examine. This list is a VERBATIM PIN, not a second
/// source: the census scope is DERIVED from the `required_tables` array in
/// `ops/postgres-reconcile-topology.sh` by [`required_tables_from_topology`],
/// and [`derived_required_tables_match_the_verbatim_roster`] fails the run when
/// the derived set and this pin diverge. A lane that lands a new canonical
/// table edits the shell array and this pin — two edits in two files, instead
/// of a fixed-arity const plus two shared arrays that four port lanes had to
/// hand-append in lockstep.
///
/// As of migration 0215 this is the WHOLE roster — every name in
/// `ObjectKey::owned_tables` now resolves to a real relation, so the set can no
/// longer grow without a seventh object key. `employee_person_bindings`,
/// `person_revisions` and `persons` landed with migration 0213 (the
/// `ObjectKey::Person` tables); `employment_heads`, `employment_revisions` and
/// `employment_source_bindings` with 0214 (the three `ObjectKey::Employment`
/// tables that did not already exist — `employees` is the fourth and predates
/// both); `company_revisions`, `org_units`, `org_unit_revisions`,
/// `org_unit_source_bindings`, `job_positions` and `job_position_revisions`
/// with 0215 (the six of `ObjectKey::Company`, `ObjectKey::OrgUnit` and
/// `ObjectKey::JobPosition` that did not already exist — `organizations` is the
/// seventh and predates all three migrations). Sorted, because [`examined_set`]
/// sorts what it reads back.
const EXPECTED_REQUIRED_TABLES: [&str; 20] = [
    "company_revisions",
    "employee_person_bindings",
    "employees",
    "employment_heads",
    "employment_revisions",
    "employment_source_bindings",
    "job_position_revisions",
    "job_positions",
    "org_unit_revisions",
    "org_unit_source_bindings",
    "org_units",
    "organizations",
    "payroll_disbursements",
    "payroll_draft_lines",
    "payroll_draft_runs",
    "payroll_line_calculations",
    "payroll_payslip_deliveries",
    "payroll_run_exceptions",
    "person_revisions",
    "persons",
];

type Fallible = Result<(), Box<dyn std::error::Error>>;
/// Derives the census scope from the `required_tables` array in the topology
/// script — the single production source — instead of a hand-maintained const.
/// Isolates the `DO $canonical$` block first (whole-line opener, same as
/// `canonical_block` in `gate_detects_violation.rs`), with the opener and
/// `$canonical$;` terminator themselves located in live code, then the
/// `required_tables CONSTANT TEXT[] := ARRAY[...]` declaration inside that
/// block, then the `'name'` entries between its BEGIN/END markers. A decoy
/// marker or `ARRAY[` statement earlier in the shell script cannot win, and
/// neither can a block or declaration parked inside a `--` or `/* ... */`
/// comment or a single-/dollar-quoted string literal PostgreSQL ignores: only a
/// code-context block/declaration whose own `];`-bounded span holds both
/// markers is read, and only code-context entry literals feed the roster. Sorts
/// the names (the
/// census compares
/// against a sorted `examined_set`) and fails loudly when the block, the
/// statement, the markers, or the entries are missing rather than examining a
/// silently shrunk scope.
fn required_tables_from_topology(script: &Path) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let text =
        std::fs::read_to_string(script).map_err(|e| format!("read {}: {e}", script.display()))?;
    // Anchor to the psql heredoc body first: the `DO $canonical$` block is SQL
    // psql actually executes, so a decoy copy parked in shell data (a non-psql
    // heredoc or here-string) must not win block selection. Within that body,
    // mask comments and dollar-quoted literals (except the `$canonical$`
    // delimiter itself) so a commented or string-embedded old/example copy also
    // cannot win. The opener is whole-line matched, same as `canonical_block`
    // in `gate_detects_violation.rs`.
    let body = psql_heredoc_body(&text).ok_or_else(|| {
        format!(
            "missing psql heredoc with a `DO $canonical$` block in {}",
            script.display()
        )
    })?;
    let comment_code = code_mask_block(body);
    let opener = "\nDO $canonical$\n";
    // Bind to the canonical block that actually ENFORCES the roster: prefer the
    // block whose body reads `unnest(required_tables)`. An earlier valid block
    // with an unused `required_tables` declaration must not win. Simplified
    // fixtures without enforcement fall back to the first block.
    let mut blocks: Vec<(usize, usize)> = Vec::new();
    let mut search_from = 0;
    while let Some(start) = find_code(body, &comment_code, search_from, opener) {
        let content_start = start + opener.len();
        let Some(end) = find_code(body, &comment_code, content_start, "$canonical$;") else {
            break;
        };
        blocks.push((content_start, end));
        search_from = end + "$canonical$;".len();
    }
    // The enforcement reference must be LIVE SQL, not a comment or string
    // mention of `unnest(required_tables)`.
    let chosen = blocks
        .iter()
        .enumerate()
        .rev()
        .find(|&(_, &(content_start, end))| {
            find_code(
                &body[content_start..end],
                &comment_code[content_start..end],
                0,
                "unnest(required_tables)",
            )
            .is_some()
        })
        .map(|(index, _)| index)
        .unwrap_or(0);
    let &(content_start, end) = blocks
        .get(chosen)
        .ok_or_else(|| format!("no `DO $canonical$` block in {}", script.display()))?;
    let block = &body[content_start..end];
    // Inside the block, ALSO mask single- and dollar-quoted string literals so
    // a decoy declaration (or `];`) parked in a string cannot win declaration
    // selection; the roster entries are themselves single-quoted, so the entry
    // scan below uses a dollar-aware mask (comments + dollar-quotes, but not
    // single quotes).
    let code = code_mask_full(block);
    let entry_code = code_mask_dollar(block);
    let statement = "required_tables CONSTANT TEXT[] := ARRAY[";
    let begin = "-- canonical-writer-ownership: BEGIN required tables";
    let end = "-- canonical-writer-ownership: END required tables";
    // Walk CODE-context declaration occurrences and keep the FIRST one whose
    // own ARRAY[...] span contains both markers: the outermost declaration is
    // the one the enforcement reads, so a later shadowing declaration in a
    // nested block cannot win.
    let mut names = Vec::new();
    let mut found = false;
    let mut from = 0;
    while let Some(statement_at) = find_code(block, &code, from, statement) {
        // `required_tables` must be a FULL identifier, not the suffix of a
        // longer one such as `backup_required_tables`: a bare substring match
        // would read the decoy's pinned roster while the enforcement's array
        // drifts. Reject the match when the preceding byte is still part of an
        // unquoted SQL identifier (letter, digit, `_`, or `$`).
        let bytes = block.as_bytes();
        if statement_at > 0 {
            let prev = bytes[statement_at - 1];
            if prev.is_ascii_alphanumeric() || prev == b'_' || prev == b'$' {
                from = statement_at + statement.len();
                continue;
            }
        }
        // The declaration owns everything up to its closing `];`: markers (and
        // entries) past that close belong to a different statement.
        let Some(close) = find_code(block, &code, statement_at + statement.len(), "];") else {
            from = statement_at + statement.len();
            continue;
        };
        let span = &block[statement_at..close];
        if let (Some(s), Some(e)) = (span.find(begin), span.find(end)) {
            // Read only single-quoted literals whose opening quote is LIVE code:
            // a literal moved into a `--`/`/* ... */` comment or a dollar-quoted
            // string is omitted by PostgreSQL and must not feed the roster.
            let region = &span[s..e];
            let region_code = &entry_code[statement_at + s..statement_at + e];
            let region_bytes = region.as_bytes();
            let mut parsed = Vec::new();
            let mut at = 0;
            while at < region_bytes.len() {
                if !region_code[at] {
                    at += 1;
                    continue;
                }
                let byte = region_bytes[at];
                if byte == b'\'' {
                    if let Some((name, _)) = region[at + 1..].split_once('\'') {
                        if !name.is_empty() {
                            parsed.push(name.to_string());
                        }
                        at += 1 + name.len() + 1;
                        continue;
                    }
                    at += 1;
                    continue;
                }
                if byte.is_ascii_whitespace() || byte == b',' {
                    at += 1;
                    continue;
                }
                // Any other live code token (CASE, identifiers, operators, ...)
                // means the array is an expression PostgreSQL evaluates, which a
                // static scan cannot reproduce: fail loudly rather than silently
                // mis-derive the roster.
                return Err(format!(
                    "non-literal expression in the required_tables array in {}",
                    script.display()
                )
                .into());
            }
            if !parsed.is_empty() {
                names = parsed;
                found = true;
                break;
            }
        }
        from = statement_at + statement.len();
    }
    if !found {
        return Err(format!(
            "no canonical `required_tables` declaration with both markers in {}",
            script.display()
        )
        .into());
    }
    names.sort();
    names.dedup();
    Ok(names)
}

/// Byte-parallel lex mask: `true` where the byte is live SQL/PLpgSQL code.
/// `--` line comments (through the next newline) and `/* ... */` block comments
/// are always masked; PostgreSQL block comments NEST (unlike C), so the mask
/// tracks `/*`/`*/` depth. `mask_single` masks single-quoted literals
/// (`'...'` with `''` doubling; `E'...'` honors backslash escapes), and
/// `mask_dollar` masks dollar-quoted (`$tag$...$tag$`) literals — a decoy
/// declaration, `];`, or roster entry parked in a string must not win.
/// `keep_dollar_tag` names a dollar-quote tag (e.g. `canonical`) that is left
/// as code rather than masked, so block selection can still locate the real
/// `DO $canonical$` delimiter.
fn lex_mask(
    block: &str,
    mask_single: bool,
    mask_dollar: bool,
    keep_dollar_tag: Option<&[u8]>,
) -> Vec<bool> {
    let bytes = block.as_bytes();
    let mut mask = vec![true; bytes.len()];
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'-' && i + 1 < bytes.len() && bytes[i + 1] == b'-' {
            while i < bytes.len() && bytes[i] != b'\n' {
                mask[i] = false;
                i += 1;
            }
            continue;
        }
        if bytes[i] == b'/' && i + 1 < bytes.len() && bytes[i + 1] == b'*' {
            mask[i] = false;
            mask[i + 1] = false;
            i += 2;
            let mut depth = 1;
            while i < bytes.len() {
                if bytes[i] == b'/' && i + 1 < bytes.len() && bytes[i + 1] == b'*' {
                    mask[i] = false;
                    mask[i + 1] = false;
                    i += 2;
                    depth += 1;
                } else if bytes[i] == b'*' && i + 1 < bytes.len() && bytes[i + 1] == b'/' {
                    mask[i] = false;
                    mask[i + 1] = false;
                    i += 2;
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                } else {
                    mask[i] = false;
                    i += 1;
                }
            }
            continue;
        }
        if bytes[i] == b'"' {
            // Double-quoted identifier (`"..."` with `""` doubling): a single
            // token, never live declaration code, so mask it everywhere.
            mask[i] = false;
            i += 1;
            while i < bytes.len() {
                if bytes[i] == b'"' {
                    mask[i] = false;
                    i += 1;
                    if i < bytes.len() && bytes[i] == b'"' {
                        mask[i] = false;
                        i += 1;
                        continue;
                    }
                    break;
                }
                mask[i] = false;
                i += 1;
            }
            continue;
        }
        if mask_single && bytes[i] == b'\'' {
            // `E'...'` escape strings interpret backslash escapes, so `\'` is an
            // escaped quote, not the terminator; a regular string only escapes
            // via doubled `''`.
            let escape = i > 0 && (bytes[i - 1] == b'E' || bytes[i - 1] == b'e');
            mask[i] = false;
            i += 1;
            while i < bytes.len() {
                if escape && bytes[i] == b'\\' && i + 1 < bytes.len() {
                    mask[i] = false;
                    mask[i + 1] = false;
                    i += 2;
                    continue;
                }
                if bytes[i] == b'\'' {
                    mask[i] = false;
                    i += 1;
                    if !escape && i < bytes.len() && bytes[i] == b'\'' {
                        mask[i] = false;
                        i += 1;
                        continue;
                    }
                    break;
                }
                mask[i] = false;
                i += 1;
            }
            continue;
        }
        if mask_dollar && bytes[i] == b'$' {
            let mut j = i + 1;
            while j < bytes.len() && bytes[j] != b'$' {
                if !(bytes[j] == b'_' || bytes[j].is_ascii_alphanumeric()) {
                    break;
                }
                j += 1;
            }
            let valid_tag = j < bytes.len()
                && bytes[j] == b'$'
                && (j == i + 1 || bytes[i + 1] == b'_' || bytes[i + 1].is_ascii_alphabetic());
            if valid_tag {
                let tag = &bytes[i + 1..j];
                if let Some(keep) = keep_dollar_tag
                    && tag == keep
                {
                    // The kept delimiter (e.g. `$canonical$`) stays code so block
                    // selection can locate it.
                    i += 1;
                    continue;
                }
                let tag_len = tag.len();
                let mut k = j + 1;
                let mut close_at = None;
                while k + tag_len + 1 < bytes.len() {
                    if bytes[k] == b'$'
                        && bytes[k + 1..k + 1 + tag_len] == *tag
                        && bytes[k + 1 + tag_len] == b'$'
                    {
                        close_at = Some(k);
                        break;
                    }
                    k += 1;
                }
                let end = match close_at {
                    Some(k) => k + tag_len + 2,
                    None => bytes.len(),
                };
                mask[i..end].fill(false);
                i = end;
                continue;
            }
            i += 1;
            continue;
        }
        i += 1;
    }
    mask
}

/// Block-selection code mask: comments plus dollar-quoted literals, EXCEPT the
/// `$canonical$` delimiter itself. A decoy `DO $canonical$` block parked inside
/// a different dollar-quoted string cannot win block selection.
fn code_mask_block(block: &str) -> Vec<bool> {
    lex_mask(block, false, true, Some(b"canonical"))
}

/// Full code mask (see [`lex_mask`]): comments plus single- and dollar-quoted
/// string literals, so a decoy declaration parked in a string cannot win.
fn code_mask_full(block: &str) -> Vec<bool> {
    lex_mask(block, true, true, None)
}

/// Roster-entry code mask (see [`lex_mask`]): comments plus dollar-quoted
/// literals, but NOT single-quoted literals (the roster entries are themselves
/// single-quoted). A `'name'` parked inside a dollar-quoted string is omitted.
fn code_mask_dollar(block: &str) -> Vec<bool> {
    lex_mask(block, false, true, None)
}

/// The body of the psql heredoc (`<<'SQL'` ... `SQL`) that carries the live
/// `DO $canonical$` block — the only SQL psql actually receives for this gate.
/// A decoy block parked in shell data (a non-psql heredoc or here-string) must
/// not win block selection. The scanner tracks EVERY heredoc's body so that a
/// `cat <<'DOC'` data block containing example text `psql ... <<'SQL'` is not
/// misread as a real psql heredoc. Searches the last heredoc first, so a later
/// reconcile wins over an earlier example.
fn psql_heredoc_body(text: &str) -> Option<&str> {
    let mut regions = Vec::new();
    let mut search_from = 0;
    while let Some(rel) = text[search_from..].find("<<") {
        let opener = search_from + rel;
        // `<<<` is a here-string, not a heredoc.
        if text[opener + 2..].starts_with('<') {
            search_from = opener + 3;
            continue;
        }
        // Parse the delimiter (quoted `'DELIM'`/`"DELIM"` or a bare word) and
        // locate the body start (after the opener line's newline).
        let after = &text[opener + 2..];
        let (delim, body_start) = match after.as_bytes().first().copied() {
            Some(quote @ (b'\'' | b'"')) => {
                let Some(close) = after[1..].find(quote as char) else {
                    break;
                };
                let Some(nl) = after[1 + close + 1..].find('\n') else {
                    break;
                };
                let delim = &after[1..1 + close];
                let body_start = opener + 2 + 1 + close + 1 + nl + 1;
                (delim, body_start)
            }
            _ => {
                let delim_end = after.find(char::is_whitespace).unwrap_or(after.len());
                let Some(nl) = after[delim_end..].find('\n') else {
                    break;
                };
                let delim = &after[..delim_end];
                let body_start = opener + 2 + delim_end + nl + 1;
                (delim, body_start)
            }
        };
        // Find the closing delimiter line.
        let tail = &text[body_start..];
        let mut line_start = 0;
        let mut close_offset = None;
        loop {
            match tail[line_start..].find('\n') {
                Some(nl) => {
                    let line_end = line_start + nl;
                    if tail[line_start..line_end].trim() == delim {
                        close_offset = Some(line_start);
                        break;
                    }
                    line_start = line_end + 1;
                }
                None => {
                    if tail[line_start..].trim() == delim {
                        close_offset = Some(line_start);
                    }
                    break;
                }
            }
        }
        let Some(close_offset) = close_offset else {
            break;
        };
        let body_end = body_start + close_offset;
        // Only a `psql`-owned `<<'SQL'` heredoc executes SQL; the command word
        // itself must be `psql` (or follow a `VAR=value` env assignment), so
        // `echo psql <<'SQL'` and `cat <<'SQL'` are rejected.
        let line_start = text[..opener].rfind('\n').map_or(0, |n| n + 1);
        let line = text[line_start..opener].trim();
        let mut words = line.split_whitespace();
        let is_psql = match words.next() {
            Some("psql") => true,
            Some(word) if word.contains('=') => words.next() == Some("psql"),
            _ => false,
        };
        if is_psql && delim == "SQL" {
            regions.push((body_start, body_end));
        }
        // Skip the whole body (data heredocs may contain decoy `<<'SQL'` text).
        search_from = body_end;
    }
    // A fixture without any psql heredoc treats the whole text as the executing
    // SQL; the production script always carries the block inside a heredoc.
    if regions.is_empty() {
        return Some(text);
    }
    regions
        .into_iter()
        .rev()
        .map(|(start, end)| &text[start..end])
        .find(|body| body.contains("DO $canonical$"))
}

/// The first offset at or after `from` where `needle` starts entirely inside
/// live code (per [`code_mask`]); comment-hidden occurrences are skipped.
fn find_code(block: &str, mask: &[bool], from: usize, needle: &str) -> Option<usize> {
    let mut search_from = from;
    while let Some(rel) = block[search_from..].find(needle) {
        let at = search_from + rel;
        if mask[at..at + needle.len()].iter().all(|&live| live) {
            return Some(at);
        }
        search_from = at + needle.len();
    }
    None
}

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../..")
}

fn gate_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn topology_script() -> PathBuf {
    repo_root().join("ops/postgres-reconcile-topology.sh")
}

fn run(program: &str, args: &[&str]) -> Result<(bool, String), Box<dyn std::error::Error>> {
    run_env(program, args, &[])
}

fn run_env(
    program: &str,
    args: &[&str],
    env: &[(&str, &str)],
) -> Result<(bool, String), Box<dyn std::error::Error>> {
    let mut command = Command::new(program);
    command.args(args);
    for (key, value) in env {
        command.env(key, value);
    }
    let output = command.output()?;
    let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    Ok((output.status.success(), text))
}

fn docker_available() -> bool {
    Command::new("docker")
        .arg("version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

/// Docker is a requirement, not a nicety. See the module doc: skipping silently
/// is how a run that executed nothing certified the database half as green.
fn require_docker() {
    assert!(
        docker_available(),
        "Docker is REQUIRED by this test and is absent. This file is the only \
         thing that executes the canonical writer-ownership census; skipping it \
         would report a pass having verified nothing, which is exactly the \
         fail-open it exists to close. Run it where Docker is available."
    );
}

struct Container(String);

impl Drop for Container {
    fn drop(&mut self) {
        let _ = Command::new("docker")
            .args(["rm", "-fv", &self.0])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

/// Six distinct 64-hex secrets, derived without a crypto dependency. The
/// reconcile refuses non-distinct passwords, and these never leave the
/// container's env file.
fn secrets() -> Vec<String> {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0u128, |d| d.as_nanos());
    (0..6u128)
        .map(|i| {
            let seed = nanos
                .wrapping_mul(0x9E37_79B9_7F4A_7C15)
                .wrapping_add(i.wrapping_mul(0x1234_5678_9ABC_DEF1));
            format!("{seed:032x}{i:032x}")
        })
        .collect()
}

fn unique(prefix: &str) -> String {
    format!(
        "{prefix}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos())
    )
}

// ---------------------------------------------------------------------------
// The rogue probes, and the mutations that must flip each one
// ---------------------------------------------------------------------------

/// One planted violation and the reconcile's required reaction to it.
struct Probe {
    /// Probe-database suffix; also the label in failures.
    label: &'static str,
    /// Applied as the cluster admin AFTER migrations, BEFORE enforcement.
    sql: &'static str,
    /// The `topology.*` code the reconcile must raise.
    error: &'static str,
    /// The offending object or role the message must name.
    names: &'static str,
}

/// Every shape an unaccounted writer can take on a canonical table. Each is
/// planted on its own probe database, because planting them together would let
/// one detection carry the others.
///
/// Six of these used to be caught by six DIFFERENT guards — an ACL union, a
/// column-ACL union, an owner check, a `pg_auth_members` check, a `rolsuper`
/// exclusion. They are now all the same finding, because they are all the same
/// question: can this role write this relation?
const PROBES: [Probe; 16] = [
    Probe {
        // OWNERSHIP is not the DML question, and the census's ratchet is a
        // (role, relation) whitelist of who may WRITE. `console_rt` is on that
        // ratchet — it is the runtime login principal — so folding ownership
        // into the census made this plant invisible: the census excludes the
        // candidate by NAME before it ever asks the privilege question. An
        // owner also holds ALTER, DROP, TRUNCATE and, decisively for a table
        // whose tenant isolation is entirely RLS, `DISABLE ROW LEVEL SECURITY`
        // and `DROP POLICY`. The owner is pinned by its own list.
        label: "owner_is_a_ratcheted_writer",
        sql: "ALTER TABLE public.employees OWNER TO console_rt;\n",
        error: "topology.canonical_table_owner_failed",
        names: "employees:console_rt",
    },
    Probe {
        // Reachability runs DOWN to children and UP to parents. A canonical
        // table made the CHILD of a new relation is written through that
        // parent, which holds no privilege on `employees` at all — so the
        // deny-by-default census reports clean while the rows disappear.
        label: "inheritance_parent",
        sql: "DROP ROLE IF EXISTS console_rogue_parent;\n\
              CREATE ROLE console_rogue_parent NOLOGIN;\n\
              CREATE TABLE public.rogue_super_tbl (id uuid);\n\
              ALTER TABLE public.rogue_super_tbl OWNER TO console_app;\n\
              ALTER TABLE public.employees INHERIT public.rogue_super_tbl;\n\
              GRANT UPDATE, DELETE ON public.rogue_super_tbl TO console_rogue_parent;\n",
        error: "topology.canonical_writer_ownership_failed",
        names: "console_rogue_parent",
    },
    Probe {
        // The real, data-bearing table moves to another schema and a same-named
        // decoy is left in `public`. Nothing disappears, so a name-matched
        // census reports clean while the rogue keeps DML on the real rows.
        label: "schema_moved_away",
        sql: "DROP ROLE IF EXISTS console_rogue_schema;\n\
              CREATE ROLE console_rogue_schema NOLOGIN;\n\
              CREATE SCHEMA shadow;\n\
              ALTER TABLE public.employees SET SCHEMA shadow;\n\
              CREATE TABLE public.employees (id uuid);\n\
              ALTER TABLE public.employees OWNER TO console_app;\n\
              GRANT INSERT, UPDATE, DELETE ON shadow.employees TO console_rogue_schema;\n",
        error: "topology.canonical_roster_unresolved",
        names: "shadow.employees",
    },
    Probe {
        // The same shrink, parked in the ONE schema the previous design
        // hard-excluded from its search. A roster name is a name wherever it
        // lives, so the resolution has no exclusion list at all.
        label: "information_schema_park",
        sql: "DROP ROLE IF EXISTS console_rogue_infoschema;\n\
              CREATE ROLE console_rogue_infoschema NOLOGIN;\n\
              CREATE TABLE information_schema.persons (id uuid);\n\
              GRANT INSERT, UPDATE, DELETE ON information_schema.persons TO console_rogue_infoschema;\n",
        error: "topology.canonical_roster_unresolved",
        names: "information_schema.persons",
    },
    Probe {
        // DML held by MEMBERSHIP rather than by a grant. `pg_write_all_data`
        // writes every table in the cluster and appears in no `relacl`.
        label: "write_all_data",
        sql: "DROP ROLE IF EXISTS console_rogue_all_data;\n\
              CREATE ROLE console_rogue_all_data NOLOGIN;\n\
              GRANT pg_write_all_data TO console_rogue_all_data;\n",
        error: "topology.canonical_writer_ownership_failed",
        names: "console_rogue_all_data",
    },
    Probe {
        // The same membership, spelled NOINHERIT. `has_table_privilege` reports
        // FALSE for this role — it must SET ROLE first — and it can, so the
        // census asks whether the candidate can BECOME a holder, not whether it
        // inherits from one.
        label: "noinherit_member",
        sql: "DROP ROLE IF EXISTS console_rogue_noinherit;\n\
              CREATE ROLE console_rogue_noinherit NOLOGIN NOINHERIT;\n\
              GRANT pg_write_all_data TO console_rogue_noinherit;\n",
        error: "topology.canonical_writer_ownership_failed",
        names: "console_rogue_noinherit",
    },
    Probe {
        // A SUPERUSER writes every canonical table and is checked against no
        // ACL at all, so the previous census EXCLUDED superusers and reported
        // clean. The trusted set is now named instead.
        label: "superuser",
        sql: "DROP ROLE IF EXISTS console_rogue_super;\n\
              CREATE ROLE console_rogue_super SUPERUSER NOLOGIN;\n",
        error: "topology.canonical_writer_ownership_failed",
        names: "console_rogue_super",
    },
    Probe {
        label: "table_level",
        sql: "DROP ROLE IF EXISTS console_rogue_table;\n\
              CREATE ROLE console_rogue_table NOLOGIN;\n\
              GRANT UPDATE ON public.employees TO console_rogue_table;\n",
        error: "topology.canonical_writer_ownership_failed",
        names: "console_rogue_table",
    },
    Probe {
        // `has_table_privilege` answers FALSE for a column-only grant, which is
        // why the census asks `has_any_column_privilege` for the two DML
        // privileges PostgreSQL lets a grant scope to a column.
        label: "column_level",
        sql: "DROP ROLE IF EXISTS console_rogue_column;\n\
              CREATE ROLE console_rogue_column NOLOGIN;\n\
              GRANT UPDATE (org_unit) ON public.employees TO console_rogue_column;\n",
        error: "topology.canonical_writer_ownership_failed",
        names: "console_rogue_column",
    },
    Probe {
        // The other branch: TRUNCATE has no column form, so only the table
        // question can see it, and it empties the table.
        label: "truncate_only",
        sql: "DROP ROLE IF EXISTS console_rogue_truncate;\n\
              CREATE ROLE console_rogue_truncate NOLOGIN;\n\
              GRANT TRUNCATE ON public.employees TO console_rogue_truncate;\n",
        error: "topology.canonical_writer_ownership_failed",
        names: "console_rogue_truncate",
    },
    Probe {
        // relkind 'p'. Partitioning is already live in this tree, and a
        // relkind='r' filter drops a partitioned canonical table out of both the
        // REVOKE loop and the census.
        //
        // The plant used to require a roster name NO migration creates, and was
        // moved twice for it: off `employment_heads` when 0214 created that
        // table, onto `job_positions`, which 0215 has now created too. There is
        // no third name — 0215 completed the roster, so "the next roster name
        // that is still absent" is a set that is now permanently empty and
        // moving the plant again is not available to anyone. The plant TAKES the
        // name instead: `DROP TABLE ... CASCADE` first, then recreate it
        // partitioned. That works whether or not a migration created it, so no
        // future lane has to touch this probe again.
        label: "partitioned",
        sql: "DROP ROLE IF EXISTS console_rogue_partition;\n\
              CREATE ROLE console_rogue_partition NOLOGIN;\n\
              DROP TABLE IF EXISTS public.job_positions CASCADE;\n\
              CREATE TABLE public.job_positions (id uuid NOT NULL, org_id uuid NOT NULL)\n\
                PARTITION BY LIST (org_id);\n\
              ALTER TABLE public.job_positions OWNER TO console_app;\n\
              GRANT UPDATE ON public.job_positions TO console_rogue_partition;\n",
        error: "topology.canonical_writer_ownership_failed",
        names: "console_rogue_partition",
    },
    Probe {
        // The CHILD. Rows written through a partition land in the parent, and
        // the child carries a relname the roster does not contain, so it is
        // invisible to any census that matches on names. Reachability through
        // pg_inherits is the only thing that finds it.
        label: "partition_child",
        sql: "DROP ROLE IF EXISTS console_rogue_child;\n\
              CREATE ROLE console_rogue_child NOLOGIN;\n\
              DROP TABLE IF EXISTS public.job_positions CASCADE;\n\
              CREATE TABLE public.job_positions (id uuid NOT NULL, org_id uuid NOT NULL)\n\
                PARTITION BY LIST (org_id);\n\
              ALTER TABLE public.job_positions OWNER TO console_app;\n\
              CREATE TABLE public.job_positions_p1 PARTITION OF public.job_positions\n\
                FOR VALUES IN ('00000000-0000-0000-0000-000000000000');\n\
              ALTER TABLE public.job_positions_p1 OWNER TO console_app;\n\
              GRANT UPDATE ON public.job_positions_p1 TO console_rogue_child;\n",
        error: "topology.canonical_writer_ownership_failed",
        names: "console_rogue_child:job_positions_p1",
    },
    Probe {
        // The owner's DML is implicit: it is not in relacl and REVOKE cannot
        // remove it. The previous census EXCLUDED the owner for exactly that
        // reason, so re-owning a canonical table read as clean.
        label: "owner",
        sql: "DROP ROLE IF EXISTS console_rogue_owner;\n\
              CREATE ROLE console_rogue_owner NOLOGIN;\n\
              ALTER TABLE public.employees OWNER TO console_rogue_owner;\n",
        error: "topology.canonical_writer_ownership_failed",
        names: "console_rogue_owner",
    },
    Probe {
        // A partial loss of the roster. A non-empty count passes this: it falls
        // by one and the run reports success on what is left.
        label: "renamed_away",
        sql: "DROP ROLE IF EXISTS console_rogue_rename;\n\
              CREATE ROLE console_rogue_rename NOLOGIN;\n\
              ALTER TABLE public.employees RENAME TO employees_v2;\n\
              GRANT INSERT, UPDATE, DELETE ON public.employees_v2 TO console_rogue_rename;\n",
        error: "topology.canonical_roster_incomplete",
        names: "employees",
    },
    Probe {
        // Same shrink, spelled so the canonical NAME still resolves: a view is
        // writable through and is not a table.
        label: "view_swap_existing",
        sql: "ALTER TABLE public.employees RENAME TO employees_v2;\n\
              CREATE VIEW public.employees AS SELECT * FROM public.employees_v2;\n",
        error: "topology.canonical_roster_unresolved",
        names: "employees:v",
    },
    Probe {
        // A roster name landing as a VIEW instead of a table. Step 1a's relkind
        // branch is what refuses it, and it refuses it BEFORE the required-set
        // check of step 1c runs, which is why the expected error is
        // `unresolved` and not `incomplete`.
        //
        // Renamed from `view_swap_future`: it was moved off `persons` when 0213
        // created that table and onto `org_units`, which 0215 has now created
        // too. Like `partitioned`, it TAKES the name — `DROP TABLE ... CASCADE`
        // then `CREATE VIEW` — instead of borrowing one no migration has
        // claimed yet, because after 0215 there are none left to borrow.
        label: "view_swap_roster",
        sql: "DROP TABLE IF EXISTS public.org_units CASCADE;\n\
              CREATE VIEW public.org_units AS SELECT 1 AS id;\n",
        error: "topology.canonical_roster_unresolved",
        names: "org_units:v",
    },
];

/// A single-`sed` edit to the reconcile, the probe it must break, and the
/// verdict that probe must then return.
struct Mutation {
    label: &'static str,
    /// `sed` expression applied to a COPY; the committed file is never written.
    expression: &'static str,
    /// [`PROBES`] label, or `""` for the clean migrated database.
    probe: &'static str,
    /// What the probe must report once the control is broken. Every entry but
    /// `drop-expected-writer` flips a detection to a silent pass.
    expect_pass: bool,
    /// Whether this mutation is measured with
    /// `CONSOLE_TOPOLOGY_REQUIRE_CANONICAL_TABLES=1`.
    ///
    /// TRUE for all but two, and the exception is a consequence of migration
    /// 0215 rather than a convenience. Armed, step 1c demands that every
    /// `required_tables` entry exist as an `'r'`/`'p'` relation in `public`, and
    /// as of 0215 that list is the WHOLE roster — so 1c now refuses a roster
    /// name that has been turned into a view or dropped out of the relkind
    /// filter, and it refuses it no matter what step 1a or 1b do. Two controls
    /// therefore have no armed configuration in which breaking them changes the
    /// verdict, and pairing them with a probe that "fails either way" would be a
    /// mutation that has stopped testing what it names.
    ///
    /// Their unique coverage is the UNARMED reconcile — the one every automated
    /// path runs BEFORE migrations, where 1c does not execute at all — so that
    /// is where they are measured. The loop runs the committed script on the
    /// same probe unarmed first and requires it to FAIL, so the flip is between
    /// two runs that differ only in the mutation.
    armed: bool,
}

const MUTATIONS: [Mutation; 14] = [
    Mutation {
        // The owner pin, which the expected-writer ratchet may not widen.
        label: "neuter-owner-check",
        expression: "s/WHERE pg_get_userbyid(relation.relowner) <> ALL (expected_owners)/WHERE false/",
        probe: "owner_is_a_ratcheted_writer",
        expect_pass: true,
        armed: true,
    },
    Mutation {
        // The UP half of reachability.
        label: "no-inheritance-parents",
        expression: "s/SELECT inheritance.inhrelid AS held, inheritance.inhparent AS reached/SELECT NULL::oid AS held, NULL::oid AS reached/",
        probe: "inheritance_parent",
        expect_pass: true,
        armed: true,
    },
    Mutation {
        // The census still runs and still computes `leaked`, but can never raise.
        label: "invert-census",
        expression: "s/IF leaked IS NOT NULL THEN/IF leaked IS NOT NULL AND false THEN/",
        probe: "table_level",
        expect_pass: true,
        armed: true,
    },
    Mutation {
        // The privilege question itself.
        label: "neuter-privilege-question",
        expression: "s/                AND CASE/                AND false AND CASE/",
        probe: "table_level",
        expect_pass: true,
        armed: true,
    },
    Mutation {
        // Ask the TABLE question where a COLUMN grant lives.
        label: "drop-column-privilege-question",
        expression: "s/THEN has_any_column_privilege(holder.oid, relation.oid, privilege.name)/THEN has_table_privilege(holder.oid, relation.oid, privilege.name)/",
        probe: "column_level",
        expect_pass: true,
        armed: true,
    },
    Mutation {
        // ... and the branch that carries DELETE and TRUNCATE.
        label: "neuter-table-privilege-question",
        expression: "s/ELSE has_table_privilege(holder.oid, relation.oid, privilege.name)/ELSE false/",
        probe: "truncate_only",
        expect_pass: true,
        armed: true,
    },
    Mutation {
        // Ask only what the role INHERITS instead of what it can BECOME.
        label: "drop-membership-reachability",
        expression: "s/WHERE pg_has_role(candidate.oid, holder.oid, 'MEMBER')/WHERE candidate.oid = holder.oid/",
        probe: "noinherit_member",
        expect_pass: true,
        armed: true,
    },
    Mutation {
        // The exclusion the previous census actually shipped.
        label: "exclude-superusers",
        expression: "s/AND candidate.rolname <> session_user/AND NOT candidate.rolsuper/",
        probe: "superuser",
        expect_pass: true,
        armed: true,
    },
    Mutation {
        // console_rt loses its ratchet entry, so the CLEAN database must stop
        // passing. This is what proves the clean phase is not vacuous.
        label: "drop-expected-writer",
        expression: "/('console_rt', '\\*'),/d",
        probe: "",
        expect_pass: false,
        armed: true,
    },
    Mutation {
        label: "no-partition-children",
        expression: "s/SELECT inheritance.inhparent AS held, inheritance.inhrelid AS reached/SELECT NULL::oid AS held, NULL::oid AS reached/",
        probe: "partition_child",
        expect_pass: true,
        armed: true,
    },
    Mutation {
        label: "relkind-r-only",
        expression: "s/AND relation.relkind IN ('r', 'p')/AND relation.relkind = 'r'/g",
        probe: "partitioned",
        expect_pass: true,
        armed: false,
    },
    Mutation {
        label: "allow-any-schema",
        expression: "s/HAVING bool_or(namespace.nspname <> 'public')/HAVING bool_or(false)/",
        probe: "information_schema_park",
        expect_pass: true,
        armed: true,
    },
    Mutation {
        label: "allow-any-relkind",
        expression: "s/OR bool_or(relation.relkind NOT IN ('r', 'p'))/OR bool_or(false)/",
        probe: "view_swap_roster",
        expect_pass: true,
        armed: false,
    },
    Mutation {
        label: "empty-required-set",
        expression: "s/FROM unnest(required_tables) AS wanted(name)/FROM unnest(ARRAY[]::TEXT[]) AS wanted(name)/",
        probe: "renamed_away",
        expect_pass: true,
        armed: true,
    },
];

/// A rogue role granted MEMBERSHIP in `console_app`, the owner of every
/// canonical table. It inherits the owner's implicit DML: no `relacl` entry, no
/// REVOKE on the table that removes it, and `relowner` unchanged, so neither the
/// ACL census nor the owner guard can see it.
const MEMBERSHIP_PLANT: &str = "DROP ROLE IF EXISTS console_rogue_member;\n\
     CREATE ROLE console_rogue_member NOLOGIN;\n\
     GRANT console_app TO console_rogue_member;\n";

/// The exact-topology membership REVOKE, turned into a harmless SELECT so that
/// no membership edge is removed. This is the control that actually covers
/// [`MEMBERSHIP_PLANT`].
const KEEP_MEMBERSHIPS: &str = "s/SELECT format('REVOKE %I FROM %I', granted.rolname, member.rolname)/SELECT format('SELECT %L, %L', granted.rolname, member.rolname)/";

/// A copy of the reconcile with one `sed` expression applied, asserted to differ
/// from the committed file so that a moved anchor cannot silently test the
/// original.
fn mutant(
    workdir: &Path,
    label: &str,
    expression: &str,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let path = workdir.join(format!("{label}.sh"));
    let output = Command::new("sed")
        .args([expression, &topology_script().display().to_string()])
        .output()?;
    assert!(
        output.status.success(),
        "{label}: sed failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    std::fs::write(&path, &output.stdout)?;
    assert_ne!(
        std::fs::read(&path)?,
        std::fs::read(topology_script())?,
        "{label}: the mutation changed nothing — its anchor moved, so it has been \
         silently testing the unmutated script"
    );
    Ok(path)
}

// ---------------------------------------------------------------------------
// The executed census
// ---------------------------------------------------------------------------

struct Harness {
    container: Container,
    workdir: PathBuf,
}

impl Harness {
    /// Runs the post-migration enforcement on a fresh probe database.
    /// `extra` is `--skip-migrations`, a path to SQL applied after migrations,
    /// or `None`. `armed` selects
    /// `CONSOLE_TOPOLOGY_REQUIRE_CANONICAL_TABLES`; every phase but the two
    /// unarmed mutations passes `true`, which is what every automated path does
    /// post-migration.
    fn enforce(
        &self,
        probe: &str,
        extra: Option<&str>,
        armed: bool,
    ) -> Result<(bool, String), Box<dyn std::error::Error>> {
        let script = gate_dir()
            .join("canonical-enforce.sh")
            .display()
            .to_string();
        let root = repo_root().display().to_string();
        let mut args = vec![
            script.as_str(),
            root.as_str(),
            self.container.0.as_str(),
            probe,
        ];
        if let Some(extra) = extra {
            args.push(extra);
        }
        run_env(
            "bash",
            &args,
            &[("CANONICAL_REQUIRE_TABLES", if armed { "1" } else { "0" })],
        )
    }

    /// Plants `sql` on its own probe database and enforces.
    fn probe(
        &self,
        label: &str,
        sql: &str,
        armed: bool,
    ) -> Result<(bool, String), Box<dyn std::error::Error>> {
        let file = self.workdir.join(format!("probe-{label}.sql"));
        std::fs::write(&file, sql)?;
        let path = file.display().to_string();
        self.enforce(&format!("census_probe_{label}"), Some(&path), armed)
    }

    /// Replaces the script inside the container. Passing the committed path
    /// restores it.
    fn install_script(&self, path: &Path) -> Fallible {
        let (ok, log) = run(
            "docker",
            &[
                "cp",
                &path.display().to_string(),
                &format!("{}:/topology.sh", self.container.0),
            ],
        )?;
        assert!(ok, "docker cp {} -> /topology.sh: {log}", path.display());
        Ok(())
    }
}

/// Boots one PostgreSQL container with the role topology in place.
fn boot() -> Result<Harness, Box<dyn std::error::Error>> {
    let name = unique("console-canonical-census");
    let database = "console_canonical_census";
    let values = secrets();
    let env_body = format!(
        "POSTGRES_DB={database}\nPOSTGRES_USER=console_census_admin\nPOSTGRES_PASSWORD={0}\n\
         POSTGRES_HOST=127.0.0.1\nPOSTGRES_PORT=5432\n\
         POSTGRES_ADMIN_USER=console_census_admin\nPOSTGRES_ADMIN_PASSWORD={0}\n\
         CONSOLE_APP_POSTGRES_PASSWORD={1}\nCONSOLE_RT_POSTGRES_PASSWORD={2}\n\
         CONSOLE_LEAVE_COMMAND_POSTGRES_PASSWORD={3}\n\
         CONSOLE_ONTOLOGY_COMMAND_POSTGRES_PASSWORD={4}\n\
         CONSOLE_PLATFORM_FORCE_COMMAND_POSTGRES_PASSWORD={5}\n",
        values[0], values[1], values[2], values[3], values[4], values[5]
    );
    let workdir = std::env::temp_dir().join(&name);
    std::fs::create_dir_all(&workdir)?;
    let env_file = workdir.join("topology.env");
    std::fs::write(&env_file, &env_body)?;

    let (started, log) = run(
        "docker",
        &[
            "run",
            "-d",
            "--rm",
            "--name",
            &name,
            "--env-file",
            &env_file.display().to_string(),
            IMAGE,
        ],
    )?;
    assert!(started, "could not start the census container: {log}");
    let container = Container(name);

    let mut healthy = false;
    for _ in 0..60 {
        let (ok, _) = run(
            "docker",
            &[
                "exec",
                &container.0,
                "pg_isready",
                "-h",
                "127.0.0.1",
                "-U",
                "console_census_admin",
                "-d",
                database,
            ],
        )?;
        if ok {
            healthy = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_secs(1));
    }
    assert!(healthy, "postgres never became healthy in {}", container.0);

    for (from, to) in [
        (topology_script().display().to_string(), "/topology.sh"),
        (env_file.display().to_string(), "/topology.env"),
    ] {
        let (ok, log) = run("docker", &["cp", &from, &format!("{}:{to}", container.0)])?;
        assert!(ok, "docker cp {from} -> {to}: {log}");
    }
    Ok(Harness { container, workdir })
}

/// The `[a,b,c]` set the enforcement's own summary line reports.
///
/// Read from the SUMMARY line only, never from the per-step NOTICEs: the
/// pre-migration topology run inside the probe legitimately reports zero, and
/// reading that one would be the very confusion this test exists to reject.
fn examined_set(log: &str) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let line = log
        .lines()
        .find(|line| line.starts_with("canonical-enforce: enforced"))
        .ok_or_else(|| format!("no enforcement summary line in:\n{log}"))?;
    let inside = line
        .split_once('[')
        .and_then(|(_, tail)| tail.split_once(']'))
        .ok_or_else(|| format!("the summary line names no table set: {line}"))?
        .0;
    let mut names: Vec<String> = inside
        .split(',')
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
        .collect();
    names.sort();
    Ok(names)
}

#[test]
fn derived_required_tables_match_the_verbatim_roster() -> Fallible {
    let derived = required_tables_from_topology(&topology_script())?;
    assert_eq!(
        derived,
        EXPECTED_REQUIRED_TABLES.map(str::to_string).to_vec(),
        "the shell `required_tables` array and the verbatim pin must stay the same \
         set; a new canonical table edits BOTH (shell first, then this pin), and \
         this test is the ratchet that refuses a drift"
    );
    Ok(())
}

/// A decoy `required_tables CONSTANT TEXT[] := ARRAY[` plus BEGIN/END markers
/// earlier in the shell script must not win. The production declaration lives
/// inside `DO $canonical$`; parsing the whole file (or the first ARRAY[
/// statement) lets a comment with the pinned 20 names keep the cheap ratchet
/// green while the real SQL array drifts.
#[test]
fn required_tables_parser_ignores_decoy_outside_canonical_block() -> Fallible {
    let path = std::env::temp_dir().join(format!(
        "writer-ownership-decoy-required-tables-{}.sh",
        std::process::id()
    ));
    std::fs::write(
        &path,
        r#"#!/bin/sh
required_tables CONSTANT TEXT[] := ARRAY[
      -- canonical-writer-ownership: BEGIN required tables
      'organizations',
      'employees',
      'persons',
      'person_revisions',
      'employee_person_bindings',
      'employment_heads',
      'employment_revisions',
      'employment_source_bindings',
      'company_revisions',
      'org_units',
      'org_unit_revisions',
      'org_unit_source_bindings',
      'job_positions',
      'job_position_revisions',
      'payroll_draft_runs',
      'payroll_draft_lines',
      'payroll_line_calculations',
      'payroll_run_exceptions',
      'payroll_disbursements',
      'payroll_payslip_deliveries'
      -- canonical-writer-ownership: END required tables
    ];
# Read back by the DO $canonical$ block below
DO $canonical$
DECLARE
    required_tables CONSTANT TEXT[] := ARRAY[
      -- canonical-writer-ownership: BEGIN required tables
      'organizations'
      -- canonical-writer-ownership: END required tables
    ];
END
$canonical$;
"#,
    )?;
    let derived = required_tables_from_topology(&path);
    let _ = std::fs::remove_file(&path);
    assert_eq!(
        derived?,
        vec!["organizations".to_string()],
        "the parser must read the declaration inside DO $canonical$, not a decoy ARRAY[ earlier in the script"
    );
    Ok(())
}

/// A decoy `required_tables CONSTANT TEXT[] := ARRAY[...]` parked inside a
/// block comment AFTER the real array, still inside `DO $canonical$`, must not
/// win. Keeping the LAST textual occurrence would read the commented roster
/// (which PostgreSQL ignores) while the real `required_tables` array drifts.
#[test]
fn required_tables_parser_ignores_decoy_block_comment_after_real_array() -> Fallible {
    let path = std::env::temp_dir().join(format!(
        "writer-ownership-decoy-block-comment-required-tables-{}.sh",
        std::process::id()
    ));
    std::fs::write(
        &path,
        r#"#!/bin/sh
DO $canonical$
DECLARE
    required_tables CONSTANT TEXT[] := ARRAY[
      -- canonical-writer-ownership: BEGIN required tables
      'organizations'
      -- canonical-writer-ownership: END required tables
    ];
    /* PostgreSQL ignores this whole region, so it must not win:
    required_tables CONSTANT TEXT[] := ARRAY[
      -- canonical-writer-ownership: BEGIN required tables
      'organizations',
      'employees',
      'persons'
      -- canonical-writer-ownership: END required tables
    ];
    */
END
$canonical$;
"#,
    )?;
    let derived = required_tables_from_topology(&path);
    let _ = std::fs::remove_file(&path);
    assert_eq!(
        derived?,
        vec!["organizations".to_string()],
        "the parser must read the real code declaration, not a decoy inside a block comment PostgreSQL ignores"
    );
    Ok(())
}

/// An old/example `DO $canonical$ ... $canonical$;` block parked in a block
/// comment BEFORE the live block must not win block selection. A raw
/// `split_once` on the opener/terminator selects the commented copy (whose
/// roster could feed the pin) while the live `required_tables` declaration
/// shrinks.
#[test]
fn required_tables_parser_ignores_commented_block_before_live_block() -> Fallible {
    let path = std::env::temp_dir().join(format!(
        "writer-ownership-decoy-commented-block-{}.sh",
        std::process::id()
    ));
    std::fs::write(
        &path,
        r#"#!/bin/sh
/* an old example block PostgreSQL ignores:
DO $canonical$
DECLARE
    required_tables CONSTANT TEXT[] := ARRAY[
      -- canonical-writer-ownership: BEGIN required tables
      'organizations',
      'employees',
      'persons'
      -- canonical-writer-ownership: END required tables
    ];
END
$canonical$;
*/
DO $canonical$
DECLARE
    required_tables CONSTANT TEXT[] := ARRAY[
      -- canonical-writer-ownership: BEGIN required tables
      'organizations'
      -- canonical-writer-ownership: END required tables
    ];
END
$canonical$;
"#,
    )?;
    let derived = required_tables_from_topology(&path);
    let _ = std::fs::remove_file(&path);
    assert_eq!(
        derived?,
        vec!["organizations".to_string()],
        "the parser must select the live DO block, not a commented old/example copy before it"
    );
    Ok(())
}

/// A required-table literal moved into an ordinary multiline `/* ... */`
/// comment INSIDE the live array must not feed the roster. PostgreSQL omits the
/// entry, so the parser must too — reading it would let the verbatim pin stay
/// green while the census requirement is weakened.
#[test]
fn required_tables_parser_ignores_commented_entry_inside_array() -> Fallible {
    let path = std::env::temp_dir().join(format!(
        "writer-ownership-decoy-commented-entry-{}.sh",
        std::process::id()
    ));
    std::fs::write(
        &path,
        r#"#!/bin/sh
DO $canonical$
DECLARE
    required_tables CONSTANT TEXT[] := ARRAY[
      -- canonical-writer-ownership: BEGIN required tables
      'organizations'
      /*
      'employees'
      */
      -- canonical-writer-ownership: END required tables
    ];
END
$canonical$;
"#,
    )?;
    let derived = required_tables_from_topology(&path);
    let _ = std::fs::remove_file(&path);
    assert_eq!(
        derived?,
        vec!["organizations".to_string()],
        "the parser must ignore a literal moved into a block comment inside the array"
    );
    Ok(())
}

/// A required-table literal parked after an inner `*/` but before the matching
/// outer `*/` must stay masked. PostgreSQL block comments NEST, so the outer
/// comment still swallows that entry; a scanner that stops at the first `*/`
/// would read it and keep the verbatim pin green while the requirement shrinks.
#[test]
fn required_tables_parser_ignores_nested_block_comment_entry() -> Fallible {
    let path = std::env::temp_dir().join(format!(
        "writer-ownership-decoy-nested-comment-{}.sh",
        std::process::id()
    ));
    std::fs::write(
        &path,
        r#"#!/bin/sh
DO $canonical$
DECLARE
    required_tables CONSTANT TEXT[] := ARRAY[
      -- canonical-writer-ownership: BEGIN required tables
      'organizations'
      /* outer
      /* inner */
      'employees'
      */
      -- canonical-writer-ownership: END required tables
    ];
END
$canonical$;
"#,
    )?;
    let derived = required_tables_from_topology(&path);
    let _ = std::fs::remove_file(&path);
    assert_eq!(
        derived?,
        vec!["organizations".to_string()],
        "the parser must respect nested block-comment depth, not stop at the first */"
    );
    Ok(())
}

/// A decoy `required_tables` declaration parked inside a dollar-quoted literal
/// after the real array must not win the last-occurrence selection. PostgreSQL
/// treats the `$$...$$` body as a string (not code), so the parser must mask it
/// too — otherwise the string's pinned roster feeds the pin while the live
/// declaration shrinks.
#[test]
fn required_tables_parser_ignores_declaration_inside_dollar_quote() -> Fallible {
    let path = std::env::temp_dir().join(format!(
        "writer-ownership-decoy-dollar-quote-{}.sh",
        std::process::id()
    ));
    std::fs::write(
        &path,
        r#"#!/bin/sh
DO $canonical$
DECLARE
    required_tables CONSTANT TEXT[] := ARRAY[
      -- canonical-writer-ownership: BEGIN required tables
      'organizations'
      -- canonical-writer-ownership: END required tables
    ];
    example CONSTANT TEXT := $$
required_tables CONSTANT TEXT[] := ARRAY[
  -- canonical-writer-ownership: BEGIN required tables
  'organizations',
  'employees',
  'persons'
  -- canonical-writer-ownership: END required tables
];
$$;
END
$canonical$;
"#,
    )?;
    let derived = required_tables_from_topology(&path);
    let _ = std::fs::remove_file(&path);
    assert_eq!(
        derived?,
        vec!["organizations".to_string()],
        "the parser must mask dollar-quoted literals and select only the live declaration"
    );
    Ok(())
}

/// A decoy `DO $canonical$` block parked in a NON-psql shell heredoc (shell data,
/// not SQL psql executes) must not win block selection. Block selection anchors
/// to the psql `<<'SQL'` heredoc, so the shell-data copy cannot feed the pin.
#[test]
fn required_tables_parser_ignores_decoy_in_non_psql_heredoc() -> Fallible {
    let path = std::env::temp_dir().join(format!(
        "writer-ownership-decoy-non-psql-heredoc-{}.sh",
        std::process::id()
    ));
    std::fs::write(
        &path,
        r#"#!/bin/sh
cat <<'DOC'
DO $canonical$
DECLARE
    required_tables CONSTANT TEXT[] := ARRAY[
      -- canonical-writer-ownership: BEGIN required tables
      'organizations',
      'employees',
      'persons'
      -- canonical-writer-ownership: END required tables
    ];
END
$canonical$;
DOC
psql "${admin_psql_args[@]}" <<'SQL'
BEGIN;
DO $canonical$
DECLARE
    required_tables CONSTANT TEXT[] := ARRAY[
      -- canonical-writer-ownership: BEGIN required tables
      'organizations'
      -- canonical-writer-ownership: END required tables
    ];
END
$canonical$;
SQL
"#,
    )?;
    let derived = required_tables_from_topology(&path);
    let _ = std::fs::remove_file(&path);
    assert_eq!(
        derived?,
        vec!["organizations".to_string()],
        "the parser must anchor to the psql heredoc, not a decoy in shell data"
    );
    Ok(())
}

/// A dollar-quoted literal inside the live array that embeds `'name'` text must
/// not feed the roster: PostgreSQL omits it, so the entry scan must too.
#[test]
fn required_tables_parser_ignores_dollar_quoted_entry_in_array() -> Fallible {
    let path = std::env::temp_dir().join(format!(
        "writer-ownership-decoy-dollar-entry-{}.sh",
        std::process::id()
    ));
    std::fs::write(
        &path,
        r#"#!/bin/sh
DO $canonical$
DECLARE
    required_tables CONSTANT TEXT[] := ARRAY[
      -- canonical-writer-ownership: BEGIN required tables
      'organizations'
      , $$'employees', 'persons'$$
      -- canonical-writer-ownership: END required tables
    ];
END
$canonical$;
"#,
    )?;
    let derived = required_tables_from_topology(&path);
    let _ = std::fs::remove_file(&path);
    assert_eq!(
        derived?,
        vec!["organizations".to_string()],
        "the parser must ignore 'name' text parked inside a dollar-quoted literal in the array"
    );
    Ok(())
}

/// A later nested PL/pgSQL block that declares its own `required_tables` must
/// not win: the enforcement reads the OUTERMOST declaration, so the first code
/// declaration (not the last) is the real one.
#[test]
fn required_tables_parser_ignores_shadowing_nested_declaration() -> Fallible {
    let path = std::env::temp_dir().join(format!(
        "writer-ownership-decoy-shadowing-{}.sh",
        std::process::id()
    ));
    std::fs::write(
        &path,
        r#"#!/bin/sh
DO $canonical$
DECLARE
    required_tables CONSTANT TEXT[] := ARRAY[
      -- canonical-writer-ownership: BEGIN required tables
      'organizations'
      -- canonical-writer-ownership: END required tables
    ];
BEGIN
    IF false THEN
        DECLARE
            required_tables CONSTANT TEXT[] := ARRAY[
              -- canonical-writer-ownership: BEGIN required tables
              'employees',
              'persons'
              -- canonical-writer-ownership: END required tables
            ];
        BEGIN
            NULL;
        END;
    END IF;
END
$canonical$;
"#,
    )?;
    let derived = required_tables_from_topology(&path);
    let _ = std::fs::remove_file(&path);
    assert_eq!(
        derived?,
        vec!["organizations".to_string()],
        "the parser must read the outermost declaration, not a shadowing nested one"
    );
    Ok(())
}

/// A `backup_required_tables CONSTANT TEXT[] := ARRAY[...]` declared BEFORE the
/// real `required_tables` must not win: `required_tables` must match as a full
/// identifier, not as the suffix of a longer one. A bare substring match would
/// read the backup's pinned roster while the enforcement's `required_tables`
/// array drifts.
#[test]
fn required_tables_parser_ignores_backup_suffix_identifier() -> Fallible {
    let path = std::env::temp_dir().join(format!(
        "writer-ownership-decoy-backup-identifier-{}.sh",
        std::process::id()
    ));
    std::fs::write(
        &path,
        r#"#!/bin/sh
DO $canonical$
DECLARE
    backup_required_tables CONSTANT TEXT[] := ARRAY[
      -- canonical-writer-ownership: BEGIN required tables
      'employees',
      'persons'
      -- canonical-writer-ownership: END required tables
    ];
    required_tables CONSTANT TEXT[] := ARRAY[
      -- canonical-writer-ownership: BEGIN required tables
      'organizations'
      -- canonical-writer-ownership: END required tables
    ];
BEGIN
    NULL;
END
$canonical$;
"#,
    )?;
    let derived = required_tables_from_topology(&path);
    let _ = std::fs::remove_file(&path);
    assert_eq!(
        derived?,
        vec!["organizations".to_string()],
        "the parser must match `required_tables` as a full identifier, not as a \
         suffix of `backup_required_tables`"
    );
    Ok(())
}

/// An `E'...'` escape string whose `\'` escape precedes the declaration text
/// must not leak that text as live code: the whole string is masked, so a decoy
/// declaration parked there cannot win.
#[test]
fn required_tables_parser_ignores_e_string_declaration() -> Fallible {
    let path = std::env::temp_dir().join(format!(
        "writer-ownership-decoy-e-string-{}.sh",
        std::process::id()
    ));
    std::fs::write(
        &path,
        r#"#!/bin/sh
DO $canonical$
DECLARE
    required_tables CONSTANT TEXT[] := ARRAY[
      -- canonical-writer-ownership: BEGIN required tables
      'organizations'
      -- canonical-writer-ownership: END required tables
    ];
    example CONSTANT TEXT := E'escaped \' then required_tables CONSTANT TEXT[] := ARRAY[
      -- canonical-writer-ownership: BEGIN required tables
      \'employees\', \'persons\'
      -- canonical-writer-ownership: END required tables
    ];';
END
$canonical$;
"#,
    )?;
    let derived = required_tables_from_topology(&path);
    let _ = std::fs::remove_file(&path);
    assert_eq!(
        derived?,
        vec!["organizations".to_string()],
        "the parser must honor E-string backslash escapes and not leak the decoy declaration"
    );
    Ok(())
}

/// A complete example `DO $canonical$` block parked inside a dollar-quoted SQL
/// value (a `$$...$$` string) before the real block must not win block
/// selection. Block selection masks dollar-quoted literals except the
/// `$canonical$` delimiter itself.
#[test]
fn required_tables_parser_ignores_block_inside_dollar_quote() -> Fallible {
    let path = std::env::temp_dir().join(format!(
        "writer-ownership-decoy-block-dollar-{}.sh",
        std::process::id()
    ));
    std::fs::write(
        &path,
        r#"#!/bin/sh
psql "${admin_psql_args[@]}" <<'SQL'
SELECT $$
DO $canonical$
DECLARE
    required_tables CONSTANT TEXT[] := ARRAY[
      -- canonical-writer-ownership: BEGIN required tables
      'organizations',
      'employees',
      'persons'
      -- canonical-writer-ownership: END required tables
    ];
END
$canonical$;
$$;
DO $canonical$
DECLARE
    required_tables CONSTANT TEXT[] := ARRAY[
      -- canonical-writer-ownership: BEGIN required tables
      'organizations'
      -- canonical-writer-ownership: END required tables
    ];
END
$canonical$;
SQL
"#,
    )?;
    let derived = required_tables_from_topology(&path);
    let _ = std::fs::remove_file(&path);
    assert_eq!(
        derived?,
        vec!["organizations".to_string()],
        "the parser must select the live block, not an example inside a dollar-quoted value"
    );
    Ok(())
}

/// A `cat <<'SQL'` heredoc (same delimiter, non-psql command) parked after the
/// real psql heredoc must not win block selection: only a `psql`-owned heredoc
/// executes SQL.
#[test]
fn required_tables_parser_ignores_non_psql_sql_heredoc() -> Fallible {
    let path = std::env::temp_dir().join(format!(
        "writer-ownership-decoy-cat-sql-heredoc-{}.sh",
        std::process::id()
    ));
    std::fs::write(
        &path,
        r#"#!/bin/sh
psql "${admin_psql_args[@]}" <<'SQL'
BEGIN;
DO $canonical$
DECLARE
    required_tables CONSTANT TEXT[] := ARRAY[
      -- canonical-writer-ownership: BEGIN required tables
      'organizations'
      -- canonical-writer-ownership: END required tables
    ];
END
$canonical$;
SQL
cat <<'SQL'
DO $canonical$
DECLARE
    required_tables CONSTANT TEXT[] := ARRAY[
      -- canonical-writer-ownership: BEGIN required tables
      'organizations',
      'employees',
      'persons'
      -- canonical-writer-ownership: END required tables
    ];
END
$canonical$;
SQL
"#,
    )?;
    let derived = required_tables_from_topology(&path);
    let _ = std::fs::remove_file(&path);
    assert_eq!(
        derived?,
        vec!["organizations".to_string()],
        "the parser must select the psql heredoc, not a later cat <<'SQL' decoy"
    );
    Ok(())
}

/// An entry replaced with an evaluated expression (e.g. a CASE) must make the
/// parser FAIL LOUDLY rather than silently reading every quoted literal: a
/// static scan cannot reproduce PostgreSQL's expression evaluation.
#[test]
fn required_tables_parser_fails_closed_on_expression() -> Fallible {
    let path = std::env::temp_dir().join(format!(
        "writer-ownership-decoy-case-expression-{}.sh",
        std::process::id()
    ));
    std::fs::write(
        &path,
        r#"#!/bin/sh
DO $canonical$
DECLARE
    required_tables CONSTANT TEXT[] := ARRAY[
      -- canonical-writer-ownership: BEGIN required tables
      CASE WHEN false THEN 'employees' ELSE 'organizations' END
      -- canonical-writer-ownership: END required tables
    ];
END
$canonical$;
"#,
    )?;
    let derived = required_tables_from_topology(&path);
    let _ = std::fs::remove_file(&path);
    assert!(
        derived.is_err(),
        "the parser must fail closed on a non-literal expression instead of mis-deriving the roster"
    );
    Ok(())
}

/// A non-psql command that merely CONTAINS the substring `psql` (e.g.
/// `echo psql <<'SQL'`) must not be admitted as the psql heredoc.
#[test]
fn required_tables_parser_ignores_echo_psql_heredoc() -> Fallible {
    let path = std::env::temp_dir().join(format!(
        "writer-ownership-decoy-echo-psql-{}.sh",
        std::process::id()
    ));
    std::fs::write(
        &path,
        r#"#!/bin/sh
psql "${admin_psql_args[@]}" <<'SQL'
BEGIN;
DO $canonical$
DECLARE
    required_tables CONSTANT TEXT[] := ARRAY[
      -- canonical-writer-ownership: BEGIN required tables
      'organizations'
      -- canonical-writer-ownership: END required tables
    ];
END
$canonical$;
SQL
echo psql <<'SQL'
DO $canonical$
DECLARE
    required_tables CONSTANT TEXT[] := ARRAY[
      -- canonical-writer-ownership: BEGIN required tables
      'organizations',
      'employees',
      'persons'
      -- canonical-writer-ownership: END required tables
    ];
END
$canonical$;
SQL
"#,
    )?;
    let derived = required_tables_from_topology(&path);
    let _ = std::fs::remove_file(&path);
    assert_eq!(
        derived?,
        vec!["organizations".to_string()],
        "the parser must select the real psql heredoc, not an echo psql decoy"
    );
    Ok(())
}

/// An earlier valid `DO $canonical$` block with an UNUSED `required_tables`
/// declaration must not win over the later block that actually reads
/// `unnest(required_tables)`.
#[test]
fn required_tables_parser_binds_to_enforcing_block() -> Fallible {
    let path = std::env::temp_dir().join(format!(
        "writer-ownership-decoy-unused-block-{}.sh",
        std::process::id()
    ));
    std::fs::write(
        &path,
        r#"#!/bin/sh
psql "${admin_psql_args[@]}" <<'SQL'
DO $canonical$
DECLARE
    required_tables CONSTANT TEXT[] := ARRAY[
      -- canonical-writer-ownership: BEGIN required tables
      'organizations',
      'employees',
      'persons'
      -- canonical-writer-ownership: END required tables
    ];
BEGIN
    NULL;
END
$canonical$;
DO $canonical$
DECLARE
    required_tables CONSTANT TEXT[] := ARRAY[
      -- canonical-writer-ownership: BEGIN required tables
      'organizations'
      -- canonical-writer-ownership: END required tables
    ];
BEGIN
    PERFORM 1 FROM unnest(required_tables) AS wanted(name);
END
$canonical$;
SQL
"#,
    )?;
    let derived = required_tables_from_topology(&path);
    let _ = std::fs::remove_file(&path);
    assert_eq!(
        derived?,
        vec!["organizations".to_string()],
        "the parser must read the block that enforces unnest(required_tables), not an unused earlier block"
    );
    Ok(())
}

/// A comment (not live SQL) mention of `unnest(required_tables)` must not mark
/// a later helper block as the enforcing one.
#[test]
fn required_tables_parser_ignores_commented_enforcement_mention() -> Fallible {
    let path = std::env::temp_dir().join(format!(
        "writer-ownership-decoy-comment-enforcement-{}.sh",
        std::process::id()
    ));
    std::fs::write(
        &path,
        r#"#!/bin/sh
psql "${admin_psql_args[@]}" <<'SQL'
BEGIN;
DO $canonical$
DECLARE
    required_tables CONSTANT TEXT[] := ARRAY[
      -- canonical-writer-ownership: BEGIN required tables
      'organizations'
      -- canonical-writer-ownership: END required tables
    ];
BEGIN
    PERFORM 1 FROM unnest(required_tables) AS wanted(name);
END
$canonical$;
DO $canonical$
DECLARE
    required_tables CONSTANT TEXT[] := ARRAY[
      -- canonical-writer-ownership: BEGIN required tables
      'organizations',
      'employees',
      'persons'
      -- canonical-writer-ownership: END required tables
    ];
BEGIN
    -- unnest(required_tables) is mentioned only in this comment
    NULL;
END
$canonical$;
SQL
"#,
    )?;
    let derived = required_tables_from_topology(&path);
    let _ = std::fs::remove_file(&path);
    assert_eq!(
        derived?,
        vec!["organizations".to_string()],
        "the parser must bind to the block with LIVE unnest(required_tables), not a comment mention"
    );
    Ok(())
}

/// A `cat <<'DOC'` data heredoc whose body contains example text `psql ...
/// <<'SQL'` must not be misread as a real psql heredoc.
#[test]
fn required_tables_parser_ignores_data_heredoc_psql_text() -> Fallible {
    let path = std::env::temp_dir().join(format!(
        "writer-ownership-decoy-data-heredoc-{}.sh",
        std::process::id()
    ));
    std::fs::write(
        &path,
        r#"#!/bin/sh
cat <<'DOC'
psql "${admin_psql_args[@]}" <<'SQL'
DO $canonical$
DECLARE
    required_tables CONSTANT TEXT[] := ARRAY[
      -- canonical-writer-ownership: BEGIN required tables
      'organizations',
      'employees',
      'persons'
      -- canonical-writer-ownership: END required tables
    ];
END
$canonical$;
SQL
DOC
psql "${admin_psql_args[@]}" <<'SQL'
BEGIN;
DO $canonical$
DECLARE
    required_tables CONSTANT TEXT[] := ARRAY[
      -- canonical-writer-ownership: BEGIN required tables
      'organizations'
      -- canonical-writer-ownership: END required tables
    ];
END
$canonical$;
SQL
"#,
    )?;
    let derived = required_tables_from_topology(&path);
    let _ = std::fs::remove_file(&path);
    assert_eq!(
        derived?,
        vec!["organizations".to_string()],
        "the parser must skip data-heredoc bodies instead of reading psql text inside them"
    );
    Ok(())
}

/// A double-quoted identifier (`"..."`) that embeds the declaration text must
/// not be read as a live declaration.
#[test]
fn required_tables_parser_ignores_double_quoted_identifier() -> Fallible {
    let path = std::env::temp_dir().join(format!(
        "writer-ownership-decoy-quoted-identifier-{}.sh",
        std::process::id()
    ));
    std::fs::write(
        &path,
        r#"#!/bin/sh
DO $canonical$
DECLARE
    "required_tables CONSTANT TEXT[] := ARRAY[
  -- canonical-writer-ownership: BEGIN required tables
  'employees',
  'persons'
  -- canonical-writer-ownership: END required tables
  ];" CONSTANT TEXT;
    required_tables CONSTANT TEXT[] := ARRAY[
      -- canonical-writer-ownership: BEGIN required tables
      'organizations'
      -- canonical-writer-ownership: END required tables
    ];
END
$canonical$;
"#,
    )?;
    let derived = required_tables_from_topology(&path);
    let _ = std::fs::remove_file(&path);
    assert_eq!(
        derived?,
        vec!["organizations".to_string()],
        "the parser must mask double-quoted identifiers, not read the declaration text inside them"
    );
    Ok(())
}

#[test]
fn census_binds_to_an_executed_database() -> Fallible {
    require_docker();
    let harness = boot()?;

    // Phase 1 — zero canonical tables while CLAIMING to enforce them.
    let (ok, log) = harness.enforce("census_probe_fresh", Some("--skip-migrations"), true)?;
    assert!(
        !ok,
        "a run that examines ZERO canonical tables while claiming to enforce them \
         MUST fail — this is exactly the fail-open every automated path shipped:\n{log}"
    );
    assert!(
        log.contains("topology.canonical_enforcement_examined_no_tables"),
        "it must fail for the RIGHT reason, not incidentally:\n{log}"
    );

    // Phase 2 — migrations, then enforcement, over an EXACT set of tables.
    let (ok, clean_log) = harness.enforce("census_probe_clean", None, true)?;
    assert!(
        ok,
        "the measured writer surface of a fully migrated database must pass the \
         census; if `console_rt` is missing from the expected-writer ratchet this \
         is where it shows:\n{clean_log}"
    );
    assert_eq!(
        examined_set(&clean_log)?,
        required_tables_from_topology(&topology_script())?,
        "the census must examine exactly the canonical tables a migrated database \
         has. A count assertion passes while the scope silently shrinks — a rename \
         took it from eight to seven and the run still reported success:\n{clean_log}"
    );

    // Phase 2b — DML held by MEMBERSHIP in the table owner. The reconcile
    // REPAIRS this one rather than rejecting it: the exact-topology REVOKE
    // strips every membership edge touching an application role, and
    // `topology.membership_readback_failed` pins the result — both long before
    // the canonical block runs. A guard for it inside the canonical block would
    // therefore examine nothing, so the proof has to be the mutation: break that
    // REVOKE and the same plant must fail.
    let (ok, log) = harness.probe("membership_repaired", MEMBERSHIP_PLANT, true)?;
    assert!(
        ok,
        "a membership edge into console_app is removed by the reconcile itself, so \
         the run must succeed:\n{log}"
    );
    harness.install_script(&mutant(
        &harness.workdir,
        "keep-memberships",
        KEEP_MEMBERSHIPS,
    )?)?;
    let (ok, log) = harness.probe("membership_kept", MEMBERSHIP_PLANT, true)?;
    harness.install_script(&topology_script())?;
    assert!(
        !ok && log.contains("topology.membership_readback_failed"),
        "with the exact-topology REVOKE broken, a rogue MEMBER of console_app must \
         fail the reconcile — that member inherits the owner's implicit DML on \
         every canonical table. It did not, so nothing removes the edge:\n{log}"
    );

    // Phase 3 — every shape of unaccounted writer, one probe database each.
    for probe in &PROBES {
        let (ok, log) = harness.probe(probe.label, probe.sql, true)?;
        assert!(!ok, "{}: this must fail the reconcile:\n{log}", probe.label);
        assert!(
            log.contains(probe.error),
            "{}: the failure must be `{}`, not something incidental:\n{log}",
            probe.label,
            probe.error
        );
        assert!(
            log.contains(probe.names),
            "{}: the failure must name `{}`:\n{log}",
            probe.label,
            probe.names
        );
    }

    // Phase 4 — break each control and prove the probe that covers it flips.
    for mutation in &MUTATIONS {
        let suffix = mutation.label.replace('-', "_");
        let probe = PROBES.iter().find(|probe| probe.label == mutation.probe);

        // The two unarmed mutations need their own BEFORE picture: phase 3 ran
        // this probe armed, where step 1c refuses it whatever these controls
        // do, so an armed baseline would make the flip a comparison between two
        // different questions. See `Mutation::armed`.
        assert!(
            mutation.armed || probe.is_some(),
            "{}: an unarmed mutation must name a probe — the clean database is not a \
             configuration in which these controls are the only thing looking",
            mutation.label
        );
        if let Some(probe) = probe.filter(|_| !mutation.armed) {
            let (ok, log) = harness.probe(&format!("unarmed_before_{suffix}"), probe.sql, false)?;
            assert!(
                !ok,
                "{}: with the committed script and this control INTACT, the `{}` \
                 probe must fail an UNARMED run — that is the configuration in \
                 which this control is the only thing looking. It passed, so the \
                 mutation below flips nothing:\n{log}",
                mutation.label, probe.label
            );
            assert!(
                log.contains(probe.error),
                "{}: the unarmed baseline must fail as `{}`:\n{log}",
                mutation.label,
                probe.error
            );
        }

        let mutant = mutant(&harness.workdir, mutation.label, mutation.expression)?;
        harness.install_script(&mutant)?;
        let (ok, log) = match probe {
            Some(probe) => harness.probe(&format!("mut_{suffix}"), probe.sql, mutation.armed)?,
            None => harness.enforce(&format!("census_probe_mut_{suffix}"), None, mutation.armed)?,
        };
        harness.install_script(&topology_script())?;

        assert_eq!(
            ok,
            mutation.expect_pass,
            "{}: breaking this control must flip the `{}` probe's verdict. It did \
             not, so nothing here tests that control:\n{log}",
            mutation.label,
            if mutation.probe.is_empty() {
                "clean"
            } else {
                mutation.probe
            }
        );
    }

    let _ = std::fs::remove_dir_all(&harness.workdir);
    Ok(())
}

// ---------------------------------------------------------------------------
// The wiring, proven by RUNNING each harness
// ---------------------------------------------------------------------------

/// The enforcement's own output line. Its presence is proof the enforcement
/// executed; the name of the script that would have produced it is not.
const ENFORCED: &str = "canonical-enforce: enforced";

#[test]
fn pgtest_harness_executes_the_enforcement() -> Fallible {
    require_docker();
    let root = repo_root().display().to_string();
    let script = repo_root()
        .join("tools/lanes/pgtest.sh")
        .display()
        .to_string();
    // `true` as the command under test: the harness still boots PostgreSQL,
    // reconciles and enforces, and nothing is compiled.
    let (ok, log) = run("bash", &[&script, &root, "true"])?;
    assert!(ok, "tools/lanes/pgtest.sh did not complete:\n{log}");
    assert!(
        log.contains(ENFORCED),
        "tools/lanes/pgtest.sh must EXECUTE the canonical enforcement. Commenting \
         the invocation out leaves the literal `canonical-enforce.sh` in the file, \
         so only its output proves it ran:\n{log}"
    );
    Ok(())
}

#[test]
fn cargo_needs_postgres_harness_executes_the_enforcement() -> Fallible {
    require_docker();
    let script = repo_root()
        .join("tools/ci/cargo_needs_postgres.sh")
        .display()
        .to_string();
    // A `--only` name no map entry carries: the harness boots, reconciles and
    // enforces, then exits on an empty selection without building anything.
    let (ok, log) = run(
        "bash",
        &[&script, "--only", "console-canonical-wiring-probe"],
    )?;
    assert!(
        log.contains(ENFORCED),
        "tools/ci/cargo_needs_postgres.sh must EXECUTE the canonical enforcement \
         before it selects targets:\n{log}"
    );
    assert!(
        !ok && log.contains("no map entries selected"),
        "the probe selection is expected to end the run, and did not:\n{log}"
    );
    Ok(())
}
