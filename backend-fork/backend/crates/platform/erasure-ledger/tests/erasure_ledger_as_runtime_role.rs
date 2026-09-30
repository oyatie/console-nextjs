#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! The erasure ledger's refusals and its restore-detection verdicts, proven as
//! the GENUINE runtime role `console_rt` (NOSUPERUSER, NOBYPASSRLS, FORCE RLS) —
//! never the BYPASSRLS superuser the default `#[sqlx::test]` pool connects as,
//! which would mask a missing REVOKE or a broken policy.
//!
//! Append-only is proven in TWO layers, because they fail independently and
//! each one alone passes a test shaped for the other. PostgreSQL checks
//! privileges BEFORE it fires triggers, so as `console_rt` the refusal must be
//! `42501`, which proves the REVOKE and would still pass with the trigger
//! dropped; while as the table owner, who keeps the privilege, it must be
//! `P0001`, which proves the trigger and would still pass with the REVOKE
//! dropped. A table carrying only one of the two is mutable in one of the two
//! environments — `ALTER DEFAULT PRIVILEGES` fires for the production applier
//! and not for the `#[sqlx::test]` superuser applier — so both layers are
//! asserted every time.
//!
//! Wording is a constraint here: rows named by this ledger were DELETED FROM THE
//! LIVE CLUSTER and remain reconstructable from the WAL archive. Nothing in this
//! file calls that destruction (ADR-0037 constraint 3).

use console_kernel_core::OrgId;
use console_platform_erasure_ledger::{
    ErasureFacts, ErasureLedgerError, LedgerWitness, RestoreVerdict, append, classify,
    entries_since, head,
};
use console_platform_test_support::{runtime_role_pool, seed_org_and_super_admin};
use sqlx::PgPool;
use std::ops::RangeInclusive;
use std::time::Duration;
use time::OffsetDateTime;
use uuid::Uuid;

const ORG_A: Uuid = Uuid::from_u128(0x0e0a_0e0a_0e0a_0e0a_0e0a_0e0a_0e0a_0e0a);
const ORG_B: Uuid = Uuid::from_u128(0x0e0b_0e0b_0e0b_0e0b_0e0b_0e0b_0e0b_0e0b);

/// The recorded fact set, as the ledger's contract states it. `seq`,
/// `prev_entry_hash` and `entry_hash` are placeholders: the database assigns all
/// three, which is what makes the chain something a caller holding only INSERT
/// cannot forge. A caller-supplied value here must be overwritten.
const SEED_ENTRY_SQL: &str = "INSERT INTO erasure_ledger (\
     org_id, seq, subject_kind, subject_digest, erased_relation, erased_selector, \
     erased_row_count, effective_at, actor, authority, prev_entry_hash, entry_hash) \
     VALUES ($1, 0, 'user', decode(repeat('a1', 32), 'hex'), 'users', $2, 1, \
     now(), 'tester', 'record-only; no legal conclusion asserted', \
     decode(repeat('00', 32), 'hex'), decode(repeat('00', 32), 'hex'))";

fn database_error_code(error: &sqlx::Error) -> Option<String> {
    error
        .as_database_error()?
        .code()
        .map(|code| code.into_owned())
}

fn facts(selector: &str) -> ErasureFacts {
    ErasureFacts {
        subject_kind: "user".to_owned(),
        subject_id: Uuid::from_u128(0x51b1_0000_0000_0000_0000_0000_0000_0001),
        erased_relation: "users".to_owned(),
        erased_selector: selector.to_owned(),
        erased_row_count: 1,
        effective_at: OffsetDateTime::now_utc(),
        actor: "tester".to_owned(),
        authority: "record-only; no legal conclusion asserted".to_owned(),
    }
}

/// Append one entry per sequence in `seqs` through the shipped Rust path,
/// returning the witness each one produced. The selector carries the sequence
/// because several tests below assert on it.
async fn append_run(rt: &PgPool, org: OrgId, seqs: RangeInclusive<i64>) -> Vec<LedgerWitness> {
    let mut witnesses = Vec::new();
    for n in seqs {
        witnesses.push(append(rt, org, &facts(&format!("id = {n}"))).await.unwrap());
    }
    witnesses
}

/// Seed one ledger entry as the migration owner, bypassing the runtime role
/// entirely: the refusal tests must observe the DATABASE refusing a mutation of
/// an EXISTING row, not an empty table quietly matching zero rows.
async fn seed_entry(owner_pool: &PgPool, org: Uuid, selector: &str) {
    sqlx::query(SEED_ENTRY_SQL)
        .bind(org)
        .bind(selector)
        // rls-arming: ok test fixture seeds as owner during setup, before the console_rt role switch
        .execute(owner_pool)
        .await
        .expect("migration 0207 must create erasure_ledger with the recorded fact set");
}

/// Arm the tenant GUC transaction-locally, exactly as `with_org_conn` does.
async fn arm_org(tx: &mut sqlx::Transaction<'_, sqlx::Postgres>, org: Uuid) {
    sqlx::query("SELECT set_config('app.current_org', $1, true)")
        .bind(org.to_string())
        .execute(&mut **tx)
        .await
        .unwrap();
}

/// A point-in-time restore is a CLUSTER event, not a role action: it is not
/// bound by the append-only trigger or by RLS. `session_replication_role =
/// replica` disables user triggers; the superuser owner already bypasses RLS.
/// This is the only honest way to simulate the ledger losing entries it had
/// already recorded.
async fn restore_deleting(owner_pool: &PgPool, statement: &'static str, org: Uuid, seq: i64) {
    let mut tx = owner_pool.begin().await.unwrap();
    sqlx::query("SET LOCAL session_replication_role = replica")
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("SET LOCAL row_security = off")
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query(statement)
        .bind(org)
        .bind(seq)
        .execute(&mut *tx)
        .await
        .expect("migration 0207 must create erasure_ledger");
    tx.commit().await.unwrap();
}

/// The ledger loses every entry past `keep_through`. `keep_through = 0` is the
/// restore that predates the ledger entirely.
async fn simulate_restore_losing_entries_after(owner_pool: &PgPool, org: Uuid, keep_through: i64) {
    restore_deleting(
        owner_pool,
        "DELETE FROM erasure_ledger WHERE org_id = $1 AND seq > $2",
        org,
        keep_through,
    )
    .await;
}

/// A restore that lands BETWEEN recorded entries, leaving a hole rather than a
/// truncation: `lost` is gone while entries after it survive.
async fn simulate_restore_losing_only(owner_pool: &PgPool, org: Uuid, lost: i64) {
    restore_deleting(
        owner_pool,
        "DELETE FROM erasure_ledger WHERE org_id = $1 AND seq = $2",
        org,
        lost,
    )
    .await;
}

// ---------------------------------------------------------------------------
// (a) An UPDATE and (b) a DELETE against a ledger row are refused BY THE
// DATABASE. One body: the two statements differ only in the verb, and a refusal
// that stops being asserted for one of them is exactly what a divergent copy
// would hide.
// ---------------------------------------------------------------------------

/// `statement` must bind `$1` to the org and target the seeded row. Both layers
/// are asserted for it — see the module header for why either alone is a table
/// that is mutable in one of the two environments.
async fn both_layers_refuse(owner_pool: &PgPool, statement: &'static str) {
    seed_org_and_super_admin(owner_pool, ORG_A, "A").await;
    seed_entry(owner_pool, ORG_A, "id = 1").await;
    let rt = runtime_role_pool(owner_pool).await;

    // Layer 1 — the REVOKE. The GUC is armed to the row's OWN org so the row is
    // VISIBLE: under FORCE RLS a mutation against an invisible row affects zero
    // rows and returns Ok, and a bare `expect_err` would then pass for a reason
    // that has nothing to do with append-only.
    let mut tx = rt.begin().await.unwrap();
    arm_org(&mut tx, ORG_A).await;
    let visible: i64 = sqlx::query_scalar("SELECT count(*) FROM erasure_ledger WHERE org_id = $1")
        .bind(ORG_A)
        .fetch_one(&mut *tx)
        .await
        .expect("console_rt must be granted SELECT on erasure_ledger");
    assert_eq!(
        visible, 1,
        "the row `{statement}` targets must be visible to console_rt"
    );
    let runtime_error = sqlx::query(statement)
        .bind(ORG_A)
        .execute(&mut *tx)
        .await
        .expect_err("console_rt must not hold this privilege on erasure_ledger");
    assert_eq!(
        database_error_code(&runtime_error).as_deref(),
        Some("42501"),
        "console_rt's `{statement}` must be refused as a privilege violation, got: {runtime_error}"
    );
    drop(tx);

    // Layer 2 — the trigger. The owner KEEPS the privilege, so a REVOKE-only
    // table would let this through.
    let mut owner_tx = owner_pool.begin().await.unwrap();
    sqlx::query("SET LOCAL row_security = off")
        .execute(&mut *owner_tx)
        .await
        .unwrap();
    let owner_error = sqlx::query(statement)
        .bind(ORG_A)
        .execute(&mut *owner_tx)
        .await
        .expect_err("the append-only trigger must refuse this even for the table owner");
    assert_eq!(
        database_error_code(&owner_error).as_deref(),
        Some("P0001"),
        "the owner's `{statement}` must be refused by the append-only trigger, got: {owner_error}"
    );
}

#[sqlx::test(migrations = "../db/migrations")]
async fn update_of_a_ledger_row_is_refused_by_the_database(owner_pool: PgPool) {
    both_layers_refuse(
        &owner_pool,
        "UPDATE erasure_ledger SET actor = 'rewritten' WHERE org_id = $1",
    )
    .await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn delete_of_a_ledger_row_is_refused_by_the_database(owner_pool: PgPool) {
    both_layers_refuse(&owner_pool, "DELETE FROM erasure_ledger WHERE org_id = $1").await;
}

// ---------------------------------------------------------------------------
// (c) Restore detection. The load-bearing test of the slice.
// ---------------------------------------------------------------------------

/// The negative half, and it is not decoration: a detector that fires in normal
/// operation is a detector nobody will keep switched on.
#[sqlx::test(migrations = "../db/migrations")]
async fn restore_detection_stays_silent_in_normal_operation(owner_pool: PgPool) {
    seed_org_and_super_admin(&owner_pool, ORG_A, "A").await;
    let rt = runtime_role_pool(&owner_pool).await;
    let org = OrgId::from_uuid(ORG_A);

    append_run(&rt, org, 1..=3).await;
    let witness = head(&rt, org)
        .await
        .unwrap()
        .expect("a written ledger has a head");
    assert_eq!(witness.seq, 3);

    append_run(&rt, org, 4..=5).await;

    assert_eq!(
        classify(&rt, org, &witness).await.unwrap(),
        RestoreVerdict::Consistent { head_seq: 5 },
        "appending past a witness must not read as a restore"
    );
}

/// The ledger loses entries it had already recorded and never regains them.
#[sqlx::test(migrations = "../db/migrations")]
async fn restore_detection_fires_when_the_ledger_falls_behind_its_witness(owner_pool: PgPool) {
    seed_org_and_super_admin(&owner_pool, ORG_A, "A").await;
    let rt = runtime_role_pool(&owner_pool).await;
    let org = OrgId::from_uuid(ORG_A);

    append_run(&rt, org, 1..=3).await;
    let witness = head(&rt, org)
        .await
        .unwrap()
        .expect("a written ledger has a head");
    assert_eq!(witness.seq, 3);

    simulate_restore_losing_entries_after(&owner_pool, ORG_A, 1).await;

    assert_eq!(
        classify(&rt, org, &witness).await.unwrap(),
        RestoreVerdict::RolledBack {
            head_seq: 1,
            witness_seq: 3
        },
        "a head behind the witness must be reported as a rollback"
    );
}

/// The scenario a sequence-only detector calls healthy, and the reason this
/// slice exists: the ledger is rolled back and then WRITTEN FORWARD AGAIN, so
/// the head reaches the witness's sequence carrying different content. Comparing
/// sequences alone returns `Consistent` here — evidence that reads as intact
/// while the entries it claims to hold are gone.
#[sqlx::test(migrations = "../db/migrations")]
async fn restore_detection_fires_when_the_witnessed_sequence_holds_other_content(
    owner_pool: PgPool,
) {
    seed_org_and_super_admin(&owner_pool, ORG_A, "A").await;
    let rt = runtime_role_pool(&owner_pool).await;
    let org = OrgId::from_uuid(ORG_A);

    append_run(&rt, org, 1..=3).await;
    let witness = head(&rt, org)
        .await
        .unwrap()
        .expect("a written ledger has a head");
    assert_eq!(witness.seq, 3);

    simulate_restore_losing_entries_after(&owner_pool, ORG_A, 2).await;
    append(
        &rt,
        org,
        &facts("id = 99 (a different erasure, same position)"),
    )
    .await
    .unwrap();

    let resumed = head(&rt, org)
        .await
        .unwrap()
        .expect("a written ledger has a head");
    assert_eq!(
        resumed.seq, witness.seq,
        "the fixture must recreate the witnessed SEQUENCE, or this proves nothing a rollback test does not"
    );
    assert_ne!(
        resumed.entry_hash, witness.entry_hash,
        "the fixture must recreate it with DIFFERENT content"
    );

    assert_eq!(
        classify(&rt, org, &witness).await.unwrap(),
        RestoreVerdict::Forked { witness_seq: 3 },
        "a witnessed sequence holding other content must not be reported as consistent"
    );
}

// ---------------------------------------------------------------------------
// (d) An append that omits a required fact is refused.
// ---------------------------------------------------------------------------

#[sqlx::test(migrations = "../db/migrations")]
async fn an_append_omitting_a_required_fact_is_refused(owner_pool: PgPool) {
    seed_org_and_super_admin(&owner_pool, ORG_A, "A").await;
    let rt = runtime_role_pool(&owner_pool).await;

    let mut tx = rt.begin().await.unwrap();
    arm_org(&mut tx, ORG_A).await;

    // Absent: an entry that cannot say under what authority it was performed.
    let missing = sqlx::query(
        "INSERT INTO erasure_ledger (\
         org_id, seq, subject_kind, subject_digest, erased_relation, erased_selector, \
         erased_row_count, effective_at, actor, authority, prev_entry_hash, entry_hash) \
         VALUES ($1, 0, 'user', decode(repeat('a1', 32), 'hex'), 'users', 'id = 1', 1, \
         now(), 'tester', NULL, decode(repeat('00', 32), 'hex'), decode(repeat('00', 32), 'hex'))",
    )
    .bind(ORG_A)
    .execute(&mut *tx)
    .await
    .expect_err("an entry with no authority is not evidence");
    assert_eq!(
        database_error_code(&missing).as_deref(),
        Some("23502"),
        "an absent required fact must be refused as a not-null violation, got: {missing}"
    );
    drop(tx);

    // Present but empty. A blank string satisfies NOT NULL and says nothing, so
    // the not-null constraint alone does not make the entry evidence.
    let mut tx = rt.begin().await.unwrap();
    arm_org(&mut tx, ORG_A).await;
    let blank = sqlx::query(
        "INSERT INTO erasure_ledger (\
         org_id, seq, subject_kind, subject_digest, erased_relation, erased_selector, \
         erased_row_count, effective_at, actor, authority, prev_entry_hash, entry_hash) \
         VALUES ($1, 0, 'user', decode(repeat('a1', 32), 'hex'), 'users', 'id = 1', 1, \
         now(), 'tester', '   ', decode(repeat('00', 32), 'hex'), decode(repeat('00', 32), 'hex'))",
    )
    .bind(ORG_A)
    .execute(&mut *tx)
    .await
    .expect_err("a blank authority is not evidence either");
    assert_eq!(
        database_error_code(&blank).as_deref(),
        Some("23514"),
        "a blank required fact must be refused as a check violation, got: {blank}"
    );

    // The two refusals above exercise ONE column. "Any required fact" is a
    // statement about all twelve, and a NOT NULL dropped from `actor` or
    // `erased_relation` would leave both assertions above green while the ledger
    // accepted an entry that cannot say who erased what. The catalog answers for
    // every column at once, which is also less code than twelve inserts.
    let nullable: Option<String> = sqlx::query_scalar(
        "SELECT string_agg(attname, ', ' ORDER BY attnum) FROM pg_catalog.pg_attribute \
         WHERE attrelid = 'erasure_ledger'::regclass \
           AND attnum > 0 AND NOT attisdropped AND NOT attnotnull",
    )
    .fetch_one(&owner_pool)
    .await
    .unwrap();
    assert_eq!(
        nullable, None,
        "every column of the recorded fact set must be NOT NULL; these are not"
    );
}

// ---------------------------------------------------------------------------
// (e) Tenant isolation.
// ---------------------------------------------------------------------------

#[sqlx::test(migrations = "../db/migrations")]
async fn one_org_cannot_reach_another_orgs_ledger_entries(owner_pool: PgPool) {
    seed_org_and_super_admin(&owner_pool, ORG_A, "A").await;
    seed_org_and_super_admin(&owner_pool, ORG_B, "B").await;
    seed_entry(&owner_pool, ORG_A, "org A erasure").await;
    seed_entry(&owner_pool, ORG_B, "org B erasure").await;
    let rt = runtime_role_pool(&owner_pool).await;

    let mut tx = rt.begin().await.unwrap();
    arm_org(&mut tx, ORG_A).await;

    // The WRITE refusal is asserted FIRST and deliberately so: if it is asserted
    // after another statement, a refusal originating anywhere else satisfies it
    // and the isolation proof quietly evaporates while the assertion still passes.
    let cross_org = sqlx::query(SEED_ENTRY_SQL)
        .bind(ORG_B)
        .bind("smuggled into org B")
        .execute(&mut *tx)
        .await
        .expect_err("org A must not write into org B's ledger");
    assert_eq!(
        database_error_code(&cross_org).as_deref(),
        Some("42501"),
        "the cross-org append must be refused by row-level security, got: {cross_org}"
    );
    drop(tx);

    let mut tx = rt.begin().await.unwrap();
    arm_org(&mut tx, ORG_A).await;
    let visible: Vec<String> = sqlx::query_scalar("SELECT erased_selector FROM erasure_ledger")
        .fetch_all(&mut *tx)
        .await
        .expect("console_rt must be granted SELECT on erasure_ledger");
    tx.commit().await.unwrap();

    // Both directions. Asserting only the absence passes against a table that
    // returns nothing at all, which is not isolation — it is a broken grant.
    assert!(
        visible.iter().any(|s| s == "org A erasure"),
        "org A must see its own ledger entry, saw: {visible:?}"
    );
    assert!(
        !visible.iter().any(|s| s == "org B erasure"),
        "org A must not see org B's ledger entry, saw: {visible:?}"
    );
}

// ---------------------------------------------------------------------------
// The read path a re-applier depends on.
// ---------------------------------------------------------------------------

/// Deliverable 4's read path: after a restore is detected, something has to
/// replay what the ledger recorded past the witness. That replay is only
/// possible if the entries come back in sequence order carrying the SCOPE —
/// relation, selector, row count — not merely the fact that something happened.
#[sqlx::test(migrations = "../db/migrations")]
async fn entries_since_returns_the_replay_set_in_order_with_its_scope(owner_pool: PgPool) {
    seed_org_and_super_admin(&owner_pool, ORG_A, "A").await;
    let rt = runtime_role_pool(&owner_pool).await;
    let org = OrgId::from_uuid(ORG_A);

    let witnesses = append_run(&rt, org, 1..=3).await;

    let replay = entries_since(&rt, org, 1).await.unwrap();
    assert_eq!(
        replay.iter().map(|e| e.seq).collect::<Vec<_>>(),
        vec![2, 3],
        "the replay set must be everything after the witness, in sequence order"
    );
    assert_eq!(replay[0].erased_relation, "users");
    assert_eq!(replay[0].erased_selector, "id = 2");
    assert_eq!(replay[0].erased_row_count, 1);
    assert_eq!(
        replay[1].entry_hash, witnesses[2].entry_hash,
        "the replayed entry must be the one the witness names"
    );
    assert_eq!(
        replay[1].prev_entry_hash, witnesses[1].entry_hash,
        "the chain must link each replayed entry to its predecessor"
    );

    // A witness at the head has nothing to replay.
    assert!(entries_since(&rt, org, 3).await.unwrap().is_empty());
}

// ---------------------------------------------------------------------------
// Coverage phase. MEASURED with cargo-llvm-cov 0.8.7 against the eight tests
// above: src/lib.rs at 84.78% region / 71.43% function coverage, with lines
// 160-162 (`hash32`'s MalformedHash), 165-170 (`is_unique_violation`, whole
// function), 184 (the retry arm), 188 (`Contention`) and 257 (an unwritten
// ledger's head) never executed. The tests below close those, and also the
// properties LINE COVERAGE CANNOT SEE: that the DATABASE and not the caller
// assigns the chain, that `entry_hash` is the framing migration 0207 documents
// for an external verifier, and that the shipped Rust read path — not merely
// raw SQL — is confined to the armed org.
// ---------------------------------------------------------------------------

/// The predecessor a genesis entry carries.
const GENESIS: [u8; 32] = [0u8; 32];

/// Bounds on the append-contention rendezvous below. A poll rather than a fixed
/// sleep: the test must observe the contender ACTUALLY blocked, or it proves
/// nothing about the retry.
const CONTENTION_DEADLINE: Duration = Duration::from_secs(10);
const CONTENTION_POLL: Duration = Duration::from_millis(10);

/// The full recorded fact set with a caller-chosen sequence and chain. Every
/// value here is one the caller must NOT be able to keep.
const FORGED_ENTRY_SQL: &str = "INSERT INTO erasure_ledger (\
     org_id, seq, subject_kind, subject_digest, erased_relation, erased_selector, \
     erased_row_count, effective_at, actor, authority, prev_entry_hash, entry_hash) \
     VALUES ($1, 999, 'user', decode(repeat('a1', 32), 'hex'), 'users', $2, 1, \
     now(), 'tester', 'record-only; no legal conclusion asserted', \
     decode(repeat('de', 32), 'hex'), decode(repeat('ad', 32), 'hex')) \
     RETURNING seq, prev_entry_hash, entry_hash";

// ---------------------------------------------------------------------------
// The chain is assigned by the database, not by the appender.
// ---------------------------------------------------------------------------

/// `console_rt` holds INSERT and nothing else, and the entire restore-detection
/// design rests on that being insufficient to forge a chain. A caller that could
/// choose `seq` would park entries beyond the head; one that could choose
/// `prev_entry_hash` would re-parent the chain around an entry it wanted gone.
/// Either makes a held witness meaningless. Migration 0207's BEFORE INSERT
/// trigger overwrites all three unconditionally — and nothing above asserts it,
/// because the spec tests supply well-behaved placeholders and never check that
/// a badly-behaved one is refused the chance to stick.
#[sqlx::test(migrations = "../db/migrations")]
async fn the_database_and_not_the_caller_assigns_the_chain(owner_pool: PgPool) {
    seed_org_and_super_admin(&owner_pool, ORG_A, "A").await;
    let rt = runtime_role_pool(&owner_pool).await;

    let mut tx = rt.begin().await.unwrap();
    arm_org(&mut tx, ORG_A).await;
    let (seq, prev, entry): (i64, Vec<u8>, Vec<u8>) = sqlx::query_as(FORGED_ENTRY_SQL)
        .bind(ORG_A)
        .bind("id = 1")
        .fetch_one(&mut *tx)
        .await
        .expect("console_rt must hold INSERT on erasure_ledger");
    tx.commit().await.unwrap();

    assert_eq!(
        seq, 1,
        "the caller's seq=999 must be replaced by the database's own"
    );
    assert_eq!(
        prev,
        GENESIS.to_vec(),
        "the caller's chosen predecessor must be replaced by genesis"
    );
    assert_ne!(
        entry,
        vec![0xad_u8; 32],
        "the caller's chosen entry hash must be replaced by the computed one"
    );

    // And again, so the SECOND entry proves the trigger links rather than merely
    // zeroing: a trigger that always wrote genesis would pass the first half.
    let mut tx = rt.begin().await.unwrap();
    arm_org(&mut tx, ORG_A).await;
    let (second_seq, second_prev, _): (i64, Vec<u8>, Vec<u8>) = sqlx::query_as(FORGED_ENTRY_SQL)
        .bind(ORG_A)
        .bind("id = 2")
        .fetch_one(&mut *tx)
        .await
        .expect("console_rt must hold INSERT on erasure_ledger");
    tx.commit().await.unwrap();

    assert_eq!(
        second_seq, 2,
        "the second entry must take the next sequence, not 999"
    );
    assert_eq!(
        second_prev, entry,
        "the second entry must be chained to the first's REAL hash, not the caller's"
    );
}

// ---------------------------------------------------------------------------
// The hash framing an external verifier has to reproduce.
// ---------------------------------------------------------------------------

/// SHA-256 of migration 0207's documented preimage for a fully fixed tuple,
/// computed OUTSIDE PostgreSQL from the written framing alone — not by calling
/// the function it checks. Reproduce it with any SHA-256 implementation:
///
/// ```text
/// ns(s)    = len(utf8(s)) || ':' || s
/// preimage = 'console.erasure_ledger.v1'
///          || ns('0e0a0e0a-0e0a-0e0a-0e0a-0e0a0e0a0e0a')  -- org_id
///          || ns('1')                                     -- seq
///          || ns('00' x 32)                               -- prev_entry_hash, hex
///          || ns('user')                                  -- subject_kind
///          || ns('a1' x 32)                               -- subject_digest, hex
///          || ns('users')                                 -- erased_relation
///          || ns('id = 1')                                -- erased_selector
///          || ns('1')                                     -- erased_row_count
///          || ns('tester')                                -- actor
///          || ns('record-only; no legal conclusion asserted')
///          || ns('2026-07-31T00:00:00.000000Z')           -- effective_at, UTC
/// ```
const DOCUMENTED_ENTRY_HASH: &str =
    "ad4998011f3dce6d03d325843da75322e3bacb3b528bf935032e4bd10ea85df8";

/// The migration specifies the framing so that an external holder of a witness
/// never has to read plpgsql to verify one — and an external verifier is the
/// only thing that can ever make this ledger's restore detection non-inert. If
/// the function and the specification drift, every such verifier is silently
/// wrong, and no test above would notice: they all compare hashes the same
/// function produced, so a reordered or dropped field stays self-consistent.
#[sqlx::test(migrations = "../db/migrations")]
async fn the_entry_hash_is_the_framing_the_migration_documents(owner_pool: PgPool) {
    let computed: String = sqlx::query_scalar(
        "SELECT encode(erasure_ledger_entry_hash($1, 1::bigint, \
         decode(repeat('00', 32), 'hex'), 'user', decode(repeat('a1', 32), 'hex'), \
         'users', 'id = 1', 1::bigint, 'tester', \
         'record-only; no legal conclusion asserted', \
         '2026-07-31T00:00:00Z'::timestamptz), 'hex')",
    )
    .bind(ORG_A)
    .fetch_one(&owner_pool)
    .await
    .expect("migration 0207 must create erasure_ledger_entry_hash");

    assert_eq!(
        computed, DOCUMENTED_ENTRY_HASH,
        "the database's entry hash must be the preimage the migration documents"
    );

    // The length prefix, and the reason the migration gives for choosing it over
    // a separator: `erased_selector` is free text, so any separator can occur
    // inside a field. ('ab','c') and ('a','bc') both concatenate to 'abc'.
    let framing_separates_fields: bool = sqlx::query_scalar(
        "SELECT erasure_ledger_entry_hash($1, 1::bigint, decode(repeat('00', 32), 'hex'), \
         'user', decode(repeat('a1', 32), 'hex'), 'ab', 'c', 1::bigint, 'tester', 'auth', \
         '2026-07-31T00:00:00Z'::timestamptz) \
         <> erasure_ledger_entry_hash($1, 1::bigint, decode(repeat('00', 32), 'hex'), \
         'user', decode(repeat('a1', 32), 'hex'), 'a', 'bc', 1::bigint, 'tester', 'auth', \
         '2026-07-31T00:00:00Z'::timestamptz)",
    )
    .bind(ORG_A)
    .fetch_one(&owner_pool)
    .await
    .unwrap();

    assert!(
        framing_separates_fields,
        "relation 'ab' + selector 'c' must not hash alike with relation 'a' + selector 'bc'"
    );
}

/// `org_id` is the FIRST field of the preimage, which is what makes a witness
/// bind to the tenant that produced it. `classify` scopes its read by the org
/// argument and never inspects `witness.org_id`, so a caller that transposes two
/// tenants' witnesses gets a verdict computed against the wrong ledger. That
/// must fail CLOSED — a false alarm, never a false `Consistent` — and it does
/// only because the org is inside the hash. Drop it from the preimage and two
/// tenants performing the same erasure at the same position produce the same
/// hash, and the transposition reads as healthy.
#[sqlx::test(migrations = "../db/migrations")]
async fn a_witness_from_another_org_can_never_read_as_consistent(owner_pool: PgPool) {
    seed_org_and_super_admin(&owner_pool, ORG_A, "A").await;
    seed_org_and_super_admin(&owner_pool, ORG_B, "B").await;
    let rt = runtime_role_pool(&owner_pool).await;
    let (org_a, org_b) = (OrgId::from_uuid(ORG_A), OrgId::from_uuid(ORG_B));

    // Byte-identical facts, same position in each org's chain. Everything the
    // hash covers except org_id is equal.
    let shared = facts("id = 1");
    let witness_a = append(&rt, org_a, &shared).await.unwrap();
    let witness_b = append(&rt, org_b, &shared).await.unwrap();

    assert_eq!(
        witness_a.seq, witness_b.seq,
        "both witnesses must name the same position"
    );
    assert_ne!(
        witness_a.entry_hash, witness_b.entry_hash,
        "identical facts in different tenants must not share an entry hash"
    );

    assert_eq!(
        classify(&rt, org_b, &witness_a).await.unwrap(),
        RestoreVerdict::Forked {
            witness_seq: witness_a.seq
        },
        "org A's witness read against org B's ledger must not read as consistent"
    );

    // The same subject, pseudonymised per tenant: the digest carries org_id too,
    // so one tenant's ledger cannot be scanned for a subject known from another.
    let digest_a = entries_since(&rt, org_a, 0).await.unwrap()[0].subject_digest;
    let digest_b = entries_since(&rt, org_b, 0).await.unwrap()[0].subject_digest;
    assert_ne!(
        digest_a, digest_b,
        "the same subject id must digest differently in different tenants"
    );
}

// ---------------------------------------------------------------------------
// Tenant isolation of the SHIPPED read path, not just of raw SQL.
// ---------------------------------------------------------------------------

/// Test (e) above proves the POLICY, through hand-written SQL with no org
/// predicate of its own. It does not prove that `head`, `entries_since` and
/// `classify` — the functions a re-applier will actually call — arm the tenant
/// GUC on the connection they read from. A read helper that forgot to route
/// through `with_org_conn` leaves test (e) green and returns NOTHING under FORCE
/// RLS, which a re-applier reads as "no erasures to re-apply".
///
/// What this does NOT distinguish, stated because it was measured rather than
/// assumed: confinement here is enforced TWICE, by the RLS policy and by each
/// query's own `org_id = $1`. Widening the policy to `USING (true)` alone leaves
/// this test GREEN — verified — because the predicate still holds the line. It
/// goes red when the read is unarmed, and when both layers give way together.
/// The policy on its own is what test (e) pins.
#[sqlx::test(migrations = "../db/migrations")]
async fn the_rust_read_path_is_confined_to_the_armed_org(owner_pool: PgPool) {
    seed_org_and_super_admin(&owner_pool, ORG_A, "A").await;
    seed_org_and_super_admin(&owner_pool, ORG_B, "B").await;
    let rt = runtime_role_pool(&owner_pool).await;
    let (org_a, org_b) = (OrgId::from_uuid(ORG_A), OrgId::from_uuid(ORG_B));

    assert!(
        head(&rt, org_a).await.unwrap().is_none(),
        "a ledger that was never written has no head"
    );

    let witness_a = append(&rt, org_a, &facts("org A erasure")).await.unwrap();
    for n in 1..=3 {
        append(&rt, org_b, &facts(&format!("org B erasure {n}")))
            .await
            .unwrap();
    }

    let head_a = head(&rt, org_a).await.unwrap().expect("org A has a head");
    assert_eq!(
        head_a, witness_a,
        "org A's head must be org A's own entry, not the deeper ledger next door"
    );

    let replay_a = entries_since(&rt, org_a, 0).await.unwrap();
    assert_eq!(
        replay_a
            .iter()
            .map(|entry| entry.erased_selector.as_str())
            .collect::<Vec<_>>(),
        vec!["org A erasure"],
        "org A's replay set must hold org A's entries and only those"
    );
    assert!(
        replay_a.iter().all(|entry| entry.org_id == ORG_A),
        "every replayed entry must belong to the org that asked"
    );

    // And the verdict path: org B's witness sits at a sequence org A does not
    // even have, so a read that leaked across the boundary would find it.
    let witness_b = head(&rt, org_b).await.unwrap().expect("org B has a head");
    assert_eq!(witness_b.seq, 3);
    assert_eq!(
        classify(&rt, org_a, &witness_b).await.unwrap(),
        RestoreVerdict::RolledBack {
            head_seq: 1,
            witness_seq: 3
        },
        "org A's classify must see only org A's ledger depth"
    );
}

// ---------------------------------------------------------------------------
// Restore verdicts at the boundaries the spec tests stopped short of.
// ---------------------------------------------------------------------------

/// The restore that predates the ledger entirely — the plainest form of the
/// failure this slice exists for, and the one the spec tests skip: they always
/// keep at least one entry, so `classify`'s `COALESCE(max(seq), 0)` never
/// returns its default and `head`'s empty-ledger branch never runs. Without the
/// COALESCE this is a NULL, not a zero, and the verdict is an error rather than
/// `RolledBack`.
#[sqlx::test(migrations = "../db/migrations")]
async fn restore_detection_fires_when_the_entire_ledger_is_lost(owner_pool: PgPool) {
    seed_org_and_super_admin(&owner_pool, ORG_A, "A").await;
    let rt = runtime_role_pool(&owner_pool).await;
    let org = OrgId::from_uuid(ORG_A);

    append_run(&rt, org, 1..=3).await;
    let witness = head(&rt, org)
        .await
        .unwrap()
        .expect("a written ledger has a head");

    simulate_restore_losing_entries_after(&owner_pool, ORG_A, 0).await;

    assert_eq!(
        classify(&rt, org, &witness).await.unwrap(),
        RestoreVerdict::RolledBack {
            head_seq: 0,
            witness_seq: 3
        },
        "an emptied ledger must be reported as a rollback, not as an absent org"
    );
    assert!(
        head(&rt, org).await.unwrap().is_none(),
        "an emptied ledger has no head"
    );
    assert!(
        entries_since(&rt, org, 0).await.unwrap().is_empty(),
        "an emptied ledger has nothing to replay"
    );
}

/// A restore that leaves a HOLE: the witnessed sequence is gone while later
/// entries survive. `head_seq >= witness.seq` so the rollback branch does not
/// fire, and the witnessed row is absent rather than different — the other input
/// to the same match arm the fork test exercises, and the one that would read as
/// `Consistent` if absence were ever treated as "nothing to disagree with".
#[sqlx::test(migrations = "../db/migrations")]
async fn restore_detection_fires_when_the_witnessed_sequence_is_a_hole(owner_pool: PgPool) {
    seed_org_and_super_admin(&owner_pool, ORG_A, "A").await;
    let rt = runtime_role_pool(&owner_pool).await;
    let org = OrgId::from_uuid(ORG_A);

    let witnesses = append_run(&rt, org, 1..=3).await;
    let witness = witnesses[1];
    assert_eq!(witness.seq, 2);

    simulate_restore_losing_only(&owner_pool, ORG_A, 2).await;

    let head_seq = head(&rt, org).await.unwrap().expect("entry 3 survives").seq;
    assert_eq!(
        head_seq, 3,
        "the fixture must leave a hole, not a truncation"
    );

    assert_eq!(
        classify(&rt, org, &witness).await.unwrap(),
        RestoreVerdict::Forked { witness_seq: 2 },
        "a missing witnessed entry under a live head must not read as consistent"
    );
}

/// The hole the witness comparison alone cannot see, and the reason this test
/// exists: a restore that loses entries ABOVE the witness while leaving both the
/// witness and a later head intact. `head_seq >= witness.seq`, and the witnessed
/// row is byte-for-byte the one that was witnessed, so every check the witness
/// itself can make passes — and entries the ledger recorded are gone. That is
/// the "reads as evidence while being empty" failure the whole slice exists to
/// make detectable, and it is invisible to a comparison anchored at one point.
///
/// It is detectable from a whole-ledger invariant instead: migration 0207's
/// trigger assigns `seq` as `max(seq) + 1` starting at 1 and nothing can delete
/// a row, so a live ledger always satisfies `count(*) = max(seq)`. A gap breaks
/// that identity no matter where the witness sits.
#[sqlx::test(migrations = "../db/migrations")]
async fn restore_detection_fires_when_entries_above_the_witness_are_lost(owner_pool: PgPool) {
    seed_org_and_super_admin(&owner_pool, ORG_A, "A").await;
    let rt = runtime_role_pool(&owner_pool).await;
    let org = OrgId::from_uuid(ORG_A);

    let witnesses = append_run(&rt, org, 1..=3).await;
    let witness = witnesses[0];
    assert_eq!(witness.seq, 1);

    simulate_restore_losing_only(&owner_pool, ORG_A, 2).await;

    // Everything the witness alone can check still agrees: the head is past it
    // and its own entry is untouched.
    let head_seq = head(&rt, org).await.unwrap().expect("entry 3 survives").seq;
    assert_eq!(
        head_seq, 3,
        "the fixture must leave the head above the witness"
    );
    assert_eq!(
        entries_since(&rt, org, 0).await.unwrap()[0].entry_hash,
        witness.entry_hash,
        "the witnessed entry itself must survive, or this is the plain fork case"
    );

    assert_eq!(
        classify(&rt, org, &witness).await.unwrap(),
        RestoreVerdict::Gapped {
            head_seq: 3,
            entry_count: 2
        },
        "a ledger holding fewer entries than its own head must not read as consistent"
    );

    // And the replay set the re-applier would act on is short by exactly the
    // entry that vanished — which is why the verdict has to fire before it.
    assert_eq!(
        entries_since(&rt, org, witness.seq)
            .await
            .unwrap()
            .iter()
            .map(|entry| entry.seq)
            .collect::<Vec<_>>(),
        vec![3],
        "the replay set silently skips the lost entry, so the verdict is the only guard"
    );
}

// ---------------------------------------------------------------------------
// TRUNCATE. The one statement that empties a table without firing a row trigger.
// ---------------------------------------------------------------------------

/// `DELETE` is refused twice over — by the REVOKE for `console_rt` and by the
/// row trigger for the owner. `TRUNCATE` is refused only once: the REVOKE stops
/// `console_rt`, and `BEFORE DELETE ... FOR EACH ROW` does not fire for it at
/// all, so the owner who keeps the privilege can empty the ledger in one
/// statement and leave no row for any trigger to object to. An evidence table
/// whose stated property is that it cannot be silently emptied has to refuse the
/// statement whose entire purpose is to silently empty it.
#[sqlx::test(migrations = "../db/migrations")]
async fn truncate_of_the_ledger_is_refused_by_the_database(owner_pool: PgPool) {
    seed_org_and_super_admin(&owner_pool, ORG_A, "A").await;
    seed_entry(&owner_pool, ORG_A, "id = 1").await;
    let rt = runtime_role_pool(&owner_pool).await;

    // Layer 1 — the REVOKE, as the runtime role.
    let mut tx = rt.begin().await.unwrap();
    arm_org(&mut tx, ORG_A).await;
    let runtime_error = sqlx::query("TRUNCATE erasure_ledger")
        .execute(&mut *tx)
        .await
        .expect_err("console_rt must not hold TRUNCATE on erasure_ledger");
    assert_eq!(
        database_error_code(&runtime_error).as_deref(),
        Some("42501"),
        "console_rt's TRUNCATE must be refused as a privilege violation, got: {runtime_error}"
    );
    drop(tx);

    // Layer 2 — the trigger, as the owner who keeps the privilege. Without a
    // statement-level guard this succeeds and the ledger is empty.
    let mut owner_tx = owner_pool.begin().await.unwrap();
    let owner_error = sqlx::query("TRUNCATE erasure_ledger")
        .execute(&mut *owner_tx)
        .await
        .expect_err("the append-only trigger must refuse TRUNCATE even for the table owner");
    assert_eq!(
        database_error_code(&owner_error).as_deref(),
        Some("P0001"),
        "the owner's TRUNCATE must be refused by the append-only trigger, got: {owner_error}"
    );
    drop(owner_tx);

    let surviving: i64 = sqlx::query_scalar("SELECT count(*) FROM erasure_ledger")
        .fetch_one(&owner_pool)
        .await
        .unwrap();
    assert_eq!(surviving, 1, "the entry must still be there");
}

// ---------------------------------------------------------------------------
// Tenant teardown against an unconditional DELETE trigger.
// ---------------------------------------------------------------------------

/// Migration 0207's sharpest call — no `REFERENCES organizations(id)`, and no
/// `app.platform_force_remove_org` bypass branch in the DELETE trigger — and
/// nothing asserted it. The two are load-bearing together: 0196's closure loop
/// selects children by FOREIGN KEY to `organizations`, so an `a`/`r` FK would
/// put `erasure_ledger` in that loop and a CASCADE FK would be cascaded onto by
/// `DELETE FROM organizations`. Either lands on the unconditional trigger and
/// raises `P0001`, and `platform_force_remove_organization` — a SECURITY DEFINER
/// that deletes some forty relations before it reaches `organizations` — fails
/// only after it has begun, at operator time.
///
/// So this asserts BOTH halves of that decision at once: teardown completes, and
/// the erasure record survives the tenant whose data it was. Add an FK to this
/// table in any later migration and this test is what goes red.
#[sqlx::test(migrations = "../db/migrations")]
async fn tenant_teardown_completes_and_the_erasure_record_outlives_the_tenant(owner_pool: PgPool) {
    seed_org_and_super_admin(&owner_pool, ORG_A, "A").await;
    seed_entry(&owner_pool, ORG_A, "id = 1").await;

    sqlx::query("UPDATE organizations SET status = 'ARCHIVED' WHERE id = $1")
        .bind(ORG_A)
        // rls-arming: ok test fixture archives as owner during setup, before force removal
        .execute(&owner_pool)
        .await
        .unwrap();

    let outcome: String = sqlx::query_scalar("SELECT platform_force_remove_organization($1)")
        .bind(ORG_A)
        .fetch_one(&owner_pool)
        .await
        .expect("force removal must not be blocked by the append-only ledger");
    assert_eq!(
        outcome, "removed",
        "the erasure ledger must not stand between a tenant and its teardown"
    );

    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM organizations WHERE id = $1")
            .bind(ORG_A)
            .fetch_one(&owner_pool)
            .await
            .unwrap(),
        0,
        "the tenant must actually be gone, or 'removed' proves nothing"
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM erasure_ledger WHERE org_id = $1")
            .bind(ORG_A)
            .fetch_one(&owner_pool)
            .await
            .unwrap(),
        1,
        "the record that personal data was erased must outlive the tenant it was about"
    );
}

// ---------------------------------------------------------------------------
// Concurrent appends. The retry is what stands between a lost erasure record
// and a caller that believes it recorded one.
// ---------------------------------------------------------------------------

/// Two appenders reach for the same next sequence; the loser gets `23505` from
/// `(org_id, seq)` — the constraint that also prevents a fork — and `append`
/// retries against the winner's head. Untested until now: `is_unique_violation`
/// and the retry arm were the largest never-executed region in the crate. If the
/// retry were dropped, the loser would surface a raw unique violation and the
/// erasure it recorded would be silently absent from the ledger.
///
/// The rendezvous is a poll, not a sleep: the contender must be OBSERVED blocked
/// on the held transaction before it is released, or a scheduler that happened to
/// run them in series would make this test pass while covering nothing.
#[sqlx::test(migrations = "../db/migrations")]
async fn a_losing_appender_retries_onto_the_winners_head(owner_pool: PgPool) {
    seed_org_and_super_admin(&owner_pool, ORG_A, "A").await;
    let rt = runtime_role_pool(&owner_pool).await;
    let org = OrgId::from_uuid(ORG_A);

    // The winner: seq 1 written but NOT yet committed, so the contender computes
    // the same next sequence and then blocks on the primary key.
    let mut winner = rt.begin().await.unwrap();
    arm_org(&mut winner, ORG_A).await;
    let winner_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *winner)
        .await
        .unwrap();
    sqlx::query(SEED_ENTRY_SQL)
        .bind(ORG_A)
        .bind("the winner")
        .execute(&mut *winner)
        .await
        .expect("console_rt must hold INSERT on erasure_ledger");

    let contender_pool = rt.clone();
    let contender =
        tokio::spawn(async move { append(&contender_pool, org, &facts("the loser")).await });

    let deadline = tokio::time::Instant::now() + CONTENTION_DEADLINE;
    loop {
        let blocked: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM pg_stat_activity blocked_activity \
             WHERE blocked_activity.wait_event_type = 'Lock' \
               AND $1 = ANY(pg_blocking_pids(blocked_activity.pid)))",
        )
        .bind(winner_pid)
        .fetch_one(&owner_pool)
        .await
        .unwrap();
        if blocked {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the contending append never blocked on the held insert, so no retry was exercised"
        );
        tokio::time::sleep(CONTENTION_POLL).await;
    }

    winner.commit().await.unwrap();
    let loser = contender
        .await
        .unwrap()
        .expect("a losing appender must retry, not surface the unique violation");

    assert_eq!(
        loser.seq, 2,
        "the retry must re-read the head and take the sequence after the winner's"
    );

    let chain = entries_since(&rt, org, 0).await.unwrap();
    assert_eq!(
        chain
            .iter()
            .map(|entry| entry.erased_selector.as_str())
            .collect::<Vec<_>>(),
        vec!["the winner", "the loser"],
        "both appends must be present, in the order the database granted them"
    );
    assert_eq!(
        chain[0].prev_entry_hash, GENESIS,
        "the first entry is genesis-rooted"
    );
    assert_eq!(
        chain[1].prev_entry_hash, chain[0].entry_hash,
        "the retried append must chain onto the winner, not onto the head it first read"
    );
}

// ---------------------------------------------------------------------------
// A drifted schema is an error, not a silently shortened hash.
// ---------------------------------------------------------------------------

/// `hash32` is the last thing standing between a ledger whose CHECK constraints
/// have drifted and a caller handed a value that reads like a hash but is not
/// one. Only reachable with the constraint gone, which is exactly the state it
/// exists for; the alternative — `copy_from_slice` or a truncating cast — would
/// hand back 32 bytes of something else and a witness comparison would then be
/// comparing padding.
#[sqlx::test(migrations = "../db/migrations")]
async fn a_hash_column_that_is_not_32_bytes_is_reported_not_truncated(owner_pool: PgPool) {
    seed_org_and_super_admin(&owner_pool, ORG_A, "A").await;
    let rt = runtime_role_pool(&owner_pool).await;
    let org = OrgId::from_uuid(ORG_A);

    let mut tx = owner_pool.begin().await.unwrap();
    sqlx::query("ALTER TABLE erasure_ledger DROP CONSTRAINT erasure_ledger_entry_hash_check")
        .execute(&mut *tx)
        .await
        .expect("migration 0207 must constrain entry_hash to 32 octets");
    // A cluster-level restore is not bound by the assign trigger, so neither is
    // the drifted schema this stands in for.
    sqlx::query("SET LOCAL session_replication_role = replica")
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("SET LOCAL row_security = off")
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO erasure_ledger (\
         org_id, seq, subject_kind, subject_digest, erased_relation, erased_selector, \
         erased_row_count, effective_at, actor, authority, prev_entry_hash, entry_hash) \
         VALUES ($1, 1, 'user', decode(repeat('a1', 32), 'hex'), 'users', 'id = 1', 1, \
         now(), 'tester', 'record-only; no legal conclusion asserted', \
         decode(repeat('00', 32), 'hex'), decode(repeat('bb', 16), 'hex'))",
    )
    .bind(ORG_A)
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();

    match head(&rt, org).await {
        Err(ErasureLedgerError::MalformedHash { seq, len }) => {
            assert_eq!(
                (seq, len),
                (1, 16),
                "the error must name the row and the length it found"
            );
        }
        other => panic!("a 16-octet entry hash must be an error, got: {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Deny by default, for the inputs that are not a wrong org.
// ---------------------------------------------------------------------------

/// Test (e) proves the WRONG org is refused. Migration 0207:253 claims more
/// than that — that an UNARMED caller "sees none and the insert then fails the
/// policy's WITH CHECK", closed either way — and nothing asserted it. The
/// distinction matters because the two reach the policy differently: a wrong
/// org makes the predicate FALSE, while an absent or blank GUC makes it NULL,
/// and a policy written with `COALESCE(..., org_id)` or a bare
/// `current_setting('app.current_org')` would keep test (e) green while opening
/// the unarmed path. Both are the deny-by-default input a caller reaches by
/// forgetting `with_org_conn`, not by attacking.
#[sqlx::test(migrations = "../db/migrations")]
async fn an_unarmed_caller_can_neither_read_nor_append(owner_pool: PgPool) {
    seed_org_and_super_admin(&owner_pool, ORG_A, "A").await;
    seed_entry(&owner_pool, ORG_A, "id = 1").await;
    let rt = runtime_role_pool(&owner_pool).await;

    // "" is the value `NULLIF` exists for; skipping `arm_org` entirely is the
    // absent case, where `current_setting(..., true)` yields NULL.
    for armed in [None, Some("")] {
        let mut tx = rt.begin().await.unwrap();
        if let Some(blank) = armed {
            sqlx::query("SELECT set_config('app.current_org', $1, true)")
                .bind(blank)
                .execute(&mut *tx)
                .await
                .unwrap();
        }

        let visible: i64 = sqlx::query_scalar("SELECT count(*) FROM erasure_ledger")
            .fetch_one(&mut *tx)
            .await
            .expect("console_rt must be granted SELECT on erasure_ledger");
        assert_eq!(
            visible, 0,
            "an unarmed read must see nothing, not the whole table (armed: {armed:?})"
        );

        let refused = sqlx::query(SEED_ENTRY_SQL)
            .bind(ORG_A)
            .bind("appended with no tenant armed")
            .execute(&mut *tx)
            .await
            .expect_err("an unarmed append must be refused");
        assert_eq!(
            database_error_code(&refused).as_deref(),
            Some("42501"),
            "an unarmed append must be refused by row-level security (armed: {armed:?}), \
             got: {refused}"
        );
    }
}

// ---------------------------------------------------------------------------
// The chain assignment against a caller who controls name resolution.
// ---------------------------------------------------------------------------

/// `the_database_and_not_the_caller_assigns_the_chain` proves the trigger
/// overwrites a caller-supplied `seq` and `prev_entry_hash`. It proves that
/// against a caller who sends bad VALUES. It says nothing about a caller who
/// changes what the trigger's own unqualified `erasure_ledger` RESOLVES TO.
///
/// `erasure_ledger_assign_chain` is SECURITY INVOKER plpgsql, so without a
/// `SET search_path` it inherits the caller's. Nothing in this repository
/// revokes TEMP on the database — 0168 revokes only CREATE on schema `public` —
/// so `console_rt`, holding INSERT and nothing else, can put its own
/// `erasure_ledger` in `pg_temp`, name `pg_temp` ahead of `public`, and have the
/// trigger read the head from a table it wrote. Temp tables carry no RLS, so the
/// org floor does not narrow that read either.
///
/// What it buys the attacker is the whole restore-detection design: after a real
/// rollback it re-creates the exact `(seq, entry_hash)` an external holder
/// witnessed and `classify` answers `Consistent` for a ledger that lost entries.
/// The cheap version is a `seq` parked far above the head, which breaks the
/// `count(*) = max(seq)` identity permanently and leaves `classify` returning
/// `Gapped` for a ledger nothing happened to.
#[sqlx::test(migrations = "../db/migrations")]
async fn the_caller_cannot_forge_the_chain_by_controlling_name_resolution(owner_pool: PgPool) {
    seed_org_and_super_admin(&owner_pool, ORG_A, "A").await;
    let rt = runtime_role_pool(&owner_pool).await;

    let mut tx = rt.begin().await.unwrap();
    arm_org(&mut tx, ORG_A).await;

    // A decoy head, in a schema the runtime role owns outright.
    sqlx::query("CREATE TEMP TABLE erasure_ledger (org_id UUID, seq BIGINT, entry_hash BYTEA)")
        .execute(&mut *tx)
        .await
        .expect("the attack's premise: console_rt retains the default TEMP privilege");
    sqlx::query(
        "INSERT INTO pg_temp.erasure_ledger VALUES ($1, 500, decode(repeat('cc', 32), 'hex'))",
    )
    .bind(ORG_A)
    .execute(&mut *tx)
    .await
    .unwrap();
    sqlx::query("SET LOCAL search_path = pg_temp, public")
        .execute(&mut *tx)
        .await
        .unwrap();

    // Schema-qualified, so the INSERT itself lands on the real ledger. Only the
    // trigger's own reads are redirected.
    let (seq, prev): (i64, Vec<u8>) = sqlx::query_as(
        "INSERT INTO public.erasure_ledger (\
         org_id, seq, subject_kind, subject_digest, erased_relation, erased_selector, \
         erased_row_count, effective_at, actor, authority, prev_entry_hash, entry_hash) \
         VALUES ($1, 0, 'user', decode(repeat('a1', 32), 'hex'), 'users', 'id = 1', 1, \
         now(), 'tester', 'record-only; no legal conclusion asserted', \
         decode(repeat('00', 32), 'hex'), decode(repeat('00', 32), 'hex')) \
         RETURNING seq, prev_entry_hash",
    )
    .bind(ORG_A)
    .fetch_one(&mut *tx)
    .await
    .expect("console_rt must hold INSERT on erasure_ledger");
    tx.commit().await.unwrap();

    assert_eq!(
        seq, 1,
        "the trigger must read the head from the REAL ledger, not from a table the caller planted"
    );
    assert_eq!(
        prev,
        GENESIS.to_vec(),
        "the first real entry is genesis-rooted; a caller-planted predecessor must not stick"
    );
}
