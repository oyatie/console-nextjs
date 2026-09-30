#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! A populated mailbox must survive the concurrent-key/index expand step.
//! Retrying the migrator must not rebuild the index or replace its account.

use sqlx::PgPool;
use uuid::Uuid;

#[sqlx::test(migrations = false)]
async fn populated_mailbox_upgrade_preserves_rows_and_is_idempotent(pool: PgPool) {
    let migrator = sqlx::migrate!("./migrations");
    migrator.run_to(234, &pool).await.unwrap();

    let org = Uuid::new_v4();
    let actor = Uuid::new_v4();
    let account = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO organizations (id, slug, name) VALUES ($1, 'mail-upgrade', 'Upgrade company')",
    )
    .bind(org)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO users (id, org_id, display_name, roles, is_active) VALUES ($1, $2, 'Mailer', ARRAY['ADMIN'], true)",
    )
    .bind(actor)
    .bind(org)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        r#"INSERT INTO email_accounts (
            id, org_id, display_name, email_address,
            imap_host, imap_port, imap_security, imap_username,
            smtp_host, smtp_port, smtp_security, smtp_username,
            smtp_password_ct, smtp_password_nonce, imap_password_ct,
            imap_password_nonce, dek_wrapped, dek_nonce,
            imap_dek_wrapped, imap_dek_nonce, created_by
        ) VALUES (
            $1, $2, 'Existing mailbox', 'existing@example.com',
            'imap.example.com', 993, 'TLS', 'existing',
            'smtp.example.com', 587, 'STARTTLS', 'existing',
            '\x01'::bytea, '\x02'::bytea, '\x03'::bytea,
            '\x04'::bytea, '\x05'::bytea, '\x06'::bytea,
            '\x07'::bytea, '\x08'::bytea, $3
        )"#,
    )
    .bind(account)
    .bind(org)
    .bind(actor)
    .execute(&pool)
    .await
    .unwrap();

    migrator.run_to(238, &pool).await.unwrap();
    let unaudited_operation = Uuid::new_v4();
    sqlx::query(
        r#"INSERT INTO email_outbound_operations (
            org_id, operation_id, account_id, actor_user_id,
            request_codec_version, request_hash, payload_hash,
            payload_format, payload_bytes, envelope_from,
            envelope_recipients, rfc_message_id
        ) VALUES ($1, $2, $3, $4, 1, $5, $6,
                  'SMTP_MIME', $7, 'existing@example.com',
                  ARRAY['recipient@example.com'], '<unaudited@example.com>')"#,
    )
    .bind(org)
    .bind(unaudited_operation)
    .bind(account)
    .bind(actor)
    .bind(vec![1_u8; 32])
    .bind(vec![2_u8; 32])
    .bind(b"body".to_vec())
    .execute(&pool)
    .await
    .unwrap();
    let error = migrator
        .run(&pool)
        .await
        .expect_err("upgrade must stop on an unaudited prepared row");
    assert!(
        error
            .to_string()
            .contains("existing prepared mail lacks exactly one matching audit"),
        "unexpected migration error: {error}"
    );
    sqlx::query("DELETE FROM email_outbound_operations WHERE operation_id = $1")
        .bind(unaudited_operation)
        .execute(&pool)
        .await
        .unwrap();

    migrator.run(&pool).await.unwrap();
    let stored: (Uuid, Uuid, String) =
        sqlx::query_as("SELECT org_id, id, email_address FROM email_accounts WHERE id = $1")
            .bind(account)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(stored, (org, account, "existing@example.com".to_owned()));
    let index_ready: (bool, bool) = sqlx::query_as(
        "SELECT i.indisvalid, i.indisunique FROM pg_index i WHERE i.indexrelid = 'email_accounts_org_id_id_unique'::regclass",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(index_ready, (true, true));

    // A routine application restart runs the same checked migration set again.
    migrator.run(&pool).await.unwrap();
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM email_accounts WHERE id = $1")
        .bind(account)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 1);
}
