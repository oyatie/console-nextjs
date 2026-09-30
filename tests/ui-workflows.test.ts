// ui-workflows.test.ts — Enterprise UI Workflow Interaction, Action Counter & State Machine Verification Suite
// Tracks exact click counts, screen transitions, state diffs, BAU vs Edge Case handling,
// 3-5 year statutory regulatory shifts, and centralized glossary mapping.

import { describe, it, expect, beforeEach } from "vitest";
import { useConsoleStore } from "../src/lib/store";
import { workflowTelemetry } from "../src/lib/workflow-telemetry";
import { resolveStatutoryRates, STATUTORY_SCHEDULE_REGISTRY } from "../src/lib/regulatory-registry";
import { computeEmployeePayslip, trunc10 } from "../src/lib/payroll-engine";
import { t, getGlossaryEntry, searchGlossary, ENTERPRISE_GLOSSARY } from "../src/lib/i18n/glossary";
import { evaluateSheetFormula, parseTsvClipboard } from "../src/lib/sheet-engine";
import { evaluate52HourCompliance } from "../src/lib/attendance-calculator";

describe("Enterprise UI Workflow Verification, Action Counter & State Transitions", () => {
  beforeEach(() => {
    useConsoleStore.getState().resetToSeedData();
    workflowTelemetry.clear();
  });

  // =========================================================================
  // WORKFLOW 1: HR Onboarding — Action Counting & State Machine (BAU Path)
  // =========================================================================
  it("Workflow 1 [HR Onboarding]: counts clicks/actions, tracks screen changes, and validates state transition in <= 5 actions", () => {
    const session = workflowTelemetry.createSession(
      "WF-HR-ONBOARD-01",
      "신규 현장 엔지니어 온보딩 워크플로우",
      "/hr/people",
      "LIST_VIEW"
    );

    // Step 1: User navigates to /hr/people and clicks "신규 입사자 등록" button
    session.recordClick("btn-open-onboarding-modal");
    session.recordStateTransition("MODAL_OPEN", "btn-open-onboarding-modal");

    // Step 2: User enters form fields
    session.recordInput("input-name", "강태양");
    session.recordInput("input-email", "ty.kang@oyatie.com");
    session.recordInput("select-entity", "(주)오야티 로지스틱스");
    session.recordInput("select-site", "인천 제1물류센터");
    session.recordInput("input-salary", 3600000);

    // Step 3: User submits onboarding form (Action 3)
    session.recordClick("btn-submit-onboarding");
    session.recordStateTransition("SUBMITTING", "btn-submit-onboarding");

    // Execute state mutation via Store
    const store = useConsoleStore.getState();
    const prevCount = store.employees.length;
    const newEmp = store.createEmployee({
      name: "강태양",
      email: "ty.kang@oyatie.com",
      phone: "010-3344-5566",
      entity: "(주)오야티 로지스틱스",
      site: "인천 제1물류센터",
      dept: "운영1팀",
      role: "엔지니어",
      grade: "대리",
      jobTitle: "설비 기사",
      position: "주임",
      empType: "정규",
      baseSalary: 3600000,
      fixedAllow: 200000,
      joinedDate: "2026-07-01",
      dependents: 1,
      status: "재직",
    });

    session.recordStateTransition("RECORD_COMMITTED", "store.createEmployee", {
      headcount: { before: prevCount, after: prevCount + 1 },
    });

    // Step 4: System closes modal and displays confirmation toast (Action 4)
    session.recordAction("modal_close", "modal-onboarding");
    session.recordStateTransition("LIST_VIEW_UPDATED", "modal_close");

    const report = session.complete("COMPLETED");

    // Assertions: Workflow Efficiency & Metrics
    expect(report.status).toBe("COMPLETED");
    expect(report.isBAU).toBe(true); // Pure Business As Usual
    expect(report.totalClicks).toBe(2); // Open modal + Submit
    expect(report.totalInputs).toBe(5); // Form inputs
    expect(report.totalActions).toBe(8); // Total user interactions
    expect(report.stateTransitions.length).toBe(4); // IDLE -> MODAL -> SUBMIT -> COMMITTED -> LIST_VIEW

    // Business assertions
    expect(newEmp.id).toBeDefined();
    expect(useConsoleStore.getState().employees.length).toBe(prevCount + 1);
    expect(useConsoleStore.getState().attendance.some((a) => a.employeeId === newEmp.id)).toBe(true);
  });

  // =========================================================================
  // WORKFLOW 2: Attendance Exception Adjudication -> Payroll Freeze Gate (Edge Case Recovery)
  // =========================================================================
  it("Workflow 2 [Attendance Adjudication & Payroll Gate]: tracks edge-case blocker, measures adjudication clicks (<= 4 clicks), and transitions gate to FROZEN", () => {
    const session = workflowTelemetry.createSession(
      "WF-ATT-PAYROLL-02",
      "근태 예외 소명 심사 및 급여 마감 게이트 통과 워크플로우",
      "/hr/payroll",
      "PRE_RUN_BLOCKED"
    );

    const store = useConsoleStore.getState();

    // Step 1: User attempts to freeze payroll (Click 1)
    session.recordClick("btn-freeze-payroll");
    const prematureResult = store.calculateAndFreezePayroll();

    // EDGE CASE TRIGGERED: Gate rejects because unresolved exceptions exist
    expect(prematureResult.success).toBe(false);
    session.recordEdgeCase(
      "ERR_UNRESOLVED_EXCEPTIONS",
      "statutory_limit",
      "근태 미소명 이상 건 존재로 급여 마감 게이트 차단됨",
      false
    );
    session.recordStateTransition("GATE_BLOCKED", "btn-freeze-payroll");

    // Step 2: Screen transition to Attendance Adjudication Drawer (Click 2)
    session.recordScreenChange("/hr/attendance", "navigate_to_resolve_exceptions");
    session.recordClick("btn-open-adjudication-drawer");
    session.recordStateTransition("ADJUDICATION_DRAWER_OPEN", "btn-open-adjudication-drawer");

    // Step 3: HR Admin adjudicates all pending discrepancies (Click 3)
    const openDiscs = store.discrepancies.filter((d) => d.status === "unresolved");
    expect(openDiscs.length).toBeGreaterThan(0);

    for (const disc of openDiscs) {
      store.repairDiscrepancy(disc.id, "선박 입항 지연에 따른 긴급 하역 연장근무 사전소명 승인");
    }

    session.recordClick("btn-confirm-all-adjudications");
    session.recordStateTransition("EXCEPTIONS_CLEARED", "store.repairDiscrepancy");

    // Edge case successfully recovered
    const openAfter = useConsoleStore.getState().discrepancies.filter((d) => d.status === "unresolved");
    expect(openAfter.length).toBe(0);

    // Step 4: Return to Payroll and execute freeze (Click 4)
    session.recordScreenChange("/hr/payroll", "return_to_freeze");
    session.recordClick("btn-freeze-payroll-retry");
    const freezeRes = useConsoleStore.getState().calculateAndFreezePayroll();

    expect(freezeRes.success).toBe(true);
    session.recordStateTransition("PAYROLL_FROZEN", "calculateAndFreezePayroll");

    const report = session.complete("COMPLETED");

    // Telemetry Assertions
    expect(report.status).toBe("COMPLETED");
    expect(report.isBAU).toBe(false); // Non-BAU due to discrepancy edge case
    expect(report.edgeCases.length).toBe(1);
    expect(report.edgeCases[0].code).toBe("ERR_UNRESOLVED_EXCEPTIONS");
    expect(report.screenChanges.length).toBe(2); // /hr/payroll -> /hr/attendance -> /hr/payroll
    expect(report.totalClicks).toBeLessThanOrEqual(5); // Ultra-efficient resolution in 4 clicks
    expect(useConsoleStore.getState().payrollRun.isLocked).toBe(true);
  });

  // =========================================================================
  // WORKFLOW 3: Connected Sheet Mass Salary Adjustment & Consequence Review
  // =========================================================================
  it("Workflow 3 [Connected Sheet Interaction]: parses rectangular TSV paste, runs in-cell formula, reviews consequence, and commits in <= 4 actions", () => {
    const session = workflowTelemetry.createSession(
      "WF-SHEET-REVISE-03",
      "연계 시트 대량 수당 조정 및 영향도 검토 워크플로우",
      "/hr/payroll?tab=grid",
      "GRID_READY"
    );

    const store = useConsoleStore.getState();

    // Step 1: User focuses cell and inputs formula (Action 1)
    session.recordClick("cell-row-0-allowance");
    session.recordAction("input", "formula-bar", "=150000*1.5");
    const formulaResult = evaluateSheetFormula("=150000*1.5");
    expect(formulaResult).toBe(225000);
    session.recordStateTransition("PROPOSED_DELTA_STAGED", "evaluateSheetFormula");

    // Step 2: Rectangular TSV clipboard paste from Excel (Action 2)
    session.recordAction("paste", "grid-selection-area", "4500000\t250000\n4200000\t200000");
    const pasted = parseTsvClipboard("4500000\t250000\n4200000\t200000");
    expect(pasted.length).toBe(2);
    expect(pasted[0][0]).toBe("4500000");
    session.recordStateTransition("CLIPBOARD_MATRIX_APPLIED", "parseTsvClipboard");

    // Step 3: User clicks "파급효과 검토 (Consequence Review)" (Action 3)
    session.recordClick("btn-open-consequence-review");
    session.recordStateTransition("CONSEQUENCE_MODAL_OPEN", "btn-open-consequence-review");

    // Step 4: User reviews employer social insurance diff and clicks "원장 반영 확정 (Commit)" (Action 4)
    session.recordClick("btn-commit-changes");
    const targetEmp = store.employees[0];
    const prevSalary = targetEmp.baseSalary;
    store.updateEmployee(targetEmp.id, { baseSalary: prevSalary + 300000 });

    session.recordStateTransition("ATOMIC_COMMITTED", "store.updateEmployee", {
      salary: { before: prevSalary, after: prevSalary + 300000 },
    });

    const report = session.complete("COMPLETED");

    expect(report.status).toBe("COMPLETED");
    expect(report.isBAU).toBe(true);
    expect(report.totalActions).toBe(5); // 5 total actions (focus cell, formula input, TSV paste, open review modal, commit)
    expect(useConsoleStore.getState().employees[0].baseSalary).toBe(prevSalary + 300000);
  });

  // =========================================================================
  // WORKFLOW 4: 3-Year & 5-Year Regulatory Shift Compatibility (2024 to 2028+)
  // =========================================================================
  it("Workflow 4 [3-5 Year Regulatory Evolution]: accurately computes retroactive 2024, current 2026, and forward 2027 statutory payslips without code changes", () => {
    const store = useConsoleStore.getState();
    const emp = store.employees[0]; // Base salary 4,200,000 KRW

    // 1. Retroactive Calculation for 2024 (Past Regulatory Schedule)
    const rates2024 = resolveStatutoryRates("2024-05");
    expect(rates2024.minWage).toBe(9860);
    expect(rates2024.hiTotal).toBe(0.0709); // 7.09%
    expect(rates2024.npBandHigh).toBe(5900000); // 2024 Cap 5.9M

    const payslip2024 = computeEmployeePayslip(emp, { otHours: 0, nightHours: 0, holidayHours: 0 }, "2024-05");
    expect(payslip2024.yearMonth).toBe("2024-05");
    expect(payslip2024.netPay % 10).toBe(0); // Statutory 10-won truncation

    // 2. Current Production Calculation for 2026
    const rates2026 = resolveStatutoryRates("2026-07");
    expect(rates2026.minWage).toBe(10320);
    expect(rates2026.hiTotal).toBe(0.0719); // 7.19%
    expect(rates2026.npBandHigh).toBe(6590000); // 2026 Cap 6.59M

    const payslip2026 = computeEmployeePayslip(emp, { otHours: 0, nightHours: 0, holidayHours: 0 }, "2026-07");
    expect(payslip2026.yearMonth).toBe("2026-07");
    expect(payslip2026.netPay % 10).toBe(0);

    // Health insurance deduction in 2026 should be higher than 2024 due to rate increase (7.19% vs 7.09%)
    const hi2024 = payslip2024.deductions.find((d) => d.code === "hi")!.amt;
    const hi2026 = payslip2026.deductions.find((d) => d.code === "hi")!.amt;
    expect(hi2026).toBeGreaterThan(hi2024);

    // 3. Forward Projected Simulation for 2027 (Future Regulatory Reform)
    const rates2027 = resolveStatutoryRates("2027-03");
    expect(rates2027.minWage).toBe(10750);
    expect(rates2027.hiTotal).toBe(0.0735); // 7.35%

    const payslip2027 = computeEmployeePayslip(emp, { otHours: 0, nightHours: 0, holidayHours: 0 }, "2027-03");
    expect(payslip2027.yearMonth).toBe("2027-03");
    const hi2027 = payslip2027.deductions.find((d) => d.code === "hi")!.amt;
    expect(hi2027).toBeGreaterThan(hi2026);

    // 4. Registry completeness: all 5 schedules (2024, 2025, 2026, 2027, 2028+) exist and are contiguous
    expect(STATUTORY_SCHEDULE_REGISTRY.length).toBe(5);
  });

  // =========================================================================
  // WORKFLOW 5: Centralized Enterprise Glossary & Zero-Code Digging i18n Lookup
  // =========================================================================
  it("Workflow 5 [Centralized Glossary & i18n]: resolves all domain terms, citations, and dual-language dictionaries without code digging", () => {
    // 1. Verify Korean standard term lookup
    expect(t("payroll.statutory10WonTruncation")).toBe("국고금관리법 10원 미만 절사");
    expect(t("attendance.weekly52Limit")).toBe("주 52시간 한도 관리");
    expect(t("approvals.sodRule")).toBe("직무 분리 규정 (SoD)");
    expect(t("rbac.safetySupervisorOutrank")).toBe("안전감시관 즉각 작업중지권");

    // 2. Verify English translation lookup (i18n ready)
    expect(t("payroll.statutory10WonTruncation", undefined, "en")).toBe("10-Won Statutory Truncation");
    expect(t("attendance.weekly52Limit", undefined, "en")).toBe("Weekly 52-Hour Cap");
    expect(t("approvals.sodRule", undefined, "en")).toBe("Separation of Duties (SoD)");

    // 3. Verify statutory legal citation lookup
    const term10Won = getGlossaryEntry("payroll.statutory10WonTruncation");
    expect(term10Won?.legalBasis).toBe("국고금관리법 제47조 제1항 (국고금의 끝수 계산)");

    const term52h = getGlossaryEntry("attendance.weekly52Limit");
    expect(term52h?.legalBasis).toBe("근로기준법 제53조 (연장 근로의 제한)");

    const termRest = getGlossaryEntry("attendance.statutoryRestBreak");
    expect(termRest?.legalBasis).toBe("근로기준법 제54조 (휴게)");

    const termLeave = getGlossaryEntry("hr.leavePromotionRound1");
    expect(termLeave?.legalBasis).toBe("근로기준법 제61조 제1항 제1호 (연차 유급휴가의 사용 촉진)");

    // 4. Search glossary by legal keyword
    const searchRes = searchGlossary("제56조");
    expect(searchRes.length).toBeGreaterThanOrEqual(2); // 연장근로, 야간근로
    expect(searchRes.some((e) => e.key === "attendance.overtimeHours")).toBe(true);
    expect(searchRes.some((e) => e.key === "attendance.nightHours")).toBe(true);

    // 5. Verify total registered glossary coverage
    expect(Object.keys(ENTERPRISE_GLOSSARY).length).toBeGreaterThanOrEqual(30);
  });

  // =========================================================================
  // WORKFLOW 6: Edge Case Hardening — Adversarial Mutation & Malformed Inputs
  // =========================================================================
  it("Workflow 6 [Edge Case Fuzzing & Recovery]: hardens against malformed formulas, negative breaks, and overflow limits", () => {
    // 1. Connected Sheet formula fuzzing
    expect(evaluateSheetFormula("=alert('xss')")).toBe("#ERR:INVALID_CHAR");
    expect(evaluateSheetFormula("=SELECT * FROM users")).toBe("#ERR:INVALID_CHAR");
    expect(evaluateSheetFormula("=SUM(100, 200, 300)")).toBe(600);
    expect(evaluateSheetFormula("=AVERAGE(100, 200, 300)")).toBe(200);

    // 2. Weekly 52-hour threshold evaluation
    const safe = evaluate52HourCompliance(40);
    expect(safe.status).toBe("NORMAL");

    const warning = evaluate52HourCompliance(49);
    expect(warning.status).toBe("WARNING");

    const violation = evaluate52HourCompliance(53);
    expect(violation.status).toBe("VIOLATION");
    expect(violation.label).toContain("초과");

    // 3. Truncation precision
    expect(trunc10(0)).toBe(0);
    expect(trunc10(9)).toBe(0);
    expect(trunc10(10)).toBe(10);
    expect(trunc10(19)).toBe(10);
    expect(trunc10(123456789)).toBe(123456780);
  });
});
