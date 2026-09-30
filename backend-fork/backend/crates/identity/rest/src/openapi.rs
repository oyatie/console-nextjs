//! This face's slice of the published OpenAPI contract.
//!
//! Bodies live as YAML under `openapi/` and are pulled in via `include_str!`.
//! `console_contracts` re-indents them; composition rejects duplicate keys.

use console_contracts::{Fragment, NamedYaml, Operation, PathItem};

/// This face's contribution to the composed OpenAPI document.
pub const OPENAPI_FRAGMENT: Fragment = Fragment {
    source: "console-identity-rest",
    paths: PATHS,
    schemas: SCHEMAS,
    parameters: &[],
    responses: &[],
    security_schemes: &[],
    external_schemas: EXTERNAL_SCHEMAS,
};

const EXTERNAL_SCHEMAS: &[&str] = &[
    "ErrorBody",
    "MobilePasskeyStepUpBinding",
    "MobileStepUpActionKind",
    "OrgChangeDetail",
    "PasskeyStepUpAssertion",
    "Timestamp",
    "Uuid",
];

const PATHS: &[PathItem] = &[
    PathItem {
        path: "/.well-known/apple-app-site-association",
        operations: &[Operation {
            method: "get",
            body: include_str!("../openapi/paths/.well-known__apple-app-site-association.get.yaml"),
        }],
    },
    PathItem {
        path: "/.well-known/assetlinks.json",
        operations: &[Operation {
            method: "get",
            body: include_str!("../openapi/paths/.well-known__assetlinks.json.get.yaml"),
        }],
    },
    PathItem {
        path: "/api/platform/groups",
        operations: &[
            Operation {
                method: "get",
                body: include_str!("../openapi/paths/api__platform__groups.get.yaml"),
            },
            Operation {
                method: "post",
                body: include_str!("../openapi/paths/api__platform__groups.post.yaml"),
            },
        ],
    },
    PathItem {
        path: "/api/platform/groups/{id}",
        operations: &[Operation {
            method: "patch",
            body: include_str!("../openapi/paths/api__platform__groups__id.patch.yaml"),
        }],
    },
    PathItem {
        path: "/api/platform/groups/{id}/accounts",
        operations: &[
            Operation {
                method: "get",
                body: include_str!("../openapi/paths/api__platform__groups__id__accounts.get.yaml"),
            },
            Operation {
                method: "post",
                body: include_str!(
                    "../openapi/paths/api__platform__groups__id__accounts.post.yaml"
                ),
            },
        ],
    },
    PathItem {
        path: "/api/platform/groups/{id}/accounts/{user_id}/roles/{group_role}",
        operations: &[Operation {
            method: "delete",
            body: include_str!(
                "../openapi/paths/api__platform__groups__id__accounts__user_id__roles__group_role.delete.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/platform/groups/{id}/organizations/{org_id}",
        operations: &[
            Operation {
                method: "delete",
                body: include_str!(
                    "../openapi/paths/api__platform__groups__id__organizations__org_id.delete.yaml"
                ),
            },
            Operation {
                method: "put",
                body: include_str!(
                    "../openapi/paths/api__platform__groups__id__organizations__org_id.put.yaml"
                ),
            },
        ],
    },
    PathItem {
        path: "/api/platform/ops",
        operations: &[Operation {
            method: "get",
            body: include_str!("../openapi/paths/api__platform__ops.get.yaml"),
        }],
    },
    PathItem {
        path: "/api/platform/orgs",
        operations: &[
            Operation {
                method: "get",
                body: include_str!("../openapi/paths/api__platform__orgs.get.yaml"),
            },
            Operation {
                method: "post",
                body: include_str!("../openapi/paths/api__platform__orgs.post.yaml"),
            },
        ],
    },
    PathItem {
        path: "/api/platform/orgs/{id}",
        operations: &[
            Operation {
                method: "delete",
                body: include_str!("../openapi/paths/api__platform__orgs__id.delete.yaml"),
            },
            Operation {
                method: "patch",
                body: include_str!("../openapi/paths/api__platform__orgs__id.patch.yaml"),
            },
        ],
    },
    PathItem {
        path: "/api/platform/tenant-context",
        operations: &[Operation {
            method: "post",
            body: include_str!("../openapi/paths/api__platform__tenant-context.post.yaml"),
        }],
    },
    PathItem {
        path: "/api/platform/tenant-context/exit",
        operations: &[Operation {
            method: "post",
            body: include_str!("../openapi/paths/api__platform__tenant-context__exit.post.yaml"),
        }],
    },
    PathItem {
        path: "/api/platform/view-as",
        operations: &[Operation {
            method: "post",
            body: include_str!("../openapi/paths/api__platform__view-as.post.yaml"),
        }],
    },
    PathItem {
        path: "/api/platform/view-as/exit",
        operations: &[Operation {
            method: "post",
            body: include_str!("../openapi/paths/api__platform__view-as__exit.post.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/auth/admin/credential-reset",
        operations: &[Operation {
            method: "post",
            body: include_str!("../openapi/paths/api__v1__auth__admin__credential-reset.post.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/auth/admin/otp/issue",
        operations: &[Operation {
            method: "post",
            body: include_str!("../openapi/paths/api__v1__auth__admin__otp__issue.post.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/auth/device-login/approve",
        operations: &[Operation {
            method: "post",
            body: include_str!("../openapi/paths/api__v1__auth__device-login__approve.post.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/auth/device-login/approve-session",
        operations: &[Operation {
            method: "post",
            body: include_str!(
                "../openapi/paths/api__v1__auth__device-login__approve-session.post.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/auth/device-login/poll",
        operations: &[Operation {
            method: "post",
            body: include_str!("../openapi/paths/api__v1__auth__device-login__poll.post.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/auth/device-login/start",
        operations: &[Operation {
            method: "post",
            body: include_str!("../openapi/paths/api__v1__auth__device-login__start.post.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/auth/logout",
        operations: &[Operation {
            method: "post",
            body: include_str!("../openapi/paths/api__v1__auth__logout.post.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/auth/otp/redeem",
        operations: &[Operation {
            method: "post",
            body: include_str!("../openapi/paths/api__v1__auth__otp__redeem.post.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/auth/passkey/enroll-handoff",
        operations: &[Operation {
            method: "post",
            body: include_str!("../openapi/paths/api__v1__auth__passkey__enroll-handoff.post.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/auth/passkey/login/finish",
        operations: &[Operation {
            method: "post",
            body: include_str!("../openapi/paths/api__v1__auth__passkey__login__finish.post.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/auth/passkey/login/start",
        operations: &[Operation {
            method: "post",
            body: include_str!("../openapi/paths/api__v1__auth__passkey__login__start.post.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/auth/passkey/register/finish",
        operations: &[Operation {
            method: "post",
            body: include_str!(
                "../openapi/paths/api__v1__auth__passkey__register__finish.post.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/auth/passkey/register/start",
        operations: &[Operation {
            method: "post",
            body: include_str!(
                "../openapi/paths/api__v1__auth__passkey__register__start.post.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/auth/passkey/step-up/start",
        operations: &[Operation {
            method: "post",
            body: include_str!("../openapi/paths/api__v1__auth__passkey__step-up__start.post.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/auth/passkeys",
        operations: &[Operation {
            method: "get",
            body: include_str!("../openapi/paths/api__v1__auth__passkeys.get.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/auth/passkeys/{id}",
        operations: &[Operation {
            method: "delete",
            body: include_str!("../openapi/paths/api__v1__auth__passkeys__id.delete.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/auth/privacy-consent/accept",
        operations: &[Operation {
            method: "post",
            body: include_str!("../openapi/paths/api__v1__auth__privacy-consent__accept.post.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/auth/privacy-consent/status",
        operations: &[Operation {
            method: "post",
            body: include_str!("../openapi/paths/api__v1__auth__privacy-consent__status.post.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/auth/signup",
        operations: &[Operation {
            method: "post",
            body: include_str!("../openapi/paths/api__v1__auth__signup.post.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/auth/token/refresh",
        operations: &[Operation {
            method: "post",
            body: include_str!("../openapi/paths/api__v1__auth__token__refresh.post.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/branches",
        operations: &[
            Operation {
                method: "get",
                body: include_str!("../openapi/paths/api__v1__branches.get.yaml"),
            },
            Operation {
                method: "post",
                body: include_str!("../openapi/paths/api__v1__branches.post.yaml"),
            },
        ],
    },
    PathItem {
        path: "/api/v1/branches/{id}",
        operations: &[
            Operation {
                method: "delete",
                body: include_str!("../openapi/paths/api__v1__branches__id.delete.yaml"),
            },
            Operation {
                method: "get",
                body: include_str!("../openapi/paths/api__v1__branches__id.get.yaml"),
            },
            Operation {
                method: "patch",
                body: include_str!("../openapi/paths/api__v1__branches__id.patch.yaml"),
            },
        ],
    },
    PathItem {
        path: "/api/v1/console/kill-switch",
        operations: &[Operation {
            method: "post",
            body: include_str!("../openapi/paths/api__v1__console__kill-switch.post.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/console/rollout",
        operations: &[Operation {
            method: "get",
            body: include_str!("../openapi/paths/api__v1__console__rollout.get.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/console/rollout/opt-in",
        operations: &[Operation {
            method: "put",
            body: include_str!("../openapi/paths/api__v1__console__rollout__opt-in.put.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/console/rollout/org-flag",
        operations: &[Operation {
            method: "put",
            body: include_str!("../openapi/paths/api__v1__console__rollout__org-flag.put.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/console/telemetry/route",
        operations: &[Operation {
            method: "post",
            body: include_str!("../openapi/paths/api__v1__console__telemetry__route.post.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/directory/people",
        operations: &[Operation {
            method: "get",
            body: include_str!("../openapi/paths/api__v1__directory__people.get.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/group-admin/groups",
        operations: &[Operation {
            method: "get",
            body: include_str!("../openapi/paths/api__v1__group-admin__groups.get.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/group-admin/tenant-context",
        operations: &[Operation {
            method: "post",
            body: include_str!("../openapi/paths/api__v1__group-admin__tenant-context.post.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/group-admin/tenant-context/exit",
        operations: &[Operation {
            method: "post",
            body: include_str!(
                "../openapi/paths/api__v1__group-admin__tenant-context__exit.post.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/me/action-inbox",
        operations: &[Operation {
            method: "get",
            body: include_str!("../openapi/paths/api__v1__me__action-inbox.get.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/me/authz",
        operations: &[Operation {
            method: "get",
            body: include_str!("../openapi/paths/api__v1__me__authz.get.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/me/workbench",
        operations: &[Operation {
            method: "get",
            body: include_str!("../openapi/paths/api__v1__me__workbench.get.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/me/workspace",
        operations: &[
            Operation {
                method: "get",
                body: include_str!("../openapi/paths/api__v1__me__workspace.get.yaml"),
            },
            Operation {
                method: "put",
                body: include_str!("../openapi/paths/api__v1__me__workspace.put.yaml"),
            },
        ],
    },
    PathItem {
        path: "/api/v1/passkeys",
        operations: &[Operation {
            method: "get",
            body: include_str!("../openapi/paths/api__v1__passkeys.get.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/passkeys/{id}",
        operations: &[Operation {
            method: "delete",
            body: include_str!("../openapi/paths/api__v1__passkeys__id.delete.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/policy/assignments",
        operations: &[Operation {
            method: "get",
            body: include_str!("../openapi/paths/api__v1__policy__assignments.get.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/policy/audit-events",
        operations: &[Operation {
            method: "get",
            body: include_str!("../openapi/paths/api__v1__policy__audit-events.get.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/policy/authorize",
        operations: &[Operation {
            method: "post",
            body: include_str!("../openapi/paths/api__v1__policy__authorize.post.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/policy/authorize/bulk",
        operations: &[Operation {
            method: "post",
            body: include_str!("../openapi/paths/api__v1__policy__authorize__bulk.post.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/policy/catalog",
        operations: &[Operation {
            method: "get",
            body: include_str!("../openapi/paths/api__v1__policy__catalog.get.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/policy/decisions",
        operations: &[Operation {
            method: "get",
            body: include_str!("../openapi/paths/api__v1__policy__decisions.get.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/policy/drafts",
        operations: &[
            Operation {
                method: "get",
                body: include_str!("../openapi/paths/api__v1__policy__drafts.get.yaml"),
            },
            Operation {
                method: "post",
                body: include_str!("../openapi/paths/api__v1__policy__drafts.post.yaml"),
            },
        ],
    },
    PathItem {
        path: "/api/v1/policy/drafts/{draft_id}",
        operations: &[
            Operation {
                method: "get",
                body: include_str!("../openapi/paths/api__v1__policy__drafts__draft_id.get.yaml"),
            },
            Operation {
                method: "put",
                body: include_str!("../openapi/paths/api__v1__policy__drafts__draft_id.put.yaml"),
            },
        ],
    },
    PathItem {
        path: "/api/v1/policy/drafts/{draft_id}/review",
        operations: &[Operation {
            method: "post",
            body: include_str!(
                "../openapi/paths/api__v1__policy__drafts__draft_id__review.post.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/policy/drafts/{draft_id}/submit",
        operations: &[Operation {
            method: "post",
            body: include_str!(
                "../openapi/paths/api__v1__policy__drafts__draft_id__submit.post.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/policy/drafts/{draft_id}/validate",
        operations: &[Operation {
            method: "post",
            body: include_str!(
                "../openapi/paths/api__v1__policy__drafts__draft_id__validate.post.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/policy/features",
        operations: &[Operation {
            method: "get",
            body: include_str!("../openapi/paths/api__v1__policy__features.get.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/policy/role-templates",
        operations: &[Operation {
            method: "get",
            body: include_str!("../openapi/paths/api__v1__policy__role-templates.get.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/policy/roles",
        operations: &[
            Operation {
                method: "get",
                body: include_str!("../openapi/paths/api__v1__policy__roles.get.yaml"),
            },
            Operation {
                method: "post",
                body: include_str!("../openapi/paths/api__v1__policy__roles.post.yaml"),
            },
        ],
    },
    PathItem {
        path: "/api/v1/policy/roles/{id}",
        operations: &[Operation {
            method: "patch",
            body: include_str!("../openapi/paths/api__v1__policy__roles__id.patch.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/policy/roles/{id}/status",
        operations: &[Operation {
            method: "patch",
            body: include_str!("../openapi/paths/api__v1__policy__roles__id__status.patch.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/policy/roles/{id}/status-preview",
        operations: &[Operation {
            method: "post",
            body: include_str!(
                "../openapi/paths/api__v1__policy__roles__id__status-preview.post.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/policy/simulate",
        operations: &[Operation {
            method: "post",
            body: include_str!("../openapi/paths/api__v1__policy__simulate.post.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/policy/users/{id}/assignment-preview",
        operations: &[Operation {
            method: "post",
            body: include_str!(
                "../openapi/paths/api__v1__policy__users__id__assignment-preview.post.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/policy/users/{id}/assignments",
        operations: &[Operation {
            method: "put",
            body: include_str!("../openapi/paths/api__v1__policy__users__id__assignments.put.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/regions",
        operations: &[
            Operation {
                method: "get",
                body: include_str!("../openapi/paths/api__v1__regions.get.yaml"),
            },
            Operation {
                method: "post",
                body: include_str!("../openapi/paths/api__v1__regions.post.yaml"),
            },
        ],
    },
    PathItem {
        path: "/api/v1/regions/{id}",
        operations: &[
            Operation {
                method: "delete",
                body: include_str!("../openapi/paths/api__v1__regions__id.delete.yaml"),
            },
            Operation {
                method: "patch",
                body: include_str!("../openapi/paths/api__v1__regions__id.patch.yaml"),
            },
        ],
    },
    PathItem {
        path: "/api/v1/users",
        operations: &[
            Operation {
                method: "get",
                body: include_str!("../openapi/paths/api__v1__users.get.yaml"),
            },
            Operation {
                method: "post",
                body: include_str!("../openapi/paths/api__v1__users.post.yaml"),
            },
        ],
    },
    PathItem {
        path: "/api/v1/users/me",
        operations: &[
            Operation {
                method: "get",
                body: include_str!("../openapi/paths/api__v1__users__me.get.yaml"),
            },
            Operation {
                method: "patch",
                body: include_str!("../openapi/paths/api__v1__users__me.patch.yaml"),
            },
        ],
    },
    PathItem {
        path: "/api/v1/users/{id}",
        operations: &[
            Operation {
                method: "get",
                body: include_str!("../openapi/paths/api__v1__users__id.get.yaml"),
            },
            Operation {
                method: "patch",
                body: include_str!("../openapi/paths/api__v1__users__id.patch.yaml"),
            },
        ],
    },
    PathItem {
        path: "/api/v1/users/{id}/activate",
        operations: &[Operation {
            method: "post",
            body: include_str!("../openapi/paths/api__v1__users__id__activate.post.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/users/{id}/deactivate",
        operations: &[Operation {
            method: "post",
            body: include_str!("../openapi/paths/api__v1__users__id__deactivate.post.yaml"),
        }],
    },
    PathItem {
        path: "/healthz",
        operations: &[Operation {
            method: "get",
            body: include_str!("../openapi/paths/healthz.get.yaml"),
        }],
    },
    PathItem {
        path: "/readyz",
        operations: &[Operation {
            method: "get",
            body: include_str!("../openapi/paths/readyz.get.yaml"),
        }],
    },
];

const SCHEMAS: &[NamedYaml] = &[
    NamedYaml {
        name: "AccountStatus",
        body: include_str!("../openapi/schemas/AccountStatus.yaml"),
    },
    NamedYaml {
        name: "ActionInboxItem",
        body: include_str!("../openapi/schemas/ActionInboxItem.yaml"),
    },
    NamedYaml {
        name: "ActionInboxLink",
        body: include_str!("../openapi/schemas/ActionInboxLink.yaml"),
    },
    NamedYaml {
        name: "ActionInboxResponse",
        body: include_str!("../openapi/schemas/ActionInboxResponse.yaml"),
    },
    NamedYaml {
        name: "AdminCredentialResetRequest",
        body: include_str!("../openapi/schemas/AdminCredentialResetRequest.yaml"),
    },
    NamedYaml {
        name: "AdminCredentialResetResponse",
        body: include_str!("../openapi/schemas/AdminCredentialResetResponse.yaml"),
    },
    NamedYaml {
        name: "AdminIssueOtpRequest",
        body: include_str!("../openapi/schemas/AdminIssueOtpRequest.yaml"),
    },
    NamedYaml {
        name: "AdminIssueOtpResponse",
        body: include_str!("../openapi/schemas/AdminIssueOtpResponse.yaml"),
    },
    NamedYaml {
        name: "AndroidAssetLinkStatement",
        body: include_str!("../openapi/schemas/AndroidAssetLinkStatement.yaml"),
    },
    NamedYaml {
        name: "AppleAppSiteAssociation",
        body: include_str!("../openapi/schemas/AppleAppSiteAssociation.yaml"),
    },
    NamedYaml {
        name: "BranchScope",
        body: include_str!("../openapi/schemas/BranchScope.yaml"),
    },
    NamedYaml {
        name: "BranchSummary",
        body: include_str!("../openapi/schemas/BranchSummary.yaml"),
    },
    NamedYaml {
        name: "BulkAuthorizeBody",
        body: include_str!("../openapi/schemas/BulkAuthorizeBody.yaml"),
    },
    NamedYaml {
        name: "BulkDecisionResponse",
        body: include_str!("../openapi/schemas/BulkDecisionResponse.yaml"),
    },
    NamedYaml {
        name: "ConsoleRouteSurface",
        body: include_str!("../openapi/schemas/ConsoleRouteSurface.yaml"),
    },
    NamedYaml {
        name: "ConsoleRouteTelemetryAccepted",
        body: include_str!("../openapi/schemas/ConsoleRouteTelemetryAccepted.yaml"),
    },
    NamedYaml {
        name: "ConsoleRouteTelemetryEventKind",
        body: include_str!("../openapi/schemas/ConsoleRouteTelemetryEventKind.yaml"),
    },
    NamedYaml {
        name: "ConsoleRouteTelemetryRequest",
        body: include_str!("../openapi/schemas/ConsoleRouteTelemetryRequest.yaml"),
    },
    NamedYaml {
        name: "CreateBranchRequest",
        body: include_str!("../openapi/schemas/CreateBranchRequest.yaml"),
    },
    NamedYaml {
        name: "CreatePlatformGroupAccountRequest",
        body: include_str!("../openapi/schemas/CreatePlatformGroupAccountRequest.yaml"),
    },
    NamedYaml {
        name: "CreatePlatformGroupAccountResponse",
        body: include_str!("../openapi/schemas/CreatePlatformGroupAccountResponse.yaml"),
    },
    NamedYaml {
        name: "CreatePlatformGroupRequest",
        body: include_str!("../openapi/schemas/CreatePlatformGroupRequest.yaml"),
    },
    NamedYaml {
        name: "CreatePlatformOrgRequest",
        body: include_str!("../openapi/schemas/CreatePlatformOrgRequest.yaml"),
    },
    NamedYaml {
        name: "CreatePolicyRoleRequest",
        body: include_str!("../openapi/schemas/CreatePolicyRoleRequest.yaml"),
    },
    NamedYaml {
        name: "CreateRegionRequest",
        body: include_str!("../openapi/schemas/CreateRegionRequest.yaml"),
    },
    NamedYaml {
        name: "CreateUserRequest",
        body: include_str!("../openapi/schemas/CreateUserRequest.yaml"),
    },
    NamedYaml {
        name: "DecisionLogRow",
        body: include_str!("../openapi/schemas/DecisionLogRow.yaml"),
    },
    NamedYaml {
        name: "DeviceLoginApproveRequest",
        body: include_str!("../openapi/schemas/DeviceLoginApproveRequest.yaml"),
    },
    NamedYaml {
        name: "DeviceLoginApproveSessionRequest",
        body: include_str!("../openapi/schemas/DeviceLoginApproveSessionRequest.yaml"),
    },
    NamedYaml {
        name: "DeviceLoginPollRequest",
        body: include_str!("../openapi/schemas/DeviceLoginPollRequest.yaml"),
    },
    NamedYaml {
        name: "DeviceLoginPollResponse",
        body: include_str!("../openapi/schemas/DeviceLoginPollResponse.yaml"),
    },
    NamedYaml {
        name: "DeviceLoginStartResponse",
        body: include_str!("../openapi/schemas/DeviceLoginStartResponse.yaml"),
    },
    NamedYaml {
        name: "DirectoryPage",
        body: include_str!("../openapi/schemas/DirectoryPage.yaml"),
    },
    NamedYaml {
        name: "DirectoryPerson",
        body: include_str!("../openapi/schemas/DirectoryPerson.yaml"),
    },
    NamedYaml {
        name: "EnrollHandoffRequest",
        body: include_str!("../openapi/schemas/EnrollHandoffRequest.yaml"),
    },
    NamedYaml {
        name: "EnrollHandoffResponse",
        body: include_str!("../openapi/schemas/EnrollHandoffResponse.yaml"),
    },
    NamedYaml {
        name: "LogoutRequest",
        body: include_str!("../openapi/schemas/LogoutRequest.yaml"),
    },
    NamedYaml {
        name: "MeAuthzCapability",
        body: include_str!("../openapi/schemas/MeAuthzCapability.yaml"),
    },
    NamedYaml {
        name: "MeAuthzResponse",
        body: include_str!("../openapi/schemas/MeAuthzResponse.yaml"),
    },
    NamedYaml {
        name: "MobilePasskeyStepUpStartRequest",
        body: include_str!("../openapi/schemas/MobilePasskeyStepUpStartRequest.yaml"),
    },
    NamedYaml {
        name: "MobilePasskeyStepUpStartResponse",
        body: include_str!("../openapi/schemas/MobilePasskeyStepUpStartResponse.yaml"),
    },
    NamedYaml {
        name: "MyWorkbenchResponse",
        body: include_str!("../openapi/schemas/MyWorkbenchResponse.yaml"),
    },
    NamedYaml {
        name: "OtpRedeemRequest",
        body: include_str!("../openapi/schemas/OtpRedeemRequest.yaml"),
    },
    NamedYaml {
        name: "OtpRedeemResponse",
        body: include_str!("../openapi/schemas/OtpRedeemResponse.yaml"),
    },
    NamedYaml {
        name: "PasskeyLoginFinishRequest",
        body: include_str!("../openapi/schemas/PasskeyLoginFinishRequest.yaml"),
    },
    NamedYaml {
        name: "PasskeyLoginStartResponse",
        body: include_str!("../openapi/schemas/PasskeyLoginStartResponse.yaml"),
    },
    NamedYaml {
        name: "PasskeyRegisterFinishRequest",
        body: include_str!("../openapi/schemas/PasskeyRegisterFinishRequest.yaml"),
    },
    NamedYaml {
        name: "PasskeyRegisterFinishResponse",
        body: include_str!("../openapi/schemas/PasskeyRegisterFinishResponse.yaml"),
    },
    NamedYaml {
        name: "PasskeyRegisterStartRequest",
        body: include_str!("../openapi/schemas/PasskeyRegisterStartRequest.yaml"),
    },
    NamedYaml {
        name: "PasskeyRegisterStartResponse",
        body: include_str!("../openapi/schemas/PasskeyRegisterStartResponse.yaml"),
    },
    NamedYaml {
        name: "PasskeySummary",
        body: include_str!("../openapi/schemas/PasskeySummary.yaml"),
    },
    NamedYaml {
        name: "PlatformAccountStatus",
        body: include_str!("../openapi/schemas/PlatformAccountStatus.yaml"),
    },
    NamedYaml {
        name: "PlatformExitResponse",
        body: include_str!("../openapi/schemas/PlatformExitResponse.yaml"),
    },
    NamedYaml {
        name: "PlatformGroup",
        body: include_str!("../openapi/schemas/PlatformGroup.yaml"),
    },
    NamedYaml {
        name: "PlatformGroupAccount",
        body: include_str!("../openapi/schemas/PlatformGroupAccount.yaml"),
    },
    NamedYaml {
        name: "PlatformGroupMember",
        body: include_str!("../openapi/schemas/PlatformGroupMember.yaml"),
    },
    NamedYaml {
        name: "PlatformGroupRole",
        body: include_str!("../openapi/schemas/PlatformGroupRole.yaml"),
    },
    NamedYaml {
        name: "PlatformOpsResponse",
        body: include_str!("../openapi/schemas/PlatformOpsResponse.yaml"),
    },
    NamedYaml {
        name: "PlatformOrg",
        body: include_str!("../openapi/schemas/PlatformOrg.yaml"),
    },
    NamedYaml {
        name: "PlatformOrgOnboardingResponse",
        body: include_str!("../openapi/schemas/PlatformOrgOnboardingResponse.yaml"),
    },
    NamedYaml {
        name: "PlatformOrgStatus",
        body: include_str!("../openapi/schemas/PlatformOrgStatus.yaml"),
    },
    NamedYaml {
        name: "PlatformTenantContextStartRequest",
        body: include_str!("../openapi/schemas/PlatformTenantContextStartRequest.yaml"),
    },
    NamedYaml {
        name: "PlatformTenantContextStartResponse",
        body: include_str!("../openapi/schemas/PlatformTenantContextStartResponse.yaml"),
    },
    NamedYaml {
        name: "PlatformTenantHealth",
        body: include_str!("../openapi/schemas/PlatformTenantHealth.yaml"),
    },
    NamedYaml {
        name: "PlatformTenantRole",
        body: include_str!("../openapi/schemas/PlatformTenantRole.yaml"),
    },
    NamedYaml {
        name: "PlatformViewAsStartRequest",
        body: include_str!("../openapi/schemas/PlatformViewAsStartRequest.yaml"),
    },
    NamedYaml {
        name: "PlatformViewAsStartResponse",
        body: include_str!("../openapi/schemas/PlatformViewAsStartResponse.yaml"),
    },
    NamedYaml {
        name: "PolicyAssignmentPreviewResponse",
        body: include_str!("../openapi/schemas/PolicyAssignmentPreviewResponse.yaml"),
    },
    NamedYaml {
        name: "PolicyAuditEventResponse",
        body: include_str!("../openapi/schemas/PolicyAuditEventResponse.yaml"),
    },
    NamedYaml {
        name: "PolicyAuthorizeRequest",
        body: include_str!("../openapi/schemas/PolicyAuthorizeRequest.yaml"),
    },
    NamedYaml {
        name: "PolicyConditionResponse",
        body: include_str!("../openapi/schemas/PolicyConditionResponse.yaml"),
    },
    NamedYaml {
        name: "PolicyCreateDraftRequest",
        body: include_str!("../openapi/schemas/PolicyCreateDraftRequest.yaml"),
    },
    NamedYaml {
        name: "PolicyDefaultPermissionResponse",
        body: include_str!("../openapi/schemas/PolicyDefaultPermissionResponse.yaml"),
    },
    NamedYaml {
        name: "PolicyFeatureGrantPreviewResponse",
        body: include_str!("../openapi/schemas/PolicyFeatureGrantPreviewResponse.yaml"),
    },
    NamedYaml {
        name: "PolicyFeatureResponse",
        body: include_str!("../openapi/schemas/PolicyFeatureResponse.yaml"),
    },
    NamedYaml {
        name: "PolicyNoCodeBlocks",
        body: include_str!("../openapi/schemas/PolicyNoCodeBlocks.yaml"),
    },
    NamedYaml {
        name: "PolicyNoCodeCondition",
        body: include_str!("../openapi/schemas/PolicyNoCodeCondition.yaml"),
    },
    NamedYaml {
        name: "PolicyPermissionResponse",
        body: include_str!("../openapi/schemas/PolicyPermissionResponse.yaml"),
    },
    NamedYaml {
        name: "PolicyReviewRequest",
        body: include_str!("../openapi/schemas/PolicyReviewRequest.yaml"),
    },
    NamedYaml {
        name: "PolicyRoleAssignmentDeltaResponse",
        body: include_str!("../openapi/schemas/PolicyRoleAssignmentDeltaResponse.yaml"),
    },
    NamedYaml {
        name: "PolicyRoleAssignmentResponse",
        body: include_str!("../openapi/schemas/PolicyRoleAssignmentResponse.yaml"),
    },
    NamedYaml {
        name: "PolicyRoleCatalogResponse",
        body: include_str!("../openapi/schemas/PolicyRoleCatalogResponse.yaml"),
    },
    NamedYaml {
        name: "PolicyRoleImpactResponse",
        body: include_str!("../openapi/schemas/PolicyRoleImpactResponse.yaml"),
    },
    NamedYaml {
        name: "PolicyRoleResponse",
        body: include_str!("../openapi/schemas/PolicyRoleResponse.yaml"),
    },
    NamedYaml {
        name: "PolicyRoleStatusPreviewRequest",
        body: include_str!("../openapi/schemas/PolicyRoleStatusPreviewRequest.yaml"),
    },
    NamedYaml {
        name: "PolicyRoleStatusPreviewResponse",
        body: include_str!("../openapi/schemas/PolicyRoleStatusPreviewResponse.yaml"),
    },
    NamedYaml {
        name: "PolicyRoleTemplateResponse",
        body: include_str!("../openapi/schemas/PolicyRoleTemplateResponse.yaml"),
    },
    NamedYaml {
        name: "PolicySimRequest",
        body: include_str!("../openapi/schemas/PolicySimRequest.yaml"),
    },
    NamedYaml {
        name: "PolicySimResource",
        body: include_str!("../openapi/schemas/PolicySimResource.yaml"),
    },
    NamedYaml {
        name: "PolicySimSubject",
        body: include_str!("../openapi/schemas/PolicySimSubject.yaml"),
    },
    NamedYaml {
        name: "PolicySimulateRequest",
        body: include_str!("../openapi/schemas/PolicySimulateRequest.yaml"),
    },
    NamedYaml {
        name: "PolicyUpdateDraftRequest",
        body: include_str!("../openapi/schemas/PolicyUpdateDraftRequest.yaml"),
    },
    NamedYaml {
        name: "PolicyVersionResponse",
        body: include_str!("../openapi/schemas/PolicyVersionResponse.yaml"),
    },
    NamedYaml {
        name: "PrivacyConsentAcceptRequest",
        body: include_str!("../openapi/schemas/PrivacyConsentAcceptRequest.yaml"),
    },
    NamedYaml {
        name: "PrivacyConsentStatusResponse",
        body: include_str!("../openapi/schemas/PrivacyConsentStatusResponse.yaml"),
    },
    NamedYaml {
        name: "RefreshTokenRequest",
        body: include_str!("../openapi/schemas/RefreshTokenRequest.yaml"),
    },
    NamedYaml {
        name: "RegionSummary",
        body: include_str!("../openapi/schemas/RegionSummary.yaml"),
    },
    NamedYaml {
        name: "ReplacePolicyRoleAssignmentsRequest",
        body: include_str!("../openapi/schemas/ReplacePolicyRoleAssignmentsRequest.yaml"),
    },
    NamedYaml {
        name: "RouteAdoptionMetric",
        body: include_str!("../openapi/schemas/RouteAdoptionMetric.yaml"),
    },
    NamedYaml {
        name: "SignupRequest",
        body: include_str!("../openapi/schemas/SignupRequest.yaml"),
    },
    NamedYaml {
        name: "SignupResponse",
        body: include_str!("../openapi/schemas/SignupResponse.yaml"),
    },
    NamedYaml {
        name: "SimulationOutcome",
        body: include_str!("../openapi/schemas/SimulationOutcome.yaml"),
    },
    NamedYaml {
        name: "SystemPolicyRoleResponse",
        body: include_str!("../openapi/schemas/SystemPolicyRoleResponse.yaml"),
    },
    NamedYaml {
        name: "Team",
        body: include_str!("../openapi/schemas/Team.yaml"),
    },
    NamedYaml {
        name: "TokenPairResponse",
        body: include_str!("../openapi/schemas/TokenPairResponse.yaml"),
    },
    NamedYaml {
        name: "UpdateBranchRequest",
        body: include_str!("../openapi/schemas/UpdateBranchRequest.yaml"),
    },
    NamedYaml {
        name: "UpdatePlatformGroupRequest",
        body: include_str!("../openapi/schemas/UpdatePlatformGroupRequest.yaml"),
    },
    NamedYaml {
        name: "UpdatePlatformOrgRequest",
        body: include_str!("../openapi/schemas/UpdatePlatformOrgRequest.yaml"),
    },
    NamedYaml {
        name: "UpdatePolicyRoleRequest",
        body: include_str!("../openapi/schemas/UpdatePolicyRoleRequest.yaml"),
    },
    NamedYaml {
        name: "UpdatePolicyRoleStatusRequest",
        body: include_str!("../openapi/schemas/UpdatePolicyRoleStatusRequest.yaml"),
    },
    NamedYaml {
        name: "UpdateRegionRequest",
        body: include_str!("../openapi/schemas/UpdateRegionRequest.yaml"),
    },
    NamedYaml {
        name: "UpdateSelfProfileRequest",
        body: include_str!("../openapi/schemas/UpdateSelfProfileRequest.yaml"),
    },
    NamedYaml {
        name: "UpdateUserRequest",
        body: include_str!("../openapi/schemas/UpdateUserRequest.yaml"),
    },
    NamedYaml {
        name: "UserPage",
        body: include_str!("../openapi/schemas/UserPage.yaml"),
    },
    NamedYaml {
        name: "UserSummary",
        body: include_str!("../openapi/schemas/UserSummary.yaml"),
    },
    NamedYaml {
        name: "WorkbenchActionInboxItem",
        body: include_str!("../openapi/schemas/WorkbenchActionInboxItem.yaml"),
    },
    NamedYaml {
        name: "WorkbenchActionSourceEnvelope",
        body: include_str!("../openapi/schemas/WorkbenchActionSourceEnvelope.yaml"),
    },
    NamedYaml {
        name: "WorkbenchActionSourceOk",
        body: include_str!("../openapi/schemas/WorkbenchActionSourceOk.yaml"),
    },
    NamedYaml {
        name: "WorkbenchCalendarItem",
        body: include_str!("../openapi/schemas/WorkbenchCalendarItem.yaml"),
    },
    NamedYaml {
        name: "WorkbenchCalendarSourceEnvelope",
        body: include_str!("../openapi/schemas/WorkbenchCalendarSourceEnvelope.yaml"),
    },
    NamedYaml {
        name: "WorkbenchCalendarSourceOk",
        body: include_str!("../openapi/schemas/WorkbenchCalendarSourceOk.yaml"),
    },
    NamedYaml {
        name: "WorkbenchDeniedSourceEnvelope",
        body: include_str!("../openapi/schemas/WorkbenchDeniedSourceEnvelope.yaml"),
    },
    NamedYaml {
        name: "WorkbenchEffectiveScope",
        body: include_str!("../openapi/schemas/WorkbenchEffectiveScope.yaml"),
    },
    NamedYaml {
        name: "WorkbenchRange",
        body: include_str!("../openapi/schemas/WorkbenchRange.yaml"),
    },
    NamedYaml {
        name: "WorkbenchScopeAll",
        body: include_str!("../openapi/schemas/WorkbenchScopeAll.yaml"),
    },
    NamedYaml {
        name: "WorkbenchScopeBranches",
        body: include_str!("../openapi/schemas/WorkbenchScopeBranches.yaml"),
    },
    NamedYaml {
        name: "WorkbenchSourceRef",
        body: include_str!("../openapi/schemas/WorkbenchSourceRef.yaml"),
    },
    NamedYaml {
        name: "WorkbenchTarget",
        body: include_str!("../openapi/schemas/WorkbenchTarget.yaml"),
    },
    NamedYaml {
        name: "WorkbenchTodoItem",
        body: include_str!("../openapi/schemas/WorkbenchTodoItem.yaml"),
    },
    NamedYaml {
        name: "WorkbenchTodoSourceEnvelope",
        body: include_str!("../openapi/schemas/WorkbenchTodoSourceEnvelope.yaml"),
    },
    NamedYaml {
        name: "WorkbenchTodoSourceOk",
        body: include_str!("../openapi/schemas/WorkbenchTodoSourceOk.yaml"),
    },
    NamedYaml {
        name: "WorkbenchUnavailableSourceEnvelope",
        body: include_str!("../openapi/schemas/WorkbenchUnavailableSourceEnvelope.yaml"),
    },
    NamedYaml {
        name: "WorkbenchUrgency",
        body: include_str!("../openapi/schemas/WorkbenchUrgency.yaml"),
    },
    NamedYaml {
        name: "WorkspaceResponse",
        body: include_str!("../openapi/schemas/WorkspaceResponse.yaml"),
    },
    NamedYaml {
        name: "WorkspaceUpsertRequest",
        body: include_str!("../openapi/schemas/WorkspaceUpsertRequest.yaml"),
    },
];
