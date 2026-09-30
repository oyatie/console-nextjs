//! Integrity engine: governance findings, detectors, and REST API.
//!
//! # Architecture
//!
//! Three logical layers in one crate (the scope is small enough to not warrant
//! four separate crates):
//!
//! * **`domain`** — pure types: `GovernanceFinding`, `FindingSeverity`,
//!   `FindingStatus`, the `Detector` trait, and the `PriceOutlierDetector`.
//!   No sqlx, no axum, no async.
//!
//! * **`store`** — Postgres adapter: reads and writes `governance_findings`
//!   via `with_org_conn` / `with_audit`. The `self_approval` finding is
//!   written by the financial adapter inside its own `with_audit` transaction;
//!   the price-outlier finding is written here OnWrite.
//!
//! * **`rest`** — Axum handlers: `GET /api/v1/integrity/findings` (list) and
//!   `POST /api/v1/integrity/findings/{id}/triage`. Both require
//!   `Feature::IntegrityFindingsRead` / `Feature::IntegrityFindingTriage`.
//!
//! # Framing
//!
//! Findings are "검토 필요" (review needed), NOT "사기" (fraud). The UI
//! presents them as items requiring human review, not accusations.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

pub mod domain;
pub mod rest;
pub mod store;

pub use rest::{INTEGRITY_ROUTE_PATHS, IntegrityRestState, router};
pub use store::PgIntegrityStore;
