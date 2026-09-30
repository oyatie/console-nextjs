#!/usr/bin/env node
/**
 * Mechanical soft-red / block ingest → ci/harness/lane-board.live.json
 *
 * Realizes aspirational "no silent soft reds": any open PR that is BEHIND,
 * DIRTY, CONFLICTING, or has failing/pending Required-ish signals gets a
 * board item with stable source_key.
 *
 * Usage:
 *   node tools/ci/ingest-soft-reds.mjs           # write board
 *   node tools/ci/ingest-soft-reds.mjs --check   # exit 2 if silence (observed but not on board)
 *   node tools/ci/ingest-soft-reds.mjs --dry-run
 */
import { execFileSync } from "node:child_process";
import { readFileSync, writeFileSync, existsSync, mkdirSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
const boardPath = resolve(root, "ci/harness/lane-board.live.json");

const args = new Set(process.argv.slice(2));
const dryRun = args.has("--dry-run");
const checkOnly = args.has("--check");

function ghJson(argv) {
  const out = execFileSync("gh", argv, {
    cwd: root,
    encoding: "utf8",
    maxBuffer: 8 * 1024 * 1024,
  });
  return JSON.parse(out);
}

function loadBoard() {
  if (!existsSync(boardPath)) {
    return {
      version: "1.0.0",
      updated_at: null,
      updated_by: "ingest-soft-reds",
      items: [],
      notes: "",
    };
  }
  return JSON.parse(readFileSync(boardPath, "utf8"));
}

/**
 * @param {object} pr
 * @returns {{ source_key: string, kind: string, title: string, priority: string, status: string, pr_number: number, evidence: string }[]}
 */
export function softRedsFromPr(pr) {
  const n = pr.number;
  const ms = pr.mergeStateStatus || "";
  const m = pr.mergeable || "";
  const title = pr.title || "";
  const out = [];

  const push = (suffix, kind, priority, status, why) => {
    out.push({
      source_key: `${kind === "hard_block" ? "block" : "soft_red"}:pr:${n}:${suffix}`,
      kind,
      title: `#${n} ${suffix}: ${title}`.slice(0, 120),
      priority,
      status,
      pr_number: n,
      evidence: why,
      allowlist: [],
      forbidden: ["docs/current/**"],
      holds_checked: "ingest only — no HOLD clearance",
      last_review_verdict: null,
      fix_rounds: 0,
      derived: true,
    });
  };

  if (ms === "BEHIND") push("behind", "soft_red", "P1", "ready", `mergeStateStatus=BEHIND`);
  if (ms === "DIRTY" || m === "CONFLICTING")
    push("dirty", "hard_block", "P0", "ready", `ms=${ms} mergeable=${m}`);
  if (ms === "BLOCKED" && m === "MERGEABLE")
    push("blocked_checks", "soft_red", "P1", "ready", `mergeStateStatus=BLOCKED (checks or protection)`);

  // statusCheckRollup may be absent when using limited json
  const rollup = pr.statusCheckRollup || [];
  for (const c of rollup) {
    const name = c.name || c.context || "check";
    const conclusion = (c.conclusion || "").toUpperCase();
    const status = (c.status || "").toUpperCase();
    const slug = String(name)
      .toLowerCase()
      .replace(/[^a-z0-9]+/g, "_")
      .slice(0, 40);
    if (conclusion === "FAILURE" || conclusion === "CANCELLED" || conclusion === "TIMED_OUT" || conclusion === "STARTUP_FAILURE" || conclusion === "ACTION_REQUIRED") {
      const hard =
        /required/i.test(name) ||
        /preflight/i.test(name) ||
        /authority/i.test(name) ||
        /security/i.test(name);
      push(
        `check_${slug}`,
        hard ? "hard_block" : "soft_red",
        hard ? "P0" : "P1",
        "ready",
        `${name} conclusion=${conclusion || status}`,
      );
    } else if (
      status === "PENDING" ||
      status === "IN_PROGRESS" ||
      status === "QUEUED" ||
      status === "WAITING" ||
      status === "REQUESTED" ||
      status === "REQUEUED"
    ) {
      // A rerunning check has no terminal conclusion; keep it on the board so
      // absence-based expiration does not drop a previously failing observation.
      // Preserve the hard/soft kind so the source_key matches the prior failure.
      const hard =
        /required/i.test(name) ||
        /preflight/i.test(name) ||
        /authority/i.test(name) ||
        /security/i.test(name);
      push(
        `check_${slug}`,
        hard ? "hard_block" : "soft_red",
        hard ? "P0" : "P2",
        "fixing",
        `${name} status=${status} (rerunning)`,
      );
    }
  }

  return out;
}

export function mergeItems(existing, incoming) {
  const byKey = new Map();
  const incomingKeys = new Set((incoming || []).map((it) => it?.source_key).filter(Boolean));
  for (const it of existing || []) {
    if (it && it.source_key) {
      // Expire DERIVED PR observations absent from the new open-PR scan, so a
      // PR that became clean/merged no longer lingers as a soft red/block.
      // Custom board observations (recorded by work-manager/pr-babysit with an
      // arbitrary suffix) are preserved. Pre-marker entries written by the
      // previous ingester lack `derived`, so match the derived suffix set too.
      const derivedSuffix = /^(soft_red|block):pr:\d+:(behind|dirty|blocked_checks|check_[a-z0-9_]+)$/i.test(it.source_key);
      if ((it.derived === true || derivedSuffix) && !incomingKeys.has(it.source_key)) continue;
      byKey.set(it.source_key, { ...it });
    }
  }
  const added = [];
  const updated = [];
  for (const it of incoming) {
    const prev = byKey.get(it.source_key);
    if (!prev) {
      // A "fixing" item is a rerunning check with no terminal conclusion; only
      // keep it when the check was already on the board (a prior failure being
      // re-run). A first-run pending check is not an actionable soft red.
      if (it.status === "fixing") continue;
      byKey.set(it.source_key, { ...it, updated_at: new Date().toISOString() });
      added.push(it.source_key);
    } else {
      byKey.set(it.source_key, {
        ...prev,
        ...it,
        status: prev.status === "merged" || prev.status === "deferred" ? prev.status : it.status,
        fix_rounds: prev.fix_rounds || 0,
        updated_at: new Date().toISOString(),
      });
      updated.push(it.source_key);
    }
  }
  return { items: [...byKey.values()], added, updated };
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
      "1000",
      "--json",
      "number,title,mergeable,mergeStateStatus,headRefOid,statusCheckRollup,url",
    ]);
  } catch (e) {
    console.error("gh pr list failed:", e.message || e);
    process.exit(1);
  }

  /** skip release-please bot noise unless dirty */
  const observed = [];
  for (const pr of prs) {
    if (/release-please/i.test(pr.title || "") && pr.mergeStateStatus === "BEHIND") {
      // still enqueue as soft_red so not silent, but lower priority
      const items = softRedsFromPr(pr).map((x) => ({ ...x, priority: "P3", kind: "soft_red" }));
      observed.push(...items);
      continue;
    }
    observed.push(...softRedsFromPr(pr));
  }

  const board = loadBoard();
  const { items, added, updated } = mergeItems(board.items || [], observed);

  if (checkOnly) {
    const keys = new Set((board.items || []).map((i) => i.source_key));
    // First-run pending checks are "fixing" observations with no board item yet;
    // they are not silent soft-reds, so exclude them from the silence check.
    const missing = observed.filter((o) => !keys.has(o.source_key) && o.status !== "fixing");
    if (missing.length) {
      console.error(
        JSON.stringify(
          {
            silence: true,
            missing: missing.map((m) => m.source_key),
            message: "ops.soft-red-silence: observed soft reds/blocks not on board",
          },
          null,
          2,
        ),
      );
      process.exit(2);
    }
    console.log(JSON.stringify({ silence: false, observed: observed.length, board: keys.size }, null, 2));
    process.exit(0);
  }

  const next = {
    version: "1.0.0",
    updated_at: new Date().toISOString(),
    updated_by: "ingest-soft-reds",
    items,
    notes: `ingested ${observed.length} observations; added ${added.length}; updated ${updated.length}`,
  };

  if (dryRun) {
    console.log(JSON.stringify({ dry_run: true, added, updated, sample: observed.slice(0, 8) }, null, 2));
    process.exit(0);
  }

  mkdirSync(dirname(boardPath), { recursive: true });
  writeFileSync(boardPath, JSON.stringify(next, null, 2) + "\n");
  console.log(
    JSON.stringify(
      {
        ok: true,
        board: boardPath,
        observed: observed.length,
        added,
        updated,
        item_count: items.length,
      },
      null,
      2,
    ),
  );
}

if (import.meta.url === `file://${process.argv[1]}` || process.argv[1]?.endsWith("ingest-soft-reds.mjs")) {
  main();
}
