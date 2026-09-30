import { existsSync, readFileSync } from 'node:fs';
import path from 'node:path';

export const CONSOLE_NAV_SOURCE = 'web/src/console/shell/nav.ts';
export const CONSOLE_REGISTRY_SOURCE = 'web/src/console/screens/registry.ts';

// The single legitimate "no facts" value. `route_source_present: false` is the
// only thing that lets a consumer distinguish "the console has no frontend, so
// it presents no routes" from "extraction produced nothing". Consumers must
// treat a missing/false flag as absent source and refuse to corroborate any
// route claim against it.
export const ABSENT_CONSOLE_ROUTE_FACTS = Object.freeze({ route_source_present: false, facts: Object.freeze({}) });

function literals(text, declaration) {
  const match = text.match(new RegExp(`(?:export\\s+)?const\\s+${declaration}[\\s\\S]*?=\\s*\\[([\\s\\S]*?)\\](?:\\s+as const)?`));
  if (!match) throw new Error(`missing ${declaration}`);
  return [...match[1].matchAll(/"([A-Za-z][A-Za-z0-9]*)"/g)].map((entry) => entry[1]);
}
export function extractConsoleRouteFactsFromTexts(navText, registryText) {
  const mounted = literals(navText, 'MOUNTED_SCREEN_KEYS');
  const exposed = literals(navText, 'EXPOSED_SCREEN_KEYS');
  const nav = [...navText.matchAll(/screen:\s*"([A-Za-z][A-Za-z0-9]*)"/g)].map((entry) => entry[1]);
  const bodyBlock = registryText.match(/SCREEN_REGISTRY[^=]*=\s*\{([\s\S]*?)\n\};/);
  if (!bodyBlock) throw new Error('missing SCREEN_REGISTRY');
  const bodies = [...bodyBlock[1].matchAll(/^\s*([A-Za-z][A-Za-z0-9]*):/gm)].map((entry) => entry[1]);
  const all = new Set([...mounted, ...exposed, ...nav, ...bodies]);
  const facts = Object.fromEntries([...all].sort().map((key) => [key, { source_mounted: mounted.includes(key), production_exposed: exposed.includes(key), registry_body_present: bodies.includes(key), nav_declared: nav.includes(key) }]));
  return { route_source_present: true, facts, mounted, exposed, nav, bodies };
}

/**
 * Absence of BOTH route sources is the only tolerated failure: the 2026-07-28
 * clean-slate pivot deleted the frontend, and a console with no frontend
 * presents no routes. Every other failure — a renamed constant, a malformed
 * SCREEN_REGISTRY, an unreadable file, a half-landed frontend — propagates.
 * Swallowing those would report "no routes" for a console that has them.
 */
export function extractConsoleRouteFacts(repoRoot) {
  const navPath = path.join(repoRoot, CONSOLE_NAV_SOURCE);
  const registryPath = path.join(repoRoot, CONSOLE_REGISTRY_SOURCE);
  const navPresent = existsSync(navPath);
  const registryPresent = existsSync(registryPath);
  if (navPresent !== registryPresent) throw new Error(`console route source is partially present: ${navPresent ? CONSOLE_REGISTRY_SOURCE : CONSOLE_NAV_SOURCE} is missing`);
  if (!navPresent) return ABSENT_CONSOLE_ROUTE_FACTS;
  return extractConsoleRouteFactsFromTexts(readFileSync(navPath, 'utf8'), readFileSync(registryPath, 'utf8'));
}

// ---------------------------------------------------------------------------
// ADR-0041: the Rust/Leptos side of the inventory.
//
// The two tombstone paths above pin the deleted React stack and keep
// absence-as-green until a mounted shell lands. ADR-0041 allows
// `console-<domain>-ui` members and lockfile Leptos; a machine-readable route
// facts file is a frontend-lane follow-up. Until that lands, inventory may
// report zero Leptos packages and must not fail HEAD for that absence.
//
// What still fails: a non-ui workspace member declaring a Leptos-family
// dependency. HEAD classification uses git-tracked manifests so docs-only
// preflight does not require cargo. True package names come from member
// `package =` and from `[workspace.dependencies]` when `workspace = true`.
// Dotted tables such as `[dependencies.leptos]` are the same classifier.
// Unreadable inventory (missing lockfile, zero members, zero locked packages)
// throws — the gate fails closed, never reports a clean state it did not observe.
//
// The import lives down here so the tombstone constants above keep their
// lines: ADR-0030 cites route-inventory.mjs:4-5 verbatim. ESM hoists it.
import { execFileSync } from 'node:child_process';

export const BACKEND_WORKSPACE_MANIFEST = 'backend/Cargo.toml';
export const BACKEND_WORKSPACE_LOCKFILE = 'backend/Cargo.lock';

// Matches the framework's package namespace (leptos, leptos_axum,
// leptos_meta, leptos-use, ...) on a separator boundary, so `leptose`-style
// strangers do not match. Lockfile leptos is allowed (ADR-0041); a non-ui
// member must still declare the true package name to be flagged.
export const LEPTOS_PACKAGE_FAMILY = /^leptos(?:[_-]|$)/;

// ADR-0030 §6: the chartered per-domain surface crate is console-<domain>-ui.
export const CONSOLE_UI_MEMBER_NAME = /(?:^|-)ui$/;

const ADR = 'ADR-0041 non-ui Leptos violation';
const DEP_TABLE = /^(dependencies|dev-dependencies|build-dependencies)$/;
const DEP_DOTTED = /^(dependencies|dev-dependencies|build-dependencies)\.([A-Za-z0-9_-]+)$/;
const WORKSPACE_DEP_TABLE = 'workspace.dependencies';
const WORKSPACE_DEP_DOTTED = /^workspace\.dependencies\.([A-Za-z0-9_-]+)$/;

function tomlTables(text) {
  const headers = [...text.matchAll(/^\[([^\]]+)\][ \t]*$/gm)];
  return headers.map((match, index) => ({
    name: match[1],
    body: text.slice(match.index + match[0].length, headers[index + 1]?.index),
  }));
}

function depRecord(key, packageName) {
  return packageName === key ? { name: packageName } : { name: packageName, rename: key };
}

function truePackageName(key, inlineBody, workspacePackageByKey, context) {
  const pkg = inlineBody.match(/package\s*=\s*"([^"]+)"/)?.[1];
  if (pkg) return pkg;
  if (/\bworkspace\s*=/.test(inlineBody)) {
    const workspaceName = workspacePackageByKey[key];
    if (typeof workspaceName !== 'string' || workspaceName === '') {
      throw new Error(`${context} declares '${key}' via workspace = true but [workspace.dependencies] does not name a package for it; refusing to classify what it cannot read`);
    }
    return workspaceName;
  }
  return key;
}

function parseInlineDependencyLines(body, workspacePackageByKey, context) {
  const deps = [];
  const lines = body.split('\n');
  for (let index = 0; index < lines.length; index += 1) {
    const trimmed = lines[index].split('#')[0].trim();
    if (!trimmed) continue;
    const tableStart = trimmed.match(/^([A-Za-z0-9_-]+)\s*=\s*\{(.*)$/);
    if (tableStart) {
      let inner = tableStart[2];
      while (!inner.includes('}') && index + 1 < lines.length) {
        index += 1;
        inner += ` ${lines[index].split('#')[0].trim()}`;
      }
      if (!inner.includes('}')) {
        throw new Error(`${context} has an unclosed inline table for '${tableStart[1]}'; refusing to classify what it cannot read`);
      }
      deps.push(depRecord(tableStart[1], truePackageName(tableStart[1], inner, workspacePackageByKey, context)));
      continue;
    }
    const workspaceDot = trimmed.match(/^([A-Za-z0-9_-]+)\.workspace\s*=/);
    if (workspaceDot) {
      deps.push(depRecord(workspaceDot[1], truePackageName(workspaceDot[1], 'workspace = true', workspacePackageByKey, context)));
      continue;
    }
    const simple = trimmed.match(/^([A-Za-z0-9_-]+)\s*=/);
    if (simple) deps.push(depRecord(simple[1], truePackageName(simple[1], '', workspacePackageByKey, context)));
  }
  return deps;
}

function asWorkspacePackageByKey(workspaceDependencies) {
  if (workspaceDependencies == null) return Object.create(null);
  if (typeof workspaceDependencies === 'string') return parseWorkspaceDependencies(workspaceDependencies);
  if (typeof workspaceDependencies !== 'object' || Array.isArray(workspaceDependencies)) {
    throw new Error('workspace.dependencies map is unreadable; refusing to classify what it cannot read');
  }
  return workspaceDependencies;
}

/**
 * Cargo-free `[workspace.dependencies]` map: local key → true package name.
 * `foo = { package = "leptos" }` stores leptos under foo so members that
 * write `foo.workspace = true` cannot hide the family from HEAD classification.
 */
export function parseWorkspaceDependencies(text) {
  if (typeof text !== 'string') {
    throw new Error('workspace manifest is unreadable; refusing to classify what it cannot read');
  }
  const map = Object.create(null);
  const context = BACKEND_WORKSPACE_MANIFEST;
  for (const table of tomlTables(text)) {
    if (table.name === WORKSPACE_DEP_TABLE) {
      for (const dep of parseInlineDependencyLines(table.body, {}, context)) {
        map[dep.rename ?? dep.name] = dep.name;
      }
      continue;
    }
    const dotted = table.name.match(WORKSPACE_DEP_DOTTED);
    if (dotted) map[dotted[1]] = truePackageName(dotted[1], table.body, {}, context);
  }
  return map;
}

function parseMemberDependencies(text, workspacePackageByKey, manifestPath) {
  const context = `workspace member manifest ${manifestPath || ''}`.trim();
  const deps = [];
  for (const table of tomlTables(text)) {
    if (DEP_TABLE.test(table.name)) {
      deps.push(...parseInlineDependencyLines(table.body, workspacePackageByKey, context));
      continue;
    }
    const dotted = table.name.match(DEP_DOTTED);
    if (dotted) deps.push(depRecord(dotted[2], truePackageName(dotted[2], table.body, workspacePackageByKey, context)));
  }
  return deps;
}

/**
 * Cargo-free member parse for docs-only preflight (no rustup).
 * Resolves `package = "leptos"` and `foo.workspace = true` via the workspace
 * dependency map so a rename cannot hide Leptos from the HEAD classifier.
 */
export function parseWorkspaceMemberManifest(text, manifestPath, workspaceDependencies) {
  if (typeof text !== 'string' || !/^\[package\]/m.test(text)) {
    throw new Error(`workspace member manifest ${manifestPath || ''} is unreadable; refusing to classify what it cannot read`);
  }
  const name = text.match(/^\[package\][\s\S]*?^name\s*=\s*"([^"]+)"/m)?.[1];
  if (!name) {
    throw new Error(`workspace member manifest ${manifestPath || ''} carries no package name; refusing to classify what it cannot read`);
  }
  return {
    name,
    manifest_path: manifestPath || '',
    dependencies: parseMemberDependencies(text, asWorkspacePackageByKey(workspaceDependencies), manifestPath),
  };
}

/** Git-tracked backend Cargo.toml members. Does not invoke cargo. */
export function trackedWorkspaceMembers(repoRoot) {
  const workspacePath = path.join(repoRoot, BACKEND_WORKSPACE_MANIFEST);
  if (!existsSync(workspacePath)) {
    throw new Error(`${BACKEND_WORKSPACE_MANIFEST} is missing under ${repoRoot}; the workspace inventory cannot see [workspace.dependencies] and unreadable must fail`);
  }
  let listing;
  try {
    listing = execFileSync('git', ['-C', repoRoot, 'ls-files', '-z', '--', 'backend'], { encoding: 'utf8' });
  } catch (error) {
    throw new Error(`git ls-files failed; the workspace inventory cannot enumerate members and unreadable must fail: ${error.message}`);
  }
  const tracked = listing.split('\0').filter(Boolean);
  if (!tracked.includes(BACKEND_WORKSPACE_MANIFEST)) {
    throw new Error(`${BACKEND_WORKSPACE_MANIFEST} is not git-tracked; the workspace inventory cannot see [workspace.dependencies] and unreadable must fail`);
  }
  const workspacePackageByKey = parseWorkspaceDependencies(readFileSync(workspacePath, 'utf8'));
  const manifests = tracked.filter((entry) => entry.endsWith('Cargo.toml') && entry !== BACKEND_WORKSPACE_MANIFEST);
  const packages = [];
  for (const relativePath of manifests) {
    const abs = path.join(repoRoot, relativePath);
    const text = readFileSync(abs, 'utf8');
    if (!/^\[package\]/m.test(text)) continue;
    packages.push(parseWorkspaceMemberManifest(text, abs, workspacePackageByKey));
  }
  if (packages.length === 0) {
    throw new Error('workspace scan examined zero workspace members; refusing to report a clean state it did not observe');
  }
  return packages;
}

/**
 * HEAD inventory for docs-only preflight: git + lockfile text, no cargo.
 */
export function extractConsoleHeadWorkspaceFacts(repoRoot) {
  const lockfilePath = path.join(repoRoot, BACKEND_WORKSPACE_LOCKFILE);
  if (!existsSync(lockfilePath)) throw new Error(`${BACKEND_WORKSPACE_LOCKFILE} is missing under ${repoRoot}; the workspace inventory cannot see the locked graph and unreadable must fail`);
  const packages = trackedWorkspaceMembers(repoRoot);
  const memberViolations = workspaceMemberViolations(packages, { repoRoot });
  const lockedNames = lockedPackageNames(readFileSync(lockfilePath, 'utf8'));
  return {
    workspace_scanned: true,
    member_count: packages.length,
    locked_package_count: lockedNames.length,
    ui_member_count: packages.filter((pkg) => CONSOLE_UI_MEMBER_NAME.test(pkg.name)).length,
    leptos_locked_package_count: lockedNames.filter((name) => LEPTOS_PACKAGE_FAMILY.test(name)).length,
    violations: memberViolations,
  };
}

/**
 * Classifier over `cargo metadata` packages. Ui members may declare Leptos.
 * A non-ui member that declares a Leptos-family dependency is a violation.
 * Throws instead of returning clean when handed nothing or anything it cannot
 * read: a scan that examined zero members has observed nothing and must not pass.
 */
export function workspaceMemberViolations(packages, { repoRoot } = {}) {
  if (!Array.isArray(packages) || packages.length === 0) {
    throw new Error('workspace scan examined zero workspace members; refusing to report a clean state it did not observe');
  }
  const violations = [];
  for (const pkg of packages) {
    if (typeof pkg?.name !== 'string' || pkg.name === '' || typeof pkg?.manifest_path !== 'string' || !Array.isArray(pkg?.dependencies)) {
      throw new Error('workspace scan met a member missing name/manifest_path/dependencies; refusing to classify what it cannot read');
    }
    const where = repoRoot ? path.relative(repoRoot, pkg.manifest_path) : pkg.manifest_path;
    const uiMember = CONSOLE_UI_MEMBER_NAME.test(pkg.name);
    for (const dep of pkg.dependencies) {
      if (typeof dep?.name !== 'string' || dep.name === '') {
        throw new Error(`workspace scan met an unreadable dependency of '${pkg.name}'; refusing to classify what it cannot read`);
      }
      if (!uiMember && LEPTOS_PACKAGE_FAMILY.test(dep.name)) {
        const alias = typeof dep.rename === 'string' && dep.rename ? ` (renamed locally to '${dep.rename}')` : '';
        violations.push(`${ADR}: workspace member '${pkg.name}' (${where}) declares Leptos-family dependency '${dep.name}'${alias}; Leptos is legal only on Layer::Ui members (package name ending in -ui)`);
      }
    }
  }
  return violations;
}

/**
 * Pure lockfile reader: every `[[package]]` block must yield a name, and a
 * lockfile with no blocks is unreadable, not clean.
 */
export function lockedPackageNames(lockfileText) {
  if (typeof lockfileText !== 'string' || !lockfileText.includes('[[package]]')) {
    throw new Error(`${BACKEND_WORKSPACE_LOCKFILE} holds no [[package]] blocks; the locked dependency graph is unreadable and unreadable must fail`);
  }
  return lockfileText.split('[[package]]').slice(1).map((block, index) => {
    const name = block.match(/^\s*name\s*=\s*"([^"]+)"/m);
    if (!name) throw new Error(`${BACKEND_WORKSPACE_LOCKFILE} [[package]] block ${index + 1} carries no name; refusing to scan a graph it cannot read`);
    return name[1];
  });
}

/**
 * Cargo-metadata inventory. Not for docs-only preflight — use
 * [`extractConsoleHeadWorkspaceFacts`]. Keep this for fail-closed cargo paths
 * when rustup is on PATH.
 */
export function extractConsoleWorkspaceFacts(repoRoot) {
  const manifestPath = path.join(repoRoot, BACKEND_WORKSPACE_MANIFEST);
  const lockfilePath = path.join(repoRoot, BACKEND_WORKSPACE_LOCKFILE);
  if (!existsSync(manifestPath)) throw new Error(`${BACKEND_WORKSPACE_MANIFEST} is missing under ${repoRoot}; the workspace inventory cannot see its subject and unreadable must fail`);
  if (!existsSync(lockfilePath)) throw new Error(`${BACKEND_WORKSPACE_LOCKFILE} is missing under ${repoRoot}; the workspace inventory cannot see the locked graph and unreadable must fail`);
  let stdout;
  try {
    stdout = execFileSync(
      'cargo',
      ['metadata', '--no-deps', '--format-version', '1', '--offline', '--manifest-path', manifestPath],
      { encoding: 'utf8', maxBuffer: 64 * 1024 * 1024, stdio: ['ignore', 'pipe', 'pipe'] },
    );
  } catch (error) {
    const stderr = typeof error?.stderr === 'string' ? error.stderr.trim() : '';
    throw new Error(`cargo metadata failed; the workspace inventory cannot enumerate workspace members and unreadable must fail: ${stderr || error.message}`);
  }
  let metadata;
  try {
    metadata = JSON.parse(stdout);
  } catch {
    throw new Error('cargo metadata emitted unparseable JSON; the workspace inventory cannot enumerate workspace members and unreadable must fail');
  }
  const packages = Array.isArray(metadata.packages) ? metadata.packages : [];
  const memberViolations = workspaceMemberViolations(packages, { repoRoot });
  const lockedNames = lockedPackageNames(readFileSync(lockfilePath, 'utf8'));
  const uiMemberCount = packages.filter((pkg) => typeof pkg?.name === 'string' && CONSOLE_UI_MEMBER_NAME.test(pkg.name)).length;
  const leptosLockedPackageCount = lockedNames.filter((name) => LEPTOS_PACKAGE_FAMILY.test(name)).length;
  return {
    workspace_scanned: true,
    member_count: packages.length,
    locked_package_count: lockedNames.length,
    ui_member_count: uiMemberCount,
    leptos_locked_package_count: leptosLockedPackageCount,
    violations: memberViolations,
  };
}
