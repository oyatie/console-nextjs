#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
//! Executable Consulting engagement contract over the composed app router and
//! the genuine non-owner `console_rt` role.

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
use time::format_description::well_known::Rfc3339;
use time::{Duration, OffsetDateTime};
use tower::ServiceExt;
use uuid::Uuid;

const TEST_ISSUER: &str = "console-platform-auth";
const TEST_AUDIENCE: &str = "console-api";

struct Keys {
    private_pem: String,
    public_pem: String,
}

struct Fixture {
    org: OrgId,
    other_org_engagement: Uuid,
    requester: UserId,
    approver: UserId,
    customer: Uuid,
    document: Uuid,
    ontology: Uuid,
    evidence: Uuid,
    kpi: Uuid,
}

struct JsonResponse {
    status: StatusCode,
    json: Value,
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn consulting_engagement_story_is_tenant_scoped_idempotent_and_terminal(owner_pool: PgPool) {
    let fixture = seed_fixture(&owner_pool).await;
    let keys = keys();
    let requester_token = bearer(&keys, fixture.requester, fixture.org);
    let approver_token = bearer(&keys, fixture.approver, fixture.org);
    let runtime_pool = runtime_role_pool(&owner_pool).await;
    let service = build_router(app_state(runtime_pool.clone(), &keys));

    let create_body = json!({
        "customerId": fixture.customer,
        "customerDocumentId": fixture.document,
        "ontologyInstanceId": fixture.ontology,
        "title": "Stabilize customer operations",
        "idempotencyKey": "consulting-story-1"
    });

    let created = send(
        service.clone(),
        "POST",
        "/api/v1/consulting/engagements",
        Some(&requester_token),
        Some(create_body.clone()),
    )
    .await;
    assert_eq!(created.status, StatusCode::CREATED, "{:?}", created.json);
    let engagement_id = response_uuid(&created.json, "id");
    assert_eq!(created.json["status"], "DRAFT");
    assert_eq!(created.json["version"], 1);
    assert_rfc3339(&created.json, "created_at");
    assert_rfc3339(&created.json, "updated_at");

    let replayed = send(
        service.clone(),
        "POST",
        "/api/v1/consulting/engagements",
        Some(&requester_token),
        Some(create_body.clone()),
    )
    .await;
    assert_eq!(replayed.status, StatusCode::CREATED, "{:?}", replayed.json);
    assert_eq!(response_uuid(&replayed.json, "id"), engagement_id);
    assert_rfc3339(&replayed.json, "created_at");

    let mismatch = send(
        service.clone(),
        "POST",
        "/api/v1/consulting/engagements",
        Some(&requester_token),
        Some(json!({
            "customerId": fixture.customer,
            "customerDocumentId": fixture.document,
            "ontologyInstanceId": fixture.ontology,
            "title": "A different payload",
            "idempotencyKey": "consulting-story-1"
        })),
    )
    .await;
    assert_eq!(mismatch.status, StatusCode::CONFLICT, "{:?}", mismatch.json);
    assert_eq!(mismatch.json["error"]["code"], "conflict");
    assert_eq!(
        mismatch.json["error"]["message"],
        "idempotency key was already used with a different request payload"
    );

    let (concurrent_replay_a, concurrent_replay_b) = tokio::join!(
        send(
            service.clone(),
            "POST",
            "/api/v1/consulting/engagements",
            Some(&requester_token),
            Some(create_body.clone()),
        ),
        send(
            service.clone(),
            "POST",
            "/api/v1/consulting/engagements",
            Some(&requester_token),
            Some(create_body.clone()),
        )
    );
    for replay in [concurrent_replay_a, concurrent_replay_b] {
        assert_eq!(replay.status, StatusCode::CREATED, "{:?}", replay.json);
        assert_eq!(response_uuid(&replay.json, "id"), engagement_id);
    }
    assert_eq!(
        count_engagements(&owner_pool, fixture.org, "consulting-story-1").await,
        1
    );

    let other_org = send(
        service.clone(),
        "GET",
        &format!(
            "/api/v1/consulting/engagements/{}",
            fixture.other_org_engagement
        ),
        Some(&requester_token),
        None,
    )
    .await;
    assert_eq!(
        other_org.status,
        StatusCode::NOT_FOUND,
        "{:?}",
        other_org.json
    );

    let listed = send(
        service.clone(),
        "GET",
        "/api/v1/consulting/engagements?limit=10&offset=0",
        Some(&requester_token),
        None,
    )
    .await;
    assert_eq!(listed.status, StatusCode::OK, "{:?}", listed.json);
    assert_eq!(listed.json["total"], 1);
    assert_eq!(response_uuid(&listed.json["items"][0], "id"), engagement_id);

    let diagnostic = send(
        service.clone(),
        "POST",
        &format!("/api/v1/consulting/engagements/{engagement_id}/diagnostics"),
        Some(&requester_token),
        Some(json!({
            "summary": "Customer handoffs are inconsistent",
            "documentId": fixture.document
        })),
    )
    .await;
    assert_eq!(
        diagnostic.status,
        StatusCode::CREATED,
        "{:?}",
        diagnostic.json
    );
    let diagnostic_id = response_uuid(&diagnostic.json, "id");

    let finding = send(
        service.clone(),
        "POST",
        &format!("/api/v1/consulting/engagements/{engagement_id}/findings"),
        Some(&requester_token),
        Some(json!({
            "diagnosticId": diagnostic_id,
            "statement": "Manual routing creates avoidable delay",
            "evidenceId": fixture.evidence,
            "documentId": fixture.document
        })),
    )
    .await;
    assert_eq!(finding.status, StatusCode::CREATED, "{:?}", finding.json);
    let finding_id = response_uuid(&finding.json, "id");

    let initiative = send(
        service.clone(),
        "POST",
        &format!("/api/v1/consulting/engagements/{engagement_id}/initiatives"),
        Some(&requester_token),
        Some(json!({
            "findingId": finding_id,
            "title": "Standardize operational handoffs",
            "hypothesis": "A shared handoff protocol reduces lead time",
            "kpiDefinitionId": fixture.kpi,
            "targetDirection": "DECREASE"
        })),
    )
    .await;
    assert_eq!(
        initiative.status,
        StatusCode::CREATED,
        "{:?}",
        initiative.json
    );
    let initiative_id = response_uuid(&initiative.json, "id");

    let proposed = transition(
        service.clone(),
        engagement_id,
        &requester_token,
        "PROPOSED",
        1,
        None,
    )
    .await;
    assert_eq!(proposed.status, StatusCode::OK, "{:?}", proposed.json);
    assert_eq!(proposed.json["version"], 2);

    let approval_id = seed_approval(
        &owner_pool,
        fixture.org,
        fixture.requester,
        fixture.approver,
        engagement_id,
    )
    .await;
    let (approval_a, approval_b) = tokio::join!(
        transition(
            service.clone(),
            engagement_id,
            &approver_token,
            "APPROVED",
            2,
            Some(approval_id),
        ),
        transition(
            service.clone(),
            engagement_id,
            &approver_token,
            "APPROVED",
            2,
            Some(approval_id),
        )
    );
    let (approved, rejected) = if approval_a.status == StatusCode::OK {
        (approval_a, approval_b)
    } else {
        (approval_b, approval_a)
    };
    assert_eq!(approved.status, StatusCode::OK, "{:?}", approved.json);
    assert_eq!(approved.json["status"], "APPROVED");
    assert_eq!(approved.json["version"], 3);
    assert_eq!(rejected.status, StatusCode::CONFLICT, "{:?}", rejected.json);
    assert_eq!(rejected.json["error"]["code"], "conflict");
    assert_eq!(approval_consumptions(&owner_pool, approval_id).await, 1);

    let implemented = transition(
        service.clone(),
        engagement_id,
        &requester_token,
        "IMPLEMENTED",
        3,
        None,
    )
    .await;
    assert_eq!(implemented.status, StatusCode::OK, "{:?}", implemented.json);
    assert_eq!(implemented.json["version"], 4);

    let observation = send(
        service.clone(),
        "POST",
        &format!("/api/v1/consulting/engagements/{engagement_id}/observations"),
        Some(&requester_token),
        Some(json!({
            "initiativeId": initiative_id,
            "kpiDefinitionId": fixture.kpi,
            "evidenceId": fixture.evidence,
            "observedAt": "2026-07-23T12:00:00Z",
            "note": "Median handoff time decreased"
        })),
    )
    .await;
    assert_eq!(
        observation.status,
        StatusCode::CREATED,
        "{:?}",
        observation.json
    );
    // The literal posted above must come back byte for byte: proves both halves
    // of the `observed_at` wire contract, not just that some value survived.
    assert_eq!(observation.json["observed_at"], "2026-07-23T12:00:00Z");
    assert_rfc3339(&observation.json, "created_at");
    let observation_id = response_uuid(&observation.json, "id");

    let measured = transition(
        service.clone(),
        engagement_id,
        &requester_token,
        "MEASURED",
        4,
        None,
    )
    .await;
    assert_eq!(measured.status, StatusCode::OK, "{:?}", measured.json);
    assert_eq!(measured.json["version"], 5);

    let sustained = transition(
        service.clone(),
        engagement_id,
        &requester_token,
        "SUSTAINED",
        5,
        None,
    )
    .await;
    assert_eq!(sustained.status, StatusCode::OK, "{:?}", sustained.json);
    assert_eq!(sustained.json["status"], "SUSTAINED");
    assert_eq!(sustained.json["version"], 6);

    let detail = send(
        service.clone(),
        "GET",
        &format!("/api/v1/consulting/engagements/{engagement_id}"),
        Some(&requester_token),
        None,
    )
    .await;
    assert_eq!(detail.status, StatusCode::OK, "{:?}", detail.json);
    assert_eq!(detail.json["status"], "SUSTAINED");
    assert_eq!(detail.json["diagnostics"].as_array().unwrap().len(), 1);
    assert_eq!(detail.json["findings"].as_array().unwrap().len(), 1);
    assert_eq!(detail.json["initiatives"].as_array().unwrap().len(), 1);
    assert_eq!(detail.json["observations"].as_array().unwrap().len(), 1);

    let history = send(
        service.clone(),
        "GET",
        &format!("/api/v1/consulting/engagements/{engagement_id}/history"),
        Some(&requester_token),
        None,
    )
    .await;
    assert_eq!(history.status, StatusCode::OK, "{:?}", history.json);
    assert_eq!(history.json.as_array().unwrap().len(), 10);

    let terminal_api_writes = [
        (
            format!("/api/v1/consulting/engagements/{engagement_id}/diagnostics"),
            json!({"summary": "must remain immutable"}),
        ),
        (
            format!("/api/v1/consulting/engagements/{engagement_id}/findings"),
            json!({
                "diagnosticId": diagnostic_id,
                "statement": "must remain immutable",
                "evidenceId": fixture.evidence,
                "documentId": fixture.document
            }),
        ),
        (
            format!("/api/v1/consulting/engagements/{engagement_id}/initiatives"),
            json!({
                "findingId": finding_id,
                "title": "must remain immutable",
                "hypothesis": "terminal records cannot gain initiatives",
                "kpiDefinitionId": fixture.kpi,
                "targetDirection": "DECREASE"
            }),
        ),
        (
            format!("/api/v1/consulting/engagements/{engagement_id}/observations"),
            json!({
                "initiativeId": initiative_id,
                "kpiDefinitionId": fixture.kpi,
                "evidenceId": fixture.evidence,
                "observedAt": "2026-07-23T13:00:00Z",
                "note": "must remain immutable"
            }),
        ),
    ];
    for (path, body) in terminal_api_writes {
        let response = send(
            service.clone(),
            "POST",
            &path,
            Some(&requester_token),
            Some(body),
        )
        .await;
        assert_eq!(response.status, StatusCode::CONFLICT, "{:?}", response.json);
        assert_eq!(response.json["error"]["code"], "conflict");
        assert_eq!(
            response.json["error"]["message"],
            "terminal consulting engagements are immutable"
        );
    }

    assert_terminal_runtime_writes_rejected(
        &runtime_pool,
        fixture.org,
        engagement_id,
        diagnostic_id,
        finding_id,
        initiative_id,
        observation_id,
    )
    .await;
    assert_terminal_owner_deletes_rejected(
        &owner_pool,
        engagement_id,
        diagnostic_id,
        finding_id,
        initiative_id,
        observation_id,
    )
    .await;
}

/// Replay records are immutable, so engagements created before the RFC 3339
/// switch keep `time`'s nine-element component array in `idempotency_response`
/// forever. Replaying one must still return its stored response, and must return
/// it as RFC 3339 — the array shape stays out of the wire contract.
#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn legacy_component_array_replay_survives_rfc3339_switch(owner_pool: PgPool) {
    let fixture = seed_fixture(&owner_pool).await;
    let keys = keys();
    let requester_token = bearer(&keys, fixture.requester, fixture.org);
    let runtime_pool = runtime_role_pool(&owner_pool).await;
    let service = build_router(app_state(runtime_pool, &keys));

    let create_body = json!({
        "customerId": fixture.customer,
        "customerDocumentId": fixture.document,
        "ontologyInstanceId": fixture.ontology,
        "title": "Engagement stored before the wire fix",
        "idempotencyKey": "consulting-legacy-replay"
    });
    let created = send(
        service.clone(),
        "POST",
        "/api/v1/consulting/engagements",
        Some(&requester_token),
        Some(create_body.clone()),
    )
    .await;
    assert_eq!(created.status, StatusCode::CREATED, "{:?}", created.json);
    let engagement_id = response_uuid(&created.json, "id");
    let created_at = created.json["created_at"].as_str().unwrap().to_owned();
    let updated_at = created.json["updated_at"].as_str().unwrap().to_owned();

    // Age the stored replay record into the pre-fix on-disk shape. Only the two
    // timestamps differ, so this is exactly what a pre-fix row still holds.
    let mut legacy = created.json.clone();
    legacy["created_at"] = legacy_time_components(&created_at);
    legacy["updated_at"] = legacy_time_components(&updated_at);
    assert!(legacy["created_at"].is_array(), "{legacy}");
    sqlx::query("UPDATE consulting_engagements SET idempotency_response=$1 WHERE id=$2")
        .bind(legacy)
        .bind(engagement_id)
        .execute(&owner_pool)
        .await
        .unwrap();

    let replayed = send(
        service.clone(),
        "POST",
        "/api/v1/consulting/engagements",
        Some(&requester_token),
        Some(create_body),
    )
    .await;
    assert_eq!(replayed.status, StatusCode::CREATED, "{:?}", replayed.json);
    assert_eq!(response_uuid(&replayed.json, "id"), engagement_id);
    assert_rfc3339(&replayed.json, "created_at");
    assert_rfc3339(&replayed.json, "updated_at");
    assert_eq!(replayed.json["created_at"], json!(created_at));
    assert_eq!(replayed.json["updated_at"], json!(updated_at));
    assert_eq!(
        count_engagements(&owner_pool, fixture.org, "consulting-legacy-replay").await,
        1
    );
}

/// `time`'s non-human-readable `OffsetDateTime` representation, which is what
/// `serde_json::to_value` produced before the RFC 3339 attributes landed: year,
/// ordinal, hour, minute, second, nanosecond, then the offset's whole hours,
/// minutes past the hour, and seconds past the minute.
fn legacy_time_components(rfc3339: &str) -> Value {
    let value = OffsetDateTime::parse(rfc3339, &Rfc3339).unwrap();
    json!([
        value.year(),
        value.ordinal(),
        value.hour(),
        value.minute(),
        value.second(),
        value.nanosecond(),
        value.offset().whole_hours(),
        value.offset().minutes_past_hour(),
        value.offset().seconds_past_minute()
    ])
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn terminal_transition_winning_after_child_precheck_returns_conflict_without_write(
    owner_pool: PgPool,
) {
    let fixture = seed_fixture(&owner_pool).await;
    let keys = keys();
    let requester_token = bearer(&keys, fixture.requester, fixture.org);
    let runtime_pool = runtime_role_pool(&owner_pool).await;
    let service = build_router(app_state(runtime_pool, &keys));

    let created = send(
        service.clone(),
        "POST",
        "/api/v1/consulting/engagements",
        Some(&requester_token),
        Some(json!({
            "customerId": fixture.customer,
            "customerDocumentId": fixture.document,
            "ontologyInstanceId": fixture.ontology,
            "title": "Exercise the terminal-write race",
            "idempotencyKey": "consulting-terminal-race"
        })),
    )
    .await;
    assert_eq!(created.status, StatusCode::CREATED, "{:?}", created.json);
    let engagement_id = response_uuid(&created.json, "id");

    let gate_key = (Uuid::new_v4().as_u128() & i64::MAX as u128) as i64;
    // PostgreSQL orders same-event triggers by name: `pause` runs after the
    // handler precheck and before the migration's `terminal` trigger.
    sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
        "CREATE OR REPLACE FUNCTION consulting_test_pause_diagnostic_insert() \
         RETURNS TRIGGER LANGUAGE plpgsql AS $$ \
         BEGIN \
           PERFORM pg_advisory_xact_lock({gate_key}); \
           RETURN NEW; \
         END; \
         $$; \
         CREATE TRIGGER trg_consulting_diagnostics_pause \
           BEFORE INSERT ON consulting_diagnostics \
           FOR EACH ROW EXECUTE FUNCTION consulting_test_pause_diagnostic_insert();"
    )))
    .execute(&owner_pool)
    .await
    .unwrap();

    let mut gate = owner_pool.begin().await.unwrap();
    let gate_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(gate.as_mut())
        .await
        .unwrap();
    sqlx::query("SELECT pg_advisory_xact_lock($1)")
        .bind(gate_key)
        .execute(gate.as_mut())
        .await
        .unwrap();

    let request_service = service.clone();
    let request_token = requester_token.clone();
    let request_path = format!("/api/v1/consulting/engagements/{engagement_id}/diagnostics");
    let request = tokio::spawn(async move {
        send(
            request_service,
            "POST",
            &request_path,
            Some(&request_token),
            Some(json!({"summary": "must lose to terminal transition"})),
        )
        .await
    });

    wait_for_advisory_waiter(&owner_pool, gate_pid).await;
    sqlx::query(
        "UPDATE consulting_engagements \
         SET status = 'SUSTAINED', version = version + 1, updated_at = now() \
         WHERE id = $1",
    )
    .bind(engagement_id)
    .execute(&owner_pool)
    .await
    .unwrap();

    gate.commit().await.unwrap();

    let response = tokio::time::timeout(std::time::Duration::from_secs(10), request)
        .await
        .expect("diagnostic request remained blocked")
        .unwrap();
    assert_eq!(response.status, StatusCode::CONFLICT, "{:?}", response.json);
    assert_eq!(response.json["error"]["code"], "conflict");
    assert_eq!(
        response.json["error"]["message"],
        "terminal consulting engagements are immutable"
    );

    let child_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM consulting_diagnostics WHERE engagement_id = $1")
            .bind(engagement_id)
            .fetch_one(&owner_pool)
            .await
            .unwrap();
    assert_eq!(child_count, 0, "terminal race must not write a child");
    let status: String =
        sqlx::query_scalar("SELECT status FROM consulting_engagements WHERE id = $1")
            .bind(engagement_id)
            .fetch_one(&owner_pool)
            .await
            .unwrap();
    assert_eq!(status, "SUSTAINED");
}

async fn wait_for_advisory_waiter(owner_pool: &PgPool, blocker_pid: i32) {
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            let blocked: bool = sqlx::query_scalar(
                "SELECT EXISTS ( \
                   SELECT 1 FROM pg_locks \
                   WHERE locktype = 'advisory' \
                     AND NOT granted \
                     AND $1 = ANY(pg_blocking_pids(pid)) \
                 )",
            )
            .bind(blocker_pid)
            .fetch_one(owner_pool)
            .await
            .unwrap();
            if blocked {
                return;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("diagnostic insert never reached the advisory-lock barrier");
}

async fn assert_terminal_runtime_writes_rejected(
    pool: &PgPool,
    org: OrgId,
    engagement_id: Uuid,
    diagnostic_id: Uuid,
    finding_id: Uuid,
    initiative_id: Uuid,
    observation_id: Uuid,
) {
    let updates = [
        (
            "UPDATE consulting_engagements SET status='MEASURED' WHERE id=$1",
            engagement_id,
        ),
        (
            "UPDATE consulting_diagnostics SET summary=summary WHERE id=$1",
            diagnostic_id,
        ),
        (
            "UPDATE consulting_findings SET statement=statement WHERE id=$1",
            finding_id,
        ),
        (
            "UPDATE consulting_initiatives SET title=title WHERE id=$1",
            initiative_id,
        ),
        (
            "UPDATE consulting_benefit_observations SET note=note WHERE id=$1",
            observation_id,
        ),
    ];
    for (statement, id) in updates {
        assert_terminal_trigger(pool, org, statement, id).await;
    }

    let inserts = [
        (
            "INSERT INTO consulting_diagnostics \
             (org_id, engagement_id, summary, document_id, created_by) \
             SELECT org_id, engagement_id, summary, document_id, created_by \
             FROM consulting_diagnostics WHERE id=$1",
            diagnostic_id,
        ),
        (
            "INSERT INTO consulting_findings \
             (org_id, engagement_id, diagnostic_id, statement, evidence_id, document_id, created_by) \
             SELECT org_id, engagement_id, diagnostic_id, statement, evidence_id, document_id, created_by \
             FROM consulting_findings WHERE id=$1",
            finding_id,
        ),
        (
            "INSERT INTO consulting_initiatives \
             (org_id, engagement_id, finding_id, title, hypothesis, kpi_definition_id, target_direction, created_by) \
             SELECT org_id, engagement_id, finding_id, title, hypothesis, kpi_definition_id, target_direction, created_by \
             FROM consulting_initiatives WHERE id=$1",
            initiative_id,
        ),
        (
            "INSERT INTO consulting_benefit_observations \
             (org_id, engagement_id, initiative_id, kpi_definition_id, evidence_id, observed_at, note, created_by) \
             SELECT org_id, engagement_id, initiative_id, kpi_definition_id, evidence_id, observed_at, note, created_by \
             FROM consulting_benefit_observations WHERE id=$1",
            observation_id,
        ),
    ];
    for (statement, id) in inserts {
        assert_terminal_trigger(pool, org, statement, id).await;
    }

    for (table, id) in [
        ("consulting_engagements", engagement_id),
        ("consulting_diagnostics", diagnostic_id),
        ("consulting_findings", finding_id),
        ("consulting_initiatives", initiative_id),
        ("consulting_benefit_observations", observation_id),
    ] {
        let error = execute_as_org(pool, org, consulting_delete_statement(table), id)
            .await
            .expect_err("runtime role must not delete consulting records");
        assert!(
            error.to_string().contains("permission denied"),
            "unexpected runtime DELETE error for {table}: {error}"
        );
    }
}

fn consulting_delete_statement(table: &str) -> &'static str {
    match table {
        "consulting_engagements" => "DELETE FROM consulting_engagements WHERE id=$1",
        "consulting_diagnostics" => "DELETE FROM consulting_diagnostics WHERE id=$1",
        "consulting_findings" => "DELETE FROM consulting_findings WHERE id=$1",
        "consulting_initiatives" => "DELETE FROM consulting_initiatives WHERE id=$1",
        "consulting_benefit_observations" => {
            "DELETE FROM consulting_benefit_observations WHERE id=$1"
        }
        _ => panic!("unknown consulting table in deletion allowlist: {table}"),
    }
}

async fn assert_terminal_owner_deletes_rejected(
    pool: &PgPool,
    engagement_id: Uuid,
    diagnostic_id: Uuid,
    finding_id: Uuid,
    initiative_id: Uuid,
    observation_id: Uuid,
) {
    for (table, id) in [
        ("consulting_engagements", engagement_id),
        ("consulting_diagnostics", diagnostic_id),
        ("consulting_findings", finding_id),
        ("consulting_initiatives", initiative_id),
        ("consulting_benefit_observations", observation_id),
    ] {
        let error = sqlx::query(consulting_delete_statement(table))
            .bind(id)
            .execute(pool)
            .await
            .expect_err("terminal trigger must reject owner delete");
        assert_terminal_database_error(&error, &format!("owner DELETE on {table}"));
    }
}

async fn assert_terminal_trigger(pool: &PgPool, org: OrgId, statement: &'static str, id: Uuid) {
    let error = execute_as_org(pool, org, statement, id)
        .await
        .expect_err("terminal trigger must reject write");
    assert_terminal_database_error(&error, "runtime terminal write");
}

fn assert_terminal_database_error(error: &sqlx::Error, operation: &str) {
    let sqlx::Error::Database(database) = error else {
        panic!("unexpected {operation} error: {error}");
    };
    assert_eq!(
        database.code().as_deref(),
        Some("P0001"),
        "unexpected {operation} SQLSTATE"
    );
    assert_eq!(
        database.constraint(),
        Some("consulting_terminal_immutable"),
        "unexpected {operation} constraint identity"
    );
    assert_eq!(
        database.message(),
        "terminal consulting engagement is immutable",
        "unexpected {operation} message"
    );
}

async fn execute_as_org(
    pool: &PgPool,
    org: OrgId,
    statement: &'static str,
    id: Uuid,
) -> Result<sqlx::postgres::PgQueryResult, sqlx::Error> {
    let mut tx = pool.begin().await?;
    sqlx::query("SELECT set_config('app.current_org', $1, true)")
        .bind(org.as_uuid().to_string())
        .execute(tx.as_mut())
        .await?;
    let result = sqlx::query(statement).bind(id).execute(tx.as_mut()).await;
    match result {
        Ok(result) => {
            tx.commit().await?;
            Ok(result)
        }
        Err(error) => {
            let _ = tx.rollback().await;
            Err(error)
        }
    }
}

async fn transition(
    service: axum::Router,
    engagement_id: Uuid,
    token: &str,
    to_status: &str,
    expected_version: i64,
    approval_id: Option<Uuid>,
) -> JsonResponse {
    send(
        service,
        "POST",
        &format!("/api/v1/consulting/engagements/{engagement_id}/transition"),
        Some(token),
        Some(json!({
            "toStatus": to_status,
            "expectedVersion": expected_version,
            "approvalId": approval_id,
            "reason": format!("advance to {to_status}")
        })),
    )
    .await
}

async fn send(
    service: axum::Router,
    method: &str,
    uri: &str,
    token: Option<&str>,
    body: Option<Value>,
) -> JsonResponse {
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(token) = token {
        builder = builder.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }
    if body.is_some() {
        builder = builder.header(header::CONTENT_TYPE, "application/json");
    }
    let response = service
        .oneshot(
            builder
                .body(body.map_or_else(Body::empty, |body| Body::from(body.to_string())))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let json = serde_json::from_slice(&bytes).unwrap_or_else(|_| json!({}));
    JsonResponse { status, json }
}

fn response_uuid(value: &Value, field: &str) -> Uuid {
    value[field]
        .as_str()
        .unwrap_or_else(|| panic!("response field {field} must be a UUID: {value}"))
        .parse()
        .unwrap()
}

/// The published schema declares every engagement timestamp as
/// `{type: string, format: date-time}`; time's default `OffsetDateTime` impl
/// emits a nine-element integer array instead, which this rejects.
fn assert_rfc3339(value: &Value, field: &str) {
    let raw = value[field]
        .as_str()
        .unwrap_or_else(|| panic!("response field {field} must be an RFC 3339 string: {value}"));
    OffsetDateTime::parse(raw, &Rfc3339)
        .unwrap_or_else(|error| panic!("response field {field} is not RFC 3339: {raw} ({error})"));
}

fn keys() -> Keys {
    let signing_key = SigningKey::random(&mut OsRng);
    Keys {
        private_pem: signing_key
            .to_pkcs8_pem(LineEnding::LF)
            .unwrap()
            .to_string(),
        public_pem: signing_key
            .verifying_key()
            .to_public_key_pem(LineEnding::LF)
            .unwrap(),
    }
}

fn bearer(keys: &Keys, user_id: UserId, org: OrgId) -> String {
    JwtIssuer::from_es256_pem(
        JwtSettings {
            issuer: TEST_ISSUER.to_owned(),
            audience: TEST_AUDIENCE.to_owned(),
            access_token_ttl: Duration::minutes(15),
        },
        keys.private_pem.as_bytes(),
        keys.public_pem.as_bytes(),
    )
    .unwrap()
    .issue_access_token(AccessTokenInput {
        subject: user_id,
        org_id: org,
        roles: vec!["SUPER_ADMIN".to_owned()],
        branches: Vec::<BranchId>::new(),
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

fn app_state(pool: PgPool, keys: &Keys) -> AppState {
    let config = AppConfig::from_pairs([
        ("CONSOLE_APP_ROLE", AppRole::Api.to_string()),
        ("CONSOLE_HTTP_ADDR", "127.0.0.1:0".to_owned()),
        ("CONSOLE_JWT_ISSUER", TEST_ISSUER.to_owned()),
        ("CONSOLE_JWT_AUDIENCE", TEST_AUDIENCE.to_owned()),
        ("CONSOLE_JWT_PUBLIC_KEY_PEM", keys.public_pem.clone()),
    ])
    .unwrap();
    AppState::new(config, DatabaseDependency::Postgres(pool)).unwrap()
}

async fn runtime_role_pool(owner_pool: &PgPool) -> PgPool {
    PgPoolOptions::new()
        .max_connections(8)
        .after_connect(|connection, _meta| {
            Box::pin(async move {
                sqlx::query("SET ROLE console_rt")
                    .execute(connection)
                    .await?;
                Ok(())
            })
        })
        .connect_with(owner_pool.connect_options().as_ref().clone())
        .await
        .unwrap()
}

async fn seed_fixture(pool: &PgPool) -> Fixture {
    let org = OrgId::knl();
    let other_org = OrgId::from_uuid(Uuid::new_v4());
    seed_org(pool, org, "knl", "KNL").await;
    seed_org(pool, other_org, "consulting-other", "Consulting Other").await;

    let requester = UserId::new();
    let approver = UserId::new();
    let other_user = UserId::new();
    seed_user(pool, org, requester, "Consulting Requester").await;
    seed_user(pool, org, approver, "Consulting Approver").await;
    seed_user(pool, other_org, other_user, "Other Tenant User").await;
    seed_consulting_policy(pool, org, &[requester, approver]).await;

    let branch = seed_branch(pool, org, "Consulting Primary").await;
    let other_branch = seed_branch(pool, other_org, "Consulting Other").await;
    let customer = seed_customer(pool, org, branch, "Consulting Customer").await;
    let other_customer =
        seed_customer(pool, other_org, other_branch, "Other Tenant Customer").await;

    let document = seed_reference(pool, org, requester, "DOCUMENT").await;
    let ontology = seed_reference(pool, org, requester, "ONTOLOGY_INSTANCE").await;
    let evidence = seed_reference(pool, org, requester, "EVIDENCE").await;
    let kpi = seed_reference(pool, org, requester, "KPI_DEFINITION").await;

    let other_org_engagement: Uuid = sqlx::query_scalar(
        "INSERT INTO consulting_engagements \
         (org_id, customer_id, title, idempotency_key, idempotency_request_hash, created_by) \
         VALUES ($1, $2, 'Other tenant engagement', 'other-tenant-key', \
         'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa', $3) \
         RETURNING id",
    )
    .bind(*other_org.as_uuid())
    .bind(other_customer)
    .bind(*other_user.as_uuid())
    .fetch_one(pool)
    .await
    .unwrap();

    Fixture {
        org,
        other_org_engagement,
        requester,
        approver,
        customer,
        document,
        ontology,
        evidence,
        kpi,
    }
}

async fn seed_org(pool: &PgPool, org: OrgId, slug: &str, name: &str) {
    sqlx::query(
        "INSERT INTO organizations (id, slug, name) VALUES ($1, $2, $3) \
         ON CONFLICT (id) DO NOTHING",
    )
    .bind(*org.as_uuid())
    .bind(slug)
    .bind(name)
    .execute(pool)
    .await
    .unwrap();
}

async fn seed_user(pool: &PgPool, org: OrgId, user: UserId, name: &str) {
    sqlx::query(
        "INSERT INTO users (id, display_name, roles, is_active, org_id) \
         VALUES ($1, $2, $3, true, $4)",
    )
    .bind(*user.as_uuid())
    .bind(name)
    .bind(vec!["SUPER_ADMIN".to_owned()])
    .bind(*org.as_uuid())
    .execute(pool)
    .await
    .unwrap();
}

async fn seed_consulting_policy(pool: &PgPool, org: OrgId, users: &[UserId]) {
    let role_id: Uuid = sqlx::query_scalar(
        "INSERT INTO policy_roles \
         (org_id, role_key, display_name, status, is_system) \
         VALUES ($1, 'consulting_test_operator', 'Consulting test operator', 'ACTIVE', false) \
         RETURNING id",
    )
    .bind(*org.as_uuid())
    .fetch_one(pool)
    .await
    .unwrap();
    for feature in ["consulting_read", "consulting_manage"] {
        sqlx::query(
            "INSERT INTO policy_role_permissions \
             (org_id, role_id, feature_key, permission_level) \
             VALUES ($1, $2, $3, 'allow')",
        )
        .bind(*org.as_uuid())
        .bind(role_id)
        .bind(feature)
        .execute(pool)
        .await
        .unwrap();
    }
    for user in users {
        sqlx::query(
            "INSERT INTO user_role_assignments (org_id, user_id, role_id) \
             VALUES ($1, $2, $3)",
        )
        .bind(*org.as_uuid())
        .bind(*user.as_uuid())
        .bind(role_id)
        .execute(pool)
        .await
        .unwrap();
    }
}

async fn seed_branch(pool: &PgPool, org: OrgId, name: &str) -> Uuid {
    let region: Uuid =
        sqlx::query_scalar("INSERT INTO regions (org_id, name) VALUES ($1, $2) RETURNING id")
            .bind(*org.as_uuid())
            .bind(format!("{name} Region"))
            .fetch_one(pool)
            .await
            .unwrap();
    sqlx::query_scalar(
        "INSERT INTO branches (org_id, region_id, name) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(*org.as_uuid())
    .bind(region)
    .bind(name)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn seed_customer(pool: &PgPool, org: OrgId, branch: Uuid, name: &str) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO registry_customers (org_id, branch_id, name) \
         VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(*org.as_uuid())
    .bind(branch)
    .bind(name)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn seed_reference(pool: &PgPool, org: OrgId, actor: UserId, kind: &str) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO consulting_reference_bindings \
         (org_id, source_kind, source_id, source_version, evaluated_by) \
         VALUES ($1, $2, $3, 'v1', $4) RETURNING id",
    )
    .bind(*org.as_uuid())
    .bind(kind)
    .bind(Uuid::new_v4())
    .bind(*actor.as_uuid())
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn seed_approval(
    pool: &PgPool,
    org: OrgId,
    requester: UserId,
    approver: UserId,
    engagement_id: Uuid,
) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO gov_approvals \
         (org_id, request_ref, kind, requested_by, approver_id, decision, target_ref) \
         VALUES ($1, $2, 'consulting.engagement.approval', $3, $4, 'approved', $5) \
         RETURNING id",
    )
    .bind(*org.as_uuid())
    .bind(Uuid::new_v4())
    .bind(*requester.as_uuid())
    .bind(*approver.as_uuid())
    .bind(engagement_id)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn count_engagements(pool: &PgPool, org: OrgId, key: &str) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM consulting_engagements WHERE org_id=$1 AND idempotency_key=$2",
    )
    .bind(*org.as_uuid())
    .bind(key)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn approval_consumptions(pool: &PgPool, approval_id: Uuid) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM gov_approval_consumptions WHERE approval_id=$1")
        .bind(approval_id)
        .fetch_one(pool)
        .await
        .unwrap()
}
