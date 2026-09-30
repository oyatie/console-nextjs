//! Append-only erasure ledger: a durable record of what personal data was
//! DELETED FROM THE LIVE CLUSTER, at what scope, when, by whom, and under what
//! authority.
//!
//! Deliberate wording, and it is a constraint rather than a style note: a row
//! recorded here was deleted from the live cluster and remains reconstructable
//! from the WAL archive, whose ObjectStore declares no retention policy. Nothing
//! in this crate — doc comment, variant name, error string or test name — may
//! call that destruction without naming the archive (ADR-0037 constraint 3;
//! that record adopts no option and this crate adopts none on its behalf).
//!
//! # What this does NOT solve
//!
//! 1. **Nothing outside PostgreSQL holds a witness yet, so the mechanism is
//!    INERT.** A point-in-time restore rolls the ledger back together with every
//!    in-cluster copy of its high-water mark, and [`classify`] then returns
//!    [`RestoreVerdict::Consistent`] against whatever witness it is handed.
//!    Detection is structurally impossible in that state, not merely unfired.
//!    This crate ships the three pieces — produce a witness ([`head`]), compare
//!    one ([`classify`]), replay after one ([`entries_since`]) — and no holder
//!    and no re-applier. [`append`] does emit the witness on the
//!    `console.erasure_ledger.witness` tracing target, which is the only copy
//!    that leaves the cluster without a `deploy/**` change; it is NOT a holder.
//!    Log retention is not under this crate's control, the line is not
//!    tamper-evident, nothing reads it back, and no test asserts it. Treat it as
//!    a breadcrumb for a human, not as the anchor.
//!
//!    With ONE exception, stated because "structurally impossible" is otherwise
//!    too strong: [`RestoreVerdict::Gapped`] rests on `count(*) = max(seq)`, an
//!    identity internal to the ledger, so a restore that lands BETWEEN recorded
//!    entries is detectable with nothing held outside the cluster at all. A
//!    restore that truncates the ledger from its head is not — that leaves the
//!    identity intact — and that is the case an external witness is still the
//!    only answer to.
//! 2. **It detects; it never prevents.** No in-database construct survives a
//!    point-in-time restore of its own cluster.
//! 3. **No signature.** A caller holding `INSERT` can record false facts at
//!    append time. The chain is tamper-evident against a rollback and against
//!    nothing else. Do not call it tamper-proof.
//! 4. **It records only.** It does not erase, authorise or re-apply. No
//!    retention period, no automatic deletion, no data-subject-request workflow.
//! 5. **The scope it hands back is DATA, not a statement.**
//!    [`ErasureFacts::erased_relation`] and [`ErasureFacts::erased_selector`]
//!    are unvalidated free text from whoever held `INSERT`. A re-applier that
//!    concatenates them into SQL makes every appender an author of statements
//!    it executes later — and no `UPDATE` can take a bad one back out.
//!
//! Every Korea control remains HOLD. This crate asserts no Korean legal
//! conclusion; `authority` is free text precisely so that enumerating legal
//! bases does not smuggle one in.

use console_kernel_core::OrgId;
use console_platform_db::{DbError, with_org_conn};
use sqlx::PgPool;
use time::OffsetDateTime;
use uuid::Uuid;

/// The replay read, columns in the order [`LedgerEntry`] names them.
const ENTRIES_SINCE_SQL: &str = "SELECT org_id, seq, subject_kind, subject_digest, \
     erased_relation, erased_selector, erased_row_count, effective_at, actor, authority, \
     prev_entry_hash, entry_hash FROM erasure_ledger \
     WHERE org_id = $1 AND seq > $2 ORDER BY seq";

/// `seq`, `prev_entry_hash` and `entry_hash` are placeholders. Migration 0207's
/// `BEFORE INSERT` trigger overwrites all three, which is what makes the chain
/// something a caller holding only `INSERT` cannot forge — so this crate carries
/// no crypto and no canonical encoding of its own.
const APPEND_SQL: &str = "INSERT INTO erasure_ledger (\
     org_id, seq, subject_kind, subject_digest, erased_relation, erased_selector, \
     erased_row_count, effective_at, actor, authority, prev_entry_hash, entry_hash) \
     VALUES ($1, 0, $2, erasure_ledger_subject_digest($1, $2, $3), $4, $5, $6, $7, $8, $9, \
     decode(repeat('00', 32), 'hex'), decode(repeat('00', 32), 'hex')) \
     RETURNING seq, entry_hash, encode(entry_hash, 'hex')";

/// How many times a losing appender re-reads the head before giving up. Two
/// concurrent appends collide on `(org_id, seq)` or `(org_id, prev_entry_hash)`
/// and the loser sees `23505`; a fresh transaction then reads the winner's head.
const APPEND_ATTEMPTS: u32 = 5;

type EntryRow = (
    Uuid,
    i64,
    String,
    Vec<u8>,
    String,
    String,
    i64,
    OffsetDateTime,
    String,
    String,
    Vec<u8>,
    Vec<u8>,
);

/// The fact set a single erasure entry records. Every field is required: an
/// entry that cannot say WHAT was erased is not evidence.
#[derive(Debug, Clone)]
pub struct ErasureFacts {
    /// What kind of subject the erasure was about (`"user"`, `"applicant"`, …).
    pub subject_kind: String,
    /// The subject's identifier. A `Uuid` rather than a string so a low-entropy
    /// natural key cannot be digested here; the stored reference is a digest.
    pub subject_id: Uuid,
    /// The relation rows were deleted from.
    pub erased_relation: String,
    /// The predicate that selected them, precise enough that a later reader can
    /// tell WHAT was erased rather than merely that something was.
    ///
    /// Free text written by whoever holds `INSERT`, and READ BACK AS DATA ONLY.
    /// It is not validated, not parsed and not bound to any relation's real
    /// columns. A re-applier that concatenates this and [`Self::erased_relation`]
    /// into SQL turns every appender into an author of statements it will run
    /// later, against a ledger no `UPDATE` can correct.
    pub erased_selector: String,
    /// How many rows the selector matched.
    pub erased_row_count: i64,
    /// When the erasure took effect.
    pub effective_at: OffsetDateTime,
    /// Who performed it.
    pub actor: String,
    /// The authority it was performed under. Free text by design.
    pub authority: String,
}

/// A high-water mark: the position and content hash of one org's ledger head at
/// the moment it was observed. The thing an external holder would record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LedgerWitness {
    pub org_id: Uuid,
    pub seq: i64,
    pub entry_hash: [u8; 32],
}

/// One recorded entry, as the re-applier reads it back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LedgerEntry {
    pub org_id: Uuid,
    pub seq: i64,
    pub subject_kind: String,
    pub subject_digest: [u8; 32],
    pub erased_relation: String,
    pub erased_selector: String,
    pub erased_row_count: i64,
    pub effective_at: OffsetDateTime,
    pub actor: String,
    pub authority: String,
    pub prev_entry_hash: [u8; 32],
    pub entry_hash: [u8; 32],
}

/// What comparing a held witness against the live ledger says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestoreVerdict {
    /// The witness is still on the live chain. Normal operation.
    Consistent { head_seq: i64 },
    /// The head is behind the witness: the ledger lost entries it had recorded.
    RolledBack { head_seq: i64, witness_seq: i64 },
    /// The witness's sequence exists but holds different content: the ledger was
    /// rolled back and then written forward again. A sequence-only comparison
    /// reads this as healthy, which is the failure this variant exists to name.
    Forked { witness_seq: i64 },
    /// The ledger holds fewer entries than its own head sequence, so entries it
    /// once recorded are gone — even though the witness itself survived and
    /// every check anchored at the witness agrees.
    ///
    /// Migration 0207's trigger assigns `seq` as `max(seq) + 1` starting at 1 and
    /// nothing can delete a row, so a live ledger always satisfies
    /// `count(*) = max(seq)`. That identity is what sees a loss ABOVE the
    /// witness, which no comparison anchored at one point can.
    Gapped { head_seq: i64, entry_count: i64 },
}

#[derive(Debug, thiserror::Error)]
pub enum ErasureLedgerError {
    #[error("erasure ledger database error: {0}")]
    Db(#[from] DbError),
    #[error("erasure ledger append lost the race for the next sequence")]
    Contention,
    #[error("erasure ledger seq {seq} carries a {len}-byte hash where 32 are required")]
    MalformedHash { seq: i64, len: usize },
}

/// Migration 0207 CHECKs `octet_length(...) = 32` on every hash column, so this
/// only fires against a ledger whose constraints have been removed — which is
/// worth an error rather than a truncation that reads like a hash.
fn hash32(bytes: &[u8], seq: i64) -> Result<[u8; 32], ErasureLedgerError> {
    <[u8; 32]>::try_from(bytes).map_err(|_| ErasureLedgerError::MalformedHash {
        seq,
        len: bytes.len(),
    })
}

fn is_unique_violation(error: &sqlx::Error) -> bool {
    error
        .as_database_error()
        .and_then(|db| db.code())
        .is_some_and(|code| code == "23505")
}

/// Append one entry to `org`'s ledger, returning the witness it produced.
///
/// The witness is the whole point: an external holder that records
/// `(org_id, seq, entry_hash)` is what makes [`classify`] able to say anything
/// at all after a restore. Nothing in this repository holds one yet.
pub async fn append(
    pool: &PgPool,
    org: OrgId,
    facts: &ErasureFacts,
) -> Result<LedgerWitness, ErasureLedgerError> {
    for _ in 0..APPEND_ATTEMPTS {
        match append_once(pool, org, facts).await {
            Err(ErasureLedgerError::Db(DbError::Sqlx(error))) if is_unique_violation(&error) => {}
            outcome => return outcome,
        }
    }
    Err(ErasureLedgerError::Contention)
}

async fn append_once(
    pool: &PgPool,
    org: OrgId,
    facts: &ErasureFacts,
) -> Result<LedgerWitness, ErasureLedgerError> {
    let org_id = *org.as_uuid();
    // Cloned, not borrowed: `with_org_conn`'s closure is `for<'tx>`, so anything
    // the future captures by reference is forced to `'static`.
    let facts = facts.clone();
    let (seq, entry_hash, entry_hash_hex) = with_org_conn(pool, org, move |tx| {
        Box::pin(async move {
            sqlx::query_as::<_, (i64, Vec<u8>, String)>(APPEND_SQL)
                .bind(org_id)
                .bind(facts.subject_kind)
                .bind(facts.subject_id)
                .bind(facts.erased_relation)
                .bind(facts.erased_selector)
                .bind(facts.erased_row_count)
                .bind(facts.effective_at)
                .bind(facts.actor)
                .bind(facts.authority)
                .fetch_one(tx.as_mut())
                .await
                .map_err(|error| ErasureLedgerError::Db(DbError::Sqlx(error)))
        })
    })
    .await?;

    let entry_hash = hash32(&entry_hash, seq)?;
    // The one copy of the witness that leaves the PITR domain without a
    // deploy/** change. It is NOT a holder: log retention is not under this
    // crate's control, the line is not tamper-evident, and nothing reads it
    // back. Naming it here so it is not mistaken for the external anchor the
    // crate docs say is missing.
    tracing::info!(
        target: "console.erasure_ledger.witness",
        org_id = %org_id,
        seq,
        entry_hash = %entry_hash_hex,
        "erasure ledger entry appended"
    );
    Ok(LedgerWitness {
        org_id,
        seq,
        entry_hash,
    })
}

/// The current head of `org`'s ledger, or `None` if it has never been written.
pub async fn head(pool: &PgPool, org: OrgId) -> Result<Option<LedgerWitness>, ErasureLedgerError> {
    let org_id = *org.as_uuid();
    let row = with_org_conn(pool, org, move |tx| {
        Box::pin(async move {
            sqlx::query_as::<_, (i64, Vec<u8>)>(
                "SELECT seq, entry_hash FROM erasure_ledger \
                 WHERE org_id = $1 ORDER BY seq DESC LIMIT 1",
            )
            .bind(org_id)
            .fetch_optional(tx.as_mut())
            .await
            .map_err(|error| ErasureLedgerError::Db(DbError::Sqlx(error)))
        })
    })
    .await?;

    match row {
        None => Ok(None),
        Some((seq, entry_hash)) => Ok(Some(LedgerWitness {
            org_id,
            seq,
            entry_hash: hash32(&entry_hash, seq)?,
        })),
    }
}

/// Every entry after `after_seq`, in sequence order. The read path a re-applier
/// replays once a restore has been detected: it carries the SCOPE — relation,
/// selector, row count — because "something was erased" cannot be re-applied.
pub async fn entries_since(
    pool: &PgPool,
    org: OrgId,
    after_seq: i64,
) -> Result<Vec<LedgerEntry>, ErasureLedgerError> {
    let org_id = *org.as_uuid();
    let rows = with_org_conn(pool, org, move |tx| {
        Box::pin(async move {
            sqlx::query_as::<_, EntryRow>(ENTRIES_SINCE_SQL)
                .bind(org_id)
                .bind(after_seq)
                .fetch_all(tx.as_mut())
                .await
                .map_err(|error| ErasureLedgerError::Db(DbError::Sqlx(error)))
        })
    })
    .await?;

    rows.into_iter()
        .map(|row| {
            let (
                org_id,
                seq,
                subject_kind,
                subject_digest,
                erased_relation,
                erased_selector,
                erased_row_count,
                effective_at,
                actor,
                authority,
                prev_entry_hash,
                entry_hash,
            ) = row;
            Ok(LedgerEntry {
                org_id,
                seq,
                subject_kind,
                subject_digest: hash32(&subject_digest, seq)?,
                erased_relation,
                erased_selector,
                erased_row_count,
                effective_at,
                actor,
                authority,
                prev_entry_hash: hash32(&prev_entry_hash, seq)?,
                entry_hash: hash32(&entry_hash, seq)?,
            })
        })
        .collect()
}

/// Compare a held witness against the live ledger.
///
/// One query, three facts: where the head is now, how many entries the ledger
/// actually holds, and what content the witnessed sequence holds now.
///
/// Comparing sequences alone is not enough — a restore followed by continued
/// operation drives the head back up to the witnessed sequence carrying
/// different entries, and that is the case [`RestoreVerdict::Forked`] names.
/// Comparing the witness alone is not enough either: a restore that loses
/// entries ABOVE the witness leaves the head past it and the witnessed row
/// untouched, so every check anchored at the witness agrees while recorded
/// entries are gone. The count catches that, because `seq` is assigned
/// contiguously from 1 and no row can be removed — see
/// [`RestoreVerdict::Gapped`].
pub async fn classify(
    pool: &PgPool,
    org: OrgId,
    witness: &LedgerWitness,
) -> Result<RestoreVerdict, ErasureLedgerError> {
    let org_id = *org.as_uuid();
    let witness_seq = witness.seq;
    let (head_seq, entry_count, witnessed_hash) = with_org_conn(pool, org, move |tx| {
        Box::pin(async move {
            sqlx::query_as::<_, (i64, i64, Option<Vec<u8>>)>(
                "SELECT COALESCE((SELECT max(seq) FROM erasure_ledger WHERE org_id = $1), 0), \
                 (SELECT count(*) FROM erasure_ledger WHERE org_id = $1), \
                 (SELECT entry_hash FROM erasure_ledger WHERE org_id = $1 AND seq = $2)",
            )
            .bind(org_id)
            .bind(witness_seq)
            .fetch_one(tx.as_mut())
            .await
            .map_err(|error| ErasureLedgerError::Db(DbError::Sqlx(error)))
        })
    })
    .await?;

    if head_seq < witness.seq {
        return Ok(RestoreVerdict::RolledBack {
            head_seq,
            witness_seq: witness.seq,
        });
    }
    match witnessed_hash {
        // A mismatched or absent witnessed entry is the sharper statement — it
        // invalidates the witness itself — so it is reported ahead of the count.
        Some(hash) if hash32(&hash, witness.seq)? == witness.entry_hash => {
            if entry_count == head_seq {
                Ok(RestoreVerdict::Consistent { head_seq })
            } else {
                Ok(RestoreVerdict::Gapped {
                    head_seq,
                    entry_count,
                })
            }
        }
        _ => Ok(RestoreVerdict::Forked {
            witness_seq: witness.seq,
        }),
    }
}
