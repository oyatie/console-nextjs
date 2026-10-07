// Synthetic, local-only CLI regressions; no institutional connection is exercised.
import test from 'node:test';
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { generateKeyPairSync } from 'node:crypto';
import {
  chmodSync, lstatSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync,
  statSync, symlinkSync, writeFileSync,
} from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';

const agent = fileURLToPath(new URL('./local-certificate-agent.mjs', import.meta.url));
const sentinel = 'SYNTHETIC_PRIVATE_SENTINEL_7e921';
const constantError = 'local certificate agent failed';

function sandbox(t) {
  const root = mkdtempSync(join(tmpdir(), 'synthetic-certificate-agent-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  return root;
}

function run(args, extraEnv = {}) {
  const oldUmask = process.umask(0);
  try {
    return spawnSync(process.execPath, [agent, ...args], {
      encoding: 'utf8',
      timeout: 30000,
      env: { ...process.env, ...extraEnv },
    });
  } finally {
    process.umask(oldUmask);
  }
}

function assertFailureIsPrivate(result) {
  assert.notEqual(result.status, 0, 'CLI must fail');
  assert.ok(!result.stdout.includes(sentinel) && !result.stderr.includes(sentinel),
    'CLI must not echo supplied private values');
  assert.ok(result.stderr.trim() === constantError,
    'CLI failure text must be one constant nonsecret line');
}

test('fixture proof output is 0600 under a permissive umask and keeps its contract', (t) => {
  const root = sandbox(t);
  const output = join(root, 'fixture.json');
  const result = run(['--out', output]);
  assert.equal(result.status, 0, 'fixture CLI must succeed');
  assert.equal(statSync(output).mode & 0o777, 0o600);
  const parsed = JSON.parse(readFileSync(output, 'utf8'));
  const reported = JSON.parse(result.stdout);
  assert.equal(parsed.execution_mode, 'fixture_only');
  assert.equal(parsed.server_envelope.accepted_boundary, 'SIGNED_PROOF_ONLY');
  assert.equal(Object.hasOwn(reported, "out"), false);
  assert.equal(result.stdout.includes(output), false);
  assert.equal(reported.ok, true);
  assert.deepEqual(readdirSync(root), ['fixture.json']);
});

test('existing output and symlink are rejected without changing their targets', (t) => {
  const root = sandbox(t);
  const existing = join(root, 'existing.json');
  writeFileSync(existing, 'original synthetic artifact', { mode: 0o600 });
  assertFailureIsPrivate(run(['--out', existing]));
  assert.equal(readFileSync(existing, 'utf8'), 'original synthetic artifact');

  const destination = join(root, 'destination.json');
  writeFileSync(destination, 'outside synthetic artifact', { mode: 0o600 });
  const link = join(root, 'link.json');
  symlinkSync(destination, link);
  assertFailureIsPrivate(run(['--out', link]));
  assert.equal(lstatSync(link).isSymbolicLink(), true);
  assert.equal(readFileSync(destination, 'utf8'), 'outside synthetic artifact');
  assert.deepEqual(readdirSync(root).sort(), ['destination.json', 'existing.json', 'link.json']);
});

test('unknown argument text is never returned in a CLI failure', (t) => {
  const root = sandbox(t);
  const output = join(root, 'failure.json');
  assertFailureIsPrivate(run(['--out', output, `--unknown-${sentinel}`]));
  assert.deepEqual(readdirSync(root), []);
});

test('wrong local fixture-key password fails privately without publishing proof', (t) => {
  const root = sandbox(t);
  const password = 'correct-fixture-key-password';
  const { privateKey, publicKey } = generateKeyPairSync('rsa', { modulusLength: 2048 });
  const privateDer = join(root, 'private.der');
  const publicDer = join(root, 'public.der');
  writeFileSync(privateDer, privateKey.export({
    type: 'pkcs8', format: 'der', cipher: 'aes-256-cbc', passphrase: password,
  }), { mode: 0o600 });
  writeFileSync(publicDer, publicKey.export({ type: 'spki', format: 'der' }), { mode: 0o600 });
  const output = join(root, 'proof.json');
  const result = run([
    '--allow-local-key-files', '--sign-pri-key', privateDer,
    '--sign-cert-der', publicDer, '--cert-password-env', 'SYNTHETIC_CERT_PASSWORD',
    '--out', output,
  ], { SYNTHETIC_CERT_PASSWORD: `wrong-${sentinel}` });
  assertFailureIsPrivate(result);
  assert.deepEqual(readdirSync(root).sort(), ['private.der', 'public.der']);
});

test('writable output parent is rejected without publishing proof', (t) => {
  const root = sandbox(t);
  const unsafe = join(root, 'unsafe');
  mkdirSync(unsafe);
  chmodSync(unsafe, 0o777);
  assertFailureIsPrivate(run(['--out', join(unsafe, 'proof.json')]));
  assert.deepEqual(readdirSync(unsafe), []);
});

test('writable output ancestor is rejected without publishing proof', (t) => {
  const root = sandbox(t);
  const unsafe = join(root, 'unsafe-ancestor');
  mkdirSync(unsafe);
  chmodSync(unsafe, 0o777);
  const parent = join(unsafe, 'private');
  mkdirSync(parent, { mode: 0o700 });
  assertFailureIsPrivate(run(['--out', join(parent, 'proof.json')]));
  assert.deepEqual(readdirSync(parent), []);
});

test('missing output parent is rejected without creating directories', (t) => {
  const root = sandbox(t);
  assertFailureIsPrivate(run(['--out', join(root, 'missing', 'proof.json')]));
  assert.deepEqual(readdirSync(root), []);
});
