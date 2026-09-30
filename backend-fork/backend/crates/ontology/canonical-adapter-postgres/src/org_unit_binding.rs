//! L5-ORG binding seam — region/branch → `org_unit_source_bindings`.
//!
//! This module is the single source file for production DML that binds a legacy
//! region/branch UUID onto a canonical OrgUnit. It lives under the OrgUnit
//! owner crate (`console-ontology-canonical-adapter-postgres`) so
//! `console-gate-writer-ownership` attributes the writes correctly.
//!
//! Sibling adapters (org-change) compile the same file via `#[path]` rather than
//! a Cargo dependency — adapter→adapter edges are forbidden by
//! `console-gate-layer-boundary`. Do not move these SQL strings into an adapter
//! that does not own `ObjectKey::OrgUnit`.

use console_kernel_core::{OrgId, UserId};
use console_ontology_canonical_domain::DispatchTarget;
use sha2::{Digest, Sha256};
use uuid::Uuid;

/// Production `source_kind` for a legacy `regions` row. UUID `source_id` only.
pub const SOURCE_KIND_REGION: &str = "region";
/// Production `source_kind` for a legacy `branches` row. UUID `source_id` only.
pub const SOURCE_KIND_BRANCH: &str = "branch";

/// Outcome of resolving a legacy source onto at most one canonical OrgUnit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceBindingResolution {
    /// Exactly one binding row for `(org, kind, id)`.
    Resolved(Uuid),
    /// No binding yet — callers may `ensure_*` only for unambiguous UUID sources.
    Unbound,
    /// More than one unit matches a text probe. MUST NOT create authority.
    Ambiguous { match_count: usize },
}

/// Free-text (or non-UUID) legacy labels never mint OrgUnit authority.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error(
    "ambiguous org-structure text must not create OrgUnit authority (source_kind={source_kind}, text={text})"
)]
pub struct AmbiguousTextAuthority {
    pub source_kind: String,
    pub text: String,
}

/// Errors from the in-transaction binding seam.
#[derive(Debug, thiserror::Error)]
pub enum OrgUnitBindingError {
    #[error("preflight blocked the command: {0:?}")]
    Blocked(Vec<String>),
    #[error(transparent)]
    Database(#[from] sqlx::Error),
}

/// Accept only an unambiguous legacy id (UUID). Names/labels are refused.
pub fn unambiguous_legacy_source_id(
    source_kind: &str,
    raw: &str,
) -> Result<String, AmbiguousTextAuthority> {
    let trimmed = raw.trim();
    if Uuid::parse_str(trimmed).is_ok() {
        return Ok(trimmed.to_owned());
    }
    Err(AmbiguousTextAuthority {
        source_kind: source_kind.to_owned(),
        text: trimmed.to_owned(),
    })
}

/// Look up `org_unit_source_bindings` for one legacy source. PK guarantees at
/// most one row per `(org_id, source_kind, source_id)`; `Ambiguous` is unused
/// here and reserved for text probes below.
pub async fn resolve_source_binding<'e, E>(
    executor: E,
    org_id: OrgId,
    source_kind: &str,
    source_id: &str,
) -> Result<SourceBindingResolution, sqlx::Error>
where
    E: sqlx::Executor<'e, Database = sqlx::Postgres>,
{
    let org_unit_id: Option<Uuid> = sqlx::query_scalar(
        "SELECT org_unit_id FROM org_unit_source_bindings \
         WHERE org_id = $1 AND source_kind = $2 AND source_id = $3",
    )
    .bind(*org_id.as_uuid())
    .bind(source_kind)
    .bind(source_id)
    .fetch_optional(executor)
    .await?;
    Ok(match org_unit_id {
        Some(id) => SourceBindingResolution::Resolved(id),
        None => SourceBindingResolution::Unbound,
    })
}

/// Count OrgUnits whose latest revision `attributes.name` equals `name`.
///
/// Used to refuse name-based authority: `match_count != 1` → do not bind.
pub async fn count_org_units_named<'e, E>(
    executor: E,
    org_id: OrgId,
    name: &str,
) -> Result<SourceBindingResolution, sqlx::Error>
where
    E: sqlx::Executor<'e, Database = sqlx::Postgres>,
{
    let rows: Vec<Uuid> = sqlx::query_scalar(
        "SELECT u.id FROM org_units u \
         INNER JOIN LATERAL ( \
             SELECT attributes FROM org_unit_revisions r \
             WHERE r.org_id = u.org_id AND r.org_unit_id = u.id \
             ORDER BY r.version DESC LIMIT 1 \
         ) latest ON true \
         WHERE u.org_id = $1 AND latest.attributes->>'name' = $2 \
         ORDER BY u.id",
    )
    .bind(*org_id.as_uuid())
    .bind(name)
    .fetch_all(executor)
    .await?;
    Ok(match rows.len() {
        0 => SourceBindingResolution::Unbound,
        1 => SourceBindingResolution::Resolved(rows[0]),
        n => SourceBindingResolution::Ambiguous { match_count: n },
    })
}

/// Bind a legacy region or branch UUID to a canonical OrgUnit inside an open
/// transaction. Replays an existing binding; never accepts free-text source ids.
///
/// SQL strings stay in this owner-crate source file so writer-ownership remains
/// total when a sibling adapter compiles the file via `#[path]`.
pub async fn ensure_unambiguous_legacy_binding_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    org_id: OrgId,
    actor_id: UserId,
    source_kind: &str,
    legacy_id: Uuid,
    attributes: serde_json::Value,
    command_id: Uuid,
) -> Result<Uuid, OrgUnitBindingError> {
    if source_kind != SOURCE_KIND_REGION && source_kind != SOURCE_KIND_BRANCH {
        return Err(OrgUnitBindingError::Blocked(vec![format!(
            "source_kind must be '{SOURCE_KIND_REGION}' or '{SOURCE_KIND_BRANCH}', got {source_kind}"
        )]));
    }
    let source_id = legacy_id.to_string();
    match resolve_source_binding(&mut **tx, org_id, source_kind, &source_id).await? {
        SourceBindingResolution::Resolved(existing) => return Ok(existing),
        SourceBindingResolution::Unbound => {}
        SourceBindingResolution::Ambiguous { match_count } => {
            return Err(OrgUnitBindingError::Blocked(vec![format!(
                "refusing ambiguous source binding (match_count={match_count})"
            )]));
        }
    }
    if !attributes.is_object() {
        return Err(OrgUnitBindingError::Blocked(vec![
            "attributes must be a JSON object".to_owned(),
        ]));
    }

    let org = *org_id.as_uuid();
    let actor = *actor_id.as_uuid();
    let digest = {
        let mut hasher = Sha256::new();
        hasher.update(org.as_bytes());
        hasher.update(command_id.as_bytes());
        hasher.update(actor.as_bytes());
        hasher.update(
            DispatchTarget::OrganizationCreateOrgUnit
                .as_str()
                .as_bytes(),
        );
        hasher.update(source_kind.as_bytes());
        hasher.update(source_id.as_bytes());
        hasher.update(canonical_json(&attributes).to_string().as_bytes());
        let digest: [u8; 32] = hasher.finalize().into();
        digest
    };

    let org_unit_id: Uuid =
        sqlx::query_scalar("INSERT INTO org_units (org_id) VALUES ($1) RETURNING id")
            .bind(org)
            .fetch_one(&mut **tx)
            .await?;

    let result = serde_json::json!({
        "org_unit_id": org_unit_id.to_string(),
        "version": 1,
        "target": DispatchTarget::OrganizationCreateOrgUnit.as_str(),
        "source_kind": source_kind,
        "source_id": source_id,
    });

    sqlx::query(
        "INSERT INTO org_unit_revisions \
         (org_id, org_unit_id, version, command_id, actor_id, payload_digest, attributes, receipt) \
         VALUES ($1, $2, 1, $3, $4, $5, $6, $7)",
    )
    .bind(org)
    .bind(org_unit_id)
    .bind(command_id)
    .bind(actor)
    .bind(digest.as_slice())
    .bind(&attributes)
    .bind(&result)
    .execute(&mut **tx)
    .await?;

    sqlx::query(
        "INSERT INTO org_unit_source_bindings \
         (org_id, source_kind, source_id, org_unit_id, actor_id, payload_digest) \
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(org)
    .bind(source_kind)
    .bind(&source_id)
    .bind(org_unit_id)
    .bind(actor)
    .bind(digest.as_slice())
    .execute(&mut **tx)
    .await?;

    Ok(org_unit_id)
}

fn canonical_json(value: &serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Array(values) => {
            serde_json::Value::Array(values.iter().map(canonical_json).collect())
        }
        serde_json::Value::Object(values) => {
            let mut entries: Vec<(String, serde_json::Value)> = values
                .iter()
                .map(|(key, value)| (key.clone(), canonical_json(value)))
                .collect();
            entries.sort_by(|left, right| left.0.cmp(&right.0));
            serde_json::Value::Object(entries.into_iter().collect())
        }
        primitive => primitive.clone(),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod binding_seam_tests {
    use super::*;

    #[test]
    fn uuid_source_ids_are_unambiguous() {
        let id = Uuid::new_v4().to_string();
        assert_eq!(
            unambiguous_legacy_source_id(SOURCE_KIND_BRANCH, &id).unwrap(),
            id
        );
    }

    #[test]
    fn free_text_labels_never_mint_authority() {
        let err = unambiguous_legacy_source_id(SOURCE_KIND_REGION, "영업본부").unwrap_err();
        assert_eq!(err.source_kind, SOURCE_KIND_REGION);
        assert_eq!(err.text, "영업본부");
        let err = unambiguous_legacy_source_id(SOURCE_KIND_BRANCH, " team-a ").unwrap_err();
        assert_eq!(err.text, "team-a");
    }
}
