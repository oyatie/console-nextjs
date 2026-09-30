#![allow(clippy::unwrap_used, clippy::expect_used)]

use sqlx::{Acquire, PgPool, Row};
use uuid::Uuid;

#[sqlx::test(migrations = "./migrations")]
async fn new_account_state_is_private_and_survives_retirement(pool: PgPool) {
    let company = Uuid::new_v4();
    let account = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO organizations (id, slug, name) VALUES ($1, $2, 'Security state test')",
    )
    .bind(company)
    .bind(format!("as-{}", company.simple()))
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO users (id, display_name, org_id, roles) VALUES ($1, 'New account', $2, ARRAY['MEMBER'])")
        .bind(account)
        .bind(company)
        .execute(&pool)
        .await
        .unwrap();

    let state = sqlx::query("SELECT home_org_id, auth_generation, revision, ever_enrolled, status, retired_at FROM auth_security.account_state WHERE account_id=$1")
        .bind(account)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(state.get::<Uuid, _>("home_org_id"), company);
    assert_eq!(state.get::<i64, _>("auth_generation"), 1);
    assert_eq!(state.get::<i64, _>("revision"), 1);
    assert!(!state.get::<bool, _>("ever_enrolled"));
    assert_eq!(state.get::<String, _>("status"), "never_enrolled");
    assert!(
        state
            .get::<Option<time::OffsetDateTime>, _>("retired_at")
            .is_none()
    );

    for sql in [
        "UPDATE auth_security.account_state SET home_org_id=gen_random_uuid(), revision=2 WHERE account_id=$1",
        "UPDATE auth_security.account_state SET auth_generation=0, revision=2 WHERE account_id=$1",
        "UPDATE auth_security.account_state SET ever_enrolled=NULL, revision=2 WHERE account_id=$1",
    ] {
        assert!(
            sqlx::query(sql).bind(account).execute(&pool).await.is_err(),
            "protected identity, generation and enrollment history cannot regress"
        );
    }

    let mut runtime = pool.begin().await.unwrap();
    sqlx::query("SET LOCAL ROLE console_rt")
        .execute(&mut *runtime)
        .await
        .unwrap();
    let denied = sqlx::query("SELECT account_id FROM auth_security.account_state")
        .fetch_all(&mut *runtime)
        .await
        .unwrap_err();
    assert_eq!(
        denied.as_database_error().and_then(|e| e.code()).as_deref(),
        Some("42501")
    );
    drop(runtime);

    sqlx::query("DELETE FROM users WHERE id=$1")
        .bind(account)
        .execute(&pool)
        .await
        .unwrap();
    let state = sqlx::query(
        "SELECT status, retired_at FROM auth_security.account_state WHERE account_id=$1",
    )
    .bind(account)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(state.get::<String, _>("status"), "retired");
    assert!(
        state
            .get::<Option<time::OffsetDateTime>, _>("retired_at")
            .is_some()
    );
    assert!(
        sqlx::query("UPDATE auth_security.account_state SET status='enrolled', ever_enrolled=true, retired_at=NULL, revision=3 WHERE account_id=$1")
            .bind(account)
            .execute(&pool)
            .await
            .is_err(),
        "retirement cannot be reversed"
    );
    let reused = sqlx::query("INSERT INTO users (id, display_name, org_id, roles) VALUES ($1, 'Reused', $2, ARRAY['MEMBER'])")
        .bind(account)
        .bind(company)
        .execute(&pool)
        .await;
    assert!(reused.is_err(), "retired Account UUID must not be reusable");
}

#[sqlx::test(migrations = "./migrations")]
async fn new_provenance_versions_require_exact_source_fields(pool: PgPool) {
    // Schema shape and operation uniqueness only. The auth owner must still
    // prove the source and bind it atomically before a family is trusted.
    let company = Uuid::new_v4();
    let account = Uuid::new_v4();
    sqlx::query("INSERT INTO organizations (id, slug, name) VALUES ($1, $2, 'Provenance test')")
        .bind(company)
        .bind(format!("ap-{}", company.simple()))
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO users (id, display_name, org_id, roles) VALUES ($1, 'Account', $2, ARRAY['MEMBER'])")
        .bind(account)
        .bind(company)
        .execute(&pool)
        .await
        .unwrap();
    let now = time::OffsetDateTime::now_utc();
    let source = Uuid::new_v4();
    let bad = sqlx::query("INSERT INTO auth_bootstrap_credentials (user_id, token_hash, issued_at, expires_at, org_id, issuance_version) VALUES ($1,$2,$3,$4,$5,1)")
        .bind(account).bind(vec![1_u8; 32]).bind(now).bind(now + time::Duration::hours(1)).bind(company)
        .execute(&pool).await;
    assert!(bad.is_err(), "versioned OTP without source must fail");
    sqlx::query("INSERT INTO auth_bootstrap_credentials (user_id, token_hash, issued_at, expires_at, org_id, issuance_version, issued_generation, issuance_purpose, source_operation_id) VALUES ($1,$2,$3,$4,$5,1,1,'first_enrollment',$6)")
        .bind(account).bind(vec![2_u8; 32]).bind(now).bind(now + time::Duration::hours(1)).bind(company).bind(source)
        .execute(&pool).await.unwrap();
    let duplicate = sqlx::query("INSERT INTO auth_bootstrap_credentials (user_id, token_hash, issued_at, expires_at, org_id, issuance_version, issued_generation, issuance_purpose, source_operation_id) VALUES ($1,$2,$3,$4,$5,1,1,'first_enrollment',$6)")
        .bind(account).bind(vec![3_u8; 32]).bind(now).bind(now + time::Duration::hours(1)).bind(company).bind(source)
        .execute(&pool).await;
    assert!(
        duplicate.is_err(),
        "one source operation must not issue two OTPs"
    );

    let family_source = Uuid::new_v4();
    let bad_family = sqlx::query("INSERT INTO auth_refresh_token_families (user_id, created_at, org_id, provenance_version) VALUES ($1,$2,$3,1)")
        .bind(account).bind(now).bind(company).execute(&pool).await;
    assert!(
        bad_family.is_err(),
        "versioned family without proof must fail"
    );
    // The valid shape is inspected before COMMIT; the new deferred custody
    // guard correctly rejects a standalone enrollment family at COMMIT.
    let mut shape_tx = pool.begin().await.unwrap();
    sqlx::query("INSERT INTO auth_refresh_token_families (user_id, created_at, org_id, provenance_version, auth_generation, session_purpose, source_kind, source_operation_id) VALUES ($1,$2,$3,1,1,'enrollment','bootstrap_otp',$4)")
        .bind(account).bind(now).bind(company).bind(family_source).execute(shape_tx.as_mut()).await.unwrap();
    let mut duplicate_tx = shape_tx.begin().await.unwrap();
    let duplicate_family = sqlx::query("INSERT INTO auth_refresh_token_families (user_id, created_at, org_id, provenance_version, auth_generation, session_purpose, source_kind, source_operation_id) VALUES ($1,$2,$3,1,1,'enrollment','bootstrap_otp',$4)")
        .bind(account).bind(now).bind(company).bind(family_source).execute(duplicate_tx.as_mut()).await;
    assert!(
        duplicate_family.is_err(),
        "one proof operation must not mint two families"
    );
    duplicate_tx.rollback().await.unwrap();
    shape_tx.rollback().await.unwrap();
}

#[sqlx::test(migrations = "./migrations")]
async fn private_admission_and_registration_intent_keep_exact_terminal_custody(pool: PgPool) {
    let account = Uuid::new_v4();
    let company = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO organizations(id,slug,name) VALUES($1,'auth-state-custody','Custody')",
    )
    .bind(company)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO users(id,display_name,org_id,roles) VALUES($1,'Custody Account',$2,ARRAY['MEMBER'])")
        .bind(account)
        .bind(company)
        .execute(&pool)
        .await
        .unwrap();
    let operation = Uuid::new_v4();
    let family = Uuid::new_v4();
    let source = Uuid::new_v4();
    let issuance = Uuid::new_v4();
    let receipt = Uuid::new_v4();
    let ceremony = Uuid::new_v4();
    let expires = time::OffsetDateTime::now_utc() + time::Duration::minutes(5);
    let terminal_insert = sqlx::query(
        "INSERT INTO auth_security.session_admissions \
         (operation_id,status_secret_hash,account_id,home_org_id,auth_generation, \
          session_purpose,status,source_kind,source_row_id,source_operation_id, \
          family_id,receipt_id,expires_at,finalized_at) \
         VALUES ($1,$2,$3,$4,1,'enrollment','committed','bootstrap_otp', \
                 $5,$6,$7,$8,$9,clock_timestamp())",
    )
    .bind(Uuid::new_v4())
    .bind(vec![6_u8; 32])
    .bind(account)
    .bind(company)
    .bind(source)
    .bind(issuance)
    .bind(Uuid::new_v4())
    .bind(Uuid::new_v4())
    .bind(expires)
    .execute(&pool)
    .await
    .unwrap_err();
    assert_eq!(
        terminal_insert
            .as_database_error()
            .and_then(|error| error.code())
            .as_deref(),
        Some("23514"),
        "a serving writer cannot insert a completed admission"
    );
    sqlx::query(
        "INSERT INTO auth_security.session_admissions \
         (operation_id,status_secret_hash,account_id,home_org_id,auth_generation,session_purpose,expires_at) \
         VALUES ($1,$2,$3,$4,1,'enrollment',$5)",
    )
    .bind(operation)
    .bind(vec![7_u8; 32])
    .bind(account)
    .bind(company)
    .bind(expires)
    .execute(&pool)
    .await
    .unwrap();
    let invalid = sqlx::query(
        "UPDATE auth_security.session_admissions SET status='committed', \
         source_kind='passkey', source_row_id=$2, source_operation_id=$3, \
         family_id=$4, receipt_id=$5, finalized_at=clock_timestamp() WHERE operation_id=$1",
    )
    .bind(operation)
    .bind(source)
    .bind(issuance)
    .bind(family)
    .bind(receipt)
    .execute(&pool)
    .await;
    assert!(invalid.is_err(), "enrollment cannot use a passkey source");
    let created_at: time::OffsetDateTime = sqlx::query_scalar(
        "SELECT created_at FROM auth_security.session_admissions WHERE operation_id=$1",
    )
    .bind(operation)
    .fetch_one(&pool)
    .await
    .unwrap();
    for terminal_time in [expires, created_at - time::Duration::seconds(1)] {
        assert!(
            sqlx::query(
                "UPDATE auth_security.session_admissions SET status='committed', \
                 source_kind='bootstrap_otp', source_row_id=$2, source_operation_id=$3, \
                 family_id=$4, receipt_id=$5, finalized_at=$6 WHERE operation_id=$1"
            )
            .bind(operation)
            .bind(source)
            .bind(issuance)
            .bind(family)
            .bind(receipt)
            .bind(terminal_time)
            .execute(&pool)
            .await
            .is_err(),
            "admission must finish within its open time interval"
        );
    }
    // Inspect one-way terminal shape inside a transaction. The admission has
    // synthetic source IDs, so the owner closure guard must reject its COMMIT.
    let mut shape_tx = pool.begin().await.unwrap();
    sqlx::query(
        "UPDATE auth_security.session_admissions SET status='committed', \
         source_kind='bootstrap_otp', source_row_id=$2, source_operation_id=$3, \
         family_id=$4, receipt_id=$5, finalized_at=clock_timestamp() WHERE operation_id=$1",
    )
    .bind(operation)
    .bind(source)
    .bind(issuance)
    .bind(family)
    .bind(receipt)
    .execute(shape_tx.as_mut())
    .await
    .unwrap();
    let mut reopen = shape_tx.begin().await.unwrap();
    assert!(
        sqlx::query(
            "UPDATE auth_security.session_admissions SET status='rejected' WHERE operation_id=$1"
        )
        .bind(operation)
        .execute(reopen.as_mut())
        .await
        .is_err(),
        "a committed proof cannot be reopened or rewritten"
    );
    reopen.rollback().await.unwrap();
    shape_tx.rollback().await.unwrap();

    let consumed_insert = sqlx::query(
        "INSERT INTO auth_security.registration_intents \
         (ceremony_id,account_id,home_org_id,auth_generation,family_id,purpose, \
          source_bootstrap_id,source_issuance_operation_id,expires_at,consumed_at) \
         VALUES ($1,$2,$3,1,$4,'first_enrollment',$5,$6,$7,clock_timestamp())",
    )
    .bind(Uuid::new_v4())
    .bind(account)
    .bind(company)
    .bind(family)
    .bind(source)
    .bind(issuance)
    .bind(expires)
    .execute(&pool)
    .await
    .unwrap_err();
    assert_eq!(
        consumed_insert
            .as_database_error()
            .and_then(|error| error.code())
            .as_deref(),
        Some("23514"),
        "a serving writer cannot insert a pre-consumed intent"
    );
    sqlx::query(
        "INSERT INTO auth_security.registration_intents \
         (ceremony_id,account_id,home_org_id,auth_generation,family_id,purpose, \
          source_bootstrap_id,source_issuance_operation_id,expires_at) \
         VALUES ($1,$2,$3,1,$4,'first_enrollment',$5,$6,$7)",
    )
    .bind(ceremony)
    .bind(account)
    .bind(company)
    .bind(family)
    .bind(source)
    .bind(issuance)
    .bind(expires)
    .execute(&pool)
    .await
    .unwrap();
    let created_at: time::OffsetDateTime = sqlx::query_scalar(
        "SELECT created_at FROM auth_security.registration_intents WHERE ceremony_id=$1",
    )
    .bind(ceremony)
    .fetch_one(&pool)
    .await
    .unwrap();
    for terminal_time in [expires, created_at - time::Duration::seconds(1)] {
        assert!(
            sqlx::query(
                "UPDATE auth_security.registration_intents SET consumed_at=$2 WHERE ceremony_id=$1"
            )
            .bind(ceremony)
            .bind(terminal_time)
            .execute(&pool)
            .await
            .is_err(),
            "registration intent must finish within its open time interval"
        );
    }
    assert!(
        sqlx::query("UPDATE auth_security.registration_intents SET source_bootstrap_id=gen_random_uuid() WHERE ceremony_id=$1")
            .bind(ceremony)
            .execute(&pool)
            .await
            .is_err(),
        "registration cannot switch the OTP source"
    );
    sqlx::query("UPDATE auth_security.registration_intents SET consumed_at=clock_timestamp() WHERE ceremony_id=$1")
        .bind(ceremony)
        .execute(&pool)
        .await
        .unwrap();
    assert!(
        sqlx::query(
            "UPDATE auth_security.registration_intents SET consumed_at=NULL WHERE ceremony_id=$1"
        )
        .bind(ceremony)
        .execute(&pool)
        .await
        .is_err(),
        "completed registration cannot be reopened"
    );

    // Restore uses the offline owner. Constraints must still reject impossible
    // terminal times even when the serving-role INSERT guard is bypassed.
    // SQLx test databases are owned by the bootstrap role rather than console_app.
    sqlx::query("GRANT USAGE ON SCHEMA auth_security TO console_app")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        "GRANT INSERT ON auth_security.session_admissions, auth_security.registration_intents TO console_app",
    )
    .execute(&pool)
    .await
    .unwrap();
    let created = time::OffsetDateTime::now_utc();
    let deadline = created + time::Duration::minutes(5);
    for (index, terminal, valid) in [
        (0_u8, created - time::Duration::seconds(1), false),
        (1, deadline, false),
        (2, created, true),
    ] {
        let mut restore = pool.begin().await.unwrap();
        sqlx::query("SET LOCAL ROLE console_app")
            .execute(&mut *restore)
            .await
            .unwrap();
        let result = sqlx::query(
            "INSERT INTO auth_security.session_admissions \
             (operation_id,status_secret_hash,account_id,home_org_id,auth_generation, \
              session_purpose,status,source_kind,source_row_id,source_operation_id, \
              family_id,receipt_id,created_at,expires_at,finalized_at) \
             VALUES ($1,$2,$3,$4,1,'enrollment','committed','bootstrap_otp', \
                     $5,$6,$7,$8,$9,$10,$11)",
        )
        .bind(Uuid::new_v4())
        .bind(vec![10_u8 + index; 32])
        .bind(account)
        .bind(company)
        .bind(source)
        .bind(Uuid::new_v4())
        .bind(Uuid::new_v4())
        .bind(Uuid::new_v4())
        .bind(created)
        .bind(deadline)
        .bind(terminal)
        .execute(&mut *restore)
        .await;
        if valid {
            assert!(result.is_ok(), "valid admission restore: {result:?}");
        } else {
            assert_eq!(
                result
                    .as_ref()
                    .err()
                    .and_then(|error| error.as_database_error())
                    .and_then(|error| error.code())
                    .as_deref(),
                Some("23514"),
                "invalid admission time must fail its CHECK"
            );
        }
        if valid {
            restore.commit().await.unwrap();
        }
    }
    for (terminal, valid) in [
        (created - time::Duration::seconds(1), false),
        (deadline, false),
        (created, true),
    ] {
        let mut restore = pool.begin().await.unwrap();
        sqlx::query("SET LOCAL ROLE console_app")
            .execute(&mut *restore)
            .await
            .unwrap();
        let result = sqlx::query(
            "INSERT INTO auth_security.registration_intents \
             (ceremony_id,account_id,home_org_id,auth_generation,family_id,purpose, \
              source_bootstrap_id,source_issuance_operation_id,created_at,expires_at,consumed_at) \
             VALUES ($1,$2,$3,1,$4,'first_enrollment',$5,$6,$7,$8,$9)",
        )
        .bind(Uuid::new_v4())
        .bind(account)
        .bind(company)
        .bind(Uuid::new_v4())
        .bind(source)
        .bind(Uuid::new_v4())
        .bind(created)
        .bind(deadline)
        .bind(terminal)
        .execute(&mut *restore)
        .await;
        if valid {
            assert!(result.is_ok(), "valid intent restore: {result:?}");
        } else {
            assert_eq!(
                result
                    .as_ref()
                    .err()
                    .and_then(|error| error.as_database_error())
                    .and_then(|error| error.code())
                    .as_deref(),
                Some("23514"),
                "invalid intent time must fail its CHECK"
            );
        }
        if valid {
            restore.commit().await.unwrap();
        }
    }

    let mut runtime = pool.begin().await.unwrap();
    sqlx::query("SET LOCAL ROLE console_rt")
        .execute(&mut *runtime)
        .await
        .unwrap();
    assert!(
        sqlx::query("SELECT operation_id FROM auth_security.session_admissions")
            .fetch_all(&mut *runtime)
            .await
            .is_err(),
        "ordinary runtime cannot inspect private admissions"
    );
}
