#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::{Arc, Mutex};

use console_kernel_core::{
    BranchId, BranchScope, ErrorKind, OrgId, ThreadId, TraceContext, UserId, WorkOrderId,
};
use console_messenger_adapter_postgres::PgMessengerStore;
use console_messenger_application::{
    CreateThreadCommand, EnsureWorkOrderThreadCommand, JoinThreadCommand, ListChannelsQuery,
    ListThreadsQuery, MarkThreadReadCommand, MemberProfileQuery, MessageNotifier,
    MessageNotifyFuture, MessagePageQuery, MessagePostedNotification, SearchMessagesQuery,
    SendMessageCommand, SetThreadMuteCommand, ThreadPresenceQuery, ToggleAckCommand,
};
use console_messenger_domain::{PresenceStatus, ThreadKind, ThreadVisibility};
use sqlx::{PgPool, Row};
use time::{Duration, OffsetDateTime};

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn message_send_persists_audit_before_post_commit_notify(pool: PgPool) {
    console_platform_request_context::scope_org(console_kernel_core::OrgId::knl(), async move {
        let seeded = seed_context(&pool).await;
        let notifier = Arc::new(RecordingNotifier::new(pool.clone()));
        let store = PgMessengerStore::new(pool.clone()).with_notifier(notifier.clone());
        let thread = create_team_thread(&store, &seeded).await;

        let message = store
            .send_message(SendMessageCommand {
                actor: seeded.sender,
                branch_scope: BranchScope::single(seeded.branch),
                thread_id: thread.id,
                body: "지게차 누유 사진 확인했습니다".to_owned(),
                attachment_evidence_ids: Vec::new(),
                quoted_message_id: None,
                trace: TraceContext::generate(),
                occurred_at: OffsetDateTime::now_utc(),
            })
            .await
            .unwrap();

        assert_eq!(message.thread_id, thread.id);
        assert_eq!(message.sender_id, seeded.sender);
        // The same-org LEFT JOIN on users resolves the sender's display name
        // (seed_user stamps "Sender <uuid>") — no raw-UUID leak to the client.
        assert!(
            message
                .sender_name
                .as_deref()
                .is_some_and(|name| name.starts_with("Sender")),
            "expected sender_name to resolve, got {:?}",
            message.sender_name
        );
        assert_eq!(message.body, "지게차 누유 사진 확인했습니다");

        let calls = notifier.calls.lock().unwrap().clone();
        assert_eq!(
            calls,
            vec![MessagePostedNotification {
                message_id: message.id,
                thread_id: thread.id,
                branch_id: seeded.branch,
            }]
        );
    })
    .await;
}

// AC (UI-M2a): in a messenger input an `@`-mention creates a notification-center
// row for its target; a `#` object-link does not (DESIGN §4.7-7). Deny-by-
// omission: mentioning a non-member notifies no one. Wires the real #198
// NotificationSink, so the assertion is on actual rows, not a discarded field.
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn at_mention_emits_notification_for_thread_member_only(pool: PgPool) {
    console_platform_request_context::scope_org(console_kernel_core::OrgId::knl(), async move {
        let seeded = seed_context(&pool).await;
        let sink = Arc::new(
            console_notifications_adapter_postgres::PgNotificationStore::new(pool.clone()),
        );
        let store = PgMessengerStore::new(pool.clone()).with_notification_sink(sink);
        // Thread members: sender + recipient. receptionist is NOT a member.
        let thread = create_team_thread(&store, &seeded).await;
        let t0 = OffsetDateTime::now_utc();

        // 1. `@`-mention of a thread member → one notification row for that member.
        send_at(
            &store,
            &seeded,
            thread.id,
            &format!("@{} 지게차 점검 부탁드립니다", seeded.recipient),
            t0,
        )
        .await;
        // 2. `#` object-link only (no mention) → no mention notification.
        send_at(
            &store,
            &seeded,
            thread.id,
            "#WO-20260612-001 관련 자료 첨부합니다",
            t0 + Duration::seconds(1),
        )
        .await;
        // 3. `@`-mention of a NON-member → deny-by-omission, no notification.
        send_at(
            &store,
            &seeded,
            thread.id,
            &format!("@{} 확인 바랍니다", seeded.receptionist),
            t0 + Duration::seconds(2),
        )
        .await;
        // 4. `@`-mention of self → self-filter, no notification.
        send_at(
            &store,
            &seeded,
            thread.id,
            &format!("@{} 자기 점검 기록입니다", seeded.sender),
            t0 + Duration::seconds(3),
        )
        .await;

        assert_eq!(
            notification_count(&pool, seeded.recipient).await,
            1,
            "@-mention of a thread member emits exactly one notification",
        );
        assert_eq!(
            notification_count(&pool, seeded.receptionist).await,
            0,
            "a non-member mention emits nothing (deny-by-omission)",
        );
        assert_eq!(
            notification_count(&pool, seeded.sender).await,
            0,
            "the sender never notifies itself",
        );

        // Stable dedup key `msg-mention:{message_id}:{recipient}` for at-most-once.
        let key: String =
            sqlx::query_scalar("SELECT dedup_key FROM notifications WHERE recipient_user_id = $1")
                .bind(*seeded.recipient.as_uuid())
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(
            key.starts_with("msg-mention:") && key.ends_with(&format!(":{}", seeded.recipient)),
            "unexpected dedup key {key}",
        );

        // Thread unread still reflects the ordinary fan-out (all 4 posts).
        let threads = store
            .list_threads(ListThreadsQuery {
                actor: seeded.recipient,
                branch_scope: BranchScope::single(seeded.branch),
                limit: 10,
            })
            .await
            .unwrap();
        assert_eq!(
            threads
                .iter()
                .find(|t| t.id == thread.id)
                .expect("recipient sees the thread")
                .unread_count,
            4,
        );
    })
    .await;
}

async fn notification_count(pool: &PgPool, recipient: UserId) -> i64 {
    sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM notifications WHERE recipient_user_id = $1")
        .bind(*recipient.as_uuid())
        .fetch_one(pool)
        .await
        .unwrap()
}

// BE-OBJ slice 2, item 2: `#`-object-code tokens persist as message_refs on
// write (parse-on-write), feeding the object's inbound reference chain / graph
// traversal. Only a token whose prefix matches a seeded object_types
// code_prefix is stored — `#hashtag` noise with no known prefix is dropped.
// `#`-refs never notify (DESIGN §4.7-7), unlike `@`-mentions.
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn object_code_ref_persisted_on_write_and_hashtag_noise_dropped(pool: PgPool) {
    console_platform_request_context::scope_org(console_kernel_core::OrgId::knl(), async move {
        let seeded = seed_context(&pool).await;
        let store = PgMessengerStore::new(pool.clone());
        let thread = create_team_thread(&store, &seeded).await;

        // CS- (support_ticket) is a seeded code_prefix; WO- deliberately is
        // NOT (work_order keeps its own date-based request_no scheme, not
        // this issuance table — see the object_code_counters migration's
        // LOW-fix rationale).
        let message = send_at(
            &store,
            &seeded,
            thread.id,
            "#CS-3121 관련 자료 첨부, #hashtag 는 코드가 아닙니다, #WO-20260612-001 도 아닙니다",
            OffsetDateTime::now_utc(),
        )
        .await;

        let refs: Vec<(String, String)> = sqlx::query_as(
            "SELECT ref_kind, ref_code FROM message_refs WHERE message_id = $1 ORDER BY ref_code",
        )
        .bind(*message.id.as_uuid())
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(
            refs,
            vec![("support_ticket".to_owned(), "CS-3121".to_owned())],
            "only the known-prefix code is persisted; #hashtag noise and the \
             unregistered WO- prefix are both dropped"
        );
    })
    .await;
}

// AC (UI-M2a): opening a person chip pins a summary from a real branch-scoped
// API and records a `person.view` audit for a non-self view (DESIGN §4.7 "열람
// — 기록 남음"); a self-view records none; a target outside the branch yields
// not_found with no audit (deny-by-omission).
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn member_profile_records_person_view_audit_for_non_self_only(pool: PgPool) {
    console_platform_request_context::scope_org(console_kernel_core::OrgId::knl(), async move {
        let seeded = seed_context(&pool).await;
        let isolated_branch = seed_branch(&pool, "Isolated Branch").await;
        let outsider = seed_user(&pool, "Outsider", "MECHANIC", isolated_branch).await;
        let store = PgMessengerStore::new(pool.clone());

        // Non-self view of a branch coworker → summary + one person.view audit.
        let profile = store
            .member_profile(MemberProfileQuery {
                actor: seeded.sender,
                branch_scope: BranchScope::single(seeded.branch),
                branch_id: seeded.branch,
                user_id: seeded.recipient,
                trace: TraceContext::generate(),
                occurred_at: OffsetDateTime::now_utc(),
            })
            .await
            .unwrap();
        assert_eq!(profile.id, seeded.recipient);
        assert_eq!(person_view_audit_count(&pool, seeded.recipient).await, 1);

        // Self-view → summary, but NO audit event.
        let own = store
            .member_profile(MemberProfileQuery {
                actor: seeded.sender,
                branch_scope: BranchScope::single(seeded.branch),
                branch_id: seeded.branch,
                user_id: seeded.sender,
                trace: TraceContext::generate(),
                occurred_at: OffsetDateTime::now_utc(),
            })
            .await
            .unwrap();
        assert_eq!(own.id, seeded.sender);
        assert_eq!(person_view_audit_count(&pool, seeded.sender).await, 0);

        // A target outside the actor's branch → not_found, no audit trail.
        let denied = store
            .member_profile(MemberProfileQuery {
                actor: seeded.sender,
                branch_scope: BranchScope::single(seeded.branch),
                branch_id: seeded.branch,
                user_id: outsider,
                trace: TraceContext::generate(),
                occurred_at: OffsetDateTime::now_utc(),
            })
            .await
            .unwrap_err();
        assert_eq!(denied.kind(), ErrorKind::NotFound);
        assert_eq!(person_view_audit_count(&pool, outsider).await, 0);
    })
    .await;
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn work_order_thread_auto_create_is_idempotent_and_members_actor(pool: PgPool) {
    console_platform_request_context::scope_org(console_kernel_core::OrgId::knl(), async move {
        let seeded = seed_context(&pool).await;
        let work_order_id = seed_work_order(&pool, &seeded).await;
        let store = PgMessengerStore::new(pool.clone());

        let first = store
            .ensure_work_order_thread(EnsureWorkOrderThreadCommand {
                actor: seeded.sender,
                branch_id: seeded.branch,
                work_order_id,
                trace: TraceContext::generate(),
                occurred_at: OffsetDateTime::now_utc(),
            })
            .await
            .unwrap();
        let second = store
            .ensure_work_order_thread(EnsureWorkOrderThreadCommand {
                actor: seeded.sender,
                branch_id: seeded.branch,
                work_order_id,
                trace: TraceContext::generate(),
                occurred_at: OffsetDateTime::now_utc(),
            })
            .await
            .unwrap();

        assert_eq!(first.id, second.id);
        assert_eq!(first.kind, ThreadKind::WorkOrder);
        assert_eq!(first.work_order_id, Some(work_order_id));

        let thread_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM messenger_threads WHERE work_order_id = $1")
                .bind(*work_order_id.as_uuid())
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(thread_count, 1);

        let member_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM messenger_thread_members WHERE thread_id = $1 AND user_id = $2",
        )
        .bind(*first.id.as_uuid())
        .bind(*seeded.sender.as_uuid())
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(member_count, 1);
    })
    .await;
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn membership_and_branch_scope_are_default_deny(pool: PgPool) {
    console_platform_request_context::scope_org(console_kernel_core::OrgId::knl(), async move {
        let seeded = seed_context(&pool).await;
        let store = PgMessengerStore::new(pool.clone());
        let thread = create_team_thread(&store, &seeded).await;

        let outsider = seed_user(&pool, "Outsider", "MECHANIC", seeded.branch).await;
        let not_member = store
            .send_message(SendMessageCommand {
                actor: outsider,
                branch_scope: BranchScope::single(seeded.branch),
                thread_id: thread.id,
                body: "should not land".to_owned(),
                attachment_evidence_ids: Vec::new(),
                quoted_message_id: None,
                trace: TraceContext::generate(),
                occurred_at: OffsetDateTime::now_utc(),
            })
            .await
            .unwrap_err();
        assert_eq!(not_member.kind(), ErrorKind::Forbidden);

        let wrong_scope = store
            .send_message(SendMessageCommand {
                actor: seeded.sender,
                branch_scope: BranchScope::single(seeded.other_branch),
                thread_id: thread.id,
                body: "wrong branch scope".to_owned(),
                attachment_evidence_ids: Vec::new(),
                quoted_message_id: None,
                trace: TraceContext::generate(),
                occurred_at: OffsetDateTime::now_utc(),
            })
            .await
            .unwrap_err();
        assert_eq!(wrong_scope.kind(), ErrorKind::Forbidden);

        let visible = store
            .list_threads(ListThreadsQuery {
                actor: seeded.sender,
                branch_scope: BranchScope::single(seeded.other_branch),
                limit: 20,
            })
            .await
            .unwrap();
        assert!(visible.is_empty());
    })
    .await;
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn fts_search_returns_korean_message_hits(pool: PgPool) {
    console_platform_request_context::scope_org(console_kernel_core::OrgId::knl(), async move {
        let seeded = seed_context(&pool).await;
        let store = PgMessengerStore::new(pool.clone());
        let thread = create_team_thread(&store, &seeded).await;
        let message = send_at(
            &store,
            &seeded,
            thread.id,
            "긴급 지게차 누유 점검 필요",
            OffsetDateTime::now_utc(),
        )
        .await;

        let hits = store
            .search_messages(SearchMessagesQuery {
                actor: seeded.sender,
                branch_scope: BranchScope::single(seeded.branch),
                query: "누유".to_owned(),
                limit: 10,
            })
            .await
            .unwrap();

        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, message.id);
    })
    .await;
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn cursor_pagination_is_stable_when_newer_messages_arrive(pool: PgPool) {
    console_platform_request_context::scope_org(console_kernel_core::OrgId::knl(), async move {
        let seeded = seed_context(&pool).await;
        let store = PgMessengerStore::new(pool.clone());
        let thread = create_team_thread(&store, &seeded).await;
        let base = OffsetDateTime::now_utc();
        let first = send_at(&store, &seeded, thread.id, "first", base).await;
        let second = send_at(
            &store,
            &seeded,
            thread.id,
            "second",
            base + Duration::seconds(1),
        )
        .await;
        let third = send_at(
            &store,
            &seeded,
            thread.id,
            "third",
            base + Duration::seconds(2),
        )
        .await;

        let first_page = store
            .message_page(MessagePageQuery {
                actor: seeded.sender,
                branch_scope: BranchScope::single(seeded.branch),
                thread_id: thread.id,
                before_message_id: None,
                limit: 2,
            })
            .await
            .unwrap();

        assert_eq!(
            first_page
                .items
                .iter()
                .map(|message| message.id)
                .collect::<Vec<_>>(),
            vec![third.id, second.id]
        );
        assert_eq!(first_page.next_cursor, Some(second.id));

        let _newer = send_at(
            &store,
            &seeded,
            thread.id,
            "newer",
            base + Duration::seconds(3),
        )
        .await;

        let second_page = store
            .message_page(MessagePageQuery {
                actor: seeded.sender,
                branch_scope: BranchScope::single(seeded.branch),
                thread_id: thread.id,
                before_message_id: first_page.next_cursor,
                limit: 2,
            })
            .await
            .unwrap();

        assert_eq!(second_page.items.len(), 1);
        assert_eq!(second_page.items[0].id, first.id);
        assert_eq!(second_page.next_cursor, None);
    })
    .await;
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn list_threads_reports_unread_incoming_messages_for_actor(pool: PgPool) {
    console_platform_request_context::scope_org(console_kernel_core::OrgId::knl(), async move {
        let seeded = seed_context(&pool).await;
        let store = PgMessengerStore::new(pool.clone());
        let thread = create_team_thread(&store, &seeded).await;
        let base = OffsetDateTime::now_utc();
        let _own = send_at(&store, &seeded, thread.id, "own setup", base).await;
        let incoming = send_from(
            &store,
            seeded.recipient,
            seeded.branch,
            thread.id,
            "incoming approval note",
            base + Duration::seconds(1),
        )
        .await;

        let visible = store
            .list_threads(ListThreadsQuery {
                actor: seeded.sender,
                branch_scope: BranchScope::single(seeded.branch),
                limit: 20,
            })
            .await
            .unwrap();
        let summary = visible.iter().find(|item| item.id == thread.id).unwrap();
        assert_eq!(summary.unread_count, 1);

        store
            .mark_thread_read(MarkThreadReadCommand {
                actor: seeded.sender,
                branch_scope: BranchScope::single(seeded.branch),
                thread_id: thread.id,
                last_read_message_id: incoming.id,
                trace: TraceContext::generate(),
                occurred_at: base + Duration::seconds(2),
            })
            .await
            .unwrap();

        let after_read = store
            .list_threads(ListThreadsQuery {
                actor: seeded.sender,
                branch_scope: BranchScope::single(seeded.branch),
                limit: 20,
            })
            .await
            .unwrap();
        let summary = after_read.iter().find(|item| item.id == thread.id).unwrap();
        assert_eq!(summary.unread_count, 0);
    })
    .await;
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn read_receipt_never_moves_back_to_an_older_message(pool: PgPool) {
    console_platform_request_context::scope_org(console_kernel_core::OrgId::knl(), async move {
        let seeded = seed_context(&pool).await;
        let store = PgMessengerStore::new(pool.clone());
        let thread = create_team_thread(&store, &seeded).await;
        let base = OffsetDateTime::now_utc();
        let first = send_from(
            &store,
            seeded.recipient,
            seeded.branch,
            thread.id,
            "older incoming",
            base,
        )
        .await;
        let second = send_from(
            &store,
            seeded.recipient,
            seeded.branch,
            thread.id,
            "newer incoming",
            base + Duration::seconds(1),
        )
        .await;

        let latest_receipt = store
            .mark_thread_read(MarkThreadReadCommand {
                actor: seeded.sender,
                branch_scope: BranchScope::single(seeded.branch),
                thread_id: thread.id,
                last_read_message_id: second.id,
                trace: TraceContext::generate(),
                occurred_at: base + Duration::seconds(2),
            })
            .await
            .unwrap();
        assert_eq!(latest_receipt.last_read_message_id, second.id);

        let stale_receipt = store
            .mark_thread_read(MarkThreadReadCommand {
                actor: seeded.sender,
                branch_scope: BranchScope::single(seeded.branch),
                thread_id: thread.id,
                last_read_message_id: first.id,
                trace: TraceContext::generate(),
                occurred_at: base + Duration::seconds(3),
            })
            .await
            .unwrap();
        assert_eq!(stale_receipt.last_read_message_id, second.id);

        let after_stale_read = store
            .list_threads(ListThreadsQuery {
                actor: seeded.sender,
                branch_scope: BranchScope::single(seeded.branch),
                limit: 20,
            })
            .await
            .unwrap();
        let summary = after_stale_read
            .iter()
            .find(|item| item.id == thread.id)
            .unwrap();
        assert_eq!(summary.unread_count, 0);
    })
    .await;
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn read_receipt_coalesces_to_latest_message_and_audits_once(pool: PgPool) {
    console_platform_request_context::scope_org(console_kernel_core::OrgId::knl(), async move {
        let seeded = seed_context(&pool).await;
        let store = PgMessengerStore::new(pool.clone());
        let thread = create_team_thread(&store, &seeded).await;
        let base = OffsetDateTime::now_utc();
        let _first = send_at(&store, &seeded, thread.id, "first read", base).await;
        let second = send_at(
            &store,
            &seeded,
            thread.id,
            "second read",
            base + Duration::seconds(1),
        )
        .await;

        let receipt = store
            .mark_thread_read(MarkThreadReadCommand {
                actor: seeded.sender,
                branch_scope: BranchScope::single(seeded.branch),
                thread_id: thread.id,
                last_read_message_id: second.id,
                trace: TraceContext::generate(),
                occurred_at: base + Duration::seconds(2),
            })
            .await
            .unwrap();

        assert_eq!(receipt.last_read_message_id, second.id);
        let audit_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM audit_events WHERE action = 'message.read'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(audit_count, 1);
    })
    .await;
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn message_page_reports_non_sender_read_progress(pool: PgPool) {
    console_platform_request_context::scope_org(console_kernel_core::OrgId::knl(), async move {
        let seeded = seed_context(&pool).await;
        let store = PgMessengerStore::new(pool.clone());
        let thread = create_team_thread(&store, &seeded).await;
        let base = OffsetDateTime::now_utc();
        let sent = send_at(&store, &seeded, thread.id, "read progress", base).await;

        let before_read = store
            .message_page(MessagePageQuery {
                actor: seeded.sender,
                branch_scope: BranchScope::single(seeded.branch),
                thread_id: thread.id,
                before_message_id: None,
                limit: 20,
            })
            .await
            .unwrap();
        let message = before_read
            .items
            .iter()
            .find(|item| item.id == sent.id)
            .unwrap();
        assert_eq!(message.read_count, 0);
        assert_eq!(message.read_target_count, 1);

        store
            .mark_thread_read(MarkThreadReadCommand {
                actor: seeded.recipient,
                branch_scope: BranchScope::single(seeded.branch),
                thread_id: thread.id,
                last_read_message_id: sent.id,
                trace: TraceContext::generate(),
                occurred_at: base + Duration::seconds(1),
            })
            .await
            .unwrap();

        let after_read = store
            .message_page(MessagePageQuery {
                actor: seeded.sender,
                branch_scope: BranchScope::single(seeded.branch),
                thread_id: thread.id,
                before_message_id: None,
                limit: 20,
            })
            .await
            .unwrap();
        let message = after_read
            .items
            .iter()
            .find(|item| item.id == sent.id)
            .unwrap();
        assert_eq!(message.read_count, 1);
        assert_eq!(message.read_target_count, 1);
    })
    .await;
}

// Capability 1: thread taxonomy + joinable channels. A named team thread is a
// channel (discoverable + joinable in-branch); a DM is direct (fixed members,
// not joinable). Deny-by-omission: joining outside branch scope is not_found.
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn channel_is_discoverable_and_joinable_but_dm_is_not(pool: PgPool) {
    console_platform_request_context::scope_org(console_kernel_core::OrgId::knl(), async move {
        let seeded = seed_context(&pool).await;
        let store = PgMessengerStore::new(pool.clone());
        let channel = create_team_thread(&store, &seeded).await;
        assert_eq!(channel.visibility, ThreadVisibility::Channel);

        // A DM between sender and recipient is direct.
        let dm = store
            .create_thread(CreateThreadCommand {
                actor: seeded.sender,
                branch_scope: BranchScope::single(seeded.branch),
                branch_id: seeded.branch,
                kind: ThreadKind::Dm,
                visibility: None,
                title: None,
                work_order_id: None,
                member_ids: vec![seeded.sender, seeded.recipient],
                trace: TraceContext::generate(),
                occurred_at: OffsetDateTime::now_utc(),
            })
            .await
            .unwrap();
        assert_eq!(dm.visibility, ThreadVisibility::Direct);

        // The receptionist is not a member of the channel, but can discover it.
        let discovered = store
            .list_channels(ListChannelsQuery {
                actor: seeded.receptionist,
                branch_scope: BranchScope::single(seeded.branch),
                limit: 20,
            })
            .await
            .unwrap();
        assert!(discovered.iter().any(|t| t.id == channel.id));
        assert!(
            discovered
                .iter()
                .all(|t| t.visibility == ThreadVisibility::Channel)
        );

        // ...and join it.
        let joined = store
            .join_thread(JoinThreadCommand {
                actor: seeded.receptionist,
                branch_scope: BranchScope::single(seeded.branch),
                thread_id: channel.id,
                trace: TraceContext::generate(),
                occurred_at: OffsetDateTime::now_utc(),
            })
            .await
            .unwrap();
        assert_eq!(joined.member_count, 3);
        // After joining, the receptionist may post.
        send_from(
            &store,
            seeded.receptionist,
            seeded.branch,
            channel.id,
            "합류했습니다",
            OffsetDateTime::now_utc(),
        )
        .await;

        // A DM is not joinable.
        let dm_join = store
            .join_thread(JoinThreadCommand {
                actor: seeded.receptionist,
                branch_scope: BranchScope::single(seeded.branch),
                thread_id: dm.id,
                trace: TraceContext::generate(),
                occurred_at: OffsetDateTime::now_utc(),
            })
            .await
            .unwrap_err();
        assert_eq!(dm_join.kind(), ErrorKind::Forbidden);

        // Joining outside branch scope is hidden (deny-by-omission).
        let out_of_scope = store
            .join_thread(JoinThreadCommand {
                actor: seeded.receptionist,
                branch_scope: BranchScope::single(seeded.other_branch),
                thread_id: channel.id,
                trace: TraceContext::generate(),
                occurred_at: OffsetDateTime::now_utc(),
            })
            .await
            .unwrap_err();
        assert_eq!(out_of_scope.kind(), ErrorKind::NotFound);
    })
    .await;
}

// Capability 3: ack toggle is idempotent, counts live, and a non-member cannot
// ack a thread they are not in.
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn ack_toggle_is_idempotent_and_member_only(pool: PgPool) {
    console_platform_request_context::scope_org(console_kernel_core::OrgId::knl(), async move {
        let seeded = seed_context(&pool).await;
        let notifier = Arc::new(RecordingNotifier::new(pool.clone()));
        let store = PgMessengerStore::new(pool.clone()).with_notifier(notifier.clone());
        let thread = create_team_thread(&store, &seeded).await;
        let message = send_at(
            &store,
            &seeded,
            thread.id,
            "확인 부탁",
            OffsetDateTime::now_utc(),
        )
        .await;

        // Toggle on.
        let on = store
            .toggle_ack(ToggleAckCommand {
                actor: seeded.recipient,
                branch_scope: BranchScope::single(seeded.branch),
                message_id: message.id,
                trace: TraceContext::generate(),
                occurred_at: OffsetDateTime::now_utc(),
            })
            .await
            .unwrap();
        assert!(on.acked);
        assert_eq!(on.ack_count, 1);

        // The acker's page read reflects ack_count + acked_by_me.
        let page = store
            .message_page(MessagePageQuery {
                actor: seeded.recipient,
                branch_scope: BranchScope::single(seeded.branch),
                thread_id: thread.id,
                before_message_id: None,
                limit: 10,
            })
            .await
            .unwrap();
        let seen = page.items.iter().find(|m| m.id == message.id).unwrap();
        assert_eq!(seen.ack_count, 1);
        assert!(seen.acked_by_me);

        // Toggle off (idempotent inverse).
        let off = store
            .toggle_ack(ToggleAckCommand {
                actor: seeded.recipient,
                branch_scope: BranchScope::single(seeded.branch),
                message_id: message.id,
                trace: TraceContext::generate(),
                occurred_at: OffsetDateTime::now_utc(),
            })
            .await
            .unwrap();
        assert!(!off.acked);
        assert_eq!(off.ack_count, 0);

        // A non-member cannot ack.
        let denied = store
            .toggle_ack(ToggleAckCommand {
                actor: seeded.receptionist,
                branch_scope: BranchScope::single(seeded.branch),
                message_id: message.id,
                trace: TraceContext::generate(),
                occurred_at: OffsetDateTime::now_utc(),
            })
            .await
            .unwrap_err();
        assert_eq!(denied.kind(), ErrorKind::Forbidden);

        // Each successful toggle audited + fanned a realtime ack event (2 toggles).
        assert_eq!(notifier.ack_calls.lock().unwrap().len(), 2);
        assert_eq!(
            ack_audit_count(&pool, message.id).await,
            2,
            "each successful ack toggle writes a message.ack audit event"
        );
    })
    .await;
}

// Double-tap / two-tab race: a second toggle-on whose own DELETE also
// observed "not yet acked" (it started before the first toggle committed)
// reaches the INSERT after the winner's row already exists. Before the ON
// CONFLICT DO NOTHING fix this raw duplicate INSERT hit the (message_id,
// user_id) primary key and the request 500'd instead of landing idempotently.
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn racing_duplicate_ack_insert_is_absorbed_not_a_500(pool: PgPool) {
    console_platform_request_context::scope_org(console_kernel_core::OrgId::knl(), async move {
        let seeded = seed_context(&pool).await;
        let store = PgMessengerStore::new(pool.clone());
        let thread = create_team_thread(&store, &seeded).await;
        let message = send_at(
            &store,
            &seeded,
            thread.id,
            "동시 확인",
            OffsetDateTime::now_utc(),
        )
        .await;

        // Force both production `toggle_ack` calls onto the insert branch: the
        // first insert pauses before writing, letting the second insert commit;
        // the first then resumes and must hit ON CONFLICT DO NOTHING instead of
        // surfacing a duplicate-key 500.
        sqlx::query("CREATE SEQUENCE messenger_ack_race_seq")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            r#"
            CREATE FUNCTION messenger_ack_race_pause() RETURNS trigger
            LANGUAGE plpgsql
            AS $$
            BEGIN
                IF nextval('messenger_ack_race_seq') = 1 THEN
                    PERFORM pg_sleep(0.25);
                END IF;
                RETURN NEW;
            END;
            $$
            "#,
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            r#"
            CREATE TRIGGER messenger_ack_race_pause
            BEFORE INSERT ON messenger_message_acks
            FOR EACH ROW EXECUTE FUNCTION messenger_ack_race_pause()
            "#,
        )
        .execute(&pool)
        .await
        .unwrap();

        let first_store = store.clone();
        let second_store = store.clone();
        let first = first_store.toggle_ack(ToggleAckCommand {
            actor: seeded.recipient,
            branch_scope: BranchScope::single(seeded.branch),
            message_id: message.id,
            trace: TraceContext::generate(),
            occurred_at: OffsetDateTime::now_utc(),
        });
        let second = second_store.toggle_ack(ToggleAckCommand {
            actor: seeded.recipient,
            branch_scope: BranchScope::single(seeded.branch),
            message_id: message.id,
            trace: TraceContext::generate(),
            occurred_at: OffsetDateTime::now_utc(),
        });

        let (first, second) = tokio::join!(first, second);
        let first = first.expect("first racing toggle_ack must not 500");
        let second = second.expect("second racing toggle_ack must not 500");
        assert!(first.acked, "first racing toggle should converge to acked");
        assert!(
            second.acked,
            "second racing toggle should converge to acked"
        );
        assert_eq!(first.ack_count, 1);
        assert_eq!(second.ack_count, 1);

        let row_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM messenger_message_acks WHERE message_id = $1")
                .bind(*message.id.as_uuid())
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(
            row_count, 1,
            "the race must converge on exactly one ack row, never a duplicate or an error"
        );
    })
    .await;
}

// Capability 4: reply-quote surfaces a same-thread quote and rejects a
// cross-thread one.
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn reply_quote_is_same_thread_only_and_surfaced(pool: PgPool) {
    console_platform_request_context::scope_org(console_kernel_core::OrgId::knl(), async move {
        let seeded = seed_context(&pool).await;
        let store = PgMessengerStore::new(pool.clone());
        let thread = create_team_thread(&store, &seeded).await;
        let base = OffsetDateTime::now_utc();
        let original = send_at(&store, &seeded, thread.id, "원본 메시지", base).await;

        let reply = store
            .send_message(SendMessageCommand {
                actor: seeded.sender,
                branch_scope: BranchScope::single(seeded.branch),
                thread_id: thread.id,
                body: "인용 답장".to_owned(),
                attachment_evidence_ids: Vec::new(),
                quoted_message_id: Some(original.id),
                trace: TraceContext::generate(),
                occurred_at: base + Duration::seconds(1),
            })
            .await
            .unwrap();
        assert_eq!(reply.quoted_message_id, Some(original.id));
        assert_eq!(reply.quoted_body.as_deref(), Some("원본 메시지"));

        // A quote of a message in another thread is rejected.
        let other = create_team_thread(&store, &seeded).await;
        let cross = store
            .send_message(SendMessageCommand {
                actor: seeded.sender,
                branch_scope: BranchScope::single(seeded.branch),
                thread_id: other.id,
                body: "잘못된 인용".to_owned(),
                attachment_evidence_ids: Vec::new(),
                quoted_message_id: Some(original.id),
                trace: TraceContext::generate(),
                occurred_at: base + Duration::seconds(2),
            })
            .await
            .unwrap_err();
        assert_eq!(cross.kind(), ErrorKind::Validation);
    })
    .await;
}

// Capability 5: a muted thread suppresses this user's mention notification and
// is flagged muted in the thread list; a non-member cannot mute it.
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn mute_suppresses_mention_notification_and_flags_thread(pool: PgPool) {
    console_platform_request_context::scope_org(console_kernel_core::OrgId::knl(), async move {
        let seeded = seed_context(&pool).await;
        let sink = Arc::new(
            console_notifications_adapter_postgres::PgNotificationStore::new(pool.clone()),
        );
        let store = PgMessengerStore::new(pool.clone()).with_notification_sink(sink);
        let thread = create_team_thread(&store, &seeded).await;
        let base = OffsetDateTime::now_utc();

        // Recipient mutes the thread (direct-save personal setting).
        let muted = store
            .set_thread_mute(SetThreadMuteCommand {
                actor: seeded.recipient,
                branch_scope: BranchScope::single(seeded.branch),
                thread_id: thread.id,
                muted: true,
                trace: TraceContext::generate(),
                occurred_at: base,
            })
            .await
            .unwrap();
        assert!(muted.muted);

        // A mention of the muted recipient produces NO notification.
        send_at(
            &store,
            &seeded,
            thread.id,
            &format!("@{} 확인 부탁", seeded.recipient),
            base + Duration::seconds(1),
        )
        .await;
        assert_eq!(notification_count(&pool, seeded.recipient).await, 0);

        // The thread list flags it muted for the recipient (badge exclusion).
        let threads = store
            .list_threads(ListThreadsQuery {
                actor: seeded.recipient,
                branch_scope: BranchScope::single(seeded.branch),
                limit: 20,
            })
            .await
            .unwrap();
        assert!(threads.iter().find(|t| t.id == thread.id).unwrap().muted);

        // Unmuting restores mention notifications.
        store
            .set_thread_mute(SetThreadMuteCommand {
                actor: seeded.recipient,
                branch_scope: BranchScope::single(seeded.branch),
                thread_id: thread.id,
                muted: false,
                trace: TraceContext::generate(),
                occurred_at: base + Duration::seconds(2),
            })
            .await
            .unwrap();
        send_at(
            &store,
            &seeded,
            thread.id,
            &format!("@{} 다시 확인", seeded.recipient),
            base + Duration::seconds(3),
        )
        .await;
        assert_eq!(notification_count(&pool, seeded.recipient).await, 1);

        // A non-member cannot mute a thread they are not in.
        let denied = store
            .set_thread_mute(SetThreadMuteCommand {
                actor: seeded.receptionist,
                branch_scope: BranchScope::single(seeded.branch),
                thread_id: thread.id,
                muted: true,
                trace: TraceContext::generate(),
                occurred_at: base + Duration::seconds(4),
            })
            .await
            .unwrap_err();
        assert_eq!(denied.kind(), ErrorKind::Forbidden);
    })
    .await;
}

// Capability 2: presence is member-only and activity-derived. A member who just
// sent a message is online; a non-member cannot read presence at all.
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn presence_is_member_only_and_activity_derived(pool: PgPool) {
    console_platform_request_context::scope_org(console_kernel_core::OrgId::knl(), async move {
        let seeded = seed_context(&pool).await;
        let store = PgMessengerStore::new(pool.clone());
        let thread = create_team_thread(&store, &seeded).await;
        // Sending bumps the sender's presence to "now".
        send_at(
            &store,
            &seeded,
            thread.id,
            "왔습니다",
            OffsetDateTime::now_utc(),
        )
        .await;

        let presence = store
            .thread_presence(ThreadPresenceQuery {
                actor: seeded.sender,
                branch_scope: BranchScope::single(seeded.branch),
                thread_id: thread.id,
            })
            .await
            .unwrap();
        let sender_row = presence
            .iter()
            .find(|p| p.user_id == seeded.sender)
            .unwrap();
        assert_eq!(sender_row.status, PresenceStatus::Online);
        assert!(sender_row.last_activity_at.is_some());
        // The recipient has taken no action → offline, null timestamp.
        let recipient_row = presence
            .iter()
            .find(|p| p.user_id == seeded.recipient)
            .unwrap();
        assert_eq!(recipient_row.status, PresenceStatus::Offline);
        assert!(recipient_row.last_activity_at.is_none());

        // A non-member cannot read presence of a thread they are not in.
        let denied = store
            .thread_presence(ThreadPresenceQuery {
                actor: seeded.receptionist,
                branch_scope: BranchScope::single(seeded.branch),
                thread_id: thread.id,
            })
            .await
            .unwrap_err();
        assert_eq!(denied.kind(), ErrorKind::Forbidden);
    })
    .await;
}

async fn ack_audit_count(pool: &PgPool, message_id: console_kernel_core::MessageId) -> i64 {
    sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM audit_events WHERE action = 'message.ack' AND target_id = $1",
    )
    .bind(message_id.to_string())
    .fetch_one(pool)
    .await
    .unwrap()
}

#[derive(Debug, Clone, Copy)]
struct SeededContext {
    branch: BranchId,
    other_branch: BranchId,
    sender: UserId,
    recipient: UserId,
    receptionist: UserId,
    equipment_id: uuid::Uuid,
}

#[derive(Debug)]
struct RecordingNotifier {
    pool: PgPool,
    calls: Arc<Mutex<Vec<MessagePostedNotification>>>,
    ack_calls: Arc<Mutex<Vec<console_messenger_application::MessageAckNotification>>>,
}

impl RecordingNotifier {
    fn new(pool: PgPool) -> Self {
        Self {
            pool,
            calls: Arc::new(Mutex::new(Vec::new())),
            ack_calls: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

impl MessageNotifier for RecordingNotifier {
    fn message_posted(&self, notification: MessagePostedNotification) -> MessageNotifyFuture<'_> {
        Box::pin(async move {
            let row = sqlx::query(
                r#"
                SELECT EXISTS(
                    SELECT 1 FROM messenger_messages WHERE id = $1
                ) AS message_exists,
                EXISTS(
                    SELECT 1 FROM audit_events
                    WHERE action = 'message.send'
                      AND target_id = $2
                ) AS audit_exists
                "#,
            )
            .bind(*notification.message_id.as_uuid())
            .bind(notification.message_id.to_string())
            .fetch_one(&self.pool)
            .await
            .unwrap();
            assert!(row.get::<Option<bool>, _>("message_exists").unwrap());
            assert!(row.get::<Option<bool>, _>("audit_exists").unwrap());
            self.calls.lock().unwrap().push(notification);
        })
    }

    fn message_ack_toggled(
        &self,
        notification: console_messenger_application::MessageAckNotification,
    ) -> MessageNotifyFuture<'_> {
        Box::pin(async move {
            // The ack event fans out post-commit, so the audit row is already
            // durable when the realtime signal fires.
            let audited: Option<bool> = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM audit_events WHERE action = 'message.ack' AND target_id = $1)",
            )
            .bind(notification.message_id.to_string())
            .fetch_one(&self.pool)
            .await
            .unwrap();
            assert!(audited.unwrap());
            self.ack_calls.lock().unwrap().push(notification);
        })
    }
}

async fn create_team_thread(
    store: &PgMessengerStore,
    seeded: &SeededContext,
) -> console_messenger_application::ThreadSummary {
    store
        .create_thread(CreateThreadCommand {
            actor: seeded.sender,
            branch_scope: BranchScope::single(seeded.branch),
            branch_id: seeded.branch,
            kind: ThreadKind::Team,
            visibility: None,
            title: Some("정비팀".to_owned()),
            work_order_id: None,
            member_ids: vec![seeded.sender, seeded.recipient],
            trace: TraceContext::generate(),
            occurred_at: OffsetDateTime::now_utc(),
        })
        .await
        .unwrap()
}

async fn send_at(
    store: &PgMessengerStore,
    seeded: &SeededContext,
    thread_id: ThreadId,
    body: &str,
    occurred_at: OffsetDateTime,
) -> console_messenger_application::MessageSummary {
    store
        .send_message(SendMessageCommand {
            actor: seeded.sender,
            branch_scope: BranchScope::single(seeded.branch),
            thread_id,
            body: body.to_owned(),
            attachment_evidence_ids: Vec::new(),
            quoted_message_id: None,
            trace: TraceContext::generate(),
            occurred_at,
        })
        .await
        .unwrap()
}

async fn send_from(
    store: &PgMessengerStore,
    actor: UserId,
    branch_id: BranchId,
    thread_id: ThreadId,
    body: &str,
    occurred_at: OffsetDateTime,
) -> console_messenger_application::MessageSummary {
    store
        .send_message(SendMessageCommand {
            actor,
            branch_scope: BranchScope::single(branch_id),
            thread_id,
            body: body.to_owned(),
            attachment_evidence_ids: Vec::new(),
            quoted_message_id: None,
            trace: TraceContext::generate(),
            occurred_at,
        })
        .await
        .unwrap()
}

async fn person_view_audit_count(pool: &PgPool, target: UserId) -> i64 {
    sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM audit_events WHERE action = 'person.view' AND target_id = $1",
    )
    .bind(target.to_string())
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn seed_context(pool: &PgPool) -> SeededContext {
    let branch = seed_branch(pool, "Messenger Branch").await;
    let other_branch = seed_branch(pool, "Other Messenger Branch").await;
    let sender = seed_user(pool, "Sender", "MECHANIC", branch).await;
    let recipient = seed_user(pool, "Recipient", "ADMIN", branch).await;
    let receptionist = seed_user(pool, "Reception", "RECEPTIONIST", branch).await;
    let equipment_id = seed_equipment(pool, branch).await;
    SeededContext {
        branch,
        other_branch,
        sender,
        recipient,
        receptionist,
        equipment_id,
    }
}

async fn seed_branch(pool: &PgPool, name_prefix: &str) -> BranchId {
    let region_id: uuid::Uuid =
        sqlx::query_scalar("INSERT INTO regions (name, org_id) VALUES ($1, $2) RETURNING id")
            .bind(format!("{name_prefix} Region {}", uuid::Uuid::new_v4()))
            .bind(*OrgId::knl().as_uuid())
            .fetch_one(pool)
            .await
            .unwrap();
    let branch_id: uuid::Uuid = sqlx::query_scalar(
        "INSERT INTO branches (region_id, name, org_id) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(region_id)
    .bind(format!("{name_prefix} {}", uuid::Uuid::new_v4()))
    .bind(*OrgId::knl().as_uuid())
    .fetch_one(pool)
    .await
    .unwrap();
    BranchId::from_uuid(branch_id)
}

async fn seed_user(pool: &PgPool, name: &str, role: &str, branch_id: BranchId) -> UserId {
    let user_id = UserId::new();
    sqlx::query("INSERT INTO users (id, display_name, roles, org_id) VALUES ($1, $2, $3, $4)")
        .bind(*user_id.as_uuid())
        .bind(format!("{name} {}", uuid::Uuid::new_v4()))
        .bind(Vec::from([role]))
        .bind(*OrgId::knl().as_uuid())
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO user_branches (user_id, branch_id, org_id) VALUES ($1, $2, $3)")
        .bind(*user_id.as_uuid())
        .bind(*branch_id.as_uuid())
        .bind(*OrgId::knl().as_uuid())
        .execute(pool)
        .await
        .unwrap();
    user_id
}

async fn seed_equipment(pool: &PgPool, branch_id: BranchId) -> uuid::Uuid {
    let customer_id: uuid::Uuid = sqlx::query_scalar(
        "INSERT INTO registry_customers (branch_id, name, org_id) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(*branch_id.as_uuid())
    .bind(format!("Messenger Customer {}", uuid::Uuid::new_v4()))
    .bind(*OrgId::knl().as_uuid())
    .fetch_one(pool)
    .await
    .unwrap();
    let site_id: uuid::Uuid = sqlx::query_scalar(
        "INSERT INTO registry_sites (branch_id, customer_id, name, org_id) VALUES ($1, $2, $3, $4) RETURNING id",
    )
    .bind(*branch_id.as_uuid())
    .bind(customer_id)
    .bind(format!("Messenger Site {}", uuid::Uuid::new_v4()))
    .bind(*OrgId::knl().as_uuid())
    .fetch_one(pool)
    .await
    .unwrap();
    sqlx::query_scalar(
        r#"
        INSERT INTO registry_equipment (
            branch_id, customer_id, site_id, equipment_no, management_no,
            manufacturer_code, kind_code, power_code, status,
            specification, ton_text, model, source_sheet, source_row, org_id
        )
        VALUES ($1, $2, $3, $4, $5,
                'S', 'T', 'R', '임대', '좌식', '2.5', 'MSG', 'test', 1, $6)
        RETURNING id
        "#,
    )
    .bind(*branch_id.as_uuid())
    .bind(customer_id)
    .bind(site_id)
    .bind(format!("MSG{}-{:04}", short_code(), numeric_suffix()))
    .bind(format!("M-{}", short_code()))
    .bind(*OrgId::knl().as_uuid())
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn seed_work_order(pool: &PgPool, seeded: &SeededContext) -> WorkOrderId {
    let work_order_id = WorkOrderId::new();
    sqlx::query(
        r#"
        INSERT INTO work_orders (
            id, request_no, branch_id, equipment_id, customer_id, site_id,
            requested_by, status, priority, symptom, result_type, org_id
        )
        SELECT $1, $2, $3, e.id, e.customer_id, e.site_id,
               $4, 'RECEIVED', 'UNSET', 'Messenger fixture', 'UNKNOWN', $6
        FROM registry_equipment e
        WHERE e.id = $5
        "#,
    )
    .bind(*work_order_id.as_uuid())
    .bind(format!("20260612-{}", fastrandish_sequence()))
    .bind(*seeded.branch.as_uuid())
    .bind(*seeded.receptionist.as_uuid())
    .bind(seeded.equipment_id)
    .bind(*OrgId::knl().as_uuid())
    .execute(pool)
    .await
    .unwrap();
    work_order_id
}

fn fastrandish_sequence() -> String {
    let raw = uuid::Uuid::new_v4().as_u128() % 900 + 100;
    format!("{raw:03}")
}

fn short_code() -> String {
    uuid::Uuid::new_v4()
        .simple()
        .to_string()
        .chars()
        .take(2)
        .map(|ch| ch.to_ascii_uppercase())
        .collect()
}

fn numeric_suffix() -> u128 {
    uuid::Uuid::new_v4().as_u128() % 9000 + 1000
}
