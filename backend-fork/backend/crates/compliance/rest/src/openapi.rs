//! This face's slice of the published OpenAPI contract.
//!
//! Bodies live as YAML under `openapi/` and are pulled in via `include_str!`.
//! `console_contracts` re-indents them; composition rejects duplicate keys.

use console_contracts::{Fragment, NamedYaml, Operation, PathItem};

/// This face's contribution to the composed OpenAPI document.
pub const OPENAPI_FRAGMENT: Fragment = Fragment {
    source: "console-compliance-rest",
    paths: PATHS,
    schemas: SCHEMAS,
    parameters: &[],
    responses: &[],
    security_schemes: &[],
    external_schemas: EXTERNAL_SCHEMAS,
};

const EXTERNAL_SCHEMAS: &[&str] = &["Timestamp", "Uuid"];

const PATHS: &[PathItem] = &[
    PathItem {
        path: "/api/v1/audit-streams/ceo-covert/access-events",
        operations: &[Operation {
            method: "get",
            body: include_str!(
                "../openapi/paths/api__v1__audit-streams__ceo-covert__access-events.get.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/audit-streams/ceo-covert/events",
        operations: &[Operation {
            method: "get",
            body: include_str!(
                "../openapi/paths/api__v1__audit-streams__ceo-covert__events.get.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/compliance/control-obligation-coverage",
        operations: &[Operation {
            method: "post",
            body: include_str!(
                "../openapi/paths/api__v1__compliance__control-obligation-coverage.post.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/compliance/evidence-bindings",
        operations: &[
            Operation {
                method: "get",
                body: include_str!(
                    "../openapi/paths/api__v1__compliance__evidence-bindings.get.yaml"
                ),
            },
            Operation {
                method: "post",
                body: include_str!(
                    "../openapi/paths/api__v1__compliance__evidence-bindings.post.yaml"
                ),
            },
        ],
    },
    PathItem {
        path: "/api/v1/compliance/evidence-bindings/{id}/accept",
        operations: &[Operation {
            method: "post",
            body: include_str!(
                "../openapi/paths/api__v1__compliance__evidence-bindings__id__accept.post.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/compliance/framework-controls",
        operations: &[
            Operation {
                method: "get",
                body: include_str!(
                    "../openapi/paths/api__v1__compliance__framework-controls.get.yaml"
                ),
            },
            Operation {
                method: "post",
                body: include_str!(
                    "../openapi/paths/api__v1__compliance__framework-controls.post.yaml"
                ),
            },
        ],
    },
    PathItem {
        path: "/api/v1/compliance/frameworks",
        operations: &[
            Operation {
                method: "get",
                body: include_str!("../openapi/paths/api__v1__compliance__frameworks.get.yaml"),
            },
            Operation {
                method: "post",
                body: include_str!("../openapi/paths/api__v1__compliance__frameworks.post.yaml"),
            },
        ],
    },
    PathItem {
        path: "/api/v1/compliance/obligation-regulation-links",
        operations: &[Operation {
            method: "post",
            body: include_str!(
                "../openapi/paths/api__v1__compliance__obligation-regulation-links.post.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/compliance/obligations",
        operations: &[
            Operation {
                method: "get",
                body: include_str!("../openapi/paths/api__v1__compliance__obligations.get.yaml"),
            },
            Operation {
                method: "post",
                body: include_str!("../openapi/paths/api__v1__compliance__obligations.post.yaml"),
            },
        ],
    },
    PathItem {
        path: "/api/v1/compliance/regulations",
        operations: &[
            Operation {
                method: "get",
                body: include_str!("../openapi/paths/api__v1__compliance__regulations.get.yaml"),
            },
            Operation {
                method: "post",
                body: include_str!("../openapi/paths/api__v1__compliance__regulations.post.yaml"),
            },
        ],
    },
    PathItem {
        path: "/api/v1/location-consent/grant",
        operations: &[Operation {
            method: "post",
            body: include_str!("../openapi/paths/api__v1__location-consent__grant.post.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/location-consent/resume",
        operations: &[Operation {
            method: "post",
            body: include_str!("../openapi/paths/api__v1__location-consent__resume.post.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/location-consent/status",
        operations: &[Operation {
            method: "get",
            body: include_str!("../openapi/paths/api__v1__location-consent__status.get.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/location-consent/suspend",
        operations: &[Operation {
            method: "post",
            body: include_str!("../openapi/paths/api__v1__location-consent__suspend.post.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/location-consent/withdraw",
        operations: &[Operation {
            method: "post",
            body: include_str!("../openapi/paths/api__v1__location-consent__withdraw.post.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/location-consents/ledger",
        operations: &[Operation {
            method: "get",
            body: include_str!("../openapi/paths/api__v1__location-consents__ledger.get.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/location-consents/ledger.csv",
        operations: &[Operation {
            method: "get",
            body: include_str!("../openapi/paths/api__v1__location-consents__ledger.csv.get.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/location-pings",
        operations: &[Operation {
            method: "post",
            body: include_str!("../openapi/paths/api__v1__location-pings.post.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/location/arrival-events",
        operations: &[Operation {
            method: "get",
            body: include_str!("../openapi/paths/api__v1__location__arrival-events.get.yaml"),
        }],
    },
];

const SCHEMAS: &[NamedYaml] = &[
    NamedYaml {
        name: "ArrivalEvent",
        body: include_str!("../openapi/schemas/ArrivalEvent.yaml"),
    },
    NamedYaml {
        name: "ArrivalEventPage",
        body: include_str!("../openapi/schemas/ArrivalEventPage.yaml"),
    },
    NamedYaml {
        name: "AuditStreamPage",
        body: include_str!("../openapi/schemas/AuditStreamPage.yaml"),
    },
    NamedYaml {
        name: "AuditStreamReadKind",
        body: include_str!("../openapi/schemas/AuditStreamReadKind.yaml"),
    },
    NamedYaml {
        name: "AuditStreamRecord",
        body: include_str!("../openapi/schemas/AuditStreamRecord.yaml"),
    },
    NamedYaml {
        name: "ComplianceControl",
        body: include_str!("../openapi/schemas/ComplianceControl.yaml"),
    },
    NamedYaml {
        name: "ComplianceControlPage",
        body: include_str!("../openapi/schemas/ComplianceControlPage.yaml"),
    },
    NamedYaml {
        name: "ComplianceFramework",
        body: include_str!("../openapi/schemas/ComplianceFramework.yaml"),
    },
    NamedYaml {
        name: "ComplianceFrameworkPage",
        body: include_str!("../openapi/schemas/ComplianceFrameworkPage.yaml"),
    },
    NamedYaml {
        name: "ComplianceObligation",
        body: include_str!("../openapi/schemas/ComplianceObligation.yaml"),
    },
    NamedYaml {
        name: "ComplianceObligationPage",
        body: include_str!("../openapi/schemas/ComplianceObligationPage.yaml"),
    },
    NamedYaml {
        name: "ComplianceRiskLevel",
        body: include_str!("../openapi/schemas/ComplianceRiskLevel.yaml"),
    },
    NamedYaml {
        name: "ComplianceScope",
        body: include_str!("../openapi/schemas/ComplianceScope.yaml"),
    },
    NamedYaml {
        name: "ComplianceScopeKind",
        body: include_str!("../openapi/schemas/ComplianceScopeKind.yaml"),
    },
    NamedYaml {
        name: "ControlObligationCoverage",
        body: include_str!("../openapi/schemas/ControlObligationCoverage.yaml"),
    },
    NamedYaml {
        name: "ControlStatus",
        body: include_str!("../openapi/schemas/ControlStatus.yaml"),
    },
    NamedYaml {
        name: "ControlType",
        body: include_str!("../openapi/schemas/ControlType.yaml"),
    },
    NamedYaml {
        name: "CoverageLevel",
        body: include_str!("../openapi/schemas/CoverageLevel.yaml"),
    },
    NamedYaml {
        name: "CoverageStatus",
        body: include_str!("../openapi/schemas/CoverageStatus.yaml"),
    },
    NamedYaml {
        name: "CreateComplianceControlRequest",
        body: include_str!("../openapi/schemas/CreateComplianceControlRequest.yaml"),
    },
    NamedYaml {
        name: "CreateComplianceFrameworkRequest",
        body: include_str!("../openapi/schemas/CreateComplianceFrameworkRequest.yaml"),
    },
    NamedYaml {
        name: "CreateComplianceObligationRequest",
        body: include_str!("../openapi/schemas/CreateComplianceObligationRequest.yaml"),
    },
    NamedYaml {
        name: "CreateEvidenceBindingRequest",
        body: include_str!("../openapi/schemas/CreateEvidenceBindingRequest.yaml"),
    },
    NamedYaml {
        name: "CreateRegulationImpactRequest",
        body: include_str!("../openapi/schemas/CreateRegulationImpactRequest.yaml"),
    },
    NamedYaml {
        name: "EvidenceBinding",
        body: include_str!("../openapi/schemas/EvidenceBinding.yaml"),
    },
    NamedYaml {
        name: "EvidenceBindingPage",
        body: include_str!("../openapi/schemas/EvidenceBindingPage.yaml"),
    },
    NamedYaml {
        name: "EvidenceBindingStatus",
        body: include_str!("../openapi/schemas/EvidenceBindingStatus.yaml"),
    },
    NamedYaml {
        name: "EvidenceConfidence",
        body: include_str!("../openapi/schemas/EvidenceConfidence.yaml"),
    },
    NamedYaml {
        name: "EvidenceTargetType",
        body: include_str!("../openapi/schemas/EvidenceTargetType.yaml"),
    },
    NamedYaml {
        name: "FrameworkKind",
        body: include_str!("../openapi/schemas/FrameworkKind.yaml"),
    },
    NamedYaml {
        name: "FrameworkStatus",
        body: include_str!("../openapi/schemas/FrameworkStatus.yaml"),
    },
    NamedYaml {
        name: "LinkControlObligationRequest",
        body: include_str!("../openapi/schemas/LinkControlObligationRequest.yaml"),
    },
    NamedYaml {
        name: "LinkObligationRegulationRequest",
        body: include_str!("../openapi/schemas/LinkObligationRegulationRequest.yaml"),
    },
    NamedYaml {
        name: "LocationConsentLedgerEntry",
        body: include_str!("../openapi/schemas/LocationConsentLedgerEntry.yaml"),
    },
    NamedYaml {
        name: "LocationConsentLedgerPage",
        body: include_str!("../openapi/schemas/LocationConsentLedgerPage.yaml"),
    },
    NamedYaml {
        name: "LocationConsentState",
        body: include_str!("../openapi/schemas/LocationConsentState.yaml"),
    },
    NamedYaml {
        name: "LocationConsentStatus",
        body: include_str!("../openapi/schemas/LocationConsentStatus.yaml"),
    },
    NamedYaml {
        name: "LocationConsentTransitionRequest",
        body: include_str!("../openapi/schemas/LocationConsentTransitionRequest.yaml"),
    },
    NamedYaml {
        name: "LocationPingRequest",
        body: include_str!("../openapi/schemas/LocationPingRequest.yaml"),
    },
    NamedYaml {
        name: "ObligationRegulationLink",
        body: include_str!("../openapi/schemas/ObligationRegulationLink.yaml"),
    },
    NamedYaml {
        name: "ObligationRegulationRelationship",
        body: include_str!("../openapi/schemas/ObligationRegulationRelationship.yaml"),
    },
    NamedYaml {
        name: "ObligationStatus",
        body: include_str!("../openapi/schemas/ObligationStatus.yaml"),
    },
    NamedYaml {
        name: "ObligationType",
        body: include_str!("../openapi/schemas/ObligationType.yaml"),
    },
    NamedYaml {
        name: "RegulationImpact",
        body: include_str!("../openapi/schemas/RegulationImpact.yaml"),
    },
    NamedYaml {
        name: "RegulationImpactPage",
        body: include_str!("../openapi/schemas/RegulationImpactPage.yaml"),
    },
    NamedYaml {
        name: "RegulationImpactStatus",
        body: include_str!("../openapi/schemas/RegulationImpactStatus.yaml"),
    },
    NamedYaml {
        name: "RegulationLinkRequest",
        body: include_str!("../openapi/schemas/RegulationLinkRequest.yaml"),
    },
];
