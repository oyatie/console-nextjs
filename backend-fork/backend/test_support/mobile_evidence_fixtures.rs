use console_kernel_core::{BranchId, UserId, WorkOrderId};
use sqlx::PgPool;

/// Seed a sync row that was claimed (IN_PROGRESS) but never completed — the
/// state a worker crash between the business-mutation commit and the
/// completion mark leaves behind. The stored payload_hash matches what the REST
/// layer computes (sha256 of the canonical envelope with an RFC3339
/// client_created_at), so a retry of the same operation reconciles it.
#[allow(clippy::too_many_arguments)]
pub async fn seed_crashed_sync_request(
    pool: &PgPool,
    user_id: UserId,
    device_hash: &str,
    request_id: &str,
    sync_id: &str,
    operation_type: &str,
    client_created_at: time::OffsetDateTime,
    payload: &serde_json::Value,
) {
    use sha2::{Digest, Sha256};
    use time::format_description::well_known::Rfc3339;

    let created_at_rfc = client_created_at.format(&Rfc3339).unwrap();
    let envelope = serde_json::json!({
        "user_id": user_id.to_string(),
        "sync_id": sync_id,
        "operation_type": operation_type,
        "client_created_at": created_at_rfc,
        "payload": payload,
    });
    let payload_hash = hex::encode(Sha256::digest(serde_json::to_vec(&envelope).unwrap()));
    sqlx::query(
        r#"
        INSERT INTO offline_sync_requests (
            user_id, device_hash, request_id, sync_id, operation_type,
            client_created_at, status, payload_hash, request_payload, org_id
        )
        VALUES ($1, $2, $3, $4, $5, $6, 'IN_PROGRESS', $7, $8,
                '00000000-0000-0000-0000-0000000000a1')
        "#,
    )
    .bind(*user_id.as_uuid())
    .bind(device_hash)
    .bind(request_id)
    .bind(sync_id)
    .bind(operation_type)
    .bind(client_created_at)
    .bind(&payload_hash)
    .bind(&envelope)
    .execute(pool)
    .await
    .unwrap();
}

pub async fn seed_branch(pool: &PgPool, region_name: &str, branch_name: &str) -> BranchId {
    let region_id: uuid::Uuid =
        sqlx::query_scalar("INSERT INTO regions (name, org_id) VALUES ($1, '00000000-0000-0000-0000-0000000000a1') RETURNING id")
            .bind(region_name)
            .fetch_one(pool)
            .await
            .unwrap();
    let branch_id: uuid::Uuid =
        sqlx::query_scalar("INSERT INTO branches (region_id, name, org_id) VALUES ($1, $2, '00000000-0000-0000-0000-0000000000a1') RETURNING id")
            .bind(region_id)
            .bind(branch_name)
            .fetch_one(pool)
            .await
            .unwrap();
    BranchId::from_uuid(branch_id)
}

pub async fn seed_user_with_branch(
    pool: &PgPool,
    user_id: UserId,
    role: &str,
    branch_id: BranchId,
) {
    sqlx::query("INSERT INTO users (id, display_name, roles, org_id) VALUES ($1, $2, $3, '00000000-0000-0000-0000-0000000000a1')")
        .bind(*user_id.as_uuid())
        .bind(format!("Evidence API {role}"))
        .bind(Vec::from([role]))
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO user_branches (user_id, branch_id, org_id) VALUES ($1, $2, '00000000-0000-0000-0000-0000000000a1')")
        .bind(*user_id.as_uuid())
        .bind(*branch_id.as_uuid())
        .execute(pool)
        .await
        .unwrap();
}

/// A user with NO `user_branches` row, i.e. a principal whose live branch scope
/// resolves to `BranchScope::Branches({})` — belonging to no branch in the org.
/// Only meaningful for a role that is not `SUPER_ADMIN`/`EXECUTIVE`, since those
/// short-circuit to `BranchScope::All` before the membership read.
pub async fn seed_user_without_branch(pool: &PgPool, user_id: UserId, role: &str) {
    sqlx::query("INSERT INTO users (id, display_name, roles, org_id) VALUES ($1, $2, $3, '00000000-0000-0000-0000-0000000000a1')")
        .bind(*user_id.as_uuid())
        .bind(format!("Branchless {role}"))
        .bind(Vec::from([role]))
        .execute(pool)
        .await
        .unwrap();
}

pub async fn seed_equipment(pool: &PgPool, branch_id: BranchId, management_no: &str) -> uuid::Uuid {
    let equipment_suffix = format!("{:0>4}", management_no);
    let customer_id: uuid::Uuid = sqlx::query_scalar(
        "INSERT INTO registry_customers (branch_id, name, org_id) VALUES ($1, $2, '00000000-0000-0000-0000-0000000000a1') RETURNING id",
    )
    .bind(*branch_id.as_uuid())
    .bind(format!("Evidence Customer {management_no}"))
    .fetch_one(pool)
    .await
    .unwrap();
    let site_id: uuid::Uuid = sqlx::query_scalar(
        "INSERT INTO registry_sites (branch_id, customer_id, name, org_id) VALUES ($1, $2, $3, '00000000-0000-0000-0000-0000000000a1') RETURNING id",
    )
    .bind(*branch_id.as_uuid())
    .bind(customer_id)
    .bind(format!("Evidence Site {management_no}"))
    .fetch_one(pool)
    .await
    .unwrap();
    sqlx::query_scalar(
        r#"
        INSERT INTO registry_equipment (
            branch_id, customer_id, site_id, equipment_no, management_no,
            manufacturer_code, kind_code, power_code, status,
            specification, ton_text, model, source_sheet, source_row, org_id
        )
        VALUES ($1, $2, $3, $4, $5,
                'A', 'B', 'C', '임대', '좌식', '2.5', 'GTS25DE', 'test', 1,
                '00000000-0000-0000-0000-0000000000a1')
        RETURNING id
        "#,
    )
    .bind(*branch_id.as_uuid())
    .bind(customer_id)
    .bind(site_id)
    .bind(format!("ABC12-{equipment_suffix}"))
    .bind(management_no)
    .fetch_one(pool)
    .await
    .unwrap()
}

pub async fn seed_assigned_work_order(
    pool: &PgPool,
    branch_id: BranchId,
    equipment_id: uuid::Uuid,
    receptionist: UserId,
    mechanic: UserId,
) -> WorkOrderId {
    seed_work_order_with_status(
        pool,
        branch_id,
        equipment_id,
        receptionist,
        mechanic,
        "20260612-802",
        "ASSIGNED",
    )
    .await
}

pub async fn seed_terminal_work_order(
    pool: &PgPool,
    branch_id: BranchId,
    equipment_id: uuid::Uuid,
    receptionist: UserId,
    mechanic: UserId,
    status: &str,
) -> WorkOrderId {
    seed_work_order_with_status(
        pool,
        branch_id,
        equipment_id,
        receptionist,
        mechanic,
        "20260612-803",
        status,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn seed_work_order_with_status(
    pool: &PgPool,
    branch_id: BranchId,
    equipment_id: uuid::Uuid,
    receptionist: UserId,
    mechanic: UserId,
    request_no: &str,
    status: &str,
) -> WorkOrderId {
    let work_order_id = WorkOrderId::new();
    sqlx::query(
        r#"
        INSERT INTO work_orders (
            id, request_no, branch_id, equipment_id, customer_id, site_id,
            requested_by, status, priority, symptom, org_id
        )
        SELECT $1, $5, $2, e.id, e.customer_id, e.site_id,
               $3, $6, 'UNSET', 'Evidence mobile fixture', e.org_id
        FROM registry_equipment e
        WHERE e.id = $4
        "#,
    )
    .bind(*work_order_id.as_uuid())
    .bind(*branch_id.as_uuid())
    .bind(*receptionist.as_uuid())
    .bind(equipment_id)
    .bind(request_no)
    .bind(status)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO work_order_assignments (work_order_id, mechanic_id, role, assigned_at, org_id)
        VALUES ($1, $2, 'PRIMARY', now(), '00000000-0000-0000-0000-0000000000a1')
        "#,
    )
    .bind(*work_order_id.as_uuid())
    .bind(*mechanic.as_uuid())
    .execute(pool)
    .await
    .unwrap();
    work_order_id
}
