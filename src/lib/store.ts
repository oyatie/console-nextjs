import { create } from "zustand";
import {
  SEED_ENTITIES,
  SEED_SITES,
  SEED_DEPARTMENTS,
  SEED_POLICY,
  SEED_CONTRACTS,
  SEED_EQUIPMENT,
  SEED_PERSONNEL_ACTIONS,
  SEED_BANK_BATCH,
  SEED_EMPLOYEES,
  SEED_ATTENDANCE,
  SEED_APPROVALS,
  SEED_WORK_ORDERS,
  SEED_PAYROLL_RUN,
  SEED_AUDIT_EVENTS,
  SEED_THREADS,
  SEED_MESSAGES,
  SEED_LEAVE_NOTICES,
  SEED_DISCREPANCIES,
  SEED_EMAILS,
} from "./seed-data";
import {
  Employee,
  AttendanceRecord,
  ApprovalDoc,
  WorkOrder,
  PayrollRun,
  Payslip,
  AuditEvent,
  CommThread,
  CommMessage,
  LeaveNotice,
  ToastMessage,
  OrgEntity,
  OrgSite,
  OrgDepartment,
  OrgPolicy,
  Contract,
  Equipment,
  PersonnelAction,
  PersonnelActionType,
  BankTransferBatch,
  PayrollDiscrepancy,
  PayrollDraftState,
  CedarPolicyStatement,
  CapabilityBundle,
  AccessGrantAssignment,
  OperationalIncident,
  RoleDefinition,
  RoleAssignment,
  EffectivePermissionFold,
  EmailItem,
  ObjectKind,
} from "./types";
import { computeEmployeePayslip } from "./payroll-engine";
import { calculateWorkHours, evaluate52HourCompliance } from "./attendance-calculator";
import {
  DEFAULT_CEDAR_POLICIES,
  CAPABILITY_BUNDLES,
  INITIAL_ACCESS_ASSIGNMENTS,
  INITIAL_OPERATIONAL_INCIDENTS,
  DEFAULT_ROLE_DEFINITIONS,
  INITIAL_ROLE_ASSIGNMENTS,
  foldUserEffectivePermissions,
} from "./policy-engine";
import {
  generateDynamicOrganization,
  generateDynamicWorkforce,
  deriveDynamicAttendance,
  processDynamicPayrollRun,
  DynamicOrgConfig,
  calculateEventHash,
  generateInitialAuditChain,
} from "./dynamic-factory";

// Cryptographically linked audit chain recorder
function recordStoreAuditEvent(
  currentEvents: AuditEvent[],
  data: {
    actorId: string;
    actorName: string;
    action: string;
    targetKind: ObjectKind;
    targetCode: string;
    policyDecision?: AuditEvent["policyDecision"];
    reason?: string;
    dataClass?: AuditEvent["dataClass"];
  }
): AuditEvent[] {
  const latest = currentEvents[0];
  const prevHash = latest ? latest.hash : "0000000000000000000000000000000000000000000000000000000000000000";
  const seq = (latest?.seq || 10840) + 1;
  const id = `au_${Date.now()}_${seq}`;
  const timestamp = new Date().toISOString().replace("T", " ").slice(0, 19);

  const raw: Omit<AuditEvent, "hash"> = {
    id,
    seq,
    prevHash,
    timestamp,
    actorId: data.actorId,
    actorName: data.actorName,
    action: data.action,
    targetKind: data.targetKind,
    targetCode: data.targetCode,
    policyDecision: data.policyDecision || "PERMIT",
    reason: data.reason,
    dataClass: data.dataClass || "일반",
  };

  const hash = calculateEventHash(raw);
  const newEvent: AuditEvent = { ...raw, hash };
  return [newEvent, ...currentEvents];
}

const INITIAL_AUDIT_CHAIN: AuditEvent[] = generateInitialAuditChain().reverse();

// 초기 시드 사원에 대한 기준 급여명세서 사전 계산
const INITIAL_PAYSLIPS: Payslip[] = SEED_EMPLOYEES.map((emp) => {
  const att = SEED_ATTENDANCE.find((a) => a.employeeId === emp.id);
  return computeEmployeePayslip(emp, {
    otHours: att?.otHours || 0,
    nightHours: att?.nightHours || 0,
    holidayHours: att?.holHours || 0,
  });
});

export interface AppState {
  // Navigation & Shell
  activeEntityId: string;
  activeSiteId: string;
  viewAsRole: string;
  sidebarCollapsed: boolean;
  rightRailOpen: boolean;
  rightRailTab: "thread" | "chat" | "notices" | "inspector";
  activeObjectCode: string | null;
  commandPaletteOpen: boolean;
  theme: "light" | "dark";

  // Data Collections
  employees: Employee[];
  attendance: AttendanceRecord[];
  approvals: ApprovalDoc[];
  workOrders: WorkOrder[];
  payrollRun: PayrollRun;
  payslips: Payslip[];
  auditEvents: AuditEvent[];
  threads: CommThread[];
  messages: Record<string, CommMessage[]>;
  leaveNotices: LeaveNotice[];
  emails: EmailItem[];
  toasts: ToastMessage[];
  shortcutModalOpen: boolean;

  // Organization, Policy, Operations Collections
  orgEntities: OrgEntity[];
  orgSites: OrgSite[];
  orgDepartments: OrgDepartment[];
  orgPolicy: OrgPolicy;
  contracts: Contract[];
  equipment: Equipment[];
  personnelActions: PersonnelAction[];
  bankTransferBatch: BankTransferBatch;
  discrepancies: PayrollDiscrepancy[];
  payrollDraft: PayrollDraftState | null;

  // Discord-Style Multi-Layered Role Engine
  roleDefinitions: RoleDefinition[];
  roleAssignments: RoleAssignment[];
  activeActingCapacityId: string | null;

  // Cedar Policy & Governance Collections
  cedarPolicies: CedarPolicyStatement[];
  accessAssignments: AccessGrantAssignment[];
  capabilityBundles: CapabilityBundle[];
  operationalIncidents: OperationalIncident[];

  // Shell Actions
  setActiveEntityId: (id: string) => void;
  setActiveSiteId: (id: string) => void;
  setViewAsRole: (role: string) => void;
  toggleSidebar: () => void;
  toggleRightRail: (tab?: "thread" | "chat" | "notices" | "inspector") => void;
  openObjectInspector: (code: string) => void;
  closeObjectInspector: () => void;
  setCommandPaletteOpen: (open: boolean) => void;
  setShortcutModalOpen: (open: boolean) => void;
  toggleTheme: () => void;
  addToast: (toast: Omit<ToastMessage, "id">) => void;
  removeToast: (id: string) => void;
  resetToSeedData: () => void;

  // Domain Actions
  createEmployee: (emp: Omit<Employee, "id" | "code">) => Employee;
  updateEmployee: (id: string, updates: Partial<Employee>) => void;
  transferEmployee: (employeeId: string, targetDept: string, targetSite: string, effectiveDate: string, reason: string) => PersonnelAction;
  createApprovalDraft: (doc: Omit<ApprovalDoc, "id" | "code" | "createdAt" | "status" | "stages">) => ApprovalDoc;
  submitApproval: (id: string) => void;
  approveDoc: (
    id: string,
    approverName: string,
    comment?: string,
    capacity?: {
      actingCapacityRole?: string;
      actingCapacityName?: string;
      authorizingGrantId?: string;
      passkeyVerified?: boolean;
    }
  ) => void;
  rejectDoc: (id: string, approverName: string, comment: string) => void;
  closeDoc: (id: string) => void;
  
  createWorkOrder: (wo: Omit<WorkOrder, "id" | "code" | "status">) => WorkOrder;
  updateWorkOrderStatus: (id: string, status: WorkOrder["status"]) => void;

  updateAttendancePunch: (id: string, clockIn: string, clockOut: string, breakMinutes: number) => void;
  adjudicateAttendanceException: (
    attendanceId: string,
    resolution: "approved" | "corrected" | "rejected",
    note: string,
    newHours?: number
  ) => void;

  // Discrepancy & Draft Actions
  repairDiscrepancy: (id: string, resolutionComment: string) => void;
  savePayrollDraft: (draft: Partial<PayrollDraftState>) => void;
  resumePayrollDraft: () => PayrollDraftState | null;

  calculateAndFreezePayroll: () => { success: boolean; error?: string };
  sendMessage: (threadId: string, text: string) => void;
  createThreadForObject: (objectRef: string, title: string) => CommThread;
  createThread: (payload: { title: string; objectRef?: string; initialMessage?: string }) => CommThread;
  pasteExcelEmployees: (newRows: Partial<Employee>[]) => number;
  sendLeaveNotice: (id: string) => void;
  signLeaveNotice: (id: string) => void;
  createLeaveNotice: (payload: { employeeId: string; round: "1차" | "2차"; remainingDays: number; deadline: string }) => LeaveNotice;

  // Enterprise Mail Actions
  sendEmail: (payload: {
    recipient: string;
    recipientEmail: string;
    subject: string;
    body: string;
    linkedObject?: string;
  }) => EmailItem;
  archiveEmail: (id: string) => void;
  deleteEmail: (id: string) => void;
  markEmailAsRead: (id: string) => void;
  createApprovalDraftFromEmail: (emailId: string) => ApprovalDoc | null;

  // Organization & Setup Actions
  createOrgEntity: (entity: Omit<OrgEntity, "id">) => OrgEntity;
  updateOrgEntity: (id: string, updates: Partial<OrgEntity>) => void;
  createOrgSite: (site: Omit<OrgSite, "id">) => OrgSite;
  updateOrgSite: (id: string, updates: Partial<OrgSite>) => void;
  createDepartment: (dept: Omit<OrgDepartment, "id">) => OrgDepartment;
  updateDepartment: (id: string, updates: Partial<OrgDepartment>) => void;
  updateOrgPolicy: (policy: Partial<OrgPolicy>) => void;
  createContract: (contract: Omit<Contract, "id" | "code">) => Contract;
  updateContract: (id: string, updates: Partial<Contract>) => void;
  createEquipment: (eq: Omit<Equipment, "id" | "code">) => Equipment;
  updateEquipmentStatus: (id: string, status: Equipment["status"]) => void;

  // HR Actions Processing
  processPersonnelAction: (input: {
    employeeId: string;
    actionType: PersonnelActionType;
    effectiveDate: string;
    reason: string;
    updates: {
      dept?: string;
      role?: string;
      site?: string;
      baseSalary?: number;
      status?: Employee["status"];
    };
  }) => PersonnelAction;

  // Bank Firm Banking & Transfer Dispatch
  generateFirmBankingBatch: () => BankTransferBatch;
  sealAndDispatchBankBatch: (approverName: string, passkeyHash?: string) => void;
  freezePayrollRun: () => void;
  signBatchWithPasskey: (userId: string, approverName: string) => { success: boolean; passkeyHash?: string };

  // Cedar Policy & Access Governance Actions
  addCedarPolicy: (policy: CedarPolicyStatement) => void;
  removeCedarPolicy: (id: string) => void;
  grantAccessAssignment: (assignment: Omit<AccessGrantAssignment, "id" | "grantedAt" | "status">) => AccessGrantAssignment;
  revokeAccessAssignment: (id: string, reason?: string) => void;
  resolveOperationalIncident: (id: string, resolutionNote: string) => void;
  reassignApprovalDelegate: (docCode: string, fromPersonId: string, toPersonId: string, reason: string) => { success: boolean; message: string };

  // Discord-Style Layered Role Actions
  assignRoleToEmployee: (
    employeeIdOrPayload:
      | string
      | {
          employeeId: string;
          roleId: string;
          scope?: RoleAssignment["scope"];
          grantReason?: string;
          reason?: string;
        },
    roleId?: string,
    scope?: RoleAssignment["scope"],
    reason?: string
  ) => RoleAssignment;
  removeRoleFromEmployee: (assignmentId: string) => void;
  createRoleDefinition: (role: Omit<RoleDefinition, "id">) => RoleDefinition;
  updateRoleDefinition: (id: string, updates: Partial<RoleDefinition>) => void;
  reorderRoles: (orderedIds: string[]) => void;
  setActiveActingCapacityId: (roleId: string | null) => void;
  getEffectivePermissionsForEmployee: (
    employeeId: string,
    targetScope?: { entityId?: string; siteId?: string; deptId?: string }
  ) => EffectivePermissionFold;

  // Enterprise Backup, State Durability & Dynamic Derivation
  exportStateBackup: () => string;
  importStateBackup: (jsonString: string) => { success: boolean; message: string };
  bootstrapDynamicWorkspace: (config?: DynamicOrgConfig) => void;
}

export const useAppStore = create<AppState>((set, get) => ({
  activeEntityId: "all",
  activeSiteId: "all",
  viewAsRole: "인사·노무관리자",
  sidebarCollapsed: false,
  rightRailOpen: true,
  rightRailTab: "thread",
  activeObjectCode: "AP-3121",
  commandPaletteOpen: false,
  theme: "light",

  employees: SEED_EMPLOYEES,
  attendance: SEED_ATTENDANCE,
  approvals: SEED_APPROVALS,
  workOrders: SEED_WORK_ORDERS,
  payrollRun: SEED_PAYROLL_RUN,
  payslips: INITIAL_PAYSLIPS,
  auditEvents: INITIAL_AUDIT_CHAIN,
  threads: SEED_THREADS,
  messages: SEED_MESSAGES,
  leaveNotices: SEED_LEAVE_NOTICES,
  emails: SEED_EMAILS,
  toasts: [],
  shortcutModalOpen: false,

  orgEntities: SEED_ENTITIES,
  orgSites: SEED_SITES,
  orgDepartments: SEED_DEPARTMENTS,
  orgPolicy: SEED_POLICY,
  contracts: SEED_CONTRACTS,
  equipment: SEED_EQUIPMENT,
  personnelActions: SEED_PERSONNEL_ACTIONS,
  bankTransferBatch: SEED_BANK_BATCH,
  discrepancies: SEED_DISCREPANCIES,
  payrollDraft: null,

  cedarPolicies: DEFAULT_CEDAR_POLICIES,
  accessAssignments: INITIAL_ACCESS_ASSIGNMENTS,
  capabilityBundles: CAPABILITY_BUNDLES,
  operationalIncidents: INITIAL_OPERATIONAL_INCIDENTS,

  roleDefinitions: DEFAULT_ROLE_DEFINITIONS,
  roleAssignments: INITIAL_ROLE_ASSIGNMENTS,
  activeActingCapacityId: null,

  setActiveEntityId: (id) => set({ activeEntityId: id }),
  setActiveSiteId: (id) => set({ activeSiteId: id }),
  setViewAsRole: (role) => set({ viewAsRole: role }),
  toggleSidebar: () => set((s) => ({ sidebarCollapsed: !s.sidebarCollapsed })),
  toggleRightRail: (tab) =>
    set((s) => ({
      rightRailOpen: tab ? true : !s.rightRailOpen,
      rightRailTab: tab || s.rightRailTab,
    })),
  openObjectInspector: (code) => {
    // 스레드 존재 여부 확인 후 자동 연결
    const existingThread = get().threads.find((t) => t.objectRef === code);
    if (!existingThread) {
      get().createThreadForObject(code, `${code} 맥락 협업 스레드`);
    }
    set({
      activeObjectCode: code,
      rightRailOpen: true,
      rightRailTab: "inspector",
    });
  },
  closeObjectInspector: () => set({ activeObjectCode: null }),
  setCommandPaletteOpen: (open) => set({ commandPaletteOpen: open }),
  setShortcutModalOpen: (open) => set({ shortcutModalOpen: open }),
  addToast: (toast) =>
    set((s) => ({
      toasts: [
        ...s.toasts,
        {
          ...toast,
          id: `t_${Date.now()}_${Math.random().toString(16).slice(2, 6)}`,
        },
      ],
    })),
  removeToast: (id) =>
    set((s) => ({
      toasts: s.toasts.filter((t) => t.id !== id),
    })),
  resetToSeedData: () => {
    set({
      employees: SEED_EMPLOYEES,
      attendance: SEED_ATTENDANCE,
      approvals: SEED_APPROVALS,
      workOrders: SEED_WORK_ORDERS,
      payrollRun: SEED_PAYROLL_RUN,
      payslips: INITIAL_PAYSLIPS,
      auditEvents: INITIAL_AUDIT_CHAIN,
      threads: SEED_THREADS,
      messages: SEED_MESSAGES,
      leaveNotices: SEED_LEAVE_NOTICES,
      emails: SEED_EMAILS,
      orgEntities: SEED_ENTITIES,
      orgSites: SEED_SITES,
      orgDepartments: SEED_DEPARTMENTS,
      orgPolicy: SEED_POLICY,
      contracts: SEED_CONTRACTS,
      equipment: SEED_EQUIPMENT,
      personnelActions: SEED_PERSONNEL_ACTIONS,
      bankTransferBatch: SEED_BANK_BATCH,
      discrepancies: SEED_DISCREPANCIES,
      payrollDraft: null,
      cedarPolicies: DEFAULT_CEDAR_POLICIES,
      accessAssignments: INITIAL_ACCESS_ASSIGNMENTS,
      capabilityBundles: CAPABILITY_BUNDLES,
      operationalIncidents: INITIAL_OPERATIONAL_INCIDENTS,
      roleDefinitions: DEFAULT_ROLE_DEFINITIONS,
      roleAssignments: INITIAL_ROLE_ASSIGNMENTS,
      activeActingCapacityId: null,
    });
    get().addToast({
      title: "데이터 초기화 완료",
      description: "시연용 화면 데이터를 초기 상태로 되돌렸습니다.",
      tone: "info",
    });
  },
  bootstrapDynamicWorkspace: (config?: DynamicOrgConfig) => {
    const org = generateDynamicOrganization(config);
    const employees = generateDynamicWorkforce(org, 12);
    const attendance = deriveDynamicAttendance(employees, "2026-07");
    const { run, payslips, batch } = processDynamicPayrollRun(employees, attendance, "2026-07");

    set({
      orgEntities: org.entities,
      orgSites: org.sites,
      orgDepartments: org.departments,
      employees,
      attendance,
      payrollRun: run,
      payslips,
      bankTransferBatch: batch,
      auditEvents: generateInitialAuditChain().reverse(),
      activeEntityId: org.entities[0]?.id || "dyn_corp_1",
      activeSiteId: org.sites[0]?.id || "dyn_site_1_1",
      payrollDraft: null,
    });

    get().addToast({
      title: "동적 엔터프라이즈 그래프 합성 완료",
      description: `${org.entities.length}개 법인, ${org.sites.length}개 사업장, ${employees.length}명 임직원 및 7월 정산원장이 실시간 동적으로 도출되었습니다.`,
      tone: "ok",
    });
  },
  toggleTheme: () =>
    set((s) => {
      const next = s.theme === "light" ? "dark" : "light";
      if (typeof document !== "undefined") {
        document.documentElement.classList.toggle("dark", next === "dark");
      }
      return { theme: next };
    }),

  createEmployee: (empInput) => {
    const existing = get().employees;
    const nextCode = `EMP-${1000 + existing.length + 1}`;
    const newEmp: Employee = {
      ...empInput,
      id: `emp_${Date.now()}`,
      code: nextCode,
    };

    // 신규 사원의 당월 근태 레코드 자동 초기화
    const newAtt: AttendanceRecord = {
      id: `att_${newEmp.id}`,
      employeeId: newEmp.id,
      employeeName: newEmp.name,
      entity: newEmp.entity,
      site: newEmp.site,
      date: "2026-07-03",
      clockIn: "09:00",
      clockOut: "18:00",
      breakMinutes: 60,
      plannedIn: "09:00",
      plannedOut: "18:00",
      workedHours: 8.0,
      otHours: 0,
      nightHours: 0,
      holHours: 0,
      weeklyHoursTotal: 40.0,
      status: "정상",
    };

    set((s) => ({
      employees: [newEmp, ...s.employees],
      attendance: [newAtt, ...s.attendance],
      auditEvents: recordStoreAuditEvent(s.auditEvents, {
        actorId: "hr_admin",
        actorName: get().viewAsRole,
        action: "EMPLOYEE_ONBOARDED",
        targetCode: nextCode,
        targetKind: "P",
        policyDecision: "PERMIT",
        reason: `신규 사원 등록 — ${newEmp.name} (${newEmp.dept} ${newEmp.position})`,
        dataClass: "민감",
      }),
      payrollRun: {
        ...s.payrollRun,
        headcount: s.payrollRun.headcount + 1,
      },
    }));

    get().addToast({
      title: "신규 사원 입사 등록 완료",
      description: `${newEmp.name} (${newEmp.dept}) 사번 ${nextCode} 발급 및 당월 근태 자동 편성`,
      tone: "ok",
    });

    return newEmp;
  },

  updateEmployee: (id, updates) => {
    set((s) => ({
      employees: s.employees.map((e) => (e.id === id ? { ...e, ...updates } : e)),
    }));
    get().addToast({
      title: "사원 정보 갱신 완료",
      description: `사원 인적/직무/급여 정보가 성공적으로 업데이트되었습니다.`,
      tone: "ok",
    });
  },

  createApprovalDraft: (docInput) => {
    const num = Math.floor(3120 + Math.random() * 800);
    const code = `AP-${num}`;
    const newDoc: ApprovalDoc = {
      ...docInput,
      id: `appr_${Date.now()}`,
      code,
      createdAt: new Date().toISOString().replace("T", " ").slice(0, 16),
      status: "초안",
      stages: [
        { step: 1, approverRole: "부서장", approverName: "부서장 심사", status: "pending" },
        { step: 2, approverRole: "총괄임원", approverName: "최종 인가", status: "pending" },
      ],
    };

    set((s) => ({
      approvals: [newDoc, ...s.approvals],
      auditEvents: recordStoreAuditEvent(s.auditEvents, {
        actorId: docInput.drafterId,
        actorName: docInput.drafterName,
        action: "APPROVAL_DRAFT_PARKED",
        targetCode: code,
        targetKind: "AP",
        policyDecision: "PERMIT",
        reason: `문서 파킹 (SAP Document Parking) — ${docInput.title}`,
        dataClass: "일반",
      }),
    }));

    return newDoc;
  },

  submitApproval: (id) => {
    set((s) => ({
      approvals: s.approvals.map((a) =>
        a.id === id ? { ...a, status: "결재대기" } : a
      ),
    }));
  },

  approveDoc: (id, approverName, comment, capacity) => {
    set((s) => ({
      approvals: s.approvals.map((a) => {
        if (a.id !== id) return a;
        // 첫 번째 대기 중인 단계 찾기
        const pendingIdx = a.stages.findIndex((st) => st.status === "pending");
        if (pendingIdx === -1) return a;

        const updatedStages = a.stages.map((st, idx) =>
          idx === pendingIdx
            ? {
                ...st,
                status: "approved" as const,
                approverName,
                comment: comment || "승인 완료",
                timestamp: new Date().toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" }),
                actingCapacityRole: capacity?.actingCapacityRole,
                actingCapacityName: capacity?.actingCapacityName,
                authorizingGrantId: capacity?.authorizingGrantId,
                passkeyVerified: capacity?.passkeyVerified,
              }
            : st
        );

        const allApproved = updatedStages.every((st) => st.status === "approved");
        return {
          ...a,
          stages: updatedStages,
          status: allApproved ? "승인완료" : "결재대기",
        };
      }),
    }));

    get().addToast({
      title: "결재 승인 완료",
      description: `${approverName} 님이 ${capacity?.actingCapacityName ? `[${capacity.actingCapacityName}] 자격으로 ` : ""}결재를 승인하였습니다.`,
      tone: "ok",
    });
  },

  rejectDoc: (id, approverName, comment) => {
    set((s) => ({
      approvals: s.approvals.map((a) => {
        if (a.id !== id) return a;
        const pendingIdx = a.stages.findIndex((st) => st.status === "pending");
        return {
          ...a,
          status: "반려",
          stages: a.stages.map((st, idx) =>
            idx === (pendingIdx >= 0 ? pendingIdx : 0)
              ? {
                  ...st,
                  status: "rejected" as const,
                  approverName,
                  comment,
                  timestamp: new Date().toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" }),
                }
              : st
          ),
        };
      }),
    }));

    get().addToast({
      title: "결재 반려 처리",
      description: `${approverName} 님이 결재를 반려하였습니다: ${comment}`,
      tone: "danger",
    });
  },

  closeDoc: (id) => {
    set((s) => ({
      approvals: s.approvals.map((a) =>
        a.id === id ? { ...a, status: "종결" } : a
      ),
    }));
  },

  createWorkOrder: (woInput) => {
    const num = Math.floor(2640 + Math.random() * 500);
    const code = `WO-${num}`;
    const newWo: WorkOrder = {
      ...woInput,
      id: `wo_${Date.now()}`,
      code,
      status: "접수",
    };

    set((s) => ({
      workOrders: [newWo, ...s.workOrders],
      auditEvents: recordStoreAuditEvent(s.auditEvents, {
        actorId: "ops_manager",
        actorName: get().viewAsRole,
        action: "WORK_ORDER_DISPATCHED",
        targetCode: code,
        targetKind: "WO",
        policyDecision: "PERMIT",
        reason: `신규 현장 작업오더 발행 — ${newWo.title}`,
        dataClass: "일반",
      }),
    }));

    get().addToast({
      title: "현장 작업오더 발행",
      description: `${code} ${newWo.title} (담당: ${newWo.assignedTo})`,
      tone: "ok",
    });

    return newWo;
  },

  updateWorkOrderStatus: (id, status) => {
    set((s) => ({
      workOrders: s.workOrders.map((w) =>
        w.id === id ? { ...w, status } : w
      ),
    }));

    get().addToast({
      title: "작업오더 상태 전이",
      description: `작업 상태가 '${status}'(으)로 갱신되었습니다.`,
      tone: "ok",
    });
  },

  updateAttendancePunch: (id, clockIn, clockOut, breakMinutes) => {
    const punch = calculateWorkHours(clockIn, clockOut, breakMinutes);
    const workedHours = punch.netWorkHours ?? 0;
    const otHours = Math.max(0, Math.round((workedHours - 8) * 10) / 10);

    set((s) => ({
      attendance: s.attendance.map((att) => {
        if (att.id !== id) return att;
        return {
          ...att,
          clockIn,
          clockOut,
          breakMinutes,
          workedHours,
          otHours,
          status: workedHours > 8 ? "연장" : "정상",
        };
      }),
    }));
  },

  adjudicateAttendanceException: (attendanceId, resolution, note, newHours) => {
    set((s) => {
      const updatedAtt = s.attendance.map((att) => {
        if (att.id !== attendanceId) return att;
        const finalHours = newHours !== undefined ? newHours : att.workedHours;
        return {
          ...att,
          workedHours: finalHours,
          otHours: Math.max(0, Math.round((finalHours - 8) * 10) / 10),
          status: resolution === "rejected" ? ("미출근" as const) : ("정상" as const),
          exceptionCode: undefined, // 예외 해제!
        };
      });

      const unresolvedCount = updatedAtt.filter((a) => a.exceptionCode).length;

      return {
        attendance: updatedAtt,
        payrollRun: {
          ...s.payrollRun,
          unresolvedExceptions: unresolvedCount,
        },
        auditEvents: recordStoreAuditEvent(s.auditEvents, {
          actorId: "hr_admin",
          actorName: s.viewAsRole,
          action: "ATTENDANCE_EXCEPTION_ADJUDICATED",
          targetCode: attendanceId,
          targetKind: "AT",
          policyDecision: resolution === "rejected" ? "DENY" : "PERMIT",
          reason: `근태 예외 심사 [${resolution}] — ${note}`,
          dataClass: "민감",
        }),
      };
    });

    get().addToast({
      title: resolution === "approved" ? "근태 예외 소명 인가" : "근태 예외 반려",
      description: `예외 심사 결과가 반영되어 급여 마감 게이트에 즉시 연동되었습니다.`,
      tone: resolution === "approved" ? "ok" : "warn",
    });
  },

  calculateAndFreezePayroll: () => {
    const state = get();
    if (state.payrollRun.unresolvedExceptions > 0) {
      return {
        success: false,
        error: `미해결된 근태 예외 ${state.payrollRun.unresolvedExceptions}건이 존재합니다. 근태 마감 게이트를 먼저 통과해야 급여 마감이 가능합니다 (Workday/Monday 정책 준용).`,
      };
    }

    let totalGross = 0;
    let totalDeductions = 0;
    let totalNet = 0;

    // 모든 활성 사원에 대해 실제 급여명세서 실시간 계산 및 저장
    const calculatedPayslips: Payslip[] = state.employees.map((emp) => {
      const attRecord = state.attendance.find((a) => a.employeeId === emp.id);
      const slip = computeEmployeePayslip(emp, {
        otHours: attRecord?.otHours || 0,
        nightHours: attRecord?.nightHours || 0,
        holidayHours: attRecord?.holHours || 0,
      });
      totalGross += slip.grossPay;
      totalDeductions += slip.totalDeductions;
      totalNet += slip.netPay;
      return slip;
    });

    set((s) => ({
      payslips: calculatedPayslips,
      auditEvents: recordStoreAuditEvent(s.auditEvents, {
        actorId: "payroll_admin",
        actorName: state.viewAsRole,
        action: "PAYROLL_CALCULATED_AND_FROZEN",
        targetCode: state.payrollRun.code,
        targetKind: "PR",
        policyDecision: "PERMIT",
        reason: `2026년 7월 급여 전사 확정 마감 (인원: ${state.employees.length}명, 실지급총액: ${totalNet.toLocaleString()}원)`,
        dataClass: "민감",
      }),
      payrollRun: {
        ...s.payrollRun,
        status: "마감게이트통과",
        totalGross,
        totalDeductions,
        totalNet,
        isLocked: true,
      },
    }));

    get().addToast({
      title: "정기 급여 마감 및 확정",
      description: `${calculatedPayslips.length}명 전원 10원절사 및 4대보험 법정 급여명세서가 확정 동결되었습니다.`,
      tone: "ok",
    });

    return { success: true };
  },

  createThreadForObject: (objectRef, title) => {
    const newThread: CommThread = {
      id: `th_${Date.now()}`,
      objectRef,
      title: `${objectRef} [${title}]`,
      lastMessage: "스레드가 생성되었습니다.",
      updatedAt: "방금 전",
      unread: false,
      participantNames: ["박지영", "담당자"],
    };

    set((s) => ({
      threads: [newThread, ...s.threads],
      messages: {
        ...s.messages,
        [newThread.id]: [
          {
            id: `msg_init_${Date.now()}`,
            threadId: newThread.id,
            authorId: "system",
            authorName: "시스템 알림",
            avatarText: "알",
            text: `${objectRef} 개체에 대한 전용 협업 스레드가 개설되었습니다.`,
            timestamp: new Date().toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" }),
            linkedCodes: [objectRef],
          },
        ],
      },
    }));

    return newThread;
  },

  createThread: ({ title, objectRef, initialMessage }) => {
    const newThreadId = `th_${Date.now()}`;
    const newThread: CommThread = {
      id: newThreadId,
      objectRef,
      title: title || (objectRef ? `${objectRef} 맥락 협업 스레드` : "신규 업무 채널"),
      lastMessage: initialMessage || "채널이 개설되었습니다.",
      updatedAt: "방금 전",
      unread: false,
      participantNames: ["박지영 수석", "최원석 대표", "황도현 수석"],
    };

    const firstMsg: CommMessage = {
      id: `msg_${Date.now()}`,
      threadId: newThreadId,
      authorId: "current_user",
      authorName: "박지영 수석",
      avatarText: "박",
      text: initialMessage || (objectRef ? `[${objectRef}]에 관한 업무 스레드가 개설되었습니다.` : "새로운 업무 채널이 개설되었습니다."),
      timestamp: new Date().toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" }),
      linkedCodes: objectRef ? [objectRef] : undefined,
    };

    set((s) => ({
      threads: [newThread, ...s.threads],
      messages: {
        ...s.messages,
        [newThreadId]: [firstMsg],
      },
    }));

    get().addToast({
      title: "협업 채널 생성 완료",
      description: `'${newThread.title}' 채널이 개설되었습니다.`,
      tone: "ok",
    });

    return newThread;
  },

  sendMessage: (threadId, text) => {
    const newMsg: CommMessage = {
      id: `msg_${Date.now()}`,
      threadId,
      authorId: "current_user",
      authorName: "박지영 (나)",
      avatarText: "박",
      text,
      timestamp: new Date().toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" }),
    };

    set((s) => ({
      messages: {
        ...s.messages,
        [threadId]: [...(s.messages[threadId] || []), newMsg],
      },
      threads: s.threads.map((t) =>
        t.id === threadId
          ? { ...t, lastMessage: text, updatedAt: "방금 전" }
          : t
      ),
    }));
  },

  pasteExcelEmployees: (newRows) => {
    const existing = get().employees;
    const added: Employee[] = newRows.map((r, i) => ({
      id: `emp_pasted_${Date.now()}_${i}`,
      code: r.code || `EMP-${1000 + existing.length + i + 1}`,
      name: r.name || `신규사원 ${i + 1}`,
      email: r.email || `new.${i + 1}@oyatie.com`,
      phone: r.phone || "010-0000-0000",
      entity: r.entity || "(주)오야티 로지스틱스",
      site: r.site || "인천 제1물류센터",
      dept: r.dept || "현장운영",
      role: r.role || "사원",
      position: r.position || "물류운영",
      empType: (r.empType as any) || "정규",
      baseSalary: Number(r.baseSalary) || 3200000,
      fixedAllow: Number(r.fixedAllow) || 200000,
      joinedDate: r.joinedDate || "2026-07-01",
      status: "재직",
      dependents: Number(r.dependents) || 1,
    }));

    // 신규 사원들 근태도 즉시 자동 초기화
    const newAtts: AttendanceRecord[] = added.map((e) => ({
      id: `att_${e.id}`,
      employeeId: e.id,
      employeeName: e.name,
      entity: e.entity,
      site: e.site,
      date: "2026-07-03",
      clockIn: "09:00",
      clockOut: "18:00",
      breakMinutes: 60,
      plannedIn: "09:00",
      plannedOut: "18:00",
      workedHours: 8.0,
      otHours: 0,
      nightHours: 0,
      holHours: 0,
      weeklyHoursTotal: 40.0,
      status: "정상",
    }));

    set((s) => ({
      employees: [...added, ...s.employees],
      attendance: [...newAtts, ...s.attendance],
      payrollRun: {
        ...s.payrollRun,
        headcount: s.payrollRun.headcount + added.length,
      },
    }));

    get().addToast({
      title: "엑셀 TSV 일괄 사원 등록 완료",
      description: `총 ${added.length}명의 신규 사원이 등록되었으며 당월 근태가 자동 편성되었습니다.`,
      tone: "ok",
    });

    return added.length;
  },

  sendLeaveNotice: (id) => {
    set((s) => ({
      leaveNotices: s.leaveNotices.map((n) =>
        n.id === id ? { ...n, status: "발송완료" as const } : n
      ),
    }));
    get().addToast({
      title: "연차 유급휴가 촉진 통보 발송",
      description: "근로기준법 제61조 법정 기한 내 전자 통보가 발송되었습니다.",
      tone: "info",
    });
  },

  signLeaveNotice: (id) => {
    const timestamp = new Date().toISOString().replace("T", " ").slice(0, 16);
    set((s) => ({
      leaveNotices: s.leaveNotices.map((n) =>
        n.id === id ? { ...n, status: "수령확인(Passkey)" as const, confirmedAt: timestamp } : n
      ),
    }));
    get().addToast({
      title: "Passkey 생체 인증 수령확인 완료",
      description: "FIDO2 암호학적 서명이 WORM 감사 체인에 영구 기록되었습니다.",
      tone: "ok",
    });
  },

  createLeaveNotice: ({ employeeId, round, remainingDays, deadline }) => {
    const emp = get().employees.find((e) => e.id === employeeId);
    const noticeCode = `LV-2026-${String(get().leaveNotices.length + 1).padStart(3, "0")}`;
    const newNotice: LeaveNotice = {
      id: `ln_${Date.now()}`,
      code: noticeCode,
      employeeId,
      employeeName: emp?.name || "사원",
      dept: emp?.dept || "미지정",
      round,
      remainingDays,
      deadline,
      status: "통지대기",
    };

    set((s) => ({
      leaveNotices: [newNotice, ...s.leaveNotices],
      auditEvents: recordStoreAuditEvent(s.auditEvents, {
        actorId: "hr_admin",
        actorName: get().viewAsRole,
        action: "STATUTORY_LEAVE_NOTICE_ISSUED",
        targetCode: noticeCode,
        targetKind: "OP",
        policyDecision: "PERMIT",
        reason: `근로기준법 제61조 연차유급휴가 사용촉진 ${round} 통지서 발행 (${newNotice.employeeName} 잔여: ${remainingDays}일)`,
        dataClass: "일반",
      }),
    }));

    get().addToast({
      title: "연차촉진 통지 생성",
      description: `${newNotice.employeeName} 사원에 대한 ${round} 통지서(${noticeCode})가 등록되었습니다.`,
      tone: "ok",
    });

    return newNotice;
  },

  sendEmail: (payload) => {
    const newMail: EmailItem = {
      id: `mail_${Date.now()}`,
      sender: "박지영 수석",
      senderEmail: "jy.park@oyatie.com",
      recipient: payload.recipient,
      recipientEmail: payload.recipientEmail,
      subject: payload.subject,
      preview: payload.body.slice(0, 80).replace(/\n/g, " "),
      date: "방금 전",
      isUnread: false,
      folder: "sent",
      linkedObject: payload.linkedObject,
      body: payload.body,
    };
    set((s) => ({
      emails: [newMail, ...s.emails],
    }));
    get().addToast({
      title: "메일 발송 완료",
      description: `'${payload.recipient}' (${payload.recipientEmail}) 수신처로 메일이 정상 발송되었습니다.`,
      tone: "ok",
    });
    return newMail;
  },

  archiveEmail: (id) => {
    set((s) => ({
      emails: s.emails.map((m) => (m.id === id ? { ...m, folder: "archive" as const } : m)),
    }));
    get().addToast({
      title: "메일 보관 완료",
      description: "선택한 메일이 보관함으로 이동되었습니다.",
      tone: "info",
    });
  },

  deleteEmail: (id) => {
    set((s) => ({
      emails: s.emails.map((m) => (m.id === id ? { ...m, folder: "trash" as const } : m)),
    }));
    get().addToast({
      title: "메일 삭제",
      description: "선택한 메일이 휴지통으로 이동되었습니다.",
      tone: "warn",
    });
  },

  markEmailAsRead: (id) => {
    set((s) => ({
      emails: s.emails.map((m) => (m.id === id ? { ...m, isUnread: false } : m)),
    }));
  },

  createApprovalDraftFromEmail: (emailId) => {
    const email = get().emails.find((m) => m.id === emailId);
    if (!email) return null;

    const draft = get().createApprovalDraft({
      title: `[메일연계] ${email.subject}`,
      category: "기타",
      drafterId: "emp_01",
      drafterName: "박지영 수석",
      drafterDept: "인사노무팀",
      content: `원문 메일 발신: ${email.sender} <${email.senderEmail}>\n수신: ${email.recipient} <${email.recipientEmail}>\n일시: ${email.date}\n\n--- 본문 내용 ---\n${email.body}`,
      linkedObjects: email.linkedObject ? [email.linkedObject] : [],
    });

    get().addToast({
      title: "전자결재 초안 등록",
      description: `메일 연계 결재문서(${draft.code}) 초안이 상신 대기열에 등록되었습니다.`,
      tone: "ok",
    });

    return draft;
  },

  // 1. Organization Setup Actions
  createOrgEntity: (entityInput) => {
    const newEntity: OrgEntity = {
      ...entityInput,
      id: `corp_${Date.now()}`,
    };
    set((s) => ({
      orgEntities: [...s.orgEntities, newEntity],
      auditEvents: recordStoreAuditEvent(s.auditEvents, {
        actorId: "current_user",
        actorName: "최원석 대표이사",
        action: "ORG_ENTITY_CREATED",
        targetCode: newEntity.code,
        targetKind: "OP",
        policyDecision: "PERMIT",
        reason: `신규 법인 설립 및 계좌 등록: ${newEntity.name} (${newEntity.bizNumber})`,
        dataClass: "대외비",
      }),
    }));
    get().addToast({
      title: "신규 법인 등록 완료",
      description: `${newEntity.name} (${newEntity.code}) 법인이 성공적으로 등록되었습니다.`,
      tone: "ok",
    });
    return newEntity;
  },

  updateOrgEntity: (id, updates) => {
    set((s) => ({
      orgEntities: s.orgEntities.map((e) => (e.id === id ? { ...e, ...updates } : e)),
    }));
    get().addToast({
      title: "법인 정보 수정 완료",
      description: "법인 세부 정보 및 주거래 모계좌 정보가 갱신되었습니다.",
      tone: "ok",
    });
  },

  createOrgSite: (siteInput) => {
    const newSite: OrgSite = {
      ...siteInput,
      id: `site_${Date.now()}`,
    };
    set((s) => ({
      orgSites: [...s.orgSites, newSite],
    }));
    get().addToast({
      title: "신규 사업장 / 현장 등록 완료",
      description: `${newSite.name} 사업장이 등록되었습니다.`,
      tone: "ok",
    });
    return newSite;
  },

  updateOrgSite: (id, updates) => {
    set((s) => ({
      orgSites: s.orgSites.map((st) => (st.id === id ? { ...st, ...updates } : st)),
    }));
    get().addToast({
      title: "사업장 정보 수정 완료",
      description: "현장 총괄 책임자 및 안전관리자 정보가 갱신되었습니다.",
      tone: "ok",
    });
  },

  createDepartment: (deptInput) => {
    const newDept: OrgDepartment = {
      ...deptInput,
      id: `dept_${Date.now()}`,
    };
    set((s) => ({
      orgDepartments: [...s.orgDepartments, newDept],
    }));
    get().addToast({
      title: "신규 부서 / 조직 편성 완료",
      description: `${newDept.name} 부서가 신설되었습니다.`,
      tone: "ok",
    });
    return newDept;
  },

  updateDepartment: (id, updates) => {
    set((s) => ({
      orgDepartments: s.orgDepartments.map((d) => (d.id === id ? { ...d, ...updates } : d)),
    }));
    get().addToast({
      title: "부서 정보 변경 완료",
      description: "조직도 및 코스트센터가 갱신되었습니다.",
      tone: "ok",
    });
  },

  updateOrgPolicy: (policyUpdates) => {
    set((s) => ({
      orgPolicy: {
        ...s.orgPolicy,
        ...policyUpdates,
        updatedAt: new Date().toISOString().replace("T", " ").slice(0, 16),
        updatedBy: `${s.viewAsRole} (사규관리위원회)`,
      },
      auditEvents: recordStoreAuditEvent(s.auditEvents, {
        actorId: "current_user",
        actorName: "박지영 수석",
        action: "ORG_POLICY_UPDATED",
        targetCode: "POL-LSA-2026",
        targetKind: "OP",
        policyDecision: "PERMIT",
        reason: "취업규칙, 근태·주52시간, DoA 전결 한도 및 연차 정책 개정",
        dataClass: "대외비",
      }),
    }));
    get().addToast({
      title: "사규 및 전결 정책 개정 완료",
      description: "근태/DoA/연차 정책이 전사 적용되었으며 감사 로그에 기록되었습니다.",
      tone: "ok",
    });
  },

  // 2. Operations & Equipment Actions
  createContract: (contractInput) => {
    const nextNum = get().contracts.length + 210;
    const code = `C-${nextNum}`;
    const newContract: Contract = {
      ...contractInput,
      id: `cnt_${Date.now()}`,
      code,
    };
    set((s) => ({
      contracts: [newContract, ...s.contracts],
    }));
    get().addToast({
      title: "신규 도급 / 위수탁 계약 등록 완료",
      description: `${newContract.title} (${newContract.code}) 계약이 유효 원장에 등록되었습니다.`,
      tone: "ok",
    });
    return newContract;
  },

  updateContract: (id, updates) => {
    set((s) => ({
      contracts: s.contracts.map((c) => (c.id === id ? { ...c, ...updates } : c)),
    }));
    get().addToast({
      title: "계약 정보 변경 완료",
      description: "도급 금액 및 투입 인력 기준이 갱신되었습니다.",
      tone: "ok",
    });
  },

  createEquipment: (eqInput) => {
    const nextNum = get().equipment.length + 105;
    const code = `EQ-${nextNum}`;
    const newEq: Equipment = {
      ...eqInput,
      id: `eq_${Date.now()}`,
      code,
    };
    set((s) => ({
      equipment: [newEq, ...s.equipment],
    }));
    get().addToast({
      title: "신규 현장 장비 / 기물 등록 완료",
      description: `${newEq.name} (${newEq.code}) 장비가 안전점검 대장에 등록되었습니다.`,
      tone: "ok",
    });
    return newEq;
  },

  updateEquipmentStatus: (id, status) => {
    set((s) => ({
      equipment: s.equipment.map((eq) => (eq.id === id ? { ...eq, status } : eq)),
    }));
    get().addToast({
      title: "장비 가동 상태 갱신",
      description: `장비 상태가 '${status}'(으)로 전이되었습니다.`,
      tone: "info",
    });
  },

  // 3. HR Personnel Actions Processor
  processPersonnelAction: ({ employeeId, actionType, effectiveDate, reason, updates }) => {
    const emp = get().employees.find((e) => e.id === employeeId);
    if (!emp) throw new Error("Employee not found");

    const prevDetails = {
      dept: emp.dept,
      role: emp.role,
      site: emp.site,
      baseSalary: emp.baseSalary,
      status: emp.status,
    };

    const seq = get().personnelActions.length + 42;
    const code = `PA-2026-${seq < 100 ? "0" + seq : seq}`;
    const orderNumber = `제2026-07-${seq}호`;

    const newAction: PersonnelAction = {
      id: `pa_${Date.now()}`,
      code,
      employeeId,
      employeeName: emp.name,
      actionType,
      effectiveDate,
      orderNumber,
      reason,
      prevDetails,
      newDetails: updates,
      approvedBy: `${get().viewAsRole} (인사위원회 의결)`,
      createdAt: new Date().toISOString().replace("T", " ").slice(0, 16),
    };

    // 사원 원장에 발령 내용 즉시 반영
    get().updateEmployee(employeeId, updates);

    set((s) => ({
      personnelActions: [newAction, ...s.personnelActions],
      auditEvents: recordStoreAuditEvent(s.auditEvents, {
        actorId: "current_user",
        actorName: "박지영 수석",
        action: `HR_PERSONNEL_ACTION_${actionType}`,
        targetCode: code,
        targetKind: "PA",
        policyDecision: "PERMIT",
        reason: `[${actionType}] ${emp.name} 사원 인사명령 발령 — ${reason}`,
        dataClass: "대외비",
      }),
    }));

    get().addToast({
      title: `인사명령 발령 완료 (${orderNumber})`,
      description: `${emp.name} 사원의 ${actionType} 명령이 발령되었습니다.`,
      tone: "ok",
    });

    return newAction;
  },

  // 4. Payroll Bank Transfer Batch & Firm Banking KFTC 100-byte Flat File Generator
  generateFirmBankingBatch: () => {
    const activeEmps = get().employees.filter((e) => e.status === "재직");
    const currentPayslips = get().payslips;
    const today = "20260725";
    const totalAmount = currentPayslips.reduce((sum, p) => sum + p.netPay, 0);

    const bankMap: Record<string, { bankName: string; bankCode: string; count: number; amount: number }> = {
      "088": { bankName: "신한은행", bankCode: "088", count: 0, amount: 0 },
      "004": { bankName: "국민은행", bankCode: "004", count: 0, amount: 0 },
      "020": { bankName: "우리은행", bankCode: "020", count: 0, amount: 0 },
      "081": { bankName: "하나은행", bankCode: "081", count: 0, amount: 0 },
      "003": { bankName: "IBK기업은행", bankCode: "003", count: 0, amount: 0 },
      "011": { bankName: "NH농협은행", bankCode: "011", count: 0, amount: 0 },
    };

    const headerCount = String(activeEmps.length).padStart(6, "0");
    const headerAmount = String(totalAmount).padStart(13, "0");
    const header = `H00099823100${today}${today}088${"100032998231".padEnd(20, " ")}${headerCount}${headerAmount}${" ".repeat(25)}`;

    const dataLines: string[] = [];
    const reqList: any[] = [];

    activeEmps.forEach((emp, idx) => {
      const slip = currentPayslips.find((p) => p.employeeId === emp.id) || currentPayslips[0];
      const net = slip ? slip.netPay : emp.baseSalary;
      const bankName = emp.bankName || "신한은행";
      const bankCode = bankName.includes("신한") ? "088" : bankName.includes("국민") ? "004" : bankName.includes("우리") ? "020" : bankName.includes("하나") ? "081" : bankName.includes("기업") ? "003" : "088";
      const account = (emp.bankAccount || "110-384-918231").replace(/-/g, "");

      if (!bankMap[bankCode]) {
        bankMap[bankCode] = { bankName, bankCode, count: 0, amount: 0 };
      }
      bankMap[bankCode].count += 1;
      bankMap[bankCode].amount += net;

      const seq = String(idx + 1).padStart(6, "0");
      const amtStr = String(net).padStart(10, "0");
      const namePad = emp.name.padEnd(10, " ");
      const companyPad = "(주)오야티  ";
      const dataLine = `D${seq}${bankCode}${account.padEnd(20, " ")}${amtStr}${namePad}${companyPad}01202607급여             `;
      dataLines.push(dataLine);

      reqList.push({
        tran_no: idx + 1,
        bank_code_std: bankCode,
        account_num: account,
        tran_amt: net,
        req_client_name: emp.name,
      });
    });

    const trailer = `T${headerCount}${headerAmount}${" ".repeat(74)}`;
    const flatFile = [header, ...dataLines, trailer].join("\n");
    const summaries = Object.values(bankMap).filter((b) => b.count > 0);

    const batch: BankTransferBatch = {
      id: `batch_${Date.now()}`,
      code: `FB-2607-${String(Math.floor(10 + Math.random() * 89))}`,
      payrollRunId: get().payrollRun.id,
      yearMonth: "2026-07",
      paymentDate: "2026-07-25",
      totalHeadcount: activeEmps.length,
      totalAmount,
      masterBank: "신한은행",
      masterAccount: "100-032-998231",
      bankSummaries: summaries,
      preflightVerified: true,
      preflightSuccessRate: 100,
      kftcFlatFileContent: flatFile,
      openApiPayload: {
        tran_dtime: `${today}093000`,
        req_cnt: activeEmps.length,
        req_list: reqList,
      },
      status: "전문생성",
    };

    set({ bankTransferBatch: batch });

    get().addToast({
      title: "KFTC 펌뱅킹 이체 전문 파일 생성 완료",
      description: `총 ${activeEmps.length}명 / ${(totalAmount).toLocaleString()}원 금융결제원 100바이트 표준 전문이 생성되었습니다.`,
      tone: "ok",
    });

    return batch;
  },

  sealAndDispatchBankBatch: (approverName: string, passkeyHash?: string) => {
    const current = get().bankTransferBatch;
    const hash = passkeyHash || ("9a7b" + Math.random().toString(16).slice(2) + Math.random().toString(16).slice(2));
    const updated: BankTransferBatch = {
      ...current,
      status: "출금승인대기",
      sealedAt: new Date().toISOString().replace("T", " ").slice(0, 16),
      sealedBy: `${approverName} (FIDO2 Passkey 날인)`,
      passkeyHash: hash,
    };

    set({
      bankTransferBatch: updated,
      payrollRun: { ...get().payrollRun, status: "지급승인" },
    });

    get().addToast({
      title: "급여 이체 승인 및 펌뱅킹 출금 지시 승인",
      description: "FIDO2 Passkey 서명 및 해시체인 봉인이 완료되어 은행 게이트웨이 전송 준비 상태로 전이되었습니다.",
      tone: "ok",
    });
  },

  freezePayrollRun: () => {
    get().calculateAndFreezePayroll();
    const batch = get().generateFirmBankingBatch();
    set({
      bankTransferBatch: {
        ...batch,
        status: "준비",
      },
    });
  },

  signBatchWithPasskey: (userId: string, approverName: string) => {
    const current = get().bankTransferBatch;
    const hash = "fido2_" + Math.random().toString(16).slice(2) + Math.random().toString(16).slice(2);
    const updated: BankTransferBatch = {
      ...current,
      status: "이체지시완료",
      sealedAt: new Date().toISOString(),
      sealedBy: approverName,
      passkeyHash: hash,
    };

    set((s) => ({
      bankTransferBatch: updated,
      payrollRun: { ...s.payrollRun, status: "지급승인" },
      auditEvents: recordStoreAuditEvent(s.auditEvents, {
        actorId: userId,
        actorName: approverName,
        action: "BANKING_BATCH_PASSKEY_SEALED",
        targetCode: current?.code || "FB-202607",
        targetKind: "FB",
        policyDecision: "PERMIT",
        reason: "FIDO2 Passkey 생체인증 전자서명 및 금융결제원 펌뱅킹 이체 승인",
        dataClass: "민감",
      }),
    }));

    get().addToast({
      title: "FIDO2 Passkey 날인 및 이체 지시 완료",
      description: `${approverName} 서명으로 펌뱅킹 출금 지시가 최종 완료되었습니다.`,
      tone: "ok",
    });

    return { success: true, passkeyHash: hash };
  },

  transferEmployee: (employeeId, targetDept, targetSite, effectiveDate, reason) => {
    return get().processPersonnelAction({
      employeeId,
      actionType: "부서전보",
      effectiveDate,
      reason,
      updates: {
        dept: targetDept,
        site: targetSite,
      },
    });
  },

  repairDiscrepancy: (id: string, resolutionComment: string) => {
    const disc = get().discrepancies.find((d) => d.id === id);
    if (!disc) return;

    // 1. Mark discrepancy as resolved
    set((s) => ({
      discrepancies: s.discrepancies.map((d) =>
        d.id === id
          ? {
              ...d,
              status: "resolved",
              resolutionComment,
              resolvedAt: new Date().toISOString().slice(0, 16).replace("T", " "),
              resolvedBy: get().viewAsRole,
            }
          : d
      ),
    }));

    // 2. If it's an attendance exception (sourceKind === 'AT'), also update attendance record
    if (disc.sourceKind === "AT") {
      set((s) => ({
        attendance: s.attendance.map((att) =>
          att.employeeId === disc.employeeId && att.exceptionCode
            ? { ...att, exceptionCode: undefined, status: "정상" }
            : att
        ),
      }));
    }

    // 3. Decrement unresolved exceptions count on payrollRun
    const currentUnresolved = Math.max(0, get().payrollRun.unresolvedExceptions - 1);
    set((s) => ({
      payrollRun: {
        ...s.payrollRun,
        unresolvedExceptions: currentUnresolved,
        status: currentUnresolved === 0 ? "계산완료" : s.payrollRun.status,
      },
    }));

    // 4. Reactively recalculate payslips for affected employee
    const emp = get().employees.find((e) => e.id === disc.employeeId);
    if (emp) {
      const att = get().attendance.find((a) => a.employeeId === emp.id);
      const updatedSlip = computeEmployeePayslip(emp, {
        otHours: att?.otHours || 0,
        nightHours: att?.nightHours || 0,
        holidayHours: att?.holHours || 0,
      });
      set((s) => ({
        payslips: s.payslips.map((p) => (p.employeeId === emp.id ? updatedSlip : p)),
      }));
    }

    // 5. Audit event
    set((s) => ({
      auditEvents: recordStoreAuditEvent(s.auditEvents, {
        actorId: "current_user",
        actorName: get().viewAsRole,
        action: "PAYROLL_DISCREPANCY_REPAIRED",
        targetCode: disc.sourceCode,
        targetKind: "AT",
        policyDecision: "PERMIT",
        reason: `[급여 예외 해소] ${disc.employeeName}: ${disc.title} (${resolutionComment})`,
        dataClass: "대외비",
      }),
    }));

    get().addToast({
      title: "급여 예외 정정 및 해소 완료",
      description: `${disc.employeeName} 사원의 ${disc.sourceCode} 예외가 승인되어 급여 계산 락이 해제되었습니다.`,
      tone: "ok",
    });
  },

  savePayrollDraft: (draftPartial) => {
    const existing = get().payrollDraft || {
      savedAt: new Date().toISOString().slice(0, 16).replace("T", " "),
      yearMonth: "2026-07",
      filterQuery: "",
      activeTab: "payslip",
      unresolvedCount: get().payrollRun.unresolvedExceptions,
    };
    const draft: PayrollDraftState = {
      ...existing,
      ...draftPartial,
      savedAt: new Date().toISOString().slice(0, 16).replace("T", " "),
      unresolvedCount: get().payrollRun.unresolvedExceptions,
    };
    set({ payrollDraft: draft });
    get().addToast({
      title: "급여 작업 초안 임시 보관",
      description: `${draft.yearMonth} 작업 내용은 현재 탭에서만 유지되며 새로고침하면 사라집니다.`,
      tone: "info",
    });
  },

  resumePayrollDraft: () => {
    return get().payrollDraft;
  },

  addCedarPolicy: (policy) => {
    set((s) => ({
      cedarPolicies: [...s.cedarPolicies, policy],
      auditEvents: recordStoreAuditEvent(s.auditEvents, {
        actorId: "current_user",
        actorName: get().viewAsRole,
        action: "CEDAR_POLICY_ADDED",
        targetCode: policy.id,
        targetKind: "OP",
        policyDecision: "PERMIT",
        reason: `[Cedar 사규 정책 추가] ${policy.title} (${policy.effect.toUpperCase()})`,
        dataClass: "대외비",
      }),
    }));
    get().addToast({
      title: "Cedar 보안 정책 등록 완료",
      description: `정책 [${policy.title}]이(가) 인가 엔진에 활성화되었습니다.`,
      tone: "ok",
    });
  },

  removeCedarPolicy: (id) => {
    const target = get().cedarPolicies.find((p) => p.id === id);
    if (target?.isSystemDefault) {
      get().addToast({
        title: "기본 정책 삭제 불가",
        description: "시스템 기본 거버넌스 정책(SoD 등)은 비활성화할 수 없습니다.",
        tone: "danger",
      });
      return;
    }
    set((s) => ({
      cedarPolicies: s.cedarPolicies.filter((p) => p.id !== id),
    }));
    get().addToast({
      title: "보안 정책 삭제 완료",
      description: `정책 [${target?.title || id}]이(가) 해제되었습니다.`,
      tone: "warn",
    });
  },

  grantAccessAssignment: (grantInput) => {
    const newGrant: AccessGrantAssignment = {
      ...grantInput,
      id: `grant-${Date.now().toString().slice(-4)}`,
      grantedAt: new Date().toISOString(),
      status: "active",
    };
    set((s) => ({
      accessAssignments: [newGrant, ...s.accessAssignments],
      auditEvents: recordStoreAuditEvent(s.auditEvents, {
        actorId: "current_user",
        actorName: get().viewAsRole,
        action: "ACCESS_GRANT_ASSIGNED",
        targetCode: newGrant.id,
        targetKind: "OP",
        policyDecision: "PERMIT",
        reason: `[권한 배정] ${newGrant.assigneeName}에게 ${newGrant.capabilityBundleName} 부여 (${newGrant.justification})`,
        dataClass: "대외비",
      }),
    }));

    get().addToast({
      title: "직무 권한 배정 승인 완료",
      description: `${newGrant.assigneeName} 대상 [${newGrant.capabilityBundleName}] 권한이 즉시 부여되었습니다.`,
      tone: "ok",
    });
    return newGrant;
  },

  revokeAccessAssignment: (id, reason) => {
    const target = get().accessAssignments.find((a) => a.id === id);
    if (!target) return;
    set((s) => ({
      accessAssignments: s.accessAssignments.map((a) =>
        a.id === id ? { ...a, status: "revoked" } : a
      ),
      auditEvents: recordStoreAuditEvent(s.auditEvents, {
        actorId: "current_user",
        actorName: get().viewAsRole,
        action: "ACCESS_GRANT_REVOKED",
        targetCode: id,
        targetKind: "OP",
        policyDecision: "PERMIT",
        reason: `[권한 회수] ${target.assigneeName}의 ${target.capabilityBundleName} 권한 철회: ${reason || "관리자 직권 회수"}`,
        dataClass: "대외비",
      }),
    }));
    get().addToast({
      title: "권한 배정 회수 완료",
      description: `${target.assigneeName}의 [${target.capabilityBundleName}] 권한이 철회되었습니다.`,
      tone: "warn",
    });
  },

  resolveOperationalIncident: (id, resolutionNote) => {
    const inc = get().operationalIncidents.find((i) => i.id === id);
    if (!inc) return;

    set((s) => ({
      operationalIncidents: s.operationalIncidents.map((i) =>
        i.id === id
          ? {
              ...i,
              status: "resolved",
              resolutionNote,
              resolvedAt: new Date().toISOString().slice(0, 19).replace("T", " "),
            }
          : i
      ),
    }));

    // Domain side-effects for specific incident categories:
    if (inc.category === "banking_cert") {
      set((s) => ({
        bankTransferBatch: {
          ...s.bankTransferBatch,
          preflightVerified: true,
          preflightSuccessRate: 100,
        },
      }));
    } else if (inc.category === "import_validation") {
      // Cleanse employee data / attendance discrepancies
      get().discrepancies.forEach((d) => {
        if (d.status === "unresolved" && d.canAutoRepair) {
          get().repairDiscrepancy(d.id, "운영 복구 센터 대량 데이터 정제 일괄 반영");
        }
      });
    }

    set((s) => ({
      auditEvents: recordStoreAuditEvent(s.auditEvents, {
        actorId: "current_user",
        actorName: get().viewAsRole,
        action: "OPERATIONAL_INCIDENT_RESOLVED",
        targetCode: id,
        targetKind: "OP",
        policyDecision: "PERMIT",
        reason: `[운영 장애 셀프 복구] ${inc.title} - ${resolutionNote}`,
        dataClass: "대외비",
      }),
    }));

    get().addToast({
      title: "운영 장애 자체 복구 완료",
      description: `[${inc.title}] 건이 성공적으로 복구 및 정상화되었습니다.`,
      tone: "ok",
    });
  },

  reassignApprovalDelegate: (docCode, fromPersonId, toPersonId, reason) => {
    const doc = get().approvals.find((d) => d.code === docCode);
    if (!doc) {
      return { success: false, message: `문서(${docCode})를 찾을 수 없습니다.` };
    }

    // Independence verification: Check if toPersonId is the drafter (PersonIdentity check)
    const toEmployee = get().employees.find((e) => e.id === toPersonId || e.code === toPersonId);
    if (!toEmployee) {
      return { success: false, message: `지정된 대결자(${toPersonId})를 구성원 원장에서 찾을 수 없습니다.` };
    }

    if (doc.drafterId === toEmployee.id || doc.drafterName === toEmployee.name) {
      return {
        success: false,
        message: `직무 분리(SoD) 규정 위반: 대결 대상자(${toEmployee.name})가 해당 결재 문서의 최초 기안자이므로 대결권자로 지정할 수 없습니다.`,
      };
    }

    // Reassign pending stage
    const updatedStages = doc.stages.map((stage) => {
      if (stage.status === "pending") {
        return {
          ...stage,
          approverName: `${toEmployee.name} (대결: ${reason})`,
          approverRole: toEmployee.role || stage.approverRole,
        };
      }
      return stage;
    });

    set((s) => ({
      approvals: s.approvals.map((d) => (d.code === docCode ? { ...d, stages: updatedStages } : d)),
      auditEvents: recordStoreAuditEvent(s.auditEvents, {
        actorId: "current_user",
        actorName: get().viewAsRole,
        action: "APPROVAL_DELEGATE_REASSIGNED",
        targetCode: docCode,
        targetKind: "AP",
        policyDecision: "PERMIT",
        reason: `[부재자 대결 지정] ${docCode} 결재선 ${fromPersonId} -> ${toEmployee.name} (${reason})`,
        dataClass: "대외비",
      }),
    }));

    get().addToast({
      title: "결재 대결 지정 완료 (SoD 검증 통과)",
      description: `${docCode} 결재선이 ${toEmployee.name} 대결권자에게 안전하게 인계되었습니다.`,
      tone: "ok",
    });

    return {
      success: true,
      message: `${docCode} 문서의 결재선이 ${toEmployee.name} 님으로 안전하게 인계되었습니다.`,
    };
  },

  setActiveActingCapacityId: (roleId) => set({ activeActingCapacityId: roleId }),

  assignRoleToEmployee: (employeeIdOrPayload, roleIdParam, scopeParam, reasonParam) => {
    let employeeId: string;
    let roleId: string;
    let scope: RoleAssignment["scope"] | undefined;
    let reason: string | undefined;

    if (typeof employeeIdOrPayload === "object" && employeeIdOrPayload !== null) {
      employeeId = employeeIdOrPayload.employeeId;
      roleId = employeeIdOrPayload.roleId;
      scope = employeeIdOrPayload.scope;
      reason = employeeIdOrPayload.grantReason || employeeIdOrPayload.reason;
    } else {
      employeeId = employeeIdOrPayload;
      roleId = roleIdParam!;
      scope = scopeParam;
      reason = reasonParam;
    }

    const roleDef = get().roleDefinitions.find((r) => r.id === roleId);
    const emp = get().employees.find((e) => e.id === employeeId || e.code === employeeId);
    const actualEmpId = emp ? emp.id : employeeId;
    const actualPersonId = emp ? `P-${emp.id}` : `P-${employeeId}`;

    const newAssignment: RoleAssignment = {
      id: `ra-${Date.now()}-${Math.floor(Math.random() * 1000)}`,
      personId: actualPersonId,
      employeeId: actualEmpId,
      roleId,
      roleName: roleDef?.name || roleId,
      scope: scope || { type: "all" },
      validFrom: new Date().toISOString().slice(0, 10),
      grantedBy: "ACC-SYS-ADMIN",
      grantReason: reason || "관리자 직접 부여",
      status: "active",
    };

    // Keep employee.roleIds in sync
    const updatedEmployees = get().employees.map((e) => {
      if (e.id === actualEmpId) {
        const currentRoleIds = e.roleIds || [];
        if (!currentRoleIds.includes(roleId)) {
          return { ...e, roleIds: [...currentRoleIds, roleId] };
        }
      }
      return e;
    });

    set((s) => ({
      roleAssignments: [newAssignment, ...s.roleAssignments],
      employees: updatedEmployees,
      auditEvents: recordStoreAuditEvent(s.auditEvents, {
        actorId: "current_user",
        actorName: get().viewAsRole,
        action: "ROLE_LAYER_ASSIGNED",
        targetCode: actualEmpId,
        targetKind: "P",
        policyDecision: "PERMIT",
        reason: `[역할 레이어 부여] ${emp?.name || actualEmpId} 사원에게 '${roleDef?.name || roleId}' 역할 부여 (${reason || "관리자 부여"})`,
        dataClass: "대외비",
      }),
    }));

    get().addToast({
      title: "역할 레이어 부여 완료",
      description: `${emp?.name || actualEmpId} 님에게 '${roleDef?.name || roleId}' 역할이 즉시 활성화되었습니다.`,
      tone: "ok",
    });

    return newAssignment;
  },

  removeRoleFromEmployee: (assignmentId) => {
    const target = get().roleAssignments.find((a) => a.id === assignmentId);
    if (!target) return;

    const remainingForEmpAndRole = get().roleAssignments.filter(
      (a) => a.id !== assignmentId && a.employeeId === target.employeeId && a.roleId === target.roleId && a.status === "active"
    );

    // If no more active assignments for this role, remove from employee.roleIds
    const updatedEmployees = get().employees.map((e) => {
      if (e.id === target.employeeId && remainingForEmpAndRole.length === 0) {
        return {
          ...e,
          roleIds: (e.roleIds || []).filter((rId) => rId !== target.roleId),
        };
      }
      return e;
    });

    set((s) => ({
      roleAssignments: s.roleAssignments.filter((a) => a.id !== assignmentId),
      employees: updatedEmployees,
      auditEvents: recordStoreAuditEvent(s.auditEvents, {
        actorId: "current_user",
        actorName: get().viewAsRole,
        action: "ROLE_LAYER_REVOKED",
        targetCode: target.employeeId,
        targetKind: "P",
        policyDecision: "PERMIT",
        reason: `[역할 레이어 회수] ${target.roleName} (배정ID: ${assignmentId}) 회수`,
        dataClass: "대외비",
      }),
    }));

    get().addToast({
      title: "역할 레이어 회수 완료",
      description: `'${target.roleName}' 역할이 안전하게 회수되었으며 인가 권한이 즉시 재계산되었습니다.`,
      tone: "warn",
    });
  },

  createRoleDefinition: (roleInput) => {
    const id = `role-custom-${Date.now()}`;
    const newRole: RoleDefinition = {
      ...roleInput,
      id,
    };
    set((s) => ({
      roleDefinitions: [...s.roleDefinitions, newRole],
      auditEvents: recordStoreAuditEvent(s.auditEvents, {
        actorId: "current_user",
        actorName: get().viewAsRole,
        action: "ROLE_DEFINITION_CREATED",
        targetCode: id,
        targetKind: "ROLE",
        policyDecision: "PERMIT",
        reason: `[역할 카탈로그 등록] ${newRole.name} (우선순위: ${newRole.priority})`,
        dataClass: "대외비",
      }),
    }));

    get().addToast({
      title: "신규 역할 카탈로그 생성 완료",
      description: `'${newRole.name}' 역할이 등록되었습니다. 이제 구성원에게 배정할 수 있습니다.`,
      tone: "ok",
    });

    return newRole;
  },

  updateRoleDefinition: (id, updates) => {
    set((s) => ({
      roleDefinitions: s.roleDefinitions.map((r) => (r.id === id ? { ...r, ...updates } : r)),
    }));
    get().addToast({
      title: "역할 속성 갱신 완료",
      description: "역할 정의 및 권한 번들이 갱신되었습니다.",
      tone: "info",
    });
  },

  reorderRoles: (orderedIds) => {
    set((s) => {
      const updated = s.roleDefinitions.map((role) => {
        const idx = orderedIds.indexOf(role.id);
        if (idx !== -1) {
          const newPriority = Math.max(10, 100 - idx * 5);
          return { ...role, priority: newPriority };
        }
        return role;
      });
      updated.sort((a, b) => b.priority - a.priority);
      return { roleDefinitions: updated };
    });
    get().addToast({
      title: "역할 우선순위 계층 갱신 완료",
      description: "Discord식 역할 우선순위 순서가 성공적으로 적용되었습니다.",
      tone: "ok",
    });
  },

  getEffectivePermissionsForEmployee: (employeeId, targetScope) => {
    const emp = get().employees.find((e) => e.id === employeeId || e.code === employeeId);
    const personId = emp ? `P-${emp.id}` : `P-${employeeId}`;
    return foldUserEffectivePermissions(
      personId,
      targetScope,
      get().roleDefinitions,
      get().roleAssignments
    );
  },

  exportStateBackup: () => {
    const s = get();
    const backup = {
      version: "1.0.0",
      exportedAt: new Date().toISOString(),
      employees: s.employees,
      attendance: s.attendance,
      approvals: s.approvals,
      workOrders: s.workOrders,
      payrollRun: s.payrollRun,
      payslips: s.payslips,
      auditEvents: s.auditEvents,
      orgEntities: s.orgEntities,
      orgSites: s.orgSites,
      orgDepartments: s.orgDepartments,
      orgPolicy: s.orgPolicy,
      contracts: s.contracts,
      equipment: s.equipment,
      roleDefinitions: s.roleDefinitions,
      roleAssignments: s.roleAssignments,
    };
    return JSON.stringify(backup, null, 2);
  },

  importStateBackup: (jsonString: string) => {
    try {
      const data = JSON.parse(jsonString);
      if (!data || typeof data !== "object") {
        return { success: false, message: "올바르지 않은 백업 데이터 형식입니다." };
      }
      set((s) => ({
        ...s,
        ...(data.employees ? { employees: data.employees } : {}),
        ...(data.attendance ? { attendance: data.attendance } : {}),
        ...(data.approvals ? { approvals: data.approvals } : {}),
        ...(data.workOrders ? { workOrders: data.workOrders } : {}),
        ...(data.payrollRun ? { payrollRun: data.payrollRun } : {}),
        ...(data.payslips ? { payslips: data.payslips } : {}),
        ...(data.auditEvents ? { auditEvents: data.auditEvents } : {}),
        ...(data.orgEntities ? { orgEntities: data.orgEntities } : {}),
        ...(data.orgSites ? { orgSites: data.orgSites } : {}),
        ...(data.orgDepartments ? { orgDepartments: data.orgDepartments } : {}),
        ...(data.orgPolicy ? { orgPolicy: data.orgPolicy } : {}),
        ...(data.contracts ? { contracts: data.contracts } : {}),
        ...(data.equipment ? { equipment: data.equipment } : {}),
        ...(data.roleDefinitions ? { roleDefinitions: data.roleDefinitions } : {}),
        ...(data.roleAssignments ? { roleAssignments: data.roleAssignments } : {}),
      }));
      get().addToast({
        title: "백업 데이터 복원 완료",
        description: "원장 및 설정 데이터가 성공적으로 복원되었습니다.",
        tone: "ok",
      });
      return { success: true, message: "성공적으로 복원되었습니다." };
    } catch (err: any) {
      return { success: false, message: `복원 실패: ${err.message}` };
    }
  },
}));

export const useConsoleStore = useAppStore;
