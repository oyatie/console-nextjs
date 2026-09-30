#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! RUNTIME proofs for the C-chain (거래처 계약 → 직무 → 채용 공고) engine types,
//! exercised as the genuine non-owner `console_rt` role (NOSUPERUSER, NOBYPASSRLS,
//! FORCE RLS) — the only faithful exercise of `org_isolation`. The default
//! `#[sqlx::test]` pool is a BYPASSRLS superuser that would green-light a broken
//! policy, so every store call rides a `SET ROLE console_rt` pool.
//!
//! Proves:
//!   (a) `seed_c_chain_object_types` publishes contract/position/posting through
//!       the engine (not raw SQL), each an `instance` type with a `create` action,
//!       and each is a DISTINCT row per org — org B's copies are not org A's
//!       (RLS isolation), and a read with NO org armed fails closed;
//!   (b) the chain links: a contract instance → position instance → posting
//!       instance, connected by the seeded forward link types, and
//!       `traverse(contract, depth)` returns the downstream position + posting
//!       nodes at the right depths — the §2 search-around walk over the chain;
//!   (c) traversal is org-scoped: org B, walking org A's contract id, sees no
//!       edges and no hydrated nodes (cross-tenant graph isolation as console_rt).

use console_ontology_adapter_postgres::instances::{CreateInstance, PgInstanceStore};
use console_ontology_adapter_postgres::seed::{
    CONTRACT_KEY, POSITION_KEY, POSTING_KEY, seed_c_chain_object_types,
};
use console_ontology_adapter_postgres::{ObjectTypeDetail, PgOntologyStore};
use console_ontology_domain::{InstanceId, LinkTypeId, ObjectTypeId, SchemaLifecycleState};

use console_kernel_core::{OrgId, TraceContext, UserId};
use console_platform_request_context::scope_org;
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use time::macros::datetime;
use uuid::Uuid;

const ORG_A: Uuid = Uuid::from_u128(0x0C0C_0C0C_0C0C_0C0C_0C0C_0C0C_0C0C_0C0C);
const ORG_B: Uuid = Uuid::from_u128(0x0B0B_0B0B_0B0B_0B0B_0B0B_0B0B_0B0B_0B0B);
const AT: time::OffsetDateTime = datetime!(2026-07-10 09:00 UTC);

/// Every connection becomes the genuine non-owner `console_rt`. The registry/instance
/// migrations grant it the needed privileges, so no manual GRANT here.
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
    sqlx::query("INSERT INTO organizations (id, slug, name) VALUES ($1, $2, $3) ON CONFLICT (id) DO NOTHING")
        .bind(org)
        .bind(format!("org-{}", &org.simple().to_string()[..12]))
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

/// The published link-type id for `link_key` on `detail`.
fn link_id(detail: &ObjectTypeDetail, link_key: &str) -> LinkTypeId {
    detail
        .links
        .iter()
        .find(|l| l.stable_key == link_key)
        .unwrap_or_else(|| panic!("link {link_key} must be authored on the type"))
        .id
}

// ---------------------------------------------------------------------------
// (a) The three types publish through the engine, isolated per org.
// ---------------------------------------------------------------------------
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn c_chain_types_seed_published_and_isolated_per_org(owner_pool: PgPool) {
    let org_a = OrgId::from_uuid(ORG_A);
    let org_b = OrgId::from_uuid(ORG_B);
    let actor_a = seed_org_and_user(&owner_pool, ORG_A, "A").await;
    let actor_b = seed_org_and_user(&owner_pool, ORG_B, "B").await;

    let rt = runtime_role_pool(&owner_pool).await;
    let cmd = command_role_pool(&owner_pool).await;
    let store = PgOntologyStore::new(rt).with_command_pool(cmd);

    scope_org(org_a, async {
        seed_c_chain_object_types(&store, actor_a, AT)
            .await
            .expect("seed org A")
    })
    .await;
    scope_org(org_b, async {
        seed_c_chain_object_types(&store, actor_b, AT)
            .await
            .expect("seed org B")
    })
    .await;

    for key in [CONTRACT_KEY, POSITION_KEY, POSTING_KEY] {
        let detail_a = scope_org(org_a, async { store.get_object_type(key, None).await })
            .await
            .unwrap_or_else(|e| panic!("{key} visible under org A: {e}"));
        assert_eq!(
            detail_a.object_type.lifecycle_state,
            SchemaLifecycleState::Published,
            "{key} must be published"
        );
        assert!(
            detail_a.actions.iter().any(|a| a.stable_key == "create"),
            "{key} must ship the generic create action"
        );

        let detail_b = scope_org(org_b, async { store.get_object_type(key, None).await })
            .await
            .unwrap_or_else(|e| panic!("{key} visible under org B: {e}"));
        assert_ne!(
            detail_a.object_type.id, detail_b.object_type.id,
            "{key} must be a DISTINCT row per org (RLS isolation), not shared"
        );
    }

    // The chain links are authored with cardinality + reverse title, and the
    // posting → employee link is intentionally unresolved (backfill lane wires it).
    let contract = scope_org(org_a, async {
        store.get_object_type(CONTRACT_KEY, None).await
    })
    .await
    .unwrap();
    let contract_positions = contract
        .links
        .iter()
        .find(|l| l.stable_key == "positions")
        .unwrap();
    assert_eq!(contract_positions.reverse_title.as_deref(), Some("계약"));
    assert!(
        contract_positions.to_object_type_id.is_some(),
        "contract → position link must resolve to the position type"
    );
    let posting = scope_org(org_a, async {
        store.get_object_type(POSTING_KEY, None).await
    })
    .await
    .unwrap();
    let posting_employee = posting
        .links
        .iter()
        .find(|l| l.stable_key == "employee")
        .unwrap();
    assert!(
        posting_employee.to_object_type_id.is_none(),
        "posting → employee target is unresolved until the backfill lane registers it"
    );

    // Fail-closed: no org armed ⇒ the read errors, never leaks.
    assert!(
        store.get_object_type(CONTRACT_KEY, None).await.is_err(),
        "with no org armed the read must fail closed"
    );
}

// ---------------------------------------------------------------------------
// (b) + (c) Instances link into the chain and traverse returns downstream nodes,
// org-scoped.
// ---------------------------------------------------------------------------
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn c_chain_instances_link_and_traverse_downstream(owner_pool: PgPool) {
    let org_a = OrgId::from_uuid(ORG_A);
    let org_b = OrgId::from_uuid(ORG_B);
    let actor = seed_org_and_user(&owner_pool, ORG_A, "A").await;
    seed_org_and_user(&owner_pool, ORG_B, "B").await;

    let rt = runtime_role_pool(&owner_pool).await;
    let cmd = command_role_pool(&owner_pool).await;
    let store = PgOntologyStore::new(rt.clone()).with_command_pool(cmd);
    let instances = PgInstanceStore::new(rt.clone());

    // Seed the chain + resolve the type ids and the forward link ids.
    let (contract_type, position_type, posting_type, positions_link, postings_link): (
        ObjectTypeId,
        ObjectTypeId,
        ObjectTypeId,
        LinkTypeId,
        LinkTypeId,
    ) = scope_org(org_a, async {
        seed_c_chain_object_types(&store, actor, AT)
            .await
            .expect("seed org A");
        let contract = store.get_object_type(CONTRACT_KEY, None).await.unwrap();
        let position = store.get_object_type(POSITION_KEY, None).await.unwrap();
        let posting = store.get_object_type(POSTING_KEY, None).await.unwrap();
        (
            contract.object_type.id,
            position.object_type.id,
            posting.object_type.id,
            link_id(&contract, "positions"),
            link_id(&position, "postings"),
        )
    })
    .await;

    // Create one instance of each type, well-formed against its property schema.
    let (contract_inst, position_inst, posting_inst): (InstanceId, InstanceId, InstanceId) =
        scope_org(org_a, async {
            let contract = instances
                .create_instance(
                    actor,
                    CreateInstance {
                        object_type_id: contract_type,
                        title: "C-0001".to_owned(),
                        attributes: serde_json::json!({
                            "client": "코스콕 유지보수",
                            "monthly_fee": 5_000_000,
                            "period": {"from": "2026-01-01", "to": "2026-12-31"},
                            "status": "active",
                            "margin": 18.5
                        }),
                        valid_from: Some(AT),
                        action_type_id: None,
                        reason: Some("계약 체결".to_owned()),
                    },
                    TraceContext::generate(),
                    AT,
                )
                .await
                .expect("create contract instance");
            let position = instances
                .create_instance(
                    actor,
                    CreateInstance {
                        object_type_id: position_type,
                        title: "설비 보전 주임".to_owned(),
                        attributes: serde_json::json!({
                            "worksite": Uuid::new_v4().to_string(),
                            "job_function": "설비 보전",
                            "job_title": "주임",
                            "headcount": 3
                        }),
                        valid_from: Some(AT),
                        action_type_id: None,
                        reason: None,
                    },
                    TraceContext::generate(),
                    AT,
                )
                .await
                .expect("create position instance");
            let posting = instances
                .create_instance(
                    actor,
                    CreateInstance {
                        object_type_id: posting_type,
                        title: "2026 상반기 채용".to_owned(),
                        attributes: serde_json::json!({
                            "scope": "external",
                            "fill_count": 2,
                            "deadline": "2026-08-31"
                        }),
                        valid_from: Some(AT),
                        action_type_id: None,
                        reason: None,
                    },
                    TraceContext::generate(),
                    AT,
                )
                .await
                .expect("create posting instance");
            (
                contract.instance.id,
                position.instance.id,
                posting.instance.id,
            )
        })
        .await;

    // Wire the chain edges: contract → position → posting.
    scope_org(org_a, async {
        instances
            .create_link(
                actor,
                positions_link,
                contract_inst,
                position_inst,
                Some(AT),
                TraceContext::generate(),
                AT,
            )
            .await
            .expect("link contract → position");
        instances
            .create_link(
                actor,
                postings_link,
                position_inst,
                posting_inst,
                Some(AT),
                TraceContext::generate(),
                AT,
            )
            .await
            .expect("link position → posting");
    })
    .await;

    // Traverse from the contract: both downstream nodes are reached at the right
    // depths, over both live edges.
    let graph = scope_org(org_a, async {
        instances.traverse(contract_inst, None, 8).await
    })
    .await
    .expect("traverse the chain");

    assert_eq!(graph.root, contract_inst);
    assert_eq!(graph.edges.len(), 2, "both chain edges are returned");
    let depth_of = |id: InstanceId| {
        graph
            .nodes
            .iter()
            .find(|n| n.instance_id == id)
            .unwrap_or_else(|| panic!("node {id} must be in the traversal"))
            .depth
    };
    assert_eq!(depth_of(contract_inst), 0, "contract is the root");
    assert_eq!(depth_of(position_inst), 1, "position is one hop downstream");
    assert_eq!(depth_of(posting_inst), 2, "posting is two hops downstream");

    // (c) Cross-tenant: org B walking org A's contract id sees no edges and no
    // hydrated nodes — the whole chain is invisible under RLS as console_rt.
    let cross = scope_org(org_b, async {
        instances.traverse(contract_inst, None, 8).await
    })
    .await
    .expect("traverse under org B returns a graph, not an error");
    assert!(
        cross.edges.is_empty(),
        "org B must not see org A's chain edges"
    );
    assert!(
        cross.nodes.is_empty(),
        "org B must not hydrate org A's chain nodes"
    );
}
