#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Serialize credential issuance against deactivation (console-cg6 review).
//!
//! `finish_registration_in_tx` must lock the user row and recheck `is_active`, so
//! a passkey can never be minted on an account a concurrent (or prior)
//! `deactivate_user` has already swept. This is the registration half of the race
//! the sweep-on-no-op fix left open: without this lock, issuance commits a usable
//! credential after the cleanup sweep ran, and it becomes usable on reactivation.

use console_kernel_core::{ErrorKind, OrgId};
use console_platform_auth::{
    AuthError, PasskeyRegistrationStart, PasskeyService, WebauthnSettings,
};
use sqlx::PgPool;
use time::Duration;
use url::Url;
use webauthn_authenticator_rs::prelude::WebauthnAuthenticator;
use webauthn_authenticator_rs::softpasskey::SoftPasskey;

async fn seed_user(pool: &PgPool) -> uuid::Uuid {
    sqlx::query_scalar(
        "INSERT INTO users (display_name, roles, org_id) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind("Passkey User")
    .bind(Vec::<String>::from(["MECHANIC".to_owned()]))
    .bind(*OrgId::knl().as_uuid())
    .fetch_one(pool)
    .await
    .unwrap()
}

fn service() -> PasskeyService {
    PasskeyService::new(WebauthnSettings {
        rp_id: "example.com".to_owned(),
        rp_origin: Url::parse("https://auth.example.com").unwrap(),
        rp_name: "Console".to_owned(),
        extra_allowed_origins: vec![],
        ceremony_ttl: Duration::minutes(5),
    })
    .unwrap()
}

#[sqlx::test(migrations = "../db/migrations")]
async fn finish_registration_on_deactivated_user_is_refused(pool: PgPool) {
    let user_id = seed_user(&pool).await;
    let service = service();

    let registration = service
        .start_registration(
            &pool,
            OrgId::knl(),
            PasskeyRegistrationStart {
                user_id,
                username: "passkey.user".to_owned(),
                display_name: "Passkey User".to_owned(),
            },
        )
        .await
        .unwrap();

    let mut authenticator = WebauthnAuthenticator::new(SoftPasskey::new(true));
    let credential = authenticator
        .do_registration(
            Url::parse("https://auth.example.com").unwrap(),
            registration.challenge,
        )
        .unwrap();

    // Deactivate between ceremony start and finish — the exact issuance race.
    sqlx::query("UPDATE users SET is_active = false WHERE id = $1")
        .bind(user_id)
        .execute(&pool)
        .await
        .unwrap();

    let result = service
        .finish_registration(&pool, OrgId::knl(), registration.ceremony_id, credential)
        .await;

    match result {
        Err(AuthError::Kernel(err)) => assert_eq!(
            err.kind,
            ErrorKind::Conflict,
            "issuance on a deactivated account must be refused (409)"
        ),
        other => panic!("expected deactivated-user conflict, got {other:?}"),
    }

    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM auth_webauthn_credentials WHERE user_id = $1")
            .bind(user_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(count, 0, "no credential may land on a deactivated account");
}
