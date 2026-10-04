//! UPDATE-only credential identity protection; not authentication command isolation.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::borrow::Cow;

use console_kernel_core::OrgId;
use console_platform_auth::{
    PasskeyRegistrationStart, PasskeyService, StoredPasskey, WebauthnSettings,
};
use serde_json::{Value, json};
use sqlx::{PgPool, postgres::PgPoolOptions};
use time::Duration;
use url::Url;
use uuid::Uuid;
use webauthn_authenticator_rs::{WebauthnAuthenticator, softpasskey::SoftPasskey};

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("../db/migrations");

struct Fixture {
    owner: PgPool,
    runtime: PgPool,
    service: PasskeyService,
    user: Uuid,
    key: StoredPasskey,
    authenticator: WebauthnAuthenticator<SoftPasskey>,
}

fn origin() -> Url {
    Url::parse("https://auth.example.com").unwrap()
}

impl Fixture {
    async fn new(owner: PgPool) -> Self {
        // Actual restricted current_user, using inherited test-owner connection.
        // This proves trigger/RLS enforcement, NOT password separation.
        let runtime = PgPoolOptions::new()
            .max_connections(2)
            .after_connect(|conn, _| {
                Box::pin(async move {
                    sqlx::query("SET ROLE console_rt")
                        .execute(&mut *conn)
                        .await?;
                    sqlx::query("SELECT set_config('app.current_org',$1,false)")
                        .bind(OrgId::knl().to_string())
                        .execute(conn)
                        .await?;
                    Ok(())
                })
            })
            .connect_with(owner.connect_options().as_ref().clone())
            .await
            .unwrap();
        let identity: (String, bool, bool) = sqlx::query_as(
            "SELECT current_user::text,rolsuper,rolbypassrls FROM pg_roles WHERE rolname=current_user"
        ).fetch_one(&runtime).await.unwrap();
        assert_eq!(identity, ("console_rt".into(), false, false));
        let user = sqlx::query_scalar("INSERT INTO users(display_name,roles,org_id) VALUES ('Credential guard',ARRAY['MEMBER'],$1) RETURNING id")
            .bind(*OrgId::knl().as_uuid()).fetch_one(&owner).await.unwrap();
        let service = PasskeyService::new(WebauthnSettings {
            rp_id: "example.com".into(),
            rp_origin: origin(),
            rp_name: "Console".into(),
            extra_allowed_origins: vec![],
            ceremony_ttl: Duration::minutes(5),
        })
        .unwrap();
        let mut authenticator = WebauthnAuthenticator::new(SoftPasskey::new(true));
        let start = service
            .start_registration(
                &runtime,
                OrgId::knl(),
                PasskeyRegistrationStart {
                    user_id: user,
                    username: user.to_string(),
                    display_name: "Credential guard".into(),
                },
            )
            .await
            .unwrap();
        let signed = authenticator
            .do_registration(origin(), start.challenge)
            .unwrap();
        let key = service
            .finish_registration(&runtime, OrgId::knl(), start.ceremony_id, signed)
            .await
            .unwrap();
        Self {
            owner,
            runtime,
            service,
            user,
            key,
            authenticator,
        }
    }

    async fn state(&self) -> Value {
        sqlx::query_scalar("SELECT to_jsonb(c) FROM auth_webauthn_credentials c WHERE id=$1")
            .bind(self.key.id)
            .fetch_one(&self.owner)
            .await
            .unwrap()
    }

    async fn replace(&self, value: Value) -> Result<sqlx::postgres::PgQueryResult, sqlx::Error> {
        sqlx::query("UPDATE auth_webauthn_credentials SET passkey_json=$1,last_used_at=clock_timestamp() WHERE id=$2")
            .bind(value).bind(self.key.id).execute(&self.runtime).await
    }

    async fn deny_json(&self, value: Value) {
        let before = self.state().await;
        let error = self
            .replace(value)
            .await
            .expect_err("invalid UPDATE must be rejected");
        assert_eq!(
            error.as_database_error().unwrap().code().as_deref(),
            Some("23514")
        );
        assert_eq!(
            self.state().await,
            before,
            "denial must preserve the entire key"
        );
    }

    async fn authenticate(&mut self, step_up: bool) -> Result<(), String> {
        let start = self
            .service
            .start_authentication(&self.runtime)
            .await
            .unwrap();
        let mut challenge = serde_json::to_value(start.challenge).unwrap();
        // SoftPasskey requires a hint; the stored server ceremony stays discoverable.
        challenge["publicKey"]["allowCredentials"] =
            json!([{"type":"public-key","id":self.key.credential_id}]);
        let signed = self
            .authenticator
            .do_authentication(origin(), serde_json::from_value(challenge).unwrap())
            .unwrap();
        let result = if step_up {
            self.service
                .verify_step_up_for_user(&self.runtime, start.ceremony_id, signed, self.user)
                .await
        } else {
            self.service
                .finish_authentication(&self.runtime, start.ceremony_id, signed)
                .await
                .map(|_| ())
        };
        if result.is_err() {
            let consumed: bool = sqlx::query_scalar(
                "SELECT consumed_at IS NOT NULL FROM auth_webauthn_ceremonies WHERE id=$1",
            )
            .bind(start.ceremony_id)
            .fetch_one(&self.owner)
            .await
            .unwrap();
            assert!(
                !consumed,
                "denied crypto/storage result cannot consume proof"
            );
        }
        result.map_err(|e| e.to_string())
    }
}

fn immutable(mut row: Value) -> Value {
    row.as_object_mut().unwrap().remove("last_used_at");
    for field in ["counter", "backup_state", "backup_eligible"] {
        row["passkey_json"]["cred"]
            .as_object_mut()
            .unwrap()
            .remove(field);
    }
    row
}

#[sqlx::test(migrations = "../db/migrations")]
async fn genuine_registration_login_and_step_up_preserve_key_identity(owner: PgPool) {
    let mut f = Fixture::new(owner).await;
    let before = f.state().await;
    for step_up in [false, true] {
        f.authenticate(step_up).await.unwrap();
        let after = f.state().await;
        assert_eq!(immutable(before.clone()), immutable(after.clone()));
        assert!(
            after["passkey_json"]["cred"]["counter"].as_u64().unwrap()
                > before["passkey_json"]["cred"]["counter"].as_u64().unwrap()
        );
        assert!(!after["last_used_at"].is_null());
    }
}

#[sqlx::test(migrations = "../db/migrations")]
async fn runtime_cannot_reassign_row_identity(owner: PgPool) {
    let f = Fixture::new(owner).await;
    let other: Uuid = sqlx::query_scalar("INSERT INTO users(display_name,roles,org_id) VALUES ('Other',ARRAY['MEMBER'],$1) RETURNING id")
        .bind(*OrgId::knl().as_uuid()).fetch_one(&f.owner).await.unwrap();
    for sql in [
        "UPDATE auth_webauthn_credentials SET id=gen_random_uuid() WHERE id=$1 AND $2::uuid IS NOT NULL",
        "UPDATE auth_webauthn_credentials SET user_id=$2 WHERE id=$1",
        "UPDATE auth_webauthn_credentials SET org_id='ffffffff-ffff-ffff-ffff-ffffffffffff' WHERE id=$1 AND $2::uuid IS NOT NULL",
        "UPDATE auth_webauthn_credentials SET credential_id=credential_id || '-different' WHERE id=$1 AND $2::uuid IS NOT NULL",
        "UPDATE auth_webauthn_credentials SET created_at=created_at+interval '1 second' WHERE id=$1 AND $2::uuid IS NOT NULL",
    ] {
        let before = f.state().await;
        let result = sqlx::query(sql)
            .bind(f.key.id)
            .bind(other)
            .execute(&f.runtime)
            .await;
        let error = result.expect_err("immutable row update must be rejected");
        assert_eq!(
            error.as_database_error().unwrap().code().as_deref(),
            Some("23514"),
            "immutable row field: {sql}"
        );
        assert_eq!(f.state().await, before);
    }
}

#[sqlx::test(migrations = "../db/migrations")]
async fn immutable_json_and_unknown_shape_updates_are_denied(owner: PgPool) {
    let mut f = Fixture::new(owner).await;
    let original = f.state().await["passkey_json"].clone();
    for field in [
        "cred_id",
        "cred",
        "transports",
        "user_verified",
        "registration_policy",
        "extensions",
        "attestation",
        "attestation_format",
    ] {
        let mut changed = original.clone();
        changed["cred"][field] = json!({"substitution": true});
        f.deny_json(changed).await;
        let mut missing = original.clone();
        missing["cred"].as_object_mut().unwrap().remove(field);
        f.deny_json(missing).await;
    }
    for changed in [
        json!({}),
        json!([]),
        json!(null),
        json!({"cred": []}),
        {
            let mut v = original.clone();
            v["rp_id"] = json!("other.example");
            v
        },
        {
            let mut v = original.clone();
            v["cred"]["future_codec"] = json!(6);
            v
        },
    ] {
        f.deny_json(changed).await;
    }
    let other = Fixture::new(f.owner.clone()).await;
    let mut substituted = original;
    substituted["cred"]["cred"] = other.state().await["passkey_json"]["cred"]["cred"].clone();
    f.deny_json(substituted).await;
    f.authenticate(false).await.unwrap();
}

#[sqlx::test(migrations = "../db/migrations")]
async fn counter_is_exact_uint32_and_never_decreases(owner: PgPool) {
    let f = Fixture::new(owner).await;
    let mut valid = f.state().await["passkey_json"].clone();
    for count in [0_u64, 1, 1, u64::from(u32::MAX)] {
        valid["cred"]["counter"] = json!(count);
        assert_eq!(f.replace(valid.clone()).await.unwrap().rows_affected(), 1);
    }
    for count in [
        json!(0),
        json!(-1),
        json!(1.5),
        json!("4294967295"),
        json!(4294967296_u64),
        json!(null),
        json!(true),
        json!({}),
    ] {
        let mut invalid = valid.clone();
        invalid["cred"]["counter"] = count;
        f.deny_json(invalid).await;
    }
    valid["cred"].as_object_mut().unwrap().remove("counter");
    f.deny_json(valid).await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn backup_state_can_clear_but_eligibility_cannot(owner: PgPool) {
    let f = Fixture::new(owner).await;
    let mut value = f.state().await["passkey_json"].clone();
    assert_eq!(value["cred"]["backup_eligible"], false);
    value["cred"]["backup_state"] = json!(true);
    f.deny_json(value.clone()).await;
    value["cred"]["backup_eligible"] = json!(true);
    for state in [true, false, true] {
        value["cred"]["backup_state"] = json!(state);
        f.replace(value.clone()).await.unwrap();
    }
    let mut downgrade = value.clone();
    downgrade["cred"]["backup_eligible"] = json!(false);
    downgrade["cred"]["backup_state"] = json!(false);
    f.deny_json(downgrade).await;
    for field in ["backup_state", "backup_eligible"] {
        for invalid in [json!(null), json!("true"), json!(1), json!({}), json!([])] {
            let mut v = value.clone();
            v["cred"][field] = invalid;
            f.deny_json(v).await;
        }
        let mut v = value.clone();
        v["cred"].as_object_mut().unwrap().remove(field);
        f.deny_json(v).await;
    }
}

async fn preserve_upgrade(owner: PgPool, baseline: bool) {
    let baseline_migrator = sqlx::migrate!("../db/baseline");
    if baseline {
        baseline_migrator.run(&owner).await.unwrap();
        sqlx::query("INSERT INTO organizations(id,slug,name) VALUES($1,'credential-upgrade','Credential upgrade test')")
            .bind(*OrgId::knl().as_uuid()).execute(&owner).await.unwrap();
    } else {
        MIGRATOR.run_to(225, &owner).await.unwrap();
    }
    let mut valid = Fixture::new(owner.clone()).await;
    let mut unknown = Fixture::new(owner.clone()).await;
    let mut value = unknown.state().await["passkey_json"].clone();
    // Serde ordinarily ignores this extra field; storage admission must not.
    value["cred"]["future_codec"] = json!(6);
    unknown.replace(value).await.unwrap();
    let old_valid = valid.state().await;
    let old_unknown = unknown.state().await;
    let migrator = sqlx::migrate::Migrator {
        migrations: Cow::Owned(if baseline {
            baseline_migrator
                .iter()
                .chain(MIGRATOR.iter().filter(|m| m.version > 225))
                .cloned()
                .collect()
        } else {
            MIGRATOR.iter().cloned().collect()
        }),
        ..sqlx::migrate::Migrator::DEFAULT
    };
    let suffix_owner = if baseline {
        // The dump restores console_app ownership; the suffix must use that
        // same owner. Only hand off this disposable database and SQLx ledger.
        sqlx::raw_sql(
            r#"
            DO $handoff$
            BEGIN
                IF CURRENT_USER <> 'console_buck_admin'
                   OR SESSION_USER <> CURRENT_USER
                   OR current_setting('console.sqlx_test_bootstrap', true)
                      IS DISTINCT FROM 'buck-sqlx-superuser-v1'
                   OR CURRENT_DATABASE() !~ '^_sqlx_test_[A-Za-z0-9_]{52}$'
                   OR (SELECT pg_get_userbyid(datdba) FROM pg_database
                       WHERE datname = CURRENT_DATABASE()) <> CURRENT_USER
                THEN
                    RAISE EXCEPTION 'credential_upgrade.disposable_owner_required'
                        USING ERRCODE = '42501';
                END IF;
                EXECUTE format('ALTER DATABASE %I OWNER TO console_app', CURRENT_DATABASE());
                ALTER TABLE public._sqlx_migrations OWNER TO console_app;
            END
            $handoff$;
            "#,
        )
        .execute(&owner)
        .await
        .unwrap();
        // This isolates current-role state, not password separation.
        let suffix = PgPoolOptions::new()
            .max_connections(1)
            .after_connect(|conn, _| {
                Box::pin(async move {
                    sqlx::query("SET ROLE console_app").execute(conn).await?;
                    Ok(())
                })
            })
            .connect_with(owner.connect_options().as_ref().clone())
            .await
            .unwrap();
        let identity: (String, String, bool, bool) = sqlx::query_as(
            "SELECT current_user::text,session_user::text,rolsuper,rolbypassrls FROM pg_roles WHERE rolname=current_user",
        ).fetch_one(&suffix).await.unwrap();
        assert_eq!(
            identity,
            (
                "console_app".into(),
                "console_buck_admin".into(),
                false,
                true
            )
        );
        suffix
    } else {
        owner.clone()
    };
    migrator.run(&suffix_owner).await.unwrap();
    let version: i64 = sqlx::query_scalar("SELECT max(version) FROM _sqlx_migrations")
        .fetch_one(&owner)
        .await
        .unwrap();
    assert!(version >= 226, "the actual suffix must have been applied");
    assert_eq!(valid.state().await, old_valid);
    assert_eq!(unknown.state().await, old_unknown);
    let result = sqlx::query(
        "UPDATE auth_webauthn_credentials SET last_used_at=clock_timestamp() WHERE id=$1",
    )
    .bind(unknown.key.id)
    .execute(&unknown.runtime)
    .await;
    assert!(
        result.is_err(),
        "unknown codec denies even timestamp-only changes"
    );
    assert!(unknown.authenticate(false).await.is_err());
    assert_eq!(unknown.state().await, old_unknown);
    valid.authenticate(false).await.unwrap();
}

#[sqlx::test(migrations = false)]
async fn historical_upgrade_preserves_keys_and_denies_unknown_codec(owner: PgPool) {
    preserve_upgrade(owner, false).await;
}

#[sqlx::test(migrations = false)]
async fn baseline_upgrade_preserves_keys_and_denies_unknown_codec(owner: PgPool) {
    preserve_upgrade(owner, true).await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn malformed_existing_codec_cannot_be_updated_or_silently_repaired(owner: PgPool) {
    let f = Fixture::new(owner).await;
    let valid = f.state().await["passkey_json"].clone();
    let mut variants = vec![json!({}), json!([]), json!(null), json!({"cred":null})];
    for (field, bad) in [
        ("counter", json!("0")),
        ("counter", json!(-1)),
        ("counter", json!(1.5)),
        ("counter", json!(4294967296_u64)),
        ("backup_state", json!(null)),
        ("backup_eligible", json!(1)),
        ("cred", json!(null)),
        ("user_verified", json!("true")),
    ] {
        let mut v = valid.clone();
        v["cred"][field] = bad;
        variants.push(v);
    }
    let mut missing = valid.clone();
    missing["cred"]
        .as_object_mut()
        .unwrap()
        .remove("attestation");
    variants.push(missing);
    for malformed in variants {
        let id = Uuid::new_v4();
        sqlx::query("INSERT INTO auth_webauthn_credentials(id,user_id,credential_id,passkey_json,org_id) VALUES($1,$2,$3,$4,$5)")
            .bind(id).bind(f.user).bind(id.to_string()).bind(&malformed).bind(*OrgId::knl().as_uuid())
            .execute(&f.owner).await.unwrap();
        // Legacy malformed fixtures; current raw INSERT custody remains an open finding.
        for replacement in [&malformed, &valid] {
            let result=sqlx::query("UPDATE auth_webauthn_credentials SET passkey_json=$1,last_used_at=clock_timestamp() WHERE id=$2")
                .bind(replacement).bind(id).execute(&f.runtime).await;
            assert!(
                result.is_err(),
                "unknown OLD shape must not be silently normalized"
            );
            let current: Value = sqlx::query_scalar(
                "SELECT passkey_json FROM auth_webauthn_credentials WHERE id=$1",
            )
            .bind(id)
            .fetch_one(&f.owner)
            .await
            .unwrap();
            assert_eq!(current, malformed);
        }
    }
}

#[sqlx::test(migrations = "../db/migrations")]
async fn multirow_denial_and_caller_rollback_preserve_all_keys(owner: PgPool) {
    let a = Fixture::new(owner.clone()).await;
    let b = Fixture::new(owner).await;
    let before_a = a.state().await;
    let before_b = b.state().await;
    let result=sqlx::query("UPDATE auth_webauthn_credentials SET last_used_at=clock_timestamp(),passkey_json=CASE WHEN id=$1 THEN jsonb_set(passkey_json,'{cred,counter}','-1') ELSE passkey_json END WHERE id=ANY($2)")
        .bind(b.key.id).bind(vec![a.key.id,b.key.id]).execute(&a.runtime).await;
    assert!(result.is_err());
    assert_eq!(a.state().await, before_a);
    assert_eq!(b.state().await, before_b);
    let mut tx = a.runtime.begin().await.unwrap();
    sqlx::query("UPDATE auth_webauthn_credentials SET last_used_at=clock_timestamp() WHERE id=$1")
        .bind(a.key.id)
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(a.state().await, before_a);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn guard_needs_no_account_lock_and_runtime_cannot_disable_it(owner: PgPool) {
    let f = Fixture::new(owner).await;
    let mut held = f.owner.begin().await.unwrap();
    sqlx::query("SELECT id FROM users WHERE id=$1 FOR UPDATE")
        .bind(f.user)
        .fetch_one(&mut *held)
        .await
        .unwrap();
    let mut tx = f.runtime.begin().await.unwrap();
    sqlx::query("SET LOCAL lock_timeout='500ms'")
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("UPDATE auth_webauthn_credentials SET last_used_at=clock_timestamp() WHERE id=$1")
        .bind(f.key.id)
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    held.rollback().await.unwrap();
    for sql in [
        "ALTER TABLE auth_webauthn_credentials DISABLE TRIGGER ALL",
        "SET session_replication_role=replica",
    ] {
        assert!(sqlx::query(sql).execute(&f.runtime).await.is_err());
    }
    let before = f.state().await;
    let mut invalid = before["passkey_json"].clone();
    invalid["cred"]["user_verified"] = json!(false);
    f.deny_json(invalid).await;
}
