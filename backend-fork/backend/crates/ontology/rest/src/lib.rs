//! Ontology REST API — the §18 registry surface + the §2/§16 single mutation
//! path (action preflight / execute) that serves humans and automation alike.
//!
//! The object-type and instance endpoints are thin pass-throughs over the
//! registry + instance stores (which already own RLS + audit + fixity). The
//! action `preflight` / `execute` endpoints are the substance of this lane:
//!
//!  * `preflight` resolves the action, runs the §16 gate chain (authority via the
//!    legacy authorization contract → self-checklist → four-eyes read from the DB
//!    → egress derived from side effects) and returns each gate's status WITHOUT
//!    committing anything;
//!  * `execute` runs the same chain, and if it allows, dispatches:
//!    an `instance_revision` action opens ONE `with_audits` writeback that
//!    **re-checks** the mutable gate (four-eyes) inside the tx (TOCTOU-safe) and
//!    appends a fixity-chained revision; a `projected_usecase` action routes
//!    through the [`ProjectedDispatchRegistry`] into the OWNING domain crate's
//!    use-case (which owns its own RLS and transaction — §9.3, no second source
//!    of truth). Non-roster projected handlers (e.g. `registry.update_equipment`)
//!    also own their audit row; **canonical** ports open a raw `pool.begin()` and
//!    write no `audit_events`, so after a successful canonical dispatch the engine
//!    emits one org-scoped `ontology.action.execute` row (same action key as the
//!    instance-revision writeback, fail-closed if that insert cannot land). An
//!    unknown `dispatch_target` fails closed.
//!
//! `router(state)` self-applies `with_request_context`; `build_router` merges it
//! (L-WIRE), this crate does not.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

mod openapi;
pub use openapi::OPENAPI_FRAGMENT;

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use console_governance_adapter_postgres::{
    PgGovernanceError, PgGovernanceStore, authority_effect_from_cedar, four_eyes_consume_conn,
};
use console_governance_domain::{
    AuthorityEffect, GateChainConfig, GateChainOutcome, GateEvidence, LifecycleState,
    evaluate_gate_chain, validate_lifecycle_transition,
};
use console_kernel_core::{AuditAction, AuditEvent, ErrorKind, KernelError, TraceContext};
use console_ontology_adapter_postgres::instances::{
    AggregateBucket, AggregateGroupBy, CreateInstance, InstanceHead, InstanceState,
    PgInstanceStore, RevisionSummary, StageRevision, TraversalGraph, TraversalNode,
    create_instance_in_tx, stage_revision_in_tx,
};
use console_ontology_adapter_postgres::{
    ActingRule, ActionTypeSummary, CreateObjectTypeDraft, ObjectTypeSummary,
    ObjectTypeWritePrecondition, ObjectTypeWriteVersion, PgOntologyError, PgOntologyStore,
    PropertyDefSummary, ResolvedInstance,
};
use console_ontology_application::{
    ActionDefinition, ActionDispatch, CommandInputs, PreparedCommand, PreparedDispatch,
    WritebackInputs,
};
use console_ontology_canonical_domain::{
    CanonicalObject, CanonicalPort, CanonicalPortError, CanonicalQuery, CommandId, DispatchTarget,
    ObjectKey,
};
use console_ontology_domain::{
    FieldKind, InstanceId, InstanceLifecycleState, LinkTypeId, ObjectTypeId, SchemaLifecycleState,
};
use console_platform_auth::JwtVerifier;
use console_platform_authz::cedar_pbac::authoring::{
    self, Condition, ConditionOp, ConditionValue, DeclaredAttr, Effect, NoCodeBlocks,
};
use console_platform_authz::cedar_pbac::evaluate_legacy_contract;
use console_platform_authz::cedar_pbac::residual::{
    ObjectPolicy, Predicate, PredicateValue, ResidualOp, SqlValue, SubjectAttrs,
};
use console_platform_authz::{
    Action, AuthorizationRequest, AuthorizationResource, Feature, Principal, authorize_org_wide,
};
use console_platform_authz_rest::{AttachObjectPolicyCommand, PgCedarError, PgCedarPolicyStore};
use console_platform_db::{DbError, with_audits, with_org_conn, with_org_rollback};
use console_platform_request_context::current_org;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use sqlx::Row;
use std::collections::{HashMap, HashSet, VecDeque, hash_map::Entry};
use std::future::Future;
use std::pin::Pin;
use std::str::FromStr;
use std::sync::Arc;
use time::OffsetDateTime;
use uuid::Uuid;

mod job_position;
pub use job_position::{
    JobPositionIdentity, JobPositionProjectionError, identity_from_receipt_result,
};

// ---------------------------------------------------------------------------
// State + router
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct OntologyRestState {
    registry: PgOntologyStore,
    /// Sealed: see [`gate`]. Only `gate` can reach the unfiltered store inside.
    instances: gate::Instances,
    governance: PgGovernanceStore,
    policies: PgCedarPolicyStore,
    jwt_verifier: Option<JwtVerifier>,
    /// Routes a `projected_usecase` action to the OWNING domain crate's use-case.
    /// Empty by default ⇒ every projected dispatch fails closed (`NotWiredYet`),
    /// preserving the pre-wire dark behavior. The App composition root installs
    /// the real handlers via [`Self::with_projected_dispatch`].
    projected_dispatch: ProjectedDispatchRegistry,
}

impl OntologyRestState {
    #[must_use]
    pub fn new(
        registry: PgOntologyStore,
        instances: PgInstanceStore,
        governance: PgGovernanceStore,
        jwt_verifier: Option<JwtVerifier>,
    ) -> Self {
        // The attach route reaches `ont_policy_api.attach_object_policy` as
        // `console_ontology_cmd` (migration 0206), so the policy store needs the
        // SAME command credential the registry was built with. Derived from
        // `registry` rather than taken as a parameter: every composition root and
        // fixture that already wires the ontology command pool gets the policy one
        // for free, and none of them changes arity.
        let mut policies = PgCedarPolicyStore::new(registry.pool().clone());
        if let Some(command_pool) = registry.command_pool_opt() {
            policies = policies.with_command_pool(command_pool.clone());
        }
        Self {
            registry,
            // The public parameter stays a `PgInstanceStore`: sealing is internal
            // to this crate and no composition root or fixture changes.
            instances: gate::Instances::new(instances),
            policies,
            governance,
            jwt_verifier,
            projected_dispatch: ProjectedDispatchRegistry::new(),
        }
    }

    /// Install the projected-dispatch registry (target → domain use-case). Supplied
    /// by the App tier, which alone may depend on the domain adapters; the ontology
    /// REST tier stays free of a domain-adapter edge (dependency inversion, exactly
    /// like `TenantConfigSeeder`). An unregistered target still fails closed.
    #[must_use]
    pub fn with_projected_dispatch(mut self, registry: ProjectedDispatchRegistry) -> Self {
        self.projected_dispatch = registry;
        self
    }
}

// ---------------------------------------------------------------------------
// Projected dispatch registry (§18 D1/D2, arch §1a + §9.3)
// ---------------------------------------------------------------------------

/// Everything a domain use-case needs to service one `projected_usecase` action,
/// resolved from the action + command (HTTP-independent). The engine performs NO
/// writeback of its own for a projected action; the handler routes into the owning
/// domain crate's use-case, which owns its RLS + audit + transaction (§9.3: never a
/// second source of truth). Tenant scope is ambient via `app.current_org`
/// (the caller already scoped it), so no org travels in this struct.
#[derive(Debug, Clone)]
pub struct ProjectedDispatch {
    /// The signed-in principal (actor + org + scope) for the domain command.
    pub principal: Principal,
    /// The `dispatch_target` key the registry routes on (e.g. `registry.update_equipment`).
    pub target: String,
    /// The projected entity's primary key (equipment id, work-order id, …) — the
    /// domain row the action targets. `None` for a create-style projected action.
    pub target_id: Option<Uuid>,
    /// The caller's idempotency key, forwarded verbatim from `ActionCommand`.
    /// A canonical port REPLAYS a repeat of the same id, so it is the difference
    /// between a retried network call and a second write; the canonical
    /// dispatcher refuses a command without one rather than minting a fresh key
    /// on the caller's behalf, which would silently defeat that replay.
    pub command_id: Option<Uuid>,
    /// The ontology action key (stable key) that accepted the command. Bound
    /// into the canonical port's receipt so a replay can reconstruct the
    /// accepted wrapper metadata.
    pub action_key: String,
    /// The ontology object type that owns the action. `action_key` is unique
    /// only per object type, so this is bound into the receipt alongside it so
    /// a replay can reject a retry that reuses the `command_id` through a
    /// different object type.
    pub object_type_id: Uuid,
    /// Validated action params (the edit values) for the domain command.
    pub params: Value,
    /// Optional caller reason, forwarded to the domain audit trail.
    pub reason: Option<String>,
    /// Deterministic occurrence time for the domain audit event.
    pub occurred_at: OffsetDateTime,
}

/// One projected-dispatch handler: an async adapter that invokes the owning
/// domain use-case. Returns a JSON summary of the domain result (opaque to the
/// engine) or a typed [`ActionError`] (fail-closed).
pub type ProjectedHandler = Arc<
    dyn Fn(ProjectedDispatch) -> Pin<Box<dyn Future<Output = Result<Value, ActionError>> + Send>>
        + Send
        + Sync,
>;

/// The PURE half of a canonical port dispatch: decode a payload, re-check it
/// names the contract's target, bind its subject, and run `P::preflight` — with
/// no execute, no connection, no side effect. `preflight_action` routes the
/// projected dry run here so `would_execute` reflects the port's own refusal,
/// not just the §16 gates and submit criteria.
pub type PortPreflight =
    Arc<dyn Fn(DispatchTarget, Option<Uuid>, Value) -> Result<(), ActionError> + Send + Sync>;

/// Resolves a `dispatch_target` to the domain use-case that owns its write.
///
/// # Why there are two maps, and why only one of them can grow by hand
///
/// A target that `console-ontology-canonical-domain` declares is resolved by
/// DERIVATION: `DispatchTarget::from_str` parses it against the roster and
/// `DispatchTarget::object()` names the [`ObjectKey`] that owns it, so the
/// lookup key is the OBJECT — of which there are exactly six, locked by
/// `six_projected_stable_object_keys_verbatim`. A fourteenth target added to
/// `dispatch_targets!` therefore resolves the moment it exists, with no edit to
/// this crate, to the App tier's wiring, or to any registration list.
/// [`Self::register_port`] is generic over [`CanonicalPort`]: it is called once
/// per OBJECT, never once per action, and it takes no closure.
///
/// `handlers` is the pre-canonical escape hatch for a projected target that is
/// NOT a roster member (`registry.update_equipment`). It is a per-action map and
/// it is exactly the shape this type is moving away from; nothing canonical may
/// be registered there — a roster member never reaches it, because the branch
/// above returns first.
///
/// Both halves own the same fail-closed contract: an **unresolved target is a
/// typed `NotWiredYet` error**, so a mis-seeded or not-yet-wired action can
/// never silently no-op or write.
#[derive(Clone, Default)]
pub struct ProjectedDispatchRegistry {
    handlers: HashMap<String, ProjectedHandler>,
    ports: HashMap<ObjectKey, ProjectedHandler>,
    port_preflights: HashMap<ObjectKey, PortPreflight>,
}

impl ProjectedDispatchRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a handler for one NON-canonical `dispatch_target`. Chainable
    /// builder. A canonical target must go through [`Self::register_port`]:
    /// registering one here is dead wiring, because [`Self::dispatch`] resolves
    /// roster members from the ports map and never falls through to this one.
    #[must_use]
    pub fn register(mut self, target: impl Into<String>, handler: ProjectedHandler) -> Self {
        self.handlers.insert(target.into(), handler);
        self
    }

    /// Install the canonical port that owns one object — and with it EVERY
    /// dispatch target the contract assigns to that object, present and future.
    ///
    /// The port keeps its own transaction and RLS arming: this registry decodes
    /// the payload, checks it against the target, runs the port's PURE preflight
    /// and hands over. It writes no domain table itself (§9.3). Canonical adapters
    /// open raw `pool.begin()` without an `audit_events` row; the engine emits
    /// that row from [`OntologyRestState::execute_action`] after a successful
    /// canonical dispatch (see `emit_canonical_projected_audit`).
    #[must_use]
    pub fn register_port<P>(mut self, port: P) -> Self
    where
        P: CanonicalPort + Send + Sync + 'static,
        P::Query: DeserializeOwned + Send,
        P::Command: Send + 'static,
        // `CanonicalPortError: Send` is on the associated type, but
        // `spawn_blocking` needs the bound named here for Result<_, P::Error>.
        P::Error: Send + 'static,
    {
        self.ports.insert(
            <P::Object as CanonicalObject>::KEY,
            canonical_port_handler(port),
        );
        self.port_preflights.insert(
            <P::Object as CanonicalObject>::KEY,
            Arc::new(move |target, target_id, params| {
                canonical_port_preflight::<P>(target, target_id, params)
            }),
        );
        self
    }

    /// Whether this registry resolves `target` to a port. The coverage question
    /// a test must be able to ask: without it, "every target is wired" is not
    /// observable from outside and the registry can rot back into a partial list
    /// while every test stays green.
    #[must_use]
    pub fn resolves(&self, target: DispatchTarget) -> bool {
        self.ports.contains_key(&target.object())
    }

    /// Run the owning canonical port's PURE preflight for a projected payload,
    /// without dispatching or executing. `preflight_action` routes the projected
    /// dry run here so `would_execute` reflects the port's own verdict, not just
    /// the §16 gates and submit criteria. An unresolved roster target fails
    /// closed (`NotWiredYet`), matching [`Self::dispatch`].
    pub fn preflight(
        &self,
        target: DispatchTarget,
        target_id: Option<Uuid>,
        params: Value,
    ) -> Result<(), ActionError> {
        match self.port_preflights.get(&target.object()) {
            Some(preflight) => preflight(target, target_id, params),
            None => Err(ActionError::NotWiredYet {
                target: Some(target.as_str().to_owned()),
            }),
        }
    }

    /// Route to the owning port (canonical target) or to the per-action handler
    /// (everything else), or fail closed.
    async fn dispatch(&self, input: ProjectedDispatch) -> Result<Value, ActionError> {
        if let Ok(target) = DispatchTarget::from_str(&input.target) {
            return match self.ports.get(&target.object()) {
                Some(handler) => handler(input).await,
                None => Err(ActionError::NotWiredYet {
                    target: Some(input.target),
                }),
            };
        }
        match self.handlers.get(&input.target) {
            Some(handler) => handler(input).await,
            None => Err(ActionError::NotWiredYet {
                target: Some(input.target),
            }),
        }
    }
}

/// Decode a projected payload into the owning port's query, re-check that the
/// decoded query names the contract's target (the fail-closed seam — a port
/// whose serde tags and `target()` ever disagree is refused here rather than
/// writing under the wrong target), and bind the payload subject to the gated
/// `target_id`. Shared by the dispatcher and the preflight path so the two can
/// never disagree on decode, target, or subject binding.
fn decode_canonical_query<P>(
    target: DispatchTarget,
    target_id: Option<Uuid>,
    params: Value,
) -> Result<P::Query, ActionError>
where
    P: CanonicalPort,
    P::Query: DeserializeOwned,
{
    let mut payload = match params {
        Value::Object(map) => map,
        Value::Null => serde_json::Map::new(),
        other => {
            return Err(ActionError::Validation(format!(
                "params must be a JSON object for {}, got {other}",
                target.as_str()
            )));
        }
    };
    // The contract's own spelling of the target, never the caller's: `from_str`
    // already rejected anything else, so a payload cannot choose which variant
    // it decodes to.
    payload.insert(
        "target".to_owned(),
        Value::String(target.as_str().to_owned()),
    );
    let query: P::Query = serde_json::from_value(Value::Object(payload)).map_err(|error| {
        ActionError::Validation(format!(
            "params do not decode as {}: {error}",
            target.as_str()
        ))
    })?;
    if query.dispatch_target() != target {
        return Err(ActionError::Validation(format!(
            "payload decoded as {} but the action names {}",
            query.dispatch_target().as_str(),
            target.as_str()
        )));
    }
    match (target_id, query.subject_id()) {
        (Some(bound), Some(subject)) if bound != subject => {
            return Err(ActionError::Validation(format!(
                "payload subject {subject} does not match the action target_id {bound}"
            )));
        }
        (None, Some(subject)) => {
            return Err(ActionError::Validation(format!(
                "payload subject {subject} requires target_id on the projected action"
            )));
        }
        _ => {}
    }
    Ok(query)
}

/// The PURE preflight verdict of the owning canonical port for a projected
/// payload: decode, target re-check, subject bind, and `P::preflight` — no
/// execute, no connection, no side effect. `P::preflight` is an associated
/// function (`no &self`), so this is free to call on a dry run without spending
/// an approval or opening a transaction.
fn canonical_port_preflight<P>(
    target: DispatchTarget,
    target_id: Option<Uuid>,
    params: Value,
) -> Result<(), ActionError>
where
    P: CanonicalPort,
    P::Query: DeserializeOwned,
{
    let query = decode_canonical_query::<P>(target, target_id, params)?;
    let preflight = P::preflight(&query);
    if !preflight.is_ok() {
        return Err(ActionError::Validation(preflight.blockers().join("; ")));
    }
    Ok(())
}

/// ONE handler for every target a canonical port owns.
///
/// This function is the whole mechanism ADR-0030 §7 row 4 asks for, and it is
/// generic rather than repeated: nothing in it names a target, an object, a
/// param or a use-case. The roster is the only thing that decides which query a
/// payload decodes to, because the query enums are internally tagged on the
/// target string the CONTRACT spells — so a new target is a variant in the
/// owning port, not a closure here.
///
/// The `dispatch_target()` re-check afterwards is not redundant with the serde
/// tag: it is the fail-closed seam. Serde chooses a variant from the tag we
/// inject; `dispatch_target()` reports what the DECODED value actually is. A
/// port whose variant/target mapping ever disagrees with its serde tags is
/// refused here rather than writing under the wrong target.
fn canonical_port_handler<P>(port: P) -> ProjectedHandler
where
    P: CanonicalPort + Send + Sync + 'static,
    P::Query: DeserializeOwned + Send,
    P::Command: Send + 'static,
    P::Error: Send + 'static,
{
    let port = Arc::new(port);
    Arc::new(move |input: ProjectedDispatch| {
        let port = Arc::clone(&port);
        Box::pin(async move {
            // `reason` and `occurred_at` are deliberately not consumed here: a
            // canonical command stamps its own receipt time. The write subject
            // still comes from the payload (`org_unit_id`, `run_id`, …) — but
            // `target_id` is the gated/approved instance id, so when the decoded
            // query names a subject it must agree with that binding (same seam
            // as the `dispatch_target()` re-check below).
            let ProjectedDispatch {
                principal,
                target,
                target_id,
                params,
                command_id,
                action_key,
                object_type_id,
                ..
            } = input;
            let Ok(target) = DispatchTarget::from_str(&target) else {
                // Unreachable through `dispatch`, which parses first. Kept as a
                // typed refusal so a direct caller cannot smuggle a non-roster
                // string into a port.
                return Err(ActionError::NotWiredYet {
                    target: Some(target),
                });
            };
            let command_id = command_id.ok_or_else(|| {
                ActionError::Validation(format!(
                    "{} requires command_id: it is the tenant-global idempotency \
                     key the owning port replays on",
                    target.as_str()
                ))
            })?;

            let query = decode_canonical_query::<P>(target, target_id, params)?;

            // PURE, and run before anything opens a connection.
            let preflight = P::preflight(&query);
            if !preflight.is_ok() {
                return Err(ActionError::Validation(preflight.blockers().join("; ")));
            }

            let command = P::command(
                principal.org_id,
                CommandId::from_uuid(command_id),
                principal.user_id,
                query,
                &action_key,
                object_type_id,
            );
            // `CanonicalPort::execute` is synchronous and blocks on a runtime
            // handle, which panics on a worker thread. The blocking pool is not
            // an async context, so this is the one legal way in — the same
            // bridge every port's own suite uses.
            let receipt = tokio::task::spawn_blocking(move || port.execute(&command))
                .await
                .map_err(|error| {
                    ActionError::domain(KernelError::internal(format!(
                        "canonical port for {} did not complete: {error}",
                        target.as_str()
                    )))
                })?
                .map_err(|error| ActionError::domain(error.into_kernel_error()))?;

            Ok(serde_json::json!({
                "owner": receipt.owner().as_str(),
                "target": receipt.target().as_str(),
                "command_id": receipt.command_id().as_uuid(),
                "result": receipt.result(),
            }))
        })
    })
}

pub const OBJECT_TYPES_PATH: &str = "/api/v1/ontology/object-types";
pub const OBJECT_TYPE_KEY_PATH: &str = "/api/v1/ontology/object-types/{key}";
pub const OBJECT_TYPE_ACTING_PATH: &str = "/api/v1/ontology/object-types/{key}/acting";
pub const OBJECT_TYPE_LIFECYCLE_PATH: &str = "/api/v1/ontology/object-types/{key}/lifecycle";
pub const OBJECT_TYPE_POLICIES_PATH: &str = "/api/v1/ontology/object-types/{key}/policies";
pub const INSTANCES_PATH: &str = "/api/v1/ontology/instances";
pub const INSTANCES_AGGREGATE_PATH: &str = "/api/v1/ontology/instances/aggregate";
pub const INSTANCE_ID_PATH: &str = "/api/v1/ontology/instances/{id}";
pub const INSTANCE_HISTORY_PATH: &str = "/api/v1/ontology/instances/{id}/history";
pub const INSTANCE_TRAVERSE_PATH: &str = "/api/v1/ontology/instances/{id}/traverse";
pub const INSTANCE_LIFECYCLE_PATH: &str = "/api/v1/ontology/instances/{id}/lifecycle";
pub const INSTANCE_ACTING_PATH: &str = "/api/v1/ontology/instances/{id}/acting";
pub const RESOLVE_PATH: &str = "/api/v1/ontology/resolve";
pub const ACTION_PREFLIGHT_PATH: &str = "/api/v1/ontology/actions/{action_key}/preflight";
pub const ACTION_EXECUTE_PATH: &str = "/api/v1/ontology/actions/{action_key}/execute";

pub const ONTOLOGY_ROUTE_PATHS: &[&str] = &[
    OBJECT_TYPES_PATH,
    OBJECT_TYPE_KEY_PATH,
    OBJECT_TYPE_ACTING_PATH,
    OBJECT_TYPE_LIFECYCLE_PATH,
    OBJECT_TYPE_POLICIES_PATH,
    INSTANCES_PATH,
    INSTANCES_AGGREGATE_PATH,
    INSTANCE_ID_PATH,
    INSTANCE_HISTORY_PATH,
    INSTANCE_TRAVERSE_PATH,
    INSTANCE_LIFECYCLE_PATH,
    INSTANCE_ACTING_PATH,
    RESOLVE_PATH,
    ACTION_PREFLIGHT_PATH,
    ACTION_EXECUTE_PATH,
];

pub fn router(state: OntologyRestState) -> Router {
    let verifier = state.jwt_verifier.clone();
    let pool = state.registry.pool().clone();
    let router = Router::new()
        .route(
            OBJECT_TYPES_PATH,
            get(list_object_types).post(create_object_type),
        )
        .route(
            OBJECT_TYPE_KEY_PATH,
            get(get_object_type).put(stage_object_type_revision),
        )
        .route(OBJECT_TYPE_ACTING_PATH, get(object_type_acting))
        .route(
            OBJECT_TYPE_LIFECYCLE_PATH,
            post(transition_object_type_lifecycle),
        )
        .route(OBJECT_TYPE_POLICIES_PATH, post(attach_object_policy))
        .route(INSTANCES_PATH, get(list_instances))
        // Static `/aggregate` must be registered before `/{id}` so "aggregate"
        // is never captured as an instance id.
        .route(INSTANCES_AGGREGATE_PATH, get(aggregate_instances))
        .route(INSTANCE_ID_PATH, get(get_instance))
        .route(INSTANCE_HISTORY_PATH, get(get_instance_history))
        .route(INSTANCE_TRAVERSE_PATH, get(traverse_instance))
        .route(INSTANCE_LIFECYCLE_PATH, post(commit_lifecycle))
        .route(INSTANCE_ACTING_PATH, get(instance_acting))
        .route(RESOLVE_PATH, get(resolve_code))
        .route(ACTION_PREFLIGHT_PATH, post(action_preflight))
        .route(ACTION_EXECUTE_PATH, post(action_execute))
        .with_state(state);
    console_platform_request_context::with_request_context(router, verifier, pool)
}

// ---------------------------------------------------------------------------
// Registry surface (thin over PgOntologyStore)
// ---------------------------------------------------------------------------

async fn list_object_types(
    State(state): State<OntologyRestState>,
    headers: HeaderMap,
) -> Result<Json<Vec<ObjectTypeSummary>>, RestError> {
    authorize_ontology(&state, &headers).await?;
    let types = state
        .registry
        .list_object_types()
        .await
        .map_err(RestError::from_ontology)?;
    Ok(Json(types))
}

fn object_type_response<T: Serialize>(
    status: StatusCode,
    value: T,
    write_version: &ObjectTypeWriteVersion,
) -> Result<Response, RestError> {
    let etag = axum::http::HeaderValue::from_str(&write_version.etag)
        .map_err(|_| RestError::internal("invalid ontology write validator"))?;
    let mut response = (status, Json(value)).into_response();
    response
        .headers_mut()
        .insert(axum::http::header::ETAG, etag);
    Ok(response)
}

fn required_object_type_write_precondition(
    headers: &HeaderMap,
) -> Result<ObjectTypeWritePrecondition, RestError> {
    let mut values = headers.get_all(axum::http::header::IF_MATCH).iter();
    let raw = values
        .next()
        .ok_or_else(RestError::write_precondition_required)?;
    if values.next().is_some() {
        return Err(RestError::invalid_write_precondition());
    }
    let raw = raw
        .to_str()
        .map_err(|_| RestError::invalid_write_precondition())?;
    if raw.starts_with("W/") || raw == "*" || raw.contains(',') {
        return Err(RestError::invalid_write_precondition());
    }
    let inner = raw
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
        .ok_or_else(RestError::invalid_write_precondition)?;
    let payload = inner
        .strip_prefix("ont-object-type-key:")
        .ok_or_else(RestError::invalid_write_precondition)?;
    let (validator, revision) = payload
        .split_once(":r")
        .ok_or_else(RestError::invalid_write_precondition)?;
    if validator.len() != 32
        || !validator
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        || revision.is_empty()
        || (revision.len() > 1 && revision.starts_with('0'))
        || !revision.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err(RestError::invalid_write_precondition());
    }
    let validator_id =
        Uuid::parse_str(validator).map_err(|_| RestError::invalid_write_precondition())?;
    let revision = revision
        .parse::<i64>()
        .ok()
        .filter(|revision| *revision >= 1)
        .ok_or_else(RestError::invalid_write_precondition)?;
    Ok(ObjectTypeWritePrecondition {
        validator_id,
        revision,
    })
}

async fn create_object_type(
    State(state): State<OntologyRestState>,
    headers: HeaderMap,
    Json(draft): Json<CreateObjectTypeDraft>,
) -> Result<impl IntoResponse, RestError> {
    let principal = authorize_ontology(&state, &headers).await?;
    let summary = state
        .registry
        .create_object_type(
            principal.user_id,
            draft,
            TraceContext::generate(),
            OffsetDateTime::now_utc(),
        )
        .await
        .map_err(RestError::from_ontology)?;
    let write_version = summary.write_version();
    object_type_response(StatusCode::CREATED, summary, &write_version)
}

#[derive(Debug, Deserialize)]
struct ObjectTypeVersionQuery {
    #[serde(default)]
    version: Option<i64>,
}

async fn get_object_type(
    State(state): State<OntologyRestState>,
    headers: HeaderMap,
    Path(key): Path<String>,
    Query(query): Query<ObjectTypeVersionQuery>,
) -> Result<Response, RestError> {
    authorize_ontology(&state, &headers).await?;
    let detail = state
        .registry
        .get_object_type(&key, query.version)
        .await
        .map_err(RestError::from_ontology)?;
    let write_version = detail.object_type.write_version();
    object_type_response(StatusCode::OK, detail, &write_version)
}

async fn stage_object_type_revision(
    State(state): State<OntologyRestState>,
    headers: HeaderMap,
    Path(key): Path<String>,
    Json(draft): Json<CreateObjectTypeDraft>,
) -> Result<impl IntoResponse, RestError> {
    let principal = authorize_ontology(&state, &headers).await?;
    let expected = required_object_type_write_precondition(&headers)?;
    let summary = state
        .registry
        .stage_revision(
            principal.user_id,
            &key,
            expected,
            draft,
            TraceContext::generate(),
            OffsetDateTime::now_utc(),
        )
        .await
        .map_err(RestError::from_ontology)?;
    let write_version = summary.write_version();
    object_type_response(StatusCode::CREATED, summary, &write_version)
}

#[derive(Debug, Deserialize)]
struct ObjectTypeLifecycleRequest {
    to_state: SchemaLifecycleState,
}

/// Drive the object-type schema FSM. The legal edge set lives in SQL
/// (`0165_ontology_object_type_key_revisions.sql:1015-1022`) and the
/// approval-consuming `review_pending -> published` branch is enforced there
/// too, so this handler holds NO transition table: every rejected edge arrives
/// as a mapped `PgOntologyError` and every new state costs zero rest-crate code.
async fn transition_object_type_lifecycle(
    State(state): State<OntologyRestState>,
    headers: HeaderMap,
    Path(key): Path<String>,
    Query(query): Query<ObjectTypeVersionQuery>,
    Json(body): Json<ObjectTypeLifecycleRequest>,
) -> Result<Response, RestError> {
    let principal = authorize_ontology(&state, &headers).await?;
    let expected = required_object_type_write_precondition(&headers)?;
    // `transition_lifecycle` takes a VERSION id, and `get_object_type(key, None)`
    // returns the published-preferred head — so once v1 is published, a key-only
    // call would address v1 forever. `?version=` is how a staged v2 is reachable.
    let detail = state
        .registry
        .get_object_type(&key, query.version)
        .await
        .map_err(RestError::from_ontology)?;
    let summary = state
        .registry
        .transition_lifecycle(
            principal.user_id,
            detail.object_type.id,
            expected,
            body.to_state,
            true,
            TraceContext::generate(),
            OffsetDateTime::now_utc(),
        )
        .await
        .map_err(RestError::from_ontology)?;
    let write_version = summary.write_version();
    object_type_response(StatusCode::OK, summary, &write_version)
}

// ---------------------------------------------------------------------------
// Object-policy attachment (arch §5d authoring → the row-visibility residual)
// ---------------------------------------------------------------------------

/// The wire body carries the rule and NOTHING that identifies what it applies to.
///
/// `resource_type` is derived from the path `{key}` and `action` is fixed to
/// [`authoring::OBJECT_POLICY_ACTION`] on purpose: a persisted policy whose
/// `resource_type` disagrees with the object type's `stable_key` never matches
/// `applicable_object_policies`, so it denies forever at HTTP 200 `[]` — a
/// silent failure no post-hoc test can see. Deriving both makes it
/// unrepresentable, and is less code than accepting them.
#[derive(Debug, Deserialize)]
struct AttachObjectPolicyRequest {
    effect: Effect,
    #[serde(default)]
    conditions: Vec<Condition>,
}

#[derive(Debug, Serialize)]
struct AttachObjectPolicyResponse {
    id: Uuid,
}

/// The most AND-ed conditions one attached policy may PERSIST.
///
/// The authoring grammar bounds neither the count nor the literal lengths
/// (`authoring.rs:306-341`), and an attachment can never be revoked: both tables
/// are append-only (`0154:90-99`) and no role anywhere holds UPDATE on
/// `cedar_policy_catalog_entries` (`0150:118` revokes it from `console_rt`,
/// `0205:151` grants the writer only SELECT and INSERT), so a row can never
/// leave the `status = 'enforced'` the loader selects on (`store.rs:558`).
/// An oversized list is therefore permanent work charged to every later read of
/// the type — re-validated, re-rendered, re-normalized and lowered into the SQL
/// residual on the list, on all five single-instance paths, and once per
/// node-type group inside a traversal.
///
/// 32 is far above any authorable rule: a condition's left side comes from
/// `RESOURCE_ATTRS` (4), `SUBJECT_ATTRS` (4) or the type's declared Text/Boolean
/// properties. The bound is applied HERE and not in the validator on purpose —
/// tightening the validator would retroactively invalidate any already-enforced
/// row and 500 every read of its type, which is the failure this bound exists to
/// prevent.
const MAX_ATTACHED_CONDITIONS: usize = 32;

/// Author one enforced object policy and attach it to an object type.
///
/// This route asserts the authoring validator's verdict; it never re-encodes the
/// rule. A condition the validator cannot represent — for instance one over a
/// Date property, which [`declared_attrs`] cannot splice into the Cedar schema —
/// is refused here with the validator's own message rather than landing enforced
/// and 500-ing every later read of the type.
async fn attach_object_policy(
    State(state): State<OntologyRestState>,
    headers: HeaderMap,
    Path(key): Path<String>,
    Json(body): Json<AttachObjectPolicyRequest>,
) -> Result<(StatusCode, Json<AttachObjectPolicyResponse>), RestError> {
    let principal = authorize_ontology(&state, &headers).await?;
    if body.conditions.len() > MAX_ATTACHED_CONDITIONS {
        return Err(RestError::from_kernel(KernelError::validation(format!(
            "a policy may carry at most {MAX_ATTACHED_CONDITIONS} conditions, got {}",
            body.conditions.len()
        ))));
    }
    let detail = state
        .registry
        .get_object_type(&key, None)
        .await
        .map_err(RestError::from_ontology)?;
    let blocks = NoCodeBlocks {
        effect: body.effect,
        action: authoring::OBJECT_POLICY_ACTION.to_owned(),
        resource_type: detail.object_type.stable_key.clone(),
        conditions: body.conditions,
    };
    // `stable_key`, `title` and `natural_language_rule` are NOT sent: all three
    // are functions of the object type, and a value the definer derives for
    // itself is a value a hand-crafted call to it cannot forge (0205 §4).
    let id = state
        .policies
        .attach_object_policy(AttachObjectPolicyCommand {
            actor: principal.user_id,
            object_type_id: *detail.object_type.id.as_uuid(),
            declared: declared_attrs(&detail.properties),
            blocks,
        })
        .await
        .map_err(RestError::from_cedar)?;
    Ok((StatusCode::CREATED, Json(AttachObjectPolicyResponse { id })))
}

// ---------------------------------------------------------------------------
// Instance surface (thin over PgInstanceStore)
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct InstanceListQuery {
    /// Object-type VERSION id (0105 head) whose current-state instances to list.
    r#type: Uuid,
}

async fn list_instances(
    State(state): State<OntologyRestState>,
    headers: HeaderMap,
    Query(query): Query<InstanceListQuery>,
) -> Result<Json<Vec<InstanceState>>, RestError> {
    let principal = authorize_ontology(&state, &headers).await?;
    let policies = object_view_policies(&state, query.r#type)
        .await
        .map_err(RestError::from_ontology)?;
    let subject = ontology_subject(&principal);
    let list = state
        .instances
        .list_instances_filtered(ObjectTypeId::from_uuid(query.r#type), &subject, &policies)
        .await
        .map_err(RestError::from_ontology)?;
    Ok(Json(list))
}

#[derive(Debug, Deserialize)]
struct InstanceAggregateQuery {
    /// Object-type VERSION id whose current-state instances to aggregate.
    r#type: Uuid,
    /// Allowlisted group key: `lifecycle_state`, `object_type_id`, or `attribute`.
    group_by: String,
    /// Required when `group_by=attribute`; must be a declared property key.
    #[serde(default)]
    attribute_key: Option<String>,
}

async fn aggregate_instances(
    State(state): State<OntologyRestState>,
    headers: HeaderMap,
    Query(query): Query<InstanceAggregateQuery>,
) -> Result<Json<Vec<AggregateBucket>>, RestError> {
    let principal = authorize_ontology(&state, &headers).await?;
    let object_type_id = ObjectTypeId::from_uuid(query.r#type);
    let group_by = resolve_aggregate_group_by(&state, object_type_id, &query).await?;
    let policies = object_view_policies(&state, query.r#type)
        .await
        .map_err(RestError::from_ontology)?;
    let subject = ontology_subject(&principal);
    let buckets = state
        .instances
        .aggregate_instances(object_type_id, group_by, &subject, &policies)
        .await
        .map_err(RestError::from_ontology)?;
    Ok(Json(buckets))
}

async fn resolve_aggregate_group_by(
    state: &OntologyRestState,
    object_type_id: ObjectTypeId,
    query: &InstanceAggregateQuery,
) -> Result<AggregateGroupBy, RestError> {
    match query.group_by.as_str() {
        "lifecycle_state" => Ok(AggregateGroupBy::LifecycleState),
        "object_type_id" => Ok(AggregateGroupBy::ObjectTypeId),
        "attribute" => {
            let key = query
                .attribute_key
                .as_deref()
                .map(str::trim)
                .filter(|k| !k.is_empty())
                .ok_or_else(|| {
                    RestError::from_kernel(KernelError::validation(
                        "attribute_key is required when group_by=attribute",
                    ))
                })?;
            let (stable_key, schema_version) = state
                .registry
                .object_type_version(object_type_id)
                .await
                .map_err(RestError::from_ontology)?;
            let declared = state
                .registry
                .get_object_type(&stable_key, Some(schema_version))
                .await
                .map_err(RestError::from_ontology)?;
            if !declared.properties.iter().any(|p| p.key == key) {
                return Err(RestError::from_kernel(KernelError::validation(format!(
                    "attribute_key {key:?} is not a declared property of this object type"
                ))));
            }
            Ok(AggregateGroupBy::Attribute(key.to_owned()))
        }
        other => Err(RestError::from_kernel(KernelError::validation(format!(
            "unsupported group_by {other:?}; expected lifecycle_state, object_type_id, or attribute"
        )))),
    }
}

/// The object type's own declared properties, which are admissible in policies
/// scoped to it. The residual lowering already reads arbitrary instance
/// attributes; supplying the declared set lets the authoring validator agree,
/// with each property spliced into the Cedar schema so reads stay proven.
/// Only Text and Boolean are representable as optional Cedar attributes; a
/// policy over any other kind fails closed in the validator.
///
/// Attach time and read time MUST derive this from the same function: the
/// loader re-validates every enforced row against the declared set and hard-
/// errors on disagreement, so two spellings would 500 the type forever.
///
/// CEILING (escalated, not fixed here): this is a point-in-time claim. A
/// property that is `Text` at attach and later revised to `Date` turns a valid
/// enforced row into a permanent 500 across every read path that shares
/// [`object_view_policies`]. The durable fix is revision-time re-validation.
fn declared_attrs(properties: &[PropertyDefSummary]) -> Vec<DeclaredAttr> {
    properties
        .iter()
        .filter_map(|property| match property.field_kind {
            FieldKind::Boolean => Some(DeclaredAttr {
                key: property.key.clone(),
                boolean: true,
            }),
            FieldKind::Text => Some(DeclaredAttr {
                key: property.key.clone(),
                boolean: false,
            }),
            _ => None,
        })
        .collect()
}

/// The enforced `view` object policies attached to one object-type VERSION id.
///
/// An unknown / cross-tenant version id is a 404 here, never an empty policy set
/// — an unresolvable type must not be mistaken for an unpoliced one.
///
/// The version is resolved BY ITS OWN ID through
/// [`PgOntologyStore::object_type_version`], never through the registry head;
/// that function's doc carries the reason, and it is the only reason it exists.
///
/// `declared` likewise comes from the instance's OWN version, not the head: the
/// loader re-validates each enforced row against it, and a policy authored over
/// a property the next version dropped would otherwise 500 forever.
/// Errors on [`PgOntologyError`], not [`RestError`], because BOTH the read
/// channel (`RestError::from_ontology`) and the action channel
/// (`ActionError::Store`) already consume it. That is what lets ONE gate serve
/// the read routes and the write routes without a second error currency — and
/// `RestError` is private, so an `ActionError` variant holding one is E0446.
async fn object_view_policies(
    state: &OntologyRestState,
    object_type_id: Uuid,
) -> Result<Vec<ObjectPolicy>, PgOntologyError> {
    let (stable_key, schema_version) = state
        .registry
        .object_type_version(ObjectTypeId::from_uuid(object_type_id))
        .await?;
    let declared = declared_attrs(
        &state
            .registry
            .get_object_type(&stable_key, Some(schema_version))
            .await?
            .properties,
    );
    let blocks = state
        .policies
        .load_enforced_object_policy_blocks(object_type_id, &declared)
        .await
        .map_err(|error| {
            tracing::error!(%error, "ontology object-policy load failed");
            // Renders 500 / "internal" through `status_for_error_kind`, byte-identical
            // to the `RestError::internal` this replaced.
            PgOntologyError::Domain(KernelError::internal(
                "unable to evaluate object visibility policy",
            ))
        })?;
    Ok(applicable_object_policies(&blocks, &stable_key))
}

/// The gate, and the only module in this crate that can reach an ungated
/// single-row read.
///
/// [`PgInstanceStore::get_current`] serves any row the RLS org floor admits and
/// applies NO object-policy residual, so every call to it outside
/// [`visible_head_inner`] is a policy bypass. This module makes that
/// unrepresentable rather than merely reviewed: [`Instances`] wraps the store in
/// a private tuple field, and `lib.rs` is this module's PARENT, which cannot
/// reach a child's private field.
///
/// Wrapping alone is NOT the property, and claiming it was is how this hole
/// nearly shipped: `get_as_of`, `history` and `traverse` apply no residual
/// either, so re-exporting them by `InstanceId` would have let a new route read
/// every revision of a hidden row with the seal fully intact. Each of them takes
/// PROOF instead of an id:
///
///   * [`Instances::get_as_of`] and [`Instances::history`] take [`Visible`],
///     whose field is private to this module, so [`visible_head_inner`] is the
///     only thing in the crate that can produce one — `InstanceState` itself is
///     all-`pub` and a struct literal would forge it. The id is read off the
///     proof, so it cannot even be pointed at a different row than the one gated.
///   * `traverse` is not re-exported at all. [`visible_traversal`] is the whole
///     route body, so the raw graph never exists outside this module.
///
/// WHAT THIS SEAL DOES AND DOES NOT COVER — stated precisely, because an
/// earlier version of this comment claimed more than the code delivers and a
/// reviewer refuted it by execution.
///
/// It DOES make the specific hole that produced this slice unrepresentable:
/// `get_current` has exactly one call site and it is inside this module, so a
/// future route cannot fetch a single row's head unfiltered no matter how it is
/// classified in the route table.
///
/// It does NOT make every conceivable ungated read impossible. The passthroughs
/// above are re-exported, and a sufficiently determined new handler can compose
/// them into a read this module never anticipated. `NotInstanceBearing` in the
/// route-classification table therefore remains a DECLARATIVE label with no
/// enforcement behind it: a future route mislabelled there ships green. That is
/// a known, named gap, not a covered one — the honest compensating control is
/// that `Gated` IS load-bearing and is proven red by mutation, not that
/// mislabelling has been made harmless.
mod gate {
    use super::*;

    #[derive(Clone)]
    pub(super) struct Instances(PgInstanceStore);

    /// One instance head the object-policy residual admitted for this principal.
    ///
    /// The field is private to `gate`, which is the entire point: a value of this
    /// type is evidence that [`visible_head_inner`] ran, and no other module can
    /// manufacture one.
    pub(super) struct Visible(InstanceState);

    impl Visible {
        pub(super) fn into_inner(self) -> InstanceState {
            self.0
        }
    }

    impl Instances {
        pub(super) fn new(inner: PgInstanceStore) -> Self {
            Self(inner)
        }

        pub(super) async fn list_instances_filtered(
            &self,
            object_type_id: ObjectTypeId,
            subject: &SubjectAttrs,
            policies: &[ObjectPolicy],
        ) -> Result<Vec<InstanceState>, PgOntologyError> {
            self.0
                .list_instances_filtered(object_type_id, subject, policies)
                .await
        }

        pub(super) async fn aggregate_instances(
            &self,
            object_type_id: ObjectTypeId,
            group_by: AggregateGroupBy,
            subject: &SubjectAttrs,
            policies: &[ObjectPolicy],
        ) -> Result<Vec<AggregateBucket>, PgOntologyError> {
            self.0
                .aggregate_instances(object_type_id, group_by, subject, policies)
                .await
        }

        pub(super) async fn visible_instances(
            &self,
            object_type_id: ObjectTypeId,
            ids: Option<&[Uuid]>,
            subject: &SubjectAttrs,
            policies: &[ObjectPolicy],
        ) -> Result<Vec<InstanceState>, PgOntologyError> {
            self.0
                .visible_instances(object_type_id, ids, subject, policies)
                .await
        }

        /// An earlier revision of a row whose HEAD the caller has already been
        /// granted. `head` is the proof — only [`visible_head_inner`] mints one
        /// — and the id is read off it, so this cannot be pointed at a row the
        /// residual never admitted.
        pub(super) async fn get_as_of(
            &self,
            head: &Visible,
            at: OffsetDateTime,
        ) -> Result<InstanceState, PgOntologyError> {
            self.0.get_as_of(head.0.instance.id, at).await
        }

        /// The revision chain of a row whose HEAD the caller has already been
        /// granted, INTACT. Never filtered per revision: `verify_chain` breaks on
        /// the first `prev_hash` gap, so a per-revision security filter would
        /// masquerade as a tamper alarm.
        pub(super) async fn history(
            &self,
            head: &Visible,
        ) -> Result<Vec<RevisionSummary>, PgOntologyError> {
            self.0.history(head.0.instance.id).await
        }
    }

    /// THE gate. Every path that serves, evaluates or mutates a single instance
    /// routes through here — the read routes via [`visible_head`], the action and
    /// lifecycle routes directly, because they carry the [`ActionError`] channel.
    /// One helper, one residual: a second gating path would let the two diverge.
    ///
    /// Returns the row the FILTER produced, never the unfiltered head, so what a
    /// caller is served is by construction what the residual permitted — a gate
    /// that read twice could let the two reads diverge.
    ///
    /// A denied row is `not_found` with the adapter's own message, byte-identical
    /// to `load_current_state_tx`'s, so a denied row and a nonexistent one are
    /// indistinguishable. **404, never 403**: a 403 turns the status code into an
    /// existence oracle.
    ///
    /// The object type is resolved from the INSTANCE'S OWN `object_type_id`, never
    /// from a caller-supplied one, so naming a type you can see while targeting an
    /// instance of a type you cannot is not a bypass.
    pub(super) async fn visible_head_inner(
        state: &OntologyRestState,
        principal: &Principal,
        id: Uuid,
    ) -> Result<Visible, PgOntologyError> {
        let head = state
            .instances
            .0
            .get_current(InstanceId::from_uuid(id))
            .await?;
        let object_type_id = head.instance.object_type_id;
        let policies = object_view_policies(state, *object_type_id.as_uuid()).await?;
        let subject = ontology_subject(principal);
        state
            .instances
            .visible_instances(object_type_id, Some(&[id]), &subject, &policies)
            .await?
            .into_iter()
            .next()
            .map(Visible)
            .ok_or_else(|| {
                PgOntologyError::Domain(KernelError::not_found("instance was not found"))
            })
    }

    pub(super) async fn visible_head(
        state: &OntologyRestState,
        principal: &Principal,
        id: Uuid,
    ) -> Result<Visible, RestError> {
        visible_head_inner(state, principal, id)
            .await
            .map_err(RestError::from_ontology)
    }

    /// Walk the graph and filter it in ONE call, so the unfiltered
    /// [`TraversalGraph`] never leaves this module. [`Instances`] deliberately
    /// does not re-export `traverse`: a raw walk in the parent module would be an
    /// ungated read of every neighbour's id, type, title and lifecycle state.
    pub(super) async fn visible_traversal(
        state: &OntologyRestState,
        principal: &Principal,
        root: InstanceId,
        link_type_id: Option<LinkTypeId>,
        depth: u32,
    ) -> Result<TraversalGraph, RestError> {
        let graph = state
            .instances
            .0
            .traverse(root, link_type_id, depth)
            .await
            .map_err(RestError::from_ontology)?;
        visible_subgraph(state, principal, graph).await
    }

    /// Drop from a traversal every node the object-policy residual does not admit,
    /// every edge touching one, and everything the root can no longer reach.
    ///
    /// Gating only the root measurably disclosed both hidden neighbours' titles at
    /// depth 1. Nodes are grouped by `object_type_id` because a neighbour may be of
    /// a different type than the root — including a built-in catalog type — and each
    /// type carries its own attached policies.
    async fn visible_subgraph(
        state: &OntologyRestState,
        principal: &Principal,
        graph: TraversalGraph,
    ) -> Result<TraversalGraph, RestError> {
        let subject = ontology_subject(principal);
        let mut by_type: HashMap<ObjectTypeId, Vec<Uuid>> = HashMap::new();
        for node in &graph.nodes {
            by_type
                .entry(node.object_type_id)
                .or_default()
                .push(*node.instance_id.as_uuid());
        }
        let mut visible: HashSet<Uuid> = HashSet::new();
        for (object_type_id, ids) in by_type {
            let policies = object_view_policies(state, *object_type_id.as_uuid())
                .await
                .map_err(RestError::from_ontology)?;
            let rows = state
                .instances
                .visible_instances(object_type_id, Some(&ids), &subject, &policies)
                .await
                .map_err(RestError::from_ontology)?;
            visible.extend(rows.iter().map(|row| *row.instance.id.as_uuid()));
        }

        let root = *graph.root.as_uuid();
        if !visible.contains(&root) {
            return Err(RestError::from_kernel(KernelError::not_found(
                "instance was not found",
            )));
        }
        let edges: Vec<_> = graph
            .edges
            .into_iter()
            .filter(|edge| {
                visible.contains(edge.from_instance_id.as_uuid())
                    && visible.contains(edge.to_instance_id.as_uuid())
            })
            .collect();

        // Re-BFS from the root over what survives: a node whose only path ran
        // through a hidden one is no longer reachable, and its recorded depth would
        // otherwise disclose the length of that hidden path.
        //
        // FIFO, not LIFO. `depth` is the SHORTEST hop count — `PgInstanceStore::
        // traverse` computes it level by level — and the `Vacant` guard below writes
        // each node's depth exactly once, so the first visit has to be the shortest
        // one. A stack visits the long arm of a diamond first, records its length,
        // and then declines to lower it: measured `far` at 3 when it was 2 hops
        // away, with nothing hidden at all.
        let mut depths: HashMap<Uuid, u32> = HashMap::from([(root, 0)]);
        let mut frontier = VecDeque::from([root]);
        while let Some(current) = frontier.pop_front() {
            let depth = depths[&current];
            for edge in &edges {
                if *edge.from_instance_id.as_uuid() == current {
                    let next = *edge.to_instance_id.as_uuid();
                    if let Entry::Vacant(slot) = depths.entry(next) {
                        slot.insert(depth + 1);
                        frontier.push_back(next);
                    }
                }
            }
        }
        let mut nodes: Vec<_> = graph
            .nodes
            .into_iter()
            .filter_map(|node| {
                depths
                    .get(node.instance_id.as_uuid())
                    .map(|depth| TraversalNode {
                        depth: *depth,
                        ..node
                    })
            })
            .collect();
        // `traverse` hands its nodes over sorted by `(depth, id)`; a recomputed
        // depth can violate that, so restore the order the response contract states.
        nodes.sort_by_key(|node| (node.depth, *node.instance_id.as_uuid()));
        let edges = edges
            .into_iter()
            .filter(|edge| {
                depths.contains_key(edge.from_instance_id.as_uuid())
                    && depths.contains_key(edge.to_instance_id.as_uuid())
            })
            .collect();
        Ok(TraversalGraph {
            root: graph.root,
            nodes,
            edges,
        })
    }
}

use gate::{visible_head, visible_head_inner, visible_traversal};

/// Convert only the already-validated no-code row-policy subset into the SQL
/// residual grammar. A condition unsupported by residual lowering is retained
/// as an intentionally untranslatable predicate, making the adapter return
/// `WHERE FALSE`; it can never silently widen a list.
fn applicable_object_policies(blocks: &[NoCodeBlocks], stable_key: &str) -> Vec<ObjectPolicy> {
    blocks
        .iter()
        .filter(|block| block.action == "view" && block.resource_type == stable_key)
        .map(|block| ObjectPolicy {
            effect: block.effect,
            predicates: block.conditions.iter().map(residual_predicate).collect(),
        })
        .collect()
}

fn residual_predicate(
    condition: &console_platform_authz::cedar_pbac::authoring::Condition,
) -> Predicate {
    let op = match condition.op {
        ConditionOp::Eq => ResidualOp::Eq,
        ConditionOp::Ne => ResidualOp::Ne,
        // `contains` has no row-field equivalent. Use a deliberately missing
        // subject attribute so lowering fails closed for the whole request.
        ConditionOp::Contains => ResidualOp::In,
    };
    let value = match (&condition.op, &condition.value) {
        (ConditionOp::Contains, _) => {
            PredicateValue::SubjectAttr("__unsupported_contains__".to_owned())
        }
        (_, ConditionValue::Literal(value)) => {
            PredicateValue::Literal(SqlValue::Text(value.clone()))
        }
        (_, ConditionValue::Bool(value)) => PredicateValue::Literal(SqlValue::Bool(*value)),
        (_, ConditionValue::SubjectAttr(value)) => PredicateValue::SubjectAttr(value.clone()),
    };
    Predicate {
        field: condition.attr.clone(),
        op,
        value,
    }
}

fn ontology_subject(principal: &Principal) -> SubjectAttrs {
    SubjectAttrs::default()
        .with_scalar("user_id", principal.user_id.to_string())
        .with_scalar("org", principal.org_id.to_string())
        .with_set(
            "roles",
            principal
                .roles
                .iter()
                .map(|role| role.as_str().to_owned())
                .collect(),
        )
}

#[derive(Debug, Deserialize)]
struct AsOfQuery {
    /// RFC3339 instant for a bi-temporal as-of read; absent = current head.
    #[serde(default, with = "time::serde::rfc3339::option")]
    as_of: Option<OffsetDateTime>,
}

async fn get_instance(
    State(state): State<OntologyRestState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Query(query): Query<AsOfQuery>,
) -> Result<Json<InstanceState>, RestError> {
    let principal = authorize_ontology(&state, &headers).await?;
    // The HEAD is what the gate decides on, including for an as-of read:
    // gating the as-of revision instead would be a time-travel bypass — a
    // caller could read a hidden row at an instant before the attribute the
    // policy matches on took its current value.
    let head = visible_head(&state, &principal, id).await?;
    match query.as_of {
        Some(at) => Ok(Json(
            state
                .instances
                .get_as_of(&head, at)
                .await
                .map_err(RestError::from_ontology)?,
        )),
        None => Ok(Json(head.into_inner())),
    }
}

async fn get_instance_history(
    State(state): State<OntologyRestState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Json<Vec<RevisionSummary>>, RestError> {
    let principal = authorize_ontology(&state, &headers).await?;
    // The gated head is the ARGUMENT, not a discarded precondition: `history`
    // takes proof, so this cannot decay into an ungated read by deleting a line.
    let head = visible_head(&state, &principal, id).await?;
    let history = state
        .instances
        .history(&head)
        .await
        .map_err(RestError::from_ontology)?;
    Ok(Json(history))
}

#[derive(Debug, Deserialize)]
struct TraverseQuery {
    #[serde(default)]
    link_type: Option<Uuid>,
    #[serde(default = "default_depth")]
    depth: u32,
}

const fn default_depth() -> u32 {
    2
}

async fn traverse_instance(
    State(state): State<OntologyRestState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Query(query): Query<TraverseQuery>,
) -> Result<Json<TraversalGraph>, RestError> {
    let principal = authorize_ontology(&state, &headers).await?;
    // The root is gated by `visible_traversal` along with every other node, from
    // ONE residual evaluation. A separate pre-gate would be a second read of the
    // same fact, free to diverge from the one that decides the node set. The walk
    // and the filter are ONE call because the unfiltered graph must not be
    // nameable here: `traverse` is not re-exported from `gate` at all.
    Ok(Json(
        visible_traversal(
            &state,
            &principal,
            InstanceId::from_uuid(id),
            query.link_type.map(LinkTypeId::from_uuid),
            query.depth,
        )
        .await?,
    ))
}

// ---------------------------------------------------------------------------
// Action preflight / execute (§2 single mutation path, §16 gate chain)
// ---------------------------------------------------------------------------

/// Typed action command (HTTP-independent) shared by preflight + execute, so the
/// same single mutation path is drivable from a test / automation caller without
/// a live HTTP request. `object_type_id` disambiguates the `action_key` (an action
/// key is unique only per object type); the target is `instance_id` for an edit,
/// or absent for a create (which then needs a `title`).
#[derive(Debug, Clone)]
pub struct ActionCommand {
    pub object_type_id: ObjectTypeId,
    pub instance_id: Option<InstanceId>,
    pub title: Option<String>,
    pub params: Value,
    pub reason: Option<String>,
    pub valid_from: Option<OffsetDateTime>,
    /// Client-supplied self-checklist acknowledgement (there is no checklist
    /// object store yet; §16 gate 2 reads this witness, fail-closed when absent).
    pub checklist_all_acknowledged: Option<bool>,
    /// Four-eyes request ref; its decision is read from the DB, never trusted
    /// from the caller.
    pub four_eyes_request_ref: Option<Uuid>,
    /// Stable client id reused for a network retry of this command.
    pub command_id: Option<Uuid>,
    /// Required current revision for an instance edit (create has no prior head).
    pub expected_revision: Option<i64>,
}

/// The HTTP body for both preflight and execute (JSON with bare UUIDs); converted
/// to a typed [`ActionCommand`] before it touches the orchestration.
#[derive(Debug, Deserialize)]
struct ActionRequest {
    object_type_id: Uuid,
    #[serde(default)]
    instance_id: Option<Uuid>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    params: Value,
    #[serde(default)]
    reason: Option<String>,
    #[serde(default, with = "time::serde::rfc3339::option")]
    valid_from: Option<OffsetDateTime>,
    #[serde(default)]
    checklist_all_acknowledged: Option<bool>,
    #[serde(default)]
    four_eyes_request_ref: Option<Uuid>,
    #[serde(default)]
    command_id: Option<Uuid>,
    #[serde(default)]
    expected_revision: Option<i64>,
}

impl ActionRequest {
    fn into_command(self) -> ActionCommand {
        ActionCommand {
            object_type_id: ObjectTypeId::from_uuid(self.object_type_id),
            instance_id: self.instance_id.map(InstanceId::from_uuid),
            title: self.title,
            params: self.params,
            reason: self.reason,
            valid_from: self.valid_from,
            checklist_all_acknowledged: self.checklist_all_acknowledged,
            four_eyes_request_ref: self.four_eyes_request_ref,
            command_id: self.command_id,
            expected_revision: self.expected_revision,
        }
    }
}

/// Outcome of a preflight — each gate's status plus whether submit criteria hold,
/// without committing anything.
#[derive(Debug, Clone, Serialize)]
pub struct PreflightOutcome {
    pub dispatch: ActionDispatch,
    pub dispatch_target: Option<String>,
    pub config: GateChainConfig,
    pub gates: GateChainOutcome,
    pub criteria_ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub criteria_error: Option<String>,
    /// Would `execute` proceed? (gates allow AND criteria hold).
    pub would_execute: bool,
}

/// Outcome of a successful execute — the gate chain that admitted it plus the
/// dispatch result. Exactly one of `instance` / `projected` is populated per the
/// `dispatch` kind; both are `Option` so the serialized shape stays backward-
/// compatible (an `instance_revision` result still carries the same top-level
/// `instance` key it always did — the console reads it unchanged).
#[derive(Debug, Clone, Serialize)]
pub struct ExecuteOutcome {
    pub dispatch: ActionDispatch,
    pub gates: GateChainOutcome,
    /// The appended revision head — present for an `instance_revision` dispatch.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instance: Option<InstanceState>,
    /// The domain use-case's JSON summary — present for a `projected_usecase`
    /// dispatch (the engine wrote nothing; the owning domain crate did).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub projected: Option<Value>,
    /// Immutable evidence for an instance-revision command. Projected actions
    /// retain their established domain-owned contract and have no engine receipt.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub receipt: Option<CommandReceipt>,
}

/// Immutable, replayable evidence of one accepted instance action command.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandReceipt {
    pub command_id: Uuid,
    pub payload_digest: String,
    pub instance: InstanceState,
    pub gates: GateChainOutcome,
}

/// Typed action failure, distinct from a raw DB error so callers (and tests) can
/// tell a gate deny from a not-yet-wired dispatch from a validation failure.
#[derive(Debug)]
pub enum ActionError {
    /// No action of that key on the given object type (or cross-tenant → hidden).
    NotFound,
    /// Params / control-point / edit shape rejected (fail-closed).
    Validation(String),
    /// A §16 gate blocked the action; nothing was written.
    GateDenied(String),
    /// A submission criterion did not hold; nothing was written.
    CriteriaFailed(String),
    /// A `projected_usecase` action whose `dispatch_target` has no registered
    /// domain handler (unwired or misconfigured). Fail-closed: no table write.
    NotWiredYet { target: Option<String> },
    /// A store / DB / context error.
    Store(PgOntologyError),
}

impl ActionError {
    /// Wrap a domain use-case's [`KernelError`] as a projected-dispatch failure,
    /// preserving its kind so the REST layer maps it to the right status (a domain
    /// `forbidden`/`conflict`/`not_found` stays a 403/409/404). The canonical way a
    /// [`ProjectedHandler`] surfaces a domain rejection without depending on the
    /// ontology adapter's error type.
    #[must_use]
    pub fn domain(error: KernelError) -> Self {
        Self::Store(PgOntologyError::Domain(error))
    }
}

/// Everything resolved for an action request, shared by preflight + execute: the
/// registry row (for the ids only this tier persists) plus the ONE prepared
/// command that owns every decision both paths act on.
struct Prepared {
    action: ActionTypeSummary,
    command: PreparedCommand,
}

impl OntologyRestState {
    /// Preflight an action: prepare it, read (never consume) the four-eyes
    /// decision, run the §16 gate chain and report the per-gate status WITHOUT
    /// committing anything.
    ///
    /// Nothing on this path PERSISTS anything. The registry read and the
    /// policy-gated head read are ordinary reads, the four-eyes read is the
    /// non-consuming [`PgGovernanceStore::four_eyes_approved`], and the
    /// [`PreparedCommand`] that owns every remaining decision is pure. The one
    /// place this path issues writes at all is
    /// [`Self::dry_run_instance_revision`], which issues them into a transaction
    /// it always rolls back. None of that is a property the type system holds, so
    /// what actually holds the zero-write claim is the row-delta census in
    /// `preflight_writes_zero_rows_and_never_spends_the_approval` (accepted edit
    /// set) and `preflight_refuses_an_edit_set_the_writeback_refuses` (rejected).
    ///
    /// A command this refuses is exactly a command [`Self::execute_action`]
    /// refuses, because both prepare through the same constructor — and, for
    /// everything only the WRITER can decide, because
    /// [`Self::dry_run_instance_revision`] runs that writer here.
    pub async fn preflight_action(
        &self,
        principal: &Principal,
        action_key: &str,
        command: ActionCommand,
    ) -> Result<PreflightOutcome, ActionError> {
        let prepared = self.prepare(principal, action_key, &command).await?;
        let gates = self.evaluate_gates(principal, &prepared).await?;
        let criteria_ok = prepared.command.criteria_ok();
        // Preparation resolves the edits; only the WRITEBACK judges what they
        // resolved TO — against the object type's property schema, its field
        // kinds and its derived-property declarations. Reporting `would_execute`
        // without that judgement is a false green about the one thing a dry run
        // exists to answer. Run the real writer, and throw its rows away.
        //
        // Only when the answer would otherwise be "yes": execute never reaches
        // the writeback past a denied gate or a failed criterion, so simulating
        // it there would refuse commands execute refuses for another reason
        // first, and would spend a transaction on every denied preflight.
        if gates.allow && criteria_ok {
            self.dry_run_instance_revision(principal, &prepared, &command)
                .await?;
        }
        Ok(PreflightOutcome {
            dispatch: prepared.command.dispatch(),
            dispatch_target: prepared.command.dispatch_target().map(str::to_owned),
            config: prepared.command.config(),
            would_execute: gates.allow && criteria_ok,
            gates,
            criteria_ok,
            criteria_error: prepared.command.criteria_error().map(str::to_owned),
        })
    }

    /// Execute an action — the core single mutation path for humans + automation.
    /// Fail-closed: an unmet gate, a failed submit criterion, or a malformed edit
    /// denies BEFORE any writeback opens. `instance_revision` then appends a
    /// fixity-chained revision inside one audited tx that re-checks the mutable
    /// gate; `projected_usecase` routes to the owning domain use-case via the
    /// [`ProjectedDispatchRegistry`] (unknown target ⇒ [`ActionError::NotWiredYet`]).
    pub async fn execute_action(
        &self,
        principal: &Principal,
        action_key: &str,
        command: ActionCommand,
    ) -> Result<ExecuteOutcome, ActionError> {
        let prepared = self.prepare(principal, action_key, &command).await?;

        if let Some(message) = prepared.command.criteria_error() {
            return Err(ActionError::CriteriaFailed(message.to_owned()));
        }

        match prepared.command.prepared_dispatch() {
            PreparedDispatch::ProjectedUsecase => {
                // No engine domain writeback: route to the owning domain crate's
                // use-case, which owns its own RLS + tx (§9.3 — no second source of
                // truth). An unwired/unknown target fails closed (`NotWiredYet`).
                // Non-roster handlers keep their own audit; canonical ports do not
                // (raw `begin()`), so success below emits `ontology.action.execute`.
                //
                // The "submission criteria are not evaluable for a projected action"
                // refusal now lives in `PreparedCommand::prepare`, so preflight
                // reports it too instead of promising an execute that cannot run.
                let target = prepared
                    .command
                    .dispatch_target()
                    .map(str::to_owned)
                    .ok_or(
                        // A projected action with no target can never resolve a handler.
                        ActionError::NotWiredYet { target: None },
                    )?;
                let canonical_target = DispatchTarget::from_str(&target).ok();

                // A canonical port REPLAYS a repeat of the same command_id and
                // returns the stored receipt verbatim. Peek the receipt store
                // BEFORE the gate chain: a retry whose single-use approval was
                // spent on the first attempt would otherwise be denied by the
                // four-eyes gate's non-consuming peek inside `evaluate_gates` and
                // never reach the port that replays it. Mirror InstanceRevision's
                // replay — the stored receipt's actor must match, then dispatch
                // (the port replays) and skip the gate chain and the single-use
                // consume. The accepted `action_key` is read back so the repair
                // audit records the ACCEPTED wrapper, not the retry's.
                let prior = match (canonical_target, command.command_id) {
                    (Some(_), Some(command_id)) => {
                        let org = current_org().map_err(|e| {
                            ActionError::Store(PgOntologyError::from(KernelError::from(e)))
                        })?;
                        // Tenant-scoped read: `ont_action_command_receipts` is
                        // FORCE-RLS on `app.current_org`, so the peek must run in
                        // the same armed transaction the writers use, not on a
                        // bare pool checkout (which would see no rows).
                        with_org_conn::<_, _, PgOntologyError>(
                            self.registry.pool(),
                            org,
                            move |tx| {
                                Box::pin(async move {
                                    let row: Option<(Uuid, Option<String>, Option<Uuid>)> =
                                        sqlx::query_as(
                                            "SELECT actor_id, action_key, object_type_id \
                                                 FROM ont_action_command_receipts \
                                                 WHERE org_id = $1 AND command_id = $2",
                                        )
                                        .bind(*org.as_uuid())
                                        .bind(command_id)
                                        .fetch_optional(tx.as_mut())
                                        .await?;
                                    Ok(row)
                                })
                            },
                        )
                        .await
                        .map_err(ActionError::Store)?
                    }
                    _ => None,
                };
                if let Some((prior_actor, prior_action_key, prior_object_type_id)) = prior {
                    if prior_actor != *principal.user_id.as_uuid() {
                        return Err(ActionError::Store(PgOntologyError::Domain(
                            KernelError::forbidden("command_id belongs to another principal"),
                        )));
                    }
                    // Reject a cross-action replay: the stored receipt binds the
                    // ACCEPTED action key, and a retry that reuses the same
                    // `command_id` through a DIFFERENT action (same canonical
                    // target + payload, possibly different checklist/four-eyes/
                    // egress controls) must not be handed the stored receipt.
                    // Legacy receipts written before 0219 carry a NULL action_key
                    // and keep the retry's-key fallback below.
                    if let Some(prior_key) = prior_action_key.as_deref()
                        && prior_key != action_key
                    {
                        return Err(ActionError::domain(KernelError::conflict(
                            "command_id was accepted under a different action",
                        )));
                    }
                    // Reject a cross-object-type replay: `action_key` is unique
                    // only per object type, so the same stable key on a DIFFERENT
                    // object type (same canonical target + payload, possibly
                    // different controls) must not be handed the stored receipt.
                    // Legacy receipts written before this column existed carry a
                    // NULL object_type_id and keep the retry's fallback.
                    if let Some(prior_type) = prior_object_type_id
                        && prior_type != *command.object_type_id.as_uuid()
                    {
                        return Err(ActionError::domain(KernelError::conflict(
                            "command_id was accepted under a different object type",
                        )));
                    }
                    // Recheck the CURRENT authority effect: owning the historical
                    // receipt is proof of ownership, not of present authorization.
                    // A requester who has since lost the org-wide capability is
                    // refused with 409 rather than handed the stored receipt.
                    if authority_effect(principal) == AuthorityEffect::Deny {
                        return Err(ActionError::domain(KernelError::conflict(
                            "the requester no longer holds the capability to \
                             replay this command",
                        )));
                    }
                    let projected = self
                        .projected_dispatch
                        .dispatch(ProjectedDispatch {
                            principal: principal.clone(),
                            target: target.clone(),
                            target_id: command.instance_id.map(|id| *id.as_uuid()),
                            command_id: command.command_id,
                            action_key: action_key.to_owned(),
                            object_type_id: *command.object_type_id.as_uuid(),
                            params: prepared.command.params().clone(),
                            reason: command.reason.clone(),
                            occurred_at: OffsetDateTime::now_utc(),
                        })
                        .await?;
                    // Replay: the stored receipt is returned verbatim; the gate
                    // chain is not re-evaluated (the four-eyes approval is already
                    // spent). Idempotently ensure the execute audit exists — a
                    // first attempt whose port committed its mutation + receipt
                    // but whose audit emission then failed must not replay to
                    // success while the mutation stays unaudited. After the
                    // cross-action rejection above, a non-NULL accepted action key
                    // is guaranteed equal to `action_key`; a NULL (legacy) key
                    // falls back to the retry's, which the digest check in the
                    // port has already bound to the accepted command.
                    emit_canonical_projected_audit(
                        self,
                        principal,
                        action_key,
                        &target,
                        command.command_id,
                        &projected,
                    )
                    .await?;
                    return Ok(ExecuteOutcome {
                        dispatch: ActionDispatch::ProjectedUsecase,
                        gates: GateChainOutcome {
                            gates: Vec::new(),
                            allow: true,
                        },
                        instance: None,
                        projected: Some(projected),
                        receipt: None,
                    });
                }

                // First attempt: run the §16 gate chain (authority → checklist →
                // four-eyes peek → egress) fail-closed, then consume the single-use
                // approval in our own committed step right before dispatch. A failed
                // first dispatch spends the approval: fail-closed, the requester
                // re-requests. TOCTOU-safety of the domain MUTATION is the domain
                // use-case's own responsibility and varies by use-case — the engine
                // makes no claim about it here.
                let gates = self.evaluate_gates(principal, &prepared).await?;
                if !gates.allow {
                    return Err(ActionError::GateDenied(
                        "an action gate is not satisfied".to_owned(),
                    ));
                }
                if let Some(request_ref) = prepared
                    .command
                    .four_eyes_request_ref()
                    .filter(|_| prepared.command.config().four_eyes)
                {
                    let (kind, bound_target) = prepared.command.four_eyes_binding();
                    let consumed = self
                        .governance
                        .four_eyes_consume(request_ref, kind, Some(bound_target), principal.user_id)
                        .await
                        .map_err(|e| ActionError::Store(governance_to_ontology(e)))?;
                    if consumed != Some(true) {
                        return Err(ActionError::GateDenied(
                            "four-eyes approval was already consumed or does not \
                             match this action"
                                .to_owned(),
                        ));
                    }
                }
                let projected = self
                    .projected_dispatch
                    .dispatch(ProjectedDispatch {
                        principal: principal.clone(),
                        target: target.clone(),
                        target_id: command.instance_id.map(|id| *id.as_uuid()),
                        command_id: command.command_id,
                        action_key: action_key.to_owned(),
                        object_type_id: *command.object_type_id.as_uuid(),
                        params: prepared.command.params().clone(),
                        reason: command.reason.clone(),
                        occurred_at: OffsetDateTime::now_utc(),
                    })
                    .await?;
                if canonical_target.is_some() {
                    emit_canonical_projected_audit(
                        self,
                        principal,
                        action_key,
                        &target,
                        command.command_id,
                        &projected,
                    )
                    .await?;
                }
                Ok(ExecuteOutcome {
                    dispatch: ActionDispatch::ProjectedUsecase,
                    gates,
                    instance: None,
                    projected: Some(projected),
                    receipt: None,
                })
            }
            PreparedDispatch::InstanceRevision { .. } => {
                // The two inputs only the writeback consumes. Refused HERE — the
                // one tier that maps a preparation refusal — so preflight, which
                // evaluates neither, is never refused for them.
                let writeback = prepared
                    .command
                    .writeback_inputs()
                    .map_err(|e| ActionError::Validation(e.message))?;
                // The edits already resolved during preparation (so preflight saw
                // any malformed edit too), against the same base the gate chain and
                // the criteria were evaluated from.
                let receipt = self
                    .execute_instance_revision(principal, action_key, &command, &prepared, writeback)
                    .await
                    .map_err(|error| match &error {
                        PgOntologyError::Domain(kernel)
                            if kernel.kind == ErrorKind::Forbidden
                                && kernel.message
                                    == "action gate re-check failed inside the writeback transaction" =>
                        {
                            ActionError::GateDenied("an action gate is not satisfied".to_owned())
                        }
                        _ => ActionError::Store(error),
                    })?;
                Ok(ExecuteOutcome {
                    dispatch: ActionDispatch::InstanceRevision,
                    gates: receipt.gates.clone(),
                    instance: Some(receipt.instance.clone()),
                    projected: None,
                    receipt: Some(receipt),
                })
            }
        }
    }

    /// Resolve the action and load the target's current attributes (the two DB
    /// reads only this tier can do), then hand both to the ONE
    /// [`PreparedCommand`] preparation. Every decision after this point — control
    /// points, params, inputs, edits, criteria, gate evidence — is that value's,
    /// which is why preflight and execute cannot disagree.
    async fn prepare(
        &self,
        principal: &Principal,
        action_key: &str,
        command: &ActionCommand,
    ) -> Result<Prepared, ActionError> {
        let action = self
            .registry
            .get_action_type(command.object_type_id, action_key)
            .await
            .map_err(ActionError::Store)?
            .ok_or(ActionError::NotFound)?;

        // Load the edit target's current attributes (empty for a create) so submit
        // criteria can read both the pending params and the object's current state.
        // Only an `instance_revision` target lives in `ont_instances`; a projected
        // action's target_id is a DOMAIN row (equipment, work order, …) that the
        // engine does not own, so we never resolve it here (submit criteria for a
        // projected action read params only) — and gating it would 404 every
        // projected dispatch against a row this crate cannot even see.
        //
        // Read THROUGH the gate: what an action evaluates is by construction what
        // the residual permitted. Both callers reach this before any write opens,
        // so a hidden row is refused before a revision, an approval or a dispatch
        // can be spent. A denied row raises the adapter's own `not_found`, so it
        // is indistinguishable from a genuinely missing one — 404, never 403.
        let base_attrs = match (action.dispatch, command.instance_id) {
            (ActionDispatch::InstanceRevision, Some(id)) => {
                visible_head_inner(self, principal, *id.as_uuid())
                    .await
                    .map_err(ActionError::Store)?
                    .into_inner()
                    .revision
                    .attributes
            }
            _ => Value::Object(serde_json::Map::new()),
        };

        let command = PreparedCommand::prepare(
            action_definition(&action),
            command_inputs(command),
            &base_attrs,
        )
        .map_err(|e| ActionError::Validation(e.message))?;

        Ok(Prepared { action, command })
    }

    /// Gather gate evidence and evaluate the chain. Authority is the legacy
    /// authorization contract's effect (the sole enforcer today; the seam is
    /// `authority_effect_from_cedar`); four-eyes is read from the DB. Checklist
    /// and egress come from the prepared command, so the evidence bag is not
    /// hand-assembled twice.
    async fn evaluate_gates(
        &self,
        principal: &Principal,
        prepared: &Prepared,
    ) -> Result<GateChainOutcome, ActionError> {
        // Non-consuming peek only — the authoritative bind-match + single-use
        // consume happens inside the writeback tx (`instance_revision_writeback`).
        let (expected_kind, expected_target) = prepared.command.four_eyes_binding();
        let four_eyes_approved = match prepared.command.four_eyes_request_ref() {
            Some(request_ref) => self
                .governance
                .four_eyes_approved(request_ref, expected_kind, Some(expected_target))
                .await
                .map_err(|e| ActionError::Store(governance_to_ontology(e)))?,
            None => None,
        };
        Ok(prepared
            .command
            .gates(authority_effect(principal), four_eyes_approved))
    }

    async fn execute_instance_revision(
        &self,
        principal: &Principal,
        action_key: &str,
        command: &ActionCommand,
        prepared: &Prepared,
        writeback: WritebackInputs,
    ) -> Result<CommandReceipt, PgOntologyError> {
        instance_revision_writeback(self, principal, action_key, command, prepared, writeback).await
    }

    /// The DRY RUN (DESIGN.md §4-42): run the writer the execute path runs, on a
    /// transaction that is ALWAYS rolled back, and keep only its verdict.
    ///
    /// This is not a second copy of the writeback's checks — it is the same
    /// [`stage_revision_in_tx`] / [`create_instance_in_tx`] call, against the same
    /// pool, with the same resolved attribute bag. So every refusal only the
    /// writer can reach is a refusal preflight reports: an attribute the object
    /// type does not declare, a value of the wrong field kind, a required
    /// attribute the edits left null, a `config.derive` that will not resolve, a
    /// disposed instance, a link binding with no referent. A hand-written
    /// preflight validator would have to re-derive all of that and would drift
    /// from the writer the first time either changed.
    ///
    /// Deliberately NOT replayed here: the four-eyes consume, the receipt insert
    /// and the audit event. Those are execute's bookkeeping about a command that
    /// happened, not the writer's verdict on whether it can — and consuming an
    /// approval to answer "would this work?" would spend it. The four-eyes gate
    /// preflight does report comes from the non-consuming peek in
    /// [`Self::evaluate_gates`].
    ///
    /// A `projected_usecase` action cannot have its domain write simulated
    /// (that would be the side effect §4-42 forbids), but it CAN run the owning
    /// canonical port's PURE `P::preflight` — an associated function with no
    /// `&self`, no IO and no side effect — so a projected dispatch refused at
    /// execute time is refused here too. A non-roster handler has no pure
    /// preflight and stays `Ok`.
    ///
    /// The transaction takes the same `FOR UPDATE` head lock the real writeback
    /// takes, for the length of the simulation. That is the honest cost of a dry
    /// run that is actually dry: the alternative prices a lock-free preflight at
    /// a verdict that can be wrong.
    async fn dry_run_instance_revision(
        &self,
        principal: &Principal,
        prepared: &Prepared,
        command: &ActionCommand,
    ) -> Result<(), ActionError> {
        let PreparedDispatch::InstanceRevision { attributes } =
            prepared.command.prepared_dispatch().clone()
        else {
            let Some(target_str) = prepared.command.dispatch_target() else {
                return Ok(());
            };
            let Ok(target) = DispatchTarget::from_str(target_str) else {
                return Ok(());
            };
            return self.projected_dispatch.preflight(
                target,
                command.instance_id.map(|id| *id.as_uuid()),
                prepared.command.params().clone(),
            );
        };
        let org = current_org()
            .map_err(|e| ActionError::Store(PgOntologyError::from(KernelError::from(e))))?;
        let actor = principal.user_id;
        let action_type_id = prepared.action.id;
        let instance_id = command.instance_id;
        let object_type_id = command.object_type_id;
        let title = command.title.clone();
        let reason = command.reason.clone();
        let valid_from = command.valid_from;
        let now = OffsetDateTime::now_utc();

        with_org_rollback::<_, (), PgOntologyError>(self.registry.pool(), org, move |tx| {
            Box::pin(async move {
                match instance_id {
                    Some(id) => {
                        stage_revision_in_tx(
                            tx,
                            actor,
                            org,
                            id,
                            StageRevision {
                                attributes,
                                valid_from,
                                action_type_id: Some(action_type_id),
                                reason,
                            },
                            now,
                        )
                        .await?;
                    }
                    None => {
                        create_instance_in_tx(
                            tx,
                            actor,
                            org,
                            CreateInstance {
                                object_type_id,
                                title: title.unwrap_or_default(),
                                attributes,
                                valid_from,
                                action_type_id: Some(action_type_id),
                                reason,
                            },
                            now,
                        )
                        .await?;
                    }
                }
                Ok(())
            })
        })
        .await
        // Returned as the SAME `ActionError` execute returns for the same
        // writer refusal, so the two entry points cannot disagree even in the
        // status the console renders.
        .map_err(ActionError::Store)
    }
}

/// Copy the registry row's declarative facts into the application layer's
/// [`ActionDefinition`] (which may not depend on the adapter's row type).
fn action_definition(action: &ActionTypeSummary) -> ActionDefinition {
    ActionDefinition {
        stable_key: action.stable_key.clone(),
        dispatch: action.dispatch,
        dispatch_target: action.dispatch_target.clone(),
        control_points: action.control_points.clone(),
        params_schema: action.params_schema.clone(),
        submission_criteria: action.submission_criteria.clone(),
        side_effects: action.side_effects.clone(),
        edits: action.edits.clone(),
    }
}

/// The untrusted half of the command, as bare ids for the pure layer.
fn command_inputs(command: &ActionCommand) -> CommandInputs {
    CommandInputs {
        object_type_id: *command.object_type_id.as_uuid(),
        instance_id: command.instance_id.map(|id| *id.as_uuid()),
        command_id: command.command_id,
        expected_revision: command.expected_revision,
        params: command.params.clone(),
        checklist_all_acknowledged: command.checklist_all_acknowledged,
        four_eyes_request_ref: command.four_eyes_request_ref,
    }
}

/// The §16 Authority gate input: today the legacy role matrix is the sole
/// enforcer, evaluated through the typed authorization contract and mapped onto
/// the gate's [`AuthorityEffect`] via the governance seam. Ontology is an
/// org-scoped admin surface, so this authorizes org-wide `RoleManage` (matching
/// the governance console); L-WIRE may introduce a dedicated ontology feature.
fn authority_effect(principal: &Principal) -> AuthorityEffect {
    let request = AuthorizationRequest::new(
        principal.clone(),
        Action::new(Feature::RoleManage),
        AuthorizationResource::org_wide(principal.org_id, "ontology_action"),
    );
    authority_effect_from_cedar(evaluate_legacy_contract(&request).effect)
}

/// The four-eyes `kind` a lifecycle-transition approval is decided under. A
/// lifecycle transition has no action key, so the kind is fixed and the approval
/// binds to the specific instance (its `target_ref`).
const LIFECYCLE_FOUR_EYES_KIND: &str = "ontology.lifecycle";

async fn action_preflight(
    State(state): State<OntologyRestState>,
    headers: HeaderMap,
    Path(action_key): Path<String>,
    Json(body): Json<ActionRequest>,
) -> Result<Json<PreflightOutcome>, RestError> {
    let principal = authorize_ontology(&state, &headers).await?;
    let outcome = state
        .preflight_action(&principal, &action_key, body.into_command())
        .await
        .map_err(RestError::from_action)?;
    Ok(Json(outcome))
}

async fn action_execute(
    State(state): State<OntologyRestState>,
    headers: HeaderMap,
    Path(action_key): Path<String>,
    Json(body): Json<ActionRequest>,
) -> Result<Json<ExecuteOutcome>, RestError> {
    let principal = authorize_ontology(&state, &headers).await?;
    let outcome = state
        .execute_action(&principal, &action_key, body.into_command())
        .await
        .map_err(RestError::from_action)?;
    Ok(Json(outcome))
}

/// Emit the engine-side audit row for a successful **canonical** projected
/// dispatch.
///
/// Canonical Postgres ports open `pool.begin()` and commit domain + receipt
/// rows without an `audit_events` insert. The canonical-projected execute is
/// recorded under a DISTINCT action, `ontology.canonical.execute`, so its dedup
/// key can never collide with `instance_revision_writeback`'s
/// `ontology.action.execute` rows (both key `target_id` by a tenant-global
/// `command_id`). Distinguish further via `target_type` (= receipt owner /
/// object key) and `after_snap`'s `dispatch_target`. Fail-closed: a failed
/// audit insert surfaces as [`ActionError::Store`] even though the port tx
/// already committed (inherent to the port-owned transaction boundary; same
/// class as four-eyes consume-before-dispatch).
async fn emit_canonical_projected_audit(
    state: &OntologyRestState,
    principal: &Principal,
    action_key: &str,
    dispatch_target: &str,
    command_id: Option<Uuid>,
    projected: &Value,
) -> Result<(), ActionError> {
    let org = current_org()
        .map_err(|e| ActionError::Store(PgOntologyError::from(KernelError::from(e))))?;
    let actor = principal.user_id;
    let command_id = command_id.ok_or_else(|| {
        ActionError::Validation(
            "canonical projected dispatch requires command_id for audit binding".to_owned(),
        )
    })?;
    let action_key = action_key.to_owned();
    let dispatch_target = dispatch_target.to_owned();
    let owner = projected
        .get("owner")
        .and_then(Value::as_str)
        .unwrap_or("canonical")
        .to_owned();
    let result = projected.get("result").cloned().unwrap_or(Value::Null);

    with_audits::<_, (), PgOntologyError>(state.registry.pool(), org, move |tx| {
        Box::pin(async move {
            // Idempotent: a canonical replay re-enters this helper to repair an
            // audit that was never recorded (the first attempt's port committed
            // its mutation + receipt and this emission then failed). Never mint
            // a second execute row for a command_id that already has one — under
            // EITHER taxonomy: this deployment records
            // `ontology.canonical.execute`, while commands accepted before it
            // were recorded under `ontology.action.execute` with the same
            // canonical-projected `after_snap` shape (`"dispatch":
            // "projected_usecase"`). Recognising both keeps a replay of a legacy
            // command from appending a second audit for one immutable mutation.
            // The partial unique index in migration 0220 is the DB-enforced
            // backstop for the check-then-insert window.
            let target_id = command_id.to_string();
            let already: bool = sqlx::query_scalar(
                "SELECT EXISTS(
                    SELECT 1 FROM audit_events
                    WHERE org_id = $1
                      AND target_id = $2
                      AND (
                        action = 'ontology.canonical.execute'
                        OR (
                          action = 'ontology.action.execute'
                          AND after_snap->>'dispatch' = 'projected_usecase'
                        )
                      )
                )",
            )
            .bind(*org.as_uuid())
            .bind(target_id.clone())
            .fetch_one(tx.as_mut())
            .await?;
            if already {
                return Ok(((), Vec::new()));
            }
            let now = OffsetDateTime::now_utc();
            let event = AuditEvent::new(
                Some(actor),
                AuditAction::new("ontology.canonical.execute")?,
                &owner,
                target_id,
                TraceContext::generate(),
                now,
            )
            .with_org(org)
            .with_snapshots(
                None,
                Some(serde_json::json!({
                    "action_key": action_key,
                    "dispatch": "projected_usecase",
                    "dispatch_target": dispatch_target,
                    "owner": owner,
                    "result": result,
                })),
            );
            Ok(((), vec![event]))
        })
    })
    .await
    .or_else(|error| {
        // A concurrent repair already minted the audit: the partial unique index
        // (migration 0220) turns the losing insert into 23505. Treat that as
        // idempotent success — the audit exists — not as an error.
        if matches!(
            &error,
            PgOntologyError::Db(DbError::Sqlx(sqlx::Error::Database(db)))
                if db.code().as_deref() == Some("23505")
        ) {
            Ok(())
        } else {
            Err(error)
        }
    })
    .map_err(ActionError::Store)
}

/// The instance-revision writeback: ONE `with_audits` tx that re-checks the
/// mutable gate (four-eyes) INSIDE the tx (TOCTOU-safe), then appends the
/// revision through the store's in-tx helper and writes the action's audit row —
/// all atomic. A re-check failure returns `Err` so the tx rolls back with zero
/// rows written.
async fn instance_revision_writeback(
    state: &OntologyRestState,
    principal: &Principal,
    action_key: &str,
    command: &ActionCommand,
    prepared: &Prepared,
    writeback: WritebackInputs,
) -> Result<CommandReceipt, PgOntologyError> {
    let body = command;
    let org = current_org().map_err(KernelError::from)?;
    let actor = principal.user_id;
    let action_type_id = prepared.action.id;
    let authority = authority_effect(principal);
    let instance_id = body.instance_id;
    let object_type_id = body.object_type_id;
    let title = body.title.clone();
    let reason = body.reason.clone();
    let valid_from = body.valid_from;
    let action_key = action_key.to_owned();
    // Every input this closure gates on is the ONE prepared command, moved in
    // whole: no field is re-derived here, so the writeback cannot gate on
    // anything preflight did not already see.
    let prepared_command = prepared.command.clone();
    let (expected_kind, expected_target) = {
        let (kind, target) = prepared_command.four_eyes_binding();
        (kind.to_owned(), target)
    };
    let four_eyes_ref = prepared_command.four_eyes_request_ref();
    let WritebackInputs {
        command_id,
        expected_revision,
    } = writeback;
    // The attribute bag was resolved during preparation, from the same base the
    // criteria and the gate chain were evaluated against.
    let PreparedDispatch::InstanceRevision {
        attributes: new_attrs,
    } = prepared_command.prepared_dispatch().clone()
    else {
        return Err(KernelError::validation(
            "instance_revision writeback reached with a non-instance dispatch",
        )
        .into());
    };
    let payload_digest = action_command_digest(&action_key, body, &new_attrs)?;

    with_audits::<_, CommandReceipt, PgOntologyError>(state.registry.pool(), org, move |tx| {
        Box::pin(async move {
            // Serialize same-id attempts before inspecting the immutable receipt.
            // This is tenant-scoped by the receipt key and keeps a retry from
            // consuming approvals or appending another revision.
            sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
                .bind(command_id.to_string())
                .execute(tx.as_mut()).await?;
            if let Some(row) = sqlx::query(
                "SELECT actor_id, payload_digest, receipt FROM ont_action_command_receipts WHERE org_id = $1 AND command_id = $2",
            )
            .bind(*org.as_uuid()).bind(command_id).fetch_optional(tx.as_mut()).await? {
                let receipt_actor: Uuid = row.try_get("actor_id")?;
                if receipt_actor != *actor.as_uuid() {
                    return Err(KernelError::forbidden("command_id belongs to another principal").into());
                }
                let stored: Vec<u8> = row.try_get("payload_digest")?;
                if stored != payload_digest {
                    return Err(KernelError::conflict("command_id was already used with a different payload").into());
                }
                return Ok((row.try_get::<serde_json::Value, _>("receipt")
                    .map_err(|e| KernelError::validation(format!("invalid command receipt: {e}")))
                    .and_then(|value| serde_json::from_value(value).map_err(|e| KernelError::validation(format!("invalid command receipt: {e}"))))?, vec![]));
            }

            // Lock and CAS the edit head before consuming a four-eyes approval. The
            // version match pins the same immutable revision row `prepare` read, so
            // `new_attrs` is derived from exactly the state the criteria and the
            // gate chain were evaluated against; a head that moved fails here.
            if let (Some(id), Some(expected)) = (instance_id, expected_revision) {
                let locked = sqlx::query(
                    "SELECT r.version FROM ont_instances i JOIN ont_instance_revisions r ON r.id = i.current_revision_id WHERE i.id = $1 FOR UPDATE",
                ).bind(*id.as_uuid()).fetch_optional(tx.as_mut()).await?
                    .ok_or_else(|| KernelError::not_found("instance was not found"))?;
                let current: i64 = locked.try_get("version")?;
                if current != expected {
                    return Err(PgOntologyError::ActionPreconditionFailed { current });
                }
            }
            // TOCTOU re-check: bind-match AND consume the four-eyes approval inside
            // THIS tx, then re-run the whole chain. Anything not satisfied now ⇒ deny
            // ⇒ rollback (0 rows, and the consumption rolls back too so a legitimate
            // retry can re-use the approval).
            let four_eyes_approved = match four_eyes_ref {
                Some(request_ref) => four_eyes_consume_conn(
                    tx.as_mut(),
                    request_ref,
                    &expected_kind,
                    Some(expected_target),
                    actor,
                )
                .await
                .map_err(governance_to_ontology)?,
                None => None,
            };
            let gates = prepared_command.gates(authority, four_eyes_approved);
            if !gates.allow {
                return Err(KernelError::forbidden(
                    "action gate re-check failed inside the writeback transaction",
                )
                .into());
            }

            let now = OffsetDateTime::now_utc();
            let result = match instance_id {
                Some(id) => {
                    stage_revision_in_tx(
                        tx,
                        actor,
                        org,
                        id,
                        StageRevision {
                            attributes: new_attrs,
                            valid_from,
                            action_type_id: Some(action_type_id),
                            reason,
                        },
                        now,
                    )
                    .await?
                }
                None => {
                    create_instance_in_tx(
                        tx,
                        actor,
                        org,
                        CreateInstance {
                            object_type_id,
                            title: title.unwrap_or_default(),
                            attributes: new_attrs,
                            valid_from,
                            action_type_id: Some(action_type_id),
                            reason,
                        },
                        now,
                    )
                    .await?
                }
            };

            let event = AuditEvent::new(
                Some(actor),
                AuditAction::new("ontology.action.execute")?,
                "ont_instances",
                result.instance.id.to_string(),
                TraceContext::generate(),
                now,
            )
            .with_org(org)
            .with_snapshots(
                None,
                Some(serde_json::json!({
                    "action_key": action_key,
                    "version": result.revision.version,
                    "attributes": result.revision.attributes,
                })),
            );
            let receipt = CommandReceipt {
                command_id,
                payload_digest: digest_hex(&payload_digest),
                instance: result,
                gates,
            };
            sqlx::query(
                "INSERT INTO ont_action_command_receipts (org_id, command_id, actor_id, payload_digest, receipt, created_at) VALUES ($1, $2, $3, $4, $5, $6)",
            ).bind(*org.as_uuid()).bind(command_id).bind(*actor.as_uuid()).bind(&payload_digest)
                .bind(serde_json::to_value(&receipt).map_err(|e| KernelError::validation(format!("command receipt did not serialize: {e}")))?)
                .bind(now).execute(tx.as_mut()).await?;
            Ok((receipt, vec![event]))
        })
    })
    .await
    // Side-effects (notify / webhook / WORM attachment) run AFTER commit and must
    // be idempotent. None are dispatched in v1.
    // ponytail: side-effect dispatch lands with the §13 egress / comms lane.
}

fn action_command_digest(
    action_key: &str,
    command: &ActionCommand,
    _attributes: &Value,
) -> Result<Vec<u8>, PgOntologyError> {
    let canonical = serde_json::json!({
        "action_key": action_key,
        "object_type_id": command.object_type_id.to_string(),
        "instance_id": command.instance_id.map(|id| id.to_string()),
        "title": command.title.clone(),
        "params": command.params.clone(),
        "reason": command.reason.clone(),
        "valid_from": command.valid_from,
        "checklist_all_acknowledged": command.checklist_all_acknowledged,
        "four_eyes_request_ref": command.four_eyes_request_ref,
        "expected_revision": command.expected_revision,
    });
    let bytes = serde_json::to_vec(&canonical_json(&canonical))
        .map_err(|e| KernelError::validation(format!("command payload did not serialize: {e}")))?;
    Ok(Sha256::digest(bytes).to_vec())
}

/// Recursively sort JSON object keys before digesting a client command. The
/// digest intentionally excludes derived/current instance attributes: those can
/// change after a transport loss without changing the command being retried.
fn canonical_json(value: &Value) -> Value {
    match value {
        Value::Array(values) => Value::Array(values.iter().map(canonical_json).collect()),
        Value::Object(values) => Value::Object(
            values
                .iter()
                .map(|(key, value)| (key.clone(), canonical_json(value)))
                .collect(),
        ),
        primitive => primitive.clone(),
    }
}

fn digest_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Map the governance store error onto the ontology error so both can flow
/// through one `with_audits` closure error type. Same two-variant shape.
fn governance_to_ontology(error: PgGovernanceError) -> PgOntologyError {
    match error {
        PgGovernanceError::Db(db) => PgOntologyError::Db(db),
        PgGovernanceError::Domain(kernel) => PgOntologyError::Domain(kernel),
    }
}

// ---------------------------------------------------------------------------
// Lifecycle commit (§3b governance-gated instance-lifecycle transition)
// ---------------------------------------------------------------------------

/// Typed lifecycle-transition command (HTTP-independent) — the write counterpart
/// to the governance `lifecycle/preflight` read: the console preflights the edge,
/// then hands the allowed transition here to commit it.
#[derive(Debug, Clone)]
pub struct LifecycleCommand {
    pub to_state: InstanceLifecycleState,
    pub reason: Option<String>,
    /// Client-supplied self-checklist witness (§16 gate 2; fail-closed when absent).
    pub checklist_all_acknowledged: Option<bool>,
    /// Four-eyes request ref; its decision is read from the DB, never trusted from
    /// the caller.
    pub four_eyes_request_ref: Option<Uuid>,
}

#[derive(Debug, Deserialize)]
struct LifecycleRequest {
    to_state: InstanceLifecycleState,
    #[serde(default)]
    reason: Option<String>,
    #[serde(default)]
    checklist_all_acknowledged: Option<bool>,
    #[serde(default)]
    four_eyes_request_ref: Option<Uuid>,
}

impl LifecycleRequest {
    fn into_command(self) -> LifecycleCommand {
        LifecycleCommand {
            to_state: self.to_state,
            reason: self.reason,
            checklist_all_acknowledged: self.checklist_all_acknowledged,
            four_eyes_request_ref: self.four_eyes_request_ref,
        }
    }
}

/// Outcome of a committed transition — the new instance head plus the gate chain
/// that admitted it (mirrors [`ExecuteOutcome`]).
#[derive(Debug, Clone, Serialize)]
pub struct LifecycleOutcome {
    pub instance: InstanceHead,
    pub config: GateChainConfig,
    pub gates: GateChainOutcome,
}

/// The instance lifecycle FSM and the governance lifecycle FSM are the same five
/// states under different casings; map onto the governance state for validation +
/// config lookup (which the console preflight already speaks).
const fn to_governance_state(state: InstanceLifecycleState) -> LifecycleState {
    match state {
        InstanceLifecycleState::Draft => LifecycleState::Draft,
        InstanceLifecycleState::Active => LifecycleState::Active,
        InstanceLifecycleState::Locked => LifecycleState::Locked,
        InstanceLifecycleState::Archived => LifecycleState::Archived,
        InstanceLifecycleState::Disposed => LifecycleState::Disposed,
    }
}

impl OntologyRestState {
    /// Commit a §3b instance-lifecycle transition — the write counterpart to the
    /// governance lifecycle preflight. Validated against the base FSM AND the
    /// per-object-type `gov_lifecycle_transitions` config (an unconfigured edge is
    /// fail-closed), gated by the §16 chain (authority via the legacy contract,
    /// four-eyes read from the DB, checklist client-supplied), then committed in ONE
    /// audited tx that re-checks four-eyes and guards the from-state (TOCTOU-safe).
    pub async fn commit_lifecycle(
        &self,
        principal: &Principal,
        instance_id: InstanceId,
        command: LifecycleCommand,
    ) -> Result<LifecycleOutcome, ActionError> {
        // Load the target THROUGH the gate (RLS floor + object-policy residual):
        // a cross-org, missing OR policy-hidden id ⇒ NotFound, no leak. This is the
        // first statement, so refusal precedes `lifecycle_writeback` and the
        // four-eyes consume inside it — and precedes the 403 below, whose message
        // interpolates the row's CURRENT STATE.
        let head = visible_head_inner(self, principal, *instance_id.as_uuid())
            .await
            .map_err(|e| match e {
                PgOntologyError::Domain(k) if k.kind == ErrorKind::NotFound => {
                    ActionError::NotFound
                }
                other => ActionError::Store(other),
            })?
            .into_inner()
            .instance;
        let from = to_governance_state(head.lifecycle_state);
        let to = to_governance_state(command.to_state);

        // Base FSM: an illegal edge can never commit (preserves the kernel kind, so
        // an illegal edge is a 409, a disposed source is a conflict).
        validate_lifecycle_transition(from, to)
            .map_err(|k| ActionError::Store(PgOntologyError::Domain(k)))?;

        // Per-object-type config: an unconfigured edge is fail-closed (deny).
        let reqs = self
            .governance
            .transition_requirements(*head.object_type_id.as_uuid(), from, to)
            .await
            .map_err(|e| ActionError::Store(governance_to_ontology(e)))?
            .ok_or_else(|| {
                ActionError::GateDenied(format!(
                    "lifecycle transition {} -> {} is not configured for this object type",
                    from.as_db_str(),
                    to.as_db_str()
                ))
            })?;

        // `requires_reason` is a config precondition, not a §16 gate.
        let has_reason = command
            .reason
            .as_deref()
            .is_some_and(|r| !r.trim().is_empty());
        if reqs.requires_reason && !has_reason {
            return Err(ActionError::CriteriaFailed(
                "this lifecycle transition requires a reason".to_owned(),
            ));
        }

        let config = GateChainConfig {
            authority: true,
            self_checklist: reqs.requires_checklist,
            four_eyes: reqs.requires_four_eyes,
            // A pure lifecycle transition has no outbound side-effects to classify.
            egress_dlp: false,
        };

        // Fail-closed pre-tx gate evaluation.
        let gates = self
            .evaluate_lifecycle_gates(principal, instance_id, config, &command)
            .await?;
        if !gates.allow {
            let reason = gates.first_blocking().map_or_else(
                || "a lifecycle gate is not satisfied".to_owned(),
                |g| format!("gate {:?} blocked: {:?}", g.gate, g.status),
            );
            return Err(ActionError::GateDenied(reason));
        }

        // Audited writeback with TOCTOU re-check + from-state guard.
        let instance = lifecycle_writeback(self, principal, instance_id, command, config, head)
            .await
            .map_err(ActionError::Store)?;
        Ok(LifecycleOutcome {
            instance,
            config,
            gates,
        })
    }

    async fn evaluate_lifecycle_gates(
        &self,
        principal: &Principal,
        instance_id: InstanceId,
        config: GateChainConfig,
        command: &LifecycleCommand,
    ) -> Result<GateChainOutcome, ActionError> {
        let authority = authority_effect(principal);
        // Non-consuming peek only — the authoritative consume is in the writeback.
        let four_eyes_approved = match command.four_eyes_request_ref {
            Some(request_ref) => self
                .governance
                .four_eyes_approved(
                    request_ref,
                    LIFECYCLE_FOUR_EYES_KIND,
                    Some(*instance_id.as_uuid()),
                )
                .await
                .map_err(|e| ActionError::Store(governance_to_ontology(e)))?,
            None => None,
        };
        let evidence = GateEvidence {
            authority: Some(authority),
            checklist_all_acknowledged: command.checklist_all_acknowledged,
            four_eyes_approved,
            egress_cleared: None,
        };
        Ok(evaluate_gate_chain(config, &evidence))
    }
}

async fn commit_lifecycle(
    State(state): State<OntologyRestState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(body): Json<LifecycleRequest>,
) -> Result<Json<LifecycleOutcome>, RestError> {
    let principal = authorize_ontology(&state, &headers).await?;
    let outcome = state
        .commit_lifecycle(&principal, InstanceId::from_uuid(id), body.into_command())
        .await
        .map_err(RestError::from_action)?;
    Ok(Json(outcome))
}

/// The lifecycle writeback: ONE `with_audits` tx that re-reads four-eyes evidence
/// INSIDE the tx (TOCTOU-safe) and re-runs the chain, then updates the head state
/// with a from-state guard (`WHERE lifecycle_state = <expected>`) so a concurrent
/// transition can never be double-applied — a mismatch or a re-check failure rolls
/// the tx back with zero rows written.
async fn lifecycle_writeback(
    state: &OntologyRestState,
    principal: &Principal,
    instance_id: InstanceId,
    command: LifecycleCommand,
    config: GateChainConfig,
    head: InstanceHead,
) -> Result<InstanceHead, PgOntologyError> {
    let org = current_org().map_err(KernelError::from)?;
    let actor = principal.user_id;
    let authority = authority_effect(principal);
    let checklist = command.checklist_all_acknowledged;
    let four_eyes_ref = command.four_eyes_request_ref;
    let to = command.to_state;
    let reason = command.reason.clone();
    let expected_from = head.lifecycle_state;
    let expected_target = *instance_id.as_uuid();

    with_audits::<_, InstanceHead, PgOntologyError>(state.registry.pool(), org, move |tx| {
        Box::pin(async move {
            // TOCTOU re-check: bind-match AND consume the four-eyes approval inside
            // THIS tx (single-use), re-run the chain. Not satisfied ⇒ rollback.
            let four_eyes_approved = match four_eyes_ref {
                Some(request_ref) => four_eyes_consume_conn(
                    tx.as_mut(),
                    request_ref,
                    LIFECYCLE_FOUR_EYES_KIND,
                    Some(expected_target),
                    actor,
                )
                .await
                .map_err(governance_to_ontology)?,
                None => None,
            };
            let evidence = GateEvidence {
                authority: Some(authority),
                checklist_all_acknowledged: checklist,
                four_eyes_approved,
                egress_cleared: None,
            };
            if !evaluate_gate_chain(config, &evidence).allow {
                return Err(KernelError::forbidden(
                    "lifecycle gate re-check failed inside the writeback transaction",
                )
                .into());
            }

            let now = OffsetDateTime::now_utc();
            // From-state guard: the transition applies iff the state is still the one
            // preflight validated (also covers cross-org/missing → 0 rows).
            let result = sqlx::query(
                "UPDATE ont_instances SET lifecycle_state = $2, updated_at = $3 \
                 WHERE id = $1 AND lifecycle_state = $4",
            )
            .bind(*instance_id.as_uuid())
            .bind(to.as_db_str())
            .bind(now)
            .bind(expected_from.as_db_str())
            .execute(tx.as_mut())
            .await?;
            if result.rows_affected() == 0 {
                return Err(KernelError::conflict(
                    "instance lifecycle state changed during the transition; retry",
                )
                .into());
            }

            let event = AuditEvent::new(
                Some(actor),
                AuditAction::new("ontology.instance.transition")?,
                "ont_instances",
                instance_id.to_string(),
                TraceContext::generate(),
                now,
            )
            .with_org(org)
            .with_snapshots(
                Some(serde_json::json!({ "lifecycle_state": expected_from.as_db_str() })),
                Some(serde_json::json!({
                    "lifecycle_state": to.as_db_str(),
                    "reason": reason,
                })),
            );
            Ok((
                InstanceHead {
                    lifecycle_state: to,
                    ..head
                },
                vec![event],
            ))
        })
    })
    .await
}

// ---------------------------------------------------------------------------
// Acting-read (§2 dynamics chips) + code→instance resolve
// ---------------------------------------------------------------------------

async fn instance_acting(
    State(state): State<OntologyRestState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Json<Vec<ActingRule>>, RestError> {
    let principal = authorize_ontology(&state, &headers).await?;
    // Measured existence oracle before the gate: an ungated 200 here confirmed a
    // hidden row existed AND named the policies attached to it.
    visible_head(&state, &principal, id).await?;
    let acting = state
        .registry
        .acting_on_instance(id)
        .await
        .map_err(RestError::from_ontology)?;
    Ok(Json(acting))
}

async fn object_type_acting(
    State(state): State<OntologyRestState>,
    headers: HeaderMap,
    Path(key): Path<String>,
) -> Result<Json<Vec<ActingRule>>, RestError> {
    authorize_ontology(&state, &headers).await?;
    let acting = state
        .registry
        .acting_on_type(&key)
        .await
        .map_err(RestError::from_ontology)?;
    Ok(Json(acting))
}

#[derive(Debug, Deserialize)]
struct ResolveQuery {
    code: String,
}

async fn resolve_code(
    State(state): State<OntologyRestState>,
    headers: HeaderMap,
    Query(query): Query<ResolveQuery>,
) -> Result<Json<ResolvedInstance>, RestError> {
    let principal = authorize_ontology(&state, &headers).await?;
    // Deny-by-omission: an unknown / cross-tenant code is a 404, never a 403.
    let unresolvable =
        || RestError::from_kernel(KernelError::not_found("no instance resolves that code"));
    let resolved = state
        .registry
        .resolve_by_code(&query.code)
        .await
        .map_err(RestError::from_ontology)?
        .ok_or_else(unresolvable)?;
    // `ResolvedInstance` carries no `object_type_id`, so the gate resolves it
    // from the instance itself. Measured before the gate: this route returned
    // the id, type and title of a policy-hidden row.
    //
    // The gate's own 404 says "instance was not found" — a DIFFERENT body than
    // this route's miss, and the only gated route where the two diverge. Object
    // codes are human-meaningful and enumerable, so two distinguishable 404s
    // answer "does this code exist?" for rows the policy hides. Restate the
    // route's own miss; only a NotFound is rewritten, so a policy-loader failure
    // still surfaces as the 500 it is instead of a silent "no such code".
    visible_head(&state, &principal, resolved.id)
        .await
        .map_err(|error| match error.status {
            StatusCode::NOT_FOUND => unresolvable(),
            _ => error,
        })?;
    Ok(Json(resolved))
}

// ---------------------------------------------------------------------------
// Auth
// ---------------------------------------------------------------------------

/// Ontology is an org-scoped admin surface, so it authorizes org-wide.
// ponytail: dark/unwired surface — every endpoint gates on org-wide RoleManage
// (the existing PBAC-admin capability, as the governance console does). L-WIRE
// assigns per-endpoint ontology features when it merges this router live.
async fn authorize_ontology(
    state: &OntologyRestState,
    headers: &HeaderMap,
) -> Result<Principal, RestError> {
    let principal = principal_from_headers(state, headers).await?;
    authorize_org_wide(&principal, Action::new(Feature::RoleManage))
        .map_err(RestError::from_kernel)?;
    Ok(principal)
}

async fn principal_from_headers(
    state: &OntologyRestState,
    headers: &HeaderMap,
) -> Result<Principal, RestError> {
    let verifier = state.jwt_verifier.as_ref().ok_or_else(|| {
        RestError::unavailable("JWT verification is not configured for ontology API")
    })?;
    console_platform_request_context::resolve_principal(verifier, state.registry.pool(), headers)
        .await
        .map_err(rest_error_from_request_context)
}

// ---------------------------------------------------------------------------
// Errors (mirrors the governance rest error surface)
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct RestError {
    status: StatusCode,
    code: &'static str,
    message: String,
    current: Option<ObjectTypeWriteVersion>,
}

impl RestError {
    fn unauthorized(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::UNAUTHORIZED,
            code: "unauthorized",
            message: message.into(),
            current: None,
        }
    }

    fn unavailable(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::SERVICE_UNAVAILABLE,
            code: "unavailable",
            message: message.into(),
            current: None,
        }
    }

    fn internal(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            code: "internal",
            message: message.into(),
            current: None,
        }
    }

    /// A §16 gate denied the action (nothing was written).
    fn gate_denied(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::FORBIDDEN,
            code: "gate_denied",
            message: message.into(),
            current: None,
        }
    }

    /// A `projected_usecase` action whose real domain dispatch is not yet wired
    /// (L-WIRE). Typed so callers can distinguish "not implemented" from a deny.
    fn not_wired_yet(target: Option<&str>) -> Self {
        let message = target.map_or_else(
            || {
                "projected_usecase dispatch is not wired yet (lands in L-WIRE); nothing was written"
                    .to_owned()
            },
            |t| {
                format!(
                    "projected_usecase dispatch to '{t}' is not wired yet (lands in L-WIRE); nothing was written"
                )
            },
        );
        Self {
            status: StatusCode::NOT_IMPLEMENTED,
            code: "not_wired_yet",
            message,
            current: None,
        }
    }

    fn write_precondition_required() -> Self {
        Self {
            status: StatusCode::PRECONDITION_REQUIRED,
            code: "ontology_write_precondition_required",
            message: "If-Match is required for ontology object type writes".to_owned(),
            current: None,
        }
    }

    fn invalid_write_precondition() -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            code: "invalid_ontology_write_precondition",
            message: "If-Match must contain exactly one strong ontology key validator".to_owned(),
            current: None,
        }
    }

    fn from_kernel(error: KernelError) -> Self {
        Self {
            status: status_for_error_kind(error.kind),
            code: code_for_error_kind(error.kind),
            message: error.message,
            current: None,
        }
    }

    fn from_ontology(error: PgOntologyError) -> Self {
        match error {
            PgOntologyError::Domain(kernel) => Self::from_kernel(kernel),
            PgOntologyError::Db(db) => Self::from_db(db),
            PgOntologyError::PreconditionFailed { current } => Self {
                status: StatusCode::PRECONDITION_FAILED,
                code: "ontology_write_precondition_failed",
                message: "stale ontology object type write validator".to_owned(),
                current: Some(current),
            },
            PgOntologyError::ActionPreconditionFailed { current } => Self {
                status: StatusCode::PRECONDITION_FAILED,
                code: "ontology_action_revision_precondition_failed",
                message: format!("stale action revision; current revision is {current}"),
                current: None,
            },
            PgOntologyError::CommandUnavailable => Self {
                status: StatusCode::SERVICE_UNAVAILABLE,
                code: "ontology_command_unavailable",
                message: "ontology command database is not configured or unavailable".to_owned(),
                current: None,
            },
        }
    }

    fn from_action(error: ActionError) -> Self {
        match error {
            ActionError::NotFound => Self::from_kernel(KernelError::not_found(
                "action type was not found for that object type",
            )),
            ActionError::Validation(message) => Self {
                status: StatusCode::UNPROCESSABLE_ENTITY,
                code: "validation",
                message,
                current: None,
            },
            ActionError::GateDenied(message) => Self::gate_denied(message),
            ActionError::CriteriaFailed(message) => Self {
                status: StatusCode::UNPROCESSABLE_ENTITY,
                code: "criteria_failed",
                message,
                current: None,
            },
            ActionError::NotWiredYet { target } => Self::not_wired_yet(target.as_deref()),
            ActionError::Store(error) => Self::from_ontology(error),
        }
    }

    /// The Cedar policy store's error surface. `Domain` already carries the
    /// kernel `ErrorKind`, so a validation failure at attach maps to 422 with the
    /// validator's own message and needs no new mapping. `CommandUnavailable` is
    /// a deployment fault and maps to 503, exactly like
    /// `PgOntologyError::CommandUnavailable`: a 500 would be indistinguishable
    /// from a real database fault.
    fn from_cedar(error: PgCedarError) -> Self {
        match error {
            PgCedarError::Domain(error) => Self::from_kernel(error),
            PgCedarError::Db(error) => Self::from_db(error),
            PgCedarError::CommandUnavailable => Self {
                status: StatusCode::SERVICE_UNAVAILABLE,
                code: "ontology_command_unavailable",
                message: "ontology command database is not configured or unavailable".to_owned(),
                current: None,
            },
        }
    }

    fn from_db(error: DbError) -> Self {
        match error {
            DbError::Sqlx(sqlx::Error::RowNotFound) => {
                Self::from_kernel(KernelError::not_found("row was not found"))
            }
            DbError::Sqlx(sqlx::Error::Database(err))
                if err.code().is_some_and(|code| code == "23505") =>
            {
                tracing::error!(error = %err, "ontology unique-constraint violation");
                Self::from_kernel(KernelError::conflict("resource already exists"))
            }
            // 23503, foreign-key violation: the request named a row that does not
            // exist. Measured: attaching an object policy as a validly-signed
            // principal with no `users` row hit `created_by`'s
            // `(created_by, org_id) -> users(id, org_id)` FK and returned 500 —
            // a client-caused refusal reported as a server fault, which buries a
            // real authz condition (a token for a since-removed user) in the
            // noise operators are trained to page on.
            //
            // The message is deliberately generic and the constraint name stays
            // in the log: naming it would disclose the schema.
            DbError::Sqlx(sqlx::Error::Database(err))
                if err.code().is_some_and(|code| code == "23503") =>
            {
                tracing::error!(error = %err, "ontology foreign-key violation");
                Self::from_kernel(KernelError::validation(
                    "the request references a row that does not exist",
                ))
            }
            DbError::Sqlx(err) => {
                tracing::error!(error = %err, "database error");
                Self::internal("internal server error")
            }
            DbError::Serialize(err) => {
                tracing::error!(error = %err, "serialization error");
                Self::internal("internal server error")
            }
            DbError::CodeIssuance(err) => {
                tracing::error!(error = %err, "object-code issuance error");
                Self::internal("internal server error")
            }
        }
    }
}

fn rest_error_from_request_context(
    err: console_platform_request_context::RequestContextError,
) -> RestError {
    use console_platform_request_context::RequestContextError as E;
    match err {
        E::VerifierUnavailable => {
            RestError::unavailable("JWT verification is not configured for ontology API")
        }
        E::WrongTokenTier => RestError::from_kernel(KernelError::forbidden(
            "token tier is not valid for this route",
        )),
        E::AccessScope(error) => RestError::from_kernel(error),
        E::BranchScope(message) | E::EffectivePolicy(message) => RestError::internal(message),
        E::MissingOrg => RestError::internal("no tenant context is bound to the current request"),
        E::MissingBearer => RestError::unauthorized("missing or malformed bearer token"),
        E::InvalidToken => RestError::unauthorized("invalid bearer token"),
        E::InvalidClaim(message) => {
            RestError::unauthorized(format!("token claim is invalid: {message}"))
        }
    }
}

#[derive(Debug, Serialize)]
struct ErrorBody {
    error: ErrorPayload,
}

#[derive(Debug, Serialize)]
struct ErrorPayload {
    code: &'static str,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    current_key_write_revision: Option<i64>,
}

impl IntoResponse for RestError {
    fn into_response(self) -> Response {
        let current_revision = self.current.as_ref().map(|current| current.revision);
        let mut response = (
            self.status,
            Json(ErrorBody {
                error: ErrorPayload {
                    code: self.code,
                    message: self.message,
                    current_key_write_revision: current_revision,
                },
            }),
        )
            .into_response();
        if let Some(current) = self.current {
            if let Ok(etag) = axum::http::HeaderValue::from_str(&current.etag) {
                response
                    .headers_mut()
                    .insert(axum::http::header::ETAG, etag);
            }
            response.headers_mut().insert(
                axum::http::header::CACHE_CONTROL,
                axum::http::HeaderValue::from_static("no-store"),
            );
        }
        response
    }
}

const fn status_for_error_kind(kind: ErrorKind) -> StatusCode {
    match kind {
        ErrorKind::Validation => StatusCode::UNPROCESSABLE_ENTITY,
        ErrorKind::NotFound => StatusCode::NOT_FOUND,
        ErrorKind::Forbidden => StatusCode::FORBIDDEN,
        ErrorKind::Conflict | ErrorKind::InvalidTransition => StatusCode::CONFLICT,
        ErrorKind::Internal => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

const fn code_for_error_kind(kind: ErrorKind) -> &'static str {
    match kind {
        ErrorKind::Validation => "validation",
        ErrorKind::NotFound => "not_found",
        ErrorKind::Forbidden => "forbidden",
        ErrorKind::Conflict => "conflict",
        ErrorKind::InvalidTransition => "invalid_transition",
        ErrorKind::Internal => "internal",
    }
}

#[cfg(test)]
mod projected_dispatch_derivation;

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::header::IF_MATCH;
    use console_kernel_core::{OrgId, UserId};
    use console_ontology_canonical_domain::{
        CommandReceipt as CanonicalReceipt, PayRun, Preflight,
    };

    #[test]
    fn divergent_child_identity_conflict_maps_to_http_409() {
        let error = RestError::from_ontology(PgOntologyError::Domain(KernelError::conflict(
            "child identity already names a different definition",
        )));
        assert_eq!(error.status, StatusCode::CONFLICT);
        assert_eq!(error.code, "conflict");
    }

    #[test]
    fn missing_ontology_command_capability_maps_to_stable_503() {
        let error = RestError::from_ontology(PgOntologyError::CommandUnavailable);
        assert_eq!(error.status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(error.code, "ontology_command_unavailable");
        assert_eq!(
            error.message,
            "ontology command database is not configured or unavailable"
        );
        assert!(error.current.is_none());
    }

    #[test]
    fn object_type_if_match_requires_one_strong_key_validator() {
        let mut headers = HeaderMap::new();
        let missing = required_object_type_write_precondition(&headers).unwrap_err();
        assert_eq!(missing.status, StatusCode::PRECONDITION_REQUIRED);
        assert_eq!(missing.code, "ontology_write_precondition_required");

        for malformed in [
            "W/\"ont-object-type-key:00000000000000000000000000000001:r7\"",
            "*",
            "\"ont-object-type-key:00000000000000000000000000000001:r7\", \"other\"",
            "\"not-an-ontology-key-validator\"",
        ] {
            headers.insert(IF_MATCH, malformed.parse().unwrap());
            let error = required_object_type_write_precondition(&headers).unwrap_err();
            assert_eq!(error.status, StatusCode::BAD_REQUEST, "{malformed}");
            assert_eq!(error.code, "invalid_ontology_write_precondition");
        }

        headers.insert(
            IF_MATCH,
            "\"ont-object-type-key:00000000000000000000000000000001:r7\""
                .parse()
                .unwrap(),
        );
        headers.append(
            IF_MATCH,
            "\"ont-object-type-key:00000000000000000000000000000001:r8\""
                .parse()
                .unwrap(),
        );
        let duplicate = required_object_type_write_precondition(&headers).unwrap_err();
        assert_eq!(duplicate.status, StatusCode::BAD_REQUEST);
        headers.remove(IF_MATCH);
        headers.insert(
            IF_MATCH,
            "\"ont-object-type-key:00000000000000000000000000000001:r7\""
                .parse()
                .unwrap(),
        );
        let parsed = required_object_type_write_precondition(&headers).unwrap();
        assert_eq!(parsed.revision, 7);
        assert_eq!(
            parsed.validator_id,
            uuid::Uuid::from_u128(1),
            "the opaque strong validator is parsed exactly once at the REST boundary"
        );
    }

    #[test]
    fn stale_key_precondition_maps_to_412_with_current_etag_and_no_store() {
        let current = console_ontology_adapter_postgres::ObjectTypeWriteVersion {
            etag: "\"ont-object-type-key:00000000000000000000000000000001:r8\"".to_owned(),
            revision: 8,
        };
        let response = RestError::from_ontology(PgOntologyError::PreconditionFailed {
            current: current.clone(),
        })
        .into_response();
        assert_eq!(response.status(), StatusCode::PRECONDITION_FAILED);
        assert_eq!(
            response.headers().get("etag").unwrap(),
            current.etag.as_str()
        );
        assert_eq!(response.headers().get("cache-control").unwrap(), "no-store");
    }

    #[test]
    fn command_digest_canonicalizer_sorts_nested_payload_keys() {
        let left = serde_json::json!({"z": {"b": 2, "a": 1}, "a": [ {"d": 4, "c": 3} ]});
        let right = serde_json::json!({"a": [ {"c": 3, "d": 4} ], "z": {"a": 1, "b": 2}});
        assert_eq!(canonical_json(&left), canonical_json(&right));
    }

    #[test]
    fn command_digest_ignores_current_instance_attributes() {
        let command = ActionCommand {
            object_type_id: ObjectTypeId::from_uuid(Uuid::new_v4()),
            instance_id: Some(InstanceId::from_uuid(Uuid::new_v4())),
            title: None,
            params: serde_json::json!({"priority": "hi"}),
            reason: Some("operator request".to_owned()),
            valid_from: None,
            checklist_all_acknowledged: Some(true),
            four_eyes_request_ref: None,
            command_id: Some(Uuid::new_v4()),
            expected_revision: Some(7),
        };

        let before = action_command_digest(
            "set_priority",
            &command,
            &serde_json::json!({"priority": "lo", "unrelated": "before"}),
        )
        .unwrap();
        let after = action_command_digest(
            "set_priority",
            &command,
            &serde_json::json!({"priority": "lo", "unrelated": "after"}),
        )
        .unwrap();

        assert_eq!(before, after);
    }

    #[derive(Debug, serde::Deserialize)]
    struct BlockingQuery {
        target: String,
    }

    impl CanonicalQuery for BlockingQuery {
        fn dispatch_target(&self) -> DispatchTarget {
            self.target
                .parse()
                .expect("test payload names a roster member")
        }

        fn subject_id(&self) -> Option<Uuid> {
            None
        }
    }

    /// A port whose PURE preflight always blocks, so the projected dry-run path
    /// can be observed without opening a connection or executing anything.
    struct BlockingPort;

    impl CanonicalPort for BlockingPort {
        type Object = PayRun;
        type Query = BlockingQuery;
        type Command = (OrgId, CommandId, UserId, DispatchTarget);
        type Error = std::convert::Infallible;

        fn preflight(_query: &Self::Query) -> Preflight {
            Preflight::blocked(vec!["period_end before period_start".to_owned()])
        }

        fn command(
            org_id: OrgId,
            command_id: CommandId,
            actor_id: UserId,
            query: Self::Query,
            _action_key: &str,
            _object_type_id: uuid::Uuid,
        ) -> Self::Command {
            (org_id, command_id, actor_id, query.dispatch_target())
        }

        fn execute(&self, _command: &Self::Command) -> Result<CanonicalReceipt, Self::Error> {
            unreachable!("the preflight path must never execute")
        }
    }

    /// RED before the fix: `dry_run_instance_revision` returned `Ok(())` for
    /// every projected dispatch, so a port whose `P::preflight` blocks sailed
    /// through preflight and only refused at execute time. The registry must
    /// expose the port's pure preflight so preflight_action reports the same
    /// refusal execute would.
    #[test]
    fn projected_preflight_reports_the_ports_pure_blockers() {
        let registry = ProjectedDispatchRegistry::new().register_port(BlockingPort);
        let error = registry
            .preflight(
                DispatchTarget::PayrollCreateRun,
                None,
                serde_json::json!({"period_end": "2026-01-01", "period_start": "2026-02-01"}),
            )
            .expect_err("a blocking port preflight must refuse the dry run");
        match error {
            ActionError::Validation(message) => {
                assert!(
                    message.contains("period_end before period_start"),
                    "preflight must surface the port's blockers verbatim: {message}"
                );
            }
            other => panic!("expected Validation, got {other:?}"),
        }
    }
}
