import assert from "node:assert/strict";

// Same real passkey users/runtime/Inbox artifacts as the canonical browser probe.
// No API interception or successful provider fixture is used here.
export async function runPayslipJourney(h) {
  const actor = h.actors.get("a");
  const other = h.actors.get("b");
  const session = h.sessions.get("a");
  const home = `/me/${session.context}/payslips/`;
  const documents = actor.facts.payslips;
  assert.equal(documents.length, 26);
  const initialCookies = await actor.context.cookies();
  const firstNavigation = actor.page.waitForURL((url) => url.pathname === home);
  await actor.page.getByRole("link", { name: "급여명세서", exact: true }).click(); await firstNavigation;
  await actor.page.locator("#console-browser-private-region").waitFor({ state: "visible" });
  assert.equal(await actor.page.locator(".browser-document-list li").count(), 25);
  const html = await actor.page.content(); h.assertNoProof(html);
  for (const row of other.facts.payslips) assert.ok(!html.includes(row.title));
  const next = actor.page.waitForURL((url) => url.pathname === home && url.searchParams.has("before"));
  await actor.page.getByRole("link", { name: "이전 명세서", exact: true }).click(); await next;
  const returnTo = new URL(actor.page.url());
  await actor.page.locator("#console-browser-private-region").waitFor({ state: "visible" });
  assert.equal(await actor.page.locator(".browser-document-list li").count(), 1);
  assert.equal(await actor.page.getByRole("link", { name: "이전 명세서", exact: true }).count(), 0);
  const issued = documents[0];
  const opened = actor.page.waitForURL((url) => url.pathname === `${home}${issued.id}/`);
  await actor.page.getByRole("link", { name: issued.title, exact: true }).click(); await opened;
  await actor.page.locator("#console-browser-private-region").waitFor({ state: "visible" });
  let text = await actor.page.locator("#console-browser-private-region").innerText();
  assert.ok(text.includes("9,007,199,254,740,993원"));
  assert.ok(text.includes("9,007,199,254,740,992원"));
  assert.ok(text.includes("은행 이체"));
  assert.equal(await actor.page.getByRole("table").count(), 1);
  await actor.page.setViewportSize({ width: 360, height: 780 });
  assert.ok(await actor.page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), "whole page must fit the mobile viewport");
  const back = actor.page.getByRole("link", { name: "명세서 목록으로 돌아가기", exact: true });
  await back.focus(); assert.equal(await back.evaluate((node) => node === document.activeElement), true);
  const reloaded = await actor.page.reload();
  assert.equal(reloaded.status(), 200);
  assert.match(reloaded.headers()["cache-control"], /no-store/);
  assert.equal((await reloaded.headersArray()).filter((header) => header.name.toLowerCase() === "set-cookie").length, 0);
  await actor.page.locator("#console-browser-private-region").waitFor({ state: "visible" });
  h.assertNoProof(await actor.page.content());
  const returning = actor.page.waitForURL((url) => url.href === returnTo.href);
  await actor.page.getByRole("link", { name: "명세서 목록으로 돌아가기", exact: true }).click(); await returning;
  for (const id of [other.facts.payslips[0].id, "00000000-0000-4000-8000-000000000000"]) {
    const response = await actor.page.goto(`${actor.origin}${home}${id}/`);
    assert.match(response.headers()["cache-control"], /no-store/);
    text = await actor.page.locator("body").innerText();
    assert.ok(text.includes("명세서를 찾을 수 없습니다"));
    assert.ok(!text.includes("9,007,199,254,740,993"));
    assert.equal(await actor.page.locator("#console-browser-private-region").count(), 0);
  }
  // Reads retain the same cookie and its exact originally observed deadline.
  const currentCookies = await actor.context.cookies();
  const selected = (cookies) => cookies.filter((cookie) => cookie.name === `__Host-console-session-${session.context}`);
  assert.deepEqual(selected(currentCookies), selected(initialCookies));
  const storage = await actor.page.evaluate(async () => ({ local: Object.keys(localStorage), session: Object.keys(sessionStorage),
    indexed: (await indexedDB.databases()).map((db) => db.name), cache: await caches.keys() }));
  assert.deepEqual(storage, { local: [], session: [], indexed: [], cache: [] });
  await actor.page.setViewportSize({ width: 1280, height: 900 });
  await actor.page.goto(`${actor.origin}/me/${session.context}/attendance/`);
  await h.checkTable("a", actor.facts.first_page);
}

export async function checkPayslipLogout(h, context) {
  const actor = h.actors.get("a");
  const response = await actor.page.goto(`${actor.origin}/me/${context}/payslips/${actor.facts.payslips[0].id}/`);
  assert.match(response.headers()["cache-control"], /no-store/);
  const text = await actor.page.locator("body").innerText();
  assert.ok(text.includes("접근 정보를 확인할 수 없습니다"));
  assert.ok(!text.includes("9,007,199,254,740,993"));
  assert.equal(await actor.page.locator("#console-browser-private-region").count(), 0);
}
