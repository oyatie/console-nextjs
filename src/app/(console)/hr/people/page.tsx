"use client";

import React, { useState } from "react";
import { useAppStore } from "@/lib/store";
import { DataTable, Column } from "@/components/ui/DataTable";
import { StatusChip } from "@/components/ui/StatusChip";
import { ObjectLink } from "@/components/ui/ObjectLink";
import { Button } from "@/components/ui/Button";
import { StatBar } from "@/components/ui/StatBar";
import { Employee, ConsequenceSummary } from "@/lib/types";
import { MoneyInput } from "@/components/ui/MoneyInput";
import { ConsequenceModal } from "@/components/ui/ConsequenceModal";
import { DiscordRoleBadge } from "@/components/gov/DiscordRoleBadge";
import { RoleDefinition } from "@/lib/types";
import { ModuleScopeFilter, ModuleScopeState } from "@/components/gov/ModuleScopeFilter";
import {
  Users,
  UserPlus,
  Building,
  Mail,
  Phone,
  Calendar,
  CreditCard,
  Clock,
  Wrench,
  CheckCircle2,
  Edit3,
  Save,
  X,
  Briefcase,
  ExternalLink,
  MessageSquare,
  ArrowRightLeft,
  ShieldCheck,
  FileText,
  Layers,
  Plus,
} from "lucide-react";

export default function PeoplePage() {
  const employees = useAppStore((s) => s.employees);
  const workOrders = useAppStore((s) => s.workOrders);
  const attendance = useAppStore((s) => s.attendance);
  const orgEntities = useAppStore((s) => s.orgEntities);
  const orgSites = useAppStore((s) => s.orgSites);
  const roleDefinitions = useAppStore((s) => s.roleDefinitions);
  const roleAssignments = useAppStore((s) => s.roleAssignments);
  const assignRoleToEmployee = useAppStore((s) => s.assignRoleToEmployee);
  const removeRoleFromEmployee = useAppStore((s) => s.removeRoleFromEmployee);
  const getEffectivePermissionsForEmployee = useAppStore((s) => s.getEffectivePermissionsForEmployee);
  const createEmployee = useAppStore((s) => s.createEmployee);
  const updateEmployee = useAppStore((s) => s.updateEmployee);
  const transferEmployee = useAppStore((s) => s.transferEmployee);
  const pasteExcelEmployees = useAppStore((s) => s.pasteExcelEmployees);
  const openObjectInspector = useAppStore((s) => s.openObjectInspector);
  const addToast = useAppStore((s) => s.addToast);

  const [onboardingModalOpen, setOnboardingModalOpen] = useState(false);
  const [selectedEmpId, setSelectedEmpId] = useState<string | null>(null);
  const [isEditing, setIsEditing] = useState(false);
  const [transferModalOpen, setTransferModalOpen] = useState(false);
  const [isConsequenceOpen, setIsConsequenceOpen] = useState(false);
  const [transferDept, setTransferDept] = useState("물류운영팀");
  const [transferSite, setTransferSite] = useState(orgSites[0]?.name || "평택 항만 복합물류센터");
  const [transferReason, setTransferReason] = useState("현장 거점 전문 인력 확충에 따른 부서 전보 발령");
  const [transferEffectiveDate, setTransferEffectiveDate] = useState("2026-08-01");

  // Discord role assignment UI state
  const [roleAssignOpen, setRoleAssignOpen] = useState(false);
  const [selectedRoleToAdd, setSelectedRoleToAdd] = useState("");

  // 모듈 관할 인가 범위 필터 상태
  const [scopeFilter, setScopeFilter] = useState<ModuleScopeState>({
    entityId: "all",
    siteId: "all",
    deptId: "all",
  });

  // 현재 선택된 사원 객체 (store에서 최신 상태 구독)
  const selectedEmp = employees.find((e) => e.id === selectedEmpId) || null;

  // 관할 범위에 따른 사원 목록 필터링
  const filteredEmployees = React.useMemo(() => {
    return employees.filter((e) => {
      if (scopeFilter.entityId !== "all" && e.entity !== scopeFilter.entityId) return false;
      if (scopeFilter.siteId !== "all" && e.site !== scopeFilter.siteId) return false;
      if (scopeFilter.deptId !== "all" && e.dept !== scopeFilter.deptId) return false;
      return true;
    });
  }, [employees, scopeFilter]);

  // 실효 인가 권한 실시간 산출 (요청 시점 동적 폴드)
  const selectedEmpFold = React.useMemo(() => {
    if (!selectedEmp) return null;
    return getEffectivePermissionsForEmployee(selectedEmp.id);
  }, [selectedEmp, getEffectivePermissionsForEmployee, roleAssignments, roleDefinitions]);

  // 인라인 수정용 폼 상태
  const [editSalary, setEditSalary] = useState<number>(0);
  const [editDept, setEditDept] = useState("");
  const [editRole, setEditRole] = useState("");
  const [editSite, setEditSite] = useState("");
  const [editStatus, setEditStatus] = useState<Employee["status"]>("재직");

  const transferConsequenceSummary: ConsequenceSummary = {
    title: "임직원 부서 전보 및 인사 발령",
    actionType: "부서전보",
    subject: `${selectedEmp?.name || "사원"} (${selectedEmp?.code || "EMP"})`,
    effectiveDate: `${transferEffectiveDate} (발령 즉시 효력)`,
    summaryLines: [
      `소속 부서 변경: ${selectedEmp?.dept} (${selectedEmp?.site}) → ${transferDept} (${transferSite})`,
      "인사발령 대장(PA-*)에 제2026-08호 전자발령 명령 번호 부여 및 감사 원장 자동 기표",
      "발령 부서에 따른 근태 관리자 및 현장 작업오더 배차 승인 권한이 즉시 전환됩니다.",
      "기존 수행 중인 작업오더 및 결재선은 이전 부서 이력으로 안전하게 동결 보존됩니다.",
    ],
    historicalImpact: "본 인사 전보 내역은 감사 해시체인(SHA-256)과 인사기록 카드에 영구 보존됩니다.",
    requiresApproval: true,
    approverRole: "인사총괄 / 최고인사책임자(CHRO)",
    consequenceButtonLabel: "부서 전보 및 인사발령 확정",
  };

  const handleConfirmTransfer = () => {
    if (!selectedEmp) return;
    transferEmployee(selectedEmp.id, transferDept, transferSite, transferEffectiveDate, transferReason);
    setIsConsequenceOpen(false);
  };

  const startEditing = (emp: Employee) => {
    setEditSalary(emp.baseSalary);
    setEditDept(emp.dept);
    setEditRole(emp.role);
    setEditSite(emp.site);
    setEditStatus(emp.status);
    setIsEditing(true);
  };

  const saveEditing = () => {
    if (!selectedEmp) return;
    updateEmployee(selectedEmp.id, {
      baseSalary: editSalary,
      dept: editDept,
      role: editRole,
      site: editSite,
      status: editStatus,
    });
    setIsEditing(false);
    addToast({
      title: "인사 정보 변경 완료",
      description: `${selectedEmp.name} 사원의 인사 정보 및 급여 기준이 즉시 갱신되었습니다.`,
      tone: "ok",
    });
  };

  // 신규 사원 등록 폼 상태
  const [name, setName] = useState("");
  const [email, setEmail] = useState("");
  const [phone, setPhone] = useState("010-");
  const [entity, setEntity] = useState(orgEntities[0]?.name || "");
  const [site, setSite] = useState(orgSites[0]?.name || "");
  const [dept, setDept] = useState("운영지원팀");
  const [role, setRole] = useState("사원");
  const [position, setPosition] = useState("현장운영");
  const [empType, setEmpType] = useState<Employee["empType"]>("정규");
  const [baseSalary, setBaseSalary] = useState<number>(3500000);
  const [fixedAllow, setFixedAllow] = useState<number>(300000);
  const [dependents, setDependents] = useState<number>(1);
  const [bankName, setBankName] = useState("신한은행");
  const [bankAccount, setBankAccount] = useState("110-384-918231");

  const handleCreateSubmit = (e: React.FormEvent) => {
    e.preventDefault();
    if (!name.trim()) return;

    const created = createEmployee({
      name,
      email: email || `${name.toLowerCase()}@oyatie.com`,
      phone,
      entity,
      site,
      dept,
      role,
      position,
      empType,
      baseSalary,
      fixedAllow,
      joinedDate: new Date().toISOString().slice(0, 10),
      status: "재직",
      dependents,
      bankName,
      bankAccount,
      leaveBalance: { granted: 15, used: 0, remaining: 15 },
    });

    setOnboardingModalOpen(false);
    setSelectedEmpId(created.id);
    setName("");
  };

  const columns: Column<Employee>[] = [
    {
      key: "code",
      header: "사번",
      width: "100px",
      render: (row) => <ObjectLink code={row.code} />,
    },
    {
      key: "name",
      header: "성명",
      width: "90px",
      render: (row) => <span className="font-bold text-[var(--ink)]">{row.name}</span>,
    },
    {
      key: "entity",
      header: "소속 법인",
      render: (row) => <span className="text-[var(--steel)]">{row.entity}</span>,
    },
    {
      key: "site",
      header: "배치 현장",
      render: (row) => <span className="text-[var(--steel)]">{row.site}</span>,
    },
    {
      key: "dept",
      header: "부서 · 직책",
      render: (row) => (
        <span>
          {row.dept} · <span className="font-medium text-[var(--ink)]">{row.grade ? `${row.grade} (${row.role})` : row.role}</span>
        </span>
      ),
    },
    {
      key: "roles",
      header: "보유 다중 역할 (Discord Model)",
      width: "280px",
      render: (row) => {
        const userAssignments = roleAssignments.filter(
          (a) => (a.employeeId === row.id || a.personId === `P-${row.id}`) && a.status === "active"
        );
        const assignedRoles = userAssignments
          .map((a) => roleDefinitions.find((r) => r.id === a.roleId))
          .filter((r): r is RoleDefinition => Boolean(r));

        return (
          <div className="flex flex-wrap items-center gap-1">
            {assignedRoles.length === 0 ? (
              <span className="text-[11px] text-[var(--steel)]">기본 임직원</span>
            ) : (
              assignedRoles.map((role) => (
                <DiscordRoleBadge key={role.id} role={role} size="sm" />
              ))
            )}
          </div>
        );
      },
    },
    {
      key: "empType",
      header: "고용 형태",
      width: "80px",
      render: (row) => <StatusChip label={row.empType} size="xs" />,
    },
    {
      key: "baseSalary",
      header: "기본급",
      align: "right",
      render: (row) => <span className="font-mono">{row.baseSalary.toLocaleString()}원</span>,
    },
    {
      key: "status",
      header: "재직 상태",
      width: "80px",
      render: (row) => <StatusChip label={row.status} size="xs" dot />,
    },
  ];

  // 사원 관련 연계 데이터 추출
  const empAttendance = selectedEmp
    ? attendance.find((a) => a.employeeId === selectedEmp.id)
    : null;
  const empWorkOrders = selectedEmp
    ? workOrders.filter((w) => w.assignedTo === selectedEmp.name)
    : [];

  return (
    <div className="space-y-5">
      {/* 1. 상단 통계 바 */}
      <StatBar
        items={[
          { label: "총 인원 대장", value: `${employees.length}명` },
          {
            label: "정규직 비율",
            value: `${Math.round(
              (employees.filter((e) => e.empType === "정규").length / Math.max(1, employees.length)) * 100
            )}%`,
            badge: "법정 안정성",
            badgeTone: "ok",
          },
          {
            label: "현장 배치 인력",
            value: `${employees.filter((e) => e.site.includes("물류") || e.site.includes("항만")).length}명`,
            badgeTone: "info",
          },
          { label: "월 인건비 기준", value: "3,850만원", subValue: "기본급 + 정액수당" },
        ]}
      />

      {/* 2. 사원 목록 및 등록 액션 바 */}
      <div className="flex items-center justify-between">
        <div className="text-xs text-[var(--steel)]">
          사원 행을 클릭하면 Workday 스타일의 <strong>사원 360° 인사 원장 프로필</strong>이 우측에 열립니다.
        </div>
        <Button
          size="sm"
          variant="brand"
          leftIcon={<UserPlus className="w-3.5 h-3.5" />}
          onClick={() => setOnboardingModalOpen(true)}
        >
          신규 사원 등록
        </Button>
      </div>

      {/* 2.5 모듈 레벨 관할 인가 범위 필터 */}
      <ModuleScopeFilter
        value={scopeFilter}
        onChange={setScopeFilter}
        filteredCount={filteredEmployees.length}
        totalCount={employees.length}
      />

      {/* 3. 사원 대장 테이블 */}
      <DataTable
        columns={columns}
        data={filteredEmployees}
        keyExtractor={(item) => item.id}
        onRowClick={(item) => {
          setSelectedEmpId(item.id);
          setIsEditing(false);
        }}
        onPasteRows={(rows) => pasteExcelEmployees(rows)}
        searchPlaceholder="성명, 부서, 직책, 사번 검색..."
        searchFilter={(item, q) =>
          item.name.toLowerCase().includes(q.toLowerCase()) ||
          item.dept.toLowerCase().includes(q.toLowerCase()) ||
          item.code.toLowerCase().includes(q.toLowerCase()) ||
          item.site.toLowerCase().includes(q.toLowerCase())
        }
      />

      {/* 4. Workday 스타일 사원 360° 인사 원장 드로어 */}
      {selectedEmp && (
        <div className="fixed inset-0 z-50 flex justify-end bg-black/40 backdrop-blur-2xs">
          <div className="w-full max-w-lg h-full bg-[var(--surface)] border-l border-[var(--border)] shadow-2xl flex flex-col animate-in slide-in-from-right duration-200">
            {/* 드로어 헤더 */}
            <div className="p-4 border-b border-[var(--border)] flex items-center justify-between bg-[var(--canvas)]">
              <div className="flex items-center gap-3">
                <div className="w-10 h-10 rounded-full bg-[var(--brand)] text-white flex items-center justify-center font-bold text-sm shadow-xs">
                  {selectedEmp.name.slice(0, 1)}
                </div>
                <div>
                  <div className="flex items-center gap-2">
                    <h3 className="text-base font-bold text-[var(--ink)]">{selectedEmp.name}</h3>
                    <StatusChip label={selectedEmp.status} size="xs" dot />
                  </div>
                  <div className="flex items-center gap-2 text-xs text-[var(--steel)]">
                    <ObjectLink code={selectedEmp.code} />
                    <span>•</span>
                    <span>{selectedEmp.dept}</span>
                    <span>•</span>
                    <span className="font-medium text-[var(--ink)]">{selectedEmp.role}</span>
                  </div>
                </div>
              </div>

              <div className="flex items-center gap-1">
                {!isEditing ? (
                  <Button
                    size="xs"
                    variant="secondary"
                    leftIcon={<Edit3 className="w-3 h-3" />}
                    onClick={() => startEditing(selectedEmp)}
                  >
                    인사 변경
                  </Button>
                ) : (
                  <Button
                    size="xs"
                    variant="brand"
                    leftIcon={<Save className="w-3 h-3" />}
                    onClick={saveEditing}
                  >
                    저장
                  </Button>
                )}
                <button
                  onClick={() => {
                    setSelectedEmpId(null);
                    setIsEditing(false);
                  }}
                  className="p-1.5 rounded hover:bg-[var(--muted)] text-[var(--steel)] hover:text-[var(--ink)] cursor-pointer"
                >
                  <X className="w-4 h-4" />
                </button>
              </div>
            </div>

            {/* 드로어 스크롤 본문 */}
            <div className="flex-1 overflow-y-auto p-4 space-y-4 text-xs">
              {/* 편집 모드 양식 */}
              {isEditing ? (
                <div className="rounded-lg border border-[var(--brand)]/40 bg-[var(--brand)]/5 p-3.5 space-y-3">
                  <div className="font-bold text-xs text-[var(--ink)] flex items-center gap-1.5">
                    <Edit3 className="w-3.5 h-3.5 text-[var(--brand)]" />
                    <span>인사 발령 및 급여 계약 변경</span>
                  </div>
                  <div className="grid grid-cols-2 gap-2 text-xs">
                    <div>
                      <label className="font-semibold block mb-1">소속 부서</label>
                      <input
                        type="text"
                        value={editDept}
                        onChange={(e) => setEditDept(e.target.value)}
                        className="w-full h-7 px-2 rounded border border-[var(--border)] bg-[var(--surface)]"
                      />
                    </div>
                    <div>
                      <label className="font-semibold block mb-1">직책</label>
                      <input
                        type="text"
                        value={editRole}
                        onChange={(e) => setEditRole(e.target.value)}
                        className="w-full h-7 px-2 rounded border border-[var(--border)] bg-[var(--surface)]"
                      />
                    </div>
                    <div>
                      <label className="font-semibold block mb-1">배치 사업장</label>
                      <select
                        value={editSite}
                        onChange={(e) => setEditSite(e.target.value)}
                        className="w-full h-7 px-1.5 rounded border border-[var(--border)] bg-[var(--surface)]"
                      >
                        {orgSites.map((s) => (
                          <option key={s.id} value={s.name}>
                            {s.name}
                          </option>
                        ))}
                      </select>
                    </div>
                    <div>
                      <label className="font-semibold block mb-1">재직 상태</label>
                      <select
                        value={editStatus}
                        onChange={(e) => setEditStatus(e.target.value as any)}
                        className="w-full h-7 px-1.5 rounded border border-[var(--border)] bg-[var(--surface)]"
                      >
                        <option value="재직">재직</option>
                        <option value="휴직">휴직</option>
                        <option value="퇴직예정">퇴직예정</option>
                        <option value="퇴직">퇴직</option>
                      </select>
                    </div>
                    <div className="col-span-2">
                      <MoneyInput
                        value={editSalary}
                        onChange={(val) => setEditSalary(val)}
                        label="월 약정 기본급"
                        placeholder="3,500,000"
                      />
                    </div>
                  </div>
                  <div className="flex justify-end gap-2 pt-2 border-t border-[var(--brand)]/20">
                    <Button size="xs" variant="ghost" onClick={() => setIsEditing(false)}>
                      취소
                    </Button>
                    <Button size="xs" variant="brand" onClick={saveEditing}>
                      변경 확정 및 감사로그 발행
                    </Button>
                  </div>
                </div>
              ) : null}

              {/* Workday 스타일 KPI 요약 카드 */}
              <div className="grid grid-cols-3 gap-2 text-center">
                <div className="rounded-lg border border-[var(--border)] bg-[var(--canvas)] p-2.5">
                  <div className="text-[10px] text-[var(--steel)]">월 기본급</div>
                  <div className="text-sm font-bold font-mono text-[var(--ink)] mt-0.5">
                    {Math.round(selectedEmp.baseSalary / 10000).toLocaleString()}만원
                  </div>
                </div>
                <div className="rounded-lg border border-[var(--border)] bg-[var(--canvas)] p-2.5">
                  <div className="text-[10px] text-[var(--steel)]">잔여 연차</div>
                  <div className="text-sm font-bold font-mono text-emerald-700 mt-0.5">
                    {selectedEmp.leaveBalance?.remaining ?? 15}일
                  </div>
                  <div className="text-[9px] text-[var(--faint)]">
                    부여 {selectedEmp.leaveBalance?.granted ?? 15}일
                  </div>
                </div>
                <div className="rounded-lg border border-[var(--border)] bg-[var(--canvas)] p-2.5">
                  <div className="text-[10px] text-[var(--steel)]">부양 가족</div>
                  <div className="text-sm font-bold font-mono text-sky-700 mt-0.5">
                    {selectedEmp.dependents}인
                  </div>
                  <div className="text-[9px] text-[var(--faint)]">간이세액 공제</div>
                </div>
              </div>

              {/* 1. 인적 정체성 (Person Identity) */}
              <div className="rounded-lg border border-[var(--border)] p-3.5 space-y-2.5 bg-[var(--surface)] shadow-2xs">
                <div className="flex items-center justify-between border-b border-[var(--border-soft)] pb-1.5">
                  <div className="font-bold text-[var(--ink)] text-xs flex items-center gap-1.5">
                    <Users className="w-3.5 h-3.5 text-indigo-600" />
                    <span>인적 정체성 (Person Identity)</span>
                  </div>
                  <span className="text-[10px] font-mono text-[var(--steel)]">고유식별 {selectedEmp.id}</span>
                </div>
                <div className="grid grid-cols-2 gap-y-2 gap-x-3 text-[11px]">
                  <div>
                    <span className="text-[var(--steel)] block">실명 (한글)</span>
                    <span className="font-bold text-[var(--ink)]">{selectedEmp.name}</span>
                  </div>
                  <div>
                    <span className="text-[var(--steel)] block">이메일</span>
                    <span className="font-mono text-[var(--ink)] truncate block">{selectedEmp.email}</span>
                  </div>
                  <div>
                    <span className="text-[var(--steel)] block">연락처</span>
                    <span className="font-mono text-[var(--ink)]">{selectedEmp.phone}</span>
                  </div>
                  <div>
                    <span className="text-[var(--steel)] block">부양가족 공제</span>
                    <span className="font-semibold text-blue-700">{selectedEmp.dependents}인 (소득세법 반영)</span>
                  </div>
                </div>
              </div>

              {/* 2. 고용 계약 (Employment Contract) */}
              <div className="rounded-lg border border-[var(--border)] p-3.5 space-y-2.5 bg-[var(--surface)] shadow-2xs">
                <div className="flex items-center justify-between border-b border-[var(--border-soft)] pb-1.5">
                  <div className="font-bold text-[var(--ink)] text-xs flex items-center gap-1.5">
                    <FileText className="w-3.5 h-3.5 text-emerald-600" />
                    <span>고용 계약 (Employment Contract)</span>
                  </div>
                  <span className="text-[10px] font-mono text-emerald-800 bg-emerald-50 px-1.5 py-0.5 rounded border border-emerald-200">
                    EC-2026-001
                  </span>
                </div>
                <div className="grid grid-cols-2 gap-y-2 gap-x-3 text-[11px]">
                  <div>
                    <span className="text-[var(--steel)] block">고용 형태</span>
                    <span className="font-semibold text-[var(--ink)]">{selectedEmp.empType}직</span>
                  </div>
                  <div>
                    <span className="text-[var(--steel)] block">입사일자</span>
                    <span className="font-mono text-[var(--ink)]">{selectedEmp.joinedDate}</span>
                  </div>
                  <div>
                    <span className="text-[var(--steel)] block">약정 월 기본급</span>
                    <span className="font-mono font-bold text-[var(--ink)]">
                      {selectedEmp.baseSalary.toLocaleString()}원
                    </span>
                  </div>
                  <div>
                    <span className="text-[var(--steel)] block">직책 및 정액 수당</span>
                    <span className="font-mono font-bold text-blue-700">
                      {(selectedEmp.fixedAllow || 0).toLocaleString()}원
                    </span>
                  </div>
                  <div className="col-span-2 text-[10px] text-[var(--steel)] bg-gray-50 p-2 rounded">
                    근로기준법 제17조 필수 기재사항 교부 완료 · 주 40시간(월 209시간) 통상시급 산출 기준 적용
                  </div>
                </div>
              </div>

              {/* 3. 조직 및 현장 배치 (Org Assignment) */}
              <div className="rounded-lg border border-[var(--border)] p-3.5 space-y-2.5 bg-[var(--surface)] shadow-2xs">
                <div className="flex items-center justify-between border-b border-[var(--border-soft)] pb-1.5">
                  <div className="font-bold text-[var(--ink)] text-xs flex items-center gap-1.5">
                    <Briefcase className="w-3.5 h-3.5 text-[var(--brand)]" />
                    <span>조직 및 현장 발령 (Org Assignment)</span>
                  </div>
                  <Button
                    size="xs"
                    variant="secondary"
                    leftIcon={<ArrowRightLeft className="w-3 h-3 text-blue-600" />}
                    onClick={() => {
                      setTransferDept(selectedEmp.dept === "운영1팀" ? "물류지원팀" : "운영1팀");
                      setTransferSite(selectedEmp.site);
                      setTransferModalOpen(true);
                    }}
                  >
                    부서 전보 / 발령 기안
                  </Button>
                </div>
                <div className="grid grid-cols-2 gap-y-2 gap-x-3 text-[11px]">
                  <div>
                    <span className="text-[var(--steel)] block">소속 법인</span>
                    <span className="font-medium text-[var(--ink)]">{selectedEmp.entity}</span>
                  </div>
                  <div>
                    <span className="text-[var(--steel)] block">배치 사업장</span>
                    <span className="font-medium text-[var(--ink)]">{selectedEmp.site}</span>
                  </div>
                  <div>
                    <span className="text-[var(--steel)] block">소속 부서</span>
                    <span className="font-bold text-blue-800">{selectedEmp.dept}</span>
                  </div>
                  <div>
                    <span className="text-[var(--steel)] block">부여 직책</span>
                    <span className="font-bold text-[var(--ink)]">{selectedEmp.role}</span>
                  </div>
                </div>
              </div>

              {/* 4. Discord-Style 다중 역할 레이어 및 실효 인가 권한 (Layered Roles & Fold) */}
              <div className="rounded-lg border border-[var(--border)] p-3.5 space-y-3 bg-[var(--surface)] shadow-2xs">
                <div className="flex items-center justify-between border-b border-[var(--border-soft)] pb-2">
                  <div className="font-bold text-[var(--ink)] text-xs flex items-center gap-1.5">
                    <Layers className="w-3.5 h-3.5 text-indigo-600" />
                    <span>보유 다중 역할 레이어 ({selectedEmpFold?.layeredRoles.length || 0}개 활성)</span>
                  </div>
                  <Button
                    size="xs"
                    variant="outline"
                    leftIcon={<Plus className="w-3 h-3 text-indigo-600" />}
                    onClick={() => setRoleAssignOpen(!roleAssignOpen)}
                  >
                    {roleAssignOpen ? "닫기" : "역할 배정"}
                  </Button>
                </div>

                {/* Quick Add Role Popover/Box */}
                {roleAssignOpen && (
                  <div className="p-2.5 rounded-lg bg-indigo-50/60 dark:bg-indigo-950/40 border border-indigo-200 dark:border-indigo-800 space-y-2">
                    <span className="text-xs font-bold text-indigo-950 dark:text-indigo-200">
                      신규 역할 레이어 배정
                    </span>
                    <div className="flex items-center gap-1.5">
                      <select
                        value={selectedRoleToAdd}
                        onChange={(e) => setSelectedRoleToAdd(e.target.value)}
                        className="flex-1 px-2 py-1 text-xs rounded bg-white dark:bg-slate-900 border border-[var(--border)] text-[var(--ink)]"
                      >
                        <option value="">배정할 역할을 선택하십시오...</option>
                        {roleDefinitions
                          .filter(
                            (r) =>
                              !selectedEmpFold?.layeredRoles.some(
                                (lr) => lr.role.id === r.id
                              )
                          )
                          .map((r) => (
                            <option key={r.id} value={r.id}>
                              {r.name} (우선순위 #{r.priority} · {r.category})
                            </option>
                          ))}
                      </select>
                      <Button
                        size="xs"
                        variant="brand"
                        disabled={!selectedRoleToAdd}
                        onClick={() => {
                          if (!selectedRoleToAdd) return;
                          assignRoleToEmployee(
                            selectedEmp.id,
                            selectedRoleToAdd,
                            { type: "all" },
                            "인사 360° 원장 직접 부여"
                          );
                          setSelectedRoleToAdd("");
                          setRoleAssignOpen(false);
                        }}
                      >
                        부여
                      </Button>
                    </div>
                  </div>
                )}

                {/* Stacked Role Badges */}
                <div className="flex flex-wrap items-center gap-1.5">
                  {selectedEmpFold?.layeredRoles.map(({ role, assignment }) => (
                    <DiscordRoleBadge
                      key={assignment.id}
                      role={role}
                      size="sm"
                      showPriority
                      scopeLabel={assignment.scope.targetName}
                      onRemove={() => removeRoleFromEmployee(assignment.id)}
                    />
                  ))}
                </div>

                {/* Live Folded Capabilities & Acting Capacities Accordion */}
                <div className="p-2.5 rounded-lg bg-[var(--muted)]/40 border border-[var(--border)] space-y-2 text-xs">
                  <div className="flex items-center justify-between text-[11px]">
                    <span className="font-semibold text-[var(--ink)]">
                      실효 인가 권한 합산 (Effective Fold):
                    </span>
                    <span className="font-mono font-bold text-indigo-600 dark:text-indigo-400">
                      총 {selectedEmpFold?.effectiveCapabilities.size || 0}개 액션 허용
                    </span>
                  </div>

                  {/* Acting capacities if any */}
                  {selectedEmpFold?.actingCapacities && selectedEmpFold.actingCapacities.length > 0 && (
                    <div className="space-y-1 pt-1 border-t border-[var(--border)]/60">
                      <span className="text-[10px] text-[var(--steel)] block">
                        행사 가능 결재/서명 자격 (Acting Capacities):
                      </span>
                      <div className="space-y-1">
                        {selectedEmpFold.actingCapacities.map((cap) => (
                          <div
                            key={cap.roleId}
                            className="flex items-center justify-between text-[11px] p-1.5 rounded bg-white dark:bg-slate-900 border border-[var(--border)]"
                          >
                            <span className="font-medium text-[var(--ink)]">
                              {cap.roleName}
                            </span>
                            <span className="font-mono text-[10px] text-indigo-600">
                              {cap.maxAmount ? `한도 ₩${cap.maxAmount.toLocaleString()}` : "전결 상한 없음"}
                            </span>
                          </div>
                        ))}
                      </div>
                    </div>
                  )}

                  <p className="text-[10px] text-[var(--steel)] leading-relaxed pt-1">
                    <span className="font-semibold">인가 및 전결 원칙:</span>{" "}
                    실효 권한은 정적 캐시가 아닌 다중 역할의 합집합과 제약 조건을 실시간 폴드하여
                    도출되며, 서명 시 행사 자격이 기표됩니다.
                  </p>
                </div>
              </div>

              {/* 급여 이체 계좌 정보 */}
              <div className="rounded-lg border border-[var(--border)] p-3.5 space-y-2.5">
                <div className="flex items-center justify-between">
                  <div className="font-bold text-[var(--ink)] text-xs flex items-center gap-1.5">
                    <CreditCard className="w-3.5 h-3.5 text-emerald-600" />
                    <span>급여 지급 전용 계좌</span>
                  </div>
                  <span className="text-[10px] text-emerald-700 bg-emerald-50 border border-emerald-200 px-1.5 py-0.5 rounded font-mono font-medium">
                    실명인증 완료
                  </span>
                </div>
                <div className="flex items-center justify-between p-2 rounded bg-[var(--muted)] text-[11px]">
                  <span className="text-[var(--steel)]">{selectedEmp.bankName || "신한은행"}</span>
                  <span className="font-mono font-bold text-[var(--ink)]">
                    {selectedEmp.bankAccount || "110-384-918231"}
                  </span>
                </div>
              </div>

              {/* 연계 근태 상태 & 주52시간 게이트 */}
              <div className="rounded-lg border border-[var(--border)] p-3.5 space-y-2.5">
                <div className="flex items-center justify-between">
                  <div className="font-bold text-[var(--ink)] text-xs flex items-center gap-1.5">
                    <Clock className="w-3.5 h-3.5 text-sky-600" />
                    <span>근태 현황 & 주52시간 한도</span>
                  </div>
                  {empAttendance ? (
                    empAttendance.exceptionCode ? (
                      <ObjectLink code={empAttendance.exceptionCode} />
                    ) : (
                      <span className="font-mono text-[10px] text-[var(--steel)]">{empAttendance.date}</span>
                    )
                  ) : (
                    <span className="text-[10px] text-[var(--steel)]">당월 기록 준비중</span>
                  )}
                </div>
                {empAttendance ? (
                  <div className="space-y-1.5 text-[11px]">
                    <div className="flex justify-between">
                      <span className="text-[var(--steel)]">금주 총 근무시간:</span>
                      <span className="font-mono font-bold text-[var(--ink)]">
                        {empAttendance.weeklyHoursTotal}시간 / 52시간
                      </span>
                    </div>
                    <div className="w-full bg-[var(--border)] h-1.5 rounded-full overflow-hidden">
                      <div
                        className={`h-full ${
                          empAttendance.weeklyHoursTotal >= 50
                            ? "bg-red-500"
                            : empAttendance.weeklyHoursTotal >= 45
                            ? "bg-amber-500"
                            : "bg-emerald-500"
                        }`}
                        style={{
                          width: `${Math.min(100, (empAttendance.weeklyHoursTotal / 52) * 100)}%`,
                        }}
                      />
                    </div>
                    <div className="flex justify-between text-[10px] text-[var(--steel)]">
                      <span>연장 {empAttendance.otHours}h · 야간 {empAttendance.nightHours}h</span>
                      <StatusChip label={empAttendance.status} size="xs" />
                    </div>
                  </div>
                ) : (
                  <p className="text-[11px] text-[var(--steel)]">당월 근태 데이터가 정상 등록되어 있습니다.</p>
                )}
              </div>

              {/* 배차된 현장 작업오더 목록 */}
              <div className="rounded-lg border border-[var(--border)] p-3.5 space-y-2.5">
                <div className="flex items-center justify-between">
                  <div className="font-bold text-[var(--ink)] text-xs flex items-center gap-1.5">
                    <Wrench className="w-3.5 h-3.5 text-amber-600" />
                    <span>배차된 현장 작업오더 ({empWorkOrders.length}건)</span>
                  </div>
                </div>
                {empWorkOrders.length === 0 ? (
                  <div className="text-[11px] text-[var(--steel)] py-2 text-center">
                    현재 배차된 대기 작업오더가 없습니다.
                  </div>
                ) : (
                  <div className="space-y-1.5">
                    {empWorkOrders.map((wo) => (
                      <div
                        key={wo.id}
                        className="flex items-center justify-between p-2 rounded bg-[var(--muted)] text-[11px]"
                      >
                        <div className="flex items-center gap-2">
                          <ObjectLink code={wo.code} />
                          <span className="font-medium truncate max-w-[180px]">{wo.title}</span>
                        </div>
                        <StatusChip label={wo.status} size="xs" />
                      </div>
                    ))}
                  </div>
                )}
              </div>

              {/* 스레드 열기 링크 */}
              <div className="rounded-lg border border-[var(--border)] bg-indigo-50/40 p-3 flex items-center justify-between">
                <div className="flex items-center gap-2">
                  <MessageSquare className="w-4 h-4 text-indigo-700" />
                  <div>
                    <div className="font-semibold text-indigo-950">임직원 인사 1:1 스레드</div>
                    <div className="text-[10px] text-indigo-700">인사고과 및 면담 기록 채널</div>
                  </div>
                </div>
                <Button
                  size="xs"
                  variant="secondary"
                  onClick={() => openObjectInspector(selectedEmp.code)}
                >
                  스레드 열기
                </Button>
              </div>
            </div>

            {/* 드로어 푸터 */}
            <div className="p-3.5 border-t border-[var(--border)] bg-[var(--canvas)] flex justify-end gap-2">
              <Button size="sm" variant="ghost" onClick={() => setSelectedEmpId(null)}>
                닫기
              </Button>
            </div>
          </div>
        </div>
      )}

      {/* 5. 신규 사원 온보딩 모달 */}
      {onboardingModalOpen && (
        <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/40 backdrop-blur-xs p-4">
          <div className="w-full max-w-lg rounded-lg border border-[var(--border)] bg-[var(--surface)] shadow-2xl p-5 space-y-4 animate-in fade-in zoom-in-95 duration-150">
            <div className="flex items-center justify-between border-b border-[var(--border)] pb-3">
              <div className="flex items-center gap-2">
                <Users className="w-5 h-5 text-indigo-600" />
                <h3 className="text-sm font-bold text-[var(--ink)]">신규 임직원 등록 (온보딩)</h3>
              </div>
              <button
                onClick={() => setOnboardingModalOpen(false)}
                className="text-gray-400 hover:text-gray-600 cursor-pointer"
              >
                ×
              </button>
            </div>

            <form onSubmit={handleCreateSubmit} className="space-y-3 text-xs">
              <div className="grid grid-cols-2 gap-3">
                <div>
                  <label className="font-semibold block mb-1">사원 성명 *</label>
                  <input
                    type="text"
                    required
                    value={name}
                    onChange={(e) => setName(e.target.value)}
                    placeholder="홍길동"
                    className="w-full h-8 px-2.5 rounded border border-[var(--border)] bg-[var(--surface)]"
                  />
                </div>
                <div>
                  <label className="font-semibold block mb-1">연락처</label>
                  <input
                    type="text"
                    value={phone}
                    onChange={(e) => setPhone(e.target.value)}
                    placeholder="010-0000-0000"
                    className="w-full h-8 px-2.5 rounded border border-[var(--border)] bg-[var(--surface)] font-mono"
                  />
                </div>
              </div>

              <div className="grid grid-cols-2 gap-3">
                <div>
                  <label className="font-semibold block mb-1">소속 법인</label>
                  <select
                    value={entity}
                    onChange={(e) => setEntity(e.target.value)}
                    className="w-full h-8 px-2.5 rounded border border-[var(--border)] bg-[var(--surface)]"
                  >
                    {orgEntities.map((ent) => (
                      <option key={ent.id} value={ent.name}>
                        {ent.name}
                      </option>
                    ))}
                  </select>
                </div>
                <div>
                  <label className="font-semibold block mb-1">배치 현장 / 사업장</label>
                  <select
                    value={site}
                    onChange={(e) => setSite(e.target.value)}
                    className="w-full h-8 px-2.5 rounded border border-[var(--border)] bg-[var(--surface)]"
                  >
                    {orgSites.map((st) => (
                      <option key={st.id} value={st.name}>
                        {st.name}
                      </option>
                    ))}
                  </select>
                </div>
              </div>

              <div className="grid grid-cols-3 gap-3">
                <div>
                  <label className="font-semibold block mb-1">소속 부서</label>
                  <input
                    type="text"
                    value={dept}
                    onChange={(e) => setDept(e.target.value)}
                    placeholder="운영1팀"
                    className="w-full h-8 px-2.5 rounded border border-[var(--border)] bg-[var(--surface)]"
                  />
                </div>
                <div>
                  <label className="font-semibold block mb-1">직책</label>
                  <input
                    type="text"
                    value={role}
                    onChange={(e) => setRole(e.target.value)}
                    placeholder="사원 / 주임 / 팀장"
                    className="w-full h-8 px-2.5 rounded border border-[var(--border)] bg-[var(--surface)]"
                  />
                </div>
                <div>
                  <label className="font-semibold block mb-1">고용 형태</label>
                  <select
                    value={empType}
                    onChange={(e) => setEmpType(e.target.value as any)}
                    className="w-full h-8 px-2 rounded border border-[var(--border)] bg-[var(--surface)]"
                  >
                    <option value="정규">정규</option>
                    <option value="계약">계약</option>
                    <option value="시급">시급</option>
                    <option value="일당">일당</option>
                    <option value="파견">파견</option>
                    <option value="임원">임원</option>
                  </select>
                </div>
              </div>

              <div className="grid grid-cols-3 gap-3">
                <div className="col-span-1">
                  <MoneyInput
                    value={baseSalary}
                    onChange={(val) => setBaseSalary(val)}
                    label="월 기본급 *"
                    placeholder="3,500,000"
                  />
                </div>
                <div className="col-span-1">
                  <MoneyInput
                    value={fixedAllow}
                    onChange={(val) => setFixedAllow(val)}
                    label="정액 수당"
                    placeholder="300,000"
                  />
                </div>
                <div className="col-span-1">
                  <label className="font-semibold block mb-1">부양가족 수</label>
                  <input
                    type="number"
                    value={dependents}
                    onChange={(e) => setDependents(Number(e.target.value))}
                    className="w-full h-8 px-2.5 rounded border border-[var(--border)] bg-[var(--surface)] font-mono"
                  />
                </div>
              </div>

              <div className="grid grid-cols-2 gap-3">
                <div>
                  <label className="font-semibold block mb-1">급여 은행</label>
                  <input
                    type="text"
                    value={bankName}
                    onChange={(e) => setBankName(e.target.value)}
                    placeholder="신한은행"
                    className="w-full h-8 px-2.5 rounded border border-[var(--border)] bg-[var(--surface)]"
                  />
                </div>
                <div>
                  <label className="font-semibold block mb-1">급여 이체 계좌</label>
                  <input
                    type="text"
                    value={bankAccount}
                    onChange={(e) => setBankAccount(e.target.value)}
                    placeholder="110-384-918231"
                    className="w-full h-8 px-2.5 rounded border border-[var(--border)] bg-[var(--surface)] font-mono"
                  />
                </div>
              </div>

              <div className="p-2.5 rounded border border-indigo-200 bg-indigo-50/60 text-indigo-900 text-[11px]">
                * 등록 완료 시 즉시 <strong>근태 매트릭스</strong> 및 <strong>당월 급여 산정 엔진</strong>에 실시간 편입되며 감사 로그가 발행됩니다.
              </div>

              <div className="flex items-center justify-end gap-2 pt-3 border-t border-[var(--border)]">
                <Button variant="ghost" size="sm" onClick={() => setOnboardingModalOpen(false)}>
                  취소
                </Button>
                <Button variant="brand" size="sm" type="submit">
                  사원 등록 확정
                </Button>
              </div>
            </form>
          </div>
        </div>
      )}

      {/* 6. 부서 전보 / 발령 기안 모달 */}
      {transferModalOpen && selectedEmp && (
        <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/40 backdrop-blur-xs p-4">
          <div className="w-full max-w-md rounded-lg border border-[var(--border)] bg-[var(--surface)] shadow-2xl p-5 space-y-4 animate-in fade-in zoom-in-95 duration-150">
            <div className="flex items-center justify-between border-b border-[var(--border)] pb-3">
              <div className="flex items-center gap-2">
                <ArrowRightLeft className="w-5 h-5 text-blue-600" />
                <h3 className="text-sm font-bold text-[var(--ink)]">인사 발령: 부서 전보 기안</h3>
              </div>
              <button
                type="button"
                onClick={() => setTransferModalOpen(false)}
                className="text-gray-400 hover:text-gray-600 cursor-pointer"
              >
                ✕
              </button>
            </div>

            <div className="space-y-3 text-xs">
              <div className="p-2.5 rounded bg-gray-50 border border-[var(--border)] space-y-1">
                <div className="flex justify-between">
                  <span className="text-[var(--steel)]">발령 대상:</span>
                  <span className="font-bold">{selectedEmp.name} ({selectedEmp.code})</span>
                </div>
                <div className="flex justify-between">
                  <span className="text-[var(--steel)]">현재 소속:</span>
                  <span>{selectedEmp.dept} ({selectedEmp.site})</span>
                </div>
              </div>

              <div>
                <label className="font-semibold block mb-1">발령 대상 부서</label>
                <select
                  value={transferDept}
                  onChange={(e) => setTransferDept(e.target.value)}
                  className="w-full h-8 px-2.5 rounded border border-[var(--border)] bg-[var(--surface)]"
                >
                  <option value="운영1팀">운영1팀</option>
                  <option value="물류지원팀">물류지원팀</option>
                  <option value="현장설비관리팀">현장설비관리팀</option>
                  <option value="안전환경관리팀">안전환경관리팀</option>
                  <option value="재무회계팀">재무회계팀</option>
                </select>
              </div>

              <div>
                <label className="font-semibold block mb-1">배치 사업장 / 거점</label>
                <select
                  value={transferSite}
                  onChange={(e) => setTransferSite(e.target.value)}
                  className="w-full h-8 px-2.5 rounded border border-[var(--border)] bg-[var(--surface)]"
                >
                  {orgSites.map((s) => (
                    <option key={s.id} value={s.name}>
                      {s.name}
                    </option>
                  ))}
                </select>
              </div>

              <div className="grid grid-cols-2 gap-2">
                <div>
                  <label className="font-semibold block mb-1">발령 효력일</label>
                  <input
                    type="date"
                    value={transferEffectiveDate}
                    onChange={(e) => setTransferEffectiveDate(e.target.value)}
                    className="w-full h-8 px-2 rounded border border-[var(--border)] bg-[var(--surface)] font-mono"
                  />
                </div>
                <div>
                  <label className="font-semibold block mb-1">발령 사유</label>
                  <input
                    type="text"
                    value={transferReason}
                    onChange={(e) => setTransferReason(e.target.value)}
                    className="w-full h-8 px-2 rounded border border-[var(--border)] bg-[var(--surface)]"
                  />
                </div>
              </div>
            </div>

            <div className="flex justify-end gap-2 pt-3 border-t border-[var(--border)]">
              <Button size="sm" variant="secondary" onClick={() => setTransferModalOpen(false)}>
                취소
              </Button>
              <Button
                size="sm"
                variant="brand"
                leftIcon={<ShieldCheck className="w-3.5 h-3.5" />}
                onClick={() => {
                  setTransferModalOpen(false);
                  setIsConsequenceOpen(true);
                }}
              >
                파급 영향도(Consequence) 사전 검토 →
              </Button>
            </div>
          </div>
        </div>
      )}

      {/* 7. 파급 효과 검토 모달 (Consequence Modal) */}
      <ConsequenceModal
        isOpen={isConsequenceOpen}
        onClose={() => setIsConsequenceOpen(false)}
        onConfirm={handleConfirmTransfer}
        summary={transferConsequenceSummary}
      />
    </div>
  );
}
