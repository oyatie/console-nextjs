import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import test from 'node:test';
import { gitFixtureEnvironment } from '../lib/git-fixture-environment.mjs';
import { CONSOLE_NAV_SOURCE, CONSOLE_REGISTRY_SOURCE, CONSOLE_UI_MEMBER_NAME, LEPTOS_PACKAGE_FAMILY, extractConsoleHeadWorkspaceFacts, extractConsoleRouteFacts, extractConsoleRouteFactsFromTexts, extractConsoleWorkspaceFacts, lockedPackageNames, parseWorkspaceDependencies, parseWorkspaceMemberManifest, workspaceMemberViolations } from './route-inventory.mjs';

const repoRoot = fileURLToPath(new URL('../../', import.meta.url));
const authorityRegistry = JSON.parse(readFileSync(new URL('../../docs/program/console-capability-registry.json', import.meta.url)));
const ROUTE_CLAIM_FIELDS = ['source_mounted', 'production_exposed', 'registry_body_present', 'nav_declared'];

// The 2026-07-28 clean-slate pivot deleted the frontend, so the immutable
// candidate holds neither console route source. The former positive/negative
// tests read those two files out of the candidate and mutated them; both are
// permanently dead, because the extractor parses TypeScript shapes
// (MOUNTED_SCREEN_KEYS / EXPOSED_SCREEN_KEYS / SCREEN_REGISTRY) that the Leptos
// rebuild will never emit at those paths. They are deleted rather than skipped.
// What replaces them is the contract that still binds in the no-frontend state:
// the registry may not claim route presentation that no source can corroborate.
// Asserted against HEAD, not the registry's bound candidate. The candidate is
// unreachable wherever it matters most: this repository allows squash merges
// only, so C is orphaned the moment a pull request lands and `git ls-tree <C>`
// on `main` is "fatal: not a tree object" — which made this contract
// unverifiable on the only branch that ships. HEAD carries identical `web/**`
// content because T touches nothing but the three authority documents, so it
// asserts the same invariant and always resolves. A git failure here is a
// thrown error, never a skip.
const candidateTracks = (relativePath) => execFileSync(
  'git',
  ['-C', repoRoot, 'ls-tree', 'HEAD', '--', relativePath],
  { encoding: 'utf8' },
).trim() !== '';

test('the tree under test holds no console route source', () => {
  assert.deepEqual(
    [CONSOLE_NAV_SOURCE, CONSOLE_REGISTRY_SOURCE].filter(candidateTracks),
    [],
    'candidate tracks a console route source again: restore the source-inventory contract instead of asserting the empty state',
  );
});

test('with no route source the registry may not claim any route presentation', () => {
  const declared = authorityRegistry.capabilities.flatMap((capability) => capability.route_presentation.route_keys);
  assert.deepEqual(declared, [], 'registry declares route keys but the candidate has no console route source');
  assert.deepEqual(
    authorityRegistry.capabilities
      .filter((capability) => ROUTE_CLAIM_FIELDS.some((field) => capability.route_presentation[field] === true))
      .map((capability) => capability.id),
    [],
    'capabilities claim mounted/exposed/registry-body/nav route presentation that no candidate source can corroborate',
  );
  assert.deepEqual(authorityRegistry.source_inventory?.unmodeled_keys ?? [], [], 'unmodeled route keys cannot exist without a route source');
});

test('route fact extraction fails closed on anything other than a wholly absent source', () => {
  const root = mkdtempSync(path.join(tmpdir(), 'console-route-source-'));
  const write = (relativePath, text) => {
    mkdirSync(path.join(root, path.dirname(relativePath)), { recursive: true });
    writeFileSync(path.join(root, relativePath), text);
  };
  const nav = 'export const MOUNTED_SCREEN_KEYS = ["overview"] as const;\nexport const EXPOSED_SCREEN_KEYS = [] as const;\nconst items = [{ screen: "overview" }];\n';
  const registry = 'export const SCREEN_REGISTRY = {\n  overview: () => null,\n};\n';
  try {
    assert.deepEqual(extractConsoleRouteFacts(root), { route_source_present: false, facts: {} });

    write(CONSOLE_NAV_SOURCE, nav);
    assert.throws(() => extractConsoleRouteFacts(root), /partially present/, 'a half-landed frontend must not report "no routes"');

    write(CONSOLE_REGISTRY_SOURCE, registry);
    const facts = extractConsoleRouteFacts(root);
    assert.equal(facts.route_source_present, true);
    assert.deepEqual(facts.facts, { overview: { source_mounted: true, production_exposed: false, registry_body_present: true, nav_declared: true } });

    write(CONSOLE_NAV_SOURCE, nav.replace('MOUNTED_SCREEN_KEYS', 'RENAMED_SCREEN_KEYS'));
    assert.throws(() => extractConsoleRouteFacts(root), /missing MOUNTED_SCREEN_KEYS/, 'a renamed constant must not be absorbed as "no routes"');

    write(CONSOLE_NAV_SOURCE, nav);
    write(CONSOLE_REGISTRY_SOURCE, registry.replace('SCREEN_REGISTRY', 'RENAMED_REGISTRY'));
    assert.throws(() => extractConsoleRouteFacts(root), /missing SCREEN_REGISTRY/, 'a malformed registry must not be absorbed as "no routes"');
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test('extracted facts classify mounted, exposed, nav-declared and registry-bodied keys apart', () => {
  const facts = extractConsoleRouteFactsFromTexts(
    'export const MOUNTED_SCREEN_KEYS = ["alpha", "beta"] as const;\nexport const EXPOSED_SCREEN_KEYS = ["alpha"] as const;\nconst nav = [{ screen: "alpha" }];\n',
    'export const SCREEN_REGISTRY = {\n  alpha: () => null,\n  gamma: () => null,\n};\n',
  );
  assert.deepEqual(facts.facts, {
    alpha: { source_mounted: true, production_exposed: true, registry_body_present: true, nav_declared: true },
    beta: { source_mounted: true, production_exposed: false, registry_body_present: false, nav_declared: false },
    gamma: { source_mounted: false, production_exposed: false, registry_body_present: true, nav_declared: false },
  });
});

// ---------------------------------------------------------------------------
// ADR-0041, Rust side. React tombstone paths stay absent (test above) — that
// absence-as-green is NOT retired until a mounted shell lands. Ui members are
// allowed; lockfile leptos is allowed; inventory may report zero leptos
// packages. A non-ui crate that declares leptos still fails.
// HEAD classification uses git + lockfile text so docs-only preflight (no
// rustup) does not require cargo.

test('HEAD allows ui members and lockfile leptos; non-ui leptos is forbidden', () => {
  const facts = extractConsoleHeadWorkspaceFacts(repoRoot);
  assert.ok(facts.member_count > 0, 'workspace scan examined zero members; a gate that saw nothing must not pass');
  assert.ok(facts.locked_package_count > 0, 'lockfile scan examined zero packages; a gate that saw nothing must not pass');
  assert.ok(Number.isInteger(facts.ui_member_count), 'inventory must report ui member count (zero is allowed until FE lands)');
  assert.ok(Number.isInteger(facts.leptos_locked_package_count), 'inventory must report lockfile leptos count (zero is allowed until FE lands)');
  assert.deepEqual(
    facts.violations,
    [],
    'a non-ui workspace member must not declare a Leptos-family dependency',
  );
});

test('workspace member manifests surface renamed leptos without cargo metadata', () => {
  const parsed = parseWorkspaceMemberManifest(
    '[package]\nname = "console-payroll-widgets"\n\n[dependencies]\nview-framework = { package = "leptos", version = "0.9.0-beta" }\nlettre = "0.11"\n',
    'backend/crates/payroll/widgets/Cargo.toml',
  );
  assert.equal(parsed.name, 'console-payroll-widgets');
  assert.equal(CONSOLE_UI_MEMBER_NAME.test(parsed.name), false);
  const leptos = parsed.dependencies.find((dep) => dep.name === 'leptos');
  assert.equal(leptos?.rename, 'view-framework');
  assert.deepEqual(
    workspaceMemberViolations([parsed]),
    [
      "ADR-0041 non-ui Leptos violation: workspace member 'console-payroll-widgets' (backend/crates/payroll/widgets/Cargo.toml) declares Leptos-family dependency 'leptos' (renamed locally to 'view-framework'); Leptos is legal only on Layer::Ui members (package name ending in -ui)",
    ],
  );
});

const WORKSPACE_RENAME = '[workspace.dependencies]\nview-framework = { package = "leptos", version = "0.9.0-beta" }\naxum = "0.8.9"\n';
const NON_UI_LEPTOS_VIOLATION = "ADR-0041 non-ui Leptos violation: workspace member 'console-payroll-widgets' (backend/crates/payroll/widgets/Cargo.toml) declares Leptos-family dependency 'leptos' (renamed locally to 'view-framework'); Leptos is legal only on Layer::Ui members (package name ending in -ui)";

test('workspace = true reads package rename from workspace.dependencies', () => {
  const workspacePackageByKey = parseWorkspaceDependencies(WORKSPACE_RENAME);
  assert.equal(workspacePackageByKey['view-framework'], 'leptos');
  assert.equal(workspacePackageByKey.axum, 'axum');

  const dottedWorkspace = parseWorkspaceDependencies('[workspace]\n\n[workspace.dependencies.view-framework]\npackage = "leptos"\nversion = "0.9.0-beta"\n');
  assert.equal(dottedWorkspace['view-framework'], 'leptos');

  for (const member of [
    '[package]\nname = "console-payroll-widgets"\n\n[dependencies]\nview-framework.workspace = true\nlettre = "0.11"\n',
    '[package]\nname = "console-payroll-widgets"\n\n[dependencies]\nview-framework = { workspace = true }\n',
    '[package]\nname = "console-payroll-widgets"\n\n[dependencies.view-framework]\nworkspace = true\n',
  ]) {
    const parsed = parseWorkspaceMemberManifest(member, 'backend/crates/payroll/widgets/Cargo.toml', workspacePackageByKey);
    assert.equal(parsed.dependencies.find((dep) => dep.name === 'leptos')?.rename, 'view-framework');
    assert.deepEqual(workspaceMemberViolations([parsed]), [NON_UI_LEPTOS_VIOLATION]);
  }
});

test('dotted [dependencies.leptos] is classified and does not hide a later table', () => {
  const parsed = parseWorkspaceMemberManifest(
    '[package]\nname = "console-payroll-widgets"\n\n[dependencies]\nlettre = "0.11"\n\n[dependencies.leptos]\nversion = "0.9.0-beta"\n\n[dev-dependencies]\ntokio = "1"\n',
    'backend/crates/payroll/widgets/Cargo.toml',
  );
  assert.deepEqual(
    parsed.dependencies.map((dep) => dep.name).sort(),
    ['leptos', 'lettre', 'tokio'],
  );
  assert.match(workspaceMemberViolations([parsed]).join('\n'), /dependency 'leptos'/);

  const renamedDotted = parseWorkspaceMemberManifest(
    '[package]\nname = "console-payroll-widgets"\n\n[dependencies.view-framework]\npackage = "leptos"\nversion = "0.9.0-beta"\n',
    'backend/crates/payroll/widgets/Cargo.toml',
  );
  assert.deepEqual(workspaceMemberViolations([renamedDotted]), [NON_UI_LEPTOS_VIOLATION]);
});

test('unresolved workspace = true fails closed instead of treating the key as the package', () => {
  assert.throws(
    () => parseWorkspaceMemberManifest(
      '[package]\nname = "console-payroll-widgets"\n\n[dependencies]\nview-framework.workspace = true\n',
      'backend/crates/payroll/widgets/Cargo.toml',
    ),
    /workspace = true/,
    'foo.workspace = true must not be classified as package foo',
  );
});

test('HEAD workspace.dependencies remains readable without cargo', () => {
  const workspacePackageByKey = parseWorkspaceDependencies(readFileSync(path.join(repoRoot, 'backend/Cargo.toml'), 'utf8'));
  assert.equal(workspacePackageByKey.axum, 'axum');
  assert.equal(workspacePackageByKey.sqlx, 'sqlx');
  assert.equal(workspacePackageByKey.leptos, undefined);
});

const writeWorkspaceFixture = (root, crates, { lockfilePackages, workspaceDependencies } = {}) => {
  // Mirrors the real manifest's `crates/<domain>/*` discovery: the RED probe
  // proved a crate becomes a member with zero manifest edits, so the fixture
  // must exercise glob discovery, not an explicit member list.
  mkdirSync(path.join(root, 'backend'), { recursive: true });
  writeFileSync(
    path.join(root, 'backend', 'Cargo.toml'),
    `[workspace]\nresolver = "3"\nmembers = ["crates/*/*"]\n${workspaceDependencies ? `\n[workspace.dependencies]\n${workspaceDependencies}` : ''}`,
  );
  for (const crate of crates) {
    const dir = path.join(root, 'backend', crate.path);
    mkdirSync(path.join(dir, 'src'), { recursive: true });
    writeFileSync(path.join(dir, 'src', 'lib.rs'), '');
    writeFileSync(path.join(dir, 'Cargo.toml'), crate.manifest ?? `[package]\nname = "${crate.name}"\nversion = "0.0.0"\nedition = "2021"\n\n[dependencies]\n${crate.dependencies ?? ''}`);
  }
  if (lockfilePackages) {
    writeFileSync(
      path.join(root, 'backend', 'Cargo.lock'),
      `version = 4\n${lockfilePackages.map((name) => `\n[[package]]\nname = "${name}"\nversion = "0.0.0"\n`).join('')}`,
    );
  }
};

function trackFixture(root) {
  const env = gitFixtureEnvironment();
  execFileSync('git', ['init'], { cwd: root, env, stdio: 'pipe' });
  execFileSync('git', ['add', '-A'], { cwd: root, env, stdio: 'pipe' });
  execFileSync('git', ['-c', 'user.name=gate', '-c', 'user.email=gate@example.com', 'commit', '-m', 'fixture'], { cwd: root, env, stdio: 'pipe' });
}

test('HEAD cargo-free inventory flags workspace-renamed and dotted-table Leptos', () => {
  const root = mkdtempSync(path.join(tmpdir(), 'console-ui-head-inventory-'));
  try {
    writeWorkspaceFixture(
      root,
      [
        {
          path: 'crates/payroll/widgets',
          name: 'console-payroll-widgets',
          dependencies: 'view-framework.workspace = true\n',
        },
        {
          path: 'crates/hr/domain',
          name: 'console-hr-domain',
          manifest: '[package]\nname = "console-hr-domain"\nversion = "0.0.0"\nedition = "2021"\n\n[dependencies]\nlettre = "0.11"\n\n[dependencies.leptos]\nversion = "0.9.0-beta"\n',
        },
        {
          path: 'crates/payroll/ui',
          name: 'console-payroll-ui',
          dependencies: 'view-framework.workspace = true\n',
        },
      ],
      {
        workspaceDependencies: 'view-framework = { package = "leptos", version = "0.9.0-beta" }\n',
        lockfilePackages: ['console-payroll-widgets', 'console-hr-domain', 'console-payroll-ui', 'leptos', 'lettre'],
      },
    );
    trackFixture(root);
    const facts = extractConsoleHeadWorkspaceFacts(root);
    const text = facts.violations.join('\n');
    assert.equal(facts.member_count, 3);
    assert.equal(facts.ui_member_count, 1);
    assert.equal(facts.leptos_locked_package_count, 1);
    assert.equal(facts.violations.length, 2, `expected non-ui workspace-rename and dotted leptos, got:\n${text}`);
    assert.match(text, /'console-payroll-widgets' .* dependency 'leptos' \(renamed locally to 'view-framework'\)/);
    assert.match(text, /'console-hr-domain' .* dependency 'leptos'/);
    assert.doesNotMatch(text, /console-payroll-ui/);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test('the workspace scan flags non-ui leptos and allows ui members plus lockfile leptos', () => {
  const packages = [
    { name: 'console-payroll-ui', manifest_path: 'backend/crates/payroll/ui/Cargo.toml', dependencies: [{ name: 'leptos' }, { name: 'leptos_axum' }] },
    { name: 'console-payroll-widgets', manifest_path: 'backend/crates/payroll/widgets/Cargo.toml', dependencies: [{ name: 'leptos', rename: 'view-framework' }] },
    { name: 'console-hr-ui', manifest_path: 'backend/crates/hr/ui/Cargo.toml', dependencies: [] },
    { name: 'console-payroll-gui', manifest_path: 'backend/crates/payroll/gui/Cargo.toml', dependencies: [{ name: 'lettre' }, { name: 'leptonic' }] },
  ];
  const violations = workspaceMemberViolations(packages);
  const text = violations.join('\n');
  assert.equal(packages.filter((pkg) => CONSOLE_UI_MEMBER_NAME.test(pkg.name)).length, 2);
  const locked = lockedPackageNames('version = 4\n\n[[package]]\nname = "console-payroll-ui"\nversion = "0.0.0"\n\n[[package]]\nname = "leptos"\nversion = "0.0.0"\n\n[[package]]\nname = "leptos_axum"\nversion = "0.0.0"\n\n[[package]]\nname = "leptose"\nversion = "0.0.0"\n\n[[package]]\nname = "leptonic"\nversion = "0.0.0"\n');
  assert.equal(locked.filter((name) => LEPTOS_PACKAGE_FAMILY.test(name)).length, 2, 'lockfile leptos+leptos_axum must be counted, not failed');
  assert.equal(violations.length, 1, `expected only the non-ui leptos rename, got:\n${text}`);
  assert.ok(violations.every((violation) => violation.startsWith('ADR-0041 non-ui Leptos violation')), text);
  assert.match(text, /'console-payroll-widgets' .* declares Leptos-family dependency 'leptos' \(renamed locally to 'view-framework'\)/, 'a renamed dependency on a non-ui crate must be reported under its TRUE package name');
  assert.doesNotMatch(text, /console-payroll-ui|console-hr-ui/, 'ui members must not be violations');
  assert.doesNotMatch(text, /Cargo\.lock resolves/, 'lockfile leptos must not be a violation');
  assert.doesNotMatch(text, /console-payroll-gui|'leptonic'|'leptose'|'lettre'/, 'near-miss names and dependencies must not be flagged');
});

function cargoOnPath() {
  try {
    execFileSync('cargo', ['--version'], { stdio: 'ignore' });
    return true;
  } catch {
    return false;
  }
}

test('the workspace scan fails closed on anything it cannot read', () => {
  assert.throws(() => workspaceMemberViolations([]), /examined zero workspace members/, 'zero members must fail, never pass');
  assert.throws(() => workspaceMemberViolations([{ name: 'x' }]), /cannot read/, 'an unreadable member must fail, never be skipped');
  assert.throws(() => workspaceMemberViolations([{ name: 'x', manifest_path: '/x/Cargo.toml', dependencies: [{}] }]), /unreadable dependency/, 'an unreadable dependency must fail, never be skipped');
  assert.throws(() => lockedPackageNames(''), /unreadable must fail/, 'an empty lockfile must fail, never scan clean');
  assert.throws(() => lockedPackageNames('[[package]]\nversion = "1.0.0"\n'), /carries no name/, 'a nameless package block must fail, never be skipped');
  assert.throws(() => extractConsoleWorkspaceFacts(path.join(tmpdir(), 'console-ui-inventory-absent-root')), /backend\/Cargo\.toml is missing/, 'a missing manifest must fail, never pass');
  const root = mkdtempSync(path.join(tmpdir(), 'console-ui-inventory-closed-'));
  try {
    writeWorkspaceFixture(root, [{ path: 'crates/hr/domain', name: 'console-hr-domain' }]);
    assert.throws(() => extractConsoleWorkspaceFacts(root), /backend\/Cargo\.lock is missing/, 'a missing lockfile must fail, never pass');
    if (!cargoOnPath()) {
      // docs-only preflight has no rustup; do not require cargo/leptos here.
      return;
    }
    writeWorkspaceFixture(root, [{ path: 'crates/hr/domain', name: 'console-hr-domain' }], { lockfilePackages: ['console-hr-domain'] });
    assert.deepEqual(extractConsoleWorkspaceFacts(root).violations, [], 'a clean backend-shaped workspace must pass');
    writeFileSync(path.join(root, 'backend', 'Cargo.toml'), '[workspace]\nresolver = "3"\nmembers = ["crates/nonexistent/*"]\n');
    assert.throws(() => extractConsoleWorkspaceFacts(root), /cargo metadata failed/, 'an unresolvable member glob must fail, never pass');
    writeFileSync(path.join(root, 'backend', 'Cargo.toml'), '[workspace]\nresolver = "3"\nmembers = []\n');
    assert.throws(() => extractConsoleWorkspaceFacts(root), /examined zero workspace members/, 'an empty workspace must fail, never pass');
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});
