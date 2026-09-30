//! OWNED — a lane may not edit this file.
//!
//! Driver #2: the SAME `OntologyRestState` action methods, called in-process with
//! no HTTP, every call wrapped in an explicit `scope_org`. `PgOntologyStore` has
//! no action dispatch — `preflight_action`/`execute_action` live on
//! `OntologyRestState` (`rest/src/lib.rs:747`, `:774`) and are documented
//! HTTP-independent at `:592-596`.
//!
//! The two drivers therefore differ by EXACTLY the authz / request-context
//! boundary, which is the delta worth proving twice.

use std::collections::BTreeSet;

use console_kernel_core::{BranchScope, ErrorKind, OrgId, UserId};
use console_ontology_adapter_postgres::PgOntologyError;
use console_ontology_adapter_postgres::PgOntologyStore;
use console_ontology_adapter_postgres::instances::{
    InstanceState, PgInstanceStore, RevisionSummary, TraversalGraph,
};
use console_ontology_domain::{InstanceId, ObjectTypeId};
use console_ontology_rest::{ActionCommand, ActionError, OntologyRestState};
use console_platform_authz::{Principal, Role};
use console_platform_request_context::scope_org;
use serde_json::json;
use time::OffsetDateTime;

use super::harness::Harness;
use super::{ACTION_KEY, Actor, Command, Driver, Failure};

pub struct StoreDriver {
    org: OrgId,
    state: OntologyRestState,
    registry: PgOntologyStore,
    instances: PgInstanceStore,
    admin: Principal,
    executive: Principal,
}

impl StoreDriver {
    pub fn new(h: &Harness) -> Self {
        Self {
            org: h.org,
            state: h.state(None),
            registry: h.registry(),
            instances: h.instances(),
            admin: principal(h.admin, h.org, Role::SuperAdmin),
            executive: principal(h.executive, h.org, Role::Executive),
        }
    }

    fn principal(&self, actor: Actor) -> &Principal {
        match actor {
            Actor::Privileged => &self.admin,
            Actor::Unprivileged => &self.executive,
        }
    }

    /// CTL-5's store-only extension: the same admitted action with NO tenant
    /// context bound. Unreachable through REST, where the router arms
    /// `CURRENT_ORG` from the verified token before any handler runs.
    pub async fn execute_unarmed(&self, object_type_id: ObjectTypeId) -> Failure {
        let command = ActionCommand {
            object_type_id,
            instance_id: None,
            title: Some("unarmed".to_owned()),
            params: json!({}),
            reason: None,
            valid_from: None,
            checklist_all_acknowledged: None,
            four_eyes_request_ref: None,
            command_id: Some(uuid::Uuid::new_v4()),
            expected_revision: None,
        };
        match self
            .state
            .execute_action(&self.admin, ACTION_KEY, command)
            .await
        {
            Err(error) => normalize(error),
            Ok(_) => panic!("[store] CTL-5: an unarmed app.current_org must fail closed"),
        }
    }
}

fn principal(user: UserId, org: OrgId, role: Role) -> Principal {
    Principal::new(user, org, BTreeSet::from([role]), BranchScope::All)
}

/// `RestError::from_kernel/from_ontology/from_action` are private, so this
/// normalises ONLY the arms the assertions name and PANICS on the rest — an
/// unexpected arm is a harness bug and must be loud, never absorbed into a code
/// an assertion happens to accept.
fn normalize(error: ActionError) -> Failure {
    match error {
        ActionError::NotFound => Failure {
            code: "not_found".to_owned(),
            message: "action type was not found for that object type".to_owned(),
        },
        ActionError::GateDenied(message) => Failure {
            code: "gate_denied".to_owned(),
            message,
        },
        ActionError::Validation(message) => Failure {
            code: "validation".to_owned(),
            message,
        },
        ActionError::CriteriaFailed(message) => Failure {
            code: "criteria_failed".to_owned(),
            message,
        },
        ActionError::Store(error) => normalize_store(error),
        other => panic!("[store] unexpected ActionError arm: {other:?}"),
    }
}

/// Mirrors `code_for_error_kind` (`rest/src/lib.rs:1919-1928`) and the
/// `ActionPreconditionFailed` arm of `from_ontology` (`:1780-1785`), so both
/// drivers return byte-identical `(code, message)` pairs.
fn normalize_store(error: PgOntologyError) -> Failure {
    match error {
        PgOntologyError::Domain(kernel) => Failure {
            code: match kernel.kind {
                ErrorKind::Validation => "validation",
                ErrorKind::NotFound => "not_found",
                ErrorKind::Forbidden => "forbidden",
                ErrorKind::Conflict => "conflict",
                ErrorKind::InvalidTransition => "invalid_transition",
                ErrorKind::Internal => "internal",
            }
            .to_owned(),
            message: kernel.message,
        },
        PgOntologyError::ActionPreconditionFailed { current } => Failure {
            code: "ontology_action_revision_precondition_failed".to_owned(),
            message: format!("stale action revision; current revision is {current}"),
        },
        other => panic!("[store] unexpected PgOntologyError arm: {other:?}"),
    }
}

impl Driver for StoreDriver {
    const NAME: &'static str = "store";
    /// No routing layer, so the same principal reaches `execute_action`, where
    /// `authority_effect` evaluates the SAME `Feature::RoleManage`
    /// (`rest/src/lib.rs:1002-1009`) inside the writeback tx and the chain denies
    /// → the tx rolls back with zero rows.
    const DENIAL_CODE: &'static str = "gate_denied";

    async fn resolve_type(&self, key: &str) -> Result<ObjectTypeId, Failure> {
        scope_org(self.org, async {
            self.registry
                .get_object_type(key, None)
                .await
                .map(|detail| detail.object_type.id)
                .map_err(normalize_store)
        })
        .await
    }

    async fn execute(&self, cmd: &Command, actor: Actor) -> Result<InstanceState, Failure> {
        let object_type_id = self.resolve_type(cmd.type_key).await?;
        let command = ActionCommand {
            object_type_id,
            instance_id: cmd.instance.map(|(id, _)| id),
            title: cmd.title.clone(),
            params: cmd.params.clone(),
            reason: None,
            valid_from: cmd.valid_from,
            checklist_all_acknowledged: None,
            four_eyes_request_ref: None,
            command_id: Some(cmd.command_id),
            expected_revision: cmd.instance.map(|(_, revision)| revision),
        };
        scope_org(self.org, async {
            self.state
                .execute_action(self.principal(actor), ACTION_KEY, command)
                .await
                .map_err(normalize)
                .map(|outcome| {
                    outcome
                        .instance
                        .expect("an instance_revision execute carries an instance")
                })
        })
        .await
    }

    async fn read(
        &self,
        id: InstanceId,
        as_of: Option<OffsetDateTime>,
    ) -> Result<InstanceState, Failure> {
        scope_org(self.org, async {
            match as_of {
                Some(at) => self.instances.get_as_of(id, at).await,
                None => self.instances.get_current(id).await,
            }
            .map_err(normalize_store)
        })
        .await
    }

    async fn history(&self, id: InstanceId) -> Result<Vec<RevisionSummary>, Failure> {
        scope_org(self.org, async {
            self.instances.history(id).await.map_err(normalize_store)
        })
        .await
    }

    async fn traverse(&self, root: InstanceId, depth: u32) -> Result<TraversalGraph, Failure> {
        // No link-type filter: the target asserts that an edge EXISTS and where it
        // lands, never what a lane named its link type.
        scope_org(self.org, async {
            self.instances
                .traverse(root, None, depth)
                .await
                .map_err(normalize_store)
        })
        .await
    }
}
