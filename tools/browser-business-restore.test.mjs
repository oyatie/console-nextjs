// Lifecycle units only; genuine restoration remains required by P15/P16.
import assert from "node:assert/strict";
import test from "node:test";
import { waitForRestoreCookiePrerequisites } from "./browser-business-restore.mjs";

const session = { name: "__Host-console-session-live", value: "unit-session", expires: 900 };
const preauth = { name: "__Host-console-preauth-pending", value: "unit-envelope", expires: 2 };

test("restore prerequisites do nothing when no preauth cookie remains", async () => {
  let reads = 0;
  await waitForRestoreCookiePrerequisites({ async cookies() { reads++; return [session]; } });
  assert.equal(reads, 1);
});

test("restore prerequisites observe natural expiry without clearing or replacing cookies", async () => {
  let reads = 0;
  const pending = waitForRestoreCookiePrerequisites({ async cookies() {
    return ++reads === 1 ? [preauth, session] : [session];
  } });
  await pending;
  assert.equal(reads, 2);
});

test("an unrelated cookie change cannot be excused by preauth expiry", async () => {
  let reads = 0;
  const pending = waitForRestoreCookiePrerequisites({ async cookies() {
    return ++reads === 1 ? [preauth, session] : [{ ...session, value: "changed" }];
  } });
  await assert.rejects(pending, /change surviving cookies/);
});

test("premature preauth disappearance fails the recorded expiry check", async () => {
  let reads = 0;
  await assert.rejects(waitForRestoreCookiePrerequisites({ async cookies() {
    return ++reads === 1 ? [{ ...preauth, expires: Date.now() / 1000 + 60 }, session] : [session];
  } }), /actually elapsed recorded expiry/);
});

test("a session cannot disappear even when its recorded expiry has elapsed", async () => {
  let reads = 0;
  await assert.rejects(waitForRestoreCookiePrerequisites({ async cookies() {
    return ++reads === 1 ? [preauth, session] : [];
  } }), /preserve every other cookie/);
});

test("a persistent preauth cookie reaches the original bounded deadline", async (t) => {
  let reads = 0;
  t.mock.method(performance, "now", () => reads === 1 ? 0 : 70_000);
  await assert.rejects(waitForRestoreCookiePrerequisites({ async cookies() {
    reads++;
    return [preauth, session];
  } }), /did not naturally expire/);
  assert.equal(reads, 2);
});
