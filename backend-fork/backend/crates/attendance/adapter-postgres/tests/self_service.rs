#![allow(clippy::expect_used, clippy::unwrap_used)]
//! PostgreSQL/RLS regressions for linked-employee attendance self-service.

use console_attendance_adapter_postgres::PgAttendanceStore;
use console_attendance_application::{
    ListOwnExceptions, OwnAttendanceExceptionPage, OwnWeek52Read, ReadOwnWeek52,
    SelfAttendanceScope,
};
use console_attendance_domain::AttendanceDateRange;
use console_kernel_core::{OrgId, UserId};
use console_platform_request_context::scope_org;
use console_platform_test_support::{runtime_role_pool, seed_branch, seed_user};
use sqlx::PgPool;
use time::{Date, Month, UtcOffset};
use uuid::Uuid;

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn self_service_reads_only_the_linked_employee_and_ignores_other_malformed_timelines(
    owner_pool: PgPool,
) {
    scope_org(OrgId::knl(), async move {
        let branch = seed_branch(&owner_pool, "attendance-self-service", "operations").await;
        let provisioner = seed_user(
            &owner_pool,
            "Employee Directory Provisioner",
            "SUPER_ADMIN",
            branch,
        )
        .await;
        let linked_user = seed_user(&owner_pool, "Linked employee", "MECHANIC", branch).await;
        let unlinked_user = seed_user(&owner_pool, "Unlinked employee", "MECHANIC", branch).await;
        let linked_employee =
            seed_employee(&owner_pool, branch, provisioner, "Linked employee").await;
        let other_employee =
            seed_employee(&owner_pool, branch, provisioner, "Other employee").await;
        sqlx::query("UPDATE users SET employee_id=$1 WHERE id=$2")
            .bind(linked_employee)
            .bind(*linked_user.as_uuid())
            .execute(&owner_pool)
            .await
            .unwrap();

        let monday = Date::from_calendar_date(2026, Month::July, 20).unwrap();
        let start = monday
            .with_hms(9, 0, 0)
            .unwrap()
            .assume_offset(UtcOffset::from_hms(9, 0, 0).unwrap());
        seed_attendance_record(
            &owner_pool,
            linked_employee,
            *linked_user.as_uuid(),
            "CLOCK_IN",
            start,
            "CLOCKED_IN",
        )
        .await;
        seed_attendance_record(
            &owner_pool,
            linked_employee,
            *linked_user.as_uuid(),
            "CLOCK_OUT",
            start + time::Duration::hours(8),
            "OFF_DUTY",
        )
        .await;
        // This other employee's unmatched CLOCK_IN would make the manager
        // aggregation fail. The self-service query must never observe it.
        seed_attendance_record(
            &owner_pool,
            other_employee,
            *linked_user.as_uuid(),
            "CLOCK_IN",
            start,
            "CLOCKED_IN",
        )
        .await;
        seed_exception(
            &owner_pool,
            linked_employee,
            *linked_user.as_uuid(),
            monday,
            "AT-OWN",
        )
        .await;
        seed_exception(
            &owner_pool,
            other_employee,
            *linked_user.as_uuid(),
            monday,
            "AT-OTHER",
        )
        .await;

        let store = PgAttendanceStore::new(runtime_role_pool(&owner_pool).await);
        let scope = SelfAttendanceScope {
            org_id: *OrgId::knl().as_uuid(),
            user_id: *linked_user.as_uuid(),
        };
        let range = AttendanceDateRange::new(monday, monday + time::Duration::days(7)).unwrap();
        let exceptions: OwnAttendanceExceptionPage = store
            .list_own_exceptions(
                scope,
                ListOwnExceptions::new(range, None, None, None).unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(exceptions.total, 1);
        assert_eq!(exceptions.items.len(), 1);
        assert_eq!(exceptions.items[0].code, "AT-OWN");
        assert_ne!(exceptions.items[0].code, "AT-OTHER");
        assert_self_service_json_omits_directory_fields(
            &serde_json::to_value(&exceptions.items[0]).expect("own exception DTO serializes"),
        );

        let week: OwnWeek52Read = store
            .read_own_week52(scope, ReadOwnWeek52::new(monday).unwrap())
            .await
            .unwrap()
            .expect("linked employee has a 200-compatible Week52 projection");
        assert_eq!(week.current_hours, 8.0);
        assert_eq!(week.projected_hours, 8.0);
        assert_self_service_json_omits_directory_fields(
            &serde_json::to_value(&week).expect("own week52 DTO serializes"),
        );

        let unlinked_scope = SelfAttendanceScope {
            org_id: *OrgId::knl().as_uuid(),
            user_id: *unlinked_user.as_uuid(),
        };
        let unlinked: OwnAttendanceExceptionPage = store
            .list_own_exceptions(
                unlinked_scope,
                ListOwnExceptions::new(
                    AttendanceDateRange::new(monday, monday + time::Duration::days(7)).unwrap(),
                    None,
                    None,
                    None,
                )
                .unwrap(),
            )
            .await
            .unwrap();
        assert!(unlinked.items.is_empty());
        assert_eq!(unlinked.total, 0);
        // Adapter not-available is Option::None (REST maps this to status=not_available).
        let unlinked_week: Option<OwnWeek52Read> = store
            .read_own_week52(unlinked_scope, ReadOwnWeek52::new(monday).unwrap())
            .await
            .unwrap();
        assert_eq!(unlinked_week, None);
    })
    .await;
}

fn assert_self_service_json_omits_directory_fields(value: &serde_json::Value) {
    let object = value
        .as_object()
        .expect("self-service read DTO serializes as a JSON object");
    for leaked in [
        "employee_id",
        "employee_name",
        "name",
        "team",
        "branch_id",
        "links",
    ] {
        assert!(
            !object.contains_key(leaked),
            "self-service JSON must not include {leaked}: {value}"
        );
    }
}

async fn seed_employee(
    pool: &PgPool,
    branch: console_kernel_core::BranchId,
    actor: UserId,
    name: &str,
) -> Uuid {
    let employee = Uuid::new_v4();
    let mut command = pool.begin().await.unwrap();
    sqlx::query("SET LOCAL ROLE console_leave_cmd")
        .execute(&mut *command)
        .await
        .unwrap();
    sqlx::query(
        "SELECT * FROM leave_api.create_employee(\
         $1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,0::numeric,$12,$13,$14,$15,$16)",
    )
    .bind(*OrgId::knl().as_uuid())
    .bind(employee)
    .bind(format!("SELF-{employee}"))
    .bind(name)
    .bind("attendance-self-service")
    .bind("REGULAR")
    .bind("+821012345678")
    .bind("Operations")
    .bind("Forklift operator")
    .bind("Seoul depot")
    .bind(*branch.as_uuid())
    .bind(format!("attendance-self-service-{employee}"))
    .bind("a".repeat(64))
    .bind(*actor.as_uuid())
    .bind("0123456789abcdef0123456789abcdef")
    .bind("0123456789abcdef")
    .fetch_one(&mut *command)
    .await
    .unwrap();
    command.commit().await.unwrap();
    employee
}

async fn seed_attendance_record(
    pool: &PgPool,
    employee_id: Uuid,
    actor_user_id: Uuid,
    kind: &str,
    occurred_at: time::OffsetDateTime,
    state_after: &str,
) {
    sqlx::query(
        "INSERT INTO employee_attendance_records \
         (org_id,employee_id,actor_user_id,kind,occurred_at,work_date,state_after,idempotency_key) \
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8)",
    )
    .bind(*OrgId::knl().as_uuid())
    .bind(employee_id)
    .bind(actor_user_id)
    .bind(kind)
    .bind(occurred_at)
    .bind(occurred_at.date())
    .bind(state_after)
    .bind(Uuid::new_v4().to_string())
    .execute(pool)
    .await
    .unwrap();
}

async fn seed_exception(
    pool: &PgPool,
    employee_id: Uuid,
    actor_user_id: Uuid,
    work_date: Date,
    code: &str,
) {
    sqlx::query(
        "INSERT INTO attendance_exceptions \
         (org_id,code,kind,employee_id,work_date,detail,idempotency_key,request_fingerprint,created_by) \
         VALUES ($1,$2,'LATE',$3,$4,'fixture',$5,$6,$7)",
    )
    .bind(*OrgId::knl().as_uuid())
    .bind(code)
    .bind(employee_id)
    .bind(work_date)
    .bind(format!("attendance-self-service-{code}"))
    .bind("b".repeat(64))
    .bind(actor_user_id)
    .execute(pool)
    .await
    .unwrap();
}
