#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
//! Authenticated, runtime-role HTTP contract for the payroll run lifecycle
//! (CAP-PAYROLL-CONSOLE). Crosses the crate's assembled router (the same
//! `with_request_context` middleware the app mounts) on a genuine `console_rt`
//! served pool (RLS enforced), with a real ES256 signature chain, through the
//! full close → calculate → exception → SoD decision → disbursement →
//! release-gated payslip issuance pipeline; asserts PBAC denial without
//! leakage, cross-tenant invisibility, and the audit readback.
//!
//! NOTE: chartered home was `backend/app/tests/payroll_run_api.rs`, but
//! `console-app` does not compile on this branch (facilities/production rest
//! lanes are mid-refactor), so the identical proof runs here against the
//! crate router the app mounts verbatim.
//!
//! Draft-run fixtures mint via `payroll.create_run` as `console_rt` (no owner
//! INSERT of draft runs). Lines still INSERT (calculate HOLD).

use axum::body::{Body, to_bytes};
use console_kernel_core::{OrgId, UserId};
use console_ontology_canonical_adapter_postgres::company::{
    CompanyCommand, CompanyQuery, PgCompanyPort,
};
use console_ontology_canonical_adapter_postgres::employment::{
    EmploymentAttributes, EmploymentCommand, EmploymentQuery, NewEmployeeRecord, PgEmploymentPort,
    insert_employee_record,
};
use console_ontology_canonical_adapter_postgres::job_position::{
    JobPositionCommand, JobPositionQuery, PgJobPositionPort,
};
use console_ontology_canonical_adapter_postgres::org_unit::{
    OrgUnitCommand, OrgUnitQuery, PgOrgUnitPort,
};
use console_ontology_canonical_adapter_postgres::person::{
    PersonCommand, PersonQuery, PgPersonPort,
};
use console_ontology_canonical_domain::{CanonicalPort, CommandId, CommandReceipt, DispatchTarget};
use console_payroll_adapter_postgres::PgPayrollStore;
use console_payroll_adapter_postgres::pay_run::{PayRunCommand, PayRunQuery, PgPayRunPort};
use console_payroll_rest::{PayrollRestState, router};
use console_platform_auth::{AccessTokenInput, JwtIssuer, JwtSettings, JwtVerifier};
use console_platform_test_support::seed_org_and_super_admin;
use http::{Request, StatusCode, header};
use p256::ecdsa::SigningKey;
use p256::elliptic_curve::rand_core::OsRng;
use p256::pkcs8::{EncodePrivateKey, EncodePublicKey, LineEnding};
use serde_json::{Value, json};
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use std::collections::BTreeMap;
use time::format_description::well_known::Rfc3339;
use time::macros::date;
use time::{Duration, OffsetDateTime, Time};
use tower::ServiceExt;
use uuid::Uuid;

const ISSUER: &str = "console-platform-auth";
const AUDIENCE: &str = "console-api";
const SHA256_FIXTURE: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn executive_drives_full_lifecycle_with_audit_readback(pool: PgPool) {
    let keys = Keys::generate();
    let rt = runtime_role_pool(&pool).await;
    let org = OrgId::knl();
    seed_org(&pool, org).await;

    let submitter = seed_user(&pool, org, "EXECUTIVE", None).await;
    let decider = seed_user(&pool, org, "EXECUTIVE", None).await;
    let submitter_token = keys.token(submitter, org, "EXECUTIVE");
    let decider_token = keys.token(decider, org, "EXECUTIVE");

    let employee = seed_employee(&pool, org, "Alice").await;
    let recipient = seed_user(&pool, org, "MEMBER", Some(employee)).await;
    let run = seed_run(&pool, org, submitter).await;
    let import_row = seed_verified_import_row(&pool, org).await;
    seed_calculable_line(&pool, org, run, employee, import_row).await;

    // ---- Close preflight: fail-closed while the payroll period lock is
    // missing; the check names itself and close 409s with the receipt.
    let (status, preflight) = send(
        &rt,
        &keys,
        "GET",
        &format!("/api/v1/payroll/runs/{run}/close-preflight"),
        &submitter_token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{preflight}");
    assert_eq!(preflight["can_close"], false);
    let lock_check = preflight["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|check| check["key"] == "period_lock")
        .expect("period_lock check present");
    assert_eq!(lock_check["ok"], false);

    let (status, blocked) = send(
        &rt,
        &keys,
        "POST",
        &format!("/api/v1/payroll/runs/{run}/close-attendance"),
        &submitter_token,
        Some(json!({"attest": true})),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{blocked}");
    assert_eq!(blocked["error"]["code"], "preflight_blocked");
    assert_eq!(blocked["error"]["details"]["can_close"], false);

    seed_period_lock(&pool, org).await;

    // Attestation is mandatory.
    let (status, unattested) = send(
        &rt,
        &keys,
        "POST",
        &format!("/api/v1/payroll/runs/{run}/close-attendance"),
        &submitter_token,
        Some(json!({"attest": false})),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{unattested}");

    let (status, closed) = send(
        &rt,
        &keys,
        "POST",
        &format!("/api/v1/payroll/runs/{run}/close-attendance"),
        &submitter_token,
        Some(json!({"attest": true})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{closed}");
    assert_eq!(closed["run"]["status"], "ATTENDANCE_CLOSED");
    assert_eq!(
        closed["run"]["close_receipt"]["attested_by"],
        json!(submitter.as_uuid()),
        "close receipt records the attestor"
    );

    // ---- Calculate: real kernel math from the verified source ledger row.
    let (status, calculated) = send(
        &rt,
        &keys,
        "POST",
        &format!("/api/v1/payroll/runs/{run}/calculate"),
        &submitter_token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{calculated}");
    assert_eq!(calculated["run"]["status"], "CALCULATED");
    let calc = &calculated["calculation"];
    assert_eq!(calc["calculated_lines"], 1);
    assert_eq!(calc["blocked_lines"], 0);
    assert_eq!(
        calc["total_net_won"], 2_626_700,
        "3,000,000 gross with the June 2026 statutory tables + verified NTS row"
    );
    assert_eq!(calc["payable"], false, "draft until the release gate flips");
    assert_eq!(
        calculated["exceptions_open"], 1,
        "overtime hours raise 예외"
    );

    // ---- Submit is fail-closed while exceptions are open.
    let (status, refused) = send(
        &rt,
        &keys,
        "POST",
        &format!("/api/v1/payroll/runs/{run}/submit"),
        &submitter_token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{refused}");
    assert_eq!(refused["error"]["code"], "exceptions_open");

    let (status, exceptions) = send(
        &rt,
        &keys,
        "GET",
        &format!("/api/v1/payroll/runs/{run}/exceptions"),
        &submitter_token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{exceptions}");
    assert_eq!(exceptions["open"], 1);
    let exception = &exceptions["items"][0];
    assert_eq!(exception["kind"], "OVERTIME_ALLOWANCE");
    assert_eq!(exception["status"], "OPEN");
    assert!(
        exception["amount_delta_won"].is_null(),
        "no verified source for the delta — must be null, never an estimate"
    );
    let exception_id = exception["id"].as_str().unwrap().to_owned();

    // HOLD requires a reason; CONFIRM resolves; a second resolve conflicts.
    let (status, no_reason) = send(
        &rt,
        &keys,
        "POST",
        &format!("/api/v1/payroll/runs/{run}/exceptions/{exception_id}/resolve"),
        &submitter_token,
        Some(json!({"action": "HOLD"})),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{no_reason}");
    let (status, confirmed) = send(
        &rt,
        &keys,
        "POST",
        &format!("/api/v1/payroll/runs/{run}/exceptions/{exception_id}/resolve"),
        &submitter_token,
        Some(json!({"action": "CONFIRM"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{confirmed}");
    assert_eq!(confirmed["status"], "CONFIRMED");
    let (status, replay) = send(
        &rt,
        &keys,
        "POST",
        &format!("/api/v1/payroll/runs/{run}/exceptions/{exception_id}/resolve"),
        &submitter_token,
        Some(json!({"action": "CONFIRM"})),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{replay}");
    assert_eq!(replay["error"]["code"], "already_resolved");

    // ---- Submit, then SoD: the submitter may not decide.
    let (status, submitted) = send(
        &rt,
        &keys,
        "POST",
        &format!("/api/v1/payroll/runs/{run}/submit"),
        &submitter_token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{submitted}");
    assert_eq!(submitted["run"]["status"], "SUBMITTED");

    let (status, sod) = send(
        &rt,
        &keys,
        "POST",
        &format!("/api/v1/payroll/runs/{run}/decision"),
        &submitter_token,
        Some(json!({"decision": "APPROVE"})),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{sod}");
    assert_eq!(sod["error"]["code"], "sod_violation");

    // ---- Reject (reason required) → withdraw → resubmit → approve.
    let (status, no_reason) = send(
        &rt,
        &keys,
        "POST",
        &format!("/api/v1/payroll/runs/{run}/decision"),
        &decider_token,
        Some(json!({"decision": "REJECT"})),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{no_reason}");
    let (status, rejected) = send(
        &rt,
        &keys,
        "POST",
        &format!("/api/v1/payroll/runs/{run}/decision"),
        &decider_token,
        Some(json!({"decision": "REJECT", "reason": "지급 계좌 재확인"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{rejected}");
    assert_eq!(rejected["run"]["status"], "REJECTED");

    let (status, withdrawn) = send(
        &rt,
        &keys,
        "POST",
        &format!("/api/v1/payroll/runs/{run}/withdraw"),
        &submitter_token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{withdrawn}");
    assert_eq!(withdrawn["run"]["status"], "CALCULATED");

    let (status, _) = send(
        &rt,
        &keys,
        "POST",
        &format!("/api/v1/payroll/runs/{run}/submit"),
        &submitter_token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, approved) = send(
        &rt,
        &keys,
        "POST",
        &format!("/api/v1/payroll/runs/{run}/decision"),
        &decider_token,
        Some(json!({"decision": "APPROVE"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{approved}");
    assert_eq!(approved["run"]["status"], "APPROVED");

    // ---- Issuing BEFORE the run is PAID must refuse, and must leak no payslip.
    //
    // THIS BLOCK MUST STAY ABOVE THE LEGITIMATE ISSUANCE BELOW. `emit_inbox_doc`
    // dedups on `payroll-run:{run}:line:{line}`, so a leaked document created
    // after the real one collapses into it and the `leaked == 0` assertion below
    // becomes vacuously true. Order is load-bearing here, not stylistic.
    //
    // The release gate is registered FIRST so that the PAID check is the guard
    // actually under test: without it the request is refused by the legal gate
    // and this proves nothing about status. The gate is removed again
    // afterwards, so the legal-gate refusal below still exercises its own path.
    //
    // The status assertion alone would NOT catch a missing PAID check.
    // `record_payslip_deliveries_in_tx` raises the identical `invalid_state`
    // error, so the 409 survives even with the guard deleted. What does not
    // survive is the vault: `issue_payslips` emits documents OUTSIDE the
    // refusing transaction, so a payslip for an unpaid run reaches the
    // employee's inbox while the caller sees a clean refusal. That is what the
    // count asserts.
    register_release_gate(&pool, run).await;
    let (status, premature) = send(
        &rt,
        &keys,
        "POST",
        &format!("/api/v1/payroll/runs/{run}/issue-payslips"),
        &submitter_token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{premature}");
    let leaked: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM inbox_docs WHERE kind = 'payslip' AND source_id = $1",
    )
    .bind(run.to_string())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        leaked, 0,
        "a run at APPROVED must issue no payslip; the caller's 409 does not undo a document \
         already emitted outside the refusing transaction"
    );
    let recipient_inbox: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM inbox_docs WHERE kind = 'payslip' AND recipient_user_id = $1",
    )
    .bind(*recipient.as_uuid())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        recipient_inbox, 0,
        "the employee recipient's inbox must stay empty after a refused issuance"
    );

    // An org-B EXECUTIVE: this run is indistinguishable from a missing one
    // on the issue-payslips write path (RLS deny-by-omission). 404 omit, not 403.
    let org_b = OrgId::from_uuid(Uuid::new_v4());
    sqlx::query("INSERT INTO organizations (id, slug, name) VALUES ($1, 'org-b', 'Org B')")
        .bind(*org_b.as_uuid())
        .execute(&pool)
        .await
        .unwrap();
    let outsider = seed_user(&pool, org_b, "EXECUTIVE", None).await;
    let outsider_token = keys.token(outsider, org_b, "EXECUTIVE");
    let (status, omitted) = send(
        &rt,
        &keys,
        "POST",
        &format!("/api/v1/payroll/runs/{run}/issue-payslips"),
        &outsider_token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{omitted}");
    assert_eq!(omitted["error"]["code"], "not_found");

    sqlx::query(
        "UPDATE payroll_draft_runs SET legal_basis = legal_basis - 'release_gate' WHERE id = $1",
    )
    .bind(run)
    .execute(&pool)
    .await
    .unwrap();

    // ---- Disbursement: past date 422; then the operator-attested FSM.
    let past = (OffsetDateTime::now_utc() - Duration::hours(1))
        .format(&Rfc3339)
        .unwrap();
    let (status, _) = send(
        &rt,
        &keys,
        "POST",
        &format!("/api/v1/payroll/runs/{run}/schedule-disbursement"),
        &submitter_token,
        Some(json!({"scheduled_at": past})),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let future = (OffsetDateTime::now_utc() + Duration::days(1))
        .format(&Rfc3339)
        .unwrap();
    let (status, scheduled) = send(
        &rt,
        &keys,
        "POST",
        &format!("/api/v1/payroll/runs/{run}/schedule-disbursement"),
        &submitter_token,
        Some(json!({"scheduled_at": future})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{scheduled}");
    assert_eq!(scheduled["status"], "SCHEDULED");

    // PAID straight from SCHEDULED is an invalid operator attestation.
    let (status, skipped) = send(
        &rt,
        &keys,
        "POST",
        &format!("/api/v1/payroll/runs/{run}/disbursement/attest"),
        &submitter_token,
        Some(json!({"status": "PAID"})),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{skipped}");
    assert_eq!(skipped["error"]["code"], "invalid_transition");
    let (status, _) = send(
        &rt,
        &keys,
        "POST",
        &format!("/api/v1/payroll/runs/{run}/disbursement/attest"),
        &submitter_token,
        Some(json!({"status": "SUBMITTED_TO_BANK"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, paid) = send(
        &rt,
        &keys,
        "POST",
        &format!("/api/v1/payroll/runs/{run}/disbursement/attest"),
        &submitter_token,
        Some(json!({"status": "PAID"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{paid}");
    assert_eq!(paid["status"], "PAID");

    // ---- Payslip issuance is hard-gated by the registered release gate.
    let (status, gate) = send(
        &rt,
        &keys,
        "POST",
        &format!("/api/v1/payroll/runs/{run}/issue-payslips"),
        &submitter_token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{gate}");
    assert_eq!(gate["error"]["code"], "legal_gate");

    register_release_gate(&pool, run).await;

    let (status, issued) = send(
        &rt,
        &keys,
        "POST",
        &format!("/api/v1/payroll/runs/{run}/issue-payslips"),
        &submitter_token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{issued}");
    assert_eq!(issued["issued"], 1);
    assert_eq!(issued["acknowledged"], 0);

    // Replay after ISSUED is a typed conflict, and the vault holds exactly
    // one deduped payslip document sourced from this run.
    let (status, replay) = send(
        &rt,
        &keys,
        "POST",
        &format!("/api/v1/payroll/runs/{run}/issue-payslips"),
        &submitter_token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{replay}");
    let docs: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM inbox_docs WHERE kind = 'payslip' AND source_id = $1",
    )
    .bind(run.to_string())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(docs, 1);

    let (status, delivery) = send(
        &rt,
        &keys,
        "GET",
        &format!("/api/v1/payroll/runs/{run}/payslip-delivery"),
        &decider_token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{delivery}");
    assert_eq!(delivery["issued"], 1);
    assert_eq!(delivery["items"][0]["acknowledged_at"], Value::Null);

    let (status, final_detail) = send(
        &rt,
        &keys,
        "GET",
        &format!("/api/v1/payroll/runs/{run}"),
        &decider_token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(final_detail["run"]["status"], "ISSUED");

    // ---- Audit readback: every lifecycle mutation left its event.
    for action in [
        "payroll_run.close",
        "payroll_run.calculate",
        "payroll_run.submit",
        "payroll_run.decide",
        "payroll_run.withdraw",
        "payroll_run.disburse_schedule",
        "payroll_run.disburse_attest",
        "payroll_run.payslip_issue",
    ] {
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM audit_events WHERE action = $1 AND (target_id = $2 OR \
             target_type = 'payroll_disbursement')",
        )
        .bind(action)
        .bind(run.to_string())
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(count >= 1, "audit action {action} must be recorded");
    }
    let resolve_audits: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM audit_events WHERE action = 'payroll_run.exception_resolve'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        resolve_audits, 1,
        "exactly the one successful resolve audits"
    );
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn lifecycle_writes_deny_without_leakage_and_cross_tenant_is_invisible(pool: PgPool) {
    let keys = Keys::generate();
    let rt = runtime_role_pool(&pool).await;
    let org_a = OrgId::knl();
    seed_org(&pool, org_a).await;
    let org_b = OrgId::from_uuid(Uuid::new_v4());
    sqlx::query("INSERT INTO organizations (id, slug, name) VALUES ($1, 'org-b', 'Org B')")
        .bind(*org_b.as_uuid())
        .execute(&pool)
        .await
        .unwrap();

    let stager = seed_user(&pool, org_a, "EXECUTIVE", None).await;
    let run = seed_run(&pool, org_a, stager).await;

    // MEMBER and built-in ADMIN (the write tier is EXECUTIVE/SUPER_ADMIN or a
    // custom org-wide PBAC grant): 403, and the same 403 whether or not the
    // run exists — no existence oracle.
    for role in ["MEMBER", "ADMIN"] {
        let user = seed_user(&pool, org_a, role, None).await;
        let token = keys.token(user, org_a, role);
        for target in [run, Uuid::new_v4()] {
            let (status, denied) = send(
                &rt,
                &keys,
                "POST",
                &format!("/api/v1/payroll/runs/{target}/calculate"),
                &token,
                None,
            )
            .await;
            assert_eq!(status, StatusCode::FORBIDDEN, "{role}: {denied}");
            assert_eq!(denied["error"]["code"], "forbidden");
        }
    }

    // An org-B EXECUTIVE: org A's run is indistinguishable from a missing one
    // on both the read and the write path (RLS deny-by-omission).
    let outsider = seed_user(&pool, org_b, "EXECUTIVE", None).await;
    let outsider_token = keys.token(outsider, org_b, "EXECUTIVE");
    for (method, uri) in [
        ("GET", format!("/api/v1/payroll/runs/{run}")),
        ("GET", format!("/api/v1/payroll/runs/{run}/close-preflight")),
        ("GET", format!("/api/v1/payroll/runs/{run}/exceptions")),
        ("POST", format!("/api/v1/payroll/runs/{run}/calculate")),
        ("POST", format!("/api/v1/payroll/runs/{run}/submit")),
    ] {
        let (status, body) = send(&rt, &keys, method, &uri, &outsider_token, None).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{method} {uri}: {body}");
        assert_eq!(body["error"]["code"], "not_found");
    }

    // The denied probes must not have advanced the run.
    let status: String = sqlx::query_scalar("SELECT status FROM payroll_draft_runs WHERE id = $1")
        .bind(run)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        status, "BLOCKED_LEGAL_GATE",
        "denied probes must not advance the create_run status"
    );
    let probe_audits: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM audit_events WHERE action LIKE 'payroll_run.%' AND target_id = $1",
    )
    .bind(run.to_string())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        probe_audits, 0,
        "denied probes must not commit audit events"
    );
}

// ---------------------------------------------------------------------------
// Close preflight is a TRUE read; close recomputes it (TOCTOU)
// ---------------------------------------------------------------------------

/// The SHIPPED preflight — `GET /api/v1/payroll/runs/{id}/close-preflight`,
/// driven through the assembled router — must persist NO VERDICT: the only row
/// it may leave behind anywhere in the database is its own `preflight_read`
/// audit row. Asserted as a whole-database content delta on all three branches
/// (`can_close = true`, blocked, and the 404 for a run that does not exist), so
/// a verdict cached on any one of them is caught — the `can_close = true`
/// branch is merely the one a "helpfully" caching implementation would want to
/// write first. `audit_events` is checked row by row rather than as one digest,
/// because it is itself a readable table: a verdict parked in a snapshot column
/// there would be the same TOCTOU cache as one parked on the run.
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn close_preflight_persists_no_verdict_only_its_read_audit(pool: PgPool) {
    let keys = Keys::generate();
    let rt = runtime_role_pool(&pool).await;
    let org = OrgId::knl();
    seed_org(&pool, org).await;
    let executive = seed_user(&pool, org, "EXECUTIVE", None).await;
    let token = keys.token(executive, org, "EXECUTIVE");
    let employee = seed_employee(&pool, org, "Alice").await;
    let run = seed_run(&pool, org, executive).await;
    let import_row = seed_verified_import_row(&pool, org).await;
    seed_calculable_line(&pool, org, run, employee, import_row).await;
    seed_period_lock(&pool, org).await;

    let before = snapshot_db(&pool).await;
    // The snapshot is not vacuous: the tables a preflight would be tempted to
    // write are watched, and the ones it reads already hold rows.
    for table in [
        "public.payroll_draft_runs",
        "public.payroll_draft_lines",
        "public.audit_events",
        "public.workflow_outbox_events",
        "public.period_locks",
    ] {
        assert!(
            before.digests.contains_key(table),
            "{table} must be watched by the snapshot"
        );
    }
    for table in [
        "public.payroll_draft_runs",
        "public.payroll_draft_lines",
        "public.period_locks",
    ] {
        assert!(
            !before.digests[table].starts_with("0:"),
            "{table} must already hold rows for the digest to be able to change, got {:?}",
            before.digests[table]
        );
    }

    // BRANCH 1 — `can_close = true`, twice.
    for pass in 1..=2 {
        let (status, preflight) = send(
            &rt,
            &keys,
            "GET",
            &format!("/api/v1/payroll/runs/{run}/close-preflight"),
            &token,
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "pass {pass}: {preflight}");
        assert_eq!(
            preflight["can_close"], true,
            "pass {pass} must reach the can_close branch: {preflight}"
        );
    }
    let after = snapshot_db(&pool).await;
    assert_only_the_read_audit_changed(&before, &after, "can_close = true", 2);

    // BRANCH 2 — blocked. Releasing the period lock flips the verdict; a
    // refusal is exactly the thing an implementation is tempted to record.
    sqlx::query(
        "UPDATE period_locks SET unlocked_at = now(), unlock_reason = '동결창 해제' \
         WHERE domain = 'payroll'",
    )
    .execute(&pool)
    .await
    .unwrap();
    let before = snapshot_db(&pool).await;
    let (status, blocked) = send(
        &rt,
        &keys,
        "GET",
        &format!("/api/v1/payroll/runs/{run}/close-preflight"),
        &token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{blocked}");
    assert_eq!(
        blocked["can_close"], false,
        "must reach the blocked branch: {blocked}"
    );
    let after = snapshot_db(&pool).await;
    assert_only_the_read_audit_changed(&before, &after, "blocked", 1);

    // BRANCH 3 — the run does not exist: 404, and not even an audit row, since
    // there is no run whose access could be recorded.
    let before = snapshot_db(&pool).await;
    let missing = Uuid::new_v4();
    let (status, body) = send(
        &rt,
        &keys,
        "GET",
        &format!("/api/v1/payroll/runs/{missing}/close-preflight"),
        &token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    let after = snapshot_db(&pool).await;
    let changed = changed_tables(&before.digests, &after.digests);
    assert!(
        changed.is_empty(),
        "a 404 preflight must persist nothing at all; changed: {changed:?}"
    );

    // The three preflights that found a run left exactly three audit rows and
    // nothing else — in particular no verdict on the run itself.
    let reads: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM audit_events \
         WHERE action = 'payroll_run.preflight_read' AND target_id = $1",
    )
    .bind(run.to_string())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(reads, 3, "every preflight that read a run must be audited");
    let receipt: Option<serde_json::Value> =
        sqlx::query_scalar("SELECT close_receipt FROM payroll_draft_runs WHERE id = $1")
            .bind(run)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(
        receipt.is_none(),
        "preflight must not stash a verdict on the run: {receipt:?}"
    );
}

/// The preflight's ONLY permitted persistent effect is `expected_reads` bare
/// read-audit rows. Any other changed table — above all a verdict cached on
/// `payroll_draft_runs` — fails; so does an extra row inside `audit_events`, or
/// a verdict carried in the read row's own `before_snap`/`after_snap`. The
/// table digest cannot tell those last two from a legitimate read audit, which
/// is why the rows are compared individually.
fn assert_only_the_read_audit_changed(
    before: &DbSnapshot,
    after: &DbSnapshot,
    branch: &str,
    expected_reads: usize,
) {
    let changed = changed_tables(&before.digests, &after.digests);
    let tables: Vec<&str> = changed.iter().map(|(table, ..)| table.as_str()).collect();
    assert_eq!(
        tables,
        vec!["public.audit_events"],
        "[{branch}] the close preflight may persist its read audit and NOTHING \
         else; changed (table, before digest, after digest): {changed:?}"
    );
    // `audit_events` is append-only (migration 0003), so its delta is exactly
    // the rows whose id is new.
    let appended: Vec<(&Uuid, &AuditRow)> = after
        .audits
        .iter()
        .filter(|(id, _)| !before.audits.contains_key(*id))
        .collect();
    assert_eq!(
        appended.len(),
        expected_reads,
        "[{branch}] the close preflight may append exactly {expected_reads} audit \
         row(s); appended (id, action, before_snap, after_snap): {appended:?}"
    );
    for (id, (action, before_snap, after_snap)) in appended {
        assert_eq!(
            action, "payroll_run.preflight_read",
            "[{branch}] audit row {id} is not the read marker"
        );
        assert_eq!(
            (before_snap, after_snap),
            (&None, &None),
            "[{branch}] audit row {id} must record only THAT the run was read; a \
             verdict in its snapshot columns is a cached verdict like any other"
        );
    }
}

/// Close must RECOMPUTE the preflight in its own transaction, never trust a
/// verdict computed earlier. The verdict the caller was shown said `can_close`;
/// the state it checked then changed; close must refuse.
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn close_recomputes_the_preflight_after_the_read_verdict_goes_stale(pool: PgPool) {
    let keys = Keys::generate();
    let rt = runtime_role_pool(&pool).await;
    let org = OrgId::knl();
    seed_org(&pool, org).await;
    let executive = seed_user(&pool, org, "EXECUTIVE", None).await;
    let token = keys.token(executive, org, "EXECUTIVE");
    let employee = seed_employee(&pool, org, "Alice").await;
    let run = seed_run(&pool, org, executive).await;
    let import_row = seed_verified_import_row(&pool, org).await;
    seed_calculable_line(&pool, org, run, employee, import_row).await;
    seed_period_lock(&pool, org).await;

    let (status, preflight) = send(
        &rt,
        &keys,
        "GET",
        &format!("/api/v1/payroll/runs/{run}/close-preflight"),
        &token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{preflight}");
    assert_eq!(preflight["can_close"], true, "{preflight}");

    // TIME OF USE: the period lock the verdict above was built on is released.
    sqlx::query(
        "UPDATE period_locks SET unlocked_at = now(), unlock_reason = '동결창 해제' \
         WHERE domain = 'payroll'",
    )
    .execute(&pool)
    .await
    .unwrap();

    let (status, blocked) = send(
        &rt,
        &keys,
        "POST",
        &format!("/api/v1/payroll/runs/{run}/close-attendance"),
        &token,
        Some(json!({"attest": true})),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "close must recompute the preflight against current state, not trust the earlier \
         verdict: {blocked}"
    );
    assert_eq!(blocked["error"]["code"], "preflight_blocked");
    assert_eq!(blocked["error"]["details"]["can_close"], false);

    let status: String = sqlx::query_scalar("SELECT status FROM payroll_draft_runs WHERE id = $1")
        .bind(run)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        status, "BLOCKED_LEGAL_GATE",
        "the refused close must not have advanced the create_run status"
    );
}

/// HTTP close-preflight against a draft minted by `payroll.create_run` after
/// Company → OrgUnit → JobPosition → Person → hr.appoint. No calculate, no won.
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn empty_tenant_run_close_preflight_sits_on_canonical_org_tree(owner_pool: PgPool) {
    let tree = provision_empty_tenant_appointed_run(&owner_pool).await;
    let keys = Keys::generate();
    let rt = runtime_role_pool(&owner_pool).await;
    let executive = seed_user(&owner_pool, tree.org, "EXECUTIVE", None).await;
    let token = keys.token(executive, tree.org, "EXECUTIVE");

    let (status, preflight) = send(
        &rt,
        &keys,
        "GET",
        &format!("/api/v1/payroll/runs/{}/close-preflight", tree.draft_run_id),
        &token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{preflight}");
    assert_eq!(preflight["can_close"], false, "{preflight}");
    let lock_check = preflight["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|check| check["key"] == "period_lock")
        .expect("period_lock check present");
    assert_eq!(lock_check["ok"], false);

    let enabled: bool = sqlx::query_scalar(
        "SELECT calculation_enabled FROM payroll_draft_runs WHERE org_id = $1 AND id = $2",
    )
    .bind(*tree.org.as_uuid())
    .bind(tree.draft_run_id)
    .fetch_one(&owner_pool)
    .await
    .unwrap();
    assert!(
        !enabled,
        "create_run on the canonical tree must not enable calculation"
    );
    assert_eq!(tree.appointed.target(), DispatchTarget::HrAppoint);
    let person_id: Uuid = sqlx::query_scalar(
        "SELECT person_id FROM employee_person_bindings \
         WHERE org_id = $1 AND employee_id = $2",
    )
    .bind(*tree.org.as_uuid())
    .bind(tree.employee_id)
    .fetch_one(&owner_pool)
    .await
    .unwrap();
    assert_eq!(
        person_id, tree.employee_id,
        "a uniquely-resolved person is bound with person_id = employee_id"
    );
    let instances: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM ont_instances WHERE org_id = $1")
        .bind(*tree.org.as_uuid())
        .fetch_one(&owner_pool)
        .await
        .unwrap();
    assert_eq!(
        instances, 0,
        "canonical org/hr/payroll writes must not write ont_instances"
    );
}

/// `(action, before_snap, after_snap)` — the columns a verdict cached in the
/// audit log would travel in.
type AuditRow = (String, Option<Value>, Option<Value>);

/// A whole-database snapshot: a content digest per table, plus every
/// `audit_events` row. The digests alone are table-granular, which is too
/// coarse for `audit_events`: an extra audit row, or a verdict stuffed into the
/// read row's `after_snap`, changes that one table's digest exactly as a
/// legitimate read audit does.
struct DbSnapshot {
    digests: BTreeMap<String, String>,
    audits: BTreeMap<Uuid, AuditRow>,
}

async fn snapshot_db(owner_pool: &PgPool) -> DbSnapshot {
    let audits: Vec<(Uuid, String, Option<Value>, Option<Value>)> =
        sqlx::query_as("SELECT id, action, before_snap, after_snap FROM audit_events")
            .fetch_all(owner_pool)
            .await
            .unwrap();
    DbSnapshot {
        digests: content_digest_of_every_table(owner_pool).await,
        audits: audits
            .into_iter()
            .map(|(id, action, before_snap, after_snap)| (id, (action, before_snap, after_snap)))
            .collect(),
    }
}

/// Content digest of every catalog-listed table, so an UPDATE is caught as well
/// as an INSERT (a row count is blind to an UPDATE, and `console_rt` holds
/// UPDATE on `payroll_draft_runs` — "preflight caches its verdict in
/// `close_receipt`" is a mutation the runtime role is actually permitted to
/// make). Tables come from `pg_class`, NOT `information_schema.tables`: the
/// latter lists only tables the current role holds some privilege on, so a
/// table created under another owner and granted only to `console_rt` (the
/// `console_platform_force_cmd` pattern, migration 0196) would be invisible
/// here and a write into it would pass unnoticed. `pg_class` is unfiltered,
/// so a table added later is watched automatically.
async fn content_digest_of_every_table(owner_pool: &PgPool) -> BTreeMap<String, String> {
    sqlx::query_as::<_, (String, String)>(
        r#"
        SELECT format('%I.%I', n.nspname, c.relname)::text AS qualified_name,
               (xpath(
                   '/row/d/text()',
                   query_to_xml(
                       format(
                           'SELECT count(*)::text || '':'' || coalesce(md5(string_agg(r::text, '''' ORDER BY r::text)), ''empty'') AS d FROM %I.%I AS r',
                           n.nspname, c.relname
                       ),
                       false, true, ''
                   )
               ))[1]::text AS digest
        FROM pg_class c
        JOIN pg_namespace n ON n.oid = c.relnamespace
        WHERE c.relkind IN ('r', 'p')
          AND n.nspname NOT LIKE 'pg\_%'
          AND n.nspname <> 'information_schema'
        "#,
    )
    .fetch_all(owner_pool)
    .await
    .unwrap()
    .into_iter()
    .collect()
}

fn changed_tables(
    before: &BTreeMap<String, String>,
    after: &BTreeMap<String, String>,
) -> Vec<(String, String, String)> {
    let mut changed: Vec<(String, String, String)> = before
        .iter()
        .filter_map(|(table, was)| {
            let now = after
                .get(table)
                .cloned()
                .unwrap_or_else(|| "absent".to_owned());
            (&now != was).then(|| (table.clone(), was.clone(), now))
        })
        .collect();
    for (table, now) in after {
        if !before.contains_key(table) {
            changed.push((table.clone(), "absent".to_owned(), now.clone()));
        }
    }
    changed
}

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

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

    fn token(&self, user: UserId, org: OrgId, role: &str) -> String {
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
            roles: vec![role.to_owned()],
            branches: Vec::new(),
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

fn app(pool: PgPool, public_key: &str) -> axum::Router {
    let verifier = JwtVerifier::from_es256_public_pem(
        JwtSettings {
            issuer: ISSUER.into(),
            audience: AUDIENCE.into(),
            access_token_ttl: Duration::minutes(15),
        },
        public_key.as_bytes(),
    )
    .unwrap();
    router(PayrollRestState::new(
        PgPayrollStore::new(pool),
        Some(verifier),
    ))
}

async fn send(
    pool: &PgPool,
    keys: &Keys,
    method: &str,
    uri: &str,
    token: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let request = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(
            body.map(|v| Body::from(serde_json::to_vec(&v).unwrap()))
                .unwrap_or_else(Body::empty),
        )
        .unwrap();
    let response = app(pool.clone(), &keys.public_pem)
        .oneshot(request)
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let json = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, json)
}

async fn seed_org(pool: &PgPool, org: OrgId) {
    sqlx::query(
        "INSERT INTO organizations (id, slug, name) VALUES ($1, 'knl', 'KNL') \
         ON CONFLICT (id) DO NOTHING",
    )
    .bind(*org.as_uuid())
    .execute(pool)
    .await
    .unwrap();
}

async fn seed_user(pool: &PgPool, org: OrgId, role: &str, employee: Option<Uuid>) -> UserId {
    let user = UserId::new();
    sqlx::query(
        "INSERT INTO users (id, display_name, roles, is_active, org_id, employee_id) \
         VALUES ($1, $2, $3, true, $4, $5)",
    )
    .bind(*user.as_uuid())
    .bind(format!("payroll-{role}-{user}"))
    .bind(vec![role.to_owned()])
    .bind(*org.as_uuid())
    .bind(employee)
    .execute(pool)
    .await
    .unwrap();
    user
}

async fn seed_employee(pool: &PgPool, org: OrgId, name: &str) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO employees \
         (id, org_id, company, name, source_filename, source_sheet, source_row, source_key) \
         VALUES ($1, $2, 'KNL', $3, 'roster.xlsx', 'Sheet1', 1, $4)",
    )
    .bind(id)
    .bind(*org.as_uuid())
    .bind(name)
    .bind(format!("emp-{id}"))
    .execute(pool)
    .await
    .unwrap();
    id
}

async fn seed_run(pool: &PgPool, org: OrgId, actor: UserId) -> Uuid {
    let runtime_pool = runtime_role_pool(pool).await;
    let pay_run = PgPayRunPort::new(runtime_pool, tokio::runtime::Handle::current());
    let created = execute_sync(
        &pay_run,
        PayRunCommand {
            org_id: org,
            command_id: CommandId::from_uuid(Uuid::new_v4()),
            actor_id: actor,
            query: PayRunQuery::CreateRun {
                run_id: Uuid::new_v4(),
                period_start: date!(2026 - 06 - 01),
                period_end: date!(2026 - 06 - 30),
                connector: Some("m2".to_owned()),
                job: Some("payroll_draft".to_owned()),
            },
            action_key: "create_run".to_owned(),
            object_type_id: Uuid::nil(),
        },
    )
    .await
    .expect("payroll.create_run as console_rt");
    assert_eq!(created.target(), DispatchTarget::PayrollCreateRun);
    created.result()["draft_run_id"]
        .as_str()
        .expect("CreateRun must name draft_run_id")
        .parse()
        .unwrap()
}

/// One DRY_RUN import run + row whose `canonical_row.payroll` carries the
/// verified figures the calculate step reads verbatim.
async fn seed_verified_import_row(pool: &PgPool, org: OrgId) -> Uuid {
    let import_run: Uuid = sqlx::query_scalar(
        "INSERT INTO data_import_runs \
         (org_id, entity_type, status, source_filename, source_format, source_sha256, pay_period_start, pay_period_end) \
         VALUES ($1, 'employee_hr', 'DRY_RUN', 'payroll-source.xlsx', 'xlsx', $2, DATE '2026-06-01', DATE '2026-06-30') RETURNING id",
    )
    .bind(*org.as_uuid())
    .bind(SHA256_FIXTURE)
    .fetch_one(pool)
    .await
    .unwrap();
    sqlx::query_scalar(
        "INSERT INTO data_import_rows \
         (org_id, run_id, source_sheet, source_row, source_key, row_status, canonical_row) \
         VALUES ($1, $2, '급여', 2, 'alice-2026-06', 'CANDIDATE', $3) RETURNING id",
    )
    .bind(*org.as_uuid())
    .bind(import_run)
    .bind(json!({
        "payroll": {
            "monthly_gross_pay_won": 3_000_000,
            "nts_tax_row": {
                "table_version": "NTS-간이세액표-fixture-row-v1",
                "monthly_income_tax_won": 74_350,
                "local_income_tax_won": 7_430,
            },
        },
    }))
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn seed_calculable_line(
    pool: &PgPool,
    org: OrgId,
    run: Uuid,
    employee: Uuid,
    import_row: Uuid,
) {
    sqlx::query(
        "INSERT INTO payroll_draft_lines \
         (org_id, run_id, employee_id, employee_source_key, employee_display_name, \
          employee_company, attendance_source_row_count, overtime_hours, \
          gross_pay_source_present, nts_tax_row_status, source_data_import_row_ids) \
         VALUES ($1, $2, $3, $4, 'Alice', 'KNL', 1, 2.5, true, 'VERIFIED_SOURCE_ROW', $5)",
    )
    .bind(*org.as_uuid())
    .bind(run)
    .bind(employee)
    .bind(format!("src-{employee}"))
    .bind(vec![import_row])
    .execute(pool)
    .await
    .unwrap();
}

async fn seed_period_lock(pool: &PgPool, org: OrgId) {
    sqlx::query(
        "INSERT INTO period_locks (org_id, domain, period_start, period_end, reason) \
         VALUES ($1, 'payroll', $2, $3, '2026-06 급여 동결창')",
    )
    .bind(*org.as_uuid())
    .bind(date!(2026 - 06 - 01))
    .bind(date!(2026 - 06 - 30))
    .execute(pool)
    .await
    .unwrap();
}

/// Register a release-gate record that satisfies
/// `console_payroll_domain::validate_release_gate` on the run's `legal_basis`.
async fn register_release_gate(pool: &PgPool, run: Uuid) {
    sqlx::query("UPDATE payroll_draft_runs SET legal_basis = legal_basis || $2 WHERE id = $1")
        .bind(run)
        .bind(json!({
            "release_gate": {
                "rate_table_version": "statutory-rates-2026-06-27",
                "official_source_urls": [
                    "https://www.nps.or.kr/pnsinfo/ntpsklg/getOHAF0038M0.do",
                ],
                // The case carries its own kernel inputs because the gate now
                // RE-EXECUTES it: 2026-06-30 is this run's period_end, which
                // lifecycle.rs passes as pay_date, and it sits inside both the
                // contribution-rate window and the pension base-limit window,
                // so these figures are the same ones the domain fixture pins.
                // The tax row matches seed_verified_import_row below.
                "golden_cases": [{
                    "case_id": "GC-2026-06-A",
                    "rate_table_version": "statutory-rates-2026-06-27",
                    "professionally_validated": true,
                    "pay_date": "2026-06-30",
                    "monthly_gross_pay_won": 3_000_000,
                    "nts_tax_row": {
                        "table_version": "NTS-간이세액표-fixture-row-v1",
                        "monthly_income_tax_won": 74_350,
                        "local_income_tax_won": 7_430,
                    },
                    "expected_total_employee_deductions_won": 373_300,
                }],
                "professional_validation": {
                    "reviewer_kind": "labor_attorney",
                    "reviewed_on": "2026-07-01",
                    "artifact_sha256": SHA256_FIXTURE,
                    "reviewer_reference": "노무법인 검증 2026-07",
                },
            },
        }))
        .execute(pool)
        .await
        .unwrap();
}

struct EmptyTenantAppointedRun {
    org: OrgId,
    employee_id: Uuid,
    draft_run_id: Uuid,
    appointed: CommandReceipt,
}

async fn provision_empty_tenant_appointed_run(owner_pool: &PgPool) -> EmptyTenantAppointedRun {
    const ORG: Uuid = Uuid::from_u128(0xe3b0_0000_0000_0000_0000_0000_0000_00c1);
    let actor = seed_org_and_super_admin(owner_pool, ORG, "run-http").await;
    let runtime_pool = runtime_role_pool(owner_pool).await;
    let handle = tokio::runtime::Handle::current();
    let company = PgCompanyPort::new(runtime_pool.clone(), handle.clone());
    let units = PgOrgUnitPort::new(runtime_pool.clone(), handle.clone());
    let positions = PgJobPositionPort::new(runtime_pool.clone(), handle.clone());
    let persons = PgPersonPort::new(runtime_pool.clone(), handle.clone());
    let employment = PgEmploymentPort::new(runtime_pool.clone(), handle.clone());
    let pay_run = PgPayRunPort::new(runtime_pool.clone(), handle);
    let org = OrgId::from_uuid(ORG);

    execute_sync(
        &company,
        CompanyCommand {
            org_id: org,
            command_id: CommandId::from_uuid(Uuid::new_v4()),
            actor_id: actor,
            query: CompanyQuery {
                attributes: json!({ "legal_name": "주식회사 아크메" }),
            },
            action_key: "revise".to_owned(),
            object_type_id: Uuid::nil(),
        },
    )
    .await
    .unwrap();

    let unit_receipt = execute_sync(
        &units,
        OrgUnitCommand {
            org_id: org,
            command_id: CommandId::from_uuid(Uuid::new_v4()),
            actor_id: actor,
            query: OrgUnitQuery::Create {
                source: None,
                attributes: json!({ "name": "영업본부", "kind": "site" }),
            },
            action_key: "create_org_unit".to_owned(),
            object_type_id: Uuid::nil(),
        },
    )
    .await
    .unwrap();
    let sales: Uuid = unit_receipt.result()["org_unit_id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();

    let position_receipt = execute_sync(
        &positions,
        JobPositionCommand {
            org_id: org,
            command_id: CommandId::from_uuid(Uuid::new_v4()),
            actor_id: actor,
            query: JobPositionQuery::Create {
                org_unit_id: sales,
                attributes: json!({ "title": "백엔드 엔지니어" }),
            },
            action_key: "create_job_position".to_owned(),
            object_type_id: Uuid::nil(),
        },
    )
    .await
    .unwrap();
    let engineer: Uuid = position_receipt.result()["job_position_id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();

    let employee_id = Uuid::new_v4();
    let sales_text = sales.to_string();
    let engineer_text = engineer.to_string();
    let mut tx = runtime_pool.begin().await.unwrap();
    sqlx::query("SELECT set_config('app.current_org', $1, true)")
        .bind(ORG.to_string())
        .execute(&mut *tx)
        .await
        .unwrap();
    insert_employee_record(
        &mut tx,
        ORG,
        NewEmployeeRecord {
            employee_id,
            company: "ACME",
            name: "김직원",
            employee_number: "E-HTTP-1",
            org_unit: &sales_text,
            position: &engineer_text,
            worksite_name: "서울",
        },
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();

    execute_sync(
        &persons,
        PersonCommand {
            org_id: org,
            command_id: CommandId::from_uuid(Uuid::new_v4()),
            actor_id: actor,
            query: PersonQuery::Create {
                employee_id: Some(employee_id),
                attributes: json!({ "legal_name": "김직원" }),
            },
            action_key: "create_person".to_owned(),
            object_type_id: Uuid::nil(),
        },
    )
    .await
    .unwrap();

    let appointed = execute_sync(
        &employment,
        EmploymentCommand {
            org_id: org,
            command_id: CommandId::from_uuid(Uuid::new_v4()),
            actor_id: actor,
            query: EmploymentQuery::Appoint {
                employee_id,
                valid_from: OffsetDateTime::new_utc(date!(2026 - 01 - 01), Time::MIDNIGHT),
                attributes: EmploymentAttributes {
                    company: "ACME".to_owned(),
                    org_unit_id: Some(sales),
                    job_position_id: Some(engineer),
                    employment_status: "ACTIVE".to_owned(),
                },
            },
            action_key: "appoint".to_owned(),
            object_type_id: Uuid::nil(),
        },
    )
    .await
    .unwrap();

    let created = execute_sync(
        &pay_run,
        PayRunCommand {
            org_id: org,
            command_id: CommandId::from_uuid(Uuid::new_v4()),
            actor_id: actor,
            query: PayRunQuery::CreateRun {
                run_id: Uuid::new_v4(),
                period_start: date!(2026 - 06 - 01),
                period_end: date!(2026 - 06 - 30),
                connector: Some("m2".to_owned()),
                job: Some("payroll_draft".to_owned()),
            },
            action_key: "create_run".to_owned(),
            object_type_id: Uuid::nil(),
        },
    )
    .await
    .expect("create_run");
    assert_eq!(created.target(), DispatchTarget::PayrollCreateRun);
    let draft_run_id: Uuid = created.result()["draft_run_id"]
        .as_str()
        .expect("CreateRun must name draft_run_id")
        .parse()
        .unwrap();

    EmptyTenantAppointedRun {
        org,
        employee_id,
        draft_run_id,
        appointed,
    }
}

async fn execute_sync<P: CanonicalPort + Clone + Send + 'static>(
    port: &P,
    command: P::Command,
) -> Result<CommandReceipt, P::Error>
where
    P::Command: Send + 'static,
    P::Error: Send + 'static,
{
    let port = port.clone();
    tokio::task::spawn_blocking(move || port.execute(&command))
        .await
        .unwrap()
}
