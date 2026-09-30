// Launched by the Rust sqlx::test: no database credentials or fixture API live here.
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { once } from "node:events";
import { createServer } from "node:net";
import { createInterface } from "node:readline";
import { setTimeout as delay } from "node:timers/promises";
import { chromium, expect } from "@playwright/test";

const origin = process.env.REAL_SERVICE_API_ORIGIN;
const listingId = process.env.REAL_SERVICE_LISTING_ID;
assert.match(origin ?? "", /^http:\/\/127\.0\.0\.1:\d+$/);
assert.match(listingId ?? "", /^[0-9a-f-]{36}$/i);

const reservation = createServer().listen(0, "127.0.0.1");
await once(reservation, "listening");
const port = reservation.address().port;
await new Promise((resolve) => reservation.close(resolve));

const next = spawn(process.execPath, ["node_modules/next/dist/bin/next", "start", "-p", String(port)], {
  cwd: process.cwd(),
  env: { ...process.env, HOSTNAME: "127.0.0.1", CONSOLE_BACKEND_ORIGIN: origin },
  stdio: ["ignore", "pipe", "pipe"],
});
let serverOutput = "";
for (const stream of [next.stdout, next.stderr]) {
  stream.on("data", (chunk) => { serverOutput = (serverOutput + chunk).slice(-16000); });
}
const input = createInterface({ input: process.stdin });
const replies = input[Symbol.asyncIterator]();
async function checkpoint(marker, expectedReply) {
  const pending = replies.next();
  process.stdout.write(`${marker}\n`);
  const { value, done } = await pending;
  assert.ok(!done, `Rust closed the ${marker} checkpoint`);
  assert.equal(value, expectedReply);
}

let browser;
try {
  const url = `http://127.0.0.1:${port}/storefront/${listingId}`;
  let ready = false;
  for (let attempt = 0; attempt < 120; attempt += 1) {
    if (next.exitCode !== null) throw new Error(`Next exited: ${serverOutput}`);
    try {
      ready = (await fetch(url)).status === 200;
      if (ready) break;
    } catch { /* startup in progress */ }
    await delay(250);
  }
  assert.ok(ready, `Next or real Rust listing detail did not start: ${serverOutput}`);

  browser = await chromium.launch();
  const page = await browser.newPage();
  await page.goto(url);
  await expect(page.getByRole("heading", { name: "실제 서비스 문의 장비" })).toBeVisible();
  await page.getByRole("textbox", { name: "성함" }).fill("김고객");
  await page.getByRole("textbox", { name: "전화번호" }).fill("010-9999-8888");
  await page.getByRole("textbox", { name: "문의 내용" }).fill("이 장비를 문의합니다.");
  await checkpoint("DETAIL_READY", "WITHDRAWN");

  await page.getByRole("button", { name: "문의 접수" }).click();
  await expect(page.locator("form [role=alert]")).toContainText("선택한 장비가 변경되었거나");
  await expect(page.getByRole("heading", { name: "이 장비 문의하기" })).toBeVisible();
  await expect(page.getByRole("textbox", { name: "성함" })).toHaveValue("김고객");
  await expect(page.getByRole("textbox", { name: "문의 내용" })).toHaveValue("이 장비를 문의합니다.");
  await checkpoint("CONFLICT_SEEN", "CHECKED");

  await page.getByRole("button", { name: "일반 문의로 전환" }).click();
  await expect(page.getByRole("heading", { name: "일반 문의하기" })).toBeVisible();
  await expect(page.locator('input[name="listing_id"]')).toHaveCount(0);
  await expect(page.locator("form [role=status]")).toBeFocused();
  await expect(page.getByRole("textbox", { name: "전화번호" })).toHaveValue("010-9999-8888");
  await checkpoint("GENERAL_SELECTED", "CHECKED");

  await page.getByRole("button", { name: "문의 접수" }).click();
  await expect(page.getByRole("status")).toContainText("최종 접수 확인은 아직 할 수 없으니");
  await expect(page.getByRole("button", { name: "문의 접수" })).toHaveCount(0);
} finally {
  input.close();
  await browser?.close();
  next.kill();
  if (next.exitCode === null && next.signalCode === null) {
    const force = setTimeout(() => next.kill("SIGKILL"), 2000);
    await once(next, "exit");
    clearTimeout(force);
  }
}
