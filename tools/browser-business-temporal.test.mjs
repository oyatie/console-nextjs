import assert from "node:assert/strict";
import test from "node:test";
import { assertReadCookies } from "./browser-business-temporal.mjs";

const live = { name: "session-live", value: "opaque-test-value", expires: 101, httpOnly: true, secure: true };
const elapsed = { name: "preauth-elapsed", value: "opaque-test-envelope", expires: 99, httpOnly: true, secure: true };

test("an elapsed preauthentication cookie may disappear while every live cookie survives unchanged", () => {
  assertReadCookies([elapsed, live], [live], 100);
  assertReadCookies([elapsed, live], [elapsed, live], 100);
  assertReadCookies([elapsed], [], 99);
});

test("a live or session cookie cannot disappear", () => {
  for (const expires of [101, -1, 0, NaN, Infinity, undefined]) {
    assert.throws(() => assertReadCookies([{ ...live, expires }], [], 100));
  }
});

test("an observation before expiry cannot excuse later disappearance", () => {
  assert.throws(() => assertReadCookies([elapsed], [], 98.999));
});

test("no cookie may be added even when its expiry has elapsed", () => {
  assert.throws(() => assertReadCookies([live], [live, elapsed], 100));
});

test("every surviving cookie attribute must remain exact", () => {
  for (const patch of [{ value: "replacement" }, { expires: 102 }, { httpOnly: false }, { secure: false }, { path: "/other" }]) {
    assert.throws(() => assertReadCookies([live], [{ ...live, ...patch }], 100));
  }
});

test("duplicate names in either observation fail closed", () => {
  assert.throws(() => assertReadCookies([live, live], [live], 100));
  assert.throws(() => assertReadCookies([live], [live, live], 100));
});

test("an invalid observation time cannot authorize disappearance", () => {
  for (const time of [NaN, Infinity, -1, 0, undefined, "100"]) {
    assert.throws(() => assertReadCookies([elapsed], [], time));
  }
});
