//! Complement to `dev_auth_persona_guard.rs`: under `--features dev-auth` the
//! guard is a no-op (a dev-auth build is EXPECTED to carry dev-auth:* rows).
#![cfg(feature = "dev-auth")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use console_app::assert_no_dev_auth_personas;
use console_kernel_core::OrgId;
use sqlx::PgPool;

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn is_a_no_op_under_the_dev_auth_feature(pool: PgPool) {
    sqlx::query("INSERT INTO users (display_name, phone, roles, org_id) VALUES ($1, $2, $3, $4)")
        .bind("dev:ADMIN")
        .bind("dev-auth:00000000-0000-0000-0000-0000000000a1:ADMIN")
        .bind(vec!["ADMIN".to_owned()])
        .bind(*OrgId::knl().as_uuid())
        .execute(&pool)
        .await
        .unwrap();

    assert_no_dev_auth_personas(&pool)
        .await
        .expect("dev-auth builds must never refuse startup over their own persona rows");
}
