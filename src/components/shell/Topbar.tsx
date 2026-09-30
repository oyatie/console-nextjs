"use client";

import React from "react";
import { usePathname } from "next/navigation";
import { useAppStore } from "@/lib/store";
import {
  Search,
  Plus,
  KeyRound,
  PanelRight,
  ShieldCheck,
  UserCheck,
  HelpCircle,
  RotateCcw,
  ChevronDown,
} from "lucide-react";
import { Button } from "../ui/Button";
import { DiscordRoleBadge } from "@/components/gov/DiscordRoleBadge";
import { RoleDefinition } from "@/lib/types";

export function Topbar() {
  const pathname = usePathname();
  const setCommandPaletteOpen = useAppStore((s) => s.setCommandPaletteOpen);
  const setShortcutModalOpen = useAppStore((s) => s.setShortcutModalOpen);
  const resetToSeedData = useAppStore((s) => s.resetToSeedData);
  const rightRailOpen = useAppStore((s) => s.rightRailOpen);
  const toggleRightRail = useAppStore((s) => s.toggleRightRail);
  const viewAsRole = useAppStore((s) => s.viewAsRole);
  const setViewAsRole = useAppStore((s) => s.setViewAsRole);
  const employees = useAppStore((s) => s.employees);
  const roleDefinitions = useAppStore((s) => s.roleDefinitions);
  const roleAssignments = useAppStore((s) => s.roleAssignments);

  const [personaMenuOpen, setPersonaMenuOpen] = React.useState(false);

  // Active person corresponding to viewAsRole
  const currentPerson =
    employees.find((e) => e.role === viewAsRole || e.grade === viewAsRole) ||
    employees[1] || // 김인사 default
    employees[0];

  const currentPersonAssignments = roleAssignments.filter(
    (a) =>
      (a.employeeId === currentPerson?.id || a.personId === `P-${currentPerson?.id}`) &&
      a.status === "active"
  );

  const currentPersonRoles = currentPersonAssignments
    .map((a) => roleDefinitions.find((r) => r.id === a.roleId))
    .filter((r): r is RoleDefinition => Boolean(r))
    .sort((a, b) => b.priority - a.priority);

  const topRole = currentPersonRoles[0];

  // 경로에 따른 Breadcrumb 레이블 추출
  const getBreadcrumb = () => {
    if (pathname.startsWith("/approvals")) return ["전자결재", "결재 및 인테이크 대장"];
    if (pathname.startsWith("/org/setup")) return ["조직 거버넌스", "법인 · 사업장 · 부서 체계"];
    if (pathname.startsWith("/org/policy")) return ["조직 거버넌스", "근태 사규 · 전결권한 · 연차 정책"];
    if (pathname.startsWith("/hr/actions")) return ["인사 · 노무", "인사 발령 대장 (PA-)"];
    if (pathname.startsWith("/hr/attendance")) return ["인사 · 노무", "근태 관리 & 주52시간 게이트"];
    if (pathname.startsWith("/hr/payroll")) return ["인사 · 노무", "급여 산정 & 펌뱅킹 이체"];
    if (pathname.startsWith("/hr/leave")) return ["인사 · 노무", "연차 사용촉진 (§61)"];
    if (pathname.startsWith("/hr/people")) return ["인사 · 노무", "직원 디렉토리 & 편성"];
    if (pathname.startsWith("/ops/setup")) return ["운영 · 정비", "도급 계약 & 장비 마스터"];
    if (pathname.startsWith("/ops/work-orders")) return ["운영 · 정비", "현장 작업오더 & 장비 관리"];
    if (pathname.startsWith("/comms/messenger")) return ["협업 · 소통", "사내 메신저 채널"];
    if (pathname.startsWith("/comms/mail")) return ["협업 · 소통", "웹메일 (3-Pane)"];
    if (pathname.startsWith("/gov/audit")) return ["거버넌스", "감사 로그 & 무결성 해시체인"];
    if (pathname.startsWith("/gov/ontology")) return ["거버넌스", "Palantir 온톨로지 탐색기"];
    if (pathname.startsWith("/gov/glossary")) return ["거버넌스", "용어 사전 · 법률 거버넌스"];
    if (pathname.startsWith("/gov/access")) return ["거버넌스", "Cedar PBAC 권한 거버넌스 및 진단"];
    return ["워크스페이스", "개요 대시보드"];
  };

  const [category, currentScreen] = getBreadcrumb();

  return (
    <header className="flex items-center justify-between h-12 px-4 border-b border-[var(--border)] bg-[var(--surface)] shrink-0 z-20">
      {/* 좌측: Breadcrumb & 화면 타이틀 */}
      <div className="flex items-center gap-2 text-xs">
        <span className="text-[var(--steel)]">{category}</span>
        <span className="text-[var(--faint)]">/</span>
        <span className="font-semibold text-[var(--ink)]">{currentScreen}</span>
      </div>

      {/* 중앙: Command Palette 검색 트리거 (Cmd+K) */}
      <button
        type="button"
        onClick={() => setCommandPaletteOpen(true)}
        className="hidden md:flex items-center gap-2 px-3 py-1.5 rounded-md border border-[var(--border)] bg-[var(--canvas)] text-xs text-[var(--faint)] hover:border-[var(--steel)] hover:text-[var(--ink)] transition-all cursor-pointer w-72"
      >
        <Search className="w-3.5 h-3.5" />
        <span className="flex-1 text-left">명령어 또는 개체 검색...</span>
        <kbd className="px-1.5 py-0.5 rounded bg-[var(--surface)] border border-[var(--border)] font-mono text-[10px] text-[var(--steel)] shadow-2xs">
          ⌘K
        </kbd>
      </button>

      {/* 우측 도구: 새 기안, View As, Passkey, 우측 레일 토글 */}
      <div className="flex items-center gap-2">
        {/* 새 기안 작성 버튼 */}
        <Button
          size="sm"
          variant="brand"
          leftIcon={<Plus className="w-3.5 h-3.5" />}
          onClick={() => setCommandPaletteOpen(true)}
        >
          기안 상신
        </Button>

        {/* 자연인 당사자(Party) & Discord 다중 역할 페르소나 전환기 */}
        <div className="relative">
          <button
            type="button"
            onClick={() => setPersonaMenuOpen(!personaMenuOpen)}
            className="flex items-center gap-2 px-2.5 py-1 rounded-lg bg-[var(--muted)] border border-[var(--border)] text-xs text-[var(--ink)] hover:border-indigo-400 transition cursor-pointer select-none"
            title="현재 로그인 당사자(Party) 및 보유 역할 레이어 확인 / 전환"
          >
            <div className="w-5 h-5 rounded-full bg-indigo-600 text-white text-[10px] font-bold flex items-center justify-center shrink-0">
              {currentPerson?.name?.[0] || "나"}
            </div>
            <div className="flex flex-col text-left leading-tight">
              <div className="flex items-center gap-1">
                <span className="font-bold text-[11px] text-[var(--ink)]">
                  {currentPerson?.name || "김인사"}
                </span>
                <span className="text-[10px] text-[var(--steel)]">
                  ({currentPerson?.grade || "팀장"})
                </span>
              </div>
            </div>

            {/* Top highest priority role badge */}
            {topRole && (
              <span
                style={{
                  backgroundColor: `${topRole.color}20`,
                  borderColor: `${topRole.color}50`,
                }}
                className="hidden xl:inline-flex items-center gap-1 px-1.5 py-0.2 rounded-full border text-[10px] font-medium"
              >
                <span
                  className="w-1.5 h-1.5 rounded-full"
                  style={{ backgroundColor: topRole.color }}
                />
                <span className="truncate max-w-[80px]">{topRole.name}</span>
              </span>
            )}
            <ChevronDown size={12} className="text-[var(--steel)]" />
          </button>

          {/* Persona & Layered Roles Popover */}
          {personaMenuOpen && (
            <div className="absolute right-0 top-full mt-1.5 w-80 rounded-xl bg-[var(--surface)] border border-[var(--border)] shadow-xl z-50 p-3 space-y-3 animate-in fade-in zoom-in-95 duration-100">
              <div className="flex items-center justify-between border-b border-[var(--border)] pb-2">
                <div>
                  <span className="text-[11px] font-bold text-[var(--steel)] uppercase tracking-wider block">
                    접속 계정 & 역할 폴드 (Fold)
                  </span>
                  <span className="text-xs font-bold text-[var(--ink)]">
                    {currentPerson?.name} ({currentPerson?.dept})
                  </span>
                </div>
                <span className="text-[10px] font-mono px-1.5 py-0.5 rounded bg-indigo-50 dark:bg-indigo-950 text-indigo-600 dark:text-indigo-400 font-semibold">
                  {currentPersonAssignments.length}개 레이어
                </span>
              </div>

              {/* Current user's layered role stack */}
              <div className="space-y-1.5">
                <span className="text-[10px] text-[var(--steel)] block">
                  현재 보유한 Discord식 역할 스택:
                </span>
                <div className="flex flex-wrap gap-1">
                  {currentPersonRoles.map((role) => (
                    <DiscordRoleBadge key={role.id} role={role} size="sm" showPriority />
                  ))}
                </div>
              </div>

              {/* Switch to other personas */}
              <div className="space-y-1 pt-2 border-t border-[var(--border)]">
                <span className="text-[10px] font-semibold text-[var(--steel)] block mb-1">
                  테스트 페르소나 전환 (Authority does not follow rank):
                </span>
                <div className="space-y-1 max-h-48 overflow-y-auto pr-1">
                  {employees.slice(0, 6).map((emp) => {
                    const empAssignments = roleAssignments.filter(
                      (a) => (a.employeeId === emp.id || a.personId === `P-${emp.id}`) && a.status === "active"
                    );
                    const isCurrent = currentPerson?.id === emp.id;

                    return (
                      <div
                        key={emp.id}
                        onClick={() => {
                          setViewAsRole(emp.role);
                          setPersonaMenuOpen(false);
                        }}
                        className={`p-2 rounded-lg text-xs flex items-center justify-between cursor-pointer transition ${
                          isCurrent
                            ? "bg-indigo-50/80 dark:bg-indigo-950/40 border border-indigo-300 dark:border-indigo-800 font-bold"
                            : "hover:bg-[var(--muted)] border border-transparent"
                        }`}
                      >
                        <div>
                          <div className="flex items-center gap-1.5">
                            <span className="text-[var(--ink)]">{emp.name}</span>
                            <span className="text-[10px] text-[var(--steel)] font-normal">
                              {emp.grade || emp.role}
                            </span>
                          </div>
                          <div className="text-[10px] text-[var(--steel)] font-normal">
                            {emp.dept} • {empAssignments.length}개 역할
                          </div>
                        </div>
                        {isCurrent && (
                          <span className="text-[10px] text-indigo-600 font-bold">
                            현재
                          </span>
                        )}
                      </div>
                    );
                  })}
                </div>
              </div>

              <div className="pt-2 border-t border-[var(--border)] text-[10px] text-[var(--steel)]">
                사내 권한 거버넌스 규정에 따라 직급이 아닌 명시적 역할 레이어에 의해 인가가 결정됩니다.
              </div>
            </div>
          )}
        </div>

        {/* Passkey / WebAuthn 활성 인증 상태 배지 */}
        <div
          className="hidden sm:flex items-center gap-1 px-2 py-1 rounded border border-emerald-200 bg-emerald-50 text-emerald-800 text-[11px] font-mono font-medium"
          title="FIDO2 WebAuthn Passkey 활성화 상태 — 법적 통지 열람 및 수령확인 인증 완료"
        >
          <KeyRound className="w-3 h-3 text-emerald-600" />
          <span>Passkey 온</span>
        </div>

        {/* 단축키 가이드 (?) 모달 열기 */}
        <button
          onClick={() => setShortcutModalOpen(true)}
          className="p-1.5 rounded border border-[var(--border)] text-[var(--steel)] hover:text-[var(--ink)] hover:bg-[var(--muted)] cursor-pointer transition-colors"
          title="단축키 치트시트 열기 (? 또는 Shift+/)"
        >
          <HelpCircle className="w-4 h-4" />
        </button>

        {/* 데이터 초기화 버튼 */}
        <button
          onClick={() => {
            if (window.confirm("모든 데이터를 기본 초기 시드 데이터로 리셋하시겠습니까?")) {
              resetToSeedData();
            }
          }}
          className="p-1.5 rounded border border-[var(--border)] text-[var(--steel)] hover:text-amber-600 hover:bg-amber-50 cursor-pointer transition-colors"
          title="초기 시드 데이터로 재설정 (Reset)"
        >
          <RotateCcw className="w-4 h-4" />
        </button>

        {/* 우측 커뮤니케이션 레일 열기/닫기 */}
        <button
          onClick={() => toggleRightRail()}
          className="p-1.5 rounded border border-[var(--border)] text-[var(--steel)] hover:text-[var(--ink)] hover:bg-[var(--muted)] cursor-pointer"
          title={rightRailOpen ? "우측 패널 닫기" : "우측 패널 열기 (스레드/메신저/검사기)"}
        >
          <PanelRight className="w-4 h-4" />
        </button>
      </div>
    </header>
  );
}
