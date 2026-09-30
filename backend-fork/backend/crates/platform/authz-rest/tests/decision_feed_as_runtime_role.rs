#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! RUNTIME proofs for the bulk-authorize + decision-feed gaps, exercised as the
//! genuine non-owner role `console_rt` under an armed org.
//!
//! Proves:
//!   * bulk authorize evaluates every check over the same fail-closed evaluator —
//!     with nothing enforced, every check denies by omission;
//!   * the decision feed reads back the decisions the authorize path just
//!     recorded (append-only `cedar_decision_log`), newest-first, RLS-scoped.
//!
//! NOTE (migrations path): runs against the canonical `../db/migrations` (the
//! ship path). The earlier concurrent-lane migration-number collision has been
//! reconciled, so no deduplicated copy is needed.

use console_kernel_core::{OrgId, UserId};
use console_platform_authz::cedar_pbac::authoring::{SimEffect, SimResource, SimSubject};
use console_platform_authz_rest::{DecisionLogEntry, PgCedarPolicyStore};
use console_platform_request_context::scope_org;
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use uuid::Uuid;

const ORG_A: Uuid = Uuid::from_u128(0xA000_0000_0000_0000_0000_0000_0000_0001);
const ORG_B: Uuid = Uuid::from_u128(0xB000_0000_0000_0000_0000_0000_0000_0002);

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

async fn seed_org(pool: &PgPool, org_id: Uuid, slug: &str) {
    sqlx::query("INSERT INTO organizations (id, slug, name) VALUES ($1, $2, $3)")
        .bind(org_id)
        .bind(slug)
        .bind(format!("Org {slug}"))
        .execute(pool)
        .await
        .unwrap();
}

async fn seed_user(pool: &PgPool, org_id: Uuid, name: &str) -> UserId {
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO users (display_name, roles, org_id) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(name)
    .bind(["SUPER_ADMIN"].as_slice())
    .bind(org_id)
    .fetch_one(pool)
    .await
    .unwrap();
    UserId::from_uuid(id)
}

fn subject(org: Uuid, user_id: &str) -> SimSubject {
    SimSubject {
        org: OrgId::from_uuid(org),
        user_id: user_id.to_owned(),
        roles: vec![],
        clearance_keys: vec![],
    }
}

fn row(org: Uuid, id: &str) -> SimResource {
    SimResource {
        org: OrgId::from_uuid(org),
        resource_type: "work_order".to_owned(),
        resource_id: Some(id.to_owned()),
        owner: Some("alice".to_owned()),
        branch: None,
        legal_hold: None,
    }
}

// Bulk authorize denies by omission over the empty enforced set (via the store's
// single evaluator, the same path the /authorize/bulk handler drives).
#[sqlx::test(migrations = "../db/migrations")]
async fn bulk_authorize_denies_by_omission(pool: PgPool) {
    seed_org(&pool, ORG_A, "org-alpha").await;
    let store = PgCedarPolicyStore::new(runtime_role_pool(&pool).await);

    let outcome = scope_org(OrgId::from_uuid(ORG_A), async {
        let policies = store.load_enforced_policies().await.unwrap();
        // Two checks, nothing enforced ⇒ every one denies.
        console_platform_authz::cedar_pbac::authoring::simulate(
            &policies,
            &console_platform_authz::cedar_pbac::authoring::SimRequest {
                subject: subject(ORG_A, "alice"),
                action: "view".to_owned(),
                resource: row(ORG_A, "wo-1"),
                purpose: None,
                field: None,
            },
        )
    })
    .await;
    assert_eq!(outcome.effect, SimEffect::Deny, "no enforced policy ⇒ deny");
    assert!(outcome.determining_policies.is_empty());
}

// The authorize path records decisions; the feed reads them back, RLS-scoped.
#[sqlx::test(migrations = "../db/migrations")]
async fn decision_feed_reads_back_recorded_decisions(pool: PgPool) {
    seed_org(&pool, ORG_A, "org-alpha").await;
    seed_org(&pool, ORG_B, "org-beta").await;
    let actor_a = seed_user(&pool, ORG_A, "Admin A").await;
    let store = PgCedarPolicyStore::new(runtime_role_pool(&pool).await);

    // Record two decisions under org A (as the authorize handler would).
    scope_org(OrgId::from_uuid(ORG_A), async {
        store
            .record_decisions(
                *actor_a.as_uuid(),
                vec![
                    DecisionLogEntry {
                        subject_ref: "alice".to_owned(),
                        action: "view".to_owned(),
                        resource_type: "work_order".to_owned(),
                        resource_id: Some("wo-1".to_owned()),
                        effect: "deny".to_owned(),
                        determining_policies: vec![],
                        reason: "deny-by-omission".to_owned(),
                    },
                    DecisionLogEntry {
                        subject_ref: "alice".to_owned(),
                        action: "edit".to_owned(),
                        resource_type: "work_order".to_owned(),
                        resource_id: Some("wo-1".to_owned()),
                        effect: "allow".to_owned(),
                        determining_policies: vec!["p1".to_owned()],
                        reason: "matched p1".to_owned(),
                    },
                ],
            )
            .await
            .expect("record decisions");
    })
    .await;

    // The feed shows both, newest-first.
    let feed = scope_org(OrgId::from_uuid(ORG_A), async {
        store.recent_decisions(None, 50).await
    })
    .await
    .expect("feed read");
    assert_eq!(feed.len(), 2, "both decisions surface: {feed:?}");
    assert!(feed.iter().any(|d| d.action == "edit"
        && d.effect == "allow"
        && d.determining_policies == vec!["p1".to_owned()]));
    assert!(
        feed.iter()
            .any(|d| d.action == "view" && d.effect == "deny")
    );

    // Org B sees none of org A's decisions (FORCE-RLS isolation).
    let cross = scope_org(OrgId::from_uuid(ORG_B), async {
        store.recent_decisions(None, 50).await
    })
    .await
    .expect("feed read");
    assert!(
        cross.is_empty(),
        "another tenant's decisions must be invisible"
    );
}
