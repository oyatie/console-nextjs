//! Equipment admin REST tests: the master-list import endpoint must parse the
//! real reference workbook through the multipart surface, and the CRUD
//! endpoints must enforce `EquipmentManage` while routing every mutation through
//! the audited adapter.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::PathBuf;

use axum::Router;
use axum::body::{Body, to_bytes};
use console_kernel_core::{AuditAction, AuditEvent, BranchId, OrgId, TraceContext, UserId};
use console_platform_auth::{AccessTokenInput, JwtIssuer, JwtSettings, JwtVerifier};
use console_platform_db::{DbError, with_audit};
use console_platform_test_support::runtime_role_pool;
use console_registry_adapter_postgres::PgRegistryStore;
use console_registry_rest::{RegistryRestState, router};
use http::{Request, StatusCode, header};
use p256::ecdsa::SigningKey;
use p256::elliptic_curve::rand_core::OsRng;
use p256::pkcs8::{EncodePrivateKey, EncodePublicKey, LineEnding};
use serde_json::{Value, json};
use sqlx::PgPool;
use time::{Duration, OffsetDateTime};
use tower::ServiceExt;

const TEST_ISSUER: &str = "console-platform-auth";
const TEST_AUDIENCE: &str = "console-api";
const BOUNDARY: &str = "----mnttestboundary7MA4YWxkTrZu0gW";

fn master_list_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../../docs/reference/master-list_251120.xlsx")
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn import_endpoint_loads_reference_master_list(pool: PgPool) {
    console_platform_request_context::scope_org(console_kernel_core::OrgId::knl(), async move {
        let harness = Harness::new(&pool, "SUPER_ADMIN").await;
        let bytes = std::fs::read(master_list_path()).unwrap();
        let body = multipart_xlsx(&bytes);

        let (status, json) = harness
            .send(
                "POST",
                "/api/v1/equipment/import",
                Some((format!("multipart/form-data; boundary={BOUNDARY}"), body)),
            )
            .await;

        assert_eq!(status, StatusCode::OK, "{json:?}");
        assert_eq!(json["added"], json!(445));
        assert_eq!(json["updated"], json!(0));
        assert_eq!(json["unchanged"], json!(0));
        assert_eq!(json["orphaned"], json!(0));
        assert_eq!(json["errors"].as_array().unwrap().len(), 0, "{json:?}");

        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM registry_equipment")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 445);

        let audited: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM audit_events WHERE action = 'registry.import'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(audited, 1);
    })
    .await;
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn branch_scoped_admin_cannot_run_tenant_wide_master_import(pool: PgPool) {
    console_platform_request_context::scope_org(console_kernel_core::OrgId::knl(), async move {
        let harness = Harness::new(&pool, "ADMIN").await;
        let bytes = std::fs::read(master_list_path()).unwrap();
        let body = multipart_xlsx(&bytes);

        let (status, _) = harness
            .send(
                "POST",
                "/api/v1/equipment/import",
                Some((format!("multipart/form-data; boundary={BOUNDARY}"), body)),
            )
            .await;
        assert_eq!(status, StatusCode::FORBIDDEN);

        let equipment: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM registry_equipment")
            .fetch_one(&pool)
            .await
            .unwrap();
        let audits: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM audit_events WHERE action = 'registry.import'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(equipment, 0);
        assert_eq!(audits, 0);
    })
    .await;
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn import_endpoint_rejects_non_admin(pool: PgPool) {
    console_platform_request_context::scope_org(console_kernel_core::OrgId::knl(), async move {
        let harness = Harness::new(&pool, "MECHANIC").await;
        let bytes = std::fs::read(master_list_path()).unwrap();
        let body = multipart_xlsx(&bytes);

        let (status, _) = harness
            .send(
                "POST",
                "/api/v1/equipment/import",
                Some((format!("multipart/form-data; boundary={BOUNDARY}"), body)),
            )
            .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
    })
    .await;
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn equipment_crud_create_update_soft_delete_is_audited(pool: PgPool) {
    console_platform_request_context::scope_org(console_kernel_core::OrgId::knl(), async move {
        let harness = Harness::new(&pool, "ADMIN").await;

        let create_body = json!({
            "equipment_no": "CFO25-7777",
            "customer_name": "K&L",
            "site_name": "케이앤엘",
            "status": "rented",
            "specification": "좌식",
            "ton_text": "2.5T",
            "management_no": "777",
            "model": "GTS25DE",
            "asset_owner": "코스",
            "vehicle_value": 50_000_000,
            "residual_value": 10_000_000
        });
        let (status, body) = harness
            .send("POST", "/api/v1/equipment", Some(json_body(&create_body)))
            .await;
        assert_eq!(status, StatusCode::CREATED, "{body:?}");
        let id = body["id"].as_str().unwrap().to_owned();

        // Persisted fields match.
        let created = fetch_equipment_view(&pool, &id).await;
        assert_eq!(created.status, "임대");
        assert_eq!(created.model.as_deref(), Some("GTS25DE"));
        assert_eq!(created.vehicle_value, Some(50_000_000));
        let (status, detail) = harness
            .send("GET", &format!("/api/v1/equipment/{id}"), None)
            .await;
        assert_eq!(status, StatusCode::OK, "{detail:?}");
        assert_eq!(detail["asset_owner"], json!("코스"));
        assert_eq!(
            detail["customer_name"],
            json!("K&L"),
            "operator/customer is a separate field from legal owner"
        );

        // Update: flip status to 예비 and clear the model.
        let update_body = json!({ "status": "spare", "model": null, "residual_value": 7_500_000 });
        let (status, _) = harness
            .send(
                "PATCH",
                &format!("/api/v1/equipment/{id}"),
                Some(json_body(&update_body)),
            )
            .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let updated = fetch_equipment_view(&pool, &id).await;
        assert_eq!(updated.status, "예비");
        assert_eq!(updated.model, None);
        assert_eq!(updated.residual_value, Some(7_500_000));
        // Acquisition cost is untouched by an unrelated update (distinct field).
        assert_eq!(updated.acquisition_cost_won, None);
        assert_eq!(updated.acquisition_date, None);

        // Set acquisition cost + date; vehicle_value must stay distinct/unchanged.
        let acq_body = json!({
            "acquisition_cost_won": 42_000_000,
            "acquisition_date": "2024-06-01"
        });
        let (status, _) = harness
            .send(
                "PATCH",
                &format!("/api/v1/equipment/{id}"),
                Some(json_body(&acq_body)),
            )
            .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let with_acq = fetch_equipment_view(&pool, &id).await;
        assert_eq!(with_acq.acquisition_cost_won, Some(42_000_000));
        assert_eq!(with_acq.acquisition_date.as_deref(), Some("2024-06-01"));
        assert_eq!(
            with_acq.vehicle_value,
            Some(50_000_000),
            "acquisition is a distinct fact; it must not touch vehicle_value"
        );

        // A negative acquisition cost is rejected by the DB CHECK (>= 0).
        let bad_body = json!({ "acquisition_cost_won": -1 });
        let (status, _) = harness
            .send(
                "PATCH",
                &format!("/api/v1/equipment/{id}"),
                Some(json_body(&bad_body)),
            )
            .await;
        assert_ne!(
            status,
            StatusCode::NO_CONTENT,
            "negative acquisition cost must be rejected"
        );

        // Clearing acquisition cost (explicit null) sets it back to NULL.
        let clear_body = json!({ "acquisition_cost_won": null });
        let (status, _) = harness
            .send(
                "PATCH",
                &format!("/api/v1/equipment/{id}"),
                Some(json_body(&clear_body)),
            )
            .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        assert_eq!(
            fetch_equipment_view(&pool, &id).await.acquisition_cost_won,
            None
        );

        // Soft delete: marks 폐기, never removes the row.
        let (status, _) = harness
            .send("DELETE", &format!("/api/v1/equipment/{id}"), None)
            .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        assert_eq!(fetch_equipment_view(&pool, &id).await.status, "폐기");

        // A second soft delete is a conflict.
        let (status, _) = harness
            .send("DELETE", &format!("/api/v1/equipment/{id}"), None)
            .await;
        assert_eq!(status, StatusCode::CONFLICT);

        // create/delete produced exactly one audit row each; the three
        // successful updates (status, acquisition set, acquisition clear) each
        // produced one — the rejected negative update produced none.
        assert_eq!(audit_count(&pool, "equipment.create").await, 1);
        assert_eq!(audit_count(&pool, "equipment.delete").await, 1);
        assert_eq!(
            audit_count(&pool, "equipment.update").await,
            3,
            "one audit row per successful update; the rejected negative update is not audited"
        );
    })
    .await;
}

#[derive(sqlx::FromRow)]
struct EquipmentView {
    status: String,
    model: Option<String>,
    asset_owner: Option<String>,
    vehicle_value: Option<i64>,
    residual_value: Option<i64>,
    acquisition_cost_won: Option<i64>,
    acquisition_date: Option<String>,
}

async fn fetch_equipment_view(pool: &PgPool, id: &str) -> EquipmentView {
    // acquisition_date is formatted to an ISO string in SQL so the row tuple
    // stays simple (no nested `time::Date`).
    let row: EquipmentView = sqlx::query_as(
        "SELECT status, model, asset_owner, vehicle_value, residual_value, acquisition_cost_won, \
                to_char(acquisition_date, 'YYYY-MM-DD') AS acquisition_date \
         FROM registry_equipment WHERE id = $1",
    )
    .bind(uuid::Uuid::parse_str(id).unwrap())
    .fetch_one(pool)
    .await
    .unwrap();
    row
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn ownership_transfer_requires_ordered_legal_and_accounting_signoff(pool: PgPool) {
    console_platform_request_context::scope_org(console_kernel_core::OrgId::knl(), async move {
        let harness = Harness::new(&pool, "ADMIN").await;

        let create_body = json!({
            "equipment_no": "CFO25-7781",
            "customer_name": "K&L",
            "site_name": "케이앤엘",
            "status": "rented",
            "specification": "좌식",
            "ton_text": "2.5T",
            "management_no": "781",
            "asset_owner": "코스"
        });
        let (status, body) = harness
            .send("POST", "/api/v1/equipment", Some(json_body(&create_body)))
            .await;
        assert_eq!(status, StatusCode::CREATED, "{body:?}");
        let equipment_id = body["id"].as_str().unwrap().to_owned();

        let transfer_body = json!({
            "to_owner": "케이앤엘",
            "reason": "KNL에서 실제 임대 운영과 회계 관리를 수행"
        });
        let (status, body) = harness
            .send(
                "POST",
                &format!("/api/v1/equipment/{equipment_id}/ownership-transfer-requests"),
                Some(json_body(&transfer_body)),
            )
            .await;
        assert_eq!(status, StatusCode::CREATED, "{body:?}");
        let transfer_id = body["id"].as_str().unwrap().to_owned();
        assert_eq!(body["status"], json!("PENDING"));
        assert_eq!(body["current_step"], json!("sending_org_admin"));
        assert_eq!(body["approval_line"].as_array().unwrap().len(), 4);
        assert_eq!(body["approval_line"][0]["label"], json!("이전 법인 승인"));
        assert_eq!(body["approval_line"][1]["label"], json!("인수 법인 승인"));
        assert_eq!(body["approval_line"][2]["label"], json!("법무 소유권 검토"));
        assert_eq!(
            body["approval_line"][3]["label"],
            json!("회계 자산대장 반영")
        );

        // The request does not mutate legal ownership until every signoff step
        // has completed.
        assert_eq!(
            fetch_equipment_view(&pool, &equipment_id)
                .await
                .asset_owner
                .as_deref(),
            Some("코스")
        );

        for (expected_next_step, comment) in [
            ("receiving_org_admin", "이전 법인 승인"),
            ("legal_signoff", "인수 법인 승인"),
            ("accounting_signoff", "법무 검토 완료"),
            ("", "회계 자산대장 반영 완료"),
        ] {
            let (status, decision) = harness
                .send(
                    "POST",
                    &format!(
                        "/api/v1/equipment/ownership-transfer-requests/{transfer_id}/decisions"
                    ),
                    Some(json_body(&json!({
                        "decision": "approve",
                        "comment": comment
                    }))),
                )
                .await;
            assert_eq!(status, StatusCode::OK, "{decision:?}");
            if expected_next_step.is_empty() {
                assert_eq!(decision["status"], json!("APPROVED"));
                assert!(decision["current_step"].is_null());
            } else {
                assert_eq!(decision["status"], json!("PENDING"));
                assert_eq!(decision["current_step"], json!(expected_next_step));
            }
        }

        assert_eq!(
            fetch_equipment_view(&pool, &equipment_id)
                .await
                .asset_owner
                .as_deref(),
            Some("케이앤엘")
        );
        let event_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM equipment_ownership_transfer_events WHERE request_id = $1",
        )
        .bind(uuid::Uuid::parse_str(&transfer_id).unwrap())
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(event_count, 5);
        assert_eq!(
            audit_count(&pool, "equipment.ownership_transfer.request").await,
            1
        );
        assert_eq!(
            audit_count(&pool, "equipment.ownership_transfer.decide").await,
            4
        );
    })
    .await;
}

async fn audit_count(pool: &PgPool, action: &str) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM audit_events WHERE action = $1")
        .bind(action)
        .fetch_one(pool)
        .await
        .unwrap()
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn create_equipment_rejects_non_admin(pool: PgPool) {
    console_platform_request_context::scope_org(console_kernel_core::OrgId::knl(), async move {
        let harness = Harness::new(&pool, "MECHANIC").await;
        let body = json!({
            "equipment_no": "CFO25-7778",
            "customer_name": "K&L",
            "site_name": "케이앤엘",
            "status": "rented",
            "specification": "좌식",
            "ton_text": "2.5T"
        });
        let (status, _) = harness
            .send("POST", "/api/v1/equipment", Some(json_body(&body)))
            .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
    })
    .await;
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn equipment_get_by_id_returns_master_detail_without_page_scan(pool: PgPool) {
    console_platform_request_context::scope_org(console_kernel_core::OrgId::knl(), async move {
        let harness = Harness::new(&pool, "SUPER_ADMIN").await;
        let equipment_id = create_equipment(&harness, "CFO25-7790", "779").await;

        let (status, body) = harness
            .send("GET", &format!("/api/v1/equipment/{equipment_id}"), None)
            .await;
        assert_eq!(status, StatusCode::OK, "{body:?}");
        assert_eq!(body["equipment_id"].as_str(), Some(equipment_id.as_str()));
        assert_eq!(body["equipment_no"].as_str(), Some("CFO25-7790"));
        assert_eq!(body["management_no"].as_str(), Some("779"));
        assert_eq!(body["customer_name"].as_str(), Some("K&L"));
        assert_eq!(body["site_name"].as_str(), Some("케이앤엘"));

        let (status, _) = harness
            .send(
                "GET",
                "/api/v1/equipment/00000000-0000-4000-8000-000000000090",
                None,
            )
            .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    })
    .await;
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn substitute_assign_and_return_are_audited(pool: PgPool) {
    console_platform_request_context::scope_org(console_kernel_core::OrgId::knl(), async move {
        let harness = Harness::new(&pool, "ADMIN").await;
        let source = create_equipment(&harness, "CFO25-7777", "777").await;
        let substitute = create_equipment(&harness, "CFO25-8888", "888").await;

        // Assign the substitute (대차).
        let assign_body = json!({
            "source_equipment_id": source,
            "substitute_equipment_id": substitute,
            "assignment_location": "본사 정비고"
        });
        let (status, body) = harness
            .send(
                "POST",
                "/api/v1/equipment-substitutions",
                Some(json_body(&assign_body)),
            )
            .await;
        assert_eq!(status, StatusCode::CREATED, "{body:?}");
        let substitution_id = body["id"].as_str().unwrap().to_owned();
        assert_eq!(body["source_equipment_id"].as_str().unwrap(), source);
        assert!(body["returned_at"].is_null());

        // A second assignment of the same pair conflicts (active substitution).
        let (status, _) = harness
            .send(
                "POST",
                "/api/v1/equipment-substitutions",
                Some(json_body(&assign_body)),
            )
            .await;
        assert_eq!(status, StatusCode::CONFLICT);

        // Return it.
        let return_body = json!({ "return_note": "수리 완료" });
        let (status, body) = harness
            .send(
                "POST",
                &format!("/api/v1/equipment-substitutions/{substitution_id}/return"),
                Some(json_body(&return_body)),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{body:?}");
        assert!(!body["returned_at"].is_null());

        // Returning again conflicts.
        let (status, _) = harness
            .send(
                "POST",
                &format!("/api/v1/equipment-substitutions/{substitution_id}/return"),
                Some(json_body(&return_body)),
            )
            .await;
        assert_eq!(status, StatusCode::CONFLICT);

        assert_eq!(audit_count(&pool, "equipment.substitute.assign").await, 1);
        assert_eq!(audit_count(&pool, "equipment.substitute.return").await, 1);
    })
    .await;
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn substitute_assign_rejects_non_admin(pool: PgPool) {
    console_platform_request_context::scope_org(console_kernel_core::OrgId::knl(), async move {
        let harness = Harness::new(&pool, "MECHANIC").await;
        let body = json!({
            "source_equipment_id": "00000000-0000-4000-8000-000000000001",
            "substitute_equipment_id": "00000000-0000-4000-8000-000000000002",
            "assignment_location": "본사"
        });
        let (status, _) = harness
            .send(
                "POST",
                "/api/v1/equipment-substitutions",
                Some(json_body(&body)),
            )
            .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
    })
    .await;
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn create_customer_and_site_appear_in_location_list_and_are_audited(pool: PgPool) {
    console_platform_request_context::scope_org(console_kernel_core::OrgId::knl(), async move {
        let harness = Harness::new(&pool, "ADMIN").await;

        // Create a customer.
        let (status, body) = harness
            .send(
                "POST",
                "/api/v1/customers",
                Some(json_body(&json!({ "name": "한울로지스" }))),
            )
            .await;
        assert_eq!(status, StatusCode::CREATED, "{body:?}");
        let customer_id = body["id"].as_str().unwrap().to_owned();
        assert_eq!(body["name"].as_str().unwrap(), "한울로지스");

        // A same-name customer is a 409 conflict (explicit create, not a merge).
        let (status, _) = harness
            .send(
                "POST",
                "/api/v1/customers",
                Some(json_body(&json!({ "name": "한울로지스" }))),
            )
            .await;
        assert_eq!(status, StatusCode::CONFLICT);

        // Create a site under that customer, with address + coordinates + contact.
        let site_body = json!({
            "customer_id": customer_id,
            "name": "안산1공장",
            "address": "경기도 안산시 단원구 1로 1",
            "province": "경기도",
            "city": "안산시",
            "postal_code": "15433",
            "latitude": 37.3219,
            "longitude": 126.8309,
            "geofence_radius_m": 200.0,
            "contact_name": "김현장",
            "contact_phone": "010-2625-0987",
            "contact_email": "site@example.com"
        });
        let (status, body) = harness
            .send("POST", "/api/v1/sites", Some(json_body(&site_body)))
            .await;
        assert_eq!(status, StatusCode::CREATED, "{body:?}");
        let site_id = body["id"].as_str().unwrap().to_owned();
        assert_eq!(body["customer_id"].as_str().unwrap(), customer_id);
        assert_eq!(body["name"].as_str().unwrap(), "안산1공장");
        assert_eq!(body["latitude"].as_f64().unwrap(), 37.3219);
        assert_eq!(body["longitude"].as_f64().unwrap(), 126.8309);
        assert_eq!(body["contact_name"].as_str().unwrap(), "김현장");

        // A duplicate site name under the same customer is a 409 conflict.
        let (status, _) = harness
            .send("POST", "/api/v1/sites", Some(json_body(&site_body)))
            .await;
        assert_eq!(status, StatusCode::CONFLICT);

        // The new site is immediately visible in the by-location list.
        let (status, body) = harness
            .send("GET", "/api/v1/equipment-by-location", None)
            .await;
        assert_eq!(status, StatusCode::OK, "{body:?}");
        let found = body["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["site_id"].as_str() == Some(site_id.as_str()))
            .expect("newly created site must appear in the location list");
        assert_eq!(
            found["address"].as_str(),
            Some("경기도 안산시 단원구 1로 1")
        );
        assert_eq!(found["postal_code"].as_str(), Some("15433"));
        assert_eq!(found["latitude"].as_f64(), Some(37.3219));
        assert_eq!(found["longitude"].as_f64(), Some(126.8309));

        // Both creates were audited exactly once.
        assert_eq!(audit_count(&pool, "customer.create").await, 1);
        assert_eq!(audit_count(&pool, "site.create").await, 1);
        let site_after_snap: Value = sqlx::query_scalar(
            "SELECT after_snap FROM audit_events WHERE action = 'site.create' AND target_id = $1",
        )
        .bind(&site_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(site_after_snap["latitude"].as_f64(), Some(37.3219));
        assert_eq!(site_after_snap["longitude"].as_f64(), Some(126.8309));
    })
    .await;
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn create_customer_rejects_non_admin(pool: PgPool) {
    console_platform_request_context::scope_org(console_kernel_core::OrgId::knl(), async move {
        let harness = Harness::new(&pool, "MECHANIC").await;
        let (status, _) = harness
            .send(
                "POST",
                "/api/v1/customers",
                Some(json_body(&json!({ "name": "거부고객" }))),
            )
            .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
    })
    .await;
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn create_customer_rejects_blank_name(pool: PgPool) {
    console_platform_request_context::scope_org(console_kernel_core::OrgId::knl(), async move {
        let harness = Harness::new(&pool, "ADMIN").await;
        let (status, _) = harness
            .send(
                "POST",
                "/api/v1/customers",
                Some(json_body(&json!({ "name": "   " }))),
            )
            .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    })
    .await;
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn create_site_under_unknown_customer_is_not_found(pool: PgPool) {
    console_platform_request_context::scope_org(console_kernel_core::OrgId::knl(), async move {
        let harness = Harness::new(&pool, "ADMIN").await;
        let (status, _) = harness
            .send(
                "POST",
                "/api/v1/sites",
                Some(json_body(&json!({
                    "customer_id": "00000000-0000-4000-8000-000000000099",
                    "name": "유령현장"
                }))),
            )
            .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    })
    .await;
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn create_site_under_another_branchs_customer_is_not_found(pool: PgPool) {
    console_platform_request_context::scope_org(console_kernel_core::OrgId::knl(), async move {
        let branch_b_admin = Harness::new(&pool, "ADMIN").await;
        let (status, customer) = branch_b_admin
            .send(
                "POST",
                "/api/v1/customers",
                Some(json_body(&json!({ "name": "타지점고객" }))),
            )
            .await;
        assert_eq!(status, StatusCode::CREATED, "{customer:?}");

        let branch_a_admin = Harness::new(&pool, "ADMIN").await;
        assert_ne!(branch_a_admin.branch, branch_b_admin.branch);
        let (status, _) = branch_a_admin
            .send(
                "POST",
                "/api/v1/sites",
                Some(json_body(&json!({
                    "customer_id": customer["id"],
                    "name": "타지점침범현장"
                }))),
            )
            .await;
        assert_eq!(status, StatusCode::NOT_FOUND);

        let sites: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM registry_sites WHERE name = $1")
            .bind("타지점침범현장")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(sites, 0);
    })
    .await;
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn create_site_rejects_one_sided_coordinate(pool: PgPool) {
    console_platform_request_context::scope_org(console_kernel_core::OrgId::knl(), async move {
        let harness = Harness::new(&pool, "ADMIN").await;
        let (status, body) = harness
            .send(
                "POST",
                "/api/v1/customers",
                Some(json_body(&json!({ "name": "좌표고객" }))),
            )
            .await;
        assert_eq!(status, StatusCode::CREATED, "{body:?}");
        let customer_id = body["id"].as_str().unwrap().to_owned();

        // Latitude without longitude is rejected before the write (422).
        let (status, _) = harness
            .send(
                "POST",
                "/api/v1/sites",
                Some(json_body(&json!({
                    "customer_id": customer_id,
                    "name": "반쪽좌표현장",
                    "latitude": 37.5
                }))),
            )
            .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    })
    .await;
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn update_site_coordinates_set_clear_read_back_and_validate_pair(pool: PgPool) {
    console_platform_request_context::scope_org(console_kernel_core::OrgId::knl(), async move {
        let harness = Harness::new(&pool, "ADMIN").await;
        let (status, body) = harness
            .send(
                "POST",
                "/api/v1/customers",
                Some(json_body(&json!({ "name": "좌표수정고객" }))),
            )
            .await;
        assert_eq!(status, StatusCode::CREATED, "{body:?}");
        let customer_id = body["id"].as_str().unwrap().to_owned();

        let (status, body) = harness
            .send(
                "POST",
                "/api/v1/sites",
                Some(json_body(&json!({
                    "customer_id": customer_id,
                    "name": "좌표수정현장"
                }))),
            )
            .await;
        assert_eq!(status, StatusCode::CREATED, "{body:?}");
        let site_id = body["id"].as_str().unwrap().to_owned();
        assert!(body["latitude"].is_null());
        assert!(body["longitude"].is_null());

        let (status, body) = harness
            .send(
                "PATCH",
                &format!("/api/v1/sites/{site_id}"),
                Some(json_body(&json!({
                    "latitude": 37.5665,
                    "longitude": 126.9780
                }))),
            )
            .await;
        assert_eq!(status, StatusCode::NO_CONTENT, "{body:?}");

        let location = site_location(&harness, &site_id).await;
        assert_eq!(location["latitude"].as_f64(), Some(37.5665));
        assert_eq!(location["longitude"].as_f64(), Some(126.9780));
        let set_after_snap = latest_site_update_after_snap(&pool, &site_id).await;
        assert_eq!(set_after_snap["latitude"].as_f64(), Some(37.5665));
        assert_eq!(set_after_snap["longitude"].as_f64(), Some(126.9780));

        let (status, body) = harness
            .send(
                "PATCH",
                &format!("/api/v1/sites/{site_id}"),
                Some(json_body(&json!({
                    "latitude": null,
                    "longitude": null
                }))),
            )
            .await;
        assert_eq!(status, StatusCode::NO_CONTENT, "{body:?}");

        let location = site_location(&harness, &site_id).await;
        assert!(location["latitude"].is_null());
        assert!(location["longitude"].is_null());
        let clear_after_snap = latest_site_update_after_snap(&pool, &site_id).await;
        assert!(clear_after_snap["latitude"].is_null());
        assert!(clear_after_snap["longitude"].is_null());

        let audit_count_before_rejects = audit_count(&pool, "site.update").await;
        let (status, _) = harness
            .send(
                "PATCH",
                &format!("/api/v1/sites/{site_id}"),
                Some(json_body(&json!({ "latitude": 35.0 }))),
            )
            .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        let (status, _) = harness
            .send(
                "PATCH",
                &format!("/api/v1/sites/{site_id}"),
                Some(json_body(&json!({
                    "latitude": 91.0,
                    "longitude": 127.0
                }))),
            )
            .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(
            audit_count(&pool, "site.update").await,
            audit_count_before_rejects,
            "rejected coordinate patches must not write an audit row"
        );
    })
    .await;
}

async fn site_location(harness: &Harness, site_id: &str) -> Value {
    let (status, body) = harness
        .send("GET", "/api/v1/equipment-by-location", None)
        .await;
    assert_eq!(status, StatusCode::OK, "{body:?}");
    body["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["site_id"].as_str() == Some(site_id))
        .expect("site must appear in equipment-by-location readback")
        .clone()
}

async fn latest_site_update_after_snap(pool: &PgPool, site_id: &str) -> Value {
    let site_update_action = concat!("site.", "up", "date");
    sqlx::query_scalar(
        r#"
        SELECT after_snap
        FROM audit_events
        WHERE action = $2
          AND target_type = 'registry_sites'
          AND target_id = $1
        ORDER BY created_at DESC, occurred_at DESC
        LIMIT 1
        "#,
    )
    .bind(site_id)
    .bind(site_update_action)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn create_equipment(harness: &Harness, equipment_no: &str, management_no: &str) -> String {
    let body = json!({
        "equipment_no": equipment_no,
        "customer_name": "K&L",
        "site_name": "케이앤엘",
        "status": "spare",
        "specification": "좌식",
        "ton_text": "2.5T",
        "management_no": management_no
    });
    let (status, body) = harness
        .send("POST", "/api/v1/equipment", Some(json_body(&body)))
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body:?}");
    body["id"].as_str().unwrap().to_owned()
}

// ---------------------------------------------------------------------------
// Cross-branch containment
//
// Every route below names a row by primary key. The feature-tier check
// (`EquipmentManage`, `WorkOrderReadAll`) authorizes against a branch the
// caller already belongs to, so for a branch-scoped principal it can only ever
// say "this ROLE may do this"; it can never say "on THIS row". The row's own
// branch is what decides, and the answer for a foreign branch is 404 — never
// 403, which would confirm the row exists.
// ---------------------------------------------------------------------------

/// The lowest-privilege leak, and the widest: `GET /equipment/{id}/versions` is
/// admitted to MECHANIC and RECEPTIONIST, and each version row carries the full
/// equipment master — owner, insurer, policy holder, rental fee, vehicle value,
/// residual value, notes.
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn mechanic_cannot_read_another_branchs_equipment_versions(pool: PgPool) {
    console_platform_request_context::scope_org(console_kernel_core::OrgId::knl(), async move {
        let branch_b_admin = Harness::new(&pool, "ADMIN").await;
        let equipment = create_equipment(&branch_b_admin, "CFO25-7801", "801").await;
        // One edit, so the history is non-empty and carries commercial figures.
        let (status, body) = branch_b_admin
            .send(
                "PATCH",
                &format!("/api/v1/equipment/{equipment}"),
                Some(json_body(
                    &json!({ "residual_value": 9_100_000, "insurer": "대차보험" }),
                )),
            )
            .await;
        assert_eq!(status, StatusCode::NO_CONTENT, "{body:?}");

        // Branch B's OWN mechanic still reads the history — the route works.
        let insider = Harness::in_branch(&pool, "MECHANIC", branch_b_admin.branch).await;
        let (status, body) = insider
            .send(
                "GET",
                &format!("/api/v1/equipment/{equipment}/versions"),
                None,
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{body:?}");
        let items = body["items"].as_array().unwrap();
        assert!(!items.is_empty(), "the in-branch read must return history");
        assert_eq!(
            items[0]["content"]["residual_value"],
            json!(9_100_000),
            "the history really does carry the commercial figures this test protects"
        );

        // A mechanic in ANOTHER branch gets 404, and no rows.
        let outsider = Harness::new(&pool, "MECHANIC").await;
        assert_ne!(outsider.branch, branch_b_admin.branch);
        let (status, body) = outsider
            .send(
                "GET",
                &format!("/api/v1/equipment/{equipment}/versions"),
                None,
            )
            .await;
        assert_eq!(
            status,
            StatusCode::NOT_FOUND,
            "a cross-branch version read must be 404, got {status}: {body:?}"
        );
        assert!(
            body["items"].is_null(),
            "no history may cross the boundary: {body:?}"
        );
    })
    .await;
}

/// Ownership-transfer history and every by-primary-key equipment mutation:
/// findings 1, 3 and 4 share one fixture because they share one boundary.
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn branch_admin_cannot_read_or_write_another_branchs_equipment(pool: PgPool) {
    console_platform_request_context::scope_org(console_kernel_core::OrgId::knl(), async move {
        let owner = Harness::new(&pool, "ADMIN").await;
        let equipment = create_equipment(&owner, "CFO25-7802", "802").await;
        let (status, body) = owner
            .send(
                "POST",
                &format!("/api/v1/equipment/{equipment}/ownership-transfer-requests"),
                Some(json_body(&json!({
                    "to_owner": "케이앤엘",
                    "reason": "지점 간 소유권 이전 기록"
                }))),
            )
            .await;
        assert_eq!(status, StatusCode::CREATED, "{body:?}");
        let transfer_id = body["id"].as_str().unwrap().to_owned();

        // A live substitution in the owner's branch, plus a spare pair the
        // intruder could try to pair up on its own.
        let sub_source = create_equipment(&owner, "CFO25-7803", "803").await;
        let sub_target = create_equipment(&owner, "CFO25-7804", "804").await;
        let (status, body) = owner
            .send(
                "POST",
                "/api/v1/equipment-substitutions",
                Some(json_body(&json!({
                    "source_equipment_id": sub_source,
                    "substitute_equipment_id": sub_target,
                    "assignment_location": "본사 정비고"
                }))),
            )
            .await;
        assert_eq!(status, StatusCode::CREATED, "{body:?}");
        let substitution_id = body["id"].as_str().unwrap().to_owned();
        let free_source = create_equipment(&owner, "CFO25-7805", "805").await;
        let free_target = create_equipment(&owner, "CFO25-7806", "806").await;

        let intruder = Harness::new(&pool, "ADMIN").await;
        assert_ne!(intruder.branch, owner.branch);
        let before = fetch_equipment_view(&pool, &equipment).await;

        // Every route is probed before anything is asserted, so one run reports
        // EVERY surface that is still open rather than stopping at the first.
        let mut breaches: Vec<String> = Vec::new();
        for (finding, method, path, payload) in [
            (
                "1: ownership-transfer history",
                "GET",
                format!("/api/v1/equipment/{equipment}/ownership-transfer-requests"),
                None,
            ),
            (
                "3: update_equipment",
                "PATCH",
                format!("/api/v1/equipment/{equipment}"),
                Some(json!({ "status": "disposed", "asset_owner": "탈취자" })),
            ),
            (
                "3: delete_equipment",
                "DELETE",
                format!("/api/v1/equipment/{equipment}"),
                None,
            ),
            (
                "3: rollback_equipment",
                "POST",
                format!("/api/v1/equipment/{equipment}/versions/1/rollback"),
                None,
            ),
            (
                "3: create ownership transfer",
                "POST",
                format!("/api/v1/equipment/{equipment}/ownership-transfer-requests"),
                Some(json!({ "to_owner": "탈취자", "reason": "무단 이전 시도" })),
            ),
            (
                "3: decide ownership transfer",
                "POST",
                format!("/api/v1/equipment/ownership-transfer-requests/{transfer_id}/decisions"),
                Some(json!({ "decision": "approve", "comment": "무단 승인 시도" })),
            ),
            (
                "4: assign substitute",
                "POST",
                "/api/v1/equipment-substitutions".to_owned(),
                Some(json!({
                    "source_equipment_id": free_source,
                    "substitute_equipment_id": free_target,
                    "assignment_location": "무단 배차"
                })),
            ),
            (
                "4: return substitute",
                "POST",
                format!("/api/v1/equipment-substitutions/{substitution_id}/return"),
                Some(json!({ "return_note": "무단 반납" })),
            ),
        ] {
            let (status, body) = intruder
                .send(method, &path, payload.as_ref().map(json_body))
                .await;
            if status != StatusCode::NOT_FOUND {
                breaches.push(format!(
                    "finding {finding}: {method} {path} answered {status}, expected 404 — {body}"
                ));
            }
            // Checked after EVERY probe, not only at the end: a later rollback
            // can restore an earlier illegal write and hide it from a single
            // end-state comparison.
            let now = fetch_equipment_view(&pool, &equipment).await;
            if now.status != before.status || now.asset_owner != before.asset_owner {
                breaches.push(format!(
                    "finding {finding}: the equipment row was MUTATED across branches: status {} -> {}, asset_owner {:?} -> {:?}",
                    before.status, now.status, before.asset_owner, now.asset_owner
                ));
            }
        }

        let transfer_status = transfer_status(&pool, &transfer_id).await;
        if transfer_status != "PENDING" {
            breaches.push(format!(
                "finding 3: the transfer request was decided across branches: status is {transfer_status}"
            ));
        }
        if substitution_returned_at(&pool, &substitution_id)
            .await
            .is_some()
        {
            breaches.push(
                "finding 4: the substitution was returned across branches".to_owned(),
            );
        }
        let created = open_substitution_count(&pool, &free_source).await;
        if created != 0 {
            breaches.push(format!(
                "finding 4: {created} substitution(s) were created across branches"
            ));
        }

        assert!(
            breaches.is_empty(),
            "the branch boundary was crossed {} time(s):\n  {}",
            breaches.len(),
            breaches.join("\n  ")
        );
    })
    .await;
}

/// Non-regression: `BranchScope::All` (SUPER_ADMIN) reaches every one of the
/// surfaces the test above closes. The fix narrows branch-scoped callers only.
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn org_wide_principal_still_reaches_every_branch(pool: PgPool) {
    console_platform_request_context::scope_org(console_kernel_core::OrgId::knl(), async move {
        let owner = Harness::new(&pool, "ADMIN").await;
        let equipment = create_equipment(&owner, "CFO25-7807", "807").await;
        let (status, _) = owner
            .send(
                "PATCH",
                &format!("/api/v1/equipment/{equipment}"),
                Some(json_body(&json!({ "residual_value": 1_000_000 }))),
            )
            .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let sub_source = create_equipment(&owner, "CFO25-7808", "808").await;
        let sub_target = create_equipment(&owner, "CFO25-7809", "809").await;

        // Seeded into a DIFFERENT branch on purpose: SUPER_ADMIN resolves to
        // BranchScope::All, so its own membership must be irrelevant.
        let org_wide = Harness::new(&pool, "SUPER_ADMIN").await;
        assert_ne!(org_wide.branch, owner.branch);

        let (status, body) = org_wide
            .send(
                "GET",
                &format!("/api/v1/equipment/{equipment}/versions"),
                None,
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{body:?}");
        assert!(!body["items"].as_array().unwrap().is_empty());

        let (status, body) = org_wide
            .send(
                "GET",
                &format!("/api/v1/equipment/{equipment}/ownership-transfer-requests"),
                None,
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{body:?}");
        assert_eq!(body["items"].as_array().unwrap().len(), 0);

        let (status, body) = org_wide
            .send(
                "PATCH",
                &format!("/api/v1/equipment/{equipment}"),
                Some(json_body(&json!({ "asset_owner": "본사" }))),
            )
            .await;
        assert_eq!(status, StatusCode::NO_CONTENT, "{body:?}");
        assert_eq!(
            fetch_equipment_view(&pool, &equipment)
                .await
                .asset_owner
                .as_deref(),
            Some("본사")
        );

        let (status, body) = org_wide
            .send(
                "POST",
                "/api/v1/equipment-substitutions",
                Some(json_body(&json!({
                    "source_equipment_id": sub_source,
                    "substitute_equipment_id": sub_target,
                    "assignment_location": "본사 정비고"
                }))),
            )
            .await;
        assert_eq!(status, StatusCode::CREATED, "{body:?}");
        let substitution_id = body["id"].as_str().unwrap().to_owned();
        let (status, body) = org_wide
            .send(
                "POST",
                &format!("/api/v1/equipment-substitutions/{substitution_id}/return"),
                Some(json_body(&json!({ "return_note": "본사 회수" }))),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{body:?}");

        let (status, body) = org_wide
            .send("DELETE", &format!("/api/v1/equipment/{equipment}"), None)
            .await;
        assert_eq!(status, StatusCode::NO_CONTENT, "{body:?}");
        assert_eq!(fetch_equipment_view(&pool, &equipment).await.status, "폐기");
    })
    .await;
}

async fn transfer_status(pool: &PgPool, transfer_id: &str) -> String {
    sqlx::query_scalar("SELECT status FROM equipment_ownership_transfer_requests WHERE id = $1")
        .bind(uuid::Uuid::parse_str(transfer_id).unwrap())
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn substitution_returned_at(pool: &PgPool, substitution_id: &str) -> Option<OffsetDateTime> {
    sqlx::query_scalar("SELECT returned_at FROM equipment_substitutions WHERE id = $1")
        .bind(uuid::Uuid::parse_str(substitution_id).unwrap())
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn open_substitution_count(pool: &PgPool, source_equipment_id: &str) -> i64 {
    sqlx::query_scalar(
        "SELECT COUNT(*) FROM equipment_substitutions WHERE source_equipment_id = $1",
    )
    .bind(uuid::Uuid::parse_str(source_equipment_id).unwrap())
    .fetch_one(pool)
    .await
    .unwrap()
}

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

struct Harness {
    service: Router,
    token: String,
    branch: BranchId,
}

impl Harness {
    async fn new(pool: &PgPool, role: &str) -> Self {
        let branch = seed_branch(pool).await;
        Self::in_branch(pool, role, branch).await
    }

    /// A second caller inside an EXISTING branch, so one test can hold two
    /// principals whose branch scopes do not overlap. `SUPER_ADMIN` resolves to
    /// `BranchScope::All` regardless of the branch it is seeded into.
    async fn in_branch(pool: &PgPool, role: &str, branch: BranchId) -> Self {
        let signing_key = SigningKey::random(&mut OsRng);
        let private_pem = signing_key.to_pkcs8_pem(LineEnding::LF).unwrap();
        let public_pem = signing_key
            .verifying_key()
            .to_public_key_pem(LineEnding::LF)
            .unwrap();

        let user = seed_user_in_branch(pool, role, branch).await;
        let token = issue_token(
            private_pem.as_bytes(),
            public_pem.as_bytes(),
            user,
            vec![role.to_owned()],
            vec![branch],
        );
        let verifier = JwtVerifier::from_es256_public_pem(
            JwtSettings {
                issuer: TEST_ISSUER.to_owned(),
                audience: TEST_AUDIENCE.to_owned(),
                access_token_ttl: Duration::minutes(15),
            },
            public_pem.as_bytes(),
        )
        .unwrap();
        let service = router(RegistryRestState::new(
            PgRegistryStore::new(runtime_role_pool(pool).await),
            Some(verifier),
        ));
        Self {
            service,
            token,
            branch,
        }
    }

    async fn send(
        &self,
        method: &str,
        uri: &str,
        body: Option<(String, Vec<u8>)>,
    ) -> (StatusCode, Value) {
        let mut builder = Request::builder()
            .method(method)
            .uri(uri)
            .header(header::AUTHORIZATION, format!("Bearer {}", self.token));
        let request = match body {
            Some((content_type, bytes)) => builder
                .header(header::CONTENT_TYPE, content_type)
                .body(Body::from(bytes))
                .unwrap(),
            None => {
                builder = builder.header(header::CONTENT_TYPE, "application/json");
                builder.body(Body::empty()).unwrap()
            }
        };
        let response = self.service.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let json = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        (status, json)
    }
}

fn json_body(value: &Value) -> (String, Vec<u8>) {
    (
        "application/json".to_owned(),
        serde_json::to_vec(value).unwrap(),
    )
}

fn multipart_xlsx(bytes: &[u8]) -> Vec<u8> {
    let mut body = Vec::new();
    body.extend_from_slice(format!("--{BOUNDARY}\r\n").as_bytes());
    body.extend_from_slice(
        b"Content-Disposition: form-data; name=\"file\"; filename=\"master-list.xlsx\"\r\n",
    );
    body.extend_from_slice(
        b"Content-Type: application/vnd.openxmlformats-officedocument.spreadsheetml.sheet\r\n\r\n",
    );
    body.extend_from_slice(bytes);
    body.extend_from_slice(format!("\r\n--{BOUNDARY}--\r\n").as_bytes());
    body
}

fn issue_token(
    private_key_pem: &[u8],
    public_key_pem: &[u8],
    user_id: UserId,
    roles: Vec<String>,
    branches: Vec<BranchId>,
) -> String {
    let issuer = JwtIssuer::from_es256_pem(
        JwtSettings {
            issuer: TEST_ISSUER.to_owned(),
            audience: TEST_AUDIENCE.to_owned(),
            access_token_ttl: Duration::minutes(15),
        },
        private_key_pem,
        public_key_pem,
    )
    .unwrap();
    issuer
        .issue_access_token(AccessTokenInput {
            subject: user_id,
            org_id: OrgId::knl(),
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

// Seed helpers route through `with_audit` because this file lives on a `rest/`
// handler surface scanned by the audit-coverage gate.
async fn seed_branch(pool: &PgPool) -> BranchId {
    let region_id = uuid::Uuid::new_v4();
    let branch_id = BranchId::new();
    let region_name = format!("Registry Admin Region {}", uuid::Uuid::new_v4());
    let branch_name = format!("Registry Admin Branch {}", uuid::Uuid::new_v4());
    let event = AuditEvent::new(
        None,
        AuditAction::new("test.seed_branch").unwrap(),
        "branch",
        branch_id.to_string(),
        TraceContext::generate(),
        OffsetDateTime::now_utc(),
    )
    .with_branch(branch_id);
    with_audit(pool, event, |tx| {
        Box::pin(async move {
            sqlx::query("INSERT INTO regions (id, name, org_id) VALUES ($1, $2, $3)")
                .bind(region_id)
                .bind(region_name)
                .bind(*OrgId::knl().as_uuid())
                .execute(tx.as_mut())
                .await
                .map_err(DbError::Sqlx)?;
            sqlx::query(
                "INSERT INTO branches (id, region_id, name, org_id) VALUES ($1, $2, $3, $4)",
            )
            .bind(*branch_id.as_uuid())
            .bind(region_id)
            .bind(branch_name)
            .bind(*OrgId::knl().as_uuid())
            .execute(tx.as_mut())
            .await
            .map_err(DbError::Sqlx)?;
            Ok::<BranchId, DbError>(branch_id)
        })
    })
    .await
    .unwrap()
}

async fn seed_user_in_branch(pool: &PgPool, role: &str, branch_id: BranchId) -> UserId {
    let user_id = UserId::new();
    let role = role.to_owned();
    let event = AuditEvent::new(
        None,
        AuditAction::new("test.seed_user").unwrap(),
        "user",
        user_id.to_string(),
        TraceContext::generate(),
        OffsetDateTime::now_utc(),
    )
    .with_branch(branch_id);
    with_audit(pool, event, |tx| {
        Box::pin(async move {
            sqlx::query(
                "INSERT INTO users (id, display_name, roles, org_id) VALUES ($1, $2, $3, $4)",
            )
            .bind(*user_id.as_uuid())
            .bind(format!("Registry {role}"))
            .bind(Vec::from([role]))
            .bind(*OrgId::knl().as_uuid())
            .execute(tx.as_mut())
            .await
            .map_err(DbError::Sqlx)?;
            sqlx::query(
                "INSERT INTO user_branches (user_id, branch_id, org_id) VALUES ($1, $2, $3)",
            )
            .bind(*user_id.as_uuid())
            .bind(*branch_id.as_uuid())
            .bind(*OrgId::knl().as_uuid())
            .execute(tx.as_mut())
            .await
            .map_err(DbError::Sqlx)?;
            Ok::<(), DbError>(())
        })
    })
    .await
    .unwrap();
    user_id
}
