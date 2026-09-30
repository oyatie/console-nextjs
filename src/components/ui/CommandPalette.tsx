"use client";

import React, { useEffect } from "react";
import { Command } from "cmdk";
import { useRouter } from "next/navigation";
import { useAppStore } from "@/lib/store";
import {
  FileText,
  Clock,
  DollarSign,
  Users,
  Wrench,
  MessageSquare,
  Shield,
  Layers,
  PlusCircle,
  Home,
  Building,
  Mail,
  BookOpen,
  Award,
  CalendarCheck,
  Sliders,
  ShieldCheck,
} from "lucide-react";

export function CommandPalette() {
  const router = useRouter();
  const isOpen = useAppStore((s) => s.commandPaletteOpen);
  const setIsOpen = useAppStore((s) => s.setCommandPaletteOpen);
  const openInspector = useAppStore((s) => s.openObjectInspector);
  const approvals = useAppStore((s) => s.approvals);
  const workOrders = useAppStore((s) => s.workOrders);
  const employees = useAppStore((s) => s.employees);

  useEffect(() => {
    function handleKeyDown(e: KeyboardEvent) {
      if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "k") {
        e.preventDefault();
        setIsOpen(!isOpen);
      }
    }
    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, [isOpen, setIsOpen]);

  if (!isOpen) return null;

  const handleSelect = (callback: () => void) => {
    callback();
    setIsOpen(false);
  };

  return (
    <div className="fixed inset-0 z-50 flex items-start justify-center pt-24 bg-black/40 backdrop-blur-xs animate-in fade-in duration-100">
      <div
        className="relative w-full max-w-xl rounded-lg border border-[var(--border)] bg-[var(--surface)] shadow-2xl overflow-hidden"
        onClick={(e) => e.stopPropagation()}
      >
        <Command className="w-full">
          <div className="flex items-center px-3 border-b border-[var(--border)]">
            <Command.Input
              autoFocus
              placeholder="명령어, 화면, 결재문서(AP-), 작업오더(WO-), 직원 검색... (Cmd+K)"
              className="w-full h-11 text-sm bg-transparent text-[var(--ink)] placeholder:text-[var(--faint)] focus:outline-none"
            />
            <button
              onClick={() => setIsOpen(false)}
              className="text-xs px-1.5 py-0.5 rounded border border-[var(--border)] text-[var(--steel)] cursor-pointer"
            >
              ESC
            </button>
          </div>

          <Command.List className="max-h-80 overflow-y-auto p-2 text-xs space-y-1">
            <Command.Empty className="py-6 text-center text-[var(--faint)]">
              일치하는 항목이 없습니다.
            </Command.Empty>

            <Command.Group heading="빠른 액션 (Quick Actions)" className="text-[11px] font-semibold text-[var(--steel)] px-2 py-1">
              <Command.Item
                onSelect={() => handleSelect(() => router.push("/approvals?action=new"))}
                className="flex items-center gap-2 px-2.5 py-1.5 rounded-md text-[var(--ink)] hover:bg-[var(--muted)] cursor-pointer"
              >
                <PlusCircle className="w-3.5 h-3.5 text-[var(--signal-deep)]" />
                <span>+ 전자결재 기안문서 작성 (SAP Document Parking)</span>
              </Command.Item>
              <Command.Item
                onSelect={() => handleSelect(() => router.push("/hr/attendance?action=resolve"))}
                className="flex items-center gap-2 px-2.5 py-1.5 rounded-md text-[var(--ink)] hover:bg-[var(--muted)] cursor-pointer"
              >
                <Clock className="w-3.5 h-3.5 text-orange-500" />
                <span>+ 근태 52시간 예외 소명 심사 (AT-0703)</span>
              </Command.Item>
            </Command.Group>

            <Command.Group heading="콘솔 메뉴 (Screens)" className="text-[11px] font-semibold text-[var(--steel)] px-2 py-1 mt-2">
              <Command.Item
                onSelect={() => handleSelect(() => router.push("/dashboard"))}
                className="flex items-center gap-2 px-2.5 py-1.5 rounded-md text-[var(--ink)] hover:bg-[var(--muted)] cursor-pointer"
              >
                <Home className="w-3.5 h-3.5" />
                <span>개요 대시보드 (Overview Launchpad)</span>
              </Command.Item>
              <Command.Item
                onSelect={() => handleSelect(() => router.push("/approvals"))}
                className="flex items-center gap-2 px-2.5 py-1.5 rounded-md text-[var(--ink)] hover:bg-[var(--muted)] cursor-pointer"
              >
                <FileText className="w-3.5 h-3.5 text-blue-600" />
                <span>전자결재 수신함 / 기안함 (Approvals)</span>
              </Command.Item>
              <Command.Item
                onSelect={() => handleSelect(() => router.push("/hr/attendance"))}
                className="flex items-center gap-2 px-2.5 py-1.5 rounded-md text-[var(--ink)] hover:bg-[var(--muted)] cursor-pointer"
              >
                <Clock className="w-3.5 h-3.5 text-orange-600" />
                <span>근태 관리 & 주52시간 게이트 (Attendance)</span>
              </Command.Item>
              <Command.Item
                onSelect={() => handleSelect(() => router.push("/hr/payroll"))}
                className="flex items-center gap-2 px-2.5 py-1.5 rounded-md text-[var(--ink)] hover:bg-[var(--muted)] cursor-pointer"
              >
                <DollarSign className="w-3.5 h-3.5 text-emerald-600" />
                <span>급여 계산 & 4대보험 마감 (Payroll)</span>
              </Command.Item>
              <Command.Item
                onSelect={() => handleSelect(() => router.push("/hr/people"))}
                className="flex items-center gap-2 px-2.5 py-1.5 rounded-md text-[var(--ink)] hover:bg-[var(--muted)] cursor-pointer"
              >
                <Users className="w-3.5 h-3.5 text-indigo-600" />
                <span>인사 대장 & 조직도 (Workforce)</span>
              </Command.Item>
              <Command.Item
                onSelect={() => handleSelect(() => router.push("/ops/work-orders"))}
                className="flex items-center gap-2 px-2.5 py-1.5 rounded-md text-[var(--ink)] hover:bg-[var(--muted)] cursor-pointer"
              >
                <Wrench className="w-3.5 h-3.5 text-amber-600" />
                <span>정비 & 현장 배차 오더 (Work Orders)</span>
              </Command.Item>
              <Command.Item
                onSelect={() => handleSelect(() => router.push("/comms/messenger"))}
                className="flex items-center gap-2 px-2.5 py-1.5 rounded-md text-[var(--ink)] hover:bg-[var(--muted)] cursor-pointer"
              >
                <MessageSquare className="w-3.5 h-3.5 text-teal-600" />
                <span>사내 메신저 & 슬랙형 협업 (Comms)</span>
              </Command.Item>
              <Command.Item
                onSelect={() => handleSelect(() => router.push("/comms/mail"))}
                className="flex items-center gap-2 px-2.5 py-1.5 rounded-md text-[var(--ink)] hover:bg-[var(--muted)] cursor-pointer"
              >
                <Mail className="w-3.5 h-3.5 text-blue-500" />
                <span>사내 웹메일 3-Pane (Mail)</span>
              </Command.Item>
              <Command.Item
                onSelect={() => handleSelect(() => router.push("/hr/actions"))}
                className="flex items-center gap-2 px-2.5 py-1.5 rounded-md text-[var(--ink)] hover:bg-[var(--muted)] cursor-pointer"
              >
                <Award className="w-3.5 h-3.5 text-yellow-600" />
                <span>인사 발령 대장 (Personnel Actions PA-)</span>
              </Command.Item>
              <Command.Item
                onSelect={() => handleSelect(() => router.push("/hr/leave"))}
                className="flex items-center gap-2 px-2.5 py-1.5 rounded-md text-[var(--ink)] hover:bg-[var(--muted)] cursor-pointer"
              >
                <CalendarCheck className="w-3.5 h-3.5 text-green-600" />
                <span>연차 사용촉진 관리 (LSA §61 Leave)</span>
              </Command.Item>
              <Command.Item
                onSelect={() => handleSelect(() => router.push("/ops/setup"))}
                className="flex items-center gap-2 px-2.5 py-1.5 rounded-md text-[var(--ink)] hover:bg-[var(--muted)] cursor-pointer"
              >
                <Layers className="w-3.5 h-3.5 text-amber-700" />
                <span>도급 계약 & 장비 마스터 (Operations Setup)</span>
              </Command.Item>
              <Command.Item
                onSelect={() => handleSelect(() => router.push("/org/setup"))}
                className="flex items-center gap-2 px-2.5 py-1.5 rounded-md text-[var(--ink)] hover:bg-[var(--muted)] cursor-pointer"
              >
                <Building className="w-3.5 h-3.5 text-slate-600" />
                <span>조직 및 사업장 체계 (Organization Setup)</span>
              </Command.Item>
              <Command.Item
                onSelect={() => handleSelect(() => router.push("/org/policy"))}
                className="flex items-center gap-2 px-2.5 py-1.5 rounded-md text-[var(--ink)] hover:bg-[var(--muted)] cursor-pointer"
              >
                <Sliders className="w-3.5 h-3.5 text-indigo-700" />
                <span>사규 및 전결 정책 (DoA Governance)</span>
              </Command.Item>
              <Command.Item
                onSelect={() => handleSelect(() => router.push("/gov/access"))}
                className="flex items-center gap-2 px-2.5 py-1.5 rounded-md text-[var(--ink)] hover:bg-[var(--muted)] cursor-pointer"
              >
                <ShieldCheck className="w-3.5 h-3.5 text-emerald-600" />
                <span>Cedar PBAC 권한 거버넌스 및 SoD 진단</span>
              </Command.Item>
              <Command.Item
                onSelect={() => handleSelect(() => router.push("/gov/audit"))}
                className="flex items-center gap-2 px-2.5 py-1.5 rounded-md text-[var(--ink)] hover:bg-[var(--muted)] cursor-pointer"
              >
                <Shield className="w-3.5 h-3.5 text-purple-600" />
                <span>감사 로그 & 무결성 해시체인 (Audit)</span>
              </Command.Item>
              <Command.Item
                onSelect={() => handleSelect(() => router.push("/gov/ontology"))}
                className="flex items-center gap-2 px-2.5 py-1.5 rounded-md text-[var(--ink)] hover:bg-[var(--muted)] cursor-pointer"
              >
                <Layers className="w-3.5 h-3.5 text-pink-600" />
                <span>온톨로지 객체 탐색기 (Palantir Foundry)</span>
              </Command.Item>
              <Command.Item
                onSelect={() => handleSelect(() => router.push("/gov/glossary"))}
                className="flex items-center gap-2 px-2.5 py-1.5 rounded-md text-[var(--ink)] hover:bg-[var(--muted)] cursor-pointer"
              >
                <BookOpen className="w-3.5 h-3.5 text-teal-600" />
                <span>용어 사전 & 법률 조항 거버넌스 (Glossary)</span>
              </Command.Item>
            </Command.Group>

            <Command.Group heading="활성 개체 (Live Objects)" className="text-[11px] font-semibold text-[var(--steel)] px-2 py-1 mt-2">
              {approvals.map((a) => (
                <Command.Item
                  key={a.id}
                  onSelect={() => handleSelect(() => openInspector(a.code))}
                  className="flex items-center justify-between px-2.5 py-1.5 rounded-md text-[var(--ink)] hover:bg-[var(--muted)] cursor-pointer"
                >
                  <div className="flex items-center gap-2 truncate">
                    <span className="font-mono font-bold text-blue-600">{a.code}</span>
                    <span className="truncate">{a.title}</span>
                  </div>
                  <span className="text-[10px] text-[var(--faint)]">{a.status}</span>
                </Command.Item>
              ))}
              {workOrders.map((w) => (
                <Command.Item
                  key={w.id}
                  onSelect={() => handleSelect(() => openInspector(w.code))}
                  className="flex items-center justify-between px-2.5 py-1.5 rounded-md text-[var(--ink)] hover:bg-[var(--muted)] cursor-pointer"
                >
                  <div className="flex items-center gap-2 truncate">
                    <span className="font-mono font-bold text-amber-600">{w.code}</span>
                    <span className="truncate">{w.title}</span>
                  </div>
                  <span className="text-[10px] text-[var(--faint)]">{w.status}</span>
                </Command.Item>
              ))}
            </Command.Group>

            <Command.Group heading="직원 디렉토리 (People)" className="text-[11px] font-semibold text-[var(--steel)] px-2 py-1 mt-2">
              {employees.slice(0, 5).map((emp) => (
                <Command.Item
                  key={emp.id}
                  onSelect={() => handleSelect(() => router.push(`/hr/people?id=${emp.id}`))}
                  className="flex items-center justify-between px-2.5 py-1.5 rounded-md text-[var(--ink)] hover:bg-[var(--muted)] cursor-pointer"
                >
                  <div className="flex items-center gap-2">
                    <span className="font-bold">{emp.name}</span>
                    <span className="text-[var(--steel)]">{emp.dept} · {emp.position}</span>
                  </div>
                  <span className="font-mono text-[10px] text-[var(--faint)]">{emp.code}</span>
                </Command.Item>
              ))}
            </Command.Group>
          </Command.List>
        </Command>
      </div>
    </div>
  );
}
