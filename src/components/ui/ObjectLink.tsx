"use client";

import React, { useState, useRef } from "react";
import { clsx } from "clsx";
import { useAppStore } from "@/lib/store";
import {
  ExternalLink,
  Layers,
  FileCheck2,
  Wrench,
  Clock,
  User,
  DollarSign,
  FileText,
  CheckCircle2,
  ArrowRight,
  MessageSquare,
  AlertTriangle,
} from "lucide-react";
import { StatusChip } from "./StatusChip";

export interface ObjectLinkProps {
  code: string;
  label?: string;
  className?: string;
}

export function ObjectLink({ code, label, className }: ObjectLinkProps) {
  const openInspector = useAppStore((s) => s.openObjectInspector);
  const toggleRightRail = useAppStore((s) => s.toggleRightRail);
  const approveDoc = useAppStore((s) => s.approveDoc);
  const updateWorkOrderStatus = useAppStore((s) => s.updateWorkOrderStatus);

  // Store entities for rich live card
  const approvals = useAppStore((s) => s.approvals);
  const workOrders = useAppStore((s) => s.workOrders);
  const employees = useAppStore((s) => s.employees);
  const attendance = useAppStore((s) => s.attendance);
  const payslips = useAppStore((s) => s.payslips);
  const payrollRun = useAppStore((s) => s.payrollRun);

  const [hovered, setHovered] = useState(false);
  const closeTimeoutRef = useRef<NodeJS.Timeout | null>(null);

  // 코드 접두사에 따른 메타데이터 추출
  const prefix = code.split("-")[0];
  let badgeTone = "border-[var(--border)] bg-[var(--surface)] text-[var(--ink)]";
  let EntityIcon = Layers;
  let kindLabel = "개체";

  if (prefix === "AP") {
    badgeTone = "border-blue-300 bg-blue-50/80 text-blue-900";
    EntityIcon = FileCheck2;
    kindLabel = "결재문서";
  } else if (prefix === "WO") {
    badgeTone = "border-amber-300 bg-amber-50/80 text-amber-900";
    EntityIcon = Wrench;
    kindLabel = "작업오더";
  } else if (prefix === "AT") {
    badgeTone = "border-orange-300 bg-orange-50/80 text-orange-900";
    EntityIcon = Clock;
    kindLabel = "근태기록";
  } else if (prefix === "EMP") {
    badgeTone = "border-slate-300 bg-slate-100 text-slate-900";
    EntityIcon = User;
    kindLabel = "사원";
  } else if (prefix === "PS") {
    badgeTone = "border-emerald-300 bg-emerald-50/80 text-emerald-900";
    EntityIcon = DollarSign;
    kindLabel = "급여명세서";
  } else if (prefix === "C") {
    badgeTone = "border-teal-300 bg-teal-50/80 text-teal-900";
    EntityIcon = FileText;
    kindLabel = "도급계약";
  } else if (prefix === "PR") {
    badgeTone = "border-purple-300 bg-purple-50/80 text-purple-900";
    EntityIcon = Layers;
    kindLabel = "급여차수";
  }

  // 실시간 개체 정보 확인
  let entityDetails: any = null;
  if (prefix === "AP") {
    const doc = approvals.find((a) => a.code === code);
    if (doc) {
      entityDetails = {
        title: doc.title,
        status: doc.status,
        subtext: `기안: ${doc.drafterName} (${doc.drafterDept}) · ${doc.category}`,
        amount: doc.doaAmount ? `${doc.doaAmount.toLocaleString()}원` : null,
        canApprove: doc.status === "결재대기" || doc.status === "초안",
        rawId: doc.id,
      };
    }
  } else if (prefix === "WO") {
    const wo = workOrders.find((w) => w.code === code);
    if (wo) {
      entityDetails = {
        title: wo.title,
        status: wo.status,
        subtext: `현장: ${wo.site} · 담당: ${wo.assignedTo}`,
        priority: wo.priority,
        rawId: wo.id,
      };
    }
  } else if (prefix === "EMP") {
    const emp = employees.find((e) => e.code === code);
    if (emp) {
      entityDetails = {
        title: `${emp.name} ${emp.role}`,
        status: emp.status,
        subtext: `${emp.dept} · ${emp.site}`,
        email: emp.email,
        phone: emp.phone,
      };
    }
  } else if (prefix === "AT") {
    const att = attendance.find((a) => a.exceptionCode === code);
    if (att) {
      entityDetails = {
        title: `${att.employeeName} 근태 예외`,
        status: att.status,
        subtext: `${att.site} · 실근로 ${att.workedHours}h (연장 ${att.otHours}h)`,
      };
    }
  } else if (prefix === "PS") {
    const ps = payslips.find((p) => p.code === code);
    if (ps) {
      entityDetails = {
        title: `${ps.employeeName} 급여명세서`,
        status: "지급대기",
        subtext: `${ps.yearMonth} 귀속 · 과세총액 ${ps.grossPay.toLocaleString()}원`,
        amount: `실수령 ${ps.netPay.toLocaleString()}원`,
      };
    }
  }

  const handleMouseEnter = () => {
    if (closeTimeoutRef.current) clearTimeout(closeTimeoutRef.current);
    setHovered(true);
  };

  const handleMouseLeave = () => {
    closeTimeoutRef.current = setTimeout(() => {
      setHovered(false);
    }, 200);
  };

  return (
    <span
      className="relative inline-block"
      onMouseEnter={handleMouseEnter}
      onMouseLeave={handleMouseLeave}
    >
      <button
        type="button"
        onClick={(e) => {
          e.stopPropagation();
          openInspector(code);
        }}
        className={clsx(
          "inline-flex items-center gap-1 px-1.5 py-0.5 rounded-[4px] border font-mono text-[11px] font-semibold tracking-tight transition-all hover:ring-2 hover:ring-[var(--signal)] hover:border-transparent active:scale-[0.98] cursor-pointer shadow-2xs",
          badgeTone,
          className
        )}
        title={`${code} 개체 상세 및 작업 원장 열기`}
      >
        <EntityIcon className="w-2.5 h-2.5 opacity-75 shrink-0" />
        <span>{code}</span>
        {label && <span className="font-sans font-normal opacity-85 pl-0.5">{label}</span>}
      </button>

      {/* 게임/Discord 스타일 리치 개체 미니 카드 (Live Entity Mini-Card) */}
      {hovered && (
        <div
          className="absolute z-50 bottom-full left-0 mb-2 w-72 rounded-lg border border-[var(--border)] bg-[var(--surface)] p-3 shadow-xl text-left animate-in fade-in zoom-in-95 duration-150 backdrop-blur-md"
          onMouseEnter={handleMouseEnter}
          onMouseLeave={handleMouseLeave}
        >
          {/* 카드 상단: 개체 타입 및 코드 + 상태 칩 */}
          <div className="flex items-center justify-between border-b border-[var(--border)] pb-2 mb-2">
            <div className="flex items-center gap-1.5">
              <span className="p-1 rounded bg-[var(--muted)] text-[var(--ink)]">
                <EntityIcon className="w-3.5 h-3.5 text-amber-600" />
              </span>
              <div className="leading-tight">
                <span className="text-[10px] text-[var(--faint)] block uppercase font-mono tracking-wider font-bold">
                  {kindLabel}
                </span>
                <span className="font-mono font-bold text-xs text-[var(--ink)]">{code}</span>
              </div>
            </div>

            {entityDetails?.status && (
              <StatusChip
                label={entityDetails.status}
                tone={
                  entityDetails.status === "승인완료" ||
                  entityDetails.status === "정상" ||
                  entityDetails.status === "완료" ||
                  entityDetails.status === "재직"
                    ? "ok"
                    : entityDetails.status === "결재대기" ||
                      entityDetails.status === "진행중" ||
                      entityDetails.status === "배차"
                    ? "info"
                    : entityDetails.status === "반려"
                    ? "danger"
                    : "warn"
                }
                size="xs"
              />
            )}
          </div>

          {/* 카드 바디: 작업 제목 및 맥락 정보 */}
          <div className="space-y-1.5 text-xs">
            <h4 className="font-semibold text-[var(--ink)] leading-snug line-clamp-2">
              {entityDetails?.title || `${code} 작업 대상 개체`}
            </h4>

            {entityDetails?.subtext && (
              <p className="text-[11px] text-[var(--steel)]">{entityDetails.subtext}</p>
            )}

            {entityDetails?.amount && (
              <div className="flex items-center justify-between text-[11px] pt-1 font-mono font-semibold text-emerald-700 bg-emerald-50/50 px-2 py-1 rounded">
                <span>인가/수령 금액</span>
                <span>{entityDetails.amount}</span>
              </div>
            )}
          </div>

          {/* 카드 하단: 1-Click 직관적 퀵 액션 */}
          <div className="mt-3 pt-2 border-t border-[var(--border)] flex items-center justify-between gap-1 text-[11px]">
            {prefix === "AP" && entityDetails?.canApprove && (
              <button
                type="button"
                onClick={(e) => {
                  e.stopPropagation();
                  approveDoc(entityDetails.rawId, "김현수 팀장", "Discord/Chat 인라인 1-클릭 승인");
                  setHovered(false);
                }}
                className="flex items-center gap-1 px-2 py-1 rounded bg-emerald-600 text-white hover:bg-emerald-700 font-bold transition-colors cursor-pointer"
              >
                <CheckCircle2 className="w-3 h-3" />
                <span>1-클릭 승인</span>
              </button>
            )}

            {prefix === "WO" && entityDetails?.status === "배차" && (
              <button
                type="button"
                onClick={(e) => {
                  e.stopPropagation();
                  updateWorkOrderStatus(entityDetails.rawId, "진행중");
                  setHovered(false);
                }}
                className="flex items-center gap-1 px-2 py-1 rounded bg-amber-600 text-white hover:bg-amber-700 font-bold transition-colors cursor-pointer"
              >
                <ArrowRight className="w-3 h-3" />
                <span>작업 착수</span>
              </button>
            )}

            <button
              type="button"
              onClick={(e) => {
                e.stopPropagation();
                openInspector(code);
                setHovered(false);
              }}
              className="ml-auto flex items-center gap-1 px-2 py-1 rounded hover:bg-[var(--muted)] text-[var(--steel)] hover:text-[var(--ink)] font-medium transition-colors cursor-pointer"
            >
              <span>작업 원장 검사</span>
              <ExternalLink className="w-2.5 h-2.5" />
            </button>
          </div>
        </div>
      )}
    </span>
  );
}
