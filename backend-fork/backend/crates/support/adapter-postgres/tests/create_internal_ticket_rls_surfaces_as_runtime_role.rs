#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! RUNTIME RLS gate for the support-ticket CREATE path (`create_internal_ticket`).
//!
//! `create_internal_ticket` runs through `with_audit`, which arms the
//! transaction-local `app.current_org` GUC from `event.org_id` BEFORE the closure
//! runs. The closure then (1) SELECTs the actor's branch membership via
//! `ensure_active_user_in_branch` and (2) INSERTs the ticket. Under FORCE RLS the
//! membership SELECT is filtered and the INSERT's `WITH CHECK` is evaluated
//! against the GUC — so if the audit event is NOT armed with `.with_org(org)`,
//! BOTH the membership lookup and the insert fail closed as the real `console_rt`
//! runtime role. The existing `support_tickets.rs` tests use the default
//! `#[sqlx::test]` BYPASSRLS superuser pool, which masks that gap entirely.
//!
//! This test exercises the create as the genuine non-owner runtime role `console_rt`
//! (NOSUPERUSER, NOBYPASSRLS, FORCE RLS) — the only faithful exercise of the
//! tenant policy. It asserts, with two tenants A (KNL) and B:
//!   * under A's armed GUC, `create_internal_ticket` SUCCEEDS and the row is
//!     org-scoped to A (visible to A, invisible to B);
//!   * cross-tenant isolation: under B's GUC, A's created ticket is NOT FOUND;
//!   * fail-closed: a raw `console_rt` INSERT into `support_tickets` with NO GUC
//!     armed is rejected by the `org_isolation` WITH CHECK policy — proving the
//!     create only succeeds because `with_audit` arms `app.current_org`.

use console_kernel_core::{BranchId, BranchScope, ErrorKind, OrgId, TraceContext, UserId};
use console_platform_request_context::CURRENT_ORG;
use console_support_adapter_postgres::PgSupportStore;
use console_support_application::{CommentAudience, CreateInternalTicketCommand, ListTicketsQuery};
use console_support_domain::{TicketCategory, TicketOrigin, TicketPriority};
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use time::macros::datetime;
use uuid::Uuid;

/// A second, non-KNL tenant id, to prove cross-tenant isolation under `console_rt`.
const ORG_B: Uuid = Uuid::from_u128(0x4444_4444_4444_4444_4444_4444_4444_4444);

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

/// Seed an ACTIVE user that is a member of `branch` (owner pool, RLS off), so the
/// `ensure_active_user_in_branch` membership SELECT can resolve it ONLY when the
/// reading role's `app.current_org` is armed to this org.
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

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn create_internal_ticket_arms_org_and_is_tenant_isolated_as_console_rt(owner_pool: PgPool) {
    let org_a = OrgId::knl();
    let org_a_uuid = *org_a.as_uuid();
    let org_b = OrgId::from_uuid(ORG_B);

    seed_org(&owner_pool, org_a_uuid, "A").await;
    seed_org(&owner_pool, ORG_B, "B").await;

    let branch_a = seed_branch(&owner_pool, org_a_uuid).await;
    let branch_b = seed_branch(&owner_pool, ORG_B).await;
    let actor_a = seed_user(&owner_pool, org_a_uuid, branch_a, "Staff A").await;
    let _actor_b = seed_user(&owner_pool, ORG_B, branch_b, "Staff B").await;

    // Every statement runs as the genuine runtime role under FORCE RLS.
    let rt_pool = runtime_role_pool(&owner_pool).await;
    let store = PgSupportStore::new(rt_pool.clone());
    let now = datetime!(2026-06-13 09:00 UTC);

    // Under A's armed GUC: the create SUCCEEDS. Before the `.with_org(org)` fix
    // this failed closed — the membership SELECT saw no rows (RLS) so
    // ensure_active_user_in_branch rejected, and the INSERT's WITH CHECK failed.
    let created = CURRENT_ORG
        .scope(org_a, async {
            store
                .create_internal_ticket(CreateInternalTicketCommand {
                    actor: UserId::from_uuid(actor_a),
                    branch_id: BranchId::from_uuid(branch_a),
                    category: TicketCategory::Operational,
                    priority: TicketPriority::Medium,
                    title: "Forklift hydraulic leak".to_owned(),
                    body: "Unit 7 leaking under load".to_owned(),
                    trace: TraceContext::generate(),
                    occurred_at: now,
                })
                .await
                .expect("create_internal_ticket must succeed as console_rt with armed GUC")
        })
        .await;
    assert_eq!(created.origin, TicketOrigin::Internal);
    assert_eq!(created.branch_id, Some(BranchId::from_uuid(branch_a)));

    // Under A's GUC the created ticket is visible and is the only one.
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
            assert_eq!(page.total, 1, "RLS must scope the total to org A only");
            assert_eq!(page.items.len(), 1);
            assert_eq!(page.items[0].id, created.id);
        })
        .await;

    // Cross-tenant isolation: under B's GUC, A's ticket is NOT FOUND (no
    // cross-tenant read even by direct id) and B sees an empty list.
    CURRENT_ORG
        .scope(org_b, async {
            let err = store
                .get_ticket(created.id, &BranchScope::All, CommentAudience::Internal)
                .await
                .unwrap_err();
            assert_eq!(err.kind(), ErrorKind::NotFound);

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
            assert_eq!(page.total, 0, "B must not see A's ticket");
        })
        .await;

    // Fail-closed proof: a raw console_rt INSERT into support_tickets with NO GUC
    // armed is rejected by the org_isolation WITH CHECK policy. This is exactly
    // the path create_internal_ticket would hit if `.with_org(org)` were dropped.
    let unarmed_insert = sqlx::query(
        r#"
        INSERT INTO support_tickets (
            id, branch_id, origin, category, priority, status,
            title, body, requester_user_id, created_at, updated_at, org_id
        )
        VALUES ($1, $2, 'INTERNAL', 'OPERATIONAL', 'MEDIUM', 'OPEN',
                $3, $4, $5, $6, $6, $7)
        "#,
    )
    .bind(Uuid::new_v4())
    .bind(branch_a)
    .bind("unarmed")
    .bind("should be rejected by RLS")
    .bind(actor_a)
    .bind(now)
    .bind(org_a_uuid)
    .execute(&rt_pool)
    .await;
    assert!(
        unarmed_insert.is_err(),
        "console_rt INSERT without an armed app.current_org must be rejected by FORCE RLS WITH CHECK"
    );
}
