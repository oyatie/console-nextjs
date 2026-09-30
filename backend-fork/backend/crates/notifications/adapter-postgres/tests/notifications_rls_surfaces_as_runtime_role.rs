#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! RUNTIME RLS + recipient-isolation gate for the notification center.
//!
//! Proven as the genuine non-owner runtime role `console_rt` (NOSUPERUSER,
//! NOBYPASSRLS, FORCE RLS) — NOT the default `#[sqlx::test]` BYPASSRLS
//! superuser pool, which sees every row and would green-light a broken
//! recipient filter. There is no per-person GUC, so recipient scoping is
//! enforced in application code; this test is the thing that proves user B
//! cannot list or read-mark user A's notifications, and that another tenant
//! sees nothing.

use console_kernel_core::{ErrorKind, OrgId, TraceContext, UserId};
use console_notifications_adapter_postgres::PgNotificationStore;
use console_notifications_application::{
    DeleteNotificationPolicyCommand, EmitNotificationCommand, ListNotificationObjectGroupsQuery,
    ListNotificationPoliciesQuery, ListNotificationsQuery, MarkAllNotificationsReadCommand,
    MarkNotificationReadCommand, MarkNotificationUnreadCommand, NotificationCountsSummaryQuery,
    NotificationCreatedNotification, NotificationNotifier, NotificationNotifyFuture,
    UnreadNotificationCountQuery, UpsertNotificationPolicyCommand,
};
use console_notifications_domain::{
    NotificationCategory, NotificationLink, NotificationPolicyScope,
};
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use std::sync::{Arc, Mutex};
use time::OffsetDateTime;
use uuid::Uuid;

const OTHER_ORG: Uuid = Uuid::from_u128(0x7202_7202_7202_7202_7202_7202_7202_7202);

/// Records realtime notifier calls so the test can assert emit fires it exactly
/// once per genuinely-new row (and never on a dedup redelivery).
#[derive(Default)]
struct RecordingNotifier {
    calls: Mutex<Vec<NotificationCreatedNotification>>,
}

impl NotificationNotifier for RecordingNotifier {
    fn notification_created(
        &self,
        notification: NotificationCreatedNotification,
    ) -> NotificationNotifyFuture<'_> {
        Box::pin(async move {
            self.calls.lock().unwrap().push(notification);
        })
    }
}

async fn runtime_role_pool(owner_pool: &PgPool) -> PgPool {
    for grant in [
        "GRANT SELECT, INSERT, UPDATE ON notifications TO console_rt",
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

fn emit_to(recipient: UserId, category: &str, dedup_key: Option<&str>) -> EmitNotificationCommand {
    EmitNotificationCommand {
        actor: None,
        recipient,
        category: category.to_owned(),
        kind: "info".to_owned(),
        text: "결재 문서가 도착했습니다".to_owned(),
        link: NotificationLink::Object {
            kind: "approval".to_owned(),
            id: Uuid::new_v4().to_string(),
        },
        dedup_key: dedup_key.map(str::to_owned),
        trace: TraceContext::generate(),
        occurred_at: OffsetDateTime::now_utc(),
    }
}

fn unread_count_of(recipient: UserId) -> UnreadNotificationCountQuery {
    UnreadNotificationCountQuery { recipient }
}

fn list_unread(recipient: UserId) -> ListNotificationsQuery {
    ListNotificationsQuery {
        recipient,
        unread_only: true,
        before_id: None,
        limit: 50,
    }
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn recipient_isolation_and_read_marking_as_runtime_role(owner_pool: PgPool) {
    let rt_pool = runtime_role_pool(&owner_pool).await;
    let knl = OrgId::knl();
    let other = OrgId::from_uuid(OTHER_ORG);
    seed_org(&owner_pool, OTHER_ORG, "Other").await;
    let user_a = seed_user(&owner_pool, *knl.as_uuid(), "Approver A").await;
    let user_b = seed_user(&owner_pool, *knl.as_uuid(), "Approver B").await;

    let notifier = Arc::new(RecordingNotifier::default());
    let store = PgNotificationStore::new(rt_pool.clone()).with_notifier(notifier.clone());

    // Emit one to A and one to B (all under knl).
    let a_notif = console_platform_request_context::scope_org(knl, async {
        store.emit_notification(emit_to(user_a, "결재", None)).await
    })
    .await
    .expect("emit to A");
    console_platform_request_context::scope_org(knl, async {
        store.emit_notification(emit_to(user_b, "멘션", None)).await
    })
    .await
    .expect("emit to B");

    assert_eq!(a_notif.category, "결재");
    assert!(a_notif.unread);
    assert_eq!(a_notif.recipient_user_id, user_a);

    // (a) recipient isolation: A sees only A's; B sees only B's.
    let a_list = console_platform_request_context::scope_org(knl, async {
        store.list(list_unread(user_a)).await
    })
    .await
    .expect("A list");
    assert_eq!(a_list.items.len(), 1, "A sees exactly one notification");
    assert_eq!(a_list.items[0].id, a_notif.id);

    let b_list = console_platform_request_context::scope_org(knl, async {
        store.list(list_unread(user_b)).await
    })
    .await
    .expect("B list");
    assert_eq!(b_list.items.len(), 1);
    assert_ne!(
        b_list.items[0].id, a_notif.id,
        "B must never see A's notification"
    );

    // (b) cross-user read-mark: B marking A's notification is NotFound, not a
    //     silent success — and A's notification stays unread.
    let cross = console_platform_request_context::scope_org(knl, async {
        store
            .mark_read(MarkNotificationReadCommand {
                recipient: user_b,
                notification_id: a_notif.id,
                trace: TraceContext::generate(),
                occurred_at: OffsetDateTime::now_utc(),
            })
            .await
    })
    .await;
    let cross_err = cross.expect_err("B marking A's notification must fail");
    assert_eq!(
        cross_err.kind(),
        ErrorKind::NotFound,
        "B marking A's notification must be NotFound, not a silent success"
    );

    // A marks its own read -> unread=false, read_at set.
    let marked = console_platform_request_context::scope_org(knl, async {
        store
            .mark_read(MarkNotificationReadCommand {
                recipient: user_a,
                notification_id: a_notif.id,
                trace: TraceContext::generate(),
                occurred_at: OffsetDateTime::now_utc(),
            })
            .await
    })
    .await
    .expect("A marks own read");
    assert!(!marked.unread);
    assert!(marked.read_at.is_some());

    let a_unread_after = console_platform_request_context::scope_org(knl, async {
        store.list(list_unread(user_a)).await
    })
    .await
    .expect("A unread after");
    assert_eq!(
        a_unread_after.items.len(),
        0,
        "A has no unread after marking"
    );

    // (c) cross-tenant: under another org's GUC, A's rows are invisible (RLS).
    let cross_tenant = console_platform_request_context::scope_org(other, async {
        store.list(list_unread(user_a)).await
    })
    .await
    .expect("cross-tenant list itself succeeds");
    assert_eq!(
        cross_tenant.items.len(),
        0,
        "another tenant sees none of A's notifications"
    );
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn unread_count_is_recipient_scoped_as_runtime_role(owner_pool: PgPool) {
    let rt_pool = runtime_role_pool(&owner_pool).await;
    let knl = OrgId::knl();
    let user_a = seed_user(&owner_pool, *knl.as_uuid(), "Counter A").await;
    let user_b = seed_user(&owner_pool, *knl.as_uuid(), "Counter B").await;
    let store = PgNotificationStore::new(rt_pool.clone());

    // Zero unread to start.
    let zero = console_platform_request_context::scope_org(knl, async {
        store.unread_count(unread_count_of(user_a)).await
    })
    .await
    .expect("A count when empty");
    assert_eq!(zero, 0, "no notifications => zero unread");

    // Two for A, one for B.
    let a_first = console_platform_request_context::scope_org(knl, async {
        store.emit_notification(emit_to(user_a, "결재", None)).await
    })
    .await
    .expect("emit A#1");
    console_platform_request_context::scope_org(knl, async {
        store.emit_notification(emit_to(user_a, "멘션", None)).await
    })
    .await
    .expect("emit A#2");
    console_platform_request_context::scope_org(knl, async {
        store.emit_notification(emit_to(user_b, "공지", None)).await
    })
    .await
    .expect("emit B#1");

    let a_count = console_platform_request_context::scope_org(knl, async {
        store.unread_count(unread_count_of(user_a)).await
    })
    .await
    .expect("A count");
    assert_eq!(a_count, 2, "A has exactly its own two unread");

    // Cross-user isolation: B's count is unaffected by A's rows.
    let b_count = console_platform_request_context::scope_org(knl, async {
        store.unread_count(unread_count_of(user_b)).await
    })
    .await
    .expect("B count");
    assert_eq!(b_count, 1, "B sees only its own unread");

    // Read rows are excluded: marking one of A's read drops the count to one.
    console_platform_request_context::scope_org(knl, async {
        store
            .mark_read(MarkNotificationReadCommand {
                recipient: user_a,
                notification_id: a_first.id,
                trace: TraceContext::generate(),
                occurred_at: OffsetDateTime::now_utc(),
            })
            .await
    })
    .await
    .expect("A marks one read");
    let a_after = console_platform_request_context::scope_org(knl, async {
        store.unread_count(unread_count_of(user_a)).await
    })
    .await
    .expect("A count after read");
    assert_eq!(a_after, 1, "read rows are excluded from the unread count");
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn mark_all_read_and_dedup_idempotency_as_runtime_role(owner_pool: PgPool) {
    let rt_pool = runtime_role_pool(&owner_pool).await;
    let knl = OrgId::knl();
    let user = seed_user(&owner_pool, *knl.as_uuid(), "Busy User").await;

    let notifier = Arc::new(RecordingNotifier::default());
    let store = PgNotificationStore::new(rt_pool.clone()).with_notifier(notifier.clone());

    // Three unread notifications.
    for cat in ["결재", "근태", "급여"] {
        console_platform_request_context::scope_org(knl, async {
            store.emit_notification(emit_to(user, cat, None)).await
        })
        .await
        .expect("emit");
    }

    let marked = console_platform_request_context::scope_org(knl, async {
        store
            .mark_all_read(MarkAllNotificationsReadCommand {
                recipient: user,
                trace: TraceContext::generate(),
                occurred_at: OffsetDateTime::now_utc(),
            })
            .await
    })
    .await
    .expect("mark all");
    assert_eq!(marked, 3, "all three unread are marked");

    let unread_after = console_platform_request_context::scope_org(knl, async {
        store.list(list_unread(user)).await
    })
    .await
    .expect("list");
    assert!(unread_after.items.is_empty());

    // Dedup: two emits with the same key produce ONE row and fire the realtime
    // notifier ONCE (the redelivery is a no-op returning the existing row).
    let notifier_calls_before = notifier.calls.lock().unwrap().len();
    let first = console_platform_request_context::scope_org(knl, async {
        store
            .emit_notification(emit_to(user, "공지", Some("outbox-evt-1")))
            .await
    })
    .await
    .expect("first dedup emit");
    let second = console_platform_request_context::scope_org(knl, async {
        store
            .emit_notification(emit_to(user, "공지", Some("outbox-evt-1")))
            .await
    })
    .await
    .expect("second dedup emit");
    assert_eq!(first.id, second.id, "same dedup_key returns the same row");

    let row_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM notifications WHERE dedup_key = $1")
            .bind("outbox-evt-1")
            .fetch_one(&owner_pool)
            .await
            .unwrap();
    assert_eq!(row_count, 1, "dedup_key never doubles a row");

    let notifier_calls_after = notifier.calls.lock().unwrap().len();
    assert_eq!(
        notifier_calls_after - notifier_calls_before,
        1,
        "the realtime notifier fires once, not on the redelivery"
    );
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn summary_is_grouped_by_category_as_runtime_role(owner_pool: PgPool) {
    let rt_pool = runtime_role_pool(&owner_pool).await;
    let knl = OrgId::knl();
    let user = seed_user(&owner_pool, *knl.as_uuid(), "Summary User").await;
    let store = PgNotificationStore::new(rt_pool.clone());

    for cat in ["결재", "결재", "공지"] {
        console_platform_request_context::scope_org(knl, async {
            store.emit_notification(emit_to(user, cat, None)).await
        })
        .await
        .expect("emit");
    }

    let summary = console_platform_request_context::scope_org(knl, async {
        store
            .summary(
                console_notifications_application::NotificationCountsSummaryQuery {
                    recipient: user,
                },
            )
            .await
    })
    .await
    .expect("summary");

    assert_eq!(summary.total_unread, 3);
    let approval = summary
        .by_category
        .iter()
        .find(|c| c.category == "결재")
        .expect("결재 present");
    assert_eq!(approval.unread, 2);
    let notice = summary
        .by_category
        .iter()
        .find(|c| c.category == "공지")
        .expect("공지 present");
    assert_eq!(notice.unread, 1);
}

/// Proves the generic detect -> assign -> resolve chain: a resolve-by-link
/// sweep marks EVERY still-open notification pointing at that link resolved,
/// across recipients, in one audited call — and never touches another org's
/// rows (RLS) or an already-resolved row.
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn resolve_by_link_closes_every_open_notification_for_that_target_as_runtime_role(
    owner_pool: PgPool,
) {
    let rt_pool = runtime_role_pool(&owner_pool).await;
    let knl = OrgId::knl();
    let other = OrgId::from_uuid(OTHER_ORG);
    seed_org(&owner_pool, OTHER_ORG, "Other").await;
    let user_a = seed_user(&owner_pool, *knl.as_uuid(), "Coverage A").await;
    let user_b = seed_user(&owner_pool, *knl.as_uuid(), "Coverage B").await;
    let store = PgNotificationStore::new(rt_pool.clone());

    let breach_link = NotificationLink::Object {
        kind: "attendance_gap".to_owned(),
        id: "shift-2026-07-10".to_owned(),
    };
    let slo_notification = |recipient: UserId| EmitNotificationCommand {
        actor: None,
        recipient,
        category: "근태".to_owned(),
        kind: "slo_violation".to_owned(),
        text: "미편성 결원이 발생했습니다".to_owned(),
        link: breach_link.clone(),
        dedup_key: None,
        trace: TraceContext::generate(),
        occurred_at: OffsetDateTime::now_utc(),
    };

    // Two people got notified of the same coverage breach; plus an unrelated
    // notification that must NOT be touched by the resolve sweep.
    let notif_a = console_platform_request_context::scope_org(knl, async {
        store.emit_notification(slo_notification(user_a)).await
    })
    .await
    .expect("emit to A");
    let notif_b = console_platform_request_context::scope_org(knl, async {
        store.emit_notification(slo_notification(user_b)).await
    })
    .await
    .expect("emit to B");
    let unrelated = console_platform_request_context::scope_org(knl, async {
        store.emit_notification(emit_to(user_a, "결재", None)).await
    })
    .await
    .expect("emit unrelated");

    // Another tenant's identical-shaped link must stay untouched (RLS).
    seed_user(&owner_pool, OTHER_ORG, "Other Tenant User").await;

    let resolved_count = console_platform_request_context::scope_org(knl, async {
        store
            .resolve_notifications_by_link(
                console_notifications_application::ResolveNotificationsByLinkCommand {
                    link: breach_link.clone(),
                    resolved_by: Some(user_b),
                    trace: TraceContext::generate(),
                    occurred_at: OffsetDateTime::now_utc(),
                },
            )
            .await
    })
    .await
    .expect("resolve by link");
    assert_eq!(
        resolved_count, 2,
        "both open notifications for the breach resolve"
    );

    let a_after = console_platform_request_context::scope_org(knl, async {
        store.list(list_unread(user_a)).await
    })
    .await
    .expect("A list after resolve");
    // Resolving does not itself mark a notification read; it's still unread
    // but now carries a resolved_at stamp.
    let a_notif = a_after
        .items
        .iter()
        .find(|n| n.id == notif_a.id)
        .expect("A's breach notification still listed");
    assert!(
        a_notif.resolved_at.is_some(),
        "A's breach notification is resolved"
    );
    let a_unrelated = a_after
        .items
        .iter()
        .find(|n| n.id == unrelated.id)
        .expect("A's unrelated notification still listed");
    assert!(
        a_unrelated.resolved_at.is_none(),
        "the unrelated notification must NOT be auto-resolved"
    );

    let b_after = console_platform_request_context::scope_org(knl, async {
        store.list(list_unread(user_b)).await
    })
    .await
    .expect("B list after resolve");
    let b_notif = b_after
        .items
        .iter()
        .find(|n| n.id == notif_b.id)
        .expect("B's breach notification still listed");
    assert!(
        b_notif.resolved_at.is_some(),
        "B's breach notification is resolved too"
    );

    // Re-resolving the same link is idempotent-friendly: nothing left open.
    let second_sweep = console_platform_request_context::scope_org(knl, async {
        store
            .resolve_notifications_by_link(
                console_notifications_application::ResolveNotificationsByLinkCommand {
                    link: breach_link.clone(),
                    resolved_by: None,
                    trace: TraceContext::generate(),
                    occurred_at: OffsetDateTime::now_utc(),
                },
            )
            .await
    })
    .await
    .expect("second resolve sweep");
    assert_eq!(second_sweep, 0, "nothing left open to resolve");

    // Cross-tenant: the other org never sees or resolves knl's rows.
    let cross_tenant_sweep = console_platform_request_context::scope_org(other, async {
        store
            .resolve_notifications_by_link(
                console_notifications_application::ResolveNotificationsByLinkCommand {
                    link: breach_link,
                    resolved_by: None,
                    trace: TraceContext::generate(),
                    occurred_at: OffsetDateTime::now_utc(),
                },
            )
            .await
    })
    .await
    .expect("cross-tenant sweep itself succeeds");
    assert_eq!(
        cross_tenant_sweep, 0,
        "another tenant's sweep resolves none of knl's notifications"
    );
}

/// The swipe toggle's reverse arc: mark_unread flips the attention flag back
/// WITHOUT clearing `read_at` (forensic first-read stamp), cross-user ids are
/// NotFound, and both directions land audit rows.
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn mark_unread_toggles_but_preserves_first_read_as_runtime_role(owner_pool: PgPool) {
    let rt_pool = runtime_role_pool(&owner_pool).await;
    let knl = OrgId::knl();
    let user_a = seed_user(&owner_pool, *knl.as_uuid(), "Toggle A").await;
    let user_b = seed_user(&owner_pool, *knl.as_uuid(), "Toggle B").await;
    let store = PgNotificationStore::new(rt_pool.clone());

    let notif = console_platform_request_context::scope_org(knl, async {
        store.emit_notification(emit_to(user_a, "결재", None)).await
    })
    .await
    .expect("emit");

    let read = console_platform_request_context::scope_org(knl, async {
        store
            .mark_read(MarkNotificationReadCommand {
                recipient: user_a,
                notification_id: notif.id,
                trace: TraceContext::generate(),
                occurred_at: OffsetDateTime::now_utc(),
            })
            .await
    })
    .await
    .expect("mark read");
    let first_read_at = read.read_at.expect("read_at set on first read");

    // Toggle back to unread: unread=true, read_at UNCHANGED.
    let unread = console_platform_request_context::scope_org(knl, async {
        store
            .mark_unread(MarkNotificationUnreadCommand {
                recipient: user_a,
                notification_id: notif.id,
                trace: TraceContext::generate(),
                occurred_at: OffsetDateTime::now_utc(),
            })
            .await
    })
    .await
    .expect("mark unread");
    assert!(unread.unread, "row is unread again");
    assert_eq!(
        unread.read_at,
        Some(first_read_at),
        "read_at stays the forensic FIRST-read timestamp"
    );

    // Re-reading keeps the original first-read stamp (COALESCE), too.
    let reread = console_platform_request_context::scope_org(knl, async {
        store
            .mark_read(MarkNotificationReadCommand {
                recipient: user_a,
                notification_id: notif.id,
                trace: TraceContext::generate(),
                occurred_at: OffsetDateTime::now_utc(),
            })
            .await
    })
    .await
    .expect("second mark read");
    assert_eq!(reread.read_at, Some(first_read_at));

    // Cross-user toggle is NotFound, indistinguishable from absent.
    let cross = console_platform_request_context::scope_org(knl, async {
        store
            .mark_unread(MarkNotificationUnreadCommand {
                recipient: user_b,
                notification_id: notif.id,
                trace: TraceContext::generate(),
                occurred_at: OffsetDateTime::now_utc(),
            })
            .await
    })
    .await;
    assert_eq!(
        cross.expect_err("B toggling A's notification fails").kind(),
        ErrorKind::NotFound
    );

    // Audit readback: the toggle transitions are on the audit trail.
    let unread_audits: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM audit_events WHERE action = 'notification.unread' AND target_id = $1",
    )
    .bind(notif.id.to_string())
    .fetch_one(&owner_pool)
    .await
    .unwrap();
    assert_eq!(unread_audits, 1, "notification.unread audited exactly once");
}

/// Mute policies route ATTENTION, never data: counts and the realtime notifier
/// honor them, the list only annotates; policy CRUD is recipient-owned and
/// audited; RLS keeps another tenant's policies invisible.
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn mute_policies_route_attention_as_runtime_role(owner_pool: PgPool) {
    let rt_pool = runtime_role_pool(&owner_pool).await;
    let knl = OrgId::knl();
    let other = OrgId::from_uuid(OTHER_ORG);
    seed_org(&owner_pool, OTHER_ORG, "Other").await;
    let user_a = seed_user(&owner_pool, *knl.as_uuid(), "Mute A").await;
    let user_b = seed_user(&owner_pool, *knl.as_uuid(), "Mute B").await;

    let notifier = Arc::new(RecordingNotifier::default());
    let store = PgNotificationStore::new(rt_pool.clone()).with_notifier(notifier.clone());

    console_platform_request_context::scope_org(knl, async {
        store.emit_notification(emit_to(user_a, "결재", None)).await
    })
    .await
    .expect("emit 결재");
    console_platform_request_context::scope_org(knl, async {
        store.emit_notification(emit_to(user_a, "멘션", None)).await
    })
    .await
    .expect("emit 멘션");
    assert_eq!(notifier.calls.lock().unwrap().len(), 2);

    // Category mute: badge counts drop, summary tallies the hidden unread.
    let policy = console_platform_request_context::scope_org(knl, async {
        store
            .upsert_policy(UpsertNotificationPolicyCommand {
                recipient: user_a,
                scope: NotificationPolicyScope::Category(
                    NotificationCategory::new("결재".to_owned()).unwrap(),
                ),
                trace: TraceContext::generate(),
                occurred_at: OffsetDateTime::now_utc(),
            })
            .await
    })
    .await
    .expect("upsert category mute");
    assert_eq!(policy.scope, "category");
    assert_eq!(policy.category.as_deref(), Some("결재"));
    assert_eq!(policy.action, "mute");

    let count = console_platform_request_context::scope_org(knl, async {
        store.unread_count(unread_count_of(user_a)).await
    })
    .await
    .expect("count under category mute");
    assert_eq!(count, 1, "muted 결재 row leaves only 멘션 in the badge");

    let summary = console_platform_request_context::scope_org(knl, async {
        store
            .summary(NotificationCountsSummaryQuery { recipient: user_a })
            .await
    })
    .await
    .expect("summary under category mute");
    assert_eq!(summary.total_unread, 1);
    assert_eq!(summary.muted_unread, 1, "hidden unread surfaced honestly");
    assert!(
        summary.by_category.iter().all(|c| c.category != "결재"),
        "an all-muted category is absent from the breakdown"
    );

    // The list is NEVER filtered — rows only get annotated.
    let listed = console_platform_request_context::scope_org(knl, async {
        store.list(list_unread(user_a)).await
    })
    .await
    .expect("list under category mute");
    assert_eq!(listed.items.len(), 2, "mute suppresses attention, not data");
    let muted_row = listed
        .items
        .iter()
        .find(|n| n.category == "결재")
        .expect("결재 row still listed");
    assert!(muted_row.muted, "muted row is annotated");
    assert!(
        !listed
            .items
            .iter()
            .find(|n| n.category == "멘션")
            .expect("멘션 row listed")
            .muted
    );

    // Emit-time routing: a muted emit persists + audits but stays silent.
    let calls_before = notifier.calls.lock().unwrap().len();
    let muted_emit = console_platform_request_context::scope_org(knl, async {
        store.emit_notification(emit_to(user_a, "결재", None)).await
    })
    .await
    .expect("muted emit persists");
    assert!(muted_emit.muted);
    assert_eq!(
        notifier.calls.lock().unwrap().len(),
        calls_before,
        "realtime notifier stays silent for a muted row"
    );
    let listed_after = console_platform_request_context::scope_org(knl, async {
        store.list(list_unread(user_a)).await
    })
    .await
    .expect("list after muted emit");
    assert_eq!(listed_after.items.len(), 3, "the muted row IS persisted");

    // PUT is an upsert: same target returns the same policy row.
    let again = console_platform_request_context::scope_org(knl, async {
        store
            .upsert_policy(UpsertNotificationPolicyCommand {
                recipient: user_a,
                scope: NotificationPolicyScope::Category(
                    NotificationCategory::new("결재".to_owned()).unwrap(),
                ),
                trace: TraceContext::generate(),
                occurred_at: OffsetDateTime::now_utc(),
            })
            .await
    })
    .await
    .expect("second upsert");
    assert_eq!(again.id, policy.id, "same target upserts the same row");

    // Recipient isolation: B sees no policies, and B deleting A's is NotFound.
    let b_policies = console_platform_request_context::scope_org(knl, async {
        store
            .list_policies(ListNotificationPoliciesQuery { recipient: user_b })
            .await
    })
    .await
    .expect("B lists policies");
    assert!(b_policies.is_empty());
    let cross_delete = console_platform_request_context::scope_org(knl, async {
        store
            .delete_policy(DeleteNotificationPolicyCommand {
                recipient: user_b,
                policy_id: policy.id,
                trace: TraceContext::generate(),
                occurred_at: OffsetDateTime::now_utc(),
            })
            .await
    })
    .await;
    assert_eq!(
        cross_delete
            .expect_err("B deleting A's policy fails")
            .kind(),
        ErrorKind::NotFound
    );

    // RLS: under another tenant's GUC the policy is invisible.
    let cross_tenant = console_platform_request_context::scope_org(other, async {
        store
            .list_policies(ListNotificationPoliciesQuery { recipient: user_a })
            .await
    })
    .await
    .expect("cross-tenant policy list succeeds");
    assert!(
        cross_tenant.is_empty(),
        "RLS hides another tenant's policies"
    );

    // Scope=all is the same mechanism (DND): every count goes quiet.
    console_platform_request_context::scope_org(knl, async {
        store
            .upsert_policy(UpsertNotificationPolicyCommand {
                recipient: user_a,
                scope: NotificationPolicyScope::All,
                trace: TraceContext::generate(),
                occurred_at: OffsetDateTime::now_utc(),
            })
            .await
    })
    .await
    .expect("upsert all-scope mute");
    let dnd_count = console_platform_request_context::scope_org(knl, async {
        store.unread_count(unread_count_of(user_a)).await
    })
    .await
    .expect("count under DND");
    assert_eq!(dnd_count, 0, "scope=all silences the whole badge");

    // Delete = unmute: category policy removal restores its rows' attention.
    console_platform_request_context::scope_org(knl, async {
        store
            .delete_policy(DeleteNotificationPolicyCommand {
                recipient: user_a,
                policy_id: policy.id,
                trace: TraceContext::generate(),
                occurred_at: OffsetDateTime::now_utc(),
            })
            .await
    })
    .await
    .expect("A deletes own policy");
    let a_policies = console_platform_request_context::scope_org(knl, async {
        store
            .list_policies(ListNotificationPoliciesQuery { recipient: user_a })
            .await
    })
    .await
    .expect("A lists after delete");
    assert_eq!(a_policies.len(), 1, "only the all-scope policy remains");

    // Audit readback for the policy lifecycle.
    let set_audits: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM audit_events WHERE action = 'notification.policy_set'",
    )
    .fetch_one(&owner_pool)
    .await
    .unwrap();
    let clear_audits: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM audit_events WHERE action = 'notification.policy_clear'",
    )
    .fetch_one(&owner_pool)
    .await
    .unwrap();
    assert_eq!(
        set_audits, 3,
        "each upsert (incl. idempotent re-set) audited"
    );
    assert_eq!(clear_audits, 1, "the successful delete audited");
}

/// 개체별 view: one group per distinct link with totals, category breakdown,
/// latest preview and the caller's object-mute state; keyset-paginated behind
/// an opaque fail-closed cursor; recipient- and tenant-isolated.
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn object_groups_aggregate_by_link_as_runtime_role(owner_pool: PgPool) {
    let rt_pool = runtime_role_pool(&owner_pool).await;
    let knl = OrgId::knl();
    let other = OrgId::from_uuid(OTHER_ORG);
    seed_org(&owner_pool, OTHER_ORG, "Other").await;
    let user_a = seed_user(&owner_pool, *knl.as_uuid(), "Group A").await;
    let user_b = seed_user(&owner_pool, *knl.as_uuid(), "Group B").await;
    let store = PgNotificationStore::new(rt_pool.clone());

    let approval_link = NotificationLink::Object {
        kind: "approval".to_owned(),
        id: "ap-2026-001".to_owned(),
    };
    let workorder_link = NotificationLink::Object {
        kind: "workorder".to_owned(),
        id: "wo-2026-009".to_owned(),
    };
    let emit_link =
        |recipient: UserId, category: &str, link: &NotificationLink| EmitNotificationCommand {
            actor: None,
            recipient,
            category: category.to_owned(),
            kind: "info".to_owned(),
            text: "결재 문서가 도착했습니다".to_owned(),
            link: link.clone(),
            dedup_key: None,
            trace: TraceContext::generate(),
            occurred_at: OffsetDateTime::now_utc(),
        };

    // A: two rows on the approval, then one newer row on the workorder.
    // B: one row on the SAME approval link (must never leak into A's groups).
    console_platform_request_context::scope_org(knl, async {
        store
            .emit_notification(emit_link(user_a, "결재", &approval_link))
            .await
    })
    .await
    .expect("emit A approval #1");
    let approval_latest = console_platform_request_context::scope_org(knl, async {
        store
            .emit_notification(emit_link(user_a, "멘션", &approval_link))
            .await
    })
    .await
    .expect("emit A approval #2");
    console_platform_request_context::scope_org(knl, async {
        store
            .emit_notification(emit_link(user_a, "근태", &workorder_link))
            .await
    })
    .await
    .expect("emit A workorder");
    console_platform_request_context::scope_org(knl, async {
        store
            .emit_notification(emit_link(user_b, "결재", &approval_link))
            .await
    })
    .await
    .expect("emit B approval");

    let groups_of = |recipient: UserId, unread_only: bool, before: Option<String>, limit: i64| {
        ListNotificationObjectGroupsQuery {
            recipient,
            unread_only,
            before,
            limit,
        }
    };

    let page = console_platform_request_context::scope_org(knl, async {
        store
            .list_object_groups(groups_of(user_a, false, None, 50))
            .await
    })
    .await
    .expect("A groups");
    assert_eq!(page.items.len(), 2, "one group per distinct link");
    assert!(page.next_cursor.is_none());
    // Newest activity first: the workorder row was emitted last.
    assert_eq!(page.items[0].link, workorder_link);
    let approval_group = &page.items[1];
    assert_eq!(approval_group.link, approval_link);
    assert_eq!(
        approval_group.total, 2,
        "B's row on the same link never counts into A's group"
    );
    assert_eq!(approval_group.unread, 2);
    assert_eq!(approval_group.latest.id, approval_latest.id);
    assert!(!approval_group.muted);
    let mut cats: Vec<(&str, i64)> = approval_group
        .categories
        .iter()
        .map(|c| (c.category.as_str(), c.unread))
        .collect();
    cats.sort_unstable();
    assert_eq!(cats, vec![("결재", 1), ("멘션", 1)]);

    // Reading a row updates the group's unread + category breakdown.
    console_platform_request_context::scope_org(knl, async {
        store
            .mark_read(MarkNotificationReadCommand {
                recipient: user_a,
                notification_id: approval_latest.id,
                trace: TraceContext::generate(),
                occurred_at: OffsetDateTime::now_utc(),
            })
            .await
    })
    .await
    .expect("read latest approval row");
    let after_read = console_platform_request_context::scope_org(knl, async {
        store
            .list_object_groups(groups_of(user_a, false, None, 50))
            .await
    })
    .await
    .expect("groups after read");
    let approval_after = after_read
        .items
        .iter()
        .find(|g| g.link == approval_link)
        .expect("approval group still present");
    assert_eq!(approval_after.total, 2);
    assert_eq!(approval_after.unread, 1);
    assert_eq!(
        approval_after.categories.len(),
        1,
        "read category drops out"
    );
    assert_eq!(approval_after.categories[0].category, "결재");

    // unread_only: a fully-read group disappears from the filtered view.
    console_platform_request_context::scope_org(knl, async {
        store
            .mark_all_read(MarkAllNotificationsReadCommand {
                recipient: user_a,
                trace: TraceContext::generate(),
                occurred_at: OffsetDateTime::now_utc(),
            })
            .await
    })
    .await
    .expect("read everything");
    let unread_view = console_platform_request_context::scope_org(knl, async {
        store
            .list_object_groups(groups_of(user_a, true, None, 50))
            .await
    })
    .await
    .expect("unread-only view");
    assert!(unread_view.items.is_empty(), "no unread => no groups");

    // Object-mute flips the group's bell; a category policy must NOT (the
    // bell could never un-toggle it), though rows still annotate.
    console_platform_request_context::scope_org(knl, async {
        store
            .upsert_policy(UpsertNotificationPolicyCommand {
                recipient: user_a,
                scope: NotificationPolicyScope::Object(approval_link.clone()),
                trace: TraceContext::generate(),
                occurred_at: OffsetDateTime::now_utc(),
            })
            .await
    })
    .await
    .expect("mute the approval object");
    console_platform_request_context::scope_org(knl, async {
        store
            .upsert_policy(UpsertNotificationPolicyCommand {
                recipient: user_a,
                scope: NotificationPolicyScope::Category(
                    NotificationCategory::new("근태".to_owned()).unwrap(),
                ),
                trace: TraceContext::generate(),
                occurred_at: OffsetDateTime::now_utc(),
            })
            .await
    })
    .await
    .expect("mute the 근태 category");
    let muted_view = console_platform_request_context::scope_org(knl, async {
        store
            .list_object_groups(groups_of(user_a, false, None, 50))
            .await
    })
    .await
    .expect("groups under policies");
    let approval_muted = muted_view
        .items
        .iter()
        .find(|g| g.link == approval_link)
        .unwrap();
    assert!(approval_muted.muted, "object policy mutes its group bell");
    let workorder_muted = muted_view
        .items
        .iter()
        .find(|g| g.link == workorder_link)
        .unwrap();
    assert!(
        !workorder_muted.muted,
        "a category policy never flips the group bell"
    );
    assert!(
        workorder_muted.latest.muted,
        "…but the row itself is annotated muted"
    );

    // Keyset pagination behind the opaque cursor.
    let first = console_platform_request_context::scope_org(knl, async {
        store
            .list_object_groups(groups_of(user_a, false, None, 1))
            .await
    })
    .await
    .expect("page 1");
    assert_eq!(first.items.len(), 1);
    assert_eq!(first.items[0].link, workorder_link);
    let cursor = first.next_cursor.expect("full page carries a cursor");
    let second = console_platform_request_context::scope_org(knl, async {
        store
            .list_object_groups(groups_of(user_a, false, Some(cursor), 1))
            .await
    })
    .await
    .expect("page 2");
    assert_eq!(second.items.len(), 1);
    assert_eq!(second.items[0].link, approval_link);

    // A cursor this server never issued fails CLOSED: empty page, no error.
    let forged = console_platform_request_context::scope_org(knl, async {
        store
            .list_object_groups(groups_of(
                user_a,
                false,
                Some("never-issued-cursor".to_owned()),
                50,
            ))
            .await
    })
    .await
    .expect("forged cursor still 200s");
    assert!(forged.items.is_empty());
    assert!(forged.next_cursor.is_none());

    // Cross-tenant: another org's GUC sees no groups at all.
    let cross_tenant = console_platform_request_context::scope_org(other, async {
        store
            .list_object_groups(groups_of(user_a, false, None, 50))
            .await
    })
    .await
    .expect("cross-tenant groups succeed");
    assert!(cross_tenant.items.is_empty());
}
