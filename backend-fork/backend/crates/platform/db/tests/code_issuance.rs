#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Shared object-code issuance (BE-OBJ slice 2, item 1): a per-(org, kind)
//! monotonic counter generalized from the work-order request-number counter
//! (`work_order_request_counters`), exercised as the genuine non-owner runtime
//! role `console_rt` — FORCE RLS on `object_code_counters` is only meaningful
//! against a non-owner/non-BYPASSRLS role (see `rls_isolation.rs`'s rationale;
//! the default `#[sqlx::test]` pool is a BYPASSRLS superuser).

use console_kernel_core::OrgId;
use console_platform_db::{DbError, issue_code, with_org_conn};
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use uuid::Uuid;

const ORG_A: Uuid = Uuid::from_u128(0xAAAA_AAAA_AAAA_AAAA_AAAA_AAAA_AAAA_AAAA);
const ORG_B: Uuid = Uuid::from_u128(0xBBBB_BBBB_BBBB_BBBB_BBBB_BBBB_BBBB_BBBB);

async fn seed_org(pool: &PgPool, org: Uuid) {
    sqlx::query("INSERT INTO organizations (id, slug, name) VALUES ($1, $2, $3)")
        .bind(org)
        .bind(format!("org-{}", org.simple()))
        .bind(format!("Org {org}"))
        .execute(pool)
        .await
        .unwrap();
}

/// A pool whose every connection drops to the genuine runtime role `console_rt`
/// (NOSUPERUSER, NOBYPASSRLS) before use, so FORCE RLS on
/// `object_code_counters` is actually exercised. Copied pattern from
/// `rls_isolation.rs` / `workorder/adapter-postgres` runtime-role tests.
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

#[sqlx::test(migrations = "./migrations")]
async fn issues_monotonic_per_org_codes_as_runtime_role(pool: PgPool) {
    seed_org(&pool, ORG_A).await;
    seed_org(&pool, ORG_B).await;
    let rt = runtime_role_pool(&pool).await;

    let first = with_org_conn::<_, _, DbError>(&rt, OrgId::from_uuid(ORG_A), |tx| {
        Box::pin(async move { issue_code(tx, OrgId::from_uuid(ORG_A), "approval_run").await })
    })
    .await
    .unwrap();
    assert_eq!(first, "AP-1");

    let second = with_org_conn::<_, _, DbError>(&rt, OrgId::from_uuid(ORG_A), |tx| {
        Box::pin(async move { issue_code(tx, OrgId::from_uuid(ORG_A), "approval_run").await })
    })
    .await
    .unwrap();
    assert_eq!(second, "AP-2", "sequence is monotonic per (org, kind)");

    // A different org starts its own sequence at 1 — no cross-tenant leakage,
    // proven under console_rt's FORCE RLS (not merely app-level org_id scoping).
    let org_b_first = with_org_conn::<_, _, DbError>(&rt, OrgId::from_uuid(ORG_B), |tx| {
        Box::pin(async move { issue_code(tx, OrgId::from_uuid(ORG_B), "approval_run").await })
    })
    .await
    .unwrap();
    assert_eq!(
        org_b_first, "AP-1",
        "a second org's sequence must start at 1, independent of org A's"
    );
}

/// Concurrency-safety proof: N concurrent issuers hitting the SAME (org,
/// kind) row must serialize on the `INSERT … ON CONFLICT DO UPDATE …
/// RETURNING` row lock rather than racing — every sequence number is
/// distinct and the result is gapless 1..=N (no lost update, no duplicate).
#[sqlx::test(migrations = "./migrations")]
async fn concurrent_issuance_is_serialized_and_gapless(pool: PgPool) {
    seed_org(&pool, ORG_A).await;
    let rt = runtime_role_pool(&pool).await;

    const CONCURRENT_ISSUERS: usize = 10;
    let mut handles = Vec::with_capacity(CONCURRENT_ISSUERS);
    for _ in 0..CONCURRENT_ISSUERS {
        let rt = rt.clone();
        handles.push(tokio::spawn(async move {
            with_org_conn::<_, _, DbError>(&rt, OrgId::from_uuid(ORG_A), |tx| {
                Box::pin(
                    async move { issue_code(tx, OrgId::from_uuid(ORG_A), "approval_run").await },
                )
            })
            .await
            .unwrap()
        }));
    }

    let mut sequences: Vec<i64> = Vec::with_capacity(CONCURRENT_ISSUERS);
    for handle in handles {
        let code = handle.await.unwrap();
        let seq: i64 = code
            .strip_prefix("AP-")
            .expect("issued code must carry the AP- prefix")
            .parse()
            .expect("sequence suffix must be numeric");
        sequences.push(seq);
    }
    sequences.sort_unstable();
    assert_eq!(
        sequences,
        (1..=i64::try_from(CONCURRENT_ISSUERS).unwrap()).collect::<Vec<_>>(),
        "concurrent issuance must serialize into a gapless, duplicate-free \
         1..=N run: {sequences:?}"
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn rejects_unknown_and_non_issuable_kinds(pool: PgPool) {
    seed_org(&pool, ORG_A).await;
    let rt = runtime_role_pool(&pool).await;

    let unknown = with_org_conn::<_, _, DbError>(&rt, OrgId::from_uuid(ORG_A), |tx| {
        Box::pin(async move { issue_code(tx, OrgId::from_uuid(ORG_A), "banana").await })
    })
    .await;
    assert!(unknown.is_err(), "an unknown kind must error");

    // person is seeded with no code_prefix (id/name-referenced kind, matching
    // the frontend objectRegistry, which gives person no codePrefix either).
    let not_issuable = with_org_conn::<_, _, DbError>(&rt, OrgId::from_uuid(ORG_A), |tx| {
        Box::pin(async move { issue_code(tx, OrgId::from_uuid(ORG_A), "person").await })
    })
    .await;
    assert!(
        not_issuable.is_err(),
        "a kind with no code_prefix must error"
    );
}
