//! Shared object-code issuance (BE-OBJ slice 2, item 1).
//!
//! Generalizes the work-order request counter into ONE issuer keyed by object
//! kind, so new domains stop hand-rolling counters. A code is
//! `<object_types.code_prefix><sequence>` where the sequence is a per-org,
//! per-kind monotonic BIGINT bumped atomically.
//!
//! The caller supplies the transaction (already inside its own `with_audit`
//! write), so issuance shares the caller's tenant GUC and commits atomically
//! with the object being created. The bump uses the same
//! `INSERT … ON CONFLICT DO UPDATE … RETURNING` row-lock pattern as
//! `work_order_request_counters`: monotonic and concurrency-safe (concurrent
//! issuers serialize on the counter row), gap-free NOT guaranteed (a
//! rolled-back tx burns its number).

use console_kernel_core::OrgId;
use sqlx::{Postgres, Transaction};

use crate::error::DbError;

/// Issue the next canonical code for `kind` under `org`, e.g. `AP-42`.
///
/// Runs on the passed transaction, which the caller must already have armed
/// with the tenant GUC (via `with_audit`/`with_org_conn`); RLS on
/// `object_code_counters` then scopes the counter to `org`.
///
/// # Errors
/// - [`DbError::CodeIssuance`] if `kind` is unknown or has no `code_prefix`
///   (an id/name-referenced kind such as `person`/`org_unit`).
/// - [`DbError::Sqlx`] on any database failure.
pub async fn issue_code(
    tx: &mut Transaction<'_, Postgres>,
    org: OrgId,
    kind: &str,
) -> Result<String, DbError> {
    let prefix: Option<String> =
        sqlx::query_scalar("SELECT code_prefix FROM object_types WHERE kind = $1")
            .bind(kind)
            .fetch_optional(tx.as_mut())
            .await?
            .ok_or_else(|| DbError::CodeIssuance(format!("unknown object kind {kind:?}")))?;
    let prefix = prefix.ok_or_else(|| {
        DbError::CodeIssuance(format!(
            "object kind {kind:?} has no code prefix (not issuable)"
        ))
    })?;

    let sequence: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO object_code_counters (org_id, kind, last_sequence)
        VALUES ($1, $2, 1)
        ON CONFLICT (org_id, kind) DO UPDATE
        SET last_sequence = object_code_counters.last_sequence + 1
        RETURNING last_sequence
        "#,
    )
    .bind(*org.as_uuid())
    .bind(kind)
    .fetch_one(tx.as_mut())
    .await?;

    Ok(format!("{prefix}{sequence}"))
}
