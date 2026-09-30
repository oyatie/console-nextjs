#!/usr/bin/env node
/**
 * Mechanical tip-serial contention assessor.
 *
 * Realizes failure class ops.tip-serial-contention (+ feeds ops.missed-tip-sync):
 * open PRs that touch tip-serial roots are "tip writers"; BEHIND tip writers
 * need restack before any new tip PR.
 *
 * Usage:
 *   node tools/ci/assess-tip-contention.mjs           # JSON report; exit 0
 *   node tools/ci/assess-tip-contention.mjs --check    # exit 2 if any tip writer is BEHIND (restack debt)
 *   # tip_writers>=2 alone does NOT fail --check (open tip PRs fan-out CI/review; only MERGE is serial)
 *   node tools/ci/assess-tip-contention.mjs --dry-run  # no gh required if --fixture
 */
import { execFileSync } from "node:child_process";
import { resolve, dirname } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "../..");

/** Paths that force tip-serial merge order (manifest / baseline / ledger / CI graph / locks). */
export const TIP_SERIAL_PATH_PREFIXES = [
  "docs/documentation-manifest.seed.json",
  "docs/documentation-index.json",
  "docs/program/executed-tests-baseline.json",
  "docs/program/js-test-reachability-baseline.json",
  "docs/program/gate-input-exceptions.json",
  "docs/program/console-program-ledger.md",
  "docs/program/console-capability-registry.json",
  "docs/program/console-enterprise-roadmap.md",
  "docs/program/ledger/",
  "docs/program/console-jurisdiction-register.json",
  "docs/CI-GATES.md",
  "docs/GO-LIVE-CHECKLIST.md",
  "docs/benchmarks/enterprise-parity-matrix.md",
  "docs/specs/",
  "docs/program/console-fanout-epoch-contract.md",
  "docs/program/console-buck2-scale-playbook.md",
  "docs/current/",
  ".github/workflows/",
  ".github/actions/",
  ".github/trust/console.allowed_signers",
  "scripts/",
  ".grok/",
  ".claude/workflows/",
  "backend/Cargo.toml",
  "backend/Cargo.lock",
  "backend/.sqlx/",
  "package.json",
  "package-lock.json",
  "tools/buck/",
  ".buckconfig",
  "**/BUCK",

  "**/*.bzl",
  "third-party/rust/reindeer.toml",
  "third-party/rust/reindeer/",
  "tools/ci/",
  "backend/ci/gates/",
  "registry/",
  "backend/**/migrations/",
  "backend/**/openapi/",

  "security/",
  "backend/rust-toolchain.toml",
  "backend/deny.toml",
  "renovate.json5",
  "release-please-config.json",
  "ops/",
  "deploy/",
  ".config/nextest.toml",
  ".release-please-manifest.json",
];

const args = new Set(process.argv.slice(2));
const checkOnly = args.has("--check");

/**
 * Compile a serial-path entry into a prefix-anchored RegExp. Entries may carry
 * glob metacharacters: `**` matches zero or more full path segments (so
 * `backend/**\/migrations/` reaches `backend/crates/platform/db/migrations/...`),
 * `*` matches within one segment, and `?` matches a single non-separator char.
 * Entries without metacharacters keep the literal prefix semantics below.
 */
export function globToRegExp(glob) {
  let re = "";
  for (let i = 0; i < glob.length; i++) {
    const ch = glob[i];
    if (ch === "*") {
      if (glob[i + 1] === "*") {
        if (glob[i + 2] === "/") {
          re += "(?:[^/]+/)*";
          i += 2; // consumed "**/"
        } else {
          re += ".*";
          i += 1; // consumed "**"
        }
      } else {
        re += "[^/]*";
      }
    } else if (ch === "?") {
      re += "[^/]";
    } else if ("+()^$.{}[]|\\".includes(ch)) {
      re += "\\" + ch;
    } else {
      re += ch;
    }
  }
  // File globs (ending in a non-separator) are exact-match anchored; directory
  // globs (ending in "/") keep prefix semantics so a child path still matches.
  return new RegExp(`^${re}${glob.endsWith("/") ? "" : "$"}`);
}

// Pre-compile glob entries once; keep literal entries for exact/prefix matching.
const GLOB_PREFIX_MATCHERS = TIP_SERIAL_PATH_PREFIXES.map((prefix) =>
  prefix.includes("*") || prefix.includes("?")
    ? { glob: true, matcher: globToRegExp(prefix) }
    : { glob: false, prefix },
);

export function pathIsTipSerial(path) {
  if (!path || typeof path !== "string") return false;
  const p = path.replace(/^\.\//, "");
  return GLOB_PREFIX_MATCHERS.some((entry) => {
    if (entry.glob) return entry.matcher.test(p);
    if (p === entry.prefix) return true;
    // Directory entries (trailing "/") keep prefix semantics; file entries are
    // exact-match only so near-matches like `package.json.bak` do not classify.
    if (entry.prefix.endsWith("/")) {
      return p.startsWith(entry.prefix) || entry.prefix.startsWith(p + "/");
    }
    return false;
  });
}

export function classifyPrFiles(files) {
  const tip_files = [];
  for (const f of files || []) {
    const path = typeof f === "string" ? f : f.path || f.filename || "";
    if (pathIsTipSerial(path)) tip_files.push(path);
  }
  return { is_tip_writer: tip_files.length > 0, tip_files };
}

/**
 * @param {object[]} prs - gh pr list/view shaped objects with number, title, mergeStateStatus, files?
 * @returns {{ tip_writers: number, writers: object[], behind: object[], class_ids: string[] }}
 */
export function assessTipContention(prs) {
  const writers = [];
  const behind = [];
  for (const pr of prs || []) {
    const files = pr.files || pr.filePaths || [];
    const { is_tip_writer, tip_files } = classifyPrFiles(files);
    const ms = pr.mergeStateStatus || "";
    const row = {
      number: pr.number,
      title: pr.title || "",
      mergeStateStatus: ms,
      is_tip_writer,
      tip_files,
      url: pr.url || "",
    };
    if (is_tip_writer) writers.push(row);
    if (ms === "BEHIND") behind.push(row);
  }
  writers.sort((a, b) => a.number - b.number);
  const class_ids = [];
  if (writers.length >= 2) class_ids.push("ops.tip-serial-contention");
  if (behind.some((b) => b.is_tip_writer) || behind.length > 0) {
    if (behind.length > 0) class_ids.push("ops.missed-tip-sync");
  }
  return {
    tip_writers: writers.length,
    writers,
    behind,
    behind_count: behind.length,
    class_ids: [...new Set(class_ids)],
    tip_serial_prefixes: TIP_SERIAL_PATH_PREFIXES,
  };
}

function ghJson(argv) {
  const out = execFileSync("gh", argv, {
    cwd: root,
    encoding: "utf8",
    maxBuffer: 8 * 1024 * 1024,
  });
  return JSON.parse(out);
}

function prFiles(number) {
  try {
    const files = ghJson([
      "pr",
      "view",
      String(number),
      "--json",
      "files",
    ]);
    return (files.files || []).map((f) => f.path);
  } catch {
    return [];
  }
}

function main() {
  let prs;
  try {
    prs = ghJson([
      "pr",
      "list",
      "--state",
      "open",
      "--limit",
      "40",
      "--json",
      "number,title,mergeable,mergeStateStatus,headRefOid,url",
    ]);
  } catch (e) {
    console.error("gh pr list failed:", e.message || e);
    process.exit(1);
  }

  const enriched = prs.map((pr) => ({
    ...pr,
    files: prFiles(pr.number),
  }));

  const report = assessTipContention(enriched);
  const tipBehind = report.behind.filter((b) => b.is_tip_writer);
  const out = {
    ok: true,
    ...report,
    tip_behind_count: tipBehind.length,
    product_capacity_hint:
      tipBehind.length > 0
        ? "BEHIND tip writers — restack before next tip merge (open PRs still OK)"
        : report.tip_writers >= 2
          ? "tip_writers>=2 — open/CI/review fan-out OK; serialize tip MERGES only; restack after each tip land"
          : "tip serial free or single writer — open tip PR + merge when green",
    merge_policy:
      "Serialize tip merges only; never hold tip PRs unopened for contention reasons",
  };

  console.log(JSON.stringify(out, null, 2));

  // Fail closed on restack debt (velocity + correctness), not on parallel open tip PRs
  if (checkOnly && tipBehind.length > 0) {
    console.error(
      JSON.stringify(
        {
          class_id: "ops.missed-tip-sync",
          tip_writers: report.tip_writers,
          tip_behind: tipBehind.map((w) => w.number),
          message:
            "BEHIND tip writers must be restacked (signed C/T + prebind) before next tip merge — open tip PRs remain allowed",
        },
        null,
        2,
      ),
    );
    process.exit(2);
  }
}

if (
  import.meta.url === `file://${process.argv[1]}` ||
  process.argv[1]?.endsWith("assess-tip-contention.mjs")
) {
  main();
}
