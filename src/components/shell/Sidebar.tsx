"use client";

import React from "react";
import Link from "next/link";
import { usePathname } from "next/navigation";
import { clsx } from "clsx";
import { useAppStore } from "@/lib/store";
import {
  Home,
  FileText,
  Clock,
  DollarSign,
  Users,
  Wrench,
  MessageSquare,
  Mail,
  Shield,
  Layers,
  ChevronLeft,
  ChevronRight,
  Sun,
  Moon,
  Building,
  CalendarCheck,
  Award,
  Sliders,
  ShieldCheck,
  BookOpen,
} from "lucide-react";

export function Sidebar() {
  const pathname = usePathname();
  const collapsed = useAppStore((s) => s.sidebarCollapsed);
  const toggleSidebar = useAppStore((s) => s.toggleSidebar);
  const theme = useAppStore((s) => s.theme);
  const toggleTheme = useAppStore((s) => s.toggleTheme);

  const navGroups = [
    {
      title: "내 업무 (Work)",
      items: [
        { label: "홈 워크스페이스", href: "/dashboard", icon: Home },
        { label: "전자결재 (AP-)", href: "/approvals", icon: FileText, badge: "3" },
        { label: "현장 작업오더 (WO-)", href: "/ops/work-orders", icon: Wrench },
      ],
    },
    {
      title: "구성원 · 조직 (People)",
      items: [
        { label: "직원 및 고용 원장", href: "/hr/people", icon: Users },
        { label: "인사 발령 대장 (PA-)", href: "/hr/actions", icon: Award, badge: "발령" },
        { label: "조직 및 사업장 체계", href: "/org/setup", icon: Building },
        { label: "근태 관리 (주52h)", href: "/hr/attendance", icon: Clock, badge: "예외" },
        { label: "급여 산정 및 펌뱅킹", href: "/hr/payroll", icon: DollarSign },
        { label: "연차 사용촉진 (§61)", href: "/hr/leave", icon: CalendarCheck },
      ],
    },
    {
      title: "운영 · 자산 (Operations)",
      items: [
        { label: "도급 계약 · 장비 마스터", href: "/ops/setup", icon: Layers },
      ],
    },
    {
      title: "소통 · 협업 (Communications)",
      items: [
        { label: "사내 메신저", href: "/comms/messenger", icon: MessageSquare },
        { label: "사내 메일", href: "/comms/mail", icon: Mail },
      ],
    },
    {
      title: "데이터 · 거버넌스 (Data)",
      items: [
        { label: "권한 거버넌스 · 진단 (Cedar)", href: "/gov/access", icon: Shield, badge: "SoD" },
        { label: "감사 로그 (해시체인)", href: "/gov/audit", icon: ShieldCheck },
        { label: "온톨로지 객체 탐색기", href: "/gov/ontology", icon: Layers },
        { label: "용어 사전 · 법률 거버넌스", href: "/gov/glossary", icon: BookOpen },
      ],
    },
    {
      title: "시스템 · 관리 (Administration)",
      items: [
        { label: "사규 · 전결 정책 (DoA)", href: "/org/policy", icon: Sliders },
      ],
    },
  ];

  return (
    <aside
      className={clsx(
        "flex flex-col border-r border-[var(--border)] bg-[var(--surface)] select-none transition-all duration-200 z-30 shrink-0",
        collapsed ? "w-16" : "w-60"
      )}
    >
      {/* 로고 & 법인 선택 헤더 */}
      <div className="flex flex-col border-b border-[var(--border)] p-3 gap-2">
        <div className="flex items-center justify-between">
          <div className="flex items-center gap-2 overflow-hidden">
            {/* 앰버 라운드 로고 마크 (Acme / Oyatie DS 사양) */}
            <div className="flex items-center justify-center w-7 h-7 rounded-md bg-[var(--signal)] text-[var(--ink)] font-black text-sm shrink-0 shadow-xs">
              O
            </div>
            {!collapsed && (
              <div className="flex flex-col truncate">
                <span className="font-bold text-xs tracking-tight text-[var(--ink)] truncate">OYATIE CONSOLE</span>
                <span className="text-[10px] text-[var(--faint)] tracking-widest font-mono">ENTERPRISE v1.0</span>
              </div>
            )}
          </div>
          <button
            onClick={toggleSidebar}
            className="p-1 rounded text-[var(--steel)] hover:bg-[var(--muted)] cursor-pointer"
            title={collapsed ? "사이드바 펼치기" : "사이드바 접기"}
          >
            {collapsed ? <ChevronRight className="w-4 h-4" /> : <ChevronLeft className="w-4 h-4" />}
          </button>
        </div>

        {/* 통합 업무 환경 거버넌스 뱃지 */}
        {!collapsed && (
          <div className="flex items-center gap-1.5 px-2 py-1 rounded bg-[var(--muted)]/60 border border-[var(--border)] text-[10px] text-[var(--steel)]">
            <span className="w-1.5 h-1.5 rounded-full bg-emerald-500" />
            <span className="truncate font-medium">통합 엔터프라이즈 업무 공간</span>
          </div>
        )}
      </div>

      {/* 네비게이션 메뉴 목록 */}
      <nav className="flex-1 overflow-y-auto p-2 space-y-4">
        {navGroups.map((group, gIdx) => (
          <div key={gIdx} className="space-y-1">
            {!collapsed && (
              <div className="px-2 py-1 text-[10px] font-semibold tracking-wider text-[var(--faint)] uppercase">
                {group.title}
              </div>
            )}
            {group.items.map((item) => {
              const Icon = item.icon;
              const isActive = pathname === item.href || (item.href !== "/dashboard" && pathname.startsWith(item.href));
              return (
                <Link
                  key={item.href}
                  href={item.href}
                  className={clsx(
                    "flex items-center gap-2.5 px-2.5 py-1.5 rounded-md text-xs font-medium transition-colors group",
                    isActive
                      ? "bg-amber-500/10 text-[var(--ink)] font-semibold border-l-2 border-l-[var(--signal)] rounded-l-none"
                      : "text-[var(--steel)] hover:bg-[var(--muted)] hover:text-[var(--ink)]"
                  )}
                  title={collapsed ? item.label : undefined}
                >
                  <Icon
                    className={clsx(
                      "w-4 h-4 shrink-0 transition-colors",
                      isActive ? "text-[var(--signal-deep)]" : "text-[var(--steel)] group-hover:text-[var(--ink)]"
                    )}
                  />
                  {!collapsed && (
                    <span className="truncate flex-1">{item.label}</span>
                  )}
                  {!collapsed && item.badge && (
                    <span className="px-1.5 py-0.2 rounded text-[10px] font-mono font-bold bg-amber-100 text-amber-900">
                      {item.badge}
                    </span>
                  )}
                </Link>
              );
            })}
          </div>
        ))}
      </nav>

      {/* 하단 유틸리티 (테마 토글 & 버전) */}
      <div className="border-t border-[var(--border)] p-2 flex items-center justify-between">
        <button
          onClick={toggleTheme}
          className="flex items-center gap-2 px-2 py-1 rounded text-xs text-[var(--steel)] hover:bg-[var(--muted)] cursor-pointer"
          title="테마 전환"
        >
          {theme === "dark" ? <Sun className="w-3.5 h-3.5 text-amber-400" /> : <Moon className="w-3.5 h-3.5" />}
          {!collapsed && <span>{theme === "dark" ? "라이트 모드" : "다크 모드"}</span>}
        </button>
        {!collapsed && <span className="text-[10px] text-[var(--faint)] font-mono">KR-2026</span>}
      </div>
    </aside>
  );
}
