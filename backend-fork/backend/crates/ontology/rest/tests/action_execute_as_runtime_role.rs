#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! RUNTIME proofs for the §2/§16 action execute path, exercised as the genuine
//! non-owner `console_rt` role (NOSUPERUSER, NOBYPASSRLS, FORCE RLS) — the only
//! faithful exercise of RLS org-isolation. The default `#[sqlx::test]` pool is a
//! BYPASSRLS superuser that would green-light a broken policy.
//!
//! Proves:
//!   (a) execute happy-path appends a v1 revision + exactly one action audit row,
//!       atomically;
//!   (b) a failed gate (four-eyes required, no approval) ⇒ GateDenied AND zero
//!       rows written;
//!   (c) with the four-eyes approval present, the in-tx re-check admits and the
//!       revision commits (the TOCTOU re-check path is exercised on the DB);
//!   (d) a submission-criterion failure ⇒ CriteriaFailed AND zero rows;
//!   (e) a projected_usecase action ⇒ NotWiredYet AND zero rows (no domain write);
//!   (f) a cross-org action is invisible (NotFound) under another tenant's GUC.
//!   (g) preflight and execute reject the SAME commands — the two entry points
//!       share one `PreparedCommand`, so a command execute refuses can never be
//!       reported by preflight as one that `would_execute`;
//!   (h) preflight is READ-ONLY: zero row delta across every affected table
//!       (business, receipt, approval-consumption, audit, outbox), and the
//!       four-eyes approval it peeked at is still spendable afterwards;
//!   (i) a failed mutation NEVER spends the four-eyes approval;
//!   (j) a replay returns the STORED receipt (proven by MOVING the head between
//!       the first execution and the replay: a recomputed receipt could not still
//!       describe v1), while a changed digest or actor conflicts;
//!   (k) the DRY RUN is dry: preflight refuses an edit set only the WRITEBACK can
//!       refuse (and with execute's own error), while persisting nothing on the
//!       rejected AND the accepted set.
//!
//! NOTE (migrations path): runs against the canonical
//! `../../platform/db/migrations` (the ship path). The earlier concurrent-lane
//! migration-number collision has been reconciled, so no deduplicated copy is
//! needed.

use std::collections::BTreeSet;

use console_governance_adapter_postgres::PgGovernanceStore;
use console_governance_application::{ApprovalDecision, DecideApprovalCommand};
use console_kernel_core::{BranchScope, OrgId, TraceContext, UserId};
use console_ontology_adapter_postgres::instances::PgInstanceStore;
use console_ontology_adapter_postgres::{
    ActionTypeInput, CreateObjectTypeDraft, PgOntologyStore, PropertyDefInput,
};
use console_ontology_domain::{ActionDispatch, BackingKind, ObjectTypeId};
use console_ontology_rest::{ActionCommand, ActionError, OntologyRestState};
use console_platform_authz::{Principal, Role};
use console_platform_test_support::{
    attach_enforced_view_permit, runtime_role_pool, seed_org_and_super_admin,
};
use serde_json::{Value, json};
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use time::macros::datetime;
use uuid::Uuid;

const ORG_B: Uuid = Uuid::from_u128(0x4444_4444_4444_4444_4444_4444_4444_4444);
const AT: time::OffsetDateTime = datetime!(2026-07-10 12:00 UTC);

fn super_admin(user_id: UserId, org: OrgId) -> Principal {
    Principal::new(
        user_id,
        org,
        BTreeSet::from([Role::SuperAdmin]),
        BranchScope::All,
    )
}

async fn command_role_pool(owner_pool: &PgPool) -> PgPool {
    let options = owner_pool.connect_options().as_ref().clone();
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

fn state(pool: &PgPool, command_pool: &PgPool) -> OntologyRestState {
    OntologyRestState::new(
        PgOntologyStore::new(pool.clone()).with_command_pool(command_pool.clone()),
        PgInstanceStore::new(pool.clone()),
        PgGovernanceStore::new(pool.clone()),
        None,
    )
}

/// Publish an instance-backed object type with one required `priority` choice
/// property and one action (`action_key`) that writes `priority` from a param.
async fn seed_instance_type_with_action(
    owner_pool: &PgPool,
    org: OrgId,
    actor: UserId,
    key: &str,
    action_key: &str,
    control_points: Value,
    submission_criteria: Value,
) -> ObjectTypeId {
    console_platform_request_context::scope_org(org, async {
        let store = PgOntologyStore::new(owner_pool.clone())
            .with_command_pool(command_role_pool(owner_pool).await);
        let draft = CreateObjectTypeDraft {
            stable_key: key.to_owned(),
            title: "작업지시".to_owned(),
            title_property_key: Some("title".to_owned()),
            backing_kind: BackingKind::Instance,
            backing_table: None,
            primary_key_property: None,
            properties: vec![PropertyDefInput {
                key: "priority".to_owned(),
                title: "우선순위".to_owned(),
                field_type: "choice".to_owned(),
                config: json!({"choices": [{"id": "hi", "name": "높음"}, {"id": "lo", "name": "낮음"}]}),
                backing_column: None,
                required: true,
                in_property_policy: false,
            }],
            links: Vec::new(),
            actions: vec![ActionTypeInput {
                stable_key: action_key.to_owned(),
                title: "우선순위 설정".to_owned(),
                params_schema: json!({"priority": {"required": true}, "count": {}}),
                edits: json!([{"property": "priority", "param": "priority"}]),
                submission_criteria,
                side_effects: json!([]),
                dispatch: ActionDispatch::InstanceRevision,
                dispatch_target: None,
                control_points,
            }],
            analytics: Vec::new(),
        };
        store
            .create_object_type(actor, draft, TraceContext::generate(), AT)
            .await
            .expect("create object type")
            .id
    })
    .await
}

/// Publish a projected object type with a projected_usecase action.
async fn seed_projected_type_with_action(
    owner_pool: &PgPool,
    org: OrgId,
    actor: UserId,
    key: &str,
    action_key: &str,
) -> ObjectTypeId {
    console_platform_request_context::scope_org(org, async {
        let store = PgOntologyStore::new(owner_pool.clone())
            .with_command_pool(command_role_pool(owner_pool).await);
        let draft = CreateObjectTypeDraft {
            stable_key: key.to_owned(),
            title: "장비".to_owned(),
            title_property_key: None,
            backing_kind: BackingKind::Projected,
            backing_table: Some("registry_equipment".to_owned()),
            primary_key_property: Some("id".to_owned()),
            properties: Vec::new(),
            links: Vec::new(),
            actions: vec![ActionTypeInput {
                stable_key: action_key.to_owned(),
                title: "장비 갱신".to_owned(),
                params_schema: json!({}),
                edits: json!([]),
                submission_criteria: json!([]),
                side_effects: json!([]),
                dispatch: ActionDispatch::ProjectedUsecase,
                dispatch_target: Some("registry.update_equipment".to_owned()),
                control_points: json!(["authority"]),
            }],
            analytics: Vec::new(),
        };
        store
            .create_object_type(actor, draft, TraceContext::generate(), AT)
            .await
            .expect("create projected object type")
            .id
    })
    .await
}

/// Clone of [`seed_projected_type_with_action`] with only `dispatch_target`
/// changed to the non-roster Intelligence draft string.
async fn seed_projected_type_with_intelligence_draft(
    owner_pool: &PgPool,
    org: OrgId,
    actor: UserId,
    key: &str,
    action_key: &str,
) -> ObjectTypeId {
    console_platform_request_context::scope_org(org, async {
        let store = PgOntologyStore::new(owner_pool.clone())
            .with_command_pool(command_role_pool(owner_pool).await);
        let draft = CreateObjectTypeDraft {
            stable_key: key.to_owned(),
            title: "장비".to_owned(),
            title_property_key: None,
            backing_kind: BackingKind::Projected,
            backing_table: Some("registry_equipment".to_owned()),
            primary_key_property: Some("id".to_owned()),
            properties: Vec::new(),
            links: Vec::new(),
            actions: vec![ActionTypeInput {
                stable_key: action_key.to_owned(),
                title: "장비 갱신".to_owned(),
                params_schema: json!({}),
                edits: json!([]),
                submission_criteria: json!([]),
                side_effects: json!([]),
                dispatch: ActionDispatch::ProjectedUsecase,
                dispatch_target: Some("intelligence.draft".to_owned()),
                control_points: json!(["authority"]),
            }],
            analytics: Vec::new(),
        };
        store
            .create_object_type(actor, draft, TraceContext::generate(), AT)
            .await
            .expect("create projected object type")
            .id
    })
    .await
}

fn intelligence_draft_command(object_type_id: ObjectTypeId) -> ActionCommand {
    ActionCommand {
        object_type_id,
        instance_id: None,
        title: None,
        params: json!({}),
        reason: None,
        valid_from: Some(AT),
        checklist_all_acknowledged: None,
        four_eyes_request_ref: None,
        command_id: None,
        expected_revision: None,
    }
}

async fn count_instances(owner_pool: &PgPool, org: OrgId) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM ont_instances WHERE org_id = $1")
        .bind(*org.as_uuid())
        .fetch_one(owner_pool)
        .await
        .unwrap()
}

async fn count_execute_audits(owner_pool: &PgPool, org: OrgId) -> i64 {
    sqlx::query_scalar(
        "SELECT COUNT(*) FROM audit_events WHERE org_id = $1 AND action = 'ontology.action.execute'",
    )
    .bind(*org.as_uuid())
    .fetch_one(owner_pool)
    .await
        .unwrap()
}

async fn count_command_receipts(owner_pool: &PgPool, org: OrgId) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM ont_action_command_receipts WHERE org_id = $1")
        .bind(*org.as_uuid())
        .fetch_one(owner_pool)
        .await
        .unwrap()
}

fn create_command(object_type_id: ObjectTypeId, priority: &str) -> ActionCommand {
    ActionCommand {
        object_type_id,
        instance_id: None,
        title: Some("WO-1".to_owned()),
        params: json!({"priority": priority, "count": 5}),
        reason: Some("via action".to_owned()),
        valid_from: Some(AT),
        checklist_all_acknowledged: None,
        four_eyes_request_ref: None,
        command_id: Some(Uuid::new_v4()),
        expected_revision: None,
    }
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn execute_happy_path_appends_revision_and_one_audit_atomically(owner_pool: PgPool) {
    let rt = runtime_role_pool(&owner_pool).await;
    let cmd = command_role_pool(&owner_pool).await;
    let org = OrgId::knl();
    let actor = seed_org_and_super_admin(&owner_pool, *org.as_uuid(), "a").await;
    let type_id = seed_instance_type_with_action(
        &owner_pool,
        org,
        actor,
        "wo.exec",
        "set_priority",
        json!(["authority"]),
        json!([]),
    )
    .await;

    let outcome = console_platform_request_context::scope_org(org, async {
        state(&rt, &cmd)
            .execute_action(
                &super_admin(actor, org),
                "set_priority",
                create_command(type_id, "hi"),
            )
            .await
    })
    .await
    .expect("execute must succeed");

    assert!(outcome.gates.allow);
    let instance = outcome
        .instance
        .as_ref()
        .expect("an instance_revision dispatch returns the appended head");
    assert_eq!(instance.revision.version, 1);
    assert_eq!(instance.revision.attributes["priority"], "hi");
    assert_eq!(
        count_instances(&owner_pool, org).await,
        1,
        "exactly one instance was created"
    );
    assert_eq!(
        count_execute_audits(&owner_pool, org).await,
        1,
        "exactly one action-execute audit row landed in the same tx"
    );
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn missing_four_eyes_denies_and_writes_zero_rows(owner_pool: PgPool) {
    let rt = runtime_role_pool(&owner_pool).await;
    let cmd = command_role_pool(&owner_pool).await;
    let org = OrgId::knl();
    let actor = seed_org_and_super_admin(&owner_pool, *org.as_uuid(), "a").await;
    let type_id = seed_instance_type_with_action(
        &owner_pool,
        org,
        actor,
        "wo.foureyes",
        "set_priority",
        json!(["authority", "four_eyes"]),
        json!([]),
    )
    .await;

    let err = console_platform_request_context::scope_org(org, async {
        state(&rt, &cmd)
            .execute_action(
                &super_admin(actor, org),
                "set_priority",
                create_command(type_id, "hi"), // four_eyes_request_ref: None
            )
            .await
    })
    .await
    .expect_err("a required-but-unapproved four-eyes gate must deny");
    assert!(matches!(err, ActionError::GateDenied(_)), "got {err:?}");
    assert_eq!(
        count_instances(&owner_pool, org).await,
        0,
        "a denied gate must write zero rows"
    );

    // (c) Now record an approved four-eyes decision and pass its ref: the in-tx
    // re-check reads it and the revision commits.
    let request_ref = Uuid::new_v4();
    let approver = seed_org_and_super_admin(&owner_pool, *org.as_uuid(), "b").await;
    console_platform_request_context::scope_org(org, async {
        PgGovernanceStore::new(rt.clone())
            .decide_approval(DecideApprovalCommand {
                approver,
                request_ref,
                // Bound to the action being executed: kind = the action key,
                // target = the object type (a create has no instance target).
                kind: "set_priority".to_owned(),
                requested_by: actor,
                target_ref: Some(*type_id.as_uuid()),
                decision: ApprovalDecision::Approved,
                trace: TraceContext::generate(),
                occurred_at: AT,
            })
            .await
            .expect("record four-eyes approval");
    })
    .await;

    let mut approved = create_command(type_id, "hi");
    approved.four_eyes_request_ref = Some(request_ref);
    let outcome = console_platform_request_context::scope_org(org, async {
        state(&rt, &cmd)
            .execute_action(&super_admin(actor, org), "set_priority", approved)
            .await
    })
    .await
    .expect("an approved four-eyes gate must admit the writeback (in-tx re-check)");
    assert!(outcome.gates.allow);
    assert_eq!(count_instances(&owner_pool, org).await, 1);
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn submission_criteria_failure_denies_with_zero_rows(owner_pool: PgPool) {
    let rt = runtime_role_pool(&owner_pool).await;
    let cmd = command_role_pool(&owner_pool).await;
    let org = OrgId::knl();
    let actor = seed_org_and_super_admin(&owner_pool, *org.as_uuid(), "a").await;
    // Require count >= 10, but the command supplies count = 5.
    let type_id = seed_instance_type_with_action(
        &owner_pool,
        org,
        actor,
        "wo.crit",
        "set_priority",
        json!(["authority"]),
        json!([{"field": "count", "op": "gte", "value": 10}]),
    )
    .await;

    let err = console_platform_request_context::scope_org(org, async {
        state(&rt, &cmd)
            .execute_action(
                &super_admin(actor, org),
                "set_priority",
                create_command(type_id, "hi"),
            )
            .await
    })
    .await
    .expect_err("a failed submission criterion must deny");
    assert!(matches!(err, ActionError::CriteriaFailed(_)), "got {err:?}");
    assert_eq!(count_instances(&owner_pool, org).await, 0);
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn projected_dispatch_is_not_wired_yet_and_writes_nothing(owner_pool: PgPool) {
    let rt = runtime_role_pool(&owner_pool).await;
    let cmd = command_role_pool(&owner_pool).await;
    let org = OrgId::knl();
    let actor = seed_org_and_super_admin(&owner_pool, *org.as_uuid(), "a").await;
    let type_id =
        seed_projected_type_with_action(&owner_pool, org, actor, "equip.proj", "update_equipment")
            .await;

    let err = console_platform_request_context::scope_org(org, async {
        state(&rt, &cmd)
            .execute_action(
                &super_admin(actor, org),
                "update_equipment",
                ActionCommand {
                    object_type_id: type_id,
                    instance_id: None,
                    title: None,
                    params: json!({}),
                    reason: None,
                    valid_from: Some(AT),
                    checklist_all_acknowledged: None,
                    four_eyes_request_ref: None,
                    command_id: None,
                    expected_revision: None,
                },
            )
            .await
    })
    .await
    .expect_err("projected dispatch is a v1 stub");
    match err {
        ActionError::NotWiredYet { target } => {
            assert_eq!(target.as_deref(), Some("registry.update_equipment"));
        }
        other => panic!("expected NotWiredYet, got {other:?}"),
    }
    assert_eq!(
        count_instances(&owner_pool, org).await,
        0,
        "a not-wired projected dispatch must write nothing"
    );
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn intelligence_draft_dispatch_target_is_not_wired_yet_and_writes_nothing(
    owner_pool: PgPool,
) {
    let rt = runtime_role_pool(&owner_pool).await;
    let cmd = command_role_pool(&owner_pool).await;
    let org = OrgId::knl();
    let actor = seed_org_and_super_admin(&owner_pool, *org.as_uuid(), "a").await;
    let type_id = seed_projected_type_with_intelligence_draft(
        &owner_pool,
        org,
        actor,
        "intel.draft",
        "draft",
    )
    .await;

    let before = row_census(&owner_pool, org).await;
    let err = console_platform_request_context::scope_org(org, async {
        state(&rt, &cmd)
            .execute_action(
                &super_admin(actor, org),
                "draft",
                intelligence_draft_command(type_id),
            )
            .await
    })
    .await
    .expect_err("intelligence.draft execute is unwired");
    match err {
        ActionError::NotWiredYet { target } => {
            assert_eq!(target.as_deref(), Some("intelligence.draft"));
        }
        other => panic!("expected NotWiredYet, got {other:?}"),
    }
    assert_eq!(
        before,
        row_census(&owner_pool, org).await,
        "authority-only intelligence.draft execute must not move the census"
    );
}

/// HOLD: non-roster preflight is dry-run Ok / would_execute; execute is still NotWiredYet.
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn intelligence_draft_preflight_of_non_roster_string_is_ok(owner_pool: PgPool) {
    let rt = runtime_role_pool(&owner_pool).await;
    let cmd = command_role_pool(&owner_pool).await;
    let org = OrgId::knl();
    let actor = seed_org_and_super_admin(&owner_pool, *org.as_uuid(), "a").await;
    let type_id = seed_projected_type_with_intelligence_draft(
        &owner_pool,
        org,
        actor,
        "intel.draft.pre",
        "draft",
    )
    .await;

    let before = row_census(&owner_pool, org).await;
    let outcome = console_platform_request_context::scope_org(org, async {
        state(&rt, &cmd)
            .preflight_action(
                &super_admin(actor, org),
                "draft",
                intelligence_draft_command(type_id),
            )
            .await
    })
    .await
    .expect("non-roster intelligence.draft preflight is dry-run Ok");
    assert!(
        outcome.would_execute,
        "SuperAdmin + authority-only + empty criteria must report would_execute: {outcome:?}"
    );
    let after_preflight = row_census(&owner_pool, org).await;
    assert_eq!(
        before, after_preflight,
        "intelligence.draft preflight must not move the census"
    );

    let err = console_platform_request_context::scope_org(org, async {
        state(&rt, &cmd)
            .execute_action(
                &super_admin(actor, org),
                "draft",
                intelligence_draft_command(type_id),
            )
            .await
    })
    .await
    .expect_err("intelligence.draft execute is still unwired");
    match err {
        ActionError::NotWiredYet { target } => {
            assert_eq!(target.as_deref(), Some("intelligence.draft"));
        }
        other => panic!("expected NotWiredYet, got {other:?}"),
    }
    assert_eq!(
        after_preflight,
        row_census(&owner_pool, org).await,
        "authority-only intelligence.draft execute must not move the census"
    );
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn cross_org_action_is_invisible(owner_pool: PgPool) {
    let rt = runtime_role_pool(&owner_pool).await;
    let cmd = command_role_pool(&owner_pool).await;
    let org_a = OrgId::knl();
    let org_b = OrgId::from_uuid(ORG_B);
    let actor_a = seed_org_and_super_admin(&owner_pool, *org_a.as_uuid(), "a").await;
    let actor_b = seed_org_and_super_admin(&owner_pool, *org_b.as_uuid(), "b").await;
    let type_a = seed_instance_type_with_action(
        &owner_pool,
        org_a,
        actor_a,
        "wo.a",
        "set_priority",
        json!(["authority"]),
        json!([]),
    )
    .await;

    // Under org-B's GUC, org-A's action type does not resolve → NotFound.
    let err = console_platform_request_context::scope_org(org_b, async {
        state(&rt, &cmd)
            .execute_action(
                &super_admin(actor_b, org_b),
                "set_priority",
                create_command(type_a, "hi"),
            )
            .await
    })
    .await
    .expect_err("org-A's action must be invisible to org-B");
    assert!(matches!(err, ActionError::NotFound), "got {err:?}");
    assert_eq!(count_instances(&owner_pool, org_b).await, 0);
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn command_receipt_replays_and_stale_editor_cannot_append(owner_pool: PgPool) {
    let rt = runtime_role_pool(&owner_pool).await;
    let cmd = command_role_pool(&owner_pool).await;
    let org = OrgId::knl();
    let actor = seed_org_and_super_admin(&owner_pool, *org.as_uuid(), "a").await;
    let type_id = seed_instance_type_with_action(
        &owner_pool,
        org,
        actor,
        "wo.command",
        "set_priority",
        json!(["authority"]),
        json!([]),
    )
    .await;
    // The only test in this file that acts on an EXISTING instance, and an action
    // now reads its target through the object-policy gate. Visibility is
    // deny-by-default, so without a permit the edit below is `NotFound` before it
    // reaches the command receipt it exists to assert on. The other eight tests
    // are creates (`instance_id: None`), which the gate does not touch.
    attach_enforced_view_permit(
        &owner_pool,
        *org.as_uuid(),
        *type_id.as_uuid(),
        "wo.command",
    )
    .await;
    let created = console_platform_request_context::scope_org(org, async {
        state(&rt, &cmd)
            .execute_action(
                &super_admin(actor, org),
                "set_priority",
                create_command(type_id, "lo"),
            )
            .await
    })
    .await
    .unwrap();
    let instance_id = created.instance.unwrap().instance.id;

    let command_id = Uuid::new_v4();
    let edit = ActionCommand {
        object_type_id: type_id,
        instance_id: Some(instance_id),
        title: None,
        params: json!({"priority": "hi", "count": 5}),
        reason: Some("editor A".to_owned()),
        valid_from: Some(AT + time::Duration::seconds(1)),
        checklist_all_acknowledged: None,
        four_eyes_request_ref: None,
        command_id: Some(command_id),
        expected_revision: Some(1),
    };
    let first = console_platform_request_context::scope_org(org, async {
        state(&rt, &cmd)
            .execute_action(&super_admin(actor, org), "set_priority", edit.clone())
            .await
    })
    .await
    .unwrap();
    let replay = console_platform_request_context::scope_org(org, async {
        state(&rt, &cmd)
            .execute_action(&super_admin(actor, org), "set_priority", edit)
            .await
    })
    .await
    .unwrap();
    assert_eq!(
        first.receipt.unwrap().payload_digest,
        replay.receipt.unwrap().payload_digest
    );
    assert_eq!(replay.instance.unwrap().revision.version, 2);

    let mismatch = ActionCommand {
        object_type_id: type_id,
        instance_id: Some(instance_id),
        title: None,
        params: json!({"priority": "lo", "count": 5}),
        reason: Some("editor A changed payload".to_owned()),
        valid_from: Some(AT + time::Duration::seconds(1)),
        checklist_all_acknowledged: None,
        four_eyes_request_ref: None,
        command_id: Some(command_id),
        expected_revision: Some(1),
    };
    assert!(
        console_platform_request_context::scope_org(org, async {
            state(&rt, &cmd)
                .execute_action(&super_admin(actor, org), "set_priority", mismatch)
                .await
        })
        .await
        .is_err()
    );

    let stale = ActionCommand {
        object_type_id: type_id,
        instance_id: Some(instance_id),
        title: None,
        params: json!({"priority": "lo", "count": 5}),
        reason: Some("editor B".to_owned()),
        valid_from: Some(AT + time::Duration::seconds(2)),
        checklist_all_acknowledged: None,
        four_eyes_request_ref: None,
        command_id: Some(Uuid::new_v4()),
        expected_revision: Some(1),
    };
    assert!(
        console_platform_request_context::scope_org(org, async {
            state(&rt, &cmd)
                .execute_action(&super_admin(actor, org), "set_priority", stale)
                .await
        })
        .await
        .is_err()
    );
    assert_eq!(
        count_execute_audits(&owner_pool, org).await,
        2,
        "create + one edit only"
    );
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn command_receipt_rejects_same_org_cross_principal_reuse(owner_pool: PgPool) {
    let rt = runtime_role_pool(&owner_pool).await;
    let cmd = command_role_pool(&owner_pool).await;
    let org = OrgId::knl();
    let actor_a = seed_org_and_super_admin(&owner_pool, *org.as_uuid(), "a").await;
    let actor_b = seed_org_and_super_admin(&owner_pool, *org.as_uuid(), "b").await;
    let type_id = seed_instance_type_with_action(
        &owner_pool,
        org,
        actor_a,
        "wo.command.principal",
        "set_priority",
        json!(["authority"]),
        json!([]),
    )
    .await;
    let command_id = Uuid::new_v4();
    let command = ActionCommand {
        command_id: Some(command_id),
        ..create_command(type_id, "hi")
    };

    console_platform_request_context::scope_org(org, async {
        state(&rt, &cmd)
            .execute_action(&super_admin(actor_a, org), "set_priority", command.clone())
            .await
    })
    .await
    .expect("the command owner must be able to execute it");

    let err = console_platform_request_context::scope_org(org, async {
        state(&rt, &cmd)
            .execute_action(&super_admin(actor_b, org), "set_priority", command)
            .await
    })
    .await
    .expect_err("a different principal must not replay another principal's command");
    assert!(
        format!("{err:?}").contains("another principal"),
        "got {err:?}"
    );
    assert_eq!(count_instances(&owner_pool, org).await, 1);
    assert_eq!(count_execute_audits(&owner_pool, org).await, 1);
    assert_eq!(count_command_receipts(&owner_pool, org).await, 1);
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn concurrent_same_command_creates_one_revision_audit_and_receipt(owner_pool: PgPool) {
    let rt = runtime_role_pool(&owner_pool).await;
    let cmd = command_role_pool(&owner_pool).await;
    let org = OrgId::knl();
    let actor = seed_org_and_super_admin(&owner_pool, *org.as_uuid(), "a").await;
    let type_id = seed_instance_type_with_action(
        &owner_pool,
        org,
        actor,
        "wo.command.concurrent",
        "set_priority",
        json!(["authority"]),
        json!([]),
    )
    .await;
    let command = create_command(type_id, "hi");
    let first_command = command.clone();

    let first = console_platform_request_context::scope_org(org, async {
        state(&rt, &cmd)
            .execute_action(&super_admin(actor, org), "set_priority", first_command)
            .await
    });
    let second = console_platform_request_context::scope_org(org, async {
        state(&rt, &cmd)
            .execute_action(&super_admin(actor, org), "set_priority", command)
            .await
    });
    let (first, second) = tokio::join!(first, second);
    let first = first.expect("first concurrent execution must succeed");
    let second = second.expect("second concurrent execution must replay");
    assert_eq!(
        first.receipt.unwrap().payload_digest,
        second.receipt.unwrap().payload_digest
    );
    assert_eq!(count_instances(&owner_pool, org).await, 1);
    assert_eq!(count_execute_audits(&owner_pool, org).await, 1);
    assert_eq!(count_command_receipts(&owner_pool, org).await, 1);
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn command_id_can_be_reused_by_different_tenants(owner_pool: PgPool) {
    let rt = runtime_role_pool(&owner_pool).await;
    let cmd = command_role_pool(&owner_pool).await;
    let org_a = OrgId::knl();
    let org_b = OrgId::from_uuid(ORG_B);
    let actor_a = seed_org_and_super_admin(&owner_pool, *org_a.as_uuid(), "a").await;
    let actor_b = seed_org_and_super_admin(&owner_pool, *org_b.as_uuid(), "b").await;
    let type_a = seed_instance_type_with_action(
        &owner_pool,
        org_a,
        actor_a,
        "wo.command.tenant.a",
        "set_priority",
        json!(["authority"]),
        json!([]),
    )
    .await;
    let type_b = seed_instance_type_with_action(
        &owner_pool,
        org_b,
        actor_b,
        "wo.command.tenant.b",
        "set_priority",
        json!(["authority"]),
        json!([]),
    )
    .await;
    let command_id = Uuid::new_v4();
    let command_a = ActionCommand {
        command_id: Some(command_id),
        ..create_command(type_a, "hi")
    };
    let command_b = ActionCommand {
        command_id: Some(command_id),
        ..create_command(type_b, "hi")
    };

    console_platform_request_context::scope_org(org_a, async {
        state(&rt, &cmd)
            .execute_action(&super_admin(actor_a, org_a), "set_priority", command_a)
            .await
    })
    .await
    .expect("tenant A command must succeed");
    console_platform_request_context::scope_org(org_b, async {
        state(&rt, &cmd)
            .execute_action(&super_admin(actor_b, org_b), "set_priority", command_b)
            .await
    })
    .await
    .expect("tenant B may reuse tenant A's command id");

    for org in [org_a, org_b] {
        assert_eq!(count_instances(&owner_pool, org).await, 1);
        assert_eq!(count_execute_audits(&owner_pool, org).await, 1);
        assert_eq!(count_command_receipts(&owner_pool, org).await, 1);
    }
}

/// (g) PARITY, and its ONE bounded exception. Preflight exists to answer "would
/// execute proceed?", so a command execute refuses for a bad input must be
/// refused by preflight too — otherwise the console shows a green preflight for a
/// command that cannot run. Both entry points resolve the same `PreparedCommand`,
/// so that list is the whole shared input contract, not a sample of it.
///
/// The exception is `command_id` and `expected_revision`: the writeback consumes
/// them, preflight evaluates neither, and the shipped request schema requires
/// neither. Preflight must therefore ACCEPT a body without them — a console that
/// preflights before minting a command id would otherwise get a validation error
/// instead of its gate report. The second half of this test pins that divergence
/// to exactly those two inputs so it cannot quietly grow a third.
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn preflight_rejects_every_command_execute_rejects(owner_pool: PgPool) {
    let rt = runtime_role_pool(&owner_pool).await;
    let cmd = command_role_pool(&owner_pool).await;
    let org = OrgId::knl();
    let actor = seed_org_and_super_admin(&owner_pool, *org.as_uuid(), "a").await;
    let type_id = seed_instance_type_with_action(
        &owner_pool,
        org,
        actor,
        "wo.parity",
        "set_priority",
        json!(["authority"]),
        json!([]),
    )
    .await;
    attach_enforced_view_permit(&owner_pool, *org.as_uuid(), *type_id.as_uuid(), "wo.parity").await;

    // One committed instance so the edit cases address a real target.
    let created = console_platform_request_context::scope_org(org, async {
        state(&rt, &cmd)
            .execute_action(
                &super_admin(actor, org),
                "set_priority",
                create_command(type_id, "lo"),
            )
            .await
    })
    .await
    .expect("seed instance");
    let instance_id = created.instance.unwrap().instance.id;

    let cases: Vec<(&str, ActionCommand)> = vec![
        (
            "param not declared in params_schema",
            ActionCommand {
                params: json!({"priority": "hi", "undeclared": 1}),
                ..create_command(type_id, "hi")
            },
        ),
        (
            "params fail the declared schema",
            ActionCommand {
                params: json!({}),
                ..create_command(type_id, "hi")
            },
        ),
    ];

    for (label, command) in cases {
        let preflighted = console_platform_request_context::scope_org(org, async {
            state(&rt, &cmd)
                .preflight_action(&super_admin(actor, org), "set_priority", command.clone())
                .await
        })
        .await;
        assert!(
            matches!(preflighted, Err(ActionError::Validation(_))),
            "{label}: preflight must reject exactly what execute rejects, got {preflighted:?}"
        );

        let executed = console_platform_request_context::scope_org(org, async {
            state(&rt, &cmd)
                .execute_action(&super_admin(actor, org), "set_priority", command)
                .await
        })
        .await;
        assert!(
            matches!(executed, Err(ActionError::Validation(_))),
            "{label}: execute must reject with Validation, got {executed:?}"
        );
    }

    // THE BOUNDED DIVERGENCE. Each of these is a body preflight must ANSWER and
    // execute must refuse — the writeback-only inputs, and nothing else.
    let writeback_only: Vec<(&str, ActionCommand)> = vec![
        (
            "command_id missing",
            ActionCommand {
                command_id: None,
                ..create_command(type_id, "hi")
            },
        ),
        (
            "expected_revision missing for an instance edit",
            ActionCommand {
                instance_id: Some(instance_id),
                title: None,
                expected_revision: None,
                // Strictly after the seeded revision. Everything EXCEPT the
                // writeback-only input has to be genuinely executable, or this
                // case would pass by refusing for the wrong reason: preflight
                // now dry-runs the writer, and a `valid_from` equal to the
                // current revision's is a refusal that writer raises.
                valid_from: Some(AT + time::Duration::seconds(1)),
                ..create_command(type_id, "hi")
            },
        ),
    ];

    for (label, command) in writeback_only {
        let preflighted = console_platform_request_context::scope_org(org, async {
            state(&rt, &cmd)
                .preflight_action(&super_admin(actor, org), "set_priority", command.clone())
                .await
        })
        .await
        .unwrap_or_else(|e| panic!("{label}: preflight must still report the gates, got {e:?}"));
        assert!(
            preflighted.would_execute,
            "{label}: preflight evaluates neither input, so its verdict must be the \
             gate chain's: {preflighted:?}"
        );

        let executed = console_platform_request_context::scope_org(org, async {
            state(&rt, &cmd)
                .execute_action(&super_admin(actor, org), "set_priority", command)
                .await
        })
        .await;
        assert!(
            matches!(executed, Err(ActionError::Validation(_))),
            "{label}: execute must refuse with Validation, got {executed:?}"
        );
    }

    assert_eq!(
        count_instances(&owner_pool, org).await,
        1,
        "only the seeded instance exists — every refused command wrote nothing, \
         and no preflight wrote at all"
    );
    assert_eq!(count_execute_audits(&owner_pool, org).await, 1);
}

/// Every table an action command can touch: business (instance, revision, link),
/// receipt, approval consumption, audit, outbox.
///
/// The census digests each table's whole CONTENT (`string_agg` of every row's
/// text form, ordered), not its row count — an execute UPDATEs `ont_instances`
/// in place rather than inserting, and a count cannot see that. Compared as a
/// whole rather than table by table, so a write to a table nobody thought to
/// assert on still fails the equality.
async fn row_census(owner_pool: &PgPool, org: OrgId) -> Vec<(&'static str, String)> {
    // Literal SQL per table: `sqlx` refuses an interpolated query string, and the
    // digest expression is character-identical across every row of this table.
    let mut census = Vec::new();
    for (table, query) in [
        (
            "ont_instances",
            "SELECT coalesce(md5(string_agg(t::text, '|' ORDER BY t::text)), '<empty>') FROM ont_instances t WHERE org_id = $1",
        ),
        (
            "ont_instance_revisions",
            "SELECT coalesce(md5(string_agg(t::text, '|' ORDER BY t::text)), '<empty>') FROM ont_instance_revisions t WHERE org_id = $1",
        ),
        (
            "ont_links",
            "SELECT coalesce(md5(string_agg(t::text, '|' ORDER BY t::text)), '<empty>') FROM ont_links t WHERE org_id = $1",
        ),
        (
            "ont_action_command_receipts",
            "SELECT coalesce(md5(string_agg(t::text, '|' ORDER BY t::text)), '<empty>') FROM ont_action_command_receipts t WHERE org_id = $1",
        ),
        (
            "gov_approval_consumptions",
            "SELECT coalesce(md5(string_agg(t::text, '|' ORDER BY t::text)), '<empty>') FROM gov_approval_consumptions t WHERE org_id = $1",
        ),
        (
            "audit_events",
            "SELECT coalesce(md5(string_agg(t::text, '|' ORDER BY t::text)), '<empty>') FROM audit_events t WHERE org_id = $1",
        ),
        (
            "workflow_outbox_events",
            "SELECT coalesce(md5(string_agg(t::text, '|' ORDER BY t::text)), '<empty>') FROM workflow_outbox_events t WHERE org_id = $1",
        ),
    ] {
        let digest: String = sqlx::query_scalar(query)
            .bind(*org.as_uuid())
            .fetch_one(owner_pool)
            .await
            .unwrap();
        census.push((table, digest));
    }
    census
}

/// The tables whose content differs between two censuses.
fn moved(before: &[(&'static str, String)], after: &[(&'static str, String)]) -> Vec<&'static str> {
    before
        .iter()
        .zip(after)
        .filter(|((_, b), (_, a))| b != a)
        .map(|((table, _), _)| *table)
        .collect()
}

async fn count_approval_consumptions(owner_pool: &PgPool, org: OrgId) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM gov_approval_consumptions WHERE org_id = $1")
        .bind(*org.as_uuid())
        .fetch_one(owner_pool)
        .await
        .unwrap()
}

/// Record an approved four-eyes decision bound to `target` under `kind`.
async fn approve_four_eyes(
    rt: &PgPool,
    org: OrgId,
    requested_by: UserId,
    approver: UserId,
    kind: &str,
    target: Uuid,
) -> Uuid {
    let request_ref = Uuid::new_v4();
    console_platform_request_context::scope_org(org, async {
        PgGovernanceStore::new(rt.clone())
            .decide_approval(DecideApprovalCommand {
                approver,
                request_ref,
                kind: kind.to_owned(),
                requested_by,
                target_ref: Some(target),
                decision: ApprovalDecision::Approved,
                trace: TraceContext::generate(),
                occurred_at: AT,
            })
            .await
            .expect("record four-eyes approval");
    })
    .await;
    request_ref
}

/// (h) PREFLIGHT IS READ-ONLY. Driven through the SHIPPED entry point — the same
/// `preflight_action` the `POST .../preflight` route calls — with a command whose
/// gates are all SATISFIED, i.e. the one case that has something to write and a
/// live approval to spend. Zero delta across every affected table, and the
/// approval is still spendable afterwards.
///
/// The command is an EDIT of a committed instance, not a create: that is the case
/// where execute's `ont_instances` write is an in-place UPDATE (the head pointer)
/// rather than an INSERT, so the census below has to be content-sensitive to see
/// it at all. A create would have left the strongest column of the oracle untested.
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn preflight_writes_zero_rows_and_never_spends_the_approval(owner_pool: PgPool) {
    let rt = runtime_role_pool(&owner_pool).await;
    let cmd = command_role_pool(&owner_pool).await;
    let org = OrgId::knl();
    let actor = seed_org_and_super_admin(&owner_pool, *org.as_uuid(), "a").await;
    let approver = seed_org_and_super_admin(&owner_pool, *org.as_uuid(), "b").await;
    let type_id = seed_instance_type_with_action(
        &owner_pool,
        org,
        actor,
        "wo.readonly",
        "set_priority",
        json!(["authority", "four_eyes"]),
        json!([]),
    )
    .await;
    attach_enforced_view_permit(
        &owner_pool,
        *org.as_uuid(),
        *type_id.as_uuid(),
        "wo.readonly",
    )
    .await;

    // One committed instance, so the edit below addresses a real head.
    let seed_ref = approve_four_eyes(
        &rt,
        org,
        actor,
        approver,
        "set_priority",
        *type_id.as_uuid(),
    )
    .await;
    let created = console_platform_request_context::scope_org(org, async {
        state(&rt, &cmd)
            .execute_action(
                &super_admin(actor, org),
                "set_priority",
                ActionCommand {
                    four_eyes_request_ref: Some(seed_ref),
                    ..create_command(type_id, "lo")
                },
            )
            .await
    })
    .await
    .expect("seed instance");
    let instance_id = created.instance.unwrap().instance.id;

    // An EDIT's four-eyes approval binds to the instance, not the object type.
    let request_ref = approve_four_eyes(
        &rt,
        org,
        actor,
        approver,
        "set_priority",
        *instance_id.as_uuid(),
    )
    .await;
    let edit = ActionCommand {
        instance_id: Some(instance_id),
        title: None,
        expected_revision: Some(1),
        four_eyes_request_ref: Some(request_ref),
        valid_from: Some(AT + time::Duration::seconds(1)),
        ..create_command(type_id, "hi")
    };

    let before = row_census(&owner_pool, org).await;
    let outcome = console_platform_request_context::scope_org(org, async {
        state(&rt, &cmd)
            .preflight_action(
                &super_admin(actor, org),
                "set_priority",
                // Without the writeback-only inputs: this is the body a console
                // sends to render the gate report before minting a command id.
                ActionCommand {
                    command_id: None,
                    expected_revision: None,
                    ..edit.clone()
                },
            )
            .await
    })
    .await
    .expect("preflight must resolve");
    assert!(
        outcome.would_execute,
        "the admitting case is the one that could write: {outcome:?}"
    );
    let after_preflight = row_census(&owner_pool, org).await;
    assert_eq!(
        before, after_preflight,
        "preflight wrote rows — it must be read-only across business, receipt, \
         approval, audit and outbox"
    );

    // The approval it PEEKED at is still there to be spent: proof the read was
    // non-consuming, not merely that no consumption row was visible.
    let executed = console_platform_request_context::scope_org(org, async {
        state(&rt, &cmd)
            .execute_action(&super_admin(actor, org), "set_priority", edit)
            .await
    })
    .await
    .expect("the approval preflight peeked at must still admit the execute");
    assert!(executed.gates.allow);
    assert_eq!(count_approval_consumptions(&owner_pool, org).await, 2);

    // POSITIVE CONTROL for the census itself. The SAME command, now executed,
    // must move exactly this footprint — proof the census can see a write rather
    // than reading equal because it looks at nothing. `ont_instances` is the
    // load-bearing entry: this edit only UPDATEs it in place (one instance before,
    // one after), so a row-COUNT census would list every other table here and
    // still miss it.
    let after_execute = row_census(&owner_pool, org).await;
    assert_eq!(
        moved(&after_preflight, &after_execute),
        vec![
            "ont_instances",
            "ont_instance_revisions",
            "ont_action_command_receipts",
            "gov_approval_consumptions",
            "audit_events",
        ],
        "execute's write footprint changed — if the census cannot see these, the \
         zero-delta assertion above proves nothing"
    );
    assert_eq!(
        count_instances(&owner_pool, org).await,
        1,
        "the edit moved ont_instances WITHOUT changing its row count — which is \
         exactly what a count-only census cannot see"
    );
}

/// (k) THE DRY RUN IS ACTUALLY DRY. Preparation resolves the action's `edits`;
/// only the WRITEBACK judges what they resolved to, against the object type's
/// property schema and field kinds. A preflight that skips that judgement reports
/// `would_execute: true` for an edit set the writeback then refuses — a false
/// green in the product, and worse than no dry run because operators trust it
/// (DESIGN.md §4-42 puts 시뮬레이션 on the write path precisely so they can).
///
/// `priority` is declared `choice`, so the writer's `check_field_shape` requires
/// a string. A NUMERIC `priority` param passes `validate_params` (which checks
/// declaration and presence, not type) and passes `apply_edits` (which resolves
/// writes and does not type them), so the refusal is reachable ONLY by running
/// the writer. Both dispatch branches are covered: an edit
/// (`stage_revision_in_tx`) and a create (`create_instance_in_tx`).
///
/// And the simulation persists NOTHING — the same content census as (h), over a
/// REJECTED edit set and an ACCEPTED one. (h) proves the accepted case still
/// leaves the four-eyes approval spendable; this one proves the accepted case is
/// still byte-identical now that preflight opens a real write transaction to
/// answer it.
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn preflight_refuses_an_edit_set_the_writeback_refuses(owner_pool: PgPool) {
    let rt = runtime_role_pool(&owner_pool).await;
    let cmd = command_role_pool(&owner_pool).await;
    let org = OrgId::knl();
    let actor = seed_org_and_super_admin(&owner_pool, *org.as_uuid(), "a").await;
    let type_id = seed_instance_type_with_action(
        &owner_pool,
        org,
        actor,
        "wo.dryrun",
        "set_priority",
        json!(["authority"]),
        json!([]),
    )
    .await;
    attach_enforced_view_permit(&owner_pool, *org.as_uuid(), *type_id.as_uuid(), "wo.dryrun").await;

    // One committed instance so the EDIT case addresses a real head.
    let created = console_platform_request_context::scope_org(org, async {
        state(&rt, &cmd)
            .execute_action(
                &super_admin(actor, org),
                "set_priority",
                create_command(type_id, "lo"),
            )
            .await
    })
    .await
    .expect("seed instance");
    let instance_id = created.instance.unwrap().instance.id;

    // A number into a `choice` property: refused by the writer, by nothing before it.
    let bad_params = json!({"priority": 7, "count": 5});
    let rejected: Vec<(&str, ActionCommand)> = vec![
        (
            "create with a wrongly-typed edit",
            ActionCommand {
                params: bad_params.clone(),
                ..create_command(type_id, "hi")
            },
        ),
        (
            "edit with a wrongly-typed edit",
            ActionCommand {
                instance_id: Some(instance_id),
                title: None,
                expected_revision: Some(1),
                valid_from: Some(AT + time::Duration::seconds(1)),
                params: bad_params,
                ..create_command(type_id, "hi")
            },
        ),
    ];

    for (label, command) in rejected {
        let before = row_census(&owner_pool, org).await;
        let preflighted = console_platform_request_context::scope_org(org, async {
            state(&rt, &cmd)
                .preflight_action(&super_admin(actor, org), "set_priority", command.clone())
                .await
        })
        .await;
        let preflight_error = preflighted
            .as_ref()
            .err()
            .map(|e| format!("{e:?}"))
            .unwrap_or_else(|| {
                panic!("{label}: preflight reported an executable command: {preflighted:?}")
            });
        assert!(
            preflight_error.contains("wrong type for field kind"),
            "{label}: the refusal must be the WRITER's, not some earlier one: \
             {preflight_error}"
        );
        assert_eq!(
            before,
            row_census(&owner_pool, org).await,
            "{label}: the refused simulation persisted rows — §4-42 requires zero \
             side effects, and the dry run's transaction must roll back"
        );

        // PARITY, as the identical error value: preflight is not merely refusing
        // too, it is refusing with exactly what execute refuses with, so the
        // console renders one status and one message for one command.
        let executed = console_platform_request_context::scope_org(org, async {
            state(&rt, &cmd)
                .execute_action(&super_admin(actor, org), "set_priority", command)
                .await
        })
        .await;
        assert_eq!(
            preflight_error,
            format!("{:?}", executed.as_ref().unwrap_err()),
            "{label}: preflight and execute must refuse identically, got \
             {executed:?}"
        );
        assert_eq!(
            before,
            row_census(&owner_pool, org).await,
            "{label}: the refused EXECUTE persisted rows"
        );
    }

    // The ACCEPTED edit set: still reported executable, and still byte-identical
    // afterwards even though answering it now opens a write transaction.
    let accepted = ActionCommand {
        instance_id: Some(instance_id),
        title: None,
        expected_revision: Some(1),
        valid_from: Some(AT + time::Duration::seconds(1)),
        ..create_command(type_id, "hi")
    };
    let before_accepted = row_census(&owner_pool, org).await;
    let outcome = console_platform_request_context::scope_org(org, async {
        state(&rt, &cmd)
            .preflight_action(
                &super_admin(actor, org),
                "set_priority",
                ActionCommand {
                    command_id: None,
                    expected_revision: None,
                    ..accepted.clone()
                },
            )
            .await
    })
    .await
    .expect("a valid edit set must still preflight");
    assert!(outcome.would_execute, "{outcome:?}");
    assert_eq!(
        before_accepted,
        row_census(&owner_pool, org).await,
        "the ACCEPTED simulation persisted rows — the rollback is what makes the \
         dry run dry, and it is load-bearing on exactly this case"
    );

    // POSITIVE CONTROL for the census: the same command, executed, must move it.
    // Without this, every equality above could be reading nothing at all.
    let after_dry = row_census(&owner_pool, org).await;
    console_platform_request_context::scope_org(org, async {
        state(&rt, &cmd)
            .execute_action(&super_admin(actor, org), "set_priority", accepted)
            .await
    })
    .await
    .expect("the command the dry run admitted must execute");
    assert_eq!(
        moved(&after_dry, &row_census(&owner_pool, org).await),
        vec![
            "ont_instances",
            "ont_instance_revisions",
            "ont_action_command_receipts",
            "audit_events",
        ],
        "the census cannot see this write, so it could not have seen a dry-run \
         write either"
    );
}

/// (i) A FAILED MUTATION NEVER SPENDS THE APPROVAL. The four-eyes consume happens
/// inside the owner transaction, so a chain that denies AFTER the consume must
/// roll the consumption back with everything else. Silently spending an approval
/// here costs the user a second round of approvals for a command that never ran.
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn a_failed_mutation_never_spends_the_approval(owner_pool: PgPool) {
    let rt = runtime_role_pool(&owner_pool).await;
    let cmd = command_role_pool(&owner_pool).await;
    let org = OrgId::knl();
    let actor = seed_org_and_super_admin(&owner_pool, *org.as_uuid(), "a").await;
    let approver = seed_org_and_super_admin(&owner_pool, *org.as_uuid(), "b").await;
    // self_checklist sits AFTER the four-eyes consume in the writeback, so an
    // unacknowledged checklist denies once the approval has already been spent
    // inside the tx — exactly the ordering that must roll back.
    let type_id = seed_instance_type_with_action(
        &owner_pool,
        org,
        actor,
        "wo.unspent",
        "set_priority",
        json!(["authority", "self_checklist", "four_eyes"]),
        json!([]),
    )
    .await;
    let request_ref = approve_four_eyes(
        &rt,
        org,
        actor,
        approver,
        "set_priority",
        *type_id.as_uuid(),
    )
    .await;

    let err = console_platform_request_context::scope_org(org, async {
        state(&rt, &cmd)
            .execute_action(
                &super_admin(actor, org),
                "set_priority",
                ActionCommand {
                    four_eyes_request_ref: Some(request_ref),
                    checklist_all_acknowledged: None,
                    ..create_command(type_id, "hi")
                },
            )
            .await
    })
    .await
    .expect_err("an unacknowledged checklist must deny");
    assert!(matches!(err, ActionError::GateDenied(_)), "got {err:?}");
    assert_eq!(count_instances(&owner_pool, org).await, 0);
    assert_eq!(count_command_receipts(&owner_pool, org).await, 0);
    assert_eq!(
        count_approval_consumptions(&owner_pool, org).await,
        0,
        "the rolled-back mutation must leave the approval UNSPENT"
    );

    // Unspent means genuinely re-usable, not merely unrecorded.
    let retried = console_platform_request_context::scope_org(org, async {
        state(&rt, &cmd)
            .execute_action(
                &super_admin(actor, org),
                "set_priority",
                ActionCommand {
                    four_eyes_request_ref: Some(request_ref),
                    checklist_all_acknowledged: Some(true),
                    ..create_command(type_id, "hi")
                },
            )
            .await
    })
    .await
    .expect("the unspent approval must still admit the corrected retry");
    assert!(retried.gates.allow);
    assert_eq!(count_instances(&owner_pool, org).await, 1);
    assert_eq!(count_approval_consumptions(&owner_pool, org).await, 1);
}

/// (j) A REPLAY RETURNS THE STORED RECEIPT. Proven without tampering (the receipt
/// row is DB-immutable): the head is MOVED between the first execution and the
/// replay, so a recomputed receipt could not possibly still describe v1 with the
/// original attributes. A changed digest or a changed actor under the same
/// command id conflicts instead of replaying.
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn a_replay_returns_the_stored_receipt_not_a_recomputed_one(owner_pool: PgPool) {
    let rt = runtime_role_pool(&owner_pool).await;
    let cmd = command_role_pool(&owner_pool).await;
    let org = OrgId::knl();
    let actor = seed_org_and_super_admin(&owner_pool, *org.as_uuid(), "a").await;
    let other = seed_org_and_super_admin(&owner_pool, *org.as_uuid(), "b").await;
    let type_id = seed_instance_type_with_action(
        &owner_pool,
        org,
        actor,
        "wo.replay",
        "set_priority",
        json!(["authority"]),
        json!([]),
    )
    .await;
    attach_enforced_view_permit(&owner_pool, *org.as_uuid(), *type_id.as_uuid(), "wo.replay").await;

    let create = create_command(type_id, "lo");
    let first = console_platform_request_context::scope_org(org, async {
        state(&rt, &cmd)
            .execute_action(&super_admin(actor, org), "set_priority", create.clone())
            .await
    })
    .await
    .expect("first execution");
    let instance_id = first.instance.clone().unwrap().instance.id;

    // Move the head so a recomputed receipt could not match the stored one.
    console_platform_request_context::scope_org(org, async {
        state(&rt, &cmd)
            .execute_action(
                &super_admin(actor, org),
                "set_priority",
                ActionCommand {
                    instance_id: Some(instance_id),
                    title: None,
                    expected_revision: Some(1),
                    params: json!({"priority": "hi", "count": 5}),
                    valid_from: Some(AT + time::Duration::seconds(1)),
                    ..create_command(type_id, "hi")
                },
            )
            .await
    })
    .await
    .expect("the head moves to v2 under a different command id");

    let replay = console_platform_request_context::scope_org(org, async {
        state(&rt, &cmd)
            .execute_action(&super_admin(actor, org), "set_priority", create.clone())
            .await
    })
    .await
    .expect("the same command id + actor + digest must replay");
    let stored = first.receipt.clone().unwrap();
    assert_eq!(
        serde_json::to_value(&stored).unwrap(),
        serde_json::to_value(replay.receipt.as_ref().unwrap()).unwrap(),
        "the replay must return the STORED receipt byte-for-byte"
    );
    assert_eq!(
        replay.instance.clone().unwrap().revision.version,
        1,
        "the stored receipt still describes v1 even though the live head is v2"
    );
    assert_eq!(
        replay.instance.unwrap().revision.attributes["priority"],
        "lo"
    );
    assert_eq!(count_instances(&owner_pool, org).await, 1);
    assert_eq!(count_execute_audits(&owner_pool, org).await, 2);
    assert_eq!(count_command_receipts(&owner_pool, org).await, 2);

    // A CHANGED DIGEST under the same command id conflicts.
    let changed_digest = console_platform_request_context::scope_org(org, async {
        state(&rt, &cmd)
            .execute_action(
                &super_admin(actor, org),
                "set_priority",
                ActionCommand {
                    params: json!({"priority": "hi", "count": 5}),
                    ..create.clone()
                },
            )
            .await
    })
    .await
    .expect_err("a changed payload must not replay");
    assert!(
        format!("{changed_digest:?}").contains("different payload"),
        "got {changed_digest:?}"
    );

    // A CHANGED ACTOR under the same command id conflicts.
    let changed_actor = console_platform_request_context::scope_org(org, async {
        state(&rt, &cmd)
            .execute_action(&super_admin(other, org), "set_priority", create)
            .await
    })
    .await
    .expect_err("another principal must not replay this command");
    assert!(
        format!("{changed_actor:?}").contains("another principal"),
        "got {changed_actor:?}"
    );

    assert_eq!(count_instances(&owner_pool, org).await, 1);
    assert_eq!(count_execute_audits(&owner_pool, org).await, 2);
    assert_eq!(count_command_receipts(&owner_pool, org).await, 2);
}
