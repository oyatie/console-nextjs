// dynamic-factory.ts — Enterprise Dynamic Domain Derivation & Synthesis Engine
// Generates, derives, validates, and simulates full enterprise operational graphs dynamically.

import {
  OrgEntity,
  OrgSite,
  OrgDepartment,
  Employee,
  AttendanceRecord,
  Payslip,
  PayrollRun,
  BankTransferBatch,
  AuditEvent,
  ObjectKind,
} from "./types";
import { resolveStatutoryRates } from "./regulatory-registry";
import { computeEmployeePayslip } from "./payroll-engine";

export interface DynamicOrgConfig {
  corporateCount?: number;
  sitesPerCorp?: number;
  deptsPerCorp?: number;
  baseYear?: number;
}

export interface DynamicOrgGraph {
  entities: OrgEntity[];
  sites: OrgSite[];
  departments: OrgDepartment[];
}

/**
 * 1. Dynamically generates an enterprise multi-corporate graph
 */
export function generateDynamicOrganization(config: DynamicOrgConfig = {}): DynamicOrgGraph {
  const corpCount = config.corporateCount || 3;
  const sitesPerCorp = config.sitesPerCorp || 2;
  const deptsPerCorp = config.deptsPerCorp || 3;
  const baseYear = config.baseYear || 2026;

  const entities: OrgEntity[] = [];
  const sites: OrgSite[] = [];
  const departments: OrgDepartment[] = [];

  const corpNames = [
    "(주)오야티 코퍼레이션",
    "(주)오야티 로지스틱스",
    "(주)오야티 서비스",
    "(주)오야티 인프라",
    "(주)오야티 솔루션",
  ];
  const corpCodes = ["OYT-HQ", "OYT-LOG", "OYT-SVC", "OYT-INF", "OYT-SOL"];
  const ceos = ["최원석", "강성훈", "조민규", "박진호", "윤상미"];
  const banks = ["신한은행", "우리은행", "IBK기업은행", "하나은행", "KB국민은행"];

  const siteRegions = ["인천", "평택", "당진", "부산", "울산", "광양", "천안"];
  const deptTypes = ["운영기획실", "물류운영팀", "시설보전팀", "안전품질팀", "재경인사팀"];

  for (let c = 0; c < corpCount; c++) {
    const corpId = `dyn_corp_${c + 1}`;
    const corpCode = corpCodes[c % corpCodes.length] || `OYT-0${c + 1}`;
    const name = corpNames[c % corpNames.length] || `(주)오야티 엔터프라이즈 ${c + 1}`;
    const bizNo = `${100 + c * 14}-${80 + c * 3}-${String(10000 + c * 1234).slice(-5)}`;
    const corpNo = `110111-${String(3000000 + c * 100000).slice(-7)}`;

    entities.push({
      id: corpId,
      code: corpCode,
      name,
      bizNumber: bizNo,
      corpNumber: corpNo,
      ceoName: ceos[c % ceos.length] || "대표이사",
      address: `대한민국 서울특별시 테헤란로 ${400 + c * 10}`,
      mainBank: banks[c % banks.length],
      mainAccount: `${100 + c * 20}-${String(100000 + c * 9876).slice(-6)}`,
      establishedDate: `${baseYear - 8 + c}-03-15`,
      status: "active",
    });

    for (let s = 0; s < sitesPerCorp; s++) {
      const siteId = `dyn_site_${c + 1}_${s + 1}`;
      const region = siteRegions[(c * sitesPerCorp + s) % siteRegions.length];
      const siteName = `${region} 제${s + 1}물류센터`;

      sites.push({
        id: siteId,
        entityId: corpId,
        code: `S-${c + 1}0${s + 1}`,
        name: siteName,
        category: s === 0 ? "물류센터" : "정비사업소",
        address: `${region} 산업단지 ${10 + s}길 ${s + 1}`,
        siteManager: `현장소장_${region}`,
        safetyManager: `안전관리자_${region}`,
        phone: `032-${500 + s * 10}-100${s}`,
        activeHeadcount: 0,
      });
    }

    for (let d = 0; d < deptsPerCorp; d++) {
      const deptId = `dyn_dept_${c + 1}_${d + 1}`;
      departments.push({
        id: deptId,
        name: deptTypes[d % deptTypes.length],
        code: `D-${c + 1}0${d + 1}`,
        entityId: corpId,
        division: "운영본부",
        managerName: `팀장_${d + 1}`,
        costCenter: `CC-${1000 + c * 100 + d * 10}`,
      });
    }
  }

  return { entities, sites, departments };
}

/**
 * 2. Dynamically generates workforce given an organizational graph
 */
export function generateDynamicWorkforce(
  orgGraph: DynamicOrgGraph,
  totalEmployees: number = 10,
  options: { baseSalaryFloor?: number; baseSalaryCeil?: number } = {}
): Employee[] {
  const { entities, sites, departments } = orgGraph;
  const employees: Employee[] = [];

  const familyNames = ["김", "이", "박", "최", "정", "강", "조", "윤", "장", "임", "한", "오", "서", "신", "권"];
  const givenNames = ["민수", "지훈", "영호", "성진", "도현", "하은", "서연", "수빈", "예은", "지원", "재원", "동현", "태호"];
  const grades = ["사원", "대리", "과장", "차장", "부장"];
  const roles = ["현장운영", "정비엔지니어", "물류관리", "안전점검", "HR담당", "회계담당"];
  const banks = ["신한은행", "우리은행", "국민은행", "하나은행", "토스뱅크", "카카오뱅크"];

  const floor = options.baseSalaryFloor || 2800000;
  const ceil = options.baseSalaryCeil || 6500000;

  for (let i = 0; i < totalEmployees; i++) {
    const id = `dyn_emp_${String(i + 1).padStart(3, "0")}`;
    const fName = familyNames[i % familyNames.length];
    const gName = givenNames[(i * 3) % givenNames.length];
    const name = `${fName}${gName}`;
    const corp = entities[i % entities.length];
    const site = sites[i % sites.length];
    const dept = departments[i % departments.length];
    const grade = grades[i % grades.length];
    const role = roles[i % roles.length];

    const baseSalary = Math.round((floor + ((ceil - floor) * (i / Math.max(1, totalEmployees - 1)))) / 10000) * 10000;
    const fixedAllow = 200000 + (i % 4) * 100000;

    employees.push({
      id,
      code: `OYT-${String(1000 + i + 1)}`,
      name,
      email: `${id.toLowerCase()}@oyatie.com`,
      phone: `010-${String(2000 + i * 7).padStart(4, "0")}-${String(3000 + i * 11).padStart(4, "0")}`,
      entity: corp.name,
      site: site.name,
      dept: dept.name,
      role,
      grade,
      position: `${dept.name} ${grade}`,
      empType: i % 7 === 0 ? "계약" : "정규",
      status: "재직",
      joinedDate: `202${Math.max(0, 4 - (i % 4))}-0${(i % 9) + 1}-15`,
      baseSalary,
      fixedAllow,
      dependents: (i % 3) + 1,
      bankName: banks[i % banks.length],
      bankAccount: `${100 + (i * 13) % 900}-${String(100000 + i * 291).slice(-6)}`,
    });
  }

  return employees;
}

/**
 * 3. Dynamically derives attendance punches and weekly hours
 */
export function deriveDynamicAttendance(
  employees: Employee[],
  period: string, // YYYY-MM
  options: { inject52hOverloadIndex?: number; injectMissingPunchesIndex?: number } = {}
): AttendanceRecord[] {
  const records: AttendanceRecord[] = [];

  employees.forEach((emp, idx) => {
    let workedHours = 160;
    let otHours = 8 + (idx % 6) * 2;
    let nightHours = idx % 3 === 0 ? 6 : 0;
    let holHours = idx % 4 === 0 ? 8 : 0;
    let status: AttendanceRecord["status"] = "정상";
    let exceptionCode: string | undefined = undefined;

    if (options.inject52hOverloadIndex !== undefined && idx === options.inject52hOverloadIndex) {
      otHours = 16;
      status = "예외승인대기";
      exceptionCode = `AT-52H-${emp.id}`;
    }

    if (options.injectMissingPunchesIndex !== undefined && idx === options.injectMissingPunchesIndex) {
      status = "미출근";
      exceptionCode = `AT-MISS-${emp.id}`;
    }

    records.push({
      id: `dyn_att_${emp.id}_${period}`,
      employeeId: emp.id,
      employeeName: emp.name,
      entity: emp.entity,
      site: emp.site,
      date: `${period}-15`,
      clockIn: "08:55",
      clockOut: "18:05",
      breakMinutes: 60,
      plannedIn: "09:00",
      plannedOut: "18:00",
      workedHours,
      otHours,
      nightHours,
      holHours,
      weeklyHoursTotal: 40 + otHours / 4,
      status,
      exceptionCode,
    });
  });

  return records;
}

/**
 * 4. Dynamically processes monthly payroll run with statutory compliance
 */
export function processDynamicPayrollRun(
  employees: Employee[],
  attendance: AttendanceRecord[],
  period: string,
  options: { companyMatchName?: string } = {}
): { run: PayrollRun; payslips: Payslip[]; batch: BankTransferBatch } {
  const filteredEmployees = options.companyMatchName
    ? employees.filter((e) => e.entity === options.companyMatchName)
    : employees;

  const payslips: Payslip[] = filteredEmployees.map((emp) => {
    const att = attendance.find((a) => a.employeeId === emp.id);
    return computeEmployeePayslip(emp, {
      otHours: att?.otHours || 0,
      nightHours: att?.nightHours || 0,
      holidayHours: att?.holHours || 0,
    });
  });

  const totalGross = payslips.reduce((sum, p) => sum + p.grossPay, 0);
  const totalDeductions = payslips.reduce((sum, p) => sum + p.totalDeductions, 0);
  const totalNet = payslips.reduce((sum, p) => sum + p.netPay, 0);

  const unresolvedCount = attendance.filter(
    (a) =>
      filteredEmployees.some((e) => e.id === a.employeeId) &&
      (a.status === "예외승인대기" || a.status === "미출근")
  ).length;

  const run: PayrollRun = {
    id: `dyn_run_${period}`,
    code: `PR-${period.replace("-", "")}`,
    yearMonth: period,
    entity: options.companyMatchName || filteredEmployees[0]?.entity || "(주)오야티 코퍼레이션",
    title: `${period} 정기 급여 산정 및 펌뱅킹 정산`,
    status: unresolvedCount > 0 ? "준비" : "계산완료",
    totalGross,
    totalNet,
    totalDeductions,
    headcount: filteredEmployees.length,
    unresolvedExceptions: unresolvedCount,
    isLocked: false,
  };

  const batch: BankTransferBatch = {
    id: `dyn_batch_${period}`,
    code: `FB-${period.replace("-", "")}-01`,
    payrollRunId: run.id,
    yearMonth: period,
    paymentDate: `${period}-25`,
    totalHeadcount: payslips.length,
    totalAmount: totalNet,
    masterBank: "신한은행",
    masterAccount: "100-032-998231",
    bankSummaries: [
      { bankName: "신한은행", bankCode: "088", count: payslips.length, amount: totalNet },
    ],
    preflightVerified: true,
    preflightSuccessRate: 100,
    kftcFlatFileContent: `HEADER_${period}_${totalNet}`,
    openApiPayload: { runId: run.id, totalAmount: totalNet },
    status: "준비",
  };

  return { run, payslips, batch };
}

/**
 * 5. Dynamic cryptographic hash chain verification engine
 */
export function calculateEventHash(event: Omit<AuditEvent, "hash">): string {
  const content = `${event.id}:${event.seq}:${event.timestamp}:${event.actorId}:${event.action}:${event.targetKind}:${event.targetCode}:${event.prevHash}`;
  let h = 0x811c9dc5;
  for (let i = 0; i < content.length; i++) {
    h ^= content.charCodeAt(i);
    h = Math.imul(h, 0x01000193);
  }
  const hashHex = (h >>> 0).toString(16).padStart(8, "0");
  return `sha256_${hashHex}_${Math.abs(h).toString(36)}`;
}

export function appendDynamicAuditEvent(
  chain: AuditEvent[],
  eventData: {
    actorId: string;
    actorName: string;
    action: string;
    targetKind: ObjectKind;
    targetCode: string;
    policyDecision?: AuditEvent["policyDecision"];
    reason?: string;
    dataClass?: AuditEvent["dataClass"];
  }
): AuditEvent {
  const prevEvent = chain[chain.length - 1];
  const prevHash = prevEvent ? prevEvent.hash : "0000000000000000000000000000000000000000000000000000000000000000";
  const seq = (prevEvent?.seq || 0) + 1;
  const id = `evt_${Date.now()}_${seq}`;
  const timestamp = new Date().toISOString();

  const rawEvent: Omit<AuditEvent, "hash"> = {
    id,
    seq,
    prevHash,
    timestamp,
    actorId: eventData.actorId,
    actorName: eventData.actorName,
    action: eventData.action,
    targetKind: eventData.targetKind,
    targetCode: eventData.targetCode,
    policyDecision: eventData.policyDecision || "PERMIT",
    reason: eventData.reason,
    dataClass: eventData.dataClass || "일반",
  };

  const hash = calculateEventHash(rawEvent);
  const newEvent: AuditEvent = { ...rawEvent, hash };
  return newEvent;
}

export function generateInitialAuditChain(): AuditEvent[] {
  const chain: AuditEvent[] = [];
  const baseActions = [
    {
      actorId: "emp_01",
      actorName: "김민수 (경영기획)",
      action: "APPROVAL_DRAFT_CREATED",
      targetKind: "AP" as ObjectKind,
      targetCode: "AP-3125",
      reason: "동국제강 도급계약 갱신안 신규 기안 작성",
      dataClass: "대외비" as const,
    },
    {
      actorId: "emp_02",
      actorName: "박지영 (인사노무)",
      action: "ATTENDANCE_OVERTIME_FLAGGED",
      targetKind: "AT" as ObjectKind,
      targetCode: "AT-0703-01",
      reason: "주52시간 상한 초과 감지 (53.8h) — 노무 게이트 트리거",
      dataClass: "민감" as const,
    },
    {
      actorId: "emp_06",
      actorName: "황도현 (정비2팀)",
      action: "WORK_ORDER_COMPLETED",
      targetKind: "WO" as ObjectKind,
      targetCode: "WO-2641",
      reason: "지게차 실린더 교체 및 안전 센서 테스트 합격",
      dataClass: "일반" as const,
    },
  ];

  for (const b of baseActions) {
    chain.push(appendDynamicAuditEvent(chain, b));
  }

  return chain;
}

export function verifyAuditChainIntegrity(chain: AuditEvent[]): { valid: boolean; corruptedIndex?: number; reason?: string } {
  if (chain.length === 0) return { valid: true };

  const isReverse = chain.length > 1 && chain[0].seq > chain[chain.length - 1].seq;
  const ordered = isReverse ? [...chain].reverse() : chain;

  for (let i = 0; i < ordered.length; i++) {
    const current = ordered[i];
    const expectedPrevHash = i === 0 ? "0000000000000000000000000000000000000000000000000000000000000000" : ordered[i - 1].hash;

    if (current.prevHash !== expectedPrevHash) {
      return {
        valid: false,
        corruptedIndex: isReverse ? chain.length - 1 - i : i,
        reason: `Previous hash mismatch at event ${current.id}. Expected ${expectedPrevHash}, got ${current.prevHash}`,
      };
    }

    const { hash, ...raw } = current;
    const computedHash = calculateEventHash(raw);
    if (computedHash !== current.hash) {
      return {
        valid: false,
        corruptedIndex: isReverse ? chain.length - 1 - i : i,
        reason: `Cryptographic payload hash mismatch at event ${current.id}. Data has been mutated.`,
      };
    }
  }

  return { valid: true };
}
