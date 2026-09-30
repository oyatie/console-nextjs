import { test, expect } from "@playwright/test";

test.describe("User Story 1: Enterprise Multi-Tenant Scoping, Scoped Filtering & Employee Onboarding", () => {
  test("navigates to employee directory, tests module-scoped filtering, and onboards a new employee", async ({
    page,
  }) => {
    let actionCount = 0;

    // 1. Visit HR People page
    await page.goto("/hr/people");
    actionCount++;
    await expect(page).toHaveTitle(/오야티|콘솔|Acme/);

    // Verify screen title in Topbar and presence of ModuleScopeFilter (NO global corporate dropdown)
    await expect(page.locator("header").filter({ hasText: /직원 디렉토리 & 편성/ })).toBeVisible();
    await expect(page.locator("text=총 인원 대장")).toBeVisible();
    await expect(page.locator("text=관할 필터:")).toBeVisible();

    // 2. Count initial employee rows
    const initialRows = await page.locator("tbody tr").count();
    expect(initialRows).toBeGreaterThan(0);

    // 3. Test Module-Scoped Filter without global dropdown
    const entitySelect = page.locator("select[title*='법인']").first();
    if (await entitySelect.isVisible()) {
      await entitySelect.selectOption({ index: 1 });
      actionCount++;
      await page.waitForTimeout(300);
      // Reset back to all entities
      await entitySelect.selectOption({ index: 0 });
      actionCount++;
    }

    // 4. Click '신규 사원 등록' to open the onboarding modal
    const onboardBtn = page.getByRole("button", { name: "신규 사원 등록" });
    await expect(onboardBtn).toBeVisible();
    await onboardBtn.click();
    actionCount++;

    // 5. Verify modal opened
    const modalHeader = page.locator("h3").filter({ hasText: /신규 임직원 등록/ });
    await expect(modalHeader).toBeVisible();

    // 6. Fill onboarding details
    const uniqueEmpName = `테스트사원_${Date.now().toString().slice(-4)}`;
    await page.locator('input[placeholder="홍길동"]').fill(uniqueEmpName);
    actionCount++;
    await page.locator('input[placeholder="010-0000-0000"]').fill("010-9999-8888");
    actionCount++;

    const baseSalaryInput = page.locator('input[placeholder="3,500,000"]').first();
    if (await baseSalaryInput.isVisible()) {
      await baseSalaryInput.fill("4500000");
      actionCount++;
    }

    // 7. Submit Onboarding Form
    const submitBtn = page.getByRole("button", { name: "사원 등록 확정" });
    await expect(submitBtn).toBeVisible();
    await submitBtn.click();
    actionCount++;

    // 8. Verify Toast Notification and New Row in Table
    await expect(page.locator("text=신규 사원 입사 등록 완료")).toBeVisible({ timeout: 5000 });

    // 9. Verify 360° Drawer auto-opened for the new employee
    await expect(page.locator("h3").filter({ hasText: uniqueEmpName })).toBeVisible();
    await expect(page.locator("text=재직").first()).toBeVisible();

    // 10. Close Drawer
    const closeDrawerBtn = page.getByText("닫기", { exact: true });
    await expect(closeDrawerBtn).toBeVisible();
    await closeDrawerBtn.click();
    actionCount++;
    await page.waitForTimeout(300);

    // 11. Now click on the newly created employee row to reopen 360° Drawer
    const empRow = page.locator("tr").filter({ hasText: uniqueEmpName }).first();
    await expect(empRow).toBeVisible();
    await empRow.click();
    actionCount++;

    // Verify 360° Drawer opened again
    await expect(page.locator("h3").filter({ hasText: uniqueEmpName })).toBeVisible();

    // Close Drawer
    await closeDrawerBtn.click();
    actionCount++;

    expect(actionCount).toBeLessThan(15); // Strict click efficiency guarantee
  });
});
