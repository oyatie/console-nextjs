#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! RUNTIME RLS + owner-isolation gate for the todos domain.
//!
//! Proven as the genuine non-owner runtime role `console_rt` (NOSUPERUSER,
//! NOBYPASSRLS, FORCE RLS) — NOT the default `#[sqlx::test]` BYPASSRLS
//! superuser pool, which sees every row and would green-light a broken owner
//! filter. There is no per-person GUC, so owner scoping is enforced in
//! application code; this test is the thing that proves user B cannot list,
//! done-mark, or delete user A's todos, and that another tenant sees nothing.

use console_kernel_core::{ErrorKind, OrgId, TraceContext, UserId};
use console_todos_adapter_postgres::PgTodoStore;
use console_todos_application::{
    CreateTodoCommand, DeleteTodoCommand, ListTodosQuery, SetTodoDoneCommand,
};
use console_todos_domain::TodoRef;
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use time::OffsetDateTime;
use uuid::Uuid;

const OTHER_ORG: Uuid = Uuid::from_u128(0x7303_7303_7303_7303_7303_7303_7303_7303);

async fn runtime_role_pool(owner_pool: &PgPool) -> PgPool {
    for grant in [
        "GRANT SELECT, INSERT, UPDATE, DELETE ON todos TO console_rt",
        "GRANT SELECT, INSERT ON audit_events TO console_rt",
        "GRANT SELECT ON users TO console_rt",
        "GRANT SELECT ON organizations TO console_rt",
    ] {
        sqlx::query(grant).execute(owner_pool).await.unwrap();
    }
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

async fn seed_org(owner_pool: &PgPool, org: Uuid, tag: &str) {
    sqlx::query(
        "INSERT INTO organizations (id, slug, name) VALUES ($1, $2, $3) ON CONFLICT (id) DO NOTHING",
    )
    .bind(org)
    .bind(format!("org-{}", tag.to_lowercase()))
    .bind(format!("Org {tag}"))
    .execute(owner_pool)
    .await
    .unwrap();
}

async fn seed_user(owner_pool: &PgPool, org: Uuid, name: &str) -> UserId {
    let user_id = UserId::new();
    sqlx::query("INSERT INTO users (id, display_name, roles, org_id) VALUES ($1, $2, $3, $4)")
        .bind(user_id.as_uuid())
        .bind(format!("{name} {}", Uuid::new_v4()))
        .bind(Vec::from(["ADMIN"]))
        .bind(org)
        .execute(owner_pool)
        .await
        .unwrap();
    user_id
}

fn create_for(owner: UserId, text: &str) -> CreateTodoCommand {
    CreateTodoCommand {
        owner,
        text: text.to_owned(),
        scopes: Vec::from([TodoRef {
            kind: "site".to_owned(),
            id: Uuid::new_v4().to_string(),
            label: Some("창원 1공장".to_owned()),
        }]),
        links: Vec::from([TodoRef {
            kind: "workOrder".to_owned(),
            id: Uuid::new_v4().to_string(),
            label: None,
        }]),
        trace: TraceContext::generate(),
        occurred_at: OffsetDateTime::now_utc(),
    }
}

fn list_open(owner: UserId) -> ListTodosQuery {
    ListTodosQuery {
        owner,
        include_done: false,
        limit: 50,
    }
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn owner_isolation_and_done_undo_as_runtime_role(owner_pool: PgPool) {
    let rt_pool = runtime_role_pool(&owner_pool).await;
    let knl = OrgId::knl();
    let other = OrgId::from_uuid(OTHER_ORG);
    seed_org(&owner_pool, OTHER_ORG, "Other").await;
    let user_a = seed_user(&owner_pool, *knl.as_uuid(), "Owner A").await;
    let user_b = seed_user(&owner_pool, *knl.as_uuid(), "Owner B").await;

    let store = PgTodoStore::new(rt_pool.clone());

    // Create one todo per user (both under knl).
    let a_todo = console_platform_request_context::scope_org(knl, async {
        store
            .create(create_for(user_a, "지게차 12호 점검 일정 잡기"))
            .await
    })
    .await
    .expect("create for A");
    console_platform_request_context::scope_org(knl, async {
        store.create(create_for(user_b, "월간 보고서 검토")).await
    })
    .await
    .expect("create for B");

    assert_eq!(a_todo.owner_user_id, user_a);
    assert!(!a_todo.done);
    assert_eq!(a_todo.scopes.len(), 1);
    assert_eq!(a_todo.scopes[0].kind, "site");
    assert_eq!(a_todo.links.len(), 1);

    // (a) owner isolation: A sees only A's; B sees only B's.
    let a_list = console_platform_request_context::scope_org(knl, async {
        store.list(list_open(user_a)).await
    })
    .await
    .expect("A list");
    assert_eq!(a_list.items.len(), 1, "A sees exactly one todo");
    assert_eq!(a_list.items[0].id, a_todo.id);

    let b_list = console_platform_request_context::scope_org(knl, async {
        store.list(list_open(user_b)).await
    })
    .await
    .expect("B list");
    assert_eq!(b_list.items.len(), 1);
    assert_ne!(b_list.items[0].id, a_todo.id, "B must never see A's todo");

    // (b) cross-user done-mark and delete: NotFound, not a silent success.
    let cross_done = console_platform_request_context::scope_org(knl, async {
        store
            .set_done(SetTodoDoneCommand {
                owner: user_b,
                todo_id: a_todo.id,
                done: true,
                trace: TraceContext::generate(),
                occurred_at: OffsetDateTime::now_utc(),
            })
            .await
    })
    .await;
    assert_eq!(
        cross_done.expect_err("B marking A's todo must fail").kind(),
        ErrorKind::NotFound
    );

    let cross_delete = console_platform_request_context::scope_org(knl, async {
        store
            .delete(DeleteTodoCommand {
                owner: user_b,
                todo_id: a_todo.id,
                trace: TraceContext::generate(),
                occurred_at: OffsetDateTime::now_utc(),
            })
            .await
    })
    .await;
    assert_eq!(
        cross_delete
            .expect_err("B deleting A's todo must fail")
            .kind(),
        ErrorKind::NotFound
    );

    // (c) done -> undo round-trip on the owner's own todo.
    let marked = console_platform_request_context::scope_org(knl, async {
        store
            .set_done(SetTodoDoneCommand {
                owner: user_a,
                todo_id: a_todo.id,
                done: true,
                trace: TraceContext::generate(),
                occurred_at: OffsetDateTime::now_utc(),
            })
            .await
    })
    .await
    .expect("A marks own done");
    assert!(marked.done);
    assert!(marked.done_at.is_some());

    let open_after = console_platform_request_context::scope_org(knl, async {
        store.list(list_open(user_a)).await
    })
    .await
    .expect("A open list after done");
    assert!(
        open_after.items.is_empty(),
        "done todo leaves the open list"
    );

    let undone = console_platform_request_context::scope_org(knl, async {
        store
            .set_done(SetTodoDoneCommand {
                owner: user_a,
                todo_id: a_todo.id,
                done: false,
                trace: TraceContext::generate(),
                occurred_at: OffsetDateTime::now_utc(),
            })
            .await
    })
    .await
    .expect("A undoes");
    assert!(!undone.done);
    assert!(undone.done_at.is_none(), "undo clears done_at");

    // (d) cross-tenant: under another org's GUC, A's rows are invisible (RLS).
    let cross_tenant = console_platform_request_context::scope_org(other, async {
        store.list(list_open(user_a)).await
    })
    .await
    .expect("cross-tenant list itself succeeds");
    assert_eq!(
        cross_tenant.items.len(),
        0,
        "another tenant sees none of A's todos"
    );
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn delete_and_audit_trail_as_runtime_role(owner_pool: PgPool) {
    let rt_pool = runtime_role_pool(&owner_pool).await;
    let knl = OrgId::knl();
    let user = seed_user(&owner_pool, *knl.as_uuid(), "Busy Owner").await;
    let store = PgTodoStore::new(rt_pool.clone());

    let todo = console_platform_request_context::scope_org(knl, async {
        store.create(create_for(user, "삭제될 항목")).await
    })
    .await
    .expect("create");

    console_platform_request_context::scope_org(knl, async {
        store
            .delete(DeleteTodoCommand {
                owner: user,
                todo_id: todo.id,
                trace: TraceContext::generate(),
                occurred_at: OffsetDateTime::now_utc(),
            })
            .await
    })
    .await
    .expect("delete own todo");

    let remaining = console_platform_request_context::scope_org(knl, async {
        store
            .list(ListTodosQuery {
                owner: user,
                include_done: true,
                limit: 50,
            })
            .await
    })
    .await
    .expect("list after delete");
    assert!(remaining.items.is_empty());

    // Every mutation left an audit event (with_audit envelope).
    let audit_actions: Vec<String> = sqlx::query_scalar(
        "SELECT action FROM audit_events WHERE target_type = 'todo' AND target_id = $1 ORDER BY occurred_at, created_at",
    )
    .bind(todo.id.to_string())
    .fetch_all(&owner_pool)
    .await
    .unwrap();
    assert_eq!(
        audit_actions,
        Vec::from(["todo.create".to_owned(), "todo.delete".to_owned()]),
        "create and delete are both audited"
    );

    // Validation failures reject before any write.
    let invalid = console_platform_request_context::scope_org(knl, async {
        store.create(create_for(user, "   ")).await
    })
    .await;
    assert_eq!(
        invalid.expect_err("blank text must fail").kind(),
        ErrorKind::Validation
    );
}
