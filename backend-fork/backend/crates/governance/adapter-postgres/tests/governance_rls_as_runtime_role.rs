#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! RUNTIME governance gates, exercised as the genuine non-owner role `console_rt`.
//!
//! Why `console_rt` and not the default `#[sqlx::test]` pool: that pool connects as a
//! BYPASSRLS superuser and would see every tenant's rows regardless of
//! `app.current_org`, green-lighting a totally broken isolation policy. We SEED
//! as the owner and RUN every governance mutation/read as `console_rt` (NOSUPERUSER,
//! NOBYPASSRLS, FORCE RLS) — the only faithful exercise of the tenant policy.
//!
//! Proves:
//!   (a) self-approval is rejected — in the store AND by the DB CHECK;
//!   (b) a four-eyes decision by a distinct principal is appended and is
//!       thereafter immutable (append-only: UPDATE/DELETE rejected);
//!   (c) cross-org override rows are invisible under RLS as `console_rt`;
//!   (d) the §16 gate chain fail-closes: with a required four-eyes gate and NO
//!       approval, the chain denies and nothing is written.
//!   (e) the SHIPPED §16 lifecycle preflight (`lifecycle_preflight`, the body of
//!       `POST /api/v1/governance/lifecycle/preflight`) is TRUE non-mutation:
//!       an unchanged CONTENT digest of EVERY table in every non-system schema —
//!       business, receipt, approval, audit and outbox alike — even on the branch
//!       whose verdict is `allow`. Content and not row count, so an in-place
//!       UPDATE (a "PRECHECKED" state flip on a table `console_rt` may UPDATE) is
//!       caught too;
//!   (f) `decide_pending_approval` refuses to decide without an open PENDING
//!       request, so the account a decision is "distinct from" is always one an
//!       authenticated requester recorded — never one the approver names, and
//!       every authority field of the decision (requester, KIND and target) is
//!       read off that request row rather than off the approver's command;
//!   (g) `lifecycle_preflight` fail-closes an UNCONFIGURED edge — a base-FSM-legal
//!       edge with no requirements row denies even when the client claims
//!       `authority_allow: true`, because an empty requirement set otherwise leaves
//!       every gate NotRequired and the chain allows.

use console_governance_adapter_postgres::{
    LIFECYCLE_FOUR_EYES_KIND, LifecyclePreflight, LifecyclePreflightQuery, PgGovernanceError,
    PgGovernanceStore,
};
use console_governance_application::{
    ApprovalDecision, ConfigureTransitionCommand, CreateApprovalCommand, DecideApprovalCommand,
    DecidePendingApprovalCommand, OpenOverrideCommand,
};
use console_governance_domain::{LifecycleState, TransitionRequirements};
use console_kernel_core::{ErrorKind, OrgId, TraceContext, UserId};
use console_platform_request_context::scope_org;
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use std::collections::BTreeMap;
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

fn trace() -> TraceContext {
    TraceContext::generate()
}

fn now() -> time::OffsetDateTime {
    time::OffsetDateTime::now_utc()
}

// (a) Self-approval rejected in the store, and by the DB CHECK.
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn self_approval_is_rejected(pool: PgPool) {
    seed_org(&pool, ORG_A, "org-alpha").await;
    let requester = seed_user(&pool, ORG_A, "Requester").await;
    let rt = runtime_role_pool(&pool).await;
    let store = PgGovernanceStore::new(rt.clone());
    let request_ref = Uuid::new_v4();

    // Store rejects approver == requester before any write.
    let result = scope_org(OrgId::from_uuid(ORG_A), async {
        store
            .decide_approval(DecideApprovalCommand {
                approver: requester,
                request_ref,
                kind: "override".to_owned(),
                requested_by: requester,
                target_ref: None,
                decision: ApprovalDecision::Approved,
                trace: trace(),
                occurred_at: now(),
            })
            .await
    })
    .await;
    assert!(
        result.is_err(),
        "self-approval must be rejected by the store"
    );

    // Nothing was written.
    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM gov_approvals WHERE request_ref = $1")
            .bind(request_ref)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(count, 0, "rejected self-approval must write no row");

    // The DB CHECK is the backstop: a direct insert with equal ids fails.
    let mut tx = rt.begin().await.unwrap();
    sqlx::query("SELECT set_config('app.current_org', $1, true)")
        .bind(ORG_A.to_string())
        .execute(tx.as_mut())
        .await
        .unwrap();
    let direct = sqlx::query(
        r#"INSERT INTO gov_approvals
             (id, org_id, request_ref, kind, requested_by, approver_id, decision)
           VALUES ($1, $2, $3, 'override', $4, $4, 'approved')"#,
    )
    .bind(Uuid::new_v4())
    .bind(ORG_A)
    .bind(Uuid::new_v4())
    .bind(requester.as_uuid())
    .execute(tx.as_mut())
    .await;
    assert!(
        direct.is_err(),
        "DB CHECK (approver_id <> requested_by) must reject self-approval"
    );
}

// (b) Distinct-approver decision is appended and thereafter immutable.
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn distinct_approval_is_appended_and_immutable(pool: PgPool) {
    seed_org(&pool, ORG_A, "org-alpha").await;
    let requester = seed_user(&pool, ORG_A, "Requester").await;
    let approver = seed_user(&pool, ORG_A, "Approver").await;
    let store = PgGovernanceStore::new(runtime_role_pool(&pool).await);
    let request_ref = Uuid::new_v4();

    let summary = scope_org(OrgId::from_uuid(ORG_A), async {
        store
            .decide_approval(DecideApprovalCommand {
                approver,
                request_ref,
                kind: "override".to_owned(),
                requested_by: requester,
                target_ref: None,
                decision: ApprovalDecision::Approved,
                trace: trace(),
                occurred_at: now(),
            })
            .await
    })
    .await
    .unwrap();
    assert_eq!(summary.decision, ApprovalDecision::Approved);

    // Append-only: UPDATE and DELETE are both rejected by the trigger.
    let update = sqlx::query("UPDATE gov_approvals SET decision = 'rejected' WHERE id = $1")
        .bind(summary.id)
        .execute(&pool)
        .await;
    assert!(update.is_err(), "gov_approvals UPDATE must be rejected");
    let delete = sqlx::query("DELETE FROM gov_approvals WHERE id = $1")
        .bind(summary.id)
        .execute(&pool)
        .await;
    assert!(delete.is_err(), "gov_approvals DELETE must be rejected");
}

// (c) Cross-org override rows are invisible under RLS as console_rt.
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn cross_org_overrides_are_invisible(pool: PgPool) {
    seed_org(&pool, ORG_A, "org-alpha").await;
    seed_org(&pool, ORG_B, "org-bravo").await;
    let actor_a = seed_user(&pool, ORG_A, "A").await;
    let actor_b = seed_user(&pool, ORG_B, "B").await;
    let store = PgGovernanceStore::new(runtime_role_pool(&pool).await);
    let rt = store.pool().clone();

    let override_a = scope_org(OrgId::from_uuid(ORG_A), async {
        store
            .open_override(OpenOverrideCommand {
                actor: actor_a,
                target_type: "ont_instance".to_owned(),
                target_id: Uuid::new_v4(),
                reason: "edit active instance".to_owned(),
                before_snapshot: serde_json::json!({"state": "ACTIVE"}),
                trace: trace(),
                occurred_at: now(),
            })
            .await
    })
    .await
    .unwrap();

    let override_b = scope_org(OrgId::from_uuid(ORG_B), async {
        store
            .open_override(OpenOverrideCommand {
                actor: actor_b,
                target_type: "ont_instance".to_owned(),
                target_id: Uuid::new_v4(),
                reason: "edit active instance".to_owned(),
                before_snapshot: serde_json::json!({"state": "ACTIVE"}),
                trace: trace(),
                occurred_at: now(),
            })
            .await
    })
    .await
    .unwrap();

    // As console_rt under org-A's armed GUC, only A's override is visible.
    let mut tx = rt.begin().await.unwrap();
    sqlx::query("SELECT set_config('app.current_org', $1, true)")
        .bind(ORG_A.to_string())
        .execute(tx.as_mut())
        .await
        .unwrap();
    let visible: Vec<Uuid> = sqlx::query_scalar("SELECT id FROM gov_overrides")
        .fetch_all(tx.as_mut())
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert!(
        visible.contains(&override_a.id),
        "A must see its own override"
    );
    assert!(
        !visible.contains(&override_b.id),
        "A must NOT see org-B's override under RLS"
    );
}

// (d) §16 gate chain fail-closes: required four-eyes gate + no approval ⇒ deny,
//     nothing written; a distinct approval then flips it to allow.
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn gate_chain_fails_closed_without_four_eyes(pool: PgPool) {
    seed_org(&pool, ORG_A, "org-alpha").await;
    let admin = seed_user(&pool, ORG_A, "Admin").await;
    let requester = seed_user(&pool, ORG_A, "Requester").await;
    let approver = seed_user(&pool, ORG_A, "Approver").await;
    let store = PgGovernanceStore::new(runtime_role_pool(&pool).await);
    let object_type_id = Uuid::new_v4();
    let request_ref = Uuid::new_v4();

    // Configure archive->dispose to require four-eyes.
    scope_org(OrgId::from_uuid(ORG_A), async {
        store
            .configure_transition(ConfigureTransitionCommand {
                actor: admin,
                object_type_id,
                from_state: LifecycleState::Archived,
                to_state: LifecycleState::Disposed,
                requirements: TransitionRequirements {
                    requires_reason: true,
                    requires_four_eyes: true,
                    requires_checklist: false,
                },
                trace: trace(),
                occurred_at: now(),
            })
            .await
    })
    .await
    .unwrap();

    // No approval yet ⇒ four-eyes evidence is None ⇒ fail-closed deny.
    let denied = preflight_dispose(&store, object_type_id, request_ref).await;
    assert!(denied.configured, "the edge is configured");
    assert!(
        !denied.outcome.allow,
        "missing four-eyes must deny (fail-closed)"
    );
    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM gov_approvals WHERE request_ref = $1")
            .bind(request_ref)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(count, 0, "a denied gate chain must have written nothing");

    // Record a distinct-principal approval, then the chain allows.
    scope_org(OrgId::from_uuid(ORG_A), async {
        store
            .decide_approval(DecideApprovalCommand {
                approver,
                request_ref,
                kind: LIFECYCLE_FOUR_EYES_KIND.to_owned(),
                requested_by: requester,
                target_ref: Some(object_type_id),
                decision: ApprovalDecision::Approved,
                trace: trace(),
                occurred_at: now(),
            })
            .await
    })
    .await
    .unwrap();

    let allowed = preflight_dispose(&store, object_type_id, request_ref).await;
    assert!(
        allowed.outcome.allow,
        "distinct four-eyes approval must allow"
    );
}

/// Run the REAL archive->dispose preflight under org-A: the same
/// `PgGovernanceStore::lifecycle_preflight` that
/// `POST /api/v1/governance/lifecycle/preflight` calls once it has authorized the
/// caller, NOT a re-assembly of the gate chain in the test. A test that rebuilds
/// the chain itself asserts against its own copy and stays green while the
/// shipped path writes rows.
async fn preflight_dispose(
    store: &PgGovernanceStore,
    object_type_id: Uuid,
    request_ref: Uuid,
) -> LifecyclePreflight {
    scope_org(OrgId::from_uuid(ORG_A), async {
        store
            .lifecycle_preflight(LifecyclePreflightQuery {
                object_type_id,
                from_state: LifecycleState::Archived,
                to_state: LifecycleState::Disposed,
                authority_allow: Some(true),
                checklist_all_acknowledged: None,
                four_eyes_request_ref: Some(request_ref),
                egress_cleared: None,
            })
            .await
    })
    .await
    .unwrap()
}

/// Full CONTENT digest — `"<row count>:<md5 of every row>"` — for EVERY base table
/// in every non-system schema, read as the OWNER (BYPASSRLS) so nothing — no audit
/// row, no other tenant's row, no outbox row — can hide behind a tenant policy.
///
/// Content, not row count: a row count is blind to an UPDATE, and `console_rt`
/// holds `GRANT ... UPDATE ON gov_lifecycle_transitions` (0153) on a table with no
/// append-only trigger, so "preflight flips a row to PRECHECKED" is a mutation the
/// runtime role is actually permitted to make. Digesting the rows catches it.
///
/// Schemas and tables are enumerated from the catalog rather than a hand-written
/// list, and the schema filter is a system-schema EXCLUSION rather than a
/// `= 'public'` inclusion, so a table added later — in any schema — is watched
/// automatically.
async fn content_digest_of_every_table(owner_pool: &PgPool) -> BTreeMap<String, String> {
    // `query_to_xml` digests each catalog-listed table without building a dynamic
    // SQL string in Rust (`%I` quotes the identifier server-side). `r::text` is the
    // whole row, so every column is covered.
    sqlx::query_as::<_, (String, String)>(
        r#"
        SELECT format('%I.%I', t.table_schema, t.table_name)::text AS qualified_name,
               (xpath(
                   '/row/d/text()',
                   query_to_xml(
                       format(
                           'SELECT count(*)::text || '':'' || coalesce(md5(string_agg(r::text, '''' ORDER BY r::text)), ''empty'') AS d FROM %I.%I AS r',
                           t.table_schema, t.table_name
                       ),
                       false, true, ''
                   )
               ))[1]::text AS digest
        FROM information_schema.tables t
        WHERE t.table_schema NOT IN ('pg_catalog', 'information_schema')
          AND t.table_type = 'BASE TABLE'
        "#,
    )
    .fetch_all(owner_pool)
    .await
    .unwrap()
    .into_iter()
    .collect()
}

fn changed_tables(
    before: &BTreeMap<String, String>,
    after: &BTreeMap<String, String>,
) -> Vec<(String, String, String)> {
    let mut changed: Vec<(String, String, String)> = before
        .iter()
        .filter_map(|(table, was)| {
            let now = after
                .get(table)
                .cloned()
                .unwrap_or_else(|| "absent".to_owned());
            (&now != was).then(|| (table.clone(), was.clone(), now))
        })
        .collect();
    for (table, now) in after {
        if !before.contains_key(table) {
            changed.push((table.clone(), "absent".to_owned(), now.clone()));
        }
    }
    changed
}

// (e) TRUE preflight non-mutation of the SHIPPED preflight — `preflight_dispose`
//     calls `PgGovernanceStore::lifecycle_preflight`, the entire body of
//     `POST /api/v1/governance/lifecycle/preflight` below authorization. It must
//     persist NOTHING: no PRECHECKED state, no receipt, no approval, no
//     consumption, no audit row, no outbox row. Asserted as a zero row delta over
//     every table in the database, on the `allow` branch (the one a "helpfully"
//     caching implementation would want to write a receipt for).
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn preflight_writes_no_row_in_any_table(pool: PgPool) {
    seed_org(&pool, ORG_A, "org-alpha").await;
    let admin = seed_user(&pool, ORG_A, "Admin").await;
    let requester = seed_user(&pool, ORG_A, "Requester").await;
    let approver = seed_user(&pool, ORG_A, "Approver").await;
    let store = PgGovernanceStore::new(runtime_role_pool(&pool).await);
    let object_type_id = Uuid::new_v4();
    let request_ref = Uuid::new_v4();

    // Configure archive->dispose with a required four-eyes gate, and satisfy it,
    // so preflight evaluates its FULL path and reaches `allow`.
    scope_org(OrgId::from_uuid(ORG_A), async {
        store
            .configure_transition(ConfigureTransitionCommand {
                actor: admin,
                object_type_id,
                from_state: LifecycleState::Archived,
                to_state: LifecycleState::Disposed,
                requirements: TransitionRequirements {
                    requires_reason: true,
                    requires_four_eyes: true,
                    requires_checklist: false,
                },
                trace: trace(),
                occurred_at: now(),
            })
            .await
    })
    .await
    .unwrap();
    scope_org(OrgId::from_uuid(ORG_A), async {
        store
            .decide_approval(DecideApprovalCommand {
                approver,
                request_ref,
                kind: LIFECYCLE_FOUR_EYES_KIND.to_owned(),
                requested_by: requester,
                target_ref: Some(object_type_id),
                decision: ApprovalDecision::Approved,
                trace: trace(),
                occurred_at: now(),
            })
            .await
    })
    .await
    .unwrap();

    let before = content_digest_of_every_table(&pool).await;
    // The snapshot is not vacuous: the tables preflight would be tempted to write
    // are present and already populated.
    for table in [
        "public.gov_approvals",
        "public.gov_lifecycle_transitions",
        "public.audit_events",
        "public.gov_approval_consumptions",
        "public.workflow_outbox_events",
    ] {
        assert!(
            before.contains_key(table),
            "{table} must be watched by the snapshot"
        );
    }
    for table in [
        "public.gov_approvals",
        "public.gov_lifecycle_transitions",
        "public.audit_events",
    ] {
        assert!(
            !before[table].starts_with("0:"),
            "{table} must already hold rows for the digest to be able to change, got {:?}",
            before[table]
        );
    }

    // Two full preflights — the second also proves the peek did not consume the
    // evidence the first one saw.
    for pass in 1..=2 {
        let preflight = preflight_dispose(&store, object_type_id, request_ref).await;
        assert!(
            preflight.configured && preflight.outcome.allow,
            "preflight pass {pass} must reach the allow branch, got {preflight:?}"
        );
    }

    let after = content_digest_of_every_table(&pool).await;
    let changed = changed_tables(&before, &after);
    assert!(
        changed.is_empty(),
        "preflight must persist NOTHING — no inserted, updated or deleted row; \
         these tables changed (table, before digest, after digest): {changed:?}"
    );
}

// (g) An UNCONFIGURED edge is fail-closed in the SHIPPED preflight. The
//     unconfigured branch synthesises an EMPTY requirement set, so every gate
//     reads NotRequired and the chain would otherwise allow an edge no admin ever
//     enabled; the store must force an authority DENY instead. Driven through
//     `lifecycle_preflight` with the most favourable client input the handler can
//     pass (`authority_allow: Some(true)`).
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn preflight_denies_an_unconfigured_edge(pool: PgPool) {
    seed_org(&pool, ORG_A, "org-alpha").await;
    let admin = seed_user(&pool, ORG_A, "Admin").await;
    let store = PgGovernanceStore::new(runtime_role_pool(&pool).await);
    let object_type_id = Uuid::new_v4();
    let request_ref = Uuid::new_v4();

    // archive->dispose is legal in the base FSM but configured for NO object type.
    let unconfigured = preflight_dispose(&store, object_type_id, request_ref).await;
    assert!(
        !unconfigured.configured,
        "the edge must report not-configured, got {unconfigured:?}"
    );
    assert!(
        !unconfigured.outcome.allow,
        "an unconfigured edge must deny even with authority_allow=true, got {unconfigured:?}"
    );

    // Non-vacuity: the SAME query on the SAME edge allows once an admin configures
    // it with no gates, so the deny above came from the unconfigured branch and not
    // from the base FSM or some unrelated gate.
    scope_org(OrgId::from_uuid(ORG_A), async {
        store
            .configure_transition(ConfigureTransitionCommand {
                actor: admin,
                object_type_id,
                from_state: LifecycleState::Archived,
                to_state: LifecycleState::Disposed,
                requirements: TransitionRequirements {
                    requires_reason: false,
                    requires_four_eyes: false,
                    requires_checklist: false,
                },
                trace: trace(),
                occurred_at: now(),
            })
            .await
    })
    .await
    .unwrap();

    let configured = preflight_dispose(&store, object_type_id, request_ref).await;
    assert!(
        configured.configured && configured.outcome.allow,
        "the configured edge must allow, got {configured:?}"
    );
}

// (f) A decision requires an OPEN PENDING request. Without one, an org admin
//     could name any other user as the requester and manufacture a four-eyes
//     approval single-handedly — the exact bypass four-eyes exists to prevent.
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn decide_pending_approval_requires_an_open_request(pool: PgPool) {
    seed_org(&pool, ORG_A, "org-alpha").await;
    let _requester = seed_user(&pool, ORG_A, "Requester").await;
    let approver = seed_user(&pool, ORG_A, "Approver").await;
    let store = PgGovernanceStore::new(runtime_role_pool(&pool).await);
    let request_ref = Uuid::new_v4();

    let result = scope_org(OrgId::from_uuid(ORG_A), async {
        store
            .decide_pending_approval(DecidePendingApprovalCommand {
                approver,
                request_ref,
                kind: "override".to_owned(),
                decision: ApprovalDecision::Approved,
                trace: trace(),
                occurred_at: now(),
            })
            .await
    })
    .await;
    // Pinned to the guard itself, not merely to "some error": the command's unused
    // `requested_by` placeholder would trip the self-approval bar anyway, so a
    // bare `is_err()` would still pass with the guard deleted.
    match result {
        Err(PgGovernanceError::Domain(error)) => {
            assert_eq!(error.kind, ErrorKind::Conflict, "got {error:?}");
            assert!(
                error.message.contains("pending approval request"),
                "the refusal must name the missing pending request, got {:?}",
                error.message
            );
        }
        other => panic!("a decision with no open pending request must be refused, got {other:?}"),
    }

    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM gov_approvals WHERE request_ref = $1")
            .bind(request_ref)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(count, 0, "a refused decision must write no approval row");
}

// (f) The requester the decision is distinct FROM comes from the request row, so
//     the requester deciding their own request is denied — with no `requested_by`
//     field on the command there is nothing for the approver to spoof.
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn decide_pending_approval_denies_the_recorded_requester(pool: PgPool) {
    seed_org(&pool, ORG_A, "org-alpha").await;
    let requester = seed_user(&pool, ORG_A, "Requester").await;
    let store = PgGovernanceStore::new(runtime_role_pool(&pool).await);
    let request_ref = Uuid::new_v4();

    scope_org(OrgId::from_uuid(ORG_A), async {
        store
            .create_approval(CreateApprovalCommand {
                requester,
                request_ref,
                kind: "override".to_owned(),
                target_ref: None,
                payload_summary: serde_json::json!({"why": "post-draft edit"}),
                trace: trace(),
                occurred_at: now(),
            })
            .await
    })
    .await
    .unwrap();

    let result = scope_org(OrgId::from_uuid(ORG_A), async {
        store
            .decide_pending_approval(DecidePendingApprovalCommand {
                approver: requester,
                request_ref,
                kind: "override".to_owned(),
                decision: ApprovalDecision::Approved,
                trace: trace(),
                occurred_at: now(),
            })
            .await
    })
    .await;
    // Pinned to the self-approval bar itself: a bare `is_err()` would also pass on
    // an FK violation or a kind mismatch.
    match &result {
        Err(PgGovernanceError::Domain(error)) => {
            assert_eq!(error.kind, ErrorKind::Forbidden, "got {error:?}");
            assert!(
                error.message.contains("self-approval"),
                "the refusal must name self-approval, got {:?}",
                error.message
            );
        }
        other => panic!("the recorded requester must not decide their own request, got {other:?}"),
    }

    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM gov_approvals WHERE request_ref = $1")
            .bind(request_ref)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        count, 0,
        "a refused self-decision must write no approval row"
    );
}

// (f) Happy path: the open request supplies BOTH the requester and the binding
//     target; a distinct approver's decision records them.
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn decide_pending_approval_sources_requester_and_target_from_the_request(pool: PgPool) {
    seed_org(&pool, ORG_A, "org-alpha").await;
    let requester = seed_user(&pool, ORG_A, "Requester").await;
    let approver = seed_user(&pool, ORG_A, "Approver").await;
    let store = PgGovernanceStore::new(runtime_role_pool(&pool).await);
    let request_ref = Uuid::new_v4();
    let bound_target = Uuid::new_v4();

    scope_org(OrgId::from_uuid(ORG_A), async {
        store
            .create_approval(CreateApprovalCommand {
                requester,
                request_ref,
                kind: "override".to_owned(),
                target_ref: Some(bound_target),
                payload_summary: serde_json::json!({"why": "post-draft edit"}),
                trace: trace(),
                occurred_at: now(),
            })
            .await
    })
    .await
    .unwrap();

    let decision = scope_org(OrgId::from_uuid(ORG_A), async {
        store
            .decide_pending_approval(DecidePendingApprovalCommand {
                approver,
                request_ref,
                kind: "override".to_owned(),
                decision: ApprovalDecision::Approved,
                trace: trace(),
                occurred_at: now(),
            })
            .await
    })
    .await
    .unwrap();
    assert_eq!(decision.decision, ApprovalDecision::Approved);
    assert_eq!(
        decision.requested_by, requester,
        "the requester is the one the open request recorded"
    );
    assert_eq!(decision.approver_id, approver);

    let recorded_target: Option<Uuid> =
        sqlx::query_scalar("SELECT target_ref FROM gov_approvals WHERE request_ref = $1")
            .bind(request_ref)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        recorded_target,
        Some(bound_target),
        "the binding target is sourced from the open request"
    );
    let recorded_kind: String =
        sqlx::query_scalar("SELECT kind FROM gov_approvals WHERE request_ref = $1")
            .bind(request_ref)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        recorded_kind, "override",
        "the bound kind is sourced from the open request"
    );
}

/// The §16 gate binds on all three of `(request_ref, kind, target_ref)`
/// (`0164_bind_consume_four_eyes.sql`). `requested_by` and `target_ref` are taken
/// off the pending request row; `kind` is the third authority field and must be
/// too. Otherwise an approver decides a request opened for a low-privilege
/// purpose while naming a different, higher-privilege `kind`, and the row that
/// lands satisfies a gate nobody ever requested — on the request's own target.
async fn open_request_of_kind(
    store: &PgGovernanceStore,
    requester: UserId,
    request_ref: Uuid,
    kind: &str,
    target_ref: Option<Uuid>,
) {
    scope_org(OrgId::from_uuid(ORG_A), async {
        store
            .create_approval(CreateApprovalCommand {
                requester,
                request_ref,
                kind: kind.to_owned(),
                target_ref,
                payload_summary: serde_json::json!({"why": "post-draft edit"}),
                trace: trace(),
                occurred_at: now(),
            })
            .await
    })
    .await
    .unwrap();
}

/// The refusal must be the kind-binding guard itself. A bare `is_err()` would
/// also pass on the self-approval bar or an FK violation, i.e. with the guard
/// deleted.
fn assert_kind_binding_refusal(
    result: &Result<console_governance_application::ApprovalSummary, PgGovernanceError>,
) {
    match result {
        Err(PgGovernanceError::Domain(error)) => {
            assert_eq!(error.kind, ErrorKind::Conflict, "got {error:?}");
            assert!(
                error.message.contains("kind"),
                "the refusal must name the kind mismatch, got {:?}",
                error.message
            );
        }
        other => panic!("a decision naming a foreign kind must be refused, got {other:?}"),
    }
}

async fn approvals_of_kind(pool: &PgPool, request_ref: Uuid, kind: &str) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM gov_approvals WHERE request_ref = $1 AND kind = $2")
        .bind(request_ref)
        .bind(kind)
        .fetch_one(pool)
        .await
        .unwrap()
}

// (f) Privilege escalation across gates: a request opened for one kind must not
//     mint an approval for another.
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn decide_pending_approval_refuses_a_kind_the_request_did_not_open(pool: PgPool) {
    seed_org(&pool, ORG_A, "org-alpha").await;
    let requester = seed_user(&pool, ORG_A, "Requester").await;
    let approver = seed_user(&pool, ORG_A, "Approver").await;
    let store = PgGovernanceStore::new(runtime_role_pool(&pool).await);
    let request_ref = Uuid::new_v4();
    let target = Uuid::new_v4();

    open_request_of_kind(
        &store,
        requester,
        request_ref,
        "console_view.team_deploy",
        Some(target),
    )
    .await;

    let result = scope_org(OrgId::from_uuid(ORG_A), async {
        store
            .decide_pending_approval(DecidePendingApprovalCommand {
                approver,
                request_ref,
                // Never asked for. The §16 gate for this kind is a strictly more
                // privileged one than the deploy the requester opened.
                kind: "ontology.schema.publish".to_owned(),
                decision: ApprovalDecision::Approved,
                trace: trace(),
                occurred_at: now(),
            })
            .await
    })
    .await;

    assert_kind_binding_refusal(&result);
    assert_eq!(
        approvals_of_kind(&pool, request_ref, "ontology.schema.publish").await,
        0,
        "no approval may exist for the kind nobody requested"
    );
    assert_eq!(
        approvals_of_kind(&pool, request_ref, "console_view.team_deploy").await,
        0,
        "and the refused decision must not silently satisfy the requested gate either"
    );
}

// (f) The same guard, reached through the DEPRECATED compat contract. The
//     escalation is a property of the pending row, not of one entry point, so the
//     check lives in the shared `record_decision` and both callers get it.
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn deprecated_decide_approval_refuses_a_kind_the_request_did_not_open(pool: PgPool) {
    seed_org(&pool, ORG_A, "org-alpha").await;
    let requester = seed_user(&pool, ORG_A, "Requester").await;
    let approver = seed_user(&pool, ORG_A, "Approver").await;
    let store = PgGovernanceStore::new(runtime_role_pool(&pool).await);
    let request_ref = Uuid::new_v4();
    let target = Uuid::new_v4();

    open_request_of_kind(
        &store,
        requester,
        request_ref,
        "console_view.team_deploy",
        Some(target),
    )
    .await;

    let result = scope_org(OrgId::from_uuid(ORG_A), async {
        store
            .decide_approval(DecideApprovalCommand {
                approver,
                request_ref,
                kind: "ontology.schema.publish".to_owned(),
                requested_by: requester,
                target_ref: Some(target),
                decision: ApprovalDecision::Approved,
                trace: trace(),
                occurred_at: now(),
            })
            .await
    })
    .await;

    assert_kind_binding_refusal(&result);
    assert_eq!(
        approvals_of_kind(&pool, request_ref, "ontology.schema.publish").await,
        0,
        "no approval may exist for the kind nobody requested"
    );
}

// (f) The pending row is authority for the target even when that target is NULL —
//     the state 0164 explicitly supports for create-style actions. A fallback to
//     the approver's own `target_ref` there would bind the approval to an object
//     the requester never named, satisfying the §16
//     `(request_ref, kind, target_ref)` gate for it.
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn a_decision_cannot_name_a_target_the_open_request_left_null(pool: PgPool) {
    seed_org(&pool, ORG_A, "org-alpha").await;
    let requester = seed_user(&pool, ORG_A, "Requester").await;
    let approver = seed_user(&pool, ORG_A, "Approver").await;
    let store = PgGovernanceStore::new(runtime_role_pool(&pool).await);
    let request_ref = Uuid::new_v4();
    let victim = Uuid::new_v4();

    open_request_of_kind(
        &store,
        requester,
        request_ref,
        "ontology.schema.publish",
        None,
    )
    .await;

    scope_org(OrgId::from_uuid(ORG_A), async {
        store
            .decide_approval(DecideApprovalCommand {
                approver,
                request_ref,
                kind: "ontology.schema.publish".to_owned(),
                requested_by: requester,
                // Never requested: the open row's NULL target is the authority.
                target_ref: Some(victim),
                decision: ApprovalDecision::Approved,
                trace: trace(),
                occurred_at: now(),
            })
            .await
    })
    .await
    .unwrap();

    let recorded: Option<Uuid> =
        sqlx::query_scalar("SELECT target_ref FROM gov_approvals WHERE request_ref = $1")
            .bind(request_ref)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        recorded, None,
        "an open request's NULL target is authority; the approver's own target_ref \
         must not fill it in and bind the approval to an unrequested object"
    );
}
