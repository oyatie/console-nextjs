//! This face's slice of the published OpenAPI contract.
//!
//! Bodies live as YAML under `openapi/` and are pulled in via `include_str!`.
//! `console_contracts` re-indents them; composition rejects duplicate keys.

use console_contracts::{Fragment, NamedYaml, Operation, PathItem};

/// This face's contribution to the composed OpenAPI document.
pub const OPENAPI_FRAGMENT: Fragment = Fragment {
    source: "console-sales-rest",
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
        path: "/api/v1/sales/inquiries",
        operations: &[Operation {
            method: "get",
            body: include_str!("../openapi/paths/api__v1__sales__inquiries.get.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/sales/inquiries/{id}",
        operations: &[Operation {
            method: "patch",
            body: include_str!("../openapi/paths/api__v1__sales__inquiries__id.patch.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/sales/listings",
        operations: &[
            Operation {
                method: "get",
                body: include_str!("../openapi/paths/api__v1__sales__listings.get.yaml"),
            },
            Operation {
                method: "post",
                body: include_str!("../openapi/paths/api__v1__sales__listings.post.yaml"),
            },
        ],
    },
    PathItem {
        path: "/api/v1/sales/listings/{id}",
        operations: &[
            Operation {
                method: "delete",
                body: include_str!("../openapi/paths/api__v1__sales__listings__id.delete.yaml"),
            },
            Operation {
                method: "patch",
                body: include_str!("../openapi/paths/api__v1__sales__listings__id.patch.yaml"),
            },
        ],
    },
    PathItem {
        path: "/api/v1/storefront/inquiries",
        operations: &[Operation {
            method: "post",
            body: include_str!("../openapi/paths/api__v1__storefront__inquiries.post.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/storefront/listings",
        operations: &[Operation {
            method: "get",
            body: include_str!("../openapi/paths/api__v1__storefront__listings.get.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/storefront/listings/{id}",
        operations: &[Operation {
            method: "get",
            body: include_str!("../openapi/paths/api__v1__storefront__listings__id.get.yaml"),
        }],
    },
    PathItem {
        path: "/api/v1/storefront/listings/{id}/media/{media_id}",
        operations: &[Operation {
            method: "get",
            body: include_str!(
                "../openapi/paths/api__v1__storefront__listings__id__media__media_id.get.yaml"
            ),
        }],
    },
];

const SCHEMAS: &[NamedYaml] = &[
    NamedYaml {
        name: "CreateListingRequest",
        body: include_str!("../openapi/schemas/CreateListingRequest.yaml"),
    },
    NamedYaml {
        name: "CreateListingResponse",
        body: include_str!("../openapi/schemas/CreateListingResponse.yaml"),
    },
    NamedYaml {
        name: "CustomerInquiryPage",
        body: include_str!("../openapi/schemas/CustomerInquiryPage.yaml"),
    },
    NamedYaml {
        name: "CustomerInquiryView",
        body: include_str!("../openapi/schemas/CustomerInquiryView.yaml"),
    },
    NamedYaml {
        name: "InquiryAck",
        body: include_str!("../openapi/schemas/InquiryAck.yaml"),
    },
    NamedYaml {
        name: "InquiryStatus",
        body: include_str!("../openapi/schemas/InquiryStatus.yaml"),
    },
    NamedYaml {
        name: "InquiryTopic",
        body: include_str!("../openapi/schemas/InquiryTopic.yaml"),
    },
    NamedYaml {
        name: "ListingCondition",
        body: include_str!("../openapi/schemas/ListingCondition.yaml"),
    },
    NamedYaml {
        name: "ListingKind",
        body: include_str!("../openapi/schemas/ListingKind.yaml"),
    },
    NamedYaml {
        name: "ListingMediaView",
        body: include_str!("../openapi/schemas/ListingMediaView.yaml"),
    },
    NamedYaml {
        name: "ListingStatus",
        body: include_str!("../openapi/schemas/ListingStatus.yaml"),
    },
    NamedYaml {
        name: "ListingType",
        body: include_str!("../openapi/schemas/ListingType.yaml"),
    },
    NamedYaml {
        name: "SalesListingPage",
        body: include_str!("../openapi/schemas/SalesListingPage.yaml"),
    },
    NamedYaml {
        name: "SalesListingView",
        body: include_str!("../openapi/schemas/SalesListingView.yaml"),
    },
    NamedYaml {
        name: "SubmitInquiryRequest",
        body: include_str!("../openapi/schemas/SubmitInquiryRequest.yaml"),
    },
    NamedYaml {
        name: "UpdateInquiryStatusRequest",
        body: include_str!("../openapi/schemas/UpdateInquiryStatusRequest.yaml"),
    },
    NamedYaml {
        name: "UpdateListingRequest",
        body: include_str!("../openapi/schemas/UpdateListingRequest.yaml"),
    },
];
