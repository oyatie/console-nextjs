//! Ontology action application layer (§2 dispatch, §6 request composition).
//!
//! This layer is PURE (domain + kernel only, no sqlx / axum / tokio — enforced by
//! the layer-boundary gate). The DB *I/O* of the execute path (read the action,
//! read the target through the object-policy gate, open the audited tx) lives in
//! `console-ontology-rest`, which can touch the stores and Cedar; the DECISION
//! both preflight and execute act on is assembled here, once. Here we own the
//! deterministic, unit-testable pieces of that path:
//!
//!  * [`PreparedCommand`] — the ontology action command's single preparation
//!    step: control points, params, inputs, edits, criteria and the §16 evidence
//!    bag resolved ONCE, so preflight and execute cannot drift;
//!  * [`parse_control_points`] — the action's `control_points` JSONB → a §16
//!    [`GateChainConfig`], **fail-closed** on any gate name this build cannot
//!    enforce (a security gate we don't understand must never be silently skipped);
//!  * [`validate_params`] — inputs vs the action's `params_schema`;
//!  * [`evaluate_submission_criteria`] — the field·op·value predicate grammar that
//!    gates submit (same shape as the no-code conditions), **fail-closed**: a
//!    malformed criterion or an un-evaluable comparison denies, naming the culprit;
//!  * [`apply_edits`] — the declarative property writes that produce the new
//!    attribute bag the instance-revision dispatch persists.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

use console_governance_domain::GateChainConfig;
use console_kernel_core::KernelError;
use serde::{Deserialize, Serialize};
use serde_json::Value;

mod prepared;

pub use prepared::{
    ActionDefinition, CommandInputs, PreparedCommand, PreparedDispatch, WritebackInputs,
};

/// Which dispatch a resolved action performs — mirrors
/// [`console_ontology_domain::ActionDispatch`] but re-exported here so the rest layer
/// can branch without importing the domain enum by another name.
pub use console_ontology_domain::ActionDispatch;

// ===========================================================================
// §16 control-point config parsing.
// ===========================================================================

/// Parse the action's `control_points` (a JSONB array of gate-name strings) into
/// the §16 [`GateChainConfig`]. An action opts a gate in by listing its slug:
/// `["authority", "four_eyes"]`.
///
/// **Fail-closed:** an unrecognized gate slug is a hard error, not a skip — an
/// action that requests a control point this build cannot enforce must not run at
/// all. (Contrast the field-type reader, which degrades unknown *display* types;
/// a security gate is not a display concern.)
pub fn parse_control_points(control_points: &Value) -> Result<GateChainConfig, KernelError> {
    // A missing/empty config means "no extra gates"; the array CHECK on the column
    // means we normally get an array, but be lenient about JSON null == no gates.
    if control_points.is_null() {
        return Ok(GateChainConfig::default());
    }
    let arr = control_points.as_array().ok_or_else(|| {
        KernelError::validation("control_points must be a JSON array of gate names")
    })?;
    let mut config = GateChainConfig::default();
    for entry in arr {
        let slug = entry.as_str().ok_or_else(|| {
            KernelError::validation("each control point must be a gate-name string")
        })?;
        match slug {
            "authority" => config.authority = true,
            "self_checklist" => config.self_checklist = true,
            "four_eyes" => config.four_eyes = true,
            "egress_dlp" => config.egress_dlp = true,
            other => {
                return Err(KernelError::validation(format!(
                    "unknown control point gate '{other}' — refusing to execute (fail-closed)"
                )));
            }
        }
    }
    Ok(config)
}

/// Egress-gate evidence derived server-side from the action's declared side
/// effects. An action with no outbound side effects has nothing to classify, so
/// its egress gate auto-clears; an action that DOES declare side effects returns
/// `None` (no evidence) so the egress gate fails closed until a real §13 egress /
/// DLP classifier clears it.
// ponytail: no classifier yet — side-effect-bearing actions can't pass egress_dlp
// until the §13 egress lane wires one; empty side_effects is the only auto-clear.
#[must_use]
pub fn egress_evidence(side_effects: &Value) -> Option<bool> {
    match side_effects.as_array() {
        Some(effects) if effects.is_empty() => Some(true),
        Some(_) => None,
        // Not an array (shouldn't happen given the column CHECK) — fail closed.
        None => None,
    }
}

// ===========================================================================
// params_schema validation.
// ===========================================================================

/// Validate `params` against the action's `params_schema` and return the
/// normalized params object.
///
/// `params_schema` is a JSON object keyed by param name; each value may carry a
/// `"required": true` flag. When the schema declares params, an input key that is
/// not declared is rejected (a trust boundary — unknown inputs never flow through
/// to edits), and a missing required param is rejected. An empty schema accepts
/// the params as-is.
// ponytail: coarse — enforces declared-keys + required only, not deep per-type
// checks; the resulting attributes are still shape-validated by the instance
// store against the property schema. Tighten if an authoring surface needs it.
pub fn validate_params(params_schema: &Value, params: &Value) -> Result<Value, KernelError> {
    let params_obj = match params {
        Value::Null => serde_json::Map::new(),
        Value::Object(map) => map.clone(),
        _ => return Err(KernelError::validation("params must be a JSON object")),
    };
    let Some(schema) = params_schema.as_object() else {
        // No/other schema shape → accept as-is (nothing to check against).
        return Ok(Value::Object(params_obj));
    };
    if schema.is_empty() {
        return Ok(Value::Object(params_obj));
    }
    for key in params_obj.keys() {
        if !schema.contains_key(key) {
            return Err(KernelError::validation(format!(
                "param '{key}' is not declared in the action's params_schema"
            )));
        }
    }
    for (key, spec) in schema {
        let required = spec
            .get("required")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if required {
            match params_obj.get(key) {
                Some(v) if !v.is_null() => {}
                _ => {
                    return Err(KernelError::validation(format!(
                        "required param '{key}' is missing"
                    )));
                }
            }
        }
    }
    Ok(Value::Object(params_obj))
}

// ===========================================================================
// submission_criteria — field·op·value predicates, fail-closed.
// ===========================================================================

/// Comparison a submission criterion applies. Mirrors the no-code condition
/// grammar (`field <op> value`), widened with the ordering/`exists` ops a submit
/// gate needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PredicateOp {
    Eq,
    Ne,
    Gt,
    Lt,
    Gte,
    Lte,
    /// Set-membership (array contains) or substring (string contains).
    Contains,
    /// The field is present and non-null (`value` ignored).
    Exists,
}

/// One AND-ed submit predicate over the evaluation context (params merged onto
/// the target's current attributes).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubmissionCriterion {
    pub field: String,
    pub op: PredicateOp,
    #[serde(default)]
    pub value: Value,
}

/// Evaluate the action's `submission_criteria` (a JSON array of predicates)
/// against `context` — the params merged over the target instance's current
/// attributes. All predicates must hold (AND).
///
/// **Fail-closed:** if the criteria array does not parse into predicates, or a
/// comparison cannot be evaluated (e.g. an ordering op on a non-number, or a
/// missing field for a comparison), the submit is DENIED, naming the criterion —
/// never skipped or defaulted to pass.
pub fn evaluate_submission_criteria(
    submission_criteria: &Value,
    context: &Value,
) -> Result<(), KernelError> {
    if submission_criteria.is_null() {
        return Ok(());
    }
    let criteria: Vec<SubmissionCriterion> = serde_json::from_value(submission_criteria.clone())
        .map_err(|e| {
            KernelError::validation(format!(
                "submission_criteria is malformed (fail-closed deny): {e}"
            ))
        })?;
    let ctx = context.as_object().ok_or_else(|| {
        KernelError::validation("submission-criteria context must be a JSON object")
    })?;
    for criterion in &criteria {
        let present = ctx.get(&criterion.field);
        let satisfied = match criterion.op {
            PredicateOp::Exists => present.is_some_and(|v| !v.is_null()),
            PredicateOp::Eq => present == Some(&criterion.value),
            PredicateOp::Ne => present != Some(&criterion.value),
            PredicateOp::Contains => match present {
                Some(Value::Array(items)) => items.contains(&criterion.value),
                Some(Value::String(haystack)) => criterion
                    .value
                    .as_str()
                    .is_some_and(|needle| haystack.contains(needle)),
                _ => false,
            },
            PredicateOp::Gt | PredicateOp::Lt | PredicateOp::Gte | PredicateOp::Lte => {
                let lhs = present.and_then(Value::as_f64);
                let rhs = criterion.value.as_f64();
                match (lhs, rhs) {
                    (Some(l), Some(r)) => match criterion.op {
                        PredicateOp::Gt => l > r,
                        PredicateOp::Lt => l < r,
                        PredicateOp::Gte => l >= r,
                        PredicateOp::Lte => l <= r,
                        _ => unreachable!(),
                    },
                    // A non-numeric operand for an ordering op is un-evaluable → deny.
                    _ => {
                        return Err(KernelError::validation(format!(
                            "submission criterion '{}' requires numeric operands (fail-closed deny)",
                            criterion.field
                        )));
                    }
                }
            }
        };
        if !satisfied {
            return Err(KernelError::validation(format!(
                "submission criterion failed: '{}' {:?} {}",
                criterion.field, criterion.op, criterion.value
            )));
        }
    }
    Ok(())
}

// ===========================================================================
// edits — declarative property writes.
// ===========================================================================

/// One declarative property write. Sets `property` to either a constant `value`
/// or the value of a named `param`. Exactly one of `value` / `param` must be set.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Edit {
    pub property: String,
    #[serde(default)]
    pub value: Option<Value>,
    #[serde(default)]
    pub param: Option<String>,
}

/// Apply the action's `edits` to `base` (the target's current attributes, or an
/// empty object for a create) using `params`, returning the new attribute bag.
///
/// The returned object is later shape-validated by the instance store against the
/// object-type property schema, so this only resolves the writes — it does not
/// re-validate types here.
pub fn apply_edits(edits: &Value, params: &Value, base: &Value) -> Result<Value, KernelError> {
    let mut attributes = match base {
        Value::Null => serde_json::Map::new(),
        Value::Object(map) => map.clone(),
        _ => {
            return Err(KernelError::validation(
                "base attributes must be a JSON object",
            ));
        }
    };
    if edits.is_null() {
        return Ok(Value::Object(attributes));
    }
    let edits: Vec<Edit> = serde_json::from_value(edits.clone())
        .map_err(|e| KernelError::validation(format!("action edits are malformed: {e}")))?;
    for edit in &edits {
        let resolved = match (&edit.value, &edit.param) {
            (Some(_), Some(_)) => {
                return Err(KernelError::validation(format!(
                    "edit for property '{}' sets both value and param",
                    edit.property
                )));
            }
            (Some(value), None) => value.clone(),
            (None, Some(param)) => params.get(param).cloned().unwrap_or(Value::Null),
            (None, None) => {
                return Err(KernelError::validation(format!(
                    "edit for property '{}' sets neither value nor param",
                    edit.property
                )));
            }
        };
        attributes.insert(edit.property.clone(), resolved);
    }
    Ok(Value::Object(attributes))
}

/// Merge `params` over `attributes` into one evaluation context for
/// [`evaluate_submission_criteria`]. Params win on key collision (a submit
/// predicate reads the values the action is about to write).
#[must_use]
pub fn evaluation_context(attributes: &Value, params: &Value) -> Value {
    let mut ctx = attributes.as_object().cloned().unwrap_or_default();
    if let Some(params) = params.as_object() {
        for (key, value) in params {
            ctx.insert(key.clone(), value.clone());
        }
    }
    Value::Object(ctx)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn control_points_parse_and_reject_unknown_gate() {
        let cfg = parse_control_points(&json!(["authority", "four_eyes"])).unwrap();
        assert!(cfg.authority && cfg.four_eyes);
        assert!(!cfg.self_checklist && !cfg.egress_dlp);
        // fail-closed: an unknown gate slug is a hard error, not a skip.
        assert!(parse_control_points(&json!(["authority", "teleport"])).is_err());
        // empty / null → no gates.
        assert_eq!(
            parse_control_points(&json!([])).unwrap(),
            GateChainConfig::default()
        );
        assert_eq!(
            parse_control_points(&Value::Null).unwrap(),
            GateChainConfig::default()
        );
    }

    #[test]
    fn egress_auto_clears_only_when_no_side_effects() {
        assert_eq!(egress_evidence(&json!([])), Some(true));
        assert_eq!(egress_evidence(&json!([{"kind": "webhook"}])), None);
    }

    #[test]
    fn params_enforce_declared_keys_and_required() {
        let schema = json!({"priority": {"required": true}, "note": {}});
        assert!(validate_params(&schema, &json!({"priority": "hi"})).is_ok());
        // missing required
        assert!(validate_params(&schema, &json!({"note": "x"})).is_err());
        // unknown key
        assert!(validate_params(&schema, &json!({"priority": "hi", "nope": 1})).is_err());
        // empty schema accepts anything
        assert!(validate_params(&json!({}), &json!({"whatever": 1})).is_ok());
    }

    #[test]
    fn submission_criteria_pass_fail_and_fail_closed() {
        let ctx = json!({"priority": "hi", "count": 5});
        // passes: eq + gte
        assert!(
            evaluate_submission_criteria(
                &json!([
                    {"field": "priority", "op": "eq", "value": "hi"},
                    {"field": "count", "op": "gte", "value": 3}
                ]),
                &ctx
            )
            .is_ok()
        );
        // fails: value mismatch, named
        let err = evaluate_submission_criteria(
            &json!([{"field": "priority", "op": "eq", "value": "lo"}]),
            &ctx,
        )
        .unwrap_err();
        assert!(err.message.contains("priority"));
        // fail-closed: ordering op on a non-number denies
        assert!(
            evaluate_submission_criteria(
                &json!([{"field": "priority", "op": "gt", "value": 1}]),
                &ctx
            )
            .is_err()
        );
        // fail-closed: malformed criteria array denies
        assert!(evaluate_submission_criteria(&json!([{"nope": 1}]), &ctx).is_err());
    }

    #[test]
    fn edits_resolve_value_and_param_and_reject_ambiguity() {
        let params = json!({"priority": "hi"});
        let base = json!({"note": "keep"});
        let out = apply_edits(
            &json!([
                {"property": "priority", "param": "priority"},
                {"property": "status", "value": "open"}
            ]),
            &params,
            &base,
        )
        .unwrap();
        assert_eq!(out["priority"], "hi");
        assert_eq!(out["status"], "open");
        assert_eq!(out["note"], "keep", "base attributes are preserved");
        // both value and param set → error
        assert!(
            apply_edits(
                &json!([{"property": "x", "value": 1, "param": "priority"}]),
                &params,
                &json!({})
            )
            .is_err()
        );
        // neither set → error
        assert!(apply_edits(&json!([{"property": "x"}]), &params, &json!({})).is_err());
    }

    #[test]
    fn evaluation_context_lets_params_win() {
        let ctx = evaluation_context(&json!({"a": 1, "b": 2}), &json!({"b": 9, "c": 3}));
        assert_eq!(ctx["a"], 1);
        assert_eq!(ctx["b"], 9, "params override attributes");
        assert_eq!(ctx["c"], 3);
    }
}
