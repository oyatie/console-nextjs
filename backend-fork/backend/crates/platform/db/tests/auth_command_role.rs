#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::borrow::Cow;

use sqlx::migrate::Migrator;
use sqlx::{Acquire, PgPool, Row};
use uuid::Uuid;

static MIGRATOR: Migrator = sqlx::migrate!("./migrations");

#[sqlx::test(migrations = "./migrations")]
async fn auth_command_role_is_private_and_expand_only(pool: PgPool) {
    let role = sqlx::query(
        "SELECT rolcanlogin, rolsuper, rolbypassrls, rolinherit, rolcreatedb, rolcreaterole, rolreplication \
         FROM pg_roles WHERE rolname = 'console_auth_cmd'",
    )
    .fetch_optional(&pool)
    .await
    .unwrap()
    .expect("authentication command role must be preprovisioned");
    assert!(!role.get::<bool, _>("rolcanlogin"));
    for attribute in [
        "rolsuper",
        "rolbypassrls",
        "rolinherit",
        "rolcreatedb",
        "rolcreaterole",
        "rolreplication",
    ] {
        assert!(!role.get::<bool, _>(attribute), "unsafe {attribute}");
    }

    let membership: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM pg_auth_members \
         WHERE roleid='console_auth_cmd'::regrole OR member='console_auth_cmd'::regrole)",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(!membership, "auth role must not inherit or be inherited");
    let owned_objects: i64 = sqlx::query_scalar(
        "SELECT (SELECT count(*) FROM pg_class WHERE relowner='console_auth_cmd'::regrole) \
         + (SELECT count(*) FROM pg_namespace WHERE nspowner='console_auth_cmd'::regrole)",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        owned_objects, 0,
        "command role must own no schema or relation"
    );
    let (can_read_active, can_read_phone): (bool, bool) = sqlx::query_as(
        "SELECT has_column_privilege('console_auth_cmd','public.users','is_active','SELECT'), \
         has_column_privilege('console_auth_cmd','public.users','phone','SELECT')",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(can_read_active, "the owner needs only live Account status");
    assert!(!can_read_phone, "roster profile data is outside this stage");

    for (table, verbs) in [
        ("auth_security.account_state", "SELECT,UPDATE"),
        ("auth_security.credential_removals", "SELECT"),
        ("auth_security.session_admissions", "SELECT,INSERT,UPDATE"),
        ("auth_security.registration_intents", "SELECT,INSERT,UPDATE"),
        (
            "public.auth_webauthn_credentials",
            "SELECT,INSERT,UPDATE,DELETE",
        ),
        ("public.auth_webauthn_ceremonies", "SELECT,INSERT,UPDATE"),
        ("public.auth_webauthn_ceremony_bindings", "SELECT,INSERT"),
        ("public.auth_bootstrap_credentials", "SELECT,INSERT,UPDATE"),
        ("public.auth_refresh_token_families", "SELECT,INSERT,UPDATE"),
        ("public.auth_refresh_tokens", "SELECT,INSERT,UPDATE"),
        ("public.auth_device_login_handoffs", "SELECT,INSERT,UPDATE"),
    ] {
        for verb in verbs.split(',') {
            let allowed: bool =
                sqlx::query_scalar("SELECT has_table_privilege('console_auth_cmd', $1, $2)")
                    .bind(table)
                    .bind(verb)
                    .fetch_one(&pool)
                    .await
                    .unwrap();
            assert!(allowed, "missing {verb} on {table}");
        }
    }
    for (table, verb) in [
        ("auth_security.account_id_reservations", "SELECT"),
        ("auth_security.account_state", "INSERT"),
        ("auth_security.session_admissions", "DELETE"),
        ("public.auth_webauthn_credentials", "TRUNCATE"),
        ("public.auth_bootstrap_credentials", "DELETE"),
        ("public.users", "INSERT"),
        ("public.users", "UPDATE"),
        ("public.work_orders", "INSERT"),
        ("public.auth_rate_limit", "INSERT"),
        ("public.audit_events", "INSERT"),
        ("public.audit_events", "SELECT"),
    ] {
        let allowed: bool =
            sqlx::query_scalar("SELECT has_table_privilege('console_auth_cmd', $1, $2)")
                .bind(table)
                .bind(verb)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(!allowed, "unexpected {verb} on {table}");
    }

    let private_to_runtime: bool = sqlx::query_scalar(
        "SELECT has_schema_privilege('console_rt', 'auth_security', 'USAGE') \
         OR has_table_privilege('console_rt', 'auth_security.session_admissions', 'SELECT')",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(!private_to_runtime);

    let legacy_reader_still_works: bool = sqlx::query_scalar(
        "SELECT has_table_privilege('console_rt', 'public.auth_webauthn_credentials', 'SELECT')",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(
        legacy_reader_still_works,
        "expand must preserve old callers"
    );
    for table in [
        "public.auth_legacy_otp_family_sources",
        "public.auth_legacy_registration_bindings",
    ] {
        for verb in ["SELECT", "INSERT"] {
            let allowed: bool =
                sqlx::query_scalar("SELECT has_table_privilege('console_rt',$1,$2)")
                    .bind(table)
                    .bind(verb)
                    .fetch_one(&pool)
                    .await
                    .unwrap();
            assert!(allowed, "runtime needs {verb} on {table}");
        }
        for verb in ["UPDATE", "DELETE", "TRUNCATE"] {
            let allowed: bool =
                sqlx::query_scalar("SELECT has_table_privilege('console_rt',$1,$2)")
                    .bind(table)
                    .bind(verb)
                    .fetch_one(&pool)
                    .await
                    .unwrap();
            assert!(
                !allowed,
                "runtime may not {verb} an accepted binding in {table}"
            );
        }
    }
}

#[sqlx::test(migrations = false)]
async fn auth_command_expand_preserves_populated_0231_database(pool: PgPool) {
    let through_0231 = Migrator {
        migrations: Cow::Owned(
            MIGRATOR
                .iter()
                .filter(|migration| migration.version <= 231)
                .cloned()
                .collect(),
        ),
        ..Migrator::DEFAULT
    };
    through_0231.run(&pool).await.unwrap();

    let company = Uuid::new_v4();
    let account = Uuid::new_v4();
    let otp = Uuid::new_v4();
    let family = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO organizations (id,slug,name) VALUES ($1,'auth-upgrade','Auth upgrade')",
    )
    .bind(company)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO users (id,display_name,org_id,roles) VALUES ($1,'Legacy account',$2,ARRAY['MEMBER'])")
        .bind(account)
        .bind(company)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO auth_bootstrap_credentials (id,user_id,token_hash,issued_at,expires_at,org_id) VALUES ($1,$2,$3,clock_timestamp(),clock_timestamp()+interval '1 hour',$4)")
        .bind(otp)
        .bind(account)
        .bind(vec![7_u8; 32])
        .bind(company)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO auth_refresh_token_families (id,user_id,created_at,org_id) VALUES ($1,$2,clock_timestamp(),$3)")
        .bind(family)
        .bind(account)
        .bind(company)
        .execute(&pool)
        .await
        .unwrap();

    let before: serde_json::Value = sqlx::query_scalar(
        "SELECT jsonb_build_object( \
         'account',(SELECT to_jsonb(a) FROM auth_security.account_state a WHERE account_id=$1), \
         'otp',(SELECT to_jsonb(b) FROM auth_bootstrap_credentials b WHERE id=$2), \
         'family',(SELECT to_jsonb(f) FROM auth_refresh_token_families f WHERE id=$3))",
    )
    .bind(account)
    .bind(otp)
    .bind(family)
    .fetch_one(&pool)
    .await
    .unwrap();

    MIGRATOR.run(&pool).await.unwrap();

    let after: serde_json::Value = sqlx::query_scalar(
        "SELECT jsonb_build_object( \
         'account',(SELECT to_jsonb(a) FROM auth_security.account_state a WHERE account_id=$1), \
         'otp',(SELECT to_jsonb(b) FROM auth_bootstrap_credentials b WHERE id=$2), \
         'family',(SELECT to_jsonb(f) FROM auth_refresh_token_families f WHERE id=$3))",
    )
    .bind(account)
    .bind(otp)
    .bind(family)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        after, before,
        "role expansion must not rewrite live auth state"
    );
    MIGRATOR.run(&pool).await.unwrap();
    let after_retry: serde_json::Value = sqlx::query_scalar(
        "SELECT jsonb_build_object( \
         'account',(SELECT to_jsonb(a) FROM auth_security.account_state a WHERE account_id=$1), \
         'otp',(SELECT to_jsonb(b) FROM auth_bootstrap_credentials b WHERE id=$2), \
         'family',(SELECT to_jsonb(f) FROM auth_refresh_token_families f WHERE id=$3))",
    )
    .bind(account)
    .bind(otp)
    .bind(family)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(after_retry, after, "migration retry must not rewrite state");
}

#[sqlx::test(migrations = false)]
async fn first_enrollment_owner_rejects_unreconciled_v1_upgrade(pool: PgPool) {
    let through_0234 = Migrator {
        migrations: Cow::Owned(
            MIGRATOR
                .iter()
                .filter(|migration| migration.version <= 234)
                .cloned()
                .collect(),
        ),
        ..Migrator::DEFAULT
    };
    through_0234.run(&pool).await.unwrap();

    let company = Uuid::new_v4();
    let account = Uuid::new_v4();
    let source = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO organizations(id,slug,name) VALUES($1,'auth-v1-upgrade','V1 upgrade')",
    )
    .bind(company)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO users(id,display_name,org_id,roles) VALUES($1,'Account',$2,ARRAY['MEMBER'])",
    )
    .bind(account)
    .bind(company)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO auth_bootstrap_credentials \
         (id,user_id,token_hash,issued_at,expires_at,consumed_at,org_id, \
          issuance_version,issued_generation,issuance_purpose,source_operation_id) \
         VALUES($1,$2,$3,clock_timestamp()-interval '1 minute', \
                clock_timestamp()+interval '1 hour',clock_timestamp(),$4, \
                1,1,'first_enrollment',$5)",
    )
    .bind(source)
    .bind(account)
    .bind(vec![9_u8; 32])
    .bind(company)
    .bind(Uuid::new_v4())
    .execute(&pool)
    .await
    .unwrap();

    let error = MIGRATOR.run(&pool).await.unwrap_err();
    assert!(
        error.to_string().contains("require reconciliation"),
        "unexpected migration error: {error}"
    );
    let preserved: bool = sqlx::query_scalar(
        "SELECT consumed_at IS NOT NULL FROM auth_bootstrap_credentials WHERE id=$1",
    )
    .bind(source)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(preserved, "failed expansion must preserve the live source");
    let applied: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM _sqlx_migrations WHERE version=235 AND success)",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(
        !applied,
        "the owner migration must not be recorded as applied"
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn auth_command_can_lock_only_armed_company_account(pool: PgPool) {
    let company = Uuid::new_v4();
    let other_company = Uuid::new_v4();
    let account = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO organizations (id,slug,name) VALUES \
         ($1,'auth-lock-home','Home'),($2,'auth-lock-other','Other')",
    )
    .bind(company)
    .bind(other_company)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO users (id,display_name,org_id,roles) VALUES ($1,'Lock target',$2,ARRAY['MEMBER'])")
        .bind(account)
        .bind(company)
        .execute(&pool)
        .await
        .unwrap();

    let allowed: bool = sqlx::query_scalar(
        "SELECT has_function_privilege('console_auth_cmd', \
         'auth_security.lock_account_for_auth(uuid,uuid)', 'EXECUTE')",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let runtime_allowed: bool = sqlx::query_scalar(
        "SELECT has_function_privilege('console_rt', \
         'auth_security.lock_account_for_auth(uuid,uuid)', 'EXECUTE')",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(allowed);
    assert!(!runtime_allowed);

    for armed in [None, Some(other_company)] {
        let mut tx = pool.begin().await.unwrap();
        sqlx::query("SET LOCAL ROLE console_auth_cmd")
            .execute(&mut *tx)
            .await
            .unwrap();
        if let Some(company) = armed {
            sqlx::query("SELECT set_config('app.current_org',$1,true)")
                .bind(company.to_string())
                .execute(&mut *tx)
                .await
                .unwrap();
        }
        let result: Option<bool> =
            sqlx::query_scalar("SELECT auth_security.lock_account_for_auth($1,$2)")
                .bind(company)
                .bind(account)
                .fetch_one(&mut *tx)
                .await
                .unwrap();
        assert_eq!(result, None, "unarmed or wrong Company cannot lock Account");
    }

    let mut command = pool.begin().await.unwrap();
    sqlx::query("SET LOCAL ROLE console_auth_cmd")
        .execute(&mut *command)
        .await
        .unwrap();
    sqlx::query("SELECT set_config('app.current_org',$1,true)")
        .bind(company.to_string())
        .execute(&mut *command)
        .await
        .unwrap();
    let active: Option<bool> =
        sqlx::query_scalar("SELECT auth_security.lock_account_for_auth($1,$2)")
            .bind(company)
            .bind(account)
            .fetch_one(&mut *command)
            .await
            .unwrap();
    assert_eq!(active, Some(true));

    for query in [
        "SELECT is_active FROM users WHERE id=$1 FOR NO KEY UPDATE",
        "UPDATE users SET is_active=false WHERE id=$1",
    ] {
        let mut forbidden = command.begin().await.unwrap();
        let denied = sqlx::query(query)
            .bind(account)
            .execute(&mut *forbidden)
            .await
            .unwrap_err();
        assert_eq!(
            denied.as_database_error().and_then(|e| e.code()).as_deref(),
            Some("42501"),
            "command role must not lock or update users directly"
        );
        forbidden.rollback().await.unwrap();
    }

    let mut lifecycle = pool.begin().await.unwrap();
    sqlx::query("SELECT set_config('app.current_org',$1,true)")
        .bind(company.to_string())
        .execute(&mut *lifecycle)
        .await
        .unwrap();
    sqlx::query("SET LOCAL lock_timeout = '100ms'")
        .execute(&mut *lifecycle)
        .await
        .unwrap();
    let blocked = sqlx::query("UPDATE users SET is_active=false WHERE id=$1 AND org_id=$2")
        .bind(account)
        .bind(company)
        .execute(&mut *lifecycle)
        .await
        .unwrap_err();
    assert_eq!(
        blocked
            .as_database_error()
            .and_then(|e| e.code())
            .as_deref(),
        Some("55P03"),
        "lifecycle update must wait for the auth Account mutex"
    );
    lifecycle.rollback().await.unwrap();
    command.commit().await.unwrap();

    sqlx::query("UPDATE users SET is_active=false WHERE id=$1 AND org_id=$2")
        .bind(account)
        .bind(company)
        .execute(&pool)
        .await
        .unwrap();
    let is_active: bool = sqlx::query_scalar("SELECT is_active FROM users WHERE id=$1")
        .bind(account)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(!is_active, "lifecycle update proceeds after lock release");
}

#[sqlx::test(migrations = "./migrations")]
async fn auth_command_private_rows_require_armed_company(pool: PgPool) {
    let companies = [Uuid::new_v4(), Uuid::new_v4()];
    let accounts = [Uuid::new_v4(), Uuid::new_v4()];
    let admission_ids = [Uuid::new_v4(), Uuid::new_v4()];
    let intent_ids = [Uuid::new_v4(), Uuid::new_v4()];
    for index in 0..2 {
        let slug = format!("auth-private-{index}");
        sqlx::query("INSERT INTO organizations(id,slug,name) VALUES($1,$2,'Private Company')")
            .bind(companies[index])
            .bind(slug)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO users(id,display_name,org_id,roles) VALUES($1,'Private Account',$2,ARRAY['MEMBER'])")
            .bind(accounts[index])
            .bind(companies[index])
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO auth_security.credential_removals(credential_row_id,account_id,company_id,credential_id,credential_created_at,removed_at) VALUES($1,$2,$3,'removed',clock_timestamp(),clock_timestamp())")
            .bind(Uuid::new_v4()).bind(accounts[index]).bind(companies[index])
            .execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO auth_security.session_admissions(operation_id,status_secret_hash,account_id,home_org_id,auth_generation,session_purpose,expires_at) VALUES($1,$2,$3,$4,1,'enrollment',clock_timestamp()+interval '1 hour')")
            .bind(admission_ids[index]).bind(vec![index as u8 + 1; 32])
            .bind(accounts[index]).bind(companies[index]).execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO auth_security.registration_intents(ceremony_id,account_id,home_org_id,auth_generation,family_id,purpose,source_bootstrap_id,source_issuance_operation_id,expires_at) VALUES($1,$2,$3,1,$4,'first_enrollment',$5,$6,clock_timestamp()+interval '1 hour')")
            .bind(intent_ids[index]).bind(accounts[index]).bind(companies[index])
            .bind(Uuid::new_v4()).bind(Uuid::new_v4()).bind(Uuid::new_v4())
            .execute(&pool).await.unwrap();
    }

    let tables = [
        (
            "auth_security.account_state",
            "SELECT count(*) FROM auth_security.account_state",
        ),
        (
            "auth_security.credential_removals",
            "SELECT count(*) FROM auth_security.credential_removals",
        ),
        (
            "auth_security.session_admissions",
            "SELECT count(*) FROM auth_security.session_admissions",
        ),
        (
            "auth_security.registration_intents",
            "SELECT count(*) FROM auth_security.registration_intents",
        ),
    ];
    for (table, _) in tables {
        let (enabled, forced): (bool, bool) = sqlx::query_as(
            "SELECT relrowsecurity, relforcerowsecurity FROM pg_class WHERE oid=$1::regclass",
        )
        .bind(table)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(enabled && forced, "{table} must enforce Company RLS");
    }

    let mut tx = pool.begin().await.unwrap();
    sqlx::query("SET LOCAL ROLE console_auth_cmd")
        .execute(&mut *tx)
        .await
        .unwrap();
    for (table, query) in tables {
        let count: i64 = sqlx::query_scalar(query).fetch_one(&mut *tx).await.unwrap();
        assert_eq!(count, 0, "{table} must be empty without Company context");
    }
    sqlx::query("SELECT set_config('app.current_org',$1,true)")
        .bind(companies[0].to_string())
        .execute(&mut *tx)
        .await
        .unwrap();
    for (table, query) in tables {
        let count: i64 = sqlx::query_scalar(query).fetch_one(&mut *tx).await.unwrap();
        assert_eq!(count, 1, "{table} must expose only the armed Company");
    }
    let wrong_account: Option<Uuid> = sqlx::query_scalar(
        "SELECT account_id FROM auth_security.account_state WHERE account_id=$1",
    )
    .bind(accounts[1])
    .fetch_optional(&mut *tx)
    .await
    .unwrap();
    assert!(wrong_account.is_none());
    let wrong_update = sqlx::query(
        "UPDATE auth_security.account_state SET revision=revision+1 WHERE account_id=$1",
    )
    .bind(accounts[1])
    .execute(&mut *tx)
    .await
    .unwrap();
    assert_eq!(wrong_update.rows_affected(), 0);
    let mut denied = tx.begin().await.unwrap();
    let wrong_insert = sqlx::query("INSERT INTO auth_security.session_admissions(operation_id,status_secret_hash,account_id,home_org_id,auth_generation,session_purpose,expires_at) VALUES($1,$2,$3,$4,1,'enrollment',clock_timestamp()+interval '1 hour')")
        .bind(Uuid::new_v4()).bind(vec![9_u8; 32])
        .bind(accounts[1]).bind(companies[1])
        .execute(&mut *denied).await.unwrap_err();
    assert_eq!(
        wrong_insert
            .as_database_error()
            .and_then(|e| e.code())
            .as_deref(),
        Some("42501")
    );
    denied.rollback().await.unwrap();

    let mut wrong_account = tx.begin().await.unwrap();
    let admission_error = sqlx::query("INSERT INTO auth_security.session_admissions(operation_id,status_secret_hash,account_id,home_org_id,auth_generation,session_purpose,expires_at) VALUES($1,$2,$3,$4,1,'enrollment',clock_timestamp()+interval '1 hour')")
        .bind(Uuid::new_v4()).bind(vec![10_u8; 32])
        .bind(accounts[1]).bind(companies[0])
        .execute(&mut *wrong_account).await.unwrap_err();
    assert_eq!(
        admission_error
            .as_database_error()
            .and_then(|e| e.code())
            .as_deref(),
        Some("23503"),
        "same-Company column cannot conceal another Company's Account"
    );
    wrong_account.rollback().await.unwrap();

    let mut wrong_account = tx.begin().await.unwrap();
    let intent_error = sqlx::query("INSERT INTO auth_security.registration_intents(ceremony_id,account_id,home_org_id,auth_generation,family_id,purpose,source_bootstrap_id,source_issuance_operation_id,expires_at) VALUES($1,$2,$3,1,$4,'first_enrollment',$5,$6,clock_timestamp()+interval '1 hour')")
        .bind(Uuid::new_v4()).bind(accounts[1]).bind(companies[0])
        .bind(Uuid::new_v4()).bind(Uuid::new_v4()).bind(Uuid::new_v4())
        .execute(&mut *wrong_account).await.unwrap_err();
    assert_eq!(
        intent_error
            .as_database_error()
            .and_then(|e| e.code())
            .as_deref(),
        Some("23503"),
        "registration intent must bind the Account's true Company"
    );
    wrong_account.rollback().await.unwrap();
    tx.rollback().await.unwrap();
}
