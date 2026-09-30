import { execFileSync, spawnSync } from 'node:child_process';
import { chmodSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';

export const CONSOLE_SSH_ALLOWED_SIGNERS_PATH = '.github/trust/console.allowed_signers';
export const CONSOLE_CANDIDATE_SIGNING_AUTHORITY = Object.freeze({ format: 'ssh', principal: 'jason19931225@gmail.com', fingerprint: 'SHA256:5grGNUtX9Zgmy1SWne6wF9DR8W1ElUQaF/Z8SYRz8E8' });
const FINGERPRINT = /^SHA256:[A-Za-z0-9+/]+={0,2}$/;
const POLICY_LINE = /^([^\s]+) (ssh-(?:ed25519|rsa|ecdsa-[A-Za-z0-9@._+-]+)) ([A-Za-z0-9+/]+={0,2})$/;
function fail(message) { throw new Error(message); }
function validAuthority(authority) { return authority?.format === 'ssh' && typeof authority.principal === 'string' && authority.principal !== '' && authority.principal.trim() === authority.principal && FINGERPRINT.test(authority.fingerprint ?? ''); }
function policyFromCandidate(repoRoot, candidateSha) {
  const entry = execFileSync('git', ['-C', repoRoot, 'ls-tree', candidateSha, '--', CONSOLE_SSH_ALLOWED_SIGNERS_PATH], { encoding: 'utf8' }).trim();
  if (!/^100644 blob [0-9a-f]{40}\t/.test(entry)) fail('candidate SSH allowed-signers policy is missing or not a regular Git blob');
  return execFileSync('git', ['-C', repoRoot, 'show', `${candidateSha}:${CONSOLE_SSH_ALLOWED_SIGNERS_PATH}`], { encoding: 'utf8' });
}
function assertPolicy(policy, authority) {
  if (!validAuthority(authority)) fail('candidate SSH signing authority is invalid');
  const lines = policy.split(/\r?\n/).filter(Boolean);
  const match = lines.length === 1 && lines[0].match(POLICY_LINE);
  if (!match || match[1] !== authority.principal) fail('candidate SSH allowed-signers policy principal is not trusted');
  const fingerprint = spawnSync('ssh-keygen', ['-lf', '-', '-E', 'sha256'], { input: `${match[2]} ${match[3]}\n`, encoding: 'utf8' });
  if (fingerprint.status !== 0 || fingerprint.stdout.trim().split(/\s+/)[1] !== authority.fingerprint) fail('candidate SSH allowed-signers policy fingerprint is not trusted');
}
export function sshSignatureMatchesAuthority(rawStatus, authority) {
  if (!validAuthority(authority) || typeof rawStatus !== 'string') return false;
  const lines = rawStatus.split(/\r?\n/).filter((line) => line.startsWith('Good "git" signature'));
  const match = lines.length === 1 && lines[0].match(/^Good "git" signature for (.+) with [A-Za-z0-9-]+ key (SHA256:[A-Za-z0-9+/]+={0,2})$/);
  return Boolean(match && match[1] === authority.principal && match[2] === authority.fingerprint);
}
export function verifyCommitWithCandidateSshPolicy(repoRoot, candidateSha, sha, authority = CONSOLE_CANDIDATE_SIGNING_AUTHORITY) {
  const policy = policyFromCandidate(repoRoot, candidateSha); assertPolicy(policy, authority);
  const directory = mkdtempSync(path.join(tmpdir(), 'console-allowed-signers-')); const policyPath = path.join(directory, 'allowed_signers');
  try {
    writeFileSync(policyPath, policy, { mode: 0o600 }); chmodSync(policyPath, 0o600);
    const result = spawnSync('git', ['-C', repoRoot, '-c', 'gpg.format=ssh', '-c', 'gpg.ssh.program=ssh-keygen', '-c', `gpg.ssh.allowedSignersFile=${policyPath}`, 'verify-commit', '--raw', sha], { encoding: 'utf8' });
    if (result.error) fail(`git verify-commit unavailable: ${result.error.message}`);
    const status = `${result.stdout ?? ''}${result.stderr ?? ''}`;
    if (result.status !== 0 || !sshSignatureMatchesAuthority(status, authority)) fail('git verify-commit rejected the exact trusted SSH signature');
    return status;
  } finally { rmSync(directory, { recursive: true, force: true }); }
}
