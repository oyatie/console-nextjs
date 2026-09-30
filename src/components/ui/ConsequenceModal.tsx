"use client";

import React from "react";
import { ConsequenceSummary } from "@/lib/types";
import { Button } from "./Button";
import { ShieldCheck, AlertCircle, ArrowRight, X } from "lucide-react";

interface ConsequenceModalProps {
  isOpen: boolean;
  onClose: () => void;
  onConfirm: () => void;
  summary: ConsequenceSummary;
  loading?: boolean;
}

export function ConsequenceModal({
  isOpen,
  onClose,
  onConfirm,
  summary,
  loading = false,
}: ConsequenceModalProps) {
  if (!isOpen) return null;

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/60 backdrop-blur-xs p-4 animate-in fade-in duration-150">
      <div className="w-full max-w-lg rounded-xl border border-[var(--border)] bg-[var(--surface)] shadow-2xl p-6 space-y-5 animate-in zoom-in-95 duration-150">
        {/* 헤더 */}
        <div className="flex items-start justify-between border-b border-[var(--border)] pb-3.5">
          <div className="space-y-1">
            <div className="flex items-center gap-2">
              <span className="px-2 py-0.5 rounded text-[10px] font-mono font-bold bg-amber-100 text-amber-900 border border-amber-300">
                실행 전 영향도 검토
              </span>
              <span className="text-xs font-mono text-[var(--steel)]">
                효력발생: {summary.effectiveDate}
              </span>
            </div>
            <h3 className="text-sm font-bold text-[var(--ink)]">
              {summary.title}
            </h3>
          </div>
          <button
            type="button"
            onClick={onClose}
            className="text-gray-400 hover:text-gray-600 cursor-pointer p-1 rounded hover:bg-gray-100"
          >
            <X className="w-4 h-4" />
          </button>
        </div>

        {/* 대상 객체 배너 */}
        <div className="p-3 rounded-lg bg-gray-50 border border-[var(--border)] flex items-center justify-between text-xs">
          <div>
            <span className="text-[10px] text-[var(--steel)] block">적용 대상</span>
            <span className="font-bold text-[var(--ink)]">{summary.subject}</span>
          </div>
          <div className="text-right">
            <span className="text-[10px] text-[var(--steel)] block">업무 구분</span>
            <span className="font-semibold text-blue-700">{summary.actionType}</span>
          </div>
        </div>

        {/* 상세 비즈니스 영향 요약 (Plain Business Language) */}
        <div className="space-y-2 text-xs">
          <span className="font-bold text-[var(--steel)] block text-[11px] uppercase tracking-wider">
            변경 시 시스템 및 법정 파급 효과 (Consequences):
          </span>
          <div className="space-y-2 rounded-lg border border-[var(--border)] p-3.5 bg-[var(--canvas)]">
            {summary.summaryLines.map((line, idx) => (
              <div key={idx} className="flex items-start gap-2 text-[var(--ink)]">
                <ArrowRight className="w-3.5 h-3.5 text-blue-600 shrink-0 mt-0.5" />
                <span className="leading-relaxed">{line}</span>
              </div>
            ))}
          </div>
        </div>

        {/* 과거 이력 보존 안내 */}
        <div className="p-3 rounded-lg bg-blue-50/70 border border-blue-200 text-[11px] text-blue-900 space-y-1">
          <div className="flex items-center gap-1.5 font-bold">
            <ShieldCheck className="w-4 h-4 text-blue-700 shrink-0" />
            <span>이력 보존 및 거버넌스 보증</span>
          </div>
          <p className="leading-relaxed pl-5 text-[11px]">
            {summary.historicalImpact}
          </p>
        </div>

        {/* 결재선 필요 여부 */}
        {summary.requiresApproval && (
          <div className="flex items-center gap-2 p-2.5 rounded bg-amber-50 border border-amber-200 text-xs text-amber-900">
            <AlertCircle className="w-4 h-4 text-amber-700 shrink-0" />
            <span>
              본 작업은 즉시 단독 변경되지 않으며, <strong>{summary.approverRole}</strong>의 승인 결재를 거쳐 최종 발효됩니다.
            </span>
          </div>
        )}

        {/* 액션 버튼 바: 명시적 결과 명명 버튼 (Consequence-Naming Buttons) */}
        <div className="flex items-center justify-end gap-2.5 pt-2 border-t border-[var(--border)]">
          <Button variant="secondary" size="sm" onClick={onClose} disabled={loading}>
            취소하고 돌아가기
          </Button>
          <Button
            variant="brand"
            size="sm"
            onClick={onConfirm}
            disabled={loading}
          >
            {loading ? "처리 및 서명 중..." : summary.consequenceButtonLabel}
          </Button>
        </div>
      </div>
    </div>
  );
}
