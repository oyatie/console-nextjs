#!/usr/bin/env node
/**
 * Merge-queue half of `authenticate-console-authority`.
 *
 * The PR-time job authenticates a pull request's exact coordinates from
 * protected code. A merge group is not a pull request -- it is the queue's
 * candidate merge commit -- so there are no PR coordinates to authenticate and
 * re-running that check against it would assert nothing.
 *
 * What the queue actually depends on is narrower and checkable: every pull
 * request in the group must ALREADY have passed authentication, because the
 * check is required and a PR cannot be queued otherwise. This verifies exactly
 * that, and fails closed if it cannot.
 *
 * Security posture matches the PR-time job: protected code only, read-only
 * metadata, no PR bytes executed, no write or OIDC scope.
 *
 * BATCH SIZE 1 IS ASSUMED, and asserted rather than hoped for. GitHub names a
 * merge-group branch `gh-readonly-queue/<base>/pr-<number>-<sha>`, which
 * identifies one pull request. With batching enabled the group can contain
 * several PRs whose numbers are not all recoverable from the ref, so this
 * refuses to guess: raise `max_entries_to_build` only together with a change
 * here that enumerates the group.
 */
import { spawnSync } from "node:child_process";

const fail = (message) => {
  console.error(`console merge-group authority: ${message}`);
  process.exitCode = 1;
  return false;
};

/** @returns {number|null} the single PR number a merge-group ref identifies. */
export function pullRequestNumberFromHeadRef(headRef) {
  if (typeof headRef !== "string" || !headRef) return null;
  // refs/heads/gh-readonly-queue/main/pr-123-<40-hex>
  const match = /(?:^|\/)gh-readonly-queue\/[^/]+\/pr-(\d+)-[0-9a-f]{40}$/.exec(headRef.trim());
  if (!match) return null;
  const number = Number(match[1]);
  return Number.isSafeInteger(number) && number > 0 ? number : null;
}

/** @returns {{ok: boolean, reason: string}} */
/**
 * Parse the NDJSON `gh api --paginate --jq` emits: one object per line, across
 * every page.
 *
 * @param {string} stdout
 * @returns {Array<{name:string, conclusion:string}>|null} null if any line is unparseable
 */
export function parseCheckRunLines(stdout) {
  const lines = String(stdout ?? "").split("\n").filter((line) => line.trim() !== "");
  const out = [];
  for (const line of lines) {
    try {
      out.push(JSON.parse(line));
    } catch {
      return null;
    }
  }
  return out;
}

/**
 * @param {Array<object>|null} runs
 * @param {string} checkName
 * @param {number|null} expectedTotal the API's own `total_count`, when known
 */
export function evaluateCheckRuns(runs, checkName, expectedTotal = null) {
  if (!Array.isArray(runs)) return { ok: false, reason: "unreadable check-run list" };
  // A short list and an absent check are the same observation to a filter, and
  // treating them alike is what made this gate eject a PR that had passed. If
  // the API says there are more check runs than were read, say THAT -- never
  // "no such run", which reads as a missing gate rather than a missing page.
  if (
    typeof expectedTotal === "number"
    && Number.isFinite(expectedTotal)
    && runs.length < expectedTotal
  ) {
    return {
      ok: false,
      reason:
        `read only ${runs.length} of ${expectedTotal} check runs on the queued head; `
        + "the list is incomplete, so absence of a run cannot be concluded",
    };
  }
  const named = runs.filter((run) => run && run.name === checkName);
  if (named.length === 0) return { ok: false, reason: `no ${checkName} run on the queued head` };
  const conclusions = new Set(named.map((run) => run.conclusion));
  if (conclusions.size !== 1 || !conclusions.has("success")) {
    return {
      ok: false,
      reason: `${checkName} did not conclude success on the queued head (${[...conclusions].join(", ")})`,
    };
  }
  return { ok: true, reason: "ok" };
}

function parseArguments(argv) {
  const out = {};
  for (let i = 0; i < argv.length; i += 2) {
    const key = argv[i];
    const value = argv[i + 1];
    if (!key?.startsWith("--") || value === undefined) return null;
    out[key.slice(2)] = value;
  }
  return out;
}

const isMain = process.argv[1]?.endsWith("verify-console-merge-group-authority.mjs");
if (isMain) {
  const args = parseArguments(process.argv.slice(2));
  const headRef = args?.["head-ref"];
  const repository = args?.repository;
  if (!args || !headRef || !repository) {
    fail("usage: --head-ref <ref> --repository <owner/repo>");
  } else {
    const number = pullRequestNumberFromHeadRef(headRef);
    if (number === null) {
      // Includes the batched case: a ref this does not recognise is refused
      // rather than waved through.
      fail(`cannot identify a single queued pull request from head ref ${JSON.stringify(headRef)}`);
    } else {
      const pr = spawnSync("gh", [
        "api", `repos/${repository}/pulls/${number}`, "--jq", ".head.sha",
      ], { encoding: "utf8" });
      const headSha = pr.status === 0 ? pr.stdout.trim() : "";
      if (!/^[0-9a-f]{40}$/.test(headSha)) {
        fail(`cannot read the head SHA of queued pull request #${number}`);
      } else {
        // `--paginate` with an explicit per_page: the API defaults to 30 per
        // page, and this repository's pull requests already carry more than
        // that (32 when #820 was ejected). Without it the authority check falls
        // off page one whenever it sorts late, and the gate reports it missing
        // on a pull request that passed it -- a non-deterministic ejection that
        // grows more likely with every job added to CI.
        const checks = spawnSync("gh", [
          "api", "--paginate",
          `repos/${repository}/commits/${headSha}/check-runs?per_page=100`,
          "--jq", ".check_runs[] | {name, conclusion}",
        ], { encoding: "utf8" });
        const runs = checks.status === 0 ? parseCheckRunLines(checks.stdout) : null;

        // The API's own count, used to prove the read was complete rather than
        // assumed complete.
        const counted = spawnSync("gh", [
          "api", `repos/${repository}/commits/${headSha}/check-runs?per_page=1`,
          "--jq", ".total_count",
        ], { encoding: "utf8" });
        const parsedTotal = counted.status === 0 ? Number(counted.stdout.trim()) : NaN;
        const expectedTotal = Number.isFinite(parsedTotal) ? parsedTotal : null;

        const verdict = evaluateCheckRuns(runs, "authenticate-console-authority", expectedTotal);
        if (!verdict.ok) {
          fail(`pull request #${number} at ${headSha}: ${verdict.reason}`);
        } else {
          console.log(
            `console merge-group authority OK (pull request #${number} at ${headSha} `
            + "passed authenticate-console-authority before entering the queue)",
          );
        }
      }
    }
  }
}
