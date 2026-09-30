#![allow(clippy::unwrap_used, clippy::expect_used)]
// From the frontend root, after `fnm exec --using=24.21.0 npm run build`:
// SQLX_OFFLINE=true FRONTEND_ROOT="$PWD" backend-fork/tools/lanes/pgtest.sh "$PWD/backend-fork" \
//   fnm exec --using=24.21.0 cargo test --locked --features frontend-e2e \
//   -p console-sales-rest --test real_browser_inquiry -- --test-threads=1

use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use console_kernel_core::{OrgId, SalesListingId, TraceContext, UserId};
use console_sales_adapter_postgres::PgSalesStore;
use console_sales_application::{CreateListingCommand, DeleteListingCommand, ListingInput};
use console_sales_domain::{ListingCondition, ListingKind, ListingStatus, ListingType};
use console_sales_rest::{SalesRestState, router};
use sqlx::PgPool;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;

async fn expect_marker(
    lines: &mut tokio::io::Lines<BufReader<tokio::process::ChildStdout>>,
    expected: &str,
) {
    let received = tokio::time::timeout(Duration::from_secs(90), lines.next_line())
        .await
        .expect("browser handshake timed out")
        .expect("browser stdout failed")
        .expect("browser ended before handshake");
    assert_eq!(received, expected);
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn browser_preserves_listing_conflict_and_requires_explicit_general_inquiry(pool: PgPool) {
    let frontend_root =
        PathBuf::from(std::env::var_os("FRONTEND_ROOT").expect(
            "FRONTEND_ROOT must point to the built Next frontend for this real browser test",
        ));
    assert!(
        frontend_root.join(".next/BUILD_ID").is_file(),
        "build Next before this test"
    );
    let browser_script = frontend_root.join("tools/test-storefront-real-service.mjs");
    assert!(
        browser_script.is_file(),
        "real-service browser script is missing"
    );

    let actor_id: uuid::Uuid = sqlx::query_scalar(
        "INSERT INTO users (display_name, roles, org_id) VALUES ('Sales Admin', ARRAY['ADMIN'], $1) RETURNING id",
    )
    .bind(*OrgId::knl().as_uuid())
    .fetch_one(&pool)
    .await
    .unwrap();
    let actor = UserId::from_uuid(actor_id);
    let listing_id = SalesListingId::new();
    let store = PgSalesStore::new(pool.clone());
    console_platform_request_context::scope_org(OrgId::knl(), async {
        store
            .create_listing(CreateListingCommand {
                actor,
                listing_id,
                input: ListingInput {
                    equipment_id: None,
                    kind: ListingKind::Electric,
                    condition: ListingCondition::Used,
                    model_name: "실제 서비스 문의 장비".into(),
                    capacity_milli: None,
                    model_year: None,
                    usage_hours: None,
                    price_won: None,
                    badge: None,
                    usage_label: None,
                    condition_label: None,
                    availability: None,
                    location: None,
                    description: None,
                    listing_type: ListingType::Sale,
                    status: ListingStatus::Published,
                    sort_weight: 0,
                },
                trace: TraceContext::generate(),
                occurred_at: time::OffsetDateTime::now_utc(),
            })
            .await
            .unwrap();
    })
    .await;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        axum::serve(listener, router(SalesRestState::new(store.clone(), None)))
            .await
            .unwrap();
    });

    let mut command = Command::new("node");
    command
        .arg(browser_script)
        .current_dir(frontend_root)
        .env_clear()
        .env("REAL_SERVICE_API_ORIGIN", origin)
        .env("REAL_SERVICE_LISTING_ID", listing_id.to_string())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    // The disposable SQLx administrator URL stays in Rust. Next and Chromium
    // receive only their local endpoints and process-launch environment.
    for name in [
        "PATH",
        "HOME",
        "TMPDIR",
        "LANG",
        "LC_ALL",
        "PLAYWRIGHT_BROWSERS_PATH",
    ] {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    let mut browser = command
        .spawn()
        .expect("start real-service Chromium journey");
    let mut lines = BufReader::new(browser.stdout.take().unwrap()).lines();
    let mut stdin = browser.stdin.take().unwrap();

    expect_marker(&mut lines, "DETAIL_READY").await;
    console_platform_request_context::scope_org(OrgId::knl(), async {
        PgSalesStore::new(pool.clone())
            .delete_listing(DeleteListingCommand {
                actor,
                listing_id,
                trace: TraceContext::generate(),
                occurred_at: time::OffsetDateTime::now_utc(),
            })
            .await
            .unwrap();
    })
    .await;
    stdin.write_all(b"WITHDRAWN\n").await.unwrap();

    // Check each browser boundary before allowing the next action.
    expect_marker(&mut lines, "CONFLICT_SEEN").await;
    let inquiries: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM customer_inquiries")
        .fetch_one(&pool)
        .await
        .unwrap();
    let audit: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM audit_events WHERE action = 'sales_inquiry.submit'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        (inquiries, audit),
        (0, 0),
        "409 must not record a general inquiry"
    );
    stdin.write_all(b"CHECKED\n").await.unwrap();

    expect_marker(&mut lines, "GENERAL_SELECTED").await;
    let inquiries: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM customer_inquiries")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(inquiries, 0, "switching context must not submit");
    stdin.write_all(b"CHECKED\n").await.unwrap();

    drop(stdin);
    let output = tokio::time::timeout(Duration::from_secs(90), browser.wait_with_output())
        .await
        .expect("real-service browser did not exit")
        .unwrap();
    server.abort();
    assert!(
        output.status.success(),
        "real-service browser failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let (inquiries, unlinked): (i64, i64) = sqlx::query_as(
        "SELECT COUNT(*), COUNT(*) FILTER (WHERE listing_id IS NULL) FROM customer_inquiries",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let audit: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM audit_events WHERE action = 'sales_inquiry.submit'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!((inquiries, unlinked, audit), (1, 1, 1));
    let (name, phone, message, topic): (String, String, Option<String>, String) =
        sqlx::query_as("SELECT name, phone, message, topic::text FROM customer_inquiries")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        (
            name.as_str(),
            phone.as_str(),
            message.as_deref(),
            topic.as_str()
        ),
        (
            "김고객",
            "010-9999-8888",
            Some("이 장비를 문의합니다."),
            "USED_SALES"
        )
    );
}
