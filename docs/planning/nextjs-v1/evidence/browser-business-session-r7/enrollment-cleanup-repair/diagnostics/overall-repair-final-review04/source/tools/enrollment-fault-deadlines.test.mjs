// Deadline-policy units only; no browser, provider or native acceptance.
import assert from "node:assert/strict";
import test from "node:test";
import { superviseEnrollmentFault } from "./enrollment-fault-deadlines.mjs";

function supervision(t) {
  t.mock.timers.enable({ apis: ["setTimeout"] });
  const signals = [];
  const owner = superviseEnrollmentFault({ kill(signal) { signals.push(signal); } });
  t.after(() => owner.clear());
  return { owner, signals };
}

test("slow prerequisite startup does not spend the requested cleanup budget", (t) => {
  const { owner, signals } = supervision(t);
  t.mock.timers.tick(26000);
  assert.deepEqual(signals, []);
  assert.equal(owner.forced, false);
  owner.beginFault();
  t.mock.timers.tick(24999);
  assert.deepEqual(signals, []);
  t.mock.timers.tick(1);
  assert.deepEqual(signals, ["SIGTERM"]);
  assert.equal(owner.forced, true);
  t.mock.timers.tick(10000);
  assert.deepEqual(signals, ["SIGTERM", "SIGKILL"]);
});

test("a ready runner that fails to exit still triggers the cleanup fallback", (t) => {
  const { owner, signals } = supervision(t);
  t.mock.timers.tick(7000);
  owner.beginFault();
  t.mock.timers.tick(24999);
  assert.deepEqual(signals, []);
  t.mock.timers.tick(1);
  assert.deepEqual(signals, ["SIGTERM"]);
  t.mock.timers.tick(9999);
  assert.deepEqual(signals, ["SIGTERM"]);
  t.mock.timers.tick(1);
  assert.deepEqual(signals, ["SIGTERM", "SIGKILL"]);
});

test("never-ready startup has one absolute bounded termination deadline", (t) => {
  const { owner, signals } = supervision(t);
  t.mock.timers.tick(139999);
  assert.deepEqual(signals, []);
  t.mock.timers.tick(1);
  assert.deepEqual(signals, ["SIGTERM"]);
  assert.equal(owner.forced, true);
  assert.throws(() => owner.beginFault());
  t.mock.timers.tick(9999);
  assert.deepEqual(signals, ["SIGTERM"]);
  t.mock.timers.tick(1);
  assert.deepEqual(signals, ["SIGTERM", "SIGKILL"]);
});

test("a second fault cannot reset an already running cleanup deadline", (t) => {
  const { owner, signals } = supervision(t);
  owner.beginFault();
  t.mock.timers.tick(1000);
  assert.throws(() => owner.beginFault());
  t.mock.timers.tick(24000);
  assert.deepEqual(signals, ["SIGTERM"]);
});

test("late cleanup cannot erase evidence of forced intervention", (t) => {
  const { owner, signals } = supervision(t);
  owner.beginFault();
  t.mock.timers.tick(25000);
  assert.deepEqual(signals, ["SIGTERM"]);
  owner.clear();
  t.mock.timers.tick(200000);
  assert.deepEqual(signals, ["SIGTERM"]);
  assert.equal(owner.forced, true);
  assert.throws(() => owner.beginFault());
});

test("natural completion disposes both clocks and cannot rearm them", (t) => {
  const { owner, signals } = supervision(t);
  owner.beginFault();
  owner.clear();
  owner.clear();
  t.mock.timers.tick(200000);
  assert.deepEqual(signals, []);
  assert.equal(owner.forced, false);
  assert.throws(() => owner.beginFault());
});
