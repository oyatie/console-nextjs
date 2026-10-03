import { test, expect } from "@playwright/test";


test.describe("Visual & Interactive Verification Suite across Cockpit Modules", () => {
  test.beforeEach(async ({ page }) => {
    // Set standard enterprise desktop viewport (1440x900)
    await page.setViewportSize({ width: 1440, height: 900 });
  });

  test("01. HR People Directory & Workday-style 360° Profile Drawer", async ({ page }) => {
    await page.goto("/hr/people");
    await expect(page.locator("header").filter({ hasText: /직원 디렉토리 & 편성/ })).toBeVisible();
    await expect(page.locator("text=총 인원 대장")).toBeVisible();

    // Verify no horizontal overflow
    const hasHorizontalOverflow = await page.evaluate(() => {
      return document.documentElement.scrollWidth > window.innerWidth;
    });
    expect(hasHorizontalOverflow).toBe(false);

    // Capture main directory table
    await page.screenshot({
      path: test.info().outputPath("visual_01_people_directory.png"),
      fullPage: false,
    });

    // Click on the first employee row to open 360° profile drawer
    const firstRow = page.locator("tbody tr").first();
    await firstRow.click();
    await expect(page.locator("text=인적 정체성 (Person Identity)")).toBeVisible();

    // Capture open 360° drawer
    await page.screenshot({
      path: test.info().outputPath("visual_02_people_360_drawer.png"),
      fullPage: false,
    });

    // Close drawer
    const closeBtn = page.getByText("닫기", { exact: true });
    await closeBtn.click();
  });

  test("02. Attendance Cockpit & Statutory 52-Hour Compliance", async ({ page }) => {
    await page.goto("/hr/attendance");
    await expect(page.locator("header").filter({ hasText: "근태 관리" })).toBeVisible();
    await expect(page.locator("text=근기법 §54")).toBeVisible();

    // Capture Attendance screen with Statutory indicators
    await page.screenshot({
      path: test.info().outputPath("visual_03_attendance_cockpit.png"),
      fullPage: false,
    });
  });

  test("03. Payroll Run: Connected Sheet Grid & Firm Banking KFTC CMS", async ({ page }) => {
    await page.goto("/hr/payroll");
    await expect(page.locator("header").filter({ hasText: "급여 산정 & 펌뱅킹 이체" })).toBeVisible();

    // Switch to Connected Sheet
    const sheetTab = page.getByRole("button", { name: /Connected Sheet/ });
    await sheetTab.click();
    await page.waitForTimeout(300);

    // Capture Connected Sheet formula grid
    await page.screenshot({
      path: test.info().outputPath("visual_04_payroll_connected_sheet.png"),
      fullPage: false,
    });

    // Switch to Firm Banking CMS
    const bankingTab = page.getByRole("button", { name: /펌뱅킹/ });
    await bankingTab.click();
    await page.waitForTimeout(300);

    // Capture KFTC Firm Banking flat file protocol view
    await page.screenshot({
      path: test.info().outputPath("visual_05_payroll_firm_banking.png"),
      fullPage: false,
    });
  });

  test("04. Approvals Intake & Operations Work Orders Kanban", async ({ page }) => {
    await page.goto("/approvals");
    await expect(page.locator("header").filter({ hasText: "결재 및 인테이크 대장" })).toBeVisible();

    // Click '새 기안문서 작성' to view Parking composer modal
    const newDraftBtn = page.getByRole("button", { name: "새 기안문서 작성" });
    await newDraftBtn.click();
    await expect(page.locator("text=신규 전자결재 기안 작성")).toBeVisible();

    // Capture Parking composer modal
    await page.screenshot({
      path: test.info().outputPath("visual_06_approvals_parking_composer.png"),
      fullPage: false,
    });

    // Close modal
    await page.getByRole("button", { name: "취소" }).click();

    // Navigate to Operations Work Orders
    await page.goto("/ops/work-orders");
    await expect(page.locator("header").filter({ hasText: "현장 작업오더 & 장비 관리" })).toBeVisible();

    // Capture Operations 5-Stage Kanban board
    await page.screenshot({
      path: test.info().outputPath("visual_07_ops_work_orders_kanban.png"),
      fullPage: false,
    });
  });

  test("05. Cryptographic WORM Audit Chain & Discord Layered Roles", async ({ page }) => {
    await page.goto("/gov/audit");
    await expect(page.locator("header").filter({ hasText: "감사 로그 & 무결성 해시체인" })).toBeVisible();

    // Switch to Technical & Cryptographic tab
    const techTab = page.getByRole("button", { name: /암호학적 무결성 및 기술 로그/ });
    await techTab.click();
    await page.waitForTimeout(300);
    await expect(page.locator("text=현재 SHA-256 해시")).toBeVisible();

    // Capture cryptographic audit chain with SHA-256 blocks
    await page.screenshot({
      path: test.info().outputPath("visual_08_gov_audit_worm_ledger.png"),
      fullPage: false,
    });

    // Navigate to Access & Governance
    await page.goto("/gov/access");
    await expect(page.locator("header").filter({ hasText: "Cedar PBAC 권한 거버넌스 및 진단" })).toBeVisible();

    // Capture Discord-style Layered Role Canvas
    await page.screenshot({
      path: test.info().outputPath("visual_09_gov_access_discord_roles.png"),
      fullPage: false,
    });
  });
});
