import { test, expect } from "@playwright/test";

test.describe("User Story 3: Approval Workflows (AP-) & Field Work Orders (WO-)", () => {
  test("creates an approval draft, inspects approval stages, and issues a field work order", async ({
    page,
  }) => {
    let actionCount = 0;

    // 1. Visit Approvals page
    await page.goto("/approvals");
    actionCount++;
    await expect(page.locator("header").filter({ hasText: "결재 및 인테이크 대장" })).toBeVisible();

    // 2. Click "새 기안문서 작성"
    const newDraftBtn = page.getByRole("button", { name: "새 기안문서 작성" });
    await expect(newDraftBtn).toBeVisible();
    await newDraftBtn.click();
    actionCount++;

    // 3. Verify Composer Modal opened
    await expect(page.locator("text=신규 전자결재 기안 작성")).toBeVisible();

    const draftTitle = `[현장 정비비] 긴급 크레인 부품 수급 품의 ${Date.now().toString().slice(-4)}`;
    await page.locator('input[placeholder*="사전 승인의 건"]').fill(draftTitle);
    actionCount++;

    await page.locator('textarea[placeholder*="품의 사유"]').fill("평택 40톤 크레인 유압호스 파손 긴급 수리 건");
    actionCount++;

    // Submit Draft
    const submitDraftBtn = page.getByRole("button", { name: "상신 제출 (게이트 검증)" });
    await submitDraftBtn.click();
    actionCount++;

    // Verify Draft in list
    await expect(page.locator(`text=${draftTitle}`).first()).toBeVisible({ timeout: 5000 });

    // 4. Navigate to Operations Work Orders page
    await page.goto("/ops/work-orders");
    actionCount++;
    await expect(page.locator("header").filter({ hasText: "현장 작업오더 & 장비 관리" })).toBeVisible();

    // 5. Click "현장 작업오더 발행"
    const newWoBtn = page.getByRole("button", { name: "현장 작업오더 발행" });
    await expect(newWoBtn).toBeVisible();
    await newWoBtn.click();
    actionCount++;

    // 6. Fill Work Order Modal
    await expect(page.locator("text=신규 현장 작업오더 (WO-) 발행")).toBeVisible();
    const woTitle = `유압호스 정비 교체 ${Date.now().toString().slice(-4)}`;
    await page.locator('input[placeholder*="유압 호스"]').fill(woTitle);
    actionCount++;

    // Submit Work Order
    const submitWoBtn = page.getByRole("button", { name: "작업오더 발행", exact: true });
    await submitWoBtn.click();
    actionCount++;

    // Verify Work Order appeared
    await expect(page.locator(`text=${woTitle}`).first()).toBeVisible({ timeout: 5000 });

    expect(actionCount).toBeLessThan(12);
  });
});
