//! Cedar/PBAC enrollment wave 2 — shared observe-only parity shadow + report.
//!
//! This is the direct generalization of the RoleManage shadow lane (#182,
//! `identity/rest`) to two new surfaces: the workflow task decide/claim/finalize
//! guard and the object-resolve read path. It exists to MEASURE, per tenant,
//! whether Cedar — evaluated as if it were the sole enforcer — would reach the
//! SAME allow/deny as the legacy matrix that actually enforces today. That
//! recorded agree/disagree stream is the promotion evidence artifact
//! (`console-cedar-parity-report`).
//!
//! ## Load-bearing safety invariant (ADR-0021)
//! The legacy decision is the SOLE enforcer and is computed + enforced by the
//! call site BEFORE [`observe_parity`] runs. This lane is best-effort and
//! side-effect-only: every error, deny, or panic anywhere inside it is swallowed
//! (via [`std::panic::catch_unwind`]) and can NEVER change the live outcome, abort
//! the request, or leave broken state. Its only effect is one append-only audit
//! row when the per-tenant DARK flag is enabled. Production ships zero enabled
//! flag rows, so the whole lane is inert (a single flag read, then nothing).
//!
//! The shadow entry is pinned to [`DualEngineMode::CedarOnly`] on purpose: that
//! arm returns Cedar's OWN verdict (preconditions + policy) without folding in a
//! legacy consult, so the recorded `shadow_effect` is a clean "what would Cedar
//! decide alone", which we compare against the enforced `legacy_effect` passed in
//! by the caller. Using it here is safe precisely because the result is recorded,
//! never returned to gate anything.

use std::collections::BTreeMap;
use std::panic::AssertUnwindSafe;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use console_identity_adapter_postgres::PgOrgStore;
use console_kernel_core::{AuditAction, AuditEvent, OrgId, TraceContext, UserId};
use console_platform_authz::cedar_pbac::engine;
use console_platform_authz::{
    Action, AuthorizationRequest, AuthorizationResource, CoexistenceMapEntry, DecisionEffect,
    DualEngineMode, Feature, Principal, RlsScopeProof, SubjectFreshnessRequirement,
    evaluate_cedar_pbac_boundary,
};
use console_platform_db::{DbError, with_audit};
use futures::FutureExt;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use time::OffsetDateTime;

/// Per-tenant DARK switch for the workflow decide/claim/finalize parity shadow.
/// Absent row ⇒ FALSE ⇒ the lane never runs. Production ships zero enabled rows.
pub const CEDAR_PBAC_SHADOW_WORKFLOW_DECIDE_FLAG: &str = "cedar_pbac_shadow_workflow_decide";

/// Per-tenant DARK switch for the object-resolve read-surface parity shadow.
pub const CEDAR_PBAC_SHADOW_OBJECT_RESOLVE_FLAG: &str = "cedar_pbac_shadow_object_resolve";

/// Append-only audit action carrying one recorded parity observation. Distinct
/// from the inert `workflow_runtime.cedar_shadow` guard event so the report reads
/// only real Cedar-vs-legacy comparisons.
pub const CEDAR_PBAC_PARITY_AUDIT_ACTION: &str = "authz.cedar_pbac_parity";

/// Policy domain recorded on a workflow decide-path parity observation.
pub const WORKFLOW_DECIDE_DOMAIN: &str = "workflow.decide";

/// Policy domain recorded on an object-resolve parity observation.
pub const OBJECT_RESOLVE_DOMAIN: &str = "object.resolve";

/// The self-contained `after_snap` payload of one parity observation. The report
/// aggregates from `audit_events` alone, so everything it needs lives here — and
/// nothing beyond ids/roles does (no PII).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ParityObservation {
    pub domain: String,
    /// The Cedar action id == [`Feature::as_str`].
    pub action: String,
    pub resource_kind: String,
    /// Server-derived principal roles (canonical codes), for divergence triage.
    pub principal_roles: Vec<String>,
    pub legacy_effect: DecisionEffect,
    pub shadow_effect: DecisionEffect,
    /// Machine-readable Cedar-pipeline reason (e.g. `cedar_denied`, `stale_subject`).
    pub shadow_reason: String,
    pub divergent: bool,
}

/// Run the audit-only Cedar parity observation for one already-enforced legacy
/// decision. Never fails the live request: any error/panic is caught + logged.
///
/// `legacy_allowed` is the enforced legacy verdict the caller already computed
/// and acted on; this lane only records how a Cedar-only evaluation compares.
#[allow(clippy::too_many_arguments)]
pub async fn observe_parity(
    pool: &PgPool,
    principal: &Principal,
    org: OrgId,
    feature: Feature,
    resource: AuthorizationResource,
    domain: &'static str,
    flag_key: &'static str,
    legacy_allowed: bool,
) {
    // Panic-isolate the ENTIRE lane on the current task (task-local CURRENT_ORG
    // preserved for the store reads). AssertUnwindSafe is sound: the lane holds no
    // locks and its only mutation is an audit txn that rolls back on panic.
    let outcome = AssertUnwindSafe(try_observe_parity(
        pool,
        principal,
        org,
        feature,
        resource,
        domain,
        flag_key,
        legacy_allowed,
    ))
    .catch_unwind()
    .await;
    match outcome {
        Ok(Ok(())) => {}
        Ok(Err(err)) => tracing::warn!(
            event = "cedar_pbac_parity_error",
            error = %err,
            "cedar/pbac parity shadow failed (audit-only; live decision unaffected)"
        ),
        Err(_panic) => tracing::warn!(
            event = "cedar_pbac_parity_error",
            "cedar/pbac parity shadow panicked (audit-only; live decision unaffected)"
        ),
    }
}

#[derive(Debug, Clone, Hash, PartialEq, Eq, PartialOrd, Ord)]
struct RuntimeFlagCacheKey {
    database: String,
    org: String,
    flag: &'static str,
}

static RUNTIME_FLAG_CACHE: OnceLock<Mutex<BTreeMap<RuntimeFlagCacheKey, (bool, Instant)>>> =
    OnceLock::new();
const RUNTIME_FLAG_CACHE_TTL: Duration = Duration::from_secs(30);

async fn runtime_flag_enabled_cached(
    pool: &PgPool,
    store: &PgOrgStore,
    org: OrgId,
    flag_key: &'static str,
) -> Result<bool, String> {
    let key = RuntimeFlagCacheKey {
        // Stable across cloned pools/State instances that point at the same DB,
        // but distinct for isolated sqlx::test databases with the same org ids.
        database: pool
            .connect_options()
            .as_ref()
            .get_database()
            .unwrap_or("")
            .to_owned(),
        org: org.as_uuid().to_string(),
        flag: flag_key,
    };
    let now = Instant::now();
    let cached = {
        let cache = RUNTIME_FLAG_CACHE
            .get_or_init(|| Mutex::new(BTreeMap::new()))
            .lock()
            .map_err(|_err| "runtime flag cache mutex poisoned".to_string())?;
        cache.get(&key).copied()
    };
    if let Some((enabled, _cached_at)) =
        cached.filter(|(_, cached_at)| now.duration_since(*cached_at) < RUNTIME_FLAG_CACHE_TTL)
    {
        return Ok(enabled);
    }

    let enabled = store
        .org_runtime_flag_enabled(flag_key)
        .await
        .map_err(|err| format!("flag read failed: {err:?}"))?;
    RUNTIME_FLAG_CACHE
        .get_or_init(|| Mutex::new(BTreeMap::new()))
        .lock()
        .map_err(|_err| "runtime flag cache mutex poisoned".to_string())?
        .insert(key, (enabled, now));
    Ok(enabled)
}

#[allow(clippy::too_many_arguments)]
async fn try_observe_parity(
    pool: &PgPool,
    principal: &Principal,
    org: OrgId,
    feature: Feature,
    resource: AuthorizationResource,
    domain: &'static str,
    flag_key: &'static str,
    legacy_allowed: bool,
) -> Result<(), String> {
    let store = PgOrgStore::new(pool.clone());
    // DARK switch: absent/false flag ⇒ do nothing. Cache the short-lived
    // per-process result so hot paths do not pay a DB flag read on every request
    // while preserving per-tenant DB control once the cache expires.
    if !runtime_flag_enabled_cached(pool, &store, org, flag_key).await? {
        return Ok(());
    }

    // DB-current freshness the token snapshot must be at least as fresh as, read
    // under the armed console_rt GUC.
    let policy_version = store
        .get_policy_version()
        .await
        .map_err(|err| format!("policy version read failed: {err:?}"))?
        .version;
    let (subject_version, session_generation) = store
        .get_subject_authz_versions(principal.user_id)
        .await
        .map_err(|err| format!("subject version read failed: {err:?}"))?;

    let policy_version = u64::try_from(policy_version).unwrap_or(0);

    let bundle = engine::compile_bundle_for_feature(org, policy_version, feature)
        .map_err(|err| format!("bundle compile failed: {}", err.message))?;

    // CedarOnly shadow entry: yields Cedar's own verdict without a legacy consult.
    let entry = CoexistenceMapEntry::new(
        format!("{domain}.{}", feature.as_str()),
        domain,
        feature,
        resource.resource_type.clone(),
        DualEngineMode::CedarOnly,
        Some(bundle.key.clone()),
    );

    let request =
        AuthorizationRequest::new(principal.clone(), Action::new(feature), resource.clone())
            .with_policy_domain(domain)
            .with_subject_freshness(principal.authz_freshness)
            .requiring_freshness(SubjectFreshnessRequirement {
                min_policy_version: policy_version,
                min_subject_version: u64::try_from(subject_version).unwrap_or(0),
                min_session_generation: u64::try_from(session_generation).unwrap_or(0),
                required_step_up_generation: None,
            })
            .with_rls_scope_proof(RlsScopeProof::runtime_role_guc(org));

    // Real Cedar evaluation (Result + catch_unwind guarded — cannot throw).
    let cedar = engine::evaluate(&request, &bundle);
    let shadow = evaluate_cedar_pbac_boundary(&request, Some(&entry), cedar);

    let legacy_effect = if legacy_allowed {
        DecisionEffect::Allow
    } else {
        DecisionEffect::Deny
    };
    let observation = ParityObservation {
        domain: domain.to_owned(),
        action: feature.as_str().to_owned(),
        resource_kind: resource.resource_type.clone(),
        principal_roles: principal
            .roles
            .iter()
            .map(|role| role.as_str().to_owned())
            .collect(),
        legacy_effect,
        shadow_effect: shadow.effect,
        shadow_reason: reason_label(&shadow.reason),
        divergent: shadow.effect != legacy_effect,
    };

    persist(
        pool,
        org,
        principal.user_id,
        resource.resource_id.as_deref(),
        &observation,
    )
    .await
}

/// Render a serde snake_case enum (e.g. [`console_platform_authz::DecisionReason`]) to
/// its string label.
fn reason_label(value: &impl Serialize) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_else(|| "unknown".to_owned())
}

/// Persist one parity observation as an append-only audit row under armed RLS.
async fn persist(
    pool: &PgPool,
    org: OrgId,
    actor: UserId,
    resource_id: Option<&str>,
    observation: &ParityObservation,
) -> Result<(), String> {
    let payload = serde_json::to_value(observation)
        .map_err(|err| format!("parity payload serialize failed: {err}"))?;
    let action = AuditAction::new(CEDAR_PBAC_PARITY_AUDIT_ACTION)
        .map_err(|err| format!("parity audit action invalid: {}", err.message))?;
    let target_id = resource_id
        .filter(|id| !id.trim().is_empty())
        .unwrap_or(observation.resource_kind.as_str())
        .to_owned();
    let event = AuditEvent::new(
        Some(actor),
        action,
        "cedar_parity",
        target_id,
        TraceContext::generate(),
        OffsetDateTime::now_utc(),
    )
    .with_org(org)
    .with_snapshots(None, Some(payload));

    with_audit::<_, (), DbError>(pool, event, |_tx| Box::pin(async move { Ok(()) }))
        .await
        .map_err(|err| format!("parity audit write failed: {err:?}"))
}

// ---------------------------------------------------------------------------
// Report aggregation (consumed by `bin/console-cedar-parity-report`).
// ---------------------------------------------------------------------------

/// One concrete divergent case, deduped by (action, kind, roles, effects,
/// reason) with an occurrence count. No PII beyond role codes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Divergence {
    pub domain: String,
    pub action: String,
    pub resource_kind: String,
    pub principal_roles: Vec<String>,
    pub legacy_effect: DecisionEffect,
    pub shadow_effect: DecisionEffect,
    pub shadow_reason: String,
    pub count: u64,
}

/// Per-site (per-org) parity totals plus its concrete divergent cases.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SiteParity {
    pub total: u64,
    pub agree: u64,
    pub disagree: u64,
    pub divergences: Vec<Divergence>,
    /// True once this site has recorded observations and NONE diverged — the
    /// per-site promotion signal.
    pub clean: bool,
}

/// The full parity report: per-site rollups keyed by org id.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ParityReport {
    pub per_site: BTreeMap<String, SiteParity>,
    pub total: u64,
    pub disagree: u64,
}

impl ParityReport {
    /// Whether every site with observations recorded zero divergences.
    #[must_use]
    pub fn all_sites_clean(&self) -> bool {
        self.disagree == 0 && self.total > 0
    }
}

/// Aggregate recorded observations (site id, observation) into the report. Pure:
/// no DB, so it is the load-bearing logic the report test pins.
#[must_use]
pub fn aggregate(rows: impl IntoIterator<Item = (String, ParityObservation)>) -> ParityReport {
    // Divergences deduped within a site by their identifying key.
    let mut divergence_index: BTreeMap<String, BTreeMap<String, Divergence>> = BTreeMap::new();
    let mut report = ParityReport::default();

    for (site, obs) in rows {
        report.total += 1;
        let site_entry = report.per_site.entry(site.clone()).or_default();
        site_entry.total += 1;
        if obs.divergent {
            report.disagree += 1;
            site_entry.disagree += 1;
            let key = format!(
                "{}|{}|{}|{}|{:?}|{:?}|{}",
                obs.domain,
                obs.action,
                obs.resource_kind,
                obs.principal_roles.join(","),
                obs.legacy_effect,
                obs.shadow_effect,
                obs.shadow_reason,
            );
            let bucket = divergence_index.entry(site).or_default();
            bucket
                .entry(key)
                .and_modify(|d| d.count += 1)
                .or_insert(Divergence {
                    domain: obs.domain,
                    action: obs.action,
                    resource_kind: obs.resource_kind,
                    principal_roles: obs.principal_roles,
                    legacy_effect: obs.legacy_effect,
                    shadow_effect: obs.shadow_effect,
                    shadow_reason: obs.shadow_reason,
                    count: 1,
                });
        } else {
            site_entry.agree += 1;
        }
    }

    for (site, buckets) in divergence_index {
        if let Some(site_entry) = report.per_site.get_mut(&site) {
            site_entry.divergences = buckets.into_values().collect();
        }
    }
    for site_entry in report.per_site.values_mut() {
        site_entry.clean = site_entry.total > 0 && site_entry.disagree == 0;
    }

    report
}

#[cfg(test)]
mod tests {
    use super::*;

    fn obs(divergent: bool, roles: &[&str]) -> ParityObservation {
        ParityObservation {
            domain: WORKFLOW_DECIDE_DOMAIN.to_owned(),
            action: "completion_review".to_owned(),
            resource_kind: "work_order".to_owned(),
            principal_roles: roles.iter().map(|r| (*r).to_owned()).collect(),
            legacy_effect: DecisionEffect::Allow,
            shadow_effect: if divergent {
                DecisionEffect::Deny
            } else {
                DecisionEffect::Allow
            },
            shadow_reason: if divergent {
                "stale_subject"
            } else {
                "cedar_allowed"
            }
            .to_owned(),
            divergent,
        }
    }

    #[cfg(not(feature = "test-postgres"))]
    #[test]
    fn report_surfaces_a_seeded_divergence_per_site() {
        let rows = vec![
            ("site-a".to_owned(), obs(false, &["ADMIN"])),
            ("site-a".to_owned(), obs(false, &["ADMIN"])),
            ("site-a".to_owned(), obs(true, &["ADMIN"])), // the seeded divergence
            ("site-b".to_owned(), obs(false, &["SUPER_ADMIN"])),
        ];

        let report = aggregate(rows);

        assert_eq!(report.total, 4);
        assert_eq!(report.disagree, 1);
        assert!(!report.all_sites_clean(), "site-a diverged");

        let a = &report.per_site["site-a"];
        assert_eq!(a.total, 3);
        assert_eq!(a.agree, 2);
        assert_eq!(a.disagree, 1);
        assert!(!a.clean);
        assert_eq!(a.divergences.len(), 1, "one distinct divergent case");
        let d = &a.divergences[0];
        assert_eq!(d.count, 1);
        assert_eq!(d.domain, WORKFLOW_DECIDE_DOMAIN);
        assert_eq!(d.action, "completion_review");
        assert_eq!(d.resource_kind, "work_order");
        assert_eq!(d.legacy_effect, DecisionEffect::Allow);
        assert_eq!(d.shadow_effect, DecisionEffect::Deny);
        assert_eq!(d.shadow_reason, "stale_subject");

        let b = &report.per_site["site-b"];
        assert!(b.clean, "site-b recorded only agreements → clean");
        assert!(b.divergences.is_empty());
    }

    #[cfg(not(feature = "test-postgres"))]
    #[test]
    fn identical_divergences_dedupe_with_count() {
        let rows = vec![
            ("s".to_owned(), obs(true, &["MECHANIC"])),
            ("s".to_owned(), obs(true, &["MECHANIC"])),
        ];
        let report = aggregate(rows);
        let s = &report.per_site["s"];
        assert_eq!(s.disagree, 2);
        assert_eq!(s.divergences.len(), 1);
        assert_eq!(s.divergences[0].count, 2);
    }

    #[cfg(not(feature = "test-postgres"))]
    #[test]
    fn divergences_from_different_domains_remain_distinct() {
        let mut object = obs(true, &["MECHANIC"]);
        object.domain = OBJECT_RESOLVE_DOMAIN.to_owned();
        let report = aggregate(vec![
            ("s".to_owned(), obs(true, &["MECHANIC"])),
            ("s".to_owned(), object),
        ]);
        let s = &report.per_site["s"];
        assert_eq!(s.disagree, 2);
        assert_eq!(s.divergences.len(), 2);
        assert!(
            s.divergences
                .iter()
                .any(|d| d.domain == WORKFLOW_DECIDE_DOMAIN)
        );
        assert!(
            s.divergences
                .iter()
                .any(|d| d.domain == OBJECT_RESOLVE_DOMAIN)
        );
    }
}
