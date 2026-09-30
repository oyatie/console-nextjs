//! OWNED — a lane may not edit this file.
//!
//! Driver #1: the generic ontology REST surface, over the REAL axum router.
//! `router()` self-applies `with_request_context`, so every request here also
//! proves the router resolves a principal from the signed token, arms
//! `CURRENT_ORG` (and therefore the `app.current_org` GUC) and runs the org-wide
//! authority gate BEFORE any handler. A store-only suite would pass against a
//! route that forgot to arm the tenant.
//!
//! ZERO new routes: the surface is already type-agnostic. An Instance-backed type
//! needs none.

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use console_ontology_adapter_postgres::instances::{
    InstanceState, RevisionSummary, TraversalGraph,
};
use console_ontology_domain::{InstanceId, ObjectTypeId};
use console_ontology_rest::router;
use serde_json::{Value, json};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use tower::ServiceExt;
use uuid::Uuid;

use super::harness::Harness;
use super::{ACTION_KEY, Actor, Command, Driver, Failure};

pub struct RestDriver {
    service: axum::Router,
    admin_token: String,
    executive_token: String,
}

impl RestDriver {
    pub fn new(h: &Harness) -> Self {
        Self {
            service: router(h.state(Some(h.verifier()))),
            admin_token: h.admin_token.clone(),
            executive_token: h.executive_token.clone(),
        }
    }

    fn token(&self, actor: Actor) -> &str {
        match actor {
            Actor::Privileged => &self.admin_token,
            Actor::Unprivileged => &self.executive_token,
        }
    }

    /// Raw bytes, no JSON assumption — CTL-4 needs to see an EMPTY body.
    pub async fn raw_get(&self, uri: &str, token: &str) -> (StatusCode, Vec<u8>) {
        let request = Request::builder()
            .method("GET")
            .uri(uri)
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .body(Body::empty())
            .expect("build request");
        let response = self
            .service
            .clone()
            .oneshot(request)
            .await
            .expect("router response");
        let status = response.status();
        let bytes = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("read body");
        (status, bytes.to_vec())
    }

    async fn json(
        &self,
        method: &str,
        uri: &str,
        token: &str,
        body: Value,
    ) -> Result<Value, Failure> {
        let mut builder = Request::builder()
            .method(method)
            .uri(uri)
            .header(header::AUTHORIZATION, format!("Bearer {token}"));
        let payload = if body == Value::Null {
            Body::empty()
        } else {
            builder = builder.header(header::CONTENT_TYPE, "application/json");
            Body::from(serde_json::to_vec(&body).expect("serialize request body"))
        };
        let response = self
            .service
            .clone()
            .oneshot(builder.body(payload).expect("build request"))
            .await
            .expect("router response");
        let status = response.status();
        let bytes = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("read body");
        if status.is_success() {
            return Ok(serde_json::from_slice(&bytes).expect("success bodies are JSON"));
        }
        Err(failure_from(status, &bytes))
    }

    /// Resolution is harness plumbing, always done as the privileged actor, so a
    /// denial assertion pins the ACTION endpoint rather than the lookup.
    async fn type_id(&self, key: &str) -> Result<ObjectTypeId, Failure> {
        let body = self
            .json(
                "GET",
                &format!("/api/v1/ontology/object-types/{key}"),
                &self.admin_token,
                Value::Null,
            )
            .await?;
        Ok(ObjectTypeId::from_uuid(
            body["object_type"]["id"]
                .as_str()
                .and_then(|s| s.parse::<Uuid>().ok())
                .expect("object_type.id must be a uuid"),
        ))
    }
}

/// The handler error envelope (`rest/src/lib.rs:1867-1878`). A non-JSON body is a
/// layer rejection (401s are plain text, `request-context/src/lib.rs:558-578`) and
/// must never be silently coerced into a domain code.
fn failure_from(status: StatusCode, bytes: &[u8]) -> Failure {
    match serde_json::from_slice::<Value>(bytes) {
        Ok(body) if body["error"]["code"].is_string() => Failure {
            code: body["error"]["code"]
                .as_str()
                .unwrap_or_default()
                .to_owned(),
            message: body["error"]["message"]
                .as_str()
                .unwrap_or_default()
                .to_owned(),
        },
        _ => Failure {
            code: format!("http_{}", status.as_u16()),
            message: String::from_utf8_lossy(bytes).into_owned(),
        },
    }
}

/// `+00:00` in a query string decodes `+` as a space and the rfc3339 parse fails.
fn as_of_param(at: OffsetDateTime) -> String {
    at.format(&Rfc3339)
        .expect("format rfc3339")
        .replace("+00:00", "Z")
}

impl Driver for RestDriver {
    const NAME: &'static str = "rest";
    /// `authorize_ontology` runs org-wide `Feature::RoleManage` PRE-handler
    /// (`rest/src/lib.rs:1649-1657`) → 403 `forbidden`, zero DB contact.
    const DENIAL_CODE: &'static str = "forbidden";

    async fn resolve_type(&self, key: &str) -> Result<ObjectTypeId, Failure> {
        self.type_id(key).await
    }

    async fn execute(&self, cmd: &Command, actor: Actor) -> Result<InstanceState, Failure> {
        let type_id = self.type_id(cmd.type_key).await?;
        let mut body = json!({
            "object_type_id": type_id.as_uuid().to_string(),
            "params": cmd.params,
            "command_id": cmd.command_id.to_string(),
        });
        if let Some(title) = &cmd.title {
            body["title"] = json!(title);
        }
        if let Some((instance, expected)) = cmd.instance {
            body["instance_id"] = json!(instance.to_string());
            body["expected_revision"] = json!(expected);
        }
        if let Some(valid_from) = cmd.valid_from {
            body["valid_from"] = json!(as_of_param(valid_from));
        }
        let outcome = self
            .json(
                "POST",
                &format!("/api/v1/ontology/actions/{ACTION_KEY}/execute"),
                self.token(actor),
                body,
            )
            .await?;
        Ok(serde_json::from_value(outcome["instance"].clone())
            .expect("an instance_revision execute carries an instance"))
    }

    async fn read(
        &self,
        id: InstanceId,
        as_of: Option<OffsetDateTime>,
    ) -> Result<InstanceState, Failure> {
        // Never the list endpoint: `list_instances` fail-closes to `[]` without an
        // ENFORCED Cedar permit (`authz/src/cedar_pbac/residual.rs:201-203`).
        let uri = match as_of {
            Some(at) => format!("/api/v1/ontology/instances/{id}?as_of={}", as_of_param(at)),
            None => format!("/api/v1/ontology/instances/{id}"),
        };
        let body = self
            .json("GET", &uri, &self.admin_token, Value::Null)
            .await?;
        Ok(serde_json::from_value(body).expect("instance read model"))
    }

    async fn history(&self, id: InstanceId) -> Result<Vec<RevisionSummary>, Failure> {
        let body = self
            .json(
                "GET",
                &format!("/api/v1/ontology/instances/{id}/history"),
                &self.admin_token,
                Value::Null,
            )
            .await?;
        Ok(serde_json::from_value(body).expect("revision history"))
    }

    async fn traverse(&self, root: InstanceId, depth: u32) -> Result<TraversalGraph, Failure> {
        // No `link_type` filter: the target asserts that an edge EXISTS and where
        // it lands, never what a lane named its link type.
        let body = self
            .json(
                "GET",
                &format!("/api/v1/ontology/instances/{root}/traverse?depth={depth}"),
                &self.admin_token,
                Value::Null,
            )
            .await?;
        Ok(serde_json::from_value(body).expect("traversal graph"))
    }
}
