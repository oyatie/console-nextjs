#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! RUNTIME RLS gate for the notice board, proven as the genuine non-owner
//! runtime role `console_rt` (NOSUPERUSER, NOBYPASSRLS, FORCE RLS) — not the
//! default `#[sqlx::test]` BYPASSRLS superuser pool. Proves: draft visibility
//! is publish-tier-gated, publish snapshots the effective audience (org-wide
//! or branch-scoped via `user_branches`) + issues an NT- code + fans out one
//! notification per recipient, drafts are editable and frozen at publish,
//! 수령확인 progress/receipts are correct, and cross-org isolation holds
//! throughout — including for `notice_audience_branches`.

use console_kernel_core::{BranchId, NoticeId, OrgId, TraceContext, UserId};
use console_notices_adapter_postgres::PgNoticeStore;
use console_notices_application::{
    AcknowledgeNoticeCommand, CreateDraftNoticeCommand, GetNoticeQuery, ListNoticeReceiptsQuery,
    ListNoticesQuery, NoticeAudienceInput, NoticeProgressQuery, PublishNoticeCommand,
    UpdateDraftNoticeCommand,
};
use console_notifications_adapter_postgres::PgNotificationStore;
use console_notifications_application::{ListNotificationsQuery, UnreadNotificationCountQuery};
use console_platform_db::{DbError, with_org_conn};
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use std::sync::Arc;
use time::OffsetDateTime;
use uuid::Uuid;

const OTHER_ORG: Uuid = Uuid::from_u128(0x7303_7303_7303_7303_7303_7303_7303_7303);

async fn runtime_role_pool(owner_pool: &PgPool) -> PgPool {
    for grant in [
        "GRANT SELECT, INSERT, UPDATE ON notices TO console_rt",
        "GRANT SELECT, INSERT, UPDATE ON notice_receipts TO console_rt",
        "GRANT SELECT, INSERT, UPDATE ON notifications TO console_rt",
        "GRANT SELECT, INSERT, UPDATE ON object_code_counters TO console_rt",
        "GRANT SELECT ON object_types TO console_rt",
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
    sqlx::query(
        "INSERT INTO users (id, display_name, roles, org_id, is_active) VALUES ($1, $2, $3, $4, true)",
    )
    .bind(user_id.as_uuid())
    .bind(format!("{name} {}", Uuid::new_v4()))
    .bind(Vec::from(["ADMIN"]))
    .bind(org)
    .execute(owner_pool)
    .await
    .unwrap();
    user_id
}

async fn seed_branch(owner_pool: &PgPool, org: Uuid, name: &str) -> BranchId {
    let region: Uuid =
        sqlx::query_scalar("INSERT INTO regions (name, org_id) VALUES ($1, $2) RETURNING id")
            .bind(format!("region-{name}"))
            .bind(org)
            .fetch_one(owner_pool)
            .await
            .unwrap();
    BranchId::from_uuid(
        sqlx::query_scalar(
            "INSERT INTO branches (region_id, name, org_id) VALUES ($1, $2, $3) RETURNING id",
        )
        .bind(region)
        .bind(name)
        .bind(org)
        .fetch_one(owner_pool)
        .await
        .unwrap(),
    )
}

async fn join_branch(owner_pool: &PgPool, org: Uuid, user: UserId, branch: BranchId) {
    sqlx::query("INSERT INTO user_branches (user_id, branch_id, org_id) VALUES ($1, $2, $3)")
        .bind(user.as_uuid())
        .bind(branch.as_uuid())
        .bind(org)
        .execute(owner_pool)
        .await
        .unwrap();
}

fn draft(author: UserId) -> CreateDraftNoticeCommand {
    CreateDraftNoticeCommand {
        author,
        title: "2026년 정기인사 명령".to_owned(),
        body: "전사 정기인사를 아래와 같이 공지합니다.".to_owned(),
        category: Some("hr_order".to_owned()),
        audience: None,
        trace: TraceContext::generate(),
        occurred_at: OffsetDateTime::now_utc(),
    }
}

fn publish(notice_id: NoticeId, publisher: UserId) -> PublishNoticeCommand {
    PublishNoticeCommand {
        notice_id,
        publisher,
        trace: TraceContext::generate(),
        occurred_at: OffsetDateTime::now_utc(),
    }
}

fn ack(notice_id: NoticeId, recipient: UserId) -> AcknowledgeNoticeCommand {
    AcknowledgeNoticeCommand {
        notice_id,
        recipient,
        trace: TraceContext::generate(),
        occurred_at: OffsetDateTime::now_utc(),
    }
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn draft_visibility_publish_and_progress_as_runtime_role(owner_pool: PgPool) {
    let rt_pool = runtime_role_pool(&owner_pool).await;
    let knl = OrgId::knl();
    let other = OrgId::from_uuid(OTHER_ORG);
    seed_org(&owner_pool, OTHER_ORG, "Other").await;

    let author = seed_user(&owner_pool, *knl.as_uuid(), "총무팀").await;
    let recipient_a = seed_user(&owner_pool, *knl.as_uuid(), "직원 A").await;
    let recipient_b = seed_user(&owner_pool, *knl.as_uuid(), "직원 B").await;

    let notifications = PgNotificationStore::new(rt_pool.clone());
    let store =
        PgNoticeStore::new(rt_pool.clone()).with_notification_sink(Arc::new(notifications.clone()));

    // Create a draft.
    let created = console_platform_request_context::scope_org(knl, async {
        store.create_draft(draft(author)).await
    })
    .await
    .expect("create draft");
    assert_eq!(created.status, "draft");
    assert!(created.code.is_none(), "a draft has no code yet");
    assert_eq!(created.category, "hr_order");
    assert_eq!(created.audience_scope, "org");
    assert!(created.audience_branches.is_empty());
    assert!(created.my_receipt.is_none(), "no receipts before publish");
    let progress = created.progress.expect("manager summary carries progress");
    assert_eq!((progress.total, progress.acknowledged), (0, 0));

    // (a) draft visibility is publish-tier-gated: a non-manager get() is
    // NotFound, never a silent leak of unpublished content.
    let hidden = console_platform_request_context::scope_org(knl, async {
        store
            .get(
                GetNoticeQuery {
                    notice_id: created.id,
                    viewer: recipient_a,
                },
                false,
            )
            .await
    })
    .await;
    assert!(hidden.is_err(), "a non-manager must not see a draft");

    let visible_to_author = console_platform_request_context::scope_org(knl, async {
        store
            .get(
                GetNoticeQuery {
                    notice_id: created.id,
                    viewer: author,
                },
                true,
            )
            .await
    })
    .await
    .expect("publish-tier caller sees the draft");
    assert_eq!(visible_to_author.id, created.id);

    // A draft never appears in a non-manager's list.
    let public_list = console_platform_request_context::scope_org(knl, async {
        store
            .list(ListNoticesQuery {
                include_drafts: false,
                limit: 50,
                viewer: recipient_a,
            })
            .await
    })
    .await
    .expect("public list");
    assert!(
        public_list.is_empty(),
        "an unpublished draft must not appear in the public list"
    );

    // (b) publish: issues an NT- code, snapshots every active org member into
    // notice_receipts, and fans out one notification per recipient.
    let published = console_platform_request_context::scope_org(knl, async {
        store.publish(publish(created.id, author)).await
    })
    .await
    .expect("publish");
    assert_eq!(published.status, "published");
    let code = published.code.clone().expect("published notice has a code");
    assert!(
        code.starts_with("NT-"),
        "code {code} must carry the NT- prefix"
    );
    let progress = published.progress.expect("publisher sees progress");
    assert_eq!((progress.total, progress.acknowledged), (3, 0));

    // Publishing twice is a Conflict, not a silent duplicate code/receipt set.
    let republish = console_platform_request_context::scope_org(knl, async {
        store.publish(publish(created.id, author)).await
    })
    .await;
    assert!(
        republish.is_err(),
        "publishing an already-published notice must fail"
    );

    // A published notice is frozen: draft edits are rejected.
    let frozen = console_platform_request_context::scope_org(knl, async {
        store
            .update_draft(UpdateDraftNoticeCommand {
                notice_id: created.id,
                editor: author,
                title: Some("몰래 고친 제목".to_owned()),
                body: None,
                category: None,
                audience: None,
                trace: TraceContext::generate(),
                occurred_at: OffsetDateTime::now_utc(),
            })
            .await
    })
    .await;
    assert!(frozen.is_err(), "a published notice must not be editable");

    // Now visible in the public list, with the caller's own receipt state.
    let public_list_after = console_platform_request_context::scope_org(knl, async {
        store
            .list(ListNoticesQuery {
                include_drafts: false,
                limit: 50,
                viewer: recipient_a,
            })
            .await
    })
    .await
    .expect("public list after publish");
    assert_eq!(public_list_after.len(), 1);
    assert_eq!(public_list_after[0].id, created.id);
    let my_receipt = public_list_after[0]
        .my_receipt
        .expect("recipient sees their own receipt state");
    assert!(my_receipt.acknowledged_at.is_none());
    assert!(
        public_list_after[0].progress.is_none(),
        "a non-manager list row must not carry progress"
    );

    // Every active org member (author + A + B) got a notification pointing at
    // the notice.
    for recipient in [author, recipient_a, recipient_b] {
        let unread = console_platform_request_context::scope_org(knl, async {
            notifications
                .unread_count(UnreadNotificationCountQuery { recipient })
                .await
        })
        .await
        .expect("unread count");
        assert_eq!(unread, 1, "recipient must have exactly one notification");
        let list = console_platform_request_context::scope_org(knl, async {
            notifications
                .list(ListNotificationsQuery {
                    recipient,
                    unread_only: true,
                    before_id: None,
                    limit: 10,
                })
                .await
        })
        .await
        .expect("list");
        assert_eq!(list.items[0].category, "공지");
    }

    // (c) 수령확인 progress starts at 0/3.
    let progress_before = console_platform_request_context::scope_org(knl, async {
        store
            .progress(NoticeProgressQuery {
                notice_id: created.id,
            })
            .await
    })
    .await
    .expect("progress before");
    assert_eq!(progress_before.total, 3);
    assert_eq!(progress_before.acknowledged, 0);

    // Recipient A acknowledges; progress becomes 1/3. A cross-user
    // acknowledge attempt (someone who was never snapshotted) is NotFound.
    console_platform_request_context::scope_org(knl, async {
        store.acknowledge(ack(created.id, recipient_a)).await
    })
    .await
    .expect("A acknowledges");

    let stranger = UserId::new();
    let stranger_ack = console_platform_request_context::scope_org(knl, async {
        store.acknowledge(ack(created.id, stranger)).await
    })
    .await;
    assert!(
        stranger_ack.is_err(),
        "a non-recipient acknowledging must fail, not silently succeed"
    );

    let progress_after = console_platform_request_context::scope_org(knl, async {
        store
            .progress(NoticeProgressQuery {
                notice_id: created.id,
            })
            .await
    })
    .await
    .expect("progress after");
    assert_eq!(progress_after.total, 3);
    assert_eq!(progress_after.acknowledged, 1);

    // (d) cross-tenant: under another org's GUC, the notice is invisible.
    let cross_tenant_list = console_platform_request_context::scope_org(other, async {
        store
            .list(ListNoticesQuery {
                include_drafts: true,
                limit: 50,
                viewer: author,
            })
            .await
    })
    .await
    .expect("cross-tenant list itself succeeds");
    assert!(
        cross_tenant_list.is_empty(),
        "another tenant sees none of knl's notices"
    );
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn branch_scoped_audience_publish_and_receipts_as_runtime_role(owner_pool: PgPool) {
    let rt_pool = runtime_role_pool(&owner_pool).await;
    let knl = OrgId::knl();
    let knl_uuid = *knl.as_uuid();
    let other = OrgId::from_uuid(OTHER_ORG);
    seed_org(&owner_pool, OTHER_ORG, "Other").await;

    let author = seed_user(&owner_pool, knl_uuid, "본사 총무").await;
    let member_a1 = seed_user(&owner_pool, knl_uuid, "창원 대원 A1").await;
    let member_a2 = seed_user(&owner_pool, knl_uuid, "창원 대원 A2").await;
    let member_b = seed_user(&owner_pool, knl_uuid, "부산 대원 B").await;

    let branch_a = seed_branch(&owner_pool, knl_uuid, "창원지사").await;
    let branch_b = seed_branch(&owner_pool, knl_uuid, "부산지사").await;
    let branch_empty = seed_branch(&owner_pool, knl_uuid, "신설지사").await;
    join_branch(&owner_pool, knl_uuid, member_a1, branch_a).await;
    join_branch(&owner_pool, knl_uuid, member_a2, branch_a).await;
    join_branch(&owner_pool, knl_uuid, member_b, branch_b).await;

    let store = PgNoticeStore::new(rt_pool.clone());

    // Draft targeted at branch B first, then re-targeted to branch A by a
    // draft edit (audience replaced whole) — drafts are mutable.
    let created = console_platform_request_context::scope_org(knl, async {
        store
            .create_draft(CreateDraftNoticeCommand {
                author,
                title: "지사 안전교육 안내".to_owned(),
                body: "대상 지사는 일정 확인 바랍니다.".to_owned(),
                category: Some("training".to_owned()),
                audience: Some(NoticeAudienceInput {
                    scope: "branches".to_owned(),
                    branch_ids: vec![branch_b],
                }),
                trace: TraceContext::generate(),
                occurred_at: OffsetDateTime::now_utc(),
            })
            .await
    })
    .await
    .expect("create branch-scoped draft");
    assert_eq!(created.audience_scope, "branches");
    assert_eq!(created.audience_branches.len(), 1);
    assert_eq!(created.audience_branches[0].id, branch_b);
    assert_eq!(created.audience_branches[0].name, "부산지사");

    let retargeted = console_platform_request_context::scope_org(knl, async {
        store
            .update_draft(UpdateDraftNoticeCommand {
                notice_id: created.id,
                editor: author,
                title: None,
                body: None,
                category: None,
                audience: Some(NoticeAudienceInput {
                    scope: "branches".to_owned(),
                    branch_ids: vec![branch_a],
                }),
                trace: TraceContext::generate(),
                occurred_at: OffsetDateTime::now_utc(),
            })
            .await
    })
    .await
    .expect("retarget draft audience");
    assert_eq!(retargeted.audience_branches.len(), 1);
    assert_eq!(retargeted.audience_branches[0].id, branch_a);
    assert_eq!(retargeted.category, "training", "unchanged fields survive");

    // An audience branch belonging to another org is fail-closed validation.
    let foreign_branch = console_platform_request_context::scope_org(knl, async {
        store
            .update_draft(UpdateDraftNoticeCommand {
                notice_id: created.id,
                editor: author,
                title: None,
                body: None,
                category: None,
                audience: Some(NoticeAudienceInput {
                    scope: "branches".to_owned(),
                    branch_ids: vec![BranchId::new()],
                }),
                trace: TraceContext::generate(),
                occurred_at: OffsetDateTime::now_utc(),
            })
            .await
    })
    .await;
    assert!(
        foreign_branch.is_err(),
        "an unknown/foreign branch id must be rejected"
    );

    // Publishing to an empty effective audience is rejected before any state
    // changes — the notice stays a draft.
    let empty_draft = console_platform_request_context::scope_org(knl, async {
        store
            .create_draft(CreateDraftNoticeCommand {
                author,
                title: "빈 대상 공지".to_owned(),
                body: "게시되면 안 됩니다.".to_owned(),
                category: None,
                audience: Some(NoticeAudienceInput {
                    scope: "branches".to_owned(),
                    branch_ids: vec![branch_empty],
                }),
                trace: TraceContext::generate(),
                occurred_at: OffsetDateTime::now_utc(),
            })
            .await
    })
    .await
    .expect("create empty-audience draft");
    let empty_publish = console_platform_request_context::scope_org(knl, async {
        store.publish(publish(empty_draft.id, author)).await
    })
    .await;
    assert!(
        empty_publish.is_err(),
        "publishing to an empty audience must fail closed"
    );
    let still_draft = console_platform_request_context::scope_org(knl, async {
        store
            .get(
                GetNoticeQuery {
                    notice_id: empty_draft.id,
                    viewer: author,
                },
                true,
            )
            .await
    })
    .await
    .expect("empty-audience notice still readable to manager");
    assert_eq!(still_draft.status, "draft");
    assert!(
        still_draft.code.is_none(),
        "the rejected publish must roll back atomically — no NT- code leaks"
    );
    let leaked = still_draft
        .progress
        .expect("manager summary carries progress");
    assert_eq!(
        (leaked.total, leaked.acknowledged),
        (0, 0),
        "the rejected publish must leave zero receipt rows"
    );

    // Publish to branch A only: exactly its 2 members are snapshotted.
    let published = console_platform_request_context::scope_org(knl, async {
        store.publish(publish(created.id, author)).await
    })
    .await
    .expect("publish branch-scoped notice");
    let progress = published.progress.expect("publisher sees progress");
    assert_eq!((progress.total, progress.acknowledged), (2, 0));

    // The branch-B member was never snapshotted: ack fails, no receipt row.
    let outsider_ack = console_platform_request_context::scope_org(knl, async {
        store.acknowledge(ack(created.id, member_b)).await
    })
    .await;
    assert!(
        outsider_ack.is_err(),
        "a member outside the audience must not be able to acknowledge"
    );

    console_platform_request_context::scope_org(knl, async {
        store.acknowledge(ack(created.id, member_a1)).await
    })
    .await
    .expect("audience member acknowledges");

    // Receipts drill: 2 rows total, 1 outstanding, names hydrated.
    let all_receipts = console_platform_request_context::scope_org(knl, async {
        store
            .list_receipts(ListNoticeReceiptsQuery {
                notice_id: created.id,
                acknowledged: None,
                limit: 50,
                offset: 0,
            })
            .await
    })
    .await
    .expect("receipts page");
    assert_eq!(all_receipts.total, 2);
    assert_eq!(all_receipts.items.len(), 2);
    assert!(
        all_receipts.items[0].acknowledged_at.is_some(),
        "newest-ack-first ordering"
    );
    assert_eq!(all_receipts.items[0].recipient_user_id, member_a1);
    assert!(!all_receipts.items[1].display_name.is_empty());

    let outstanding = console_platform_request_context::scope_org(knl, async {
        store
            .list_receipts(ListNoticeReceiptsQuery {
                notice_id: created.id,
                acknowledged: Some(false),
                limit: 50,
                offset: 0,
            })
            .await
    })
    .await
    .expect("outstanding chase list");
    assert_eq!(outstanding.total, 1);
    assert_eq!(outstanding.items[0].recipient_user_id, member_a2);

    // Cross-tenant: notice_audience_branches rows are invisible under the
    // other org's GUC even though the runtime role holds table SELECT.
    let knl_rows: i64 = with_org_conn::<_, _, DbError>(&rt_pool, knl, |tx| {
        Box::pin(async move {
            sqlx::query_scalar("SELECT COUNT(*) FROM notice_audience_branches")
                .fetch_one(tx.as_mut())
                .await
                .map_err(DbError::Sqlx)
        })
    })
    .await
    .expect("count under knl");
    assert!(knl_rows >= 1, "knl sees its own audience rows");
    let other_rows: i64 = with_org_conn::<_, _, DbError>(&rt_pool, other, |tx| {
        Box::pin(async move {
            sqlx::query_scalar("SELECT COUNT(*) FROM notice_audience_branches")
                .fetch_one(tx.as_mut())
                .await
                .map_err(DbError::Sqlx)
        })
    })
    .await
    .expect("count under other org");
    assert_eq!(
        other_rows, 0,
        "another tenant sees none of knl's audience rows"
    );
}
