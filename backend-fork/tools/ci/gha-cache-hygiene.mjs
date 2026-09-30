#!/usr/bin/env node
/**
 * GHA cache hygiene for console — automated cleanup protocol.
 *
 * Policy (order matters):
 *  1. FORCE-DELETE every cache whose key starts with DELETE_PREFIXES
 *     (default: buildkit-, index- — Docker BuildKit type=gha leftovers).
 *  2. AGE-DELETE non-KEEP caches whose last_accessed_at is older than
 *     MAX_AGE_DAYS (default: 14). Branch / PR noise does not pile forever.
 *  3. BUDGET-DELETE oldest non-KEEP caches until total projected size
 *     is ≤ MAX_TOTAL_BYTES (default: 8 GiB under the shared 10 GB hard cap).
 *  4. FAIL the job if usage still exceeds MAX_TOTAL_BYTES after deletes
 *     (problem stays visible; never silently over budget).
 *
 * KEEP_PREFIXES (default: v0-rust-, node-cache-) are never age- or
 * budget-deleted. Force-delete prefixes still win if they overlap.
 *
 * Env:
 *   GITHUB_TOKEN, GITHUB_REPOSITORY (owner/repo)
 *   DRY_RUN=true|false
 *   MAX_TOTAL_BYTES (default 8GiB)
 *   MAX_AGE_DAYS (default 14; 0 disables age eviction)
 *   DELETE_PREFIXES (comma-separated)
 *   KEEP_PREFIXES (comma-separated)
 *
 * Pure policy helpers are exported for unit tests (see
 * gha-cache-hygiene.test.mjs).
 */
import { writeFileSync } from "node:fs";

const DEFAULT_DELETE_PREFIXES = ["buildkit-", "index-"];
const DEFAULT_KEEP_PREFIXES = ["v0-rust-", "node-cache-"];
const DEFAULT_MAX_TOTAL = 8 * 1024 ** 3;
const DEFAULT_MAX_AGE_DAYS = 14;

/** @param {string} key @param {string[]} prefixes */
export const matchesPrefix = (key, prefixes) =>
  prefixes.some((p) => (key || "").startsWith(p));

/** @param {number} n */
export const fmtBytes = (n) => `${(n / 1024 ** 3).toFixed(2)} GiB`;

/**
 * Select victims under the hygiene protocol.
 *
 * @param {Array<{id:number|string,key:string,size_in_bytes?:number,last_accessed_at?:string,created_at?:string}>} caches
 * @param {{ beforeBytes: number, maxTotal: number, maxAgeDays: number, deletePrefixes: string[], keepPrefixes: string[], nowMs?: number }} opts
 * @returns {{ victims: Array<object>, projected: number }}
 */
export const selectVictims = (caches, opts) => {
  const {
    beforeBytes,
    maxTotal,
    maxAgeDays,
    deletePrefixes,
    keepPrefixes,
    nowMs = Date.now(),
  } = opts;
  const maxAgeMs = maxAgeDays > 0 ? maxAgeDays * 24 * 60 * 60 * 1000 : 0;

  const victims = [];
  const force = caches.filter((c) => matchesPrefix(c.key, deletePrefixes));
  for (const c of force) victims.push({ ...c, reason: "docker-gha-prefix" });

  const forceIds = new Set(force.map((c) => c.id));
  let projected =
    beforeBytes - force.reduce((s, c) => s + (c.size_in_bytes || 0), 0);

  if (maxAgeMs > 0) {
    for (const c of caches) {
      if (forceIds.has(c.id)) continue;
      if (matchesPrefix(c.key, keepPrefixes)) continue;
      const accessed = Date.parse(c.last_accessed_at || c.created_at || 0);
      if (!Number.isFinite(accessed)) continue;
      if (nowMs - accessed < maxAgeMs) continue;
      victims.push({ ...c, reason: "age-stale" });
      forceIds.add(c.id);
      projected -= c.size_in_bytes || 0;
    }
  }

  if (projected > maxTotal) {
    const rest = caches
      .filter((c) => !forceIds.has(c.id))
      .filter((c) => !matchesPrefix(c.key, keepPrefixes))
      .sort((a, b) => {
        const ta = Date.parse(a.last_accessed_at || a.created_at || 0);
        const tb = Date.parse(b.last_accessed_at || b.created_at || 0);
        return ta - tb; // oldest first
      });
    for (const c of rest) {
      if (projected <= maxTotal) break;
      victims.push({ ...c, reason: "budget-lru" });
      forceIds.add(c.id);
      projected -= c.size_in_bytes || 0;
    }
  }

  const byId = new Map();
  for (const v of victims) byId.set(v.id, v);
  return { victims: [...byId.values()], projected };
};

const isMain = (() => {
  try {
    const entry = process.argv[1] || "";
    return entry.endsWith("gha-cache-hygiene.mjs");
  } catch {
    return true;
  }
})();

const api = async (token, path, init = {}) => {
  const res = await fetch(`https://api.github.com${path}`, {
    ...init,
    headers: {
      Accept: "application/vnd.github+json",
      Authorization: `Bearer ${token}`,
      "X-GitHub-Api-Version": "2022-11-28",
      ...(init.headers || {}),
    },
  });
  if (!res.ok) {
    const body = await res.text();
    throw new Error(
      `${init.method || "GET"} ${path} -> ${res.status}: ${body.slice(0, 400)}`,
    );
  }
  if (res.status === 204) return null;
  return res.json();
};

const listAllCaches = async (token, repo) => {
  const out = [];
  let page = 1;
  for (;;) {
    const batch = await api(
      token,
      `/repos/${repo}/actions/caches?per_page=100&page=${page}`,
    );
    const actions = batch.actions_caches || [];
    out.push(...actions);
    if (actions.length < 100) break;
    page += 1;
    if (page > 50) break;
  }
  return out;
};

const main = async () => {
  const token = process.env.GITHUB_TOKEN;
  const repo = process.env.GITHUB_REPOSITORY;
  if (!token || !repo) {
    console.error("GITHUB_TOKEN and GITHUB_REPOSITORY are required");
    process.exit(2);
  }
  const dryRun = String(process.env.DRY_RUN || "false").toLowerCase() === "true";
  const maxTotal = Number(
    process.env.MAX_TOTAL_BYTES || String(DEFAULT_MAX_TOTAL),
  );
  const maxAgeDays = Number(
    process.env.MAX_AGE_DAYS || String(DEFAULT_MAX_AGE_DAYS),
  );
  const deletePrefixes = (
    process.env.DELETE_PREFIXES || DEFAULT_DELETE_PREFIXES.join(",")
  )
    .split(",")
    .map((s) => s.trim())
    .filter(Boolean);
  const keepPrefixes = (
    process.env.KEEP_PREFIXES || DEFAULT_KEEP_PREFIXES.join(",")
  )
    .split(",")
    .map((s) => s.trim())
    .filter(Boolean);

  const usage = await api(token, `/repos/${repo}/actions/cache/usage`);
  const caches = await listAllCaches(token, repo);
  const beforeBytes = usage.active_caches_size_in_bytes || 0;
  console.log(
    `usage before: ${fmtBytes(beforeBytes)} across ${usage.active_caches_count} (list ${caches.length})`,
  );
  console.log(
    `dry_run=${dryRun} max_total=${fmtBytes(maxTotal)} max_age_days=${maxAgeDays}`,
  );
  console.log(`delete_prefixes=${deletePrefixes.join(",")}`);
  console.log(`keep_prefixes=${keepPrefixes.join(",")}`);

  const { victims: unique, projected } = selectVictims(caches, {
    beforeBytes,
    maxTotal,
    maxAgeDays,
    deletePrefixes,
    keepPrefixes,
  });

  console.log(`victims: ${unique.length} (projected=${fmtBytes(Math.max(0, projected))})`);
  const byReason = {};
  for (const v of unique) {
    byReason[v.reason] = (byReason[v.reason] || 0) + 1;
  }
  console.log(`by_reason: ${JSON.stringify(byReason)}`);
  for (const v of unique.slice(0, 40)) {
    console.log(
      `  - ${v.reason} id=${v.id} ${((v.size_in_bytes || 0) / 1e6).toFixed(1)}MB ${String(v.key).slice(0, 70)}`,
    );
  }
  if (unique.length > 40) console.log(`  ... +${unique.length - 40} more`);

  let deleted = 0;
  let failed = 0;
  if (!dryRun) {
    for (const v of unique) {
      try {
        await api(token, `/repos/${repo}/actions/caches/${v.id}`, {
          method: "DELETE",
        });
        deleted += 1;
      } catch (e) {
        failed += 1;
        console.error(`delete failed id=${v.id}: ${e.message}`);
      }
    }
  }

  const usageAfter = await api(token, `/repos/${repo}/actions/cache/usage`);
  const afterBytes = usageAfter.active_caches_size_in_bytes || 0;
  console.log(
    `usage after: ${fmtBytes(afterBytes)} across ${usageAfter.active_caches_count} (deleted=${deleted} failed=${failed} dry_run=${dryRun})`,
  );

  if (typeof process.env.GITHUB_STEP_SUMMARY === "string") {
    writeFileSync(
      process.env.GITHUB_STEP_SUMMARY,
      [
        `## GHA cache hygiene`,
        ``,
        `| | |`,
        `|---|---|`,
        `| before | ${fmtBytes(beforeBytes)} / ${usage.active_caches_count} keys |`,
        `| after | ${fmtBytes(afterBytes)} / ${usageAfter.active_caches_count} keys |`,
        `| victims | ${unique.length} (deleted=${deleted}, failed=${failed}) |`,
        `| by_reason | ${JSON.stringify(byReason)} |`,
        `| dry_run | ${dryRun} |`,
        `| max_total | ${fmtBytes(maxTotal)} |`,
        `| max_age_days | ${maxAgeDays} |`,
        ``,
        `### Protocol`,
        `1. Force-delete \`DELETE_PREFIXES\` (Docker type=gha leftovers)`,
        `2. Age-delete non-KEEP older than \`MAX_AGE_DAYS\``,
        `3. Budget-LRU non-KEEP until ≤ \`MAX_TOTAL_BYTES\``,
        `4. Fail if still over budget (visibility, not silent drift)`,
        ``,
      ].join("\n"),
      { flag: "a" },
    );
  }

  if (afterBytes > maxTotal && !dryRun) {
    console.error(
      `cache budget still over max_total (${fmtBytes(afterBytes)} > ${fmtBytes(maxTotal)})`,
    );
    process.exit(1);
  }
};

if (isMain) {
  main().catch((e) => {
    console.error(e);
    process.exit(1);
  });
}
