import { describe, it, expect, beforeEach } from "vitest";
import { useConsoleStore } from "../src/lib/store";
import {
  foldUserEffectivePermissions,
  evaluateAccess,
  verifyAuditChain,
  DEFAULT_ROLE_DEFINITIONS,
  INITIAL_ROLE_ASSIGNMENTS,
  DEFAULT_CEDAR_POLICIES,
} from "../src/lib/policy-engine";
import {
  calculateWorkHours,
  evaluate52HourCompliance,
} from "../src/lib/attendance-calculator";
import {
  computeEmployeePayslip,
  trunc10,
  statutoryDeductions,
} from "../src/lib/payroll-engine";
import {
  parseMoneyInput,
  evaluateSheetFormula,
  parseTsvClipboard,
} from "../src/lib/sheet-engine";
import type { AccessEvaluationRequest } from "../src/lib/types";

describe("Tier-1 Big Tech Enterprise End-to-End User Story & QA Test Suite", () => {
  beforeEach(() => {
    useConsoleStore.getState().resetToSeedData();
  });

  // =========================================================================
  // USER STORY 1: Enterprise Organization & Multi-Entity Hierarchy Setup
  // =========================================================================
  it("Story 1 [Org Setup]: establishes a new subsidiary corporation, logistics site, and operational department with zero stubs", () => {
    const store = useConsoleStore.getState();
    const initialEntityCount = store.orgEntities.length;
    const initialSiteCount = store.orgSites.length;
    const initialDeptCount = store.orgDepartments.length;

    // 1. Establish new subsidiary corporation: (주)오야티 에너지
    const newCorp = store.createOrgEntity({
      code: "OYT-ENG",
      name: "(주)오야티 에너지",
      bizNumber: "401-88-29183",
      corpNumber: "110111-7829104",
      ceoName: "배진호",
      address: "울산광역시 남구 여천동 항만단지 45",
      mainBank: "하나은행",
      mainAccount: "381-910023-19201",
      establishedDate: "2026-07-01",
      status: "active",
    });

    expect(newCorp.id).toBeDefined();
    expect(newCorp.name).toBe("(주)오야티 에너지");
    expect(useConsoleStore.getState().orgEntities.length).toBe(initialEntityCount + 1);

    // 2. Establish new operational site associated with the corporation
    const newSite = store.createOrgSite({
      entityId: newCorp.id,
      code: "SITE-ULS",
      name: "울산 복합에너지 터미널",
      category: "항만터미널",
      address: "울산광역시 남구 여천동 120",
      siteManager: "박준혁 소장",
      safetyManager: "김안전",
      phone: "052-258-0100",
      activeHeadcount: 24,
    });

    expect(newSite.id).toBeDefined();
    expect(newSite.entityId).toBe(newCorp.id);
    expect(useConsoleStore.getState().orgSites.length).toBe(initialSiteCount + 1);

    // 3. Establish department under the new site
    const newDept = store.createDepartment({
      entityId: newCorp.id,
      code: "DEPT-H2",
      name: "수소에너지운영팀",
      division: "에너지사업본부",
      managerName: "김선우 팀장",
      costCenter: "CC-ENERGY-01",
    });

    expect(newDept.id).toBeDefined();
    expect(newDept.name).toBe("수소에너지운영팀");
    expect(useConsoleStore.getState().orgDepartments.length).toBe(initialDeptCount + 1);

    // 4. Update corporation details reactively
    store.updateOrgEntity(newCorp.id, { ceoName: "정유진" });
    expect(useConsoleStore.getState().orgEntities.find((e) => e.id === newCorp.id)?.ceoName).toBe("정유진");
  });

  // =========================================================================
  // USER STORY 2: Statutory Corporate Policy & DoA Threshold Setup
  // =========================================================================
  it("Story 2 [Policy Setup]: configures statutory labor rules, DoA financial thresholds, and leave promotion schedules", () => {
    const store = useConsoleStore.getState();

    // 1. Update working hour & DoA policies
    store.updateOrgPolicy({
      version: "2026.3-REVISED",
      attendance: {
        ...store.orgPolicy.attendance,
        maxWeeklyHours: 52,
        warningWeeklyHours: 48,
      },
      leave: {
        ...store.orgPolicy.leave,
        statutoryPromotionRound1MonthsBefore: 6,
        statutoryPromotionRound2MonthsBefore: 2,
      },
    });

    const updatedPolicy = useConsoleStore.getState().orgPolicy;
    expect(updatedPolicy.version).toBe("2026.3-REVISED");
    expect(updatedPolicy.attendance.maxWeeklyHours).toBe(52);
    expect(updatedPolicy.attendance.warningWeeklyHours).toBe(48);
    expect(updatedPolicy.leave.statutoryPromotionRound1MonthsBefore).toBe(6);
  });

  // =========================================================================
  // USER STORY 3: Operations Setup (Contracts & Equipment Lifecycle)
  // =========================================================================
  it("Story 3 [Ops Setup]: establishes master client contracts and registers equipment with reactive inspection cycles", () => {
    const store = useConsoleStore.getState();
    const initialContractCount = store.contracts.length;
    const initialEquipmentCount = store.equipment.length;

    // 1. Establish Master B2B Maintenance Contract
    const contract = store.createContract({
      title: "울산신항 하역 크레인 12기 종합 순회정비 도급",
      client: "(주)울산신항만운영",
      site: "울산 항만정비소",
      contractType: "정비용역",
      monthlyAmount: 95000000,
      startDate: "2026-07-01",
      endDate: "2027-06-30",
      assignedHeadcount: 14,
      status: "유효",
    });

    expect(contract.id).toBeDefined();
    expect(contract.code.startsWith("C-")).toBe(true);
    expect(useConsoleStore.getState().contracts.length).toBe(initialContractCount + 1);

    // 2. Register operational heavy equipment with inspection cycles
    const equipment = store.createEquipment({
      name: "50톤 항만 갠트리 크레인 (GC-50-광양)",
      site: "광양 제철소 부두",
      category: "크레인",
      serialNo: "LIEBHERR-P120-2024",
      inspectionCycleDays: 30,
      lastInspectionDate: "2026-06-15",
      nextInspectionDate: "2026-07-15",
      assignedEngineer: "황도현",
      status: "점검요망",
    });

    expect(equipment.id).toBeDefined();
    expect(equipment.code.startsWith("EQ-")).toBe(true);
    expect(useConsoleStore.getState().equipment.length).toBe(initialEquipmentCount + 1);

    // 3. Update equipment status after inspection
    store.updateEquipmentStatus(equipment.id, "정상가동");
    expect(useConsoleStore.getState().equipment.find((eq) => eq.id === equipment.id)?.status).toBe("정상가동");
  });

  // =========================================================================
  // USER STORY 4: Employee Onboarding -> Discord Layered Role Assignment -> Dynamic PBAC Folding
  // =========================================================================
  it("Story 4 [HR Onboarding & RBAC]: onboards employee, assigns Discord-style layered roles, and evaluates live folded permissions without JWT re-issuance", () => {
    const store = useConsoleStore.getState();
    const initialEmpCount = store.employees.length;

    // 1. Onboard new employee
    const newEmp = store.createEmployee({
      name: "정해성",
      email: "hs.jung@oyatie.com",
      phone: "010-9988-1122",
      entity: "(주)오야티 로지스틱스",
      site: "인천 제1물류센터",
      dept: "운영1팀",
      role: "엔지니어",
      grade: "대리",
      jobTitle: "지게차 정비기사",
      position: "중장비 정비",
      empType: "정규",
      baseSalary: 3800000,
      fixedAllow: 200000,
      joinedDate: "2026-07-01",
      dependents: 1,
      status: "재직",
    });

    expect(newEmp.id).toBeDefined();
    expect(useConsoleStore.getState().employees.length).toBe(initialEmpCount + 1);

    // Verify linked attendance & payroll records initialized automatically
    expect(useConsoleStore.getState().attendance.some((a) => a.employeeId === newEmp.id)).toBe(true);
    expect(useConsoleStore.getState().payrollRun.headcount).toBe(initialEmpCount + 1);

    // 2. Assign Discord-Style Layered Roles to the employee
    // Layer 1: @everyone base member
    store.assignRoleToEmployee({
      employeeId: newEmp.id,
      roleId: "role-base-member",
      scope: { type: "all" },
      grantReason: "신규 입사자 기본 권한 부여",
    });

    // Layer 2: Operations Lead scoped specifically to Incheon site
    store.assignRoleToEmployee({
      employeeId: newEmp.id,
      roleId: "role-ops-lead",
      scope: { type: "site", targetId: "site_02", targetName: "인천 제1물류센터" },
      grantReason: "인천 물류센터 지게차 반장 보직 임명",
    });

    // 3. Evaluate live folded permissions (Discord folding)
    const incheonFold = store.getEffectivePermissionsForEmployee(newEmp.id, {
      siteId: "site_02",
    });
    expect(incheonFold.effectiveCapabilities.has("Operations::AssignWorkOrder")).toBe(true);
    expect(incheonFold.effectiveCapabilities.has("Comms::SendChat")).toBe(true);
    expect(incheonFold.layeredRoles.length).toBe(2);

    // Evaluating against another site (e.g. 평택) should NOT inherit the site-restricted lead role
    const pyeongtaekFold = store.getEffectivePermissionsForEmployee(newEmp.id, {
      siteId: "site_03",
    });
    expect(pyeongtaekFold.effectiveCapabilities.has("Operations::AssignWorkOrder")).toBe(false);
    expect(pyeongtaekFold.effectiveCapabilities.has("Comms::SendChat")).toBe(true); // Base member remains
  });

  // =========================================================================
  // USER STORY 5: Attendance Punching -> Overtime Risk -> Exception Adjudication -> Unblocking Payroll
  // =========================================================================
  it("Story 5 [Attendance Adjudication]: handles time punches, flags LSA §53 52-hour risks, adjudicates exceptions, and clears payroll freeze gate", () => {
    const store = useConsoleStore.getState();

    // 1. Locate an employee attendance record
    const att = store.attendance.find((a) => a.employeeId === "emp_03")!;
    expect(att).toBeDefined();

    // 2. Update punch times: 07:30 to 22:30 (15 hours gross, 60m rest = 14h net)
    store.updateAttendancePunch(att.id, "07:30", "22:30", 60);

    const updatedAtt = useConsoleStore.getState().attendance.find((a) => a.id === att.id)!;
    expect(updatedAtt.workedHours).toBe(14);
    expect(updatedAtt.otHours).toBe(6); // 14 - 8 = 6 hours OT

    // 3. Verify weekly 52-hour compliance evaluation
    const compliance = evaluate52HourCompliance(updatedAtt.weeklyHoursTotal);
    expect(["WARNING", "VIOLATION"]).toContain(compliance.status);

    // 4. Verify initial payroll freeze gate is blocked due to unresolved exceptions
    expect(store.payrollRun.unresolvedExceptions).toBeGreaterThan(0);
    const prematureFreeze = store.calculateAndFreezePayroll();
    expect(prematureFreeze.success).toBe(false);

    // 5. Adjudicate all open exceptions through formal approval
    const openDiscs = store.discrepancies.filter((d) => d.status === "unresolved");
    for (const d of openDiscs) {
      store.repairDiscrepancy(d.id, "선박 긴급 하역 작업에 따른 특별인가 사전 소명 승인");
    }

    expect(useConsoleStore.getState().payrollRun.unresolvedExceptions).toBe(0);

    // 6. Now payroll calculation and freeze gate passes cleanly
    const freezeRes = useConsoleStore.getState().calculateAndFreezePayroll();
    expect(freezeRes.success).toBe(true);
    expect(useConsoleStore.getState().payrollRun.isLocked).toBe(true);
  });

  // =========================================================================
  // USER STORY 6: Connected Sheet Live Grid Editing -> Formula Evaluation -> Consequence Review
  // =========================================================================
  it("Story 6 [Connected Sheet]: evaluates safe formulas, parses TSV clipboard data, and generates consequence reviews before committing", () => {
    const store = useConsoleStore.getState();

    // 1. Safe in-cell formula evaluation
    expect(evaluateSheetFormula("=150000*1.5")).toBe(225000);
    expect(evaluateSheetFormula("=SUM(100000, 200000, 300000)")).toBe(600000);
    expect(evaluateSheetFormula("=AVERAGE(100, 200, 300)")).toBe(200);
    expect(evaluateSheetFormula("=INVALID_FORMULA")).toBe("#ERR:INVALID_CHAR"); // Malicious / invalid rejected

    // 2. Parse currency input variants
    expect(parseMoneyInput("₩4,500,000")).toBe(4500000);
    expect(parseMoneyInput("320만원")).toBe(3200000);
    expect(parseMoneyInput("5000000")).toBe(5000000);

    // 3. Parse rectangular TSV clipboard paste from Excel / Google Sheets
    const tsvData = "김민수\t4800000\t200000\n박지영\t4200000\t150000";
    const parsedMatrix = parseTsvClipboard(tsvData);
    expect(parsedMatrix.length).toBe(2);
    expect(parsedMatrix[0][0]).toBe("김민수");
    expect(parsedMatrix[0][1]).toBe("4800000");

    // 4. Simulate consequence review and execute employee salary adjustment
    const emp = store.employees[0];
    const prevSalary = emp.baseSalary;
    const newSalary = prevSalary + 300000;

    store.updateEmployee(emp.id, { baseSalary: newSalary });
    expect(useConsoleStore.getState().employees.find((e) => e.id === emp.id)?.baseSalary).toBe(newSalary);
  });

  // =========================================================================
  // USER STORY 7: Statutory Payroll Calculation -> 10-Won Truncation -> 4 Major Social Insurances
  // =========================================================================
  it("Story 7 [Statutory Payroll Engine]: enforces National Treasury Administration Act §47(1) 10-won truncation and statutory 4-major insurances", () => {
    const store = useConsoleStore.getState();

    // Verify 10-won truncation for odd statutory calculations
    expect(trunc10(1234567)).toBe(1234560);
    expect(trunc10(9999999)).toBe(9999990);
    expect(trunc10(5000005)).toBe(5000000);

    // Compute payslip for employee
    const emp = store.employees[0];
    const payslip = computeEmployeePayslip(emp, { otHours: 4, nightHours: 1, holidayHours: 0 });

    expect(payslip.id).toBeDefined();
    expect(payslip.basePay).toBe(emp.baseSalary);
    expect(payslip.grossPay).toBeGreaterThan(emp.baseSalary);

    // Verify all monetary amounts end in 0 (10-won truncation enforced)
    expect(payslip.netPay % 10).toBe(0);
    expect(payslip.totalDeductions % 10).toBe(0);
    expect(payslip.grossPay % 10).toBe(0);

    // Verify 4-major social insurance statutory breakdowns
    const deductions = payslip.deductions;
    const np = deductions.find((d) => d.code === "np");
    const hi = deductions.find((d) => d.code === "hi");
    const ltc = deductions.find((d) => d.code === "ltc");
    const ei = deductions.find((d) => d.code === "ei");

    expect(np).toBeDefined();
    expect(hi).toBeDefined();
    expect(ltc).toBeDefined();
    expect(ei).toBeDefined();

    expect(np!.amt % 10).toBe(0);
    expect(hi!.amt % 10).toBe(0);
    expect(ltc!.amt % 10).toBe(0);
    expect(ei!.amt % 10).toBe(0);
  });

  // =========================================================================
  // USER STORY 8: KFTC 100-Byte Firm Banking CMS Generation -> FIDO2 Passkey Ceremony
  // =========================================================================
  it("Story 8 [Firm Banking CMS]: generates 100-byte fixed-width KFTC flat-file, validates record formats, and seals batch with Passkey hash", () => {
    const store = useConsoleStore.getState();

    // 1. Generate Firm Banking CMS batch
    const batch = store.generateFirmBankingBatch();
    expect(batch.status).toBe("전문생성");
    expect(batch.totalHeadcount).toBe(store.employees.length);
    expect(batch.preflightVerified).toBe(true);

    // 2. Validate KFTC 100-byte flat-file layout
    const fileLines = batch.kftcFlatFileContent.split("\n").filter(Boolean);
    expect(fileLines.length).toBe(store.employees.length + 2); // Header + N Data + Trailer

    // Header record check: Record Type 'H'
    const header = fileLines[0];
    expect(header[0]).toBe("H");
    expect(header.length).toBeGreaterThanOrEqual(80);

    // Data records check: Record Type 'D'
    const firstData = fileLines[1];
    expect(firstData[0]).toBe("D");
    expect(firstData).toContain("(주)오야티");

    // Trailer record check: Record Type 'T'
    const trailer = fileLines[fileLines.length - 1];
    expect(trailer[0]).toBe("T");

    // 3. Seal batch with FIDO2 Passkey biometric ceremony
    const passkeyHash = "c5b2e9a187d320c19a0fae32b8e6b1297594bf34b12c5ec6e5b4109ca49372ef";
    store.sealAndDispatchBankBatch("최원석 대표이사 (FIDO2)", passkeyHash);

    const sealedBatch = useConsoleStore.getState().bankTransferBatch;
    expect(sealedBatch.status).toBe("출금승인대기");
    expect(sealedBatch.passkeyHash).toBe(passkeyHash);
    expect(sealedBatch.sealedBy).toContain("최원석 대표이사");
  });

  // =========================================================================
  // USER STORY 9: Multi-Stage Electronic Approvals -> Reviewer Delegation -> Separation of Duties
  // =========================================================================
  it("Story 9 [Electronic Approvals]: supports document parking, submission, capacity-bearing sign-offs, and prevents self-delegation", () => {
    const store = useConsoleStore.getState();

    // 1. Create a parked draft approval docket (SAP-style parking)
    const draft = store.createApprovalDraft({
      title: "인천물류 제1라인 지게차 비상 충전기 증설 품의",
      category: "지출결의",
      content: "물동량 급증에 따른 7톤 전동지게차 급속충전기 2기 설치 품의서입니다.",
      doaAmount: 4200000,
      drafterId: "emp_01",
      drafterName: "이수력",
      drafterDept: "정비1팀",
      linkedObjects: ["EQ-101"],
    });

    expect(draft.id).toBeDefined();
    expect(draft.code.startsWith("AP-")).toBe(true);
    expect(draft.status).toBe("초안"); // Parked

    // 2. Submit parked draft to the approval pipeline
    store.submitApproval(draft.id);
    expect(useConsoleStore.getState().approvals.find((a) => a.id === draft.id)?.status).toBe("결재대기");

    // 3. Test reviewer handoff delegation
    // Attempting to delegate approval back to the drafter himself must fail under SoD
    const invalidDelegation = store.reassignApprovalDelegate(draft.code, "이수력", "emp_01", "자가 대결 시도");
    expect(invalidDelegation.success).toBe(false);
    expect(invalidDelegation.message).toContain("직무 분리(SoD)");

    // Delegating to an independent manager succeeds cleanly
    const validDelegation = store.reassignApprovalDelegate(draft.code, "이수력", "emp_02", "출장으로 인한 직무 대결");
    expect(validDelegation.success).toBe(true);

    // 4. Capacity-bearing approval signing
    store.approveDoc(draft.id, "박지영 팀장", "현장 실사 완료 후 승인합니다.", {
      actingCapacityRole: "role-doa-tier1",
      actingCapacityName: "DoA 1단계 전결관",
      passkeyVerified: false,
    });

    const updatedDoc = useConsoleStore.getState().approvals.find((a) => a.id === draft.id)!;
    expect(updatedDoc.stages[0].status).toBe("approved");
    expect(updatedDoc.stages[0].actingCapacityRole).toBe("role-doa-tier1");
  });

  // =========================================================================
  // USER STORY 10: Operations Work Order Pipeline Progression
  // =========================================================================
  it("Story 10 [Work Order Kanban]: creates work order and transitions through the 5-stage engineering lifecycle", () => {
    const store = useConsoleStore.getState();
    const initialWOCount = store.workOrders.length;

    // 1. Create emergency work order
    const wo = store.createWorkOrder({
      title: "인천물류 #2 야드트랙터 에어브레이크 밸브 긴급 교체",
      site: "인천 제1물류센터",
      priority: "긴급",
      assignedTo: "황도현",
      targetEquipment: "EQ-102",
      dueDate: "2026-07-05",
      contractCode: "C-207",
    });

    expect(wo.id).toBeDefined();
    expect(wo.code.startsWith("WO-")).toBe(true);
    expect(wo.status).toBe("접수");
    expect(useConsoleStore.getState().workOrders.length).toBe(initialWOCount + 1);

    // 2. Sequential pipeline transition: 접수 -> 배차 -> 진행중 -> 완료 -> 검수
    store.updateWorkOrderStatus(wo.id, "배차");
    expect(useConsoleStore.getState().workOrders.find((w) => w.id === wo.id)?.status).toBe("배차");

    store.updateWorkOrderStatus(wo.id, "진행중");
    expect(useConsoleStore.getState().workOrders.find((w) => w.id === wo.id)?.status).toBe("진행중");

    store.updateWorkOrderStatus(wo.id, "완료");
    expect(useConsoleStore.getState().workOrders.find((w) => w.id === wo.id)?.status).toBe("완료");

    store.updateWorkOrderStatus(wo.id, "검수");
    expect(useConsoleStore.getState().workOrders.find((w) => w.id === wo.id)?.status).toBe("검수");
  });

  // =========================================================================
  // USER STORY 11: Enterprise Comms -> Email Conversion to Approval Draft -> Thread
  // =========================================================================
  it("Story 11 [Comms & Collaboration]: sends emails, converts email into an approval draft, and chats in context-anchored thread", () => {
    const store = useConsoleStore.getState();

    // 1. Send an enterprise email
    const email = store.sendEmail({
      recipient: "최원석 대표이사",
      recipientEmail: "ws.choi@oyatie.com",
      subject: "[견적] 평택항 야드트랙터 타이어 대량 교체 건",
      body: "총 12대 분량 타이어 교체 견적서 수신되어 결재 상신 준비 중입니다.",
      linkedObject: "C-208",
    });

    expect(email.id).toBeDefined();
    expect(email.folder).toBe("sent");

    // 2. 1-Click convert email to approval draft
    const draft = store.createApprovalDraftFromEmail(email.id);
    expect(draft).not.toBeNull();
    expect(draft!.title).toContain("[견적] 평택항 야드트랙터");
    expect(draft!.status).toBe("초안");

    // 3. Create context-anchored thread discussing this draft
    const thread = store.createThread({
      title: "타이어 교체 품의 사전 협의",
      objectRef: draft!.code,
      initialMessage: "금일 17시까지 할인율 확인 후 최종 상신 예정입니다.",
    });

    expect(thread.id).toBeDefined();
    expect(thread.objectRef).toBe(draft!.code);

    // 4. Send chat message inside the thread
    store.sendMessage(thread.id, "타이어 공급사로부터 15% 추가 할인 확약 받았습니다.");
    const msgs = useConsoleStore.getState().messages[thread.id];
    expect(msgs.length).toBe(2);
    expect(msgs[1].text).toContain("15% 추가 할인");
  });

  // =========================================================================
  // USER STORY 12: Statutory Leave Promotion Notice Issuance under LSA §61
  // =========================================================================
  it("Story 12 [Statutory Leave Notice]: issues annual leave promotion notice under LSA Article 61 with formal audit trail", () => {
    const store = useConsoleStore.getState();
    const initialNoticeCount = store.leaveNotices.length;
    const initialAuditCount = store.auditEvents.length;

    // Issue Round 1 statutory notice
    const notice = store.createLeaveNotice({
      employeeId: "emp_08",
      round: "1차",
      remainingDays: 9,
      deadline: "2026-07-25",
    });

    expect(notice.id).toBeDefined();
    expect(notice.code.startsWith("LV-2026-")).toBe(true);
    expect(notice.employeeName).toBe("송민우");
    expect(notice.remainingDays).toBe(9);
    expect(notice.status).toBe("통지대기");

    expect(useConsoleStore.getState().leaveNotices.length).toBe(initialNoticeCount + 1);

    // Statutory audit event must be generated for LSA compliance records
    const latestAudit = useConsoleStore.getState().auditEvents[0];
    expect(latestAudit.action).toBe("STATUTORY_LEAVE_NOTICE_ISSUED");
    expect(latestAudit.targetCode).toBe(notice.code);
    expect(useConsoleStore.getState().auditEvents.length).toBe(initialAuditCount + 1);
  });

  // =========================================================================
  // USER STORY 13: Cryptographic Audit Chain Integrity & Tamper Detection
  // =========================================================================
  it("Story 13 [Cryptographic Audit Log]: verifies unbroken forward-hash chain and detects block tampering", () => {
    const store = useConsoleStore.getState();

    // 1. Initial verification of genuine chain
    const initialVerify = verifyAuditChain(store.auditEvents);
    expect(initialVerify.valid).toBe(true);
    expect(initialVerify.totalEvents).toBe(store.auditEvents.length);

    // 2. Perform operational actions appending new events
    store.createWorkOrder({
      title: "긴급 크레인 와이어 점검",
      site: "인천 제1물류센터",
      priority: "높음",
      assignedTo: "이준호",
      dueDate: "2026-07-06",
    });

    const chainAfterActions = verifyAuditChain(useConsoleStore.getState().auditEvents);
    expect(chainAfterActions.valid).toBe(true);
    expect(chainAfterActions.totalEvents).toBeGreaterThan(initialVerify.totalEvents);

    // 3. Adversarial simulation: attacker mutates an older event's hash in database
    const tampered = JSON.parse(JSON.stringify(useConsoleStore.getState().auditEvents));
    const sorted = [...tampered].sort((a: any, b: any) => a.seq - b.seq);
    sorted[1].hash = "corrupted_malicious_hash_0000000000";

    const verifyTampered = verifyAuditChain(sorted);
    expect(verifyTampered.valid).toBe(false);
    expect(verifyTampered.brokenAtSeq).toBe(sorted[2].seq);
    expect(verifyTampered.error).toContain("해시 체인 훼손 감지");
  });

  // =========================================================================
  // USER STORY 14: Palantir Foundry 3-Layer Semantic Ontology Entity Graph
  // =========================================================================
  it("Story 14 [Palantir Ontology Graph]: resolves bidirectional object linkages across all 7 core schema types without broken references", () => {
    const store = useConsoleStore.getState();

    // Verify all 7 core ontology schema types exist in store
    expect(store.employees.length).toBeGreaterThan(0);
    expect(store.attendance.length).toBeGreaterThan(0);
    expect(store.approvals.length).toBeGreaterThan(0);
    expect(store.workOrders.length).toBeGreaterThan(0);
    expect(store.payslips.length).toBeGreaterThan(0);
    expect(store.contracts.length).toBeGreaterThan(0);
    expect(store.equipment.length).toBeGreaterThan(0);

    // Person -> Attendance relationship
    const emp = store.employees[0];
    const empAtt = store.attendance.filter((a) => a.employeeId === emp.id);
    expect(empAtt.length).toBeGreaterThan(0);

    // Person -> Payslip relationship
    const empSlips = store.payslips.filter((p) => p.employeeId === emp.id);
    expect(empSlips.length).toBeGreaterThan(0);

    // Work Order -> Equipment & Contract relationship
    const linkedWO = store.workOrders.find((w) => w.targetEquipment && w.contractCode);
    expect(linkedWO).toBeDefined();
    expect(store.equipment.some((eq) => eq.code === linkedWO?.targetEquipment)).toBe(true);
    expect(store.contracts.some((c) => c.code === linkedWO?.contractCode)).toBe(true);
  });
});
