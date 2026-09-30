#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! STORY-EVALUATION-001 end-to-end contract for the authenticated evaluation
//! console (CAP-EVALUATION-CONSOLE).
//!
//! The first test exercises `build_router` so route composition and JWT
//! authentication cannot regress. The workflow story below mounts the same
//! evaluation router against an `console_rt` pool to exercise FORCE RLS
//! (NOSUPERUSER, NOBYPASSRLS), never the BYPASSRLS superuser the default
//! `#[sqlx::test]` pool connects as. Seed helpers live here rather than in the
//! rest crate so the audit-coverage gate does not misread them as unaudited
//! mutations.

use axum::body::{Body, to_bytes};
use console_app::{AppConfig, AppRole, AppState, DatabaseDependency, build_router};
use console_evaluation_adapter_postgres::PgEvaluationStore;
use console_evaluation_rest::EvaluationRestState;
use console_kernel_core::{BranchId, OrgId, UserId};
use console_platform_auth::{AccessTokenInput, JwtIssuer, JwtSettings, JwtVerifier};
use http::{Request, StatusCode, header};
use p256::ecdsa::SigningKey;
use p256::elliptic_curve::rand_core::OsRng;
use p256::pkcs8::{EncodePrivateKey, EncodePublicKey, LineEnding};
use serde_json::{Value, json};
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use time::{Duration, OffsetDateTime};
use tower::ServiceExt;
use uuid::Uuid;

const ISSUER: &str = "console-platform-auth";
const AUDIENCE: &str = "console-api";
const CYCLES: &str = "/api/v1/evaluation/cycles";
const SUBJECTS: &str = "/api/v1/evaluation/subjects";
const ORG_B: Uuid = Uuid::from_u128(0xeb00_0000_0000_0000_0000_0000_0000_00b2);

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn evaluation_routes_are_mounted_by_the_authenticated_app_router(pool: PgPool) {
    let fixture = Fixture::new(&pool).await;
    let router = build_router(
        app_state(runtime_role_pool(&pool).await, fixture.public_pem.clone()).unwrap(),
    );

    let (status, body) = send(&router, "GET", CYCLES, None, None).await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "app router must authenticate evaluation routes: {body}"
    );
    assert_eq!(body, json!("missing or malformed bearer token"));

    let (status, page) = send(&router, "GET", CYCLES, Some(&fixture.admin), None).await;
    assert_eq!(status, StatusCode::OK, "authorized evaluation read: {page}");
    assert_eq!(page["items"], json!([]));
    assert_eq!(page["total"], 0);
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn story_evaluation_001_walks_cycle_to_ledger_as_runtime_role(pool: PgPool) {
    let f = Fixture::new(&pool).await;
    let router = f.router(&pool).await;

    // Cycle opens as DRAFT with typed fields.
    let (status, cycle) = send(
        &router,
        "POST",
        CYCLES,
        Some(&f.admin),
        Some(json!({
            "name": "2026 하반기 정기평가",
            "kind": "REGULAR",
            "period_label": "2026-H2",
            "due_date": "2026-08-31"
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "create cycle: {cycle}");
    assert_eq!(cycle["stage"], "DRAFT");
    assert_eq!(cycle["created_by"], f.admin_id.to_string());
    let cycle_id = cycle["id"].as_str().unwrap().to_owned();

    // Enroll subjects; duplicate enrollment conflicts.
    let s1_body =
        json!({"cycle_id": cycle_id, "employee_id": f.employee_a, "manager_user_id": f.manager_id});
    let (status, s1) = send(
        &router,
        "POST",
        SUBJECTS,
        Some(&f.admin),
        Some(s1_body.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "enroll s1: {s1}");
    assert_eq!(s1["state"], "ENROLLED");
    let s1 = s1["id"].as_str().unwrap().to_owned();
    let (status, s2) = send(
        &router,
        "POST",
        SUBJECTS,
        Some(&f.admin),
        Some(json!({"cycle_id": cycle_id, "employee_id": f.employee_b, "manager_user_id": f.admin_id})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "enroll s2: {s2}");
    let s2 = s2["id"].as_str().unwrap().to_owned();
    let (status, dup) = send(&router, "POST", SUBJECTS, Some(&f.admin), Some(s1_body)).await;
    assert_eq!(status, StatusCode::CONFLICT, "duplicate subject: {dup}");

    // Preflight fails closed: subjects without goals block `open`.
    let (status, report) = send(
        &router,
        "GET",
        &format!("{CYCLES}/{cycle_id}/preflight"),
        Some(&f.admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(report["next_transition"], "open");
    assert_eq!(report["blockers"].as_array().unwrap().len(), 2, "{report}");
    let (status, blocked) = send(
        &router,
        "POST",
        &format!("{CYCLES}/{cycle_id}/open"),
        Some(&f.admin),
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "open past blockers: {blocked}"
    );

    // Typed goals; the assigned manager may edit their own subject's goals.
    let goals = json!({"goals": [
        {"title": "불량률 개선", "metric_kind": "KPI", "target_label": "0.5% 이하", "weight_pct": 60},
        {"title": "정시 출근", "metric_kind": "ATTENDANCE", "target_label": "지각 0회", "weight_pct": 40}
    ]});
    let (status, body) = send(
        &router,
        "PUT",
        &format!("{SUBJECTS}/{s1}/goals"),
        Some(&f.manager),
        Some(goals.clone()),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "manager sets own-subject goals: {body}"
    );
    assert_eq!(body["goals"].as_array().unwrap().len(), 2);
    let (status, invalid) = send(
        &router,
        "PUT",
        &format!("{SUBJECTS}/{s2}/goals"),
        Some(&f.admin),
        Some(json!({"goals": [{"title": "x", "metric_kind": "KPI", "target_label": "y", "weight_pct": 150}]})),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "weight bound: {invalid}"
    );
    let (status, _) = send(
        &router,
        "PUT",
        &format!("{SUBJECTS}/{s2}/goals"),
        Some(&f.admin),
        Some(goals),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // DRAFT → OPEN; the FSM rejects a replayed transition.
    let (status, opened) = send(
        &router,
        "POST",
        &format!("{CYCLES}/{cycle_id}/open"),
        Some(&f.admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "open: {opened}");
    assert_eq!(opened["stage"], "OPEN");
    assert!(!opened["opened_at"].is_null());
    let (status, replay) = send(
        &router,
        "POST",
        &format!("{CYCLES}/{cycle_id}/open"),
        Some(&f.admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "re-open: {replay}");

    // Review work is relationship-scoped by kind: the assigned manager sees
    // only MANAGER work and the linked employee sees only SELF work.
    let (status, tasks) = send(
        &router,
        "GET",
        "/api/v1/evaluation/my-tasks",
        Some(&f.manager),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(tasks["items"].as_array().unwrap().len(), 1, "{tasks}");
    assert_eq!(tasks["items"][0]["kind"], "MANAGER", "{tasks}");
    let (status, tasks) = send(
        &router,
        "GET",
        "/api/v1/evaluation/my-tasks",
        Some(&f.subject),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(tasks["items"].as_array().unwrap().len(), 1, "{tasks}");
    assert_eq!(tasks["items"][0]["kind"], "SELF", "{tasks}");

    // Server-persisted draft: a refresh (fresh GET) still sees it.
    let review_path = format!("{SUBJECTS}/{s1}/reviews/manager");
    let (status, draft) = send(
        &router,
        "PUT",
        &review_path,
        Some(&f.manager),
        Some(json!({"note": "초안 메모", "evidence_links": []})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "draft save: {draft}");
    assert_eq!(draft["status"], "DRAFT");
    let (status, subject) = send(
        &router,
        "GET",
        &format!("{SUBJECTS}/{s1}"),
        Some(&f.manager),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        subject["reviews"][0]["note"], "초안 메모",
        "draft survives refresh: {subject}"
    );

    // Submit gates: grade required; a MANAGER review needs ≥1 evidence link.
    let submit_path = format!("{review_path}/submit");
    let (status, no_grade) = send(&router, "POST", &submit_path, Some(&f.manager), None).await;
    assert_eq!(status, StatusCode::CONFLICT, "grade required: {no_grade}");
    let (status, _) = send(
        &router,
        "PUT",
        &review_path,
        Some(&f.manager),
        Some(json!({"grade": "A", "note": "근거 없는 초안", "evidence_links": []})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, no_evidence) = send(&router, "POST", &submit_path, Some(&f.manager), None).await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "evidence required: {no_evidence}"
    );
    let (status, _) = send(
        &router,
        "PUT",
        &review_path,
        Some(&f.manager),
        Some(
            json!({"grade": "A", "note": "근거 첨부", "evidence_links": [
                {"object_kind": "ATTENDANCE", "object_ref": "AT-1024", "label": "근태 요약"},
                {"object_kind": "WORK_ORDER", "object_ref": "WO-2210", "label": "최근 작업"}
            ]}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, submitted) = send(&router, "POST", &submit_path, Some(&f.manager), None).await;
    assert_eq!(status, StatusCode::OK, "submit manager review: {submitted}");
    assert_eq!(submitted["status"], "SUBMITTED");
    assert!(!submitted["submitted_at"].is_null());
    let (status, resubmit) = send(&router, "POST", &submit_path, Some(&f.manager), None).await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "submitted is immutable: {resubmit}"
    );
    let (status, mutate) = send(
        &router,
        "PUT",
        &review_path,
        Some(&f.manager),
        Some(json!({"grade": "S", "evidence_links": []})),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "draft upsert after submit: {mutate}"
    );

    // The subject, not their manager, owns the SELF review.
    let self_path = format!("{SUBJECTS}/{s1}/reviews/self");
    let (status, _) = send(
        &router,
        "PUT",
        &self_path,
        Some(&f.subject),
        Some(json!({"grade": "B", "evidence_links": []})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = send(
        &router,
        "POST",
        &format!("{self_path}/submit"),
        Some(&f.subject),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, tasks) = send(
        &router,
        "GET",
        "/api/v1/evaluation/my-tasks",
        Some(&f.manager),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        tasks["items"].as_array().unwrap().len(),
        0,
        "submitted tasks disappear: {tasks}"
    );

    // start-calibration blocks while s2's MANAGER review is missing; the
    // missing SELF review is only an advisory.
    let (status, blocked) = send(
        &router,
        "POST",
        &format!("{CYCLES}/{cycle_id}/start-calibration"),
        Some(&f.admin),
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "calibration past blocker: {blocked}"
    );
    let (status, report) = send(
        &router,
        "GET",
        &format!("{CYCLES}/{cycle_id}/preflight"),
        Some(&f.admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(report["next_transition"], "start_calibration");
    assert_eq!(report["blockers"].as_array().unwrap().len(), 1, "{report}");
    assert!(
        !report["advisories"].as_array().unwrap().is_empty(),
        "{report}"
    );

    let s2_review = format!("{SUBJECTS}/{s2}/reviews/manager");
    let (status, _) = send(
        &router,
        "PUT",
        &s2_review,
        Some(&f.admin),
        Some(json!({"grade": "A", "evidence_links": [
            {"object_kind": "KPI", "object_ref": "KPI-88", "label": "분기 KPI"}
        ]})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = send(
        &router,
        "POST",
        &format!("{s2_review}/submit"),
        Some(&f.admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, calibrating) = send(
        &router,
        "POST",
        &format!("{CYCLES}/{cycle_id}/start-calibration"),
        Some(&f.admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "start calibration: {calibrating}");
    assert_eq!(calibrating["stage"], "CALIBRATION");
    let (status, late_draft) = send(
        &router,
        "PUT",
        &self_path,
        Some(&f.subject),
        Some(json!({"grade": "A", "evidence_links": []})),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "drafts lock at calibration: {late_draft}"
    );

    // Calibration: reason required on a grade change; four-eyes SoD enforced.
    let calibrate_s1 = format!("{SUBJECTS}/{s1}/calibrate");
    let (status, no_reason) = send(
        &router,
        "POST",
        &calibrate_s1,
        Some(&f.admin),
        Some(json!({"final_grade": "B"})),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "reason required on change: {no_reason}"
    );
    let (status, calibrated) = send(
        &router,
        "POST",
        &calibrate_s1,
        Some(&f.admin),
        Some(json!({"final_grade": "B", "reason": "상대평가 조정"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "calibrate s1: {calibrated}");
    assert_eq!(calibrated["calibrated_grade"], "B");
    assert_eq!(calibrated["state"], "CALIBRATED");

    let (status, premature) = send(
        &router,
        "POST",
        &format!("{CYCLES}/{cycle_id}/finalize"),
        Some(&f.admin),
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "finalize past uncalibrated: {premature}"
    );

    let calibrate_s2 = format!("{SUBJECTS}/{s2}/calibrate");
    let (status, sod) = send(
        &router,
        "POST",
        &calibrate_s2,
        Some(&f.admin),
        Some(json!({"final_grade": "A"})),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "four-eyes: evaluator cannot calibrate: {sod}"
    );
    let (status, _) = send(
        &router,
        "POST",
        &calibrate_s2,
        Some(&f.admin2),
        Some(json!({"final_grade": "A"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "second admin calibrates");

    // Finalize issues RV- codes from the per-org counter and stamps grades.
    let (status, finalized) = send(
        &router,
        "POST",
        &format!("{CYCLES}/{cycle_id}/finalize"),
        Some(&f.admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "finalize: {finalized}");
    assert_eq!(finalized["stage"], "FINALIZED");
    let subjects = finalized["subjects"].as_array().unwrap();
    let by_id = |id: &str| subjects.iter().find(|s| s["id"] == id).unwrap();
    assert_eq!(by_id(&s1)["rv_code"], "RV-2500");
    assert_eq!(by_id(&s1)["final_grade"], "B");
    assert_eq!(by_id(&s1)["state"], "FINALIZED");
    assert_eq!(by_id(&s2)["rv_code"], "RV-2501");
    assert_eq!(by_id(&s2)["final_grade"], "A");
    let counter: i32 =
        sqlx::query_scalar("SELECT next_value FROM evaluation_code_counters WHERE org_id = $1")
            .bind(*OrgId::knl().as_uuid())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(counter, 2502, "counter advanced once per issued RV- code");

    // Team-progress aggregates group by employee org_unit.
    let units = finalized["progress_by_unit"].as_array().unwrap();
    assert_eq!(units.len(), 2, "{finalized}");
    assert!(
        units
            .iter()
            .all(|u| u["total"] == 1 && u["manager_submitted"] == 1)
    );

    // Person ledger read returns finalized entries and is itself audited.
    let (status, ledger) = send(
        &router,
        "GET",
        &format!("/api/v1/evaluation/employees/{}/reviews", f.employee_a),
        Some(&f.admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "ledger: {ledger}");
    let entries = ledger["items"].as_array().unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["rv_code"], "RV-2500");
    assert_eq!(entries[0]["final_grade"], "B");
    assert_eq!(entries[0]["cycle_id"], cycle_id);
    let viewed: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_events WHERE action = 'evaluation.history.viewed' AND target_type = 'employee' AND target_id = $1",
    )
    .bind(f.employee_a.to_string())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(viewed, 1, "the person-ledger read must be audited");

    // FINALIZED → ARCHIVED; archived cycles leave the default list.
    let (status, archived) = send(
        &router,
        "POST",
        &format!("{CYCLES}/{cycle_id}/archive"),
        Some(&f.admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "archive: {archived}");
    assert_eq!(archived["stage"], "ARCHIVED");
    let (status, active) = send(&router, "GET", CYCLES, Some(&f.admin), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        active["total"], 0,
        "default list excludes ARCHIVED: {active}"
    );
    let (status, all_archived) = send(
        &router,
        "GET",
        &format!("{CYCLES}?stage=ARCHIVED"),
        Some(&f.admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(all_archived["total"], 1);

    // Audit readback: every lifecycle edge and per-subject finalization landed.
    let cycle_events: Vec<String> = sqlx::query_scalar(
        "SELECT action FROM audit_events WHERE target_type = 'evaluation_cycle' AND target_id = $1 ORDER BY occurred_at, action",
    )
    .bind(&cycle_id)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(
        cycle_events,
        [
            "evaluation.cycle.created",
            "evaluation.cycle.opened",
            "evaluation.calibration.started",
            "evaluation.cycle.finalized",
            "evaluation.cycle.archived"
        ],
        "cycle lifecycle audit chain"
    );
    let finalized_subjects: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_events WHERE action = 'evaluation.subject.finalized'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        finalized_subjects, 2,
        "one finalization audit row per subject"
    );
    let calibrations: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_events WHERE action = 'evaluation.subject.calibrated'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(calibrations, 2, "only successful calibrations are audited");
    let submissions: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_events WHERE action = 'evaluation.review.submitted'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(submissions, 3);
    // Remaining mutation classes: rejected requests (409/422) write nothing.
    for (action, expected) in [
        ("evaluation.subject.added", 2_i64),
        ("evaluation.goals.replaced", 2),
        ("evaluation.review.saved", 5),
    ] {
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM audit_events WHERE action = $1")
            .bind(action)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, expected, "{action}");
    }
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn authorization_conceals_and_isolates_without_leakage(pool: PgPool) {
    let f = Fixture::new(&pool).await;
    let router = f.router(&pool).await;

    let (status, cycle) = send(
        &router,
        "POST",
        CYCLES,
        Some(&f.admin),
        Some(json!({"name": "격리 검증", "kind": "PROBATION", "period_label": "2026-Q3", "due_date": "2026-09-30"})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{cycle}");
    let cycle_id = cycle["id"].as_str().unwrap().to_owned();
    let (_, s1) = send(
        &router,
        "POST",
        SUBJECTS,
        Some(&f.admin),
        Some(json!({"cycle_id": cycle_id, "employee_id": f.employee_a, "manager_user_id": f.manager_id})),
    )
    .await;
    let s1 = s1["id"].as_str().unwrap().to_owned();
    let (_, s2) = send(
        &router,
        "POST",
        SUBJECTS,
        Some(&f.admin),
        Some(json!({"cycle_id": cycle_id, "employee_id": f.employee_b, "manager_user_id": f.admin_id})),
    )
    .await;
    let s2 = s2["id"].as_str().unwrap().to_owned();

    // 401 without a bearer token.
    let (status, _) = send(&router, "GET", CYCLES, None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // An ungranted MEMBER holds no evaluation feature: everything is 403.
    for (method, uri) in [
        ("GET", CYCLES.to_owned()),
        ("GET", "/api/v1/evaluation/my-tasks".to_owned()),
        ("GET", format!("{SUBJECTS}/{s1}")),
    ] {
        let (status, denied) = send(&router, method, &uri, Some(&f.outsider), None).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{method} {uri}: {denied}");
        assert!(
            denied["error"]["message"].is_string(),
            "canonical envelope: {denied}"
        );
    }
    let (status, denied) = send(
        &router,
        "POST",
        CYCLES,
        Some(&f.outsider),
        Some(
            json!({"name": "x", "kind": "REGULAR", "period_label": "y", "due_date": "2026-09-30"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{denied}");

    // A Submit-only manager sees exactly their own subject; other subjects
    // answer 404 (deny-by-omission), and cycle-level reads stay 403.
    let (status, own) = send(
        &router,
        "GET",
        &format!("{SUBJECTS}/{s1}"),
        Some(&f.manager),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{own}");
    let (status, concealed) = send(
        &router,
        "GET",
        &format!("{SUBJECTS}/{s2}"),
        Some(&f.manager),
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "deny-by-omission: {concealed}"
    );
    let (status, concealed) = send(
        &router,
        "PUT",
        &format!("{SUBJECTS}/{s2}/reviews/manager"),
        Some(&f.manager),
        Some(json!({"grade": "A", "evidence_links": []})),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "review write concealed: {concealed}"
    );
    let (status, _) = send(&router, "GET", CYCLES, Some(&f.manager), None).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "submit does not grant read");
    let (status, denied) = send(
        &router,
        "POST",
        &format!("{SUBJECTS}/{s1}/calibrate"),
        Some(&f.manager),
        Some(json!({"final_grade": "A"})),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "calibrate needs manage: {denied}"
    );

    // An unknown review kind is a 404, not a 500 or an enum leak.
    let (status, _) = send(
        &router,
        "PUT",
        &format!("{SUBJECTS}/{s1}/reviews/peer"),
        Some(&f.manager),
        Some(json!({"evidence_links": []})),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Handler-validated bounds answer 422 before any DB CHECK can fire.
    let (status, invalid) = send(
        &router,
        "POST",
        CYCLES,
        Some(&f.admin),
        Some(json!({"name": "긴".repeat(121), "kind": "REGULAR", "period_label": "2026", "due_date": "2026-09-30"})),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{invalid}");

    // Cross-tenant: an ADMIN of another org sees nothing — not even a 403.
    let (status, foreign_list) = send(&router, "GET", CYCLES, Some(&f.foreign_admin), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        foreign_list["total"], 0,
        "RLS conceals the other tenant: {foreign_list}"
    );
    let (status, _) = send(
        &router,
        "GET",
        &format!("{CYCLES}/{cycle_id}"),
        Some(&f.foreign_admin),
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "cross-org cycle is invisible"
    );
    let (status, _) = send(
        &router,
        "GET",
        &format!("{SUBJECTS}/{s1}"),
        Some(&f.foreign_admin),
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "cross-org subject is invisible"
    );
    let (status, _) = send(
        &router,
        "GET",
        &format!("/api/v1/evaluation/employees/{}/reviews", f.employee_a),
        Some(&f.foreign_admin),
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "cross-org ledger is invisible"
    );

    // Fail-closed RLS: with no armed app.current_org GUC the runtime role
    // reads zero rows even though the tenant's data exists.
    let owner_visible: i64 = sqlx::query_scalar("SELECT count(*) FROM evaluation_cycles")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(owner_visible, 1);
    let rt = runtime_role_pool(&pool).await;
    let unarmed: i64 = sqlx::query_scalar("SELECT count(*) FROM evaluation_cycles")
        .fetch_one(&rt)
        .await
        .unwrap();
    assert_eq!(unarmed, 0, "unarmed GUC must read nothing under FORCE RLS");
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn review_identity_relationships_fail_closed_by_kind(pool: PgPool) {
    let f = Fixture::new(&pool).await;
    let router = f.router(&pool).await;

    let (_, cycle) = send(
        &router,
        "POST",
        CYCLES,
        Some(&f.admin),
        Some(json!({"name": "관계 검증", "kind": "REGULAR", "period_label": "2026-Q4", "due_date": "2026-12-31"})),
    )
    .await;
    let cycle_id = cycle["id"].as_str().unwrap();
    let (_, subject) = send(
        &router,
        "POST",
        SUBJECTS,
        Some(&f.admin),
        Some(json!({"cycle_id": cycle_id, "employee_id": f.employee_a, "manager_user_id": f.manager_id})),
    )
    .await;
    let subject_id = subject["id"].as_str().unwrap();
    let (_, _) = send(
        &router,
        "PUT",
        &format!("{SUBJECTS}/{subject_id}/goals"),
        Some(&f.manager),
        Some(json!({"goals": [{"title": "관계 검증", "metric_kind": "KPI", "target_label": "완료", "weight_pct": 100}]})),
    )
    .await;
    let (status, _) = send(
        &router,
        "POST",
        &format!("{CYCLES}/{cycle_id}/open"),
        Some(&f.admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let self_path = format!("{SUBJECTS}/{subject_id}/reviews/self");
    let manager_path = format!("{SUBJECTS}/{subject_id}/reviews/manager");
    let draft = json!({"grade": "A", "evidence_links": []});

    let (status, detail) = send(
        &router,
        "GET",
        &format!("{SUBJECTS}/{subject_id}"),
        Some(&f.subject),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "linked subject detail: {detail}");
    let (status, _) = send(
        &router,
        "GET",
        &format!("{SUBJECTS}/{subject_id}"),
        Some(&f.unlinked),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // The assigned manager is linked, but cannot impersonate the subject.
    let (status, _) = send(
        &router,
        "PUT",
        &self_path,
        Some(&f.manager),
        Some(draft.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    // The linked subject cannot impersonate their manager.
    let (status, _) = send(
        &router,
        "PUT",
        &manager_path,
        Some(&f.subject),
        Some(draft.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    // A capability holder without users.employee_id fails closed for either kind.
    let (status, _) = send(&router, "PUT", &self_path, Some(&f.unlinked), Some(draft)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, saved) = send(
        &router,
        "PUT",
        &self_path,
        Some(&f.subject),
        Some(json!({"grade": "A", "evidence_links": []})),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "linked subject self review: {saved}"
    );
    assert_eq!(saved["kind"], "SELF");
    let (status, saved) = send(
        &router,
        "PUT",
        &manager_path,
        Some(&f.manager),
        Some(json!({"grade": "A", "evidence_links": [{"object_kind": "KPI", "object_ref": "KPI-identity", "label": "identity proof"}]})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "linked manager review: {saved}");
    assert_eq!(saved["kind"], "MANAGER");
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn identity_relinks_serialize_submit_detail_and_review_authorship(pool: PgPool) {
    let f = Fixture::new(&pool).await;
    let router = f.router(&pool).await;

    let (_, cycle) = send(
        &router,
        "POST",
        CYCLES,
        Some(&f.admin),
        Some(json!({"name": "신원 잠금", "kind": "REGULAR", "period_label": "2026-Q4", "due_date": "2026-12-31"})),
    )
    .await;
    let cycle_id = cycle["id"].as_str().unwrap();
    let (_, subject) = send(
        &router,
        "POST",
        SUBJECTS,
        Some(&f.admin),
        Some(json!({"cycle_id": cycle_id, "employee_id": f.employee_a, "manager_user_id": f.manager_id})),
    )
    .await;
    let subject_id = subject["id"].as_str().unwrap().to_owned();
    let (_, _) = send(
        &router,
        "PUT",
        &format!("{SUBJECTS}/{subject_id}/goals"),
        Some(&f.manager),
        Some(json!({"goals": [{"title": "신원 잠금", "metric_kind": "KPI", "target_label": "완료", "weight_pct": 100}]})),
    )
    .await;
    let (status, _) = send(
        &router,
        "POST",
        &format!("{CYCLES}/{cycle_id}/open"),
        Some(&f.admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // A relink transaction holds the same update-conflicting user lock used by
    // submit-only detail reads. The request cannot observe the pre-unlink
    // relationship and then load a body after the unlink commits.
    let mut relink = pool.begin().await.unwrap();
    sqlx::query("SELECT id FROM users WHERE id = $1 FOR NO KEY UPDATE")
        .bind(*f.subject_user_id.as_uuid())
        .fetch_one(&mut *relink)
        .await
        .unwrap();
    let relink_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *relink)
        .await
        .unwrap();
    let detail_router = router.clone();
    let detail_token = f.subject.clone();
    let detail_path = format!("{SUBJECTS}/{subject_id}");
    let detail = tokio::spawn(async move {
        send(
            &detail_router,
            "GET",
            &detail_path,
            Some(&detail_token),
            None,
        )
        .await
    });
    await_actor_identity_lock_wait(&pool, relink_pid).await;
    sqlx::query("UPDATE users SET employee_id = NULL WHERE id = $1")
        .bind(*f.subject_user_id.as_uuid())
        .execute(&mut *relink)
        .await
        .unwrap();
    relink.commit().await.unwrap();
    let (status, _) = detail.await.unwrap();
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Restore the canonical link, then prove the same serialization protects
    // the review write itself: a relink that wins the lock race leaves no
    // authored review behind.
    link_user_employee(&pool, OrgId::knl(), f.subject_user_id, f.employee_a).await;
    let mut unlink = pool.begin().await.unwrap();
    sqlx::query("SELECT id FROM users WHERE id = $1 FOR NO KEY UPDATE")
        .bind(*f.subject_user_id.as_uuid())
        .fetch_one(&mut *unlink)
        .await
        .unwrap();
    let unlink_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *unlink)
        .await
        .unwrap();
    let write_router = router.clone();
    let write_token = f.subject.clone();
    let write_path = format!("{SUBJECTS}/{subject_id}/reviews/self");
    let write = tokio::spawn(async move {
        send(
            &write_router,
            "PUT",
            &write_path,
            Some(&write_token),
            Some(json!({"grade": "A", "evidence_links": []})),
        )
        .await
    });
    await_actor_identity_lock_wait(&pool, unlink_pid).await;
    sqlx::query("UPDATE users SET employee_id = NULL WHERE id = $1")
        .bind(*f.subject_user_id.as_uuid())
        .execute(&mut *unlink)
        .await
        .unwrap();
    unlink.commit().await.unwrap();
    let (status, _) = write.await.unwrap();
    assert_eq!(status, StatusCode::NOT_FOUND);
    let authored: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM evaluation_reviews WHERE subject_id = $1 AND kind = 'SELF'",
    )
    .bind(Uuid::parse_str(&subject_id).unwrap())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(authored, 0, "unlinked actor must not author a review");

    // Task discovery is an authorization result too. It must take the same
    // update-conflicting canonical identity lock before deciding whether this
    // caller has SELF work, so an unlink that wins the race cannot leak a
    // stale task into the response.
    link_user_employee(&pool, OrgId::knl(), f.subject_user_id, f.employee_a).await;
    let mut task_unlink = pool.begin().await.unwrap();
    sqlx::query("SELECT id FROM users WHERE id = $1 FOR NO KEY UPDATE")
        .bind(*f.subject_user_id.as_uuid())
        .fetch_one(&mut *task_unlink)
        .await
        .unwrap();
    let task_unlink_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *task_unlink)
        .await
        .unwrap();
    let task_router = router.clone();
    let task_token = f.subject.clone();
    let tasks = tokio::spawn(async move {
        send(
            &task_router,
            "GET",
            "/api/v1/evaluation/my-tasks",
            Some(&task_token),
            None,
        )
        .await
    });
    await_actor_identity_lock_wait(&pool, task_unlink_pid).await;
    sqlx::query("UPDATE users SET employee_id = NULL WHERE id = $1")
        .bind(*f.subject_user_id.as_uuid())
        .execute(&mut *task_unlink)
        .await
        .unwrap();
    task_unlink.commit().await.unwrap();
    let (status, task_page) = tasks.await.unwrap();
    assert_eq!(status, StatusCode::OK);
    assert_eq!(task_page["items"], json!([]));
}

/// Synchronize on PostgreSQL's actual lock graph, not scheduler timing. The
/// spawned HTTP request must be waiting at the adapter's update-conflicting
/// canonical-identity lock and be blocked by this exact relink transaction
/// before the test commits the unlink.
async fn await_actor_identity_lock_wait(pool: &PgPool, relink_pid: i32) {
    for _ in 0..512 {
        let waiting: bool = sqlx::query_scalar(
            "SELECT EXISTS ( \
               SELECT 1 \
               FROM pg_stat_activity a \
               WHERE $1 = ANY(pg_blocking_pids(a.pid)) \
                 AND a.query LIKE '%SELECT employee_id FROM users WHERE id = $1 FOR NO KEY UPDATE%' \
             )",
        )
        .bind(relink_pid)
        .fetch_one(pool)
        .await
        .unwrap();
        if waiting {
            return;
        }
        tokio::task::yield_now().await;
    }
    panic!("request did not reach the canonical actor identity lock");
}

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

struct Fixture {
    admin: String,
    admin2: String,
    manager: String,
    subject: String,
    unlinked: String,
    outsider: String,
    foreign_admin: String,
    admin_id: UserId,
    manager_id: UserId,
    subject_user_id: UserId,
    employee_a: Uuid,
    employee_b: Uuid,
    public_pem: String,
}

impl Fixture {
    async fn new(pool: &PgPool) -> Self {
        let signing = SigningKey::random(&mut OsRng);
        let private_pem = signing.to_pkcs8_pem(LineEnding::LF).unwrap().to_string();
        let public_pem = signing
            .verifying_key()
            .to_public_key_pem(LineEnding::LF)
            .unwrap();
        let org = OrgId::knl();
        seed_org(pool, ORG_B, "evaluation-isolation").await;
        let branch = seed_branch(pool, org, "평가-본점").await;
        let foreign_branch = seed_branch(pool, OrgId::from_uuid(ORG_B), "격리-지점").await;

        let employee_a = seed_employee(pool, org, "김평가", "품질팀", 1).await;
        let employee_b = seed_employee(pool, org, "이성과", "생산팀", 2).await;
        let manager_employee = seed_employee(pool, org, "평가관리자", "품질팀", 3).await;

        let admin_id = UserId::new();
        let admin2_id = UserId::new();
        let manager_id = UserId::new();
        let subject_user_id = UserId::new();
        let unlinked_id = UserId::new();
        let outsider_id = UserId::new();
        let foreign_admin_id = UserId::new();
        seed_user(pool, org, admin_id, branch, "ADMIN").await;
        seed_user(pool, org, admin2_id, branch, "ADMIN").await;
        seed_user(pool, org, manager_id, branch, "MEMBER").await;
        seed_user(pool, org, subject_user_id, branch, "MEMBER").await;
        seed_user(pool, org, unlinked_id, branch, "MEMBER").await;
        seed_user(pool, org, outsider_id, branch, "MEMBER").await;
        seed_user(
            pool,
            OrgId::from_uuid(ORG_B),
            foreign_admin_id,
            foreign_branch,
            "ADMIN",
        )
        .await;
        grant_feature(pool, org, manager_id, "evaluation_submit").await;
        grant_feature(pool, org, subject_user_id, "evaluation_submit").await;
        grant_feature(pool, org, unlinked_id, "evaluation_submit").await;
        link_user_employee(pool, org, admin_id, employee_b).await;
        link_user_employee(pool, org, manager_id, manager_employee).await;
        link_user_employee(pool, org, subject_user_id, employee_a).await;

        let issuer = JwtIssuer::from_es256_pem(
            jwt_settings(),
            private_pem.as_bytes(),
            public_pem.as_bytes(),
        )
        .unwrap();
        let token = |user: UserId, org: OrgId, roles: Vec<&str>, branches: Vec<BranchId>| {
            issuer
                .issue_access_token(AccessTokenInput {
                    subject: user,
                    org_id: org,
                    roles: roles.into_iter().map(str::to_owned).collect(),
                    branches,
                    platform: false,
                    view_as: false,
                    read_only: false,
                    display_name: None,
                    feature_grants: Vec::new(),
                    authz_subject_version: 0,
                    authz_policy_version: 0,
                    session_generation: 0,
                    issued_at: OffsetDateTime::now_utc(),
                })
                .unwrap()
        };
        Self {
            admin: token(admin_id, org, vec!["ADMIN"], Vec::new()),
            admin2: token(admin2_id, org, vec!["ADMIN"], Vec::new()),
            manager: token(manager_id, org, vec!["MEMBER"], vec![branch]),
            subject: token(subject_user_id, org, vec!["MEMBER"], vec![branch]),
            unlinked: token(unlinked_id, org, vec!["MEMBER"], vec![branch]),
            outsider: token(outsider_id, org, vec!["MEMBER"], vec![branch]),
            foreign_admin: token(
                foreign_admin_id,
                OrgId::from_uuid(ORG_B),
                vec!["ADMIN"],
                Vec::new(),
            ),
            admin_id,
            manager_id,
            subject_user_id,
            employee_a,
            employee_b,
            public_pem,
        }
    }

    /// The evaluation router mounted exactly as the app will mount it, but
    /// backed by a pool whose every connection runs as `console_rt`.
    async fn router(&self, owner: &PgPool) -> axum::Router {
        let rt = runtime_role_pool(owner).await;
        let verifier =
            JwtVerifier::from_es256_public_pem(jwt_settings(), self.public_pem.as_bytes()).unwrap();
        console_evaluation_rest::router(EvaluationRestState::new(
            PgEvaluationStore::new(rt),
            Some(verifier),
        ))
    }
}

fn jwt_settings() -> JwtSettings {
    JwtSettings {
        issuer: ISSUER.to_owned(),
        audience: AUDIENCE.to_owned(),
        access_token_ttl: Duration::minutes(15),
    }
}

async fn runtime_role_pool(owner: &PgPool) -> PgPool {
    PgPoolOptions::new()
        .max_connections(8)
        .after_connect(|conn, _| {
            Box::pin(async move {
                sqlx::query("SET ROLE console_rt").execute(conn).await?;
                Ok(())
            })
        })
        .connect_with(owner.connect_options().as_ref().clone())
        .await
        .unwrap()
}

fn app_state(pool: PgPool, public_key_pem: String) -> Result<AppState, console_app::AppError> {
    let config = AppConfig::from_pairs([
        ("CONSOLE_APP_ROLE", AppRole::Api.to_string()),
        ("CONSOLE_HTTP_ADDR", "127.0.0.1:0".to_owned()),
        ("CONSOLE_JWT_ISSUER", ISSUER.to_owned()),
        ("CONSOLE_JWT_AUDIENCE", AUDIENCE.to_owned()),
        ("CONSOLE_JWT_PUBLIC_KEY_PEM", public_key_pem),
    ])?;
    AppState::new(config, DatabaseDependency::Postgres(pool))
}

async fn send(
    router: &axum::Router,
    method: &str,
    uri: &str,
    token: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut request = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(token) = token {
        request = request.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }
    let request = request
        .body(
            body.map(|value| Body::from(serde_json::to_vec(&value).unwrap()))
                .unwrap_or_else(Body::empty),
        )
        .unwrap();
    let response = router.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let json = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into_owned()))
    };
    (status, json)
}

async fn seed_org(pool: &PgPool, org: Uuid, slug: &str) {
    sqlx::query("INSERT INTO organizations (id, slug, name) VALUES ($1, $2, $2) ON CONFLICT (id) DO NOTHING")
        .bind(org)
        .bind(slug)
        .execute(pool)
        .await
        .unwrap();
}

async fn seed_branch(pool: &PgPool, org: OrgId, name: &str) -> BranchId {
    let region: Uuid =
        sqlx::query_scalar("INSERT INTO regions (name, org_id) VALUES ($1, $2) RETURNING id")
            .bind(format!("{name}-권역"))
            .bind(*org.as_uuid())
            .fetch_one(pool)
            .await
            .unwrap();
    BranchId::from_uuid(
        sqlx::query_scalar(
            "INSERT INTO branches (region_id, name, org_id) VALUES ($1, $2, $3) RETURNING id",
        )
        .bind(region)
        .bind(name)
        .bind(*org.as_uuid())
        .fetch_one(pool)
        .await
        .unwrap(),
    )
}

async fn seed_user(pool: &PgPool, org: OrgId, user: UserId, branch: BranchId, role: &str) {
    sqlx::query("INSERT INTO users (id, display_name, roles, is_active, org_id) VALUES ($1, $2, $3, true, $4)")
        .bind(*user.as_uuid())
        .bind(format!("evaluation-{role}-{user}"))
        .bind(vec![role])
        .bind(*org.as_uuid())
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO user_branches (user_id, branch_id, org_id) VALUES ($1, $2, $3)")
        .bind(*user.as_uuid())
        .bind(*branch.as_uuid())
        .bind(*org.as_uuid())
        .execute(pool)
        .await
        .unwrap();
}

async fn seed_employee(pool: &PgPool, org: OrgId, name: &str, org_unit: &str, row: i32) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO employees (org_id, company, name, org_unit, source_filename, source_sheet, source_row, source_key) \
         VALUES ($1, 'KNL', $2, $3, 'evaluation-test.xlsx', 'Sheet1', $4, $5) RETURNING id",
    )
    .bind(*org.as_uuid())
    .bind(name)
    .bind(org_unit)
    .bind(row)
    .bind(format!("evaluation-test-{row}"))
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn link_user_employee(pool: &PgPool, org: OrgId, user: UserId, employee: Uuid) {
    sqlx::query("UPDATE users SET employee_id = $1 WHERE id = $2 AND org_id = $3")
        .bind(employee)
        .bind(*user.as_uuid())
        .bind(*org.as_uuid())
        .execute(pool)
        .await
        .unwrap();
}

/// Grant one evaluation feature to a user through the declarative policy-role
/// map (the deny-by-default feature catalog registered by migration 0190).
async fn grant_feature(pool: &PgPool, org: OrgId, user: UserId, feature: &str) {
    let role: Uuid = sqlx::query_scalar(
        "INSERT INTO policy_roles (org_id, role_key, display_name, status, is_system, created_by, updated_by) \
         VALUES ($1, $2, $3, 'ACTIVE', false, $4, $4) RETURNING id",
    )
    .bind(*org.as_uuid())
    .bind(format!("evaluation_{}", Uuid::new_v4().simple()))
    .bind("Evaluation submitter")
    .bind(*user.as_uuid())
    .fetch_one(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO policy_role_permissions (org_id, role_id, feature_key, permission_level) VALUES ($1, $2, $3, 'allow')",
    )
    .bind(*org.as_uuid())
    .bind(role)
    .bind(feature)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO user_role_assignments (org_id, user_id, role_id, assigned_by) VALUES ($1, $2, $3, $2)")
        .bind(*org.as_uuid())
        .bind(*user.as_uuid())
        .bind(role)
        .execute(pool)
        .await
        .unwrap();
}
