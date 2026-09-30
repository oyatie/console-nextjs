#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use console_kernel_core::OrgId;
use console_platform_provisioning::RosterProvisioner;
use sqlx::{PgPool, Row};
use time::{Duration, OffsetDateTime};

async fn seed_branch(pool: &PgPool, region: &str, branch: &str) -> uuid::Uuid {
    let region_id: uuid::Uuid = sqlx::query_scalar(
        r#"
        INSERT INTO regions (name, org_id)
        VALUES ($1, $2)
        ON CONFLICT (org_id, name) DO UPDATE SET name = EXCLUDED.name
        RETURNING id
        "#,
    )
    .bind(region)
    .bind(*OrgId::knl().as_uuid())
    .fetch_one(pool)
    .await
    .unwrap();

    sqlx::query_scalar(
        "INSERT INTO branches (region_id, name, org_id) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(region_id)
    .bind(branch)
    .bind(*OrgId::knl().as_uuid())
    .fetch_one(pool)
    .await
    .unwrap()
}

#[sqlx::test(migrations = "../db/migrations")]
async fn bulk_roster_import_is_idempotent_and_reports_reconciliation_counts(pool: PgPool) {
    let seoul = seed_branch(&pool, "수도권", "서울").await;
    let incheon = seed_branch(&pool, "수도권", "인천").await;
    let provisioner = RosterProvisioner::new(Duration::hours(2));
    let now = OffsetDateTime::now_utc();

    let roster = r#"
    {
      "users": [
        {
          "display_name": "Kim Mechanic",
          "phone": "010-1000-0001",
          "team": "정비",
          "roles": ["MECHANIC"],
          "branches": [
            { "region": "수도권", "branch": "서울" }
          ]
        },
        {
          "display_name": "Park Admin",
          "phone": "010-1000-0002",
          "team": "관리",
          "roles": ["ADMIN", "EXECUTIVE"],
          "branches": [
            { "region": "수도권", "branch": "서울" },
            { "region": "수도권", "branch": "인천" }
          ]
        }
      ]
    }
    "#;

    let first = provisioner.import_json(&pool, roster, now).await.unwrap();
    assert_eq!(first.users_created, 2);
    assert_eq!(first.users_updated, 0);
    assert_eq!(first.users_unchanged, 0);
    assert_eq!(first.branch_memberships_added, 3);
    assert_eq!(first.branch_memberships_removed, 0);
    assert_eq!(first.bootstrap_credentials_issued.len(), 2);
    assert_eq!(first.changed_count(), 7);

    let second = provisioner
        .import_json(&pool, roster, now + Duration::minutes(1))
        .await
        .unwrap();
    assert_eq!(second.users_created, 0);
    assert_eq!(second.users_updated, 0);
    assert_eq!(second.users_unchanged, 2);
    assert_eq!(second.branch_memberships_added, 0);
    assert_eq!(second.branch_memberships_removed, 0);
    assert!(second.bootstrap_credentials_issued.is_empty());
    assert_eq!(second.changed_count(), 0);

    // Exclude the cold-start admin seeded by migration 0021 so the count reflects
    // only the roster import under test.
    let user_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM users WHERE display_name <> 'Cold Start Admin'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(user_count, 2);

    let bootstrap_count: i64 = sqlx::query_scalar(
        r#"
        SELECT COUNT(*)
        FROM auth_bootstrap_credentials c
        JOIN users u ON u.id = c.user_id
        WHERE u.display_name <> 'Cold Start Admin'
        "#,
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(bootstrap_count, 2);

    let admin = sqlx::query("SELECT id, roles, team FROM users WHERE phone = $1")
        .bind("010-1000-0002")
        .fetch_one(&pool)
        .await
        .unwrap();
    let admin_id: uuid::Uuid = admin.try_get("id").unwrap();
    let roles: Vec<String> = admin.try_get("roles").unwrap();
    let team: Option<String> = admin.try_get("team").unwrap();
    assert_eq!(roles, vec!["ADMIN", "EXECUTIVE"]);
    assert_eq!(team.as_deref(), Some("관리"));

    let branch_ids: Vec<uuid::Uuid> = sqlx::query_scalar(
        "SELECT branch_id FROM user_branches WHERE user_id = $1 ORDER BY branch_id",
    )
    .bind(admin_id)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(branch_ids.len(), 2);
    assert!(branch_ids.contains(&seoul));
    assert!(branch_ids.contains(&incheon));
}

#[sqlx::test(migrations = "../db/migrations")]
async fn roster_with_unknown_branch_rolls_back_without_partial_writes(pool: PgPool) {
    seed_branch(&pool, "수도권", "서울").await;
    let provisioner = RosterProvisioner::new(Duration::hours(2));

    let roster = r#"
    {
      "users": [
        {
          "display_name": "Valid User",
          "phone": "010-2000-0001",
          "team": "정비",
          "roles": ["MECHANIC"],
          "branches": [
            { "region": "수도권", "branch": "서울" }
          ]
        },
        {
          "display_name": "Bad Branch User",
          "phone": "010-2000-0002",
          "team": "정비",
          "roles": ["MECHANIC"],
          "branches": [
            { "region": "수도권", "branch": "없는지점" }
          ]
        }
      ]
    }
    "#;

    let err = provisioner
        .import_json(&pool, roster, OffsetDateTime::now_utc())
        .await
        .unwrap_err();
    assert!(err.to_string().contains("unknown branch"));

    // Exclude the cold-start admin seeded by migration 0021: the rollback under
    // test must leave no roster rows, but the seed is independent infrastructure.
    let user_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM users WHERE display_name <> 'Cold Start Admin'")
            .fetch_one(&pool)
            .await
            .unwrap();
    let membership_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM user_branches")
        .fetch_one(&pool)
        .await
        .unwrap();
    let bootstrap_count: i64 = sqlx::query_scalar(
        r#"
        SELECT COUNT(*)
        FROM auth_bootstrap_credentials c
        JOIN users u ON u.id = c.user_id
        WHERE u.display_name <> 'Cold Start Admin'
        "#,
    )
    .fetch_one(&pool)
    .await
    .unwrap();

    assert_eq!(user_count, 0);
    assert_eq!(membership_count, 0);
    assert_eq!(bootstrap_count, 0);
}
