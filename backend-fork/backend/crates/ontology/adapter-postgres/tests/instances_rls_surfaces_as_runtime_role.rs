#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! RUNTIME proofs for the §1b owned instance store, exercised as the genuine
//! non-owner `console_rt` role (NOSUPERUSER, NOBYPASSRLS, FORCE RLS) — the only
//! faithful exercise of the org_isolation policy. The default `#[sqlx::test]`
//! pool is a BYPASSRLS superuser that would green-light a broken policy.
//!
//! Proves:
//!   (a) create + read current state;
//!   (b) stage a v+1 revision, then as-of(t) returns the HISTORICAL revision
//!       (not the current head) — bi-temporal effective-dating;
//!   (c) cross-tenant instance is INVISIBLE under another org's GUC (RLS);
//!   (d) FAIL-CLOSED: with no org armed every read/write errors, never leaks;
//!   (e) fixity: the stored per-(org,instance) hash chain verifies, and a tamper
//!       of any fixity-covered field is detected by recompute;
//!   (f) traversal: search-around returns linked nodes/edges.
//!
//! NOTE (migrations path): runs against the canonical
//! `../../platform/db/migrations` (the ship path). The earlier concurrent-lane
//! migration-number collision has been reconciled, so no deduplicated copy is
//! needed.

use console_ontology_adapter_postgres::instances::{
    CreateInstance, PgInstanceStore, StageRevision, verify_chain,
};
use console_ontology_adapter_postgres::{CreateObjectTypeDraft, PgOntologyStore, PropertyDefInput};
use console_ontology_domain::{BackingKind, InstanceLifecycleState, LinkCardinality, ObjectTypeId};

use console_kernel_core::{OrgId, TraceContext, UserId};
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use time::macros::datetime;
use uuid::Uuid;

const ORG_B: Uuid = Uuid::from_u128(0x3333_3333_3333_3333_3333_3333_3333_3333);

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

async fn seed_org_and_user(owner_pool: &PgPool, org: Uuid, tag: &str) -> UserId {
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

/// Publish an `instance`-backed object type with one required `priority` choice
/// property + a `linked_to` link type, and return its (head) object_type_id.
async fn seed_object_type(
    owner_pool: &PgPool,
    org: OrgId,
    actor: UserId,
    key: &str,
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
                config: serde_json::json!({"choices": [{"id": "hi", "name": "높음"}]}),
                backing_column: None,
                required: true,
                in_property_policy: false,
            }],
            links: vec![console_ontology_adapter_postgres::LinkTypeInput {
                stable_key: "linked_to".to_owned(),
                title: "연결".to_owned(),
                reverse_title: None,
                to_object_type_id: None,
                cardinality: LinkCardinality::ManyMany,
                traversable: true,
            }],
            actions: Vec::new(),
            analytics: Vec::new(),
        };
        let created = store
            .create_object_type(
                actor,
                draft,
                TraceContext::generate(),
                datetime!(2026-07-09 12:00 UTC),
            )
            .await
            .expect("create object type");
        created.id
    })
    .await
}

fn create_input(object_type_id: ObjectTypeId, priority: &str) -> CreateInstance {
    CreateInstance {
        object_type_id,
        title: "WO-1".to_owned(),
        attributes: serde_json::json!({ "priority": priority }),
        valid_from: None,
        action_type_id: None,
        reason: Some("initial".to_owned()),
    }
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn create_read_current_and_as_of_history(owner_pool: PgPool) {
    let rt = runtime_role_pool(&owner_pool).await;
    let org = OrgId::knl();
    let actor = seed_org_and_user(&owner_pool, *org.as_uuid(), "a").await;
    let type_id = seed_object_type(&owner_pool, org, actor, "wo.inst").await;

    let t1 = datetime!(2026-07-09 12:00 UTC);
    let t2 = datetime!(2026-07-10 12:00 UTC);

    console_platform_request_context::scope_org(org, async {
        let store = PgInstanceStore::new(rt.clone());

        // (a) create v1 (priority hi) effective at t1.
        let mut v1 = create_input(type_id, "hi");
        v1.valid_from = Some(t1);
        let created = store
            .create_instance(actor, v1, TraceContext::generate(), t1)
            .await
            .expect("create instance");
        assert_eq!(created.revision.version, 1);
        assert_eq!(
            created.instance.lifecycle_state,
            InstanceLifecycleState::Draft
        );
        assert_eq!(created.revision.attributes["priority"], "hi");

        let instance_id = created.instance.id;

        // Reject an unknown attribute (schema validation is a real trust boundary).
        let mut bad = create_input(type_id, "hi");
        bad.attributes = serde_json::json!({ "priority": "hi", "nope": 1 });
        assert!(
            store
                .create_instance(actor, bad, TraceContext::generate(), t1)
                .await
                .is_err(),
            "unknown attribute must be rejected"
        );

        // (b) stage v2 (priority lo) effective at t2.
        store
            .stage_revision(
                actor,
                instance_id,
                StageRevision {
                    attributes: serde_json::json!({ "priority": "hi" }),
                    valid_from: Some(t2),
                    action_type_id: None,
                    reason: Some("bump".to_owned()),
                },
                TraceContext::generate(),
                t2,
            )
            .await
            .expect("stage revision");

        // current head = v2 (valid_to IS NULL).
        let cur = store.get_current(instance_id).await.unwrap();
        assert_eq!(cur.revision.version, 2);

        // as-of just after t1 (before t2) returns the HISTORICAL v1, not current.
        let as_of = store
            .get_as_of(instance_id, t1 + time::Duration::hours(1))
            .await
            .unwrap();
        assert_eq!(
            as_of.revision.version, 1,
            "as-of(t) must return the effective historical revision"
        );

        // (e) fixity: the stored chain verifies, and a tamper is detected.
        let history = store.history(instance_id).await.unwrap();
        assert_eq!(history.len(), 2);
        assert_eq!(
            history[1].prev_hash, history[0].row_hash,
            "chain links v2.prev = v1.row_hash"
        );
        assert!(
            verify_chain(&history).is_none(),
            "untampered chain must verify"
        );

        let mut tampered = history.clone();
        tampered[0].attributes = serde_json::json!({ "priority": "TAMPERED" });
        assert_eq!(
            verify_chain(&tampered),
            Some(tampered[0].id),
            "tampering a revision's attributes must be detected"
        );
    })
    .await;
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn cross_tenant_instance_is_invisible_and_fails_closed(owner_pool: PgPool) {
    let rt = runtime_role_pool(&owner_pool).await;
    let org_a = OrgId::knl();
    let org_b = OrgId::from_uuid(ORG_B);
    let actor_a = seed_org_and_user(&owner_pool, *org_a.as_uuid(), "a").await;
    let actor_b = seed_org_and_user(&owner_pool, *org_b.as_uuid(), "b").await;
    let type_a = seed_object_type(&owner_pool, org_a, actor_a, "wo.a").await;
    let type_b = seed_object_type(&owner_pool, org_b, actor_b, "wo.b").await;
    let at = datetime!(2026-07-09 12:00 UTC);

    let a_instance = console_platform_request_context::scope_org(org_a, async {
        let store = PgInstanceStore::new(rt.clone());
        store
            .create_instance(
                actor_a,
                create_input(type_a, "hi"),
                TraceContext::generate(),
                at,
            )
            .await
            .unwrap()
            .instance
            .id
    })
    .await;

    let b_instance = console_platform_request_context::scope_org(org_b, async {
        let store = PgInstanceStore::new(rt.clone());
        store
            .create_instance(
                actor_b,
                create_input(type_b, "hi"),
                TraceContext::generate(),
                at,
            )
            .await
            .unwrap()
            .instance
            .id
    })
    .await;

    // (c) Under org-A's GUC, B's instance is invisible; A's own is visible.
    console_platform_request_context::scope_org(org_a, async {
        let store = PgInstanceStore::new(rt.clone());
        assert!(
            store.get_current(a_instance).await.is_ok(),
            "A sees its own instance"
        );
        assert!(
            store.get_current(b_instance).await.is_err(),
            "B's instance must be invisible under org-A's GUC as console_rt"
        );
        let list = store.list_instances(type_a).await.unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].instance.id, a_instance);
    })
    .await;

    // (d) FAIL-CLOSED: no org armed → reads and writes error, never leak.
    let store = PgInstanceStore::new(rt.clone());
    assert!(
        store.get_current(a_instance).await.is_err(),
        "read must fail closed without org"
    );
    assert!(
        store
            .create_instance(
                actor_a,
                create_input(type_a, "hi"),
                TraceContext::generate(),
                at
            )
            .await
            .is_err(),
        "write must fail closed without org"
    );
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn traversal_returns_linked_nodes(owner_pool: PgPool) {
    let rt = runtime_role_pool(&owner_pool).await;
    let org = OrgId::knl();
    let actor = seed_org_and_user(&owner_pool, *org.as_uuid(), "a").await;
    let type_id = seed_object_type(&owner_pool, org, actor, "wo.g").await;
    let at = datetime!(2026-07-09 12:00 UTC);

    // Resolve the link_type_id created with the object type.
    let link_type_uuid: Uuid = sqlx::query_scalar(
        "SELECT id FROM ont_link_types WHERE object_type_id = $1 AND stable_key = 'linked_to'",
    )
    .bind(*type_id.as_uuid())
    .fetch_one(&owner_pool)
    .await
    .unwrap();
    let link_type = console_ontology_domain::LinkTypeId::from_uuid(link_type_uuid);

    console_platform_request_context::scope_org(org, async {
        let store = PgInstanceStore::new(rt.clone());
        let a = store
            .create_instance(
                actor,
                create_input(type_id, "hi"),
                TraceContext::generate(),
                at,
            )
            .await
            .unwrap()
            .instance
            .id;
        let b = store
            .create_instance(
                actor,
                create_input(type_id, "hi"),
                TraceContext::generate(),
                at,
            )
            .await
            .unwrap()
            .instance
            .id;
        let c = store
            .create_instance(
                actor,
                create_input(type_id, "hi"),
                TraceContext::generate(),
                at,
            )
            .await
            .unwrap()
            .instance
            .id;

        // a -> b -> c
        store
            .create_link(actor, link_type, a, b, None, TraceContext::generate(), at)
            .await
            .unwrap();
        store
            .create_link(actor, link_type, b, c, None, TraceContext::generate(), at)
            .await
            .unwrap();

        // depth 1 from a reaches b only.
        let g1 = store.traverse(a, Some(link_type), 1).await.unwrap();
        let ids1: Vec<_> = g1.nodes.iter().map(|n| n.instance_id).collect();
        assert!(ids1.contains(&a) && ids1.contains(&b));
        assert!(!ids1.contains(&c), "depth 1 must not reach c");

        // depth 2 reaches c too.
        let g2 = store.traverse(a, Some(link_type), 2).await.unwrap();
        let ids2: Vec<_> = g2.nodes.iter().map(|n| n.instance_id).collect();
        assert!(ids2.contains(&c), "depth 2 must reach c");
        assert_eq!(g2.edges.len(), 2, "both live edges are returned");
    })
    .await;
}
