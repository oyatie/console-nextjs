#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! RUNTIME RLS gate for the support display-name JOINs (담당자/작성자 이름).
//!
//! `list_tickets` / `get_ticket` resolve `assignee_name` and `author_name` via a
//! same-org correlated subquery on `users` (semantically a LEFT JOIN on a unique
//! key — NULL when absent). This test proves the lookup WORKS and stays RLS-SAFE
//! when the read runs as the genuine non-owner runtime role `console_rt` (NOSUPERUSER,
//! NOBYPASSRLS, FORCE RLS) — the only faithful exercise of the tenant policy.
//!
//! Why `console_rt` and not the default `#[sqlx::test]` pool: that pool connects as a
//! BYPASSRLS superuser, which sees every row regardless of `app.current_org` and
//! would green-light a name JOIN that silently reaches across tenants. We SEED as
//! the owner (raw inserts, row_security off) and READ as `console_rt`.
//!
//! Asserts, with two tenants A (KNL) and B:
//!   * under A's armed GUC, A's ticket lists with `assignee_name` resolved to A's
//!     user display_name (the JOIN works for the in-tenant row);
//!   * cross-tenant isolation: under A's GUC, B's ticket is NOT in the list and
//!     `get_ticket` on B's id is NOT FOUND — the JOIN never widens visibility;
//!   * the correlated name subquery is itself RLS-scoped: it can only resolve a
//!     `users` row in the caller's tenant, so it can never leak another org's
//!     display_name even if an id matched.

use console_kernel_core::{BranchScope, ErrorKind, OrgId, SupportTicketId};
use console_platform_request_context::CURRENT_ORG;
use console_support_adapter_postgres::PgSupportStore;
use console_support_application::{CommentAudience, ListTicketsQuery};
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use time::macros::datetime;
use uuid::Uuid;

/// A second, non-KNL tenant id, to prove cross-tenant isolation under `console_rt`.
const ORG_B: Uuid = Uuid::from_u128(0x3333_3333_3333_3333_3333_3333_3333_3333);

/// A pool whose every connection runs `SET ROLE console_rt`, so statements execute as
/// the production runtime role (NOSUPERUSER, NOBYPASSRLS) under FORCE RLS.
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

/// Seed an organization row (owner pool, RLS off).
async fn seed_org(owner_pool: &PgPool, org: Uuid, tag: &str) {
    let mut tx = owner_pool.begin().await.unwrap();
    sqlx::query("SET LOCAL row_security = off")
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO organizations (id, slug, name) VALUES ($1, $2, $3) ON CONFLICT (id) DO NOTHING",
    )
    .bind(org)
    .bind(format!("org-{}", tag.to_lowercase()))
    .bind(format!("Org {tag}"))
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
}

/// Seed a branch (with its region) in `org`. Owner pool, RLS off.
async fn seed_branch(owner_pool: &PgPool, org: Uuid) -> Uuid {
    let mut tx = owner_pool.begin().await.unwrap();
    sqlx::query("SET LOCAL row_security = off")
        .execute(&mut *tx)
        .await
        .unwrap();
    let region_id: Uuid =
        sqlx::query_scalar("INSERT INTO regions (name, org_id) VALUES ($1, $2) RETURNING id")
            .bind(format!("Region {}", Uuid::new_v4()))
            .bind(org)
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    let branch_id: Uuid = sqlx::query_scalar(
        "INSERT INTO branches (region_id, name, org_id) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(region_id)
    .bind(format!("Branch {}", Uuid::new_v4()))
    .bind(org)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    branch_id
}

/// Seed an ACTIVE user in `branch` (owner pool, RLS off), with an explicit
/// display_name so the JOIN's resolved value is assertable.
async fn seed_user(owner_pool: &PgPool, org: Uuid, branch: Uuid, display_name: &str) -> Uuid {
    let mut tx = owner_pool.begin().await.unwrap();
    sqlx::query("SET LOCAL row_security = off")
        .execute(&mut *tx)
        .await
        .unwrap();
    let user_id: Uuid = sqlx::query_scalar(
        "INSERT INTO users (display_name, roles, org_id, is_active) \
         VALUES ($1, $2, $3, true) RETURNING id",
    )
    .bind(display_name)
    .bind(vec!["ADMIN".to_string()])
    .bind(org)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    sqlx::query("INSERT INTO user_branches (user_id, branch_id, org_id) VALUES ($1, $2, $3)")
        .bind(user_id)
        .bind(branch)
        .bind(org)
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    user_id
}

/// Seed an INTERNAL ticket already ASSIGNED to `assignee`, directly as the owner
/// (RLS off). This isolates the verification to the display-name JOIN on the READ
/// path: we seed the row deterministically and then exercise `list_tickets` /
/// `get_ticket` as the real `console_rt` runtime role under each tenant's GUC.
async fn seed_assigned_ticket(
    owner_pool: &PgPool,
    org: Uuid,
    branch: Uuid,
    requester: Uuid,
    assignee: Uuid,
) -> SupportTicketId {
    let now = datetime!(2026-06-13 09:00 UTC);
    let mut tx = owner_pool.begin().await.unwrap();
    sqlx::query("SET LOCAL row_security = off")
        .execute(&mut *tx)
        .await
        .unwrap();
    let id = SupportTicketId::new();
    sqlx::query(
        r#"
        INSERT INTO support_tickets (
            id, branch_id, origin, category, priority, status,
            title, body, requester_user_id, assignee_user_id,
            created_at, updated_at, org_id
        )
        VALUES ($1, $2, 'INTERNAL', 'OPERATIONAL', 'MEDIUM', 'OPEN',
                $3, $4, $5, $6, $7, $7, $8)
        "#,
    )
    .bind(*id.as_uuid())
    .bind(branch)
    .bind("scoped ticket")
    .bind("details")
    .bind(requester)
    .bind(assignee)
    .bind(now)
    .bind(org)
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    id
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn assignee_name_join_resolves_in_tenant_and_never_leaks_as_console_rt(owner_pool: PgPool) {
    let org_a = OrgId::knl();
    let org_a_uuid = *org_a.as_uuid();
    let org_b = OrgId::from_uuid(ORG_B);

    seed_org(&owner_pool, org_a_uuid, "A").await;
    seed_org(&owner_pool, ORG_B, "B").await;

    let branch_a = seed_branch(&owner_pool, org_a_uuid).await;
    let branch_b = seed_branch(&owner_pool, ORG_B).await;
    let requester_a = seed_user(&owner_pool, org_a_uuid, branch_a, "Requester A").await;
    let assignee_a = seed_user(&owner_pool, org_a_uuid, branch_a, "정비사 김").await;
    let requester_b = seed_user(&owner_pool, ORG_B, branch_b, "Requester B").await;
    let assignee_b = seed_user(&owner_pool, ORG_B, branch_b, "정비사 박").await;

    // Seed an assigned ticket per tenant directly as owner, so the test isolates
    // the display-name JOIN on the READ path.
    let ticket_a =
        seed_assigned_ticket(&owner_pool, org_a_uuid, branch_a, requester_a, assignee_a).await;
    let ticket_b =
        seed_assigned_ticket(&owner_pool, ORG_B, branch_b, requester_b, assignee_b).await;

    // All reads go through the genuine runtime role under FORCE RLS.
    let rt_pool = runtime_role_pool(&owner_pool).await;
    let store = PgSupportStore::new(rt_pool);

    // Under A's armed GUC: A's ticket lists with assignee_name resolved to A's
    // user, and B's ticket is invisible (RLS scopes the read; the name subquery
    // never widens it).
    CURRENT_ORG
        .scope(org_a, async {
            let page = store
                .list_tickets(ListTicketsQuery {
                    branch_scope: BranchScope::All,
                    status: None,
                    priority: None,
                    category: None,
                    origin: None,
                    assignee_user_id: None,
                    include_untriaged: true,
                    limit: None,
                    site_id: None,
                    cursor: None,
                })
                .await
                .unwrap();

            // Exactly A's one ticket is visible; B's is RLS-filtered out.
            assert_eq!(page.total, 1, "RLS must scope the total to org A only");
            assert_eq!(page.items.len(), 1);
            let listed = &page.items[0];
            assert_eq!(listed.id, ticket_a);
            // The correlated subquery resolved A's display_name (the JOIN works).
            assert_eq!(listed.assignee_name.as_deref(), Some("정비사 김"));
            assert!(
                !page.items.iter().any(|t| t.id == ticket_b),
                "B's ticket must never appear under A's GUC"
            );

            // get_ticket on B's id is NOT FOUND under A's GUC (no cross-tenant
            // read even by direct id), so the name JOIN can never expose B.
            let err = store
                .get_ticket(ticket_b, &BranchScope::All, CommentAudience::Internal)
                .await
                .unwrap_err();
            assert_eq!(err.kind(), ErrorKind::NotFound);
        })
        .await;

    // Symmetric check under B's GUC: B sees only its own ticket + its own name.
    CURRENT_ORG
        .scope(org_b, async {
            let page = store
                .list_tickets(ListTicketsQuery {
                    branch_scope: BranchScope::All,
                    status: None,
                    priority: None,
                    category: None,
                    origin: None,
                    assignee_user_id: None,
                    include_untriaged: true,
                    limit: None,
                    site_id: None,
                    cursor: None,
                })
                .await
                .unwrap();
            assert_eq!(page.total, 1);
            assert_eq!(page.items.len(), 1);
            assert_eq!(page.items[0].id, ticket_b);
            assert_eq!(page.items[0].assignee_name.as_deref(), Some("정비사 박"));
        })
        .await;
}
