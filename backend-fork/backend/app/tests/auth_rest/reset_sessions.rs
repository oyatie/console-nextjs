//! Credential recovery must revoke real signed sessions, atomically and in one Company.
use super::*;
use console_platform_auth::{RefreshRotation, RefreshTokenStore};
use sqlx::{Postgres, Transaction, postgres::PgPoolOptions};
use tokio::time::{sleep, timeout};

const RESET: &str = "/api/v1/auth/admin/credential-reset";
const WAIT: std::time::Duration = std::time::Duration::from_secs(10);

struct Fixture {
    router: axum::Router,
    runtime: PgPool,
    target: UserId,
    admin: String,
    session: TokenPairResponse,
    authenticator: WebauthnAuthenticator<SoftPasskey>,
    credential: String,
}

async fn runtime(owner: &PgPool, label: &str) -> PgPool {
    let pool = PgPoolOptions::new()
        .max_connections(4)
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
        "SELECT current_user::text,rolsuper,rolbypassrls FROM pg_roles WHERE rolname=current_user",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(identity, ("console_rt".into(), false, false));
    pool
}

async fn fixture(owner: &PgPool, label: &str) -> Fixture {
    let key = SigningKey::random(&mut OsRng);
    let rt = runtime(owner, label).await;
    let router = build_router(
        app_state(
            rt.clone(),
            key.to_pkcs8_pem(LineEnding::LF).unwrap().to_string(),
            key.verifying_key()
                .to_public_key_pem(LineEnding::LF)
                .unwrap(),
        )
        .unwrap(),
    );
    let branch = seed_branch(owner, "Recovery region", "Recovery branch").await;
    let admin = seed_user_with_branch(
        owner,
        "Recovery administrator",
        "010-8800-0000",
        "SUPER_ADMIN",
        branch,
    )
    .await;
    let target = seed_user_with_branch(
        owner,
        "Recovery subject",
        "010-8800-0001",
        "MECHANIC",
        branch,
    )
    .await;
    let admin = admin_session_via_otp(&router, &rt, admin).await;
    let bootstrap = admin_session_via_otp(&router, &rt, target).await;
    let mut authenticator = WebauthnAuthenticator::new(SoftPasskey::new(true));
    let credential = enroll_passkey(&router, &mut authenticator, &bootstrap).await;
    let session = usernameless_login(&router, &mut authenticator, &credential).await;
    assert_eq!(
        access_status(&router, &session.access_token).await,
        StatusCode::OK
    );
    Fixture {
        router,
        runtime: rt,
        target,
        admin,
        session,
        authenticator,
        credential,
    }
}

async fn access_status(router: &axum::Router, access: &str) -> StatusCode {
    router
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/api/v1/hr/attendance-records/me")
                .header(header::AUTHORIZATION, format!("Bearer {access}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
        .status()
}

async fn reset(f: &Fixture) -> http::Response<Body> {
    post_raw(
        f.router.clone(),
        RESET,
        Some(&f.admin),
        json!({"user_id": f.target.as_uuid()}),
    )
    .await
}

async fn family(owner: &PgPool, token: &str) -> Uuid {
    use sha2::{Digest, Sha256};
    sqlx::query_scalar("SELECT family_id FROM auth_refresh_tokens WHERE token_hash=$1")
        .bind(Sha256::digest(token.as_bytes()).to_vec())
        .fetch_one(owner)
        .await
        .unwrap()
}

async fn snapshot(owner: &PgPool) -> Value {
    sqlx::query_scalar("SELECT jsonb_build_object( \
        'families',(SELECT jsonb_agg(to_jsonb(f) ORDER BY id) FROM auth_refresh_token_families f), \
        'tokens',(SELECT jsonb_agg(to_jsonb(t) ORDER BY id) FROM auth_refresh_tokens t), \
        'keys',(SELECT jsonb_agg(to_jsonb(k) ORDER BY id) FROM auth_webauthn_credentials k), \
        'sources',(SELECT jsonb_agg(to_jsonb(b) ORDER BY id) FROM auth_bootstrap_credentials b), \
        'audits',(SELECT jsonb_agg(to_jsonb(a) ORDER BY id) FROM audit_events a), \
        'removals',(SELECT jsonb_agg(to_jsonb(r) ORDER BY event_id) FROM auth_security.credential_removals r))")
        .fetch_one(owner).await.unwrap()
}

async fn assert_dead(owner: &PgPool, f: &Fixture, session: &TokenPairResponse) {
    assert_eq!(
        access_status(&f.router, &session.access_token).await,
        StatusCode::UNAUTHORIZED,
        "pre-reset signed access must be denied"
    );
    let refresh = session.refresh_token.as_ref().unwrap();
    let response = post_raw(
        f.router.clone(),
        "/api/v1/auth/token/refresh",
        None,
        json!({"refresh_token": refresh}),
    )
    .await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let family = family(owner, refresh).await;
    let all_revoked: bool = sqlx::query_scalar(
        "SELECT f.revoked_at IS NOT NULL AND NOT EXISTS \
        (SELECT 1 FROM auth_refresh_tokens t WHERE t.family_id=f.id AND t.revoked_at IS NULL) \
        FROM auth_refresh_token_families f WHERE id=$1",
    )
    .bind(family)
    .fetch_one(owner)
    .await
    .unwrap();
    assert!(all_revoked, "every target family token must be revoked");
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn reset_revokes_signed_sessions_and_all_tokens_without_cross_account_effects(owner: PgPool) {
    let mut f = fixture(&owner, "reset-complete").await;
    let second = usernameless_login(&f.router, &mut f.authenticator, &f.credential).await;
    let first_refresh = f.session.refresh_token.as_ref().unwrap();
    let rotated: TokenPairResponse = post_json(
        f.router.clone(),
        "/api/v1/auth/token/refresh",
        None,
        json!({"refresh_token": first_refresh}),
        StatusCode::OK,
    )
    .await;
    let sibling = console_platform_test_support::seed_org_and_super_admin(
        &owner,
        *OrgId::knl().as_uuid(),
        "same-company",
    )
    .await;
    let foreign_org = OrgId::from_uuid(Uuid::new_v4());
    let foreign = console_platform_test_support::seed_org_and_super_admin(
        &owner,
        *foreign_org.as_uuid(),
        "foreign",
    )
    .await;
    let mut others = Vec::new();
    for (user, org) in [(sibling, OrgId::knl()), (foreign, foreign_org)] {
        let otp = BootstrapCredentialStore
            .issue_for_zero_credential_user(
                &f.runtime,
                *user.as_uuid(),
                org,
                OffsetDateTime::now_utc(),
                Duration::minutes(5),
            )
            .await
            .unwrap();
        let enrollment: OtpRedeemResponse = post_json(
            f.router.clone(),
            "/api/v1/auth/otp/redeem",
            None,
            json!({"otp":otp.token.as_str()}),
            StatusCode::OK,
        )
        .await;
        let mut auth = WebauthnAuthenticator::new(SoftPasskey::new(true));
        let key = enroll_passkey(&f.router, &mut auth, &enrollment.access_token).await;
        let session = usernameless_login(&f.router, &mut auth, &key).await;
        assert_eq!(
            access_status(&f.router, &session.access_token).await,
            StatusCode::OK
        );
        others.push(session);
    }
    let before = snapshot(&owner).await;
    let response = reset(&f).await;
    assert_eq!(response.status(), StatusCode::OK);
    let recovery: AdminCredentialResetResponse =
        serde_json::from_value(body_json(response).await).unwrap();
    // Check signed access first: this is the missing behavior on the clean base.
    assert_dead(&owner, &f, &f.session).await;
    assert_dead(&owner, &f, &second).await;
    assert_dead(&owner, &f, &rotated).await;
    assert_eq!(access_status(&f.router, &f.admin).await, StatusCode::OK);
    for session in &others {
        assert_eq!(
            access_status(&f.router, &session.access_token).await,
            StatusCode::OK
        );
    }
    let after = snapshot(&owner).await;
    for table in ["families", "tokens", "keys", "sources"] {
        let untouched = |state: &Value| {
            state[table]
                .as_array()
                .unwrap()
                .iter()
                .filter(|row| row["user_id"] != json!(f.target.as_uuid()))
                .cloned()
                .collect::<Vec<_>>()
        };
        assert_eq!(
            untouched(&before),
            untouched(&after),
            "unrelated {table} changed"
        );
    }
    let live: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM auth_refresh_token_families WHERE user_id=$1 AND revoked_at IS NULL",
    )
    .bind(f.target.as_uuid())
    .fetch_one(&owner)
    .await
    .unwrap();
    assert_eq!(live, 0);
    let audit: (Option<Uuid>, Value) = sqlx::query_as(
        "SELECT actor,after_snap FROM audit_events \
        WHERE action='auth.refresh.revoke_all' AND target_id=$1",
    )
    .bind(f.target.to_string())
    .fetch_one(&owner)
    .await
    .unwrap();
    assert_eq!(audit.0, None);
    assert_eq!(audit.1["reason"], "admin_reset");
    assert_eq!(audit.1["revoked_family_count"], 3);
    assert_eq!(audit.1["revoked_token_count"], 4);
    let recovered: OtpRedeemResponse = post_json(
        f.router.clone(),
        "/api/v1/auth/otp/redeem",
        None,
        json!({"otp":recovery.otp}),
        StatusCode::OK,
    )
    .await;
    let mut replacement = WebauthnAuthenticator::new(SoftPasskey::new(true));
    let key = enroll_passkey(&f.router, &mut replacement, &recovered.access_token).await;
    let fresh = usernameless_login(&f.router, &mut replacement, &key).await;
    assert_eq!(
        access_status(&f.router, &fresh.access_token).await,
        StatusCode::OK
    );
    assert_dead(&owner, &f, &f.session).await;
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn reset_with_no_keys_preserves_prior_revocation_and_sweeps_token_residue(owner: PgPool) {
    let f = fixture(&owner, "reset-residue").await;
    let family = family(&owner, f.session.refresh_token.as_ref().unwrap()).await;
    sqlx::query("DELETE FROM auth_webauthn_credentials WHERE user_id=$1")
        .bind(f.target.as_uuid())
        .execute(&owner)
        .await
        .unwrap();
    sqlx::query("UPDATE auth_refresh_token_families SET revoked_at=clock_timestamp()-interval '1 day',revoked_reason='logout' WHERE id=$1")
        .bind(family).execute(&owner).await.unwrap();
    let previous: Value =
        sqlx::query_scalar("SELECT to_jsonb(f) FROM auth_refresh_token_families f WHERE id=$1")
            .bind(family)
            .fetch_one(&owner)
            .await
            .unwrap();
    assert_eq!(reset(&f).await.status(), StatusCode::OK);
    let current: Value =
        sqlx::query_scalar("SELECT to_jsonb(f) FROM auth_refresh_token_families f WHERE id=$1")
            .bind(family)
            .fetch_one(&owner)
            .await
            .unwrap();
    assert_eq!(current, previous, "prior revocation metadata must survive");
    let live: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM auth_refresh_tokens WHERE user_id=$1 AND revoked_at IS NULL",
    )
    .bind(f.target.as_uuid())
    .fetch_one(&owner)
    .await
    .unwrap();
    assert_eq!(
        live, 0,
        "reset must revoke tokens under already-revoked families"
    );
    let before = snapshot(&owner).await;
    assert_eq!(reset(&f).await.status(), StatusCode::OK);
    let after = snapshot(&owner).await;
    assert_eq!(before["families"], after["families"]);
    assert_eq!(before["tokens"], after["tokens"]);
    let counts: Vec<Value> = sqlx::query_scalar(
        "SELECT after_snap FROM audit_events WHERE \
        action='auth.refresh.revoke_all' AND target_id=$1 ORDER BY occurred_at,id",
    )
    .bind(f.target.to_string())
    .fetch_all(&owner)
    .await
    .unwrap();
    assert_eq!(counts.len(), 2);
    assert_eq!(counts[1]["revoked_family_count"], 0);
    assert_eq!(counts[1]["revoked_token_count"], 0);
}

async fn waiting(owner: &PgPool, label: &str, blocker: i32) {
    timeout(WAIT, async {
        loop {
            let blocked: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE \
                datname=current_database() AND application_name=$1 AND wait_event_type='Lock' AND $2=ANY(pg_blocking_pids(pid)))")
                .bind(label).bind(blocker).fetch_one(owner).await.unwrap();
            if blocked { break; }
            sleep(std::time::Duration::from_millis(5)).await;
        }
    }).await.expect("real owner must wait on the specified transaction");
}

async fn transaction_pid(tx: &mut Transaction<'_, Postgres>) -> i32 {
    sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(tx.as_mut())
        .await
        .unwrap()
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn reset_first_blocks_refresh_and_prevents_replacement(owner: PgPool) {
    let f = fixture(&owner, "reset-first").await;
    sqlx::raw_sql(
        "CREATE FUNCTION test_reset_gate() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN \
        PERFORM pg_advisory_xact_lock(8294173); RETURN OLD; END $$; \
        CREATE TRIGGER test_reset_gate BEFORE DELETE ON auth_webauthn_credentials \
        FOR EACH ROW EXECUTE FUNCTION test_reset_gate()",
    )
    .execute(&owner)
    .await
    .unwrap();
    let mut gate = owner.begin().await.unwrap();
    sqlx::query("SELECT pg_advisory_xact_lock(8294173)")
        .execute(gate.as_mut())
        .await
        .unwrap();
    let gate_pid = transaction_pid(&mut gate).await;
    let router = f.router.clone();
    let admin = f.admin.clone();
    let target = f.target;
    let task = tokio::spawn(async move {
        post_raw(
            router,
            RESET,
            Some(&admin),
            json!({"user_id":target.as_uuid()}),
        )
        .await
    });
    waiting(&owner, "reset-first", gate_pid).await;
    let reset_pid: i32 = sqlx::query_scalar(
        "SELECT pid FROM pg_stat_activity WHERE datname=current_database() \
        AND application_name='reset-first' AND $1=ANY(pg_blocking_pids(pid))",
    )
    .bind(gate_pid)
    .fetch_one(&owner)
    .await
    .unwrap();
    let refresh_rt = runtime(&owner, "refresh-after-reset").await;
    let token = f.session.refresh_token.as_ref().unwrap().clone();
    let refresh = tokio::spawn(async move {
        RefreshTokenStore
            .rotate(
                &refresh_rt,
                &token,
                OffsetDateTime::now_utc(),
                Duration::minutes(10),
                Duration::days(1),
            )
            .await
    });
    waiting(&owner, "refresh-after-reset", reset_pid).await;
    gate.commit().await.unwrap();
    assert_eq!(
        timeout(WAIT, task).await.unwrap().unwrap().status(),
        StatusCode::OK
    );
    let rotated = timeout(WAIT, refresh).await.unwrap().unwrap();
    assert!(
        matches!(
            rotated,
            Err(console_platform_auth::RefreshTokenUseError::FamilyRevoked)
        ),
        "refresh queued behind reset must not issue a surviving replacement"
    );
    assert_dead(&owner, &f, &f.session).await;
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn rotation_first_replacement_is_revoked_by_waiting_reset(owner: PgPool) {
    let f = fixture(&owner, "reset-after-rotation").await;
    let mut tx = f.runtime.begin().await.unwrap();
    let rotated = RefreshTokenStore
        .rotate_in_tx(
            &mut tx,
            f.session.refresh_token.as_ref().unwrap(),
            OffsetDateTime::now_utc(),
            Duration::minutes(10),
            Duration::days(1),
        )
        .await
        .unwrap();
    let RefreshRotation::Issued(rotated) = rotated else {
        panic!("live rotation must issue");
    };
    let pid = transaction_pid(&mut tx).await;
    let router = f.router.clone();
    let admin = f.admin.clone();
    let target = f.target;
    let task = tokio::spawn(async move {
        post_raw(
            router,
            RESET,
            Some(&admin),
            json!({"user_id":target.as_uuid()}),
        )
        .await
    });
    waiting(&owner, "reset-after-rotation", pid).await;
    tx.commit().await.unwrap();
    assert_eq!(
        timeout(WAIT, task).await.unwrap().unwrap().status(),
        StatusCode::OK
    );
    let response = post_raw(
        f.router.clone(),
        "/api/v1/auth/token/refresh",
        None,
        json!({"refresh_token":rotated.token.as_str()}),
    )
    .await;
    assert_eq!(
        response.status(),
        StatusCode::UNAUTHORIZED,
        "replacement committed before reset must be revoked"
    );
    assert_dead(&owner, &f, &f.session).await;
}

async fn storage_fault(owner: &PgPool, table: &str) {
    let f = fixture(owner, "reset-storage-fault").await;
    sqlx::raw_sql(&format!("CREATE FUNCTION test_reset_storage_fault() RETURNS trigger LANGUAGE plpgsql AS $$ \
        BEGIN RAISE EXCEPTION 'reset_storage_failed'; END $$; CREATE TRIGGER test_reset_storage_fault \
        BEFORE UPDATE ON {table} FOR EACH ROW EXECUTE FUNCTION test_reset_storage_fault()"))
        .execute(owner).await.unwrap();
    let before = snapshot(owner).await;
    let response = reset(&f).await;
    assert_eq!(
        response.status(),
        StatusCode::INTERNAL_SERVER_ERROR,
        "session storage failure must refuse reset"
    );
    assert_eq!(
        snapshot(owner).await,
        before,
        "failed reset must roll back every credential/audit"
    );
    assert_eq!(
        access_status(&f.router, &f.session.access_token).await,
        StatusCode::OK
    );
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn family_storage_failure_rolls_back_reset(owner: PgPool) {
    storage_fault(&owner, "auth_refresh_token_families").await;
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn token_storage_failure_rolls_back_reset(owner: PgPool) {
    storage_fault(&owner, "auth_refresh_tokens").await;
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn otp_audit_and_deferred_commit_failure_roll_back_sessions_and_keys(owner: PgPool) {
    let f = fixture(&owner, "reset-late-fault").await;
    for (table, event, predicate, deferred) in [
        ("auth_bootstrap_credentials", "INSERT", "true", false),
        (
            "audit_events",
            "INSERT",
            "NEW.action='auth.refresh.revoke_all'",
            false,
        ),
        (
            "audit_events",
            "INSERT",
            "NEW.action='auth.otp.issue'",
            true,
        ),
    ] {
        let constraint = if deferred { "CONSTRAINT " } else { "" };
        let timing = if deferred { "AFTER" } else { "BEFORE" };
        let deferrable = if deferred {
            "DEFERRABLE INITIALLY DEFERRED"
        } else {
            ""
        };
        sqlx::raw_sql(&format!("CREATE FUNCTION test_reset_late_fault() RETURNS trigger LANGUAGE plpgsql AS $$ \
            BEGIN IF {predicate} THEN RAISE EXCEPTION 'reset_late_failed'; END IF; RETURN NEW; END $$; \
            CREATE {constraint}TRIGGER test_reset_late_fault {timing} {event} ON {table} {deferrable} \
            FOR EACH ROW EXECUTE FUNCTION test_reset_late_fault()"))
            .execute(&owner).await.unwrap();
        let before = snapshot(&owner).await;
        assert_eq!(
            reset(&f).await.status(),
            StatusCode::INTERNAL_SERVER_ERROR,
            "late credential/audit/commit failure must refuse reset"
        );
        assert_eq!(snapshot(&owner).await, before);
        assert_eq!(
            access_status(&f.router, &f.session.access_token).await,
            StatusCode::OK
        );
        sqlx::raw_sql(&format!(
            "DROP TRIGGER test_reset_late_fault ON {table}; DROP FUNCTION test_reset_late_fault()"
        ))
        .execute(&owner)
        .await
        .unwrap();
    }
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn reset_wrong_company_missing_and_inactive_targets_have_no_effects(owner: PgPool) {
    let f = fixture(&owner, "reset-scope").await;
    let foreign = console_platform_test_support::seed_org_and_super_admin(
        &owner,
        Uuid::new_v4(),
        "foreign-reset-target",
    )
    .await;
    for (target, org) in [
        (f.target, OrgId::from_uuid(Uuid::new_v4())),
        (foreign, OrgId::knl()),
        (UserId::new(), OrgId::knl()),
    ] {
        let before = snapshot(&owner).await;
        let result = BootstrapCredentialStore
            .reset_credentials_for_user(
                &f.runtime,
                *target.as_uuid(),
                org,
                OffsetDateTime::now_utc(),
                Duration::minutes(5),
            )
            .await;
        assert!(result.is_err());
        assert_eq!(snapshot(&owner).await, before);
    }
    sqlx::query("UPDATE users SET is_active=false WHERE id=$1")
        .bind(f.target.as_uuid())
        .execute(&owner)
        .await
        .unwrap();
    let before = snapshot(&owner).await;
    assert!(
        BootstrapCredentialStore
            .reset_credentials_for_user(
                &f.runtime,
                *f.target.as_uuid(),
                OrgId::knl(),
                OffsetDateTime::now_utc(),
                Duration::minutes(5)
            )
            .await
            .is_err()
    );
    assert_eq!(snapshot(&owner).await, before);
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn ordinary_otp_issue_and_self_handoff_do_not_revoke_established_sessions(owner: PgPool) {
    let f = fixture(&owner, "handoff-preserves-session").await;
    for handoff in [false, true] {
        let user = if handoff {
            f.target
        } else {
            console_platform_test_support::seed_org_and_super_admin(
                &owner,
                *OrgId::knl().as_uuid(),
                "zero-key",
            )
            .await
        };
        let family = RefreshTokenStore
            .issue_family(
                &f.runtime,
                *user.as_uuid(),
                OrgId::knl(),
                OffsetDateTime::now_utc(),
                Duration::minutes(5),
            )
            .await
            .unwrap();
        let before = snapshot(&owner).await;
        let store = BootstrapCredentialStore;
        if handoff {
            store
                .issue_self_enroll_handoff(
                    &f.runtime,
                    *user.as_uuid(),
                    OrgId::knl(),
                    OffsetDateTime::now_utc(),
                    Duration::minutes(5),
                )
                .await
                .unwrap();
        } else {
            store
                .issue_for_zero_credential_user(
                    &f.runtime,
                    *user.as_uuid(),
                    OrgId::knl(),
                    OffsetDateTime::now_utc(),
                    Duration::minutes(5),
                )
                .await
                .unwrap();
        }
        let after = snapshot(&owner).await;
        for table in ["families", "tokens"] {
            let state = |value: &Value| {
                value[table]
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter(|row| {
                        row["id"] == json!(family.family_id)
                            || row["family_id"] == json!(family.family_id)
                    })
                    .cloned()
                    .collect::<Vec<_>>()
            };
            assert_eq!(state(&before), state(&after));
        }
        assert_eq!(
            access_status(&f.router, &f.session.access_token).await,
            StatusCode::OK
        );
    }
}
