#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;
use std::time::Duration;

use console_kernel_core::{
    AuditAction, AuditEvent, BranchId, BranchScope, OrgId, TraceContext, UserId,
};
use console_messenger_adapter_postgres::PgMessengerStore;
use console_messenger_application::{CreateThreadCommand, SendMessageCommand};
use console_messenger_domain::ThreadKind;
use console_platform_db::{DbError, with_audit};
use console_platform_realtime::{
    PgRealtimeHub, PostgresMessageNotifier, RealtimeEvent, RealtimeHubConfig, RealtimePrincipal,
};
use sqlx::PgPool;
use time::OffsetDateTime;
use tokio::time::timeout;

#[sqlx::test(migrations = "../db/migrations")]
async fn postgres_notify_from_instance_a_wakes_instance_b_and_rereads_message_body(pool: PgPool) {
    console_platform_request_context::scope_org(console_kernel_core::OrgId::knl(), async move {
        let branch_id = seed_branch(&pool).await;
        let sender = seed_user_with_branch(&pool, "sender", "MECHANIC", branch_id).await;
        let recipient = seed_user_with_branch(&pool, "recipient", "ADMIN", branch_id).await;

        let hub_b = Arc::new(PgRealtimeHub::new(
            pool.clone(),
            RealtimeHubConfig {
                connection_buffer: 8,
            },
        ));
        let _listener_b = hub_b.clone().start_postgres_listener().await.unwrap();
        let mut subscriber_b = hub_b
            .connect(
                RealtimePrincipal {
                    user_id: recipient,
                    branch_scope: BranchScope::single(branch_id),
                    org_id: OrgId::knl(),
                },
                None,
            )
            .await
            .unwrap();

        let store_a = PgMessengerStore::new(pool.clone())
            .with_notifier(Arc::new(PostgresMessageNotifier::new(pool.clone())));
        let thread = store_a
            .create_thread(CreateThreadCommand {
                actor: sender,
                branch_scope: BranchScope::single(branch_id),
                branch_id,
                kind: ThreadKind::Team,
                visibility: None,
                title: Some("정비팀".to_owned()),
                work_order_id: None,
                member_ids: vec![sender, recipient],
                trace: TraceContext::generate(),
                occurred_at: OffsetDateTime::now_utc(),
            })
            .await
            .unwrap();

        let sent = store_a
            .send_message(SendMessageCommand {
                actor: sender,
                branch_scope: BranchScope::single(branch_id),
                thread_id: thread.id,
                body: "A 인스턴스에서 저장된 본문을 B 인스턴스가 DB에서 재조회".to_owned(),
                attachment_evidence_ids: Vec::new(),
                quoted_message_id: None,
                trace: TraceContext::generate(),
                occurred_at: OffsetDateTime::now_utc(),
            })
            .await
            .unwrap();

        let delivered = timeout(Duration::from_secs(3), subscriber_b.recv())
            .await
            .expect("instance B should receive the LISTEN/NOTIFY wake")
            .expect("instance B connection should still be open");

        let RealtimeEvent::MessagePosted { message } = delivered else {
            panic!("expected a messenger MessagePosted event");
        };
        assert_eq!(message.id, sent.id);
        assert_eq!(message.thread_id, thread.id);
        assert_eq!(message.branch_id, branch_id);
        assert_eq!(
            message.body,
            "A 인스턴스에서 저장된 본문을 B 인스턴스가 DB에서 재조회"
        );
    })
    .await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn reconnect_replays_messages_after_the_last_read_cursor(pool: PgPool) {
    console_platform_request_context::scope_org(console_kernel_core::OrgId::knl(), async move {
        let branch_id = seed_branch(&pool).await;
        let sender = seed_user_with_branch(&pool, "resume sender", "MECHANIC", branch_id).await;
        let recipient = seed_user_with_branch(&pool, "resume recipient", "ADMIN", branch_id).await;
        let store = PgMessengerStore::new(pool.clone());
        let thread = store
            .create_thread(CreateThreadCommand {
                actor: sender,
                branch_scope: BranchScope::single(branch_id),
                branch_id,
                kind: ThreadKind::Team,
                visibility: None,
                title: Some("정비팀".to_owned()),
                work_order_id: None,
                member_ids: vec![sender, recipient],
                trace: TraceContext::generate(),
                occurred_at: OffsetDateTime::now_utc(),
            })
            .await
            .unwrap();
        let first = store
            .send_message(SendMessageCommand {
                actor: sender,
                branch_scope: BranchScope::single(branch_id),
                thread_id: thread.id,
                body: "already read".to_owned(),
                attachment_evidence_ids: Vec::new(),
                quoted_message_id: None,
                trace: TraceContext::generate(),
                occurred_at: OffsetDateTime::now_utc(),
            })
            .await
            .unwrap();
        let second = store
            .send_message(SendMessageCommand {
                actor: sender,
                branch_scope: BranchScope::single(branch_id),
                thread_id: thread.id,
                body: "replayed after reconnect".to_owned(),
                attachment_evidence_ids: Vec::new(),
                quoted_message_id: None,
                trace: TraceContext::generate(),
                occurred_at: OffsetDateTime::now_utc() + time::Duration::seconds(1),
            })
            .await
            .unwrap();

        let hub = Arc::new(PgRealtimeHub::new(
            pool.clone(),
            RealtimeHubConfig {
                connection_buffer: 8,
            },
        ));
        let mut reconnected = hub
            .connect(
                RealtimePrincipal {
                    user_id: recipient,
                    branch_scope: BranchScope::single(branch_id),
                    org_id: OrgId::knl(),
                },
                Some(first.id),
            )
            .await
            .unwrap();

        let delivered = timeout(Duration::from_secs(3), reconnected.recv())
            .await
            .expect("resume replay should arrive")
            .expect("connection should stay open");

        let RealtimeEvent::MessagePosted { message } = delivered else {
            panic!("expected a messenger MessagePosted event");
        };
        assert_eq!(message.id, second.id);
        assert_eq!(message.body, "replayed after reconnect");
    })
    .await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn reconnect_replay_pages_past_one_hundred_messages_without_truncating(pool: PgPool) {
    console_platform_request_context::scope_org(console_kernel_core::OrgId::knl(), async move {
        let branch_id = seed_branch(&pool).await;
        let sender = seed_user_with_branch(&pool, "page sender", "MECHANIC", branch_id).await;
        let recipient = seed_user_with_branch(&pool, "page recipient", "ADMIN", branch_id).await;
        let store = PgMessengerStore::new(pool.clone());
        let thread = create_thread(&store, sender, recipient, branch_id).await;
        let base = OffsetDateTime::now_utc();
        let cursor = send_at(&store, sender, branch_id, thread.id, "already read", base).await;
        let mut expected_ids = Vec::new();

        for index in 0..105 {
            let message = send_at(
                &store,
                sender,
                branch_id,
                thread.id,
                &format!("missed {index:03}"),
                base + time::Duration::seconds(i64::from(index + 1)),
            )
            .await;
            expected_ids.push(message.id);
        }

        let hub = Arc::new(PgRealtimeHub::new(
            pool.clone(),
            RealtimeHubConfig {
                connection_buffer: 8,
            },
        ));
        let mut reconnected = hub
            .connect(
                RealtimePrincipal {
                    user_id: recipient,
                    branch_scope: BranchScope::single(branch_id),
                    org_id: OrgId::knl(),
                },
                Some(cursor.id),
            )
            .await
            .unwrap();

        let mut delivered_ids = Vec::new();
        for _ in 0..expected_ids.len() {
            delivered_ids.push(recv_message_id(&mut reconnected).await);
        }

        assert_eq!(delivered_ids, expected_ids);
    })
    .await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn reconnect_replay_streams_backlog_larger_than_connection_buffer(pool: PgPool) {
    console_platform_request_context::scope_org(console_kernel_core::OrgId::knl(), async move {
        let branch_id = seed_branch(&pool).await;
        let sender = seed_user_with_branch(&pool, "buffer sender", "MECHANIC", branch_id).await;
        let recipient = seed_user_with_branch(&pool, "buffer recipient", "ADMIN", branch_id).await;
        let store = PgMessengerStore::new(pool.clone());
        let thread = create_thread(&store, sender, recipient, branch_id).await;
        let base = OffsetDateTime::now_utc();
        let cursor = send_at(&store, sender, branch_id, thread.id, "already read", base).await;
        let mut expected_ids = Vec::new();

        for index in 0..12 {
            let message = send_at(
                &store,
                sender,
                branch_id,
                thread.id,
                &format!("buffered replay {index:02}"),
                base + time::Duration::seconds(i64::from(index + 1)),
            )
            .await;
            expected_ids.push(message.id);
        }

        let hub = Arc::new(PgRealtimeHub::new(
            pool.clone(),
            RealtimeHubConfig {
                connection_buffer: 3,
            },
        ));
        let mut reconnected = hub
            .connect(
                RealtimePrincipal {
                    user_id: recipient,
                    branch_scope: BranchScope::single(branch_id),
                    org_id: OrgId::knl(),
                },
                Some(cursor.id),
            )
            .await
            .unwrap();

        let mut delivered_ids = Vec::new();
        for _ in 0..expected_ids.len() {
            delivered_ids.push(recv_message_id(&mut reconnected).await);
        }

        assert_eq!(delivered_ids, expected_ids);
        assert!(
            timeout(Duration::from_millis(50), reconnected.disconnect())
                .await
                .is_err(),
            "a draining client should not be disconnected just because replay exceeds the mpsc buffer"
        );
    })
    .await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn live_messages_during_replay_are_delivered_after_replay_without_duplicates(pool: PgPool) {
    console_platform_request_context::scope_org(console_kernel_core::OrgId::knl(), async move {
        let branch_id = seed_branch(&pool).await;
        let sender = seed_user_with_branch(&pool, "race sender", "MECHANIC", branch_id).await;
        let recipient = seed_user_with_branch(&pool, "race recipient", "ADMIN", branch_id).await;
        let store = PgMessengerStore::new(pool.clone());
        let thread = create_thread(&store, sender, recipient, branch_id).await;
        let base = OffsetDateTime::now_utc();
        let cursor = send_at(&store, sender, branch_id, thread.id, "already read", base).await;
        let mut expected_ids = Vec::new();

        for index in 0..5 {
            let message = send_at(
                &store,
                sender,
                branch_id,
                thread.id,
                &format!("missed before live {index}"),
                base + time::Duration::seconds(i64::from(index + 1)),
            )
            .await;
            expected_ids.push(message.id);
        }

        let hub = Arc::new(PgRealtimeHub::new(
            pool.clone(),
            RealtimeHubConfig {
                connection_buffer: 2,
            },
        ));
        let mut reconnected = hub
            .connect(
                RealtimePrincipal {
                    user_id: recipient,
                    branch_scope: BranchScope::single(branch_id),
                    org_id: OrgId::knl(),
                },
                Some(cursor.id),
            )
            .await
            .unwrap();

        let live = send_at(
            &store,
            sender,
            branch_id,
            thread.id,
            "live while replaying",
            base + time::Duration::seconds(10),
        )
        .await;
        expected_ids.push(live.id);
        hub.dispatch_local_for_test(
            OrgId::knl(),
            RealtimeEvent::MessagePosted {
                message: live.clone(),
            },
        )
        .await
        .unwrap();

        let mut delivered_ids = Vec::new();
        for _ in 0..expected_ids.len() {
            delivered_ids.push(recv_message_id(&mut reconnected).await);
        }

        assert_eq!(delivered_ids, expected_ids);
    })
    .await;
}

async fn seed_branch(pool: &PgPool) -> BranchId {
    let region_id = uuid::Uuid::new_v4();
    let branch_id = BranchId::new();
    let event = AuditEvent::new(
        None,
        AuditAction::new("test.seed_realtime_branch").unwrap(),
        "branch",
        branch_id.to_string(),
        TraceContext::generate(),
        OffsetDateTime::now_utc(),
    )
    .with_branch(branch_id);
    with_audit(pool, event, |tx| {
        Box::pin(async move {
            sqlx::query("INSERT INTO regions (id, name, org_id) VALUES ($1, $2, $3)")
                .bind(region_id)
                .bind(format!("Realtime Region {}", uuid::Uuid::new_v4()))
                .bind(*OrgId::knl().as_uuid())
                .execute(tx.as_mut())
                .await
                .map_err(DbError::Sqlx)?;
            sqlx::query(
                "INSERT INTO branches (id, region_id, name, org_id) VALUES ($1, $2, $3, $4)",
            )
            .bind(*branch_id.as_uuid())
            .bind(region_id)
            .bind(format!("Realtime Branch {}", uuid::Uuid::new_v4()))
            .bind(*OrgId::knl().as_uuid())
            .execute(tx.as_mut())
            .await
            .map_err(DbError::Sqlx)?;
            Ok::<BranchId, DbError>(branch_id)
        })
    })
    .await
    .unwrap()
}

async fn create_thread(
    store: &PgMessengerStore,
    sender: UserId,
    recipient: UserId,
    branch_id: BranchId,
) -> console_messenger_application::ThreadSummary {
    store
        .create_thread(CreateThreadCommand {
            actor: sender,
            branch_scope: BranchScope::single(branch_id),
            branch_id,
            kind: ThreadKind::Team,
            visibility: None,
            title: Some("정비팀".to_owned()),
            work_order_id: None,
            member_ids: vec![sender, recipient],
            trace: TraceContext::generate(),
            occurred_at: OffsetDateTime::now_utc(),
        })
        .await
        .unwrap()
}

async fn send_at(
    store: &PgMessengerStore,
    sender: UserId,
    branch_id: BranchId,
    thread_id: console_kernel_core::ThreadId,
    body: &str,
    occurred_at: OffsetDateTime,
) -> console_messenger_application::MessageSummary {
    store
        .send_message(SendMessageCommand {
            actor: sender,
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

async fn recv_message_id(
    connection: &mut console_platform_realtime::RealtimeConnection,
) -> console_kernel_core::MessageId {
    let delivered = timeout(Duration::from_secs(3), connection.recv())
        .await
        .expect("replay event should arrive")
        .expect("connection should stay open");
    let RealtimeEvent::MessagePosted { message } = delivered else {
        panic!("expected a messenger MessagePosted event");
    };
    message.id
}

async fn seed_user_with_branch(
    pool: &PgPool,
    name: &str,
    role: &str,
    branch_id: BranchId,
) -> UserId {
    let user_id = UserId::new();
    let name = name.to_owned();
    let role = role.to_owned();
    let event = AuditEvent::new(
        None,
        AuditAction::new("test.seed_realtime_user").unwrap(),
        "user",
        user_id.to_string(),
        TraceContext::generate(),
        OffsetDateTime::now_utc(),
    )
    .with_branch(branch_id);
    with_audit(pool, event, |tx| {
        Box::pin(async move {
            sqlx::query(
                "INSERT INTO users (id, display_name, roles, org_id) VALUES ($1, $2, $3, $4)",
            )
            .bind(*user_id.as_uuid())
            .bind(format!("Realtime {name} {}", uuid::Uuid::new_v4()))
            .bind(Vec::from([role]))
            .bind(*OrgId::knl().as_uuid())
            .execute(tx.as_mut())
            .await
            .map_err(DbError::Sqlx)?;
            sqlx::query(
                "INSERT INTO user_branches (user_id, branch_id, org_id) VALUES ($1, $2, $3)",
            )
            .bind(*user_id.as_uuid())
            .bind(*branch_id.as_uuid())
            .bind(*OrgId::knl().as_uuid())
            .execute(tx.as_mut())
            .await
            .map_err(DbError::Sqlx)?;
            Ok::<(), DbError>(())
        })
    })
    .await
    .unwrap();
    user_id
}

// These probes use the real listener and RLS role. A final canonical notification
// is an ordered barrier: seeing it proves earlier hostile wakes were processed.
struct RoutingFixture {
    owner: PgPool,
    hub: Arc<PgRealtimeHub>,
    _listener: console_platform_realtime::PostgresBridgeHandle,
    branch: BranchId,
    recipient: UserId,
    other: UserId,
    foreign_org: OrgId,
}

impl RoutingFixture {
    async fn new(owner: PgPool) -> Self {
        let branch = seed_branch(&owner).await;
        let recipient = seed_user_with_branch(&owner, "routing recipient", "ADMIN", branch).await;
        let other = seed_user_with_branch(&owner, "other recipient", "ADMIN", branch).await;
        let foreign_org = OrgId::new();
        sqlx::query("INSERT INTO organizations(id, slug, name) VALUES ($1,$2,'Foreign Company')")
            .bind(*foreign_org.as_uuid())
            .bind(foreign_org.to_string())
            .execute(&owner)
            .await
            .unwrap();
        let runtime = sqlx::postgres::PgPoolOptions::new()
            .after_connect(|conn, _| {
                Box::pin(async move {
                    sqlx::query("SET ROLE console_rt").execute(conn).await?;
                    Ok(())
                })
            })
            .connect_with(owner.connect_options().as_ref().clone())
            .await
            .unwrap();
        let safe: bool = sqlx::query_scalar("SELECT current_user='console_rt' AND NOT rolsuper AND NOT rolbypassrls FROM pg_roles WHERE rolname=current_user")
            .fetch_one(&runtime).await.unwrap();
        assert!(safe, "the bridge must exercise actual Company RLS");
        let hub = Arc::new(PgRealtimeHub::new(runtime, RealtimeHubConfig::default()));
        let listener = hub.clone().start_postgres_listener().await.unwrap();
        Self {
            owner,
            hub,
            _listener: listener,
            branch,
            recipient,
            other,
            foreign_org,
        }
    }

    async fn connect(
        &self,
        user: UserId,
        org: OrgId,
    ) -> console_platform_realtime::RealtimeConnection {
        self.hub
            .connect(
                RealtimePrincipal {
                    user_id: user,
                    branch_scope: BranchScope::All,
                    org_id: org,
                },
                None,
            )
            .await
            .unwrap()
    }

    async fn notification(
        &self,
        body: &str,
    ) -> console_platform_realtime::NotificationNotifyPayload {
        let id = console_kernel_core::NotificationId::new();
        sqlx::query("INSERT INTO notifications(id,org_id,recipient_user_id,category,body,link) VALUES($1,$2,$3,'test',$4,$5)")
            .bind(*id.as_uuid()).bind(*OrgId::knl().as_uuid()).bind(*self.recipient.as_uuid())
            .bind(body).bind(serde_json::to_value(console_notifications_domain::NotificationLink::Screen { screen: "notifications".into() }).unwrap()).execute(&self.owner).await.unwrap();
        console_platform_realtime::NotificationNotifyPayload {
            notification_id: id,
            recipient_user_id: self.recipient,
            org_id: OrgId::knl(),
        }
    }

    async fn publish(&self, wakes: Vec<(&str, serde_json::Value)>) {
        let mut tx = self.owner.begin().await.unwrap();
        for (channel, payload) in wakes {
            sqlx::query("SELECT pg_notify($1,$2)")
                .bind(channel)
                .bind(payload.to_string())
                .execute(&mut *tx)
                .await
                .unwrap();
        }
        tx.commit().await.unwrap();
    }
}

async fn receive(connection: &mut console_platform_realtime::RealtimeConnection) -> RealtimeEvent {
    timeout(Duration::from_secs(5), connection.recv())
        .await
        .expect("canonical listener barrier must be delivered, not inferred from a timeout")
        .expect("connection must remain open")
}

#[sqlx::test(migrations = "../db/migrations")]
async fn notification_wakes_cannot_redirect_canonical_content_or_cross_company(owner: PgPool) {
    console_platform_request_context::scope_org(OrgId::knl(), async move {
        use console_platform_realtime::NOTIFICATION_CREATED_CHANNEL;
        let f = RoutingFixture::new(owner).await;
        let mut canonical = f.connect(f.recipient, OrgId::knl()).await;
        let mut wrong_person = f.connect(f.other, OrgId::knl()).await;
        let mut wrong_company = f.connect(f.recipient, f.foreign_org).await;
        let notification = f.notification("canonical private content").await;
        let barrier = f.notification("ordered barrier").await;
        let mut redirected = notification;
        redirected.recipient_user_id = f.other;
        let mut foreign = notification;
        foreign.org_id = f.foreign_org;
        f.publish(vec![
            (
                NOTIFICATION_CREATED_CHANNEL,
                serde_json::to_value(redirected).unwrap(),
            ),
            (
                NOTIFICATION_CREATED_CHANNEL,
                serde_json::to_value(foreign).unwrap(),
            ),
            (
                NOTIFICATION_CREATED_CHANNEL,
                serde_json::to_value(notification).unwrap(),
            ),
            (
                NOTIFICATION_CREATED_CHANNEL,
                serde_json::to_value(barrier).unwrap(),
            ),
        ])
        .await;
        for expected in [notification.notification_id, barrier.notification_id] {
            let RealtimeEvent::NotificationCreated { notification } = receive(&mut canonical).await
            else {
                panic!("expected canonical notification");
            };
            assert_eq!(notification.id, expected);
            assert_eq!(notification.recipient_user_id, f.recipient);
        }
        f.hub.shutdown().await;
        assert!(
            canonical.recv().await.is_none(),
            "hostile wakes must not duplicate canonical delivery"
        );
        assert!(
            wrong_person.recv().await.is_none(),
            "a forged recipient must not redirect private content"
        );
        assert!(
            wrong_company.recv().await.is_none(),
            "the same account in another Company must receive nothing"
        );
    })
    .await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn message_and_ack_wakes_bind_company_message_and_thread(owner: PgPool) {
    console_platform_request_context::scope_org(OrgId::knl(), async move {
        use console_platform_realtime::{
            MESSAGE_ACK_CHANNEL, MESSAGE_POSTED_CHANNEL, MessageAckNotifyPayload,
            MessageNotifyPayload, NOTIFICATION_CREATED_CHANNEL,
        };
        let f = RoutingFixture::new(owner).await;
        let store = PgMessengerStore::new(f.owner.clone());
        let thread = create_thread(&store, f.other, f.recipient, f.branch).await;
        let other_thread = create_thread(&store, f.other, f.recipient, f.branch).await;
        let message = send_at(
            &store,
            f.other,
            f.branch,
            thread.id,
            "private message",
            OffsetDateTime::now_utc(),
        )
        .await;
        let outsider = seed_user_with_branch(&f.owner, "nonmember", "ADMIN", f.branch).await;
        let mut canonical = f.connect(f.recipient, OrgId::knl()).await;
        let mut nonmember = f.connect(outsider, OrgId::knl()).await;
        let mut wrong_company = f.connect(f.recipient, f.foreign_org).await;
        let payload = MessageNotifyPayload {
            message_id: message.id,
            thread_id: thread.id,
            org_id: OrgId::knl(),
        };
        let mut foreign = payload;
        foreign.org_id = f.foreign_org;
        let mut wrong_thread = payload;
        wrong_thread.thread_id = other_thread.id;
        let ack = MessageAckNotifyPayload {
            message_id: message.id,
            thread_id: thread.id,
            org_id: OrgId::knl(),
        };
        let barrier = f.notification("message listener barrier").await;
        f.publish(vec![
            (
                MESSAGE_POSTED_CHANNEL,
                serde_json::to_value(foreign).unwrap(),
            ),
            (
                MESSAGE_POSTED_CHANNEL,
                serde_json::to_value(wrong_thread).unwrap(),
            ),
            (MESSAGE_ACK_CHANNEL, serde_json::to_value(foreign).unwrap()),
            (
                MESSAGE_ACK_CHANNEL,
                serde_json::to_value(wrong_thread).unwrap(),
            ),
            (
                MESSAGE_POSTED_CHANNEL,
                serde_json::to_value(payload).unwrap(),
            ),
            (MESSAGE_ACK_CHANNEL, serde_json::to_value(ack).unwrap()),
            (
                NOTIFICATION_CREATED_CHANNEL,
                serde_json::to_value(barrier).unwrap(),
            ),
        ])
        .await;
        let RealtimeEvent::MessagePosted { message: delivered } = receive(&mut canonical).await
        else {
            panic!("expected canonical message")
        };
        assert_eq!(delivered.id, message.id);
        assert_eq!(delivered.body, message.body);
        assert_eq!(delivered.thread_id, thread.id);
        assert_eq!(
            receive(&mut canonical).await,
            RealtimeEvent::MessageAcked {
                message_id: message.id,
                thread_id: thread.id,
                branch_id: f.branch,
                ack_count: 0,
            }
        );
        let RealtimeEvent::NotificationCreated { notification } = receive(&mut canonical).await
        else {
            panic!("expected listener barrier")
        };
        assert_eq!(notification.id, barrier.notification_id);
        f.hub.shutdown().await;
        assert!(
            canonical.recv().await.is_none(),
            "mismatched IDs must not produce extra deliveries"
        );
        assert!(
            nonmember.recv().await.is_none(),
            "branch access alone is insufficient"
        );
        assert!(
            wrong_company.recv().await.is_none(),
            "thread membership in another Company is insufficient"
        );
    })
    .await;
}
