import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import {
  chmodSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  readdirSync,
  rmSync,
  symlinkSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { after, describe, it } from "node:test";

import yaml from "js-yaml";

const workflowPath = new URL(
  "../.github/workflows/image-release.yml",
  import.meta.url,
);
const workflowText = readFileSync(workflowPath, "utf8");
const workflow = yaml.load(workflowText);
const admission = workflow.jobs["ci-admission"];
const admissionStep = admission.steps.find((step) => step.id === "admit");
const ciWorkflowPath = new URL("../.github/workflows/ci.yml", import.meta.url);
const ciWorkflow = yaml.load(readFileSync(ciWorkflowPath, "utf8"));
const workflowsDirectory = new URL("../.github/workflows/", import.meta.url);
const cacheHygienePath = new URL(
  "../.github/workflows/cache-hygiene.yml",
  import.meta.url,
);
const cacheHygieneText = readFileSync(cacheHygienePath, "utf8");
const cacheHygiene = yaml.load(cacheHygieneText);

const CANDIDATE = "a".repeat(40);
const PARENT = "b".repeat(40);
const OTHER = "c".repeat(40);
const RELEASE_HEAD = "d".repeat(40);
const RELEASE_TREE = "e".repeat(40);
const REPOSITORY_ID = 1269693002;
const CI_WORKFLOW_ID = 296023727;
const RELEASE_WORKFLOW_ID = 296023729;
const SECURITY_WORKFLOW_ID = 296023731;
const CI_RUN_ID = 1101;
const RELEASE_RUN_ID = 1701;
const RELEASE_RUN_NUMBER = 701;
const RELEASE_PROOF_JOB_ID = 1702;
const RELEASE_PR_NUMBER = 760;
const SECURITY_RUN_ID = 2201;
const RELEASE_TRANSPORT_ID = 56489493;
const RELEASE_TRANSPORT_LOGIN = "jason931225";
const RELEASE_TRANSPORT_NAME = "Jason Lee";
const RELEASE_TRANSPORT_EMAIL =
  "56489493+jason931225@users.noreply.github.com";

const temporaryRoots = [];
after(() => {
  for (const root of temporaryRoots) {
    rmSync(root, { recursive: true, force: true });
  }
});

function canonical(value) {
  if (Array.isArray(value)) return value.map(canonical);
  if (value && typeof value === "object") {
    return Object.fromEntries(
      Object.entries(value)
        .sort(([left], [right]) => left.localeCompare(right))
        .map(([key, nested]) => [key, canonical(nested)]),
    );
  }
  return value;
}

function productionJobDigest(model = workflow) {
  const protectedJobs = {
    "production-promotion-preflight":
      model.jobs["production-promotion-preflight"],
    "bump-digests": model.jobs["bump-digests"],
  };
  return createHash("sha256")
    .update(JSON.stringify(canonical(protectedJobs)))
    .digest("hex");
}

function releaseFiles() {
  return [
    { filename: ".release-please-manifest.json", status: "modified" },
    { filename: "CHANGELOG.md", status: "modified" },
    { filename: "docs/documentation-index.json", status: "modified" },
    {
      filename: "docs/documentation-manifest.seed.json",
      status: "modified",
    },
  ];
}

function releaseCommit() {
  return {
    sha: CANDIDATE,
    parents: [{ sha: PARENT }],
    commit: {
      message: `chore(dev): release 1.2.3 (#${RELEASE_PR_NUMBER})\n\nrelease notes`,
      tree: { sha: RELEASE_TREE },
      author: {
        name: "github-actions[bot]",
        email: "41898282+github-actions[bot]@users.noreply.github.com",
      },
      committer: { name: "GitHub", email: "noreply@github.com" },
      verification: { verified: true, reason: "valid" },
    },
    author: { login: "github-actions[bot]", id: 41898282, type: "Bot" },
    committer: { login: "web-flow", id: 19864447, type: "User" },
    files: releaseFiles(),
  };
}

function transportAuthoredReleaseCommit() {
  const candidate = releaseCommit();
  return {
    ...candidate,
    commit: {
      ...candidate.commit,
      author: {
        name: RELEASE_TRANSPORT_NAME,
        email: RELEASE_TRANSPORT_EMAIL,
      },
    },
    author: {
      login: RELEASE_TRANSPORT_LOGIN,
      id: RELEASE_TRANSPORT_ID,
      type: "User",
    },
  };
}

function releaseHeadCommit() {
  return {
    sha: RELEASE_HEAD,
    parents: [{ sha: PARENT }],
    commit: {
      message: "chore(dev): release 1.2.3",
      tree: { sha: RELEASE_TREE },
      author: {
        name: "github-actions[bot]",
        email: "41898282+github-actions[bot]@users.noreply.github.com",
      },
      committer: { name: "GitHub", email: "noreply@github.com" },
      verification: { verified: false, reason: "unsigned" },
    },
    author: { login: "github-actions[bot]", id: 41898282, type: "Bot" },
    committer: { login: "web-flow", id: 19864447, type: "User" },
    files: releaseFiles(),
  };
}

function releasePullRequest({ transport = false } = {}) {
  const user = transport
    ? { id: RELEASE_TRANSPORT_ID, login: RELEASE_TRANSPORT_LOGIN, type: "User" }
    : { id: 41898282, login: "github-actions[bot]", type: "Bot" };
  return {
    number: RELEASE_PR_NUMBER,
    state: "closed",
    draft: false,
    merged: true,
    merged_at: "2026-08-16T12:00:00Z",
    merge_commit_sha: CANDIDATE,
    title: "chore(dev): release 1.2.3",
    user,
    base: {
      ref: "dev",
      sha: PARENT,
      repo: { id: REPOSITORY_ID, full_name: "oyatie/console" },
    },
    head: {
      ref: "release-please--branches--dev--components--console",
      sha: RELEASE_HEAD,
      repo: { id: REPOSITORY_ID, full_name: "oyatie/console" },
    },
  };
}

function workflowRun({
  id,
  workflowId,
  path,
  headSha = CANDIDATE,
  runNumber = id,
  runAttempt = 1,
  status = "completed",
  conclusion = "success",
}) {
  return {
    id,
    workflow_id: workflowId,
    path,
    run_number: runNumber,
    run_attempt: runAttempt,
    event: "push",
    head_branch: "dev",
    head_sha: headSha,
    status,
    conclusion,
    repository: { id: REPOSITORY_ID, full_name: "oyatie/console" },
  };
}

function releaseProofJobs(overrides = {}) {
  return {
    total_count: 1,
    jobs: [
      {
        id: RELEASE_PROOF_JOB_ID,
        run_id: RELEASE_RUN_ID,
        run_attempt: 1,
        workflow_name: "Release Please",
        head_sha: PARENT,
        name: `release-authority-proof pr=${RELEASE_PR_NUMBER} head=${RELEASE_HEAD}`,
        status: "completed",
        conclusion: "success",
        ...overrides,
      },
    ],
  };
}

function aggregateJobs(name, conclusion = "success") {
  return {
    total_count: 1,
    jobs: [{ name, status: "completed", conclusion }],
  };
}

function publishedRelease(overrides = {}) {
  return {
    id: 3301,
    tag_name: "v1.2.3",
    target_commitish: CANDIDATE,
    draft: false,
    prerelease: false,
    immutable: true,
    published_at: "2026-08-16T12:00:00Z",
    author: { login: "github-actions[bot]", id: 41898282, type: "Bot" },
    ...overrides,
  };
}

function baseRoutes() {
  const ciRun = workflowRun({
    id: CI_RUN_ID,
    workflowId: CI_WORKFLOW_ID,
    path: ".github/workflows/ci.yml",
  });
  const securityRun = workflowRun({
    id: SECURITY_RUN_ID,
    workflowId: SECURITY_WORKFLOW_ID,
    path: ".github/workflows/security.yml",
  });
  const releaseRun = workflowRun({
    id: RELEASE_RUN_ID,
    workflowId: RELEASE_WORKFLOW_ID,
    path: ".github/workflows/release-please.yml",
    headSha: PARENT,
    runNumber: RELEASE_RUN_NUMBER,
  });
  return {
    [`repos/oyatie/console/actions/runs/${CI_RUN_ID}`]: [ciRun],
    [`repos/oyatie/console/actions/workflows/${CI_WORKFLOW_ID}/runs?event=push&branch=dev&head_sha=${CANDIDATE}&per_page=100`]: [
      { total_count: 1, workflow_runs: [ciRun] },
    ],
    [`repos/oyatie/console/actions/runs/${CI_RUN_ID}/attempts/1/jobs?per_page=100`]: [
      aggregateJobs("Required / CI"),
    ],
    [`repos/oyatie/console/actions/workflows/${SECURITY_WORKFLOW_ID}/runs?event=push&branch=dev&head_sha=${CANDIDATE}&per_page=100`]: [
      { total_count: 1, workflow_runs: [securityRun] },
    ],
    [`repos/oyatie/console/actions/runs/${SECURITY_RUN_ID}/attempts/1/jobs?per_page=100`]: [
      aggregateJobs("Required / Security"),
    ],
    [`repos/oyatie/console/actions/workflows/${RELEASE_WORKFLOW_ID}/runs?event=push&branch=dev&head_sha=${PARENT}&per_page=100`]: [
      { total_count: 1, workflow_runs: [releaseRun] },
    ],
    [`repos/oyatie/console/actions/runs/${RELEASE_RUN_ID}/attempts/1/jobs?per_page=100`]: [
      releaseProofJobs(),
    ],
    [`repos/oyatie/console/git/ref/heads/dev`]: [
      { object: { type: "commit", sha: CANDIDATE } },
    ],
    [`repos/oyatie/console/commits/${CANDIDATE}`]: [releaseCommit()],
    [`repos/oyatie/console/commits/${RELEASE_HEAD}`]: [releaseHeadCommit()],
    [`repos/oyatie/console/pulls/${RELEASE_PR_NUMBER}`]: [releasePullRequest()],
    [`repos/oyatie/console/contents/.release-please-manifest.json?ref=${CANDIDATE}`]: [
      '{".":"1.2.3"}\n',
    ],
    [`repos/oyatie/console/contents/.release-please-manifest.json?ref=${PARENT}`]: [
      '{".":"1.2.2"}\n',
    ],
    [`repos/oyatie/console/releases/tags/v1.2.3`]: [publishedRelease()],
    [`repos/oyatie/console/releases/3301`]: [publishedRelease()],
    [`repos/oyatie/console/git/ref/tags/v1.2.3`]: [
      { object: { type: "commit", sha: CANDIDATE } },
    ],
  };
}

function transportRoutes() {
  const routes = baseRoutes();
  routes[`repos/oyatie/console/commits/${CANDIDATE}`] = [
    transportAuthoredReleaseCommit(),
  ];
  routes[`repos/oyatie/console/pulls/${RELEASE_PR_NUMBER}`] = [
    releasePullRequest({ transport: true }),
  ];
  return routes;
}

function fakeExecutable(path, body) {
  writeFileSync(path, `#!${process.execPath}\n${body}`);
  chmodSync(path, 0o755);
}

function runAdmission({ routes = baseRoutes(), env = {} } = {}) {
  const root = mkdtempSync(join(tmpdir(), "console-image-admission-"));
  temporaryRoots.push(root);
  const bin = join(root, "bin");
  const fixturePath = join(root, "fixture.json");
  const statePath = join(root, "state.json");
  const logPath = join(root, "requests.jsonl");
  const sleepLogPath = join(root, "sleep.log");
  const outputPath = join(root, "github-output");
  const summaryPath = join(root, "github-summary");
  mkdirSync(bin);
  writeFileSync(fixturePath, JSON.stringify(routes));
  writeFileSync(statePath, "{}");
  writeFileSync(logPath, "");
  writeFileSync(sleepLogPath, "");
  writeFileSync(outputPath, "");
  writeFileSync(summaryPath, "");
  symlinkSync("/usr/bin/jq", join(bin, "jq"));

  fakeExecutable(
    join(bin, "gh"),
    String.raw`
const fs = require("node:fs");
const args = process.argv.slice(2);
if (args[0] !== "api") process.exit(96);
const required = [
  "api", "-H", "X-GitHub-Api-Version: 2022-11-28",
  "-H", "Cache-Control: no-cache",
];
if (JSON.stringify(args.slice(0, required.length)) !== JSON.stringify(required)) {
  process.stderr.write("unexpected gh argv: " + JSON.stringify(args) + "\n");
  process.exit(97);
}
const endpoint = args.at(-1);
const ordinaryArgv = [...required, endpoint];
const rawArgv = [
  ...required,
  "-H", "Accept: application/vnd.github.raw+json",
  endpoint,
];
const expectedArgv = endpoint.includes("/contents/") ? rawArgv : ordinaryArgv;
if (JSON.stringify(args) !== JSON.stringify(expectedArgv)) {
  process.stderr.write("unexpected extra gh argv: " + JSON.stringify(args) + "\n");
  process.exit(97);
}
fs.appendFileSync(process.env.FAKE_GH_LOG, JSON.stringify(args) + "\n");
const fixtures = JSON.parse(fs.readFileSync(process.env.FAKE_GH_FIXTURE, "utf8"));
const states = JSON.parse(fs.readFileSync(process.env.FAKE_GH_STATE, "utf8"));
if (!Object.hasOwn(fixtures, endpoint)) {
  process.stderr.write("unexpected endpoint: " + endpoint + "\n");
  process.exit(98);
}
const sequence = fixtures[endpoint];
const index = states[endpoint] ?? 0;
states[endpoint] = index + 1;
fs.writeFileSync(process.env.FAKE_GH_STATE, JSON.stringify(states));
const response = sequence[Math.min(index, sequence.length - 1)];
if (response && typeof response === "object" && Object.hasOwn(response, "exit")) {
  if (response.stderr) process.stderr.write(response.stderr);
  process.exit(response.exit);
}
process.stdout.write(typeof response === "string" ? response : JSON.stringify(response));
`,
  );
  fakeExecutable(
    join(bin, "timeout"),
    `const { spawnSync } = require("node:child_process");\nconst args = process.argv.slice(2);\nif (args[0] !== "20s" || args[1] !== "gh" || args[2] !== "api") {\n  process.stderr.write("unexpected timeout argv: " + JSON.stringify(args) + "\\n");\n  process.exit(95);\n}\nconst result = spawnSync(args[1], args.slice(2), { stdio: "inherit" });\nprocess.exit(result.status ?? 99);\n`,
  );
  fakeExecutable(
    join(bin, "sleep"),
    `require("node:fs").appendFileSync(process.env.FAKE_SLEEP_LOG, process.argv.slice(2).join(" ") + "\\n");\n`,
  );

  const result = spawnSync("/bin/bash", ["-c", admissionStep.run], {
    cwd: root,
    encoding: "utf8",
    env: {
      PATH: bin,
      EVENT_NAME: "workflow_run",
      REPO: "oyatie/console",
      EXPECTED_REPOSITORY_ID: String(REPOSITORY_ID),
      GH_TOKEN: "fixture-token",
      TRIGGER_RUN_ID: String(CI_RUN_ID),
      TRIGGER_WORKFLOW_NAME: "CI",
      TRIGGER_WORKFLOW_CONCLUSION: "success",
      TRIGGER_WORKFLOW_EVENT: "push",
      TRIGGER_WORKFLOW_HEAD_BRANCH: "dev",
      TRIGGER_WORKFLOW_HEAD_SHA: CANDIDATE,
      DISPATCH_CANDIDATE_SHA: "",
      DISPATCH_REF: "",
      RUN_ATTEMPT: "1",
      ADMISSION_MAX_POLLS: "2",
      ADMISSION_POLL_SECONDS: "0",
      GITHUB_OUTPUT: outputPath,
      GITHUB_STEP_SUMMARY: summaryPath,
      FAKE_GH_FIXTURE: fixturePath,
      FAKE_GH_STATE: statePath,
      FAKE_GH_LOG: logPath,
      FAKE_SLEEP_LOG: sleepLogPath,
      HTTP_PROXY: "http://127.0.0.1:9",
      HTTPS_PROXY: "http://127.0.0.1:9",
      ALL_PROXY: "http://127.0.0.1:9",
      NO_PROXY: "",
      ...env,
    },
  });

  return {
    ...result,
    output: readFileSync(outputPath, "utf8"),
    summary: readFileSync(summaryPath, "utf8"),
    sleeps: readFileSync(sleepLogPath, "utf8").trim().split("\n").filter(Boolean),
    requests: readFileSync(logPath, "utf8")
      .trim()
      .split("\n")
      .filter(Boolean)
      .map((line) => JSON.parse(line)),
  };
}

describe("Image Release protected workflow shape", () => {
  it("keeps admission read-only and gates every privileged publication stage", () => {
    assert.deepEqual(workflow.permissions, {});
    assert.deepEqual(admission.permissions, { contents: "read", actions: "read" });
    assert.equal(admission["timeout-minutes"], 15);
    assert.equal(admissionStep.env.ADMISSION_MAX_POLLS, "48");
    assert.equal(admissionStep.env.ADMISSION_POLL_SECONDS, "10");
    assert.equal(admission["continue-on-error"], undefined);
    assert.equal(admissionStep["continue-on-error"], undefined);
    assert.equal(admissionStep.if, undefined);
    assert.equal(admission.outputs.eligible, "${{ steps.admit.outputs.eligible }}");
    assert.equal(
      admission.outputs.release_sha,
      "${{ steps.admit.outputs.release_sha }}",
    );
    assert.equal(
      admission.outputs.release_tag,
      "${{ steps.admit.outputs.release_tag }}",
    );
    assert.equal(
      admission.outputs.release_version,
      "${{ steps.admit.outputs.release_version }}",
    );
    assert.equal(workflow.jobs.build.if, "needs.ci-admission.outputs.eligible == 'true'");
    assert.equal(workflow.jobs.build.needs, "ci-admission");
    assert.deepEqual(workflow.jobs.merge.needs, ["ci-admission", "build"]);
    const productionJobs = new Set([
      "production-promotion-preflight",
      "bump-digests",
    ]);
    const nonProductionJobs = Object.entries(workflow.jobs)
      .filter(([name]) => !productionJobs.has(name));
    for (const [name, job] of nonProductionJobs) {
      const permissions = job.permissions;
      assert.ok(
        permissions === undefined
          || (permissions !== null
            && typeof permissions === "object"
            && !Array.isArray(permissions)),
        `${name} must use mapping-form permissions; scalar write-all is forbidden`,
      );
    }
    const nonProductionWriteJobs = nonProductionJobs
      .filter(([, job]) =>
        Object.values(job.permissions ?? {}).some((access) => access === "write"))
      .map(([name]) => name)
      .sort();
    assert.deepEqual(nonProductionWriteJobs, ["build", "merge"]);
    assert.deepEqual(workflow.jobs.build.permissions, {
      contents: "read",
      packages: "write",
    });
    assert.deepEqual(workflow.jobs.merge.permissions, {
      contents: "read",
      packages: "write",
      "id-token": "write",
      attestations: "write",
    });
    for (const name of nonProductionWriteJobs) {
      const job = workflow.jobs[name];
      assert.equal(job["continue-on-error"], undefined);
      if (name === "build") {
        assert.equal(job.if, "needs.ci-admission.outputs.eligible == 'true'");
      } else {
        assert.equal(job.if, undefined);
        assert.ok(
          (Array.isArray(job.needs) ? job.needs : [job.needs]).includes("build"),
          `${name} must remain downstream of the exactly guarded build job`,
        );
      }
    }
    for (const name of ["build", "merge", "release-probe"]) {
      const job = workflow.jobs[name];
      assert.equal(job["continue-on-error"], undefined);
      for (const step of job.steps) {
        assert.equal(
          step["continue-on-error"],
          undefined,
          `${name}/${step.name ?? step.id ?? step.uses} must fail closed`,
        );
      }
    }
    assert.equal(workflow.jobs["release-probe"].if, undefined);
    for (const name of ["build", "merge"]) {
      for (const step of workflow.jobs[name].steps) {
        assert.equal(
          step.if,
          undefined,
          `${name}/${step.name ?? step.id ?? step.uses} must not be conditionally skipped`,
        );
      }
    }
    assert.deepEqual(
      workflow.jobs["release-probe"].steps
        .filter((step) => step.if !== undefined)
        .map((step) => ({ name: step.name, if: step.if })),
      [{ name: "Stop the probe container", if: "always()" }],
    );
    assert.equal(
      workflow.jobs.merge.steps.some(
        (step) => step.uses === "./.github/actions/setup-trivy",
      ),
      false,
    );
    assert.equal(
      workflow.jobs.merge.steps.some((step) =>
        String(step.uses ?? "").startsWith("actions/checkout@"),
      ),
      false,
    );
    assert.match(
      workflow.jobs.merge.steps.map((step) => step.run ?? "").join("\n"),
      /3cbae37cd440cd8676e5ce9207fe460b5641c7579a17e9d00f8894928c41a88d/,
    );
    const buildMetadata = workflow.jobs.build.steps.find((step) => step.id === "meta");
    const mergeMetadata = workflow.jobs.merge.steps.find((step) => step.id === "meta");
    assert.deepEqual(
      mergeMetadata.with.tags.trim().split("\n"),
      [
        "type=semver,pattern={{version}},value=${{ needs.ci-admission.outputs.release_tag }}",
        "type=semver,pattern={{major}}.{{minor}},value=${{ needs.ci-admission.outputs.release_tag }}",
        "type=raw,value=dev",
        "type=raw,value=edge",
        "type=raw,value=sha-${{ needs.ci-admission.outputs.release_sha }}",
      ],
    );
    assert.equal(mergeMetadata.with.flavor, "latest=false");
    for (const metadata of [buildMetadata, mergeMetadata]) {
      assert.match(
        metadata.with.labels,
        /org\.opencontainers\.image\.version=\$\{\{ needs\.ci-admission\.outputs\.release_version \}\}/,
      );
      assert.match(
        metadata.with.labels,
        /org\.opencontainers\.image\.revision=\$\{\{ needs\.ci-admission\.outputs\.release_sha \}\}/,
      );
    }
    assert.deepEqual(workflow.jobs["release-probe"].needs, [
      "ci-admission",
      "merge",
    ]);
    const executableAdmissionShell = admissionStep.run
      .split("\n")
      .filter((line) => !line.trimStart().startsWith("#"))
      .join("\n");
    assert.doesNotMatch(
      executableAdmissionShell,
      /(?:^|[\s;|&()])(?:\/(?:[^/\s;|&()]+\/)+)?(?:curl|wget|git|node|python3?|eval)(?=$|[\s;|&()])/m,
    );
    assert.doesNotMatch(
      executableAdmissionShell,
      /(?:^|[;|&()]\s*)(?:(?:builtin|command)\s+)?source(?=$|\s)/m,
    );
    assert.doesNotMatch(admissionStep.run, /\$\{\{/);
    assert.equal(
      admission.steps.some((step) => /checkout|cache|artifact/i.test(step.uses ?? "")),
      false,
    );
  });

  it("does not alter either protected production-promotion job", () => {
    assert.equal(
      productionJobDigest(),
      "6cf3c0754c9f65c0a92a87233e2dd41a36d6edb6cf8ff45be2064e805c1376db",
    );
  });

  it("keeps cache deletion on its independent schedule or manual trigger", () => {
    assert.deepEqual(Object.keys(cacheHygiene.on).sort(), [
      "schedule",
      "workflow_dispatch",
    ]);
    assert.doesNotMatch(cacheHygieneText, /github\.event\.workflow_run/);

    for (const filename of readdirSync(workflowsDirectory).filter((entry) =>
      entry.endsWith(".yml"))) {
      const candidate = yaml.load(
        readFileSync(new URL(filename, workflowsDirectory), "utf8"),
      );
      const subscriptions = candidate?.on?.workflow_run?.workflows ?? [];
      assert.equal(
        Array.isArray(subscriptions) && subscriptions.includes("Image Release"),
        false,
        `${filename} must not subscribe write-capable follow-up work to Image Release`,
      );
    }
  });

  it("installs locked dependencies before the dependency-backed hardening suite", () => {
    const steps = ciWorkflow.jobs["kubernetes-manifests"].steps;
    const installIndex = steps.findIndex(
      (step) =>
        step.name === "Install production-hardening test dependencies" &&
        step.run === "npm ci --ignore-scripts",
    );
    const testIndex = steps.findIndex(
      (step) =>
        step.name === "Production hardening regression tests" &&
        step.run === "npm run test:production-hardening",
    );
    assert.ok(installIndex >= 0, "dependency install step is absent");
    assert.ok(testIndex > installIndex, "hardening tests run before dependency install");
    assert.equal(
      steps[installIndex].if,
      "${{ !cancelled() && needs.preflight.outputs.run_heavy == 'true' }}",
    );
  });
});

describe("Image Release live admission shell", () => {
  it("admits one immutable exact-SHA release with exact CI and Security", () => {
    const result = runAdmission();
    assert.equal(result.status, 0, result.stderr);
    assert.match(result.output, /^eligible=true$/m);
    assert.match(result.output, new RegExp(`^release_sha=${CANDIDATE}$`, "m"));
    assert.match(result.output, /^release_tag=v1\.2\.3$/m);
    assert.match(result.output, /^release_version=1\.2\.3$/m);
    assert.match(result.output, /^ci_run_id=1101$/m);
    assert.match(result.output, /^security_run_id=2201$/m);
  });

  it("admits the exact pinned-transport squash identity", () => {
    const routes = transportRoutes();
    const result = runAdmission({ routes });
    assert.equal(result.status, 0, result.stderr);
    assert.match(result.output, /^eligible=true$/m);
    assert.match(result.output, new RegExp(`^release_sha=${CANDIDATE}$`, "m"));
  });

  it("classifies an unchanged manifest as an ordinary green no-op", () => {
    const routes = baseRoutes();
    routes[
      `repos/oyatie/console/contents/.release-please-manifest.json?ref=${PARENT}`
    ] = ['{".":"1.2.3"}\n'];
    const result = runAdmission({ routes });
    assert.equal(result.status, 0, result.stderr);
    assert.match(result.output, /^eligible=false$/m);
    assert.doesNotMatch(result.output, /^release_sha=/m);
    assert.equal(
      result.requests.some((args) => args.at(-1).includes("/releases/")),
      false,
    );
  });

  it("cleanly ignores an automatic stale candidate", () => {
    const routes = baseRoutes();
    routes["repos/oyatie/console/git/ref/heads/dev"] = [
      { object: { type: "commit", sha: OTHER } },
    ];
    const result = runAdmission({ routes });
    assert.equal(result.status, 0, result.stderr);
    assert.match(result.output, /^eligible=false$/m);
  });

  it("fails red when current-main readback is unavailable during polling", () => {
    const routes = baseRoutes();
    routes["repos/oyatie/console/git/ref/heads/dev"] = [
      { object: { type: "commit", sha: CANDIDATE } },
      { exit: 1, stderr: "connection refused\n" },
    ];
    const result = runAdmission({ routes });
    assert.notEqual(result.status, 0);
    assert.doesNotMatch(result.output, /^eligible=false$/m);
    assert.doesNotMatch(result.output, /^eligible=true$/m);
  });

  it("fails red instead of treating malformed main refs as stale", () => {
    for (const sha of ["not-a-full-lowercase-sha", `${CANDIDATE}\n`]) {
      const routes = baseRoutes();
      routes["repos/oyatie/console/git/ref/heads/dev"] = [
        { object: { type: "commit", sha } },
      ];
      const result = runAdmission({ routes });
      assert.notEqual(result.status, 0, `accepted malformed main SHA ${JSON.stringify(sha)}`);
      assert.doesNotMatch(result.output, /^eligible=false$/m);
      assert.doesNotMatch(result.output, /^eligible=true$/m);
    }
  });

  it("fails closed on a mutable, draft, or prerelease source release", () => {
    for (const mutation of [
      { immutable: false },
      { draft: true },
      { prerelease: true },
    ]) {
      const routes = baseRoutes();
      const hostileRelease = publishedRelease(mutation);
      routes["repos/oyatie/console/releases/tags/v1.2.3"] = [hostileRelease];
      routes["repos/oyatie/console/releases/3301"] = [hostileRelease];
      const result = runAdmission({ routes });
      assert.notEqual(result.status, 0, result.stdout);
      assert.doesNotMatch(result.output, /^eligible=true$/m);
    }
  });

  it("rejects every exact-release snapshot disagreement", () => {
    const byTagMutations = [
      ["tag", { tag_name: "v1.2.4" }],
      ["target", { target_commitish: OTHER }],
      ["published timestamp", { published_at: null }],
      ["release ID type", { id: "3301" }],
      ["author login", { author: { ...publishedRelease().author, login: "jason931225" } }],
      ["author ID", { author: { ...publishedRelease().author, id: 56489493 } }],
      ["author type", { author: { ...publishedRelease().author, type: "User" } }],
    ];
    for (const [label, mutation] of byTagMutations) {
      const routes = baseRoutes();
      const hostileRelease = publishedRelease(mutation);
      routes["repos/oyatie/console/releases/tags/v1.2.3"] = [hostileRelease];
      routes["repos/oyatie/console/releases/3301"] = [hostileRelease];
      const result = runAdmission({ routes });
      assert.notEqual(result.status, 0, `accepted wrong release ${label}`);
      assert.doesNotMatch(result.output, /^eligible=true$/m);
    }

    const mismatchedById = baseRoutes();
    mismatchedById["repos/oyatie/console/releases/3301"] = [
      publishedRelease({ published_at: "2026-08-16T12:00:01Z" }),
    ];
    const result = runAdmission({ routes: mismatchedById });
    assert.notEqual(result.status, 0, "accepted mismatched release-by-ID evidence");
    assert.doesNotMatch(result.output, /^eligible=true$/m);
  });

  it("rejects every release-commit authority disagreement", () => {
    const mutations = [
      ["candidate SHA", (commit) => ({ ...commit, sha: OTHER })],
      [
        "parent count",
        (commit) => ({
          ...commit,
          parents: [...commit.parents, { sha: OTHER }],
        }),
      ],
      [
        "subject",
        (commit) => ({
          ...commit,
          commit: { ...commit.commit, message: "chore(dev): release 1.2.4 (#760)" },
        }),
      ],
      [
        "subject NUL",
        (commit) => ({
          ...commit,
          commit: {
            ...commit.commit,
            message: "chore(dev): release 1.2.3 (#760)\u0000\n\nrelease notes",
          },
        }),
      ],
      [
        "parent SHA",
        (commit) => ({
          ...commit,
          parents: [{ sha: `${PARENT}\n` }],
        }),
      ],
      [
        "tree type",
        (commit) => ({
          ...commit,
          commit: { ...commit.commit, tree: { sha: null } },
        }),
      ],
      [
        "tree syntax",
        (commit) => ({
          ...commit,
          commit: { ...commit.commit, tree: { sha: `${RELEASE_TREE}\n` } },
        }),
      ],
      [
        "commit author name",
        (commit) => ({
          ...commit,
          commit: {
            ...commit.commit,
            author: { ...commit.commit.author, name: "github-actions" },
          },
        }),
      ],
      [
        "commit author email",
        (commit) => ({
          ...commit,
          commit: {
            ...commit.commit,
            author: { ...commit.commit.author, email: "actions@github.com" },
          },
        }),
      ],
      [
        "commit committer name",
        (commit) => ({
          ...commit,
          commit: {
            ...commit.commit,
            committer: { ...commit.commit.committer, name: "github-actions[bot]" },
          },
        }),
      ],
      [
        "commit committer email",
        (commit) => ({
          ...commit,
          commit: {
            ...commit.commit,
            committer: { ...commit.commit.committer, email: "actions@github.com" },
          },
        }),
      ],
      [
        "verified bit",
        (commit) => ({
          ...commit,
          commit: {
            ...commit.commit,
            verification: { ...commit.commit.verification, verified: false },
          },
        }),
      ],
      [
        "verification reason",
        (commit) => ({
          ...commit,
          commit: {
            ...commit.commit,
            verification: { ...commit.commit.verification, reason: "unsigned" },
          },
        }),
      ],
      ["API author ID", (commit) => ({ ...commit, author: { ...commit.author, id: 7 } })],
      [
        "API author login",
        (commit) => ({ ...commit, author: { ...commit.author, login: "github-actions" } }),
      ],
      [
        "API author type",
        (commit) => ({ ...commit, author: { ...commit.author, type: "User" } }),
      ],
      [
        "API committer ID",
        (commit) => ({ ...commit, committer: { ...commit.committer, id: 7 } }),
      ],
      [
        "API committer login",
        (commit) => ({ ...commit, committer: { ...commit.committer, login: "github" } }),
      ],
      [
        "API committer type",
        (commit) => ({ ...commit, committer: { ...commit.committer, type: "Bot" } }),
      ],
      [
        "extra file",
        (commit) => ({
          ...commit,
          files: [
            ...commit.files,
            { filename: "backend/src/lib.rs", status: "modified" },
          ],
        }),
      ],
      [
        "missing file",
        (commit) => ({ ...commit, files: commit.files.slice(0, -1) }),
      ],
      [
        "file status",
        (commit) => ({
          ...commit,
          files: commit.files.map((file, index) =>
            index === 0 ? { ...file, status: "added" } : file),
        }),
      ],
    ];
    for (const [label, mutate] of mutations) {
      const routes = baseRoutes();
      routes[`repos/oyatie/console/commits/${CANDIDATE}`] = [
        mutate(releaseCommit()),
      ];
      const result = runAdmission({ routes });
      assert.notEqual(result.status, 0, `accepted wrong release-commit ${label}`);
      assert.doesNotMatch(result.output, /^eligible=true$/m);
    }
  });

  it("rejects every pinned-transport author disagreement", () => {
    const mutations = [
      [
        "commit author name",
        (commit) => ({
          ...commit,
          commit: {
            ...commit.commit,
            author: { ...commit.commit.author, name: "Jason" },
          },
        }),
      ],
      [
        "commit author email",
        (commit) => ({
          ...commit,
          commit: {
            ...commit.commit,
            author: { ...commit.commit.author, email: "jason@example.invalid" },
          },
        }),
      ],
      [
        "API author ID",
        (commit) => ({ ...commit, author: { ...commit.author, id: 7 } }),
      ],
      [
        "API author login",
        (commit) => ({
          ...commit,
          author: { ...commit.author, login: "jason" },
        }),
      ],
      [
        "API author type",
        (commit) => ({ ...commit, author: { ...commit.author, type: "Bot" } }),
      ],
    ];
    for (const [label, mutate] of mutations) {
      const routes = transportRoutes();
      routes[`repos/oyatie/console/commits/${CANDIDATE}`] = [
        mutate(transportAuthoredReleaseCommit()),
      ];
      const result = runAdmission({ routes });
      assert.notEqual(result.status, 0, `accepted wrong transport author ${label}`);
      assert.doesNotMatch(result.output, /^eligible=true$/m);
    }
  });

  it("requires native proof and an exact same-tree protected release head", () => {
    const withoutProof = transportRoutes();
    withoutProof[
      `repos/oyatie/console/actions/workflows/${RELEASE_WORKFLOW_ID}/runs?event=push&branch=dev&head_sha=${PARENT}&per_page=100`
    ] = [{ total_count: 0, workflow_runs: [] }];
    const missingProof = runAdmission({ routes: withoutProof });
    assert.notEqual(missingProof.status, 0, "accepted transport author without proof");
    assert.match(missingProof.stderr, /protected Release Please proof run is missing or invalid/);

    const wrongTree = transportRoutes();
    wrongTree[`repos/oyatie/console/commits/${RELEASE_HEAD}`] = [
      {
        ...releaseHeadCommit(),
        commit: {
          ...releaseHeadCommit().commit,
          tree: { sha: OTHER },
        },
      },
    ];
    const mismatchedTree = runAdmission({ routes: wrongTree });
    assert.notEqual(mismatchedTree.status, 0, "accepted a different protected-head tree");
    assert.match(mismatchedTree.stderr, /does not bind the exact squash tree/);
  });

  it("rejects every merged release-PR provenance disagreement", () => {
    const mutations = [
      ["number", (pr) => ({ ...pr, number: RELEASE_PR_NUMBER + 1 })],
      ["state", (pr) => ({ ...pr, state: "open" })],
      ["draft", (pr) => ({ ...pr, draft: true })],
      ["merged bit", (pr) => ({ ...pr, merged: false })],
      ["merged timestamp", (pr) => ({ ...pr, merged_at: null })],
      ["empty merged timestamp", (pr) => ({ ...pr, merged_at: "" })],
      ["title", (pr) => ({ ...pr, title: "chore(dev): release 1.2.4" })],
      ["merge SHA", (pr) => ({ ...pr, merge_commit_sha: OTHER })],
      ["base ref", (pr) => ({ ...pr, base: { ...pr.base, ref: "release" } })],
      ["base SHA", (pr) => ({ ...pr, base: { ...pr.base, sha: OTHER } })],
      [
        "base repository ID",
        (pr) => ({
          ...pr,
          base: { ...pr.base, repo: { ...pr.base.repo, id: 7 } },
        }),
      ],
      [
        "base repository name",
        (pr) => ({
          ...pr,
          base: { ...pr.base, repo: { ...pr.base.repo, full_name: "attacker/console" } },
        }),
      ],
      ["head SHA", (pr) => ({ ...pr, head: { ...pr.head, sha: OTHER } })],
      ["head SHA type", (pr) => ({ ...pr, head: { ...pr.head, sha: null } })],
      [
        "head SHA syntax",
        (pr) => ({ ...pr, head: { ...pr.head, sha: `${RELEASE_HEAD}\n` } }),
      ],
      ["head ref", (pr) => ({ ...pr, head: { ...pr.head, ref: "feature/release" } })],
      ["head ref type", (pr) => ({ ...pr, head: { ...pr.head, ref: null } })],
      [
        "head repository ID",
        (pr) => ({
          ...pr,
          head: { ...pr.head, repo: { ...pr.head.repo, id: 7 } },
        }),
      ],
      [
        "head repository name",
        (pr) => ({
          ...pr,
          head: { ...pr.head, repo: { ...pr.head.repo, full_name: "attacker/console" } },
        }),
      ],
      ["bot creator ID", (pr) => ({ ...pr, user: { ...pr.user, id: 7 } })],
      [
        "bot creator login",
        (pr) => ({ ...pr, user: { ...pr.user, login: "github-actions" } }),
      ],
      ["bot creator type", (pr) => ({ ...pr, user: { ...pr.user, type: "User" } })],
    ];
    for (const [label, mutate] of mutations) {
      const routes = baseRoutes();
      routes[`repos/oyatie/console/pulls/${RELEASE_PR_NUMBER}`] = [
        mutate(releasePullRequest()),
      ];
      if (label === "head SHA") {
        routes[`repos/oyatie/console/commits/${OTHER}`] = [
          { ...releaseHeadCommit(), sha: OTHER },
        ];
      }
      const result = runAdmission({ routes });
      assert.notEqual(result.status, 0, `accepted wrong release PR ${label}`);
      assert.match(
        result.stderr,
        label === "head SHA"
          ? /native Release Please authority proof/
          : /release pull request provenance is invalid/,
      );
      assert.doesNotMatch(result.output, /^eligible=true$/m);
    }

    const transportCreatorMutations = [
      ["transport creator ID", (pr) => ({ ...pr, user: { ...pr.user, id: 7 } })],
      [
        "transport creator login",
        (pr) => ({ ...pr, user: { ...pr.user, login: "jason" } }),
      ],
      [
        "transport creator type",
        (pr) => ({ ...pr, user: { ...pr.user, type: "Bot" } }),
      ],
    ];
    for (const [label, mutate] of transportCreatorMutations) {
      const routes = transportRoutes();
      routes[`repos/oyatie/console/pulls/${RELEASE_PR_NUMBER}`] = [
        mutate(releasePullRequest({ transport: true })),
      ];
      const result = runAdmission({ routes });
      assert.notEqual(result.status, 0, `accepted wrong release PR ${label}`);
      assert.match(result.stderr, /release pull request provenance is invalid/);
      assert.doesNotMatch(result.output, /^eligible=true$/m);
    }
  });

  it("rejects mixed release-PR creator and squash-author classes", () => {
    const transportCommitForBotPull = baseRoutes();
    transportCommitForBotPull[`repos/oyatie/console/commits/${CANDIDATE}`] = [
      transportAuthoredReleaseCommit(),
    ];
    const first = runAdmission({ routes: transportCommitForBotPull });
    assert.notEqual(first.status, 0, "accepted transport squash for a bot-created PR");
    assert.match(first.stderr, /release commit identity or exact four-file envelope is invalid/);

    const botCommitForTransportPull = baseRoutes();
    botCommitForTransportPull[`repos/oyatie/console/pulls/${RELEASE_PR_NUMBER}`] = [
      releasePullRequest({ transport: true }),
    ];
    const second = runAdmission({ routes: botCommitForTransportPull });
    assert.notEqual(second.status, 0, "accepted bot squash for a transport-created PR");
    assert.match(second.stderr, /release commit identity or exact four-file envelope is invalid/);
  });

  it("rejects every protected release-head disagreement", () => {
    const mutations = [
      ["SHA", (head) => ({ ...head, sha: OTHER })],
      ["parent", (head) => ({ ...head, parents: [{ sha: OTHER }] })],
      ["parent count", (head) => ({ ...head, parents: [...head.parents, { sha: OTHER }] })],
      [
        "subject",
        (head) => ({
          ...head,
          commit: { ...head.commit, message: "chore(dev): release 1.2.4" },
        }),
      ],
      [
        "Git author name",
        (head) => ({
          ...head,
          commit: {
            ...head.commit,
            author: { ...head.commit.author, name: "github-actions" },
          },
        }),
      ],
      [
        "Git author email",
        (head) => ({
          ...head,
          commit: {
            ...head.commit,
            author: { ...head.commit.author, email: "actions@github.com" },
          },
        }),
      ],
      [
        "Git committer name",
        (head) => ({
          ...head,
          commit: {
            ...head.commit,
            committer: { ...head.commit.committer, name: "github-actions[bot]" },
          },
        }),
      ],
      [
        "Git committer email",
        (head) => ({
          ...head,
          commit: {
            ...head.commit,
            committer: { ...head.commit.committer, email: "actions@github.com" },
          },
        }),
      ],
      ["API author ID", (head) => ({ ...head, author: { ...head.author, id: 7 } })],
      [
        "API author login",
        (head) => ({ ...head, author: { ...head.author, login: "github-actions" } }),
      ],
      ["API author type", (head) => ({ ...head, author: { ...head.author, type: "User" } })],
      [
        "API committer ID",
        (head) => ({ ...head, committer: { ...head.committer, id: 7 } }),
      ],
      [
        "API committer login",
        (head) => ({
          ...head,
          committer: { ...head.committer, login: "github" },
        }),
      ],
      [
        "API committer type",
        (head) => ({ ...head, committer: { ...head.committer, type: "Bot" } }),
      ],
      [
        "tree",
        (head) => ({
          ...head,
          commit: { ...head.commit, tree: { sha: OTHER } },
        }),
      ],
      [
        "message type",
        (head) => ({ ...head, commit: { ...head.commit, message: null } }),
      ],
      [
        "extra file",
        (head) => ({
          ...head,
          files: [...head.files, { filename: "backend/src/lib.rs", status: "modified" }],
        }),
      ],
      ["missing file", (head) => ({ ...head, files: head.files.slice(0, -1) })],
      [
        "file status",
        (head) => ({
          ...head,
          files: head.files.map((file, index) =>
            index === 0 ? { ...file, status: "added" } : file),
        }),
      ],
    ];
    for (const [label, mutate] of mutations) {
      const routes = baseRoutes();
      routes[`repos/oyatie/console/commits/${RELEASE_HEAD}`] = [
        mutate(releaseHeadCommit()),
      ];
      const result = runAdmission({ routes });
      assert.notEqual(result.status, 0, `accepted wrong protected release head ${label}`);
      assert.match(result.stderr, /protected release head does not bind the exact squash tree/);
      assert.doesNotMatch(result.output, /^eligible=true$/m);
    }
  });

  it("rejects every protected Release Please run disagreement", () => {
    const endpoint = `repos/oyatie/console/actions/workflows/${RELEASE_WORKFLOW_ID}/runs?event=push&branch=dev&head_sha=${PARENT}&per_page=100`;
    const trusted = workflowRun({
      id: RELEASE_RUN_ID,
      workflowId: RELEASE_WORKFLOW_ID,
      path: ".github/workflows/release-please.yml",
      headSha: PARENT,
      runNumber: RELEASE_RUN_NUMBER,
    });
    const mutations = [
      ["workflow ID", (run) => ({ ...run, workflow_id: 7 })],
      ["workflow path", (run) => ({ ...run, path: ".github/workflows/untrusted.yml" })],
      ["event", (run) => ({ ...run, event: "workflow_dispatch" })],
      ["branch", (run) => ({ ...run, head_branch: "release" })],
      ["head", (run) => ({ ...run, head_sha: OTHER })],
      [
        "repository",
        (run) => ({
          ...run,
          repository: { id: 7, full_name: "attacker/console" },
        }),
      ],
      ["run ID", (run) => ({ ...run, id: "1701" })],
      ["run number", (run) => ({ ...run, run_number: 0 })],
      ["run attempt", (run) => ({ ...run, run_attempt: "1" })],
      ["status", (run) => ({ ...run, status: "in_progress", conclusion: null })],
      ["conclusion", (run) => ({ ...run, conclusion: "failure" })],
    ];
    for (const [label, mutate] of mutations) {
      const routes = baseRoutes();
      routes[endpoint] = [{ total_count: 1, workflow_runs: [mutate(trusted)] }];
      const result = runAdmission({ routes });
      assert.notEqual(result.status, 0, `accepted wrong Release Please run ${label}`);
      assert.match(result.stderr, /protected Release Please proof run is missing or invalid/);
      assert.doesNotMatch(result.output, /^eligible=true$/m);
    }

    const duplicate = baseRoutes();
    duplicate[endpoint] = [
      { total_count: 2, workflow_runs: [trusted, { ...trusted, id: RELEASE_RUN_ID + 1 }] },
    ];
    const duplicateResult = runAdmission({ routes: duplicate });
    assert.notEqual(duplicateResult.status, 0, "accepted duplicate Release Please runs");
    assert.match(duplicateResult.stderr, /protected Release Please proof run is missing or invalid/);
  });

  it("rejects every native Release Please proof-job disagreement", () => {
    const endpoint = `repos/oyatie/console/actions/runs/${RELEASE_RUN_ID}/attempts/1/jobs?per_page=100`;
    const trusted = releaseProofJobs().jobs[0];
    const cases = [
      ["missing", { total_count: 0, jobs: [] }],
      ["duplicate", { total_count: 2, jobs: [trusted, { ...trusted, id: RELEASE_PROOF_JOB_ID + 1 }] }],
      ["count type", { total_count: "1", jobs: [trusted] }],
      ["negative count", { total_count: -1, jobs: [] }],
      ["fractional count", { total_count: 1.5, jobs: [trusted] }],
      [
        "page overflow",
        { total_count: 101, jobs: Array.from({ length: 101 }, () => trusted) },
      ],
      ["jobs collection type", { total_count: 1, jobs: { 0: trusted } }],
      ["truncated page", { total_count: 2, jobs: [trusted] }],
      ["count mismatch", { total_count: 1, jobs: [] }],
      ["job ID", releaseProofJobs({ id: "1702" })],
      ["run ID", releaseProofJobs({ run_id: 7 })],
      ["run attempt", releaseProofJobs({ run_attempt: 2 })],
      ["workflow name", releaseProofJobs({ workflow_name: "CI" })],
      ["head", releaseProofJobs({ head_sha: OTHER })],
      ["PR coordinate", releaseProofJobs({ name: `release-authority-proof pr=761 head=${RELEASE_HEAD}` })],
      ["head coordinate", releaseProofJobs({ name: `release-authority-proof pr=${RELEASE_PR_NUMBER} head=${OTHER}` })],
      ["status", releaseProofJobs({ status: "in_progress", conclusion: null })],
      ["conclusion", releaseProofJobs({ conclusion: "failure" })],
    ];
    for (const [label, jobs] of cases) {
      const routes = baseRoutes();
      routes[endpoint] = [jobs];
      const result = runAdmission({ routes });
      assert.notEqual(result.status, 0, `accepted wrong Release Please proof job ${label}`);
      assert.match(result.stderr, /native Release Please authority proof|proof jobs are malformed/);
      assert.doesNotMatch(result.output, /^eligible=true$/m);
    }
  });

  it("rejects release PR, protected head, or proof-run movement on final readback", () => {
    const races = [
      [
        `repos/oyatie/console/pulls/${RELEASE_PR_NUMBER}`,
        releasePullRequest(),
        { ...releasePullRequest(), title: "chore(dev): release 1.2.4" },
        /release pull request changed during admission/,
      ],
      [
        `repos/oyatie/console/commits/${RELEASE_HEAD}`,
        releaseHeadCommit(),
        { ...releaseHeadCommit(), files: releaseHeadCommit().files.slice(0, -1) },
        /protected release head changed during admission/,
      ],
      [
        `repos/oyatie/console/actions/workflows/${RELEASE_WORKFLOW_ID}/runs?event=push&branch=dev&head_sha=${PARENT}&per_page=100`,
        {
          total_count: 1,
          workflow_runs: [
            workflowRun({
              id: RELEASE_RUN_ID,
              workflowId: RELEASE_WORKFLOW_ID,
              path: ".github/workflows/release-please.yml",
              headSha: PARENT,
              runNumber: RELEASE_RUN_NUMBER,
            }),
          ],
        },
        {
          total_count: 1,
          workflow_runs: [
            workflowRun({
              id: RELEASE_RUN_ID,
              workflowId: RELEASE_WORKFLOW_ID,
              path: ".github/workflows/release-please.yml",
              headSha: PARENT,
              runNumber: RELEASE_RUN_NUMBER,
              runAttempt: 2,
            }),
          ],
        },
        /protected Release Please proof run changed during admission/,
      ],
    ];
    for (const [endpoint, before, after, error] of races) {
      const routes = baseRoutes();
      routes[endpoint] = [before, after];
      const result = runAdmission({ routes });
      assert.notEqual(result.status, 0, `accepted moving release evidence at ${endpoint}`);
      assert.match(result.stderr, error);
      assert.doesNotMatch(result.output, /^eligible=true$/m);
    }
  });

  it("fails closed when exact Security is terminally red", () => {
    const routes = baseRoutes();
    routes[
      `repos/oyatie/console/actions/workflows/${SECURITY_WORKFLOW_ID}/runs?event=push&branch=dev&head_sha=${CANDIDATE}&per_page=100`
    ] = [
      {
        total_count: 1,
        workflow_runs: [
          workflowRun({
            id: SECURITY_RUN_ID,
            workflowId: SECURITY_WORKFLOW_ID,
            path: ".github/workflows/security.yml",
            conclusion: "failure",
          }),
        ],
      },
    ];
    const result = runAdmission({ routes });
    assert.notEqual(result.status, 0, result.stdout);
    assert.doesNotMatch(result.output, /^eligible=true$/m);
  });

  it("fails closed when a workflow attempt advances during its jobs read", () => {
    const routes = baseRoutes();
    const attemptOne = workflowRun({
      id: SECURITY_RUN_ID,
      workflowId: SECURITY_WORKFLOW_ID,
      path: ".github/workflows/security.yml",
    });
    const attemptTwo = {
      ...attemptOne,
      run_attempt: 2,
      status: "in_progress",
      conclusion: null,
    };
    routes[
      `repos/oyatie/console/actions/workflows/${SECURITY_WORKFLOW_ID}/runs?event=push&branch=dev&head_sha=${CANDIDATE}&per_page=100`
    ] = [
      { total_count: 1, workflow_runs: [attemptOne] },
      { total_count: 1, workflow_runs: [attemptTwo] },
    ];
    const result = runAdmission({ routes });
    assert.notEqual(result.status, 0, result.stdout);
    assert.doesNotMatch(result.output, /^eligible=true$/m);
  });

  it("fails closed if the immutable tag moves during the final readback", () => {
    const routes = baseRoutes();
    routes["repos/oyatie/console/git/ref/tags/v1.2.3"] = [
      { object: { type: "commit", sha: CANDIDATE } },
      { object: { type: "commit", sha: OTHER } },
    ];
    const result = runAdmission({ routes });
    assert.notEqual(result.status, 0, result.stdout);
    assert.doesNotMatch(result.output, /^eligible=true$/m);
  });

  it("polls bounded pending Security evidence and then admits it", () => {
    const routes = baseRoutes();
    routes[
      `repos/oyatie/console/actions/workflows/${SECURITY_WORKFLOW_ID}/runs?event=push&branch=dev&head_sha=${CANDIDATE}&per_page=100`
    ] = [
      {
        total_count: 1,
        workflow_runs: [
          workflowRun({
            id: SECURITY_RUN_ID,
            workflowId: SECURITY_WORKFLOW_ID,
            path: ".github/workflows/security.yml",
            status: "in_progress",
            conclusion: null,
          }),
        ],
      },
      {
        total_count: 1,
        workflow_runs: [
          workflowRun({
            id: SECURITY_RUN_ID,
            workflowId: SECURITY_WORKFLOW_ID,
            path: ".github/workflows/security.yml",
          }),
        ],
      },
    ];
    const result = runAdmission({ routes });
    assert.equal(result.status, 0, result.stderr);
    assert.match(result.output, /^eligible=true$/m);
    assert.deepEqual(result.sleeps, ["0"]);
  });

  it("outlasts the former twelve-poll bound while remaining finite", () => {
    const routes = baseRoutes();
    const pendingSecurity = {
      total_count: 1,
      workflow_runs: [
        workflowRun({
          id: SECURITY_RUN_ID,
          workflowId: SECURITY_WORKFLOW_ID,
          path: ".github/workflows/security.yml",
          status: "in_progress",
          conclusion: null,
        }),
      ],
    };
    const successfulSecurity = {
      total_count: 1,
      workflow_runs: [
        workflowRun({
          id: SECURITY_RUN_ID,
          workflowId: SECURITY_WORKFLOW_ID,
          path: ".github/workflows/security.yml",
        }),
      ],
    };
    routes[
      `repos/oyatie/console/actions/workflows/${SECURITY_WORKFLOW_ID}/runs?event=push&branch=dev&head_sha=${CANDIDATE}&per_page=100`
    ] = [
      ...Array.from({ length: 12 }, () => pendingSecurity),
      successfulSecurity,
    ];

    const formerBound = runAdmission({
      routes,
      env: { ADMISSION_MAX_POLLS: "12" },
    });
    assert.notEqual(formerBound.status, 0);
    assert.match(
      formerBound.stderr,
      /release, CI, or Security evidence did not converge within the bounded wait/,
    );
    assert.equal(formerBound.sleeps.length, 11);

    const replacementBound = runAdmission({
      routes,
      env: { ADMISSION_MAX_POLLS: "13" },
    });
    assert.equal(replacementBound.status, 0, replacementBound.stderr);
    assert.match(replacementBound.output, /^eligible=true$/m);
    assert.equal(replacementBound.sleeps.length, 12);
  });

  it("polls a not-yet-visible release only for an explicit 404", () => {
    const routes = baseRoutes();
    routes["repos/oyatie/console/releases/tags/v1.2.3"] = [
      { exit: 1, stderr: "gh: Not Found (HTTP 404)\n" },
      publishedRelease(),
    ];
    const result = runAdmission({ routes });
    assert.equal(result.status, 0, result.stderr);
    assert.match(result.output, /^eligible=true$/m);
    assert.deepEqual(result.sleeps, ["0"]);

    const networkRoutes = baseRoutes();
    networkRoutes["repos/oyatie/console/releases/tags/v1.2.3"] = [
      { exit: 1, stderr: "connection refused\n" },
    ];
    const networkFailure = runAdmission({ routes: networkRoutes });
    assert.notEqual(networkFailure.status, 0);
    assert.deepEqual(networkFailure.sleeps, []);
  });

  it("uses the same immutable release, CI, and Security proof for manual recovery", () => {
    const result = runAdmission({
      env: {
        EVENT_NAME: "workflow_dispatch",
        TRIGGER_RUN_ID: "",
        TRIGGER_WORKFLOW_NAME: "",
        TRIGGER_WORKFLOW_CONCLUSION: "",
        TRIGGER_WORKFLOW_EVENT: "",
        TRIGGER_WORKFLOW_HEAD_BRANCH: "",
        TRIGGER_WORKFLOW_HEAD_SHA: "",
        DISPATCH_CANDIDATE_SHA: CANDIDATE,
        DISPATCH_REF: "refs/heads/dev",
      },
    });
    assert.equal(result.status, 0, result.stderr);
    assert.match(result.output, /^eligible=true$/m);
    assert.equal(
      result.requests.some((args) =>
        args.at(-1).endsWith(`/actions/runs/${CI_RUN_ID}`),
      ),
      false,
    );
  });

  it("turns a terminally failed source CI wake-up into a green no-op", () => {
    const routes = baseRoutes();
    routes[`repos/oyatie/console/actions/runs/${CI_RUN_ID}`] = [
      workflowRun({
        id: CI_RUN_ID,
        workflowId: CI_WORKFLOW_ID,
        path: ".github/workflows/ci.yml",
        conclusion: "failure",
      }),
    ];
    const result = runAdmission({
      routes,
      env: { TRIGGER_WORKFLOW_CONCLUSION: "failure" },
    });
    assert.equal(result.status, 0, result.stderr);
    assert.match(result.output, /^eligible=false$/m);
    assert.equal(result.requests.length, 1);
  });

  it("turns a successful source CI rerun wake-up into a green no-op", () => {
    const routes = baseRoutes();
    routes[`repos/oyatie/console/actions/runs/${CI_RUN_ID}`] = [
      {
        ...workflowRun({
          id: CI_RUN_ID,
          workflowId: CI_WORKFLOW_ID,
          path: ".github/workflows/ci.yml",
        }),
        run_attempt: 2,
      },
    ];
    const result = runAdmission({ routes });
    assert.equal(result.status, 0, result.stderr);
    assert.match(result.output, /^eligible=false$/m);
    assert.equal(result.requests.length, 1);
  });

  it("rejects malformed release intent and protected-workflow identity", () => {
    const malformedManifest = baseRoutes();
    malformedManifest[
      `repos/oyatie/console/contents/.release-please-manifest.json?ref=${CANDIDATE}`
    ] = ['{"component":"1.2.3"}\n'];
    assert.notEqual(runAdmission({ routes: malformedManifest }).status, 0);

    for (const hostileVersion of [
      '{".":"1.2.3\\n"}\n',
      '{".":"1.2.3\\r"}\n',
      '{".":"1.2.3\\u0000"}\n',
      '{".":"1.2.\x003"}\n',
    ]) {
      const normalizedVersion = baseRoutes();
      normalizedVersion[
        `repos/oyatie/console/contents/.release-please-manifest.json?ref=${CANDIDATE}`
      ] = [hostileVersion];
      assert.notEqual(runAdmission({ routes: normalizedVersion }).status, 0);
    }

    const annotatedTag = baseRoutes();
    annotatedTag["repos/oyatie/console/git/ref/tags/v1.2.3"] = [
      { object: { type: "tag", sha: OTHER } },
    ];
    assert.notEqual(runAdmission({ routes: annotatedTag }).status, 0);

    const wrongTrigger = baseRoutes();
    wrongTrigger[`repos/oyatie/console/actions/runs/${CI_RUN_ID}`] = [
      workflowRun({
        id: CI_RUN_ID,
        workflowId: SECURITY_WORKFLOW_ID,
        path: ".github/workflows/security.yml",
      }),
    ];
    assert.notEqual(runAdmission({ routes: wrongTrigger }).status, 0);

    const duplicateSecurity = baseRoutes();
    const securityRun = workflowRun({
      id: SECURITY_RUN_ID,
      workflowId: SECURITY_WORKFLOW_ID,
      path: ".github/workflows/security.yml",
    });
    duplicateSecurity[
      `repos/oyatie/console/actions/workflows/${SECURITY_WORKFLOW_ID}/runs?event=push&branch=dev&head_sha=${CANDIDATE}&per_page=100`
    ] = [{ total_count: 2, workflow_runs: [securityRun, { ...securityRun, id: 2202 }] }];
    assert.notEqual(runAdmission({ routes: duplicateSecurity }).status, 0);
  });

  it("rejects every generic CI and Security run-identity disagreement", () => {
    const lanes = [
      {
        id: CI_RUN_ID,
        workflowId: CI_WORKFLOW_ID,
        path: ".github/workflows/ci.yml",
      },
      {
        id: SECURITY_RUN_ID,
        workflowId: SECURITY_WORKFLOW_ID,
        path: ".github/workflows/security.yml",
      },
    ];
    const mutations = [
      ["repository", (run) => ({ ...run, repository: { ...run.repository, id: 7 } })],
      ["workflow", (run) => ({ ...run, workflow_id: 7 })],
      ["path", (run) => ({ ...run, path: ".github/workflows/untrusted.yml" })],
      ["event", (run) => ({ ...run, event: "workflow_dispatch" })],
      ["branch", (run) => ({ ...run, head_branch: "not-main" })],
      ["head", (run) => ({ ...run, head_sha: OTHER })],
      ["status control", (run) => ({ ...run, status: "completed\u0000" })],
      ["conclusion control", (run) => ({ ...run, conclusion: "success\u0000" })],
    ];
    for (const lane of lanes) {
      const endpoint = `repos/oyatie/console/actions/workflows/${lane.workflowId}/runs?event=push&branch=dev&head_sha=${CANDIDATE}&per_page=100`;
      for (const [label, mutate] of mutations) {
        const routes = baseRoutes();
        const trusted = workflowRun(lane);
        routes[endpoint] = [
          { total_count: 1, workflow_runs: [mutate(trusted)] },
        ];
        const result = runAdmission({ routes });
        assert.notEqual(
          result.status,
          0,
          `${lane.path} accepted a mismatched ${label}`,
        );
        assert.doesNotMatch(result.output, /^eligible=true$/m);
      }
    }
  });

  it("rejects missing, duplicate, or red CI and Security aggregate jobs", () => {
    const lanes = [
      { id: CI_RUN_ID, name: "Required / CI" },
      { id: SECURITY_RUN_ID, name: "Required / Security" },
    ];
    for (const lane of lanes) {
      const endpoint = `repos/oyatie/console/actions/runs/${lane.id}/attempts/1/jobs?per_page=100`;
      const cases = [
        ["missing", { total_count: 0, jobs: [] }],
        [
          "duplicate",
          {
            total_count: 2,
            jobs: [
              { name: lane.name, status: "completed", conclusion: "success" },
              { name: lane.name, status: "completed", conclusion: "success" },
            ],
          },
        ],
        ["red", aggregateJobs(lane.name, "failure")],
        [
          "normalized status",
          {
            total_count: 1,
            jobs: [{ name: lane.name, status: "completed\u0000", conclusion: "success" }],
          },
        ],
        [
          "normalized conclusion",
          {
            total_count: 1,
            jobs: [{ name: lane.name, status: "completed", conclusion: "success\u0000" }],
          },
        ],
        [
          "normalized total count",
          {
            total_count: "1\u0000",
            jobs: [{ name: lane.name, status: "completed", conclusion: "success" }],
          },
        ],
      ];
      for (const [label, jobs] of cases) {
        const routes = baseRoutes();
        routes[endpoint] = [jobs];
        const result = runAdmission({ routes });
        assert.notEqual(result.status, 0, `${lane.name} accepted ${label} aggregate evidence`);
        assert.doesNotMatch(result.output, /^eligible=true$/m);
      }
    }
  });

  it("rejects a shell-normalizable workflow run count before extraction", () => {
    const routes = baseRoutes();
    const endpoint = `repos/oyatie/console/actions/workflows/${SECURITY_WORKFLOW_ID}/runs?event=push&branch=dev&head_sha=${CANDIDATE}&per_page=100`;
    routes[endpoint] = [
      {
        total_count: "1\u0000",
        workflow_runs: [
          workflowRun({
            id: SECURITY_RUN_ID,
            workflowId: SECURITY_WORKFLOW_ID,
            path: ".github/workflows/security.yml",
          }),
        ],
      },
    ];
    const result = runAdmission({ routes });
    assert.notEqual(result.status, 0);
    assert.doesNotMatch(result.output, /^eligible=true$/m);
  });

  it("rejects Image Release reruns before any API read", () => {
    const result = runAdmission({ env: { RUN_ATTEMPT: "2" } });
    assert.notEqual(result.status, 0);
    assert.deepEqual(result.requests, []);
  });
});
