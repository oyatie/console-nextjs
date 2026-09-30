#!/usr/bin/env node
// Verify code citations in a markdown doc.
//
// A line-number citation is unverifiable by construction: you can confirm the
// file has that many lines, you cannot confirm the line says what the citation
// claims. Off-by-one errors survive every review that does not open both files
// side by side. This script counts that population so a sweep can drive it to
// zero, and hard-fails on citations that are provably wrong.
//
// Usage:
//   node scripts/console/verify-doc-citations.mjs <doc.md> [--max-unverifiable=N]
//
// Exit 1 if any citation is BROKEN, or UNVERIFIABLE exceeds --max-unverifiable
// (default 0). Raise the threshold to ratchet a sweep down in waves.

import { execFileSync } from 'node:child_process';
import { existsSync, readFileSync } from 'node:fs';
import { beginGate, emitProvenanceIfRequested, noteAssertion, noteRead } from '../lib/gate-inputs.mjs';
import { basename, dirname, resolve } from 'node:path';
import process from 'node:process';

const MIGRATIONS = 'backend/crates/platform/db/migrations';
const DECISIONS = 'docs/decisions';
const CODE_EXT = /[^/]\.(rs|sql|md|mjs|js|ts|tsx|toml|yml|yaml|json|lock|sh|tsv|csv)$/;

// ---------------------------------------------------------------- repo index

function repoRoot(from) {
  for (const dir of [from, process.cwd()]) {
    try {
      return execFileSync('git', ['-C', dir, 'rev-parse', '--show-toplevel'], {
        encoding: 'utf8',
        stdio: ['ignore', 'pipe', 'ignore'],
      }).trim();
    } catch {
      /* doc may live outside the repo it cites; fall back to cwd */
    }
  }
  console.error('not inside a git repository, and the doc is not either');
  process.exit(2);
}

function trackedFiles(root) {
  return execFileSync('git', ['-C', root, 'ls-files'], {
    encoding: 'utf8',
    maxBuffer: 64 * 1024 * 1024,
  })
    .split('\n')
    .filter((p) => p && !p.startsWith('console-lanes/')); // lanes are off-limits
}

// ------------------------------------------------------------- file shapes

const isMigrationNumber = (t) => /^\d{4}$/.test(t);
const isMigrationFile = (t) => /^\d{4}_[\w-]+\.sql$/.test(t);
const isAdrName = (t) => /^(ADR|DN)-\d{4}$/.test(t) || t === 'README';

/**
 * Could this span name a file? Guards `Feature::ALL`, `app.current_org`,
 * `LISTEN/NOTIFY`, `prelude/`, `ontology/*`, `.sql` — none of which are
 * citations. A slash alone is not enough: extension-less paths count only when
 * they actually resolve to a tracked file (`tools/buck2` does, `platform/db`
 * is a directory).
 */
function isFileShaped(t, resolveTarget) {
  if (!t || /[\s*]/.test(t)) return false;
  if (t.startsWith('/') || t.endsWith('/')) return false;
  if (isMigrationNumber(t) || isMigrationFile(t) || isAdrName(t)) return true;
  if (CODE_EXT.test(t)) return true;
  return t.includes('/') && Boolean(resolveTarget(t).path);
}

// ------------------------------------------------------------- resolution

/** @returns {{path: string} | {error: string}} */
function makeResolver(root, index) {
  const tracked = new Set(index);
  const cache = new Map();

  const globOne = (dir, re) => {
    const hits = index.filter((p) => dirname(p) === dir && re.test(basename(p)));
    return hits;
  };

  function uncached(target) {
    if (isMigrationNumber(target)) {
      const hits = globOne(MIGRATIONS, new RegExp(`^${target}_`));
      if (hits.length === 1) return { path: hits[0] };
      return { error: hits.length ? 'ambiguous migration number' : 'no migration with that number' };
    }
    if (isMigrationFile(target)) {
      const p = `${MIGRATIONS}/${target}`;
      return tracked.has(p) ? { path: p } : { error: 'migration file not found' };
    }
    if (isAdrName(target)) {
      // Design notes live under docs/decisions/notes/, ADRs directly under it.
      const re = target === 'README' ? /^README\.md$/ : new RegExp(`^${target}[-.]`);
      const hits = index.filter(
        (p) => p.startsWith(`${DECISIONS}/`) && re.test(basename(p)),
      );
      if (hits.length === 1) return { path: hits[0] };
      return { error: hits.length ? 'ambiguous decision record' : 'no decision record with that id' };
    }
    if (tracked.has(target)) return { path: target };
    // Shorthand paths: `authz/src/lib.rs`, `instances.rs`. Unique suffix only.
    const suffix = `/${target}`;
    const hits = index.filter((p) => p.endsWith(suffix));
    if (hits.length === 1) return { path: hits[0] };
    if (hits.length > 1) return { error: `ambiguous path (${hits.length} matches)` };
    return { error: 'file not found' };
  }

  return (target) => {
    if (!cache.has(target)) cache.set(target, uncached(target));
    return cache.get(target);
  };
}

const lineCache = new Map();
function fileText(root, path) {
  if (!lineCache.has(path)) {
    const abs = resolve(root, path);
    const text = existsSync(abs) ? readFileSync(abs, 'utf8') : null;
    lineCache.set(path, {
      text,
      lines: text === null ? 0 : text.split('\n').length,
      flat: text === null ? null : text.replace(/\s+/g, ' '),
    });
  }
  return lineCache.get(path);
}

// ---------------------------------------------------------------- parsing

/** Code spans in document order, skipping fenced blocks. */
function codeSpans(md) {
  const spans = [];
  const lines = md.split('\n');
  let offset = 0;
  let fenced = false;
  lines.forEach((line, i) => {
    if (/^\s*```/.test(line)) fenced = !fenced;
    else if (!fenced) {
      const re = /`([^`\n]+)`/g;
      let m;
      while ((m = re.exec(line))) {
        spans.push({
          text: m[1],
          mdLine: i + 1,
          start: offset + m.index,
          end: offset + m.index + m[0].length,
        });
      }
    }
    offset += line.length + 1;
  });
  return spans;
}

const LINE_CITE = /^(.*?):(\d+)(?:-(\d+))?$/;
const SYMBOL_CITE = /^(.*?):([A-Za-z_][\w:.]*)$/;

/**
 * Classify spans into citations. `carry` is the last resolved file, so a bare
 * `:44` continuation inherits the file its neighbours established.
 */
function extractCitations(md, resolveTarget) {
  const spans = codeSpans(md);
  const cites = [];
  const fileish = (t) => isFileShaped(t, resolveTarget);
  // The antecedent for a bare `:44`. An anchor that failed to resolve clears it:
  // inheriting an unproven file is exactly how a false fact gets propagated.
  let carry = null;

  const push = (c) => {
    cites.push(c);
    if (c.kind !== 'line' || !c.inherited) carry = c.file ? { target: c.target, file: c.file } : null;
  };

  for (let i = 0; i < spans.length; i++) {
    const s = spans[i];
    const t = s.text.trim();

    const lineM = LINE_CITE.exec(t);
    if (lineM) {
      const [, prefix, a, b] = lineM;
      const at = { from: Number(a), to: Number(b ?? a) };
      if (prefix === '') {
        push({ ...s, kind: 'line', ...at, inherited: true,
               target: carry?.target ?? null, file: carry?.file ?? null,
               unbound: !carry,
               detail: carry ? undefined : 'no resolvable file citation precedes this bare :line' });
        continue;
      }
      if (fileish(prefix)) {
        const r = resolveTarget(prefix);
        push({ ...s, kind: 'line', ...at, target: prefix, file: r.path ?? null,
               detail: r.error, unbound: !r.path && r.error.startsWith('ambiguous') });
        continue;
      }
      // `preflight:75`, `realtime:40` — a name only a human can bind to a file.
      push({ ...s, kind: 'line', ...at, target: prefix, file: null, unbound: true,
             detail: 'target is not a path; only a human can bind it to a file' });
      continue;
    }

    const symM = SYMBOL_CITE.exec(t);
    if (symM && fileish(symM[1])) {
      const r = resolveTarget(symM[1]);
      push({ ...s, kind: 'symbol', target: symM[1], file: r.path ?? null,
             detail: r.error, symbol: symM[2] });
      continue;
    }

    if (fileish(t)) {
      const r = resolveTarget(t);
      // `file.rs` `symbol` — adjacent spans separated by whitespace only.
      const next = spans[i + 1];
      const gap = next ? md.slice(s.end, next.start) : null;
      if (next && /^[ \t]*$/.test(gap) && !LINE_CITE.test(next.text) && !fileish(next.text)) {
        push({ ...s, kind: 'symbol', target: t, file: r.path ?? null,
               detail: r.error, symbol: next.text.trim(), end: next.end });
        i++;
        continue;
      }
      push({ ...s, kind: 'file', target: t, file: r.path ?? null, detail: r.error });
    }
  }
  return cites;
}

// ------------------------------------------------------------- verification

function verify(root, cite) {
  // Cannot be bound to a file at all: ambiguous shorthand, opaque job name, or
  // an orphan `:44`. Not provably wrong — provably uncheckable.
  if (cite.unbound) return { verdict: 'UNVERIFIABLE', why: cite.detail };
  if (!cite.file) {
    // A bare file mention may be a planned deliverable, not a typo; the tool
    // cannot tell, so it reports without failing. An anchored citation to a
    // file that does not exist is wrong either way.
    const verdict = cite.kind === 'file' ? 'MISSING' : 'BROKEN';
    return { verdict, why: cite.detail ?? 'unresolved target' };
  }
  const f = fileText(root, cite.file);
  if (f.text === null) {
    return { verdict: 'BROKEN', why: 'tracked but not present on disk' };
  }

  if (cite.kind === 'line') {
    if (cite.from < 1 || cite.to < cite.from) {
      return { verdict: 'BROKEN', why: `impossible range ${cite.from}-${cite.to}` };
    }
    if (cite.to > f.lines) {
      // A bare `:174` inherits its file from the nearest preceding citation.
      // That inheritance is this tool's guess — the doc may carry the real
      // antecedent in prose ("ledger `:174`"). Never call a guess wrong.
      if (cite.inherited) {
        return {
          verdict: 'UNVERIFIABLE',
          suspect: true,
          why: `line ${cite.to} is past the end of the inferred antecedent ${cite.file} (${f.lines} lines) — either the citation or the inference is wrong, and neither can be settled from the citation alone`,
        };
      }
      return { verdict: 'BROKEN', why: `line ${cite.to} past end of file (${f.lines} lines)` };
    }
    return {
      verdict: 'UNVERIFIABLE',
      why: `file has ${f.lines} lines; nothing can confirm what line ${cite.from} says`,
    };
  }

  if (cite.kind === 'symbol') {
    const needle = cite.symbol;
    const hit =
      f.text.includes(needle) || f.flat.includes(needle.replace(/\s+/g, ' '));
    return hit
      ? { verdict: 'RESOLVES', why: `found "${needle}"` }
      : { verdict: 'BROKEN', why: `"${needle}" not found in file` };
  }

  return { verdict: 'FILE-ONLY', why: 'file exists; citation claims nothing checkable' };
}

// ---------------------------------------------------------------------- main

const args = process.argv.slice(2);
const docArg = args.find((a) => !a.startsWith('--'));
const maxFlag = args.find((a) => a.startsWith('--max-unverifiable='));
const maxUnverifiable = maxFlag ? Number(maxFlag.split('=')[1]) : 0;
const quiet = args.includes('--quiet');

if (!docArg) {
  console.error('usage: verify-doc-citations.mjs <doc.md> [--max-unverifiable=N] [--quiet]');
  process.exit(2);
}
const docPath = resolve(process.cwd(), docArg);
if (!existsSync(docPath)) {
  console.error(`no such file: ${docArg}`);
  process.exit(2);
}

const root = repoRoot(dirname(docPath));
// Declare only the document under verification. Cited targets are resolution
// inputs, not gate prose-dependencies in the K-1 sense.
const docRel = docPath.startsWith(root + '/')
  ? docPath.slice(root.length + 1)
  : docArg.replace(/^\.\//, '');
beginGate({
  gate: 'check:doc-citations',
  script: 'scripts/console/verify-doc-citations.mjs',
  documentInputs: [docRel],
});
const index = trackedFiles(root);
const resolveTarget = makeResolver(root, index);
const md = readFileSync(docPath, 'utf8');
noteRead(docRel);
const cites = extractCitations(md, resolveTarget);
for (let i = 0; i < Math.max(cites.length, 1); i += 1) noteAssertion(docRel);
const results = cites.map((c) => ({ cite: c, ...verify(root, c) }));

const by = (v) => results.filter((r) => r.verdict === v);
const broken = by('BROKEN');
const unverifiable = by('UNVERIFIABLE');
const resolves = by('RESOLVES');
const fileOnly = by('FILE-ONLY');
const missing = by('MISSING');

const label = (c) =>
  c.kind === 'symbol'
    ? `\`${c.target}\` \`${c.symbol}\``
    : `\`${c.inherited ? '' : c.target ?? ''}${c.kind === 'line' ? `:${c.from}${c.to !== c.from ? `-${c.to}` : ''}` : ''}\``;

if (broken.length) {
  console.log('BROKEN — provably wrong, fix these first');
  for (const r of broken) {
    console.log(`  ${basename(docArg)}:${r.cite.mdLine}  ${label(r.cite)}  → ${r.why}`);
  }
  console.log('');
}

const suspect = unverifiable.filter((r) => r.suspect);
if (suspect.length) {
  console.log('SUSPECT — unverifiable, and the numbers do not line up. Fix these first');
  for (const r of suspect) {
    console.log(`  ${basename(docArg)}:${r.cite.mdLine}  ${label(r.cite)}  → ${r.why}`);
  }
  console.log('');
}

if (unverifiable.length && !quiet) {
  console.log('UNVERIFIABLE — line-number citations, the population to eliminate');
  for (const r of unverifiable) {
    const tgt = r.cite.file ?? r.cite.target ?? '?';
    console.log(
      `  ${basename(docArg)}:${r.cite.mdLine}  ${tgt}:${r.cite.from}${r.cite.to !== r.cite.from ? `-${r.cite.to}` : ''}${r.cite.inherited ? '  (inherited file)' : ''}${r.cite.opaque ? '  (opaque target)' : ''}`,
    );
  }
  console.log('');
}

if (resolves.length && !quiet) {
  console.log('RESOLVES — verifiable by grep');
  for (const r of resolves) {
    console.log(`  ${basename(docArg)}:${r.cite.mdLine}  ${r.cite.file}  ${r.why}`);
  }
  console.log('');
}

if (missing.length && !quiet) {
  console.log('MISSING — file mentioned but not in the repo (typo, or a planned artifact)');
  for (const r of missing) {
    console.log(`  ${basename(docArg)}:${r.cite.mdLine}  ${r.cite.target}  → ${r.why}`);
  }
  console.log('');
}

const total = results.length;
console.log(`${docArg}`);
console.log(`  total citations : ${total}`);
console.log(`  RESOLVES        : ${resolves.length}   (symbol or fragment found in file)`);
console.log(`  UNVERIFIABLE    : ${unverifiable.length}   (line numbers — max allowed ${maxUnverifiable}, of which ${suspect.length} SUSPECT)`);
console.log(`  BROKEN          : ${broken.length}`);
console.log(`  FILE-ONLY       : ${fileOnly.length}   (file exists, no checkable claim)`);
console.log(`  MISSING         : ${missing.length}   (file mention, absent from repo — not fatal)`);

emitProvenanceIfRequested();
let exit = 0;
if (broken.length) {
  console.log(`\nFAIL: ${broken.length} broken citation(s).`);
  exit = 1;
}
if (unverifiable.length > maxUnverifiable) {
  console.log(
    `\nFAIL: ${unverifiable.length} unverifiable citation(s) exceeds --max-unverifiable=${maxUnverifiable}.`,
  );
  exit = 1;
}
process.exit(exit);
