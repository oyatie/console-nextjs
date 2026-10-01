//! `console-platform-db` — Postgres schema migrations and the `with_audit`
//! transactional helper.
//!
//! # Layering
//! This crate sits in the `platform` layer and is allowed to depend on
//! `console-kernel-core` (pure types) and `sqlx`. Domain and application crates
//! depend on this crate for the `with_audit` building block.
//!
//! # Append-only invariant
//! `audit_events` is structurally append-only: migration 0003 REVOKEs
//! UPDATE/DELETE from PUBLIC and installs BEFORE triggers that raise an
//! exception on any such attempt, even from privileged roles.
//!
//! # Offline query cache
//! Compile-time `query!` macros require either a live DATABASE_URL or a
//! committed `.sqlx/` offline cache. After schema changes run:
//!
//! ```sh
//! DATABASE_URL=postgres://localhost/console_dev \
//!     cargo sqlx prepare --workspace
//! ```
//!
//! then commit the regenerated `.sqlx/` directory. CI must set
//! `SQLX_OFFLINE=true`; missing cache entries fail the build explicitly.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

pub mod audit_tx;
pub mod code_issuance;
pub mod durability;
pub mod error;
pub mod governance_finding;
pub mod lifecycle;
pub mod period_lock;
pub mod versioning;

pub use audit_tx::{
    SubjectAuthzFreshness, insert_audit_event, read_subject_authz_freshness, with_audit,
    with_audited_snapshot, with_audits, with_org_conn, with_org_rollback, with_org_snapshot,
};
pub use code_issuance::issue_code;
pub use error::DbError;
pub use governance_finding::{OpenFinding, upsert_open_finding_tx};
pub use period_lock::{
    PeriodLockDomain, assert_period_open, assert_period_open_range, lock_period_lock_key,
};
pub use versioning::{ObjectVersionRecord, ObjectVersions};
