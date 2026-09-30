#!/usr/bin/env node
// Simulate the ordinary pull_request_target graph gate as main runs it.
//
// Live GitHub PR/ref readback is intentionally outside this local simulator.
// The structural decision -- exact protected base B, exact head H, and B as an
// ancestor of H -- uses the real protected verifier imported from the supplied
// main worktree. No candidate bytes are checked out or executed here.
//
// usage: node simulate-main-gate.mjs <main-worktree> <H-sha> [base-sha]
//
// `base-sha` defaults to origin/main.

import { execFileSync } from 'node:child_process';
import path from 'node:path';

const [mainWorktree, tipSha, baseArg] = process.argv.slice(2);
const gateUrl = new URL(`file://${path.join(mainWorktree, 'scripts/console/verify-console-pr-authority-bootstrap.mjs')}`);
const gate = await import(gateUrl.href);

const git = (repo, args) => execFileSync('git', ['-C', repo, '-c', 'core.hooksPath=/dev/null', ...args], { encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'] });

const H = git(mainWorktree, ['rev-parse', tipSha]).trim();
const base = git(mainWorktree, ['rev-parse', baseArg ?? 'origin/main']).trim();
console.log(`base(${baseArg ?? 'origin/main'}) = ${base}`);
console.log(`H                 = ${H}\n`);

let graph;
try {
  graph = gate.verifyBootstrapGraph(gate.createProtectedGitOps(mainWorktree), {
    baseSha: base,
    headSha: H,
    prNumber: 1,
    prAuthorId: 1,
    prAuthorLogin: 'local-simulator',
    prHeadRef: 'local-simulator',
    prHeadRepository: 'local/simulator',
    repository: gate.PINNED_RELEASE_REPOSITORY,
  });
  console.log('MAIN BOOTSTRAP GATE: PASS');
  console.log(JSON.stringify(graph, null, 2));
} catch (error) {
  console.log(`MAIN BOOTSTRAP GATE: REFUSED — ${error.message}`);
  process.exit(1);
}
