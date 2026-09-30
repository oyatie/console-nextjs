//! Dormant first-enrollment command: exercise the real restricted role, not the
//! SQLx migration owner's BYPASSRLS connection.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use console_platform_provisioning::{
    BootstrapCredentialStore, FirstEnrollmentCommandPool, FirstEnrollmentOutcome, ProvisioningError,
};
use sha2::{Digest, Sha256};
use sqlx::postgres::PgPoolOptions;
use sqlx::{Acquire, PgPool, Row};
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

async fn command_pool(owner: &PgPool) -> PgPool {
    PgPoolOptions::new()
        .max_connections(2)
        .after_connect(|connection, _| {
            Box::pin(async move {
                sqlx::query("SET ROLE console_auth_cmd")
                    .execute(connection)
                    .await?;
                Ok(())
            })
        })
        .connect_with(owner.connect_options().as_ref().clone())
        .await
        .unwrap()
}

async fn runtime_pool(owner: &PgPool) -> PgPool {
    PgPoolOptions::new()
        .max_connections(1)
        .after_connect(|connection, _| {
            Box::pin(async move {
                sqlx::query("SET ROLE console_rt")
                    .execute(connection)
                    .await?;
                Ok(())
            })
        })
        .connect_with(owner.connect_options().as_ref().clone())
        .await
        .unwrap()
}

async fn first_source(owner: &PgPool, company: Uuid) -> (Uuid, Uuid, String, Uuid) {
    let phone = format!("010-{:08}", Uuid::new_v4().as_u128() % 100_000_000);
    let account = sqlx::query_scalar(
        "INSERT INTO users(display_name,phone,roles,org_id) \
         VALUES('First Enrollee',$2,ARRAY['MEMBER'],$1) RETURNING id",
    )
    .bind(company)
    .bind(phone)
    .fetch_one(owner)
    .await
    .unwrap();
    let source = Uuid::new_v4();
    let issuance = Uuid::new_v4();
    let otp = format!("test-first-{}", Uuid::new_v4());
    let now = OffsetDateTime::now_utc();
    sqlx::query(
        "INSERT INTO auth_bootstrap_credentials \
         (id,user_id,token_hash,issued_at,expires_at,org_id,issuance_version,issued_generation,issuance_purpose,source_operation_id) \
         VALUES($1,$2,$3,$4,$5,$6,1,1,'first_enrollment',$7)",
    )
    .bind(source)
    .bind(account)
    .bind(Sha256::digest(otp.as_bytes()).to_vec())
    .bind(now)
    .bind(now + Duration::hours(1))
    .bind(company)
    .bind(issuance)
    .execute(owner)
    .await
    .unwrap();
    (account, source, otp, issuance)
}

#[sqlx::test(migrations = "../db/migrations")]
async fn command_admits_exact_source_family_and_receipt_once(owner: PgPool) {
    let company = Uuid::new_v4();
    sqlx::query("INSERT INTO organizations(id,slug,name) VALUES($1,'command-home','Home')")
        .bind(company)
        .execute(&owner)
        .await
        .unwrap();
    let (account, source, otp, issuance) = first_source(&owner, company).await;
    let command = FirstEnrollmentCommandPool::new(command_pool(&owner).await);
    let operation = Uuid::new_v4();
    let status_hash: [u8; 32] = Sha256::digest(Uuid::new_v4().as_bytes()).into();
    let issued = command
        .admit(&otp, operation, status_hash, OffsetDateTime::now_utc())
        .await
        .unwrap();
    let (receipt, family, token, deadline) = match issued {
        FirstEnrollmentOutcome::LocallyCommitted {
            receipt_id,
            refresh,
            enrollment_expires_at,
        } => (
            receipt_id,
            refresh.family_id,
            refresh.token.as_str().to_owned(),
            enrollment_expires_at,
        ),
        FirstEnrollmentOutcome::AlreadyCommitted { .. } => panic!("first call must issue"),
    };
    assert!(token.starts_with("console_rt_"));
    let row = sqlx::query(
        "SELECT a.account_id,a.home_org_id,a.auth_generation,a.status,a.source_row_id, \
         a.source_operation_id,a.family_id,a.receipt_id,a.expires_at, \
         f.provenance_version,f.session_purpose,f.source_kind,f.source_operation_id AS family_operation, \
         b.consumed_at, t.expires_at AS token_expires_at \
         FROM auth_security.session_admissions a \
         JOIN auth_refresh_token_families f ON f.id=a.family_id \
         JOIN auth_bootstrap_credentials b ON b.id=a.source_row_id \
         JOIN auth_refresh_tokens t ON t.family_id=f.id \
         WHERE a.operation_id=$1",
    )
    .bind(operation)
    .fetch_one(&owner)
    .await
    .unwrap();
    assert_eq!(row.get::<Uuid, _>("account_id"), account);
    assert_eq!(row.get::<Uuid, _>("home_org_id"), company);
    assert_eq!(row.get::<i64, _>("auth_generation"), 1);
    assert_eq!(row.get::<String, _>("status"), "committed");
    assert_eq!(row.get::<Uuid, _>("source_row_id"), source);
    assert_eq!(row.get::<Uuid, _>("source_operation_id"), issuance);
    assert_eq!(row.get::<Uuid, _>("family_id"), family);
    assert_eq!(row.get::<Uuid, _>("receipt_id"), receipt);
    assert_eq!(row.get::<i16, _>("provenance_version"), 1);
    assert_eq!(row.get::<String, _>("session_purpose"), "enrollment");
    assert_eq!(row.get::<String, _>("source_kind"), "bootstrap_otp");
    assert_eq!(row.get::<Uuid, _>("family_operation"), operation);
    assert!(
        row.get::<Option<OffsetDateTime>, _>("consumed_at")
            .is_some()
    );
    assert_eq!(row.get::<OffsetDateTime, _>("expires_at"), deadline);
    assert_eq!(row.get::<OffsetDateTime, _>("token_expires_at"), deadline);
    let audit_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_events WHERE action='auth.first_enrollment.admit' \
         AND target_id=$1 AND org_id=$2",
    )
    .bind(operation.to_string())
    .bind(company)
    .fetch_one(&owner)
    .await
    .unwrap();
    assert_eq!(audit_count, 1);

    let replay = command
        .admit(&otp, operation, status_hash, OffsetDateTime::now_utc())
        .await
        .unwrap();
    assert!(
        matches!(replay, FirstEnrollmentOutcome::AlreadyCommitted { receipt_id } if receipt_id == receipt)
    );
    let wrong_secret = command
        .admit(&otp, operation, [0_u8; 32], OffsetDateTime::now_utc())
        .await;
    assert!(matches!(
        wrong_secret,
        Err(ProvisioningError::InvalidBootstrapCredential)
    ));
    let family_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM auth_refresh_token_families WHERE source_operation_id=$1",
    )
    .bind(operation)
    .fetch_one(&owner)
    .await
    .unwrap();
    assert_eq!(family_count, 1, "replay must never mint a second bearer");

    let restricted = command_pool(&owner).await;
    let mut duplicate = restricted.begin().await.unwrap();
    sqlx::query("SELECT set_config('app.current_org',$1,true)")
        .bind(company.to_string())
        .execute(duplicate.as_mut())
        .await
        .unwrap();
    let duplicate_error = sqlx::query("SELECT auth_security.append_first_enrollment_audit($1)")
        .bind(operation)
        .execute(duplicate.as_mut())
        .await
        .unwrap_err();
    assert_eq!(
        duplicate_error
            .as_database_error()
            .and_then(|e| e.code())
            .as_deref(),
        Some("23505")
    );
    duplicate.rollback().await.unwrap();

    let mut wrong_company = restricted.begin().await.unwrap();
    sqlx::query("SELECT set_config('app.current_org',$1,true)")
        .bind(Uuid::new_v4().to_string())
        .execute(wrong_company.as_mut())
        .await
        .unwrap();
    let wrong_error = sqlx::query("SELECT auth_security.append_first_enrollment_audit($1)")
        .bind(operation)
        .execute(wrong_company.as_mut())
        .await
        .unwrap_err();
    assert_eq!(
        wrong_error
            .as_database_error()
            .and_then(|e| e.code())
            .as_deref(),
        Some("42501")
    );
    wrong_company.rollback().await.unwrap();

    let runtime_error = sqlx::query("SELECT auth_security.append_first_enrollment_audit($1)")
        .bind(operation)
        .execute(&runtime_pool(&owner).await)
        .await
        .unwrap_err();
    assert_eq!(
        runtime_error
            .as_database_error()
            .and_then(|e| e.code())
            .as_deref(),
        Some("42501")
    );

    let mut reset = restricted.begin().await.unwrap();
    sqlx::query("SELECT set_config('app.current_org',$1,true)")
        .bind(company.to_string())
        .execute(reset.as_mut())
        .await
        .unwrap();
    let reset_error =
        sqlx::query("UPDATE auth_bootstrap_credentials SET consumed_at=NULL WHERE id=$1")
            .bind(source)
            .execute(reset.as_mut())
            .await
            .unwrap_err();
    assert_eq!(
        reset_error
            .as_database_error()
            .and_then(|e| e.code())
            .as_deref(),
        Some("23514"),
        "a consumed version-1 source cannot be reopened"
    );
    reset.rollback().await.unwrap();
}

#[sqlx::test(migrations = "../db/migrations")]
async fn command_role_has_company_custody_but_no_business_or_audit_write(owner: PgPool) {
    let company = Uuid::new_v4();
    let other_company = Uuid::new_v4();
    sqlx::query("INSERT INTO organizations(id,slug,name) VALUES($1,'command-scope','Scope')")
        .bind(company)
        .execute(&owner)
        .await
        .unwrap();
    sqlx::query("INSERT INTO organizations(id,slug,name) VALUES($1,'command-other','Other')")
        .bind(other_company)
        .execute(&owner)
        .await
        .unwrap();
    let (account, source, otp, _) = first_source(&owner, company).await;
    first_source(&owner, other_company).await;
    let wrong_role = FirstEnrollmentCommandPool::new(runtime_pool(&owner).await)
        .admit(&otp, Uuid::new_v4(), [1_u8; 32], OffsetDateTime::now_utc())
        .await;
    assert!(matches!(
        wrong_role,
        Err(ProvisioningError::InvalidBootstrapCredential)
    ));
    let unconsumed: Option<OffsetDateTime> =
        sqlx::query_scalar("SELECT consumed_at FROM auth_bootstrap_credentials WHERE id=$1")
            .bind(source)
            .fetch_one(&owner)
            .await
            .unwrap();
    assert_eq!(unconsumed, None);
    let restricted = command_pool(&owner).await;
    let mut tx = restricted.begin().await.unwrap();
    let unarmed: i64 = sqlx::query_scalar("SELECT count(*) FROM auth_security.account_state")
        .fetch_one(tx.as_mut())
        .await
        .unwrap();
    assert_eq!(unarmed, 0);
    sqlx::query("SELECT set_config('app.current_org',$1,true)")
        .bind(company.to_string())
        .execute(tx.as_mut())
        .await
        .unwrap();
    let armed: i64 = sqlx::query_scalar("SELECT count(*) FROM auth_security.account_state")
        .fetch_one(tx.as_mut())
        .await
        .unwrap();
    assert_eq!(armed, 1);
    for query in [
        "SELECT is_active FROM users WHERE id=$1 FOR NO KEY UPDATE",
        "UPDATE users SET is_active=false WHERE id=$1",
    ] {
        let mut savepoint = tx.begin().await.unwrap();
        let denied = sqlx::query(query)
            .bind(account)
            .execute(savepoint.as_mut())
            .await
            .unwrap_err();
        assert_eq!(
            denied.as_database_error().and_then(|e| e.code()).as_deref(),
            Some("42501")
        );
        savepoint.rollback().await.unwrap();
    }
    let mut savepoint = tx.begin().await.unwrap();
    let denied = sqlx::query("INSERT INTO audit_events(actor,action,target_type,target_id,trace_id,span_id,occurred_at,org_id) VALUES($1,'auth.fake','x','x',repeat('0',32),repeat('0',16),clock_timestamp(),$2)")
        .bind(account).bind(company).execute(savepoint.as_mut()).await.unwrap_err();
    assert_eq!(
        denied.as_database_error().and_then(|e| e.code()).as_deref(),
        Some("42501")
    );
    savepoint.rollback().await.unwrap();
    let business_write: bool = sqlx::query_scalar(
        "SELECT has_table_privilege(current_user,'public.work_orders','INSERT') \
         OR has_table_privilege(current_user,'public.payroll_draft_runs','INSERT')",
    )
    .fetch_one(tx.as_mut())
    .await
    .unwrap();
    assert!(!business_write);
    tx.rollback().await.unwrap();
}

#[sqlx::test(migrations = "../db/migrations")]
async fn mismatched_committed_evidence_cannot_be_audited_or_partially_kept(owner: PgPool) {
    let company = Uuid::new_v4();
    sqlx::query("INSERT INTO organizations(id,slug,name) VALUES($1,'command-mismatch','Mismatch')")
        .bind(company)
        .execute(&owner)
        .await
        .unwrap();
    let (account, source, _, issuance) = first_source(&owner, company).await;
    let operation = Uuid::new_v4();
    let family = Uuid::new_v4();
    let receipt = Uuid::new_v4();
    let wrong_issuance = Uuid::new_v4();
    assert_ne!(wrong_issuance, issuance);
    let now = OffsetDateTime::now_utc();
    let mut tx = owner.begin().await.unwrap();
    sqlx::query("SELECT set_config('app.current_org',$1,true)")
        .bind(company.to_string())
        .execute(tx.as_mut())
        .await
        .unwrap();
    sqlx::query("UPDATE auth_bootstrap_credentials SET consumed_at=$1 WHERE id=$2")
        .bind(now)
        .bind(source)
        .execute(tx.as_mut())
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO auth_security.session_admissions \
         (operation_id,status_secret_hash,account_id,home_org_id,auth_generation,session_purpose,expires_at) \
         VALUES($1,$2,$3,$4,1,'enrollment',$5)",
    )
    .bind(operation)
    .bind(Sha256::digest(b"test-mismatch-status").to_vec())
    .bind(account)
    .bind(company)
    .bind(now + Duration::minutes(15))
    .execute(tx.as_mut())
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO auth_refresh_token_families \
         (id,user_id,created_at,org_id,provenance_version,auth_generation,session_purpose,source_kind,source_operation_id) \
         VALUES($1,$2,$3,$4,1,1,'enrollment','bootstrap_otp',$5)",
    )
    .bind(family)
    .bind(account)
    .bind(now)
    .bind(company)
    .bind(operation)
    .execute(tx.as_mut())
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO auth_refresh_tokens \
         (id,family_id,user_id,token_hash,issued_at,expires_at,org_id) \
         VALUES($1,$2,$3,$4,$5,$6,$7)",
    )
    .bind(Uuid::new_v4())
    .bind(family)
    .bind(account)
    .bind(Sha256::digest(b"test-mismatch-token").to_vec())
    .bind(now)
    .bind(now + Duration::minutes(15))
    .bind(company)
    .execute(tx.as_mut())
    .await
    .unwrap();
    sqlx::query(
        "UPDATE auth_security.session_admissions \
         SET status='committed',source_kind='bootstrap_otp',source_row_id=$2, \
             source_operation_id=$3,family_id=$4,receipt_id=$5,finalized_at=clock_timestamp() \
         WHERE operation_id=$1",
    )
    .bind(operation)
    .bind(source)
    .bind(wrong_issuance) // Does not match the OTP issuance operation.
    .bind(family)
    .bind(receipt)
    .execute(tx.as_mut())
    .await
    .unwrap();
    sqlx::query("SET LOCAL ROLE console_auth_cmd")
        .execute(tx.as_mut())
        .await
        .unwrap();
    let mismatch = sqlx::query("SELECT auth_security.append_first_enrollment_audit($1)")
        .bind(operation)
        .execute(tx.as_mut())
        .await
        .unwrap_err();
    assert_eq!(
        mismatch
            .as_database_error()
            .and_then(|e| e.code())
            .as_deref(),
        Some("42501")
    );
    tx.rollback().await.unwrap();
    let remaining: (Option<OffsetDateTime>, i64, i64, i64) = sqlx::query_as(
        "SELECT b.consumed_at, \
         (SELECT count(*) FROM auth_security.session_admissions WHERE operation_id=$2), \
         (SELECT count(*) FROM auth_refresh_token_families WHERE id=$3), \
         (SELECT count(*) FROM audit_events WHERE action='auth.first_enrollment.admit' AND target_id=$2::TEXT) \
         FROM auth_bootstrap_credentials b WHERE b.id=$1",
    )
    .bind(source)
    .bind(operation)
    .bind(family)
    .fetch_one(&owner)
    .await
    .unwrap();
    assert_eq!(remaining, (None, 0, 0, 0));
}

#[sqlx::test(migrations = "../db/migrations")]
async fn direct_v1_consumption_cannot_commit_without_completed_admission(owner: PgPool) {
    let company = Uuid::new_v4();
    sqlx::query("INSERT INTO organizations(id,slug,name) VALUES($1,'command-orphan','Orphan')")
        .bind(company)
        .execute(&owner)
        .await
        .unwrap();
    let (_, source, otp, _) = first_source(&owner, company).await;
    let restricted = command_pool(&owner).await;
    let mut tx = restricted.begin().await.unwrap();
    let proof = BootstrapCredentialStore
        .redeem_otp_in_tx(&mut tx, &otp, OffsetDateTime::now_utc())
        .await
        .unwrap();
    assert_eq!(proof.source.credential_id, source);
    let commit = tx.commit().await.unwrap_err();
    assert_eq!(
        commit.as_database_error().and_then(|e| e.code()).as_deref(),
        Some("23514"),
        "a consumed version-1 proof must have a completed admission at COMMIT"
    );
    let consumed: Option<OffsetDateTime> =
        sqlx::query_scalar("SELECT consumed_at FROM auth_bootstrap_credentials WHERE id=$1")
            .bind(source)
            .fetch_one(&owner)
            .await
            .unwrap();
    assert_eq!(consumed, None);

    let family = Uuid::new_v4();
    let mut orphan_family = restricted.begin().await.unwrap();
    sqlx::query("SELECT set_config('app.current_org',$1,true)")
        .bind(company.to_string())
        .execute(orphan_family.as_mut())
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO auth_refresh_token_families \
         (id,user_id,created_at,org_id,provenance_version,auth_generation,session_purpose,source_kind,source_operation_id) \
         SELECT $1,b.user_id,$2,b.org_id,1,1,'enrollment','bootstrap_otp',$3 \
         FROM auth_bootstrap_credentials b WHERE b.id=$4",
    )
    .bind(family)
    .bind(OffsetDateTime::now_utc())
    .bind(Uuid::new_v4())
    .bind(source)
    .execute(orphan_family.as_mut())
    .await
    .unwrap();
    let orphan_commit = orphan_family.commit().await.unwrap_err();
    assert_eq!(
        orphan_commit
            .as_database_error()
            .and_then(|e| e.code())
            .as_deref(),
        Some("23514"),
        "a version-1 family without a completed admission must not commit"
    );
    let family_count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM auth_refresh_token_families WHERE id=$1")
            .bind(family)
            .fetch_one(&owner)
            .await
            .unwrap();
    assert_eq!(family_count, 0);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn command_role_cannot_commit_fabricated_admission(owner: PgPool) {
    let company = Uuid::new_v4();
    sqlx::query("INSERT INTO organizations(id,slug,name) VALUES($1,'command-fake','Fake')")
        .bind(company)
        .execute(&owner)
        .await
        .unwrap();
    let (account, source, _, _) = first_source(&owner, company).await;
    let operation = Uuid::new_v4();
    let restricted = command_pool(&owner).await;
    let mut tx = restricted.begin().await.unwrap();
    sqlx::query("SELECT set_config('app.current_org',$1,true)")
        .bind(company.to_string())
        .execute(tx.as_mut())
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO auth_security.session_admissions \
         (operation_id,status_secret_hash,account_id,home_org_id,auth_generation,session_purpose,expires_at) \
         VALUES($1,$2,$3,$4,1,'enrollment',clock_timestamp()+interval '15 minutes')",
    )
    .bind(operation)
    .bind(vec![31_u8; 32])
    .bind(account)
    .bind(company)
    .execute(tx.as_mut())
    .await
    .unwrap();
    sqlx::query(
        "UPDATE auth_security.session_admissions \
         SET status='committed',source_kind='bootstrap_otp',source_row_id=$2, \
             source_operation_id=$3,family_id=$4,receipt_id=$5,finalized_at=clock_timestamp() \
         WHERE operation_id=$1",
    )
    .bind(operation)
    .bind(Uuid::new_v4())
    .bind(Uuid::new_v4())
    .bind(Uuid::new_v4())
    .bind(Uuid::new_v4())
    .execute(tx.as_mut())
    .await
    .unwrap();

    let commit = tx.commit().await.unwrap_err();
    assert_eq!(
        commit.as_database_error().and_then(|e| e.code()).as_deref(),
        Some("23514"),
        "a fabricated admission must fail at COMMIT without source, family, token and audit"
    );
    let admissions: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM auth_security.session_admissions WHERE operation_id=$1",
    )
    .bind(operation)
    .fetch_one(&owner)
    .await
    .unwrap();
    assert_eq!(admissions, 0);
    let consumed: Option<OffsetDateTime> =
        sqlx::query_scalar("SELECT consumed_at FROM auth_bootstrap_credentials WHERE id=$1")
            .bind(source)
            .fetch_one(&owner)
            .await
            .unwrap();
    assert_eq!(consumed, None);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn two_operations_contending_for_one_source_admit_only_once(owner: PgPool) {
    let company = Uuid::new_v4();
    sqlx::query("INSERT INTO organizations(id,slug,name) VALUES($1,'command-race','Race')")
        .bind(company)
        .execute(&owner)
        .await
        .unwrap();
    let (account, source, otp, _) = first_source(&owner, company).await;
    let command = FirstEnrollmentCommandPool::new(command_pool(&owner).await);
    let first_operation = Uuid::new_v4();
    let second_operation = Uuid::new_v4();
    let (first, second) = tokio::join!(
        command.admit(
            &otp,
            first_operation,
            [11_u8; 32],
            OffsetDateTime::now_utc()
        ),
        command.admit(
            &otp,
            second_operation,
            [22_u8; 32],
            OffsetDateTime::now_utc()
        )
    );
    let outcomes = [first, second];
    assert_eq!(
        outcomes
            .iter()
            .filter(|outcome| matches!(
                outcome,
                Ok(FirstEnrollmentOutcome::LocallyCommitted { .. })
            ))
            .count(),
        1
    );
    assert_eq!(
        outcomes
            .iter()
            .filter(|outcome| matches!(outcome, Err(ProvisioningError::InvalidBootstrapCredential)))
            .count(),
        1
    );
    let admitted: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM auth_security.session_admissions \
         WHERE account_id=$1 AND source_row_id=$2 AND status='committed'",
    )
    .bind(account)
    .bind(source)
    .fetch_one(&owner)
    .await
    .unwrap();
    assert_eq!(admitted, 1);
    let families: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM auth_refresh_token_families \
         WHERE user_id=$1 AND provenance_version=1 AND session_purpose='enrollment'",
    )
    .bind(account)
    .fetch_one(&owner)
    .await
    .unwrap();
    assert_eq!(families, 1);
}
