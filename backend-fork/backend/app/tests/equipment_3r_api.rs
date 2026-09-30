#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
//! Authenticated, runtime-role PostgreSQL story for the bounded equipment 3R
//! pilot.  It intentionally crosses the assembled HTTP router rather than
//! calling stores, and runs as `console_rt` so RLS is exercised, not bypassed.

use axum::body::{Body, to_bytes};
use console_app::{AppConfig, AppRole, AppState, DatabaseDependency, build_router};
use console_kernel_core::{BranchId, OrgId, UserId};
use console_platform_auth::{AccessTokenInput, JwtIssuer, JwtSettings};
use http::{Request, StatusCode, header};
use p256::ecdsa::SigningKey;
use p256::elliptic_curve::rand_core::OsRng;
use p256::pkcs8::{EncodePrivateKey, EncodePublicKey, LineEnding};
use serde_json::{Value, json};
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use std::sync::atomic::{AtomicU16, Ordering};
use time::format_description::well_known::Rfc3339;
use time::{Duration, OffsetDateTime};
use tower::ServiceExt;
use uuid::Uuid;

const ISSUER: &str = "console-platform-auth";
const AUDIENCE: &str = "console-api";
const UNITS: &str = "/api/v1/equipment-3r/units";
const CASES: &str = "/api/v1/equipment-3r/rental-cases";
const ALL_FEATURES: &[&str] = &[
    "equipment_3r_registry",
    "equipment_3r_quote",
    "equipment_3r_approve",
    "equipment_3r_dispatch",
    "equipment_3r_inspect",
    "equipment_3r_assess",
    "equipment_3r_disposition",
    "equipment_3r_observe",
];

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn repair_lifecycle_completes_with_audits_history_and_no_finance_posting(pool: PgPool) {
    let keys = Keys::generate();
    let rt = runtime_role_pool(&pool).await;
    let branch = seed_branch(&pool, OrgId::knl(), "equip-main").await;
    let operator = seed_actor_with_grants(&pool, OrgId::knl(), branch, ALL_FEATURES).await;
    let approver =
        seed_actor_with_grants(&pool, OrgId::knl(), branch, &["equipment_3r_approve"]).await;
    let token = keys.token(operator, OrgId::knl(), vec!["MEMBER".into()], vec![branch]);
    let approver_token = keys.token(approver, OrgId::knl(), vec!["MEMBER".into()], vec![branch]);
    let handover_evidence =
        seed_eligible_handover_evidence(&pool, OrgId::knl(), branch, operator).await;

    let (status, unit) = send(
        &rt,
        &keys,
        "POST",
        UNITS,
        &token,
        Some(json!({
            "branchId": branch, "serialNo": "3R-STORY-0001", "modelName": "FX-25 Diesel",
            "capacityClass": "2.5t", "acquisitionCostMinor": 32_000_000_i64
        })),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "register unit: {unit}");
    assert_eq!(unit["availability"], "AVAILABLE");
    let unit_id = unit["id"].as_str().unwrap().to_owned();

    let quote = json!({
        "branchId": branch, "unitId": unit_id, "customerName": "성산건설",
        "siteReference": "창원 성산구 현장 A", "monthlyRateMinor": 1_500_000_i64,
        "durationMonths": 12, "currencyCode": "KRW"
    });
    let key = "equip-story-key-000001";
    let (status, case) = send(
        &rt,
        &keys,
        "POST",
        CASES,
        &token,
        Some(quote.clone()),
        Some(key),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "quote: {case}");
    assert_eq!(case["status"], "QUOTED");
    let case_id = case["id"].as_str().unwrap().to_owned();

    let (status, replay) = send(
        &rt,
        &keys,
        "POST",
        CASES,
        &token,
        Some(quote.clone()),
        Some(key),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "idempotent replay: {replay}");
    assert_eq!(replay["replayed"], true);
    assert_eq!(replay["id"], case["id"]);
    let mut changed = quote.clone();
    changed["monthlyRateMinor"] = json!(1_600_000_i64);
    let (status, conflicted) =
        send(&rt, &keys, "POST", CASES, &token, Some(changed), Some(key)).await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "changed replay must conflict: {conflicted}"
    );

    let approval_path = format!("{CASES}/{case_id}/approval");
    let (status, denied) = send(
        &rt,
        &keys,
        "POST",
        &approval_path,
        &token,
        Some(json!({"decision": "APPROVED"})),
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "creator self-approval must hit four-eyes: {denied}"
    );
    let (status, approved) = send(
        &rt,
        &keys,
        "POST",
        &approval_path,
        &approver_token,
        Some(json!({"decision": "APPROVED"})),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "approval: {approved}");
    assert_eq!(approved["status"], "APPROVED");

    let (status, detail) = send(
        &rt,
        &keys,
        "GET",
        &format!("{UNITS}/{unit_id}"),
        &token,
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "unit detail: {detail}");
    assert_eq!(detail["availability"], "RESERVED");
    assert_eq!(detail["activeCaseId"], case["id"]);

    let (status, dispatched) = send(
        &rt,
        &keys,
        "POST",
        &format!("{CASES}/{case_id}/dispatch"),
        &token,
        Some(json!({"carrierName": "Pilot Carrier", "vehicleReference": "TRUCK-3R-1"})),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "dispatch: {dispatched}");
    assert_eq!(dispatched["status"], "DISPATCHED");

    let handed_over_at = OffsetDateTime::now_utc().format(&Rfc3339).unwrap();
    let (status, handed) = send(&rt, &keys, "POST", &format!("{CASES}/{case_id}/handover"), &token, Some(json!({"recipientName": "현장 소장", "evidenceObjectId": handover_evidence, "handedOverAt": handed_over_at})), None).await;
    assert_eq!(status, StatusCode::OK, "handover: {handed}");
    assert_eq!(handed["status"], "HANDED_OVER");

    // The concealment story proves the four refusals; this is the other half of
    // the pair — that a permitted handover actually writes the custody relation
    // and that the read path surfaces the typed object instead of the retired
    // `evidenceReference` string.
    let (status, case_detail) = send(
        &rt,
        &keys,
        "GET",
        &format!("{CASES}/{case_id}"),
        &token,
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "case detail: {case_detail}");
    assert_eq!(
        case_detail["handover"]["evidenceObjectId"],
        json!(handover_evidence),
        "the detail view must surface the bound evidence object: {case_detail}"
    );
    let original_copy: Uuid = sqlx::query_scalar(
        "SELECT id FROM docs_evidence_copies WHERE evidence_object_id=$1 AND copy_kind='ORIGINAL'",
    )
    .bind(handover_evidence)
    .fetch_one(&pool)
    .await
    .unwrap();
    let custody: (Uuid, Uuid, Uuid, Uuid) = sqlx::query_as(
        "SELECT evidence_object_id,original_copy_id,branch_id,created_by \
         FROM docs_equipment_handover_custody WHERE equipment_case_id=$1",
    )
    .bind(case_id.parse::<Uuid>().unwrap())
    .fetch_one(&pool)
    .await
    .expect("handover must bind exactly one immutable custody row");
    assert_eq!(
        custody,
        (
            handover_evidence,
            original_copy,
            *branch.as_uuid(),
            *operator.as_uuid()
        ),
        "custody must record the object, its verified ORIGINAL copy, the case branch and the actor"
    );

    let inspections_path = format!("{CASES}/{case_id}/inspections");
    let (status, pass) = send(
        &rt,
        &keys,
        "POST",
        &inspections_path,
        &token,
        Some(json!({"outcome": "PASS", "findings": "마스트/체인 정상"})),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "PASS inspection: {pass}");
    let (status, invalid) = send(
        &rt,
        &keys,
        "POST",
        &inspections_path,
        &token,
        Some(json!({"outcome": "MAINTENANCE_PERFORMED", "findings": "유압 누유"})),
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "maintenance without note must fail validation: {invalid}"
    );
    let (status, maintained) = send(&rt, &keys, "POST", &inspections_path, &token, Some(json!({"outcome": "MAINTENANCE_PERFORMED", "findings": "유압 누유", "maintenanceNote": "호스 교체"})), None).await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "maintenance inspection: {maintained}"
    );

    let returned_at = OffsetDateTime::now_utc().format(&Rfc3339).unwrap();
    let (status, returned) = send(
        &rt,
        &keys,
        "POST",
        &format!("{CASES}/{case_id}/return"),
        &token,
        Some(json!({"returnedAt": returned_at})),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "return: {returned}");
    assert_eq!(returned["status"], "RETURNED");

    let (status, closed) = send(&rt, &keys, "POST", &format!("{CASES}/{case_id}/assessment"), &token, Some(json!({"conditionGrade": "B", "findings": "타이어 마모, 수리 필요", "disposition": "REPAIR"})), None).await;
    assert_eq!(status, StatusCode::OK, "assessment: {closed}");
    assert_eq!(closed["status"], "CLOSED");
    assert_eq!(closed["assessment"]["disposition"], "REPAIR");
    assert_eq!(closed["inspections"].as_array().unwrap().len(), 2);
    let disposition_id = closed["dispositionId"].as_str().unwrap().to_owned();

    let (status, in_repair) = send(
        &rt,
        &keys,
        "GET",
        &format!("{UNITS}/{unit_id}"),
        &token,
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(in_repair["availability"], "IN_REPAIR");
    assert_eq!(in_repair["openDispositionId"], closed["dispositionId"]);

    let (status, completed) = send(
        &rt,
        &keys,
        "POST",
        &format!("/api/v1/equipment-3r/dispositions/{disposition_id}/completion"),
        &token,
        Some(json!({"costMinor": 850_000_i64})),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "completion: {completed}");
    assert_eq!(completed["status"], "COMPLETED");
    assert!(
        completed["financeGlPosting"].is_null(),
        "pilot must not claim a GL posting: {completed}"
    );

    let (status, available_again) = send(
        &rt,
        &keys,
        "GET",
        &format!("{UNITS}/{unit_id}"),
        &token,
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(available_again["availability"], "AVAILABLE");
    assert!(available_again["openDispositionId"].is_null());

    let (status, history) = send(
        &rt,
        &keys,
        "GET",
        &format!("{UNITS}/{unit_id}/history"),
        &token,
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "history: {history}");
    let entries = history.as_array().unwrap();
    assert_eq!(
        entries.len(),
        14,
        "6 unit + 6 case + 2 disposition transitions: {history}"
    );

    let case_audits: i64 =
        sqlx::query_scalar("SELECT count(*) FROM audit_events WHERE target_id = $1")
            .bind(&case_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        case_audits, 8,
        "quote, approval, dispatch, handover, 2 inspections, return, assess"
    );
    let unit_audits: i64 =
        sqlx::query_scalar("SELECT count(*) FROM audit_events WHERE target_id = $1")
            .bind(&unit_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(unit_audits, 1, "unit registration audit");
    let disposition_audits: i64 =
        sqlx::query_scalar("SELECT count(*) FROM audit_events WHERE target_id = $1")
            .bind(&disposition_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(disposition_audits, 1, "disposition completion audit");

    // Org-wide observation: an all-branch principal reads the list; the
    // branch-scoped operator is denied the org-wide surface.
    let observer =
        seed_actor_with_grants(&pool, OrgId::knl(), branch, &["equipment_3r_observe"]).await;
    sqlx::query("UPDATE users SET roles = $1 WHERE id = $2")
        .bind(vec!["EXECUTIVE"])
        .bind(*observer.as_uuid())
        .execute(&pool)
        .await
        .unwrap();
    let observer_token = keys.token(observer, OrgId::knl(), vec!["EXECUTIVE".into()], vec![]);
    let (status, listed) = send(&rt, &keys, "GET", UNITS, &observer_token, None, None).await;
    assert_eq!(status, StatusCode::OK, "org-wide unit list: {listed}");
    assert!(
        listed
            .as_array()
            .unwrap()
            .iter()
            .any(|u| u["id"] == unit["id"])
    );
    let (status, denied_list) = send(&rt, &keys, "GET", UNITS, &token, None, None).await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "branch-scoped list must be denied: {denied_list}"
    );
    let (status, cases_listed) = send(&rt, &keys, "GET", CASES, &observer_token, None, None).await;
    assert_eq!(status, StatusCode::OK, "org-wide case list: {cases_listed}");
    assert!(
        cases_listed
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["id"] == case["id"])
    );
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn capabilities_deny_without_leakage_across_branch_grant_and_org(pool: PgPool) {
    let keys = Keys::generate();
    let rt = runtime_role_pool(&pool).await;
    let branch_a = seed_branch(&pool, OrgId::knl(), "equip-a").await;
    let branch_b = seed_branch(&pool, OrgId::knl(), "equip-b").await;
    let operator = seed_actor_with_grants(&pool, OrgId::knl(), branch_a, ALL_FEATURES).await;
    let token = keys.token(
        operator,
        OrgId::knl(),
        vec!["MEMBER".into()],
        vec![branch_a],
    );

    let (status, widened) = send(
        &rt,
        &keys,
        "POST",
        UNITS,
        &token,
        Some(json!({
            "branchId": branch_b, "serialNo": "3R-OUTSIDE-0001", "modelName": "FX-25",
            "capacityClass": "2.5t", "acquisitionCostMinor": 1_i64
        })),
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "grant cannot widen JWT branch scope: {widened}"
    );

    let ungranted = UserId::new();
    seed_user(&pool, OrgId::knl(), ungranted, branch_a).await;
    let ungranted_token = keys.token(
        ungranted,
        OrgId::knl(),
        vec!["MEMBER".into()],
        vec![branch_a],
    );
    let (status, denied) = send(
        &rt,
        &keys,
        "POST",
        UNITS,
        &ungranted_token,
        Some(json!({
            "branchId": branch_a, "serialNo": "3R-NOGRANT-0001", "modelName": "FX-25",
            "capacityClass": "2.5t", "acquisitionCostMinor": 1_i64
        })),
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "equipment 3R is PBAC grant-only: {denied}"
    );

    let (status, unit) = send(
        &rt,
        &keys,
        "POST",
        UNITS,
        &token,
        Some(json!({
            "branchId": branch_a, "serialNo": "3R-ISOLATION-0001", "modelName": "FX-30",
            "capacityClass": "3.0t", "acquisitionCostMinor": 40_000_000_i64
        })),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "register: {unit}");
    let unit_id = unit["id"].as_str().unwrap().to_owned();
    let (status, case) = send(
        &rt,
        &keys,
        "POST",
        CASES,
        &token,
        Some(json!({
            "branchId": branch_a, "unitId": unit_id, "customerName": "고객",
            "siteReference": "현장", "monthlyRateMinor": 1_000_000_i64,
            "durationMonths": 6, "currencyCode": "KRW"
        })),
        Some("equip-isolation-key-0001"),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "quote: {case}");
    let case_id = case["id"].as_str().unwrap().to_owned();

    // Cross-org concealment as the runtime role: a fully-granted actor in a
    // second org must see 404, never 403, for first-org objects.
    let org2 = OrgId::from_uuid(
        sqlx::query_scalar("INSERT INTO organizations (slug, name) VALUES ('equip-pilot-two', 'Equip Pilot Two') RETURNING id")
            .fetch_one(&pool).await.unwrap(),
    );
    let branch2 = seed_branch(&pool, org2, "equip-two-main").await;
    let outsider = seed_actor_with_grants(&pool, org2, branch2, ALL_FEATURES).await;
    let outsider_token = keys.token(outsider, org2, vec!["MEMBER".into()], vec![branch2]);
    let (status, concealed_unit) = send(
        &rt,
        &keys,
        "GET",
        &format!("{UNITS}/{unit_id}"),
        &outsider_token,
        None,
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "cross-org unit must be concealed: {concealed_unit}"
    );
    let (status, concealed_case) = send(
        &rt,
        &keys,
        "POST",
        &format!("{CASES}/{case_id}/approval"),
        &outsider_token,
        Some(json!({"decision": "APPROVED"})),
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "cross-org case must be concealed: {concealed_case}"
    );
    let status_now: String =
        sqlx::query_scalar("SELECT status FROM equipment_3r_rental_cases WHERE id = $1")
            .bind(Uuid::parse_str(&case_id).unwrap())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        status_now, "QUOTED",
        "denied approvals must not transition the case"
    );

    // Count-leak-free listing: an org-wide observer in the second org sees an
    // EMPTY list, never first-org rows or counts.
    sqlx::query("UPDATE users SET roles = $1 WHERE id = $2")
        .bind(vec!["EXECUTIVE"])
        .bind(*outsider.as_uuid())
        .execute(&pool)
        .await
        .unwrap();
    let outsider_exec = keys.token(outsider, org2, vec!["EXECUTIVE".into()], vec![]);
    let (status, foreign_units) = send(&rt, &keys, "GET", UNITS, &outsider_exec, None, None).await;
    assert_eq!(status, StatusCode::OK, "org-2 unit list: {foreign_units}");
    assert_eq!(
        foreign_units.as_array().unwrap().len(),
        0,
        "cross-org unit list must be empty: {foreign_units}"
    );
    let (status, foreign_cases) = send(&rt, &keys, "GET", CASES, &outsider_exec, None, None).await;
    assert_eq!(status, StatusCode::OK, "org-2 case list: {foreign_cases}");
    assert_eq!(
        foreign_cases.as_array().unwrap().len(),
        0,
        "cross-org case list must be empty: {foreign_cases}"
    );

    // An org-wide principal skips the JWT branch-scope check, so the store
    // must still conceal a cross-org (or nonexistent) branch as 404 — never
    // surface the FK violation as a 500.
    let (status, foreign_branch) = send(
        &rt,
        &keys,
        "POST",
        UNITS,
        &outsider_exec,
        Some(json!({
            "branchId": branch_a, "serialNo": "3R-FOREIGN-0001", "modelName": "FX-25",
            "capacityClass": "2.5t", "acquisitionCostMinor": 1_i64
        })),
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "cross-org branch must be concealed on register: {foreign_branch}"
    );
    assert_eq!(foreign_branch["error"]["code"], "not_found");
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn concurrent_approvals_on_one_unit_have_exactly_one_winner(pool: PgPool) {
    let keys = Keys::generate();
    let rt = runtime_role_pool(&pool).await;
    let branch = seed_branch(&pool, OrgId::knl(), "equip-race").await;
    let operator = seed_actor_with_grants(&pool, OrgId::knl(), branch, ALL_FEATURES).await;
    let approver =
        seed_actor_with_grants(&pool, OrgId::knl(), branch, &["equipment_3r_approve"]).await;
    let token = keys.token(operator, OrgId::knl(), vec!["MEMBER".into()], vec![branch]);
    let approver_token = keys.token(approver, OrgId::knl(), vec!["MEMBER".into()], vec![branch]);

    let (status, unit) = send(
        &rt,
        &keys,
        "POST",
        UNITS,
        &token,
        Some(json!({
            "branchId": branch, "serialNo": "3R-RACE-0001", "modelName": "FX-25",
            "capacityClass": "2.5t", "acquisitionCostMinor": 30_000_000_i64
        })),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "register: {unit}");
    let unit_id = unit["id"].as_str().unwrap().to_owned();

    let mut case_ids = Vec::new();
    for i in 0..2 {
        let (status, case) = send(
            &rt,
            &keys,
            "POST",
            CASES,
            &token,
            Some(json!({
                "branchId": branch, "unitId": unit_id, "customerName": format!("고객-{i}"),
                "siteReference": format!("현장-{i}"), "monthlyRateMinor": 1_200_000_i64,
                "durationMonths": 6, "currencyCode": "KRW"
            })),
            Some(&format!("equip-race-key-000{i}")),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "quote {i}: {case}");
        case_ids.push(case["id"].as_str().unwrap().to_owned());
    }

    let body = json!({"decision": "APPROVED"});
    let path_first = format!("{CASES}/{}/approval", case_ids[0]);
    let path_second = format!("{CASES}/{}/approval", case_ids[1]);
    let (first, second) = tokio::join!(
        send(
            &rt,
            &keys,
            "POST",
            &path_first,
            &approver_token,
            Some(body.clone()),
            None
        ),
        send(
            &rt,
            &keys,
            "POST",
            &path_second,
            &approver_token,
            Some(body),
            None
        )
    );
    let statuses = [first.0, second.0];
    assert_eq!(
        statuses.iter().filter(|s| **s == StatusCode::OK).count(),
        1,
        "exactly one concurrent approval may win: {} / {}",
        first.1,
        second.1
    );
    assert_eq!(
        statuses
            .iter()
            .filter(|s| **s == StatusCode::CONFLICT)
            .count(),
        1,
        "the losing approval must conflict without reserving: {} / {}",
        first.1,
        second.1
    );
    let availability: String =
        sqlx::query_scalar("SELECT availability FROM equipment_3r_units WHERE id = $1")
            .bind(Uuid::parse_str(&unit_id).unwrap())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        availability, "RESERVED",
        "the unit is reserved exactly once"
    );
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn resale_disposition_sells_unit_and_blocks_further_quotes(pool: PgPool) {
    let keys = Keys::generate();
    let rt = runtime_role_pool(&pool).await;
    let branch = seed_branch(&pool, OrgId::knl(), "equip-resale").await;
    let operator = seed_actor_with_grants(&pool, OrgId::knl(), branch, ALL_FEATURES).await;
    let approver =
        seed_actor_with_grants(&pool, OrgId::knl(), branch, &["equipment_3r_approve"]).await;
    let token = keys.token(operator, OrgId::knl(), vec!["MEMBER".into()], vec![branch]);
    let approver_token = keys.token(approver, OrgId::knl(), vec!["MEMBER".into()], vec![branch]);
    let handover_evidence =
        seed_eligible_handover_evidence(&pool, OrgId::knl(), branch, operator).await;

    let (status, unit) = send(
        &rt,
        &keys,
        "POST",
        UNITS,
        &token,
        Some(json!({
            "branchId": branch, "serialNo": "3R-RESALE-0001", "modelName": "FX-20",
            "capacityClass": "2.0t", "acquisitionCostMinor": 20_000_000_i64
        })),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "register: {unit}");
    let unit_id = unit["id"].as_str().unwrap().to_owned();
    let (status, case) = send(
        &rt,
        &keys,
        "POST",
        CASES,
        &token,
        Some(json!({
            "branchId": branch, "unitId": unit_id, "customerName": "매각 전 고객",
            "siteReference": "현장 B", "monthlyRateMinor": 900_000_i64,
            "durationMonths": 3, "currencyCode": "KRW"
        })),
        Some("equip-resale-key-0001"),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "quote: {case}");
    let case_id = case["id"].as_str().unwrap().to_owned();

    let (status, approved) = send(
        &rt,
        &keys,
        "POST",
        &format!("{CASES}/{case_id}/approval"),
        &approver_token,
        Some(json!({"decision": "APPROVED"})),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "approval: {approved}");
    let (status, _) = send(
        &rt,
        &keys,
        "POST",
        &format!("{CASES}/{case_id}/dispatch"),
        &token,
        Some(json!({"carrierName": "Carrier", "vehicleReference": "TRUCK-2"})),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let handed_over_at = OffsetDateTime::now_utc().format(&Rfc3339).unwrap();
    let (status, _) = send(&rt, &keys, "POST", &format!("{CASES}/{case_id}/handover"), &token, Some(json!({"recipientName": "수령인", "evidenceObjectId": handover_evidence, "handedOverAt": handed_over_at})), None).await;
    assert_eq!(status, StatusCode::OK);
    let returned_at = OffsetDateTime::now_utc().format(&Rfc3339).unwrap();
    let (status, _) = send(
        &rt,
        &keys,
        "POST",
        &format!("{CASES}/{case_id}/return"),
        &token,
        Some(json!({"returnedAt": returned_at})),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, closed) = send(&rt, &keys, "POST", &format!("{CASES}/{case_id}/assessment"), &token, Some(json!({"conditionGrade": "C", "findings": "노후화, 매각 대상", "disposition": "RESALE"})), None).await;
    assert_eq!(status, StatusCode::OK, "assessment: {closed}");
    let disposition_id = closed["dispositionId"].as_str().unwrap().to_owned();
    let availability: String =
        sqlx::query_scalar("SELECT availability FROM equipment_3r_units WHERE id = $1")
            .bind(Uuid::parse_str(&unit_id).unwrap())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(availability, "FOR_SALE");

    let completion_path = format!("/api/v1/equipment-3r/dispositions/{disposition_id}/completion");
    let (status, invalid) = send(
        &rt,
        &keys,
        "POST",
        &completion_path,
        &token,
        Some(json!({"costMinor": 1_i64})),
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "resale completion requires sale fields: {invalid}"
    );
    let (status, sold) = send(
        &rt,
        &keys,
        "POST",
        &completion_path,
        &token,
        Some(json!({"saleAmountMinor": 9_000_000_i64, "buyerName": "중고장비상사"})),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "resale completion: {sold}");
    assert_eq!(sold["status"], "COMPLETED");
    assert!(sold["financeGlPosting"].is_null());
    let availability: String =
        sqlx::query_scalar("SELECT availability FROM equipment_3r_units WHERE id = $1")
            .bind(Uuid::parse_str(&unit_id).unwrap())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(availability, "SOLD");

    let (status, requoted) = send(
        &rt,
        &keys,
        "POST",
        CASES,
        &token,
        Some(json!({
            "branchId": branch, "unitId": unit_id, "customerName": "새 고객",
            "siteReference": "현장 C", "monthlyRateMinor": 800_000_i64,
            "durationMonths": 2, "currencyCode": "KRW"
        })),
        Some("equip-resale-key-0002"),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "sold unit must reject new quotes: {requoted}"
    );

    let (status, completed_again) = send(
        &rt,
        &keys,
        "POST",
        &completion_path,
        &token,
        Some(json!({"saleAmountMinor": 1_i64, "buyerName": "재시도"})),
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "completed disposition is terminal: {completed_again}"
    );
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn handover_conceals_ineligible_or_foreign_evidence_and_denies_foreign_branch(pool: PgPool) {
    let keys = Keys::generate();
    let rt = runtime_role_pool(&pool).await;
    let branch = seed_branch(&pool, OrgId::knl(), "equip-evidence").await;
    let foreign_branch = seed_branch(&pool, OrgId::knl(), "equip-evidence-foreign").await;
    let operator = seed_actor_with_grants(&pool, OrgId::knl(), branch, ALL_FEATURES).await;
    let foreign_operator = seed_actor_with_grants(
        &pool,
        OrgId::knl(),
        foreign_branch,
        &["equipment_3r_dispatch"],
    )
    .await;
    let token = keys.token(operator, OrgId::knl(), vec!["MEMBER".into()], vec![branch]);
    let foreign_branch_token = keys.token(
        foreign_operator,
        OrgId::knl(),
        vec!["MEMBER".into()],
        vec![foreign_branch],
    );

    let org2 = OrgId::from_uuid(
        sqlx::query_scalar("INSERT INTO organizations (slug,name) VALUES ('equipment-evidence-org-two','Equipment Evidence Org Two') RETURNING id")
            .fetch_one(&pool).await.unwrap(),
    );
    let org2_branch = seed_branch(&pool, org2, "equip-evidence-org-two").await;
    let org2_actor = seed_actor_with_grants(&pool, org2, org2_branch, &[]).await;

    let rejected = [
        (
            "foreign-org",
            seed_handover_evidence_variant(&pool, org2, org2_branch, org2_actor, true, false, true)
                .await,
        ),
        (
            "mutable",
            seed_handover_evidence_variant(
                &pool,
                OrgId::knl(),
                branch,
                operator,
                true,
                false,
                false,
            )
            .await,
        ),
        (
            "disposed",
            seed_handover_evidence_variant(&pool, OrgId::knl(), branch, operator, true, true, true)
                .await,
        ),
        (
            "non-admissible",
            seed_handover_evidence_variant(
                &pool,
                OrgId::knl(),
                branch,
                operator,
                false,
                false,
                true,
            )
            .await,
        ),
    ];
    for (label, evidence_object_id) in rejected {
        let case = seed_dispatched_case(&pool, OrgId::knl(), branch, operator, label).await;
        let (status, body) = send(
            &rt, &keys, "POST", &format!("{CASES}/{case}/handover"), &token,
            Some(json!({"recipientName":"수령인","evidenceObjectId":evidence_object_id,"handedOverAt":OffsetDateTime::now_utc().format(&Rfc3339).unwrap()})), None,
        ).await;
        assert_eq!(
            status,
            StatusCode::NOT_FOUND,
            "{label} must be concealed: {body}"
        );
        let state: String =
            sqlx::query_scalar("SELECT status FROM equipment_3r_rental_cases WHERE id=$1")
                .bind(case)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(state, "DISPATCHED", "{label} must not transition the case");
    }

    let valid = seed_eligible_handover_evidence(&pool, OrgId::knl(), branch, operator).await;
    let case = seed_dispatched_case(&pool, OrgId::knl(), branch, operator, "foreign-branch").await;
    let (status, body) = send(
        &rt, &keys, "POST", &format!("{CASES}/{case}/handover"), &foreign_branch_token,
        Some(json!({"recipientName":"수령인","evidenceObjectId":valid,"handedOverAt":OffsetDateTime::now_utc().format(&Rfc3339).unwrap()})), None,
    ).await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "foreign branch must be denied: {body}"
    );
    let custody_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM docs_equipment_handover_custody WHERE equipment_case_id=$1",
    )
    .bind(case)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(custody_count, 0, "unauthorized branch cannot bind custody");
}

struct Keys {
    private_pem: String,
    public_pem: String,
}
impl Keys {
    fn generate() -> Self {
        let key = SigningKey::random(&mut OsRng);
        Self {
            private_pem: key.to_pkcs8_pem(LineEnding::LF).unwrap().to_string(),
            public_pem: key
                .verifying_key()
                .to_public_key_pem(LineEnding::LF)
                .unwrap(),
        }
    }
    fn token(
        &self,
        user: UserId,
        org: OrgId,
        roles: Vec<String>,
        branches: Vec<BranchId>,
    ) -> String {
        JwtIssuer::from_es256_pem(
            JwtSettings {
                issuer: ISSUER.into(),
                audience: AUDIENCE.into(),
                access_token_ttl: Duration::minutes(15),
            },
            self.private_pem.as_bytes(),
            self.public_pem.as_bytes(),
        )
        .unwrap()
        .issue_access_token(AccessTokenInput {
            subject: user,
            org_id: org,
            roles,
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
async fn send(
    pool: &PgPool,
    keys: &Keys,
    method: &str,
    uri: &str,
    token: &str,
    body: Option<Value>,
    key: Option<&str>,
) -> (StatusCode, Value) {
    let mut request = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(key) = key {
        request = request.header("Idempotency-Key", key);
    }
    let request = request
        .body(
            body.map(|v| Body::from(serde_json::to_vec(&v).unwrap()))
                .unwrap_or_else(Body::empty),
        )
        .unwrap();
    let response = build_router(app_state(pool.clone(), keys.public_pem.clone()).unwrap())
        .oneshot(request)
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    (
        status,
        if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap()
        },
    )
}
fn app_state(pool: PgPool, public_key: String) -> Result<AppState, console_app::AppError> {
    AppState::new(
        AppConfig::from_pairs([
            ("CONSOLE_APP_ROLE", AppRole::Api.to_string()),
            ("CONSOLE_HTTP_ADDR", "127.0.0.1:0".into()),
            ("CONSOLE_JWT_ISSUER", ISSUER.into()),
            ("CONSOLE_JWT_AUDIENCE", AUDIENCE.into()),
            ("CONSOLE_JWT_PUBLIC_KEY_PEM", public_key),
        ])?,
        DatabaseDependency::Postgres(pool),
    )
}
async fn seed_eligible_handover_evidence(
    pool: &PgPool,
    org: OrgId,
    branch: BranchId,
    actor: UserId,
) -> Uuid {
    seed_handover_evidence_variant(pool, org, branch, actor, true, false, true).await
}

/// Seed one Docs/Evidence object with an ORIGINAL copy.
///
/// `verified_worm` is not a flag the fixture may assert: migration 0195's
/// `docs_evidence_copies_bind_storage_attestation` trigger overwrites any
/// caller-supplied `worm_status`/`verified_at` and promotes a copy to VERIFIED
/// only when a tenant-matching `evidence_media` row proves the WORM replica by
/// object key and SHA-256.  The eligible variant therefore seeds that real
/// storage attestation; the ineligible variant simply omits it.
async fn seed_handover_evidence_variant(
    pool: &PgPool,
    org: OrgId,
    branch: BranchId,
    actor: UserId,
    admissible: bool,
    disposed: bool,
    verified_worm: bool,
) -> Uuid {
    let object_id = Uuid::new_v4();
    let copy_id = Uuid::new_v4();
    let digest = "a".repeat(64);
    let storage_key = format!("handover-{object_id}.pdf");
    let media = if verified_worm {
        Some(seed_verified_worm_media(pool, org, branch, actor, &storage_key, &digest).await)
    } else {
        None
    };
    let status = if admissible {
        "ADMISSIBLE"
    } else {
        "REVIEW_NEEDED"
    };
    let stage = if disposed {
        "DISPOSED"
    } else {
        "ADMISSIBILITY_EVALUATED"
    };
    sqlx::query(
        "INSERT INTO docs_evidence_objects \
         (id,org_id,code,title,source_type,source_id,classification,admissibility_status,current_custody_stage,disposed_at,disposed_by,disposal_reason,created_by,updated_by) \
         VALUES ($1,$2,$3,'Equipment handover evidence','external_document',$4,'GENERAL',$5,$6,$7,$8,$9,$10,$10)",
    )
    .bind(object_id)
    .bind(*org.as_uuid())
    .bind(format!("EV-EQUIP-{}", object_id.simple().to_string().to_uppercase()))
    .bind(format!("equipment-handover-{object_id}"))
    .bind(status)
    .bind(stage)
    .bind(disposed.then_some(OffsetDateTime::now_utc()))
    .bind(disposed.then_some(*actor.as_uuid()))
    .bind(disposed.then_some("evidence disposition test"))
    .bind(*actor.as_uuid())
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO docs_evidence_copies \
         (id,org_id,evidence_object_id,copy_kind,storage_provider,storage_object_id,source_evidence_media_id,digest_sha256,content_type,size_bytes,created_by) \
         VALUES ($1,$2,$3,'ORIGINAL','seaweedfs-worm',$4,$5,$6,'application/pdf',1024,$7)",
    )
    .bind(copy_id)
    .bind(*org.as_uuid())
    .bind(object_id)
    .bind(&storage_key)
    .bind(media)
    .bind(&digest)
    .bind(*actor.as_uuid())
    .execute(pool)
    .await
    .unwrap();
    let promoted: String =
        sqlx::query_scalar("SELECT worm_status FROM docs_evidence_copies WHERE id=$1")
            .bind(copy_id)
            .fetch_one(pool)
            .await
            .unwrap();
    assert_eq!(
        promoted,
        if verified_worm { "VERIFIED" } else { "PENDING" },
        "the storage-attestation trigger, not the fixture, decides WORM verification"
    );
    object_id
}

/// Seed the server-written WORM replica attestation that migration 0195 makes
/// the sole promotion authority for an EV original.  Mirrors the fixture in
/// `backend/crates/docs/rest/tests/evidence_rest_rls_surfaces_as_runtime_role.rs`.
async fn seed_verified_worm_media(
    pool: &PgPool,
    org: OrgId,
    branch: BranchId,
    actor: UserId,
    storage_key: &str,
    digest_hex: &str,
) -> Uuid {
    let unique = Uuid::new_v4();
    let customer: Uuid = sqlx::query_scalar(
        "INSERT INTO registry_customers (branch_id,name,org_id) VALUES ($1,$2,$3) RETURNING id",
    )
    .bind(*branch.as_uuid())
    .bind(format!("Equipment handover evidence customer {unique}"))
    .bind(*org.as_uuid())
    .fetch_one(pool)
    .await
    .unwrap();
    let site: Uuid = sqlx::query_scalar(
        "INSERT INTO registry_sites (customer_id,branch_id,name,org_id) VALUES ($1,$2,$3,$4) RETURNING id",
    )
    .bind(customer)
    .bind(*branch.as_uuid())
    .bind(format!("Equipment handover evidence site {unique}"))
    .bind(*org.as_uuid())
    .fetch_one(pool)
    .await
    .unwrap();
    // `equipment_no` and `request_no` have fixed 4- and 3-digit shapes, so a
    // random suffix would collide. Count instead: each `#[sqlx::test]` gets its
    // own database and the whole binary seeds a handful of these.
    static SEQUENCE: AtomicU16 = AtomicU16::new(1);
    let suffix = SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let equipment: Uuid = sqlx::query_scalar(
        "INSERT INTO registry_equipment \
         (branch_id,customer_id,site_id,equipment_no,management_no,manufacturer_code,kind_code,power_code,status,specification,ton_text,model,source_sheet,source_row,org_id) \
         VALUES ($1,$2,$3,$4,$5,'S','T','R','임대','좌식','2.5','Model','test',1,$6) RETURNING id",
    )
    .bind(*branch.as_uuid())
    .bind(customer)
    .bind(site)
    .bind(format!("EVD01-{suffix:04}"))
    .bind(format!("M{suffix:04}"))
    .bind(*org.as_uuid())
    .fetch_one(pool)
    .await
    .unwrap();
    let work_order = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO work_orders \
         (id,request_no,branch_id,equipment_id,customer_id,site_id,requested_by,status,priority,symptom,result_type,org_id) \
         VALUES ($1,$2,$3,$4,$5,$6,$7,'REPORT_SUBMITTED','P3','Equipment handover evidence','COMPLETED',$8)",
    )
    .bind(work_order)
    .bind(format!("20260725-{suffix:03}"))
    .bind(*branch.as_uuid())
    .bind(equipment)
    .bind(customer)
    .bind(site)
    .bind(*actor.as_uuid())
    .bind(*org.as_uuid())
    .execute(pool)
    .await
    .unwrap();
    let media = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO evidence_media \
         (id,work_order_id,stage,s3_key,content_type,size_bytes,checksum_sha256,uploaded_by,worm_replica_status,verified_at,retry_count,next_retry_at,org_id) \
         VALUES ($1,$2,'REPORT',$3,'application/pdf',1024,encode(decode($4,'hex'),'base64'),$5,'VERIFIED',$6,0,$6,$7)",
    )
    .bind(media)
    .bind(work_order)
    .bind(storage_key)
    .bind(digest_hex)
    .bind(*actor.as_uuid())
    .bind(OffsetDateTime::now_utc())
    .bind(*org.as_uuid())
    .execute(pool)
    .await
    .unwrap();
    media
}

async fn seed_dispatched_case(
    pool: &PgPool,
    org: OrgId,
    branch: BranchId,
    actor: UserId,
    suffix: &str,
) -> Uuid {
    let unit = Uuid::new_v4();
    let case = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO equipment_3r_units \
         (id,org_id,branch_id,serial_no,model_name,capacity_class,acquisition_cost_minor,availability,created_by) \
         VALUES ($1,$2,$3,$4,'Evidence test unit','2.5t',1,'RESERVED',$5)",
    )
    .bind(unit).bind(*org.as_uuid()).bind(*branch.as_uuid())
    .bind(format!("EVIDENCE-{}", Uuid::new_v4().simple())).bind(*actor.as_uuid())
    .execute(pool).await.unwrap();
    sqlx::query(
        "INSERT INTO equipment_3r_rental_cases \
         (id,org_id,branch_id,unit_id,customer_name,site_reference,monthly_rate_minor,duration_months,currency_code,status,carrier_name,vehicle_reference,dispatched_at,idempotency_key,request_fingerprint,created_by) \
         VALUES ($1,$2,$3,$4,'Evidence test customer',$5,1,1,'KRW','DISPATCHED','Test carrier','TEST-VEHICLE',now(),$6,$7,$8)",
    )
    .bind(case).bind(*org.as_uuid()).bind(*branch.as_uuid()).bind(unit)
    .bind(format!("Evidence test {suffix}"))
    .bind(format!("evidence-dispatch-{}", Uuid::new_v4()))
    .bind("b".repeat(64)).bind(*actor.as_uuid())
    .execute(pool).await.unwrap();
    case
}

async fn seed_branch(pool: &PgPool, org: OrgId, name: &str) -> BranchId {
    let region: Uuid =
        sqlx::query_scalar("INSERT INTO regions (name, org_id) VALUES ($1,$2) RETURNING id")
            .bind(format!("region-{name}"))
            .bind(*org.as_uuid())
            .fetch_one(pool)
            .await
            .unwrap();
    BranchId::from_uuid(
        sqlx::query_scalar(
            "INSERT INTO branches (region_id,name,org_id) VALUES ($1,$2,$3) RETURNING id",
        )
        .bind(region)
        .bind(name)
        .bind(*org.as_uuid())
        .fetch_one(pool)
        .await
        .unwrap(),
    )
}
async fn seed_user(pool: &PgPool, org: OrgId, user: UserId, branch: BranchId) {
    sqlx::query(
        "INSERT INTO users (id,display_name,roles,is_active,org_id) VALUES ($1,$2,$3,true,$4)",
    )
    .bind(*user.as_uuid())
    .bind(format!("equip-{user}"))
    .bind(vec!["MEMBER"])
    .bind(*org.as_uuid())
    .execute(pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO user_branches (user_id,branch_id,org_id) VALUES ($1,$2,$3)")
        .bind(*user.as_uuid())
        .bind(*branch.as_uuid())
        .bind(*org.as_uuid())
        .execute(pool)
        .await
        .unwrap();
}
async fn seed_actor_with_grants(
    pool: &PgPool,
    org: OrgId,
    branch: BranchId,
    features: &[&str],
) -> UserId {
    let user = UserId::new();
    seed_user(pool, org, user, branch).await;
    let role: Uuid = sqlx::query_scalar("INSERT INTO policy_roles (org_id,role_key,display_name,status,is_system,created_by,updated_by) VALUES ($1,$2,$3,'ACTIVE',false,$4,$4) RETURNING id").bind(*org.as_uuid()).bind(format!("equip_{}", Uuid::new_v4().simple())).bind("Equipment 3R pilot operator").bind(*user.as_uuid()).fetch_one(pool).await.unwrap();
    for feature in features {
        sqlx::query("INSERT INTO policy_role_permissions (org_id,role_id,feature_key,permission_level) VALUES ($1,$2,$3,'allow')").bind(*org.as_uuid()).bind(role).bind(feature).execute(pool).await.unwrap();
    }
    sqlx::query("INSERT INTO user_role_assignments (org_id,user_id,role_id,assigned_by) VALUES ($1,$2,$3,$2)").bind(*org.as_uuid()).bind(*user.as_uuid()).bind(role).execute(pool).await.unwrap();
    user
}
