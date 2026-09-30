"use client";

import React, { useState, useMemo } from "react";
import { useAppStore } from "@/lib/store";
import { RoleDefinition, RoleAssignment } from "@/lib/types";
import { DiscordRoleBadge } from "./DiscordRoleBadge";
import { Button } from "@/components/ui/Button";
import {
  ShieldAlert,
  KeyRound,
  ShieldCheck,
  DollarSign,
  FileCheck2,
  Users,
  FileSpreadsheet,
  Wrench,
  Building,
  User,
  Shield,
  Plus,
  ArrowUp,
  ArrowDown,
  Search,
  Check,
  Info,
  Lock,
  Fingerprint,
  UserPlus,
  X,
  AlertTriangle,
  BadgeAlert,
} from "lucide-react";

const DISCORD_PALETTE = [
  "#5865F2", // Discord Blurple
  "#57F287", // Green
  "#FEE75C", // Yellow
  "#EB459E", // Fuchsia
  "#ED4245", // Red
  "#3BA55D", // Dark Green
  "#FAA61A", // Orange
  "#06B6D4", // Cyan
  "#8B5CF6", // Purple
  "#747F8D", // Gray
  "#EF4444", // Crimson
  "#10B981", // Emerald
];

const AVAILABLE_CAPABILITIES = [
  { id: "Approvals::SignTier1", label: "DoA 1단계: 일상경비 전결 (500만원 이하)", cat: "결재·전결", law: "사내 전결규정 제4조" },
  { id: "Approvals::SignTier2", label: "DoA 2단계: 운영예산 전결 (5,000만원 이하)", cat: "결재·전결", law: "사내 전결규정 제6조" },
  { id: "Approvals::SignTier3", label: "DoA 3단계: 고액 계약 및 이사회 안건 승인", cat: "결재·전결", law: "상법 제393조" },
  { id: "Payroll::Calculate", label: "월별 전사 정기 급여 산정 및 근태 연계 집계", cat: "급여·보상", law: "근로기준법 제43조" },
  { id: "Payroll::EditDraftSheet", label: "급여 조정 시트(Connected Sheet) 수정 및 수당 입력", cat: "급여·보상", law: "임금대장 작성규정" },
  { id: "Payroll::AdjudicateDiscrepancy", label: "근태/급여 이상치(Discrepancy) 소명 심사 및 승인", cat: "급여·보상", law: "근로기준법 제54조" },
  { id: "Payroll::ApproveRun", label: "급여 대장 최종 마감 및 승인 (SoD 독립 승인)", cat: "급여·보상", law: "내부회계관리제도(K-SOX)" },
  { id: "Payroll::SealRegister", label: "공증 임금대장 전자봉인 및 법적 보존", cat: "급여·보상", law: "근로기준법 제42조" },
  { id: "Payroll::ExecuteFirmBanking", label: "KFTC 펌뱅킹 CMS 100바이트 이체 전문 생성 및 집행", cat: "자금·외환", law: "전자금융거래법 제21조" },
  { id: "Treasury::GenerateKFTCFile", label: "금융결제원 송신 파일 무결성 해시 검증 및 암호화", cat: "자금·외환", law: "금융보안원 기술규격" },
  { id: "HR::OnboardEmployee", label: "신규 사원 입사 처리 및 인사기록 카드 등록", cat: "인사·노무", law: "근로기준법 제17조" },
  { id: "HR::TransferEmployee", label: "조직 개편 및 부서 전보(Transfer) 발령 기안", cat: "인사·노무", law: "취업규칙 제14조" },
  { id: "HR::UpdateProfile", label: "구성원 인적 사항 및 자격 이력 갱신", cat: "인사·노무", law: "개인정보보호법 제15조" },
  { id: "Operations::AssignWorkOrder", label: "현장 정비·운영 작업오더(WO) 배차 및 승인", cat: "운영·정비", law: "산업안전보건법 제36조" },
  { id: "Operations::CompleteInspection", label: "작업 완료 안전 점검 및 현장 검수", cat: "운영·정비", law: "산업안전보건법 제38조" },
  { id: "Governance::InspectAuditChain", label: "블록체인 감사 추적 체인(Audit Ledger) 열람", cat: "감사·거버넌스", law: "주식회사외부감사법 제8조" },
  { id: "Governance::SimulateAccess", label: "Cedar PBAC 정책 영향도 사전 시뮬레이션", cat: "감사·거버넌스", law: "내부통제기준" },
  { id: "Comms::SendChat", label: "사내 메신저 및 실시간 팀 채널 메시지 발송", cat: "소통·협업", law: "정보통신망법" },
  { id: "Comms::ReadMail", label: "공식 사내 통지 및 전자 공문 수신 열람", cat: "소통·협업", law: "전자문서법" },
  { id: "Profile::ViewSelf", label: "본인 근태, 급여명세서 및 인사정보 조회", cat: "소통·협업", law: "근로기준법 제48조" },
];

export function LayeredRoleCanvas() {
  const roleDefinitions = useAppStore((s) => s.roleDefinitions);
  const roleAssignments = useAppStore((s) => s.roleAssignments);
  const employees = useAppStore((s) => s.employees);
  const reorderRoles = useAppStore((s) => s.reorderRoles);
  const createRoleDefinition = useAppStore((s) => s.createRoleDefinition);
  const updateRoleDefinition = useAppStore((s) => s.updateRoleDefinition);
  const assignRoleToEmployee = useAppStore((s) => s.assignRoleToEmployee);
  const removeRoleFromEmployee = useAppStore((s) => s.removeRoleFromEmployee);

  const [selectedRoleId, setSelectedRoleId] = useState<string>(
    roleDefinitions[0]?.id || "role-super-admin"
  );
  const [roleSearchQuery, setRoleSearchQuery] = useState("");
  const [memberAddSearch, setMemberAddSearch] = useState("");
  const [isAddingMember, setIsAddingMember] = useState(false);

  // Sorted roles by priority descending (Discord standard)
  const sortedRoles = useMemo(() => {
    return [...roleDefinitions].sort((a, b) => b.priority - a.priority);
  }, [roleDefinitions]);

  const filteredRoles = useMemo(() => {
    if (!roleSearchQuery.trim()) return sortedRoles;
    const q = roleSearchQuery.toLowerCase();
    return sortedRoles.filter(
      (r) =>
        r.name.toLowerCase().includes(q) ||
        r.category.toLowerCase().includes(q) ||
        r.description.toLowerCase().includes(q)
    );
  }, [sortedRoles, roleSearchQuery]);

  const activeRole = useMemo(() => {
    return roleDefinitions.find((r) => r.id === selectedRoleId) || sortedRoles[0];
  }, [roleDefinitions, selectedRoleId, sortedRoles]);

  // Assignments for active role
  const activeRoleAssignments = useMemo(() => {
    if (!activeRole) return [];
    return roleAssignments.filter(
      (a) => a.roleId === activeRole.id && a.status === "active"
    );
  }, [roleAssignments, activeRole]);

  // Handle reordering up/down
  const handleMoveRole = (roleId: string, direction: "up" | "down") => {
    const currentIndex = sortedRoles.findIndex((r) => r.id === roleId);
    if (currentIndex === -1) return;
    const targetIndex = direction === "up" ? currentIndex - 1 : currentIndex + 1;
    if (targetIndex < 0 || targetIndex >= sortedRoles.length) return;

    const newOrder = [...sortedRoles];
    const [moved] = newOrder.splice(currentIndex, 1);
    newOrder.splice(targetIndex, 0, moved);
    reorderRoles(newOrder.map((r) => r.id));
  };

  // Handle new role creation
  const handleCreateRole = () => {
    const newRole = createRoleDefinition({
      name: "새로운 직무 역할",
      color: "#5865F2",
      badgeTone: "brand",
      iconName: "Shield",
      priority: Math.max(15, (sortedRoles[sortedRoles.length - 1]?.priority || 20) + 5),
      category: "functional",
      scopeType: "entity",
      grantedCapabilities: ["Profile::ViewSelf", "Comms::SendChat"],
      description: "사내 직무 분장에 따라 새로 정의된 역할 레이어입니다.",
      isSystemDefault: false,
    });
    setSelectedRoleId(newRole.id);
  };

  // Toggle capability
  const handleToggleCapability = (capId: string) => {
    if (!activeRole) return;
    const exists = activeRole.grantedCapabilities.includes(capId);
    const updated = exists
      ? activeRole.grantedCapabilities.filter((c) => c !== capId)
      : [...activeRole.grantedCapabilities, capId];
    updateRoleDefinition(activeRole.id, { grantedCapabilities: updated });
  };

  return (
    <div className="bg-[var(--surface)] border border-[var(--border)] rounded-xl shadow-xs overflow-hidden">
      {/* Top Banner explaining Discord-Style Multi-Layered Role Engine */}
      <div className="px-5 py-4 bg-gradient-to-r from-indigo-900/10 via-purple-900/10 to-transparent border-b border-[var(--border)]">
        <div className="flex flex-col sm:flex-row sm:items-center justify-between gap-3">
          <div>
            <div className="flex items-center gap-2">
              <span className="px-2 py-0.5 rounded text-[11px] font-bold bg-indigo-600 text-white">
                Discord-Style Role Architecture
              </span>
              <h2 className="text-base font-bold text-[var(--ink)]">
                다중 역할 레이어 거버넌스 (Layered Role Engine)
              </h2>
            </div>
            <p className="text-xs text-[var(--steel)] mt-1">
              자연인 당사자(Party)는 단일 직위가 아닌 여러 개의 역할 레이어를 동시에 보유하며,
              인가 엔진은 유효 권한을 실시간 폴드(Fold)하여 도출합니다.
              <span className="font-semibold text-indigo-600 dark:text-indigo-400 ml-1">
                (원칙: 권한은 직급에 종속되지 않습니다 — Authority does not follow rank)
              </span>
            </p>
          </div>
          <Button
            size="sm"
            variant="brand"
            leftIcon={<Plus size={14} />}
            onClick={handleCreateRole}
          >
            새 역할 레이어 만들기
          </Button>
        </div>
      </div>

      {/* 3-Pane Discord Server Settings Layout */}
      <div className="grid grid-cols-1 lg:grid-cols-12 min-h-[640px]">
        {/* Pane 1: Roles List (Left, 3.5 cols) */}
        <div className="lg:col-span-4 border-r border-[var(--border)] flex flex-col bg-[var(--surface)]">
          {/* Search box */}
          <div className="p-3 border-b border-[var(--border)]">
            <div className="relative">
              <Search
                size={14}
                className="absolute left-2.5 top-1/2 -translate-y-1/2 text-[var(--steel)]"
              />
              <input
                type="text"
                placeholder="역할 검색 (이름, 분류, 권한)..."
                value={roleSearchQuery}
                onChange={(e) => setRoleSearchQuery(e.target.value)}
                className="w-full pl-8 pr-3 py-1.5 text-xs rounded-md bg-[var(--muted)] border border-[var(--border)] text-[var(--ink)] focus:outline-hidden focus:ring-1 focus:ring-indigo-500"
              />
            </div>
            <div className="flex items-center justify-between mt-2 text-[11px] text-[var(--steel)]">
              <span>총 {roleDefinitions.length}개 역할</span>
              <span>상위 우선순위 순 정렬</span>
            </div>
          </div>

          {/* List items */}
          <div className="flex-1 overflow-y-auto divide-y divide-[var(--border)]/50">
            {filteredRoles.map((role, idx) => {
              const isSelected = activeRole?.id === role.id;
              const memberCount = roleAssignments.filter(
                (a) => a.roleId === role.id && a.status === "active"
              ).length;

              return (
                <div
                  key={role.id}
                  onClick={() => setSelectedRoleId(role.id)}
                  className={`p-3 flex items-center justify-between cursor-pointer transition select-none ${
                    isSelected
                      ? "bg-indigo-50/80 dark:bg-indigo-950/40 border-l-4 border-indigo-600 font-medium"
                      : "hover:bg-[var(--muted)]"
                  }`}
                >
                  <div className="flex items-center gap-2.5 min-w-0">
                    {/* Discord colored circle */}
                    <span
                      className="w-3.5 h-3.5 rounded-full shrink-0 shadow-xs"
                      style={{ backgroundColor: role.color }}
                    />
                    <div className="min-w-0">
                      <div className="flex items-center gap-1.5">
                        <span className="text-xs text-[var(--ink)] truncate">
                          {role.name}
                        </span>
                        {role.conditions?.requireMfa && (
                          <span title="FIDO2 생체인증 필수">
                            <Fingerprint
                              size={12}
                              className="text-amber-500 shrink-0"
                            />
                          </span>
                        )}
                      </div>
                      <div className="flex items-center gap-2 text-[10px] text-[var(--steel)]">
                        <span className="font-mono">우선순위 #{role.priority}</span>
                        <span>•</span>
                        <span>{memberCount}명 보유</span>
                      </div>
                    </div>
                  </div>

                  {/* Reorder Up/Down arrows */}
                  <div
                    className="flex items-center gap-0.5 opacity-60 hover:opacity-100 shrink-0"
                    onClick={(e) => e.stopPropagation()}
                  >
                    <button
                      type="button"
                      disabled={idx === 0}
                      onClick={() => handleMoveRole(role.id, "up")}
                      className="p-1 rounded hover:bg-black/10 dark:hover:bg-white/10 disabled:opacity-20 text-[var(--steel)]"
                      title="우선순위 올리기"
                    >
                      <ArrowUp size={12} />
                    </button>
                    <button
                      type="button"
                      disabled={idx === filteredRoles.length - 1}
                      onClick={() => handleMoveRole(role.id, "down")}
                      className="p-1 rounded hover:bg-black/10 dark:hover:bg-white/10 disabled:opacity-20 text-[var(--steel)]"
                      title="우선순위 내리기"
                    >
                      <ArrowDown size={12} />
                    </button>
                  </div>
                </div>
              );
            })}
          </div>
        </div>

        {/* Pane 2: Role Editor (Center, 5 cols) */}
        {activeRole ? (
          <div className="lg:col-span-5 p-5 border-r border-[var(--border)] overflow-y-auto space-y-5">
            {/* Header / Identity */}
            <div className="space-y-3 pb-4 border-b border-[var(--border)]">
              <div className="flex items-center justify-between">
                <span className="text-xs font-bold text-[var(--steel)] tracking-wider uppercase">
                  역할 식별 및 배지 설정
                </span>
                <span className="text-xs font-mono px-2 py-0.5 rounded bg-black/5 dark:bg-white/10 text-[var(--steel)]">
                  ID: {activeRole.id}
                </span>
              </div>

              {/* Role Name Input */}
              <div>
                <label className="block text-xs font-semibold text-[var(--ink)] mb-1">
                  역할 명칭 (Role Display Name)
                </label>
                <input
                  type="text"
                  value={activeRole.name}
                  onChange={(e) =>
                    updateRoleDefinition(activeRole.id, { name: e.target.value })
                  }
                  className="w-full px-3 py-1.5 text-sm rounded-md bg-[var(--surface)] border border-[var(--border)] text-[var(--ink)] focus:outline-hidden focus:ring-1 focus:ring-indigo-500 font-semibold"
                />
              </div>

              {/* Discord Color Swatches */}
              <div>
                <label className="block text-xs font-semibold text-[var(--ink)] mb-1.5">
                  역할 대표 색상 (Role Color)
                </label>
                <div className="flex flex-wrap items-center gap-2">
                  {DISCORD_PALETTE.map((c) => (
                    <button
                      key={c}
                      type="button"
                      onClick={() =>
                        updateRoleDefinition(activeRole.id, { color: c })
                      }
                      style={{ backgroundColor: c }}
                      className={`w-7 h-7 rounded-full transition-transform flex items-center justify-center shadow-xs ${
                        activeRole.color.toLowerCase() === c.toLowerCase()
                          ? "ring-2 ring-offset-2 ring-indigo-500 scale-110"
                          : "hover:scale-105"
                      }`}
                    >
                      {activeRole.color.toLowerCase() === c.toLowerCase() && (
                        <Check size={14} className="text-white drop-shadow-xs" />
                      )}
                    </button>
                  ))}
                  <input
                    type="color"
                    value={activeRole.color}
                    onChange={(e) =>
                      updateRoleDefinition(activeRole.id, { color: e.target.value })
                    }
                    className="w-7 h-7 rounded-full border-0 cursor-pointer p-0 bg-transparent"
                    title="사용자 지정 색상"
                  />
                </div>
              </div>

              {/* Preview Badge */}
              <div className="pt-2 flex items-center gap-2">
                <span className="text-xs text-[var(--steel)]">배지 미리보기:</span>
                <DiscordRoleBadge role={activeRole} showPriority size="md" />
              </div>
            </div>

            {/* Classification & Scope */}
            <div className="grid grid-cols-2 gap-3 pb-4 border-b border-[var(--border)]">
              <div>
                <label className="block text-xs font-semibold text-[var(--ink)] mb-1">
                  역할 분류 (Category)
                </label>
                <select
                  value={activeRole.category}
                  onChange={(e) =>
                    updateRoleDefinition(activeRole.id, {
                      category: e.target.value as RoleDefinition["category"],
                    })
                  }
                  className="w-full px-2.5 py-1.5 text-xs rounded-md bg-[var(--surface)] border border-[var(--border)] text-[var(--ink)]"
                >
                  <option value="functional">기능 직무 (Functional)</option>
                  <option value="positional">직위/직책 (Positional)</option>
                  <option value="jurisdiction">관할 구역 (Jurisdiction)</option>
                  <option value="responsibility">특별 책무 (Responsibility)</option>
                  <option value="delegation">임시 대결 (Delegation)</option>
                </select>
              </div>

              <div>
                <label className="block text-xs font-semibold text-[var(--ink)] mb-1">
                  관할 단위 (Scope Type)
                </label>
                <select
                  value={activeRole.scopeType}
                  onChange={(e) =>
                    updateRoleDefinition(activeRole.id, {
                      scopeType: e.target.value as RoleDefinition["scopeType"],
                    })
                  }
                  className="w-full px-2.5 py-1.5 text-xs rounded-md bg-[var(--surface)] border border-[var(--border)] text-[var(--ink)]"
                >
                  <option value="platform">전사 플랫폼 (Platform-wide)</option>
                  <option value="group">기업집단/그룹 (Group-wide)</option>
                  <option value="entity">법인 한정 (Company Code)</option>
                  <option value="site">사업장 한정 (Plant / Site)</option>
                  <option value="department">부서 한정 (Cost Center)</option>
                </select>
              </div>
            </div>

            {/* High Security Conditions */}
            <div className="space-y-2 pb-4 border-b border-[var(--border)]">
              <span className="text-xs font-bold text-[var(--steel)] tracking-wider uppercase">
                고위험 보호 조건 (FIDO2 & DoA)
              </span>

              <label className="flex items-center gap-2 p-2.5 rounded-lg border border-[var(--border)] bg-[var(--muted)]/40 cursor-pointer">
                <input
                  type="checkbox"
                  checked={!!activeRole.conditions?.requireMfa}
                  onChange={(e) =>
                    updateRoleDefinition(activeRole.id, {
                      conditions: {
                        ...activeRole.conditions,
                        requireMfa: e.target.checked,
                      },
                    })
                  }
                  className="rounded text-indigo-600 focus:ring-indigo-500"
                />
                <div className="flex-1 text-xs">
                  <div className="font-semibold text-[var(--ink)] flex items-center gap-1.5">
                    <Fingerprint size={14} className="text-amber-500" />
                    <span>하드웨어 FIDO2 / Passkey 생체인증 강제</span>
                  </div>
                  <p className="text-[11px] text-[var(--steel)]">
                    전자서명법 제3조에 의거, 고위험 자금 집행 및 최종 대장 서명 시 생체인증을 요구합니다.
                  </p>
                </div>
              </label>

              {activeRole.grantedCapabilities.some((c) => c.includes("SignTier1")) && (
                <div className="p-2.5 rounded-lg border border-[var(--border)] bg-[var(--muted)]/40 text-xs">
                  <span className="font-semibold text-[var(--ink)]">DoA 1단계 전결 상한선:</span>
                  <span className="ml-2 font-mono font-bold text-indigo-600">₩5,000,000 이하</span>
                  <span className="text-[11px] text-[var(--steel)] block mt-0.5">
                    사내 전결규정 제4조에 의거하여 500만원 초과 시 상위 임원 결재가 강제됩니다.
                  </span>
                </div>
              )}
            </div>

            {/* Granted Capabilities Checklist */}
            <div className="space-y-3">
              <div className="flex items-center justify-between">
                <span className="text-xs font-bold text-[var(--steel)] tracking-wider uppercase">
                  인가 권한 번들 ({activeRole.grantedCapabilities.length}건 부여됨)
                </span>
                <span className="text-[11px] text-[var(--steel)]">
                  Cedar PBAC Permit 매핑
                </span>
              </div>

              <div className="space-y-1.5 max-h-[280px] overflow-y-auto pr-1">
                {AVAILABLE_CAPABILITIES.map((cap) => {
                  const isChecked = activeRole.grantedCapabilities.includes(cap.id);
                  return (
                    <label
                      key={cap.id}
                      onClick={() => handleToggleCapability(cap.id)}
                      className={`flex items-start gap-2.5 p-2 rounded-md border text-xs cursor-pointer transition select-none ${
                        isChecked
                          ? "bg-indigo-50/50 dark:bg-indigo-950/30 border-indigo-300 dark:border-indigo-800"
                          : "border-[var(--border)] hover:bg-[var(--muted)] opacity-70"
                      }`}
                    >
                      <input
                        type="checkbox"
                        checked={isChecked}
                        onChange={() => {}} // Handled by label click
                        className="mt-0.5 rounded text-indigo-600 focus:ring-indigo-500"
                      />
                      <div className="min-w-0 flex-1">
                        <div className="flex items-center justify-between">
                          <span className="font-semibold text-[var(--ink)]">
                            {cap.label}
                          </span>
                          <span className="text-[10px] font-mono px-1 rounded bg-black/5 dark:bg-white/10 text-[var(--steel)]">
                            {cap.cat}
                          </span>
                        </div>
                        <div className="flex items-center gap-2 text-[10px] text-[var(--steel)] mt-0.5">
                          <span className="font-mono text-indigo-600 dark:text-indigo-400">
                            {cap.id}
                          </span>
                          <span>•</span>
                          <span>근거: {cap.law}</span>
                        </div>
                      </div>
                    </label>
                  );
                })}
              </div>
            </div>
          </div>
        ) : null}

        {/* Pane 3: Members holding this role (Right, 3 cols) */}
        {activeRole ? (
          <div className="lg:col-span-3 p-4 flex flex-col bg-[var(--surface)]">
            <div className="flex items-center justify-between pb-3 border-b border-[var(--border)]">
              <div>
                <span className="text-xs font-bold text-[var(--steel)] tracking-wider uppercase">
                  역할 보유 구성원
                </span>
                <span className="text-xs text-[var(--steel)] block font-mono">
                  {activeRoleAssignments.length}명 배정됨
                </span>
              </div>
              <Button
                size="xs"
                variant="outline"
                leftIcon={<UserPlus size={12} />}
                onClick={() => setIsAddingMember(!isAddingMember)}
              >
                {isAddingMember ? "닫기" : "배정"}
              </Button>
            </div>

            {/* Quick Add Member Drawer/Box */}
            {isAddingMember && (
              <div className="p-3 my-2 bg-indigo-50/70 dark:bg-indigo-950/40 border border-indigo-200 dark:border-indigo-800 rounded-lg space-y-2">
                <span className="text-xs font-bold text-indigo-900 dark:text-indigo-200">
                  구성원 검색 및 즉시 배정
                </span>
                <input
                  type="text"
                  placeholder="이름 또는 부서 입력..."
                  value={memberAddSearch}
                  onChange={(e) => setMemberAddSearch(e.target.value)}
                  className="w-full px-2 py-1 text-xs rounded bg-white dark:bg-slate-900 border border-[var(--border)] text-[var(--ink)]"
                />
                <div className="max-h-36 overflow-y-auto divide-y divide-[var(--border)]">
                  {employees
                    .filter((e) => {
                      const matches =
                        e.name.includes(memberAddSearch) ||
                        e.dept.includes(memberAddSearch);
                      const alreadyHas = activeRoleAssignments.some(
                        (a) => a.employeeId === e.id
                      );
                      return matches && !alreadyHas;
                    })
                    .slice(0, 5)
                    .map((emp) => (
                      <div
                        key={emp.id}
                        className="py-1.5 flex items-center justify-between text-xs"
                      >
                        <div>
                          <span className="font-semibold text-[var(--ink)]">
                            {emp.name}
                          </span>
                          <span className="text-[10px] text-[var(--steel)] ml-1">
                            ({emp.dept} · {emp.grade || emp.role})
                          </span>
                        </div>
                        <Button
                          size="xs"
                          variant="brand"
                          onClick={() => {
                            assignRoleToEmployee(
                              emp.id,
                              activeRole.id,
                              { type: "all" },
                              "관리자 화면 직접 배정"
                            );
                            setIsAddingMember(false);
                            setMemberAddSearch("");
                          }}
                        >
                          부여
                        </Button>
                      </div>
                    ))}
                </div>
              </div>
            )}

            {/* Members List */}
            <div className="flex-1 overflow-y-auto divide-y divide-[var(--border)] mt-2">
              {activeRoleAssignments.length === 0 ? (
                <div className="py-8 text-center text-xs text-[var(--steel)]">
                  현재 이 역할을 보유한 구성원이 없습니다.
                </div>
              ) : (
                activeRoleAssignments.map((assignment) => {
                  const emp = employees.find(
                    (e) => e.id === assignment.employeeId || e.code === assignment.employeeId
                  );
                  return (
                    <div
                      key={assignment.id}
                      className="py-2.5 flex items-center justify-between group"
                    >
                      <div className="min-w-0 pr-2">
                        <div className="flex items-center gap-1.5">
                          <span className="text-xs font-semibold text-[var(--ink)]">
                            {emp?.name || assignment.personId}
                          </span>
                          <span className="text-[10px] px-1 rounded bg-[var(--muted)] text-[var(--steel)]">
                            {emp?.grade || "사원"}
                          </span>
                        </div>
                        <div className="text-[11px] text-[var(--steel)] truncate">
                          {emp?.dept} • {assignment.scope.targetName || "전사"}
                        </div>
                        <div className="text-[10px] text-slate-400 dark:text-slate-500 font-mono truncate">
                          사유: {assignment.grantReason}
                        </div>
                      </div>

                      <button
                        type="button"
                        onClick={() => removeRoleFromEmployee(assignment.id)}
                        className="opacity-0 group-hover:opacity-100 p-1 text-slate-400 hover:text-rose-500 hover:bg-rose-50 dark:hover:bg-rose-950/40 rounded transition shrink-0"
                        title="역할 회수"
                      >
                        <X size={14} />
                      </button>
                    </div>
                  );
                })
              )}
            </div>

            {/* Audit & Legal Note footer */}
            <div className="pt-3 mt-auto border-t border-[var(--border)] text-[10px] text-[var(--steel)] leading-relaxed">
              <span className="font-semibold text-slate-700 dark:text-slate-300">
                요청 시점 실시간 권한 산출 (무지연 인가):
              </span>{" "}
              역할 배정 및 회수는 요청 즉시 실효 권한 폴드에 반영되며, 캐시 지연 없이
              즉시 강제됩니다.
            </div>
          </div>
        ) : null}
      </div>
    </div>
  );
}
