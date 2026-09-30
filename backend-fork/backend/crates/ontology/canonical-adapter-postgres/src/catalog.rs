//! Port-level catalog for canonical Company, OrgUnit, and JobPosition.
//!
//! Foundry's object/property/action split is the reason these keys exist as a
//! closed set rather than an open JSON bag: a regulation or ops change that
//! adds a field is an edit here plus the tests that name it, not a new table.
//!
//! This is deliberately NOT `ont_object_types` / the digest-allowlisted builtin
//! catalog. That installer occupies every key in the manifest on every tenant
//! that runs `seed_governed_config_object_types`, and the instance-backed
//! `company_conformance` fixtures already publish `company` / `org_unit` /
//! `job_position` under those stable keys. Putting the canonical objects in
//! the builtin catalog would collide. 0215 also refuses a `parent_id` column
//! on `org_units`; kind and parent live on the revision bag, not as columns.
//!
//! Writes still go through the owning ports. Required-ness is enforced in
//! PURE preflight so a blocked command never spends an approval.

use serde_json::Value;

/// `Company` title property. Stored on `company_revisions.attributes`, never
/// copied from provisioning-owned `organizations.name`.
pub const COMPANY_LEGAL_NAME: &str = "legal_name";

/// `OrgUnit` title property. Stored on `org_unit_revisions.attributes`.
pub const ORG_UNIT_NAME: &str = "name";

/// Closed OrgUnit kind. Stored on `org_unit_revisions.attributes`.
pub const ORG_UNIT_KIND: &str = "kind";

/// `JobPosition` title property. Stored on `job_position_revisions.attributes`.
/// Recruiting `role_title` / `employees.position` are not this field.
pub const JOB_POSITION_TITLE: &str = "title";

/// Require `attributes` to be a JSON object carrying a non-empty string `key`.
///
/// Missing, non-string, empty, and whitespace-only values are distinct
/// blockers so a caller learns the catalog rule rather than a JSON-shape
/// failure. A non-object short-circuits: there is no property to inspect.
#[must_use]
pub fn require_text_property(attributes: &Value, key: &str) -> Vec<String> {
    let mut blockers = Vec::new();
    let Some(object) = attributes.as_object() else {
        blockers.push("attributes must be a JSON object".to_owned());
        return blockers;
    };
    match object.get(key).and_then(Value::as_str) {
        Some(value) if !value.trim().is_empty() => {}
        Some(_) => blockers.push(format!("{key} must not be empty")),
        None => blockers.push(format!("{key} is required")),
    }
    blockers
}
