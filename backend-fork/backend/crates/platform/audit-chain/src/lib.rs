//! `console-platform-audit-chain` — L20 tamper-evident audit chain (design charter).
//!
//! A per-org, append-only, cryptographically-sealed hash chain over
//! `audit_events`. A background worker seals batches of audit rows in a fixed
//! total order; [`verify_org_chain`] recomputes and compares, localizing any
//! divergence to a seal.
//!
//! # What it defends against
//! A party with direct DB write access (the `console_app` owner, a leaked
//! superuser, an edited backup restore) who can bypass the append-only
//! triggers/grants on `audit_events`: a **row edit / delete / insert / reorder**
//! recomputes a divergent `batch_hash` → `seal_hash`. The signature over
//! `seal_hash` means re-sealing a tampered suffix requires the signing key.
//! Mid-chain seal deletion/splice is caught (`MissingSeq` / `BrokenContinuity`).
//! **Residual:** a coordinated **tail rollback** (delete the newest seal(s)
//! *and* the rows they cover, leaving a self-consistent signed prefix) still
//! verifies `ok=true` under prefix-only verification — detecting that class
//! requires an out-of-band head/seal anchor (future work; not claimed here).
//!
//! # What it does NOT defend against (custody boundary)
//! A party who ALSO holds the seal signing key can rewrite history and re-sign.
//! The private key therefore lives behind a context-scoped external signer and
//! key-custody port (never in-crate); [`SealSigner`] is asymmetric so the DB
//! owner does not hold it. The owner-controlled self-host implementation lands
//! first; OCI Vault is only the OCI adapter, while other clouds use their native
//! KMS/HSM adapters. [`InMemoryEd25519Signer`] is for dev/test only.
//!
//! # Tamper evidence: the trust boundary is the signer
//! Real tamper evidence against the DB-writer threat actor requires that
//! `verify` establish the seal's public key from a source the writer CANNOT
//! forge — never from the seal's own `key_ref`. [`ExternalSealSigner`] closes
//! that boundary: the private key lives in an external key-custody service
//! reached through a [`SealSignTransport`], and `verify` checks signatures ONLY
//! against LOCALLY pinned trust anchors. An attacker who rewrites a row,
//! recomputes the hashes, generates a fresh keypair, re-signs, and overwrites
//! `signature` + `key_ref` now produces a seal under an UNPINNED key → a
//! `BadSignature` verdict. Signing fails **closed**: an unreachable custody
//! service yields [`SealSignError::Unavailable`] and no seal is written (no
//! in-crate fallback key). Verification is deliberately custody-INDEPENDENT — it
//! needs only the pinned public keys — so attestation still works during a
//! signer outage (a DoS of the signer cannot blind tamper detection).
//! [`InMemoryEd25519Signer`] is dev/test only: its `verify` reconstructs the
//! public key from the attacker-writable `key_ref`, so it provides no evidence
//! against the DB-writer actor and must never seal production chains.
//!
//! # Coverage gap: NULL-org audit rows
//! Platform-tier audit rows (`audit_events.org_id IS NULL` — retention/roster
//! jobs) are invisible under org RLS, so they are never sealed and carry no
//! tamper evidence. Seal and verify agree (no false alarm), but this is a known
//! coverage gap a future platform-org chain can close.
//!
//! # Ordering (charter §2)
//! The chain orders each org's events by `(created_at ASC, id ASC)`.
//! `created_at` is DB-authoritative (`now()` at insert), immutable (append-only
//! trigger), and total when tie-broken by the immutable PK `id`. `occurred_at`
//! is app-supplied and hashed as *content*, never as the sort key.
//!
//! # Query discipline
//! Every statement is a runtime-checked `sqlx::query*` (never a `query!` macro):
//! this crate ships no `.sqlx` offline cache and CI runs `SQLX_OFFLINE=true`.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

use std::panic::AssertUnwindSafe;
use std::sync::Arc;
use std::time::Duration as StdDuration;

use console_kernel_core::OrgId;
use console_platform_db::{DbError, with_org_conn};
use futures::FutureExt;
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Postgres, Transaction};
use time::{Duration, OffsetDateTime, UtcOffset};
use tokio::sync::watch;
use uuid::Uuid;

mod external;
pub use external::{ExternalSealSigner, SealSignTransport};

// ===========================================================================
// Constants
// ===========================================================================

/// Domain-separation tags. Prefixing every hash input with a distinct tag makes
/// a row-hash preimage unusable as a batch- or seal-hash preimage.
const DOMAIN_ROW: &[u8] = b"console.audit-chain.row.v1";
const DOMAIN_BATCH: &[u8] = b"console.audit-chain.batch.v1";
const DOMAIN_SEAL: &[u8] = b"console.audit-chain.seal.v1";

/// Genesis predecessor hash: the first seal for an org links to 32 zero bytes.
const GENESIS_PREV: [u8; 32] = [0u8; 32];

/// Seconds between worker seal ticks (charter §2.5).
pub const SEAL_TICK_SECS: u64 = 30;

/// Canonical timestamp format: RFC3339-shaped, forced UTC, FIXED 9-digit
/// sub-second precision + literal `Z`, so seal-time and verify-time bytes are
/// byte-identical regardless of trailing-zero normalization (charter §2.3).
const TS_FORMAT: &[time::format_description::BorrowedFormatItem<'_>] = time::macros::format_description!(
    "[year]-[month]-[day]T[hour]:[minute]:[second].[subsecond digits:9]Z"
);

/// Tunables for one seal pass. `now` is injected separately so tests drive the
/// watermark with a synthetic clock instead of real sleeps.
#[derive(Debug, Clone, Copy)]
pub struct SealConfig {
    /// A row is sealed only once `created_at <= now - seal_lag`; the query sees
    /// only rows committed in its snapshot (commit order ≠ `now()` order). The
    /// topology reconciliation pins PostgreSQL 17+ `transaction_timeout` at
    /// 45s on every serving login that can write an audit row, and migration
    /// `0167_serving_role_transaction_timeouts` asserts that catalog state. The
    /// complementary 30s statement and idle-in-transaction defaults do not by
    /// themselves bound total transaction duration. Thus a normal serving
    /// transaction cannot outlive the default 60s `seal_lag`.
    /// That lag reduces the risk of a late commit below an advanced cursor; it
    /// establishes no global gap-free invariant. Migration-owner, offline, and
    /// operator writers remain outside this reconciliation/startup correctness
    /// backstop, so gap-free sealing still requires quiescence/coordination or
    /// a future xmin/snapshot watermark. These USERSET values are not a security
    /// boundary against a compromised database login.
    // ponytail: time-lag watermark. Serving-role defaults are reconciled
    // operationally and asserted by 0167 (45s < 60s), but gap-free sealing
    // still needs writer quiescence or a future xmin/snapshot watermark
    // (pg_snapshot_xmin(pg_current_snapshot())).
    pub seal_lag: Duration,
    /// Max rows sealed in one pass (bounds one transaction). A backlog drains
    /// over subsequent ticks.
    pub batch_max: i64,
}

impl Default for SealConfig {
    fn default() -> Self {
        Self {
            seal_lag: Duration::seconds(60),
            batch_max: 500,
        }
    }
}

// ===========================================================================
// Errors
// ===========================================================================

/// A signer failure. Distinct from a *bad signature*, which [`SealSigner::verify`]
/// reports as `Ok(false)`.
#[derive(Debug, thiserror::Error)]
pub enum SealSignError {
    #[error("ed25519 key generation failed")]
    KeyGen,
    #[error("ed25519 key rejected")]
    KeyRejected,
    #[error("invalid key_ref: {0}")]
    KeyRef(String),
    /// The external key-custody signer could not be reached or refused to sign.
    /// Sealing MUST fail closed on this: a chain is never sealed by any in-crate
    /// fallback key, because a self-signed seal the DB writer can reproduce is
    /// not tamper evidence. Distinct from a *bad signature* (`verify → Ok(false)`)
    /// and from a malformed `key_ref` (`KeyRef`).
    #[error("external seal signer unavailable: {0}")]
    Unavailable(String),
}

/// Crate error surface. `with_org_conn` requires `E: From<DbError>`.
#[derive(Debug, thiserror::Error)]
pub enum AuditChainError {
    #[error(transparent)]
    Db(#[from] DbError),
    #[error(transparent)]
    Signer(#[from] SealSignError),
    #[error("canonicalization error: {0}")]
    Canonical(String),
    #[error("corrupt seal storage: {0}")]
    CorruptSeal(String),
}

impl From<sqlx::Error> for AuditChainError {
    fn from(err: sqlx::Error) -> Self {
        Self::Db(DbError::Sqlx(err))
    }
}

// ===========================================================================
// Signer (charter §4)
// ===========================================================================

/// Signs and verifies seal hashes. Asymmetric by design (Ed25519): production
/// holds the private key behind the context-selected external key-custody port
/// and `verify` needs only the public key, so the DB owner cannot forge a fresh
/// chain. Self-host custody is owner-controlled; cloud adapters use native
/// KMS/HSM services (including OCI Vault only in the OCI context).
pub trait SealSigner: Send + Sync {
    /// Opaque identifier of the key that produced (and should verify) a
    /// signature. Persisted in `audit_chain_seals.key_ref` so a chain signed
    /// under a rotated key still verifies against the right public key.
    fn key_ref(&self) -> &str;

    /// Sign `message` (the `seal_hash`).
    fn sign(&self, message: &[u8]) -> Result<Vec<u8>, SealSignError>;

    /// Verify `signature` over `message` against the key named by `key_ref` (the
    /// seal's *stored* `key_ref`, not necessarily this signer's current key).
    /// `Ok(false)` = valid key, bad signature; `Err` = the key_ref is malformed.
    fn verify(
        &self,
        message: &[u8],
        signature: &[u8],
        key_ref: &str,
    ) -> Result<bool, SealSignError>;
}

/// In-process Ed25519 signer for dev/test ONLY. Generates a fresh keypair at
/// construction and embeds the public key in `key_ref` as `test:ed25519:<hex
/// pk>`, so `verify` reconstructs the public key from the seal's stored
/// `key_ref` alone. That makes it defenceless against the DB-writer threat
/// actor — a party who edits a sealed row can re-sign under a fresh keypair and
/// rewrite `key_ref`, and verification accepts it (exercised by the
/// `resign_attack_defeats_key_ref_derived_dev_signer` integration test).
/// Production uses [`ExternalSealSigner`] with locally pinned trust anchors.
pub struct InMemoryEd25519Signer {
    key_pair: ring::signature::Ed25519KeyPair,
    key_ref: String,
}

const KEY_REF_PREFIX: &str = "test:ed25519:";

impl InMemoryEd25519Signer {
    /// Generate a fresh keypair from the system CSPRNG.
    pub fn generate() -> Result<Self, SealSignError> {
        let rng = ring::rand::SystemRandom::new();
        let pkcs8 = ring::signature::Ed25519KeyPair::generate_pkcs8(&rng)
            .map_err(|_| SealSignError::KeyGen)?;
        let key_pair = ring::signature::Ed25519KeyPair::from_pkcs8(pkcs8.as_ref())
            .map_err(|_| SealSignError::KeyRejected)?;
        let pk_hex = {
            use ring::signature::KeyPair;
            hex::encode(key_pair.public_key().as_ref())
        };
        Ok(Self {
            key_pair,
            key_ref: format!("{KEY_REF_PREFIX}{pk_hex}"),
        })
    }

    /// Raw Ed25519 public key bytes of this keypair. Lets a dev/test deployment
    /// pin this ephemeral key as a trust anchor for [`ExternalSealSigner`]
    /// (custody-backed verification) without going through the attacker-writable
    /// `key_ref` path.
    #[must_use]
    pub fn public_key(&self) -> Vec<u8> {
        use ring::signature::KeyPair;
        self.key_pair.public_key().as_ref().to_vec()
    }

    /// Extract the raw Ed25519 public key from a `test:ed25519:<hex>` key_ref.
    fn public_key_from_ref(key_ref: &str) -> Result<Vec<u8>, SealSignError> {
        let hex_pk = key_ref
            .strip_prefix(KEY_REF_PREFIX)
            .ok_or_else(|| SealSignError::KeyRef(format!("unexpected key_ref: {key_ref}")))?;
        hex::decode(hex_pk)
            .map_err(|_| SealSignError::KeyRef(format!("non-hex key_ref: {key_ref}")))
    }
}

impl SealSigner for InMemoryEd25519Signer {
    fn key_ref(&self) -> &str {
        &self.key_ref
    }

    fn sign(&self, message: &[u8]) -> Result<Vec<u8>, SealSignError> {
        Ok(self.key_pair.sign(message).as_ref().to_vec())
    }

    fn verify(
        &self,
        message: &[u8],
        signature: &[u8],
        key_ref: &str,
    ) -> Result<bool, SealSignError> {
        let public_key = Self::public_key_from_ref(key_ref)?;
        let peer = ring::signature::UnparsedPublicKey::new(&ring::signature::ED25519, public_key);
        Ok(peer.verify(message, signature).is_ok())
    }
}

// ===========================================================================
// Canonicalization (charter §2.3 — the crux)
// ===========================================================================

/// One `audit_events` row, in the exact column set the chain hashes.
#[derive(Debug, Clone, sqlx::FromRow)]
struct AuditRow {
    id: Uuid,
    actor: Option<Uuid>,
    action: String,
    target_type: String,
    target_id: String,
    branch_id: Option<Uuid>,
    org_id: Option<Uuid>,
    before_snap: Option<serde_json::Value>,
    after_snap: Option<serde_json::Value>,
    trace_id: String,
    span_id: String,
    occurred_at: OffsetDateTime,
    created_at: OffsetDateTime,
}

/// Length-prefix: `u32_be(len) ‖ bytes`. Concatenating a FIXED sequence of
/// length-prefixed fields is injective — every field is self-delimiting, so no
/// two distinct field tuples can produce the same byte string (this is what
/// defeats the `H(a‖b)` boundary-ambiguity class). A NULL field encodes as
/// `u32_be(0)`, distinct from any present value.
fn lp(buf: &mut Vec<u8>, bytes: &[u8]) {
    // A field longer than u32::MAX cannot occur (Postgres row/field limits are
    // far below 4 GiB); the cast is saturating-safe in practice, and a truncated
    // length would only ever cause a verify MISMATCH, never a false match.
    let len = u32::try_from(bytes.len()).unwrap_or(u32::MAX);
    buf.extend_from_slice(&len.to_be_bytes());
    buf.extend_from_slice(bytes);
}

/// Raw bytes of an optional UUID: 16 bytes when present, empty (→ LP length 0)
/// when NULL.
fn opt_uuid_bytes(value: &Option<Uuid>) -> &[u8] {
    match value {
        Some(uuid) => uuid.as_bytes(),
        None => &[],
    }
}

/// Deterministic JSON bytes for a JSONB snapshot, with object keys sorted in
/// CRATE-OWNED code. This must NOT rely on serde_json's Map ordering: the
/// `preserve_order`/`indexmap` feature IS enabled transitively in this workspace
/// (cedar-policy-core + sqlx-postgres pull it), so `Value::Object` is an
/// insertion-ordered `IndexMap`, not sorted. Re-serializing with plain
/// `serde_json::to_vec` would emit keys in JSONB-storage order — seal==verify
/// only by accident, and a dep bump flipping the feature would mass-fail every
/// existing seal. Sorting here makes the encoding independent of the Map
/// backing; scalars reuse serde_json's own deterministic serialization (only
/// object key ORDER is controlled here). NULL column → empty.
fn canonical_json(value: &Option<serde_json::Value>) -> Result<Vec<u8>, AuditChainError> {
    match value {
        None => Ok(Vec::new()),
        Some(json) => {
            let mut out = Vec::new();
            write_canonical_json(json, &mut out)?;
            Ok(out)
        }
    }
}

fn write_canonical_json(
    value: &serde_json::Value,
    out: &mut Vec<u8>,
) -> Result<(), AuditChainError> {
    match value {
        serde_json::Value::Object(map) => {
            out.push(b'{');
            let mut entries: Vec<_> = map.iter().collect();
            entries.sort_unstable_by_key(|(k, _)| *k);
            for (i, (key, child)) in entries.into_iter().enumerate() {
                if i > 0 {
                    out.push(b',');
                }
                // Key as a JSON string (serde's escaping), then ':' then value.
                serde_json::to_writer(&mut *out, key)
                    .map_err(|err| AuditChainError::Canonical(format!("json key: {err}")))?;
                out.push(b':');
                write_canonical_json(child, out)?;
            }
            out.push(b'}');
        }
        serde_json::Value::Array(items) => {
            out.push(b'[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(b',');
                }
                write_canonical_json(item, out)?;
            }
            out.push(b']');
        }
        // Null / Bool / Number / String: serde's serialization is deterministic
        // and free of any key-order concern.
        scalar => serde_json::to_writer(&mut *out, scalar)
            .map_err(|err| AuditChainError::Canonical(format!("json scalar: {err}")))?,
    }
    Ok(())
}

/// Canonical timestamp string (forced UTC, fixed 9-digit sub-second).
fn format_ts(ts: OffsetDateTime) -> Result<String, AuditChainError> {
    ts.to_offset(UtcOffset::UTC)
        .format(&TS_FORMAT)
        .map_err(|err| AuditChainError::Canonical(format!("timestamp format: {err}")))
}

/// `row_hash := SHA-256(DOMAIN_ROW ‖ LP(id) ‖ LP(actor) ‖ … ‖ LP(created_at))`.
fn row_hash(row: &AuditRow) -> Result<[u8; 32], AuditChainError> {
    let mut buf = Vec::new();
    lp(&mut buf, row.id.as_bytes());
    lp(&mut buf, opt_uuid_bytes(&row.actor));
    lp(&mut buf, row.action.as_bytes());
    lp(&mut buf, row.target_type.as_bytes());
    lp(&mut buf, row.target_id.as_bytes());
    lp(&mut buf, opt_uuid_bytes(&row.branch_id));
    lp(&mut buf, opt_uuid_bytes(&row.org_id));
    lp(&mut buf, &canonical_json(&row.before_snap)?);
    lp(&mut buf, &canonical_json(&row.after_snap)?);
    lp(&mut buf, row.trace_id.as_bytes());
    lp(&mut buf, row.span_id.as_bytes());
    lp(&mut buf, format_ts(row.occurred_at)?.as_bytes());
    lp(&mut buf, format_ts(row.created_at)?.as_bytes());
    Ok(sha256(DOMAIN_ROW, &buf))
}

/// `batch_hash := SHA-256(DOMAIN_BATCH ‖ row_hash_1 ‖ … ‖ row_hash_n)`.
fn batch_hash(rows: &[AuditRow]) -> Result<[u8; 32], AuditChainError> {
    let mut hasher = Sha256::new();
    hasher.update(DOMAIN_BATCH);
    for row in rows {
        hasher.update(row_hash(row)?);
    }
    Ok(finalize32(hasher))
}

/// `seal_hash := SHA-256(DOMAIN_SEAL ‖ org ‖ u64_be(seq) ‖ from_ca ‖ from_id ‖
/// to_ca ‖ to_id ‖ u64_be(row_count) ‖ batch_hash ‖ prev_seal_hash)`.
///
/// `org`, `seq`, `row_count`, and the two ids/hashes are all fixed-length; only
/// the two timestamps are variable, so they are LP-prefixed to keep the whole
/// preimage injective.
#[allow(clippy::too_many_arguments)]
fn seal_hash(
    org_id: Uuid,
    seq: u64,
    from_created_at: OffsetDateTime,
    from_event_id: Uuid,
    to_created_at: OffsetDateTime,
    to_event_id: Uuid,
    row_count: u64,
    batch: &[u8; 32],
    prev_seal_hash: &[u8; 32],
) -> Result<[u8; 32], AuditChainError> {
    let mut buf = Vec::new();
    buf.extend_from_slice(org_id.as_bytes());
    buf.extend_from_slice(&seq.to_be_bytes());
    lp(&mut buf, format_ts(from_created_at)?.as_bytes());
    buf.extend_from_slice(from_event_id.as_bytes());
    lp(&mut buf, format_ts(to_created_at)?.as_bytes());
    buf.extend_from_slice(to_event_id.as_bytes());
    buf.extend_from_slice(&row_count.to_be_bytes());
    buf.extend_from_slice(batch);
    buf.extend_from_slice(prev_seal_hash);
    Ok(sha256(DOMAIN_SEAL, &buf))
}

fn sha256(domain: &[u8], body: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(domain);
    hasher.update(body);
    finalize32(hasher)
}

/// Narrow a `Sha256` digest to `[u8; 32]` without depending on the exact
/// `finalize()` output type (`GenericArray` vs `hybrid_array::Array` across
/// digest versions) — both deref to `[u8]`.
fn finalize32(hasher: Sha256) -> [u8; 32] {
    let digest = hasher.finalize();
    let mut out = [0u8; 32];
    out.copy_from_slice(&digest);
    out
}

/// A `Vec<u8>` BYTEA column narrowed to a 32-byte hash, or a corruption error.
fn hash32(bytes: Vec<u8>, field: &str) -> Result<[u8; 32], AuditChainError> {
    <[u8; 32]>::try_from(bytes.as_slice())
        .map_err(|_| AuditChainError::CorruptSeal(format!("{field} is not 32 bytes")))
}

// ===========================================================================
// Seal worker (charter §5.1 / §5.2)
// ===========================================================================

/// The outcome of a single successful seal INSERT.
#[derive(Debug, Clone)]
pub struct SealSummary {
    pub org_id: Uuid,
    pub seq: i64,
    pub row_count: i64,
    pub seal_hash: [u8; 32],
    pub prev_seal_hash: [u8; 32],
}

/// The head seal's resume cursor: `(to_created_at, to_event_id)`.
type Cursor = (OffsetDateTime, Uuid);

#[derive(Debug, Clone, sqlx::FromRow)]
struct HeadRow {
    seq: i64,
    to_created_at: OffsetDateTime,
    to_event_id: Uuid,
    seal_hash: Vec<u8>,
}

const SELECT_HEAD: &str = "SELECT seq, to_created_at, to_event_id, seal_hash \
     FROM audit_chain_seals ORDER BY seq DESC LIMIT 1";

const SELECT_BATCH_COLUMNS: &str = "id, actor, action, target_type, target_id, branch_id, \
     org_id, before_snap, after_snap, trace_id, span_id, occurred_at, created_at";

/// Seal at most `config.batch_max` not-yet-sealed rows for one org, in a single
/// tenant-scoped transaction as `console_rt`. Returns `Ok(None)` when there is
/// nothing old enough to seal.
///
/// Idempotency + crash-safety (charter §5.2): progress *is* the head seal row;
/// compute-then-insert is one transaction. Die before insert ⇒ cursor unmoved ⇒
/// next call recomputes the identical batch. Die after commit ⇒ head advanced ⇒
/// next call continues. A racing second run that passes the advisory lock and
/// tries `seq+1` hits the `PRIMARY KEY(org_id, seq)` / `UNIQUE(org_id,
/// prev_seal_hash)` constraint and aborts — no duplicate, no fork.
pub async fn seal_org_once(
    pool: &PgPool,
    org: OrgId,
    signer: &Arc<dyn SealSigner>,
    now: OffsetDateTime,
    config: &SealConfig,
) -> Result<Option<SealSummary>, AuditChainError> {
    let org_id = *org.as_uuid();
    let watermark = now - config.seal_lag;
    let batch_max = config.batch_max;
    let signer = Arc::clone(signer);

    with_org_conn(pool, org, move |tx| {
        Box::pin(async move {
            // Per-org advisory lock: two worker replicas never seal one org
            // concurrently (belt-and-suspenders over the PK/UNIQUE constraints).
            // Transaction-scoped ⇒ auto-released on COMMIT/ROLLBACK.
            sqlx::query(
                "SELECT pg_advisory_xact_lock(hashtext('console.audit-chain'), hashtext($1))",
            )
            .bind(org_id.to_string())
            .execute(tx.as_mut())
            .await?;

            // Head seal → resume cursor + predecessor hash + sequence.
            let head: Option<HeadRow> = sqlx::query_as(SELECT_HEAD)
                .fetch_optional(tx.as_mut())
                .await?;
            let (prev_seq, cursor, prev_seal_hash) = match head {
                Some(head) => {
                    let prev = hash32(head.seal_hash, "seal_hash")?;
                    (head.seq, Some((head.to_created_at, head.to_event_id)), prev)
                }
                None => (0, None, GENESIS_PREV),
            };

            // Next batch, RLS-scoped to this org, old enough to have committed.
            let rows = fetch_batch(tx, cursor, watermark, batch_max).await?;
            let Some((first, last)) = rows.first().zip(rows.last()) else {
                return Ok(None);
            };

            let seq_u64 = u64::try_from(prev_seq + 1)
                .map_err(|_| AuditChainError::CorruptSeal("seq overflow".to_owned()))?;
            let row_count = rows.len();
            let batch = batch_hash(&rows)?;
            let seal = seal_hash(
                org_id,
                seq_u64,
                first.created_at,
                first.id,
                last.created_at,
                last.id,
                row_count as u64,
                &batch,
                &prev_seal_hash,
            )?;
            let signature = signer.sign(&seal)?;

            let seq = prev_seq + 1;
            let row_count_i64 = i64::try_from(row_count)
                .map_err(|_| AuditChainError::CorruptSeal("row_count overflow".to_owned()))?;
            sqlx::query(
                "INSERT INTO audit_chain_seals \
                 (org_id, seq, from_event_id, from_created_at, to_event_id, to_created_at, \
                  row_count, batch_hash, prev_seal_hash, seal_hash, signature, key_ref) \
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)",
            )
            .bind(org_id)
            .bind(seq)
            .bind(first.id)
            .bind(first.created_at)
            .bind(last.id)
            .bind(last.created_at)
            .bind(row_count_i64)
            .bind(&batch[..])
            .bind(&prev_seal_hash[..])
            .bind(&seal[..])
            .bind(&signature[..])
            .bind(signer.key_ref())
            .execute(tx.as_mut())
            .await?;

            Ok(Some(SealSummary {
                org_id,
                seq,
                row_count: row_count_i64,
                seal_hash: seal,
                prev_seal_hash,
            }))
        })
    })
    .await
}

/// Select the next `(created_at, id)`-ordered batch after `cursor`, no newer
/// than `watermark`. Genesis (no cursor) has no lower bound.
async fn fetch_batch(
    tx: &mut Transaction<'_, Postgres>,
    cursor: Option<Cursor>,
    watermark: OffsetDateTime,
    batch_max: i64,
) -> Result<Vec<AuditRow>, AuditChainError> {
    let rows = match cursor {
        None => {
            let sql = format!(
                "SELECT {SELECT_BATCH_COLUMNS} FROM audit_events \
                 WHERE created_at <= $1 ORDER BY created_at ASC, id ASC LIMIT $2"
            );
            sqlx::query_as::<_, AuditRow>(sqlx::AssertSqlSafe(sql))
                .bind(watermark)
                .bind(batch_max)
                .fetch_all(tx.as_mut())
                .await?
        }
        Some((cursor_ca, cursor_id)) => {
            let sql = format!(
                "SELECT {SELECT_BATCH_COLUMNS} FROM audit_events \
                 WHERE created_at <= $1 \
                   AND (created_at > $2 OR (created_at = $2 AND id > $3)) \
                 ORDER BY created_at ASC, id ASC LIMIT $4"
            );
            sqlx::query_as::<_, AuditRow>(sqlx::AssertSqlSafe(sql))
                .bind(watermark)
                .bind(cursor_ca)
                .bind(cursor_id)
                .bind(batch_max)
                .fetch_all(tx.as_mut())
                .await?
        }
    };
    Ok(rows)
}

/// A handle that stops the seal loop on explicit shutdown (mirrors
/// `workflow_drain::WorkflowDrainHandle`).
#[derive(Debug)]
pub struct AuditChainHandle {
    shutdown_tx: watch::Sender<bool>,
}

impl AuditChainHandle {
    /// Signal the seal loop to stop.
    pub fn shutdown(&self) {
        let _ = self.shutdown_tx.send(true);
    }
}

/// Spawn the seal worker on the app `console_rt` pool. Ticks every
/// [`SEAL_TICK_SECS`]; per tick, enumerates tenants and seals one batch per org.
/// A backlog larger than `batch_max` drains over subsequent ticks (the watermark
/// bounds staleness to `seal_lag + tick`). The loop runs until the returned
/// handle is shut down.
#[must_use]
pub fn spawn(pool: PgPool, signer: Arc<dyn SealSigner>) -> AuditChainHandle {
    let (shutdown_tx, mut shutdown_rx) = watch::channel(false);
    let config = SealConfig::default();

    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(StdDuration::from_secs(SEAL_TICK_SECS));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        tracing::info!(
            tick_secs = SEAL_TICK_SECS,
            "audit-chain seal worker started"
        );

        loop {
            tokio::select! {
                changed = shutdown_rx.changed() => {
                    if changed.is_err() || *shutdown_rx.borrow() {
                        tracing::info!("audit-chain seal worker stopping");
                        break;
                    }
                }
                _ = ticker.tick() => {
                    run_tick(&pool, &signer, &config).await;
                }
            }
        }
    });

    AuditChainHandle { shutdown_tx }
}

/// One seal tick: enumerate tenants, then seal one batch per org under its
/// armed GUC. `platform_list_organizations()` is the RLS-safe cross-tenant
/// discovery `workflow_drain` also uses.
async fn run_tick(pool: &PgPool, signer: &Arc<dyn SealSigner>, config: &SealConfig) {
    let orgs: Vec<Uuid> = match sqlx::query_scalar("SELECT id FROM platform_list_organizations()")
        // rls-arming: ok platform_list_organizations() is the SECURITY DEFINER id-only tenant discovery (same read as workflow_drain); every per-org seal below runs under with_org_conn
        .fetch_all(pool)
        .await
    {
        Ok(orgs) => orgs,
        Err(err) => {
            tracing::warn!(error = %err, "audit-chain: enumerate tenants failed");
            return;
        }
    };

    let now = OffsetDateTime::now_utc();
    for org_uuid in orgs {
        let org = OrgId::from_uuid(org_uuid);
        // Panic-isolate the per-org seal (mirrors the cedar shadow-lane
        // precedent, identity/rest run_role_manage_cedar_shadow). `catch_unwind`
        // turns any panic — signer, sqlx, canonicalizer — into an `Err` instead
        // of unwinding out of the tick and killing the whole seal loop, so one
        // bad org cannot starve every other tenant's chain. `AssertUnwindSafe`
        // is sound: seal_org_once holds no cross-await locks and its only
        // mutation is a seal INSERT that rolls back on a panic, so a caught panic
        // leaves no observable broken state. It runs on the SAME task; per-org
        // isolation via `with_org_conn` arms `app.current_org` on its own
        // connection, so there is no task-local to preserve.
        let outcome = AssertUnwindSafe(seal_org_once(pool, org, signer, now, config))
            .catch_unwind()
            .await;
        match outcome {
            Ok(Ok(None)) => {}
            Ok(Ok(Some(summary))) => tracing::info!(
                org = %org_uuid,
                seq = summary.seq,
                row_count = summary.row_count,
                "audit-chain: sealed a batch"
            ),
            Ok(Err(err)) => tracing::warn!(
                org = %org_uuid,
                error = %err,
                "audit-chain: seal pass failed"
            ),
            Err(_panic) => tracing::error!(
                org = %org_uuid,
                "audit-chain: seal pass PANICKED (isolated; other tenants continue)"
            ),
        }
    }
}

// ===========================================================================
// Verify routine (charter §5.3)
// ===========================================================================

/// The TAMPER classification of a chain. Distinct from the behind-schedule
/// freshness signal (`ChainReport::unsealed_tail`), which is NOT tamper.
///
/// `Serialize` (snake_case, matching the app's `AppRole` precedent) so the
/// PR-2 attestation REST handler returns this type directly — no parallel
/// app-level DTO duplicating the crate's own verdict shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ChainReportKind {
    /// Every seal recomputes, chains, verifies, and covers its range with no gap.
    Ok,
    /// A seal's stored `seal_hash` does not equal the hash recomputed from its
    /// own stored scalar fields — a seal row was tampered.
    SealHashMismatch,
    /// A seal's stored `batch_hash` does not equal the hash recomputed from the
    /// `audit_events` in its range — a row was edited, deleted, inserted, or
    /// reordered.
    BatchHashMismatch,
    /// `prev_seal_hash` does not chain to the previous seal's `seal_hash`.
    BrokenContinuity,
    /// The signature over the stored `seal_hash` does not verify under `key_ref`.
    BadSignature,
    /// The `seq` column is not contiguous from 1 — a seal was deleted or spliced.
    MissingSeq,
    /// A committed `audit_events` row sits in a hole the sealed ranges do not
    /// cover — before the first seal, or strictly between two consecutive seals.
    /// This is what turns a codex-class backdated-insert / commit-order gap into
    /// a DETECTABLE finding (the seal_lag watermark bounds it at seal time; this
    /// check catches anything that still slipped in below the head).
    CoverageGap,
    /// A stored seal column is structurally corrupt — a hash/`bytea` that is not
    /// 32 bytes (truncated/overwritten by an attacker). Reported as tamper, not
    /// raised as a DB/infra `Err`.
    CorruptSeal,
}

/// The verdict for one org's chain.
///
/// `ok` reflects TAMPER integrity ONLY (`kind` is the classification). The
/// separate `unsealed_tail` freshness flag — old rows past the head that the
/// worker has not sealed yet — must NOT force `ok = false`: a live tenant always
/// carries a rolling unsealed window (up to `seal_lag + tick`), so conflating
/// behind-schedule with tamper would false-alarm every healthy chain. Tamper
/// (act now) and behind-schedule (a freshness/ops signal) are kept distinct.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ChainReport {
    pub org_id: Uuid,
    /// No tamper detected. Independent of `unsealed_tail`.
    pub ok: bool,
    /// The first offending seal's `seq`, when the failure localizes to a seal.
    pub first_bad_seq: Option<i64>,
    pub kind: ChainReportKind,
    /// Freshness signal: committed rows older than the grace-margined watermark
    /// exist beyond the head seal's cursor (worker fell behind / was stopped).
    /// A healthy live chain leaves this `false`; it never sets `ok = false`.
    pub unsealed_tail: bool,
}

impl ChainReport {
    fn healthy(org_id: Uuid, unsealed_tail: bool) -> Self {
        Self {
            org_id,
            ok: true,
            first_bad_seq: None,
            kind: ChainReportKind::Ok,
            unsealed_tail,
        }
    }

    fn tampered(org_id: Uuid, seq: Option<i64>, kind: ChainReportKind) -> Self {
        Self {
            org_id,
            ok: false,
            first_bad_seq: seq,
            kind,
            // Integrity already failed; freshness is moot.
            unsealed_tail: false,
        }
    }
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct SealRow {
    seq: i64,
    from_event_id: Uuid,
    from_created_at: OffsetDateTime,
    to_event_id: Uuid,
    to_created_at: OffsetDateTime,
    row_count: i64,
    batch_hash: Vec<u8>,
    prev_seal_hash: Vec<u8>,
    seal_hash: Vec<u8>,
    signature: Vec<u8>,
    key_ref: String,
}

const SELECT_SEALS: &str = "SELECT seq, from_event_id, from_created_at, to_event_id, \
     to_created_at, row_count, batch_hash, prev_seal_hash, seal_hash, signature, key_ref \
     FROM audit_chain_seals ORDER BY seq ASC";

/// Recompute-and-compare a single org's chain. Read-only; runnable as a REST
/// attestation handler (PR-2), a cron integrity job, or a test. Returns a
/// verdict even on tamper — including anything an attacker can do to seal
/// STORAGE: a structurally-corrupt stored hash column → `CorruptSeal`, an
/// unparseable stored `key_ref` → `BadSignature`. `Err` is reserved strictly for
/// GENUINE DB/infra failures (the `sqlx` queries, `fetch_range`, `rows_in_gap`).
///
/// Re-homed / deleted-tenant note (charter §1): a re-homed row (owner-only,
/// separately audited PIPA reference release) drops out of its tenant chain
/// under RLS and would surface as `BatchHashMismatch`. That is expected for a
/// *deleted* tenant and out of the threat model; verify is meant for LIVE
/// tenants.
pub async fn verify_org_chain(
    pool: &PgPool,
    org: OrgId,
    signer: &Arc<dyn SealSigner>,
    now: OffsetDateTime,
    config: &SealConfig,
) -> Result<ChainReport, AuditChainError> {
    let org_id = *org.as_uuid();
    let watermark = now - config.seal_lag;
    let signer = Arc::clone(signer);

    with_org_conn(pool, org, move |tx| {
        Box::pin(async move {
            let seals: Vec<SealRow> = sqlx::query_as(SELECT_SEALS).fetch_all(tx.as_mut()).await?;

            let mut prev_seal_hash = GENESIS_PREV;
            let mut prev_to: Option<Cursor> = None;
            for (index, seal) in seals.iter().enumerate() {
                let expected_seq = i64::try_from(index + 1).unwrap_or(i64::MAX);

                // (a) contiguity from 1.
                if seal.seq != expected_seq {
                    return Ok(ChainReport::tampered(
                        org_id,
                        Some(expected_seq),
                        ChainReportKind::MissingSeq,
                    ));
                }

                // Structurally corrupt stored hash columns (wrong length) are a
                // TAMPER verdict, not a DB/infra Err — verify's contract is to
                // return a verdict for anything an attacker can do to storage.
                let (stored_prev, stored_batch, stored_seal) = match (
                    hash32(seal.prev_seal_hash.clone(), "prev_seal_hash"),
                    hash32(seal.batch_hash.clone(), "batch_hash"),
                    hash32(seal.seal_hash.clone(), "seal_hash"),
                ) {
                    (Ok(prev), Ok(batch), Ok(seal_h)) => (prev, batch, seal_h),
                    _ => {
                        return Ok(ChainReport::tampered(
                            org_id,
                            Some(seal.seq),
                            ChainReportKind::CorruptSeal,
                        ));
                    }
                };

                // (b) continuity: this seal must link to the previous seal_hash.
                if stored_prev != prev_seal_hash {
                    return Ok(ChainReport::tampered(
                        org_id,
                        Some(seal.seq),
                        ChainReportKind::BrokenContinuity,
                    ));
                }

                // (b2) coverage: no committed audit_events row may sit strictly
                // between the previous seal's `to_` (or start-of-time at genesis)
                // and this seal's `from_`. A row bracketed by two seals was never
                // legitimately sealed, so it can only be a backdated / commit-order
                // -late insert → a DETECTABLE CoverageGap (closes the codex-class
                // silent skip: the seal_lag watermark bounds it at seal time; this
                // proves nothing slipped in below the head afterward).
                let seal_from: Cursor = (seal.from_created_at, seal.from_event_id);
                if rows_in_gap(tx, prev_to, seal_from).await? {
                    return Ok(ChainReport::tampered(
                        org_id,
                        Some(seal.seq),
                        ChainReportKind::CoverageGap,
                    ));
                }

                // (c) signature over the stored seal_hash under the stored key_ref.
                // A malformed/garbage *stored* key_ref (unparseable) can't
                // verify a signature = BadSignature verdict, NOT a propagated
                // Err. Preserve genuine signer failures as infra errors.
                match signer.verify(&stored_seal, &seal.signature, &seal.key_ref) {
                    Ok(true) => {}
                    Ok(false) | Err(SealSignError::KeyRef(_)) => {
                        return Ok(ChainReport::tampered(
                            org_id,
                            Some(seal.seq),
                            ChainReportKind::BadSignature,
                        ));
                    }
                    Err(err) => return Err(AuditChainError::Signer(err)),
                }

                // (d) internal consistency: the seal_hash must equal the hash of
                // the seal's own stored scalar fields (catches a tampered scalar
                // — row_count, a bound id/ts — that left seal_hash stale).
                let seq_u64 = u64::try_from(seal.seq)
                    .map_err(|_| AuditChainError::CorruptSeal("negative seq".to_owned()))?;
                let count_u64 = u64::try_from(seal.row_count)
                    .map_err(|_| AuditChainError::CorruptSeal("negative row_count".to_owned()))?;
                let recomputed_seal = seal_hash(
                    org_id,
                    seq_u64,
                    seal.from_created_at,
                    seal.from_event_id,
                    seal.to_created_at,
                    seal.to_event_id,
                    count_u64,
                    &stored_batch,
                    &stored_prev,
                )?;
                if recomputed_seal != stored_seal {
                    return Ok(ChainReport::tampered(
                        org_id,
                        Some(seal.seq),
                        ChainReportKind::SealHashMismatch,
                    ));
                }

                // (e) batch integrity: re-select the audit_events in this seal's
                // range and recompute batch_hash (catches edit/delete/insert).
                let rows = fetch_range(
                    tx,
                    (seal.from_created_at, seal.from_event_id),
                    (seal.to_created_at, seal.to_event_id),
                )
                .await?;
                if batch_hash(&rows)? != stored_batch {
                    return Ok(ChainReport::tampered(
                        org_id,
                        Some(seal.seq),
                        ChainReportKind::BatchHashMismatch,
                    ));
                }

                prev_seal_hash = stored_seal;
                prev_to = Some((seal.to_created_at, seal.to_event_id));
            }

            // (f) unsealed-tail is a FRESHNESS signal, not tamper: a live tenant
            // always carries a rolling window of committed-but-unsealed rows (up
            // to seal_lag + tick). Report it as a flag; it never forces ok=false.
            let unsealed_tail = unsealed_tail_exists(tx, prev_to, watermark).await?;
            Ok(ChainReport::healthy(org_id, unsealed_tail))
        })
    })
    .await
}

/// Select the `audit_events` in `[from, to]` inclusive, in canonical order.
async fn fetch_range(
    tx: &mut Transaction<'_, Postgres>,
    from: Cursor,
    to: Cursor,
) -> Result<Vec<AuditRow>, AuditChainError> {
    let sql = format!(
        "SELECT {SELECT_BATCH_COLUMNS} FROM audit_events \
         WHERE (created_at > $1 OR (created_at = $1 AND id >= $2)) \
           AND (created_at < $3 OR (created_at = $3 AND id <= $4)) \
         ORDER BY created_at ASC, id ASC"
    );
    let rows = sqlx::query_as::<_, AuditRow>(sqlx::AssertSqlSafe(sql))
        .bind(from.0)
        .bind(from.1)
        .bind(to.0)
        .bind(to.1)
        .fetch_all(tx.as_mut())
        .await?;
    Ok(rows)
}

/// Is there any watermark-old `audit_events` row strictly after `head_cursor`?
async fn unsealed_tail_exists(
    tx: &mut Transaction<'_, Postgres>,
    head_cursor: Option<Cursor>,
    watermark: OffsetDateTime,
) -> Result<bool, AuditChainError> {
    let count: i64 = match head_cursor {
        None => {
            sqlx::query_scalar("SELECT count(*) FROM audit_events WHERE created_at <= $1")
                .bind(watermark)
                .fetch_one(tx.as_mut())
                .await?
        }
        Some((cursor_ca, cursor_id)) => {
            sqlx::query_scalar(
                "SELECT count(*) FROM audit_events \
                 WHERE created_at <= $1 \
                   AND (created_at > $2 OR (created_at = $2 AND id > $3))",
            )
            .bind(watermark)
            .bind(cursor_ca)
            .bind(cursor_id)
            .fetch_one(tx.as_mut())
            .await?
        }
    };
    Ok(count > 0)
}

/// Is there any committed `audit_events` row strictly inside the open interval
/// `(lo, hi)` by `(created_at, id)` order? At genesis (`lo = None`) the interval
/// is `(-inf, hi)`. Used to PROVE sealed ranges leave no coverage gap: a row
/// bracketed by two seals — or before the first seal — was never legitimately
/// sealed, so its presence is a detectable `CoverageGap`.
async fn rows_in_gap(
    tx: &mut Transaction<'_, Postgres>,
    lo: Option<Cursor>,
    hi: Cursor,
) -> Result<bool, AuditChainError> {
    let count: i64 = match lo {
        None => {
            sqlx::query_scalar(
                "SELECT count(*) FROM audit_events \
                 WHERE created_at < $1 OR (created_at = $1 AND id < $2)",
            )
            .bind(hi.0)
            .bind(hi.1)
            .fetch_one(tx.as_mut())
            .await?
        }
        Some((lo_ca, lo_id)) => {
            sqlx::query_scalar(
                "SELECT count(*) FROM audit_events \
                 WHERE (created_at > $1 OR (created_at = $1 AND id > $2)) \
                   AND (created_at < $3 OR (created_at = $3 AND id < $4))",
            )
            .bind(lo_ca)
            .bind(lo_id)
            .bind(hi.0)
            .bind(hi.1)
            .fetch_one(tx.as_mut())
            .await?
        }
    };
    Ok(count > 0)
}

// ===========================================================================
// Pure unit tests (canonicalization crux — no DB)
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn base_row() -> AuditRow {
        AuditRow {
            id: Uuid::from_u128(0x1111),
            actor: None,
            action: "a.b".to_owned(),
            target_type: "t".to_owned(),
            target_id: "x".to_owned(),
            branch_id: None,
            org_id: Some(Uuid::from_u128(0xa1)),
            before_snap: None,
            after_snap: None,
            trace_id: "0".repeat(32),
            span_id: "0".repeat(16),
            occurred_at: OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap(),
            created_at: OffsetDateTime::from_unix_timestamp(1_700_000_001).unwrap(),
        }
    }

    /// Length-prefixing is injective across field boundaries: moving a character
    /// from one field to the next must change the hash (defeats `H(a‖b)`
    /// ambiguity). Without LP, "a.b"+"cd" and "a.bc"+"d" would collide.
    #[test]
    fn lp_encoding_is_injective_across_field_boundaries() {
        let mut left = base_row();
        left.action = "a.bc".to_owned();
        left.target_type = "d".to_owned();

        let mut right = base_row();
        right.action = "a.b".to_owned();
        right.target_type = "cd".to_owned();

        assert_ne!(
            row_hash(&left).unwrap(),
            row_hash(&right).unwrap(),
            "field-boundary shift must change the row hash"
        );
    }

    /// A NULL snapshot (LP length 0) is distinct from a present JSON `null`.
    #[test]
    fn null_snapshot_differs_from_json_null() {
        let mut null_row = base_row();
        null_row.before_snap = None;

        let mut present_row = base_row();
        present_row.before_snap = Some(serde_json::Value::Null);

        assert_ne!(
            row_hash(&null_row).unwrap(),
            row_hash(&present_row).unwrap(),
            "absent snapshot must not collide with a stored JSON null"
        );
    }

    /// Canonical JSON is key-order independent (sorted-key re-serialization).
    #[test]
    fn canonical_json_is_key_order_independent() {
        let a = canonical_json(&Some(serde_json::json!({"a": 1, "b": 2}))).unwrap();
        let b = canonical_json(&Some(serde_json::json!({"b": 2, "a": 1}))).unwrap();
        assert_eq!(a, b, "object key order must not affect canonical bytes");
    }

    /// Timestamps format to fixed 9-digit UTC, stably.
    #[test]
    fn timestamp_format_is_fixed_width_utc() {
        let ts = OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap();
        let formatted = format_ts(ts).unwrap();
        assert!(
            formatted.ends_with('Z'),
            "must be UTC-suffixed: {formatted}"
        );
        assert_eq!(
            formatted, "2023-11-14T22:13:20.000000000Z",
            "fixed 9-digit sub-second precision"
        );
    }

    /// The seal hash commits to the predecessor: same batch, different prev ⇒
    /// different seal (so tampering seal k breaks every seal ≥ k).
    #[test]
    fn seal_hash_commits_to_prev() {
        let org = Uuid::from_u128(0xa1);
        let id = Uuid::from_u128(0x1);
        let ts = OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap();
        let batch = [7u8; 32];
        let a = seal_hash(org, 1, ts, id, ts, id, 1, &batch, &[0u8; 32]).unwrap();
        let b = seal_hash(org, 1, ts, id, ts, id, 1, &batch, &[1u8; 32]).unwrap();
        assert_ne!(a, b, "seal hash must depend on prev_seal_hash");
    }
}
