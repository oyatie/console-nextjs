#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeSet;

use console_kernel_core::{
    BranchId, BranchScope, ErrorKind, OrgId, ServicePrincipalId, Timestamp, UserId,
};
use console_platform_authz::{
    Action, AuthorizationContext, AuthorizationRequest, AuthorizationResource, BranchColumn,
    BranchScopedResource, CedarEvaluation, CoexistenceMapEntry, CompiledBundleCacheKey,
    DecisionEffect, DecisionEngine, DecisionReason, DualEngineMode, EffectiveFeatureGrant, Feature,
    PermissionLevel, Principal, ResourceBranch, RlsScopeProof, Role, ServicePrincipal,
    SubjectFreshness, SubjectFreshnessRequirement, authorize, authorize_capability,
    authorize_org_wide, authorize_scoped, authorize_service, evaluate_cedar_pbac_boundary,
    evaluate_legacy_contract, observe_cedar_pbac_decision, permission_for, repository_filter,
    resolve_branch_scope_in_org, resolve_effective_feature_grants_in_org,
};
use sqlx::PgPool;

const ROLES: [Role; 6] = [
    Role::Member,
    Role::Receptionist,
    Role::Mechanic,
    Role::Admin,
    Role::Executive,
    Role::SuperAdmin,
];

fn expected_matrix() -> [(Feature, [PermissionLevel; 6]); 96] {
    use Feature::{
        AiAssist, ApprovalFinalize, AssigneeManage, AuditLogRead, AuditStreamAccessLogRead,
        AuditStreamRead, BenefitCatalogManage, BenefitCatalogRead, BranchManage, CompletionReview,
        ComplianceDomainManage, ComplianceDomainRead, ComplianceEvidenceLink, ConsultingManage,
        ConsultingRead, DailyPlanRequest, DailyPlanReview, ElevatedRoleGrant,
        EmployeeDirectoryManage, EmployeeDirectoryRead, Equipment3rApprove, Equipment3rAssess,
        Equipment3rDispatch, Equipment3rDisposition, Equipment3rInspect, Equipment3rObserve,
        Equipment3rQuote, Equipment3rRegistry, EquipmentCostLedgerRead, EquipmentCostLedgerWrite,
        EquipmentManage, EvaluationManage, EvaluationRead, EvaluationSubmit, EvidenceAttach,
        ExcelDownload, ExitCaseHqConfirm, ExitCaseHrConfirm, ExitCaseReport, ExitSettlementManage,
        FacilitiesAccept, FacilitiesDispatch, FacilitiesExecute, FacilitiesManage,
        FacilitiesObserve, InspectionRoundComplete, InspectionScheduleManage,
        IntegrityFindingTriage, IntegrityFindingsRead, InventoryConsume, InventoryManage,
        InventoryRead, InventoryReorder, KpiExclusionManage, KpiRead, LifecycleManage, Login,
        LogisticsDispatch, LogisticsPickPack, LogisticsPod, LogisticsPutaway, LogisticsReceive,
        LogisticsRelease, LogisticsSettle, MailAccountManage, MailUse, MasterListImport,
        NoticeManage, OpsDashboardRead, OrgWideQueueTriage, PayrollRunManage, PayrollRunRead,
        PeriodLockManage, PriorityManage, ProductionSourceIngest, PurchaseExecute,
        PurchaseFinalApprove, PurchaseRequestApprove, PurchaseRequestCreate, PurchaseRequestRead,
        RecruitingManage, RecruitingRead, RegionManage, RentalQuoteManage, RoleManage, SalesManage,
        SubordinateUserCreate, TargetManage, UserManage, WorkOrderCreate, WorkOrderEditIntake,
        WorkOrderReadAll, WorkOrderStart, WorkReportSubmit,
    };
    use PermissionLevel::{Allow as A, Deny as D, Limited as L, RequestOnly as R};

    // Column order: [MEMBER, RECEPTIONIST, MECHANIC, ADMIN, EXECUTIVE, SUPER_ADMIN].
    // MEMBER (open-signup default) is default-DENY everywhere but `Login`.
    [
        (Login, [A, A, A, A, A, A]),
        (WorkOrderCreate, [D, A, L, A, L, A]),
        (WorkOrderEditIntake, [D, A, L, A, L, A]),
        (WorkOrderReadAll, [D, A, A, A, A, A]),
        (WorkOrderStart, [D, L, A, A, L, A]),
        (WorkReportSubmit, [D, L, A, A, L, A]),
        (EvidenceAttach, [D, A, A, A, L, A]),
        (PriorityManage, [D, D, D, A, D, A]),
        (AssigneeManage, [D, D, D, A, D, A]),
        (TargetManage, [D, D, R, A, D, A]),
        (CompletionReview, [D, D, D, A, D, A]),
        (ApprovalFinalize, [D, D, D, A, A, A]),
        (DailyPlanRequest, [D, D, A, A, D, A]),
        (DailyPlanReview, [D, D, D, A, D, A]),
        // Org-wide queue triage read: EXECUTIVE + SUPER_ADMIN only (a branch
        // ADMIN stays confined to its branch scope), matching the org-wide tier
        // of resolve_branch_scope_in_org.
        (OrgWideQueueTriage, [D, D, D, D, A, A]),
        (KpiRead, [D, D, D, A, A, A]),
        (KpiExclusionManage, [D, D, D, A, A, A]),
        (UserManage, [D, D, D, A, D, A]),
        (SubordinateUserCreate, [D, D, D, L, D, A]),
        (ElevatedRoleGrant, [D, D, D, D, D, A]),
        (RoleManage, [D, D, D, D, D, A]),
        (RegionManage, [D, D, D, A, A, A]),
        (BranchManage, [D, D, D, A, A, A]),
        (EquipmentManage, [D, D, D, A, A, A]),
        (MasterListImport, [D, D, D, A, D, A]),
        (RentalQuoteManage, [D, A, D, A, A, A]),
        (EquipmentCostLedgerRead, [D, D, D, A, A, A]),
        (EquipmentCostLedgerWrite, [D, D, D, A, D, A]),
        (PurchaseRequestCreate, [D, A, R, A, D, A]),
        (PurchaseRequestRead, [D, A, L, A, A, A]),
        (PurchaseRequestApprove, [D, D, D, A, D, A]),
        (PurchaseFinalApprove, [D, D, D, D, A, A]),
        (PurchaseExecute, [D, A, D, A, D, A]),
        (InspectionScheduleManage, [D, D, D, A, D, A]),
        (InspectionRoundComplete, [D, D, A, A, D, A]),
        (AuditLogRead, [D, D, D, A, D, A]),
        (ExcelDownload, [D, A, A, A, A, A]),
        (OpsDashboardRead, [D, D, D, A, D, A]),
        (SalesManage, [D, D, D, A, A, A]),
        // Logistics is explicit-grant only: no built-in role fallback.
        (LogisticsReceive, [D, D, D, D, D, D]),
        (LogisticsPutaway, [D, D, D, D, D, D]),
        (LogisticsRelease, [D, D, D, D, D, D]),
        (LogisticsPickPack, [D, D, D, D, D, D]),
        (LogisticsDispatch, [D, D, D, D, D, D]),
        (LogisticsPod, [D, D, D, D, D, D]),
        (LogisticsSettle, [D, D, D, D, D, D]),
        // IV inventory is branch-operational: front-office/mechanics may read,
        // mechanics/admins may consume, and management/reorder is admin-tier.
        (InventoryRead, [D, A, A, A, A, A]),
        (InventoryManage, [D, D, D, A, D, A]),
        (InventoryConsume, [D, D, A, A, D, A]),
        (InventoryReorder, [D, D, D, A, D, A]),
        // Benefit catalog is HR compensation reference data — mirrors the HR
        // EmployeeDirectory tier (the closest analogous feature): read is admin +
        // leadership (ADMIN/EXECUTIVE/SUPER_ADMIN), management is the HR-config
        // authority (ADMIN/SUPER_ADMIN). Values match the authoritative
        // `permission_matrix` in lib.rs.
        (BenefitCatalogRead, [D, D, D, A, A, A]),
        (BenefitCatalogManage, [D, D, D, A, D, A]),
        // Compliance domain is branch-operational for scoped obligations but
        // the REST boundary independently requires org-wide scope for tenant
        // catalog objects. Evidence linking is a distinct audited capability.
        (ComplianceDomainRead, [D, D, D, A, A, A]),
        (ComplianceDomainManage, [D, D, D, A, D, A]),
        (ComplianceEvidenceLink, [D, D, D, A, D, A]),
        // Consulting is capability-driven from its first slice.
        (ConsultingRead, [D, D, D, D, D, D]),
        (ConsultingManage, [D, D, D, D, D, D]),
        // Keep the historical wire key, but v1 has no AI runtime authority.
        (AiAssist, [D, D, D, D, D, D]),
        // Integrity findings are labor-law sensitive: EXECUTIVE + SUPER_ADMIN only.
        (IntegrityFindingsRead, [D, D, D, D, A, A]),
        (IntegrityFindingTriage, [D, D, D, D, A, A]),
        // Webmail: configuring the mailbox is ADMIN + SUPER_ADMIN (holds the
        // mail secrets); sending is front-office + leadership (no MECHANIC).
        (MailAccountManage, [D, D, D, A, D, A]),
        (MailUse, [D, A, D, A, A, A]),
        (EmployeeDirectoryRead, [D, D, D, A, A, A]),
        (EmployeeDirectoryManage, [D, D, D, A, D, A]),
        // Absence → exit → settlement separation of duties. Report / HR confirm /
        // settlement are the branch HR-manager tier (ADMIN + SUPER_ADMIN); HQ
        // confirm is the org-wide leadership tier (EXECUTIVE + SUPER_ADMIN),
        // mirroring OrgWideQueueTriage. The confirm handler's distinct-actor rule
        // still forbids one person from performing both confirmation tiers.
        (ExitCaseReport, [D, D, D, A, D, A]),
        (ExitCaseHrConfirm, [D, D, D, A, D, A]),
        (ExitCaseHqConfirm, [D, D, D, D, A, A]),
        (ExitSettlementManage, [D, D, D, A, D, A]),
        (Feature::AttendanceExceptionManage, [D, D, D, A, D, A]),
        (Feature::AttendanceSubstitutionManage, [D, D, D, A, D, A]),
        // Covert audit stream actions are Cedar clearance-only: the legacy
        // role matrix intentionally denies every built-in role.
        (AuditStreamRead, [D, D, D, D, D, D]),
        (AuditStreamAccessLogRead, [D, D, D, D, D, D]),
        // BE-LC: period close authority (ADMIN + EXECUTIVE + SUPER_ADMIN) and
        // lifecycle/records management (ADMIN + SUPER_ADMIN).
        (PeriodLockManage, [D, D, D, A, A, A]),
        (LifecycleManage, [D, D, D, A, D, A]),
        // Payroll draft-run/line staging read: same tier as
        // EmployeeDirectoryRead (financial/HR-sensitive).
        (PayrollRunManage, [D, D, D, A, A, A]),
        (PayrollRunRead, [D, D, D, A, A, A]),
        // Notice-board publish tier: ADMIN + EXECUTIVE + SUPER_ADMIN.
        (NoticeManage, [D, D, D, A, A, A]),
        (FacilitiesManage, [D, D, D, A, D, A]),
        (FacilitiesDispatch, [D, D, D, A, D, A]),
        (FacilitiesExecute, [D, D, A, A, D, A]),
        (FacilitiesAccept, [D, D, D, A, D, A]),
        (FacilitiesObserve, [D, A, A, A, A, A]),
        (ProductionSourceIngest, [D, D, D, D, D, D]),
        // Equipment 3R: capability-driven, custom-grant only.
        (Equipment3rRegistry, [D, D, D, D, D, D]),
        (Equipment3rQuote, [D, D, D, D, D, D]),
        (Equipment3rApprove, [D, D, D, D, D, D]),
        (Equipment3rDispatch, [D, D, D, D, D, D]),
        (Equipment3rInspect, [D, D, D, D, D, D]),
        (Equipment3rAssess, [D, D, D, D, D, D]),
        (Equipment3rDisposition, [D, D, D, D, D, D]),
        (Equipment3rObserve, [D, D, D, D, D, D]),
        // Recruiting mirrors the HR directory pair (HR-owned data, EXECUTIVE
        // read-only) and is gated org-wide like EmployeeDirectory*.
        (RecruitingRead, [D, D, D, A, A, A]),
        (RecruitingManage, [D, D, D, A, D, A]),
        (EvaluationRead, [D, D, D, A, A, A]),
        (EvaluationManage, [D, D, D, A, D, A]),
        (EvaluationSubmit, [D, D, D, A, A, A]),
    ]
}

fn principal(role: Role, scope: BranchScope) -> Principal {
    Principal::new(UserId::new(), OrgId::knl(), BTreeSet::from([role]), scope)
}

fn cedar_bundle_key() -> CompiledBundleCacheKey {
    CompiledBundleCacheKey::new(
        OrgId::knl(),
        7,
        "schema-v1",
        "sha256:bundle",
        "4.11.2",
        "4.5",
    )
    .unwrap()
}

fn stale_cedar_bundle_key() -> CompiledBundleCacheKey {
    CompiledBundleCacheKey::new(
        OrgId::knl(),
        6,
        "schema-v1",
        "sha256:stale-bundle",
        "4.11.2",
        "4.5",
    )
    .unwrap()
}

fn cedar_freshness() -> SubjectFreshness {
    SubjectFreshness {
        policy_version: 7,
        subject_version: 11,
        session_generation: 3,
        step_up_generation: Some(2),
    }
}

fn cedar_freshness_requirement() -> SubjectFreshnessRequirement {
    SubjectFreshnessRequirement {
        min_policy_version: 7,
        min_subject_version: 11,
        min_session_generation: 3,
        required_step_up_generation: Some(2),
    }
}

fn cedar_ready(request: AuthorizationRequest) -> AuthorizationRequest {
    request
        .with_policy_domain("identity.policy")
        .with_subject_freshness(cedar_freshness())
        .requiring_freshness(cedar_freshness_requirement())
        .with_rls_scope_proof(RlsScopeProof::runtime_role_guc(OrgId::knl()))
}

fn coexistence_entry(mode: DualEngineMode) -> CoexistenceMapEntry {
    CoexistenceMapEntry::new(
        "identity.policy.role_manage",
        "identity.policy",
        Feature::RoleManage,
        "policy_role",
        mode,
        Some(cedar_bundle_key()),
    )
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct CedarPbacReadinessFixture {
    version: u64,
    status: String,
    decision_source: String,
    cutover_contract: String,
    observability_contract: String,
    cases: Vec<CedarPbacReadinessCase>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct CedarPbacReadinessCase {
    id: String,
    mode: Option<DualEngineMode>,
    input_fault: String,
    expected_effect: DecisionEffect,
    expected_engine: DecisionEngine,
    expected_reason: DecisionReason,
    must_audit: Vec<String>,
}

#[test]
fn role_enum_uses_canonical_database_codes() {
    assert_eq!(Role::SuperAdmin.as_str(), "SUPER_ADMIN");
    assert_eq!(Role::Admin.as_str(), "ADMIN");
    assert_eq!(Role::Mechanic.as_str(), "MECHANIC");
    assert_eq!(Role::Receptionist.as_str(), "RECEPTIONIST");
    assert_eq!(Role::Executive.as_str(), "EXECUTIVE");
    assert_eq!(Role::Member.as_str(), "MEMBER");

    assert_eq!("SUPER_ADMIN".parse::<Role>().unwrap(), Role::SuperAdmin);
    assert_eq!("MEMBER".parse::<Role>().unwrap(), Role::Member);
    assert!("OWNER".parse::<Role>().is_err());
}

#[test]
fn member_role_is_default_deny_except_login() {
    // The open-signup default tier: it can authenticate but nothing else until an
    // admin elevates it. `Login` is its only `Allow` cell across all features.
    for feature in Feature::ALL {
        let level = permission_for(Role::Member, feature);
        if feature == Feature::Login {
            assert_eq!(
                level,
                PermissionLevel::Allow,
                "MEMBER must be able to log in"
            );
        } else {
            assert_eq!(
                level,
                PermissionLevel::Deny,
                "MEMBER must be denied {feature:?} until elevated"
            );
        }
    }

    // And it cannot pass an authorize() gate for any real feature.
    let branch = BranchId::new();
    let member = principal(Role::Member, BranchScope::single(branch));
    let err = authorize(&member, Action::new(Feature::WorkOrderReadAll), branch).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Forbidden);
}

#[test]
fn retired_ai_permission_denies_every_authorization_door() {
    let branch = BranchId::new();
    assert_eq!("ai_assist".parse::<Feature>().unwrap(), Feature::AiAssist);
    assert_eq!(Feature::AiAssist.as_str(), "ai_assist");
    for role in ROLES {
        assert_eq!(
            permission_for(role, Feature::AiAssist),
            PermissionLevel::Deny
        );
    }

    let actor = principal(Role::SuperAdmin, BranchScope::All).with_effective_feature_grants(vec![
        EffectiveFeatureGrant::new(Feature::AiAssist, PermissionLevel::Allow, BranchScope::All),
    ]);
    let service = ServicePrincipal::new(
        ServicePrincipalId::new(),
        OrgId::knl(),
        branch,
        Feature::AiAssist,
    );
    let entry = CoexistenceMapEntry::new(
        "retired.ai_assist",
        "identity.policy",
        Feature::AiAssist,
        "policy_role",
        DualEngineMode::CedarOnly,
        Some(cedar_bundle_key()),
    );
    for action in [
        Action::new(Feature::AiAssist),
        Action::limited(Feature::AiAssist),
        Action::request(Feature::AiAssist),
    ] {
        for result in [
            authorize(&actor, action, branch),
            authorize_capability(&actor, action),
            authorize_org_wide(&actor, action),
            authorize_service(&service, action, branch),
        ] {
            assert_eq!(result.unwrap_err().kind, ErrorKind::Forbidden);
        }

        let request = cedar_ready(
            AuthorizationRequest::new(
                actor.clone(),
                action,
                AuthorizationResource::branch(OrgId::knl(), branch, "policy_role"),
            )
            .with_policy_domain("identity.policy"),
        );
        assert_eq!(
            evaluate_legacy_contract(&request).effect,
            DecisionEffect::Deny
        );
        assert_eq!(
            evaluate_cedar_pbac_boundary(
                &request,
                Some(&entry),
                CedarEvaluation::Allow {
                    bundle_key: cedar_bundle_key(),
                },
            )
            .effect,
            DecisionEffect::Deny
        );
    }
}

#[test]
fn cedar_pbac_legacy_contract_preserves_current_authorize_behavior() {
    let branch = BranchId::new();
    let actor = principal(Role::Admin, BranchScope::single(branch));
    let request = AuthorizationRequest::new(
        actor,
        Action::new(Feature::PriorityManage),
        AuthorizationResource::branch(OrgId::knl(), branch, "work_order"),
    );

    let decision = evaluate_legacy_contract(&request);

    assert_eq!(decision.effect, DecisionEffect::Allow);
    assert_eq!(decision.reason, DecisionReason::LegacyAllowed);
    assert_eq!(decision.mode, Some(DualEngineMode::LegacyOnly));
}

#[test]
fn cedar_pbac_boundary_denies_missing_coexistence_map() {
    let branch = BranchId::new();
    let actor = principal(Role::SuperAdmin, BranchScope::single(branch));
    let request = AuthorizationRequest::new(
        actor,
        Action::new(Feature::RoleManage),
        AuthorizationResource::branch(OrgId::knl(), branch, "policy_role"),
    );

    let decision = evaluate_cedar_pbac_boundary(
        &request,
        None,
        CedarEvaluation::Allow {
            bundle_key: cedar_bundle_key(),
        },
    );

    assert_eq!(decision.effect, DecisionEffect::Deny);
    assert_eq!(decision.reason, DecisionReason::MissingCoexistenceMap);
}

#[test]
fn cedar_pbac_readiness_fixture_cases_are_structurally_typed() {
    let fixture: CedarPbacReadinessFixture =
        serde_json::from_str(include_str!("fixtures/cedar_pbac_readiness_cases.json"))
            .expect("Cedar/PBAC readiness fixture must be valid JSON");

    assert_eq!(fixture.version, 1);
    assert_eq!(fixture.status, "design_no_live_switch");
    assert!(fixture.decision_source.ends_with(".md"));
    assert!(fixture.cutover_contract.ends_with(".md"));
    assert!(fixture.observability_contract.ends_with(".md"));

    let actual_cases = fixture
        .cases
        .iter()
        .map(|case| case.id.as_str())
        .collect::<BTreeSet<_>>();
    let expected_cases = BTreeSet::from([
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
    assert_eq!(actual_cases, expected_cases);

    for case in fixture.cases {
        assert!(!case.input_fault.trim().is_empty(), "{}", case.id);
        assert!(
            !case.must_audit.is_empty(),
            "case {} must name required audit evidence",
            case.id
        );

        match case.id.as_str() {
            "dual_engine_map_missing_denies" => assert!(case.mode.is_none()),
            _ => assert!(case.mode.is_some(), "case {} must name a mode", case.id),
        }

        match case.id.as_str() {
            "stale_policy_denies" => {
                assert_eq!(case.expected_effect, DecisionEffect::Deny);
                assert_eq!(case.expected_engine, DecisionEngine::BoundaryPreflight);
                assert_eq!(case.expected_reason, DecisionReason::StalePolicyBundle);
            }
            "cedar_error_denies" => {
                assert_eq!(case.expected_effect, DecisionEffect::Deny);
                assert_eq!(case.expected_engine, DecisionEngine::Cedar);
                assert_eq!(case.expected_reason, DecisionReason::CedarError);
            }
            "dual_engine_disagreement_denies" => {
                assert_eq!(case.expected_effect, DecisionEffect::Deny);
                assert_eq!(case.expected_engine, DecisionEngine::DualEngine);
                assert_eq!(case.expected_reason, DecisionReason::EngineDisagreement);
            }
            _ => assert_eq!(case.expected_effect, DecisionEffect::Deny),
        }
    }
}

#[test]
fn cedar_pbac_boundary_denies_stale_policy_bundle() {
    let branch = BranchId::new();
    let actor = principal(Role::SuperAdmin, BranchScope::single(branch));
    let request = cedar_ready(AuthorizationRequest::new(
        actor,
        Action::new(Feature::RoleManage),
        AuthorizationResource::branch(OrgId::knl(), branch, "policy_role"),
    ));
    let entry = coexistence_entry(DualEngineMode::CedarOnly);

    let decision = evaluate_cedar_pbac_boundary(
        &request,
        Some(&entry),
        CedarEvaluation::Allow {
            bundle_key: stale_cedar_bundle_key(),
        },
    );

    assert_eq!(decision.effect, DecisionEffect::Deny);
    assert_eq!(decision.reason, DecisionReason::StalePolicyBundle);
}

#[test]
fn cedar_pbac_boundary_denies_stale_subject_before_legacy_allow() {
    let branch = BranchId::new();
    let actor = principal(Role::SuperAdmin, BranchScope::single(branch));
    let request = AuthorizationRequest::new(
        actor,
        Action::new(Feature::RoleManage),
        AuthorizationResource::branch(OrgId::knl(), branch, "policy_role"),
    )
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

    let decision = evaluate_cedar_pbac_boundary(
        &request,
        Some(&coexistence_entry(DualEngineMode::LegacyOnly)),
        CedarEvaluation::NotConfigured,
    );

    assert_eq!(decision.effect, DecisionEffect::Deny);
    assert_eq!(decision.reason, DecisionReason::StaleSubject);
}

#[test]
fn cedar_pbac_boundary_denies_missing_freshness_before_policy_allow() {
    let branch = BranchId::new();
    let actor = principal(Role::SuperAdmin, BranchScope::single(branch));
    let request = AuthorizationRequest::new(
        actor,
        Action::new(Feature::RoleManage),
        AuthorizationResource::branch(OrgId::knl(), branch, "policy_role"),
    )
    .with_policy_domain("identity.policy")
    .with_rls_scope_proof(RlsScopeProof::runtime_role_guc(OrgId::knl()));

    let decision = evaluate_cedar_pbac_boundary(
        &request,
        Some(&coexistence_entry(DualEngineMode::CedarOnly)),
        CedarEvaluation::Allow {
            bundle_key: cedar_bundle_key(),
        },
    );

    assert_eq!(decision.effect, DecisionEffect::Deny);
    assert_eq!(decision.reason, DecisionReason::MissingSubjectFreshness);
}

#[test]
fn cedar_pbac_boundary_denies_missing_rls_scope_proof_before_policy_allow() {
    let branch = BranchId::new();
    let actor = principal(Role::SuperAdmin, BranchScope::single(branch));
    let request = AuthorizationRequest::new(
        actor,
        Action::new(Feature::RoleManage),
        AuthorizationResource::branch(OrgId::knl(), branch, "policy_role"),
    )
    .with_policy_domain("identity.policy")
    .with_subject_freshness(cedar_freshness())
    .requiring_freshness(cedar_freshness_requirement());

    let decision = evaluate_cedar_pbac_boundary(
        &request,
        Some(&coexistence_entry(DualEngineMode::CedarOnly)),
        CedarEvaluation::Allow {
            bundle_key: cedar_bundle_key(),
        },
    );

    assert_eq!(decision.effect, DecisionEffect::Deny);
    assert_eq!(decision.reason, DecisionReason::MissingRlsScopeProof);
}

#[test]
fn cedar_pbac_boundary_denies_malformed_map_action_before_policy_allow() {
    let branch = BranchId::new();
    let actor = principal(Role::SuperAdmin, BranchScope::single(branch));
    let request = cedar_ready(AuthorizationRequest::new(
        actor,
        Action::new(Feature::RoleManage),
        AuthorizationResource::branch(OrgId::knl(), branch, "policy_role"),
    ));
    let wrong_action_entry = CoexistenceMapEntry::new(
        "identity.policy.user_manage",
        "identity.policy",
        Feature::UserManage,
        "policy_role",
        DualEngineMode::CedarOnly,
        Some(cedar_bundle_key()),
    );

    let decision = evaluate_cedar_pbac_boundary(
        &request,
        Some(&wrong_action_entry),
        CedarEvaluation::Allow {
            bundle_key: cedar_bundle_key(),
        },
    );

    assert_eq!(decision.effect, DecisionEffect::Deny);
    assert_eq!(decision.reason, DecisionReason::MalformedCoexistenceMap);
}

#[test]
fn cedar_pbac_boundary_denies_malformed_map_domain_before_policy_allow() {
    let branch = BranchId::new();
    let actor = principal(Role::SuperAdmin, BranchScope::single(branch));
    let request = cedar_ready(AuthorizationRequest::new(
        actor,
        Action::new(Feature::RoleManage),
        AuthorizationResource::branch(OrgId::knl(), branch, "policy_role"),
    ));
    let wrong_domain_entry = CoexistenceMapEntry::new(
        "identity.policy.role_manage",
        "workflow.guards",
        Feature::RoleManage,
        "policy_role",
        DualEngineMode::CedarOnly,
        Some(cedar_bundle_key()),
    );

    let decision = evaluate_cedar_pbac_boundary(
        &request,
        Some(&wrong_domain_entry),
        CedarEvaluation::Allow {
            bundle_key: cedar_bundle_key(),
        },
    );

    assert_eq!(decision.effect, DecisionEffect::Deny);
    assert_eq!(decision.reason, DecisionReason::MalformedCoexistenceMap);
}

#[test]
fn cedar_pbac_boundary_denies_cross_org_resource_before_policy_allow() {
    let branch = BranchId::new();
    let actor = principal(Role::SuperAdmin, BranchScope::single(branch));
    let request = AuthorizationRequest::new(
        actor,
        Action::new(Feature::RoleManage),
        AuthorizationResource::branch(OrgId::platform(), branch, "policy_role"),
    );

    let decision = evaluate_cedar_pbac_boundary(
        &request,
        Some(&coexistence_entry(DualEngineMode::CedarOnly)),
        CedarEvaluation::Allow {
            bundle_key: cedar_bundle_key(),
        },
    );

    assert_eq!(decision.effect, DecisionEffect::Deny);
    assert_eq!(decision.reason, DecisionReason::RlsBoundaryMismatch);
}

#[test]
fn cedar_pbac_shadow_mode_cannot_use_cedar_allow_to_grant() {
    let branch = BranchId::new();
    let actor = principal(Role::Member, BranchScope::single(branch));
    let request = cedar_ready(AuthorizationRequest::new(
        actor,
        Action::new(Feature::RoleManage),
        AuthorizationResource::branch(OrgId::knl(), branch, "policy_role"),
    ));

    let decision = evaluate_cedar_pbac_boundary(
        &request,
        Some(&coexistence_entry(DualEngineMode::CedarShadowLegacyEnforce)),
        CedarEvaluation::Allow {
            bundle_key: cedar_bundle_key(),
        },
    );

    assert_eq!(decision.effect, DecisionEffect::Deny);
    assert_eq!(decision.reason, DecisionReason::LegacyDenied);
}

#[test]
fn cedar_pbac_shadow_mode_denies_cedar_error_before_legacy_allow() {
    let branch = BranchId::new();
    let actor = principal(Role::SuperAdmin, BranchScope::single(branch));
    let request = cedar_ready(AuthorizationRequest::new(
        actor,
        Action::new(Feature::RoleManage),
        AuthorizationResource::branch(OrgId::knl(), branch, "policy_role"),
    ));

    let decision = evaluate_cedar_pbac_boundary(
        &request,
        Some(&coexistence_entry(DualEngineMode::CedarShadowLegacyEnforce)),
        CedarEvaluation::Error {
            reason: "schema validation failed".to_owned(),
        },
    );

    assert_eq!(decision.effect, DecisionEffect::Deny);
    assert_eq!(decision.reason, DecisionReason::CedarError);
}

#[test]
fn cedar_pbac_shadow_mode_denies_cedar_deny_before_legacy_allow() {
    let branch = BranchId::new();
    let actor = principal(Role::SuperAdmin, BranchScope::single(branch));
    let request = cedar_ready(AuthorizationRequest::new(
        actor,
        Action::new(Feature::RoleManage),
        AuthorizationResource::branch(OrgId::knl(), branch, "policy_role"),
    ));

    let decision = evaluate_cedar_pbac_boundary(
        &request,
        Some(&coexistence_entry(DualEngineMode::CedarShadowLegacyEnforce)),
        CedarEvaluation::Deny {
            bundle_key: cedar_bundle_key(),
            reason: "shadow policy denied".to_owned(),
        },
    );

    assert_eq!(decision.effect, DecisionEffect::Deny);
    assert_eq!(decision.reason, DecisionReason::CedarDenied);
}

#[test]
fn cedar_pbac_compare_mode_denies_engine_disagreement() {
    let branch = BranchId::new();
    let actor = principal(Role::Member, BranchScope::single(branch));
    let request = cedar_ready(AuthorizationRequest::new(
        actor,
        Action::new(Feature::RoleManage),
        AuthorizationResource::branch(OrgId::knl(), branch, "policy_role"),
    ));

    let decision = evaluate_cedar_pbac_boundary(
        &request,
        Some(&coexistence_entry(
            DualEngineMode::CedarEnforceLegacyCompare,
        )),
        CedarEvaluation::Allow {
            bundle_key: cedar_bundle_key(),
        },
    );

    assert_eq!(decision.effect, DecisionEffect::Deny);
    assert_eq!(decision.reason, DecisionReason::EngineDisagreement);
}

#[test]
fn cedar_pbac_boundary_denies_cedar_errors() {
    let branch = BranchId::new();
    let actor = principal(Role::SuperAdmin, BranchScope::single(branch));
    let request = cedar_ready(AuthorizationRequest::new(
        actor,
        Action::new(Feature::RoleManage),
        AuthorizationResource::branch(OrgId::knl(), branch, "policy_role"),
    ));

    let decision = evaluate_cedar_pbac_boundary(
        &request,
        Some(&coexistence_entry(DualEngineMode::CedarOnly)),
        CedarEvaluation::Error {
            reason: "schema validation failed".to_owned(),
        },
    );

    assert_eq!(decision.effect, DecisionEffect::Deny);
    assert_eq!(decision.reason, DecisionReason::CedarError);
}

#[test]
fn cedar_pbac_observation_records_metric_labels_and_full_audit_context() {
    let branch = BranchId::new();
    let actor = principal(Role::SuperAdmin, BranchScope::single(branch));
    let mut request = cedar_ready(AuthorizationRequest::new(
        actor,
        Action::new(Feature::RoleManage),
        AuthorizationResource::branch(OrgId::knl(), branch, "policy_role")
            .with_resource_id("role:custom-admin"),
    ));
    request.context = AuthorizationContext {
        purpose: Some("policy_admin".to_owned()),
        channel: Some("web".to_owned()),
        request_id: Some("req-cedar-pbac-1".to_owned()),
    };
    let entry = coexistence_entry(DualEngineMode::CedarOnly);

    let cedar = CedarEvaluation::Allow {
        bundle_key: stale_cedar_bundle_key(),
    };
    let decision = evaluate_cedar_pbac_boundary(&request, Some(&entry), cedar.clone());
    let audit = observe_cedar_pbac_decision(&request, Some(&entry), Some(&cedar), decision);
    let metric = audit.metric_labels();

    assert_eq!(metric.effect, DecisionEffect::Deny);
    assert_eq!(metric.engine, DecisionEngine::BoundaryPreflight);
    assert_eq!(metric.reason, DecisionReason::StalePolicyBundle);
    assert_eq!(metric.mode, Some(DualEngineMode::CedarOnly));
    assert_eq!(metric.domain.as_deref(), Some("identity.policy"));
    assert_eq!(audit.request_domain, "identity.policy");

    assert_eq!(
        audit.coexistence_entry_id.as_deref(),
        Some("identity.policy.role_manage")
    );
    assert_eq!(audit.action, "role_manage");
    assert_eq!(audit.required_permission, "allow");
    assert_eq!(audit.resource_type, "policy_role");
    assert_eq!(audit.resource_id.as_deref(), Some("role:custom-admin"));
    assert_eq!(audit.request_id.as_deref(), Some("req-cedar-pbac-1"));
    assert_eq!(audit.purpose.as_deref(), Some("policy_admin"));
    assert_eq!(audit.channel.as_deref(), Some("web"));
    assert_eq!(audit.subject_freshness.policy_version, 7);
    assert_eq!(
        audit.required_freshness.required_step_up_generation,
        Some(2)
    );
    assert_eq!(
        audit.rls_scope_proof.map(|proof| proof.org_id),
        Some(OrgId::knl())
    );
    assert_eq!(
        audit
            .bundle_key
            .as_ref()
            .map(|key| key.bundle_digest.as_str()),
        Some("sha256:bundle")
    );
    assert_eq!(
        audit
            .evaluated_bundle_key
            .as_ref()
            .map(|key| key.bundle_digest.as_str()),
        Some("sha256:stale-bundle")
    );
    assert_eq!(audit.evaluated_reason_detail, None);
    assert_eq!(
        audit
            .bundle_key
            .as_ref()
            .map(|key| key.cedar_sdk_version.as_str()),
        Some("4.11.2")
    );
}

#[test]
fn cedar_pbac_observation_preserves_raw_cedar_reason_detail() {
    let branch = BranchId::new();
    let actor = principal(Role::SuperAdmin, BranchScope::single(branch));
    let request = cedar_ready(AuthorizationRequest::new(
        actor,
        Action::new(Feature::RoleManage),
        AuthorizationResource::branch(OrgId::knl(), branch, "policy_role"),
    ));
    let entry = coexistence_entry(DualEngineMode::CedarOnly);

    let cedar_deny = CedarEvaluation::Deny {
        bundle_key: cedar_bundle_key(),
        reason: "shadow policy denied principal role_manage".to_owned(),
    };
    let deny_decision = evaluate_cedar_pbac_boundary(&request, Some(&entry), cedar_deny.clone());
    let deny_audit =
        observe_cedar_pbac_decision(&request, Some(&entry), Some(&cedar_deny), deny_decision);
    assert_eq!(deny_audit.decision.reason, DecisionReason::CedarDenied);
    assert_eq!(
        deny_audit.evaluated_reason_detail.as_deref(),
        Some("shadow policy denied principal role_manage")
    );

    let cedar_error = CedarEvaluation::Error {
        reason: "schema validation failed".to_owned(),
    };
    let error_decision = evaluate_cedar_pbac_boundary(&request, Some(&entry), cedar_error.clone());
    let error_audit =
        observe_cedar_pbac_decision(&request, Some(&entry), Some(&cedar_error), error_decision);
    assert_eq!(error_audit.decision.reason, DecisionReason::CedarError);
    assert_eq!(
        error_audit.evaluated_reason_detail.as_deref(),
        Some("schema validation failed")
    );
}

#[test]
fn cedar_compiled_bundle_cache_key_requires_versioned_identity() {
    let missing_digest =
        CompiledBundleCacheKey::new(OrgId::knl(), 7, "schema-v1", " ", "4.11.2", "4.5")
            .unwrap_err();

    assert_eq!(missing_digest.kind, ErrorKind::Validation);
    assert!(missing_digest.message.contains("bundle_digest"));
    assert_eq!(cedar_bundle_key().cedar_sdk_version, "4.11.2");
}

#[test]
fn permission_matrix_is_exhaustive_and_matches_inherited_table() {
    let matrix = expected_matrix();
    assert_eq!(Feature::ALL.len(), 96);
    assert_eq!(matrix.len(), Feature::ALL.len());

    for feature in Feature::ALL {
        assert!(
            matrix.iter().any(|(candidate, _)| *candidate == feature),
            "missing matrix row for {feature:?}"
        );
    }

    for (feature, permissions) in matrix {
        for (role, expected) in ROLES.into_iter().zip(permissions) {
            assert_eq!(
                permission_for(role, feature),
                expected,
                "unexpected permission for {role:?} on {feature:?}"
            );
        }
    }
}

#[test]
fn org_wide_queue_triage_is_executive_and_super_admin_only() {
    // codex G001 HIGH-1: org-wide work-order/daily-plan queue visibility is gated
    // on the OrgWideQueueTriage capability, NOT a role string. It must mirror the
    // org-wide tier of `resolve_branch_scope_in_org` — EXECUTIVE + SUPER_ADMIN —
    // so a branch-scoped ADMIN is confined to its branches and does NOT see the
    // org-wide queue. `work_order_list_scope` widens to `BranchScope::All` exactly
    // when a principal holds this capability at `Allow`.
    let holds =
        |role: Role| permission_for(role, Feature::OrgWideQueueTriage) == PermissionLevel::Allow;
    assert!(
        holds(Role::Executive),
        "EXECUTIVE must hold OrgWideQueueTriage"
    );
    assert!(
        holds(Role::SuperAdmin),
        "SUPER_ADMIN must hold OrgWideQueueTriage"
    );
    assert!(
        !holds(Role::Admin),
        "a branch-scoped ADMIN must NOT hold OrgWideQueueTriage (no org-wide widen)"
    );
    assert!(!holds(Role::Receptionist), "RECEPTIONIST must NOT hold it");
    assert!(!holds(Role::Mechanic), "MECHANIC must NOT hold it");
    assert!(!holds(Role::Member), "MEMBER must NOT hold it");
}

#[test]
fn exit_workflow_capabilities_enforce_separation_of_duties() {
    // US-005: the absence → exit → settlement chain is split into distinct
    // capabilities so no single coarse gate authorizes the whole flow. Report,
    // HR-confirm and settlement are the branch HR/manager tier (ADMIN +
    // SUPER_ADMIN); HQ-confirm is the org-wide leadership tier (EXECUTIVE +
    // SUPER_ADMIN), so the two confirmation tiers land on DIFFERENT built-in
    // roles by default (ADMIN vs EXECUTIVE), with SUPER_ADMIN the only overlap
    // — and even then the confirm handler's distinct-actor rule forbids one
    // person doing both tiers on the same case.
    let allows =
        |role: Role, feature: Feature| permission_for(role, feature) == PermissionLevel::Allow;

    for (feature, holder, non_holder) in [
        (Feature::ExitCaseReport, Role::Admin, Role::Mechanic),
        (Feature::ExitCaseHrConfirm, Role::Admin, Role::Executive),
        (Feature::ExitCaseHqConfirm, Role::Executive, Role::Admin),
        (
            Feature::ExitSettlementManage,
            Role::Admin,
            Role::Receptionist,
        ),
    ] {
        assert!(allows(holder, feature), "{holder:?} must hold {feature:?}");
        assert!(
            allows(Role::SuperAdmin, feature),
            "SUPER_ADMIN must hold {feature:?}"
        );
        assert!(
            !allows(non_holder, feature),
            "{non_holder:?} must NOT hold {feature:?}"
        );
        assert!(
            !allows(Role::Member, feature),
            "MEMBER must NOT hold {feature:?}"
        );
    }

    // The two confirmation tiers must not both default to the same non-super
    // role, or the built-in matrix alone would let one role complete both tiers.
    assert!(
        !allows(Role::Admin, Feature::ExitCaseHqConfirm),
        "a branch ADMIN must NOT hold the HQ confirmation tier"
    );
    assert!(
        !allows(Role::Executive, Feature::ExitCaseHrConfirm),
        "an org-wide EXECUTIVE must NOT hold the HR confirmation tier"
    );
}

#[test]
fn daily_plan_list_gate_requires_daily_plan_or_org_wide_triage() {
    // codex G001 HIGH-2: the daily-plan LIST is a MECHANIC-requests / ADMIN-reviews
    // flow, gated on DailyPlanRequest OR DailyPlanReview — NOT the broad
    // WorkOrderReadAll (which RECEPTIONIST also passes). An org-wide queue triager
    // (EXECUTIVE / SUPER_ADMIN, via OrgWideQueueTriage) may ALSO read it for
    // org-wide oversight, mirroring their work-order-queue visibility. RECEPTIONIST
    // (none of these) stays denied. Mirrors `authorize_daily_plan_list` in
    // workorder/rest: a role passes iff it holds ANY of the three at `Allow`.
    let can_list = |role: Role| {
        permission_for(role, Feature::DailyPlanRequest) == PermissionLevel::Allow
            || permission_for(role, Feature::DailyPlanReview) == PermissionLevel::Allow
            || permission_for(role, Feature::OrgWideQueueTriage) == PermissionLevel::Allow
    };
    assert!(
        can_list(Role::Mechanic),
        "MECHANIC (request) must list daily plans"
    );
    assert!(
        can_list(Role::Admin),
        "ADMIN (review) must list daily plans"
    );
    assert!(
        can_list(Role::SuperAdmin),
        "SUPER_ADMIN must list daily plans"
    );
    assert!(
        can_list(Role::Executive),
        "EXECUTIVE (org-wide triager) may list daily plans for oversight"
    );
    assert!(
        !can_list(Role::Receptionist),
        "RECEPTIONIST has no daily-plan or triage capability and must be denied the list"
    );
    assert!(!can_list(Role::Member), "MEMBER must be denied the list");
}

#[test]
fn default_authorize_requires_full_allowance_and_matching_branch_scope() {
    let branch = BranchId::new();
    let actor = principal(Role::Admin, BranchScope::single(branch));

    authorize(&actor, Action::new(Feature::PriorityManage), branch).unwrap();

    let other_branch = BranchId::new();
    let err = authorize(&actor, Action::new(Feature::PriorityManage), other_branch).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Forbidden);
}

#[test]
fn request_only_permission_can_use_request_action_but_not_full_action() {
    let branch = BranchId::new();
    let actor = principal(Role::Mechanic, BranchScope::single(branch));

    authorize(&actor, Action::request(Feature::TargetManage), branch).unwrap();

    let err = authorize(&actor, Action::new(Feature::TargetManage), branch).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Forbidden);
}

#[test]
fn cross_branch_reads_and_writes_are_denied_by_default() {
    let branch = BranchId::new();
    let other_branch = BranchId::new();
    let actor = principal(Role::Admin, BranchScope::single(branch));

    let read_err = authorize(&actor, Action::new(Feature::WorkOrderReadAll), other_branch)
        .expect_err("read outside the actor scope must fail");
    assert_eq!(read_err.kind, ErrorKind::Forbidden);

    let write_err = authorize(&actor, Action::new(Feature::AssigneeManage), other_branch)
        .expect_err("write outside the actor scope must fail");
    assert_eq!(write_err.kind, ErrorKind::Forbidden);
}

#[test]
fn super_admin_spans_branches() {
    let actor = principal(Role::SuperAdmin, BranchScope::All);

    authorize(
        &actor,
        Action::new(Feature::ElevatedRoleGrant),
        BranchId::new(),
    )
    .unwrap();
}

#[test]
fn executive_is_read_only_even_with_all_branch_scope() {
    let actor = principal(Role::Executive, BranchScope::All);
    let branch = BranchId::new();

    authorize(&actor, Action::new(Feature::KpiRead), branch).unwrap();

    let err = authorize(&actor, Action::new(Feature::PriorityManage), branch).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Forbidden);
}

#[test]
fn empty_scope_denies_even_allowed_features() {
    let actor = principal(Role::Receptionist, BranchScope::none());

    let err = authorize(
        &actor,
        Action::new(Feature::WorkOrderReadAll),
        BranchId::new(),
    )
    .expect_err("empty explicit scope should deny by default");
    assert_eq!(err.kind, ErrorKind::Forbidden);
}

#[test]
fn effective_custom_grant_extends_authorize_without_widening_branch_scope() {
    let branch = BranchId::new();
    let other_branch = BranchId::new();
    let actor =
        principal(Role::Member, BranchScope::single(branch)).with_effective_feature_grants(vec![
            EffectiveFeatureGrant::new(
                Feature::WorkOrderCreate,
                PermissionLevel::Allow,
                BranchScope::single(branch),
            ),
        ]);

    authorize(&actor, Action::new(Feature::WorkOrderCreate), branch).unwrap();

    let static_denied =
        authorize(&actor, Action::new(Feature::PriorityManage), branch).unwrap_err();
    assert_eq!(static_denied.kind, ErrorKind::Forbidden);

    let cross_branch =
        authorize(&actor, Action::new(Feature::WorkOrderCreate), other_branch).unwrap_err();
    assert_eq!(cross_branch.kind, ErrorKind::Forbidden);
}

#[test]
fn effective_custom_grant_honors_permission_level_semantics() {
    let branch = BranchId::new();
    let actor =
        principal(Role::Member, BranchScope::single(branch)).with_effective_feature_grants(vec![
            EffectiveFeatureGrant::new(
                Feature::TargetManage,
                PermissionLevel::RequestOnly,
                BranchScope::single(branch),
            ),
        ]);

    authorize(&actor, Action::request(Feature::TargetManage), branch).unwrap();

    let full_action = authorize(&actor, Action::new(Feature::TargetManage), branch).unwrap_err();
    assert_eq!(full_action.kind, ErrorKind::Forbidden);
}

#[test]
fn org_wide_authorize_uses_effective_grants_but_never_widens_branch_grants() {
    let all_branch_actor =
        principal(Role::Member, BranchScope::All).with_effective_feature_grants(vec![
            EffectiveFeatureGrant::new(
                Feature::AuditLogRead,
                PermissionLevel::Allow,
                BranchScope::All,
            ),
        ]);

    authorize_org_wide(&all_branch_actor, Action::new(Feature::AuditLogRead)).unwrap();

    let branch = BranchId::new();
    let branch_only_actor = principal(Role::Member, BranchScope::All)
        .with_effective_feature_grants(vec![EffectiveFeatureGrant::new(
            Feature::AuditLogRead,
            PermissionLevel::Allow,
            BranchScope::single(branch),
        )]);
    let branch_only_err =
        authorize_org_wide(&branch_only_actor, Action::new(Feature::AuditLogRead))
            .expect_err("branch-scoped custom grants must not authorize all-branch reads");
    assert_eq!(branch_only_err.kind, ErrorKind::Forbidden);

    let no_all_scope_actor = principal(Role::Admin, BranchScope::single(branch));
    let no_all_scope_err =
        authorize_org_wide(&no_all_scope_actor, Action::new(Feature::AuditLogRead)).expect_err(
            "built-in permissions still need all-branch scope for branch-omitted org reads",
        );
    assert_eq!(no_all_scope_err.kind, ErrorKind::Forbidden);
}

#[test]
fn org_wide_authorize_rejects_builtin_admin_even_with_all_branch_scope() {
    let admin = principal(Role::Admin, BranchScope::All);
    let admin_err = authorize_org_wide(&admin, Action::new(Feature::KpiRead))
        .expect_err("ADMIN must not gain org-wide reads from an all-branch scope alone");
    assert_eq!(admin_err.kind, ErrorKind::Forbidden);

    let executive = principal(Role::Executive, BranchScope::All);
    authorize_org_wide(&executive, Action::new(Feature::KpiRead))
        .expect("EXECUTIVE remains an org-wide built-in role for KPI reads");

    let super_admin = principal(Role::SuperAdmin, BranchScope::All);
    authorize_org_wide(&super_admin, Action::new(Feature::KpiRead))
        .expect("SUPER_ADMIN remains an org-wide built-in role for KPI reads");

    let custom_all_actor =
        principal(Role::Member, BranchScope::All).with_effective_feature_grants(vec![
            EffectiveFeatureGrant::new(Feature::KpiRead, PermissionLevel::Allow, BranchScope::All),
        ]);
    authorize_org_wide(&custom_all_actor, Action::new(Feature::KpiRead))
        .expect("tenant-owned custom All grants may authorize org-wide reads");
}

#[test]
fn repository_filter_helper_is_default_deny_and_uses_binds() {
    let column = BranchColumn::new("work_orders.branch_id").unwrap();

    let all = repository_filter(&BranchScope::All, column).unwrap();
    assert_eq!(all.sql(), "TRUE");
    assert!(all.branch_ids().is_empty());

    let none = repository_filter(&BranchScope::none(), column).unwrap();
    assert_eq!(none.sql(), "FALSE");
    assert!(none.branch_ids().is_empty());

    let branch = BranchId::new();
    let scoped = repository_filter(&BranchScope::single(branch), column).unwrap();
    assert_eq!(scoped.sql(), "work_orders.branch_id = ANY($1)");
    assert_eq!(scoped.branch_ids(), &[*branch.as_uuid()]);
}

#[test]
fn repository_filter_rejects_unsafe_column_names() {
    let err = BranchColumn::new("branch_id; DROP TABLE audit_events").unwrap_err();
    assert_eq!(err.kind, ErrorKind::Validation);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn resolves_user_branch_scope_from_memberships(pool: PgPool) {
    let user_id = seed_user_with_two_branches_and_one_membership(&pool, "ADMIN").await;
    let scope = resolve_branch_scope_in_org(
        &pool,
        OrgId::knl(),
        UserId::from_uuid(user_id.user),
        &[Role::Admin],
    )
    .await
    .unwrap();

    assert!(scope.allows(BranchId::from_uuid(user_id.member_branch)));
    assert!(!scope.allows(BranchId::from_uuid(user_id.other_branch)));
}

#[sqlx::test(migrations = "../db/migrations")]
async fn resolves_super_admin_to_all_scope_without_memberships(pool: PgPool) {
    let user: uuid::Uuid = sqlx::query_scalar(
        "INSERT INTO users (display_name, roles, org_id) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind("Global Admin")
    .bind(Vec::from(["SUPER_ADMIN"]))
    .bind(*OrgId::knl().as_uuid())
    .fetch_one(&pool)
    .await
    .unwrap();

    let scope = resolve_branch_scope_in_org(
        &pool,
        OrgId::knl(),
        UserId::from_uuid(user),
        &[Role::SuperAdmin],
    )
    .await
    .unwrap();

    assert_eq!(scope, BranchScope::All);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn resolves_active_custom_policy_role_grants_fail_closed(pool: PgPool) {
    let seeded = seed_user_with_two_branches_and_one_membership(&pool, "MEMBER").await;
    let user_id = UserId::from_uuid(seeded.user);
    let member_branch = BranchId::from_uuid(seeded.member_branch);
    let other_branch = BranchId::from_uuid(seeded.other_branch);
    sqlx::query("UPDATE users SET team = '정비' WHERE id = $1")
        .bind(seeded.user)
        .execute(&pool)
        .await
        .unwrap();

    let active_role = seed_policy_role(
        &pool,
        "custom_work_order_creator",
        "ACTIVE",
        &[
            (Feature::WorkOrderCreate, PermissionLevel::Allow),
            (Feature::RoleManage, PermissionLevel::Allow),
        ],
        &[],
    )
    .await;
    let draft_role = seed_policy_role(
        &pool,
        "draft_priority_manager",
        "DRAFT",
        &[(Feature::PriorityManage, PermissionLevel::Allow)],
        &[],
    )
    .await;
    let branch_mismatch_role = seed_policy_role(
        &pool,
        "other_branch_starter",
        "ACTIVE",
        &[(Feature::WorkOrderStart, PermissionLevel::Allow)],
        &[("branch", "equals", vec![other_branch.to_string()])],
    )
    .await;
    let team_match_role = seed_policy_role(
        &pool,
        "maintenance_team_reporter",
        "ACTIVE",
        &[(Feature::WorkReportSubmit, PermissionLevel::Allow)],
        &[(
            "team",
            "in",
            vec!["MAINTENANCE".to_owned(), "예방".to_owned()],
        )],
    )
    .await;
    let team_mismatch_role = seed_policy_role(
        &pool,
        "reception_team_reviewer",
        "ACTIVE",
        &[(Feature::CompletionReview, PermissionLevel::Allow)],
        &[("team", "equals", vec!["RECEPTION".to_owned()])],
    )
    .await;
    let unsupported_condition_role = seed_policy_role(
        &pool,
        "department_evidence",
        "ACTIVE",
        &[(Feature::EvidenceAttach, PermissionLevel::Allow)],
        &[("department", "equals", vec!["field-service".to_owned()])],
    )
    .await;
    let negative_branch_condition_role = seed_policy_role(
        &pool,
        "negative_branch_evidence",
        "ACTIVE",
        &[(Feature::WorkOrderEditIntake, PermissionLevel::Allow)],
        &[("branch", "not_equals", vec![member_branch.to_string()])],
    )
    .await;
    let invalid_branch_condition_role = seed_policy_role(
        &pool,
        "invalid_branch_assignee",
        "ACTIVE",
        &[(Feature::AssigneeManage, PermissionLevel::Allow)],
        &[("branch", "equals", vec!["not-a-branch-uuid".to_owned()])],
    )
    .await;
    assign_policy_roles(
        &pool,
        seeded.user,
        &[
            active_role,
            draft_role,
            branch_mismatch_role,
            team_match_role,
            team_mismatch_role,
            unsupported_condition_role,
            negative_branch_condition_role,
            invalid_branch_condition_role,
        ],
    )
    .await;

    let grants = resolve_effective_feature_grants_in_org(
        &pool,
        OrgId::knl(),
        user_id,
        &BranchScope::single(member_branch),
    )
    .await
    .unwrap();

    assert_eq!(
        grants.len(),
        2,
        "only active ordinary in-scope grants with supported matching conditions apply"
    );
    assert!(
        grants
            .iter()
            .any(|grant| grant.feature == Feature::WorkOrderCreate
                && grant.permission == PermissionLevel::Allow)
    );
    assert!(
        grants
            .iter()
            .any(|grant| grant.feature == Feature::WorkReportSubmit
                && grant.permission == PermissionLevel::Allow)
    );

    let actor = principal(Role::Member, BranchScope::single(member_branch))
        .with_effective_feature_grants(grants);
    authorize(&actor, Action::new(Feature::WorkOrderCreate), member_branch).unwrap();
    authorize(
        &actor,
        Action::new(Feature::WorkReportSubmit),
        member_branch,
    )
    .expect("matching team ABAC condition should become runtime-effective");

    let elevated = authorize(&actor, Action::new(Feature::RoleManage), member_branch).unwrap_err();
    assert_eq!(elevated.kind, ErrorKind::Forbidden);

    let inactive = authorize(&actor, Action::new(Feature::PriorityManage), member_branch)
        .expect_err("draft custom roles must not become effective");
    assert_eq!(inactive.kind, ErrorKind::Forbidden);

    let condition_mismatch = authorize(&actor, Action::new(Feature::WorkOrderStart), member_branch)
        .expect_err("branch condition mismatch must fail closed");
    assert_eq!(condition_mismatch.kind, ErrorKind::Forbidden);

    let team_mismatch = authorize(
        &actor,
        Action::new(Feature::CompletionReview),
        member_branch,
    )
    .expect_err("team condition mismatch must fail closed");
    assert_eq!(team_mismatch.kind, ErrorKind::Forbidden);

    let unsupported = authorize(&actor, Action::new(Feature::EvidenceAttach), member_branch)
        .expect_err("unsupported runtime ABAC conditions must fail closed");
    assert_eq!(unsupported.kind, ErrorKind::Forbidden);

    let negative_operator = authorize(
        &actor,
        Action::new(Feature::WorkOrderEditIntake),
        member_branch,
    )
    .expect_err("negative branch operators must fail closed for runtime grants");
    assert_eq!(negative_operator.kind, ErrorKind::Forbidden);

    let invalid_branch = authorize(&actor, Action::new(Feature::AssigneeManage), member_branch)
        .expect_err("invalid branch condition values must fail closed");
    assert_eq!(invalid_branch.kind, ErrorKind::Forbidden);
}

struct SeededBranches {
    user: uuid::Uuid,
    member_branch: uuid::Uuid,
    other_branch: uuid::Uuid,
}

async fn seed_user_with_two_branches_and_one_membership(
    pool: &PgPool,
    role: &str,
) -> SeededBranches {
    let region_id: uuid::Uuid =
        sqlx::query_scalar("INSERT INTO regions (name, org_id) VALUES ($1, $2) RETURNING id")
            .bind(format!("Region {role}"))
            .bind(*OrgId::knl().as_uuid())
            .fetch_one(pool)
            .await
            .unwrap();

    let member_branch: uuid::Uuid = sqlx::query_scalar(
        "INSERT INTO branches (region_id, name, org_id) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(region_id)
    .bind(format!("Member {role}"))
    .bind(*OrgId::knl().as_uuid())
    .fetch_one(pool)
    .await
    .unwrap();

    let other_branch: uuid::Uuid = sqlx::query_scalar(
        "INSERT INTO branches (region_id, name, org_id) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(region_id)
    .bind(format!("Other {role}"))
    .bind(*OrgId::knl().as_uuid())
    .fetch_one(pool)
    .await
    .unwrap();

    let user: uuid::Uuid = sqlx::query_scalar(
        "INSERT INTO users (display_name, roles, org_id) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(format!("User {role}"))
    .bind(Vec::from([role]))
    .bind(*OrgId::knl().as_uuid())
    .fetch_one(pool)
    .await
    .unwrap();

    sqlx::query("INSERT INTO user_branches (user_id, branch_id, org_id) VALUES ($1, $2, $3)")
        .bind(user)
        .bind(member_branch)
        .bind(*OrgId::knl().as_uuid())
        .execute(pool)
        .await
        .unwrap();

    SeededBranches {
        user,
        member_branch,
        other_branch,
    }
}

async fn seed_policy_role(
    pool: &PgPool,
    role_key: &str,
    status: &str,
    permissions: &[(Feature, PermissionLevel)],
    conditions: &[(&str, &str, Vec<String>)],
) -> uuid::Uuid {
    let role_id: uuid::Uuid = sqlx::query_scalar(
        r#"
        INSERT INTO policy_roles (
            org_id, role_key, display_name, status, is_system
        ) VALUES ($1, $2, $3, $4, false)
        RETURNING id
        "#,
    )
    .bind(*OrgId::knl().as_uuid())
    .bind(role_key)
    .bind(format!("Role {role_key}"))
    .bind(status)
    .fetch_one(pool)
    .await
    .unwrap();

    for (feature, permission) in permissions {
        sqlx::query(
            r#"
            INSERT INTO policy_role_permissions (
                org_id, role_id, feature_key, permission_level
            ) VALUES ($1, $2, $3, $4)
            "#,
        )
        .bind(*OrgId::knl().as_uuid())
        .bind(role_id)
        .bind(feature.as_str())
        .bind(permission.as_str())
        .execute(pool)
        .await
        .unwrap();
    }

    for (index, (attribute, operator, values)) in conditions.iter().enumerate() {
        sqlx::query(
            r#"
            INSERT INTO policy_role_conditions (
                org_id, role_id, condition_key, attribute, operator, condition_values
            ) VALUES ($1, $2, $3, $4, $5, $6)
            "#,
        )
        .bind(*OrgId::knl().as_uuid())
        .bind(role_id)
        .bind(format!("condition_{index}"))
        .bind(attribute)
        .bind(operator)
        .bind(values)
        .execute(pool)
        .await
        .unwrap();
    }

    role_id
}

async fn assign_policy_roles(pool: &PgPool, user_id: uuid::Uuid, role_ids: &[uuid::Uuid]) {
    for role_id in role_ids {
        sqlx::query(
            r#"
            INSERT INTO user_role_assignments (org_id, user_id, role_id)
            VALUES ($1, $2, $3)
            "#,
        )
        .bind(*OrgId::knl().as_uuid())
        .bind(user_id)
        .bind(role_id)
        .execute(pool)
        .await
        .unwrap();
    }
}

/// `ResourceBranch::lookup` is the ONLY production way into a `ResourceBranch`,
/// so it is proven against a real row rather than trusted.
///
/// The branch it yields must be the RESOURCE's branch — allowed for the branch
/// the principal is a member of, denied for the sibling it is not. The caller
/// picks the row and the resource KIND; it does not pick the table or the
/// column, so it has no way to aim this at the principal's own membership row.
#[sqlx::test(migrations = "../db/migrations")]
async fn a_resource_branch_looked_up_by_id_decides_as_the_resources_branch(pool: PgPool) {
    let seeded = seed_user_with_two_branches_and_one_membership(&pool, "ADMIN").await;
    let principal = Principal::new(
        UserId::from_uuid(seeded.user),
        OrgId::knl(),
        BTreeSet::from([Role::Admin]),
        BranchScope::single(BranchId::from_uuid(seeded.member_branch)),
    );
    let action = Action::new(Feature::CompletionReview);
    let at = Timestamp::now_utc();

    for (branch, expected_ok) in [(seeded.member_branch, true), (seeded.other_branch, false)] {
        let customer: uuid::Uuid = sqlx::query_scalar(
            "INSERT INTO registry_customers (branch_id, name, org_id)
             VALUES ($1, $2, $3) RETURNING id",
        )
        .bind(branch)
        .bind(format!("Customer {branch}"))
        .bind(*OrgId::knl().as_uuid())
        .fetch_one(&pool)
        .await
        .unwrap();

        let resource_branch = ResourceBranch::lookup(
            &pool,
            principal.org_id,
            BranchScopedResource::RegistryCustomer,
            customer,
        )
        .await
        .unwrap();

        // Asserted through the door, because the door is the only way out of a
        // `ResourceBranch` from outside this crate: the accessor is
        // crate-private so that no caller can unwrap the branch and re-enter
        // via `authorize`, which has no tenant to compare. The pair below is
        // the same claim an equality on the branch would make — a lookup that
        // returned the PRINCIPAL's branch instead of the resource's would allow
        // both rows.
        assert_eq!(
            authorize_scoped(&principal, action, resource_branch, at).is_ok(),
            expected_ok,
            "a branch looked up by resource id must decide as the RESOURCE's branch"
        );
    }
}

/// A resource that does not exist has no branch, and that must surface as a
/// typed refusal. It must NOT collapse into the branch-less (`None`) arm:
/// "the row is missing" and "the row has no branch" authorize differently.
#[sqlx::test(migrations = "../db/migrations")]
async fn a_missing_resource_row_is_a_typed_refusal_not_a_branchless_pass(pool: PgPool) {
    let absent = ResourceBranch::lookup(
        &pool,
        OrgId::knl(),
        BranchScopedResource::RegistryCustomer,
        uuid::Uuid::new_v4(),
    )
    .await
    .unwrap_err();

    assert_eq!(absent.kind, ErrorKind::NotFound);
}

/// CROSS-TENANT. "This uuid came back from Postgres" is not the claim — "this is
/// THIS TENANT's resource branch" is. A row id is a bare uuid on the wire and
/// nothing about it names an org, so a lookup that does not bind the tenant
/// resolves another tenant's row and hands back a branch the principal's own org
/// never contained. The whole point of the type dissolves one org over.
#[sqlx::test(migrations = "../db/migrations")]
async fn a_resource_branch_lookup_cannot_resolve_another_tenants_resource(pool: PgPool) {
    let (other_org, other_customer) = seed_other_tenant_customer(&pool).await;

    let leaked = ResourceBranch::lookup(
        &pool,
        OrgId::knl(),
        BranchScopedResource::RegistryCustomer,
        other_customer,
    )
    .await
    .expect_err("org knl must not resolve another tenant's resource branch");
    assert_eq!(leaked.kind, ErrorKind::NotFound);

    // The control: the row is genuinely there, and its OWN tenant reads it.
    ResourceBranch::lookup(
        &pool,
        other_org,
        BranchScopedResource::RegistryCustomer,
        other_customer,
    )
    .await
    .expect("the owning tenant must still resolve its own resource branch");
}

/// The org `lookup` binds is an ARGUMENT, and the shape a handler reaches for
/// first is the one on the request path — `path.org_id`, which the caller of the
/// API chose. So the tenant the row was read under is re-checked against the
/// principal at the decision.
///
/// Without that, this is the whole attack: look the resource up under ITS org
/// (the predicate is satisfied — it is that tenant's row), then authorize a
/// principal from a different tenant against it. A `BranchScope::All` principal
/// allows every branch, so the branch check cannot object, and the answer would
/// be ALLOW against another tenant's resource.
#[sqlx::test(migrations = "../db/migrations")]
async fn a_resource_branch_read_under_another_tenant_does_not_authorize(pool: PgPool) {
    let (other_org, other_customer) = seed_other_tenant_customer(&pool).await;
    let principal = Principal::new(
        UserId::new(),
        OrgId::knl(),
        BTreeSet::from([Role::Admin]),
        BranchScope::All,
    );
    let action = Action::new(Feature::CompletionReview);
    let at = Timestamp::now_utc();

    // The lookup succeeds: it is that tenant's row, read under that tenant.
    let other_tenants_branch = ResourceBranch::lookup(
        &pool,
        other_org,
        BranchScopedResource::RegistryCustomer,
        other_customer,
    )
    .await
    .unwrap();

    let err = authorize_scoped(&principal, action, other_tenants_branch, at)
        .expect_err("a knl principal must not be authorized against another tenant's resource");
    assert_eq!(err.kind, ErrorKind::Forbidden);

    // THE CONTROL: the same principal, the same action, the same wide scope,
    // against a resource in its OWN tenant — so the refusal above is the tenant
    // and nothing else.
    let seeded = seed_user_with_two_branches_and_one_membership(&pool, "ADMIN").await;
    let own_customer: uuid::Uuid = sqlx::query_scalar(
        "INSERT INTO registry_customers (branch_id, name, org_id)
         VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(seeded.member_branch)
    .bind("KNL Customer")
    .bind(*OrgId::knl().as_uuid())
    .fetch_one(&pool)
    .await
    .unwrap();
    let own_branch = ResourceBranch::lookup(
        &pool,
        OrgId::knl(),
        BranchScopedResource::RegistryCustomer,
        own_customer,
    )
    .await
    .unwrap();

    assert!(authorize_scoped(&principal, action, own_branch, at).is_ok());
}

async fn seed_other_tenant_customer(pool: &PgPool) -> (OrgId, uuid::Uuid) {
    let org: uuid::Uuid =
        sqlx::query_scalar("INSERT INTO organizations (slug, name) VALUES ($1, $2) RETURNING id")
            .bind("other-tenant")
            .bind("Other Tenant")
            .fetch_one(pool)
            .await
            .unwrap();

    let region: uuid::Uuid =
        sqlx::query_scalar("INSERT INTO regions (name, org_id) VALUES ($1, $2) RETURNING id")
            .bind("Other Tenant Region")
            .bind(org)
            .fetch_one(pool)
            .await
            .unwrap();

    let branch: uuid::Uuid = sqlx::query_scalar(
        "INSERT INTO branches (region_id, name, org_id) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(region)
    .bind("Other Tenant Branch")
    .bind(org)
    .fetch_one(pool)
    .await
    .unwrap();

    let customer: uuid::Uuid = sqlx::query_scalar(
        "INSERT INTO registry_customers (branch_id, name, org_id)
         VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(branch)
    .bind("Other Tenant Customer")
    .bind(org)
    .fetch_one(pool)
    .await
    .unwrap();

    (OrgId::from_uuid(org), customer)
}

/// `assignment_validities` stamps every 발령 with `GrantValidity::always()`,
/// and that is correct only while `user_role_assignments` has no interval
/// columns. When ADR-0032's other half lands them and that function is not
/// changed to read them, two legitimately consecutive 발령 of one role —
/// `[100, 200)` then `[200, 300)`, the case
/// `touching_half_open_intervals_for_one_role_do_not_overlap` exists to permit —
/// become two `always()` intervals that DO overlap, and
/// `resolve_effective_feature_grants_in_org` returns `Conflict` instead of that
/// user's whole custom-role authority.
///
/// A comment saying so is not an obligation, so the absence is asserted against
/// the live schema instead: the migration that adds the columns turns this red,
/// and the message names the function that must change with it.
#[sqlx::test(migrations = "../db/migrations")]
async fn assignment_validity_is_stamped_only_while_the_schema_has_no_intervals(pool: PgPool) {
    let columns: Vec<String> = sqlx::query_scalar(
        "SELECT column_name::text FROM information_schema.columns
         WHERE table_name = 'user_role_assignments'
           AND column_name IN ('valid_from', 'valid_to')
         ORDER BY column_name",
    )
    .fetch_all(&pool)
    .await
    .unwrap();

    assert!(
        columns.is_empty(),
        "user_role_assignments now carries {columns:?}. `assignment_validities` in \
         platform/authz/src/lib.rs must read them instead of stamping \
         GrantValidity::always(), or every user holding two consecutive appointments \
         of one role loses their entire custom-role authority"
    );
}

/// TOTALITY. Every `BranchScopedResource` variant must name a table and a branch
/// column that really exist: a wrong name is a database error, which surfaces as
/// `Internal`, never as `NotFound`. Without this, a variant added with a typo
/// would fail every authorization at runtime and no test would say why.
#[sqlx::test(migrations = "../db/migrations")]
async fn every_resource_kind_names_a_table_and_branch_column_that_exist(pool: PgPool) {
    for &resource in BranchScopedResource::ALL {
        let err = ResourceBranch::lookup(&pool, OrgId::knl(), resource, uuid::Uuid::new_v4())
            .await
            .unwrap_err();

        assert_eq!(
            err.kind,
            ErrorKind::NotFound,
            "{resource:?} did not resolve against the live schema: {}",
            err.message
        );
    }
}
