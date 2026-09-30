import { expect, test } from "@playwright/test";

test("uses server redirects and dynamic responses without a static API fallback", async ({ request }) => {
  const entry = await request.get("/", { maxRedirects: 0 });
  expect(entry.status()).toBe(307);
  expect(entry.headers().location).toMatch(/^\/dashboard\/?$/);

  const page = await request.get("/dashboard/");
  expect(page.status()).toBe(200);
  expect(page.headers()["cache-control"]).toBe("no-cache, must-revalidate");

  const missing = await request.get("/api/v1/missing-runtime-probe/");
  expect(missing.status()).toBe(404);
});
