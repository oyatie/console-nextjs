#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! RUNTIME proofs for approvals-CREATE, exercised as the genuine non-owner
//! `console_rt` role (NOSUPERUSER, NOBYPASSRLS, FORCE RLS) — the only faithful
//! exercise of RLS org-isolation.
//!
//! Proves:
//!   (a) create a pending request (requester recorded), then a DISTINCT approver
//!       decides it → approved; the decision's requester is the one the request
//!       recorded (authoritative), not whatever the decide caller supplies;
//!   (b) self-decide (approver == the request's recorded requester) is rejected,
//!       even when the decide command lies about `requested_by`, and writes no
//!       decision row;
//!   (c) the pending request row is append-only (UPDATE/DELETE rejected);
//!   (d) a cross-org request is invisible under another tenant's GUC (RLS);
//!   (e) console-dgo.1: Company/HR/Payroll kinds deny same-Person different
//!       accounts (would pass a mere user_id inequality), fail closed for
//!       unbound requesters, and leave the generic four-eyes path alone for
//!       NULL `users.employee_id` accounts.

use console_governance_adapter_postgres::PgGovernanceStore;
use console_governance_application::{
    ApprovalDecision, CreateApprovalCommand, DecideApprovalCommand, DecidePendingApprovalCommand,
};
use console_kernel_core::{ErrorKind, OrgId, TraceContext, UserId};
use console_platform_request_context::scope_org;
use serde_json::json;
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use uuid::Uuid;

const ORG_A: Uuid = Uuid::from_u128(0x1111_1111_1111_1111_1111_1111_1111_1111);
const ORG_B: Uuid = Uuid::from_u128(0x2222_2222_2222_2222_2222_2222_2222_2222);

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

async fn seed_org(pool: &PgPool, org: Uuid, tag: &str) {
    sqlx::query("INSERT INTO organizations (id, slug, name) VALUES ($1, $2, $3) ON CONFLICT (id) DO NOTHING")
        .bind(org)
        .bind(format!("org-{tag}"))
        .bind(format!("Org {tag}"))
        .execute(pool)
        .await
        .unwrap();
}

async fn seed_user(pool: &PgPool, org: Uuid, name: &str) -> UserId {
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO users (id, display_name, roles, org_id) VALUES ($1, $2, $3, $4)")
        .bind(id)
        .bind(name)
        .bind(["SUPER_ADMIN"].as_slice())
        .bind(org)
        .execute(pool)
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

// (a) create pending → distinct approver decides → approved; requester is the
// one the request recorded, even though decide is told a different requester.
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn create_then_decide_by_distinct_approver(pool: PgPool) {
    seed_org(&pool, ORG_A, "alpha").await;
    let requester = seed_user(&pool, ORG_A, "Requester").await;
    let approver = seed_user(&pool, ORG_A, "Approver").await;
    let store = PgGovernanceStore::new(runtime_role_pool(&pool).await);
    let request_ref = Uuid::new_v4();
    let bound_target = Uuid::new_v4();

    let request = scope_org(OrgId::from_uuid(ORG_A), async {
        store
            .create_approval(CreateApprovalCommand {
                requester,
                request_ref,
                kind: "console_view.team_deploy".to_owned(),
                target_ref: Some(bound_target),
                payload_summary: json!({"screen_key": "ops.dashboard", "scope": "team"}),
                trace: trace(),
                occurred_at: now(),
            })
            .await
    })
    .await
    .unwrap();
    assert_eq!(request.requested_by, requester);
    assert_eq!(request.request_ref, request_ref);

    // Decide by the distinct approver. Note the command LIES about requested_by
    // (claims the approver) AND supplies no target — the store must ignore the
    // spoofed requester and source BOTH the requester and the binding target from
    // the pending request row.
    let decision = scope_org(OrgId::from_uuid(ORG_A), async {
        store
            .decide_approval(DecideApprovalCommand {
                approver,
                request_ref,
                kind: "console_view.team_deploy".to_owned(),
                requested_by: approver, // spoofed; must be ignored
                target_ref: None,       // sourced authoritatively from the request
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
        "authoritative requester comes from the pending request, not the client"
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
        "the binding target is sourced from the pending request, not the decide body"
    );
}

// (b) self-decide is rejected: the approver IS the request's recorded requester,
// even though the decide command claims someone else asked. Zero decision rows.
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn self_decide_is_rejected_against_recorded_requester(pool: PgPool) {
    seed_org(&pool, ORG_A, "alpha").await;
    let requester = seed_user(&pool, ORG_A, "Requester").await;
    let other = seed_user(&pool, ORG_A, "Other").await;
    let store = PgGovernanceStore::new(runtime_role_pool(&pool).await);
    let request_ref = Uuid::new_v4();

    scope_org(OrgId::from_uuid(ORG_A), async {
        store
            .create_approval(CreateApprovalCommand {
                requester,
                request_ref,
                kind: "override".to_owned(),
                target_ref: None,
                payload_summary: json!({}),
                trace: trace(),
                occurred_at: now(),
            })
            .await
    })
    .await
    .unwrap();

    let result = scope_org(OrgId::from_uuid(ORG_A), async {
        store
            .decide_approval(DecideApprovalCommand {
                approver: requester, // == the recorded requester
                request_ref,
                kind: "override".to_owned(),
                requested_by: other, // lie: claim someone else asked
                target_ref: None,
                decision: ApprovalDecision::Approved,
                trace: trace(),
                occurred_at: now(),
            })
            .await
    })
    .await;
    assert!(result.is_err(), "self-decide must be rejected");

    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM gov_approvals WHERE request_ref = $1")
            .bind(request_ref)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(count, 0, "a rejected self-decide writes no decision row");
}

// (c) the pending request row is append-only.
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn pending_request_is_append_only(pool: PgPool) {
    seed_org(&pool, ORG_A, "alpha").await;
    let requester = seed_user(&pool, ORG_A, "Requester").await;
    let store = PgGovernanceStore::new(runtime_role_pool(&pool).await);

    let request = scope_org(OrgId::from_uuid(ORG_A), async {
        store
            .create_approval(CreateApprovalCommand {
                requester,
                request_ref: Uuid::new_v4(),
                kind: "override".to_owned(),
                target_ref: None,
                payload_summary: json!({"note": "x"}),
                trace: trace(),
                occurred_at: now(),
            })
            .await
    })
    .await
    .unwrap();

    let update = sqlx::query("UPDATE gov_approval_requests SET kind = 'y' WHERE id = $1")
        .bind(request.id)
        .execute(&pool)
        .await;
    assert!(
        update.is_err(),
        "gov_approval_requests UPDATE must be rejected"
    );
    let delete = sqlx::query("DELETE FROM gov_approval_requests WHERE id = $1")
        .bind(request.id)
        .execute(&pool)
        .await;
    assert!(
        delete.is_err(),
        "gov_approval_requests DELETE must be rejected"
    );
}

// (d) cross-org pending requests are invisible under another tenant's GUC.
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn cross_org_requests_are_invisible(pool: PgPool) {
    seed_org(&pool, ORG_A, "alpha").await;
    seed_org(&pool, ORG_B, "bravo").await;
    let requester_a = seed_user(&pool, ORG_A, "A").await;
    let store = PgGovernanceStore::new(runtime_role_pool(&pool).await);
    let request_ref = Uuid::new_v4();

    scope_org(OrgId::from_uuid(ORG_A), async {
        store
            .create_approval(CreateApprovalCommand {
                requester: requester_a,
                request_ref,
                kind: "override".to_owned(),
                target_ref: None,
                payload_summary: json!({}),
                trace: trace(),
                occurred_at: now(),
            })
            .await
    })
    .await
    .unwrap();

    // Under org-B's GUC, org-A's request does not exist → a decide finds no
    // pending request and falls back to org-B's client-supplied requester. Both
    // decide parties are org-B users (the gov_approvals FK is (id, org_id)). Had
    // org-A's request been visible, the store would have overridden requested_by
    // with the org-A requester and the org-B FK insert would fail — a successful
    // decide recording requester_b is the invisibility proof.
    let requester_b = seed_user(&pool, ORG_B, "B-req").await;
    let approver_b = seed_user(&pool, ORG_B, "B-app").await;
    let _ = requester_a;
    let decision = scope_org(OrgId::from_uuid(ORG_B), async {
        store
            .decide_approval(DecideApprovalCommand {
                approver: approver_b,
                request_ref,
                kind: "override".to_owned(),
                requested_by: requester_b,
                target_ref: None,
                decision: ApprovalDecision::Approved,
                trace: trace(),
                occurred_at: now(),
            })
            .await
    })
    .await
    .unwrap();
    assert_eq!(decision.requested_by, requester_b);
}

// ---------------------------------------------------------------------------
// console-dgo.1 — distinct-natural-person four-eyes (Company/HR/Payroll only)
// ---------------------------------------------------------------------------

const NATURAL_PERSON_KIND: &str = "company.revise";

async fn seed_employee(pool: &PgPool, org: Uuid, source_key: &str) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO employees \
         (org_id, company, name, source_filename, source_sheet, source_row, source_key) \
         VALUES ($1, 'ACME', $2, 'seed.xlsx', 'Sheet1', 1, $2) RETURNING id",
    )
    .bind(org)
    .bind(source_key)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn seed_person_bound_user(
    pool: &PgPool,
    org: Uuid,
    name: &str,
    employee_id: Uuid,
    person_id: Uuid,
    binder: UserId,
) -> UserId {
    let user = seed_user(pool, org, name).await;
    sqlx::query("UPDATE users SET employee_id = $1 WHERE id = $2 AND org_id = $3")
        .bind(employee_id)
        .bind(*user.as_uuid())
        .bind(org)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO employee_person_bindings \
         (org_id, employee_id, person_id, actor_id, payload_digest) \
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(org)
    .bind(employee_id)
    .bind(person_id)
    .bind(*binder.as_uuid())
    .bind([7_u8; 32].as_slice())
    .execute(pool)
    .await
    .unwrap();
    user
}

async fn seed_person(pool: &PgPool, org: Uuid) -> Uuid {
    sqlx::query_scalar("INSERT INTO persons (org_id) VALUES ($1) RETURNING id")
        .bind(org)
        .fetch_one(pool)
        .await
        .unwrap()
}

/// Two DISTINCT accounts bound to the SAME Person must be denied on
/// Company/HR/Payroll kinds. A mere `user_id != user_id` check would ALLOW this
/// fixture — that is the whole point of the bar.
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn same_person_different_accounts_denied_on_company_kind(pool: PgPool) {
    seed_org(&pool, ORG_A, "np-same").await;
    let binder = seed_user(&pool, ORG_A, "Binder").await;
    let person = seed_person(&pool, ORG_A).await;
    let emp_a = seed_employee(&pool, ORG_A, "np-same-a").await;
    let emp_b = seed_employee(&pool, ORG_A, "np-same-b").await;
    let requester = seed_person_bound_user(&pool, ORG_A, "Cap-A", emp_a, person, binder).await;
    let approver = seed_person_bound_user(&pool, ORG_A, "Cap-B", emp_b, person, binder).await;
    assert_ne!(
        requester, approver,
        "fixture must use two accounts; otherwise the account-level bar masks the defect"
    );

    let store = PgGovernanceStore::new(runtime_role_pool(&pool).await);
    let request_ref = Uuid::new_v4();

    scope_org(OrgId::from_uuid(ORG_A), async {
        store
            .create_approval(CreateApprovalCommand {
                requester,
                request_ref,
                kind: NATURAL_PERSON_KIND.to_owned(),
                target_ref: Some(Uuid::new_v4()),
                payload_summary: json!({"case": "same-person"}),
                trace: trace(),
                occurred_at: now(),
            })
            .await
    })
    .await
    .expect("bound requester may open a Company/HR/Payroll request");

    let refused = scope_org(OrgId::from_uuid(ORG_A), async {
        store
            .decide_pending_approval(DecidePendingApprovalCommand {
                approver,
                request_ref,
                kind: NATURAL_PERSON_KIND.to_owned(),
                decision: ApprovalDecision::Approved,
                trace: trace(),
                occurred_at: now(),
            })
            .await
    })
    .await
    .expect_err("same Person under two capacities must be denied");

    match refused {
        console_governance_adapter_postgres::PgGovernanceError::Domain(err) => {
            assert_eq!(err.kind, ErrorKind::Forbidden);
            assert!(
                err.message.contains("same Person"),
                "refusal must name the Person bar, got {:?}",
                err.message
            );
        }
        other => panic!("expected Domain forbidden, got {other:?}"),
    }

    let decisions: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM gov_approvals WHERE request_ref = $1")
            .bind(request_ref)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(decisions, 0, "denied same-Person decide must write no row");
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn distinct_persons_may_decide_company_kind(pool: PgPool) {
    seed_org(&pool, ORG_A, "np-distinct").await;
    let binder = seed_user(&pool, ORG_A, "Binder").await;
    let person_a = seed_person(&pool, ORG_A).await;
    let person_b = seed_person(&pool, ORG_A).await;
    let emp_a = seed_employee(&pool, ORG_A, "np-dist-a").await;
    let emp_b = seed_employee(&pool, ORG_A, "np-dist-b").await;
    let requester = seed_person_bound_user(&pool, ORG_A, "Human-A", emp_a, person_a, binder).await;
    let approver = seed_person_bound_user(&pool, ORG_A, "Human-B", emp_b, person_b, binder).await;

    let store = PgGovernanceStore::new(runtime_role_pool(&pool).await);
    let request_ref = Uuid::new_v4();

    scope_org(OrgId::from_uuid(ORG_A), async {
        store
            .create_approval(CreateApprovalCommand {
                requester,
                request_ref,
                kind: NATURAL_PERSON_KIND.to_owned(),
                target_ref: None,
                payload_summary: json!({"case": "distinct-person"}),
                trace: trace(),
                occurred_at: now(),
            })
            .await
            .unwrap();
        store
            .decide_pending_approval(DecidePendingApprovalCommand {
                approver,
                request_ref,
                kind: NATURAL_PERSON_KIND.to_owned(),
                decision: ApprovalDecision::Approved,
                trace: trace(),
                occurred_at: now(),
            })
            .await
            .unwrap()
    })
    .await;
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn unbound_requester_fails_closed_on_company_kind_open(pool: PgPool) {
    seed_org(&pool, ORG_A, "np-unbound").await;
    let unbound = seed_user(&pool, ORG_A, "Unbound").await;
    let store = PgGovernanceStore::new(runtime_role_pool(&pool).await);

    let refused = scope_org(OrgId::from_uuid(ORG_A), async {
        store
            .create_approval(CreateApprovalCommand {
                requester: unbound,
                request_ref: Uuid::new_v4(),
                kind: NATURAL_PERSON_KIND.to_owned(),
                target_ref: None,
                payload_summary: json!({}),
                trace: trace(),
                occurred_at: now(),
            })
            .await
    })
    .await
    .expect_err("unbound requester must fail closed on Company/HR/Payroll open");

    match refused {
        console_governance_adapter_postgres::PgGovernanceError::Domain(err) => {
            assert_eq!(err.kind, ErrorKind::Forbidden);
            assert!(
                err.message.contains("unbound") || err.message.contains("natural person"),
                "got {:?}",
                err.message
            );
        }
        other => panic!("expected Domain forbidden, got {other:?}"),
    }
}

/// Regression pin: the generic four-eyes path must still accept NULL
/// `users.employee_id` accounts — the failure mode of the P2 retrofit.
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn generic_kind_still_allows_null_employee_id_accounts(pool: PgPool) {
    seed_org(&pool, ORG_A, "np-generic").await;
    let requester = seed_user(&pool, ORG_A, "NullEmp-Req").await;
    let approver = seed_user(&pool, ORG_A, "NullEmp-App").await;

    let null_emp: Option<Uuid> = sqlx::query_scalar("SELECT employee_id FROM users WHERE id = $1")
        .bind(*requester.as_uuid())
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(
        null_emp.is_none(),
        "fixture must be the migration-0076 normal state"
    );

    let store = PgGovernanceStore::new(runtime_role_pool(&pool).await);
    let request_ref = Uuid::new_v4();

    scope_org(OrgId::from_uuid(ORG_A), async {
        store
            .create_approval(CreateApprovalCommand {
                requester,
                request_ref,
                kind: "override".to_owned(),
                target_ref: None,
                payload_summary: json!({}),
                trace: trace(),
                occurred_at: now(),
            })
            .await
            .unwrap();
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
            .unwrap()
    })
    .await;
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn pending_inbox_page_excludes_requester_decided_and_other_org(pool: PgPool) {
    seed_org(&pool, ORG_A, "inbox-a").await;
    seed_org(&pool, ORG_B, "inbox-b").await;
    let binder = seed_user(&pool, ORG_A, "Inbox-Binder").await;
    let requester = seed_person_bound_user(
        &pool,
        ORG_A,
        "Inbox-Req",
        seed_employee(&pool, ORG_A, "inbox-req").await,
        seed_person(&pool, ORG_A).await,
        binder,
    )
    .await;
    let approver = seed_person_bound_user(
        &pool,
        ORG_A,
        "Inbox-App",
        seed_employee(&pool, ORG_A, "inbox-app").await,
        seed_person(&pool, ORG_A).await,
        binder,
    )
    .await;
    let other_org_user = seed_user(&pool, ORG_B, "Inbox-B").await;
    let store = PgGovernanceStore::new(runtime_role_pool(&pool).await);
    let pending_ref = Uuid::new_v4();
    let decided_ref = Uuid::new_v4();
    let foreign_ref = Uuid::new_v4();
    let created_at = now();

    let pending = scope_org(OrgId::from_uuid(ORG_A), async {
        store
            .create_approval(CreateApprovalCommand {
                requester,
                request_ref: pending_ref,
                kind: "company.revise".to_owned(),
                target_ref: None,
                payload_summary: json!({"case": "pending"}),
                trace: trace(),
                occurred_at: created_at,
            })
            .await
    })
    .await
    .unwrap();
    scope_org(OrgId::from_uuid(ORG_A), async {
        store
            .create_approval(CreateApprovalCommand {
                requester,
                request_ref: decided_ref,
                kind: "override".to_owned(),
                target_ref: None,
                payload_summary: json!({"case": "decided"}),
                trace: trace(),
                occurred_at: created_at,
            })
            .await
            .unwrap();
        store
            .decide_pending_approval(DecidePendingApprovalCommand {
                approver,
                request_ref: decided_ref,
                kind: "override".to_owned(),
                decision: ApprovalDecision::Approved,
                trace: trace(),
                occurred_at: now(),
            })
            .await
            .unwrap();
    })
    .await;
    scope_org(OrgId::from_uuid(ORG_B), async {
        store
            .create_approval(CreateApprovalCommand {
                requester: other_org_user,
                request_ref: foreign_ref,
                kind: "override".to_owned(),
                target_ref: None,
                payload_summary: json!({"case": "foreign"}),
                trace: trace(),
                occurred_at: created_at,
            })
            .await
            .unwrap()
    })
    .await;

    let as_of = created_at + time::Duration::seconds(1);
    let for_requester = scope_org(OrgId::from_uuid(ORG_A), async {
        store
            .list_pending_action_inbox_page(requester, as_of, None, 50)
            .await
    })
    .await
    .unwrap();
    assert_eq!(for_requester.1, 0);
    assert!(for_requester.0.is_empty());

    let for_approver = scope_org(OrgId::from_uuid(ORG_A), async {
        store
            .list_pending_action_inbox_page(approver, as_of, None, 50)
            .await
    })
    .await
    .unwrap();
    assert_eq!(for_approver.1, 1);
    assert_eq!(for_approver.0.len(), 1);
    assert_eq!(for_approver.0[0].id, pending.id);
    assert_eq!(for_approver.0[0].request_ref, pending_ref);
    assert_eq!(for_approver.0[0].kind, "company.revise");
    assert!(!for_approver.2);

    let other_org_for_approver = scope_org(OrgId::from_uuid(ORG_B), async {
        store
            .list_pending_action_inbox_page(approver, as_of, None, 50)
            .await
    })
    .await
    .unwrap();
    assert_eq!(other_org_for_approver.1, 1);
    assert_eq!(other_org_for_approver.0[0].request_ref, foreign_ref);
    assert_ne!(other_org_for_approver.0[0].id, pending.id);

    let other_org_for_requester = scope_org(OrgId::from_uuid(ORG_B), async {
        store
            .list_pending_action_inbox_page(other_org_user, as_of, None, 50)
            .await
    })
    .await
    .unwrap();
    assert_eq!(other_org_for_requester.1, 0);
    assert!(other_org_for_requester.0.is_empty());
}
