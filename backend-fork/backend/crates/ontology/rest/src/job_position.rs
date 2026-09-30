//! JobPosition identity readback on the preserved ontology action namespace.
//!
//! L5-JOB does **not** invent `/api/v1/job-positions` or widen `OrgEntitySummary`
//! (that DTO is Company/OrgUnit — L5-ORG / console-7sx). Canonical position IDs
//! round-trip through `organization.create_job_position` /
//! `organization.revise_job_position` receipt results under
//! `/api/v1/ontology/actions/{action_key}/execute`.
//!
//! Recruiting postings and `employees.position` free text are never projected
//! here: they are not canonical positions (canonical-domain JobPosition contract).

use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

/// Compatible readback of a canonical JobPosition from an action receipt
/// `result` object. Field names match the port's stored receipt JSON
/// (snake_case), so a client that already consumes action execute responses
/// needs no parallel DTO family.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobPositionIdentity {
    pub job_position_id: Uuid,
    pub org_unit_id: Uuid,
    pub version: i64,
}

/// Parse the authority IDs out of a JobPosition command receipt `result`.
///
/// Fail-closed: missing or non-UUID fields are refused. A result that only
/// carries a free-text title (the recruiting/employee shape) cannot decode.
pub fn identity_from_receipt_result(
    result: &Value,
) -> Result<JobPositionIdentity, JobPositionProjectionError> {
    let job_position_id = uuid_field(result, "job_position_id")?;
    let org_unit_id = uuid_field(result, "org_unit_id")?;
    let version = result
        .get("version")
        .and_then(Value::as_i64)
        .ok_or(JobPositionProjectionError::MissingField("version"))?;
    Ok(JobPositionIdentity {
        job_position_id,
        org_unit_id,
        version,
    })
}

fn uuid_field(result: &Value, key: &'static str) -> Result<Uuid, JobPositionProjectionError> {
    let raw = result
        .get(key)
        .and_then(Value::as_str)
        .ok_or(JobPositionProjectionError::MissingField(key))?;
    Uuid::parse_str(raw).map_err(|_| JobPositionProjectionError::InvalidUuid(key))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobPositionProjectionError {
    MissingField(&'static str),
    InvalidUuid(&'static str),
}

impl std::fmt::Display for JobPositionProjectionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingField(key) => write!(f, "job position receipt missing `{key}`"),
            Self::InvalidUuid(key) => write!(f, "job position receipt `{key}` is not a UUID"),
        }
    }
}

impl std::error::Error for JobPositionProjectionError {}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn receipt_result_round_trips_canonical_ids() {
        let job_position_id = Uuid::from_u128(0x6f10_0000_0000_0000_0000_0000_0000_00aa);
        let org_unit_id = Uuid::from_u128(0x6f10_0000_0000_0000_0000_0000_0000_00bb);
        let result = json!({
            "job_position_id": job_position_id.to_string(),
            "org_unit_id": org_unit_id.to_string(),
            "version": 2,
            "target": "organization.revise_job_position",
            "title": "백엔드 엔지니어",
            "phone": "+82-10-0000-0000",
            "salary": 90_000_000,
        });
        let identity = identity_from_receipt_result(&result).unwrap();
        assert_eq!(
            identity,
            JobPositionIdentity {
                job_position_id,
                org_unit_id,
                version: 2,
            }
        );
        let wire = serde_json::to_value(&identity).unwrap();
        assert!(wire.get("phone").is_none());
        assert!(wire.get("salary").is_none());
        assert!(wire.get("title").is_none());

        let missing_org_unit = json!({
            "job_position_id": job_position_id.to_string(),
            "version": 2,
        });
        assert_eq!(
            identity_from_receipt_result(&missing_org_unit),
            Err(JobPositionProjectionError::MissingField("org_unit_id"))
        );
        let non_uuid_org_unit = json!({
            "job_position_id": job_position_id.to_string(),
            "org_unit_id": "not-a-uuid",
            "version": 2,
        });
        assert_eq!(
            identity_from_receipt_result(&non_uuid_org_unit),
            Err(JobPositionProjectionError::InvalidUuid("org_unit_id"))
        );
    }

    #[test]
    fn free_text_title_alone_is_not_a_job_position_identity() {
        let forged = json!({ "title": "백엔드 엔지니어", "version": 1 });
        assert_eq!(
            identity_from_receipt_result(&forged),
            Err(JobPositionProjectionError::MissingField("job_position_id"))
        );
        let posting = json!({
            "title": "백엔드 엔지니어",
            "department": "engineering",
        });
        assert!(identity_from_receipt_result(&posting).is_err());
    }

    #[test]
    fn org_entity_summary_shape_is_refused() {
        // OrgEntitySummary is Company/OrgUnit reference (7sx) — not JobPosition.
        let org_entity = json!({
            "orgId": "6f100000-0000-0000-0000-000000000001",
            "slug": "acme",
            "name": "Acme",
            "status": "ACTIVE",
        });
        assert!(identity_from_receipt_result(&org_entity).is_err());
    }
}
