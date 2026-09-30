//! Writer-ownership gate: the STATIC half.
//!
//! Exactly one production crate may hold DML against each canonical-object
//! table. The roster comes from `console_ontology_canonical_domain::ObjectKey`,
//! so a seventh object key cannot skip this gate — it arrives with an owner
//! crate and its tables or the roster test fails.
//!
//! ## WHICH HALF ENFORCES WHAT — read this before trusting either
//!
//! The two halves do NOT enforce the same rule, and an earlier version of this
//! doc claimed they did.
//!
//! * The DATABASE half in `ops/postgres-reconcile-topology.sh` enforces
//!   **ROLE-level** ownership. It revokes table DML on the canonical tables from
//!   the three command roles, takes a deny-by-default census of every remaining
//!   role — asking `has_table_privilege`/`has_any_column_privilege` rather than
//!   unioning catalogs — and separately pins the table OWNER, which the census's
//!   write ratchet may not widen.
//!   That is real, it is enforced by Postgres after parsing, and
//!   `tests/census_executes_against_postgres.rs` proves it against a live
//!   database. It caught `console_leave_definer`'s INSERT/UPDATE on
//!   `public.employees`.
//!
//! * It CANNOT see crates. `deploy/apps/console/base/backend.yaml:62` and
//!   `worker.yaml:53` both connect the runtime as `console_rt`, and the
//!   reconcile whitelists `('console_rt','*')`. Every crate in the application
//!   is therefore the same database principal: the database half cannot tell
//!   `console-payroll-adapter-postgres` from `console-leave-adapter-postgres`.
//!
//! * So **this static half is the only crate-level boundary that exists today**.
//!   It is load-bearing, not defence-in-depth. Per-crate database roles would
//!   make the database half crate-aware; that is specified as follow-up work,
//!   not done here.
//!
//! ## What this half is TOTAL over, and what it is not
//!
//! It is total over the CFG QUESTION. An item is skipped only when some
//! `#[cfg(…)]` on it is UNSATISFIABLE with `test = false`; everything else is
//! production code and is scanned. That is one decidable rule over the whole
//! predicate language — `all`, `any`, `not`, and any operator this gate does not
//! model, which evaluates to `Maybe` and therefore SCANS. There is no list of
//! spellings to keep in sync, which is what the four rounds before this one
//! spent themselves on. See [`cfg_truth`].
//!
//! It is total over LEXICAL SHAPE, because it does not lex. The file is
//! parsed with `syn::parse_file` and real items are walked, so `'{'`,
//! `/* { */`, `r#"a"b{"#`, a multi-line string literal, `} // end tests` and an
//! attribute with its item on one line are all simply not questions any more. A
//! file that does NOT parse is scanned in full rather than skipped, and so is a
//! file holding a token the walk cannot render at all.
//!
//! It is total over the NODE KIND, and there is no list of them either. The
//! walk reads two things: a literal, and a raw TOKEN STREAM. `syn::visit`
//! funnels every token stream in the grammar — the eight `Verbatim` arms
//! (`Item`, `ImplItem`, `TraitItem`, `ForeignItem`, `Expr`, `Type`, `Pat`,
//! `TypeParamBound`), a macro's arguments and an attribute's `Meta::List` body
//! — through the single `Visit::visit_token_stream`, whose default body is
//! `{}`. Overriding that one method replaced four hand-written `Verbatim` arms
//! that named only the four ITEM positions, under which a `become` tail call,
//! `builtin #`, a `box` pattern and a `dyn*` type each carried a full `UPDATE`
//! past the gate. See [`Production::visit_token_stream`].
//!
//! This is opt-OUT, not opt-in, and it is NOT a flattening of every AST node:
//! flattened, ordinary Rust reads as SQL. `struct Update { employees: u8 }`
//! would resolve `employees` as the target of an `UPDATE`, and
//! `#[derive(Debug, Clone, Copy)]` above a `"organizations"` literal resolved a
//! `COPY` against the real tree until a token STREAM was made to TERMINATE a
//! statement. Both are pinned by
//! `flattening_every_token_into_the_statement_reading_manufactures_writes`.
//! The end of a stream is the ONLY such boundary — see [`Production::tokens`],
//! which is where the stream is SEALED into its own statement and the one `;`
//! is emitted. A GROUP inside a stream is not one.
//! Terminating attribute bodies alone was tried and reopened the same false
//! positive for `m!(Debug, Clone, Copy);`, `matches!(op, Op::Update)` and
//! `quote! { #[derive(Copy)] }` — ordinary Rust in streams no attribute body
//! reaches. The cost of the boundary is stated with it, in [`Production::tokens`]
//! and in
//! `a_write_whose_target_position_holds_a_placeholder_or_a_literal_boundary_is_charged_as_unresolved`;
//! what it does NOT cover is residual 9 below.
//!
//! Exactly ONE thing is still decided by looking at tokens, and it is named
//! rather than implied: the body of a `doc` attribute is dropped. That is ONE
//! rule — [`is_doc_attribute`] — called from both positions an attribute can
//! appear in: at item level ([`Production::visit_attribute`]) and inside a
//! token stream ([`Production::tokens`], via [`is_doc_attribute_body`]). It was
//! spelled twice, and the second spelling asked a different question: the
//! attribute-macro invocation `#[doc::hidden("UPDATE …")]` is compiled code,
//! and it was dropped inside a macro body and read at item level. Everything
//! else a `#` can introduce — `quote!`'s `#( … )*` repetition, `#var` interpolation,
//! `#[derive(…)]` and every non-`doc` attribute — is walked, because a rule
//! keyed on `#` alone is a token-shape rule and drops real code. An earlier
//! version of this gate applied the narrow rule inside macros and the broad one
//! at item level, which hid a full `UPDATE` behind `#[my_attr("…")]`.
//!
//! It is NOT total over the CRATE BOUNDARY. The boundary is drawn from the
//! manifest tree, and a target assembled at runtime, generated by a macro
//! defined elsewhere, or written from a non-Rust caller is outside it. Those
//! residuals are enumerated below because they are gaps, not coverage.
//!
//! ## How this half reads a write
//!
//! Two questions, two readings of the same items, and conflating them has now
//! cost a round in each direction. See [`Production`].
//!
//! A STATEMENT reaches the driver only inside a string literal — `"…"`, `b"…"`,
//! `c"…"` and their raw forms — or inside a token stream handed verbatim to a
//! macro. Both spellings of that second carrier are read: a `foo!(…)`
//! invocation, and an attribute, which is how an attribute proc-macro and a
//! derive helper receive their tokens. Nothing else in Rust can carry one, so
//! nothing else is read as SQL.
//!
//! A table NAME is not so confined. `pub mod employees`, `struct Employees` and
//! `use schema::employees` all name the table without carrying any statement, so
//! the unresolved fallback below searches identifiers too. Narrowing that
//! reading to literals was a detection regression: a builder assembling
//! `UPDATE {t}` against a table it names as a Rust identifier shipped clean.
//!
//! A `doc` attribute is not code, at item level or inside a macro body: `///`
//! and `//!` ARE `#[doc = "…"]`, and this repository's own doc comments quote
//! the statements this gate hunts for. Every other attribute IS code and is
//! read in both readings.
//!
//! Each literal and each token stream is ONE statement, read on its own rather
//! than concatenated with the rest of the file — see [`file_write_targets`].
//! Within one, whitespace is collapsed, so a line-broken statement reads as one
//! line. A SQL COMMENT is resolved earlier still, inside the one literal that
//! holds it ([`without_sql_comments`]), because that is the only place its
//! extent is known: a `--` ends at the newline in its own literal, and a
//! `/*…*/` that closes, closes in its own literal. So a comment is gone by the
//! time the verb is searched for, wherever it stood —
//! `UPDATE/*c*/employees`, `INSERT/*c*/INTO employees` and
//! `UPDATE -- hint⏎employees` are all read as the writes PostgreSQL executes.
//! One that does NOT close inside its literal is left standing, and is charged
//! as an unknown target below. The TARGET POSITION is then READ rather than
//! matched: quoting, `ONLY`, `TABLE`, any schema qualifier and spaces around the
//! qualifier dot are followed to the table's last identifier component.
//!
//! When the target cannot be read, the write is charged to every canonical
//! table the same file names: an unreadable target is an UNKNOWN table, not the
//! absence of one, so it fails CLOSED. That is narrower than "every unreadable
//! target", and the difference is what STANDS there, not when the string was
//! built. [`write_targets`] reaches the fallback only when the verb arrived with
//! its trailing space AND the target position holds an [`UNREADABLE_TARGET`]
//! marker. That position is located ONCE, by [`skip_target_noise`], and the same
//! offset is handed to the identifier read and to the marker test; reading it
//! twice is what let `UPDATE ONLY {t}` through for four rounds. So
//! `format!("UPDATE {t} …")`, `format!("UPDATE ONLY {t} …")`, `push("UPDATE ")`,
//! `push("UPDATE ONLY ")`, `concat!("UPDATE ", …)` and a block comment that
//! opens where the table should be and never closes INSIDE ITS LITERAL all fail
//! closed, while a
//! split that cuts the verb and a split that cuts the TARGET are charged
//! NOTHING. Both are residual 9 below, each constructed and run.
//!
//! ## RESIDUAL EVASIONS OF THIS HALF — enumerated, not claimed covered
//!
//! Since nothing behind it catches a crate-level violation, these are live gaps,
//! each of which lets a non-owner crate write a canonical table unseen:
//!
//! 1. **A write that never names the table in the same file.** The unresolved
//!    fallback charges an unreadable target to the canonical tables the FILE
//!    names; a table name imported from another module, read from config, or
//!    built from fragments (`"emplo" + "yees"`) leaves nothing to charge.
//! 2. **A macro that generates the statement.** The gate walks the macro's
//!    tokens as written, not as expanded, so `write_to!(employees, …)` expanding
//!    to an `UPDATE` is invisible unless the macro body itself lives in the
//!    offending crate.
//! 3. **A second-hand constant.** `format!("UPDATE {}", other_crate::TABLE)`
//!    resolves nothing and names nothing; only the crate DEFINING the constant
//!    is charged.
//! 4. **A new crate that is not linked into anything shipped.** Such a crate is
//!    skipped by design (see [`CrateTree::is_unshipped`]); the moment something
//!    depends on it, it is scanned — but a lane that adds both edges in one
//!    change is only caught on the dependency edge.
//! 5. **Non-Rust writers.** Only `*.rs` is walked. A shell script, a psql
//!    invocation from an ops runbook, or a migration is not seen here at all;
//!    those are the database half's job, and against them it is role-level only.
//! 6. **`ALTER TABLE`, `CREATE TRIGGER`, functions and rules.** The verb list is
//!    DML only, so a `SECURITY DEFINER` function body that writes is invisible
//!    to this half.
//! 7. **SQL that is not a literal in the crate.** `include_str!("x.sql")`
//!    contributes the PATH, not the statement.
//! 8. **A literal spelling `syn` does not yet classify.** `Lit` is
//!    `#[non_exhaustive]`, and the wildcard does not drop: a variant this `syn`
//!    cannot classify marks the file UNREADABLE and the raw text is scanned,
//!    the same answer a file `syn` cannot parse gets. That path is
//!    UNREACHABLE with the pinned `syn` — every variant it has is named — so it
//!    carries no test and is a claim about a future `syn`, not a measured
//!    control. `tests/gate_detects_violation.rs` pins the six spellings Rust
//!    has today. The sibling hole in the token stream is a different question
//!    and IS measured: see [`Production::visit_token_stream`].
//! 9. **A write that misses either condition [`write_targets`] charges on.**
//!    The statement reading is TEXT, and it charges only when BOTH hold:
//!    (1) the verb and the space after it arrive contiguously (`update `), and
//!    (2) the target position — located once by [`skip_target_noise`], which
//!    consumes any `only `/`table `/`/*…*/` — either reads as an identifier
//!    whose last component is the exact name of a canonical table, or fails to
//!    read as an identifier while what stands there is an [`UNREADABLE_TARGET`]
//!    marker. Miss either and NOTHING is charged. Which condition each spelling misses
//!    was measured by printing the read text, not inferred from the shape of
//!    the source; all of them are constructed and run in
//!    `known_residual_a_write_that_misses_either_charging_condition_is_not_charged`:
//!
//!    * missing (1) — the verb split across literals (`concat!("UPD", "ATE
//!      employees …")`) and a verb literal that ends at the verb
//!      (`push("UPDATE")` then `push(" ")`). Both cut inside `update `;
//!    * missing (2) with (1) held — a bare-token verb, which the token walk
//!      renders WITH its trailing space and whose next byte is punctuation:
//!      `stringify!` produces that inside one `concat!` (a `,`) and at the
//!      stream boundary of [`Production::tokens`] (a `;`). Also a split inside
//!      the TARGET (`concat!("UPDATE employ", "ees …")`), which RESOLVES to the
//!      non-canonical `employ` so the unresolved arm never runs.
//!
//!    The axis is therefore neither WHEN the string is assembled — `push("UPDATE
//!    ")` is runtime and IS charged, `concat!("UPDATE ", "employees …")` is
//!    compile time and IS charged — nor the stream boundary, which is one
//!    spelling of the second miss and not its cause. The bare-token family is a
//!    specialisation of residual 2, tokens read as written rather than as
//!    expanded. Charging a verb at the boundary would close the `;` spelling
//!    only, and was measured and rejected even for that: it manufactures false
//!    positives from `#[q(mode = Update)]` and `matches!(op, Op::Update)`.
//!    Widening [`UNREADABLE_TARGET`] does not close the cases above either: `,`
//!    and `;` were tried and DO break the real tree, because ordinary Rust puts
//!    them after a bare verb token. What they want is a reading this half does
//!    not have — the EXPANDED tokens, and a target matched as a substring of a
//!    canonical name rather than as a whole identifier.
//!
//!    What used to be filed here and is NOT any more: `UPDATE ONLY {t}`,
//!    `TRUNCATE TABLE {t}`, `DELETE FROM ONLY {t}`, `push("UPDATE ONLY ")` and
//!    an inline `/*…*/` between verb and target. Those escaped because the
//!    target position was read twice and the two readings disagreed, which is a
//!    bug and not a boundary; [`skip_target_noise`] closed all five with no new
//!    violation on the real tree.
//!
//! 10. **A `--` comment that runs off the END of its literal, in a statement
//!     assembled from more than one.** A comment is resolved inside the literal
//!     that holds it ([`without_sql_comments`]), and a `--` with no newline left
//!     in that literal has taken the literal's own closing quote with it. What
//!     stands in the target position is then a `;` rather than the `"` that
//!     would fail closed, because the rest of the LINE is comment too — which is
//!     true of this literal and ASSUMED of the next one. So
//!     `push("UPDATE -- hint")` followed by `push("⏎employees SET x = 1")` is a
//!     real write and is charged nothing, and it is the price of not charging
//!     `"truncate --size=0 …"` and `"cargo update --workspace --locked"`, which
//!     end the same way and are not writes at all
//!     (`a_command_line_flag_after_a_verb_word_is_not_a_write`). Modelling it
//!     exactly needs the fragments joined, which is residual 9's reading, not
//!     this one.
//!
//!     What used to be filed here and is NOT any more: a comment standing where
//!     the statement expects a SEPARATOR (`UPDATE/*c*/employees …`,
//!     `UPDATE--c⏎employees …`, `UPDATE ONLY/*c*/employees …`,
//!     `INSERT/*c*/INTO employees …`) and a `--` standing where it expects the
//!     TABLE (`UPDATE -- hint⏎employees …`). Those were filed on the argument
//!     that resolving comments would have to run over the whole flattened
//!     statement, where a `--` in one literal eats a real statement in the next
//!     one (`m!("a--b", "UPDATE employees SET x = 1")`). The argument was about
//!     the wrong GRANULARITY rather than about the fix: per LITERAL, a `--`
//!     ends at the newline it is written with and a `/*…*/` closes where it is
//!     written to close. All five are now charged, in
//!     `a_comment_where_the_statement_expects_a_separator_or_a_table_is_charged`
//!     and in `gate_detects_every_measured_evasion`, with no new violation on
//!     the real tree.
//!
//!     Two things this did NOT do. `--` is still not in [`UNREADABLE_TARGET`]:
//!     the marker test sees the two bytes standing at the position and no more,
//!     and `--` is the commonest flag prefix in shell and prose, so it charged
//!     `truncate --size=0 …` in a crate that merely NAMES a canonical table.
//!     And a comment is resolved in ONE literal only, never across two, which is
//!     what keeps a `/*` from closing on a `*/` in an unrelated later literal —
//!     `a_block_comment_does_not_close_on_a_later_literals_text`.
//!
//! 11. **CLOSED — `--` / `/*` inside single-quoted SQL data.** When
//!     [`sql_comment_extent_needs_lexer`] says the fragment needs it,
//!     [`without_sql_comments`] tokenizes with `sqlparser` (PostgreSQL dialect)
//!     so `'a -- b'` and `'a /* b'` stay string data and a later
//!     `UPDATE employees …` in the same Rust literal is charged. Pinned by
//!     `known_residual_a_dash_inside_quoted_sql_data_hides_a_later_statement`
//!     (includes the `/*`-in-quotes sibling). All other fragments keep the byte
//!     path unchanged.
//!
//!     Historical note: a raw byte scan could not decide comment extent inside
//!     quoted strings; Fix A charged ordinary Rust (`cargo update --workspace`);
//!     Fix B re-opened the separator/target family. Dropping comment resolution
//!     re-opens residual 10's separator/target family (`UPDATE/*c*/employees …`
//!     and four more), and putting `--` in [`UNREADABLE_TARGET`] charges
//!     ordinary Rust — `"truncate --size=0 /var/log/app.log"` and
//!     `"cargo update --workspace --locked"` — in any crate that merely NAMES a
//!     canonical table. The total primitive is a real SQL lexer, which is a new
//!     backend dependency (`grep -c sqlparser backend/Cargo.lock` is 0) and a
//!     separate decision from this reading.
//!
//! Deliberately NOT modelled on `backend/ci/gates/fabricated-branch`, whose own
//! module doc concedes its text rules do not see a rephrased call. This gate is
//! no longer in that family for the two questions above, and still is for the
//! crate boundary.
//!
//! ## Excluded surfaces (handoff line 69: migrations and tests are out)
//!
//! * any path under a `tests/`, `migrations/`, `benches/`, `examples/` or
//!   `target/` directory,
//! * items, impl items and trait items inside `src/` whose `#[cfg(…)]` cannot
//!   hold with `test = false`. A cfg on a STATEMENT or an expression is not
//!   read, so such an item is SCANNED — over-scanning, not a fail-open,
//! * crates that cannot reach production, decided by [`CrateTree::is_unshipped`]
//!   from the manifest tree rather than from the crate's name.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use console_ontology_canonical_domain::{ObjectKey, ReceiptOwner};

/// One production-source DML statement against a canonical-object table held by
/// a crate that does not own it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Violation {
    pub table: String,
    pub owner_crate: String,
    pub offending_crate: String,
    pub path: String,
}

/// One production-source DML statement against a SHARED authority table held by
/// a crate that is not permitted to write it.
///
/// Kept separate from [`Violation`] deliberately: a shared table has no single
/// owner, so there is no `owner_crate` to name, and — more importantly — these
/// are NOT ratchetable. [`KNOWN_SECOND_WRITERS`] cannot exempt one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SharedWriteViolation {
    pub table: String,
    pub permitted_crates: Vec<&'static str>,
    pub offending_crate: String,
    pub path: String,
}

#[derive(Debug, Default)]
pub struct Report {
    pub violations: Vec<Violation>,
    /// Writes to a shared authority table by a crate outside its permitted set.
    pub shared_violations: Vec<SharedWriteViolation>,
    /// `(table, crate)` for every PERMITTED crate actually observed writing a
    /// shared table. Feeds [`Report::stale_permitted_writers`].
    pub shared_writers: BTreeSet<(&'static str, String)>,
    pub scanned_files: usize,
}

impl Report {
    /// True when the tree holds no second writer at all — the end state.
    #[must_use]
    pub fn passed(&self) -> bool {
        self.violations.is_empty() && self.shared_violations.is_empty()
    }

    /// Second writers that are NOT in [`KNOWN_SECOND_WRITERS`]. Any of these
    /// fails the gate.
    #[must_use]
    pub fn unknown(&self) -> Vec<&Violation> {
        self.violations
            .iter()
            .filter(|violation| {
                !KNOWN_SECOND_WRITERS.iter().any(|known| {
                    known.table == violation.table
                        && known.offending_crate == violation.offending_crate
                })
            })
            .collect()
    }

    /// Ratchet entries that no longer describe a real violation. These fail the
    /// gate too: a landed port lane must delete its entry, so the list can only
    /// shrink.
    #[must_use]
    pub fn stale_exemptions(&self) -> Vec<&'static KnownSecondWriter> {
        KNOWN_SECOND_WRITERS
            .iter()
            .filter(|known| {
                !self.violations.iter().any(|violation| {
                    known.table == violation.table
                        && known.offending_crate == violation.offending_crate
                })
            })
            .collect()
    }
}

impl Report {
    /// Permitted writers that write nothing.
    ///
    /// This is the half that makes the shared roster a RATCHET rather than a
    /// growing allowance. [`Report::unknown`] and [`Report::stale_exemptions`]
    /// together make `KNOWN_SECOND_WRITERS` able only to shrink; a permitted-
    /// writer set derived from a roster has the opposite bias — it grows
    /// silently, because declaring a seventh object key would grant its owner
    /// crate DML on the shared store with no gate edit and no failing test.
    ///
    /// So a permitted crate that holds no write against the table fails the
    /// gate. The permission and the write must both exist, or neither does.
    #[must_use]
    pub fn stale_permitted_writers(&self) -> Vec<(&'static str, &'static str)> {
        let mut stale = Vec::new();
        for rule in shared_authority_roster() {
            for permitted in &rule.permitted_crates {
                if !self
                    .shared_writers
                    .iter()
                    .any(|(table, crate_name)| *table == rule.table && crate_name == permitted)
                {
                    stale.push((rule.table, *permitted));
                }
            }
        }
        stale
    }
}

/// A table with no single owner, written by several named crates and by nobody
/// else.
///
/// The receipt store is the case this exists for: it is the authority record
/// for every dispatch target, so each object's owner writes ITS rows there. That
/// is not a single-writer table and pretending otherwise would mean collapsing
/// six owners into one crate — measured as a false green, because
/// `scan`'s `owner_of` map would become constant and could never charge a
/// cross-object write again.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SharedAuthorityRule {
    pub table: &'static str,
    pub permitted_crates: Vec<&'static str>,
}

/// The shared-authority roster, derived from [`ReceiptOwner::ALL`] so it cannot
/// go stale against the writer-ownership registry.
#[must_use]
pub fn shared_authority_roster() -> Vec<SharedAuthorityRule> {
    let mut permitted: Vec<&'static str> = ReceiptOwner::ALL
        .iter()
        .map(|owner| owner.writer_crate())
        .collect();
    permitted.sort_unstable();
    permitted.dedup();
    vec![SharedAuthorityRule {
        table: ReceiptOwner::RECEIPT_STORE,
        permitted_crates: permitted,
    }]
}

/// One second writer that already exists, named exactly, with the lane that
/// removes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KnownSecondWriter {
    pub table: &'static str,
    pub offending_crate: &'static str,
    pub source: &'static str,
    pub removed_by: &'static str,
}

/// The dual writers measured in this tree at the time this gate landed.
///
/// **It is now EMPTY, and that is the result P4 set out to prove: every table
/// the contract assigns to a canonical object has exactly ONE production writer
/// crate, and it is the crate the contract names.**
///
/// This is a RATCHET, not an exemption list: [`Report::unknown`] rejects any
/// writer not named here, and [`Report::stale_exemptions`] rejects any entry
/// that no longer matches, so the list can only shrink. It is keyed on
/// (table, crate) — not on a source spelling — so rewording the SQL does not
/// move an entry off the ratchet. Empty, [`Report::unknown`] degenerates to
/// "every violation fails the gate", which is the rule this list was always
/// buying time against.
///
/// Both original entries are gone, each deleted in the same commit that removed
/// the writer it named — [`Report::stale_exemptions`] rejects an entry without
/// its writer and [`Report::unknown`] rejects a writer without its entry, so
/// neither half could move alone:
///
/// * `employees` / `console-app` — deleted by console-kmb.
///   `backend/app/src/hr.rs` held three statements and now holds none;
///   `console-ontology-canonical-adapter-postgres`, the contract's
///   `ObjectKey::Employment` owner, holds them in `src/employment.rs`
///   (retargeted from the interim `console-orgchange-adapter-postgres` owner by
///   console-1qw.4).
/// * `payroll_draft_runs` / `console-workflow-runtime-adapter-postgres` —
///   deleted by console-0hq. The JOB outbox drain held one `INSERT` and now
///   holds none; `console-payroll-adapter-postgres`, the contract's
///   `ObjectKey::PayRun` owner, holds it in `src/pay_run.rs` and the drain
///   reaches it through `console_workflow_domain::PayrollDraftStaging` — a port
///   in a DOMAIN crate, because `backend/ci/gates/layer-boundary` forbids one
///   adapter depending on another, so "just call the owner" would have traded
///   this violation for that one.
///
/// An empty ratchet is load-bearing on the TESTS, not just on this constant:
/// every assertion of the form `x.len() == KNOWN_SECOND_WRITERS.len()` is
/// `0 == 0` now and passes vacuously. `tests/gate_detects_violation.rs` states
/// its expectations against planted trees with their own counts instead, so the
/// gate is still proven to DETECT rather than merely to agree with an empty
/// list.
pub const KNOWN_SECOND_WRITERS: &[KnownSecondWriter] = &[];

/// One object's writer-ownership rule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnershipRule {
    pub object: ObjectKey,
    pub owner_crate: &'static str,
    pub tables: Vec<&'static str>,
}

/// The roster the gate loops over, derived from the single `ObjectKey::ALL`
/// constant so it cannot go stale.
#[must_use]
pub fn ownership_roster() -> Vec<OwnershipRule> {
    ObjectKey::ALL
        .iter()
        .map(|object| OwnershipRule {
            object: *object,
            owner_crate: object.owner_crate(),
            tables: object.owned_tables().to_vec(),
        })
        .collect()
}

const EXCLUDED_DIRS: &[&str] = &[
    "target",
    "tests",
    "migrations",
    "benches",
    "examples",
    ".git",
];

/// Scans `root` for production DML against a canonical-object table held by a
/// crate that does not own it.
///
/// # Errors
/// Returns an error if the tree cannot be walked or a source file cannot be read.
pub fn scan(root: &Path) -> Result<Report, std::io::Error> {
    let owner_of: BTreeMap<&'static str, &'static str> = ObjectKey::ALL
        .iter()
        .flat_map(|object| {
            object
                .owned_tables()
                .iter()
                .map(move |table| (*table, object.owner_crate()))
        })
        .collect();

    // Shared authority tables have no single owner, so they are charged by a
    // permitted-SET rule rather than by `owner_of`.
    let shared = shared_authority_roster();

    let mut report = Report::default();
    let mut sources = Vec::new();
    let mut manifests = Vec::new();
    collect_sources(root, &mut sources, &mut manifests)?;
    sources.sort();
    let tree = CrateTree::read(root, &manifests)?;

    for path in sources {
        let Some((crate_name, manifest_dir)) = owning_crate(&path, root) else {
            continue;
        };
        if tree.is_unshipped(&crate_name, &manifest_dir, root) {
            continue;
        }
        let text = std::fs::read_to_string(&path)?;
        report.scanned_files += 1;
        let production = production_source(&text);
        let names = collapsed_lower(&production.names);
        let targets = file_write_targets(&production.statements);

        let mut hit: BTreeSet<&'static str> = BTreeSet::new();
        for table in &targets.tables {
            if let Some((owned, owner)) = owner_of.get_key_value(table.as_str())
                && **owner != crate_name
            {
                hit.insert(owned);
            }
        }
        // A write whose target could not be read is a write to an UNKNOWN table,
        // not to no table. Charging it to every canonical table the same file
        // names is what stops `format!("UPDATE {t} ...")` and a split whose
        // first fragment ends after the verb's space — `push("UPDATE ")`,
        // `concat!("UPDATE ", ...)` — from passing as clean. It is NOT every
        // unreadable target: `write_targets` sets this flag only for the
        // `UNREADABLE_TARGET` markers, so a split inside the TARGET never
        // reaches here.
        // That is residual 9 in the module doc, constructed and run. The
        // NAME is searched in the wider reading: a table named only as a module,
        // a type or a `use` path names it just as well.
        if targets.unresolved {
            for (owned, owner) in &owner_of {
                if **owner != crate_name && names_table(&names, owned) {
                    hit.insert(owned);
                }
            }
        }

        // Shared-store pass. Runs on the same extracted targets as the owned
        // pass, including the unresolved-target fallback: a write whose target
        // could not be read is a write to an UNKNOWN table, and the shared store
        // must not be the one place that reasoning stops applying.
        for rule in &shared {
            let named_directly = targets.tables.iter().any(|table| table == rule.table);
            let named_unresolved = targets.unresolved && names_table(&names, rule.table);
            if !(named_directly || named_unresolved) {
                continue;
            }
            if rule.permitted_crates.contains(&crate_name.as_str()) {
                report
                    .shared_writers
                    .insert((rule.table, crate_name.clone()));
            } else {
                report.shared_violations.push(SharedWriteViolation {
                    table: rule.table.to_owned(),
                    permitted_crates: rule.permitted_crates.clone(),
                    offending_crate: crate_name.clone(),
                    path: path.display().to_string(),
                });
            }
        }

        for table in hit {
            report.violations.push(Violation {
                table: table.to_owned(),
                owner_crate: (*owner_of[table]).to_owned(),
                offending_crate: crate_name.clone(),
                path: path.display().to_string(),
            });
        }
    }
    Ok(report)
}

fn collect_sources(
    dir: &Path,
    out: &mut Vec<PathBuf>,
    manifests: &mut Vec<PathBuf>,
) -> Result<(), std::io::Error> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let file_type = entry.file_type()?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if file_type.is_dir() {
            if EXCLUDED_DIRS.contains(&name.as_ref()) {
                continue;
            }
            collect_sources(&path, out, manifests)?;
        } else if file_type.is_file() {
            if name.ends_with(".rs") {
                out.push(path);
            } else if name == "Cargo.toml" {
                manifests.push(path);
            }
        }
    }
    Ok(())
}

/// Nearest ancestor `Cargo.toml` package name and the directory holding it,
/// searching no higher than `root`.
fn owning_crate(source: &Path, root: &Path) -> Option<(String, PathBuf)> {
    let mut dir = source.parent();
    while let Some(current) = dir {
        let manifest = current.join("Cargo.toml");
        if manifest.is_file()
            && let Ok(text) = std::fs::read_to_string(&manifest)
            && let Some(name) = package_name(&text)
        {
            return Some((name, current.to_path_buf()));
        }
        if current == root {
            break;
        }
        dir = current.parent();
    }
    None
}

fn package_name(manifest: &str) -> Option<String> {
    let mut in_package = false;
    for line in manifest.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_package = line == "[package]";
            continue;
        }
        if in_package
            && let Some(rest) = line.strip_prefix("name")
            && let Some(rest) = rest.trim_start().strip_prefix('=')
        {
            return Some(rest.trim().trim_matches('"').to_owned());
        }
    }
    None
}

/// Which crates in the tree are actually linked into something that ships.
///
/// The two exclusions this replaces were keyed on the crate's NAME —
/// `starts_with("console-gate-")` and `ends_with("-test-support")` — so a crate
/// could opt itself out of the gate by choosing a name. `ci/gates/*` is a
/// workspace member glob, which made a `console-gate-*` crate holding a real
/// `UPDATE` invisible. Both rules are now keyed on something the build already
/// enforces: where the manifest lives, and whether anything depends on the
/// crate outside `[dev-dependencies]`.
#[derive(Debug, Default)]
struct CrateTree {
    /// Crate names appearing in a `[dependencies]`, `[build-dependencies]` or
    /// `[target.*.dependencies]` table somewhere in the tree. A crate here is
    /// linked into whatever depends on it.
    shipped_dependencies: BTreeSet<String>,
    /// Crate names appearing in a `[dev-dependencies]` table.
    dev_dependencies: BTreeSet<String>,
}

impl CrateTree {
    /// Prefer Cargo-authoritative metadata when `root` is a Cargo project.
    /// Planted synthetic trees (gate tests) have no root manifest — those use
    /// the key-only text scan, which deliberately does **not** resolve
    /// `package =` renames (console-ugg: text-scan rename spellings are retired).
    fn read(root: &Path, manifests: &[PathBuf]) -> Result<Self, std::io::Error> {
        match Self::from_cargo_metadata(root) {
            Ok(tree) => Ok(tree),
            Err(meta_err) if root.join("Cargo.toml").is_file() => Err(std::io::Error::other(
                format!("cargo metadata required for writer-ownership crate graph: {meta_err}"),
            )),
            Err(_) => Self::from_manifest_text(manifests),
        }
    }

    fn from_cargo_metadata(root: &Path) -> Result<Self, String> {
        let output = std::process::Command::new("cargo")
            .args(["metadata", "--format-version", "1", "--no-deps"])
            .current_dir(root)
            .output()
            .map_err(|e| format!("failed to run `cargo metadata`: {e}"))?;
        if !output.status.success() {
            return Err(format!(
                "`cargo metadata` failed:\n{}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }
        let meta: serde_json::Value = serde_json::from_slice(&output.stdout)
            .map_err(|e| format!("failed to parse cargo metadata JSON: {e}"))?;
        let packages = meta["packages"]
            .as_array()
            .ok_or("cargo metadata JSON has no `packages` array")?;
        let mut tree = Self::default();
        for package in packages {
            let Some(deps) = package["dependencies"].as_array() else {
                continue;
            };
            for dep in deps {
                let Some(name) = dep["name"].as_str() else {
                    continue;
                };
                let is_dev = dep["kind"].as_str() == Some("dev");
                let set = if is_dev {
                    &mut tree.dev_dependencies
                } else {
                    &mut tree.shipped_dependencies
                };
                // `name` is the resolved package (Cargo's answer to every
                // `package =` / workspace-inherited rename spelling).
                set.insert(name.to_owned());
                // The Cargo.toml key survives too — that is how the crate is
                // referred to in code when renamed.
                if let Some(rename) = dep["rename"].as_str() {
                    set.insert(rename.to_owned());
                }
            }
        }
        Ok(tree)
    }

    fn from_manifest_text(manifests: &[PathBuf]) -> Result<Self, std::io::Error> {
        let mut tree = Self::default();
        for manifest in manifests {
            let text = std::fs::read_to_string(manifest)?;
            let (shipped, dev) = dependency_edges(&text);
            tree.shipped_dependencies.extend(shipped);
            tree.dev_dependencies.extend(dev);
        }
        Ok(tree)
    }

    /// True when this crate cannot reach production. Exactly two rules, both
    /// keyed on something the build already enforces:
    ///
    /// 1. its manifest lives under `ci/gates/` — the workspace's `ci/gates/*`
    ///    member glob, which is also where the CI runner looks for gate
    ///    binaries;
    /// 2. every dependency edge pointing at it in this tree is a
    ///    `[dev-dependencies]` edge, so it is linked into test binaries only.
    ///
    /// Rule 2 requires at least one edge. A crate nothing references at all is
    /// SCANNED, not exempted: "nobody depends on me yet" is how a new writer
    /// would arrive, and it is also the shape of every synthetic tree in
    /// `tests/gate_detects_violation.rs`.
    ///
    /// Neither rule can be bought by renaming. The previous rules —
    /// `starts_with("console-gate-")` and `ends_with("-test-support")` — both
    /// could, and the first made a `console-gate-*` crate holding a real
    /// `UPDATE` invisible even though `ci/gates/*` is a workspace member.
    fn is_unshipped(&self, crate_name: &str, manifest_dir: &Path, root: &Path) -> bool {
        is_ci_gate(manifest_dir, root)
            || (self.dev_dependencies.contains(crate_name)
                && !self.shipped_dependencies.contains(crate_name))
    }
}

/// `<root>/…/ci/gates/<crate>` — the workspace's `ci/gates/*` member glob.
fn is_ci_gate(manifest_dir: &Path, root: &Path) -> bool {
    let Ok(relative) = manifest_dir.strip_prefix(root) else {
        return false;
    };
    let parts: Vec<_> = relative
        .components()
        .map(|part| part.as_os_str().to_string_lossy().into_owned())
        .collect();
    parts
        .windows(2)
        .any(|pair| pair[0] == "ci" && pair[1] == "gates")
}

/// Key-only dependency edges from one manifest text.
///
/// Used only for planted synthetic trees that are not a Cargo project.
/// Rename resolution (`package =`, workspace-inherited aliases) is **not**
/// done here — [`CrateTree::from_cargo_metadata`] is the total primitive
/// (console-ugg). A `package =` directive line is skipped so it cannot invent
/// a crate named `package`.
///
/// `[workspace.dependencies]` is NOT an edge — it is the workspace's version
/// registry, and every dev-dependency in the tree is also listed there. Reading
/// it as an edge is what would make `console-platform-test-support` look
/// production-linked.
fn dependency_edges(manifest: &str) -> (BTreeSet<String>, BTreeSet<String>) {
    let mut shipped = BTreeSet::new();
    let mut dev = BTreeSet::new();
    let mut table: Option<bool> = None; // Some(is_dev)
    for line in manifest.lines() {
        let line = line.trim();
        if let Some(header) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            let header = header.trim_start_matches('[').trim_end_matches(']');
            table = classify_dependency_table(header).map(|(is_dev, named)| {
                if let Some(dependency) = named {
                    // `[dependencies.foo]` names the crate in the header.
                    if is_dev { &mut dev } else { &mut shipped }
                        .insert(dependency.trim_matches('"').to_owned());
                }
                is_dev
            });
            continue;
        }
        let Some(is_dev) = table else { continue };
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        // `name = …` and `name.workspace = true` both key on the same prefix.
        let key = line
            .split('=')
            .next()
            .unwrap_or_default()
            .split('.')
            .next()
            .unwrap_or_default()
            .trim();
        // Rename directive — never a dependency named `package`. Real package
        // names come from `cargo metadata`, not another text-scan branch.
        if key == "package" || key.is_empty() {
            continue;
        }
        if is_dev { &mut dev } else { &mut shipped }.insert(key.trim_matches('"').to_owned());
    }
    (shipped, dev)
}

/// `(is_dev, the crate named by the header itself)` for a dependency table, or
/// `None` when the header is not one.
fn classify_dependency_table(header: &str) -> Option<(bool, Option<&str>)> {
    if header == "workspace.dependencies" || header.starts_with("workspace.") {
        return None;
    }
    let is_dev_table =
        |name: &str| name == "dev-dependencies" || name.ends_with(".dev-dependencies");
    let is_table = |name: &str| {
        name == "dependencies"
            || name == "build-dependencies"
            || name.ends_with(".dependencies")
            || name.ends_with(".build-dependencies")
            || is_dev_table(name)
    };
    if is_table(header) {
        return Some((is_dev_table(header), None));
    }
    // `[dependencies.foo]` / `[target.'cfg(x)'.dev-dependencies.foo]`
    let (prefix, dependency) = header.rsplit_once('.')?;
    if is_table(prefix) {
        return Some((is_dev_table(prefix), Some(dependency)));
    }
    None
}

/// Three-valued truth of a `cfg` predicate evaluated with `test = false` — the
/// production build. `Maybe` is the honest answer for a predicate whose value
/// depends on a feature, a target or an operator this gate does not model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Truth {
    /// Cannot hold in a production build.
    Never,
    /// May hold, so the item may ship.
    Maybe,
    /// Holds in every production build.
    Always,
}

impl Truth {
    const fn negate(self) -> Self {
        match self {
            Self::Never => Self::Always,
            Self::Maybe => Self::Maybe,
            Self::Always => Self::Never,
        }
    }

    const fn and(self, other: Self) -> Self {
        match (self, other) {
            (Self::Never, _) | (_, Self::Never) => Self::Never,
            (Self::Always, Self::Always) => Self::Always,
            _ => Self::Maybe,
        }
    }

    const fn or(self, other: Self) -> Self {
        match (self, other) {
            (Self::Always, _) | (_, Self::Always) => Self::Always,
            (Self::Never, Self::Never) => Self::Never,
            _ => Self::Maybe,
        }
    }
}

/// Evaluates a `cfg` predicate with `test = false`.
///
/// This is the ENTIRE rule the previous four rounds spent on a list of
/// spellings, and the list lost a round to each new one — `not(all(test))`,
/// `not(any(test, …))`, `not( test )` — before the fix that inverted it:
/// `any(test, X)` is TRUE in a production build whenever `X` holds, so items
/// under it SHIP and dropping them from the scan is a fail-open.
///
/// Anything unmodelled — an unknown operator, a predicate that will not parse —
/// is [`Truth::Maybe`], which SCANS. There is nothing here to keep in sync.
fn cfg_truth(meta: &syn::Meta) -> Truth {
    match meta {
        syn::Meta::Path(path) if path.is_ident("test") => Truth::Never,
        syn::Meta::List(list) => {
            let Ok(inner) = list.parse_args_with(
                syn::punctuated::Punctuated::<syn::Meta, syn::Token![,]>::parse_terminated,
            ) else {
                return Truth::Maybe;
            };
            let mut truths = inner.iter().map(cfg_truth);
            if list.path.is_ident("not") {
                truths.next().map_or(Truth::Maybe, Truth::negate)
            } else if list.path.is_ident("all") {
                truths.fold(Truth::Always, Truth::and)
            } else if list.path.is_ident("any") {
                truths.fold(Truth::Never, Truth::or)
            } else {
                Truth::Maybe
            }
        }
        _ => Truth::Maybe,
    }
}

/// True when these attributes gate their item to TEST builds only — that is,
/// when some `#[cfg(…)]` on it is UNSATISFIABLE with `test = false`.
///
/// Everything else is production code and is SCANNED, including every negation
/// of `test`, every `any(test, …)`, and every predicate this gate cannot read.
fn gated_to_test_builds(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|attribute| {
        let syn::Meta::List(list) = &attribute.meta else {
            return false;
        };
        list.path.is_ident("cfg")
            && list
                .parse_args::<syn::Meta>()
                .map_or(Truth::Maybe, |predicate| cfg_truth(&predicate))
                == Truth::Never
    })
}

/// `syn::Item` is `#[non_exhaustive]`, so an unrecognised item reports NO
/// attributes and is therefore SCANNED.
fn item_attrs(item: &syn::Item) -> &[syn::Attribute] {
    match item {
        syn::Item::Const(node) => &node.attrs,
        syn::Item::Enum(node) => &node.attrs,
        syn::Item::ExternCrate(node) => &node.attrs,
        syn::Item::Fn(node) => &node.attrs,
        syn::Item::ForeignMod(node) => &node.attrs,
        syn::Item::Impl(node) => &node.attrs,
        syn::Item::Macro(node) => &node.attrs,
        syn::Item::Mod(node) => &node.attrs,
        syn::Item::Static(node) => &node.attrs,
        syn::Item::Struct(node) => &node.attrs,
        syn::Item::Trait(node) => &node.attrs,
        syn::Item::TraitAlias(node) => &node.attrs,
        syn::Item::Type(node) => &node.attrs,
        syn::Item::Union(node) => &node.attrs,
        syn::Item::Use(node) => &node.attrs,
        _ => &[],
    }
}

fn impl_item_attrs(item: &syn::ImplItem) -> &[syn::Attribute] {
    match item {
        syn::ImplItem::Const(node) => &node.attrs,
        syn::ImplItem::Fn(node) => &node.attrs,
        syn::ImplItem::Macro(node) => &node.attrs,
        syn::ImplItem::Type(node) => &node.attrs,
        _ => &[],
    }
}

fn trait_item_attrs(item: &syn::TraitItem) -> &[syn::Attribute] {
    match item {
        syn::TraitItem::Const(node) => &node.attrs,
        syn::TraitItem::Fn(node) => &node.attrs,
        syn::TraitItem::Macro(node) => &node.attrs,
        syn::TraitItem::Type(node) => &node.attrs,
        _ => &[],
    }
}

/// The text of the items that reach a production build, in the TWO readings the
/// gate actually needs. Conflating them has now cost a round in each direction.
///
/// * [`Production::statements`] is what can carry a STATEMENT to the driver: a
///   string literal, or a macro's token stream. Nothing else in Rust can. One
///   entry per carrier — with ONE exception, stated because a reader who
///   assumes otherwise will re-derive a bound that is not there: for a file
///   `syn` cannot parse, [`production_source`] puts the WHOLE FILE into a single
///   entry. Nothing downstream depends on the split, because a comment's extent
///   is resolved earlier, inside one literal ([`without_sql_comments`]), and no
///   later reading follows a comment at all. Each literal is re-emitted WITH its
///   quotes, because the quote is what tells `UPDATE "employees"` (a resolvable
///   target) from `push("UPDATE ") … push(t)` (an unresolvable one).
/// * [`Production::names`] is what can carry a table NAME, which is a strictly
///   larger question and the one the unresolved fallback asks. `pub mod
///   employees`, `struct Employees` and `use schema::employees` all name the
///   table; none of them can carry a statement. So `names` is `statements` plus
///   every identifier in the surviving items.
#[derive(Default)]
struct Production {
    /// One string literal, or one macro/attribute token stream, per entry —
    /// except on the raw-text path, where [`production_source`] puts the whole
    /// file in one entry. See [`file_write_targets`] for what that costs, which
    /// is nothing the comment reading depends on.
    statements: Vec<String>,
    /// The statement currently being built, sealed into `statements` by
    /// [`Self::literal`] and [`Self::tokens`].
    open: String,
    names: String,
    /// A token this gate cannot render at all was reached. It cannot be read,
    /// so it must not be cleared: [`production_source`] falls back to the RAW
    /// text scan it already uses for a file `syn` cannot parse. This is what
    /// closes the `#[non_exhaustive]` wildcard on [`syn::Lit`] — the same hole
    /// `Lit::CStr` fell through — without an enumeration to keep in sync.
    unreadable: bool,
}

impl Production {
    /// Text that can carry a statement. It names things too, so it lands in
    /// both readings.
    fn statement(&mut self, chunk: &str) {
        self.open.push_str(chunk);
        self.names.push_str(chunk);
    }

    /// Ends the statement being built. The two callers are the two carriers a
    /// statement can arrive in: one literal, and one token stream.
    fn seal(&mut self) {
        self.statements.push(std::mem::take(&mut self.open));
    }

    /// Text that can only name.
    fn name(&mut self, chunk: &str) {
        self.names.push_str(chunk);
        self.names.push(' ');
    }

    /// One token stream, flattened. Every token is walked; the ONE group that
    /// is not is a `doc` attribute's body.
    ///
    /// `///` and `//!` inside a macro invocation arrive here as `#[doc = "…"]`,
    /// and `console-ontology-canonical-domain`'s own `object_keys!` invocation
    /// documents which crate holds `UPDATE employees SET …`. A doc comment is
    /// not compiled, so dropping it cannot hide a write.
    ///
    /// What this used to do instead was skip the next group after ANY `#`,
    /// which is not the attribute grammar: it also dropped the body of
    /// `quote!`'s ordinary `#( … )*` repetition and of `#var` interpolation,
    /// both of which ARE code.
    ///
    /// The end of a token STREAM ends a statement, and this is the ONE place
    /// that is decided. What the `;` buys is a byte that is neither a name nor
    /// an [`UNREADABLE_TARGET`] marker in the target position of a bare-token
    /// verb that ends the stream. The [`Self::seal`] beside it makes the stream
    /// its own entry in [`Production::statements`]; nothing depends on that
    /// split any more, since a comment is resolved inside ONE literal by
    /// [`Self::walk`] before it can reach a later one.
    /// Whitespace is collapsed by design, so a newline is not
    /// a boundary at all: with attributes walked, `#[derive(Debug, Clone,
    /// Copy)]` ends on the bare verb `Copy` and the next node's text became its
    /// target — a measured false `COPY organizations` against
    /// backend/crates/platform/provisioning. `m!(Debug, Clone, Copy);`,
    /// `matches!(op, Op::Update)` and `quote! { #[derive(Copy)] }` are the same
    /// shape in streams an attribute's body never reaches. `;` because no table
    /// name can contain one.
    ///
    /// Appended in this wrapper and NOT in [`Self::walk`], because a GROUP is
    /// not a node boundary: terminating on every `(…)` split `m! { (UPDATE)
    /// employees … }` into two statements and dropped the write.
    ///
    /// The cost is stated rather than implied, because the two directions are
    /// ONE operation — whether node N's text glues to node N+1's — and no data
    /// in a token stream separates them. A statement spelled as bare tokens
    /// across two ADJACENT nodes is not read as one.
    ///
    /// What that costs is decided by [`write_targets`] and by nothing here. It
    /// is NOT the runtime/compile-time split an earlier version of this doc
    /// claimed, which was false in both directions: `push("UPDATE ")` is runtime
    /// and IS charged, `concat!("UPDATE ", "employees …")` is compile time and
    /// IS charged, `push("UPDATE")` then `push(" ")` is runtime and is NOT. What
    /// decides is TEXT — the verb must arrive with its trailing space, and the
    /// target position after it must resolve to a canonical name or hold an
    /// [`UNREADABLE_TARGET`] marker. The charged half is pinned by
    /// `a_write_whose_target_position_holds_a_placeholder_or_a_literal_boundary_is_charged_as_unresolved`;
    /// the uncharged half is residual 9 in the module doc.
    ///
    /// This terminator puts a `;` in the target position, which is neither a
    /// name nor a marker, so a bare-token verb ending a stream is charged
    /// nothing — a miss of the SECOND condition, not the first: the walk renders
    /// the bare verb WITH its trailing space. That is ONE spelling of residual 9
    /// and not its cause:
    /// `concat!(stringify!(UPDATE), " employees …")` escapes the same way inside
    /// a SINGLE stream, where the byte is a `,`, and a split inside the TARGET
    /// escapes with no bare token anywhere. Those spellings are constructed in
    /// `known_residual_a_write_that_misses_either_charging_condition_is_not_charged`.
    ///
    /// Charging a verb that reaches this boundary would close the `;` spelling
    /// alone, and was MEASURED rather than assumed: excluding only `COPY` (on
    /// the theory that only the `Copy` trait collides with ordinary Rust) still
    /// manufactured a false `organizations` violation from `#[q(mode = Update)]`
    /// and from `matches!(op, Op::Update)`, two cases already in
    /// `a_verb_ending_a_token_stream_does_not_take_the_next_node_as_its_target`.
    /// `UPDATE` collides with ordinary Rust too, so that asymmetry does not
    /// exist and the boundary stays where it is.
    fn tokens(&mut self, cursor: syn::buffer::Cursor<'_>) {
        self.walk(cursor);
        self.statement(";\n");
        self.seal();
    }

    /// The recursive half of [`Self::tokens`]. Emits no terminator.
    fn walk(&mut self, cursor: syn::buffer::Cursor<'_>) {
        let mut cursor = cursor;
        let mut attribute_head = false;
        while !cursor.eof() {
            if let Some((inner, _, _, rest)) = cursor.any_group() {
                if !(attribute_head && is_doc_attribute_body(cursor, inner)) {
                    self.walk(inner);
                }
                attribute_head = false;
                cursor = rest;
                continue;
            }
            // A literal token is a literal, and its SQL comments are resolved
            // inside it — the same rule [`Self::literal`] applies to the
            // literals `syn` classified. A macro's argument never reaches that
            // method, and `sqlx::query!("…")` is how this repository ordinarily
            // spells SQL.
            if let Some((literal, rest)) = cursor.literal() {
                self.statement(&without_sql_comments(&literal.to_string()));
                self.statement(" ");
                attribute_head = false;
                cursor = rest;
                continue;
            }
            let Some((tree, rest)) = cursor.token_tree() else {
                break;
            };
            let rendered = tree.to_string();
            attribute_head = rendered == "#" || (attribute_head && rendered == "!");
            self.statement(&rendered);
            self.statement(" ");
            cursor = rest;
        }
    }

    fn literal(&mut self, value: &str) {
        self.statement("\"");
        self.statement(&without_sql_comments(value));
        self.statement("\"\n");
        self.seal();
    }
}

/// One literal's text with its SQL comments replaced by one space.
///
/// This is called from the TWO places a literal's text enters the statement
/// reading — [`Production::literal`] for a literal `syn` classified, and
/// [`Production::walk`] for a literal token inside a macro or attribute — and
/// nowhere else, because ONE LITERAL is the only place a comment's extent is
/// knowable. A `--` comment ends at the newline in its own literal; a `/*…*/`
/// that closes, closes in its own literal. Run over the assembled statement
/// instead, a `--` in one literal would eat a real statement in the next one
/// (`m!("a--b", "UPDATE employees SET x = 1")`) and a `/*` in one literal would
/// close on a `*/` in another. Both were measured, and the second one shipped:
/// `a_block_comment_does_not_close_on_a_later_literals_text`.
///
/// A `/*` that does NOT close inside its literal is left standing, so it reaches
/// the target position as the [`UNREADABLE_TARGET`] marker it is and the write
/// fails CLOSED.
///
/// Removing text can only take a verb or a table AWAY from the reading; the one
/// thing it adds is the space that PostgreSQL itself reads there, which is the
/// whole point — `UPDATE/*c*/employees` is a write, and `cargo update
/// --workspace` is still not one
/// (`a_command_line_flag_after_a_verb_word_is_not_a_write`).
fn without_sql_comments(text: &str) -> String {
    if sql_comment_extent_needs_lexer(text) {
        without_sql_comments_sqlparser(strip_one_string_literal_layer(text))
    } else {
        without_sql_comments_byte_fallback(text)
    }
}

/// Macro token streams pass `Literal::to_string()` with surrounding `"`.
fn strip_one_string_literal_layer(text: &str) -> &str {
    let t = text.trim();
    if t.len() >= 2 && t.starts_with('"') && t.ends_with('"') && !t[1..t.len() - 1].contains('"') {
        &t[1..t.len() - 1]
    } else {
        t
    }
}

/// True when comment markers' extents depend on SQL single-quoted strings, or
/// when real SQL block/line comments must be resolved. Otherwise the byte path
/// is unchanged — macro splits, unreadable targets, and doc-attribute drops
/// behave exactly as before.
fn sql_comment_extent_needs_lexer(text: &str) -> bool {
    let mut in_single = false;
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\'' {
            if in_single && i + 1 < bytes.len() && bytes[i + 1] == b'\'' {
                i += 2;
                continue;
            }
            in_single = !in_single;
            i += 1;
            continue;
        }
        if !in_single {
            if bytes[i..].starts_with(b"--") || bytes[i..].starts_with(b"/*") {
                return true;
            }
        } else if bytes[i..].starts_with(b"--") || bytes[i..].starts_with(b"/*") {
            // Same extent question as `--` in quotes: the byte path cannot tell
            // data from a real block comment, so route to sqlparser (console-jth).
            return true;
        }
        i += 1;
    }
    false
}

fn without_sql_comments_sqlparser(text: &str) -> String {
    use sqlparser::dialect::PostgreSqlDialect;
    use sqlparser::tokenizer::{Token, Tokenizer, Whitespace};

    let dialect = PostgreSqlDialect {};
    // Decode Rust/SQL escapes so `\n` becomes a real newline. With unescape
    // off, `-- hint\nemployees …` is one EOF line comment and the write vanishes
    // (fail-OPEN vs the byte path, which left the target unresolved).
    match Tokenizer::new(&dialect, text)
        .with_unescape(true)
        .tokenize()
    {
        Ok(tokens) => {
            let last = tokens.len().saturating_sub(1);
            tokens
                .into_iter()
                .enumerate()
                .map(|(i, token)| match token {
                    Token::Whitespace(Whitespace::SingleLineComment { comment, .. }) => {
                        // EOF line comments become `;` — same rule as the byte path
                        // (residual 10: shell flags like `--workspace`).
                        if i == last && !comment.contains('\n') {
                            ";".to_string()
                        } else {
                            " ".to_string()
                        }
                    }
                    Token::Whitespace(Whitespace::MultiLineComment(_)) => " ".to_string(),
                    // Hold quoted DATA opaque so write_targets cannot charge a
                    // table name that only appears inside a string literal
                    // (`INSERT INTO notes VALUES ('-- UPDATE employees …')`).
                    Token::SingleQuotedString(_)
                    | Token::NationalStringLiteral(_)
                    | Token::HexStringLiteral(_) => "''".to_string(),
                    other => other.to_string(),
                })
                .collect()
        }
        Err(_) => without_sql_comments_byte_fallback(text),
    }
}

/// Original byte scan — preserved verbatim for fragments that do not need the lexer.
fn without_sql_comments_byte_fallback(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut copied = 0usize;
    let mut at = 0usize;
    while at < bytes.len() {
        let (end, replacement) = if bytes[at..].starts_with(b"--") {
            let end = past_line_comment(bytes, at);
            // A `--` that runs off the END of the literal took the literal's own
            // closing quote with it, and `"` is an [`UNREADABLE_TARGET`] marker:
            // left as a space, `"truncate --size=0 …"` would read as a statement
            // assembled up to `truncate ` and fail closed on a shell command.
            // A `;` is the same thing [`Production::tokens`] puts at a boundary
            // for the same reason — no table name can contain one — and it is
            // what PostgreSQL sees here, since the rest of the LINE is comment
            // too. What that costs is residual 10.
            (Some(end), if end == bytes.len() { ';' } else { ' ' })
        } else {
            (past_block_comment(bytes, at), ' ')
        };
        if let Some(end) = end {
            out.push_str(&text[copied..at]);
            out.push(replacement);
            copied = end;
            at = end;
        } else {
            at += 1;
        }
    }
    out.push_str(&text[copied..]);
    out
}

/// Where the `--` comment at `at` ends: at the newline that terminates it, or at
/// the end of the literal. A literal spells that newline either as one byte
/// (`Production::literal`, which hands over the literal's VALUE) or as the
/// two-byte escape `\n` (`Production::walk`, which renders a literal token as it
/// was written).
fn past_line_comment(bytes: &[u8], at: usize) -> usize {
    let mut end = at + 2;
    while end < bytes.len() {
        if bytes[end] == b'\n' || bytes[end..].starts_with(b"\\n") {
            return end;
        }
        end += 1;
    }
    bytes.len()
}

/// THE doc-attribute rule. Defined once, called from both positions an
/// attribute can appear in: on an item ([`Production::visit_attribute`]) and
/// inside a token stream ([`is_doc_attribute_body`]).
///
/// It was spelled twice, differently. The token-stream side asked whether the
/// FIRST IDENT of the bracket body was `doc`, which is a different question
/// from "the path is `doc`": `#[doc::hidden("UPDATE …")]` is an attribute
/// proc-macro invocation — COMPILED CODE that receives the statement verbatim —
/// and it answered yes there and no here.
fn is_doc_attribute(path: &syn::Path) -> bool {
    path.is_ident("doc")
}

/// True when the group at `outer` is a `doc` attribute's body: BRACKET
/// delimited, per Rust's attribute grammar, and a `doc` attribute by the one
/// rule above. `#(…)` and `#{…}` are not attributes at all, and `#[derive(…)]`
/// is one but is not a comment.
///
/// The path is PARSED — `syn::Meta` covers all three attribute forms, `#[doc]`,
/// `#[doc = "…"]` and `#[doc(…)]` — rather than lexed out of the first ident. A
/// body that is not a `Meta` at all is not dropped.
fn is_doc_attribute_body(outer: syn::buffer::Cursor<'_>, inner: syn::buffer::Cursor<'_>) -> bool {
    outer
        .token_tree()
        .is_some_and(|(group, _)| group.to_string().starts_with('['))
        && syn::parse2::<syn::Meta>(inner.token_stream())
            .is_ok_and(|meta| is_doc_attribute(meta.path()))
}

impl<'ast> syn::visit::Visit<'ast> for Production {
    fn visit_item(&mut self, node: &'ast syn::Item) {
        if gated_to_test_builds(item_attrs(node)) {
            return;
        }
        syn::visit::visit_item(self, node);
    }

    fn visit_impl_item(&mut self, node: &'ast syn::ImplItem) {
        if gated_to_test_builds(impl_item_attrs(node)) {
            return;
        }
        syn::visit::visit_impl_item(self, node);
    }

    fn visit_trait_item(&mut self, node: &'ast syn::TraitItem) {
        if gated_to_test_builds(trait_item_attrs(node)) {
            return;
        }
        syn::visit::visit_trait_item(self, node);
    }

    /// EVERY raw token stream in the file, whatever node kind holds it.
    ///
    /// `syn::visit`'s default body for this is `{}`, and it is the ONE method
    /// every token stream in the grammar arrives at: the eight `Verbatim` arms
    /// (`Item`, `ImplItem`, `TraitItem`, `ForeignItem`, `Expr`, `Type`, `Pat`,
    /// `TypeParamBound`), a `Macro`'s arguments, and a `Meta::List`'s body. So
    /// a token `syn` declined to parse is READ rather than dropped, with no
    /// list of node kinds to keep in sync — a ninth `Verbatim` arm in a future
    /// `syn` is covered the day it lands.
    ///
    /// This replaced four hand-written `Verbatim` arms that named the four ITEM
    /// positions and called the category closed. `syn` 2.0.117 has eight, and
    /// the four that are not item positions were read as NOTHING: a `become`
    /// tail call, `builtin #`, a `box` pattern and a `dyn*` type each carried a
    /// full `UPDATE` past the gate. See
    /// `every_verbatim_arm_syn_emits_is_read_rather_than_skipped`.
    ///
    /// It is opt-OUT, not opt-in: this walks token STREAMS, never every AST
    /// node, so `struct Update { employees: u8 }` still contributes nothing to
    /// the statement reading.
    fn visit_token_stream(&mut self, node: &'ast syn::__private::TokenStream2) {
        // `syn::__private::TokenStream2` IS `proc_macro2::TokenStream`; the
        // signature has to name it and `proc-macro2` is not a dependency of
        // this crate, which needs backend/Cargo.toml. See followUps.
        let buffer = syn::buffer::TokenBuffer::new2(node.clone());
        self.tokens(buffer.begin());
    }

    /// Only a `doc` attribute is dropped, by [`is_doc_attribute`] — the same
    /// call [`is_doc_attribute_body`] makes inside a token stream, so the rule
    /// exists once rather than in two spellings that drifted. `///`
    /// and `//!` arrive here as `#[doc = "…"]`, and this file's own doc comments
    /// quote the statements it hunts for; a doc comment is not compiled, so it
    /// cannot write a row.
    ///
    /// Every OTHER attribute is compiled code and its tokens are read. An
    /// attribute proc-macro receives them verbatim and can emit the statement,
    /// and a derive helper is the ordinary place an ORM crate spells its table
    /// name. Dropping the whole category is the token-shape rule this gate
    /// exists to remove, spelled "it starts with `#`".
    fn visit_attribute(&mut self, node: &'ast syn::Attribute) {
        if is_doc_attribute(node.path()) {
            return;
        }
        // A `Meta::List`'s body reaches [`Self::visit_token_stream`] from here,
        // exactly as a macro's arguments do.
        syn::visit::visit_meta(self, &node.meta);
    }

    /// Identifiers, for the NAME reading only. They cannot carry a statement,
    /// but they are how a file names the table that an unreadable write target
    /// has to be charged to.
    fn visit_ident(&mut self, node: &'ast syn::Ident) {
        self.name(&node.to_string());
    }

    /// Every literal spelling Rust has that is wider than one character:
    /// `"…"`, `b"…"`, `c"…"` and their raw forms. `c"…"` is `Lit::CStr`, a
    /// variant of its own, and `PQexec` takes a `*const c_char` — a literal KIND
    /// dropped here is the same fail-open as a token shape dropped above.
    ///
    /// The five scalar variants are named rather than defaulted, so that adding
    /// one to the ignored set is a deliberate edit. `Lit` is `#[non_exhaustive]`,
    /// so the wildcard cannot be removed — but it no longer DROPS anything: a
    /// variant this `syn` does not classify makes the file UNREADABLE, and an
    /// unreadable file is scanned raw. A future string-bearing variant is
    /// therefore over-scanned rather than skipped.
    fn visit_lit(&mut self, node: &'ast syn::Lit) {
        match node {
            syn::Lit::Str(text) => self.literal(&text.value()),
            syn::Lit::ByteStr(bytes) => {
                self.literal(&String::from_utf8_lossy(&bytes.value()));
            }
            syn::Lit::CStr(bytes) => {
                self.literal(&String::from_utf8_lossy(bytes.value().to_bytes()));
            }
            // A literal this version of `syn` does not classify, re-emitted as
            // it was written.
            syn::Lit::Verbatim(token) => self.literal(&token.to_string()),
            syn::Lit::Byte(_)
            | syn::Lit::Char(_)
            | syn::Lit::Int(_)
            | syn::Lit::Float(_)
            | syn::Lit::Bool(_) => {}
            // UNREACHABLE with the pinned `syn` — every variant it has is named
            // above — so it has NO test, which is stated rather than implied.
            // It exists so that a ninth variant fails CLOSED instead of being
            // dropped the way `Lit::CStr` was.
            _ => self.unreadable = true,
        }
    }
}

/// The production half of one source file.
///
/// Parsed, not lexed. The token-shape family that cost four rounds — `'{'`,
/// `/* { */`, `r#"a"b{"#`, a multi-line string literal, an attribute and its
/// item on one line — does not exist here, because nothing counts characters.
fn production_source(text: &str) -> Production {
    // Source rustc would reject is not source this gate may ignore: "leave the
    // file unparseable" would otherwise be the cheapest evasion there is. Scan
    // the raw text instead, in both readings.
    let raw = || Production {
        statements: vec![text.to_owned()],
        open: String::new(),
        names: text.to_owned(),
        unreadable: true,
    };
    let Ok(file) = syn::parse_file(text) else {
        return raw();
    };
    let mut production = Production::default();
    syn::visit::Visit::visit_file(&mut production, &file);
    // A token the walk could not render is the same situation as a file it
    // could not parse, and gets the same answer.
    if production.unreadable {
        raw()
    } else {
        production
    }
}

const VERBS: &[&str] = &[
    "insert into ",
    "update ",
    "delete from ",
    "merge into ",
    "truncate ",
    "copy ",
];

/// Words that turn a following verb into something other than a write:
/// `SELECT … FOR UPDATE`, `FOR NO KEY UPDATE`, `ON CONFLICT DO UPDATE`.
const NOT_A_WRITE_AFTER: &[&[u8]] = &[b"for ", b"no ", b"key ", b"do "];

/// Noise between the verb and the table name. `TRUNCATE TABLE x`, `UPDATE ONLY
/// x` and `DELETE FROM ONLY x` all target `x`.
const TARGET_PREFIXES: &[&[u8]] = &[b"table ", b"only "];

/// What a target position that is not an identifier may hold and still be read
/// AS a write, to a table this half cannot name:
///
/// * the string was ASSEMBLED — `{` opens a `format!` placeholder, `"` and `\`
///   are a literal boundary (`push`, `concat!`);
/// * `/*` is a block comment that does not CLOSE inside its own literal — the
///   ones that do are gone before this reading runs ([`without_sql_comments`]).
///
/// `--` is deliberately NOT here, and was for one round. This test sees only
/// the bytes standing at the target position, so it cannot tell a PostgreSQL
/// line comment from the commonest flag prefix in shell and prose: it charged
/// `truncate --size=0 …` and `cargo update --workspace …` in a crate that
/// merely NAMES a canonical table, where the round before it charged neither.
/// `a_command_line_flag_after_a_verb_word_is_not_a_write` pins that. It is not
/// the trade it looked like: the line comment that DOES stand where the table
/// belongs — `"UPDATE -- hint⏎employees SET …"` — is resolved inside its literal
/// and charged by name, which is stronger than failing closed on it, and the
/// two shell strings are still charged nothing. A false positive is worse than
/// the residual it removes: a gate that charges `cargo update --workspace` is
/// turned off by the next person it blocks.
///
/// `/*` is KEPT, on the same evidence rather than by symmetry, and it is not
/// free either. To reach here it must be spelled inside a string literal,
/// immediately after a DML verb word and its space, and must NOT close later in
/// the same literal — Rust's own block comments are dropped by `syn`, tokens
/// and all, and never reach this reading. What survives that shape is a root
/// glob, and `"copy /* to the clipboard"` and `"truncate /*.log"` were both
/// constructed and DO charge. What does not: a glob that closes
/// (`"copy /*.txt */ dest"`), a non-DML command word (`"cp -a /* /mnt"`), the
/// actual Windows switch convention (`"copy /y src dst"`), and a Rust block
/// comment holding a full statement — all four measured at zero in
/// `ordinary_rust_that_puts_a_block_comment_marker_near_a_verb_is_not_charged`,
/// and the real tree holds none of the two that charge
/// (`measured_tip_has_exactly_the_ratcheted_dual_writers`).
///
/// So the difference is one of frequency, and it is a large one: `--` follows a
/// command word in every CLI ever written, while `/*` after a DML verb word
/// requires an unterminated absolute-root glob. What `/*` buys is the comment
/// that opens in one fragment of an ASSEMBLED statement and closes in another —
/// `push("UPDATE /* hint")` then `push(" */ employees …")` — which is the same
/// fail-closed family as `{` and `"`, and is charged in
/// `gate_detects_every_measured_evasion`.
///
/// Anything ELSE at this position is charged nothing, which is a live fail-open
/// rather than a proof that it is not SQL — residual 9. It is deliberately not
/// "any byte that is not an identifier": `T: Copy + Send` puts a `+` here, and
/// charging that manufactures a violation against any file that names a
/// canonical table. `non_dml_uses_of_the_verbs_are_not_writes` pins it.
const UNREADABLE_TARGET: &[&[u8]] = &[b"{", b"\"", b"\\", b"/*"];

/// Collapses whitespace runs to one space and lowercases, so a statement broken
/// across lines reads the same as a single-line one.
fn collapsed_lower(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_space = false;
    for ch in text.chars() {
        if ch.is_whitespace() {
            if !in_space {
                out.push(' ');
            }
            in_space = true;
        } else {
            out.extend(ch.to_lowercase());
            in_space = false;
        }
    }
    out
}

/// What a DML verb in the source was aimed at.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct WriteTargets {
    /// Table names read straight out of the statement.
    pub(crate) tables: Vec<String>,
    /// At least one write whose target position held an [`UNREADABLE_TARGET`]
    /// marker instead of an identifier — a `format!` placeholder, a literal
    /// boundary, or a block comment that never closes. The target is unknown,
    /// NOT absent, so it is charged rather than dropped.
    ///
    /// This is NARROWER than "a target that could not be read", and the
    /// difference is a live fail-open, not a technicality: a split inside the
    /// TARGET reads as the identifier `employ` and never sets this, and anything
    /// in the target position that is not a marker sets nothing at all.
    /// Residual 9.
    pub(crate) unresolved: bool,
}

/// Every write target in one file, read ONE STATEMENT AT A TIME.
///
/// The separation is NOT what keeps one statement's comment out of another's
/// text, and claiming that it was cost this gate two rounds. It is not true for
/// two literals inside one macro's token stream, which [`Production::tokens`]
/// seals into ONE entry, nor for a file `syn` cannot parse, which
/// [`production_source`] seals whole. What keeps them apart is that a comment is
/// resolved by [`without_sql_comments`] inside the ONE literal that holds it,
/// and nothing from here on follows a comment at all. Both directions are
/// measured: `a_block_comment_does_not_close_on_a_later_literals_text` for a
/// comment that must not travel, and
/// `a_block_comment_body_may_hold_a_quote_and_a_semicolon` for a comment body
/// holding the bytes an earlier bound tried to stop at.
fn file_write_targets(statements: &[String]) -> WriteTargets {
    let mut all = WriteTargets {
        tables: Vec::new(),
        unresolved: false,
    };
    for statement in statements {
        let found = write_targets(&collapsed_lower(statement));
        all.tables.extend(found.tables);
        all.unresolved |= found.unresolved;
    }
    all.tables.sort();
    all.tables.dedup();
    all
}

/// Every table named as the target of a DML statement in `collapsed`, plus
/// whether any write had a target this cannot read.
///
/// `collapsed` is ONE statement, already [`collapsed_lower`]ed.
fn write_targets(collapsed: &str) -> WriteTargets {
    let bytes = collapsed.as_bytes();
    let mut result = WriteTargets {
        tables: Vec::new(),
        unresolved: false,
    };
    for verb in VERBS {
        let mut from = 0usize;
        while let Some(offset) = collapsed[from..].find(verb) {
            let start = from + offset;
            from = start + verb.len();
            let before = &bytes[..start];
            // `qualified.update ` and `some_update ` are not statements.
            if before
                .last()
                .is_some_and(|byte| is_ident_byte(*byte) || *byte == b'.')
            {
                continue;
            }
            if NOT_A_WRITE_AFTER.iter().any(|word| before.ends_with(word)) {
                continue;
            }
            // ONE reading of where the target position is, shared by the two
            // things that then ask about it. Reading it twice is what let
            // `UPDATE ONLY {t}` through for four rounds: `read_target` consumed
            // the prefix and the marker test re-read the byte in front of it.
            let target = skip_target_noise(bytes, from);
            match read_target(bytes, target) {
                Some(table) => result.tables.push(table),
                // `COPY` is NOT excluded here. It was, on the grounds that the
                // Rust marker trait `Copy` outnumbers the SQL statement — but
                // `COPY … FROM STDIN` via `copy_in_raw` is a full bulk write,
                // and excluding it meant `format!("COPY {t} …")` scored zero
                // violations where the identical `UPDATE` scored one. The trait
                // spellings that reach this arm do not exist: `T: Copy + Send`,
                // `#[derive(Clone, Copy)]` and `T: Copy,` all put something
                // other than an `UNREADABLE_TARGET` entry after the verb.
                None if UNREADABLE_TARGET
                    .iter()
                    .any(|marker| bytes[target..].starts_with(marker)) =>
                {
                    result.unresolved = true;
                }
                None => {}
            }
        }
    }
    result.tables.sort();
    result.tables.dedup();
    result
}

/// The target position after any `only `/`table ` prefix between the verb and
/// the table, in any number. `TRUNCATE TABLE x`, `UPDATE ONLY x` and `DELETE
/// FROM ONLY x` all target `x`.
///
/// A comment is NOT noise here any more, because it is no longer standing here:
/// [`without_sql_comments`] resolved it inside the literal that held it, which
/// is the only place its extent is known. Following one from here was what read
/// a `*/` out of an unrelated later literal —
/// `a_block_comment_does_not_close_on_a_later_literals_text`. What survives to
/// this position is a `/*` that never closed, and that is an unknown target.
///
/// [`write_targets`] calls this ONCE and hands the result to both the
/// identifier read and the marker test, so the two cannot disagree about where
/// the target position is.
fn skip_target_noise(bytes: &[u8], mut cursor: usize) -> usize {
    while let Some(prefix) = TARGET_PREFIXES
        .iter()
        .find(|prefix| bytes[cursor..].starts_with(prefix))
    {
        cursor += prefix.len();
    }
    cursor
}

/// One `/*…*/` at `cursor`, and the offset just past it. PostgreSQL block
/// comments NEST, so this counts depth rather than stopping at the first `*/`.
///
/// The ONE caller is [`without_sql_comments`], so `bytes` is ONE LITERAL and
/// that is the whole bound: the scan runs off the end of the literal rather than
/// into another one. `None` when there is no comment there, and also when one
/// opens and does not close inside this literal — the caller leaves that `/*`
/// standing, so it reaches the target position as the [`UNREADABLE_TARGET`]
/// marker it is and the write fails closed.
fn past_block_comment(bytes: &[u8], cursor: usize) -> Option<usize> {
    if !bytes[cursor..].starts_with(b"/*") {
        return None;
    }
    let mut depth = 1usize;
    let mut at = cursor + 2;
    while depth > 0 {
        if bytes[at..].starts_with(b"/*") {
            depth += 1;
            at += 2;
        } else if bytes[at..].starts_with(b"*/") {
            depth -= 1;
            at += 2;
        } else if at < bytes.len() {
            at += 1;
        } else {
            return None;
        }
    }
    Some(at)
}

/// Reads the table a verb targets from the target position `cursor`, following
/// schema qualification to its last component. `None` when the bytes there are
/// not an identifier at all.
///
/// `Some` is not "resolved to a table": it is whatever identifier is there, so a
/// fragment of one (`concat!("UPDATE employ", "ees …")` gives `employ`) returns
/// `Some` and is then dropped for not naming a canonical table, WITHOUT reaching
/// the caller's unresolved arm. That is residual 9 in the module doc, and it is
/// not fixed by widening the marker set.
fn read_target(bytes: &[u8], cursor: usize) -> Option<String> {
    let (mut name, mut next) = read_ident(bytes, cursor)?;
    // `schema.table`, `"schema"."table"` and `public . employees` all name the
    // last component; only the literal `public.` prefix was skipped before.
    loop {
        let mut probe = next;
        while bytes.get(probe) == Some(&b' ') {
            probe += 1;
        }
        if bytes.get(probe) != Some(&b'.') {
            break;
        }
        let (component, after) = read_ident(bytes, probe + 1)?;
        name = component;
        next = after;
    }
    Some(name)
}

/// One identifier at or just after `cursor`, bare or double-quoted, with the
/// offset just past it.
fn read_ident(bytes: &[u8], mut cursor: usize) -> Option<(String, usize)> {
    while bytes.get(cursor) == Some(&b' ') {
        cursor += 1;
    }
    let quoted = past_quote(bytes, cursor);
    if let Some(after) = quoted {
        cursor = after;
    }
    let start = cursor;
    while cursor < bytes.len() && is_ident_byte(bytes[cursor]) {
        cursor += 1;
    }
    if cursor == start {
        return None;
    }
    let name = String::from_utf8_lossy(&bytes[start..cursor]).into_owned();
    if quoted.is_some() {
        cursor = past_quote(bytes, cursor)?;
    }
    Some((name, cursor))
}

/// One SQL identifier quote at `cursor`, spelled `"` in a raw string and `\"`
/// in an ordinary one, and the offset just past it.
fn past_quote(bytes: &[u8], mut cursor: usize) -> Option<usize> {
    if bytes.get(cursor) == Some(&b'\\') {
        cursor += 1;
    }
    if bytes.get(cursor) == Some(&b'"') {
        Some(cursor + 1)
    } else {
        None
    }
}

/// True when `needle` appears in `collapsed` as a whole identifier.
fn names_table(collapsed: &str, needle: &str) -> bool {
    let bytes = collapsed.as_bytes();
    let mut from = 0usize;
    while let Some(offset) = collapsed[from..].find(needle) {
        let start = from + offset;
        let end = start + needle.len();
        from = start + 1;
        let bounded = (start == 0 || !is_ident_byte(bytes[start - 1]))
            && (end == bytes.len() || !is_ident_byte(bytes[end]));
        if bounded {
            return true;
        }
    }
    false
}

const fn is_ident_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

#[cfg(test)]
mod tests {
    /// Text fallback must not invent a crate named `package` from a rename
    /// directive. Real package names are Cargo metadata's job (console-ugg).
    #[test]
    fn text_scan_skips_package_rename_directives() {
        let (shipped, _dev) =
            dependency_edges("[dependencies.alias]\npackage = \"real-package\"\nversion = \"1\"\n");
        assert!(
            shipped.contains("alias"),
            "subtable header still names the key: {shipped:?}"
        );
        assert!(
            !shipped.contains("package"),
            "a `package =` key is a rename directive, not a crate: {shipped:?}"
        );
        assert!(
            !shipped.contains("real-package"),
            "text scan must not pretend to resolve renames: {shipped:?}"
        );
    }

    #[test]
    fn a_dependency_without_the_rename_form_records_only_its_key() {
        let (shipped, _dev) =
            dependency_edges("[dependencies]\nserde = { features = [\"derive\"] }\n");
        assert!(shipped.contains("serde"));
        assert_eq!(shipped.len(), 1, "no phantom package name: {shipped:?}");
    }

    /// `cargo metadata` resolves every `package =` spelling — including
    /// workspace-inherited aliases — so a production edge cannot fail-open as
    /// "dev-only" when another crate lists the real name under `[dev-dependencies]`.
    #[allow(clippy::unwrap_used, clippy::panic)]
    #[test]
    fn cargo_metadata_records_workspace_inherited_package_renames() {
        let root = std::env::temp_dir().join(format!("console-wo-ugg-meta-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("crates/real_pkg/src")).unwrap();
        std::fs::create_dir_all(root.join("crates/producer/src")).unwrap();
        std::fs::write(
            root.join("Cargo.toml"),
            r#"[workspace]
members = ["crates/*"]
resolver = "2"

[workspace.dependencies]
alias = { package = "real-pkg", path = "crates/real_pkg" }
"#,
        )
        .unwrap();
        std::fs::write(
            root.join("crates/real_pkg/Cargo.toml"),
            "[package]\nname = \"real-pkg\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )
        .unwrap();
        std::fs::write(root.join("crates/real_pkg/src/lib.rs"), "pub fn x() {}\n").unwrap();
        std::fs::write(
            root.join("crates/producer/Cargo.toml"),
            "[package]\nname = \"producer\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n\
             [dependencies]\nalias = { workspace = true }\n",
        )
        .unwrap();
        std::fs::write(
            root.join("crates/producer/src/lib.rs"),
            "pub fn nothing() {}\n",
        )
        .unwrap();

        let tree = CrateTree::from_cargo_metadata(&root)
            .unwrap_or_else(|e| panic!("metadata must resolve the workspace rename: {e}"));
        assert!(
            tree.shipped_dependencies.contains("real-pkg"),
            "workspace-inherited package= must ship under the REAL name: {:?}",
            tree.shipped_dependencies
        );
        assert!(
            tree.shipped_dependencies.contains("alias"),
            "the Cargo.toml key must survive too: {:?}",
            tree.shipped_dependencies
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    use super::{
        CrateTree, Production, Truth, WriteTargets, cfg_truth, collapsed_lower, dependency_edges,
        file_write_targets, gated_to_test_builds, production_source, write_targets,
    };

    /// One statement, read on its own, ALREADY assembled — no literal boundary
    /// is visible in it any more. Use [`literal_targets`] whenever the question
    /// is about a SQL comment, whose extent is a property of the literal that
    /// holds it and not of the assembled text.
    fn targets(source: &str) -> WriteTargets {
        write_targets(&collapsed_lower(source))
    }

    /// One string LITERAL, read exactly as [`Production::literal`] reads it:
    /// its SQL comments are resolved INSIDE it, which is the only place their
    /// extent is known.
    fn literal_targets(value: &str) -> WriteTargets {
        let mut production = Production::default();
        production.literal(value);
        file_write_targets(&production.statements)
    }

    /// Real Rust, split into statements by the real walk. Use this whenever the
    /// question is about the STATEMENT BOUNDARY — `targets` above cannot ask it,
    /// because it is handed one statement by construction.
    fn targets_of(rust: &str) -> WriteTargets {
        file_write_targets(&production_source(rust).statements)
    }

    /// Pins the RESOLUTION path on its own. `tests/gate_detects_violation.rs`
    /// asserts these spellings are caught, but the unresolved fallback would
    /// also catch most of them, so breaking the parser alone leaves that suite
    /// green. Here the table must be READ out of the statement.
    ///
    /// The COMMENT spellings are not here, because a comment's extent belongs to
    /// the literal that holds it and this helper is handed an already-assembled
    /// statement with no literal boundary left in it. They are in
    /// [`comments_are_resolved_inside_the_literal_that_holds_them`], which reads
    /// them through [`Production::literal`] itself, and end to end in
    /// `gate_detects_every_measured_evasion`.
    #[test]
    fn every_target_spelling_resolves_to_the_table() {
        for source in [
            r#""UPDATE \"employees\" SET x = 1""#,
            r#""INSERT INTO \"employees\" (a) VALUES (1)""#,
            r##"r#"UPDATE "employees" SET x = 1"#"##,
            "\"MERGE INTO employees t USING s ON t.id = s.id\"",
            "\"UPDATE ONLY employees SET x = 1\"",
            "\"DELETE FROM ONLY employees WHERE id = 1\"",
            "\"UPDATE hr.employees SET x = 1\"",
            "\"INSERT INTO public . employees (a) VALUES (1)\"",
            "\"TRUNCATE TABLE employees\"",
            "\"COPY employees (a, b) FROM STDIN\"",
            "\"UPDATE\n   public.employees\n SET x = 1\"",
        ] {
            let found = targets(source);
            assert!(
                found.tables.iter().any(|table| table == "employees"),
                "did not resolve `employees` out of {source}: {found:?}"
            );
        }
    }

    /// Every comment spelling, resolved INSIDE the literal that holds it.
    ///
    /// This is the whole comment rule, and it is one function
    /// ([`without_sql_comments`]) at one place — the point where a literal's
    /// text enters the statement reading. The extent of a comment is knowable
    /// exactly there and nowhere later: a `--` ends at the newline in its own
    /// literal, and a `/*…*/` that closes, closes in its own literal.
    ///
    /// The second list is what the same rule must NOT charge. `--` is the
    /// commonest flag prefix in shell and prose, and stripping it can only
    /// REMOVE text from the reading, never join a verb to a table that was not
    /// already next to it.
    #[test]
    fn comments_are_resolved_inside_the_literal_that_holds_them() {
        for value in [
            // between the verb and the target
            "UPDATE /*hint*/ employees SET x = 1",
            "TRUNCATE /*hint*/ TABLE /*hint*/ employees",
            "UPDATE -- hint\nemployees SET x = 1",
            // a comment BODY may hold either byte the reading uses elsewhere
            "UPDATE /* was: SET x = 1; */ employees SET x = 1",
            "UPDATE /* was: name = \"x\" */ employees SET x = 1",
            // PostgreSQL block comments NEST
            "DELETE FROM /* outer /* inner */ still a comment */ ONLY employees WHERE id = 1",
            // …and standing where a SEPARATOR belongs, which defeats the verb
            // match outright unless the comment is resolved FIRST
            "UPDATE/*c*/employees SET x = 1",
            "UPDATE--c\nemployees SET x = 1",
            "UPDATE ONLY/*c*/employees SET x = 1",
            "INSERT/*c*/INTO employees (id) VALUES (9)",
        ] {
            let found = literal_targets(value);
            assert!(
                found.tables.iter().any(|table| table == "employees"),
                "did not resolve `employees` out of {value:?}: {found:?}"
            );
        }
        for value in [
            "truncate --size=0 /var/log/app.log",
            "cargo update --workspace --locked",
            "copy /*.txt */ dest",
            "cp -a /* /mnt",
        ] {
            let found = literal_targets(value);
            assert!(
                !found.unresolved && !found.tables.iter().any(|table| table == "employees"),
                "{value:?} is not a write: {found:?}"
            );
        }
        // A `/*` that does NOT close inside its literal is an UNKNOWN target,
        // which fails closed rather than resolving anything.
        let open = literal_targets("UPDATE /* hint");
        assert!(
            open.unresolved && open.tables.is_empty(),
            "an unterminated comment in the target position is unknown, not absent: {open:?}"
        );
    }

    /// The measured answer to "why does the visitor opt IN to the nodes that
    /// can carry a statement, instead of walking every token by default?".
    ///
    /// Flattened, ordinary Rust READS AS SQL. Both of these are declarations
    /// with no statement anywhere in them, and both resolve a table out of the
    /// statement reading — the second is the shape that reached the real tree
    /// once attributes were walked. So a token walk that fed every identifier
    /// into `statements` would manufacture violations out of type and module
    /// names, which is why `names` is the wider reading and `statements` is not.
    #[test]
    fn flattening_every_token_into_the_statement_reading_manufactures_writes() {
        // Both strings are what [`Production::tokens`] would PRINT for the
        // Rust above them: group delimiters do not survive the flattening.
        // `struct Update { employees: u8 }`
        assert_eq!(
            targets("struct Update employees : u8").tables,
            ["employees"],
            "a struct named `Update` with a field named `employees` is not a write"
        );
        // `#[derive(Clone, Copy)]` and then `const Q: &str = "organizations";`
        assert_eq!(
            targets("Clone , Copy \"organizations\"").tables,
            ["organizations"],
            "a `Copy` derive followed by a literal is not a write"
        );
    }

    /// And pins the FALLBACK on its own: each case below is charged as unknown
    /// rather than as absent.
    ///
    /// WHEN the string is assembled is not the axis, which is why this test is
    /// no longer named for it: the third case is assembled at COMPILE time and
    /// is charged all the same. The axis is TEXT — the verb arrives with its
    /// trailing space, and what stands in the target position, after
    /// [`skip_target_noise`] has consumed any `only `/`table `/`/*…*/`, is an
    /// [`UNREADABLE_TARGET`] marker. The runtime `push("UPDATE")` then `push(" ")` is NOT charged,
    /// because the verb's space arrives in a later fragment; that miss is
    /// `known_residual_a_write_that_misses_either_charging_condition_is_not_charged`
    /// in `tests/gate_detects_violation.rs`.
    #[test]
    fn a_marker_in_the_target_position_is_unresolved_not_absent() {
        for source in [
            "format!(\"UPDATE {t} SET x = 1\")",
            "b.push(\"INSERT INTO \"); b.push(\"employees\");",
            "concat!(\"INSERT INTO \", \"employees\")",
            "format!(\"UPDATE ONLY {t} SET x = 1\")",
            "format!(\"TRUNCATE TABLE {t}\")",
            "format!(\"DELETE FROM ONLY {t} WHERE id = 1\")",
            "b.push(\"UPDATE ONLY \"); b.push(t);",
            "format!(\"UPDATE /*hint*/ {t} SET x = 1\")",
        ] {
            let found = targets(source);
            assert!(found.unresolved, "{source} must mark an unknown target");
        }
    }

    /// A `/*` in one literal must not close on a `*/` in another one — in ANY
    /// of the four ways two literals can end up next to each other in this
    /// reading. Each of these resolved `organizations`, a table the statement
    /// never names, and charged it directly past the `names_table` guard the
    /// unresolved arm goes through.
    ///
    /// The bound is not a scan limit any more, it is the granularity: comments
    /// are resolved by [`without_sql_comments`] inside ONE literal, and nothing
    /// downstream follows a comment at all. So the two literals of a `concat!`,
    /// the two arguments of a macro (which [`Production::tokens`] seals into ONE
    /// statement), two separate items, and the whole file of an unparseable
    /// source (which [`production_source`] seals into one statement too) all get
    /// the same answer.
    #[test]
    fn a_block_comment_does_not_close_on_a_later_literals_text() {
        for (label, found) in [
            (
                "assembled text with both literals in it",
                targets("format!(\"UPDATE /* {t} SET x = 1\") \"*/ organizations\""),
            ),
            (
                "two literals in ONE macro token stream",
                targets_of(
                    "pub fn f() { m!(\"UPDATE /* hint\", \"*/ organizations SET x = 1\"); }\n",
                ),
            ),
            (
                "two separate items",
                targets_of(
                    "pub fn q(t: &str) -> String { format!(\"UPDATE /* {t} SET x = 1\") }\n\
                     pub const OTHER: &str = \"*/ organizations\";\n",
                ),
            ),
            (
                "an unparseable file, scanned as raw text",
                targets_of(
                    "pub const SQL: &str = \"UPDATE /* SET x = 1\";\n\
                     pub const OTHER: &str = \"*/ organizations\";\n\
                     pub fn broken( {\n",
                ),
            ),
        ] {
            assert!(
                found.tables.is_empty(),
                "{label}: the reading crossed into another literal and read a table \
                 this statement never names: {found:?}"
            );
            assert!(
                found.unresolved,
                "{label}: a target behind a comment that does not close is UNKNOWN, \
                 not absent: {found:?}"
            );
        }
    }

    /// The words that make a verb something other than a write. Each of these
    /// produced a false violation against a real crate before it was excluded.
    #[test]
    fn non_dml_uses_of_the_verbs_are_not_writes() {
        for source in [
            "\"SELECT id FROM employees WHERE x = 1\n   FOR UPDATE\n \"",
            "\"... ON CONFLICT (id) DO UPDATE\n   SET x = 1\"",
            "\"SELECT 1 FROM employees FOR NO KEY UPDATE \"",
            "pub fn f<T: Copy + Send>(_: T) {}",
            "AuditAction::new(\"platform.group.update \")",
        ] {
            let found = targets(source);
            assert!(
                !found.unresolved && !found.tables.iter().any(|table| table == "employees"),
                "{source} is not a write: {found:?}"
            );
        }
    }

    /// The one rule the static half turns on, exercised DIRECTLY. Every
    /// end-to-end case in `tests/gate_detects_violation.rs` reaches it through a
    /// temp tree, so a rule that is right there but wrong at the edges would be
    /// invisible; and `is_cfg_test_attribute`, the predicate this replaces, was
    /// load-bearing with no unit test at all for four rounds.
    #[test]
    fn cfg_truth_is_evaluated_with_test_false() {
        let cases = [
            ("#[cfg(test)]", Truth::Never),
            ("#[cfg(all(test, feature = \"x\"))]", Truth::Never),
            ("#[cfg(any(test))]", Truth::Never),
            ("#[cfg(not(not(test)))]", Truth::Never),
            ("#[cfg(any())]", Truth::Never),
            // SAT: `X` may hold, so the item SHIPS. Dropping it is a fail-open,
            // and shipping that inversion is what lost the fourth round.
            ("#[cfg(any(test, feature = \"x\"))]", Truth::Maybe),
            ("#[cfg(not(any(test, feature = \"x\")))]", Truth::Maybe),
            ("#[cfg(feature = \"test-utils\")]", Truth::Maybe),
            ("#[cfg(latest)]", Truth::Maybe),
            // An operator this gate does not model is UNKNOWN, never absent.
            ("#[cfg(version(\"1.80\"))]", Truth::Maybe),
            ("#[cfg(not(test))]", Truth::Always),
            ("#[cfg(not(all(test)))]", Truth::Always),
            ("#[cfg(all())]", Truth::Always),
        ];
        let mut wrong = Vec::new();
        for (source, expected) in cases {
            let parsed = syn::parse_str::<syn::File>(&format!("{source}\nfn item() {{}}"));
            let observed = parsed.as_ref().ok().and_then(|file| {
                let attribute = super::item_attrs(file.items.first()?).first()?;
                let syn::Meta::List(list) = &attribute.meta else {
                    return None;
                };
                let predicate = list.parse_args::<syn::Meta>().ok()?;
                Some((
                    cfg_truth(&predicate),
                    gated_to_test_builds(std::slice::from_ref(attribute)),
                ))
            });
            // An item is SKIPPED exactly when its cfg cannot hold with
            // test = false, and only then.
            if observed != Some((expected, expected == Truth::Never)) {
                wrong.push(format!("{source}: expected {expected:?}, got {observed:?}"));
            }
        }
        assert!(wrong.is_empty(), "{wrong:#?}");
    }
}
