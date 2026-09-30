#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! RUNTIME RLS gate for the GPS location-consent status + transition flow that
//! the 위치동의 (LocationSettingsPage / LocationConsentPanel) screen drives
//! (issue #19.7 — the consent page reportedly did not load / the feature looked
//! non-functional).
//!
//! The existing `location_store` tests run as the BYPASSRLS owner pool, so they
//! never exercise the consent read/write as the genuine non-owner `console_rt`
//! (NOSUPERUSER, NOBYPASSRLS, FORCE RLS) — a missing GRANT or a read that fails
//! to arm `app.current_org` would be invisible to them but 500 in production,
//! which is exactly the "page won't load" symptom. This test exercises the whole
//! flow as `console_rt` under org A's armed GUC:
//!   (a) `current_consent` on a user with NO row returns NO_RECORD (the initial
//!       page load must NOT error just because nothing was granted yet),
//!   (b) grant → suspend → resume → withdraw each succeed and the status the
//!       handler returns reflects the new state,
//!   (c) after the round-trip a fresh `current_consent` read still works and
//!       reports WITHDRAWN,
//!   (d) cross-tenant: with org B's GUC armed, org A's consent row is invisible
//!       (the read fails closed to NO_RECORD), never leaked.

use console_compliance_adapter_postgres::PgComplianceStore;
use console_compliance_application::{
    AcceptEvidenceBindingCommand, ConsentTransitionCommand, ConsentTransitionKind,
    CreateEvidenceBindingCommand,
};
use console_compliance_domain::{EvidenceConfidence, EvidenceTargetType, LocationConsentState};
use console_kernel_core::{BranchId, OrgId, TraceContext, UserId};
use console_platform_request_context::scope_org;
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use time::OffsetDateTime;
use time::macros::datetime;
use uuid::Uuid;

/// Second tenant org used for cross-tenant isolation assertions.
const ORG_B: Uuid = Uuid::from_u128(0xc000_0000_0000_0000_0000_0000_0000_0007);

/// Runtime-role pool: every connection becomes the genuine non-owner `console_rt`
/// (NOBYPASSRLS, subject to FORCE RLS), exactly like production.
async fn runtime_role_pool(owner_pool: &PgPool) -> PgPool {
    let options = owner_pool.connect_options().as_ref().clone();
    PgPoolOptions::new()
        .max_connections(4)
        .after_connect(|conn, _meta| {
            Box::pin(async move {
                sqlx::query("SET ROLE console_rt").execute(conn).await?;
                Ok(())
            })
        })
        .connect_with(options)
        .await
        .unwrap()
}

async fn seed_org(owner_pool: &PgPool, org: Uuid, tag: &str) {
    // `organizations.slug` is CHECKed against `^[a-z0-9][a-z0-9-]{1,38}[a-z0-9]$`
    // (0026_create_organizations.sql). Lowercasing alone is not enough: a tag such as
    // "Evidence A" became the slug "org-evidence a", and the space fails the constraint
    // during fixture setup, before a single assertion in this file runs.
    //
    // This test had never executed in CI, so the constraint could be added without anything
    // reporting that the fixture no longer satisfied it. Slugifying here rather than at the
    // two call sites that happen to contain a space, so a future tag cannot reintroduce it.
    let slug: String = tag
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    sqlx::query("INSERT INTO organizations (id, slug, name) VALUES ($1, $2, $3) ON CONFLICT (id) DO NOTHING")
        .bind(org)
        .bind(format!("org-{slug}"))
        .bind(format!("Org {tag}"))
        .execute(owner_pool)
        .await
        .unwrap();
}

/// Seed an org's region + branch + a MECHANIC user on that branch (owner pool,
/// raw inserts with explicit org_id), returning (user, branch).
async fn seed_user_and_branch(owner_pool: &PgPool, org: Uuid, tag: &str) -> (UserId, BranchId) {
    let region_id: Uuid =
        sqlx::query_scalar("INSERT INTO regions (name, org_id) VALUES ($1, $2) RETURNING id")
            .bind(format!("{tag} Region"))
            .bind(org)
            .fetch_one(owner_pool)
            .await
            .unwrap();
    let branch_id: Uuid = sqlx::query_scalar(
        "INSERT INTO branches (region_id, name, org_id) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(region_id)
    .bind(format!("{tag} Branch"))
    .bind(org)
    .fetch_one(owner_pool)
    .await
    .unwrap();
    let user_id: Uuid = sqlx::query_scalar(
        "INSERT INTO users (display_name, roles, org_id) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(format!("{tag} Mechanic"))
    .bind(Vec::from(["MECHANIC"]))
    .bind(org)
    .fetch_one(owner_pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO user_branches (user_id, branch_id, org_id) VALUES ($1, $2, $3)")
        .bind(user_id)
        .bind(branch_id)
        .bind(org)
        .execute(owner_pool)
        .await
        .unwrap();
    (UserId::from_uuid(user_id), BranchId::from_uuid(branch_id))
}

fn command(
    kind: ConsentTransitionKind,
    user_id: UserId,
    branch_id: BranchId,
    occurred_at: OffsetDateTime,
) -> ConsentTransitionCommand {
    ConsentTransitionCommand {
        kind,
        actor: Some(user_id),
        user_id,
        branch_id,
        trace: TraceContext::generate(),
        occurred_at,
    }
}

/// Runtime-role RLS gate for evidence acceptance: a binding from org A must be
/// indistinguishable from a missing binding under org B, and the rejected read
/// must not emit an audit event in either tenant.
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn evidence_acceptance_is_tenant_invisible_and_does_not_leak_audit_as_runtime_role(
    owner_pool: PgPool,
) {
    let rt_pool = runtime_role_pool(&owner_pool).await;
    let org_a = OrgId::knl();
    let org_a_uuid = *org_a.as_uuid();
    let org_b = OrgId::from_uuid(ORG_B);

    seed_org(&owner_pool, org_a_uuid, "Evidence A").await;
    seed_org(&owner_pool, ORG_B, "Evidence B").await;
    let (actor_a, _branch_a) = seed_user_and_branch(&owner_pool, org_a_uuid, "Evidence A").await;
    let control_a = seed_compliance_control(&owner_pool, org_a_uuid, actor_a, "A").await;

    let binding = scope_org(org_a, async {
        PgComplianceStore::new(owner_pool.clone())
            .create_evidence_binding(CreateEvidenceBindingCommand {
                actor: actor_a,
                control_id: control_a,
                obligation_id: None,
                evidence_target_type: EvidenceTargetType::ExternalDocument,
                evidence_target_id: "tenant-a-private-evidence".to_owned(),
                source_audit_event_id: None,
                confidence: EvidenceConfidence::High,
                collected_at: None,
                collected_by: None,
                valid_from: None,
                valid_to: None,
                hash_sha256: None,
                metadata: serde_json::json!({"tenant": "A"}),
                trace: TraceContext::generate(),
                occurred_at: datetime!(2026-07-24 12:00:00 UTC),
            })
            .await
            .expect("owner seeds an org-A proposed evidence binding")
    })
    .await;

    let denied = scope_org(org_b, async {
        PgComplianceStore::new(rt_pool.clone())
            .accept_evidence_binding(AcceptEvidenceBindingCommand {
                actor: actor_a,
                id: binding.id,
                trace: TraceContext::generate(),
                occurred_at: datetime!(2026-07-24 12:01:00 UTC),
            })
            .await
            .expect_err("org-B runtime role must not see or accept org-A evidence")
    })
    .await;
    assert_eq!(denied.kind(), console_kernel_core::ErrorKind::NotFound);

    let audit_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM audit_events WHERE action = 'compliance.evidence_binding.accept' AND target_id = $1",
    )
    .bind(binding.id.to_string())
    .fetch_one(&owner_pool)
    .await
    .unwrap();
    assert_eq!(
        audit_count, 0,
        "an RLS-hidden binding must not produce an audit leak"
    );
}

async fn seed_compliance_control(owner_pool: &PgPool, org: Uuid, actor: UserId, tag: &str) -> Uuid {
    let framework_id: Uuid = sqlx::query_scalar(
        "INSERT INTO compliance_frameworks (org_id, code, name, version_label, framework_kind, created_by, updated_by) VALUES ($1, $2, $3, $4, $5, $6, $6) RETURNING id",
    )
    .bind(org)
    // `compliance_frameworks.code` is CHECKed against `^FW-[0-9]{4,}$`
    // (0148_create_compliance_domain.sql:122). The literal this used —
    // `RLS-EVIDENCE-A` — has never been a legal code for this column. Derived from the
    // tag so two frameworks in one test stay distinct, though only (id, org_id) is UNIQUE.
    .bind(format!("FW-{:04}", tag.bytes().map(u32::from).sum::<u32>() % 10_000))
    .bind(format!("RLS evidence framework {tag}"))
    .bind("2026.07")
    .bind("INTERNAL_CONTROL")
    .bind(*actor.as_uuid())
    .fetch_one(owner_pool)
    .await
    .unwrap();
    sqlx::query_scalar(
        "INSERT INTO compliance_controls (org_id, framework_id, control_key, title, objective, control_type, created_by, updated_by) VALUES ($1, $2, $3, $4, $5, $6, $7, $7) RETURNING id",
    )
    .bind(org)
    .bind(framework_id)
    // `control_key` is CHECKed against `^[A-Z0-9][A-Z0-9._-]{0,63}$` — uppercase only, no
    // spaces. The single current caller passes "A" and happens to satisfy it; a tag like
    // "Evidence A" would not, which is the same trap the org slug above already sprang.
    .bind(format!(
        "RLS.EVIDENCE.{}",
        tag.to_uppercase()
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
            .collect::<String>()
    ))
    .bind(format!("RLS evidence control {tag}"))
    .bind("RLS evidence acceptance isolation")
    .bind("DETECTIVE")
    .bind(*actor.as_uuid())
    .fetch_one(owner_pool)
    .await
    .unwrap()
}

// ===========================================================================
// (a)(b)(c) status read + full grant/suspend/resume/withdraw flow as console_rt.
// ===========================================================================
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn consent_status_and_transitions_succeed_as_runtime_role(owner_pool: PgPool) {
    let rt_pool = runtime_role_pool(&owner_pool).await;
    let org_a = OrgId::knl();
    let org_uuid = *org_a.as_uuid();

    seed_org(&owner_pool, org_uuid, "A").await;
    let (user_id, branch_id) = seed_user_and_branch(&owner_pool, org_uuid, "A").await;

    scope_org(org_a, async {
        let store = PgComplianceStore::new(rt_pool.clone());

        // (a) The initial page load: no consent row yet → NO_RECORD, NOT an error.
        let initial = store
            .current_consent(user_id, branch_id)
            .await
            .expect("current_consent must read (NO_RECORD) as console_rt under org-A's GUC");
        assert_eq!(initial.state(), LocationConsentState::NoRecord);

        // (b) grant → suspend → resume → withdraw, each surfacing the new state.
        let granted = store
            .transition_consent(command(
                ConsentTransitionKind::Grant,
                user_id,
                branch_id,
                datetime!(2026-06-23 08:00:00 UTC),
            ))
            .await
            .expect("grant must succeed as console_rt");
        assert_eq!(granted.state(), LocationConsentState::Granted);

        let suspended = store
            .transition_consent(command(
                ConsentTransitionKind::Suspend,
                user_id,
                branch_id,
                datetime!(2026-06-23 09:00:00 UTC),
            ))
            .await
            .expect("suspend must succeed as console_rt");
        assert_eq!(suspended.state(), LocationConsentState::Suspended);

        let resumed = store
            .transition_consent(command(
                ConsentTransitionKind::Resume,
                user_id,
                branch_id,
                datetime!(2026-06-23 10:00:00 UTC),
            ))
            .await
            .expect("resume must succeed as console_rt");
        assert_eq!(resumed.state(), LocationConsentState::Granted);

        let withdrawn = store
            .transition_consent(command(
                ConsentTransitionKind::Withdraw,
                user_id,
                branch_id,
                datetime!(2026-06-23 11:00:00 UTC),
            ))
            .await
            .expect("withdraw must succeed as console_rt");
        assert_eq!(withdrawn.state(), LocationConsentState::Withdrawn);

        // (c) A fresh read after the round-trip still works and is WITHDRAWN.
        let after = store
            .current_consent(user_id, branch_id)
            .await
            .expect("current_consent must still read as console_rt after the round-trip");
        assert_eq!(after.state(), LocationConsentState::Withdrawn);
    })
    .await;

    // Every transition was audited (grant/suspend/resume/withdraw) under org A.
    let consent_audits: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM audit_events WHERE target_type = 'location_consent' AND org_id = $1",
    )
    .bind(org_uuid)
    .fetch_one(&owner_pool)
    .await
    .unwrap();
    assert_eq!(consent_audits, 4, "all four transitions must be audited");
}

// ===========================================================================
// (d) Cross-tenant: org A's consent is invisible under org B's armed GUC.
// ===========================================================================
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn consent_read_is_tenant_isolated_as_runtime_role(owner_pool: PgPool) {
    let rt_pool = runtime_role_pool(&owner_pool).await;
    let org_a = OrgId::knl();
    let org_a_uuid = *org_a.as_uuid();
    let org_b = OrgId::from_uuid(ORG_B);

    seed_org(&owner_pool, org_a_uuid, "A").await;
    seed_org(&owner_pool, ORG_B, "B").await;
    let (user_a, branch_a) = seed_user_and_branch(&owner_pool, org_a_uuid, "A").await;

    // Org A grants consent (as console_rt, under org A's GUC).
    scope_org(org_a, async {
        let store = PgComplianceStore::new(rt_pool.clone());
        store
            .transition_consent(command(
                ConsentTransitionKind::Grant,
                user_a,
                branch_a,
                datetime!(2026-06-23 08:00:00 UTC),
            ))
            .await
            .expect("org A grant must succeed as console_rt");
    })
    .await;

    // Under org B's GUC, org A's user has no visible consent row — the FORCE RLS
    // policy hides it, so the read fails closed to NO_RECORD rather than leaking
    // org A's GRANTED state across the tenant boundary.
    scope_org(org_b, async {
        let store = PgComplianceStore::new(rt_pool.clone());
        let seen = store
            .current_consent(user_a, branch_a)
            .await
            .expect("read must succeed as console_rt under org B's GUC");
        assert_eq!(
            seen.state(),
            LocationConsentState::NoRecord,
            "org A's consent must be invisible to org B (tenant isolation)"
        );
    })
    .await;
}
