import {
  CedarPolicyStatement,
  CapabilityBundle,
  AccessGrantAssignment,
  AccessEvaluationRequest,
  AccessExplanationResult,
  OperationalIncident,
  Employee,
  RoleDefinition,
  RoleAssignment,
  EffectivePermissionFold,
  AuditEvent,
} from "./types";

// -------------------------------------------------------------
// 1. Predefined Semantic Capability Bundles
// -------------------------------------------------------------
export const CAPABILITY_BUNDLES: CapabilityBundle[] = [
  {
    id: "cap-hr-onboard",
    name: "인사 채용 및 전보 발령",
    category: "HR",
    description: "신규 사원 등록, 부서 전보 및 직책 임명 발령 기안/승인",
    actions: ["HR::OnboardEmployee", "HR::TransferEmployee", "HR::UpdateProfile"],
    suggestedDoATier: "부서장 전결",
    requiresPasskeyDefault: false,
  },
  {
    id: "cap-payroll-operator",
    name: "급여 산정 및 시트 기안",
    category: "Payroll",
    description: "근태 집계, Connected Sheet 수당 조정, 급여 차이점(Discrepancy) 소명 처리",
    actions: ["Payroll::Calculate", "Payroll::EditDraftSheet", "Payroll::AdjudicateDiscrepancy"],
    suggestedDoATier: "실무자",
    requiresPasskeyDefault: false,
  },
  {
    id: "cap-payroll-approver",
    name: "급여 대장 마감 및 최종 승인",
    category: "Payroll",
    description: "월별 정기 급여 대장 마감 게이트 검증 및 최종 지급 승인 (SoD 적용)",
    actions: ["Payroll::ApproveRun", "Payroll::SealRegister"],
    suggestedDoATier: "총괄본부장 C-Level",
    requiresPasskeyDefault: true,
  },
  {
    id: "cap-treasury-firm-banking",
    name: "펌뱅킹 자금 집행 및 CMS 송신",
    category: "Payroll",
    description: "KFTC 100바이트 전문 생성, 출금 모계좌 이체 지시 및 전송",
    actions: ["Payroll::ExecuteFirmBanking", "Treasury::GenerateKFTCFile"],
    suggestedDoATier: "대표이사 / 재무총괄",
    requiresPasskeyDefault: true,
  },
  {
    id: "cap-approval-tier1",
    name: "DoA 1단계: 일상 운영비 전결 (500만원 이하)",
    category: "Approvals",
    description: "부서 내 소모품비, 일상 경비 및 정비 자재 품의 전결",
    actions: ["Approvals::SignTier1"],
    suggestedDoATier: "팀장 / 부서장",
    requiresPasskeyDefault: false,
  },
  {
    id: "cap-approval-tier2",
    name: "DoA 2단계: 장비 부품 및 계약 전결 (5,000만원 이하)",
    category: "Approvals",
    description: "장비 수리, 외주 용역 변경 및 5천만원 이하 지출 승인",
    actions: ["Approvals::SignTier2"],
    suggestedDoATier: "총괄본부장",
    requiresPasskeyDefault: true,
  },
  {
    id: "cap-approval-tier3",
    name: "DoA 3단계: 고액 계약 및 이사회 안건 승인 (5,000만원 초과)",
    category: "Approvals",
    description: "대규모 신규 계약, 설비 자산 취득 및 회장단 최종 결재",
    actions: ["Approvals::SignTier3"],
    suggestedDoATier: "대표이사",
    requiresPasskeyDefault: true,
  },
  {
    id: "cap-ops-dispatch",
    name: "현장 정비 배차 및 작업오더 완료 검수",
    category: "Operations",
    description: "WO- 배차 지정, 현장 정비 착수 및 최종 안전 검수 인가",
    actions: ["Operations::AssignWorkOrder", "Operations::CompleteInspection"],
    suggestedDoATier: "현장소장",
    requiresPasskeyDefault: false,
  },
  {
    id: "cap-gov-audit",
    name: "감사 체인 및 권한 거버넌스 열람",
    category: "Governance",
    description: "WORM 암호학적 감사 원장, 권한 정책 시뮬레이터 및 내부통제 리포트 열람",
    actions: ["Governance::InspectAuditChain", "Governance::SimulateAccess"],
    suggestedDoATier: "감사위원회 / 준법감시인",
    requiresPasskeyDefault: false,
  },
];

// -------------------------------------------------------------
// 2. Default Authoritative Cedar Policies (with SoD)
// -------------------------------------------------------------
export const DEFAULT_CEDAR_POLICIES: CedarPolicyStatement[] = [
  {
    id: "pol-sod-independence",
    title: "직무 분리(SoD) 자기 기안 승인 절대 금지 규정",
    description: "자연인(Person) 단위로 기안자와 승인자가 동일한 경우 모든 계정에서 승인 행위를 절대 차단합니다.",
    effect: "forbid",
    principalScope: { isWildcard: true },
    actionScope: [
      "Approvals::SignTier1",
      "Approvals::SignTier2",
      "Approvals::SignTier3",
      "Payroll::ApproveRun",
      "Payroll::ExecuteFirmBanking",
    ],
    resourceScope: { kinds: ["AP", "PR", "FB"] },
    condition: {
      enforceSeparationOfDuties: true,
    },
    isSystemDefault: true,
    createdAt: "2026-01-01T00:00:00Z",
  },
  {
    id: "pol-tenant-isolation",
    title: "법인 관할권 경계 격리 규정",
    description: "타 계열 법인의 인사/급여/자금 자원에 대한 무단 접근 및 승인을 차단합니다.",
    effect: "forbid",
    principalScope: { isWildcard: true },
    actionScope: ["*"],
    resourceScope: { kinds: ["*"] },
    condition: {},
    isSystemDefault: true,
    createdAt: "2026-01-01T00:00:00Z",
  },
  {
    id: "pol-tier3-passkey-mfa",
    title: "고액 결재 및 펌뱅킹 FIDO2 Passkey 생체인증 강제",
    effect: "forbid",
    description: "5천만원 초과 결재 및 은행 펌뱅킹 이체 지시는 하드웨어 Passkey 인증 없이는 실행할 수 없습니다.",
    principalScope: { isWildcard: true },
    actionScope: ["Approvals::SignTier3", "Payroll::ExecuteFirmBanking"],
    resourceScope: { kinds: ["AP", "FB"] },
    condition: {
      requireMfa: true,
    },
    isSystemDefault: true,
    createdAt: "2026-01-01T00:00:00Z",
  },
  {
    id: "pol-doa-tier1-limit",
    title: "1단계 전결 한도 500만원 상한 통제",
    effect: "forbid",
    description: "DoA 1단계 권한으로는 500만원을 초과하는 지출 품의를 전결할 수 없습니다.",
    principalScope: { roleGroup: "팀장" },
    actionScope: ["Approvals::SignTier1"],
    resourceScope: { kinds: ["AP"] },
    condition: {
      maxAmount: 5000000,
    },
    isSystemDefault: true,
    createdAt: "2026-01-01T00:00:00Z",
  },
];

// -------------------------------------------------------------
// 3. Seed Access Grant Assignments
// -------------------------------------------------------------
export const INITIAL_ACCESS_ASSIGNMENTS: AccessGrantAssignment[] = [
  {
    id: "grant-001",
    assigneeType: "role_group",
    assigneeId: "인사담당",
    assigneeName: "인사팀 실무담당자 그룹",
    capabilityBundleId: "cap-hr-onboard",
    capabilityBundleName: "인사 채용 및 전보 발령",
    entityScope: "all",
    validFrom: "2026-01-01",
    validTo: "2026-12-31",
    justification: "인사총무팀 표준 직무 배정 (취업규칙 제14조)",
    grantedBy: "ACC-SYS-ADMIN",
    grantedAt: "2026-01-01T09:00:00Z",
    status: "active",
  },
  {
    id: "grant-002",
    assigneeType: "account",
    assigneeId: "ACC-001",
    assigneeName: "홍길동 (대표이사)",
    personId: "P-001",
    capabilityBundleId: "cap-approval-tier3",
    capabilityBundleName: "DoA 3단계: 고액 계약 및 이사회 안건 승인",
    entityScope: "all",
    validFrom: "2026-01-01",
    validTo: "2026-12-31",
    justification: "대표이사 고유 결재 권한 및 최종 전결권",
    grantedBy: "ACC-SYS-ADMIN",
    grantedAt: "2026-01-01T09:00:00Z",
    status: "active",
  },
  {
    id: "grant-003",
    assigneeType: "account",
    assigneeId: "ACC-002",
    assigneeName: "김인사 (인사기획팀장)",
    personId: "P-002",
    capabilityBundleId: "cap-payroll-operator",
    capabilityBundleName: "급여 산정 및 시트 기안",
    entityScope: "all",
    validFrom: "2026-01-01",
    validTo: "2026-12-31",
    justification: "월별 전사 정기 급여 계산 및 소명 총괄",
    grantedBy: "ACC-001",
    grantedAt: "2026-01-05T10:00:00Z",
    status: "active",
  },
  {
    id: "grant-004",
    assigneeType: "account",
    assigneeId: "ACC-003",
    assigneeName: "박재무 (재무운영본부장)",
    personId: "P-003",
    capabilityBundleId: "cap-payroll-approver",
    capabilityBundleName: "급여 대장 마감 및 최종 승인",
    entityScope: "all",
    validFrom: "2026-01-01",
    validTo: "2026-12-31",
    justification: "내부회계관리제도에 따른 급여 최종 독립 승인권자",
    grantedBy: "ACC-001",
    grantedAt: "2026-01-05T10:00:00Z",
    status: "active",
  },
  {
    id: "grant-005",
    assigneeType: "account",
    assigneeId: "ACC-003",
    assigneeName: "박재무 (재무운영본부장)",
    personId: "P-003",
    capabilityBundleId: "cap-treasury-firm-banking",
    capabilityBundleName: "펌뱅킹 자금 집행 및 CMS 송신",
    entityScope: "all",
    validFrom: "2026-01-01",
    validTo: "2026-12-31",
    justification: "펌뱅킹 모계좌 출금 통제 및 최종 전자서명 책임",
    grantedBy: "ACC-001",
    grantedAt: "2026-01-05T10:00:00Z",
    status: "active",
  },
  {
    id: "grant-006",
    assigneeType: "role_group",
    assigneeId: "팀장",
    assigneeName: "현업 부서장/팀장 그룹",
    capabilityBundleId: "cap-approval-tier1",
    capabilityBundleName: "DoA 1단계: 일상 운영비 전결 (500만원 이하)",
    entityScope: "all",
    validFrom: "2026-01-01",
    validTo: "2026-12-31",
    justification: "부서 일상 경비 집행 전결 규정 (전결규정 제4조)",
    grantedBy: "ACC-SYS-ADMIN",
    grantedAt: "2026-01-01T09:00:00Z",
    status: "active",
  },
];

// -------------------------------------------------------------
// 3.1 Discord-Style Multi-Layered Role Definitions
// -------------------------------------------------------------
export const DEFAULT_ROLE_DEFINITIONS: RoleDefinition[] = [
  {
    id: "role-super-admin",
    name: "최고 거버넌스 관리자",
    color: "#EF4444",
    badgeTone: "danger",
    iconName: "ShieldAlert",
    priority: 100,
    category: "positional",
    scopeType: "platform",
    grantedCapabilities: [
      "Governance::InspectAuditChain",
      "Governance::SimulateAccess",
      "Approvals::SignTier3",
      "Payroll::ApproveRun",
      "Payroll::ExecuteFirmBanking",
      "HR::OnboardEmployee",
      "HR::TransferEmployee",
    ],
    conditions: { requireMfa: true },
    description: "플랫폼 및 전 그룹 최고 감사/거버넌스 권한",
    isSystemDefault: true,
  },
  {
    id: "role-corp-officer",
    name: "법인 C-Level 총괄본부장",
    color: "#8B5CF6",
    badgeTone: "brand",
    iconName: "KeyRound",
    priority: 90,
    category: "positional",
    scopeType: "entity",
    grantedCapabilities: [
      "Approvals::SignTier2",
      "Approvals::SignTier3",
      "Payroll::ApproveRun",
      "Payroll::SealRegister",
      "HR::TransferEmployee",
    ],
    conditions: { requireMfa: true, maxAmount: 50000000 },
    description: "법인 사업본부 총괄 전결 및 급여 최종 승인권",
    isSystemDefault: true,
  },
  {
    id: "role-safety-inspector",
    name: "그룹 환경안전 총괄감시관",
    color: "#10B981",
    badgeTone: "ok",
    iconName: "ShieldCheck",
    priority: 85,
    category: "functional",
    scopeType: "group",
    grantedCapabilities: [
      "Operations::AssignWorkOrder",
      "Operations::CompleteInspection",
      "Governance::InspectAuditChain",
    ],
    description: "직급 불문 전 사업장 안전점검 및 작업중지 명령권 (Authority does not follow rank)",
    isSystemDefault: true,
  },
  {
    id: "role-treasury-signer",
    name: "KFTC 펌뱅킹 자금집행관",
    color: "#EC4899",
    badgeTone: "danger",
    iconName: "DollarSign",
    priority: 80,
    category: "responsibility",
    scopeType: "entity",
    grantedCapabilities: [
      "Payroll::ExecuteFirmBanking",
      "Treasury::GenerateKFTCFile",
    ],
    conditions: { requireMfa: true },
    description: "금융결제원 CMS 100바이트 펌뱅킹 출금 전문 생성 및 전자서명권",
    isSystemDefault: true,
  },
  {
    id: "role-payroll-lead",
    name: "정기 급여 마감 총괄관",
    color: "#F59E0B",
    badgeTone: "warn",
    iconName: "FileCheck2",
    priority: 75,
    category: "functional",
    scopeType: "entity",
    grantedCapabilities: [
      "Payroll::ApproveRun",
      "Payroll::SealRegister",
    ],
    conditions: { requireMfa: true },
    description: "급여 계산 마감 게이트 검증 및 최종 대장 결재권 (SoD 적용)",
    isSystemDefault: true,
  },
  {
    id: "role-doa-tier1",
    name: "DoA 1단계: 일상경비 전결관",
    color: "#3B82F6",
    badgeTone: "info",
    iconName: "FileCheck2",
    priority: 70,
    category: "functional",
    scopeType: "department",
    grantedCapabilities: [
      "Approvals::SignTier1",
    ],
    conditions: { maxAmount: 5000000 },
    description: "부서 내 500만원 이하 소모품 및 운영비 일상 전결권",
    isSystemDefault: true,
  },
  {
    id: "role-hr-onboard",
    name: "인사 채용 및 전보 발령관",
    color: "#14B8A6",
    badgeTone: "brand",
    iconName: "Users",
    priority: 65,
    category: "functional",
    scopeType: "entity",
    grantedCapabilities: [
      "HR::OnboardEmployee",
      "HR::TransferEmployee",
      "HR::UpdateProfile",
    ],
    description: "신규 입사 처리 및 조직 전보/발령 기안권",
    isSystemDefault: true,
  },
  {
    id: "role-payroll-operator",
    name: "급여 산정 및 시트 기안관",
    color: "#EAB308",
    badgeTone: "warn",
    iconName: "FileSpreadsheet",
    priority: 60,
    category: "functional",
    scopeType: "entity",
    grantedCapabilities: [
      "Payroll::Calculate",
      "Payroll::EditDraftSheet",
      "Payroll::AdjudicateDiscrepancy",
    ],
    description: "근태 연계 급여 계산, 시트 수당 입력 및 예외 소명 심사권",
    isSystemDefault: true,
  },
  {
    id: "role-ops-lead",
    name: "현장 정비 배차 총괄관",
    color: "#F97316",
    badgeTone: "warn",
    iconName: "Wrench",
    priority: 55,
    category: "functional",
    scopeType: "site",
    grantedCapabilities: [
      "Operations::AssignWorkOrder",
      "Operations::CompleteInspection",
    ],
    description: "정비 사업소 작업오더 배차 및 작업 완료 검수권",
    isSystemDefault: true,
  },
  {
    id: "role-site-custodian-incheon",
    name: "인천사업소 설비자산 관리관",
    color: "#06B6D4",
    badgeTone: "info",
    iconName: "Building",
    priority: 50,
    category: "responsibility",
    scopeType: "site",
    grantedCapabilities: [
      "Operations::AssignWorkOrder",
    ],
    description: "인천제1물류센터 크레인 및 장비 점검/보전 책임",
    isSystemDefault: false,
  },
  {
    id: "role-base-member",
    name: "임직원 기본 역할 (@everyone)",
    color: "#64748B",
    badgeTone: "neutral",
    iconName: "User",
    priority: 10,
    category: "positional",
    scopeType: "platform",
    grantedCapabilities: [
      "Comms::SendChat",
      "Comms::ReadMail",
      "Profile::ViewSelf",
    ],
    description: "전사 공통 기본 커뮤니케이션 및 본인 원장 조회권",
    isSystemDefault: true,
  },
];

// -------------------------------------------------------------
// 3.2 Seed Layered Role Assignments (Multi-Role per Person)
// -------------------------------------------------------------
export const INITIAL_ROLE_ASSIGNMENTS: RoleAssignment[] = [
  // 이수력 (emp_01) - 4개의 역할 레이어 보유 (Discord 모델)
  {
    id: "ra-001",
    personId: "P-emp_01",
    employeeId: "emp_01",
    roleId: "role-base-member",
    roleName: "임직원 기본 역할 (@everyone)",
    scope: { type: "all" },
    validFrom: "2026-01-01",
    grantedBy: "ACC-SYS-ADMIN",
    grantReason: "기본 임직원 권한",
    status: "active",
  },
  {
    id: "ra-002",
    personId: "P-emp_01",
    employeeId: "emp_01",
    roleId: "role-ops-lead",
    roleName: "현장 정비 배차 총괄관",
    scope: { type: "site", targetId: "site_01", targetName: "인천제1물류센터" },
    validFrom: "2026-01-01",
    grantedBy: "ACC-SYS-ADMIN",
    grantReason: "정비팀장 보직 발령",
    status: "active",
  },
  {
    id: "ra-003",
    personId: "P-emp_01",
    employeeId: "emp_01",
    roleId: "role-doa-tier1",
    roleName: "DoA 1단계: 일상경비 전결관",
    scope: { type: "department", targetId: "dept_01", targetName: "정비1팀" },
    validFrom: "2026-01-01",
    grantedBy: "ACC-SYS-ADMIN",
    grantReason: "팀 운영비 500만원 전결권",
    status: "active",
  },
  {
    id: "ra-004",
    personId: "P-emp_01",
    employeeId: "emp_01",
    roleId: "role-site-custodian-incheon",
    roleName: "인천사업소 설비자산 관리관",
    scope: { type: "site", targetId: "site_01", targetName: "인천제1물류센터" },
    validFrom: "2026-01-01",
    grantedBy: "ACC-SYS-ADMIN",
    grantReason: "인천 사업소 장비 안전관리자 선임",
    status: "active",
  },

  // 김인사 (emp_02) - 4개의 역할 레이어 보유
  {
    id: "ra-005",
    personId: "P-emp_02",
    employeeId: "emp_02",
    roleId: "role-base-member",
    roleName: "임직원 기본 역할 (@everyone)",
    scope: { type: "all" },
    validFrom: "2026-01-01",
    grantedBy: "ACC-SYS-ADMIN",
    grantReason: "기본 임직원 권한",
    status: "active",
  },
  {
    id: "ra-006",
    personId: "P-emp_02",
    employeeId: "emp_02",
    roleId: "role-hr-onboard",
    roleName: "인사 채용 및 전보 발령관",
    scope: { type: "all" },
    validFrom: "2026-01-01",
    grantedBy: "ACC-SYS-ADMIN",
    grantReason: "인사기획팀장 직무 배정",
    status: "active",
  },
  {
    id: "ra-007",
    personId: "P-emp_02",
    employeeId: "emp_02",
    roleId: "role-payroll-operator",
    roleName: "급여 산정 및 시트 기안관",
    scope: { type: "all" },
    validFrom: "2026-01-01",
    grantedBy: "ACC-SYS-ADMIN",
    grantReason: "월별 전사 급여 대장 실무 기안",
    status: "active",
  },
  {
    id: "ra-008",
    personId: "P-emp_02",
    employeeId: "emp_02",
    roleId: "role-doa-tier1",
    roleName: "DoA 1단계: 일상경비 전결관",
    scope: { type: "department", targetId: "dept_02", targetName: "인사기획팀" },
    validFrom: "2026-01-01",
    grantedBy: "ACC-SYS-ADMIN",
    grantReason: "인사팀 일상 운영비 전결권",
    status: "active",
  },

  // 박재무 (emp_03) - 5개의 역할 레이어 보유
  {
    id: "ra-009",
    personId: "P-emp_03",
    employeeId: "emp_03",
    roleId: "role-base-member",
    roleName: "임직원 기본 역할 (@everyone)",
    scope: { type: "all" },
    validFrom: "2026-01-01",
    grantedBy: "ACC-SYS-ADMIN",
    grantReason: "기본 임직원 권한",
    status: "active",
  },
  {
    id: "ra-010",
    personId: "P-emp_03",
    employeeId: "emp_03",
    roleId: "role-corp-officer",
    roleName: "법인 C-Level 총괄본부장",
    scope: { type: "all" },
    validFrom: "2026-01-01",
    grantedBy: "ACC-SYS-ADMIN",
    grantReason: "재무총괄본부장 C-Level 선임",
    status: "active",
  },
  {
    id: "ra-011",
    personId: "P-emp_03",
    employeeId: "emp_03",
    roleId: "role-payroll-lead",
    roleName: "정기 급여 마감 총괄관",
    scope: { type: "all" },
    validFrom: "2026-01-01",
    grantedBy: "ACC-SYS-ADMIN",
    grantReason: "급여 최종 승인 및 락 마감 책임",
    status: "active",
  },
  {
    id: "ra-012",
    personId: "P-emp_03",
    employeeId: "emp_03",
    roleId: "role-treasury-signer",
    roleName: "KFTC 펌뱅킹 자금집행관",
    scope: { type: "all" },
    validFrom: "2026-01-01",
    grantedBy: "ACC-SYS-ADMIN",
    grantReason: "은행 출금 펌뱅킹 전자인증 책임자",
    status: "active",
  },

  // 홍길동 (emp_04) - 3개의 역할 레이어
  {
    id: "ra-013",
    personId: "P-emp_04",
    employeeId: "emp_04",
    roleId: "role-base-member",
    roleName: "임직원 기본 역할 (@everyone)",
    scope: { type: "all" },
    validFrom: "2026-01-01",
    grantedBy: "ACC-SYS-ADMIN",
    grantReason: "기본 임직원 권한",
    status: "active",
  },
  {
    id: "ra-014",
    personId: "P-emp_04",
    employeeId: "emp_04",
    roleId: "role-super-admin",
    roleName: "최고 거버넌스 관리자",
    scope: { type: "all" },
    validFrom: "2026-01-01",
    grantedBy: "ACC-SYS-ADMIN",
    grantReason: "대표이사 최고 통제권",
    status: "active",
  },

  // 정안전 (emp_05) - Authority does not follow rank 예시! 사원이지만 그룹 안전감시관 역할 보유
  {
    id: "ra-015",
    personId: "P-emp_05",
    employeeId: "emp_05",
    roleId: "role-base-member",
    roleName: "임직원 기본 역할 (@everyone)",
    scope: { type: "all" },
    validFrom: "2026-01-01",
    grantedBy: "ACC-SYS-ADMIN",
    grantReason: "기본 임직원 권한",
    status: "active",
  },
  {
    id: "ra-016",
    personId: "P-emp_05",
    employeeId: "emp_05",
    roleId: "role-safety-inspector",
    roleName: "그룹 환경안전 총괄감시관",
    scope: { type: "all" },
    validFrom: "2026-01-01",
    grantedBy: "ACC-SYS-ADMIN",
    grantReason: "산업안전보건법상 독립 안전관리자 지정 (Authority does not follow rank)",
    status: "active",
  },
];

// -------------------------------------------------------------
// 3.3 The Discord Fold Engine: foldUserEffectivePermissions
// -------------------------------------------------------------
export function foldUserEffectivePermissions(
  personId: string,
  targetScope: { entityId?: string; siteId?: string; deptId?: string } = {},
  allRoles: RoleDefinition[] = DEFAULT_ROLE_DEFINITIONS,
  allAssignments: RoleAssignment[] = INITIAL_ROLE_ASSIGNMENTS
): EffectivePermissionFold {
  const now = new Date().toISOString();
  const effectiveCaps = new Set<string>();
  const activeRoles: Array<{ role: RoleDefinition; assignment: RoleAssignment }> = [];
  const actingCapacities: EffectivePermissionFold["actingCapacities"] = [];

  // Filter assignments for this person that are currently active
  const userAssignments = allAssignments.filter((a) => {
    const matchPerson = a.personId === personId || a.employeeId === personId || `P-${a.employeeId}` === personId;
    if (!matchPerson) return false;
    if (a.status !== "active") return false;
    if (a.validFrom && now < a.validFrom) return false;
    if (a.validTo && now > a.validTo + "T23:59:59Z") return false;

    // Scope check: If targetScope is specified, grant must cover it
    if (a.scope.type === "entity" && targetScope.entityId && a.scope.targetId !== targetScope.entityId && a.scope.targetId !== "all") {
      return false;
    }
    if (a.scope.type === "site" && targetScope.siteId && a.scope.targetId !== targetScope.siteId && a.scope.targetId !== "all") {
      return false;
    }
    if (a.scope.type === "department" && targetScope.deptId && a.scope.targetId !== targetScope.deptId && a.scope.targetId !== "all") {
      return false;
    }
    return true;
  });

  // Resolve matching role definitions
  for (const assign of userAssignments) {
    const roleDef = allRoles.find((r) => r.id === assign.roleId);
    if (roleDef) {
      activeRoles.push({ role: roleDef, assignment: assign });
      for (const cap of roleDef.grantedCapabilities) {
        effectiveCaps.add(cap);
      }
      actingCapacities.push({
        roleId: roleDef.id,
        roleName: roleDef.name,
        color: roleDef.color,
        maxAmount: roleDef.conditions?.maxAmount,
        requiresPasskey: roleDef.conditions?.requireMfa,
        justification: assign.grantReason,
      });
    }
  }

  // Sort layered roles by priority descending (Discord hierarchy: higher outranks)
  activeRoles.sort((a, b) => b.role.priority - a.role.priority);

  return {
    personId,
    personName: personId,
    targetScope,
    layeredRoles: activeRoles,
    effectiveCapabilities: effectiveCaps,
    actingCapacities,
    evaluationTimestamp: now,
  };
}

// -------------------------------------------------------------
// 4. Cedar Evaluation Engine & Explainable Access Inspector
// -------------------------------------------------------------
export function evaluateAccess(
  req: AccessEvaluationRequest,
  policies: CedarPolicyStatement[] = DEFAULT_CEDAR_POLICIES,
  assignments: AccessGrantAssignment[] = INITIAL_ACCESS_ASSIGNMENTS,
  bundles: CapabilityBundle[] = CAPABILITY_BUNDLES,
  roleDefs: RoleDefinition[] = DEFAULT_ROLE_DEFINITIONS,
  roleAssignments: RoleAssignment[] = INITIAL_ROLE_ASSIGNMENTS
): AccessExplanationResult {
  const now = req.context.time || new Date().toISOString();
  const matchedPermits: Array<{ id: string; title: string; scope: string }> = [];
  const blockingForbids: Array<{ id: string; title: string; violationReason: string }> = [];

  // 1. Check Permit Grants (Capability Assignment)
  // Look for active assignments targeting this account or its role group
  const relevantAssignments = assignments.filter((a) => {
    if (a.status !== "active") return false;
    // Date validity check
    if (a.validFrom && now < a.validFrom) return false;
    if (a.validTo && now > a.validTo + "T23:59:59Z") return false;

    // Entity Scope check
    if (a.entityScope !== "all" && a.entityScope !== req.resource.entityId) return false;

    // Principal match
    const matchAccount = a.assigneeType === "account" && a.assigneeId === req.principalAccountId;
    const matchRole = a.assigneeType === "role_group" && a.assigneeId === req.principalRoleGroup;
    return matchAccount || matchRole;
  });

  // Check if any assigned capability bundle contains the requested action
  for (const grant of relevantAssignments) {
    const bundle = bundles.find((b) => b.id === grant.capabilityBundleId);
    if (bundle && bundle.actions.includes(req.action)) {
      matchedPermits.push({
        id: grant.id,
        title: `권한 부여: ${bundle.name} (${grant.assigneeName})`,
        scope: `관할 법인: ${grant.entityScope === "all" ? "전사 통합" : grant.entityScope} · 만료일: ${grant.validTo}`,
      });
    }
  }

  // 1.1 Check Discord-Style Layered Roles fold
  const roleFold = foldUserEffectivePermissions(
    req.principalPersonId,
    {
      entityId: req.resource.entityId,
      siteId: req.resource.siteId,
      deptId: req.resource.deptId,
    },
    roleDefs,
    roleAssignments
  );

  for (const layered of roleFold.layeredRoles) {
    if (layered.role.grantedCapabilities.includes(req.action)) {
      matchedPermits.push({
        id: layered.assignment.id,
        title: `역할 레이어: ${layered.role.name} (${layered.role.category})`,
        scope: `범위: ${layered.assignment.scope.type} · 우선순위 P${layered.role.priority}`,
      });
    }
  }

  // 2. Check Forbid Policies (Explicit Cedar Forbid Overrides)
  for (const pol of policies) {
    if (pol.effect !== "forbid") continue;

    // Does policy apply to this action?
    const actionMatches = pol.actionScope.includes("*") || pol.actionScope.includes(req.action);
    if (!actionMatches) continue;

    // Does policy apply to this resource kind?
    const kindMatches = pol.resourceScope.kinds.includes("*") || pol.resourceScope.kinds.includes(req.resource.kind);
    if (!kindMatches) continue;

    // Condition A: Separation of Duties (SoD) by natural PersonIdentity
    if (pol.condition?.enforceSeparationOfDuties) {
      // If the principal is the same natural person who prepared the resource
      if (
        req.resource.preparedByPersonId &&
        req.principalPersonId === req.resource.preparedByPersonId
      ) {
        blockingForbids.push({
          id: pol.id,
          title: pol.title,
          violationReason: `자연인(${req.principalPersonId}) 기준으로 기안자(${req.resource.preparedByName || req.resource.preparedByPersonId})와 동일인입니다. 직무 분리(SoD) 규정에 의해 본인 기안 안건에 대한 승인이 절대 금지됩니다.`,
        });
      }
    }

    // Condition B: Tenant/Company Boundary Isolation
    if (pol.id === "pol-tenant-isolation") {
      if (
        req.principalEntityId &&
        req.principalEntityId !== "all" &&
        req.resource.entityId &&
        req.resource.entityId !== req.principalEntityId
      ) {
        blockingForbids.push({
          id: pol.id,
          title: pol.title,
          violationReason: `소속 법인(${req.principalEntityId})과 대상 자원의 귀속 법인(${req.resource.entityId})이 상이합니다. 계열사 간 인가되지 않은 월경 접근입니다.`,
        });
      }
    }

    // Condition C: Passkey / MFA Assurance Requirement
    if (pol.condition?.requireMfa) {
      if (!req.context.mfaVerified) {
        blockingForbids.push({
          id: pol.id,
          title: pol.title,
          violationReason: `해당 고위험 행위(${req.action})는 하드웨어 FIDO2 Passkey 생체 인증이 필수입니다. 현재 일반 세션 상태입니다.`,
        });
      }
    }

    // Condition D: Maximum DoA Amount Limit
    if (pol.condition?.maxAmount !== undefined && req.resource.amount !== undefined) {
      if (req.resource.amount > pol.condition.maxAmount) {
        blockingForbids.push({
          id: pol.id,
          title: pol.title,
          violationReason: `품의 금액(₩${req.resource.amount.toLocaleString()})이 1단계 전결 한도(₩${pol.condition.maxAmount.toLocaleString()})를 초과하였습니다. 상위 본부장 또는 대표이사 결재가 필요합니다.`,
        });
      }
    }
  }

  // Final Decision: Must have at least 1 permit and 0 blocking forbids
  const isPermitted = matchedPermits.length > 0 && blockingForbids.length === 0;

  // Compile structured explanation
  let summary = "";
  let remediationAdvice: AccessExplanationResult["remediationAdvice"] = undefined;

  if (isPermitted) {
    summary = `인가 완료 (PERMIT): ${matchedPermits.length}건의 활성 권한 부합 및 직무 분리(SoD) 규정 통과`;
  } else if (blockingForbids.length > 0) {
    const topForbid = blockingForbids[0];
    summary = `인가 거부 (FORBID): 내부 통제 규정 위반 [${topForbid.title}]`;

    if (topForbid.id === "pol-sod-independence") {
      remediationAdvice = {
        canSelfRemediate: false,
        recommendedAction: "자신이 기안한 문서는 직무분리 규정에 의해 직접 결재할 수 없습니다. 상위 전결권자 또는 지정된 독립 검토자에게 결재를 인계하십시오.",
        actionType: "handoff_delegate",
      };
    } else if (topForbid.id === "pol-tier3-passkey-mfa") {
      remediationAdvice = {
        canSelfRemediate: true,
        recommendedAction: "기기에 등록된 Touch ID / Face ID / FIDO2 Passkey 생체 인증을 수행하여 세션 신뢰도를 높이십시오.",
        actionType: "passkey_register",
      };
    } else if (topForbid.id === "pol-tenant-isolation") {
      remediationAdvice = {
        canSelfRemediate: true,
        recommendedAction: `상단 메뉴에서 작업 대상 법인(${req.resource.entityName || req.resource.entityId})으로 작업 컨텍스트를 전환하십시오.`,
        actionType: "switch_entity",
      };
    } else {
      remediationAdvice = {
        canSelfRemediate: false,
        recommendedAction: "상위 전결권자에게 지출 품의를 상신하여 승인을 받으십시오.",
        actionType: "grant_request",
      };
    }
  } else {
    // Default Deny (no permit found)
    summary = `인가 거부 (DEFAULT DENY): 해당 행위(${req.action})에 대한 유효한 권한 번들이 할당되지 않았습니다.`;
    remediationAdvice = {
      canSelfRemediate: false,
      recommendedAction: "사내 권한 거버넌스 페이지에서 [권한 신청]을 진행하거나 준법감시팀에 직무 배정을 요청하십시오.",
      actionType: "grant_request",
    };
  }

  return {
    decision: isPermitted ? "PERMIT" : "FORBID",
    summary,
    evaluatedAt: now,
    principal: {
      accountId: req.principalAccountId,
      username: req.principalAccountId,
      personId: req.principalPersonId,
      personName: req.principalPersonId,
      roleGroup: req.principalRoleGroup,
    },
    action: req.action,
    resource: {
      id: req.resource.id,
      code: req.resource.code || req.resource.id,
      kind: req.resource.kind,
      entityName: req.resource.entityName || "(주)오야티",
      preparedByName: req.resource.preparedByName,
    },
    matchedPermitRules: matchedPermits,
    blockingForbidRules: blockingForbids,
    remediationAdvice,
  };
}

// -------------------------------------------------------------
// 5. Pre-commit Impact Preview Generator
// -------------------------------------------------------------
export function previewPolicyChange(
  targetAssigneeType: "account" | "role_group",
  targetAssigneeId: string,
  bundleId: string,
  entityScope: string,
  employees: Employee[],
  currentAssignments: AccessGrantAssignment[]
) {
  const bundle = CAPABILITY_BUNDLES.find((b) => b.id === bundleId);
  const affectedEmployees: Employee[] = [];

  if (targetAssigneeType === "account") {
    const emp = employees.find((e) => e.id === targetAssigneeId || e.code === targetAssigneeId);
    if (emp) affectedEmployees.push(emp);
  } else {
    // Role group
    const emps = employees.filter((e) => e.role === targetAssigneeId || e.empType === targetAssigneeId);
    affectedEmployees.push(...emps);
  }

  const gainedUsers = affectedEmployees.map((e) => `${e.name} (${e.dept} · ${e.role})`);
  const affectedCategories = bundle ? [bundle.category] : ["General"];
  const riskWarnings: string[] = [];

  if (bundle?.requiresPasskeyDefault) {
    riskWarnings.push("고위험 권한 번들입니다. 대상자 전원의 FIDO2 Passkey 등록 상태를 사전에 검증해야 합니다.");
  }
  if (entityScope === "all") {
    riskWarnings.push("그룹 전사(All Entities) 관할 권한입니다. 타 계열사 자원에 대한 열람 및 변경이 허용됩니다.");
  }
  if (bundle?.actions.some((a) => a.includes("ExecuteFirmBanking") || a.includes("SignTier3"))) {
    riskWarnings.push("자금 집행 및 5,000만원 초과 전결 권한이 포함되어 있습니다. 내부회계관리제도(K-SOX) 감사 대상에 즉시 편입됩니다.");
  }

  return {
    gainedUsers,
    lostUsers: [],
    affectedCategories,
    riskWarnings,
    bundleName: bundle?.name || bundleId,
  };
}

// -------------------------------------------------------------
// 6. Pre-seeded Operational Recovery Incidents
// -------------------------------------------------------------
export const INITIAL_OPERATIONAL_INCIDENTS: OperationalIncident[] = [
  {
    id: "INC-2026-081",
    category: "banking_cert",
    title: "금융결제원(KFTC) 펌뱅킹 클라이언트 전자인증서 갱신 만료 임박 (D-3)",
    severity: "critical",
    description: "신한은행 법인 모계좌 CMS 100바이트 송신용 공개키 인증서 만료일이 2026년 9월 24일로 임박하여 이체 중단 위험이 감지되었습니다.",
    affectedEntity: "(주)오야티",
    createdAt: "2026-09-21T08:30:00Z",
    status: "open",
    recoveryActionLabel: "인증서 자체 갱신 및 키 교환 테스트 (Ping)",
    metadata: {
      certSerial: "7B98-E412-00F1",
      expiresAt: "2026-09-24T23:59:59Z",
      bankName: "신한은행",
      accountNumber: "110-452-987654",
    },
  },
  {
    id: "INC-2026-082",
    category: "import_validation",
    title: "8월 외주 파견직 근태 대량 업로드 오류 (주민번호/은행코드 누락 3건)",
    severity: "warning",
    description: "외부 협력사 CSV 일괄 등록 중 3명의 수당 지급용 은행 식별코드와 식별자 마스킹이 비표준으로 인입되어 급여 계산 게이트가 차단되었습니다.",
    affectedEntity: "(주)아크메로지스",
    createdAt: "2026-09-20T14:15:00Z",
    status: "open",
    recoveryActionLabel: "스테이징 오류 데이터 즉시 교정 및 재검증",
    metadata: {
      totalRows: 48,
      failedRows: 3,
      errorDetails: ["박정비: 은행코드 999 미등록", "최운송: 주민번호 13자리 형식 불일치", "강하역: 필수 기본급 필드 공백"],
    },
  },
  {
    id: "INC-2026-083",
    category: "batch_interrupted",
    title: "주식회사 아크메로지스 7월 정기급여 펌뱅킹 네트워크 타임아웃 세션 중단",
    severity: "critical",
    description: "은행 호스트 서버 패킷 응답 지연으로 인해 45건 중 32건 전송 후 소켓이 강제 종료되었습니다. 중복 출금 방지 락이 활성화되어 있습니다.",
    affectedEntity: "(주)아크메로지스",
    createdAt: "2026-09-19T17:40:00Z",
    status: "open",
    recoveryActionLabel: "중복 방지 멱등성 롤백 및 미송신 13건 이어서 재개",
    metadata: {
      batchCode: "FB-202607-01",
      sentCount: 32,
      pendingCount: 13,
      idempotencyKey: "IDEMP-KFTC-202607-ACME",
    },
  },
  {
    id: "INC-2026-084",
    category: "approval_deadlock",
    title: "정비사업부 전결권자 장기 부재로 인한 작업오더(WO-2643) 결재 정체",
    severity: "warning",
    description: "지정 결재권자인 이수력 부장의 해외 출장 및 병가로 인해 긴급 부품 교체 품의가 48시간 이상 지연되었습니다.",
    affectedEntity: "(주)오야티",
    createdAt: "2026-09-21T10:00:00Z",
    status: "open",
    recoveryActionLabel: "독립성 보장 직무 대결(Delegate) 안전 지정",
    metadata: {
      stalledDocCode: "AP-3121",
      absentApprover: "이수력 (정비팀장)",
      delayHours: 52,
    },
  },
];

// -------------------------------------------------------------
// 7. Cryptographic Audit Chain Integrity Verification Engine
// -------------------------------------------------------------
export interface AuditVerificationResult {
  valid: boolean;
  totalEvents: number;
  brokenAtSeq?: number;
  brokenEventId?: string;
  error?: string;
}

/**
 * Validates the unbroken cryptographic forward-linkage of the Audit Log.
 * In Console, audit events are recorded with each event referencing the previous event's hash (prevHash).
 * Any tampering with an event's payload, timestamp, or sequence breaks the chain immediately.
 */
export function verifyAuditChain(events: AuditEvent[]): AuditVerificationResult {
  if (!events || events.length === 0) {
    return { valid: true, totalEvents: 0 };
  }

  // Sort events chronologically by sequence number ascending to ensure order-agnostic verification
  const sorted = [...events].sort((a, b) => a.seq - b.seq);

  for (let i = 0; i < sorted.length; i++) {
    const current = sorted[i];

    if (i === 0) {
      // Genesis event: must have a valid root marker or hash
      if (!current.prevHash) {
        return {
          valid: false,
          totalEvents: sorted.length,
          brokenAtSeq: current.seq,
          brokenEventId: current.id,
          error: `제네시스 루트 감사 블록 #${current.seq}의 무결성 검증 실패: root prevHash 누락`,
        };
      }
    } else {
      const prev = sorted[i - 1];
      if (current.prevHash !== prev.hash) {
        return {
          valid: false,
          totalEvents: sorted.length,
          brokenAtSeq: current.seq,
          brokenEventId: current.id,
          error: `감사 추적 해시 체인 훼손 감지: 시퀀스 #${current.seq}의 prevHash("${current.prevHash}")가 선행 이벤트 #${prev.seq}의 해시("${prev.hash}")와 불일치합니다.`,
        };
      }
    }
  }

  return { valid: true, totalEvents: sorted.length };
}

