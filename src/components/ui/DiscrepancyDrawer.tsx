"use client";

import React, { useState } from "react";
import { PayrollDiscrepancy } from "@/lib/types";
import { Button } from "./Button";
import {
  AlertTriangle,
  Clock,
  ShieldAlert,
  CheckCircle2,
  X,
  ArrowRight,
  FileCheck,
} from "lucide-react";

interface DiscrepancyDrawerProps {
  isOpen: boolean;
  onClose: () => void;
  discrepancy: PayrollDiscrepancy | null;
  onRepair: (id: string, resolutionComment: string) => void;
}

export function DiscrepancyDrawer({
  isOpen,
  onClose,
  discrepancy,
  onRepair,
}: DiscrepancyDrawerProps) {
  const [resolutionNote, setResolutionNote] = useState("현장 긴급 복구 작업에 따른 연장근로 사전 소명 인정 및 특별인가 승인");
  const [submitting, setSubmitting] = useState(false);

  if (!isOpen || !discrepancy) return null;

  const handleExecuteRepair = () => {
    setSubmitting(true);
    setTimeout(() => {
      onRepair(discrepancy.id, resolutionNote);
      setSubmitting(false);
      onClose();
    }, 400);
  };

  return (
    <div className="fixed inset-0 z-50 flex justify-end bg-black/40 backdrop-blur-xs animate-in fade-in duration-150">
      <div className="w-full max-w-md h-full bg-[var(--surface)] border-l border-[var(--border)] shadow-2xl p-6 flex flex-col justify-between overflow-y-auto animate-in slide-in-from-right duration-200">
        <div className="space-y-5">
          {/* 상단 헤더 */}
          <div className="flex items-center justify-between border-b border-[var(--border)] pb-3">
            <div className="flex items-center gap-2">
              <div className="w-8 h-8 rounded-lg bg-amber-500/15 text-amber-700 flex items-center justify-center font-bold">
                <AlertTriangle className="w-4 h-4" />
              </div>
              <div>
                <span className="font-mono text-xs font-bold text-amber-800">
                  {discrepancy.sourceCode}
                </span>
                <h3 className="text-sm font-bold text-[var(--ink)]">
                  근태 예외 및 급여 마감 블로커 심사
                </h3>
              </div>
            </div>
            <button
              type="button"
              onClick={onClose}
              className="text-gray-400 hover:text-gray-600 cursor-pointer p-1 rounded hover:bg-gray-100"
            >
              <X className="w-4 h-4" />
            </button>
          </div>

          {/* 대상 정보 카드 */}
          <div className="p-3 rounded-lg border border-[var(--border)] bg-gray-50 space-y-1 text-xs">
            <div className="flex justify-between">
              <span className="text-[var(--steel)]">대상 사원:</span>
              <span className="font-bold text-[var(--ink)]">{discrepancy.employeeName}</span>
            </div>
            <div className="flex justify-between">
              <span className="text-[var(--steel)]">귀속 급여 회차:</span>
              <span className="font-mono">{discrepancy.payrollRunId} (2026-07)</span>
            </div>
            <div className="flex justify-between">
              <span className="text-[var(--steel)]">블로커 심각도:</span>
              <span className="font-bold text-red-700 font-mono">CRITICAL BLOCKER</span>
            </div>
          </div>

          {/* 법정 사규 근거 설명 */}
          <div className="space-y-1.5 text-xs">
            <span className="font-bold text-[var(--steel)] block text-[11px] uppercase tracking-wider">
              법정 노동법 및 취업규칙 기준:
            </span>
            <div className="p-3 rounded-lg border border-amber-300 bg-amber-50/70 space-y-2 text-amber-950">
              <div className="font-bold flex items-center gap-1.5">
                <ShieldAlert className="w-4 h-4 text-amber-700 shrink-0" />
                <span>{discrepancy.statutoryRule}</span>
              </div>
              <p className="text-[11px] text-amber-900 leading-relaxed">
                {discrepancy.explanation}
              </p>
            </div>
          </div>

          {/* 실근무시간 vs. 법정한도 비교 박스 */}
          <div className="grid grid-cols-2 gap-3 text-xs">
            <div className="p-3 rounded-lg border border-amber-300 bg-amber-50 text-center space-y-1">
              <span className="text-[10px] text-amber-800 font-medium block">
                실제 누적 근무시간
              </span>
              <span className="text-xl font-black font-mono text-amber-950">
                54.5h
              </span>
              <span className="text-[10px] text-amber-700 font-semibold block">
                +2.5h 법정 초과
              </span>
            </div>
            <div className="p-3 rounded-lg border border-[var(--border)] bg-gray-50 text-center space-y-1">
              <span className="text-[10px] text-[var(--steel)] font-medium block">
                주간 법정 한도
              </span>
              <span className="text-xl font-black font-mono text-[var(--ink)]">
                52.0h
              </span>
              <span className="text-[10px] text-[var(--steel)] block">
                기본 40h + 연장 12h
              </span>
            </div>
          </div>

          {/* 타임카드 일별 상세 내역 */}
          <div className="space-y-1.5 text-xs">
            <span className="font-bold text-[var(--steel)] block text-[11px] uppercase tracking-wider">
              해당 주차 타임카드 상세 내역:
            </span>
            <div className="rounded-lg border border-[var(--border)] divide-y divide-[var(--border-soft)] text-[11px]">
              <div className="p-2 flex justify-between">
                <span>07-13 (월) 정상출근</span>
                <span className="font-mono">09:00 - 18:00 (8.0h)</span>
              </div>
              <div className="p-2 flex justify-between">
                <span>07-14 (화) 야간연장</span>
                <span className="font-mono">09:00 - 22:00 (12.0h)</span>
              </div>
              <div className="p-2 flex justify-between">
                <span>07-15 (수) 정상출근</span>
                <span className="font-mono">09:00 - 18:00 (8.0h)</span>
              </div>
              <div className="p-2 flex justify-between bg-amber-50/50 text-amber-900 font-semibold">
                <span>07-16 (목) 긴급복구 연장 (초과)</span>
                <span className="font-mono text-amber-800 font-bold">09:00 - 23:30 (13.5h)</span>
              </div>
              <div className="p-2 flex justify-between">
                <span>07-17 (금) 정상출근</span>
                <span className="font-mono">09:00 - 18:00 (8.0h)</span>
              </div>
              <div className="p-2 flex justify-between bg-gray-50 text-[var(--steel)]">
                <span>07-18 (토) 휴일비상대기</span>
                <span className="font-mono">10:00 - 15:30 (5.0h)</span>
              </div>
            </div>
          </div>

          {/* 소명 사유 및 승인 의견 입력란 */}
          <div className="space-y-1.5 text-xs">
            <label className="font-bold text-[var(--ink)] block">
              승인 사유 및 대체조치 지정:
            </label>
            <textarea
              rows={2}
              value={resolutionNote}
              onChange={(e) => setResolutionNote(e.target.value)}
              className="w-full rounded-md border border-[var(--border)] p-2 text-xs text-[var(--ink)] focus:outline-none focus:border-[var(--signal-deep)]"
            />
          </div>
        </div>

        {/* 액션 버튼 바 */}
        <div className="pt-4 border-t border-[var(--border)] space-y-2">
          <Button
            variant="brand"
            size="md"
            className="w-full justify-center bg-emerald-700 hover:bg-emerald-800 text-white"
            onClick={handleExecuteRepair}
            disabled={submitting}
            leftIcon={<CheckCircle2 className="w-4 h-4" />}
          >
            {submitting ? "소명 승인 처리 중..." : discrepancy.repairActionLabel}
          </Button>
          <Button
            variant="secondary"
            size="sm"
            className="w-full justify-center"
            onClick={onClose}
          >
            닫기
          </Button>
        </div>
      </div>
    </div>
  );
}
