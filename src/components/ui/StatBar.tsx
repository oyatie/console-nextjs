"use client";

import React from "react";
import { clsx } from "clsx";

export interface StatItem {
  label: string;
  value: string | number;
  subValue?: string;
  badge?: string;
  badgeTone?: "ok" | "warn" | "danger" | "info";
}

export interface StatBarProps {
  items: StatItem[];
  className?: string;
}

export function StatBar({ items, className }: StatBarProps) {
  return (
    <div
      className={clsx(
        "flex flex-wrap items-center divide-x divide-[var(--border)] rounded-[6px] border border-[var(--border)] bg-[var(--surface)] text-[12px] shadow-2xs overflow-hidden",
        className
      )}
    >
      {items.map((item, idx) => (
        <div key={idx} className="flex items-center gap-2 px-3.5 py-2 grow min-w-[140px]">
          <span className="text-[var(--steel)] font-medium">{item.label}</span>
          <span className="font-mono font-bold text-[14px] text-[var(--ink)] tracking-tight">
            {item.value}
          </span>
          {item.badge && (
            <span
              className={clsx(
                "px-1.5 py-0.2 rounded text-[10px] font-mono font-semibold",
                item.badgeTone === "danger" && "bg-red-50 text-red-700 border border-red-200",
                item.badgeTone === "warn" && "bg-amber-50 text-amber-800 border border-amber-200",
                item.badgeTone === "ok" && "bg-emerald-50 text-emerald-800 border border-emerald-200",
                (!item.badgeTone || item.badgeTone === "info") && "bg-blue-50 text-blue-800 border border-blue-200"
              )}
            >
              {item.badge}
            </span>
          )}
          {item.subValue && <span className="text-[11px] text-[var(--faint)] ml-auto">{item.subValue}</span>}
        </div>
      ))}
    </div>
  );
}
