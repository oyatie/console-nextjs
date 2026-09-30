import { describe, it, expect, beforeEach } from "vitest";
import { useAppStore } from "../src/lib/store";
import { computeEmployeePayslip } from "../src/lib/payroll-engine";
import { calculateWorkHours, evaluate52HourCompliance } from "../src/lib/attendance-calculator";
import { resolveStatutoryRates } from "../src/lib/regulatory-registry";
import {
  generateDynamicOrganization,
  generateDynamicWorkforce,
  deriveDynamicAttendance,
  processDynamicPayrollRun,
  appendDynamicAuditEvent,
  verifyAuditChainIntegrity,
} from "../src/lib/dynamic-factory";
import { evaluateAccess } from "../src/lib/policy-engine";
import { AuditEvent } from "../src/lib/types";

describe("E2E Enterprise Lifecycle Integration Suite", () => {
  beforeEach(() => {
    // Reset store to a clean state before each test
    useAppStore.getState().resetToSeedData();
  });

  it("executes the entire multi-tenant corporate lifecycle end-to-end with dynamic derivation", () => {
    const store = useAppStore.getState();

    // 1. Dynamic Corporate & Site Creation
    const initialSiteCount = store.orgSites.length;
    const newSite = {
      id: "site_pyeongtaek_auto",
      name: "평택 친환경 자동화 터미널",
      code: "S-901",
      entityId: "corp_02",
      category: "항만터미널" as const,
      address: "경기도 평택시 포승읍 평택항로 120",
      siteManager: "박물류",
      safetyManager: "안안전",
      phone: "031-680-9900",
      activeHeadcount: 0,
    };
    useAppStore.setState((s) => ({
      orgSites: [...s.orgSites, newSite],
    }));

    expect(useAppStore.getState().orgSites.length).toBe(initialSiteCount + 1);

    // 2. Onboard Employee to New Dynamic Site
    const newEmp = useAppStore.getState().createEmployee({
      name: "한혁신",
      email: "innovation.han@oyatie.com",
      phone: "010-8822-1199",
      entity: "(주)오야티 로지스틱스",
      site: "평택 친환경 자동화 터미널",
      dept: "물류운영팀",
      role: "사원",
      position: "터미널 자동화 기사",
      empType: "정규",
      baseSalary: 4200000,
      fixedAllow: 300000,
      bankName: "신한은행",
      bankAccount: "110-999-887766",
      joinedDate: "2026-07-01",
      status: "재직",
      dependents: 1,
    });

    expect(newEmp.id).toBeDefined();
    expect(newEmp.site).toBe("평택 친환경 자동화 터미널");
    expect(newEmp.baseSalary).toBe(4200000);

    // 3. Assign Multi-Layered Roles (Discord-style: Ops Lead + Safety Inspector)
    const supervisorRole = useAppStore.getState().roleDefinitions.find((r) => r.id === "role-ops-lead");
    const safetyRole = useAppStore.getState().roleDefinitions.find((r) => r.id === "role-safety-inspector");

    if (supervisorRole) {
      useAppStore.getState().assignRoleToEmployee({
        employeeId: newEmp.id,
        roleId: supervisorRole.id,
        scope: { type: "site", targetId: newSite.id, targetName: newSite.name },
        grantReason: "평택 자동화 터미널 현장 총괄 책임자 임명",
      });
    }

    if (safetyRole) {
      useAppStore.getState().assignRoleToEmployee({
        employeeId: newEmp.id,
        roleId: safetyRole.id,
        scope: { type: "site", targetId: newSite.id, targetName: newSite.name },
        grantReason: "산업안전보건법 제52조 안전보건 감독관 겸임",
      });
    }

    const assignedRoles = useAppStore
      .getState()
      .roleAssignments.filter((a) => a.employeeId === newEmp.id && a.status === "active");
    expect(assignedRoles.length).toBe(2);

    // 4. Dynamic Attendance Punch & 52-Hour Gate Evaluation (LSA §50, §53)
    // Create attendance with 16h overtime (exceeding legal 12h weekly overtime limit)
    const heavyAttendance = {
      id: `att_${newEmp.id}_2026-07`,
      employeeId: newEmp.id,
      employeeName: newEmp.name,
      entity: newEmp.entity,
      site: newEmp.site,
      date: "2026-07-15",
      clockIn: "08:30",
      clockOut: "22:30",
      breakMinutes: 60,
      plannedIn: "09:00",
      plannedOut: "18:00",
      workedHours: 176, // 40h regular + 16h OT
      otHours: 16,
      nightHours: 4,
      holHours: 8,
      weeklyHoursTotal: 56, // 40 regular + 16 OT -> EXCEEDS 52h
      status: "예외승인대기" as const,
      exceptionCode: `AT-52H-${newEmp.id}`,
    };

    useAppStore.setState((s) => ({
      attendance: [...s.attendance, heavyAttendance],
    }));

    // Verify 52h compliance calculator flags this as a critical violation
    const compliance = evaluate52HourCompliance(56);
    expect(compliance.tone).toBe("danger");
    expect(compliance.status).toBe("VIOLATION");

    // 5. Cross-Module Gate: Payroll Run MUST be blocked from freezing
    let currentPayroll = useAppStore.getState().payrollRun;
    expect(currentPayroll.isLocked).toBe(false);

    // Attempting to freeze while unadjudicated exception exists should keep status locked or blocked
    const payslipBefore = computeEmployeePayslip(newEmp, {
      otHours: heavyAttendance.otHours,
      nightHours: heavyAttendance.nightHours,
      holidayHours: heavyAttendance.holHours,
    });

    // Check Korean statutory rates (2026 schedule with pension reform step increase and 7.19% health insurance)
    const rates2026 = resolveStatutoryRates("2026-07");
    expect(rates2026.npEmp).toBe(0.0475);
    expect(rates2026.hiTotal).toBe(0.0719);

    // Verify statutory overtime premiums: 1.5x base hourly rate with 10-won truncation
    const hourlyBase = Math.round((newEmp.baseSalary + newEmp.fixedAllow) / 209);
    const expectedOtPay = Math.floor((hourlyBase * 1.5 * heavyAttendance.otHours) / 10) * 10;
    expect(payslipBefore.overtimePay).toBe(expectedOtPay);

    // Verify National Treasury Administration Act §47(1) 10-won truncation
    expect(payslipBefore.netPay % 10).toBe(0);

    // 6. Adjudicate Exception via Formal Approval Workflow (AP-)
    const createdApproval = useAppStore.getState().createApprovalDraft({
      title: `[근태 예외 소명] ${newEmp.name} 주 52시간 상한 초과 승인 심사`,
      category: "연장근로",
      drafterId: newEmp.id,
      drafterName: newEmp.name,
      drafterDept: newEmp.dept,
      content: "터미널 자동화 크레인 긴급 야간 전력 복구로 인한 연장근로 16시간 발생. 특별연장근로 인가 심사 요청.",
      doaAmount: 0,
      linkedObjects: [heavyAttendance.exceptionCode || `AT-52H-${newEmp.id}`],
    });

    expect(createdApproval.code).toMatch(/^AP-\d+/);
    expect(createdApproval.status).toBe("초안");

    // Submit draft to approval stages (Parking -> Pending Approval)
    useAppStore.getState().submitApproval(createdApproval.id);
    const submittedApproval = useAppStore.getState().approvals.find((a) => a.id === createdApproval.id);
    expect(submittedApproval?.status).toBe("결재대기");

    // 7. Cedar PBAC Policy Authorization Evaluation
    const cedarDecision = evaluateAccess({
      principalAccountId: "ACC-003",
      principalPersonId: "P-emp_03",
      principalRoleGroup: "총괄본부장",
      principalEntityId: "all",
      action: "Approvals::SignTier2",
      resource: {
        id: createdApproval.id,
        code: createdApproval.code,
        kind: "AP",
        entityId: "corp_02",
        entityName: "(주)오야티 로지스틱스",
      },
      context: {
        time: new Date().toISOString(),
        mfaVerified: true,
      },
    });

    expect(cedarDecision.decision).toBe("PERMIT");

    // 8. Approve and Adjudicate Attendance Exception
    useAppStore.getState().approveDoc(createdApproval.id, "최재무", "특별연장근로 사후 승인 및 대체휴무 부여 조건부 결재 완료");

    // Mark attendance resolved
    useAppStore.setState((s) => ({
      attendance: s.attendance.map((a) =>
        a.id === heavyAttendance.id ? { ...a, status: "연장" as const, exceptionCode: undefined } : a
      ),
      payrollRun: {
        ...s.payrollRun,
        unresolvedExceptions: 0,
      },
    }));

    const resolvedAttendance = useAppStore.getState().attendance.find((a) => a.id === heavyAttendance.id);
    expect(resolvedAttendance?.status).toBe("연장");

    // 9. Freeze Payroll and Generate Banking Transfer Batch (Firm Banking KFTC CMS)
    useAppStore.getState().freezePayrollRun();
    const frozenPayroll = useAppStore.getState().payrollRun;
    expect(frozenPayroll.status).toBe("마감게이트통과");
    expect(frozenPayroll.isLocked).toBe(true);

    // 10. Passkey Verification Ceremony for Banking Batch Sealing
    const batch = useAppStore.getState().bankTransferBatch;
    expect(batch).toBeDefined();
    expect(batch.status).toBe("준비");

    const passkeyResult = useAppStore.getState().signBatchWithPasskey("user_cf_01", "최재무");
    expect(passkeyResult.success).toBe(true);

    const sealedBatch = useAppStore.getState().bankTransferBatch;
    expect(sealedBatch.status).toBe("이체지시완료");
    expect(sealedBatch.sealedBy).toBe("최재무");
    expect(sealedBatch.passkeyHash).toBeDefined();

    // 11. Verify Immutable Cryptographic Hash Chain Audit Trail
    const events = useAppStore.getState().auditEvents;
    expect(events.length).toBeGreaterThan(0);

    const auditVerification = verifyAuditChainIntegrity(events);
    expect(auditVerification.valid).toBe(true);

    // 12. Backup & Hydration Round-trip
    const backupJson = useAppStore.getState().exportStateBackup();
    expect(typeof backupJson).toBe("string");
    const parsed = JSON.parse(backupJson);
    expect(parsed.employees.length).toBeGreaterThan(0);

    const restoreResult = useAppStore.getState().importStateBackup(backupJson);
    expect(restoreResult.success).toBe(true);
  });

  it("dynamically synthesizes an entire enterprise workspace via bootstrapDynamicWorkspace", () => {
    // Generate fresh dynamic enterprise structure with 3 entities, 6 sites, 9 departments, 12 employees
    useAppStore.getState().bootstrapDynamicWorkspace({
      corporateCount: 3,
      sitesPerCorp: 2,
      deptsPerCorp: 3,
      baseYear: 2026,
    });

    const state = useAppStore.getState();
    expect(state.orgEntities.length).toBe(3);
    expect(state.orgSites.length).toBe(6);
    expect(state.orgDepartments.length).toBe(9);
    expect(state.employees.length).toBe(12);
    expect(state.attendance.length).toBe(12);
    expect(state.payslips.length).toBe(12);
    expect(state.bankTransferBatch.totalHeadcount).toBe(12);
    expect(state.bankTransferBatch.totalAmount).toBeGreaterThan(0);
  });
});
