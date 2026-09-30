#!/usr/bin/env node
import assert from "node:assert/strict";
import { randomUUID } from "node:crypto";
import { test } from "node:test";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import {
  WORK_ORDER_REQUEST_NO_CHECK,
  fixtureRequestNoFromWorkOrderId,
  rejectKnownBadFixtureRequestNo,
  uuidStringToU128,
} from "./work-order-request-no-seed.mjs";

const root = join(dirname(fileURLToPath(import.meta.url)), "../..");

test("migration 0008 still pins the request_no CHECK regex this module encodes", () => {
  const sql = readFileSync(
    join(root, "backend/crates/platform/db/migrations/0008_create_work_orders.sql"),
    "utf8",
  );
  assert.match(sql, /request_no\s+TEXT\s+NOT NULL/);
  assert.match(sql, /request_no\s*~\s*'\^\[0-9\]\{8\}-\[0-9\]\{3\}\$'/);
  // Module constant must stay in lockstep with the migration.
  assert.equal(WORK_ORDER_REQUEST_NO_CHECK.source, "^[0-9]{8}-[0-9]{3}$");
});

test("fixtureRequestNoFromWorkOrderId always satisfies the CHECK", () => {
  for (let i = 0; i < 500; i++) {
    const id = randomUUID();
    const requestNo = fixtureRequestNoFromWorkOrderId(id);
    assert.match(requestNo, WORK_ORDER_REQUEST_NO_CHECK, id);
    assert.equal(rejectKnownBadFixtureRequestNo(requestNo), null, requestNo);
  }
});

test("fixture derivation is deterministic for a fixed UUID", () => {
  const id = "04c0915d-d853-4646-a8d4-cc24e94e29c8";
  const a = fixtureRequestNoFromWorkOrderId(id);
  const b = fixtureRequestNoFromWorkOrderId(uuidStringToU128(id));
  assert.equal(a, b);
  assert.match(a, WORK_ORDER_REQUEST_NO_CHECK);
  // The hosted 23514 failure used EVD-{uuid}; derived form must not match that.
  assert.notEqual(a, `EVD-${id}`);
});

test("rejects EVD-uuid and other non-CHECK patterns that burned hosted wall", () => {
  const id = randomUUID();
  // Hosted 23514 on #579: EVD-{uuid} violates CHECK.
  assert.notEqual(rejectKnownBadFixtureRequestNo(`EVD-${id}`), null);
  assert.notEqual(rejectKnownBadFixtureRequestNo("WO-1"), null);
  assert.notEqual(rejectKnownBadFixtureRequestNo("123"), null);
  assert.notEqual(rejectKnownBadFixtureRequestNo("abcdefgh-001"), null);
  // Valid CHECK shape (uniqueness is a separate DB concern).
  assert.equal(rejectKnownBadFixtureRequestNo("20260805-042"), null);
});

test("uuid%1000 alone is not a full request_no (collision class)", () => {
  // Document the old anti-pattern: only 1000 values, not CHECK-complete by itself.
  const suffix = Number(uuidStringToU128(randomUUID()) % 1000n);
  const bare = String(suffix).padStart(3, "0");
  assert.notEqual(rejectKnownBadFixtureRequestNo(bare), null);
});

test("many random fixtures produce many distinct request_nos (uniqueness pressure)", () => {
  const set = new Set();
  for (let i = 0; i < 2000; i++) {
    set.add(fixtureRequestNoFromWorkOrderId(randomUUID()));
  }
  // 11 bits of structure (8+3 digits from u128) still yields high cardinality for 2k samples.
  assert.ok(set.size > 1900, `expected high uniqueness, got ${set.size}`);
});
