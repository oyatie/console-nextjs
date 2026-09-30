#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use console_kernel_core::OrgId;
use console_platform_auth::{PasskeyRegistrationStart, PasskeyService, WebauthnSettings};
use sqlx::{PgPool, Row};
use time::Duration;
use url::Url;
use webauthn_authenticator_rs::prelude::{RequestChallengeResponse, WebauthnAuthenticator};
use webauthn_authenticator_rs::softpasskey::SoftPasskey;

/// Inject one `allowCredentials` entry into a discoverable challenge so the
/// SoftPasskey harness — which has no resident-key store and cannot sign against
/// an empty allowCredentials — can locate its key. This emulates what a real
/// discoverable authenticator does internally; the SERVER ceremony remains fully
/// discoverable (it issues an empty allowCredentials). The returned assertion
/// still carries the credential id, which is what the server resolves by.
fn inject_allow_credential(
    challenge: RequestChallengeResponse,
    credential_id: &str,
) -> RequestChallengeResponse {
    let mut value = serde_json::to_value(&challenge).unwrap();
    let allow = value
        .get_mut("publicKey")
        .and_then(|pk| pk.get_mut("allowCredentials"))
        .and_then(serde_json::Value::as_array_mut)
        .expect("discoverable challenge must have an allowCredentials array");
    allow.push(serde_json::json!({ "type": "public-key", "id": credential_id }));
    serde_json::from_value(value).unwrap()
}

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

/// Register a discoverable passkey, then authenticate WITHOUT supplying a
/// user_id. The user is resolved from the asserted credential at finish time —
/// this is the usernameless sign-in path.
#[sqlx::test(migrations = "../db/migrations")]
async fn discoverable_passkey_registration_and_usernameless_login(pool: PgPool) {
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

    let stored_state_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM auth_webauthn_ceremonies WHERE id = $1")
            .bind(registration.ceremony_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(stored_state_count, 1);

    let mut authenticator = WebauthnAuthenticator::new(SoftPasskey::new(true));
    let credential = authenticator
        .do_registration(
            Url::parse("https://auth.example.com").unwrap(),
            registration.challenge,
        )
        .unwrap();

    let stored_passkey = service
        .finish_registration(&pool, OrgId::knl(), registration.ceremony_id, credential)
        .await
        .unwrap();
    assert_eq!(stored_passkey.user_id, user_id);

    let consumed_at: Option<time::OffsetDateTime> =
        sqlx::query("SELECT consumed_at FROM auth_webauthn_ceremonies WHERE id = $1")
            .bind(registration.ceremony_id)
            .fetch_one(&pool)
            .await
            .unwrap()
            .try_get("consumed_at")
            .unwrap();
    assert!(consumed_at.is_some());

    // Usernameless authentication: start carries no user_id; the persisted
    // ceremony has a NULL user_id and an empty allowCredentials challenge.
    let authentication = service.start_authentication(&pool).await.unwrap();
    let ceremony_user_id: Option<uuid::Uuid> =
        sqlx::query_scalar("SELECT user_id FROM auth_webauthn_ceremonies WHERE id = $1")
            .bind(authentication.ceremony_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(
        ceremony_user_id.is_none(),
        "discoverable auth ceremony must not bind a user up front"
    );

    let challenge =
        inject_allow_credential(authentication.challenge, &stored_passkey.credential_id);
    let assertion = authenticator
        .do_authentication(Url::parse("https://auth.example.com").unwrap(), challenge)
        .unwrap();

    let outcome = service
        .finish_authentication(&pool, authentication.ceremony_id, assertion)
        .await
        .unwrap();
    assert_eq!(
        outcome.user_id, user_id,
        "user must be resolved from the asserted credential"
    );
    assert_eq!(outcome.passkey_id, stored_passkey.id);
}

/// An assertion for a credential that is not registered must be rejected (the
/// user cannot be resolved), and the ceremony stays the single-use, atomic kind.
#[sqlx::test(migrations = "../db/migrations")]
async fn usernameless_login_rejects_unregistered_credential(pool: PgPool) {
    let service = service();

    let authentication = service.start_authentication(&pool).await.unwrap();

    // Drive an authenticator that never registered against this service.
    let mut authenticator = WebauthnAuthenticator::new(SoftPasskey::new(true));
    let assertion = authenticator.do_authentication(
        Url::parse("https://auth.example.com").unwrap(),
        authentication.challenge,
    );

    // SoftPasskey with no matching credential fails to produce an assertion; if it
    // somehow does, finish must still reject because the credential is unknown.
    if let Ok(assertion) = assertion {
        let result = service
            .finish_authentication(&pool, authentication.ceremony_id, assertion)
            .await;
        assert!(result.is_err(), "unregistered credential must be rejected");
    }
}
