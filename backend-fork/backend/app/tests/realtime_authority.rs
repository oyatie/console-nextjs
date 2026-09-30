#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Real WebSocket frames, live PostgreSQL and the non-owner runtime role.
use console_kernel_core::{BranchId, MessageId, NotificationId, OrgId, ThreadId, UserId};
use console_platform_auth::{
    AccessTokenInput, JwtIssuer, JwtSettings, JwtVerifier, RefreshTokenStore,
};
use console_platform_realtime::{
    PgRealtimeHub, RealtimeEvent, RealtimeHubConfig, RealtimeRestState,
};
use console_platform_test_support::{seed_branch, seed_user};
use p256::{
    ecdsa::SigningKey,
    elliptic_curve::rand_core::OsRng,
    pkcs8::{EncodePrivateKey, EncodePublicKey, LineEnding},
};
use serde_json::{Value, json};
use sqlx::{PgPool, Postgres, Transaction};
use std::{net::SocketAddr, sync::Arc, time::Duration as StdDuration};
use time::{Duration, OffsetDateTime};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    time::timeout,
};

struct Fixture {
    owner: PgPool,
    runtime: PgPool,
    hub: Arc<PgRealtimeHub>,
    server: tokio::task::JoinHandle<()>,
    addr: SocketAddr,
    user: UserId,
    branch: BranchId,
    thread: ThreadId,
    message: MessageId,
    cursor: MessageId,
    notification: NotificationId,
    family: uuid::Uuid,
    token: String,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.server.abort();
    }
}
impl Fixture {
    async fn new(owner: PgPool, ttl: Duration) -> Self {
        let branch = seed_branch(&owner, "emission", "emission").await;
        let user = seed_user(&owner, "Current sender", "MECHANIC", branch).await;
        let runtime = sqlx::postgres::PgPoolOptions::new()
            .max_connections(6)
            .connect_with(
                owner
                    .connect_options()
                    .as_ref()
                    .clone()
                    .username("console_rt"),
            )
            .await
            .unwrap();
        let safe: bool = sqlx::query_scalar("SELECT current_user='console_rt' AND NOT rolsuper AND NOT rolbypassrls FROM pg_roles WHERE rolname=current_user")
            .fetch_one(&runtime).await.unwrap();
        assert!(safe);
        let thread = ThreadId::new();
        sqlx::query("INSERT INTO messenger_threads(id,org_id,kind,branch_id,created_by,visibility) VALUES($1,$2,'team',$3,$4,'direct')")
            .bind(*thread.as_uuid()).bind(*OrgId::knl().as_uuid()).bind(*branch.as_uuid()).bind(*user.as_uuid()).execute(&owner).await.unwrap();
        sqlx::query("INSERT INTO messenger_thread_members(thread_id,user_id,org_id,joined_at) VALUES($1,$2,$3,now())")
            .bind(*thread.as_uuid()).bind(*user.as_uuid()).bind(*OrgId::knl().as_uuid()).execute(&owner).await.unwrap();
        let cursor = MessageId::new();
        let message = MessageId::new();
        for (id, body, date) in [
            (cursor, "cursor", "2026-09-22T00:00:00Z"),
            (message, "Current body", "2026-09-23T00:00:00Z"),
        ] {
            sqlx::query("INSERT INTO messenger_messages(id,org_id,thread_id,branch_id,sender_id,body,sent_at) VALUES($1,$2,$3,$4,$5,$6,$7::text::timestamptz)")
                .bind(*id.as_uuid()).bind(*OrgId::knl().as_uuid()).bind(*thread.as_uuid()).bind(*branch.as_uuid()).bind(*user.as_uuid()).bind(body).bind(date).execute(&owner).await.unwrap();
        }
        let notification = NotificationId::new();
        sqlx::query("INSERT INTO notifications(id,org_id,recipient_user_id,category,body,link) VALUES($1,$2,$3,'test','Current notice',$4)")
            .bind(*notification.as_uuid()).bind(*OrgId::knl().as_uuid()).bind(*user.as_uuid()).bind(json!({"type":"screen","screen":"notifications"})).execute(&owner).await.unwrap();
        let key = SigningKey::random(&mut OsRng);
        let private = key.to_pkcs8_pem(LineEnding::LF).unwrap();
        let public = key
            .verifying_key()
            .to_public_key_pem(LineEnding::LF)
            .unwrap();
        let settings = JwtSettings {
            issuer: "ws-authority".into(),
            audience: "ws-authority".into(),
            access_token_ttl: ttl,
        };
        let issuer =
            JwtIssuer::from_es256_pem(settings.clone(), private.as_bytes(), public.as_bytes())
                .unwrap();
        let verifier = JwtVerifier::from_es256_public_pem(settings, public.as_bytes()).unwrap();
        let now = OffsetDateTime::now_utc();
        let family = RefreshTokenStore
            .issue_family(
                &runtime,
                *user.as_uuid(),
                OrgId::knl(),
                now,
                Duration::hours(1),
            )
            .await
            .unwrap();
        let token = issuer
            .issue_session_access_token(
                AccessTokenInput {
                    subject: user,
                    org_id: OrgId::knl(),
                    roles: vec!["MECHANIC".into()],
                    branches: vec![branch],
                    platform: false,
                    view_as: false,
                    read_only: false,
                    display_name: None,
                    feature_grants: vec![],
                    authz_subject_version: 0,
                    authz_policy_version: 0,
                    session_generation: 0,
                    issued_at: now,
                },
                None,
                vec![],
                family.family_id,
                family.expires_at,
            )
            .unwrap();
        let hub = Arc::new(PgRealtimeHub::new(
            runtime.clone(),
            RealtimeHubConfig::default(),
        ));
        let router =
            console_platform_realtime::router(RealtimeRestState::new(hub.clone(), Some(verifier)));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        Self {
            owner,
            runtime,
            hub,
            server,
            addr,
            user,
            branch,
            thread,
            message,
            cursor,
            notification,
            family: family.family_id,
            token,
        }
    }
    async fn connect(&self, cursor: Option<MessageId>) -> TcpStream {
        let mut stream = TcpStream::connect(self.addr).await.unwrap();
        let path = cursor.map_or("/api/v1/ws".to_owned(), |id| {
            format!("/api/v1/ws?last_message_id={id}")
        });
        stream.write_all(format!("GET {path} HTTP/1.1\r\nHost: {}\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nAuthorization: Bearer {}\r\n\r\n",self.addr,self.token).as_bytes()).await.unwrap();
        let header = timeout(StdDuration::from_secs(4), async {
            let mut bytes = Vec::new();
            while !bytes.ends_with(b"\r\n\r\n") {
                assert!(bytes.len() < 8192);
                bytes.push(stream.read_u8().await.unwrap());
            }
            String::from_utf8(bytes).unwrap()
        })
        .await
        .unwrap();
        assert!(
            header.starts_with("HTTP/1.1 101"),
            "upgrade status: {}",
            header.lines().next().unwrap()
        );
        if cursor.is_none() {
            timeout(StdDuration::from_secs(2), async {
                while self.hub.connection_count().await == 0 {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .expect("upgrade task registered its connection");
        }
        stream
    }
    fn event(&self, kind: &str) -> RealtimeEvent {
        match kind {
            "notification" => serde_json::from_value(json!({"type":"notification_created","notification":{
                "id":self.notification,"recipient_user_id":self.user,"category":"test","kind":"info","text":"Current notice","link":{"type":"screen","screen":"notifications"},"unread":true,"created_at":"2026-09-23T00:00:00Z","read_at":null,"resolved_at":null,"muted":false
            }})).unwrap(),
            "ack" => RealtimeEvent::MessageAcked { message_id:self.message,thread_id:self.thread,branch_id:self.branch,ack_count:0 },
            _ => serde_json::from_value(json!({"type":"message_posted","message":{
                "id":self.message,"thread_id":self.thread,"branch_id":self.branch,"sender_id":self.user,"sender_name":"Current sender","body":"Current body","read_count":0,"read_target_count":0,"ack_count":0,"acked_by_me":false,"quoted_message_id":null,"quoted_body":null,"quoted_sender_name":null,"attachment_evidence_ids":[],"sent_at":"2026-09-23T00:00:00Z","created_at":"2026-09-23T00:00:00Z"
            }})).unwrap(),
        }
    }
    async fn enqueue(&self, event: RealtimeEvent) {
        match event {
            RealtimeEvent::NotificationCreated { notification } => {
                self.hub
                    .dispatch_notification_for_test(OrgId::knl(), notification)
                    .await
            }
            event => self
                .hub
                .dispatch_local_for_test(OrgId::knl(), event)
                .await
                .unwrap(),
        }
    }
    async fn block_authority(&self) -> Transaction<'_, Postgres> {
        let mut tx = self.owner.begin().await.unwrap();
        sqlx::query("LOCK auth_refresh_token_families IN ACCESS EXCLUSIVE MODE")
            .execute(&mut *tx)
            .await
            .unwrap();
        tx
    }
    async fn block_messages(&self) -> Transaction<'_, Postgres> {
        let mut tx = self.owner.begin().await.unwrap();
        sqlx::query("LOCK messenger_messages IN ACCESS EXCLUSIVE MODE")
            .execute(&mut *tx)
            .await
            .unwrap();
        tx
    }
    async fn wait_for_blocked_message_read(&self) {
        timeout(StdDuration::from_secs(3),async {
            loop {
                let waiting:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_locks l JOIN pg_stat_activity a USING(pid) WHERE a.datname=current_database() AND a.usename='console_rt' AND l.locktype='relation' AND l.relation='messenger_messages'::regclass AND NOT l.granted)")
                    .fetch_one(&self.owner).await.unwrap();
                if waiting {break;}
                tokio::task::yield_now().await;
            }
        }).await.expect("current-owner read must run before emission");
    }
    async fn assert_removed(&self) {
        timeout(StdDuration::from_secs(3), async {
            while self.hub.connection_count().await != 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("connection must be removed");
    }
}

async fn frame(stream: &mut TcpStream) -> (u8, Vec<u8>) {
    timeout(StdDuration::from_secs(8), async {
        let first = stream.read_u8().await.unwrap();
        let second = stream.read_u8().await.unwrap();
        assert_eq!(first & 0x80, 0x80, "bounded test frames must be complete");
        assert_eq!(second & 0x80, 0, "server frames are unmasked");
        let size = match second & 0x7f {
            126 => u64::from(stream.read_u16().await.unwrap()),
            127 => stream.read_u64().await.unwrap(),
            n => u64::from(n),
        };
        assert!(size < 131072, "bound test frame allocation");
        let mut bytes = vec![0; usize::try_from(size).unwrap()];
        stream.read_exact(&mut bytes).await.unwrap();
        (first & 0xf, bytes)
    })
    .await
    .expect("expected a bounded wire outcome")
}
async fn data(stream: &mut TcpStream) -> Value {
    let (opcode, bytes) = frame(stream).await;
    assert_eq!(opcode, 1);
    serde_json::from_slice(&bytes).unwrap()
}
async fn closed(stream: &mut TcpStream, code: u16) {
    let (opcode, bytes) = frame(stream).await;
    assert_eq!(opcode, 8, "no business frame may precede the close");
    assert!(bytes.len() >= 2);
    assert_eq!(u16::from_be_bytes([bytes[0], bytes[1]]), code);
    let reason = std::str::from_utf8(&bytes[2..]).unwrap();
    assert!(
        matches!(
            reason,
            "realtime access denied"
                | "realtime unavailable"
                | "server_shutdown"
                | "replay_failed; reconnect with last_message_id cursor"
                | "lagging_consumer; reconnect with last_message_id cursor"
        ),
        "close reason must be generic"
    );
}

async fn queued_denied(owner: PgPool, change: &str, kind: &str) {
    let f = Fixture::new(owner, Duration::minutes(10)).await;
    let mut stream = f.connect(None).await;
    let mut tx = f.block_authority().await;
    f.enqueue(f.event(kind)).await;
    match change {
        "family" => {
            sqlx::query("UPDATE auth_refresh_token_families SET revoked_at=now() WHERE id=$1")
                .bind(f.family)
                .execute(&mut *tx)
                .await
                .unwrap();
        }
        "account" => {
            sqlx::query("UPDATE users SET is_active=false WHERE id=$1")
                .bind(*f.user.as_uuid())
                .execute(&mut *tx)
                .await
                .unwrap();
        }
        "company" => {
            sqlx::query("UPDATE organizations SET status='SUSPENDED' WHERE id=$1")
                .bind(*OrgId::knl().as_uuid())
                .execute(&mut *tx)
                .await
                .unwrap();
        }
        "branch" => {
            sqlx::query("DELETE FROM user_branches WHERE user_id=$1")
                .bind(*f.user.as_uuid())
                .execute(&mut *tx)
                .await
                .unwrap();
        }
        "thread" => {
            sqlx::query("DELETE FROM messenger_thread_members WHERE user_id=$1")
                .bind(*f.user.as_uuid())
                .execute(&mut *tx)
                .await
                .unwrap();
        }
        _ => panic!("unknown change"),
    }
    tx.commit().await.unwrap();
    closed(&mut stream, 1008).await;
    f.assert_removed().await;
}
#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn family_logout_stops_queued_message(owner: PgPool) {
    queued_denied(owner, "family", "message").await;
}
#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn family_logout_stops_queued_ack(owner: PgPool) {
    queued_denied(owner, "family", "ack").await;
}
#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn family_logout_stops_queued_notification(owner: PgPool) {
    queued_denied(owner, "family", "notification").await;
}
#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn account_deactivation_stops_queued_content(owner: PgPool) {
    queued_denied(owner, "account", "message").await;
}
#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn company_suspension_stops_queued_content(owner: PgPool) {
    queued_denied(owner, "company", "message").await;
}
#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn branch_removal_stops_queued_content(owner: PgPool) {
    queued_denied(owner, "branch", "message").await;
}
#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn thread_removal_stops_queued_content(owner: PgPool) {
    queued_denied(owner, "thread", "message").await;
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn unchanged_authorized_notification_is_delivered(owner: PgPool) {
    let f = Fixture::new(owner, Duration::minutes(10)).await;
    let mut stream = f.connect(None).await;
    f.enqueue(f.event("notification")).await;
    let event = data(&mut stream).await;
    assert_eq!(event["notification"]["id"], f.notification.to_string());
    assert_eq!(event["notification"]["text"], "Current notice");
    f.hub.shutdown().await;
    closed(&mut stream, 1001).await;
}
#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn queued_message_reloads_owner_body_and_actor_ack(owner: PgPool) {
    let f = Fixture::new(owner, Duration::minutes(10)).await;
    let mut stream = f.connect(None).await;
    let mut tx = f.block_authority().await;
    f.enqueue(f.event("message")).await;
    sqlx::query("UPDATE messenger_messages SET body='Corrected current body' WHERE id=$1")
        .bind(*f.message.as_uuid())
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("INSERT INTO messenger_message_acks(message_id,user_id,org_id,acked_at) VALUES($1,$2,$3,now())")
        .bind(*f.message.as_uuid())
        .bind(*f.user.as_uuid())
        .bind(*OrgId::knl().as_uuid())
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let event = data(&mut stream).await;
    assert_eq!(event["message"]["body"], "Corrected current body");
    assert_eq!(event["message"]["ack_count"], 1);
    assert_eq!(event["message"]["acked_by_me"], true);
}
#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn queued_notification_reloads_current_owner_state(owner: PgPool) {
    let f = Fixture::new(owner, Duration::minutes(10)).await;
    let mut stream = f.connect(None).await;
    let mut tx = f.block_authority().await;
    f.enqueue(f.event("notification")).await;
    sqlx::query(
        "UPDATE notifications SET body='Corrected notice',unread=false,read_at=now() WHERE id=$1",
    )
    .bind(*f.notification.as_uuid())
    .execute(&mut *tx)
    .await
    .unwrap();
    sqlx::query("INSERT INTO notification_policies(org_id,user_id,scope) VALUES($1,$2,'all')")
        .bind(*OrgId::knl().as_uuid())
        .bind(*f.user.as_uuid())
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let event = data(&mut stream).await;
    assert_eq!(event["notification"]["text"], "Corrected notice");
    assert_eq!(event["notification"]["unread"], false);
    assert!(event["notification"]["read_at"].is_string());
    assert_eq!(event["notification"]["muted"], true);
}
#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn final_resolution_catches_revoke_during_owner_read(owner: PgPool) {
    let f = Fixture::new(owner, Duration::minutes(10)).await;
    let mut stream = f.connect(None).await;
    let mut tx = f.block_messages().await;
    f.enqueue(f.event("message")).await;
    f.wait_for_blocked_message_read().await;
    sqlx::query("UPDATE auth_refresh_token_families SET revoked_at=now() WHERE id=$1")
        .bind(f.family)
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    closed(&mut stream, 1008).await;
}
#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn replay_revalidates_after_revocation_commits(owner: PgPool) {
    let f = Fixture::new(owner, Duration::minutes(10)).await;
    let mut tx = f.block_messages().await;
    let mut stream = f.connect(Some(f.cursor)).await;
    sqlx::query("UPDATE auth_refresh_token_families SET revoked_at=now() WHERE id=$1")
        .bind(f.family)
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    closed(&mut stream, 1008).await;
}
#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn inaccessible_same_company_cursor_cannot_choose_replay_start(owner: PgPool) {
    let f = Fixture::new(owner, Duration::minutes(10)).await;
    let hidden = ThreadId::new();
    sqlx::query("INSERT INTO messenger_threads(id,org_id,kind,branch_id,created_by,visibility) VALUES($1,$2,'team',$3,$4,'direct')")
        .bind(*hidden.as_uuid()).bind(*OrgId::knl().as_uuid()).bind(*f.branch.as_uuid()).bind(*f.user.as_uuid()).execute(&f.owner).await.unwrap();
    sqlx::query("UPDATE messenger_messages SET thread_id=$1 WHERE id=$2")
        .bind(*hidden.as_uuid())
        .bind(*f.cursor.as_uuid())
        .execute(&f.owner)
        .await
        .unwrap();
    let mut stream = f.connect(Some(f.cursor)).await;
    closed(&mut stream, 1011).await;
}
#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn idle_socket_closes_at_its_signed_deadline(owner: PgPool) {
    let f = Fixture::new(owner, Duration::seconds(3)).await;
    let mut stream = f.connect(None).await;
    closed(&mut stream, 1008).await;
    f.assert_removed().await;
}
#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn expiry_interrupts_blocked_owner_preparation(owner: PgPool) {
    let f = Fixture::new(owner, Duration::seconds(4)).await;
    let mut stream = f.connect(None).await;
    let tx = f.block_messages().await;
    f.enqueue(f.event("message")).await;
    f.wait_for_blocked_message_read().await;
    closed(&mut stream, 1008).await;
    f.assert_removed().await;
    tx.rollback().await.unwrap();
}
#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn terminal_close_interrupts_owner_read_and_drops_queued_frames(owner: PgPool) {
    let f = Fixture::new(owner, Duration::minutes(10)).await;
    let mut stream = f.connect(None).await;
    let tx = f.block_messages().await;
    f.enqueue(f.event("message")).await;
    f.wait_for_blocked_message_read().await;
    f.enqueue(f.event("ack")).await;
    f.hub.shutdown().await;
    closed(&mut stream, 1001).await;
    f.assert_removed().await;
    tx.rollback().await.unwrap();
}
#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn unavailable_current_authority_never_uses_cached_payload(owner: PgPool) {
    let f = Fixture::new(owner, Duration::minutes(10)).await;
    let mut stream = f.connect(None).await;
    sqlx::query("REVOKE SELECT ON users FROM console_rt")
        .execute(&f.owner)
        .await
        .unwrap();
    f.enqueue(f.event("message")).await;
    closed(&mut stream, 1011).await;
}

async fn quote_projection(owner: PgPool, valid: bool) {
    let f = Fixture::new(owner, Duration::minutes(10)).await;
    if !valid {
        let hidden = ThreadId::new();
        sqlx::query("INSERT INTO messenger_threads(id,org_id,kind,branch_id,created_by,visibility) VALUES($1,$2,'team',$3,$4,'direct')")
            .bind(*hidden.as_uuid()).bind(*OrgId::knl().as_uuid()).bind(*f.branch.as_uuid()).bind(*f.user.as_uuid()).execute(&f.owner).await.unwrap();
        sqlx::query("UPDATE messenger_messages SET thread_id=$1 WHERE id=$2")
            .bind(*hidden.as_uuid())
            .bind(*f.cursor.as_uuid())
            .execute(&f.owner)
            .await
            .unwrap();
    }
    sqlx::query("UPDATE messenger_messages SET quoted_message_id=$1 WHERE id=$2")
        .bind(*f.cursor.as_uuid())
        .bind(*f.message.as_uuid())
        .execute(&f.owner)
        .await
        .unwrap();
    let mut event = f.event("message");
    if let RealtimeEvent::MessagePosted { message } = &mut event {
        message.quoted_message_id = Some(f.cursor);
        message.quoted_body = Some("cursor".into());
        message.quoted_sender_name = Some("Current sender".into());
    }
    let mut stream = f.connect(None).await;
    f.enqueue(event).await;
    let event = data(&mut stream).await;
    if valid {
        assert_eq!(event["message"]["quoted_message_id"], f.cursor.to_string());
        assert_eq!(event["message"]["quoted_body"], "cursor");
        assert_eq!(event["message"]["quoted_sender_name"], "Current sender");
    } else {
        assert!(event["message"]["quoted_message_id"].is_null());
        assert!(event["message"]["quoted_body"].is_null());
        assert!(event["message"]["quoted_sender_name"].is_null());
    }
}
#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn same_thread_quote_projection_is_preserved(owner: PgPool) {
    quote_projection(owner, true).await;
}
#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn cross_thread_quote_reference_and_preview_are_not_disclosed(owner: PgPool) {
    quote_projection(owner, false).await;
}

async fn no_blocked_replay_reads(f: &Fixture) {
    timeout(StdDuration::from_secs(7), async {
        loop {
            let waiting: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_locks l JOIN pg_stat_activity a USING(pid) WHERE a.datname=current_database() AND a.usename='console_rt' AND l.locktype='relation' AND l.relation='messenger_messages'::regclass AND NOT l.granted)")
                .fetch_one(&f.owner).await.unwrap();
            if !waiting { break; }
            tokio::task::yield_now().await;
        }
    }).await.expect("replay must not retain an orphaned PostgreSQL statement");
    assert_eq!(
        sqlx::query_scalar::<_, i32>("SELECT 1")
            .fetch_one(&f.runtime)
            .await
            .unwrap(),
        1
    );
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn replay_query_has_a_native_database_bound(owner: PgPool) {
    let f = Fixture::new(owner, Duration::minutes(10)).await;
    let tx = f.block_messages().await;
    let mut stream = f.connect(Some(f.cursor)).await;
    f.wait_for_blocked_message_read().await;
    closed(&mut stream, 1011).await;
    f.assert_removed().await;
    no_blocked_replay_reads(&f).await;
    tx.rollback().await.unwrap();
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn shutdown_closes_and_bounds_replay_query_cleanup(owner: PgPool) {
    let f = Fixture::new(owner, Duration::minutes(10)).await;
    let tx = f.block_messages().await;
    let mut stream = f.connect(Some(f.cursor)).await;
    f.wait_for_blocked_message_read().await;
    f.hub.shutdown().await;
    closed(&mut stream, 1001).await;
    no_blocked_replay_reads(&f).await;
    tx.rollback().await.unwrap();
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn unchanged_authorized_message_is_delivered(owner: PgPool) {
    let f = Fixture::new(owner, Duration::minutes(10)).await;
    let mut stream = f.connect(None).await;
    f.enqueue(f.event("message")).await;
    let event = data(&mut stream).await;
    assert_eq!(event["message"]["id"], f.message.to_string());
    assert_eq!(event["message"]["body"], "Current body");
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn queued_ack_reloads_its_current_count(owner: PgPool) {
    let f = Fixture::new(owner, Duration::minutes(10)).await;
    let mut stream = f.connect(None).await;
    let mut tx = f.block_authority().await;
    f.enqueue(f.event("ack")).await;
    sqlx::query("INSERT INTO messenger_message_acks(message_id,user_id,org_id,acked_at) VALUES($1,$2,$3,now())")
        .bind(*f.message.as_uuid())
        .bind(*f.user.as_uuid())
        .bind(*OrgId::knl().as_uuid())
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let event = data(&mut stream).await;
    assert_eq!(event["type"], "message_acked");
    assert_eq!(event["message_id"], f.message.to_string());
    assert_eq!(event["thread_id"], f.thread.to_string());
    assert_eq!(event["ack_count"], 1);
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn changed_notification_recipient_invalidates_the_queued_hint(owner: PgPool) {
    let f = Fixture::new(owner, Duration::minutes(10)).await;
    let other = seed_user(&f.owner, "Other recipient", "MECHANIC", f.branch).await;
    let mut stream = f.connect(None).await;
    let mut tx = f.block_authority().await;
    f.enqueue(f.event("notification")).await;
    sqlx::query("UPDATE notifications SET recipient_user_id=$1 WHERE id=$2")
        .bind(*other.as_uuid())
        .bind(*f.notification.as_uuid())
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    closed(&mut stream, 1008).await;
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn mismatched_message_thread_hint_cannot_be_emitted(owner: PgPool) {
    let f = Fixture::new(owner, Duration::minutes(10)).await;
    let other = ThreadId::new();
    sqlx::query("INSERT INTO messenger_threads(id,org_id,kind,branch_id,created_by,visibility) VALUES($1,$2,'team',$3,$4,'direct')")
        .bind(*other.as_uuid()).bind(*OrgId::knl().as_uuid()).bind(*f.branch.as_uuid()).bind(*f.user.as_uuid()).execute(&f.owner).await.unwrap();
    sqlx::query("INSERT INTO messenger_thread_members(thread_id,user_id,org_id,joined_at) VALUES($1,$2,$3,now())")
        .bind(*other.as_uuid()).bind(*f.user.as_uuid()).bind(*OrgId::knl().as_uuid()).execute(&f.owner).await.unwrap();
    let mut event = f.event("message");
    if let RealtimeEvent::MessagePosted { message } = &mut event {
        message.thread_id = other;
    }
    let mut stream = f.connect(None).await;
    f.enqueue(event).await;
    closed(&mut stream, 1008).await;
}
