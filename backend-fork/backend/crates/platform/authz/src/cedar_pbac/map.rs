//! Canonical Cedar/PBAC coexistence-map loader.
//!
//! Parses the org-agnostic spec map into typed [`CoexistenceMapEntry`] rows.
//! Slice 3 is ADDITIVE/INERT: the produced entries are not on any live request
//! path yet (that is the shadow-wiring slice). This loader's whole job is to make
//! the map-shape gaps explicit and fail-closed:
//!
//!  * **Multi-action expansion.** Each JSON entry carries an `actions` array; the
//!    loader expands it into one [`CoexistenceMapEntry`] per resolvable action,
//!    keyed on the entry's single `resourceType`, so `(domain, feature,
//!    resource_type)` identifies an entry.
//!  * **Canonical action id.** A map action id IS the snake_case [`Feature::as_str`]
//!    value (the Cedar action id). The loader resolves it via [`Feature::from_str`]
//!    and re-asserts `feature.as_str() == action` so the map ids and the matrix
//!    can never drift.
//!  * **Fail-closed on drift.** A genuinely unknown action id (a typo or an
//!    un-declared action) aborts the whole load with an error — never a silent
//!    drop, never a silent allow.
//!  * **Explicitly scoped-out actions.** A small set of action ids
//!    ([`SCOPED_OUT_ACTIONS`]) have no [`Feature`] variant *today* and are known
//!    to stay on the legacy gate for their domain. They are surfaced in
//!    [`CoexistenceMapLoad::scoped_out`] (visible, not dropped) rather than
//!    enrolled. Adding their `Feature`s later turns them into enrolled entries
//!    with no loader change — enrolling a domain is a data change, not a rewrite.
//!  * **No org-specific bundle binding here.** The map is org-agnostic; a real
//!    [`CompiledBundleCacheKey`](super::CompiledBundleCacheKey) needs an org +
//!    policy version that only exist at authorization time. Legacy-only entries
//!    therefore carry `bundle_key = None`, and a Cedar-requiring mode fails the
//!    load closed (the static loader must not fabricate a bundle identity).

use std::str::FromStr;

use console_kernel_core::KernelError;

use super::{CoexistenceMapEntry, DualEngineMode, cedar_required};
use crate::Feature;

/// The canonical coexistence map, baked in at compile time so the loader always
/// parses THE source-of-truth spec artifact. A path or shape drift is a compile
/// error, not a silent runtime miss.
pub const CANONICAL_COEXISTENCE_MAP_JSON: &str =
    include_str!("../../../../../../docs/specs/cedar-pbac-coexistence-map.json");

/// Coexistence-map action ids that intentionally have NO [`Feature`] variant
/// today and are therefore NOT Cedar-enrolled. They stay governed by the
/// existing legacy gate for their domain:
///
///  * `user_role_assignment_write` / `policy_role_write` route through the
///    RoleManage legacy gate — there is no distinct `Feature`; writing role
///    assignments / policy roles is authorized by the same `role_manage`
///    capability that the pilot enrolls.
///  * the `workflow_*` guard actions route through the legacy workflow guards,
///    pending the workflow capability `Feature`s (ADR-0018) being added to
///    [`Feature`].
///
/// Listing them EXPLICITLY is what keeps the loader fail-closed: a genuinely
/// unknown action id (a typo or spec drift) still errors, while these
/// known-unmodeled ids are scoped out with intent and surfaced in
/// [`CoexistenceMapLoad::scoped_out`] instead of being silently dropped.
pub const SCOPED_OUT_ACTIONS: &[&str] = &[
    "user_role_assignment_write",
    "policy_role_write",
    "workflow_trigger",
    "workflow_node_execute",
    "workflow_connector_invoke",
    "workflow_human_approval",
];

/// A map action id that resolved to no [`Feature`] but is on the explicit
/// [`SCOPED_OUT_ACTIONS`] allowlist: recorded so the scope-out is auditable
/// rather than silent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScopedOutAction {
    /// The source map entry the action was declared under.
    pub entry_id: String,
    pub domain: String,
    pub action: String,
}

/// Result of loading the coexistence map: the enrolled entries plus the
/// explicitly scoped-out actions that were declared but intentionally not
/// enrolled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoexistenceMapLoad {
    pub entries: Vec<CoexistenceMapEntry>,
    pub scoped_out: Vec<ScopedOutAction>,
}

/// Parse and validate the canonical coexistence map baked into the binary.
///
/// Fails closed on any parse error, unknown mode, unknown action id, or
/// Cedar-requiring mode (see module docs and [`parse_coexistence_map`]).
pub fn canonical_coexistence_map() -> Result<CoexistenceMapLoad, KernelError> {
    parse_coexistence_map(CANONICAL_COEXISTENCE_MAP_JSON)
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawCoexistenceMap {
    entries: Vec<RawEntry>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawEntry {
    id: String,
    domain: String,
    resource_type: String,
    actions: Vec<String>,
    current_mode: String,
}

/// Parse `json` (canonical coexistence-map shape) into typed entries.
///
/// Generic over domain/action: it expands every entry's `actions` and resolves
/// `resource_type` from the entry, so enrolling a new domain later is a JSON
/// change, not a loader change. Fail-closed behavior:
///  * a malformed mode string, empty domain, or empty resource type → `Err`;
///  * an action id that resolves to no [`Feature`] and is not on
///    [`SCOPED_OUT_ACTIONS`] → `Err` (drift/typo can't silently pass);
///  * a Cedar-requiring mode → `Err` (the org-agnostic loader must not fabricate
///    a per-org bundle identity; that binding happens at authorization time).
pub fn parse_coexistence_map(json: &str) -> Result<CoexistenceMapLoad, KernelError> {
    let raw: RawCoexistenceMap = serde_json::from_str(json)
        .map_err(|err| KernelError::validation(format!("coexistence map parse failed: {err}")))?;

    let mut entries = Vec::new();
    let mut scoped_out = Vec::new();

    for entry in raw.entries {
        let domain = entry.domain.trim();
        if domain.is_empty() {
            return Err(KernelError::validation(format!(
                "coexistence map entry {} has an empty domain",
                entry.id
            )));
        }
        let resource_type = entry.resource_type.trim();
        if resource_type.is_empty() {
            return Err(KernelError::validation(format!(
                "coexistence map entry {} has an empty resourceType",
                entry.id
            )));
        }

        let mode = DualEngineMode::from_str(&entry.current_mode)?;
        // The org-agnostic static map cannot resolve a per-org compiled bundle
        // key (org + policy version only exist at authorization time), so a
        // Cedar-requiring mode fails the load closed rather than fabricating a
        // bundle identity. Legacy-only entries carry `None`; the shadow-wiring
        // slice attaches the real per-org key from `engine::compile_bundle`.
        if cedar_required(mode) {
            return Err(KernelError::validation(format!(
                "coexistence map entry {} enrolls Cedar-requiring mode {mode:?}, but the \
                 static loader cannot resolve a per-org compiled bundle key; bundle binding \
                 is wired per-org at authorization time",
                entry.id
            )));
        }

        for action in &entry.actions {
            match Feature::from_str(action) {
                Ok(feature) => {
                    // The map action id must BE the canonical Cedar action id
                    // (Feature::as_str). from_str/as_str are inverse for valid
                    // snake_case, so this only fires if the map and matrix drift.
                    if feature.as_str() != action.as_str() {
                        return Err(KernelError::validation(format!(
                            "coexistence map action id {action} is not its canonical Feature id {}",
                            feature.as_str()
                        )));
                    }
                    entries.push(CoexistenceMapEntry::new(
                        format!("{domain}.{}", feature.as_str()),
                        domain,
                        feature,
                        resource_type,
                        mode,
                        None,
                    ));
                }
                Err(_) if SCOPED_OUT_ACTIONS.contains(&action.as_str()) => {
                    scoped_out.push(ScopedOutAction {
                        entry_id: entry.id.clone(),
                        domain: domain.to_owned(),
                        action: action.clone(),
                    });
                }
                Err(err) => {
                    return Err(KernelError::validation(format!(
                        "coexistence map entry {} declares action {action} that resolves to no \
                         Feature and is not an explicitly scoped-out action: {err}",
                        entry.id
                    )));
                }
            }
        }
    }

    Ok(CoexistenceMapLoad {
        entries,
        scoped_out,
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::{
        CANONICAL_COEXISTENCE_MAP_JSON, SCOPED_OUT_ACTIONS, canonical_coexistence_map,
        parse_coexistence_map,
    };
    use crate::{DualEngineMode, Feature};

    #[test]
    fn canonical_map_expands_only_feature_backed_actions_and_scopes_out_the_rest() {
        let load = canonical_coexistence_map().expect("canonical coexistence map must load");

        // identity.policy enrolls exactly the two Feature-backed capabilities;
        // its two write actions and every workflow.guards action are scoped out.
        let enrolled: Vec<_> = load
            .entries
            .iter()
            .map(|entry| (entry.domain.as_str(), entry.feature))
            .collect();
        assert_eq!(
            enrolled,
            vec![
                ("identity.policy", Feature::RoleManage),
                ("identity.policy", Feature::ElevatedRoleGrant),
            ],
            "only Feature-backed identity.policy actions may enroll"
        );

        let scoped: Vec<_> = load
            .scoped_out
            .iter()
            .map(|scoped| scoped.action.as_str())
            .collect();
        assert_eq!(
            scoped.as_slice(),
            SCOPED_OUT_ACTIONS,
            "scope-out must be surfaced, not dropped"
        );
    }

    #[test]
    fn canonical_map_entries_are_legacy_only_with_no_bundle_and_derived_ids() {
        let load = canonical_coexistence_map().unwrap();

        for entry in &load.entries {
            // Slice 3 is inert: the map stays legacy_only and carries no
            // fabricated bundle identity.
            assert_eq!(entry.mode, DualEngineMode::LegacyOnly);
            assert!(
                entry.bundle_key.is_none(),
                "static loader must not fabricate a per-org bundle key"
            );
            // The map action id is the canonical Cedar action id, and the entry
            // id is derived from (domain, action) so multi-action expansion is
            // unambiguous.
            assert_eq!(
                entry.id,
                format!("{}.{}", entry.domain, entry.feature.as_str())
            );
            assert_eq!(entry.resource_type, "identity.policy_role");
        }
    }

    #[test]
    fn unknown_action_id_fails_closed() {
        let json = r#"{
            "entries": [
                {
                    "id": "identity.policy.role_manage",
                    "domain": "identity.policy",
                    "resourceType": "identity.policy_role",
                    "actions": ["role_manage", "not_a_real_action"],
                    "currentMode": "legacy_only"
                }
            ]
        }"#;
        let err = parse_coexistence_map(json).expect_err("unknown action must fail closed");
        assert!(err.message.contains("not_a_real_action"), "{}", err.message);
    }

    #[test]
    fn cedar_requiring_mode_fails_closed_without_per_org_bundle() {
        let json = r#"{
            "entries": [
                {
                    "id": "identity.policy.role_manage",
                    "domain": "identity.policy",
                    "resourceType": "identity.policy_role",
                    "actions": ["role_manage"],
                    "currentMode": "cedar_shadow_legacy_enforce"
                }
            ]
        }"#;
        let err = parse_coexistence_map(json)
            .expect_err("Cedar-requiring mode must fail the static load");
        assert!(
            err.message.contains("per-org compiled bundle key"),
            "{}",
            err.message
        );
    }

    #[test]
    fn unknown_mode_and_empty_resource_type_fail_closed() {
        let bad_mode = r#"{
            "entries": [
                {
                    "id": "identity.policy.role_manage",
                    "domain": "identity.policy",
                    "resourceType": "identity.policy_role",
                    "actions": ["role_manage"],
                    "currentMode": "cedar_someday"
                }
            ]
        }"#;
        assert!(parse_coexistence_map(bad_mode).is_err());

        let empty_resource = r#"{
            "entries": [
                {
                    "id": "identity.policy.role_manage",
                    "domain": "identity.policy",
                    "resourceType": "  ",
                    "actions": ["role_manage"],
                    "currentMode": "legacy_only"
                }
            ]
        }"#;
        assert!(parse_coexistence_map(empty_resource).is_err());
    }

    #[test]
    fn canonical_map_json_uses_snake_case_canonical_action_ids() {
        // The baked-in map uses snake_case canonical action ids (Feature::as_str),
        // not the old PascalCase, so the map ids and the Cedar schema action ids
        // (also Feature::as_str) cannot drift.
        assert!(CANONICAL_COEXISTENCE_MAP_JSON.contains("\"role_manage\""));
        assert!(!CANONICAL_COEXISTENCE_MAP_JSON.contains("\"RoleManage\""));

        let load = canonical_coexistence_map().unwrap();
        for entry in &load.entries {
            assert!(
                entry.id.ends_with(entry.feature.as_str()),
                "entry id {} must embed the canonical action id {}",
                entry.id,
                entry.feature.as_str()
            );
        }
    }
}
