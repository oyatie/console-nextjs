#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Cedar/PBAC readiness cases — table-driven binding.
//!
//! Each row of `tests/fixtures/cedar_pbac_readiness_cases.json` is bound to an
//! EXECUTABLE assertion against the already-existing boundary
//! (`evaluate_cedar_pbac_boundary`) and observation (`observe_cedar_pbac_decision`).
//! The boundary logic already exists (Cedar-activation slices 1–2); this slice
//! proves the nine fail-closed scenarios the spec fixture enumerates actually
//! produce the deny + engine + reason the fixture declares, that the metric
//! projection stays low-cardinality (`{effect,engine,reason,mode,domain}` only),
//! and that each case's declared `mustAudit` evidence is present on the audit
//! event (through the nested `bundle_key`/`evaluated_bundle_key` path — the audit
//! event is NOT weakened to match the fixture's flattened field names).
//!
//! Nothing here wires Cedar into a live path: every scenario feeds the boundary a
//! constructed request/entry/`CedarEvaluation`, and the bundle identities come
//! from the real slice-1 `engine::compile_bundle` output.

use std::collections::BTreeSet;

use console_kernel_core::{BranchId, BranchScope, OrgId, UserId};
use console_platform_authz::cedar_pbac::engine;
use console_platform_authz::{
    Action, AuthorizationContext, AuthorizationRequest, AuthorizationResource, CedarEvaluation,
    CoexistenceMapEntry, CompiledBundleCacheKey, DecisionEffect, DecisionEngine, DecisionReason,
    DualEngineMode, Feature, Principal, RlsScopeProof, Role, SubjectFreshness,
    SubjectFreshnessRequirement, evaluate_cedar_pbac_boundary, observe_cedar_pbac_decision,
};

const DOMAIN: &str = "identity.policy";
const RESOURCE_TYPE: &str = "identity.policy_role";

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReadinessFixture {
    cases: Vec<ReadinessCase>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReadinessCase {
    id: String,
    mode: Option<DualEngineMode>,
    expected_effect: DecisionEffect,
    expected_engine: DecisionEngine,
    expected_reason: DecisionReason,
    must_audit: Vec<String>,
}

/// Real slice-1 compiled-bundle identity at `policy_version`. Two versions give
/// two distinct cache keys (the stale-policy case compares them).
fn bundle_key(policy_version: u64) -> CompiledBundleCacheKey {
    engine::compile_bundle(OrgId::knl(), policy_version)
        .expect("slice-1 bundle must compile")
        .key
}

fn principal(role: Role, branch: BranchId) -> Principal {
    Principal::new(
        UserId::new(),
        OrgId::knl(),
        BTreeSet::from([role]),
        BranchScope::single(branch),
    )
}

fn fresh() -> SubjectFreshness {
    SubjectFreshness {
        policy_version: 7,
        subject_version: 11,
        session_generation: 3,
        step_up_generation: Some(2),
    }
}

fn required() -> SubjectFreshnessRequirement {
    SubjectFreshnessRequirement {
        min_policy_version: 7,
        min_subject_version: 11,
        min_session_generation: 3,
        required_step_up_generation: Some(2),
    }
}

fn identity_entry(
    feature: Feature,
    mode: DualEngineMode,
    bundle: Option<CompiledBundleCacheKey>,
) -> CoexistenceMapEntry {
    CoexistenceMapEntry::new(
        format!("identity.policy.{}", feature.as_str()),
        DOMAIN,
        feature,
        RESOURCE_TYPE,
        mode,
        bundle,
    )
}

/// A `role_manage` request against a same-org `identity.policy_role` resource,
/// tagged with the domain and a request id for the audit trail.
fn role_manage_request(role: Role, resource_org: OrgId) -> AuthorizationRequest {
    let branch = BranchId::new();
    let mut request = AuthorizationRequest::new(
        principal(role, branch),
        Action::new(Feature::RoleManage),
        AuthorizationResource::branch(resource_org, branch, RESOURCE_TYPE),
    )
    .with_policy_domain(DOMAIN);
    request.context = AuthorizationContext {
        purpose: Some("policy_admin".to_owned()),
        channel: Some("web".to_owned()),
        request_id: Some(format!("req-{}", role.as_str())),
    };
    request
}

/// Freshness + RLS-scope-proof armed: the shape a Cedar-enrolled request has once
/// slice-2 sourcing is live.
fn cedar_ready(request: AuthorizationRequest) -> AuthorizationRequest {
    request
        .with_subject_freshness(fresh())
        .requiring_freshness(required())
        .with_rls_scope_proof(RlsScopeProof::runtime_role_guc(OrgId::knl()))
}

fn audit_stream_entry(bundle: Option<CompiledBundleCacheKey>) -> CoexistenceMapEntry {
    CoexistenceMapEntry::new(
        "compliance.audit_stream.audit_stream_read",
        "compliance.audit_stream",
        Feature::AuditStreamRead,
        "audit_stream",
        DualEngineMode::CedarOnly,
        bundle,
    )
}

fn audit_stream_request(clearance_keys: BTreeSet<String>) -> AuthorizationRequest {
    let principal = Principal::new(
        UserId::new(),
        OrgId::knl(),
        BTreeSet::from([Role::SuperAdmin]),
        BranchScope::All,
    );
    AuthorizationRequest::new(
        principal,
        Action::new(Feature::AuditStreamRead),
        AuthorizationResource::org_wide(OrgId::knl(), "audit_stream")
            .with_resource_id(engine::CEO_COVERT_AUDIT_STREAM_KEY),
    )
    .with_policy_domain("compliance.audit_stream")
    .with_subject_freshness(SubjectFreshness {
        policy_version: 7,
        subject_version: 11,
        session_generation: 3,
        step_up_generation: None,
    })
    .requiring_freshness(SubjectFreshnessRequirement {
        min_policy_version: 7,
        min_subject_version: 11,
        min_session_generation: 3,
        required_step_up_generation: None,
    })
    .with_clearance_keys(clearance_keys)
    .with_rls_scope_proof(RlsScopeProof::runtime_role_guc(OrgId::knl()))
}

#[test]
fn ceo_covert_audit_stream_cedar_allows_only_with_clearance() {
    let bundle = engine::compile_audit_stream_bundle(OrgId::knl(), 7)
        .expect("audit stream bundle must compile");
    let entry = audit_stream_entry(Some(bundle.key.clone()));

    let allowed_request = audit_stream_request(BTreeSet::from([
        engine::CEO_COVERT_AUDIT_CLEARANCE_KEY.to_owned(),
    ]));
    let allowed = evaluate_cedar_pbac_boundary(
        &allowed_request,
        Some(&entry),
        engine::evaluate(&allowed_request, &bundle),
    );
    assert_eq!(allowed.effect, DecisionEffect::Allow);
    assert_eq!(allowed.reason, DecisionReason::CedarAllowed);

    let denied_request = audit_stream_request(BTreeSet::new());
    let denied = evaluate_cedar_pbac_boundary(
        &denied_request,
        Some(&entry),
        engine::evaluate(&denied_request, &bundle),
    );
    assert_eq!(denied.effect, DecisionEffect::Deny);
    assert_eq!(denied.reason, DecisionReason::CedarDenied);
}

struct Scenario {
    request: AuthorizationRequest,
    entry: Option<CoexistenceMapEntry>,
    cedar: CedarEvaluation,
    expected_domain: Option<String>,
    expected_mode: Option<DualEngineMode>,
}

/// Build the fault scenario for one fixture case id.
fn scenario_for(case_id: &str) -> Scenario {
    match case_id {
        // Cedar returns a bundle key that differs from the coexistence-map key.
        "stale_policy_denies" => Scenario {
            request: cedar_ready(role_manage_request(Role::SuperAdmin, OrgId::knl())),
            entry: Some(identity_entry(
                Feature::RoleManage,
                DualEngineMode::CedarOnly,
                Some(bundle_key(7)),
            )),
            cedar: CedarEvaluation::Allow {
                bundle_key: bundle_key(6),
            },
            expected_domain: Some(DOMAIN.to_owned()),
            expected_mode: Some(DualEngineMode::CedarOnly),
        },
        // Carried subject freshness is below the required minimum.
        "stale_subject_denies" => {
            let request = role_manage_request(Role::SuperAdmin, OrgId::knl())
                .with_subject_freshness(SubjectFreshness {
                    policy_version: 1,
                    subject_version: 1,
                    session_generation: 1,
                    step_up_generation: Some(1),
                })
                .requiring_freshness(SubjectFreshnessRequirement {
                    min_policy_version: 2,
                    min_subject_version: 1,
                    min_session_generation: 1,
                    required_step_up_generation: Some(1),
                });
            Scenario {
                request,
                entry: Some(identity_entry(
                    Feature::RoleManage,
                    DualEngineMode::LegacyOnly,
                    None,
                )),
                cedar: CedarEvaluation::NotConfigured,
                expected_domain: Some(DOMAIN.to_owned()),
                expected_mode: Some(DualEngineMode::LegacyOnly),
            }
        }
        // Subject org differs from the server-loaded resource org.
        "rls_separation_denies" => Scenario {
            request: cedar_ready(role_manage_request(Role::SuperAdmin, OrgId::platform())),
            entry: Some(identity_entry(
                Feature::RoleManage,
                DualEngineMode::CedarOnly,
                Some(bundle_key(7)),
            )),
            cedar: CedarEvaluation::Allow {
                bundle_key: bundle_key(7),
            },
            expected_domain: Some(DOMAIN.to_owned()),
            expected_mode: Some(DualEngineMode::CedarOnly),
        },
        // An enrolled request reaches the boundary with no coexistence-map entry.
        "dual_engine_map_missing_denies" => Scenario {
            request: cedar_ready(role_manage_request(Role::SuperAdmin, OrgId::knl())),
            entry: None,
            cedar: CedarEvaluation::Allow {
                bundle_key: bundle_key(7),
            },
            expected_domain: None,
            expected_mode: None,
        },
        // Compare mode: Cedar allows but legacy denies (MEMBER).
        "dual_engine_disagreement_denies" => Scenario {
            request: cedar_ready(role_manage_request(Role::Member, OrgId::knl())),
            entry: Some(identity_entry(
                Feature::RoleManage,
                DualEngineMode::CedarEnforceLegacyCompare,
                Some(bundle_key(7)),
            )),
            cedar: CedarEvaluation::Allow {
                bundle_key: bundle_key(7),
            },
            expected_domain: Some(DOMAIN.to_owned()),
            expected_mode: Some(DualEngineMode::CedarEnforceLegacyCompare),
        },
        // The Cedar adapter returns an error / schema-validation failure.
        "cedar_error_denies" => Scenario {
            request: cedar_ready(role_manage_request(Role::SuperAdmin, OrgId::knl())),
            entry: Some(identity_entry(
                Feature::RoleManage,
                DualEngineMode::CedarOnly,
                Some(bundle_key(7)),
            )),
            cedar: CedarEvaluation::Error {
                reason: "cedar schema validation failed".to_owned(),
            },
            expected_domain: Some(DOMAIN.to_owned()),
            expected_mode: Some(DualEngineMode::CedarOnly),
        },
        // Cedar-enrolled request carries no subject-freshness material.
        "missing_freshness_denies" => {
            let request = role_manage_request(Role::SuperAdmin, OrgId::knl())
                .with_rls_scope_proof(RlsScopeProof::runtime_role_guc(OrgId::knl()));
            Scenario {
                request,
                entry: Some(identity_entry(
                    Feature::RoleManage,
                    DualEngineMode::CedarOnly,
                    Some(bundle_key(7)),
                )),
                cedar: CedarEvaluation::Allow {
                    bundle_key: bundle_key(7),
                },
                expected_domain: Some(DOMAIN.to_owned()),
                expected_mode: Some(DualEngineMode::CedarOnly),
            }
        }
        // Cedar-enrolled request has freshness but no armed RLS scope proof.
        "missing_rls_scope_proof_denies" => {
            let request = role_manage_request(Role::SuperAdmin, OrgId::knl())
                .with_subject_freshness(fresh())
                .requiring_freshness(required());
            Scenario {
                request,
                entry: Some(identity_entry(
                    Feature::RoleManage,
                    DualEngineMode::CedarOnly,
                    Some(bundle_key(7)),
                )),
                cedar: CedarEvaluation::Allow {
                    bundle_key: bundle_key(7),
                },
                expected_domain: Some(DOMAIN.to_owned()),
                expected_mode: Some(DualEngineMode::CedarOnly),
            }
        }
        // Entry whose feature does not match the request's action.
        "malformed_coexistence_map_denies" => Scenario {
            request: cedar_ready(role_manage_request(Role::SuperAdmin, OrgId::knl())),
            entry: Some(identity_entry(
                Feature::UserManage,
                DualEngineMode::CedarOnly,
                Some(bundle_key(7)),
            )),
            cedar: CedarEvaluation::Allow {
                bundle_key: bundle_key(7),
            },
            expected_domain: Some(DOMAIN.to_owned()),
            expected_mode: Some(DualEngineMode::CedarOnly),
        },
        other => panic!("unhandled readiness case id: {other}"),
    }
}

/// Assert the case-specific `mustAudit` evidence is present on the audit event
/// through the real (nested) accessors.
fn assert_must_audit(
    case: &ReadinessCase,
    scenario: &Scenario,
    audit: &console_platform_authz::AuthorizationAuditEvent,
) {
    // Common to every case.
    assert_eq!(
        audit.decision.reason, case.expected_reason,
        "{}: decision.reason",
        case.id
    );

    match case.id.as_str() {
        "stale_policy_denies" => {
            // expected_* ride audit.bundle_key (the map's key); evaluated_* ride
            // audit.evaluated_bundle_key (what Cedar reported).
            let expected = audit.bundle_key.as_ref().expect("expected bundle key");
            let evaluated = audit
                .evaluated_bundle_key
                .as_ref()
                .expect("evaluated bundle key");
            assert_eq!(expected.policy_version, 7);
            assert_eq!(evaluated.policy_version, 6);
            assert_ne!(expected, evaluated, "stale key must differ from map key");
            for key in [expected, evaluated] {
                assert!(!key.bundle_digest.is_empty());
                assert!(!key.schema_version.is_empty());
                assert_eq!(key.cedar_sdk_version, "4.11.2");
                assert_eq!(key.cedar_language_version, "4.5");
            }
        }
        "stale_subject_denies" => {
            assert_eq!(audit.subject_freshness, scenario.request.subject.freshness);
            assert_eq!(
                audit.required_freshness,
                scenario.request.required_freshness
            );
            assert!(audit.request_id.is_some());
        }
        "rls_separation_denies" => {
            assert_eq!(audit.principal_org_id, OrgId::knl());
            assert_eq!(audit.resource_org_id, OrgId::platform());
            assert_eq!(audit.resource_type, RESOURCE_TYPE);
        }
        "dual_engine_map_missing_denies" => {
            assert_eq!(audit.action, "role_manage");
            assert_eq!(audit.resource_type, RESOURCE_TYPE);
            assert!(
                audit.coexistence_entry_id.is_none(),
                "missing map => no entry id"
            );
        }
        "dual_engine_disagreement_denies" => {
            assert_eq!(
                audit.decision.mode,
                Some(DualEngineMode::CedarEnforceLegacyCompare)
            );
            assert_eq!(audit.action, "role_manage");
        }
        "cedar_error_denies" => {
            assert_eq!(audit.decision.mode, Some(DualEngineMode::CedarOnly));
            assert_eq!(
                audit.evaluated_reason_detail.as_deref(),
                Some("cedar schema validation failed")
            );
            let key = audit.bundle_key.as_ref().expect("map bundle key");
            assert!(!key.bundle_digest.is_empty());
            assert_eq!(key.cedar_sdk_version, "4.11.2");
            assert_eq!(key.cedar_language_version, "4.5");
        }
        "missing_freshness_denies" => {
            assert_eq!(audit.subject_freshness, SubjectFreshness::default());
            assert_eq!(
                audit.required_freshness,
                SubjectFreshnessRequirement::default()
            );
        }
        "missing_rls_scope_proof_denies" => {
            assert_eq!(audit.principal_org_id, OrgId::knl());
            assert_eq!(audit.resource_org_id, OrgId::knl());
            assert!(
                audit.rls_scope_proof.is_none(),
                "the fault is a missing scope proof"
            );
        }
        "malformed_coexistence_map_denies" => {
            assert!(audit.coexistence_entry_id.is_some());
            assert_eq!(audit.action, "role_manage");
            assert_eq!(audit.request_domain, DOMAIN);
            assert_eq!(audit.resource_type, RESOURCE_TYPE);
        }
        other => panic!("unhandled readiness case id: {other}"),
    }
}

#[test]
fn every_readiness_case_binds_to_a_fail_closed_boundary_decision() {
    let fixture: ReadinessFixture =
        serde_json::from_str(include_str!("fixtures/cedar_pbac_readiness_cases.json"))
            .expect("readiness fixture must be valid JSON");

    let expected_ids = BTreeSet::from([
        "stale_policy_denies",
        "stale_subject_denies",
        "rls_separation_denies",
        "dual_engine_map_missing_denies",
        "dual_engine_disagreement_denies",
        "cedar_error_denies",
        "missing_freshness_denies",
        "missing_rls_scope_proof_denies",
        "malformed_coexistence_map_denies",
    ]);
    let actual_ids = fixture
        .cases
        .iter()
        .map(|case| case.id.as_str())
        .collect::<BTreeSet<_>>();
    assert_eq!(actual_ids, expected_ids, "all nine cases must be present");

    for case in &fixture.cases {
        let scenario = scenario_for(&case.id);
        // The fixture's declared mode must match the scenario we bind it to (or be
        // absent for the missing-map case).
        assert_eq!(case.mode, scenario.expected_mode, "{}: mode", case.id);

        let decision = evaluate_cedar_pbac_boundary(
            &scenario.request,
            scenario.entry.as_ref(),
            scenario.cedar.clone(),
        );

        assert_eq!(decision.effect, case.expected_effect, "{}: effect", case.id);
        assert_eq!(
            decision.effect,
            DecisionEffect::Deny,
            "{}: fail-closed",
            case.id
        );
        assert_eq!(decision.engine, case.expected_engine, "{}: engine", case.id);
        assert_eq!(decision.reason, case.expected_reason, "{}: reason", case.id);

        let audit = observe_cedar_pbac_decision(
            &scenario.request,
            scenario.entry.as_ref(),
            Some(&scenario.cedar),
            decision.clone(),
        );

        // Observation records, never mutates, the enforced decision.
        assert_eq!(
            audit.decision, decision,
            "{}: observation is fed the decision",
            case.id
        );

        // The metric projection is exactly the five low-cardinality labels — no
        // ids, versions, or digests leak into metric cardinality.
        let metric = audit.metric_labels();
        assert_eq!(
            metric.effect, case.expected_effect,
            "{}: metric effect",
            case.id
        );
        assert_eq!(
            metric.engine, case.expected_engine,
            "{}: metric engine",
            case.id
        );
        assert_eq!(
            metric.reason, case.expected_reason,
            "{}: metric reason",
            case.id
        );
        assert_eq!(
            metric.mode, scenario.expected_mode,
            "{}: metric mode",
            case.id
        );
        assert_eq!(
            metric.domain, scenario.expected_domain,
            "{}: metric domain",
            case.id
        );
        let metric_keys = metric_label_keys(&metric);
        assert_eq!(
            metric_keys,
            BTreeSet::from([
                "effect".to_owned(),
                "engine".to_owned(),
                "reason".to_owned(),
                "mode".to_owned(),
                "domain".to_owned(),
            ]),
            "{}: metric labels must be exactly the five low-cardinality dimensions",
            case.id
        );

        assert!(
            !case.must_audit.is_empty(),
            "{}: fixture must name required audit evidence",
            case.id
        );
        assert_must_audit(case, &scenario, &audit);
    }
}

fn metric_label_keys(
    metric: &console_platform_authz::AuthorizationMetricLabels,
) -> BTreeSet<String> {
    let value = serde_json::to_value(metric).expect("metric labels serialize");
    value
        .as_object()
        .expect("metric labels are an object")
        .keys()
        .cloned()
        .collect()
}
