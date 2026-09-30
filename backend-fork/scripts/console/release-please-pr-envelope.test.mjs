import assert from 'node:assert/strict';
import test from 'node:test';
import {
  RELEASE_PLEASE_ACTION_BODY_PREFIX,
  RELEASE_PLEASE_ACTION_BODY_SUFFIX,
  RELEASE_PLEASE_PENDING_LABEL,
  assertReleasePleasePrEnvelope,
  deriveReleasePleasePrEnvelope,
} from './release-please-pr-envelope.mjs';

const HEAD_REF = 'release-please--branches--dev--components--console';
const NOTES = [
  '## [0.3.9](https://github.com/oyatie/console/compare/v0.3.8...v0.3.9) (2026-08-24)',
  '',
  '',
  '### Bug Fixes',
  '',
  '* **release:** preserve the exact protected fallback',
].join('\n');
const PRIOR = [
  '## [0.3.8](https://github.com/oyatie/console/compare/v0.3.7...v0.3.8) (2026-08-19)',
  '',
  '* prior release',
  '',
].join('\n');
const BASE_CHANGELOG = `# Changelog\n\n${PRIOR}`;
const HEAD_CHANGELOG = `# Changelog\n\n${NOTES}\n\n${PRIOR}`;
const baseManifest = Buffer.from('{\n  ".": "0.3.8"\n}\n');
const headManifest = Buffer.from('{\n  ".": "0.3.9"\n}\n');

const fixture = (overrides = {}) => ({
  baseManifest,
  headManifest,
  baseChangelog: Buffer.from(BASE_CHANGELOG),
  headChangelog: Buffer.from(HEAD_CHANGELOG),
  subject: 'chore(dev): release 0.3.9',
  headRef: HEAD_REF,
  ...overrides,
});

test('derives the exact native Release Please title, body, labels, and path envelope', () => {
  const envelope = deriveReleasePleasePrEnvelope(fixture());
  assert.deepEqual(envelope, {
    headBranchName: HEAD_REF,
    baseBranchName: 'dev',
    title: 'chore(dev): release 0.3.9',
    body: `${RELEASE_PLEASE_ACTION_BODY_PREFIX}${NOTES}${RELEASE_PLEASE_ACTION_BODY_SUFFIX}`,
    labels: [RELEASE_PLEASE_PENDING_LABEL],
    files: [],
    releasePaths: ['.release-please-manifest.json', 'CHANGELOG.md'],
    version: '0.3.9',
  });
  assert.equal(assertReleasePleasePrEnvelope({ ...envelope, files: [] }, envelope), envelope);
});

test('rejects malformed UTF-8, CRLF, truncation, and ambiguous changelog insertion', () => {
  for (const changed of [
    { baseChangelog: Buffer.from([0xc3, 0x28]) },
    { headChangelog: Buffer.from(HEAD_CHANGELOG.replaceAll('\n', '\r\n')) },
    { headChangelog: Buffer.from(`# Changelog\n\n${NOTES}`) },
    { headChangelog: Buffer.from(`# Changelog\n\n\n${NOTES}\n\n${PRIOR}`) },
    { baseChangelog: Buffer.from('# Changelog\n\n') },
  ]) {
    assert.throws(() => deriveReleasePleasePrEnvelope(fixture(changed)));
  }
});

test('rejects manifest, leading changelog version, subject version, and ref drift', () => {
  for (const changed of [
    { headManifest: Buffer.from('{\n  ".": "0.3.8"\n}\n') },
    { headManifest: Buffer.from('{".":"0.3.9"}\n') },
    { headChangelog: Buffer.from(HEAD_CHANGELOG.replaceAll('0.3.9', '0.4.0')) },
    { subject: 'chore(dev): release 0.4.0' },
    { subject: 'fix: not a release' },
    { headRef: 'feature/release' },
  ]) {
    assert.throws(() => deriveReleasePleasePrEnvelope(fixture(changed)));
  }
});

test('rejects PR body overflow and any action-output metadata drift', () => {
  const oversizedNotes = `## [0.3.9](https://github.com/oyatie/console/compare/v0.3.8...v0.3.9) (2026-08-24)\n${'x'.repeat(65_536)}`;
  assert.throws(
    () => deriveReleasePleasePrEnvelope(fixture({
      headChangelog: Buffer.from(`# Changelog\n\n${oversizedNotes}\n\n${PRIOR}`),
    })),
    /exceeds/,
  );
  const expected = deriveReleasePleasePrEnvelope(fixture());
  for (const mutation of [
    { ...expected, title: 'chore(dev): release 9.9.9' },
    { ...expected, body: `${expected.body}\n` },
    { ...expected, labels: [] },
    { ...expected, baseBranchName: 'main' },
    { ...expected, files: null },
  ]) {
    assert.throws(() => assertReleasePleasePrEnvelope(mutation, expected));
  }
});
