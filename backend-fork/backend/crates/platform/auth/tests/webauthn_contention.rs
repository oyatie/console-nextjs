//! Real signatures and observed PostgreSQL lock waits at the shared auth owner.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use console_kernel_core::OrgId;
use console_platform_auth::{
    MobilePasskeyStepUpAssertion, MobilePasskeyStepUpBinding, MobilePasskeyStepUpEnvelope,
    PasskeyAuthenticationCredential, PasskeyRegistrationCredential, PasskeyRegistrationStart,
    PasskeyService, StoredPasskey, WebauthnSettings,
};
use serde_json::Value;
use sqlx::{PgPool, Postgres, Transaction, postgres::PgPoolOptions};
use time::{Duration, OffsetDateTime};
use tokio::time::{sleep, timeout};
use url::Url;
use uuid::Uuid;
use webauthn_authenticator_rs::{WebauthnAuthenticator, softpasskey::SoftPasskey};

const WAIT: std::time::Duration = std::time::Duration::from_secs(8);

#[derive(Clone, Copy, Debug)]
enum Path {
    Login,
    StepUp,
    Mobile,
}

const PATHS: [Path; 3] = [Path::Login, Path::StepUp, Path::Mobile];

async fn runtime_pool(owner: &PgPool, label: &str) -> PgPool {
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .after_connect(|conn, _| {
            Box::pin(async move {
                sqlx::query("SET ROLE console_rt").execute(conn).await?;
                Ok(())
            })
        })
        .connect_with(
            owner
                .connect_options()
                .as_ref()
                .clone()
                .application_name(label),
        )
        .await
        .unwrap();
    let identity: (String, bool, bool) = sqlx::query_as(
        "SELECT current_user::text, rolsuper, rolbypassrls FROM pg_roles WHERE rolname=current_user",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(identity, ("console_rt".into(), false, false));
    pool
}

fn service() -> PasskeyService {
    PasskeyService::new(WebauthnSettings {
        rp_id: "example.com".into(),
        rp_origin: origin(),
        rp_name: "Console".into(),
        extra_allowed_origins: vec![],
        ceremony_ttl: Duration::minutes(5),
    })
    .unwrap()
}

fn origin() -> Url {
    Url::parse("https://auth.example.com").unwrap()
}

struct Fixture {
    owner: PgPool,
    runtime: PgPool,
    service: PasskeyService,
    user: Uuid,
    authenticator: WebauthnAuthenticator<SoftPasskey>,
}

impl Fixture {
    async fn new(owner: PgPool) -> Self {
        let runtime = runtime_pool(&owner, "contention-main").await;
        let user = sqlx::query_scalar(
            "INSERT INTO users(display_name,roles,org_id) VALUES ('Contention',ARRAY['MEMBER'],$1) RETURNING id",
        )
        .bind(*OrgId::knl().as_uuid())
        .fetch_one(&owner)
        .await
        .unwrap();
        Self {
            owner,
            runtime,
            service: service(),
            user,
            authenticator: WebauthnAuthenticator::new(SoftPasskey::new(true)),
        }
    }

    async fn registration(&mut self) -> (Uuid, PasskeyRegistrationCredential) {
        let start = self
            .service
            .start_registration(
                &self.runtime,
                OrgId::knl(),
                PasskeyRegistrationStart {
                    user_id: self.user,
                    username: self.user.to_string(),
                    display_name: "Contention".into(),
                },
            )
            .await
            .unwrap();
        let credential = self
            .authenticator
            .do_registration(origin(), start.challenge)
            .unwrap();
        (start.ceremony_id, credential)
    }

    async fn register(&mut self) -> StoredPasskey {
        let (id, credential) = self.registration().await;
        self.service
            .finish_registration(&self.runtime, OrgId::knl(), id, credential)
            .await
            .unwrap()
    }

    async fn assertion(&mut self, path: Path, key: &StoredPasskey) -> Attempt {
        self.assertion_with_uv(path, key, true).await
    }

    async fn assertion_with_uv(&mut self, path: Path, key: &StoredPasskey, uv: bool) -> Attempt {
        let binding = MobilePasskeyStepUpBinding::approval_decision(Uuid::new_v4(), Some(1));
        let start = match path {
            Path::Mobile => {
                self.service
                    .start_mobile_step_up(&self.runtime, self.user, binding.clone())
                    .await
            }
            _ => self.service.start_authentication(&self.runtime).await,
        }
        .unwrap();
        // SoftPasskey has no resident store; only the authenticator-facing hint
        // is populated. The server's stored challenge remains discoverable.
        let mut challenge = serde_json::to_value(start.challenge).unwrap();
        challenge["publicKey"]["allowCredentials"] = serde_json::json!([
            {"type":"public-key", "id":key.credential_id}
        ]);
        if !uv {
            challenge["publicKey"]["userVerification"] = serde_json::json!("discouraged");
        }
        let credential = self
            .authenticator
            .do_authentication(origin(), serde_json::from_value(challenge).unwrap())
            .unwrap();
        Attempt {
            path,
            id: start.ceremony_id,
            user: self.user,
            binding,
            credential,
        }
    }

    async fn key_state(&self, key: &StoredPasskey) -> Value {
        sqlx::query_scalar("SELECT to_jsonb(c) FROM auth_webauthn_credentials c WHERE id=$1")
            .bind(key.id)
            .fetch_one(&self.owner)
            .await
            .unwrap()
    }

    async fn unconsumed(&self, id: Uuid) {
        let consumed: Option<OffsetDateTime> =
            sqlx::query_scalar("SELECT consumed_at FROM auth_webauthn_ceremonies WHERE id=$1")
                .bind(id)
                .fetch_one(&self.owner)
                .await
                .unwrap();
        assert!(
            consumed.is_none(),
            "denied/rolled-back ceremony must stay unconsumed"
        );
    }

    async fn accepted_time(&self, id: Uuid, key: &StoredPasskey, registration: bool) {
        let (consumed, expires, created, used): (OffsetDateTime, OffsetDateTime, OffsetDateTime, Option<OffsetDateTime>) = sqlx::query_as(
            "SELECT c.consumed_at,c.expires_at,k.created_at,k.last_used_at FROM auth_webauthn_ceremonies c CROSS JOIN auth_webauthn_credentials k WHERE c.id=$1 AND k.id=$2",
        ).bind(id).bind(key.id).fetch_one(&self.owner).await.unwrap();
        assert!(consumed < expires, "acceptance is strictly before expiry");
        assert_eq!(
            if registration { Some(created) } else { used },
            Some(consumed)
        );
    }
}

#[derive(Clone)]
struct Attempt {
    path: Path,
    id: Uuid,
    user: Uuid,
    binding: MobilePasskeyStepUpBinding,
    credential: PasskeyAuthenticationCredential,
}

impl Attempt {
    async fn finish(self, pool: PgPool) -> Result<(), String> {
        let svc = service();
        match self.path {
            Path::Login => svc
                .finish_authentication(&pool, self.id, self.credential)
                .await
                .map(|_| ())
                .map_err(|e| e.to_string()),
            Path::StepUp => svc
                .verify_step_up_for_user(&pool, self.id, self.credential, self.user)
                .await
                .map_err(|e| e.to_string()),
            Path::Mobile => svc
                .verify_mobile_step_up_for_user(
                    &pool,
                    MobilePasskeyStepUpEnvelope {
                        binding: self.binding.clone(),
                        assertion: MobilePasskeyStepUpAssertion {
                            ceremony_id: self.id,
                            credential: self.credential,
                        },
                    },
                    self.user,
                    &self.binding,
                )
                .await
                .map_err(|e| e.to_string()),
        }
    }
}

async fn row_lock<'a>(
    owner: &'a PgPool,
    table: &str,
    id: Uuid,
) -> (Transaction<'a, Postgres>, i32) {
    assert!(
        [
            "users",
            "auth_webauthn_credentials",
            "auth_webauthn_ceremonies"
        ]
        .contains(&table)
    );
    let mut tx = owner.begin().await.unwrap();
    let sql = match table {
        "users" => "SELECT id FROM users WHERE id=$1 FOR UPDATE",
        "auth_webauthn_credentials" => {
            "SELECT id FROM auth_webauthn_credentials WHERE id=$1 FOR UPDATE"
        }
        "auth_webauthn_ceremonies" => {
            "SELECT id FROM auth_webauthn_ceremonies WHERE id=$1 FOR UPDATE"
        }
        _ => panic!("unsupported lock target"),
    };
    sqlx::query(sql).bind(id).fetch_one(&mut *tx).await.unwrap();
    let pid = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    (tx, pid)
}

async fn blocked(owner: &PgPool, label: &str, blocker: Option<i32>) {
    timeout(WAIT, async {
        loop {
            let found: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND application_name=$1 AND wait_event_type='Lock' AND cardinality(pg_blocking_pids(pid))>0 AND ($2::int IS NULL OR $2=ANY(pg_blocking_pids(pid))))",
            ).bind(label).bind(blocker).fetch_one(owner).await.unwrap();
            if found { break; }
            sleep(std::time::Duration::from_millis(5)).await;
        }
    }).await.expect("actual target transaction must reach the held lock");
}

async fn expire_soon(owner: &PgPool, id: Uuid) {
    sqlx::query("UPDATE auth_webauthn_ceremonies SET expires_at=clock_timestamp()+interval '2 seconds' WHERE id=$1")
        .bind(id).execute(owner).await.unwrap();
}

async fn past_deadline(owner: &PgPool, id: Uuid) {
    timeout(WAIT, async {
        loop {
            let expired: bool = sqlx::query_scalar(
                "SELECT clock_timestamp()>=expires_at FROM auth_webauthn_ceremonies WHERE id=$1",
            )
            .bind(id)
            .fetch_one(owner)
            .await
            .unwrap();
            if expired {
                break;
            }
            sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("DB clock crosses the stored expiry");
}

async fn assertion_expiry(owner: PgPool, path: Path, ceremony_lock: bool) {
    let mut f = Fixture::new(owner).await;
    let key = f.register().await;
    let attempt = f.assertion(path, &key).await;
    let id = attempt.id;
    let before = f.key_state(&key).await;
    expire_soon(&f.owner, id).await;
    let (table, lock_id) = if ceremony_lock {
        ("auth_webauthn_ceremonies", id)
    } else {
        ("auth_webauthn_credentials", key.id)
    };
    let (lock, pid) = row_lock(&f.owner, table, lock_id).await;
    let task = tokio::spawn(attempt.finish(f.runtime.clone()));
    blocked(&f.owner, "contention-main", Some(pid)).await;
    past_deadline(&f.owner, id).await;
    lock.rollback().await.unwrap();
    assert!(
        timeout(WAIT, task).await.unwrap().unwrap().is_err(),
        "{path:?} cannot admit an expired proof after {table} wait"
    );
    f.unconsumed(id).await;
    assert_eq!(f.key_state(&key).await, before);
}

macro_rules! expiry_test {
    ($name:ident, $path:ident, $ceremony:expr) => {
        #[sqlx::test(migrations = "../db/migrations")]
        async fn $name(owner: PgPool) {
            assertion_expiry(owner, Path::$path, $ceremony).await;
        }
    };
}

expiry_test!(login_expired_while_waiting_for_ceremony, Login, true);
expiry_test!(step_up_expired_while_waiting_for_ceremony, StepUp, true);
expiry_test!(mobile_expired_while_waiting_for_ceremony, Mobile, true);
expiry_test!(login_expired_while_waiting_for_key, Login, false);
expiry_test!(step_up_expired_while_waiting_for_key, StepUp, false);
expiry_test!(mobile_expired_while_waiting_for_key, Mobile, false);

async fn registration_expiry(owner: PgPool, ceremony_lock: bool) {
    let mut f = Fixture::new(owner).await;
    let (id, credential) = f.registration().await;
    expire_soon(&f.owner, id).await;
    let (table, lock_id) = if ceremony_lock {
        ("auth_webauthn_ceremonies", id)
    } else {
        ("users", f.user)
    };
    let (lock, pid) = row_lock(&f.owner, table, lock_id).await;
    let runtime = f.runtime.clone();
    let task = tokio::spawn(async move {
        service()
            .finish_registration(&runtime, OrgId::knl(), id, credential)
            .await
    });
    blocked(&f.owner, "contention-main", Some(pid)).await;
    past_deadline(&f.owner, id).await;
    lock.rollback().await.unwrap();
    assert!(timeout(WAIT, task).await.unwrap().unwrap().is_err());
    f.unconsumed(id).await;
    let keys: i64 =
        sqlx::query_scalar("SELECT count(*) FROM auth_webauthn_credentials WHERE user_id=$1")
            .bind(f.user)
            .fetch_one(&f.owner)
            .await
            .unwrap();
    assert_eq!(keys, 0);
    let audits: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_events WHERE action='auth.passkey.register' AND actor=$1",
    )
    .bind(f.user)
    .fetch_one(&f.owner)
    .await
    .unwrap();
    assert_eq!(audits, 0);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn registration_expired_while_waiting_for_ceremony(owner: PgPool) {
    registration_expiry(owner, true).await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn registration_expired_while_waiting_for_account(owner: PgPool) {
    registration_expiry(owner, false).await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn distinct_ceremonies_serialize_latest_key_across_all_paths(owner: PgPool) {
    let mut f = Fixture::new(owner).await;
    let key = f.register().await;
    let high_pool = runtime_pool(&f.owner, "contention-high").await;
    let low_pool = runtime_pool(&f.owner, "contention-low").await;
    for high_path in PATHS {
        for low_path in PATHS {
            let low = f.assertion(low_path, &key).await;
            let high = f.assertion(high_path, &key).await;
            let high_id = high.id;
            let low_id = low.id;
            let data = high.credential.response.authenticator_data.as_slice();
            let expected_counter = u32::from_be_bytes(data[33..37].try_into().unwrap());
            let (lock, pid) = row_lock(&f.owner, "auth_webauthn_credentials", key.id).await;
            let high_task = tokio::spawn(high.finish(high_pool.clone()));
            blocked(&f.owner, "contention-high", Some(pid)).await;
            let low_task = tokio::spawn(low.finish(low_pool.clone()));
            blocked(&f.owner, "contention-low", None).await;
            lock.rollback().await.unwrap();
            timeout(WAIT, high_task).await.unwrap().unwrap().unwrap();
            let low_result = timeout(WAIT, low_task).await.unwrap().unwrap();
            assert!(
                low_result.is_err(),
                "{high_path:?} then {low_path:?}: lower counter must be verified against the committed higher counter"
            );
            assert_eq!(
                f.key_state(&key).await["passkey_json"]["cred"]["counter"],
                expected_counter
            );
            f.accepted_time(high_id, &key, false).await;
            f.unconsumed(low_id).await;
        }
    }
}

#[sqlx::test(migrations = "../db/migrations")]
async fn credential_deleted_during_wait_cannot_authenticate(owner: PgPool) {
    let mut f = Fixture::new(owner).await;
    for path in PATHS {
        let key = f.register().await;
        let attempt = f.assertion(path, &key).await;
        let id = attempt.id;
        let (mut lock, pid) = row_lock(&f.owner, "auth_webauthn_credentials", key.id).await;
        let task = tokio::spawn(attempt.finish(f.runtime.clone()));
        blocked(&f.owner, "contention-main", Some(pid)).await;
        sqlx::query("DELETE FROM auth_webauthn_credentials WHERE id=$1")
            .bind(key.id)
            .execute(&mut *lock)
            .await
            .unwrap();
        lock.commit().await.unwrap();
        assert!(
            timeout(WAIT, task).await.unwrap().unwrap().is_err(),
            "{path:?}: deleted key cannot return successful proof"
        );
        f.unconsumed(id).await;
    }
}

#[sqlx::test(migrations = "../db/migrations")]
async fn cryptographic_failure_rolls_back_and_valid_retry_survives(owner: PgPool) {
    let mut f = Fixture::new(owner).await;
    let key = f.register().await;
    for path in PATHS {
        let attempt = f.assertion(path, &key).await;
        let before = f.key_state(&key).await;
        let mut invalid = attempt.clone();
        invalid.credential.response.signature = vec![0_u8; 64].into();
        assert!(invalid.finish(f.runtime.clone()).await.is_err());
        f.unconsumed(attempt.id).await;
        assert_eq!(f.key_state(&key).await, before);
        let id = attempt.id;
        attempt.finish(f.runtime.clone()).await.unwrap();
        f.accepted_time(id, &key, false).await;
    }
}

#[sqlx::test(migrations = "../db/migrations")]
async fn owner_handle_and_mobile_binding_checks_survive_shared_verifier(owner: PgPool) {
    let mut f = Fixture::new(owner).await;
    let key = f.register().await;
    for path in PATHS {
        let attempt = f.assertion(path, &key).await;
        let before = f.key_state(&key).await;
        let mut wrong_handle = attempt.clone();
        wrong_handle.credential.response.user_handle =
            Some(Uuid::new_v4().as_bytes().to_vec().into());
        assert!(wrong_handle.finish(f.runtime.clone()).await.is_err());
        if !matches!(path, Path::Login) {
            let mut wrong_owner = attempt.clone();
            wrong_owner.user = Uuid::new_v4();
            assert!(wrong_owner.finish(f.runtime.clone()).await.is_err());
        }
        if matches!(path, Path::Mobile) {
            let mut wrong_binding = attempt.clone();
            wrong_binding.binding.object_id = Uuid::new_v4();
            assert!(wrong_binding.finish(f.runtime.clone()).await.is_err());
            let result = f
                .service
                .verify_mobile_step_up_for_user(
                    &f.runtime,
                    MobilePasskeyStepUpEnvelope {
                        binding: MobilePasskeyStepUpBinding::poll_vote(Uuid::new_v4(), None),
                        assertion: MobilePasskeyStepUpAssertion {
                            ceremony_id: attempt.id,
                            credential: attempt.credential.clone(),
                        },
                    },
                    f.user,
                    &attempt.binding,
                )
                .await;
            assert!(result.is_err());
        }
        f.unconsumed(attempt.id).await;
        assert_eq!(f.key_state(&key).await, before);
        attempt.finish(f.runtime.clone()).await.unwrap();
    }
}

#[sqlx::test(migrations = "../db/migrations")]
async fn step_up_requires_real_signed_user_verification(owner: PgPool) {
    let mut f = Fixture::new(owner).await;
    let key = f.register().await;
    for path in [Path::StepUp, Path::Mobile] {
        let attempt = f.assertion_with_uv(path, &key, false).await;
        // A genuine signature with UV absent, not a tampered signed flag.
        assert_eq!(
            attempt.credential.response.authenticator_data.as_slice()[32] & 4,
            0
        );
        let id = attempt.id;
        let before = f.key_state(&key).await;
        assert!(attempt.finish(f.runtime.clone()).await.is_err());
        f.unconsumed(id).await;
        assert_eq!(f.key_state(&key).await, before);
        f.assertion(path, &key)
            .await
            .finish(f.runtime.clone())
            .await
            .unwrap();
    }
}

#[sqlx::test(migrations = "../db/migrations")]
async fn registration_uses_db_admission_time_and_caller_rollback(owner: PgPool) {
    let mut f = Fixture::new(owner).await;
    let (id, credential) = f.registration().await;
    let mut tx = f.runtime.begin().await.unwrap();
    let key = f
        .service
        .finish_registration_in_tx(
            &mut tx,
            OrgId::knl(),
            id,
            credential.clone(),
            OffsetDateTime::UNIX_EPOCH,
        )
        .await
        .unwrap();
    tx.rollback().await.unwrap();
    f.unconsumed(id).await;
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM auth_webauthn_credentials WHERE id=$1")
            .bind(key.id)
            .fetch_one(&f.owner)
            .await
            .unwrap();
    assert_eq!(count, 0);
    let audits: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_events WHERE action='auth.passkey.register' AND actor=$1",
    )
    .bind(f.user)
    .fetch_one(&f.owner)
    .await
    .unwrap();
    assert_eq!(audits, 0, "caller rollback also removes registration audit");
    let mut tx = f.runtime.begin().await.unwrap();
    let key = f
        .service
        .finish_registration_in_tx(
            &mut tx,
            OrgId::knl(),
            id,
            credential,
            OffsetDateTime::UNIX_EPOCH,
        )
        .await
        .unwrap();
    tx.commit().await.unwrap();
    f.accepted_time(id, &key, true).await;
    let created: OffsetDateTime =
        sqlx::query_scalar("SELECT created_at FROM auth_webauthn_credentials WHERE id=$1")
            .bind(key.id)
            .fetch_one(&f.owner)
            .await
            .unwrap();
    assert!(
        created > OffsetDateTime::UNIX_EPOCH + Duration::days(1),
        "legacy caller clock cannot backdate accepted key"
    );
    let audits: Vec<(Uuid, String, String, OffsetDateTime)> = sqlx::query_as(
        "SELECT actor,target_type,target_id,occurred_at FROM audit_events WHERE action='auth.passkey.register' AND actor=$1",
    )
    .bind(f.user)
    .fetch_all(&f.owner)
    .await
    .unwrap();
    assert_eq!(
        audits,
        vec![(
            f.user,
            "auth_webauthn_credential".into(),
            key.id.to_string(),
            created
        )],
        "one exact registration audit shares the accepted DB instant"
    );
}

#[sqlx::test(migrations = "../db/migrations")]
async fn cancelled_owning_verification_rolls_back_and_releases_locks(owner: PgPool) {
    let mut f = Fixture::new(owner).await;
    let key = f.register().await;
    for path in PATHS {
        let attempt = f.assertion(path, &key).await;
        let before = f.key_state(&key).await;
        let (lock, pid) = row_lock(&f.owner, "auth_webauthn_credentials", key.id).await;
        let task = tokio::spawn(attempt.clone().finish(f.runtime.clone()));
        blocked(&f.owner, "contention-main", Some(pid)).await;
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        // The SQL request may still be in flight. Releasing the independent
        // blocker permits SQLx's queued rollback; do not assert instant cancel.
        lock.rollback().await.unwrap();
        timeout(WAIT, sqlx::query("SELECT 1").execute(&f.runtime))
            .await
            .unwrap()
            .unwrap();
        let mut observer = f.owner.begin().await.unwrap();
        sqlx::query("SELECT id FROM auth_webauthn_ceremonies WHERE id=$1 FOR UPDATE NOWAIT")
            .bind(attempt.id)
            .fetch_one(&mut *observer)
            .await
            .unwrap();
        observer.rollback().await.unwrap();
        f.unconsumed(attempt.id).await;
        assert_eq!(f.key_state(&key).await, before);
        attempt.finish(f.runtime.clone()).await.unwrap();
    }
}

#[sqlx::test(migrations = "../db/migrations")]
async fn expired_proofs_cannot_be_consumed_on_any_path(owner: PgPool) {
    let mut f = Fixture::new(owner).await;
    let key = f.register().await;
    for path in PATHS {
        let attempt = f.assertion(path, &key).await;
        sqlx::query("UPDATE auth_webauthn_ceremonies SET expires_at=clock_timestamp() WHERE id=$1")
            .bind(attempt.id)
            .execute(&f.owner)
            .await
            .unwrap();
        let id = attempt.id;
        let before = f.key_state(&key).await;
        assert!(attempt.finish(f.runtime.clone()).await.is_err());
        f.unconsumed(id).await;
        assert_eq!(f.key_state(&key).await, before);
    }
    let (id, credential) = f.registration().await;
    sqlx::query("UPDATE auth_webauthn_ceremonies SET expires_at=clock_timestamp() WHERE id=$1")
        .bind(id)
        .execute(&f.owner)
        .await
        .unwrap();
    assert!(
        f.service
            .finish_registration(&f.runtime, OrgId::knl(), id, credential)
            .await
            .is_err()
    );
    f.unconsumed(id).await;
}

// The actual Account mutex must conflict with another NO KEY UPDATE holder,
// not only DELETE/FOR UPDATE; KEY SHARE alone is not a serialization mutex.
async fn account_mutex(owner: &PgPool, user: Uuid) -> (Transaction<'_, Postgres>, i32) {
    let mut tx = owner.begin().await.unwrap();
    sqlx::query("SELECT id FROM users WHERE id=$1 FOR NO KEY UPDATE")
        .bind(user)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    let pid = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    (tx, pid)
}

// Authentication ownership transition: Account precedes every proof/key lock.
async fn assertion_account_expiry(owner: PgPool, path: Path) {
    let mut f = Fixture::new(owner).await;
    let key = f.register().await;
    let attempt = f.assertion(path, &key).await;
    let before = f.key_state(&key).await;
    let id = attempt.id;
    expire_soon(&f.owner, id).await;
    let (held, pid) = account_mutex(&f.owner, f.user).await;
    let task = tokio::spawn(attempt.finish(f.runtime.clone()));
    blocked(&f.owner, "contention-main", Some(pid)).await;
    past_deadline(&f.owner, id).await;
    held.rollback().await.unwrap();
    assert!(timeout(WAIT, task).await.unwrap().unwrap().is_err());
    f.unconsumed(id).await;
    assert_eq!(f.key_state(&key).await, before);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn login_expired_while_waiting_for_account(owner: PgPool) {
    assertion_account_expiry(owner, Path::Login).await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn step_up_expired_while_waiting_for_account(owner: PgPool) {
    assertion_account_expiry(owner, Path::StepUp).await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn mobile_expired_while_waiting_for_account(owner: PgPool) {
    assertion_account_expiry(owner, Path::Mobile).await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn assertions_wait_for_account_without_holding_proof_or_key_locks(owner: PgPool) {
    let mut f = Fixture::new(owner).await;
    let key = f.register().await;
    for path in PATHS {
        let attempt = f.assertion(path, &key).await;
        let id = attempt.id;
        let (held, pid) = account_mutex(&f.owner, f.user).await;
        let task = tokio::spawn(attempt.finish(f.runtime.clone()));
        blocked(&f.owner, "contention-main", Some(pid)).await;
        let mut observer = f.owner.begin().await.unwrap();
        sqlx::query("SELECT id FROM auth_webauthn_ceremonies WHERE id=$1 FOR UPDATE NOWAIT")
            .bind(id)
            .fetch_one(&mut *observer)
            .await
            .unwrap();
        sqlx::query("SELECT id FROM auth_webauthn_credentials WHERE id=$1 FOR UPDATE NOWAIT")
            .bind(key.id)
            .fetch_one(&mut *observer)
            .await
            .unwrap();
        observer.rollback().await.unwrap();
        held.rollback().await.unwrap();
        timeout(WAIT, task).await.unwrap().unwrap().unwrap();
        f.accepted_time(id, &key, false).await;
    }
}

#[sqlx::test(migrations = "../db/migrations")]
async fn registration_waits_for_account_without_holding_ceremony(owner: PgPool) {
    let mut f = Fixture::new(owner).await;
    let (id, signed) = f.registration().await;
    let (held, pid) = account_mutex(&f.owner, f.user).await;
    let runtime = f.runtime.clone();
    let task = tokio::spawn(async move {
        service()
            .finish_registration(&runtime, OrgId::knl(), id, signed)
            .await
    });
    blocked(&f.owner, "contention-main", Some(pid)).await;
    let mut observer = f.owner.begin().await.unwrap();
    sqlx::query("SELECT id FROM auth_webauthn_ceremonies WHERE id=$1 FOR UPDATE NOWAIT")
        .bind(id)
        .fetch_one(&mut *observer)
        .await
        .unwrap();
    observer.rollback().await.unwrap();
    held.rollback().await.unwrap();
    let key = timeout(WAIT, task).await.unwrap().unwrap().unwrap();
    f.accepted_time(id, &key, true).await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn account_deactivation_while_waiting_denies_every_assertion(owner: PgPool) {
    for path in PATHS {
        let mut f = Fixture::new(owner.clone()).await;
        let key = f.register().await;
        let attempt = f.assertion(path, &key).await;
        let id = attempt.id;
        let before = f.key_state(&key).await;
        let (mut held, pid) = account_mutex(&f.owner, f.user).await;
        let task = tokio::spawn(attempt.finish(f.runtime.clone()));
        blocked(&f.owner, "contention-main", Some(pid)).await;
        // The auth owner must independently deny inactive Accounts. The
        // later lifecycle owner may also remove keys in this transaction.
        sqlx::query("UPDATE users SET is_active=false WHERE id=$1")
            .bind(f.user)
            .execute(&mut *held)
            .await
            .unwrap();
        held.commit().await.unwrap();
        assert!(timeout(WAIT, task).await.unwrap().unwrap().is_err());
        f.unconsumed(id).await;
        let current: Option<Value> =
            sqlx::query_scalar("SELECT to_jsonb(c) FROM auth_webauthn_credentials c WHERE id=$1")
                .bind(key.id)
                .fetch_optional(&f.owner)
                .await
                .unwrap();
        assert!(
            current.is_none() || current == Some(before),
            "denied proof cannot mutate a surviving key; lifecycle removal is permitted"
        );
    }
}

#[sqlx::test(migrations = "../db/migrations")]
async fn vanished_credential_locator_is_not_a_locked_key_fallback(owner: PgPool) {
    let mut f = Fixture::new(owner).await;
    for path in PATHS {
        let key = f.register().await;
        let attempt = f.assertion(path, &key).await;
        let id = attempt.id;
        let (held, pid) = account_mutex(&f.owner, f.user).await;
        let task = tokio::spawn(attempt.finish(f.runtime.clone()));
        blocked(&f.owner, "contention-main", Some(pid)).await;
        let mut remover = f.owner.begin().await.unwrap();
        sqlx::query("SET LOCAL lock_timeout='500ms'")
            .execute(&mut *remover)
            .await
            .unwrap();
        sqlx::query("DELETE FROM auth_webauthn_credentials WHERE id=$1")
            .bind(key.id)
            .execute(&mut *remover)
            .await
            .unwrap();
        remover.commit().await.unwrap();
        held.rollback().await.unwrap();
        assert!(timeout(WAIT, task).await.unwrap().unwrap().is_err());
        f.unconsumed(id).await;
    }
}

#[sqlx::test(migrations = "../db/migrations")]
async fn changed_registration_locator_cannot_switch_locked_account(owner: PgPool) {
    let mut f = Fixture::new(owner).await;
    let other = Fixture::new(f.owner.clone()).await;
    let (id, signed) = f.registration().await;
    let (held, pid) = account_mutex(&f.owner, f.user).await;
    let runtime = f.runtime.clone();
    let task = tokio::spawn(async move {
        service()
            .finish_registration(&runtime, OrgId::knl(), id, signed)
            .await
    });
    blocked(&f.owner, "contention-main", Some(pid)).await;
    let mut rebinder = f.owner.begin().await.unwrap();
    sqlx::query("SET LOCAL lock_timeout='500ms'")
        .execute(&mut *rebinder)
        .await
        .unwrap();
    sqlx::query("UPDATE auth_webauthn_ceremonies SET user_id=$1 WHERE id=$2")
        .bind(other.user)
        .bind(id)
        .execute(&mut *rebinder)
        .await
        .unwrap();
    rebinder.commit().await.unwrap();
    held.rollback().await.unwrap();
    assert!(timeout(WAIT, task).await.unwrap().unwrap().is_err());
    f.unconsumed(id).await;
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM auth_webauthn_credentials WHERE user_id=ANY($1)")
            .bind(vec![f.user, other.user])
            .fetch_one(&f.owner)
            .await
            .unwrap();
    assert_eq!(count, 0, "a stale locator cannot enroll either Account");
}

#[sqlx::test(migrations = "../db/migrations")]
async fn proof_account_mutex_is_compatible_with_child_fk_key_share(owner: PgPool) {
    let mut f = Fixture::new(owner).await;
    let mut fk = f.owner.begin().await.unwrap();
    sqlx::query("SELECT id FROM users WHERE id=$1 FOR KEY SHARE")
        .bind(f.user)
        .fetch_one(&mut *fk)
        .await
        .unwrap();
    sqlx::query("SET lock_timeout='500ms'")
        .execute(&f.runtime)
        .await
        .unwrap();
    let key = f.register().await;
    for path in PATHS {
        f.assertion(path, &key)
            .await
            .finish(f.runtime.clone())
            .await
            .unwrap();
    }
    fk.rollback().await.unwrap();
}

#[sqlx::test(migrations = "../db/migrations")]
async fn same_account_assertions_serialize_distinct_keys_but_other_accounts_progress(
    owner: PgPool,
) {
    let mut f = Fixture::new(owner).await;
    let first = f.register().await;
    let second = f.register().await;
    let mut other = Fixture::new(f.owner.clone()).await;
    let other_key = other.register().await;
    let a = f.assertion(Path::Login, &first).await;
    let b = f.assertion(Path::Login, &second).await;
    let second_ceremony = b.id;
    let second_pool = runtime_pool(&f.owner, "contention-second-account-proof").await;
    let first_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&f.runtime)
        .await
        .unwrap();
    let (held, pid) = row_lock(&f.owner, "auth_webauthn_credentials", first.id).await;
    let first_task = tokio::spawn(a.finish(f.runtime.clone()));
    blocked(&f.owner, "contention-main", Some(pid)).await;
    let second_task = tokio::spawn(b.finish(second_pool));
    blocked(&f.owner, "contention-second-account-proof", Some(first_pid)).await;
    let mut observer = f.owner.begin().await.unwrap();
    sqlx::query("SELECT id FROM auth_webauthn_ceremonies WHERE id=$1 FOR UPDATE NOWAIT")
        .bind(second_ceremony)
        .fetch_one(&mut *observer)
        .await
        .unwrap();
    sqlx::query("SELECT id FROM auth_webauthn_credentials WHERE id=$1 FOR UPDATE NOWAIT")
        .bind(second.id)
        .fetch_one(&mut *observer)
        .await
        .unwrap();
    observer.rollback().await.unwrap();
    timeout(
        WAIT,
        other
            .assertion(Path::Login, &other_key)
            .await
            .finish(other.runtime.clone()),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(!first_task.is_finished());
    assert!(!second_task.is_finished());
    held.rollback().await.unwrap();
    timeout(WAIT, first_task).await.unwrap().unwrap().unwrap();
    timeout(WAIT, second_task).await.unwrap().unwrap().unwrap();
}

#[sqlx::test(migrations = "../db/migrations")]
async fn replaced_assertion_locator_cannot_adopt_another_account(owner: PgPool) {
    for path in PATHS {
        let mut f = Fixture::new(owner.clone()).await;
        let other = Fixture::new(owner.clone()).await;
        let key = f.register().await;
        let attempt = f.assertion(path, &key).await;
        let ceremony = attempt.id;
        let original = f.key_state(&key).await;
        let (held, pid) = account_mutex(&f.owner, f.user).await;
        let task = tokio::spawn(attempt.finish(f.runtime.clone()));
        blocked(&f.owner, "contention-main", Some(pid)).await;
        let mut replacement = f.owner.begin().await.unwrap();
        sqlx::query("SET LOCAL lock_timeout='500ms'")
            .execute(&mut *replacement)
            .await
            .unwrap();
        sqlx::query("DELETE FROM auth_webauthn_credentials WHERE id=$1")
            .bind(key.id)
            .execute(&mut *replacement)
            .await
            .unwrap();
        let replacement_id = Uuid::new_v4();
        // Privileged fixture simulates a concurrent locator replacement. The
        // original real public key is retained; no signature is fabricated.
        sqlx::query("INSERT INTO auth_webauthn_credentials(id,user_id,credential_id,passkey_json,org_id) VALUES($1,$2,$3,$4,$5)")
            .bind(replacement_id).bind(other.user).bind(&key.credential_id)
            .bind(&original["passkey_json"]).bind(*OrgId::knl().as_uuid())
            .execute(&mut *replacement).await.unwrap();
        replacement.commit().await.unwrap();
        let before: Value =
            sqlx::query_scalar("SELECT to_jsonb(c) FROM auth_webauthn_credentials c WHERE id=$1")
                .bind(replacement_id)
                .fetch_one(&f.owner)
                .await
                .unwrap();
        held.rollback().await.unwrap();
        assert!(timeout(WAIT, task).await.unwrap().unwrap().is_err());
        f.unconsumed(ceremony).await;
        let after: Value =
            sqlx::query_scalar("SELECT to_jsonb(c) FROM auth_webauthn_credentials c WHERE id=$1")
                .bind(replacement_id)
                .fetch_one(&f.owner)
                .await
                .unwrap();
        assert_eq!(
            after, before,
            "no key use may be attributed to the new Account"
        );
    }
}
