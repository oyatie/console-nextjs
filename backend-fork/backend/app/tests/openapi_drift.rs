#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::{BTreeMap, BTreeSet};
use std::str::FromStr;

use console_app::{AUDIT_ROUTE_PATH, CONFIGURED_ROUTE_SURFACES};
use console_ontology_canonical_domain::DispatchTarget;
use console_platform_rest::PLATFORM_ROUTE_OPERATIONS;

const OPENAPI_YAML: &str = include_str!("../../openapi/openapi.yaml");
const REQUIRED_CONFIGURED_SURFACES: &[&str] = &[
    "audit",
    "attendance",
    "inventory",
    "dispatch",
    "benefit",
    "financial",
    "evaluation",
    "integrity",
    "hr",
    "browser-payslips",
    "workflow-studio",
    "collaboration",
    "sales",
    "workorder",
    "workorder-mobile",
    "comms",
    "platform",
    "auth",
    "realtime",
    "ontology",
    "governance",
    "policy",
];

struct RouteSource {
    name: &'static str,
    surface: &'static str,
    source: &'static str,
    ignored_route_refs: &'static [&'static str],
}

const CONFIGURED_ROUTE_SOURCES: &[RouteSource] = &[
    RouteSource {
        name: "issued payslip browser adapter",
        surface: "browser-payslips",
        source: include_str!("../src/browser_payslips.rs"),
        ignored_route_refs: &[],
    },
    RouteSource {
        name: "attendance REST router",
        surface: "attendance",
        source: include_str!("../../crates/attendance/rest/src/lib.rs"),
        ignored_route_refs: &[],
    },
    RouteSource {
        name: "inventory REST router",
        surface: "inventory",
        source: include_str!("../../crates/inventory/rest/src/lib.rs"),
        ignored_route_refs: &[],
    },
    RouteSource {
        name: "dispatch REST router",
        surface: "dispatch",
        source: include_str!("../../crates/dispatch/rest/src/lib.rs"),
        ignored_route_refs: &[],
    },
    RouteSource {
        name: "benefit REST router",
        surface: "benefit",
        source: include_str!("../../crates/benefit/rest/src/lib.rs"),
        ignored_route_refs: &[],
    },
    RouteSource {
        name: "financial REST router",
        surface: "financial",
        source: include_str!("../../crates/financial/rest/src/lib.rs"),
        ignored_route_refs: &[],
    },
    RouteSource {
        name: "evaluation REST router",
        surface: "evaluation",
        source: include_str!("../../crates/evaluation/rest/src/lib.rs"),
        ignored_route_refs: &[],
    },
    RouteSource {
        name: "inspection REST router",
        surface: "inspection",
        source: include_str!("../../crates/inspection/rest/src/lib.rs"),
        ignored_route_refs: &[],
    },
    RouteSource {
        name: "support REST router",
        surface: "support",
        source: include_str!("../../crates/support/rest/src/lib.rs"),
        ignored_route_refs: &[],
    },
    RouteSource {
        name: "identity REST router",
        surface: "identity",
        source: include_str!("../../crates/identity/rest/src/lib.rs"),
        ignored_route_refs: &[],
    },
    RouteSource {
        name: "compliance REST router",
        surface: "compliance",
        source: include_str!("../../crates/compliance/rest/src/lib.rs"),
        ignored_route_refs: &[],
    },
    RouteSource {
        name: "integrity REST router",
        surface: "integrity",
        source: include_str!("../../crates/compliance/integrity/src/rest.rs"),
        ignored_route_refs: &[],
    },
    RouteSource {
        name: "registry REST router",
        surface: "registry",
        source: include_str!("../../crates/registry/rest/src/lib.rs"),
        ignored_route_refs: &[],
    },
    RouteSource {
        name: "HR app router",
        surface: "hr",
        source: include_str!("../src/hr.rs"),
        ignored_route_refs: &[],
    },
    RouteSource {
        name: "Workflow Studio app router",
        surface: "workflow-studio",
        source: include_str!("../src/workflow_studio.rs"),
        ignored_route_refs: &[],
    },
    RouteSource {
        name: "collaboration app router",
        surface: "collaboration",
        source: include_str!("../src/collaboration.rs"),
        ignored_route_refs: &[],
    },
    RouteSource {
        name: "sales REST router",
        surface: "sales",
        source: include_str!("../../crates/sales/rest/src/lib.rs"),
        ignored_route_refs: &[],
    },
    RouteSource {
        name: "reporting REST router",
        surface: "reporting",
        source: include_str!("../../crates/reporting/rest/src/lib.rs"),
        ignored_route_refs: &[],
    },
    RouteSource {
        name: "workorder REST routers",
        surface: "workorder",
        source: include_str!("../../crates/workorder/rest/src/lib.rs"),
        ignored_route_refs: &[],
    },
    RouteSource {
        name: "messenger REST router",
        surface: "messenger",
        source: include_str!("../../crates/messenger/rest/src/lib.rs"),
        ignored_route_refs: &[],
    },
    RouteSource {
        name: "comms REST router",
        surface: "comms",
        source: include_str!("../../crates/comms/rest/src/lib.rs"),
        // The MOX inbound webhook authenticates with a provider HMAC secret (not a
        // customer session) and is deliberately absent from the customer OpenAPI +
        // SDK clients; keep it out of the drift inventory intentionally.
        ignored_route_refs: &["MAIL_MOX_WEBHOOK_PATH"],
    },
    RouteSource {
        name: "platform REST router",
        surface: "platform",
        source: include_str!("../../crates/platform/platform-rest/src/lib.rs"),
        ignored_route_refs: &[],
    },
    RouteSource {
        name: "platform view-as router",
        surface: "platform",
        source: include_str!("../../crates/platform/platform-rest/src/view_as.rs"),
        ignored_route_refs: &[],
    },
    RouteSource {
        name: "auth REST router",
        surface: "auth",
        source: include_str!("../../crates/platform/auth-rest/src/lib.rs"),
        // `dev-auth` is feature-gated and intentionally absent from the production
        // OpenAPI contract/inventory.
        ignored_route_refs: &["DEV_AUTH_SESSION_PATH"],
    },
    RouteSource {
        name: "realtime router",
        surface: "realtime",
        source: include_str!("../../crates/platform/realtime/src/lib.rs"),
        ignored_route_refs: &[],
    },
    RouteSource {
        name: "ontology REST router",
        surface: "ontology",
        source: include_str!("../../crates/ontology/rest/src/lib.rs"),
        ignored_route_refs: &[],
    },
    RouteSource {
        name: "governance REST router",
        surface: "governance",
        source: include_str!("../../crates/governance/rest/src/lib.rs"),
        ignored_route_refs: &[],
    },
    RouteSource {
        name: "policy (cedar authoring) REST router",
        surface: "policy",
        source: include_str!("../../crates/platform/authz-rest/src/lib.rs"),
        ignored_route_refs: &[],
    },
    RouteSource {
        name: "evidence (docs) REST router",
        surface: "evidence",
        source: include_str!("../../crates/docs/rest/src/lib.rs"),
        ignored_route_refs: &[],
    },
    RouteSource {
        name: "notices REST router",
        surface: "notices",
        source: include_str!("../../crates/notices/rest/src/lib.rs"),
        ignored_route_refs: &[],
    },
    RouteSource {
        name: "finance-gl REST router",
        surface: "finance-gl",
        source: include_str!("../../crates/finance-gl/rest/src/lib.rs"),
        ignored_route_refs: &[],
    },
    RouteSource {
        name: "payroll REST router",
        surface: "payroll",
        source: include_str!("../../crates/payroll/rest/src/lib.rs"),
        ignored_route_refs: &[],
    },
    RouteSource {
        name: "analytics-quant REST router",
        surface: "analytics",
        source: include_str!("../../crates/analytics-quant/rest/src/lib.rs"),
        ignored_route_refs: &[],
    },
    RouteSource {
        name: "equipment 3R REST router",
        surface: "equipment-3r",
        source: include_str!("../../crates/equipment/rest/src/lib.rs"),
        ignored_route_refs: &[],
    },
    RouteSource {
        name: "notifications REST router",
        surface: "notifications",
        source: include_str!("../../crates/notifications/rest/src/lib.rs"),
        ignored_route_refs: &[],
    },
    RouteSource {
        name: "orgchange REST router",
        surface: "orgchange",
        source: include_str!("../../crates/orgchange/rest/src/lib.rs"),
        ignored_route_refs: &[],
    },
    RouteSource {
        name: "recruiting REST router",
        surface: "recruiting",
        source: include_str!("../../crates/recruiting/rest/src/lib.rs"),
        ignored_route_refs: &[],
    },
    RouteSource {
        name: "recruiting hire app router",
        surface: "recruiting-hire",
        source: include_str!("../src/recruiting_hire.rs"),
        ignored_route_refs: &[],
    },
];

const APP_CARGO_TOML: &str = include_str!("../Cargo.toml");
const APP_LIB_RS: &str = include_str!("../src/lib.rs");

const APP_PRODUCTION_SOURCES: &[(&str, &str)] = &[
    (
        "app src/browser_payslips.rs",
        include_str!("../src/browser_payslips.rs"),
    ),
    (
        "app src/action_inbox.rs",
        include_str!("../src/action_inbox.rs"),
    ),
    (
        "app src/audit_chain_signer.rs",
        include_str!("../src/audit_chain_signer.rs"),
    ),
    (
        "app src/bin/console_cedar_parity_report.rs",
        include_str!("../src/bin/console_cedar_parity_report.rs"),
    ),
    (
        "app src/cedar_parity.rs",
        include_str!("../src/cedar_parity.rs"),
    ),
    (
        "app src/collaboration.rs",
        include_str!("../src/collaboration.rs"),
    ),
    (
        "app src/console_telemetry.rs",
        include_str!("../src/console_telemetry.rs"),
    ),
    (
        "app src/facilities_schedule.rs",
        include_str!("../src/facilities_schedule.rs"),
    ),
    ("app src/hr.rs", include_str!("../src/hr.rs")),
    ("app src/lib.rs", APP_LIB_RS),
    ("app src/lifecycle.rs", include_str!("../src/lifecycle.rs")),
    ("app src/mail_sync.rs", include_str!("../src/mail_sync.rs")),
    ("app src/main.rs", include_str!("../src/main.rs")),
    ("app src/objects.rs", include_str!("../src/objects.rs")),
    ("app src/office.rs", include_str!("../src/office.rs")),
    (
        "app src/recruiting_hire.rs",
        include_str!("../src/recruiting_hire.rs"),
    ),
    ("app src/workbench.rs", include_str!("../src/workbench.rs")),
    (
        "app src/workbench_native.rs",
        include_str!("../src/workbench_native.rs"),
    ),
    (
        "app src/workflow_drain.rs",
        include_str!("../src/workflow_drain.rs"),
    ),
    (
        "app src/workflow_object_context.rs",
        include_str!("../src/workflow_object_context.rs"),
    ),
    (
        "app src/workflow_schedules.rs",
        include_str!("../src/workflow_schedules.rs"),
    ),
    (
        "app src/workflow_studio.rs",
        include_str!("../src/workflow_studio.rs"),
    ),
];

const EXTRA_REST_SOURCES: &[(&str, &str)] = &[
    (
        "logistics REST",
        include_str!("../../crates/logistics/rest/src/lib.rs"),
    ),
    (
        "facilities REST",
        include_str!("../../crates/facilities/rest/src/lib.rs"),
    ),
    (
        "production REST",
        include_str!("../../crates/production/rest/src/lib.rs"),
    ),
    (
        "inbox REST",
        include_str!("../../crates/inbox/rest/src/lib.rs"),
    ),
    (
        "leave REST",
        include_str!("../../crates/leave/rest/src/lib.rs"),
    ),
    (
        "consulting REST",
        include_str!("../../crates/consulting/rest/src/lib.rs"),
    ),
    (
        "todos REST",
        include_str!("../../crates/todos/rest/src/lib.rs"),
    ),
];

const PAYROLL_REST_INNER_SOURCES: &[(&str, &str)] = &[
    (
        "payroll payslip_draft.rs",
        include_str!("../../crates/payroll/rest/src/payslip_draft.rs"),
    ),
    (
        "payroll lifecycle.rs",
        include_str!("../../crates/payroll/rest/src/lifecycle.rs"),
    ),
];

fn todos_rest_source() -> &'static str {
    EXTRA_REST_SOURCES
        .iter()
        .find(|(name, _)| *name == "todos REST")
        .map(|(_, source)| *source)
        .expect("todos REST census source")
}

#[test]
fn configured_route_inventory_includes_each_configured_surface() {
    let surface_names: BTreeSet<_> = CONFIGURED_ROUTE_SURFACES
        .iter()
        .map(|surface| surface.name)
        .collect();

    for required in REQUIRED_CONFIGURED_SURFACES {
        assert!(
            surface_names.contains(required),
            "configured route inventory is missing the {required} surface"
        );
    }

    let audit_paths = CONFIGURED_ROUTE_SURFACES
        .iter()
        .find(|surface| surface.name == "audit")
        .map(|surface| surface.paths)
        .unwrap_or_default();
    assert!(
        audit_paths.contains(&AUDIT_ROUTE_PATH),
        "configured route inventory is missing {AUDIT_ROUTE_PATH}"
    );

    for surface in CONFIGURED_ROUTE_SURFACES {
        assert!(
            !surface.paths.is_empty(),
            "configured route inventory surface {} has no paths",
            surface.name
        );
    }
}

#[test]
fn audit_attestation_route_is_in_configured_inventory() {
    let audit = CONFIGURED_ROUTE_SURFACES
        .iter()
        .find(|surface| surface.name == "audit")
        .expect("audit surface is configured");
    assert!(
        audit.paths.contains(&"/api/v1/audit/attestation"),
        "the registered audit attestation route is missing from the configured inventory"
    );
}

#[test]
fn configured_route_inventory_covers_router_route_calls() {
    let mut missing_surfaces = Vec::new();
    let mut uncovered_routes = Vec::new();
    let mut unresolved_routes = Vec::new();

    for source in CONFIGURED_ROUTE_SOURCES {
        let Some(inventory_paths) = configured_source_inventory_paths(source.surface) else {
            missing_surfaces.push(source.surface);
            continue;
        };

        for route in route_calls(source.source, source.ignored_route_refs) {
            let Some(path) = route.path else {
                unresolved_routes.push(format!(
                    "{} has unresolved route argument {}",
                    source.name, route.argument
                ));
                continue;
            };
            let normalized = normalize_path_parameters(&path);
            if !inventory_paths.contains(&normalized) {
                uncovered_routes.push(format!(
                    "{} route {path} ({}) is missing from configured surface {}",
                    source.name, route.argument, source.surface
                ));
            }
        }
    }

    assert!(
        missing_surfaces.is_empty(),
        "configured route source references unknown surfaces: {}",
        missing_surfaces.join(", ")
    );
    assert!(
        unresolved_routes.is_empty(),
        "configured router source parsing found unresolved route refs:\n{}",
        unresolved_routes.join("\n")
    );
    assert!(
        uncovered_routes.is_empty(),
        "configured router route calls are missing from the OpenAPI drift inventory:\n{}",
        uncovered_routes.join("\n")
    );
}

#[test]
fn openapi_yaml_covers_configured_route_inventory() {
    let openapi_paths = openapi_path_keys(OPENAPI_YAML);

    for surface in CONFIGURED_ROUTE_SURFACES {
        for path in surface.paths {
            let normalized = normalize_path_parameters(path);
            assert!(
                openapi_paths.contains(&normalized),
                "OpenAPI YAML is missing configured {surface} route {path}",
                surface = surface.name
            );
        }
    }

    // All six legacy org-setup mutation operations require an idempotency key
    // (422 when absent/invalid) and can conflict on key reuse or live target
    // state (409). Assert per operation: a path-level scan would let DELETE's
    // response accidentally mask a missing PATCH response, or vice versa.
    for (method, path) in [
        ("post", "/api/v1/regions"),
        ("patch", "/api/v1/regions/{id}"),
        ("delete", "/api/v1/regions/{id}"),
        ("post", "/api/v1/branches"),
        ("patch", "/api/v1/branches/{id}"),
        ("delete", "/api/v1/branches/{id}"),
    ] {
        let operation = openapi_operation_body(OPENAPI_YAML, path, method);
        for status in ["409", "422"] {
            assert!(
                operation.contains(&format!("        '{status}':")),
                "OpenAPI {method} {path} must document its reachable {status} response"
            );
        }
    }
}

#[test]
fn openapi_documents_native_discoverable_passkey_modes() {
    let registration =
        openapi_operation_body(OPENAPI_YAML, "/api/v1/auth/passkey/register/start", "post");
    assert!(
        registration.contains("requestBody:\n        required: true"),
        "native Json requires a body"
    );
    assert!(registration.contains("'422':") && registration.contains("'415':"));
    let schema = openapi_schema_body(OPENAPI_YAML, "PasskeyRegisterStartRequest");
    assert!(
        !schema
            .lines()
            .any(|line| line.starts_with("      required:")),
        "registration request properties remain optional"
    );
    let property = schema
        .split_once("        require_discoverable:\n")
        .expect("new typed optional flag")
        .1;
    let property = property
        .lines()
        .take_while(|line| line.starts_with("          ") || line.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(property.contains("type: boolean") && property.contains("default: false"));
    for path in [
        "/api/v1/auth/passkey/login/start",
        "/api/v1/auth/passkey/login/explicit/start",
    ] {
        let operation = openapi_operation_body(OPENAPI_YAML, path, "post");
        assert!(
            !operation.contains("requestBody:"),
            "login start stays bodyless"
        );
        assert!(operation.contains("PasskeyLoginStartResponse"));
        assert!(operation.contains("'429':"));
    }
}

#[test]
fn storefront_inquiry_contract_documents_the_reachable_conflict() {
    let operation = openapi_operation_body(OPENAPI_YAML, "/api/v1/storefront/inquiries", "post");
    assert!(
        operation.contains("        '409':\n          $ref: '#/components/responses/Conflict'"),
        "published POST /api/v1/storefront/inquiries must document the listing conflict"
    );
}

fn openapi_operation_body<'a>(yaml: &'a str, path: &str, method: &str) -> &'a str {
    let path_needle = format!("  {path}:\n");
    let path_start = yaml
        .find(&path_needle)
        .unwrap_or_else(|| panic!("OpenAPI YAML must define {path}"));
    let path_end = yaml[path_start + path_needle.len()..]
        .find("\n  /")
        .map_or(yaml.len(), |offset| {
            path_start + path_needle.len() + offset + 1
        });
    let path_body = &yaml[path_start..path_end];
    let method_needle = format!("    {method}:\n");
    let method_start = path_body
        .find(&method_needle)
        .unwrap_or_else(|| panic!("OpenAPI YAML must define {method} {path}"));
    let after_method = method_start + method_needle.len();
    let mut method_end = path_body.len();
    let mut offset = after_method;
    for line in path_body[after_method..].split_inclusive('\n') {
        let trimmed = line.trim_end();
        if line.starts_with("    ")
            && !line.starts_with("     ")
            && trimmed.ends_with(':')
            && is_openapi_method(trimmed.trim_end_matches(':').trim())
        {
            method_end = offset;
            break;
        }
        offset += line.len();
    }
    &path_body[method_start..method_end]
}

fn openapi_schema_body<'a>(yaml: &'a str, schema_name: &str) -> &'a str {
    let needle = format!("    {schema_name}:\n");
    let start = yaml
        .find(&needle)
        .unwrap_or_else(|| panic!("OpenAPI YAML must define {schema_name}"));
    let after = start + needle.len();
    let mut end = yaml.len();
    let bytes = yaml.as_bytes();
    let mut i = after;
    while i < yaml.len() {
        if bytes[i] != b'\n' {
            i += 1;
            continue;
        }
        let line_start = i + 1;
        let line = &yaml[line_start..];
        // Exactly four leading spaces ⇒ next components.schemas key (compose indent).
        if line.starts_with("    ") && !line.starts_with("     ") {
            end = line_start;
            break;
        }
        if !line.is_empty() && !line.starts_with(' ') {
            end = line_start;
            break;
        }
        i = line_start;
    }
    &yaml[start..end]
}

#[test]
fn openapi_documents_closed_inventory_movement_source_variants() {
    // Compose emits schemas in sorted key order; assert by named anchors, not sibling windows.
    for variant in [
        "InventoryMovementSourceWorkOrder",
        "InventoryMovementSourceP1Dispatch",
        "InventoryMovementSourceCycleCount",
        "InventoryMovementSourceExternalRef",
    ] {
        assert!(
            OPENAPI_YAML.contains(&format!("    {variant}:\n")),
            "OpenAPI movement source is missing {variant}"
        );
        assert!(
            openapi_schema_body(OPENAPI_YAML, variant).contains("additionalProperties: false"),
            "every inventory movement source variant must be closed to unknown fields ({variant})"
        );
    }
    assert!(
        OPENAPI_YAML.contains("source: { $ref: '#/components/schemas/InventoryMovementSource' }"),
        "InventoryMovement.source must not degrade to an untyped object"
    );
    assert!(
        OPENAPI_YAML.contains("InventoryMovementSource:\n      oneOf:")
            && OPENAPI_YAML.contains("discriminator:\n        propertyName: kind"),
        "Inventory movement sources must remain a kind-discriminated union"
    );
    for wire_kind in ["work_order", "p1_dispatch", "cycle_count", "external_ref"] {
        assert!(
            OPENAPI_YAML.contains(wire_kind),
            "OpenAPI movement source must document the {wire_kind} runtime discriminator"
        );
    }
}

#[test]
fn openapi_documents_closed_month_as_year_month_not_calendar_date() {
    let schema = openapi_schema_body(OPENAPI_YAML, "AttendanceMonthClose");
    assert!(
        schema.contains("month: { type: string, pattern: '^\\\\d{4}-\\\\d{2}$' }"),
        "closed-month response must match the server's YYYY-MM wire value, not an OpenAPI calendar date"
    );
}

#[test]
fn openapi_documents_hr_attendance_branch_scope_query() {
    for (path, next_path) in [
        (
            "/api/v1/hr/attendance-summary:",
            "/api/v1/hr/readiness-summary:",
        ),
        (
            "/api/v1/hr/attendance-records:",
            "/api/v1/employees/import:",
        ),
    ] {
        let start = OPENAPI_YAML
            .find(path)
            .unwrap_or_else(|| panic!("OpenAPI YAML is missing {path}"));
        let end = OPENAPI_YAML[start..]
            .find(next_path)
            .map(|offset| start + offset)
            .unwrap_or(OPENAPI_YAML.len());
        let operation = &OPENAPI_YAML[start..end];
        assert!(
            operation.contains("- name: branch_id\n        in: query"),
            "{path} must expose the optional snake_case branch_id query accepted by its Axum handler"
        );
    }
}

#[test]
fn openapi_documents_evidence_register_snapshot_and_evidentiary_contract() {
    let endpoint_start = OPENAPI_YAML
        .find("  /api/v1/evidence/objects:\n")
        .expect("OpenAPI YAML must define the EV object list endpoint");
    let endpoint_end = OPENAPI_YAML[endpoint_start..]
        .find("  /api/v1/evidence/objects/{id}:")
        .map(|offset| endpoint_start + offset)
        .expect("EV object detail endpoint must follow the list endpoint");
    let endpoint = &OPENAPI_YAML[endpoint_start..endpoint_end];

    for parameter in ["offset", "as_of", "cursor"] {
        assert!(
            endpoint.contains(&format!("name: {parameter}, in: query")),
            "EV list endpoint must document the runtime-supported {parameter} query parameter"
        );
    }
    assert!(
        endpoint.contains(
            "name: as_of, in: query, required: false, schema: { type: integer, format: int64 }"
        ),
        "EV as_of must be an optional immutable registration sequence"
    );
    assert!(
        endpoint.contains("name: cursor, in: query, required: false, schema: { type: string, pattern: '^[A-Za-z0-9_-]+$' }"),
        "EV cursor must expose the runtime's opaque unpadded-base64url wire contract"
    );
    assert!(
        endpoint.contains("When cursor is supplied, offset must be omitted or zero."),
        "EV cursor pagination must document its offset compatibility boundary"
    );
    assert!(
        endpoint.contains("When cursor is supplied, as_of must match that cursor's snapshot."),
        "EV cursor pagination must document its immutable snapshot boundary"
    );
    assert!(
        endpoint.contains("'422': { $ref: '#/components/responses/ValidationError' }"),
        "EV list endpoint must document validation failures for inconsistent pagination inputs"
    );

    // Compose emits schemas in sorted key order; assert each schema by name, not sibling windows.
    let page = openapi_schema_body(OPENAPI_YAML, "EvidenceObjectPage");
    assert!(
        page.contains("required: [items, limit, offset, total, as_of, next_cursor]"),
        "EV list response must always return the registered snapshot and nullable continuation token"
    );
    // No closing brace: the flow map may also carry a description.
    assert!(
        page.contains("as_of: { type: integer, format: int64"),
        "EV page as_of must be the int64 evidence-register sequence"
    );
    assert!(
        page.contains("next_cursor:\n          type:\n          - string\n          - 'null'"),
        "EV next_cursor must use the OpenAPI 3.1 nullable-string form consumed by every generated client"
    );

    let copy = openapi_schema_body(OPENAPI_YAML, "EvidenceCopyView");
    assert!(
        copy.contains(
            "evidentiary_status: { $ref: '#/components/schemas/EvidenceCopyEvidentiaryStatus' }"
        ),
        "EV copy view must expose the server-derived evidentiary classification"
    );
    assert!(
        copy.contains("required: [id, evidence_object_id, copy_kind, evidentiary_status, storage, digest_sha256, content_type, size_bytes, worm_status, created_by, created_at]"),
        "EV copy view must require the server-derived evidentiary classification"
    );

    let status = openapi_schema_body(OPENAPI_YAML, "EvidenceCopyEvidentiaryStatus");
    assert!(
        status
            .contains("enum: [VERIFIED_ORIGINAL, ORIGINAL_UNVERIFIED, NON_EVIDENTIARY_DERIVATIVE]")
    );
}

#[test]
fn openapi_yaml_covers_platform_route_operations() {
    let missing = missing_platform_route_operations(OPENAPI_YAML);
    assert!(
        missing.is_empty(),
        "OpenAPI YAML is missing platform route operation coverage:\n{}",
        missing.join("\n")
    );

    let expected = platform_route_operation_keys();
    let unexpected: Vec<_> = openapi_operation_keys(OPENAPI_YAML)
        .into_iter()
        .filter(|(path, _method)| path.starts_with("/api/platform/"))
        .filter(|operation| !expected.contains(operation))
        .map(|(path, method)| format!("{} {path}", method.to_ascii_uppercase()))
        .collect();
    assert!(
        unexpected.is_empty(),
        "OpenAPI YAML documents platform operations that are not in console_platform_rest::PLATFORM_ROUTE_OPERATIONS:\n{}",
        unexpected.join("\n")
    );
}

#[test]
fn platform_route_operation_gate_rejects_missing_contract_entry() {
    let broken_yaml = OPENAPI_YAML.replacen(
        "    delete:\n      tags:\n        - platform\n      operationId: removePlatformOrgFromGroup",
        "    x-delete-missing-for-test:\n      tags:\n        - platform\n      operationId: removePlatformOrgFromGroup",
        1,
    );
    assert_ne!(
        broken_yaml, OPENAPI_YAML,
        "test fixture anchor no longer matches the OpenAPI YAML; update the replacen target so the DELETE operation is actually removed before asserting the gate detects it"
    );
    let missing = missing_platform_route_operations(&broken_yaml);

    assert!(
        missing
            .iter()
            .any(|entry| entry == "DELETE /api/platform/groups/{id}/organizations/{org_id}"),
        "deliberately removing DELETE /api/platform/groups/{{id}}/organizations/{{org_id}} from OpenAPI should be reported; missing={missing:?}"
    );
}

fn openapi_path_keys(yaml: &str) -> BTreeSet<String> {
    yaml.lines()
        .filter_map(|line| {
            let trimmed = line.trim_end();
            if !line.starts_with("  /") || !trimmed.ends_with(':') {
                return None;
            }
            Some(normalize_path_parameters(
                trimmed.trim_end_matches(':').trim(),
            ))
        })
        .collect()
}

fn missing_platform_route_operations(yaml: &str) -> Vec<String> {
    let openapi_operations = openapi_operation_keys(yaml);
    PLATFORM_ROUTE_OPERATIONS
        .iter()
        .filter_map(|operation| {
            let key = (
                normalize_path_parameters(operation.path),
                operation.method.to_ascii_lowercase(),
            );
            if openapi_operations.contains(&key) {
                None
            } else {
                Some(format!("{} {}", operation.method, operation.path))
            }
        })
        .collect()
}

fn platform_route_operation_keys() -> BTreeSet<(String, String)> {
    PLATFORM_ROUTE_OPERATIONS
        .iter()
        .map(|operation| {
            (
                normalize_path_parameters(operation.path),
                operation.method.to_ascii_lowercase(),
            )
        })
        .collect()
}

fn openapi_operation_keys(yaml: &str) -> BTreeSet<(String, String)> {
    let mut current_path: Option<String> = None;
    let mut operations = BTreeSet::new();

    for line in yaml.lines() {
        let trimmed = line.trim_end();

        if line.starts_with("  /") && trimmed.ends_with(':') {
            current_path = Some(normalize_path_parameters(
                trimmed.trim_end_matches(':').trim(),
            ));
            continue;
        }

        // Blank lines inside folded/literal descriptions are not path terminators.
        if !line.starts_with(' ') {
            if !trimmed.is_empty() {
                current_path = None;
            }
            continue;
        }

        let Some(path) = current_path.as_ref() else {
            continue;
        };
        if line.starts_with("    ") && !line.starts_with("      ") && trimmed.ends_with(':') {
            let method = trimmed.trim_end_matches(':').trim();
            if is_openapi_method(method) {
                operations.insert((path.clone(), method.to_ascii_lowercase()));
            }
        }
    }

    operations
}

fn is_openapi_method(value: &str) -> bool {
    matches!(
        value,
        "get" | "put" | "post" | "delete" | "options" | "head" | "patch" | "trace"
    )
}

struct RouteCall {
    argument: String,
    path: Option<String>,
}

fn configured_surface_paths(surface_name: &str) -> Option<&'static [&'static str]> {
    CONFIGURED_ROUTE_SURFACES
        .iter()
        .find(|surface| surface.name == surface_name)
        .map(|surface| surface.paths)
}

fn configured_source_inventory_paths(surface_name: &str) -> Option<BTreeSet<String>> {
    let mut inventory = BTreeSet::new();
    if surface_name == "workorder" {
        for name in ["workorder", "workorder-mobile"] {
            let paths = configured_surface_paths(name)?;
            inventory.extend(paths.iter().map(|path| normalize_path_parameters(path)));
        }
        return Some(inventory);
    }

    let paths = configured_surface_paths(surface_name)?;
    inventory.extend(paths.iter().map(|path| normalize_path_parameters(path)));
    Some(inventory)
}

fn route_calls(source: &'static str, ignored_route_refs: &[&str]) -> Vec<RouteCall> {
    let constants = route_path_constants(source);
    let mut calls = Vec::new();
    let mut offset = 0;
    while let Some(relative_start) = source[offset..].find(".route(") {
        let args_start = offset + relative_start + ".route(".len();
        let Some(argument) = first_route_argument(&source[args_start..]) else {
            break;
        };
        let trimmed = argument.trim();
        let parsed = route_argument_path(trimmed, &constants, ignored_route_refs);
        if !parsed.ignored {
            calls.push(RouteCall {
                argument: trimmed.to_owned(),
                path: parsed.path,
            });
        }
        offset = args_start + argument.len();
    }
    calls
}

struct ParsedRouteArgument {
    path: Option<String>,
    ignored: bool,
}

fn route_argument_path(
    argument: &str,
    constants: &BTreeMap<String, String>,
    ignored_route_refs: &[&str],
) -> ParsedRouteArgument {
    if let Some(path) = quoted_argument(argument) {
        return ParsedRouteArgument {
            path: Some(path.to_owned()),
            ignored: false,
        };
    }

    let Some(identifier) = leading_identifier(argument) else {
        return ParsedRouteArgument {
            path: None,
            ignored: false,
        };
    };
    if ignored_route_refs.contains(&identifier) {
        return ParsedRouteArgument {
            path: None,
            ignored: true,
        };
    }

    ParsedRouteArgument {
        path: constants.get(identifier).cloned(),
        ignored: false,
    }
}

fn route_path_constants(source: &str) -> BTreeMap<String, String> {
    let mut constants = BTreeMap::new();
    let mut offset = 0;
    while let Some(relative_start) = source[offset..].find("const ") {
        let name_start = offset + relative_start + "const ".len();
        let Some(statement_end) = source[name_start..].find(';') else {
            break;
        };
        let statement = &source[name_start..name_start + statement_end];
        offset = name_start + statement_end + 1;

        if !statement.contains("&str") {
            continue;
        }
        let Some(name_end) = statement.find(':') else {
            continue;
        };
        let Some(path) = quoted_argument(statement) else {
            continue;
        };
        constants.insert(statement[..name_end].trim().to_owned(), path.to_owned());
    }
    constants
}

fn first_route_argument(source: &str) -> Option<&str> {
    let mut in_string = false;
    let mut escaped = false;
    let mut nested = 0usize;

    for (idx, ch) in source.char_indices() {
        if in_string {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }

        match ch {
            '"' => in_string = true,
            '(' | '[' | '{' => nested += 1,
            ')' | ']' | '}' => nested = nested.saturating_sub(1),
            ',' if nested == 0 => return Some(&source[..idx]),
            _ => {}
        }
    }
    None
}

fn quoted_argument(value: &str) -> Option<&str> {
    let first_quote = value.find('"')?;
    let after_first = &value[first_quote + '"'.len_utf8()..];
    let second_quote = after_first.find('"')?;
    Some(&after_first[..second_quote])
}

fn leading_identifier(value: &str) -> Option<&str> {
    let trimmed = value.trim_start();
    let end = trimmed
        .char_indices()
        .find_map(|(idx, ch)| (!is_identifier_char(ch)).then_some(idx))
        .unwrap_or(trimmed.len());
    (end > 0).then_some(&trimmed[..end])
}

fn is_identifier_char(ch: char) -> bool {
    ch == '_' || ch.is_ascii_alphanumeric()
}

fn normalize_path_parameters(path: &str) -> String {
    let mut normalized = String::with_capacity(path.len());
    let mut in_parameter = false;
    for ch in path.chars() {
        match ch {
            '{' => {
                in_parameter = true;
                normalized.push_str("{}");
            }
            '}' => in_parameter = false,
            _ if !in_parameter => normalized.push(ch),
            _ => {}
        }
    }
    normalized
}

fn bounded_section<'a>(source: &'a str, start: &str, end: &str) -> &'a str {
    let start = source
        .find(start)
        .unwrap_or_else(|| panic!("missing start {start}"));
    let after_start = start + source[start..].find('\n').unwrap_or(0) + 1;
    let end = source[after_start..]
        .find(end)
        .map(|offset| after_start + offset)
        .unwrap_or_else(|| panic!("missing end {end}"));
    &source[start..end]
}

#[test]
fn bounded_generated_sections_reject_later_operation_or_enum_text() {
    let operation = bounded_section("target\nnext target", "target", "next");
    assert!(
        !operation.contains("next target"),
        "later operation text must not satisfy target assertions"
    );
    let status = bounded_section(
        "DispatchQueueStatus\nRECEIVED\nOtherStatus\nDELAYED",
        "DispatchQueueStatus",
        "OtherStatus",
    );
    assert!(
        status.contains("RECEIVED") && !status.contains("DELAYED"),
        "later enum text must not satisfy target assertions"
    );
}

#[test]
#[should_panic(expected = "missing end absent")]
fn bounded_generated_sections_reject_missing_end_boundary() {
    let _ = bounded_section("target only", "target", "absent");
}

struct CensusSource {
    name: &'static str,
    source: &'static str,
    ignored_route_refs: &'static [&'static str],
}

fn landable_http_census_sources() -> Vec<CensusSource> {
    let mut sources = Vec::new();
    for source in CONFIGURED_ROUTE_SOURCES {
        sources.push(CensusSource {
            name: source.name,
            source: source.source,
            ignored_route_refs: source.ignored_route_refs,
        });
    }
    for &(name, source) in APP_PRODUCTION_SOURCES
        .iter()
        .chain(EXTRA_REST_SOURCES)
        .chain(PAYROLL_REST_INNER_SOURCES)
    {
        sources.push(CensusSource {
            name,
            source,
            ignored_route_refs: &[],
        });
    }
    sources
}

fn strip_cfg_test(source: &str) -> String {
    let mut out = String::with_capacity(source.len());
    let mut rest = source;
    while let Some(idx) = rest.find("#[cfg(test)]") {
        out.push_str(&rest[..idx]);
        rest = skip_ws_and_attributes(&rest[idx + "#[cfg(test)]".len()..]);
        rest = skip_item(rest);
    }
    out.push_str(rest);
    out
}

fn skip_ws_and_attributes(source: &str) -> &str {
    let mut s = source.trim_start();
    while s.starts_with("#[") {
        s = skip_delimited(&s[1..], '[', ']').unwrap_or("");
        s = s.trim_start();
    }
    s
}

fn skip_item(source: &str) -> &str {
    let mut in_string = false;
    let mut escaped = false;
    let mut depth = 0usize;
    let mut started_block = false;
    for (idx, ch) in source.char_indices() {
        if in_string {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }
        match ch {
            '"' => in_string = true,
            '{' => {
                depth += 1;
                started_block = true;
            }
            '}' => {
                depth = depth.saturating_sub(1);
                if started_block && depth == 0 {
                    return &source[idx + ch.len_utf8()..];
                }
            }
            ';' if depth == 0 => return &source[idx + ch.len_utf8()..],
            _ => {}
        }
    }
    ""
}

fn skip_delimited(source: &str, open: char, close: char) -> Option<&str> {
    if !source.starts_with(open) {
        return None;
    }
    let mut in_string = false;
    let mut escaped = false;
    let mut depth = 0usize;
    for (idx, ch) in source.char_indices() {
        if in_string {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }
        if ch == '"' {
            in_string = true;
            continue;
        }
        if ch == open {
            depth += 1;
        } else if ch == close {
            depth = depth.saturating_sub(1);
            if depth == 0 {
                return Some(&source[idx + ch.len_utf8()..]);
            }
        }
    }
    None
}

fn brace_matched_from(source: &str, open_idx: usize) -> Option<&str> {
    let slice = &source[open_idx..];
    if !slice.starts_with('{') && !slice.starts_with('(') {
        return None;
    }
    let (open, close) = if slice.starts_with('{') {
        ('{', '}')
    } else {
        ('(', ')')
    };
    let mut in_string = false;
    let mut escaped = false;
    let mut depth = 0usize;
    for (idx, ch) in slice.char_indices() {
        if in_string {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }
        if ch == '"' {
            in_string = true;
            continue;
        }
        if ch == open {
            depth += 1;
        } else if ch == close {
            depth = depth.saturating_sub(1);
            if depth == 0 {
                return Some(&slice[..=idx]);
            }
        }
    }
    None
}

fn contains_ai_assist_constructor(source: &str) -> bool {
    let compact: String = source.chars().filter(|c| !c.is_whitespace()).collect();
    compact.contains("Action::new(Feature::AiAssist)")
        || compact.contains("Action::limited(Feature::AiAssist)")
        || compact.contains("Action::request(Feature::AiAssist)")
}

fn forbids_chat_or_intelligence(path: &str) -> bool {
    const PREFIXES: &[&str] = &[
        "/v1/chat",
        "/api/v1/chat",
        "/api/v1/intelligence",
        "/api/intelligence",
        "/intelligence",
    ];
    if PREFIXES
        .iter()
        .any(|prefix| path == *prefix || path.starts_with(&format!("{prefix}/")))
    {
        return true;
    }
    path.split('/')
        .any(|segment| segment == "chat" || segment == "intelligence")
}

fn matching_close_from(source: &str, open: char, close: char) -> Option<usize> {
    let mut in_string = false;
    let mut escaped = false;
    let mut depth = 0usize;
    for (idx, ch) in source.char_indices() {
        if in_string {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }
        match ch {
            '"' => in_string = true,
            ch if ch == open => depth += 1,
            ch if ch == close => {
                if depth == 0 {
                    return Some(idx);
                }
                depth = depth.saturating_sub(1);
            }
            _ => {}
        }
    }
    None
}

fn nest_calls(source: &'static str) -> Vec<(Option<String>, &'static str)> {
    let constants = route_path_constants(source);
    let mut calls = Vec::new();
    let mut offset = 0;
    while let Some(relative_start) = source[offset..].find(".nest(") {
        let args_start = offset + relative_start + ".nest(".len();
        let Some(end) = matching_close_from(&source[args_start..], '(', ')') else {
            break;
        };
        let inner = &source[args_start..args_start + end];
        let Some(argument) = first_route_argument(inner) else {
            break;
        };
        let trimmed = argument.trim();
        let parsed = route_argument_path(trimmed, &constants, &[]);
        calls.push((parsed.path, inner));
        offset = args_start + end + 1;
    }
    calls
}

fn quoted_strings(source: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut rest = source;
    while let Some(value) = quoted_argument(rest) {
        out.push(value);
        let Some(first) = rest.find('"') else {
            break;
        };
        rest = &rest[first + 1 + value.len() + 1..];
    }
    out
}

fn function_body<'a>(source: &'a str, signature: &str) -> Option<&'a str> {
    let start = source.find(signature)?;
    let brace = source[start..].find('{')?;
    brace_matched_from(source, start + brace)
}

fn projected_dispatch_registry_source(lib: &str) -> &str {
    let needle = "fn projected_dispatch_registry";
    let start = lib
        .find(needle)
        .unwrap_or_else(|| panic!("missing {needle}"));
    let brace = lib[start..]
        .find('{')
        .unwrap_or_else(|| panic!("{needle} has no body"));
    brace_matched_from(lib, start + brace).expect("projected_dispatch_registry braces")
}

#[test]
fn ai_assist_action_constructors_are_absent_from_landable_http_surfaces() {
    let mut hits = Vec::new();
    for source in landable_http_census_sources() {
        let stripped = strip_cfg_test(source.source);
        if contains_ai_assist_constructor(&stripped) {
            hits.push(source.name);
        }
    }
    assert!(
        hits.is_empty(),
        "Action::{{new,limited,request}}(Feature::AiAssist) must stay off HTTP surfaces: {}",
        hits.join(", ")
    );
}

#[test]
fn internal_intelligence_bind_is_forbidden() {
    for path in [
        "/internal/intelligence/bind",
        "/v1/chat",
        "/api/v1/chat",
        "/api/v1/intelligence",
        "/api/intelligence",
        "/intelligence",
        "/internal/intelligence",
        "/internal/intelligence/bind/extra",
        "/api/v1/internal/intelligence/bind",
    ] {
        assert!(
            forbids_chat_or_intelligence(path),
            "{path} must stay forbidden"
        );
    }
}

#[test]
fn intelligence_and_chat_http_surfaces_are_absent() {
    let mut forbidden = Vec::new();
    for path in openapi_path_keys(OPENAPI_YAML) {
        if forbids_chat_or_intelligence(&path) {
            forbidden.push(format!("openapi {path}"));
        }
    }
    for source in landable_http_census_sources() {
        for route in route_calls(source.source, source.ignored_route_refs) {
            let Some(path) = route.path else {
                continue;
            };
            if forbids_chat_or_intelligence(&path) {
                forbidden.push(format!("{} route {path}", source.name));
            }
        }
    }

    let mut saw_ui_nest = false;
    for (name, source) in APP_PRODUCTION_SOURCES {
        for (prefix, body) in nest_calls(source) {
            let Some(prefix) = prefix else {
                continue;
            };
            if forbids_chat_or_intelligence(&prefix) {
                forbidden.push(format!("{name} nest {prefix}"));
            }
            if prefix == "/_ui" {
                saw_ui_nest = true;
                assert!(
                    body.contains(".merge(console_payroll_ui::pkg_router())"),
                    "{name} /_ui nest must still merge console_payroll_ui::pkg_router()"
                );
            }
        }
    }
    assert!(saw_ui_nest, "app production sources must still nest /_ui");

    for value in route_path_constants(todos_rest_source()).values() {
        if forbids_chat_or_intelligence(value) {
            forbidden.push(format!("todos-rest const {value}"));
        }
    }
    let routes_body =
        function_body(todos_rest_source(), "fn routes(").expect("todos-rest fn routes");
    for value in quoted_strings(routes_body) {
        if forbids_chat_or_intelligence(value) {
            forbidden.push(format!("todos-rest routes() {value}"));
        }
    }

    assert!(
        forbidden.is_empty(),
        "chat/intelligence HTTP surfaces must stay absent:\n{}",
        forbidden.join("\n")
    );
}

#[test]
fn app_does_not_wire_intelligence_draft_dispatch_or_depend_on_the_seam_crate() {
    let body = projected_dispatch_registry_source(APP_LIB_RS);
    assert!(
        !body.contains("mod projected_dispatch_coverage"),
        "brace-match must stop before projected_dispatch_coverage"
    );
    let register_count = body.matches(".register(").count();
    assert_eq!(
        register_count, 1,
        "projected_dispatch_registry must keep exactly one .register( call, got {register_count}"
    );
    let register_at = body.find(".register(").expect(".register(");
    let after = &body[register_at..];
    let literal = quoted_argument(after).expect("register string literal");
    assert_eq!(literal, "registry.update_equipment");
    assert!(
        !body.contains("intelligence.draft"),
        "projected_dispatch_registry must not register intelligence.draft"
    );
    assert!(DispatchTarget::from_str("intelligence.draft").is_err());
    assert!(
        !APP_CARGO_TOML.contains("console-intelligence-application"),
        "console-app must not depend on the intelligence seam crate"
    );
}

#[test]
fn issued_browser_payslip_contract_matches_read_only_owner_routes() {
    let operations = openapi_operation_keys(OPENAPI_YAML);
    for (path, request, response) in [
        (
            "/api/v1/me/browser-session/payslips",
            "BrowserPayslipListRequest",
            "BrowserPayslipListResponse",
        ),
        (
            "/api/v1/me/browser-session/payslips/{id}",
            "BrowserSessionIdentityRequest",
            "BrowserPayslipDetailResponse",
        ),
    ] {
        assert!(
            configured_surface_paths("browser-payslips")
                .unwrap()
                .contains(&path)
        );
        let methods: BTreeSet<_> = operations
            .iter()
            .filter(|(candidate, _)| candidate == &normalize_path_parameters(path))
            .map(|(_, method)| method.as_str())
            .collect();
        assert_eq!(methods, BTreeSet::from(["post"]));
        let body = openapi_operation_body(OPENAPI_YAML, path, "post");
        assert!(
            body.contains("security:\n      - BrowserIngress: []") && !body.contains("bearerAuth")
        );
        assert!(body.contains(request) && body.contains(response));
        if path.ends_with("/{id}") {
            assert!(body.contains("'400':"), "malformed path must be documented");
        }
        for status in ["200", "401", "404", "413", "422", "503"] {
            assert!(
                body.contains(&format!("'{status}':")),
                "{path} missing {status}"
            );
        }
    }
    for name in [
        "BrowserPayslipListRequest",
        "BrowserPayslipListResponse",
        "BrowserPayslipSummary",
        "BrowserPayslipDocument",
        "BrowserPayslipDetailResponse",
        "BrowserPayslipPayload",
        "BrowserPayslipDeduction",
    ] {
        let schema = openapi_schema_body(OPENAPI_YAML, name);
        assert!(schema.contains("additionalProperties: false"));
        for field in ["recipient", "user_id", "access_token", "refresh_token"] {
            assert!(
                !schema.contains(&format!("{field}:")),
                "{name} must not accept authority selectors"
            );
        }
    }
    for (name, property) in [
        ("BrowserPayslipListRequest", "before"),
        ("BrowserPayslipListResponse", "next_cursor"),
    ] {
        let schema = openapi_schema_body(OPENAPI_YAML, name);
        assert!(schema.contains(&format!(
            "        {property}:\n          type: [string, 'null']\n          format: uuid"
        )));
        assert!(
            !schema.contains("nullable:"),
            "3.1 must use a true null type"
        );
    }
    let money = openapi_schema_body(OPENAPI_YAML, "BrowserPayslipMoney");
    assert!(money.contains("type: string") && money.contains("maxLength: 20"));
    assert!(money.contains("pattern: '^(?:0|-?[1-9][0-9]*)$'"));
    assert!(openapi_schema_body(OPENAPI_YAML, "BrowserPayslipPayload").contains("maxItems: 6"));
    assert!(
        openapi_schema_body(OPENAPI_YAML, "BrowserPayslipListResponse").contains("maxItems: 25")
    );
}

#[test]
fn browser_session_contract_is_handle_only_and_matches_dedicated_owner_routes() {
    let operations = openapi_operation_keys(OPENAPI_YAML);
    for (surface, path) in [
        ("auth", "/api/v1/auth/browser-session/start"),
        ("auth", "/api/v1/auth/browser-session/login"),
        ("auth", "/api/v1/auth/browser-session/logout"),
        ("hr", "/api/v1/hr/browser-session/attendance-records/me"),
    ] {
        assert!(
            configured_surface_paths(surface).unwrap().contains(&path),
            "dedicated route must be registered by its actual owner: {path}"
        );
        let methods: BTreeSet<_> = operations
            .iter()
            .filter(|(candidate, _)| candidate == path)
            .map(|(_, method)| method.as_str())
            .collect();
        assert_eq!(
            methods,
            BTreeSet::from(["post"]),
            "opaque proof stays out of GET URLs"
        );
        let operation = openapi_operation_body(OPENAPI_YAML, path, "post");
        assert!(
            operation.contains("security:\n      - BrowserIngress: []")
                && !operation.contains("bearerAuth"),
            "browser ingress is separately authenticated; ambient bearer is not authority"
        );
        assert!(
            operation.contains("'401':") && operation.contains("'503':"),
            "denied and unavailable outcomes must remain distinct"
        );
        if path.ends_with("/start") {
            assert!(
                !operation.contains("requestBody:"),
                "new explicit start is bodyless"
            );
            assert!(
                operation.contains("PasskeyLoginStartResponse") && operation.contains("'429':")
            );
        } else if path.ends_with("/logout") {
            assert!(
                operation.contains("BrowserSessionIdentityRequest") && operation.contains("'204':")
            );
            let terminal = operation
                .split("'204':")
                .nth(1)
                .unwrap()
                .split("        '")
                .next()
                .unwrap();
            assert!(
                !terminal.contains("content:"),
                "confirmed closure has an empty body"
            );
        } else if path.ends_with("/login") {
            assert!(
                operation.contains("PasskeyLoginFinishRequest")
                    && operation.contains("BrowserSessionLoginResponse")
                    && operation.contains("'413':")
            );
        } else {
            assert!(
                operation.contains("BrowserSessionAttendanceRequest")
                    && operation.contains("BrowserSessionAttendanceResponse")
            );
        }
    }
    assert!(
        !operations
            .iter()
            .any(|(path, _)| path == "/api/v1/auth/browser-session/resolve"),
        "no public JWT conversion path exists"
    );
    for (name, fields) in [
        (
            "BrowserSessionLoginResponse",
            vec!["session_token", "context_id", "expires_at"],
        ),
        (
            "BrowserSessionIdentityRequest",
            vec!["session_token", "browser_context"],
        ),
        (
            "BrowserSessionAttendanceRequest",
            vec!["session_token", "browser_context", "limit", "offset"],
        ),
        (
            "BrowserSessionAttendanceResponse",
            vec!["context", "history", "browser_context", "expires_at"],
        ),
        (
            "BrowserSessionContext",
            vec![
                "company_id",
                "company_name",
                "account_display_name",
                "employee_linked",
            ],
        ),
    ] {
        let schema = openapi_schema_body(OPENAPI_YAML, name);
        assert!(
            schema.contains("additionalProperties: false"),
            "{name} is a closed contract"
        );
        let properties = schema
            .split_once("      properties:\n")
            .expect("typed properties")
            .1;
        let actual: BTreeSet<_> = properties
            .lines()
            .filter(|line| line.starts_with("        ") && !line.starts_with("          "))
            .filter_map(|line| line.trim().split_once(':').map(|(name, _)| name))
            .collect();
        assert_eq!(
            actual,
            fields.into_iter().collect(),
            "{name} must never acquire token conversion or identity selectors"
        );
        let mut lines = schema.lines();
        let required_line = lines
            .find(|line| line.starts_with("      required:"))
            .expect("explicit required set");
        let inline = required_line
            .trim()
            .strip_prefix("required:")
            .unwrap()
            .trim();
        let required: BTreeSet<String> = if inline.starts_with('[') {
            inline
                .trim_matches(['[', ']'])
                .split(',')
                .map(|item| item.trim().trim_matches(['\"', '\'']).to_owned())
                .collect()
        } else {
            lines
                .take_while(|line| line.trim().starts_with("- "))
                .map(|line| {
                    line.trim()
                        .trim_start_matches("- ")
                        .trim_matches(['\"', '\''])
                        .to_owned()
                })
                .collect()
        };
        let expected_required: BTreeSet<String> =
            actual.iter().map(|item| (*item).to_owned()).collect();
        assert_eq!(
            required, expected_required,
            "exact responses and identity binding cannot make mandatory fields optional"
        );
        assert!(!schema.contains("access_token") && !schema.contains("refresh_token"));
    }
    let scheme = OPENAPI_YAML
        .split_once("    BrowserIngress:\n")
        .expect("real header credential scheme")
        .1
        .lines()
        .take_while(|line| line.starts_with("      ") || line.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        scheme.contains("type: apiKey")
            && scheme.contains("in: header")
            && scheme.contains("name: x-console-browser-ingress")
    );
    let history = openapi_schema_body(OPENAPI_YAML, "BrowserSessionAttendanceResponse");
    assert!(
        history.contains("EmployeeAttendanceRecordPage"),
        "legacy timestamp/page contract is preserved"
    );
    let credential = openapi_schema_body(OPENAPI_YAML, "BrowserSessionIdentityRequest");
    assert!(
        credential.contains("minLength: 47")
            && credential.contains("maxLength: 47")
            && credential.contains("bs1"),
        "handle codec has exact wire bounds"
    );
}
