//! OWNED — a lane may not edit this file, with ONE narrow exception recorded
//! here: [`attach_view_permits`], the object-policy attachment at the tail of
//! bootstrap.
//!
//! That attachment is LOAD-BEARING for all four REST-driver reads. Instance
//! visibility is deny-by-default: with no object policy attached, the list
//! serves nothing and — since the single-instance reads are gated too — `read`,
//! `history`, `traverse`, `acting` and `resolve` are all 404. The suite used to
//! pass only because those five reads ignored the policy entirely; closing that
//! made an explicit, org-authored permit the price of every read here.
//!
//! Deny-by-default is intact: the org AUTHORED a permit through the audited HTTP
//! writer. The engine assumes nothing.
//!
//! Nothing else here may change. No assertion in this suite or its helpers may
//! be weakened, deleted, loosened or made conditional; if conformance ever looks
//! like it needs one changed, that is the vacuity failure this suite exists to
//! prevent — stop and escalate.
//!
//! Pools, roles, tokens and the built-in catalog install shared by both drivers.
//!
//! NOTE for anyone editing the helper files: `tools/buck/gen_first_party.py`
//! classifies a `tests/**` file as a TEST BINARY by plain substring match on
//! `TEST_MARKERS` (`:1407`), COMMENTS INCLUDED. Writing the sqlx test attribute
//! literally in a doc comment here makes the generator demand a
//! `TEST_RESOURCE_REQUIREMENTS` entry for a helper. Spell it `sqlx::test`.
//!
//! Three pools, three roles. Mixing them up produces green-looking nonsense:
//!   * `owner_pool`   — the `sqlx::test` superuser (BYPASSRLS). SEEDING ONLY.
//!     Building a driver off it makes every RLS assertion vacuous.
//!   * `runtime_pool` — `SET ROLE console_rt` (NOSUPERUSER, NOBYPASSRLS, FORCE
//!     RLS). Every read and every instance write.
//!   * `command_pool` — `SET ROLE console_ontology_cmd`. Object-TYPE writes only;
//!     without it every registry mutation returns `CommandUnavailable`
//!     (`adapter-postgres/src/lib.rs:364-368`).

use console_governance_adapter_postgres::PgGovernanceStore;
use console_kernel_core::{OrgId, UserId};
use console_ontology_adapter_postgres::PgOntologyStore;
use console_ontology_adapter_postgres::instances::PgInstanceStore;
use console_ontology_adapter_postgres::seed::seed_governed_config_object_types;
use console_ontology_rest::OntologyRestState;
use console_platform_auth::{AccessTokenInput, JwtIssuer, JwtSettings, JwtVerifier};
use console_platform_request_context::scope_org;
use console_platform_test_support::{runtime_role_pool, seed_org_and_super_admin};
use p256::ecdsa::SigningKey;
use p256::elliptic_curve::rand_core::OsRng;
use p256::pkcs8::{EncodePrivateKey, EncodePublicKey, LineEnding};
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use time::macros::datetime;
use time::{Duration, OffsetDateTime};

const TEST_ISSUER: &str = "console-platform-auth";
const TEST_AUDIENCE: &str = "console-api";

/// The catalog-install instant, and the scenario's T0.
pub const AT: OffsetDateTime = datetime!(2026-07-10 12:00 UTC);
/// Founding / hiring instant.
pub const T0: OffsetDateTime = AT;
/// Transfer instant (T0 + 24h). `stage_revision` requires `valid_from` strictly
/// after the current revision's (`instances.rs:733-739`).
pub const T1: OffsetDateTime = datetime!(2026-07-11 12:00 UTC);

pub struct Harness {
    pub org: OrgId,
    pub admin: UserId,
    /// No `users` row: roles come from the verified token claims
    /// (`request-context/src/lib.rs:333-339`) and `Role::Executive` short-circuits
    /// branch-scope resolution to `BranchScope::All` without a query
    /// (`authz/src/lib.rs:1478-1483`).
    pub executive: UserId,
    /// The four-eyes counterparty for schema publication, and — unlike
    /// [`Self::executive`] — a REAL, ACTIVE `users` row, because every layer that
    /// enforces four-eyes reads the table rather than a token:
    /// `gov_approvals` FKs `(approver_id, org_id) -> users(id, org_id)` plus
    /// `CHECK (approver_id <> requested_by)` (`0153_create_governance.sql:74,78`),
    /// and `assert_write_context` additionally requires `u.is_active`
    /// (`0165_ontology_object_type_key_revisions.sql:468-473`).
    ///
    /// Unread until the first lane publishes a type, and pre-reserved rather than
    /// added by that lane precisely so no lane has to edit this owned file. The
    /// allow goes away on its own the moment any `declare` body stops being a
    /// no-op; it is not suppressing an unused field, it is holding a seam open.
    #[allow(dead_code)]
    pub approver: UserId,
    pub runtime_pool: PgPool,
    pub command_pool: PgPool,
    pub admin_token: String,
    pub executive_token: String,
    public_pem: String,
}

impl Harness {
    pub async fn bootstrap(owner_pool: PgPool) -> Self {
        let org = OrgId::knl();
        let admin =
            seed_org_and_super_admin(&owner_pool, *org.as_uuid(), "company-conformance").await;
        let executive = UserId::new();
        // A SECOND real user. `seed_org_and_super_admin` conflicts the org row away
        // and always inserts a fresh `users` row, so a second call is the whole
        // change; `executive` cannot serve here because it deliberately has none.
        let approver =
            seed_org_and_super_admin(&owner_pool, *org.as_uuid(), "company-conformance-approver")
                .await;
        let command_pool = command_role_pool(&owner_pool).await;

        // Install the built-in catalog BEFORE anything else. A hand-authored type
        // first trips `ontology_builtin.empty_org_required` (23514,
        // `0204_ontology_catalog_additive_upgrade.sql:119-123`). One call installs
        // all 27 (`seed.rs:1310-1322` = 9 governed + 3 C-chain + 15 projected).
        scope_org(org, async {
            let store =
                PgOntologyStore::new(owner_pool.clone()).with_command_pool(command_pool.clone());
            seed_governed_config_object_types(&store, admin, AT)
                .await
                .expect("install the built-in catalog")
        })
        .await;

        let runtime_pool = runtime_role_pool(&owner_pool).await;

        // BOTH tokens from ONE keypair. Two keypairs yields 401
        // `invalid bearer token` and destroys CC-12 / CTL-5.
        let signing_key = SigningKey::random(&mut OsRng);
        let private_pem = signing_key
            .to_pkcs8_pem(LineEnding::LF)
            .expect("encode private pem");
        let public_pem = signing_key
            .verifying_key()
            .to_public_key_pem(LineEnding::LF)
            .expect("encode public pem");
        let issuer =
            JwtIssuer::from_es256_pem(settings(), private_pem.as_bytes(), public_pem.as_bytes())
                .expect("build issuer");

        let admin_token = token(&issuer, admin, org, "SUPER_ADMIN");
        let executive_token = token(&issuer, executive, org, "EXECUTIVE");

        let harness = Self {
            org,
            admin,
            executive,
            approver,
            runtime_pool,
            command_pool,
            admin_token,
            executive_token,
            public_pem,
        };

        // The lane types, LAST — after the built-in catalog install above, which
        // `0204_ontology_catalog_additive_upgrade.sql:119-123` requires: an org
        // holding `ont_object_types` rows with no prior
        // `ont_builtin_catalog_installs` row raises 23514
        // `ontology_builtin.empty_org_required`. Unbuilt types are no-ops, so this
        // leaves the suite RED exactly where it should be.
        crate::fixtures::declare_all(&harness).await;

        // LAST, after every type exists: one org-authored view permit per object
        // type. See the module header — this is the narrow R2 exception.
        attach_view_permits(&harness).await;

        harness
    }

    pub fn verifier(&self) -> JwtVerifier {
        JwtVerifier::from_es256_public_pem(settings(), self.public_pem.as_bytes())
            .expect("build verifier")
    }

    /// The shared engine surface. `jwt_verifier` is `Some` for the REST driver
    /// (`None` makes EVERY request 503, not 401) and irrelevant to the store
    /// driver, which never routes.
    pub fn state(&self, verifier: Option<JwtVerifier>) -> OntologyRestState {
        OntologyRestState::new(
            PgOntologyStore::new(self.runtime_pool.clone())
                .with_command_pool(self.command_pool.clone()),
            PgInstanceStore::new(self.runtime_pool.clone()),
            PgGovernanceStore::new(self.runtime_pool.clone()),
            verifier,
        )
    }

    pub fn instances(&self) -> PgInstanceStore {
        PgInstanceStore::new(self.runtime_pool.clone())
    }

    pub fn registry(&self) -> PgOntologyStore {
        PgOntologyStore::new(self.runtime_pool.clone()).with_command_pool(self.command_pool.clone())
    }
}

/// Attach ONE unconditional enforced `view` permit to every object type the org
/// holds, through the audited HTTP writer this slice ships.
///
/// The types are ENUMERATED FROM THE ENGINE rather than derived from a list.
/// `LANE_TYPES` omits the built-in `position` and `customer` that the controls
/// drive, and a lane instance's traversal neighbour can be built-in-typed, so any
/// hand-maintained list would silently drift and 404 a control.
///
/// Driving it over HTTP rather than through the store is deliberate: it exercises
/// the authority gate, the derived `resource_type` and the 422 mapping, which
/// makes this suite the writer's strongest integration test.
async fn attach_view_permits(h: &Harness) {
    let service = console_ontology_rest::router(h.state(Some(h.verifier())));
    let (status, body) = request_json(
        &service,
        "GET",
        "/api/v1/ontology/object-types",
        &h.admin_token,
        serde_json::Value::Null,
    )
    .await;
    assert!(
        status.is_success(),
        "object-type enumeration must succeed before attaching permits: {status} {body}"
    );
    // No dedup: the enumeration is `SELECT DISTINCT ON (o.stable_key)`
    // (`adapter-postgres/src/lib.rs:611`), one row per key.
    for object_type in body.as_array().expect("object-type list is an array") {
        let key = object_type["stable_key"]
            .as_str()
            .expect("every object type carries a stable_key");
        let (status, body) = request_json(
            &service,
            "POST",
            &format!("/api/v1/ontology/object-types/{key}/policies"),
            &h.admin_token,
            serde_json::json!({ "effect": "permit", "conditions": [] }),
        )
        .await;
        // Never `continue` on a non-2xx: an unbuilt lane type simply is not in
        // this list, so there is nothing to swallow, and a silently failed attach
        // would 404 every read of that type.
        assert_eq!(
            status,
            axum::http::StatusCode::CREATED,
            "attaching the view permit for {key} must be created: {status} {body}"
        );
    }
}

async fn request_json(
    service: &axum::Router,
    method: &str,
    uri: &str,
    token: &str,
    body: serde_json::Value,
) -> (axum::http::StatusCode, serde_json::Value) {
    use axum::body::{Body, to_bytes};
    use axum::http::{Request, header};
    use tower::ServiceExt;

    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {token}"));
    let payload = if body == serde_json::Value::Null {
        Body::empty()
    } else {
        builder = builder.header(header::CONTENT_TYPE, "application/json");
        Body::from(serde_json::to_vec(&body).expect("serialize request body"))
    };
    let response = service
        .clone()
        .oneshot(builder.body(payload).expect("build request"))
        .await
        .expect("router response");
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("read body");
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null),
    )
}

fn settings() -> JwtSettings {
    JwtSettings {
        issuer: TEST_ISSUER.to_owned(),
        audience: TEST_AUDIENCE.to_owned(),
        access_token_ttl: Duration::minutes(15),
    }
}

fn token(issuer: &JwtIssuer, subject: UserId, org: OrgId, role: &str) -> String {
    issuer
        .issue_access_token(AccessTokenInput {
            subject,
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
        .expect("issue access token")
}

async fn command_role_pool(owner_pool: &PgPool) -> PgPool {
    let options = owner_pool.connect_options().as_ref().clone();
    PgPoolOptions::new()
        .max_connections(4)
        .after_connect(|conn, _meta| {
            Box::pin(async move {
                sqlx::query("SET ROLE console_ontology_cmd")
                    .execute(conn)
                    .await?;
                Ok(())
            })
        })
        .connect_with(options)
        .await
        .expect("connect console_ontology_cmd-role test pool")
}
