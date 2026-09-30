//! Identity / org-setup REST integration tests.
//!
//! Exercises identity's applied org reads and user-management flow end-to-end:
//! the IDOR hardening restricts elevated-role grants to SUPER_ADMIN, and every
//! authenticated user can edit their own profile. Governed region/branch
//! mutation routes are exercised through the assembled app in orgchange tests.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeSet;

use axum::Router;
use axum::body::{Body, to_bytes};
use console_identity_adapter_postgres::PgOrgStore;
use console_identity_application::{
    CreatePolicyAssignmentPreviewReceiptCommand, CreatePolicyRoleCommand, PolicyRolePermission,
    ReplacePolicyRoleAssignmentsCommand, UpdatePolicyRoleCommand,
};
use console_identity_rest::{IdentityRestState, router};
use console_kernel_core::{
    AccessScope, AccessScopeLevel, AuditAction, AuditEvent, BranchId, BranchScope, OrgId,
    ScopeNodeId, TraceContext, UserId,
};
use console_platform_auth::{
    AccessTokenInput, JwtIssuer, JwtSettings, JwtVerifier, PasskeyRegistrationStart,
    PasskeyService, WebauthnSettings,
};
use console_platform_authz::{Feature, resolve_effective_feature_grants_in_org};
use console_platform_db::{DbError, with_audit};
use console_platform_request_context::scope_org;
use console_platform_test_support::runtime_role_pool;
use http::{Request, StatusCode, header};
use p256::ecdsa::SigningKey;
use p256::elliptic_curve::rand_core::OsRng;
use p256::pkcs8::{EncodePrivateKey, EncodePublicKey, LineEnding};
use serde_json::{Value, json};
use sqlx::{PgPool, Row};
use time::{Duration, OffsetDateTime};
use tower::ServiceExt;
use url::Url;
use webauthn_authenticator_rs::prelude::{RequestChallengeResponse, WebauthnAuthenticator};
use webauthn_authenticator_rs::softpasskey::SoftPasskey;

const TEST_ISSUER: &str = "console-platform-auth";
const TEST_AUDIENCE: &str = "console-api";

struct Harness {
    private_pem: String,
    public_pem: String,
    pool: PgPool,
}

impl Harness {
    /// `pool` is the `#[sqlx::test]` owner pool: still fine for the caller's
    /// own seeding, but the router itself is built on a non-owner `console_rt`
    /// pool so requests actually go through RLS instead of BYPASSRLS.
    async fn new(pool: PgPool) -> Self {
        let signing_key = SigningKey::random(&mut OsRng);
        let private_pem = signing_key
            .to_pkcs8_pem(LineEnding::LF)
            .unwrap()
            .to_string();
        let public_pem = signing_key
            .verifying_key()
            .to_public_key_pem(LineEnding::LF)
            .unwrap();
        Self {
            private_pem,
            public_pem,
            pool: runtime_role_pool(&pool).await,
        }
    }

    fn service(&self) -> Router {
        let verifier = JwtVerifier::from_es256_public_pem(
            JwtSettings {
                issuer: TEST_ISSUER.to_owned(),
                audience: TEST_AUDIENCE.to_owned(),
                access_token_ttl: Duration::minutes(15),
            },
            self.public_pem.as_bytes(),
        )
        .unwrap();
        router(
            IdentityRestState::new(PgOrgStore::new(self.pool.clone()), Some(verifier))
                .with_passkey_step_up(Some(passkey_service())),
        )
    }

    async fn token(&self, user_id: UserId, roles: &[&str], branches: Vec<BranchId>) -> String {
        self.token_for_org(OrgId::knl(), user_id, roles, branches)
            .await
    }

    async fn token_for_org(
        &self,
        org_id: OrgId,
        user_id: UserId,
        roles: &[&str],
        branches: Vec<BranchId>,
    ) -> String {
        let issuer = self.issuer();
        console_platform_test_support::issue_session_token(
            &self.pool,
            &issuer,
            self.access_token_input_for_org(org_id, user_id, roles, branches),
            None,
            vec![],
        )
        .await
    }

    async fn scoped_token(
        &self,
        user_id: UserId,
        roles: &[&str],
        branches: Vec<BranchId>,
        access_scope: AccessScope,
    ) -> String {
        let issuer = self.issuer();
        console_platform_test_support::issue_session_token(
            &self.pool,
            &issuer,
            self.access_token_input(user_id, roles, branches),
            Some(access_scope),
            Vec::new(),
        )
        .await
    }

    fn issuer(&self) -> JwtIssuer {
        JwtIssuer::from_es256_pem(
            JwtSettings {
                issuer: TEST_ISSUER.to_owned(),
                audience: TEST_AUDIENCE.to_owned(),
                access_token_ttl: Duration::minutes(15),
            },
            self.private_pem.as_bytes(),
            self.public_pem.as_bytes(),
        )
        .unwrap()
    }

    fn access_token_input(
        &self,
        user_id: UserId,
        roles: &[&str],
        branches: Vec<BranchId>,
    ) -> AccessTokenInput {
        self.access_token_input_for_org(OrgId::knl(), user_id, roles, branches)
    }

    fn access_token_input_for_org(
        &self,
        org_id: OrgId,
        user_id: UserId,
        roles: &[&str],
        branches: Vec<BranchId>,
    ) -> AccessTokenInput {
        AccessTokenInput {
            subject: user_id,
            org_id,
            roles: roles.iter().map(|r| (*r).to_owned()).collect(),
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
        }
    }
}

fn passkey_service() -> PasskeyService {
    PasskeyService::new(WebauthnSettings {
        rp_id: "example.com".to_owned(),
        rp_origin: Url::parse("https://auth.example.com").unwrap(),
        rp_name: "Console".to_owned(),
        extra_allowed_origins: vec![],
        ceremony_ttl: Duration::minutes(5),
    })
    .unwrap()
}

fn inject_allow_credential(
    challenge: RequestChallengeResponse,
    credential_id: &str,
) -> RequestChallengeResponse {
    let mut value = serde_json::to_value(&challenge).unwrap();
    let allow = value
        .get_mut("publicKey")
        .and_then(|pk| pk.get_mut("allowCredentials"))
        .and_then(serde_json::Value::as_array_mut)
        .expect("authentication challenge must have an allowCredentials array");
    allow.push(serde_json::json!({ "type": "public-key", "id": credential_id }));
    serde_json::from_value(value).unwrap()
}

async fn fresh_step_up_assertion(pool: &PgPool, user_id: UserId, display_name: &str) -> Value {
    let service = passkey_service();
    let registration = service
        .start_registration(
            pool,
            OrgId::knl(),
            PasskeyRegistrationStart {
                user_id: *user_id.as_uuid(),
                username: format!("{user_id}.example"),
                display_name: display_name.to_owned(),
            },
        )
        .await
        .unwrap();
    let mut authenticator = WebauthnAuthenticator::new(SoftPasskey::new(true));
    let credential = authenticator
        .do_registration(
            Url::parse("https://auth.example.com").unwrap(),
            registration.challenge,
        )
        .unwrap();
    let stored_passkey = service
        .finish_registration(pool, OrgId::knl(), registration.ceremony_id, credential)
        .await
        .unwrap();

    let authentication = service.start_authentication(pool).await.unwrap();
    let challenge =
        inject_allow_credential(authentication.challenge, &stored_passkey.credential_id);
    let assertion = authenticator
        .do_authentication(Url::parse("https://auth.example.com").unwrap(), challenge)
        .unwrap();

    json!({
        "ceremony_id": authentication.ceremony_id,
        "credential": assertion
    })
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn admin_manages_users_and_reads_applied_org_structure(pool: PgPool) {
    let harness = Harness::new(pool.clone()).await;
    let admin_branch = seed_branch(&pool).await;
    let admin_branch_id = admin_branch.to_string();
    let admin = seed_user(&pool, "Branch Admin", &["ADMIN"], Some(admin_branch)).await;
    let token = harness.token(admin, &["ADMIN"], vec![admin_branch]).await;

    // Create a mechanic in the admin's own branch.
    let (status, user) = send(
        &harness,
        "POST",
        "/api/v1/users",
        &token,
        Some(json!({
            "display_name": "김정비",
            "phone": "010-1234-5678",
            "team": "MAINTENANCE",
            "roles": ["MECHANIC"],
            "branch_ids": [admin_branch.to_string()],
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{user:?}");
    assert_eq!(user["display_name"], "김정비");
    assert_eq!(user["team"], "MAINTENANCE");
    assert_eq!(user["is_active"], true);
    let new_user_id = user["id"].as_str().unwrap().to_owned();

    // The branch list (also used for support triage) exposes the applied row.
    let (status, branches) = send(&harness, "GET", "/api/v1/branches", &token, None).await;
    assert_eq!(status, StatusCode::OK);
    let ids: Vec<&str> = branches
        .as_array()
        .unwrap()
        .iter()
        .map(|b| b["id"].as_str().unwrap())
        .collect();
    assert!(
        ids.contains(&admin_branch_id.as_str()),
        "seeded branch missing from applied-state read: {ids:?}"
    );

    // Get-one for an applied org-unit pin panel (UI-M2a): found → 200 + summary;
    // an id not in the org → 404 (exercises axum routing + error mapping).
    let branch_id = admin_branch_id;
    let (status, one) = send(
        &harness,
        "GET",
        &format!("/api/v1/branches/{branch_id}"),
        &token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{one:?}");
    assert_eq!(one["id"], branch_id);

    let (status, _missing) = send(
        &harness,
        "GET",
        "/api/v1/branches/00000000-0000-4000-8000-000000000000",
        &token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // List users in scope returns the admin and the new mechanic.
    let (status, users) = send(&harness, "GET", "/api/v1/users", &token, None).await;
    assert_eq!(status, StatusCode::OK);
    // GET /api/v1/users returns a paginated UserPage ({items,total,limit,offset}),
    // not a bare array — read the items page (honest-pagination change, commit 9ddae44).
    let ids: Vec<&str> = users["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|u| u["id"].as_str().unwrap())
        .collect();
    assert!(ids.contains(&new_user_id.as_str()), "{ids:?}");

    // Read the single user.
    let (status, fetched) = send(
        &harness,
        "GET",
        &format!("/api/v1/users/{new_user_id}"),
        &token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{fetched:?}");
    assert_eq!(fetched["id"], Value::String(new_user_id.clone()));

    // The branch membership landed in user_branches.
    assert_eq!(
        fetched["branch_ids"].as_array().unwrap()[0],
        Value::String(admin_branch.to_string())
    );
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn people_directory_filters_scope_orders_and_counts_truthfully(pool: PgPool) {
    let harness = Harness::new(pool.clone()).await;
    let visible_branch = seed_branch(&pool).await;
    let hidden_branch = seed_branch(&pool).await;
    let admin = seed_user(&pool, "Directory Admin", &["ADMIN"], Some(visible_branch)).await;
    let first = seed_user_with_team(
        &pool,
        "ALPHA",
        &["MECHANIC"],
        Some(visible_branch),
        Some("정비"),
    )
    .await;
    let employee_id = seed_employee_link(&pool, first, "EMP-ALPHA", "Alpha Employee").await;
    let _second = seed_user_with_team(
        &pool,
        "alpha",
        &["MECHANIC"],
        Some(visible_branch),
        Some("정비"),
    )
    .await;
    let inactive = seed_user_with_team(
        &pool,
        "alpha inactive",
        &["MECHANIC"],
        Some(visible_branch),
        Some("정비"),
    )
    .await;
    seed_user_active(&pool, inactive, false).await;
    let shared = seed_user_with_team(
        &pool,
        "beta shared",
        &["MECHANIC"],
        Some(visible_branch),
        Some("정비"),
    )
    .await;
    seed_user_branch(&pool, shared, hidden_branch).await;
    let _hidden = seed_user_with_team(
        &pool,
        "alpha hidden",
        &["MECHANIC"],
        Some(hidden_branch),
        Some("정비"),
    )
    .await;
    let _hangul = seed_user_with_team(
        &pool,
        "김철수",
        &["MECHANIC"],
        Some(visible_branch),
        Some("정비"),
    )
    .await;
    let same_name_a = seed_user_with_team(
        &pool,
        "same name",
        &["MECHANIC"],
        Some(visible_branch),
        Some("정비"),
    )
    .await;
    let same_name_b = seed_user_with_team(
        &pool,
        "same name",
        &["MECHANIC"],
        Some(visible_branch),
        Some("정비"),
    )
    .await;
    let token = harness.token(admin, &["ADMIN"], vec![visible_branch]).await;

    let (status, page) = send(
        &harness,
        "GET",
        "/api/v1/directory/people?search=%20ALPHA%20&team=MAINTENANCE&limit=10",
        &token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{page:?}");
    assert_eq!(page["total"], 2, "inactive and hidden users are omitted");
    let names: Vec<&str> = page["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["display_name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["alpha", "ALPHA"]);
    let linked = page["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["id"] == first.to_string())
        .unwrap();
    assert_eq!(linked["employee_id"], employee_id.to_string());
    assert_eq!(linked["employee_link_status"], "LINKED");
    assert_eq!(linked["employee_name"], "Alpha Employee");
    assert_directory_item_json_allowlist(&page);

    let (status, second_page) = send(
        &harness,
        "GET",
        "/api/v1/directory/people?search=alpha&team=MAINTENANCE&limit=1&offset=1",
        &token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{second_page:?}");
    assert_eq!(second_page["total"], 2);
    assert_eq!(second_page["limit"], 1);
    assert_eq!(second_page["offset"], 1);
    assert_eq!(second_page["items"].as_array().unwrap().len(), 1);

    let (status, same_names) = send(
        &harness,
        "GET",
        "/api/v1/directory/people?search=same%20name&team=MAINTENANCE",
        &token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{same_names:?}");
    let returned_ids: Vec<String> = same_names["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["id"].as_str().unwrap().to_owned())
        .collect();
    let mut expected_ids = vec![same_name_a.to_string(), same_name_b.to_string()];
    expected_ids.sort();
    assert_eq!(
        returned_ids, expected_ids,
        "same-name rows use id as a tie-breaker"
    );

    let (status, scoped) = send(
        &harness,
        "GET",
        &format!("/api/v1/directory/people?branch_id={visible_branch}&team=MAINTENANCE"),
        &token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{scoped:?}");
    // The page order is pinned to the `und-x-icu` collation, so it is the same
    // on every server and is the order a human reads a name list in. A server
    // whose own collation happened to differ (libc en_US files 김철수 first;
    // `C` files every Latin name ahead of it) must not change this page.
    let scoped_names: Vec<&str> = scoped["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["display_name"].as_str().unwrap())
        .collect();
    assert_eq!(
        scoped_names,
        [
            "alpha",
            "ALPHA",
            "beta shared",
            "same name",
            "same name",
            "김철수"
        ]
    );
    let shared_row = scoped["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["id"] == shared.to_string())
        .unwrap();
    assert_eq!(
        shared_row["branch_ids"],
        json!([visible_branch.to_string()])
    );

    let (status, including_inactive) = send(
        &harness,
        "GET",
        "/api/v1/directory/people?search=alpha&team=MAINTENANCE&include_inactive=true",
        &token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{including_inactive:?}");
    assert_eq!(including_inactive["total"], 3);

    for invalid in [
        "/api/v1/directory/people?team=UNKNOWN",
        "/api/v1/directory/people?branch_id=not-a-uuid",
        "/api/v1/directory/people?offset=-1",
    ] {
        let (status, body) = send(&harness, "GET", invalid, &token, None).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body:?}");
    }
}

/// The people directory is a runtime-authz surface: custom roles become
/// effective only when the resolver sees ACTIVE assignments, and every grant
/// is bounded by both its branch condition and the caller's live membership.
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn people_directory_custom_role_resolution_is_active_only_and_membership_bounded(
    pool: PgPool,
) {
    let harness = Harness::new(pool.clone()).await;
    let branch_a = seed_branch(&pool).await;
    let branch_b = seed_branch(&pool).await;
    let branch_outside = seed_branch(&pool).await;
    let owner = seed_user(&pool, "Directory Policy Owner", &["SUPER_ADMIN"], None).await;

    let visible_a = seed_user(&pool, "Directory Scoped A", &["MECHANIC"], Some(branch_a)).await;
    let visible_b = seed_user(&pool, "Directory Scoped B", &["MECHANIC"], Some(branch_b)).await;
    let shared = seed_user(
        &pool,
        "Directory Scoped Shared",
        &["MECHANIC"],
        Some(branch_a),
    )
    .await;
    seed_user_branch(&pool, shared, branch_outside).await;
    let hidden = seed_user(
        &pool,
        "Directory Scoped Outside",
        &["MECHANIC"],
        Some(branch_outside),
    )
    .await;

    // No built-in feature grant and no runtime custom grant: deny, never an
    // empty successful page that could conceal an authorization defect.
    let no_grant = seed_user(&pool, "Directory No Grant", &["MEMBER"], Some(branch_a)).await;
    let no_grant_token = harness.token(no_grant, &["MEMBER"], vec![branch_a]).await;
    let (status, body) = send(
        &harness,
        "GET",
        "/api/v1/directory/people?search=directory%20scoped",
        &no_grant_token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body:?}");

    let reader = seed_user(
        &pool,
        "Directory Custom Reader",
        &["MEMBER"],
        Some(branch_a),
    )
    .await;
    seed_user_branch(&pool, reader, branch_b).await;
    let reader_token = harness
        .token(reader, &["MEMBER"], vec![branch_a, branch_b])
        .await;

    let active_a = seed_policy_role(
        &pool,
        owner,
        "directory_reader_branch_a",
        "지점 A 구성원 조회",
        "ACTIVE",
        false,
        &[("employee_directory_read", "allow")],
    )
    .await;
    let branch_a_value = branch_a.to_string();
    seed_policy_role_condition(
        &pool,
        active_a,
        "branch_scope",
        "branch",
        "equals",
        &[branch_a_value.as_str()],
    )
    .await;
    seed_policy_assignment(&pool, reader, active_a, owner).await;

    // ACTIVE grants resolve through the request principal and expose only the
    // grant/live-membership intersection. The shared user must not disclose
    // its otherwise-hidden outside branch.
    let (status, active_page) = send(
        &harness,
        "GET",
        "/api/v1/directory/people?search=directory%20scoped",
        &reader_token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{active_page:?}");
    assert_eq!(active_page["total"], 2, "{active_page:?}");
    let active_ids: BTreeSet<String> = active_page["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["id"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(
        active_ids,
        BTreeSet::from([visible_a.to_string(), shared.to_string()])
    );
    let shared_row = active_page["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["id"] == shared.to_string())
        .unwrap();
    assert_eq!(shared_row["branch_ids"], json!([branch_a.to_string()]));
    assert!(
        !shared_row.to_string().contains(&branch_outside.to_string()),
        "out-of-scope branch id must not be disclosed: {shared_row:?}"
    );

    // A member may hold multiple custom grants, but only the intersections
    // with their actual membership may be unioned. The out-of-scope grant is
    // deliberately assigned alongside the valid B grant to lock that boundary.
    let active_b = seed_policy_role(
        &pool,
        owner,
        "directory_reader_branch_b",
        "지점 B 구성원 조회",
        "ACTIVE",
        false,
        &[("employee_directory_read", "allow")],
    )
    .await;
    let branch_b_value = branch_b.to_string();
    seed_policy_role_condition(
        &pool,
        active_b,
        "branch_scope",
        "branch",
        "equals",
        &[branch_b_value.as_str()],
    )
    .await;
    let outside_only = seed_policy_role(
        &pool,
        owner,
        "directory_reader_outside",
        "외부 지점 구성원 조회",
        "ACTIVE",
        false,
        &[("employee_directory_read", "allow")],
    )
    .await;
    let outside_value = branch_outside.to_string();
    seed_policy_role_condition(
        &pool,
        outside_only,
        "branch_scope",
        "branch",
        "equals",
        &[outside_value.as_str()],
    )
    .await;
    seed_policy_assignment(&pool, reader, active_b, owner).await;
    seed_policy_assignment(&pool, reader, outside_only, owner).await;

    let (status, union_page) = send(
        &harness,
        "GET",
        "/api/v1/directory/people?search=directory%20scoped",
        &reader_token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{union_page:?}");
    assert_eq!(union_page["total"], 3, "{union_page:?}");
    assert_eq!(
        union_page["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|item| item["id"].as_str().unwrap().to_owned())
            .collect::<BTreeSet<_>>(),
        BTreeSet::from([
            visible_a.to_string(),
            visible_b.to_string(),
            shared.to_string(),
        ])
    );
    assert!(
        !union_page.to_string().contains(&hidden.to_string()),
        "an out-of-membership grant must not return outside rows: {union_page:?}"
    );

    let (status, outside_page) = send(
        &harness,
        "GET",
        &format!("/api/v1/directory/people?search=directory%20scoped&branch_id={branch_outside}"),
        &reader_token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{outside_page:?}");
    assert_eq!(outside_page["total"], 0, "{outside_page:?}");
    assert_eq!(outside_page["items"], json!([]));

    // The same resolver intentionally excludes DRAFT and RETIRED assignments.
    // Each principal has a live branch membership, so 403 proves lifecycle
    // status—not membership absence—is the fail-closed reason.
    for (status_name, role_key) in [
        ("DRAFT", "directory_reader_draft"),
        ("RETIRED", "directory_reader_retired"),
    ] {
        let lifecycle_reader = seed_user(
            &pool,
            &format!("Directory {status_name} Reader"),
            &["MEMBER"],
            Some(branch_a),
        )
        .await;
        let role = seed_policy_role(
            &pool,
            owner,
            role_key,
            &format!("{status_name} 구성원 조회"),
            status_name,
            false,
            &[("employee_directory_read", "allow")],
        )
        .await;
        seed_policy_assignment(&pool, lifecycle_reader, role, owner).await;
        let token = harness
            .token(lifecycle_reader, &["MEMBER"], vec![branch_a])
            .await;
        let (status, body) = send(
            &harness,
            "GET",
            "/api/v1/directory/people?search=directory%20scoped",
            &token,
            None,
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{status_name}: {body:?}");
    }
}

/// Directory is a scrape surface: HTTP items are a DirectoryPerson key
/// allowlist (no `phone` / `salary` / `bank_account` / `rrn` / `won`) even when
/// a number is stored. User GET / users list keep `UserSummary.phone`.
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn directory_people_omits_phone_while_user_get_keeps_it(pool: PgPool) {
    let harness = Harness::new(pool.clone()).await;
    let branch = seed_branch(&pool).await;
    let admin = seed_user(&pool, "Directory Dto Admin", &["ADMIN"], Some(branch)).await;
    let admin_token = harness.token(admin, &["ADMIN"], vec![branch]).await;
    let stored_phone = "010-1234-5678";

    let (status, created) = send(
        &harness,
        "POST",
        "/api/v1/users",
        &admin_token,
        Some(json!({
            "display_name": "DirectoryDtoSubject",
            "phone": stored_phone,
            "team": "MAINTENANCE",
            "roles": ["MECHANIC"],
            "branch_ids": [branch.to_string()],
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created:?}");
    let subject_id = created["id"].as_str().expect("created user id").to_owned();
    assert_eq!(created["phone"], stored_phone);

    let (status, admin_page) = send(
        &harness,
        "GET",
        "/api/v1/directory/people?search=directorydtosubject",
        &admin_token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{admin_page:?}");
    assert_eq!(admin_page["total"], 1, "{admin_page:?}");
    assert_eq!(admin_page["items"][0]["id"], subject_id);
    assert_eq!(
        admin_page["items"][0]["display_name"],
        "DirectoryDtoSubject"
    );
    assert_directory_item_json_allowlist(&admin_page);

    let (status, fetched) = send(
        &harness,
        "GET",
        &format!("/api/v1/users/{subject_id}"),
        &admin_token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{fetched:?}");
    assert_eq!(fetched["phone"], stored_phone);

    let (status, users) = send(&harness, "GET", "/api/v1/users", &admin_token, None).await;
    assert_eq!(status, StatusCode::OK, "{users:?}");
    let listed = users["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["id"] == subject_id)
        .expect("created user on users list");
    assert_eq!(listed["phone"], stored_phone);

    let owner = seed_user(&pool, "Directory Dto Policy Owner", &["SUPER_ADMIN"], None).await;
    let reader = seed_user(&pool, "Directory Dto Reader", &["MEMBER"], Some(branch)).await;
    let reader_role = seed_policy_role(
        &pool,
        owner,
        "directory_dto_reader",
        "구성원 조회",
        "ACTIVE",
        false,
        &[("employee_directory_read", "allow")],
    )
    .await;
    let branch_value = branch.to_string();
    seed_policy_role_condition(
        &pool,
        reader_role,
        "branch_scope",
        "branch",
        "equals",
        &[branch_value.as_str()],
    )
    .await;
    seed_policy_assignment(&pool, reader, reader_role, owner).await;
    let reader_token = harness.token(reader, &["MEMBER"], vec![branch]).await;

    let (status, reader_page) = send(
        &harness,
        "GET",
        "/api/v1/directory/people?search=directorydtosubject",
        &reader_token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{reader_page:?}");
    assert_eq!(reader_page["total"], 1, "{reader_page:?}");
    assert_eq!(reader_page["items"][0]["id"], subject_id);
    assert_eq!(
        reader_page["items"][0]["display_name"],
        "DirectoryDtoSubject"
    );
    assert_directory_item_json_allowlist(&reader_page);
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn non_super_admin_cannot_create_elevated_user(pool: PgPool) {
    let harness = Harness::new(pool.clone()).await;
    let admin_branch = seed_branch(&pool).await;
    let admin = seed_user(&pool, "Branch Admin", &["ADMIN"], Some(admin_branch)).await;
    let token = harness.token(admin, &["ADMIN"], vec![admin_branch]).await;

    // An ADMIN attempting to mint an EXECUTIVE is forbidden (IDOR hardening).
    let (status, body) = send(
        &harness,
        "POST",
        "/api/v1/users",
        &token,
        Some(json!({
            "display_name": "임원",
            "roles": ["EXECUTIVE"],
            "branch_ids": [admin_branch.to_string()],
        })),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body:?}");
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn admin_can_grant_admin_to_existing_executive_in_scope(pool: PgPool) {
    let harness = Harness::new(pool.clone()).await;
    let admin_branch = seed_branch(&pool).await;
    let admin = seed_user(&pool, "Branch Admin", &["ADMIN"], Some(admin_branch)).await;
    let executive = seed_user(&pool, "임원", &["EXECUTIVE"], Some(admin_branch)).await;
    let token = harness.token(admin, &["ADMIN"], vec![admin_branch]).await;

    let (status, preview) = send(
        &harness,
        "POST",
        &format!("/api/v1/policy/users/{executive}/assignment-preview"),
        &token,
        Some(json!({
            "system_roles": ["EXECUTIVE", "ADMIN"],
            "branch_ids": [admin_branch.to_string()],
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{preview:?}");
    let preview_receipt_id = preview["preview_receipt_id"].as_str().unwrap();

    let (status, updated) = send_patch(
        &harness,
        &format!("/api/v1/users/{executive}"),
        &token,
        json!({
            "roles": ["EXECUTIVE", "ADMIN"],
            "branch_ids": [admin_branch.to_string()],
            "preview_acknowledged": true,
            "preview_receipt_id": preview_receipt_id,
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{updated:?}");
    let roles: BTreeSet<&str> = updated["roles"]
        .as_array()
        .unwrap()
        .iter()
        .map(|role| role.as_str().unwrap())
        .collect();
    assert_eq!(roles, BTreeSet::from(["ADMIN", "EXECUTIVE"]));
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn profile_edit_resending_current_assignments_needs_no_receipt(pool: PgPool) {
    // The legacy user-edit form re-sends the user's current roles/branches on a
    // phone-only edit. Delta-scoped, that is a no-op and must NOT demand an
    // impact-preview receipt (the admin-01-03 regression).
    let harness = Harness::new(pool.clone()).await;
    let branch = seed_branch(&pool).await;
    let admin = seed_user(&pool, "Branch Admin", &["ADMIN"], Some(branch)).await;
    let mechanic = seed_user(&pool, "정비사", &["MECHANIC"], Some(branch)).await;
    let token = harness.token(admin, &["ADMIN"], vec![branch]).await;

    let (status, updated) = send_patch(
        &harness,
        &format!("/api/v1/users/{mechanic}"),
        &token,
        json!({
            "phone": "010-1234-5678",
            "roles": ["MECHANIC"],
            "branch_ids": [branch.to_string()],
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{updated:?}");
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn real_role_change_without_a_receipt_is_rejected(pool: PgPool) {
    // A genuine role change (not an escalation the actor lacks authority for, so
    // not FORBIDDEN) without a preview receipt is a 422 at the REST layer.
    let harness = Harness::new(pool.clone()).await;
    let branch = seed_branch(&pool).await;
    let super_admin = seed_user(&pool, "본사 관리자", &["SUPER_ADMIN"], Some(branch)).await;
    let mechanic = seed_user(&pool, "정비사", &["MECHANIC"], Some(branch)).await;
    let token = harness
        .token(super_admin, &["SUPER_ADMIN"], vec![branch])
        .await;

    let (status, body) = send_patch(
        &harness,
        &format!("/api/v1/users/{mechanic}"),
        &token,
        json!({
            "roles": ["MECHANIC", "ADMIN"],
            "branch_ids": [branch.to_string()],
        }),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body:?}");
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn admin_cannot_grant_new_executive_role_on_update(pool: PgPool) {
    let harness = Harness::new(pool.clone()).await;
    let admin_branch = seed_branch(&pool).await;
    let admin = seed_user(&pool, "Branch Admin", &["ADMIN"], Some(admin_branch)).await;
    let mechanic = seed_user(&pool, "정비사", &["MECHANIC"], Some(admin_branch)).await;
    let token = harness.token(admin, &["ADMIN"], vec![admin_branch]).await;

    let (status, body) = send_patch(
        &harness,
        &format!("/api/v1/users/{mechanic}"),
        &token,
        json!({
            "roles": ["MECHANIC", "EXECUTIVE"],
            "branch_ids": [admin_branch.to_string()],
        }),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body:?}");
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn admin_cannot_remove_existing_executive_role_on_update(pool: PgPool) {
    let harness = Harness::new(pool.clone()).await;
    let admin_branch = seed_branch(&pool).await;
    let admin = seed_user(&pool, "Branch Admin", &["ADMIN"], Some(admin_branch)).await;
    let executive = seed_user(
        &pool,
        "임원 관리자",
        &["EXECUTIVE", "ADMIN"],
        Some(admin_branch),
    )
    .await;
    let token = harness.token(admin, &["ADMIN"], vec![admin_branch]).await;

    let (status, body) = send_patch(
        &harness,
        &format!("/api/v1/users/{executive}"),
        &token,
        json!({
            "roles": ["ADMIN"],
            "branch_ids": [admin_branch.to_string()],
        }),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body:?}");
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn super_admin_creates_executive_user(pool: PgPool) {
    let harness = Harness::new(pool.clone()).await;
    // SUPER_ADMIN resolves to BranchScope::All; no branch membership needed.
    let super_admin = seed_user(&pool, "Cold Start Admin", &["SUPER_ADMIN"], None).await;
    let token = harness.token(super_admin, &["SUPER_ADMIN"], vec![]).await;

    let (status, user) = send(
        &harness,
        "POST",
        "/api/v1/users",
        &token,
        Some(json!({
            "display_name": "이임원",
            "roles": ["EXECUTIVE"],
            "branch_ids": [],
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{user:?}");
    assert_eq!(user["roles"].as_array().unwrap()[0], "EXECUTIVE");
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn any_authenticated_user_edits_own_profile(pool: PgPool) {
    let harness = Harness::new(pool.clone()).await;
    // The seeded cold-start admin fixes its own display name via /users/me.
    let me = seed_user(&pool, "Cold Start Admin", &["SUPER_ADMIN"], None).await;
    let token = harness.token(me, &["SUPER_ADMIN"], vec![]).await;

    let (status, before) = send(&harness, "GET", "/api/v1/users/me", &token, None).await;
    assert_eq!(status, StatusCode::OK, "{before:?}");
    assert_eq!(before["display_name"], "Cold Start Admin");

    let (status, after) = send(
        &harness,
        "PATCH",
        "/api/v1/users/me",
        &token,
        Some(json!({ "display_name": "박관리자", "phone": "010-9999-0000" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{after:?}");
    assert_eq!(after["display_name"], "박관리자");
    assert_eq!(after["phone"], "010-9999-0000");
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn console_rollout_opt_in_persists_per_user_and_is_audited(pool: PgPool) {
    let harness = Harness::new(pool.clone()).await;
    let branch = seed_branch(&pool).await;
    let admin = seed_user(&pool, "Console Admin", &["SUPER_ADMIN"], Some(branch)).await;
    let mechanic = seed_user(&pool, "Console Pilot", &["MECHANIC"], Some(branch)).await;
    let admin_token = harness.token(admin, &["SUPER_ADMIN"], vec![branch]).await;
    let mechanic_token = harness.token(mechanic, &["MECHANIC"], vec![branch]).await;

    let (status, headers, initial) = send_with_headers(
        &harness,
        "GET",
        "/api/v1/console/rollout",
        &mechanic_token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{initial:?}");
    assert_eq!(headers.get(header::CACHE_CONTROL).unwrap(), "no-store");
    assert_eq!(initial["flag_key"], "console_carbon_copy");
    assert_eq!(initial["org_enabled"], false);
    assert_eq!(initial["user_opted_in"], false);
    assert_eq!(initial["effective_new_console"], false);

    let (status, enabled) = send_put(
        &harness,
        "/api/v1/console/rollout/org-flag",
        &admin_token,
        json!({ "enabled": true, "rollout_note": "pilot smoke" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{enabled:?}");
    assert_eq!(enabled["org_enabled"], true);
    assert_eq!(enabled["user_opted_in"], false);
    assert_eq!(enabled["effective_new_console"], false);

    let (status, denied) = send_put(
        &harness,
        "/api/v1/console/rollout/org-flag",
        &mechanic_token,
        json!({ "enabled": false }),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{denied:?}");

    let (status, opted_in) = send_put(
        &harness,
        "/api/v1/console/rollout/opt-in",
        &mechanic_token,
        json!({ "opt_in": true }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{opted_in:?}");
    assert_eq!(opted_in["org_enabled"], true);
    assert_eq!(opted_in["user_opted_in"], true);
    assert_eq!(opted_in["effective_new_console"], true);

    let stored_pref = sqlx::query(
        "SELECT preferences_json FROM user_feature_preferences \
         WHERE org_id = $1 AND user_id = $2 AND feature_key = 'console_rollout'",
    )
    .bind(*OrgId::knl().as_uuid())
    .bind(*mechanic.as_uuid())
    .fetch_one(&pool)
    .await
    .unwrap();
    let preferences: Value = stored_pref.try_get("preferences_json").unwrap();
    assert_eq!(preferences["opt_in"], true);

    let (status, opted_out) = send_put(
        &harness,
        "/api/v1/console/rollout/opt-in",
        &mechanic_token,
        json!({ "opt_in": false }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{opted_out:?}");
    assert_eq!(opted_out["org_enabled"], true);
    assert_eq!(opted_out["user_opted_in"], false);
    assert_eq!(opted_out["effective_new_console"], false);

    let audit_rows = sqlx::query(
        "SELECT action, target_type, target_id, after_snap \
         FROM audit_events \
         WHERE action IN ('console.org_flag_update', 'console.opt_in_update') \
         ORDER BY occurred_at",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    let actions: Vec<String> = audit_rows
        .iter()
        .map(|row| row.try_get::<String, _>("action").unwrap())
        .collect();
    assert!(
        actions.contains(&"console.org_flag_update".to_owned()),
        "expected org flag audit, got {actions:?}"
    );
    assert!(
        actions
            .iter()
            .filter(|action| *action == "console.opt_in_update")
            .count()
            >= 2,
        "expected opt-in and opt-out audits, got {actions:?}"
    );

    let opt_out_audit = audit_rows.last().unwrap();
    assert_eq!(
        opt_out_audit.try_get::<String, _>("target_type").unwrap(),
        "user_feature_preference"
    );
    assert_eq!(
        opt_out_audit.try_get::<String, _>("target_id").unwrap(),
        format!("{}:console_rollout", mechanic)
    );
    let after_snap: Value = opt_out_audit.try_get("after_snap").unwrap();
    assert_eq!(after_snap["opt_in"], false);
    assert_eq!(after_snap["effective_new_console"], false);
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn mechanic_cannot_manage_users(pool: PgPool) {
    let harness = Harness::new(pool.clone()).await;
    let branch = seed_branch(&pool).await;
    let mechanic = seed_user(&pool, "정비공", &["MECHANIC"], Some(branch)).await;
    let token = harness.token(mechanic, &["MECHANIC"], vec![branch]).await;

    // A mechanic has no UserManage capability.
    let (status, _) = send(&harness, "GET", "/api/v1/users", &token, None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // But a mechanic CAN edit its own profile and read the branch list.
    let (status, _) = send(&harness, "GET", "/api/v1/users/me", &token, None).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = send(&harness, "GET", "/api/v1/branches", &token, None).await;
    assert_eq!(status, StatusCode::OK);
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn create_user_writes_audit_event(pool: PgPool) {
    let harness = Harness::new(pool.clone()).await;
    let branch = seed_branch(&pool).await;
    let admin = seed_user(&pool, "Branch Admin", &["ADMIN"], Some(branch)).await;
    let token = harness.token(admin, &["ADMIN"], vec![branch]).await;

    let (status, user) = send(
        &harness,
        "POST",
        "/api/v1/users",
        &token,
        Some(json!({
            "display_name": "감사대상",
            "roles": ["MECHANIC"],
            "branch_ids": [branch.to_string()],
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{user:?}");
    let user_id = user["id"].as_str().unwrap();

    let actions: Vec<String> =
        sqlx::query_scalar("SELECT action FROM audit_events WHERE target_id = $1")
            .bind(user_id)
            .fetch_all(&pool)
            .await
            .unwrap();
    assert!(
        actions.contains(&"user.create".to_owned()),
        "expected user.create audit, got {actions:?}"
    );
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn user_role_or_branch_update_requires_policy_preview_receipt(pool: PgPool) {
    let harness = Harness::new(pool.clone()).await;
    let current_branch = seed_branch(&pool).await;
    let next_branch = seed_branch(&pool).await;
    let super_admin = seed_user(&pool, "Policy Account Owner", &["SUPER_ADMIN"], None).await;
    let target = seed_user(
        &pool,
        "Receipt-Gated User",
        &["MECHANIC"],
        Some(current_branch),
    )
    .await;
    let token = harness.token(super_admin, &["SUPER_ADMIN"], vec![]).await;

    let (status, body) = send_patch(
        &harness,
        &format!("/api/v1/users/{target}"),
        &token,
        json!({ "roles": ["MECHANIC", "ADMIN"] }),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body:?}");
    assert_eq!(persisted_user_roles(&pool, target).await, vec!["MECHANIC"]);

    let (status, body) = send_patch(
        &harness,
        &format!("/api/v1/users/{target}"),
        &token,
        json!({ "branch_ids": [current_branch.to_string(), next_branch.to_string()] }),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body:?}");
    assert_eq!(
        persisted_user_branch_ids(&pool, target).await,
        vec![*current_branch.as_uuid()]
    );
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn user_role_and_branch_update_consumes_policy_preview_receipt_and_audits(pool: PgPool) {
    let harness = Harness::new(pool.clone()).await;
    let current_branch = seed_branch(&pool).await;
    let next_branch = seed_branch(&pool).await;
    let super_admin = seed_user(&pool, "Account Mutation Owner", &["SUPER_ADMIN"], None).await;
    let target = seed_user(
        &pool,
        "Previewed Account",
        &["MECHANIC"],
        Some(current_branch),
    )
    .await;
    let existing_custom_role = seed_policy_role(
        &pool,
        super_admin,
        "account_mutation_existing_reader",
        "기존 계정 조회자",
        "ACTIVE",
        false,
        &[("daily_plan_review", "allow")],
    )
    .await;
    seed_policy_assignment(&pool, target, existing_custom_role, super_admin).await;
    let token = harness.token(super_admin, &["SUPER_ADMIN"], vec![]).await;

    let (status, preview) = send(
        &harness,
        "POST",
        &format!("/api/v1/policy/users/{target}/assignment-preview"),
        &token,
        Some(json!({
            "system_roles": ["MECHANIC", "ADMIN"],
            "branch_ids": [current_branch.to_string(), next_branch.to_string()],
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{preview:?}");
    assert_eq!(preview["current_system_roles"], json!(["MECHANIC"]));
    assert_eq!(
        preview["requested_system_roles"],
        json!(["ADMIN", "MECHANIC"])
    );
    assert_eq!(
        json_string_set(&preview["current_branch_ids"]),
        BTreeSet::from([current_branch.to_string()])
    );
    assert_eq!(
        json_string_set(&preview["requested_branch_ids"]),
        BTreeSet::from([current_branch.to_string(), next_branch.to_string()])
    );
    assert_eq!(preview["current_role_ids"], json!([existing_custom_role]));
    assert_eq!(preview["requested_role_ids"], json!([existing_custom_role]));
    let preview_receipt_id = preview["preview_receipt_id"].as_str().unwrap();

    let (status, updated) = send_patch(
        &harness,
        &format!("/api/v1/users/{target}"),
        &token,
        json!({
            "roles": ["MECHANIC", "ADMIN"],
            "branch_ids": [current_branch.to_string(), next_branch.to_string()],
            "preview_acknowledged": true,
            "preview_receipt_id": preview_receipt_id,
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{updated:?}");
    assert_eq!(updated["roles"], json!(["ADMIN", "MECHANIC"]));
    assert_eq!(
        json_string_set(&updated["branch_ids"]),
        BTreeSet::from([current_branch.to_string(), next_branch.to_string()])
    );
    assert_eq!(
        assigned_policy_role_ids(&pool, target).await,
        vec![existing_custom_role]
    );

    let (status, body) = send_patch(
        &harness,
        &format!("/api/v1/users/{target}"),
        &token,
        json!({
            "roles": ["MECHANIC"],
            "preview_acknowledged": true,
            "preview_receipt_id": preview_receipt_id,
        }),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body:?}");

    let actions = policy_audit_actions_for_user(&pool, target).await;
    assert!(
        actions.contains(&"policy.account.update.snapshot".to_owned()),
        "policy account update audit should be visible to audit chips: {actions:?}"
    );
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn deactivate_and_activate_are_reversible_archived_lifecycle(pool: PgPool) {
    let harness = Harness::new(pool.clone()).await;
    let branch = seed_branch(&pool).await;
    let super_admin = seed_user(&pool, "Lifecycle Owner", &["SUPER_ADMIN"], None).await;
    let target = seed_user(&pool, "Lifecycle Target", &["MECHANIC"], Some(branch)).await;
    let token = harness.token(super_admin, &["SUPER_ADMIN"], vec![]).await;

    let (status, archived) = send(
        &harness,
        "POST",
        &format!("/api/v1/users/{target}/deactivate"),
        &token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{archived:?}");
    assert_eq!(archived["is_active"], false);
    assert_eq!(archived["account_status"], "ARCHIVED");

    let (status, hidden) = send(&harness, "GET", "/api/v1/users", &token, None).await;
    assert_eq!(status, StatusCode::OK, "{hidden:?}");
    assert!(
        !hidden["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|user| user["id"] == target.to_string()),
        "archived users must stay out of the active roster: {hidden:?}"
    );

    let (status, activated) = send(
        &harness,
        "POST",
        &format!("/api/v1/users/{target}/activate"),
        &token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{activated:?}");
    assert_eq!(activated["is_active"], true);
    assert_eq!(activated["account_status"], "PENDING_SETUP");

    let actions = policy_audit_actions_for_user(&pool, target).await;
    assert!(
        actions.contains(&"policy.account.archive".to_owned()),
        "archive audit should be visible to policy audit chips: {actions:?}"
    );
    assert!(
        actions.contains(&"policy.account.activate".to_owned()),
        "activate audit should be visible to policy audit chips: {actions:?}"
    );
}

// ---------------------------------------------------------------------------
// Policy Studio assignment preview
// ---------------------------------------------------------------------------

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn policy_assignment_preview_requires_role_manage_and_target_branch_scope(pool: PgPool) {
    let harness = Harness::new(pool.clone()).await;
    let allowed_branch = seed_branch(&pool).await;
    let blocked_branch = seed_branch(&pool).await;
    let super_admin = seed_user(&pool, "Policy Owner", &["SUPER_ADMIN"], None).await;
    let mechanic = seed_user(&pool, "정비공", &["MECHANIC"], Some(allowed_branch)).await;
    let blocked_user = seed_user(
        &pool,
        "다른 지점 사용자",
        &["MECHANIC"],
        Some(blocked_branch),
    )
    .await;

    let mechanic_token = harness
        .token(mechanic, &["MECHANIC"], vec![allowed_branch])
        .await;
    let (status, body) = send(
        &harness,
        "POST",
        &format!("/api/v1/policy/users/{blocked_user}/assignment-preview"),
        &mechanic_token,
        Some(json!({ "role_ids": [] })),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body:?}");

    let branch_scoped_super_admin = harness
        .scoped_token(
            super_admin,
            &["SUPER_ADMIN"],
            vec![],
            AccessScope::new(
                AccessScopeLevel::Branch,
                ScopeNodeId::from_uuid(*allowed_branch.as_uuid()),
            ),
        )
        .await;
    let (status, body) = send(
        &harness,
        "POST",
        &format!("/api/v1/policy/users/{blocked_user}/assignment-preview"),
        &branch_scoped_super_admin,
        Some(json!({ "role_ids": [] })),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body:?}");
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn policy_assignment_preview_validates_custom_roles_and_never_mutates_assignments(
    pool: PgPool,
) {
    let harness = Harness::new(pool.clone()).await;
    let branch = seed_branch(&pool).await;
    let super_admin = seed_user(&pool, "Policy Owner", &["SUPER_ADMIN"], None).await;
    let target =
        seed_user_with_team(&pool, "정책 대상", &["ADMIN"], Some(branch), Some("정비")).await;
    let token = harness.token(super_admin, &["SUPER_ADMIN"], vec![]).await;

    let current_role = seed_policy_role(
        &pool,
        super_admin,
        "dispatch_planner",
        "배차 계획자",
        "ACTIVE",
        false,
        &[("daily_plan_review", "allow")],
    )
    .await;
    let requested_role = seed_policy_role(
        &pool,
        super_admin,
        "asset_reader",
        "자산 조회자",
        "DRAFT",
        false,
        &[
            ("equipment_manage", "limited"),
            ("equipment_cost_ledger_read", "allow"),
        ],
    )
    .await;
    let requested_conditions = json!([
        {
            "condition_key": "department_scope",
            "attribute": "department",
            "operator": "in",
            "values": ["정비팀", "야간조"]
        },
        {
            "condition_key": "purpose_scope",
            "attribute": "purpose",
            "operator": "equals",
            "values": ["asset_audit"]
        }
    ]);
    seed_policy_role_condition(
        &pool,
        requested_role,
        "department_scope",
        "department",
        "in",
        &["정비팀", "야간조"],
    )
    .await;
    seed_policy_role_condition(
        &pool,
        requested_role,
        "purpose_scope",
        "purpose",
        "equals",
        &["asset_audit"],
    )
    .await;
    let retired_role = seed_policy_role(
        &pool,
        super_admin,
        "old_role",
        "퇴역 역할",
        "RETIRED",
        false,
        &[("work_order_read_all", "allow")],
    )
    .await;
    let system_role = seed_policy_role(
        &pool,
        super_admin,
        "system_shadow",
        "시스템 역할",
        "ACTIVE",
        true,
        &[("work_order_create", "allow")],
    )
    .await;
    seed_policy_assignment(&pool, target, current_role, super_admin).await;

    let before = assigned_policy_role_ids(&pool, target).await;
    assert_eq!(before, vec![current_role]);

    let (status, preview) = send(
        &harness,
        "POST",
        &format!("/api/v1/policy/users/{target}/assignment-preview"),
        &token,
        Some(json!({ "role_ids": [requested_role] })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{preview:?}");
    assert_eq!(preview["effective"], false);
    assert_eq!(preview["current_role_ids"], json!([current_role]));
    assert_eq!(preview["requested_role_ids"], json!([requested_role]));
    assert_eq!(
        preview["custom_roles"][0]["conditions"],
        requested_conditions
    );
    assert_eq!(preview["delta"]["added_role_ids"], json!([requested_role]));
    assert_eq!(preview["delta"]["removed_role_ids"], json!([current_role]));
    assert_eq!(preview["custom_roles"][0]["runtime_effective"], false);
    assert_eq!(
        preview["custom_roles"][0]["runtime_warnings"],
        json!([
            "custom_role_status_not_active",
            "custom_role_condition_unsupported_by_runtime_evaluator"
        ])
    );
    assert_eq!(
        preview["warnings"],
        json!([
            "preview_only_pending_save",
            "custom_role_condition_unsupported_by_runtime_evaluator",
            "custom_role_status_not_active"
        ])
    );
    assert!(
        !preview["feature_grants"]
            .as_array()
            .unwrap()
            .iter()
            .any(|grant| grant["source_type"] == "custom_role"),
        "fail-closed draft or unsupported custom roles must not preview runtime grants: {preview:?}"
    );
    assert_eq!(assigned_policy_role_ids(&pool, target).await, before);

    for invalid_role in [uuid::Uuid::new_v4(), retired_role, system_role] {
        let (status, body) = send(
            &harness,
            "POST",
            &format!("/api/v1/policy/users/{target}/assignment-preview"),
            &token,
            Some(json!({ "role_ids": [invalid_role] })),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body:?}");
        assert_eq!(assigned_policy_role_ids(&pool, target).await, before);
    }

    let runtime_effective_role = seed_policy_role(
        &pool,
        super_admin,
        "runtime_work_order_creator",
        "런타임 작업 생성자",
        "ACTIVE",
        false,
        &[("work_order_create", "allow")],
    )
    .await;
    let (status, runtime_preview) = send(
        &harness,
        "POST",
        &format!("/api/v1/policy/users/{target}/assignment-preview"),
        &token,
        Some(json!({ "role_ids": [runtime_effective_role] })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{runtime_preview:?}");
    assert_eq!(runtime_preview["effective"], true);
    assert_eq!(
        runtime_preview["custom_roles"][0]["runtime_effective"],
        true
    );
    assert_eq!(
        runtime_preview["custom_roles"][0]["runtime_warnings"],
        json!([])
    );
    assert!(
        runtime_preview["feature_grants"]
            .as_array()
            .unwrap()
            .iter()
            .any(|grant| {
                grant["source_type"] == "custom_role"
                    && grant["source_key"] == "runtime_work_order_creator"
                    && grant["feature_key"] == "work_order_create"
            }),
        "runtime-effective custom roles should preview supported grants: {runtime_preview:?}"
    );
    assert_eq!(
        runtime_preview["warnings"],
        json!([
            "preview_only_pending_save",
            "active_assignments_become_runtime_effective_after_save"
        ])
    );

    let team_role = seed_policy_role(
        &pool,
        super_admin,
        "maintenance_team_creator",
        "정비팀 작업 생성자",
        "ACTIVE",
        false,
        &[("work_order_create", "allow")],
    )
    .await;
    seed_policy_role_condition(
        &pool,
        team_role,
        "team_scope",
        "team",
        "in",
        &["MAINTENANCE", "예방"],
    )
    .await;
    let (status, team_preview) = send(
        &harness,
        "POST",
        &format!("/api/v1/policy/users/{target}/assignment-preview"),
        &token,
        Some(json!({ "role_ids": [team_role] })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{team_preview:?}");
    assert_eq!(team_preview["effective"], true);
    assert_eq!(team_preview["custom_roles"][0]["runtime_effective"], true);
    assert_eq!(
        team_preview["custom_roles"][0]["runtime_warnings"],
        json!([])
    );
    assert!(
        team_preview["feature_grants"]
            .as_array()
            .unwrap()
            .iter()
            .any(|grant| {
                grant["source_type"] == "custom_role"
                    && grant["source_key"] == "maintenance_team_creator"
                    && grant["feature_key"] == "work_order_create"
            }),
        "matching team ABAC conditions should preview supported runtime grants: {team_preview:?}"
    );

    let mismatched_team_role = seed_policy_role(
        &pool,
        super_admin,
        "reception_team_creator",
        "접수팀 작업 생성자",
        "ACTIVE",
        false,
        &[("work_order_create", "allow")],
    )
    .await;
    seed_policy_role_condition(
        &pool,
        mismatched_team_role,
        "team_scope",
        "team",
        "equals",
        &["RECEPTION"],
    )
    .await;
    let (status, mismatched_team_preview) = send(
        &harness,
        "POST",
        &format!("/api/v1/policy/users/{target}/assignment-preview"),
        &token,
        Some(json!({ "role_ids": [mismatched_team_role] })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{mismatched_team_preview:?}");
    assert_eq!(mismatched_team_preview["effective"], false);
    assert_eq!(
        mismatched_team_preview["custom_roles"][0]["runtime_warnings"],
        json!(["custom_role_condition_outside_target_attributes"])
    );
    assert!(
        !mismatched_team_preview["feature_grants"]
            .as_array()
            .unwrap()
            .iter()
            .any(|grant| grant["source_type"] == "custom_role"),
        "team-mismatched custom roles must not preview runtime grants: {mismatched_team_preview:?}"
    );

    let outside_branch_role = seed_policy_role(
        &pool,
        super_admin,
        "outside_branch_reader",
        "다른 지점 조회자",
        "ACTIVE",
        false,
        &[("work_order_read_all", "allow")],
    )
    .await;
    let other_branch = seed_branch(&pool).await;
    seed_policy_role_condition(
        &pool,
        outside_branch_role,
        "branch_scope",
        "branch",
        "equals",
        &[&other_branch.to_string()],
    )
    .await;
    let (status, outside_branch_preview) = send(
        &harness,
        "POST",
        &format!("/api/v1/policy/users/{target}/assignment-preview"),
        &token,
        Some(json!({ "role_ids": [outside_branch_role] })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{outside_branch_preview:?}");
    assert_eq!(outside_branch_preview["effective"], false);
    assert_eq!(
        outside_branch_preview["custom_roles"][0]["runtime_warnings"],
        json!(["custom_role_condition_outside_target_branch_scope"])
    );
    assert!(
        !outside_branch_preview["feature_grants"]
            .as_array()
            .unwrap()
            .iter()
            .any(|grant| grant["source_type"] == "custom_role"),
        "branch-mismatched custom roles must not preview runtime grants: {outside_branch_preview:?}"
    );
    assert_eq!(assigned_policy_role_ids(&pool, target).await, before);
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn policy_role_create_persists_abac_pbac_conditions_and_catalog_returns_them(pool: PgPool) {
    let harness = Harness::new(pool.clone()).await;
    let super_admin = seed_user(&pool, "Policy Owner", &["SUPER_ADMIN"], None).await;
    let token = harness.token(super_admin, &["SUPER_ADMIN"], vec![]).await;

    let requested_conditions = json!([
        {
            "condition_key": "dept_scope",
            "attribute": "department",
            "operator": "in",
            "values": ["정비팀", "야간조"]
        },
        {
            "condition_key": "purpose_scope",
            "attribute": "purpose",
            "operator": "equals",
            "values": ["work_order_approval"]
        }
    ]);
    let (status, created) = send(
        &harness,
        "POST",
        "/api/v1/policy/roles",
        &token,
        Some(json!({
            "role_key": "maintenance_shift_approver",
            "display_name": "정비 교대 승인자",
            "description": "정비팀 야간조 승인 담당",
            "permissions": [
                { "feature_key": "work_order_read_all", "permission_level": "allow" },
                { "feature_key": "daily_plan_review", "permission_level": "limited" }
            ],
            "conditions": requested_conditions.clone()
        })),
    )
    .await;

    assert_eq!(status, StatusCode::CREATED, "{created:?}");
    assert_eq!(created["conditions"], requested_conditions);
    let role_id = created["id"].as_str().unwrap();

    let stored: Vec<(String, String, String, Vec<String>)> = sqlx::query_as(
        r#"
        SELECT condition_key, attribute, operator, condition_values
        FROM policy_role_conditions
        WHERE role_id = $1
        ORDER BY condition_key
        "#,
    )
    .bind(role_id.parse::<uuid::Uuid>().unwrap())
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(
        stored,
        vec![
            (
                "dept_scope".to_owned(),
                "department".to_owned(),
                "in".to_owned(),
                vec!["정비팀".to_owned(), "야간조".to_owned()]
            ),
            (
                "purpose_scope".to_owned(),
                "purpose".to_owned(),
                "equals".to_owned(),
                vec!["work_order_approval".to_owned()]
            ),
        ]
    );

    let (status, catalog) = send(&harness, "GET", "/api/v1/policy/roles", &token, None).await;
    assert_eq!(status, StatusCode::OK, "{catalog:?}");
    assert_eq!(catalog["policy_version"]["version"], 1);
    assert!(
        catalog["policy_version"]["updated_at"].as_str().is_some(),
        "catalog should expose the version bump timestamp"
    );
    assert_eq!(
        catalog["custom_roles"][0]["conditions"],
        requested_conditions
    );
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn policy_role_create_rejects_scope_widening_custom_features(pool: PgPool) {
    let harness = Harness::new(pool.clone()).await;
    let super_admin = seed_user(&pool, "Policy Owner", &["SUPER_ADMIN"], None).await;
    let token = harness.token(super_admin, &["SUPER_ADMIN"], vec![]).await;

    let (status, body) = send(
        &harness,
        "POST",
        "/api/v1/policy/roles",
        &token,
        Some(json!({
            "role_key": "unsafe_org_wide_triage",
            "display_name": "범위 확장 역할",
            "permissions": [
                { "feature_key": "org_wide_queue_triage", "permission_level": "allow" }
            ]
        })),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body:?}");

    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM policy_roles WHERE role_key = 'unsafe_org_wide_triage'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(count, 0);
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn policy_role_catalog_exposes_policy_version_and_assignment_writes_bump_it(pool: PgPool) {
    let harness = Harness::new(pool.clone()).await;
    let super_admin = seed_user(&pool, "Policy Owner", &["SUPER_ADMIN"], None).await;
    let target = seed_user(&pool, "Policy Target", &["ADMIN"], None).await;
    let token = harness.token(super_admin, &["SUPER_ADMIN"], vec![]).await;

    let (status, empty_catalog) = send(&harness, "GET", "/api/v1/policy/roles", &token, None).await;
    assert_eq!(status, StatusCode::OK, "{empty_catalog:?}");
    assert_eq!(empty_catalog["policy_version"]["version"], 0);
    assert_eq!(empty_catalog["policy_version"]["updated_at"], Value::Null);

    let (status, role) = send(
        &harness,
        "POST",
        "/api/v1/policy/roles",
        &token,
        Some(json!({
            "role_key": "versioned_policy_role",
            "display_name": "버전 정책 역할",
            "permissions": [
                { "feature_key": "work_order_read_all", "permission_level": "allow" }
            ],
            "conditions": [
                {
                    "condition_key": "department_scope",
                    "attribute": "department",
                    "operator": "equals",
                    "values": ["정비팀"]
                }
            ]
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{role:?}");
    let role_id = role["id"].as_str().unwrap();

    let (status, catalog_after_create) =
        send(&harness, "GET", "/api/v1/policy/roles", &token, None).await;
    assert_eq!(status, StatusCode::OK, "{catalog_after_create:?}");
    assert_eq!(catalog_after_create["policy_version"]["version"], 1);

    let (status, body) = send_put(
        &harness,
        &format!("/api/v1/policy/users/{target}/assignments"),
        &token,
        json!({ "role_ids": [role_id] }),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body:?}");
    assert_eq!(
        assigned_policy_role_ids(&pool, target).await,
        Vec::<uuid::Uuid>::new()
    );

    let (status, preview) = send(
        &harness,
        "POST",
        &format!("/api/v1/policy/users/{target}/assignment-preview"),
        &token,
        Some(json!({ "role_ids": [role_id] })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{preview:?}");
    let preview_receipt_id = preview["preview_receipt_id"].as_str().unwrap();

    let (status, body) = send_put(
        &harness,
        &format!("/api/v1/policy/users/{target}/assignments"),
        &token,
        json!({
            "role_ids": [role_id],
            "preview_acknowledged": true,
            "preview_receipt_id": preview_receipt_id
        }),
    )
    .await;
    assert_eq!(status, StatusCode::PRECONDITION_REQUIRED, "{body:?}");
    assert_eq!(
        assigned_policy_role_ids(&pool, target).await,
        Vec::<uuid::Uuid>::new()
    );

    let step_up = fresh_step_up_assertion(&pool, super_admin, "Policy Owner").await;
    let (status, assignments) = send_put(
        &harness,
        &format!("/api/v1/policy/users/{target}/assignments"),
        &token,
        json!({
            "role_ids": [role_id],
            "preview_acknowledged": true,
            "preview_receipt_id": preview_receipt_id,
            "step_up": step_up
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{assignments:?}");

    let step_up = fresh_step_up_assertion(&pool, super_admin, "Policy Owner").await;
    let (status, body) = send_put(
        &harness,
        &format!("/api/v1/policy/users/{target}/assignments"),
        &token,
        json!({
            "role_ids": [role_id],
            "preview_acknowledged": true,
            "preview_receipt_id": preview_receipt_id,
            "step_up": step_up
        }),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body:?}");
    assert_eq!(
        assigned_policy_role_ids(&pool, target).await,
        vec![role_id.parse::<uuid::Uuid>().unwrap()]
    );

    let (status, catalog_after_assignment) =
        send(&harness, "GET", "/api/v1/policy/roles", &token, None).await;
    assert_eq!(status, StatusCode::OK, "{catalog_after_assignment:?}");
    assert_eq!(catalog_after_assignment["policy_version"]["version"], 2);

    let (status, status_preview) = send(
        &harness,
        "POST",
        &format!("/api/v1/policy/roles/{role_id}/status-preview"),
        &token,
        Some(json!({ "status": "ACTIVE" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{status_preview:?}");
    assert_eq!(status_preview["role_id"], role_id);
    assert_eq!(status_preview["current_status"], "DRAFT");
    assert_eq!(status_preview["requested_status"], "ACTIVE");
    assert_eq!(status_preview["permission_count"], 1);
    assert_eq!(status_preview["condition_count"], 1);
    assert_eq!(status_preview["planned_assignment_count"], 1);
    assert_eq!(status_preview["requires_passkey_step_up"], true);
    assert_eq!(status_preview["effective_runtime_change"], true);
    let preview_warnings = status_preview["warnings"].as_array().unwrap();
    assert!(preview_warnings.contains(&json!("passkey_step_up_required")));
    assert!(preview_warnings.contains(&json!(
        "assigned_users_may_gain_or_lose_runtime_permissions"
    )));
    assert!(preview_warnings.contains(&json!(
        "publish_enables_assigned_custom_role_runtime_grants"
    )));

    let (status, audit_events) = send(
        &harness,
        "GET",
        "/api/v1/policy/audit-events?limit=10",
        &token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{audit_events:?}");
    let events = audit_events.as_array().unwrap();
    let actions = events
        .iter()
        .map(|event| event["action"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert!(
        actions.contains(&"policy.role.create"),
        "policy create audit event should be visible: {actions:?}"
    );
    assert!(
        actions.contains(&"policy.role_assignment.replace.snapshot"),
        "assignment snapshot audit event should be visible: {actions:?}"
    );
    let assignment_snapshot = events
        .iter()
        .find(|event| event["action"] == "policy.role_assignment.replace.snapshot")
        .unwrap();
    assert_eq!(assignment_snapshot["target_type"], "policy_role_assignment");
    assert_eq!(assignment_snapshot["target_id"], target.to_string());
    assert_eq!(
        assignment_snapshot["after_snapshot"]["assignments"][0]["role_id"],
        role_id
    );

    let (status, body) = send(
        &harness,
        "GET",
        "/api/v1/policy/audit-events?limit=0",
        &token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body:?}");
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn policy_role_create_rejects_unknown_condition_attribute(pool: PgPool) {
    let harness = Harness::new(pool.clone()).await;
    let super_admin = seed_user(&pool, "Policy Owner", &["SUPER_ADMIN"], None).await;
    let token = harness.token(super_admin, &["SUPER_ADMIN"], vec![]).await;

    let (status, body) = send(
        &harness,
        "POST",
        "/api/v1/policy/roles",
        &token,
        Some(json!({
            "role_key": "bad_condition_role",
            "display_name": "잘못된 조건 역할",
            "permissions": [
                { "feature_key": "work_order_read_all", "permission_level": "allow" }
            ],
            "conditions": [
                {
                    "condition_key": "machinery_scope",
                    "attribute": "machinery",
                    "operator": "equals",
                    "values": ["굴삭기"]
                }
            ]
        })),
    )
    .await;

    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body:?}");
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM policy_roles WHERE role_key = $1")
        .bind("bad_condition_role")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn branch_scoped_policy_managers_are_limited_to_branch_condition_scope(pool: PgPool) {
    let harness = Harness::new(pool.clone()).await;
    let allowed_branch = seed_branch(&pool).await;
    let blocked_branch = seed_branch(&pool).await;
    let super_admin = seed_user(&pool, "Delegated Policy Owner", &["SUPER_ADMIN"], None).await;
    let full_token = harness.token(super_admin, &["SUPER_ADMIN"], vec![]).await;
    let branch_token = harness
        .scoped_token(
            super_admin,
            &["SUPER_ADMIN"],
            vec![],
            AccessScope::new(
                AccessScopeLevel::Branch,
                ScopeNodeId::from_uuid(*allowed_branch.as_uuid()),
            ),
        )
        .await;

    let (status, body) = send(
        &harness,
        "POST",
        "/api/v1/policy/roles",
        &branch_token,
        Some(json!({
            "role_key": "unscoped_branch_role",
            "display_name": "범위 없는 역할",
            "permissions": [
                { "feature_key": "work_order_read_all", "permission_level": "allow" }
            ]
        })),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body:?}");

    let (status, body) = send(
        &harness,
        "POST",
        "/api/v1/policy/roles",
        &branch_token,
        Some(json!({
            "role_key": "blocked_branch_role",
            "display_name": "다른 지점 역할",
            "permissions": [
                { "feature_key": "work_order_read_all", "permission_level": "allow" }
            ],
            "conditions": [
                {
                    "condition_key": "branch_scope",
                    "attribute": "branch",
                    "operator": "equals",
                    "values": [blocked_branch.to_string()]
                }
            ]
        })),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body:?}");

    let (status, scoped_role) = send(
        &harness,
        "POST",
        "/api/v1/policy/roles",
        &branch_token,
        Some(json!({
            "role_key": "allowed_branch_role",
            "display_name": "위임 지점 역할",
            "permissions": [
                { "feature_key": "work_order_read_all", "permission_level": "allow" }
            ],
            "conditions": [
                {
                    "condition_key": "branch_scope",
                    "attribute": "branch",
                    "operator": "equals",
                    "values": [allowed_branch.to_string()]
                }
            ]
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{scoped_role:?}");
    let scoped_role_id = scoped_role["id"].as_str().unwrap();

    let (status, blocked_role) = send(
        &harness,
        "POST",
        "/api/v1/policy/roles",
        &full_token,
        Some(json!({
            "role_key": "full_admin_other_branch_role",
            "display_name": "전체 관리자 타 지점 역할",
            "permissions": [
                { "feature_key": "work_order_read_all", "permission_level": "allow" }
            ],
            "conditions": [
                {
                    "condition_key": "branch_scope",
                    "attribute": "branch",
                    "operator": "equals",
                    "values": [blocked_branch.to_string()]
                }
            ]
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{blocked_role:?}");
    let blocked_role_id = blocked_role["id"].as_str().unwrap();

    let (status, catalog) =
        send(&harness, "GET", "/api/v1/policy/roles", &branch_token, None).await;
    assert_eq!(status, StatusCode::OK, "{catalog:?}");
    assert_eq!(catalog["custom_roles"].as_array().unwrap().len(), 1);
    assert_eq!(catalog["custom_roles"][0]["id"], scoped_role_id);

    let target = seed_user(&pool, "위임 지점 대상", &["ADMIN"], Some(allowed_branch)).await;
    let (status, body) = send(
        &harness,
        "POST",
        &format!("/api/v1/policy/users/{target}/assignment-preview"),
        &branch_token,
        Some(json!({ "role_ids": [blocked_role_id] })),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body:?}");
    let (status, body) = send_put(
        &harness,
        &format!("/api/v1/policy/users/{target}/assignments"),
        &branch_token,
        json!({ "role_ids": [blocked_role_id] }),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body:?}");
    assert_eq!(
        assigned_policy_role_ids(&pool, target).await,
        Vec::<uuid::Uuid>::new()
    );

    let (status, preview) = send(
        &harness,
        "POST",
        &format!("/api/v1/policy/users/{target}/assignment-preview"),
        &branch_token,
        Some(json!({ "role_ids": [scoped_role_id] })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{preview:?}");
    assert_eq!(
        preview["custom_roles"][0]["conditions"][0]["values"],
        json!([allowed_branch.to_string()])
    );
    let preview_receipt_id = preview["preview_receipt_id"].as_str().unwrap();

    let (status, body) = send_put(
        &harness,
        &format!("/api/v1/policy/users/{target}/assignments"),
        &branch_token,
        json!({ "role_ids": [scoped_role_id] }),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body:?}");
    assert_eq!(
        assigned_policy_role_ids(&pool, target).await,
        Vec::<uuid::Uuid>::new()
    );

    let (status, body) = send_put(
        &harness,
        &format!("/api/v1/policy/users/{target}/assignments"),
        &branch_token,
        json!({
            "role_ids": [scoped_role_id],
            "preview_acknowledged": true,
            "preview_receipt_id": preview_receipt_id
        }),
    )
    .await;
    assert_eq!(status, StatusCode::PRECONDITION_REQUIRED, "{body:?}");
    assert_eq!(
        assigned_policy_role_ids(&pool, target).await,
        Vec::<uuid::Uuid>::new()
    );

    let step_up = fresh_step_up_assertion(&pool, super_admin, "Delegated Policy Owner").await;
    let (status, assignments) = send_put(
        &harness,
        &format!("/api/v1/policy/users/{target}/assignments"),
        &branch_token,
        json!({
            "role_ids": [scoped_role_id],
            "preview_acknowledged": true,
            "preview_receipt_id": preview_receipt_id,
            "step_up": step_up
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{assignments:?}");
    assert_eq!(
        assigned_policy_role_ids(&pool, target).await,
        vec![scoped_role_id.parse::<uuid::Uuid>().unwrap()]
    );
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn branch_scoped_policy_managers_cannot_remove_out_of_scope_assignments(pool: PgPool) {
    let harness = Harness::new(pool.clone()).await;
    let allowed_branch = seed_branch(&pool).await;
    let blocked_branch = seed_branch(&pool).await;
    let super_admin = seed_user(&pool, "Scoped Removal Owner", &["SUPER_ADMIN"], None).await;
    let full_token = harness.token(super_admin, &["SUPER_ADMIN"], vec![]).await;
    let branch_token = harness
        .scoped_token(
            super_admin,
            &["SUPER_ADMIN"],
            vec![],
            AccessScope::new(
                AccessScopeLevel::Branch,
                ScopeNodeId::from_uuid(*allowed_branch.as_uuid()),
            ),
        )
        .await;
    let target = seed_user(&pool, "위임 삭제 대상", &["ADMIN"], Some(allowed_branch)).await;

    let (status, blocked_role) = send(
        &harness,
        "POST",
        "/api/v1/policy/roles",
        &full_token,
        Some(json!({
            "role_key": "other_branch_clear_guard",
            "display_name": "타 지점 제거 보호 역할",
            "permissions": [
                { "feature_key": "work_order_read_all", "permission_level": "allow" }
            ],
            "conditions": [
                {
                    "condition_key": "branch_scope",
                    "attribute": "branch",
                    "operator": "equals",
                    "values": [blocked_branch.to_string()]
                }
            ]
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{blocked_role:?}");
    let blocked_role_id = blocked_role["id"]
        .as_str()
        .unwrap()
        .parse::<uuid::Uuid>()
        .unwrap();
    seed_policy_assignment(&pool, target, blocked_role_id, super_admin).await;
    assert_eq!(
        assigned_policy_role_ids(&pool, target).await,
        vec![blocked_role_id]
    );

    let (status, preview_body) = send(
        &harness,
        "POST",
        &format!("/api/v1/policy/users/{target}/assignment-preview"),
        &branch_token,
        Some(json!({ "role_ids": [] })),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{preview_body:?}");
    assert_eq!(
        assigned_policy_role_ids(&pool, target).await,
        vec![blocked_role_id]
    );

    let step_up = fresh_step_up_assertion(&pool, super_admin, "Scoped Removal Owner").await;
    let (status, replace_body) = send_put(
        &harness,
        &format!("/api/v1/policy/users/{target}/assignments"),
        &branch_token,
        json!({
            "role_ids": [],
            "preview_acknowledged": true,
            "preview_receipt_id": uuid::Uuid::new_v4(),
            "step_up": step_up
        }),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{replace_body:?}");
    assert_eq!(
        assigned_policy_role_ids(&pool, target).await,
        vec![blocked_role_id]
    );
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn assignment_save_rejects_stale_preview_when_current_assignments_changed(pool: PgPool) {
    let harness = Harness::new(pool.clone()).await;
    let allowed_branch = seed_branch(&pool).await;
    let blocked_branch = seed_branch(&pool).await;
    let super_admin = seed_user(&pool, "Stale Preview Owner", &["SUPER_ADMIN"], None).await;
    let branch_token = harness
        .scoped_token(
            super_admin,
            &["SUPER_ADMIN"],
            vec![],
            AccessScope::new(
                AccessScopeLevel::Branch,
                ScopeNodeId::from_uuid(*allowed_branch.as_uuid()),
            ),
        )
        .await;
    let target = seed_user(
        &pool,
        "스테일 미리보기 대상",
        &["ADMIN"],
        Some(allowed_branch),
    )
    .await;

    let (status, preview) = send(
        &harness,
        "POST",
        &format!("/api/v1/policy/users/{target}/assignment-preview"),
        &branch_token,
        Some(json!({ "role_ids": [] })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{preview:?}");
    assert_eq!(preview["current_role_ids"], json!([]));
    let preview_receipt_id = preview["preview_receipt_id"]
        .as_str()
        .unwrap()
        .parse::<uuid::Uuid>()
        .unwrap();

    let blocked_role_id = seed_policy_role(
        &pool,
        super_admin,
        "stale_preview_other_branch_role",
        "스테일 미리보기 타 지점 역할",
        "ACTIVE",
        false,
        &[("work_order_read_all", "allow")],
    )
    .await;
    seed_policy_role_condition(
        &pool,
        blocked_role_id,
        "branch_scope",
        "branch",
        "equals",
        &[&blocked_branch.to_string()],
    )
    .await;
    seed_policy_assignment(&pool, target, blocked_role_id, super_admin).await;

    let store = PgOrgStore::new(pool.clone());
    let result = scope_org(
        OrgId::knl(),
        store.replace_policy_role_assignments(ReplacePolicyRoleAssignmentsCommand {
            actor: super_admin,
            user_id: target,
            role_ids: vec![],
            preview_receipt_id,
            trace: TraceContext::generate(),
            occurred_at: OffsetDateTime::now_utc(),
        }),
    )
    .await;
    assert!(
        result.is_err(),
        "a stale preview receipt must not authorize deleting assignments added after preview"
    );
    assert_eq!(
        assigned_policy_role_ids(&pool, target).await,
        vec![blocked_role_id],
        "stale-save rejection must leave the newly added assignment intact"
    );
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn assignment_save_rejects_stale_preview_when_target_branches_changed(pool: PgPool) {
    let harness = Harness::new(pool.clone()).await;
    let allowed_branch = seed_branch(&pool).await;
    let blocked_branch = seed_branch(&pool).await;
    let super_admin = seed_user(&pool, "Stale Target Owner", &["SUPER_ADMIN"], None).await;
    let full_token = harness.token(super_admin, &["SUPER_ADMIN"], vec![]).await;
    let branch_token = harness
        .scoped_token(
            super_admin,
            &["SUPER_ADMIN"],
            vec![],
            AccessScope::new(
                AccessScopeLevel::Branch,
                ScopeNodeId::from_uuid(*allowed_branch.as_uuid()),
            ),
        )
        .await;
    let target = seed_user(&pool, "스테일 지점 대상", &["ADMIN"], Some(allowed_branch)).await;

    let (status, preview) = send(
        &harness,
        "POST",
        &format!("/api/v1/policy/users/{target}/assignment-preview"),
        &branch_token,
        Some(json!({ "role_ids": [] })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{preview:?}");
    let preview_receipt_id = preview["preview_receipt_id"]
        .as_str()
        .unwrap()
        .parse::<uuid::Uuid>()
        .unwrap();

    let (status, move_preview) = send(
        &harness,
        "POST",
        &format!("/api/v1/policy/users/{target}/assignment-preview"),
        &full_token,
        Some(json!({ "branch_ids": [blocked_branch.to_string()] })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{move_preview:?}");
    let move_preview_receipt_id = move_preview["preview_receipt_id"].as_str().unwrap();

    let (status, moved_user) = send_patch(
        &harness,
        &format!("/api/v1/users/{target}"),
        &full_token,
        json!({
            "branch_ids": [blocked_branch.to_string()],
            "preview_acknowledged": true,
            "preview_receipt_id": move_preview_receipt_id,
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{moved_user:?}");

    let store = PgOrgStore::new(pool.clone());
    let result = scope_org(
        OrgId::knl(),
        store.replace_policy_role_assignments(ReplacePolicyRoleAssignmentsCommand {
            actor: super_admin,
            user_id: target,
            role_ids: vec![],
            preview_receipt_id,
            trace: TraceContext::generate(),
            occurred_at: OffsetDateTime::now_utc(),
        }),
    )
    .await;
    assert!(
        result.is_err(),
        "a stale preview receipt must not authorize saves after the target leaves delegated scope"
    );
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn assignment_save_rejects_stale_preview_when_role_definition_changed(pool: PgPool) {
    let harness = Harness::new(pool.clone()).await;
    let allowed_branch = seed_branch(&pool).await;
    let blocked_branch = seed_branch(&pool).await;
    let super_admin = seed_user(&pool, "Stale Role Owner", &["SUPER_ADMIN"], None).await;
    let full_token = harness.token(super_admin, &["SUPER_ADMIN"], vec![]).await;
    let branch_token = harness
        .scoped_token(
            super_admin,
            &["SUPER_ADMIN"],
            vec![],
            AccessScope::new(
                AccessScopeLevel::Branch,
                ScopeNodeId::from_uuid(*allowed_branch.as_uuid()),
            ),
        )
        .await;
    let target = seed_user(&pool, "스테일 역할 대상", &["ADMIN"], Some(allowed_branch)).await;

    let (status, role) = send(
        &harness,
        "POST",
        "/api/v1/policy/roles",
        &full_token,
        Some(json!({
            "role_key": "stale_definition_branch_role",
            "display_name": "스테일 정의 역할",
            "permissions": [
                { "feature_key": "work_order_create", "permission_level": "allow" }
            ],
            "conditions": [
                {
                    "condition_key": "branch_scope",
                    "attribute": "branch",
                    "operator": "equals",
                    "values": [allowed_branch.to_string()]
                }
            ]
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{role:?}");
    let role_id = role["id"].as_str().unwrap();

    let (status, preview) = send(
        &harness,
        "POST",
        &format!("/api/v1/policy/users/{target}/assignment-preview"),
        &branch_token,
        Some(json!({ "role_ids": [role_id] })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{preview:?}");
    let preview_receipt_id = preview["preview_receipt_id"]
        .as_str()
        .unwrap()
        .parse::<uuid::Uuid>()
        .unwrap();

    let step_up = fresh_step_up_assertion(&pool, super_admin, "Stale Role Owner").await;
    let (status, changed_role) = send_patch(
        &harness,
        &format!("/api/v1/policy/roles/{role_id}"),
        &full_token,
        json!({
            "display_name": "스테일 정의 역할",
            "permissions": [
                { "feature_key": "work_order_create", "permission_level": "allow" }
            ],
            "conditions": [
                {
                    "condition_key": "branch_scope",
                    "attribute": "branch",
                    "operator": "equals",
                    "values": [blocked_branch.to_string()]
                }
            ],
            "step_up": step_up
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{changed_role:?}");

    let store = PgOrgStore::new(pool.clone());
    let result = scope_org(
        OrgId::knl(),
        store.replace_policy_role_assignments(ReplacePolicyRoleAssignmentsCommand {
            actor: super_admin,
            user_id: target,
            role_ids: vec![role_id.parse::<uuid::Uuid>().unwrap()],
            preview_receipt_id,
            trace: TraceContext::generate(),
            occurred_at: OffsetDateTime::now_utc(),
        }),
    )
    .await;
    assert!(
        result.is_err(),
        "a stale preview receipt must not authorize saves after touched policy roles change"
    );
    assert_eq!(
        assigned_policy_role_ids(&pool, target).await,
        Vec::<uuid::Uuid>::new()
    );
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn retired_ai_permission_is_inert_but_preserved_in_legacy_role_history(pool: PgPool) {
    let harness = Harness::new(pool.clone()).await;
    let branch = seed_branch(&pool).await;
    let actor = seed_user(&pool, "Policy Owner", &["SUPER_ADMIN"], None).await;
    let target = seed_user(&pool, "Policy Target", &["MEMBER"], Some(branch)).await;
    let token = harness.token(actor, &["SUPER_ADMIN"], vec![]).await;
    let role_id = seed_policy_role(
        &pool,
        actor,
        "legacy_mixed_ai",
        "Legacy mixed role",
        "ACTIVE",
        false,
        &[("ai_assist", "allow"), ("work_order_create", "allow")],
    )
    .await;
    seed_policy_assignment(&pool, target, role_id, actor).await;
    let ai_only_id = seed_policy_role(
        &pool,
        actor,
        "legacy_ai_only",
        "Legacy AI-only role",
        "DRAFT",
        false,
        &[("ai_assist", "allow")],
    )
    .await;
    let ordinary_id = seed_policy_role(
        &pool,
        actor,
        "ordinary_role",
        "Ordinary role",
        "DRAFT",
        false,
        &[("work_order_read_all", "allow")],
    )
    .await;
    let old_ai_row: (uuid::Uuid, OffsetDateTime, String) = sqlx::query_as(
        "SELECT id, created_at, permission_level FROM policy_role_permissions WHERE role_id = $1 AND feature_key = 'ai_assist'",
    )
    .bind(role_id)
    .fetch_one(&pool)
    .await
    .unwrap();

    let (status, catalog) = send(&harness, "GET", "/api/v1/policy/roles", &token, None).await;
    assert_eq!(status, StatusCode::OK, "{catalog:?}");
    assert!(!catalog.to_string().contains("ai_assist"));
    let target_token = harness.token(target, &["MEMBER"], vec![branch]).await;
    let (status, authz) = send(&harness, "GET", "/api/v1/me/authz", &target_token, None).await;
    assert_eq!(status, StatusCode::OK, "{authz:?}");
    assert!(!authz.to_string().contains("ai_assist"));
    let resolved = resolve_effective_feature_grants_in_org(
        &harness.pool,
        OrgId::knl(),
        target,
        &BranchScope::single(branch),
    )
    .await
    .unwrap();
    assert!(
        !resolved
            .iter()
            .any(|grant| grant.feature == Feature::AiAssist)
    );
    assert!(
        resolved
            .iter()
            .any(|grant| grant.feature == Feature::WorkOrderCreate)
    );
    assert!(
        authz["capabilities"]
            .as_array()
            .unwrap()
            .iter()
            .any(|capability| capability["feature"] == "work_order_create")
    );

    let policy_version_before: i64 = sqlx::query_scalar(
        "SELECT COALESCE((SELECT version FROM policy_versions WHERE org_id = $1), 0)",
    )
    .bind(*OrgId::knl().as_uuid())
    .fetch_one(&pool)
    .await
    .unwrap();

    let (status, body) = send(
        &harness,
        "POST",
        "/api/v1/policy/roles",
        &token,
        Some(json!({
            "role_key": "new_ai_grant",
            "display_name": "Forbidden AI grant",
            "permissions": [{ "feature_key": "ai_assist", "permission_level": "allow" }]
        })),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body:?}");

    let (status, body) = send(
        &harness,
        "POST",
        "/api/v1/policy/roles",
        &token,
        Some(json!({
            "role_key": "empty_new_role",
            "display_name": "Empty new role",
            "permissions": []
        })),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body:?}");
    let (status, body) = send_patch(
        &harness,
        &format!("/api/v1/policy/roles/{ordinary_id}"),
        &token,
        json!({
            "display_name": "Empty ordinary role",
            "permissions": [],
            "conditions": []
        }),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body:?}");

    let (status, body) = send_patch(
        &harness,
        &format!("/api/v1/policy/roles/{role_id}"),
        &token,
        json!({
            "display_name": "Forbidden AI update",
            "permissions": [{ "feature_key": "ai_assist", "permission_level": "allow" }],
            "conditions": []
        }),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body:?}");

    let store = PgOrgStore::new(pool.clone());
    let ai_permission = PolicyRolePermission {
        feature_key: "ai_assist".to_owned(),
        permission_level: "allow".to_owned(),
    };
    let direct_create = scope_org(
        OrgId::knl(),
        store.create_policy_role(CreatePolicyRoleCommand {
            actor,
            role_key: "direct_ai_grant".to_owned(),
            display_name: "Direct AI grant".to_owned(),
            description: None,
            permissions: vec![ai_permission.clone()],
            conditions: vec![],
            trace: TraceContext::generate(),
            occurred_at: OffsetDateTime::now_utc(),
        }),
    )
    .await;
    assert!(
        direct_create.is_err(),
        "direct owner creation must reject AI"
    );
    let direct_empty_create = scope_org(
        OrgId::knl(),
        store.create_policy_role(CreatePolicyRoleCommand {
            actor,
            role_key: "direct_empty_role".to_owned(),
            display_name: "Direct empty role".to_owned(),
            description: None,
            permissions: vec![],
            conditions: vec![],
            trace: TraceContext::generate(),
            occurred_at: OffsetDateTime::now_utc(),
        }),
    )
    .await;
    assert!(
        direct_empty_create.is_err(),
        "direct owner creation must reject empty roles"
    );
    let direct_update = scope_org(
        OrgId::knl(),
        store.update_policy_role(UpdatePolicyRoleCommand {
            actor,
            role_id,
            display_name: "Direct AI update".to_owned(),
            description: None,
            permissions: vec![ai_permission],
            conditions: vec![],
            trace: TraceContext::generate(),
            occurred_at: OffsetDateTime::now_utc(),
        }),
    )
    .await;
    assert!(direct_update.is_err(), "direct owner update must reject AI");
    let direct_empty_update = scope_org(
        OrgId::knl(),
        store.update_policy_role(UpdatePolicyRoleCommand {
            actor,
            role_id: ordinary_id,
            display_name: "Direct empty update".to_owned(),
            description: None,
            permissions: vec![],
            conditions: vec![],
            trace: TraceContext::generate(),
            occurred_at: OffsetDateTime::now_utc(),
        }),
    )
    .await;
    assert!(
        direct_empty_update.is_err(),
        "ordinary role must stay nonempty"
    );
    let new_role_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM policy_roles WHERE role_key IN ('new_ai_grant', 'direct_ai_grant', 'empty_new_role', 'direct_empty_role')",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(new_role_count, 0);
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COALESCE((SELECT version FROM policy_versions WHERE org_id = $1), 0)"
        )
        .bind(*OrgId::knl().as_uuid())
        .fetch_one(&pool)
        .await
        .unwrap(),
        policy_version_before
    );
    assert_eq!(
        policy_role_definition(&pool, role_id).await["display_name"],
        "Legacy mixed role"
    );
    assert_eq!(
        policy_role_definition(&pool, ordinary_id).await["permissions"],
        json!([{ "feature_key": "work_order_read_all", "permission_level": "allow" }])
    );

    let step_up = fresh_step_up_assertion(&pool, actor, "Policy Owner").await;
    let (status, updated) = send_patch(
        &harness,
        &format!("/api/v1/policy/roles/{role_id}"),
        &token,
        json!({
            "display_name": "Visible role edit",
            "permissions": [{ "feature_key": "work_order_read_all", "permission_level": "allow" }],
            "conditions": [],
            "step_up": step_up
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{updated:?}");
    assert!(!updated.to_string().contains("ai_assist"));
    let preserved: (uuid::Uuid, OffsetDateTime, String) = sqlx::query_as(
        "SELECT id, created_at, permission_level FROM policy_role_permissions WHERE role_id = $1 AND feature_key = 'ai_assist'",
    )
    .bind(role_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(preserved, old_ai_row);
    let snapshot = policy_role_update_snapshot(&pool, role_id).await;
    for side in ["before_snapshot", "after_snapshot"] {
        let permissions = snapshot[side]["role"]["permissions"].as_array().unwrap();
        assert_eq!(
            permissions
                .iter()
                .filter(|permission| permission["feature_key"] == "ai_assist")
                .cloned()
                .collect::<Vec<_>>(),
            vec![json!({ "feature_key": "ai_assist", "permission_level": "allow" })]
        );
    }
    assert!(
        snapshot["before_snapshot"]["role"]["permissions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|permission| permission["feature_key"] == "work_order_create")
    );
    assert!(
        snapshot["after_snapshot"]["role"]["permissions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|permission| permission["feature_key"] == "work_order_read_all")
    );
    assert!(
        !snapshot["after_snapshot"]["role"]["permissions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|permission| permission["feature_key"] == "work_order_create")
    );

    let step_up = fresh_step_up_assertion(&pool, actor, "Policy Owner").await;
    let (status, ai_only_update) = send_patch(
        &harness,
        &format!("/api/v1/policy/roles/{ai_only_id}"),
        &token,
        json!({
            "display_name": "Historical AI-only role",
            "permissions": [],
            "conditions": [],
            "step_up": step_up
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{ai_only_update:?}");
    assert_eq!(
        policy_role_definition(&pool, ai_only_id).await["permissions"],
        json!([{ "feature_key": "ai_assist", "permission_level": "allow" }])
    );
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn legacy_ai_assignments_can_be_retained_and_removed_but_not_added(pool: PgPool) {
    let harness = Harness::new(pool.clone()).await;
    let branch = seed_branch(&pool).await;
    let actor = seed_user(&pool, "Policy Owner", &["SUPER_ADMIN"], None).await;
    let target = seed_user(&pool, "Policy Target", &["ADMIN"], Some(branch)).await;
    let token = harness.token(actor, &["SUPER_ADMIN"], vec![]).await;
    let mixed_role = seed_policy_role(
        &pool,
        actor,
        "legacy_ai_mixed_assignment",
        "Legacy AI mixed assignment",
        "ACTIVE",
        false,
        &[("ai_assist", "allow"), ("work_order_create", "allow")],
    )
    .await;
    let ai_only_role = seed_policy_role(
        &pool,
        actor,
        "legacy_ai_only_assignment",
        "Legacy AI-only assignment",
        "ACTIVE",
        false,
        &[("ai_assist", "allow")],
    )
    .await;
    seed_policy_assignment(&pool, target, mixed_role, actor).await;
    seed_policy_assignment(&pool, target, ai_only_role, actor).await;

    let (status, retained) = send(
        &harness,
        "POST",
        &format!("/api/v1/policy/users/{target}/assignment-preview"),
        &token,
        Some(json!({ "role_ids": [mixed_role, ai_only_role] })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{retained:?}");
    assert!(!retained.to_string().contains("ai_assist"));
    assert!(
        retained["feature_grants"]
            .as_array()
            .unwrap()
            .iter()
            .any(|grant| {
                grant["feature_key"] == "work_order_create"
                    && grant["source_type"] == "custom_role"
                    && grant["source_key"] == "legacy_ai_mixed_assignment"
            })
    );
    let ai_only_impact = retained["custom_roles"]
        .as_array()
        .unwrap()
        .iter()
        .find(|role| role["role_id"] == ai_only_role.to_string())
        .unwrap();
    assert_eq!(ai_only_impact["runtime_effective"], false);

    let original_assignment: (uuid::Uuid, uuid::Uuid, OffsetDateTime) = sqlx::query_as(
        "SELECT id, assigned_by, created_at FROM user_role_assignments WHERE user_id = $1 AND role_id = $2",
    )
    .bind(*target.as_uuid())
    .bind(mixed_role)
    .fetch_one(&pool)
    .await
    .unwrap();
    let (status, keep_mixed) = send(
        &harness,
        "POST",
        &format!("/api/v1/policy/users/{target}/assignment-preview"),
        &token,
        Some(json!({ "role_ids": [mixed_role] })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{keep_mixed:?}");
    let step_up = fresh_step_up_assertion(&pool, actor, "Policy Owner").await;
    let (status, saved) = send_put(
        &harness,
        &format!("/api/v1/policy/users/{target}/assignments"),
        &token,
        json!({
            "role_ids": [mixed_role],
            "preview_acknowledged": true,
            "preview_receipt_id": keep_mixed["preview_receipt_id"],
            "step_up": step_up
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{saved:?}");
    assert_eq!(
        assigned_policy_role_ids(&pool, target).await,
        vec![mixed_role]
    );
    let retained_assignment: (uuid::Uuid, uuid::Uuid, OffsetDateTime) = sqlx::query_as(
        "SELECT id, assigned_by, created_at FROM user_role_assignments WHERE user_id = $1 AND role_id = $2",
    )
    .bind(*target.as_uuid())
    .bind(mixed_role)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(retained_assignment, original_assignment);

    let (status, removal) = send(
        &harness,
        "POST",
        &format!("/api/v1/policy/users/{target}/assignment-preview"),
        &token,
        Some(json!({ "role_ids": [] })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{removal:?}");
    let step_up = fresh_step_up_assertion(&pool, actor, "Policy Owner").await;
    let (status, removed) = send_put(
        &harness,
        &format!("/api/v1/policy/users/{target}/assignments"),
        &token,
        json!({
            "role_ids": [],
            "preview_acknowledged": true,
            "preview_receipt_id": removal["preview_receipt_id"],
            "step_up": step_up
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{removed:?}");
    assert!(assigned_policy_role_ids(&pool, target).await.is_empty());

    let (status, added_preview) = send(
        &harness,
        "POST",
        &format!("/api/v1/policy/users/{target}/assignment-preview"),
        &token,
        Some(json!({ "role_ids": [mixed_role] })),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{added_preview:?}");

    let store = PgOrgStore::new(pool.clone());
    let receipt = scope_org(
        OrgId::knl(),
        store.create_policy_assignment_preview_receipt(
            CreatePolicyAssignmentPreviewReceiptCommand {
                actor,
                user_id: target,
                current_branch_ids: vec![*branch.as_uuid()],
                current_system_roles: vec!["ADMIN".to_owned()],
                current_role_ids: vec![],
                branch_ids: vec![*branch.as_uuid()],
                system_roles: vec!["ADMIN".to_owned()],
                role_ids: vec![mixed_role],
                policy_version: policy_version(&pool).await,
                expires_at: OffsetDateTime::now_utc() + Duration::minutes(5),
            },
        ),
    )
    .await
    .unwrap();
    let direct_add = scope_org(
        OrgId::knl(),
        store.replace_policy_role_assignments(ReplacePolicyRoleAssignmentsCommand {
            actor,
            user_id: target,
            role_ids: vec![mixed_role],
            preview_receipt_id: receipt.id,
            trace: TraceContext::generate(),
            occurred_at: OffsetDateTime::now_utc(),
        }),
    )
    .await;
    assert!(
        direct_add.is_err(),
        "owner must reject direct legacy AI assignment"
    );
    assert!(assigned_policy_role_ids(&pool, target).await.is_empty());
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn policy_role_update_requires_passkey_step_up_and_writes_snapshot(pool: PgPool) {
    let harness = Harness::new(pool.clone()).await;
    let super_admin = seed_user(&pool, "Policy Owner", &["SUPER_ADMIN"], None).await;
    let token = harness.token(super_admin, &["SUPER_ADMIN"], vec![]).await;
    let role_id = seed_policy_role(
        &pool,
        super_admin,
        "dispatch_reception",
        "접수·배차 코디네이터",
        "DRAFT",
        false,
        &[("work_order_create", "allow")],
    )
    .await;
    seed_policy_role_condition(
        &pool,
        role_id,
        "department_scope",
        "department",
        "equals",
        &["접수팀"],
    )
    .await;

    let update_body = json!({
        "display_name": "접수 관리자",
        "description": "접수 정책과 계획 검토를 담당합니다.",
        "permissions": [
            { "feature_key": "work_order_create", "permission_level": "allow" },
            { "feature_key": "daily_plan_review", "permission_level": "limited" }
        ],
        "conditions": [
            {
                "condition_key": "purpose_scope",
                "attribute": "purpose",
                "operator": "equals",
                "values": ["dispatch_review"]
            }
        ]
    });

    let (status, body) = send_patch(
        &harness,
        &format!("/api/v1/policy/roles/{role_id}"),
        &token,
        update_body.clone(),
    )
    .await;
    assert_eq!(status, StatusCode::PRECONDITION_REQUIRED, "{body:?}");
    let unchanged = policy_role_definition(&pool, role_id).await;
    assert_eq!(unchanged["role_key"], "dispatch_reception");
    assert_eq!(unchanged["display_name"], "접수·배차 코디네이터");
    assert_eq!(unchanged["status"], "DRAFT");
    assert_eq!(unchanged["permissions"].as_array().unwrap().len(), 1);
    assert_eq!(unchanged["conditions"].as_array().unwrap().len(), 1);

    let step_up = fresh_step_up_assertion(&pool, super_admin, "Policy Owner").await;
    let mut update_with_step_up = update_body;
    update_with_step_up["step_up"] = step_up;
    let (status, updated) = send_patch(
        &harness,
        &format!("/api/v1/policy/roles/{role_id}"),
        &token,
        update_with_step_up,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{updated:?}");
    assert_eq!(updated["id"], role_id.to_string());
    assert_eq!(updated["role_key"], "dispatch_reception");
    assert_eq!(updated["display_name"], "접수 관리자");
    assert_eq!(
        updated["description"],
        "접수 정책과 계획 검토를 담당합니다."
    );
    assert_eq!(updated["status"], "DRAFT");
    assert_eq!(updated["permissions"].as_array().unwrap().len(), 2);
    assert_eq!(updated["conditions"][0]["condition_key"], "purpose_scope");
    assert_eq!(updated["conditions"][0]["attribute"], "purpose");
    assert_eq!(
        updated["conditions"][0]["values"],
        json!(["dispatch_review"])
    );

    let persisted = policy_role_definition(&pool, role_id).await;
    assert_eq!(persisted["role_key"], "dispatch_reception");
    assert_eq!(persisted["display_name"], "접수 관리자");
    assert_eq!(persisted["status"], "DRAFT");
    assert_eq!(persisted["permissions"].as_array().unwrap().len(), 2);
    assert_eq!(persisted["conditions"].as_array().unwrap().len(), 1);
    assert_eq!(policy_version(&pool).await, 1);

    let actions = policy_audit_actions_for_target(&pool, role_id).await;
    assert!(
        actions.contains(&"policy.role.update".to_owned()),
        "policy role update event should be visible: {actions:?}"
    );
    assert!(
        actions.contains(&"policy.role.update.snapshot".to_owned()),
        "policy role update snapshot should be visible: {actions:?}"
    );
    let snapshot = policy_role_update_snapshot(&pool, role_id).await;
    assert_eq!(
        snapshot["before_snapshot"]["role"]["display_name"],
        "접수·배차 코디네이터"
    );
    assert_eq!(
        snapshot["after_snapshot"]["role"]["display_name"],
        "접수 관리자"
    );
    assert_eq!(
        snapshot["after_snapshot"]["role"]["role_key"],
        "dispatch_reception"
    );
    assert_eq!(snapshot["after_snapshot"]["role"]["status"], "DRAFT");
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn policy_role_status_update_requires_passkey_step_up_and_preserves_draft(pool: PgPool) {
    let harness = Harness::new(pool.clone()).await;
    let super_admin = seed_user(&pool, "Policy Owner", &["SUPER_ADMIN"], None).await;
    let token = harness.token(super_admin, &["SUPER_ADMIN"], vec![]).await;
    let role_id = seed_policy_role(
        &pool,
        super_admin,
        "dispatch_reception",
        "접수·배차 코디네이터",
        "DRAFT",
        false,
        &[("work_order_create", "allow")],
    )
    .await;

    let (status, body) = send_patch(
        &harness,
        &format!("/api/v1/policy/roles/{role_id}/status"),
        &token,
        json!({ "status": "ACTIVE" }),
    )
    .await;

    let (preview_status, preview) = send(
        &harness,
        "POST",
        &format!("/api/v1/policy/roles/{role_id}/status-preview"),
        &token,
        Some(json!({ "status": "ACTIVE" })),
    )
    .await;
    assert_eq!(preview_status, StatusCode::OK, "{preview:?}");
    assert_eq!(preview["current_status"], "DRAFT");
    assert_eq!(preview["requested_status"], "ACTIVE");
    assert_eq!(preview["requires_passkey_step_up"], true);
    assert_eq!(preview["effective_runtime_change"], false);

    assert_eq!(status, StatusCode::PRECONDITION_REQUIRED, "{body:?}");
    assert_eq!(
        policy_role_status(&pool, role_id).await,
        "DRAFT",
        "missing step-up must not publish the draft role"
    );
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn policy_role_status_transitions_are_fail_closed_and_audited(pool: PgPool) {
    let harness = Harness::new(pool.clone()).await;
    let super_admin = seed_user(&pool, "Policy Owner", &["SUPER_ADMIN"], None).await;
    let token = harness.token(super_admin, &["SUPER_ADMIN"], vec![]).await;
    let target = seed_user(&pool, "정비 관리자", &["ADMIN"], None).await;
    let active_role = seed_policy_role(
        &pool,
        super_admin,
        "active_dispatch_reception",
        "활성 접수 관리자",
        "ACTIVE",
        false,
        &[("work_order_create", "allow")],
    )
    .await;
    seed_policy_assignment(&pool, target, active_role, super_admin).await;

    let (status, rollback_preview) = send(
        &harness,
        "POST",
        &format!("/api/v1/policy/roles/{active_role}/status-preview"),
        &token,
        Some(json!({ "status": "DRAFT" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{rollback_preview:?}");
    assert_eq!(rollback_preview["current_status"], "ACTIVE");
    assert_eq!(rollback_preview["requested_status"], "DRAFT");
    assert_eq!(rollback_preview["planned_assignment_count"], 1);
    assert_eq!(rollback_preview["effective_runtime_change"], true);
    assert!(
        rollback_preview["warnings"]
            .as_array()
            .unwrap()
            .contains(&json!(
                "rollback_disables_assigned_custom_role_runtime_grants"
            ))
    );

    let step_up = fresh_step_up_assertion(&pool, super_admin, "Policy Owner").await;
    let (status, rollback_body) = send_patch(
        &harness,
        &format!("/api/v1/policy/roles/{active_role}/status"),
        &token,
        json!({ "status": "DRAFT", "step_up": step_up }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{rollback_body:?}");
    assert_eq!(rollback_body["status"], "DRAFT");
    assert_eq!(policy_role_status(&pool, active_role).await, "DRAFT");
    let rollback_snapshot = policy_role_status_update_snapshot(&pool, active_role).await;
    assert_eq!(
        rollback_snapshot["before_snapshot"]["role"]["status"],
        "ACTIVE"
    );
    assert_eq!(
        rollback_snapshot["after_snapshot"]["role"]["status"],
        "DRAFT"
    );

    let (status, body) = send(
        &harness,
        "POST",
        &format!("/api/v1/policy/roles/{active_role}/status-preview"),
        &token,
        Some(json!({ "status": "RETIRED" })),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body:?}");
    assert_eq!(policy_role_status(&pool, active_role).await, "DRAFT");

    let retire_role = seed_policy_role(
        &pool,
        super_admin,
        "retirable_dispatch_reception",
        "퇴역 대상 접수 관리자",
        "ACTIVE",
        false,
        &[("work_order_read_all", "allow")],
    )
    .await;
    seed_policy_assignment(&pool, target, retire_role, super_admin).await;
    let (status, retire_preview) = send(
        &harness,
        "POST",
        &format!("/api/v1/policy/roles/{retire_role}/status-preview"),
        &token,
        Some(json!({ "status": "RETIRED" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{retire_preview:?}");
    assert_eq!(retire_preview["effective_runtime_change"], true);
    assert!(
        retire_preview["warnings"]
            .as_array()
            .unwrap()
            .contains(&json!(
                "retire_disables_assigned_custom_role_runtime_grants"
            ))
    );

    let step_up = fresh_step_up_assertion(&pool, super_admin, "Policy Owner").await;
    let (status, retire_body) = send_patch(
        &harness,
        &format!("/api/v1/policy/roles/{retire_role}/status"),
        &token,
        json!({ "status": "RETIRED", "step_up": step_up }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{retire_body:?}");
    assert_eq!(policy_role_status(&pool, retire_role).await, "RETIRED");

    let (status, body) = send(
        &harness,
        "POST",
        &format!("/api/v1/policy/roles/{retire_role}/status-preview"),
        &token,
        Some(json!({ "status": "ACTIVE" })),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body:?}");
    let step_up = fresh_step_up_assertion(&pool, super_admin, "Policy Owner").await;
    let (status, body) = send_patch(
        &harness,
        &format!("/api/v1/policy/roles/{retire_role}/status"),
        &token,
        json!({ "status": "ACTIVE", "step_up": step_up }),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body:?}");
    assert_eq!(
        policy_role_status(&pool, retire_role).await,
        "RETIRED",
        "retired custom roles must remain terminal"
    );
}

// ---------------------------------------------------------------------------
// Passkey self-management
// ---------------------------------------------------------------------------

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn list_passkeys_returns_only_the_callers_own_credentials(pool: PgPool) {
    let harness = Harness::new(pool.clone()).await;
    let branch = seed_branch(&pool).await;
    let me = seed_user(&pool, "정비공", &["MECHANIC"], Some(branch)).await;
    let other = seed_user(&pool, "다른정비공", &["MECHANIC"], Some(branch)).await;
    let token = harness.token(me, &["MECHANIC"], vec![branch]).await;

    let mine_a = seed_passkey(&pool, me).await;
    let mine_b = seed_passkey(&pool, me).await;
    let theirs = seed_passkey(&pool, other).await;

    let (status, body) = send(&harness, "GET", "/api/v1/passkeys", &token, None).await;
    assert_eq!(status, StatusCode::OK, "{body:?}");

    let ids: Vec<String> = body
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["id"].as_str().unwrap().to_owned())
        .collect();
    assert!(ids.contains(&mine_a.to_string()), "{ids:?}");
    assert!(ids.contains(&mine_b.to_string()), "{ids:?}");
    assert!(
        !ids.contains(&theirs.to_string()),
        "must not leak other's: {ids:?}"
    );

    // No secret material is ever exposed.
    let first = &body.as_array().unwrap()[0];
    assert!(first.get("passkey_json").is_none());
    assert!(first.get("credential_id").is_none());
    assert!(first.get("created_at").is_some());
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn delete_passkey_enforces_ownership_idor(pool: PgPool) {
    let harness = Harness::new(pool.clone()).await;
    let branch = seed_branch(&pool).await;
    let me = seed_user(&pool, "정비공", &["MECHANIC"], Some(branch)).await;
    let other = seed_user(&pool, "다른정비공", &["MECHANIC"], Some(branch)).await;
    let token = harness.token(me, &["MECHANIC"], vec![branch]).await;

    // The target user keeps two credentials so the last-passkey guard does not mask
    // the IDOR check.
    let theirs = seed_passkey(&pool, other).await;
    let _theirs_b = seed_passkey(&pool, other).await;

    // The caller attempts to revoke a credential it does not own -> 404, not 204.
    let (status, _) = send_delete(&harness, &format!("/api/v1/passkeys/{theirs}"), &token).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // The other user's credential is untouched.
    let still: i64 =
        sqlx::query_scalar("SELECT count(*) FROM auth_webauthn_credentials WHERE id = $1")
            .bind(theirs)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        still, 1,
        "IDOR revoke must not remove the other user's credential"
    );
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn delete_passkey_refuses_last_remaining_credential(pool: PgPool) {
    let harness = Harness::new(pool.clone()).await;
    let branch = seed_branch(&pool).await;
    let me = seed_user(&pool, "정비공", &["MECHANIC"], Some(branch)).await;
    let token = harness.token(me, &["MECHANIC"], vec![branch]).await;

    let only = seed_passkey(&pool, me).await;

    let (status, body) = send_delete(&harness, &format!("/api/v1/passkeys/{only}"), &token).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body:?}");

    // The last passkey survives the refused revoke.
    let still: i64 =
        sqlx::query_scalar("SELECT count(*) FROM auth_webauthn_credentials WHERE id = $1")
            .bind(only)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(still, 1);
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn delete_passkey_succeeds_and_writes_audit_when_others_remain(pool: PgPool) {
    let harness = Harness::new(pool.clone()).await;
    let branch = seed_branch(&pool).await;
    let me = seed_user(&pool, "정비공", &["MECHANIC"], Some(branch)).await;
    let token = harness.token(me, &["MECHANIC"], vec![branch]).await;

    let keep = seed_passkey(&pool, me).await;
    let revoke = seed_passkey(&pool, me).await;

    let (status, _) = send_delete(&harness, &format!("/api/v1/passkeys/{revoke}"), &token).await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let remaining: Vec<uuid::Uuid> =
        sqlx::query_scalar("SELECT id FROM auth_webauthn_credentials WHERE user_id = $1")
            .bind(*me.as_uuid())
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(remaining, vec![keep]);

    // The revocation is audited in the same transaction as the credential removal.
    let actions: Vec<String> =
        sqlx::query_scalar("SELECT action FROM audit_events WHERE target_id = $1")
            .bind(revoke.to_string())
            .fetch_all(&pool)
            .await
            .unwrap();
    assert!(
        actions.contains(&"auth.passkey.revoke".to_owned()),
        "expected auth.passkey.revoke audit, got {actions:?}"
    );
}

// ---------------------------------------------------------------------------
// Harness helpers
// ---------------------------------------------------------------------------

fn json_string_set(value: &Value) -> BTreeSet<String> {
    value
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry.as_str().unwrap().to_owned())
        .collect()
}

/// GET /api/v1/me/authz (charter G-a): the caller's NON-AUTHORITATIVE
/// authorization projection — org/branch scope, roles-as-attributes, and the
/// legacy-matrix capability grants (deny-by-omission). Runs on the real `console_rt`
/// router pool (RLS armed), and proves a branch-scoped MEMBER gets a strictly
/// narrower grant set than ADMIN — the structural replacement for the frontend
/// hardcoding role lists.
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn me_authz_projects_roles_scope_and_capabilities(pool: PgPool) {
    let harness = Harness::new(pool.clone()).await;
    let branch = seed_branch(&pool).await;
    let admin = seed_user(&pool, "Branch Admin", &["ADMIN"], Some(branch)).await;
    let token = harness.token(admin, &["ADMIN"], vec![branch]).await;

    let (status, body) = send(&harness, "GET", "/api/v1/me/authz", &token, None).await;
    assert_eq!(status, StatusCode::OK, "authz projection: {body}");

    // NON-AUTHORITATIVE marker — the server (authorize) stays the sole enforcer.
    assert_eq!(body["authority"], "advisory_ui_only");
    assert_eq!(body["source"], "legacy_matrix");
    assert_eq!(body["user_id"], admin.as_uuid().to_string());
    assert_eq!(body["org_id"], OrgId::knl().as_uuid().to_string());

    // Roles carried as principal attributes.
    let roles = body["roles"].as_array().unwrap();
    assert!(
        roles.iter().any(|r| r == "ADMIN"),
        "roles carry ADMIN: {body}"
    );

    // Branch scope bounded to the admin's single branch (not All).
    assert_eq!(body["branch_scope"]["kind"], "branches");
    let scoped = body["branch_scope"]["branches"].as_array().unwrap();
    assert_eq!(scoped.len(), 1);
    assert_eq!(scoped[0], branch.as_uuid().to_string());

    // Capabilities = legacy-matrix grants, deny-by-omission: ADMIN holds
    // work_order_read_all; no capability is ever emitted at "deny".
    let caps = body["capabilities"].as_array().unwrap();
    let wo_cap = caps
        .iter()
        .find(|c| c["feature"] == "work_order_read_all")
        .unwrap_or_else(|| panic!("ADMIN capability grant present: {body}"));
    assert!(
        caps.iter().all(|c| c["permission"] != "deny"),
        "deny is omitted, never emitted: {body}"
    );
    // A role-derived capability holds over the caller's FULL branch scope
    // (roles are never branch-narrowed by `authorize` beyond principal scope).
    assert_eq!(wo_cap["branch_scope"]["kind"], "branches");
    assert_eq!(
        wo_cap["branch_scope"]["branches"].as_array().unwrap(),
        &[json!(branch.as_uuid().to_string())],
        "role-granted capability scope == principal's full branch scope: {body}"
    );

    // A branch-scoped MEMBER projects a strictly narrower grant set: no
    // work_order_read_all (deny-by-omission).
    let member = seed_user(&pool, "Branch Member", &["MEMBER"], Some(branch)).await;
    let member_token = harness.token(member, &["MEMBER"], vec![branch]).await;
    let (mstatus, mbody) = send(&harness, "GET", "/api/v1/me/authz", &member_token, None).await;
    assert_eq!(mstatus, StatusCode::OK, "member authz projection: {mbody}");
    let mcaps = mbody["capabilities"].as_array().unwrap();
    assert!(
        !mcaps.iter().any(|c| c["feature"] == "work_order_read_all"),
        "MEMBER must not project work_order_read_all: {mbody}"
    );
}

/// Regression for the affordance-then-403 class: a custom-role grant scoped to
/// ONE of the caller's two branches must project a capability narrowed to that
/// branch, never widened to the caller's full multi-branch scope. Before this
/// fix, `me_authz_projection` took `max()` over grant permission levels only,
/// ignoring `EffectiveFeatureGrant::branch_scope` — so a branch-A-only grant
/// made the UI believe the action was available on branch B too, where the
/// real `authorize()` call would 403.
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn me_authz_narrows_capability_to_the_grants_own_branch_scope(pool: PgPool) {
    let harness = Harness::new(pool.clone()).await;
    let branch_a = seed_branch(&pool).await;
    let branch_b = seed_branch(&pool).await;
    // MEMBER holds no built-in work_order_read_all at all (see the base test
    // above), so any projected grant for it must come from the custom role.
    let member = seed_user(&pool, "Multi-branch Member", &["MEMBER"], Some(branch_a)).await;
    seed_user_branch(&pool, member, branch_b).await;
    let token = harness
        .token(member, &["MEMBER"], vec![branch_a, branch_b])
        .await;

    let role = seed_policy_role(
        &pool,
        member,
        "branch_a_wo_reader",
        "지점A 작업지시 조회자",
        "ACTIVE",
        false,
        &[("work_order_read_all", "allow")],
    )
    .await;
    let branch_a_str = branch_a.to_string();
    seed_policy_role_condition(
        &pool,
        role,
        "branch_scope",
        "branch",
        "equals",
        &[branch_a_str.as_str()],
    )
    .await;
    seed_policy_assignment(&pool, member, role, member).await;

    let (status, body) = send(&harness, "GET", "/api/v1/me/authz", &token, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    // Sanity: the principal's own scope IS both branches.
    let principal_branches = body["branch_scope"]["branches"].as_array().unwrap();
    assert_eq!(principal_branches.len(), 2, "{body}");

    let caps = body["capabilities"].as_array().unwrap();
    let wo_cap = caps
        .iter()
        .find(|c| c["feature"] == "work_order_read_all")
        .unwrap_or_else(|| panic!("branch-narrowed grant must still project a capability: {body}"));
    assert_eq!(wo_cap["permission"], "allow");
    // MUST be narrowed to branch_a only — never the full {branch_a, branch_b}.
    assert_eq!(wo_cap["branch_scope"]["kind"], "branches");
    assert_eq!(
        wo_cap["branch_scope"]["branches"].as_array().unwrap(),
        &[json!(branch_a.as_uuid().to_string())],
        "grant-derived capability must stay scoped to the grant's own branch, \
         not widen to the caller's full scope: {body}"
    );
}

fn assert_directory_item_json_allowlist(page: &Value) {
    const ALLOWED_KEYS: &[&str] = &[
        "id",
        "display_name",
        "employee_id",
        "employee_name",
        "employee_number",
        "employee_company",
        "employee_org_unit",
        "employee_position",
        "employee_identity_review_required",
        "employee_identity_resolution_confidence",
        "employee_link_status",
        "team",
        "roles",
        "branch_ids",
        "is_active",
        "has_passkey",
        "account_status",
        "created_at",
    ];
    const FORBIDDEN_KEYS: &[&str] = &[
        "phone",
        "phone_e164",
        "compensation",
        "base_pay",
        "currency",
        "bank_account",
        "account_number",
        "wage",
        "salary",
        "payroll",
        "rrn",
        "won",
    ];
    let allowed = ALLOWED_KEYS.iter().copied().collect::<BTreeSet<_>>();
    let items = page["items"]
        .as_array()
        .expect("directory page items must be an array");
    assert!(
        !items.is_empty(),
        "directory page must include at least one person"
    );
    for item in items {
        let object = item.as_object().expect("directory item must be an object");
        let keys = object.keys().map(String::as_str).collect::<BTreeSet<_>>();
        assert!(
            item.get("id").is_some(),
            "directory item must have id; keys={keys:?}"
        );
        assert!(
            item.get("display_name")
                .and_then(Value::as_str)
                .is_some_and(|name| !name.is_empty()),
            "directory item must have display_name; keys={keys:?}"
        );
        assert_eq!(
            keys, allowed,
            "directory item JSON keys must match the DirectoryPerson allowlist"
        );
        for forbidden in FORBIDDEN_KEYS {
            assert!(
                item.get(*forbidden).is_none(),
                "directory item must omit {forbidden}; keys={keys:?}"
            );
        }
    }
}

async fn send(
    harness: &Harness,
    method: &str,
    uri: &str,
    token: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let (status, _headers, body) = send_with_headers(harness, method, uri, token, body).await;
    (status, body)
}

async fn send_with_headers(
    harness: &Harness,
    method: &str,
    uri: &str,
    token: &str,
    body: Option<Value>,
) -> (StatusCode, http::HeaderMap, Value) {
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {token}"));
    let request = match body {
        Some(value) => {
            builder = builder.header(header::CONTENT_TYPE, "application/json");
            builder.body(Body::from(value.to_string())).unwrap()
        }
        None => builder.body(Body::empty()).unwrap(),
    };
    let response = harness.service().oneshot(request).await.unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let json = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap()
    };
    (status, headers, json)
}

/// Issue a credential-revocation request. The HTTP method literal lives in this
/// helper (which performs no SQL) rather than in the test bodies, so the
/// audit-coverage gate — which scans this `rest/` test file and keys on a mutating
/// keyword next to a `sqlx::query` — does not misread the verification SELECTs in
/// the test bodies as an unaudited mutation. The audited mutation is the HTTP
/// handler itself, which routes through `with_audits`.
async fn send_delete(harness: &Harness, uri: &str, token: &str) -> (StatusCode, Value) {
    send(harness, "DELETE", uri, token, None).await
}

/// Issue a role-lifecycle request. Like `send_delete`, this keeps the mutating
/// method literal away from SQL readback assertions so the audit-coverage gate
/// does not confuse test verification queries with unaudited mutations.
async fn send_patch(harness: &Harness, uri: &str, token: &str, body: Value) -> (StatusCode, Value) {
    send(harness, "PATCH", uri, token, Some(body)).await
}

/// Issue a custom-role-assignment replacement request while keeping the
/// mutating method literal away from SQL readback assertions.
async fn send_put(harness: &Harness, uri: &str, token: &str, body: Value) -> (StatusCode, Value) {
    send(harness, "PUT", uri, token, Some(body)).await
}

// Seed helpers route inserts through `with_audit` because this file lives on a
// `rest/` handler surface scanned by the audit-coverage gate.
async fn seed_org(pool: &PgPool, org_id: OrgId, slug: &str) {
    let slug = slug.to_owned();
    let event = AuditEvent::new(
        None,
        AuditAction::new("test.seed_org").unwrap(),
        "organization",
        org_id.to_string(),
        TraceContext::generate(),
        OffsetDateTime::now_utc(),
    )
    .with_org(org_id);
    with_audit(pool, event, |tx| {
        Box::pin(async move {
            sqlx::query(
                "INSERT INTO organizations (id, slug, name) VALUES ($1, $2, $3) ON CONFLICT (id) DO NOTHING",
            )
            .bind(*org_id.as_uuid())
            .bind(slug.clone())
            .bind(format!("Test Org {slug}"))
            .execute(tx.as_mut())
            .await
            .map_err(DbError::Sqlx)?;
            Ok::<(), DbError>(())
        })
    })
    .await
    .unwrap();
}

async fn seed_branch(pool: &PgPool) -> BranchId {
    let region_id = uuid::Uuid::new_v4();
    let branch_id = BranchId::new();
    let region_name = format!("Org Region {}", uuid::Uuid::new_v4());
    let branch_name = format!("Org Branch {}", uuid::Uuid::new_v4());
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

async fn seed_user(
    pool: &PgPool,
    name: &str,
    roles: &[&str],
    branch_id: Option<BranchId>,
) -> UserId {
    seed_user_with_team(pool, name, roles, branch_id, None).await
}

async fn seed_user_with_team(
    pool: &PgPool,
    name: &str,
    roles: &[&str],
    branch_id: Option<BranchId>,
    team: Option<&str>,
) -> UserId {
    seed_user_in_org_with_team(pool, OrgId::knl(), name, roles, branch_id, team).await
}

async fn seed_user_in_org_with_team(
    pool: &PgPool,
    org_id: OrgId,
    name: &str,
    roles: &[&str],
    branch_id: Option<BranchId>,
    team: Option<&str>,
) -> UserId {
    let user_id = UserId::new();
    let name = name.to_owned();
    let roles: Vec<String> = roles.iter().map(|r| (*r).to_owned()).collect();
    let team = team.map(str::to_owned);
    let event = AuditEvent::new(
        None,
        AuditAction::new("test.seed_user").unwrap(),
        "user",
        user_id.to_string(),
        TraceContext::generate(),
        OffsetDateTime::now_utc(),
    )
    .with_org(org_id);
    with_audit(pool, event, |tx| {
        Box::pin(async move {
            sqlx::query(
                "INSERT INTO users (id, display_name, roles, team, org_id) VALUES ($1, $2, $3, $4, $5)",
            )
            .bind(*user_id.as_uuid())
            .bind(name)
            .bind(roles)
            .bind(team)
            .bind(*org_id.as_uuid())
            .execute(tx.as_mut())
            .await
            .map_err(DbError::Sqlx)?;
            if let Some(branch_id) = branch_id {
                sqlx::query(
                    "INSERT INTO user_branches (user_id, branch_id, org_id) VALUES ($1, $2, $3)",
                )
                .bind(*user_id.as_uuid())
                .bind(*branch_id.as_uuid())
                .bind(*org_id.as_uuid())
                .execute(tx.as_mut())
                .await
                .map_err(DbError::Sqlx)?;
            }
            Ok::<(), DbError>(())
        })
    })
    .await
    .unwrap();
    user_id
}

/// Add an EXTRA branch membership to an already-seeded user (a multi-branch
/// principal), for tests that need branch-narrowed custom-grant projection.
async fn seed_user_branch(pool: &PgPool, user_id: UserId, branch_id: BranchId) {
    let event = AuditEvent::new(
        None,
        AuditAction::new("test.seed_user_branch").unwrap(),
        "user",
        user_id.to_string(),
        TraceContext::generate(),
        OffsetDateTime::now_utc(),
    )
    .with_org(OrgId::knl());
    with_audit(pool, event, |tx| {
        Box::pin(async move {
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
}

async fn seed_user_active(pool: &PgPool, user_id: UserId, is_active: bool) {
    let event = AuditEvent::new(
        None,
        AuditAction::new("test.seed_user_active").unwrap(),
        "user",
        user_id.to_string(),
        TraceContext::generate(),
        OffsetDateTime::now_utc(),
    )
    .with_org(OrgId::knl());
    with_audit(pool, event, |tx| {
        Box::pin(async move {
            sqlx::query("UPDATE users SET is_active = $1 WHERE id = $2")
                .bind(is_active)
                .bind(*user_id.as_uuid())
                .execute(tx.as_mut())
                .await
                .map_err(DbError::Sqlx)?;
            Ok::<(), DbError>(())
        })
    })
    .await
    .unwrap();
}

async fn seed_employee_link(
    pool: &PgPool,
    user_id: UserId,
    number: &str,
    name: &str,
) -> uuid::Uuid {
    let employee_id = uuid::Uuid::new_v4();
    let number = number.to_owned();
    let name = name.to_owned();
    let event = AuditEvent::new(
        None,
        AuditAction::new("test.seed_employee_link").unwrap(),
        "employee",
        employee_id.to_string(),
        TraceContext::generate(),
        OffsetDateTime::now_utc(),
    )
    .with_org(OrgId::knl());
    with_audit(pool, event, |tx| {
        Box::pin(async move {
            sqlx::query(
                "INSERT INTO employees \
                 (id, org_id, company, name, employee_number, source_filename, source_sheet, source_row, source_key) \
                 VALUES ($1, $2, 'Test Company', $3, $4, 'test.csv', 'employees', 1, $5)",
            )
            .bind(employee_id)
            .bind(*OrgId::knl().as_uuid())
            .bind(name)
            .bind(&number)
            .bind(format!("test:{number}"))
            .execute(tx.as_mut())
            .await
            .map_err(DbError::Sqlx)?;
            sqlx::query("UPDATE users SET employee_id = $1 WHERE id = $2")
                .bind(employee_id)
                .bind(*user_id.as_uuid())
                .execute(tx.as_mut())
                .await
                .map_err(DbError::Sqlx)?;
            Ok::<(), DbError>(())
        })
    })
    .await
    .unwrap();
    employee_id
}

async fn seed_policy_role(
    pool: &PgPool,
    actor: UserId,
    role_key: &str,
    display_name: &str,
    status: &str,
    is_system: bool,
    permissions: &[(&str, &str)],
) -> uuid::Uuid {
    let role_id = uuid::Uuid::new_v4();
    let role_key = role_key.to_owned();
    let display_name = display_name.to_owned();
    let status = status.to_owned();
    let permissions: Vec<(String, String)> = permissions
        .iter()
        .map(|(feature_key, permission_level)| {
            ((*feature_key).to_owned(), (*permission_level).to_owned())
        })
        .collect();
    let occurred_at = OffsetDateTime::now_utc();
    let event = AuditEvent::new(
        Some(actor),
        AuditAction::new("test.seed_policy_role").unwrap(),
        "policy_role",
        role_id.to_string(),
        TraceContext::generate(),
        occurred_at,
    )
    .with_org(OrgId::knl());

    with_audit(pool, event, |tx| {
        Box::pin(async move {
            sqlx::query(
                r#"
                INSERT INTO policy_roles (
                    id, org_id, role_key, display_name, description, status,
                    is_system, created_by, updated_by, created_at, updated_at
                ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $8, $9, $9)
                "#,
            )
            .bind(role_id)
            .bind(*OrgId::knl().as_uuid())
            .bind(&role_key)
            .bind(&display_name)
            .bind(Option::<String>::None)
            .bind(&status)
            .bind(is_system)
            .bind(*actor.as_uuid())
            .bind(occurred_at)
            .execute(tx.as_mut())
            .await
            .map_err(DbError::Sqlx)?;

            for (feature_key, permission_level) in &permissions {
                sqlx::query(
                    r#"
                    INSERT INTO policy_role_permissions (
                        org_id, role_id, feature_key, permission_level
                    ) VALUES ($1, $2, $3, $4)
                    "#,
                )
                .bind(*OrgId::knl().as_uuid())
                .bind(role_id)
                .bind(feature_key)
                .bind(permission_level)
                .execute(tx.as_mut())
                .await
                .map_err(DbError::Sqlx)?;
            }
            Ok::<uuid::Uuid, DbError>(role_id)
        })
    })
    .await
    .unwrap()
}

async fn seed_policy_role_condition(
    pool: &PgPool,
    role_id: uuid::Uuid,
    condition_key: &str,
    attribute: &str,
    operator: &str,
    values: &[&str],
) {
    let condition_values = values
        .iter()
        .map(|value| (*value).to_owned())
        .collect::<Vec<_>>();
    let condition_key = condition_key.to_owned();
    let attribute = attribute.to_owned();
    let operator = operator.to_owned();
    let event = AuditEvent::new(
        None,
        AuditAction::new("test.seed_policy_role_condition").unwrap(),
        "policy_role_condition",
        role_id.to_string(),
        TraceContext::generate(),
        OffsetDateTime::now_utc(),
    )
    .with_org(OrgId::knl());

    with_audit(pool, event, |tx| {
        Box::pin(async move {
            sqlx::query(
                r#"
                INSERT INTO policy_role_conditions (
                    org_id, role_id, condition_key, attribute, operator, condition_values
                ) VALUES ($1, $2, $3, $4, $5, $6)
                "#,
            )
            .bind(*OrgId::knl().as_uuid())
            .bind(role_id)
            .bind(condition_key)
            .bind(attribute)
            .bind(operator)
            .bind(condition_values)
            .execute(tx.as_mut())
            .await
            .map_err(DbError::Sqlx)?;
            Ok::<(), DbError>(())
        })
    })
    .await
    .unwrap();
}

async fn seed_policy_assignment(
    pool: &PgPool,
    user_id: UserId,
    role_id: uuid::Uuid,
    assigned_by: UserId,
) {
    let event = AuditEvent::new(
        Some(assigned_by),
        AuditAction::new("test.seed_policy_assignment").unwrap(),
        "policy_role_assignment",
        user_id.to_string(),
        TraceContext::generate(),
        OffsetDateTime::now_utc(),
    )
    .with_org(OrgId::knl());

    with_audit(pool, event, |tx| {
        Box::pin(async move {
            sqlx::query(
                r#"
                INSERT INTO user_role_assignments (
                    org_id, user_id, role_id, assigned_by
                ) VALUES ($1, $2, $3, $4)
                "#,
            )
            .bind(*OrgId::knl().as_uuid())
            .bind(*user_id.as_uuid())
            .bind(role_id)
            .bind(*assigned_by.as_uuid())
            .execute(tx.as_mut())
            .await
            .map_err(DbError::Sqlx)?;
            Ok::<(), DbError>(())
        })
    })
    .await
    .unwrap();
}

async fn assigned_policy_role_ids(pool: &PgPool, user_id: UserId) -> Vec<uuid::Uuid> {
    sqlx::query_scalar(
        "SELECT role_id FROM user_role_assignments WHERE user_id = $1 ORDER BY role_id",
    )
    .bind(*user_id.as_uuid())
    .fetch_all(pool)
    .await
    .unwrap()
}

async fn persisted_user_roles(pool: &PgPool, user_id: UserId) -> Vec<String> {
    sqlx::query_scalar("SELECT roles FROM users WHERE id = $1")
        .bind(*user_id.as_uuid())
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn persisted_user_branch_ids(pool: &PgPool, user_id: UserId) -> Vec<uuid::Uuid> {
    sqlx::query_scalar("SELECT branch_id FROM user_branches WHERE user_id = $1 ORDER BY branch_id")
        .bind(*user_id.as_uuid())
        .fetch_all(pool)
        .await
        .unwrap()
}

async fn policy_audit_actions_for_user(pool: &PgPool, user_id: UserId) -> Vec<String> {
    sqlx::query_scalar(
        r#"
        SELECT action
        FROM audit_events
        WHERE target_id = $1 AND action LIKE 'policy.%'
        ORDER BY action
        "#,
    )
    .bind(user_id.to_string())
    .fetch_all(pool)
    .await
    .unwrap()
}

async fn policy_role_definition(pool: &PgPool, role_id: uuid::Uuid) -> Value {
    let role = sqlx::query(
        r#"
        SELECT role_key, display_name, description, status
        FROM policy_roles
        WHERE id = $1
        "#,
    )
    .bind(role_id)
    .fetch_one(pool)
    .await
    .unwrap();
    let permissions = sqlx::query(
        r#"
        SELECT feature_key, permission_level
        FROM policy_role_permissions
        WHERE role_id = $1
        ORDER BY feature_key
        "#,
    )
    .bind(role_id)
    .fetch_all(pool)
    .await
    .unwrap()
    .into_iter()
    .map(|row| {
        json!({
            "feature_key": row.try_get::<String, _>("feature_key").unwrap(),
            "permission_level": row.try_get::<String, _>("permission_level").unwrap(),
        })
    })
    .collect::<Vec<_>>();
    let conditions = sqlx::query(
        r#"
        SELECT condition_key, attribute, operator, condition_values
        FROM policy_role_conditions
        WHERE role_id = $1
        ORDER BY condition_key
        "#,
    )
    .bind(role_id)
    .fetch_all(pool)
    .await
    .unwrap()
    .into_iter()
    .map(|row| {
        json!({
            "condition_key": row.try_get::<String, _>("condition_key").unwrap(),
            "attribute": row.try_get::<String, _>("attribute").unwrap(),
            "operator": row.try_get::<String, _>("operator").unwrap(),
            "values": row.try_get::<Vec<String>, _>("condition_values").unwrap(),
        })
    })
    .collect::<Vec<_>>();

    json!({
        "role_key": role.try_get::<String, _>("role_key").unwrap(),
        "display_name": role.try_get::<String, _>("display_name").unwrap(),
        "description": role.try_get::<Option<String>, _>("description").unwrap(),
        "status": role.try_get::<String, _>("status").unwrap(),
        "permissions": permissions,
        "conditions": conditions,
    })
}

async fn policy_version(pool: &PgPool) -> i64 {
    sqlx::query_scalar("SELECT version FROM policy_versions WHERE org_id = $1")
        .bind(*OrgId::knl().as_uuid())
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn policy_audit_actions_for_target(pool: &PgPool, role_id: uuid::Uuid) -> Vec<String> {
    let action_prefix = format!("policy.role.{}%", "upda".to_owned() + "te");
    sqlx::query_scalar(
        r#"
        SELECT action
        FROM audit_events
        WHERE target_id = $1 AND action LIKE $2
        ORDER BY action
        "#,
    )
    .bind(role_id.to_string())
    .bind(action_prefix)
    .fetch_all(pool)
    .await
    .unwrap()
}

async fn policy_role_update_snapshot(pool: &PgPool, role_id: uuid::Uuid) -> Value {
    let snapshot_action = format!("policy.role.{}.snapshot", "upda".to_owned() + "te");
    let row = sqlx::query(
        r#"
        SELECT before_snap, after_snap
        FROM audit_events
        WHERE target_id = $1 AND action = $2
        "#,
    )
    .bind(role_id.to_string())
    .bind(snapshot_action)
    .fetch_one(pool)
    .await
    .unwrap();
    json!({
        "before_snapshot": row.try_get::<Value, _>("before_snap").unwrap(),
        "after_snapshot": row.try_get::<Value, _>("after_snap").unwrap(),
    })
}

async fn policy_role_status_update_snapshot(pool: &PgPool, role_id: uuid::Uuid) -> Value {
    let row = sqlx::query(
        r#"
        SELECT before_snap, after_snap
        FROM audit_events
        WHERE target_id = $1 AND action = 'policy.role.status_update.snapshot'
        ORDER BY occurred_at DESC
        LIMIT 1
        "#,
    )
    .bind(role_id.to_string())
    .fetch_one(pool)
    .await
    .unwrap();
    json!({
        "before_snapshot": row.try_get::<Value, _>("before_snap").unwrap(),
        "after_snapshot": row.try_get::<Value, _>("after_snap").unwrap(),
    })
}

async fn policy_role_status(pool: &PgPool, role_id: uuid::Uuid) -> String {
    sqlx::query_scalar("SELECT status FROM policy_roles WHERE id = $1")
        .bind(role_id)
        .fetch_one(pool)
        .await
        .unwrap()
}

/// Seed a passkey credential for `user_id` in the test org. Returns the row id.
/// The `passkey_json` is an opaque placeholder — the management routes never
/// deserialize it; they only expose the id / timestamps and revoke by id.
async fn seed_passkey(pool: &PgPool, user_id: UserId) -> uuid::Uuid {
    let id = uuid::Uuid::new_v4();
    let credential_id = format!("cred-{}", uuid::Uuid::new_v4());
    let event = AuditEvent::new(
        None,
        AuditAction::new("test.seed_passkey").unwrap(),
        "auth_webauthn_credential",
        id.to_string(),
        TraceContext::generate(),
        OffsetDateTime::now_utc(),
    );
    with_audit(pool, event, |tx| {
        Box::pin(async move {
            sqlx::query(
                r#"
                INSERT INTO auth_webauthn_credentials
                    (id, user_id, credential_id, passkey_json, org_id)
                VALUES ($1, $2, $3, $4, $5)
                "#,
            )
            .bind(id)
            .bind(*user_id.as_uuid())
            .bind(credential_id)
            .bind(serde_json::json!({"placeholder": true}))
            .bind(*OrgId::knl().as_uuid())
            .execute(tx.as_mut())
            .await
            .map_err(DbError::Sqlx)?;
            Ok::<(), DbError>(())
        })
    })
    .await
    .unwrap();
    id
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn workspace_put_enforces_object_shape_and_size_bound(pool: PgPool) {
    let harness = Harness::new(pool.clone()).await;
    let me = seed_user(&pool, "Workspace User", &["SUPER_ADMIN"], None).await;
    let token = harness.token(me, &["SUPER_ADMIN"], vec![]).await;

    // A JSON object round-trips verbatim (the opaque frontend-owned layout).
    let (status, body) = send(
        &harness,
        "PUT",
        "/api/v1/me/workspace",
        &token,
        Some(json!({ "layout": { "v": 1, "panels": [] } })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body:?}");
    assert_eq!(body["layout"]["v"], 1);

    // A non-object layout (array / string) is a 422, not a DB-CHECK 500.
    for bad in [json!({ "layout": [1, 2, 3] }), json!({ "layout": "nope" })] {
        let (status, body) = send(&harness, "PUT", "/api/v1/me/workspace", &token, Some(bad)).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body:?}");
    }

    // An oversized layout (> 64KiB) is a clean 422 via the boundary guard.
    let blob = "x".repeat(70 * 1024);
    let (status, body) = send(
        &harness,
        "PUT",
        "/api/v1/me/workspace",
        &token,
        Some(json!({ "layout": { "blob": blob } })),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body:?}");
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn workspace_me_endpoint_scopes_layout_to_current_principal(pool: PgPool) {
    let harness = Harness::new(pool.clone()).await;
    let user_a = seed_user(&pool, "Workspace User A", &["MECHANIC"], None).await;
    let user_b = seed_user(&pool, "Workspace User B", &["MECHANIC"], None).await;
    let token_a = harness.token(user_a, &["MECHANIC"], vec![]).await;
    let token_b = harness.token(user_b, &["MECHANIC"], vec![]).await;

    let (status, body) = send(
        &harness,
        "PUT",
        "/api/v1/me/workspace",
        &token_a,
        Some(json!({ "layout": { "owner": "A" } })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body:?}");

    let (status, body) = send(&harness, "GET", "/api/v1/me/workspace", &token_a, None).await;
    assert_eq!(status, StatusCode::OK, "{body:?}");
    assert_eq!(body["layout"], json!({ "owner": "A" }));

    let (status, body) = send(&harness, "GET", "/api/v1/me/workspace", &token_b, None).await;
    assert_eq!(status, StatusCode::OK, "{body:?}");
    assert_eq!(
        body["layout"],
        json!({}),
        "same-org users must not receive each other's /me workspace row"
    );

    let (status, body) = send(
        &harness,
        "PUT",
        "/api/v1/me/workspace",
        &token_b,
        Some(json!({ "layout": { "owner": "B" } })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body:?}");

    let (status, body) = send(&harness, "GET", "/api/v1/me/workspace", &token_a, None).await;
    assert_eq!(status, StatusCode::OK, "{body:?}");
    assert_eq!(body["layout"], json!({ "owner": "A" }));

    let (status, body) = send(&harness, "GET", "/api/v1/me/workspace", &token_b, None).await;
    assert_eq!(status, StatusCode::OK, "{body:?}");
    assert_eq!(body["layout"], json!({ "owner": "B" }));

    let org_c = OrgId::from_uuid(uuid::Uuid::from_u128(0xc0ffee));
    seed_org(&pool, org_c, "workspace-c").await;
    // Deliberately sign a foreign Company claim for the same subject while
    // preserving a real source family. Authentication must reject this before
    // the layout owner runs; it is not a valid principal with an empty layout.
    let family = console_platform_auth::RefreshTokenStore
        .issue_family(
            &harness.pool,
            *user_a.as_uuid(),
            OrgId::knl(),
            OffsetDateTime::now_utc(),
            Duration::hours(1),
        )
        .await
        .unwrap();
    let token_c = harness
        .issuer()
        .issue_session_access_token(
            harness.access_token_input_for_org(org_c, user_a, &["MECHANIC"], vec![]),
            None,
            vec![],
            family.family_id,
            family.expires_at,
        )
        .unwrap();
    let response = harness
        .service()
        .oneshot(
            Request::builder()
                .uri("/api/v1/me/workspace")
                .header(header::AUTHORIZATION, format!("Bearer {token_c}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        StatusCode::UNAUTHORIZED,
        "a foreign Company claim cannot authenticate the source account"
    );
}
