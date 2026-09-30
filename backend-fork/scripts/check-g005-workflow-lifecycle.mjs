#!/usr/bin/env node
import { existsSync, readFileSync } from "node:fs";
import { beginGate, emitProvenanceIfRequested, noteAssertion, noteRead } from "./lib/gate-inputs.mjs";
import { stripRustCommentsAndStringLiterals } from "./check-executed-tests-cfg.mjs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
beginGate({
  gate: "check:g005-workflow-lifecycle",
  script: "scripts/check-g005-workflow-lifecycle.mjs",
  documentInputs: [],
});

const matrixPath = "docs/benchmarks/g005-workflow-lifecycle-matrix.json";
const auditPath = "docs/benchmarks/enterprise-ui-route-audit.json";
const goalId = "G005-workflow-builder-approvals-work-hub";
const failures = [];
const passes = [];

function pathOf(path) {
  return resolve(root, path);
}

function read(path) {
  const abs = pathOf(path);
  noteRead(path);
  if (!existsSync(abs)) {
    failures.push(`${path}: missing`);
    return "";
  }
  return readFileSync(abs, "utf8");
}

function parseJson(path) {
  const text = read(path);
  if (!text) return null;
  try {
    return JSON.parse(text);
  } catch (error) {
    failures.push(`${path}: invalid JSON: ${error.message}`);
    return null;
  }
}

function pass(label) {
  passes.push(label);
}

function assert(condition, ok, failure) {
  if (condition) pass(ok);
  else failures.push(failure);
}

function requireFile(path, label = path) {
  assert(existsSync(pathOf(path)), `${label}: present`, `${label}: missing (${path})`);
}

function requireIncludes(path, needle, label) {
  const text = read(path);
  assert(text.includes(needle), label, `${label}: ${path} must include ${JSON.stringify(needle)}`);
  if (text.includes(needle)) noteAssertion(path);
}

function requireNotIncludes(path, needle, label) {
  const text = read(path);
  assert(!text.includes(needle), label, `${label}: ${path} must not include ${JSON.stringify(needle)}`);
  if (!text.includes(needle)) noteAssertion(path);
}

function requireArrayOfStrings(value, path, label) {
  assert(
    Array.isArray(value) && value.length > 0 && value.every((entry) => typeof entry === "string" && entry.length > 0),
    label,
    `${path}: ${label} must be a non-empty string array`,
  );
}

function compactRustFunction(source, name) {
  const stripped = stripRustCommentsAndStringLiterals(source);
  const signature = new RegExp(`\\b(?:pub\\s+)?(?:async\\s+)?fn\\s+${name}\\s*\\(`).exec(stripped);
  if (!signature) return "";
  const openBrace = stripped.indexOf("{", signature.index + signature[0].length);
  if (openBrace === -1) return "";
  let depth = 1;
  for (let index = openBrace + 1; index < stripped.length; index += 1) {
    if (stripped[index] === "{") depth += 1;
    else if (stripped[index] === "}") depth -= 1;
    if (depth === 0) return stripped.slice(openBrace + 1, index).replace(/\s+/g, "");
  }
  return "";
}

function normalizeRustRawIdentifiers(source) {
  return source.replace(/\br#([A-Za-z_][A-Za-z0-9_]*)\b/g, "$1");
}

function hasWorkflowApprovalLifecycle(source) {
  const body = compactRustFunction(source, "approve_next");
  const normalizedBody = normalizeRustRawIdentifiers(body);
  const roleSelection = "letrole=self.approval_line.next_pending_non_mechanic_role()";
  const approvalClone = "letmutnext_line=self.approval_line.clone();";
  const approval = "next_line.approve(role,actor_id,at)?;";
  const context = "letcontext=TransitionGuardContext{actor:TransitionActor::Admin,approval_line_complete:next_line.is_complete(),completion_evidence_verified,};";
  const guardedTransition = "lettransition=self.apply_transition(to,at,context)?;";
  const approvalCommit = "self.approval_line=next_line;";
  const requiredPredicates = [
    roleSelection,
    approvalClone,
    approval,
    "ApprovalRole::Admin=>WorkOrderStatus::AdminReview",
    "ApprovalRole::Executiveifself.result_type==WorkResultType::Completed=>{WorkOrderStatus::FinalCompleted}",
    "letcompletion_evidence_verified=ifto==WorkOrderStatus::FinalCompleted{evidence.final_completion_evidence_verified(self.id)?}else{true};",
    "approval_line_complete:next_line.is_complete(),completion_evidence_verified,",
    guardedTransition,
    approvalCommit,
  ];
  const orderedPredicates = [
    roleSelection,
    approvalClone,
    approval,
    context,
    guardedTransition,
    approvalCommit,
  ];
  const hasPresenceAffectingCfg = /#!?\[cfg(?:_attr)?\(/.test(normalizedBody);
  const criticalAnchorsAreUnique = orderedPredicates.every(
    (predicate) => body.split(predicate).length === 2,
  );
  let cursor = 0;
  const lifecycleIsOrdered = orderedPredicates.every((predicate) => {
    const index = body.indexOf(predicate, cursor);
    if (index === -1) return false;
    cursor = index + predicate.length;
    return true;
  });
  return (
    !hasPresenceAffectingCfg &&
    requiredPredicates.every((predicate) => body.includes(predicate)) &&
    criticalAnchorsAreUnique &&
    lifecycleIsOrdered
  );
}

function hasServerOwnedScopedApprovalFeed(source) {
  const router = compactRustFunction(source, "router");
  const endpoint = compactRustFunction(source, "list_approval_items");
  const visibility = compactRustFunction(source, "approval_source_visibility");
  const counts = compactRustFunction(source, "fetch_approval_source_counts");
  const rows = compactRustFunction(source, "fetch_approval_rows");
  const union = compactRustFunction(source, "push_approval_federation_union");
  return [
    router.includes(".route(APPROVAL_ITEMS_PATH,get(list_approval_items))"),
    endpoint.includes("letprincipal=principal_from_headers(&state,&headers).await?;"),
    endpoint.includes("letvisibility=approval_source_visibility(&principal)?;"),
    endpoint.includes("letbranch_scope=work_order_list_scope(&principal);"),
    endpoint.includes("fetch_approval_source_counts(pool,&branch_scope,visibility,principal.user_id).await?;"),
    endpoint.includes("fetch_approval_rows(pool,&branch_scope,visibility,principal.user_id,&query).await?;"),
    visibility.includes("work_orders:org_wide||feature_allowed_in_scope(principal,Feature::CompletionReview)"),
    visibility.includes("daily_plans:org_wide||feature_allowed_in_scope(principal,Feature::DailyPlanReview)"),
    visibility.includes("target_changes:org_wide||feature_allowed_in_scope(principal,Feature::TargetManage)"),
    counts.includes("letorg=current_org().map_err(KernelError::from).map_err(RestError::from_kernel)?;"),
    counts.includes("push_approval_federation_union(&mutbuilder,branch_scope,visibility,actor)?;"),
    counts.includes("with_org_conn::<_,_,RestError>(pool,org,"),
    rows.includes("letorg=current_org().map_err(KernelError::from).map_err(RestError::from_kernel)?;"),
    rows.includes("push_approval_federation_union(&mutbuilder,branch_scope,visibility,actor)?;"),
    rows.includes("with_org_conn::<_,_,RestError>(pool,org,"),
    (union.match(/push_branch_scope_filter\(builder,branch_scope,/g) ?? []).length === 3,
  ].every(Boolean);
}

const matrix = parseJson(matrixPath);
const routeAudit = parseJson(auditPath);
const packageJson = parseJson("package.json") ?? {};
const ci = read(".github/workflows/ci.yml");

assert(
  packageJson.scripts?.["check:g005-workflow-lifecycle"] === "node scripts/check-g005-workflow-lifecycle.mjs",
  "package script check:g005-workflow-lifecycle",
  "package.json must define check:g005-workflow-lifecycle",
);
assert(
  ci.includes("npm run check:g005-workflow-lifecycle"),
  "CI runs G005 workflow lifecycle gate",
  ".github/workflows/ci.yml must run npm run check:g005-workflow-lifecycle",
);
requireFile(matrixPath, "G005 workflow lifecycle matrix");
requireFile(auditPath, "enterprise UI route audit register");

if (matrix) {
  assert(matrix.schemaVersion === 1, "G005 matrix schema version 1", `${matrixPath}: schemaVersion must be 1`);
  assert(matrix.goalId === goalId, "G005 matrix goal id", `${matrixPath}: goalId must be ${goalId}`);
  assert(
    typeof matrix.nonClaimPolicy === "string" && matrix.nonClaimPolicy.includes("G009"),
    "G005 matrix records live-evidence non-claim policy",
    `${matrixPath}: nonClaimPolicy must reserve live rollout/screenshot claims for G009`,
  );
  assert(Array.isArray(matrix.routePaths) && matrix.routePaths.length >= 6, "G005 matrix routePaths", `${matrixPath}: routePaths must cover workflow routes`);
  assert(Array.isArray(matrix.dependencyRoutes) && matrix.dependencyRoutes.length >= 3, "G005 matrix dependency routePaths", `${matrixPath}: dependencyRoutes must cover downstream route dependencies`);
  requireArrayOfStrings(matrix.requiredBackendTests, matrixPath, "requiredBackendTests");
  assert(Array.isArray(matrix.backendContracts) && matrix.backendContracts.length >= 9, "G005 backend contract inventory", `${matrixPath}: backendContracts must include workflow/approval/evidence contracts`);
  assert(Array.isArray(matrix.safetyAssertions) && matrix.safetyAssertions.length >= 10, "G005 safety assertions", `${matrixPath}: safetyAssertions must capture workflow, approval, evidence, badge, and scope guardrails`);

  const requiredRouteGroups = new Set(["overview", "approvals", "workflow-builder", "work-order-detail", "intake", "planned-work"]);
  const matrixRoutes = new Map();
  for (const row of matrix.routePaths ?? []) {
    assert(typeof row.path === "string" && row.path.startsWith("/"), `G005 route ${row.path ?? "<missing>"}: path`, `${matrixPath}: route row missing path`);
    assert(row.mustContainOwnerGoal === "G005", `G005 route ${row.path}: owner marker`, `${matrixPath}: route ${row.path} mustContainOwnerGoal must be G005`);
    assert(typeof row.requiredStory === "string" && row.requiredStory.length >= 48, `G005 route ${row.path}: required story`, `${matrixPath}: route ${row.path} requiredStory is too weak`);
    matrixRoutes.set(row.path, row);
  }
  for (const group of requiredRouteGroups) {
    assert([...matrixRoutes.values()].some((row) => row.routeGroup === group), `G005 route group ${group}: covered`, `${matrixPath}: no route covers group ${group}`);
  }

  if (routeAudit?.routeCoverage) {
    const auditByCanonicalPath = new Map(routeAudit.routeCoverage.map((row) => [row.canonicalPath, row]));
    const auditByRawPath = new Map(routeAudit.routeCoverage.map((row) => [row.rawPath ?? row.canonicalPath, row]));
    const auditForMatrixPath = (path) => auditByRawPath.get(path) ?? auditByCanonicalPath.get(path);
    const ownedG005Rows = routeAudit.routeCoverage.filter((row) => String(row.ownerLane ?? "").startsWith("G005"));
    assert(ownedG005Rows.length >= 6, "enterprise route audit has G005-owned rows", `${auditPath}: expected G005-owned route rows`);
    for (const auditRow of ownedG005Rows) {
      assert(matrixRoutes.has(auditRow.canonicalPath), `route audit ${auditRow.canonicalPath}: represented in G005 matrix`, `${matrixPath}: missing routePaths row for G005-owned route ${auditRow.canonicalPath}`);
    }
    const weakNeedles = ["fallback", "unclassified", "demo", "placeholder", "coming soon", "black background", "text wall"];
    for (const [path, matrixRow] of matrixRoutes) {
      const auditRow = auditForMatrixPath(path);
      assert(Boolean(auditRow), `G005 matrix route ${path}: exists in route audit`, `${auditPath}: missing routeCoverage for ${path}`);
      if (!auditRow) continue;
      assert(String(auditRow.ownerLane ?? "").includes("G005"), `G005 matrix route ${path}: ownerLane contains G005`, `${auditPath}: ${path} ownerLane must include G005`);
      assert(String(auditRow.e2eSpec ?? "").includes("Required browser"), `G005 matrix route ${path}: browser story required`, `${auditPath}: ${path} e2eSpec must require browser story`);
      assert(String(auditRow.e2eSpec ?? "").includes("comment") || String(auditRow.e2eSpec ?? "").includes("evidence"), `G005 matrix route ${path}: comment/evidence story`, `${auditPath}: ${path} e2eSpec must include comment or evidence lifecycle`);
      for (const field of ["sourceObject", "lifecycleStates", "denialScopeTest", "groupScopeStory"]) {
        assert(typeof auditRow[field] === "string" && auditRow[field].length >= 24, `G005 matrix route ${path}: ${field}`, `${auditPath}: ${path} missing ${field}`);
      }
      const combined = `${auditRow.ownerLane} ${auditRow.e2eSpec} ${auditRow.denialScopeTest} ${auditRow.groupScopeStory} ${matrixRow.requiredStory}`.toLowerCase();
      for (const needle of weakNeedles) {
        assert(!combined.includes(needle), `G005 matrix route ${path}: no weak ${needle} marker`, `${auditPath}: ${path} still contains weak marker ${needle}`);
      }
      assert(typeof auditRow.screenshotTraceEvidence === "string" && auditRow.screenshotTraceEvidence.includes("Pending"), `G005 matrix route ${path}: screenshot/trace is explicitly non-closed`, `${auditPath}: ${path} screenshotTraceEvidence must remain explicit until G009 live closure evidence lands`);
    }

    for (const depRoute of matrix.dependencyRoutes ?? []) {
      const auditRow = auditForMatrixPath(depRoute.path);
      assert(Boolean(auditRow), `G005 dependency route ${depRoute.path}: exists in route audit`, `${auditPath}: missing dependency routeCoverage for ${depRoute.path}`);
      if (!auditRow) continue;
      const combined = `${auditRow.ownerLane} ${auditRow.sourceObject} ${auditRow.e2eSpec} ${auditRow.groupScopeStory}`;
      assert(combined.includes(depRoute.expectedDependency), `G005 dependency route ${depRoute.path}: ${depRoute.expectedDependency}`, `${auditPath}: ${depRoute.path} must mention dependency ${depRoute.expectedDependency}`);
      assert(typeof depRoute.requiredStory === "string" && depRoute.requiredStory.length >= 48, `G005 dependency route ${depRoute.path}: required story`, `${matrixPath}: dependency route ${depRoute.path} requiredStory is too weak`);
    }
  }

  for (const contract of matrix.backendContracts ?? []) {
    requireFile(contract.file, `backend contract ${contract.file}`);
    assert(typeof contract.contract === "string" && contract.contract.length >= 40, `backend contract ${contract.file}: rationale`, `${matrixPath}: ${contract.file} contract rationale is too weak`);
    requireArrayOfStrings(contract.requiredSnippets, matrixPath, `backend contract ${contract.file} required snippets`);
    for (const snippet of contract.requiredSnippets ?? []) {
      requireIncludes(contract.file, snippet, `backend contract ${contract.file}: ${snippet}`);
    }
  }


  for (const test of matrix.requiredBackendTests ?? []) requireFile(test, `G005 backend test ${test}`);

  assert(
    hasWorkflowApprovalLifecycle(read("backend/crates/workorder/domain/src/lib.rs")),
    "G005 executable workflow/approval lifecycle",
    "G005 executable workflow/approval lifecycle must preserve ordered approval and guarded transition application",
  );
  assert(
    hasServerOwnedScopedApprovalFeed(read("backend/crates/workorder/rest/src/lib.rs")),
    "G005 server-owned scoped approval feed",
    "G005 server-owned approval feed must derive visibility from the principal and preserve branch-scoped queries",
  );

}

if (failures.length) {
  console.error(`G005 workflow lifecycle gate failed:\n${failures.map((failure) => `- ${failure}`).join("\n")}`);
  process.exit(1);
}

emitProvenanceIfRequested();
console.log(`G005 workflow lifecycle gate passed (${passes.length} checks).`);
for (const item of passes) console.log(`- ${item}`);
