//! Proves the composition-root dev-auth persona guard fires in a default
//! (non-dev-auth) build: a leaked `dev-auth:*` row in `users` refuses to let
//! the app start. Complement: `dev_auth_persona_guard_feature.rs` proves the
//! guard is a no-op under `--features dev-auth`.
#![cfg(not(feature = "dev-auth"))]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use console_app::assert_no_dev_auth_personas;
use console_kernel_core::OrgId;
use sqlx::PgPool;

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn refuses_to_start_when_a_dev_auth_persona_row_exists(pool: PgPool) {
    sqlx::query("INSERT INTO users (display_name, phone, roles, org_id) VALUES ($1, $2, $3, $4)")
        .bind("dev:ADMIN")
        .bind("dev-auth:00000000-0000-0000-0000-0000000000a1:ADMIN")
        .bind(vec!["ADMIN".to_owned()])
        .bind(*OrgId::knl().as_uuid())
        .execute(&pool)
        .await
        .unwrap();

    let err = assert_no_dev_auth_personas(&pool)
        .await
        .expect_err("a dev-auth:* row must refuse startup in a default build");
    assert!(
        err.to_string().contains("dev-auth"),
        "error should name the reason: {err}"
    );
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn starts_normally_with_no_dev_auth_persona_rows(pool: PgPool) {
    sqlx::query("INSERT INTO users (display_name, phone, roles, org_id) VALUES ($1, $2, $3, $4)")
        .bind("Real Employee")
        .bind("010-1234-5678")
        .bind(vec!["ADMIN".to_owned()])
        .bind(*OrgId::knl().as_uuid())
        .execute(&pool)
        .await
        .unwrap();

    assert_no_dev_auth_personas(&pool).await.unwrap();
}
