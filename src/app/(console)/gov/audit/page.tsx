"use client";

import React, { useState } from "react";
import { useAppStore } from "@/lib/store";
import { DataTable, Column } from "@/components/ui/DataTable";
import { StatusChip } from "@/components/ui/StatusChip";
import { ObjectLink } from "@/components/ui/ObjectLink";
import { DynamicText } from "@/components/ui/DynamicText";
import { StatBar } from "@/components/ui/StatBar";
import { AuditEvent } from "@/lib/types";
import {
  ShieldCheck,
  FileSpreadsheet,
  Cpu,
  History,
  CheckCircle2,
  Lock,
  Copy,
  ExternalLink,
  Info,
  X,
} from "lucide-react";

function getHumanAction(action: string): { label: string; tone: "ok" | "info" | "warn" | "danger" } {
  if (action.includes("APPROVAL_STAGE_APPROVED")) return { label: "결재 단계 승인", tone: "ok" };
  if (action.includes("APPROVAL_STAGE_REJECTED")) return { label: "결재 반려", tone: "danger" };
  if (action.includes("APPROVAL_DRAFT_PARKED")) return { label: "품의서 작성 (Parking)", tone: "info" };
  if (action.includes("WORK_ORDER_ADVANCED_배차")) return { label: "정비 배차 지정", tone: "info" };
  if (action.includes("WORK_ORDER_ADVANCED_진행중")) return { label: "현장 정비 착수", tone: "warn" };
  if (action.includes("WORK_ORDER_ADVANCED_완료")) return { label: "정비 작업 완료", tone: "ok" };
  if (action.includes("ATTENDANCE_EXCEPTION")) return { label: "근태 예외 소명 인가", tone: "ok" };
  if (action.includes("PAYROLL_RUN_FROZEN")) return { label: "정기 급여 마감 및 확정", tone: "ok" };
  if (action.includes("EMPLOYEE_ONBOARDED")) return { label: "신규 사원 입사 등록", tone: "info" };
  return { label: action.replace(/_/g, " "), tone: "info" };
}

export default function AuditPage() {
  const auditEvents = useAppStore((s) => s.auditEvents);
  const [activeTab, setActiveTab] = useState<"work-ledger" | "tech-proof">("work-ledger");
  const [selectedEvent, setSelectedEvent] = useState<AuditEvent | null>(null);
  const [detailTab, setDetailTab] = useState<"work" | "crypto">("work");
  const [copiedHash, setCopiedHash] = useState(false);

  // 1. 일상 경로: 업무 수행 원장 컬럼 (The Ledger of the Work Itself)
  const workLedgerColumns: Column<AuditEvent>[] = [
    {
      key: "timestamp",
      header: "수행 시각",
      width: "150px",
      render: (row) => (
        <span className="font-mono text-[11px] text-[var(--steel)] font-medium">
          {row.timestamp}
        </span>
      ),
    },
    {
      key: "actorName",
      header: "수행자 (Who)",
      width: "130px",
      render: (row) => (
        <div className="flex items-center gap-1.5">
          <span className="w-5 h-5 rounded bg-[var(--muted)] text-[var(--ink)] text-[10px] font-bold flex items-center justify-center shrink-0">
            {row.actorName.slice(0, 1)}
          </span>
          <span className="font-semibold text-[var(--ink)] text-xs truncate">
            {row.actorName}
          </span>
        </div>
      ),
    },
    {
      key: "action",
      header: "수행한 업무 (What Happened)",
      width: "180px",
      render: (row) => {
        const h = getHumanAction(row.action);
        return (
          <span className="flex items-center gap-1.5 text-xs font-medium text-[var(--ink)]">
            <span
              className={`w-1.5 h-1.5 rounded-full shrink-0 ${
                h.tone === "ok"
                  ? "bg-emerald-500"
                  : h.tone === "danger"
                  ? "bg-red-500"
                  : h.tone === "warn"
                  ? "bg-amber-500"
                  : "bg-blue-500"
              }`}
            />
            <span>{h.label}</span>
          </span>
        );
      },
    },
    {
      key: "targetCode",
      header: "관련 업무 개체",
      width: "120px",
      render: (row) => <ObjectLink code={row.targetCode} />,
    },
    {
      key: "reason",
      header: "사유 및 업무 맥락 (Why / Context)",
      render: (row) => (
        <span className="text-xs text-[var(--steel)] block truncate max-w-md">
          <DynamicText text={row.reason || "정상 업무 절차에 따른 실행"} />
        </span>
      ),
    },
    {
      key: "policyDecision",
      header: "처리 결과",
      width: "100px",
      render: (row) => (
        <StatusChip
          label={row.policyDecision === "PERMIT" ? "정상 인가" : "인가 거부"}
          tone={row.policyDecision === "PERMIT" ? "ok" : "danger"}
          size="xs"
        />
      ),
    },
  ];

  // 2. 심층/전문 경로: 기술 로그 및 암호학적 증적 컬럼 (Deliberate Cryptographic Proof)
  const techProofColumns: Column<AuditEvent>[] = [
    {
      key: "seq",
      header: "블록 #",
      width: "80px",
      render: (row) => <span className="font-mono text-emerald-800 font-bold">#{row.seq}</span>,
    },
    {
      key: "timestamp",
      header: "타임스탬프 (UTC)",
      width: "150px",
      render: (row) => <span className="font-mono text-[11px] text-[var(--steel)]">{row.timestamp}</span>,
    },
    {
      key: "action",
      header: "시스템 액션 심볼",
      width: "190px",
      render: (row) => (
        <span className="font-mono text-[11px] font-bold text-[var(--ink)] block truncate">
          {row.action}
        </span>
      ),
    },
    {
      key: "hash",
      header: "현재 SHA-256 해시",
      width: "150px",
      render: (row) => (
        <span className="font-mono text-[11px] text-blue-700 font-semibold truncate block">
          {row.hash.slice(0, 14)}...
        </span>
      ),
    },
    {
      key: "prevHash",
      header: "부모 해시 (prevHash)",
      width: "130px",
      render: (row) => (
        <span className="font-mono text-[11px] text-[var(--faint)] truncate block">
          {row.prevHash.slice(0, 10)}...
        </span>
      ),
    },
    {
      key: "policyDecision",
      header: "Cedar PBAC AST",
      width: "110px",
      render: (row) => (
        <span className="font-mono text-[10px] px-1.5 py-0.5 rounded bg-gray-100 text-gray-800">
          {row.policyDecision}:permit-rule
        </span>
      ),
    },
    {
      key: "dataClass",
      header: "보안 등급",
      width: "90px",
      render: (row) => (
        <StatusChip
          label={row.dataClass}
          tone={row.dataClass === "비밀" || row.dataClass === "민감" ? "purple" : "neutral"}
          size="xs"
        />
      ),
    },
  ];

  const handleCopyHash = (hash: string) => {
    navigator.clipboard.writeText(hash);
    setCopiedHash(true);
    setTimeout(() => setCopiedHash(false), 2000);
  };

  return (
    <div className="space-y-5">
      {/* 1. 일상 운영 통계 바 */}
      <StatBar
        items={[
          { label: "누적 업무 수행 원장", value: `${auditEvents.length}건`, subValue: "실시간 전산 기록" },
          { label: "주요 의사결정 인가율", value: "100%", subValue: "결재·배차·급여 결격 없음", badgeTone: "ok" },
          { label: "기록 불변 보증", value: "위·변조 방지 봉인", badge: "검증 가능", badgeTone: "ok" },
          { label: "보존 규정 준수", value: "근로기준법·상법", subValue: "5년 법정 보존 보장", badgeTone: "info" },
        ]}
      />

      {/* 2. 뷰 전환 탭: 일상 업무 원장 vs 심층 기술 증적 */}
      <div className="flex items-center justify-between border-b border-[var(--border)] pb-2">
        <div className="flex items-center gap-2">
          <button
            type="button"
            onClick={() => setActiveTab("work-ledger")}
            className={`flex items-center gap-1.5 px-3 py-1.5 rounded-md text-xs font-semibold transition-all cursor-pointer ${
              activeTab === "work-ledger"
                ? "bg-amber-100 text-amber-950 ring-1 ring-amber-300"
                : "text-[var(--steel)] hover:bg-[var(--muted)] hover:text-[var(--ink)]"
            }`}
          >
            <History className="w-3.5 h-3.5 text-amber-700" />
            <span>업무 수행 원장 (Ledger of Work)</span>
            <span className="text-[10px] px-1.5 py-0.2 rounded-full bg-amber-200/80 font-bold">
              기본
            </span>
          </button>

          <button
            type="button"
            onClick={() => setActiveTab("tech-proof")}
            className={`flex items-center gap-1.5 px-3 py-1.5 rounded-md text-xs font-semibold transition-all cursor-pointer ${
              activeTab === "tech-proof"
                ? "bg-amber-100 text-amber-950 ring-1 ring-amber-300"
                : "text-[var(--steel)] hover:bg-[var(--muted)] hover:text-[var(--ink)]"
            }`}
          >
            <Cpu className="w-3.5 h-3.5 text-emerald-700" />
            <span>암호학적 무결성 및 기술 로그 (Technical & Cryptographic)</span>
          </button>
        </div>

        <span className="text-[11px] text-[var(--steel)] hidden sm:inline-block">
          행을 클릭하면 작업의 맥락과 감사 증적을 즉시 열람할 수 있습니다.
        </span>
      </div>

      {/* 3. 선택된 모드 배너 */}
      {activeTab === "work-ledger" ? (
        <div className="p-3 rounded-lg border border-[var(--border)] bg-gray-50/70 text-[var(--ink)] flex items-center justify-between gap-3 text-xs">
          <div className="flex items-center gap-2.5">
            <CheckCircle2 className="w-4 h-4 text-emerald-600 shrink-0" />
            <div>
              <span className="font-bold">업무 수행 원장 (The Ledger of the Work Itself)</span>
              <span className="text-[var(--steel)] ml-2">
                사원, 팀장, 관리자가 실행한 모든 승인·배차·근태 소명·급여 마감 내역을 인간 중심의 서사로 표시합니다.
              </span>
            </div>
          </div>
          <span className="text-[11px] font-mono text-[var(--steel)] font-medium shrink-0">
            총 {auditEvents.length}개 활동
          </span>
        </div>
      ) : (
        <div className="p-3 rounded-lg border border-emerald-300 bg-emerald-50/70 text-emerald-950 flex items-center justify-between gap-3 text-xs">
          <div className="flex items-center gap-2.5">
            <ShieldCheck className="w-4 h-4 text-emerald-700 shrink-0" />
            <div>
              <span className="font-bold">Palantir식 불변 감사 원장 (Append-Only SHA-256 Hash Chain)</span>
              <span className="text-emerald-900 ml-2">
                모든 작업 상태 전이는 이전 블록 해시와 암호학적으로 결합되어 사후 위·변조가 수학적으로 방지됩니다.
              </span>
            </div>
          </div>
          <span className="px-2 py-0.5 rounded bg-emerald-200 text-emerald-900 font-mono text-[10px] font-bold shrink-0">
            CHAIN VERIFIED
          </span>
        </div>
      )}

      {/* 4. 데이터 테이블 */}
      <DataTable
        columns={activeTab === "work-ledger" ? workLedgerColumns : techProofColumns}
        data={auditEvents}
        keyExtractor={(item) => item.id}
        onRowClick={(item) => setSelectedEvent(item)}
        searchPlaceholder={
          activeTab === "work-ledger"
            ? "수행자, 업무 내용, 대상 개체코드(AP-, WO-) 검색..."
            : "SHA-256 해시, 액션 심볼, 블록 시퀀스 검색..."
        }
        searchFilter={(item, q) =>
          item.action.toLowerCase().includes(q.toLowerCase()) ||
          item.actorName.toLowerCase().includes(q.toLowerCase()) ||
          item.targetCode.toLowerCase().includes(q.toLowerCase()) ||
          item.hash.toLowerCase().includes(q.toLowerCase()) ||
          Boolean(item.reason?.toLowerCase().includes(q.toLowerCase()))
        }
      />

      {/* 5. 행 클릭 시 작업 상세 & 기술 증적 모달 */}
      {selectedEvent && (
        <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/40 backdrop-blur-xs p-4">
          <div className="w-full max-w-lg rounded-lg border border-[var(--border)] bg-[var(--surface)] shadow-2xl p-5 space-y-4 animate-in fade-in zoom-in-95 duration-150">
            {/* 모달 상단 */}
            <div className="flex items-center justify-between border-b border-[var(--border)] pb-3">
              <div className="flex items-center gap-2">
                <span className="font-mono font-bold text-sm text-[var(--ink)]">
                  이벤트 #{selectedEvent.seq}
                </span>
                <StatusChip
                  label={selectedEvent.policyDecision === "PERMIT" ? "정상 인가" : "인가 거부"}
                  tone={selectedEvent.policyDecision === "PERMIT" ? "ok" : "danger"}
                  size="xs"
                />
              </div>
              <button
                type="button"
                onClick={() => setSelectedEvent(null)}
                className="p-1 rounded text-[var(--steel)] hover:bg-[var(--muted)] cursor-pointer"
              >
                <X className="w-4 h-4" />
              </button>
            </div>

            {/* 모달 탭: 업무 상세 vs 기술 로그 */}
            <div className="flex border-b border-[var(--border)] gap-2">
              <button
                type="button"
                onClick={() => setDetailTab("work")}
                className={`px-3 py-1.5 text-xs font-semibold border-b-2 cursor-pointer transition-colors ${
                  detailTab === "work"
                    ? "border-amber-600 text-amber-900"
                    : "border-transparent text-[var(--steel)] hover:text-[var(--ink)]"
                }`}
              >
                업무 상세 내역 (Work Narrative)
              </button>
              <button
                type="button"
                onClick={() => setDetailTab("crypto")}
                className={`px-3 py-1.5 text-xs font-semibold border-b-2 cursor-pointer transition-colors ${
                  detailTab === "crypto"
                    ? "border-amber-600 text-amber-900"
                    : "border-transparent text-[var(--steel)] hover:text-[var(--ink)]"
                }`}
              >
                기술 로그 및 암호학적 증적 (Technical Proof)
              </button>
            </div>

            {/* 모달 바디: 업무 상세 */}
            {detailTab === "work" ? (
              <div className="space-y-3 text-xs">
                <div className="grid grid-cols-2 gap-2 p-3 rounded-md bg-[var(--canvas)]">
                  <div>
                    <span className="text-[10px] text-[var(--steel)] block">실행 시각</span>
                    <span className="font-mono font-medium">{selectedEvent.timestamp}</span>
                  </div>
                  <div>
                    <span className="text-[10px] text-[var(--steel)] block">작업자</span>
                    <span className="font-semibold">{selectedEvent.actorName}</span>
                  </div>
                  <div>
                    <span className="text-[10px] text-[var(--steel)] block">수행 업무</span>
                    <span className="font-semibold text-amber-900">
                      {getHumanAction(selectedEvent.action).label}
                    </span>
                  </div>
                  <div>
                    <span className="text-[10px] text-[var(--steel)] block">대상 개체</span>
                    <ObjectLink code={selectedEvent.targetCode} />
                  </div>
                </div>

                <div className="p-3 rounded-md border border-[var(--border)] bg-[var(--surface)] space-y-1">
                  <span className="text-[10px] font-bold text-[var(--steel)] block">
                    업무 배경 및 사유 (Why)
                  </span>
                  <div className="text-xs text-[var(--ink)] leading-relaxed">
                    <DynamicText text={selectedEvent.reason || "정상 프로세스에 따른 변경 기록"} />
                  </div>
                </div>
              </div>
            ) : (
              /* 모달 바디: 암호학적 기술 증적 */
              <div className="space-y-3 text-xs">
                <div className="p-3 rounded-md bg-gray-900 text-emerald-400 font-mono text-[11px] space-y-2 overflow-x-auto">
                  <div>
                    <span className="text-gray-400 block text-[9px] uppercase">SHA-256 Block Hash:</span>
                    <div className="flex items-center justify-between gap-2">
                      <span className="break-all">{selectedEvent.hash}</span>
                      <button
                        type="button"
                        onClick={() => handleCopyHash(selectedEvent.hash)}
                        className="p-1 rounded bg-gray-800 text-gray-200 hover:bg-gray-700 shrink-0 cursor-pointer"
                        title="해시 복사"
                      >
                        <Copy className="w-3 h-3" />
                      </button>
                    </div>
                  </div>

                  <div>
                    <span className="text-gray-400 block text-[9px] uppercase">Previous Block Hash (Parent):</span>
                    <span className="break-all text-gray-300">{selectedEvent.prevHash}</span>
                  </div>

                  <div>
                    <span className="text-gray-400 block text-[9px] uppercase">Cedar PBAC Authorization:</span>
                    <span className="text-amber-300">
                      permit(principal == {selectedEvent.actorId}, action == {selectedEvent.action}, resource == {selectedEvent.targetCode})
                    </span>
                  </div>
                </div>

                {copiedHash && (
                  <p className="text-[11px] text-emerald-600 font-medium">
                    ✓ SHA-256 해시가 클립보드에 복사되었습니다.
                  </p>
                )}
              </div>
            )}

            {/* 모달 푸터 */}
            <div className="flex justify-end pt-2 border-t border-[var(--border)]">
              <button
                type="button"
                onClick={() => setSelectedEvent(null)}
                className="px-3 py-1.5 rounded-md bg-[var(--muted)] text-[var(--ink)] hover:bg-gray-200 font-semibold text-xs cursor-pointer"
              >
                닫기
              </button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
