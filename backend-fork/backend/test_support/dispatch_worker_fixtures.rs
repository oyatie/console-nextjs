use console_kernel_core::{BranchId, UserId, WorkOrderId};
use sqlx::PgPool;
use time::macros::datetime;

#[derive(Debug)]
pub struct SeededDispatchContext {
    pub receptionist: UserId,
    pub manager: UserId,
    pub near_mechanic: UserId,
    pub work_order_id: WorkOrderId,
}

pub async fn seed_dispatch_context(pool: &PgPool) -> SeededDispatchContext {
    let branch_id = seed_branch(pool).await;
    let receptionist = seed_user(pool, "Worker Receptionist", "RECEPTIONIST", branch_id).await;
    let manager = seed_user(pool, "Worker Manager", "ADMIN", branch_id).await;
    let near_mechanic = seed_user(pool, "Worker Near Mechanic", "MECHANIC", branch_id).await;
    let far_mechanic = seed_user(pool, "Worker Far Mechanic", "MECHANIC", branch_id).await;
    seed_device(pool, manager).await;
    seed_device(pool, near_mechanic).await;
    seed_device(pool, far_mechanic).await;
    seed_location(pool, branch_id, near_mechanic, 37.5652, 126.9897).await;
    seed_location(pool, branch_id, far_mechanic, 37.4979, 127.0276).await;
    let work_order_id = seed_work_order(pool, branch_id, receptionist).await;

    SeededDispatchContext {
        receptionist,
        manager,
        near_mechanic,
        work_order_id,
    }
}

async fn seed_branch(pool: &PgPool) -> BranchId {
    let region_id: uuid::Uuid =
        sqlx::query_scalar("INSERT INTO regions (name, org_id) VALUES ($1, '00000000-0000-0000-0000-0000000000a1') RETURNING id")
            .bind(format!("Worker Region {}", uuid::Uuid::new_v4()))
            .fetch_one(pool)
            .await
            .unwrap();
    let branch_id: uuid::Uuid =
        sqlx::query_scalar("INSERT INTO branches (region_id, name, org_id) VALUES ($1, $2, '00000000-0000-0000-0000-0000000000a1') RETURNING id")
            .bind(region_id)
            .bind("Worker Branch")
            .fetch_one(pool)
            .await
            .unwrap();
    BranchId::from_uuid(branch_id)
}

async fn seed_user(pool: &PgPool, name: &str, role: &str, branch_id: BranchId) -> UserId {
    let user_id = UserId::new();
    sqlx::query("INSERT INTO users (id, display_name, phone, roles, org_id) VALUES ($1, $2, $3, $4, '00000000-0000-0000-0000-0000000000a1')")
        .bind(*user_id.as_uuid())
        .bind(name)
        .bind(format!("010{}", &user_id.to_string()[..8]))
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
    user_id
}

async fn seed_device(pool: &PgPool, user_id: UserId) {
    sqlx::query(
        r#"
        INSERT INTO registered_devices (
            user_id, device_hash, platform, push_token, app_version,
            last_registered_at, created_at, updated_at, org_id
        )
        VALUES ($1, $2, 'ANDROID', $3, '1.0.0', now(), now(), now(),
                '00000000-0000-0000-0000-0000000000a1')
        "#,
    )
    .bind(*user_id.as_uuid())
    .bind(format!("{:064x}", user_id.as_uuid().as_u128()))
    .bind(format!("push-token-{user_id}"))
    .execute(pool)
    .await
    .unwrap();
}

async fn seed_location(
    pool: &PgPool,
    branch_id: BranchId,
    user_id: UserId,
    latitude: f64,
    longitude: f64,
) {
    let now = datetime!(2026-06-12 08:59 UTC);
    sqlx::query(
        r#"
        INSERT INTO location_consents (
            user_id, branch_id, status, granted_at, updated_at, org_id
        )
        VALUES ($1, $2, 'GRANTED', $3, $3, '00000000-0000-0000-0000-0000000000a1')
        "#,
    )
    .bind(*user_id.as_uuid())
    .bind(*branch_id.as_uuid())
    .bind(now)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query_scalar::<_, String>("SELECT location_pings_ensure_partition($1)")
        .bind(now)
        .fetch_one(pool)
        .await
        .unwrap();
    sqlx::query(
        r#"
        INSERT INTO location_pings (
            user_id, branch_id, latitude, longitude, accuracy_m, recorded_at, on_duty, org_id
        )
        VALUES ($1, $2, $3, $4, 5.0, $5, true, '00000000-0000-0000-0000-0000000000a1')
        "#,
    )
    .bind(*user_id.as_uuid())
    .bind(*branch_id.as_uuid())
    .bind(latitude)
    .bind(longitude)
    .bind(now)
    .execute(pool)
    .await
    .unwrap();
}

async fn seed_work_order(pool: &PgPool, branch_id: BranchId, requested_by: UserId) -> WorkOrderId {
    let customer_id: uuid::Uuid = sqlx::query_scalar(
        "INSERT INTO registry_customers (branch_id, name, org_id) VALUES ($1, $2, '00000000-0000-0000-0000-0000000000a1') RETURNING id",
    )
    .bind(*branch_id.as_uuid())
    .bind(format!("Worker Customer {}", uuid::Uuid::new_v4()))
    .fetch_one(pool)
    .await
    .unwrap();
    let site_id: uuid::Uuid = sqlx::query_scalar(
        "INSERT INTO registry_sites (branch_id, customer_id, name, org_id) VALUES ($1, $2, $3, '00000000-0000-0000-0000-0000000000a1') RETURNING id",
    )
    .bind(*branch_id.as_uuid())
    .bind(customer_id)
    .bind("Worker Site")
    .fetch_one(pool)
    .await
    .unwrap();
    let equipment_id: uuid::Uuid = sqlx::query_scalar(
        r#"
        INSERT INTO registry_equipment (
            branch_id, customer_id, site_id, equipment_no, management_no,
            manufacturer_code, kind_code, power_code, status,
            specification, ton_text, model, source_sheet, source_row, org_id
        )
        VALUES ($1, $2, $3, $4, $5, 'A', 'B', 'C', '임대',
                '좌식', '2.5', 'GTS25DE', 'dispatch-worker-test', 1,
                '00000000-0000-0000-0000-0000000000a1')
        RETURNING id
        "#,
    )
    .bind(*branch_id.as_uuid())
    .bind(customer_id)
    .bind(site_id)
    .bind(
        format!(
            "WRK{}-0001",
            &uuid::Uuid::new_v4().simple().to_string()[..2]
        )
        .to_uppercase(),
    )
    .bind(format!(
        "W{}",
        &uuid::Uuid::new_v4().simple().to_string()[..8]
    ))
    .fetch_one(pool)
    .await
    .unwrap();
    let work_order_id = WorkOrderId::new();
    sqlx::query(
        r#"
        INSERT INTO work_orders (
            id, request_no, branch_id, equipment_id, customer_id, site_id,
            requested_by, status, priority, symptom, created_at, updated_at, org_id
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, 'RECEIVED', 'P1',
                'Emergency worker dispatch test', now(), now(),
                '00000000-0000-0000-0000-0000000000a1')
        "#,
    )
    .bind(*work_order_id.as_uuid())
    .bind("20260612-777")
    .bind(*branch_id.as_uuid())
    .bind(equipment_id)
    .bind(customer_id)
    .bind(site_id)
    .bind(*requested_by.as_uuid())
    .execute(pool)
    .await
    .unwrap();
    work_order_id
}
