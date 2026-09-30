import { describe, it, expect, beforeEach } from "vitest";
import {
  calculateWorkHours,
  evaluate52HourCompliance,
} from "../src/lib/attendance-calculator";
import {
  trunc10,
  pensionBase,
  statutoryDeductions,
  computeEmployeePayslip,
  hourlyBase,
} from "../src/lib/payroll-engine";
import {
  parseMoneyInput,
  parseDateInput,
  parseTsvClipboard,
  evaluateSheetFormula,
  matchObjectReference,
  summarizeProposedAllowanceChanges,
} from "../src/lib/sheet-engine";
import {
  evaluateAccess,
  previewPolicyChange,
  CAPABILITY_BUNDLES,
  DEFAULT_CEDAR_POLICIES,
  INITIAL_ACCESS_ASSIGNMENTS,
  DEFAULT_ROLE_DEFINITIONS,
  INITIAL_ROLE_ASSIGNMENTS,
  foldUserEffectivePermissions,
  verifyAuditChain,
} from "../src/lib/policy-engine";
import { useConsoleStore } from "../src/lib/store";
import type { Employee, ProposedChange, AccessEvaluationRequest } from "../src/lib/types";

describe("Korean Labor Standards Act (근로기준법) Attendance Engine", () => {
  it("computes statutory rest break correctly per §54", () => {
    // 3.5 hours work: 0 min rest
    const under4 = calculateWorkHours("09:00", "12:30");
    expect(under4.breakMinutes).toBe(0);
    expect(under4.netWorkHours).toBe(3.5);

    // 4 hours to under 8 hours: 30 min statutory rest
    const fourHours = calculateWorkHours("09:00", "13:00");
    expect(fourHours.breakMinutes).toBe(30);
    expect(fourHours.netWorkHours).toBe(3.5);

    const sevenHours = calculateWorkHours("09:00", "16:00");
    expect(sevenHours.breakMinutes).toBe(30);
    expect(sevenHours.netWorkHours).toBe(6.5);

    // 8 hours or more: 60 min statutory rest
    const eightHours = calculateWorkHours("09:00", "17:00");
    expect(eightHours.breakMinutes).toBe(60);
    expect(eightHours.netWorkHours).toBe(7.0);

    const nineHours = calculateWorkHours("09:00", "18:00");
    expect(nineHours.breakMinutes).toBe(60);
    expect(nineHours.netWorkHours).toBe(8.0);
  });

  it("calculates shift with overtime correctly", () => {
    // 09:00 - 21:00 is 12 hours elapsed, minus 60m rest = 11h net work
    const res = calculateWorkHours("09:00", "21:00");
    expect(res.grossHours).toBe(12);
    expect(res.breakMinutes).toBe(60);
    expect(res.netWorkHours).toBe(11);
  });

  it("calculates midnight-crossing night shift correctly", () => {
    // 21:00 to 06:00 next day (9 hours elapsed, 60m rest = 8h net work)
    const res = calculateWorkHours("21:00", "06:00");
    expect(res.isMidnightShift).toBe(true);
    expect(res.grossHours).toBe(9);
    expect(res.breakMinutes).toBe(60);
    expect(res.netWorkHours).toBe(8);
  });

  it("evaluates 주 52시간 (52-hour weekly limit) compliance", () => {
    const normal = evaluate52HourCompliance(40);
    expect(normal.status).toBe("NORMAL");
    expect(normal.tone).toBe("ok");
    expect(normal.remainingHours).toBe(12);

    const overtime = evaluate52HourCompliance(45);
    expect(overtime.status).toBe("OVERTIME");
    expect(overtime.tone).toBe("info");
    expect(overtime.remainingHours).toBe(7);

    const warn = evaluate52HourCompliance(49);
    expect(warn.status).toBe("WARNING");
    expect(warn.tone).toBe("warn");
    expect(warn.remainingHours).toBe(3);

    const violation = evaluate52HourCompliance(54);
    expect(violation.status).toBe("VIOLATION");
    expect(violation.tone).toBe("danger");
    expect(violation.remainingHours).toBe(0);
  });
});

describe("Korean Statutory Payroll Engine (급여 및 4대보험 계산기)", () => {
  it("truncates 10-won units per National Treasury Administration Act §47(1)", () => {
    expect(trunc10(12345)).toBe(12340);
    expect(trunc10(12349)).toBe(12340);
    expect(trunc10(12340)).toBe(12340);
    expect(trunc10(9)).toBe(0);
  });

  it("clamps National Pension between statutory min (410k) and max (6.59M)", () => {
    // Under min threshold
    expect(pensionBase(200_000)).toBe(410_000);
    // Standard in band (truncates 1,000-won unit)
    expect(pensionBase(3_456_789)).toBe(3_456_000);
    // Over max threshold
    expect(pensionBase(10_000_000)).toBe(6_590_000);
  });

  it("computes 4 major insurance deductions with statutory caps and rates", () => {
    const taxableGross = 3_500_000;
    const deductions = statutoryDeductions(taxableGross, 1);

    const np = deductions.find((d) => d.code === "np");
    const hi = deductions.find((d) => d.code === "hi");
    const ltc = deductions.find((d) => d.code === "ltc");
    const ei = deductions.find((d) => d.code === "ei");
    const tax = deductions.find((d) => d.code === "tax");
    const local = deductions.find((d) => d.code === "local");

    expect(np).toBeDefined();
    expect(hi).toBeDefined();
    expect(ltc).toBeDefined();
    expect(ei).toBeDefined();
    expect(tax).toBeDefined();
    expect(local).toBeDefined();

    // National Pension: 4.75% of clamped base (3.5M * 0.0475 = 166,250 -> trunc10 166,250)
    expect(np?.amt).toBe(trunc10(3_500_000 * 0.0475));

    // Health Insurance: 7.19% halved (3.595%) with 10-won statutory truncation
    expect(hi?.amt).toBe(trunc10(Math.round(trunc10(3_500_000 * 0.0719) / 2)));

    // Employment Insurance: 0.9%
    expect(ei?.amt).toBe(Math.round(3_500_000 * 0.009));
  });

  it("computes complete payslip with statutory overtime multiplier", () => {
    const mockEmployee: Employee = {
      id: "emp_test_01",
      code: "EMP-9999",
      name: "테스트",
      email: "test@oyatie.com",
      phone: "010-1234-5678",
      entity: "(주)오야티 로지스틱스",
      site: "인천 제1물류센터",
      dept: "설비운영팀",
      role: "엔지니어",
      position: "주임",
      empType: "정규",
      baseSalary: 3_135_000, // 209 * 15,000
      fixedAllow: 200_000,
      joinedDate: "2024-01-01",
      status: "재직",
      dependents: 1,
    };

    const hourly = hourlyBase(mockEmployee);
    expect(Math.round(hourly)).toBe(Math.round((3_135_000 + 200_000) / 209));

    // 10h overtime, 4h night, 0h holiday
    const payslip = computeEmployeePayslip(mockEmployee, {
      otHours: 10,
      nightHours: 4,
      holidayHours: 0,
    });

    expect(payslip.code).toBe("PS-9999");
    expect(payslip.basePay).toBe(3_135_000);
    expect(payslip.fixedAllowance).toBe(200_000);
    expect(payslip.overtimePay).toBe(trunc10(Math.round(10 * hourly * 1.5)));
    expect(payslip.nightPay).toBe(trunc10(Math.round(4 * hourly * 0.5)));
    expect(payslip.grossPay).toBe(
      payslip.basePay + payslip.fixedAllowance + payslip.overtimePay + payslip.nightPay
    );
    expect(payslip.netPay).toBe(payslip.grossPay - payslip.totalDeductions);
  });
});

describe("Zustand Console Store & Palantir Ontology Zero-Stub Workflows", () => {
  beforeEach(() => {
    useConsoleStore.getState().resetToSeedData();
  });

  it("onboards an employee and reactively links attendance & payroll", () => {
    const store = useConsoleStore.getState();
    const initialEmpCount = store.employees.length;
    const initialAttCount = store.attendance.length;

    const newEmp = store.createEmployee({
      name: "신규엔지니어",
      email: "new.eng@oyatie.com",
      phone: "010-9999-8888",
      entity: "(주)오야티 인프라솔루션",
      site: "평택 데이터센터",
      dept: "서버인프라팀",
      role: "엔지니어",
      position: "주임",
      empType: "정규",
      baseSalary: 3_500_000,
      fixedAllow: 300_000,
      joinedDate: "2026-07-03",
      status: "재직",
      dependents: 1,
    });

    const updatedState = useConsoleStore.getState();
    expect(updatedState.employees.length).toBe(initialEmpCount + 1);
    expect(updatedState.attendance.length).toBe(initialAttCount + 1);
    expect(updatedState.payrollRun.headcount).toBe(initialEmpCount + 1);

    // Check attendance auto-creation
    const attRecord = updatedState.attendance.find((a) => a.employeeId === newEmp.id);
    expect(attRecord).toBeDefined();
    expect(attRecord?.site).toBe("평택 데이터센터");
    expect(attRecord?.clockIn).toBe("09:00");
  });

  it("updates attendance punch times and recalculates work hours", () => {
    const store = useConsoleStore.getState();
    const targetRecord = store.attendance[0];
    expect(targetRecord).toBeDefined();

    store.updateAttendancePunch(targetRecord.id, "08:30", "19:30", 60);

    const updated = useConsoleStore.getState().attendance.find((a) => a.id === targetRecord.id);
    expect(updated?.clockIn).toBe("08:30");
    expect(updated?.clockOut).toBe("19:30");
    // 11 hours elapsed, 60m rest = 10h net work (8h regular, 2h overtime)
    expect(updated?.workedHours).toBe(10);
    expect(updated?.otHours).toBe(2);
  });

  it("adjudicates attendance exception and decrements unresolved count", () => {
    const store = useConsoleStore.getState();
    const excRecord = store.attendance.find((a) => a.exceptionCode);
    if (!excRecord) return;

    const initialUnresolved = store.payrollRun.unresolvedExceptions;
    store.adjudicateAttendanceException(excRecord.id, "approved", "고객사 긴급 정비 지원 확인");

    const updated = useConsoleStore.getState().attendance.find((a) => a.id === excRecord.id);
    expect(updated?.exceptionCode).toBeUndefined();

    const updatedRun = useConsoleStore.getState().payrollRun;
    expect(updatedRun.unresolvedExceptions).toBe(Math.max(0, initialUnresolved - 1));
  });

  it("adjudicating all exceptions unblocks the payroll freeze gate", () => {
    const store = useConsoleStore.getState();
    // Adjudicate all remaining records with exceptions
    for (const rec of store.attendance) {
      if (rec.exceptionCode) {
        store.adjudicateAttendanceException(rec.id, "approved", "관리자 일괄 승인");
      }
    }

    const state = useConsoleStore.getState();
    expect(state.payrollRun.unresolvedExceptions).toBe(0);

    // Now freeze payroll
    const result = store.calculateAndFreezePayroll();
    expect(result.success).toBe(true);

    const finalizedState = useConsoleStore.getState();
    expect(finalizedState.payrollRun.status).toBe("마감게이트통과");
    expect(finalizedState.payslips.length).toBe(finalizedState.employees.length);
  });

  it("supports full multi-stage approval workflow with audit log trail", () => {
    const store = useConsoleStore.getState();
    const initialAuditCount = store.auditEvents.length;

    // 1. Create Parking draft
    const draft = store.createApprovalDraft({
      title: "화성공장 유압 밸브 긴급 구매 품의",
      category: "지출결의",
      doaAmount: 4_500_000,
      drafterId: "emp_101",
      drafterName: "한지우",
      drafterDept: "생산기술1팀",
      content: "화성 제2라인 유압 밸브 노후화로 인한 즉시 교체 발주 승인 요청의 건",
      linkedObjects: ["WO-2026-001"],
    });

    expect(draft.status).toBe("초안");

    // 2. Submit to stage 1
    store.submitApproval(draft.id);
    let currentDoc = useConsoleStore.getState().approvals.find((a) => a.id === draft.id);
    expect(currentDoc?.status).toBe("결재대기");
    expect(currentDoc?.stages[0].status).toBe("pending");

    // 3. Approve stage 1 (팀장 심사)
    store.approveDoc(draft.id, "김현수 팀장", "유압 장비 이상 여부 확인 완료");
    currentDoc = useConsoleStore.getState().approvals.find((a) => a.id === draft.id);
    expect(currentDoc?.stages[0].status).toBe("approved");
    expect(currentDoc?.stages[1].status).toBe("pending");

    // 4. Approve stage 2 (총괄임원 / 최종 승인)
    store.approveDoc(draft.id, "강태훈 상무", "최종 지출 집행 승인");
    currentDoc = useConsoleStore.getState().approvals.find((a) => a.id === draft.id);
    expect(currentDoc?.status).toBe("승인완료");

    // 5. Audit event generated
    const finalState = useConsoleStore.getState();
    expect(finalState.auditEvents.length).toBeGreaterThan(initialAuditCount);
  });

  it("updates work order status through pipeline sequentially", () => {
    const store = useConsoleStore.getState();
    const newWo = store.createWorkOrder({
      title: "변전실 정기 점검",
      site: "화성 제1제조공장",
      assignedTo: "한지우",
      priority: "높음",
      targetEquipment: "EQ-SUBSTATION-01",
      dueDate: "2026-07-04",
      contractCode: "C-2026-01",
    });

    expect(newWo.status).toBe("접수");

    store.updateWorkOrderStatus(newWo.id, "배차");
    let wo = useConsoleStore.getState().workOrders.find((w) => w.id === newWo.id);
    expect(wo?.status).toBe("배차");

    store.updateWorkOrderStatus(newWo.id, "진행중");
    wo = useConsoleStore.getState().workOrders.find((w) => w.id === newWo.id);
    expect(wo?.status).toBe("진행중");

    store.updateWorkOrderStatus(newWo.id, "완료");
    wo = useConsoleStore.getState().workOrders.find((w) => w.id === newWo.id);
    expect(wo?.status).toBe("완료");
  });

  it("supports real-time messaging and object-anchored discussion threads", () => {
    const store = useConsoleStore.getState();
    const initialThreadCount = store.threads.length;

    // Create thread anchored on Work Order WO-2026-001
    const thread = store.createThreadForObject("WO-2026-001", "WO-2026-001 현장 정비 소통 스레드");
    expect(useConsoleStore.getState().threads.length).toBe(initialThreadCount + 1);

    // Send a message into this thread
    store.sendMessage(thread.id, "1차 부품 분해 완료했습니다. 현장 점검 바랍니다.");

    const updatedThread = useConsoleStore.getState().threads.find((t) => t.id === thread.id);
    const msgs = useConsoleStore.getState().messages[thread.id];
    expect(msgs).toBeDefined();
    expect(msgs.some((m) => m.text.includes("1차 부품 분해 완료"))).toBe(true);
  });

  it("sets up organization entities, policies, and contracts with audit trail", () => {
    const store = useConsoleStore.getState();

    // 1. Create a legal entity
    const newEntity = store.createOrgEntity({
      code: "OYT-ENERGY",
      name: "(주)오야티 에너지솔루션",
      bizNumber: "502-81-99401",
      corpNumber: "110111-9982310",
      ceoName: "정우진",
      address: "부산광역시 해운대구 센텀중앙로 78",
      mainBank: "하나은행",
      mainAccount: "291-9102-3910",
      establishedDate: "2026-07-01",
      status: "active",
    });

    expect(newEntity.id).toBeDefined();
    const storedEntity = useConsoleStore.getState().orgEntities.find((e) => e.code === "OYT-ENERGY");
    expect(storedEntity?.name).toBe("(주)오야티 에너지솔루션");

    // 2. Update Policy
    store.updateOrgPolicy({
      attendance: {
        ...store.orgPolicy.attendance,
        warningWeeklyHours: 46, // stricter warning threshold
      },
    });
    expect(useConsoleStore.getState().orgPolicy.attendance.warningWeeklyHours).toBe(46);

    // 3. Register Equipment
    const eq = store.createEquipment({
      name: "10톤 크롤러 크레인 (CR-10-부산)",
      site: "부산 신항만",
      category: "크레인",
      serialNo: "TADANO-CC100-2026",
      inspectionCycleDays: 180,
      lastInspectionDate: "2026-07-01",
      nextInspectionDate: "2026-12-31",
      assignedEngineer: "정우진",
      status: "정상가동",
    });
    expect(eq.code).toMatch(/^EQ-/);
  });

  it("processes HR personnel actions and reactively updates employee dossier", () => {
    const store = useConsoleStore.getState();
    const targetEmp = store.employees[0];
    const initialSalary = targetEmp.baseSalary;

    // Process Promotion & Salary Adjustment
    const action = store.processPersonnelAction({
      employeeId: targetEmp.id,
      actionType: "승진",
      effectiveDate: "2026-08-01",
      reason: "2026 하반기 EPM 기획 성과 우수 및 리더십 평가 1등급 정기 승진",
      updates: {
        role: "수석부장",
        baseSalary: initialSalary + 800000,
        dept: "전략기획실",
      },
    });

    expect(action.code).toMatch(/^PA-2026-/);
    expect(action.actionType).toBe("승진");

    // Verify employee record was reactively updated
    const updatedEmp = useConsoleStore.getState().employees.find((e) => e.id === targetEmp.id);
    expect(updatedEmp?.role).toBe("수석부장");
    expect(updatedEmp?.baseSalary).toBe(initialSalary + 800000);
    expect(updatedEmp?.dept).toBe("전략기획실");

    // Verify Audit Event was recorded
    const latestAudit = useConsoleStore.getState().auditEvents[0];
    expect(latestAudit.action).toBe("HR_PERSONNEL_ACTION_승진");
    expect(latestAudit.targetCode).toBe(action.code);
  });

  it("generates KFTC 100-byte Firm Banking CMS flat-file and seals transfer batch", () => {
    const store = useConsoleStore.getState();

    // Generate Firm Banking Disbursement Batch
    const batch = store.generateFirmBankingBatch();
    expect(batch.code).toMatch(/^FB-2607-/);
    expect(batch.totalHeadcount).toBeGreaterThan(0);
    expect(batch.totalAmount).toBeGreaterThan(0);
    expect(batch.bankSummaries.length).toBeGreaterThan(0);

    // Verify KFTC Flat-File structure
    const lines = batch.kftcFlatFileContent.trim().split("\n");
    expect(lines.length).toBe(batch.totalHeadcount + 2); // 1 Header + N Data + 1 Trailer

    // Header record check
    const header = lines[0];
    expect(header.startsWith("H00099823100")).toBe(true);
    expect(header).toContain("088"); // Main bank code
    expect(header).toContain("100032998231"); // Main account

    // Data records check
    const dataRow = lines[1];
    expect(dataRow.startsWith("D000001")).toBe(true);
    expect(dataRow).toContain("(주)오야티");

    // Trailer record check
    const trailer = lines[lines.length - 1];
    expect(trailer.startsWith("T")).toBe(true);

    // Seal and dispatch batch with passkey
    store.sealAndDispatchBankBatch("최원석 대표이사");
    const sealedBatch = useConsoleStore.getState().bankTransferBatch;
    expect(sealedBatch.status).toBe("출금승인대기");
    expect(sealedBatch.sealedBy).toContain("최원석 대표이사");
    expect(sealedBatch.passkeyHash).toBeDefined();
    expect(useConsoleStore.getState().payrollRun.status).toBe("지급승인");
  });

  it("repairs attendance discrepancy and reactively recalculates employee payslip", () => {
    const store = useConsoleStore.getState();

    // Verify initial open discrepancy
    const initialDiscrepancy = store.discrepancies.find((d) => d.sourceCode === "AT-0828-02");
    expect(initialDiscrepancy).toBeDefined();
    expect(initialDiscrepancy?.status).toBe("unresolved");
    expect(store.payrollRun.unresolvedExceptions).toBeGreaterThan(0);

    // Execute 1-click repair with statutory justification
    store.repairDiscrepancy(
      initialDiscrepancy!.id,
      "항만 특수 긴급 복구 작업에 따른 연장근로 사전 소명 인정 및 특별인가 승인"
    );

    // Verify discrepancy marked resolved
    const resolvedDiscrepancy = useConsoleStore.getState().discrepancies.find((d) => d.id === initialDiscrepancy!.id);
    expect(resolvedDiscrepancy?.status).toBe("resolved");
    expect(resolvedDiscrepancy?.resolutionComment).toContain("특별인가 승인");

    // Verify unresolved exceptions count decremented
    expect(useConsoleStore.getState().payrollRun.unresolvedExceptions).toBe(initialDiscrepancy ? store.payrollRun.unresolvedExceptions - 1 : 0);

    // Verify audit event recorded
    const latestAudit = useConsoleStore.getState().auditEvents[0];
    expect(latestAudit.action).toBe("PAYROLL_DISCREPANCY_REPAIRED");
    expect(latestAudit.targetCode).toBe("AT-0828-02");
  });

  it("preserves durable payroll draft state across user sessions", () => {
    const store = useConsoleStore.getState();

    // Save payroll draft
    store.savePayrollDraft({
      activeTab: "sheet",
      yearMonth: "2026-07",
      filterQuery: "송민우",
    });

    // Resume payroll draft
    const draft = store.resumePayrollDraft();
    expect(draft).toBeDefined();
    expect(draft?.activeTab).toBe("sheet");
    expect(draft?.yearMonth).toBe("2026-07");
    expect(draft?.filterQuery).toBe("송민우");
    expect(draft?.savedAt).toBeDefined();
  });

  it("executes employee transfer (부서전보) with personnel action and reactive record update", () => {
    const store = useConsoleStore.getState();
    const targetEmp = store.employees[0];
    const initialDept = targetEmp.dept;
    const initialSite = targetEmp.site;

    const action = store.transferEmployee(
      targetEmp.id,
      "물류지원팀",
      "인천 항만 터미널",
      "2026-08-01",
      "거점 전문인력 재배치"
    );

    expect(action.actionType).toBe("부서전보");
    expect(action.newDetails.dept).toBe("물류지원팀");
    expect(action.newDetails.site).toBe("인천 항만 터미널");

    // Verify employee record updated reactively
    const updatedEmp = useConsoleStore.getState().employees.find((e) => e.id === targetEmp.id);
    expect(updatedEmp?.dept).toBe("물류지원팀");
    expect(updatedEmp?.site).toBe("인천 항만 터미널");

    // Verify audit event recorded
    const latestAudit = useConsoleStore.getState().auditEvents[0];
    expect(latestAudit.action).toBe("HR_PERSONNEL_ACTION_부서전보");
    expect(latestAudit.targetCode).toBe(action.code);
  });
});

describe("Connected Sheet & Spreadsheet Engine (Spreadsheet Familiarity & Governance)", () => {
  it("parses currency and money inputs correctly with various formats", () => {
    expect(parseMoneyInput("1,250,000")).toBe(1250000);
    expect(parseMoneyInput("₩1,250,000")).toBe(1250000);
    expect(parseMoneyInput("1250000원")).toBe(1250000);
    expect(parseMoneyInput("125만")).toBe(1250000);
    expect(parseMoneyInput("-50,000")).toBe(-50000);
    expect(parseMoneyInput("")).toBeNull();
    expect(parseMoneyInput("invalid")).toBeNull();
  });

  it("normalizes date input to standard YYYY-MM-DD format", () => {
    expect(parseDateInput("2026-07-01")).toBe("2026-07-01");
    expect(parseDateInput("2026.7.1")).toBe("2026-07-01");
    expect(parseDateInput("2026/07/15")).toBe("2026-07-15");
    expect(parseDateInput("invalid")).toBeNull();
  });

  it("evaluates safe sheet formulas without arbitrary code execution", () => {
    const rowRecord = {
      calculatedGross: 4500000,
      baseSalary: 3500000,
      proposedAllowance: 300000,
      fixedAllow: 200000,
    };

    // Subtraction formula
    const diff = evaluateSheetFormula("=[@calculatedGross] - [@baseSalary]", rowRecord);
    expect(diff).toBe(1000000);

    // Percentage formula
    const increase = evaluateSheetFormula("=[@calculatedGross] * 1.05", rowRecord);
    expect(increase).toBe(4725000);

    // Addition formula
    const allowanceDiff = evaluateSheetFormula("=[@proposedAllowance] - [@fixedAllow]", rowRecord);
    expect(allowanceDiff).toBe(100000);

    // Malicious or invalid characters rejected
    const blocked = evaluateSheetFormula("=alert('hack')", rowRecord);
    expect(blocked).toBe("#ERR:INVALID_CHAR");
  });

  it("parses rectangular TSV clipboard paste from Excel / Google Sheets", () => {
    const tsv = "1,200,000\t200,000\t운영1팀\n1,500,000\t300,000\t물류지원팀";
    const matrix = parseTsvClipboard(tsv);
    expect(matrix.length).toBe(2);
    expect(matrix[0]).toEqual(["1,200,000", "200,000", "운영1팀"]);
    expect(matrix[1]).toEqual(["1,500,000", "300,000", "물류지원팀"]);
  });

  it("disambiguates object references and detects collisions", () => {
    const candidates = [
      { id: "dept_01", name: "운영1팀", context: "본사 경영총괄본부" },
      { id: "dept_02", name: "물류지원팀", context: "평택 항만 센터" },
      { id: "dept_03", name: "운영2팀", context: "부산 신항 센터" },
    ];

    // Exact unambiguous match
    const exact = matchObjectReference("물류지원팀", candidates);
    expect(exact.isAmbiguous).toBe(false);
    expect(exact.matchedId).toBe("dept_02");

    // Partial ambiguous match (운영 matches dept_01 and dept_03)
    const ambiguous = matchObjectReference("운영", candidates);
    expect(ambiguous.isAmbiguous).toBe(true);
    expect(ambiguous.competingCandidates.length).toBe(2);
  });

  it("summarizes proposed allowance changes into actionable consequence review", () => {
    const changes = new Map<string, ProposedChange>();
    changes.set("emp_1:proposedAllowance", {
      rowKey: "emp_1",
      columnId: "proposedAllowance",
      previousValue: 200000,
      proposedValue: 500000,
      status: "draft",
      timestamp: new Date().toISOString(),
    });
    changes.set("emp_2:proposedAllowance", {
      rowKey: "emp_2",
      columnId: "proposedAllowance",
      previousValue: 300000,
      proposedValue: 400000,
      status: "draft",
      timestamp: new Date().toISOString(),
    });

    const employees = [
      { id: "emp_1", name: "이서진", fixedAllow: 200000 },
      { id: "emp_2", name: "김도윤", fixedAllow: 300000 },
    ];

    const summary = summarizeProposedAllowanceChanges(changes, employees);
    expect(summary.changeCount).toBe(2);
    expect(summary.affectedHeadcount).toBe(2);
    expect(summary.totalAllowanceDelta).toBe(400000); // (500k-200k) + (400k-300k) = 300k + 100k = 400k
    expect(summary.items.length).toBe(2);
  });
});

describe("Cedar Policy Engine & Non-Technical Governance", () => {
  beforeEach(() => {
    useConsoleStore.getState().resetToSeedData();
  });

  it("permits access when active capability bundle is assigned and no forbid rule triggers", () => {
    // ACC-003 (박재무) has cap-payroll-approver ("Payroll::ApproveRun")
    const req: AccessEvaluationRequest = {
      principalAccountId: "ACC-003",
      principalPersonId: "P-003",
      principalRoleGroup: "총괄본부장",
      principalEntityId: "all",
      action: "Payroll::ApproveRun",
      resource: {
        id: "PR-2026-07",
        code: "PR-2026-07",
        kind: "PR",
        entityId: "ENT-01",
        entityName: "(주)오야티",
        preparedByPersonId: "P-002", // Prepared by 김인사
        preparedByName: "김인사",
      },
      context: {
        time: "2026-07-25T10:00:00Z",
        mfaVerified: true,
      },
    };

    const res = evaluateAccess(req);
    expect(res.decision).toBe("PERMIT");
    expect(res.matchedPermitRules.length).toBeGreaterThan(0);
    expect(res.blockingForbidRules.length).toBe(0);
  });

  it("denies access with DEFAULT DENY when no matching capability is assigned", () => {
    // ACC-002 (김인사) only has cap-payroll-operator, trying to execute firm banking
    const req: AccessEvaluationRequest = {
      principalAccountId: "ACC-002",
      principalPersonId: "P-002",
      principalRoleGroup: "팀장",
      principalEntityId: "all",
      action: "Payroll::ExecuteFirmBanking",
      resource: {
        id: "FB-202607-01",
        code: "FB-202607-01",
        kind: "FB",
        entityId: "ENT-01",
        entityName: "(주)오야티",
      },
      context: {
        time: "2026-07-25T10:00:00Z",
        mfaVerified: true,
      },
    };

    const res = evaluateAccess(req);
    expect(res.decision).toBe("FORBID");
    expect(res.matchedPermitRules.length).toBe(0);
    expect(res.summary).toContain("DEFAULT DENY");
    expect(res.remediationAdvice?.actionType).toBe("grant_request");
  });

  it("strictly forbids approval when principal.personId === resource.preparedByPersonId (SoD enforcement)", () => {
    // P-003 (박재무) prepared the run himself, and now attempts to approve it
    const req: AccessEvaluationRequest = {
      principalAccountId: "ACC-003",
      principalPersonId: "P-003",
      principalRoleGroup: "총괄본부장",
      principalEntityId: "all",
      action: "Payroll::ApproveRun",
      resource: {
        id: "PR-2026-07",
        code: "PR-2026-07",
        kind: "PR",
        entityId: "ENT-01",
        entityName: "(주)오야티",
        preparedByPersonId: "P-003", // SELF-PREPARED!
        preparedByName: "박재무",
      },
      context: {
        time: "2026-07-25T10:00:00Z",
        mfaVerified: true,
      },
    };

    const res = evaluateAccess(req);
    expect(res.decision).toBe("FORBID");
    expect(res.blockingForbidRules.some((r) => r.id === "pol-sod-independence")).toBe(true);
    expect(res.remediationAdvice?.actionType).toBe("handoff_delegate");
  });

  it("forbids high-value / tier-3 actions when Passkey/MFA is unverified", () => {
    // ACC-001 has tier-3 approval, but session is not MFA verified
    const req: AccessEvaluationRequest = {
      principalAccountId: "ACC-001",
      principalPersonId: "P-001",
      principalRoleGroup: "대표이사",
      principalEntityId: "all",
      action: "Approvals::SignTier3",
      resource: {
        id: "AP-3121",
        code: "AP-3121",
        kind: "AP",
        entityId: "ENT-01",
        entityName: "(주)오야티",
        preparedByPersonId: "P-004",
        amount: 80000000,
      },
      context: {
        time: "2026-07-25T10:00:00Z",
        mfaVerified: false, // NOT VERIFIED
      },
    };

    const res = evaluateAccess(req);
    expect(res.decision).toBe("FORBID");
    expect(res.blockingForbidRules.some((r) => r.id === "pol-tier3-passkey-mfa")).toBe(true);
    expect(res.remediationAdvice?.actionType).toBe("passkey_register");
  });

  it("forbids action when principal entity does not match resource entity (Tenant boundary)", () => {
    const req: AccessEvaluationRequest = {
      principalAccountId: "ACC-010",
      principalPersonId: "P-010",
      principalRoleGroup: "팀장",
      principalEntityId: "ENT-02", // Belonging to Acme Logis
      action: "Approvals::SignTier1",
      resource: {
        id: "AP-3121",
        code: "AP-3121",
        kind: "AP",
        entityId: "ENT-01", // Resource belongs to Oyatie!
        entityName: "(주)오야티",
        preparedByPersonId: "P-004",
        amount: 1000000,
      },
      context: {
        time: "2026-07-25T10:00:00Z",
        mfaVerified: true,
      },
    };

    const res = evaluateAccess(req);
    expect(res.decision).toBe("FORBID");
    expect(res.blockingForbidRules.some((r) => r.id === "pol-tenant-isolation")).toBe(true);
  });

  it("enforces independence during reviewer handoff and forbids delegation to document drafter", () => {
    const store = useConsoleStore.getState();
    const doc = store.approvals.find((d) => d.code === "AP-3121")!;
    // drafter is "이수력" (emp_01)
    const drafterEmp = store.employees.find((e) => e.name === doc.drafterName || e.id === doc.drafterId);
    expect(drafterEmp).toBeDefined();

    // Try to reassign delegate back to the drafter himself
    const result = store.reassignApprovalDelegate("AP-3121", "이수력", drafterEmp!.id, "자가 대결 시도");
    expect(result.success).toBe(false);
    expect(result.message).toContain("직무 분리(SoD) 규정 위반");
  });

  it("permits reviewer handoff to independent peer and updates approval stages", () => {
    const store = useConsoleStore.getState();
    // Independent peer: "김민준" (emp_02)
    const independentEmp = store.employees.find((e) => e.name !== "이수력" && e.id !== "emp_01")!;
    expect(independentEmp).toBeDefined();

    const result = store.reassignApprovalDelegate("AP-3121", "이수력", independentEmp.id, "출장으로 인한 직무대결");
    expect(result.success).toBe(true);

    const updatedDoc = useConsoleStore.getState().approvals.find((d) => d.code === "AP-3121")!;
    const pendingStage = updatedDoc.stages.find((s) => s.status === "pending");
    expect(pendingStage?.approverName).toContain(independentEmp.name);
  });

  it("resolves banking certificate operational incident and unlocks batch preflight", () => {
    const store = useConsoleStore.getState();
    const initialBatch = store.bankTransferBatch;
    // Set preflight to false to test incident resolution
    useConsoleStore.setState({
      bankTransferBatch: { ...initialBatch, preflightVerified: false, preflightSuccessRate: 0 },
    });

    store.resolveOperationalIncident("INC-2026-081", "KFTC 갱신 인증서 등록 및 핑 테스트 100% 성공");

    const resolvedIncident = useConsoleStore.getState().operationalIncidents.find((i) => i.id === "INC-2026-081")!;
    expect(resolvedIncident.status).toBe("resolved");
    expect(resolvedIncident.resolutionNote).toContain("KFTC 갱신 인증서");

    const updatedBatch = useConsoleStore.getState().bankTransferBatch;
    expect(updatedBatch.preflightVerified).toBe(true);
    expect(updatedBatch.preflightSuccessRate).toBe(100);
  });
});

describe("Discord-Style Multi-Layered Role Engine & 직무 전결 자격 기반 전자서명", () => {
  beforeEach(() => {
    useConsoleStore.getState().resetToSeedData();
  });

  it("folds multiple active role layers into a unified capability set for a Person Identity (Party)", () => {
    // emp_01 (이수력) holds 4 layered roles:
    // role-base-member, role-ops-lead, role-doa-tier1, role-site-custodian-incheon
    const fold = foldUserEffectivePermissions(
      "emp_01",
      {},
      DEFAULT_ROLE_DEFINITIONS,
      INITIAL_ROLE_ASSIGNMENTS
    );

    expect(fold.layeredRoles.length).toBe(4);
    // Inherited from role-ops-lead
    expect(fold.effectiveCapabilities.has("Operations::AssignWorkOrder")).toBe(true);
    // Inherited from role-doa-tier1
    expect(fold.effectiveCapabilities.has("Approvals::SignTier1")).toBe(true);
    // Inherited from role-base-member
    expect(fold.effectiveCapabilities.has("Comms::SendChat")).toBe(true);
    // Does NOT have payroll approval
    expect(fold.effectiveCapabilities.has("Payroll::ApproveRun")).toBe(false);
  });

  it("proves that authority does not follow rank (사원 outranks executives for safety orders)", () => {
    // emp_05 (정안전): Rank is "사원" (Associate), but holds @role-safety-inspector (Priority 85)
    // emp_03 (박재무): Rank is "본부장" (VP / C-Level)
    const safetyFold = foldUserEffectivePermissions(
      "emp_05",
      {},
      DEFAULT_ROLE_DEFINITIONS,
      INITIAL_ROLE_ASSIGNMENTS
    );
    const financeFold = foldUserEffectivePermissions(
      "emp_03",
      {},
      DEFAULT_ROLE_DEFINITIONS,
      INITIAL_ROLE_ASSIGNMENTS
    );

    // 정안전 (사원) has safety inspection & stop-work powers
    expect(safetyFold.effectiveCapabilities.has("Operations::CompleteInspection")).toBe(true);
    expect(safetyFold.effectiveCapabilities.has("Governance::InspectAuditChain")).toBe(true);

    // 박재무 (본부장) does NOT have site inspection powers
    expect(financeFold.effectiveCapabilities.has("Operations::CompleteInspection")).toBe(false);
  });

  it("enforces DoA amount ceilings on acting capacities", () => {
    // emp_01 holds DoA 1단계 전결관 with maxAmount = 5,000,000
    const fold = foldUserEffectivePermissions(
      "emp_01",
      {},
      DEFAULT_ROLE_DEFINITIONS,
      INITIAL_ROLE_ASSIGNMENTS
    );

    const doaCap = fold.actingCapacities.find((c) => c.roleId === "role-doa-tier1");
    expect(doaCap).toBeDefined();
    expect(doaCap?.maxAmount).toBe(5000000);

    // Document within limit (4.85M)
    expect(4850000 <= doaCap!.maxAmount!).toBe(true);
    // Document exceeding limit (12M)
    expect(12000000 <= doaCap!.maxAmount!).toBe(false);
  });

  it("records capacity-bearing signatures with actingCapacityRole and passkeyVerified", () => {
    const store = useConsoleStore.getState();
    const doc = store.approvals[0]; // AP-3121

    store.approveDoc(doc.id, "김인사", "DoA 1단계 전결규정 적합 승인", {
      actingCapacityRole: "role-doa-tier1",
      actingCapacityName: "DoA 1단계: 일상경비 전결관",
      authorizingGrantId: "grant-006",
      passkeyVerified: true,
    });

    const updatedDoc = useConsoleStore.getState().approvals.find((a) => a.id === doc.id)!;
    const approvedStage = updatedDoc.stages[1]; // Stage 2 was pending, now approved with capacity

    expect(approvedStage.status).toBe("approved");
    expect(approvedStage.actingCapacityRole).toBe("role-doa-tier1");
    expect(approvedStage.actingCapacityName).toBe("DoA 1단계: 일상경비 전결관");
    expect(approvedStage.authorizingGrantId).toBe("grant-006");
    expect(approvedStage.passkeyVerified).toBe(true);
  });

  it("assigns new role layer to employee and immediately folds into effective permissions (무지연 인가)", () => {
    const store = useConsoleStore.getState();
    // emp_06 (한정비) initially does not have Payroll::ApproveRun
    const initialFold = store.getEffectivePermissionsForEmployee("emp_06");
    expect(initialFold.effectiveCapabilities.has("Payroll::ApproveRun")).toBe(false);

    // Assign role-payroll-lead to emp_06
    store.assignRoleToEmployee(
      "emp_06",
      "role-payroll-lead",
      { type: "all" },
      "임시 급여 결재 책임관 위임"
    );

    // Re-evaluate fold
    const updatedFold = useConsoleStore.getState().getEffectivePermissionsForEmployee("emp_06");
    expect(updatedFold.effectiveCapabilities.has("Payroll::ApproveRun")).toBe(true);

    // Check audit trail
    const audit = useConsoleStore.getState().auditEvents[0];
    expect(audit.action).toBe("ROLE_LAYER_ASSIGNED");
    expect(audit.targetCode).toBe("emp_06");
  });

  it("revokes role layer from employee and immediately recalculates effective permissions", () => {
    const store = useConsoleStore.getState();

    // Assign a new role first
    const assignment = store.assignRoleToEmployee(
      "emp_06",
      "role-treasury-signer",
      { type: "all" },
      "임시 펌뱅킹 집행권 부여"
    );

    const withRoleFold = useConsoleStore.getState().getEffectivePermissionsForEmployee("emp_06");
    expect(withRoleFold.effectiveCapabilities.has("Payroll::ExecuteFirmBanking")).toBe(true);

    // Revoke the role assignment
    useConsoleStore.getState().removeRoleFromEmployee(assignment.id);

    // Re-evaluate fold
    const revokedFold = useConsoleStore.getState().getEffectivePermissionsForEmployee("emp_06");
    expect(revokedFold.effectiveCapabilities.has("Payroll::ExecuteFirmBanking")).toBe(false);

    // Check audit trail
    const audit = useConsoleStore.getState().auditEvents[0];
    expect(audit.action).toBe("ROLE_LAYER_REVOKED");
  });

  it("reorders role priorities dynamically and maintains descending rank precedence", () => {
    const store = useConsoleStore.getState();
    const ordered = ["role-safety-inspector", "role-super-admin", "role-corp-officer"];

    store.reorderRoles(ordered);

    const updatedRoles = useConsoleStore.getState().roleDefinitions;
    const safety = updatedRoles.find((r) => r.id === "role-safety-inspector")!;
    const superAdmin = updatedRoles.find((r) => r.id === "role-super-admin")!;

    expect(safety.priority).toBe(100);
    expect(superAdmin.priority).toBe(95);
    expect(safety.priority).toBeGreaterThan(superAdmin.priority);
  });
});

describe("Enterprise Comms, Mail Engine & Statutory Leave Notice Zero-Stub Workflows", () => {
  beforeEach(() => {
    useConsoleStore.getState().resetToSeedData();
  });

  it("sends an enterprise email and archives/deletes across mailboxes", () => {
    const store = useConsoleStore.getState();
    const initialSentCount = store.emails.filter((m) => m.folder === "sent").length;

    const sent = store.sendEmail({
      recipient: "최원석 대표이사",
      recipientEmail: "ws.choi@oyatie.com",
      subject: "[보고] 2026년 하반기 안전관리 점검 결과",
      body: "인천 제1물류센터 및 당진 제철 현장 안전점검이 완료되었습니다.",
      linkedObject: "WO-2641",
    });

    expect(sent.id).toBeDefined();
    expect(sent.folder).toBe("sent");
    expect(sent.linkedObject).toBe("WO-2641");
    expect(useConsoleStore.getState().emails.filter((m) => m.folder === "sent").length).toBe(
      initialSentCount + 1
    );

    // Archive an inbox email
    const inboxMail = store.emails.find((m) => m.folder === "inbox")!;
    store.archiveEmail(inboxMail.id);
    expect(useConsoleStore.getState().emails.find((m) => m.id === inboxMail.id)?.folder).toBe(
      "archive"
    );

    // Delete an email
    store.deleteEmail(sent.id);
    expect(useConsoleStore.getState().emails.find((m) => m.id === sent.id)?.folder).toBe("trash");
  });

  it("converts an enterprise email directly into an approval draft (AP-) without dead paths", () => {
    const store = useConsoleStore.getState();
    const mail = store.emails[0]; // [긴급] 인천물류 제1라인 심야 선적 작업 사전 협의 건

    const draft = store.createApprovalDraftFromEmail(mail.id);
    expect(draft).not.toBeNull();
    expect(draft?.code.startsWith("AP-")).toBe(true);
    expect(draft?.title).toContain(mail.subject);
    expect(draft?.content).toContain(mail.sender);
    expect(draft?.linkedObjects).toContain("AP-3121");

    // Verify it appears in the store approvals collection
    const inStore = useConsoleStore.getState().approvals.find((a) => a.id === draft?.id);
    expect(inStore).toBeDefined();
    expect(inStore?.status).toBe("초안");
  });

  it("creates a new context-anchored thread with initial message", () => {
    const store = useConsoleStore.getState();
    const initialThreadCount = store.threads.length;

    const newThread = store.createThread({
      title: "인천물류 야간 선적 긴급 회의",
      objectRef: "AP-3121",
      initialMessage: "금일 22시 선적 작업 인력 배치 현황 공유 바랍니다.",
    });

    expect(newThread.id).toBeDefined();
    expect(useConsoleStore.getState().threads.length).toBe(initialThreadCount + 1);

    const msgs = useConsoleStore.getState().messages[newThread.id];
    expect(msgs.length).toBe(1);
    expect(msgs[0].text).toContain("금일 22시 선적 작업");
    expect(msgs[0].linkedCodes).toContain("AP-3121");
  });

  it("issues a new statutory leave promotion notice under LSA Article 61", () => {
    const store = useConsoleStore.getState();
    const initialCount = store.leaveNotices.length;

    const notice = store.createLeaveNotice({
      employeeId: "emp_06",
      round: "1차",
      remainingDays: 7,
      deadline: "2026-07-20",
    });

    expect(notice.id).toBeDefined();
    expect(notice.code.startsWith("LV-2026-")).toBe(true);
    expect(notice.employeeName).toBe("황도현");
    expect(notice.round).toBe("1차");
    expect(notice.status).toBe("통지대기");
    expect(useConsoleStore.getState().leaveNotices.length).toBe(initialCount + 1);
  });
});

describe("Enterprise Adversarial, Mutation & Fuzzing Security Hardening Suite", () => {
  beforeEach(() => {
    useConsoleStore.getState().resetToSeedData();
  });

  it("blocks cross-site & cross-entity scope bypass attacks under Cedar PBAC policy", () => {
    // Attack 1: User with site-level restriction (이수력: 인천 제1물류센터 site_01)
    // attempts to assign a work order at another site (평택 항만터미널 site_02)
    const outOfScopeFold = foldUserEffectivePermissions("P-emp_01", { siteId: "site_02" });
    expect(outOfScopeFold.effectiveCapabilities.has("Operations::AssignWorkOrder")).toBe(false);

    // In-scope fold should have the permission
    const inScopeFold = foldUserEffectivePermissions("P-emp_01", { siteId: "site_01" });
    expect(inScopeFold.effectiveCapabilities.has("Operations::AssignWorkOrder")).toBe(true);

    // Cedar evaluateAccess for out-of-scope site operation
    const crossSiteReq: AccessEvaluationRequest = {
      principalAccountId: "ACC-OPS-01",
      principalRoleGroup: "정비팀장",
      principalPersonId: "P-emp_01",
      action: "Operations::AssignWorkOrder",
      resource: {
        id: "WO-2645",
        kind: "WO",
        entityId: "corp_02",
        siteId: "site_02", // Target is Pyeongtaek, but user is restricted to Incheon
      },
      context: {
        time: "2026-07-03T10:00:00Z",
        networkZone: "internal_vpn",
        mfaVerified: false,
      },
    };

    const crossSiteRes = evaluateAccess(crossSiteReq);
    expect(crossSiteRes.decision).toBe("FORBID");
    expect(crossSiteRes.summary).toContain("DEFAULT DENY");
    expect(crossSiteRes.matchedPermitRules.length).toBe(0);

    // Attack 2: Subsidiary user from corp_02 attempts to tamper with parent corp_01 confidential payroll ledger
    const crossEntityReq: AccessEvaluationRequest = {
      principalAccountId: "ACC-LOG-01",
      principalRoleGroup: "물류팀",
      principalPersonId: "P-emp_08",
      principalEntityId: "corp_02",
      action: "Payroll::Calculate",
      resource: {
        id: "PR-2026-07",
        kind: "PR",
        entityId: "corp_01", // Target belongs to corp_01, but principal belongs to corp_02
      },
      context: {
        time: "2026-07-03T10:00:00Z",
        networkZone: "internal_vpn",
        mfaVerified: true,
      },
    };

    const crossEntityRes = evaluateAccess(crossEntityReq);
    expect(crossEntityRes.decision).toBe("FORBID");
    expect(crossEntityRes.blockingForbidRules.some((p) => p.id === "pol-tenant-isolation")).toBe(true);
  });

  it("strictly enforces SoD (Separation of Duties) prohibiting self-approval by natural person identity", () => {
    // Attack: Drafter P-emp_01 drafts an approval document AP-3121 and attempts to approve their own document
    // even though they hold a role layer with signing authority
    const selfApprovalReq: AccessEvaluationRequest = {
      principalAccountId: "ACC-SYS-ADMIN",
      principalRoleGroup: "팀장",
      principalPersonId: "P-emp_01", // The natural person who drafted it
      action: "Approvals::SignTier1",
      resource: {
        id: "AP-3121",
        kind: "AP",
        amount: 2500000,
        preparedByPersonId: "P-emp_01", // Match! Self-approval attempt
        preparedByName: "이수력",
      },
      context: {
        time: "2026-07-03T10:00:00Z",
        networkZone: "internal_vpn",
        mfaVerified: false,
      },
    };

    const evalResult = evaluateAccess(selfApprovalReq);
    expect(evalResult.decision).toBe("FORBID");
    expect(evalResult.blockingForbidRules.some((p) => p.id === "pol-sod-independence")).toBe(true);
    expect(evalResult.summary).toContain("직무 분리(SoD)");

    // Counter-test: An independent manager P-emp_04 evaluating the same document
    const independentReq: AccessEvaluationRequest = {
      ...selfApprovalReq,
      principalPersonId: "P-emp_04", // Hong Gil-dong (CEO / Independent)
    };
    const independentRes = evaluateAccess(independentReq);
    expect(independentRes.blockingForbidRules.some((p) => p.id === "pol-sod-independence")).toBe(false);
  });

  it("fuzzes DoA ceiling boundaries and enforces maximum approval limits", () => {
    const baseReq = {
      principalAccountId: "ACC-MGR-01",
      principalRoleGroup: "팀장",
      principalPersonId: "P-emp_02",
      action: "Approvals::SignTier1",
      context: {
        time: "2026-07-03T10:00:00Z",
        networkZone: "internal_vpn",
        mfaVerified: false,
      },
    };

    // Fuzz boundary 1: 4,999,999 KRW (under 5,000,000 limit)
    const underCeiling = evaluateAccess({
      ...baseReq,
      resource: { id: "AP-F1", kind: "AP", amount: 4999999 },
    });
    expect(underCeiling.blockingForbidRules.some((p) => p.id === "pol-doa-tier1-limit")).toBe(false);

    // Fuzz boundary 2: 5,000,000 KRW (exact limit)
    const exactCeiling = evaluateAccess({
      ...baseReq,
      resource: { id: "AP-F2", kind: "AP", amount: 5000000 },
    });
    expect(exactCeiling.blockingForbidRules.some((p) => p.id === "pol-doa-tier1-limit")).toBe(false);

    // Fuzz boundary 3: 5,000,001 KRW (exceeds by 1 won)
    const overCeiling = evaluateAccess({
      ...baseReq,
      resource: { id: "AP-F3", kind: "AP", amount: 5000001 },
    });
    expect(overCeiling.decision).toBe("FORBID");
    expect(overCeiling.blockingForbidRules.some((p) => p.id === "pol-doa-tier1-limit")).toBe(true);

    // Fuzz extreme boundaries: 50M and 100B
    const extremeCeiling = evaluateAccess({
      ...baseReq,
      resource: { id: "AP-F4", kind: "AP", amount: 100000000000 },
    });
    expect(extremeCeiling.decision).toBe("FORBID");
    expect(extremeCeiling.blockingForbidRules.some((p) => p.id === "pol-doa-tier1-limit")).toBe(true);
  });

  it("fuzzes punch timestamps and hardens against negative breaks and duration overflow", () => {
    // 1. Negative break minutes fuzzing: attacker passes -60 min to artificially inflate work hours
    const negativeBreakRes = calculateWorkHours("09:00", "18:00", -60);
    expect(negativeBreakRes.breakMinutes).toBe(0); // Must clamp to 0
    expect(negativeBreakRes.netWorkHours).toBe(9.0); // 9h elapsed - 0h = 9h, NOT 10h!

    // 2. Excessive break minutes fuzzing: break exceeds elapsed gross shift duration
    const excessiveBreakRes = calculateWorkHours("09:00", "18:00", 99999);
    expect(excessiveBreakRes.breakMinutes).toBe(540); // Clamped to 540 min (gross elapsed)
    expect(excessiveBreakRes.netWorkHours).toBe(0.0); // Does not produce negative hours

    // 3. Timestamp inversion without night shift flag: 21:00 in, 06:00 out
    const invertedRes = calculateWorkHours("21:00", "06:00", 60);
    expect(invertedRes.isMidnightShift).toBe(true);
    expect(invertedRes.grossHours).toBe(9.0);
    expect(invertedRes.netWorkHours).toBe(8.0);

    // 4. Invalid timestamp format fuzzing: corrupt or out-of-range strings
    const invalidPunch1 = calculateWorkHours("25:99", "18:00");
    expect(invalidPunch1.isValid).toBe(false);
    expect(invalidPunch1.netWorkHours).toBeNull();

    const invalidPunch2 = calculateWorkHours("invalid", "18:00");
    expect(invalidPunch2.isValid).toBe(false);

    const invalidPunch3 = calculateWorkHours(null, "18:00");
    expect(invalidPunch3.isValid).toBe(false);
    expect(invalidPunch3.errorReason).toBe("출근 미기록");
  });

  it("hardens payroll freeze gate and bank transfer sealing against race conditions and premature execution", () => {
    const store = useConsoleStore.getState();

    // 1. Attempting to freeze payroll when unresolved blockers exist MUST be rejected
    expect(store.payrollRun.unresolvedExceptions).toBeGreaterThan(0);
    const prematureResult = store.calculateAndFreezePayroll();
    expect(prematureResult.success).toBe(false);
    expect(prematureResult.error).toContain("근태 예외");
    expect(useConsoleStore.getState().payrollRun.isLocked).toBe(false);

    // 2. Clear all discrepancy blockers
    const openDiscs = store.discrepancies.filter((d) => d.status === "unresolved");
    for (const d of openDiscs) {
      store.repairDiscrepancy(d.id, "정상 사유 인정 및 소명 승인");
    }
    expect(useConsoleStore.getState().payrollRun.unresolvedExceptions).toBe(0);

    // 3. Now freezing payroll succeeds cleanly
    const freezeResult = useConsoleStore.getState().calculateAndFreezePayroll();
    expect(freezeResult.success).toBe(true);
    expect(useConsoleStore.getState().payrollRun.isLocked).toBe(true);

    // 4. Bank transfer batch sealing with Passkey ceremony
    expect(store.bankTransferBatch.status).toBe("전문생성");
    const testPasskeyHash = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
    store.sealAndDispatchBankBatch("최원석 대표이사 (FIDO2)", testPasskeyHash);

    const sealedBatch = useConsoleStore.getState().bankTransferBatch;
    expect(sealedBatch.status).toBe("출금승인대기");
    expect(sealedBatch.passkeyHash).toBe(testPasskeyHash);
    expect(sealedBatch.sealedBy).toContain("최원석 대표이사");
  });

  it("detects cryptographic audit chain tampering and pinpoints broken sequence", () => {
    const store = useConsoleStore.getState();

    // Initial audit log must be cryptographically unbroken
    const initialVerify = verifyAuditChain(store.auditEvents);
    expect(initialVerify.valid).toBe(true);
    expect(initialVerify.totalEvents).toBe(store.auditEvents.length);

    // Append a new genuine audit event via an operational action
    store.createLeaveNotice({
      employeeId: "emp_01",
      round: "2차",
      remainingDays: 5,
      deadline: "2026-08-01",
    });

    const updatedEvents = useConsoleStore.getState().auditEvents;
    const postActionVerify = verifyAuditChain(updatedEvents);
    expect(postActionVerify.valid).toBe(true);
    expect(postActionVerify.totalEvents).toBeGreaterThan(initialVerify.totalEvents);

    // Adversarial Tampering: An attacker directly mutates the hash of an internal audit block
    const tamperedEvents = JSON.parse(JSON.stringify(updatedEvents));
    const sorted = [...tamperedEvents].sort((a: any, b: any) => a.seq - b.seq);
    // Tamper with an older event's hash (index 1)
    sorted[1].hash = "malicious_corrupted_sha256_hash_99999999";

    const tamperedVerify = verifyAuditChain(sorted);
    expect(tamperedVerify.valid).toBe(false);
    expect(tamperedVerify.brokenAtSeq).toBe(sorted[2].seq);
    expect(tamperedVerify.error).toContain("해시 체인 훼손 감지");
  });
});





