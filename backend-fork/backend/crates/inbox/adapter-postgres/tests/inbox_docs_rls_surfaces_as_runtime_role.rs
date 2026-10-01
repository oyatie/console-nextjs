#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! RUNTIME RLS + recipient-isolation gate for the statutory-notice vault.
//!
//! Proven as the genuine non-owner runtime role `console_rt` (NOSUPERUSER,
//! NOBYPASSRLS, FORCE RLS) — NOT the `#[sqlx::test]` BYPASSRLS superuser pool,
//! which sees every row and would green-light a broken recipient filter. There
//! is no per-person GUC, so recipient scoping is enforced in application code.
//!
//! This is the compliance-critical proof: a non-recipient can neither read nor
//! confirm another user's legal notice (deny-by-omission → NotFound); a locked
//! legal notice never discloses its body before receipt; reading it does not
//! auto-confirm; and a double-confirm is idempotent.

use console_inbox_adapter_postgres::PgInboxStore;
use console_inbox_application::{
    ConfirmReceiptCommand, EmitInboxDocCommand, GetInboxDocQuery, InboxDocFilter,
    ListInboxDocsQuery,
};
use console_inbox_domain::{InboxDocKind, NewInboxDoc};
use console_kernel_core::{ErrorKind, InboxDocId, OrgId, TraceContext, UserId};
use serde_json::json;
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use time::OffsetDateTime;
use uuid::Uuid;

const OTHER_ORG: Uuid = Uuid::from_u128(0x7202_7202_7202_7202_7202_7202_7202_7202);

async fn runtime_role_pool(owner_pool: &PgPool) -> PgPool {
    for grant in [
        "GRANT SELECT, INSERT, UPDATE ON inbox_docs TO console_rt",
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

fn legal_notice_to(recipient: UserId, dedup_key: Option<&str>) -> EmitInboxDocCommand {
    EmitInboxDocCommand {
        actor: None,
        recipient,
        doc: NewInboxDoc::new(
            InboxDocKind::LegalNotice,
            "연차 사용 촉진 통지 (1차)",
            Some("연차촉진"),
            Some("근로기준법 §61"),
            Some("workflow_run"),
            Some("AP-3111"),
            json!({ "paragraphs": ["귀하의 미사용 연차에 대해 사용을 촉진합니다."] }),
        )
        .unwrap(),
        dedup_key: dedup_key.map(str::to_owned),
        trace: TraceContext::generate(),
        occurred_at: OffsetDateTime::now_utc(),
    }
}

fn payslip_to(recipient: UserId) -> EmitInboxDocCommand {
    EmitInboxDocCommand {
        actor: None,
        recipient,
        doc: NewInboxDoc::new(
            InboxDocKind::Payslip,
            "6월 급여명세",
            None,
            None,
            Some("payroll_run"),
            Some("PR-2026-06"),
            json!({ "net": 3_120_000, "base": 2_800_000 }),
        )
        .unwrap(),
        dedup_key: None,
        trace: TraceContext::generate(),
        occurred_at: OffsetDateTime::now_utc(),
    }
}

fn confirm(recipient: UserId, doc_id: InboxDocId) -> ConfirmReceiptCommand {
    ConfirmReceiptCommand {
        recipient,
        doc_id,
        trace: TraceContext::generate(),
        occurred_at: OffsetDateTime::now_utc(),
    }
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn legal_notice_lock_confirm_and_cross_user_isolation(owner_pool: PgPool) {
    let rt_pool = runtime_role_pool(&owner_pool).await;
    let knl = OrgId::knl();
    let other = OrgId::from_uuid(OTHER_ORG);
    seed_org(&owner_pool, OTHER_ORG, "Other").await;
    let user_a = seed_user(&owner_pool, *knl.as_uuid(), "Employee A").await;
    let user_b = seed_user(&owner_pool, *knl.as_uuid(), "Employee B").await;
    let store = PgInboxStore::new(rt_pool.clone());

    // Deliver a legal notice to A.
    let doc = console_platform_request_context::scope_org(knl, async {
        store.emit_inbox_doc(legal_notice_to(user_a, None)).await
    })
    .await
    .expect("emit legal notice to A");
    assert!(doc.locked, "a fresh legal notice is locked");
    assert_eq!(doc.recipient_user_id, user_a);

    // (a) LOCK-BEFORE-CONFIRM: A reads the locked doc — metadata yes, body no,
    //     and reading does NOT auto-confirm.
    let locked_view = console_platform_request_context::scope_org(knl, async {
        store
            .get(GetInboxDocQuery {
                recipient: user_a,
                id: doc.id,
            })
            .await
    })
    .await
    .expect("A reads locked doc");
    assert!(locked_view.summary.locked);
    assert!(
        locked_view.payload.is_none(),
        "a locked legal notice must not disclose its body before receipt"
    );
    assert!(
        locked_view.summary.confirmed_at.is_none(),
        "reading a locked doc must not auto-confirm it"
    );

    // (b) CROSS-USER READ: B cannot read A's doc — NotFound, invisibly.
    let cross_read = console_platform_request_context::scope_org(knl, async {
        store
            .get(GetInboxDocQuery {
                recipient: user_b,
                id: doc.id,
            })
            .await
    })
    .await;
    assert_eq!(
        cross_read.expect_err("B reading A's doc must fail").kind(),
        ErrorKind::NotFound,
        "a non-recipient gets deny-by-omission (NotFound), not a leak"
    );

    // (c) CROSS-USER CONFIRM: B cannot confirm A's receipt — NotFound.
    let cross_confirm = console_platform_request_context::scope_org(knl, async {
        store.confirm_receipt(confirm(user_b, doc.id)).await
    })
    .await;
    assert_eq!(
        cross_confirm
            .expect_err("B confirming A's receipt must fail")
            .kind(),
        ErrorKind::NotFound,
        "a non-recipient cannot forge another's legal receipt"
    );

    // A's doc is still unconfirmed after the failed cross-user confirm.
    let still_locked = console_platform_request_context::scope_org(knl, async {
        store
            .get(GetInboxDocQuery {
                recipient: user_a,
                id: doc.id,
            })
            .await
    })
    .await
    .expect("A re-reads");
    assert!(still_locked.summary.locked, "A's doc stays locked");

    // (d) A confirms its own receipt -> unlocked, stamped by A.
    let confirmed = console_platform_request_context::scope_org(knl, async {
        store.confirm_receipt(confirm(user_a, doc.id)).await
    })
    .await
    .expect("A confirms own receipt");
    assert!(!confirmed.locked);
    assert_eq!(confirmed.confirmed_by, Some(user_a));
    assert!(confirmed.confirmed_at.is_some());

    // After confirm, the body is disclosed.
    let unlocked_view = console_platform_request_context::scope_org(knl, async {
        store
            .get(GetInboxDocQuery {
                recipient: user_a,
                id: doc.id,
            })
            .await
    })
    .await
    .expect("A reads unlocked doc");
    assert!(
        unlocked_view.payload.is_some(),
        "body is disclosed once receipt is confirmed"
    );

    // (e) DOUBLE-CONFIRM is idempotent: same stamp, and only ONE receipt audit
    //     event exists.
    let again = console_platform_request_context::scope_org(knl, async {
        store.confirm_receipt(confirm(user_a, doc.id)).await
    })
    .await
    .expect("second confirm is a no-op");
    assert_eq!(
        again.confirmed_at, confirmed.confirmed_at,
        "re-confirm keeps the original stamp"
    );
    let receipt_events: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM audit_events \
         WHERE action = 'inbox_doc.confirm_receipt' AND target_id = $1",
    )
    .bind(doc.id.to_string())
    .fetch_one(&owner_pool)
    .await
    .unwrap();
    assert_eq!(
        receipt_events, 1,
        "a double-confirm records exactly one legal-receipt event"
    );

    // (f) CROSS-TENANT: under another org's GUC, A's doc is invisible (RLS).
    let cross_tenant = console_platform_request_context::scope_org(other, async {
        store
            .list(ListInboxDocsQuery {
                recipient: user_a,
                filter: InboxDocFilter::All,
                before_id: None,
                limit: 50,
            })
            .await
    })
    .await
    .expect("cross-tenant list succeeds");
    assert!(
        cross_tenant.items.is_empty(),
        "another tenant sees none of A's inbox documents"
    );
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn payslip_is_frictionless_and_never_receipt_confirmed(owner_pool: PgPool) {
    let rt_pool = runtime_role_pool(&owner_pool).await;
    let knl = OrgId::knl();
    let user = seed_user(&owner_pool, *knl.as_uuid(), "Payee").await;
    let store = PgInboxStore::new(rt_pool.clone());

    let pay = console_platform_request_context::scope_org(knl, async {
        store.emit_inbox_doc(payslip_to(user)).await
    })
    .await
    .expect("emit payslip");
    assert!(
        !pay.locked,
        "a payslip is never locked (frictionless self-view)"
    );

    // Self-view discloses the body immediately, no confirmation gate.
    let view = console_platform_request_context::scope_org(knl, async {
        store
            .get(GetInboxDocQuery {
                recipient: user,
                id: pay.id,
            })
            .await
    })
    .await
    .expect("payslip self-view");
    assert!(
        view.payload.is_some(),
        "payslip body is always visible to its owner"
    );

    // A payslip cannot be receipt-confirmed.
    let rejected = console_platform_request_context::scope_org(knl, async {
        store.confirm_receipt(confirm(user, pay.id)).await
    })
    .await;
    assert_eq!(
        rejected
            .expect_err("payslip confirm must be rejected")
            .kind(),
        ErrorKind::Validation,
        "a payslip is a self-view, not a receipt-gated legal notice"
    );
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn filters_and_dedup_idempotency(owner_pool: PgPool) {
    let rt_pool = runtime_role_pool(&owner_pool).await;
    let knl = OrgId::knl();
    let user = seed_user(&owner_pool, *knl.as_uuid(), "Filterer").await;
    let store = PgInboxStore::new(rt_pool.clone());

    let legal = console_platform_request_context::scope_org(knl, async {
        store.emit_inbox_doc(legal_notice_to(user, None)).await
    })
    .await
    .expect("emit legal");
    console_platform_request_context::scope_org(knl, async {
        store.emit_inbox_doc(payslip_to(user)).await
    })
    .await
    .expect("emit payslip");

    let list = |filter: InboxDocFilter| {
        let store = store.clone();
        async move {
            console_platform_request_context::scope_org(knl, async move {
                store
                    .list(ListInboxDocsQuery {
                        recipient: user,
                        filter,
                        before_id: None,
                        limit: 50,
                    })
                    .await
            })
            .await
            .unwrap()
        }
    };

    // 확인 필요 = the unconfirmed legal notice only.
    let action = list(InboxDocFilter::ActionRequired).await;
    assert_eq!(action.items.len(), 1);
    assert_eq!(action.items[0].id, legal.id);
    // 급여명세 = the payslip only.
    assert_eq!(list(InboxDocFilter::Payslip).await.items.len(), 1);
    // 완료 = none yet.
    assert_eq!(list(InboxDocFilter::Done).await.items.len(), 0);
    // 전체 = both.
    assert_eq!(list(InboxDocFilter::All).await.items.len(), 2);

    // Confirm the legal notice -> it leaves 확인 필요 and enters 완료.
    console_platform_request_context::scope_org(knl, async {
        store.confirm_receipt(confirm(user, legal.id)).await
    })
    .await
    .expect("confirm");
    assert_eq!(list(InboxDocFilter::ActionRequired).await.items.len(), 0);
    assert_eq!(list(InboxDocFilter::Done).await.items.len(), 1);

    // Dedup: two emits with the same key produce ONE row.
    let first = console_platform_request_context::scope_org(knl, async {
        store
            .emit_inbox_doc(legal_notice_to(user, Some("promote-run-1")))
            .await
    })
    .await
    .expect("first dedup emit");
    let second = console_platform_request_context::scope_org(knl, async {
        store
            .emit_inbox_doc(legal_notice_to(user, Some("promote-run-1")))
            .await
    })
    .await
    .expect("second dedup emit");
    assert_eq!(first.id, second.id, "same dedup_key returns the same row");
    let row_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM inbox_docs WHERE dedup_key = $1")
        .bind("promote-run-1")
        .fetch_one(&owner_pool)
        .await
        .unwrap();
    assert_eq!(row_count, 1, "dedup_key never doubles a delivery");
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn dedup_redelivery_requires_every_immutable_field_and_preserves_the_receipt(owner: PgPool) {
    let org = OrgId::knl();
    let recipient = seed_user(&owner, *org.as_uuid(), "Recipient").await;
    let rt = runtime_role_pool(&owner).await;
    let store = PgInboxStore::new(rt);
    let first = console_platform_request_context::scope_org(org, async {
        store
            .emit_inbox_doc(legal_notice_to(recipient, Some("exact-artifact")))
            .await
    })
    .await
    .unwrap();
    console_platform_request_context::scope_org(org, async {
        store.confirm_receipt(confirm(recipient, first.id)).await
    })
    .await
    .unwrap();
    let rows = "SELECT to_jsonb(d)::text FROM inbox_docs d ORDER BY id";
    let before: Vec<String> = sqlx::query_scalar(rows).fetch_all(&owner).await.unwrap();
    let audits: Vec<String> =
        sqlx::query_scalar("SELECT to_jsonb(a)::text FROM audit_events a ORDER BY id")
            .fetch_all(&owner)
            .await
            .unwrap();
    let exact = console_platform_request_context::scope_org(org, async {
        store
            .emit_inbox_doc(legal_notice_to(recipient, Some("exact-artifact")))
            .await
    })
    .await
    .unwrap();
    assert_eq!(exact.id, first.id);
    assert!(
        !exact.locked,
        "valid redelivery preserves prior receipt confirmation"
    );
    for field in [
        "kind",
        "title",
        "notice_type",
        "legal_basis",
        "source_kind",
        "source_id",
        "payload",
    ] {
        let mut changed = legal_notice_to(recipient, Some("exact-artifact"));
        match field {
            "kind" => {
                changed.doc.kind = InboxDocKind::Payslip;
                changed.doc.notice_type = None;
            }
            "title" => changed.doc.title = "다른 통지".to_owned(),
            "notice_type" => changed.doc.notice_type = Some("근로계약".to_owned()),
            "legal_basis" => changed.doc.legal_basis = Some("다른 근거".to_owned()),
            "source_kind" => changed.doc.source_kind = Some("other_source".to_owned()),
            "source_id" => changed.doc.source_id = Some("different-version".to_owned()),
            _ => changed.doc.payload = json!({"paragraphs":["changed private content"]}),
        }
        let result = console_platform_request_context::scope_org(org, async {
            store.emit_inbox_doc(changed).await
        })
        .await;
        assert!(
            result.is_err(),
            "same key with different immutable input must conflict"
        );
        let err = result.unwrap_err();
        assert_eq!(err.kind(), ErrorKind::Conflict, "field={field}: {err:?}");
        assert!(
            !err.to_string().contains("private content"),
            "no document content in conflict"
        );
        assert_eq!(
            sqlx::query_scalar::<_, String>(rows)
                .fetch_all(&owner)
                .await
                .unwrap(),
            before
        );
        assert_eq!(
            sqlx::query_scalar::<_, String>(
                "SELECT to_jsonb(a)::text FROM audit_events a ORDER BY id"
            )
            .fetch_all(&owner)
            .await
            .unwrap(),
            audits
        );
    }
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn concurrent_dedup_collision_revalidates_the_committed_document(owner: PgPool) {
    let org = OrgId::knl();
    let recipient = seed_user(&owner, *org.as_uuid(), "Recipient").await;
    let mut initial = owner.begin().await.unwrap();
    let blocker: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *initial)
        .await
        .unwrap();
    let input = legal_notice_to(recipient, Some("racing-artifact"));
    let doc = &input.doc;
    sqlx::query("INSERT INTO inbox_docs (org_id,recipient_user_id,kind,notice_type,title,legal_basis,source_kind,source_id,payload,dedup_key) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)")
        .bind(org.as_uuid()).bind(recipient.as_uuid()).bind(doc.kind.as_str()).bind(&doc.notice_type).bind(&doc.title).bind(&doc.legal_basis).bind(&doc.source_kind).bind(&doc.source_id).bind(&doc.payload).bind(&input.dedup_key)
        .execute(&mut *initial).await.unwrap();
    let rt = PgPoolOptions::new()
        .max_connections(2)
        .after_connect(|c, _| {
            Box::pin(async move {
                sqlx::query("SET ROLE console_rt").execute(c).await?;
                Ok(())
            })
        })
        .connect_with(
            owner
                .connect_options()
                .as_ref()
                .clone()
                .application_name("inbox-dedup-race"),
        )
        .await
        .unwrap();
    let store = PgInboxStore::new(rt);
    let mut changed = legal_notice_to(recipient, Some("racing-artifact"));
    changed.doc.payload = json!({"paragraphs":["conflicting"]});
    let pending = tokio::spawn(async move {
        console_platform_request_context::scope_org(org, async {
            store.emit_inbox_doc(changed).await
        })
        .await
    });
    let observed=tokio::time::timeout(std::time::Duration::from_secs(10),async{
        loop{
            let blocked:bool=sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND application_name='inbox-dedup-race' AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)))")
                .bind(blocker).fetch_one(&owner).await.unwrap();
            if blocked {break;}
            tokio::task::yield_now().await;
        }
    }).await.is_ok();
    initial.commit().await.unwrap();
    let result = tokio::time::timeout(std::time::Duration::from_secs(10), pending)
        .await
        .unwrap()
        .unwrap();
    assert!(result.is_err(), "concurrent changed artifact must conflict");
    let error = result.unwrap_err();
    assert!(
        observed,
        "the conflicting insert must actually collide with the uncommitted unique key"
    );
    assert_eq!(error.kind(), ErrorKind::Conflict);
    let count:(i64,i64)=sqlx::query_as("SELECT (SELECT COUNT(*) FROM inbox_docs WHERE dedup_key='racing-artifact'),(SELECT COUNT(*) FROM audit_events WHERE action='inbox_doc.emit')").fetch_one(&owner).await.unwrap();
    assert_eq!(count, (1, 0));
}
