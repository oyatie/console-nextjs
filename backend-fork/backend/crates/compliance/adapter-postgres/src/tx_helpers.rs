//! Transaction-fragment helpers for the compliance Postgres adapter.
//!
//! These are pure `..._tx` fragments invoked from inside `with_audits`
//! closures in `lib.rs`; the auditable business write is the enclosing
//! `with_audits` call, which emits the `AuditEvent` in the same transaction.
//! They intentionally live outside `lib.rs` (the adapter's audit-coverage
//! handler surface) because they carry no audit event of their own — the
//! calling handler owns it.

use sqlx::{Postgres, Transaction};

use console_compliance_domain::{ObligationRegulationLink, ObligationRegulationRelationship};
use console_kernel_core::{Timestamp, UserId};

use crate::{PgComplianceError, obligation_regulation_link_from_row};

pub(crate) async fn next_compliance_code_tx(
    tx: &mut Transaction<'_, Postgres>,
    org_uuid: uuid::Uuid,
    prefix: &str,
) -> Result<String, PgComplianceError> {
    let allocated: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO compliance_code_counters (org_id, object_prefix, next_value, updated_at)
        VALUES ($1, $2, 2, now())
        ON CONFLICT (org_id, object_prefix) DO UPDATE
        SET next_value = compliance_code_counters.next_value + 1,
            updated_at = now()
        RETURNING next_value - 1
        "#,
    )
    .bind(org_uuid)
    .bind(prefix)
    .fetch_one(tx.as_mut())
    .await?;
    Ok(format!("{prefix}-{allocated:04}"))
}

// ponytail: builder-shaped signature, struct-ify deferred
#[allow(clippy::too_many_arguments)]
pub(crate) async fn insert_obligation_regulation_link_tx(
    tx: &mut Transaction<'_, Postgres>,
    org_uuid: uuid::Uuid,
    obligation_id: uuid::Uuid,
    regulation_impact_id: uuid::Uuid,
    relationship: ObligationRegulationRelationship,
    rationale: Option<&str>,
    actor: UserId,
    occurred_at: Timestamp,
) -> Result<ObligationRegulationLink, PgComplianceError> {
    let row = sqlx::query(
        r#"
        WITH inserted AS (
            INSERT INTO compliance_obligation_regulations (
                id, org_id, obligation_id, regulation_impact_id, relationship,
                rationale, created_by, created_at
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
            ON CONFLICT (org_id, obligation_id, regulation_impact_id, relationship) DO NOTHING
            RETURNING id, obligation_id, regulation_impact_id, relationship, rationale,
                      created_by, created_at
        )
        SELECT id, obligation_id, regulation_impact_id, relationship, rationale,
               created_by, created_at
        FROM inserted
        UNION ALL
        SELECT id, obligation_id, regulation_impact_id, relationship, rationale,
               created_by, created_at
        FROM compliance_obligation_regulations
        WHERE org_id = $2
          AND obligation_id = $3
          AND regulation_impact_id = $4
          AND relationship = $5
        LIMIT 1
        "#,
    )
    .bind(uuid::Uuid::new_v4())
    .bind(org_uuid)
    .bind(obligation_id)
    .bind(regulation_impact_id)
    .bind(relationship.as_db_str())
    .bind(rationale.map(str::trim))
    .bind(*actor.as_uuid())
    .bind(occurred_at)
    .fetch_one(tx.as_mut())
    .await?;
    obligation_regulation_link_from_row(&row)
}
