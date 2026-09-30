#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! RUNTIME proofs for BE-semantic-backfill's `projected` read path, exercised
//! as the genuine non-owner `console_rt` role (NOSUPERUSER, NOBYPASSRLS, FORCE
//! RLS) — the only faithful exercise of the domain tables' org_isolation
//! policy. The default `#[sqlx::test]` pool is a BYPASSRLS superuser that
//! would green-light a broken policy.
//!
//! Proves:
//!   (a) `seed_projected_domain_object_types` registers all 15 domain tables
//!       as `projected` object types, listed via the registry;
//!   (b) `PgInstanceStore::list_instances` on two of those types (`employee`,
//!       `workflow_definition`) returns the REAL rows already sitting in
//!       `employees` / `workflow_definitions` — not an empty owned-store read;
//!   (c) cross-tenant rows are invisible under another org's GUC (RLS), same
//!       as the owned instance store.

use console_ontology_adapter_postgres::PgOntologyStore;
use console_ontology_adapter_postgres::instances::PgInstanceStore;
use console_ontology_adapter_postgres::seed::{
    EMPLOYEE_KEY, SITE_KEY, WORK_ORDER_KEY, WORKFLOW_DEFINITION_KEY,
    seed_projected_domain_object_types,
};
use console_ontology_domain::{BackingKind, ObjectTypeId};

use console_kernel_core::{OrgId, UserId};
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use time::macros::datetime;
use uuid::Uuid;

const ORG_B: Uuid = Uuid::from_u128(0x4444_4444_4444_4444_4444_4444_4444_4444);

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

/// Insert a real `employees` row directly (as the domain crate's own use-case
/// would), independent of the ontology engine — proving the read path finds
/// data it never wrote itself.
async fn insert_employee(owner_pool: &PgPool, org: Uuid, name: &str) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        r#"
        INSERT INTO employees (id, org_id, company, name, source_filename, source_sheet, source_row, source_key)
        VALUES ($1, $2, 'ACME', $3, 'roster.xlsx', 'Sheet1', 1, $4)
        "#,
    )
    .bind(id)
    .bind(org)
    .bind(name)
    .bind(format!("key-{id}"))
    .execute(owner_pool)
    .await
    .unwrap();
    id
}

async fn insert_workflow_definition(owner_pool: &PgPool, org: Uuid, key: &str, name: &str) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        r#"
        INSERT INTO workflow_definitions (id, org_id, workflow_key, display_name, object_type, status)
        VALUES ($1, $2, $3, $4, 'work_order', 'ACTIVE')
        "#,
    )
    .bind(id)
    .bind(org)
    .bind(key)
    .bind(name)
    .execute(owner_pool)
    .await
    .unwrap();
    id
}

/// Seed all 15 projected domain types for `org`, returning the (employee,
/// workflow_definition) object_type_ids.
async fn seed_projected_types(
    owner_pool: &PgPool,
    org: OrgId,
    actor: UserId,
) -> (ObjectTypeId, ObjectTypeId) {
    console_platform_request_context::scope_org(org, async {
        let store = PgOntologyStore::new(runtime_role_pool(owner_pool).await)
            .with_command_pool(command_role_pool(owner_pool).await);
        let published =
            seed_projected_domain_object_types(&store, actor, datetime!(2026-07-10 09:00 UTC))
                .await
                .expect("seed projected domain object types");
        assert_eq!(published.len(), 15, "all 15 domain tables must register");
        assert!(
            published
                .iter()
                .all(|s| s.backing_kind == BackingKind::Projected),
            "every seeded type must be backing_kind = projected"
        );

        let employee_id = published
            .iter()
            .find(|s| s.stable_key == EMPLOYEE_KEY)
            .expect("employee type registered")
            .id;
        let workflow_id = published
            .iter()
            .find(|s| s.stable_key == WORKFLOW_DEFINITION_KEY)
            .expect("workflow_definition type registered")
            .id;

        // Registry list surfaces both (object-types REST is a thin pass-through
        // over this same store call).
        let listed = store.list_object_types().await.expect("list object types");
        assert!(listed.iter().any(|s| s.id == employee_id));
        assert!(listed.iter().any(|s| s.id == workflow_id));

        (employee_id, workflow_id)
    })
    .await
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn projected_list_returns_real_domain_rows(owner_pool: PgPool) {
    let rt = runtime_role_pool(&owner_pool).await;
    let org = OrgId::knl();
    let org_uuid = *org.as_uuid();
    let actor = seed_org_and_user(&owner_pool, org_uuid, "a").await;
    let (employee_type, workflow_type) = seed_projected_types(&owner_pool, org, actor).await;

    insert_employee(&owner_pool, org_uuid, "김철수").await;
    insert_employee(&owner_pool, org_uuid, "이영희").await;
    insert_workflow_definition(&owner_pool, org_uuid, "wo.escalate", "에스컬레이션").await;

    console_platform_request_context::scope_org(org, async {
        let store = PgInstanceStore::new(rt.clone());

        // (b) employee: 2 real rows, attributes carry the real column values,
        // title resolves via title_property_key ("name").
        let employees = store.list_instances(employee_type).await.unwrap();
        assert_eq!(employees.len(), 2, "must read the 2 real employees rows");
        let names: Vec<String> = employees.iter().map(|e| e.instance.title.clone()).collect();
        assert!(names.contains(&"김철수".to_owned()));
        assert!(names.contains(&"이영희".to_owned()));
        for e in &employees {
            assert_eq!(e.revision.attributes["company"], "ACME");
            assert_eq!(e.revision.version, 1);
        }

        // workflow_definition: 1 real row.
        let workflows = store.list_instances(workflow_type).await.unwrap();
        assert_eq!(workflows.len(), 1);
        assert_eq!(workflows[0].instance.title, "에스컬레이션");
        assert_eq!(
            workflows[0].revision.attributes["workflow_key"],
            "wo.escalate"
        );
        assert_eq!(workflows[0].revision.attributes["status"], "ACTIVE");
    })
    .await;

    // Projected fk_links carry the reverse title (the console relationship
    // tab's "← arrow"), same as the C-chain links — not left blank.
    console_platform_request_context::scope_org(org, async {
        let store = PgOntologyStore::new(owner_pool.clone());
        let site = store.get_object_type(SITE_KEY, None).await.unwrap();
        let site_customer = site
            .links
            .iter()
            .find(|l| l.stable_key == "customer")
            .expect("site → customer link");
        assert_eq!(site_customer.reverse_title.as_deref(), Some("현장"));

        let work_order = store.get_object_type(WORK_ORDER_KEY, None).await.unwrap();
        for l in &work_order.links {
            assert_eq!(
                l.reverse_title.as_deref(),
                Some("작업지시"),
                "work_order link {} must carry its reverse title",
                l.stable_key
            );
        }
    })
    .await;
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn projected_list_is_cross_tenant_isolated(owner_pool: PgPool) {
    let rt = runtime_role_pool(&owner_pool).await;
    let org_a = OrgId::knl();
    let org_b = OrgId::from_uuid(ORG_B);
    let actor_a = seed_org_and_user(&owner_pool, *org_a.as_uuid(), "a").await;
    let actor_b = seed_org_and_user(&owner_pool, ORG_B, "b").await;

    let (employee_type_a, _) = seed_projected_types(&owner_pool, org_a, actor_a).await;
    let (employee_type_b, _) = seed_projected_types(&owner_pool, org_b, actor_b).await;

    insert_employee(&owner_pool, *org_a.as_uuid(), "A직원").await;
    insert_employee(&owner_pool, ORG_B, "B직원").await;

    // Under org-A's GUC, only A's employee is visible — B's row (and B's
    // distinct object-type version) never leaks in.
    console_platform_request_context::scope_org(org_a, async {
        let store = PgInstanceStore::new(rt.clone());
        let list = store.list_instances(employee_type_a).await.unwrap();
        assert_eq!(list.len(), 1, "org A must see only its own employee row");
        assert_eq!(list[0].instance.title, "A직원");

        // B's object-type version is invisible under A's GUC (RLS on
        // ont_object_types itself) — the type lookup fails closed.
        assert!(
            store.list_instances(employee_type_b).await.is_err(),
            "org A must not resolve org B's object-type version"
        );
    })
    .await;

    // FAIL-CLOSED: no org armed → the read errors, never leaks.
    let store = PgInstanceStore::new(rt.clone());
    assert!(
        store.list_instances(employee_type_a).await.is_err(),
        "read must fail closed without an armed org"
    );
}
