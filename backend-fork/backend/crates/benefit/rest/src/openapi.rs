//! This face's slice of the published OpenAPI contract.
//!
//! Bodies live as YAML under `openapi/` and are pulled in via `include_str!`.
//! `console_contracts` re-indents them; composition rejects duplicate keys.

use console_contracts::{Fragment, NamedYaml, Operation, PathItem};

/// This face's contribution to the composed OpenAPI document.
pub const OPENAPI_FRAGMENT: Fragment = Fragment {
    source: "console-benefit-rest",
    paths: PATHS,
    schemas: SCHEMAS,
    parameters: &[],
    responses: &[],
    security_schemes: &[],
    external_schemas: EXTERNAL_SCHEMAS,
};

const EXTERNAL_SCHEMAS: &[&str] = &["Uuid"];

const PATHS: &[PathItem] = &[
    PathItem {
        path: "/api/v1/benefit-catalog/items",
        operations: &[
            Operation {
                method: "get",
                body: include_str!("../openapi/paths/api__v1__benefit-catalog__items.get.yaml"),
            },
            Operation {
                method: "post",
                body: include_str!("../openapi/paths/api__v1__benefit-catalog__items.post.yaml"),
            },
        ],
    },
    PathItem {
        path: "/api/v1/benefit-catalog/items/{benefit_id}",
        operations: &[
            Operation {
                method: "get",
                body: include_str!(
                    "../openapi/paths/api__v1__benefit-catalog__items__benefit_id.get.yaml"
                ),
            },
            Operation {
                method: "patch",
                body: include_str!(
                    "../openapi/paths/api__v1__benefit-catalog__items__benefit_id.patch.yaml"
                ),
            },
        ],
    },
    PathItem {
        path: "/api/v1/benefit-catalog/items/{benefit_id}/conditions",
        operations: &[Operation {
            method: "put",
            body: include_str!(
                "../openapi/paths/api__v1__benefit-catalog__items__benefit_id__conditions.put.yaml"
            ),
        }],
    },
    PathItem {
        path: "/api/v1/benefit-catalog/items/{benefit_id}/tiers",
        operations: &[Operation {
            method: "put",
            body: include_str!(
                "../openapi/paths/api__v1__benefit-catalog__items__benefit_id__tiers.put.yaml"
            ),
        }],
    },
];

const SCHEMAS: &[NamedYaml] = &[
    NamedYaml {
        name: "BenefitCatalogCondition",
        body: include_str!("../openapi/schemas/BenefitCatalogCondition.yaml"),
    },
    NamedYaml {
        name: "BenefitCatalogCreateRequest",
        body: include_str!("../openapi/schemas/BenefitCatalogCreateRequest.yaml"),
    },
    NamedYaml {
        name: "BenefitCatalogItem",
        body: include_str!("../openapi/schemas/BenefitCatalogItem.yaml"),
    },
    NamedYaml {
        name: "BenefitCatalogItemPage",
        body: include_str!("../openapi/schemas/BenefitCatalogItemPage.yaml"),
    },
    NamedYaml {
        name: "BenefitCatalogLifecycleBinding",
        body: include_str!("../openapi/schemas/BenefitCatalogLifecycleBinding.yaml"),
    },
    NamedYaml {
        name: "BenefitCatalogReplaceConditionsRequest",
        body: include_str!("../openapi/schemas/BenefitCatalogReplaceConditionsRequest.yaml"),
    },
    NamedYaml {
        name: "BenefitCatalogReplaceTiersRequest",
        body: include_str!("../openapi/schemas/BenefitCatalogReplaceTiersRequest.yaml"),
    },
    NamedYaml {
        name: "BenefitCatalogScope",
        body: include_str!("../openapi/schemas/BenefitCatalogScope.yaml"),
    },
    NamedYaml {
        name: "BenefitCatalogTier",
        body: include_str!("../openapi/schemas/BenefitCatalogTier.yaml"),
    },
    NamedYaml {
        name: "BenefitCatalogUpdateRequest",
        body: include_str!("../openapi/schemas/BenefitCatalogUpdateRequest.yaml"),
    },
];
