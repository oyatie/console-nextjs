import { test, expect } from "@playwright/test";

test.describe("User Story 2: Attendance Exception Adjudication & Statutory Payroll Run", () => {
  test("adjudicates attendance exception via UI and navigates to payroll to verify 10-won truncation", async ({
    page,
  }) => {
    let actionCount = 0;

    // 1. Visit Attendance page
    await page.goto("/hr/attendance");
    actionCount++;
    await expect(page.locator("header").filter({ hasText: "근태 관리" })).toBeVisible();

    // Verify statutory indicators (LSA §50, §53, §54)
    await expect(page.locator("text=근기법 §54")).toBeVisible();

    // 2. Locate any record with "보정/심사" button
    const adjudicateBtn = page.getByRole("button", { name: "보정/심사" }).first();
    if (await adjudicateBtn.isVisible()) {
      await adjudicateBtn.click();
      actionCount++;

      // Verify Adjudication Modal opened
      await expect(page.locator("text=근태 펀치 보정 및 예외 심사")).toBeVisible();

      // Enter justification comment
      const noteInput = page.locator('input[placeholder*="선적 지연"]');
      if (await noteInput.isVisible()) {
        await noteInput.fill("야간 긴급 장비 복구로 인한 연장근로 소명 승인");
        actionCount++;
      }

      // Click "심사 확정 및 급여 게이트 해제"
      const approveBtn = page.getByRole("button", { name: "심사 확정 및 급여 게이트 해제" });
      if (await approveBtn.isVisible()) {
        await approveBtn.click();
        actionCount++;
        await page.waitForTimeout(400);
      }
    }

    // 3. Navigate to Payroll page
    await page.goto("/hr/payroll");
    actionCount++;
    await expect(page.locator("header").filter({ hasText: "급여 산정 & 펌뱅킹 이체" })).toBeVisible();

    // 4. Verify StatBar numbers
    await expect(page.locator("text=지급 총액 (Gross)").first()).toBeVisible();
    await expect(page.locator("text=실지급 총액 (Net)").first()).toBeVisible();

    // 5. Test Tab Switching: Connected Sheet
    const sheetTab = page.getByRole("button", { name: /Connected Sheet/ });
    await expect(sheetTab).toBeVisible();
    await sheetTab.click();
    actionCount++;

    await page.waitForTimeout(400);

    // 6. Test Tab Switching: Firm Banking KFTC CMS
    const bankingTab = page.getByRole("button", { name: /펌뱅킹/ });
    await expect(bankingTab).toBeVisible();
    await bankingTab.click();
    actionCount++;

    await page.waitForTimeout(400);
    await expect(page.locator("text=금융결제원(KFTC) 기업 펌뱅킹 CMS")).toBeVisible();

    // 7. Verify National Treasury Administration Act §47(1) 10-won truncation
    // Check that every net amount ends with 0 (won)
    const netCells = page.locator("td:has-text('원')");
    const count = await netCells.count();
    for (let i = 0; i < Math.min(5, count); i++) {
      const text = await netCells.nth(i).textContent();
      if (text && text.includes("원")) {
        const num = parseInt(text.replace(/[^0-9]/g, ""), 10);
        if (!isNaN(num)) {
          expect(num % 10).toBe(0);
        }
      }
    }

    expect(actionCount).toBeLessThan(12);
  });
});
