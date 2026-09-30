#!/usr/bin/env node
import assert from "node:assert/strict";
import { existsSync, readFileSync } from "node:fs";
import { spawnSync } from "node:child_process";
import test from "node:test";

const root = new URL("../", import.meta.url);
const read = (path) => readFileSync(new URL(path, root), "utf8");

const run = (command, args) =>
  spawnSync(command, args, {
    cwd: root,
    encoding: "utf8",
    stdio: ["ignore", "pipe", "pipe"],
  });

const render = (overlay) => {
  const result = run("kubectl", ["kustomize", overlay]);
  assert.equal(
    result.status,
    0,
    `kubectl kustomize ${overlay} failed:\n${result.stderr}`,
  );
  return result.stdout;
};

test("database-backed CI and release probes reconcile, migrate, and serve with separate identities", () => {
  for (const path of [
    ".github/workflows/ci.yml",
    ".github/workflows/image-release.yml",
  ]) {
    const source = read(path);
    assert.match(source, /postgres-reconcile-topology\.sh/);
    assert.match(source, /console_app:\$\{?(?:APP_PASSWORD|PROBE_OWNER)/);
    assert.match(source, /console_rt:\$\{?(?:RT_PASSWORD|PROBE_RUNTIME)/);
    assert.doesNotMatch(
      source,
      /CONSOLE_APP_ROLE=migrate[^\n]*DATABASE_URL="postgres:\/\/postgres:/,
    );
  }

  const ci = read(".github/workflows/ci.yml");
  assert.match(ci, /CONSOLE_PLATFORM_FORCE_COMMAND_POSTGRES_PASSWORD="\$PLATFORM_FORCE_COMMAND_PASSWORD"/);
  assert.match(ci, /PLATFORM_FORCE_COMMAND_DATABASE_URL="postgres:\/\/console_platform_force_cmd:/);
});

test("the dev boot path never aliases command URLs to DATABASE_URL", () => {
  // dev-up.mjs is the surviving boot path (the contract and browser harnesses
  // it replaced were deleted with the frontend): each command role must get
  // its OWN least-privilege URL, never a reuse of the runtime/owner
  // DATABASE_URL.
  const devUp = read("scripts/dev-up.mjs");
  assert.match(devUp, /DATABASE_URL: role === "migrate" \? databaseUrl\(\) : runtimeDatabaseUrl\(\)/);
  assert.match(devUp, /LEAVE_COMMAND_DATABASE_URL: commandDatabaseUrl\(\s*"console_leave_cmd"/);
  assert.match(devUp, /ONTOLOGY_COMMAND_DATABASE_URL: commandDatabaseUrl\(\s*"console_ontology_cmd"/);
  assert.match(
    devUp,
    /PLATFORM_FORCE_COMMAND_DATABASE_URL: commandDatabaseUrl\(\s*"console_platform_force_cmd"/,
  );
  assert.doesNotMatch(
    devUp,
    /(?:LEAVE|ONTOLOGY|PLATFORM_FORCE)_COMMAND_DATABASE_URL:\s*(?:databaseUrl\(\)|runtimeDatabaseUrl\(\)|process\.env\.DATABASE_URL)/,
  );
});

test("append-only migration 0167 declares serving-role bounds and nonclaims", () => {
  const path =
    "backend/crates/platform/db/migrations/0167_serving_role_transaction_timeouts.sql";
  assert.ok(existsSync(new URL(path, root)), `${path} must exist`);
  const migration = read(path);
  for (const role of ["console_rt", "console_leave_cmd", "console_ontology_cmd"]) {
    assert.match(migration, new RegExp(`'${role}'`));
  }
  assert.match(migration, /ALTER ROLE %I SET statement_timeout/);
  assert.match(migration, /ALTER ROLE %I SET idle_in_transaction_session_timeout/);
  assert.match(migration, /ALTER ROLE %I SET transaction_timeout/);
  assert.match(migration, /statement_timeout=30s/);
  assert.match(migration, /idle_in_transaction_session_timeout=30s/);
  assert.match(migration, /transaction_timeout=45s/);
  assert.match(migration, /owner[\s\S]*outside this reconciliation and[\s\S]*startup correctness backstop/);
  assert.match(migration, /quiescence\/coordination[\s\S]*xmin\/snapshot watermark/);
});

test("topology reconciliation drains every serving role after changing defaults", () => {
  const topology = read("ops/postgres-reconcile-topology.sh");
  assert.match(
    topology,
    /WHERE usename IN \('console_rt','console_leave_cmd','console_ontology_cmd','console_platform_force_cmd'\) AND pid <> pg_backend_pid\(\) ORDER BY pid/,
  );
  assert.match(topology, /verify_serving_login console_platform_force_cmd/);
});

test("live Argo, base, prod, and secret wiring remain DARK-topology-free", () => {
  const argo = read("deploy/argocd/apps/console.yaml");
  const prod = read("deploy/apps/console/overlays/prod/kustomization.yaml");
  const base = read("deploy/apps/console/base/kustomization.yaml");
  const database = read("deploy/apps/console/base/database.yaml");
  const backend = read("deploy/apps/console/base/backend.yaml");
  const secrets = read(
    "deploy/apps/secrets-management/wiring/kustomization.yaml",
  );

  assert.match(argo, /path: deploy\/apps\/console\/overlays\/prod/);
  assert.match(argo, /targetRevision: main/);
  assert.match(prod, /resources:\s*\n\s+- \.\.\/\.\.\/base/);
  assert.doesNotMatch(prod, /components:|pr-473|governed-command-database/);
  assert.doesNotMatch(base, /database-topology-job|governed-command-database/);

  for (const source of [argo, prod, base, database, backend, secrets]) {
    assert.doesNotMatch(source, /pr-473-expand|governed-command-database/);
    assert.doesNotMatch(source, /console-db-(?:leave|ontology)-command/);
    assert.doesNotMatch(
      source,
      /(?:LEAVE|ONTOLOGY)_COMMAND_DATABASE_URL|console_(?:leave|ontology)_(?:cmd|definer|writer)/,
    );
  }

  const mainRef = run("git", ["rev-parse", "--verify", "origin/main^{commit}"]);
  assert.equal(
    mainRef.status,
    0,
    `origin/main is mandatory for the live GitOps identity gate:\n${mainRef.stderr}`,
  );
  const LIVE_PATHS = [
    "deploy/argocd/apps/console.yaml",
    "deploy/apps/console/base",
    "deploy/apps/console/overlays/prod",
    "deploy/apps/secrets-management/wiring",
  ];
  const changed = run("git", ["diff", "--name-only", "origin/main", "--", ...LIVE_PATHS]);
  assert.equal(changed.status, 0, `git diff failed:\n${changed.stderr}`);
  const changedPaths = changed.stdout.split("\n").map((line) => line.trim()).filter(Boolean);

  // ArgoCD syncs these paths from `main` with `targetRevision: main`, so a change to any of
  // them takes effect the instant it merges. The assertions above keep the DARK
  // governed-command-database topology out by NAME; this one is the backstop for a topology
  // nobody has named yet, and it used to be an unconditional byte-identity check.
  //
  // An unconditional identity check is not a gate, it is a wall: it cannot pass on any branch
  // that changes these paths, because it compares against `origin/main`, which by definition
  // does not carry the change yet. A control with no exception route either stops all change
  // or gets deleted by whoever needs the next change badly enough — a 90-day retention policy
  // was withdrawn rather than landed for exactly this reason, and the withdrawal is recorded
  // in the program ledger.
  //
  // So the route is: declare the change. Every changed path must appear on a line ADDED to
  // LIVE_GITOPS_CHANGES relative to origin/main. Naming a path once does not buy silence
  // forever — the declaration has to be new, because the diff of the declaration file is what
  // is read. Nothing changes silently; deliberate change costs one line.
  if (changedPaths.length > 0) {
    const LIVE_GITOPS_CHANGES = "deploy/apps/console/LIVE-GITOPS-CHANGES.md";
    const declared = run("git", ["diff", "origin/main", "--", LIVE_GITOPS_CHANGES]);
    assert.equal(declared.status, 0, `git diff failed:\n${declared.stderr}`);
    // `git diff` cannot see an untracked file, so a declaration written but never `git add`ed
    // produces an empty diff and the failure below reads as "you did not declare it" when the
    // author is looking straight at the file they just wrote. Say which of the two it is.
    if (declared.stdout === "" && existsSync(new URL(LIVE_GITOPS_CHANGES, root))) {
      const tracked = run("git", ["ls-files", "--error-unmatch", LIVE_GITOPS_CHANGES]);
      assert.equal(
        tracked.status,
        0,
        `${LIVE_GITOPS_CHANGES} exists but is untracked, so git diff cannot see it and the declaration below will look absent. Run: git add ${LIVE_GITOPS_CHANGES}`,
      );
    }
    const addedLines = declared.stdout
      .split("\n")
      .filter((line) => line.startsWith("+") && !line.startsWith("+++"));
    const undeclared = changedPaths.filter(
      (entry) => !addedLines.some((line) => line.includes(entry)),
    );
    assert.deepEqual(
      undeclared,
      [],
      `live GitOps inputs changed without a declaration. ArgoCD syncs these from main, so ${
        undeclared.length
      } path(s) would take effect on merge:\n${undeclared.map((entry) => `  ${entry}`).join("\n")}\n\n` +
        `Add an entry to ${LIVE_GITOPS_CHANGES} naming each path and why it changed.`,
    );
  }
});

test("DARK overlays opt into the portable governed command-database component", () => {
  const cases = [
    ["pr-473-expand-oci-guest", "../prod"],
    ["pr-473-expand-on-prem", "../on-prem"],
  ];

  for (const [overlay, base] of cases) {
    const source = read(
      `deploy/apps/console/overlays/${overlay}/kustomization.yaml`,
    );
    assert.match(
      source,
      new RegExp(`resources:\\s*\\n\\s+- ${base.replaceAll(".", "\\.")}`),
    );
    assert.match(
      source,
      /components:\s*\n\s+- \.\.\/\.\.\/components\/governed-command-database/,
    );
  }
});

test("governed command-database component declares seven roles, topology readback, ordering, and bounded Job networking", () => {
  const component = read(
    "deploy/apps/console/components/governed-command-database/kustomization.yaml",
  );
  const topology = read(
    "deploy/apps/console/components/governed-command-database/database-topology-job.yaml",
  );

  const managedNames = [
    ...component.matchAll(/^\s+- name: (console_[a-z_]+)$/gm),
  ].map((match) => match[1]);
  assert.deepEqual(managedNames, [
    "console_app",
    "console_rt",
    "console_leave_cmd",
    "console_ontology_cmd",
    "console_platform_force_cmd",
    "console_leave_definer",
    "console_ontology_writer",
  ]);
  assert.match(
    component,
    /name: console_app[\s\S]*?inRoles: \[console_leave_definer, console_ontology_writer\]/,
  );
  assert.match(component, /name: console_app[\s\S]*?bypassrls: true/);
  for (const role of managedNames.slice(1)) {
    assert.match(
      component,
      new RegExp(`name: ${role}[\\s\\S]*?bypassrls: false`),
    );
  }

  assert.match(component, /database-topology-job\.yaml/);
  assert.match(
    component,
    /path: \/spec\/enableSuperuserAccess[\s\S]*?value: false/,
  );
  assert.match(
    component,
    /path: \/spec\/postgresql\/parameters\/max_prepared_transactions[\s\S]*?value: "0"/,
  );
  assert.match(
    component,
    /name: console_app[\s\S]*?passwordSecret:\s*\n\s+name: console-db-app/,
  );
  assert.match(component, /name: LEAVE_COMMAND_DATABASE_URL/);
  assert.match(component, /name: ONTOLOGY_COMMAND_DATABASE_URL/);
  assert.match(component, /name: PLATFORM_FORCE_COMMAND_DATABASE_URL/);
  assert.match(component, /name: console-migrate[\s\S]*?value: Sync/);
  assert.match(
    component,
    /name: console-migrate[\s\S]*?sync-wave[\s\S]*?value: "2"/,
  );
  assert.match(
    component,
    /kind: Rollout, name: console-app[\s\S]*?argocd\.argoproj\.io\/sync-wave: "3"/,
  );
  assert.match(
    component,
    /kind: Deployment, name: console-worker[\s\S]*?argocd\.argoproj\.io\/sync-wave: "3"/,
  );
  for (const policy of [
    "allow-postgres-from-app",
    "default-deny-egress-app-tier",
    "allow-app-egress-dns",
    "allow-app-egress-postgres",
  ]) {
    assert.match(
      component,
      new RegExp(
        `kind: NetworkPolicy, name: ${policy}[\\s\\S]*?console-db-topology`,
      ),
    );
  }

  assert.match(
    topology,
    /expected_roles='console_app\|t\|f\|t\|t\|f\|f\|f[\s\S]*?console_platform_force_cmd\|t\|f\|f\|f\|f\|f\|f/,
  );
  assert.match(topology, /argocd\.argoproj\.io\/hook: Sync/);
  assert.match(topology, /argocd\.argoproj\.io\/sync-wave: "1"/);
  assert.match(topology, /membership\.admin_option/);
  assert.match(topology, /membership\.inherit_option/);
  assert.match(topology, /membership\.set_option/);
  assert.match(topology, /OR granted\.rolname IN/);
  assert.match(topology, /test "\$\{PGUSER\}" = console_app/);
  assert.match(topology, /secretKeyRef: \{ name: console-db-app, key: username \}/);
  assert.match(
    topology,
    /PGUSER="\$\{role\}" PGPASSWORD="\$\{password\}" psql[\s\S]*?BEGIN;[\s\S]*?ALTER ROLE %I SET statement_timeout[\s\S]*?ALTER ROLE %I SET idle_in_transaction_session_timeout[\s\S]*?ALTER ROLE %I SET transaction_timeout[\s\S]*?ALTER ROLE %I IN DATABASE %I RESET statement_timeout[\s\S]*?ALTER ROLE %I IN DATABASE %I RESET idle_in_transaction_session_timeout[\s\S]*?ALTER ROLE %I IN DATABASE %I RESET transaction_timeout[\s\S]*?COMMIT;/,
  );
  for (const [role, password] of [
    ["console_rt", "CONSOLE_RT_PASSWORD"],
    ["console_leave_cmd", "CONSOLE_LEAVE_COMMAND_PASSWORD"],
    ["console_ontology_cmd", "CONSOLE_ONTOLOGY_COMMAND_PASSWORD"],
    ["console_platform_force_cmd", "CONSOLE_PLATFORM_FORCE_COMMAND_PASSWORD"],
  ]) {
    assert.match(
      topology,
      new RegExp(
        `reconcile_serving_defaults ${role} "\\$\\{${password}\\}"`,
      ),
    );
    assert.match(
      topology,
      new RegExp(
        `assert_direct_serving_login ${role} "\\$\\{${password}\\}" '30s\\|30s\\|45s'`,
      ),
    );
  }
  assert.match(topology, /current_setting\('server_version_num'\)/);
  assert.match(topology, /current_setting\('max_prepared_transactions'\)/);
  assert.match(topology, /pg_prepared_xacts/);
  assert.match(topology, /pg_terminate_backend/);
  assert.match(topology, /pg_terminate_backend\(\$\{pid\}, 5000\)/);
  assert.match(topology, /captured_pid_output="\$\(PGOPTIONS=/);
  assert.doesNotMatch(topology, /mapfile -t captured_pids < <\(/);
  assert.match(topology, /pid = ANY \(ARRAY\[\$\{captured_pid_csv\}\]::integer\[\]\)/);
  assert.match(topology, /repair_pgoptions='-c statement_timeout=0 -c idle_in_transaction_session_timeout=0 -c transaction_timeout=0'/);
  assert.match(topology, /serving_defaults_need_repair\(\)/);
  assert.match(topology, /repair_console_rt="\$\(serving_defaults_need_repair console_rt/);
  assert.match(topology, /if \[\[ "\$\{repair_console_rt\}" == true \]\]; then[\s\S]*?reconcile_serving_defaults console_rt/);
  assert.match(topology, /if \[\[ "\$\{repair_console_rt\}" == true \]\]; then[\s\S]*?drain_serving_backends console_rt/);
  const preflightEnd = topology.indexOf(
    'preflight_serving_login console_platform_force_cmd "${CONSOLE_PLATFORM_FORCE_COMMAND_PASSWORD}"',
  );
  const mutationStart = topology.indexOf(
    'reconcile_serving_defaults console_rt "${CONSOLE_RT_PASSWORD}"',
  );
  const repairClassification = topology.indexOf(
    'repair_console_rt="$(serving_defaults_need_repair console_rt',
  );
  const mutationEnd = topology.indexOf(
    'reconcile_serving_defaults console_platform_force_cmd "${CONSOLE_PLATFORM_FORCE_COMMAND_PASSWORD}"',
  );
  const drainStart = topology.indexOf(
    'drain_serving_backends console_rt "${CONSOLE_RT_PASSWORD}"',
  );
  const freshReadback = topology.indexOf(
    'assert_direct_serving_login console_rt "${CONSOLE_RT_PASSWORD}"',
  );
  assert.ok(preflightEnd > 0 && preflightEnd < repairClassification);
  assert.ok(repairClassification < mutationStart);
  assert.ok(mutationEnd < drainStart && drainStart < freshReadback);
  const platformForceMutation = topology.indexOf(
    'reconcile_serving_defaults console_platform_force_cmd "${CONSOLE_PLATFORM_FORCE_COMMAND_PASSWORD}"',
  );
  const platformForceDrain = topology.indexOf(
    'drain_serving_backends console_platform_force_cmd "${CONSOLE_PLATFORM_FORCE_COMMAND_PASSWORD}"',
  );
  const platformForceReadback = topology.indexOf(
    'assert_direct_serving_login console_platform_force_cmd "${CONSOLE_PLATFORM_FORCE_COMMAND_PASSWORD}"',
  );
  assert.ok(platformForceMutation < platformForceDrain && platformForceDrain < platformForceReadback);
  assert.match(
    topology,
    /passwords=\([\s\S]*?\$\{PGPASSWORD\}[\s\S]*?\$\{CONSOLE_RT_PASSWORD\}[\s\S]*?\$\{CONSOLE_LEAVE_COMMAND_PASSWORD\}[\s\S]*?\$\{CONSOLE_ONTOLOGY_COMMAND_PASSWORD\}[\s\S]*?\$\{CONSOLE_PLATFORM_FORCE_COMMAND_PASSWORD\}[\s\S]*?\)/,
  );
  assert.match(
    topology,
    /for \(\(i = 0; i < \$\{#passwords\[@\]\}; i\+\+\)\); do[\s\S]*?test -n "\$\{passwords\[i\]\}"[\s\S]*?for \(\(j = i \+ 1; j < \$\{#passwords\[@\]\}; j\+\+\)\); do[\s\S]*?test "\$\{passwords\[i\]\}" != "\$\{passwords\[j\]\}"/,
  );
  assert.match(
    topology,
    /SELECT session_user::text \|\| '\|' \|\| current_user::text"\)" = 'console_app\|console_app'/,
  );
  assert.match(
    topology,
    /expected_memberships='console_app\|console_leave_definer\|f\|t\|t\s+console_app\|console_ontology_writer\|f\|t\|t'/,
  );
  assert.match(
    topology,
    /test "\$\{actual_memberships\}" = "\$\{expected_memberships\}"/,
  );
  assert.match(
    topology,
    /membership\.member = authenticated\.oid\s+OR membership\.roleid = authenticated\.oid/,
  );
  assert.match(
    topology,
    /test "\$\{actual\}" = "\$\{role\}\|\$\{role\}\|t\|f\|f\|f\|f\|f\|f\|f\|\$\{expected_defaults\}"/,
  );
  for (const [secret, role] of [
    ["console-db-rt", "console_rt"],
    ["console-db-leave-command", "console_leave_cmd"],
    ["console-db-ontology-command", "console_ontology_cmd"],
  ]) {
    assert.match(topology, new RegExp(`name: ${secret}, key: username`));
    assert.match(topology, new RegExp(`assert_direct_serving_login ${role}`));
  }
  assert.doesNotMatch(topology, /console-db-superuser/);
  assert.match(component, /enableSuperuserAccess[\s\S]*?value: false/);
  assert.equal(
    component.match(/console\.oyatie\.com\/database-role-defaults: "0167"/g)
      ?.length,
    2,
  );
});

test("DARK OCI and self-host renders include the governed topology without changing live prod", () => {
  const kubectl = run("kubectl", ["version", "--client=true"]);
  assert.equal(
    kubectl.status,
    0,
    `kubectl with the pinned kustomize renderer is mandatory:\n${kubectl.stderr}`,
  );
  const prod = render("deploy/apps/console/overlays/prod");
  assert.doesNotMatch(prod, /name: console-db-topology/);
  assert.doesNotMatch(
    prod,
    /LEAVE_COMMAND_DATABASE_URL|ONTOLOGY_COMMAND_DATABASE_URL/,
  );

  for (const overlay of ["pr-473-expand-oci-guest", "pr-473-expand-on-prem"]) {
    const rendered = render(`deploy/apps/console/overlays/${overlay}`);
    for (const role of [
      "console_app",
      "console_rt",
      "console_leave_cmd",
      "console_ontology_cmd",
      "console_leave_definer",
      "console_ontology_writer",
    ]) {
      assert.match(rendered, new RegExp(`name: ${role}`));
    }
    assert.match(
      rendered,
      /kind: Job\s+metadata:[\s\S]*?name: console-db-topology/,
    );
    assert.match(rendered, /name: LEAVE_COMMAND_DATABASE_URL/);
    assert.match(rendered, /name: ONTOLOGY_COMMAND_DATABASE_URL/);
    assert.match(rendered, /argocd\.argoproj\.io\/sync-wave: "?1"?/);
    assert.match(rendered, /argocd\.argoproj\.io\/sync-wave: "?2"?/);
    assert.match(rendered, /argocd\.argoproj\.io\/sync-wave: "?3"?/);
    assert.match(
      rendered,
      /kind: NetworkPolicy[\s\S]*?name: allow-postgres-from-app[\s\S]*?console-db-topology/,
    );
    assert.match(
      rendered,
      /kind: NetworkPolicy[\s\S]*?name: allow-app-egress-postgres[\s\S]*?console-db-topology/,
    );
  }
});

test("DARK operating contract locks whole-Application activation, credentials, rotation, and capacity", () => {
  const databaseDocs = read(
    "deploy/apps/console/components/governed-command-database/README.md",
  );
  const secretDocs = read(
    "deploy/apps/secrets-management/components/governed-command-database/README.md",
  );
  const ociDocs = read(
    "deploy/apps/console/overlays/pr-473-expand-oci-guest/README.md",
  );
  const onPremDocs = read(
    "deploy/apps/console/overlays/pr-473-expand-on-prem/README.md",
  );
  const docs = [databaseDocs, secretDocs, ociDocs, onPremDocs].join("\n");

  assert.match(
    databaseDocs,
    /Never selectively sync[\s\S]*?Sync the\s+whole console Application/,
  );
  assert.match(
    secretDocs,
    /sync the complete console Application[\s\S]*?Do not selectively sync/,
  );
  assert.match(docs, /32-byte hexadecimal/);
  assert.match(docs, /percent-encode/);
  assert.match(docs, /kubernetes\.io\/basic-auth/);
  assert.match(docs, /cnpg\.io\/reload=true/);
  assert.match(docs, /restart every consumer deliberately/i);
  assert.match(docs, /Wait for rollout\/deployment readiness/);
  assert.match(docs, /retired password is rejected/);
  assert.match(docs, /Do not claim zero-downtime rotation/);
  assert.match(
    databaseDocs,
    /pool at 6 connections and each API command pool at 2/,
  );
  assert.match(databaseDocs, /4 x \(6 \+ 2 \+ 2 \+ 2\) = 48/);
  assert.match(databaseDocs, /2 x 6 = 12/);
  assert.match(
    databaseDocs,
    /total serving demand is 60, leaving no headroom/,
  );
  assert.match(databaseDocs, /PostgreSQL is configured for 60 connections/);
  assert.match(docs, /pairwise distinct/);
  assert.match(docs, /session_user = current_user/);
  assert.match(docs, /expected role and membership rows/);
});

test("DARK secrets component contains exactly three typed ExternalSecrets and live wiring does not reference it", () => {
  const componentPath =
    "deploy/apps/secrets-management/components/governed-command-database";
  const kustomization = read(`${componentPath}/kustomization.yaml`);
  const expectedFiles = [
    "externalsecret-console-db-leave-command.yaml",
    "externalsecret-console-db-ontology-command.yaml",
    "externalsecret-console-db-platform-force-command.yaml",
  ];

  const resources = [
    ...kustomization.matchAll(/^\s+- (externalsecret-[^\s]+\.yaml)$/gm),
  ].map((match) => match[1]);
  assert.deepEqual(resources, expectedFiles);

  for (const file of expectedFiles) {
    assert.ok(existsSync(new URL(`${componentPath}/${file}`, root)));
    const source = read(`${componentPath}/${file}`);
    assert.match(source, /apiVersion: external-secrets\.io\/v1/);
    assert.match(source, /kind: ExternalSecret/);
    assert.match(source, /type: kubernetes\.io\/basic-auth/);
    assert.match(source, /cnpg\.io\/reload: "true"/);
    for (const key of ["username", "password", "uri"]) {
      assert.match(source, new RegExp(`secretKey: ${key}`));
    }
  }

  const liveWiring = read(
    "deploy/apps/secrets-management/wiring/kustomization.yaml",
  );
  assert.doesNotMatch(liveWiring, /governed-command-database/);
  assert.doesNotMatch(
    liveWiring,
    /externalsecret-console-db-(?:leave|ontology|platform-force)-command/,
  );
});
