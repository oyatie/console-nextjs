import assert from "node:assert/strict";
import test from "node:test";
import { browserFailureLocation } from "./browser-business-failure.mjs";

const url = new URL("browser-business-temporal.mjs", import.meta.url).href;
function failure(stack, message = "private") {
  const error = new Error(message);
  error.stack = stack;
  return error;
}

test("only the first exact probe frame yields a fixed basename and bounded numbers", () => {
  for (const frame of [`    at run (${url}:194:12)`, `    at ${url}:194:12`]) {
    const result = browserFailureLocation(failure(`Error: private\n    at node:assert:1:2\n${frame}\n    at ${url}:485:9`));
    assert.deepEqual(result, { file: "browser-business-temporal.mjs", line: 194, column: 12 });
  }
});

test("sensitive assertion headers and unrelated paths never enter diagnostic output", () => {
  const secret = "bs1.private-cookie __Host-console-session secret-response-body";
  const result = browserFailureLocation(failure(`AssertionError: ${secret}\n${secret}\n    at /private/customer/${secret}:12:34\n    at check (${url}:123:45)`, `${secret}\n${secret}`));
  assert.equal(JSON.stringify(result), '{"file":"browser-business-temporal.mjs","line":123,"column":45}');
  assert.ok(!JSON.stringify(result).includes(secret));
  for (const suffix of [".other", "/child", "?token=private", "#private"]) {
    assert.equal(browserFailureLocation(failure(`Error: private\n    at ${url}${suffix}:123:45`)), undefined);
  }
});

test("absent, malformed, overlong, and unsafe diagnostic inputs are omitted", () => {
  for (const input of [null, {}, { stack: `    at ${url}:1:2` }, "secret"]) {
    assert.equal(browserFailureLocation(input), undefined);
  }
  for (const stack of [undefined, 123, "Error: private", `Error: private\n    at ${url}:0:1`,
    `Error: private\n    at ${url}:1:0`, `Error: private\n    at ${url}:10000:1`, `Error: private\n    at ${url}:1:10000`,
    `Error: private\n    at ${url}:1:2 private`, `Error: private\n${"x".repeat(65537)}`]) {
    assert.equal(browserFailureLocation(failure(stack)), undefined);
  }
  const error = new Error("private");
  Object.defineProperty(error, "stack", { get() { throw new Error("private getter failure"); } });
  assert.equal(browserFailureLocation(error), undefined);
});

test("native multiline assertion messages cannot forge a source location", () => {
  let error;
  try { assert.equal(1, 2, `private\n    at ${url}:999:999`); } catch (caught) { error = caught; }
  assert.equal(browserFailureLocation(error), undefined);
  error.stack += `\n    at actual (${url}:123:45)`;
  assert.deepEqual(browserFailureLocation(error), { file: "browser-business-temporal.mjs", line: 123, column: 45 });
});

test("unverified or accessor-backed messages are omitted without invoking their getter", () => {
  const error = new Error("private");
  error.stack = `Error: private\n    at ${url}:123:45`;
  let reads = 0;
  const get = () => { reads += 1; return "private"; };
  Object.defineProperty(error, "message", { get, configurable: true });
  assert.equal(browserFailureLocation(error), undefined);
  delete error.message;
  Object.setPrototypeOf(error, Object.create(Error.prototype, { message: { get } }));
  assert.equal(browserFailureLocation(error), undefined);
  assert.equal(reads, 0);
  assert.equal(browserFailureLocation(failure(`Error: different\n    at ${url}:123:45`)), undefined);
  const inheritedStack = new Error("private");
  delete inheritedStack.stack;
  Object.setPrototypeOf(inheritedStack, Object.create(Error.prototype, { stack: { get } }));
  assert.equal(browserFailureLocation(inheritedStack), undefined);
  assert.equal(reads, 0);
});

test("issued-payslip failures retain only their exact safe source location", () => {
  const payslip = new URL("browser-payslips.mjs", import.meta.url).href;
  assert.deepEqual(browserFailureLocation(failure(`Error: private\n    at check (${payslip}:60:12)\n    at ${url}:485:9`)),
    { file: "browser-payslips.mjs", line: 60, column: 12 });
  assert.equal(browserFailureLocation(failure(`Error: private\n    at ${payslip}?token=private:60:12`)), undefined);
});
