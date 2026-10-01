//! Existing staged population and local transaction proof, not legal payroll,
//! independent-person proofing, bank execution, browser or two-site durability.
use super::*;
use console_inbox_adapter_postgres::PgInboxStore;
use console_inbox_application::{EmitInboxDocCommand, GetInboxDocQuery};
use console_inbox_domain::{InboxDocKind, NewInboxDoc};
use console_kernel_core::{InboxDocId, TraceContext};
use console_payroll_adapter_postgres::lifecycle::{self, LifecycleError};
use sqlx::{Postgres, Transaction};

struct Population {
    rt: PgPool,
    keys: Keys,
    org: OrgId,
    actor: UserId,
    decider: UserId,
    token: String,
    run: Uuid,
    // Stable staged line, employee, current recipient, expected payload.
    members: Vec<(Uuid, Uuid, UserId, Value)>,
}

async fn population(owner: &PgPool, size: usize) -> Population {
    let org = OrgId::knl();
    seed_org(owner, org).await;
    let actor = seed_user(owner, org, "EXECUTIVE", None).await;
    let decider = seed_user(owner, org, "EXECUTIVE", None).await;
    let rt = runtime_role_pool(owner).await;
    let keys = Keys::generate();
    let token = keys.token(actor, org, "EXECUTIVE");
    let run = seed_run(owner, org, actor).await;
    let mut members = Vec::new();
    for index in 0..size {
        let employee = seed_employee(owner, org, &format!("Person {index}")).await;
        let recipient = seed_user(owner, org, "MEMBER", Some(employee)).await;
        let source = seed_verified_import_row(owner, org).await;
        seed_calculable_line(owner, org, run, employee, source).await;
        let line: Uuid = sqlx::query_scalar(
            "SELECT id FROM payroll_draft_lines WHERE run_id = $1 AND employee_id = $2",
        )
        .bind(run)
        .bind(employee)
        .fetch_one(owner)
        .await
        .unwrap();
        // Eliminate overtime exceptions only in fixture inputs, before calculation.
        sqlx::query("UPDATE payroll_draft_lines SET overtime_hours = 0 WHERE id = $1")
            .bind(line)
            .execute(owner)
            .await
            .unwrap();
        members.push((line, employee, recipient, Value::Null));
    }
    if size > 0 {
        seed_period_lock(owner, org).await;
        let (status, body) = send(
            &rt,
            &keys,
            "POST",
            &format!("/api/v1/payroll/runs/{run}/close-attendance"),
            &token,
            Some(json!({"attest":true})),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let (status, body) = send(
            &rt,
            &keys,
            "POST",
            &format!("/api/v1/payroll/runs/{run}/calculate"),
            &token,
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["calculation"]["calculated_lines"], json!(size));
        assert_eq!(body["exceptions_open"], 0);
        for (line, _, _, payload) in &mut members {
            let (gross, deductions, total, net, tax, version): (i64, Value, i64, i64, String, i32) = sqlx::query_as(
                "SELECT gross_won, deductions, total_deductions_won, net_won, tax_table_version, version FROM payroll_line_calculations WHERE run_id = $1 AND line_id = $2",
            ).bind(run).bind(*line).fetch_one(owner).await.unwrap();
            assert_eq!(
                (gross, total, net, version),
                (3_000_000, 373_300, 2_626_700, 1)
            );
            *payload = json!({"run_id":run,"line_id":line,"period_start":"2026-06-01","period_end":"2026-06-30",
                "gross_won":gross,"deductions":deductions,"total_deductions_won":total,"net_won":net,
                "tax_table_version":tax,"calculation_version":version});
        }
    } else {
        sqlx::query("UPDATE payroll_draft_runs SET status = 'CALCULATED' WHERE id = $1")
            .bind(run)
            .execute(owner)
            .await
            .unwrap();
    }
    Population {
        rt,
        keys,
        org,
        actor,
        decider,
        token,
        run,
        members,
    }
}

async fn paid_fixture(owner: &PgPool, p: &Population) {
    // Local publication prerequisite only. This never performs/claims a bank effect.
    sqlx::query("UPDATE payroll_draft_runs SET status = 'PAID' WHERE id = $1")
        .bind(p.run)
        .execute(owner)
        .await
        .unwrap();
    register_release_gate(owner, p.run).await;
}

async fn post(p: &Population, action: &str, body: Option<Value>) -> (StatusCode, Value) {
    send(
        &p.rt,
        &p.keys,
        "POST",
        &format!("/api/v1/payroll/runs/{}/{action}", p.run),
        &p.token,
        body,
    )
    .await
}

async fn tx(p: &Population) -> Transaction<'static, Postgres> {
    let mut tx = p.rt.begin().await.unwrap();
    let role: (String, bool, bool) = sqlx::query_as("SELECT current_user::text, rolsuper, rolbypassrls FROM pg_roles WHERE rolname = current_user")
        .fetch_one(&mut *tx).await.unwrap();
    assert_eq!(role, ("console_rt".to_owned(), false, false));
    sqlx::query("SELECT set_config('app.current_org', $1, true)")
        .bind(p.org.to_string())
        .execute(&mut *tx)
        .await
        .unwrap();
    tx
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn defective_population_cannot_submit_or_issue_and_has_no_total(owner: PgPool) {
    // Each case has a fresh run; owner-only fixture corruption is not a new writer.
    for defect in [
        "empty",
        "partial",
        "blocked",
        "non-array",
        "not-ready",
        "unresolved",
        "duplicate-person",
        "mixed-version",
        "wrong-run",
        "extraneous",
    ] {
        let p = population(&owner, if defect == "empty" { 0 } else { 2 }).await;
        if !p.members.is_empty() {
            let (line, employee, _, _) = &p.members[1];
            match defect {
                "partial" => {
                    sqlx::query("DELETE FROM payroll_line_calculations WHERE line_id = $1")
                        .bind(line)
                        .execute(&owner)
                        .await
                        .unwrap();
                }
                "blocked" | "non-array" => {
                    sqlx::query("UPDATE payroll_draft_lines SET blockers = $2 WHERE id = $1")
                        .bind(line)
                        .bind(if defect == "blocked" {
                            json!(["TEST_BLOCKER"])
                        } else {
                            json!({})
                        })
                        .execute(&owner)
                        .await
                        .unwrap();
                }
                "not-ready" => {
                    sqlx::query("UPDATE payroll_draft_lines SET calculation_status = 'BLOCKED_LEGAL_GATE' WHERE id = $1").bind(line).execute(&owner).await.unwrap();
                }
                "unresolved" => {
                    sqlx::query("UPDATE payroll_draft_lines SET employee_id = NULL WHERE id = $1")
                        .bind(line)
                        .execute(&owner)
                        .await
                        .unwrap();
                }
                "duplicate-person" => {
                    sqlx::query("UPDATE payroll_draft_lines SET employee_id = $2 WHERE id = $1")
                        .bind(line)
                        .bind(p.members[0].1)
                        .execute(&owner)
                        .await
                        .unwrap();
                }
                "mixed-version" => {
                    copy_calculation(&owner, p.run, p.members[0].0, 2).await;
                }
                "wrong-run" | "extraneous" => {
                    // Same raw count, but the second calculation references another run's line.
                    let foreign_run = seed_run(&owner, p.org, p.actor).await;
                    let source = seed_verified_import_row(&owner, p.org).await;
                    seed_calculable_line(&owner, p.org, foreign_run, *employee, source).await;
                    let other_line: Uuid =
                        sqlx::query_scalar("SELECT id FROM payroll_draft_lines WHERE run_id = $1")
                            .bind(foreign_run)
                            .fetch_one(&owner)
                            .await
                            .unwrap();
                    if defect == "extraneous" {
                        sqlx::query("INSERT INTO payroll_line_calculations (org_id,run_id,line_id,version,gross_won,deductions,total_deductions_won,net_won,tax_table_version) SELECT org_id,run_id,$2,version,gross_won,deductions,total_deductions_won,net_won,tax_table_version FROM payroll_line_calculations WHERE line_id=$1")
                            .bind(line).bind(other_line).execute(&owner).await.unwrap();
                    } else {
                        sqlx::query(
                            "UPDATE payroll_line_calculations SET line_id = $2 WHERE line_id = $1",
                        )
                        .bind(line)
                        .bind(other_line)
                        .execute(&owner)
                        .await
                        .unwrap();
                    }
                }
                _ => unreachable!(),
            }
        }
        let before = content_digest_of_every_table(&owner).await;
        let (status, body) = post(&p, "submit", None).await;
        assert_eq!(status, StatusCode::CONFLICT, "defect={defect}: {body}");
        assert_eq!(
            body["error"]["code"], "invalid_state",
            "defect={defect}: {body}"
        );
        assert_eq!(
            content_digest_of_every_table(&owner).await,
            before,
            "defect={defect}"
        );
        let mut conn = tx(&p).await;
        // Deliberately stale caller count: the owner must count its own roster.
        let summary = lifecycle::latest_calc_summary_in_tx(&mut conn, p.run, 999)
            .await
            .unwrap();
        assert!(
            summary.is_none_or(|s| s.total_net_won.is_none()),
            "defect={defect}: no misleading total"
        );
        conn.rollback().await.unwrap();
        paid_fixture(&owner, &p).await;
        let before = content_digest_of_every_table(&owner).await;
        let (status, body) = post(&p, "issue-payslips", None).await;
        assert_eq!(status, StatusCode::CONFLICT, "defect={defect}: {body}");
        assert_eq!(body["error"]["code"], "invalid_state");
        assert_eq!(
            content_digest_of_every_table(&owner).await,
            before,
            "defect={defect}: publication leaked"
        );
    }
}

async fn copy_calculation(owner: &PgPool, run: Uuid, line: Uuid, version: i32) {
    sqlx::query("INSERT INTO payroll_line_calculations (org_id, run_id, line_id, version, gross_won, deductions, total_deductions_won, net_won, tax_table_version) \
                SELECT org_id, run_id, line_id, $3, gross_won, deductions, total_deductions_won, net_won, tax_table_version FROM payroll_line_calculations WHERE run_id = $1 AND line_id = $2 AND version = 1")
        .bind(run).bind(line).bind(version).execute(owner).await.unwrap();
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn approval_rechecks_population_and_exceptions_but_rejection_remains_available(
    owner: PgPool,
) {
    for defect in ["blocked", "duplicate-person", "exception"] {
        let p = population(&owner, 2).await;
        let (status, body) = post(&p, "submit", None).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let line = p.members[1].0;
        match defect {
            "blocked" => {
                sqlx::query(
                    "UPDATE payroll_draft_lines SET blockers = '[\"changed\"]' WHERE id = $1",
                )
                .bind(line)
                .execute(&owner)
                .await
                .unwrap();
            }
            "duplicate-person" => {
                sqlx::query("UPDATE payroll_draft_lines SET employee_id = $2 WHERE id = $1")
                    .bind(line)
                    .bind(p.members[0].1)
                    .execute(&owner)
                    .await
                    .unwrap();
            }
            _ => {
                sqlx::query("INSERT INTO payroll_run_exceptions (org_id, run_id, line_id, employee_display_name, kind, severity, summary_ko) VALUES ($1,$2,$3,'Alice','ACCOUNT_VERIFICATION','warn','확인 필요')")
                .bind(p.org.as_uuid()).bind(p.run).bind(line).execute(&owner).await.unwrap();
            }
        }
        let before = content_digest_of_every_table(&owner).await;
        let token = p.keys.token(p.decider, p.org, "EXECUTIVE");
        let (status, body) = send(
            &p.rt,
            &p.keys,
            "POST",
            &format!("/api/v1/payroll/runs/{}/decision", p.run),
            &token,
            Some(json!({"decision":"APPROVE"})),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT, "{defect}: {body}");
        assert_eq!(
            body["error"]["code"],
            if defect == "exception" {
                "exceptions_open"
            } else {
                "invalid_state"
            }
        );
        assert_eq!(content_digest_of_every_table(&owner).await, before);
        let (status, sod) = post(&p, "decision", Some(json!({"decision":"APPROVE"}))).await;
        assert_eq!(status, StatusCode::CONFLICT, "{sod}");
        assert_eq!(
            sod["error"]["code"], "sod_violation",
            "SoD takes precedence"
        );
        let (status, body) = send(
            &p.rt,
            &p.keys,
            "POST",
            &format!("/api/v1/payroll/runs/{}/decision", p.run),
            &token,
            Some(json!({"decision":"REJECT", "reason":"자료 재확인"})),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{defect}: {body}");
        assert_eq!(body["run"]["status"], "REJECTED");
    }
}

async fn emit_matching(p: &Population, member: usize, payload: Value) -> Uuid {
    let (line, _, recipient, _) = &p.members[member];
    let store = PgInboxStore::new(p.rt.clone());
    let doc = NewInboxDoc::new(
        InboxDocKind::Payslip,
        "급여명세서 2026-06-01 ~ 2026-06-30 · Alice",
        None,
        None,
        Some("payroll_run"),
        Some(&p.run.to_string()),
        payload,
    )
    .unwrap();
    let result = console_platform_request_context::scope_org(p.org, async {
        store
            .emit_inbox_doc(EmitInboxDocCommand {
                actor: Some(p.actor),
                recipient: *recipient,
                doc,
                dedup_key: Some(format!("payroll-run:{}:line:{line}", p.run)),
                trace: TraceContext::generate(),
                occurred_at: OffsetDateTime::now_utc(),
            })
            .await
    })
    .await
    .unwrap();
    *result.id.as_uuid()
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn complete_current_version_publication_includes_exited_workers_and_reuses_exact_legacy_artifacts(
    owner: PgPool,
) {
    let p = population(&owner, 2).await;
    let mut current = tx(&p).await;
    let summary = lifecycle::latest_calc_summary_in_tx(&mut current, p.run, 999)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (
            summary.calculated_lines,
            summary.blocked_lines,
            summary.total_net_won
        ),
        (2, 0, Some(5_253_400)),
        "summary uses actual current roster, never a stale caller count"
    );
    assert!(!summary.payable);
    current.rollback().await.unwrap();
    sqlx::query(
        "UPDATE employees SET employment_status = 'EXITED', exit_date = '2026-06-30' WHERE id = $1",
    )
    .bind(p.members[1].1)
    .execute(&owner)
    .await
    .unwrap();
    for (line, _, _, payload) in &p.members {
        copy_calculation(&owner, p.run, *line, 2).await;
        assert_eq!(payload["calculation_version"], 1);
    }
    paid_fixture(&owner, &p).await;
    let mut expected = p.members.iter().map(|m| m.3.clone()).collect::<Vec<_>>();
    for payload in &mut expected {
        payload["calculation_version"] = json!(2);
    }
    let legacy = emit_matching(&p, 0, expected[0].clone()).await;
    sqlx::query("INSERT INTO payroll_payslip_deliveries (org_id,run_id,line_id,employee_id,inbox_doc_id) VALUES ($1,$2,$3,$4,$5)")
        .bind(p.org.as_uuid()).bind(p.run).bind(p.members[0].0).bind(p.members[0].1).bind(legacy).execute(&owner).await.unwrap();
    let (status, body) = post(&p, "issue-payslips", None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["issued"], 2);
    for (index, (line, employee, recipient, _)) in p.members.iter().enumerate() {
        let (doc, actual, dest): (Uuid,Value,Uuid) = sqlx::query_as("SELECT i.id, i.payload, i.recipient_user_id FROM payroll_payslip_deliveries d JOIN inbox_docs i ON i.id = d.inbox_doc_id WHERE d.run_id = $1 AND d.line_id = $2 AND d.employee_id = $3")
            .bind(p.run).bind(line).bind(employee).fetch_one(&owner).await.unwrap();
        assert_eq!(actual, expected[index]);
        assert_eq!(dest, *recipient.as_uuid());
        if index == 0 {
            assert_eq!(doc, legacy);
        }
        let own = console_platform_request_context::scope_org(p.org, async {
            PgInboxStore::new(p.rt.clone())
                .get(GetInboxDocQuery {
                    recipient: *recipient,
                    id: InboxDocId::from_uuid(doc),
                })
                .await
        })
        .await
        .unwrap();
        assert_eq!(own.payload, Some(expected[index].clone()));
    }
    let counts: (i64,i64,i64) = sqlx::query_as("SELECT (SELECT COUNT(*) FROM inbox_docs WHERE source_id = $1), (SELECT COUNT(*) FROM audit_events WHERE action = 'inbox_doc.emit'), (SELECT COUNT(*) FROM audit_events WHERE action = 'payroll_run.payslip_issue')")
        .bind(p.run.to_string()).fetch_one(&owner).await.unwrap();
    assert_eq!(counts, (2, 2, 1));
    let before = content_digest_of_every_table(&owner).await;
    let (status, body) = post(&p, "issue-payslips", None).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(content_digest_of_every_table(&owner).await, before);
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn missing_recipient_or_conflicting_artifact_never_publishes_a_partial_batch(owner: PgPool) {
    for defect in ["unlinked", "conflicting-dedup"] {
        let p = population(&owner, 2).await;
        paid_fixture(&owner, &p).await;
        if defect == "unlinked" {
            sqlx::query("UPDATE users SET employee_id = NULL WHERE id = $1")
                .bind(p.members[1].2.as_uuid())
                .execute(&owner)
                .await
                .unwrap();
        } else {
            let mut payload = p.members[1].3.clone();
            payload["calculation_version"] = json!(99);
            emit_matching(&p, 1, payload).await;
        }
        let before = content_digest_of_every_table(&owner).await;
        let (status, body) = post(&p, "issue-payslips", None).await;
        assert_eq!(status, StatusCode::CONFLICT, "{defect}: {body}");
        assert_eq!(
            content_digest_of_every_table(&owner).await,
            before,
            "{defect}: no new disclosure/links/audits/status"
        );
        assert!(
            !body.to_string().contains("2626700"),
            "refusal must not echo pay"
        );
    }
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn second_document_database_failure_rolls_back_every_new_publication(owner: PgPool) {
    let p = population(&owner, 2).await;
    paid_fixture(&owner, &p).await;
    // A real DB fault after one document insert, in either old or repaired path.
    sqlx::raw_sql("CREATE FUNCTION test_fail_second_payslip() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.kind = 'payslip' AND EXISTS (SELECT 1 FROM inbox_docs WHERE source_id = NEW.source_id AND kind = 'payslip') THEN RAISE EXCEPTION 'test second-document failure'; END IF; RETURN NEW; END $$; CREATE TRIGGER test_fail_second_payslip BEFORE INSERT ON inbox_docs FOR EACH ROW EXECUTE FUNCTION test_fail_second_payslip();")
        .execute(&owner).await.unwrap();
    let before = content_digest_of_every_table(&owner).await;
    let (status, body) = post(&p, "issue-payslips", None).await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{body}");
    assert_eq!(
        content_digest_of_every_table(&owner).await,
        before,
        "a late real failure must undo all document/audit/link/status effects"
    );
    sqlx::raw_sql("DROP TRIGGER test_fail_second_payslip ON inbox_docs; DROP FUNCTION test_fail_second_payslip();").execute(&owner).await.unwrap();
    let (status, body) = post(&p, "issue-payslips", None).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "retry after authoritative rollback: {body}"
    );
    assert_eq!(body["issued"], 2);
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn final_recorder_refuses_incomplete_duplicate_foreign_or_mismatched_evidence(owner: PgPool) {
    let p = population(&owner, 2).await;
    paid_fixture(&owner, &p).await;
    let mut deliveries = Vec::new();
    for (index, (line, employee, _, payload)) in p.members.iter().enumerate() {
        deliveries.push((
            *line,
            *employee,
            emit_matching(&p, index, payload.clone()).await,
        ));
    }
    let other_run = seed_run(&owner, p.org, p.actor).await;
    let other_employee = seed_employee(&owner, p.org, "Other").await;
    let source = seed_verified_import_row(&owner, p.org).await;
    seed_calculable_line(&owner, p.org, other_run, other_employee, source).await;
    let other_line: Uuid =
        sqlx::query_scalar("SELECT id FROM payroll_draft_lines WHERE run_id = $1")
            .bind(other_run)
            .fetch_one(&owner)
            .await
            .unwrap();
    let cases = [
        vec![],
        vec![deliveries[0]],
        vec![deliveries[0], deliveries[0]],
        vec![
            (other_line, deliveries[0].1, deliveries[0].2),
            deliveries[1],
        ],
        vec![
            (deliveries[0].0, deliveries[1].1, deliveries[0].2),
            deliveries[1],
        ],
        vec![
            (deliveries[0].0, deliveries[0].1, deliveries[1].2),
            deliveries[1],
        ],
        vec![deliveries[0], deliveries[1], deliveries[0]],
    ];
    for supplied in cases {
        let before = content_digest_of_every_table(&owner).await;
        let mut conn = tx(&p).await;
        let result = lifecycle::record_payslip_deliveries_in_tx(&mut conn, p.run, &supplied).await;
        assert!(
            matches!(result, Err(LifecycleError::InvalidState(_))),
            "{supplied:?}: {result:?}"
        );
        conn.rollback().await.unwrap();
        assert_eq!(content_digest_of_every_table(&owner).await, before);
    }
    // Existing links must be validated; ON CONFLICT DO NOTHING cannot hide corruption.
    sqlx::query("INSERT INTO payroll_payslip_deliveries (org_id,run_id,line_id,employee_id,inbox_doc_id) VALUES ($1,$2,$3,$4,$5)")
        .bind(p.org.as_uuid()).bind(p.run).bind(deliveries[0].0).bind(deliveries[1].1).bind(deliveries[1].2).execute(&owner).await.unwrap();
    let before = content_digest_of_every_table(&owner).await;
    let mut conn = tx(&p).await;
    let result = lifecycle::record_payslip_deliveries_in_tx(&mut conn, p.run, &deliveries).await;
    assert!(
        matches!(result, Err(LifecycleError::InvalidState(_))),
        "{result:?}"
    );
    conn.rollback().await.unwrap();
    assert_eq!(content_digest_of_every_table(&owner).await, before);
}

const LOCK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
async fn observed_blocker(owner: &PgPool, blocker: i32, app_name: &str) -> bool {
    tokio::time::timeout(LOCK_TIMEOUT, async {
        loop {
            let blocked: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM pg_stat_activity WHERE datname = current_database() AND application_name = $1 AND wait_event_type = 'Lock' AND $2 = ANY(pg_blocking_pids(pid)))")
                .bind(app_name).bind(blocker).fetch_one(owner).await.unwrap();
            if blocked {break;}
            tokio::task::yield_now().await;
        }
    }).await.is_ok()
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn relinking_before_publication_is_observed_and_refused_without_disclosure(owner: PgPool) {
    for replace in [false, true] {
        let p = population(&owner, 2).await;
        let replacement = seed_user(&owner, p.org, "MEMBER", None).await;
        paid_fixture(&owner, &p).await;
        let mut relink = owner.begin().await.unwrap();
        let blocker: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *relink)
            .await
            .unwrap();
        sqlx::query("SELECT id FROM users WHERE id = $1 FOR UPDATE")
            .bind(p.members[1].2.as_uuid())
            .fetch_one(&mut *relink)
            .await
            .unwrap();
        sqlx::query("UPDATE users SET employee_id = NULL WHERE id = $1")
            .bind(p.members[1].2.as_uuid())
            .execute(&mut *relink)
            .await
            .unwrap();
        if replace {
            sqlx::query("UPDATE users SET employee_id=$2 WHERE id=$1")
                .bind(replacement.as_uuid())
                .bind(p.members[1].1)
                .execute(&mut *relink)
                .await
                .unwrap();
        }
        let named = PgPoolOptions::new()
            .max_connections(2)
            .after_connect(|c, _| {
                Box::pin(async move {
                    sqlx::query("SET ROLE console_rt").execute(c).await?;
                    Ok(())
                })
            })
            .connect_with(
                owner
                    .connect_options()
                    .as_ref()
                    .clone()
                    .application_name("population-issue"),
            )
            .await
            .unwrap();
        let keys = Keys {
            private_pem: p.keys.private_pem.clone(),
            public_pem: p.keys.public_pem.clone(),
        };
        let token = p.token.clone();
        let run = p.run;
        let pending = tokio::spawn(async move {
            send(
                &named,
                &keys,
                "POST",
                &format!("/api/v1/payroll/runs/{run}/issue-payslips"),
                &token,
                None,
            )
            .await
        });
        let observed = observed_blocker(&owner, blocker, "population-issue").await;
        relink.commit().await.unwrap();
        let (status, body) = tokio::time::timeout(LOCK_TIMEOUT, pending)
            .await
            .unwrap()
            .unwrap();
        assert!(
            observed,
            "publication must lock the recipient selected before the relink commit"
        );
        assert_eq!(status, StatusCode::CONFLICT, "{body}");
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM inbox_docs WHERE source_id = $1")
            .bind(p.run.to_string())
            .fetch_one(&owner)
            .await
            .unwrap();
        assert_eq!(count, 0);
        let state: String = sqlx::query_scalar("SELECT status FROM payroll_draft_runs WHERE id=$1")
            .bind(p.run)
            .fetch_one(&owner)
            .await
            .unwrap();
        assert_eq!(state, "PAID");
        let effects: (i64,i64) = sqlx::query_as("SELECT (SELECT COUNT(*) FROM payroll_payslip_deliveries WHERE run_id=$1),(SELECT COUNT(*) FROM audit_events WHERE action IN ('inbox_doc.emit','payroll_run.payslip_issue'))").bind(p.run).fetch_one(&owner).await.unwrap();
        assert_eq!(
            effects,
            (0, 0),
            "replacement={replace}: no publication evidence may escape"
        );
    }
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn publication_holds_recipient_linkage_until_its_commit(owner: PgPool) {
    let p = population(&owner, 2).await;
    paid_fixture(&owner, &p).await;
    let mut issuance = tx(&p).await;
    let blocker: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *issuance)
        .await
        .unwrap();
    let (_, lines) = lifecycle::load_payslip_issuance_in_tx(&mut issuance, p.run)
        .await
        .unwrap();
    assert_eq!(lines.len(), 2);
    let recipient = *p.members[1].2.as_uuid();
    let mut relink = owner.begin().await.unwrap();
    sqlx::query("SET LOCAL application_name='population-relink'")
        .execute(&mut *relink)
        .await
        .unwrap();
    let pending = tokio::spawn(async move {
        sqlx::query("SELECT id FROM users WHERE id = $1 FOR UPDATE")
            .bind(recipient)
            .fetch_one(&mut *relink)
            .await
            .unwrap();
        sqlx::query("UPDATE users SET employee_id=NULL WHERE id=$1")
            .bind(recipient)
            .execute(&mut *relink)
            .await
            .unwrap();
        relink.commit().await.unwrap();
    });
    let observed = observed_blocker(&owner, blocker, "population-relink").await;
    issuance.commit().await.unwrap();
    tokio::time::timeout(LOCK_TIMEOUT, pending)
        .await
        .unwrap()
        .unwrap();
    assert!(
        observed,
        "the publication transaction must retain selected recipient locks until commit"
    );
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn final_recorder_revalidates_document_content_metadata_and_current_recipient(owner: PgPool) {
    let p = population(&owner, 1).await;
    paid_fixture(&owner, &p).await;
    let doc = emit_matching(&p, 0, p.members[0].3.clone()).await;
    let supplied = [(p.members[0].0, p.members[0].1, doc)];
    for defect in ["title", "source", "payload", "recipient"] {
        let mut corruption = owner.begin().await.unwrap();
        match defect {
            "title" => {
                sqlx::query("UPDATE inbox_docs SET title='different artifact' WHERE id=$1")
                    .bind(doc)
                    .execute(&mut *corruption)
                    .await
                    .unwrap();
            }
            "source" => {
                sqlx::query("UPDATE inbox_docs SET source_id='another run' WHERE id=$1")
                    .bind(doc)
                    .execute(&mut *corruption)
                    .await
                    .unwrap();
            }
            "payload" => {
                sqlx::query("UPDATE inbox_docs SET payload=jsonb_set(payload,'{calculation_version}','99') WHERE id=$1").bind(doc).execute(&mut *corruption).await.unwrap();
            }
            _ => {
                sqlx::query("UPDATE inbox_docs SET recipient_user_id=$2 WHERE id=$1")
                    .bind(doc)
                    .bind(p.actor.as_uuid())
                    .execute(&mut *corruption)
                    .await
                    .unwrap();
            }
        }
        corruption.commit().await.unwrap();
        let before = content_digest_of_every_table(&owner).await;
        let mut conn = tx(&p).await;
        let result = lifecycle::record_payslip_deliveries_in_tx(&mut conn, p.run, &supplied).await;
        assert!(
            matches!(result, Err(LifecycleError::InvalidState(_))),
            "{defect}: {result:?}"
        );
        conn.rollback().await.unwrap();
        assert_eq!(content_digest_of_every_table(&owner).await, before);
        sqlx::query("UPDATE inbox_docs SET title='급여명세서 2026-06-01 ~ 2026-06-30 · Alice',source_id=$2,payload=$3,recipient_user_id=$4 WHERE id=$1")
            .bind(doc).bind(p.run.to_string()).bind(&p.members[0].3).bind(p.members[0].2.as_uuid()).execute(&owner).await.unwrap();
    }
    let mut conn = tx(&p).await;
    lifecycle::record_payslip_deliveries_in_tx(&mut conn, p.run, &supplied)
        .await
        .unwrap();
    conn.commit().await.unwrap();
    let state: String = sqlx::query_scalar("SELECT status FROM payroll_draft_runs WHERE id=$1")
        .bind(p.run)
        .fetch_one(&owner)
        .await
        .unwrap();
    assert_eq!(state, "ISSUED");
}
