// glossary.ts — Acme Group / Oyatie Console Centralized Enterprise Glossary & i18n Dictionary
// Maps all domain terminology, statutory legal citations, UI labels, and action tags
// into structured, type-safe namespaces for zero-code-digging terminology governance.

export interface GlossaryEntry {
  key: string;
  ko: string;
  en: string;
  legalBasis?: string; // e.g. "근로기준법 제56조", "국고금관리법 제47조 제1항"
  description: string;
  category: "common" | "org" | "rbac" | "hr" | "attendance" | "payroll" | "ops" | "approvals" | "gov";
}

export const ENTERPRISE_GLOSSARY: Record<string, GlossaryEntry> = {
  // -------------------------------------------------------------
  // 1. Common UI & Actions
  // -------------------------------------------------------------
  "common.save": {
    key: "common.save",
    ko: "저장",
    en: "Save",
    description: "양식 및 변경사항 저장",
    category: "common",
  },
  "common.cancel": {
    key: "common.cancel",
    ko: "취소",
    en: "Cancel",
    description: "작업 취소 및 모달 닫기",
    category: "common",
  },
  "common.confirm": {
    key: "common.confirm",
    ko: "확인",
    en: "Confirm",
    description: "작업 확정 및 실행",
    category: "common",
  },
  "common.edit": {
    key: "common.edit",
    ko: "수정",
    en: "Edit",
    description: "항목 내용 수정",
    category: "common",
  },
  "common.delete": {
    key: "common.delete",
    ko: "삭제",
    en: "Delete",
    description: "항목 삭제 또는 비활성화",
    category: "common",
  },
  "common.searchPlaceholder": {
    key: "common.searchPlaceholder",
    ko: "검색어 입력 (성명, 사번, 코드 등)...",
    en: "Search (Name, ID, Code)...",
    description: "공통 테이블 및 목록 검색 플레이스홀더",
    category: "common",
  },
  "common.filterAll": {
    key: "common.filterAll",
    ko: "전체",
    en: "All",
    description: "전체 선택 필터 옵션",
    category: "common",
  },
  "common.status": {
    key: "common.status",
    ko: "상태",
    en: "Status",
    description: "항목의 현재 진행 상태",
    category: "common",
  },
  "common.actions": {
    key: "common.actions",
    ko: "작업",
    en: "Actions",
    description: "테이블 작업 액션 열",
    category: "common",
  },
  "common.consequenceReview": {
    key: "common.consequenceReview",
    ko: "파급효과 사전 영향도 검토",
    en: "Consequence Pre-Commit Review",
    description: "변경사항 반영 전 4대보험, 세액, 원장 영향도 시뮬레이션",
    category: "common",
  },

  // -------------------------------------------------------------
  // 2. Organization Structure & Setup
  // -------------------------------------------------------------
  "org.entity": {
    key: "org.entity",
    ko: "법인",
    en: "Legal Entity",
    legalBasis: "상법 제169조 (회사의 정의)",
    description: "그룹 산하 개별 독립 법인 사업자",
    category: "org",
  },
  "org.site": {
    key: "org.site",
    ko: "사업장 / 현장",
    en: "Operational Site",
    legalBasis: "산업안전보건법 제15조 (안전보건관리책임자)",
    description: "물류센터, 항만터미널, 정비사업소 등 물리적 운영 거점",
    category: "org",
  },
  "org.department": {
    key: "org.department",
    ko: "부서",
    en: "Department",
    description: "조직 내 업무 기능 단위 (팀, 부서)",
    category: "org",
  },
  "org.costCenter": {
    key: "org.costCenter",
    ko: "코스트센터",
    en: "Cost Center",
    description: "경비 및 급여 예산 귀속 회계 단위",
    category: "org",
  },
  "org.bizRegNo": {
    key: "org.bizRegNo",
    ko: "사업자등록번호",
    en: "Business Registration No.",
    legalBasis: "부가가치세법 제8조",
    description: "국세청 등록 10자리 고유 사업자 번호",
    category: "org",
  },

  // -------------------------------------------------------------
  // 3. Multi-Layered RBAC & Capacities (Discord Model)
  // -------------------------------------------------------------
  "rbac.roleLayer": {
    key: "rbac.roleLayer",
    ko: "역할 레이어",
    en: "Role Layer",
    description: "사용자에게 중첩 부여되는 Discord식 직무 권한 단위",
    category: "rbac",
  },
  "rbac.actingCapacity": {
    key: "rbac.actingCapacity",
    ko: "직무 전결 자격",
    en: "Acting Capacity",
    description: "서명 및 승인 시 착용하는 결재 권한 및 한도 자격",
    category: "rbac",
  },
  "rbac.priority": {
    key: "rbac.priority",
    ko: "권한 우선순위 (Priority)",
    en: "Role Priority",
    description: "상위 우선순위 역할이 하위 역할을 오버라이드하는 서열 지수",
    category: "rbac",
  },
  "rbac.foldedCapabilities": {
    key: "rbac.foldedCapabilities",
    ko: "합성 실효 권한",
    en: "Folded Effective Capabilities",
    description: "중첩된 여러 역할 레이어가 실시간 합성된 최종 권한 세트",
    category: "rbac",
  },
  "rbac.safetySupervisorOutrank": {
    key: "rbac.safetySupervisorOutrank",
    ko: "안전감시관 즉각 작업중지권",
    en: "Safety Officer Stop-Work Authority",
    legalBasis: "산업안전보건법 제52조 (근로자의 작업중지)",
    description: "직급과 무관하게 위험 발생 시 임원 지시보다 우선하는 법정 작업중지 명령권",
    category: "rbac",
  },

  // -------------------------------------------------------------
  // 4. Human Resources & Employment
  // -------------------------------------------------------------
  "hr.dossier": {
    key: "hr.dossier",
    ko: "인사기록부 (인사원장)",
    en: "Employee Dossier",
    legalBasis: "근로기준법 제41조 (근로자 명부)",
    description: "근로자의 인적사항, 발령이력, 계약 및 직책 통합 원장",
    category: "hr",
  },
  "hr.onboardEmployee": {
    key: "hr.onboardEmployee",
    ko: "신규 입사자 등록",
    en: "Onboard Employee",
    description: "신규 채용 인원 인사원장 및 권한 등록",
    category: "hr",
  },
  "hr.personnelAction": {
    key: "hr.personnelAction",
    ko: "인사발령 (PA)",
    en: "Personnel Action (PA)",
    description: "부서전보, 직위승진, 휴직, 복직 등 공식 행정 명령",
    category: "hr",
  },
  "hr.leavePromotionRound1": {
    key: "hr.leavePromotionRound1",
    ko: "1차 연차유급휴가 사용촉진 통보",
    en: "1st Statutory Leave Promotion Notice",
    legalBasis: "근로기준법 제61조 제1항 제1호 (연차 유급휴가의 사용 촉진)",
    description: "휴가 발생일로부터 1년 끝나기 6개월 전 미사용 일수 서면 통보",
    category: "hr",
  },
  "hr.leavePromotionRound2": {
    key: "hr.leavePromotionRound2",
    ko: "2차 연차유급휴가 시기지정 통보",
    en: "2nd Statutory Leave Promotion Notice",
    legalBasis: "근로기준법 제61조 제1항 제2호 (사용자의 시기 지정권)",
    description: "근로자가 시기를 정하지 않은 경우 사용자가 휴가 일자를 직접 지정하여 서면 통보",
    category: "hr",
  },

  // -------------------------------------------------------------
  // 5. Attendance & Working Hours Governance
  // -------------------------------------------------------------
  "attendance.clockIn": {
    key: "attendance.clockIn",
    ko: "출근 시각",
    en: "Clock-In Time",
    description: "사업장 또는 현장 실제 출근 타임스탬프",
    category: "attendance",
  },
  "attendance.clockOut": {
    key: "attendance.clockOut",
    ko: "퇴근 시각",
    en: "Clock-Out Time",
    description: "사업장 또는 현장 실제 퇴근 타임스탬프",
    category: "attendance",
  },
  "attendance.workedHours": {
    key: "attendance.workedHours",
    ko: "실 근로시간",
    en: "Net Worked Hours",
    legalBasis: "근로기준법 제50조 (근로시간)",
    description: "총 체류시간에서 법정 휴게시간을 차감한 실제 인정 근로시간",
    category: "attendance",
  },
  "attendance.statutoryRestBreak": {
    key: "attendance.statutoryRestBreak",
    ko: "법정 휴게시간",
    en: "Statutory Rest Break",
    legalBasis: "근로기준법 제54조 (휴게)",
    description: "4시간 근무 시 30분, 8시간 근무 시 1시간 이상 의무 부여되는 휴게시간",
    category: "attendance",
  },
  "attendance.weekly52Limit": {
    key: "attendance.weekly52Limit",
    ko: "주 52시간 한도 관리",
    en: "Weekly 52-Hour Cap",
    legalBasis: "근로기준법 제53조 (연장 근로의 제한)",
    description: "기본 40시간 + 연장 한도 12시간을 합산한 법정 주당 최대 근로시간",
    category: "attendance",
  },
  "attendance.overtimeHours": {
    key: "attendance.overtimeHours",
    ko: "연장 근로시간",
    en: "Overtime Hours",
    legalBasis: "근로기준법 제56조 제1항 (연장근로 가산임금 50%)",
    description: "1일 8시간 또는 1주 40시간을 초과하여 근로한 시간",
    category: "attendance",
  },
  "attendance.nightHours": {
    key: "attendance.nightHours",
    ko: "야간 근로시간",
    en: "Night Shift Hours",
    legalBasis: "근로기준법 제56조 제3항 (야간근로 가산임금 50%)",
    description: "오후 10시(22:00)부터 다음 날 오전 6시(06:00) 사이에 제공된 근로",
    category: "attendance",
  },
  "attendance.discrepancyAdjudication": {
    key: "attendance.discrepancyAdjudication",
    ko: "근태 예외 소명 심사",
    en: "Attendance Discrepancy Adjudication",
    description: "누락 펀치, 연장근로 초과, 이상 근태 건에 대한 관리자 소명 및 승인 절차",
    category: "attendance",
  },

  // -------------------------------------------------------------
  // 6. Statutory Payroll & Social Insurance Engine
  // -------------------------------------------------------------
  "payroll.baseSalary": {
    key: "payroll.baseSalary",
    ko: "기본급",
    en: "Base Salary",
    legalBasis: "근로기준법 제2조 제1항 제5호 (임금의 정의)",
    description: "월 소정근로시간(209시간)에 대해 고정적으로 지급되는 기준 임금",
    category: "payroll",
  },
  "payroll.fixedAllowance": {
    key: "payroll.fixedAllowance",
    ko: "고정 제수당",
    en: "Fixed Allowance",
    description: "직책수당, 현장수당 등 매월 정기적으로 지급되는 통상수당",
    category: "payroll",
  },
  "payroll.grossPay": {
    key: "payroll.grossPay",
    ko: "지급총액 (과세대상 총급여)",
    en: "Gross Pay (Taxable Gross)",
    description: "기본급, 고정수당, 연장/야간/휴일 가산수당이 합산된 세전 총액",
    category: "payroll",
  },
  "payroll.netPay": {
    key: "payroll.netPay",
    ko: "실지급액 (차인지급액)",
    en: "Net Pay",
    description: "지급총액에서 4대보험 및 제세공과금을 공제한 후 근로자 계좌로 입금되는 실수령액",
    category: "payroll",
  },
  "payroll.statutory10WonTruncation": {
    key: "payroll.statutory10WonTruncation",
    ko: "국고금관리법 10원 미만 절사",
    en: "10-Won Statutory Truncation",
    legalBasis: "국고금관리법 제47조 제1항 (국고금의 끝수 계산)",
    description: "국고금 및 공과금 산정 시 10원 미만의 끝수는 0원으로 절사 계산하는 법정 의무",
    category: "payroll",
  },
  "payroll.nationalPension": {
    key: "payroll.nationalPension",
    ko: "국민연금",
    en: "National Pension",
    legalBasis: "국민연금법 제88조 (연금보험료의 부과·징수)",
    description: "기준소득월액의 9% (근로자 4.5% 원천징수, 사업주 4.5% 부담)",
    category: "payroll",
  },
  "payroll.healthInsurance": {
    key: "payroll.healthInsurance",
    ko: "건강보험",
    en: "National Health Insurance",
    legalBasis: "국민건강보험법 제69조 및 제76조",
    description: "보수월액의 7.09%~7.19% (근로자 절반 부담)",
    category: "payroll",
  },
  "payroll.longTermCare": {
    key: "payroll.longTermCare",
    ko: "장기요양보험",
    en: "Long-Term Care Insurance",
    legalBasis: "노인장기요양보험법 제8조 및 제9조",
    description: "건강보험료 금액의 12.81%~12.95% (근로자 절반 부담)",
    category: "payroll",
  },
  "payroll.employmentInsurance": {
    key: "payroll.employmentInsurance",
    ko: "고용보험",
    en: "Employment Insurance",
    legalBasis: "고용보험 및 산업재해보상보험의 보험료징수 등에 관한 법률 제13조",
    description: "보수총액의 0.9% (근로자 실업급여 계정 부담분)",
    category: "payroll",
  },
  "payroll.incomeTax": {
    key: "payroll.incomeTax",
    ko: "근로소득세 (간이세액표)",
    en: "Withholding Income Tax",
    legalBasis: "소득세법 제134조 (근로소득에 대한 원천징수)",
    description: "국세청 근로소득 간이세액표에 따른 원천징수 세액",
    category: "payroll",
  },
  "payroll.localIncomeTax": {
    key: "payroll.localIncomeTax",
    ko: "지방소득세",
    en: "Local Income Tax",
    legalBasis: "지방세법 제95조",
    description: "소득세 산출세액의 10%를 관할 지자체에 납부",
    category: "payroll",
  },
  "payroll.freezeGate": {
    key: "payroll.freezeGate",
    ko: "급여 마감 통제 게이트",
    en: "Payroll Freeze Gate",
    description: "근태 미승인 소명 건이 잔존할 경우 계산 확정 및 지급을 원천 차단하는 안전 게이트",
    category: "payroll",
  },
  "payroll.firmBankingCMS": {
    key: "payroll.firmBankingCMS",
    ko: "금융결제원 펌뱅킹 CMS 대량이체",
    en: "KFTC Firm Banking CMS Batch",
    description: "100바이트 고정 규격 전문 생성 및 Passkey 생체인증 기반 대량 급여 이체",
    category: "payroll",
  },

  // -------------------------------------------------------------
  // 7. Operations & Equipment Lifecycle
  // -------------------------------------------------------------
  "ops.workOrder": {
    key: "ops.workOrder",
    ko: "작업 오더 (WO)",
    en: "Work Order (WO)",
    description: "현장 장비 정비, 순회 점검, 긴급 수리 작업 지시서",
    category: "ops",
  },
  "ops.equipment": {
    key: "ops.equipment",
    ko: "중장비 / 자산 (EQ)",
    en: "Heavy Equipment / Asset (EQ)",
    description: "갠트리 크레인, 야드 트랙터, 지게차 등 자산 원장",
    category: "ops",
  },
  "ops.contract": {
    key: "ops.contract",
    ko: "도급 계약 (C)",
    en: "B2B Master Contract (C)",
    description: "발주처와의 정비용역, 하역위탁 등 원도급 계약 원장",
    category: "ops",
  },
  "ops.inspectionCycle": {
    key: "ops.inspectionCycle",
    ko: "정기 예방점검 주기",
    en: "Periodic Inspection Cycle",
    legalBasis: "산업안전보건법 제125조 (작업환경측정 및 정기안전검사)",
    description: "장비별 법정/자율 안전점검 주기 (30일, 60일, 90일)",
    category: "ops",
  },

  // -------------------------------------------------------------
  // 8. Approvals & Governance (DoA & SoD)
  // -------------------------------------------------------------
  "approvals.documentParking": {
    key: "approvals.documentParking",
    ko: "SAP 전결 문서 보관 (Parking)",
    en: "Document Parking",
    description: "원장 반영 없이 초안 상태로 안전하게 사전 저장하는 SAP Fiori 패턴",
    category: "approvals",
  },
  "approvals.sodRule": {
    key: "approvals.sodRule",
    ko: "직무 분리 규정 (SoD)",
    en: "Separation of Duties (SoD)",
    description: "기안자 본인이 스스로 승인권자가 될 수 없도록 원천 차단하는 내부통제 원칙",
    category: "approvals",
  },
  "approvals.passkeyMfa": {
    key: "approvals.passkeyMfa",
    ko: "FIDO2 / WebAuthn Passkey 서명",
    en: "Passkey Biometric Signature",
    legalBasis: "전자서명법 제3조 (전자서명의 효력)",
    description: "고액 자금 집행 및 최종 법률 행위 시 요구되는 암호학적 생체인증 서명",
    category: "approvals",
  },
  "approvals.doaCeiling": {
    key: "approvals.doaCeiling",
    ko: "전결 금액 한도 (DoA)",
    en: "Delegation of Authority Ceiling",
    description: "직무 전결 자격별 집행 가능 최대 결재 한도 (1단계 500만, 2단계 5000만 등)",
    category: "approvals",
  },

  // -------------------------------------------------------------
  // 9. Governance, Policy & Audit Chain
  // -------------------------------------------------------------
  "gov.auditChain": {
    key: "gov.auditChain",
    ko: "감사 추적 암호학적 해시 체인",
    en: "Cryptographic Audit Hash Chain",
    description: "모든 업무 행위를 이전 블록 해시와 연계 기록하여 위변조를 즉각 탐지하는 원장",
    category: "gov",
  },
  "gov.cedarPbac": {
    key: "gov.cedarPbac",
    ko: "Cedar 정책 기반 접근 제어 (PBAC)",
    en: "Cedar Policy-Based Access Control",
    description: "Amazon Cedar 정형 언어로 엄격한 DEFAULT DENY 및 격리를 강제하는 인가 엔진",
    category: "gov",
  },
  "gov.ontologyGraph": {
    key: "gov.ontologyGraph",
    ko: "Palantir Foundry 3계층 온톨로지",
    en: "Palantir Foundry 3-Layer Ontology",
    description: "사람, 근태, 결재, 작업, 자산, 계약 간 양방향 참조를 보장하는 객체 그래프",
    category: "gov",
  },
};

/**
 * Type-Safe Translation / Glossary Lookup Helper
 * Allows seamless lookup with interpolation and fallback.
 */
export function t(
  key: string,
  params?: Record<string, string | number>,
  locale: "ko" | "en" = "ko"
): string {
  const entry = ENTERPRISE_GLOSSARY[key];
  if (!entry) {
    return key;
  }

  let text = locale === "en" ? entry.en : entry.ko;
  if (params) {
    for (const [pKey, pVal] of Object.entries(params)) {
      text = text.replace(new RegExp(`{${pKey}}`, "g"), String(pVal));
    }
  }
  return text;
}

/**
 * Retrieves the full metadata entry for an enterprise term
 */
export function getGlossaryEntry(key: string): GlossaryEntry | undefined {
  return ENTERPRISE_GLOSSARY[key];
}

/**
 * Searches the glossary by keyword, category, or legal reference
 */
export function searchGlossary(query: string, category?: GlossaryEntry["category"]): GlossaryEntry[] {
  const q = query.trim().toLowerCase();
  return Object.values(ENTERPRISE_GLOSSARY).filter((entry) => {
    if (category && entry.category !== category) return false;
    if (!q) return true;
    return (
      entry.key.toLowerCase().includes(q) ||
      entry.ko.toLowerCase().includes(q) ||
      entry.en.toLowerCase().includes(q) ||
      (entry.legalBasis && entry.legalBasis.toLowerCase().includes(q)) ||
      entry.description.toLowerCase().includes(q)
    );
  });
}
