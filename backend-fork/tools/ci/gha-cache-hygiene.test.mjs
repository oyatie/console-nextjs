#!/usr/bin/env node
/**
 * Unit tests for GHA cache hygiene policy (no network).
 * Run: node --test tools/ci/gha-cache-hygiene.test.mjs
 */
import { test } from "node:test";
import assert from "node:assert/strict";
import {
  matchesPrefix,
  selectVictims,
  fmtBytes,
} from "./gha-cache-hygiene.mjs";

const day = 24 * 60 * 60 * 1000;
const now = Date.parse("2026-08-05T12:00:00Z");

const cache = (id, key, sizeMb, daysAgo) => ({
  id,
  key,
  size_in_bytes: sizeMb * 1024 * 1024,
  last_accessed_at: new Date(now - daysAgo * day).toISOString(),
});

test("matchesPrefix", () => {
  assert.equal(matchesPrefix("buildkit-blob-abc", ["buildkit-", "index-"]), true);
  assert.equal(matchesPrefix("v0-rust-backend", ["v0-rust-"]), true);
  assert.equal(matchesPrefix("other", ["buildkit-"]), false);
});

test("force-deletes docker prefixes even if KEEP-looking", () => {
  const caches = [
    cache(1, "buildkit-blob-x", 100, 1),
    cache(2, "index-sha256:abc", 50, 1),
    cache(3, "v0-rust-backend-cargo", 500, 1),
  ];
  const { victims } = selectVictims(caches, {
    beforeBytes: 650 * 1024 * 1024,
    maxTotal: 8 * 1024 ** 3,
    maxAgeDays: 14,
    deletePrefixes: ["buildkit-", "index-"],
    keepPrefixes: ["v0-rust-", "node-cache-"],
    nowMs: now,
  });
  const ids = victims.map((v) => v.id).sort();
  assert.deepEqual(ids, [1, 2]);
  assert.ok(victims.every((v) => v.reason === "docker-gha-prefix"));
});

test("never age- or budget-deletes KEEP prefixes", () => {
  const caches = [
    cache(1, "v0-rust-backend-cargo", 7000, 90), // huge + ancient
    cache(2, "node-cache-Linux-x64-npm-abc", 100, 90),
  ];
  const { victims } = selectVictims(caches, {
    beforeBytes: 7100 * 1024 * 1024,
    maxTotal: 1 * 1024 ** 3, // force budget pressure
    maxAgeDays: 14,
    deletePrefixes: ["buildkit-", "index-"],
    keepPrefixes: ["v0-rust-", "node-cache-"],
    nowMs: now,
  });
  assert.equal(victims.length, 0);
});

test("age-deletes non-KEEP older than maxAgeDays", () => {
  const caches = [
    cache(1, "branch-feature-xyz", 200, 20), // stale
    cache(2, "branch-feature-new", 200, 2), // fresh
    cache(3, "v0-rust-backend-cargo", 500, 20), // keep
  ];
  const { victims } = selectVictims(caches, {
    beforeBytes: 900 * 1024 * 1024,
    maxTotal: 8 * 1024 ** 3,
    maxAgeDays: 14,
    deletePrefixes: ["buildkit-", "index-"],
    keepPrefixes: ["v0-rust-", "node-cache-"],
    nowMs: now,
  });
  assert.equal(victims.length, 1);
  assert.equal(victims[0].id, 1);
  assert.equal(victims[0].reason, "age-stale");
});

test("budget-lru deletes oldest non-KEEP until under cap", () => {
  // 3 non-keep caches, 3 GiB each → 9 GiB; max 8 GiB → delete oldest
  const caches = [
    cache(1, "misc-a", 3 * 1024, 10),
    cache(2, "misc-b", 3 * 1024, 5),
    cache(3, "misc-c", 3 * 1024, 1),
    cache(4, "v0-rust-backend", 1 * 1024, 30),
  ];
  const before = 10 * 1024 * 1024 * 1024;
  const { victims, projected } = selectVictims(caches, {
    beforeBytes: before,
    maxTotal: 8 * 1024 ** 3,
    maxAgeDays: 0, // disable age so only budget fires
    deletePrefixes: ["buildkit-", "index-"],
    keepPrefixes: ["v0-rust-", "node-cache-"],
    nowMs: now,
  });
  assert.ok(victims.length >= 1);
  assert.equal(victims[0].id, 1); // oldest first
  assert.equal(victims[0].reason, "budget-lru");
  assert.ok(projected <= 8 * 1024 ** 3);
  assert.ok(!victims.some((v) => v.id === 4));
});

test("fmtBytes", () => {
  assert.equal(fmtBytes(8 * 1024 ** 3), "8.00 GiB");
});
