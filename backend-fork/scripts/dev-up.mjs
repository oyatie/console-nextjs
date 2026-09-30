#!/usr/bin/env node
// Local backend dev environment orchestrator. Node-only (no bashisms) so it
// runs the same on macOS/Linux/Windows.
//
// Reuses ops/compose.yml (pinned Postgres, SeaweedFS per ADR-0005) + the dev
// wiring in ops/compose.dev.yml via `-f` layering; ops/compose.dev-deps.yml
// adds only what those two files don't have yet (Mailpit, a published OTEL
// port, and the dev-only WAL archive retention helper). The one thing this
// script changes vs. ops/dev-up.sh: console-app runs ON
// THE HOST from a Buck2-built executable instead of in a container, so every docker-network
// hostname (postgres, seaweedfs, otel-collector) has to be rewritten to a
// published localhost port — that relocation is `buildAppEnv()` below.
//
// Subcommands:
//   doctor     per-OS environment checks with remediation; --cached replays
//              the last result instead of re-probing (fast path for hooks).
//   up         deps -> migrate -> Buck2-built backend, foreground.
//              Ctrl+C stops the backend; deps stay up for a
//              fast restart. Run `down` separately to stop the deps.
//   bootstrap  deps -> migrate -> console-app in the background -> /readyz probe,
//              then exits. No watch loop — this is what CI's smoke job runs.
//   down       stops the exact Buck2 artifact started by `up`/`bootstrap` via
//              the pid file, then the compose deps.
//
// Env flags:
//   CONSOLE_DEV_OFFICE=1  also start the ONLYOFFICE DocumentServer dep (in-console
//                      office editor). Off by default — the image is ~2GB and
//                      most dev-up sessions never touch the office editor; the
//                      backend's office routes gracefully 503 without it.
import { spawn, spawnSync } from "node:child_process";
import { generateKeyPairSync } from "node:crypto";
import {
  existsSync,
  mkdirSync,
  openSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import http from "node:http";
import { createServer } from "node:net";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { parseSingleBuckOutput, resolveRepoBuckOutput } from "./lib/dev-up-buck-output.mjs";
import {
  parseWindowsProcessIdentity,
  processIdentityMatches,
} from "./lib/dev-up-process-identity.mjs";

const REPO_ROOT = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  "..",
);
const BUCK2_BIN = path.join(REPO_ROOT, "tools", "buck2");
const DEFAULT_APP_TARGET = "//backend/app:console-app";
const DEV_AUTH_APP_TARGET = "//backend/app:console-app-dev-auth";
const SECRETS_DIR = path.join(REPO_ROOT, "ops", ".dev-secrets");
const STATE_DIR = path.join(REPO_ROOT, ".local-dev", "dev-up");
const PID_FILE = path.join(STATE_DIR, "pids.json");
const DOCTOR_CACHE_FILE = path.join(STATE_DIR, "doctor.json");

const COMPOSE_PROJECT = "console-dev";
const COMPOSE_FILES = [
  "ops/compose.yml",
  "ops/compose.dev.yml",
  "ops/compose.dev-deps.yml",
];
// Opt-in: the ONLYOFFICE DocumentServer image is ~2GB, so it stays out of the
// routine dev-up path unless explicitly requested.
const OFFICE_ENABLED = process.env.CONSOLE_DEV_OFFICE === "1";
const DEPS_SERVICES = [
  "postgres",
  "postgres-wal-archive-pruner",
  "seaweedfs",
  "otel-collector",
  "mailpit",
  "mox",
  ...(OFFICE_ENABLED ? ["onlyoffice"] : []),
];

// Shared HS256 secret between the host and the DocumentServer container. Dev
// only; production injects a real per-deploy secret (docs/release/SECRETS.md).
const OFFICE_JWT_SECRET =
  process.env.CONSOLE_OFFICE_JWT_SECRET ?? "office-dev-shared-secret";

// Deliberately NOT the compose files' own defaults (5432/8333/8080): a local
// dev tool cannot assume it owns the only Postgres/S3/8080 on the machine.
// ponytail: fixed port block rather than auto-picking free ports — simpler,
// and `doctor`'s port-free check gives an actionable override path already.
const PORTS = {
  postgres: Number(process.env.CONSOLE_POSTGRES_PORT ?? 55432),
  s3: Number(process.env.CONSOLE_S3_PORT ?? 58333),
  otel: Number(process.env.CONSOLE_OTEL_PORT ?? 54317),
  mailpitSmtp: Number(process.env.CONSOLE_MAILPIT_SMTP_PORT ?? 1025),
  mailpitUi: Number(process.env.CONSOLE_MAILPIT_UI_PORT ?? 8025),
  moxWebapi: Number(process.env.CONSOLE_MOX_WEBAPI_PORT ?? 1080),
  moxSubmission: Number(process.env.CONSOLE_MOX_SUBMISSION_PORT ?? 1587),
  moxImap: Number(process.env.CONSOLE_MOX_IMAP_PORT ?? 1143),
  office: Number(process.env.CONSOLE_OFFICE_DOCSERVER_PORT ?? 8888),
  backend: Number(process.env.CONSOLE_DEV_HTTP_PORT ?? 8090),
};

const POSTGRES_DB = process.env.CONSOLE_POSTGRES_DB ?? "console_dev";
const POSTGRES_ADMIN_USER =
  process.env.CONSOLE_POSTGRES_ADMIN_USER ?? "console_cluster_admin";
const POSTGRES_ADMIN_PASSWORD =
  process.env.CONSOLE_POSTGRES_ADMIN_PASSWORD ?? "console-dev-admin-change-me";
const APP_POSTGRES_PASSWORD =
  process.env.CONSOLE_APP_POSTGRES_PASSWORD ?? "console-dev-owner-change-me";
const RT_POSTGRES_PASSWORD =
  process.env.CONSOLE_RT_POSTGRES_PASSWORD ?? "console-dev-runtime-change-me";
const LEAVE_COMMAND_POSTGRES_PASSWORD =
  process.env.CONSOLE_LEAVE_COMMAND_POSTGRES_PASSWORD ?? "console-dev-leave-command-change-me";
const ONTOLOGY_COMMAND_POSTGRES_PASSWORD =
  process.env.CONSOLE_ONTOLOGY_COMMAND_POSTGRES_PASSWORD ??
  "console-dev-ontology-command-change-me";
const PLATFORM_FORCE_COMMAND_POSTGRES_PASSWORD =
  process.env.CONSOLE_PLATFORM_FORCE_COMMAND_POSTGRES_PASSWORD ??
  "console-dev-platform-force-command-change-me";

function log(msg) {
  console.log(`dev-up: ${msg}`);
}

function sleep(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

function platformRemediation() {
  const plat = os.platform();
  if (plat === "darwin") {
    return "Install Docker Desktop (docker.com/products/docker-desktop), or `brew install colima docker docker-compose` then `colima start`.";
  }
  if (plat === "win32") {
    return "Install Docker Desktop with the WSL2 backend (docker.com/products/docker-desktop), or Podman Desktop as a fallback.";
  }
  return "Install Docker Engine (docs.docker.com/engine/install) or Podman + podman-compose.";
}

// `docker compose` (CLI plugin) is the modern form, but plenty of installs
// (this repo's own ops/dev-up.sh included) only have the standalone
// `docker-compose` binary — try both, then podman.
const COMPOSE_CANDIDATES = [
  { bin: "docker", prefix: ["compose"] },
  { bin: "docker-compose", prefix: [] },
  { bin: "podman", prefix: ["compose"] },
];

function detectCompose() {
  for (const candidate of COMPOSE_CANDIDATES) {
    const check = spawnSync(candidate.bin, [...candidate.prefix, "version"], {
      stdio: "ignore",
    });
    if (!check.error && check.status === 0) return candidate;
  }
  return null;
}

function composeArgs(compose, args) {
  const fileArgs = COMPOSE_FILES.flatMap((f) => ["-f", f]);
  return [...compose.prefix, "-p", COMPOSE_PROJECT, ...fileArgs, ...args];
}

function runCompose(compose, args, opts) {
  return spawnSync(compose.bin, composeArgs(compose, args), opts);
}

function portFree(port) {
  return new Promise((resolve) => {
    const srv = createServer();
    srv.once("error", () => resolve(false));
    srv.once("listening", () => srv.close(() => resolve(true)));
    srv.listen(port, "127.0.0.1");
  });
}

// Without this, a stale process already holding PORTS.backend makes /readyz
// probe THAT process (not the one we're about to spawn) and report a false
// green.
async function assertPortFree(port, label) {
  if (!(await portFree(port))) {
    throw new Error(
      `port ${port} (${label}) is already in use — a stale server may be holding it. ` +
        `Find it with \`lsof -i :${port}\` (macOS/Linux) and stop it, or override the port env var before running \`up\`/\`bootstrap\`.`,
    );
  }
}

// `detached: true` makes the child the leader of its own process group,
// so a plain SIGTERM to its pid can leave grandchildren (console-app)
// orphaned. Signalling the group reaches the whole background stack.
function readProcessIdentity(pid) {
  if (process.platform === "win32") {
    const result = spawnSync(
      "powershell.exe",
      [
        "-NoProfile",
        "-NonInteractive",
        "-Command",
        `$ErrorActionPreference = 'Stop'; $targetPid = ${Number(pid)}; $process = Get-Process -Id $targetPid -ErrorAction Stop; [pscustomobject]@{ StartTime = $process.StartTime.ToString('o'); Path = $process.Path } | ConvertTo-Json -Compress`,
      ],
      { encoding: "utf8" },
    );
    return result.status === 0 ? parseWindowsProcessIdentity(result.stdout) : null;
  }
  const start = spawnSync("ps", ["-o", "lstart=", "-p", String(pid)], {
    encoding: "utf8",
  });
  const command = spawnSync("ps", ["-o", "command=", "-p", String(pid)], {
    encoding: "utf8",
  });
  const startToken = start.status === 0 ? start.stdout.trim() : "";
  const commandText = command.status === 0 ? command.stdout.trim() : "";
  return startToken && commandText ? { startToken, command: commandText } : null;
}

function managedProcessState(proc) {
  const identity = readProcessIdentity(proc.pid);
  if (!identity) {
    throw new Error(`could not verify process identity for pid ${proc.pid}; refusing to persist an unsafe shutdown target`);
  }
  return { ...proc, identity };
}

function stopBackendProcess(proc) {
  if (!proc?.pid) return false;
  const currentIdentity = readProcessIdentity(proc.pid);
  if (!processIdentityMatches(proc.identity, currentIdentity)) {
    log(`refusing to signal stale or unverifiable pid ${proc.pid}`);
    return false;
  }
  try {
    if (proc.group || proc.mode === "buck2") {
      if (process.platform === "win32") {
        spawnSync("taskkill", ["/T", "/F", "/PID", String(proc.pid)]);
      } else {
        process.kill(-proc.pid, "SIGTERM");
      }
    } else {
      process.kill(proc.pid, "SIGTERM");
    }
    log(`stopped pid ${proc.pid}`);
    return true;
  } catch {
    // already gone
    return false;
  }
}

function waitForHttp(url, timeoutMs) {
  return new Promise((resolve, reject) => {
    const deadline = Date.now() + timeoutMs;
    const attempt = () => {
      const req = http.get(url, (res) => {
        res.resume();
        if (res.statusCode && res.statusCode < 400) {
          resolve();
        } else {
          retry();
        }
      });
      req.on("error", retry);
      req.setTimeout(2000, () => req.destroy());
    };
    const retry = () => {
      if (Date.now() > deadline) {
        reject(new Error(`timed out waiting for ${url}`));
        return;
      }
      setTimeout(attempt, 500);
    };
    attempt();
  });
}

function ensureBucket(port, bucket) {
  return new Promise((resolve, reject) => {
    const req = http.request(
      { host: "127.0.0.1", port, path: `/${bucket}`, method: "PUT" },
      (res) => {
        res.resume();
        // 409 = already exists (the deps volume persists across `up` runs
        // unless `down` is given --volumes), which is the common case, not
        // an error.
        if ((res.statusCode && res.statusCode < 400) || res.statusCode === 409) {
          resolve();
        } else {
          reject(
            new Error(`bucket create ${bucket} failed: HTTP ${res.statusCode}`),
          );
        }
      },
    );
    req.on("error", reject);
    req.end();
  });
}

// docker-compose (standalone) delegates to the `docker` daemon, so `inspect`
// always runs against the plain runtime binary, never the `-compose` suffix.
function runtimeBin(compose) {
  return compose.bin === "docker-compose" ? "docker" : compose.bin;
}

function composeServiceIds(compose, services, env) {
  const ps = runCompose(compose, ["ps", "-q", ...services], {
    cwd: REPO_ROOT,
    env,
    encoding: "utf8",
  });
  if (ps.status !== 0) throw new Error("docker compose ps (deps) failed");
  return ps.stdout
    .split(/\r?\n/)
    .map((line) => line.trim())
    .filter(Boolean);
}

async function waitForContainersHealthy(compose, services, timeoutMs, env) {
  const bin = runtimeBin(compose);
  const deadline = Date.now() + timeoutMs;
  let pending = new Set();
  while (pending.size < services.length) {
    if (Date.now() > deadline) {
      throw new Error(
        `timed out waiting for compose containers: ${services.join(", ")}`,
      );
    }
    pending = new Set(composeServiceIds(compose, services, env));
    if (pending.size < services.length) await sleep(1000);
  }
  while (pending.size > 0) {
    if (Date.now() > deadline) {
      throw new Error(
        `timed out waiting for containers healthy: ${[...pending].join(", ")}`,
      );
    }
    for (const name of [...pending]) {
      const inspect = spawnSync(
        bin,
        ["inspect", "--format", "{{.State.Health.Status}}", name],
        { encoding: "utf8" },
      );
      const status = inspect.stdout?.trim();
      if (status === "healthy") {
        pending.delete(name);
      } else if (status === "unhealthy") {
        throw new Error(`container ${name} is unhealthy`);
      }
    }
    if (pending.size > 0) await sleep(1000);
  }
}

function ensureDevKeys() {
  mkdirSync(SECRETS_DIR, { recursive: true });
  const privPath = path.join(SECRETS_DIR, "jwt-private.pem");
  const pubPath = path.join(SECRETS_DIR, "jwt-public.pem");
  if (existsSync(privPath) && existsSync(pubPath)) {
    return {
      privateKeyPem: readFileSync(privPath, "utf8"),
      publicKeyPem: readFileSync(pubPath, "utf8"),
    };
  }
  // Windows-portable replacement for ops/dev-up.sh's `openssl genpkey`/`pkey`
  // calls (openssl is commonly absent on Windows). Same PKCS#8/SPKI PEM shapes
  // the backend's JWT verifier expects.
  const { privateKey, publicKey } = generateKeyPairSync("ec", {
    namedCurve: "P-256",
    privateKeyEncoding: { type: "pkcs8", format: "pem" },
    publicKeyEncoding: { type: "spki", format: "pem" },
  });
  writeFileSync(privPath, privateKey, { mode: 0o600 });
  writeFileSync(pubPath, publicKey, { mode: 0o600 });
  log(`generated dev ES256 keypair under ${path.relative(REPO_ROOT, SECRETS_DIR)}/`);
  return { privateKeyPem: privateKey, publicKeyPem: publicKey };
}

function buck2Version() {
  const check = spawnSync(BUCK2_BIN, ["--version"], { encoding: "utf8" });
  return {
    ok: !check.error && check.status === 0,
    detail: !check.error && check.status === 0 ? check.stdout.trim() : "not executable from tools/buck2",
  };
}

async function runDoctorChecks() {
  const checks = [];

  const compose = detectCompose();
  checks.push({
    name: "Container runtime (docker/podman)",
    ok: Boolean(compose),
    detail: compose
      ? `\`${[compose.bin, ...compose.prefix].join(" ")}\` reachable`
      : "none of `docker compose`, `docker-compose`, `podman compose` responded",
    remediation: compose ? null : platformRemediation(),
  });

  const nodeMajor = Number(process.versions.node.split(".")[0]);
  const nodeOk = nodeMajor >= 22;
  checks.push({
    name: "Node.js",
    ok: nodeOk,
    detail: `v${process.versions.node} (>=22 required, package.json engines)`,
    remediation: nodeOk ? null : "Install Node >=22 (nvm install 22, or nodejs.org).",
  });

  const buck2 = buck2Version();
  checks.push({
    name: "Buck2",
    ok: buck2.ok,
    detail: buck2.detail,
    remediation: buck2.ok ? null : "Restore the repository-pinned tools/buck2 launcher and rerun `npm run dev:doctor`.",
  });

  for (const [label, port] of Object.entries(PORTS)) {
    const free = await portFree(port);
    checks.push({
      name: `Port ${port} (${label})`,
      ok: free,
      detail: free ? "free" : "in use",
      remediation: free
        ? null
        : `Something is already listening on ${port}. Stop it, or override the matching env var (see scripts/dev-up.mjs PORTS) before running \`up\`/\`bootstrap\`.`,
    });
  }

  return {
    platform: `${os.platform()} ${os.release()} (${os.arch()})`,
    timestamp: new Date().toISOString(),
    checks,
  };
}

function formatDoctorReport(result) {
  const lines = [
    `dev-up doctor — ${result.platform} — ${result.timestamp}`,
  ];
  for (const c of result.checks) {
    lines.push(`  [${c.ok ? "OK" : "FAIL"}] ${c.name}: ${c.detail}`);
    if (!c.ok && c.remediation) lines.push(`         -> ${c.remediation}`);
  }
  const failing = result.checks.filter((c) => !c.ok);
  if (failing.length > 0) {
    lines.push("");
    lines.push("=== IT handoff ===");
    lines.push("Share this block with IT/local admin if you cannot resolve it yourself:");
    for (const c of failing) {
      lines.push(`- ${c.name}: ${c.remediation ?? "see full doctor output above"}`);
    }
  }
  return lines.join("\n");
}

async function cmdDoctor(args) {
  mkdirSync(STATE_DIR, { recursive: true });
  if (args.includes("--cached")) {
    if (!existsSync(DOCTOR_CACHE_FILE)) {
      console.error("dev-up: no cached doctor result yet; run `dev-up.mjs doctor` first.");
      process.exitCode = 1;
      return;
    }
    console.log(formatDoctorReport(JSON.parse(readFileSync(DOCTOR_CACHE_FILE, "utf8"))));
    return;
  }
  const result = await runDoctorChecks();
  writeFileSync(DOCTOR_CACHE_FILE, JSON.stringify(result, null, 2));
  console.log(formatDoctorReport(result));
  process.exitCode = result.checks.some((c) => !c.ok) ? 1 : 0;
}

async function bringUpDeps() {
  const compose = detectCompose();
  if (!compose) {
    throw new Error(`no container runtime found. ${platformRemediation()}`);
  }

  const composeEnv = {
    ...process.env,
    CONSOLE_POSTGRES_PORT: String(PORTS.postgres),
    CONSOLE_POSTGRES_DB: POSTGRES_DB,
    CONSOLE_POSTGRES_ADMIN_USER: POSTGRES_ADMIN_USER,
    CONSOLE_POSTGRES_ADMIN_PASSWORD: POSTGRES_ADMIN_PASSWORD,
    CONSOLE_APP_POSTGRES_PASSWORD: APP_POSTGRES_PASSWORD,
    CONSOLE_RT_POSTGRES_PASSWORD: RT_POSTGRES_PASSWORD,
    CONSOLE_LEAVE_COMMAND_POSTGRES_PASSWORD: LEAVE_COMMAND_POSTGRES_PASSWORD,
    CONSOLE_ONTOLOGY_COMMAND_POSTGRES_PASSWORD: ONTOLOGY_COMMAND_POSTGRES_PASSWORD,
    CONSOLE_PLATFORM_FORCE_COMMAND_POSTGRES_PASSWORD:
      PLATFORM_FORCE_COMMAND_POSTGRES_PASSWORD,
    CONSOLE_S3_PORT: String(PORTS.s3),
    CONSOLE_OTEL_PORT: String(PORTS.otel),
    CONSOLE_MAILPIT_SMTP_PORT: String(PORTS.mailpitSmtp),
    CONSOLE_MAILPIT_UI_PORT: String(PORTS.mailpitUi),
    CONSOLE_OFFICE_DOCSERVER_PORT: String(PORTS.office),
    CONSOLE_OFFICE_JWT_SECRET: OFFICE_JWT_SECRET,
  };

  if (!OFFICE_ENABLED) {
    log(
      "office editor (ONLYOFFICE DocumentServer, ~2GB) is disabled — set CONSOLE_DEV_OFFICE=1 to enable",
    );
  }

  const up = runCompose(compose, ["up", "-d", ...DEPS_SERVICES], {
    cwd: REPO_ROOT,
    env: composeEnv,
    stdio: "inherit",
  });
  if (up.status !== 0) throw new Error("docker compose up (deps) failed");

  log("waiting for deps to report healthy...");
  await waitForContainersHealthy(compose, DEPS_SERVICES, 180_000, composeEnv);

  log("ensuring SeaweedFS evidence buckets exist...");
  await ensureBucket(PORTS.s3, "mnt-evidence");
  await ensureBucket(PORTS.s3, "mnt-evidence-replica");

  return compose;
}

function databaseUrl() {
  return `postgres://console_app:${APP_POSTGRES_PASSWORD}@127.0.0.1:${PORTS.postgres}/${POSTGRES_DB}`;
}

function runtimeDatabaseUrl() {
  return `postgres://console_rt:${RT_POSTGRES_PASSWORD}@127.0.0.1:${PORTS.postgres}/${POSTGRES_DB}`;
}

function commandDatabaseUrl(role, password) {
  return `postgres://${role}:${password}@127.0.0.1:${PORTS.postgres}/${POSTGRES_DB}`;
}

// Dev-up builds exactly one Buck2 target for each invocation, then executes that
// resolved artifact for both migration and serving. This prevents a stale
// non-Buck binary or separately compiled API binary from satisfying
// readiness. Any ambiguous Buck output is rejected before execution.
function resolveBuckOutput(target, stdout) {
  return resolveRepoBuckOutput(REPO_ROOT, parseSingleBuckOutput(target, stdout));
}

function buildAppBinary(devAuth) {
  const target = devAuth ? DEV_AUTH_APP_TARGET : DEFAULT_APP_TARGET;
  log(`building ${target} with Buck2...`);
  const result = spawnSync(BUCK2_BIN, ["build", target, "--show-output"], {
    cwd: REPO_ROOT,
    encoding: "utf8",
    stdio: ["ignore", "pipe", "pipe"],
  });
  if (result.error || result.status !== 0) {
    const detail = result.stderr?.trim() || result.error?.message || `exit ${result.status}`;
    throw new Error(`Buck2 build failed for ${target}: ${detail}`);
  }
  const outputPath = resolveBuckOutput(target, result.stdout);
  log(`Buck2 built ${target} -> ${path.relative(REPO_ROOT, outputPath)}`);
  return { target, outputPath };
}

function runMigrations(appBinary) {
  log(`running migrations with ${appBinary.target}...`);
  const result = spawnSync(appBinary.outputPath, [], {
    cwd: REPO_ROOT,
    env: buildAppEnv("migrate"),
    stdio: "inherit",
  });
  if (result.error || result.status !== 0) throw new Error("migration run failed (CONSOLE_APP_ROLE=migrate)");
}

function reconcileDatabaseTopology(compose) {
  log("reconciling and verifying the seven-role database topology...");
  const result = runCompose(compose, ["run", "--rm", "postgres-topology"], {
    cwd: REPO_ROOT,
    env: {
      ...process.env,
      // Keep the topology one-shot on the same Compose model used to start
      // Postgres. Falling back to compose.dev.yml's 5432 default makes Compose
      // recreate the dependency that dev-up published on PORTS.postgres.
      CONSOLE_POSTGRES_PORT: String(PORTS.postgres),
      CONSOLE_POSTGRES_DB: POSTGRES_DB,
      CONSOLE_POSTGRES_ADMIN_USER: POSTGRES_ADMIN_USER,
      CONSOLE_POSTGRES_ADMIN_PASSWORD: POSTGRES_ADMIN_PASSWORD,
      CONSOLE_APP_POSTGRES_PASSWORD: APP_POSTGRES_PASSWORD,
      CONSOLE_RT_POSTGRES_PASSWORD: RT_POSTGRES_PASSWORD,
      CONSOLE_LEAVE_COMMAND_POSTGRES_PASSWORD: LEAVE_COMMAND_POSTGRES_PASSWORD,
      CONSOLE_ONTOLOGY_COMMAND_POSTGRES_PASSWORD: ONTOLOGY_COMMAND_POSTGRES_PASSWORD,
      CONSOLE_PLATFORM_FORCE_COMMAND_POSTGRES_PASSWORD:
        PLATFORM_FORCE_COMMAND_POSTGRES_PASSWORD,
    },
    stdio: "inherit",
  });
  if (result.status !== 0) throw new Error("database topology reconciliation failed");
}

// Load the KNL tenant dev fixtures (scripts/dev-seed.sql) so every console
// screen shows real org-scoped rows instead of 0-counts. Idempotent
// (ON CONFLICT DO NOTHING) — safe to run on every up/bootstrap. Piped into the
// compose Postgres service's own psql, so no host psql is required.
function runSeed(compose) {
  const seedPath = path.join(REPO_ROOT, "scripts", "dev-seed.sql");
  if (!existsSync(seedPath)) return;
  log("seeding dev fixtures (scripts/dev-seed.sql)...");
  const result = spawnSync(
    compose.bin,
    composeArgs(compose, [
      "exec",
      "-T",
      "postgres",
      "psql",
      "-v",
      "ON_ERROR_STOP=1",
      "-U",
      POSTGRES_ADMIN_USER,
      "-d",
      POSTGRES_DB,
    ]),
    { input: readFileSync(seedPath), stdio: ["pipe", "ignore", "inherit"] },
  );
  if (result.status !== 0) throw new Error("dev seed failed (scripts/dev-seed.sql)");
}

// The env relocation is the essence of W2: everything ops/compose.dev.yml
// injects into the `app` *container* (JWT keys, WebAuthn RP, cookie flag,
// cold-start OTP) plus the docker-network hostnames the container-only
// x-app-env anchor uses (ops/compose.yml:10-13) — DATABASE_URL, the S3/
// SeaweedFS endpoint, and OTEL_EXPORTER_OTLP_ENDPOINT — rewritten to the
// published localhost ports for a host-launched process.
function buildAppEnv(role) {
  const { privateKeyPem, publicKeyPem } = ensureDevKeys();
  return {
    ...process.env,
    CONSOLE_APP_ROLE: role,
    DATABASE_URL: role === "migrate" ? databaseUrl() : runtimeDatabaseUrl(),
    LEAVE_COMMAND_DATABASE_URL: commandDatabaseUrl(
      "console_leave_cmd",
      LEAVE_COMMAND_POSTGRES_PASSWORD,
    ),
    ONTOLOGY_COMMAND_DATABASE_URL: commandDatabaseUrl(
      "console_ontology_cmd",
      ONTOLOGY_COMMAND_POSTGRES_PASSWORD,
    ),
    CONSOLE_PLATFORM_FORCE_COMMAND_POSTGRES_PASSWORD:
      PLATFORM_FORCE_COMMAND_POSTGRES_PASSWORD,
    PLATFORM_FORCE_COMMAND_DATABASE_URL: commandDatabaseUrl(
      "console_platform_force_cmd",
      PLATFORM_FORCE_COMMAND_POSTGRES_PASSWORD,
    ),
    CONSOLE_HTTP_ADDR: `127.0.0.1:${PORTS.backend}`,
    // Local dev intentionally uses the OTP-logging stub unless a caller supplies
    // a complete SMTP relay config via CONSOLE_EMAIL_*.
    CONSOLE_EMAIL_STUB_MODE: process.env.CONSOLE_EMAIL_STUB_MODE ?? "dev",
    OTEL_EXPORTER_OTLP_ENDPOINT: `http://127.0.0.1:${PORTS.otel}`,
    OTEL_SERVICE_NAME: "console-app-dev",
    CONSOLE_S3_ENDPOINT_URL: `http://127.0.0.1:${PORTS.s3}`,
    CONSOLE_S3_REGION: "us-east-1",
    CONSOLE_S3_ACCESS_KEY_ID: "dev",
    CONSOLE_S3_SECRET_ACCESS_KEY: "dev",
    CONSOLE_S3_PRIMARY_BUCKET: "mnt-evidence",
    CONSOLE_S3_REPLICA_BUCKET: "mnt-evidence-replica",
    CONSOLE_S3_FORCE_PATH_STYLE: "true",
    CONSOLE_JWT_PRIVATE_KEY_PEM: privateKeyPem,
    CONSOLE_JWT_PUBLIC_KEY_PEM: publicKeyPem,
    CONSOLE_JWT_ISSUER: "console-dev",
    CONSOLE_JWT_AUDIENCE: "console",
    CONSOLE_WEBAUTHN_RP_ID: "localhost",
    CONSOLE_WEBAUTHN_RP_ORIGIN: `http://localhost:${PORTS.backend}`,
    CONSOLE_WEBAUTHN_RP_NAME: "정비 콘솔 (dev)",
    CONSOLE_COOKIE_SECURE: "false",
    CONSOLE_COLDSTART_OTP: process.env.CONSOLE_COLDSTART_OTP ?? "coss0000",
    // mox integration (slice 1): route the webmail send transport at the dev mox
    // server's webapi, and arm the delivery-webhook shared secret. The account's
    // mox login (mox@localhost / moxmoxmox) is set when a mailbox is configured
    // through the REST /account endpoint (see scripts/mox-e2e.mjs).
    CONSOLE_MAIL_MOX_BASE_URL:
      process.env.CONSOLE_MAIL_MOX_BASE_URL ?? `http://127.0.0.1:${PORTS.moxWebapi}`,
    CONSOLE_MAIL_MOX_WEBHOOK_SECRET:
      process.env.CONSOLE_MAIL_MOX_WEBHOOK_SECRET ?? "mox-dev-webhook-secret-change-me",
    // In-console office editor (ONLYOFFICE), only when CONSOLE_DEV_OFFICE=1 started
    // the DocumentServer dep. Omitting all three CONSOLE_OFFICE_* vars otherwise
    // leaves the office routes on their existing graceful-503 path (same as an
    // unconfigured prod deploy) instead of pointing at a container that was
    // never started. The shared JWT secret matches the DocumentServer
    // container's JWT_SECRET; DocumentServer reaches the host-run app back
    // over host.docker.internal at the backend port.
    ...(OFFICE_ENABLED
      ? {
          CONSOLE_OFFICE_JWT_SECRET: OFFICE_JWT_SECRET,
          CONSOLE_OFFICE_DOCSERVER_URL: `http://127.0.0.1:${PORTS.office}`,
          CONSOLE_OFFICE_CALLBACK_BASE_URL: `http://host.docker.internal:${PORTS.backend}`,
        }
      : {}),
    RUST_LOG: process.env.RUST_LOG ?? "info,tower_http=info",
    SQLX_OFFLINE: "true",
  };
}

function writePidState(state) {
  mkdirSync(STATE_DIR, { recursive: true });
  writeFileSync(PID_FILE, JSON.stringify(state, null, 2));
}

function readPidState() {
  if (!existsSync(PID_FILE)) return null;
  try {
    return JSON.parse(readFileSync(PID_FILE, "utf8"));
  } catch {
    return null;
  }
}

function printUrls() {
  console.log("");
  console.log("Local full-stack dev environment ready:");
  console.log(`  Backend:      http://127.0.0.1:${PORTS.backend}`);
  console.log(`  /readyz:      http://127.0.0.1:${PORTS.backend}/readyz`);
  console.log(`  Mailpit UI:   http://localhost:${PORTS.mailpitUi}`);
  console.log(`  mox webapi:   http://localhost:${PORTS.moxWebapi}/webapi/  (mox@localhost / moxmoxmox, admin moxadmin)`);
  console.log(`  mox IMAP:     127.0.0.1:${PORTS.moxImap}   submission 127.0.0.1:${PORTS.moxSubmission}`);
  console.log("");
  console.log(`First sign-in one-time code: ${process.env.CONSOLE_COLDSTART_OTP ?? "coss0000"}`);
  console.log("");
}

async function cmdUp() {
  await assertPortFree(PORTS.backend, "backend");
  const compose = await bringUpDeps();
  reconcileDatabaseTopology(compose);
  const appBinary = buildAppBinary(false);
  runMigrations(appBinary);
  runSeed(compose);
  const appEnv = buildAppEnv("api");
  log(`launching ${appBinary.target}...`);
  const backend = spawn(appBinary.outputPath, [], { cwd: REPO_ROOT, env: appEnv, stdio: "inherit", detached: process.platform !== "win32" });
  const backendState = managedProcessState({ pid: backend.pid, mode: "buck2", target: appBinary.target, outputPath: appBinary.outputPath, group: process.platform !== "win32" });
  writePidState({ startedBy: "up", backend: backendState });
  let shuttingDown = false;
  const shutdown = (exitCode = 0) => {
    if (shuttingDown) return;
    shuttingDown = true;
    log("stopping Buck2-built backend (docker deps stay up — run `dev-up.mjs down` to stop them)...");
    stopBackendProcess(backendState); process.exit(exitCode);
  };
  process.once("SIGINT", () => shutdown(0)); process.once("SIGTERM", () => shutdown(0));
  backend.on("error", (err) => { log(`ERROR: could not start ${appBinary.target}: ${err.message}`); shutdown(1); });
  backend.on("exit", (code) => { if (code !== 0 && code !== null) { log(`${appBinary.target} exited with code ${code}`); shutdown(1); } });
  try { log(`waiting for /readyz on http://127.0.0.1:${PORTS.backend}/readyz ...`); await waitForHttp(`http://127.0.0.1:${PORTS.backend}/readyz`, 180_000); }
  catch (err) { log(`ERROR: ${err.message}`); shutdown(1); return; }
  printUrls(); await new Promise(() => {});
}

// CONSOLE_DEV_AUTH_E2E=1 selects the dedicated Buck2 dev-auth target and seeds
// its personas. Plain `bootstrap` (the CI "dev-up-smoke" job) is unaffected.
async function cmdBootstrap() {
  await assertPortFree(PORTS.backend, "backend");
  const devAuth = process.env.CONSOLE_DEV_AUTH_E2E === "1";
  const compose = await bringUpDeps();
  reconcileDatabaseTopology(compose);
  const appBinary = buildAppBinary(devAuth);
  runMigrations(appBinary);
  // Seed ONLY the dev-auth stack. dev-seed.sql pre-seeds a `dev-auth:*` persona
  // (so `POST /dev-auth/session` upserts the SAME row), which a DEFAULT-feature
  // build refuses to boot against — `assert_no_dev_auth_personas` treats such a
  // row as a leaked dev dump. The plain bootstrap is the release-parity boot
  // smoke and must stay on a clean, un-seeded DB (it only probes /readyz).
  if (devAuth) runSeed(compose);
  const appEnv = buildAppEnv("api");
  mkdirSync(STATE_DIR, { recursive: true });
  const logFile = path.join(STATE_DIR, "backend.log");
  const out = openSync(logFile, "a");
  log(`starting ${appBinary.target} in the background, logging to ${path.relative(REPO_ROOT, logFile)}...`);
  const backend = spawn(appBinary.outputPath, [], {
    cwd: REPO_ROOT,
    env: appEnv,
    stdio: ["ignore", out, out],
    detached: process.platform !== "win32",
  });
  const backendState = managedProcessState({
    pid: backend.pid,
    mode: "buck2",
    target: appBinary.target,
    outputPath: appBinary.outputPath,
    logFile,
    group: process.platform !== "win32",
  });
  let ready = false;
  const backendStart = new Promise((_, reject) => {
    let settled = false;
    const failBeforeReady = (message) => {
      if (!settled) {
        settled = true;
        reject(new Error(message));
      }
    };
    const handleEnd = (event, code, signal) => {
      const detail =
        code === null ? `${event} by signal ${signal}` : `${event} with code ${code}`;
      if (ready) {
        log(`${appBinary.target} ${detail} after readiness`);
      } else {
        failBeforeReady(`${appBinary.target} ${detail} before /readyz`);
      }
    };
    backend.once("error", (err) => {
      if (ready) {
        log(`${appBinary.target} process error after readiness: ${err.message}`);
      } else {
        failBeforeReady(`could not start ${appBinary.target}: ${err.message}`);
      }
    });
    backend.once("exit", (code, signal) => handleEnd("exited", code, signal));
    backend.once("close", (code, signal) => handleEnd("closed", code, signal));
  });
  backend.unref();
  writePidState({ startedBy: "bootstrap", backend: backendState });

  try {
    await Promise.race([
      waitForHttp(`http://127.0.0.1:${PORTS.backend}/readyz`, 180_000),
      backendStart,
    ]);
  } catch (err) {
    stopBackendProcess(backendState);
    throw err;
  }
  ready = true;
  log(`/readyz green at http://127.0.0.1:${PORTS.backend}/readyz`);

  printUrls();
}

async function cmdDown() {
  const state = readPidState();
  if (state?.backend?.pid) {
    stopBackendProcess(state.backend);
  }
  const compose = detectCompose();
  if (compose) {
    log("stopping docker deps...");
    runCompose(compose, ["down"], {
      cwd: REPO_ROOT,
      env: {
        ...process.env,
        CONSOLE_POSTGRES_ADMIN_PASSWORD: POSTGRES_ADMIN_PASSWORD,
        CONSOLE_APP_POSTGRES_PASSWORD: APP_POSTGRES_PASSWORD,
        CONSOLE_RT_POSTGRES_PASSWORD: RT_POSTGRES_PASSWORD,
        CONSOLE_LEAVE_COMMAND_POSTGRES_PASSWORD: LEAVE_COMMAND_POSTGRES_PASSWORD,
        CONSOLE_ONTOLOGY_COMMAND_POSTGRES_PASSWORD: ONTOLOGY_COMMAND_POSTGRES_PASSWORD,
        CONSOLE_PLATFORM_FORCE_COMMAND_POSTGRES_PASSWORD:
          PLATFORM_FORCE_COMMAND_POSTGRES_PASSWORD,
      },
      stdio: "inherit",
    });
  } else {
    log("no container runtime detected; skipping compose down");
  }
  if (existsSync(PID_FILE)) rmSync(PID_FILE);
}

async function main() {
  const [, , cmd, ...rest] = process.argv;
  switch (cmd) {
    case "doctor":
      await cmdDoctor(rest);
      break;
    case "up":
      await cmdUp();
      break;
    case "bootstrap":
      await cmdBootstrap();
      break;
    case "down":
      await cmdDown();
      break;
    default:
      console.error("usage: dev-up.mjs <doctor|up|bootstrap|down> [--cached]");
      process.exitCode = 1;
  }
}

main().catch((err) => {
  console.error(`dev-up: ERROR: ${err.message}`);
  process.exitCode = 1;
});
