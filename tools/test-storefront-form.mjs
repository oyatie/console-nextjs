// Focused browser regression for inquiry recovery; the local API is a test fixture,
// not real-service or two-site acceptance evidence.
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { once } from "node:events";
import { createServer as createHttpServer } from "node:http";
import { createServer as createTcpServer } from "node:net";
import { setTimeout as delay } from "node:timers/promises";
import { chromium, expect } from "@playwright/test";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { stageProductionRuntime } from "./production-runtime.mjs";

let inquiryStatus = 500;
let inquiryPosts = 0;
const inquiryBodies = [];
let catalogStatus = 200;
let catalogGets = 0;
const listing = {
  id: "00000000-0000-4000-8000-000000000001",
  model_name: "전동 지게차",
  kind: "ELECTRIC",
  condition: "USED",
  listing_type: "SALE",
  status: "PUBLISHED",
  capacity_milli: 2500,
  model_year: 2022,
  usage_hours: 100,
  price_won: 10000000,
  availability: "판매 가능",
  location: "서울",
  description: "공개 장비",
  media: [],
};
const api = createHttpServer(async (request, response) => {
  if (request.method === "GET" && request.url?.startsWith("/api/v1/storefront/listings?")) {
    catalogGets += 1;
    response.writeHead(catalogStatus, { "Content-Type": "application/json" });
    response.end(JSON.stringify(catalogStatus === 200
      ? { items: [], total: 0, limit: 24, offset: 0 }
      : { error: "unavailable" }));
  } else if (request.method === "GET" && request.url === `/api/v1/storefront/listings/${listing.id}`) {
    response.writeHead(200, { "Content-Type": "application/json" });
    response.end(JSON.stringify(listing));
  } else if (request.method === "POST" && request.url === "/api/v1/storefront/inquiries") {
    const chunks = [];
    for await (const chunk of request) chunks.push(chunk);
    inquiryBodies.push(JSON.parse(Buffer.concat(chunks).toString()));
    inquiryPosts += 1;
    response.writeHead(inquiryStatus, { "Content-Type": "application/json" });
    response.end(JSON.stringify(inquiryStatus === 202 ? { status: "received" } : { error: "unavailable" }));
  } else {
    response.writeHead(404).end();
  }
});
let reservation;
let owned;
let next;
let browser;
let serverOutput = "";
try {
  api.listen(0, "127.0.0.1");
  await once(api, "listening");
  const apiPort = api.address().port;

  reservation = createTcpServer().listen(0, "127.0.0.1");
  await once(reservation, "listening");
  const sitePort = reservation.address().port;
  await new Promise((resolve) => reservation.close(resolve));
  owned = await mkdtemp(path.join(tmpdir(), "storefront-form-runtime-"));
  const runtime = path.join(owned, "runtime");
  await stageProductionRuntime({ sourceRoot: path.resolve(path.dirname(fileURLToPath(import.meta.url)), ".."), destination: runtime });
  next = spawn(process.execPath, [path.join(runtime, "server.mjs")], {
    cwd: runtime,
    env: { ...process.env, NODE_ENV: "production", PORT: String(sitePort), HOSTNAME: "127.0.0.1", CONSOLE_BACKEND_ORIGIN: `http://127.0.0.1:${apiPort}` },
    stdio: ["ignore", "pipe", "pipe"],
  });
  for (const stream of [next.stdout, next.stderr]) stream.on("data", (chunk) => { serverOutput += chunk; });
  const url = `http://127.0.0.1:${sitePort}/storefront`;
  let ready = false;
  for (let attempt = 0; attempt < 80; attempt += 1) {
    if (next.exitCode !== null) throw new Error(`Next exited: ${serverOutput}`);
    try {
      ready = (await fetch(url)).status === 200;
      if (ready) break;
    } catch { /* server is still starting */ }
    await delay(250);
  }
  assert.ok(ready, `Storefront did not start: ${serverOutput}`);

  browser = await chromium.launch();
  const page = await browser.newPage();
  await page.goto(url);
  await page.getByRole("textbox", { name: "성함" }).fill("홍길동");
  await page.getByRole("textbox", { name: "전화번호" }).fill("010-1234-5678");
  await page.getByRole("textbox", { name: "문의 내용" }).fill("장비 상담을 원합니다.");
  await page.getByRole("button", { name: "문의 접수" }).click();
  await expect(page.locator("form [role=alert]")).toContainText("접수 결과를 확인할 수 없습니다");
  await expect(page.getByRole("textbox", { name: "성함" })).toHaveValue("홍길동");
  await expect(page.getByRole("textbox", { name: "전화번호" })).toHaveValue("010-1234-5678");
  await expect(page.getByRole("textbox", { name: "문의 내용" })).toHaveValue("장비 상담을 원합니다.");
  assert.equal(inquiryPosts, 1);

  inquiryStatus = 202;
  await page.getByRole("button", { name: "문의 접수" }).click();
  await expect(page.getByRole("status")).toContainText("최종 접수 확인은 아직 할 수 없으니");
  await expect(page.getByRole("status")).toBeFocused();
  await expect(page.getByRole("button", { name: "문의 접수" })).toHaveCount(0);
  assert.equal(inquiryPosts, 2);

  catalogStatus = 503;
  await page.reload();
  await expect(page.locator("main [role=alert]")).toContainText("장비 정보를 불러올 수 없습니다");
  const beforeRetry = catalogGets;
  catalogStatus = 200;
  await page.getByRole("button", { name: "다시 시도" }).click();
  await expect(page.getByText("현재 공개된 장비가 없습니다.")).toBeVisible();
  assert.ok(catalogGets > beforeRetry, "Retry must re-fetch the server catalog");

  inquiryStatus = 409;
  await page.goto(`http://127.0.0.1:${sitePort}/storefront/${listing.id}`);
  await page.getByRole("textbox", { name: "성함" }).fill("김고객");
  await page.getByRole("textbox", { name: "전화번호" }).fill("010-9999-8888");
  await page.getByRole("textbox", { name: "문의 내용" }).fill("이 지게차를 문의합니다.");
  await page.getByRole("button", { name: "문의 접수" }).click();
  await expect(page.locator("form [role=alert]")).toContainText("선택한 장비가 변경되었거나");
  assert.equal(inquiryBodies.at(-1).listing_id, listing.id);
  const beforeSwitch = inquiryPosts;
  await page.getByRole("button", { name: "일반 문의로 전환" }).click();
  await expect(page.getByRole("heading", { name: "일반 문의하기" })).toBeVisible();
  await expect(page.locator("form [role=status]")).toBeFocused();
  await page.keyboard.press("Tab");
  await expect(page.getByRole("button", { name: "문의 접수" })).toBeFocused();
  await expect(page.getByRole("textbox", { name: "성함" })).toHaveValue("김고객");
  await expect(page.getByRole("textbox", { name: "전화번호" })).toHaveValue("010-9999-8888");
  await expect(page.getByRole("textbox", { name: "문의 내용" })).toHaveValue("이 지게차를 문의합니다.");
  await expect(page.locator('input[name="listing_id"]')).toHaveCount(0);
  assert.equal(inquiryPosts, beforeSwitch, "Switching inquiry context must not submit");
  inquiryStatus = 202;
  await page.getByRole("button", { name: "문의 접수" }).click();
  await expect(page.getByRole("status")).toContainText("최종 접수 확인은 아직 할 수 없으니");
  await expect(page.getByRole("status")).toBeFocused();
  assert.equal(inquiryPosts, beforeSwitch + 1);
  assert.ok(!("listing_id" in inquiryBodies.at(-1)), "General inquiry must omit the former listing ID");
  console.log("Storefront browser fixture: uncertain input, pending receipt, SSR retry, and explicit listing-conflict recovery passed.");
} finally {
  const released = await Promise.allSettled([
    (async () => { await browser?.close(); })(),
    (async () => {
      if (!next) return;
      next.kill();
      if (next.exitCode === null && next.signalCode === null) {
        const force = setTimeout(() => next.kill("SIGKILL"), 2000);
        try { await once(next, "exit"); } finally { clearTimeout(force); }
      }
    })(),
    (async () => {
      if (reservation?.listening) await new Promise((resolve) => reservation.close(resolve));
    })(),
    (async () => {
      if (api.listening) await new Promise((resolve) => api.close(resolve));
    })(),
  ]);
  try {
    const failed = released.find((result) => result.status === "rejected");
    if (failed) throw failed.reason;
  } finally {
    if (owned) await rm(owned, { recursive: true, force: true });
  }
}
