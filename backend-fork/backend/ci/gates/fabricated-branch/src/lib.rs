//! fabricated-branch gate — **DEFENCE IN DEPTH, NOT A CONTROL.**
//!
//! `console_platform_authz::authorize(principal, action, resource_branch)` checks
//! `principal.branch_scope.allows(resource_branch)` before it reads a matrix cell.
//! That check is only meaningful when `resource_branch` was read off the RESOURCE.
//! Derive it from the principal instead —
//!
//! ```ignore
//! match &principal.branch_scope {
//!     BranchScope::All => BranchId::new(),
//!     BranchScope::Branches(b) => b.iter().next().copied()...,
//! }
//! ```
//!
//! — and the check becomes a TAUTOLOGY on **both** arms: `All` allows any freshly
//! minted id, and `Branches(set).iter().next()` is by definition a member of
//! `set`. The `.iter().any(|b| authorize(.., *b).is_ok())` variant is the same
//! tautology, only wordier: the disjunction ranges over the principal's own
//! branches, so `allows` is true on every iteration. The branch dimension does not
//! fail loudly — it disappears silently.
//!
//! This pattern was copy-pasted across production helpers, and one of them
//! documented it as correct and cited another as precedent. A doc comment is
//! what already failed; hence a gate.
//!
//! # READ THIS BEFORE TRUSTING A GREEN
//!
//! This gate greps. It reads match-arm bodies as text. A pass means only:
//!
//! > none of the literal shapes below appear in an unmarked `BranchScope::` match
//! > arm in a file that also mentions `authorize`.
//!
//! It does NOT mean "no authorization branch in this tree is derived from the
//! principal". Three blind spots are known, and every one of them is a property
//! of scanning text rather than a bug to be fixed:
//!
//! 1. **A fabrication moved one function away scans clean.** The gate reasons
//!    about match-arm bodies, never about what a caller does with the returned
//!    `Option`.
//! 2. **The `Branches` rule matches literal substrings only**, so
//!    `branches.first().copied()`, `.iter().copied().next()`, `.nth(0)` and a
//!    plain `for` loop are invisible.
//! 3. **Detection keys on the literal prefix `BranchScope::`**, so any import
//!    alias defeats it entirely.
//!
//! Do not open a fourth round of pattern-matching against these. Each added
//! pattern is one more shape enumerated and the next shape walks past it; a second
//! lane reached the identical class on the same day with a different scanner. The
//! gate's honest job is to make the three copy-paste shapes that actually occurred
//! impossible to reintroduce silently, and to force every remaining exception to
//! be a sentence a human wrote.
//!
//! ## The control now EXISTS, and this gate is still needed
//!
//! `console_platform_authz::ResourceBranch`
//! (`backend/crates/platform/authz/src/resource_branch.rs`) is the newtype this
//! section used to describe as queued. It wraps a `BranchId` in a
//! module-private field with no constructor that takes one, so a fabricated
//! branch does not compile — including when the fabrication is moved one
//! function away, which is blind spot 1 and is not a text property. The proof is
//! executable: `compile_fail` doctests on the type, paired with compiling
//! controls so that a failure for the wrong reason still shows up as a failure.
//!
//! Its one entrance is `ResourceBranch::lookup(executor, OrgId,
//! BranchScopedResource, id)`. The caller picks its tenant, a resource kind from
//! a closed enum, and a row id; it does not pick the table, the column, the
//! `org_id` predicate or any part of the SQL. That matters
//! because "this uuid came back from Postgres" is a strictly weaker claim than
//! "this is the resource's branch", and the gap between them is where the
//! tautology reappears as a row read — `branch_id` off the principal's own
//! membership row satisfies the first claim and not the second.
//!
//! `authorize_scoped(principal, action, ResourceBranch, at)` is the door that
//! consumes it, and the resource is NOT optional. The branch-less case is
//! `authorize_capability_at(principal, action, at)` — a different function, not
//! an omitted argument, because `None` on a shared door is the one spelling of a
//! fabricated branch that a compiler cannot object to.
//!
//! What is NOT done, and why this gate does not retire yet: `authorize(principal,
//! action, BranchId)` keeps its signature, because changing it is a change to
//! every one of its call sites across ~27 crates and that is one lease, not
//! several. Until those callers move to `authorize_scoped`, a fabricated
//! `BranchId` can still reach `authorize` — and for exactly that surface, these
//! three text rules are all there is.
//!
//! ## Retiring this scan
//!
//! BOTH must hold, and the second is not implied by the first:
//!
//! 1. No `authorize(.., BranchId)` call site remains — every one has moved to
//!    `authorize_scoped`, so no authorization entry point still takes a bare
//!    `BranchId`.
//! 2. No entrance to `ResourceBranch` accepts caller-chosen SQL, a caller-chosen
//!    table, or a caller-chosen column name, and every statement stays bound to
//!    one `org_id` — an unscoped read proves only that a uuid came back from
//!    Postgres, which is true of another tenant's row too. A `from_row(row,
//!    column)` or a `Decode` impl reopens the whole hole while leaving criterion 1 satisfied,
//!    and the text rules here would not see it — neither shape contains a
//!    `BranchScope::` match arm.
//!
//! Check the second by reading `resource_branch.rs`, not by reading this file.
//!
//! ## The fix at a call site is never "add a marker"
//!
//! * The resource HAS a branch → read it off the row and pass THAT to `authorize`.
//! * The resource has NO branch (no `branch_id` column; a list gate whose
//!   confinement is the caller's whole scope) → `authorize_capability`, which
//!   drops the branch dimension visibly.
//! * The action spans EVERY branch → `authorize_org_wide`.
//!
//! ## Exceptions
//!
//! Picking a representative branch is legitimate when it never reaches an
//! authorization decision — choosing a default branch for row *creation*, a
//! `len() == 1` store filter, stamping an actor branch on an audit row.
//!
//! Every one of those needs an inline `// fabricated-branch: ok <reason>` marker
//! on the arm or the line directly above it. A marker is the ONE thing a text
//! scanner can enforce honestly: it cannot tell a legitimate pick from a
//! fabrication, but it can insist that a human wrote down which one this is.
//!
//! An earlier revision inferred the exception instead — a `Branches` arm whose
//! sibling was `BranchScope::All => None` was read as "yields `Option`, therefore
//! safe" and excused without a marker. That inference is blind spot 1 in
//! executable form: an `Option`-yielding match is only safe if no CALLER unwraps
//! it into `authorize`, and this gate never looks at callers. The requirement is
//! restored.
//!
//! ## Scope
//!
//! Rules are substring matches over whitespace-stripped arm bodies, which assumes
//! `cargo fmt`-normalized source — `.github/workflows/ci.yml`'s `rustfmt check`
//! step is a hard prerequisite, not a nicety. Comment lines are ignored so this
//! module's own prose does not trip it.
//!
//! An arm body is therefore read to the END of the arm, including the lines
//! `cargo fmt` wraps a long method chain onto. Reading only the arm's first line
//! is the one failure this gate cannot afford: the fabrication would scan clean in
//! exactly the shape the formatter produces. See [`continues_on_next_line`].

use std::path::{Path, PathBuf};
use std::{collections::BTreeSet, fs};

/// Inline escape hatch for a representative branch that never feeds `authorize`.
const MARKER: &str = "fabricated-branch: ok";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViolationKind {
    /// `BranchScope::All =>` arm minting a `BranchId::new()`.
    FabricatedAllArm,
    /// `BranchScope::Branches(b) =>` arm picking `b`'s own first/any member.
    TautologicalBranchesArm,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Violation {
    pub kind: ViolationKind,
    pub file: PathBuf,
    pub line: usize,
    pub detail: String,
}

impl std::fmt::Display for Violation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "[{:?}] {}:{}: {}",
            self.kind,
            self.file.display(),
            self.line,
            self.detail
        )
    }
}

#[derive(Debug, Default)]
pub struct GateResult {
    pub violations: Vec<Violation>,
    pub files_scanned: usize,
}

impl GateResult {
    #[must_use]
    pub fn passed(&self) -> bool {
        self.violations.is_empty()
    }
}

/// Scan the backend workspace. Errors when the scan root does not resolve or
/// contains no Rust at all — a gate that scanned nothing exits 0 otherwise, which
/// is indistinguishable from a gate that passed.
pub fn check_workspace(workspace_dir: &Path) -> Result<GateResult, String> {
    let result = check_source_tree(workspace_dir)?;
    if result.files_scanned == 0 {
        return Err(format!(
            "scanned 0 Rust files under {} — the scan root did not resolve",
            workspace_dir.display()
        ));
    }
    Ok(result)
}

/// Scan an arbitrary tree. Used by the mutation suite against a throwaway
/// workspace; `check_workspace` adds the scanned-nothing guard on top.
pub fn check_source_tree(root: &Path) -> Result<GateResult, String> {
    let mut result = GateResult::default();
    for file in collect_rust_files(root)? {
        let source = fs::read_to_string(&file)
            .map_err(|e| format!("cannot read {}: {e}", file.display()))?;
        result.files_scanned += 1;
        check_source_file(&file, &source, &mut result);
    }
    Ok(result)
}

fn check_source_file(file: &Path, source: &str, result: &mut GateResult) {
    let lines: Vec<&str> = source.lines().collect();
    // The `.iter().any(|b| authorize(..))` shape is only a tautology when it
    // feeds an authorization decision; a bare membership loop is not this gate's
    // business.
    let touches_authz = source.contains("authorize");

    for (idx, line) in lines.iter().enumerate() {
        if is_comment(line) || marked(&lines, idx) {
            continue;
        }
        let body = arm_body(&lines, idx);

        if line.contains("BranchScope::All =>") && body.contains("BranchId::new()") {
            result.violations.push(Violation {
                kind: ViolationKind::FabricatedAllArm,
                file: file.to_path_buf(),
                line: idx + 1,
                detail: "BranchScope::All arm mints a BranchId that belongs to no \
                         branch; use authorize_capability or authorize_org_wide"
                    .to_owned(),
            });
        }

        if !touches_authz {
            continue;
        }
        if let Some(binding) = branches_arm_binding(line) {
            let picks_own_member = body.contains(&format!("{binding}.iter().next()"))
                || body.contains(&format!("{binding}.iter().any("));
            if picks_own_member {
                result.violations.push(Violation {
                    kind: ViolationKind::TautologicalBranchesArm,
                    file: file.to_path_buf(),
                    line: idx + 1,
                    detail: format!(
                        "BranchScope::Branches({binding}) arm authorizes against a \
                         member of {binding} itself, so branch_scope.allows is \
                         always true; pass the resource's branch or use \
                         authorize_capability"
                    ),
                });
            }
        }
    }
}

fn is_comment(line: &str) -> bool {
    let trimmed = line.trim_start();
    trimmed.starts_with("//") || trimmed.starts_with("*") || trimmed.starts_with("/*")
}

/// A violation is excused by a `// fabricated-branch: ok <reason>` marker on the
/// arm itself or on the line directly above it.
fn marked(lines: &[&str], idx: usize) -> bool {
    lines[idx].contains(MARKER) || (idx > 0 && lines[idx - 1].contains(MARKER))
}

/// The match arm starting at `idx`, whitespace-stripped so rustfmt's line breaks
/// (`branches\n.iter()\n.any(..)`) read as one expression. Comment lines are
/// dropped. Scans forward until brace/paren depth returns to its starting level
/// AND the expression has actually ended, capped so a malformed file cannot walk
/// the whole source.
fn arm_body(lines: &[&str], idx: usize) -> String {
    let mut body = String::new();
    let mut depth = 0i32;
    for (offset, line) in lines.iter().enumerate().skip(idx).take(40) {
        if is_comment(line) {
            continue;
        }
        body.extend(line.chars().filter(|c| !c.is_whitespace()));
        for ch in line.chars() {
            match ch {
                '{' | '(' | '[' => depth += 1,
                '}' | ')' | ']' => depth -= 1,
                _ => {}
            }
        }
        if depth <= 0 && !continues_on_next_line(lines, offset) {
            break;
        }
    }
    body
}

/// Whether the arm's expression carries on past `idx`.
///
/// Depth alone is not the end of an arm. `cargo fmt` wraps a long method chain so
/// the first line closes at paren depth 0 while the expression continues:
///
/// ```ignore
/// BranchScope::Branches(b) => b
///     .iter()
///     .next()
/// ```
///
/// Stopping at depth 0 reads the arm as just `b`, and the fabrication scans clean
/// — in the exact shape the formatter produces, which is the shape the next one
/// will arrive in. A continuation line begins with `.` or `?`; a match arm pattern
/// never does, so resuming on one cannot swallow the following arm.
fn continues_on_next_line(lines: &[&str], idx: usize) -> bool {
    lines
        .iter()
        .skip(idx + 1)
        .take(5)
        .find(|line| !is_comment(line) && !line.trim().is_empty())
        .is_some_and(|line| {
            let next = line.trim_start();
            next.starts_with('.') || next.starts_with('?')
        })
}

/// `BranchScope::Branches(branches) =>` / `Branches(branches) if .. =>` → the
/// binding name. Returns `None` for wildcard or destructuring patterns, which
/// cannot name a member to pick.
fn branches_arm_binding(line: &str) -> Option<&str> {
    let rest = line.split_once("BranchScope::Branches(")?.1;
    let (binding, after) = rest.split_once(')')?;
    if !after.contains("=>") {
        return None;
    }
    let binding = binding.trim();
    let valid = !binding.is_empty()
        && binding
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_')
        && !binding.starts_with(|c: char| c.is_ascii_digit());
    valid.then_some(binding)
}

fn collect_rust_files(root: &Path) -> Result<Vec<PathBuf>, String> {
    let mut files = BTreeSet::new();
    collect_rust_files_inner(root, &mut files)?;
    Ok(files.into_iter().collect())
}

fn collect_rust_files_inner(dir: &Path, files: &mut BTreeSet<PathBuf>) -> Result<(), String> {
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
            files.insert(path);
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

#[cfg(test)]
mod tests {
    use super::*;

    fn scan(source: &str) -> GateResult {
        let mut result = GateResult::default();
        check_source_file(Path::new("src/lib.rs"), source, &mut result);
        result
    }

    #[test]
    fn flags_the_all_arm_that_mints_a_branch() {
        let result = scan(
            r#"
fn representative_branch(principal: &Principal) -> Result<BranchId, E> {
    match &principal.branch_scope {
        BranchScope::All => Ok(BranchId::new()),
        BranchScope::Branches(branches) => branches.iter().next().copied().ok_or(E),
    }
}
fn call() { authorize(p, a, b) }
"#,
        );
        assert_eq!(result.violations.len(), 2, "{:#?}", result.violations);
        assert_eq!(result.violations[0].kind, ViolationKind::FabricatedAllArm);
        assert_eq!(
            result.violations[1].kind,
            ViolationKind::TautologicalBranchesArm
        );
    }

    /// The wordier form. `hr.rs` used it; a reviewer read it as stricter than
    /// `.next()` when it is exactly as vacuous.
    #[test]
    fn flags_the_any_over_own_branches_form() {
        let result = scan(
            r#"
fn authorize_scoped(principal: &Principal, feature: Feature) -> Result<(), E> {
    match &principal.branch_scope {
        BranchScope::Branches(branches) => {
            if branches
                .iter()
                .any(|branch| authorize(principal, action, *branch).is_ok())
            {
                Ok(())
            } else {
                Err(E)
            }
        }
        BranchScope::All => Ok(()),
    }
}
"#,
        );
        assert_eq!(result.violations.len(), 1, "{:#?}", result.violations);
        assert_eq!(
            result.violations[0].kind,
            ViolationKind::TautologicalBranchesArm
        );
    }

    /// THE BYPASS. `cargo fmt` wraps a long chain so the arm's first line closes
    /// at paren depth 0; a body read that stopped there saw only `branches` and
    /// scanned this clean, exit 0. This is `app/src/lib.rs::authorize_audit_read`
    /// verbatim as it stood before the migration — the formatter's own output, so
    /// the shape any future fabrication arrives in.
    #[test]
    fn flags_the_rustfmt_wrapped_branches_arm() {
        let result = scan(
            r#"
fn authorize_audit_read(principal: &Principal) -> Result<(), ApiError> {
    let resource_branch = match &principal.branch_scope {
        BranchScope::All => BranchId::new(),
        BranchScope::Branches(branches) => branches
            .iter()
            .next()
            .copied()
            .ok_or_else(|| ApiError::forbidden("principal has no branch scope"))?,
    };
    authorize(
        principal,
        Action::new(Feature::AuditLogRead),
        resource_branch,
    )
    .map_err(ApiError::from_kernel)
}
"#,
        );
        assert_eq!(result.violations.len(), 2, "{:#?}", result.violations);
        assert_eq!(result.violations[0].kind, ViolationKind::FabricatedAllArm);
        assert_eq!(
            result.violations[1].kind,
            ViolationKind::TautologicalBranchesArm
        );
    }

    /// The same wrap on the `.any()` form, with no `All` arm to fall back on.
    #[test]
    fn flags_the_rustfmt_wrapped_any_arm() {
        let result = scan(
            r#"
fn require(principal: &Principal, action: Action) -> Result<(), E> {
    match &principal.branch_scope {
        BranchScope::All => authorize_org_wide(principal, action),
        BranchScope::Branches(branches) => branches
            .iter()
            .any(|branch| authorize(principal, action, *branch).is_ok())
            .then_some(())
            .ok_or(E),
    }
}
"#,
        );
        assert_eq!(result.violations.len(), 1, "{:#?}", result.violations);
        assert_eq!(
            result.violations[0].kind,
            ViolationKind::TautologicalBranchesArm
        );
    }

    /// Reading to the end of an arm must not read INTO the next one: an honest
    /// `Branches` arm beside a minting `All` arm is one violation, not two.
    #[test]
    fn does_not_read_one_arm_into_the_next() {
        let result = scan(
            r#"
fn pick(principal: &Principal, id: Id) -> BranchId {
    match &principal.branch_scope {
        BranchScope::All => BranchId::new(),
        BranchScope::Branches(branches) => resource_branch_of(id),
    }
}
fn call() { authorize(p, a, b) }
"#,
        );
        assert_eq!(result.violations.len(), 1, "{:#?}", result.violations);
        assert_eq!(result.violations[0].kind, ViolationKind::FabricatedAllArm);
    }

    /// THE RESTORED MARKER REQUIREMENT. An earlier revision excused this shape
    /// automatically: sibling `All => None` ⇒ the match yields `Option<BranchId>`
    /// ⇒ assumed safe. That inference is blind spot 1 — the gate never looks at
    /// what the CALLER does with the `Option`, and `pick(p).unwrap()` fed to
    /// `authorize` one function away is the same fabrication. An unmarked pick is
    /// flagged again, `All => None` or not.
    #[test]
    fn an_option_yielding_pick_still_needs_a_marker() {
        let result = scan(
            r#"
fn principal_create_branch(principal: &Principal) -> Option<BranchId> {
    match &principal.branch_scope {
        BranchScope::All => None,
        BranchScope::Branches(branches) => branches.iter().next().copied(),
    }
}
fn create(p: &Principal) { authorize(p, a, principal_create_branch(p).unwrap()) }
"#,
        );
        assert_eq!(result.violations.len(), 1, "{:#?}", result.violations);
        assert_eq!(
            result.violations[0].kind,
            ViolationKind::TautologicalBranchesArm
        );
    }

    /// The same shape with the human-written sentence attached is clean. This is
    /// `registry/rest::principal_create_branch`'s intended end state.
    #[test]
    fn a_marked_option_yielding_pick_is_clean() {
        let result = scan(
            r#"
fn principal_create_branch(principal: &Principal) -> Option<BranchId> {
    match &principal.branch_scope {
        BranchScope::All => None,
        // fabricated-branch: ok default branch for row CREATION; never reaches authorize
        BranchScope::Branches(branches) => branches.iter().next().copied(),
    }
}
fn create(p: &Principal) { authorize(p, a, row.branch_id) }
"#,
        );
        assert!(result.passed(), "{:#?}", result.violations);
    }

    /// An arm that calls `authorize` itself is the tautology outright, and a
    /// sibling `All => None` never had anything to say about it.
    #[test]
    fn an_arm_that_authorizes_is_flagged_beside_an_all_none_sibling() {
        let result = scan(
            r#"
fn require(principal: &Principal, action: Action) -> Option<()> {
    match &principal.branch_scope {
        BranchScope::All => None,
        BranchScope::Branches(branches) => branches
            .iter()
            .any(|branch| authorize(principal, action, *branch).is_ok())
            .then_some(()),
    }
}
"#,
        );
        assert_eq!(result.violations.len(), 1, "{:#?}", result.violations);
        assert_eq!(
            result.violations[0].kind,
            ViolationKind::TautologicalBranchesArm
        );
    }

    /// An `All` arm that DENIES leaves the `Branches` arm free to feed
    /// `authorize` a member of its own set.
    #[test]
    fn a_denying_all_arm_does_not_excuse_the_branches_arm() {
        let result = scan(
            r#"
fn representative(principal: &Principal) -> Result<BranchId, E> {
    match &principal.branch_scope {
        BranchScope::All => Err(E::forbidden()),
        BranchScope::Branches(branches) => branches.iter().next().copied().ok_or(E::forbidden()),
    }
}
fn call() { authorize(p, a, representative(p)?) }
"#,
        );
        assert_eq!(result.violations.len(), 1, "{:#?}", result.violations);
        assert_eq!(
            result.violations[0].kind,
            ViolationKind::TautologicalBranchesArm
        );
    }

    #[test]
    fn flags_the_multi_line_all_arm() {
        let result = scan(
            r#"
fn pick(principal: &Principal) -> BranchId {
    match &principal.branch_scope {
        BranchScope::All => {
            BranchId::new()
        }
        BranchScope::Branches(_) => authorize_somehow(),
    }
}
"#,
        );
        assert_eq!(result.violations.len(), 1, "{:#?}", result.violations);
        assert_eq!(result.violations[0].kind, ViolationKind::FabricatedAllArm);
    }

    #[test]
    fn accepts_a_marked_exception() {
        let result = scan(
            r#"
fn audit_event_branch(scope: &BranchScope) -> Option<BranchId> {
    match scope {
        BranchScope::All => None,
        // fabricated-branch: ok stamps the actor's branch on an audit row, never authorize
        BranchScope::Branches(branches) => branches.iter().next().copied(),
    }
}
fn other() { authorize(p, a, b) }
"#,
        );
        assert!(result.passed(), "{:#?}", result.violations);
    }

    /// The whole point of the primitive: the honest branch-less form is clean.
    #[test]
    fn accepts_authorize_capability() {
        let result = scan(
            r#"
fn require_feature(principal: &Principal, feature: Feature) -> Result<(), E> {
    authorize_capability(principal, Action::new(feature)).map_err(E::from_kernel)
}
"#,
        );
        assert!(result.passed(), "{:#?}", result.violations);
    }

    /// And so is threading a real resource branch.
    #[test]
    fn accepts_a_resource_branch_read_off_the_row() {
        let result = scan(
            r#"
async fn complete(state: &S, principal: &Principal, id: Id) -> Result<(), E> {
    let branch_id = state.store.schedule_branch_in_scope(id, &principal.branch_scope).await?;
    authorize(principal, Action::new(Feature::X), branch_id).map_err(E::from_kernel)
}
"#,
        );
        assert!(result.passed(), "{:#?}", result.violations);
    }

    /// This gate's own prose, and `authorize`'s doc comment, describe the shape
    /// they forbid. Documenting an anti-pattern must not trip it.
    #[test]
    fn ignores_the_pattern_inside_comments() {
        let result = scan(
            r#"
/// Never write `BranchScope::All => BranchId::new()`, and never write
/// `BranchScope::Branches(b) => b.iter().next()` — both are tautologies.
// BranchScope::All => Ok(BranchId::new()),
fn authorize_capability_doc() {}
"#,
        );
        assert!(result.passed(), "{:#?}", result.violations);
    }

    /// A membership loop that never reaches `authorize` is not this gate's
    /// business.
    #[test]
    fn ignores_branch_picking_in_a_file_with_no_authorization() {
        let result = scan(
            r#"
fn first_branch(scope: &BranchScope) -> Option<BranchId> {
    match scope {
        BranchScope::All => None,
        BranchScope::Branches(branches) => branches.iter().next().copied(),
    }
}
"#,
        );
        assert!(result.passed(), "{:#?}", result.violations);
    }

    // The scanned-nothing guard is proven in
    // `tests/gate_detects_violation.rs::gate_errors_when_it_walked_zero_rust_files`,
    // which needs a real temp tree; it is not duplicated here.
}
