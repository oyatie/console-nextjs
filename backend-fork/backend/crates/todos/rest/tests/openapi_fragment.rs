//! The todos face composes, and what it composes is the published contract.
//!
//! Per-path/per-schema comparison rather than one slice compare: `compose`
//! emits keys sorted, while `openapi.yaml` lists this face's paths in router
//! order. Content equality is the invariant under test; key ORDER is deliberate
//! drift for whoever regenerates `openapi.yaml`.

use std::collections::BTreeSet;

use console_contracts::{compose, schema_refs};
use console_todos_rest::{ME_TODOS_PATH, OPENAPI_FRAGMENT};

const OPENAPI_YAML: &str = include_str!("../../../../openapi/openapi.yaml");

/// Lines under `header` that are indented deeper than `indent`, re-joined.
fn block(yaml: &str, header: &str, indent: usize) -> Option<String> {
    let lines: Vec<&str> = yaml.lines().collect();
    let start = lines.iter().position(|line| *line == header)?;
    let mut out = Vec::new();
    for line in &lines[start + 1..] {
        if line.trim().is_empty() {
            out.push(*line);
            continue;
        }
        if line.len() - line.trim_start_matches(' ').len() <= indent {
            break;
        }
        out.push(*line);
    }
    while out.last().is_some_and(|line| line.trim().is_empty()) {
        out.pop();
    }
    Some(out.join("\n"))
}

/// The published body of the schema named `name`, looked up INSIDE
/// `components/schemas`. `components/parameters` and `components/responses`
/// entries sit at the same indentation, so a whole-document lookup answers
/// "this schema exists" for a name that is only a parameter.
fn published_schema(name: &str) -> Option<String> {
    let schemas = block(OPENAPI_YAML, "  schemas:", 2)?;
    block(&schemas, &format!("    {name}:"), 4)
}

/// The top-level path keys `openapi.yaml` publishes under `prefix` — this
/// face's slice, READ OUT OF the published document rather than named by a
/// second list. Without this, every comparison below runs fragment→document
/// only, and a path the face stops serving keeps being published.
fn published_paths_under(prefix: &str) -> BTreeSet<&'static str> {
    let owned = format!("{prefix}/");
    OPENAPI_YAML
        .lines()
        .filter_map(|line| line.strip_prefix("  ")?.strip_suffix(':'))
        .filter(|key| *key == prefix || key.starts_with(&owned))
        .collect()
}

/// The schemas the published document needs in order to serve this face's
/// published paths: the `$ref` closure rooted at those path blocks, walked with
/// `console_contracts::schema_refs` so the test and the composer agree on what
/// a ref is.
fn published_schemas_for_face() -> BTreeSet<String> {
    let mut queue: Vec<String> = published_paths_under(ME_TODOS_PATH)
        .into_iter()
        .filter_map(|path| block(OPENAPI_YAML, &format!("  {path}:"), 2))
        .flat_map(|body| schema_refs(&body).map(str::to_owned).collect::<Vec<_>>())
        .collect();
    let mut seen = BTreeSet::new();
    while let Some(name) = queue.pop() {
        if !seen.insert(name.clone()) {
            continue;
        }
        if let Some(body) = published_schema(&name) {
            queue.extend(schema_refs(&body).map(str::to_owned));
        }
    }
    seen
}

/// `openapi.yaml` publishes EXACTLY the paths this face declares.
///
/// Chained with `fragment_and_router_declare_the_same_paths` this pins
/// router == fragment == published document. Dropping either equality lets a
/// route deleted from the router AND the fragment stay published, so the
/// contract advertises an endpoint that 404s and nothing observes it.
#[test]
fn the_published_contract_publishes_exactly_this_faces_paths() {
    let fragment: BTreeSet<&str> = OPENAPI_FRAGMENT
        .paths
        .iter()
        .map(|item| item.path)
        .collect();
    assert_eq!(
        published_paths_under(ME_TODOS_PATH),
        fragment,
        "left is what backend/openapi/openapi.yaml publishes under {ME_TODOS_PATH}, \
         right is what this face's fragment declares"
    );
}

/// Same direction for schemas: every component the published todos paths reach
/// is either owned by this face or declared external, and this face owns
/// nothing the published document has stopped reaching.
#[test]
fn the_published_contract_needs_exactly_the_schemas_this_face_owns() {
    let external: BTreeSet<String> = OPENAPI_FRAGMENT
        .external_schemas
        .iter()
        .map(|name| (*name).to_owned())
        .collect();
    let needed: BTreeSet<String> = published_schemas_for_face()
        .difference(&external)
        .cloned()
        .collect();
    let owned: BTreeSet<String> = OPENAPI_FRAGMENT
        .schemas
        .iter()
        .map(|schema| schema.name.to_owned())
        .collect();
    assert_eq!(
        needed, owned,
        "left is what backend/openapi/openapi.yaml's todos paths $ref (minus \
         external_schemas), right is what this face's fragment owns"
    );
}

#[test]
fn todos_face_composes() -> Result<(), Box<dyn std::error::Error>> {
    let composed = compose(&[&OPENAPI_FRAGMENT])?;
    assert!(
        composed.starts_with("paths:\n  /api/v1/me/todos:\n    get:\n"),
        "composed document does not open with the todos face:\n{composed}"
    );
    assert!(
        composed.contains("components:\n  schemas:\n    CreateTodoRequest:\n"),
        "composed document is missing the todos schemas:\n{composed}"
    );
    Ok(())
}

#[test]
fn composed_todos_fragment_is_byte_stable() -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(
        compose(&[&OPENAPI_FRAGMENT])?.as_bytes(),
        compose(&[&OPENAPI_FRAGMENT])?.as_bytes(),
        "two composition runs over the todos face must be byte-identical"
    );
    Ok(())
}

#[test]
fn composed_todos_paths_match_the_published_contract() -> Result<(), Box<dyn std::error::Error>> {
    let composed = compose(&[&OPENAPI_FRAGMENT])?;
    for path in OPENAPI_FRAGMENT.paths {
        let published = block(OPENAPI_YAML, &format!("  {}:", path.path), 2)
            .ok_or_else(|| format!("openapi.yaml has no path {}", path.path))?;
        let mine = block(&composed, &format!("  {}:", path.path), 2)
            .ok_or_else(|| format!("composed output has no path {}", path.path))?;
        assert_eq!(
            mine, published,
            "composed {} drifts from backend/openapi/openapi.yaml",
            path.path
        );
    }
    Ok(())
}

#[test]
fn composed_todos_schemas_match_the_published_contract() -> Result<(), Box<dyn std::error::Error>> {
    let composed = compose(&[&OPENAPI_FRAGMENT])?;
    for schema in OPENAPI_FRAGMENT.schemas {
        let published = published_schema(schema.name)
            .ok_or_else(|| format!("openapi.yaml has no schema {}", schema.name))?;
        let mine = block(&composed, &format!("    {}:", schema.name), 4)
            .ok_or_else(|| format!("composed output has no schema {}", schema.name))?;
        assert_eq!(
            mine, published,
            "composed schema {} drifts from backend/openapi/openapi.yaml",
            schema.name
        );
    }
    Ok(())
}

/// `external_schemas` is the one way a face can stop owning a schema it refs,
/// so every name on it must really be a component the document defines.
#[test]
fn every_borrowed_schema_exists_in_the_published_contract() {
    for name in OPENAPI_FRAGMENT.external_schemas {
        assert!(
            published_schema(name).is_some(),
            "external_schemas claims {name} is owned elsewhere, but \
             backend/openapi/openapi.yaml defines no such schema"
        );
    }
}

/// The router and the fragment declare the SAME path set, not one containing
/// the other. The route set comes from `route_paths()`, which reads the same
/// table `router()` is folded from — not from a second list maintained by hand.
///
/// Equality is what makes this a guard in both directions: a subset check
/// passes when a route is DELETED from the router while the fragment and
/// `openapi.yaml` keep publishing it, so the contract advertises an endpoint
/// that 404s and nothing observes the removal.
#[test]
fn fragment_and_router_declare_the_same_paths() {
    let fragment_paths: BTreeSet<&str> = OPENAPI_FRAGMENT.paths.iter().map(|p| p.path).collect();
    let router_paths: BTreeSet<&str> = console_todos_rest::route_paths().into_iter().collect();
    assert_eq!(
        router_paths, fragment_paths,
        "the router's path set and the OpenAPI fragment's path set differ; \
         left is what router() serves, right is what the contract publishes"
    );
}
