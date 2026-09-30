import { test, expect } from "@playwright/test";

test.describe("User Story 4: Cryptographic WORM Audit Trail & Cedar Access Governance", () => {
  test("inspects cryptographic hash chain and explores Discord-style layered roles", async ({
    page,
  }) => {
    let actionCount = 0;

    // 1. Visit Governance Audit page
    await page.goto("/gov/audit");
    actionCount++;
    await expect(page.locator("header").filter({ hasText: "감사 로그 & 무결성 해시체인" })).toBeVisible();

    // Verify audit records are visible
    await expect(page.locator("text=누적 업무 수행 원장")).toBeVisible();
    const rows = await page.locator("tbody tr").count();
    expect(rows).toBeGreaterThan(0);

    // Switch to Technical & Cryptographic tab
    const techTab = page.getByRole("button", { name: /암호학적 무결성 및 기술 로그/ });
    await expect(techTab).toBeVisible();
    await techTab.click();
    actionCount++;

    await page.waitForTimeout(300);

    // Verify cryptographic SHA-256 hash indicators
    await expect(page.locator("text=현재 SHA-256 해시")).toBeVisible();
    await expect(page.locator("text=부모 해시 (prevHash)")).toBeVisible();

    // 2. Navigate to Governance Access & PBAC page
    await page.goto("/gov/access");
    actionCount++;
    await expect(page.locator("header").filter({ hasText: "Cedar PBAC 권한 거버넌스 및 진단" })).toBeVisible();
    await expect(page.locator("text=Cedar 보안 사규")).toBeVisible();
    await expect(page.locator("text=다중 역할 레이어 카탈로그 (Discord Model")).toBeVisible();

    // 3. Switch to Access Inspector tab
    const inspectorTab = page.getByRole("button", { name: /접근 권한 진단기/ });
    await expect(inspectorTab).toBeVisible();
    await inspectorTab.click();
    actionCount++;

    await page.waitForTimeout(300);
    // Verify SoD scenario evaluation option
    await expect(page.locator("text=대표 내부통제 검증 시나리오")).toBeVisible();
    await expect(page.locator("button:has-text('SoD 위반 차단')")).toBeVisible();

    expect(actionCount).toBeLessThan(10);
  });
});
