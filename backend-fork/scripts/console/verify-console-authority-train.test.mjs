import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { chmodSync, mkdtempSync, mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { verifyConsoleAuthorityTrain } from './verify-console-authority-train.mjs';
import { createConsoleCandidateSourceResolver } from './validate-console-truth-ledger.mjs';
import { rawDiff } from './verify-console-pr-authority-bootstrap.mjs';
import { installGitFixtureEnvironment } from '../lib/git-fixture-environment.mjs';

installGitFixtureEnvironment();

const AUTHORITY_FILES = ['console-capability-registry.json', 'console-jurisdiction-register.json', 'console-program-ledger.md'];

/** A repository whose HEAD is a signed candidate C carrying the policy C's signature is checked against. */
function candidateRepository() {
  const root = mkdtempSync(path.join(tmpdir(), 'console-train-'));
  const git = (args) => execFileSync('git', ['-C', root, ...args], { encoding: 'utf8' }).trim();
  git(['init']); git(['config', 'user.name', 'Jason Lee']); git(['config', 'user.email', 'jason19931225@gmail.com']);
  const key = path.join(root, 'key'); execFileSync('ssh-keygen', ['-q', '-t', 'ed25519', '-N', '', '-f', key]);
  git(['config', 'gpg.format', 'ssh']); git(['config', 'user.signingkey', key]);
  const publicKey = readFileSync(`${key}.pub`, 'utf8').trim().split(/\s+/).slice(0, 2).join(' ');
  const fingerprint = execFileSync('ssh-keygen', ['-lf', `${key}.pub`, '-E', 'sha256'], { encoding: 'utf8' }).trim().split(/\s+/)[1];
  mkdirSync(path.join(root, '.github/trust'), { recursive: true });
  mkdirSync(path.join(root, 'docs/program/ledger'), { recursive: true });
  writeFileSync(path.join(root, '.github/trust/console.allowed_signers'), `jason19931225@gmail.com ${publicKey}\n`);
  for (const file of AUTHORITY_FILES) writeFileSync(path.join(root, 'docs/program', file), 'C\n');
  writeFileSync(path.join(root, 'docs/program/ledger/0001-existing.md'), 'existing entry\n');
  git(['add', '--', '.github', 'docs']); git(['commit', '-S', '-m', 'C']);
  return { root, git, authority: { format: 'ssh', principal: 'jason19931225@gmail.com', fingerprint }, C: git(['rev-parse', 'HEAD']) };
}

test('authority train accepts only an exact structural synthetic merge M', () => {
  const { root, git, authority, C } = candidateRepository();
  try {
    for (const file of AUTHORITY_FILES) writeFileSync(path.join(root, 'docs/program', file), 'T\n');
    git(['add', '--', 'docs']); git(['commit', '-S', '-m', 'T']); const T = git(['rev-parse', 'HEAD']); const base = C;
    const M = git(['commit-tree', `${T}^{tree}`, '-p', base, '-p', T, '-m', 'M']);
    assert.doesNotThrow(() => verifyConsoleAuthorityTrain(root, C, T, M, authority));
    assert.throws(() => verifyConsoleAuthorityTrain(root, C, T, T, authority), /two parents/);
    const wrongParent = git(['commit-tree', `${T}^{tree}`, '-p', T, '-p', base, '-m', 'wrong parent']);
    assert.throws(() => verifyConsoleAuthorityTrain(root, C, T, wrongParent, authority), /parent 2/);
    writeFileSync(path.join(root, 'drift.txt'), 'drift\n'); git(['add', '--', 'drift.txt']); const driftTree = git(['write-tree']); const treeDrift = git(['commit-tree', driftTree, '-p', base, '-p', T, '-m', 'tree drift']);
    assert.throws(() => verifyConsoleAuthorityTrain(root, C, T, treeDrift, authority), /tree and content diff/);
  } finally { rmSync(root, { recursive: true, force: true }); }
});

test('C..T may touch one authority document, and may ADD files only under the ledger directory', () => {
  const { root, git, authority, C } = candidateRepository();
  // Builds a signed T on top of C that writes exactly one file (or none), plus its structural M.
  const train = (relativePath, body, mode) => {
    git(['checkout', '-q', '--detach', C]);
    if (relativePath) {
      const absolute = path.join(root, relativePath);
      mkdirSync(path.dirname(absolute), { recursive: true });
      writeFileSync(absolute, body);
      if (mode !== undefined) chmodSync(absolute, mode);
      git(['add', '--', relativePath]);
    }
    git(['commit', '-S', '--allow-empty', '-m', 'T']);
    const T = git(['rev-parse', 'HEAD']);
    return [T, git(['commit-tree', `${T}^{tree}`, '-p', C, '-p', T, '-m', 'M'])];
  };
  const accepts = (...args) => { const [T, M] = train(...args); assert.doesNotThrow(() => verifyConsoleAuthorityTrain(root, C, T, M, authority), `expected ${args[0]} to be accepted`); };
  const refuses = (pattern, ...args) => { const [T, M] = train(...args); assert.throws(() => verifyConsoleAuthorityTrain(root, C, T, M, authority), pattern, `expected ${args[0]} to be refused`); };
  try {
    // A single document is enough. "All three" was the shared-file mutex, not the allow-list:
    // it forced every lane to rewrite the same bytes in the same three files on every train.
    accepts('docs/program/console-program-ledger.md', 'T\n');
    accepts('docs/program/console-capability-registry.json', 'T\n');
    accepts('docs/program/console-jurisdiction-register.json', 'T\n');
    // A NEW file is status `A`, and `A` is accepted under the ledger directory prefix …
    accepts('docs/program/ledger/2026-08-01-pr-1.md', 'entry\n');
    accepts('docs/program/ledger/0001-existing.md', 'edited entry\n');
    // … for a FLAT directory of `.md` entries only. A subdirectory is refused because the
    // prefix check must be about a path SEGMENT, and `.mjs` because this prefix is the one
    // place the authority tip may add a file at all — a commit forbidden to touch product code
    // must not be able to add executable code under `docs/`.
    refuses(/docs\/program\/ledger\//, 'docs/program/ledger/nested/2026-08-01-pr-1.md', 'entry\n');
    refuses(/docs\/program\/ledger\//, 'docs/program/ledger/entry.mjs', 'entry\n');
    refuses(/docs\/program\/ledger\//, 'docs/program/ledger/entry.txt', 'entry\n');
    // … and nowhere else. A near-miss sibling of the prefix is not the prefix.
    refuses(/docs\/program\/ledger\//, 'docs/program/ledgerbook.md', 'new\n');
    refuses(/docs\/program\/ledger\//, 'docs/program/console-program-ledger-2.md', 'new\n');
    refuses(/docs\/program\/ledger\//, 'docs/program/console-capability-registry-2.json', 'new\n');
    refuses(/docs\/program\/ledger\//, 'README.md', 'new\n');
    refuses(/docs\/program\/ledger\//, 'scripts/console/attack.mjs', 'new\n');
    // The mode check survives the new status: an executable ledger entry is still refused.
    refuses(/mode-100644/, 'docs/program/ledger/2026-08-01-executable.md', 'entry\n', 0o755);
    // A non-authority MODIFICATION is unchanged — this is the property that keeps product code
    // out of the authority tip, and relaxing the count must not relax it.
    refuses(/mode-100644/, '.github/trust/console.allowed_signers', 'attacker@example.invalid ssh-ed25519 AAAA\n');
    // An empty C..T asserts nothing about authority at all.
    refuses(/at least one authority document/, null);
  } finally { rmSync(root, { recursive: true, force: true }); }
});

test('all three gates read the same C..T diff, including a ledger entry that is a near-copy', () => {
  // THREE scripts gate this one diff and they disagreed on the flags: the validator passed
  // `--find-renames --find-copies-harder`, the other two `--no-renames`. A new ledger entry
  // ≥50% similar to a file already in the tree is then status `C` with two paths to one reader
  // and `A` with one path to the others — the same commit refused by one gate and accepted by
  // the two that decide the merge. Identical bytes here only to pin the similarity score.
  const { root, git, authority, C } = candidateRepository();
  try {
    const existing = readFileSync(path.join(root, 'docs/program/ledger/0001-existing.md'), 'utf8');
    writeFileSync(path.join(root, 'docs/program/ledger/0002-near-copy.md'), existing);
    git(['add', '--', 'docs/program/ledger/0002-near-copy.md']); git(['commit', '-S', '-m', 'T']);
    const T = git(['rev-parse', 'HEAD']);
    const M = git(['commit-tree', `${T}^{tree}`, '-p', C, '-p', T, '-m', 'M']);
    assert.doesNotThrow(() => verifyConsoleAuthorityTrain(root, C, T, M, authority));
    assert.doesNotThrow(() => createConsoleCandidateSourceResolver(root, C, T, { candidateSigningAuthority: authority }));
    assert.deepEqual(rawDiff(root, C, T).map(({ path: file, status }) => [file, status]), [['docs/program/ledger/0002-near-copy.md', 'A']]);
  } finally { rmSync(root, { recursive: true, force: true }); }
});
