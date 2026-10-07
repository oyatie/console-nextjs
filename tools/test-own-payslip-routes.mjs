// Real production HTTP negatives; this does not fabricate a signed-in response.
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { once } from "node:events";
import { createServer } from "node:net";
import { randomBytes } from "node:crypto";
import { setTimeout as delay } from "node:timers/promises";

const listener = createServer().listen(0, "127.0.0.1");
await once(listener, "listening");
const port = listener.address().port;
await new Promise((resolve, reject) => listener.close((error) => error ? reject(error) : resolve()));
const origin = `http://127.0.0.1:${port}`;
const context = "00000000-0000-4000-8000-000000000001";
const id = "00000000-0000-4000-8000-000000000002";
const server = spawn(process.execPath, ["server.mjs"], { env: {
  ...process.env, NODE_ENV: "production", HOSTNAME: "127.0.0.1", PORT: String(port),
  CONSOLE_PUBLIC_ORIGIN: origin, CONSOLE_BACKEND_ORIGIN: "http://127.0.0.1:9",
  CONSOLE_BROWSER_ALLOW_LOOPBACK_HTTP: "true",
  CONSOLE_BROWSER_INGRESS_KEY: randomBytes(32).toString("base64url"),
  CONSOLE_BROWSER_PREAUTH_KEY: randomBytes(32).toString("base64url"),
}, stdio: ["ignore", "pipe", "pipe"] });
let outputBytes = 0;
for (const stream of [server.stdout, server.stderr]) stream.on("data", (bytes) => { outputBytes += bytes.length; });
try {
  let ready = false;
  for (let attempt = 0; attempt < 60; attempt++) {
    assert.equal(server.exitCode, null, "production listener must stay running");
    try { await fetch(origin, { redirect: "manual" }); ready = true; break; } catch { await delay(250); }
  }
  assert.ok(ready, "production listener startup");
  for (const path of [`/me/${context}/payslips/`, `/me/${context}/payslips/${id}/`]) {
    const response = await fetch(origin + path);
    assert.equal(response.status, 200, "missing access must render the explicit recovery page");
    assert.match(response.headers.get("cache-control"), /no-store/);
    assert.equal(response.headers.get("set-cookie"), null);
    const html = await response.text();
    assert.match(html, /접근 정보를 확인할 수 없습니다/);
    assert.match(html, /다시 로그인/);
    assert.doesNotMatch(html, /<[^>]+\bid="console-browser-private-region"/);
  }
  for (const path of [
    `/me/invalid/payslips/`, `/me/${context}/payslips/invalid/`,
    `/me/${context}/payslips/?before=invalid`, `/me/${context}/payslips/?recipient=${id}`,
    `/me/${context}/payslips/?before=${id}&before=${id}`, `/me/${context}/payslips/${id}/?before=${id}`,
  ]) {
    const response = await fetch(origin + path);
    assert.equal(response.status, 200);
    assert.match(await response.text(), /입력 정보를 확인하세요/);
    assert.equal(response.headers.get("set-cookie"), null);
  }
  assert.ok(outputBytes < 1048576, "bounded listener diagnostics");
  console.log("8 production payslip route negatives passed; no authenticated/financial acceptance claimed.");
} finally {
  server.kill("SIGTERM");
  if (server.exitCode === null && server.signalCode === null) {
    const force = setTimeout(() => server.kill("SIGKILL"), 2000);
    await once(server, "exit"); clearTimeout(force);
  }
}
