"use client";

import React from "react";
import { useAppStore } from "@/lib/store";
import { Wrench, FileCheck2, User, Clock, Layers } from "lucide-react";
import { StatusChip } from "./StatusChip";

interface EntityMentionPickerProps {
  query: string;
  triggerType: "#" | "@" | "[";
  onSelect: (replacementText: string) => void;
  onClose: () => void;
}

export function EntityMentionPicker({
  query,
  triggerType,
  onSelect,
  onClose,
}: EntityMentionPickerProps) {
  const workOrders = useAppStore((s) => s.workOrders);
  const approvals = useAppStore((s) => s.approvals);
  const employees = useAppStore((s) => s.employees);
  const attendance = useAppStore((s) => s.attendance);

  const cleanQuery = query.toLowerCase();

  // Filter based on trigger
  let items: Array<{
    code: string;
    title: string;
    subtext: string;
    icon: any;
    tokenText: string;
    status?: string;
  }> = [];

  if (triggerType === "@") {
    // Person mentions
    items = employees
      .filter((e) => e.name.toLowerCase().includes(cleanQuery) || e.dept.toLowerCase().includes(cleanQuery))
      .map((e) => ({
        code: e.code,
        title: `${e.name} (${e.role})`,
        subtext: `${e.dept} · ${e.site}`,
        icon: User,
        tokenText: `@${e.name}`,
        status: e.status,
      }));
  } else {
    // Entity links (# or [)
    const woItems = workOrders
      .filter(
        (w) =>
          w.code.toLowerCase().includes(cleanQuery) ||
          w.title.toLowerCase().includes(cleanQuery) ||
          w.site.toLowerCase().includes(cleanQuery)
      )
      .map((w) => ({
        code: w.code,
        title: w.title,
        subtext: `${w.site} · ${w.assignedTo}`,
        icon: Wrench,
        tokenText: `[${w.code}]`,
        status: w.status,
      }));

    const apItems = approvals
      .filter(
        (a) =>
          a.code.toLowerCase().includes(cleanQuery) ||
          a.title.toLowerCase().includes(cleanQuery) ||
          a.drafterName.toLowerCase().includes(cleanQuery)
      )
      .map((a) => ({
        code: a.code,
        title: a.title,
        subtext: `기안: ${a.drafterName} · ${a.category}`,
        icon: FileCheck2,
        tokenText: `[${a.code}]`,
        status: a.status,
      }));

    const atItems = attendance
      .filter(
        (at) =>
          at.exceptionCode &&
          (at.exceptionCode.toLowerCase().includes(cleanQuery) ||
            at.employeeName.toLowerCase().includes(cleanQuery))
      )
      .map((at) => ({
        code: at.exceptionCode!,
        title: `${at.employeeName} 근태 예외`,
        subtext: `${at.site} · ${at.workedHours}h`,
        icon: Clock,
        tokenText: `[${at.exceptionCode}]`,
        status: at.status,
      }));

    items = [...woItems, ...apItems, ...atItems];
  }

  if (items.length === 0) return null;

  return (
    <div className="absolute z-50 bottom-full left-0 mb-2 w-80 max-h-60 overflow-y-auto rounded-lg border border-[var(--border)] bg-[var(--surface)] shadow-2xl p-1 text-xs animate-in fade-in zoom-in-95 duration-100">
      <div className="px-2 py-1 border-b border-[var(--border)] text-[10px] font-bold text-[var(--steel)] flex items-center justify-between">
        <span>{triggerType === "@" ? "사원 / 담당자 멘션" : "개체 엔진 동적 연결 (Discord 스타일)"}</span>
        <span className="font-mono text-[9px]">ESC 닫기</span>
      </div>

      <div className="py-1 space-y-0.5">
        {items.slice(0, 6).map((item) => {
          const Icon = item.icon;
          return (
            <button
              key={item.code}
              type="button"
              onClick={() => onSelect(item.tokenText)}
              className="w-full flex items-center justify-between p-2 rounded hover:bg-[var(--muted)] text-left transition-colors cursor-pointer group"
            >
              <div className="flex items-center gap-2 min-w-0">
                <span className="p-1 rounded bg-[var(--muted)] group-hover:bg-amber-100 text-amber-900 shrink-0">
                  <Icon className="w-3.5 h-3.5" />
                </span>
                <div className="min-w-0">
                  <div className="flex items-center gap-1.5">
                    <span className="font-mono font-bold text-[var(--ink)]">{item.code}</span>
                    <span className="font-medium text-[var(--ink)] truncate">{item.title}</span>
                  </div>
                  <span className="text-[10px] text-[var(--steel)] block truncate">{item.subtext}</span>
                </div>
              </div>

              {item.status && (
                <span className="shrink-0 ml-2">
                  <StatusChip label={item.status} size="xs" />
                </span>
              )}
            </button>
          );
        })}
      </div>
    </div>
  );
}
