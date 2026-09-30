export type ObjectKind =
  | "AP" // 전자결재 (Approval)
  | "WO" // 정비·작업 (Work Order)
  | "AT" // 근태 예외 (Attendance Exception)
  | "C"  // 수주·도급 계약 (Contract)
  | "PR" // 프로젝트 (Project)
  | "OP" // 회사 운영·코스트센터 (Operating Cost Center)
  | "JL" // 업무일지 (Job Log)
  | "PS" // 급여명세 (Payslip)
  | "IN" // 접수·아카이브 (Intake/Archive)
  | "P"  // 직원 (Person)
  | "PA" // 인사발령 (Personnel Action)
  | "FB" // 펌뱅킹 이체 (Firm Banking Batch)
  | "EQ" // 장비 (Equipment)
  | "ROLE" // 역할 거버넌스 (Role Definition)
  | "LV"; // 연차촉진 통지 (Leave Notice)

export type StatusTone = "ok" | "warn" | "danger" | "info" | "neutral" | "brand";

export interface OntologyObject {
  id: string;
  code: string;
  kind: ObjectKind;
  title: string;
  status: string;
  statusTone: StatusTone;
  steward: string;
  createdAt: string;
  updatedAt: string;
  linkedObjects: string[];
  summary?: string;
  metadata?: Record<string, unknown>;
}

export interface Employee {
  id: string;
  code: string;
  name: string;
  email: string;
  phone: string;
  entity: string; // 법인
  site: string;   // 사업장 / 현장
  dept: string;   // 부서
  role: string;   // 직책 (호환성 필드)
  roleIds?: string[]; // Discord-like 다중 레이어 직무 역할 ID 세트
  grade?: string; // 직급 (사원, 대리, 과장, 차장, 부장, 임원)
  jobTitle?: string; // 보직/포지션 (인사기획팀장, 정비수석 등)
  position: string; // 포지션 (보안반장, 설비기사, 인사담당 등)
  empType: "정규" | "계약" | "시급" | "일당" | "파견" | "임원";
  baseSalary: number;
  fixedAllow: number;
  joinedDate: string;
  status: "재직" | "휴직" | "퇴직예정" | "퇴직";
  dependents: number;
  bankName?: string;
  bankAccount?: string;
  leaveBalance?: {
    granted: number;
    used: number;
    remaining: number;
  };
}

export interface AttendanceRecord {
  id: string;
  employeeId: string;
  employeeName: string;
  entity: string;
  site: string;
  date: string;
  clockIn: string | null;
  clockOut: string | null;
  breakMinutes: number | null;
  plannedIn: string;
  plannedOut: string;
  workedHours: number;
  otHours: number;
  nightHours: number;
  holHours: number;
  weeklyHoursTotal: number;
  status: "정상" | "지각" | "연장" | "예외승인대기" | "미출근" | "휴가";
  exceptionCode?: string; // AT-*
}

export interface ApprovalStage {
  step: number;
  approverRole: string;
  approverName: string;
  status: "pending" | "approved" | "rejected";
  timestamp?: string;
  comment?: string;
  // 직무 전결 자격 기반 전자서명 (전자서명법 제3조 및 사내 전결규정)
  actingCapacityRole?: string; // e.g. "role-doa-tier1"
  actingCapacityName?: string; // e.g. "DoA 1단계 전결관"
  authorizingGrantId?: string; // e.g. "grant-006"
  passkeyVerified?: boolean;
}

export interface ApprovalLineItem {
  id: string;
  name: string;
  qty: number;
  unitPrice: number;
  totalPrice: number;
  item?: string;
  amount?: number;
  note?: string;
}

export interface ApprovalDoc {
  id: string;
  code: string; // AP-*
  title: string;
  category: "휴가" | "지출결의" | "연장근로" | "계약승인" | "규정개정" | "기타";
  drafterId: string;
  drafterName: string;
  drafterDept: string;
  createdAt: string;
  status: "초안" | "결재대기" | "승인완료" | "반려" | "종결";
  doaAmount?: number;
  content: string;
  stages: ApprovalStage[];
  receiptRequired?: boolean;
  receiptConfirmedAt?: string;
  linkedObjects: string[];
  lineItems?: ApprovalLineItem[];
}

export interface LeaveNotice {
  id: string;
  code: string; // IN-*
  employeeId?: string;
  round: "1차" | "2차";
  employeeName: string;
  dept: string;
  remainingDays: number;
  deadline: string;
  status: "통지대기" | "발송완료" | "수령확인(Passkey)" | "노무수령거부권발동";
  confirmedAt?: string;
}

export interface ToastMessage {
  id: string;
  title: string;
  description?: string;
  tone?: "ok" | "info" | "warn" | "danger";
  actionLabel?: string;
  onAction?: () => void;
}

export interface WorkOrder {
  id: string;
  code: string; // WO-*
  title: string;
  site: string;
  priority: "긴급" | "높음" | "보통";
  status: "접수" | "배차" | "진행중" | "완료" | "검수";
  assignedTo: string;
  targetEquipment?: string;
  dueDate: string;
  contractCode?: string; // C-*
}

export interface PayDeduction {
  code: "np" | "hi" | "ltc" | "ei" | "tax" | "local";
  label: string;
  amt: number;
  basis: string;
  assumed?: boolean;
}

export interface Payslip {
  id: string;
  code: string; // PS-*
  employeeId: string;
  employeeName: string;
  yearMonth: string;
  empType: string;
  basePay: number;
  overtimePay: number;
  nightPay: number;
  holidayPay: number;
  fixedAllowance: number;
  grossPay: number;
  deductions: PayDeduction[];
  totalDeductions: number;
  netPay: number;
  confirmedAt?: string;
}

export interface PayrollRun {
  id: string;
  code: string; // PR-*
  yearMonth: string;
  entity: string;
  title: string;
  status: "준비" | "계산완료" | "마감게이트통과" | "지급승인" | "이체완료";
  totalGross: number;
  totalNet: number;
  totalDeductions: number;
  headcount: number;
  unresolvedExceptions: number;
  isLocked: boolean;
}

export interface AuditEvent {
  id: string;
  seq: number;
  prevHash: string;
  hash: string;
  timestamp: string;
  actorId: string;
  actorName: string;
  action: string;
  targetCode: string;
  targetKind: ObjectKind;
  policyDecision: "PERMIT" | "DENY" | "OVERRIDE";
  reason?: string;
  dataClass: "일반" | "대외비" | "민감" | "비밀";
}

export interface CommThread {
  id: string;
  channelId?: string;
  objectRef?: string; // e.g. AP-3121, WO-2643
  title?: string;
  lastMessage: string;
  updatedAt: string;
  unread: boolean;
  participantNames: string[];
}

export interface CommMessage {
  id: string;
  threadId: string;
  authorId: string;
  authorName: string;
  avatarText: string;
  text: string;
  timestamp: string;
  linkedCodes?: string[];
}

export interface EmailItem {
  id: string;
  sender: string;
  senderEmail: string;
  recipient: string;
  recipientEmail: string;
  subject: string;
  preview: string;
  date: string;
  isUnread: boolean;
  folder: "inbox" | "sent" | "archive" | "trash";
  linkedObject?: string;
  body: string;
}

// -------------------------------------------------------------
// 1. Organization Structure (법인, 사업장/현장, 부서, 직위)
// -------------------------------------------------------------
export interface OrgEntity {
  id: string;
  name: string;
  code: string;
  bizNumber: string; // 사업자등록번호 (123-45-67890)
  corpNumber: string; // 법인등록번호 (110111-1234567)
  ceoName: string;
  address: string;
  mainBank: string; // 주거래은행 (신한은행 등)
  mainAccount: string; // 법인 모계좌번호 (펌뱅킹 출금계좌)
  establishedDate: string;
  status: "active" | "inactive";
}

export interface OrgSite {
  id: string;
  name: string;
  code: string;
  entityId: string;
  category: "본사" | "물류센터" | "항만터미널" | "정비사업소" | "제철운영소" | "기타";
  address: string;
  siteManager: string;
  safetyManager: string;
  phone: string;
  activeHeadcount: number;
}

export interface OrgDepartment {
  id: string;
  name: string;
  code: string;
  entityId: string;
  division: string; // 상위 본부
  managerName: string;
  costCenter: string;
}

// -------------------------------------------------------------
// 2. Policy Setup (사규, 전결, 근태/주52시간, 연차 규정)
// -------------------------------------------------------------
export interface AttendancePolicy {
  standardWeeklyHours: number; // 기본 40시간
  standardDailyHours: number;  // 기본 8시간
  maxWeeklyHours: number;      // 법정 주 52시간
  warningWeeklyHours: number;  // 경고 임계 48시간
  statutoryRestBreak4hMinutes: number; // 30분
  statutoryRestBreak8hMinutes: number; // 60분
  nightShiftStart: string;     // "22:00"
  nightShiftEnd: string;       // "06:00"
  overtimePreApprovalRequired: boolean; // 사전 승인제
}

export interface DoaPolicyTier {
  id: string;
  maxAmount: number; // 5,000,000 | 50,000,000 | Infinity
  label: string;
  approverRole: string;
  requirePasskey: boolean;
}

export interface LeavePolicy {
  accrualBasis: "fiscal_year" | "hire_date"; // 회계연도(1.1) vs 입사일
  firstYearMonthlyGrant: boolean; // 1년 미만 매월 1일 부여
  standardAnnualGrant: number;    // 15일
  statutoryPromotionRound1MonthsBefore: number; // 6개월 전
  statutoryPromotionRound2MonthsBefore: number; // 2개월 전
}

export interface OrgPolicy {
  id: string;
  version: string;
  attendance: AttendancePolicy;
  doaTiers: DoaPolicyTier[];
  leave: LeavePolicy;
  updatedAt: string;
  updatedBy: string;
}

// -------------------------------------------------------------
// 3. Operations & Equipment (도급계약, 장비/설비, 점검주기)
// -------------------------------------------------------------
export interface Contract {
  id: string;
  code: string; // C-*
  title: string;
  client: string; // 발주처/원청
  site: string;
  contractType: "도급" | "위수탁" | "정비용역" | "화물운송";
  monthlyAmount: number;
  startDate: string;
  endDate: string;
  assignedHeadcount: number;
  status: "유효" | "만료임박" | "종료" | "검토중";
}

export interface Equipment {
  id: string;
  code: string; // EQ-*
  name: string;
  site: string;
  category: "지게차" | "항만트랙터" | "크레인" | "특수정비차량" | "공조설비" | "기타";
  serialNo: string;
  inspectionCycleDays: number;
  lastInspectionDate: string;
  nextInspectionDate: string;
  assignedEngineer: string;
  status: "정상가동" | "정비중" | "점검요망" | "휴지";
}

// -------------------------------------------------------------
// 4. HR Actions Processing (인사명령 발령 대장)
// -------------------------------------------------------------
export type PersonnelActionType =
  | "신규채용"
  | "부서전보"
  | "직책임명"
  | "승진"
  | "연봉계약갱신"
  | "휴직"
  | "복직"
  | "퇴직";

export interface PersonnelAction {
  id: string;
  code: string; // PA-*
  employeeId: string;
  employeeName: string;
  actionType: PersonnelActionType;
  effectiveDate: string;
  orderNumber: string; // 제2026-07-04호
  reason: string;
  prevDetails: {
    dept?: string;
    role?: string;
    site?: string;
    baseSalary?: number;
    status?: string;
  };
  newDetails: {
    dept?: string;
    role?: string;
    site?: string;
    baseSalary?: number;
    status?: string;
  };
  approvedBy: string;
  createdAt: string;
}

// -------------------------------------------------------------
// 5. Payroll Bank Transfer Batch & Firm Banking (KFTC CMS 100-byte)
// -------------------------------------------------------------
export interface BankBatchSummary {
  bankName: string;
  bankCode: string; // 088 (신한), 004 (국민), 081 (하나), 020 (우리), 003 (기업), 011 (농협)
  count: number;
  amount: number;
}

export interface BankTransferBatch {
  id: string;
  code: string; // FB-*
  payrollRunId: string;
  yearMonth: string;
  paymentDate: string;
  totalHeadcount: number;
  totalAmount: number;
  masterBank: string;
  masterAccount: string;
  bankSummaries: BankBatchSummary[];
  preflightVerified: boolean;
  preflightSuccessRate: number; // e.g. 100%
  kftcFlatFileContent: string;  // 금융결제원 100바이트 표준 펌뱅킹 전문 레이아웃
  openApiPayload: Record<string, unknown>; // 차세대 Open Banking REST API 규격
  status: "준비" | "검증완료" | "전문생성" | "출금승인대기" | "이체지시완료";
  sealedAt?: string;
  sealedBy?: string;
  passkeyHash?: string;
}

// -------------------------------------------------------------
// 6. Sub-Entity Decomposition (Person vs. Employment vs. OrgAssignment)
// -------------------------------------------------------------
export interface PersonIdentity {
  id: string;
  code: string; // P-*
  name: string;
  email: string;
  phone: string;
  residentIdMasked?: string;
}

export interface EmploymentContract {
  id: string;
  personId: string;
  entityId: string;
  entityName: string;
  empType: "정규" | "계약" | "시급" | "일당" | "파견" | "임원";
  joinedDate: string;
  probationEndDate?: string;
  baseSalary: number;
  fixedAllow: number;
  standardHoursPerWeek: number;
  bankName: string;
  bankAccount: string;
  status: "재직" | "휴직" | "퇴직예정" | "퇴직";
}

export interface OrgAssignment {
  id: string;
  employmentId: string;
  siteId: string;
  siteName: string;
  deptId: string;
  deptName: string;
  teamId?: string;
  teamName?: string;
  role: string;
  position: string;
  assignedDate: string;
  effectiveDate: string;
  isPrimary: boolean;
}

export interface UserAccount {
  id: string;
  personId: string;
  username: string;
  roleGroup: string;
  mfaEnabled: boolean;
  lastLoginAt: string;
}

// -------------------------------------------------------------
// 7. Connected Discrepancy & Consequence Preview Models
// -------------------------------------------------------------
export interface PayrollDiscrepancy {
  id: string;
  payrollRunId: string;
  employeeId: string;
  employeeName: string;
  sourceKind: "AT" | "EMP" | "C" | "TAX";
  sourceCode: string; // e.g. AT-0828-02
  title: string;
  statutoryRule: string;
  explanation: string;
  severity: "blocking" | "advisory";
  status: "unresolved" | "resolved";
  canAutoRepair: boolean;
  repairActionLabel: string;
  resolutionComment?: string;
  resolvedAt?: string;
  resolvedBy?: string;
}

export interface ConsequenceSummary {
  title: string;
  actionType: string;
  subject: string;
  effectiveDate: string;
  summaryLines: string[];
  historicalImpact: string;
  requiresApproval: boolean;
  approverRole: string;
  consequenceButtonLabel: string;
}

export interface PayrollDraftState {
  savedAt: string;
  yearMonth: string;
  filterQuery: string;
  activeTab: "payslip" | "banking" | "sheet";
  selectedSlipId?: string;
  unresolvedCount: number;
}

// -------------------------------------------------------------
// 8. Connected Sheet & Spreadsheet Engine Models
// -------------------------------------------------------------
export type CellRole =
  | "linked-source"        // Linked authoritative record (e.g. employment start date, contracted salary)
  | "business-input"       // Editable proposed input in sheet (e.g. proposed allowance, bonus)
  | "calculated-result"    // Owner-calculated business result (e.g. calculated gross, net pay)
  | "sheet-formula"        // User/planner defined spreadsheet formula expression
  | "analytical-input"     // Local planning assumption / note (no schema change needed)
  | "object-reference";    // Real business object pointer (e.g. department, site)

export interface SheetColumn<T = any> {
  id: string;
  header: string;
  role: CellRole;
  width: number;
  type: "text" | "money" | "number" | "date" | "status" | "reference" | "formula";
  formula?: string;
  description?: string;
  provenance?: string; // e.g. "근로계약서 (EC-2026-001) 및 인사명령 원장"
  getValue: (row: T) => any;
  format?: (val: any) => string;
  isEditable?: boolean;
  referenceKind?: "department" | "site" | "employee";
}

export interface ProposedChange {
  rowKey: string;           // Stable object identity (e.g. employeeId)
  columnId: string;         // Field ID (e.g. "proposedAllowance")
  previousValue: any;
  proposedValue: any;
  status: "draft" | "submitted" | "applied" | "rejected";
  validationError?: string;
  timestamp: string;
}

export interface SheetCellCoordinate {
  rowKey: string;
  colIndex: number;
  columnId: string;
}

export interface RectangularSelection {
  startRowKey: string;
  startColIndex: number;
  endRowKey: string;
  endColIndex: number;
}

// -------------------------------------------------------------
// 9. Cedar Policy Engine & Access Governance Models
// -------------------------------------------------------------
export type CedarEffect = "permit" | "forbid";

export interface CedarPolicyStatement {
  id: string;
  title: string;
  description: string;
  effect: CedarEffect;
  principalScope: {
    roleGroup?: string;
    accountId?: string;
    isWildcard?: boolean;
  };
  actionScope: string[];
  resourceScope: {
    kinds: string[];
    entityId?: string; // "all" or specific entity
  };
  condition?: {
    requireMfa?: boolean;
    maxAmount?: number;
    enforceSeparationOfDuties?: boolean; // principal.person != resource.preparedByPerson
    validFrom?: string;
    validTo?: string;
  };
  isSystemDefault: boolean;
  createdAt: string;
}

export interface CapabilityBundle {
  id: string;
  name: string;
  category: "HR" | "Payroll" | "Approvals" | "Operations" | "Governance";
  description: string;
  actions: string[];
  suggestedDoATier?: string;
  requiresPasskeyDefault?: boolean;
}

export interface AccessGrantAssignment {
  id: string;
  assigneeType: "account" | "role_group";
  assigneeId: string;
  assigneeName: string;
  personId?: string;
  capabilityBundleId: string;
  capabilityBundleName: string;
  entityScope: string; // "all" | "ENT-01" | "ENT-02"
  validFrom: string;
  validTo: string;
  justification: string;
  grantedBy: string;
  grantedAt: string;
  status: "active" | "revoked" | "expired";
}

export interface AccessEvaluationRequest {
  principalAccountId: string;
  principalPersonId: string;
  principalRoleGroup: string;
  principalEntityId?: string;
  action: string;
  resource: {
    id: string;
    code?: string;
    kind: string;
    entityId?: string;
    entityName?: string;
    siteId?: string;
    deptId?: string;
    preparedByPersonId?: string;
    preparedByName?: string;
    amount?: number;
    dataClassification?: "일반" | "대외비" | "민감" | "비밀";
  };
  context: {
    time?: string;
    mfaVerified?: boolean;
    delegatedFromPersonId?: string;
    networkZone?: string;
  };
}

export interface AccessExplanationResult {
  decision: "PERMIT" | "FORBID";
  summary: string;
  evaluatedAt: string;
  principal: {
    accountId: string;
    username: string;
    personId: string;
    personName: string;
    roleGroup: string;
  };
  action: string;
  resource: {
    id: string;
    code: string;
    kind: string;
    entityName: string;
    preparedByName?: string;
  };
  matchedPermitRules: Array<{
    id: string;
    title: string;
    scope: string;
  }>;
  blockingForbidRules: Array<{
    id: string;
    title: string;
    violationReason: string;
  }>;
  remediationAdvice?: {
    canSelfRemediate: boolean;
    recommendedAction: string;
    actionType: "passkey_register" | "grant_request" | "handoff_delegate" | "switch_entity";
  };
}

export interface OperationalIncident {
  id: string;
  category: "banking_cert" | "import_validation" | "batch_interrupted" | "approval_deadlock";
  title: string;
  severity: "critical" | "warning";
  description: string;
  affectedEntity: string;
  createdAt: string;
  status: "open" | "resolved";
  recoveryActionLabel: string;
  resolutionNote?: string;
  resolvedAt?: string;
  metadata?: Record<string, unknown>;
}

// -------------------------------------------------------------
// 10. Discord-Style Multi-Layered Role Engine (Think of Discord)
// -------------------------------------------------------------
export interface RoleDefinition {
  id: string;
  name: string;
  color: string; // Hex color (e.g. #6366F1, #10B981, #F59E0B, #EC4899, #8B5CF6)
  badgeTone: StatusTone;
  iconName: string; // e.g. Shield, KeyRound, DollarSign, Wrench, FileCheck2, Flame, Building
  priority: number; // 0 to 100 (higher priority displays first and outranks)
  category: "functional" | "positional" | "jurisdiction" | "responsibility" | "delegation";
  scopeType: "platform" | "group" | "entity" | "site" | "department";
  grantedCapabilities: string[]; // List of actions allowed by this role
  conditions?: {
    requireMfa?: boolean;
    maxAmount?: number;
    validityWindowRequired?: boolean;
  };
  description: string;
  isSystemDefault?: boolean;
}

export interface RoleAssignment {
  id: string;
  personId: string; // P-*
  employeeId: string;
  roleId: string;
  roleName: string;
  scope: {
    type: "all" | "entity" | "site" | "department";
    targetId?: string;
    targetName?: string;
  };
  validFrom: string;
  validTo?: string; // null or empty for permanent
  grantedBy: string;
  grantReason: string;
  status: "active" | "expired" | "revoked";
}

export interface EffectivePermissionFold {
  personId: string;
  personName: string;
  targetScope: {
    entityId?: string;
    siteId?: string;
    deptId?: string;
  };
  layeredRoles: Array<{
    role: RoleDefinition;
    assignment: RoleAssignment;
  }>;
  effectiveCapabilities: Set<string>;
  actingCapacities: Array<{
    roleId: string;
    roleName: string;
    color: string;
    maxAmount?: number;
    requiresPasskey?: boolean;
    justification: string;
  }>;
  evaluationTimestamp: string;
}

