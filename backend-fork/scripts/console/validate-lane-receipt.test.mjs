import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import test, { after } from 'node:test';

const cli = fileURLToPath(new URL('./validate-lane-receipt.mjs', import.meta.url));
const schemaPath = fileURLToPath(new URL('./lane-receipt.schema.json', import.meta.url));
const repoRoot = fileURLToPath(new URL('../../', import.meta.url));
const SHA = 'b2acd80c4d7f340199b9147f6df9318d74af5f8d';
const BASE = '4417bb377a1b2c3d4e5f60718293a4b5c6d7e8f9';

// Suite floor: an emptied suite must not stay green. Every test body increments; the
// after() hook fails if fewer than FLOOR bodies executed (same idiom as SCANNED_FLOOR).
// Pin-to-current-count discipline: bump FLOOR when adding a test, exactly like the
// 29/122/366 preflight pins.
const FLOOR = 22;
let executed = 0;
after(() => {
  assert.ok(executed >= FLOOR, `suite floor: ${executed} test bodies executed, expected >= ${FLOOR}`);
});

function validLane(overrides = {}) {
  return {
    kind: 'lane',
    lane: 'v-lane-validator',
    status: 'done',
    summary: 'Tracked lane-receipt schema, validator CLI, and regression suite.',
    filesChanged: ['scripts/console/lane-receipt.schema.json'],
    redBaseline: 'Planted-red: schema-violating receipt exits 1 before implementation.',
    verification: 'node --test scripts/console/validate-lane-receipt.test.mjs — all pass.',
    contractBreaches: 'none',
    headSha: SHA,
    baseSha: BASE,
    worktree: '/Users/jasonlee/Developer/console/.worktrees/v-lane-validator',
    commands: ['node --test scripts/console/validate-lane-receipt.test.mjs'],
    commandsRun: ['node --test scripts/console/validate-lane-receipt.test.mjs'],
    enforcementPlacement:
      'WHERE: runs in the CI preflight suite; subject is every kind-bearing tracked receipt; finest data-source distinction is one receipt field; examined-zero FAILS with exit 1.',
    peripheralsUpdated: 'scripts/console/lane-receipt.schema.json',
    classification: { personalData: false, holdAdjacent: false, notes: '' },
    preMortem: 'A silent pass on an empty argv list would mint a false green.',
    blastRadius: 'Tracked receipts and the console validator CLI only.',
    detection: 'node --test scripts/console/validate-lane-receipt.test.mjs',
    rollback: 'Revert the three scripts/console/lane-receipt files.',
    stopConditions: 'Stop if validation would require editing package.json or workflows.',
    reviewIdentities: ['v-lane-validator'],
    remainingHolds: [],
    result: 'green',
    followUps: [],
    ...overrides,
  };
}

function validCritic(overrides = {}) {
  return {
    kind: 'critic',
    verdict: 'APPROVE',
    findings: [],
    ...overrides,
  };
}

function finding(overrides = {}) {
  return {
    severity: 'minor',
    claim: 'claim',
    failureScenario: 'scenario',
    location: 'file:1',
    provenByExecution: false,
    ownerLease: false,
    ...overrides,
  };
}

function writeReceipt(dir, name, value) {
  const path = join(dir, name);
  writeFileSync(path, `${JSON.stringify(value, null, 2)}\n`);
  return path;
}

function run(args) {
  return spawnSync(process.execPath, [cli, ...args], { encoding: 'utf8' });
}

function withTemp(fn) {
  const dir = mkdtempSync(join(tmpdir(), 'lane-receipt-'));
  try {
    return fn(dir);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
}

test('valid lane receipt passes', () => {
  executed += 1;
  withTemp((dir) => {
    const result = run([writeReceipt(dir, 'lane.json', validLane())]);
    assert.equal(result.status, 0, result.stderr);
  });
});

test('valid critic receipt passes', () => {
  executed += 1;
  withTemp((dir) => {
    const result = run([writeReceipt(dir, 'critic.json', validCritic())]);
    assert.equal(result.status, 0, result.stderr);
  });
});

test('missing enforcementPlacement FAILS', () => {
  executed += 1;
  withTemp((dir) => {
    const receipt = validLane();
    delete receipt.enforcementPlacement;
    const result = run([writeReceipt(dir, 'missing-enforcement.json', receipt)]);
    assert.equal(result.status, 1, result.stderr);
    assert.match(result.stderr, /enforcementPlacement missing required field/);
  });
});

test('missing redBaseline FAILS (BUILD_SCHEMA union field)', () => {
  executed += 1;
  withTemp((dir) => {
    const receipt = validLane();
    delete receipt.redBaseline;
    const result = run([writeReceipt(dir, 'missing-redbaseline.json', receipt)]);
    assert.equal(result.status, 1, result.stderr);
    assert.match(result.stderr, /redBaseline missing required field/);
  });
});

test('empty-string command in commandsRun FAILS', () => {
  executed += 1;
  withTemp((dir) => {
    const result = run([writeReceipt(dir, 'empty-command.json', validLane({ commandsRun: ['node --test', ''] }))]);
    assert.equal(result.status, 1, result.stderr);
    assert.match(result.stderr, /commandsRun\[1] must be a non-empty string/);
  });
});

test('whitespace-only command in commandsRun FAILS (false-green guard)', () => {
  executed += 1;
  withTemp((dir) => {
    const result = run([writeReceipt(dir, 'blank-command.json', validLane({ commandsRun: ['node --test', ' '] }))]);
    assert.equal(result.status, 1, result.stderr);
    assert.match(result.stderr, /commandsRun\[1] must not be blank/);
  });
});

test('status=done without commands FAILS (incumbent parity)', () => {
  executed += 1;
  withTemp((dir) => {
    const receipt = validLane();
    delete receipt.commands;
    const result = run([writeReceipt(dir, 'done-no-commands.json', receipt)]);
    assert.equal(result.status, 1, result.stderr);
    assert.match(result.stderr, /status=done requires commands/);
  });
});

test('bad headSha FAILS', () => {
  executed += 1;
  withTemp((dir) => {
    const result = run([writeReceipt(dir, 'bad-sha.json', validLane({ headSha: 'not-a-sha' }))]);
    assert.equal(result.status, 1, result.stderr);
    assert.match(result.stderr, /headSha must be 40-hex/);
  });
});

test('n/a enforcement escape accepts case-insensitive prefix (incumbent parity)', () => {
  executed += 1;
  withTemp((dir) => {
    const result = run([writeReceipt(dir, 'na-escape.json', validLane({
      enforcementPlacement: 'N/A - adds no enforcement; validator tooling only.',
    }))]);
    assert.equal(result.status, 0, result.stderr);
  });
});

test('enforcementPlacement without WHERE/sequence/subject and without n/a escape FAILS', () => {
  executed += 1;
  withTemp((dir) => {
    const result = run([writeReceipt(dir, 'keyword-less.json', validLane({
      enforcementPlacement: 'It is checked somehow, trust me.',
    }))]);
    assert.equal(result.status, 1, result.stderr);
    assert.match(result.stderr, /enforcementPlacement/);
  });
});

test('peripheralsUpdated empty array FAILS', () => {
  executed += 1;
  withTemp((dir) => {
    const result = run([writeReceipt(dir, 'empty-peripherals.json', validLane({ peripheralsUpdated: [] }))]);
    assert.equal(result.status, 1, result.stderr);
    assert.match(result.stderr, /peripheralsUpdated/);
  });
});

test('critic finding missing provenByExecution FAILS', () => {
  executed += 1;
  withTemp((dir) => {
    const receipt = validCritic({ findings: [finding()] });
    delete receipt.findings[0].provenByExecution;
    const result = run([writeReceipt(dir, 'missing-proven.json', receipt)]);
    assert.equal(result.status, 1, result.stderr);
    assert.match(result.stderr, /findings\[0]\.provenByExecution missing required field/);
  });
});

test('verdict BLOCK with empty findings FAILS', () => {
  executed += 1;
  withTemp((dir) => {
    const result = run([writeReceipt(dir, 'block-empty.json', validCritic({ verdict: 'BLOCK', findings: [] }))]);
    assert.equal(result.status, 1, result.stderr);
    assert.match(result.stderr, /findings empty findings array is valid only with verdict APPROVE/);
  });
});

test('APPROVE over a blocker finding FAILS (convergence tie-break)', () => {
  executed += 1;
  withTemp((dir) => {
    const result = run([writeReceipt(dir, 'approve-blocker.json', validCritic({
      findings: [finding({ severity: 'blocker', provenByExecution: true })],
    }))]);
    assert.equal(result.status, 1, result.stderr);
    assert.match(result.stderr, /APPROVE conflicts with 1 blocking finding/);
  });
});

test('APPROVE over a proven major FAILS; unproven major passes; leased blocker passes', () => {
  executed += 1;
  withTemp((dir) => {
    const provenMajor = run([writeReceipt(dir, 'approve-proven-major.json', validCritic({
      findings: [finding({ severity: 'major', provenByExecution: true })],
    }))]);
    assert.equal(provenMajor.status, 1, provenMajor.stderr);
    assert.match(provenMajor.stderr, /APPROVE conflicts/);

    const unprovenMajor = run([writeReceipt(dir, 'approve-unproven-major.json', validCritic({
      findings: [finding({ severity: 'major', provenByExecution: false })],
    }))]);
    assert.equal(unprovenMajor.status, 0, unprovenMajor.stderr);

    const leasedBlocker = run([writeReceipt(dir, 'approve-leased-blocker.json', validCritic({
      findings: [finding({ severity: 'blocker', provenByExecution: true, ownerLease: true })],
    }))]);
    assert.equal(leasedBlocker.status, 0, leasedBlocker.stderr);
  });
});

test('zero-input invocation FAILS', () => {
  executed += 1;
  const result = run([]);
  assert.equal(result.status, 1, result.stderr);
  assert.match(result.stderr, /examined zero receipts/);
});

test('--dir scan of a directory with zero kind-bearing receipts FAILS', () => {
  executed += 1;
  withTemp((dir) => {
    writeReceipt(dir, 'legacy.json', { status: 'done', summary: 'legacy receipt without kind' });
    const result = run(['--dir', dir]);
    assert.equal(result.status, 1, result.stderr);
    assert.match(result.stderr, /examined zero kind-bearing receipts/);
  });
});

test('--dir scan validates kind-bearing receipts and fails on a bad one', () => {
  executed += 1;
  withTemp((dir) => {
    writeReceipt(dir, 'good.json', validLane());
    const good = run(['--dir', dir]);
    assert.equal(good.status, 0, good.stderr);

    const bad = validLane({ headSha: 'not-a-sha' });
    writeReceipt(dir, 'bad.json', bad);
    const mixed = run(['--dir', dir]);
    assert.equal(mixed.status, 1, mixed.stderr);
    assert.match(mixed.stderr, /headSha must be 40-hex/);
  });
});

test('--dir scan FAILS a present-but-unrecognised kind instead of skipping it (planted red)', () => {
  executed += 1;
  withTemp((dir) => {
    writeReceipt(dir, 'good.json', validLane());
    writeReceipt(dir, 'misspelled.json', { kind: 'build', status: 'done' });
    const result = run(['--dir', dir]);
    assert.equal(result.status, 1, 'a kind-bearing receipt with an unrecognised kind must fail, not be skipped');
    assert.match(result.stderr, /kind must be lane\|critic/);
  });
});

test('--dir scan of the real tracked receipts directory examines at least this lane receipt and passes', () => {
  executed += 1;
  const result = run(['--dir', join(repoRoot, 'scripts', 'console', 'fixtures')]);
  assert.equal(result.status, 0, result.stderr);
});

test('cross-authority parity: BUILD_SCHEMA required fields are a subset of laneReceipt required', () => {
  executed += 1;
  const laneFanout = readFileSync(join(repoRoot, 'scripts', 'console', 'workflows', 'lane-fanout.js'), 'utf8');
  const schema = JSON.parse(readFileSync(schemaPath, 'utf8'));

  const buildMatch = laneFanout.match(/const BUILD_SCHEMA = \{[^]*?required: \[([^\]]*)\]/);
  assert.ok(buildMatch, 'BUILD_SCHEMA required list must be extractable (examined-zero fails)');
  const buildRequired = [...buildMatch[1].matchAll(/'([^']+)'/g)].map((m) => m[1]);
  // Exact pin, not a one-way floor: a same-size substitution inside BUILD_SCHEMA.required
  // (e.g. swapping 'verification' for an already-present lane-only field) must go red here.
  assert.deepEqual(
    [...buildRequired].sort(),
    ['contractBreaches', 'enforcementPlacement', 'filesChanged', 'peripheralsUpdated', 'redBaseline', 'status', 'summary', 'verification'],
    'BUILD_SCHEMA.required drifted from the pinned cross-authority list',
  );
  const laneRequired = new Set(schema.$defs.laneReceipt.required);
  for (const field of buildRequired) {
    assert.ok(laneRequired.has(field), `laneReceipt.required must include BUILD_SCHEMA field "${field}"`);
  }

  const reviewMatch = laneFanout.match(/const REVIEW_SCHEMA = \{[^]*?required: \[([^\]]*)\]/);
  assert.ok(reviewMatch, 'REVIEW_SCHEMA required list must be extractable (examined-zero fails)');
  const reviewRequired = [...reviewMatch[1].matchAll(/'([^']+)'/g)].map((m) => m[1]);
  assert.deepEqual(
    [...reviewRequired].sort(),
    ['findings', 'verdict'],
    'REVIEW_SCHEMA.required drifted from the pinned cross-authority list',
  );
  const criticRequired = new Set(schema.$defs.criticReceipt.required);
  for (const field of reviewRequired) {
    assert.ok(criticRequired.has(field), `criticReceipt.required must include REVIEW_SCHEMA field "${field}"`);
  }
});

test('this lane receipt validates under both the tracked and incumbent cursor validators', () => {
  executed += 1;
  const receiptPath = join(repoRoot, 'scripts', 'console', 'fixtures', 'v-lane-receipt-validator-20260812.json');
  const tracked = run([receiptPath]);
  assert.equal(tracked.status, 0, tracked.stderr);

  const incumbent = spawnSync(
    process.execPath,
    [join(repoRoot, 'scripts', 'cursor', 'validate-lane-receipt.mjs'), receiptPath],
    { encoding: 'utf8' },
  );
  assert.equal(incumbent.status, 0, incumbent.stderr);
});
