import assert from 'node:assert/strict';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { pathToFileURL } from 'node:url';
import test from 'node:test';

import { repositoryRoot } from './mjs-test-reachability.mjs';

test('repository root follows the utility location instead of a workstation path or cwd', () => {
  const checkout = mkdtempSync(path.join(tmpdir(), 'console-reachability-'));
  try {
    const moduleUrl = pathToFileURL(path.join(checkout, 'scripts', 'dev', 'mjs-test-reachability.mjs'));
    assert.equal(repositoryRoot(moduleUrl), checkout);
  } finally {
    rmSync(checkout, { recursive: true, force: true });
  }
});
