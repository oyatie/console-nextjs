#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! RUNTIME proof that a canonical `projected_usecase` retry with the same
//! `command_id` returns the stored receipt verbatim without re-spending the
//! single-use four-eyes approval, idempotently repairs a missing
//! `ontology.canonical.execute` audit that a first attempt failed to record
//! after its port already committed, rechecks the requester's CURRENT authority
//! before replaying, and refuses a replay that reuses the `command_id` through
//! a different action key.

use std::collections::BTreeSet;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use console_governance_adapter_postgres::PgGovernanceStore;
use console_governance_application::{ApprovalDecision, DecideApprovalCommand};
use console_kernel_core::{BranchScope, ErrorKind, OrgId, TraceContext, UserId};
use console_ontology_adapter_postgres::instances::PgInstanceStore;
use console_ontology_adapter_postgres::{
    ActionTypeInput, CreateObjectTypeDraft, PgOntologyError, PgOntologyStore,
};
use console_ontology_canonical_domain::{
    CanonicalPort, CanonicalQuery, CommandId, CommandReceipt as CanonicalReceipt, Company,
    DispatchTarget, ObjectKey, Preflight, ReceiptOwner,
};
use console_ontology_domain::{ActionDispatch, BackingKind, ObjectTypeId};
use console_ontology_rest::{
    ActionCommand, ActionError, OntologyRestState, ProjectedDispatchRegistry,
};
use console_platform_authz::{Principal, Role};
use console_platform_db::{DbError, with_audit};
use console_platform_request_context::scope_org;
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use time::OffsetDateTime;
use uuid::Uuid;

async fn runtime_pool(owner: &PgPool) -> PgPool {
    let options = owner.connect_options().as_ref().clone();
    PgPoolOptions::new()
        .max_connections(4)
        .after_connect(|conn, _meta| {
            Box::pin(async move {
                sqlx::query("SET ROLE console_rt").execute(conn).await?;
                Ok(())
            })
        })
        .connect_with(options)
        .await
        .unwrap()
}

async fn command_pool(owner: &PgPool) -> PgPool {
    let options = owner.connect_options().as_ref().clone();
    PgPoolOptions::new()
        .max_connections(4)
        .after_connect(|conn, _meta| {
            Box::pin(async move {
                sqlx::query("SET ROLE console_ontology_cmd")
                    .execute(conn)
                    .await?;
                Ok(())
            })
        })
        .connect_with(options)
        .await
        .unwrap()
}

async fn seed_org(owner: &PgPool, org: Uuid) {
    let event = console_kernel_core::AuditEvent::new(
        None,
        console_kernel_core::AuditAction::new("test.seed_org").unwrap(),
        "organization",
        org.to_string(),
        TraceContext::generate(),
        OffsetDateTime::now_utc(),
    )
    .with_org(OrgId::from_uuid(org));
    with_audit(owner, event, |tx| {
        Box::pin(async move {
            sqlx::query(
                "INSERT INTO organizations (id, slug, name) VALUES ($1, $2, $3) \
                 ON CONFLICT (id) DO NOTHING",
            )
            .bind(org)
            .bind(format!("org-{}", &org.simple().to_string()[..12]))
            .bind("Org replay")
            .execute(tx.as_mut())
            .await
            .map_err(DbError::Sqlx)?;
            Ok::<(), DbError>(())
        })
    })
    .await
    .unwrap();
}

async fn seed_user(owner: &PgPool, org: Uuid) -> UserId {
    let user_id = UserId::new();
    let event = console_kernel_core::AuditEvent::new(
        None,
        console_kernel_core::AuditAction::new("test.seed_user").unwrap(),
        "user",
        user_id.as_uuid().to_string(),
        TraceContext::generate(),
        OffsetDateTime::now_utc(),
    )
    .with_org(OrgId::from_uuid(org));
    with_audit(owner, event, |tx| {
        Box::pin(async move {
            sqlx::query(
                "INSERT INTO users (id, display_name, roles, org_id) VALUES ($1, $2, $3, $4)",
            )
            .bind(*user_id.as_uuid())
            .bind("Admin replay")
            .bind(["SUPER_ADMIN"].as_slice())
            .bind(org)
            .execute(tx.as_mut())
            .await
            .map_err(DbError::Sqlx)?;
            Ok::<(), DbError>(())
        })
    })
    .await
    .unwrap();
    user_id
}

fn super_admin(user_id: UserId, org: OrgId) -> Principal {
    Principal::new(
        user_id,
        org,
        BTreeSet::from([Role::SuperAdmin]),
        BranchScope::All,
    )
}

#[derive(Debug, Deserialize)]
struct ReplayQuery {
    target: String,
}

impl CanonicalQuery for ReplayQuery {
    fn dispatch_target(&self) -> DispatchTarget {
        self.target
            .parse()
            .expect("dispatcher injects a roster member")
    }

    fn subject_id(&self) -> Option<Uuid> {
        None
    }
}

/// A port that replays on `command_id`: the first execute mints a receipt with a
/// fresh counter, a repeat returns the STORED receipt verbatim (same counter),
/// so the test can tell a replay from a re-execute.
struct ReplayingPort {
    next: Arc<Mutex<u64>>,
    receipts: Arc<Mutex<HashMap<Uuid, Value>>>,
    seen: Arc<Mutex<Vec<Uuid>>>,
}

impl CanonicalPort for ReplayingPort {
    type Object = Company;
    type Query = ReplayQuery;
    type Command = (OrgId, CommandId, UserId, DispatchTarget);
    type Error = std::convert::Infallible;

    fn preflight(_query: &Self::Query) -> Preflight {
        Preflight::ok()
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

    fn execute(&self, command: &Self::Command) -> Result<CanonicalReceipt, Self::Error> {
        let (org_id, command_id, actor_id, target) = command;
        let key = *command_id.as_uuid();
        self.seen.lock().unwrap().push(key);
        let result = {
            let mut receipts = self.receipts.lock().unwrap();
            match receipts.get(&key) {
                Some(stored) => stored.clone(),
                None => {
                    let mut next = self.next.lock().unwrap();
                    *next += 1;
                    let result = json!({"n": *next});
                    receipts.insert(key, result.clone());
                    result
                }
            }
        };
        Ok(CanonicalReceipt::new(
            *org_id,
            *command_id,
            ReceiptOwner::Canonical(ObjectKey::Company),
            *target,
            *actor_id,
            [0_u8; 32],
            result,
            OffsetDateTime::UNIX_EPOCH,
        ))
    }
}

async fn seed_action(owner: &PgPool, org: OrgId, actor: UserId) -> ObjectTypeId {
    scope_org(org, async {
        let store =
            PgOntologyStore::new(owner.clone()).with_command_pool(command_pool(owner).await);
        let draft = CreateObjectTypeDraft {
            stable_key: "company.proj".to_owned(),
            title: "회사".to_owned(),
            title_property_key: None,
            backing_kind: BackingKind::Projected,
            backing_table: Some("company_revisions".to_owned()),
            primary_key_property: Some("org_id".to_owned()),
            properties: Vec::new(),
            links: Vec::new(),
            actions: vec![ActionTypeInput {
                stable_key: "revise".to_owned(),
                title: "회사 개정".to_owned(),
                params_schema: json!({}),
                edits: json!([]),
                submission_criteria: json!([]),
                side_effects: json!([]),
                dispatch: ActionDispatch::ProjectedUsecase,
                dispatch_target: Some("company.revise".to_owned()),
                control_points: json!(["authority", "four_eyes"]),
            }],
            analytics: Vec::new(),
        };
        store
            .create_object_type(
                actor,
                draft,
                TraceContext::generate(),
                OffsetDateTime::now_utc(),
            )
            .await
            .expect("create projected object type")
            .id
    })
    .await
}

async fn approve_four_eyes(
    rt: &PgPool,
    org: OrgId,
    requested_by: UserId,
    approver: UserId,
    kind: &str,
    target: Uuid,
) -> Uuid {
    let request_ref = Uuid::new_v4();
    scope_org(org, async {
        PgGovernanceStore::new(rt.clone())
            .decide_approval(DecideApprovalCommand {
                approver,
                request_ref,
                kind: kind.to_owned(),
                requested_by,
                target_ref: Some(target),
                decision: ApprovalDecision::Approved,
                trace: TraceContext::generate(),
                occurred_at: OffsetDateTime::now_utc(),
            })
            .await
            .expect("record four-eyes approval");
    })
    .await;
    request_ref
}

fn command(type_id: ObjectTypeId, request_ref: Uuid, command_id: Uuid) -> ActionCommand {
    ActionCommand {
        object_type_id: type_id,
        instance_id: None,
        title: None,
        params: json!({}),
        reason: Some("canonical replay probe".to_owned()),
        valid_from: None,
        checklist_all_acknowledged: None,
        four_eyes_request_ref: Some(request_ref),
        command_id: Some(command_id),
        expected_revision: None,
    }
}

fn state(rt: &PgPool, cmd: &PgPool, port: ReplayingPort) -> OntologyRestState {
    OntologyRestState::new(
        PgOntologyStore::new(rt.clone()).with_command_pool(cmd.clone()),
        PgInstanceStore::new(rt.clone()),
        PgGovernanceStore::new(rt.clone()),
        None,
    )
    .with_projected_dispatch(ProjectedDispatchRegistry::new().register_port(port))
}

async fn count_execute_audits(owner: &PgPool, org: Uuid, command_id: Uuid) -> i64 {
    sqlx::query_scalar(
        "SELECT COUNT(*) FROM audit_events \
         WHERE org_id = $1 AND action = 'ontology.canonical.execute' AND target_id = $2",
    )
    .bind(org)
    .bind(command_id.to_string())
    .fetch_one(owner)
    .await
    .unwrap()
}

/// RED before the fix: a retry of the same command_id was denied with
/// GateDenied — the four-eyes gate peeked an already-consumed approval —
/// instead of returning the stored receipt. The engine must short-circuit a
/// canonical replay before the gate chain, never re-spend the approval, and not
/// mint a second audit row.
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn canonical_replay_returns_stored_receipt_without_respending_four_eyes(owner_pool: PgPool) {
    let rt = runtime_pool(&owner_pool).await;
    let cmd = command_pool(&owner_pool).await;
    let org = OrgId::knl();
    let org_uuid = *org.as_uuid();
    seed_org(&owner_pool, org_uuid).await;
    let actor = seed_user(&owner_pool, org_uuid).await;
    let approver = seed_user(&owner_pool, org_uuid).await;
    let type_id = seed_action(&owner_pool, org, actor).await;

    let request_ref =
        approve_four_eyes(&rt, org, actor, approver, "revise", *type_id.as_uuid()).await;

    let seen = Arc::new(Mutex::new(Vec::new()));
    let state = state(
        &rt,
        &cmd,
        ReplayingPort {
            next: Arc::new(Mutex::new(0)),
            receipts: Arc::new(Mutex::new(HashMap::new())),
            seen: Arc::clone(&seen),
        },
    );

    let command_id = Uuid::new_v4();
    let principal = super_admin(actor, org);

    let first = scope_org(
        org,
        state.execute_action(
            &principal,
            "revise",
            command(type_id, request_ref, command_id),
        ),
    )
    .await
    .expect("first execute must succeed");

    // The mock port is in-memory, so simulate its committed receipt row (the
    // real ports write this table inside their own transaction).
    sqlx::query(
        "INSERT INTO ont_action_command_receipts \
         (org_id, command_id, actor_id, payload_digest, receipt, action_key, created_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(org_uuid)
    .bind(command_id)
    .bind(*actor.as_uuid())
    .bind([0_u8; 32].as_slice())
    .bind(json!({"n": 1}))
    .bind("revise")
    .bind(OffsetDateTime::now_utc())
    .execute(&owner_pool)
    .await
    .expect("seed the port's receipt row");

    let replay = scope_org(
        org,
        state.execute_action(
            &principal,
            "revise",
            command(type_id, request_ref, command_id),
        ),
    )
    .await
    .expect("a same-command replay must return the stored receipt, not GateDenied");

    assert_eq!(
        first.projected, replay.projected,
        "the replay must return the stored receipt verbatim"
    );
    assert_eq!(
        *seen.lock().unwrap().as_slice(),
        vec![command_id, command_id],
        "the replay must reach the port (which replays on command_id)"
    );
    let consumptions: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM gov_approval_consumptions WHERE org_id = $1")
            .bind(org_uuid)
            .fetch_one(&owner_pool)
            .await
            .expect("count consumptions");
    assert_eq!(
        consumptions, 1,
        "the four-eyes approval is spent exactly once"
    );
    assert_eq!(
        count_execute_audits(&owner_pool, org_uuid, command_id).await,
        1,
        "the replay must not mint a second execute audit row"
    );
}

/// Repair path: a canonical port already committed its mutation + receipt but
/// the engine's execute-audit emission then failed, so a retry enters the
/// replay branch with NO audit on file. The replay must idempotently emit the
/// missing audit rather than returning success with the mutation unaudited.
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn canonical_replay_repairs_a_missing_execute_audit(owner_pool: PgPool) {
    let rt = runtime_pool(&owner_pool).await;
    let cmd = command_pool(&owner_pool).await;
    let org = OrgId::knl();
    let org_uuid = *org.as_uuid();
    seed_org(&owner_pool, org_uuid).await;
    let actor = seed_user(&owner_pool, org_uuid).await;
    let approver = seed_user(&owner_pool, org_uuid).await;
    let type_id = seed_action(&owner_pool, org, actor).await;

    let request_ref =
        approve_four_eyes(&rt, org, actor, approver, "revise", *type_id.as_uuid()).await;

    let state = state(
        &rt,
        &cmd,
        ReplayingPort {
            next: Arc::new(Mutex::new(0)),
            receipts: Arc::new(Mutex::new(HashMap::new())),
            seen: Arc::new(Mutex::new(Vec::new())),
        },
    );

    let command_id = Uuid::new_v4();
    let principal = super_admin(actor, org);

    // Simulate the crash window: the port's receipt is committed but the engine
    // never emitted the `ontology.action.execute` audit.
    sqlx::query(
        "INSERT INTO ont_action_command_receipts \
         (org_id, command_id, actor_id, payload_digest, receipt, action_key, created_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(org_uuid)
    .bind(command_id)
    .bind(*actor.as_uuid())
    .bind([0_u8; 32].as_slice())
    .bind(json!({"n": 1}))
    .bind("revise")
    .bind(OffsetDateTime::now_utc())
    .execute(&owner_pool)
    .await
    .expect("seed the port's committed receipt row");

    assert_eq!(
        count_execute_audits(&owner_pool, org_uuid, command_id).await,
        0,
        "the crash window leaves no audit behind"
    );

    scope_org(
        org,
        state.execute_action(
            &principal,
            "revise",
            command(type_id, request_ref, command_id),
        ),
    )
    .await
    .expect("a replay of an already-committed command must succeed");

    assert_eq!(
        count_execute_audits(&owner_pool, org_uuid, command_id).await,
        1,
        "the replay must idempotently emit the missing execute audit"
    );
}

/// Replay authz recheck: owning the historical receipt is proof of ownership,
/// not of present authorization. A requester who has since lost the org-wide
/// capability is refused with a 409 conflict, not handed the stored receipt.
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn canonical_replay_rechecks_current_authority(owner_pool: PgPool) {
    let rt = runtime_pool(&owner_pool).await;
    let cmd = command_pool(&owner_pool).await;
    let org = OrgId::knl();
    let org_uuid = *org.as_uuid();
    seed_org(&owner_pool, org_uuid).await;
    let actor = seed_user(&owner_pool, org_uuid).await;
    let approver = seed_user(&owner_pool, org_uuid).await;
    let type_id = seed_action(&owner_pool, org, actor).await;
    let request_ref =
        approve_four_eyes(&rt, org, actor, approver, "revise", *type_id.as_uuid()).await;

    let state = state(
        &rt,
        &cmd,
        ReplayingPort {
            next: Arc::new(Mutex::new(0)),
            receipts: Arc::new(Mutex::new(HashMap::new())),
            seen: Arc::new(Mutex::new(Vec::new())),
        },
    );

    let command_id = Uuid::new_v4();

    // Simulate the first attempt's committed receipt (same actor).
    sqlx::query(
        "INSERT INTO ont_action_command_receipts \
         (org_id, command_id, actor_id, payload_digest, receipt, action_key, created_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(org_uuid)
    .bind(command_id)
    .bind(*actor.as_uuid())
    .bind([0_u8; 32].as_slice())
    .bind(json!({"n": 1}))
    .bind("revise")
    .bind(OffsetDateTime::now_utc())
    .execute(&owner_pool)
    .await
    .expect("seed the port's committed receipt row");

    // The same actor, but no longer holding the org-wide capability.
    let revoked = Principal::new(actor, org, BTreeSet::from([Role::Member]), BranchScope::All);

    let err = scope_org(
        org,
        state.execute_action(
            &revoked,
            "revise",
            command(type_id, request_ref, command_id),
        ),
    )
    .await
    .expect_err("a replay by a requester who lost the capability must be refused");

    match err {
        ActionError::Store(PgOntologyError::Domain(kernel)) => {
            assert_eq!(
                kernel.kind,
                ErrorKind::Conflict,
                "the refusal must be a 409 conflict, got {:?}",
                kernel.kind
            );
        }
        other => panic!("expected a 409 conflict, got {other:?}"),
    }
}

/// Cross-action replay rejection: owning the historical receipt proves
/// ownership, not that the retry runs under the same action. A retry that
/// reuses the same `command_id` through a DIFFERENT action key (same canonical
/// target + payload) is refused with a 409 conflict rather than handed the
/// stored receipt, because the retry's action may carry different
/// checklist/four-eyes/egress controls than the action that was accepted.
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn canonical_replay_rejects_a_different_action(owner_pool: PgPool) {
    let rt = runtime_pool(&owner_pool).await;
    let cmd = command_pool(&owner_pool).await;
    let org = OrgId::knl();
    let org_uuid = *org.as_uuid();
    seed_org(&owner_pool, org_uuid).await;
    let actor = seed_user(&owner_pool, org_uuid).await;
    let approver = seed_user(&owner_pool, org_uuid).await;
    let type_id = seed_action(&owner_pool, org, actor).await;
    let request_ref =
        approve_four_eyes(&rt, org, actor, approver, "revise", *type_id.as_uuid()).await;

    let state = state(
        &rt,
        &cmd,
        ReplayingPort {
            next: Arc::new(Mutex::new(0)),
            receipts: Arc::new(Mutex::new(HashMap::new())),
            seen: Arc::new(Mutex::new(Vec::new())),
        },
    );

    let command_id = Uuid::new_v4();
    let principal = super_admin(actor, org);

    // The ACCEPTED command ran under action key "accepted-revise"; the retry
    // below reuses the same command_id under the (different) action key
    // "revise".
    sqlx::query(
        "INSERT INTO ont_action_command_receipts \
         (org_id, command_id, actor_id, payload_digest, receipt, action_key, created_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(org_uuid)
    .bind(command_id)
    .bind(*actor.as_uuid())
    .bind([0_u8; 32].as_slice())
    .bind(json!({"n": 1}))
    .bind("accepted-revise")
    .bind(OffsetDateTime::now_utc())
    .execute(&owner_pool)
    .await
    .expect("seed the port's committed receipt row");

    let err = scope_org(
        org,
        state.execute_action(
            &principal,
            "revise",
            command(type_id, request_ref, command_id),
        ),
    )
    .await
    .expect_err("a cross-action replay must be refused");

    match err {
        ActionError::Store(PgOntologyError::Domain(kernel)) => {
            assert_eq!(
                kernel.kind,
                ErrorKind::Conflict,
                "the refusal must be a 409 conflict, got {:?}",
                kernel.kind
            );
        }
        other => panic!("expected a 409 conflict, got {other:?}"),
    }
}

/// Cross-object-type replay rejection: `action_key` is unique only per object
/// type, so the same stable key on a DIFFERENT object type (same canonical
/// target + payload) must not be handed the stored receipt, because the
/// retry's action may carry different checklist/four-eyes/egress controls than
/// the action that was accepted.
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn canonical_replay_rejects_a_different_object_type(owner_pool: PgPool) {
    let rt = runtime_pool(&owner_pool).await;
    let cmd = command_pool(&owner_pool).await;
    let org = OrgId::knl();
    let org_uuid = *org.as_uuid();
    seed_org(&owner_pool, org_uuid).await;
    let actor = seed_user(&owner_pool, org_uuid).await;
    let approver = seed_user(&owner_pool, org_uuid).await;
    let type_id = seed_action(&owner_pool, org, actor).await;
    let request_ref =
        approve_four_eyes(&rt, org, actor, approver, "revise", *type_id.as_uuid()).await;

    let state = state(
        &rt,
        &cmd,
        ReplayingPort {
            next: Arc::new(Mutex::new(0)),
            receipts: Arc::new(Mutex::new(HashMap::new())),
            seen: Arc::new(Mutex::new(Vec::new())),
        },
    );

    let command_id = Uuid::new_v4();
    let principal = super_admin(actor, org);

    // The ACCEPTED command ran under the SAME action key "revise" but a
    // DIFFERENT object type; the retry below reuses the same command_id under
    // this test's object type.
    sqlx::query(
        "INSERT INTO ont_action_command_receipts \
         (org_id, command_id, actor_id, payload_digest, receipt, action_key, object_type_id, created_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
    )
    .bind(org_uuid)
    .bind(command_id)
    .bind(*actor.as_uuid())
    .bind([0_u8; 32].as_slice())
    .bind(json!({"n": 1}))
    .bind("revise")
    .bind(Uuid::new_v4())
    .bind(OffsetDateTime::now_utc())
    .execute(&owner_pool)
    .await
    .expect("seed the port's committed receipt row");

    let err = scope_org(
        org,
        state.execute_action(
            &principal,
            "revise",
            command(type_id, request_ref, command_id),
        ),
    )
    .await
    .expect_err("a cross-object-type replay must be refused");

    match err {
        ActionError::Store(PgOntologyError::Domain(kernel)) => {
            assert_eq!(
                kernel.kind,
                ErrorKind::Conflict,
                "the refusal must be a 409 conflict, got {:?}",
                kernel.kind
            );
        }
        other => panic!("expected a 409 conflict, got {other:?}"),
    }
}
