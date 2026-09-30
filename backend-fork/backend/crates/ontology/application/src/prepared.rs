//! The ontology action command's ONE preparation step.
//!
//! Preflight and execute are the same decision asked twice: preflight reports it,
//! execute commits on it. When each entry point assembles that decision itself
//! they drift, and every drift is a lie the console tells the operator — a green
//! preflight for a command the writeback then refuses, or a gate evaluated from
//! two hand-built evidence bags that disagree.
//!
//! [`PreparedCommand`] is the single place that decision is made. It is
//! deliberately ONTOLOGY-SPECIFIC — an action definition, an action command, a
//! §16 gate chain — not a generic command framework. There is exactly one caller
//! (`console-ontology-rest`) and a framework here would be an abstraction
//! invented for a second caller that does not exist.
//!
//! It is also PURE: it is handed the facts the REST/adapter tier read (the
//! resolved action, the command, the target's current attributes, the authority
//! effect and the four-eyes decision) and returns a verdict. It performs no I/O
//! and holds no connection, which is what keeps it on the right side of the
//! layer-boundary gate.
//!
//! One decision, TWO questions. Preflight asks "would this execute?" and execute
//! asks "does this execute?", so everything either one gates on is decided here
//! once — but the inputs only the WRITEBACK consumes (`command_id`,
//! `expected_revision`) are checked in [`PreparedCommand::writeback_inputs`],
//! which only execute calls. Requiring them at preparation would refuse a
//! preflight the shipped request schema declares valid.

use console_governance_domain::{
    AuthorityEffect, GateChainConfig, GateChainOutcome, GateEvidence, evaluate_gate_chain,
};
use console_kernel_core::KernelError;
use console_ontology_domain::ActionDispatch;
use serde_json::Value;
use uuid::Uuid;

use crate::{
    apply_edits, egress_evidence, evaluate_submission_criteria, evaluation_context,
    parse_control_points, validate_params,
};

/// The resolved action type's declarative facts — the subset of the registry row
/// the decision depends on. Copied out of the adapter's row type because the
/// application layer may not depend on the adapter (layer-boundary gate).
#[derive(Debug, Clone)]
pub struct ActionDefinition {
    /// Stable action key; also the `kind` a four-eyes approval must be bound to.
    pub stable_key: String,
    pub dispatch: ActionDispatch,
    pub dispatch_target: Option<String>,
    pub control_points: Value,
    pub params_schema: Value,
    pub submission_criteria: Value,
    pub side_effects: Value,
    pub edits: Value,
}

/// The caller-supplied half of an action command. Everything here is untrusted
/// input; nothing here decides a gate.
#[derive(Debug, Clone)]
pub struct CommandInputs {
    pub object_type_id: Uuid,
    pub instance_id: Option<Uuid>,
    pub command_id: Option<Uuid>,
    pub expected_revision: Option<i64>,
    pub params: Value,
    pub checklist_all_acknowledged: Option<bool>,
    pub four_eyes_request_ref: Option<Uuid>,
}

/// What the command will do once the gates admit it — resolved during
/// preparation so preflight refuses exactly what execute would refuse.
#[derive(Debug, Clone)]
pub enum PreparedDispatch {
    /// Append a revision carrying the resolved attribute bag the edits produce.
    InstanceRevision { attributes: Value },
    /// Route to the owning domain use-case, which owns its own tx/RLS/audit.
    ProjectedUsecase,
}

/// The inputs only the WRITEBACK consumes: `command_id` keys the replay receipt
/// and `expected_revision` is the CAS version. Preflight evaluates neither, so
/// they are checked in [`PreparedCommand::writeback_inputs`] rather than during
/// preparation.
#[derive(Debug, Clone, Copy)]
pub struct WritebackInputs {
    pub command_id: Uuid,
    pub expected_revision: Option<i64>,
}

/// One action command, fully validated and ready to be reported (preflight) or
/// committed (execute). Constructed by [`PreparedCommand::prepare`]; there is no
/// other way to build one, so no caller can reach the writeback with inputs the
/// preparation would have refused.
#[derive(Debug, Clone)]
pub struct PreparedCommand {
    action: ActionDefinition,
    inputs: CommandInputs,
    config: GateChainConfig,
    params: Value,
    criteria: Result<(), KernelError>,
    dispatch: PreparedDispatch,
}

impl PreparedCommand {
    /// Resolve and validate everything that does not need a connection.
    ///
    /// `base_attributes` are the target's CURRENT attributes, already read
    /// through the object-policy gate by the caller (an empty object for a
    /// create). Order matters: control points parse first (an action naming a
    /// gate this build cannot enforce must never run), then params, then the
    /// edit-shape check.
    ///
    /// # Errors
    /// A [`KernelError`] for a malformed control point, param or edit — the
    /// same refusal both entry points report, because both prepare here.
    pub fn prepare(
        action: ActionDefinition,
        inputs: CommandInputs,
        base_attributes: &Value,
    ) -> Result<Self, KernelError> {
        let config = parse_control_points(&action.control_points)?;
        let params = validate_params(&action.params_schema, &inputs.params)?;

        let context = evaluation_context(base_attributes, &params);
        let criteria = if projected_criteria_are_not_evaluable(&action) {
            // Fail-closed on config this build cannot honor: the engine cannot
            // read a projected domain row generically, so a submission criterion
            // would evaluate against an EMPTY base and could silently pass. Report
            // it as a failed criterion — preflight says "would not execute" and
            // execute refuses — rather than dispatch on a criterion we did not
            // really check.
            Err(KernelError::validation(
                "submission criteria are not evaluable for a projected_usecase \
                 action in v1 (the engine cannot read the projected domain row); \
                 nothing was dispatched",
            ))
        } else {
            evaluate_submission_criteria(&action.submission_criteria, &context)
        };

        let dispatch = match action.dispatch {
            ActionDispatch::InstanceRevision => PreparedDispatch::InstanceRevision {
                attributes: apply_edits(&action.edits, &params, base_attributes)?,
            },
            ActionDispatch::ProjectedUsecase => PreparedDispatch::ProjectedUsecase,
        };

        Ok(Self {
            action,
            inputs,
            config,
            params,
            criteria,
            dispatch,
        })
    }

    /// The inputs the writeback consumes, checked here so only execute can be
    /// refused for missing them.
    ///
    /// # Errors
    /// A [`KernelError`] when `command_id` is absent, or when an instance EDIT
    /// carries no `expected_revision` (a blind edit has no CAS guard).
    pub fn writeback_inputs(&self) -> Result<WritebackInputs, KernelError> {
        let command_id = self.inputs.command_id.ok_or_else(|| {
            KernelError::validation("command_id is required for instance_revision actions")
        })?;
        if self.inputs.instance_id.is_some() && self.inputs.expected_revision.is_none() {
            return Err(KernelError::validation(
                "expected_revision is required for an instance edit",
            ));
        }
        Ok(WritebackInputs {
            command_id,
            expected_revision: self.inputs.expected_revision,
        })
    }

    /// Evaluate the §16 chain from the one evidence bag both entry points build.
    /// `four_eyes_approved` is the caller's reading of the approval: a
    /// NON-CONSUMING peek for preflight, the in-tx consume for execute. Every
    /// other input is derived here, so the two readings cannot diverge on
    /// anything but the approval itself.
    #[must_use]
    pub fn gates(
        &self,
        authority: AuthorityEffect,
        four_eyes_approved: Option<bool>,
    ) -> GateChainOutcome {
        let evidence = GateEvidence {
            authority: Some(authority),
            checklist_all_acknowledged: self.inputs.checklist_all_acknowledged,
            four_eyes_approved,
            egress_cleared: egress_evidence(&self.action.side_effects),
        };
        evaluate_gate_chain(self.config, &evidence)
    }

    /// The `(kind, target)` a four-eyes approval must be bound to: the action's
    /// stable key, and the object acted on — the instance for an edit, the object
    /// type for a create. Both server-derived, never trusted from the caller, so
    /// an approval decided for a different action or object cannot satisfy this
    /// gate.
    #[must_use]
    pub fn four_eyes_binding(&self) -> (&str, Uuid) {
        let target = self
            .inputs
            .instance_id
            .unwrap_or(self.inputs.object_type_id);
        (self.action.stable_key.as_str(), target)
    }

    #[must_use]
    pub fn dispatch(&self) -> ActionDispatch {
        self.action.dispatch
    }

    #[must_use]
    pub fn dispatch_target(&self) -> Option<&str> {
        self.action.dispatch_target.as_deref()
    }

    #[must_use]
    pub fn prepared_dispatch(&self) -> &PreparedDispatch {
        &self.dispatch
    }

    #[must_use]
    pub fn config(&self) -> GateChainConfig {
        self.config
    }

    #[must_use]
    pub fn params(&self) -> &Value {
        &self.params
    }

    #[must_use]
    pub fn criteria_ok(&self) -> bool {
        self.criteria.is_ok()
    }

    /// The failing criterion's message, if any.
    #[must_use]
    pub fn criteria_error(&self) -> Option<&str> {
        self.criteria.as_ref().err().map(|e| e.message.as_str())
    }

    #[must_use]
    pub fn four_eyes_request_ref(&self) -> Option<Uuid> {
        self.inputs.four_eyes_request_ref
    }

    #[must_use]
    pub fn instance_id(&self) -> Option<Uuid> {
        self.inputs.instance_id
    }
}

/// A projected action's submission criteria cannot be evaluated in v1: its target
/// is a domain row the engine cannot read, so the criteria would run against an
/// empty base and pass fail-open.
fn projected_criteria_are_not_evaluable(action: &ActionDefinition) -> bool {
    matches!(action.dispatch, ActionDispatch::ProjectedUsecase)
        && action
            .submission_criteria
            .as_array()
            .is_some_and(|criteria| !criteria.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use console_kernel_core::ErrorKind;
    use serde_json::json;

    fn instance_action(criteria: Value) -> ActionDefinition {
        ActionDefinition {
            stable_key: "set_priority".to_owned(),
            dispatch: ActionDispatch::InstanceRevision,
            dispatch_target: None,
            control_points: json!(["authority"]),
            params_schema: json!({"priority": {"required": true}}),
            submission_criteria: criteria,
            side_effects: json!([]),
            edits: json!([{"property": "priority", "param": "priority"}]),
        }
    }

    fn inputs() -> CommandInputs {
        CommandInputs {
            object_type_id: Uuid::from_u128(1),
            instance_id: None,
            command_id: Some(Uuid::from_u128(9)),
            expected_revision: None,
            params: json!({"priority": "hi"}),
            checklist_all_acknowledged: None,
            four_eyes_request_ref: None,
        }
    }

    /// Preflight evaluates neither `command_id` nor `expected_revision`, so
    /// preparation must not require them: a preflight is a legitimate request
    /// before any command id is minted, and refusing it 422s a caller the shipped
    /// request schema declares valid.
    #[test]
    fn preparation_does_not_require_the_writeback_only_inputs() {
        PreparedCommand::prepare(
            instance_action(json!([])),
            CommandInputs {
                command_id: None,
                instance_id: Some(Uuid::from_u128(4)),
                expected_revision: None,
                ..inputs()
            },
            &json!({}),
        )
        .expect("preflight must prepare without command_id / expected_revision");
    }

    #[test]
    fn the_writeback_requires_a_command_id() {
        let prepared = PreparedCommand::prepare(
            instance_action(json!([])),
            CommandInputs {
                command_id: None,
                ..inputs()
            },
            &json!({}),
        )
        .unwrap();
        assert_eq!(
            prepared.writeback_inputs().unwrap_err().message,
            "command_id is required for instance_revision actions"
        );
    }

    #[test]
    fn the_writeback_requires_an_expected_revision_for_an_instance_edit() {
        let prepared = PreparedCommand::prepare(
            instance_action(json!([])),
            CommandInputs {
                instance_id: Some(Uuid::from_u128(4)),
                expected_revision: None,
                ..inputs()
            },
            &json!({}),
        )
        .unwrap();
        assert_eq!(
            prepared.writeback_inputs().unwrap_err().message,
            "expected_revision is required for an instance edit"
        );
    }

    #[test]
    fn malformed_edits_are_refused_at_preparation_not_at_writeback() {
        let mut action = instance_action(json!([]));
        action.edits = json!([{"property": "priority", "value": 1, "param": "priority"}]);
        let err = PreparedCommand::prepare(action, inputs(), &json!({})).unwrap_err();
        assert_eq!(err.kind, ErrorKind::Validation, "got {err:?}");
    }

    #[test]
    fn an_undeclared_param_is_refused() {
        let err = PreparedCommand::prepare(
            instance_action(json!([])),
            CommandInputs {
                params: json!({"priority": "hi", "undeclared": 1}),
                ..inputs()
            },
            &json!({}),
        )
        .unwrap_err();
        assert_eq!(err.kind, ErrorKind::Validation, "got {err:?}");
    }

    #[test]
    fn an_unknown_control_point_refuses_the_whole_command() {
        let mut action = instance_action(json!([]));
        action.control_points = json!(["authority", "teleport"]);
        let err = PreparedCommand::prepare(action, inputs(), &json!({})).unwrap_err();
        assert_eq!(err.kind, ErrorKind::Validation, "got {err:?}");
    }

    #[test]
    fn a_failing_criterion_is_reported_not_raised() {
        let prepared = PreparedCommand::prepare(
            instance_action(json!([{"field": "count", "op": "gte", "value": 10}])),
            inputs(),
            &json!({"count": 5}),
        )
        .expect("a failing criterion still prepares — preflight must be able to report it");
        assert!(!prepared.criteria_ok());
        assert!(prepared.criteria_error().unwrap().contains("count"));
    }

    /// A criterion the engine cannot evaluate is a REPORTED failure, not a raised
    /// one: preflight must be able to say "would not execute, and here is why"
    /// for exactly the command execute then refuses.
    #[test]
    fn projected_actions_report_criteria_they_cannot_evaluate() {
        let action = ActionDefinition {
            dispatch: ActionDispatch::ProjectedUsecase,
            dispatch_target: Some("registry.update_equipment".to_owned()),
            params_schema: json!({}),
            edits: json!([]),
            submission_criteria: json!([{"field": "count", "op": "gte", "value": 1}]),
            ..instance_action(json!([]))
        };
        let prepared = PreparedCommand::prepare(
            action,
            CommandInputs {
                params: json!({}),
                command_id: None,
                ..inputs()
            },
            &json!({}),
        )
        .unwrap();
        assert!(!prepared.criteria_ok());
        assert!(
            prepared
                .criteria_error()
                .unwrap()
                .contains("not evaluable for a projected_usecase"),
            "got {:?}",
            prepared.criteria_error()
        );
    }

    #[test]
    fn the_edits_resolve_against_the_base_the_gates_were_evaluated_from() {
        let prepared =
            PreparedCommand::prepare(instance_action(json!([])), inputs(), &json!({"note": "a"}))
                .unwrap();
        let PreparedDispatch::InstanceRevision { attributes } = prepared.prepared_dispatch() else {
            panic!("instance_revision dispatch expected");
        };
        assert_eq!(attributes["priority"], "hi");
        assert_eq!(
            attributes["note"], "a",
            "the committed bag is derived from the base the criteria and gates saw"
        );
    }

    #[test]
    fn four_eyes_binds_to_the_instance_for_an_edit_and_the_type_for_a_create() {
        let create =
            PreparedCommand::prepare(instance_action(json!([])), inputs(), &json!({})).unwrap();
        assert_eq!(
            create.four_eyes_binding(),
            ("set_priority", Uuid::from_u128(1))
        );
        let edit = PreparedCommand::prepare(
            instance_action(json!([])),
            CommandInputs {
                instance_id: Some(Uuid::from_u128(4)),
                expected_revision: Some(1),
                ..inputs()
            },
            &json!({}),
        )
        .unwrap();
        assert_eq!(
            edit.four_eyes_binding(),
            ("set_priority", Uuid::from_u128(4))
        );
    }

    #[test]
    fn gates_read_checklist_and_egress_from_the_one_prepared_command() {
        let mut action = instance_action(json!([]));
        action.control_points = json!(["authority", "self_checklist", "egress_dlp"]);
        action.side_effects = json!([{"kind": "webhook"}]);
        let prepared = PreparedCommand::prepare(
            action,
            CommandInputs {
                checklist_all_acknowledged: Some(true),
                ..inputs()
            },
            &json!({}),
        )
        .unwrap();
        // A side-effect-bearing action has no egress evidence ⇒ fail-closed deny,
        // for preflight and execute alike because there is one evidence bag.
        assert!(!prepared.gates(AuthorityEffect::Allow, None).allow);
    }
}
