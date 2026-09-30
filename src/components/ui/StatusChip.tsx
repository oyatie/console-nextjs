"use client";

import React from "react";
import { clsx } from "clsx";
import { twMerge } from "tailwind-merge";

export type ChipTone = "ok" | "warn" | "danger" | "info" | "brand" | "purple" | "neutral";

export interface StatusChipProps {
  label: string;
  tone?: ChipTone;
  size?: "xs" | "sm" | "md";
  dot?: boolean;
  className?: string;
}

export function StatusChip({
  label,
  tone,
  size = "sm",
  dot = false,
  className,
}: StatusChipProps) {
  // 자동 tone 추론 (지정되지 않은 경우)
  let resolvedTone: ChipTone = tone || "neutral";
  if (!tone) {
    if (["승인완료", "정상", "게시", "완료", "PERMIT"].includes(label)) resolvedTone = "ok";
    else if (["결재대기", "예외승인대기", "한도임박", "진행중", "OVERRIDE"].includes(label)) resolvedTone = "warn";
    else if (["반려", "미출근", "초과위반", "긴급", "DENY"].includes(label)) resolvedTone = "danger";
    else if (["초안", "연장", "접수", "대외비"].includes(label)) resolvedTone = "info";
    else if (["휴가", "비밀", "민감"].includes(label)) resolvedTone = "purple";
    else if (["종결", "퇴직", "일반"].includes(label)) resolvedTone = "neutral";
  }

  const toneStyles: Record<ChipTone, string> = {
    ok: "bg-[var(--ok-bg)] text-[var(--ok-tx)] border border-[var(--ok-bd)]",
    warn: "bg-[var(--warn-bg)] text-[var(--warn-tx)] border border-[var(--warn-bd)]",
    danger: "bg-[var(--danger-bg)] text-[var(--danger-tx)] border border-[var(--danger-bd)]",
    info: "bg-[var(--info-bg)] text-[var(--info-tx)] border border-[var(--info-bd)]",
    brand: "bg-[var(--accent-bg)] text-[var(--accent-tx)] border border-[var(--accent-bd)]",
    purple: "bg-[var(--purple-bg)] text-[var(--purple-tx)] border border-[var(--purple-bd)]",
    neutral: "bg-[var(--muted)] text-[var(--steel)] border border-[var(--border)]",
  };

  const dotColors: Record<ChipTone, string> = {
    ok: "bg-[var(--ok-solid)]",
    warn: "bg-[var(--warn-solid)]",
    danger: "bg-[var(--danger-solid)]",
    info: "bg-[var(--info-tx)]",
    brand: "bg-[var(--signal-deep)]",
    purple: "bg-[var(--purple-tx)]",
    neutral: "bg-[var(--steel)]",
  };

  const sizeStyles = {
    xs: "text-[11px] px-1.5 py-0.2 h-4.5 gap-1 font-medium",
    sm: "text-[11px] px-2 py-0.5 h-5 gap-1.5 font-semibold",
    md: "text-xs px-2.5 py-1 h-6 gap-1.5 font-semibold",
  };

  return (
    <span
      className={twMerge(
        clsx(
          "inline-flex items-center rounded-[5px] select-none shrink-0 font-mono tracking-tight",
          toneStyles[resolvedTone],
          sizeStyles[size],
          className
        )
      )}
    >
      {dot && <span className={clsx("h-1.5 w-1.5 rounded-full", dotColors[resolvedTone])} />}
      {label}
    </span>
  );
}
