#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
//! Regression for migration 0188's only permitted substitution transition.
//!
//! The test seeds an `ASSIGNED` row through the migration-owned schema and
//! drives the production `PgAttendanceStore` using the low-privilege runtime
//! role. It proves the adapter updates the migration's real `cancel_reason`
//! column while preserving the one-way `ASSIGNED -> CANCELLED` transition.

use std::sync::Arc;

use console_attendance_adapter_postgres::PgAttendanceStore;
use console_attendance_application::{
    AssignSubstitute, CallerScope, CancelSubstitution, ResolveException, SubstitutionCandidateQuery,
};
use console_attendance_domain::{ResolutionAction, SubstitutionWindow};
use console_kernel_core::{AuditAction, AuditEvent, OrgId, TraceContext, UserId};
use console_platform_db::{DbError, with_audits};
use console_platform_request_context::scope_org;
use console_platform_test_support::{
    runtime_role_pool, seed_branch, seed_org_and_super_admin, seed_user,
};
use serde_json::json;
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use time::{Duration, OffsetDateTime};
use tokio::sync::Barrier;
use uuid::Uuid;

const LOCK_WITNESS_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(10);

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn assigned_substitution_cancels_through_runtime_adapter_using_migration_0188_columns(
    owner_pool: PgPool,
) {
    scope_org(OrgId::knl(), async move {
        let branch = seed_branch(&owner_pool, "attendance-cancel", "operations").await;
        let provisioner = seed_user(
            &owner_pool,
            "Employee Directory Provisioner",
            "SUPER_ADMIN",
            branch,
        )
        .await;
        let actor = seed_user(&owner_pool, "Attendance Manager", "ADMIN", branch).await;
        let employee = seed_employee(&owner_pool, branch, provisioner, "Covered employee").await;
        let substitution_id =
            seed_assigned_substitution(&owner_pool, branch, actor, employee).await;

        let store = PgAttendanceStore::new(runtime_role_pool(&owner_pool).await);
        let caller = CallerScope {
            org_id: *OrgId::knl().as_uuid(),
            user_id: *actor.as_uuid(),
            branch_ids: vec![*branch.as_uuid()],
            org_wide: false,
        };
        let cancelled = store
            .cancel_substitution(
                &caller,
                CancelSubstitution {
                    substitution_id,
                    reason: "approved staffing change".to_owned(),
                },
            )
            .await
            .expect("runtime adapter must permit migration 0188 ASSIGNED -> CANCELLED");

        assert_eq!(cancelled.id, substitution_id);
        assert_eq!(cancelled.status, "CANCELLED");
        let persisted: (String, Option<String>) = sqlx::query_as(
            "SELECT status, cancel_reason FROM attendance_substitutions WHERE id=$1",
        )
        .bind(substitution_id)
        .fetch_one(&owner_pool)
        .await
        .unwrap();
        assert_eq!(persisted.0, "CANCELLED");
        assert_eq!(persisted.1.as_deref(), Some("approved staffing change"));
    })
    .await;
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn confirm_resolution_without_overtime_uses_a_typed_null_through_runtime_adapter(
    owner_pool: PgPool,
) {
    scope_org(OrgId::knl(), async move {
        let branch = seed_branch(
            &owner_pool,
            "attendance-confirm-no-overtime",
            "operations",
        )
        .await;
        let provisioner = seed_user(
            &owner_pool,
            "Employee Directory Provisioner",
            "SUPER_ADMIN",
            branch,
        )
        .await;
        let actor = seed_user(&owner_pool, "Attendance Manager", "ADMIN", branch).await;
        let employee =
            seed_employee(&owner_pool, branch, provisioner, "Confirmed employee").await;
        let exception_id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO attendance_exceptions (id,org_id,code,kind,employee_id,branch_id,work_date,detail,created_by,idempotency_key,request_fingerprint) VALUES ($1,$2,$3,'LATE',$4,$5,$6,'verified arrival',$7,$8,$9)",
        )
        .bind(exception_id)
        .bind(*OrgId::knl().as_uuid())
        .bind(format!("AT-{exception_id}"))
        .bind(employee)
        .bind(*branch.as_uuid())
        .bind(OffsetDateTime::now_utc().date())
        .bind(*actor.as_uuid())
        .bind(format!("confirm-no-overtime-{exception_id}"))
        .bind("a".repeat(64))
        .execute(&owner_pool)
        .await
        .unwrap();

        let store = PgAttendanceStore::new(runtime_role_pool(&owner_pool).await);
        let caller = CallerScope {
            org_id: *OrgId::knl().as_uuid(),
            user_id: *actor.as_uuid(),
            branch_ids: vec![*branch.as_uuid()],
            org_wide: false,
        };
        let resolved = store
            .resolve_exception(
                &caller,
                ResolveException {
                    exception_id,
                    action: ResolutionAction::Confirm,
                    reason: "verified arrival".to_owned(),
                    linked_work_ref: None,
                    overtime_minutes: None,
                },
            )
            .await
            .expect("CONFIRM without overtime must persist through the runtime adapter");

        assert_eq!(resolved.id, exception_id);
        assert_eq!(resolved.status, "RESOLVED");
        assert_eq!(
            resolved
                .resolution
                .expect("resolution must be returned")
                .ot_hours,
            None
        );
        let persisted: (String, Option<String>) = sqlx::query_as(
            "SELECT status, ot_hours::text FROM attendance_exceptions e JOIN attendance_exception_resolutions r ON r.exception_id=e.id WHERE e.id=$1",
        )
        .bind(exception_id)
        .fetch_one(&owner_pool)
        .await
        .unwrap();
        assert_eq!(persisted.0, "RESOLVED");
        assert_eq!(persisted.1, None);
        let audits: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM audit_events WHERE action='attendance.exception.resolve' AND target_id=$1",
        )
        .bind(exception_id.to_string())
        .fetch_one(&owner_pool)
        .await
        .unwrap();
        assert_eq!(audits, 1, "resolve must retain its domain audit event");
    })
    .await;
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn assignment_rechecks_eligible_worker_and_derives_canonical_hr_snapshot(owner_pool: PgPool) {
    scope_org(OrgId::knl(), async move {
        let branch = seed_branch(&owner_pool, "attendance-candidates", "operations").await;
        let provisioner = seed_user(
            &owner_pool,
            "Employee Directory Provisioner",
            "SUPER_ADMIN",
            branch,
        )
        .await;
        let actor = seed_user(&owner_pool, "Attendance Manager", "ADMIN", branch).await;
        let covered = seed_employee(&owner_pool, branch, provisioner, "Covered employee").await;
        let worker = seed_employee(&owner_pool, branch, provisioner, "Canonical worker").await;
        let store = PgAttendanceStore::new(runtime_role_pool(&owner_pool).await);
        let caller = CallerScope {
            org_id: *OrgId::knl().as_uuid(),
            user_id: *actor.as_uuid(),
            branch_ids: vec![*branch.as_uuid()],
            org_wide: false,
        };
        let window = SubstitutionWindow::new(
            OffsetDateTime::now_utc().date() + Duration::days(1),
            480,
            960,
        )
        .unwrap();
        let candidates = store
            .list_substitution_candidates(
                &caller,
                SubstitutionCandidateQuery::new(
                    *branch.as_uuid(),
                    covered,
                    window.clone(),
                    Some("Canonical".to_owned()),
                    None,
                    None,
                )
                .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(candidates.total, 1);
        assert_eq!(candidates.items[0].employee_id, worker);

        let command = assignment_command(
            window,
            *branch.as_uuid(),
            covered,
            worker,
            "attendance-candidate-assign-0001",
        );
        // A real pre-v2 fixture is inserted with its original fingerprint and
        // matching assignment audit in the same transaction; row immutability
        // intentionally prevents converting a newer fixture after the fact.
        let legacy_id = Uuid::new_v4();
        let legacy_fingerprint = legacy_v1_fingerprint(
            &caller,
            &command,
            Some(worker),
            "Canonical worker",
            "REGULAR",
            None,
        );
        let audit = AuditEvent::new(
            Some(UserId::from_uuid(caller.user_id)),
            AuditAction::new("attendance.substitution.assign").unwrap(),
            "attendance_substitution",
            legacy_id.to_string(),
            TraceContext::generate(),
            OffsetDateTime::now_utc(),
        )
        .with_org(OrgId::knl())
        .with_branch(branch);
        let fixture = command.clone();
        with_audits::<_, _, DbError>(store.pool(), OrgId::knl(), move |tx| {
            Box::pin(async move {
                sqlx::query("INSERT INTO attendance_substitutions (id,org_id,site,branch_id,role,cover_date,from_minutes,to_minutes,covered_employee_id,reason_kind,reason_detail,worker_employee_id,worker_name,worker_type,worker_rate,exception_id,idempotency_key,request_fingerprint,created_by) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19)")
                    .bind(legacy_id).bind(*OrgId::knl().as_uuid()).bind(&fixture.site).bind(fixture.branch_id).bind(&fixture.role).bind(fixture.window.cover_date).bind(fixture.window.from_minutes).bind(fixture.window.to_minutes).bind(fixture.covered_employee_id).bind(&fixture.reason_kind).bind(&fixture.reason_detail).bind(worker).bind("Canonical worker").bind("REGULAR").bind(Option::<String>::None).bind(fixture.exception_id).bind(&fixture.idempotency_key).bind(&legacy_fingerprint).bind(caller.user_id)
                    .execute(tx.as_mut()).await?;
                Ok(((), vec![audit]))
            })
        }).await.unwrap();
        let assigned = store.assign_substitute(&caller, command.clone()).await.unwrap();
        assert_eq!(assigned.id, legacy_id);
        assert_eq!(assigned.worker_rate, None, "client rate must be ignored");

        // A profile change after commit cannot invalidate the immutable request
        // contract or append a second assignment audit record on replay.
        sqlx::query("UPDATE employees SET name='Renamed worker' WHERE id=$1")
            .bind(worker)
            .execute(&owner_pool)
            .await
            .unwrap();
        sqlx::query("UPDATE employee_employment_profiles SET employment_type='CONTRACT', base_pay=12345.67 WHERE employee_id=$1")
            .bind(worker)
            .execute(&owner_pool)
            .await
            .unwrap();
        let replay = store.assign_substitute(&caller, command.clone()).await.unwrap();
        assert_eq!(replay.id, assigned.id);
        assert_eq!(replay.worker_name, "Canonical worker");
        assert_eq!(replay.worker_type, "REGULAR");
        assert_eq!(replay.worker_rate, None);
        let audits: i64 = sqlx::query_scalar("SELECT count(*) FROM audit_events WHERE action='attendance.substitution.assign' AND target_id=$1")
            .bind(assigned.id.to_string())
            .fetch_one(&owner_pool)
            .await
            .unwrap();
        assert_eq!(audits, 1);
        let cancelled = store
            .cancel_substitution(
                &caller,
                CancelSubstitution {
                    substitution_id: assigned.id,
                    reason: "staffing plan changed".to_owned(),
                },
            )
            .await
            .unwrap();
        assert_eq!(cancelled.status, "CANCELLED");
        let cancelled_replay = store.assign_substitute(&caller, command.clone()).await.unwrap();
        assert_eq!(cancelled_replay.id, assigned.id);
        assert_eq!(cancelled_replay.status, "CANCELLED");
        let mut changed = command;
        changed.role = "Different role".to_owned();
        assert!(store.assign_substitute(&caller, changed).await.is_err());
    })
    .await;
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn assignment_snapshots_canonical_employment_types_without_rate_reinterpretation(
    owner_pool: PgPool,
) {
    scope_org(OrgId::knl(), async move {
        let branch = seed_branch(&owner_pool, "attendance-profile-types", "operations").await;
        let provisioner = seed_user(&owner_pool, "Employee Directory Provisioner", "SUPER_ADMIN", branch).await;
        let actor = seed_user(&owner_pool, "Attendance Manager", "ADMIN", branch).await;
        let covered = seed_employee(&owner_pool, branch, provisioner, "Covered employee").await;
        let caller = CallerScope { org_id: *OrgId::knl().as_uuid(), user_id: *actor.as_uuid(), branch_ids: vec![*branch.as_uuid()], org_wide: false };
        let store = PgAttendanceStore::new(runtime_role_pool(&owner_pool).await);
        let day = OffsetDateTime::now_utc().date() + Duration::days(2);
        for (index, (employment_type, base_pay)) in [("REGULAR", "101.25"), ("CONTRACT", "202.50"), ("PART_TIME", "303.75")].iter().enumerate() {
            let worker = seed_employee(&owner_pool, branch, provisioner, &format!("{employment_type} worker")).await;
            sqlx::query("UPDATE employee_employment_profiles SET employment_type=$1, base_pay=$2::numeric WHERE employee_id=$3")
                .bind(*employment_type)
                .bind(*base_pay)
                .bind(worker)
                .execute(&owner_pool)
                .await
                .unwrap();
            let assigned = store.assign_substitute(&caller, assignment_command(
                SubstitutionWindow::new(day + Duration::days(index as i64), 480, 960).unwrap(),
                *branch.as_uuid(), covered, worker, &format!("attendance-profile-{index:04}"),
            )).await.unwrap();
            assert_eq!(assigned.worker_type, *employment_type);
            assert_eq!(assigned.worker_rate, None);
        }
    }).await;
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn candidate_page_keeps_half_open_boundary_and_enforces_branch_scope(owner_pool: PgPool) {
    scope_org(OrgId::knl(), async move {
        let branch = seed_branch(&owner_pool, "attendance-candidate-page", "operations").await;
        let provisioner = seed_user(
            &owner_pool,
            "Employee Directory Provisioner",
            "SUPER_ADMIN",
            branch,
        )
        .await;
        let actor = seed_user(&owner_pool, "Attendance Manager", "ADMIN", branch).await;
        let covered = seed_employee(&owner_pool, branch, provisioner, "Covered employee").await;
        let boundary_worker =
            seed_employee(&owner_pool, branch, provisioner, "Boundary worker").await;
        let other_worker = seed_employee(&owner_pool, branch, provisioner, "Other worker").await;
        let caller = CallerScope {
            org_id: *OrgId::knl().as_uuid(),
            user_id: *actor.as_uuid(),
            branch_ids: vec![*branch.as_uuid()],
            org_wide: false,
        };
        let store = PgAttendanceStore::new(runtime_role_pool(&owner_pool).await);
        let day = OffsetDateTime::now_utc().date() + Duration::days(4);
        store
            .assign_substitute(
                &caller,
                assignment_command(
                    SubstitutionWindow::new(day, 480, 960).unwrap(),
                    *branch.as_uuid(),
                    covered,
                    boundary_worker,
                    "attendance-boundary-assignment-0001",
                ),
            )
            .await
            .unwrap();
        let first_page = store
            .list_substitution_candidates(
                &caller,
                SubstitutionCandidateQuery::new(
                    *branch.as_uuid(),
                    covered,
                    SubstitutionWindow::new(day, 960, 1200).unwrap(),
                    Some("worker".to_owned()),
                    Some(1),
                    Some(0),
                )
                .unwrap(),
            )
            .await
            .unwrap();
        let page = store
            .list_substitution_candidates(
                &caller,
                SubstitutionCandidateQuery::new(
                    *branch.as_uuid(),
                    covered,
                    SubstitutionWindow::new(day, 960, 1200).unwrap(),
                    Some("worker".to_owned()),
                    Some(1),
                    Some(1),
                )
                .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(page.total, 2);
        assert_eq!(first_page.total, 2);
        assert_eq!(first_page.items[0].employee_id, boundary_worker);
        assert_eq!(page.items.len(), 1);
        assert_eq!(page.items[0].employee_id, other_worker);
        let unauthorized = CallerScope {
            branch_ids: vec![],
            ..caller
        };
        assert!(
            store
                .list_substitution_candidates(
                    &unauthorized,
                    SubstitutionCandidateQuery::new(
                        *branch.as_uuid(),
                        covered,
                        SubstitutionWindow::new(day, 960, 1200).unwrap(),
                        None,
                        Some(1),
                        Some(0),
                    )
                    .unwrap()
                )
                .await
                .is_err()
        );
        let other_org = Uuid::new_v4();
        let other_actor =
            seed_org_and_super_admin(&owner_pool, other_org, "attendance candidate other").await;
        let other_tenant = CallerScope {
            org_id: other_org,
            user_id: *other_actor.as_uuid(),
            branch_ids: vec![],
            org_wide: true,
        };
        let invisible = store
            .list_substitution_candidates(
                &other_tenant,
                SubstitutionCandidateQuery::new(
                    *branch.as_uuid(),
                    covered,
                    SubstitutionWindow::new(day, 960, 1200).unwrap(),
                    Some("worker".to_owned()),
                    None,
                    None,
                )
                .unwrap(),
            )
            .await
            .unwrap();
        assert!(invisible.items.is_empty());
        assert_eq!(invisible.total, 0);
    })
    .await;
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn employee_without_profile_is_not_a_candidate_or_assignable(owner_pool: PgPool) {
    scope_org(OrgId::knl(), async move {
        let branch = seed_branch(&owner_pool, "attendance-profile-required", "operations").await;
        let provisioner = seed_user(
            &owner_pool,
            "Employee Directory Provisioner",
            "SUPER_ADMIN",
            branch,
        )
        .await;
        let actor = seed_user(&owner_pool, "Attendance Manager", "ADMIN", branch).await;
        let covered = seed_employee(&owner_pool, branch, provisioner, "Covered employee").await;
        let worker =
            seed_employee(&owner_pool, branch, provisioner, "Profile missing worker").await;
        sqlx::query("DELETE FROM employee_employment_profiles WHERE employee_id=$1")
            .bind(worker)
            .execute(&owner_pool)
            .await
            .unwrap();
        let caller = CallerScope {
            org_id: *OrgId::knl().as_uuid(),
            user_id: *actor.as_uuid(),
            branch_ids: vec![*branch.as_uuid()],
            org_wide: false,
        };
        let store = PgAttendanceStore::new(runtime_role_pool(&owner_pool).await);
        let window = SubstitutionWindow::new(
            OffsetDateTime::now_utc().date() + Duration::days(5),
            480,
            960,
        )
        .unwrap();
        let candidates = store
            .list_substitution_candidates(
                &caller,
                SubstitutionCandidateQuery::new(
                    *branch.as_uuid(),
                    covered,
                    window.clone(),
                    None,
                    None,
                    None,
                )
                .unwrap(),
            )
            .await
            .unwrap();
        assert!(candidates.items.is_empty());
        assert_eq!(candidates.total, 0);
        assert!(
            store
                .assign_substitute(
                    &caller,
                    assignment_command(
                        window,
                        *branch.as_uuid(),
                        covered,
                        worker,
                        "attendance-profile-missing-0001",
                    )
                )
                .await
                .is_err()
        );
        let rows: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM attendance_substitutions WHERE worker_employee_id=$1",
        )
        .bind(worker)
        .fetch_one(&owner_pool)
        .await
        .unwrap();
        let audits: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM audit_events WHERE action='attendance.substitution.assign'",
        )
        .fetch_one(&owner_pool)
        .await
        .unwrap();
        assert_eq!((rows, audits), (0, 0));
    })
    .await;
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn concurrent_overlapping_assignments_serialize_worker_eligibility(owner_pool: PgPool) {
    scope_org(OrgId::knl(), async move {
        let branch = seed_branch(&owner_pool, "attendance-concurrent", "operations").await;
        let provisioner = seed_user(&owner_pool, "Employee Directory Provisioner", "SUPER_ADMIN", branch).await;
        let actor = seed_user(&owner_pool, "Attendance Manager", "ADMIN", branch).await;
        let covered = seed_employee(&owner_pool, branch, provisioner, "Covered employee").await;
        let worker = seed_employee(&owner_pool, branch, provisioner, "Concurrent worker").await;
        let caller = CallerScope { org_id: *OrgId::knl().as_uuid(), user_id: *actor.as_uuid(), branch_ids: vec![*branch.as_uuid()], org_wide: false };
        let store = PgAttendanceStore::new(runtime_role_pool(&owner_pool).await);
        let window = SubstitutionWindow::new(OffsetDateTime::now_utc().date() + Duration::days(3), 480, 960).unwrap();
        let barrier = Arc::new(Barrier::new(2));
        let first = async {
            barrier.clone().wait().await;
            store.assign_substitute(&caller, assignment_command(window.clone(), *branch.as_uuid(), covered, worker, "attendance-concurrent-0001")).await
        };
        let second = async {
            barrier.wait().await;
            store.assign_substitute(&caller, assignment_command(window.clone(), *branch.as_uuid(), covered, worker, "attendance-concurrent-0002")).await
        };
        let (first, second) = tokio::join!(first, second);
        assert_eq!(usize::from(first.is_ok()) + usize::from(second.is_ok()), 1);
        assert!(first.is_err() || second.is_err());
        let assigned: i64 = sqlx::query_scalar("SELECT count(*) FROM attendance_substitutions WHERE worker_employee_id=$1 AND cover_date=$2 AND status='ASSIGNED'")
            .bind(worker).bind(window.cover_date).fetch_one(&owner_pool).await.unwrap();
        assert_eq!(assigned, 1);
        let audits: i64 = sqlx::query_scalar("SELECT count(*) FROM audit_events WHERE action='attendance.substitution.assign'")
            .fetch_one(&owner_pool).await.unwrap();
        assert_eq!(audits, 1);
    }).await;
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn open_no_show_commit_blocks_the_single_connection_adapter_then_conflicts(
    owner_pool: PgPool,
) {
    scope_org(OrgId::knl(), async move {
        let branch = seed_branch(&owner_pool, "attendance-open-race", "operations").await;
        let provisioner = seed_user(&owner_pool, "Employee Directory Provisioner", "SUPER_ADMIN", branch).await;
        let actor = seed_user(&owner_pool, "Attendance Manager", "ADMIN", branch).await;
        let covered = seed_employee(&owner_pool, branch, provisioner, "Covered employee").await;
        let worker = seed_employee(&owner_pool, branch, provisioner, "NO_SHOW worker").await;
        let caller = CallerScope { org_id: *OrgId::knl().as_uuid(), user_id: *actor.as_uuid(), branch_ids: vec![*branch.as_uuid()], org_wide: false };
        let day = OffsetDateTime::now_utc().date() + Duration::days(10);
        let writer_pool =
            one_connection_pool(&owner_pool, "attendance-open-no-show-writer", PoolRole::Owner).await;
        let reader =
            one_connection_pool(&owner_pool, "attendance-open-no-show-reader", PoolRole::Runtime).await;
        let reader_session = reader_session(&reader).await;
        let mut writer = writer_pool.begin().await.unwrap();
        let writer_session = writer_session(&mut writer).await;
        sqlx::query("INSERT INTO attendance_exceptions (org_id,code,kind,employee_id,branch_id,work_date,detail,created_by,idempotency_key,request_fingerprint) VALUES ($1,$2,'NO_SHOW',$3,$4,$5,'unavailable',$6,$7,$8)")
            .bind(*OrgId::knl().as_uuid()).bind(format!("AT-{worker}")).bind(worker).bind(*branch.as_uuid()).bind(day).bind(*actor.as_uuid()).bind(format!("open-no-show-{worker}")).bind("a".repeat(64)).execute(&mut *writer).await.unwrap();
        let store = PgAttendanceStore::new(reader);
        let task = tokio::spawn(async move { scope_org(OrgId::knl(), store.assign_substitute(&caller, assignment_command(SubstitutionWindow::new(day, 480, 960).unwrap(), *branch.as_uuid(), covered, worker, "open-no-show-race"))).await });
        wait_for_advisory_lock_waiter(&owner_pool, &reader_session, &writer_session).await;
        writer.commit().await.unwrap();
        let error = task.await.unwrap().expect_err("fresh eligibility query must reject committed NO_SHOW");
        assert!(matches!(error, console_attendance_adapter_postgres::AttendanceStoreError::Conflict));
    }).await;
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn no_show_resolution_commit_releases_single_connection_adapter(owner_pool: PgPool) {
    scope_org(OrgId::knl(), async move {
        let branch = seed_branch(&owner_pool, "attendance-resolution-race", "operations").await;
        let provisioner = seed_user(&owner_pool, "Employee Directory Provisioner", "SUPER_ADMIN", branch).await;
        let actor = seed_user(&owner_pool, "Attendance Manager", "ADMIN", branch).await;
        let covered = seed_employee(&owner_pool, branch, provisioner, "Covered employee").await;
        let worker = seed_employee(&owner_pool, branch, provisioner, "Resolution worker").await;
        let day = OffsetDateTime::now_utc().date() + Duration::days(11);
        let exception = Uuid::new_v4();
        sqlx::query("INSERT INTO attendance_exceptions (id,org_id,code,kind,employee_id,branch_id,work_date,detail,created_by,idempotency_key,request_fingerprint) VALUES ($1,$2,$3,'NO_SHOW',$4,$5,$6,'unavailable',$7,$8,$9)").bind(exception).bind(*OrgId::knl().as_uuid()).bind(format!("AT-{worker}")).bind(worker).bind(*branch.as_uuid()).bind(day).bind(*actor.as_uuid()).bind(format!("resolution-no-show-{worker}")).bind("a".repeat(64)).execute(&owner_pool).await.unwrap();
        let writer_pool = one_connection_pool(
            &owner_pool,
            "attendance-resolve-no-show-writer",
            PoolRole::Owner,
        )
        .await;
        let reader = one_connection_pool(
            &owner_pool,
            "attendance-resolve-no-show-reader",
            PoolRole::Runtime,
        )
        .await;
        let reader_session = reader_session(&reader).await;
        let mut writer = writer_pool.begin().await.unwrap();
        let writer_session = writer_session(&mut writer).await;
        sqlx::query("INSERT INTO attendance_exception_resolutions (org_id,exception_id,action,reason,actor_user_id) VALUES ($1,$2,'CONFIRM','resolved',$3)").bind(*OrgId::knl().as_uuid()).bind(exception).bind(*actor.as_uuid()).execute(&mut *writer).await.unwrap();
        sqlx::query("UPDATE attendance_exceptions SET status='RESOLVED' WHERE id=$1").bind(exception).execute(&mut *writer).await.unwrap();
        let caller = CallerScope { org_id: *OrgId::knl().as_uuid(), user_id: *actor.as_uuid(), branch_ids: vec![*branch.as_uuid()], org_wide: false };
        let store = PgAttendanceStore::new(reader);
        let task = tokio::spawn(async move { scope_org(OrgId::knl(), store.assign_substitute(&caller, assignment_command(SubstitutionWindow::new(day, 480, 960).unwrap(), *branch.as_uuid(), covered, worker, "resolution-no-show-race"))).await });
        wait_for_advisory_lock_waiter(&owner_pool, &reader_session, &writer_session).await;
        writer.commit().await.unwrap();
        assert!(task.await.unwrap().is_ok(), "resolved NO_SHOW must permit assignment");
    }).await;
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn confirm_resolution_persists_without_overtime_hours(owner_pool: PgPool) {
    scope_org(OrgId::knl(), async move {
        let branch = seed_branch(
            &owner_pool,
            "attendance-resolution-without-overtime",
            "operations",
        )
        .await;
        let provisioner = seed_user(
            &owner_pool,
            "Employee Directory Provisioner",
            "SUPER_ADMIN",
            branch,
        )
        .await;
        let actor = seed_user(&owner_pool, "Attendance Manager", "ADMIN", branch).await;
        let employee =
            seed_employee(&owner_pool, branch, provisioner, "Resolution employee").await;
        let exception_id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO attendance_exceptions \
             (id,org_id,code,kind,employee_id,branch_id,work_date,detail,created_by,idempotency_key,request_fingerprint) \
             VALUES ($1,$2,$3,'NO_SHOW',$4,$5,$6,'unavailable',$7,$8,$9)",
        )
        .bind(exception_id)
        .bind(*OrgId::knl().as_uuid())
        .bind(format!("AT-{employee}"))
        .bind(employee)
        .bind(*branch.as_uuid())
        .bind(OffsetDateTime::now_utc().date() + Duration::days(12))
        .bind(*actor.as_uuid())
        .bind(format!("resolution-without-overtime-{employee}"))
        .bind("a".repeat(64))
        .execute(&owner_pool)
        .await
        .unwrap();

        let store = PgAttendanceStore::new(runtime_role_pool(&owner_pool).await);
        let caller = CallerScope {
            org_id: *OrgId::knl().as_uuid(),
            user_id: *actor.as_uuid(),
            branch_ids: vec![*branch.as_uuid()],
            org_wide: false,
        };
        let resolved = store
            .resolve_exception(
                &caller,
                ResolveException {
                    exception_id,
                    action: ResolutionAction::Confirm,
                    reason: "verified attendance exception".to_owned(),
                    linked_work_ref: None,
                    overtime_minutes: None,
                },
            )
            .await
            .expect("CONFIRM without overtime hours must persist through the runtime adapter");

        assert_eq!(resolved.status, "RESOLVED");
        assert_eq!(
            resolved
                .resolution
                .as_ref()
                .and_then(|resolution| resolution.ot_hours.as_deref()),
            None,
        );
        let persisted: (String, Option<String>, i64) = sqlx::query_as(
            "SELECT e.status,r.ot_hours::text, \
                    (SELECT count(*) FROM audit_events \
                     WHERE action='attendance.exception.resolve' AND target_id=e.id::text) \
             FROM attendance_exceptions e \
             JOIN attendance_exception_resolutions r \
               ON r.exception_id=e.id AND r.org_id=e.org_id \
             WHERE e.id=$1",
        )
        .bind(exception_id)
        .fetch_one(&owner_pool)
        .await
        .unwrap();
        assert_eq!(persisted, ("RESOLVED".to_owned(), None, 1));
    })
    .await;
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn overtime_resolution_round_trips_numeric_hours(owner_pool: PgPool) {
    scope_org(OrgId::knl(), async move {
        let branch = seed_branch(
            &owner_pool,
            "attendance-overtime-resolution",
            "operations",
        )
        .await;
        let provisioner = seed_user(
            &owner_pool,
            "Employee Directory Provisioner",
            "SUPER_ADMIN",
            branch,
        )
        .await;
        let actor = seed_user(&owner_pool, "Attendance Manager", "ADMIN", branch).await;
        let employee =
            seed_employee(&owner_pool, branch, provisioner, "Overtime employee").await;
        let exception_id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO attendance_exceptions \
             (id,org_id,code,kind,employee_id,branch_id,work_date,detail,created_by,idempotency_key,request_fingerprint) \
             VALUES ($1,$2,$3,'UNAPPROVED_OVERTIME',$4,$5,$6,'unapproved overtime',$7,$8,$9)",
        )
        .bind(exception_id)
        .bind(*OrgId::knl().as_uuid())
        .bind(format!("AT-{employee}"))
        .bind(employee)
        .bind(*branch.as_uuid())
        .bind(OffsetDateTime::now_utc().date() + Duration::days(13))
        .bind(*actor.as_uuid())
        .bind(format!("overtime-resolution-{employee}"))
        .bind("a".repeat(64))
        .execute(&owner_pool)
        .await
        .unwrap();

        let store = PgAttendanceStore::new(runtime_role_pool(&owner_pool).await);
        let caller = CallerScope {
            org_id: *OrgId::knl().as_uuid(),
            user_id: *actor.as_uuid(),
            branch_ids: vec![*branch.as_uuid()],
            org_wide: false,
        };
        let resolved = store
            .resolve_exception(
                &caller,
                ResolveException {
                    exception_id,
                    action: ResolutionAction::ApproveOvertime,
                    reason: "verified work order".to_owned(),
                    linked_work_ref: Some("WO-ATTENDANCE-001".to_owned()),
                    overtime_minutes: Some(90),
                },
            )
            .await
            .expect("approved overtime must round-trip the persisted NUMERIC hours");

        assert_eq!(
            resolved
                .resolution
                .as_ref()
                .and_then(|resolution| resolution.ot_hours.as_deref()),
            Some("1.50"),
        );
        let persisted: String = sqlx::query_scalar(
            "SELECT ot_hours::text FROM attendance_exception_resolutions WHERE exception_id=$1",
        )
        .bind(exception_id)
        .fetch_one(&owner_pool)
        .await
        .unwrap();
        assert_eq!(persisted, "1.50");
    })
    .await;
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn legacy_assignment_rechecks_after_leave_commit(owner_pool: PgPool) {
    scope_org(OrgId::knl(), async move {
        let branch = seed_branch(&owner_pool, "attendance-legacy-race", "operations").await;
        let provisioner = seed_user(&owner_pool, "Employee Directory Provisioner", "SUPER_ADMIN", branch).await;
        let actor = seed_user(&owner_pool, "Attendance Manager", "ADMIN", branch).await;
        let worker = seed_employee(&owner_pool, branch, provisioner, "Legacy worker").await;
        let day = OffsetDateTime::now_utc().date() + Duration::days(12);
        let writer_pool = one_connection_pool(
            &owner_pool,
            "attendance-legacy-leave-writer",
            PoolRole::LeaveDefiner,
        )
        .await;
        let reader = one_connection_pool(
            &owner_pool,
            "attendance-legacy-insert-reader",
            PoolRole::Runtime,
        )
        .await;
        let reader_session = reader_session(&reader).await;
        let mut leave = writer_pool.begin().await.unwrap();
        let writer_session = writer_session(&mut leave).await;
        sqlx::query("SELECT set_config('app.current_org', $1, false)")
            .bind(OrgId::knl().as_uuid().to_string())
            .execute(&mut *leave)
            .await
            .unwrap();
        sqlx::query("SELECT public.console_employee_day_eligibility_lock($1,$2,$3)")
            .bind(*OrgId::knl().as_uuid())
            .bind(worker)
            .bind(day)
            .execute(&mut *leave)
            .await
            .unwrap();
        sqlx::query("INSERT INTO leave_requests (org_id,branch_id,requester_user_id,subject_employee_id,leave_type,days,start_date,end_date,status,decided_by,decided_at,charge_state,charge_review_reasons,charge_units) VALUES ($1,$2,$3,$4,'annual',1,$5,$5,'approved',$6,statement_timestamp(),'legacy_unverified',ARRAY[]::TEXT[],1)")
            .bind(*OrgId::knl().as_uuid())
            .bind(*branch.as_uuid())
            .bind(*provisioner.as_uuid())
            .bind(worker)
            .bind(day)
            .bind(*actor.as_uuid())
            .execute(&mut *leave)
            .await
            .unwrap();
        let legacy = tokio::spawn(async move { sqlx::query("INSERT INTO attendance_substitutions (org_id,site,branch_id,role,cover_date,from_minutes,to_minutes,covered_employee_id,reason_kind,worker_employee_id,worker_name,worker_type,created_by,idempotency_key,request_fingerprint) VALUES ($1,'site',$2,'role',$3,480,960,$4,'OTHER',$4,'legacy','REGULAR',$5,'legacy-recheck',$6)").bind(*OrgId::knl().as_uuid()).bind(*branch.as_uuid()).bind(day).bind(worker).bind(*actor.as_uuid()).bind("a".repeat(64)).execute(&reader).await });
        wait_for_advisory_lock_waiter(&owner_pool, &reader_session, &writer_session).await;
        leave.commit().await.unwrap();
        let error = legacy.await.unwrap().expect_err("legacy trigger rechecks committed leave");
        let database = error.as_database_error().expect("legacy insert must fail with a database guard");
        assert_eq!(database.code().as_deref(), Some("23514"));
        assert_eq!(database.message(), "attendance_substitutions_worker_eligibility_guard");
    }).await;
}

struct DatabaseSession {
    backend_pid: i32,
    application_name: String,
}

#[derive(Clone, Copy)]
enum PoolRole {
    Owner,
    Runtime,
    LeaveDefiner,
}

async fn one_connection_pool(
    owner_pool: &PgPool,
    application_name: &str,
    role: PoolRole,
) -> PgPool {
    let application_name = application_name.to_owned();
    PgPoolOptions::new()
        .max_connections(1)
        .after_connect(move |connection, _| {
            let application_name = application_name.clone();
            Box::pin(async move {
                sqlx::query("SELECT set_config('application_name', $1, false)")
                    .bind(application_name)
                    .execute(&mut *connection)
                    .await?;
                match role {
                    PoolRole::Owner => {}
                    PoolRole::Runtime => {
                        sqlx::query("SET ROLE console_rt")
                            .execute(&mut *connection)
                            .await?;
                    }
                    PoolRole::LeaveDefiner => {
                        sqlx::query("SET ROLE console_leave_definer")
                            .execute(&mut *connection)
                            .await?;
                    }
                }
                Ok(())
            })
        })
        .connect_with(owner_pool.connect_options().as_ref().clone())
        .await
        .unwrap()
}

async fn reader_session(reader_pool: &PgPool) -> DatabaseSession {
    sqlx::query("SELECT set_config('app.current_org', $1, false)")
        .bind(OrgId::knl().as_uuid().to_string())
        .execute(reader_pool)
        .await
        .unwrap();
    let (backend_pid, application_name): (i32, String) =
        sqlx::query_as("SELECT pg_backend_pid(), current_setting('application_name')")
            .fetch_one(reader_pool)
            .await
            .unwrap();
    DatabaseSession {
        backend_pid,
        application_name,
    }
}

async fn writer_session(writer: &mut sqlx::Transaction<'_, sqlx::Postgres>) -> DatabaseSession {
    let (backend_pid, application_name): (i32, String) =
        sqlx::query_as("SELECT pg_backend_pid(), current_setting('application_name')")
            .fetch_one(writer.as_mut())
            .await
            .unwrap();
    DatabaseSession {
        backend_pid,
        application_name,
    }
}

async fn wait_for_advisory_lock_waiter(
    observer_pool: &PgPool,
    reader: &DatabaseSession,
    writer: &DatabaseSession,
) {
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let waiting: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM pg_stat_activity reader_activity JOIN pg_stat_activity writer_activity ON writer_activity.pid=$3 AND writer_activity.application_name=$4 JOIN pg_locks reader_lock ON reader_lock.pid=reader_activity.pid AND reader_lock.locktype='advisory' AND NOT reader_lock.granted JOIN pg_locks writer_lock ON writer_lock.pid=writer_activity.pid AND writer_lock.locktype='advisory' AND writer_lock.granted AND writer_lock.database IS NOT DISTINCT FROM reader_lock.database AND writer_lock.classid=reader_lock.classid AND writer_lock.objid=reader_lock.objid AND writer_lock.objsubid=reader_lock.objsubid WHERE reader_activity.pid=$1 AND reader_activity.application_name=$2 AND reader_activity.wait_event_type='Lock' AND reader_activity.wait_event='advisory' AND $3 = ANY(pg_blocking_pids(reader_activity.pid)))",
        )
        .bind(reader.backend_pid)
        .bind(&reader.application_name)
        .bind(writer.backend_pid)
        .bind(&writer.application_name)
        .fetch_one(observer_pool)
        .await
        .unwrap();
        if waiting {
            return;
        }
        if tokio::time::Instant::now() >= deadline {
            let observed_blockers: Vec<i32> = sqlx::query_scalar("SELECT pg_blocking_pids($1)")
                .bind(reader.backend_pid)
                .fetch_one(observer_pool)
                .await
                .unwrap();
            panic!(
                "reader backend {} ({}) never waited on an advisory lock held by writer backend {} ({}); observed blocking PIDs: {:?}",
                reader.backend_pid,
                reader.application_name,
                writer.backend_pid,
                writer.application_name,
                observed_blockers
            );
        }
        tokio::time::sleep(LOCK_WITNESS_POLL_INTERVAL).await;
    }
}

fn assignment_command(
    window: SubstitutionWindow,
    branch_id: Uuid,
    covered_employee_id: Uuid,
    worker_employee_id: Uuid,
    idempotency_key: &str,
) -> AssignSubstitute {
    AssignSubstitute {
        window,
        branch_id: Some(branch_id),
        site: "Seoul depot".to_owned(),
        role: "Forklift operator".to_owned(),
        covered_employee_id,
        reason_kind: "APPROVED_LEAVE".to_owned(),
        reason_detail: None,
        worker_employee_id: Some(worker_employee_id),
        worker_name: "untrusted client name".to_owned(),
        worker_type: "CONTRACTOR".to_owned(),
        worker_rate: Some("untrusted client rate".to_owned()),
        exception_id: None,
        idempotency_key: idempotency_key.to_owned(),
    }
}

fn legacy_v1_fingerprint(
    caller: &CallerScope,
    command: &AssignSubstitute,
    worker_employee_id: Option<Uuid>,
    worker_name: &str,
    worker_type: &str,
    worker_rate: Option<&str>,
) -> String {
    let value = json!({"v":1,"orgId":caller.org_id,"branchPresent":command.branch_id.is_some(),"branchId":command.branch_id,"coverDate":command.window.cover_date,"from":command.window.from_minutes,"to":command.window.to_minutes,"coveredEmployeeId":command.covered_employee_id,"reasonKind":command.reason_kind,"reasonDetailPresent":command.reason_detail.is_some(),"reasonDetail":command.reason_detail,"site":command.site,"role":command.role,"workerEmployeePresent":worker_employee_id.is_some(),"workerEmployeeId":worker_employee_id,"workerName":worker_name,"workerType":worker_type,"workerRatePresent":worker_rate.is_some(),"workerRate":worker_rate,"exceptionPresent":command.exception_id.is_some(),"exceptionId":command.exception_id});
    let mut hasher = Sha256::new();
    hasher.update(serde_json::to_vec(&value).unwrap());
    hex::encode(hasher.finalize())
}

async fn seed_employee(
    pool: &PgPool,
    branch: console_kernel_core::BranchId,
    actor: console_kernel_core::UserId,
    name: &str,
) -> Uuid {
    let employee = Uuid::new_v4();
    let employee_number = format!("ATT-{employee}");
    let idempotency_key = format!("attendance-cancel-{employee}");
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
    .bind(employee_number)
    .bind(name)
    .bind("attendance-test")
    .bind("REGULAR")
    .bind("+821012345678")
    .bind("Operations")
    .bind("Forklift operator")
    .bind("Seoul depot")
    .bind(*branch.as_uuid())
    .bind(idempotency_key)
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

async fn seed_assigned_substitution(
    pool: &PgPool,
    branch: console_kernel_core::BranchId,
    actor: console_kernel_core::UserId,
    covered_employee_id: Uuid,
) -> Uuid {
    let substitution_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO attendance_substitutions (id, org_id, site, branch_id, role, cover_date, from_minutes, to_minutes, covered_employee_id, reason_kind, worker_name, worker_type, status, idempotency_key, request_fingerprint, created_by) VALUES ($1, $2, 'Seoul depot', $3, 'Forklift operator', CURRENT_DATE, 480, 960, $4, 'APPROVED_LEAVE', 'Contractor Kim', 'CONTRACTOR', 'ASSIGNED', $5, $6, $7)",
    )
    .bind(substitution_id)
    .bind(*OrgId::knl().as_uuid())
    .bind(*branch.as_uuid())
    .bind(covered_employee_id)
    .bind(format!("attendance-cancel-{substitution_id}"))
    .bind("a".repeat(64))
    .bind(*actor.as_uuid())
    .execute(pool)
    .await
    .unwrap();
    substitution_id
}
