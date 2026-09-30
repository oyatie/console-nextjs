#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! M1 security fix — four-eyes approvals must BIND to the action and be CONSUMED
//! single-use. Exercised as the genuine non-owner role `console_rt` (NOSUPERUSER,
//! NOBYPASSRLS, FORCE RLS): seed as owner, run every gate check/consume as `console_rt`
//! so the tenant policy and grants are faithfully exercised.
//!
//! Before this fix a gate resolved approval with only
//! `WHERE request_ref = $1` — any approved row in the org satisfied any gate,
//! replayably. Each test below is one hole the fix closes:
//!   * happy path — a matching, unconsumed approval passes and is consumed once;
//!   * wrong-kind ref — an approval decided under a different action kind denies;
//!   * wrong-target ref — an approval bound to a different object denies;
//!   * cross-purpose approved ref — an unrelated approved ref never satisfies;
//!   * replay — the same ref used twice denies the second time;
//!   * concurrent double-consume — two racing consumers, exactly one wins;
//!   * peek — the preview check never consumes.

use console_governance_adapter_postgres::PgGovernanceStore;
use console_governance_application::{ApprovalDecision, DecideApprovalCommand};
use console_kernel_core::{OrgId, TraceContext, UserId};
use console_platform_request_context::scope_org;
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use uuid::Uuid;

const ORG: Uuid = Uuid::from_u128(0xA000_0000_0000_0000_0000_0000_0000_00F4);
const KIND: &str = "workflow.publish";

async fn runtime_role_pool(owner_pool: &PgPool) -> PgPool {
    let options = owner_pool.connect_options().as_ref().clone();
    PgPoolOptions::new()
        .max_connections(6)
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

async fn seed_org(pool: &PgPool) {
    sqlx::query("INSERT INTO organizations (id, slug, name) VALUES ($1, 'org-4eyes', 'Org 4eyes')")
        .bind(ORG)
        .execute(pool)
        .await
        .unwrap();
}

async fn seed_user(pool: &PgPool, name: &str) -> UserId {
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO users (display_name, roles, org_id) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(name)
    .bind(["SUPER_ADMIN"].as_slice())
    .bind(ORG)
    .fetch_one(pool)
    .await
    .unwrap();
    UserId::from_uuid(id)
}

/// Record a distinct-approver `approved` decision bound to (`KIND`, `target`) and
/// return its `request_ref`.
async fn approve(
    store: &PgGovernanceStore,
    requester: UserId,
    approver: UserId,
    kind: &str,
    target: Uuid,
) -> Uuid {
    let request_ref = Uuid::new_v4();
    scope_org(OrgId::from_uuid(ORG), async {
        store
            .decide_approval(DecideApprovalCommand {
                approver,
                request_ref,
                kind: kind.to_owned(),
                requested_by: requester,
                target_ref: Some(target),
                decision: ApprovalDecision::Approved,
                trace: TraceContext::generate(),
                occurred_at: time::OffsetDateTime::now_utc(),
            })
            .await
    })
    .await
    .unwrap();
    request_ref
}

async fn consumption_count(owner_pool: &PgPool, request_ref: Uuid) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM gov_approval_consumptions c \
         JOIN gov_approvals a ON a.id = c.approval_id WHERE a.request_ref = $1",
    )
    .bind(request_ref)
    .fetch_one(owner_pool)
    .await
    .unwrap()
}

// Happy path: a matching, unconsumed approval passes and is consumed exactly once.
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn matching_approval_passes_and_is_consumed_once(owner_pool: PgPool) {
    seed_org(&owner_pool).await;
    let requester = seed_user(&owner_pool, "requester").await;
    let approver = seed_user(&owner_pool, "approver").await;
    let actor = seed_user(&owner_pool, "actor").await;
    let store = PgGovernanceStore::new(runtime_role_pool(&owner_pool).await);
    let target = Uuid::new_v4();
    let request_ref = approve(&store, requester, approver, KIND, target).await;

    let passed = scope_org(OrgId::from_uuid(ORG), async {
        store
            .four_eyes_consume(request_ref, KIND, Some(target), actor)
            .await
    })
    .await
    .unwrap();
    assert_eq!(passed, Some(true), "a matching approval must pass the gate");
    assert_eq!(
        consumption_count(&owner_pool, request_ref).await,
        1,
        "the approval must be consumed exactly once"
    );
}

// Wrong kind: an approval decided under a different action kind must not satisfy.
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn wrong_kind_ref_denied(owner_pool: PgPool) {
    seed_org(&owner_pool).await;
    let requester = seed_user(&owner_pool, "requester").await;
    let approver = seed_user(&owner_pool, "approver").await;
    let actor = seed_user(&owner_pool, "actor").await;
    let store = PgGovernanceStore::new(runtime_role_pool(&owner_pool).await);
    let target = Uuid::new_v4();
    // Approved for a DIFFERENT kind ("evidence.hold.release").
    let request_ref = approve(&store, requester, approver, "evidence.hold.release", target).await;

    let denied = scope_org(OrgId::from_uuid(ORG), async {
        store
            .four_eyes_consume(request_ref, KIND, Some(target), actor)
            .await
    })
    .await
    .unwrap();
    assert_eq!(denied, Some(false), "a wrong-kind approval must deny");
    assert_eq!(
        consumption_count(&owner_pool, request_ref).await,
        0,
        "a denied gate must consume nothing"
    );
}

// Wrong target: an approval bound to a different object must not satisfy.
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn wrong_target_ref_denied(owner_pool: PgPool) {
    seed_org(&owner_pool).await;
    let requester = seed_user(&owner_pool, "requester").await;
    let approver = seed_user(&owner_pool, "approver").await;
    let actor = seed_user(&owner_pool, "actor").await;
    let store = PgGovernanceStore::new(runtime_role_pool(&owner_pool).await);
    let approved_target = Uuid::new_v4();
    let request_ref = approve(&store, requester, approver, KIND, approved_target).await;

    // Same kind + ref, but the gate acts on a DIFFERENT object.
    let denied = scope_org(OrgId::from_uuid(ORG), async {
        store
            .four_eyes_consume(request_ref, KIND, Some(Uuid::new_v4()), actor)
            .await
    })
    .await
    .unwrap();
    assert_eq!(denied, Some(false), "a wrong-target approval must deny");
    assert_eq!(consumption_count(&owner_pool, request_ref).await, 0);
}

// An unrelated approved ref (approved for another kind AND target) never satisfies
// a gate — the pre-fix bypass where any approved row in the org sufficed.
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn unrelated_approved_ref_never_satisfies(owner_pool: PgPool) {
    seed_org(&owner_pool).await;
    let requester = seed_user(&owner_pool, "requester").await;
    let approver = seed_user(&owner_pool, "approver").await;
    let actor = seed_user(&owner_pool, "actor").await;
    let store = PgGovernanceStore::new(runtime_role_pool(&owner_pool).await);
    // A perfectly valid approval for a totally unrelated action.
    let unrelated_ref = approve(
        &store,
        requester,
        approver,
        "console_view.deploy",
        Uuid::new_v4(),
    )
    .await;

    let denied = scope_org(OrgId::from_uuid(ORG), async {
        store
            .four_eyes_consume(unrelated_ref, KIND, Some(Uuid::new_v4()), actor)
            .await
    })
    .await
    .unwrap();
    assert_eq!(
        denied,
        Some(false),
        "an unrelated approved ref must never satisfy a different gate"
    );
}

// Replay: the same approval used a second time is denied (single-use).
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn replay_of_same_ref_is_denied(owner_pool: PgPool) {
    seed_org(&owner_pool).await;
    let requester = seed_user(&owner_pool, "requester").await;
    let approver = seed_user(&owner_pool, "approver").await;
    let actor = seed_user(&owner_pool, "actor").await;
    let store = PgGovernanceStore::new(runtime_role_pool(&owner_pool).await);
    let target = Uuid::new_v4();
    let request_ref = approve(&store, requester, approver, KIND, target).await;

    let first = scope_org(OrgId::from_uuid(ORG), async {
        store
            .four_eyes_consume(request_ref, KIND, Some(target), actor)
            .await
    })
    .await
    .unwrap();
    let second = scope_org(OrgId::from_uuid(ORG), async {
        store
            .four_eyes_consume(request_ref, KIND, Some(target), actor)
            .await
    })
    .await
    .unwrap();
    assert_eq!(first, Some(true), "the first use consumes the approval");
    assert_eq!(second, Some(false), "the replay is denied");
    assert_eq!(
        consumption_count(&owner_pool, request_ref).await,
        1,
        "an approval can be consumed at most once"
    );
}

// Concurrent double-consume: two racing consumers of the SAME approval — exactly
// one wins (the `(org_id, approval_id)` unique index serializes them).
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn concurrent_double_consume_admits_exactly_one(owner_pool: PgPool) {
    seed_org(&owner_pool).await;
    let requester = seed_user(&owner_pool, "requester").await;
    let approver = seed_user(&owner_pool, "approver").await;
    let actor = seed_user(&owner_pool, "actor").await;
    let store = PgGovernanceStore::new(runtime_role_pool(&owner_pool).await);
    let target = Uuid::new_v4();
    let request_ref = approve(&store, requester, approver, KIND, target).await;

    let (a, b) = scope_org(OrgId::from_uuid(ORG), async {
        tokio::join!(
            store.four_eyes_consume(request_ref, KIND, Some(target), actor),
            store.four_eyes_consume(request_ref, KIND, Some(target), actor),
        )
    })
    .await;
    let a = a.unwrap();
    let b = b.unwrap();
    assert_eq!(
        [a, b].iter().filter(|r| **r == Some(true)).count(),
        1,
        "exactly one concurrent consumer may win (got {a:?}, {b:?})"
    );
    assert_eq!(
        consumption_count(&owner_pool, request_ref).await,
        1,
        "the race must leave exactly one consumption"
    );
}

// The preview peek reports the gate would pass but NEVER consumes; once the
// approval is actually consumed, the peek then reports it would fail closed.
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn peek_does_not_consume(owner_pool: PgPool) {
    seed_org(&owner_pool).await;
    let requester = seed_user(&owner_pool, "requester").await;
    let approver = seed_user(&owner_pool, "approver").await;
    let actor = seed_user(&owner_pool, "actor").await;
    let store = PgGovernanceStore::new(runtime_role_pool(&owner_pool).await);
    let target = Uuid::new_v4();
    let request_ref = approve(&store, requester, approver, KIND, target).await;

    for _ in 0..2 {
        let peek = scope_org(OrgId::from_uuid(ORG), async {
            store
                .four_eyes_approved(request_ref, KIND, Some(target))
                .await
        })
        .await
        .unwrap();
        assert_eq!(peek, Some(true), "the peek sees an unconsumed approval");
    }
    assert_eq!(
        consumption_count(&owner_pool, request_ref).await,
        0,
        "peeking must never consume"
    );

    scope_org(OrgId::from_uuid(ORG), async {
        store
            .four_eyes_consume(request_ref, KIND, Some(target), actor)
            .await
    })
    .await
    .unwrap();
    let after = scope_org(OrgId::from_uuid(ORG), async {
        store
            .four_eyes_approved(request_ref, KIND, Some(target))
            .await
    })
    .await
    .unwrap();
    assert_eq!(
        after,
        Some(false),
        "a consumed approval no longer peeks as available"
    );
}
