#!/usr/bin/env node
// Which .test.mjs suites actually execute in CI?
//
// A plain grep for the filename in ci.yml is wrong: workflows invoke `npm run
// test:adrs`, and package.json expands that to the file. So resolve the npm
// indirection to a fixed point, then ask which suites are reachable from a
// workflow `run:` line. Reports the reasoning so the answer can be checked
// rather than believed.

import { execFileSync } from 'node:child_process';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

export function repositoryRoot(moduleUrl = import.meta.url) {
  return path.resolve(path.dirname(fileURLToPath(moduleUrl)), '../..');
}

export function reportReachability(repo = repositoryRoot()) {
  const git = (...args) => execFileSync('git', ['-C', repo, ...args], { encoding: 'utf8', maxBuffer: 64 * 1024 * 1024 });
  const files = git('ls-tree', '-r', '--name-only', 'origin/main').split('\n').filter(Boolean);
  const suites = files.filter((file) => file.endsWith('.test.mjs'));
  const workflows = files.filter((file) => file.startsWith('.github/workflows/'));
  const pkg = JSON.parse(git('show', 'origin/main:package.json'));
  const scripts = pkg.scripts ?? {};

  // Every shell line a workflow actually runs.
  let runText = '';
  for (const workflow of workflows) runText += git('show', `origin/main:${workflow}`);

  // Expand `npm run X` / `npm X` to X's body, repeatedly, until nothing new appears.
  const invoked = new Set();
  for (const name of Object.keys(scripts)) {
    const re = new RegExp(`npm\\s+(?:run\\s+)?${name.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')}(?![\\w:-])`);
    if (re.test(runText)) invoked.add(name);
  }
  for (let changed = true; changed;) {
    changed = false;
    for (const name of [...invoked]) {
      for (const other of Object.keys(scripts)) {
        if (invoked.has(other)) continue;
        const re = new RegExp(`npm\\s+(?:run\\s+)?${other.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')}(?![\\w:-])`);
        if (re.test(scripts[name])) { invoked.add(other); changed = true; }
      }
    }
  }

  const expanded = runText + [...invoked].map((name) => scripts[name]).join('\n');

  const wired = [], nowhere = [];
  for (const suite of suites) {
    const direct = expanded.includes(suite);
    const byBase = expanded.includes(suite.split('/').pop());
    (direct || byBase ? wired : nowhere).push({ s: suite, how: direct ? 'path' : byBase ? 'basename' : '' });
  }

  const lines = [
    `.test.mjs suites on origin/main: ${suites.length}`,
    `npm scripts reachable from a workflow: ${invoked.size}\n`,
    `EXECUTES IN CI (${wired.length}):`,
    ...wired.sort((left, right) => left.s.localeCompare(right.s)).map(({ s, how }) => `  ${s}  [${how}]`),
    `\nEXECUTES NOWHERE (${nowhere.length}):`,
  ];
  for (const { s } of nowhere.sort((left, right) => left.s.localeCompare(right.s))) {
    // Is it at least runnable by hand via some npm script?
    const owner = Object.entries(scripts).find(([, value]) => value.includes(s) || value.includes(s.split('/').pop()));
    lines.push(`  ${s}${owner ? `   (npm run ${owner[0]} — defined but never invoked by CI)` : '   (no npm script at all)'}`);
  }
  return `${lines.join('\n')}\n`;
}

if (process.argv[1] === fileURLToPath(import.meta.url)) process.stdout.write(reportReachability());
