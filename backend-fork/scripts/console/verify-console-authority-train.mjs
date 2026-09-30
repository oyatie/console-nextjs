import { execFileSync } from 'node:child_process';
import { CONSOLE_CANDIDATE_SIGNING_AUTHORITY, verifyCommitWithCandidateSshPolicy } from './ssh-signature-policy.mjs';
import { AUTHORITY_DIFF_ARGS, LEDGER_DIRECTORY, isLedgerEntryPath } from './authority-ledger-path.mjs';
import {
  RELEASE_PLEASE_TRAIN_CLASS,
  classifyReleasePleaseBotTip,
  classifyReleasePleaseSquashBinding,
  gitOpsForReleasePlease,
  verifyReleasePleaseBotTrain,
} from './release-please-bot-candidate.mjs';

const SHA = /^[0-9a-f]{40}$/;
const AUTHORITY_PATHS = new Set([
  'docs/program/console-capability-registry.json',
  'docs/program/console-jurisdiction-register.json',
  'docs/program/console-program-ledger.md',
]);
// One file per new ledger entry, so two lanes never write the same bytes. Status `A` is
// accepted for this prefix and NOWHERE else: the two registers and the legacy ledger .md
// stay modify-only, and an added file anywhere outside this directory is still refused.
// The predicate and the diff flags are shared with the other two gates on purpose — see
// authority-ledger-path.mjs.
export { LEDGER_DIRECTORY };
const isAuthorityPath = (file) => AUTHORITY_PATHS.has(file) || isLedgerEntryPath(file);
const allowedModes = (status, oldMode, newMode) => newMode === '100644' && oldMode === (status === 'A' ? '000000' : '100644');
const allowedChange = (file, status, oldMode, newMode) => isAuthorityPath(file) && (status === 'M' || (status === 'A' && isLedgerEntryPath(file))) && allowedModes(status, oldMode, newMode);

function fail(message) { throw new Error(`console authority train: ${message}`); }
function sha(value, label) { if (!SHA.test(value ?? '')) fail(`${label} must be a full lowercase Git SHA`); return value; }
function git(repoRoot, args) { return execFileSync('git', ['-C', repoRoot, ...args], { encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'] }); }
function gitSucceeds(repoRoot, args) { try { git(repoRoot, args); return true; } catch { return false; } }
function verifySigned(repoRoot, candidateSha, value, label, authority) {
  if (!gitSucceeds(repoRoot, ['cat-file', '-e', `${value}^{commit}`])) fail(`${label} SHA is unresolvable`);
  try { verifyCommitWithCandidateSshPolicy(repoRoot, candidateSha, value, authority); }
  catch (error) { fail(`${label} signature is not valid: ${error instanceof Error ? error.message : String(error)}`); }
}
function assertAuthorityDiff(repoRoot, candidateSha, authorityTipSha) {
  const fields = git(repoRoot, [...AUTHORITY_DIFF_ARGS, candidateSha, authorityTipSha]).split('\0');
  const changed = new Set();
  for (let index = 0; index < fields.length - 1;) {
    const header = fields[index++]; if (!header) continue;
    const match = header.match(/^:(\d{6}) (\d{6}) [0-9a-f]{40} [0-9a-f]{40} ([A-Z])$/);
    if (!match) fail('C..T diff contains an unsupported entry');
    const [, oldMode, newMode, status] = match; const file = fields[index++];
    if (!allowedChange(file, status, oldMode, newMode) || changed.has(file)) fail('C..T may only modify the regular mode-100644 authority documents or add regular mode-100644 files under docs/program/ledger/');
    changed.add(file);
  }
  // ALLOW-LIST, not a checklist. Nothing outside the allow-list may change — that is the
  // property keeping product code out of the authority tip, and it is untouched. What is gone
  // is the demand that ALL THREE change, which was only ever satisfiable because the registers
  // carried a per-candidate SHA to rewrite; it forced every lane through the same shared bytes.
  if (changed.size === 0) fail('C..T must modify at least one authority document');
}

/** Validates signed candidate C, signed authority tip T, and structural-only GitHub merge M. */
export function verifyConsoleAuthorityTrain(repoRoot, candidateSha, authorityTipSha, syntheticMergeSha, authority = CONSOLE_CANDIDATE_SIGNING_AUTHORITY) {
  sha(candidateSha, 'candidate SHA'); sha(authorityTipSha, 'authority tip SHA'); sha(syntheticMergeSha, 'synthetic merge SHA');
  const ops = gitOpsForReleasePlease(repoRoot, git, gitSucceeds);
  const releaseTip = classifyReleasePleaseBotTip(ops, authorityTipSha);
  if (releaseTip) {
    if (releaseTip.candidateSha !== candidateSha) {
      fail('release-please bot tip parent must equal CONSOLE_CANDIDATE_SHA');
    }
    const admitted = verifyReleasePleaseBotTrain(ops, {
      headSha: authorityTipSha,
      mergeSha: syntheticMergeSha,
      requirePrMeta: false,
    });
    return Object.freeze({
      candidateSha: admitted.candidateSha,
      authorityTipSha: admitted.authorityTipSha,
      syntheticMergeSha: admitted.mergeSha,
      trainClass: RELEASE_PLEASE_TRAIN_CLASS,
    });
  }
  // C may be the unsigned main squash of a previously classifiable release-please tip.
  const squashC = classifyReleasePleaseSquashBinding(ops, candidateSha);
  if (!(squashC && squashC.admittedCandidateSha === candidateSha)) {
    verifySigned(repoRoot, candidateSha, candidateSha, 'candidate C', authority);
  }
  verifySigned(repoRoot, candidateSha, authorityTipSha, 'authority tip T', authority);
  const tipParents = git(repoRoot, ['show', '-s', '--format=%P', authorityTipSha]).trim().split(/\s+/).filter(Boolean);
  if (tipParents.length !== 1 || tipParents[0] !== candidateSha) fail('T must be the direct single-parent child of C');
  assertAuthorityDiff(repoRoot, candidateSha, authorityTipSha);
  if (!gitSucceeds(repoRoot, ['cat-file', '-e', `${syntheticMergeSha}^{commit}`])) fail('synthetic merge M is unresolvable');
  const mergeParents = git(repoRoot, ['show', '-s', '--format=%P', syntheticMergeSha]).trim().split(/\s+/).filter(Boolean);
  if (mergeParents.length !== 2 || mergeParents[1] !== authorityTipSha) fail('M must have exactly two parents with T as parent 2');
  if (git(repoRoot, ['show', '-s', '--format=%T', syntheticMergeSha]).trim() !== git(repoRoot, ['show', '-s', '--format=%T', authorityTipSha]).trim() || !gitSucceeds(repoRoot, ['diff', '--quiet', '--no-ext-diff', syntheticMergeSha, authorityTipSha])) fail('M tree and content diff must equal T exactly');
  return Object.freeze({ candidateSha, authorityTipSha, syntheticMergeSha, trainClass: 'ssh-authority' });
}
