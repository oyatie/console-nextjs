#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! RUNTIME RLS gate for the ontology registry read/write paths.
//!
//! Every registry mutation wraps `with_audit` (arms `app.current_org`) and every
//! read wraps `with_org_conn`. A static gate proves the wrapping is present in
//! source; THIS test proves it WORKS AT RUNTIME as the genuine non-owner
//! `console_rt` role (NOSUPERUSER, NOBYPASSRLS, FORCE RLS) — the only faithful
//! exercise of the org_isolation policy. The default `#[sqlx::test]` pool is a
//! BYPASSRLS superuser that would green-light a totally broken policy.
//!
//! Proves, with two tenants A and B:
//!   (a) under org-A's armed GUC, A sees its own object type (list + get);
//!   (b) under org-A's armed GUC, B's object type is INVISIBLE (get → not found,
//!       list → zero B rows) — cross-tenant isolation holds under RLS as console_rt;
//!   (c) FAIL-CLOSED: with NO GUC armed the read errors (MissingOrg), never leaks;
//!   (d) the schema-lifecycle FSM advances draft → review_pending → published as
//!       console_rt, and a v+1 revision stages independently of the published head.

use console_kernel_core::{ErrorKind, OrgId, TraceContext, UserId};
use console_ontology_adapter_postgres::{
    ActionTypeInput, AnalyticInput, CreateObjectTypeDraft, LinkTypeInput, ObjectTypeSummary,
    ObjectTypeWritePrecondition, PgOntologyError, PgOntologyStore, PropertyDefInput,
};
use console_ontology_domain::{ActionDispatch, BackingKind, LinkCardinality, SchemaLifecycleState};
use sqlx::PgPool;
use sqlx::pool::PoolConnection;
use sqlx::postgres::PgPoolOptions;
use sqlx::{Postgres, Row};
use std::time::Duration;
use time::macros::datetime;
use uuid::Uuid;

const ORG_B: Uuid = Uuid::from_u128(0x2222_2222_2222_2222_2222_2222_2222_2222);

/// Every connection becomes the genuine non-owner `console_rt` (BYPASSRLS does not
/// apply, FORCE RLS does). The migration itself grants console_rt the registry
/// privileges, so no manual GRANT is needed here.
async fn runtime_role_pool(owner_pool: &PgPool) -> PgPool {
    let options = owner_pool.connect_options().as_ref().clone();
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

async fn command_role_pool_with_stage_select_barrier(owner_pool: &PgPool) -> PgPool {
    let options = owner_pool.connect_options().as_ref().clone();
    PgPoolOptions::new()
        .max_connections(4)
        .after_connect(|conn, _meta| {
            Box::pin(async move {
                sqlx::query("SET ROLE console_ontology_cmd")
                    .execute(&mut *conn)
                    .await?;
                sqlx::query("SET app.test_stage_select_barrier = 'on'")
                    .execute(&mut *conn)
                    .await?;
                Ok(())
            })
        })
        .connect_with(options)
        .await
        .unwrap()
}

async fn hold_advisory_gate(owner_pool: &PgPool, key: i64) -> PoolConnection<Postgres> {
    let mut conn = owner_pool.acquire().await.unwrap();
    sqlx::query("SELECT pg_advisory_lock($1)")
        .bind(key)
        .execute(&mut *conn)
        .await
        .unwrap();
    conn
}

async fn release_advisory_gate(mut conn: PoolConnection<Postgres>, key: i64) {
    let released: bool = sqlx::query_scalar("SELECT pg_advisory_unlock($1)")
        .bind(key)
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    assert!(
        released,
        "test advisory gate must be owned by its control connection"
    );
}

async fn advisory_waiter_count(owner_pool: &PgPool) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM pg_locks WHERE locktype = 'advisory' AND NOT granted")
        .fetch_one(owner_pool)
        .await
        .unwrap()
}

async fn wait_for_advisory_waiters(owner_pool: &PgPool, minimum: i64) {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if advisory_waiter_count(owner_pool).await >= minimum {
                return;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("database advisory-lock barrier was never reached");
}

fn revision_draft(stable_key: &str, title: &str, extra_property: &str) -> CreateObjectTypeDraft {
    let mut draft = work_order_draft(stable_key);
    draft.title = title.to_owned();
    draft.properties.push(PropertyDefInput {
        key: extra_property.to_owned(),
        title: extra_property.to_owned(),
        field_type: "text".to_owned(),
        config: serde_json::json!({}),
        backing_column: None,
        required: false,
        in_property_policy: false,
    });
    draft
}

async fn current_precondition(
    store: &PgOntologyStore,
    stable_key: &str,
) -> ObjectTypeWritePrecondition {
    store
        .get_object_type(stable_key, None)
        .await
        .unwrap()
        .object_type
        .write_precondition()
}

async fn publish_with_four_eyes(
    owner_pool: &PgPool,
    store: &PgOntologyStore,
    org: OrgId,
    actor: UserId,
    approver: UserId,
    summary: &ObjectTypeSummary,
    at: time::OffsetDateTime,
) -> ObjectTypeSummary {
    let reviewed = store
        .transition_lifecycle(
            actor,
            summary.id,
            summary.write_precondition(),
            SchemaLifecycleState::ReviewPending,
            true,
            TraceContext::generate(),
            at,
        )
        .await
        .unwrap();
    let request_ref = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO gov_approval_requests (org_id, request_ref, kind, requested_by, payload_summary, target_ref) VALUES ($1, $2, 'ontology.schema.publish', $3, jsonb_build_object('key_revision', $4::bigint), $5)",
    )
    .bind(*org.as_uuid())
    .bind(request_ref)
    .bind(*actor.as_uuid())
    .bind(reviewed.key_write_revision)
    .bind(*summary.id.as_uuid())
    .execute(owner_pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO gov_approvals (org_id, request_ref, kind, requested_by, approver_id, decision, target_ref) VALUES ($1, $2, 'ontology.schema.publish', $3, $4, 'approved', $5)",
    )
    .bind(*org.as_uuid())
    .bind(request_ref)
    .bind(*actor.as_uuid())
    .bind(*approver.as_uuid())
    .bind(*summary.id.as_uuid())
    .execute(owner_pool)
    .await
    .unwrap();

    store
        .transition_lifecycle(
            actor,
            summary.id,
            reviewed.write_precondition(),
            SchemaLifecycleState::Published,
            true,
            TraceContext::generate(),
            at,
        )
        .await
        .unwrap()
}

async fn seed_org_and_user(owner_pool: &PgPool, org: Uuid, tag: &str) -> UserId {
    // Slug must match the organizations slug CHECK (no dots/underscores), so
    // derive a sanitized, per-org-unique slug rather than echoing the tag.
    let slug = format!("org-{}", &org.simple().to_string()[..12]);
    sqlx::query("INSERT INTO organizations (id, slug, name) VALUES ($1, $2, $3) ON CONFLICT (id) DO NOTHING")
        .bind(org)
        .bind(slug)
        .bind(format!("Org {tag}"))
        .execute(owner_pool)
        .await
        .unwrap();
    let user_id = UserId::new();
    sqlx::query("INSERT INTO users (id, display_name, roles, org_id) VALUES ($1, $2, $3, $4)")
        .bind(*user_id.as_uuid())
        .bind(format!("Admin {tag}"))
        .bind(["SUPER_ADMIN"].as_slice())
        .bind(org)
        .execute(owner_pool)
        .await
        .unwrap();
    user_id
}

fn work_order_draft(stable_key: &str) -> CreateObjectTypeDraft {
    CreateObjectTypeDraft {
        stable_key: stable_key.to_owned(),
        title: "작업지시".to_owned(),
        title_property_key: Some("title".to_owned()),
        backing_kind: BackingKind::Instance,
        backing_table: None,
        primary_key_property: None,
        properties: vec![PropertyDefInput {
            key: "priority".to_owned(),
            title: "우선순위".to_owned(),
            field_type: "choice".to_owned(),
            config: serde_json::json!({"choices": [{"id": "hi", "name": "높음", "color": "red"}]}),
            backing_column: None,
            required: true,
            in_property_policy: false,
        }],
        links: vec![LinkTypeInput {
            stable_key: "assigned_to".to_owned(),
            title: "담당자".to_owned(),
            reverse_title: Some("담당 작업".to_owned()),
            to_object_type_id: None,
            cardinality: LinkCardinality::ManyMany,
            traversable: true,
        }],
        actions: Vec::new(),
        analytics: Vec::new(),
    }
}

/// Create a draft object type for `org` under the armed GUC (owner pool, so the
/// write succeeds; the GUC is armed exactly as the org middleware would).
async fn seed_object_type(owner_pool: &PgPool, org: OrgId, stable_key: &str) {
    let actor = seed_org_and_user(owner_pool, *org.as_uuid(), stable_key).await;
    console_platform_request_context::scope_org(org, async {
        let store = PgOntologyStore::new(owner_pool.clone())
            .with_command_pool(command_role_pool(owner_pool).await);
        store
            .create_object_type(
                actor,
                work_order_draft(stable_key),
                TraceContext::generate(),
                datetime!(2026-07-09 12:00 UTC),
            )
            .await
            .expect("create_object_type must succeed under armed owner pool");
    })
    .await;
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn own_object_type_is_visible_as_runtime_role(owner_pool: PgPool) {
    let rt_pool = runtime_role_pool(&owner_pool).await;
    let org_a = OrgId::knl();
    seed_object_type(&owner_pool, org_a, "wo.work_order").await;

    let (list, detail) = console_platform_request_context::scope_org(org_a, async {
        let store = PgOntologyStore::new(rt_pool.clone())
            .with_command_pool(command_role_pool(&owner_pool).await);
        let list = store.list_object_types().await.unwrap();
        let detail = store.get_object_type("wo.work_order", None).await.unwrap();
        (list, detail)
    })
    .await;

    assert_eq!(list.len(), 1, "org-A sees exactly its own object type");
    assert_eq!(list[0].stable_key, "wo.work_order");
    assert_eq!(list[0].lifecycle_state, SchemaLifecycleState::Draft);
    assert_eq!(detail.properties.len(), 1);
    assert_eq!(detail.properties[0].key, "priority");
    assert!(detail.properties[0].field_kind.is_known());
    // Reverse (back-)link name round-trips through create → read (change-log 74).
    assert_eq!(detail.links.len(), 1);
    assert_eq!(detail.links[0].stable_key, "assigned_to");
    assert_eq!(detail.links[0].reverse_title.as_deref(), Some("담당 작업"));
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn cross_tenant_object_type_is_invisible_as_runtime_role(owner_pool: PgPool) {
    let rt_pool = runtime_role_pool(&owner_pool).await;
    let org_a = OrgId::knl();
    let org_b = OrgId::from_uuid(ORG_B);
    seed_object_type(&owner_pool, org_a, "wo.work_order").await;
    seed_object_type(&owner_pool, org_b, "wo.b_secret").await;

    let (list, cross) = console_platform_request_context::scope_org(org_a, async {
        let store = PgOntologyStore::new(rt_pool.clone())
            .with_command_pool(command_role_pool(&owner_pool).await);
        let list = store.list_object_types().await.unwrap();
        let cross = store.get_object_type("wo.b_secret", None).await;
        (list, cross)
    })
    .await;

    // A's list never contains B's rows.
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].stable_key, "wo.work_order");
    // B's type is not found under A's GUC (RLS isolates tenants as console_rt).
    assert!(
        cross.is_err(),
        "org-B's object type must be INVISIBLE under org-A's GUC as console_rt"
    );
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn registry_read_fails_closed_without_org_as_runtime_role(owner_pool: PgPool) {
    let rt_pool = runtime_role_pool(&owner_pool).await;
    seed_object_type(&owner_pool, OrgId::knl(), "wo.work_order").await;

    // No scope_org wrapper: current_org() is unset, so the read must fail closed.
    let store = PgOntologyStore::new(rt_pool.clone())
        .with_command_pool(command_role_pool(&owner_pool).await);
    assert!(
        store.list_object_types().await.is_err(),
        "with no org armed the list must fail closed, never leak"
    );
    assert!(
        store.get_object_type("wo.work_order", None).await.is_err(),
        "with no org armed the get must fail closed, never leak"
    );
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn lifecycle_fsm_and_revision_staging_as_runtime_role(owner_pool: PgPool) {
    let rt_pool = runtime_role_pool(&owner_pool).await;
    let org_a = OrgId::knl();
    let actor = seed_org_and_user(&owner_pool, *org_a.as_uuid(), "fsm").await;
    let approver = seed_org_and_user(&owner_pool, *org_a.as_uuid(), "fsm-approver").await;
    let at = datetime!(2026-07-09 12:00 UTC);

    console_platform_request_context::scope_org(org_a, async {
        let store = PgOntologyStore::new(rt_pool.clone())
            .with_command_pool(command_role_pool(&owner_pool).await);

        // Create v1 draft.
        let v1 = store
            .create_object_type(
                actor,
                work_order_draft("wo.fsm"),
                TraceContext::generate(),
                at,
            )
            .await
            .unwrap();
        assert_eq!(v1.schema_version, 1);
        assert_eq!(v1.lifecycle_state, SchemaLifecycleState::Draft);

        // Protection ON forbids the draft→published shortcut.
        let blocked = store
            .transition_lifecycle(
                actor,
                v1.id,
                v1.write_precondition(),
                SchemaLifecycleState::Published,
                true,
                TraceContext::generate(),
                at,
            )
            .await;
        assert!(
            blocked.is_err(),
            "direct draft→published must be forbidden under protection"
        );

        // The reviewed path publishes: draft → review_pending → published.
        let published =
            publish_with_four_eyes(&owner_pool, &store, org_a, actor, approver, &v1, at).await;
        assert_eq!(published.lifecycle_state, SchemaLifecycleState::Published);

        // Stage a v+1 revision; the published head is untouched (immutable history).
        let v2 = store
            .stage_revision(
                actor,
                "wo.fsm",
                published.write_precondition(),
                work_order_draft("wo.fsm"),
                TraceContext::generate(),
                at,
            )
            .await
            .unwrap();
        assert_eq!(v2.schema_version, 2);
        assert_eq!(v2.lifecycle_state, SchemaLifecycleState::Draft);

        // The head returned by get (no version) is still the published v1.
        let head = store.get_object_type("wo.fsm", None).await.unwrap();
        assert_eq!(head.object_type.schema_version, 1);
        assert_eq!(
            head.object_type.lifecycle_state,
            SchemaLifecycleState::Published
        );

        // Publishing v2 through independent review supersedes v1 atomically.
        publish_with_four_eyes(&owner_pool, &store, org_a, actor, approver, &v2, at).await;
        let head2 = store.get_object_type("wo.fsm", None).await.unwrap();
        assert_eq!(head2.object_type.schema_version, 2);
        assert_eq!(
            head2.object_type.lifecycle_state,
            SchemaLifecycleState::Published
        );

        // v1 is now superseded, still fetchable as-of.
        let as_of_v1 = store.get_object_type("wo.fsm", Some(1)).await.unwrap();
        assert_eq!(
            as_of_v1.object_type.lifecycle_state,
            SchemaLifecycleState::Superseded
        );
    })
    .await;
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn draft_child_identity_replay_is_idempotent_and_divergent_reuse_conflicts(
    owner_pool: PgPool,
) {
    let org = OrgId::knl();
    let actor = seed_org_and_user(&owner_pool, *org.as_uuid(), "child-reuse").await;
    let rt_pool = runtime_role_pool(&owner_pool).await;
    let at = datetime!(2026-07-19 09:45 UTC);
    let mut canonical = work_order_draft("wo.child_reuse");
    canonical.actions.push(ActionTypeInput {
        stable_key: "close_inspection".to_owned(),
        title: "Close inspection".to_owned(),
        params_schema: serde_json::json!({}),
        edits: serde_json::json!([]),
        submission_criteria: serde_json::json!([]),
        side_effects: serde_json::json!([]),
        dispatch: ActionDispatch::InstanceRevision,
        dispatch_target: None,
        control_points: serde_json::json!([]),
    });
    canonical.analytics.push(AnalyticInput {
        key: "inspection_score".to_owned(),
        title: "Inspection score".to_owned(),
        formula: serde_json::json!({"expression": "score()"}),
        result_type: serde_json::json!({"type": "number"}),
    });

    console_platform_request_context::scope_org(org, async {
        let store =
            PgOntologyStore::new(rt_pool).with_command_pool(command_role_pool(&owner_pool).await);
        store
            .create_object_type(actor, canonical.clone(), TraceContext::generate(), at)
            .await
            .expect("canonical draft");

        store
            .stage_revision(
                actor,
                "wo.child_reuse",
                current_precondition(&store, "wo.child_reuse").await,
                canonical.clone(),
                TraceContext::generate(),
                at,
            )
            .await
            .expect("same-key same-payload replay must be idempotent");

        let mut divergent_drafts = Vec::new();
        let mut property = canonical.clone();
        property.properties[0].title = "Divergent property".to_owned();
        divergent_drafts.push(("property", property));
        let mut link = canonical.clone();
        link.links[0].title = "Divergent link".to_owned();
        divergent_drafts.push(("link", link));
        let mut action = canonical.clone();
        action.actions[0].title = "Divergent action".to_owned();
        divergent_drafts.push(("action", action));
        let mut analytic = canonical.clone();
        analytic.analytics[0].title = "Divergent analytic".to_owned();
        divergent_drafts.push(("analytic", analytic));

        for (kind, divergent) in divergent_drafts {
            let error = store
                .stage_revision(
                    actor,
                    "wo.child_reuse",
                    current_precondition(&store, "wo.child_reuse").await,
                    divergent,
                    TraceContext::generate(),
                    at,
                )
                .await
                .expect_err("same child key with divergent payload must conflict");
            assert!(
                matches!(
                    error,
                    PgOntologyError::Domain(ref kernel)
                        if kernel.kind == ErrorKind::Conflict
                ),
                "{kind} reuse must surface a typed conflict, got {error:?}",
            );
        }

        let detail = store
            .get_object_type("wo.child_reuse", None)
            .await
            .expect("read canonical draft");
        assert_eq!(detail.properties.len(), 1);
        assert_eq!(detail.links.len(), 1);
        assert_eq!(detail.actions.len(), 1);
        assert_eq!(detail.analytics.len(), 1);
    })
    .await;
}

#[derive(Debug, Clone, Copy)]
enum DuplicateChildKind {
    Property,
    Link,
    Action,
    Analytic,
}

impl DuplicateChildKind {
    const ALL: [Self; 4] = [Self::Property, Self::Link, Self::Action, Self::Analytic];

    const fn slug(self) -> &'static str {
        match self {
            Self::Property => "property",
            Self::Link => "link",
            Self::Action => "action",
            Self::Analytic => "analytic",
        }
    }
}

fn push_duplicate_child(
    draft: &mut CreateObjectTypeDraft,
    kind: DuplicateChildKind,
    key: &str,
    divergent: bool,
) {
    let second_title = if divergent {
        "Different definition"
    } else {
        "Canonical definition"
    };
    match kind {
        DuplicateChildKind::Property => {
            let first = PropertyDefInput {
                key: key.to_owned(),
                title: "Canonical definition".to_owned(),
                field_type: "text".to_owned(),
                config: serde_json::json!({}),
                backing_column: None,
                required: false,
                in_property_policy: false,
            };
            draft.properties.push(first.clone());
            draft.properties.push(PropertyDefInput {
                title: second_title.to_owned(),
                ..first
            });
        }
        DuplicateChildKind::Link => {
            let first = LinkTypeInput {
                stable_key: key.to_owned(),
                title: "Canonical definition".to_owned(),
                reverse_title: None,
                to_object_type_id: None,
                cardinality: LinkCardinality::OneMany,
                traversable: true,
            };
            draft.links.push(first.clone());
            draft.links.push(LinkTypeInput {
                title: second_title.to_owned(),
                ..first
            });
        }
        DuplicateChildKind::Action => {
            let first = ActionTypeInput {
                stable_key: key.to_owned(),
                title: "Canonical definition".to_owned(),
                params_schema: serde_json::json!({}),
                edits: serde_json::json!([]),
                submission_criteria: serde_json::json!([]),
                side_effects: serde_json::json!([]),
                dispatch: ActionDispatch::InstanceRevision,
                dispatch_target: None,
                control_points: serde_json::json!([]),
            };
            draft.actions.push(first.clone());
            draft.actions.push(ActionTypeInput {
                title: second_title.to_owned(),
                ..first
            });
        }
        DuplicateChildKind::Analytic => {
            let first = AnalyticInput {
                key: key.to_owned(),
                title: "Canonical definition".to_owned(),
                formula: serde_json::json!({"expression": "score()"}),
                result_type: serde_json::json!({"type": "number"}),
            };
            draft.analytics.push(first.clone());
            draft.analytics.push(AnalyticInput {
                title: second_title.to_owned(),
                ..first
            });
        }
    }
}

async fn object_type_storage_snapshot(
    owner_pool: &PgPool,
    org: OrgId,
    stable_key: &str,
) -> serde_json::Value {
    sqlx::query_scalar(
        r#"
        SELECT CASE WHEN EXISTS (
            SELECT 1 FROM ont_object_types WHERE org_id = $1 AND stable_key = $2
        ) THEN jsonb_build_object(
            'object_types', (
                SELECT jsonb_agg(to_jsonb(o) ORDER BY o.schema_version)
                FROM ont_object_types o
                WHERE o.org_id = $1 AND o.stable_key = $2
            ),
            'properties', COALESCE((
                SELECT jsonb_agg(to_jsonb(p) ORDER BY p.object_type_id, p.key)
                FROM ont_property_defs p
                WHERE p.org_id = $1 AND p.object_type_id IN (
                    SELECT id FROM ont_object_types WHERE org_id = $1 AND stable_key = $2
                )
            ), '[]'::jsonb),
            'links', COALESCE((
                SELECT jsonb_agg(to_jsonb(l) ORDER BY l.object_type_id, l.stable_key)
                FROM ont_link_types l
                WHERE l.org_id = $1 AND l.object_type_id IN (
                    SELECT id FROM ont_object_types WHERE org_id = $1 AND stable_key = $2
                )
            ), '[]'::jsonb),
            'actions', COALESCE((
                SELECT jsonb_agg(to_jsonb(a) ORDER BY a.object_type_id, a.stable_key)
                FROM ont_action_types a
                WHERE a.org_id = $1 AND a.object_type_id IN (
                    SELECT id FROM ont_object_types WHERE org_id = $1 AND stable_key = $2
                )
            ), '[]'::jsonb),
            'analytics', COALESCE((
                SELECT jsonb_agg(to_jsonb(n) ORDER BY n.object_type_id, n.key)
                FROM ont_analytics n
                WHERE n.org_id = $1 AND n.object_type_id IN (
                    SELECT id FROM ont_object_types WHERE org_id = $1 AND stable_key = $2
                )
            ), '[]'::jsonb)
        ) ELSE 'null'::jsonb END
        "#,
    )
    .bind(*org.as_uuid())
    .bind(stable_key)
    .fetch_one(owner_pool)
    .await
    .unwrap()
}

async fn org_audit_count(owner_pool: &PgPool, org: OrgId) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM audit_events WHERE org_id = $1")
        .bind(*org.as_uuid())
        .fetch_one(owner_pool)
        .await
        .unwrap()
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn duplicate_child_identities_inside_one_draft_are_rejected_before_storage(
    owner_pool: PgPool,
) {
    let org = OrgId::knl();
    let actor = seed_org_and_user(&owner_pool, *org.as_uuid(), "child-duplicates").await;
    let rt_pool = runtime_role_pool(&owner_pool).await;
    let at = datetime!(2026-07-19 10:30 UTC);

    console_platform_request_context::scope_org(org, async {
        let store =
            PgOntologyStore::new(rt_pool).with_command_pool(command_role_pool(&owner_pool).await);

        for kind in DuplicateChildKind::ALL {
            let slug = kind.slug();
            for divergent in [false, true] {
                let variant = if divergent { "divergent" } else { "equal" };
                let fresh_key = format!("wo.{slug}_{variant}_fresh");
                let mut fresh = work_order_draft(&fresh_key);
                push_duplicate_child(&mut fresh, kind, &format!("new_{slug}"), divergent);
                let audits_before = org_audit_count(&owner_pool, org).await;
                let error = store
                    .create_object_type(actor, fresh, TraceContext::generate(), at)
                    .await
                    .expect_err("duplicate child identity must fail validation");
                assert!(
                    matches!(
                        error,
                        PgOntologyError::Domain(ref kernel)
                            if kernel.kind == ErrorKind::Validation
                    ),
                    "{kind:?} {variant} fresh duplicate returned {error:?}",
                );
                assert_eq!(
                    object_type_storage_snapshot(&owner_pool, org, &fresh_key).await,
                    serde_json::Value::Null,
                    "{kind:?} {variant} fresh duplicate must create no storage",
                );
                assert_eq!(
                    org_audit_count(&owner_pool, org).await,
                    audits_before,
                    "{kind:?} {variant} fresh duplicate must emit no audit",
                );
            }

            let append_key = format!("wo.{slug}_append");
            let base = work_order_draft(&append_key);
            store
                .create_object_type(actor, base.clone(), TraceContext::generate(), at)
                .await
                .unwrap();
            for divergent in [false, true] {
                let variant = if divergent { "divergent" } else { "equal" };
                let mut append = base.clone();
                push_duplicate_child(&mut append, kind, &format!("append_{slug}"), divergent);
                let storage_before =
                    object_type_storage_snapshot(&owner_pool, org, &append_key).await;
                let audits_before = org_audit_count(&owner_pool, org).await;
                let error = store
                    .stage_revision(
                        actor,
                        &append_key,
                        current_precondition(&store, &append_key).await,
                        append,
                        TraceContext::generate(),
                        at,
                    )
                    .await
                    .expect_err("duplicate child identity must fail validation");
                assert!(
                    matches!(
                        error,
                        PgOntologyError::Domain(ref kernel)
                            if kernel.kind == ErrorKind::Validation
                    ),
                    "{kind:?} {variant} append duplicate returned {error:?}",
                );
                assert_eq!(
                    object_type_storage_snapshot(&owner_pool, org, &append_key).await,
                    storage_before,
                    "{kind:?} {variant} append duplicate must not mutate storage",
                );
                assert_eq!(
                    org_audit_count(&owner_pool, org).await,
                    audits_before,
                    "{kind:?} {variant} append duplicate must emit no audit",
                );
            }
        }
    })
    .await;
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn analytic_string_formula_is_rejected_instead_of_silently_erased(owner_pool: PgPool) {
    let org = OrgId::knl();
    let actor = seed_org_and_user(&owner_pool, *org.as_uuid(), "analytic-formula").await;
    let rt_pool = runtime_role_pool(&owner_pool).await;
    let at = datetime!(2026-07-19 10:15 UTC);
    let mut draft = work_order_draft("wo.analytic_formula");
    draft.analytics.push(AnalyticInput {
        key: "delay_days".to_owned(),
        title: "Delay days".to_owned(),
        formula: serde_json::json!("days_between(due_date, now())"),
        result_type: serde_json::json!({"type": "number"}),
    });

    console_platform_request_context::scope_org(org, async {
        let store =
            PgOntologyStore::new(rt_pool).with_command_pool(command_role_pool(&owner_pool).await);
        let error = store
            .create_object_type(actor, draft, TraceContext::generate(), at)
            .await
            .expect_err("non-object formula must be rejected before storage");
        assert!(
            matches!(
                error,
                PgOntologyError::Domain(ref kernel)
                    if kernel.kind == ErrorKind::Validation
            ),
            "formula shape must fail with typed validation, got {error:?}",
        );
    })
    .await;
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn concurrent_stage_stage_has_one_cas_winner_as_runtime_role(owner_pool: PgPool) {
    const INSERT_GATE: i64 = 4_730_012;
    let org = OrgId::knl();
    let actor = seed_org_and_user(&owner_pool, *org.as_uuid(), "stage-stage").await;
    let approver = seed_org_and_user(&owner_pool, *org.as_uuid(), "stage-stage-approver").await;
    let rt_pool = runtime_role_pool(&owner_pool).await;
    let at = datetime!(2026-07-19 06:30 UTC);
    seed_object_type(&owner_pool, OrgId::from_uuid(ORG_B), "wo.concurrent_stage").await;

    let v1 = console_platform_request_context::scope_org(org, async {
        let store = PgOntologyStore::new(rt_pool.clone())
            .with_command_pool(command_role_pool(&owner_pool).await);
        let v1 = store
            .create_object_type(
                actor,
                work_order_draft("wo.concurrent_stage"),
                TraceContext::generate(),
                at,
            )
            .await
            .unwrap();
        publish_with_four_eyes(&owner_pool, &store, org, actor, approver, &v1, at).await
    })
    .await;
    let expected = v1.write_precondition();

    sqlx::raw_sql(
        r#"
        CREATE FUNCTION test_block_first_revision_insert() RETURNS trigger
        LANGUAGE plpgsql AS $$
        BEGIN
            IF NEW.title = 'first hostile stage' THEN
                PERFORM pg_advisory_xact_lock(4730012);
            END IF;
            RETURN NEW;
        END
        $$;
        CREATE TRIGGER test_block_first_revision_insert
        BEFORE INSERT ON ont_object_types
        FOR EACH ROW EXECUTE FUNCTION test_block_first_revision_insert();
        "#,
    )
    .execute(&owner_pool)
    .await
    .unwrap();

    let gate = hold_advisory_gate(&owner_pool, INSERT_GATE).await;
    let first_pool = rt_pool.clone();
    let first_cmd_pool = command_role_pool(&owner_pool).await;
    let first = tokio::spawn(console_platform_request_context::scope_org(
        org,
        async move {
            PgOntologyStore::new(first_pool)
                .with_command_pool(first_cmd_pool)
                .stage_revision(
                    actor,
                    "wo.concurrent_stage",
                    expected,
                    revision_draft("wo.concurrent_stage", "first hostile stage", "from_first"),
                    TraceContext::generate(),
                    at,
                )
                .await
        },
    ));
    wait_for_advisory_waiters(&owner_pool, 1).await;

    let second_pool = rt_pool.clone();
    let second_cmd_pool = command_role_pool(&owner_pool).await;
    let second = tokio::spawn(console_platform_request_context::scope_org(
        org,
        async move {
            PgOntologyStore::new(second_pool)
                .with_command_pool(second_cmd_pool)
                .stage_revision(
                    actor,
                    "wo.concurrent_stage",
                    expected,
                    revision_draft("wo.concurrent_stage", "second hostile stage", "from_second"),
                    TraceContext::generate(),
                    at,
                )
                .await
        },
    ));

    tokio::task::yield_now().await;
    assert!(
        !second.is_finished(),
        "the second same-token stage must wait while the winner holds the key transaction"
    );
    release_advisory_gate(gate, INSERT_GATE).await;

    let outcomes = [first.await.unwrap(), second.await.unwrap()];
    assert_eq!(outcomes.iter().filter(|result| result.is_ok()).count(), 1);
    assert_eq!(
        outcomes
            .iter()
            .filter(|result| matches!(result, Err(PgOntologyError::PreconditionFailed { .. })))
            .count(),
        1,
        "exactly one same-token stage must lose without mutation"
    );
    let winner = outcomes
        .iter()
        .find_map(|result| result.as_ref().ok())
        .unwrap();
    assert_eq!(winner.schema_version, 2);
    assert_eq!(winner.key_write_revision, v1.key_write_revision + 1);

    let rows = sqlx::query(
        "SELECT id, schema_version, lifecycle_state FROM ont_object_types WHERE org_id = $1 AND stable_key = 'wo.concurrent_stage' ORDER BY schema_version",
    )
    .bind(*org.as_uuid())
    .fetch_all(&owner_pool)
    .await
    .unwrap();
    assert_eq!(rows.len(), 2, "published v1 plus exactly one draft v2");
    assert_eq!(rows[0].try_get::<i64, _>("schema_version").unwrap(), 1);
    assert_eq!(
        rows[0].try_get::<String, _>("lifecycle_state").unwrap(),
        "published"
    );
    assert_eq!(rows[1].try_get::<i64, _>("schema_version").unwrap(), 2);
    assert_eq!(
        rows[1].try_get::<String, _>("lifecycle_state").unwrap(),
        "draft"
    );
    let v2_id: Uuid = rows[1].try_get("id").unwrap();
    let child_keys: Vec<String> = sqlx::query_scalar(
        "SELECT key FROM ont_property_defs WHERE object_type_id = $1 ORDER BY key",
    )
    .bind(v2_id)
    .fetch_all(&owner_pool)
    .await
    .unwrap();
    assert_eq!(child_keys.len(), 2);
    assert!(child_keys.iter().any(|key| key == "priority"));
    assert!(
        child_keys.iter().any(|key| key == "from_first")
            ^ child_keys.iter().any(|key| key == "from_second"),
        "only the CAS winner may append its child"
    );
    let stage_targets: Vec<String> = sqlx::query_scalar(
        "SELECT target_id FROM audit_events WHERE org_id = $1 AND action = 'ontology.object_type.stage_revision' ORDER BY created_at, id",
    )
    .bind(*org.as_uuid())
    .fetch_all(&owner_pool)
    .await
    .unwrap();
    assert_eq!(stage_targets, vec![v2_id.to_string()]);
    assert_ne!(v1.id.as_uuid(), &v2_id);
    let cross_tenant_row = sqlx::query(
        "SELECT schema_version, lifecycle_state FROM ont_object_types WHERE org_id = $1 AND stable_key = 'wo.concurrent_stage'",
    )
    .bind(ORG_B)
    .fetch_one(&owner_pool)
    .await
    .unwrap();
    assert_eq!(
        cross_tenant_row
            .try_get::<i64, _>("schema_version")
            .unwrap(),
        1
    );
    assert_eq!(
        cross_tenant_row
            .try_get::<String, _>("lifecycle_state")
            .unwrap(),
        "draft"
    );
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn concurrent_stage_review_has_one_cas_winner_as_runtime_role(owner_pool: PgPool) {
    const SELECT_GATE: i64 = 4_730_013;
    let org = OrgId::knl();
    let actor = seed_org_and_user(&owner_pool, *org.as_uuid(), "stage-publish").await;
    let approver = seed_org_and_user(&owner_pool, *org.as_uuid(), "stage-publish-approver").await;
    let rt_pool = runtime_role_pool(&owner_pool).await;
    let at = datetime!(2026-07-19 06:31 UTC);
    seed_object_type(
        &owner_pool,
        OrgId::from_uuid(ORG_B),
        "wo.concurrent_publish",
    )
    .await;

    let v2 = console_platform_request_context::scope_org(org, async {
        let store = PgOntologyStore::new(rt_pool.clone())
            .with_command_pool(command_role_pool(&owner_pool).await);
        let v1 = store
            .create_object_type(
                actor,
                work_order_draft("wo.concurrent_publish"),
                TraceContext::generate(),
                at,
            )
            .await
            .unwrap();
        let published =
            publish_with_four_eyes(&owner_pool, &store, org, actor, approver, &v1, at).await;
        store
            .stage_revision(
                actor,
                "wo.concurrent_publish",
                published.write_precondition(),
                revision_draft(
                    "wo.concurrent_publish",
                    "revision before publish",
                    "before_publish",
                ),
                TraceContext::generate(),
                at,
            )
            .await
            .unwrap()
    })
    .await;
    let expected = v2.write_precondition();

    sqlx::raw_sql(
        r#"
        CREATE FUNCTION test_stage_select_barrier(row_key text, row_state text) RETURNS boolean
        LANGUAGE plpgsql VOLATILE AS $$
        BEGIN
            IF current_setting('app.test_stage_select_barrier', true) = 'on'
               AND row_key = 'wo.concurrent_publish'
               AND row_state IN ('draft', 'review_pending') THEN
                PERFORM pg_advisory_xact_lock(4730013);
            END IF;
            RETURN true;
        END
        $$;
        DROP POLICY org_isolation ON ont_object_types;
        CREATE POLICY org_isolation ON ont_object_types
        USING (
            org_id = NULLIF(current_setting('app.current_org', true), '')::uuid
            AND test_stage_select_barrier(stable_key, lifecycle_state)
        )
        WITH CHECK (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid);
        "#,
    )
    .execute(&owner_pool)
    .await
    .unwrap();
    let stage_pool = rt_pool.clone();
    let stage_cmd_pool = command_role_pool_with_stage_select_barrier(&owner_pool).await;
    let gate = hold_advisory_gate(&owner_pool, SELECT_GATE).await;
    let mut staged_draft = revision_draft(
        "wo.concurrent_publish",
        "hostile stage edit",
        "before_publish",
    );
    staged_draft.properties.push(PropertyDefInput {
        key: "after_publish".to_owned(),
        title: "after_publish".to_owned(),
        field_type: "text".to_owned(),
        config: serde_json::json!({}),
        backing_column: None,
        required: false,
        in_property_policy: false,
    });

    let stage = tokio::spawn(console_platform_request_context::scope_org(
        org,
        async move {
            PgOntologyStore::new(stage_pool)
                .with_command_pool(stage_cmd_pool)
                .stage_revision(
                    actor,
                    "wo.concurrent_publish",
                    expected,
                    staged_draft,
                    TraceContext::generate(),
                    at,
                )
                .await
        },
    ));
    wait_for_advisory_waiters(&owner_pool, 1).await;

    let review_pool = rt_pool.clone();
    let review_cmd_pool = command_role_pool(&owner_pool).await;
    let review = tokio::spawn(console_platform_request_context::scope_org(
        org,
        async move {
            PgOntologyStore::new(review_pool)
                .with_command_pool(review_cmd_pool)
                .transition_lifecycle(
                    actor,
                    v2.id,
                    expected,
                    SchemaLifecycleState::ReviewPending,
                    true,
                    TraceContext::generate(),
                    at,
                )
                .await
        },
    ));

    tokio::task::yield_now().await;
    assert!(
        !review.is_finished(),
        "the same-token review submission must wait for the staged key lock"
    );
    release_advisory_gate(gate, SELECT_GATE).await;

    let staged = stage
        .await
        .unwrap()
        .expect("the operation that already advanced the key token must finish");
    let review_error = review
        .await
        .unwrap()
        .expect_err("the same-token review submission must lose without mutating");
    assert!(
        matches!(
            review_error,
            PgOntologyError::PreconditionFailed { ref current }
                if current.revision == staged.key_write_revision
                    && current.etag == staged.key_write_etag
        ),
        "stale review submission must return the current key validator"
    );
    assert_eq!(staged.id, v2.id);

    let final_row = sqlx::query(
        "SELECT title, schema_version, lifecycle_state FROM ont_object_types WHERE id = $1",
    )
    .bind(*v2.id.as_uuid())
    .fetch_one(&owner_pool)
    .await
    .unwrap();
    assert_eq!(
        final_row.try_get::<String, _>("title").unwrap(),
        staged.title
    );
    assert_eq!(final_row.try_get::<i64, _>("schema_version").unwrap(), 2);
    assert_eq!(
        final_row.try_get::<String, _>("lifecycle_state").unwrap(),
        "draft"
    );
    let child_keys: Vec<String> = sqlx::query_scalar(
        "SELECT key FROM ont_property_defs WHERE object_type_id = $1 ORDER BY key",
    )
    .bind(*v2.id.as_uuid())
    .fetch_all(&owner_pool)
    .await
    .unwrap();
    assert_eq!(
        child_keys,
        vec!["after_publish", "before_publish", "priority"]
    );
    let audit_targets: Vec<(String, String)> = sqlx::query_as(
        "SELECT action, target_id FROM audit_events WHERE org_id = $1 AND action IN ('ontology.object_type.stage_revision', 'ontology.object_type.transition') AND target_id = $2 ORDER BY created_at, id",
    )
    .bind(*org.as_uuid())
    .bind(v2.id.to_string())
    .fetch_all(&owner_pool)
    .await
    .unwrap();
    assert_eq!(
        audit_targets,
        vec![
            (
                "ontology.object_type.stage_revision".to_owned(),
                v2.id.to_string()
            ),
            (
                "ontology.object_type.stage_revision".to_owned(),
                v2.id.to_string()
            ),
        ]
    );
    let cross_tenant_row = sqlx::query(
        "SELECT schema_version, lifecycle_state FROM ont_object_types WHERE org_id = $1 AND stable_key = 'wo.concurrent_publish'",
    )
    .bind(ORG_B)
    .fetch_one(&owner_pool)
    .await
    .unwrap();
    assert_eq!(
        cross_tenant_row
            .try_get::<i64, _>("schema_version")
            .unwrap(),
        1
    );
    assert_eq!(
        cross_tenant_row
            .try_get::<String, _>("lifecycle_state")
            .unwrap(),
        "draft"
    );
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn review_pending_revision_is_immutable_until_reviewer_returns_it_to_draft(
    owner_pool: PgPool,
) {
    let org = OrgId::knl();
    let actor = seed_org_and_user(&owner_pool, *org.as_uuid(), "review-integrity").await;
    let rt_pool = runtime_role_pool(&owner_pool).await;
    let at = datetime!(2026-07-19 11:30 UTC);

    let object_type = console_platform_request_context::scope_org(org, async {
        let store = PgOntologyStore::new(rt_pool.clone())
            .with_command_pool(command_role_pool(&owner_pool).await);
        let draft = store
            .create_object_type(
                actor,
                work_order_draft("wo.review_integrity"),
                TraceContext::generate(),
                at,
            )
            .await
            .unwrap();
        store
            .transition_lifecycle(
                actor,
                draft.id,
                draft.write_precondition(),
                SchemaLifecycleState::ReviewPending,
                true,
                TraceContext::generate(),
                at,
            )
            .await
            .unwrap()
    })
    .await;
    let object_type_id = object_type.id;

    let storage_sql = r#"
        SELECT jsonb_build_object(
            'object_type', to_jsonb(o),
            'properties', COALESCE((
                SELECT jsonb_agg(to_jsonb(p) ORDER BY p.key)
                FROM ont_property_defs p
                WHERE p.org_id = o.org_id AND p.object_type_id = o.id
            ), '[]'::jsonb),
            'links', COALESCE((
                SELECT jsonb_agg(to_jsonb(l) ORDER BY l.stable_key)
                FROM ont_link_types l
                WHERE l.org_id = o.org_id AND l.object_type_id = o.id
            ), '[]'::jsonb),
            'actions', COALESCE((
                SELECT jsonb_agg(to_jsonb(a) ORDER BY a.stable_key)
                FROM ont_action_types a
                WHERE a.org_id = o.org_id AND a.object_type_id = o.id
            ), '[]'::jsonb),
            'analytics', COALESCE((
                SELECT jsonb_agg(to_jsonb(n) ORDER BY n.key)
                FROM ont_analytics n
                WHERE n.org_id = o.org_id AND n.object_type_id = o.id
            ), '[]'::jsonb)
        )
        FROM ont_object_types o
        WHERE o.org_id = $1 AND o.id = $2
    "#;
    let audits_sql = r#"
        SELECT COALESCE(jsonb_agg(to_jsonb(a) ORDER BY a.id), '[]'::jsonb)
        FROM audit_events a
        WHERE a.org_id = $1 AND a.target_id = $2
    "#;
    let before_storage: serde_json::Value = sqlx::query_scalar(storage_sql)
        .bind(*org.as_uuid())
        .bind(*object_type_id.as_uuid())
        .fetch_one(&owner_pool)
        .await
        .unwrap();
    let before_audits: serde_json::Value = sqlx::query_scalar(audits_sql)
        .bind(*org.as_uuid())
        .bind(object_type_id.to_string())
        .fetch_one(&owner_pool)
        .await
        .unwrap();

    let mut hostile = revision_draft(
        "wo.review_integrity",
        "mutated after submission",
        "post_submission",
    );
    hostile.analytics.push(AnalyticInput {
        key: "post_submission_score".to_owned(),
        title: "Post-submission score".to_owned(),
        formula: serde_json::json!({"expression": "score()"}),
        result_type: serde_json::json!({"type": "number"}),
    });
    let result = console_platform_request_context::scope_org(org, async {
        PgOntologyStore::new(rt_pool)
            .with_command_pool(command_role_pool(&owner_pool).await)
            .stage_revision(
                actor,
                "wo.review_integrity",
                object_type.write_precondition(),
                hostile,
                TraceContext::generate(),
                at,
            )
            .await
    })
    .await;

    let after_storage: serde_json::Value = sqlx::query_scalar(storage_sql)
        .bind(*org.as_uuid())
        .bind(*object_type_id.as_uuid())
        .fetch_one(&owner_pool)
        .await
        .unwrap();
    let after_audits: serde_json::Value = sqlx::query_scalar(audits_sql)
        .bind(*org.as_uuid())
        .bind(object_type_id.to_string())
        .fetch_one(&owner_pool)
        .await
        .unwrap();

    assert!(
        matches!(
            result,
            Err(PgOntologyError::Domain(ref kernel))
                if kernel.kind == ErrorKind::Conflict
        ),
        "submitted review content must require reviewer return-to-draft, got {result:?}",
    );
    assert_eq!(
        after_storage, before_storage,
        "rejected edit must preserve every head and child storage byte"
    );
    assert_eq!(
        after_audits, before_audits,
        "rejected edit must not emit a mutation audit"
    );
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn concurrent_create_create_serializes_to_one_success_and_one_typed_conflict(
    owner_pool: PgPool,
) {
    const CREATE_GATE: i64 = 4_730_014;
    let org = OrgId::knl();
    let actor = seed_org_and_user(&owner_pool, *org.as_uuid(), "create-create").await;
    let rt_pool = runtime_role_pool(&owner_pool).await;
    let at = datetime!(2026-07-19 11:31 UTC);

    sqlx::raw_sql(
        r#"
        CREATE FUNCTION test_block_first_object_type_create() RETURNS trigger
        LANGUAGE plpgsql AS $$
        BEGIN
            IF NEW.title = 'first hostile create' THEN
                PERFORM pg_advisory_xact_lock(4730014);
            END IF;
            RETURN NEW;
        END
        $$;
        CREATE TRIGGER test_block_first_object_type_create
        BEFORE INSERT ON ont_object_types
        FOR EACH ROW EXECUTE FUNCTION test_block_first_object_type_create();
        "#,
    )
    .execute(&owner_pool)
    .await
    .unwrap();

    let gate = hold_advisory_gate(&owner_pool, CREATE_GATE).await;
    let first_pool = rt_pool.clone();
    let first_cmd_pool = command_role_pool(&owner_pool).await;
    let first = tokio::spawn(console_platform_request_context::scope_org(
        org,
        async move {
            let mut draft = work_order_draft("wo.concurrent_create");
            draft.title = "first hostile create".to_owned();
            PgOntologyStore::new(first_pool)
                .with_command_pool(first_cmd_pool)
                .create_object_type(actor, draft, TraceContext::generate(), at)
                .await
        },
    ));
    wait_for_advisory_waiters(&owner_pool, 1).await;

    let second_pool = rt_pool.clone();
    let second_cmd_pool = command_role_pool(&owner_pool).await;
    let second = tokio::spawn(console_platform_request_context::scope_org(
        org,
        async move {
            let mut draft = work_order_draft("wo.concurrent_create");
            draft.title = "second hostile create".to_owned();
            PgOntologyStore::new(second_pool)
                .with_command_pool(second_cmd_pool)
                .create_object_type(actor, draft, TraceContext::generate(), at)
                .await
        },
    ));
    tokio::task::yield_now().await;
    release_advisory_gate(gate, CREATE_GATE).await;

    let first = first.await.unwrap();
    let second = second.await.unwrap();
    assert!(first.is_ok(), "first create must win, got {first:?}");
    assert!(
        matches!(
            second,
            Err(PgOntologyError::Domain(ref kernel))
                if kernel.kind == ErrorKind::Conflict
        ),
        "racing duplicate create must be a typed conflict, got {second:?}",
    );

    let rows: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM ont_object_types WHERE org_id = $1 AND stable_key = 'wo.concurrent_create'",
    )
    .bind(*org.as_uuid())
    .fetch_one(&owner_pool)
    .await
    .unwrap();
    assert_eq!(rows, 1, "exactly one version-one row may be created");
    let audits: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM audit_events WHERE org_id = $1 AND action = 'ontology.object_type.create'",
    )
    .bind(*org.as_uuid())
    .fetch_one(&owner_pool)
    .await
    .unwrap();
    assert_eq!(audits, 1, "only the committed create may be audited");
}

#[test]
fn registry_json_omissions_default_while_explicit_nulls_remain_distinct() {
    let omitted_property: PropertyDefInput = serde_json::from_value(serde_json::json!({
        "key": "status",
        "title": "Status",
        "field_type": "text"
    }))
    .unwrap();
    let explicit_null_property: PropertyDefInput = serde_json::from_value(serde_json::json!({
        "key": "status",
        "title": "Status",
        "field_type": "text",
        "config": null
    }))
    .unwrap();

    let omitted_action: ActionTypeInput = serde_json::from_value(serde_json::json!({
        "stable_key": "approve",
        "title": "Approve",
        "dispatch": "instance_revision"
    }))
    .unwrap();
    let explicit_null_action: ActionTypeInput = serde_json::from_value(serde_json::json!({
        "stable_key": "approve",
        "title": "Approve",
        "params_schema": null,
        "edits": null,
        "submission_criteria": null,
        "side_effects": null,
        "dispatch": "instance_revision",
        "control_points": null
    }))
    .unwrap();

    let omitted_analytic: AnalyticInput = serde_json::from_value(serde_json::json!({
        "key": "score",
        "title": "Score"
    }))
    .unwrap();
    let explicit_null_analytic: AnalyticInput = serde_json::from_value(serde_json::json!({
        "key": "score",
        "title": "Score",
        "formula": null,
        "result_type": null
    }))
    .unwrap();

    let checks = [
        (
            "property.config omission",
            omitted_property.config == serde_json::json!({}),
        ),
        (
            "property.config explicit null",
            explicit_null_property.config.is_null(),
        ),
        (
            "action.params_schema omission",
            omitted_action.params_schema == serde_json::json!({}),
        ),
        (
            "action.edits omission",
            omitted_action.edits == serde_json::json!([]),
        ),
        (
            "action.submission_criteria omission",
            omitted_action.submission_criteria == serde_json::json!([]),
        ),
        (
            "action.side_effects omission",
            omitted_action.side_effects == serde_json::json!([]),
        ),
        (
            "action.control_points omission",
            omitted_action.control_points == serde_json::json!([]),
        ),
        (
            "action.params_schema explicit null",
            explicit_null_action.params_schema.is_null(),
        ),
        (
            "action.edits explicit null",
            explicit_null_action.edits.is_null(),
        ),
        (
            "action.submission_criteria explicit null",
            explicit_null_action.submission_criteria.is_null(),
        ),
        (
            "action.side_effects explicit null",
            explicit_null_action.side_effects.is_null(),
        ),
        (
            "action.control_points explicit null",
            explicit_null_action.control_points.is_null(),
        ),
        (
            "analytic.formula omission",
            omitted_analytic.formula == serde_json::json!({}),
        ),
        (
            "analytic.result_type omission",
            omitted_analytic.result_type == serde_json::json!({}),
        ),
        (
            "analytic.formula explicit null",
            explicit_null_analytic.formula.is_null(),
        ),
        (
            "analytic.result_type explicit null",
            explicit_null_analytic.result_type.is_null(),
        ),
    ];
    let failures = checks
        .into_iter()
        .filter_map(|(label, passed)| (!passed).then_some(label))
        .collect::<Vec<_>>();

    assert!(
        failures.is_empty(),
        "JSON omission/null boundary violations: {failures:?}"
    );
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn registry_json_null_and_wrong_shapes_are_typed_validation_without_writes(
    owner_pool: PgPool,
) {
    let org = OrgId::knl();
    let actor = seed_org_and_user(&owner_pool, *org.as_uuid(), "json-shapes").await;
    let rt_pool = runtime_role_pool(&owner_pool).await;
    let at = datetime!(2026-07-19 12:15 UTC);

    let action_draft = |stable_key: &str| {
        let mut draft = work_order_draft(stable_key);
        draft.actions.push(ActionTypeInput {
            stable_key: "approve".to_owned(),
            title: "Approve".to_owned(),
            params_schema: serde_json::json!({}),
            edits: serde_json::json!([]),
            submission_criteria: serde_json::json!([]),
            side_effects: serde_json::json!([]),
            dispatch: ActionDispatch::InstanceRevision,
            dispatch_target: None,
            control_points: serde_json::json!([]),
        });
        draft
    };
    let analytic_draft = |stable_key: &str| {
        let mut draft = work_order_draft(stable_key);
        draft.analytics.push(AnalyticInput {
            key: "score".to_owned(),
            title: "Score".to_owned(),
            formula: serde_json::json!({"expression": "score()"}),
            result_type: serde_json::json!({"type": "number"}),
        });
        draft
    };

    let mut cases = Vec::new();

    let mut draft = work_order_draft("wo.json_property_null");
    draft.properties[0].config = serde_json::Value::Null;
    cases.push(("property config null", "config", draft));

    let mut draft = work_order_draft("wo.json_property_array");
    draft.properties[0].config = serde_json::json!([]);
    cases.push(("property config array", "config", draft));

    let mut draft = action_draft("wo.json_action_params_null");
    draft.actions[0].params_schema = serde_json::Value::Null;
    cases.push(("action params_schema null", "params_schema", draft));

    let mut draft = action_draft("wo.json_action_params_array");
    draft.actions[0].params_schema = serde_json::json!([]);
    cases.push(("action params_schema array", "params_schema", draft));

    let mut draft = action_draft("wo.json_action_edits_null");
    draft.actions[0].edits = serde_json::Value::Null;
    cases.push(("action edits null", "edits", draft));

    let mut draft = action_draft("wo.json_action_edits_object");
    draft.actions[0].edits = serde_json::json!({});
    cases.push(("action edits object", "edits", draft));

    let mut draft = action_draft("wo.json_action_criteria_null");
    draft.actions[0].submission_criteria = serde_json::Value::Null;
    cases.push((
        "action submission_criteria null",
        "submission_criteria",
        draft,
    ));

    let mut draft = action_draft("wo.json_action_criteria_object");
    draft.actions[0].submission_criteria = serde_json::json!({});
    cases.push((
        "action submission_criteria object",
        "submission_criteria",
        draft,
    ));

    let mut draft = action_draft("wo.json_action_effects_null");
    draft.actions[0].side_effects = serde_json::Value::Null;
    cases.push(("action side_effects null", "side_effects", draft));

    let mut draft = action_draft("wo.json_action_effects_object");
    draft.actions[0].side_effects = serde_json::json!({});
    cases.push(("action side_effects object", "side_effects", draft));

    let mut draft = action_draft("wo.json_action_controls_null");
    draft.actions[0].control_points = serde_json::Value::Null;
    cases.push(("action control_points null", "control_points", draft));

    let mut draft = action_draft("wo.json_action_controls_object");
    draft.actions[0].control_points = serde_json::json!({});
    cases.push(("action control_points object", "control_points", draft));

    let mut draft = analytic_draft("wo.json_analytic_result_null");
    draft.analytics[0].result_type = serde_json::Value::Null;
    cases.push(("analytic result_type null", "result_type", draft));

    let mut draft = analytic_draft("wo.json_analytic_result_array");
    draft.analytics[0].result_type = serde_json::json!([]);
    cases.push(("analytic result_type array", "result_type", draft));

    let mut draft = analytic_draft("wo.json_analytic_formula_null");
    draft.analytics[0].formula = serde_json::Value::Null;
    cases.push(("analytic formula null", "formula", draft));

    let mut draft = analytic_draft("wo.json_analytic_formula_string");
    draft.analytics[0].formula = serde_json::json!("score()");
    cases.push(("analytic formula string", "formula", draft));

    let (failures, stored_rows, audits) =
        console_platform_request_context::scope_org(org, async {
            let store = PgOntologyStore::new(rt_pool)
                .with_command_pool(command_role_pool(&owner_pool).await);
            let mut failures = Vec::new();

            for (label, expected_field, draft) in cases {
                let result = store
                    .create_object_type(actor, draft, TraceContext::generate(), at)
                    .await;
                match result {
                    Err(PgOntologyError::Domain(kernel))
                        if kernel.kind == ErrorKind::Validation
                            && kernel.message.contains(expected_field) => {}
                    other => failures.push(format!("{label}: {other:?}")),
                }
            }

            let stored_rows: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM ont_object_types WHERE org_id = $1 AND stable_key LIKE 'wo.json_%'",
            )
            .bind(*org.as_uuid())
            .fetch_one(&owner_pool)
            .await
            .unwrap();
            let audits: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM audit_events WHERE org_id = $1 AND action = 'ontology.object_type.create'",
            )
            .bind(*org.as_uuid())
            .fetch_one(&owner_pool)
            .await
            .unwrap();

            (failures, stored_rows, audits)
        })
        .await;

    assert!(
        failures.is_empty() && stored_rows == 0 && audits == 0,
        "invalid registry JSON crossed validation: failures={failures:?}, stored_rows={stored_rows}, audits={audits}"
    );
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn omitted_child_defaults_cannot_be_replayed_as_null_or_wrong_shape(owner_pool: PgPool) {
    let org = OrgId::knl();
    let actor = seed_org_and_user(&owner_pool, *org.as_uuid(), "json-replay").await;
    let rt_pool = runtime_role_pool(&owner_pool).await;
    let at = datetime!(2026-07-19 12:30 UTC);

    let property: PropertyDefInput = serde_json::from_value(serde_json::json!({
        "key": "title",
        "title": "Title",
        "field_type": "text"
    }))
    .unwrap();
    let action: ActionTypeInput = serde_json::from_value(serde_json::json!({
        "stable_key": "approve",
        "title": "Approve",
        "dispatch": "instance_revision"
    }))
    .unwrap();
    let analytic: AnalyticInput = serde_json::from_value(serde_json::json!({
        "key": "score",
        "title": "Score",
        "formula": {"expression": "score()"}
    }))
    .unwrap();

    let mut canonical = work_order_draft("wo.json_replay");
    canonical.properties = vec![property];
    canonical.actions = vec![action];
    canonical.analytics = vec![analytic];

    let object_type = console_platform_request_context::scope_org(org, async {
        PgOntologyStore::new(rt_pool.clone())
            .with_command_pool(command_role_pool(&owner_pool).await)
            .create_object_type(actor, canonical.clone(), TraceContext::generate(), at)
            .await
            .unwrap()
    })
    .await;
    let object_type_id = object_type.id;

    let storage_sql = r#"
        SELECT jsonb_build_object(
            'object_type', to_jsonb(o),
            'properties', COALESCE((
                SELECT jsonb_agg(to_jsonb(p) ORDER BY p.key)
                FROM ont_property_defs p
                WHERE p.org_id = o.org_id AND p.object_type_id = o.id
            ), '[]'::jsonb),
            'links', COALESCE((
                SELECT jsonb_agg(to_jsonb(l) ORDER BY l.stable_key)
                FROM ont_link_types l
                WHERE l.org_id = o.org_id AND l.object_type_id = o.id
            ), '[]'::jsonb),
            'actions', COALESCE((
                SELECT jsonb_agg(to_jsonb(a) ORDER BY a.stable_key)
                FROM ont_action_types a
                WHERE a.org_id = o.org_id AND a.object_type_id = o.id
            ), '[]'::jsonb),
            'analytics', COALESCE((
                SELECT jsonb_agg(to_jsonb(n) ORDER BY n.key)
                FROM ont_analytics n
                WHERE n.org_id = o.org_id AND n.object_type_id = o.id
            ), '[]'::jsonb)
        )
        FROM ont_object_types o
        WHERE o.org_id = $1 AND o.id = $2
    "#;
    let audits_sql = r#"
        SELECT COALESCE(jsonb_agg(to_jsonb(a) ORDER BY a.id), '[]'::jsonb)
        FROM audit_events a
        WHERE a.org_id = $1 AND a.target_id = $2
    "#;
    let before_storage: serde_json::Value = sqlx::query_scalar(storage_sql)
        .bind(*org.as_uuid())
        .bind(*object_type_id.as_uuid())
        .fetch_one(&owner_pool)
        .await
        .unwrap();
    let before_audits: serde_json::Value = sqlx::query_scalar(audits_sql)
        .bind(*org.as_uuid())
        .bind(object_type_id.to_string())
        .fetch_one(&owner_pool)
        .await
        .unwrap();

    let mut explicit_null = canonical.clone();
    explicit_null.properties[0].config = serde_json::Value::Null;
    let null_result = console_platform_request_context::scope_org(org, async {
        PgOntologyStore::new(rt_pool.clone())
            .with_command_pool(command_role_pool(&owner_pool).await)
            .stage_revision(
                actor,
                "wo.json_replay",
                object_type.write_precondition(),
                explicit_null,
                TraceContext::generate(),
                at,
            )
            .await
    })
    .await;

    let mut wrong_shape = canonical;
    wrong_shape.actions[0].edits = serde_json::json!({});
    let wrong_shape_result = console_platform_request_context::scope_org(org, async {
        PgOntologyStore::new(rt_pool)
            .with_command_pool(command_role_pool(&owner_pool).await)
            .stage_revision(
                actor,
                "wo.json_replay",
                object_type.write_precondition(),
                wrong_shape,
                TraceContext::generate(),
                at,
            )
            .await
    })
    .await;

    let after_storage: serde_json::Value = sqlx::query_scalar(storage_sql)
        .bind(*org.as_uuid())
        .bind(*object_type_id.as_uuid())
        .fetch_one(&owner_pool)
        .await
        .unwrap();
    let after_audits: serde_json::Value = sqlx::query_scalar(audits_sql)
        .bind(*org.as_uuid())
        .bind(object_type_id.to_string())
        .fetch_one(&owner_pool)
        .await
        .unwrap();

    let null_is_validation = matches!(
        &null_result,
        Err(PgOntologyError::Domain(kernel))
            if kernel.kind == ErrorKind::Validation && kernel.message.contains("config")
    );
    let wrong_shape_is_validation = matches!(
        &wrong_shape_result,
        Err(PgOntologyError::Domain(kernel))
            if kernel.kind == ErrorKind::Validation && kernel.message.contains("edits")
    );
    let storage_preserved = after_storage == before_storage;
    let audits_preserved = after_audits == before_audits;

    assert!(
        null_is_validation && wrong_shape_is_validation && storage_preserved && audits_preserved,
        "child replay crossed the JSON validation boundary: null_result={null_result:?}, wrong_shape_result={wrong_shape_result:?}, storage_preserved={storage_preserved}, audits_preserved={audits_preserved}"
    );
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn analytic_formula_omission_defaults_but_explicit_null_rejects_without_writes(
    owner_pool: PgPool,
) {
    let omitted: AnalyticInput = serde_json::from_value(serde_json::json!({
        "key": "omitted_formula",
        "title": "Omitted formula",
        "result_type": {"type": "number"}
    }))
    .unwrap();
    assert_eq!(
        omitted.formula,
        serde_json::json!({}),
        "wire omission must receive the documented object default"
    );
    let explicit_null: AnalyticInput = serde_json::from_value(serde_json::json!({
        "key": "null_formula",
        "title": "Null formula",
        "formula": null,
        "result_type": {"type": "number"}
    }))
    .unwrap();
    assert!(
        explicit_null.formula.is_null(),
        "explicit null must remain distinguishable for validation"
    );

    let org = OrgId::knl();
    let actor = seed_org_and_user(&owner_pool, *org.as_uuid(), "null-formula").await;
    let rt_pool = runtime_role_pool(&owner_pool).await;
    let at = datetime!(2026-07-19 11:45 UTC);
    let mut draft = work_order_draft("wo.null_formula");
    draft.analytics.push(explicit_null);
    let result = console_platform_request_context::scope_org(org, async {
        PgOntologyStore::new(rt_pool)
            .with_command_pool(command_role_pool(&owner_pool).await)
            .create_object_type(actor, draft, TraceContext::generate(), at)
            .await
    })
    .await;
    assert!(
        matches!(
            result,
            Err(PgOntologyError::Domain(ref kernel))
                if kernel.kind == ErrorKind::Validation
        ),
        "explicit null formula must be typed validation, got {result:?}",
    );

    let rows: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM ont_object_types WHERE org_id = $1 AND stable_key = 'wo.null_formula'",
    )
    .bind(*org.as_uuid())
    .fetch_one(&owner_pool)
    .await
    .unwrap();
    let audits: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM audit_events WHERE org_id = $1 AND action = 'ontology.object_type.create'",
    )
    .bind(*org.as_uuid())
    .fetch_one(&owner_pool)
    .await
    .unwrap();
    assert_eq!(rows, 0, "invalid formula must not reach storage");
    assert_eq!(audits, 0, "invalid formula must not emit an audit");
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn noncanonical_stable_key_is_typed_validation_with_no_audit_storage_divergence(
    owner_pool: PgPool,
) {
    let org = OrgId::knl();
    let actor = seed_org_and_user(&owner_pool, *org.as_uuid(), "stable-key-shape").await;
    let rt_pool = runtime_role_pool(&owner_pool).await;
    let at = datetime!(2026-07-19 11:46 UTC);
    let mut draft = work_order_draft("wo.canonical_key");
    draft.stable_key = " wo.canonical_key ".to_owned();

    let result = console_platform_request_context::scope_org(org, async {
        PgOntologyStore::new(rt_pool)
            .with_command_pool(command_role_pool(&owner_pool).await)
            .create_object_type(actor, draft, TraceContext::generate(), at)
            .await
    })
    .await;
    assert!(
        matches!(
            result,
            Err(PgOntologyError::Domain(ref kernel))
                if kernel.kind == ErrorKind::Validation
        ),
        "surrounding whitespace must be rejected before audit/lock/storage, got {result:?}",
    );

    let rows: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM ont_object_types WHERE org_id = $1 AND stable_key = 'wo.canonical_key'",
    )
    .bind(*org.as_uuid())
    .fetch_one(&owner_pool)
    .await
    .unwrap();
    let audits: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM audit_events WHERE org_id = $1 AND action = 'ontology.object_type.create'",
    )
    .bind(*org.as_uuid())
    .fetch_one(&owner_pool)
    .await
    .unwrap();
    assert_eq!(rows, 0, "invalid key must not create its trimmed alias");
    assert_eq!(audits, 0, "invalid key must not create an untruthful audit");
}
