#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! RUNTIME proof for the no-code "add-a-type" gap ① fix, exercised under the
//! effective `console_rt` and `console_ontology_cmd` roles (NOSUPERUSER, NOBYPASSRLS).
//! The test pools authenticate as the sqlx test owner and use `SET ROLE`, so
//! this proves effective-role permissions and RLS behavior, not direct login.
//!
//! Coverage-matrix finding (§B.2 step 1): a type authored through the no-code
//! Ontology Manager ships with `actions: []`; with no create-capable action
//! there is no way to ever create an instance (there is no direct
//! `POST /instances`). Publishing must auto-attach a generic `create` action
//! so the no-code loop (draft → publish → create instance) closes with zero
//! engineering.
//!
//! Proves, end to end, the acceptance path: publish a user-authored draft type
//! with no actions → a `create` action (instance_revision dispatch) exists on
//! the published head → executing it creates an instance immediately.

use console_governance_adapter_postgres::PgGovernanceStore;
use console_governance_application::{
    ApprovalDecision, CreateApprovalCommand, DecideApprovalCommand,
};
use console_kernel_core::{BranchScope, OrgId, TraceContext, UserId};
use console_ontology_adapter_postgres::instances::PgInstanceStore;
use console_ontology_adapter_postgres::{
    CreateObjectTypeDraft, ObjectTypeSummary, PgOntologyStore, PropertyDefInput,
};
use console_ontology_domain::{ActionDispatch, BackingKind, SchemaLifecycleState};
use console_ontology_rest::{ActionCommand, OntologyRestState};
use console_platform_authz::{Principal, Role};
use console_platform_test_support::{runtime_role_pool, seed_org_and_super_admin};
use serde_json::json;
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use std::collections::BTreeSet;
use time::macros::datetime;
use uuid::Uuid;

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

async fn assert_effective_role(owner_pool: &PgPool, role_pool: &PgPool, expected_role: &str) {
    let expected_session_user: String = sqlx::query_scalar("SELECT session_user::text")
        .fetch_one(owner_pool)
        .await
        .unwrap();
    let (current_user, session_user, is_superuser, bypasses_rls): (String, String, bool, bool) =
        sqlx::query_as(
            "SELECT current_user::text, session_user::text, rolsuper, rolbypassrls \
             FROM pg_roles WHERE rolname = current_user",
        )
        .fetch_one(role_pool)
        .await
        .unwrap();

    assert_eq!(
        current_user, expected_role,
        "SET ROLE must select the expected effective identity"
    );
    assert_eq!(
        session_user, expected_session_user,
        "session_user remains the sqlx test owner; this is an effective-role proof"
    );
    assert_ne!(
        current_user, session_user,
        "the effective role must not be mistaken for the authenticated session identity"
    );
    assert!(!is_superuser, "effective role must be NOSUPERUSER");
    assert!(!bypasses_rls, "effective role must be NOBYPASSRLS");
}

fn state(pool: &PgPool, command_pool: &PgPool) -> OntologyRestState {
    OntologyRestState::new(
        PgOntologyStore::new(pool.clone()).with_command_pool(command_pool.clone()),
        PgInstanceStore::new(pool.clone()),
        console_governance_adapter_postgres::PgGovernanceStore::new(pool.clone()),
        None,
    )
}

async fn publish_with_four_eyes(
    store: &PgOntologyStore,
    governance: &PgGovernanceStore,
    actor: UserId,
    approver: UserId,
    created: &ObjectTypeSummary,
) -> ObjectTypeSummary {
    let reviewed = store
        .transition_lifecycle(
            actor,
            created.id,
            created.write_precondition(),
            SchemaLifecycleState::ReviewPending,
            true,
            TraceContext::generate(),
            AT,
        )
        .await
        .expect("draft must enter review before publication");

    let request_ref = Uuid::new_v4();
    governance
        .create_approval(CreateApprovalCommand {
            requester: actor,
            request_ref,
            kind: "ontology.schema.publish".to_owned(),
            target_ref: Some(*created.id.as_uuid()),
            payload_summary: json!({"key_revision": reviewed.key_write_revision}),
            trace: TraceContext::generate(),
            occurred_at: AT,
        })
        .await
        .expect("publication approval request must be recorded");
    governance
        .decide_approval(DecideApprovalCommand {
            approver,
            request_ref,
            kind: "ontology.schema.publish".to_owned(),
            requested_by: actor,
            target_ref: Some(*created.id.as_uuid()),
            decision: ApprovalDecision::Approved,
            trace: TraceContext::generate(),
            occurred_at: AT,
        })
        .await
        .expect("a distinct reviewer must approve publication");

    store
        .transition_lifecycle(
            actor,
            created.id,
            reviewed.write_precondition(),
            SchemaLifecycleState::Published,
            true,
            TraceContext::generate(),
            AT,
        )
        .await
        .expect("reviewed and approved draft must publish")
}

/// A no-code draft exactly as the Ontology Manager's 타입 추가 flow builds one
/// today: `actions: []` (`model.ts:121`) — no create-capable action.
fn no_code_draft(stable_key: &str) -> CreateObjectTypeDraft {
    CreateObjectTypeDraft {
        stable_key: stable_key.to_owned(),
        title: "핸드오버 정책".to_owned(),
        title_property_key: Some("policy_name".to_owned()),
        backing_kind: BackingKind::Instance,
        backing_table: None,
        primary_key_property: None,
        properties: vec![PropertyDefInput {
            key: "policy_name".to_owned(),
            title: "정책명".to_owned(),
            field_type: "text".to_owned(),
            config: json!({}),
            backing_column: None,
            required: true,
            in_property_policy: false,
        }],
        links: Vec::new(),
        actions: Vec::new(),
        analytics: Vec::new(),
    }
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn publish_auto_attaches_create_action_and_instance_creation_works(owner_pool: PgPool) {
    let rt = runtime_role_pool(&owner_pool).await;
    let cmd = command_role_pool(&owner_pool).await;
    assert_effective_role(&owner_pool, &rt, "console_rt").await;
    assert_effective_role(&owner_pool, &cmd, "console_ontology_cmd").await;
    let org = OrgId::knl();
    let actor = seed_org_and_super_admin(&owner_pool, *org.as_uuid(), "a").await;
    let approver = seed_org_and_super_admin(&owner_pool, *org.as_uuid(), "a-reviewer").await;

    let (draft_actions_len, published) = console_platform_request_context::scope_org(org, async {
        let store = PgOntologyStore::new(rt.clone()).with_command_pool(cmd.clone());
        let created = store
            .create_object_type(
                actor,
                no_code_draft("handover_policy"),
                TraceContext::generate(),
                AT,
            )
            .await
            .expect("no-code draft create must succeed under effective console_rt permissions");

        // Confirm the draft truly ships with zero actions, matching the FE's
        // no-code 타입 추가 flow (the gap this fix closes).
        let draft_detail = store
            .get_object_type("handover_policy", None)
            .await
            .unwrap();
        let draft_actions_len = draft_detail.actions.len();

        let governance = PgGovernanceStore::new(rt.clone());
        let published =
            publish_with_four_eyes(&store, &governance, actor, approver, &created).await;
        (draft_actions_len, published)
    })
    .await;

    assert_eq!(draft_actions_len, 0, "the draft ships with no actions");
    assert_eq!(published.lifecycle_state, SchemaLifecycleState::Published);

    // The published head now carries an auto-attached create action.
    let create_action = console_platform_request_context::scope_org(org, async {
        let store = PgOntologyStore::new(rt.clone());
        let detail = store
            .get_object_type("handover_policy", None)
            .await
            .unwrap();
        assert_eq!(
            detail.actions.len(),
            1,
            "publish must auto-attach exactly one create action"
        );
        let action = detail.actions[0].clone();
        assert_eq!(action.stable_key, "create");
        assert_eq!(action.dispatch, ActionDispatch::InstanceRevision);
        assert_eq!(
            action.params_schema["policy_name"]["required"],
            json!(true),
            "auto-attached action params derive from the property schema"
        );
        action
    })
    .await;
    let _ = create_action;

    // Acceptance path: creating an instance works immediately — zero
    // engineering required after publish.
    let outcome = console_platform_request_context::scope_org(org, async {
        state(&rt, &cmd)
            .execute_action(
                &super_admin(actor, org),
                "create",
                ActionCommand {
                    object_type_id: published.id,
                    instance_id: None,
                    title: Some("HO-1".to_owned()),
                    params: json!({"policy_name": "야간 인수인계"}),
                    reason: Some("no-code create".to_owned()),
                    valid_from: Some(AT),
                    checklist_all_acknowledged: None,
                    four_eyes_request_ref: None,
                    command_id: Some(Uuid::new_v4()),
                    expected_revision: None,
                },
            )
            .await
    })
    .await
    .expect("the auto-attached create action must execute successfully");

    assert!(outcome.gates.allow);
    let instance = outcome
        .instance
        .as_ref()
        .expect("an instance_revision dispatch returns the appended head");
    assert_eq!(instance.revision.version, 1);
    assert_eq!(
        instance.revision.attributes["policy_name"],
        json!("야간 인수인계")
    );

    let instance_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM ont_instances WHERE org_id = $1")
            .bind(*org.as_uuid())
            .fetch_one(&owner_pool)
            .await
            .unwrap();
    assert_eq!(instance_count, 1, "exactly one instance was created");
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn publish_does_not_duplicate_an_existing_create_capable_action(owner_pool: PgPool) {
    let rt = runtime_role_pool(&owner_pool).await;
    let cmd = command_role_pool(&owner_pool).await;
    assert_effective_role(&owner_pool, &rt, "console_rt").await;
    assert_effective_role(&owner_pool, &cmd, "console_ontology_cmd").await;
    let org = OrgId::knl();
    let actor = seed_org_and_super_admin(&owner_pool, *org.as_uuid(), "b").await;
    let approver = seed_org_and_super_admin(&owner_pool, *org.as_uuid(), "b-reviewer").await;

    let mut draft = no_code_draft("with_action");
    draft.actions = vec![console_ontology_adapter_postgres::ActionTypeInput {
        stable_key: "make".to_owned(),
        title: "생성".to_owned(),
        params_schema: json!({"policy_name": {"required": true}}),
        edits: json!([{"property": "policy_name", "param": "policy_name"}]),
        submission_criteria: json!([]),
        side_effects: json!([]),
        dispatch: ActionDispatch::InstanceRevision,
        dispatch_target: None,
        control_points: json!(["authority"]),
    }];

    let detail = console_platform_request_context::scope_org(org, async {
        let store = PgOntologyStore::new(rt.clone()).with_command_pool(cmd.clone());
        let created = store
            .create_object_type(actor, draft, TraceContext::generate(), AT)
            .await
            .unwrap();
        let governance = PgGovernanceStore::new(rt.clone());
        publish_with_four_eyes(&store, &governance, actor, approver, &created).await;
        store.get_object_type("with_action", None).await.unwrap()
    })
    .await;

    assert_eq!(
        detail.actions.len(),
        1,
        "publish must not attach a second create action when one already exists"
    );
    assert_eq!(detail.actions[0].stable_key, "make");
}
