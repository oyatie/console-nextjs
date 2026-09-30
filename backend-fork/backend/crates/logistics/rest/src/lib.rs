//! Authenticated logistics-pilot routes.  Every write has a distinct
//! capability grant; there is no inherited inventory or dispatch permission.
mod openapi;
pub use openapi::OPENAPI_FRAGMENT;

use axum::{
    Json, Router,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
};
use console_kernel_core::{BranchId, BranchScope, ErrorKind, KernelError};
use console_logistics_adapter_postgres::{PgLogisticsError, PgLogisticsStore};
use console_platform_auth::JwtVerifier;
use console_platform_authz::{Action, Feature, Principal, authorize, authorize_org_wide};
use console_platform_request_context::RequestContextError;
use serde::Deserialize;
use serde_json::{Value, json};
use time::OffsetDateTime;
use uuid::Uuid;

pub const LOGISTICS_ROUTE_PATHS: &[&str] = &[
    "/api/v1/logistics/asns",
    "/api/v1/logistics/asns/{asn_id}/receipts",
    "/api/v1/logistics/asns/{asn_id}/putaway",
    "/api/v1/logistics/fulfillments",
    "/api/v1/logistics/fulfillments/{fulfillment_id}/pick",
    "/api/v1/logistics/fulfillments/{fulfillment_id}/pack",
    "/api/v1/logistics/fulfillments/{fulfillment_id}/dispatch",
    "/api/v1/logistics/shipments/{shipment_id}/pod",
    "/api/v1/logistics/shipments/{shipment_id}/settlements",
];
#[derive(Clone)]
pub struct LogisticsRestState {
    store: PgLogisticsStore,
    jwt: Option<JwtVerifier>,
}
impl LogisticsRestState {
    #[must_use]
    pub fn new(store: PgLogisticsStore, jwt: Option<JwtVerifier>) -> Self {
        Self { store, jwt }
    }
}
pub fn router(state: LogisticsRestState) -> Router {
    let verifier = state.jwt.clone();
    let pool = state.store.pool().clone();
    let r = Router::new()
        .route("/api/v1/logistics/asns", post(create_asn))
        .route("/api/v1/logistics/asns/{asn_id}/receipts", post(receive))
        .route("/api/v1/logistics/asns/{asn_id}/putaway", post(putaway))
        .route("/api/v1/logistics/fulfillments", post(release))
        .route(
            "/api/v1/logistics/fulfillments/{fulfillment_id}/pick",
            post(pick),
        )
        .route(
            "/api/v1/logistics/fulfillments/{fulfillment_id}/pack",
            post(pack),
        )
        .route(
            "/api/v1/logistics/fulfillments/{fulfillment_id}/dispatch",
            post(dispatch),
        )
        .route("/api/v1/logistics/shipments/{shipment_id}/pod", post(pod))
        .route(
            "/api/v1/logistics/shipments/{shipment_id}/settlements",
            post(settle),
        )
        .with_state(state);
    console_platform_request_context::with_request_context(r, verifier, pool)
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AsnBody {
    branch_id: Uuid,
    warehouse_code: String,
    external_reference: String,
    sku: String,
    expected_quantity: i64,
}
async fn create_asn(
    State(s): State<LogisticsRestState>,
    h: HeaderMap,
    Json(b): Json<AsnBody>,
) -> Result<(StatusCode, Json<Value>), RestError> {
    let p = principal(&s, &h).await?;
    allow(
        &p,
        Feature::LogisticsReceive,
        BranchId::from_uuid(b.branch_id),
    )?;
    Ok((
        StatusCode::CREATED,
        Json(
            s.store
                .create_asn(
                    p.user_id,
                    BranchId::from_uuid(b.branch_id),
                    b.warehouse_code,
                    b.external_reference,
                    b.sku,
                    b.expected_quantity,
                )
                .await
                .map_err(RestError::store)?,
        ),
    ))
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ReceiptBody {
    /// Legacy client hint; authorization and persistence derive branch ownership
    /// from the locked ASN, never from request JSON.
    #[serde(default, rename = "branchId")]
    _branch_hint: Option<Uuid>,
    received_quantity: i64,
}
async fn receive(
    State(s): State<LogisticsRestState>,
    h: HeaderMap,
    Path(id): Path<Uuid>,
    Json(b): Json<ReceiptBody>,
) -> Result<Json<Value>, RestError> {
    let p = principal(&s, &h).await?;
    let branch = s.store.asn_branch(id).await.map_err(RestError::store)?;
    allow(&p, Feature::LogisticsReceive, branch)?;
    let key = idem_header(&h)?;
    let fingerprint = json!({"asnId":id,"receivedQuantity":b.received_quantity});
    Ok(Json(
        s.store
            .receive(p.user_id, id, b.received_quantity, key, &fingerprint)
            .await
            .map_err(RestError::store)?,
    ))
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct BranchBody {
    /// Legacy client hint; aggregate ownership is authoritative.
    #[serde(default, rename = "branchId")]
    _branch_hint: Option<Uuid>,
}
async fn putaway(
    State(s): State<LogisticsRestState>,
    h: HeaderMap,
    Path(id): Path<Uuid>,
    Json(_b): Json<BranchBody>,
) -> Result<Json<Value>, RestError> {
    let p = principal(&s, &h).await?;
    let branch = s.store.asn_branch(id).await.map_err(RestError::store)?;
    allow(&p, Feature::LogisticsPutaway, branch)?;
    Ok(Json(
        s.store
            .putaway(p.user_id, id)
            .await
            .map_err(RestError::store)?,
    ))
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ReleaseBody {
    branch_id: Uuid,
    warehouse_code: String,
    sku: String,
    requested_quantity: i64,
    due_at: OffsetDateTime,
}
async fn release(
    State(s): State<LogisticsRestState>,
    h: HeaderMap,
    Json(b): Json<ReleaseBody>,
) -> Result<(StatusCode, Json<Value>), RestError> {
    let p = principal(&s, &h).await?;
    allow(
        &p,
        Feature::LogisticsRelease,
        BranchId::from_uuid(b.branch_id),
    )?;
    Ok((
        StatusCode::CREATED,
        Json(
            s.store
                .release(
                    p.user_id,
                    BranchId::from_uuid(b.branch_id),
                    b.warehouse_code,
                    b.sku,
                    b.requested_quantity,
                    b.due_at,
                )
                .await
                .map_err(RestError::store)?,
        ),
    ))
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PickBody {
    /// Legacy client hint; aggregate ownership is authoritative.
    #[serde(default, rename = "branchId")]
    _branch_hint: Option<Uuid>,
    picked_quantity: i64,
}
async fn pick(
    State(s): State<LogisticsRestState>,
    h: HeaderMap,
    Path(id): Path<Uuid>,
    Json(b): Json<PickBody>,
) -> Result<Json<Value>, RestError> {
    let p = principal(&s, &h).await?;
    let branch = s
        .store
        .fulfillment_branch(id)
        .await
        .map_err(RestError::store)?;
    allow(&p, Feature::LogisticsPickPack, branch)?;
    Ok(Json(
        s.store
            .pick_pack(p.user_id, id, Some(b.picked_quantity), false)
            .await
            .map_err(RestError::store)?,
    ))
}
async fn pack(
    State(s): State<LogisticsRestState>,
    h: HeaderMap,
    Path(id): Path<Uuid>,
    Json(_b): Json<BranchBody>,
) -> Result<Json<Value>, RestError> {
    let p = principal(&s, &h).await?;
    let branch = s
        .store
        .fulfillment_branch(id)
        .await
        .map_err(RestError::store)?;
    allow(&p, Feature::LogisticsPickPack, branch)?;
    Ok(Json(
        s.store
            .pick_pack(p.user_id, id, None, true)
            .await
            .map_err(RestError::store)?,
    ))
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DispatchBody {
    /// Legacy client hint; aggregate ownership is authoritative.
    #[serde(default, rename = "branchId")]
    _branch_hint: Option<Uuid>,
    carrier_name: String,
    vehicle_reference: String,
}
async fn dispatch(
    State(s): State<LogisticsRestState>,
    h: HeaderMap,
    Path(id): Path<Uuid>,
    Json(b): Json<DispatchBody>,
) -> Result<(StatusCode, Json<Value>), RestError> {
    let p = principal(&s, &h).await?;
    let branch = s
        .store
        .fulfillment_branch(id)
        .await
        .map_err(RestError::store)?;
    allow(&p, Feature::LogisticsDispatch, branch)?;
    Ok((
        StatusCode::CREATED,
        Json(
            s.store
                .dispatch(p.user_id, id, b.carrier_name, b.vehicle_reference)
                .await
                .map_err(RestError::store)?,
        ),
    ))
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PodBody {
    /// Legacy client hint; aggregate ownership is authoritative.
    #[serde(default, rename = "branchId")]
    _branch_hint: Option<Uuid>,
    recipient_name: String,
    evidence_reference: String,
    confirmed_at: OffsetDateTime,
}
/// Reject a malformed proof-of-delivery reference at the boundary.
///
/// Mirrors the `logistics_pod_evidence_evidence_reference_check` constraint
/// (migration 0212): an `evidence://` scheme followed by `[A-Za-z0-9._/-]`,
/// total length 19..=411. The database constraint remains authoritative and is
/// deliberately left in place as defence in depth; this only decides whether the
/// caller gets an actionable 400 instead of a 500 raised from the store.
fn validate_evidence_reference(reference: &str) -> Result<(), RestError> {
    const SCHEME: &str = "evidence://";
    let invalid = |detail: &str| {
        RestError::new(
            StatusCode::BAD_REQUEST,
            "invalid_evidence_reference",
            detail,
        )
    };
    let Some(suffix) = reference.strip_prefix(SCHEME) else {
        return Err(invalid(
            "evidenceReference must begin with the evidence:// scheme",
        ));
    };
    if suffix.is_empty()
        || !suffix
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '/' | '-'))
    {
        return Err(invalid(
            "evidenceReference may contain only letters, digits, '.', '_', '/' and '-' after the scheme",
        ));
    }
    // The class check above admits ASCII only, so bytes and characters agree by
    // here; `chars()` is the unit migration 0212's `char_length` counts.
    let len = reference.chars().count();
    if !(19..=411).contains(&len) {
        return Err(invalid(
            "evidenceReference must be between 19 and 411 characters including the evidence:// scheme",
        ));
    }
    Ok(())
}

async fn pod(
    State(s): State<LogisticsRestState>,
    h: HeaderMap,
    Path(id): Path<Uuid>,
    Json(b): Json<PodBody>,
) -> Result<Json<Value>, RestError> {
    let p = principal(&s, &h).await?;
    let branch = s
        .store
        .shipment_branch(id)
        .await
        .map_err(RestError::store)?;
    allow(&p, Feature::LogisticsPod, branch)?;
    // Validate only after authn/authz: an unauthenticated caller must not be able
    // to probe input validation, and must not learn a malformed body from a 400
    // where it should see 401.
    validate_evidence_reference(&b.evidence_reference)?;
    Ok(Json(
        s.store
            .pod(
                p.user_id,
                id,
                b.recipient_name,
                b.evidence_reference,
                b.confirmed_at,
            )
            .await
            .map_err(RestError::store)?,
    ))
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SettleBody {
    /// Legacy client hint; aggregate ownership is authoritative.
    #[serde(default, rename = "branchId")]
    _branch_hint: Option<Uuid>,
    currency_code: String,
    amount_minor: i64,
    settled_at: OffsetDateTime,
}
async fn settle(
    State(s): State<LogisticsRestState>,
    h: HeaderMap,
    Path(id): Path<Uuid>,
    Json(b): Json<SettleBody>,
) -> Result<Json<Value>, RestError> {
    let p = principal(&s, &h).await?;
    let branch = s
        .store
        .shipment_branch(id)
        .await
        .map_err(RestError::store)?;
    allow(&p, Feature::LogisticsSettle, branch)?;
    Ok(Json(
        s.store
            .settle(p.user_id, id, b.amount_minor, b.currency_code, b.settled_at)
            .await
            .map_err(RestError::store)?,
    ))
}
async fn principal(s: &LogisticsRestState, h: &HeaderMap) -> Result<Principal, RestError> {
    let verifier = s.jwt.as_ref().ok_or_else(|| {
        RestError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "unavailable",
            "JWT verification is not configured",
        )
    })?;
    console_platform_request_context::resolve_principal(verifier, s.store.pool(), h)
        .await
        .map_err(|e| match e {
            RequestContextError::MissingBearer
            | RequestContextError::InvalidToken
            | RequestContextError::InvalidClaim(_) => RestError::new(
                StatusCode::UNAUTHORIZED,
                "unauthorized",
                "missing, malformed, or invalid bearer token",
            ),
            RequestContextError::WrongTokenTier | RequestContextError::AccessScope(_) => {
                RestError::kernel(KernelError::forbidden(
                    "token is not authorized for logistics",
                ))
            }
            RequestContextError::VerifierUnavailable => RestError::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "JWT verification is not configured",
            ),
            RequestContextError::BranchScope(m) | RequestContextError::EffectivePolicy(m) => {
                RestError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal", m)
            }
            RequestContextError::MissingOrg => RestError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal",
                "no tenant context is bound",
            ),
        })
}
fn allow(p: &Principal, f: Feature, b: BranchId) -> Result<(), RestError> {
    let a = Action::new(f);
    match p.branch_scope {
        BranchScope::All => authorize_org_wide(p, a),
        _ => authorize(p, a, b),
    }
    .map_err(RestError::kernel)
}
fn idem_header(h: &HeaderMap) -> Result<String, RestError> {
    h.get("Idempotency-Key")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned)
        .ok_or_else(|| {
            RestError::kernel(KernelError::validation(
                "Idempotency-Key header is required",
            ))
        })
}
struct RestError {
    status: StatusCode,
    code: &'static str,
    message: String,
}
impl RestError {
    fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
        }
    }
    fn kernel(e: KernelError) -> Self {
        match e.kind {
            ErrorKind::Validation => {
                Self::new(StatusCode::UNPROCESSABLE_ENTITY, "validation", e.message)
            }
            ErrorKind::NotFound => Self::new(StatusCode::NOT_FOUND, "not_found", e.message),
            ErrorKind::Forbidden => Self::new(StatusCode::FORBIDDEN, "forbidden", e.message),
            ErrorKind::Conflict | ErrorKind::InvalidTransition => {
                Self::new(StatusCode::CONFLICT, "conflict", e.message)
            }
            ErrorKind::Internal => {
                Self::new(StatusCode::INTERNAL_SERVER_ERROR, "internal", e.message)
            }
        }
    }
    fn store(e: PgLogisticsError) -> Self {
        match e {
            PgLogisticsError::Domain(k) => Self::kernel(k),
            PgLogisticsError::Db(_) => Self::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal",
                "internal server error",
            ),
        }
    }
}
impl IntoResponse for RestError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(json!({"error":{"code":self.code,"message":self.message}})),
        )
            .into_response()
    }
}

/// The published `evidenceReference` schema must be the rule the server enforces.
///
/// A client reads `openapi.yaml`; the server runs [`validate_evidence_reference`].
/// When the published schema is WEAKER, a client builds a reference its own
/// contract accepts and gets a 400 it had no way to predict. So this is not an
/// "the field exists" assertion: it reads the rule OUT OF the published document,
/// evaluates it, and compares the verdict against the validator on concrete
/// references.
///
/// Nothing here restates 19, 411 or the character class. The published side is
/// read from the YAML; the enforced side (scheme, length window) is DISCOVERED
/// by probing the validator. The agreement corpus is built from the enforced
/// scheme, not from the published pattern's literal prefix — otherwise a
/// stricter published prefix moves the corpus with the document it is meant to
/// audit and prefix drift can agree with itself (console-5yn).
///
/// This is a WIRING claim about the deployed artifact, not a mechanism claim
/// about a fixture: `backend/openapi/openapi.yaml` is `include_str!`-ed here and
/// is the same file `console_app` `include_str!`s and serves at
/// `/openapi/openapi.yaml`. There is no second copy that could agree while the
/// served one drifts.
#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
mod published_evidence_reference_contract {
    use super::validate_evidence_reference;

    const OPENAPI_YAML: &str = include_str!("../../../../openapi/openapi.yaml");

    /// Upper bound of the length probe. This is a search limit, not the rule: a
    /// window that does not close below it FAILS rather than reporting a bound.
    const PROBE_CEILING: usize = 2048;

    // -- what the document publishes -----------------------------------------

    /// Index of the `}` that closes a YAML flow mapping whose opening `{` was
    /// already consumed. Braces inside single-quoted scalars do not count, so a
    /// published `pattern` that uses an ECMA `{n,m}` quantifier does not truncate
    /// the property mid-string.
    fn flow_mapping_end(body: &str) -> usize {
        let mut depth = 1_usize;
        let mut chars = body.char_indices().peekable();
        let mut in_single = false;
        while let Some((i, c)) = chars.next() {
            if in_single {
                if c == '\'' {
                    // YAML single-quoted escape: '' → one quote, still inside.
                    if chars.peek().is_some_and(|(_, n)| *n == '\'') {
                        chars.next();
                    } else {
                        in_single = false;
                    }
                }
                continue;
            }
            match c {
                '\'' => in_single = true,
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        return i;
                    }
                }
                _ => {}
            }
        }
        panic!("the evidenceReference schema object is unterminated");
    }

    /// True when `{` or `}` appears outside single-quoted scalars — nested flow
    /// objects this reader does not interpret as property fields.
    fn has_unquoted_brace(property: &str) -> bool {
        let mut chars = property.chars().peekable();
        let mut in_single = false;
        while let Some(c) = chars.next() {
            if in_single {
                if c == '\'' {
                    if chars.peek() == Some(&'\'') {
                        chars.next();
                    } else {
                        in_single = false;
                    }
                }
                continue;
            }
            match c {
                '\'' => in_single = true,
                '{' | '}' => return true,
                _ => {}
            }
        }
        false
    }

    /// The `evidenceReference` property object published for `verifyLogisticsPod`.
    ///
    /// Every lookup failure is a panic rather than a `None`: finding nothing here
    /// means this test examined zero subjects, and a control that examined zero
    /// subjects must never report green.
    fn published_property() -> &'static str {
        // The trailing newline makes this the WHOLE operation id. Without it,
        // `verifyLogisticsPodDraft` published earlier in the file would be read
        // instead, and its schema would be compared in place of this one's.
        const OPERATION: &str = "operationId: verifyLogisticsPod\n";
        let mut hits = OPENAPI_YAML.match_indices(OPERATION);
        let (at, _) = hits
            .next()
            .expect("openapi.yaml publishes no verifyLogisticsPod operation");
        assert!(
            hits.next().is_none(),
            "openapi.yaml publishes verifyLogisticsPod more than once; this test would \
             read only the first"
        );
        // Confine the read to this operation, so a neighbouring operation's
        // evidenceReference can never be mistaken for this one's.
        let after = &OPENAPI_YAML[at + OPERATION.len()..];
        let block = &after[..after.find("operationId: ").unwrap_or(after.len())];
        const KEY: &str = "evidenceReference: {";
        let at = block
            .find(KEY)
            .expect("verifyLogisticsPod publishes no evidenceReference schema object");
        let body = &block[at + KEY.len()..];
        let end = flow_mapping_end(body);
        let property = &body[..end];
        assert!(
            !has_unquoted_brace(property),
            "the evidenceReference schema nests objects this reader does not interpret: {property}"
        );
        property
    }

    /// One value of a flow-style `key: '<value>'`.
    fn quoted(property: &'static str, key: &str) -> Option<&'static str> {
        let at = property.find(&format!("{key}: '"))? + key.len() + 3;
        let rest = &property[at..];
        Some(&rest[..rest.find('\'')?])
    }

    /// One value of a flow-style `key: <digits>`.
    fn number(property: &str, key: &str) -> Option<usize> {
        let at = property.find(&format!("{key}: "))? + key.len() + 2;
        property[at..]
            .chars()
            .take_while(char::is_ascii_digit)
            .collect::<String>()
            .parse()
            .ok()
    }

    // -- evaluating the published pattern -------------------------------------

    /// `(literal prefix, character class)` of `^<literal>` or `^<literal>[<class>]+$`.
    ///
    /// A pattern outside that shape panics rather than being skipped: a published
    /// rule this test cannot evaluate is a rule nothing compares against.
    fn parse_pattern(pattern: &str) -> (&str, Option<&str>) {
        const NON_LITERAL: [char; 7] = ['\\', '(', ')', '|', '*', '?', '+'];
        let body = pattern
            .strip_prefix('^')
            .expect("the published pattern is not anchored at the start");
        let Some((literal, tail)) = body.split_once('[') else {
            assert!(
                !body.contains(NON_LITERAL) && !body.contains('$'),
                "published pattern {pattern} is outside the subset this test evaluates"
            );
            return (body, None);
        };
        assert!(
            !literal.contains(NON_LITERAL),
            "published pattern {pattern} has a non-literal prefix this test does not evaluate"
        );
        let (class, anchor) = tail
            .split_once(']')
            .expect("the published pattern has an unterminated character class");
        assert_eq!(
            anchor, "+$",
            "published pattern {pattern} is outside the subset this test evaluates ([class]+$)"
        );
        (literal, Some(class))
    }

    /// Membership in an OpenAPI character class, `a-z` ranges expanded. A `-` with
    /// no member after it is itself, which is exactly how this class ends. A `\`
    /// escapes the member after it, so `[..._/\-]` and `[..._/-]` — the same class,
    /// and the two spellings tooling emits for a trailing hyphen — read alike.
    fn class_contains(class: &str, c: char) -> bool {
        let members: Vec<char> = class.chars().collect();
        let mut at = 0;
        while at < members.len() {
            if members[at] == '\\' {
                if members.get(at + 1) == Some(&c) {
                    return true;
                }
                at += 2;
            } else if at + 2 < members.len() && members[at + 1] == '-' {
                if (members[at]..=members[at + 2]).contains(&c) {
                    return true;
                }
                at += 3;
            } else {
                if members[at] == c {
                    return true;
                }
                at += 1;
            }
        }
        false
    }

    /// Characters immediately outside each published range or singleton.
    ///
    /// Derived from the class text, so a widened edge (e.g. `A-Z` → `A-[`) is
    /// exercised without restating the member list. Distant widenings still need
    /// the non-ASCII probes in [`corpus`].
    fn class_boundary_reps(class: &str) -> Vec<char> {
        let members: Vec<char> = class.chars().collect();
        let mut out = Vec::new();
        let mut push_adjacent = |c: char| {
            if let Some(before) = char::from_u32(u32::from(c).wrapping_sub(1)) {
                out.push(before);
            }
            if let Some(after) = char::from_u32(u32::from(c) + 1) {
                out.push(after);
            }
        };
        let mut at = 0;
        while at < members.len() {
            if members[at] == '\\' {
                if let Some(&escaped) = members.get(at + 1) {
                    push_adjacent(escaped);
                }
                at += 2;
            } else if at + 2 < members.len() && members[at + 1] == '-' {
                push_adjacent(members[at]);
                push_adjacent(members[at + 2]);
                at += 3;
            } else {
                push_adjacent(members[at]);
                at += 1;
            }
        }
        out
    }

    fn matches_pattern(pattern: &str, candidate: &str) -> bool {
        let (literal, class) = parse_pattern(pattern);
        let Some(suffix) = candidate.strip_prefix(literal) else {
            return false;
        };
        match class {
            // JSON Schema `pattern` is an unanchored SEARCH and `^` anchors only
            // the start, so a pattern that is nothing but `^<literal>` constrains
            // NOTHING past the prefix. Treating it as if it did is what would let
            // a weak published pattern pass this test.
            None => true,
            Some(class) => !suffix.is_empty() && suffix.chars().all(|c| class_contains(class, c)),
        }
    }

    /// The constraint the document publishes, read out of the document.
    struct Published {
        pattern: &'static str,
        min_len: Option<usize>,
        max_len: Option<usize>,
    }

    impl Published {
        fn read() -> Self {
            let property = published_property();
            Self {
                pattern: quoted(property, "pattern")
                    .expect("evidenceReference publishes no pattern"),
                min_len: number(property, "minLength"),
                max_len: number(property, "maxLength"),
            }
        }

        /// `minLength`/`maxLength` count CHARACTERS of the WHOLE string, scheme
        /// included. Publishing the suffix window instead would advertise a
        /// contract that rejects references the server accepts.
        fn accepts(&self, candidate: &str) -> bool {
            let len = candidate.chars().count();
            !self.min_len.is_some_and(|min| len < min)
                && !self.max_len.is_some_and(|max| len > max)
                && matches_pattern(self.pattern, candidate)
        }
    }

    // -- what the server enforces ---------------------------------------------

    /// An `n`-character reference under `scheme`, padded with a character any
    /// plausible class admits.
    fn reference_of_len(scheme: &str, n: usize) -> String {
        format!("{scheme}{}", "a".repeat(n - scheme.chars().count()))
    }

    /// One reference the validator accepts, found by probing under `seed_prefix`.
    ///
    /// `seed_prefix` is only a search hint (typically the published literal). The
    /// corpus must not keep using it: [`enforced_scheme`] re-derives the required
    /// prefix from the accepted seed so a stricter published prefix cannot drag
    /// the agreement set with it.
    fn accepted_under(seed_prefix: &str) -> String {
        let floor = seed_prefix.chars().count() + 1;
        for n in floor..=PROBE_CEILING {
            let candidate = reference_of_len(seed_prefix, n);
            if validate_evidence_reference(&candidate).is_ok() {
                return candidate;
            }
        }
        panic!(
            "the validator accepts no reference under search prefix {seed_prefix:?}: \
             this test examined zero subjects and must not pass"
        )
    }

    /// The literal prefix the validator requires, discovered by asking which
    /// leading characters of an accepted reference cannot vary.
    ///
    /// Walking left to right: position `i` is part of the scheme while every
    /// alternate ASCII byte there is rejected at the seed's length; the first
    /// position that admits another character is the start of the flexible
    /// suffix. No bound or scheme spelling is typed here.
    fn enforced_scheme(accepted: &str) -> String {
        let chars: Vec<char> = accepted.chars().collect();
        let n = chars.len();
        let mut end = 0_usize;
        for i in 0..chars.len() {
            let head: String = chars[..i].iter().collect();
            let can_vary = (0_u8..=0x7f).map(char::from).any(|alt| {
                if alt == chars[i] {
                    return false;
                }
                let mut candidate = head.clone();
                candidate.push(alt);
                while candidate.chars().count() < n {
                    candidate.push('a');
                }
                validate_evidence_reference(&candidate).is_ok()
            });
            if can_vary {
                break;
            }
            end = i + 1;
        }
        assert!(
            end > 0,
            "the validator accepts {accepted:?} but requires no literal prefix; \
             the corpus would not be anchored"
        );
        chars[..end].iter().collect()
    }

    /// The length window `validate_evidence_reference` actually enforces,
    /// discovered by asking it rather than by restating its bounds here.
    fn enforced_window(scheme: &str) -> (usize, usize) {
        let accepted: Vec<usize> = (scheme.chars().count()..=PROBE_CEILING)
            .filter(|&n| validate_evidence_reference(&reference_of_len(scheme, n)).is_ok())
            .collect();
        let (Some(&min), Some(&max)) = (accepted.first(), accepted.last()) else {
            panic!(
                "the validator accepts no reference at all under the enforced scheme \
                 {scheme:?}: this test examined zero subjects and must not pass"
            )
        };
        assert_eq!(
            accepted.len(),
            max - min + 1,
            "the enforced length window {min}..={max} has holes; this test assumes one window"
        );
        assert!(
            max < PROBE_CEILING,
            "the enforced length window does not close below {PROBE_CEILING}"
        );
        (min, max)
    }

    // -- the agreement ---------------------------------------------------------

    /// References the published schema and the validator must classify the SAME
    /// way. Every candidate is derived from the ENFORCED scheme and the probed
    /// window, so no bound is typed here and a published prefix that merely
    /// extends the enforced one cannot hide inside a corpus that moved with it.
    fn corpus(scheme: &str, class: Option<&str>, min: usize, max: usize) -> Vec<(String, String)> {
        let mut out: Vec<(String, String)> = [
            (min - 1, "one character under the enforced floor"),
            (min, "the enforced floor exactly"),
            (max, "the enforced ceiling exactly"),
            (max + 1, "one character over the enforced ceiling"),
        ]
        .into_iter()
        .map(|(n, why)| {
            (
                reference_of_len(scheme, n),
                format!("{why} ({n} characters)"),
            )
        })
        .collect();

        // The class dimension, SWEPT rather than sampled, at a length the window
        // accepts so each verdict turns on the class alone. Latin-1 is enumerated
        // totally; boundary representatives are derived from the published class
        // (char immediately before/after each range endpoint or singleton); and a
        // fixed non-ASCII set catches widenings that stay far from those edges
        // (the reviewer's `中` example). Full Unicode enumeration remains out of
        // scope for this unit test — tracked under console-ann rather than as a
        // third spelling of the class here.
        let mut probes: Vec<char> = (0_u8..=0xff).map(char::from).collect();
        if let Some(class) = class {
            probes.extend(class_boundary_reps(class));
        }
        probes.extend(['각', 'é', '中', '\u{200b}', '\u{ff01}']);
        probes.sort_unstable();
        probes.dedup();
        for c in probes {
            let mut candidate = reference_of_len(scheme, min);
            candidate.pop();
            candidate.push(c);
            out.push((
                candidate,
                format!("{c:?} in the suffix, at the floor length"),
            ));
        }

        // Schemes. The third is legal — a leading `/` is inside the class — and is
        // here so the corpus is not classified all one way.
        for other in ["https://", "evidence:/", &format!("{scheme}/")] {
            out.push((
                reference_of_len(other, min),
                format!("scheme {other:?} at the floor length"),
            ));
        }
        out
    }

    #[test]
    fn flow_mapping_end_tolerates_ecma_quantifier_inside_quoted_pattern() {
        // BODY starts after the opening `{` of the property object.
        let body = " type: string, pattern: '^evidence://[A-Za-z0-9._/-]{1,400}$', minLength: 19 }";
        let end = flow_mapping_end(body);
        assert_eq!(&body[end..], "}");
        let property = &body[..end];
        assert!(
            property.contains("{1,400}"),
            "quantifier must survive the read: {property}"
        );
        assert!(
            !has_unquoted_brace(property),
            "quantifier braces are quoted: {property}"
        );
    }

    #[test]
    fn published_schema_and_validator_agree_on_concrete_references() {
        let published = Published::read();
        let (published_literal, class) = parse_pattern(published.pattern);
        // Bootstrap only: find one accepted reference near the published literal,
        // then discover the enforced scheme from that seed. The corpus uses the
        // discovered scheme — never the published literal — so prefix drift fails.
        let seed = accepted_under(published_literal);
        let scheme = enforced_scheme(&seed);
        let (min, max) = enforced_window(&scheme);

        let (mut accepted, mut rejected) = (0_usize, 0_usize);
        for (candidate, why) in corpus(&scheme, class, min, max) {
            let enforced = validate_evidence_reference(&candidate).is_ok();
            if enforced {
                accepted += 1;
            } else {
                rejected += 1;
            }
            assert_eq!(
                published.accepts(&candidate),
                enforced,
                "backend/openapi/openapi.yaml and validate_evidence_reference disagree on \
                 {why}: published {{pattern: {:?}, minLength: {:?}, maxLength: {:?}}} \
                 accepts={}, the server accepts={enforced}. Candidate: {candidate:?}. \
                 enforced scheme: {scheme:?}",
                published.pattern,
                published.min_len,
                published.max_len,
                published.accepts(&candidate),
            );
        }
        assert!(
            accepted > 0 && rejected > 0,
            "a corpus the validator classifies all one way agrees with anything: \
             {accepted} accepted, {rejected} rejected"
        );
    }
}
