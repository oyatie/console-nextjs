import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import {
  chmodSync,
  cpSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  realpathSync,
  rmSync,
  symlinkSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { afterEach, describe, it } from "node:test";
import { evaluate } from "./check-production-authority-blocked.mjs";
import { installGitFixtureEnvironment } from "./lib/git-fixture-environment.mjs";

installGitFixtureEnvironment();

const script = join(
  dirname(fileURLToPath(import.meta.url)),
  "check-production-authority-blocked.mjs",
);
const paths = [
  "deploy/argocd/apps/console.yaml",
  "deploy/argocd/project.yaml",
  "deploy/argocd/root.yaml",
  "docs/release/PR-473-PRODUCTION-CARDINALITY.evidence.json",
  "docs/release/PR-473-PRODUCTION-PROMOTION.authorization.json",
];
const cleanup = [];
afterEach(() => {
  while (cleanup.length)
    rmSync(cleanup.pop(), { recursive: true, force: true });
});
const zero = "0".repeat(40),
  template = "TEMPLATE_NOT_EVIDENCE";

function git(cwd, ...args) {
  const result = spawnSync("git", args, { cwd, encoding: "utf8" });
  assert.equal(result.status, 0, result.stderr);
  return result.stdout.trim();
}
function fixture() {
  const root = mkdtempSync(join(tmpdir(), "blocked-observation-"));
  cleanup.push(root);
  mkdirSync(join(root, "scripts"), { recursive: true });
  cpSync(script, join(root, "scripts/check-production-authority-blocked.mjs"));
  const cardinality = {
    schema_version: 1,
    target: "production",
    release_phase: "expand",
    candidate_source_sha: zero,
    observed_running_revision: zero,
    observed_database_topology: {
      cluster_name: template,
      namespace: template,
      writer_endpoint: template,
      reader_endpoint: template,
      instances: [],
    },
    capacity_headroom: {
      window_started_at: template,
      window_ended_at: template,
      cpu_peak_percent: 0,
      memory_peak_percent: 0,
      storage_used_percent: 0,
      connection_peak: 0,
      connection_limit: 0,
      minimum_headroom_percent: 0,
    },
    backup_restore_proof: {
      backup_id: template,
      backup_completed_at: template,
      isolated_restore_id: template,
      isolated_restore_completed_at: template,
      restored_revision: zero,
      validation_checks: [],
    },
    evidence_author: {
      github_login: template,
      identity_provider_subject: template,
    },
    independent_reviewer: {
      github_login: template,
      identity_provider_subject: template,
      team_id: 0,
    },
    charter: { charter_id: template, trust_domain_id: template },
    observed_at: template,
    prepared_at: template,
    reviewed_at: template,
  };
  const cardinalityText = `${JSON.stringify(cardinality)}\n`;
  const authorization = {
    schema_version: 2,
    pull_request: 473,
    target: "production",
    release_phase: "expand",
    rollback_floor: "f6ff236b9770c79301a3d07da6afb56be1e27bbf",
    desired_state_authority_cutover: false,
    deployment_authorized: false,
    command_only: false,
    production_cardinality_evidence: {
      path: paths[3],
      sha256: createHash("sha256").update(cardinalityText).digest("hex"),
      verified: false,
    },
    contract_authorities: {
      old_runtime_drain: false,
      rollback_floor_raise: false,
    },
  };
  const files = {
    [paths[0]]:
      "apiVersion: argoproj.io/v1alpha1\nkind: Application\nmetadata:\n  name: console\nspec:\n  source:\n    targetRevision: main\n",
    [paths[1]]:
      'apiVersion: argoproj.io/v1alpha1\nkind: AppProject\nmetadata:\n  name: console\nspec:\n  destinations:\n    - server: https://kubernetes.default.svc\n      namespace: "*"\n  clusterResourceWhitelist:\n    - group: "*"\n      kind: "*"\n  namespaceResourceWhitelist:\n    - group: "*"\n      kind: "*"\n',
    [paths[2]]:
      "apiVersion: argoproj.io/v1alpha1\nkind: Application\nmetadata:\n  name: root\nspec:\n  source:\n    targetRevision: main\n",
    [paths[3]]: cardinalityText,
    [paths[4]]: `${JSON.stringify(authorization)}\n`,
  };
  for (const [path, text] of Object.entries(files)) {
    mkdirSync(join(root, dirname(path)), { recursive: true });
    writeFileSync(join(root, path), text);
  }
  git(root, "init", "-q");
  git(root, "config", "user.email", "test@example.invalid");
  git(root, "config", "user.name", "Test");
  git(root, "add", ".");
  git(root, "commit", "-qm", "fixture");
  return { root, sha: git(root, "rev-parse", "HEAD"), files };
}
function run(root, ...args) {
  const options =
    args[0] && typeof args[0] === "object" && !Buffer.isBuffer(args[0])
      ? args.shift()
      : {};
  return spawnSync(
    process.execPath,
    [join(root, "scripts/check-production-authority-blocked.mjs"), ...args],
    { cwd: options.cwd ?? tmpdir(), encoding: "buffer" },
  );
}
function runWithEnvironment(root, environment, ...args) {
  return runAtPath(root, root, environment, ...args);
}
function runAtPath(root, executableRoot, environment, ...args) {
  return spawnSync(
    process.execPath,
    [
      join(executableRoot, "scripts/check-production-authority-blocked.mjs"),
      ...args,
    ],
    {
      cwd: tmpdir(),
      encoding: "buffer",
      env: { ...process.env, ...environment },
    },
  );
}
function gitTrace(root) {
  const log = join(root, "git-trace.json");
  return { environment: { GIT_TRACE2_EVENT: log }, log };
}
function readGitTraceCalls(log) {
  const events = readFileSync(log, "utf8")
    .trim()
    .split(/\r?\n/u)
    .map((line) => JSON.parse(line));
  const repositories = new Map(
    events
      .filter((event) => event.event === "def_repo")
      .map((event) => [event.sid, realpathSync(event.worktree)]),
  );
  return events
    .filter((event) => event.event === "start")
    .map((event) => ({
      cwd: repositories.get(event.sid),
      args: event.argv.slice(1).join(" "),
    }));
}
function assertFailure(result, code) {
  assert.notEqual(result.status, 0);
  assert.deepEqual(result.stdout, Buffer.alloc(0));
  assert.equal(result.stderr.toString(), `ERROR ${code}\n`);
}
function commitFile(root, path, text, message = "mutation") {
  writeFileSync(join(root, path), text);
  git(root, "add", ".");
  git(root, "commit", "-qm", message);
  return git(root, "rev-parse", "HEAD");
}
function clone(value) {
  return JSON.parse(JSON.stringify(value));
}
function atPath(object, path) {
  return path.reduce((value, key) => value[key], object);
}
function replaceAtPath(object, path, value) {
  const copy = clone(object);
  const parent = atPath(copy, path.slice(0, -1));
  parent[path.at(-1)] = value;
  return copy;
}
function deleteAtPath(object, path) {
  const copy = clone(object);
  delete atPath(copy, path.slice(0, -1))[path.at(-1)];
  return copy;
}
function objectPaths(value, path = []) {
  if (!value || Array.isArray(value) || typeof value !== "object") return [];
  return [path, ...Object.entries(value).flatMap(([key, child]) => objectPaths(child, [...path, key]))];
}
function leafPaths(value, path = []) {
  if (!value || typeof value !== "object") return [path];
  if (Array.isArray(value)) return [path];
  return Object.entries(value).flatMap(([key, child]) => leafPaths(child, [...path, key]));
}
function wrongType(value) {
  if (Array.isArray(value)) return {};
  if (typeof value === "string") return 0;
  if (typeof value === "number") return "0";
  if (typeof value === "boolean") return "false";
  return null;
}
function wrongValue(value) {
  if (Array.isArray(value)) return ["not-empty"];
  if (typeof value === "string") return value === zero ? "a".repeat(40) : "WRONG_VALUE";
  if (typeof value === "number") return value + 1;
  if (typeof value === "boolean") return !value;
  return value;
}
function withCardinalityDigest(card, auth) {
  const rebound = clone(auth);
  rebound.production_cardinality_evidence.sha256 = createHash("sha256")
    .update(`${JSON.stringify(card)}\n`)
    .digest("hex");
  return rebound;
}
function evaluateFiles(files, updates = {}) {
  const raw = paths.map((path) => Buffer.from(updates[path] ?? files[path]));
  const calls = [];
  const commit = "a".repeat(40);
  const runGit = (args) => {
    calls.push(args);
    if (args[0] === "cat-file") return { status: 0, stdout: Buffer.from("commit\n") };
    return { status: 0, stdout: raw[paths.indexOf(args[1].slice(41))] };
  };
  return { calls, report: evaluate([commit], runGit) };
}
function assertEvaluateFailure(files, updates, code) {
  assert.throws(() => evaluateFiles(files, updates), (error) => error?.code === code);
}
function jsonWithDuplicate(value, targetPath, path = []) {
  if (Array.isArray(value))
    return `[${value.map((child, index) => jsonWithDuplicate(child, targetPath, [...path, index])).join(",")}]`;
  if (!value || typeof value !== "object") return JSON.stringify(value);
  const entries = Object.entries(value).map(([key, child]) =>
    `${JSON.stringify(key)}:${jsonWithDuplicate(child, targetPath, [...path, key])}`,
  );
  if (path.join("\u0000") === targetPath.join("\u0000")) {
    const [key, child] = Object.entries(value)[0];
    entries.push(`${JSON.stringify(key)}:${JSON.stringify(child)}`);
  }
  return `{${entries.join(",")}}`;
}
function walkJson(value, visit, path = []) {
  if (Array.isArray(value)) {
    visit(value, path);
    value.forEach((child, index) => walkJson(child, visit, [...path, index]));
    return;
  }
  if (value && typeof value === "object") {
    visit(value, path);
    Object.entries(value).forEach(([key, child]) => {
      visit(key, [...path, key]);
      walkJson(child, visit, [...path, key]);
    });
    return;
  }
  visit(value, path);
}

describe("production authority blocked observation CLI", () => {
  it("rejects bad argument forms with stable empty stdout", () => {
    const { root, sha } = fixture();
    assertFailure(run(root), "ARGUMENT_COUNT");
    assertFailure(run(root, sha, "extra"), "ARGUMENT_COUNT");
    assertFailure(run(root, "HEAD"), "COMMIT_SHA_FORMAT");
    assertFailure(run(root, zero), "COMMIT_SHA_ZERO");
  });
  it("evaluates from the canonical repository root when the executable path uses a filesystem alias", () => {
    const { root, sha } = fixture();
    const alias = `${root}-alias`;
    symlinkSync(root, alias, process.platform === "win32" ? "junction" : "dir");
    cleanup.push(alias);
    const { environment, log } = gitTrace(root);

    const result = runAtPath(root, alias, environment, sha);

    assert.equal(result.status, 0, result.stderr.toString());
    assert.equal(JSON.parse(result.stdout).evaluated_commit_sha, sha);
    const evaluatorRoot = realpathSync(root);
    assert.deepEqual(readGitTraceCalls(log), [
      { cwd: evaluatorRoot, args: `cat-file -t ${sha}` },
      ...paths.map((path) => ({
        cwd: evaluatorRoot,
        args: `show ${sha}:${path}`,
      })),
    ]);
  });
  it("rejects every non-exact SHA form before touching Git", () => {
    const { root } = fixture();
    for (const value of [
      "",
      "a".repeat(39),
      "a".repeat(41),
      "A".repeat(40),
      "g".repeat(40),
      "main",
      "refs/heads/main",
      "v1",
    ])
      assertFailure(run(root, value), "COMMIT_SHA_FORMAT");
    assertFailure(run(root, "a".repeat(40)), "GIT_OBJECT_UNAVAILABLE");
  });
  it("suppresses raw Git diagnostics and candidate object values on unavailable objects", () => {
    const { root } = fixture();
    const bin = join(root, "diagnostic-shim");
    mkdirSync(bin);
    writeFileSync(
      join(bin, "git"),
      "#!/bin/sh\nprintf 'RAW_GIT_SENTINEL_DO_NOT_LEAK\\n' >&2\nexit 23\n",
    );
    chmodSync(join(bin, "git"), 0o755);
    const candidate = "b".repeat(40);
    const result = runWithEnvironment(root, { PATH: `${bin}:${process.env.PATH}` }, candidate);
    assertFailure(result, "GIT_OBJECT_UNAVAILABLE");
    assert.equal(result.stderr.includes(Buffer.from("RAW_GIT_SENTINEL_DO_NOT_LEAK")), false);
    assert.equal(result.stderr.includes(Buffer.from(candidate)), false);
  });
  it("rejects valid blob, tree, and tag objects after the one type query", () => {
    const { root } = fixture();
    const blob = git(root, "rev-parse", `HEAD:${paths[0]}`);
    const tree = git(root, "rev-parse", "HEAD^{tree}");
    git(root, "tag", "-a", "v1", "-m", "fixture tag");
    const tag = git(root, "rev-parse", "v1^{tag}");
    for (const object of [blob, tree, tag])
      assertFailure(run(root, object), "GIT_OBJECT_NOT_COMMIT");
  });
  it("emits the exact compact blocked observation from the committed blobs", () => {
    const { root, sha, files } = fixture();
    const result = run(root, sha);
    assert.equal(result.status, 0, result.stderr.toString());
    assert.equal(result.stderr.toString(), "");
    const output = result.stdout.toString();
    assert.equal(output.endsWith("\n"), true);
    assert.equal(output.slice(0, -1).includes("\n"), false);
    const report = JSON.parse(output);
    assert.deepEqual(Object.keys(report), [
      "schema_version",
      "artifact_identity",
      "state",
      "activation_capable",
      "independence",
      "evaluated_commit_sha",
      "input_digests",
      "codes",
      "claim_limits",
    ]);
    assert.equal(report.schema_version, 1);
    assert.equal(report.artifact_identity, "repository_blocked_observation");
    assert.equal(report.independence, "I1_NON_INDEPENDENT");
    assert.equal(report.evaluated_commit_sha, sha);
    assert.equal(report.state, "BLOCKED");
    assert.equal(report.activation_capable, false);
    assert.deepEqual(report.codes, [
      "APP_PROJECT_WILDCARD_AUTHORITY",
      "CARDINALITY_TEMPLATE_NOT_EVIDENCE",
      "MAINTENANCE_MUTABLE_MAIN",
      "PR473_CUTOVER_AND_DEPLOYMENT_FALSE",
      "ROOT_MUTABLE_MAIN",
    ]);
    assert.deepEqual(
      report.input_digests.map((entry) => entry.path),
      [...paths].sort(),
    );
    assert.equal(report.input_digests.length, 5);
    assert.equal(new Set(report.input_digests.map((entry) => entry.path)).size, 5);
    for (const entry of report.input_digests)
      {
        assert.deepEqual(Object.keys(entry), ["path", "sha256"]);
        assert.match(entry.sha256, /^[0-9a-f]{64}$/);
      assert.equal(
        entry.sha256,
        createHash("sha256").update(files[entry.path]).digest("hex"),
      );
      }
    assert.deepEqual(report.claim_limits, [
      "ARCHITECTURE_ADOPTION_NOT_ESTABLISHED",
      "CNREL_CONFORMANCE_NOT_ESTABLISHED",
      "I3_INDEPENDENCE_NOT_ESTABLISHED",
      "LEGAL_CLEARANCE_NOT_ESTABLISHED",
      "PRODUCTION_ACTIVATION_NOT_AUTHORIZED",
      "PRODUCTION_AUTHORITY_NOT_ESTABLISHED",
      "PRODUCTION_READINESS_NOT_ESTABLISHED",
      "SAME_REPOSITORY_CI_NOT_BYPASS_RESISTANT",
    ]);
    const forbiddenKey = /time|host|user|url|repository|readiness|future|live|actor|reviewer|approver|authority|custody|legal/i;
    walkJson(report, (value, keyPath) => {
      if (typeof value === "string" && typeof keyPath.at(-1) === "string") {
        const key = keyPath.at(-1);
        assert.equal(forbiddenKey.test(key), false, `forbidden output key ${keyPath.join(".")}`);
        assert.equal(["READY", "PASS", "AUTHORIZED", "HOLD"].includes(value), false, `forbidden output value at ${keyPath.join(".")}`);
      }
    });
  });
  it("fails closed on escaped-equivalent duplicate JSON keys and a failed observation", () => {
    const { root } = fixture();
    const auth = join(root, paths[4]);
    const original = readFileSync(auth, "utf8");
    writeFileSync(
      auth,
      original.replace(
        '"target":"production"',
        '"target":"production","tar\\u0067et":"production"',
      ),
    );
    git(root, "add", ".");
    git(root, "commit", "-qm", "duplicate");
    assertFailure(
      run(root, git(root, "rev-parse", "HEAD")),
      "INPUT_DUPLICATE_KEY",
    );
    writeFileSync(
      auth,
      original.replace(
        '"deployment_authorized":false',
        '"deployment_authorized":true',
      ),
    );
    git(root, "add", ".");
    git(root, "commit", "-qm", "not established");
    assertFailure(
      run(root, git(root, "rev-parse", "HEAD")),
      "OBSERVATION_NOT_ESTABLISHED",
    );
  });
  it("keeps committed evaluation deterministic across cwd, locale, dirty and deleted working trees", () => {
    const { root, sha, files } = fixture();
    const cwdOne = mkdtempSync(join(tmpdir(), "blocked-observation-cwd-one-"));
    const cwdTwo = mkdtempSync(join(tmpdir(), "blocked-observation-cwd-two-"));
    cleanup.push(cwdOne, cwdTwo);
    const first = run(root, { cwd: cwdOne }, sha);
    const secondCwd = run(root, { cwd: cwdTwo }, sha);
    const second = runWithEnvironment(
      root,
      { LC_ALL: "C", TZ: "Pacific/Kiritimati", BENIGN: "value" },
      sha,
    );
    const third = runWithEnvironment(
      root,
      { LC_ALL: "en_US.UTF-8", TZ: "UTC", HOSTNAME: "sentinel-host", USER: "sentinel-user", BENIGN: "sentinel-value" },
      sha,
    );
    assert.deepEqual(secondCwd.stdout, first.stdout);
    assert.deepEqual(second.stdout, first.stdout);
    assert.deepEqual(third.stdout, first.stdout);
    assert.equal(first.stdout.includes(Buffer.from("sentinel-")), false);
    const commented = commitFile(
      root,
      paths[0],
      `${files[paths[0]]}# formatting-only comment\n`,
      "yaml comment only",
    );
    const before = JSON.parse(first.stdout);
    const afterResult = run(root, { cwd: cwdTwo }, commented);
    assert.equal(afterResult.status, 0, afterResult.stderr.toString());
    const after = JSON.parse(afterResult.stdout);
    assert.deepEqual(after.codes, before.codes);
    assert.deepEqual(after.claim_limits, before.claim_limits);
    assert.notEqual(after.evaluated_commit_sha, before.evaluated_commit_sha);
    for (const path of paths) {
      const oldDigest = before.input_digests.find((entry) => entry.path === path).sha256;
      const newDigest = after.input_digests.find((entry) => entry.path === path).sha256;
      assert.equal(newDigest === oldDigest, path !== paths[0], path);
    }
    for (const path of paths)
      writeFileSync(join(root, path), `${files[path]}# dirty\n`);
    writeFileSync(join(root, "untracked-sentinel"), "not an input\n");
    assert.deepEqual(run(root, sha).stdout, first.stdout);
    for (const path of paths) rmSync(join(root, path));
    assert.deepEqual(run(root, sha).stdout, first.stdout);
  });
  it("uses exactly one object query and five fixed-order blob reads from the evaluator repository root", () => {
    const { root, sha } = fixture();
    const { environment, log } = gitTrace(root);
    const result = runWithEnvironment(root, environment, sha);
    assert.equal(result.status, 0, result.stderr.toString());
    const evaluatorRoot = realpathSync(root);
    assert.deepEqual(readGitTraceCalls(log), [
      { cwd: evaluatorRoot, args: `cat-file -t ${sha}` },
      ...paths.map((path) => ({
        cwd: evaluatorRoot,
        args: `show ${sha}:${path}`,
      })),
    ]);
  });
  it("attempts every fixed blob read before reporting a missing input", () => {
    const { root, sha } = fixture();
    const bin = join(root, "shim-bin"),
      log = join(root, "git.log");
    mkdirSync(bin);
    const realGit = spawnSync("which", ["git"], {
      encoding: "utf8",
    }).stdout.trim();
    writeFileSync(
      join(bin, "git"),
      `#!/bin/sh\nprintf '%s\\n' "$*" >> "$GIT_LOG"\nexec "${realGit}" "$@"\n`,
    );
    chmodSync(join(bin, "git"), 0o755);
    git(root, "rm", paths[4]);
    git(root, "commit", "-qm", "missing blob");
    const missing = git(root, "rev-parse", "HEAD");
    assertFailure(
      runWithEnvironment(
        root,
        { PATH: `${bin}:${process.env.PATH}`, GIT_LOG: log },
        missing,
      ),
      "INPUT_BLOB_UNAVAILABLE",
    );
    assert.deepEqual(readFileSync(log, "utf8").trim().split("\n"), [
      `cat-file -t ${missing}`,
      ...paths.map((path) => `show ${missing}:${path}`),
    ]);
    assert.notEqual(missing, sha);
  });
  it("enforces JSON stage precedence and exact nested schema/value observations", () => {
    const cases = [
      [paths[3], "{", "INPUT_JSON_MALFORMED"],
      [paths[3], "[]", "INPUT_SCHEMA_MISMATCH"],
      [
        paths[3],
        (text) =>
          text.replace(
            '"target":"production"',
            '"target":"production","tar\\u0067et":"production"',
          ),
        "INPUT_DUPLICATE_KEY",
      ],
      [
        paths[3],
        (text) =>
          text.replace(
            '"schema_version":1',
            '"schema_version":1,"unknown":false',
          ),
        "INPUT_UNKNOWN_KEY",
      ],
      [
        paths[3],
        (text) =>
          text.replace('"reader_endpoint":"TEMPLATE_NOT_EVIDENCE",', ""),
        "INPUT_SCHEMA_MISMATCH",
      ],
      [
        paths[4],
        (text) =>
          text.replace(
            '"deployment_authorized":false',
            '"deployment_authorized":true',
          ),
        "OBSERVATION_NOT_ESTABLISHED",
      ],
      [
        paths[3],
        (text) =>
          text.replace(
            '"candidate_source_sha":"0000000000000000000000000000000000000000"',
            '"candidate_source_sha":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"',
          ),
        "OBSERVATION_NOT_ESTABLISHED",
      ],
    ];
    for (const [path, mutation, expected] of cases) {
      const { root, files } = fixture();
      const text =
        typeof mutation === "function" ? mutation(files[path]) : mutation;
      const sha = commitFile(root, path, text);
      assertFailure(run(root, sha), expected);
    }
  });
  it("rejects consumed YAML ambiguity and nonmatching required wildcard shapes", () => {
    const cases = [
      [
        paths[2],
        "targetRevision: main",
        "targetRevision: [main]",
        "INPUT_YAML_AMBIGUOUS",
      ],
      [
        paths[2],
        "targetRevision: main",
        "targetRevision: stable",
        "OBSERVATION_NOT_ESTABLISHED",
      ],
      [
        paths[1],
        "    - server: https://kubernetes.default.svc",
        '    - { server: https://kubernetes.default.svc, namespace: "*" }',
        "INPUT_YAML_AMBIGUOUS",
      ],
      [
        paths[1],
        '      namespace: "*"',
        "      namespace: console",
        "OBSERVATION_NOT_ESTABLISHED",
      ],
      [
        paths[0],
        "apiVersion: argoproj.io/v1alpha1",
        "apiVersion: argoproj.io/v1alpha1\n---",
        "INPUT_YAML_AMBIGUOUS",
      ],
      [
        paths[0],
        "targetRevision: main",
        "targetRevision: !main main",
        "INPUT_YAML_AMBIGUOUS",
      ],
    ];
    for (const [path, from, to, expected] of cases) {
      const { root, files } = fixture();
      const sha = commitFile(root, path, files[path].replace(from, to));
      assertFailure(run(root, sha), expected);
    }
  });
  it("rejects compact project item continuation members that do not align with the first key", () => {
    const { root, files } = fixture();
    const misaligned = files[paths[1]].replace(
      '      namespace: "*"',
      '       namespace: "*"',
    );
    assertFailure(
      run(root, commitFile(root, paths[1], misaligned)),
      "INPUT_YAML_AMBIGUOUS",
    );
  });
  it("ignores unconsumed comments, flow values, anchors, aliases, and duplicate keys", () => {
    const { root, files } = fixture();
    const extra = `\n# harmless comment\nunconsumed: &shared { values: [one, two] }\nunconsumed: *shared\n`;
    const sha = commitFile(root, paths[0], `${files[paths[0]]}${extra}`);
    const result = run(root, sha);
    assert.equal(result.status, 0, result.stderr.toString());
  });
  it("gives recursive unknown keys global precedence over missing cardinality keys", () => {
    const { root, files } = fixture();
    const text = files[paths[3]]
      .replace('"schema_version":1,', "")
      .replace(
        '"trust_domain_id":"TEMPLATE_NOT_EVIDENCE"',
        '"trust_domain_id":"TEMPLATE_NOT_EVIDENCE","extra":false',
      );
    assertFailure(
      run(root, commitFile(root, paths[3], text)),
      "INPUT_UNKNOWN_KEY",
    );
  });
  it("rejects consumed metadata aliases and accepts reordered, consistently-indented project maps", () => {
    {
      const { root, files } = fixture();
      const text = files[paths[0]].replace(
        "  name: console",
        "  <<: *defaults\n  name: console",
      );
      assertFailure(
        run(root, commitFile(root, paths[0], text)),
        "INPUT_YAML_AMBIGUOUS",
      );
    }
    {
      const { root, files } = fixture();
      const text = files[paths[1]]
        .replace(
          '    - server: https://kubernetes.default.svc\n      namespace: "*"',
          '      - namespace: "*"\n        server: https://kubernetes.default.svc',
        )
        .replace(
          /^  (destinations|clusterResourceWhitelist|namespaceResourceWhitelist):/gm,
          "    $1:",
        )
        .replace(/^    - (group|kind):/gm, "      - $1:")
        .replace(/^      (kind|namespace):/gm, "        $1:");
      assert.equal(run(root, commitFile(root, paths[1], text)).status, 0);
    }
  });
  it("accepts the actual committed HEAD blobs rather than only minimal fixtures", () => {
    const root = dirname(dirname(script)),
      sha = git(root, "rev-parse", "HEAD");
    const result = spawnSync(process.execPath, [script, sha], {
      cwd: tmpdir(),
      encoding: "buffer",
    });
    assert.equal(result.status, 0, result.stderr.toString());
    assert.equal(JSON.parse(result.stdout).evaluated_commit_sha, sha);
  });
  it("keeps all consumed YAML ambiguity ahead of every observation nonmatch", () => {
    const cases = [
      [
        paths[0],
        (files) =>
          files[paths[0]].replace(
            "targetRevision: main",
            "targetRevision: stable",
          ),
        paths[1],
        (files) =>
          files[paths[1]].replace(
            "kind: AppProject",
            "kind: AppProject\n<<: *merge",
          ),
      ],
      [
        paths[1],
        (files) =>
          files[paths[1]].replace("metadata:\n", "metadata:\n  <<: *merge\n"),
        null,
        null,
      ],
      [
        paths[1],
        (files) => files[paths[1]].replace("spec:\n", "spec:\n  <<: *merge\n"),
        null,
        null,
      ],
      [
        paths[1],
        (files) =>
          files[paths[1]].replace(
            "- server: https://kubernetes.default.svc",
            "- <<: *merge\n      server: https://kubernetes.default.svc",
          ),
        null,
        null,
      ],
      [
        paths[2],
        (files) =>
          files[paths[2]].replace(
            "targetRevision: main",
            "targetRevision: main\n      invalid: sibling",
          ),
        null,
        null,
      ],
    ];
    for (const [
      firstPath,
      firstMutation,
      secondPath,
      secondMutation,
    ] of cases) {
      const { root, files } = fixture();
      writeFileSync(join(root, firstPath), firstMutation(files));
      if (secondPath)
        writeFileSync(join(root, secondPath), secondMutation(files));
      git(root, "add", ".");
      git(root, "commit", "-qm", "yaml ambiguity");
      assertFailure(
        run(root, git(root, "rev-parse", "HEAD")),
        "INPUT_YAML_AMBIGUOUS",
      );
    }
  });
  it("treats project list placement and cardinality as nonmatches while rejecting block scalars", () => {
    const mutations = [
      (text) =>
        text
          .replace("  destinations:", "  wrapper:\n    destinations:")
          .replace(/^    - /gm, "      - ")
          .replace(/^      (namespace|kind):/gm, "        $1:"),
      (text) =>
        text.replace(
          '    - server: https://kubernetes.default.svc\n      namespace: "*"',
          "",
        ),
      (text) =>
        text.replace(
          "  destinations:\n",
          '  destinations:\n    - server: https://kubernetes.default.svc\n      namespace: "*"\n',
        ),
    ];
    for (const mutate of mutations) {
      const { root, files } = fixture();
      assertFailure(
        run(root, commitFile(root, paths[1], mutate(files[paths[1]]))),
        "OBSERVATION_NOT_ESTABLISHED",
      );
    }
    {
      const { root, files } = fixture();
      assertFailure(
        run(
          root,
          commitFile(
            root,
            paths[2],
            files[paths[2]].replace(
              "targetRevision: main",
              "targetRevision: |\n      main",
            ),
          ),
        ),
        "INPUT_YAML_AMBIGUOUS",
      );
    }
  });
  it("reports fatal UTF-8 JSON as JSON malformed", () => {
    const { root } = fixture();
    writeFileSync(join(root, paths[3]), Buffer.from([0xc3, 0x28]));
    git(root, "add", ".");
    git(root, "commit", "-qm", "invalid utf8");
    assertFailure(
      run(root, git(root, "rev-parse", "HEAD")),
      "INPUT_JSON_MALFORMED",
    );
  });
  it("keeps own-key JSON unknown precedence for inherited-looking names", () => {
    const cases = [
      [
        paths[3],
        '"target":"production"',
        '"target":"production","toString":false',
      ],
      [paths[4], '"verified":false', '"verified":false,"constructor":false'],
      [paths[4], '"verified":false', '"verified":false,"__proto__":false'],
    ];
    for (const [path, from, to] of cases) {
      const { root, files } = fixture();
      assertFailure(
        run(root, commitFile(root, path, files[path].replace(from, to))),
        "INPUT_UNKNOWN_KEY",
      );
    }
  });
  it("handles deeply nested valid unknown JSON without leaking an internal error", () => {
    const { root, files } = fixture();
    const nested = "[".repeat(15_000) + "]".repeat(15_000);
    const mutated = files[paths[3]].replace(
      '"target":"production"',
      `"target":"production","unknown":${nested}`,
    );
    assertFailure(
      run(root, commitFile(root, paths[3], mutated)),
      "INPUT_UNKNOWN_KEY",
    );
  });
  it("accepts only column-zero document markers in their permitted positions", () => {
    const cases = [
      ["---\n", "", undefined],
      ["", "...\n", undefined],
      ["  ---\n", "", "INPUT_YAML_AMBIGUOUS"],
      ["", "  ...\n", "INPUT_YAML_AMBIGUOUS"],
      ["---\n---\n", "", "INPUT_YAML_AMBIGUOUS"],
      ["", "...\nkind: Application\n", "INPUT_YAML_AMBIGUOUS"],
    ];
    for (const [prefix, suffix, expected] of cases) {
      const { root, files } = fixture();
      const result = run(
        root,
        commitFile(root, paths[0], `${prefix}${files[paths[0]]}${suffix}`),
      );
      if (expected) assertFailure(result, expected);
      else assert.equal(result.status, 0, result.stderr.toString());
    }
  });
  it("rejects direct route merge keys but permits nested opaque special syntax", () => {
    const direct = [
      [paths[0], "apiVersion:", "<<: *defaults\napiVersion:"],
      [paths[0], "  name: console", "  <<: *defaults\n  name: console"],
      [paths[0], "  source:", "  <<: *defaults\n  source:"],
      [
        paths[0],
        "    targetRevision: main",
        "    <<: *defaults\n    targetRevision: main",
      ],
      [paths[1], "    - server:", "    - <<: *defaults\n      server:"],
    ];
    for (const [path, from, to] of direct) {
      const { root, files } = fixture();
      assertFailure(
        run(root, commitFile(root, path, files[path].replace(from, to))),
        "INPUT_YAML_AMBIGUOUS",
      );
    }
    const { root, files } = fixture();
    const opaque = `${files[paths[0]]}unconsumed:\n  nested: &anchor { flow: [*anchor, {<<: *anchor}] }\n`;
    assert.equal(run(root, commitFile(root, paths[0], opaque)).status, 0);
  });
  it("evaluates the fixed object and blob topology without any extra Git operation", () => {
    const { files } = fixture();
    const { calls, report } = evaluateFiles(files);
    assert.equal(report.evaluated_commit_sha, "a".repeat(40));
    assert.deepEqual(calls, [
      ["cat-file", "-t", "a".repeat(40)],
      ...paths.map((path) => ["show", `${"a".repeat(40)}:${path}`]),
    ]);
  });
  it("rejects unknown, missing, type-wrong, decoded-duplicate, and wrong settled JSON values at every semantic path", () => {
    const { files } = fixture();
    const documents = [
      [paths[3], JSON.parse(files[paths[3]]), "card"],
      [paths[4], JSON.parse(files[paths[4]]), "auth"],
    ];
    for (const [path, document, label] of documents) {
      for (const objectPath of objectPaths(document)) {
        const unknown = clone(document);
        atPath(unknown, objectPath).__inherited_like = false;
        assertEvaluateFailure(files, { [path]: `${JSON.stringify(unknown)}\n` }, "INPUT_UNKNOWN_KEY");
        const duplicate = jsonWithDuplicate(document, objectPath);
        assertEvaluateFailure(files, { [path]: `${duplicate}\n` }, "INPUT_DUPLICATE_KEY");
      }
      for (const leafPath of leafPaths(document)) {
        const original = atPath(document, leafPath);
        assertEvaluateFailure(files, { [path]: `${JSON.stringify(deleteAtPath(document, leafPath))}\n` }, "INPUT_SCHEMA_MISMATCH");
        assertEvaluateFailure(files, { [path]: `${JSON.stringify(replaceAtPath(document, leafPath, wrongType(original)))}\n` }, "INPUT_SCHEMA_MISMATCH");
        let changed = replaceAtPath(document, leafPath, wrongValue(original));
        const updates = { [path]: `${JSON.stringify(changed)}\n` };
        if (label === "card") {
          const auth = withCardinalityDigest(changed, JSON.parse(files[paths[4]]));
          updates[paths[4]] = `${JSON.stringify(auth)}\n`;
        }
        assertEvaluateFailure(files, updates, "OBSERVATION_NOT_ESTABLISHED");
      }
    }
  });
  it("preserves JSON precedence across both documents and later YAML inputs", () => {
    const { files } = fixture();
    const invalid = { [paths[3]]: "{", [paths[4]]: "{", [paths[2]]: "kind: [Application]\n" };
    assertEvaluateFailure(files, invalid, "INPUT_JSON_MALFORMED");
    const duplicate = jsonWithDuplicate(JSON.parse(files[paths[4]]), []);
    assertEvaluateFailure(files, { [paths[3]]: "{", [paths[4]]: `${duplicate}\n` }, "INPUT_JSON_MALFORMED");
    assertEvaluateFailure(files, { [paths[4]]: `${duplicate}\n`, [paths[3]]: `${JSON.stringify({ ...JSON.parse(files[paths[3]]), unknown: false })}\n` }, "INPUT_DUPLICATE_KEY");
    assertEvaluateFailure(files, { [paths[3]]: `${JSON.stringify({ ...JSON.parse(files[paths[3]]), unknown: false })}\n`, [paths[1]]: "kind: [AppProject]\n" }, "INPUT_UNKNOWN_KEY");
    assertEvaluateFailure(files, { [paths[3]]: "[]\n", [paths[1]]: "kind: [AppProject]\n" }, "INPUT_SCHEMA_MISMATCH");
    assertEvaluateFailure(files, { [paths[0]]: files[paths[0]].replace("targetRevision: main", "targetRevision: stable"), [paths[2]]: "kind: [Application]\n" }, "INPUT_YAML_AMBIGUOUS");
  });
  it("binds formatting-only commit changes to raw Git blob digests while retaining the fixed observation", () => {
    const { root, sha, files } = fixture();
    const changed = files[paths[3]].replace("{\"schema_version\"", "{\n  \"schema_version\"");
    const auth = clone(JSON.parse(files[paths[4]]));
    auth.production_cardinality_evidence.sha256 = createHash("sha256")
      .update(changed)
      .digest("hex");
    writeFileSync(join(root, paths[3]), changed);
    writeFileSync(join(root, paths[4]), `${JSON.stringify(auth, null, 2)}\n`);
    git(root, "add", ".");
    git(root, "commit", "-qm", "format only");
    const next = git(root, "rev-parse", "HEAD");
    const before = JSON.parse(run(root, sha).stdout);
    const after = JSON.parse(run(root, next).stdout);
    assert.deepEqual(after.codes, before.codes);
    assert.notEqual(after.evaluated_commit_sha, before.evaluated_commit_sha);
    for (const path of paths) {
      const committed = spawnSync("git", ["show", `${next}:${path}`], {
        cwd: root,
        encoding: "buffer",
      }).stdout;
      const entry = after.input_digests.find((value) => value.path === path);
      assert.equal(entry.sha256, createHash("sha256").update(committed).digest("hex"), path);
    }
    assert.notEqual(
      after.input_digests.find((value) => value.path === paths[3]).sha256,
      before.input_digests.find((value) => value.path === paths[3]).sha256,
    );
  });
  it("covers every compact AppProject list cardinality and continuation-column contract", () => {
    const { files } = fixture();
    const lists = ["destinations", "clusterResourceWhitelist", "namespaceResourceWhitelist"];
    for (const list of lists) {
      const without = files[paths[1]].replace(new RegExp(`  ${list}:\\n(?:    - .*\\n      .*\\n)?`), "");
      assertEvaluateFailure(files, { [paths[1]]: without }, "OBSERVATION_NOT_ESTABLISHED");
      const empty = files[paths[1]].replace(new RegExp(`(  ${list}:)\\n    - .*\\n      .*\\n`), "$1\n");
      assertEvaluateFailure(files, { [paths[1]]: empty }, "OBSERVATION_NOT_ESTABLISHED");
      const extra = files[paths[1]].replace(new RegExp(`(  ${list}:\\n    - .*\\n      .*\\n)`), "$1    - group: extra\n      kind: extra\n");
      assertEvaluateFailure(files, { [paths[1]]: extra }, "OBSERVATION_NOT_ESTABLISHED");
    }
    for (const continuation of ['      namespace: "*"', '      kind: "*"']) {
      const misaligned = files[paths[1]].replace(continuation, ` ${continuation}`);
      assertEvaluateFailure(files, { [paths[1]]: misaligned }, "INPUT_YAML_AMBIGUOUS");
    }
    const duplicateMember = files[paths[1]].replace('      namespace: "*"', '      namespace: "*"\n      namespace: "*"');
    assertEvaluateFailure(files, { [paths[1]]: duplicateMember }, "INPUT_YAML_AMBIGUOUS");
  });
  it("rejects consumed Application route aliases, tags, flow values, duplicate keys, and invalid map indentation", () => {
    const { files } = fixture();
    const cases = [
      [paths[0], "apiVersion: argoproj.io/v1alpha1", "apiVersion: &v argoproj.io/v1alpha1"],
      [paths[0], "kind: Application", "kind: !Application Application"],
      [paths[0], "  name: console", "  name: [console]"],
      [paths[0], "  name: console", "  name: console\n  name: console"],
      [paths[0], "metadata:", " metadata:"],
      [paths[2], "targetRevision: main", "targetRevision: *main"],
      [paths[2], "targetRevision: main", "targetRevision: >\n      main"],
    ];
    for (const [path, from, to] of cases) {
      assertEvaluateFailure(files, { [path]: files[path].replace(from, to) }, "INPUT_YAML_AMBIGUOUS");
    }
  });
  it("classifies every root and console targetRevision route mutation", () => {
    const { files } = fixture();
    for (const [path, name] of [[paths[0], "console"], [paths[2], "root"]]) {
      const source = files[path];
      const cases = [
        [`${name} missing`, source.replace("    targetRevision: main\n", ""), "OBSERVATION_NOT_ESTABLISHED"],
        [`${name} duplicate`, source.replace("    targetRevision: main", "    targetRevision: main\n    targetRevision: main"), "INPUT_YAML_AMBIGUOUS"],
        [`${name} relocated`, source.replace("    targetRevision: main", "    wrapper:\n      targetRevision: main"), "OBSERVATION_NOT_ESTABLISHED"],
        [`${name} wrong value`, source.replace("targetRevision: main", "targetRevision: stable"), "OBSERVATION_NOT_ESTABLISHED"],
        [`${name} wrong node`, source.replace("targetRevision: main", "targetRevision:\n      child: main"), "INPUT_YAML_AMBIGUOUS"],
      ];
      for (const [label, text, code] of cases)
        assertEvaluateFailure(files, { [path]: text }, code, label);
    }
  });
  it("classifies group and kind mutations in both AppProject wildcard lists", () => {
    const { files } = fixture();
    for (const list of ["clusterResourceWhitelist", "namespaceResourceWhitelist"]) {
      const block = `  ${list}:\n    - group: "*"\n      kind: "*"`;
      const cases = [
        [`${list} missing group`, files[paths[1]].replace(block, `  ${list}:\n    - kind: "*"`), "OBSERVATION_NOT_ESTABLISHED"],
        [`${list} missing kind`, files[paths[1]].replace(block, `  ${list}:\n    - group: "*"`), "OBSERVATION_NOT_ESTABLISHED"],
        [`${list} narrowed group`, files[paths[1]].replace(block, `  ${list}:\n    - group: "apps"\n      kind: "*"`), "OBSERVATION_NOT_ESTABLISHED"],
        [`${list} narrowed kind`, files[paths[1]].replace(block, `  ${list}:\n    - group: "*"\n      kind: "Deployment"`), "OBSERVATION_NOT_ESTABLISHED"],
        [`${list} extra member`, files[paths[1]].replace(block, `${block}\n      extra: value`), "OBSERVATION_NOT_ESTABLISHED"],
        [`${list} duplicate group`, files[paths[1]].replace(block, `${block}\n      group: "*"`), "INPUT_YAML_AMBIGUOUS"],
        [`${list} duplicate kind`, files[paths[1]].replace(block, `${block}\n      kind: "*"`), "INPUT_YAML_AMBIGUOUS"],
      ];
      for (const [label, text, code] of cases)
        assertEvaluateFailure(files, { [paths[1]]: text }, code, label);
    }
  });
  it("rejects empty consumed AppProject list scalar members as YAML ambiguity", () => {
    const { files } = fixture();
    const cases = [
      ["destination server", "server: https://kubernetes.default.svc", "server:"],
      ["destination namespace", 'namespace: "*"', "namespace:"],
      ["cluster group", '  clusterResourceWhitelist:\n    - group: "*"', "  clusterResourceWhitelist:\n    - group:"],
      ["cluster kind", '  clusterResourceWhitelist:\n    - group: "*"\n      kind: "*"', '  clusterResourceWhitelist:\n    - group: "*"\n      kind:'],
      ["namespace group", '  namespaceResourceWhitelist:\n    - group: "*"', "  namespaceResourceWhitelist:\n    - group:"],
      ["namespace kind", '  namespaceResourceWhitelist:\n    - group: "*"\n      kind: "*"', '  namespaceResourceWhitelist:\n    - group: "*"\n      kind:'],
    ];
    for (const [label, from, to] of cases)
      assertEvaluateFailure(
        files,
        { [paths[1]]: files[paths[1]].replace(from, to) },
        "INPUT_YAML_AMBIGUOUS",
        label,
      );
  });
});
