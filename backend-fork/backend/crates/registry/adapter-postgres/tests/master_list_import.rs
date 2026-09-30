#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use console_kernel_core::{
    BranchId, BranchScope, EquipmentId, EquipmentSubstitutionId, OrgId, SiteId, TraceContext,
    UserId,
};
use console_registry_adapter_postgres::{
    PgRegistryError, PgRegistryStore, parse_master_list, parse_master_list_bytes,
};
use console_registry_application::{
    SubstituteAssignmentCommand, SubstituteReturnCommand, SubstituteSearch, UpdateSiteCommand,
    UpdateSiteFields,
};
use console_registry_domain::{EquipmentNo, Ton};
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use time::OffsetDateTime;

fn master_list_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../../docs/reference/master-list_251120.xlsx")
}

fn registry_import_staging_paths_for_this_process() -> BTreeSet<PathBuf> {
    let prefix = format!("console-registry-import-{}-", std::process::id());
    std::fs::read_dir(std::env::temp_dir())
        .unwrap()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|value| value.to_str())
                .is_some_and(|name| name.starts_with(&prefix))
        })
        .collect()
}

fn assert_no_new_registry_import_staging_paths(before: &BTreeSet<PathBuf>) {
    let after = registry_import_staging_paths_for_this_process();
    let new_paths = after.difference(before).collect::<Vec<_>>();
    assert!(
        new_paths.is_empty(),
        "uploaded workbook parsing/import must not leave console-registry-import-* staging files: {new_paths:#?}"
    );
}

#[test]
fn parser_self_checks_prefix_formulas_against_the_real_workbook() {
    let parsed = parse_master_list(master_list_path()).unwrap();

    assert_eq!(parsed.input_rows, 486);
    assert_eq!(parsed.equipment.len(), 445);
    assert!(parsed.errors.is_empty(), "{:#?}", parsed.errors);
    assert_eq!(parsed.prefix_checked_rows, 486);
}

#[test]
fn byte_parser_matches_path_parser_for_the_real_workbook() {
    let before = registry_import_staging_paths_for_this_process();
    let bytes = std::fs::read(master_list_path()).unwrap();
    let from_path = parse_master_list(master_list_path()).unwrap();
    let from_bytes = parse_master_list_bytes(&bytes).unwrap();

    assert_eq!(from_bytes, from_path);
    assert_no_new_registry_import_staging_paths(&before);
}

#[test]
fn byte_parser_rejects_invalid_workbook_bytes_without_echoing_payload() {
    let before = registry_import_staging_paths_for_this_process();
    let payload = b"not a workbook with private upload contents";
    let err = parse_master_list_bytes(payload).unwrap_err();
    let message = err.to_string();

    assert!(message.contains("workbook error"));
    assert!(
        !message.contains("private upload contents"),
        "workbook errors must not echo uploaded workbook contents: {message}"
    );
    assert_no_new_registry_import_staging_paths(&before);
}

#[tokio::test]
async fn invalid_uploaded_bytes_return_workbook_error_without_temp_staging_residue() {
    let before = registry_import_staging_paths_for_this_process();
    let pool = PgPoolOptions::new()
        .connect_lazy("postgres://console_rt:***@127.0.0.1/console_registry_import_unused")
        .unwrap();

    let result = PgRegistryStore::new(pool)
        .import_master_list_bytes(UserId::new(), "broken-master-list.xlsx", b"not a workbook")
        .await;

    assert!(
        matches!(result, Err(PgRegistryError::Workbook(_))),
        "invalid uploaded workbook bytes should fail during workbook parsing, got {result:?}"
    );
    assert_no_new_registry_import_staging_paths(&before);
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn real_master_list_import_is_idempotent_queryable_and_audited(pool: PgPool) {
    console_platform_request_context::scope_org(console_kernel_core::OrgId::knl(), async move {
        let store = PgRegistryStore::new(pool.clone());

        let first = store.import_master_list(&master_list_path()).await.unwrap();
        assert_eq!(first.added, 445);
        assert_eq!(first.updated, 0);
        assert_eq!(first.unchanged, 0);
        assert_eq!(first.orphaned, 0);
        assert!(first.errors.is_empty(), "{:#?}", first.errors);

        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM registry_equipment")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 445);

        let lookup = store.find_model_by_management_no("290").await.unwrap();
        assert_eq!(lookup.as_deref(), Some("GTS25DE"));

        let residual = store
            .residual_value_by_equipment_no("CFB18-0006")
            .await
            .unwrap();
        assert_eq!(residual, Some(-10_650_084));

        let second = store.import_master_list(&master_list_path()).await.unwrap();
        assert_eq!(second.added, 0);
        assert_eq!(second.updated, 0);
        assert_eq!(second.unchanged, 445);
        assert_eq!(second.orphaned, 0);
        assert!(second.errors.is_empty(), "{:#?}", second.errors);

        let audit_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM audit_events WHERE action = 'registry.import'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(audit_count, 2);
    })
    .await;
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn modified_copy_reports_single_update(pool: PgPool) {
    console_platform_request_context::scope_org(console_kernel_core::OrgId::knl(), async move {
        let store = PgRegistryStore::new(pool.clone());
        store.import_master_list(&master_list_path()).await.unwrap();

        let modified = copy_master_list("registry-modified-copy.xlsx");
        rewrite_cell(
            &modified,
            "K&L 지게차 Master list",
            "Q291",
            "GTS25DE-UPDATED",
        );

        let report = store.import_master_list(&modified).await.unwrap();
        assert_eq!(report.added, 0);
        assert_eq!(report.updated, 1);
        assert_eq!(report.unchanged, 444);
        assert!(report.errors.is_empty(), "{:#?}", report.errors);

        let lookup = store.find_model_by_management_no("290").await.unwrap();
        assert_eq!(lookup.as_deref(), Some("GTS25DE-UPDATED"));
    })
    .await;
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn dirty_rows_are_reported_without_writing_the_failed_row(pool: PgPool) {
    console_platform_request_context::scope_org(console_kernel_core::OrgId::knl(), async move {
        let dirty = copy_master_list("registry-dirty-copy.xlsx");
        rewrite_cell(&dirty, "K&L 지게차 Master list", "F4", "");

        let store = PgRegistryStore::new(pool.clone());
        let report = store.import_master_list(&dirty).await.unwrap();

        assert_eq!(report.added, 444);
        assert_eq!(report.errors.len(), 1);
        assert_eq!(report.errors[0].sheet, "K&L 지게차 Master list");
        assert_eq!(report.errors[0].row, 4);

        let missing: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM registry_equipment WHERE equipment_no = 'CFB30-0001'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(missing, 0);
    })
    .await;
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn substitute_candidates_filter_rank_branch_scope_and_unknown_ton(pool: PgPool) {
    console_platform_request_context::scope_org(console_kernel_core::OrgId::knl(), async move {
        let store = PgRegistryStore::new(pool.clone());
        let branch = seed_branch(&pool, "Substitute Region", "Substitute Branch").await;
        let other_branch =
            seed_branch(&pool, "Other Substitute Region", "Other Substitute Branch").await;
        let down = seed_equipment(
            &pool,
            branch,
            EquipmentFixture::new("CFO25-0290", "290", "임대", "좌식", "2.5T")
                .placement_location("A-1"),
        )
        .await;
        let exact = seed_equipment(
            &pool,
            branch,
            EquipmentFixture::new("DFO25-0106", "106", "예비", "좌식", "2.5T")
                .placement_location("Reserve-Exact"),
        )
        .await;
        let above = seed_equipment(
            &pool,
            branch,
            EquipmentFixture::new("CFO35-0075", "075", "예비", "좌식", "3.5T")
                .placement_location("Reserve-Above"),
        )
        .await;
        seed_equipment(
            &pool,
            branch,
            EquipmentFixture::new("CFB25-0284", "284", "예비", "입식", "2.5T"),
        )
        .await;
        seed_equipment(
            &pool,
            branch,
            EquipmentFixture::new("CFB25-0100", "100", "예비", "좌식", "2.5T"),
        )
        .await;
        seed_equipment(
            &pool,
            branch,
            EquipmentFixture::new("CFO18-9998", "998", "예비", "좌식", "1.8T"),
        )
        .await;
        let other_branch_exact = seed_equipment(
            &pool,
            other_branch,
            EquipmentFixture::new("DFO25-9106", "9106", "예비", "좌식", "2.5T")
                .placement_location("Other Branch"),
        )
        .await;

        let same_branch = store
            .substitute_candidates(SubstituteSearch {
                equipment_id: down,
                branch_scope: BranchScope::single(branch),
                include_all_branches: false,
            })
            .await
            .unwrap();

        assert_eq!(
            equipment_numbers(&same_branch),
            vec!["DFO25-0106", "CFO35-0075"]
        );
        assert_eq!(same_branch[0].equipment_id, exact);
        assert_eq!(same_branch[0].site_name, "케이앤엘");
        assert_eq!(
            same_branch[0].placement_location.as_deref(),
            Some("Reserve-Exact")
        );
        assert_eq!(same_branch[1].equipment_id, above);

        let all_branches = store
            .substitute_candidates(SubstituteSearch {
                equipment_id: down,
                branch_scope: BranchScope::All,
                include_all_branches: true,
            })
            .await
            .unwrap();

        assert_eq!(
            equipment_numbers(&all_branches),
            vec!["DFO25-0106", "DFO25-9106", "CFO35-0075"]
        );
        assert!(
            all_branches
                .iter()
                .any(|candidate| candidate.equipment_id == other_branch_exact)
        );

        let hidden = store
            .substitute_candidates(SubstituteSearch {
                equipment_id: down,
                branch_scope: BranchScope::single(other_branch),
                include_all_branches: false,
            })
            .await
            .unwrap_err();
        assert!(hidden.to_string().contains("outside branch scope"));

        let unknown_down = seed_equipment(
            &pool,
            branch,
            EquipmentFixture::new("EOB00-0067", "067", "임대", "입식", "미정"),
        )
        .await;
        let unknown_candidate = seed_equipment(
            &pool,
            branch,
            EquipmentFixture::new("EOB00-0442", "442", "예비", "입식", "미정"),
        )
        .await;
        seed_equipment(
            &pool,
            branch,
            EquipmentFixture::new("EOB15-9999", "9999", "예비", "입식", "1.5T"),
        )
        .await;

        let unknown_matches = store
            .substitute_candidates(SubstituteSearch {
                equipment_id: unknown_down,
                branch_scope: BranchScope::single(branch),
                include_all_branches: false,
            })
            .await
            .unwrap();
        assert_eq!(unknown_matches.len(), 1);
        assert_eq!(unknown_matches[0].equipment_id, unknown_candidate);
    })
    .await;
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn substitute_assignment_lifecycle_is_audited_and_controls_availability(pool: PgPool) {
    console_platform_request_context::scope_org(console_kernel_core::OrgId::knl(), async move {
        let store = PgRegistryStore::new(pool.clone());
        let branch = seed_branch(&pool, "Assignment Region", "Assignment Branch").await;
        let actor = seed_user(&pool, branch, "ADMIN").await;
        let mechanic = seed_user(&pool, branch, "MECHANIC").await;
        let down = seed_equipment(
            &pool,
            branch,
            EquipmentFixture::new("CFO25-0290", "290", "임대", "좌식", "2.5T"),
        )
        .await;
        let second_down = seed_equipment(
            &pool,
            branch,
            EquipmentFixture::new("DFO25-0291", "291", "임대", "좌식", "2.5T"),
        )
        .await;
        let substitute = seed_equipment(
            &pool,
            branch,
            EquipmentFixture::new("DFO25-0106", "106", "예비", "좌식", "2.5T")
                .placement_location("Reserve Yard"),
        )
        .await;

        let assigned = store
            .assign_substitute(SubstituteAssignmentCommand {
                actor,
                branch_scope: BranchScope::single(branch),
                source_equipment_id: down,
                substitute_equipment_id: substitute,
                assigned_to: Some(mechanic),
                assignment_location: "Customer dock".to_owned(),
                trace: TraceContext::generate(),
                assigned_at: OffsetDateTime::now_utc(),
            })
            .await
            .unwrap();

        assert_ne!(assigned.id, EquipmentSubstitutionId::new());
        assert_eq!(assigned.source_equipment_id, down);
        assert_eq!(assigned.substitute_equipment_id, substitute);
        assert!(assigned.returned_at.is_none());
        assert_audit_count(&pool, "equipment.substitute.assign", 1).await;

        let unavailable = store
            .substitute_candidates(SubstituteSearch {
                equipment_id: second_down,
                branch_scope: BranchScope::single(branch),
                include_all_branches: false,
            })
            .await
            .unwrap();
        assert!(unavailable.is_empty());

        let returned = store
            .return_substitute(SubstituteReturnCommand {
                actor,
                branch_scope: BranchScope::single(branch),
                substitution_id: assigned.id,
                trace: TraceContext::generate(),
                returned_at: OffsetDateTime::now_utc(),
                return_note: Some("Returned after repair".to_owned()),
            })
            .await
            .unwrap();

        assert!(returned.returned_at.is_some());
        assert_eq!(
            returned.return_note.as_deref(),
            Some("Returned after repair")
        );
        assert_audit_count(&pool, "equipment.substitute.return", 1).await;

        let available_again = store
            .substitute_candidates(SubstituteSearch {
                equipment_id: second_down,
                branch_scope: BranchScope::single(branch),
                include_all_branches: false,
            })
            .await
            .unwrap();
        assert_eq!(equipment_numbers(&available_again), vec!["DFO25-0106"]);
    })
    .await;
}

fn copy_master_list(filename: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "console-registry-test-{}-{}",
        std::process::id(),
        filename
    ));
    if dir.exists() {
        std::fs::remove_dir_all(&dir).unwrap();
    }
    std::fs::create_dir_all(&dir).unwrap();
    let target = dir.join(filename);
    std::fs::copy(master_list_path(), &target).unwrap();
    target
}

fn rewrite_cell(path: &Path, sheet: &str, cell: &str, value: &str) {
    let mut workbook = umya_spreadsheet::reader::xlsx::read(path).unwrap();
    workbook
        .sheet_by_name_mut(sheet)
        .expect("sheet should exist")
        .cell_mut(cell)
        .set_value(value);
    umya_spreadsheet::writer::xlsx::write(&workbook, path).unwrap();
}

fn equipment_numbers(
    candidates: &[console_registry_application::SubstituteCandidate],
) -> Vec<&str> {
    candidates
        .iter()
        .map(|candidate| candidate.equipment_no.as_str())
        .collect()
}

#[derive(Debug, Clone)]
struct EquipmentFixture {
    equipment_no: &'static str,
    management_no: &'static str,
    status: &'static str,
    specification: &'static str,
    ton: &'static str,
    placement_location: Option<&'static str>,
}

impl EquipmentFixture {
    fn new(
        equipment_no: &'static str,
        management_no: &'static str,
        status: &'static str,
        specification: &'static str,
        ton: &'static str,
    ) -> Self {
        Self {
            equipment_no,
            management_no,
            status,
            specification,
            ton,
            placement_location: None,
        }
    }

    fn placement_location(mut self, placement_location: &'static str) -> Self {
        self.placement_location = Some(placement_location);
        self
    }
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn site_contact_update_persists_and_is_audited(pool: PgPool) {
    // Issue #13: PATCH /sites/{id} now also writes the representative contact.
    console_platform_request_context::scope_org(OrgId::knl(), async move {
        let store = PgRegistryStore::new(pool.clone());
        let branch_id = seed_branch(&pool, "수도권", "본사").await;
        let actor = seed_user(&pool, branch_id, "ADMIN").await;
        let site_id = seed_site(&pool, branch_id).await;

        store
            .update_site(UpdateSiteCommand {
                actor,
                site_id,
                fields: UpdateSiteFields {
                    contact_name: Some(Some("김담당".to_string())),
                    contact_phone: Some(Some("010-2625-0987".to_string())),
                    contact_email: Some(Some("ops@example.com".to_string())),
                    ..UpdateSiteFields::default()
                },
                branch_scope: BranchScope::All,
                trace: TraceContext::generate(),
                occurred_at: OffsetDateTime::now_utc(),
            })
            .await
            .unwrap();

        let (name, phone, email): (Option<String>, Option<String>, Option<String>) =
            sqlx::query_as(
                "SELECT contact_name, contact_phone, contact_email \
                 FROM registry_sites WHERE id = $1",
            )
            .bind(*site_id.as_uuid())
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(name.as_deref(), Some("김담당"));
        assert_eq!(phone.as_deref(), Some("010-2625-0987"));
        assert_eq!(email.as_deref(), Some("ops@example.com"));
        assert_audit_count(&pool, "site.update", 1).await;

        // Some(None) clears a column to NULL; an absent field is left untouched.
        store
            .update_site(UpdateSiteCommand {
                actor,
                site_id,
                fields: UpdateSiteFields {
                    contact_email: Some(None),
                    ..UpdateSiteFields::default()
                },
                branch_scope: BranchScope::All,
                trace: TraceContext::generate(),
                occurred_at: OffsetDateTime::now_utc(),
            })
            .await
            .unwrap();
        let (name2, email2): (Option<String>, Option<String>) =
            sqlx::query_as("SELECT contact_name, contact_email FROM registry_sites WHERE id = $1")
                .bind(*site_id.as_uuid())
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(email2, None, "Some(None) clears contact_email");
        assert_eq!(name2.as_deref(), Some("김담당"), "absent field untouched");
        assert_audit_count(&pool, "site.update", 2).await;
    })
    .await;
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn site_update_rejects_a_site_outside_the_actor_branch_scope(pool: PgPool) {
    // Issue #13 review: sites are branch-scoped (unlike org-global equipment), so
    // a branch-limited actor must not edit a site in another branch of the org.
    console_platform_request_context::scope_org(OrgId::knl(), async move {
        let store = PgRegistryStore::new(pool.clone());
        let branch_id = seed_branch(&pool, "수도권", "본사").await;
        let actor = seed_user(&pool, branch_id, "ADMIN").await;
        let site_id = seed_site(&pool, branch_id).await;

        // Actor scoped to a DIFFERENT branch → the site is treated as not found.
        let result = store
            .update_site(UpdateSiteCommand {
                actor,
                site_id,
                fields: UpdateSiteFields {
                    contact_name: Some(Some("불가".to_string())),
                    ..UpdateSiteFields::default()
                },
                branch_scope: BranchScope::single(BranchId::new()),
                trace: TraceContext::generate(),
                occurred_at: OffsetDateTime::now_utc(),
            })
            .await;
        assert!(result.is_err(), "cross-branch site edit must be rejected");

        // The write did not happen and nothing was audited.
        let name: Option<String> =
            sqlx::query_scalar("SELECT contact_name FROM registry_sites WHERE id = $1")
                .bind(*site_id.as_uuid())
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(name, None, "no contact written for an out-of-scope site");
        assert_audit_count(&pool, "site.update", 0).await;
    })
    .await;
}

async fn seed_site(pool: &PgPool, branch_id: BranchId) -> SiteId {
    let customer_id: uuid::Uuid = sqlx::query_scalar(
        r#"
        INSERT INTO registry_customers (branch_id, name, org_id)
        VALUES ($1, 'K&L', $2)
        ON CONFLICT (branch_id, name) DO UPDATE SET name = EXCLUDED.name
        RETURNING id
        "#,
    )
    .bind(*branch_id.as_uuid())
    .bind(*OrgId::knl().as_uuid())
    .fetch_one(pool)
    .await
    .unwrap();
    let site_id: uuid::Uuid = sqlx::query_scalar(
        r#"
        INSERT INTO registry_sites (branch_id, customer_id, name, org_id)
        VALUES ($1, $2, '안산현장', $3)
        RETURNING id
        "#,
    )
    .bind(*branch_id.as_uuid())
    .bind(customer_id)
    .bind(*OrgId::knl().as_uuid())
    .fetch_one(pool)
    .await
    .unwrap();
    SiteId::from_uuid(site_id)
}

async fn seed_branch(pool: &PgPool, region_name: &str, branch_name: &str) -> BranchId {
    let knl = *OrgId::knl().as_uuid();
    let region_id: uuid::Uuid =
        sqlx::query_scalar("INSERT INTO regions (name, org_id) VALUES ($1, $2) RETURNING id")
            .bind(region_name)
            .bind(knl)
            .fetch_one(pool)
            .await
            .unwrap();
    let branch_id: uuid::Uuid = sqlx::query_scalar(
        "INSERT INTO branches (region_id, name, org_id) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(region_id)
    .bind(branch_name)
    .bind(knl)
    .fetch_one(pool)
    .await
    .unwrap();
    BranchId::from_uuid(branch_id)
}

async fn seed_user(pool: &PgPool, branch_id: BranchId, role: &str) -> UserId {
    let knl = *OrgId::knl().as_uuid();
    let user_id = UserId::new();
    sqlx::query("INSERT INTO users (id, display_name, roles, org_id) VALUES ($1, $2, $3, $4)")
        .bind(*user_id.as_uuid())
        .bind(format!("Substitute {role}"))
        .bind(Vec::from([role]))
        .bind(knl)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO user_branches (user_id, branch_id, org_id) VALUES ($1, $2, $3)")
        .bind(*user_id.as_uuid())
        .bind(*branch_id.as_uuid())
        .bind(knl)
        .execute(pool)
        .await
        .unwrap();
    user_id
}

async fn seed_equipment(
    pool: &PgPool,
    branch_id: BranchId,
    fixture: EquipmentFixture,
) -> EquipmentId {
    let customer_id: uuid::Uuid = sqlx::query_scalar(
        r#"
        INSERT INTO registry_customers (branch_id, name, org_id)
        VALUES ($1, 'K&L', $2)
        ON CONFLICT (branch_id, name) DO UPDATE SET name = EXCLUDED.name
        RETURNING id
        "#,
    )
    .bind(*branch_id.as_uuid())
    .bind(*OrgId::knl().as_uuid())
    .fetch_one(pool)
    .await
    .unwrap();
    let site_id: uuid::Uuid = sqlx::query_scalar(
        r#"
        INSERT INTO registry_sites (branch_id, customer_id, name, org_id)
        VALUES ($1, $2, '케이앤엘', $3)
        ON CONFLICT (branch_id, customer_id, name) DO UPDATE SET name = EXCLUDED.name
        RETURNING id
        "#,
    )
    .bind(*branch_id.as_uuid())
    .bind(customer_id)
    .bind(*OrgId::knl().as_uuid())
    .fetch_one(pool)
    .await
    .unwrap();
    let equipment_no = EquipmentNo::parse(fixture.equipment_no).unwrap();
    let ton = Ton::parse(fixture.ton);
    let equipment_id: uuid::Uuid = sqlx::query_scalar(
        r#"
        INSERT INTO registry_equipment (
            branch_id, customer_id, site_id, equipment_no, management_no,
            manufacturer_code, kind_code, power_code, power_label, status,
            placement_location, specification, ton_text, ton_milli,
            model, source_sheet, source_row, org_id
        )
        VALUES (
            $1, $2, $3, $4, $5,
            $6, $7, $8, $9, $10,
            $11, $12, $13, $14,
            $15, 'test fixture', 1, $16
        )
        RETURNING id
        "#,
    )
    .bind(*branch_id.as_uuid())
    .bind(customer_id)
    .bind(site_id)
    .bind(equipment_no.as_str())
    .bind(fixture.management_no)
    .bind(equipment_no.manufacturer_code())
    .bind(equipment_no.kind_code())
    .bind(equipment_no.power_code())
    .bind(power_label(equipment_no.power_code()))
    .bind(fixture.status)
    .bind(fixture.placement_location)
    .bind(fixture.specification)
    .bind(ton.as_text())
    .bind(ton.milli_tons())
    .bind(format!("Model {}", fixture.management_no))
    .bind(*OrgId::knl().as_uuid())
    .fetch_one(pool)
    .await
    .unwrap();
    EquipmentId::from_uuid(equipment_id)
}

fn power_label(power_code: &str) -> &'static str {
    match power_code {
        "B" => "전동",
        "O" => "디젤",
        _ => "기타",
    }
}

async fn assert_audit_count(pool: &PgPool, action: &str, expected: i64) {
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM audit_events WHERE action = $1")
        .bind(action)
        .fetch_one(pool)
        .await
        .unwrap();
    assert_eq!(count, expected, "audit count for {action}");
}
